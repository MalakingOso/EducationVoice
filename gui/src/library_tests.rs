//! Tests for [`crate::library`], kept out of `library.rs` so that file
//! stays under the 500-line cap. Included with `#[path]`, so `super::*`
//! still reaches the module's private items.

use super::*;

/// A private root per test, so a failing test cannot take another's files
/// with it and nothing ever points at the user's real `output/`.
fn temp_paths(tag: &str) -> Paths {
    let root = std::env::temp_dir().join(format!("a2p_library_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("output")).unwrap();
    Paths {
        python: root.join(".venv/bin/python"),
        script: root.join("article2pod.py"),
        output_dir: root.join("output"),
        scripts_dir: root.join("scripts"),
        root,
    }
}

fn cleanup(paths: &Paths) {
    let _ = std::fs::remove_dir_all(&paths.root);
}

fn touch(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, b"x").unwrap();
    path
}

fn meta_fixture(source: &str) -> RunMeta {
    RunMeta {
        title: None,
        source: source.to_string(),
        hosts: 2,
        voices: vec!["Alice".into(), "Frank".into()],
        device: Some("xpu".into()),
        model: Some("claude-sonnet-4-5".into()),
        research_model: None,
        started: Some(chrono::Local::now()),
        finished: Some(chrono::Local::now()),
        elapsed_secs: Some(612),
        outcome: "completed".into(),
        spotify_episode_uri: None,
        spotify_status: None,
    }
}

fn dated(stem: &str, modified: Option<SystemTime>) -> Episode {
    Episode { stem: stem.into(), script: None, audio: None, meta: None, modified }
}

fn find<'a>(eps: &'a [Episode], stem: &str) -> &'a Episode {
    eps.iter().find(|e| e.stem == stem).unwrap_or_else(|| panic!("no episode {stem:?}"))
}

#[test]
fn one_stems_script_audio_and_sidecar_become_a_single_episode() {
    let paths = temp_paths("pair");
    touch(&paths.output_dir, "rotbigs.script.txt");
    touch(&paths.output_dir, "rotbigs.wav");
    write_meta(&meta_path(&paths, "rotbigs"), &meta_fixture("https://x/rotbigs")).unwrap();
    touch(&paths.output_dir, "run.log");

    let eps = scan(&paths).unwrap();
    assert_eq!(eps.len(), 1, "three artefacts of one run are one row, and run.log is not an artefact");
    let ep = &eps[0];
    assert!(ep.script.is_some() && ep.audio.is_some(), "both files belong to the stem");
    assert!(ep.is_playable(), "there is audio on disk, so the row offers Play");
    assert_eq!(
        ep.title(),
        "rotbigs",
        "with no recorded title the row falls back to the source's last \
         component; the whole URL would not fit and reads as machinery"
    );
    cleanup(&paths);
}

#[test]
fn a_curated_script_outranks_the_output_copy_of_the_same_stem() {
    let paths = temp_paths("curated");
    touch(&paths.output_dir, "rotbigs.script.txt");
    let curated = touch(&paths.scripts_dir, "rotbigs.txt");

    let eps = scan(&paths).unwrap();
    assert_eq!(eps.len(), 1, "the same stem in both directories is still one episode");
    assert_eq!(
        eps[0].script.as_deref(),
        Some(curated.as_path()),
        "scripts/ is the hand-kept copy; output/ is whatever the last run happened to write"
    );
    cleanup(&paths);
}

#[test]
fn a_missing_output_or_scripts_directory_yields_an_empty_library_not_an_error() {
    let paths = temp_paths("missing");
    std::fs::remove_dir_all(&paths.output_dir).unwrap();

    let eps = scan(&paths).expect("a fresh checkout has neither directory and must still open");
    assert!(eps.is_empty(), "nothing on disk means no rows, not a failure");
    cleanup(&paths);
}

