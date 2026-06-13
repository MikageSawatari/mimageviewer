//! Ctrl+G グローバルメタ検索の end-to-end 統合テスト (v0.8.0)。
//!
//! ## 目的
//!
//! 検索機能は「fixture フォルダに画像を置く → IndexerManager が初期スキャン →
//! Ctrl+G で検索 → 期待ヒットが返る」という長いパイプラインで成立する。
//! 各部の単体テストは `fts_index.rs` / `fts_meta.rs` 等にあるが、ここでは
//! **全部を繋いで動かす** レベルの回帰を張る。特に:
//!
//! - notify-rs の FsWatcher が初期スキャン後の新規ファイル / 削除 / リネームを
//!   検知して反映できること (v0.8.0 以前は search_watcher のイベント解釈の
//!   単体テストしかなく、実 I/O 経路は未カバー)。
//! - `spawn_search` が favorite フィルタを守ること (他のお気に入りから漏れない)。
//! - クエリ rejection のルール (最小長・NOT-only) が守られていること。
//!
//! ## 実行
//!
//! ```
//! cargo test --test search_metadata_e2e -- --nocapture
//! ```
//!
//! notify-rs の発火は Windows の `ReadDirectoryChangesW` 挙動に依存するため、
//! タイムアウトを長めに取っている (`common::FS_EVENT_TIMEOUT = 8s`)。
//! ローカルで不安定なら `POLL_INTERVAL` を短くする。

mod common;

use common::{
    FS_EVENT_TIMEOUT, FixtureRoot, collect_search_hits, delete_file, make_favorite, normalize_path,
    rename_file, run_search_expecting_done, start_indexer_at, wait_for_search_hits,
    wait_meta_absent, wait_meta_contains, wait_scan_done, wait_until, write_png_plain,
    write_png_with_text,
};
use mimageviewer::fts_meta::FtsMetaDb;
use mimageviewer::global_search::{DoneReason, RejectReason, SearchStreamEvent};
use std::fs;

// -----------------------------------------------------------------------
// 初期スキャン + 基本検索
// -----------------------------------------------------------------------

/// A1111 プロンプトのキーワードが初期スキャン完了後に検索ヒットすること。
/// パイプライン全体 (PNG tEXt 抽出 → ingest_text → Tantivy bigram → post-filter) が
/// 繋がっていることの基礎テスト。
#[test]
fn initial_scan_hits_embedded_prompt() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    // "mountain landscape" を含むプロンプト
    let a_path = root.path().join("a.png");
    write_png_with_text(&a_path, "a photo of a mountain landscape at sunset");
    // 別のキーワード (ヒットしてはいけない対照群)
    write_png_with_text(&root.path().join("b.png"), "a portrait of a cat indoors");

    let fav = make_favorite("A", root.path());
    let mgr = start_indexer_at(data.path(), &[fav.clone()]);
    wait_scan_done(&mgr, fav.id);

    // 初期スキャン完了 = walker 終了だが、ingest_worker は `BATCH_FLUSH_INTERVAL` /
    // flush で commit するため Tantivy reader が追いつくまで若干のラグがある。
    // `wait_meta_contains` で meta_db の status=Ok を確認 → commit 済みを保証。
    let meta_db =
        mimageviewer::fts_meta::FtsMetaDb::open_at(&data.path().join("fts_meta.db")).unwrap();
    wait_meta_contains(&meta_db, &normalize_path(&a_path));

    // Tantivy reader は `ReloadPolicy::OnCommitWithDelay` で非同期に reload されるため、
    // commit 直後に検索すると空を返す可能性がある。polling で期待ヒットを待つ。
    let hits = wait_for_search_hits(
        &mgr,
        "mountain",
        &[fav.id],
        |h| h.iter().any(|g| g.path.contains("a.png")),
        FS_EVENT_TIMEOUT,
        "search finds 'mountain' in a.png",
    );
    assert_eq!(
        hits.len(),
        1,
        "期待: 'mountain' は a.png の 1 件のみヒット (hits={hits:?})"
    );
}

/// ファイル名だけ (tEXt メタなし) でも検索できること。
#[test]
fn initial_scan_hits_by_filename() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    write_png_plain(&root.path().join("birthday_party.png"));
    write_png_plain(&root.path().join("sunset.png"));

    let fav = make_favorite("A", root.path());
    let mgr = start_indexer_at(data.path(), &[fav.clone()]);
    wait_scan_done(&mgr, fav.id);

    let hits = wait_for_search_hits(
        &mgr,
        "birthday",
        &[fav.id],
        |h| h.iter().any(|g| g.path.contains("birthday_party")),
        FS_EVENT_TIMEOUT,
        "search finds 'birthday' by filename",
    );
    assert_eq!(hits.len(), 1);
}

