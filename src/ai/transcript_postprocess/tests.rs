use super::*;

#[derive(serde::Deserialize)]
struct Fixture {
    cases: Vec<Case>,
    loops: Vec<(String, String)>,
}
#[derive(serde::Deserialize)]
struct Case {
    name: String,
    input: Vec<TranscriptSegment>,
    kept: Vec<TranscriptSegment>,
    removed_counts: RemovedCounts,
}

#[test]
fn matches_unmodified_python_clean_none_fixtures() {
    assert_fixture(include_str!("fixtures.json"));
}

#[test]
fn matches_python_on_saved_public_evaluation_segment_sequences() {
    assert_fixture(include_str!("evaluation_fixtures.json"));
}

fn assert_fixture(json: &str) {
    let fixture: Fixture = serde_json::from_str(json).unwrap();
    for case in fixture.cases {
        let (kept, removed) = clean(&case.input);
        assert_eq!(kept, case.kept, "{}: text/times", case.name);
        assert_eq!(removed, case.removed_counts, "{}: removals", case.name);
    }
    for (before, expected) in fixture.loops {
        assert_eq!(shorten_loops(&before), expected, "{before:?}");
    }
}

#[test]
fn normalization_matches_python_categories_and_whole_string_lowercase() {
    assert_eq!(normalize(" ＡＢＣ！？　🎵\u{200b}\u{1c} カナ"), "abcカナ");
    assert_eq!(normalize("ΟΔΟΣ"), "οδος");
    assert_eq!(normalize("İ"), "i\u{307}");
    // Unicode marks and unassigned characters are kept, exactly as in Python.
    assert_eq!(normalize("e\u{301}\u{378}"), "é\u{378}");
}
