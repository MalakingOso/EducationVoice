//! Tests for `run_state.rs`. These are where the run strip's honesty rules
//! live: a bar may only be drawn against a real denominator, and a completed
//! stage must not leave a truthful bar looking like a stalled one.

use super::*;
use crate::proto::Phase;

fn stage(stage: Stage, status: StageStatus) -> PyEvent {
    PyEvent::Stage {
        stage,
        status,
        detail: None,
        path: None,
        device: None,
        model: None,
        research_model: None,
    }
}

fn progress(step: u64, total: Option<u64>) -> PyEvent {
    PyEvent::Progress {
        stage: Stage::Tts,
        step,
        total,
    }
}

#[test]
fn script_generation_never_produces_a_drawable_fraction() {
    let mut s = RunState::default();
    s.begin(Instant::now(), &[Stage::Ingest, Stage::Script]);
    s.apply(&stage(Stage::Ingest, StageStatus::Start));
    s.apply(&stage(Stage::Ingest, StageStatus::Done));
    s.apply(&stage(Stage::Script, StageStatus::Start));
    for _ in 0..5 {
        s.apply(&PyEvent::Message {
            text: "brainstorming an angle".into(),
        });
    }

    assert_eq!(
        s.measured,
        Measured::Unmeasurable,
        "Claude's turn count is not a fraction of anything, so stage 1 must \
         show no track and no percentage — a bar here would be invented"
    );
}

#[test]
fn a_phase_event_moves_the_detail_line_and_nothing_measurable() {
    let mut s = RunState::default();
    s.begin(Instant::now(), &[Stage::Script]);
    s.apply(&stage(Stage::Script, StageStatus::Start));
    assert_eq!(s.detail, "writing the script");

    for (phase, label) in [
        (Phase::Researching, "researching the literature"),
        (Phase::Writing, "writing the script"),
        (Phase::Editing, "editing the script"),
    ] {
        s.apply(&PyEvent::Phase {
            stage: Stage::Script,
            phase,
        });
        assert_eq!(s.detail, label, "the strip must say which phase the script stage is in");
        assert_eq!(s.stage, Some(Stage::Script));
        assert_eq!(s.state, StageState::Running);
        assert_eq!(
            s.measured,
            Measured::Unmeasurable,
            "a phase is a landmark, never a fraction; the strip must not draw a bar from it"
        );
    }
}

#[test]
fn the_bar_appears_only_once_a_real_denominator_arrives() {
    let mut s = RunState::default();
    s.apply(&stage(Stage::Tts, StageStatus::Start));
    assert_eq!(
        s.measured,
        Measured::Unmeasurable,
        "model loading has no denominator; the bar must wait"
    );

    s.apply(&progress(0, Some(2166)));
    assert_eq!(
        s.measured,
        Measured::Fraction(0.0),
        "the first progress event is what makes TTS measurable, and its \
         arrival is itself the signal that generation actually started"
    );
}

#[test]
fn a_completed_tts_stage_leaves_its_partial_fraction_alone() {
    // The real numbers from a 4-line synthesis: max_steps was 2166 and the
    // loop broke out at 470 because the model had finished.
    //
    // This used to snap to full on Done, because a bar frozen at 22% read as
    // a stall. Nothing draws that bar now — max_steps is an upper bound, not
    // a prediction, and a display built on it was always going to be either
    // wrong or corrected. So the state keeps what the child actually said and
    // corrects nothing, which is the property worth pinning: the correction
    // must not creep back in alongside a new consumer of `measured`.
    let mut s = RunState::default();
    s.apply(&stage(Stage::Tts, StageStatus::Start));
    s.apply(&progress(470, Some(2166)));
    s.apply(&PyEvent::Stage {
        stage: Stage::Tts,
        status: StageStatus::Done,
        detail: None,
        path: Some("output/smoke.wav".into()),
        device: None,
        model: None,
        research_model: None,
    });

    assert_eq!(
        s.measured,
        Measured::Fraction(470.0 / 2166.0),
        "the run reported 470 of at most 2166 and then finished; inventing a \
         full bar out of that is the thing this display stopped doing"
    );
    assert_eq!(s.step, 470, "the step count is the child's, not the UI's");
}

