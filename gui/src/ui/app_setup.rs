//! Startup hooks, called from `App()` in a fixed order.
//!
//! Each runs once via `use_hook`. Split out of `app.rs` for the same reason
//! Beamer splits its own: the root component should read as layout, not as a
//! list of things that happen at boot.
//!
//! The spawn tiers matter here. A `Signal` moved into `tokio::spawn` resolves
//! against the wrong thread-local arena and diverges with no error at all, so
//! everything holding one uses Dioxus's `spawn`; blocking IO goes through
//! `spawn_blocking` with the signal written back outside the closure.

use std::time::Duration;

use dioxus::prelude::*;

use crate::library;
use crate::paths::Paths;
use crate::proto::PyEvent;
use crate::roster;
use crate::runner::{self, RunEvent, RunKind};
use crate::ui::app::AppState;
use crate::ui::components::truncate_chars;
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

/// Resolve the project root and the venv interpreter.
///
/// First, because everything else needs them. A failure here is fatal to the
/// app's purpose, so it is surfaced as a card rather than logged and ignored:
/// without the venv there is nothing this window can do.
pub fn setup_paths(mut state: AppState) {
    use_hook(move || match Paths::resolve() {
        Ok(p) => {
            tracing::info!(root = %p.root.display(), "resolved project paths");
            state.paths.set(Some(p));
        }
        Err(e) => {
            let msg = format!("{e:#}");
            tracing::error!(error = %msg, "could not resolve project paths");
            state.fatal.set(Some(msg));
        }
    });
}

/// Read the voice roster from the CLI rather than duplicating it here.
pub fn setup_roster(mut state: AppState) {
    use_hook(move || {
        spawn(async move {
            let Some(paths) = state.paths.peek().clone() else {
                return;
            };
            match roster::load(&paths).await {
                Ok(r) => {
                    tracing::info!(voices = r.voices.len(), "roster loaded");
                    state.roster.set(Some(r));
                }
                Err(e) => {
                    let msg = format!("could not read the voice roster: {e:#}");
                    tracing::warn!(error = %msg);
                    log_status(&mut state.log, LogLevel::Warn, msg);
                }
            }
        });
    });
}

/// Download the preset clips once, at startup.
///
/// The gated flow's stage 1 never touches voices — deliberately, so a bad
/// preset cannot fail a run after the Claude tokens are spent. That leaves the
/// gate as the first place a voice problem could surface, which is far too
/// late. Doing it here costs seconds when the clips are already on disk, and
/// moves the failure to before anything was spent.
///
/// Drained directly rather than through the run coroutine: this is not an
/// episode and must not raise the run strip.
pub fn setup_voice_prefetch(mut state: AppState) {
    use_hook(move || {
        spawn(async move {
            let Some(paths) = state.paths.peek().clone() else {
                return;
            };
            let mut session = match runner::spawn(&paths, &RunKind::FetchVoices, None) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "could not prefetch voices");
                    return;
                }
            };
            while let Some(event) = session.events.recv().await {
                match event {
                    RunEvent::Py(PyEvent::Error { text }) => {
                        log_status(
                            &mut state.log,
                            LogLevel::Warn,
                            format!("voice prefetch: {text}"),
                        );
                    }
                    RunEvent::Py(_) | RunEvent::Stderr(_) | RunEvent::Unparsed(_) => {}
                    RunEvent::Exited(outcome) => {
                        tracing::info!(?outcome, "voice prefetch finished");
                    }
                }
            }
        });
    });
}

/// Populate the Library once at startup.
pub fn setup_library_scan(state: AppState) {
    use_hook(move || rescan_library(state));
}

/// Rescan `output/` and `scripts/`.
///
/// Directory scanning is blocking IO, so it goes to `spawn_blocking`; the
/// signal is written back on the Dioxus side, outside that closure.
pub fn rescan_library(mut state: AppState) {
    let Some(paths) = state.paths.peek().clone() else {
        return;
    };
    spawn(async move {
        let scanned = tokio::task::spawn_blocking(move || library::scan(&paths)).await;
        match scanned {
            Ok(Ok(episodes)) => {
                tracing::debug!(count = episodes.len(), "library scanned");
                state.episodes.set(episodes);
            }
            Ok(Err(e)) => tracing::warn!(error = %e, "library scan failed"),
            Err(e) => tracing::warn!(error = %e, "library scan panicked"),
        }
    });
}

/// Drive the elapsed clock.
///
/// The run strip's clock is derived from an `Instant`, which does not itself
/// notify anyone; something has to re-render it. Ticking only while a run is
/// live keeps an idle window from re-rendering twice a second forever.
pub fn setup_elapsed_tick(mut state: AppState) {
    use_hook(move || {
        spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                // peek, not read: this task must not subscribe to the very
                // signal it is about to write, which would re-arm itself.
                if state.run.peek().is_running() {
                    let next = *state.tick.peek() + 1;
                    state.tick.set(next);
                }
            }
        });
    });
}

/// Mirror one protocol event into the visible log.
///
/// Progress events are omitted on purpose: 200 of them would bury the stage
/// and message lines that actually say what is happening, and the strip
/// already shows the number.
pub fn log_event(log: &mut Signal<StatusLog>, event: &PyEvent) {
    match event {
        PyEvent::Stage {
            stage,
            status,
            detail,
            path,
            device,
            ..
        } => {
            let mut line = format!("{} {:?}", stage.label(), status);
            if let Some(d) = detail {
                line.push_str(&format!(" — {d}"));
            }
            if let Some(d) = device {
                line.push_str(&format!(" on {d}"));
            }
            if let Some(p) = path {
                line.push_str(&format!(" → {p}"));
            }
            log_status(log, LogLevel::Info, line);
        }
        PyEvent::Message { text } => {
            log_status(log, LogLevel::Info, truncate_chars(text.trim(), 400));
        }
        PyEvent::Title { text } => {
            log_status(log, LogLevel::Info, format!("title: {text}"))
        }
        PyEvent::Warning { text } => log_status(log, LogLevel::Warn, text.clone()),
        PyEvent::Error { text } => log_status(log, LogLevel::Error, text.clone()),
        PyEvent::Done { output } => {
            log_status(log, LogLevel::Info, format!("done: {output}"))
        }
        PyEvent::Progress { .. } | PyEvent::Unknown => {}
    }
}
