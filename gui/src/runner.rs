//! Spawning `article2pod.py` and streaming its progress back.
//!
//! **Why `tokio::process` and not `std::process`.** Beamer uses
//! `std::process::Command` exclusively, but every child it spawns is a
//! sub-second `.output()` call. This child streams for eight to eleven
//! minutes and has to stay cancellable the whole time, which needs an async
//! reader and a live handle. That is the reason for the divergence; it is not
//! a stylistic preference.
//!
//! **Why the process group matters.** `claude_agent_sdk` spawns the `claude`
//! CLI as a *grandchild*. Killing the Python pid leaves that orphan running,
//! still burning tokens. So the child is spawned into its own process group
//! and cancellation signals the whole group.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::paths::Paths;
use crate::proto::{self, PyEvent, Stage};

/// Depth of the event channel. Comfortably more than the ~200 progress events
/// a whole synthesis emits, so the buffer only matters if the UI stops
/// draining entirely.
const EVENT_CAPACITY: usize = 256;

/// How long a cancelled group gets to exit on SIGTERM before SIGKILL.
///
/// Generous on purpose: the Python child may be mid-write to a WAV, and
/// VibeVoice holds several GB of VRAM that a clean exit releases in order.
const CANCEL_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// How one invocation of the CLI is shaped.
///
/// Each variant is one of the argv shapes the GUI can produce. Keeping this a
/// closed enum with a pure `argv()` is what lets the command lines be tested
/// without spawning anything — and what keeps orchestration out of this
/// module: auto-continue is a different variant, not a different code path.
#[derive(Debug, Clone, PartialEq)]
pub enum RunKind {
    /// Stage 1 of the gated flow. Deliberately never touches voices, so it
    /// cannot fail on a voice problem after spending Claude tokens.
    ///
    /// Neither `--tone` nor `--length` appears in any variant. The CLI's own
    /// defaults are the specified behaviour, so passing them would only
    /// restate them — and omitting `--length` is what selects the
    /// "let the article decide" prompt block rather than a duration.
    Script {
        source: String,
        hosts: u8,
        script_out: PathBuf,
        /// `--model`, the writer pass.
        write_model: String,
        /// `--edit-model`, the creative-director edit pass.
        edit_model: String,
        /// `--research-model`, the researcher sub-agent the writer delegates to.
        research_model: String,
    },
    /// Stage 2: synthesize a script that already exists on disk, edited or not.
    Synth {
        script: PathBuf,
        hosts: u8,
        voices: Vec<String>,
        output: PathBuf,
    },
    /// Auto-continue: the CLI's own one-shot path, not two runs chained.
    ///
    /// This is the already-tested route, and it keeps the CLI's deliberate
    /// fail-fast ordering — `resolve_voices` runs *before* the three-minute
    /// Claude call. Chaining two runs would push a voice-download failure to
    /// after the tokens were spent.
    OneShot {
        source: String,
        hosts: u8,
        voices: Vec<String>,
        output: PathBuf,
        script_out: PathBuf,
        /// `--model`, the writer pass.
        write_model: String,
        /// `--edit-model`, the creative-director edit pass.
        edit_model: String,
        /// `--research-model`, the researcher sub-agent the writer delegates to.
        research_model: String,
    },
    /// Resolve voices up front, so the gated flow fails in seconds rather
    /// than at the gate.
    ///
    /// Reading the voice *roster* is deliberately not a variant here: it is a
    /// one-shot query whose stdout is a single JSON document rather than a
    /// stream of events, and it lives in `roster.rs`. Routing it through this
    /// module would hand a pretty-printed object to the line-by-line parser
    /// and yield nothing but warnings.
    FetchVoices,
}

impl RunKind {
    /// The arguments after the interpreter and the script path.
    pub fn argv(&self) -> Vec<String> {
        let mut a: Vec<String> = Vec::new();
        match self {
            RunKind::Script {
                source,
                hosts,
                script_out,
                write_model,
                edit_model,
                research_model,
            } => {
                a.push(source.clone());
                push_hosts(&mut a, *hosts);
                a.push("--script-only".into());
                push_path(&mut a, "--script-out", script_out);
                push_model(&mut a, "--model", write_model);
                push_model(&mut a, "--edit-model", edit_model);
                push_model(&mut a, "--research-model", research_model);
                a.push("--progress-json".into());
            }
            RunKind::Synth {
                script,
                hosts,
                voices,
                output,
            } => {
                push_path(&mut a, "--from-script", script);
                push_hosts(&mut a, *hosts);
                push_voices(&mut a, voices);
                push_path(&mut a, "--output", output);
                a.push("--progress-json".into());
            }
            RunKind::OneShot {
                source,
                hosts,
                voices,
                output,
                script_out,
                write_model,
                edit_model,
                research_model,
            } => {
                a.push(source.clone());
                push_hosts(&mut a, *hosts);
                push_voices(&mut a, voices);
                push_path(&mut a, "--output", output);
                push_path(&mut a, "--script-out", script_out);
                push_model(&mut a, "--model", write_model);
                push_model(&mut a, "--edit-model", edit_model);
                push_model(&mut a, "--research-model", research_model);
                a.push("--progress-json".into());
            }
            // Errors still need to arrive as events, so this one carries the
            // flag even though it reports nothing on success.
            RunKind::FetchVoices => {
                a.push("--fetch-voices".into());
                a.push("--progress-json".into());
            }
        }
        a
    }

