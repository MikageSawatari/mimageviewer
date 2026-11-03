//! Read/update/merge the real 0.26.2 binary fixture through raw Tantivy APIs.
//! The app now requires a caption field absent from this historical schema;
//! its schema-rebuild policy is tested separately below.
use std::collections::BTreeSet;
use std::path::Path;

use mimageviewer::fts_index::FtsIndex;
use tantivy::collector::TopDocs;
use tantivy::merge_policy::NoMergePolicy;
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, Value};
use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer};
use tantivy::{DocAddress, Index, IndexReader, IndexWriter, TantivyDocument, Term, doc};

/// Keep the historical schema intact while exercising the upgraded binary reader/writer.
struct LegacyIndex {
    index: Index,
    reader: IndexReader,
}

impl LegacyIndex {
    fn open_at(path: &Path) -> Self {
        let index = Index::open_in_dir(path).unwrap();
        index.tokenizers().register(
            "mimv_bigram",
            TextAnalyzer::builder(NgramTokenizer::new(2, 2, false).unwrap())
                .filter(LowerCaser)
                .build(),
        );
        let reader = index.reader().unwrap();
        Self { index, reader }
    }

    fn field(&self, name: &str) -> Field {
        self.index.schema().get_field(name).unwrap()
    }

    fn document(&self, path: &str) -> TantivyDocument {
        let query = TermQuery::new(
            Term::from_field_text(self.field("path"), path),
            IndexRecordOption::Basic,
        );
        let searcher = self.reader.searcher();
        let matches = searcher
            .search(&query, &TopDocs::with_limit(1).order_by_score())
            .unwrap();
        assert_eq!(matches.len(), 1);
        searcher.doc(matches[0].1).unwrap()
    }
}

fn copy_fixture(destination: &Path) {
    let original =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fts_tantivy_0262/index");
    std::fs::create_dir(destination).unwrap();
    for entry in std::fs::read_dir(original).unwrap() {
        let entry = entry.unwrap();
        assert!(entry.file_type().unwrap().is_file());
        std::fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
    }
}

const SAKURA: &str = "c:/fixture/sakura.jpg";
const RAIN: &str = "c:/fixture/rain.jpg";
const MOUNTAIN: &str = "c:/fixture/mountain.jpg";
const SEA: &str = "c:/fixture/sea.jpg";

