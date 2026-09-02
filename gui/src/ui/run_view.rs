//! The dark run view: what the Run page becomes while a run is under way.
//!
//! It replaces the launcher outright rather than growing inside it, and it
//! covers `.content` edge to edge up to the rail. The mode change is the
//! point: the app looks like a different kind of object while it is working,
//! and the change is carried by the ground going near-black, the type going
//! sparse and letter-spaced, and a dot matrix building the dark out from nine
//! scattered origins.
//!
//! **One orange element.** `--mark-orange` marks the live stage's pip and
//! nothing else, so the accent means "this is the part that is moving" rather
//! than being decoration spread across the panel. The armed stop button is the
//! one exception, and it takes the orange *from* the pip for as long as it is
//! armed rather than introducing a second one.
//!
//! **Nothing here draws a fraction.** `max_steps` is an upper bound the
//! generation loop breaks out of early, so the waveform is indeterminate at
//! every stage and the only number on screen is the clock.

use std::time::{Duration, Instant};

use dioxus::prelude::*;

use crate::proto::Stage;
use crate::runner;
use crate::ui::app::AppState;
use crate::ui::run_state::{format_elapsed, StageState};

/// Columns and rows of the dissolve matrix.
///
/// Fixed rather than measured: the grid is laid out with `repeat(N, 1fr)`, so
/// the cells stretch with the window and no Rust code ever has to know the
/// view's pixel size. The content area is roughly 824x596 at the default
/// window, so 42x30 lands close to square cells of about 20px — fine enough to
/// read as a matrix rather than as masonry, coarse enough to stay at 1260
/// nodes that are built once and never diffed again.
const COLS: usize = 42;
const ROWS: usize = 30;

/// Where the fronts are thrown from, in grid fractions.
///
/// Nine fixed positions, none of them under the Start button: the dissolve
/// reads as the whole panel changing state rather than as something spreading
/// from a control. Fixed rather than randomised per run so the app dissolves
/// the same way every time, the way a device does, and so a screenshot of it
/// is reproducible.
const SEEDS: [(f32, f32, f32); 9] = [
    // x, y, delay — the small delays are what make fronts of different ages
    // collide, which is what curves the seams.
    (0.12, 0.18, 0.00),
    (0.68, 0.09, 0.06),
    (0.91, 0.34, 0.02),
    (0.34, 0.44, 0.09),
    (0.77, 0.62, 0.00),
    (0.08, 0.71, 0.05),
    (0.58, 0.74, 0.00),
    (0.28, 0.95, 0.11),
    (0.95, 0.90, 0.04),
];

/// How much of a cell's delay is random rather than distance.
///
/// Pure distance produces clean expanding arcs, which is the one thing a dot
/// matrix must not look like. This is the share of the threshold that breaks
/// the front into a ragged edge.
const JITTER: f32 = 0.18;

/// The content area's width over its height at the default window.
///
/// Cell coordinates are normalised 0..1 on both axes, so distance in that
/// space is distance in an anisotropically squashed plane. Scaling x by this
/// measures in something closer to screen space, which is what keeps a front
/// round instead of stretching it into an ellipse.
const ASPECT: f32 = 1.38;

/// How long the dissolve takes to paint the dark in, and to take it away.
///
/// These are resolved in Rust and emitted as literal `animation-delay` values
/// per cell. They were `calc(var(--d) * var(--dissolve-in))` in the stylesheet
/// and the whole dissolve fired at once: if a `calc()` is not accepted in an
/// animation property the declaration is invalid at computed-value time and
/// falls back to the property's initial value, `0s` — which is not an error,
/// just 1260 cells turning on the same frame.
///
/// `DISSOLVE_OUT_MS` must stay equal to `EXIT_DISSOLVE` in `app.rs`. That is
/// the sleep that holds this view up after a successful run, and if it is the
/// shorter of the two the view is torn down mid-retreat.
const DISSOLVE_IN_MS: u32 = 1800;
const DISSOLVE_OUT_MS: u32 = 500;

