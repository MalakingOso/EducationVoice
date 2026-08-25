//! Settings: the knobs that outlive a single run, and the trail the last one
//! left.
//!
//! Hosts, voices and the log all used to sit on the Run page, in front of the
//! one thing that page is for. They are set once and then read occasionally,
//! which is what this page is.

use dioxus::prelude::*;

use crate::config::{save_config, Config};
use crate::ui::app::AppState;
use crate::ui::components::{Card, Select};

/// `ZE_AFFINITY_MASK` values. Empty means "do not set it", leaving the choice
/// to the Python side's own device detection.
const DEVICES: &[(&str, &str)] = &[
    ("", "Automatic"),
    ("0", "Arc B570 (10 GB)"),
    ("1", "Arc Pro B60 (22.7 GB)"),
];

#[component]
pub fn SettingsPage(state: AppState) -> Element {
    let mut state = state;
    let cfg = state.config.read().clone();
    let paths = state.paths.read().clone();
    let roster = state.roster.read().clone();

    // The CLI accepts only these three, and its own roster is keyed on them.
    let host_options: Vec<(String, String)> = roster
        .as_ref()
        .map(|r| r.host_choices())
        .unwrap_or_else(|| vec![2, 3, 4])
        .into_iter()
        .map(|h| (h.to_string(), format!("{h} hosts")))
        .collect();

    let voice_options: Vec<(String, String)> = roster
        .as_ref()
        .map(|r| {
            r.voices
                .iter()
                .map(|(name, v)| (name.clone(), format!("{name} · {}", v.gender)))
                .collect()
        })
        .unwrap_or_default();

    // An empty selection means "let the CLI apply DEFAULT_ROSTER", so the
    // picker shows what that would be rather than leaving the slots blank.
    let selected: Vec<String> = if cfg.voices.selected.is_empty() {
        roster
            .as_ref()
            .map(|r| r.default_for(cfg.run.hosts))
            .unwrap_or_default()
    } else {
        cfg.voices.selected.clone()
    };

    rsx! {
        div { class: "content",
            Card { title: "Episode",
                div { class: "card-row",
                    span { class: "card-label", "Hosts" }
                    Select {
                        value: "{cfg.run.hosts}",
                        options: host_options,
                        onchange: move |v: String| {
                            if let Ok(h) = v.parse::<u8>() {
                                save_config(&mut state.config, |c| {
                                    c.run.hosts = h;
                                    // The old selection binds voice to speaker
                                    // by position, so a changed host count
                                    // makes it meaningless rather than short.
                                    c.voices.selected.clear();
                                });
                            }
                        },
                    }
                }
                div { class: "card-label-hint",
                    "Tone and length are the CLI's own defaults: a conversational \
                     register, and a length the article's density decides."
                }
            }

            Card { title: "Voices",
                for i in 0..cfg.run.hosts as usize {
                    div { class: "card-row", key: "{i}",
                        span { class: "card-label", "Speaker {i + 1}" }
                        Select {
                            value: selected.get(i).cloned().unwrap_or_default(),
                            options: voice_options.clone(),
                            onchange: move |v: String| {
                                let hosts = state.config.peek().run.hosts as usize;
                                let mut list = selected_or_default(&state);
                                list.resize(hosts, String::new());
                                list[i] = v;
                                save_config(&mut state.config, |c| c.voices.selected = list);
                            },
                        }
                    }
                }
            }

            Card { title: "Graphics",
                div { class: "card-row",
                    span { class: "card-label", "Device" }
                    Select {
                        value: cfg.device.gpu_mask.clone(),
                        options: DEVICES.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect(),
                        onchange: move |v: String| {
                            save_config(&mut state.config, |c| c.device.gpu_mask = v);
                        },
                    }
                }
                div { class: "card-label-hint",
                    "Selecting a card sets ZE_AFFINITY_MASK for the run. The B60 has the headroom; the B570 also drives the desktop."
                }
            }

            Card { title: "Log",
                div { class: "status-log",
                    if state.log.read().entries.is_empty() {
                        div { class: "empty-state",
                            div { class: "empty-state-text", "Nothing has run yet." }
                        }
                    }
                    for (i, entry) in state.log.read().entries.iter().enumerate() {
                        div { class: "log-entry {entry.level.class()}", key: "{i}",
                            span { class: "log-time", "{entry.time}" }
                            span { class: "log-msg", "{entry.message}" }
                        }
                    }
                }
            }

            Card { title: "Where things are",
                if let Some(p) = paths {
                    div { class: "card-row",
                        span { class: "card-label", "Project" }
                        span { class: "debug-text", "{p.root.display()}" }
                    }
                    div { class: "card-row",
                        span { class: "card-label", "Interpreter" }
                        span { class: "debug-text", "{p.python.display()}" }
                    }
                    div { class: "card-row",
                        span { class: "card-label", "Episodes" }
                        span { class: "debug-text", "{p.output_dir.display()}" }
                    }
                }
                div { class: "card-row",
                    span { class: "card-label", "Settings" }
                    span { class: "debug-text", "{Config::config_path().display()}" }
                }
            }
        }
    }
}

/// The selection to edit: the explicit one when present, otherwise the CLI's
/// default expanded so a single changed slot does not blank the others.
fn selected_or_default(state: &AppState) -> Vec<String> {
    let cfg = state.config.peek();
    if !cfg.voices.selected.is_empty() {
        return cfg.voices.selected.clone();
    }
    state
        .roster
        .peek()
        .as_ref()
        .map(|r| r.default_for(cfg.run.hosts))
        .unwrap_or_default()
}
