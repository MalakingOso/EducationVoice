//! The preset voice roster, read from the CLI rather than restated here.
//!
//! `article2pod.py --list-voices` prints `PRESET_VOICES` and `DEFAULT_ROSTER`
//! as JSON precisely so this side does not keep a second copy that can drift
//! from the one the synthesis actually uses.
//!
//! This is a *query*, not a run: it answers in well under a second and its
//! stdout is one JSON document rather than a stream of events. It therefore
//! does not go through `runner::spawn`, and `RunKind` deliberately has no
//! variant for it — feeding a pretty-printed roster to the line-by-line event
//! parser produces a screenful of "unparseable line" warnings and no roster.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::paths::Paths;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Voice {
    pub path: String,
    pub gender: String,
    /// Reflects the upstream locale prefix on the clip's filename, which is
    /// the only accent information the clips carry.
    pub accent: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Roster {
    pub voices: BTreeMap<String, Voice>,
    /// Host count to voice names in speaker order. The keys are strings
    /// because JSON object keys always are, even though Python keys this on
    /// an int.
    pub default_roster: BTreeMap<String, Vec<String>>,
}

impl Roster {
    /// The default voices for `hosts` speakers, in the order they bind to
    /// Speaker 1..N. Empty when the CLI offers no roster for that count,
    /// which the argv builder then turns into an omitted `--voices`.
    pub fn default_for(&self, hosts: u8) -> Vec<String> {
        self.default_roster
            .get(&hosts.to_string())
            .cloned()
            .unwrap_or_default()
    }

    /// Every preset name, alphabetically — the order the picker offers them in.
    pub fn names(&self) -> Vec<&str> {
        self.voices.keys().map(String::as_str).collect()
    }

    /// The host counts the CLI has a default roster for, ascending.
    pub fn host_choices(&self) -> Vec<u8> {
        let mut counts: Vec<u8> = self
            .default_roster
            .keys()
            .filter_map(|k| k.parse().ok())
            .collect();
        counts.sort_unstable();
        counts
    }
}

/// Ask the CLI for its roster.
pub async fn load(paths: &Paths) -> Result<Roster> {
    let out = tokio::process::Command::new(&paths.python)
        .arg(&paths.script)
        .arg("--list-voices")
        .current_dir(&paths.root)
        .output()
        .await
        .with_context(|| format!("could not run {}", paths.python.display()))?;

    if !out.status.success() {
        bail!(
            "--list-voices failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    serde_json::from_slice(&out.stdout).context("could not parse the voice roster")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from `article2pod.py --list-voices`, trimmed to two voices.
    const SAMPLE: &str = r#"{
      "voices": {
        "alice":  {"path": "voices/en-Alice_woman.wav", "gender": "female", "accent": "English"},
        "samuel": {"path": "voices/in-Samuel_man.wav",  "gender": "male",   "accent": "Indian"}
      },
      "default_roster": {"2": ["alice", "carter"], "3": ["alice", "carter", "maya"]}
    }"#;

    fn sample() -> Roster {
        serde_json::from_str(SAMPLE).expect("the shape --list-voices actually prints")
    }

    #[test]
    fn the_roster_parses_the_shape_the_cli_prints() {
        let r = sample();
        assert_eq!(r.voices["samuel"].gender, "male");
        assert_eq!(r.voices["samuel"].accent, "Indian");
        assert_eq!(r.voices["alice"].path, "voices/en-Alice_woman.wav");
    }

    #[test]
    fn host_counts_arrive_as_strings_and_are_looked_up_as_numbers() {
        // The bug this guards: keying the map on u8 fails to deserialize, and
        // keying it on String but querying with a number silently finds nothing.
        assert_eq!(sample().default_for(2), vec!["alice", "carter"]);
    }

    #[test]
    fn an_unknown_host_count_yields_no_voices_rather_than_a_panic() {
        assert!(
            sample().default_for(9).is_empty(),
            "an absent roster must omit --voices, letting the CLI decide"
        );
    }

    #[test]
    fn host_choices_are_offered_in_ascending_order() {
        assert_eq!(sample().host_choices(), vec![2, 3]);
    }

    #[test]
    fn names_are_alphabetical_so_the_picker_order_is_stable() {
        assert_eq!(sample().names(), vec!["alice", "samuel"]);
    }
}
