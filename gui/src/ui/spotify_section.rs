//! Spotify episode management, as a section of the Settings page: what is
//! actually live on the show right now, and deleting it.
//!
//! This is a *remote* view, deliberately separate from the Library. The
//! Library's "sent to Spotify" badge only ever reflects the last send this
//! app made from this machine; this section asks the backend directly, so it
//! is the one place that can answer "what is really up there" or catch an
//! episode deleted from Spotify's own app. Deleting from here reaches back
//! into the Library's sidecar too, so the two views cannot go on disagreeing
//! about one episode after a delete.
//!
//! Collapsed by default: a settings page is read occasionally and this list
//! is one API round trip nobody asked for just by opening it, so the fetch
//! waits for the section to actually be opened.

use dioxus::prelude::*;

use crate::library;
use crate::spotify::{self, Readiness, RemoteEpisode};
use crate::ui::app::AppState;
use crate::ui::app_setup::rescan_library;
use crate::ui::components::Card;
use crate::ui::icons::{IconArrowClockwise, IconCaretDown, IconCaretUp, IconTrash};
use crate::ui::open_external;
use crate::ui::status_log::LogLevel;

#[component]
pub fn SpotifySection(state: AppState) -> Element {
    let mut open = use_signal(|| false);
    // Has a fetch ever completed. Collapsing keeps the data around, so
    // re-expanding does not refetch — only the refresh button does.
    let loaded = use_signal(|| false);
    let episodes = use_signal(Vec::<RemoteEpisode>::new);
    let loading = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    // Which episode's delete is armed — the same two-step arm-then-confirm
    // the Library uses in place of a modal.
    let armed = use_signal(|| None::<String>);

    let list = episodes.read().clone();
    let busy = *loading.read();
    let err = error.read().clone();
    let is_open = *open.read();

    rsx! {
        Card { title: "Spotify",
            div {
                class: "card-row spotify-toggle",
                onclick: move |_| {
                    let now_open = !*open.peek();
                    open.set(now_open);
                    if now_open && !*loaded.peek() {
                        refresh(state, episodes, loading, error, loaded);
                    }
                },
                span { class: "card-label", "Manage episodes" }
                span { class: "spotify-toggle-caret",
                    if is_open { IconCaretUp { size: 16 } } else { IconCaretDown { size: 16 } }
                }
            }

            if is_open {
                div { class: "card-row",
                    span { class: "card-label-hint", "Episodes live on the article2pod show" }
                    button {
                        class: "btn-icon",
                        title: "Refresh from Spotify",
                        disabled: busy,
                        onclick: move |_| refresh(state, episodes, loading, error, loaded),
                        IconArrowClockwise { size: 16 }
                    }
                }

                if let Some(msg) = err {
                    div { class: "script-error", "{msg}" }
                } else if busy && list.is_empty() {
                    div { class: "empty-state",
                        div { class: "empty-state-text", "Loading..." }
                    }
                } else if list.is_empty() {
                    div { class: "empty-state",
                        div { class: "empty-state-text", "Nothing uploaded yet." }
                    }
                } else {
                    for ep in list {
                        SpotifyRow { key: "{ep.episode_uri}", state, episode: ep, episodes, armed }
                    }
                }
            }
        }
    }
}

/// Look up the show and list its episodes, replacing whatever was there.
///
/// No show found — including "not authenticated yet" — is not an error: it
/// is the same "nothing uploaded yet" empty state as a show with zero
/// episodes, so a user who has never sent anything sees a quiet section
/// rather than a scary red one.
fn refresh(
    state: AppState,
    mut episodes: Signal<Vec<RemoteEpisode>>,
    mut loading: Signal<bool>,
    mut error: Signal<Option<String>>,
    mut loaded: Signal<bool>,
) {
    loading.set(true);
    error.set(None);
    spawn(async move {
        let cached = state.config.peek().spotify.show_uri.clone();
        let result = async {
            let Some(show_uri) = spotify::find_show(cached.as_deref()).await? else {
                return Ok(Vec::new());
            };
            spotify::list_episodes(&show_uri).await
        }
        .await;

        match result {
            Ok(list) => episodes.set(list),
            Err(e) => error.set(Some(format!("{e}"))),
        }
        loading.set(false);
        loaded.set(true);
    });
}

