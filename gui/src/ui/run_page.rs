//! The Run page: point at an article and start.
//!
//! Everything that is set once — hosts, voices, the device, the log — lives in
//! Settings. What is left here is the one thing that changes per run: the
//! source, whether to stop at the script, and the button.

use std::path::{Path, PathBuf};

use dioxus::html::HasFileData;
use dioxus::prelude::*;

use crate::config::save_config;
use crate::paths::episode_stem;
use crate::runner::RunKind;
use crate::ui::app::{AppState, RunRequest};
use crate::ui::components::Toggle;
use crate::ui::status_log::LogLevel;

/// What the pipeline can read from a local file.
///
/// Dot-extensions, matching the `accept` attribute: dioxus's parser pulls
/// extensions only out of `.ext` forms plus three `*/*` wildcards, so a MIME
/// type there yields an empty filter rather than an error.
const READABLE: [&str; 3] = ["pdf", "txt", "md"];

/// What a source turned out to be.
///
/// The CLI takes all three. Only the first two are reachable from this page —
/// a file by drop or Browse, a URL by dragging a link out of a browser. Stdin
/// stays in the enum because `article2pod.py -` still accepts it and
/// `accept_source` is the gate for every caller, not only this page.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Url(String),
    /// The CLI's `-`.
    Stdin,
    File(PathBuf),
}

/// Classify a source, or say why it cannot be used.
///
/// Drop and Browse both come through here, so the two cannot drift apart on
/// what counts as usable. Refusing here is the whole point:
/// a folder handed to Python fails minutes later, after the ingest stage has
/// already started and the message has scrolled past.
pub fn accept_source(raw: &str) -> Result<Source, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("Nothing chosen yet.".to_string());
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Ok(Source::Url(raw.to_string()));
    }
    if raw == "-" {
        return Ok(Source::Stdin);
    }

    let path = PathBuf::from(raw);

    // Ordered so the most specific complaint wins. A folder is checked first
    // because it has no extension to complain about, and the extension is
    // checked before existence so a mistyped `.docx` is told what is wrong
    // with it rather than merely that it is absent.
    if path.is_dir() {
        return Err(format!(
            "{} is a folder. Point at the article itself.",
            name_of(&path)
        ));
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext {
        Some(e) if READABLE.contains(&e.as_str()) => {}
        Some(e) => {
            return Err(format!(
                "{} is a .{e}. The pipeline reads PDF, txt and md.",
                name_of(&path)
            ))
        }
        None => {
            return Err(format!(
                "{} has no extension, so it is neither a URL nor a file the \
                 pipeline reads.",
                name_of(&path)
            ))
        }
    }

    if !path.exists() {
        return Err(format!("There is no file at {}.", path.display()));
    }
    Ok(Source::File(path))
}

/// How a chosen source shows in the zone: a short label for the chip, and the
/// full thing under it.
///
/// A link gets the same treatment a file does. Without it a dragged-in link
/// would set the source and change nothing on screen, which reads as the drop
/// having been refused.
fn chip_for(raw: &str) -> Option<(String, String)> {
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Some((crate::ui::components::truncate_chars(raw, 52), raw.to_string()));
    }
    let path = PathBuf::from(raw);
    Some((name_of(&path), path.display().to_string()))
}

