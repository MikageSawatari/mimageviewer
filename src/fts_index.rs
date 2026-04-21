//! Tantivy ベースの全文検索インデックス (docs/search-expansion-design.md §5.2)。
//!
//! ## 役割
//!
//! - bigram tokenizer (`NgramTokenizer(2, 2)` + `lower_caser`) で画像メタを転置索引化
//! - `fts_meta.db` と二段整合性を組み、Ctrl+G の候補絞り込みに使う
//! - post-filter 用の正規化済み原文 (`all_text_norm`) は **Tantivy に持たせない**。
//!   §5.2 採用案どおり `fts_meta.db` に一元化 (Codex 2 回目指摘 #2)
//!
//! ## スキーマ (v1)
//!
//! ```text
//! path         STRING | STORED    完全一致キー、正規化済み
//! container    STRING | STORED    "fs" / "zip"
//! zip_entry    STRING | STORED    container="zip" のとき ZIP 内相対パス
//! favorite_id  STRING | STORED    UUID (exact term filter 用)
//! mtime        i64    INDEXED | STORED
//! file_size    i64    STORED
//! name         TEXT               ファイル名 (bigram + lower_caser)
//! all_text     TEXT               name + 全メタを連結 (bigram + lower_caser)
//! ```
//!
//! ## 検索の組み立て方
//!
//! クエリ文字列を `normalize_for_match` → bigram 分解 → 各 bigram を `TermQuery` にして
//! AND の `BooleanQuery` を作る。phrase/NOT/AND の最終判定は post-filter で行う (§4.3)。
//!
//! ## Searcher snapshot 固定 (§9.1 ステップ 4, Codex 3 回目指摘 #2)
//!
//! 検索ワーカーは `FtsIndex::searcher()` を 1 回だけ取得し、ページング中はそれを使い回す。
//! これで ingest 側が commit して reader reload しても、検索中の snapshot はズレない。

use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, INDEXED, STORED,
    STRING,
};
use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer, Token, TokenStream, Tokenizer};
use tantivy::{doc, DocAddress, Index, IndexReader, IndexWriter, Score, TantivyDocument, Term};
use uuid::Uuid;

const BIGRAM_TOKENIZER_NAME: &str = "mimv_bigram";
const WRITER_HEAP_MB: usize = 64;
/// 検索時のページサイズ (Codex 3 回目 #6 worst case 計測で確定)
pub const PAGE_SIZE: usize = 500;

/// Tantivy ドキュメント 1 件を構築するための入力。
#[derive(Debug, Clone)]
pub struct IndexDoc {
    /// 正規化済みキー (fts_meta.db と同じ path)
    pub path: String,
    pub container: Container,
    /// container=Zip のとき、ZIP 内相対パス ("" なら Fs)
    pub zip_entry: String,
    pub favorite_id: Uuid,
    pub mtime: i64,
    pub file_size: i64,
    pub name: String,
    /// `search_norm::normalize_for_match` 適用済みのテキスト
    pub all_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Fs,
    Zip,
}
impl Container {
    pub fn as_str(self) -> &'static str {
        match self {
            Container::Fs => "fs",
            Container::Zip => "zip",
        }
    }
}

/// Tantivy index のフィールドハンドル集合。
pub struct Fields {
    pub path: Field,
    pub container: Field,
    pub zip_entry: Field,
    pub favorite_id: Field,
    pub mtime: Field,
    pub file_size: Field,
    pub name: Field,
    pub all_text: Field,
}

impl Fields {
    fn from_schema(schema: &Schema) -> Self {
        Self {
            path: schema.get_field("path").expect("schema: path"),
            container: schema.get_field("container").expect("schema: container"),
            zip_entry: schema.get_field("zip_entry").expect("schema: zip_entry"),
            favorite_id: schema.get_field("favorite_id").expect("schema: favorite_id"),
            mtime: schema.get_field("mtime").expect("schema: mtime"),
            file_size: schema.get_field("file_size").expect("schema: file_size"),
            name: schema.get_field("name").expect("schema: name"),
            all_text: schema.get_field("all_text").expect("schema: all_text"),
        }
    }
}

/// Tantivy index のラッパー。`%APPDATA%/mimageviewer/fts_index/` を管理する。
pub struct FtsIndex {
    index: Index,
    reader: IndexReader,
    fields: Fields,
}

impl FtsIndex {
    /// 既定パス (`%APPDATA%/mimageviewer/fts_index/`) で開く (なければ作成)。
    pub fn open_default() -> tantivy::Result<Self> {
        let dir = crate::data_dir::get().join("fts_index");
        Self::open_at(&dir)
    }

