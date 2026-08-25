//! The `--progress-json` event grammar, mirrored from `article2pod.py`.
//!
//! The child writes one JSON object per line to stdout and nothing else; every
//! human-readable message goes to stderr. This module owns that grammar and
//! only that — how the process is spawned, and how stderr and exit status are
//! folded in alongside, belongs to `runner.rs`.

use serde::Deserialize;

/// Which of the pipeline's stages an event belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Ingest,
    Script,
    Tts,
    /// A stage name this build does not know.
    ///
    /// Kept rather than rejected: a stage added on the Python side should cost
    /// a precise label, never a dropped line. Every match on `Stage` handles
    /// this arm explicitly — there are no `_ =>` catch-alls, so adding a real
    /// stage here is a compile error at each site that has to care.
    #[serde(other)]
    Unknown,
}

impl Stage {
    /// Short label for the run strip. "synth" rather than "tts" because the
    /// strip is read at a glance and the acronym is jargon.
    pub fn label(self) -> &'static str {
        match self {
            Stage::Ingest => "ingest",
            Stage::Script => "script",
            Stage::Tts => "synth",
            Stage::Unknown => "working",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StageStatus {
    Start,
    Done,
    #[serde(other)]
    Unknown,
}

/// One line of the child's stdout.
///
/// Optional fields are `Option` rather than defaulted strings because "the
/// child did not say" and "the child said empty" are different facts — the
/// run strip shows a device only when one was actually reported.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum PyEvent {
    Stage {
        stage: Stage,
        status: StageStatus,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        device: Option<String>,
        #[serde(default)]
        model: Option<String>,
    },
    /// One assistant turn, emitted live during script generation.
    Message {
        text: String,
    },
    /// The article's title, emitted during ingest so it is available even on
    /// a `--script-only` run — which is stage 1 of the gated flow, and the
    /// point at which the Library first has a row to name.
    Title {
        text: String,
    },
    Progress {
        stage: Stage,
        step: u64,
        /// Absent when the shim could not determine a denominator. An event
        /// with no total is explicitly not a fraction — see [`Measured`].
        #[serde(default)]
        total: Option<u64>,
    },
    Warning {
        text: String,
    },
    Error {
        text: String,
    },
    Done {
        output: String,
    },
    /// An event kind this build does not know. Same reasoning as
    /// [`Stage::Unknown`]: parse it and ignore it rather than dropping the line.
    #[serde(other)]
    Unknown,
}

/// Whether a stage can honestly draw a progress bar.
///
/// Script generation has no denominator — Claude's turn count is not a
/// fraction of anything — so the strip must show a pulsing dot and an elapsed
/// clock with no track and no percentage. Encoding that as a type rather than
/// a convention is what keeps a later edit from quietly rendering a bar stuck
/// at 0%: there is no way to obtain a `Fraction` without a real total.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Measured {
    Fraction(f32),
    Unmeasurable,
}

impl Measured {
    /// Build a fraction from a progress event, or `Unmeasurable` when the
    /// child reported no usable denominator.
    ///
    /// A zero total counts as unusable rather than as a divide-by-zero: the
    /// shim reports `max_steps`, and a degenerate run reporting 0 should
    /// flatten the bar, not produce NaN and paint an empty track forever.
    pub fn from_step(step: u64, total: Option<u64>) -> Self {
        match total {
            Some(t) if t > 0 => {
                Measured::Fraction((step as f32 / t as f32).clamp(0.0, 1.0))
            }
            Some(_) | None => Measured::Unmeasurable,
        }
    }
}