/// The last component of a path, for a message a reader can match to what
/// they dropped. Falls back to the whole path when there is no last component.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[component]
pub fn RunPage(state: AppState, runner: Coroutine<RunRequest>) -> Element {
    let mut state = state;
    let cfg = state.config.read().clone();
    let running = state.run.read().is_running();
    let source = state.source.read().clone();

    let mut hovering = use_signal(|| false);
    let mut refusal = use_signal(|| None::<String>);

    // Drop and Browse land here identically; there is no second path to keep
    // in step.
    let mut take_file = move |path: PathBuf| match accept_source(&path.to_string_lossy()) {
        Ok(Source::File(p)) => {
            refusal.set(None);
            state.source.set(p.to_string_lossy().into_owned());
        }
        Ok(_) => {}
        Err(why) => refusal.set(Some(why)),
    };

    // Derived from the source rather than tracked alongside it. The zone used
    // to need its own signal to tell a typed path from a typed URL in one
    // shared field; with the field gone, `state.source` is set only by drop and
    // Browse and is the single truth — including across a navigation away and
    // back, which the separate signal had to be seeded for.
    let chosen = chip_for(source.trim());
    let can_run = !running && !source.trim().is_empty();

    rsx! {
        div { class: "content",
            div {
                class: if *hovering.read() { "dropzone dropzone-active" } else { "dropzone" },
                // Without a prevent_default on dragover the browser refuses
                // the drop outright and ondrop never fires at all.
                ondragover: move |e| e.prevent_default(),
                ondragenter: move |_| hovering.set(true),
                // wry never clears its own hovered-path list on leave, so a
                // file drag followed by a text drag can present a stale path.
                // Clearing here is half of what covers that; checking
                // data_transfer below is the other half.
                ondragleave: move |_| hovering.set(false),
                ondrop: move |e: Event<DragData>| {
                    e.prevent_default();
                    hovering.set(false);
                    if let Some(file) = e.files().into_iter().next() {
                        take_file(file.path());
                        return;
                    }
                    // No files means a drag out of a browser rather than a
                    // file manager. Directly useful here: this app's source
                    // may legitimately be a URL.
                    let transfer = e.data_transfer();
                    let text = transfer
                        .get_data("text/uri-list")
                        .or_else(|| transfer.get_data("text/plain"))
                        .unwrap_or_default();
                    let dropped = text.lines().find(|l| l.starts_with("http")).unwrap_or("");
                    if !dropped.is_empty() {
                        refusal.set(None);
                        state.source.set(dropped.to_string());
                    }
                },

                if let Some((label, detail)) = chosen.clone() {
                    div { class: "dropzone-chosen",
                        div { class: "tag-chip",
                            span { "{label}" }
                            button {
                                class: "tag-remove",
                                onclick: move |_| {
                                    refusal.set(None);
                                    state.source.set(String::new());
                                },
                                "\u{2715}"
                            }
                        }
                        div { class: "dropzone-path", "{detail}" }
                    }
                } else {
                    div { class: "dropzone-hint", "Drop a PDF, a text file or a link here" }
                    div { class: "dropzone-browse",
                        span { class: "dropzone-or", "or" }
                        // dioxus-desktop intercepts a click on a file input and
                        // routes it through its own native dialog, which comes
                        // back with a real path. No dependency, no plumbing.
                        label { class: "btn",
                            "Browse\u{2026}"
                            input {
                                r#type: "file",
                                class: "file-input-hidden",
                                accept: ".pdf,.txt,.md",
                                onchange: move |e: Event<FormData>| {
                                    if let Some(f) = e.files().into_iter().next() {
                                        take_file(f.path());
                                    }
                                },
                            }
                        }
                    }
                }
            }

            if let Some(why) = refusal.read().clone() {
                div { class: "source-error", "{why}" }
            }

            div { class: "run-toggle",
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
            }

            button {
                class: "btn btn-primary run-start",
                disabled: !can_run,
                onclick: move |_| start_run(state, runner, refusal),
                if cfg.run.auto_continue { "Make the episode" } else { "Write the script" }
            }
        }
    }
}

/// The selection to run with: the explicit one when present, otherwise the
/// CLI's default expanded, so an unset picker still names real voices.
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
///
/// Neither `--tone` nor `--length` is passed. The CLI's own defaults are the
/// specified behaviour — `--tone` already defaults to "conversational and
/// engaging", and omitting `--length` selects the LENGTH_BY_DENSITY block,
/// which is the "let the article decide" prompt. Passing nothing is the same
/// run with less surface.
fn start_run(
    mut state: AppState,
    runner: Coroutine<RunRequest>,
    mut refusal: Signal<Option<String>>,
) {
    let Some(paths) = state.paths.peek().clone() else {
        return;
    };
    let cfg = state.config.peek().clone();
    let raw = state.source.peek().trim().to_string();

    // Drop and Browse both validate on the way in, so by here this is
    // belt-and-braces — and it is still the only gate a dragged-in link passes
    // through, since that path has no dialog to filter it.
    let source = match accept_source(&raw) {
        Ok(Source::Url(u)) => u,
        Ok(Source::Stdin) => "-".to_string(),
        Ok(Source::File(p)) => p.to_string_lossy().into_owned(),
        Err(why) => {
            refusal.set(Some(why));
            return;
        }
    };
    refusal.set(None);

    let stem = episode_stem(&source);
    let script_out = paths.output_dir.join(format!("{stem}.script.txt"));
    let output = paths.output_dir.join(format!("{stem}.wav"));
    let voices = selected_or_default(&state);

    let kind = if cfg.run.auto_continue {
        RunKind::OneShot {
            source: source.clone(),
            hosts: cfg.run.hosts,
            voices: voices.clone(),
            output,
            script_out,
        }
    } else {
        RunKind::Script {
            source: source.clone(),
            hosts: cfg.run.hosts,
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

#[cfg(test)]
#[path = "run_page_tests.rs"]
mod tests;
