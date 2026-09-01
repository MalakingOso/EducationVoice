//! The review gate between the two stages.
//!
//! Edits are written back to the `.script.txt` before stage 2 runs, so
//! `validate_script` sees edited text on exactly the same terms as generated
//! text. A speaker-id violation therefore fails the same way either way, and
//! comes back as an `error` event shown inline under the editor.

use dioxus::prelude::*;

use crate::paths::episode_stem;
use crate::runner::RunKind;
use crate::ui::app::{AppState, Page, RunRequest};
use crate::ui::components::{script_stats, Card};
use crate::ui::icons::IconWaveform;
use crate::ui::script_editor::ScriptEditor;
use crate::ui::status_log::LogLevel;

#[component]
pub fn ScriptPage(state: AppState, runner: Coroutine<RunRequest>) -> Element {
    let mut state = state;
    let draft = state.draft.read().clone();
    let cfg = state.config.read().clone();
    let running = state.run.read().is_running();
    let error = state.run.read().last_error.clone();
    let script_path = state.run.read().script_path.clone();

    let stats = script_stats(&draft);
    // The pipeline exits hard when the speakers present do not match --hosts,
    // so the mismatch is worth showing before a run rather than after one.
    let hosts_ok = stats.speakers == cfg.run.hosts as usize;
    let can_synth = !running && stats.turns > 0 && hosts_ok && script_path.is_some();

    rsx! {
        div { class: "content",
            if draft.is_empty() {
                Card { title: "No script yet",
                    div { class: "empty-state",
                        div { class: "empty-state-text",
                            "Write a script on the Run page, or pick one from the Library to re-voice."
                        }
                    }
                }
            } else {
                Card { title: "Review the script",
                    ScriptEditor {
                        value: draft.clone(),
                        error: error.clone(),
                        hosts: cfg.run.hosts,
                        oninput: move |v: String| state.draft.set(v),
                    }
                    if !hosts_ok {
                        div { class: "script-error",
                            "This script has {stats.speakers} speakers but the run is set to {cfg.run.hosts} hosts. The pipeline rejects that outright."
                        }
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: !can_synth,
                        onclick: move |_| synthesize(state, runner),
                        IconWaveform { size: 16 }
                        "Synthesize"
                    }
                }
            }
        }
    }
}

/// Save the edited script, then run stage 2 against the file on disk.
fn synthesize(mut state: AppState, runner: Coroutine<RunRequest>) {
    let Some(paths) = state.paths.peek().clone() else {
        return;
    };
    let Some(script_path) = state.run.peek().script_path.clone() else {
        return;
    };
    let cfg = state.config.peek().clone();
    let draft = state.draft.peek().clone();

    // Written back before spawning, never passed as an argument: the CLI takes
    // a path, and validating the file is what makes an edited script and a
    // generated one behave identically.
    if let Err(e) = std::fs::write(&script_path, &draft) {
        state.log.write().push(
            LogLevel::Error,
            format!("could not save {}: {e}", script_path.display()),
        );
        return;
    }

    let stem = script_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.trim_end_matches(".script").to_string())
        .unwrap_or_else(|| episode_stem(&state.source.peek()));

    let voices = if cfg.voices.selected.is_empty() {
        state
            .roster
            .peek()
            .as_ref()
            .map(|r| r.default_for(cfg.run.hosts))
            .unwrap_or_default()
    } else {
        cfg.voices.selected.clone()
    };

    runner.send(RunRequest {
        kind: RunKind::Synth {
            script: script_path,
            hosts: cfg.run.hosts,
            voices: voices.clone(),
            output: paths.output_dir.join(format!("{stem}.wav")),
        },
        stem,
        source: state.source.peek().clone(),
        hosts: cfg.run.hosts,
        voices,
        gate_after: false,
    });
    state.page.set(Page::Run);
}
