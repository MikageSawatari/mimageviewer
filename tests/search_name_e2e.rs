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

// -----------------------------------------------------------------------
// Codex P2 回帰ガード
// -----------------------------------------------------------------------

/// Codex P2 #1 回帰テスト: 「空フォルダ化」で古い索引が残るバグ。
///
/// ## シナリオ
///
/// 1. `/root/P/Q (フォルダ)`, `/root/P/a.zip`, `/root/P/b.pdf` を作り初期スキャン
///    → Q / a.zip / b.pdf が索引に入る
/// 2. **アプリ停止中を模して** 3 つすべてディスクから削除 (`/root/P` は空フォルダに)
/// 3. 同じ DB で再度 `run_bulk_name_index` を走らせる (= アプリ再起動時のフルスキャン)
/// 4. 検索: Q / a.zip / b.pdf がどれもヒットしないこと
///
/// 旧実装は `if children.is_empty() { continue; }` で upsert_children の DELETE すら
/// スキップしていたため、空になった親フォルダの下の古い行が残ったまま。
#[test]
fn full_scan_removes_stale_entries_from_became_empty_folder() {
    use mimageviewer::name_bulk_indexer::run_bulk_name_index;
    use mimageviewer::search_index_db::SearchIndexDb;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    // /root/P/ に Q フォルダ + a.zip + b.pdf
    let p = root.mkdir("P");
    let q = root.mkdir("P/Q");
    write_empty_zip(&p.join("a.zip"));
    std::fs::write(p.join("b.pdf"), b"fake pdf").expect("write pdf");
    // 索引化対象になるよう Q の中に画像を置く (folder 扱い用)
    write_png_plain(&q.join("cover.png"));

    let db = Arc::new(
        SearchIndexDb::open_at(&data.path().join("search_index.db")).expect("open db"),
    );
    let cancel = AtomicBool::new(false);

    // Phase 1: 初期スキャン
    let summary1 = run_bulk_name_index(root.path(), &db, &cancel, None);
    assert!(!summary1.cancelled);

    let roots = vec![root.path().to_path_buf()];
    assert!(
        !name_index_search(&db, "Q", &roots).is_empty(),
        "phase1: Q がヒットしない"
    );
    assert!(
        !name_index_search(&db, "a.zip", &roots).is_empty(),
        "phase1: a.zip がヒットしない"
    );
    assert!(
        !name_index_search(&db, "b.pdf", &roots).is_empty(),
        "phase1: b.pdf がヒットしない"
    );

    // オフライン削除: /root/P/Q, a.zip, b.pdf をすべて消す → /root/P は空フォルダに
    std::fs::remove_dir_all(p.join("Q")).expect("rm Q");
    std::fs::remove_file(p.join("a.zip")).expect("rm a.zip");
    std::fs::remove_file(p.join("b.pdf")).expect("rm b.pdf");

    // Phase 2: 再スキャン (アプリ再起動相当)
    let summary2 = run_bulk_name_index(root.path(), &db, &cancel, None);
    assert!(!summary2.cancelled);

    // 古い行が消えていること
    let q_hits = name_index_search(&db, "Q", &roots);
    assert!(
        q_hits.is_empty(),
        "phase2: Q が削除後も索引に残っている (stale entries): {q_hits:?}"
    );
    let a_hits = name_index_search(&db, "a.zip", &roots);
    assert!(
        a_hits.is_empty(),
        "phase2: a.zip が削除後も索引に残っている: {a_hits:?}"
    );
    let b_hits = name_index_search(&db, "b.pdf", &roots);
    assert!(
        b_hits.is_empty(),
        "phase2: b.pdf が削除後も索引に残っている: {b_hits:?}"
    );
}

