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

#[derive(Debug, Clone, PartialEq)]
pub struct RunState {
    /// `None` means idle, and the run strip is hidden exactly then.
    pub stage: Option<Stage>,
    pub state: StageState,
    /// The stages this run will perform, in order, from `RunKind::stages`.
    ///
    /// Seeded once by `begin` and never touched again. The run view draws one
    /// row per entry, so a gated stage 2 lists synth alone rather than three
    /// rows of which two are permanently pending.
    pub stages: Vec<Stage>,
    /// The most recent assistant turn that was reasoning rather than script.
    ///
    /// Filtered on the way in by [`is_script`], so the finished episode never
    /// reaches the run view: `article2pod.py` selects the script as the last
    /// message containing `Speaker N:` lines, which means the script arrives
    /// through this same channel as the plan and the editor's commentary.
    pub last_message: Option<String>,
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
    /// The researcher sub-agent's model, recorded alongside the writer's.
    pub research_model: Option<String>,
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
            stages: Vec::new(),
            last_message: None,
            detail: String::new(),
            measured: Measured::Unmeasurable,
            step: 0,
            total: None,
            started: None,
            device: None,
            model: None,
            research_model: None,
            title: None,
            script_path: None,
            output_path: None,
            last_error: None,
            warnings: Vec::new(),
        }
    }
}

impl RunState {
    /// Reset for a new run, starting the clock and laying out its stages.
    ///
    /// `stages` comes from `RunKind::stages`. It is the one thing about a run
    /// known before a single event arrives, and the only way this struct can
    /// tell a two-stage gated script run from a one-stage synthesis.
    pub fn begin(&mut self, now: Instant, stages: &[Stage]) {
        *self = Self {
            started: Some(now),
            state: StageState::Running,
            stages: stages.to_vec(),
            detail: "starting".to_string(),
            ..Self::default()
        };
    }

    pub fn is_running(&self) -> bool {
        self.started.is_some()
    }

    /// How one row of the checklist should draw.
    ///
    /// Derived from position in the plan rather than tracked per stage: the
    /// pipeline is strictly sequential, so everything before the current stage
    /// has finished and everything after it has not. Storing a state per row
    /// would admit combinations the pipeline cannot produce.
    ///
    /// A stage the plan does not contain reads as pending. That covers
    /// `Stage::Unknown` from a newer Python without it claiming to have run.
    pub fn state_of(&self, stage: Stage) -> StageState {
        let Some(row) = self.stages.iter().position(|s| *s == stage) else {
            return StageState::Pending;
        };
        let Some(current) = self.stage.and_then(|c| self.stages.iter().position(|s| *s == c))
        else {
            return StageState::Pending;
        };
        match row.cmp(&current) {
            std::cmp::Ordering::Less => StageState::Done,
            std::cmp::Ordering::Equal => self.state,
            std::cmp::Ordering::Greater => StageState::Pending,
        }
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
                research_model,
            } => self.apply_stage(*stage, *status, detail, path, device, model, research_model),

            PyEvent::Progress { stage, step, total } => {
                self.stage = Some(*stage);
                self.state = StageState::Running;
                self.step = *step;
                self.total = *total;
                self.measured = Measured::from_step(*step, *total);
            }

            // Claude's turns arrive here, and the run view types out the
            // latest one. Anything carrying dialogue is dropped rather than
            // shown: that message *is* the finished episode, and the run view
            // is not where a script gets read.
            PyEvent::Message { text } => {
                if !is_script(text) {
                    let text = text.trim();
                    if !text.is_empty() {
                        self.last_message = Some(text.to_string());
                    }
                }
            }

            // A landmark inside the script stage. It moves the detail line
            // and nothing else: the phase is read off the shape of the SDK
            // stream, so it says what is happening, never how far along.
            PyEvent::Phase { stage, phase } => {
                self.stage = Some(*stage);
                self.state = StageState::Running;
                self.detail = phase.label().to_string();
            }

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
        research_model: &Option<String>,
    ) {
        self.stage = Some(stage);
        if let Some(d) = device {
            self.device = Some(d.clone());
        }
        if let Some(m) = model {
            self.model = Some(m.clone());
        }
        if let Some(m) = research_model {
            self.research_model = Some(m.clone());
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
                    // No bar to snap to full any more. `max_steps` is an upper
                    // bound the generation loop breaks out of early — a
                    // measured 4-line run ended at step 470 of 2166 — so
                    // nothing draws that fraction and nothing has to correct
                    // it at the end. `measured` is still folded in above,
                    // because parsing the denominator honestly is worth
                    // keeping whether or not anything renders it.
                    Stage::Tts => self.output_path = path.clone().map(PathBuf::from),
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

/// Whether an assistant turn is the script rather than reasoning about it.
///
/// This is `article2pod.py`'s own `^Speaker \d+:` test, inverted. The Python
/// side uses it to *select* the script out of the message stream; the run view
/// uses it to reject the same message, so the two cannot disagree about which
/// turn is the episode. Hand-rolled rather than pulled in as a regex
/// dependency: one anchored pattern does not justify the crate.
///
/// One line is enough to disqualify a message. The writer's plan discusses the
/// script without ever formatting a line as dialogue, and the editor's
/// commentary does the same, so a single `Speaker N:` at the start of a line
/// means the draft has arrived.
fn is_script(text: &str) -> bool {
    text.lines().any(|line| {
        let Some(rest) = line.strip_prefix("Speaker ") else {
            return false;
        };
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        !digits.is_empty() && rest[digits.len()..].starts_with(':')
    })
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
