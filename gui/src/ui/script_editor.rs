//! The structured, per-speaker script editor.
//!
//! A **lens** over `state.draft`, not parallel state: every mutation here
//! parses the current script into [`Block`]s, changes one, and re-serializes
//! straight back into a plain string through `oninput`. `script_page.rs`'s
//! `synthesize()` and its `hosts_ok` gate (`script_stats`, over in
//! `components.rs`) both keep reading `state.draft` exactly as before and
//! need no changes — they cannot tell this editor apart from someone typing
//! into a plain textarea.
//!
//! Two named risks were checked against this machine's wry/WebKitGTK
//! backend in `bin/reorder_smoke.rs` before this was written, and both
//! landed on the plan's own pre-approved fallback:
//!
//! - **Reorder** ships as move-up/move-down buttons, not HTML5 `draggable`.
//!   `drop_smoke.rs`'s own doc comment is the reason this backend cannot be
//!   trusted by default, and this sandbox has no input-injection or
//!   screenshot tooling to drive and observe a real drag interaction — so
//!   there is no way to *earn* confidence in the risky path here. Shipping
//!   the pre-approved fallback is the honest choice over shipping something
//!   unverified. `reorder_smoke.rs`'s draggable rows are kept for whoever
//!   next wants to spend the ten minutes proving it and swapping this out.
//! - **Split-at-caret** ships with the `document::eval` read of
//!   `selectionStart` as the primary path, and the plan's own fallback
//!   (split at the end) as what a failed or empty read decays to — in the
//!   same call, not behind a flag. A flaky eval degrades gracefully instead
//!   of being silently skipped.

use dioxus::prelude::*;

use crate::ui::components::{parse_turn_line, script_stats, Select};
use crate::ui::icons::{IconCaretDown, IconCaretUp, IconTrash};

/// One piece of a script, in editing order.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Turn { speaker: u32, text: String },
    /// One or more consecutive lines that did not match `Speaker N: <text>`.
    /// Shown flagged rather than dropped — dropping malformed lines is
    /// `validate_script`'s job, on the Python side, right before synthesis;
    /// the editor's job is to show the user everything that is actually on
    /// disk so nothing disappears without them choosing that.
    Raw { text: String },
}

/// Split a script into blocks. `render_blocks` is its exact inverse for any
/// script this produces — see the round-trip tests below.
pub fn parse_blocks(script: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut raw: Vec<&str> = Vec::new();

    for line in script.lines() {
        match parse_turn_line(line) {
            Some((speaker, text)) => {
                if !raw.is_empty() {
                    blocks.push(Block::Raw { text: raw.join("\n") });
                    raw.clear();
                }
                blocks.push(Block::Turn { speaker, text: text.to_string() });
            }
            None => raw.push(line),
        }
    }
    if !raw.is_empty() {
        blocks.push(Block::Raw { text: raw.join("\n") });
    }
    blocks
}