/// The activity blocks: how long each takes to run its shape, how far into
/// that shape it starts, and which of the three shapes it runs.
///
/// Seven, not forty. Forty thin bars read as a *waveform*, and a waveform is
/// a picture of a signal being sampled, which would imply something in this
/// pipeline is being measured. Seven blocks opening and closing out of step
/// read as seven things being worked at, which is what is true. The row is
/// centred rather than sitting on a baseline, so each block grows away from
/// its own middle in both directions and never reads as a quantity.
///
/// A fixed table rather than a hash, for the same reason `SEEDS` is fixed:
/// this is part of what the app looks like, and it should look the same every
/// run, the way a device does.
///
/// The motion has to read as random without being random. No two durations
/// share a simple ratio, so the blocks never fall back into step and the row
/// never repeats a pose you could recognise.
///
/// The offsets are negative, which starts each block partway into its own
/// shape. Without them all seven would open from the same height on the first
/// frame, which is the marching wave this replaces.
const BARS: [(u32, i32, u8); 7] = [
    // duration, offset into the shape, shape
    (1180, -180, 0),
    (1870, -940, 2),
    (1330, -420, 1),
    (2090, -1550, 0),
    (1450, -700, 2),
    (1690, -1210, 1),
    (1240, -300, 2),
];

/// The shape a block runs: the heights it moves between, and in what order.
///
/// Three rather than one. Seven blocks running the same shape are one
/// mechanism however their phases are staggered; three different shapes make
/// them separate ones.
fn bar_shape(shape: u8) -> &'static str {
    match shape {
        0 => "run-bar-a",
        1 => "run-bar-b",
        _ => "run-bar-c",
    }
}

/// How often the reasoning feed reveals one more character.
///
/// About 40 a second, which is fast enough that a several-paragraph plan is
/// through in roughly ten seconds and slow enough to read as arriving.
const TYPE_TICK: Duration = Duration::from_millis(25);

/// One deterministic value per cell, in 0..1.
///
/// A hash rather than an RNG so the pattern is identical between runs and
/// between builds: the dissolve is part of the app's identity, and a matrix
/// that scrambles itself on every launch would read as noise rather than as
/// a signature.
fn jitter_at(i: usize) -> f32 {
    let mut h = (i as u32).wrapping_mul(2_654_435_761);
    h ^= h >> 15;
    h = h.wrapping_mul(2_246_822_519);
    h ^= h >> 13;
    (h % 1000) as f32 / 1000.0
}

/// When each cell turns, normalised so the last one lands exactly at 1.0.
///
/// Every cell takes its moment from the seed that reaches it *soonest in
/// time*, not the one nearest in space: a late seed loses to a distant early
/// one until its own delay has elapsed, which is what makes the seams move
/// instead of sitting on a fixed boundary.
///
/// Normalising is what keeps nine seeds from simply looking faster than one.
/// It stretches the field across the *whole* duration rather than scaling it,
/// so the first cell turns at 0 and the last at 1 whatever the seed layout.
/// Dividing by the maximum alone is not enough: the jitter is added to every
/// cell including the ones sitting on a seed, so the earliest cell would start
/// late by however much jitter it happened to draw, and the dissolve would
/// open with a visible pause.
fn delays() -> Vec<f32> {
    let mut out = Vec::with_capacity(COLS * ROWS);

    for r in 0..ROWS {
        for c in 0..COLS {
            let x = (c as f32 + 0.5) / COLS as f32;
            let y = (r as f32 + 0.5) / ROWS as f32;
            let mut best = f32::INFINITY;
            for (sx, sy, delay) in SEEDS {
                let dx = (x - sx) * ASPECT;
                let dy = y - sy;
                let v = delay + (dx * dx + dy * dy).sqrt();
                if v < best {
                    best = v;
                }
            }
            out.push(best + jitter_at(r * COLS + c) * JITTER);
        }
    }

    let min = out.iter().copied().fold(f32::MAX, f32::min);
    let max = out.iter().copied().fold(f32::MIN, f32::max);
    let span = max - min;
    if span > 0.0 {
        for t in &mut out {
            *t = (*t - min) / span;
        }
    }
    out
}

/// A cell's normalised moment turned into the literal the DOM is given.
///
/// A function rather than arithmetic buried in the `rsx!` format string so a
/// test can assert what actually reaches the attribute. The cast truncates,
/// which is what puts the first cell at exactly 0ms and the last at exactly
/// the span.
fn delay_ms(d: f32, span_ms: u32) -> u32 {
    (d * span_ms as f32) as u32
}

