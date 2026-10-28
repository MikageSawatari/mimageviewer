//! Read and mutate a real 0.26.2 index through the application's FTS entry points.
use std::collections::BTreeSet;
use std::path::Path;

use mimageviewer::fts_index::{
    self, Container, FtsIndex, IndexDoc, IndexKind, QueryFilters, SearchTarget, SourceKind,
};
use mimageviewer::ingest_text::PerSourceText;
use mimageviewer::search_query::MatchMode;
use tantivy::TantivyDocument;
use tantivy::collector::TopDocs;
use tantivy::merge_policy::NoMergePolicy;
use tantivy::query::AllQuery;
use tantivy::schema::Value;
use uuid::Uuid;

const SAKURA: &str = "c:/fixture/sakura.jpg";
const RAIN: &str = "c:/fixture/rain.jpg";
const MOUNTAIN: &str = "c:/fixture/mountain.jpg";
const SEA: &str = "c:/fixture/sea.jpg";

fn paths(index: &FtsIndex) -> BTreeSet<String> {
    let searcher = index.searcher();
    let results = searcher
        .search(&AllQuery, &TopDocs::with_limit(20).order_by_score())
        .unwrap();
    let paths: BTreeSet<_> = results
        .into_iter()
        .map(|(_, address)| {
            let doc: TantivyDocument = searcher.doc(address).unwrap();
            doc.get_first(index.fields().path)
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

fn assert_search(index: &FtsIndex, terms: &[&str], expected: &[&str]) {
    let favorite_ids = [Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap()];
    let kinds = [IndexKind::Image];
    let query = fts_index::build_bigram_and_query(
        index.fields(),
        terms,
        &QueryFilters {
            favorite_ids: Some(&favorite_ids),
            kinds: Some(&kinds),
            target: SearchTarget::Only(vec![SourceKind::Filename]),
            mode: MatchMode::And,
        },
    )
    .unwrap();
    let results = fts_index::search_page(&index.searcher(), index.fields(), &query, 0, 20).unwrap();
    let actual: BTreeSet<_> = results.into_iter().map(|(path, _, _)| path).collect();
    assert_eq!(actual, expected.iter().map(|s| s.to_string()).collect());
}

fn assert_original_document(index: &FtsIndex, path: &str, name: &str, place: &str) {
    let searcher = index.searcher();
    let address = fts_index::find_doc_by_path(&searcher, index.fields(), path)
        .unwrap()
        .unwrap();
    let text = fts_index::doc_per_source_text(&searcher, index.fields(), address).unwrap();
    assert_eq!(text.name, name);
    assert_eq!(text.exif, place);
    assert_eq!(text.xmp_tweet, "xmp 東京");
    assert_eq!(text.png_prompt, "blue sky");
    assert_eq!(text.pdf_meta, "pdf 青空");
    assert_eq!(text.video_meta, "video 青空");
    assert_eq!(text.sidecar, "sidecar 青空");
    assert_eq!(text.tags, "tag 青空");
    assert_eq!(
        fts_index::doc_listing_metadata(&searcher, index.fields(), address).unwrap(),
        (100, Some(1024))
    );
}

fn new_doc(path: &str, name: &str) -> IndexDoc {
    IndexDoc {
        path: path.to_owned(),
        container: Container::Fs,
        zip_entry: String::new(),
        favorite_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        kind: IndexKind::Image,
        mtime: 200,
        file_size: 2048,
        norms: PerSourceText {
            name: name.to_owned(),
            ..Default::default()
        },
    }
}

#[test]
fn tantivy_0262_fixture_opens_updates_merges_and_reopens() {
    let original =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fts_tantivy_0262/index");
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("index");
    std::fs::create_dir(&directory).unwrap();
    for entry in std::fs::read_dir(original).unwrap() {
        let entry = entry.unwrap();
        assert!(entry.file_type().unwrap().is_file());
        std::fs::copy(entry.path(), directory.join(entry.file_name())).unwrap();
    }

    let index = FtsIndex::open_at(&directory).unwrap();
    // This also catches drift in the standalone generator's schema.
    let fresh = FtsIndex::open_at(&temporary.path().join("fresh")).unwrap();
    assert_eq!(index.index().schema(), fresh.index().schema());
    assert_eq!(
        paths(&index),
        [SAKURA, RAIN, MOUNTAIN].map(str::to_owned).into()
    );
    assert_search(&index, &["青空"], &[SAKURA, MOUNTAIN]);
    assert_search(&index, &["桜 青"], &[SAKURA]);
    assert_original_document(&index, SAKURA, "桜 青空.jpg", "東京 公園");
    let old_segments = index.index().searchable_segment_ids().unwrap();
    assert_eq!(old_segments.len(), 1);

    let mut writer = index.writer().unwrap();
    writer.set_merge_policy(Box::new(NoMergePolicy));
    fts_index::upsert_doc(&writer, index.fields(), &new_doc(SAKURA, "桜 夕焼け.jpg")).unwrap();
    fts_index::delete_doc(&writer, index.fields(), RAIN);
    fts_index::upsert_doc(&writer, index.fields(), &new_doc(SEA, "海 青空.jpg")).unwrap();
    writer.commit().unwrap();
    index.reload_reader().unwrap();
    assert_search(&index, &["青空"], &[MOUNTAIN, SEA]);
    assert_search(&index, &["夕焼け"], &[SAKURA]);
    assert_eq!(
        paths(&index),
        [SAKURA, MOUNTAIN, SEA].map(str::to_owned).into()
    );
    let segments = index.index().searchable_segment_ids().unwrap();
    assert!(segments.iter().any(|id| old_segments.contains(id)));
    assert!(segments.iter().any(|id| !old_segments.contains(id)));
    writer.merge(&segments).wait().unwrap().unwrap();
    writer.wait_merging_threads().unwrap();
    index.reload_reader().unwrap();
    assert_eq!(index.index().searchable_segment_ids().unwrap().len(), 1);
    drop(index);

    let reopened = FtsIndex::open_at(&directory).unwrap();
    assert_eq!(
        paths(&reopened),
        [SAKURA, MOUNTAIN, SEA].map(str::to_owned).into()
    );
    assert_search(&reopened, &["青空"], &[MOUNTAIN, SEA]);
    assert_search(&reopened, &["夕焼け"], &[SAKURA]);
    assert_search(&reopened, &["夜景"], &[]);
    assert_original_document(&reopened, MOUNTAIN, "山 青空.jpg", "長野 森林");
    let searcher = reopened.searcher();
    let address = fts_index::find_doc_by_path(&searcher, reopened.fields(), SAKURA)
        .unwrap()
        .unwrap();
    let text = fts_index::doc_per_source_text(&searcher, reopened.fields(), address).unwrap();
    assert_eq!(text.name, "桜 夕焼け.jpg");
    assert!(text.exif.is_empty());
    assert_eq!(
        fts_index::doc_listing_metadata(&searcher, reopened.fields(), address).unwrap(),
        (200, Some(2048))
    );
}