#[test]
fn a_stem_with_both_wav_and_mp3_keeps_the_mp3() {
    let paths = temp_paths("mp3");
    touch(&paths.output_dir, "rotbigs2.wav");
    let mp3 = touch(&paths.output_dir, "rotbigs2.mp3");

    let eps = scan(&paths).unwrap();
    assert_eq!(
        eps[0].audio.as_deref(),
        Some(mp3.as_path()),
        "the WAV survives only when conversion failed, so the MP3 is the real output"
    );
    cleanup(&paths);
}

#[test]
fn the_legacy_script_txt_is_its_own_row_and_never_shadows_a_real_episode() {
    let paths = temp_paths("legacy");
    touch(&paths.output_dir, LEGACY_SCRIPT);
    let real = touch(&paths.output_dir, "script.script.txt");

    let eps = scan(&paths).unwrap();
    assert_eq!(eps.len(), 1, "both files describe the stem \"script\"");
    assert_eq!(
        find(&eps, LEGACY_STEM).script.as_deref(),
        Some(real.as_path()),
        "an episode genuinely stemmed \"script\" outranks the pre-GUI leftover regardless of read_dir order"
    );
    cleanup(&paths);
}

#[test]
fn episodes_sort_newest_first_with_undated_ones_last() {
    let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
    let new = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(9_000);
    let mut eps = vec![dated("undated", None), dated("old", Some(old)), dated("new", Some(new))];

    sort_newest_first(&mut eps);

    let order: Vec<&str> = eps.iter().map(|e| e.stem.as_str()).collect();
    assert_eq!(
        order,
        vec!["new", "old", "undated"],
        "a row whose age is unknown must not outrank one known to be recent"
    );
}

#[test]
fn a_written_sidecar_reads_back_unchanged_and_leaves_no_temp_file() {
    let paths = temp_paths("roundtrip");
    let path = meta_path(&paths, "rotbigs");
    let meta = meta_fixture("/home/berkley/ROTBIGS.pdf");

    write_meta(&path, &meta).unwrap();

    assert_eq!(read_meta(&path), Some(meta), "every field survives the JSON round trip");
    let leftovers: Vec<_> = std::fs::read_dir(&paths.output_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "the temp file is renamed onto the target, never left behind");
    cleanup(&paths);
}

#[test]
fn an_unparseable_sidecar_is_preserved_beside_itself_and_reads_as_none() {
    let paths = temp_paths("corrupt");
    let path = touch(&paths.output_dir, "rotbigs.run.json");
    std::fs::write(&path, b"{ truncated mid-wr").unwrap();

    assert_eq!(read_meta(&path), None, "unparseable is indistinguishable from absent to a caller");
    assert!(!path.exists(), "it must not stay where the next write_meta would overwrite it");
    assert_eq!(
        std::fs::read_to_string(paths.output_dir.join("rotbigs.run.json.corrupt")).unwrap(),
        "{ truncated mid-wr",
        "the bytes are the only record of what went wrong"
    );
    cleanup(&paths);
}

#[test]
fn a_second_corruption_is_preserved_without_clobbering_the_first() {
    let paths = temp_paths("corrupt2");
    let path = paths.output_dir.join("rotbigs.run.json");
    std::fs::write(&path, b"first failure").unwrap();
    read_meta(&path);
    std::fs::write(&path, b"second failure").unwrap();
    read_meta(&path);

    assert_eq!(
        std::fs::read_to_string(paths.output_dir.join("rotbigs.run.json.corrupt")).unwrap(),
        "first failure",
        "the earliest capture is the one taken closest to whatever broke the file"
    );
    assert_eq!(
        std::fs::read_to_string(paths.output_dir.join("rotbigs.run.json.corrupt.2")).unwrap(),
        "second failure",
        "later failures are numbered rather than dropped"
    );
    cleanup(&paths);
}

