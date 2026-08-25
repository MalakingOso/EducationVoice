//! The Library: what previous runs left on disk.
//!
//! There is no database. A run's evidence is the files it wrote, and the
//! Library is whatever can be reconstructed from them: `<stem>.script.txt`,
//! `<stem>.wav`/`.mp3`, and a sidecar `<stem>.run.json` recording what
//! produced them. Scanning pairs those up by stem.
//!
//! The sidecar exists because the files alone cannot say which voices ran, on
//! which device, from which source URL — and re-deriving a source from a stem
//! is lossy in one direction only (`episode_stem` collapses punctuation).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::runner::RunOutcome;

/// Sidecar suffix. Not an "extension": `Path::extension` on
/// `rotbigs.run.json` returns `json`, which would collide with every other
/// JSON file, so the whole tail is matched as a string.
const META_SUFFIX: &str = ".run.json";

/// How the CLI names a script it wrote next to the audio.
const SCRIPT_SUFFIX: &str = ".script.txt";

/// Every pre-GUI run wrote its script to this one path, overwriting the last.
/// Only the most recent survives, but it is still a real script worth showing.
const LEGACY_SCRIPT: &str = "script.txt";
const LEGACY_STEM: &str = "script";

/// What a run recorded about itself.
///
/// **Every field defaults.** Two things depend on that. A sidecar written by an
/// older build is missing whatever has been added since, and must still load
/// rather than being moved aside as corrupt. And renaming an episode that
/// never had a sidecar — everything made before the GUI existed — has nothing
/// to write but a title: inventing a `started` for it would file a
/// year-old episode under "Today", and inventing `hosts` would render
/// "0 hosts" as though it were measured.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunMeta {
    /// The article's title, as the Library labels the row. From the Python
    /// side's `title` event, or typed in by hand. Absent when neither
    /// happened, which is what the fallback chain in [`Episode::title`] is for.
    pub title: Option<String>,
    /// What the user gave: URL, path, or "-".
    pub source: String,
    /// 0 means "not recorded", which is different from any host count the CLI
    /// accepts — so the row can omit it rather than print a measurement it
    /// does not have.
    pub hosts: u8,
    /// In speaker order. Empty means the CLI's default roster was used — the
    /// GUI omits `--voices` entirely in that case, so there is nothing to name.
    pub voices: Vec<String>,
    /// As reported by the tts stage event, e.g. "xpu".
    pub device: Option<String>,
    /// The Claude model that wrote the script.
    pub model: Option<String>,
    /// When the run started, which is the day the Library groups it under.
    /// `None` for a sidecar this build wrote by hand during a rename.
    pub started: Option<chrono::DateTime<chrono::Local>>,
    pub finished: Option<chrono::DateTime<chrono::Local>>,
    pub elapsed_secs: Option<u64>,
    /// "completed" | "cancelled" | "failed". A string rather than the runner's
    /// enum so an old sidecar written by a future version still deserializes.
    pub outcome: String,
}

impl RunMeta {
    /// Fold in what the previous sidecar for this stem knew and this run does
    /// not.
    ///
    /// A stem is written more than once: stage 1 of the gated flow records the
    /// article's title, and stage 2 — sharing the stem, because it is the same
    /// episode — replaces the whole sidecar when it finishes. Synthesis has no
    /// title of its own and never will; it reads a script off disk and has no
    /// article to ask about. Without this, the default flow fetches a title and
    /// then discards it at the moment the episode is finally done.
    ///
    /// Only fields this run genuinely has nothing to say about are carried.
    /// Everything measured — the device, the elapsed time, the outcome —
    /// belongs to the run that just happened and overwrites freely.
    pub fn carrying_forward(mut self, previous: Option<&RunMeta>) -> Self {
        let Some(previous) = previous else { return self };
        if self.title.is_none() {
            self.title = previous.title.clone();
        }
        // Re-voicing from the Library hands over a script path and no source
        // at all, so an empty one here means "unknown", not "none".
        if self.source.trim().is_empty() {
            self.source = previous.source.clone();
        }
        self
    }
}

/// The string a [`RunOutcome`] is recorded under.
pub fn outcome_label(outcome: &RunOutcome) -> &'static str {
    match outcome {
        RunOutcome::Completed => "completed",
        RunOutcome::Cancelled => "cancelled",
        RunOutcome::Failed { .. } => "failed",
    }
}

