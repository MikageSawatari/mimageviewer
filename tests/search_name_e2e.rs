//! 名前索引 (Ctrl+S 検索用 `search_index_db`) の end-to-end 統合テスト (v0.8.0)。
//!
//! ## 目的
//!
//! 名前索引は v0.8.0 で `NameIndexSupervisor` が入り「初期バルク + notify-rs 監視」構造に
//! 統一された。それ以前はワンショットの `spawn_bulk` だけで FS 変更が索引に反映されず、
//! 「✅ 索引あり」表示がスナップショット時点の静的状態を示すに過ぎなかった
//! (2026-04 のユーザ指摘)。本ファイルでは:
//!
//! - 初期バルクで期待フォルダ/ZIP/PDF が索引化されること
//! - FsWatcher 経由で **追加された** フォルダ/ZIP/PDF が索引に載ること
//! - **削除された** エントリが索引から消えること
//! - クエリ構文 (include / exclude) が SQLite LIKE で正しく動くこと
//! - お気に入りフィルタ (favorite_roots) が漏れないこと
//!
//! を検証する。
//!
//! ## 実行
//!
//! ```
//! cargo test --test search_name_e2e -- --nocapture
//! ```

mod common;

use std::path::PathBuf;

use common::{
    FS_EVENT_TIMEOUT, FixtureRoot, make_favorite, name_index_search, start_name_index_at,
    wait_for_name_index_hits, wait_name_scan_done, write_png_plain,
};
use mimageviewer::search_index_db::IndexKind;

/// サブフォルダを掘って空の画像を置く (フォルダが画像コンテナ扱いされるように)。
fn mkdir_with_image(root: &FixtureRoot, subdir: &str) -> PathBuf {
    let dir = root.mkdir(subdir);
    write_png_plain(&dir.join("cover.png"));
    dir
}

/// 空の ZIP ファイルを作る (バイト列は ZIP の最小ヘッダ "PK\x05\x06" + 18 バイトの
/// 終端ディレクトリ)。`classify_name_index_kind` は拡張子だけで判定するはずだが、
/// 念のため実 ZIP 構造を持たせておく。
fn write_empty_zip(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    // "End of central directory record" 最小形式 (zip が空でも PK\x05\x06 で終わる)
    let empty_eocd: [u8; 22] = [
        0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    std::fs::write(path, empty_eocd).expect("write zip");
}

// -----------------------------------------------------------------------
// 初期バルク
// -----------------------------------------------------------------------

/// 初期バルクスキャンでサブフォルダと ZIP ファイルが索引化されること。
#[test]
fn initial_bulk_indexes_folders_and_zips() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    // サブフォルダ (画像入り → コンテナ扱い)
    mkdir_with_image(&root, "alpha_folder");
    mkdir_with_image(&root, "beta_gallery");
    // ZIP ファイル
    write_empty_zip(&root.path().join("charlie.zip"));

    let fav = make_favorite("A", root.path());
    let (db, handle) = start_name_index_at(data.path(), &fav);
    wait_name_scan_done(&handle);

    let roots = vec![fav.path.clone()];

    let alpha = name_index_search(&db, "alpha", &roots);
    assert_eq!(alpha.len(), 1, "'alpha' はフォルダ 1 件 (got {alpha:?})");
    assert_eq!(alpha[0].kind, IndexKind::Folder);

    let charlie = name_index_search(&db, "charlie", &roots);
    assert_eq!(charlie.len(), 1, "'charlie' は ZIP 1 件 (got {charlie:?})");
    assert_eq!(charlie[0].kind, IndexKind::ZipFile);
}

// -----------------------------------------------------------------------
// notify-rs 監視 (名前索引の "continuous" 挙動 — v0.8.0 新機能の回帰ガード)
// -----------------------------------------------------------------------

/// 初期バルク完了後に新しいサブフォルダを掘ると、watcher → supervisor → DB 反映が
/// 動いて検索にヒットするようになること。**v0.8.0 で新設された挙動**。
#[test]
fn watcher_indexes_new_subfolder() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    mkdir_with_image(&root, "initial_existing");

    let fav = make_favorite("A", root.path());
    let (db, handle) = start_name_index_at(data.path(), &fav);
    wait_name_scan_done(&handle);

    // 初期バルク完了後に新規フォルダを掘る
    mkdir_with_image(&root, "late_arrival_unique_nx8");

    let hits = wait_for_name_index_hits(
        &db,
        "late_arrival",
        &[fav.path.clone()],
        |h| !h.is_empty(),
        FS_EVENT_TIMEOUT,
        "name index picks up newly created subfolder",
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kind, IndexKind::Folder);
}

