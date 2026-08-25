//! The UI's view of a run: one state machine, fed by protocol events.
//!
//! [`RunState::apply`] is deliberately pure — it takes an event and mutates
//! this struct, touching no signal, no window and no clock beyond the one it
//! was handed. Everything subtle about the run strip (when a bar may be drawn,
//! what "done" does to a bar that never filled) is therefore testable without
//! spawning a process or opening a window.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::proto::{Measured, PyEvent, Stage, StageStatus};
use crate::runner::RunOutcome;

/// How far one stage has got. An enum rather than a pair of bools because
/// "running" and "failed" are not independent, and a bool pair admits states
/// that cannot exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageState {
    Pending,
    Running,
    Done,
    Failed,
}

impl StageState {
    /// The class suffix the stylesheet expects on `.stage-dot`.
    pub fn dot_class(self) -> &'static str {
        match self {
            StageState::Pending => "pending",
            StageState::Running => "running",
            StageState::Done => "done",
            StageState::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunState {
    /// `None` means idle, and the run strip is hidden exactly then.
    pub stage: Option<Stage>,
    pub state: StageState,
    /// The line of text the strip shows beside the stage name.
    pub detail: String,
    /// Whether a bar may be drawn at all. Never assembled by hand — only
    /// `apply` sets it, and only from an event carrying a real denominator.
    pub measured: Measured,
    pub step: u64,
    pub total: Option<u64>,
    pub started: Option<Instant>,
    /// Reported by the tts stage. Displayed so a GPU-toggle mistake is visible
    /// rather than inferred.
    pub device: Option<String>,
    pub model: Option<String>,
    /// The article's title, as the ingest stage reported it. Carried into the
    /// sidecar so the Library can name the row after the article rather than
    /// after the filename it was collapsed into.
    pub title: Option<String>,
    /// Where the script landed, which is what the gate opens.
    pub script_path: Option<PathBuf>,
    pub output_path: Option<PathBuf>,
    pub last_error: Option<String>,
    pub warnings: Vec<String>,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            stage: None,
            state: StageState::Pending,
            detail: String::new(),
            measured: Measured::Unmeasurable,
            step: 0,
            total: None,
            started: None,
            device: None,
            model: None,
            title: None,
            script_path: None,
            output_path: None,
            last_error: None,
            warnings: Vec::new(),
        }
    }
}

impl RunState {
    /// Reset for a new run, starting the clock.
    pub fn begin(&mut self, now: Instant) {
        *self = Self {
            started: Some(now),
            state: StageState::Running,
            detail: "starting".to_string(),
            ..Self::default()
        };
    }

    pub fn is_running(&self) -> bool {
        self.started.is_some()
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        self.started
            .map(|s| now.saturating_duration_since(s))
            .unwrap_or_default()
    }

    /// Fold one protocol event into the state.
    pub fn apply(&mut self, event: &PyEvent) {
        match event {
            PyEvent::Stage {
                stage,
                status,
                detail,
                path,
                device,
                model,
            } => self.apply_stage(*stage, *status, detail, path, device, model),

            PyEvent::Progress { stage, step, total } => {
                self.stage = Some(*stage);
                self.state = StageState::Running;
                self.step = *step;
                self.total = *total;
                self.measured = Measured::from_step(*step, *total);
            }

            // Claude's turns arrive here. They are shown in the log pane, not
            // the strip: there is no honest fraction to derive from a count of
            // messages, and the strip must not imply one.
            PyEvent::Message { .. } => {}

            // Recorded, not displayed. The strip says what is happening; the
            // title says what it is happening to, which is the Library's
            // question rather than the strip's.
            PyEvent::Title { text } => {
                let text = text.trim();
                self.title = (!text.is_empty()).then(|| text.to_string());
            }

            PyEvent::Warning { text } => self.warnings.push(text.clone()),

            PyEvent::Error { text } => {
                self.state = StageState::Failed;
                self.last_error = Some(text.clone());
            }

            PyEvent::Done { output } => {
                self.output_path = Some(PathBuf::from(output));
                self.state = StageState::Done;
            }

            // Parsed but unrecognised. Ignored on purpose: an event kind added
            // on the Python side must not disturb a run in progress.
            PyEvent::Unknown => {}
        }
    }

    fn apply_stage(
        &mut self,
        stage: Stage,
        status: StageStatus,
        detail: &Option<String>,
        path: &Option<String>,
        device: &Option<String>,
        model: &Option<String>,
    ) {
        self.stage = Some(stage);
        if let Some(d) = device {
            self.device = Some(d.clone());
        }
        if let Some(m) = model {
            self.model = Some(m.clone());
        }

        match status {
            StageStatus::Start => {
                self.state = StageState::Running;
                // A new stage has no denominator until its first progress
                // event. Clearing this is what stops the TTS bar inheriting a
                // stale fraction from a previous stage.
                self.measured = Measured::Unmeasurable;
                self.step = 0;
                self.total = None;
                self.detail = match stage {
                    Stage::Ingest => "reading the article".to_string(),
                    Stage::Script => "writing the script".to_string(),
                    Stage::Tts => "loading the model".to_string(),
                    Stage::Unknown => "working".to_string(),
                };
            }
            StageStatus::Done => {
                self.state = StageState::Done;
                if let Some(d) = detail {
                    self.detail = d.clone();
                }
                match stage {
                    Stage::Script => self.script_path = path.clone().map(PathBuf::from),
                    Stage::Tts => {
                        self.output_path = path.clone().map(PathBuf::from);
                        // Snap the bar to full.
                        //
                        // `max_steps` is an upper bound, not a prediction: the
                        // generation loop breaks out early on finished_tags,
                        // and a measured 4-line run ended at step 470 of 2166.
                        // Left alone, a perfectly successful episode would
                        // finish with the bar stuck at 22%, which reads as a
                        // failure. Only a completed stage may do this.
                        if matches!(self.measured, Measured::Fraction(_)) {
                            self.measured = Measured::Fraction(1.0);
                            if let Some(t) = self.total {
                                self.step = t;
                            }
                        }
                    }
                    Stage::Ingest | Stage::Unknown => {}
                }
            }
            StageStatus::Unknown => {}
        }
    }

    /// Close the run out. Never invents success: a process that exited without
    /// a `done` event failed, whatever its last stage said.
    pub fn finish(&mut self, outcome: &RunOutcome) {
        match outcome {
            RunOutcome::Completed => self.state = StageState::Done,
            RunOutcome::Cancelled => {
                self.state = StageState::Failed;
                self.detail = "cancelled".to_string();
            }
            RunOutcome::Failed { code } => {
                self.state = StageState::Failed;
                if self.last_error.is_none() {
                    self.last_error = Some(match code {
                        Some(c) => format!("the pipeline exited with status {c}"),
                        None => "the pipeline exited abnormally".to_string(),
                    });
                }
                self.detail = "failed".to_string();
            }
        }
        self.started = None;
    }
}

/// `m:ss`, or `h:mm:ss` once a run passes an hour.
///
/// Tabular figures in the stylesheet keep this from jittering as the digits
/// change, which is the whole reason it is a fixed shape.
pub fn format_elapsed(d: Duration) -> String {
    let total = d.as_secs();
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
#[path = "run_state_tests.rs"]
mod tests;