/// The dot matrix that paints the dark in and out.
///
/// Its one prop is the direction, which is what Dioxus memoises on: the parent
/// re-renders every second to move the clock and never diffs these 1260 nodes,
/// and this component re-renders exactly once per run, at the flip.
///
/// The direction has to be a prop rather than purely a `.leaving` rule in CSS
/// because each cell now carries a literal delay in milliseconds, and the two
/// directions want different literals.
///
/// The reveal is a CSS animation rather than a transition. A transition needs
/// its start and end states in separate frames to fire at all, which on mount
/// means scheduling a second render purely to make the first one animate; an
/// animation just runs.
#[component]
fn DissolveGrid(leaving: bool) -> Element {
    let cells = use_signal(delays);
    let span = if leaving { DISSOLVE_OUT_MS } else { DISSOLVE_IN_MS };

    rsx! {
        div { class: "dissolve",
            for (i, d) in cells.read().iter().enumerate() {
                // Inline, and a literal: an inline declaration beats the
                // stylesheet's, so `.dissolve i` supplies everything about the
                // animation except when it starts.
                i { key: "{i}", style: "animation-delay:{delay_ms(*d, span)}ms" }
            }
        }
    }
}

/// The reasoning feed, revealed a character at a time.
///
/// Its own component because it re-renders forty times a second while typing,
/// and that has to stay off the checklist, the waveform and the matrix — all
/// of which are memoised siblings that would otherwise be diffed on every
/// character.
///
/// A new message cuts rather than queues. Claude's turns are the model
/// narrating its current thinking, so the newest one is the only one worth
/// having on screen; a queue would put the feed further behind the run the
/// longer the run went on.
#[component]
fn Typewriter(text: String) -> Element {
    // The message being typed. It is a signal rather than just the prop
    // because the future below is spawned once and outlives every message: it
    // has to see the message that is current now, not the one that was on
    // screen when it started.
    let mut full = use_signal(|| text.clone());
    let mut revealed = use_signal(|| 0usize);

    // Reset during render rather than from a `use_effect`, so the new message
    // is never painted at the old message's length for one frame. This
    // converges immediately: the next render finds the two equal.
    let arrived = *full.read() != text;
    if arrived {
        full.set(text.clone());
        revealed.set(0);
    }

    use_future(move || async move {
        loop {
            tokio::time::sleep(TYPE_TICK).await;
            let n = *revealed.peek();
            // Guarded, because a signal write is a re-render: without this the
            // component would re-render forty times a second forever on a
            // message that finished typing minutes ago.
            if n < full.peek().chars().count() {
                revealed.set(n + 1);
            }
        }
    });

    let n = *revealed.read();
    let shown: String = text.chars().take(n).collect();

    rsx! {
        div { class: "run-feed",
            // One child, not two: `.run-feed` is a flex column so that it can
            // be bottom-anchored, and a bare caret beside the text would be a
            // second flex item and land on its own line.
            span { class: "run-feed-text",
                "{shown}"
                // Only while there is more to come, so a finished message is
                // not left blinking at the reader.
                if n < text.chars().count() {
                    span { class: "run-caret" }
                }
            }
        }
    }
}

/// The label under each checklist pip.
fn row_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Ingest => "Ingest",
        Stage::Script => "Script",
        Stage::Tts => "Synth",
        Stage::Unknown => "Working",
    }
}

/// The class suffix a checklist row draws with.
fn row_class(state: StageState) -> &'static str {
    match state {
        StageState::Pending => "run-step wait",
        StageState::Running => "run-step live",
        StageState::Done => "run-step done",
        StageState::Failed => "run-step failed",
    }
}