    /// Whether this invocation is a long-running episode rather than a query.
    /// Queries finish in seconds and never need the run strip.
    pub fn is_episode(&self) -> bool {
        match self {
            RunKind::Script { .. } | RunKind::Synth { .. } | RunKind::OneShot { .. } => true,
            RunKind::FetchVoices => false,
        }
    }

    /// The stages this invocation will actually run, in order.
    ///
    /// The run view draws one row per entry, so this is what stops a gated
    /// stage 2 from listing an ingest and a script it is never going to
    /// perform — those ran in a *previous process*, and `RunState::begin`
    /// wipes the struct between the two. A `Synth` started from the Library to
    /// re-voice an old script has no earlier stages at all, which is the same
    /// answer for a different reason.
    pub fn stages(&self) -> &'static [Stage] {
        match self {
            RunKind::Script { .. } => &[Stage::Ingest, Stage::Script],
            RunKind::Synth { .. } => &[Stage::Tts],
            RunKind::OneShot { .. } => &[Stage::Ingest, Stage::Script, Stage::Tts],
            RunKind::FetchVoices => &[],
        }
    }
}

fn push_hosts(a: &mut Vec<String>, hosts: u8) {
    a.push("--hosts".into());
    a.push(hosts.to_string());
}

fn push_path(a: &mut Vec<String>, flag: &str, p: &Path) {
    a.push(flag.into());
    a.push(p.to_string_lossy().into_owned());
}

fn push_model(a: &mut Vec<String>, flag: &str, model: &str) {
    a.push(flag.into());
    a.push(model.to_string());
}

/// `--voices` takes N names in speaker order. Omitted entirely when empty, so
/// the CLI applies its own `DEFAULT_ROSTER` rather than receiving a bare flag
/// with no values, which argparse rejects.
fn push_voices(a: &mut Vec<String>, voices: &[String]) {
    if voices.is_empty() {
        return;
    }
    a.push("--voices".into());
    a.extend(voices.iter().cloned());
}

/// How a run ended. An enum rather than a bool because "cancelled" and
/// "failed" lead to different UI and different history entries, and collapsing
/// them loses the distinction exactly when it matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Cancelled,
    Failed { code: Option<i32> },
}

/// One item on the wire between the child and the UI.
#[derive(Debug, Clone)]
pub enum RunEvent {
    /// A parsed protocol event from stdout.
    Py(PyEvent),
    /// A line of stderr, verbatim. This is where a Python traceback lands.
    Stderr(String),
    /// A stdout line that did not parse. Surfaced rather than silently
    /// dropped, but never fatal — a truncated line at kill time is normal.
    Unparsed(String),
    /// Always the last event on the channel.
    Exited(RunOutcome),
}

/// A running child, shaped after Beamer's `RealtimeSession`.
pub struct RunSession {
    pub events: mpsc::Receiver<RunEvent>,
    /// Process group id, equal to the child's pid because it was spawned with
    /// `process_group(0)`. Pass to [`cancel`].
    pub pgid: i32,
}

