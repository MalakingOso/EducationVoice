//! Proves the event stream and the cancel path without a window.
//!
//! Build order step 4: the process plumbing has to be known-good before any
//! UI sits on top of it, because every failure mode here (a lost `done`, an
//! orphaned `claude`, a block-buffered pipe) looks like a frozen window
//! rather than like a process bug.
//!
//! `#[tokio::main]` is correct *here* and nowhere else in this crate: the
//! "never create a second runtime" rule guards the Dioxus binary, which owns
//! the main thread and its runtime already.

use std::path::PathBuf;

use anyhow::{bail, Result};
use article2pod_gui::paths::Paths;
use article2pod_gui::proto::{Measured, PyEvent};
use article2pod_gui::runner::{self, RunEvent, RunKind};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        bail!(USAGE);
    }

    // `--cancel-after N` is the point of this binary as much as the streaming
    // is: it is how the "no orphaned claude" check is run.
    let mut cancel_after: Option<u64> = None;
    let mut gpu_mask: Option<String> = None;
    let mut write_model = "claude-sonnet-5".to_string();
    let mut edit_model = "claude-opus-5".to_string();
    let mut research_model = "claude-sonnet-5".to_string();
    let mut positional: Vec<String> = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--cancel-after" => {
                cancel_after = Some(it.next().unwrap_or_default().parse()?);
            }
            // Proves ZE_AFFINITY_MASK actually reaches the child, which is
            // otherwise only visible as "the run used the card I expected".
            "--gpu" => {
                gpu_mask = it.next();
            }
            "--model" => {
                write_model = it.next().unwrap_or(write_model);
            }
            "--edit-model" => {
                edit_model = it.next().unwrap_or(edit_model);
            }
            "--research-model" => {
                research_model = it.next().unwrap_or(research_model);
            }
            _ => positional.push(a),
        }
    }

    // The roster is a query, not a run, so it never reaches the streaming
    // path — checking it here is what proves that separation holds.
    if positional.first().map(String::as_str) == Some("list-voices") {
        let paths = Paths::resolve()?;
        let roster = article2pod_gui::roster::load(&paths).await?;
        println!("names:   {:?}", roster.names());
        println!("hosts:   {:?}", roster.host_choices());
        for h in roster.host_choices() {
            println!("default {h}: {:?}", roster.default_for(h));
        }
        return Ok(());
    }

    let kind = match positional.first().map(String::as_str) {
        Some("fetch-voices") => RunKind::FetchVoices,
        Some("synth") => RunKind::Synth {
            script: PathBuf::from(arg(&positional, 1, "script path")?),
            hosts: arg(&positional, 2, "hosts")?.parse()?,
            voices: positional.get(4..).unwrap_or_default().to_vec(),
            output: PathBuf::from(arg(&positional, 3, "output path")?),
        },
        Some("script") => RunKind::Script {
            source: arg(&positional, 1, "source")?.to_string(),
            hosts: arg(&positional, 2, "hosts")?.parse()?,
            script_out: PathBuf::from(arg(&positional, 3, "script-out path")?),
            write_model,
            edit_model,
            research_model,
        },
        _ => bail!(USAGE),
    };

    let paths = Paths::resolve()?;
    println!("root:   {}", paths.root.display());
    println!("python: {}", paths.python.display());
    println!("argv:   {:?}", kind.argv());
    println!("---");

    let started = std::time::Instant::now();
    let mut session = runner::spawn(&paths, &kind, gpu_mask.as_deref())?;
    println!("pgid:   {}", session.pgid);

    if let Some(secs) = cancel_after {
        let pgid = session.pgid;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            println!("--- cancelling after {secs}s ---");
            runner::cancel(pgid);
        });
    }

    let mut progress_events = 0usize;
    let mut last_progress = String::new();

    while let Some(event) = session.events.recv().await {
        let t = started.elapsed().as_secs_f32();
        match event {
            // Sampled, so printing every one buries everything else. Counted
            // here and reported once at the end.
            RunEvent::Py(PyEvent::Progress { step, total, .. }) => {
                progress_events += 1;
                last_progress = match Measured::from_step(step, total) {
                    Measured::Fraction(f) => {
                        format!("{step}/{} ({:.0}%)", total.unwrap_or(0), f * 100.0)
                    }
                    Measured::Unmeasurable => format!("step {step}, no denominator"),
                };
            }
            RunEvent::Py(PyEvent::Message { text }) => {
                let head: String = text.chars().take(100).collect();
                println!("[{t:6.1}s] message  {head}...");
            }
            RunEvent::Py(other) => println!("[{t:6.1}s] {other:?}"),
            RunEvent::Stderr(line) => println!("[{t:6.1}s] stderr   {line}"),
            RunEvent::Unparsed(line) => println!("[{t:6.1}s] UNPARSED {line}"),
            RunEvent::Exited(outcome) => {
                println!("---");
                println!("progress events: {progress_events} (last: {last_progress})");
                println!("[{t:6.1}s] EXITED {outcome:?}");
            }
        }
    }

    // Reaching here means the channel closed, which only happens after the
    // exit event was sent and every sender dropped.
    println!("channel closed cleanly");
    Ok(())
}

fn arg<'a>(v: &'a [String], i: usize, what: &str) -> Result<&'a str> {
    match v.get(i) {
        Some(s) => Ok(s.as_str()),
        None => bail!("missing {what}\n\n{USAGE}"),
    }
}

const USAGE: &str = "\
usage:
  run_smoke list-voices
  run_smoke fetch-voices
  run_smoke synth  <script> <hosts> <output> [voice ...] [--cancel-after S] [--gpu N]
  run_smoke script <source> <hosts> <script-out>         [--cancel-after S] [--model M] [--edit-model M] [--research-model M]";
