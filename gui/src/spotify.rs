//! Sending a finished episode to Spotify via the `save-to-spotify` CLI.
//!
//! A plain subprocess wrapper, deliberately **not** routed through
//! `runner.rs`/`RunKind` — that machinery is shaped around
//! `article2pod.py`'s own event protocol and shares `state.run`, so reusing
//! it here would collide with the article-generation run state. Every call
//! this module makes is a single request/response, not a stream, so
//! `Command::output()` is enough; there is nothing to poll but the CLI's own
//! `episodes status`, which is one call per tick, driven by the caller.
//!
//! Every public function is split into "spawn the CLI" and "parse the bytes
//! it printed", so the parsers can be tested against captured JSON without
//! spawning anything — see the tests below. That split also matters for a
//! reason beyond testing: this account already has two real shows, so
//! nothing in this codebase may call `upload` or `shows create` outside of a
//! deliberate, user-run send.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

const BIN: &str = "save-to-spotify";

/// The one show every episode from this app lands in.
pub const SHOW_TITLE: &str = "article2pod";

/// How long between `episodes status` ticks while a send is in flight.
///
/// The CLI's own docs put most episodes ready in 1-2 minutes and warn of a
/// per-user rate limit, so this is paced closer to a human checking back than
/// to a hot poll.
pub const POLL_INTERVAL: Duration = Duration::from_secs(12);

/// How long a send waits for `READY` before giving up and marking the
/// episode's sidecar status "timed out" — matches the CLI's own `--wait`
/// default, which is the closest thing to a documented expectation for how
/// long this should ever take.
pub const POLL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Everything that can go wrong between here and a finished send.
#[derive(Debug)]
pub enum SpotifyError {
    /// The child failed to spawn at all — almost always `save-to-spotify`
    /// missing from `PATH`. Kept distinct from `Io` so the caller can show
    /// "not installed" instead of a raw OS error.
    NotInstalled,
    /// The CLI ran and reported `{"error": "..."}` on stdout, exit code 1 —
    /// its documented `--json` failure contract.
    Cli(String),
    /// The CLI exited non-zero without the documented error shape, or could
    /// not be waited on.
    Io(std::io::Error),
    /// Stdout was not the JSON this call expected.
    Parse(serde_json::Error),
}

impl std::fmt::Display for SpotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled => write!(
                f,
                "save-to-spotify is not installed (or not on PATH); see \
                 https://saveto.spotify.com for the installer"
            ),
            Self::Cli(msg) => write!(f, "save-to-spotify: {msg}"),
            Self::Io(e) => write!(f, "could not run save-to-spotify: {e}"),
            Self::Parse(e) => write!(f, "could not parse save-to-spotify's output: {e}"),
        }
    }
}

impl std::error::Error for SpotifyError {}

#[derive(Debug, Deserialize)]
struct CliError {
    error: String,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Show {
    show_uri: String,
    title: String,
}

#[derive(Debug, Deserialize, PartialEq)]
struct ShowList {
    shows: Vec<Show>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct CreatedShow {
    show_uri: String,
}

/// What `upload` reports back.
///
/// `status` here is the upload-time state (documented as `"PROCESSING"`),
/// which is a different field under a different name than what
/// `episodes status` returns — see [`StatusResult`].
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct UploadResult {
    pub episode_uri: String,
    pub status: String,
}

/// What `episodes status` reports back.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct StatusResult {
    pub episode_uri: String,
    pub readiness: Readiness,
}

/// Only the two terminal states are named. Everything else the backend
/// reports means "not done yet, ask again".
///
/// This is deliberately open. `cli-usage.md` documents exactly three values
/// -- `READY`, `PROCESSING`, `FAILED` -- but a live upload returns
/// `NOT_READY` for the first minutes after `upload`, which a closed enum
/// rejected as a parse error and the poll loop then recorded as a permanent
/// failure on an episode that was processing fine. Any word that is not
/// `READY` or `FAILED` is now just another way of saying "wait".
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(from = "String")]
pub enum Readiness {
    Ready,
    Failed,
    /// Still working, carrying the backend's own word for it so the log can
    /// say which one without the type having to know it in advance.
    Pending(String),
}

impl From<String> for Readiness {
    fn from(raw: String) -> Self {
        match raw.as_str() {
            "READY" => Self::Ready,
            "FAILED" => Self::Failed,
            _ => Self::Pending(raw),
        }
    }
}

