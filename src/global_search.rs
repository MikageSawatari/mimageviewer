//! Ctrl+G グローバルメタ検索の streaming クエリワーカー。
//!
//! docs/search-expansion-design.md §9.1 + §10.4 に準拠する。
//!
//! ## 動作
//!
//! 1. クエリ文字列を `search_query::parse` で AST (Token リスト) にする
//! 2. 最小長ポリシー + NOT-only 禁止チェック (Ctrl+G 向け)
//! 3. 正のトークンを集約し、bigram BooleanQuery を構築
//! 4. **Searcher snapshot を固定** (§9.1 ステップ 4, Codex 3 回目指摘 #2)
//! 5. `TopDocs::with_limit(PAGE_SIZE).and_offset(offset)` でページング取得
//! 6. 各ページで `fts_meta.lookup_all_text_norm` で全文を一括取得
//! 7. `search_query::matches` で phrase / NOT / AND を最終判定 (§4.3)
//! 8. post-filter 通過した結果を `SearchStreamEvent::Batch` で streaming 送信
//! 9. HARD_MAX 到達 / 候補使い切り / cancel のどれかで終了
//!
//! ## UI との接続
//!
//! 呼び出し側は別スレッドでこの関数を実行し、`crossbeam_channel::Receiver` で
//! 結果を受け取る。UI 側は毎フレーム `try_recv` で取り出して items に append する。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::Sender;
use uuid::Uuid;

use tantivy::query::{BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::IndexRecordOption;
use tantivy::Term;

use crate::fts_index::{self, Fields, FtsIndex};
use crate::fts_meta::FtsMetaDb;
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
}

