//! The Run page: what to make, and how it is going.

use dioxus::prelude::*;

use crate::config::save_config;
use crate::paths::episode_stem;
use crate::runner::RunKind;
use crate::ui::app::{AppState, RunRequest};
use crate::ui::components::{Card, Select, Toggle};
use crate::ui::icons::IconWaveform;
use crate::ui::status_log::LogLevel;

#[component]
pub fn RunPage(state: AppState, runner: Coroutine<RunRequest>) -> Element {
    let mut state = state;
    let cfg = state.config.read().clone();
    let running = state.run.read().is_running();
    let roster = state.roster.read().clone();
    let source = state.source.read().clone();

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

    let can_run = !running && !source.trim().is_empty();

    rsx! {
        div { class: "content",
            Card { title: "Source",
                input {
                    class: "input",
                    placeholder: "Article URL, a PDF or text file path, or - for stdin",
                    value: "{source}",
                    oninput: move |e: Event<FormData>| state.source.set(e.value().to_string()),
                }
            }

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
                div { class: "card-row",
                    span { class: "card-label", "Tone" }
                    input {
                        class: "input",
                        value: "{cfg.run.tone}",
                        oninput: move |e: Event<FormData>| {
                            let v = e.value().to_string();
                            save_config(&mut state.config, |c| c.run.tone = v);
                        },
                    }
                }
                div { class: "card-row",
                    span { class: "card-label", "Length" }
                    input {
                        class: "input",
                        placeholder: "leave empty to let the article decide",
                        value: "{cfg.run.length}",
                        oninput: move |e: Event<FormData>| {
                            let v = e.value().to_string();
                            save_config(&mut state.config, |c| c.run.length = v);
                        },
                    }
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

            Card { title: "Run",
                div { class: "card-row",
                    span { class: "card-label", "Skip the script review" }
                    Toggle {
                        value: cfg.run.auto_continue,
                        ontoggle: move |v: bool| {
                            save_config(&mut state.config, |c| c.run.auto_continue = v);
                        },
                    }
                }
                div { class: "card-label-hint",
                    if cfg.run.auto_continue {
                        "One process, straight through to audio."
                    } else {
                        "Stops after the script so it can be edited."
                    }
                }
                button {
                    class: "btn btn-primary",
                    disabled: !can_run,
                    onclick: move |_| start_run(state, runner),
                    IconWaveform { size: 16 }
                    if cfg.run.auto_continue { "Make the episode" } else { "Write the script" }
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

/// Build the argv shape for this run and hand it to the coroutine.
///
/// Auto-continue is a different `RunKind`, not two runs chained: the CLI's
/// one-shot path resolves voices *before* the three-minute Claude call, so a
/// bad preset fails in seconds rather than after the tokens are spent.
fn start_run(mut state: AppState, runner: Coroutine<RunRequest>) {
    let Some(paths) = state.paths.peek().clone() else {
        return;
    };
    let cfg = state.config.peek().clone();
    let source = state.source.peek().trim().to_string();
    if source.is_empty() {
        return;
    }

    let stem = episode_stem(&source);
    let script_out = paths.output_dir.join(format!("{stem}.script.txt"));
    let output = paths.output_dir.join(format!("{stem}.wav"));
    let length = if cfg.run.length.trim().is_empty() {
        None
    } else {
        Some(cfg.run.length.clone())
    };
    let voices = selected_or_default(&state);

    let kind = if cfg.run.auto_continue {
        RunKind::OneShot {
            source: source.clone(),
            hosts: cfg.run.hosts,
            tone: cfg.run.tone.clone(),
            length,
            voices: voices.clone(),
            output,
            script_out,
        }
    } else {
        RunKind::Script {
            source: source.clone(),
            hosts: cfg.run.hosts,
            tone: cfg.run.tone.clone(),
            length,
            script_out,
        }
    };

    state.log.write().push(LogLevel::Info, format!("source: {source}"));
    runner.send(RunRequest {
        kind,
        stem,
        source,
        hosts: cfg.run.hosts,
        voices,
        gate_after: !cfg.run.auto_continue,
    });
}
