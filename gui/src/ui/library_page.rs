//! The Library: what has been made, and what can be made again from it.
//!
//! Organised like a library rather than as a flat list: a search field, then
//! the episodes grouped by the day they ran, newest day first. The grouping
//! is Beamer's `grouped_by_day` convention, computed in `library.rs` and
//! memoised here so it is not redone on every keystroke.

use std::path::{Path, PathBuf};

use dioxus::prelude::*;

use crate::config::{save_config, Config};
use crate::describe;
use crate::library::{self, Episode};
use crate::paths::Paths;
use crate::spotify::{self, Readiness};
use crate::ui::app::{AppState, Page};
use crate::ui::app_setup::rescan_library;
use crate::ui::components::Card;
use crate::ui::icons::{
    IconArrowClockwise, IconFileText, IconFolderOpen, IconPlay, IconSpotify, IconTrash,
};
use crate::ui::open_external;
use crate::ui::status_log::LogLevel;

#[component]
pub fn LibraryPage(state: AppState) -> Element {
    let episodes = state.episodes.read().clone();
    // Which row's delete is armed. Beamer's two-step arm-then-confirm stands in
    // for a modal, which this design deliberately has no primitive for.
    let armed = use_signal(|| None::<String>);
    // Which row is being renamed, and the text so far.
    let editing = use_signal(|| None::<String>);
    let mut search = use_signal(String::new);

    // Memoised on both inputs, so typing filters without regrouping the
    // untouched rows and rescanning does not lose the filter.
    let groups = use_memo(move || {
        let needle = search.read().clone();
        let matching: Vec<Episode> = state
            .episodes
            .read()
            .iter()
            .filter(|ep| ep.matches(&needle))
            .cloned()
            .collect();
        library::group_by_day(&matching)
    });

    let grouped = groups.read().clone();

    rsx! {
        div { class: "content",
            input {
                class: "input library-search",
                placeholder: "Search by title or source",
                value: "{search}",
                oninput: move |e: Event<FormData>| search.set(e.value().to_string()),
            }

            if episodes.is_empty() {
                Card { title: "Episodes",
                    div { class: "empty-state",
                        div { class: "empty-state-text", "Nothing here yet." }
                    }
                }
            } else if grouped.is_empty() {
                Card { title: "Episodes",
                    div { class: "empty-state",
                        div { class: "empty-state-text", "Nothing matches that." }
                    }
                }
            }

            for (label, eps) in grouped {
                div { class: "history-group", key: "{label}",
                    div { class: "history-date-header", "{label}" }
                    for ep in eps {
                        LibraryRow { key: "{ep.stem}", state, episode: ep, armed, editing }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct LibraryRowProps {
    state: AppState,
    episode: Episode,
    armed: Signal<Option<String>>,
    editing: Signal<Option<String>>,
}

#[component]
fn LibraryRow(props: LibraryRowProps) -> Element {
    let mut state = props.state;
    let mut armed = props.armed;
    let mut editing = props.editing;
    let ep = props.episode.clone();
    let stem = ep.stem.clone();
    let is_armed = armed.read().as_deref() == Some(stem.as_str());
    let is_editing = editing.read().as_deref() == Some(stem.as_str());
    // App-wide, not per-row: v1 sends one episode at a time, so every card's
    // button is gated on whether *any* send is in flight.
    let sending = state.spotify_send.read().is_some();

    let mut draft = use_signal(|| ep.title());

    let meta_line = meta_line(&ep);

    // Renaming writes through the same atomic sidecar write the runner uses,
    // then rescans, so the row shows what is actually on disk rather than what
    // was typed.
    let mut commit = move |stem: String| {
        let title = draft.peek().clone();
        editing.set(None);
        let Some(paths) = state.paths.peek().clone() else { return };
        match library::rename(&paths, &stem, &title) {
            Ok(()) => rescan_library(state),
            Err(e) => state
                .log
                .write()
                .push(LogLevel::Error, format!("could not rename {stem}: {e}")),
        }
    };

    rsx! {
        div { class: "library-card",
            if is_editing {
                input {
                    class: "input library-rename",
                    value: "{draft}",
                    autofocus: true,
                    oninput: move |e: Event<FormData>| draft.set(e.value().to_string()),
                    onkeydown: {
                        let stem = stem.clone();
                        move |e: Event<KeyboardData>| match e.key() {
                            Key::Enter => commit(stem.clone()),
                            Key::Escape => editing.set(None),
                            _ => {}
                        }
                    },
                    // A click elsewhere is a commit rather than a discard: the
                    // alternative loses a name that was typed correctly.
                    onblur: {
                        let stem = stem.clone();
                        move |_| commit(stem.clone())
                    },
                }
            } else {
                div {
                    class: "library-card-title",
                    title: "Click to rename",
                    onclick: {
                        let stem = stem.clone();
                        move |_| {
                            draft.set(ep.title());
                            editing.set(Some(stem.clone()));
                        }
                    },
                    "{props.episode.title()}"
                }
            }
            div { class: "library-card-meta", "{meta_line}" }
            div { class: "library-card-actions",
                if let Some(audio) = props.episode.audio.clone() {
                    button {
                        class: "btn-icon",
                        title: "Play",
                        onclick: move |_| open_external(&audio.to_string_lossy()),
                        IconPlay { size: 16 }
                    }
                }
                if let Some(audio) = props.episode.audio.clone() {
                    button {
                        class: if spotify_is_ready(&props.episode) { "btn-icon spotify-ready" } else { "btn-icon" },
                        title: spotify_button_title(&props.episode, sending),
                        disabled: sending,
                        onclick: {
                            let stem = stem.clone();
                            let title = props.episode.title();
                            let script = props.episode.script.clone();
                            move |_| send_to_spotify(
                                state, stem.clone(), audio.clone(), title.clone(), script.clone(),
                            )
                        },
                        IconSpotify { size: 16 }
                    }
                }
                if let Some(script) = props.episode.script.clone() {
                    button {
                        class: "btn-icon",
                        title: "View this script",
                        onclick: move |_| {
                            match std::fs::read_to_string(&script) {
                                Ok(text) => {
                                    state.draft.set(text);
                                    state.page.set(Page::Script);
                                }
                                Err(e) => state.log.write().push(
                                    LogLevel::Error,
                                    format!("could not read {}: {e}", script.display()),
                                ),
                            }
                        },
                        IconFileText { size: 16 }
                    }
                }
                if let Some(script) = props.episode.script.clone() {
                    button {
                        class: "btn-icon",
                        title: "Re-voice this script",
                        onclick: move |_| {
                            match std::fs::read_to_string(&script) {
                                Ok(text) => {
                                    state.draft.set(text);
                                    // The gate reads the path from the run
                                    // state, so point it at this script.
                                    state.run.write().script_path = Some(script.clone());
                                    state.page.set(Page::Script);
                                }
                                Err(e) => state.log.write().push(
                                    LogLevel::Error,
                                    format!("could not read {}: {e}", script.display()),
                                ),
                            }
                        },
                        IconArrowClockwise { size: 16 }
                    }
                }
                button {
                    class: "btn-icon",
                    title: "Open the output folder",
                    onclick: move |_| {
                        if let Some(p) = state.paths.peek().clone() {
                            open_external(&p.output_dir.to_string_lossy());
                        }
                    },
                    IconFolderOpen { size: 16 }
                }
                button {
                    class: if is_armed { "btn-icon remove armed" } else { "btn-icon remove" },
                    title: if is_armed { "Click again to delete" } else { "Delete" },
                    onclick: {
                        let stem = stem.clone();
                        move |_| {
                            if !is_armed {
                                armed.set(Some(stem.clone()));
                                return;
                            }
                            armed.set(None);
                            let Some(paths) = state.paths.peek().clone() else { return };
                            match library::delete(&paths, &stem) {
                                Ok(n) => {
                                    state.log.write().push(
                                        LogLevel::Info,
                                        format!("deleted {n} file(s) for {stem}"),
                                    );
                                    rescan_library(state);
                                }
                                Err(e) => state.log.write().push(
                                    LogLevel::Error,
                                    format!("could not delete {stem}: {e}"),
                                ),
                            }
                        }
                    },
                    IconTrash { size: 16 }
                }
            }
        }
    }
}

/// Whether the sidecar's last poll saw this episode reach `READY` on
/// Spotify — the button turns green only for that terminal state, never for
/// "uploading" or "processing", so green means it is actually resident there
/// right now, not merely that a send was started.
fn spotify_is_ready(ep: &Episode) -> bool {
    ep.meta.as_ref().and_then(|m| m.spotify_status.as_deref()) == Some("ready")
}

/// What the Spotify button's tooltip says, given what the sidecar last
/// recorded about it.
fn spotify_button_title(ep: &Episode, sending: bool) -> String {
    if sending {
        return "A send is already in progress".to_string();
    }
    match ep.meta.as_ref().and_then(|m| m.spotify_status.as_deref()) {
        Some("ready") => "Sent to Spotify — send again".to_string(),
        Some(status) => format!("Spotify: {status}"),
        None => "Send to Spotify".to_string(),
    }
}

/// Kick off a send. `state.spotify_send` is set here, synchronously, so the
/// button disables on the same click that starts the task — a second click
/// before the task's first `.await` cannot slip through.
fn send_to_spotify(
    mut state: AppState,
    stem: String,
    audio: PathBuf,
    title: String,
    script: Option<PathBuf>,
) {
    state.spotify_send.set(Some(stem.clone()));
    spawn(async move {
        run_spotify_send(state, &stem, &audio, &title, script.as_deref()).await;
        // Every exit path — success, a CLI error, a timeout — funnels through
        // here, so the guard can never be left set by a forgotten branch.
        state.spotify_send.set(None);
    });
}

/// Cover image, show, upload, then poll to a terminal readiness — logging
/// and writing the sidecar at each transition.
async fn run_spotify_send(
    mut state: AppState,
    stem: &str,
    audio: &Path,
    title: &str,
    script: Option<&Path>,
) {
    let Some(paths) = state.paths.peek().clone() else { return };
    let meta_path = library::meta_path(&paths, stem);
    let cover = Config::config_dir().join("spotify-cover.jpg");

    if let Err(e) = spotify::ensure_cover_image(&cover) {
        state
            .log
            .write()
            .push(LogLevel::Error, format!("spotify: could not write the cover image: {e}"));
        return;
    }

    write_spotify_status(state, &meta_path, "uploading");

    let cached = state.config.peek().spotify.show_uri.clone();
    let show_uri = match spotify::ensure_show(cached.as_deref(), &cover).await {
        Ok(uri) => uri,
        Err(e) => {
            state.log.write().push(LogLevel::Error, format!("spotify: {e}"));
            write_spotify_status(state, &meta_path, "failed");
            return;
        }
    };
    if cached.as_deref() != Some(show_uri.as_str()) {
        let uri = show_uri.clone();
        save_config(&mut state.config, |c| c.spotify.show_uri = Some(uri));
    }

    let summary = write_description(state, &paths, script, title).await;

    state.log.write().push(LogLevel::Info, format!("spotify: uploading \"{title}\""));
    let uploaded = match spotify::upload(audio, title, &show_uri, &cover, summary.as_deref()).await
    {
        Ok(u) => u,
        Err(e) => {
            state.log.write().push(LogLevel::Error, format!("spotify: {e}"));
            write_spotify_status(state, &meta_path, "failed");
            return;
        }
    };

    let episode_id = uploaded.episode_uri;
    state
        .log
        .write()
        .push(LogLevel::Info, format!("spotify: uploaded as {episode_id}, waiting for it to process"));
    write_spotify_episode(state, &meta_path, &episode_id, "processing");

    let deadline = tokio::time::Instant::now() + spotify::POLL_TIMEOUT;
    loop {
        match spotify::poll_status(&episode_id).await {
            Ok(status) => {
                write_spotify_episode(state, &meta_path, &episode_id, status.readiness.label());
                match status.readiness {
                    Readiness::Ready => {
                        state.log.write().push(LogLevel::Info, "spotify: episode is ready".to_string());
                        return;
                    }
                    Readiness::Failed => {
                        state.log.write().push(LogLevel::Error, "spotify: processing failed".to_string());
                        return;
                    }
                    Readiness::Pending(_) => {}
                }
            }
            Err(e) => {
                state.log.write().push(LogLevel::Error, format!("spotify: {e}"));
                write_spotify_episode(state, &meta_path, &episode_id, "failed");
                return;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            state.log.write().push(
                LogLevel::Warn,
                "spotify: gave up waiting for the episode to become ready".to_string(),
            );
            write_spotify_episode(state, &meta_path, &episode_id, "timed out");
            return;
        }
        tokio::time::sleep(spotify::POLL_INTERVAL).await;
    }
}

/// Ask Haiku for the show-notes blurb, logging whichever way it goes.
///
/// Always returns — a missing description never stops a send. The episode is
/// the thing being published; the blurb under it is worth one Haiku call and
/// no more than that, which is why every failure here is a log line rather
/// than an early return.
async fn write_description(
    mut state: AppState,
    paths: &Paths,
    script: Option<&Path>,
    title: &str,
) -> Option<String> {
    // A hand-dropped audio file with no script beside it. Nothing to read.
    let script = script?;

    state.log.write().push(LogLevel::Info, "spotify: writing a description".to_string());
    match describe::describe(paths, script, title).await {
        Ok(Some(text)) => {
            state.log.write().push(LogLevel::Info, format!("spotify: description - {text}"));
            Some(text)
        }
        Ok(None) => {
            state
                .log
                .write()
                .push(LogLevel::Warn, "spotify: no description written".to_string());
            None
        }
        Err(e) => {
            // Deliberately not "failed": the send is still going ahead.
            state.log.write().push(LogLevel::Warn, format!("spotify: {e}"));
            None
        }
    }
}

/// Re-read the sidecar, set its Spotify status, write it back, rescan.
///
/// Read fresh each time rather than threading one `RunMeta` through the whole
/// send: an upload can take minutes, long enough for the title to be edited
/// mid-send, and a stale write would clobber that rename.
fn write_spotify_status(state: AppState, meta_path: &Path, status: &str) {
    let mut meta = library::read_meta(meta_path).unwrap_or_default();
    meta.spotify_status = Some(status.to_string());
    if let Err(e) = library::write_meta(meta_path, &meta) {
        tracing::warn!(error = %e, "could not write the spotify status to the sidecar");
    }
    rescan_library(state);
}

fn write_spotify_episode(state: AppState, meta_path: &Path, episode_uri: &str, status: &str) {
    let mut meta = library::read_meta(meta_path).unwrap_or_default();
    meta.spotify_episode_uri = Some(episode_uri.to_string());
    meta.spotify_status = Some(status.to_string());
    if let Err(e) = library::write_meta(meta_path, &meta) {
        tracing::warn!(error = %e, "could not write the spotify status to the sidecar");
    }
    rescan_library(state);
}

/// The dot-separated line under the title: only what was actually recorded.
///
/// Each part is omitted rather than defaulted. "0 hosts" on an episode that
/// never recorded a host count reads as a measurement, and a script-only run
/// showing nothing about its missing audio reads as a rendering fault — so
/// that one is stated instead.
fn meta_line(ep: &Episode) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(m) = &ep.meta {
        if m.hosts > 0 {
            parts.push(format!("{} hosts", m.hosts));
        }
        if let Some(d) = &m.device {
            parts.push(d.clone());
        }
        if let Some(secs) = m.elapsed_secs {
            parts.push(format!("{}m{:02}s", secs / 60, secs % 60));
        }
        if !m.outcome.is_empty() {
            parts.push(m.outcome.clone());
        }
    }
    // A stage-1 run that was never voiced is still worth listing, and still
    // re-voiceable — saying so is what makes the row's ⟳ button make sense.
    if ep.audio.is_none() {
        parts.push("script only".to_string());
    }
    if parts.is_empty() {
        // Everything made before the GUI existed has no sidecar. Saying so is
        // more useful than an empty line that looks like a rendering fault.
        return "no run record".to_string();
    }
    parts.join(" · ")
}
