//! Does a drop yield a real absolute path under the forced x11 backend?
//!
//! Beamer proves `e.files()` carries native paths on this machine, but Beamer
//! does not set `GDK_BACKEND=x11` and this app does. Beneath the HTML event
//! sits a brittle constant — wry's WebKitGTK handler only captures paths when
//! its drag callback fires with `info == 2`, an undocumented index into
//! WebKitGTK's internal target list — so this is worth one minute of evidence
//! before a drop zone is built on top of it.
//!
//! Run it, drop a file on the window, read the path it prints.

use dioxus::desktop::{Config, WindowBuilder};
use dioxus::html::HasFileData;
use dioxus::prelude::*;

fn main() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("GDK_BACKEND").is_none() {
        std::env::set_var("GDK_BACKEND", "x11");
    }

    LaunchBuilder::new()
        .with_cfg(Config::new().with_window(
            WindowBuilder::new()
                .with_title("drop smoke")
                .with_inner_size(dioxus::desktop::LogicalSize::new(520.0_f64, 260.0_f64)),
        ))
        .launch(Smoke);
}

#[component]
fn Smoke() -> Element {
    let mut report = use_signal(|| "drop a file here".to_string());

    rsx! {
        div {
            style: "font: 14px sans-serif; padding: 24px; height: 100vh; \
                    display: flex; align-items: center; justify-content: center; \
                    text-align: center; border: 4px dashed #888;",
            ondragover: move |e| e.prevent_default(),
            ondrop: move |e: Event<DragData>| {
                e.prevent_default();
                let files = e.files();
                let line = match files.into_iter().next() {
                    Some(f) => {
                        let p = f.path();
                        format!(
                            "files() -> {}\nabsolute: {}\nexists: {}",
                            p.display(),
                            p.is_absolute(),
                            p.exists(),
                        )
                    }
                    // The fallback the real zone needs anyway: a URL dragged
                    // out of a browser carries no files.
                    None => {
                        let t = e.data_transfer();
                        format!(
                            "files() was EMPTY. uri-list={:?} plain={:?}",
                            t.get_data("text/uri-list"),
                            t.get_data("text/plain"),
                        )
                    }
                };
                println!("{line}");
                report.set(line);
            },
            pre { style: "margin: 0; font: 13px monospace;", "{report}" }
        }
    }
}