#[test]
fn delete_removes_every_artefact_of_a_stem_and_counts_them() {
    let paths = temp_paths("delete");
    touch(&paths.output_dir, "rotbigs.script.txt");
    touch(&paths.output_dir, "rotbigs.mp3");
    touch(&paths.output_dir, "rotbigs.run.json");
    touch(&paths.scripts_dir, "rotbigs.txt");
    let bystander = touch(&paths.output_dir, "other.mp3");

    assert_eq!(delete(&paths, "rotbigs").unwrap(), 4, "the count is what the UI reports back");
    assert!(scan(&paths).unwrap().iter().all(|e| e.stem != "rotbigs"), "the row is gone from the Library");
    assert!(bystander.exists(), "a stem match is exact; another episode is untouched");
    cleanup(&paths);
}

#[test]
fn delete_refuses_a_stem_that_could_escape_the_directories_it_owns() {
    let paths = temp_paths("traversal");
    let outside = touch(&paths.root, "precious.wav");

    for stem in ["../precious", "..", "sub/rotbigs", ""] {
        assert!(
            delete(&paths, stem).is_err(),
            "{stem:?} is not a bare filename and must be rejected before any unlink"
        );
    }
    assert!(outside.exists(), "nothing above output/ was touched");
    cleanup(&paths);
}

// ── Titles ───────────────────────────────────────────────────────────────

fn episode_with(meta: Option<RunMeta>) -> Episode {
    Episode {
        stem: "rotbigs".to_string(),
        script: None,
        audio: None,
        meta,
        modified: None,
    }
}

#[test]
fn a_recorded_title_wins_over_everything_below_it() {
    let mut meta = meta_fixture("https://example.com/paper.html");
    meta.title = Some("Reversal of Thromboprophylaxis in Bariatric Surgery".into());
    assert_eq!(
        episode_with(Some(meta)).title(),
        "Reversal of Thromboprophylaxis in Bariatric Surgery"
    );
}

#[test]
fn a_blank_recorded_title_falls_through_rather_than_naming_the_row_nothing() {
    let mut meta = meta_fixture("https://example.com/paper.html");
    meta.title = Some("   ".into());
    assert_eq!(
        episode_with(Some(meta)).title(),
        "paper.html",
        "an empty string is not a name, and a row with no visible label \
         cannot be clicked to rename"
    );
}

#[test]
fn the_source_falls_back_to_its_last_component_for_urls_and_for_paths() {
    for (source, want) in [
        ("https://example.com/journals/paper.html", "paper.html"),
        ("https://example.com/journals/paper.html?utm=x", "paper.html"),
        ("/home/berkley/ROTBIGS.pdf", "ROTBIGS.pdf"),
        // A bare host has no path; the host is the only thing left to say.
        ("https://example.com", "example.com"),
        // A trailing slash must not yield an empty label.
        ("https://example.com/journals/", "journals"),
    ] {
        assert_eq!(
            episode_with(Some(meta_fixture(source))).title(),
            want,
            "{source} should label its row {want}"
        );
    }
}

#[test]
fn stdin_and_a_missing_sidecar_both_fall_all_the_way_to_the_stem() {
    assert_eq!(
        episode_with(Some(meta_fixture("-"))).title(),
        "rotbigs",
        "the stdin sentinel names nothing a reader could recognise"
    );
    assert_eq!(
        episode_with(None).title(),
        "rotbigs",
        "an episode made before the sidecar existed still has a filename"
    );
}

#[test]
fn a_rename_survives_a_rescan_and_needs_no_prior_sidecar() {
    let paths = temp_paths("rename");
    touch(&paths.output_dir, "rotbigs.script.txt");

    rename(&paths, "rotbigs", "The Bariatric Paper").unwrap();
    let eps = scan(&paths).unwrap();
    assert_eq!(eps[0].title(), "The Bariatric Paper");

    let meta = eps[0].meta.as_ref().expect("the rename wrote a sidecar");
    assert_eq!(
        meta.hosts, 0,
        "a title-only sidecar must not invent a host count it never measured"
    );
    assert!(
        meta.started.is_none(),
        "nor a start time, which would file a year-old episode under Today"
    );
    cleanup(&paths);
}