    /// 任意ディレクトリで開く (テスト用)。
    pub fn open_at(dir: &Path) -> tantivy::Result<Self> {
        std::fs::create_dir_all(dir).ok();
        let schema = build_schema();
        let index = if Index::exists(&tantivy::directory::MmapDirectory::open(dir)?)? {
            Index::open_in_dir(dir)?
        } else {
            Index::create_in_dir(dir, schema.clone())?
        };
        register_tokenizer(&index);
        let reader = index
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        let fields = Fields::from_schema(&index.schema());
        Ok(Self {
            index,
            reader,
            fields,
        })
    }

    pub fn fields(&self) -> &Fields {
        &self.fields
    }

    pub fn index(&self) -> &Index {
        &self.index
    }

    /// 新しい `IndexWriter` を確保する (heap 64MB)。ingest worker が保有・1 本に固定する。
    pub fn writer(&self) -> tantivy::Result<IndexWriter> {
        self.index.writer(WRITER_HEAP_MB * 1024 * 1024)
    }

    /// Searcher を取得 (§9.1 ステップ 4 の snapshot 固定用)。
    /// ワーカーは 1 クエリ中ずっと同じ Searcher を使い続ける。
    pub fn searcher(&self) -> tantivy::Searcher {
        self.reader.searcher()
    }

    /// reader を明示リロード (ingest commit 後に呼ぶ)。
    pub fn reload_reader(&self) -> tantivy::Result<()> {
        self.reader.reload()
    }
}

/// `IndexWriter` に 1 doc を追加する。`delete + add` パターンで更新も兼ねる (§5.6.1)。
pub fn upsert_doc(
    writer: &IndexWriter,
    fields: &Fields,
    d: &IndexDoc,
) -> tantivy::Result<()> {
    // 既存 doc を delete してから add (Tantivy の更新は delete + reinsert が基本)
    writer.delete_term(Term::from_field_text(fields.path, &d.path));
    writer.add_document(doc!(
        fields.path        => d.path.as_str(),
        fields.container   => d.container.as_str(),
        fields.zip_entry   => d.zip_entry.as_str(),
        fields.favorite_id => d.favorite_id.to_string().as_str(),
        fields.mtime       => d.mtime,
        fields.file_size   => d.file_size,
        fields.name        => d.name.as_str(),
        fields.all_text    => d.all_text.as_str(),
    ))?;
    Ok(())
}

/// 指定 path の doc を Tantivy から削除 (§5.6.2)。
pub fn delete_doc(writer: &IndexWriter, fields: &Fields, path: &str) {
    writer.delete_term(Term::from_field_text(fields.path, path));
}

