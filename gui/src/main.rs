//! Entry point.
//!
//! Plain `fn main()`, never `#[tokio::main]`: Dioxus owns the main thread and
//! the single tokio runtime, and a second one deadlocks in ways that look
//! like a hung window.

/// Force the GTK/WebKit stack onto XWayland.
///
/// On this machine WebKitGTK's Wayland backend never completes its IPC
/// handshake with the host process: the window opens, the component renders
/// and its hooks run, but `onmounted` never arrives and dioxus therefore never
/// enters its task-polling loop. Every `spawn` and `use_future` sits queued
/// forever, so the app looks completely healthy and does nothing at all — no
/// error, no warning, no crash.
///
/// Measured 2026-08-25 against dioxus 0.7.10 under a Wayland session: a
/// twelve-line `dioxus::launch` app reproduces it, which is what rules out
/// this crate's own code. `WEBKIT_DISABLE_DMABUF_RENDERER`,
/// `WEBKIT_DISABLE_COMPOSITING_MODE` and `LIBGL_ALWAYS_SOFTWARE` all leave it
/// broken; only the backend switch helps.
///
/// Applied before any GTK call, because GDK reads this once at initialisation
/// and ignores later changes. An existing value is left alone, and
/// `ARTICLE2POD_KEEP_GDK_BACKEND=1` opts out entirely, so this stops being
/// load-bearing the moment the underlying bug is fixed.
#[cfg(target_os = "linux")]
fn force_x11_backend() {
    if std::env::var_os("ARTICLE2POD_KEEP_GDK_BACKEND").is_some()
        || std::env::var_os("GDK_BACKEND").is_some()
    {
        return;
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        std::env::set_var("GDK_BACKEND", "x11");
        tracing::info!("GDK_BACKEND=x11 (WebKitGTK's Wayland backend stalls the vdom)");
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new("article2pod_gui=info")
                }),
        )
        .init();

    tracing::info!("article2pod starting");

    #[cfg(target_os = "linux")]
    force_x11_backend();

    // Blocks for the life of the app. Dioxus owns the main thread from here.
    article2pod_gui::ui::launch_app();
}