#[test]
fn a_rename_leaves_the_rest_of_an_existing_sidecar_alone() {
    let paths = temp_paths("rename_keep");
    touch(&paths.output_dir, "rotbigs.script.txt");
    let original = meta_fixture("https://x/rotbigs");
    write_meta(&meta_path(&paths, "rotbigs"), &original).unwrap();

    rename(&paths, "rotbigs", "A Better Name").unwrap();
    let after = read_meta(&meta_path(&paths, "rotbigs")).expect("still parses");
    assert_eq!(after.title.as_deref(), Some("A Better Name"));
    assert_eq!(after.hosts, original.hosts, "the run record is not the label");
    assert_eq!(after.voices, original.voices);
    assert_eq!(after.started, original.started);
    cleanup(&paths);
}

#[test]
fn a_rename_refuses_a_stem_that_could_escape_the_output_directory() {
    let paths = temp_paths("rename_escape");
    assert!(
        rename(&paths, "../../etc/passwd", "nope").is_err(),
        "a stem reaches rename from a deserialized sidecar as well as from a \
         filename, so it is not trusted here any more than in delete"
    );
    cleanup(&paths);
}

#[test]
fn clearing_a_name_returns_the_row_to_its_fallback() {
    let paths = temp_paths("rename_clear");
    touch(&paths.output_dir, "rotbigs.script.txt");
    rename(&paths, "rotbigs", "Temporary").unwrap();
    rename(&paths, "rotbigs", "  ").unwrap();
    assert_eq!(
        scan(&paths).unwrap()[0].title(),
        "rotbigs",
        "emptying the field means \"no title\", not a title that is blank"
    );
    cleanup(&paths);
}

// ── Search ───────────────────────────────────────────────────────────────

#[test]
fn search_matches_the_label_and_the_source_and_ignores_case() {
    let mut meta = meta_fixture("https://example.com/thrombo.html");
    meta.title = Some("Reversal of Thromboprophylaxis".into());
    let ep = episode_with(Some(meta));

    assert!(ep.matches("reversal"), "the label, case-insensitively");
    assert!(ep.matches("EXAMPLE.COM"), "the source, case-insensitively");
    assert!(ep.matches("  "), "an empty search matches everything");
    assert!(!ep.matches("cholecystectomy"));
}

// ── Day grouping ─────────────────────────────────────────────────────────

/// An episode that started `days` before now, so the boundary tests do not
/// depend on what time of day they run.
fn episode_on(stem: &str, days: i64) -> Episode {
    let mut meta = meta_fixture("https://x/paper");
    meta.started = Some(chrono::Local::now() - chrono::Duration::days(days));
    Episode {
        stem: stem.to_string(),
        script: None,
        audio: None,
        meta: Some(meta),
        modified: None,
    }
}

#[test]
fn today_and_yesterday_are_named_and_older_days_are_dated() {
    let groups = group_by_day(&[episode_on("a", 0), episode_on("b", 1), episode_on("c", 9)]);
    let labels: Vec<&str> = groups.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels[0], "Today");
    assert_eq!(labels[1], "Yesterday");
    assert!(
        labels[2].contains(char::is_numeric) && labels[2] != "Today",
        "anything older is dated outright, got {:?}",
        labels[2]
    );
}

#[test]
fn an_episode_with_no_timestamp_sorts_last_rather_than_into_today() {
    // Beamer's grouped_by_day resolves an unparseable timestamp to today.
    // Copying that here would sweep every pre-GUI episode into today's group,
    // which is the one thing this grouping must not do.
    let groups = group_by_day(&[episode_with(None), episode_on("a", 0)]);
    assert_eq!(groups.first().map(|(l, _)| l.as_str()), Some("Today"));
    assert_eq!(
        groups.last().map(|(l, _)| l.as_str()),
        Some("No run record"),
        "an undated episode belongs at the bottom, not at the top"
    );
}

#[test]
fn the_order_within_a_group_is_the_order_it_was_given() {
    // scan() already sorts newest-first; grouping must carry that through
    // rather than re-sorting on a field it does not have.
    let groups = group_by_day(&[episode_on("first", 0), episode_on("second", 0)]);
    assert_eq!(groups.len(), 1, "same day, one group");
    let stems: Vec<&str> = groups[0].1.iter().map(|e| e.stem.as_str()).collect();
    assert_eq!(stems, vec!["first", "second"]);
}

