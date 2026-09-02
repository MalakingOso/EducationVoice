//! The run strip: the same run, seen from another page.
//!
//! It sits below `.app-body` rather than inside a page, so it stays visible
//! while the Library is being browsed mid-synthesis — which is the whole
//! reason it exists now that the Run page has a view of its own.
//!
//! It is deliberately the *same mode* as that view, not the same size: the
//! near-black ground, the letter-spaced type and the single orange live
//! marker, compressed into a bar. A run that looked like two different things
//! depending on which page you were on would read as two different runs.
//!
//! It draws no bar and no percentage. There is no denominator in this
//! pipeline worth rendering — `max_steps` is an upper bound the generation
//! loop breaks out of early — so the strip says what is happening and how long
//! it has been happening, and nothing that implies a measurement.

use std::time::Instant;

use dioxus::prelude::*;

use crate::proto::Stage;
use crate::runner;
use crate::ui::app::{AppState, Page};
use crate::ui::icons::IconStop;
use crate::ui::run_state::format_elapsed;

#[component]
pub fn RunStrip(state: AppState) -> Element {
    // Subscribing to the tick is what re-renders the clock; the value itself
    // is never read for anything.
    let _tick = *state.tick.read();

    let run = state.run.read().clone();
    let running = run.is_running();
    let page = *state.page.read();

    // Idle *and* never run: nothing to say.
    //
    // Hidden on the Run page too, whatever the run is doing: that page turns
    // into the run while one is live, and a strip under it would report the
    // same thing twice.
    if page == Page::Run || (!running && run.stage.is_none()) {
        return rsx! { div { class: "run-strip hidden" } };
    }

    let stage = run.stage.unwrap_or(Stage::Unknown);
    let elapsed = format_elapsed(run.elapsed(Instant::now()));

    rsx! {
        div { class: "run-strip",
            span { class: if running { "run-pip" } else { "run-pip disarmed" } }
            span { class: "run-strip-stage", "{stage.label()}" }
            div { class: "run-strip-detail", "{run.detail}" }
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
