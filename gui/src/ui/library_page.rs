//! The Library: what has been made, and what can be made again from it.
//!
//! Organised like a library rather than as a flat list: a search field, then
//! the episodes grouped by the day they ran, newest day first. The grouping
//! is Beamer's `grouped_by_day` convention, computed in `library.rs` and
//! memoised here so it is not redone on every keystroke.

use dioxus::prelude::*;

use crate::library::{self, Episode};
use crate::ui::app::{AppState, Page};
use crate::ui::app_setup::rescan_library;
use crate::ui::components::Card;
use crate::ui::icons::{IconArrowClockwise, IconFolderOpen, IconPlay, IconTrash};
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
        div { class: "library-row",
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
                    class: "library-row-title",
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
            div { class: "library-row-meta", "{meta_line}" }
            div { class: "library-row-actions",
                if let Some(audio) = props.episode.audio.clone() {
                    button {
                        class: "btn-icon",
                        title: "Play",
                        onclick: move |_| open_external(&audio.to_string_lossy()),
                        IconPlay { size: 16 }
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
