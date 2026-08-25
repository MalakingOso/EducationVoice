//! Shared presentational components.
//!
//! Beamer's idiom throughout: thin class-only wrappers, no inline styles, no
//! variant enums, plain values in and `EventHandler<T>` out. The one place an
//! inline style is unavoidable is the progress fill's width, which is a
//! computed length and cannot be a class.
//!
//! `MaskedInput` is deliberately absent — this app holds no secrets.

use dioxus::prelude::*;

use crate::proto::Stage;
use crate::ui::run_state::StageState;

/// Truncate `text` to at most `max_chars` characters, appending an ellipsis
/// when anything was cut.
///
/// Counts **characters, not bytes**: `&text[..n]` panics when `n` lands inside
/// a multi-byte UTF-8 sequence, and both article titles and generated scripts
/// routinely carry em dashes and curly quotes.
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().nth(max_chars).is_some() {
        out.push_str("...");
    }
    out
}

/// What the editor reports back about the script in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScriptStats {
    /// Lines that are well-formed `Speaker N:` turns.
    pub turns: usize,
    pub words: usize,
    /// Distinct speaker ids seen, which is what has to match `--hosts`.
    pub speakers: usize,
}

/// Count turns, words and distinct speakers in a script.
///
/// Mirrors `validate_script`'s notion of a turn — a line beginning `Speaker N:`
/// — so the count the editor shows is the count the pipeline will agree with.
/// A mismatch here would show a happy turn count on a script that then fails
/// validation.
pub fn script_stats(script: &str) -> ScriptStats {
    let mut turns = 0;
    let mut words = 0;
    let mut ids: Vec<u32> = Vec::new();

    for line in script.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("Speaker ") else {
            continue;
        };
        let Some((digits, body)) = rest.split_once(':') else {
            continue;
        };
        let Ok(id) = digits.trim().parse::<u32>() else {
            continue;
        };
        turns += 1;
        words += body.split_whitespace().count();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }

    ScriptStats {
        turns,
        words,
        speakers: ids.len(),
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct CardProps {
    title: String,
    children: Element,
}

#[component]
pub fn Card(props: CardProps) -> Element {
    rsx! {
        div { class: "card",
            div { class: "card-title", "{props.title}" }
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct SelectProps {
    value: String,
    options: Vec<(String, String)>,
    onchange: EventHandler<String>,
}

#[component]
pub fn Select(props: SelectProps) -> Element {
    rsx! {
        select {
            class: "select",
            onchange: move |e: Event<FormData>| {
                props.onchange.call(e.value().to_string());
            },
            for (val, label) in &props.options {
                option { value: "{val}", selected: *val == props.value, "{label}" }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ToggleProps {
    value: bool,
    ontoggle: EventHandler<bool>,
}

#[component]
pub fn Toggle(props: ToggleProps) -> Element {
    let class = if props.value { "toggle active" } else { "toggle" };
    rsx! {
        div {
            class: "{class}",
            onclick: move |_| props.ontoggle.call(!props.value),
            div { class: "toggle-knob" }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct TagChipProps {
    label: String,
    onremove: EventHandler<()>,
}

#[component]
pub fn TagChip(props: TagChipProps) -> Element {
    rsx! {
        div { class: "tag-chip",
            span { "{props.label}" }
            button {
                class: "tag-remove",
                onclick: move |_| props.onremove.call(()),
                "\u{2715}"
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ProgressBarProps {
    /// 0.0 to 1.0. Callers pass this only when they hold a real denominator —
    /// see `Measured`, which is what decides whether this renders at all.
    value: f32,
}

/// The measured progress bar. Lives only inside the run strip, because its
/// fill is the one place `--phos` appears.
#[component]
pub fn ProgressBar(props: ProgressBarProps) -> Element {
    // Clamped again here rather than trusted: a NaN width silently collapses
    // the element instead of erroring, which is invisible in review.
    let pct = if props.value.is_finite() {
        (props.value * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    rsx! {
        div { class: "progress-track",
            div { class: "progress-fill", style: "width: {pct}%" }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct StageChipProps {
    stage: Stage,
    state: StageState,
}

/// Stage name plus its status dot.
///
/// "Running" and "finished" differ by motion rather than hue here: the accent
/// *is* green, so `--success` collapses into it and colour alone cannot carry
/// the difference. The pulse on `.running` is what does.
#[component]
pub fn StageChip(props: StageChipProps) -> Element {
    rsx! {
        div { class: "stage-chip",
            div { class: "stage-dot {props.state.dot_class()}" }
            span { class: "run-strip-stage", "{props.stage.label()}" }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ScriptEditorProps {
    value: String,
    /// A validation failure from the pipeline, shown inline rather than in a
    /// dialog so the text it refers to stays on screen.
    error: Option<String>,
    oninput: EventHandler<String>,
}

#[component]
pub fn ScriptEditor(props: ScriptEditorProps) -> Element {
    let stats = script_stats(&props.value);
    rsx! {
        div { class: "script-editor",
            textarea {
                class: "input input-mono",
                spellcheck: false,
                value: "{props.value}",
                oninput: move |e: Event<FormData>| props.oninput.call(e.value().to_string()),
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

#[cfg(test)]
#[path = "components_tests.rs"]
mod tests;
