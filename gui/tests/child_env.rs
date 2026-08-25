//! What the child process actually inherits.
//!
//! Spawning is normally verified by hand, but the environment is the one part
//! that fails silently: a missing `ZE_AFFINITY_MASK` simply runs on the other
//! GPU, and a missing `PYTHONUNBUFFERED` only shows up as eleven minutes of
//! events arriving in one burst at the end. Neither looks like an error.
//!
//! `Paths` has public fields, so the interpreter can be pointed at a script
//! that prints its own environment.

use std::io::Write;
use std::path::PathBuf;

use article2pod_gui::paths::Paths;
use article2pod_gui::runner::{self, RunEvent, RunKind};

fn env_dumping_paths(tag: &str) -> (Paths, PathBuf) {
    let dir = std::env::temp_dir().join(format!("a2p-env-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let script = dir.join("dump.sh");
    let mut f = std::fs::File::create(&script).expect("script");
    // Ignores the argv it is handed; only the environment is under test.
    writeln!(f, "#!/bin/sh\nenv").expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    (
        Paths {
            root: dir.clone(),
            python: PathBuf::from("/bin/sh"),
            script: script.clone(),
            output_dir: dir.join("output"),
            scripts_dir: dir.join("scripts"),
        },
        dir,
    )
}

/// Collect the child's stderr, which is where `env` output lands once stdout
/// is being parsed as the event stream.
async fn child_env(paths: &Paths, gpu: Option<&str>) -> Vec<String> {
    let mut session = runner::spawn(paths, &RunKind::FetchVoices, gpu).expect("spawn");
    let mut lines = Vec::new();
    while let Some(event) = session.events.recv().await {
        match event {
            // `env` writes to stdout, which the runner tries to parse as
            // events; unparseable lines survive as Unparsed, which is exactly
            // the behaviour that makes this readable.
            RunEvent::Unparsed(l) | RunEvent::Stderr(l) => lines.push(l),
            RunEvent::Py(_) | RunEvent::Exited(_) => {}
        }
    }
    lines
}

#[tokio::test]
async fn the_child_is_told_not_to_buffer_its_output() {
    let (paths, dir) = env_dumping_paths("buffer");
    let lines = child_env(&paths, None).await;
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        lines.iter().any(|l| l == "PYTHONUNBUFFERED=1"),
        "without this the child's stdout is block-buffered into a pipe and an \
         eleven-minute run delivers every event at once, at the end. Saw:\n{}",
        lines.join("\n")
    );
}

#[tokio::test]
async fn selecting_a_gpu_sets_the_mask_the_child_reads() {
    let (paths, dir) = env_dumping_paths("gpu");
    let lines = child_env(&paths, Some("1")).await;
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        lines.iter().any(|l| l == "ZE_AFFINITY_MASK=1"),
        "a dropped mask silently runs on the other card rather than failing. \
         Saw:\n{}",
        lines.join("\n")
    );
}

#[tokio::test]
async fn no_gpu_selection_leaves_the_mask_unset_for_python_to_decide() {
    let (paths, dir) = env_dumping_paths("auto");
    let lines = child_env(&paths, None).await;
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        !lines.iter().any(|l| l.starts_with("ZE_AFFINITY_MASK=")),
        "'Automatic' must leave device choice to the Python side; an empty \
         mask string would hide every GPU instead"
    );
}

#[tokio::test]
async fn local_bin_leads_the_path_so_a_desktop_launcher_can_find_claude() {
    let (paths, dir) = env_dumping_paths("path");
    let lines = child_env(&paths, None).await;
    let _ = std::fs::remove_dir_all(&dir);

    let path = lines
        .iter()
        .find(|l| l.starts_with("PATH="))
        .expect("PATH must reach the child");
    let first = path.trim_start_matches("PATH=").split(':').next().unwrap();
    let expected = dirs::home_dir().unwrap().join(".local/bin");
    assert_eq!(
        first,
        expected.to_string_lossy(),
        "a GUI started from a desktop launcher inherits a far thinner PATH \
         than one started from a terminal"
    );
}
