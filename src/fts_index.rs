//! Tantivy ベースの全文検索インデックス (docs/search-architecture.md)。
//!
//! ## 役割 (INDEX_VERSION=5)
//!
//! - bigram tokenizer (`NgramTokenizer(2, 2)` + `lower_caser`) で画像メタを転置索引化
//! - `fts_meta.db` と二段整合性を組み、Ctrl+G の候補絞り込みに使う
//! - **post-filter 用の正規化済み原文を STORED で保持する**。INDEX_VERSION=5 で
//!   `fts_meta.db` の `*_norm` 列群 (合計数 GB 規模になりうる) を撤去し、bigram 索引と
//!   原文の両方を Tantivy 側 (`*_text` フィールドの STORED) に集約した。`fts_meta.db` は
//!   ファイル単位の管理メタ (status / mtime / size / generation) のみを持つ。
//! - 検索 worker は `searcher.doc(addr)` で STORED 原文を取り出して post-filter に渡す
//!   (`doc_text_for_target` ヘルパ参照)
//!
//! ## スキーマ (INDEX_VERSION=5)
//!
//! ```text
//! path             STRING | STORED            完全一致キー、正規化済み
//! container        STRING | STORED            "fs" / "zip"
//! zip_entry        STRING | STORED            container="zip" のとき ZIP 内相対パス
//! favorite_id      STRING | STORED            UUID (exact term filter 用)
//! kind             STRING | STORED            "folder" / "image" / "zip" / "pdf"
//! mtime            i64    INDEXED | STORED
//! file_size        i64    STORED
//! name             TEXT   bigram | STORED     ファイル名 / ZIP エントリ名
//! exif_text        TEXT   bigram | STORED     EXIF
//! xmp_tweet_text   TEXT   bigram | STORED     XMP / mXD ツイート情報
//! png_prompt_text  TEXT   bigram | STORED     PNG tEXt/iTXt AI プロンプト
//! pdf_meta_text    TEXT   bigram | STORED     PDFium document info
//! tags             TEXT   bigram | STORED     XMP dc:subject (#プレフィックス付き)
//! ```
//!
//! per-source に分けているのは、「検索対象フィルタ」 (`SearchTarget::Only`) で
//! "EXIF のみ" / "XMP ツイートのみ" のような絞り込みを post-filter 頼みでなく
//! ネイティブに行うため。タグも同じく `Only(Tags)` で絞り込み可。
//!
//! ## 検索の組み立て方
//!
//! クエリ文字列を `normalize_for_match` → bigram 分解 → 各 bigram を `TermQuery` にして
//! AND の `BooleanQuery` を作る。phrase/NOT/AND の最終判定は post-filter で行う。
//!
//! ## Searcher snapshot 固定
//!
//! 検索ワーカーは `FtsIndex::searcher()` を 1 回だけ取得し、ページング中はそれを使い回す。
//! これで ingest 側が commit して reader reload しても、検索中の snapshot はズレない。
//! ただし ingest worker 側は commit 後に同期 reload を要求する (`reload_after_commit=true`)。
//! これにより `mark_ok` 直後の検索は確実に新 snapshot を見える状態になっており、
//! INDEX_VERSION=5 で原文を Tantivy 側に集約したことによる post-filter 偽陽性を回避する。

use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    Field, FieldType, INDEXED, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing,
    TextOptions, Value,
};
use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer, Token, TokenStream, Tokenizer};
use tantivy::{DocAddress, Index, IndexReader, IndexWriter, Score, TantivyDocument, Term, doc};
use uuid::Uuid;

const BIGRAM_TOKENIZER_NAME: &str = "mimv_bigram";
const WRITER_HEAP_MB: usize = 64;
/// 検索時のページサイズ (Codex 3 回目 #6 worst case 計測で確定)
pub const PAGE_SIZE: usize = 500;

/// 検索対象となるメタソース種別 (§19.2 + tag 機能統合)。
/// `name` は独立フィールドだが、検索 UX 上「ファイル名で検索」も同じ target として扱えるよう enum に含める。
/// `Tags` は XMP dc:subject 由来のタグ (`#原神` 等) 専用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Filename,
    Exif,
    XmpTweet,
    PngPrompt,
    PdfMeta,
    Tags,
}

impl SourceKind {
    pub const ALL: &'static [SourceKind] = &[
        SourceKind::Filename,
        SourceKind::Exif,
        SourceKind::XmpTweet,
        SourceKind::PngPrompt,
        SourceKind::PdfMeta,
        SourceKind::Tags,
    ];
}

/// 検索対象フィルタ (§19.6)。`All` は 5 ソース全部を OR する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchTarget {
    All,
    Only(Vec<SourceKind>),
}

impl Default for SearchTarget {
    fn default() -> Self {
        SearchTarget::All
    }
}

impl SearchTarget {
    /// 対象ソースのスライス。`All` のときは `SourceKind::ALL` を返す。
    pub fn sources(&self) -> &[SourceKind] {
        match self {
            SearchTarget::All => SourceKind::ALL,
            SearchTarget::Only(v) => v.as_slice(),
        }
    }

    /// `source` がこの target に含まれるか。
    pub fn includes(&self, source: SourceKind) -> bool {
        match self {
            SearchTarget::All => true,
            SearchTarget::Only(v) => v.contains(&source),
        }
    }

    /// 有効な対象が 1 件もない (空 Only) — クエリを組んでも絶対にヒットしない。
    pub fn matches_nothing(&self) -> bool {
        matches!(self, SearchTarget::Only(v) if v.is_empty())
    }
}

/// タイプフィルタ用の大分類 (§19.3)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexKind {
    Folder,
    Image,
    Zip,
    Pdf,
}

impl IndexKind {
    pub fn as_str(self) -> &'static str {
        match self {
            IndexKind::Folder => "folder",
            IndexKind::Image => "image",
            IndexKind::Zip => "zip",
            IndexKind::Pdf => "pdf",
        }
    }

    /// SQLite に保存する整数表現 (fts_meta.db の kind 列)。
    pub fn to_i64(self) -> i64 {
        match self {
            IndexKind::Folder => 0,
            IndexKind::Image => 1,
            IndexKind::Zip => 2,
            IndexKind::Pdf => 3,
        }
    }

    /// `to_i64` の逆変換。不正値は診断ログ後 `Image` にフォールバックする。
    pub fn from_i64(v: i64) -> Self {
        match v {
            0 => IndexKind::Folder,
            1 => IndexKind::Image,
            2 => IndexKind::Zip,
            3 => IndexKind::Pdf,
            other => {
                crate::logger::log(format!(
                    "fts_index: unexpected IndexKind discriminant {other} — falling back to Image"
                ));
                IndexKind::Image
            }
        }
    }
}

