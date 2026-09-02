//! Persisted GUI settings, one nested struct per settings card.
//!
//! Serialized as TOML to `~/.config/article2pod/config.toml`. Every field
//! carries a serde default so a file written by an older build — or a newer
//! one that grew keys this build has never heard of — still loads. The only
//! thing that can reject a file outright is TOML that does not parse, and
//! that path preserves the file rather than clobbering it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Top-level application configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub run: RunSettings,
    #[serde(default)]
    pub voices: VoiceSettings,
    #[serde(default)]
    pub device: DeviceSettings,
    #[serde(default)]
    pub spotify: SpotifySettings,
}

/// What the next run asks of `article2pod.py`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSettings {
    /// Speakers in the episode. The CLI's `--hosts` is a three-way choice,
    /// so `sanitise` clamps anything else — see `HOST_CHOICES`.
    ///
    /// Typed `u8` because the rest of the app is: a hand-edited value above
    /// 255 fails deserialization outright and takes the whole file down the
    /// corrupt path instead of being clamped.
    #[serde(default = "default_hosts")]
    pub hosts: u8,
    /// Skip the script-review gate and go straight to synthesis. Off by
    /// default because the review gate is the only place a bad script can be
    /// caught before it costs a full TTS pass.
    #[serde(default)]
    pub auto_continue: bool,
    /// `--model`: the writer pass that researches and drafts. Defaults to the
    /// CLI's own `SCRIPT_MODEL`, unvalidated the same way `device.gpu_mask`
    /// is — the CLI accepts any model string and passes it straight through.
    #[serde(default = "default_write_model")]
    pub write_model: String,
    /// `--edit-model`: the closed-book pass that edits the draft for
    /// AI-sounding tells. Defaults to the CLI's own `EDIT_MODEL`.
    #[serde(default = "default_edit_model")]
    pub edit_model: String,
    /// `--research-model`: the sub-agent the writer sends into the literature.
    /// Defaults to the CLI's own `RESEARCH_MODEL`.
    #[serde(default = "default_research_model")]
    pub research_model: String,
}

/// The voice picker's state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceSettings {
    /// Voice names in speaker order. Empty defers to the CLI's own roster,
    /// which is the source of truth for what a preset name means.
    #[serde(default)]
    pub selected: Vec<String>,
}

/// Which GPU the Python half runs on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceSettings {
    /// `ZE_AFFINITY_MASK` for the child process. Empty leaves the variable
    /// unset so Python picks; "1" pins the B60.
    #[serde(default)]
    pub gpu_mask: String,
}

/// The Spotify "Send" button's state. There is only one show in this
/// version, so a cached URI is all there is to remember.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpotifySettings {
    /// The `article2pod` show, once created. `None` until the first send —
    /// `spotify::ensure_show` looks it up by title and fills this in rather
    /// than creating a second show on every launch.
    #[serde(default)]
    pub show_uri: Option<String>,
}

/// The host counts `article2pod.py --hosts` accepts.
const HOST_CHOICES: [u8; 3] = [2, 3, 4];

fn default_hosts() -> u8 { 2 }

/// Mirrors `article2pod.py`'s `SCRIPT_MODEL`.
fn default_write_model() -> String { "claude-sonnet-5".to_string() }

/// Mirrors `article2pod.py`'s `EDIT_MODEL`.
fn default_edit_model() -> String { "claude-opus-5".to_string() }

/// Mirrors `article2pod.py`'s `RESEARCH_MODEL`.
fn default_research_model() -> String { "claude-sonnet-5".to_string() }

impl Default for Config {
    fn default() -> Self {
        Self {
            run: RunSettings::default(),
            voices: VoiceSettings::default(),
            device: DeviceSettings::default(),
            spotify: SpotifySettings::default(),
        }
    }
}

impl Default for RunSettings {
    fn default() -> Self {
        Self {
            hosts: default_hosts(),
            auto_continue: false,
            write_model: default_write_model(),
            edit_model: default_edit_model(),
            research_model: default_research_model(),
        }
    }
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self { selected: Vec::new() }
    }
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self { gpu_mask: String::new() }
    }
}

impl Default for SpotifySettings {
    fn default() -> Self {
        Self { show_uri: None }
    }
}

