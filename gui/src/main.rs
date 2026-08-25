//! Entry point.
//!
//! Plain `fn main()`, never `#[tokio::main]`: Dioxus owns the main thread and
//! the single tokio runtime, and a second one deadlocks in ways that look
//! like a hung window.

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

    // The window goes here once the shell exists (build order step 5).
}