/// One row in the Library. Every field but `stem` is optional because any of
/// the three artefacts can be missing: a cancelled run leaves a script and no
/// audio, a hand-kept script has never had a sidecar, and audio from before
/// the GUI has neither.
#[derive(Debug, Clone, PartialEq)]
pub struct Episode {
    pub stem: String,
    pub script: Option<PathBuf>,
    pub audio: Option<PathBuf>,
    pub meta: Option<RunMeta>,
    /// Newest mtime among the files this episode actually references. A
    /// leftover WAV that lost to an MP3 is not one of them, so it cannot make
    /// a stale row float to the top.
    pub modified: Option<SystemTime>,
}

impl Episode {
    /// A human label for the row, best first.
    ///
    /// The recorded title, then the source's last component, then the stem. No
    /// step has to succeed: `fetch_title` is a network call that can fail, a
    /// source can be stdin, and an episode made before any of this existed has
    /// only a filename. The row is nameable by hand either way, which is what
    /// makes the automatic part safe to be imperfect.
    pub fn title(&self) -> String {
        if let Some(meta) = &self.meta {
            if let Some(title) = meta.title.as_deref().map(str::trim) {
                if !title.is_empty() {
                    return title.to_string();
                }
            }
            let source = meta.source.trim();
            // "-" is the CLI's stdin sentinel; it names nothing a reader could
            // recognise, so the stem is the better label.
            if !source.is_empty() && source != "-" {
                return basename_of(source);
            }
        }
        self.stem.clone()
    }

    /// The day this episode is filed under, or `None` when nothing recorded it.
    ///
    /// Read from the sidecar rather than from file mtimes on purpose: an mtime
    /// is when the bytes were last touched, which a copy or a restore changes.
    /// An episode with no sidecar is honestly undated rather than filed under
    /// whenever its file was last written.
    pub fn day(&self) -> Option<chrono::NaiveDate> {
        use chrono::Datelike;
        let started = self.meta.as_ref()?.started?;
        chrono::NaiveDate::from_ymd_opt(started.year(), started.month(), started.day())
    }

    /// Whether this row matches a search term. Case-insensitive over the
    /// label and the source, which are the two things a reader would remember.
    pub fn matches(&self, needle: &str) -> bool {
        let needle = needle.trim().to_lowercase();
        if needle.is_empty() {
            return true;
        }
        if self.title().to_lowercase().contains(&needle) {
            return true;
        }
        self.meta
            .as_ref()
            .is_some_and(|m| m.source.to_lowercase().contains(&needle))
    }

    /// True when there is audio on disk to play.
    pub fn is_playable(&self) -> bool {
        self.audio.is_some()
    }
}

/// The last component of a source, which is what a reader recognises.
///
/// Handles a URL and a path with the same rule — split on both separators and
/// take the last non-empty piece — because a URL's path uses `/` on every
/// platform and `Path::file_name` would keep the whole thing on Windows. A
/// query string is dropped; a bare host with no path falls back to the host.
fn basename_of(source: &str) -> String {
    let head = source.split(['?', '#']).next().unwrap_or(source);
    let stripped = head
        .strip_prefix("https://")
        .or_else(|| head.strip_prefix("http://"))
        .unwrap_or(head);
    stripped
        .split(['/', '\\'])
        .rev()
        .find(|p| !p.is_empty())
        .unwrap_or(source)
        .to_string()
}

/// Group episodes by the day they ran, newest day first.
///
/// Mirrors Beamer's `grouped_by_day`, with one deliberate divergence: Beamer
/// resolves an unparseable timestamp to *today*, which here would sweep every
/// pre-GUI episode into today's group. Keying on `Option<NaiveDate>` files
/// them honestly instead — `None` sorts below every `Some`, so after the
/// reverse the undated bucket lands last without a special case.
///
/// Order inside a group is the order given, so the caller's newest-first sort
/// carries through.
pub fn group_by_day(episodes: &[Episode]) -> Vec<(String, Vec<Episode>)> {
    let today = chrono::Local::now().date_naive();
    let yesterday = today.pred_opt().unwrap_or(today);

    let mut groups: BTreeMap<Option<chrono::NaiveDate>, Vec<Episode>> = BTreeMap::new();
    for ep in episodes {
        groups.entry(ep.day()).or_default().push(ep.clone());
    }

    groups
        .into_iter()
        .rev()
        .map(|(day, eps)| {
            let label = match day {
                Some(d) if d == today => "Today".to_string(),
                Some(d) if d == yesterday => "Yesterday".to_string(),
                Some(d) => d.format("%B %d, %Y").to_string(),
                // Not "Unknown": the run is not in doubt, only its date.
                None => "No run record".to_string(),
            };
            (label, eps)
        })
        .collect()
}

/// A candidate for a slot, with the precedence that decides ties.
struct Ranked {
    rank: u8,
    path: PathBuf,
    modified: Option<SystemTime>,
}

