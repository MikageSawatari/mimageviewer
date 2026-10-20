//! Ctrl+G グローバルメタ検索の streaming クエリワーカー。
//!
//! docs/search-architecture.md に準拠する。
//!
//! ## 動作
//!
//! 1. クエリ文字列を `search_query::parse` で AST (Token リスト) にする
//! 2. 最小長ポリシー + NOT-only 禁止チェック (Ctrl+G 向け)
//! 3. 正のトークンを集約し、bigram BooleanQuery を構築
//! 4. **Searcher snapshot を固定** (ページング中に ingest が commit しても結果が
//!    ズレないようにする)
//! 5. `TopDocs::with_limit(PAGE_SIZE).and_offset(offset)` でページング取得
//! 6. 各ページで `fts_index::doc_text_for_target` で同じ Tantivy snapshot から
//!    STORED 原文を取り出し、`search_query::matches_with_mode` で phrase / NOT /
//!    AND/OR を最終判定
//! 7. post-filter 通過した結果を `SearchStreamEvent::Batch` で streaming 送信
//! 8. HARD_MAX 到達 / 候補使い切り / cancel のどれかで終了
//!
//! ## UI との接続
//!
//! 呼び出し側は別スレッドでこの関数を実行し、`crossbeam_channel::Receiver` で
//! 結果を受け取る。UI 側は毎フレーム `try_recv` で取り出して items に append する。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::Sender;
use uuid::Uuid;

use crate::fts_index::{self, FtsIndex, IndexKind, QueryFilters, SearchTarget};
use crate::search_query::{self, MatchMode, Token};

/// §9.1 ステップ 4 確定方針 (プロトタイプ計測で PASS、§15.1.9)
pub const HARD_MAX: usize = 10_000;
/// ページサイズは Tantivy 側の定数と揃える。
pub use crate::fts_index::PAGE_SIZE;

/// 検索イベント到着時に UI owner を起こす callback。
///
/// 検索層は egui に依存せず、呼び出し側が必要な viewport への wake を投影する。
pub type SearchWake = Arc<dyn Fn() + Send + Sync>;

/// Ctrl+G ワーカーが UI へ送るイベント。
#[derive(Debug, Clone)]
pub enum SearchStreamEvent {
    /// post-filter 通過済みの候補 (既出は含まない)
    Batch {
        hits: Vec<GlobalHit>,
        /// 累計で Tantivy が返した候補数 (進捗表示用)
        scanned_candidates: usize,
        /// 累計で post-filter を通った数
        valid_hits: usize,
        visibility_notice: Option<crate::archive_visibility::ArchiveVisibilityNotice>,
    },
    /// 正常終了
    Done { truncated: bool, reason: DoneReason },
    /// エラー
    Error(String),
}