/// 検索時に `build_bigram_and_query` に渡す絞り込みフィルタ群 (§19.6)。
#[derive(Debug, Clone, Default)]
pub struct QueryFilters<'a> {
    /// 指定なら該当 favorite_id の OR で絞る。空スライスは "絶対ヒットしない" シグナル (None を返す)。
    pub favorite_ids: Option<&'a [Uuid]>,
    /// 指定なら該当 kind の OR で絞る。None = すべて。
    pub kinds: Option<&'a [IndexKind]>,
    /// 検索対象ソース (ソースを跨いだ OR)。既定は `All` (= 5 ソース全部)。
    pub target: SearchTarget,
    /// include トークン結合モード (docs §20)。既定は AND。
    pub mode: crate::search_query::MatchMode,
}

/// Tantivy ドキュメント 1 件を構築するための入力。
///
/// ソース別テキスト (ファイル名・EXIF・XMP・PNG プロンプト・PDF メタ・タグ) は
/// [`crate::ingest_text::PerSourceText`] にまとめて保持する。
#[derive(Debug, Clone)]
pub struct IndexDoc {
    /// 正規化済みキー (fts_meta.db と同じ path)
    pub path: String,
    pub container: Container,
    /// container=Zip のとき、ZIP 内相対パス ("" なら Fs)
    pub zip_entry: String,
    pub favorite_id: Uuid,
    pub kind: IndexKind,
    pub mtime: i64,
    pub file_size: i64,
    /// ソース別 `normalize_for_match` 適用済みテキスト。
    pub norms: crate::ingest_text::PerSourceText,
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
    pub kind: Field,
    pub mtime: Field,
    pub file_size: Field,
    pub name: Field,
    pub exif_text: Field,
    pub xmp_tweet_text: Field,
    pub png_prompt_text: Field,
    pub pdf_meta_text: Field,
    pub tags: Field,
}

impl Fields {
    fn from_schema(schema: &Schema) -> Self {
        Self {
            path: schema.get_field("path").expect("schema: path"),
            container: schema.get_field("container").expect("schema: container"),
            zip_entry: schema.get_field("zip_entry").expect("schema: zip_entry"),
            favorite_id: schema
                .get_field("favorite_id")
                .expect("schema: favorite_id"),
            kind: schema.get_field("kind").expect("schema: kind"),
            mtime: schema.get_field("mtime").expect("schema: mtime"),
            file_size: schema.get_field("file_size").expect("schema: file_size"),
            name: schema.get_field("name").expect("schema: name"),
            exif_text: schema.get_field("exif_text").expect("schema: exif_text"),
            xmp_tweet_text: schema
                .get_field("xmp_tweet_text")
                .expect("schema: xmp_tweet_text"),
            png_prompt_text: schema
                .get_field("png_prompt_text")
                .expect("schema: png_prompt_text"),
            pdf_meta_text: schema
                .get_field("pdf_meta_text")
                .expect("schema: pdf_meta_text"),
            tags: schema.get_field("tags").expect("schema: tags"),
        }
    }