/// Spawn the CLI and start streaming.
///
/// `gpu_mask` becomes `ZE_AFFINITY_MASK`; `Some("1")` selects the B60 on this
/// machine. `None` leaves device selection to the Python side.
pub fn spawn(paths: &Paths, kind: &RunKind, gpu_mask: Option<&str>) -> Result<RunSession> {
    let args = kind.argv();
    tracing::info!(?args, "spawning article2pod");

    let mut cmd = Command::new(&paths.python);
    cmd.arg(&paths.script)
        .args(&args)
        .current_dir(&paths.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // pgid == pid, so a single kill(-pgid) reaches the `claude` grandchild.
        .process_group(0)
        // Without this the child's stdout is block-buffered when it is a pipe,
        // and eleven minutes of events arrive in one burst at exit.
        .env("PYTHONUNBUFFERED", "1")
        .env("PATH", augmented_path());

    if let Some(mask) = gpu_mask {
        cmd.env("ZE_AFFINITY_MASK", mask);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("could not start {}", paths.python.display()))?;

    let pgid = child.id().context("child exited before its pid was read")? as i32;

    let stdout = child.stdout.take().context("stdout pipe was not captured")?;
    let stderr = child.stderr.take().context("stderr pipe was not captured")?;

    let (tx, rx) = mpsc::channel(EVENT_CAPACITY);

    // Three plain `tokio::spawn`s: none of them touches a `Signal`. A Signal
    // moved onto a tokio thread resolves against the wrong thread-local arena
    // and diverges with no error at all.
    let out_tx = tx.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            let event = match proto::parse_line(&line) {
                Ok(ev) => RunEvent::Py(ev),
                Err(e) => {
                    tracing::warn!(%line, error = %e, "unparseable stdout line, skipping");
                    RunEvent::Unparsed(line)
                }
            };
            if !dispatch(&out_tx, event).await {
                break;
            }
        }
    });

    let err_tx = tx.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if !dispatch(&err_tx, RunEvent::Stderr(line)).await {
                break;
            }
        }
    });

    tokio::spawn(async move {
        let outcome = match child.wait().await {
            Ok(status) => match status.code() {
                Some(0) => RunOutcome::Completed,
                Some(code) => RunOutcome::Failed { code: Some(code) },
                // No exit code means a signal ended it. The only signals this
                // group receives come from `cancel`.
                None => RunOutcome::Cancelled,
            },
            Err(e) => {
                tracing::error!(error = %e, "could not wait on the child");
                RunOutcome::Failed { code: None }
            }
        };
        tracing::info!(?outcome, "run finished");
        // Awaited, never try_send: this is the event that tells the UI the run
        // is over, and dropping it strands the run strip forever.
        let _ = tx.send(RunEvent::Exited(outcome)).await;
    });

    Ok(RunSession { events: rx, pgid })
}

/// Put one event on the channel. Returns false once the receiver is gone.
///
/// **Progress events are droppable; nothing else is.** A progress tick is a
/// sampled signal that arrives ~200 times and is superseded by the next one,
/// so a full channel may discard it. Every other event is meaningful exactly
/// once — a dropped `Done` or `Error` leaves the UI waiting on a run that
/// already ended. So those block until there is room, which at worst applies
/// backpressure to a child that is only writing a few hundred lines an hour.
///
/// This is a deliberate refinement of Beamer's "producers always try_send"
/// convention, which was written for fungible audio frames rather than for a
/// state machine.
async fn dispatch(tx: &mpsc::Sender<RunEvent>, event: RunEvent) -> bool {
    if matches!(event, RunEvent::Py(PyEvent::Progress { .. })) {
        return match tx.try_send(event) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("event channel full, dropped a progress tick");
                true
            }
            // A closed channel means the UI dropped the run. Silent, per
            // Beamer's convention: it is an expected shutdown, not a fault.
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        };
    }
    tx.send(event).await.is_ok()
}

/// Signal a run's whole process group to stop, escalating if it does not.
///
/// Negative pid means "the group", which is what reaches the `claude`
/// grandchild. Verified by `pgrep -af claude` after a cancel during script
/// generation.
pub fn cancel(pgid: i32) {
    tracing::info!(pgid, "cancelling run");
    signal_group(pgid, libc::SIGTERM);

    tokio::spawn(async move {
        tokio::time::sleep(CANCEL_GRACE).await;
        if group_is_alive(pgid) {
            tracing::warn!(pgid, "group survived SIGTERM, escalating to SIGKILL");
            signal_group(pgid, libc::SIGKILL);
        }
    });
}

fn signal_group(pgid: i32, sig: i32) {
    // Safe: kill() only inspects a pid and a signal number, and a group that
    // has already exited returns ESRCH rather than affecting anything else.
    unsafe {
        libc::kill(-pgid, sig);
    }
}

/// Signal 0 performs the permission and existence checks without delivering
/// anything — the standard liveness probe.
fn group_is_alive(pgid: i32) -> bool {
    unsafe { libc::kill(-pgid, 0) == 0 }
}

/// `PATH` with `~/.local/bin` in front.
///
/// `claude` lives there, and a GUI started from a desktop launcher inherits a
/// far thinner environment than one started from a terminal — without this the
/// script stage dies with "claude: not found" only when launched by icon.
pub(crate) fn augmented_path() -> std::ffi::OsString {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/bin"));
    }
    dirs.extend(std::env::split_paths(&current));
    std::env::join_paths(dirs).unwrap_or(current)
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
