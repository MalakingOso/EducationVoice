//! Locating the Python half of the app.
//!
//! The GUI is a front end for `article2pod.py` running in a specific
//! interpreter — the project's `.venv`, whose `torch` is pinned to
//! `2.12.1+xpu` and whose `vibevoice` is hand-patched in place. Any other
//! Python on the machine will either fail to import or, worse, import and
//! produce wrong audio. So the interpreter is always addressed by absolute
//! path and never as bare `python`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

/// The file whose presence identifies the project root.
const ROOT_MARKER: &str = "article2pod.py";

/// Escape hatch for a layout this search cannot infer (a packaged build, a
/// checkout mounted somewhere unusual). Checked first and validated like any
/// other candidate, so a stale value fails loudly instead of silently
/// selecting the wrong tree.
const ROOT_ENV: &str = "ARTICLE2POD_ROOT";

/// Walk `start` and its ancestors, returning the first directory containing
/// `marker`.
///
/// Pure and separately testable: every way the app might be launched — `dx
/// serve` from `gui/`, the built binary in `gui/target/debug/`, a desktop
/// launcher with an unrelated working directory — reduces to "ascend from
/// some path", and that is the part worth pinning down with a test.
pub fn ascend_to_marker(start: &Path, marker: &str) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(marker).is_file())
        .map(Path::to_path_buf)
}

/// Every path the runner needs, resolved once at startup.
#[derive(Debug, Clone, PartialEq)]
pub struct Paths {
    pub root: PathBuf,
    /// The venv interpreter, by absolute path. Never bare `python`.
    pub python: PathBuf,
    pub script: PathBuf,
    /// Where episodes and their sidecars land.
    pub output_dir: PathBuf,
    /// Hand-kept scripts worth re-voicing. Tracked in git, unlike `output/`.
    pub scripts_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        let root = project_root()?;
        let python = root.join(".venv/bin/python");
        if !python.is_file() {
            return Err(anyhow!(
                "no interpreter at {}. The GUI runs article2pod.py in the \
                 project's own .venv — its torch and vibevoice are pinned and \
                 patched, and no other Python on this machine will do. \
                 Create it, or point {ROOT_ENV} at a checkout that has one.",
                python.display()
            ));
        }
        Ok(Self {
            script: root.join(ROOT_MARKER),
            output_dir: root.join("output"),
            scripts_dir: root.join("scripts"),
            python,
            root,
        })
    }
}

/// Find the project root, trying the most specific source of truth first.
fn project_root() -> Result<PathBuf> {
    if let Some(from_env) = std::env::var_os(ROOT_ENV) {
        let dir = PathBuf::from(&from_env);
        if dir.join(ROOT_MARKER).is_file() {
            return Ok(dir);
        }
        return Err(anyhow!(
            "{ROOT_ENV} is set to {:?} but there is no {ROOT_MARKER} there",
            from_env
        ));
    }

    // The built binary sits at <root>/gui/target/<profile>/, so ascending from
    // it finds the root in a release build and under `cargo run` alike.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(found) = ascend_to_marker(&exe, ROOT_MARKER) {
            return Ok(found);
        }
    }

    // `dx serve` runs with the crate directory as the working directory.
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(found) = ascend_to_marker(&cwd, ROOT_MARKER) {
            return Ok(found);
        }
    }

    // Last resort, and the one that works in `cargo test`, where the harness
    // binary lives under a target directory that may have been relocated.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    ascend_to_marker(manifest, ROOT_MARKER).with_context(|| {
        format!(
            "could not find {ROOT_MARKER} above the executable, the working \
             directory, or {}. Set {ROOT_ENV} to the checkout root.",
            manifest.display()
        )
    })
}

/// Derive a filesystem-safe episode stem from whatever the user typed as a
/// source, so one run's artefacts group together as `<stem>.script.txt`,
/// `<stem>.wav` and `<stem>.run.json`.
///
/// Everything outside `[a-z0-9._-]` collapses to a single `-`, because these
/// names reach a shell-free `Command` but also a file browser, and a source
/// can be a URL with query strings and percent escapes.
pub fn episode_stem(source: &str) -> String {
    let trimmed = source.trim().trim_end_matches('/');

    // For a URL the last path segment carries the meaning; for a file path
    // that is exactly the file stem, so one rule covers both.
    let tail = trimmed.rsplit('/').find(|s| !s.is_empty()).unwrap_or(trimmed);

    // Drop a query string before it becomes part of the name.
    let tail = tail.split(['?', '#']).next().unwrap_or(tail);

    // Strip a single known extension; a bare dot in the middle stays.
    let tail = Path::new(tail)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(tail);

    let mut out = String::new();
    for ch in tail.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches(['-', '.']).to_string();

    if out.is_empty() {
        "episode".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crate_directory_ascends_to_the_project_root() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = ascend_to_marker(manifest, ROOT_MARKER)
            .expect("the gui crate always sits one level below the project root");
        assert!(
            root.join(ROOT_MARKER).is_file(),
            "the returned directory must be the one holding {ROOT_MARKER}"
        );
        assert_eq!(
            root,
            manifest.parent().unwrap(),
            "gui/ is a direct child of the root, so exactly one level is climbed"
        );
    }

    #[test]
    fn ascending_returns_none_when_no_ancestor_holds_the_marker() {
        assert_eq!(
            ascend_to_marker(Path::new("/"), ROOT_MARKER),
            None,
            "the search must terminate rather than loop at the filesystem root"
        );
    }

    #[test]
    fn a_pdf_path_becomes_its_bare_stem() {
        assert_eq!(episode_stem("/home/berkley/ROTBIGS.pdf"), "rotbigs");
    }

    #[test]
    fn a_url_becomes_its_last_path_segment() {
        assert_eq!(
            episode_stem("https://example.com/journals/vte-prophylaxis"),
            "vte-prophylaxis"
        );
    }

    #[test]
    fn a_trailing_slash_does_not_swallow_the_segment() {
        assert_eq!(episode_stem("https://example.com/some-article/"), "some-article");
    }

    #[test]
    fn a_query_string_is_dropped_rather_than_encoded_into_the_name() {
        assert_eq!(
            episode_stem("https://example.com/article?utm_source=x&id=9"),
            "article"
        );
    }

    #[test]
    fn runs_of_unsafe_characters_collapse_to_one_dash() {
        assert_eq!(episode_stem("My  Great :: Paper!!!.txt"), "my-great-paper");
    }

    #[test]
    fn a_source_with_nothing_usable_falls_back_to_a_name_that_still_works() {
        // "-" is the CLI's stdin source, and it must still yield a filename.
        assert_eq!(episode_stem("-"), "episode");
        assert_eq!(episode_stem("   "), "episode");
        assert_eq!(episode_stem("///"), "episode");
    }

    #[test]
    fn non_ascii_titles_still_produce_a_usable_stem() {
        assert_eq!(
            episode_stem("Étude sur la thrombose.pdf"),
            "tude-sur-la-thrombose",
            "accented leads are dropped rather than transliterated, but the \
             name stays unique enough to file under and never empty"
        );
    }
}