/// 動画ファイルも Ctrl+G のメタ索引対象になること。ただしタグ刷新 (v1.4.0) 以降、
/// サイドカー XMP `dc:subject` の mIV `#` タグは FTS 索引へ投影されない
/// (タグは tags.db / タグビュー専有)。よって動画自体はファイル名等で検索できるが、
/// `#` タグ文字列では検索ヒットしない。`ingest_worker` 側ユニットテスト
/// `video_file_with_sidecar_tags_ingests_but_tag_not_in_fts` の e2e 版回帰ガード。
/// 実動画を fixture に持たずに済むよう、コンテナメタではなく `.xmp` サイドカー経路を使う。
#[test]
fn initial_scan_indexes_video_but_sidecar_tag_not_in_fts() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    let video_path = root.path().join("tagged_movie.mp4");
    fs::write(
        &video_path,
        b"not a real mp4, metadata probe should fail gracefully",
    )
    .expect("write fake video");
    fs::write(
        root.path().join("tagged_movie.mp4.xmp"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
           xmlns:dc="http://purl.org/dc/elements/1.1/">
    <rdf:Description>
      <dc:subject>
        <rdf:Bag>
          <rdf:li>#video_sidecar_marker</rdf:li>
        </rdf:Bag>
      </dc:subject>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>"#,
    )
    .expect("write video sidecar");

    let fav = make_favorite("A", root.path());
    let mgr = start_indexer_at(data.path(), &[fav.clone()]);
    wait_scan_done(&mgr, fav.id);

    let meta_db =
        mimageviewer::fts_meta::FtsMetaDb::open_at(&data.path().join("fts_meta.db")).unwrap();
    let key = normalize_path(&video_path);
    wait_meta_contains(&meta_db, &key);
    let row = meta_db.get(&key).unwrap().unwrap();
    assert_eq!(row.kind, mimageviewer::fts_index::IndexKind::Video);

    // 動画はファイル名で検索できる (= FTS に索引済みで、reader が当該ドキュメントの
    // 最新コミットを見えている)。これでこの後のタグ 0 件が「索引漏れ」ではなく
    // 「タグ未投影」であることを保証する。
    let by_name = wait_for_search_hits(
        &mgr,
        "tagged",
        &[fav.id],
        |h| h.iter().any(|g| g.path.contains("tagged_movie.mp4")),
        FS_EVENT_TIMEOUT,
        "search finds video by filename",
    );
    assert_eq!(by_name.len(), 1);

    // タグ刷新後: サイドカー `#` タグは FTS へ投影されないので 0 件 (旧挙動は 1 件)。
    // タグの検索は tags.db / タグビュー側で行う。
    let tag_hits = collect_search_hits(&mgr, "video_sidecar_marker", &[fav.id]);
    assert!(
        tag_hits.is_empty(),
        "サイドカー # タグは FTS 非投影のはず (tag_hits={tag_hits:?})"
    );
}

// -----------------------------------------------------------------------
// notify-rs 監視系
// -----------------------------------------------------------------------

/// 初期スキャン完了後に新しい画像を置くと、notify-rs → debounce → ingest が
/// 走って検索に現れること。v0.8.0 以前はこの経路の回帰が取れていなかった。
#[test]
fn watcher_picks_up_new_file() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    write_png_with_text(&root.path().join("existing.png"), "baseline content");

    let fav = make_favorite("A", root.path());
    let mgr = start_indexer_at(data.path(), &[fav.clone()]);
    wait_scan_done(&mgr, fav.id);

    // 初期スキャン完了を確認してから新規ファイルを置く (watcher が拾うか見る)
    let newly_added = root.path().join("watcher_added.png");
    write_png_with_text(&newly_added, "newly created for watcher test unique_xyz42");

    // meta_db 側が上がるのを待つ (Tantivy commit もここで起きる)
    let meta_db = FtsMetaDb::open_at(&data.path().join("fts_meta.db")).unwrap();
    let key = normalize_path(&newly_added);
    wait_meta_contains(&meta_db, &key);

    // Tantivy reader の reload は commit で自動反映される (ReloadPolicy::OnCommitWithDelay)
    // だが delay があるので少し待ってから検索する
    wait_until(
        || {
            let hits = collect_search_hits(&mgr, "unique_xyz42", &[fav.id]);
            hits.iter().any(|h| h.path.contains("watcher_added"))
        },
        FS_EVENT_TIMEOUT,
        "search reflects watcher_added.png",
    );
}