/// Parse one line of the child's stdout.
///
/// Returns `Err` for anything that is not a JSON object matching the grammar,
/// including a line truncated by a killed process. Callers log and skip:
/// a malformed line is never fatal to a run that is otherwise progressing.
pub fn parse_line(line: &str) -> anyhow::Result<PyEvent> {
    Ok(serde_json::from_str(line)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines captured verbatim from a real 4-line synthesis run
    /// (`--from-script … --progress-json`). Fixtures copied from the machine
    /// rather than hand-written, so a Python-side change to the wire format
    /// shows up here as a failing test instead of as a GUI that silently
    /// stops updating.
    const REAL_RUN: &[&str] = &[
        r#"{"event": "stage", "stage": "script", "status": "done", "path": "/tmp/four_lines.txt"}"#,
        r#"{"event": "stage", "stage": "tts", "status": "start", "device": "xpu"}"#,
        r#"{"event": "progress", "stage": "tts", "step": 0, "total": 2166}"#,
        r#"{"event": "progress", "stage": "tts", "step": 470, "total": 2166}"#,
        r#"{"event": "stage", "stage": "tts", "status": "done", "path": "output/smoke.wav"}"#,
        r#"{"event": "done", "output": "output/smoke.wav"}"#,
    ];

    #[test]
    fn every_line_of_a_real_run_parses() {
        for line in REAL_RUN {
            assert!(
                parse_line(line).is_ok(),
                "a line this program actually produced must parse: {line}"
            );
        }
    }

    #[test]
    fn a_stage_event_parses_when_every_optional_field_is_absent() {
        let got = parse_line(r#"{"event":"stage","stage":"ingest","status":"start"}"#).unwrap();
        assert_eq!(
            got,
            PyEvent::Stage {
                stage: Stage::Ingest,
                status: StageStatus::Start,
                detail: None,
                path: None,
                device: None,
                model: None,
            },
            "the ingest start event carries no extra fields and must not need any"
        );
    }

    #[test]
    fn a_stage_event_carries_the_device_when_the_child_reports_one() {
        let got = parse_line(REAL_RUN[1]).unwrap();
        match got {
            PyEvent::Stage { device, stage, .. } => {
                assert_eq!(stage, Stage::Tts);
                assert_eq!(
                    device.as_deref(),
                    Some("xpu"),
                    "the GPU toggle is verified by reading this field back"
                );
            }
            other => panic!("expected a stage event, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_event_kind_parses_as_unknown_rather_than_failing() {
        let got = parse_line(r#"{"event":"telemetry","frobs":3}"#).unwrap();
        assert_eq!(
            got,
            PyEvent::Unknown,
            "an event kind added on the Python side must not break an older GUI"
        );
    }

    #[test]
    fn an_unknown_stage_name_parses_as_unknown_rather_than_failing() {
        let got = parse_line(r#"{"event":"stage","stage":"mastering","status":"start"}"#).unwrap();
        match got {
            PyEvent::Stage { stage, .. } => assert_eq!(stage, Stage::Unknown),
            other => panic!("expected a stage event, got {other:?}"),
        }
    }

    #[test]
    fn a_truncated_line_is_an_error_not_a_panic() {
        // What a killed process leaves in the pipe: a line cut mid-write.
        let got = parse_line(r#"{"event": "progress", "stage": "tts", "st"#);
        assert!(
            got.is_err(),
            "a half-written line must be reported so the runner can skip it"
        );
    }

    #[test]
    fn an_empty_line_is_an_error_not_a_panic() {
        assert!(parse_line("").is_err(), "blank lines occur and must not parse");
    }

    #[test]
    fn progress_without_a_total_is_unmeasurable() {
        // The shim reports no total when it cannot find a denominator; that
        // must reach the strip as "draw no bar", never as 0%.
        assert_eq!(Measured::from_step(120, None), Measured::Unmeasurable);
    }

    #[test]
    fn progress_with_a_zero_total_is_unmeasurable_rather_than_nan() {
        assert_eq!(
            Measured::from_step(0, Some(0)),
            Measured::Unmeasurable,
            "dividing by a zero total would paint NaN width into the DOM"
        );
    }

    #[test]
    fn a_real_progress_pair_becomes_the_fraction_it_looks_like() {
        assert_eq!(Measured::from_step(470, Some(2166)), Measured::Fraction(470.0 / 2166.0));
    }

    #[test]
    fn a_fraction_is_clamped_when_the_child_overruns_its_own_total() {
        assert_eq!(
            Measured::from_step(9_000, Some(2_166)),
            Measured::Fraction(1.0),
            "max_steps is an upper bound the child may revise; the bar must not exceed the track"
        );
    }

    #[test]
    fn a_title_event_names_the_row_the_library_will_show() {
        let got = parse_line(
            r#"{"event":"title","text":"Reversal of Thromboprophylaxis in Bariatric Surgery"}"#,
        )
        .unwrap();
        assert_eq!(
            got,
            PyEvent::Title {
                text: "Reversal of Thromboprophylaxis in Bariatric Surgery".to_string()
            }
        );
    }

    #[test]
    fn a_message_event_keeps_its_text_intact() {
        let got = parse_line(r#"{"event":"message","text":"line one\nline two"}"#).unwrap();
        assert_eq!(
            got,
            PyEvent::Message { text: "line one\nline two".to_string() },
            "assistant turns are multi-line and must survive the wire"
        );
    }
}