impl Readiness {
    /// The word this app's sidecar records for the state, so the Library
    /// row and the log line agree on vocabulary.
    ///
    /// Every pending value collapses to "processing": the sidecar's
    /// vocabulary is a closed set the Library tooltip reads back, and the
    /// distinction between the backend's pending words is not one a user
    /// can act on.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Pending(_) => "processing",
        }
    }
}

/// Run `save-to-spotify --json <args>` to completion and return its raw
/// stdout on success.
///
/// `SAVE_TO_SPOTIFY_NO_UPDATE_CHECK` is set because the passive update check
/// the CLI runs after a successful command writes to the same stdout this
/// function parses.
async fn run_json(args: &[&str]) -> Result<Vec<u8>, SpotifyError> {
    let mut cmd = Command::new(BIN);
    cmd.arg("--json")
        .args(args)
        .env("SAVE_TO_SPOTIFY_NO_UPDATE_CHECK", "1")
        // `save-to-spotify` lives in ~/.local/bin, the same place `claude`
        // does, and a GUI started from a desktop launcher inherits a far
        // thinner PATH than one started from a terminal. Without this the
        // send reports "not installed" only when launched by icon.
        .env("PATH", crate::runner::augmented_path())
        .stdin(std::process::Stdio::null());

    let output = match cmd.output().await {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(SpotifyError::NotInstalled),
        Err(e) => return Err(SpotifyError::Io(e)),
    };

    if !output.status.success() {
        if let Ok(err) = serde_json::from_slice::<CliError>(&output.stdout) {
            return Err(SpotifyError::Cli(err.error));
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(SpotifyError::Cli(if stderr.is_empty() {
            format!("exited with status {:?}", output.status.code())
        } else {
            stderr
        }));
    }

    Ok(output.stdout)
}

fn parse_shows(bytes: &[u8]) -> Result<Vec<Show>, SpotifyError> {
    serde_json::from_slice::<ShowList>(bytes)
        .map(|l| l.shows)
        .map_err(SpotifyError::Parse)
}

fn parse_created_show(bytes: &[u8]) -> Result<String, SpotifyError> {
    serde_json::from_slice::<CreatedShow>(bytes)
        .map(|s| s.show_uri)
        .map_err(SpotifyError::Parse)
}

fn parse_upload(bytes: &[u8]) -> Result<UploadResult, SpotifyError> {
    serde_json::from_slice(bytes).map_err(SpotifyError::Parse)
}

fn parse_status(bytes: &[u8]) -> Result<StatusResult, SpotifyError> {
    serde_json::from_slice(bytes).map_err(SpotifyError::Parse)
}

/// Find the one show every episode lands in, without creating it.
///
/// Trusts `cached` first — the config's own `spotify.show_uri` — then falls
/// back to listing shows and matching on title. `None` means neither found
/// it; the caller decides whether that is "nothing to show" (the Spotify
/// tab) or "go create it" ([`ensure_show`]).
pub async fn find_show(cached: Option<&str>) -> Result<Option<String>, SpotifyError> {
    if let Some(uri) = cached {
        if !uri.is_empty() {
            return Ok(Some(uri.to_string()));
        }
    }

    let bytes = run_json(&["shows"]).await?;
    let shows = parse_shows(&bytes)?;
    Ok(shows.into_iter().find(|s| s.title == SHOW_TITLE).map(|s| s.show_uri))
}

/// Find or create the one show every episode lands in.
///
/// A normal send costs one process spawn (`upload`) rather than three, via
/// [`find_show`]'s cache-first lookup, and only creates a new show when that
/// finds nothing.
pub async fn ensure_show(cached: Option<&str>, cover: &Path) -> Result<String, SpotifyError> {
    if let Some(uri) = find_show(cached).await? {
        return Ok(uri);
    }

    let cover_arg = cover.to_string_lossy().into_owned();
    let bytes = run_json(&["shows", "create", "--title", SHOW_TITLE, "--image", &cover_arg]).await?;
    parse_created_show(&bytes)
}

/// Upload one episode: a plain audio upload, no chapters or timeline.
///
/// `summary` is the show-notes blurb, written by `describe.rs`. It is optional
/// because it is a nicety the upload works without, but it is the last chance
/// to set one: episode metadata is immutable after creation, so an episode
/// sent without a description can only get one by being deleted and re-sent.
pub async fn upload(
    audio: &Path,
    title: &str,
    show_uri: &str,
    cover: &Path,
    summary: Option<&str>,
) -> Result<UploadResult, SpotifyError> {
    let audio_arg = audio.to_string_lossy().into_owned();
    let cover_arg = cover.to_string_lossy().into_owned();
    let mut args = vec![
        "upload",
        &audio_arg,
        "--title",
        title,
        "--show-id",
        show_uri,
        "--image",
        &cover_arg,
    ];
    if let Some(text) = summary {
        args.push("--summary");
        args.push(text);
    }
    let bytes = run_json(&args).await?;
    parse_upload(&bytes)
}

/// One `episodes status` tick. The caller loops and sleeps between calls —
/// see the module doc — so this never blocks longer than one request.
pub async fn poll_status(episode_id: &str) -> Result<StatusResult, SpotifyError> {
    let bytes = run_json(&["episodes", "status", episode_id]).await?;
    parse_status(&bytes)
}

/// One row of `episodes --show-id <id>` — what the Spotify tab lists.
///
/// `status` here is the same vocabulary as [`StatusResult::readiness`], under
/// yet another field name; `readiness()` reads it through the same
/// [`Readiness`] the upload poll loop uses so the tab and the Library agree
/// on what "ready" means.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RemoteEpisode {
    pub episode_uri: String,
    pub title: String,
    pub status: String,
    pub created_at: String,
}

