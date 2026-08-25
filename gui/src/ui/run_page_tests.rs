//! Tests for the Run page's source classifier.
//!
//! `accept_source` is the only gate between what is dropped or browsed and
//! what reaches a three-minute Python process. Everything it lets through
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
fn a_dropped_link_gets_a_chip_of_its_own() {
    // The zone used to show a chip only for files, because a URL stayed
    // visible in the text field beside it. That field is gone, so a link with
    // no chip would set the source and change nothing on screen — which reads
    // as the drop having been refused.
    let (label, detail) = chip_for("https://example.com/article")
        .expect("a dropped link must show as a chip, the same as a dropped file");
    assert_eq!(detail, "https://example.com/article", "the full link stays readable");
    assert!(label.starts_with("https://example.com"), "the chip names the link: {label}");

    let long = format!("https://example.com/{}", "a".repeat(200));
    let (label, detail) = chip_for(&long).expect("chip");
    assert!(
        label.len() < long.len() && label.ends_with("..."),
        "a long link is truncated for the chip rather than widening the zone: {label}"
    );
    assert_eq!(detail, long, "the untruncated link is still shown underneath");
}

#[test]
fn a_file_chip_names_the_file_and_keeps_the_path() {
    let dir = temp_dir("chip");
    let path = touch(&dir, "x.pdf");
    let (label, detail) = chip_for(&path.to_string_lossy()).expect("chip");
    assert_eq!(label, "x.pdf", "the chip carries the name, which is what was recognised");
    assert_eq!(detail, path.display().to_string(), "the path stays visible underneath");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nothing_chosen_yields_no_chip() {
    assert!(chip_for("").is_none(), "an empty source leaves the zone showing its prompt");
}