    /// 指定ソースに対応する Tantivy フィールドを返す (§19.6)。
    pub fn text_field_for(&self, source: SourceKind) -> Field {
        match source {
            SourceKind::Filename => self.name,
            SourceKind::Exif => self.exif_text,
            SourceKind::XmpTweet => self.xmp_tweet_text,
            SourceKind::PngPrompt => self.png_prompt_text,
            SourceKind::PdfMeta => self.pdf_meta_text,
            SourceKind::Tags => self.tags,
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
    ///
    /// §19.8 + tag 統合: 既存ディレクトリに旧スキーマ (per-source フィールド or tags
    /// フィールドのどちらかが無い) が残っていたらディレクトリごと破棄して新規作成する。
    /// fts_meta.db 側の `needs_rebuild` と合わせて「起動時は自動でフル再インデックス」を実現する。
    pub fn open_at(dir: &Path) -> tantivy::Result<Self> {
        std::fs::create_dir_all(dir).ok();
        // 既存インデックスがあれば 1 回だけ open して schema を確認し、そのまま使い回す。
        // 旧スキーマだったら drop して wipe + create に切り替える。
        let mmap_dir = tantivy::directory::MmapDirectory::open(dir)?;
        let existing = if Index::exists(&mmap_dir)? {
            Some(Index::open_in_dir(dir)?)
        } else {
            None
        };
        drop(mmap_dir);

        let index = match existing {
            Some(idx) if schema_is_stale(&idx.schema()) => {
                drop(idx);
                crate::logger::log(format!(
                    "fts_index: detected old schema at {} — wiping dir for rebuild",
                    dir.display()
                ));
                wipe_index_dir(dir)?;
                Index::create_in_dir(dir, build_schema())?
            }
            Some(idx) => idx,
            None => Index::create_in_dir(dir, build_schema())?,
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
pub fn upsert_doc(writer: &IndexWriter, fields: &Fields, d: &IndexDoc) -> tantivy::Result<()> {
    // 既存 doc を delete してから add (Tantivy の更新は delete + reinsert が基本)
    writer.delete_term(Term::from_field_text(fields.path, &d.path));
    writer.add_document(doc!(
        fields.path            => d.path.as_str(),
        fields.container       => d.container.as_str(),
        fields.zip_entry       => d.zip_entry.as_str(),
        fields.favorite_id     => d.favorite_id.to_string().as_str(),
        fields.kind            => d.kind.as_str(),
        fields.mtime           => d.mtime,
        fields.file_size       => d.file_size,
        fields.name            => d.norms.name.as_str(),
        fields.exif_text       => d.norms.exif.as_str(),
        fields.xmp_tweet_text  => d.norms.xmp_tweet.as_str(),
        fields.png_prompt_text => d.norms.png_prompt.as_str(),
        fields.pdf_meta_text   => d.norms.pdf_meta.as_str(),
        fields.tags            => d.norms.tags.as_str(),
    ))?;
    Ok(())
}

/// 指定 path の doc を Tantivy から削除 (§5.6.2)。
pub fn delete_doc(writer: &IndexWriter, fields: &Fields, path: &str) {
    writer.delete_term(Term::from_field_text(fields.path, path));
}

/// 複数 include トークン + 対象フィルタ (favorite / kind / source) から BooleanQuery を作る (§4.3, §19.6)。
///
/// ## トークン単位にする理由 (Codex 6 回目指摘 #1)
///
/// 旧実装は `include_tokens.join(" ")` を 1 本化してから bigram 化していたが、
/// 連結で生じる境界の bigram (例: `け ` や ` 海`) まで AND 必須になり、検索漏れの原因になる。
///
/// 正しくは **各トークンを独立に bigram 化してトークン内部で AND、トークン間を AND** にする。
///
/// ## フィールド間の OR (§19 分割対応)
///
/// `SearchTarget::All` では `name / exif_text / xmp_tweet_text / png_prompt_text / pdf_meta_text`
/// の 5 フィールドに対して OR を取る (ソースを跨いでトークンが見つかれば OK)。
/// `SearchTarget::Only([...])` では指定フィールドのみで OR。
/// トークン 1 つあたり: (AND of bigrams) を各対象フィールドで作り、フィールド間を OR でまとめる。
/// トークン間はさらに AND。
///
/// ## 戻り値 `None` の条件
///
/// - `include_tokens` が空 (呼び出し側で早期 return)
/// - 任意のトークンが bigram を生成できない (1 文字等 — 最小長は呼び出し側で判定する契約)
/// - `favorite_ids = Some(&[])` / `kinds = Some(&[])` / `target = Only(&[])` (絶対ヒットしない)
pub fn build_bigram_and_query(
    fields: &Fields,
    include_tokens: &[&str],
    filters: &QueryFilters,
) -> Option<BooleanQuery> {
    if include_tokens.is_empty() {
        return None;
    }
    // 空集合は「絶対ヒットしない」として早期 return (Codex 6 回目指摘 #3 を各フィルタに拡張)
    if let Some(ids) = filters.favorite_ids {
        if ids.is_empty() {
            return None;
        }
    }
    if let Some(ks) = filters.kinds {
        if ks.is_empty() {
            return None;
        }
    }
    if filters.target.matches_nothing() {
        return None;
    }

    let target_sources = filters.target.sources();
    debug_assert!(!target_sources.is_empty());

    // OR モード (docs §20): include トークンを Should で束ねて別 BooleanQuery を作り、
    // その全体を Must として token_queries に積む。AND モードは従来どおり各 token を
    // 直接 Must にする。favorite_id / kind フィルタは必ず Must で後から AND 結合する。
    let token_occur = match filters.mode {
        crate::search_query::MatchMode::And => Occur::Must,
        crate::search_query::MatchMode::Or => Occur::Should,
    };
    let mut per_token_queries: Vec<(Occur, Box<dyn Query>)> =
        Vec::with_capacity(include_tokens.len());
    for tok in include_tokens {
        let lowered = crate::search_norm::normalize_for_match(tok);
        let mut tokenizer = NgramTokenizer::new(2, 2, false).ok()?;
        let mut stream: Box<dyn TokenStream> = Box::new(tokenizer.token_stream(&lowered));
        let mut bigrams: Vec<String> = Vec::new();
        stream.process(&mut |t: &Token| bigrams.push(t.text.clone()));
        bigrams.sort();
        bigrams.dedup();
        if bigrams.is_empty() {
            // このトークンは bigram 生成不可 (1 文字等) → クエリ組み立て失敗
            return None;
        }

        // 各対象フィールドに対して「AND of bigrams」を作り、フィールド間を OR でまとめる
        let mut field_disjuncts: Vec<(Occur, Box<dyn Query>)> =
            Vec::with_capacity(target_sources.len());
        for &source in target_sources {
            let field = fields.text_field_for(source);
            let mut bigram_ands: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(bigrams.len());
            for bg in &bigrams {
                let term = Term::from_field_text(field, bg);
                bigram_ands.push((
                    Occur::Must,
                    Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs)),
                ));
            }
            field_disjuncts.push((Occur::Should, Box::new(BooleanQuery::from(bigram_ands))));
        }
        per_token_queries.push((token_occur, Box::new(BooleanQuery::from(field_disjuncts))));
    }

    // AND モードは per_token_queries を直接 top-level に並べて Must 結合。
    // OR モードは Should 群を 1 つの BooleanQuery で包んで Must として積む (= 少なくとも 1 件一致)。
    // ただし token が 1 個なら OR/AND どちらも実効意味が同じなので、余計な入れ子を避ける。
    let mut token_queries: Vec<(Occur, Box<dyn Query>)> =
        if matches!(filters.mode, crate::search_query::MatchMode::Or)
            && per_token_queries.len() > 1
        {
            let any = BooleanQuery::from(per_token_queries);
            vec![(Occur::Must, Box::new(any))]
        } else {
            // 1 token の場合は occur を Must に正規化 (per_token_queries には Should が入っていることがある)
            per_token_queries
                .into_iter()
                .map(|(_, q)| (Occur::Must, q))
                .collect()
        };
    token_queries.reserve(3);

    // favorite_id スコープ filter (複数 favorite の OR を更に Must として追加)
    if let Some(ids) = filters.favorite_ids {
        let mut fav_subs: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(ids.len());
        for id in ids {
            let term = Term::from_field_text(fields.favorite_id, &id.to_string());
            fav_subs.push((
                Occur::Should,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }
        token_queries.push((Occur::Must, Box::new(BooleanQuery::from(fav_subs))));
    }

    // kind スコープ filter (タイプドロップダウン)
    if let Some(ks) = filters.kinds {
        let mut kind_subs: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(ks.len());
        for k in ks {
            let term = Term::from_field_text(fields.kind, k.as_str());
            kind_subs.push((
                Occur::Should,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }
        token_queries.push((Occur::Must, Box::new(BooleanQuery::from(kind_subs))));
    }

    Some(BooleanQuery::from(token_queries))
}

/// 1 ページ分の検索結果を取得 (§9.1 ステップ 5)。
///
/// 戻り値は (path, DocAddress, score) のリスト。`DocAddress` は post-filter で
/// `searcher.doc(addr)` を呼んで STORED 原文を取り出すために返している
/// (INDEX_VERSION=5 で fts_meta.db の `*_norm` 列を撤去した移行に伴う変更)。
///
/// ★ 同じ `searcher` をループ内で使い回すこと (snapshot 固定)。
pub fn search_page(
    searcher: &tantivy::Searcher,
    fields: &Fields,
    query: &BooleanQuery,
    offset: usize,
    limit: usize,
) -> tantivy::Result<Vec<(String, DocAddress, Score)>> {
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
                out.push((p.to_string(), addr, score));
            }
        }
    }
    Ok(out)
}

/// `path` で Tantivy doc を 1 件引いてその `DocAddress` を返す。
/// タグ書き込み worker が「既存 doc に tags だけ差し替えて upsert する」ために使う。
/// ヒットしなければ None (まだ ingest が未完了 / pending → 通常経路に任せる)。
pub fn find_doc_by_path(
    searcher: &tantivy::Searcher,
    fields: &Fields,
    path: &str,
) -> tantivy::Result<Option<DocAddress>> {
    let term = Term::from_field_text(fields.path, path);
    let q = TermQuery::new(term, IndexRecordOption::Basic);
    let top: Vec<(Score, DocAddress)> =
        searcher.search(&q, &TopDocs::with_limit(1).order_by_score())?;
    Ok(top.into_iter().next().map(|(_, addr)| addr))
}

/// 指定 doc の STORED `*_text` 6 ソース全部を `PerSourceText` に詰めて返す。
/// `tag_write_worker` が「他ソースの text を保ったまま tags だけ差し替えて upsert」
/// するのに使う (INDEX_VERSION=5 以降は fts_meta.db に norms が無いため)。
pub fn doc_per_source_text(
    searcher: &tantivy::Searcher,
    fields: &Fields,
    addr: DocAddress,
) -> tantivy::Result<crate::ingest_text::PerSourceText> {
    let doc: TantivyDocument = searcher.doc(addr)?;
    let read = |f: Field| -> String {
        doc.get_first(f)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default()
    };
    Ok(crate::ingest_text::PerSourceText {
        name: read(fields.name),
        exif: read(fields.exif_text),
        xmp_tweet: read(fields.xmp_tweet_text),
        png_prompt: read(fields.png_prompt_text),
        pdf_meta: read(fields.pdf_meta_text),
        tags: read(fields.tags),
    })
}

/// STORED フィールドから target に対応するテキストをスペース連結で取り出す
/// (post-filter 用)。INDEX_VERSION=5 以降は fts_meta.db ではなくここで原文を取る。
///
/// 各 SourceKind に対応する `*_text` フィールドは ingest 時に
/// `normalize_for_match` 適用済みで保存されているので、呼び出し側はそのまま
/// `search_query::matches_with_mode` に渡せる。
pub fn doc_text_for_target(
    searcher: &tantivy::Searcher,
    fields: &Fields,
    addr: DocAddress,
    target: &SearchTarget,
) -> tantivy::Result<String> {
    let doc: TantivyDocument = searcher.doc(addr)?;
    let mut out = String::new();
    for &src in target.sources() {
        let f = fields.text_field_for(src);
        if let Some(v) = doc.get_first(f) {
            if let Some(s) = v.as_str() {
                if s.is_empty() {
                    continue;
                }
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(s);
            }
        }
    }
    Ok(out)
}

// -----------------------------------------------------------------------
// 内部: スキーマ構築とトークナイザ登録
// -----------------------------------------------------------------------

/// 既存の Tantivy schema が最新と一致するか判定する。
/// 判定: 新フィールド 6 本 (`name`/`exif_text`/`xmp_tweet_text`/`png_prompt_text`/
/// `pdf_meta_text`/`tags`) が揃っていて、旧 `all_text` が残っておらず、
/// **かつ INDEX_VERSION=5 で要求される STORED 属性が付いていること**。
///
/// STORED チェックを忘れると、v4 (per-source field 名は同じ・STORED なし) を
/// 再利用してしまい、新 ingest した doc も STORED されず post-filter が空文字列で
/// 動いて検索ヒットがゼロに見える事故が起きる (Codex P2 指摘)。
fn schema_is_stale(schema: &Schema) -> bool {
    let has_new = schema.get_field("exif_text").is_ok()
        && schema.get_field("xmp_tweet_text").is_ok()
        && schema.get_field("png_prompt_text").is_ok()
        && schema.get_field("pdf_meta_text").is_ok()
        && schema.get_field("kind").is_ok()
        && schema.get_field("tags").is_ok();
    if !has_new {
        return true;
    }
    if schema.get_field("all_text").is_ok() {
        return true;
    }
    // text 系 6 フィールドはすべて STORED 必須 (INDEX_VERSION=5)
    for name in [
        "name",
        "exif_text",
        "xmp_tweet_text",
        "png_prompt_text",
        "pdf_meta_text",
        "tags",
    ] {
        let Ok(field) = schema.get_field(name) else {
            return true;
        };
        let entry = schema.get_field_entry(field);
        match entry.field_type() {
            FieldType::Str(opts) if opts.is_stored() => {}
            _ => return true,
        }
    }
    false
}

/// ディレクトリ配下のファイルを全削除してから、ディレクトリ自体も再作成する。
/// Tantivy が内部的に持つ .lock / meta.json / segments を一掃するため。
fn wipe_index_dir(dir: &Path) -> tantivy::Result<()> {
    // remove_dir_all → create_dir_all のトランザクションをベストエフォートで実行
    if let Err(e) = std::fs::remove_dir_all(dir) {
        // Windows で他プロセスが開いているとアクセス拒否になる。ここでは致命ではない
        // (下で create_dir_all を試みる; 残骸があっても create_in_dir が上書きする)。
        crate::logger::log(format!(
            "fts_index: remove_dir_all({}) failed: {e} (continuing)",
            dir.display()
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| {
        tantivy::TantivyError::SystemError(format!(
            "fts_index: create_dir_all after wipe failed: {e}"
        ))
    })?;
    Ok(())
}

fn build_schema() -> Schema {
    let mut b = Schema::builder();
    b.add_text_field("path", STRING | STORED);
    b.add_text_field("container", STRING | STORED);
    b.add_text_field("zip_entry", STRING | STORED);
    b.add_text_field("favorite_id", STRING | STORED);
    b.add_text_field("kind", STRING | STORED);
    b.add_i64_field("mtime", INDEXED | STORED);
    b.add_i64_field("file_size", STORED);
    // INDEX_VERSION=5 (Tantivy STORED 化): 各 *_text フィールドに STORED を付け、
    // post-filter 用の正規化済み原文を Tantivy 側に集約する。
    // これに伴い fts_meta.db の *_norm 列は撤去された。
    let text_opts = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(BIGRAM_TOKENIZER_NAME)
                .set_index_option(IndexRecordOption::WithFreqs),
        )
        .set_stored();
    b.add_text_field("name", text_opts.clone());
    b.add_text_field("exif_text", text_opts.clone());
    b.add_text_field("xmp_tweet_text", text_opts.clone());
    b.add_text_field("png_prompt_text", text_opts.clone());
    b.add_text_field("pdf_meta_text", text_opts.clone());
    // タグフィールドも bigram tokenize — `原神` キーワード検索で `#原神` タグにヒット、
    // `#原神` 入力でもヒットする (A-1 設計)。target=Tags ドロップダウンで絞り込み可。
    b.add_text_field("tags", text_opts);
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

    /// テスト用 doc ヘルパ。`text` は `name` フィールドに入れ、残りのソース別フィールドは空。
    /// これにより既存テストの「`SearchTarget::All` でヒットする」挙動は維持される。
    fn sample_doc(path: &str, fav: Uuid, text: &str) -> IndexDoc {
        let base = path.rsplit('/').next().unwrap_or(path);
        IndexDoc {
            path: path.to_string(),
            container: Container::Fs,
            zip_entry: String::new(),
            favorite_id: fav,
            kind: IndexKind::Image,
            mtime: 100,
            file_size: 1024,
            norms: crate::ingest_text::PerSourceText {
                name: format!("{base} {text}"),
                ..Default::default()
            },
        }
    }

    /// テスト用 doc で、ソース別フィールドに個別に値を入れられる版 (§19 target フィルタ検証用)。
    fn sample_doc_with_sources(
        path: &str,
        fav: Uuid,
        kind: IndexKind,
        name: &str,
        exif: &str,
        xmp_tweet: &str,
        png_prompt: &str,
    ) -> IndexDoc {
        IndexDoc {
            path: path.to_string(),
            container: Container::Fs,
            zip_entry: String::new(),
            favorite_id: fav,
            kind,
            mtime: 100,
            file_size: 1024,
            norms: crate::ingest_text::PerSourceText {
                name: name.to_string(),
                exif: exif.to_string(),
                xmp_tweet: xmp_tweet.to_string(),
                png_prompt: png_prompt.to_string(),
                pdf_meta: String::new(),
                tags: String::new(),
            },
        }
    }

    /// テスト用 doc (tags フィールドのみ書き込む版)。
    fn sample_doc_with_tags(path: &str, fav: Uuid, tags: &str) -> IndexDoc {
        IndexDoc {
            path: path.to_string(),
            container: Container::Fs,
            zip_entry: String::new(),
            favorite_id: fav,
            kind: IndexKind::Image,
            mtime: 100,
            file_size: 1024,
            norms: crate::ingest_text::PerSourceText {
                tags: tags.to_string(),
                ..Default::default()
            },
        }
    }

    fn q_all(fields: &Fields, tokens: &[&str]) -> Option<BooleanQuery> {
        build_bigram_and_query(fields, tokens, &QueryFilters::default())
    }

    #[test]
    fn build_schema_exposes_expected_fields() {
        let s = build_schema();
        assert!(s.get_field("path").is_ok());
        assert!(s.get_field("favorite_id").is_ok());
        assert!(s.get_field("kind").is_ok());
        assert!(s.get_field("name").is_ok());
        assert!(s.get_field("exif_text").is_ok());
        assert!(s.get_field("xmp_tweet_text").is_ok());
        assert!(s.get_field("png_prompt_text").is_ok());
        assert!(s.get_field("pdf_meta_text").is_ok());
        assert!(
            s.get_field("all_text").is_err(),
            "all_text は §19 で削除済み"
        );
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

        let q = q_all(idx.fields(), &["夕焼け"]).unwrap();
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
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc("c:/a.jpg", fav, "cat photo"),
        )
        .unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        // 最初は "cat" でヒット
        let q = q_all(idx.fields(), &["cat"]).unwrap();
        let searcher = idx.searcher();
        let hits = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        assert_eq!(hits.len(), 1);

        // 同じ path で別テキスト → 更新
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc("c:/a.jpg", fav, "dog photo"),
        )
        .unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        let q_cat = q_all(idx.fields(), &["cat"]).unwrap();
        let q_dog = q_all(idx.fields(), &["dog"]).unwrap();
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
        upsert_doc(&writer, idx.fields(), &sample_doc("c:/a.jpg", fav, "alpha")).unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        delete_doc(&writer, idx.fields(), "c:/a.jpg");
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        let q = q_all(idx.fields(), &["alpha"]).unwrap();
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

        let query_all = q_all(idx.fields(), &["夕焼け"]).unwrap();
        let searcher = idx.searcher();
        let all_hits = search_page(&searcher, idx.fields(), &query_all, 0, 10).unwrap();
        assert_eq!(all_hits.len(), 2);

        let favs_a = [fav_a];
        let q_a = build_bigram_and_query(
            idx.fields(),
            &["夕焼け"],
            &QueryFilters {
                favorite_ids: Some(&favs_a),
                ..Default::default()
            },
        )
        .unwrap();
        let a_hits = search_page(&searcher, idx.fields(), &q_a, 0, 10).unwrap();
        assert_eq!(a_hits.len(), 1);
        assert_eq!(a_hits[0].0, "c:/a.jpg");
    }

    #[test]
    fn single_char_query_returns_none() {
        let (_tmp, idx) = new_index();
        // 1 文字では bigram が作れない → None
        let q = q_all(idx.fields(), &["の"]);
        assert!(q.is_none());
    }

    #[test]
    fn empty_favorite_ids_returns_none() {
        let (_tmp, idx) = new_index();
        let q = build_bigram_and_query(
            idx.fields(),
            &["hello"],
            &QueryFilters {
                favorite_ids: Some(&[]),
                ..Default::default()
            },
        );
        assert!(q.is_none(), "空 favorite_ids は絶対にマッチしない");
    }

    #[test]
    fn empty_kinds_returns_none() {
        let (_tmp, idx) = new_index();
        let q = build_bigram_and_query(
            idx.fields(),
            &["hello"],
            &QueryFilters {
                kinds: Some(&[]),
                ..Default::default()
            },
        );
        assert!(q.is_none(), "空 kinds も絶対にマッチしない");
    }

    #[test]
    fn empty_target_returns_none() {
        let (_tmp, idx) = new_index();
        let q = build_bigram_and_query(
            idx.fields(),
            &["hello"],
            &QueryFilters {
                target: SearchTarget::Only(vec![]),
                ..Default::default()
            },
        );
        assert!(q.is_none(), "空 target も絶対にマッチしない");
    }

    #[test]
    fn empty_tokens_returns_none() {
        let (_tmp, idx) = new_index();
        let q = q_all(idx.fields(), &[]);
        assert!(q.is_none(), "include トークン 0 個は None");
    }

    #[test]
    fn multi_token_finds_doc_with_distant_token_positions() {
        // Codex 6 回目指摘 #1 回帰テスト:
        // 複数 include トークンを個別に bigram 化するので、元テキストでトークン間の
        // 文字位置が離れていても両方が含まれればヒットする。
        let (_tmp, idx) = new_index();
        let fav = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        // "夕焼け" と "海辺" の間に他の文字 (句読点) を挟む
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc("c:/a.jpg", fav, "夕焼け、海辺、そして人々"),
        )
        .unwrap();
        // 両方含むが隣接していない
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc("c:/b.jpg", fav, "海辺 写真 夕焼け"),
        )
        .unwrap();
        // 片方だけ
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc("c:/c.jpg", fav, "夕焼けのみ"),
        )
        .unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        // 旧実装 (join スペース 1 本化) だと "け " や " 海" の bigram 必須で
        // a も b も漏れる。新実装は各トークン独立にするので両方ヒットする。
        let q = q_all(idx.fields(), &["夕焼け", "海辺"]).unwrap();
        let searcher = idx.searcher();
        let hits = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        let paths: Vec<_> = hits.iter().map(|(p, _, _)| p.as_str()).collect();
        assert!(
            paths.contains(&"c:/a.jpg"),
            "句読点挟み doc がヒットするはず"
        );
        assert!(paths.contains(&"c:/b.jpg"), "逆順 doc もヒットするはず");
        assert!(!paths.contains(&"c:/c.jpg"), "片方だけは除外");
    }