/// Codex P2 #2 回帰テスト: nested favorites で共有パスが一方にしか載らないバグ。
///
/// ## シナリオ
///
/// 1. お気に入り A = `/root` (親)
/// 2. お気に入り B = `/root/photo` (子, A の配下にネスト)
/// 3. `/root/photo/a.zip` を作る (両方のスコープに入る実体)
/// 4. 両 favorite のバルクを走らせる
/// 5. A スコープで `a.zip` 検索 → ヒットする
/// 6. B スコープで `a.zip` 検索 → ヒットする
///
/// 旧実装は PRIMARY KEY(path) なので後に書いた favorite_root で行が上書きされ、
/// 先に走った方の favorite_root 視点からは entries.favorite_root IN (...) フィルタを
/// すり抜けて **検索ヒットが 0 件になる**。
#[test]
fn nested_favorites_both_scopes_find_shared_path() {
    use mimageviewer::name_bulk_indexer::run_bulk_name_index;
    use mimageviewer::search_index_db::SearchIndexDb;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    // 共有パス: /root/photo/a.zip
    let photo = root.mkdir("photo");
    write_empty_zip(&photo.join("a.zip"));
    // sub/ 側にも画像を置いて folder エントリとして認識されるように
    let sub = root.mkdir("photo/sub");
    write_png_plain(&sub.join("thumb.png"));

    let db = Arc::new(
        SearchIndexDb::open_at(&data.path().join("search_index.db")).expect("open db"),
    );
    let cancel = AtomicBool::new(false);

    // お気に入り A (親) と B (子) を順番にバルク
    run_bulk_name_index(root.path(), &db, &cancel, None); // A = /root
    run_bulk_name_index(&photo, &db, &cancel, None); //       B = /root/photo

    // A スコープで a.zip 検索 → 見つかるべき
    let hits_a = name_index_search(&db, "a.zip", &[root.path().to_path_buf()]);
    assert!(
        hits_a.iter().any(|e| e.display_name == "a.zip"),
        "A (/root) スコープで a.zip が見つからない (nested favorite が上書きした?): {hits_a:?}"
    );

    // B スコープで a.zip 検索 → 見つかるべき
    let hits_b = name_index_search(&db, "a.zip", &[photo.clone()]);
    assert!(
        hits_b.iter().any(|e| e.display_name == "a.zip"),
        "B (/root/photo) スコープで a.zip が見つからない: {hits_b:?}"
    );

    // sub フォルダも同様に両方から見えること
    let sub_a = name_index_search(&db, "sub", &[root.path().to_path_buf()]);
    assert!(
        sub_a.iter().any(|e| e.display_name == "sub"),
        "A スコープで sub フォルダが見つからない: {sub_a:?}"
    );
    let sub_b = name_index_search(&db, "sub", &[photo.clone()]);
    assert!(
        sub_b.iter().any(|e| e.display_name == "sub"),
        "B スコープで sub フォルダが見つからない: {sub_b:?}"
    );
}

// -----------------------------------------------------------------------
// 表示種別の分類 (Ctrl+S 検索結果が Folder / ZipFile / PdfFile を正しく返す)
// -----------------------------------------------------------------------

/// Ctrl+S 検索結果で Folder / ZipFile / PdfFile の 3 種別が期待通りの `IndexKind` で
/// 返ってくること (UI 側のアイコン / ダブルクリック挙動の分岐に影響するため)。
///
/// 2026-04 ユーザー要望: Ctrl+S の検索結果表示で各種別の動作確認テストが欲しい。
#[test]
fn name_search_classifies_folder_zip_and_pdf() {
    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    // 名前索引対象の 3 種別を 1 つずつ配置 (画像ファイル (png/jpg) は名前索引対象外)
    mkdir_with_image(&root, "mixed_folder_xuq8");
    write_empty_zip(&root.path().join("mixed_archive_xuq8.zip"));
    std::fs::write(root.path().join("mixed_document_xuq8.pdf"), b"fake").expect("write pdf");

    let fav = make_favorite("A", root.path());
    let (db, handle) = start_name_index_at(data.path(), &fav);
    wait_name_scan_done(&handle);

    let roots = vec![fav.path.clone()];
    let hits = name_index_search(&db, "mixed", &roots);
    assert_eq!(hits.len(), 3, "3 種別が全ヒット: {hits:?}");

    // 名前で辿れるように HashMap に詰める
    let by_name: std::collections::HashMap<&str, IndexKind> = hits
        .iter()
        .map(|e| (e.display_name.as_str(), e.kind))
        .collect();
    assert_eq!(
        by_name.get("mixed_folder_xuq8").copied(),
        Some(IndexKind::Folder),
        "folder の kind が Folder でない: {hits:?}"
    );
    assert_eq!(
        by_name.get("mixed_archive_xuq8.zip").copied(),
        Some(IndexKind::ZipFile),
        "zip の kind が ZipFile でない: {hits:?}"
    );
    assert_eq!(
        by_name.get("mixed_document_xuq8.pdf").copied(),
        Some(IndexKind::PdfFile),
        "pdf の kind が PdfFile でない: {hits:?}"
    );
}

