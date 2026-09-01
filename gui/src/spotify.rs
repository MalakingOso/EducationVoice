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

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum Readiness {
    #[serde(rename = "READY")]
    Ready,
    #[serde(rename = "PROCESSING")]
    Processing,
    #[serde(rename = "FAILED")]
    Failed,
}

impl Readiness {
    /// The word this app's sidecar records for the state, so the Library
    /// row and the log line agree on vocabulary.
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Processing => "processing",
            Self::Failed => "failed",
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

/// Find or create the one show every episode lands in.
///
/// Trusts `cached` first — the config's own `spotify.show_uri` — so a normal
/// send costs one process spawn (`upload`) rather than three. Falls back to
/// listing shows and matching on title, and only creates a new one when
/// neither finds it.
pub async fn ensure_show(cached: Option<&str>, cover: &Path) -> Result<String, SpotifyError> {
    if let Some(uri) = cached {
        if !uri.is_empty() {
            return Ok(uri.to_string());
        }
    }

    let bytes = run_json(&["shows"]).await?;
    let shows = parse_shows(&bytes)?;
    if let Some(existing) = shows.into_iter().find(|s| s.title == SHOW_TITLE) {
        return Ok(existing.show_uri);
    }

    let cover_arg = cover.to_string_lossy().into_owned();
    let bytes = run_json(&["shows", "create", "--title", SHOW_TITLE, "--image", &cover_arg]).await?;
    parse_created_show(&bytes)
}

/// Upload one episode: a plain audio upload, no chapters or timeline.
pub async fn upload(
    audio: &Path,
    title: &str,
    show_uri: &str,
    cover: &Path,
) -> Result<UploadResult, SpotifyError> {
    let audio_arg = audio.to_string_lossy().into_owned();
    let cover_arg = cover.to_string_lossy().into_owned();
    let bytes = run_json(&[
        "upload",
        &audio_arg,
        "--title",
        title,
        "--show-id",
        show_uri,
        "--image",
        &cover_arg,
    ])
    .await?;
    parse_upload(&bytes)
}

/// One `episodes status` tick. The caller loops and sleeps between calls —
/// see the module doc — so this never blocks longer than one request.
pub async fn poll_status(episode_id: &str) -> Result<StatusResult, SpotifyError> {
    let bytes = run_json(&["episodes", "status", episode_id]).await?;
    parse_status(&bytes)
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

    // Captured from a live `--json shows get doesnotexist` call.
    const ERROR_JSON: &[u8] =
        br#"{"error":"API error (404): {\"error_code\":\"RESOURCE_NOT_FOUND\",\"message\":\"The specified show was not found\"}"}"#;

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
        assert_eq!(processing.readiness, Readiness::Processing);

        let failed = parse_status(STATUS_FAILED_JSON).expect("parse");
        assert_eq!(failed.readiness, Readiness::Failed);
    }

    #[test]
    fn a_cli_error_envelope_is_not_mistaken_for_a_show_list() {
        assert!(parse_shows(ERROR_JSON).is_err());
    }
}