#[test]
fn a_stage_that_never_measured_anything_stays_unmeasurable_when_it_completes() {
    // The degraded path: the tqdm shim could not attach after a vibevoice
    // upgrade, so no progress event ever arrived.
    let mut s = RunState::default();
    s.apply(&stage(Stage::Tts, StageStatus::Start));
    s.apply(&stage(Stage::Tts, StageStatus::Done));

    assert_eq!(
        s.measured,
        Measured::Unmeasurable,
        "completing must not conjure a bar that was never drawn — the \
         elapsed-only display is the honest degradation, and a sudden full \
         bar would claim a measurement nobody took"
    );
}

#[test]
fn starting_a_stage_clears_the_previous_stages_denominator() {
    let mut s = RunState::default();
    s.apply(&stage(Stage::Tts, StageStatus::Start));
    s.apply(&progress(400, Some(2166)));
    s.apply(&stage(Stage::Unknown, StageStatus::Start));

    assert_eq!(
        s.measured,
        Measured::Unmeasurable,
        "a new stage inheriting the last one's fraction would show a bar \
         already part-full before it had measured anything"
    );
    assert_eq!(s.total, None);
    assert_eq!(s.step, 0);
}

#[test]
fn a_gated_stage_two_lists_only_the_stage_it_will_actually_run() {
    // The default flow is two processes, not one paused one. Stage 2 is a
    // fresh RunState that never saw ingest or script, so listing them would
    // show two rows stuck on pending for the whole synthesis.
    let mut s = RunState::default();
    s.begin(Instant::now(), &[Stage::Tts]);
    s.apply(&stage(Stage::Tts, StageStatus::Start));

    assert_eq!(s.stages, vec![Stage::Tts]);
    assert_eq!(s.state_of(Stage::Tts), StageState::Running);
    assert_eq!(
        s.state_of(Stage::Script),
        StageState::Pending,
        "a stage outside this run's plan must never claim to have finished — \
         it ran in a different process, and this one has no way to know"
    );
}

#[test]
fn the_checklist_reads_forward_from_the_current_stage() {
    let mut s = RunState::default();
    s.begin(Instant::now(), &[Stage::Ingest, Stage::Script, Stage::Tts]);
    s.apply(&stage(Stage::Ingest, StageStatus::Start));
    s.apply(&stage(Stage::Ingest, StageStatus::Done));
    s.apply(&stage(Stage::Script, StageStatus::Start));

    assert_eq!(s.state_of(Stage::Ingest), StageState::Done);
    assert_eq!(s.state_of(Stage::Script), StageState::Running);
    assert_eq!(s.state_of(Stage::Tts), StageState::Pending);
}

#[test]
fn a_failed_stage_fails_its_own_row_and_not_the_ones_behind_it() {
    let mut s = RunState::default();
    s.begin(Instant::now(), &[Stage::Ingest, Stage::Script, Stage::Tts]);
    s.apply(&stage(Stage::Ingest, StageStatus::Done));
    s.apply(&stage(Stage::Script, StageStatus::Start));
    s.apply(&PyEvent::Error { text: "the writer returned nothing".into() });

    assert_eq!(s.state_of(Stage::Script), StageState::Failed);
    assert_eq!(
        s.state_of(Stage::Ingest),
        StageState::Done,
        "ingest really did finish; a later failure must not retract it"
    );
}

#[test]
fn the_reasoning_feed_never_shows_the_finished_script() {
    // article2pod.py selects the script as the last message containing
    // Speaker lines, which means the whole episode arrives through the same
    // channel as the plan. The run view is not where a script gets read.
    let mut s = RunState::default();
    s.begin(Instant::now(), &[Stage::Script]);

    s.apply(&PyEvent::Message {
        text: "Three threads worth pulling here. The reversal timing is the \
               one a listener can hold onto."
            .into(),
    });
    assert!(s.last_message.as_deref().unwrap().starts_with("Three threads"));

    s.apply(&PyEvent::Message {
        text: "Speaker 1: So the trial was stopped early.\n\
               Speaker 2: Which is where it gets interesting."
            .into(),
    });
    assert!(
        s.last_message.as_deref().unwrap().starts_with("Three threads"),
        "the draft arrived and was correctly ignored; the panel must still be \
         showing the reasoning that preceded it"
    );
}