/// Join blocks back into the plain-text form the pipeline reads.
pub fn render_blocks(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|b| match b {
            Block::Turn { speaker, text } => format!("Speaker {speaker}: {text}"),
            Block::Raw { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Re-parse `script`, apply one change, and re-serialize. Every mutation in
/// this file goes through here, so none of them can drift from what
/// `parse_blocks`/`render_blocks` agree a script looks like.
fn apply(script: &str, mutate: impl FnOnce(&mut Vec<Block>)) -> String {
    let mut blocks = parse_blocks(script);
    mutate(&mut blocks);
    render_blocks(&blocks)
}

fn reassign(blocks: &mut [Block], idx: usize, speaker: u32) {
    if let Some(Block::Turn { speaker: s, .. }) = blocks.get_mut(idx) {
        *s = speaker;
    }
}

fn move_up(blocks: &mut [Block], idx: usize) {
    if idx > 0 && idx < blocks.len() {
        blocks.swap(idx, idx - 1);
    }
}

fn move_down(blocks: &mut [Block], idx: usize) {
    if idx + 1 < blocks.len() {
        blocks.swap(idx, idx + 1);
    }
}

fn delete_block(blocks: &mut Vec<Block>, idx: usize) {
    if idx < blocks.len() {
        blocks.remove(idx);
    }
}

/// Merge a turn into the one after it, keeping this turn's speaker. Only
/// defined between two `Turn`s — the caller gates the button on that with
/// [`can_merge_at`], so this quietly no-ops rather than being reachable on
/// a `Raw` block.
fn merge_with_next(blocks: &mut Vec<Block>, idx: usize) {
    let Some(Block::Turn { text: next_text, .. }) = blocks.get(idx + 1).cloned() else {
        return;
    };
    let Some(Block::Turn { speaker, text }) = blocks.get(idx).cloned() else {
        return;
    };
    let merged = match (text.is_empty(), next_text.is_empty()) {
        (true, _) => next_text,
        (_, true) => text,
        _ => format!("{text} {next_text}"),
    };
    blocks[idx] = Block::Turn { speaker, text: merged };
    blocks.remove(idx + 1);
}

fn can_merge_at(blocks: &[Block], idx: usize) -> bool {
    matches!(blocks.get(idx), Some(Block::Turn { .. }))
        && matches!(blocks.get(idx + 1), Some(Block::Turn { .. }))
}

/// Split one turn into two turns of the same speaker, at `at_chars`
/// characters into the text. `at_chars` is clamped to the text's own
/// length, which is what turns an out-of-range or missing caret offset
/// (the failed-eval case) into the plan's named fallback: split at the end,
/// leaving the new turn empty rather than guessing where the words break.
///
/// Char-indexed rather than byte-indexed for the same reason as
/// `components::truncate_chars`: a generated script routinely carries
/// em dashes and curly quotes, and slicing on a byte offset inside one of
/// those panics.
fn split_turn(blocks: &mut Vec<Block>, idx: usize, at_chars: usize) {
    let Some(Block::Turn { speaker, text }) = blocks.get(idx).cloned() else {
        return;
    };
    let at = at_chars.min(text.chars().count());
    let head: String = text.chars().take(at).collect();
    let tail: String = text.chars().skip(at).collect();
    blocks.splice(
        idx..=idx,
        [
            Block::Turn { speaker, text: head.trim_end().to_string() },
            Block::Turn { speaker, text: tail.trim_start().to_string() },
        ],
    );
}

/// Read a turn textarea's caret position out of the DOM.
///
/// `document::eval` is dioxus's own desktop/webview bridge for running a
/// fixed, hand-written script against the page — not an interpreter over
/// untrusted input. The only interpolated value is this editor's own
/// integer turn index, not anything a user typed.
async fn caret_offset(idx: usize) -> Option<usize> {
    let script = format!(
        "const el = document.getElementById('turn-{idx}'); \
         return el ? el.selectionStart : null;"
    );
    document::eval(&script).await.ok()?.as_u64().map(|n| n as usize)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Structured,
    Raw,
}

/// Which edge of the editor a turn's bubble hugs. Independent of the
/// speaker-color `slot` computation below — this only ever affects layout.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

impl Side {
    fn class(self) -> &'static str {
        match self {
            Side::Left => "turn-left",
            Side::Right => "turn-right",
        }
    }
}

/// Odd speakers (1, 3) align left; even speakers (2, 4) align right. Holds
/// for 2-, 3-, or 4-host scripts alike.
fn align_side(speaker: u32) -> Side {
    if speaker % 2 == 1 { Side::Left } else { Side::Right }
}

#[derive(Props, Clone, PartialEq)]
pub struct ScriptEditorProps {
    pub value: String,
    /// A validation failure from the pipeline, shown inline rather than in a
    /// dialog so the text it refers to stays on screen.
    pub error: Option<String>,
    /// The run's configured host count — the reassign menu's valid range,
    /// and what decides a turn's "out of range" styling.
    pub hosts: u8,
    pub oninput: EventHandler<String>,
}

#[component]
pub fn ScriptEditor(props: ScriptEditorProps) -> Element {
    let mut mode = use_signal(|| Mode::Structured);
    // Which turn's delete is armed, and which turn is in edit mode. Indices
    // shift under every mutation, so every handler that changes the block
    // list clears both — see `commit`.
    let armed = use_signal(|| None::<usize>);
    let editing = use_signal(|| None::<usize>);
    let stats = script_stats(&props.value);
    let blocks = parse_blocks(&props.value);
    let total = blocks.len();

    rsx! {
        div { class: "script-editor",
            div { class: "script-editor-modes",
                button {
                    class: if *mode.read() == Mode::Structured { "mode-btn active" } else { "mode-btn" },
                    onclick: move |_| mode.set(Mode::Structured),
                    "Structured"
                }
                button {
                    class: if *mode.read() == Mode::Raw { "mode-btn active" } else { "mode-btn" },
                    onclick: move |_| mode.set(Mode::Raw),
                    "Raw"
                }
            }

            if *mode.read() == Mode::Raw {
                textarea {
                    class: "input input-mono script-editor-raw",
                    spellcheck: false,
                    value: "{props.value}",
                    oninput: move |e: Event<FormData>| props.oninput.call(e.value().to_string()),
                }
            } else if blocks.is_empty() {
                div { class: "empty-state",
                    div { class: "empty-state-text", "No turns yet." }
                }
            } else {
                div { class: "script-turns",
                    for idx in 0..total {
                        TurnRow {
                            key: "{idx}",
                            idx,
                            block: blocks[idx].clone(),
                            hosts: props.hosts,
                            can_merge: can_merge_at(&blocks, idx),
                            can_move_up: idx > 0,
                            can_move_down: idx + 1 < total,
                            value: props.value.clone(),
                            oninput: props.oninput,
                            armed,
                            editing,
                        }
                    }
                }
            }

            div { class: "script-meta",
                span { "{stats.turns} turns" }
                span { "{stats.words} words" }
                span { "{stats.speakers} speakers" }
            }
            if let Some(err) = &props.error {
                div { class: "script-error", "{err}" }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct TurnRowProps {
    idx: usize,
    block: Block,
    hosts: u8,
    can_merge: bool,
    can_move_up: bool,
    can_move_down: bool,
    /// The whole script, so every mutation can re-parse the latest version
    /// rather than one captured at render time. Cheap: scripts here run to
    /// a few kilobytes, not megabytes.
    value: String,
    oninput: EventHandler<String>,
    armed: Signal<Option<usize>>,
    editing: Signal<Option<usize>>,
}

/// Clear the delete-arm and the edit-in-progress turn, then hand the parent
/// the new script in one step, so no mutation handler can forget any of the
/// three — indices shift under every mutation (move/delete/merge/split).
fn commit(
    mut armed: Signal<Option<usize>>,
    mut editing: Signal<Option<usize>>,
    oninput: EventHandler<String>,
    new_value: String,
) {
    armed.set(None);
    editing.set(None);
    oninput.call(new_value);
}

/// Ignore the mousedown that precedes a toolbar button's click, so the
/// browser's default "clicking anywhere blurs the focused control" behavior
/// doesn't fire `onblur` on the actively-edited textarea before the click
/// handler runs. Without this, clicking Split on a turn currently being
/// edited would silently fall back to split-at-end, because `editing` (and
/// the `#turn-{idx}` element `caret_offset` looks for) would already be gone
/// by the time the click handler ran.
fn keep_focus(e: Event<MouseData>) {
    e.prevent_default();
}

#[component]
fn TurnRow(props: TurnRowProps) -> Element {
    let mut armed = props.armed;
    let mut editing = props.editing;
    let is_armed = *armed.read() == Some(props.idx);
    let idx = props.idx;

    match &props.block {
        Block::Turn { speaker, text } => {
            let speaker = *speaker;
            let text = text.clone();
            let out_of_range = speaker == 0 || speaker > props.hosts as u32;
            let slot = ((speaker.saturating_sub(1)) % 4) + 1;
            let side = align_side(speaker);
            let stripe_color = if out_of_range {
                "var(--danger)".to_string()
            } else {
                format!("var(--speaker-{slot})")
            };
            let is_editing = *editing.read() == Some(idx);
            let row_class = format!(
                "script-turn {}{}{}",
                side.class(),
                if out_of_range { " out-of-range" } else { "" },
                if is_editing { " turn-editing" } else { "" },
            );

            rsx! {
                div { class: "{row_class}",
                    div {
                        class: "script-turn-stripe",
                        style: "background: {stripe_color}",
                    }
                    div {
                        class: "script-turn-body",
                        onclick: move |_| editing.set(Some(idx)),
                        div { class: "script-turn-label", "Speaker {speaker}" }
                        if is_editing {
                            textarea {
                                id: "turn-{idx}",
                                class: "script-turn-text",
                                spellcheck: false,
                                value: "{text}",
                                onmounted: move |evt: Event<MountedData>| {
                                    spawn(async move {
                                        let _ = evt.set_focus(true).await;
                                    });
                                },
                                onblur: move |_| editing.set(None),
                                oninput: {
                                    let value = props.value.clone();
                                    let oninput = props.oninput;
                                    move |e: Event<FormData>| {
                                        let new_text = e.value().to_string();
                                        let new_script = apply(&value, |blocks| {
                                            if let Some(Block::Turn { text, .. }) = blocks.get_mut(idx) {
                                                *text = new_text.clone();
                                            }
                                        });
                                        oninput.call(new_script);
                                    }
                                },
                            }
                        } else {
                            div { class: "script-turn-text", "{text}" }
                        }
                    }
                    div { class: "script-turn-actions",
                        button {
                            class: "turn-action-btn",
                            title: "Move earlier",
                            disabled: !props.can_move_up,
                            onmousedown: keep_focus,
                            onclick: {
                                let value = props.value.clone();
                                let oninput = props.oninput;
                                move |_| commit(armed, editing, oninput, apply(&value, |b| move_up(b, idx)))
                            },
                            IconCaretUp { size: 14 }
                        }
                        button {
                            class: "turn-action-btn",
                            title: "Move later",
                            disabled: !props.can_move_down,
                            onmousedown: keep_focus,
                            onclick: {
                                let value = props.value.clone();
                                let oninput = props.oninput;
                                move |_| commit(armed, editing, oninput, apply(&value, |b| move_down(b, idx)))
                            },
                            IconCaretDown { size: 14 }
                        }
                        Select {
                            value: speaker.to_string(),
                            options: (1..=props.hosts).map(|n| (n.to_string(), format!("Speaker {n}"))).collect(),
                            onchange: {
                                let value = props.value.clone();
                                let oninput = props.oninput;
                                move |v: String| {
                                    if let Ok(n) = v.parse::<u32>() {
                                        commit(armed, editing, oninput, apply(&value, |b| reassign(b, idx, n)));
                                    }
                                }
                            },
                        }
                        button {
                            class: "turn-action-btn",
                            title: "Split this turn at the cursor",
                            onmousedown: keep_focus,
                            onclick: {
                                let value = props.value.clone();
                                let oninput = props.oninput;
                                move |_| {
                                    let value = value.clone();
                                    spawn(async move {
                                        let at = caret_offset(idx).await.unwrap_or(usize::MAX);
                                        commit(armed, editing, oninput, apply(&value, |b| split_turn(b, idx, at)));
                                    });
                                }
                            },
                            "Split"
                        }
                        button {
                            class: "turn-action-btn",
                            title: "Merge with the next turn",
                            disabled: !props.can_merge,
                            onmousedown: keep_focus,
                            onclick: {
                                let value = props.value.clone();
                                let oninput = props.oninput;
                                move |_| commit(armed, editing, oninput, apply(&value, |b| merge_with_next(b, idx)))
                            },
                            "Merge"
                        }
                        button {
                            class: if is_armed { "turn-action-btn remove armed" } else { "turn-action-btn remove" },
                            title: if is_armed { "Click again to delete" } else { "Delete" },
                            onmousedown: keep_focus,
                            onclick: {
                                let value = props.value.clone();
                                let oninput = props.oninput;
                                move |_| {
                                    if !is_armed {
                                        armed.set(Some(idx));
                                        return;
                                    }
                                    commit(armed, editing, oninput, apply(&value, |b| delete_block(b, idx)));
                                }
                            },
                            IconTrash { size: 14 }
                        }
                    }
                }
            }
        }
        Block::Raw { text } => {
            let text = text.clone();
            rsx! {
                div { class: "script-raw",
                    div {
                        class: "script-turn-stripe",
                        style: "background: var(--speaker-1)",
                    }
                    div { class: "script-turn-body",
                        div { class: "script-turn-label", "Not part of any speaker's turn" }
                        textarea {
                            id: "turn-{idx}",
                            class: "script-turn-text",
                            spellcheck: false,
                            value: "{text}",
                            oninput: {
                                let value = props.value.clone();
                                let oninput = props.oninput;
                                move |e: Event<FormData>| {
                                    let new_text = e.value().to_string();
                                    let new_script = apply(&value, |blocks| {
                                        if let Some(Block::Raw { text }) = blocks.get_mut(idx) {
                                            *text = new_text.clone();
                                        }
                                    });
                                    oninput.call(new_script);
                                }
                            },
                        }
                    }
                    div { class: "script-turn-actions",
                        button {
                            class: "turn-action-btn",
                            title: "Move earlier",
                            disabled: !props.can_move_up,
                            onclick: {
                                let value = props.value.clone();
                                let oninput = props.oninput;
                                move |_| commit(armed, editing, oninput, apply(&value, |b| move_up(b, idx)))
                            },
                            IconCaretUp { size: 14 }
                        }
                        button {
                            class: "turn-action-btn",
                            title: "Move later",
                            disabled: !props.can_move_down,
                            onclick: {
                                let value = props.value.clone();
                                let oninput = props.oninput;
                                move |_| commit(armed, editing, oninput, apply(&value, |b| move_down(b, idx)))
                            },
                            IconCaretDown { size: 14 }
                        }
                        button {
                            class: if is_armed { "turn-action-btn remove armed" } else { "turn-action-btn remove" },
                            title: if is_armed { "Click again to delete" } else { "Delete" },
                            onclick: {
                                let value = props.value.clone();
                                let oninput = props.oninput;
                                move |_| {
                                    if !is_armed {
                                        armed.set(Some(idx));
                                        return;
                                    }
                                    commit(armed, editing, oninput, apply(&value, |b| delete_block(b, idx)));
                                }
                            },
                            IconTrash { size: 14 }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "Speaker 1: Hello there.\nSpeaker 2: Hi!\nSpeaker 1: How are you?";

    #[test]
    fn a_well_formed_script_parses_into_one_turn_per_line() {
        let blocks = parse_blocks(SCRIPT);
        assert_eq!(
            blocks,
            vec![
                Block::Turn { speaker: 1, text: "Hello there.".into() },
                Block::Turn { speaker: 2, text: "Hi!".into() },
                Block::Turn { speaker: 1, text: "How are you?".into() },
            ]
        );
    }

    #[test]
    fn render_blocks_is_the_exact_inverse_of_parse_blocks() {
        assert_eq!(render_blocks(&parse_blocks(SCRIPT)), SCRIPT);
    }

    #[test]
    fn a_malformed_line_is_flagged_rather_than_dropped() {
        let script = "Speaker 1: fine\nsome stray note\nSpeaker 2: also fine";
        let blocks = parse_blocks(script);
        assert_eq!(
            blocks,
            vec![
                Block::Turn { speaker: 1, text: "fine".into() },
                Block::Raw { text: "some stray note".into() },
                Block::Turn { speaker: 2, text: "also fine".into() },
            ]
        );
        // Nothing is lost: re-serializing still contains the stray line.
        assert_eq!(render_blocks(&blocks), script);
    }

    #[test]
    fn consecutive_malformed_lines_become_one_raw_block_not_several() {
        let script = "Speaker 1: fine\nnote one\nnote two\nSpeaker 2: also fine";
        let blocks = parse_blocks(script);
        assert_eq!(
            blocks,
            vec![
                Block::Turn { speaker: 1, text: "fine".into() },
                Block::Raw { text: "note one\nnote two".into() },
                Block::Turn { speaker: 2, text: "also fine".into() },
            ]
        );
    }

    #[test]
    fn reassigning_a_turn_changes_only_that_turns_speaker() {
        let out = apply(SCRIPT, |b| reassign(b, 1, 1));
        assert_eq!(out, "Speaker 1: Hello there.\nSpeaker 1: Hi!\nSpeaker 1: How are you?");
    }

    #[test]
    fn moving_a_turn_up_swaps_it_with_its_predecessor() {
        let out = apply(SCRIPT, |b| move_up(b, 1));
        assert_eq!(out, "Speaker 2: Hi!\nSpeaker 1: Hello there.\nSpeaker 1: How are you?");
    }

    #[test]
    fn moving_the_first_turn_up_is_a_no_op() {
        assert_eq!(apply(SCRIPT, |b| move_up(b, 0)), SCRIPT);
    }

    #[test]
    fn moving_the_last_turn_down_is_a_no_op() {
        assert_eq!(apply(SCRIPT, |b| move_down(b, 2)), SCRIPT);
    }

    #[test]
    fn deleting_a_turn_removes_only_that_turn() {
        let out = apply(SCRIPT, |b| delete_block(b, 1));
        assert_eq!(out, "Speaker 1: Hello there.\nSpeaker 1: How are you?");
    }

    #[test]
    fn merging_joins_two_turns_text_under_the_first_speaker() {
        let out = apply(SCRIPT, |b| merge_with_next(b, 0));
        assert_eq!(out, "Speaker 1: Hello there. Hi!\nSpeaker 1: How are you?");
    }

    #[test]
    fn merge_across_a_raw_block_is_refused() {
        let script = "Speaker 1: fine\nstray\nSpeaker 2: also fine";
        assert!(!can_merge_at(&parse_blocks(script), 0));
        // The mutation itself is a no-op too, not just the button's gate —
        // nothing calls it directly, but a stale click must still be safe.
        assert_eq!(apply(script, |b| merge_with_next(b, 0)), script);
    }

    #[test]
    fn splitting_at_a_character_offset_makes_two_turns_of_the_same_speaker() {
        let out = apply(SCRIPT, |b| split_turn(b, 0, 5));
        assert_eq!(out, "Speaker 1: Hello\nSpeaker 1: there.\nSpeaker 2: Hi!\nSpeaker 1: How are you?");
    }

    #[test]
    fn an_out_of_range_split_offset_falls_back_to_splitting_at_the_end() {
        // The named fallback for a failed or empty caret read: usize::MAX
        // clamps to the text's own length, so the new turn starts empty
        // rather than the split panicking or guessing.
        let out = apply(SCRIPT, |b| split_turn(b, 0, usize::MAX));
        assert_eq!(out, "Speaker 1: Hello there.\nSpeaker 1: \nSpeaker 2: Hi!\nSpeaker 1: How are you?");
    }

    #[test]
    fn splitting_never_panics_on_a_multi_byte_character_boundary() {
        let script = "Speaker 1: caf\u{e9} \u{2014} welcome";
        // Splitting inside the multi-byte "é" or the em dash by character
        // count must not land on a byte boundary that panics.
        for at in 0..script.chars().count() {
            let _ = apply(script, |b| split_turn(b, 0, at));
        }
    }

    #[test]
    fn a_full_edit_sequence_round_trips_to_a_script_stats_passing_result() {
        // reassign, split, merge, delete, move — chained — must still land
        // on a script script_stats agrees is well-formed two-host script.
        let mut script = SCRIPT.to_string();
        script = apply(&script, |b| split_turn(b, 0, 5));
        script = apply(&script, |b| reassign(b, 1, 2));
        script = apply(&script, |b| move_down(b, 0));
        script = apply(&script, |b| merge_with_next(b, 2));
        script = apply(&script, |b| delete_block(b, 0));

        let stats = script_stats(&script);
        assert_eq!(stats.speakers, 2, "the edited script must still be a clean 2-host script: {script:?}");
        assert!(stats.turns > 0);
    }
}
