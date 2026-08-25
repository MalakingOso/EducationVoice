//! The root component. It owns every signal and the run coroutine.
//!
//! Nothing here calls `provide_context`: `Signal<T>` is `Copy`, so passing
//! signals down as props costs nothing and keeps each page's dependencies
//! visible in its signature.

use std::time::{Duration, Instant};

use dioxus::desktop::use_window;
use dioxus::prelude::*;
use futures_util::StreamExt;

use crate::config::Config;
use crate::library::{self, Episode, RunMeta};
use crate::paths::Paths;
use crate::roster::Roster;
use crate::runner::{self, RunEvent, RunKind, RunOutcome};
use crate::ui::app_setup;
use crate::ui::icons::{IconGear, IconListBullets, IconMinus, IconWaveform, IconX};
use crate::ui::library_page::LibraryPage;
use crate::ui::run_page::RunPage;
use crate::ui::run_state::RunState;
use crate::ui::run_strip::RunStrip;
use crate::ui::script_page::ScriptPage;
use crate::ui::settings_page::SettingsPage;
use crate::ui::status_log::{log_status, LogLevel, StatusLog};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Run,
    Script,
    Library,
    Settings,
}

/// One request to the run coroutine.
///
/// Carries what the sidecar needs alongside the argv, because by the time the
/// run ends the Run page's fields may already have been edited for the next one.
#[derive(Debug, Clone)]
pub struct RunRequest {
    pub kind: RunKind,
    pub stem: String,
    pub source: String,
    pub hosts: u8,
    pub voices: Vec<String>,
    /// Open the review gate when this run finishes. False for auto-continue,
    /// which runs straight through.
    pub gate_after: bool,
}

/// Everything the pages share. Grouped into one struct purely so page
/// signatures stay short; it is still plain props, not a context.
#[derive(Clone, Copy, PartialEq)]
pub struct AppState {
    pub page: Signal<Page>,
    pub config: Signal<Config>,
    pub run: Signal<RunState>,
    pub log: Signal<StatusLog>,
    pub roster: Signal<Option<Roster>>,
    pub episodes: Signal<Vec<Episode>>,
    pub paths: Signal<Option<Paths>>,
    pub fatal: Signal<Option<String>>,
    /// The script under review at the gate.
    pub draft: Signal<String>,
    pub source: Signal<String>,
    pub pgid: Signal<Option<i32>>,
    /// Bumped on a timer so the elapsed clock re-renders while a run is live.
    pub tick: Signal<u64>,
}