/// 削除が notify-rs 経由で反映されること。
#[test]
fn watcher_picks_up_deletion() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    let victim = root.path().join("about_to_delete.png");
    write_png_with_text(&victim, "willbedeleted_marker_abc123");
    write_png_with_text(
        &root.path().join("keeper.png"),
        "keeper keeper keeper content",
    );

    let fav = make_favorite("A", root.path());
    let mgr = start_indexer_at(data.path(), &[fav.clone()]);
    wait_scan_done(&mgr, fav.id);

    // 削除前に検索できることを確認 (Tantivy reader reload 待ち)
    let hits = wait_for_search_hits(
        &mgr,
        "willbedeleted_marker_abc123",
        &[fav.id],
        |h| h.len() == 1,
        FS_EVENT_TIMEOUT,
        "search finds victim before deletion",
    );
    assert_eq!(hits.len(), 1, "削除前はヒットするはず");

    // 削除
    delete_file(&victim);

    // meta_db 側が消える / tombstone になるのを待つ
    let meta_db = FtsMetaDb::open_at(&data.path().join("fts_meta.db")).unwrap();
    let key = normalize_path(&victim);
    wait_meta_absent(&meta_db, &key);

    // 検索から消えるのを待つ (Tantivy delete + commit 後)
    wait_until(
        || {
            let hits = collect_search_hits(&mgr, "willbedeleted_marker_abc123", &[fav.id]);
            hits.is_empty()
        },
        FS_EVENT_TIMEOUT,
        "search no longer returns deleted file",
    );
}

/// リネームが反映されること (notify-rs は rename を Remove+Create 2 イベントで届けるケースが多い)。
///
/// ⚠️ **既知の動作**: Windows の `ReadDirectoryChangesW` は rename を
/// `ModifyKind::Name(From/To)` で届ける。現行の `search_watcher::absorb_event` は
/// `Modify(_)` を一律 `Upsert` に変換しているため、rename 元パスが `Upsert` として
/// ingest に回される。`apply_single_change` の Upsert 分岐では
/// `build_candidate_from_path` が「ファイルが存在しない」ので `None` を返して no-op し、
/// 旧パスの fts_meta 行が残ったままになる。
///
/// このテストは **意図的な回帰ガード**: rename 時に旧パスが消える挙動を期待値として
/// 書いてある。2026-04 に二段構えで修正:
///
/// 1. `search_watcher::absorb_event` で `ModifyKind::Name(RenameMode::From)` を
///    `ChangeKind::Remove` にマップ (主経路)。
/// 2. `indexer_supervisor::apply_single_change::Upsert` で `build_candidate_from_path`
///    が `None` を返したら `Remove` にフォールバック (保険、`RenameMode::Any` など
///    From が取りこぼされたプラットフォーム挙動をカバー)。
#[test]
fn watcher_picks_up_rename() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    let old_name = root.path().join("old_name.png");
    let new_name = root.path().join("new_name.png");
    write_png_with_text(&old_name, "renaming_test_marker_xyz789");

    let fav = make_favorite("A", root.path());
    let mgr = start_indexer_at(data.path(), &[fav.clone()]);
    wait_scan_done(&mgr, fav.id);

    rename_file(&old_name, &new_name);

    let meta_db = FtsMetaDb::open_at(&data.path().join("fts_meta.db")).unwrap();
    let old_key = normalize_path(&old_name);
    wait_meta_absent(&meta_db, &old_key);

    // 新しい名前で検索できるようになるまで待つ
    wait_until(
        || {
            let hits = collect_search_hits(&mgr, "renaming_test_marker_xyz789", &[fav.id]);
            hits.iter().any(|h| h.path.contains("new_name"))
        },
        FS_EVENT_TIMEOUT,
        "search reflects renamed file",
    );

    // 旧パスは検索結果に残っていないこと (Remove イベントで Tantivy から消えているはず)
    let hits = collect_search_hits(&mgr, "renaming_test_marker_xyz789", &[fav.id]);
    assert!(
        !hits.iter().any(|h| h.path.contains("old_name")),
        "旧パスは検索結果から消えているはず: {hits:?}"
    );
}

// -----------------------------------------------------------------------
// お気に入りフィルタ
// -----------------------------------------------------------------------

