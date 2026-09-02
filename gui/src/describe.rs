//! Asking `article2pod.py` for the blurb that goes under an episode on Spotify.
//!
//! A one-shot `--describe` call, deliberately **not** routed through
//! `runner.rs`/`RunKind` for the same reason `spotify.rs` is not: that
//! machinery streams an event protocol and shares `state.run`, and this is a
//! single request/response that has to finish inside a send.
//!
//! The Python side owns the model choice, the prompt and the sanitising
//! (`article2pod.fetch_description`), because that is where the SDK and its
//! Claude Code credentials already are — the GUI has no API key and needs
//! none. This module only spawns it and reads one JSON object back.
//!
//! Failing is normal and cheap here. A description is a nicety on an upload
//! that works without one, so the caller reports every outcome and steps over
//! it rather than aborting a send that was otherwise fine.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

use crate::paths::Paths;
use crate::runner::augmented_path;

/// How long a description may take before the send goes on without one.
///
/// The call is one Haiku turn over a script that is already written, which
/// measures in seconds. This is not a budget so much as a guarantee that a
/// wedged `claude` cannot hold the Spotify button hostage.
pub const TIMEOUT: Duration = Duration::from_secs(90);

/// What `--describe` prints. `description` is null when the model declined or
/// the sanitiser refused the answer; `error` carries the read failure when the
/// script itself could not be opened.
#[derive(Debug, Deserialize, PartialEq)]
struct Described {
    description: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug)]
pub enum DescribeError {
    /// The interpreter could not be started at all.
    Spawn(std::io::Error),
    /// It ran and exited non-zero, or could not be waited on.
    Failed(String),
    /// It did not answer inside [`TIMEOUT`].
    TimedOut,
    /// Stdout was not the JSON `--describe` promises.
    Parse(serde_json::Error),
}

impl std::fmt::Display for DescribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "could not run article2pod.py: {e}"),
            Self::Failed(msg) => write!(f, "article2pod.py --describe failed: {msg}"),
            Self::TimedOut => write!(
                f,
                "gave up waiting for a description after {}s",
                TIMEOUT.as_secs()
            ),
            Self::Parse(e) => write!(f, "could not parse the description: {e}"),
        }
    }
}

impl std::error::Error for DescribeError {}

/// Read `--describe`'s stdout.
///
/// Takes the last non-blank line rather than the whole buffer: the Agent SDK
/// shares this stdout with whatever its child `claude` decides to print, and
/// the JSON object is always the last thing written.
fn parse(bytes: &[u8]) -> Result<Option<String>, DescribeError> {
    let text = String::from_utf8_lossy(bytes);
    let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    let described: Described = serde_json::from_str(last).map_err(DescribeError::Parse)?;
    if let Some(err) = described.error {
        return Err(DescribeError::Failed(err));
    }
    Ok(described.description)
}

/// Write a Spotify blurb for `script`, under `title`.
///
/// `Ok(None)` means the call worked and produced nothing usable — an
/// unreadable script, or an answer the sanitiser refused. That is a normal
/// outcome, not a failure.
pub async fn describe(
    paths: &Paths,
    script: &Path,
    title: &str,
) -> Result<Option<String>, DescribeError> {
    let mut cmd = Command::new(&paths.python);
    cmd.arg(&paths.script)
        .arg("--describe")
        .arg(script)
        .arg("--describe-title")
        .arg(title)
        .current_dir(&paths.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // The timeout below drops this future; without kill_on_drop the
        // `claude` it started outlives the send that asked for it.
        .kill_on_drop(true)
        .env("PYTHONUNBUFFERED", "1")
        .env("PATH", augmented_path());

    let output = match tokio::time::timeout(TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(DescribeError::Spawn(e)),
        Err(_) => return Err(DescribeError::TimedOut),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(DescribeError::Failed(if stderr.is_empty() {
            format!("exited with status {:?}", output.status.code())
        } else {
            stderr
        }));
    }

    parse(&output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from a live `--describe` call against output/nejmoa0912658
    // (2026-09-02), shortened.
    const REAL: &[u8] = br#"{"description": "<p>The TOMUS trial declared two surgeries equivalent.</p><p>Both work about equally well.</p>"}"#;

    #[test]
    fn a_blurb_comes_back_as_the_single_line_spotify_wants() {
        let out = parse(REAL).expect("parse").expect("some");
        assert!(out.starts_with("<p>"));
        assert!(!out.contains('\n'));
    }

    #[test]
    fn a_refused_answer_is_none_rather_than_an_error() {
        // The sanitiser declining is a normal outcome: the send goes ahead
        // without a summary instead of reporting a failure.
        assert_eq!(parse(br#"{"description": null}"#).expect("parse"), None);
    }

    #[test]
    fn an_unreadable_script_is_reported_rather_than_silently_skipped() {
        let bytes = br#"{"description": null, "error": "No such file or directory"}"#;
        assert!(matches!(parse(bytes), Err(DescribeError::Failed(_))));
    }

    #[test]
    fn chatter_before_the_json_does_not_break_the_parse() {
        // The Agent SDK shares this stdout; the object is always last.
        let bytes = b"some warning from a child\n\n{\"description\": \"<p>Fine.</p>\"}\n";
        assert_eq!(parse(bytes).expect("parse").as_deref(), Some("<p>Fine.</p>"));
    }

    #[test]
    fn empty_stdout_is_a_parse_error_not_an_empty_description() {
        assert!(matches!(parse(b""), Err(DescribeError::Parse(_))));
    }
}
