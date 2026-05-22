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
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::Sender;
use uuid::Uuid;

use crate::fts_index::{self, FtsIndex, IndexKind, QueryFilters, SearchTarget};
use crate::search_query::{self, Token};

/// §9.1 ステップ 4 確定方針 (プロトタイプ計測で PASS、§15.1.9)
pub const HARD_MAX: usize = 10_000;
/// ページサイズは Tantivy 側の定数と揃える。
pub use crate::fts_index::PAGE_SIZE;

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
    },
    /// 正常終了
    Done { truncated: bool, reason: DoneReason },
    /// エラー
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// クエリが空
    Empty,
    /// 1 文字クエリ (CJK) / ASCII 2 文字以下
    TooShort,
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
}

/// Ctrl+G 検索ワーカーに渡すフィルタ (§19 ドロップダウン UI と対応)。
#[derive(Debug, Clone, Default)]
pub struct SearchScope {
    /// タイプドロップダウン "画像 / PDF / 動画"。空 `Vec` は呼び出し側で弾くこと。
    pub kinds: Option<Vec<IndexKind>>,
    /// 検索対象ドロップダウン "EXIF / XMP / ..."。既定は `All`。
    pub target: SearchTarget,
    /// include トークン結合モード (docs §20)。既定は AND。
    pub mode: crate::search_query::MatchMode,
}

/// 検索ワーカーのエントリーポイント。別スレッドで実行する想定。
///
/// - `favorite_ids`: 検索対象のお気に入り UUID (Ctrl+G は `auto_index_metadata=true` の全部)
/// - `scope`: タイプ / 検索対象フィルタ (§19)。初期値は `SearchScope::default()` で全開放
/// - `cancel`: ユーザ操作や新しい入力で true にされたら速やかに中断する
/// - `tx`: UI へ events を送るチャネル
pub fn run(
    query_text: &str,
    favorite_ids: &[Uuid],
    scope: &SearchScope,
    fts: &FtsIndex,
    cancel: &AtomicBool,
    tx: &Sender<SearchStreamEvent>,
) {
    // 1. クエリパース
    let tokens = search_query::parse(query_text);

    // 2. 早期 return ポリシー
    if let Err(reason) = validate_query(query_text, &tokens) {
        let _ = tx.send(SearchStreamEvent::Done {
            truncated: false,
            reason: DoneReason::RejectedQuery(reason),
        });
        return;
    }

    // 3. 対象 favorite が 0 件なら空結果で即完了 (Codex 6 回目指摘 #3)
    // 旧実装は None を fts_index に渡して "favorite filter なし = 全件検索" となる事故があった。
    if favorite_ids.is_empty() {
        let _ = tx.send(SearchStreamEvent::Done {
            truncated: false,
            reason: DoneReason::Complete,
        });
        return;
    }

    // 4. 正の include トークンを集めて bigram クエリを組む。
    //    タグはソース別フィールド `tags` として master の per-source OR に自然に組み込まれる
    //    (target=All なら tags 含む 6 ソース OR, target=Only([Tags]) ならタグフィールドのみ)。
    let include_tokens: Vec<&str> = tokens
        .iter()
        .filter(|t| t.include)
        .map(|t| t.needle.as_str())
        .collect();
    if include_tokens.is_empty() {
        // validate_query で NOT-only は弾かれるはず
        let _ = tx.send(SearchStreamEvent::Done {
            truncated: false,
            reason: DoneReason::RejectedQuery(RejectReason::NotOnly),
        });
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
        // bigram が作れない (どこかのトークンが 1 文字等) → early return
        let _ = tx.send(SearchStreamEvent::Done {
            truncated: false,
            reason: DoneReason::RejectedQuery(RejectReason::TooShort),
        });
        return;
    };

    // 4. Searcher snapshot 固定 (§9.1 ステップ 4)
    let searcher = fts.searcher();

    // 5. ページングループ
    let mut offset = 0usize;
    let mut scanned = 0usize;
    let mut valid = 0usize;

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
                let _ = tx.send(SearchStreamEvent::Error(format!(
                    "tantivy search_page: {e}"
                )));
                return;
            }
        };
        if page.is_empty() {
            break 'paging (false, DoneReason::Complete);
        }
        scanned += page.len();

        // 5b. post-filter (token matching のみ)。削除直後の短い窓では Tantivy delete_term
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
                // STORED mtime を取り出す (日付ソート用 §5.2)。post-filter を通った
                // ヒットだけに対して呼ぶので、走査候補全件への doc fetch は発生しない。
                let mtime = fts_index::doc_mtime(&searcher, fts.fields(), addr).unwrap_or(0);
                batch.push(GlobalHit {
                    path,
                    score,
                    stars: 0,
                    mtime,
                });
                valid += 1;
                if valid >= HARD_MAX {
                    inner_truncated = true;
                    break;
                }
            }
        }

        if !batch.is_empty() || offset % (PAGE_SIZE * 4) == 0 {
            // 進捗だけでも定期的に投げる (空バッチでも 4 ページ毎に 1 回)
            let _ = tx.send(SearchStreamEvent::Batch {
                hits: batch,
                scanned_candidates: scanned,
                valid_hits: valid,
            });
        }

        if inner_truncated {
            break 'paging (true, DoneReason::TruncatedAtMax);
        }
        if inner_cancelled {
            break 'paging (false, DoneReason::Cancelled);
        }
        offset += PAGE_SIZE;
    };

    let _ = tx.send(SearchStreamEvent::Done {
        truncated,
        reason: final_reason,
    });
}