    #[test]
    fn and_query_requires_all_bigrams() {
        let (_tmp, idx) = new_index();
        let fav = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc("c:/a.jpg", fav, "夕焼け"),
        )
        .unwrap();
        upsert_doc(&writer, idx.fields(), &sample_doc("c:/b.jpg", fav, "海辺")).unwrap();
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
        let q = q_all(idx.fields(), &["夕焼け"]).unwrap();
        let searcher = idx.searcher();
        let hits = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        let paths: Vec<_> = hits.iter().map(|(p, _, _)| p.as_str()).collect();
        assert!(paths.contains(&"c:/a.jpg"));
        assert!(paths.contains(&"c:/c.jpg"));
        assert!(
            !paths.contains(&"c:/b.jpg"),
            "海辺 doc には夕焼けの bigram なし"
        );
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
                &sample_doc(&format!("c:/p/{:03}.jpg", i), fav, "夕焼け"),
            )
            .unwrap();
        }
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        let q = q_all(idx.fields(), &["夕焼け"]).unwrap();
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
            .map(|(p, _, _)| p.clone())
            .collect();
        assert_eq!(all_paths.len(), 30);
    }

    // =======================================================================
    // §19 拡張: 検索対象 / タイプフィルタの検証
    // =======================================================================

    #[test]
    fn opening_old_schema_index_dir_wipes_and_rebuilds() {
        // §19.8 マイグレーション回帰: 旧スキーマ (all_text 有 / exif_text 無) のインデックスを
        // 開いたらディレクトリを wipe して新スキーマで再作成する。
        use tantivy::schema::{IndexRecordOption, Schema, TextFieldIndexing, TextOptions};
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("idx");
        std::fs::create_dir_all(&path).unwrap();

        // 旧スキーマを手動で作って書き込む
        {
            let mut b = Schema::builder();
            b.add_text_field("path", STRING | STORED);
            b.add_text_field("container", STRING | STORED);
            b.add_text_field("zip_entry", STRING | STORED);
            b.add_text_field("favorite_id", STRING | STORED);
            b.add_i64_field("mtime", INDEXED | STORED);
            b.add_i64_field("file_size", STORED);
            let bigram = TextOptions::default().set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(BIGRAM_TOKENIZER_NAME)
                    .set_index_option(IndexRecordOption::WithFreqs),
            );
            b.add_text_field("name", bigram.clone());
            b.add_text_field("all_text", bigram);
            let old_schema = b.build();
            let old_index = Index::create_in_dir(&path, old_schema).unwrap();
            register_tokenizer(&old_index);
        }
        // この時点で schema_is_stale が true を返すはず
        let old_idx = Index::open_in_dir(&path).unwrap();
        assert!(schema_is_stale(&old_idx.schema()));
        drop(old_idx);

        // open_at が wipe → 新スキーマで作り直す
        let idx = FtsIndex::open_at(&path).unwrap();
        // 新スキーマのフィールドが全部あること
        let s = idx.index().schema();
        assert!(s.get_field("exif_text").is_ok());
        assert!(s.get_field("xmp_tweet_text").is_ok());
        assert!(s.get_field("kind").is_ok());
        assert!(s.get_field("all_text").is_err(), "旧 all_text は消えた");
    }

    #[test]
    fn opening_fresh_index_dir_does_not_wipe() {
        // 新規ディレクトリでは wipe パスを通らず、普通に create される。
        let dir = TempDir::new().unwrap();
        let _idx = FtsIndex::open_at(dir.path()).unwrap();
    }

    /// Codex P2 回帰: v4 schema (per-source field 名は同じ・STORED 無し) で作られた
    /// インデックスを開いたとき、`schema_is_stale` が true を返して wipe される。
    /// STORED チェックを忘れると v4 を再利用してしまい、新 ingest した doc も STORED
    /// されず post-filter が空文字列で動いて検索ヒットがゼロに見える事故が起きる。
    #[test]
    fn opening_v4_schema_without_stored_text_is_detected_as_stale() {
        use tantivy::schema::{IndexRecordOption, Schema, TextFieldIndexing, TextOptions};
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("idx");
        std::fs::create_dir_all(&path).unwrap();

        // v4 スキーマ: フィールド名は v5 と同じだが TEXT は STORED なし
        {
            let mut b = Schema::builder();
            b.add_text_field("path", STRING | STORED);
            b.add_text_field("container", STRING | STORED);
            b.add_text_field("zip_entry", STRING | STORED);
            b.add_text_field("favorite_id", STRING | STORED);
            b.add_text_field("kind", STRING | STORED);
            b.add_i64_field("mtime", INDEXED | STORED);
            b.add_i64_field("file_size", STORED);
            let bigram = TextOptions::default().set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(BIGRAM_TOKENIZER_NAME)
                    .set_index_option(IndexRecordOption::WithFreqs),
            );
            b.add_text_field("name", bigram.clone());
            b.add_text_field("exif_text", bigram.clone());
            b.add_text_field("xmp_tweet_text", bigram.clone());
            b.add_text_field("png_prompt_text", bigram.clone());
            b.add_text_field("pdf_meta_text", bigram.clone());
            b.add_text_field("tags", bigram); // STORED 未指定
            let v4_schema = b.build();
            let v4_index = Index::create_in_dir(&path, v4_schema).unwrap();
            register_tokenizer(&v4_index);
        }
        let v4_idx = Index::open_in_dir(&path).unwrap();
        assert!(
            schema_is_stale(&v4_idx.schema()),
            "STORED 無しの v4 schema は stale 扱いになるべき"
        );
        drop(v4_idx);

        // FtsIndex::open_at が wipe → 新 schema で再作成し、STORED 付き
        let idx = FtsIndex::open_at(&path).unwrap();
        let s = idx.index().schema();
        // 再作成後の schema は STORED 付きなので stale ではない
        assert!(!schema_is_stale(&s));
    }

    #[test]
    fn target_only_exif_matches_only_exif_field() {
        let (_tmp, idx) = new_index();
        let fav = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        // a: EXIF にだけ "夕焼け"
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc_with_sources(
                "c:/a.jpg",
                fav,
                IndexKind::Image,
                "a.jpg",
                "夕焼け camera",
                "",
                "",
            ),
        )
        .unwrap();
        // b: XMP tweet にだけ "夕焼け"
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc_with_sources(
                "c:/b.jpg",
                fav,
                IndexKind::Image,
                "b.jpg",
                "",
                "夕焼け post",
                "",
            ),
        )
        .unwrap();
        // c: name にだけ "夕焼け"
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc_with_sources(
                "c:/c.jpg",
                fav,
                IndexKind::Image,
                "夕焼け.jpg",
                "",
                "",
                "",
            ),
        )
        .unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        let searcher = idx.searcher();

        // Target::All = 3 件全部ヒット
        let q = q_all(idx.fields(), &["夕焼け"]).unwrap();
        let all_hits = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        assert_eq!(all_hits.len(), 3, "All では 3 件全部ヒット");

        // Target::Only(EXIF) = a のみ
        let q_exif = build_bigram_and_query(
            idx.fields(),
            &["夕焼け"],
            &QueryFilters {
                target: SearchTarget::Only(vec![SourceKind::Exif]),
                ..Default::default()
            },
        )
        .unwrap();
        let exif_hits = search_page(&searcher, idx.fields(), &q_exif, 0, 10).unwrap();
        let exif_paths: Vec<_> = exif_hits.iter().map(|(p, _, _)| p.as_str()).collect();
        assert_eq!(exif_hits.len(), 1, "EXIF target は 1 件");
        assert_eq!(exif_paths[0], "c:/a.jpg");

        // Target::Only(XmpTweet) = b のみ
        let q_xmp = build_bigram_and_query(
            idx.fields(),
            &["夕焼け"],
            &QueryFilters {
                target: SearchTarget::Only(vec![SourceKind::XmpTweet]),
                ..Default::default()
            },
        )
        .unwrap();
        let xmp_hits = search_page(&searcher, idx.fields(), &q_xmp, 0, 10).unwrap();
        let xmp_paths: Vec<_> = xmp_hits.iter().map(|(p, _, _)| p.as_str()).collect();
        assert_eq!(xmp_hits.len(), 1);
        assert_eq!(xmp_paths[0], "c:/b.jpg");

        // Target::Only(Filename) = c のみ
        let q_name = build_bigram_and_query(
            idx.fields(),
            &["夕焼け"],
            &QueryFilters {
                target: SearchTarget::Only(vec![SourceKind::Filename]),
                ..Default::default()
            },
        )
        .unwrap();
        let name_hits = search_page(&searcher, idx.fields(), &q_name, 0, 10).unwrap();
        let name_paths: Vec<_> = name_hits.iter().map(|(p, _, _)| p.as_str()).collect();
        assert_eq!(name_hits.len(), 1);
        assert_eq!(name_paths[0], "c:/c.jpg");
    }

    #[test]
    fn target_only_multiple_sources_combined_or() {
        // 複数ソース選択時は OR (EXIF または XMP) になる。
        let (_tmp, idx) = new_index();
        let fav = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc_with_sources(
                "c:/a.jpg",
                fav,
                IndexKind::Image,
                "a.jpg",
                "夕焼け",
                "",
                "",
            ),
        )
        .unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc_with_sources(
                "c:/b.jpg",
                fav,
                IndexKind::Image,
                "b.jpg",
                "",
                "夕焼け",
                "",
            ),
        )
        .unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc_with_sources(
                "c:/c.jpg",
                fav,
                IndexKind::Image,
                "other.jpg",
                "",
                "",
                "夕焼け",
            ),
        )
        .unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        let q = build_bigram_and_query(
            idx.fields(),
            &["夕焼け"],
            &QueryFilters {
                target: SearchTarget::Only(vec![SourceKind::Exif, SourceKind::XmpTweet]),
                ..Default::default()
            },
        )
        .unwrap();
        let searcher = idx.searcher();
        let hits = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        let paths: std::collections::HashSet<_> =
            hits.iter().map(|(p, _, _)| p.as_str()).collect();
        assert_eq!(hits.len(), 2, "EXIF + XMP の OR で a,b だけヒット");
        assert!(paths.contains("c:/a.jpg"));
        assert!(paths.contains("c:/b.jpg"));
        assert!(!paths.contains("c:/c.jpg"), "PNG プロンプトは対象外");
    }

    #[test]
    fn kind_filter_scopes_results() {
        let (_tmp, idx) = new_index();
        let fav = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc_with_sources(
                "c:/folder",
                fav,
                IndexKind::Folder,
                "夕焼け フォルダ",
                "",
                "",
                "",
            ),
        )
        .unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc_with_sources(
                "c:/a.zip",
                fav,
                IndexKind::Zip,
                "夕焼け zip",
                "",
                "",
                "",
            ),
        )
        .unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc_with_sources(
                "c:/b.pdf",
                fav,
                IndexKind::Pdf,
                "夕焼け pdf",
                "",
                "",
                "",
            ),
        )
        .unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc_with_sources(
                "c:/c.jpg",
                fav,
                IndexKind::Image,
                "夕焼け jpg",
                "",
                "",
                "",
            ),
        )
        .unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        let searcher = idx.searcher();

        // kinds=None → 4 件全部
        let q_all_q = q_all(idx.fields(), &["夕焼け"]).unwrap();
        let all_hits = search_page(&searcher, idx.fields(), &q_all_q, 0, 10).unwrap();
        assert_eq!(all_hits.len(), 4);

        // kinds=[Zip, Pdf] → 2 件
        let kinds = [IndexKind::Zip, IndexKind::Pdf];
        let q = build_bigram_and_query(
            idx.fields(),
            &["夕焼け"],
            &QueryFilters {
                kinds: Some(&kinds),
                ..Default::default()
            },
        )
        .unwrap();
        let hits = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        let paths: std::collections::HashSet<_> =
            hits.iter().map(|(p, _, _)| p.as_str()).collect();
        assert_eq!(hits.len(), 2);
        assert!(paths.contains("c:/a.zip"));
        assert!(paths.contains("c:/b.pdf"));
    }

    /// target=Only([Tags]) で tags フィールドだけを対象に検索できることを確認する。
    /// name フィールドに同じトークンがあっても引っかからないこと。
    #[test]
    fn target_only_tags_matches_only_tags_field() {
        let (_tmp, idx) = new_index();
        let fav = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc_with_tags("c:/a.jpg", fav, "#原神 #風景"),
        )
        .unwrap();
        // name フィールドに "原神" を入れた doc は target=Tags では引っかからない
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc_with_sources(
                "c:/b.jpg",
                fav,
                IndexKind::Image,
                "原神.jpg",
                "",
                "",
                "",
            ),
        )
        .unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        let searcher = idx.searcher();
        let q = build_bigram_and_query(
            idx.fields(),
            &["原神"],
            &QueryFilters {
                target: SearchTarget::Only(vec![SourceKind::Tags]),
                ..Default::default()
            },
        )
        .unwrap();
        let hits = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        let paths: Vec<_> = hits.iter().map(|(p, _, _)| p.as_str()).collect();
        assert_eq!(hits.len(), 1, "Tags target は tags フィールドのみ対象");
        assert_eq!(paths[0], "c:/a.jpg");
    }

    // ---- OR モード (docs §20) ----

    #[test]
    fn or_mode_matches_any_include_token() {
        use crate::search_query::MatchMode;
        let (_tmp, idx) = new_index();
        let fav = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        upsert_doc(&writer, idx.fields(), &sample_doc("c:/a.jpg", fav, "夕焼け")).unwrap();
        upsert_doc(&writer, idx.fields(), &sample_doc("c:/b.jpg", fav, "海辺")).unwrap();
        upsert_doc(
            &writer,
            idx.fields(),
            &sample_doc("c:/c.jpg", fav, "unrelated"),
        )
        .unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        // OR で "夕焼け" OR "海辺" → a, b の両方ヒット
        let q = build_bigram_and_query(
            idx.fields(),
            &["夕焼け", "海辺"],
            &QueryFilters {
                mode: MatchMode::Or,
                ..Default::default()
            },
        )
        .unwrap();
        let searcher = idx.searcher();
        let hits = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        let paths: std::collections::HashSet<_> =
            hits.iter().map(|(p, _, _)| p.as_str()).collect();
        assert!(paths.contains("c:/a.jpg"));
        assert!(paths.contains("c:/b.jpg"));
        assert!(!paths.contains("c:/c.jpg"), "どちらも含まない doc は除外");
    }

    #[test]
    fn or_mode_single_token_behaves_like_and() {
        // include 1 個なら OR/AND 結果は常に同じ (短絡最適化の回帰防止)。
        use crate::search_query::MatchMode;
        let (_tmp, idx) = new_index();
        let fav = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        upsert_doc(&writer, idx.fields(), &sample_doc("c:/a.jpg", fav, "夕焼け 海辺")).unwrap();
        upsert_doc(&writer, idx.fields(), &sample_doc("c:/b.jpg", fav, "朝日")).unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        let searcher = idx.searcher();
        let q_and = build_bigram_and_query(idx.fields(), &["夕焼け"], &QueryFilters::default()).unwrap();
        let q_or = build_bigram_and_query(
            idx.fields(),
            &["夕焼け"],
            &QueryFilters {
                mode: MatchMode::Or,
                ..Default::default()
            },
        )
        .unwrap();
        let and_hits: Vec<_> = search_page(&searcher, idx.fields(), &q_and, 0, 10)
            .unwrap()
            .into_iter()
            .map(|(p, _, _)| p)
            .collect();
        let or_hits: Vec<_> = search_page(&searcher, idx.fields(), &q_or, 0, 10)
            .unwrap()
            .into_iter()
            .map(|(p, _, _)| p)
            .collect();
        assert_eq!(and_hits, or_hits, "1 token では AND/OR ヒット集合が一致");
        assert_eq!(and_hits, vec!["c:/a.jpg".to_string()]);
    }

    #[test]
    fn or_mode_respects_favorite_and_kind_filters() {
        // OR モードでも favorite / kind の AND フィルタは有効
        use crate::search_query::MatchMode;
        let (_tmp, idx) = new_index();
        let fav_a = Uuid::new_v4();
        let fav_b = Uuid::new_v4();
        let mut writer = idx.writer().unwrap();
        upsert_doc(&writer, idx.fields(), &sample_doc("c:/a.jpg", fav_a, "夕焼け")).unwrap();
        upsert_doc(&writer, idx.fields(), &sample_doc("c:/b.jpg", fav_b, "海辺")).unwrap();
        writer.commit().unwrap();
        idx.reload_reader().unwrap();

        // "夕焼け" OR "海辺" だが favorite=fav_a に絞る → a のみ
        let favs = [fav_a];
        let q = build_bigram_and_query(
            idx.fields(),
            &["夕焼け", "海辺"],
            &QueryFilters {
                mode: MatchMode::Or,
                favorite_ids: Some(&favs),
                ..Default::default()
            },
        )
        .unwrap();
        let searcher = idx.searcher();
        let hits = search_page(&searcher, idx.fields(), &q, 0, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "c:/a.jpg");
    }
}