#[component]
pub fn RunView(state: AppState) -> Element {
    let mut state = state;
    // Subscribing to the tick is what re-renders the clock; the value itself
    // is never read.
    let _tick = *state.tick.read();

    let run = state.run.read().clone();
    let running = run.is_running();

    // Arm-then-confirm, as the titlebar close does, because a misclick here
    // throws away a run that may be ten minutes in. It expires rather than
    // sitting armed: while armed the live pip goes grey, and that pip is the
    // view's only sign the run is alive.
    let mut armed = use_signal(|| false);
    let mut refusal = use_signal(|| None::<String>);

    // The armed state has to clear itself; nothing else would. Every tick is a
    // second, so three ticks is the timeout.
    let mut armed_at = use_signal(|| 0u64);
    if *armed.read() && _tick.saturating_sub(*armed_at.read()) >= 3 {
        armed.set(false);
    }

    let title = run
        .title
        .clone()
        .unwrap_or_else(|| state.source.read().clone());
    let elapsed = format_elapsed(run.elapsed(Instant::now()));
    let is_armed = *armed.read();

    // The feed collapses rather than reserving space: `article2pod.py` emits
    // no message events during ingest or synth, so on an auto-continue run
    // this panel would otherwise sit empty through the longest stage.
    let message = run.last_message.clone();

    rsx! {
        div {
            // `.leaving` reverses the dissolve and drops the solid ground, so
            // the matrix retreats and the launcher shows through behind it.
            class: if running { "run-view" } else { "run-view leaving" },
            // The run view still sits where the drop zone was, so a file
            // dragged onto it still fires. Refusing out loud is the rule this
            // page already follows: a gesture that silently changes what the
            // *next* run would use is the failure the drop zone was designed
            // against.
            ondragover: move |e| e.prevent_default(),
            ondrop: move |e: Event<DragData>| {
                e.prevent_default();
                refusal.set(Some("A run is under way.".to_string()));
            },

            // The solid ground arrives just as the last cells land, sealing
            // the hairlines a fractional grid leaves between them. It cannot
            // simply be the view's own background: the matrix paints in
            // --run-black, so a black ground underneath would make the whole
            // dissolve invisible.
            div { class: "run-ground" }
            DissolveGrid { leaving: !running }

            div { class: "run-view-body",
              div { class: "run-view-inner",
                div { class: "run-view-top",
                    div { class: "run-view-title", "{title}" }
                    div { class: "run-view-clock", "{elapsed}" }
                }

                div { class: "run-steps",
                    for stage in run.stages.iter().copied() {
                        div {
                            key: "{stage:?}",
                            class: row_class(run.state_of(stage)),
                            // Greyed while the stop is armed: the accent is
                            // on the button for as long as the question is
                            // open, so there is still only ever one.
                            span { class: if is_armed { "run-pip disarmed" } else { "run-pip" } }
                            span { class: "run-step-name", "{row_label(stage)}" }
                            if run.state_of(stage) == StageState::Running {
                                span { class: "run-step-detail", "{run.detail}" }
                            }
                        }
                    }
                }

                // Indeterminate at every stage. There is no denominator worth
                // drawing anywhere in this pipeline, so this says "working"
                // and never "how far".
                div { class: "run-wave",
                    for (n, (dur, offset, shape)) in BARS.iter().copied().enumerate() {
                        i {
                            key: "{n}",
                            class: bar_shape(shape),
                            // Longhands only, and inline. The stylesheet names
                            // the shape and nothing about the timing, so there
                            // is no shorthand anywhere that could reset these.
                            style: "animation-duration:{dur}ms;animation-delay:{offset}ms",
                        }
                    }
                }

                if let Some(text) = message {
                    Typewriter { text }
                }

                if let Some(why) = refusal.read().clone() {
                    div { class: "run-refusal", "{why}" }
                }

                if let Some(err) = run.last_error.clone() {
                    div { class: "run-view-error", "{err}" }
                }

                div { class: "run-view-actions",
                    if running {
                        button {
                            class: if is_armed { "run-stop armed" } else { "run-stop" },
                            onclick: move |_| {
                                if !*armed.peek() {
                                    armed.set(true);
                                    armed_at.set(_tick);
                                    return;
                                }
                                armed.set(false);
                                if let Some(pgid) = *state.pgid.peek() {
                                    // The whole process group: the `claude`
                                    // CLI runs as a grandchild, and killing
                                    // only the Python pid would leave it
                                    // running and still spending tokens.
                                    runner::cancel(pgid);
                                }
                            },
                            if is_armed { "Stop the run\u{2009}?" } else { "Stop" }
                        }
                    } else {
                        // Only a failed or cancelled run reaches here: a
                        // successful one reverses the dissolve and returns to
                        // the launcher on its own.
                        button {
                            class: "run-stop",
                            onclick: move |_| {
                                state.run.write().stage = None;
                                refusal.set(None);
                            },
                            "Dismiss"
                        }
                    }
                }
              }
            }
        }
    }
}

#[cfg(test)]
#[path = "run_view_tests.rs"]
mod tests;