impl Config {
    pub fn config_dir() -> PathBuf {
        let base = dirs::config_dir().expect("Could not determine config directory");
        base.join("article2pod")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    pub fn load() -> Result<Config> {
        load_from(&Self::config_path())
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::config_path())
    }

    /// Write to a temp file beside the target and rename over it, so a crash
    /// or a full disk mid-write leaves the previous config intact instead of
    /// a truncated one that would then take the corrupt path on next launch.
    fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("could not create {}", dir.display()))?;
        }
        let contents = toml::to_string_pretty(self)?;

        // The temp file must share a directory with the target: `rename` is
        // only atomic within one filesystem, and /tmp is often a separate one.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, contents)
            .with_context(|| format!("could not write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("could not replace {}", path.display()))?;
        Ok(())
    }
}

/// Read and sanitise the config at `path`, falling back to defaults for both
/// "never written yet" and "written by something that mangled it".
fn load_from(path: &Path) -> Result<Config> {
    if !path.exists() {
        // No write here: first launch on a read-only or not-yet-created
        // config dir must still start, and the first `save_config` creates
        // the file anyway.
        return Ok(Config::default());
    }

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;

    let mut config: Config = match toml::from_str(&contents) {
        Ok(config) => config,
        Err(parse_error) => {
            let preserved = path.with_extension("toml.corrupt");
            match std::fs::rename(path, &preserved) {
                Ok(()) => tracing::warn!(
                    "{} did not parse ({parse_error}); kept it as {} and started from defaults",
                    path.display(),
                    preserved.display()
                ),
                Err(rename_error) => tracing::warn!(
                    "{} did not parse ({parse_error}) and could not be preserved \
                     ({rename_error}); starting from defaults",
                    path.display()
                ),
            }
            return Ok(Config::default());
        }
    };

    if sanitise(&mut config) {
        // Persist the correction so the same bad value is not re-read, and
        // re-corrected, on every launch. Best-effort: a failure here costs
        // nothing the in-memory config does not already have.
        let _ = config.save_to(path);
    }

    Ok(config)
}

/// Bring a parsed config back into the range the CLI accepts.
///
/// Returns `true` when something changed, which is the caller's signal to
/// write the file back.
fn sanitise(config: &mut Config) -> bool {
    let mut dirty = false;

    // `--hosts` is `choices=[2, 3, 4]` in argparse, so a hand-edited 7 does
    // not degrade — it makes every single run die before it starts.
    if !HOST_CHOICES.contains(&config.run.hosts) {
        config.run.hosts = default_hosts();
        dirty = true;
    }

    // Python's `resolve_voices` warns and truncates a long list anyway, but
    // it truncates after the fact: a list left over from a 4-host run rebinds
    // which voice speaks which part in a 2-host run without saying so.
    // Clamped hosts first, so `hosts = 9` with nine voices lands at two.
    if config.voices.selected.len() > config.run.hosts as usize {
        config.voices.selected.truncate(config.run.hosts as usize);
        dirty = true;
    }

    dirty
}

