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
        source: source.to_string(),
        hosts: 2,
        voices: vec!["Alice".into(), "Frank".into()],
        device: Some("xpu".into()),
        model: Some("claude-sonnet-4-5".into()),
        started: chrono::Local::now(),
        finished: Some(chrono::Local::now()),
        elapsed_secs: Some(612),
        outcome: "completed".into(),
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
    assert_eq!(ep.title(), "https://x/rotbigs", "the sidecar knows what the user actually typed");
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
