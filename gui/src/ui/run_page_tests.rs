//! Tests for the Run page's source classifier.
//!
//! `accept_source` is the only gate between what is dropped, browsed or typed
//! and what reaches a three-minute Python process. Everything it lets through
//! wrongly is discovered minutes later, in a log line that has scrolled past.

use super::*;

/// A private directory per test, so a failing one cannot take another's files.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("a2p_source_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn touch(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, b"x").unwrap();
    path
}

#[test]
fn a_directory_is_refused_rather_than_handed_to_python() {
    let dir = temp_dir("dir");
    let err = accept_source(&dir.to_string_lossy())
        .expect_err("a folder is not an article and must not start a run");
    assert!(
        err.contains("folder"),
        "the message has to say what is wrong with it, got {err:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_extension_the_pipeline_cannot_read_is_refused() {
    let dir = temp_dir("ext");
    let path = touch(&dir, "paper.docx");
    let err = accept_source(&path.to_string_lossy())
        .expect_err(".docx is not one of the three the pipeline reads");
    assert!(
        err.contains(".docx"),
        "the message must name the extension it refused, got {err:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_extension_the_pipeline_reads_is_accepted() {
    let dir = temp_dir("ok");
    for name in ["a.pdf", "b.txt", "c.md"] {
        let path = touch(&dir, name);
        assert_eq!(
            accept_source(&path.to_string_lossy()),
            Ok(Source::File(path.clone())),
            "{name} is one of READABLE and must be accepted"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_uppercase_extension_is_accepted_because_the_filesystem_allows_one() {
    let dir = temp_dir("case");
    let path = touch(&dir, "ROTBIGS.PDF");
    assert!(
        accept_source(&path.to_string_lossy()).is_ok(),
        "a PDF is a PDF whatever case the exporter wrote"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_url_is_accepted_without_touching_the_filesystem() {
    assert_eq!(
        accept_source("https://example.com/article"),
        Ok(Source::Url("https://example.com/article".to_string())),
        "a URL is the reason the typed field survives the redesign"
    );
}

#[test]
fn the_stdin_sentinel_is_accepted() {
    assert_eq!(accept_source("-"), Ok(Source::Stdin));
}

#[test]
fn a_missing_file_is_refused_rather_than_read_as_raw_prose() {
    // `ingest_article` falls through to treating an unreadable source as the
    // article text itself, so a typo would otherwise produce an episode about
    // its own filename.
    let dir = temp_dir("gone");
    let err = accept_source(&dir.join("never-written.pdf").to_string_lossy())
        .expect_err("a path to nothing must not start a run");
    assert!(err.contains("no file"), "got {err:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_empty_source_is_refused() {
    assert!(accept_source("   ").is_err(), "whitespace is not a source");
}

#[test]
fn surrounding_whitespace_is_trimmed_because_a_paste_carries_it() {
    assert_eq!(
        accept_source("  https://example.com/a  "),
        Ok(Source::Url("https://example.com/a".to_string()))
    );
}

#[test]
fn only_a_file_source_seeds_the_chip_on_mount() {
    let dir = temp_dir("seed");
    let path = touch(&dir, "x.pdf");
    assert!(looks_like_a_file(&path.to_string_lossy()));
    assert!(
        !looks_like_a_file("https://example.com/a"),
        "a URL belongs in the field, not behind a chip that hides it"
    );
    assert!(!looks_like_a_file("-"));
    let _ = std::fs::remove_dir_all(&dir);
}