/// event を送信し、receiver がまだ生きているときだけ UI を起こす。
///
/// cancel 済み検索の receiver drop 後まで wake すると、旧 worker が検索 owner を
/// 不要に起こすため、send 成功を callback の前提にする。
fn send_event(
    tx: &Sender<SearchStreamEvent>,
    wake: Option<&(dyn Fn() + Send + Sync)>,
    event: SearchStreamEvent,
) -> bool {
    if tx.send(event).is_err() {
        return false;
    }
    if let Some(wake) = wake {
        wake();
    }
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoneReason {
    /// Tantivy が候補を使い切った
    Complete,
    /// HARD_MAX 到達で打ち切り
    TruncatedAtMax,
    /// キャンセル
    Cancelled,
    /// クエリが最小長に満たない等の早期 return
    RejectedQuery(RejectReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// クエリが空
    Empty,
    /// 最小長ポリシーで拒否した include 語 (パース済みの小文字表記)。
    TooShort(Vec<String>),
    /// NOT-only クエリ (Ctrl+G では禁止)
    NotOnly,
}

/// 1 ヒット = 1 ファイル / ZIP エントリ。
#[derive(Debug, Clone)]
pub struct GlobalHit {
    pub path: String,
    /// Tantivy が吐いたスコア (UI でのソートや表示に使える)
    pub score: f32,
    /// 0..=5。worker は常に 0 を入れ、UI 側 (`poll_global_search_events`) が
    /// rating DB から bulk lookup して書き込む。drilled view のサブフォルダバッジ
    /// 件数を rating_filter で絞り込むのに使う。
    ///
    /// **スナップショット**: ヒット受信時点の値で固定する。Ctrl+G セッション中に
    /// ユーザーが同じ画像のレーティングを変えてもこのフィールドは更新されない
    /// (バッジ件数が次のクエリ実行までは古いまま。実害は小さいので許容)。
    pub stars: u8,
    /// ファイルの最終更新時刻 (UNIX 秒)。Tantivy の STORED `mtime` フィールドから
    /// 取り出す。Ctrl+G 一覧 (Flat) / ドリルインビューの日付ソート
    /// (docs/search-container-item-redesign.md §4.3.3) に使う。
    pub mtime: i64,
    /// ファイルサイズ (byte)。現行 FTS producer の STORED 値は metadata 取得済みなので
    /// 0-byte を `Some(0)` として保持する。旧/合成 doc に field が無い場合だけ `None`。
    pub file_size: Option<i64>,
}

/// Ctrl+G 検索ワーカーに渡すフィルタ (§19 ドロップダウン UI と対応)。
#[derive(Debug, Clone, Default)]
pub struct SearchScope {
    /// タイプドロップダウン "画像 / PDF / 動画 / 音声"。空 `Vec` は呼び出し側で弾くこと。
    pub kinds: Option<Vec<IndexKind>>,
    /// 検索対象ドロップダウン "EXIF / XMP / ..."。既定は `All`。
    pub target: SearchTarget,
    /// include トークン結合モード (docs §20)。既定は AND。
    pub mode: crate::search_query::MatchMode,
    /// Hide EPUB paths before counting accepted hits toward HARD_MAX.
    pub ignore_epub: bool,
    /// List policy is captured once per search run and applied before the accepted-hit cap.
    pub visibility: crate::file_type_visibility::FileTypeVisibility,
}

/// Classify indexed logical identities without filesystem access. Archive member rows use
/// their member extension; PDF/EPUB rows use the original document path, never render assets.
/// Nested archive provenance is separate from this leaf-type predicate.
pub(crate) fn search_hit_type_is_visible(
    visibility: &crate::file_type_visibility::FileTypeVisibility,
    path: &str,
) -> bool {
    let source = path
        .split_once(crate::search_norm::ZIP_ENTRY_SEP)
        .map_or(path, |(_, member)| member);
    visibility.allows_physical_file(std::path::Path::new(source))
}

/// 検索ワーカーのエントリーポイント。別スレッドで実行する想定。
///
/// - `favorite_ids`: 検索対象のお気に入り UUID (Ctrl+G は `auto_index_metadata=true` の全部)
/// - `scope`: タイプ / 検索対象フィルタ (§19)。初期値は `SearchScope::default()` で全開放
/// - `cancel`: ユーザ操作や新しい入力で true にされたら速やかに中断する
/// - `tx`: UI へ events を送るチャネル
/// - `wake`: event 到着時に UI owner を起こす callback。headless caller は `None`
pub fn run(
    query_text: &str,
    favorite_ids: &[Uuid],
    scope: &SearchScope,
    fts: &FtsIndex,
    cancel: &AtomicBool,
    tx: &Sender<SearchStreamEvent>,
    wake: Option<&(dyn Fn() + Send + Sync)>,
) {
    run_with_hit_limit(
        query_text,
        favorite_ids,
        scope,
        fts,
        cancel,
        tx,
        wake,
        HARD_MAX,
    );
}

fn run_with_hit_limit(
    query_text: &str,
    favorite_ids: &[Uuid],
    scope: &SearchScope,
    fts: &FtsIndex,
    cancel: &AtomicBool,
    tx: &Sender<SearchStreamEvent>,
    wake: Option<&(dyn Fn() + Send + Sync)>,
    hit_limit: usize,
) {
    // 1. クエリパース
    let tokens = search_query::parse(query_text);

    // 2. 早期 return ポリシー
    if let Err(reason) = validate_query(query_text, &tokens, scope.mode) {
        let _ = send_event(
            tx,
            wake,
            SearchStreamEvent::Done {
                truncated: false,
                reason: DoneReason::RejectedQuery(reason),
            },
        );
        return;
    }

    // 3. 対象 favorite が 0 件なら空結果で即完了 (Codex 6 回目指摘 #3)
    // 旧実装は None を fts_index に渡して "favorite filter なし = 全件検索" となる事故があった。
    if favorite_ids.is_empty() {
        let _ = send_event(
            tx,
            wake,
            SearchStreamEvent::Done {
                truncated: false,
                reason: DoneReason::Complete,
            },
        );
        return;
    }

    // 4. 正の include トークンを集めて bigram クエリを組む。
    //    mIV タグは tags.db の専用面で扱うため、FTS の `tags` フィールドは移行専用。
    let include_tokens: Vec<&str> = tokens
        .iter()
        .filter(|t| t.include)
        .map(|t| t.needle.as_str())
        .collect();
    if include_tokens.is_empty() {
        // validate_query で NOT-only は弾かれるはず
        let _ = send_event(
            tx,
            wake,
            SearchStreamEvent::Done {
                truncated: false,
                reason: DoneReason::RejectedQuery(RejectReason::NotOnly),
            },
        );
        return;
    }

    let filters = QueryFilters {
        favorite_ids: Some(favorite_ids),
        kinds: scope.kinds.as_deref(),
        target: scope.target.clone(),
        mode: scope.mode,
    };
    let Some(query) = fts_index::build_bigram_and_query(fts.fields(), &include_tokens, &filters)
    else {
        // 1 文字語は validate_query で拒否済み。空の対象フィルタ等による構築失敗を
        // 語の長さの問題と誤表示しない。
        let _ = send_event(
            tx,
            wake,
            SearchStreamEvent::Error("検索条件から索引クエリを作成できません".into()),
        );
        return;
    };

    // 4. Searcher snapshot 固定 (§9.1 ステップ 4)
    let searcher = fts.searcher();

    // 5. ページングループ
    let mut offset = 0usize;
    let mut notice_emitted = false;
    let mut scanned = 0usize;
    let mut valid = 0usize;
    let mut archive_visibility =
        crate::archive_visibility::ArchiveVisibilityProjection::new(&scope.visibility, cancel);

    // final_reason は labeled break で確定させる (unused-assignment 警告回避)。
    let (truncated, final_reason) = 'paging: loop {
        if cancel.load(Ordering::Relaxed) {
            break 'paging (false, DoneReason::Cancelled);
        }

        // 5a. ページ取得
        let page = match fts_index::search_page(&searcher, fts.fields(), &query, offset, PAGE_SIZE)
        {
            Ok(p) => p,
            Err(e) => {
                let _ = send_event(
                    tx,
                    wake,
                    SearchStreamEvent::Error(format!("tantivy search_page: {e}")),
                );
                return;
            }
        };
        if page.is_empty() {
            break 'paging (false, DoneReason::Complete);
        }
        scanned += page.len();

        // 5b. EPUB visibility and token post-filter. 削除直後の短い窓では Tantivy delete_term
        // 投入 → 次回 commit までの間、削除済み path が結果に混じる。サムネイル読み込み
        // 失敗で気付ける前提で許容する。
        let mut batch = Vec::new();
        let mut inner_truncated = false;
        let mut inner_cancelled = false;
        for (path, addr, score) in page {
            if cancel.load(Ordering::Relaxed) {
                inner_cancelled = true;
                break;
            }
            // The index has no separate EPUB kind. Skip by indexed path here,
            // before fetching STORED text and before the accepted-hit limit.
            if scope.ignore_epub && is_epub_search_path(&path) {
                continue;
            }
            if !search_hit_type_is_visible(&scope.visibility, &path) {
                continue;
            }
            let text = match fts_index::doc_text_for_target(
                &searcher,
                fts.fields(),
                addr,
                &scope.target,
            ) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if search_query::matches_with_mode(&tokens, &text, scope.mode) {
                // Only matching omitted rows produce an origin warning. Keep this before
                // accepted-hit counting and metadata materialization.
                if path
                    .split_once(crate::search_norm::ZIP_ENTRY_SEP)
                    .is_some_and(|(container, member)| {
                        !archive_visibility.allows_member(std::path::Path::new(container), member)
                    })
                {
                    continue;
                }
                // STORED mtime を取り出す (日付ソート用 §5.2)。post-filter を通った
                // ヒットだけに対して呼ぶので、走査候補全件への doc fetch は発生しない。
                let (mtime, file_size) =
                    fts_index::doc_listing_metadata(&searcher, fts.fields(), addr)
                        .unwrap_or((0, None));
                batch.push(GlobalHit {
                    path,
                    score,
                    stars: 0,
                    mtime,
                    file_size,
                });
                valid += 1;
                if valid >= hit_limit {
                    inner_truncated = true;
                    break;
                }
            }
        }

        // 1 ページごとに累計を通知する。除外語の post-filter で全滅する検索でも
        // PAGE_SIZE 件ずつ進捗が見え、UI 側は 1 frame の drain 上限で負荷を抑える。
        let visibility_notice = archive_visibility.notice().filter(|_| !notice_emitted);
        notice_emitted |= visibility_notice.is_some();
        if !send_event(
            tx,
            wake,
            SearchStreamEvent::Batch {
                hits: batch,
                scanned_candidates: scanned,
                valid_hits: valid,
                visibility_notice,
            },
        ) {
            return;
        }

        if inner_truncated {
            break 'paging (true, DoneReason::TruncatedAtMax);
        }
        if inner_cancelled {
            break 'paging (false, DoneReason::Cancelled);
        }
        offset += PAGE_SIZE;
    };

    let _ = send_event(
        tx,
        wake,
        SearchStreamEvent::Done {
            truncated,
            reason: final_reason,
        },
    );
}

fn is_epub_search_path(path: &str) -> bool {
    // ZIP page keys are not top-level EPUB files.
    !path.contains(crate::search_norm::ZIP_ENTRY_SEP)
        && std::path::Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
}

// -----------------------------------------------------------------------
// 内部ヘルパー
// -----------------------------------------------------------------------

/// Ctrl+G 専用の最小長ポリシー + NOT-only 拒否 (§1.229)。
/// 1 文字語の索引照会可否と、単独では候補を広げすぎる 2 文字語を区別する。
/// AND では十分な長さの include 語が候補を絞るため、後者も Must として照会できる。
fn validate_query(query_text: &str, tokens: &[Token], mode: MatchMode) -> Result<(), RejectReason> {
    if query_text.trim().is_empty() || tokens.is_empty() {
        return Err(RejectReason::Empty);
    }
    // NOT-only: 正のトークンが 1 つも無ければ拒否
    if !tokens.iter().any(|t| t.include) {
        return Err(RejectReason::NotOnly);
    }
    let lengths: Vec<_> = tokens
        .iter()
        .filter(|t| t.include)
        .map(|t| (t, query_term_length(&t.needle)))
        .collect();
    let has_anchor = lengths
        .iter()
        .any(|(_, length)| *length == QueryTermLength::Sufficient);
    let short_terms: Vec<String> = lengths
        .into_iter()
        // タグ候補への橋渡し: #a 等は従来どおり免除。# 込みで 2 文字以上。
        .filter(|(t, length)| {
            !t.is_tag
                && match length {
                    QueryTermLength::Unindexable => true,
                    QueryTermLength::NeedsAnchor => mode == MatchMode::Or || !has_anchor,
                    QueryTermLength::Sufficient => false,
                }
        })
        .map(|(t, _)| t.needle.clone())
        .collect();
    if !short_terms.is_empty() {
        return Err(RejectReason::TooShort(short_terms));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryTermLength {
    /// 1 文字以下。AND の他語が十分長くても拒否する (2026-10-19 利用者決定)。
    Unindexable,
    /// 非 CJK 2 文字。AND で十分な長さの include 語があるときだけ許可する。
    NeedsAnchor,
    /// 従来の最小長 (CJK を含む 2 文字以上 / その他 3 文字以上) を満たす。
    Sufficient,
}

fn query_term_length(text: &str) -> QueryTermLength {
    let mut char_count = 0;
    let mut contains_cjk = false;
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        char_count += 1;
        contains_cjk |= is_cjk(c);
    }
    if char_count < 2 {
        QueryTermLength::Unindexable
    } else if char_count == 2 && !contains_cjk {
        QueryTermLength::NeedsAnchor
    } else {
        QueryTermLength::Sufficient
    }
}

fn is_cjk(c: char) -> bool {
    let code = c as u32;
    // Hiragana
    (0x3040..=0x309F).contains(&code)
        // Katakana (+ Katakana Phonetic Extensions)
        || (0x30A0..=0x30FF).contains(&code)
        || (0x31F0..=0x31FF).contains(&code)
        // CJK Unified Ideographs (+ extensions A/B/C/D/E/F/G)
        || (0x3400..=0x4DBF).contains(&code)
        || (0x4E00..=0x9FFF).contains(&code)
        || (0x20000..=0x2A6DF).contains(&code)
        || (0x2A700..=0x2EBEF).contains(&code)
        // Hangul (Korean, also CJK group)
        || (0xAC00..=0xD7AF).contains(&code)
        // Fullwidth ASCII / 半角カナ (Halfwidth and Fullwidth Forms)
        || (0xFF00..=0xFFEF).contains(&code)
}

// Just for external consumers who want to know the reason value.
impl RejectReason {
    pub fn as_user_message(&self) -> String {
        match self {
            RejectReason::Empty => "検索キーワードを入力してください".into(),
            RejectReason::TooShort(terms) => {
                let terms = terms
                    .iter()
                    .map(|term| format!("\"{term}\""))
                    .collect::<Vec<_>>()
                    .join("、");
                format!(
                    "{terms} が短すぎます (1 文字語は不可。英数字 2 文字語は AND で長い語と併用してください)"
                )
            }
            RejectReason::NotOnly => {
                "含める語を 1 つ以上入力してください (除外だけの検索は不可)".into()
            }
        }
    }
}

/// UI 側で `GlobalHit::path` を `PathBuf` に戻すための小ヘルパー。
pub fn hit_to_pathbuf(hit: &GlobalHit) -> PathBuf {
    PathBuf::from(&hit.path)
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fts_index::{Container, IndexDoc, upsert_doc};
    use crate::fts_meta::FtsMetaDb;
    use crate::ingest_text::PerSourceText;
    use crate::search_index_db::normalize_path;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use tempfile::TempDir;

    fn is_transient_tantivy_permission_error(error: &tantivy::TantivyError) -> bool {
        matches!(
            error,
            tantivy::TantivyError::IoError(io)
                if io.kind() == std::io::ErrorKind::PermissionDenied
        )
    }

    /// Windows の並列テストでは、AV / Search Indexer が Tantivy の一時ファイルを
    /// 数十 ms だけ掴み、テスト fixture の書き込みが OS error 5 になることがある。
    /// 製品側の検索エラーを隠さないよう、テスト setup の PermissionDenied だけを
    /// 最大 2 秒再試行し、他のエラーは即時返す。
    fn retry_tantivy_permission_denied<T>(
        mut operation: impl FnMut() -> tantivy::Result<T>,
    ) -> tantivy::Result<T> {
        let mut retries = 0;
        loop {
            match operation() {
                Err(error) if retries < 40 && is_transient_tantivy_permission_error(&error) => {
                    retries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                result => return result,
            }
        }
    }

    #[test]
    fn visibility_search_worker_preserves_cache_proof_and_index_without_current_source() {
        for missing_source in [false, true] {
            let guard = crate::data_dir::TestDataDirGuard::new();
            let (source, cached) =
                crate::archive_visibility::legacy_conversion_fixture(guard.path());
            let source_bytes = std::fs::read(&source).unwrap();
            let (_tmp, meta, fts) = setup();
            let fav = Uuid::new_v4();
            let keys = [
                "root.jpg",
                "chapter.zip/page.jpg",
                "notes.zip/page.jpg",
                "missing.jpg",
            ]
            .map(|member| {
                format!(
                    "{}{}{member}",
                    cached.to_string_lossy(),
                    crate::search_norm::ZIP_ENTRY_SEP
                )
            });
            for key in &keys {
                ingest(&meta, &fts, fav, key, "visibilitylegacy");
            }
            let stored = fts.searcher().num_docs();
            assert_eq!(stored, 4);
            let invalidate_source = || {
                if missing_source {
                    std::fs::remove_file(&source).unwrap();
                } else {
                    std::fs::write(&source, b"changed original archive").unwrap();
                }
            };
            let scope = SearchScope {
                visibility: crate::file_type_visibility::FileTypeVisibility {
                    excluded_categories: [crate::file_type_visibility::FileTypeCategory::Zip]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
                ..Default::default()
            };
            let collect = || {
                let (tx, rx) = crossbeam_channel::unbounded();
                run(
                    "visibilitylegacy",
                    &[fav],
                    &scope,
                    &fts,
                    &AtomicBool::new(false),
                    &tx,
                    None,
                );
                drop(tx);
                let mut hits = Vec::new();
                let mut notice = None;
                for event in rx.try_iter() {
                    if let SearchStreamEvent::Batch {
                        hits: batch,
                        visibility_notice,
                        ..
                    } = event
                    {
                        hits.extend(batch);
                        notice = notice.or(visibility_notice);
                    }
                }
                (hits, notice)
            };
            invalidate_source();
            let (hits, notice) = collect();
            assert_eq!(hits.len(), 1);
            assert!(hits[0].path.ends_with("missing.jpg"));
            assert_eq!(
                notice,
                Some(crate::archive_visibility::ArchiveVisibilityNotice::LegacyConvertedCache)
            );
            assert_eq!(fts.searcher().num_docs(), stored);
            std::fs::write(&source, &source_bytes).unwrap();
            crate::archive_visibility::regenerate_conversion_fixture(&source, &cached);
            let (hits, notice) = collect();
            assert_eq!(hits.len(), 3);
            assert!(
                hits.iter()
                    .all(|hit| !hit.path.ends_with("chapter.zip/page.jpg"))
            );
            assert_eq!(notice, None);
            invalidate_source();
            let (hits, notice) = collect();
            assert_eq!(hits.len(), 3);
            assert!(
                hits.iter()
                    .all(|hit| !hit.path.ends_with("chapter.zip/page.jpg"))
            );
            assert_eq!(notice, None);
            assert_eq!(fts.searcher().num_docs(), stored);
            for key in &keys {
                assert!(
                    crate::fts_index::find_doc_by_path(
                        &fts.searcher(),
                        fts.fields(),
                        &normalize_path(&PathBuf::from(key))
                    )
                    .unwrap()
                    .is_some()
                );
            }
        }
    }

    #[test]
    fn tantivy_fixture_retry_is_limited_to_permission_denied() {
        let mut permission_attempts = 0;
        let recovered = retry_tantivy_permission_denied(|| {
            permission_attempts += 1;
            if permission_attempts == 1 {
                Err(tantivy::TantivyError::IoError(std::sync::Arc::new(
                    std::io::Error::new(std::io::ErrorKind::PermissionDenied, "temporary lock"),
                )))
            } else {
                Ok("ok")
            }
        })
        .unwrap();
        assert_eq!(recovered, "ok");
        assert_eq!(permission_attempts, 2);

        let mut other_attempts = 0;
        let error = retry_tantivy_permission_denied(|| {
            other_attempts += 1;
            Err::<(), _>(tantivy::TantivyError::InvalidArgument("bad fixture".into()))
        })
        .unwrap_err();
        assert!(matches!(error, tantivy::TantivyError::InvalidArgument(_)));
        assert_eq!(other_attempts, 1);
    }

    fn setup() -> (TempDir, FtsMetaDb, FtsIndex) {
        let tmp = TempDir::new().unwrap();
        let meta = FtsMetaDb::open_at(&tmp.path().join("meta.db")).unwrap();
        let fts =
            retry_tantivy_permission_denied(|| FtsIndex::open_at(&tmp.path().join("fts_index")))
                .unwrap();
        (tmp, meta, fts)
    }

    /// `text` を **`name` フィールド + 全対象ソースに入る combined norm** で ingest する。
    /// 既存テストは `SearchTarget::All` 前提なので、これで従来挙動と互換になる。
    fn ingest(meta: &FtsMetaDb, fts: &FtsIndex, fav: Uuid, path_str: &str, text: &str) {
        let key = normalize_path(&PathBuf::from(path_str));
        let base_name = path_str.rsplit('/').next().unwrap_or(path_str);
        let combined_name = format!(
            "{} {}",
            crate::search_norm::normalize_for_match(base_name),
            crate::search_norm::normalize_for_match(text),
        );
        let norms = PerSourceText {
            name: combined_name,
            ..PerSourceText::default()
        };
        meta.upsert_meta_ok(&key, fav, &PathBuf::from("C:/"), IndexKind::Image, 0, 0)
            .unwrap();
        let doc = IndexDoc {
            path: key.clone(),
            container: Container::Fs,
            zip_entry: String::new(),
            favorite_id: fav,
            kind: IndexKind::Image,
            mtime: 0,
            file_size: 0,
            norms,
        };
        retry_tantivy_permission_denied(|| {
            let mut writer = fts.writer()?;
            upsert_doc(&writer, fts.fields(), &doc)?;
            writer.commit()?;
            fts.reload_reader()
        })
        .unwrap();
    }

    fn collect_events(
        query: &str,
        favs: &[Uuid],
        fts: &FtsIndex,
    ) -> (Vec<GlobalHit>, DoneReason, bool) {
        let (hits, reason, truncated, _) =
            collect_scoped_events(query, favs, &SearchScope::default(), fts);
        (hits, reason, truncated)
    }

    fn collect_scoped_events(
        query: &str,
        favs: &[Uuid],
        scope: &SearchScope,
        fts: &FtsIndex,
    ) -> (Vec<GlobalHit>, DoneReason, bool, usize) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = AtomicBool::new(false);
        run(query, favs, scope, fts, &cancel, &tx, None);
        drop(tx);
        let mut all_hits = Vec::new();
        let mut reason = DoneReason::Complete;
        let mut truncated = false;
        let mut scanned = 0;
        while let Ok(ev) = rx.recv() {
            match ev {
                SearchStreamEvent::Batch {
                    hits,
                    scanned_candidates,
                    ..
                } => {
                    all_hits.extend(hits);
                    scanned = scanned_candidates;
                }
                SearchStreamEvent::Done {
                    truncated: t,
                    reason: r,
                } => {
                    reason = r;
                    truncated = t;
                }
                SearchStreamEvent::Error(e) => panic!("unexpected error: {e}"),
            }
        }
        (all_hits, reason, truncated, scanned)
    }

    fn rejected_short_terms(terms: &[&str]) -> DoneReason {
        DoneReason::RejectedQuery(RejectReason::TooShort(
            terms.iter().map(|term| (*term).into()).collect(),
        ))
    }

    #[test]
    fn short_term_validation_distinguishes_indexability_and_candidate_breadth() {
        assert_eq!(query_term_length("r"), QueryTermLength::Unindexable);
        assert_eq!(query_term_length("夕"), QueryTermLength::Unindexable);
        assert_eq!(query_term_length("r5"), QueryTermLength::NeedsAnchor);
        assert_eq!(query_term_length("夕焼"), QueryTermLength::Sufficient);
        assert_eq!(query_term_length("EOS"), QueryTermLength::Sufficient);

        for mode in [MatchMode::And, MatchMode::Or] {
            for (query, short) in [
                ("r5", vec!["r5"]),
                ("sd ai", vec!["sd", "ai"]),
                ("夕 海", vec!["夕", "海"]),
                ("EOS x", vec!["x"]),
                ("EOS 夕", vec!["夕"]),
                ("夕焼 海", vec!["海"]),
                ("r5 -longword", vec!["r5"]),
                ("#a r5", vec!["r5"]),
            ] {
                let expected =
                    RejectReason::TooShort(short.iter().map(|term| (*term).into()).collect());
                assert_eq!(
                    validate_query(query, &search_query::parse(query), mode),
                    Err(expected.clone()),
                    "{query:?} / {mode:?}"
                );
                let message = expected.as_user_message();
                for term in short {
                    assert!(message.contains(&format!("\"{term}\"")), "{message}");
                }
            }
            // 除外語は最小長の対象外。タグ候補への橋渡しとフレーズの単位も維持する。
            for query in ["EOS -r5 -x -夕", "#a", "夕焼", r#""EOS R5""#] {
                assert_eq!(
                    validate_query(query, &search_query::parse(query), mode),
                    Ok(())
                );
            }
        }
        for query in ["EOS R5", "r5 EOS", "EOS sd ai", "夕焼 r5", "#long r5"] {
            assert_eq!(
                validate_query(query, &search_query::parse(query), MatchMode::And),
                Ok(())
            );
        }
        assert_eq!(
            validate_query("EOS R5", &search_query::parse("EOS R5"), MatchMode::Or),
            Err(RejectReason::TooShort(vec!["r5".into()]))
        );
        assert!(
            RejectReason::TooShort(vec!["sd".into(), "ai".into()])
                .as_user_message()
                .starts_with("\"sd\"、\"ai\" が短すぎます")
        );
    }

    #[test]
    fn single_char_terms_are_rejected_before_index_query_construction() {
        let (_tmp, _meta, fts) = setup();
        for mode in [MatchMode::And, MatchMode::Or] {
            let scope = SearchScope {
                mode,
                ..Default::default()
            };
            for (query, short) in [
                ("EOS a", vec!["a"]),
                ("EOS 夕", vec!["夕"]),
                ("夕 海", vec!["夕", "海"]),
            ] {
                // 空 favorite の早期 Complete より前に拒否するので、build の None での拒否ではない。
                let (hits, reason, _, scanned) = collect_scoped_events(query, &[], &scope, &fts);
                assert_eq!(reason, rejected_short_terms(&short));
                assert!(hits.is_empty());
                assert_eq!(scanned, 0);
            }
        }
    }

    #[test]
    fn and_short_terms_are_must_candidates_and_all_terms_are_post_filtered() {
        let (_tmp, _meta, fts) = setup();
        let fav = Uuid::new_v4();
        {
            let mut writer = fts.writer().unwrap();
            for (id, name, exif) in [
                ("a", "a.jpg", "Canon EOS R5"),
                ("b", "b.jpg", "R5 body from EOS series"),
                ("c", "EOS.jpg", "R5"),
                ("d", "d.jpg", "EOS"),
                ("e", "e.jpg", "R5"),
                // eo / os は存在するが eos は存在しない bigram 偽陽性。
                ("f", "f.jpg", "eox os R5"),
            ] {
                upsert_doc(
                    &writer,
                    fts.fields(),
                    &IndexDoc {
                        path: format!("c:/{id}.jpg"),
                        container: Container::Fs,
                        zip_entry: String::new(),
                        favorite_id: fav,
                        kind: IndexKind::Image,
                        mtime: 0,
                        file_size: 0,
                        norms: PerSourceText {
                            name: name.to_lowercase(),
                            exif: exif.to_lowercase(),
                            ..Default::default()
                        },
                    },
                )
                .unwrap();
            }
            writer.commit().unwrap();
        }
        fts.reload_reader().unwrap();
        let scope = SearchScope::default();
        let (_, reason, _, long_scanned) = collect_scoped_events("EOS", &[fav], &scope, &fts);
        assert_eq!(reason, DoneReason::Complete);
        assert_eq!(long_scanned, 5);
        let (hits, reason, _, scanned) = collect_scoped_events("EOS R5", &[fav], &scope, &fts);
        assert_eq!(reason, DoneReason::Complete);
        assert_eq!(scanned, 4, "r5 自体も Must なので EOS のみの d は候補外");
        let mut paths: Vec<_> = hits.iter().map(|hit| hit.path.as_str()).collect();
        paths.sort();
        assert_eq!(
            paths,
            ["c:/a.jpg", "c:/b.jpg", "c:/c.jpg"],
            "非隣接・逆順・別ソースを許可し、片語だけと偽陽性を除外"
        );
        let (hits, reason, _) = collect_events(r#""EOS R5""#, &[fav], &fts);
        assert_eq!(reason, DoneReason::Complete);
        assert_eq!(
            hits.iter().map(|hit| hit.path.as_str()).collect::<Vec<_>>(),
            ["c:/a.jpg"]
        );
        let (hits, _, _) = collect_events("EOS -r5", &[fav], &fts);
        assert_eq!(
            hits.iter().map(|hit| hit.path.as_str()).collect::<Vec<_>>(),
            ["c:/d.jpg"]
        );

        let or_scope = SearchScope {
            mode: MatchMode::Or,
            ..Default::default()
        };
        let (_, reason, _, scanned) = collect_scoped_events("EOS Canon", &[fav], &or_scope, &fts);
        assert_eq!(reason, DoneReason::Complete);
        assert_eq!(scanned, long_scanned, "既存の OR 候補集合を維持する");
        for query in ["EOS R5", "EOS Canon R5"] {
            let (hits, reason, _, scanned) = collect_scoped_events(query, &[fav], &or_scope, &fts);
            assert_eq!(reason, rejected_short_terms(&["r5"]));
            assert!(hits.is_empty());
            assert_eq!(scanned, 0, "OR に短い語を足しても候補走査を増やさない");
        }
        let (hits, reason, _) = collect_events("r5", &[fav], &fts);
        assert!(hits.is_empty());
        assert_eq!(reason, rejected_short_terms(&["r5"]));
    }

    #[test]
    fn and_short_phrase_post_filter_rejects_noncontiguous_bigram_candidates() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        for (path, text) in [
            ("c:/contiguous.jpg", "EOS r 5"),
            ("c:/noncontiguous.jpg", "EOS r x 5"),
            ("c:/no_spaces.jpg", "EOS r5"),
            ("c:/no_anchor.jpg", "r 5"),
            ("c:/anchor_only.jpg", "EOS only"),
        ] {
            ingest(&meta, &fts, fav, path, text);
        }

        // 空白を除くと 2 文字の短語。r5 の 1 bigram とは異なり、"r " / " 5" が
        // 同じ doc にあっても隣接は保証されないため、短語の post-filter 省略を検出できる。
        assert_eq!(query_term_length("r 5"), QueryTermLength::NeedsAnchor);
        let (hits, reason, truncated, scanned) =
            collect_scoped_events(r#"EOS "r 5""#, &[fav], &SearchScope::default(), &fts);
        assert_eq!(reason, DoneReason::Complete);
        assert!(!truncated);
        assert_eq!(scanned, 2, "連続・非連続の両方が索引候補になること");
        assert_eq!(
            hits.iter().map(|hit| hit.path.as_str()).collect::<Vec<_>>(),
            ["c:/contiguous.jpg"],
            "短いフレーズも post-filter で連続部分一致を要求すること"
        );
    }

    fn ingest_many_for_progress(fts: &FtsIndex, fav: Uuid, count: usize, text: &str) {
        retry_tantivy_permission_denied(|| {
            let mut writer = fts.writer()?;
            for index in 0..count {
                let path = format!("c:/progress/{index:04}.jpg");
                let doc = IndexDoc {
                    path: normalize_path(&PathBuf::from(&path)),
                    container: Container::Fs,
                    zip_entry: String::new(),
                    favorite_id: fav,
                    kind: IndexKind::Image,
                    mtime: 0,
                    file_size: 0,
                    norms: PerSourceText {
                        name: crate::search_norm::normalize_for_match(text),
                        ..PerSourceText::default()
                    },
                };
                upsert_doc(&writer, fts.fields(), &doc)?;
            }
            writer.commit()?;
            fts.reload_reader()
        })
        .unwrap();
    }

    #[test]
    fn ignored_epub_candidates_do_not_consume_global_hit_limit() {
        let (_tmp, _meta, fts) = setup();
        let fav = Uuid::new_v4();
        retry_tantivy_permission_denied(|| {
            let mut writer = fts.writer()?;
            for (path, text) in [
                ("c:/books/aaa.EPUB", "target target target"),
                ("c:/books/aab.epub", "target target target"),
                ("c:/books/aac.epub", "target target target"),
                ("c:/books/zzz.pdf", "target"),
            ] {
                upsert_doc(
                    &writer,
                    fts.fields(),
                    &IndexDoc {
                        path: path.into(),
                        container: Container::Fs,
                        zip_entry: String::new(),
                        favorite_id: fav,
                        kind: IndexKind::Pdf,
                        mtime: 0,
                        file_size: 0,
                        norms: PerSourceText {
                            name: crate::search_norm::normalize_for_match(text),
                            ..PerSourceText::default()
                        },
                    },
                )?;
            }
            writer.commit()?;
            fts.reload_reader()
        })
        .unwrap();

        let scope = SearchScope {
            ignore_epub: true,
            ..Default::default()
        };
        let (raw_tx, raw_rx) = crossbeam_channel::unbounded();
        run_with_hit_limit(
            "target",
            &[fav],
            &SearchScope::default(),
            &fts,
            &AtomicBool::new(false),
            &raw_tx,
            None,
            1,
        );
        let raw_hit = raw_rx.try_iter().find_map(|event| match event {
            SearchStreamEvent::Batch { hits, .. } => hits.into_iter().next(),
            _ => None,
        });
        assert!(raw_hit.is_some_and(|hit| is_epub_search_path(&hit.path)));
        let (tx, rx) = crossbeam_channel::unbounded();
        run_with_hit_limit(
            "target",
            &[fav],
            &scope,
            &fts,
            &AtomicBool::new(false),
            &tx,
            None,
            1,
        );
        let hits = rx
            .try_iter()
            .filter_map(|event| match event {
                SearchStreamEvent::Batch { hits, .. } => Some(hits),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "c:/books/zzz.pdf");
    }

    #[test]
    fn progress_batch_is_sent_for_every_candidate_page_even_when_excluded() {
        let (_tmp, _meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest_many_for_progress(&fts, fav, PAGE_SIZE + 1, "sunset genshin");

        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = AtomicBool::new(false);
        run(
            "sunset -genshin",
            &[fav],
            &SearchScope::default(),
            &fts,
            &cancel,
            &tx,
            None,
        );
        drop(tx);

        let progress: Vec<(usize, usize, usize)> = rx
            .try_iter()
            .filter_map(|event| match event {
                SearchStreamEvent::Batch {
                    hits,
                    scanned_candidates,
                    valid_hits,
                    ..
                } => Some((hits.len(), scanned_candidates, valid_hits)),
                _ => None,
            })
            .collect();
        assert_eq!(
            progress,
            vec![(0, PAGE_SIZE, 0), (0, PAGE_SIZE + 1, 0)],
            "post-filter で全滅しても各 candidate page の進捗を通知する"
        );
    }

    #[test]
    fn event_wake_runs_only_after_successful_send() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let wake_count = Arc::new(AtomicUsize::new(0));
        let wake_count_cl = Arc::clone(&wake_count);
        let wake: SearchWake = Arc::new(move || {
            wake_count_cl.fetch_add(1, Ordering::Relaxed);
        });

        assert!(send_event(
            &tx,
            Some(wake.as_ref()),
            SearchStreamEvent::Batch {
                hits: Vec::new(),
                scanned_candidates: PAGE_SIZE,
                valid_hits: 0,
                visibility_notice: None,
            },
        ));
        assert!(matches!(
            rx.recv().unwrap(),
            SearchStreamEvent::Batch { .. }
        ));
        assert_eq!(wake_count.load(Ordering::Relaxed), 1);

        drop(rx);
        assert!(!send_event(
            &tx,
            Some(wake.as_ref()),
            SearchStreamEvent::Done {
                truncated: false,
                reason: DoneReason::Cancelled,
            },
        ));
        assert_eq!(
            wake_count.load(Ordering::Relaxed),
            1,
            "receiver drop 後の旧 worker は UI owner を起こさない"
        );
    }

    #[test]
    fn empty_query_rejected() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "夕焼け");
        let (hits, reason, _) = collect_events("", &[fav], &fts);
        assert!(hits.is_empty());
        assert_eq!(reason, DoneReason::RejectedQuery(RejectReason::Empty));
    }

    #[test]
    fn single_char_cjk_rejected() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "夕焼け");
        let (hits, reason, _) = collect_events("夕", &[fav], &fts);
        assert!(hits.is_empty());
        assert_eq!(reason, rejected_short_terms(&["夕"]));
    }

    #[test]
    fn two_char_cjk_ok() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "夕焼け 海辺");
        let (hits, reason, _) = collect_events("夕焼", &[fav], &fts);
        assert_eq!(reason, DoneReason::Complete);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].file_size,
            Some(0),
            "STORED zero is a known zero-byte size, not Unknown"
        );
    }

    #[test]
    fn ascii_two_char_rejected() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "sd photo");
        let (hits, reason, _) = collect_events("sd", &[fav], &fts);
        assert!(hits.is_empty());
        assert_eq!(reason, rejected_short_terms(&["sd"]));
    }

    #[test]
    fn ascii_three_char_ok() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "stable diffusion photo");
        let (hits, reason, _) = collect_events("sdx", &[fav], &fts);
        // "sdx" はヒットしないが TooShort では弾かれない
        assert_eq!(reason, DoneReason::Complete);
        assert_eq!(hits.len(), 0);
    }

    #[test]
    fn not_only_query_rejected() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "夕焼け");
        let (hits, reason, _) = collect_events("-夕焼け", &[fav], &fts);
        assert!(hits.is_empty());
        assert_eq!(reason, DoneReason::RejectedQuery(RejectReason::NotOnly));
    }

    #[test]
    fn and_query_matches_single_doc() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "夕焼け 海辺");
        ingest(&meta, &fts, fav, "c:/b.jpg", "夕焼け 山頂");
        ingest(&meta, &fts, fav, "c:/c.jpg", "海辺 砂浜");

        let (hits, reason, _) = collect_events("夕焼け 海辺", &[fav], &fts);
        assert_eq!(reason, DoneReason::Complete);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "c:/a.jpg");
    }

    #[test]
    fn not_query_excludes() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "夕焼け 海辺");
        ingest(&meta, &fts, fav, "c:/b.jpg", "夕焼け 山頂");

        // "夕焼け -山頂" → a のみ
        let (hits, reason, _) = collect_events("夕焼け -山頂", &[fav], &fts);
        assert_eq!(reason, DoneReason::Complete);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "c:/a.jpg");
    }

    #[test]
    fn phrase_query_respects_order() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        // phrase = 連続する文字列。bigram AND だけでは偽陽性になり、
        // post-filter (search_query::matches) で最終判定される。
        ingest(&meta, &fts, fav, "c:/a.jpg", "hello world");
        ingest(&meta, &fts, fav, "c:/b.jpg", "world hello"); // 別順序
        let (hits, _, _) = collect_events(r#""hello world""#, &[fav], &fts);
        // post-filter が phrase を正確に見るので a のみ
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "c:/a.jpg");
    }

    #[test]
    fn favorite_scope_filters_results() {
        let (_tmp, meta, fts) = setup();
        let fav_a = Uuid::new_v4();
        let fav_b = Uuid::new_v4();
        ingest(&meta, &fts, fav_a, "c:/a.jpg", "夕焼け");
        ingest(&meta, &fts, fav_b, "c:/b.jpg", "夕焼け");

        // fav_a のみをスコープにする
        let (hits, _, _) = collect_events("夕焼け", &[fav_a], &fts);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "c:/a.jpg");
    }

    #[test]
    fn cancel_stops_early() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        for i in 0..30 {
            ingest(&meta, &fts, fav, &format!("c:/{:03}.jpg", i), "夕焼け");
        }

        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = AtomicBool::new(true);
        let scope = SearchScope::default();
        run("夕焼け", &[fav], &scope, &fts, &cancel, &tx, None);
        drop(tx);
        let mut reason = None;
        while let Ok(ev) = rx.recv() {
            if let SearchStreamEvent::Done { reason: r, .. } = ev {
                reason = Some(r);
            }
        }
        assert_eq!(reason, Some(DoneReason::Cancelled));
    }

    #[test]
    fn multi_include_tokens_hit_even_if_text_order_differs() {
        // Codex 6 回目指摘 #1 回帰: トークン間の距離・順序が違う doc もヒット
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "夕焼け、海辺、夏の思い出");
        ingest(&meta, &fts, fav, "c:/b.jpg", "海辺の朝 夕焼けは見られず");
        ingest(&meta, &fts, fav, "c:/c.jpg", "夕焼けだけ");

        let (hits, reason, _) = collect_events("夕焼け 海辺", &[fav], &fts);
        assert_eq!(reason, DoneReason::Complete);
        let paths: Vec<_> = hits.iter().map(|h| h.path.as_str()).collect();
        assert!(
            paths.contains(&"c:/a.jpg"),
            "句読点を挟んで含む doc もヒット"
        );
        assert!(paths.contains(&"c:/b.jpg"), "逆順で含む doc もヒット");
        assert!(!paths.contains(&"c:/c.jpg"), "片方のみは除外");
    }

    #[test]
    fn short_token_in_multi_token_query_rejected() {
        // Codex 6 回目指摘 #2 回帰: トークン単位の min-length。
        // "夕 海" は "夕焼け 海辺" と合計 4 文字だが、各トークンが 1 文字なので TooShort。
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "夕焼け 海辺");
        let (hits, reason, _) = collect_events("夕 海", &[fav], &fts);
        assert!(hits.is_empty());
        assert_eq!(reason, rejected_short_terms(&["夕", "海"]));
    }

    #[test]
    fn ascii_multi_short_tokens_rejected() {
        // 英字 2 文字語だけでは AND の候補を十分絞る語が無い。
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "sd ai photo");
        let (hits, reason, _) = collect_events("sd ai", &[fav], &fts);
        assert!(hits.is_empty());
        assert_eq!(reason, rejected_short_terms(&["sd", "ai"]));
    }

    #[test]
    fn short_hash_query_passes_min_length_for_tag_view_bridge() {
        // 1 文字タグ名 `#a` (合計 2 文字) は通常の ASCII 3 文字制約から免除される。
        // FTS タグ検索は廃止済みだが、タグビュー誘導のため `#tag` 入力自体は拒否しない。
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let key = normalize_path(&PathBuf::from("c:/tag.jpg"));
        let norms = PerSourceText {
            name: crate::search_norm::normalize_for_match("tag.jpg"),
            tags: "#a".to_string(),
            ..PerSourceText::default()
        };
        meta.upsert_meta_ok(
            &key,
            fav,
            &PathBuf::from("C:/"),
            crate::fts_index::IndexKind::Image,
            0,
            0,
        )
        .unwrap();
        {
            let mut w = fts.writer().unwrap();
            crate::fts_index::upsert_doc(
                &w,
                fts.fields(),
                &crate::fts_index::IndexDoc {
                    path: key.clone(),
                    container: crate::fts_index::Container::Fs,
                    zip_entry: String::new(),
                    favorite_id: fav,
                    kind: crate::fts_index::IndexKind::Image,
                    mtime: 0,
                    file_size: 0,
                    norms,
                },
            )
            .unwrap();
            w.commit().unwrap();
        }
        fts.reload_reader().unwrap();

        // Codex P2 回帰: 旧実装は #a を ASCII 2 文字として TooShort 拒否していた
        let (hits, reason, _) = collect_events("#a", &[fav], &fts);
        assert_eq!(
            reason,
            DoneReason::Complete,
            "短いタグ名は最小長で弾かないこと"
        );
        assert!(
            hits.is_empty(),
            "FTS tags は閉じているので #a 自体ではヒットしない"
        );
    }

    #[test]
    fn empty_favorite_ids_returns_complete_not_all_favorites() {
        // Codex 6 回目指摘 #3 回帰: favorite_ids 空 → 全件検索事故にならず即完了
        let (_tmp, meta, fts) = setup();
        let fav_a = Uuid::new_v4();
        ingest(&meta, &fts, fav_a, "c:/a.jpg", "夕焼け");

        // 対象 favorite が 0 件 → 空結果 + Complete で返る
        let (hits, reason, _) = collect_events("夕焼け", &[], &fts);
        assert!(hits.is_empty(), "favorite_ids 空なら結果は空");
        assert_eq!(reason, DoneReason::Complete);
    }

    #[test]
    fn deleted_path_no_longer_returned_after_delete_term() {
        // INDEX_VERSION=6: SQLite 側の status フィルタは廃止。Tantivy delete_term + commit
        // 後の reload で結果から消えることを確認する (削除直後の短い窓は許容範囲)。
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let key = normalize_path(&PathBuf::from("c:/a.jpg"));
        let norms = PerSourceText {
            name: crate::search_norm::normalize_for_match("夕焼け"),
            ..PerSourceText::default()
        };
        meta.upsert_meta_ok(&key, fav, &PathBuf::from("C:/"), IndexKind::Image, 0, 0)
            .unwrap();
        {
            let mut w = fts.writer().unwrap();
            upsert_doc(
                &w,
                fts.fields(),
                &IndexDoc {
                    path: key.clone(),
                    container: Container::Fs,
                    zip_entry: String::new(),
                    favorite_id: fav,
                    kind: IndexKind::Image,
                    mtime: 0,
                    file_size: 0,
                    norms,
                },
            )
            .unwrap();
            w.commit().unwrap();
        }
        fts.reload_reader().unwrap();

        // Tantivy delete_term + commit + reader reload
        {
            let mut w = fts.writer().unwrap();
            crate::fts_index::delete_doc(&w, fts.fields(), &key);
            w.commit().unwrap();
        }
        meta.delete_paths(&[key]).unwrap();
        fts.reload_reader().unwrap();

        let (hits, _, _) = collect_events("夕焼け", &[fav], &fts);
        assert!(hits.is_empty(), "delete + commit 後は結果から消える");
    }
}
