"""Checks for the episode-title helpers in article2pod.py.

Run it directly (`python test_title.py`); there is no test runner in this
project and this needs none.  Every function here is named `test_*`, so
pytest picks them up unchanged if one ever arrives.

Only `clean_title` is covered.  `fetch_title` is one network call whose whole
contract is "returns a clean title or None, and never raises" — the first half
is `clean_title`, and the second is verified by reading the try/except, not by
mocking the SDK.
"""

import sys

from article2pod import MAX_TITLE_CHARS, clean_title


def test_a_plain_title_survives_unchanged():
    assert clean_title("Reversal of Thromboprophylaxis in Bariatric Surgery") == (
        "Reversal of Thromboprophylaxis in Bariatric Surgery"
    )


def test_surrounding_quotes_are_stripped_because_models_add_them():
    assert clean_title('"Anticoagulation After Sleeve Gastrectomy"') == (
        "Anticoagulation After Sleeve Gastrectomy"
    )


def test_a_restated_label_is_stripped_rather_than_kept():
    assert clean_title("Title: Venous Thromboembolism Prophylaxis") == (
        "Venous Thromboembolism Prophylaxis"
    )


def test_html_title_indentation_is_collapsed():
    # What a site's template actually hands over.
    assert clean_title("\n      Bariatric Surgery   Outcomes\n    ") == (
        "Bariatric Surgery Outcomes"
    )


def test_a_paragraph_of_explanation_is_refused_rather_than_shown():
    prose = "The title of this article is " + ("x" * MAX_TITLE_CHARS)
    assert clean_title(prose) is None


def test_a_script_line_never_becomes_a_title():
    # The one leak worth naming: the script slot and the title slot must not
    # be able to trade contents.
    assert clean_title("Speaker 1: So the headline finding here is...") is None
    assert clean_title("speaker 2 : and the other arm") is None


def test_nothing_at_all_is_none_rather_than_an_empty_label():
    # An empty string would render as a row with no visible name, which cannot
    # then be clicked to rename.
    for empty in (None, "", "   ", '""', "Title:"):
        assert clean_title(empty) is None, repr(empty)


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