// -----------------------------------------------------------------------
// 内部ヘルパー
// -----------------------------------------------------------------------

/// 最小長ポリシー + NOT-only 拒否の判定 (§9.1 ステップ 2-3)。
/// 最小長は **各 include トークン単位** で確認する (Codex 6 回目指摘 #2)。
///
/// 旧実装は `join(" ")` した連結文字列の長さで判定していたため、`a b c` や
/// `夕 海` のような短すぎるトークン群が合計長で通過してしまう問題があった。
fn validate_query(query_text: &str, tokens: &[Token]) -> Result<(), RejectReason> {
    if query_text.trim().is_empty() || tokens.is_empty() {
        return Err(RejectReason::Empty);
    }
    // NOT-only: 正のトークンが 1 つも無ければ拒否
    if !tokens.iter().any(|t| t.include) {
        return Err(RejectReason::NotOnly);
    }
    // タグトークンは最小長チェックの対象外 — `#a` のような 1 文字タグ名でも
    // bigram (`#a` 自体) が 1 つだけ生成されて Tantivy で引け、post-filter も
    // substring 一致で通る。`#` プレフィックス込みなので最低 2 文字は確保される。
    // 通常キーワードの include トークンのみ §4.3 の最小長 (CJK: 2 / ASCII: 3) を確認。
    for t in tokens.iter().filter(|t| t.include && !t.is_tag) {
        if !has_sufficient_length(&t.needle) {
            return Err(RejectReason::TooShort);
        }
    }
    Ok(())
}

/// §4.3 最小クエリ長ポリシー。
/// - CJK を 1 文字でも含む → 2 文字以上で OK
/// - それ以外 (ASCII / 数字 / 記号のみ) → 3 文字以上
fn has_sufficient_length(text: &str) -> bool {
    let trimmed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let char_count = trimmed.chars().count();
    if char_count == 0 {
        return false;
    }
    let contains_cjk = trimmed.chars().any(is_cjk);
    if contains_cjk {
        char_count >= 2
    } else {
        char_count >= 3
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
    pub fn as_user_message(self) -> &'static str {
        match self {
            RejectReason::Empty => "検索キーワードを入力してください",
            RejectReason::TooShort => {
                "検索キーワードが短すぎます (日本語は 2 文字以上、英数字は 3 文字以上)"
            }
            RejectReason::NotOnly => "含める語を 1 つ以上入力してください (除外だけの検索は不可)",
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
    use std::sync::atomic::AtomicBool;
    use tempfile::TempDir;

    fn setup() -> (TempDir, FtsMetaDb, FtsIndex) {
        let tmp = TempDir::new().unwrap();
        let meta = FtsMetaDb::open_at(&tmp.path().join("meta.db")).unwrap();
        let fts = FtsIndex::open_at(&tmp.path().join("fts_index")).unwrap();
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
        fts.reload_reader().unwrap();
    }

    fn collect_events(
        query: &str,
        favs: &[Uuid],
        fts: &FtsIndex,
    ) -> (Vec<GlobalHit>, DoneReason, bool) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = AtomicBool::new(false);
        let scope = SearchScope::default();
        run(query, favs, &scope, fts, &cancel, &tx);
        drop(tx);
        let mut all_hits = Vec::new();
        let mut reason = DoneReason::Complete;
        let mut truncated = false;
        while let Ok(ev) = rx.recv() {
            match ev {
                SearchStreamEvent::Batch { hits, .. } => all_hits.extend(hits),
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
        (all_hits, reason, truncated)
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
        assert_eq!(reason, DoneReason::RejectedQuery(RejectReason::TooShort));
    }

    #[test]
    fn two_char_cjk_ok() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "夕焼け 海辺");
        let (hits, reason, _) = collect_events("夕焼", &[fav], &fts);
        assert_eq!(reason, DoneReason::Complete);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn ascii_two_char_rejected() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "sd photo");
        let (hits, reason, _) = collect_events("sd", &[fav], &fts);
        assert!(hits.is_empty());
        assert_eq!(reason, DoneReason::RejectedQuery(RejectReason::TooShort));
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
        run("夕焼け", &[fav], &scope, &fts, &cancel, &tx);
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
        assert_eq!(reason, DoneReason::RejectedQuery(RejectReason::TooShort));
    }

    #[test]
    fn ascii_multi_short_tokens_rejected() {
        // 英字 2 文字トークンの複数組 (`sd ai`) は各 token が ASCII 3 未満なので TooShort。
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "sd ai photo");
        let (hits, reason, _) = collect_events("sd ai", &[fav], &fts);
        assert!(hits.is_empty());
        assert_eq!(reason, DoneReason::RejectedQuery(RejectReason::TooShort));
    }

    #[test]
    fn short_tag_token_passes_min_length() {
        // 1 文字タグ名 `#a` (合計 2 文字) は通常の ASCII 3 文字制約から免除される。
        // ドキュメント (search.html §4) と整合させるため、tag トークンは常に通す。
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        // tags フィールドにだけ `#a` を入れる
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
        assert_ne!(
            reason,
            DoneReason::RejectedQuery(RejectReason::TooShort),
            "短いタグ名は最小長で弾かないこと"
        );
        assert_eq!(hits.len(), 1, "短い tag でもヒットすること");
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