impl RemoteEpisode {
    pub fn readiness(&self) -> Readiness {
        Readiness::from(self.status.clone())
    }
}

#[derive(Debug, Deserialize, PartialEq)]
struct EpisodeList {
    episodes: Vec<RemoteEpisode>,
}

fn parse_episodes(bytes: &[u8]) -> Result<Vec<RemoteEpisode>, SpotifyError> {
    serde_json::from_slice::<EpisodeList>(bytes)
        .map(|l| l.episodes)
        .map_err(SpotifyError::Parse)
}

/// List every episode in `show_uri`, for the Spotify tab.
pub async fn list_episodes(show_uri: &str) -> Result<Vec<RemoteEpisode>, SpotifyError> {
    let bytes = run_json(&["episodes", "--show-id", show_uri]).await?;
    parse_episodes(&bytes)
}

/// Delete one episode from Spotify. Irreversible on the backend's side — the
/// caller is the Spotify tab's row button, gated behind its own two-step arm,
/// not this module.
pub async fn delete_episode(episode_id: &str) -> Result<(), SpotifyError> {
    run_json(&["episodes", "delete", episode_id]).await?;
    Ok(())
}

/// The bundled cover, written to disk once. `save-to-spotify` takes a real
/// path, not a webview asset URL, so this cannot be `asset!()` — it has to
/// land somewhere on the filesystem the CLI can open.
const COVER_JPG: &[u8] = include_bytes!("../assets/spotify-cover.jpg");

