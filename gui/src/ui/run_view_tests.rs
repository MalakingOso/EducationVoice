//! The dissolve's geometry, checked without a window.
//!
//! None of this is visible in review: a matrix whose last cell never turns
//! leaves a permanent hole in the dark, and a matrix whose delays all collapse
//! to zero is a hard cut that still compiles, renders, and looks deliberate.

use super::*;

#[test]
fn every_cell_gets_a_delay() {
    assert_eq!(
        delays().len(),
        COLS * ROWS,
        "a missing cell is a permanent transparent hole in the dark view"
    );
}

#[test]
fn the_delays_span_the_whole_duration() {
    let d = delays();
    let max = d.iter().copied().fold(f32::MIN, f32::max);
    let min = d.iter().copied().fold(f32::MAX, f32::min);

    assert!(
        (max - 1.0).abs() < 1e-4,
        "the last cell must land exactly at the end of the duration, not at \
         {max} — otherwise the tail of the animation is dead time and the \
         dissolve looks like it finished early"
    );
    assert!(
        min < 0.06,
        "some cell must turn almost immediately, not at {min}; a matrix that \
         starts late reads as the click having been ignored"
    );
}

#[test]
fn the_delays_are_normalised_into_range() {
    for (i, d) in delays().iter().enumerate() {
        assert!(
            (0.0..=1.0).contains(d),
            "cell {i} has delay {d}, outside 0..1. calc() would produce a \
             negative or overlong animation-delay, which does not error — the \
             cell simply appears at the wrong time or not at all"
        );
    }
}

#[test]
fn the_matrix_is_identical_between_calls() {
    assert_eq!(
        delays(),
        delays(),
        "the dissolve is part of the app's identity and a screenshot of it has \
         to be reproducible; a scrambling matrix reads as noise"
    );
}

#[test]
fn no_seed_sits_under_the_start_button() {
    // The launcher's action row is at the bottom centre. The dissolve is
    // deliberately not thrown from it: it reads as the whole panel changing
    // state rather than as something spreading out of a control.
    for (x, y, _) in SEEDS {
        let under_button = (x - 0.5).abs() < 0.06 && (y - 0.86).abs() < 0.06;
        assert!(
            !under_button,
            "seed at ({x}, {y}) sits under the Start button, which is the one \
             origin this layout is defined as not having"
        );
    }
}

#[test]
fn the_fronts_actually_collide() {
    // The whole reason for nine seeds is the seams. If one seed reached every
    // cell first, this would be a single-origin dissolve wearing a costume.
    let mut winners = std::collections::HashSet::new();

    for r in 0..ROWS {
        for c in 0..COLS {
            let x = (c as f32 + 0.5) / COLS as f32;
            let y = (r as f32 + 0.5) / ROWS as f32;
            let mut best = (f32::INFINITY, usize::MAX);
            for (i, (sx, sy, delay)) in SEEDS.iter().enumerate() {
                let dx = (x - sx) * ASPECT;
                let dy = y - sy;
                let v = delay + (dx * dx + dy * dy).sqrt();
                if v < best.0 {
                    best = (v, i);
                }
            }
            winners.insert(best.1);
        }
    }

    assert_eq!(
        winners.len(),
        SEEDS.len(),
        "only {} of {} seeds ever reach a cell first. A seed that wins nothing \
         is invisible, and the seams it was placed to create do not exist",
        winners.len(),
        SEEDS.len()
    );
}

#[test]
fn the_jitter_actually_ragged_the_front() {
    // Without jitter the delay field is smooth and the front reads as clean
    // arcs. This checks that neighbouring cells genuinely disagree.
    let d = delays();
    let mut rough = 0;
    for r in 0..ROWS {
        for c in 0..(COLS - 1) {
            let a = d[r * COLS + c];
            let b = d[r * COLS + c + 1];
            if (a - b).abs() > 0.02 {
                rough += 1;
            }
        }
    }
    assert!(
        rough > (ROWS * COLS) / 4,
        "only {rough} neighbouring pairs differ meaningfully; the front is too \
         smooth and will read as an expanding arc rather than a dot matrix"
    );
}

#[test]
fn the_hash_spreads_across_the_range() {
    // A hash that clumps would jitter every cell by roughly the same amount,
    // which is a smooth field with an offset rather than a ragged one.
    let mut buckets = [0usize; 4];
    for i in 0..(COLS * ROWS) {
        let v = jitter_at(i);
        buckets[((v * 4.0) as usize).min(3)] += 1;
    }
    for (i, n) in buckets.iter().enumerate() {
        assert!(
            *n > (COLS * ROWS) / 8,
            "quarter {i} of the jitter range holds only {n} of {} cells; the \
             hash is clumping and the front will not break up",
            COLS * ROWS
        );
    }
}

#[test]
fn the_emitted_delays_span_the_whole_duration_in_milliseconds() {
    // What actually reaches the DOM, rather than the 0..1 field behind it.
    // These were `calc()` expressions over a custom property and every one of
    // them was silently discarded, which is a whole-view failure that no test
    // of the normalised field could have caught.
    let ms: Vec<u32> = delays()
        .iter()
        .map(|d| delay_ms(*d, DISSOLVE_IN_MS))
        .collect();

    assert_eq!(
        ms.iter().copied().min(),
        Some(0),
        "no cell turns on the first frame; the dissolve opens with a pause and \
         reads as the click having been ignored"
    );
    assert_eq!(
        ms.iter().copied().max(),
        Some(DISSOLVE_IN_MS),
        "the last cell does not land at {DISSOLVE_IN_MS}ms, so either the tail \
         of the dissolve is dead time or a cell turns after the ground has \
         already sealed over it"
    );

    // The failure this whole change exists to stop: every cell on 0ms is a
    // hard cut to black that still compiles and still looks deliberate.
    let distinct = ms.iter().copied().collect::<std::collections::HashSet<_>>();
    assert!(
        distinct.len() > 500,
        "only {} distinct start times across {} cells; the matrix is firing in \
         a handful of blocks rather than as a front",
        distinct.len(),
        ms.len()
    );
}

#[test]
fn leaving_is_the_shorter_of_the_two() {
    // DISSOLVE_OUT_MS is also EXIT_DISSOLVE in app.rs, the sleep that holds
    // this view up after a successful run. If the exit were the longer one it
    // would be torn down mid-retreat.
    assert!(
        DISSOLVE_OUT_MS < DISSOLVE_IN_MS,
        "the exit is meant to be the unceremonious one"
    );
    assert_eq!(
        delay_ms(1.0, DISSOLVE_OUT_MS),
        DISSOLVE_OUT_MS,
        "the last cell must finish leaving within EXIT_DISSOLVE"
    );
}
