//! Tests for `components.rs`. The multibyte cases are Beamer's, kept because
//! the bug they guard is the same one here: generated scripts are full of em
//! dashes and curly quotes, and byte slicing panics on them.

use super::*;

#[test]
fn short_text_is_returned_unchanged() {
    assert_eq!(truncate_chars("hello", 80), "hello");
}

#[test]
fn exact_length_gets_no_ellipsis() {
    let text = "a".repeat(80);
    assert_eq!(truncate_chars(&text, 80), text);
}

#[test]
fn one_over_gets_ellipsis() {
    let text = "a".repeat(81);
    assert_eq!(truncate_chars(&text, 80), format!("{}...", "a".repeat(80)));
}

/// Regression: byte slicing panicked with "byte index 80 is not a char
/// boundary" whenever an em dash straddled the cut point.
#[test]
fn a_multibyte_char_across_the_cut_point_does_not_panic() {
    let head = "Please send the quarterly numbers over to accounting before the end of the day";
    let text = format!("{} \u{2014} thanks", head);
    assert!(!text.is_char_boundary(80), "the fixture must straddle byte 80");
    let got = truncate_chars(&text, 80);
    assert_eq!(got.chars().count(), 83, "80 chars plus the ellipsis");
    assert!(got.ends_with("..."));
}

#[test]
fn truncation_counts_characters_not_bytes() {
    let text = "\u{2014}".repeat(90);
    assert_eq!(
        truncate_chars(&text, 80),
        format!("{}...", "\u{2014}".repeat(80))
    );
}

#[test]
fn empty_input_stays_empty() {
    assert_eq!(truncate_chars("", 80), "");
}

// ─── script_stats ────────────────────────────────────────────────────────────

/// The opening of a real generated script, verbatim.
const REAL: &str = "\
Speaker 1: So somebody finally did the thing everybody's been eyeballing.
Speaker 2: Fifty procedures.
Speaker 1: Fifty procedures, 131 studies, 1.7 million patients pooled in.
Speaker 2: Which is what I was trained on.";

#[test]
fn a_real_script_counts_its_turns_and_speakers() {
    let s = script_stats(REAL);
    assert_eq!(s.turns, 4);
    assert_eq!(s.speakers, 2, "distinct ids, not lines");
    assert_eq!(s.words, 27, "the count covers dialogue only; a Speaker label \
         contributes nothing to it");
}

#[test]
fn the_speaker_label_itself_is_not_counted_as_dialogue() {
    let s = script_stats("Speaker 1: one two three");
    assert_eq!(
        s.words, 3,
        "counting 'Speaker' and '1:' would inflate every episode's word count"
    );
}

#[test]
fn prose_between_turns_is_not_counted_as_a_turn() {
    // validate_script strips these, so showing them as turns would promise a
    // script the pipeline will then quietly shorten.
    let s = script_stats("Here is the script:\n\nSpeaker 1: hello\n\nHope that helps!");
    assert_eq!(s.turns, 1);
    assert_eq!(s.speakers, 1);
}

#[test]
fn a_speaker_id_that_is_not_a_number_is_not_a_turn() {
    let s = script_stats("Speaker Alice: hello\nSpeaker 1: hi");
    assert_eq!(
        s.turns, 1,
        "the pipeline's pattern is `Speaker \\d+:`, and agreeing with it is \
         the whole point of counting here"
    );
}

#[test]
fn leading_whitespace_does_not_hide_a_turn() {
    assert_eq!(script_stats("   Speaker 2: indented").turns, 1);
}

#[test]
fn an_out_of_range_speaker_still_counts_as_a_distinct_speaker() {
    // A stray "Speaker 3:" in a two-host script is exactly what makes the run
    // fail; the editor has to show three so the user can see why.
    let s = script_stats("Speaker 1: a\nSpeaker 2: b\nSpeaker 3: c");
    assert_eq!(s.speakers, 3);
}

#[test]
fn an_empty_script_reports_zeroes_rather_than_panicking() {
    assert_eq!(script_stats(""), ScriptStats::default());
}