#[test]
fn every_episode_given_lands_in_exactly_one_group() {
    let eps = vec![
        episode_on("a", 0),
        episode_on("b", 0),
        episode_on("c", 1),
        episode_with(None),
    ];
    let total: usize = group_by_day(&eps).iter().map(|(_, e)| e.len()).sum();
    assert_eq!(total, eps.len(), "grouping must not drop or duplicate a row");
}

// ── Carrying a sidecar forward ───────────────────────────────────────────

#[test]
fn stage_two_keeps_the_title_stage_one_fetched() {
    // The default flow, and the one that would otherwise lose the feature at
    // the very end: Script writes a sidecar carrying the article's title, the
    // user passes the gate, and Synth — same stem, same episode — finishes and
    // replaces the whole sidecar. Synthesis reads a script off disk and has no
    // article to ask about, so it never emits a title of its own.
    let mut stage_one = meta_fixture("https://x/rotbigs");
    stage_one.title = Some("Reversal of Thromboprophylaxis".into());

    let mut stage_two = meta_fixture("https://x/rotbigs");
    stage_two.title = None;
    stage_two.device = Some("xpu".into());
    stage_two.elapsed_secs = Some(554);

    let merged = stage_two.carrying_forward(Some(&stage_one));
    assert_eq!(
        merged.title.as_deref(),
        Some("Reversal of Thromboprophylaxis"),
        "the finished episode must still be named after its article"
    );
    assert_eq!(
        merged.elapsed_secs,
        Some(554),
        "what this run actually measured still wins"
    );
    assert_eq!(merged.device.as_deref(), Some("xpu"));
}

#[test]
fn a_rerun_that_found_a_better_title_replaces_the_old_one() {
    let mut previous = meta_fixture("https://x/rotbigs");
    previous.title = Some("Untitled Document".into());
    let mut fresh = meta_fixture("https://x/rotbigs");
    fresh.title = Some("Reversal of Thromboprophylaxis".into());

    assert_eq!(
        fresh.carrying_forward(Some(&previous)).title.as_deref(),
        Some("Reversal of Thromboprophylaxis"),
        "carrying forward fills gaps; it does not pin the first answer"
    );
}

#[test]
fn re_voicing_from_the_library_keeps_the_source_it_never_knew() {
    // That button hands over a script path and nothing else — the Run page's
    // source field may be empty, or may name a completely different article.
    let previous = meta_fixture("https://x/rotbigs");
    let mut fresh = meta_fixture("");
    fresh.source = String::new();

    assert_eq!(
        fresh.carrying_forward(Some(&previous)).source,
        "https://x/rotbigs",
        "an empty source means 'this run did not know', not 'there is none'"
    );
}

#[test]
fn a_first_run_has_nothing_to_carry_and_is_left_exactly_as_it_is() {
    let mut fresh = meta_fixture("https://x/rotbigs");
    fresh.title = None;
    assert_eq!(fresh.clone().carrying_forward(None), fresh);
}

#[test]
fn re_voicing_an_already_sent_episode_does_not_unpublish_it() {
    // A re-voice writes a brand-new sidecar with no Spotify fields of its
    // own; without carrying them forward the episode would look unsent the
    // moment its script was regenerated, even though the old audio is still
    // live on Spotify.
    let mut previous = meta_fixture("https://x/rotbigs");
    previous.spotify_episode_uri = Some("spotify:episode:abc123".into());
    previous.spotify_status = Some("ready".into());

    let fresh = meta_fixture("https://x/rotbigs");
    let merged = fresh.carrying_forward(Some(&previous));

    assert_eq!(merged.spotify_episode_uri.as_deref(), Some("spotify:episode:abc123"));
    assert_eq!(merged.spotify_status.as_deref(), Some("ready"));
}