/// Mutate and persist in one step. Synchronous on purpose.
///
/// A settings toggle is the last thing a user touches before quitting, and an
/// async write loses that race — the window is gone before the task runs.
pub fn save_config(config: &mut Signal<Config>, f: impl FnOnce(&mut Config)) {
    // One write guard, cloned before it drops. Taking a second borrow of the
    // same signal to read back what was just written is how Dioxus borrow
    // panics happen.
    let snapshot = {
        let mut guard = config.write();
        f(&mut guard);
        guard.clone()
    };

    if let Err(error) = snapshot.save() {
        tracing::warn!("could not save {}: {error:#}", Config::config_path().display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own. Parallel tests sharing one
    /// `config.toml` would see each other's writes.
    fn temp_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("a2p-cfg-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_missing_file_yields_defaults_rather_than_an_error() {
        let path = temp_dir("missing").join("config.toml");
        let config = load_from(&path).expect("a first launch must never fail on the config");
        assert_eq!(config, Config::default());
        assert!(
            !path.exists(),
            "loading must not create the file; a read-only config dir must still start the app"
        );
    }

    #[test]
    fn the_documented_defaults_are_what_default_actually_produces() {
        let config = Config::default();
        assert_eq!(config.run.hosts, 2);
        assert!(!config.run.auto_continue, "the script-review gate is on until asked otherwise");
        assert_eq!(config.run.write_model, "claude-sonnet-5", "matches the CLI's own SCRIPT_MODEL");
        assert_eq!(config.run.edit_model, "claude-opus-5", "matches the CLI's own EDIT_MODEL");
        assert_eq!(config.run.research_model, "claude-sonnet-5", "matches the CLI's own RESEARCH_MODEL");
        assert!(config.voices.selected.is_empty(), "an empty list defers to the CLI's roster");
        assert_eq!(config.device.gpu_mask, "", "an empty mask leaves ZE_AFFINITY_MASK unset");
        assert!(config.spotify.show_uri.is_none(), "no show exists until the first send");
    }

    #[test]
    fn a_saved_config_reloads_field_for_field() {
        let path = temp_dir("roundtrip").join("config.toml");
        let written = Config {
            run: RunSettings {
                hosts: 3,
                auto_continue: true,
                write_model: "claude-fable-5-1".into(),
                edit_model: "claude-fable-5-1".into(),
                research_model: "claude-opus-5".into(),
            },
            voices: VoiceSettings {
                selected: vec!["alice".into(), "carter".into(), "maya".into()],
            },
            device: DeviceSettings { gpu_mask: "1".into() },
            spotify: SpotifySettings { show_uri: Some("spotify:show:abc123".into()) },
        };
        written.save_to(&path).expect("save");

        assert_eq!(
            load_from(&path).expect("load"),
            written,
            "every settings card must survive a quit and relaunch unchanged"
        );
    }

    #[test]
    fn a_spotify_show_uri_written_by_hand_loads() {
        let path = temp_dir("spotify").join("config.toml");
        std::fs::write(&path, "[spotify]\nshow_uri = \"spotify:show:abc123\"\n").expect("write");

        let config = load_from(&path).expect("load");
        assert_eq!(config.spotify.show_uri.as_deref(), Some("spotify:show:abc123"));
    }

    #[test]
    fn saving_creates_the_config_directory_when_it_is_absent() {
        let path = temp_dir("mkdir").join("nested").join("deeper").join("config.toml");
        Config::default().save_to(&path).expect("save must create its own parent directories");
        assert!(path.is_file());
    }

    #[test]
    fn saving_leaves_no_temp_file_behind() {
        let dir = temp_dir("atomic");
        let path = dir.join("config.toml");
        Config::default().save_to(&path).expect("save");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("readdir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "config.toml")
            .collect();
        assert!(
            leftovers.is_empty(),
            "the temp file must be renamed over the target rather than left as debris: {leftovers:?}"
        );
    }

    #[test]
    fn unknown_keys_load_instead_of_failing() {
        let path = temp_dir("unknown").join("config.toml");
        std::fs::write(
            &path,
            // `tone` and `length` are not hypothetical: every config.toml
            // written before they left RunSettings still carries them.
            "[run]\nhosts = 3\ntone = \"dry\"\nlength = \"20 minutes\"\n\
             future_field = \"whatever\"\n\n[telemetry]\nenabled = true\n",
        )
        .expect("write");

        let config = load_from(&path).expect("a newer build's keys must not lock this build out");
        assert_eq!(config.run.hosts, 3, "the keys this build knows still take effect");
        assert!(
            !config.run.auto_continue,
            "keys the file omits fall back to their defaults"
        );
    }

    #[test]
    fn a_config_written_by_the_translucency_builds_still_opens() {
        // This is the literal file those builds left on disk: every drag of the
        // opacity slider called `save_config`, which rewrote config.toml with
        // an `[appearance]` section. The window is opaque again and the section
        // is gone from the struct, so this is now an unknown *section* rather
        // than an unknown key — and it must load rather than take the whole
        // file down the corrupt path.
        let path = temp_dir("post-translucency").join("config.toml");
        std::fs::write(
            &path,
            "[run]\nhosts = 3\n\n[device]\ngpu_mask = \"1\"\n\n\
             [appearance]\nopacity = 100\n",
        )
        .expect("write");

        let config = load_from(&path).expect("load");
        assert_eq!(config.run.hosts, 3, "the sections this build still has take effect");
        assert_eq!(config.device.gpu_mask, "1", "and none of them are lost to the stale one");
    }

    #[test]
    fn a_corrupt_file_is_preserved_rather_than_overwritten() {
        let dir = temp_dir("corrupt");
        let path = dir.join("config.toml");
        let garbage = "[run\nhosts = = 2";
        std::fs::write(&path, garbage).expect("write");

        let config = load_from(&path).expect("an unparseable file must not stop the app");
        assert_eq!(config, Config::default());

        let preserved = dir.join("config.toml.corrupt");
        assert_eq!(
            std::fs::read_to_string(&preserved).expect("the corrupt file must still exist"),
            garbage,
            "a hand-edited file that got a typo is worth recovering, so it is moved aside intact"
        );
        assert!(
            !path.exists(),
            "the original is moved, so the next save starts from a clean file"
        );
    }

    #[test]
    fn a_host_count_the_cli_rejects_is_clamped_on_load() {
        for hosts in [0u8, 1, 5, 255] {
            let mut config = Config {
                run: RunSettings { hosts, ..RunSettings::default() },
                ..Config::default()
            };
            assert!(sanitise(&mut config), "an out-of-range host count is a change worth saving");
            assert_eq!(
                config.run.hosts, 2,
                "argparse restricts --hosts to 2, 3 or 4, so {hosts} would fail every run"
            );
        }
    }

    #[test]
    fn the_three_host_counts_the_cli_accepts_are_left_alone() {
        for hosts in HOST_CHOICES {
            let mut config = Config {
                run: RunSettings { hosts, ..RunSettings::default() },
                ..Config::default()
            };
            assert!(!sanitise(&mut config), "{hosts} is a valid choice and must not be rewritten");
            assert_eq!(config.run.hosts, hosts);
        }
    }

    #[test]
    fn a_voice_list_longer_than_the_host_count_is_truncated() {
        let mut config = Config {
            run: RunSettings { hosts: 2, ..RunSettings::default() },
            voices: VoiceSettings {
                selected: vec!["alice".into(), "carter".into(), "maya".into()],
            },
            ..Config::default()
        };
        assert!(sanitise(&mut config));
        assert_eq!(
            config.voices.selected,
            vec!["alice".to_string(), "carter".to_string()],
            "a list left over from a wider run would rebind speakers without saying so"
        );
    }

    #[test]
    fn a_voice_list_shorter_than_the_host_count_is_left_for_the_cli_to_fill() {
        let mut config = Config {
            run: RunSettings { hosts: 3, ..RunSettings::default() },
            voices: VoiceSettings { selected: vec!["alice".into()] },
            ..Config::default()
        };
        assert!(!sanitise(&mut config), "an under-filled list is the CLI's to complete");
        assert_eq!(config.voices.selected, vec!["alice".to_string()]);
    }

    #[test]
    fn hosts_is_clamped_before_the_voice_list_is_measured_against_it() {
        // Truncating against the unclamped 9 would leave nine voices bound to
        // a two-host run.
        let mut config = Config {
            run: RunSettings { hosts: 9, ..RunSettings::default() },
            voices: VoiceSettings {
                selected: (0..9).map(|i| format!("voice{i}")).collect(),
            },
            ..Config::default()
        };
        assert!(sanitise(&mut config));
        assert_eq!(config.run.hosts, 2);
        assert_eq!(
            config.voices.selected.len(),
            2,
            "the voice list must be measured against the corrected host count"
        );
    }

    #[test]
    fn a_clamped_file_is_written_back_so_the_bad_value_does_not_return() {
        let path = temp_dir("clamped").join("config.toml");
        std::fs::write(&path, "[run]\nhosts = 7\n").expect("write");

        assert_eq!(load_from(&path).expect("load").run.hosts, 2);
        let reread = std::fs::read_to_string(&path).expect("read");
        assert!(
            reread.contains("hosts = 2"),
            "the correction is persisted, so the file a user opens matches what runs: {reread}"
        );
    }

    #[test]
    fn the_config_path_lives_under_the_apps_own_directory() {
        let path = Config::config_path();
        assert!(path.ends_with("article2pod/config.toml"), "{}", path.display());
    }
}
