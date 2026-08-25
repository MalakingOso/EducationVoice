//! Tests for `runner.rs`, in a sibling file because the parent is near the
//! 500-line ceiling. Spawning and signalling are verified by hand (see the
//! cancel checks in the plan's verification section); what is testable here
//! without a process is the argv construction, and that is where a silent
//! mistake would cost a real run.

use super::*;

fn script_kind() -> RunKind {
    RunKind::Script {
        source: "/home/berkley/ROTBIGS.pdf".into(),
        hosts: 2,
        script_out: PathBuf::from("output/rotbigs.script.txt"),
    }
}

#[test]
fn stage_one_asks_for_a_script_and_never_mentions_voices() {
    let argv = script_kind().argv();
    assert!(argv.contains(&"--script-only".to_string()));
    assert!(
        !argv.iter().any(|a| a.starts_with("--voice")),
        "stage 1 must not resolve voices: it runs before the gate, and a voice \
         failure there would land after the Claude tokens were already spent"
    );
    assert!(argv.contains(&"--progress-json".to_string()));
}

#[test]
fn the_source_leads_the_argv_because_it_is_a_positional() {
    assert_eq!(
        script_kind().argv().first().map(String::as_str),
        Some("/home/berkley/ROTBIGS.pdf"),
        "argparse takes `source` positionally; a flag before it still parses, \
         but a positional after `--voices` would be swallowed as a voice name"
    );
}

#[test]
fn neither_tone_nor_length_is_ever_passed() {
    // The GUI does not expose either, and the CLI's own defaults are the
    // specified behaviour: --tone already defaults to "conversational and
    // engaging", and omitting --length is what selects LENGTH_BY_DENSITY,
    // the "let the article decide" prompt block. Passing either would only
    // restate a default, or — for --length — silently replace that block
    // with a duration nobody asked for.
    for kind in [
        script_kind(),
        RunKind::OneShot {
            source: "https://example.com/paper".into(),
            hosts: 2,
            voices: vec!["alice".into(), "carter".into()],
            output: PathBuf::from("output/paper.wav"),
            script_out: PathBuf::from("output/paper.script.txt"),
        },
    ] {
        let argv = kind.argv();
        assert!(!argv.contains(&"--tone".to_string()), "in {argv:?}");
        assert!(!argv.contains(&"--length".to_string()), "in {argv:?}");
    }
}

#[test]
fn stage_two_reads_a_script_from_disk_and_names_every_voice_in_order() {
    let argv = RunKind::Synth {
        script: PathBuf::from("output/rotbigs.script.txt"),
        hosts: 3,
        voices: vec!["alice".into(), "carter".into(), "maya".into()],
        output: PathBuf::from("output/rotbigs.wav"),
    }
    .argv();

    let v = argv.iter().position(|a| a == "--voices").expect("flag present");
    assert_eq!(
        &argv[v + 1..v + 4],
        &["alice", "carter", "maya"],
        "voice order binds to Speaker 1..N and is not a set"
    );
    assert!(argv.contains(&"--from-script".to_string()));
    assert!(
        argv.contains(&"--hosts".to_string()),
        "--hosts must carry from stage 1 into stage 2; a mismatch is a hard \
         exit inside validate_script"
    );
}

#[test]
fn an_empty_voice_list_omits_the_flag_so_the_cli_applies_its_own_roster() {
    let argv = RunKind::Synth {
        script: PathBuf::from("s.txt"),
        hosts: 2,
        voices: vec![],
        output: PathBuf::from("o.wav"),
    }
    .argv();
    assert!(
        !argv.contains(&"--voices".to_string()),
        "a bare --voices with no values is an argparse error, not a default"
    );
}

#[test]
fn auto_continue_is_one_invocation_carrying_both_outputs() {
    let argv = RunKind::OneShot {
        source: "https://example.com/paper".into(),
        hosts: 2,
        voices: vec!["alice".into(), "carter".into()],
        output: PathBuf::from("output/paper.wav"),
        script_out: PathBuf::from("output/paper.script.txt"),
    }
    .argv();

    assert!(argv.contains(&"--output".to_string()));
    assert!(
        argv.contains(&"--script-out".to_string()),
        "the script still has to be saved per-run, or history loses it"
    );
    assert!(
        !argv.contains(&"--script-only".to_string()),
        "auto-continue runs straight through; --script-only would stop at the gate"
    );
    assert!(
        !argv.contains(&"--from-script".to_string()),
        "this is the CLI's one-shot path, not two runs chained"
    );
}

#[test]
fn fetching_voices_still_reports_failure_as_an_event() {
    assert_eq!(
        RunKind::FetchVoices.argv(),
        vec!["--fetch-voices".to_string(), "--progress-json".to_string()],
        "it reports nothing on success, but a download failure has to arrive \
         as an error event rather than as a silent non-zero exit"
    );
}

#[test]
fn only_episodes_claim_the_run_strip() {
    assert!(script_kind().is_episode());
    assert!(
        !RunKind::FetchVoices.is_episode(),
        "a two-second query must not raise the run strip"
    );
}

#[test]
fn every_flag_is_followed_by_its_value_and_never_by_another_flag() {
    // Catches a push_path/push_voices call that forgot its payload — the
    // failure mode is argparse consuming the next flag as the value, which
    // produces a confusing error minutes into a run rather than at spawn.
    let takes_value = ["--hosts", "--output", "--script-out", "--from-script"];
    for kind in [
        script_kind(),
        RunKind::Synth {
            script: "s.txt".into(),
            hosts: 2,
            voices: vec!["alice".into()],
            output: "o.wav".into(),
        },
        RunKind::OneShot {
            source: "src".into(),
            hosts: 4,
            voices: vec!["alice".into()],
            output: "o.wav".into(),
            script_out: "s.txt".into(),
        },
    ] {
        let argv = kind.argv();
        for (i, arg) in argv.iter().enumerate() {
            if takes_value.contains(&arg.as_str()) {
                let next = argv.get(i + 1);
                assert!(
                    next.is_some_and(|n| !n.starts_with("--")),
                    "{arg} in {argv:?} is not followed by a value"
                );
            }
        }
    }
}

#[test]
fn the_augmented_path_puts_local_bin_first_so_a_launcher_can_find_claude() {
    let path = augmented_path();
    let first = std::env::split_paths(&path).next().expect("PATH is not empty");
    if let Some(home) = dirs::home_dir() {
        assert_eq!(
            first,
            home.join(".local/bin"),
            "a GUI started from a desktop launcher inherits a thin PATH; \
             ~/.local/bin has to lead or the script stage cannot find claude"
        );
    }
}

#[test]
fn the_augmented_path_keeps_every_directory_it_inherited() {
    let before: Vec<_> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    let after: Vec<_> = std::env::split_paths(&augmented_path()).collect();
    for dir in before {
        assert!(after.contains(&dir), "{dir:?} was dropped from PATH");
    }
}