#[derive(Default)]
struct Builder {
    script: Option<Ranked>,
    audio: Option<Ranked>,
    meta: Option<RunMeta>,
    meta_modified: Option<SystemTime>,
}

/// Keep the higher-ranked candidate. `read_dir` order is arbitrary, so
/// first-wins would make the mp3/wav choice depend on inode layout.
fn offer(slot: &mut Option<Ranked>, candidate: Ranked) {
    let better = match slot {
        Some(current) => candidate.rank > current.rank,
        None => true,
    };
    if better {
        *slot = Some(candidate);
    }
}

/// `.mp3` outranks `.wav`: the CLI deletes the WAV once it has converted it,
/// so a stem holding both means the conversion died partway and the WAV is
/// the leftover, not the product.
fn audio_rank(ext: &str) -> Option<u8> {
    match ext {
        "mp3" => Some(2),
        "wav" => Some(1),
        _ => None,
    }
}

fn mtime(entry: &std::fs::DirEntry) -> Option<SystemTime> {
    entry.metadata().ok().and_then(|m| m.modified().ok())
}

/// Scan `output/` and `scripts/` and return episodes, newest first.
///
/// Blocking IO — callers run it via `spawn_blocking`.
pub fn scan(paths: &Paths) -> Result<Vec<Episode>> {
    let mut builders: BTreeMap<String, Builder> = BTreeMap::new();

    for entry in read_dir_opt(&paths.output_dir)? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let when = mtime(&entry);

        if let Some(stem) = name.strip_suffix(META_SUFFIX) {
            let b = builders.entry(stem.to_string()).or_default();
            b.meta = read_meta(&entry.path());
            b.meta_modified = when;
        } else if name == LEGACY_SCRIPT {
            // Rank 0 so a real `script.script.txt` shadows it rather than the
            // other way round, whichever `read_dir` hands over first.
            offer(
                &mut builders.entry(LEGACY_STEM.to_string()).or_default().script,
                Ranked { rank: 0, path: entry.path(), modified: when },
            );
        } else if let Some(stem) = name.strip_suffix(SCRIPT_SUFFIX) {
            offer(
                &mut builders.entry(stem.to_string()).or_default().script,
                Ranked { rank: 1, path: entry.path(), modified: when },
            );
        } else if let Some(rank) = Path::new(name).extension().and_then(|e| e.to_str()).and_then(audio_rank) {
            let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
            offer(
                &mut builders.entry(stem.to_string()).or_default().audio,
                Ranked { rank, path: entry.path(), modified: when },
            );
        }
    }

    for entry in read_dir_opt(&paths.scripts_dir)? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(stem) = name.strip_suffix(".txt") else { continue };
        // Highest rank: `scripts/` is the curated, git-tracked copy. An
        // `output/` script is a run artefact that the next run may replace.
        offer(
            &mut builders.entry(stem.to_string()).or_default().script,
            Ranked { rank: 2, path: entry.path(), modified: mtime(&entry) },
        );
    }

    let mut episodes: Vec<Episode> = builders
        .into_iter()
        .map(|(stem, b)| {
            let modified = [
                b.script.as_ref().and_then(|r| r.modified),
                b.audio.as_ref().and_then(|r| r.modified),
                b.meta_modified,
            ]
            .into_iter()
            .flatten()
            .max();
            Episode {
                stem,
                script: b.script.map(|r| r.path),
                audio: b.audio.map(|r| r.path),
                meta: b.meta,
                modified,
            }
        })
        .collect();

    sort_newest_first(&mut episodes);
    Ok(episodes)
}

/// A missing directory is not an error: a fresh checkout has no `output/`,
/// and `scripts/` is optional. Anything else — a permission problem, a file
/// where the directory should be — is worth surfacing rather than showing an
/// empty Library and calling it accurate.
fn read_dir_opt(dir: &Path) -> Result<Vec<std::fs::DirEntry>> {
    match std::fs::read_dir(dir) {
        Ok(iter) => Ok(iter.flatten().collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("could not read {}", dir.display())),
    }
}

/// Newest first. `Option` orders `None` below `Some`, so the reversed compare
/// puts undated entries last, where a row nothing is known about belongs.
fn sort_newest_first(episodes: &mut [Episode]) {
    episodes.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.stem.cmp(&b.stem)));
}

/// The sidecar path for an episode stem.
pub fn meta_path(paths: &Paths, stem: &str) -> PathBuf {
    paths.output_dir.join(format!("{stem}{META_SUFFIX}"))
}