/// Write the bundled cover to `dest` if it is not already there.
///
/// Never overwrites: the file this app ships never changes at runtime, and a
/// user is free to swap the file in place without a future launch clobbering it.
pub fn ensure_cover_image(dest: &Path) -> std::io::Result<()> {
    if dest.exists() {
        return Ok(());
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(dest, COVER_JPG)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from a real `--json shows` call against this account
    // (2026-09-01): two shows, neither named "article2pod".
    const SHOWS_JSON: &[u8] = br#"{"shows":[{"show_uri":"spotify:show:033djloVHFhq3C3DJvwz5E","title":"The Economist","language":"en","created_at":"2026-05-08T19:28:53.763Z","last_episode_uploaded_at":"2026-05-12T21:31:59.244Z"},{"show_uri":"spotify:show:033dNEZAFpOfQAJSpa8TS1","title":"Web Reads","language":"en","created_at":"2026-05-09T16:04:12.972Z","last_episode_uploaded_at":"2026-08-30T22:15:32.215Z"}]}"#;

    // Shape documented in cli-usage.md's "Quick save" example.
    const UPLOAD_JSON: &[u8] =
        br#"{"episode_uri": "spotify:episode:abc123", "title": "My Recording", "status": "PROCESSING"}"#;

    // Shape documented in cli-usage.md's "Episode readiness" section.
    const STATUS_READY_JSON: &[u8] =
        br#"{"episode_uri": "spotify:episode:abc123", "readiness": "READY"}"#;
    const STATUS_PROCESSING_JSON: &[u8] =
        br#"{"episode_uri": "spotify:episode:abc123", "readiness": "PROCESSING"}"#;
    const STATUS_FAILED_JSON: &[u8] =
        br#"{"episode_uri": "spotify:episode:abc123", "readiness": "FAILED"}"#;
    // Captured from a live `episodes status` two minutes after an upload
    // (2026-09-02). Not in cli-usage.md's list of readiness values, which is
    // the whole reason `Readiness` no longer has a closed set of them.
    const STATUS_NOT_READY_JSON: &[u8] =
        br#"{"episode_uri":"spotify:episode:abc123","readiness":"NOT_READY"}"#;

    // Captured from a live `--json shows get doesnotexist` call.
    const ERROR_JSON: &[u8] =
        br#"{"error":"API error (404): {\"error_code\":\"RESOURCE_NOT_FOUND\",\"message\":\"The specified show was not found\"}"}"#;

    // Captured from a real `--json episodes` call against this account
    // (2026-09-02).
    const EPISODES_JSON: &[u8] = br#"{"episodes":[{"episode_uri":"spotify:episode:0zgP6fArigUgmq3dJucmSK","title":"A Randomized Trial of Urodynamic Testing before Stress-Incontinence Surgery","language":"en","media_type":"EPISODE_AUDIO","status":"READY","created_at":"2026-09-02T01:56:18.140Z"},{"episode_uri":"spotify:episode:5mmg7NX3BTW1JLjxLqj4ua","title":"Risk of thrombosis and bleeding in gynecologic noncancer surgery: systematic review and meta-analysis","language":"en","media_type":"EPISODE_AUDIO","status":"READY","created_at":"2026-09-02T07:12:16.063Z"}]}"#;

    #[test]
    fn a_show_list_without_article2pod_finds_nothing() {
        let shows = parse_shows(SHOWS_JSON).expect("parse");
        assert!(shows.iter().all(|s| s.title != SHOW_TITLE));
        assert_eq!(shows.len(), 2);
    }

    #[test]
    fn upload_reports_the_uri_and_the_upload_time_status_field() {
        let result = parse_upload(UPLOAD_JSON).expect("parse");
        assert_eq!(result.episode_uri, "spotify:episode:abc123");
        assert_eq!(result.status, "PROCESSING");
    }

    #[test]
    fn status_uses_readiness_not_status_and_it_is_a_distinct_field_from_upload() {
        // The trap this module exists to avoid: upload's "status" and
        // episodes status's "readiness" are not the same key.
        assert!(serde_json::from_slice::<StatusResult>(UPLOAD_JSON).is_err());

        let ready = parse_status(STATUS_READY_JSON).expect("parse");
        assert_eq!(ready.readiness, Readiness::Ready);
        assert_eq!(ready.readiness.label(), "ready");

        let processing = parse_status(STATUS_PROCESSING_JSON).expect("parse");
        assert_eq!(processing.readiness.label(), "processing");

        let failed = parse_status(STATUS_FAILED_JSON).expect("parse");
        assert_eq!(failed.readiness, Readiness::Failed);
    }

    #[test]
    fn an_undocumented_readiness_value_is_pending_rather_than_an_error() {
        // The bug this guards: NOT_READY used to fail deserialization, and
        // the poll loop turned that parse error into a permanent "failed"
        // on the sidecar of an episode Spotify was still processing.
        let status = parse_status(STATUS_NOT_READY_JSON).expect("parse");
        assert_eq!(status.readiness, Readiness::Pending("NOT_READY".to_string()));
        assert_eq!(status.readiness.label(), "processing");
        assert_eq!(
            parse_status(STATUS_PROCESSING_JSON).expect("parse").readiness,
            Readiness::Pending("PROCESSING".to_string()),
        );
    }

    #[test]
    fn a_cli_error_envelope_is_not_mistaken_for_a_show_list() {
        assert!(parse_shows(ERROR_JSON).is_err());
    }

    #[test]
    fn an_episode_list_parses_and_its_status_reads_through_readiness() {
        let episodes = parse_episodes(EPISODES_JSON).expect("parse");
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].episode_uri, "spotify:episode:0zgP6fArigUgmq3dJucmSK");
        assert_eq!(episodes[0].readiness(), Readiness::Ready);
    }

    #[test]
    fn an_episode_lists_status_field_is_not_mistaken_for_upload_or_status_results_fields() {
        // `episodes` uses "status" for the same value `episodes status` calls
        // "readiness" — the same trap as upload's "status" vs. that field,
        // just one level further out.
        assert!(serde_json::from_slice::<EpisodeList>(STATUS_READY_JSON).is_err());
    }
}