/// クエリ文字列を bigram に分解して AND の BooleanQuery を作る (§4.3)。
///
/// 戻り値が `None` の場合は「クエリから bigram が取れなかった」= 1 文字以下か空。
/// 呼び出し側は早期 return して「2 文字以上入力してください」を UI に表示する。
pub fn build_bigram_and_query(
    fields: &Fields,
    query_text: &str,
    favorite_ids: Option<&[Uuid]>,
) -> Option<BooleanQuery> {
    // 正規化は共通関数経由 (§5.2 Codex 2回目 #3)
    let lowered = crate::search_norm::normalize_for_match(query_text);

    let mut tokenizer = NgramTokenizer::new(2, 2, false).ok()?;
    let mut stream: Box<dyn TokenStream> = Box::new(tokenizer.token_stream(&lowered));
    let mut bigrams: Vec<String> = Vec::new();
    stream.process(&mut |t: &Token| bigrams.push(t.text.clone()));
    bigrams.sort();
    bigrams.dedup();
    if bigrams.is_empty() {
        return None;
    }

    let mut subs: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(bigrams.len() + 1);
    for bg in bigrams {
        let term = Term::from_field_text(fields.all_text, &bg);
        subs.push((
            Occur::Must,
            Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs)),
        ));
    }

    // favorite_id スコープ filter (複数 favorite の OR を更に Must として追加)
    if let Some(ids) = favorite_ids {
        if ids.is_empty() {
            // 対象 favorite がゼロなら絶対マッチしない = 空結果
            return None;
        }
        let mut fav_subs: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(ids.len());
        for id in ids {
            let term = Term::from_field_text(fields.favorite_id, &id.to_string());
            fav_subs.push((
                Occur::Should,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }
        subs.push((Occur::Must, Box::new(BooleanQuery::from(fav_subs))));
    }

    Some(BooleanQuery::from(subs))
}

/// 1 ページ分の検索結果を取得 (§9.1 ステップ 5)。
///
/// 戻り値は (path, score) のリスト。呼び出し側は path で `fts_meta.lookup_all_text_norm` を
/// 引き、post-filter の正確判定を行う。
///
/// ★ 同じ `searcher` をループ内で使い回すこと (snapshot 固定)。
pub fn search_page(
    searcher: &tantivy::Searcher,
    fields: &Fields,
    query: &BooleanQuery,
    offset: usize,
    limit: usize,
) -> tantivy::Result<Vec<(String, Score)>> {
    let top_docs: Vec<(Score, DocAddress)> = searcher.search(
        query,
        &TopDocs::with_limit(limit)
            .and_offset(offset)
            .order_by_score(),
    )?;
    let mut out = Vec::with_capacity(top_docs.len());
    for (score, addr) in top_docs {
        let doc: TantivyDocument = searcher.doc(addr)?;
        if let Some(v) = doc.get_first(fields.path) {
            if let Some(p) = v.as_str() {
                out.push((p.to_string(), score));
            }
        }
    }
    Ok(out)
}

// -----------------------------------------------------------------------
// 内部: スキーマ構築とトークナイザ登録
// -----------------------------------------------------------------------

fn build_schema() -> Schema {
    let mut b = Schema::builder();
    b.add_text_field("path", STRING | STORED);
    b.add_text_field("container", STRING | STORED);
    b.add_text_field("zip_entry", STRING | STORED);
    b.add_text_field("favorite_id", STRING | STORED);
    b.add_i64_field("mtime", INDEXED | STORED);
    b.add_i64_field("file_size", STORED);
    let text_opts = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(BIGRAM_TOKENIZER_NAME)
            .set_index_option(IndexRecordOption::WithFreqs),
    );
    b.add_text_field("name", text_opts.clone());
    b.add_text_field("all_text", text_opts);
    b.build()
}