/// Write a sidecar, replacing any previous one atomically.
pub fn write_meta(path: &Path, meta: &RunMeta) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(dir)
        .with_context(|| format!("could not create {}", dir.display()))?;

    let json = serde_json::to_string_pretty(meta)?;

    // The temp file must be a sibling: `rename` across filesystems fails with
    // EXDEV, and the obvious alternative home for it — /tmp — is tmpfs here.
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);

    std::fs::write(&tmp, json).with_context(|| format!("could not write {}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("could not replace {}", path.display()));
    }
    Ok(())
}

/// Rename an episode, writing the title back to its sidecar.
///
/// This is what makes the automatic naming chain safe to be imperfect: any row
/// the Python side names badly — or never named at all, because it predates
/// the whole mechanism — can be fixed in place. An episode with no sidecar
/// gets one holding the title and nothing else, which every other field's
/// default is there to permit.
pub fn rename(paths: &Paths, stem: &str, title: &str) -> Result<()> {
    check_stem(stem)?;
    let path = meta_path(paths, stem);
    let mut meta = read_meta(&path).unwrap_or_default();
    let title = title.trim();
    meta.title = (!title.is_empty()).then(|| title.to_string());
    write_meta(&path, &meta)
}

/// Read a sidecar. `None` when absent or unparseable.
///
/// An unparseable one is moved aside rather than left where the next
/// `write_meta` would overwrite it — the bytes are the only record of what
/// went wrong, and they are cheap to keep.
pub fn read_meta(path: &Path) -> Option<RunMeta> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<RunMeta>(&text) {
        Ok(meta) => Some(meta),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "sidecar is not valid RunMeta");
            preserve_corrupt(path);
            None
        }
    }
}

/// How many numbered preservations to try before giving up.
const CORRUPT_ATTEMPTS: u32 = 20;

/// Move a bad sidecar to `<path>.corrupt`, or to `<path>.corrupt.2`, `.3`, …
/// if that name is taken.
///
/// Never overwrite an existing `.corrupt`: a second failure of the same file
/// means something keeps rewriting it, and the first capture is the one taken
/// closest to whatever broke it. When every name is used the file is left
/// exactly where it is — losing a sidecar is worse than showing a row without
/// one, and the warning says so.
fn preserve_corrupt(path: &Path) {
    for n in 1..=CORRUPT_ATTEMPTS {
        let mut name = path.as_os_str().to_os_string();
        name.push(if n == 1 { ".corrupt".to_string() } else { format!(".corrupt.{n}") });
        let target = PathBuf::from(name);
        if target.exists() {
            continue;
        }
        match std::fs::rename(path, &target) {
            Ok(()) => tracing::warn!(preserved = %target.display(), "moved the bad sidecar aside"),
            Err(e) => tracing::warn!(error = %e, "could not preserve the bad sidecar"),
        }
        return;
    }
    tracing::warn!(
        path = %path.display(),
        "{CORRUPT_ATTEMPTS} preserved copies already exist; leaving this one in place"
    );
}

/// Delete an episode's files. Returns how many were removed.
pub fn delete(paths: &Paths, stem: &str) -> Result<usize> {
    check_stem(stem)?;

    let mut targets = vec![
        paths.output_dir.join(format!("{stem}{SCRIPT_SUFFIX}")),
        paths.output_dir.join(format!("{stem}.wav")),
        paths.output_dir.join(format!("{stem}.mp3")),
        meta_path(paths, stem),
        // The curated copy goes too, or the row reappears on the next scan
        // still holding a script. It is tracked in git, so this is undoable.
        paths.scripts_dir.join(format!("{stem}.txt")),
    ];
    if stem == LEGACY_STEM {
        // Without this the legacy row's Delete button removes nothing at all:
        // its script is `script.txt`, not `script.script.txt`.
        targets.push(paths.output_dir.join(LEGACY_SCRIPT));
    }

    let mut removed = 0;
    for target in targets {
        // `symlink_metadata` does not follow links, and `remove_file` unlinks
        // the link itself — so a planted `rotbigs.wav -> ~/.ssh/id_ed25519`
        // costs the link, never the target.
        if std::fs::symlink_metadata(&target).is_err() {
            continue;
        }
        match std::fs::remove_file(&target) {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(path = %target.display(), error = %e, "could not remove"),
        }
    }
    Ok(removed)
}

/// A stem reaches here from a filename, but also from a UI row that a
/// deserialized sidecar could have named. Treating it as trusted is how a
/// delete walks out of `output/`.
fn check_stem(stem: &str) -> Result<()> {
    if stem.is_empty() {
        return Err(anyhow!("refusing to delete: the episode stem is empty"));
    }
    if stem.contains("..") || stem.contains(std::path::is_separator) {
        return Err(anyhow!(
            "refusing to delete {stem:?}: an episode stem is a bare filename, \
             and this one could escape the directories the Library owns"
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