/// 初期バルク後に新しい ZIP が現れたら索引される。
#[test]
fn watcher_indexes_new_zip() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    write_empty_zip(&root.path().join("pre_existing.zip"));

    let fav = make_favorite("A", root.path());
    let (db, handle) = start_name_index_at(data.path(), &fav);
    wait_name_scan_done(&handle);

    let newly = root.path().join("brand_new_zip_marker_q1z.zip");
    write_empty_zip(&newly);

    let hits = wait_for_name_index_hits(
        &db,
        "brand_new_zip_marker_q1z",
        &[fav.path.clone()],
        |h| !h.is_empty(),
        FS_EVENT_TIMEOUT,
        "name index picks up newly created zip",
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kind, IndexKind::ZipFile);
}

/// 初期バルクでヒットしていたフォルダを削除すると、索引からも消える。
#[test]
fn watcher_removes_deleted_folder() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    let victim = mkdir_with_image(&root, "about_to_delete_abc42");
    mkdir_with_image(&root, "keeper");

    let fav = make_favorite("A", root.path());
    let (db, handle) = start_name_index_at(data.path(), &fav);
    wait_name_scan_done(&handle);

    // 削除前にヒットを確認
    let before = name_index_search(&db, "about_to_delete_abc42", &[fav.path.clone()]);
    assert_eq!(before.len(), 1, "削除前はヒットするはず");

    // フォルダごと削除
    std::fs::remove_dir_all(&victim).expect("remove victim folder");

    wait_for_name_index_hits(
        &db,
        "about_to_delete_abc42",
        &[fav.path.clone()],
        |h| h.is_empty(),
        FS_EVENT_TIMEOUT,
        "name index drops deleted folder",
    );
}

// -----------------------------------------------------------------------
// 複数お気に入り並列性 (Tantivy 制約がないので真並列)
// -----------------------------------------------------------------------

/// 2 お気に入りを同時に supervisor で走らせても、SQLite writer 競合で
/// `LockBusy` にならずに両方が initial_scan_done に到達すること。
/// メタ索引 (Tantivy) は writer 1 本制約で共有 Mutex が必要だったが、
/// 名前索引は SQLite なのでこの制約から解放されている (挙動確認テスト)。
#[test]
fn two_supervisors_complete_in_parallel() {
    let data = FixtureRoot::new();
    let root_a = FixtureRoot::new();
    let root_b = FixtureRoot::new();
    mkdir_with_image(&root_a, "in_a_alpha");
    mkdir_with_image(&root_b, "in_b_beta");

    let fav_a = make_favorite("A", root_a.path());
    let fav_b = make_favorite("B", root_b.path());

    // 同じ data_dir 配下の search_index.db を共有する (A / B 双方から書き込み)
    std::fs::create_dir_all(data.path()).ok();
    let db = std::sync::Arc::new(
        mimageviewer::search_index_db::SearchIndexDb::open_at(
            &data.path().join("search_index.db"),
        )
        .unwrap(),
    );
    let handle_a = mimageviewer::name_index_supervisor::spawn(
        fav_a.id,
        fav_a.path.clone(),
        std::sync::Arc::clone(&db),
    );
    let handle_b = mimageviewer::name_index_supervisor::spawn(
        fav_b.id,
        fav_b.path.clone(),
        std::sync::Arc::clone(&db),
    );

    wait_name_scan_done(&handle_a);
    wait_name_scan_done(&handle_b);

    // 両 favorite を指定で検索すれば 2 件ヒットするはず
    let hits_a = name_index_search(&db, "in_a_alpha", &[fav_a.path.clone()]);
    assert_eq!(hits_a.len(), 1, "A スコープで A のフォルダがヒット");

    let hits_b = name_index_search(&db, "in_b_beta", &[fav_b.path.clone()]);
    assert_eq!(hits_b.len(), 1, "B スコープで B のフォルダがヒット");

    // A スコープで検索すれば B のフォルダはヒットしない (favorite フィルタ漏れなし)
    let cross = name_index_search(&db, "in_b_beta", &[fav_a.path.clone()]);
    assert_eq!(cross.len(), 0, "A 限定で B のフォルダが漏れ出さない");
}

// -----------------------------------------------------------------------
// クエリ構文
// -----------------------------------------------------------------------

/// 除外トークン (`-keyword`) が SQLite LIKE の NOT LIKE に変換されること。
#[test]
fn query_supports_exclude_token() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    mkdir_with_image(&root, "animal_cat");
    mkdir_with_image(&root, "animal_dog");
    mkdir_with_image(&root, "plant_fern");

    let fav = make_favorite("A", root.path());
    let (db, handle) = start_name_index_at(data.path(), &fav);
    wait_name_scan_done(&handle);

    let roots = vec![fav.path.clone()];

    // `animal -cat` → animal で始まるもののうち cat を含まないもの
    let hits = name_index_search(&db, "animal -cat", &roots);
    assert_eq!(hits.len(), 1, "animal で cat でないのは dog のみ (got {hits:?})");
    assert!(hits[0].display_name.contains("dog"));
}
