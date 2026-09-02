pub mod app;
mod app_setup;
pub mod components;
pub mod icons;
pub mod library_page;
pub mod run_page;
pub mod run_state;
pub mod run_strip;
pub mod run_view;
pub mod script_editor;
pub mod script_page;
pub mod settings_page;
pub mod spotify_section;
pub mod status_log;

use dioxus::desktop::tao::window::Icon;
use dioxus::desktop::{Config, WindowBuilder};
use dioxus::prelude::*;

/// Hand a path or URL to whatever the desktop has registered for it.
///
/// Spawned and detached: the opener outlives this call for as long as the
/// player does, and a failure is logged rather than surfaced — the user asked
/// to hear an episode, and freezing the window over it would be worse than the
/// player not opening.
///
/// In-window playback is deliberately not offered in v1.
pub fn open_external(target: &str) {
    match std::process::Command::new("xdg-open").arg(target).spawn() {
        Ok(_) => tracing::info!(target, "opened externally"),
        Err(e) => tracing::warn!(target, error = %e, "could not open"),
    }
}

/// Configure and launch the window. Blocks the main thread for the life of the
/// application.
pub fn launch_app() {
    let icon_image = image::load_from_memory(include_bytes!("../../assets/icon.png"))
        .expect("assets/icon.png must be a readable PNG")
        .to_rgba8();
    let (w, h) = icon_image.dimensions();
    let window_icon =
        Icon::from_rgba(icon_image.into_raw(), w, h).expect("icon must convert to RGBA");

    LaunchBuilder::new()
        .with_cfg(
            Config::new()
                .with_data_directory(webview_data_dir())
                // Transparent + undecorated, with .app-container painting the
                // background and the corner radius. `html, body, #main {
                // background: transparent }` in styles.css is load-bearing:
                // without it the webview paints white square corners over the
                // rounded ones.
                .with_background_color((0, 0, 0, 0))
                .with_window(
                    WindowBuilder::new()
                        .with_title("article2pod")
                        .with_decorations(false)
                        .with_transparent(true)
                        .with_window_icon(Some(window_icon))
                        // Wider than Beamer's 500px because the script editor
                        // and the run log both need room to be readable.
                        //
                        // The height is set by the launcher, which is the one
                        // page that has to fit whole: it is a single object
                        // ending in a button, and a scrollbar through the
                        // middle of it reads as the window being too small for
                        // the app rather than as more content below. At 680 it
                        // overflowed by about 55px, which `.content` absorbed
                        // by clipping its own top padding and the mark with it.
                        .with_inner_size(dioxus::desktop::LogicalSize::new(880.0_f64, 760.0_f64))
                        .with_min_inner_size(dioxus::desktop::LogicalSize::new(
                            720.0_f64, 560.0_f64,
                        )),
                ),
        )
        .launch(app::App);
}

/// The WebView user-data directory, which must be writable and persistent.
fn webview_data_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("article2pod")
}