#[component]
pub fn App() -> Element {
    let window = use_window();

    let state = AppState {
        page: use_signal(|| Page::Run),
        config: use_signal(|| Config::load().unwrap_or_default()),
        run: use_signal(RunState::default),
        log: use_signal(StatusLog::new),
        roster: use_signal(|| None),
        episodes: use_signal(Vec::new),
        paths: use_signal(|| None),
        fatal: use_signal(|| None),
        draft: use_signal(String::new),
        source: use_signal(String::new),
        pgid: use_signal(|| None),
        tick: use_signal(|| 0),
    };

    // Fixed order, as in Beamer: paths first because everything else needs
    // them, then the things that shell out, then the clock.
    app_setup::setup_paths(state);
    app_setup::setup_roster(state);
    app_setup::setup_voice_prefetch(state);
    app_setup::setup_library_scan(state);
    app_setup::setup_elapsed_tick(state);

    // The run lives here, in App()'s scope, so navigating to the Library
    // mid-synthesis does not cancel it. Dioxus drops a spawned task when its
    // owning scope drops; a run started from the Run page's own scope would
    // die silently on the first navigation.
    let runner_co = use_coroutine(move |mut rx: UnboundedReceiver<RunRequest>| async move {
        while let Some(req) = rx.next().await {
            drive_run(state, req).await;
        }
    });

    let page = *state.page.read();
    // Read here rather than in the attribute so the toggle in Settings is a
    // dependency of this scope and the window repaints the moment it flips.
    let translucent = state.config.read().appearance.translucent;
    let mut close_armed = use_signal(|| false);

    rsx! {
        head {
            link { rel: "stylesheet", href: asset!("assets/styles.css") }
        }
        div { class: if translucent { "app-container translucent" } else { "app-container" },
            div { class: "app-body",
                div { class: "left-column",
                    div { class: "corner-badge",
                        img { class: "corner-badge-icon", src: asset!("assets/icon.png"), alt: "article2pod" }
                    }
                    nav { class: "sidebar",
                        div { class: "sidebar-top",
                            RailButton { page: Page::Run, current: page, target: state.page, IconWaveform {} }
                            RailButton { page: Page::Library, current: page, target: state.page, IconListBullets {} }
                        }
                        div { class: "sidebar-bottom",
                            RailButton { page: Page::Settings, current: page, target: state.page, IconGear {} }
                        }
                    }
                }
                div { class: "right-column",
                    div {
                        class: "titlebar",
                        // -webkit-app-region: drag is a Chromium extension that
                        // WebKitGTK ignores, so the drag is done by hand here.
                        onmousedown: {
                            let window = window.clone();
                            move |_| { let _ = window.drag_window(); }
                        },
                        div { class: "titlebar-controls",
                            button {
                                class: "titlebar-btn",
                                // Without this the titlebar's drag grabs the
                                // pointer and swallows the click.
                                onmousedown: move |e: Event<MouseData>| e.stop_propagation(),
                                onclick: {
                                    let window = window.clone();
                                    move |_| window.set_minimized(true)
                                },
                                IconMinus { size: 14 }
                            }
                            button {
                                // Two-step arm-then-confirm rather than a modal,
                                // per Beamer: closing mid-run kills a synthesis
                                // that may be ten minutes in.
                                class: if *close_armed.read() { "titlebar-btn close armed" } else { "titlebar-btn close" },
                                onmousedown: move |e: Event<MouseData>| e.stop_propagation(),
                                onclick: {
                                    let window = window.clone();
                                    move |_| {
                                        let running = state.run.peek().is_running();
                                        if running && !*close_armed.peek() {
                                            close_armed.set(true);
                                            return;
                                        }
                                        if let Some(pgid) = *state.pgid.peek() {
                                            runner::cancel(pgid);
                                        }
                                        window.close();
                                    }
                                },
                                IconX { size: 14 }
                            }
                        }
                    }

                    if let Some(err) = state.fatal.read().clone() {
                        div { class: "content",
                            div { class: "card",
                                div { class: "card-title", "Cannot reach the pipeline" }
                                div { class: "script-error", "{err}" }
                            }
                        }
                    } else {
                        match page {
                            Page::Run => rsx! { RunPage { state, runner: runner_co } },
                            Page::Script => rsx! { ScriptPage { state, runner: runner_co } },
                            Page::Library => rsx! { LibraryPage { state } },
                            Page::Settings => rsx! { SettingsPage { state } },
                        }
                    }
                }
            }
            RunStrip { state }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct RailButtonProps {
    page: Page,
    current: Page,
    target: Signal<Page>,
    children: Element,
}

#[component]
fn RailButton(props: RailButtonProps) -> Element {
    let mut target = props.target;
    let active = props.page == props.current;
    rsx! {
        button {
            class: if active { "sidebar-icon active" } else { "sidebar-icon" },
            onclick: move |_| target.set(props.page),
            {props.children}
        }
    }
}

/// Spawn one run and drain it to completion.
///
/// A plain `while let` over the single event channel rather than a `select!`:
/// `recv()` on a closed channel returns `Ready(None)` immediately and forever,
/// so a `select!` without an explicit `None` arm spins hot on the main thread
/// and freezes the window. One channel needs no `select!` at all.
async fn drive_run(mut state: AppState, req: RunRequest) {
    let Some(paths) = state.paths.peek().clone() else {
        log_status(&mut state.log, LogLevel::Error, "no project paths; cannot run");
        return;
    };

    let gpu = state.config.peek().device.gpu_mask.clone();
    let gpu = if gpu.is_empty() { None } else { Some(gpu) };

    state.run.write().begin(Instant::now());
    state.log.write().clear();
    log_status(
        &mut state.log,
        LogLevel::Info,
        format!("starting: {:?}", req.kind.argv()),
    );

    let mut session = match runner::spawn(&paths, &req.kind, gpu.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("{e:#}");
            tracing::error!(error = %msg, "spawn failed");
            log_status(&mut state.log, LogLevel::Error, msg.clone());
            state.run.write().finish(&RunOutcome::Failed { code: None });
            state.run.write().last_error = Some(msg);
            return;
        }
    };

    state.pgid.set(Some(session.pgid));
    let started_at = chrono::Local::now();
    let started = Instant::now();
    let mut outcome = RunOutcome::Failed { code: None };

    while let Some(event) = session.events.recv().await {
        match event {
            RunEvent::Py(py) => {
                // Dual logging, per Beamer: once to tracing for a durable
                // record, once to the pane the user is actually looking at.
                app_setup::log_event(&mut state.log, &py);
                state.run.write().apply(&py);
            }
            RunEvent::Stderr(line) => {
                tracing::debug!(line, "child stderr");
                log_status(&mut state.log, LogLevel::Info, line);
            }
            RunEvent::Unparsed(line) => {
                log_status(
                    &mut state.log,
                    LogLevel::Warn,
                    format!("unparsed: {}", crate::ui::components::truncate_chars(&line, 120)),
                );
            }
            RunEvent::Exited(o) => {
                state.run.write().finish(&o);
                outcome = o;
            }
        }
    }

    state.pgid.set(None);
    finish_run(state, req, paths, started_at, started.elapsed(), outcome);
}

/// Record the run and decide what the UI does next.
fn finish_run(
    mut state: AppState,
    req: RunRequest,
    paths: Paths,
    started_at: chrono::DateTime<chrono::Local>,
    elapsed: Duration,
    outcome: RunOutcome,
) {
    let snapshot = state.run.peek().clone();
    let meta_path = library::meta_path(&paths, &req.stem);

    // What the *previous* run of this stem recorded. Stage 2 shares a stem
    // with stage 1 and replaces its whole sidecar, so without this the gated
    // flow — the default flow — throws away the title the ingest stage went
    // and fetched, at the moment the episode is finally finished. Synthesis
    // emits no title event of its own and never will: it reads a script off
    // disk and has no article to ask about.
    let previous = library::read_meta(&meta_path);

    let meta = RunMeta {
        title: snapshot.title.clone(),
        source: req.source.clone(),
        hosts: req.hosts,
        voices: req.voices.clone(),
        device: snapshot.device.clone(),
        model: snapshot.model.clone(),
        started: Some(started_at),
        finished: Some(chrono::Local::now()),
        elapsed_secs: Some(elapsed.as_secs()),
        outcome: match outcome {
            RunOutcome::Completed => "completed",
            RunOutcome::Cancelled => "cancelled",
            RunOutcome::Failed { .. } => "failed",
        }
        .to_string(),
    }
    .carrying_forward(previous.as_ref());

    if let Err(e) = library::write_meta(&meta_path, &meta) {
        // A missing sidecar costs the Library a row's detail, never the episode.
        tracing::warn!(error = %e, "could not write the run sidecar");
    }

    match outcome {
        RunOutcome::Completed => {
            log_status(&mut state.log, LogLevel::Info, "run finished");
            // The gate opens only on a script stage that actually produced a
            // file; a completed auto-continue run has nothing to review.
            if req.gate_after {
                if let Some(path) = snapshot.script_path.clone() {
                    match std::fs::read_to_string(&path) {
                        Ok(text) => {
                            state.draft.set(text);
                            state.page.set(Page::Script);
                        }
                        Err(e) => log_status(
                            &mut state.log,
                            LogLevel::Error,
                            format!("could not read {}: {e}", path.display()),
                        ),
                    }
                }
            }
        }
        RunOutcome::Cancelled => log_status(&mut state.log, LogLevel::Warn, "run cancelled"),
        RunOutcome::Failed { code } => log_status(
            &mut state.log,
            LogLevel::Error,
            match code {
                Some(c) => format!("run failed with status {c}"),
                None => "run failed".to_string(),
            },
        ),
    }

    app_setup::rescan_library(state);
}
