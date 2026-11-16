use crate::transcript_db::{
    FileSignature, RemovedCounts, SaveRequest, TranscriptDb, TranscriptResult,
};
use std::path::Path;

pub(crate) fn save(db: &TranscriptDb, path: &Path, text: Option<&str>) -> String {
    if !path.exists() {
        std::fs::write(path, b"synthetic media").unwrap();
    }
    let result = text.map_or(TranscriptResult::Failed, |text| TranscriptResult::Ok {
        segments: vec![(0, 3_600_000, text.into())],
        removed_counts: RemovedCounts::default(),
        duration_ms: 3_600_000,
        audio_stream: 0,
    });
    assert_eq!(
        db.save(&SaveRequest {
            item_path: path.to_owned(),
            signature: FileSignature::from_metadata(&std::fs::metadata(path).unwrap()).unwrap(),
            model_id: "synthetic-test".into(),
            config_version: 1,
            lang: "ja".into(),
            result,
        })
        .unwrap(),
        crate::transcript_db::SaveOutcome::Written
    );
    TranscriptDb::normalize_path(path)
}

pub(crate) fn candidate(path: &Path) -> crate::search_walker::CandidateFile {
    let meta = std::fs::metadata(path).unwrap();
    let mtime = meta
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    crate::search_walker::CandidateFile {
        abs_path: path.to_owned(),
        key: TranscriptDb::normalize_path(path),
        kind: if path.extension().is_some_and(|extension| extension == "mp4") {
            crate::search_walker::CandidateKind::Video
        } else {
            crate::search_walker::CandidateKind::Audio
        },
        mtime,
        file_size: meta.len() as i64,
        diff_mtime: mtime,
        diff_size: meta.len() as i64,
    }
}

pub(crate) fn stored(
    fts: &crate::fts_index::FtsIndex,
    key: &str,
) -> Option<crate::ingest_text::PerSourceText> {
    let searcher = fts.searcher();
    crate::fts_index::find_doc_by_path(&searcher, fts.fields(), key)
        .unwrap()
        .map(|address| {
            crate::fts_index::doc_per_source_text(&searcher, fts.fields(), address).unwrap()
        })
}