#[test]
fn prose_that_merely_mentions_a_speaker_is_still_reasoning() {
    // The writer's plan and the editor's commentary both discuss the script
    // in prose. Only a line *formatted* as dialogue is the draft.
    let mut s = RunState::default();
    s.apply(&PyEvent::Message {
        text: "I'll give Speaker 2 the counterargument, and let Speaker 1 \
               concede it in the third act."
            .into(),
    });
    assert!(
        s.last_message.is_some(),
        "this is the editor thinking out loud, not a script; dropping it would \
         empty the panel during exactly the stage it exists for"
    );
}

#[test]
fn an_empty_assistant_turn_does_not_blank_the_panel() {
    let mut s = RunState::default();
    s.apply(&PyEvent::Message { text: "planning the shape".into() });
    s.apply(&PyEvent::Message { text: "   \n  ".into() });
    assert_eq!(
        s.last_message.as_deref(),
        Some("planning the shape"),
        "a whitespace-only turn is not a new thought and must not replace one"
    );
}

#[test]
fn the_device_is_recorded_so_a_gpu_toggle_mistake_is_visible() {
    let mut s = RunState::default();
    s.apply(&PyEvent::Stage {
        stage: Stage::Tts,
        status: StageStatus::Start,
        detail: None,
        path: None,
        device: Some("xpu".into()),
        model: None,
        research_model: None,
    });
    assert_eq!(s.device.as_deref(), Some("xpu"));
}

#[test]
fn the_script_path_is_captured_because_the_gate_opens_it() {
    let mut s = RunState::default();
    s.apply(&PyEvent::Stage {
        stage: Stage::Script,
        status: StageStatus::Done,
        detail: None,
        path: Some("output/ep.script.txt".into()),
        device: None,
        model: None,
        research_model: None,
    });
    assert_eq!(
        s.script_path,
        Some(PathBuf::from("output/ep.script.txt")),
        "without this the review gate has no file to load or write back"
    );
}

#[test]
fn an_error_event_fails_the_run_and_keeps_its_message() {
    let mut s = RunState::default();
    s.apply(&stage(Stage::Script, StageStatus::Start));
    s.apply(&PyEvent::Error {
        text: "script does not match --hosts 3".into(),
    });

    assert_eq!(s.state, StageState::Failed);
    assert_eq!(
        s.last_error.as_deref(),
        Some("script does not match --hosts 3"),
        "the gate shows this inline in the editor, so it has to survive"
    );
}

#[test]
fn warnings_accumulate_rather_than_replacing_one_another() {
    let mut s = RunState::default();
    s.apply(&PyEvent::Warning { text: "stripped 3 non-script lines".into() });
    s.apply(&PyEvent::Warning { text: "MP3 conversion failed".into() });
    assert_eq!(s.warnings.len(), 2, "each warning is about a different thing");
}

#[test]
fn a_cancelled_run_is_never_recorded_as_finished() {
    let mut s = RunState::default();
    s.begin(Instant::now(), &[Stage::Ingest, Stage::Script]);
    s.apply(&stage(Stage::Tts, StageStatus::Start));
    s.finish(&RunOutcome::Cancelled);

    assert_eq!(
        s.state,
        StageState::Failed,
        "a cancelled run produced no episode and must not display as done"
    );
    assert!(!s.is_running(), "the strip must stop claiming a live run");
}