/// Codex P2 (2026-04) 回帰: 「アプリ停止中にフォルダごと削除されたサブツリーの
/// stale row が残る」バグ。
///
/// ## シナリオ
///
/// 1. `/root/P/Q` (フォルダ) + `/root/P/Q/a.zip` + `/root/P/Q/b.pdf` + `/root/other.zip`
///    を作り初期スキャン → すべて索引化される
/// 2. **アプリ停止中を模して** `/root/P` を **サブツリーごと** 削除
///    (`P`, `P/Q`, `P/Q/a.zip`, `P/Q/b.pdf` が全部消える)
/// 3. 再度 `run_bulk_name_index` を走らせる (= アプリ再起動時のフルスキャン)
/// 4. **期待**:
///    - `/root/other.zip` はヒットする
///    - `/root/P` / `/root/P/Q` / `Q/a.zip` / `Q/b.pdf` はどれもヒットしない
///
/// 旧実装の問題: `run_bulk_name_index` は walk で観測できたフォルダだけを
/// upsert_children の対象にする。`/root/P` 以下が全部消えると walk がそのフォルダを
/// そもそも踏まないため、過去に書き込まれていた行が残り続ける。
/// upsert_children の DELETE は直下のみスコープなので、`/root` の upsert をしても
/// 孫以下は触らない。結果として Ctrl+S で存在しない Q / a.zip / b.pdf がヒットする。
#[test]
fn full_scan_removes_offline_deleted_subtree() {
    use mimageviewer::name_bulk_indexer::run_bulk_name_index;
    use mimageviewer::search_index_db::SearchIndexDb;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    // /root/P/Q 配下の構造 + /root/other.zip
    let p = root.mkdir("P");
    let q = root.mkdir("P/Q");
    write_empty_zip(&q.join("a.zip"));
    std::fs::write(q.join("b.pdf"), b"fake").expect("write pdf");
    write_png_plain(&q.join("cover.png")); // Q を folder エントリにするため
    write_empty_zip(&root.path().join("other.zip"));

    let db = Arc::new(
        SearchIndexDb::open_at(&data.path().join("search_index.db")).expect("open db"),
    );
    let cancel = AtomicBool::new(false);
    let roots = vec![root.path().to_path_buf()];

    // Phase 1: 初期スキャン
    let s1 = run_bulk_name_index(root.path(), &db, &cancel, None);
    assert!(!s1.cancelled);
    assert!(
        !name_index_search(&db, "Q", &roots).is_empty(),
        "phase1: Q が索引に入ってない"
    );
    assert!(
        !name_index_search(&db, "a.zip", &roots).is_empty(),
        "phase1: Q/a.zip が入ってない"
    );
    assert!(
        !name_index_search(&db, "other.zip", &roots).is_empty(),
        "phase1: other.zip が入ってない"
    );

    // オフライン削除: /root/P をサブツリーごと消す (Q, a.zip, b.pdf, cover.png 全部)
    std::fs::remove_dir_all(&p).expect("rm -rf P");

    // timestamp が同秒にならないよう少し待つ (updated_at の秒精度 cutoff 回避)。
    // prune_stale_for_favorite の cutoff は「scan start 時刻」なので、
    // Phase 1 の upsert 時刻と Phase 2 の scan start 時刻が同じ秒だと
    // stale 行も「>= cutoff」扱いになって削除されないリスクがある。
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // Phase 2: 再スキャン (アプリ再起動相当)
    let s2 = run_bulk_name_index(root.path(), &db, &cancel, None);
    assert!(!s2.cancelled);

    // 削除済みサブツリーが全部消えていること
    let q_hits = name_index_search(&db, "Q", &roots);
    assert!(
        q_hits.is_empty(),
        "phase2: Q (サブツリー削除) が stale row として残っている: {q_hits:?}"
    );
    let a_hits = name_index_search(&db, "a.zip", &roots);
    assert!(
        a_hits.is_empty(),
        "phase2: Q/a.zip が stale row として残っている: {a_hits:?}"
    );
    let b_hits = name_index_search(&db, "b.pdf", &roots);
    assert!(
        b_hits.is_empty(),
        "phase2: Q/b.pdf が stale row として残っている: {b_hits:?}"
    );

    // 削除してない /root/other.zip は残っている
    assert!(
        !name_index_search(&db, "other.zip", &roots).is_empty(),
        "phase2: other.zip が誤って消された (prune が過剰)"
    );
}

