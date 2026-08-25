//! The run strip: one dark instrument surface along the bottom of the window.
//!
//! It sits below `.app-body` rather than inside a page, so it stays visible
//! while the Library is being browsed mid-synthesis.
//!
//! This is the only place `--phos` appears. `#1AFC44` has relative luminance
//! 0.70 — 1.4:1 against white — so it is legible only against `--inst`. A
//! stylesheet test enforces that; this module is the reason the rule is
//! keepable.

use std::time::Instant;

use dioxus::prelude::*;

use crate::proto::{Measured, Stage};
use crate::runner;
use crate::ui::app::AppState;
use crate::ui::components::{ProgressBar, StageChip};
use crate::ui::icons::IconStop;
use crate::ui::run_state::format_elapsed;

#[component]
pub fn RunStrip(state: AppState) -> Element {
    // Subscribing to the tick is what re-renders the clock; the value itself
    // is never read for anything.
    let _tick = *state.tick.read();

    let run = state.run.read().clone();
    let running = run.is_running();

    // Idle *and* never run: nothing to say. After a run ends the strip stays
    // up showing its final state, which is where the outcome is read.
    if !running && run.stage.is_none() {
        return rsx! { div { class: "run-strip hidden" } };
    }

    let stage = run.stage.unwrap_or(Stage::Unknown);
    let elapsed = format_elapsed(run.elapsed(Instant::now()));

    rsx! {
        div { class: "run-strip",
            StageChip { stage, state: run.state }
            div { class: "run-strip-detail", "{run.detail}" }

            // The bar exists only when a real denominator does. Script
            // generation has none — Claude's turn count is not a fraction of
            // anything — so stage 1 shows a pulsing dot and a clock, and
            // nothing that implies a measurement. The same display covers the
            // tqdm shim failing after a vibevoice upgrade: degraded, rather
            // than a bar frozen at 0% that looks like a stall.
            match run.measured {
                Measured::Fraction(f) => rsx! {
                    ProgressBar { value: f }
                    span { class: "run-strip-pct", "{(f * 100.0) as u32}%" }
                    if let Some(total) = run.total {
                        span { class: "run-strip-count", "{run.step}/{total}" }
                    }
                },
                Measured::Unmeasurable => rsx! {},
            }

            span { class: "run-strip-elapsed", "{elapsed}" }

            if running {
                button {
                    class: "run-strip-cancel",
                    title: "Cancel this run",
                    onclick: move |_| {
                        if let Some(pgid) = *state.pgid.peek() {
                            // Signals the whole process group: the `claude`
                            // CLI runs as a grandchild, and killing only the
                            // Python pid would leave it running.
                            runner::cancel(pgid);
                        }
                    },
                    IconStop { size: 16 }
                }
            }
        }
    }
}