#[derive(Props, Clone, PartialEq)]
struct SpotifyRowProps {
    state: AppState,
    episode: RemoteEpisode,
    episodes: Signal<Vec<RemoteEpisode>>,
    armed: Signal<Option<String>>,
}

#[component]
fn SpotifyRow(props: SpotifyRowProps) -> Element {
    let state = props.state;
    let mut armed = props.armed;
    let episodes = props.episodes;
    let ep = props.episode.clone();
    let uri = ep.episode_uri.clone();
    let is_armed = armed.read().as_deref() == Some(uri.as_str());
    let readiness = ep.readiness();

    let dot_class = match readiness {
        Readiness::Ready => "status-dot status-ready",
        Readiness::Failed => "status-dot status-failed",
        Readiness::Pending(_) => "status-dot status-processing",
    };

    // `spotify:episode:<id>` -> the web player URL — the CLI never hands back
    // an https link of its own.
    let web_url = uri
        .strip_prefix("spotify:episode:")
        .map(|id| format!("https://open.spotify.com/episode/{id}"));

    rsx! {
        div { class: "library-card",
            div {
                class: "library-card-title",
                title: if web_url.is_some() { "Open on Spotify" } else { "" },
                onclick: {
                    let web_url = web_url.clone();
                    move |_| {
                        if let Some(url) = &web_url {
                            open_external(url);
                        }
                    }
                },
                "{ep.title}"
            }
            div { class: "library-card-meta",
                span { class: dot_class }
                " {readiness.label()} · {ep.created_at}"
            }
            div { class: "library-card-actions",
                button {
                    class: if is_armed { "btn-icon remove armed" } else { "btn-icon remove" },
                    title: if is_armed { "Click again to delete from Spotify" } else { "Delete from Spotify" },
                    onclick: move |_| {
                        if !is_armed {
                            armed.set(Some(uri.clone()));
                            return;
                        }
                        armed.set(None);
                        let uri = uri.clone();
                        spawn(async move {
                            delete(state, &uri, episodes).await;
                        });
                    },
                    IconTrash { size: 16 }
                }
            }
        }
    }
}

/// Delete one episode from Spotify, drop it from the list, and clear the
/// Library sidecar of any run that recorded it — so a card that says "sent"
/// there stops saying so the moment it no longer is.
async fn delete(mut state: AppState, uri: &str, mut episodes: Signal<Vec<RemoteEpisode>>) {
    if let Err(e) = spotify::delete_episode(uri).await {
        state.log.write().push(LogLevel::Error, format!("spotify: {e}"));
        return;
    }

    state.log.write().push(LogLevel::Info, format!("spotify: deleted {uri}"));
    episodes.write().retain(|ep| ep.episode_uri != uri);

    let Some(paths) = state.paths.peek().clone() else { return };
    let matching: Vec<String> = state
        .episodes
        .read()
        .iter()
        .filter(|ep| ep.meta.as_ref().and_then(|m| m.spotify_episode_uri.as_deref()) == Some(uri))
        .map(|ep| ep.stem.clone())
        .collect();

    for stem in matching {
        let meta_path = library::meta_path(&paths, &stem);
        if let Some(mut meta) = library::read_meta(&meta_path) {
            meta.spotify_episode_uri = None;
            meta.spotify_status = None;
            if let Err(e) = library::write_meta(&meta_path, &meta) {
                tracing::warn!(error = %e, stem, "could not clear the spotify status after a delete");
            }
        }
    }
    rescan_library(state);
}