fn paths(index: &LegacyIndex) -> BTreeSet<String> {
    let searcher = index.reader.searcher();
    let results = searcher
        .search(&AllQuery, &TopDocs::with_limit(20).order_by_score())
        .unwrap();
    let paths: BTreeSet<_> = results
        .into_iter()
        .map(|(_, address)| {
            let doc: TantivyDocument = searcher.doc(address).unwrap();
            doc.get_first(index.field("path"))
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        paths.len() as u64,
        searcher.num_docs(),
        "duplicate live paths"
    );
    paths
}

fn assert_search(index: &LegacyIndex, terms: &[&str], expected: &[&str]) {
    let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    for term in terms {
        let chars: Vec<_> = term.chars().collect();
        for gram in chars.windows(2) {
            let text: String = gram.iter().collect();
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(index.field("name"), &text),
                    IndexRecordOption::Basic,
                )),
            ));
        }
    }
    for (field, value) in [
        ("favorite_id", "11111111-1111-4111-8111-111111111111"),
        ("kind", "image"),
    ] {
        clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_text(index.field(field), value),
                IndexRecordOption::Basic,
            )),
        ));
    }
    let searcher = index.reader.searcher();
    let results: Vec<(f32, DocAddress)> = searcher
        .search(
            &BooleanQuery::from(clauses),
            &TopDocs::with_limit(20).order_by_score(),
        )
        .unwrap();
    let actual: BTreeSet<_> = results
        .into_iter()
        .map(|(_, address)| {
            let document: TantivyDocument = searcher.doc(address).unwrap();
            document
                .get_first(index.field("path"))
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(actual, expected.iter().map(|s| s.to_string()).collect());
}

fn assert_original_document(index: &LegacyIndex, path: &str, name: &str, place: &str) {
    let document = index.document(path);
    for (field, expected) in [
        ("name", name),
        ("exif_text", place),
        ("xmp_tweet_text", "xmp 東京"),
        ("png_prompt_text", "blue sky"),
        ("pdf_meta_text", "pdf 青空"),
        ("video_meta_text", "video 青空"),
        ("sidecar_text", "sidecar 青空"),
        ("tags", "tag 青空"),
    ] {
        assert_eq!(
            document.get_first(index.field(field)).unwrap().as_str(),
            Some(expected)
        );
    }
    assert_eq!(
        document.get_first(index.field("mtime")).unwrap().as_i64(),
        Some(100)
    );
    assert_eq!(
        document
            .get_first(index.field("file_size"))
            .unwrap()
            .as_i64(),
        Some(1024)
    );
}

fn upsert(index: &LegacyIndex, writer: &IndexWriter, path: &str, name: &str) {
    writer.delete_term(Term::from_field_text(index.field("path"), path));
    writer
        .add_document(doc!(
            index.field("path") => path,
            index.field("container") => "fs",
            index.field("zip_entry") => "",
            index.field("favorite_id") => "11111111-1111-4111-8111-111111111111",
            index.field("kind") => "image",
            index.field("mtime") => 200i64,
            index.field("file_size") => 2048i64,
            index.field("name") => name,
        ))
        .unwrap();
}

#[test]
fn tantivy_0262_fixture_opens_updates_merges_and_reopens() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("index");
    copy_fixture(&directory);
    let index = LegacyIndex::open_at(&directory);
    assert!(index.index.schema().get_field("ai_caption_text").is_err());
    assert_eq!(
        paths(&index),
        [SAKURA, RAIN, MOUNTAIN].map(str::to_owned).into()
    );
    assert_search(&index, &["青空"], &[SAKURA, MOUNTAIN]);
    assert_search(&index, &["桜 青"], &[SAKURA]);
    assert_original_document(&index, SAKURA, "桜 青空.jpg", "東京 公園");
    let old_segments = index.index.searchable_segment_ids().unwrap();
    assert_eq!(old_segments.len(), 1);

    let mut writer = index.index.writer::<TantivyDocument>(64_000_000).unwrap();
    writer.set_merge_policy(Box::new(NoMergePolicy));
    upsert(&index, &writer, SAKURA, "桜 夕焼け.jpg");
    writer.delete_term(Term::from_field_text(index.field("path"), RAIN));
    upsert(&index, &writer, SEA, "海 青空.jpg");
    writer.commit().unwrap();
    index.reader.reload().unwrap();
    assert_search(&index, &["青空"], &[MOUNTAIN, SEA]);
    assert_search(&index, &["夕焼け"], &[SAKURA]);
    assert_eq!(
        paths(&index),
        [SAKURA, MOUNTAIN, SEA].map(str::to_owned).into()
    );
    let segments = index.index.searchable_segment_ids().unwrap();
    assert!(segments.iter().any(|id| old_segments.contains(id)));
    assert!(segments.iter().any(|id| !old_segments.contains(id)));
    writer.merge(&segments).wait().unwrap().unwrap();
    writer.wait_merging_threads().unwrap();
    index.reader.reload().unwrap();
    assert_eq!(index.index.searchable_segment_ids().unwrap().len(), 1);
    drop(index);

    let reopened = LegacyIndex::open_at(&directory);
    assert_eq!(
        paths(&reopened),
        [SAKURA, MOUNTAIN, SEA].map(str::to_owned).into()
    );
    assert_search(&reopened, &["青空"], &[MOUNTAIN, SEA]);
    assert_search(&reopened, &["夕焼け"], &[SAKURA]);
    assert_search(&reopened, &["夜景"], &[]);
    assert_original_document(&reopened, MOUNTAIN, "山 青空.jpg", "長野 森林");
    let document = reopened.document(SAKURA);
    assert_eq!(
        document.get_first(reopened.field("name")).unwrap().as_str(),
        Some("桜 夕焼け.jpg")
    );
    assert!(document.get_first(reopened.field("exif_text")).is_none());
    assert_eq!(
        document
            .get_first(reopened.field("mtime"))
            .unwrap()
            .as_i64(),
        Some(200)
    );
    assert_eq!(
        document
            .get_first(reopened.field("file_size"))
            .unwrap()
            .as_i64(),
        Some(2048)
    );
}

#[test]
fn application_rebuilds_historical_fixture_without_caption_schema() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("fts_index");
    let meta =
        mimageviewer::fts_meta::FtsMetaDb::open_at(&temporary.path().join("fts_meta.db")).unwrap();
    meta.upsert_meta_ok(
        SAKURA,
        uuid::Uuid::nil(),
        Path::new("c:/fixture"),
        mimageviewer::fts_index::IndexKind::Image,
        100,
        1024,
    )
    .unwrap();
    drop(meta);
    copy_fixture(&directory);
    // The standalone opener must not silently erase an old index behind its inventory.
    assert!(matches!(
        FtsIndex::open_at(&directory),
        Err(tantivy::TantivyError::InvalidArgument(_))
    ));
    {
        let original = LegacyIndex::open_at(&directory);
        assert_eq!(original.reader.searcher().num_docs(), 3);
    }
    let manager = mimageviewer::indexer_manager::IndexerManager::new_at(
        temporary.path(),
        &[],
        mimageviewer::settings::IndexerSpeedProfile::default(),
        std::sync::Arc::new(mimageviewer::activity_gate::ActivityGate::new(0)),
        vec![],
    )
    .unwrap();
    let index = manager.clone_fts_index();
    assert_eq!(index.searcher().num_docs(), 0);
    assert!(index.index().schema().get_field("ai_caption_text").is_ok());
    let meta = manager.clone_fts_meta();
    assert!(meta.get(SAKURA).unwrap().is_none());
}
