//! Do HTML5 `draggable` reorder events and `document::eval` caret reads
//! actually work on this machine's wry/WebKitGTK backend?
//!
//! `drop_smoke.rs` already proved one brittle path through this backend (an
//! OS-level file drop only reaches Dioxus through an undocumented internal
//! index). Internal HTML5 `draggable` drag-and-drop is a *different* code
//! path through the same backend, and `document::eval` reading a DOM
//! property is a technique this codebase has never used at all — neither is
//! safe to assume. Both are what the script editor's turn-reorder and
//! split-at-caret features would be built on, so both get proven here first,
//! with a named fallback ready if either fails: move-up/move-down buttons for
//! reorder, split-at-end-of-turn for caret.
//!
//! Run it, drag an item to reorder the list, then click "Report caret" with
//! text selected in the textarea below. Read what prints.

use dioxus::desktop::{Config, WindowBuilder};
use dioxus::prelude::*;

fn main() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("GDK_BACKEND").is_none() {
        std::env::set_var("GDK_BACKEND", "x11");
    }

    LaunchBuilder::new()
        .with_cfg(Config::new().with_window(
            WindowBuilder::new()
                .with_title("reorder smoke")
                .with_inner_size(dioxus::desktop::LogicalSize::new(420.0_f64, 480.0_f64)),
        ))
        .launch(Smoke);
}

#[component]
fn Smoke() -> Element {
    let mut items = use_signal(|| vec!["one".to_string(), "two".to_string(), "three".to_string()]);
    let mut drag_index = use_signal(|| None::<usize>);
    let mut drag_report = use_signal(|| "drag an item".to_string());
    let mut caret_report = use_signal(|| "type below, select some text, then click Report".to_string());

    rsx! {
        div { style: "font: 14px sans-serif; padding: 16px;",
            h3 { "Reorder" }
            for (i, item) in items.read().clone().into_iter().enumerate() {
                div {
                    key: "{item}",
                    draggable: "true",
                    style: "padding: 8px; margin-bottom: 4px; border: 2px solid #888; \
                            background: #eee; cursor: grab;",
                    ondragstart: move |_| {
                        drag_index.set(Some(i));
                        drag_report.set(format!("dragstart on index {i}"));
                    },
                    ondragover: move |e| e.prevent_default(),
                    ondrop: move |e| {
                        e.prevent_default();
                        let Some(from) = drag_index.take() else {
                            drag_report.set("drop fired but no dragstart was ever seen".into());
                            return;
                        };
                        let mut v = items.write();
                        let moved = v.remove(from);
                        let to = i.min(v.len());
                        v.insert(to, moved);
                        drag_report.set(format!("moved index {from} -> {to}"));
                    },
                    "{item}"
                }
            }
            pre { style: "font: 12px monospace;", "{drag_report}" }

            h3 { "Caret / selection" }
            textarea {
                id: "smoke-ta",
                style: "width: 100%; height: 80px;",
                value: "Speaker 1: select some of this text, then click Report.",
            }
            button {
                onclick: move |_| {
                    spawn(async move {
                        // `document::eval` is dioxus's own desktop/webview
                        // bridge for running a fixed, hand-written script
                        // against the DOM — not an interpreter over
                        // untrusted input. The string below is a constant.
                        let result = document::eval(
                            "const el = document.getElementById('smoke-ta'); \
                             return {start: el.selectionStart, end: el.selectionEnd};",
                        )
                        .await;
                        caret_report.set(format!("{result:?}"));
                    });
                },
                "Report caret"
            }
            pre { style: "font: 12px monospace; white-space: pre-wrap;", "{caret_report}" }
        }
    }
}