/// ユーザー質問 (2026-04): `c:\home\photo` と `c:\home\photo\fav` のように
/// お気に入りが **入れ子** になった構成で、prune_stale_for_favorite が
/// 他の favorite のエントリを巻き込まないこと。
///
/// 具体的には:
/// - 両 favorite のバルクを順番に走らせる
/// - アプリ停止中に片方 (親 favorite) のサブツリーだけを削除
/// - 再スキャン: 親の stale 行は消え、子 favorite の行は残る
///
/// 複合 PK `(favorite_root, path)` + prune の `WHERE favorite_root = ?` スコープで
/// 保証される設計不変量を回帰テストとして固定する。
#[test]
fn prune_stale_does_not_touch_nested_sibling_favorite() {
    use mimageviewer::name_bulk_indexer::run_bulk_name_index;
    use mimageviewer::search_index_db::SearchIndexDb;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    let data = FixtureRoot::new();
    let root = FixtureRoot::new();
    // /root/photo/ = fav A (親)
    // /root/photo/fav/ = fav B (子, ネスト)
    // /root/photo/P/Q/a.zip ← A 配下でのみ見える
    // /root/photo/fav/x.zip ← B 配下 (と A 配下の両方で見える)
    let photo = root.mkdir("photo");
    let p = root.mkdir("photo/P");
    let q = root.mkdir("photo/P/Q");
    write_empty_zip(&q.join("a.zip"));
    write_png_plain(&q.join("cover.png"));
    let fav_sub = root.mkdir("photo/fav");
    write_empty_zip(&fav_sub.join("x.zip"));

    let db = Arc::new(
        SearchIndexDb::open_at(&data.path().join("search_index.db")).expect("open db"),
    );
    let cancel = AtomicBool::new(false);

    // Phase 1: 両 favorite でバルク
    run_bulk_name_index(&photo, &db, &cancel, None); // A = /photo
    run_bulk_name_index(&fav_sub, &db, &cancel, None); // B = /photo/fav

    let roots_a = vec![photo.clone()];
    let roots_b = vec![fav_sub.clone()];

    // A スコープで a.zip と x.zip が見える
    assert!(!name_index_search(&db, "a.zip", &roots_a).is_empty());
    assert!(!name_index_search(&db, "x.zip", &roots_a).is_empty());
    // B スコープで x.zip が見える
    assert!(!name_index_search(&db, "x.zip", &roots_b).is_empty());

    // オフライン削除: A 配下の P サブツリーだけを削除 (B 配下の /photo/fav は触らない)
    std::fs::remove_dir_all(&p).expect("rm -rf P");

    // 秒境界を確実に跨ぐ
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // Phase 2: A のみ再スキャン (B は前回の行を保持したまま)
    run_bulk_name_index(&photo, &db, &cancel, None);

    // A スコープ: a.zip が消えた、x.zip は残る
    assert!(
        name_index_search(&db, "a.zip", &roots_a).is_empty(),
        "A scope: 削除済み a.zip が残っている (prune 漏れ)"
    );
    assert!(
        !name_index_search(&db, "x.zip", &roots_a).is_empty(),
        "A scope: x.zip が誤って消された"
    );

    // B スコープ: x.zip は残る (B の行は A の prune で触られない)
    assert!(
        !name_index_search(&db, "x.zip", &roots_b).is_empty(),
        "B scope: A の prune で B の行が巻き込まれた (nested favorite 巻き込みバグ)"
    );
}
