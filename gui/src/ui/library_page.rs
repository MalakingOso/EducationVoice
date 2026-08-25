//! The Library: what has been made, and what can be made again from it.

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

    rsx! {
        div { class: "content",
            Card { title: "Episodes",
                if episodes.is_empty() {
                    div { class: "empty-state",
                        div { class: "empty-state-text", "Nothing here yet." }
                    }
                }
                for ep in episodes {
                    LibraryRow { key: "{ep.stem}", state, episode: ep, armed }
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
}

#[component]
fn LibraryRow(props: LibraryRowProps) -> Element {
    let mut state = props.state;
    let mut armed = props.armed;
    let ep = props.episode.clone();
    let stem = ep.stem.clone();
    let is_armed = armed.read().as_deref() == Some(stem.as_str());

    let meta_line = match &ep.meta {
        Some(m) => {
            let mut parts = vec![format!("{} hosts", m.hosts)];
            if let Some(d) = &m.device {
                parts.push(d.clone());
            }
            if let Some(secs) = m.elapsed_secs {
                parts.push(format!("{}m{:02}s", secs / 60, secs % 60));
            }
            parts.push(m.outcome.clone());
            parts.join(" · ")
        }
        // Everything made before the GUI existed has no sidecar. Saying so is
        // more useful than an empty line that looks like a rendering fault.
        None => "no run record".to_string(),
    };

    rsx! {
        div { class: "library-row",
            div { class: "library-row-title", "{ep.title()}" }
            div { class: "library-row-meta", "{meta_line}" }
            div { class: "library-row-actions",
                if let Some(audio) = ep.audio.clone() {
                    button {
                        class: "btn-icon",
                        title: "Play",
                        onclick: move |_| open_external(&audio.to_string_lossy()),
                        IconPlay { size: 16 }
                    }
                }
                if let Some(script) = ep.script.clone() {
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