/// お気に入り A 限定の検索は、同じキーワードを含むお気に入り B のファイルを拾わないこと。
#[test]
fn search_respects_favorite_filter() {
    let data = FixtureRoot::new();
    let root_a = FixtureRoot::new();
    let root_b = FixtureRoot::new();
    // 両方に同じキーワードを含むファイルを置く
    write_png_with_text(
        &root_a.path().join("in_a.png"),
        "scoped_keyword_marker_q1w2e3",
    );
    write_png_with_text(
        &root_b.path().join("in_b.png"),
        "scoped_keyword_marker_q1w2e3",
    );

    let fav_a = make_favorite("A", root_a.path());
    let fav_b = make_favorite("B", root_b.path());
    let mgr = start_indexer_at(data.path(), &[fav_a.clone(), fav_b.clone()]);
    wait_scan_done(&mgr, fav_a.id);
    wait_scan_done(&mgr, fav_b.id);

    // A 限定で検索 (reader reload 待ち)
    let hits_a = wait_for_search_hits(
        &mgr,
        "scoped_keyword_marker_q1w2e3",
        &[fav_a.id],
        |h| h.len() == 1,
        FS_EVENT_TIMEOUT,
        "search scoped to A returns 1",
    );
    assert!(hits_a[0].path.contains("in_a"));

    // B 限定で検索
    let hits_b = wait_for_search_hits(
        &mgr,
        "scoped_keyword_marker_q1w2e3",
        &[fav_b.id],
        |h| h.len() == 1,
        FS_EVENT_TIMEOUT,
        "search scoped to B returns 1",
    );
    assert!(hits_b[0].path.contains("in_b"));

    // 両方指定すれば 2 件
    let hits_both = wait_for_search_hits(
        &mgr,
        "scoped_keyword_marker_q1w2e3",
        &[fav_a.id, fav_b.id],
        |h| h.len() == 2,
        FS_EVENT_TIMEOUT,
        "search scoped to both returns 2",
    );
    assert_eq!(hits_both.len(), 2);
}

// -----------------------------------------------------------------------
// クエリ rejection
// -----------------------------------------------------------------------

/// ASCII 2 文字以下は TooShort で reject されること (bigram index の仕様)。
#[test]
fn short_ascii_query_is_rejected() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    write_png_plain(&root.path().join("ab.png")); // "ab" でヒットさせたくても拒否される
    let fav = make_favorite("A", root.path());
    let mgr = start_indexer_at(data.path(), &[fav.clone()]);
    wait_scan_done(&mgr, fav.id);

    let ev = run_search_expecting_done(&mgr, "ab", &[fav.id]);
    match ev {
        SearchStreamEvent::Done { reason, .. } => assert!(
            matches!(reason, DoneReason::RejectedQuery(RejectReason::TooShort)),
            "reason should be TooShort, got {reason:?}"
        ),
        _ => unreachable!(),
    }
}

/// NOT-only クエリは Ctrl+G では reject されること。
#[test]
fn not_only_query_is_rejected() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    write_png_plain(&root.path().join("a.png"));
    let fav = make_favorite("A", root.path());
    let mgr = start_indexer_at(data.path(), &[fav.clone()]);
    wait_scan_done(&mgr, fav.id);

    let ev = run_search_expecting_done(&mgr, "-cat", &[fav.id]);
    match ev {
        SearchStreamEvent::Done { reason, .. } => assert!(
            matches!(reason, DoneReason::RejectedQuery(RejectReason::NotOnly)),
            "reason should be NotOnly, got {reason:?}"
        ),
        _ => unreachable!(),
    }
}

/// お気に入り 0 件の検索は空で即 Complete すること (全件検索への誤動作防止の回帰ガード)。
#[test]
fn empty_favorite_list_returns_complete() {
    let data = FixtureRoot::new();
    let mgr = start_indexer_at(data.path(), &[]);
    let ev = run_search_expecting_done(&mgr, "anything", &[]);
    match ev {
        SearchStreamEvent::Done { reason, truncated } => {
            assert!(matches!(reason, DoneReason::Complete));
            assert!(!truncated);
        }
        _ => unreachable!(),
    }
}

// -----------------------------------------------------------------------
// Streaming protocol の境界 (v0.8.x: selected/checked content-key 復元の前提)
//
// 単体側で `streaming_rebuild_preserves_selected_and_checked_by_content_key`
// (src/app.rs) が App-level でカバーしている。ここでは IndexerManager 側の
// streaming 契約 (Batch が `Done` の前に流れること、cancel で Cancelled が立つこと)
// を別ビルドの統合テストとして固定する。
// -----------------------------------------------------------------------