#[test]
fn a_nonzero_exit_with_no_error_event_still_reports_something_useful() {
    // The traceback path: Python died before emitting an error event.
    let mut s = RunState::default();
    s.begin(Instant::now(), &[Stage::Ingest, Stage::Script]);
    s.finish(&RunOutcome::Failed { code: Some(1) });

    assert_eq!(s.state, StageState::Failed);
    assert!(
        s.last_error.is_some(),
        "a failure with no message leaves the user with a blank strip and no \
         idea what happened"
    );
}

#[test]
fn an_error_event_already_seen_is_not_overwritten_by_the_exit_status() {
    let mut s = RunState::default();
    s.apply(&PyEvent::Error { text: "unknown voice 'nope'".into() });
    s.finish(&RunOutcome::Failed { code: Some(1) });

    assert_eq!(
        s.last_error.as_deref(),
        Some("unknown voice 'nope'"),
        "the specific reason beats the generic exit status every time"
    );
}

#[test]
fn an_unknown_event_leaves_a_running_stage_undisturbed() {
    let mut s = RunState::default();
    s.apply(&stage(Stage::Tts, StageStatus::Start));
    s.apply(&progress(100, Some(1000)));
    let before = s.clone();
    s.apply(&PyEvent::Unknown);
    assert_eq!(s, before, "a newer Python must not perturb an older GUI");
}

#[test]
fn beginning_a_run_clears_the_previous_runs_wreckage() {
    let mut s = RunState::default();
    s.apply(&PyEvent::Error { text: "old failure".into() });
    s.apply(&PyEvent::Warning { text: "old warning".into() });
    s.begin(Instant::now(), &[Stage::Ingest, Stage::Script]);

    assert!(s.last_error.is_none(), "a fresh run must not show a stale error");
    assert!(s.warnings.is_empty());
    assert!(s.is_running());
}

#[test]
fn elapsed_reads_as_a_clock_and_grows_a_field_past_an_hour() {
    assert_eq!(format_elapsed(Duration::from_secs(0)), "0:00");
    assert_eq!(format_elapsed(Duration::from_secs(9)), "0:09");
    assert_eq!(format_elapsed(Duration::from_secs(252)), "4:12");
    assert_eq!(format_elapsed(Duration::from_secs(659)), "10:59");
    assert_eq!(
        format_elapsed(Duration::from_secs(3661)),
        "1:01:01",
        "an eleven-minute synthesis is normal, so the hour case is reachable"
    );
}

#[test]
fn an_idle_state_reports_no_elapsed_time_rather_than_a_garbage_one() {
    let s = RunState::default();
    assert_eq!(s.elapsed(Instant::now()), Duration::ZERO);
}

#[test]
fn a_full_successful_run_replayed_end_to_end_lands_in_the_right_place() {
    // The exact event order a real `--from-script` run produced.
    let mut s = RunState::default();
    s.begin(Instant::now(), &[Stage::Tts]);
    s.apply(&PyEvent::Stage {
        stage: Stage::Script,
        status: StageStatus::Done,
        detail: None,
        path: Some("/tmp/four_lines.txt".into()),
        device: None,
        model: None,
        research_model: None,
    });
    s.apply(&PyEvent::Stage {
        stage: Stage::Tts,
        status: StageStatus::Start,
        detail: None,
        path: None,
        device: Some("xpu".into()),
        model: None,
        research_model: None,
    });
    for step in (0..=430).step_by(10) {
        s.apply(&progress(step, Some(2166)));
    }
    s.apply(&PyEvent::Stage {
        stage: Stage::Tts,
        status: StageStatus::Done,
        detail: None,
        path: Some("output/smoke2.wav".into()),
        device: None,
        model: None,
        research_model: None,
    });
    s.apply(&PyEvent::Done { output: "output/smoke2.wav".into() });
    s.finish(&RunOutcome::Completed);

    assert_eq!(s.state, StageState::Done);
    assert_eq!(
        s.measured,
        Measured::Fraction(430.0 / 2166.0),
        "the last progress event the child sent, kept verbatim"
    );
    assert_eq!(s.output_path, Some(PathBuf::from("output/smoke2.wav")));
    assert_eq!(s.device.as_deref(), Some("xpu"));
    assert!(s.last_error.is_none());
}