/// 検索ワーカーのエントリーポイント。別スレッドで実行する想定。
///
/// - `favorite_ids`: 検索対象のお気に入り UUID (Ctrl+G は `auto_index_metadata=true` の全部)
/// - `cancel`: ユーザ操作や新しい入力で true にされたら速やかに中断する
/// - `tx`: UI へ events を送るチャネル
pub fn run(
    query_text: &str,
    favorite_ids: &[Uuid],
    fts: &FtsIndex,
    meta_db: &FtsMetaDb,
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

    // 4. トークンをタグ/通常に分離し、それぞれ query を組み立てる。
    //    タグトークンは Tantivy の tags フィールドに対する TermQuery、
    //    通常トークンは all_text に対する bigram AND query。
    let (tag_tokens, keyword_tokens): (Vec<&Token>, Vec<&Token>) =
        tokens.iter().partition(|t| t.is_tag);

    let include_keywords: Vec<&str> = keyword_tokens
        .iter()
        .filter(|t| t.include)
        .map(|t| t.needle.as_str())
        .collect();

    let query = build_combined_query(
        fts.fields(),
        &include_keywords,
        &tag_tokens,
        favorite_ids,
    );
    let query = match query {
        Some(q) => q,
        None => {
            let _ = tx.send(SearchStreamEvent::Done {
                truncated: false,
                reason: DoneReason::RejectedQuery(RejectReason::TooShort),
            });
            return;
        }
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

        // 5b. 対応する all_text_norm を一括取得
        let paths_only: Vec<String> = page.iter().map(|(p, _)| p.clone()).collect();
        let norm_map = match meta_db.lookup_all_text_norm(&paths_only) {
            Ok(rows) => rows
                .into_iter()
                .collect::<std::collections::HashMap<_, _>>(),
            Err(e) => {
                let _ = tx.send(SearchStreamEvent::Error(format!("fts_meta lookup: {e}")));
                return;
            }
        };

        // 5c. post-filter
        let mut batch = Vec::new();
        let mut inner_truncated = false;
        let mut inner_cancelled = false;
        for (path, score) in page {
            if cancel.load(Ordering::Relaxed) {
                inner_cancelled = true;
                break;
            }
            let text = match norm_map.get(&path) {
                Some(t) => t,
                None => continue, // tombstone / race condition で取れない場合はスキップ
            };
            if search_query::matches(&tokens, text) {
                batch.push(GlobalHit { path, score });
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
    // タグトークンは最小長チェックの対象外 (exact match なので 1 文字でも OK)。
    // 通常キーワードの include トークンのみ §4.3 の最小長 (CJK: 2 / ASCII: 3) を確認。
    for t in tokens.iter().filter(|t| t.include && !t.is_tag) {
        if !has_sufficient_length(&t.needle) {
            return Err(RejectReason::TooShort);
        }
    }
    Ok(())
}

/// タグ tokens と keyword tokens を合わせた BooleanQuery を作る。
/// - include keyword: bigram AND (既存)
/// - include tag: tags フィールドの TermQuery (Must)
/// - exclude tag: tags フィールドの TermQuery (MustNot)
/// - favorite scope: favorite_ids の OR (既存)
///
/// keyword も tag も空なら None (Tantivy を叩かない方が速い)。
fn build_combined_query(
    fields: &Fields,
    include_keywords: &[&str],
    tag_tokens: &[&Token],
    favorite_ids: &[Uuid],
) -> Option<BooleanQuery> {
    if favorite_ids.is_empty() {
        return None;
    }

    let mut subs: Vec<(Occur, Box<dyn Query>)> = Vec::new();

    if !include_keywords.is_empty() {
        // 既存の bigram query を Must として埋め込む (favorite filter は後で足すので None を渡す)
        let Some(kw_q) = fts_index::build_bigram_and_query(fields, include_keywords, None) else {
            return None;
        };
        subs.push((Occur::Must, Box::new(kw_q)));
    }

    for t in tag_tokens {
        let term = Term::from_field_text(fields.tags, &t.needle.to_lowercase());
        let tq = TermQuery::new(term, IndexRecordOption::Basic);
        let occur = if t.include { Occur::Must } else { Occur::MustNot };
        subs.push((occur, Box::new(tq)));
    }

    if subs.is_empty() {
        return None;
    }

    // favorite_id scope を Must で追加
    let mut fav_subs: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(favorite_ids.len());
    for id in favorite_ids {
        let term = Term::from_field_text(fields.favorite_id, &id.to_string());
        fav_subs.push((
            Occur::Should,
            Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
        ));
    }
    subs.push((Occur::Must, Box::new(BooleanQuery::from(fav_subs))));

    Some(BooleanQuery::from(subs))
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

    fn ingest(meta: &FtsMetaDb, fts: &FtsIndex, fav: Uuid, path_str: &str, text: &str) {
        let key = normalize_path(&PathBuf::from(path_str));
        let all_text_norm = crate::search_norm::normalize_for_match(text);
        meta.mark_pending(&key, fav, &PathBuf::from("C:/"), 0, 0, &all_text_norm, "")
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
                mtime: 0,
                file_size: 0,
                name: path_str.rsplit('/').next().unwrap_or(path_str).to_string(),
                all_text: all_text_norm,
                tags: String::new(),
            },
        )
        .unwrap();
        w.commit().unwrap();
        meta.mark_ok(&[key]).unwrap();
        fts.reload_reader().unwrap();
    }

    fn collect_events(
        query: &str,
        favs: &[Uuid],
        fts: &FtsIndex,
        meta: &FtsMetaDb,
    ) -> (Vec<GlobalHit>, DoneReason, bool) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = AtomicBool::new(false);
        run(query, favs, fts, meta, &cancel, &tx);
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
        let (hits, reason, _) = collect_events("", &[fav], &fts, &meta);
        assert!(hits.is_empty());
        assert_eq!(reason, DoneReason::RejectedQuery(RejectReason::Empty));
    }

    #[test]
    fn single_char_cjk_rejected() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "夕焼け");
        let (hits, reason, _) = collect_events("夕", &[fav], &fts, &meta);
        assert!(hits.is_empty());
        assert_eq!(reason, DoneReason::RejectedQuery(RejectReason::TooShort));
    }

    #[test]
    fn two_char_cjk_ok() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "夕焼け 海辺");
        let (hits, reason, _) = collect_events("夕焼", &[fav], &fts, &meta);
        assert_eq!(reason, DoneReason::Complete);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn ascii_two_char_rejected() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "sd photo");
        let (hits, reason, _) = collect_events("sd", &[fav], &fts, &meta);
        assert!(hits.is_empty());
        assert_eq!(reason, DoneReason::RejectedQuery(RejectReason::TooShort));
    }

    #[test]
    fn ascii_three_char_ok() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "stable diffusion photo");
        let (hits, reason, _) = collect_events("sdx", &[fav], &fts, &meta);
        // "sdx" はヒットしないが TooShort では弾かれない
        assert_eq!(reason, DoneReason::Complete);
        assert_eq!(hits.len(), 0);
    }

    #[test]
    fn not_only_query_rejected() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "夕焼け");
        let (hits, reason, _) = collect_events("-夕焼け", &[fav], &fts, &meta);
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

        let (hits, reason, _) = collect_events("夕焼け 海辺", &[fav], &fts, &meta);
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
        let (hits, reason, _) = collect_events("夕焼け -山頂", &[fav], &fts, &meta);
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
        let (hits, _, _) = collect_events(r#""hello world""#, &[fav], &fts, &meta);
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
        let (hits, _, _) = collect_events("夕焼け", &[fav_a], &fts, &meta);
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
        run("夕焼け", &[fav], &fts, &meta, &cancel, &tx);
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

        let (hits, reason, _) = collect_events("夕焼け 海辺", &[fav], &fts, &meta);
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
        let (hits, reason, _) = collect_events("夕 海", &[fav], &fts, &meta);
        assert!(hits.is_empty());
        assert_eq!(reason, DoneReason::RejectedQuery(RejectReason::TooShort));
    }

    #[test]
    fn ascii_multi_short_tokens_rejected() {
        // 英字 2 文字トークンの複数組 (`sd ai`) は各 token が ASCII 3 未満なので TooShort。
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        ingest(&meta, &fts, fav, "c:/a.jpg", "sd ai photo");
        let (hits, reason, _) = collect_events("sd ai", &[fav], &fts, &meta);
        assert!(hits.is_empty());
        assert_eq!(reason, DoneReason::RejectedQuery(RejectReason::TooShort));
    }

    #[test]
    fn empty_favorite_ids_returns_complete_not_all_favorites() {
        // Codex 6 回目指摘 #3 回帰: favorite_ids 空 → 全件検索事故にならず即完了
        let (_tmp, meta, fts) = setup();
        let fav_a = Uuid::new_v4();
        ingest(&meta, &fts, fav_a, "c:/a.jpg", "夕焼け");

        // 対象 favorite が 0 件 → 空結果 + Complete で返る
        let (hits, reason, _) = collect_events("夕焼け", &[], &fts, &meta);
        assert!(hits.is_empty(), "favorite_ids 空なら結果は空");
        assert_eq!(reason, DoneReason::Complete);
    }

    #[test]
    fn tombstone_hits_not_returned() {
        let (_tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let key = normalize_path(&PathBuf::from("c:/a.jpg"));
        let all_text = crate::search_norm::normalize_for_match("夕焼け");
        meta.mark_pending(&key, fav, &PathBuf::from("C:/"), 0, 0, &all_text, "")
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
                mtime: 0,
                file_size: 0,
                name: "a.jpg".to_string(),
                all_text,
                tags: String::new(),
            },
        )
        .unwrap();
        w.commit().unwrap();
        meta.mark_ok(&[key.clone()]).unwrap();
        fts.reload_reader().unwrap();

        // tombstone 化する (Tantivy からはまだ消えていない = 検索すると候補には上がる)
        meta.mark_tombstone(&[key]).unwrap();

        let (hits, _, _) = collect_events("夕焼け", &[fav], &fts, &meta);
        // post-filter で tombstone は除外される (lookup_all_text_norm が status=3 を弾くため)
        assert!(hits.is_empty(), "tombstone は結果に出ない");
    }
}