/// 検索ヒットが `Done` の **前に** Batch event として流れること (= UI 側の
/// streaming rebuild ロジックが呼ばれる経路が生きている)。`Done` の payload に
/// hits がぶら下がる退行が入ると、`accumulate_hit` / `replace_search_view_items`
/// が一切走らず selected 維持テストの前提が崩れる。
///
/// 注意: corpus は 12 件 (PAGE_SIZE=500 未満) なので 1 Batch で完結する。
/// **「複数 Batch でストリーミングする」**性質は本テストでは検証していない
/// (HARD_MAX 以下のヒット数は 1 Batch にまとまる仕様)。multi-batch の paging を
/// 守りたければ別途 docs ≥ PAGE_SIZE の重いテストを追加する必要がある。
#[test]
fn search_hits_arrive_as_batch_event_before_done() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    for i in 0..12 {
        write_png_with_text(
            &root.path().join(format!("photo_{i:02}.png")),
            "evening sunset over hills",
        );
    }
    let fav = make_favorite("A", root.path());
    let mgr = start_indexer_at(data.path(), &[fav.clone()]);
    wait_scan_done(&mgr, fav.id);

    wait_for_search_hits(
        &mgr,
        "sunset",
        &[fav.id],
        |h| h.len() >= 12,
        FS_EVENT_TIMEOUT,
        "search_hits_arrive_as_batch: 索引反映",
    );

    let handle = mgr.spawn_search(
        "sunset".to_string(),
        vec![fav.id],
        mimageviewer::global_search::SearchScope::default(),
    );
    let mut batches_before_done: usize = 0;
    let mut total_hits: usize = 0;
    let deadline = std::time::Instant::now() + FS_EVENT_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            panic!("Done が来ない");
        }
        match handle.rx.recv_timeout(remaining) {
            Ok(SearchStreamEvent::Batch { hits, .. }) => {
                batches_before_done += 1;
                total_hits += hits.len();
            }
            Ok(SearchStreamEvent::Done { .. }) => break,
            Ok(SearchStreamEvent::Error(e)) => panic!("error: {e}"),
            Err(_) => panic!("rx disconnected"),
        }
    }
    assert!(
        batches_before_done >= 1,
        "Done の前に少なくとも 1 つの Batch event が流れること (got {batches_before_done})"
    );
    assert_eq!(total_hits, 12, "Batch 累積で全 12 件届くこと");
}

/// 検索中に `cancel` を立てた後、有限時間で正常 (Error/disconnect ではなく) `Done` に
/// 到達して terminate すること。Cancelled 状態の即時観測は race (Tantivy が先に
/// Complete することがある) なので、ここでは "cancel 後も protocol が壊れない" だけを
/// honest に検証する。SearchHandle の Drop 経路 / 明示 cancel 経路の両方で
/// rx が必ず Done でクローズされる契約のガード。
#[test]
fn cancel_after_spawn_terminates_with_done_event() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    for i in 0..30 {
        write_png_with_text(
            &root.path().join(format!("doc_{i:03}.png")),
            "lightning storm at midnight",
        );
    }
    let fav = make_favorite("A", root.path());
    let mgr = start_indexer_at(data.path(), &[fav.clone()]);
    wait_scan_done(&mgr, fav.id);
    wait_for_search_hits(
        &mgr,
        "lightning",
        &[fav.id],
        |h| h.len() >= 5,
        FS_EVENT_TIMEOUT,
        "cancel test: 索引反映",
    );

    let handle = mgr.spawn_search(
        "lightning".to_string(),
        vec![fav.id],
        mimageviewer::global_search::SearchScope::default(),
    );
    handle
        .cancel
        .store(true, std::sync::atomic::Ordering::Relaxed);

    let deadline = std::time::Instant::now() + FS_EVENT_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            panic!("Done が来ない (cancel test)");
        }
        match handle.rx.recv_timeout(remaining) {
            Ok(SearchStreamEvent::Done { reason, .. }) => {
                // Cancelled / Complete どちらも protocol 上 valid。
                // RejectedQuery が出たら退行 (cancel 立てただけでクエリ自体は valid)。
                assert!(
                    !matches!(reason, DoneReason::RejectedQuery(_)),
                    "cancel した有効クエリで RejectedQuery が返る退行、got {reason:?}"
                );
                break;
            }
            Ok(SearchStreamEvent::Error(e)) => panic!("error during cancel: {e}"),
            Ok(SearchStreamEvent::Batch { .. }) => continue,
            Err(_) => panic!("rx disconnected before Done"),
        }
    }
}
