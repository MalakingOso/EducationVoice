"""Checks for the episode-description helpers in article2pod.py.

Run it directly (`python test_description.py`); same shape and same reasons as
test_title.py, which this sits beside.

Only `clean_description` is covered.  `fetch_description` is one network call
whose whole contract is "returns a clean blurb or None, and never raises" —
the first half is `clean_description`, the second is the try/except.
"""

import sys

from article2pod import MAX_DESCRIPTION_CHARS, clean_description


def test_paragraphs_become_one_line_of_p_blocks():
    assert clean_description("First para.\n\nSecond para.") == (
        "<p>First para.</p><p>Second para.</p>"
    )


def test_the_result_never_contains_a_literal_newline():
    # A newline anywhere breaks the show-notes panel, so the wrapping has to
    # survive whatever spacing the model chose.
    out = clean_description("One.\n\n\n   \n\nTwo.\nStill two.")
    assert "\n" not in out
    assert out == "<p>One.</p><p>Two. Still two.</p>"


def test_em_dashes_become_hyphens():
    assert clean_description("A trial—a good one—ran.") == (
        "<p>A trial-a good one-ran.</p>"
    )


def test_html_the_model_emitted_is_stripped_not_nested():
    # Asking for prose does not stop a model returning markup anyway, and
    # <br> renders as literal text on the Spotify desktop app.
    assert clean_description("<p>Wrapped <b>already</b>.<br>Same para.</p>") == (
        "<p>Wrapped already.Same para.</p>"
    )


def test_a_stray_angle_bracket_in_prose_is_escaped_rather_than_eaten():
    assert clean_description("Risk was < 5% & falling.") == (
        "<p>Risk was &lt; 5% &amp; falling.</p>"
    )


def test_a_restated_label_and_a_code_fence_are_stripped():
    assert clean_description("Description: The blurb.") == "<p>The blurb.</p>"
    assert clean_description("```\nThe blurb.\n```") == "<p>The blurb.</p>"


def test_a_script_line_never_becomes_a_description():
    # The same trade clean_title refuses, in the other direction.
    assert clean_description("Speaker 1: So the headline finding here is...") is None


def test_paragraphs_past_the_cap_are_dropped_at_a_boundary():
    # Never cut mid-sentence: whole paragraphs are kept while they fit.
    long_para = "y" * (MAX_DESCRIPTION_CHARS // 2)
    out = clean_description(f"Short opener.\n\n{long_para}\n\n{long_para}")
    assert out.startswith("<p>Short opener.</p>")
    assert len(out) <= MAX_DESCRIPTION_CHARS
    assert out.count("<p>") == 2


def test_a_runaway_first_paragraph_is_refused_rather_than_truncated():
    assert clean_description("z" * (MAX_DESCRIPTION_CHARS + 1)) is None


def test_nothing_at_all_is_none_rather_than_an_empty_summary():
    for empty in (None, "", "   ", "UNKNOWN", "Description:", "<p></p>"):
        assert clean_description(empty) is None, repr(empty)


def main():
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    failed = 0
    for test in tests:
        try:
            test()
            print(f"ok   {test.__name__}")
        except AssertionError as e:
            failed += 1
            print(f"FAIL {test.__name__}: {e}")
    print(f"\n{len(tests) - failed}/{len(tests)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