fn register_tokenizer(index: &Index) {
    let analyzer = TextAnalyzer::builder(
        NgramTokenizer::new(2, 2, false).expect("NgramTokenizer(2,2) creation"),
    )
    .filter(LowerCaser)
    .build();
    index.tokenizers().register(BIGRAM_TOKENIZER_NAME, analyzer);
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn new_index() -> (TempDir, FtsIndex) {
        let dir = TempDir::new().unwrap();
        let idx = FtsIndex::open_at(dir.path()).unwrap();
        (dir, idx)
    }

    fn sample_doc(path: &str, fav: Uuid, text: &str) -> IndexDoc {
        IndexDoc {
            path: path.to_string(),
            container: Container::Fs,
            zip_entry: String::new(),
            favorite_id: fav,
            mtime: 100,
            file_size: 1024,
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            all_text: text.to_string(),
        }
    }

    #[test]
    fn build_schema_exposes_expected_fields() {
        let s = build_schema();
        assert!(s.get_field("path").is_ok());
        assert!(s.get_field("favorite_id").is_ok());
        assert!(s.get_field("all_text").is_ok());
    }

    #[test]
    fn upsert_then_search_hit() {
        let (_tmp, idx) = new_index();
        let fav = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc("c:/p/a.jpg", fav, "夕焼け 海辺 写真"),
        )
        .unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        let q = build_bigram_and_query(idx.fields(), "夕焼け", None).unwrap();
        let searcher = idx.searcher();
        let hits = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "c:/p/a.jpg");
    }

    #[test]
    fn upsert_replaces_existing_doc() {
        let (_tmp, idx) = new_index();
        let fav = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        upsert_doc(&writer, idx.fields(), &sample_doc("c:/a.jpg", fav, "cat photo")).unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        // 最初は "cat" でヒット
        let q = build_bigram_and_query(idx.fields(), "cat", None).unwrap();
        let searcher = idx.searcher();
        let hits = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        assert_eq!(hits.len(), 1);

        // 同じ path で別テキスト → 更新
        upsert_doc(&writer, idx.fields(), &sample_doc("c:/a.jpg", fav, "dog photo")).unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        let q_cat = build_bigram_and_query(idx.fields(), "cat", None).unwrap();
        let q_dog = build_bigram_and_query(idx.fields(), "dog", None).unwrap();
        let searcher = idx.searcher();
        let cat_hits = search_page(&searcher, idx.fields(), &q_cat, 0, 10).unwrap();
        let dog_hits = search_page(&searcher, idx.fields(), &q_dog, 0, 10).unwrap();
        assert_eq!(cat_hits.len(), 0, "旧テキストはもうヒットしない");
        assert_eq!(dog_hits.len(), 1);
    }

    #[test]
    fn delete_doc_removes_from_index() {
        let (_tmp, idx) = new_index();
        let fav = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        upsert_doc(&writer, idx.fields(), &sample_doc("c:/a.jpg", fav, "alpha"))
            .unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        delete_doc(&writer, idx.fields(), "c:/a.jpg");
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        let q = build_bigram_and_query(idx.fields(), "alpha", None).unwrap();
        let searcher = idx.searcher();
        let hits = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        assert_eq!(hits.len(), 0);
    }

    #[test]
    fn favorite_id_filter_scopes_results() {
        let (_tmp, idx) = new_index();
        let fav_a = Uuid::new_v4();
        let fav_b = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc("c:/a.jpg", fav_a, "夕焼け"),
        )
        .unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc("c:/b.jpg", fav_b, "夕焼け"),
        )
        .unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        let q_all = build_bigram_and_query(idx.fields(), "夕焼け", None).unwrap();
        let searcher = idx.searcher();
        let all_hits = search_page(&searcher, idx.fields(), &q_all, 0, 10).unwrap();
        assert_eq!(all_hits.len(), 2);

        let q_a = build_bigram_and_query(idx.fields(), "夕焼け", Some(&[fav_a])).unwrap();
        let a_hits = search_page(&searcher, idx.fields(), &q_a, 0, 10).unwrap();
        assert_eq!(a_hits.len(), 1);
        assert_eq!(a_hits[0].0, "c:/a.jpg");
    }

    #[test]
    fn single_char_query_returns_none() {
        let (_tmp, idx) = new_index();
        // 1 文字では bigram が作れない → None
        let q = build_bigram_and_query(idx.fields(), "の", None);
        assert!(q.is_none());
    }

    #[test]
    fn empty_favorite_ids_returns_none() {
        let (_tmp, idx) = new_index();
        let q = build_bigram_and_query(idx.fields(), "hello", Some(&[]));
        assert!(q.is_none(), "空 favorite_ids は絶対にマッチしない");
    }

    #[test]
    fn and_query_requires_all_bigrams() {
        let (_tmp, idx) = new_index();
        let fav = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        upsert_doc(&writer, idx.fields(), &sample_doc("c:/a.jpg", fav, "夕焼け"))
            .unwrap();
        upsert_doc(&writer, idx.fields(), &sample_doc("c:/b.jpg", fav, "海辺"))
            .unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc("c:/c.jpg", fav, "夕焼け 海辺"),
        )
        .unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        // "夕焼 海辺" (2文字以上のクエリを AND する想定だが bigram ベースなので
        // "夕焼 海辺" というスペース区切りの AND は上位レイヤーのジョブ。
        // ここでは連続する bigram を AND したときの挙動だけ確認)
        let q = build_bigram_and_query(idx.fields(), "夕焼け", None).unwrap();
        let searcher = idx.searcher();
        let hits = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        let paths: Vec<_> = hits.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"c:/a.jpg"));
        assert!(paths.contains(&"c:/c.jpg"));
        assert!(!paths.contains(&"c:/b.jpg"), "海辺 doc には夕焼けの bigram なし");
    }

    #[test]
    fn pagination_respects_offset_and_limit() {
        let (_tmp, idx) = new_index();
        let fav = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        for i in 0..30 {
            upsert_doc(
                &writer,
                idx.fields(),
                &sample_doc(
                    &format!("c:/p/{:03}.jpg", i),
                    fav,
                    "夕焼け",
                ),
            )
            .unwrap();
        }
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        let q = build_bigram_and_query(idx.fields(), "夕焼け", None).unwrap();
        let searcher = idx.searcher();
        let page0 = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        let page1 = search_page(&searcher, idx.fields(), &q, 10, 10).unwrap();
        let page2 = search_page(&searcher, idx.fields(), &q, 20, 10).unwrap();
        assert_eq!(page0.len(), 10);
        assert_eq!(page1.len(), 10);
        assert_eq!(page2.len(), 10);
        // snapshot 固定なので同じ searcher なら重複なし
        let all_paths: std::collections::HashSet<_> = page0
            .iter()
            .chain(page1.iter())
            .chain(page2.iter())
            .map(|(p, _)| p.clone())
            .collect();
        assert_eq!(all_paths.len(), 30);
    }
}
