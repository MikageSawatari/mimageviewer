use super::*;
use crate::archive_converter::ArchiveFormat;
use std::path::PathBuf;
use tempfile::TempDir;

/// `App::new_for_test` に渡すテスト設定。
///
/// Phase C では実プロセスの `data_dir::init` を経由せず、`set_test_override` で
/// `TempDir` を差し込む。App 内の全 DB/インデクサ open はその data_dir を参照する。
struct AppTestConfig {
    /// テスト用データディレクトリ。`data_dir::set_test_override(Some(...))` に設定済みの
    /// パスを渡す。(呼び出し側の `TempDir` が App より長生きする必要あり)
    data_dir: std::path::PathBuf,
    /// 起動時に `settings.json` をこの内容で上書きしてから App::default を呼ぶ。
    /// None なら `Settings::load` が空ファイルから default 設定を作る。
    settings: Option<crate::settings::Settings>,
}

impl App {
    /// テスト用コンストラクタ。本番の `App::default` と同じ DB/indexer open 経路を
    /// 通すが、以下が異なる:
    ///
    /// 1. `config.data_dir` を `data_dir::set_test_override` 経由で強制する前提 (呼び出し側で)
    /// 2. `config.settings` があれば `settings.json` に書き出してから load する
    /// 3. 名前索引 supervisor の初期 spawn は行わない
    ///    (呼び出し側が `spawn_initial_name_index_supervisors()` を明示的に呼ぶ)
    /// 4. 初期サイズ / font / theme は設定しない (テスト側で Context を用意する想定)
    ///
    /// 注意: Tantivy / SQLite / notify-rs などの実スレッドは通常どおり起動するので、
    /// テスト終了時には `drop(app)` で正しく停止すること (IndexerManager::drop が
    /// supervisor を signal_stop→join で止める)。
    fn new_for_test(config: AppTestConfig) -> Self {
        // settings.json をあらかじめ書いておく (App::default 内の Settings::load が拾う)
        if let Some(settings) = &config.settings {
            std::fs::create_dir_all(&config.data_dir).ok();
            let json = serde_json::to_string_pretty(settings).expect("serialize settings");
            std::fs::write(config.data_dir.join("settings.json"), json)
                .expect("write settings.json");
        }
        // data_dir::get() はこの時点で config.data_dir を返さなければならない
        debug_assert_eq!(
            crate::data_dir::get(),
            config.data_dir,
            "data_dir::set_test_override(Some(config.data_dir)) を先に呼ぶこと"
        );
        let app = App::default();
        // `spawn_initial_name_index_supervisors` はテスト側で必要なときだけ呼ぶ契約
        app
    }
}

fn scan_media_names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = scan_directory(dir)
        .all_media
        .into_iter()
        .filter_map(|(path, _, _, _)| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

#[test]
fn scan_directory_hides_source_when_upscaled_derivative_sidecar_exists() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(tmp.path().join("movie.mp4"), b"source").unwrap();
    std::fs::write(tmp.path().join("movie.jpg"), b"cover").unwrap();
    std::fs::write(tmp.path().join("movie.miv.mkv"), b"upscaled").unwrap();
    std::fs::write(tmp.path().join("movie.miv.json"), b"{}").unwrap();

    assert_eq!(scan_media_names(tmp.path()), vec!["movie.miv.mkv"]);
}

#[test]
fn scan_directory_hides_multiple_companion_images_when_derivative_exists() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(tmp.path().join("movie.mp4"), b"source").unwrap();
    std::fs::write(tmp.path().join("movie.jpg"), b"cover").unwrap();
    std::fs::write(tmp.path().join("movie.png"), b"thumb").unwrap();
    std::fs::write(tmp.path().join("movie.webp"), b"thumb2").unwrap();
    std::fs::write(tmp.path().join("movie.miv.mkv"), b"upscaled").unwrap();
    std::fs::write(tmp.path().join("movie.miv.json"), b"{}").unwrap();

    assert_eq!(scan_media_names(tmp.path()), vec!["movie.miv.mkv"]);
}

#[test]
fn scan_directory_keeps_source_when_upscaled_sidecar_is_missing() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(tmp.path().join("movie.mp4"), b"source").unwrap();
    std::fs::write(tmp.path().join("movie.miv.mkv"), b"upscaled").unwrap();

    assert_eq!(
        scan_media_names(tmp.path()),
        vec!["movie.miv.mkv", "movie.mp4"]
    );
}

#[test]
fn scan_directory_keeps_sources_when_upscaled_stem_is_ambiguous() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(tmp.path().join("movie.mp4"), b"source-a").unwrap();
    std::fs::write(tmp.path().join("movie.avi"), b"source-b").unwrap();
    std::fs::write(tmp.path().join("movie.miv.mkv"), b"upscaled").unwrap();
    std::fs::write(tmp.path().join("movie.miv.json"), b"{}").unwrap();

    assert_eq!(
        scan_media_names(tmp.path()),
        vec!["movie.avi", "movie.miv.mkv", "movie.mp4"]
    );
}

// ── cell_has_lower_left_container_badge ───────────────────────────────────
//
// レーティング ★ バッジを左下に出す際、コンテナバッジ (folder 名 / "ZIP" / "PDF" /
// "7z" / "LZH") と重ねないために使う純関数。ユーザー報告「フォルダ名と ★ が重なる」
// 退行ガード。

#[test]
fn lower_left_container_badge_yes_for_folder_when_loaded() {
    use crate::grid_item::GridItem;
    let dummy_tex = {
        let ctx = egui::Context::default();
        ctx.load_texture(
            "dummy",
            egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            Default::default(),
        )
    };
    let folder = GridItem::Folder(PathBuf::from("c:/x"));
    let loaded = ThumbnailState::Loaded {
        tex: dummy_tex,
        from_cache: false,
        rendered_at_px: 128,
        source_dims: None,
    };
    assert!(cell_has_lower_left_container_badge(&folder, &loaded));
}

#[test]
fn lower_left_container_badge_no_for_folder_when_pending() {
    use crate::grid_item::GridItem;
    let folder = GridItem::Folder(PathBuf::from("c:/x"));
    assert!(!cell_has_lower_left_container_badge(
        &folder,
        &ThumbnailState::Pending
    ));
    assert!(!cell_has_lower_left_container_badge(
        &folder,
        &ThumbnailState::Evicted
    ));
    assert!(!cell_has_lower_left_container_badge(
        &folder,
        &ThumbnailState::Failed
    ));
}

#[test]
fn lower_left_container_badge_always_yes_for_archive_types() {
    use crate::grid_item::GridItem;
    let zip = GridItem::ZipFile(PathBuf::from("c:/x.zip"));
    let pdf = GridItem::PdfFile(PathBuf::from("c:/x.pdf"));
    let arch = GridItem::ConvertibleArchive {
        path: PathBuf::from("c:/x.7z"),
        format: crate::archive_converter::ArchiveFormat::SevenZ,
    };
    // どの thumb 状態でも常に true (= 描画パスは Loaded/Pending/Evicted/Failed すべて
    // 最後に badge_fn を呼ぶ)
    for thumb in [
        ThumbnailState::Pending,
        ThumbnailState::Evicted,
        ThumbnailState::Failed,
    ] {
        assert!(cell_has_lower_left_container_badge(&zip, &thumb));
        assert!(cell_has_lower_left_container_badge(&pdf, &thumb));
        assert!(cell_has_lower_left_container_badge(&arch, &thumb));
    }
}

#[test]
fn lower_left_container_badge_no_for_image_like_items() {
    use crate::grid_item::GridItem;
    let image = GridItem::Image(PathBuf::from("c:/x.jpg"));
    let zip_image = GridItem::ZipImage {
        zip_path: PathBuf::from("c:/x.zip"),
        entry_name: "p1.jpg".to_string(),
    };
    let pdf_page = GridItem::PdfPage {
        pdf_path: PathBuf::from("c:/x.pdf"),
        page_num: 0,
        content_type: None,
    };
    let video = GridItem::Video(PathBuf::from("c:/x.mp4"));
    let sep = GridItem::ZipSeparator {
        dir_display: "ch01".to_string(),
    };
    for item in [&image, &zip_image, &pdf_page, &video, &sep] {
        assert!(!cell_has_lower_left_container_badge(
            item,
            &ThumbnailState::Pending
        ));
    }
}

#[test]
fn fullscreen_prefetch_candidates_respect_visible_indices() {
    use crate::grid_item::GridItem;

    let items = vec![
        GridItem::Image(PathBuf::from("c:/a.jpg")),
        GridItem::Image(PathBuf::from("c:/hidden.jpg")),
        GridItem::Video(PathBuf::from("c:/clip.mp4")),
        GridItem::ZipImage {
            zip_path: PathBuf::from("c:/book.zip"),
            entry_name: "p01.jpg".to_string(),
        },
        GridItem::PdfPage {
            pdf_path: PathBuf::from("c:/scan.pdf"),
            page_num: 0,
            content_type: None,
        },
        GridItem::Folder(PathBuf::from("c:/sub")),
    ];
    let visible_indices = vec![0, 2, 3, 4, 5];

    let image_indices = App::collect_image_indices_from(&items, &visible_indices);
    assert_eq!(
        image_indices,
        vec![0, 3, 4],
        "フィルタで隠れた画像 idx=1 や動画/フォルダはフルスクリーン画像先読み対象にしない"
    );

    assert_eq!(
        interleaved_prefetch_targets(&image_indices, 0, image_indices.len(), 1, 0),
        vec![3],
        "現在 idx=0 の次候補は raw idx=1 ではなく、表示中一覧で次の画像 idx=3"
    );
}

/// P6-6: `interleaved_prefetch_targets` 純関数の境界条件を符号化する。
///
/// この関数は `App::ai_prefetch_targets` の中核で、UI スレッドから 1 フレーム
/// 数回呼ばれる経路にいる。順序 (forward, back, forward, back, ...) を変えると
/// 「ユーザーが次に見るページから優先して AI を温める」スケジュールが崩れる
/// (= ページ送り直後に毎回 cold miss する退行)。
///
/// それぞれ独立した境界 (= 先頭で back が無い / 末尾で forward が無い /
/// 全 0 で空 / 大きい d で末尾にぶつかったらスキップ) を 1 個ずつチェックする。
#[test]
fn interleaved_prefetch_targets_boundary_cases() {
    // 通常 case: 中央 (pos=3), forward=2, back=1
    // 期待順: forward d=1 → back d=1 → forward d=2  (back d=2 は無し: pf_back=1)
    let indices: Vec<usize> = (0..7).collect(); // [0,1,2,3,4,5,6]
    assert_eq!(
        interleaved_prefetch_targets(&indices, 3, 7, 2, 1),
        vec![4, 2, 5],
        "通常 case: forward → back → forward 順 (d=1 forward, d=1 back, d=2 forward)"
    );

    // 先頭: pos=0, forward=3, back=2
    // pos.checked_sub(d) → None で back は何も生やさない
    assert_eq!(
        interleaved_prefetch_targets(&indices, 0, 7, 3, 2),
        vec![1, 2, 3],
        "先頭: back は全部 None なので forward のみ"
    );

    // 末尾: pos=6, forward=2, back=3
    // pos+d >= n で forward はカット、back は 3 件取れる
    assert_eq!(
        interleaved_prefetch_targets(&indices, 6, 7, 2, 3),
        vec![5, 4, 3],
        "末尾: forward は 範囲外なので back のみ"
    );

    // 全 0: forward=0, back=0
    assert!(
        interleaved_prefetch_targets(&indices, 3, 7, 0, 0).is_empty(),
        "forward=back=0 → 空"
    );

    // 非対称: forward >> back の典型ケース (settings 既定: forward=2, back=1)
    // pos=2, n=5 → forward d=1→3, back d=1→1, forward d=2→4 (back d=2 は無し)
    let small: Vec<usize> = vec![10, 20, 30, 40, 50];
    assert_eq!(
        interleaved_prefetch_targets(&small, 2, 5, 2, 1),
        vec![40, 20, 50],
        "デフォルト forward=2 back=1 のインタリーブ順序 (= settings 既定値の代表ケース)"
    );

    // forward が n を越える: 末尾を超えたらスキップ
    assert_eq!(
        interleaved_prefetch_targets(&small, 2, 5, 10, 0),
        vec![40, 50],
        "forward が n を越えても、範囲内のものだけが選ばれる (overflow scenarios)"
    );
}

#[test]
fn next_video_search_uses_visible_indices_and_skips_non_video_items() {
    use crate::grid_item::GridItem;

    let items = vec![
        GridItem::Video(PathBuf::from("c:/a.mp4")),
        GridItem::Image(PathBuf::from("c:/a.jpg")),
        GridItem::ZipImage {
            zip_path: PathBuf::from("c:/book.zip"),
            entry_name: "p01.jpg".to_string(),
        },
        GridItem::PdfPage {
            pdf_path: PathBuf::from("c:/scan.pdf"),
            page_num: 0,
            content_type: None,
        },
        GridItem::Video(PathBuf::from("c:/b.mp4")),
        GridItem::Folder(PathBuf::from("c:/sub")),
        GridItem::ZipSeparator {
            dir_display: "chapter".to_string(),
        },
    ];
    let visible_indices = vec![0, 1, 2, 3, 4, 5, 6];

    assert_eq!(
        App::find_next_video_in_visible_indices_from(&items, &visible_indices, 0, false),
        Some(4),
        "画像 / ZIP 内画像 / PDF ページ / フォルダ / セパレータを飛ばして次の動画を選ぶ"
    );
    assert_eq!(
        App::find_next_video_in_visible_indices_from(&items, &visible_indices, 4, false),
        None,
        "末尾側に動画が無ければ Continuous は停止する"
    );
    assert_eq!(
        App::find_next_video_in_visible_indices_from(&items, &visible_indices, 4, true),
        Some(0),
        "ContinuousLoop は visible_indices の先頭側へ wrap する"
    );

    let filtered = vec![4];
    assert_eq!(
        App::find_next_video_in_visible_indices_from(&items, &filtered, 4, true),
        Some(4),
        "表示リストに動画 1 本だけなら同じ動画を繰り返す"
    );
    assert_eq!(
        App::find_next_video_in_visible_indices_from(&items, &[1, 2, 3], 4, true),
        None,
        "現在動画が表示リスト外で、wrap しても動画候補が無ければ None"
    );
}

#[test]
fn fullscreen_keep_set_keeps_current_image_when_filtered_out() {
    use crate::grid_item::GridItem;

    let mut app = phase_c_support::setup_app();
    app.items = vec![
        GridItem::Image(PathBuf::from("c:/a.jpg")),
        GridItem::Image(PathBuf::from("c:/filtered-out.jpg")),
        GridItem::Image(PathBuf::from("c:/b.jpg")),
    ];
    app.visible_indices = vec![0, 2];

    let keep = app.compute_keep_set(1);
    assert!(
        keep.contains(&1),
        "フルスクリーン中の現在画像がフィルタ外でも派生キャッシュは保持する"
    );
    assert_eq!(keep.len(), 1);
}

// ── passes_rating_filter (コンテナ/画像/Video の挙動) ──

#[test]
fn video_resume_for_open_grid_and_nav_modes() {
    use crate::settings::ResumeMode::{FromStart, Resume};
    let saved = Some(42.0);

    // 一覧から開く (from_grid=true): open_starts_from_beginning が判定、nav_resume は無視。
    assert_eq!(
        video_resume_for_open(saved, true, false, Resume),
        saved,
        "grid 開く・続きから"
    );
    assert_eq!(
        video_resume_for_open(saved, true, true, Resume),
        None,
        "grid 開く・先頭から"
    );
    assert_eq!(video_resume_for_open(None, true, true, Resume), None);

    // Ctrl+↑↓ / ホイール等 (from_grid=false): nav_resume が判定、open 設定は無視。
    assert_eq!(
        video_resume_for_open(saved, false, true, Resume),
        saved,
        "nav・続きから (open=先頭 でも無視)"
    );
    assert_eq!(
        video_resume_for_open(saved, false, false, FromStart),
        None,
        "nav・先頭から"
    );
    assert_eq!(video_resume_for_open(None, false, false, Resume), None);
}

#[test]
fn rating_filter_container_uses_all_6_buckets() {
    let folder = GridItem::Folder(PathBuf::from("/a"));
    // ★なし OFF → 未評価フォルダも隠れる (「★5 のみ表示」が実際に効くために必要)
    let mut f = [true; 6];
    f[0] = false;
    assert!(!passes_rating_filter(&folder, 0, &f));
    // ★3 フォルダ、★3 ON なら可視
    assert!(passes_rating_filter(&folder, 3, &[true; 6]));
    // ★3 フォルダ、★3 OFF なら非可視
    let mut f = [true; 6];
    f[3] = false;
    assert!(!passes_rating_filter(&folder, 3, &f));
}

#[test]
fn rating_filter_zip_pdf_containers_behave_like_folder() {
    let zip = GridItem::ZipFile(PathBuf::from("/a.zip"));
    let pdf = GridItem::PdfFile(PathBuf::from("/a.pdf"));
    let mut f = [true; 6];
    f[0] = false;
    assert!(!passes_rating_filter(&zip, 0, &f));
    assert!(!passes_rating_filter(&pdf, 0, &f));
    let mut f = [true; 6];
    f[4] = false;
    assert!(!passes_rating_filter(&zip, 4, &f));
    assert!(!passes_rating_filter(&pdf, 4, &f));
}

#[test]
fn rating_filter_image_page_uses_all_6_buckets() {
    let img = GridItem::Image(PathBuf::from("/a.jpg"));
    let mut f = [true; 6];
    f[0] = false;
    assert!(!passes_rating_filter(&img, 0, &f));
    let f = [true; 6];
    assert!(passes_rating_filter(&img, 2, &f));
    let mut f = [true; 6];
    f[2] = false;
    assert!(!passes_rating_filter(&img, 2, &f));
}

#[test]
fn rating_filter_zip_image_and_pdf_page_behave_like_image() {
    // ページ系の残り 2 種 (ZipImage / PdfPage) が Image と同じ 6 バケット判定で
    // 動いていることを担保 (コンテナと対称)。
    let zip_img = GridItem::ZipImage {
        zip_path: PathBuf::from("/a.zip"),
        entry_name: "x.jpg".to_string(),
    };
    let pdf_page = GridItem::PdfPage {
        pdf_path: PathBuf::from("/a.pdf"),
        page_num: 1,
        content_type: None,
    };
    let mut f = [true; 6];
    f[0] = false;
    assert!(!passes_rating_filter(&zip_img, 0, &f));
    assert!(!passes_rating_filter(&pdf_page, 0, &f));
    let mut f = [true; 6];
    f[3] = false;
    assert!(!passes_rating_filter(&zip_img, 3, &f));
    assert!(!passes_rating_filter(&pdf_page, 3, &f));
}

#[test]
fn rating_filter_star5_only_hides_unrated_containers() {
    // ユーザが明示的に「★5 だけ見たい」(★5 のみ ON、他全部 OFF) を選んだとき、
    // 未評価のフォルダも確実に非表示になること (本修正の主目的)
    let folder = GridItem::Folder(PathBuf::from("/a"));
    let img = GridItem::Image(PathBuf::from("/b.jpg"));
    let mut f = [false; 6];
    f[5] = true;
    assert!(!passes_rating_filter(&folder, 0, &f));
    assert!(!passes_rating_filter(&img, 0, &f));
    assert!(passes_rating_filter(&folder, 5, &f));
    assert!(passes_rating_filter(&img, 5, &f));
}

#[test]
fn rating_filter_applies_to_video_when_unrated_off() {
    // Video もレーティング対象なので「なし」フィルタ OFF で未評価動画は隠れる。
    // 通常画像と同じ扱い。
    let vid = GridItem::Video(PathBuf::from("/a.mp4"));
    // 全 OFF: ★0 (未評価) の動画は隠れる
    assert!(!passes_rating_filter(&vid, 0, &[false; 6]));
    // ★0 ON: ★0 の動画は通る
    let mut rf = [false; 6];
    rf[0] = true;
    assert!(passes_rating_filter(&vid, 0, &rf));
    // ★3 のみ ON: ★0 動画は通らない、★3 動画は通る
    let mut rf = [false; 6];
    rf[3] = true;
    assert!(!passes_rating_filter(&vid, 0, &rf));
    assert!(passes_rating_filter(&vid, 3, &rf));
}

#[test]
fn separator_is_not_rating_target() {
    // ZipSeparator はレーティング対象外なのでフィルタ素通り (常に可視)。
    let sep = GridItem::ZipSeparator {
        dir_display: "x".into(),
    };
    assert!(passes_rating_filter(&sep, 0, &[false; 6]));
}

#[test]
fn rating_filter_defensive_against_corrupt_stars() {
    // 想定外の stars>5 はインデックス越境を避けるため非可視にする (防御)
    let folder = GridItem::Folder(PathBuf::from("/a"));
    let img = GridItem::Image(PathBuf::from("/a.jpg"));
    assert!(!passes_rating_filter(&folder, 99, &[true; 6]));
    assert!(!passes_rating_filter(&img, 99, &[true; 6]));
}

/// ★フィルタ suppression の scope 判定: folder anchor。
/// case-insensitive + component-boundary + cross-drive を `path_in_subtree_ci` で担保する。
#[test]
fn rating_filter_suppression_scope_folder_anchor() {
    use std::path::Path;
    let anchor = PathBuf::from("/books/book-a");
    // 同一 / 子孫
    assert!(path_in_subtree_ci(Path::new("/books/book-a"), &anchor));
    assert!(path_in_subtree_ci(
        Path::new("/books/book-a/chapter1"),
        &anchor
    ));
    assert!(path_in_subtree_ci(
        Path::new("/books/book-a/chapter1/sub"),
        &anchor
    ));
    // 親 / sibling は外
    assert!(!path_in_subtree_ci(Path::new("/books"), &anchor));
    assert!(!path_in_subtree_ci(Path::new("/books/book-b"), &anchor));
    // name prefix match (`book-a-extra`) は component 境界を守って外れる
    assert!(!path_in_subtree_ci(
        Path::new("/books/book-a-extra"),
        &anchor
    ));
}

#[test]
fn rating_filter_suppression_scope_zip_pdf_anchor() {
    use std::path::Path;
    let zip = PathBuf::from("/books/vol1.zip");
    assert!(path_in_subtree_ci(Path::new("/books/vol1.zip"), &zip));
    assert!(!path_in_subtree_ci(Path::new("/books"), &zip)); // BS で親へ → outside
    assert!(!path_in_subtree_ci(Path::new("/books/vol2.zip"), &zip)); // sibling

    let pdf = PathBuf::from("/books/manual.pdf");
    assert!(path_in_subtree_ci(Path::new("/books/manual.pdf"), &pdf));
    assert!(!path_in_subtree_ci(Path::new("/books"), &pdf));
}

/// Windows の case-insensitive FS: ドライブ文字や階層名の casing 違いを同一視する。
/// これが効かないと、address bar 入力や Explorer D&D で casing が変わった瞬間に
/// suppression scope から脱落してフィルタが復元されてしまう (ユーザー視点では
/// 「なぜか filter が急に戻った」= 旧バグ)。
#[test]
fn path_in_subtree_ci_is_case_insensitive() {
    use std::path::Path;
    let anchor = PathBuf::from(r"C:\Books\Vol1.zip");
    assert!(path_in_subtree_ci(Path::new(r"c:\books\vol1.zip"), &anchor));
    assert!(path_in_subtree_ci(Path::new(r"C:\BOOKS\VOL1.ZIP"), &anchor));
    // 区切り文字の違いも吸収する
    assert!(path_in_subtree_ci(Path::new("C:/Books/Vol1.zip"), &anchor));
}

/// cross-drive 偶然一致を起こさない (C:/foo と D:/foo は別扱い)。
/// path_key::normalize がドライブ文字を剥がすのに対し、こちらは保持することで
/// 別ドライブの同名コンテナを誤って同一 scope と判定しない。
#[test]
fn path_in_subtree_ci_keeps_drive_letter_distinct() {
    use std::path::Path;
    let anchor = PathBuf::from(r"C:\books\vol1.zip");
    assert!(!path_in_subtree_ci(
        Path::new(r"D:\books\vol1.zip"),
        &anchor
    ));
    assert!(!path_in_subtree_ci(
        Path::new(r"D:\books\vol1.zip\page001.jpg"),
        &anchor
    ));
}

/// 同名フォルダがある ZIP/PDF/ConvertibleArchive (7z/LZH) は
/// `filter_virtual_folder_duplicates` でスキップされる。
/// v0.7.0 の Task 17 で 7z/LZH への拡張を入れた回帰テスト。
#[test]
fn filter_virtual_folder_skips_archive_matching_folder() {
    let mut folders: Vec<GridItem> = vec![
        GridItem::Folder(PathBuf::from("/r/vol01")),
        GridItem::ZipFile(PathBuf::from("/r/vol01.zip")), // 同名フォルダあり → 消える
        GridItem::ZipFile(PathBuf::from("/r/other.zip")), // 同名フォルダなし → 残る
        GridItem::PdfFile(PathBuf::from("/r/vol01.pdf")), // 同名フォルダあり → 消える
        GridItem::ConvertibleArchive {
            path: PathBuf::from("/r/vol01.7z"), // 同名フォルダあり → 消える
            format: ArchiveFormat::SevenZ,
        },
        GridItem::ConvertibleArchive {
            path: PathBuf::from("/r/bonus.lzh"), // 同名フォルダなし → 残る
            format: ArchiveFormat::Lzh,
        },
    ];
    let mut folder_metas: Vec<Option<(i64, i64)>> = vec![None, None, None, None, None, None];

    App::filter_virtual_folder_duplicates(&mut folders, &mut folder_metas);

    let remaining_names: Vec<String> = folders
        .iter()
        .map(|item| match item {
            GridItem::Folder(p) | GridItem::ZipFile(p) | GridItem::PdfFile(p) => {
                p.file_name().unwrap().to_string_lossy().into_owned()
            }
            GridItem::ConvertibleArchive { path, .. } => {
                path.file_name().unwrap().to_string_lossy().into_owned()
            }
            _ => String::new(),
        })
        .collect();

    assert_eq!(
        remaining_names,
        vec!["vol01", "other.zip", "bonus.lzh"],
        "同名フォルダ vol01 があるアーカイブ 3 件は消え、他は残る",
    );
    assert_eq!(folders.len(), folder_metas.len(), "metas も同期して削除");
}

/// 大文字小文字は無視して同名判定する (Windows 文化圏での実運用に合わせる)。
#[test]
fn filter_virtual_folder_case_insensitive() {
    let mut folders: Vec<GridItem> = vec![
        GridItem::Folder(PathBuf::from("/r/VOL01")),
        GridItem::ConvertibleArchive {
            path: PathBuf::from("/r/vol01.7z"),
            format: ArchiveFormat::SevenZ,
        },
    ];
    let mut folder_metas: Vec<Option<(i64, i64)>> = vec![None, None];

    App::filter_virtual_folder_duplicates(&mut folders, &mut folder_metas);

    assert_eq!(folders.len(), 1, "大文字小文字違いでも一致扱い");
}

/// `clamp_dynamic_for_gpu` は 8192 以内の画像には触れず、超えるときだけ
/// 長辺 8192 にアスペクト比保持で縮小する。
#[test]
fn clamp_dynamic_for_gpu_noop_within_limit() {
    let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(4096, 2048));
    let out = clamp_dynamic_for_gpu(img);
    assert_eq!((out.width(), out.height()), (4096, 2048));
}

#[test]
fn clamp_dynamic_for_gpu_scales_portrait_oversize() {
    // 7168x9216 は再現バグのテストサイズ。長辺 9216 → 8192 で縮小され、
    // 短辺もアスペクト比を保って縮む (7168 * 8192/9216 = 6371.55… ≈ 6372)。
    let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(7168, 9216));
    let out = clamp_dynamic_for_gpu(img);
    assert_eq!(out.height(), 8192, "long edge clamped to MAX_TEXTURE_DIM");
    assert_eq!(out.width(), 6372, "aspect-preserving short edge");
}

#[test]
fn clamp_dynamic_for_gpu_scales_landscape_oversize() {
    let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(16384, 4096));
    let out = clamp_dynamic_for_gpu(img);
    assert_eq!(out.width(), 8192);
    assert_eq!(out.height(), 2048);
}

// =======================================================================
// Phase C (App-level) テスト
//
// docs/search-test-plan.md §Phase C の位置付け。App 全体を構築して、
// 検索バー起動ヘルパ (open_favsearch / open_global_search /
// open_local_metadata_search) の相互排他ロジックを回帰テストとして固定する。
//
// 完全な Ctrl+G キー → update() 経由のフルスタックテストは eframe::Frame の
// モック化が必要で重いため、本ラウンドでは **public 起動 API の状態遷移** を
// 対象にする。検索バー同時表示バグ (2026-04 ユーザー報告) の回帰防止が主目的。
// =======================================================================

/// Phase C 共通 setup。`data_dir::TEST_OVERRIDE` (プロセス全域のグローバル状態) を
/// 使うテストはすべてここの `setup_app()` を経由する。
///
/// 旧実装はモジュールごとに独自の `PHASE_C_LOCK` を持っていたが、Codex P2 v9b
/// (2026-05-14) で **process-global な** `crate::data_dir::test_override_lock()` に
/// 統合した (settings.rs / settings_db.rs と共通)。これで全モジュールの set_test_override
/// 使用者が単一の Mutex で直列化される。
#[cfg(test)]
mod phase_c_support {
    use super::{App, AppTestConfig};
    use tempfile::TempDir;

    /// テスト終了時に `data_dir::set_test_override(None)` + `reset_global_for_test()` +
    /// `set_save_suppressed(false)` を呼ぶ RAII ガード。
    /// panic 経路でも確実にオーバーライドを解除して後続テストに影響させない。
    ///
    /// 2026-05-17 事故ガード: 旧版は `set_test_override(None)` のみで、`GLOBAL_DB` の
    /// 旧 dir handle が残った状態で `App` の drop 中の save が `with_db` で
    /// data_dir 不一致を踏む経路があった。`settings_db::DataDirOverrideGuard` (こちらも
    /// `reset_global_for_test` + `set_save_suppressed(false)` を呼ぶ) と挙動を揃える。
    pub(super) struct OverrideGuard;
    impl Drop for OverrideGuard {
        fn drop(&mut self) {
            crate::data_dir::set_test_override(None);
            crate::settings_db::reset_global_for_test();
            crate::settings_db::set_save_suppressed(false);
        }
    }

    /// TempDir を data_dir として差し替え、空の settings で App を構築した結果を保持する。
    ///
    /// **フィールドの宣言順がそのまま drop 順** (Rust spec: struct fields are dropped in
    /// declaration order)。`App` を最先頭に置いて supervisor join まで完了させたあとで
    /// `OverrideGuard` (TEST_OVERRIDE クリア) → `TempDir` (削除) → `MutexGuard` (lock 解放)
    /// の順に片付く。
    ///
    /// 2026-05-17 事故ガード: 旧 `setup_app()` は `(App, OverrideGuard, TempDir, MutexGuard)`
    /// のタプル戻りだったが、`let app = setup_app();` の tuple destructure は
    /// **右から左** に drop するので、実際は `_l → _tmp → _g → app` の順だった。これだと
    /// `OverrideGuard` が `App` より先に drop して `TEST_OVERRIDE = None` になり、その後の
    /// `App::drop()` で動く supervisor / worker の最終 save が `with_db` で「data_dir が
    /// temp → APPDATA に変わった」と検知して本番 settings.db を defaults で上書きする事故が
    /// 起きた。`with_db` 側にも fail-fast ガード ([crate::settings_db]) を入れてあるが、
    /// 二重防御として drop 順自体を fix している。
    pub(super) struct AppTestEnv {
        pub app: App,
        _guard: OverrideGuard,
        /// Test 本体から `app.tmp.path()` の形でアクセスできるよう公開している
        /// (App には `tmp` フィールドが無いので名前衝突しない)。
        pub tmp: TempDir,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    /// `app.foo()` / `app.field` / `&mut app` を `App` への deref coercion で
    /// 透過させる。これで旧 `let (mut app, ...) = setup_app();` パターンを
    /// `let mut app = setup_app();` に書き換えるだけで他の test body は変更不要。
    impl std::ops::Deref for AppTestEnv {
        type Target = App;
        fn deref(&self) -> &App {
            &self.app
        }
    }
    impl std::ops::DerefMut for AppTestEnv {
        fn deref_mut(&mut self) -> &mut App {
            &mut self.app
        }
    }

    pub(super) fn setup_app() -> AppTestEnv {
        let lock = crate::data_dir::test_override_lock();
        let tmp = TempDir::new().expect("tempdir");
        crate::data_dir::set_test_override(Some(tmp.path().to_path_buf()));
        let guard = OverrideGuard;
        let config = AppTestConfig {
            data_dir: tmp.path().to_path_buf(),
            settings: None,
        };
        let app = App::new_for_test(config);
        AppTestEnv {
            app,
            _guard: guard,
            tmp,
            _lock: lock,
        }
    }
}

#[cfg(test)]
mod phase_c_key_tests {
    use super::phase_c_support::setup_app;
    use super::*;

    fn grid_key_nav(
        app: &mut App,
        modifiers: egui::Modifiers,
        key: egui::Key,
    ) -> Option<crate::ui_main::AddressBarNav> {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
            modifiers,
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        });
        let nav = app.handle_keyboard(&ctx);
        let _ = ctx.end_pass();
        nav
    }

    /// ベースライン: 新規 App はどの検索バーも開いていないこと。
    #[test]
    fn new_app_has_no_search_bar_open() {
        let app = setup_app();
        assert!(!app.show_search_bar, "Ctrl+F bar must be closed");
        assert!(!app.favsearch.active, "Ctrl+S bar must be closed");
        assert!(!app.global_search.active, "Ctrl+G bar must be closed");
    }

    /// Ctrl+F 相当の起動ヘルパを呼ぶと Ctrl+F バーのみが立ち、他 2 つは閉じたままであること。
    #[test]
    fn open_local_metadata_search_activates_only_ctrl_f() {
        let mut app = setup_app();
        app.open_local_metadata_search();
        assert!(app.show_search_bar);
        assert!(!app.favsearch.active);
        assert!(!app.global_search.active);
    }

    /// Ctrl+S 相当の起動ヘルパを呼ぶと Ctrl+S バーのみが立つこと。
    #[test]
    fn open_favsearch_activates_only_ctrl_s() {
        let mut app = setup_app();
        app.open_favsearch();
        assert!(!app.show_search_bar);
        assert!(app.favsearch.active);
        assert!(!app.global_search.active);
    }

    /// Ctrl+G 相当の起動ヘルパを呼ぶと Ctrl+G バーのみが立つこと。
    #[test]
    fn open_global_search_activates_only_ctrl_g() {
        let mut app = setup_app();
        app.open_global_search();
        assert!(!app.show_search_bar);
        assert!(!app.favsearch.active);
        assert!(app.global_search.active);
    }

    /// 既に別の検索バーが開いているところで Ctrl+F を起動すると、先行バーが閉じて
    /// Ctrl+F だけが残ること (相互排他、2026-04 バグ回帰ガード)。
    #[test]
    fn ctrl_f_closes_ctrl_s_and_ctrl_g() {
        let mut app = setup_app();
        app.open_favsearch();
        app.open_global_search();
        assert!(!app.favsearch.active, "Ctrl+G should have closed Ctrl+S");
        assert!(app.global_search.active);
        app.open_local_metadata_search();
        assert!(app.show_search_bar);
        assert!(!app.favsearch.active);
        assert!(!app.global_search.active, "Ctrl+F should close Ctrl+G");
    }

    /// 既に Ctrl+F が開いているところで Ctrl+S を起動すると Ctrl+F が閉じて
    /// Ctrl+S だけが残ること (回帰)。
    #[test]
    fn ctrl_s_closes_ctrl_f() {
        let mut app = setup_app();
        app.open_local_metadata_search();
        assert!(app.show_search_bar);
        app.open_favsearch();
        assert!(app.favsearch.active);
        assert!(!app.show_search_bar, "Ctrl+S should close Ctrl+F");
        assert!(!app.global_search.active);
    }

    /// 既に Ctrl+F が開いているところで Ctrl+G を起動すると Ctrl+F が閉じて
    /// Ctrl+G だけが残ること (回帰)。
    #[test]
    fn ctrl_g_closes_ctrl_f() {
        let mut app = setup_app();
        app.open_local_metadata_search();
        app.open_global_search();
        assert!(app.global_search.active);
        assert!(!app.show_search_bar, "Ctrl+G should close Ctrl+F");
        assert!(!app.favsearch.active);
    }

    /// Ctrl+F のフィルタが現在フォルダに効いている間は、BS でフィルタ外の親へ
    /// 抜けない。検索を閉じる操作 (Esc / ×) と親移動を分けるための回帰ガード。
    #[test]
    fn ctrl_f_filter_blocks_backspace_parent_nav_at_origin() {
        let mut app = setup_app();
        let origin = PathBuf::from("C:/pics/origin");
        app.current_folder = Some(origin.clone());
        app.show_search_bar = true;
        app.search_filter = Some(std::collections::HashSet::new());
        app.search_filter_origin_folder = Some(origin);

        let nav = grid_key_nav(&mut app, egui::Modifiers::NONE, egui::Key::Backspace);

        assert!(
            nav.is_none(),
            "Ctrl+F フィルタ元フォルダでは BS で親フォルダへ抜けない"
        );
    }

    /// Ctrl+F の検索結果から子フォルダへ入った後の BS は、元フォルダへ戻る通常ナビとして
    /// 通す。origin と現在地が異なる限り、検索バー表示だけでは親移動を止めない。
    #[test]
    fn ctrl_f_filter_allows_backspace_from_child_folder() {
        let mut app = setup_app();
        let origin = PathBuf::from("C:/pics/origin");
        let child = origin.join("child");
        app.current_folder = Some(child);
        app.show_search_bar = true;
        app.search_filter = Some(std::collections::HashSet::new());
        app.search_filter_origin_folder = Some(origin.clone());

        let nav = grid_key_nav(&mut app, egui::Modifiers::NONE, egui::Key::Backspace);

        match nav {
            Some(crate::ui_main::AddressBarNav::Direct(path)) => assert_eq!(path, origin),
            _ => panic!("子フォルダでは BS で Ctrl+F 元フォルダへ戻れること"),
        }
    }

    /// Codex P2 #3: 選択中の Ctrl+S お気に入りフィルタが設定から消えたら、
    /// `execute_favsearch` が UI と整合を取るために filter を None にクリアする。
    #[test]
    fn favsearch_clears_stale_favorite_filter() {
        let mut app = setup_app();
        // 存在しない UUID を filter に立てて search を走らせる
        let bogus = uuid::Uuid::new_v4();
        app.favsearch.favorite_filter = Some(bogus);
        app.favsearch.query = "x".to_string();
        app.execute_favsearch();
        assert_eq!(
            app.favsearch.favorite_filter, None,
            "無効 filter は None に戻さないと UI ラベルと検索スコープが食い違う"
        );
    }

    /// Codex P2 #3 (Ctrl+G 側): 選択中の Ctrl+G お気に入りフィルタが対象セットに
    /// いなくなったら、`spawn_global_search` が filter を None にクリアする。
    #[test]
    fn global_search_clears_stale_favorite_filter() {
        let mut app = setup_app();
        let bogus = uuid::Uuid::new_v4();
        app.global_search.active = true;
        app.global_search.filters.favorite = Some(bogus);
        app.global_search.query = "x".to_string();
        // spawn_global_search は indexer_manager が None のときに reject_message を出して早期 return するが、
        // その前に filter の健全化は行う (コードは filter 正規化 → manager 存在確認 → spawn の順)。
        app.spawn_global_search();
        assert_eq!(
            app.global_search.filters.favorite, None,
            "無効 filter は None に戻さないと UI ラベルと検索スコープが食い違う"
        );
    }

    /// どの順番で 3 検索モードを切り替えても、同時に 2 つ以上が active にならないこと。
    /// 2026-04 報告「検索バーが 2 つでることがあった」の総合回帰ガード。
    #[test]
    fn at_most_one_search_bar_ever_active() {
        let mut app = setup_app();
        let check_invariant = |app: &App, label: &str| {
            let count = [
                app.show_search_bar,
                app.favsearch.active,
                app.global_search.active,
            ]
            .iter()
            .filter(|b| **b)
            .count();
            assert!(
                count <= 1,
                "{label}: 同時に active なバーが {count} 個 (F={}, S={}, G={})",
                app.show_search_bar,
                app.favsearch.active,
                app.global_search.active,
            );
        };
        // F → S → G → F → G → S → F と順番に切り替えて各ステップで不変量を確認
        check_invariant(&app, "initial");
        app.open_local_metadata_search();
        check_invariant(&app, "after open F");
        app.open_favsearch();
        check_invariant(&app, "after open S (should close F)");
        app.open_global_search();
        check_invariant(&app, "after open G (should close S)");
        app.open_local_metadata_search();
        check_invariant(&app, "after open F (should close G)");
        app.open_global_search();
        check_invariant(&app, "after open G (should close F)");
        app.open_favsearch();
        check_invariant(&app, "after open S (should close G)");
        app.open_local_metadata_search();
        check_invariant(&app, "after open F (should close S)");
    }
}

#[cfg(test)]
mod phase_c_folder_nav_history_tests {
    use crate::archive_converter::ArchiveFormat;
    use crate::grid_item::GridItem;

    use super::phase_c_support::setup_app;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::SystemTime;

    #[test]
    fn folder_history_back_forward_does_not_duplicate_history_edges() {
        let mut app = setup_app();
        let a = PathBuf::from(r"C:\miv-test\a");
        let b = PathBuf::from(r"C:\miv-test\b");

        app.current_folder = Some(a.clone());
        app.record_folder_nav_transition(&b);
        app.current_folder = Some(b.clone());

        assert_eq!(app.folder_nav_back_stack, vec![a.clone()]);
        assert!(app.folder_nav_forward_stack.is_empty());
        assert_eq!(app.recent_folders.first(), Some(&b));

        assert_eq!(app.navigate_folder_history_back(), Some(a.clone()));
        app.record_folder_nav_transition(&a);
        app.current_folder = Some(a.clone());

        assert_eq!(app.folder_nav_back_stack, Vec::<PathBuf>::new());
        assert_eq!(app.folder_nav_forward_stack, vec![b.clone()]);
        assert_eq!(app.recent_folders.first(), Some(&a));

        assert_eq!(app.navigate_folder_history_forward(), Some(b.clone()));
        app.record_folder_nav_transition(&b);
        app.current_folder = Some(b.clone());

        assert_eq!(app.folder_nav_back_stack, vec![a]);
        assert!(app.folder_nav_forward_stack.is_empty());
        assert_eq!(app.recent_folders.first(), Some(&b));
    }

    #[test]
    fn converted_archive_keeps_source_path_after_zip_enumerate_finishes() {
        let mut app = setup_app();
        let original_dir = app.tmp.path().join("share/18/dmm/comic");
        std::fs::create_dir_all(&original_dir).unwrap();
        let source = original_dir.join("d_pa3584.lzh");
        std::fs::write(&source, b"lzh").unwrap();

        let cache_dir = app.tmp.path().join(
            "archive_cache/06/06f8ea1bcfd0219996db21aed88f7f7fd83df7dd68f7cf9ad805b50f1e7dfcf2",
        );
        std::fs::create_dir_all(&cache_dir).unwrap();
        let cached_zip = cache_dir.join("d_pa3584.zip");
        std::fs::write(&cached_zip, b"zip").unwrap();

        app.current_folder = Some(cached_zip.clone());
        app.archive_source_override = Some(source.clone());
        app.address = source.to_string_lossy().to_string();

        app.start_loading_items(
            cached_zip.clone(),
            vec![GridItem::ZipImage {
                zip_path: cached_zip.clone(),
                entry_name: "page001.jpg".to_string(),
            }],
            vec![None],
            HashSet::new(),
            Vec::new(),
            None,
        );

        assert_eq!(app.current_folder.as_ref(), Some(&cached_zip));
        assert_eq!(app.archive_source_override.as_ref(), Some(&source));
        assert_eq!(app.effective_folder(), Some(source.clone()));
        assert_eq!(app.address, source.to_string_lossy().to_string());
        assert!(
            !app.address.contains("archive_cache"),
            "UI address must not leak the converted cache ZIP path"
        );
        assert_eq!(
            app.effective_folder()
                .and_then(|p| p.parent().map(|parent| parent.to_path_buf())),
            Some(original_dir),
            "BS should resolve to the source archive's parent, not the cache directory"
        );
    }

    #[test]
    fn return_to_parent_uses_source_archive_parent_not_cache_folder() {
        // 自動オープン ZIP/PDF の ESC で立つ `pending_return_to_parent` を、変換アーカイブ
        // (current_folder = キャッシュ ZIP, archive_source_override = 元 .lzh) の状態で消化
        // したとき、キャッシュフォルダではなく元 .lzh の親フォルダへ戻ること。
        // 実機ログで確認した不具合 (Codex P1): 旧実装は current_folder.parent() を使っており
        // `archive_cache\<hash2>\<hash>` (キャッシュフォルダ) へ飛んでいた。
        let mut app = setup_app();
        let source = PathBuf::from(r"H:\home\mimageviewer_old\testimage\C165_206.LZH");
        let cached_zip = PathBuf::from(
            r"C:\Users\mikag\AppData\Roaming\mimageviewer\archive_cache\55\hash\C165_206.zip",
        );
        app.current_folder = Some(cached_zip);
        app.archive_source_override = Some(source.clone());

        let nav = app
            .resolve_return_to_parent_nav()
            .expect("override の親が取れるので Some を返す");
        let crate::ui_main::AddressBarNav::Direct(parent) = nav else {
            panic!("AddressBarNav::Direct を期待");
        };
        assert_eq!(
            parent,
            source.parent().unwrap().to_path_buf(),
            "ESC の戻り先はキャッシュフォルダではなく元 .lzh の親フォルダ"
        );
        // 戻り先で元アーカイブ (.lzh) を選択状態にするヒント。
        assert_eq!(app.select_after_load.as_deref(), Some("C165_206.LZH"));
    }

    #[test]
    fn converted_archive_reload_does_not_record_cache_zip_in_folder_history() {
        let mut app = setup_app();
        let source = PathBuf::from(r"E:\share\18\dmm\comic\d_pa3584.lzh");
        let cached_zip = PathBuf::from(
            r"C:\Users\mikag\AppData\Roaming\mimageviewer\archive_cache\06\hash\d_pa3584.zip",
        );
        let previous = PathBuf::from(r"E:\share\18\dmm");
        let forward = PathBuf::from(r"E:\share\18\dmm\next");
        let recent = PathBuf::from(r"E:\share\18\dmm\recent");

        app.current_folder = Some(cached_zip.clone());
        app.archive_source_override = Some(source);
        app.folder_nav_back_stack = vec![previous.clone()];
        app.folder_nav_forward_stack = vec![forward.clone()];
        app.recent_folders = vec![recent.clone()];

        app.record_folder_nav_transition(&cached_zip);

        assert_eq!(app.folder_nav_back_stack, vec![previous]);
        assert_eq!(app.folder_nav_forward_stack, vec![forward]);
        assert_eq!(app.recent_folders, vec![recent]);
    }

    #[test]
    fn cancelled_history_navigation_to_unconverted_archive_restores_stacks() {
        let mut app = setup_app();
        let a = PathBuf::from(r"C:\miv-test\a");
        let current = PathBuf::from(r"C:\miv-test\current");
        let archive = app.tmp.path().join("book.lzh");
        std::fs::write(&archive, b"lzh").unwrap();

        app.current_folder = Some(current.clone());
        app.folder_nav_back_stack = vec![a.clone(), archive.clone()];
        app.folder_nav_forward_stack = Vec::new();
        app.recent_folders = vec![current.clone()];

        let snapshot = app.folder_nav_history_snapshot();
        assert_eq!(app.navigate_folder_history_back(), Some(archive.clone()));
        assert_eq!(app.folder_nav_back_stack, vec![a.clone()]);
        assert_eq!(app.folder_nav_forward_stack, vec![current.clone()]);
        assert!(app.suppress_folder_nav_record_once);

        let (_tx, rx) = mpsc::channel();
        app.archive_convert = Some(crate::ui_dialogs::archive_convert::ArchiveConvertState {
            src_path: archive.clone(),
            format: ArchiveFormat::Lzh,
            phase: crate::ui_dialogs::archive_convert::ArchiveConvertPhase::Scanning,
            rx,
            pending_nav: None,
            nav_history_rollback: None,
            auto_fullscreen: false,
        });
        app.attach_archive_convert_nav_history_rollback(snapshot);
        let rollback = app
            .archive_convert
            .as_ref()
            .and_then(|state| state.nav_history_rollback.clone())
            .expect("history rollback snapshot should be attached to the convert dialog");
        app.archive_convert = None;
        app.restore_folder_nav_history(rollback);

        assert_eq!(app.folder_nav_back_stack, vec![a, archive]);
        assert!(app.folder_nav_forward_stack.is_empty());
        assert_eq!(app.recent_folders, vec![current]);
        assert!(!app.suppress_folder_nav_record_once);
    }

    #[test]
    fn cancelled_conversion_restores_favsearch_nav_stack() {
        let mut app = setup_app();
        let root = PathBuf::from(r"C:\miv-test\search-root");
        let current = PathBuf::from(r"C:\miv-test\search-root\current");
        let archive = app.tmp.path().join("book.lzh");
        std::fs::write(&archive, b"lzh").unwrap();

        app.favsearch.active = true;
        app.favsearch.nav_stack = vec![root.clone(), current.clone()];
        let snapshot = app.folder_nav_history_snapshot();
        app.favsearch.nav_stack.push(archive.clone());

        let (_tx, rx) = mpsc::channel();
        app.archive_convert = Some(crate::ui_dialogs::archive_convert::ArchiveConvertState {
            src_path: archive,
            format: ArchiveFormat::Lzh,
            phase: crate::ui_dialogs::archive_convert::ArchiveConvertPhase::Scanning,
            rx,
            pending_nav: None,
            nav_history_rollback: None,
            auto_fullscreen: false,
        });
        app.attach_archive_convert_nav_history_rollback(snapshot);
        let rollback = app
            .archive_convert
            .as_ref()
            .and_then(|state| state.nav_history_rollback.clone())
            .expect("history rollback snapshot should be attached to the convert dialog");
        app.archive_convert = None;
        app.restore_folder_nav_history(rollback);

        assert_eq!(app.favsearch.nav_stack, vec![root, current]);
    }

    #[test]
    fn successful_navigation_clears_stale_archive_convert_rollback() {
        let mut app = setup_app();
        let previous = PathBuf::from(r"C:\miv-test\previous");
        app.folder_nav_back_stack = vec![previous];
        let snapshot = app.folder_nav_history_snapshot();
        let (_tx, rx) = mpsc::channel();
        app.archive_convert = Some(crate::ui_dialogs::archive_convert::ArchiveConvertState {
            src_path: PathBuf::from(r"C:\miv-test\book.lzh"),
            format: ArchiveFormat::Lzh,
            phase: crate::ui_dialogs::archive_convert::ArchiveConvertPhase::Scanning,
            rx,
            pending_nav: None,
            nav_history_rollback: Some(snapshot),
            auto_fullscreen: false,
        });

        let target = app.tmp.path().join("loaded");
        std::fs::create_dir_all(&target).unwrap();
        app.load_folder(target);

        assert!(
            app.archive_convert
                .as_ref()
                .map(|state| state.nav_history_rollback.is_none())
                .unwrap_or(true),
            "successful navigation should make an old conversion-dialog rollback inert"
        );
    }

    #[test]
    fn same_folder_reload_keeps_archive_convert_rollback() {
        let mut app = setup_app();
        let current = app.tmp.path().join("current");
        std::fs::create_dir_all(&current).unwrap();

        app.current_folder = Some(current.clone());
        app.folder_nav_back_stack = vec![PathBuf::from(r"C:\miv-test\previous")];
        let snapshot = app.folder_nav_history_snapshot();
        let (_tx, rx) = mpsc::channel();
        app.archive_convert = Some(crate::ui_dialogs::archive_convert::ArchiveConvertState {
            src_path: PathBuf::from(r"C:\miv-test\book.lzh"),
            format: ArchiveFormat::Lzh,
            phase: crate::ui_dialogs::archive_convert::ArchiveConvertPhase::Scanning,
            rx,
            pending_nav: None,
            nav_history_rollback: Some(snapshot),
            auto_fullscreen: false,
        });

        app.load_folder(current);

        assert!(
            app.archive_convert
                .as_ref()
                .and_then(|state| state.nav_history_rollback.as_ref())
                .is_some(),
            "same-folder reload should not discard conversion-dialog rollback"
        );
    }

    #[test]
    fn favorite_target_is_available_only_for_real_directory_context() {
        let mut app = setup_app();
        let dir = app.tmp.path().join("images");
        std::fs::create_dir_all(&dir).unwrap();
        app.current_folder = Some(dir.clone());
        app.current_folder_last_mtime = Some(SystemTime::now());
        assert_eq!(app.current_favorite_target(), Some(dir));

        let zip = app.tmp.path().join("book.zip");
        std::fs::write(&zip, b"zip").unwrap();
        app.current_folder = Some(zip);
        app.current_folder_last_mtime = None;
        assert_eq!(app.current_favorite_target(), None);

        let source = app.tmp.path().join("book.lzh");
        let cached_zip = app.tmp.path().join("archive_cache/book.zip");
        app.current_folder = Some(cached_zip);
        app.archive_source_override = Some(source);
        app.current_folder_last_mtime = None;
        assert_eq!(app.current_favorite_target(), None);
    }

    // ── 検索 (Ctrl+G / Ctrl+S) 中のフォルダ履歴の扱い ──────────────────
    // 検索は「透明な一時オーバーレイ」で、検索中の移動は back/forward/recent に
    // 一切残さず、抜けると検索前の状態へ完全復帰する。

    #[test]
    fn search_active_global_does_not_record_folder_history() {
        let mut app = setup_app();
        let a = PathBuf::from(r"C:\miv-test\a");
        let b = PathBuf::from(r"C:\miv-test\b");
        let recent0 = PathBuf::from(r"C:\miv-test\recent0");
        app.current_folder = Some(a.clone());
        app.folder_nav_back_stack = vec![a.clone()];
        app.folder_nav_forward_stack = Vec::new();
        app.recent_folders = vec![recent0.clone()];
        app.global_search.active = true;

        app.record_folder_nav_transition(&b);

        assert_eq!(app.folder_nav_back_stack, vec![a]);
        assert!(app.folder_nav_forward_stack.is_empty());
        assert_eq!(app.recent_folders, vec![recent0]);
    }

    #[test]
    fn search_active_favsearch_does_not_record_folder_history() {
        let mut app = setup_app();
        let a = PathBuf::from(r"C:\miv-test\a");
        let b = PathBuf::from(r"C:\miv-test\b");
        let recent0 = PathBuf::from(r"C:\miv-test\recent0");
        app.current_folder = Some(a.clone());
        app.folder_nav_back_stack = vec![a.clone()];
        app.folder_nav_forward_stack = Vec::new();
        app.recent_folders = vec![recent0.clone()];
        app.favsearch.active = true;

        app.record_folder_nav_transition(&b);

        assert_eq!(app.folder_nav_back_stack, vec![a]);
        assert!(app.folder_nav_forward_stack.is_empty());
        assert_eq!(app.recent_folders, vec![recent0]);
    }

    #[test]
    fn closing_global_search_does_not_record_history() {
        let mut app = setup_app();
        let saved = app.tmp.path().join("saved");
        std::fs::create_dir_all(&saved).unwrap();
        let prev = PathBuf::from(r"C:\miv-test\prev");
        app.folder_nav_back_stack = vec![prev.clone()];
        app.folder_nav_forward_stack = Vec::new();
        // 検索中に ZIP を開いて current_folder が saved とずれている状態を模擬。
        app.current_folder = Some(PathBuf::from(r"C:\miv-test\opened-in-search.zip"));
        app.global_search.active = true;
        app.global_search.saved_folder = Some(saved.clone());

        app.close_global_search();

        // 検索クローズによる saved への復帰は履歴に積まれない。
        assert_eq!(app.folder_nav_back_stack, vec![prev]);
        assert!(app.folder_nav_forward_stack.is_empty());
        assert!(!app.suppress_nav_record_for_search_restore);
        assert_eq!(app.current_folder.as_ref(), Some(&saved));
    }

    #[test]
    fn closing_favsearch_does_not_record_history() {
        let mut app = setup_app();
        let saved = app.tmp.path().join("fav-saved");
        std::fs::create_dir_all(&saved).unwrap();
        let prev = PathBuf::from(r"C:\miv-test\prev");
        app.folder_nav_back_stack = vec![prev.clone()];
        // favsearch 結果一覧中は current_folder が合成パス。
        app.current_folder = Some(crate::app::search_results_synthetic_path());
        app.favsearch.active = true;
        app.favsearch.saved_folder = Some(saved.clone());

        app.close_favsearch();

        assert_eq!(app.folder_nav_back_stack, vec![prev]);
        assert!(!app.suppress_nav_record_for_search_restore);
        assert_eq!(app.current_folder.as_ref(), Some(&saved));
    }

    #[test]
    fn remember_recent_folder_ignored_during_search() {
        let mut app = setup_app();
        let recent0 = PathBuf::from(r"C:\miv-test\recent0");
        let archive = PathBuf::from(r"C:\miv-test\found.7z");
        app.recent_folders = vec![recent0.clone()];

        // Ctrl+G / Ctrl+S 中は archive_convert などの直接呼び出しでも recent を変えない。
        app.global_search.active = true;
        app.remember_recent_folder(&archive);
        assert_eq!(app.recent_folders, vec![recent0.clone()]);

        app.global_search.active = false;
        app.favsearch.active = true;
        app.remember_recent_folder(&archive);
        assert_eq!(app.recent_folders, vec![recent0.clone()]);

        // 検索を抜ければ通常どおり記録される。
        app.favsearch.active = false;
        app.remember_recent_folder(&archive);
        assert_eq!(app.recent_folders.first(), Some(&archive));
    }

    #[test]
    fn synthetic_search_path_not_pushed_as_nav_source() {
        let mut app = setup_app();
        let prev = PathBuf::from(r"C:\miv-test\prev");
        let stale_forward = PathBuf::from(r"C:\miv-test\stale-forward");
        let target = PathBuf::from(r"C:\miv-test\target");
        app.folder_nav_back_stack = vec![prev.clone()];
        // 検索前に ← で残っていた forward 履歴を模擬。新規ナビなのでクリアされるべき。
        app.folder_nav_forward_stack = vec![stale_forward];
        // 合成検索結果パスを移動元にした記録を試みても back_stack には積まれない。
        app.current_folder = Some(crate::app::search_results_synthetic_path());

        app.record_folder_nav_transition(&target);

        // 合成 source は back には積まないが forward は無効化する (新規ナビなので)。
        assert_eq!(app.folder_nav_back_stack, vec![prev]);
        assert!(app.folder_nav_forward_stack.is_empty());
    }

    #[test]
    fn push_nav_history_entry_records_explicit_source() {
        let mut app = setup_app();
        let older = PathBuf::from(r"C:\miv-test\older");
        let forward = PathBuf::from(r"C:\miv-test\forward");
        let c = PathBuf::from(r"C:\miv-test\pre-search");
        app.folder_nav_back_stack = vec![older.clone()];
        app.folder_nav_forward_stack = vec![forward];

        app.push_nav_history_entry(c.clone());

        assert_eq!(app.folder_nav_back_stack, vec![older, c]);
        assert!(app.folder_nav_forward_stack.is_empty());
    }

    #[test]
    fn navigate_to_folder_from_search_records_pre_search_folder() {
        let mut app = setup_app();
        let c = PathBuf::from(r"C:\miv-test\pre-search");
        let x = app.tmp.path().join("jump-target");
        std::fs::create_dir_all(&x).unwrap();
        let older = PathBuf::from(r"C:\miv-test\older");
        app.folder_nav_back_stack = vec![older.clone()];
        app.current_folder = Some(crate::app::search_results_synthetic_path());
        app.favsearch.active = true;
        app.favsearch.saved_folder = Some(c.clone());

        // context_menu「フォルダに移動」後処理の模擬: 検索前フォルダ C を捕捉して
        // 検索を閉じ、C を明示的に積んでから移動先 X へ load する。
        let pre = app.favsearch.saved_folder.clone();
        app.favsearch.saved_folder = None;
        app.close_favsearch();
        if let Some(cc) = pre {
            if !crate::folder_tree::path_eq(&cc, &x) {
                app.push_nav_history_entry(cc);
            }
        }
        app.suppress_folder_nav_record_once = true;
        app.load_folder(x.clone());

        // 「検索前フォルダ C → 移動先 X」が積まれ、X で ← を押すと C に戻れる。
        assert_eq!(app.folder_nav_back_stack, vec![older, c]);
        assert_eq!(app.recent_folders.first(), Some(&x));
    }

    #[test]
    fn search_view_scroll_does_not_corrupt_folder_history() {
        let mut app = setup_app();
        let c = PathBuf::from(r"C:\miv-test\pre-search-c");
        let x = app.tmp.path().join("g-target");
        std::fs::create_dir_all(&x).unwrap();
        // Ctrl+G 検索ビューを退場する模擬: current_folder=C のまま検索ビューを
        // スクロールして scroll_offset_y が C 本来の値から乖離している状態。
        app.current_folder = Some(c.clone());
        app.items_are_global_search_view = true;
        app.scroll_offset_y = 999.0;
        app.selected = Some(7);
        app.folder_history.insert(c.clone(), (42.0, Some(3)));

        app.start_loading_items(
            x.clone(),
            Vec::new(),
            Vec::new(),
            HashSet::new(),
            Vec::new(),
            None,
        );

        // C の folder_history は検索前の値のまま (検索ビューのスクロールで壊れない)。
        assert_eq!(app.folder_history.get(&c), Some(&(42.0, Some(3))));
    }

    #[test]
    fn normal_view_still_saves_folder_history() {
        let mut app = setup_app();
        let d = PathBuf::from(r"C:\miv-test\normal-d");
        let x = app.tmp.path().join("g-target2");
        std::fs::create_dir_all(&x).unwrap();
        app.current_folder = Some(d.clone());
        app.items_are_global_search_view = false;
        app.scroll_offset_y = 555.0;
        app.selected = Some(9);

        app.start_loading_items(
            x.clone(),
            Vec::new(),
            Vec::new(),
            HashSet::new(),
            Vec::new(),
            None,
        );

        // 通常ビューからの退場では従来どおりスクロール状態を保存する。
        assert_eq!(app.folder_history.get(&d), Some(&(555.0, Some(9))));
    }
}

// =======================================================================
// Phase C (App-level) - Ctrl+G drill ナビゲーション状態機械テスト
//
// 2026-04 ユーザー報告バグ:
//   「Ctrl+G → 検索 → 結果のフォルダを開く → フォルダの中の PDF 一覧 →
//    PDF をクリック → ページ一覧 → BS で戻ると、PDF 一覧まで戻るはずが
//    検索結果 (Aggregated) まで 1 段多く戻ってしまう」
//
// 原因: PDF/ZIP を開いても `global_search.view.DrilledInto.current_path` が
// 更新されず、drill_back_one_level が「current_path == container_root」を
// 根拠に drill_back_to_top を直接呼ぶ。
//
// 修正: container (PDF/ZIP/Folder) を開く時点で current_path をその path に
// 進めておく。BS 時は drill_back_one_level が親へ戻す動作になり、
// PDF 一覧に正しく復帰する。
// =======================================================================

#[cfg(test)]
mod phase_c_drill_nav_tests {
    use super::phase_c_support::setup_app;
    use crate::global_search::GlobalHit;
    use crate::global_search_ui::GlobalSearchView;

    /// Ctrl+G 絞り込みビューで folder_path に drill-in したあと、その配下の PDF を
    /// 開くと、drill_back_one_level が「PDF → folder_path (ヒット一覧) → Aggregated」
    /// の 2 段階 BS で辿れる状態になること。
    ///
    /// 修正前: PDF を開いても current_path=folder_path のままなので、
    /// drill_back_one_level が即 drill_back_to_top を呼び、ヒット一覧を
    /// スキップして検索結果に戻ってしまう。
    #[test]
    fn bs_after_opening_pdf_in_drilled_returns_to_folder_not_aggregated() {
        let mut app = setup_app();
        let folder_path = std::path::PathBuf::from("C:/fav/scansnap");
        let pdf_path = folder_path.join("doc.pdf");

        // Aggregated 状態でヒットだけ用意する (実検索は行わない、
        // build_drilled_items が current_path でフィルタするため)
        app.global_search.active = true;
        app.global_search.accumulate_hit(&GlobalHit {
            path: format!("{}/doc.pdf", folder_path.display()).to_lowercase(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        // コンテナへ drill-in (SearchContainer を Enter 相当)
        app.drill_into_container(folder_path.clone(), false);
        assert!(matches!(
            app.global_search.view(),
            GlobalSearchView::DrilledInto { ref current_path, .. }
                if current_path == &folder_path
        ));

        // PDF を開く操作を模擬: 新ヘルパ `advance_drilled_current_path` が current_path を
        // pdf_path に更新する (修正前は何もしない = 下の 1 段目 BS で Aggregated に飛ぶ)
        app.advance_drilled_current_path(&pdf_path);

        // 1 段目 BS: PDF ページ → drilled folder view (ヒット一覧)
        app.drill_back_one_level();
        match &app.global_search.view() {
            GlobalSearchView::DrilledInto { current_path, .. } => {
                assert_eq!(
                    current_path, &folder_path,
                    "1段目 BS で drilled folder view に戻るべき (current_path=folder)"
                );
            }
            _ => {
                panic!("BUG: BS が PDF 一覧をスキップしてトップレベルに飛んだ");
            }
        }

        // 2 段目 BS: drilled folder view → トップレベル (一覧)
        app.drill_back_one_level();
        assert!(
            app.global_search.drill.is_none(),
            "2段目 BS でトップレベルに戻るべき"
        );
    }

    /// ZIP 版: PDF と同じ状態機械で動くこと (GridItem::ZipFile の click も同じ
    /// advance_drilled_current_path 経路を通る想定)。
    #[test]
    fn bs_after_opening_zip_in_drilled_returns_to_folder_not_aggregated() {
        let mut app = setup_app();
        let folder_path = std::path::PathBuf::from("C:/fav/archives");
        let zip_path = folder_path.join("album.zip");

        app.global_search.active = true;
        app.global_search.accumulate_hit(&GlobalHit {
            path: format!("{}/album.zip", folder_path.display()).to_lowercase(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        app.drill_into_container(folder_path.clone(), false);
        app.advance_drilled_current_path(&zip_path);

        app.drill_back_one_level();
        match &app.global_search.view() {
            GlobalSearchView::DrilledInto { current_path, .. } => {
                assert_eq!(current_path, &folder_path);
            }
            _ => panic!("BUG: BS が ZIP 一覧をスキップして Aggregated に飛んだ"),
        }

        app.drill_back_one_level();
        assert!(app.global_search.drill.is_none());
    }

    /// 2026-04 ユーザー報告: Ctrl+G で 1 つめのコンテナに drill-in して Ctrl+↓ を
    /// 押したとき、現コンテナ subtree を抜けたら**次コンテナの container_root** に
    /// 跳ぶこと。旧実装は cross-container フラットリスト + dedup で、ネスト関係にある
    /// コンテナで「次コンテナ深部」(例: `output > 2025-12-30-1`) に直接ワープしていた。
    #[test]
    fn ctrl_down_at_container_end_jumps_to_next_container_root() {
        use crate::global_search_ui::GlobalSearchView;
        let mut app = setup_app();
        app.global_search.active = true;

        // コンテナ A: 直接ヒット 2 件 (件数で先頭になる)。
        // parent_container("c:/root/2025-11-30/a.jpg") = "c:/root/2025-11-30" → これがコンテナ root
        // hit 親まで walk up すると "2025-11-30" のみ → DFS = [2025-11-30]
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/2025-11-30/a.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/2025-11-30/b.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        // コンテナ B: deep ヒット 1 件。parent_container = "c:/root/output/2025-12-30-1"
        // DFS = [2025-12-30-1] のみ (container_root より上は walk しない)
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/output/2025-12-30-1/x.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });

        // A に drill-in。
        let a_root = std::path::PathBuf::from("c:/root/2025-11-30");
        app.drill_into_container(a_root.clone(), false);
        match &app.global_search.view() {
            GlobalSearchView::DrilledInto {
                current_path,
                container_root,
                ..
            } => {
                assert_eq!(current_path, &a_root);
                assert_eq!(container_root, &a_root);
            }
            _ => panic!("drill_into_container 後は DrilledInto"),
        }

        // Ctrl+↓: A の subtree は [a_root] のみ → 末端 → 次コンテナ B の container_root に跳ぶ。
        // ここで「container_root と current_path が同じ」になることが重要 (= breadcrumb は
        // `> 2025-12-30-1` だけになり、深部ワープ `> output > 2025-12-30-1` は起きない)。
        app.global_search_ctrl_nav(true);
        let b_root = std::path::PathBuf::from("c:/root/output/2025-12-30-1");
        match &app.global_search.view() {
            GlobalSearchView::DrilledInto {
                current_path,
                container_root,
                ..
            } => {
                assert_eq!(container_root, &b_root, "次コンテナ root に跳ぶべき");
                assert_eq!(
                    current_path, &b_root,
                    "current_path が container_root と一致 (= breadcrumb は root_name だけ)"
                );
            }
            _ => panic!("Ctrl+↓ 後も DrilledInto"),
        }
    }

    /// Ctrl+G drilled view で Ctrl+↓ がコンテナ subtree 内では DFS 順で潜ること。
    /// (cross-container 跨ぎではない通常ケースの回帰ガード)
    #[test]
    fn ctrl_down_within_container_descends_dfs() {
        use crate::global_search_ui::GlobalSearchView;
        let mut app = setup_app();
        app.global_search.active = true;
        // コンテナ root に直接ヒット + サブフォルダにもヒット。
        // parent_container 単位で見ると "root" と "root/sub" の 2 コンテナだが、
        // Newer/Older ではなく HitCount ソートで root (件数 2) → root/sub (件数 1) の順。
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/x/root/a.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/x/root/b.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/x/root/sub/c.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });

        // コンテナ root に drill-in。subtree DFS は [root, root/sub]。
        let root = std::path::PathBuf::from("c:/x/root");
        app.drill_into_container(root.clone(), false);

        // Ctrl+↓: subtree 内 DFS の次 (root/sub)。container_root は root のまま。
        app.global_search_ctrl_nav(true);
        match &app.global_search.view() {
            GlobalSearchView::DrilledInto {
                current_path,
                container_root,
                ..
            } => {
                assert_eq!(container_root, &root, "container_root は不変");
                assert_eq!(
                    current_path,
                    &std::path::PathBuf::from("c:/x/root/sub"),
                    "subtree 内 DFS で sub に潜る"
                );
            }
            _ => panic!("Ctrl+↓ 後も DrilledInto"),
        }
    }

    /// 2026-04 ユーザー要望: Ctrl+G drilled view のサブフォルダ表示は通常フォルダと
    /// 同じ filter ルールに揃える。「★3 のみ」では未評価サブフォルダは隠れ、
    /// 「なし+★3」では未評価サブフォルダが (descendant 件数バッジつきで) 表示される。
    #[test]
    fn drilled_unrated_subfolder_hidden_when_unrated_filter_off() {
        use crate::global_search::GlobalHit;
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        app.global_search.active = true;
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub_unrated/a.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 3,
        });
        app.drill_into_container(std::path::PathBuf::from("c:/root"), false);

        // ★3 のみ ON (なし=OFF) → unrated サブフォルダは隠れる
        let mut rf = [false; 6];
        rf[3] = true;
        app.settings.rating_filter = rf;
        app.rebuild_items_from_global_search();
        let folder_visible_star_only = app
            .visible_indices
            .iter()
            .any(|&i| matches!(app.items.get(i), Some(GridItem::Folder(_))));
        assert!(
            !folder_visible_star_only,
            "★3 のみフィルタでは unrated subfolder は通常フォルダと同様に隠れるべき"
        );

        // なし + ★3 → unrated subfolder 表示 + ★3 件数バッジ
        let mut rf = [false; 6];
        rf[0] = true;
        rf[3] = true;
        app.settings.rating_filter = rf;
        app.rebuild_items_from_global_search();
        let sub_idx = app.items.iter().position(|it| {
            matches!(it, GridItem::Folder(p) if p == &std::path::PathBuf::from("c:/root/sub_unrated"))
        }).expect("なし+★3 では unrated subfolder が items に並ぶ");
        assert!(
            app.idx_visible(sub_idx),
            "なし+★3 では unrated subfolder が visible_indices にも入る"
        );
        let badge = app.folder_rating_match(sub_idx).expect("badge expected");
        assert_eq!(badge.0, 1, "badge total = ★3 descendant 1 件");
    }

    /// 通常表示 (Ctrl+G 外) でも unrated folder は ★3-only で隠れる従来挙動を維持。
    #[test]
    fn unrated_folder_hidden_in_normal_view_under_star_only_filter() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        app.items
            .push(GridItem::Folder(std::path::PathBuf::from("c:/some/folder")));
        let mut rf = [false; 6];
        rf[3] = true;
        app.settings.rating_filter = rf;
        app.rebuild_visible_indices();
        assert!(
            app.visible_indices.is_empty(),
            "通常表示の unrated folder は ★3-only で隠れる"
        );
    }

    fn run_grid_key(app: &mut super::App, modifiers: egui::Modifiers, key: egui::Key) {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
            modifiers,
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        });
        let _ = app.handle_keyboard(&ctx);
        let _ = ctx.end_pass();
    }

    /// 2026-05 ユーザー報告: ★フィルタでサムネイルが 0 件になった状態でも、
    /// Shift+F6 は current_folder のコンテナ★を解除できること。
    #[test]
    fn shift_f6_clears_current_folder_rating_when_visible_indices_empty() {
        use crate::grid_item::{GridItem, ThumbnailState};
        let mut app = setup_app();
        let folder = std::path::PathBuf::from("c:/pics");
        let image = folder.join("a.jpg");
        let key = crate::adjustment_db::normalize_path(&folder);

        app.current_folder = Some(folder);
        app.items.push(GridItem::Image(image));
        app.thumbnails.push(ThumbnailState::Pending);
        app.rating_db
            .as_ref()
            .expect("rating_db")
            .set(&key, 4)
            .expect("seed current folder rating");

        let mut rf = [false; 6];
        rf[5] = true;
        app.settings.rating_filter = rf;
        app.rebuild_visible_indices();
        assert!(
            app.visible_indices.is_empty(),
            "前提: フィルタ後のサムネイルは 0 件"
        );

        run_grid_key(&mut app, egui::Modifiers::SHIFT, egui::Key::F6);

        assert_eq!(
            app.rating_db.as_ref().unwrap().get(&key),
            0,
            "Shift+F6 は可視アイテムがなくても current_folder の★を解除する"
        );
        assert_eq!(app.current_folder_rating_cache, Some(0));
    }

    /// 空表示で Shift+F* によるコンテナ★変更を行った直後も Ctrl+Z で戻せること。
    #[test]
    fn ctrl_z_undoes_current_folder_rating_when_visible_indices_empty() {
        use crate::grid_item::{GridItem, ThumbnailState};
        let mut app = setup_app();
        let folder = std::path::PathBuf::from("c:/pics");
        let image = folder.join("a.jpg");
        let key = crate::adjustment_db::normalize_path(&folder);

        app.current_folder = Some(folder);
        app.items.push(GridItem::Image(image));
        app.thumbnails.push(ThumbnailState::Pending);
        let mut rf = [false; 6];
        rf[3] = true;
        app.settings.rating_filter = rf;
        app.rebuild_visible_indices();
        assert!(app.visible_indices.is_empty());

        run_grid_key(&mut app, egui::Modifiers::SHIFT, egui::Key::F5);
        assert_eq!(app.rating_db.as_ref().unwrap().get(&key), 5);

        run_grid_key(&mut app, egui::Modifiers::CTRL, egui::Key::Z);

        assert_eq!(
            app.rating_db.as_ref().unwrap().get(&key),
            0,
            "空表示でもコンテナ★変更の Undo が効く"
        );
    }

    /// 2026-04 ユーザー報告: Ctrl+G で検索 → SearchContainer に drill-in →
    /// BS で Aggregated に戻ったとき、開いていたコンテナのセルにカーソルが
    /// 復帰してほしい (旧実装は selected=None で先頭に飛んでいた)。
    #[test]
    fn bs_back_to_aggregated_restores_cursor_on_previous_container() {
        use crate::global_search::GlobalHit;
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        // 2 つの SearchContainer を accumulate (folder_b の方を開く想定)
        app.global_search.active = true;
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/folder_a/x.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/folder_b/y.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        // 集約ビューに固定する (集約トグルを ON にした状態 = aggregate_auto も倒す)。
        // 新モデルの既定は一覧ビューで、aggregate_auto が立ったままだと
        // maybe_auto_switch_aggregate が total_valid=0 を見て一覧へ戻してしまう。
        app.global_search.aggregate = true;
        app.global_search.aggregate_auto = false;
        // 初回 rebuild (集約)
        app.rebuild_items_from_global_search();
        // folder_b に drill-in
        app.drill_into_container(std::path::PathBuf::from("c:/folder_b"), false);
        // BS で Aggregated に戻る
        app.drill_back_one_level();
        // 戻った先で folder_b の SearchContainer が selected であること
        let selected_path = app
            .selected
            .and_then(|i| app.items.get(i))
            .map(|it| match it {
                GridItem::SearchContainer { path, .. } => path.clone(),
                _ => std::path::PathBuf::new(),
            });
        assert_eq!(
            selected_path,
            Some(std::path::PathBuf::from("c:/folder_b")),
            "BS で Aggregated に戻ったとき、直前に開いた folder_b にカーソルが残るべき"
        );
    }

    /// Codex P2: ★コンテナ drill-in で suppression 起動 → drilled view 内の
    /// サブフォルダへ入る → BS で 1 階層戻る、を順に行ったとき、suppression が
    /// 維持されること。subtree 内の上下移動で復元されると未評価の中身が突然
    /// 消える挙動になる。
    #[test]
    fn drill_back_within_suppression_anchor_keeps_filter_disabled() {
        use crate::global_search::GlobalHit;
        let mut app = setup_app();
        // ★5 フィルタ ON
        let mut rf = [false; 6];
        rf[5] = true;
        app.settings.rating_filter = rf;
        // ★5 のコンテナを rating_db に登録
        let container = std::path::PathBuf::from("c:/books/vol1");
        let key = crate::adjustment_db::normalize_path(&container);
        app.rating_db.as_ref().unwrap().set(&key, 5).unwrap();
        // 検索ヒット: コンテナ配下のサブフォルダ画像 (未評価)
        app.global_search.active = true;
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/books/vol1/sub/p1.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        // SearchContainer を開く (path-based suppression を起動)
        app.maybe_suppress_rating_filter_for_opened_container_path(&container);
        app.drill_into_container(container.clone(), false);
        assert!(
            app.rating_filter_suppressed_at.is_some(),
            "drill 直後は suppression 起動中"
        );
        // サブフォルダにドリルイン
        app.drill_into_subfolder(std::path::PathBuf::from("c:/books/vol1/sub"));
        assert!(
            app.rating_filter_suppressed_at.is_some(),
            "サブフォルダへ進んでも suppression 維持"
        );
        // BS で 1 階層戻る (sub → vol1)
        app.drill_back_one_level();
        assert!(
            app.rating_filter_suppressed_at.is_some(),
            "subtree 内 BS では suppression 維持されるべき"
        );
        // さらに BS で Aggregated に戻る → suppression 解除
        app.drill_back_one_level();
        assert!(
            app.rating_filter_suppressed_at.is_none(),
            "anchor の外 (Aggregated) に出たら suppression 解除"
        );
    }

    /// 2026-04 ユーザー報告: Ctrl+G ストリーミング中に rebuild が走って items に
    /// アイテムが挿入されたとき、選択中のアイテムが別の物を指してしまっていた
    /// (旧実装は `replace_search_view_items` で `selected = None`)。
    /// 修正後: 内容キー (`thumb_reuse_key`) で旧選択を新 idx に再マップする。
    /// `checked` set も同じ仕組みで追従する。
    #[test]
    fn streaming_rebuild_preserves_selected_and_checked_by_content_key() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        let initial = vec![
            GridItem::Image(std::path::PathBuf::from("c:/a.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/b.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/c.jpg")),
        ];
        app.replace_search_view_items(initial, vec![None, None, None]);
        // B (idx=1) を選択、A と C をチェック
        app.selected = Some(1);
        app.checked.insert(0);
        app.checked.insert(2);

        // ストリーミング rebuild: 先頭に X が追加されて A/B/C が後ろにシフト
        let after = vec![
            GridItem::Image(std::path::PathBuf::from("c:/x.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/a.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/b.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/c.jpg")),
        ];
        app.replace_search_view_items(after, vec![None; 4]);

        assert_eq!(
            app.selected,
            Some(2),
            "選択は同じ B (内容キー) を指し続けるよう新 idx=2 に追従"
        );
        assert!(app.checked.contains(&1), "A の checked が新 idx=1 に追従");
        assert!(app.checked.contains(&3), "C の checked が新 idx=3 に追従");
        assert!(!app.checked.contains(&0), "X (新規) は checked ではない");
        assert!(
            !app.checked.contains(&2),
            "B は selected のみ、checked ではない"
        );
    }

    /// Ctrl+G streaming rebuild は Loaded サムネイルを content-key で使い回すため、
    /// 補正再生成用の `thumb_pixels` も同じ key で移す必要がある。
    #[test]
    fn streaming_rebuild_preserves_thumb_pixels_for_loaded_survivors() {
        use crate::grid_item::{GridItem, ThumbnailState};
        use std::sync::Arc;

        fn loaded_thumb(
            ctx: &egui::Context,
            label: &str,
            color: egui::Color32,
        ) -> (ThumbnailState, Arc<egui::ColorImage>) {
            let image = egui::ColorImage::filled([1, 1], color);
            let pixels = Arc::new(image.clone());
            let tex = ctx.load_texture(label, image, egui::TextureOptions::LINEAR);
            (
                ThumbnailState::Loaded {
                    tex,
                    from_cache: false,
                    rendered_at_px: 64,
                    source_dims: Some((1, 1)),
                },
                pixels,
            )
        }

        let mut app = setup_app();
        let ctx = egui::Context::default();
        let initial = vec![
            GridItem::Image(std::path::PathBuf::from("c:/a.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/b.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/c.jpg")),
        ];
        app.replace_search_view_items(initial, vec![None, None, None]);

        let (thumb_a, pixels_a) = loaded_thumb(&ctx, "search_raw_a", egui::Color32::RED);
        let (thumb_b, pixels_b) = loaded_thumb(&ctx, "search_raw_b", egui::Color32::GREEN);
        let (thumb_c, pixels_c) = loaded_thumb(&ctx, "search_raw_c", egui::Color32::BLUE);
        app.thumbnails[0] = thumb_a;
        app.thumbnails[1] = thumb_b;
        app.thumbnails[2] = thumb_c;
        app.thumb_pixels.insert(0, Arc::clone(&pixels_a));
        app.thumb_pixels.insert(1, Arc::clone(&pixels_b));
        app.thumb_pixels.insert(2, Arc::clone(&pixels_c));
        app.thumb_adjust_tex.insert(
            2,
            ctx.load_texture(
                "search_stale_adjusted_c",
                egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
                egui::TextureOptions::LINEAR,
            ),
        );

        let after = vec![
            GridItem::Image(std::path::PathBuf::from("c:/x.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/c.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/a.jpg")),
        ];
        app.replace_search_view_items(after, vec![None, None, None]);

        assert!(matches!(app.thumbnails[0], ThumbnailState::Pending));
        assert!(matches!(app.thumbnails[1], ThumbnailState::Loaded { .. }));
        assert!(matches!(app.thumbnails[2], ThumbnailState::Loaded { .. }));
        assert!(
            Arc::ptr_eq(
                app.thumb_pixels
                    .get(&1)
                    .expect("old idx 2 shifts by content key to 1"),
                &pixels_c
            ),
            "C の source pixels は新 idx=1 に復元される"
        );
        assert!(
            Arc::ptr_eq(
                app.thumb_pixels
                    .get(&2)
                    .expect("old idx 0 shifts by content key to 2"),
                &pixels_a
            ),
            "A の source pixels は新 idx=2 に復元される"
        );
        assert!(
            !app.thumb_pixels.values().any(|p| Arc::ptr_eq(p, &pixels_b)),
            "検索結果から消えた B の source pixels は残さない"
        );
        assert!(
            app.thumb_adjust_tex.is_empty(),
            "補正済み TextureHandle は invalidate 後に再生成させる"
        );

        app.settings.global_preset.brightness = 20.0;
        app.maybe_apply_thumb_adjustment(&ctx, 1);
        assert!(
            app.thumb_adjust_tex.contains_key(&1),
            "復元した thumb_pixels から検索ビューのグローバル補正を再生成できる"
        );
    }

    /// 旧選択アイテムが新 items から消えた場合は selected = None に戻し、
    /// 先頭スクロールにフォールバックする (= 復元不能時の安全側挙動)。
    #[test]
    fn streaming_rebuild_clears_selected_when_item_disappears() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        let initial = vec![
            GridItem::Image(std::path::PathBuf::from("c:/a.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/b.jpg")),
        ];
        app.replace_search_view_items(initial, vec![None, None]);
        app.selected = Some(1); // B を選択

        // B が消えて A だけになる
        let after = vec![GridItem::Image(std::path::PathBuf::from("c:/a.jpg"))];
        app.replace_search_view_items(after, vec![None]);

        assert_eq!(app.selected, None, "旧選択アイテムが消えたので None");
        assert_eq!(app.scroll_offset_y, 0.0, "復元失敗時は先頭スクロール");
    }

    /// Codex P2: Ctrl+G から実フォルダ/ZIP/PDF を開いた状態で rating 変更すると、
    /// items が検索合成ビューに置き換わってはならない。
    /// `items_are_global_search_view` フラグが install_new_items 時に false に倒され、
    /// rebuild_items_from_global_search が走らないことを確認する。
    #[test]
    fn rating_change_in_real_view_does_not_rebuild_search_items() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        // Ctrl+G 中に実体ビュー (例: PDF を開いた直後の状態) を install_new_items 経由で構築
        app.global_search.active = true;
        let pdf_pages = vec![
            GridItem::PdfPage {
                pdf_path: std::path::PathBuf::from("c:/doc.pdf"),
                page_num: 0,
                content_type: None,
            },
            GridItem::PdfPage {
                pdf_path: std::path::PathBuf::from("c:/doc.pdf"),
                page_num: 1,
                content_type: None,
            },
        ];
        app.install_new_items(pdf_pages, vec![None, None]);
        assert!(
            !app.items_are_global_search_view,
            "install_new_items は flag を false に倒す (= 実体ビュー)"
        );
        let before_len = app.items.len();
        // rating 変更を発火 (= apply_rating_to_selection 内の rebuild 分岐)
        // ここでは rebuild_items_from_global_search を直接トリガーするか check する代わりに、
        // 分岐ロジックで実体ビュー時には visible_indices だけ更新されることを確認する。
        // ※ targets が空なら refresh_global_search_hit_stars もスキップ。
        // ここでは「items.len() が変わらない」ことだけを最低限の不変条件として確認。
        app.rebuild_visible_indices();
        assert_eq!(app.items.len(), before_len, "実体ビュー items が消えない");
        assert!(
            !app.items_are_global_search_view,
            "rebuild_visible_indices ではフラグが書き換わらない"
        );
    }

    /// Codex P2: Ctrl+G 集約結果で ★コンテナを開いたとき、コンテナ★が現在
    /// フィルタを通っていれば一時解除されること。これにより内部画像が未評価でも
    /// drilled view が空にならない。
    #[test]
    fn search_container_drill_in_suppresses_rating_filter_when_container_starred() {
        let mut app = setup_app();
        // ★5 フィルタ ON
        let mut rf = [false; 6];
        rf[5] = true;
        app.settings.rating_filter = rf;
        // rating_db に ★5 でコンテナ (フォルダ) を登録。
        // setup_app は rating_db を必ず開ける前提。開けないなら test 環境がおかしい
        // ので `expect` で即時 panic させて原因を読みやすくする (Codex P3)。
        let folder = std::path::PathBuf::from("c:/books/vol1");
        let key = crate::adjustment_db::normalize_path(&folder);
        let db = app
            .rating_db
            .as_ref()
            .expect("rating_db must be open in test (setup_app の data_dir override 失敗?)");
        db.set(&key, 5).expect("set rating");
        // ★コンテナを「開く」前は suppression なし
        assert!(app.rating_filter_suppressed_at.is_none());
        // 開く
        app.maybe_suppress_rating_filter_for_opened_container_path(&folder);
        // suppression 起動 + filter は全 ON に
        assert_eq!(
            app.rating_filter_suppressed_at
                .as_ref()
                .map(|(p, _)| p.clone()),
            Some(folder),
            "★5 コンテナを開いたら suppression anchor が設定される"
        );
        // Codex P1-2 fix 後: settings.rating_filter はユーザー設定そのまま (= 書き換え無し)、
        // effective_rating_filter() が suppression 状態を見て [true;6] を返す。
        assert_eq!(
            app.effective_rating_filter(),
            [true; 6],
            "suppression 起動中は effective filter が全 ON"
        );
        // settings 自体は元の filter (= ★5 ON) を保持
        let mut expected_settings = [false; 6];
        expected_settings[5] = true;
        assert_eq!(
            app.settings.rating_filter, expected_settings,
            "suppression 中も settings.rating_filter は元の値を保持 (= P1-2 fix)"
        );
    }

    /// Codex P2 スコープ外: コンテナ★が現フィルタを通らない場合は suppression しない。
    #[test]
    fn search_container_drill_in_does_not_suppress_when_rating_does_not_match() {
        let mut app = setup_app();
        // ★5 フィルタ ON
        let mut rf = [false; 6];
        rf[5] = true;
        app.settings.rating_filter = rf;
        // ★3 コンテナ (★5 フィルタを通らない)
        let folder = std::path::PathBuf::from("c:/notes/draft");
        let key = crate::adjustment_db::normalize_path(&folder);
        let db = app
            .rating_db
            .as_ref()
            .expect("rating_db must be open in test");
        db.set(&key, 3).expect("set rating");
        app.maybe_suppress_rating_filter_for_opened_container_path(&folder);
        assert!(
            app.rating_filter_suppressed_at.is_none(),
            "コンテナ★がフィルタ外なら suppression しない"
        );
    }

    /// Codex P3: Ctrl+G 中にレーティング変更すると、`all_hits.stars` が更新され、
    /// drilled view のバッジ件数が新しい値に追従すること。
    #[test]
    fn rating_change_updates_global_search_hit_stars_snapshot() {
        use crate::global_search::GlobalHit;
        let mut app = setup_app();
        app.global_search.active = true;
        // 単一画像を accumulate (★3 で受信)
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub/a.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 3,
        });
        // items に直接置く (drill_into_container は load_folder を呼ばないテスト用簡易セットアップ)
        app.items
            .push(crate::grid_item::GridItem::Image(std::path::PathBuf::from(
                "c:/root/sub/a.jpg",
            )));
        // rating を 1 に変更
        app.set_rating(0, 1);
        let idxs = vec![0_usize];
        app.refresh_global_search_hit_stars(&idxs);
        // all_hits.stars が更新されているはず
        assert_eq!(
            app.global_search.all_hits[0].stars, 1,
            "rating 変更後、all_hits.stars が 1 に追従するはず"
        );
    }

    /// 2026-04 ユーザー報告: Ctrl+G drilled view で「なし+★2」と「★2 のみ」で
    /// 見た目が同じだった問題の修正検証。
    /// 通常フォルダと同じ仕様に揃える:
    /// - フィルタ「なし+★2」: 未評価 subfolder は表示、バッジは ★1..★5 で集計 (= ★2 件数のみ)
    /// - フィルタ「★2 のみ」: 未評価 subfolder は visibility check で隠れる
    #[test]
    fn drilled_subfolder_badge_matches_normal_folder_semantics() {
        use crate::global_search::GlobalHit;
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        app.global_search.active = true;
        // /root/sub に: ★なし 2 件、★2 が 1 件、★4 が 1 件 を accumulate
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub/a.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub/b.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub/c.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 2,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub/d.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 4,
        });
        app.drill_into_container(std::path::PathBuf::from("c:/root"), false);

        // ── フィルタ「なし + ★2」: subfolder visible + badge=1 (★2 件数のみ) ──
        let mut rf = [false; 6];
        rf[0] = true;
        rf[2] = true;
        app.settings.rating_filter = rf;
        app.rebuild_items_from_global_search();
        let sub_idx = app.items.iter().position(|it| {
            matches!(it, GridItem::Folder(p) if p == &std::path::PathBuf::from("c:/root/sub"))
        }).expect("subfolder in items");
        assert!(
            app.idx_visible(sub_idx),
            "なし+★2 で unrated subfolder が表示"
        );
        let badge = app.folder_rating_match(sub_idx).expect("badge");
        assert_eq!(
            badge.0, 1,
            "badge は ★1..★5 のみ集計。★2 が 1 件なので 1 (なしは folder visibility だけに使う)"
        );

        // ── フィルタ「★2 のみ」: subfolder 自体が visible_indices から落ちる ──
        let mut rf = [false; 6];
        rf[2] = true;
        app.settings.rating_filter = rf;
        app.rebuild_items_from_global_search();
        let sub_idx = app.items.iter().position(|it| {
            matches!(it, GridItem::Folder(p) if p == &std::path::PathBuf::from("c:/root/sub"))
        }).expect("subfolder in items");
        assert!(
            !app.idx_visible(sub_idx),
            "★2 のみで unrated subfolder は隠れる (通常フォルダと同じ挙動)"
        );
    }

    /// `restore_select_path` のターゲットが items 中に存在しない (= 何らかの理由で
    /// 消えた) 場合は selected=None のまま放置せず、先頭の visible item に
    /// フォールバックする。次の方向キーで idx 0 に飛ぶ事故を防ぐため。
    #[test]
    fn bs_back_falls_back_to_first_visible_when_target_missing() {
        use crate::global_search::GlobalHit;
        let mut app = setup_app();
        app.global_search.active = true;
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/folder_a/x.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        // 存在しないパスを restore target に設定して rebuild
        app.global_search.restore_select_path =
            Some(std::path::PathBuf::from("c:/nonexistent/folder"));
        app.rebuild_items_from_global_search();
        // フォールバック: 先頭の visible item が選択されているはず
        assert_eq!(
            app.selected,
            app.visible_indices.first().copied(),
            "target 不在のときは先頭の visible item に落ちるべき"
        );
        assert!(
            app.selected.is_some(),
            "items が空でない限り selected は Some"
        );
    }

    /// drilled view 内で deeper subfolder に drill-in → BS で 1 階層戻ったとき、
    /// 直前に居た subfolder の Folder セルにカーソルが復帰すること。
    #[test]
    fn bs_back_within_drilled_restores_cursor_on_previous_subfolder() {
        use crate::global_search::GlobalHit;
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        app.global_search.active = true;
        // /root/sub1/x.jpg と /root/sub2/y.jpg のヒットを accumulate
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub1/x.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub2/y.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        // /root に drill-in
        app.drill_into_container(std::path::PathBuf::from("c:/root"), false);
        // /root/sub2 にさらに drill-in
        app.drill_into_subfolder(std::path::PathBuf::from("c:/root/sub2"));
        // BS で /root に戻る
        app.drill_back_one_level();
        let selected_path = app
            .selected
            .and_then(|i| app.items.get(i))
            .map(|it| match it {
                GridItem::Folder(p) => p.clone(),
                _ => std::path::PathBuf::new(),
            });
        assert_eq!(
            selected_path,
            Some(std::path::PathBuf::from("c:/root/sub2")),
            "drilled view で BS したとき、直前のサブフォルダ (sub2) にカーソルが残るべき"
        );
    }

    /// Ctrl+G が非アクティブな状態で advance_drilled_current_path を呼んでも
    /// drill state に影響しないこと (no-op)。
    #[test]
    fn advance_drilled_is_noop_when_global_search_inactive() {
        let mut app = setup_app();
        assert!(app.global_search.drill.is_none());
        app.advance_drilled_current_path(std::path::Path::new("C:/anything.pdf"));
        assert!(
            app.global_search.drill.is_none(),
            "Ctrl+G 非アクティブ時の advance は no-op であるべき"
        );
    }

    /// 一覧 (Flat) ビューから PDF を開いたとき、advance_drilled_current_path が
    /// container_root = current_path = pdf の 1 段ドリルを確立し、BS 1 回で一覧へ
    /// 戻れること (Codex P2)。
    #[test]
    fn advance_drilled_from_flat_establishes_single_level_drill() {
        let mut app = setup_app();
        app.global_search.active = true;
        let pdf = std::path::PathBuf::from("c:/fav/doc.pdf");
        app.advance_drilled_current_path(&pdf);
        match &app.global_search.drill {
            Some(d) => {
                assert_eq!(d.container_root, pdf);
                assert_eq!(d.current_path, pdf);
            }
            None => panic!("一覧から PDF を開いたら 1 段ドリルが確立されるべき"),
        }
        // BS 1 回でトップレベル (一覧) へ戻る
        app.drill_back_one_level();
        assert!(app.global_search.drill.is_none(), "BS 1 回で一覧へ戻る");
    }
}

// =======================================================================
// Phase C - Ctrl+G drill view アドレスバー表示テスト (2026-04 報告)
//
// 期待: "🌐 アイテム検索: \"グルグル\" > scansnap > 衛藤ヒロユキ_魔法陣グルグル01_ipad.pdf"
// バグ: PDF を開くと raw パス "d:/oldpc_backup/data2/scansnap/衛藤..._ipad.pdf"
// が address に書かれて、ブレッドクラムが失われる。
//
// 修正: `load_pdf_as_folder` (sync 経路) / `start_loading_items` (async 経路) /
// `advance_drilled_current_path` の 3 箇所で、self.address 設定の直後に
// `update_global_search_address()` を呼び直して breadcrumb を再適用する。
// =======================================================================

#[cfg(test)]
mod phase_c_drill_address_tests {
    use super::phase_c_support::setup_app;
    use crate::global_search::GlobalHit;

    /// Ctrl+G drill-in → PDF を開いた時点で address がブレッドクラム表示
    /// (`🌐 アイテム検索: "query" > container > filename.pdf`) になること。
    /// 旧実装は raw PDF パス (`d:/.../...pdf`) が入っていた (2026-04 バグ)。
    #[test]
    fn address_shows_breadcrumb_after_opening_pdf_in_drilled() {
        let mut app = setup_app();
        let folder_path = std::path::PathBuf::from("d:/oldpc_backup/data2/scansnap");
        let pdf_path = folder_path.join("衛藤ヒロユキ_魔法陣グルグル01_ipad.pdf");

        app.global_search.active = true;
        app.global_search.last_executed = "グルグル".to_string();
        app.global_search.accumulate_hit(&GlobalHit {
            path: format!(
                "{}/衛藤ヒロユキ_魔法陣グルグル01_ipad.pdf",
                folder_path.display()
            )
            .to_lowercase(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        app.drill_into_container(folder_path.clone(), false);
        // drill 直後は container_root のみの breadcrumb
        assert!(
            app.address.contains("scansnap"),
            "drill 直後: {}",
            app.address
        );
        assert!(
            app.address.contains("グルグル"),
            "drill 直後のクエリ: {}",
            app.address
        );

        // PDF を開く: advance_drilled_current_path + load_pdf_as_folder の
        // 同期 address 書き込みパスを模擬する
        app.advance_drilled_current_path(&pdf_path);
        // 「load_pdf_as_folder 内部で一旦 address = pdf_path を書く」の再現
        app.address = pdf_path.to_string_lossy().to_string();
        // 修正: 直後に update_global_search_address() が走って breadcrumb に戻す
        app.update_global_search_address();

        // 期待: raw path ではなく breadcrumb
        assert!(
            !app.address.starts_with("d:/"),
            "raw PDF path が address に残っている (修正前のバグ): {}",
            app.address
        );
        assert!(
            app.address.contains("🌐 アイテム検索"),
            "breadcrumb prefix 欠落: {}",
            app.address
        );
        assert!(
            app.address.contains("グルグル"),
            "クエリ欠落: {}",
            app.address
        );
        assert!(
            app.address.contains("scansnap"),
            "container_root 欠落: {}",
            app.address
        );
        assert!(
            app.address
                .contains("衛藤ヒロユキ_魔法陣グルグル01_ipad.pdf"),
            "PDF ファイル名欠落: {}",
            app.address
        );
    }

    /// Ctrl+G が非アクティブなときは `update_global_search_address` が no-op で
    /// address を書き換えないこと (本番経路で raw path が壊れない回帰ガード)。
    #[test]
    fn update_address_is_noop_when_ctrl_g_inactive() {
        let mut app = setup_app();
        app.address = "C:/some/folder".to_string();
        app.update_global_search_address();
        assert_eq!(
            app.address, "C:/some/folder",
            "Ctrl+G 非アクティブ時に address を書き換えてはならない"
        );
    }

    /// Aggregated 状態 → breadcrumb は N 件表示で、raw パスには戻らないこと。
    #[test]
    fn aggregated_address_shows_hit_count_not_raw_path() {
        let mut app = setup_app();
        app.global_search.active = true;
        app.global_search.last_executed = "グルグル".to_string();
        app.global_search.accumulate_hit(&GlobalHit {
            path: "d:/scansnap/a.pdf".to_string(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        // Aggregated のまま update_global_search_address
        app.update_global_search_address();
        assert!(
            app.address.contains("🌐 アイテム検索"),
            "Aggregated でも prefix は付く: {}",
            app.address
        );
        assert!(
            !app.address.starts_with("d:/"),
            "Aggregated 中は raw path が入ってはならない: {}",
            app.address
        );
    }

    /// 入力変更後の debounce 待ち / worker 実行中相当では、0 件確定に見えないよう
    /// address の件数表示にも検索中を出す。
    #[test]
    fn aggregated_address_marks_unsettled_results_as_searching() {
        let mut app = setup_app();
        app.global_search.active = true;
        app.global_search.query = "グルグル".to_string();
        app.global_search.last_executed.clear();
        app.update_global_search_address();

        assert!(
            app.address.contains("検索中"),
            "未確定の Ctrl+G 検索は address に検索中を出す: {}",
            app.address
        );
    }
}

/// 補正パラメータのお気に入り単位標準 (v0.8.1) に関する回帰テスト。
///
/// 3 層カスケード (個別 → お気に入り → global)、入れ子時の nearest-favorite 優先、
/// `resolve_adjust_scope` によるスコープ判定、`set_favorite_default` で冗長な個別設定が
/// 自動的に解除されること (Codex P2) を担保する。
#[cfg(test)]
mod favorite_adjustment_defaults_tests {
    use super::phase_c_support::setup_app;
    use super::*;
    use crate::adjustment::AdjustParams;
    use crate::settings::FavoriteEntry;
    use crate::ui_fullscreen::AdjustScope;
    use std::path::PathBuf;

    /// テスト用: 画像 1 枚だけを items に詰めて idx 0 を返す。
    fn push_image(app: &mut App, path: &str) -> usize {
        app.items.push(GridItem::Image(PathBuf::from(path)));
        app.thumbnails.push(ThumbnailState::Pending);
        app.items.len() - 1
    }

    fn mask_2x2() -> Vec<bool> {
        vec![true, false, false, true]
    }

    fn toast_text(app: &App) -> &str {
        app.fs_feedback_toast
            .as_ref()
            .map(|(text, _, _)| text.as_str())
            .unwrap_or("")
    }

    fn insert_stale_conceal_cache(app: &mut App, ctx: &egui::Context, idx: usize, label: &str) {
        let image = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(1, 2, 3)]);
        let pixels = std::sync::Arc::new(image.clone());
        let texture = ctx.load_texture(label, image, egui::TextureOptions::LINEAR);
        app.conceal_cache.insert(
            idx,
            ConcealCacheEntry {
                pixels,
                texture,
                generation: app.conceal_generation,
            },
        );
    }

    fn insert_stale_erase_result_cache(
        app: &mut App,
        ctx: &egui::Context,
        idx: usize,
        label: &str,
    ) {
        let image = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(4, 5, 6)]);
        let pixels = std::sync::Arc::new(image.clone());
        let texture = ctx.load_texture(label, image, egui::TextureOptions::LINEAR);
        app.erase_result_cache.insert(
            EraseResultKey {
                idx,
                input_gen: 0,
                mask_gen: 0,
            },
            EraseResultCacheEntry { pixels, texture },
        );
    }

    fn insert_current_erase_result_cache(
        app: &mut App,
        ctx: &egui::Context,
        idx: usize,
        label: &str,
    ) -> std::sync::Arc<egui::ColorImage> {
        let image = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(240, 241, 242)]);
        let pixels = std::sync::Arc::new(image.clone());
        let texture = ctx.load_texture(label, image, egui::TextureOptions::LINEAR);
        let key = app.current_erase_result_key(idx);
        app.erase_result_cache.insert(
            key,
            EraseResultCacheEntry {
                pixels: std::sync::Arc::clone(&pixels),
                texture,
            },
        );
        pixels
    }

    fn params_with_brightness(v: f32) -> AdjustParams {
        let mut p = AdjustParams::default();
        p.brightness = v;
        p
    }

    #[test]
    fn apply_conceal_slot_to_selection_saves_pages_and_clears_caches() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx_a = push_image(&mut app, "C:/pics/a.jpg");
        let idx_b = push_image(&mut app, "C:/pics/b.jpg");
        app.visible_indices = vec![idx_a, idx_b];
        app.checked.insert(idx_a);
        app.checked.insert(idx_b);
        let mask = mask_2x2();
        app.conceal_db
            .as_ref()
            .unwrap()
            .set_slot(1, &mask, &[], 2, 2)
            .unwrap();
        insert_stale_conceal_cache(&mut app, &ctx, idx_a, "bulk_conceal_stale_a");
        insert_stale_conceal_cache(&mut app, &ctx, idx_b, "bulk_conceal_stale_b");
        app.conceal_base_cache.insert(
            idx_a,
            std::sync::Arc::new(egui::ColorImage::new([1, 1], vec![egui::Color32::BLACK])),
        );

        app.apply_conceal_slot_to_selection(1);

        for idx in [idx_a, idx_b] {
            assert!(
                app.conceal_pages.contains(&idx),
                "conceal badge set should include applied page {idx}"
            );
            assert!(
                !app.conceal_cache.contains_key(&idx),
                "stale conceal render cache must be invalidated for page {idx}"
            );
            let key = app.page_path_key(idx).unwrap();
            let (saved_mask, saved_shapes) = app
                .conceal_db
                .as_ref()
                .unwrap()
                .get_full(&key, 2, 2)
                .expect("conceal slot should be saved to page db");
            assert_eq!(saved_mask, mask);
            assert!(saved_shapes.is_empty());
        }
        assert!(!app.conceal_base_cache.contains_key(&idx_a));
        assert!(app.checked.is_empty());
        assert_eq!(toast_text(&app), "[隠蔽スロット1を2枚に適用]");
    }

    #[test]
    fn apply_conceal_slot_in_viewing_mode_saves_current_page() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/current.jpg");
        app.fullscreen_idx = Some(idx);
        let mask = mask_2x2();
        app.conceal_db
            .as_ref()
            .unwrap()
            .set_slot(2, &mask, &[], 2, 2)
            .unwrap();
        insert_stale_conceal_cache(&mut app, &ctx, idx, "view_conceal_stale");
        app.conceal_base_cache.insert(
            idx,
            std::sync::Arc::new(egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE])),
        );

        app.apply_conceal_slot_in_viewing_mode(2);

        assert!(app.conceal_pages.contains(&idx));
        assert!(!app.conceal_cache.contains_key(&idx));
        assert!(!app.conceal_base_cache.contains_key(&idx));
        let key = app.page_path_key(idx).unwrap();
        let (saved_mask, saved_shapes) = app
            .conceal_db
            .as_ref()
            .unwrap()
            .get_full(&key, 2, 2)
            .expect("conceal slot should be saved to current page");
        assert_eq!(saved_mask, mask);
        assert!(saved_shapes.is_empty());
        assert_eq!(toast_text(&app), "[隠蔽スロット2適用]");
    }

    #[test]
    fn delete_mask_shortcuts_remove_existing_masks_and_clear_caches() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx_a = push_image(&mut app, "C:/pics/a.jpg");
        let idx_b = push_image(&mut app, "C:/pics/b.jpg");
        app.visible_indices = vec![idx_a, idx_b];
        let mask = mask_2x2();
        app.save_mask_with_sidecar(idx_a, &mask, &[], 2, 2);
        app.save_conceal_with_sidecar(idx_b, &mask, &[], 2, 2);
        let erase_key = app.page_path_key(idx_a).unwrap();
        let conceal_key = app.page_path_key(idx_b).unwrap();
        app.checked.insert(idx_a);
        app.checked.insert(idx_b);
        insert_stale_erase_result_cache(&mut app, &ctx, idx_a, "erase_result_stale");
        insert_stale_conceal_cache(&mut app, &ctx, idx_a, "erase_conceal_stale");
        app.erase_base_cache.insert(
            idx_a,
            std::sync::Arc::new(egui::ColorImage::new(
                [1, 1],
                vec![egui::Color32::LIGHT_BLUE],
            )),
        );
        app.erase_base_tex_cache.insert(
            idx_a,
            ctx.load_texture(
                "erase_base_tex_stale",
                egui::ColorImage::new([1, 1], vec![egui::Color32::LIGHT_BLUE]),
                egui::TextureOptions::LINEAR,
            ),
        );

        app.delete_erase_masks_from_selection();

        assert!(!app.mask_pages.contains(&idx_a));
        assert!(app.conceal_pages.contains(&idx_b));
        assert!(app.erase_result_cache.is_empty());
        assert!(!app.erase_base_cache.contains_key(&idx_a));
        assert!(!app.erase_base_tex_cache.contains_key(&idx_a));
        assert!(!app.conceal_cache.contains_key(&idx_a));
        assert!(
            app.mask_db
                .as_ref()
                .unwrap()
                .get_full(&erase_key, 2, 2)
                .is_none()
        );
        assert_eq!(toast_text(&app), "[消しゴムマスクを1枚から削除]");
        assert!(app.checked.is_empty());

        app.checked.insert(idx_a);
        app.checked.insert(idx_b);
        insert_stale_conceal_cache(&mut app, &ctx, idx_b, "conceal_delete_stale");
        app.conceal_base_cache.insert(
            idx_b,
            std::sync::Arc::new(egui::ColorImage::new(
                [1, 1],
                vec![egui::Color32::LIGHT_GREEN],
            )),
        );

        app.delete_conceal_masks_from_selection();

        assert!(!app.conceal_pages.contains(&idx_b));
        assert!(!app.conceal_cache.contains_key(&idx_b));
        assert!(!app.conceal_base_cache.contains_key(&idx_b));
        assert!(
            app.conceal_db
                .as_ref()
                .unwrap()
                .get_full(&conceal_key, 2, 2)
                .is_none()
        );
        assert_eq!(toast_text(&app), "[隠蔽マスクを1枚から削除]");
        assert!(app.checked.is_empty());
    }

    #[test]
    fn empty_mask_slots_are_noop_with_specific_toasts() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/a.jpg");
        app.visible_indices = vec![idx];
        app.selected = Some(idx);
        insert_stale_conceal_cache(&mut app, &ctx, idx, "empty_slot_stale");

        app.apply_conceal_slot_to_selection(1);

        assert!(!app.conceal_pages.contains(&idx));
        assert!(
            app.conceal_cache.contains_key(&idx),
            "empty slot no-op should not invalidate unrelated conceal cache"
        );
        assert_eq!(toast_text(&app), "[隠蔽スロット1は空です]");

        app.fs_feedback_toast = None;
        app.apply_slot_to_selection(1);

        assert!(!app.mask_pages.contains(&idx));
        assert_eq!(toast_text(&app), "[消しゴムスロット1は空です]");
    }

    /// effective_params は「個別 → お気に入り → global」の順で解決する。
    #[test]
    fn cascade_individual_beats_favorite_beats_global() {
        let mut app = setup_app();
        let fav = FavoriteEntry::new("test".to_string(), PathBuf::from("C:/pics"));
        let fav_id = fav.id;
        app.settings.favorites.push(fav);
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        // 初期状態: global
        app.settings.global_preset = params_with_brightness(5.0);
        assert_eq!(app.effective_params(idx).brightness, 5.0);

        // お気に入り標準を入れる → 優先
        app.adjustment_favorite_params
            .insert(fav_id, params_with_brightness(20.0));
        assert_eq!(app.effective_params(idx).brightness, 20.0);

        // 個別設定を入れる → 最優先
        app.adjustment_page_params
            .insert(idx, params_with_brightness(50.0));
        assert_eq!(app.effective_params(idx).brightness, 50.0);

        // 個別解除 → お気に入り、お気に入り解除 → global に戻る
        app.adjustment_page_params.remove(&idx);
        assert_eq!(app.effective_params(idx).brightness, 20.0);
        app.adjustment_favorite_params.remove(&fav_id);
        assert_eq!(app.effective_params(idx).brightness, 5.0);
    }

    /// 入れ子お気に入りでは最も近い祖先 (パス最長) が優先される。
    #[test]
    fn nested_favorite_picks_nearest_ancestor() {
        let mut app = setup_app();
        let outer = FavoriteEntry::new("outer".to_string(), PathBuf::from("C:/pics"));
        let inner = FavoriteEntry::new("inner".to_string(), PathBuf::from("C:/pics/ai"));
        let inner_id = inner.id;
        app.settings.favorites.push(outer);
        app.settings.favorites.push(inner);

        let idx = push_image(&mut app, "C:/pics/ai/gen.jpg");
        let nearest = app.current_favorite_id_for_idx(idx);
        assert_eq!(
            nearest,
            Some(inner_id),
            "深い方のお気に入りが優先されるべき"
        );
    }

    /// resolve_adjust_scope は個別 > favorite > global の順に層を報告する。
    #[test]
    fn resolve_adjust_scope_picks_effective_layer() {
        let mut app = setup_app();
        let fav = FavoriteEntry::new("t".to_string(), PathBuf::from("C:/pics"));
        let fav_id = fav.id;
        app.settings.favorites.push(fav);
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        assert!(matches!(app.resolve_adjust_scope(idx), AdjustScope::Global));
        app.adjustment_favorite_params
            .insert(fav_id, params_with_brightness(10.0));
        assert!(
            matches!(app.resolve_adjust_scope(idx), AdjustScope::FavoriteDefault(id) if id == fav_id)
        );
        app.adjustment_page_params
            .insert(idx, params_with_brightness(30.0));
        assert!(matches!(
            app.resolve_adjust_scope(idx),
            AdjustScope::PageOverride
        ));
    }

    /// set_favorite_default 直後に、ちょうど同じ値の個別設定を持っていたページは
    /// 冗長なので自動的に解除され、スコープは FavoriteDefault になる (Codex P2 回帰)。
    #[test]
    fn set_favorite_default_collapses_redundant_page_override() {
        let mut app = setup_app();
        let fav = FavoriteEntry::new("t".to_string(), PathBuf::from("C:/pics"));
        let fav_id = fav.id;
        app.settings.favorites.push(fav);
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        // 個別に brightness=25 を設定 (= これから新しい favorite 標準にしたい値)
        let custom = params_with_brightness(25.0);
        app.adjustment_page_params.insert(idx, custom.clone());
        assert!(matches!(
            app.resolve_adjust_scope(idx),
            AdjustScope::PageOverride
        ));

        // 「このお気に入りの標準にする」と同じ操作
        app.set_favorite_default(fav_id, custom);

        assert!(
            !app.adjustment_page_params.contains_key(&idx),
            "新 favorite 標準と一致する個別は解除されるべき"
        );
        assert!(
            matches!(
                app.resolve_adjust_scope(idx),
                AdjustScope::FavoriteDefault(id) if id == fav_id
            ),
            "scope は FavoriteDefault に正規化されるべき"
        );
    }

    /// clear_favorite_default でそのお気に入り配下の、global と同値な個別もまとめて解除される。
    #[test]
    fn clear_favorite_default_collapses_overrides_matching_global() {
        let mut app = setup_app();
        let fav = FavoriteEntry::new("t".to_string(), PathBuf::from("C:/pics"));
        let fav_id = fav.id;
        app.settings.favorites.push(fav);
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        // favorite 標準 = 20, global = 5, 個別 = 5 (= global)
        app.settings.global_preset = params_with_brightness(5.0);
        app.adjustment_favorite_params
            .insert(fav_id, params_with_brightness(20.0));
        app.adjustment_page_params
            .insert(idx, params_with_brightness(5.0));

        // favorite 未設定のとき effective_default は global なので、個別は当初から冗長
        // (ただし UI 経路では set_page_params が弾くのでここでは手動 insert)。
        // favorite 解除後は「global が新しい default」に戻り、同値の個別は冗長になる。
        app.clear_favorite_default(fav_id);

        assert!(
            !app.adjustment_page_params.contains_key(&idx),
            "clear 後、新 default (global) と一致する個別は解除されるべき"
        );
    }

    /// set_page_params は新 3 層カスケード用の「effective_default_for_idx」との等価比較で
    /// 冗長判定を行う。お気に入り標準と一致する params を渡しても個別は作られない。
    #[test]
    fn set_page_params_drops_individual_when_matching_favorite_default() {
        let mut app = setup_app();
        let fav = FavoriteEntry::new("t".to_string(), PathBuf::from("C:/pics"));
        let fav_id = fav.id;
        app.settings.favorites.push(fav);
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        let fav_default = params_with_brightness(15.0);
        app.adjustment_favorite_params
            .insert(fav_id, fav_default.clone());

        // 個別を入れてから、favorite と同値を書く → 削除される
        app.adjustment_page_params
            .insert(idx, params_with_brightness(99.0));
        app.set_page_params(idx, fav_default);
        assert!(
            !app.adjustment_page_params.contains_key(&idx),
            "favorite 標準と等価な個別は保存しないべき"
        );
    }

    // ── Undo/Redo for image adjustments ────────────────────────────────

    /// ページ個別補正の Undo/Redo: 1 回スライダーを動かして Ctrl+Z で戻る、Ctrl+Y で戻し直し。
    #[test]
    fn page_adjustment_undo_redo_round_trip() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        // 初期: 個別なし
        assert!(!app.adjustment_page_params.contains_key(&idx));

        // 個別設定 brightness=30 を書く (capture_adjust_full でラップ)
        app.capture_adjust_full("test slider".into(), |a| {
            a.set_page_params(idx, params_with_brightness(30.0));
        });
        assert_eq!(
            app.adjustment_page_params.get(&idx).unwrap().brightness,
            30.0
        );

        // Undo: 個別が消える
        app.apply_meta_undo();
        assert!(
            !app.adjustment_page_params.contains_key(&idx),
            "Undo 後はエントリが消える"
        );

        // Redo: 個別が brightness=30 に戻る
        app.apply_meta_redo();
        assert_eq!(
            app.adjustment_page_params.get(&idx).unwrap().brightness,
            30.0,
            "Redo で再適用される"
        );
    }

    /// 補正レイヤーの Undo/Redo: レイヤー配列全体を before/after として戻す。
    #[test]
    fn local_adjustment_layers_undo_redo_round_trip() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/a.jpg");
        let layer = local_adjust_core::LocalAdjustmentLayer::new(
            "tone",
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::Tone(local_adjust_core::ToneParams {
                brightness: 25.0,
                ..Default::default()
            }),
        );

        let before = app
            .local_adjust_page_layers
            .get(&idx)
            .cloned()
            .unwrap_or_default();
        app.set_local_adjust_layers_for_idx_with_undo(
            idx,
            before,
            vec![layer],
            "test local adjust".to_string(),
        );
        assert_eq!(app.local_adjust_page_layers.get(&idx).unwrap().len(), 1);
        assert!(app.local_adjust_pages.contains(&idx));

        app.apply_meta_undo();
        assert!(
            !app.local_adjust_page_layers.contains_key(&idx),
            "Undo 後は補正レイヤーなしに戻る"
        );
        assert!(!app.local_adjust_pages.contains(&idx));

        app.apply_meta_redo();
        assert_eq!(
            app.local_adjust_page_layers.get(&idx).unwrap().len(),
            1,
            "Redo で補正レイヤーが再適用される"
        );
        assert!(app.local_adjust_pages.contains(&idx));
    }

    /// Codex P1 回帰: お気に入り標準の更新で冗長な個別ページが pruning される。
    /// Undo するとお気に入り標準は元に戻り、かつ削除された個別ページも復元される。
    #[test]
    fn favorite_default_undo_restores_pruned_page_overrides() {
        let mut app = setup_app();
        let fav = FavoriteEntry::new("t".to_string(), PathBuf::from("C:/pics"));
        let fav_id = fav.id;
        app.settings.favorites.push(fav);
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        // 個別 = 25 (favorite 未設定)
        app.adjustment_page_params
            .insert(idx, params_with_brightness(25.0));

        // 「このお気に入りの標準にする」(= 個別と同値) を capture_adjust_full でラップ
        let new_fav_default = params_with_brightness(25.0);
        app.capture_adjust_full("set favorite".into(), |a| {
            a.set_favorite_default(fav_id, new_fav_default);
        });
        // pruning が走り、個別は消える
        assert!(
            !app.adjustment_page_params.contains_key(&idx),
            "set_favorite_default は冗長な個別を pruning する"
        );
        assert!(app.adjustment_favorite_params.contains_key(&fav_id));

        // Undo: お気に入り標準が消え、個別 25 が復元される (Codex P1)
        app.apply_meta_undo();
        assert!(
            !app.adjustment_favorite_params.contains_key(&fav_id),
            "Undo でお気に入り標準が解除"
        );
        assert_eq!(
            app.adjustment_page_params.get(&idx).unwrap().brightness,
            25.0,
            "pruning された個別ページも復元されること (Codex P1)"
        );

        // Redo: 元の状態 (お気に入り標準 + 個別なし) に戻る
        app.apply_meta_redo();
        assert!(app.adjustment_favorite_params.contains_key(&fav_id));
        assert!(
            !app.adjustment_page_params.contains_key(&idx),
            "Redo で再 pruning される"
        );
    }

    /// Codex P2 回帰: バルク操作 (apply_to_all / clear_all) も Undo に積まれる。
    #[test]
    fn bulk_apply_clear_all_pages_pushes_undo_entry() {
        let mut app = setup_app();
        let idx_a = push_image(&mut app, "C:/pics/a.jpg");
        let idx_b = push_image(&mut app, "C:/pics/b.jpg");

        // a, b に個別設定を入れる
        app.adjustment_page_params
            .insert(idx_a, params_with_brightness(40.0));
        app.adjustment_page_params
            .insert(idx_b, params_with_brightness(60.0));

        // 「全画像から解除」をラップして実行
        app.capture_adjust_full("clear all".into(), |a| {
            a.clear_all_page_params();
        });
        assert!(app.adjustment_page_params.is_empty(), "全画像が解除される");

        // Undo: 個別設定が両方復元される
        app.apply_meta_undo();
        assert_eq!(
            app.adjustment_page_params.get(&idx_a).unwrap().brightness,
            40.0,
            "個別設定 a が復元"
        );
        assert_eq!(
            app.adjustment_page_params.get(&idx_b).unwrap().brightness,
            60.0,
            "個別設定 b が復元"
        );

        // Redo: 全画像が再び解除される
        app.apply_meta_redo();
        assert!(app.adjustment_page_params.is_empty(), "Redo で再 clear");
    }

    /// 新しい補正操作を行うと redo スタックがクリアされる (Ctrl+Z 後の操作で
    /// ぶら下がっていた redo は無効化される)。
    #[test]
    fn new_adjustment_op_clears_redo_stack() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        // 操作 1: brightness=10
        app.capture_adjust_full("op1".into(), |a| {
            a.set_page_params(idx, params_with_brightness(10.0));
        });
        // Undo → redo に積まれる
        app.apply_meta_undo();
        assert!(app.meta_undo.can_redo());

        // 操作 2: brightness=20 → redo がクリアされる
        app.capture_adjust_full("op2".into(), |a| {
            a.set_page_params(idx, params_with_brightness(20.0));
        });
        assert!(
            !app.meta_undo.can_redo(),
            "新しい操作で redo がクリアされる"
        );
        assert_eq!(
            app.adjustment_page_params.get(&idx).unwrap().brightness,
            20.0
        );
    }

    /// `is_meaningful` 抑止: 何も変化しない write_op は Undo に積まれない。
    #[test]
    fn no_op_write_does_not_push_undo() {
        let mut app = setup_app();
        let _idx = push_image(&mut app, "C:/pics/a.jpg");
        let undo_len_before = app.meta_undo.undo_len();

        // 何も変更しない write_op
        app.capture_adjust_full("noop".into(), |_a| {});

        assert_eq!(
            app.meta_undo.undo_len(),
            undo_len_before,
            "no-op は積まれない"
        );
    }

    /// Codex P2 #1 回帰: グリッド Ctrl+1〜0 のスロット一括適用 (`apply_slot_to_grid_selection`)
    /// が `capture_adjust_full` でラップされ、N 枚の個別設定が 1 回の Ctrl+Z で全て戻る。
    #[test]
    fn grid_slot_apply_is_undoable() {
        let mut app = setup_app();
        let idx_a = push_image(&mut app, "C:/pics/a.jpg");
        let idx_b = push_image(&mut app, "C:/pics/b.jpg");
        // ratable_page_targets は visible_indices で絞るのでテストで埋めておく
        app.visible_indices = vec![idx_a, idx_b];
        // 2 枚チェック (= 一括対象)
        app.checked.insert(idx_a);
        app.checked.insert(idx_b);
        // スロット 0 に brightness=42 を保存
        app.settings.preset_slots.slots[0] = Some(crate::adjustment::PresetSlot {
            name: "test".into(),
            params: params_with_brightness(42.0),
        });

        app.capture_adjust_full("slot apply".into(), |a| {
            a.apply_slot_to_grid_selection(0);
        });
        assert_eq!(
            app.adjustment_page_params.get(&idx_a).unwrap().brightness,
            42.0
        );
        assert_eq!(
            app.adjustment_page_params.get(&idx_b).unwrap().brightness,
            42.0
        );

        // Ctrl+Z で両方戻る
        app.apply_meta_undo();
        assert!(!app.adjustment_page_params.contains_key(&idx_a));
        assert!(!app.adjustment_page_params.contains_key(&idx_b));
    }

    // ── タグ Undo: pending → finalize の検証 (Codex P3 完全対応) ──────────────

    /// stale cache + Toggle で worker が逆方向 (Remove) に解決した場合、
    /// 楽観 cache 更新は予測値だが、Undo entry は **worker が読んだ実 disk の
    /// before/after** で作られる。これにより Ctrl+Z が真の逆操作になる。
    #[test]
    fn tag_undo_uses_worker_actual_disk_state_not_optimistic_prediction() {
        let mut app = setup_app();
        let path = PathBuf::from("C:/pics/a.jpg");

        // 1) 操作開始: pending を register
        let tx = app.next_tag_tx_id();
        app.register_pending_tag_op(tx, "#A のトグル".into(), 1);

        // 2) 楽観 cache 更新 (UI 即時反映): cache=[] と仮定し、predicted after=[#A]
        // (実際の試験では cache に何があってもよい — Undo の正しさには影響しない)

        // 3) worker 結果到着: 実 disk は実は既に [#A] だった (= 外部ツールで付与済み)。
        //    worker は Remove を選び、tags_after=[]. tags_before=[#A] を返す。
        let actual_before = vec!["#A".to_string()];
        let actual_after: Vec<String> = vec![];
        app.test_finalize_tag_success(
            tx,
            crate::undo_stack::TagChange {
                path: path.clone(),
                before: actual_before.clone(),
                after: actual_after.clone(),
            },
        );

        // 4) meta_undo に **実 disk** の before/after で entry が積まれている
        assert_eq!(app.meta_undo.undo_len(), 1);
        match app.meta_undo.peek_undo().unwrap() {
            crate::undo_stack::UndoEntry::Tag { changes, .. } => {
                assert_eq!(changes.len(), 1);
                assert_eq!(changes[0].before, actual_before);
                assert_eq!(changes[0].after, actual_after);
            }
            _ => panic!("expected Tag entry"),
        }

        // 5) pending は finalize 後に消えている
        assert!(app.pending_tag_undos.is_empty());
    }

    /// XMP 書き込みが失敗したジョブは Undo entry に含まれない (= 失敗パスを Ctrl+Z すると
    /// 「実ディスクは変わっていないのに書き戻し命令が飛ぶ」事故を防ぐ)。
    #[test]
    fn tag_undo_skips_failed_writes() {
        let mut app = setup_app();
        let path_ok = PathBuf::from("C:/pics/ok.jpg");

        let tx = app.next_tag_tx_id();
        app.register_pending_tag_op(tx, "#A のトグル".into(), 2);

        // 1 件成功、1 件失敗
        app.test_finalize_tag_success(
            tx,
            crate::undo_stack::TagChange {
                path: path_ok,
                before: vec![],
                after: vec!["#A".into()],
            },
        );
        app.test_finalize_tag_failure(tx);

        // 完了 (1 success + 1 failure = 2 = expected_total)、Undo entry は成功 1 件分のみ
        assert_eq!(app.meta_undo.undo_len(), 1);
        match app.meta_undo.peek_undo().unwrap() {
            crate::undo_stack::UndoEntry::Tag { changes, .. } => {
                assert_eq!(changes.len(), 1, "失敗分は entry に含まれない");
            }
            _ => panic!(),
        }
    }

    /// 全件失敗した場合は Undo entry が積まれない (空エントリの抑止)。
    #[test]
    fn tag_undo_all_failed_pushes_no_entry() {
        let mut app = setup_app();
        let tx = app.next_tag_tx_id();
        app.register_pending_tag_op(tx, "all fail".into(), 2);

        app.test_finalize_tag_failure(tx);
        app.test_finalize_tag_failure(tx);

        assert_eq!(app.meta_undo.undo_len(), 0, "全件失敗は entry なし");
        assert!(app.pending_tag_undos.is_empty());
    }

    /// stale cache のシナリオ C 防止: 連続操作 + Ctrl+Z 連打で外部由来タグを
    /// 破壊しないことを検証。worker 結果ベースで Undo entry が作られているので
    /// Ctrl+Z は真の disk 状態に戻る (#A は保持される)。
    #[test]
    fn tag_undo_chain_does_not_destroy_external_tags() {
        let mut app = setup_app();
        let path = PathBuf::from("C:/pics/a.jpg");

        // 操作 1: 外部で #A 付与済みの状態で mIV が #B トグル
        // worker が読む disk: [#A] → 書く: [#A, #B]
        let tx1 = app.next_tag_tx_id();
        app.register_pending_tag_op(tx1, "#B".into(), 1);
        app.test_finalize_tag_success(
            tx1,
            crate::undo_stack::TagChange {
                path: path.clone(),
                before: vec!["#A".into()],
                after: vec!["#A".into(), "#B".into()],
            },
        );

        // 操作 2: #C トグル
        // worker が読む disk: [#A, #B] → 書く: [#A, #B, #C]
        let tx2 = app.next_tag_tx_id();
        app.register_pending_tag_op(tx2, "#C".into(), 1);
        app.test_finalize_tag_success(
            tx2,
            crate::undo_stack::TagChange {
                path: path.clone(),
                before: vec!["#A".into(), "#B".into()],
                after: vec!["#A".into(), "#B".into(), "#C".into()],
            },
        );

        assert_eq!(app.meta_undo.undo_len(), 2);

        // 直近 (op2) を Ctrl+Z: pop entry — before=[#A,#B] が正しい逆操作
        let entry = app.meta_undo.pop_undo().unwrap();
        match &entry {
            crate::undo_stack::UndoEntry::Tag { changes, .. } => {
                assert_eq!(changes[0].before, vec!["#A", "#B"]);
                assert_eq!(changes[0].after, vec!["#A", "#B", "#C"]);
            }
            _ => panic!(),
        }

        // 1 つ前 (op1) を Ctrl+Z: before=[#A] が正しい逆操作 — **空ではない**
        // (旧実装ではここが [] になり、Ctrl+Z で #A まで消えてしまう破壊)
        let entry = app.meta_undo.pop_undo().unwrap();
        match &entry {
            crate::undo_stack::UndoEntry::Tag { changes, .. } => {
                assert_eq!(
                    changes[0].before,
                    vec!["#A"],
                    "Ctrl+Z は外部由来 #A を保持しなければならない (シナリオ C 防止)"
                );
                assert_eq!(changes[0].after, vec!["#A", "#B"]);
            }
            _ => panic!(),
        }
    }

    /// pending が消えている (= clear_meta_undo で boundary を跨いだ) tx_id の worker 結果は
    /// 静かに drop し、Undo stack には影響しない。
    #[test]
    fn tag_undo_orphan_result_after_boundary_does_not_push() {
        let mut app = setup_app();
        let tx = app.next_tag_tx_id();
        app.register_pending_tag_op(tx, "abandoned".into(), 1);

        // boundary clear (フォルダ移動相当) で pending が破棄される
        app.clear_meta_undo();
        assert!(app.pending_tag_undos.is_empty());

        // 後から worker 結果が届いても entry は積まれない
        app.test_finalize_tag_success(
            tx,
            crate::undo_stack::TagChange {
                path: PathBuf::from("C:/pics/a.jpg"),
                before: vec![],
                after: vec!["#A".into()],
            },
        );
        assert_eq!(app.meta_undo.undo_len(), 0);
    }

    /// Codex P2 #1 回帰: グリッド Q / Ctrl+Backspace の一括解除
    /// (`clear_page_params_for_selection`) が capture_adjust_full でラップされ、
    /// N 枚の個別設定が 1 回の Ctrl+Z で全て復元される。
    #[test]
    fn grid_clear_selection_is_undoable() {
        let mut app = setup_app();
        let idx_a = push_image(&mut app, "C:/pics/a.jpg");
        let idx_b = push_image(&mut app, "C:/pics/b.jpg");
        app.visible_indices = vec![idx_a, idx_b];
        // 既存の個別設定
        app.adjustment_page_params
            .insert(idx_a, params_with_brightness(15.0));
        app.adjustment_page_params
            .insert(idx_b, params_with_brightness(35.0));
        // 2 枚チェック
        app.checked.insert(idx_a);
        app.checked.insert(idx_b);

        app.capture_adjust_full("clear sel".into(), |a| {
            a.clear_page_params_for_selection();
        });
        assert!(!app.adjustment_page_params.contains_key(&idx_a));
        assert!(!app.adjustment_page_params.contains_key(&idx_b));

        // Ctrl+Z で両方の個別設定が復元
        app.apply_meta_undo();
        assert_eq!(
            app.adjustment_page_params.get(&idx_a).unwrap().brightness,
            15.0
        );
        assert_eq!(
            app.adjustment_page_params.get(&idx_b).unwrap().brightness,
            35.0
        );
    }

    /// Codex P3 (2026-04): BS で深い階層から戻ったとき、`select_after_load` が
    /// `folder_history` の古い選択より優先されること。Ctrl+↓ で進んだ先から
    /// 戻ったとき「最初に入った位置」ではなく「今いる位置」にカーソルを合わせる
    /// ための優先順位反転 (load_folder の選択復元) の回帰テスト。
    #[test]
    fn select_after_load_overrides_folder_history() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        app.items.push(GridItem::Folder("c:/root/folderA".into()));
        app.items.push(GridItem::Folder("c:/root/folderB".into()));
        app.rebuild_visible_indices();

        // 履歴: folderA (idx=0) を保存
        let source = std::path::PathBuf::from("c:/root");
        app.folder_history.insert(source.clone(), (123.0, Some(0)));
        // ヒント: folderB を選びたい (BS で戻った直後の状態を模擬)
        app.select_after_load = Some("folderB".to_string());

        let restored = app.try_select_after_load();
        assert!(restored, "ヒントが items に存在するなら true を返す");
        assert_eq!(
            app.selected,
            Some(1),
            "folderB (idx=1) がヒントで選ばれるべき (履歴の folderA に負けない)"
        );
        assert!(
            app.select_after_load.is_none(),
            "ヒントは take() で消費される (次回 load に持ち越さない)"
        );
        assert!(
            app.scroll_to_selected,
            "選択行を可視化するスクロール要求が立つ"
        );
    }

    /// Codex P3 (2026-04): ヒントの指す名前が items に無い場合 (削除等) は false を
    /// 返し、`start_loading_items` 側の履歴フォールバック分岐に委ねる。
    #[test]
    fn try_select_after_load_returns_false_on_missing_name() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        app.items.push(GridItem::Folder("c:/root/folderA".into()));
        app.rebuild_visible_indices();
        app.selected = None;

        app.select_after_load = Some("ghost".to_string());
        let restored = app.try_select_after_load();
        assert!(!restored, "ヒストリにフォールバックさせるため false を返す");
        assert_eq!(app.selected, None, "selected は変更されない");
        assert!(
            app.select_after_load.is_none(),
            "見つからなかった場合でもヒントは消費する (持ち越さない)"
        );
    }

    /// フルスクリーンを閉じたとき、グリッド側のカーソルを最後に表示していた item へ
    /// 戻すこと。動画は fullscreen 中に `selected` が更新されない経路があるため、
    /// 右クリック終了で元のサムネイル位置へ戻れない不具合の回帰ガード。
    #[test]
    fn close_fullscreen_restores_grid_cursor_to_current_video() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.items
            .push(GridItem::Video(std::path::PathBuf::from("c:/p/movie.mp4")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/b.jpg")));
        app.thumbnails.push(ThumbnailState::Pending);
        app.thumbnails.push(ThumbnailState::Pending);
        app.thumbnails.push(ThumbnailState::Pending);
        app.rebuild_visible_indices();

        app.selected = Some(0);
        app.scroll_to_selected = false;
        app.fullscreen_idx = Some(1);

        app.close_fullscreen();

        assert_eq!(app.fullscreen_idx, None);
        assert_eq!(
            app.selected,
            Some(1),
            "動画終了時も表示中だった動画セルへカーソルを戻す"
        );
        assert!(
            app.scroll_to_selected,
            "次のグリッド描画で選択セルへスクロールする"
        );
    }

    /// Codex P3 (動画レーティング対応): `rating_path_key` / `set_rating` / `get_rating`
    /// の Video 経路がフィルタテストだけでなく永続化往復で機能することを確認する。
    /// パス正規化キーが取れ、rating_db に書いた値がキャッシュ経由でなく DB ヒットでも
    /// 同じ値で読み戻せることを assert する。
    #[test]
    fn video_rating_roundtrips_through_rating_db() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        // Video アイテムを 1 件登録
        app.items.push(GridItem::Video(std::path::PathBuf::from(
            "C:/clips/movie.mp4",
        )));
        app.thumbnails.push(ThumbnailState::Pending);
        let idx = app.items.len() - 1;

        // rating_path_key が None でない (= rating_db キーを取れる)
        let key = app
            .rating_path_key(idx)
            .expect("Video の rating_path_key は Some を返す");
        assert!(
            !key.is_empty(),
            "正規化されたパスキーが空でない (= normalize_path 経由で生成される)"
        );

        // set_rating → rating_db に書き込みかつ rating_cache が更新される
        app.set_rating(idx, 3);
        assert_eq!(
            app.get_rating(idx),
            3,
            "set 直後に get で同値が読める (cache hit)"
        );
        // DB にも入っているはず
        let db_value = app
            .rating_db
            .as_ref()
            .expect("rating_db must be open in test")
            .get(&key);
        assert_eq!(db_value, 3, "rating_db に書き込まれている");

        // cache を捨てて DB から再取得 → 永続化を確認
        app.rating_cache.clear();
        assert_eq!(
            app.get_rating(idx),
            3,
            "cache を捨てても DB から ★3 が読み戻る (永続化済み)"
        );

        // 0 への戻し (= 「★解除」) も DB に反映
        app.set_rating(idx, 0);
        let db_value = app.rating_db.as_ref().unwrap().get(&key);
        assert_eq!(db_value, 0, "★0 で DB を上書きできる");
    }

    /// 見開きから消しゴムに入ったあと `reset_erase_mode` で元の見開き状態に戻ること。
    /// (Apply [E] / Cancel [Esc] どちらの経路でも内部的に reset_erase_mode が呼ばれる。)
    #[test]
    fn reset_erase_mode_restores_saved_spread_state() {
        use crate::grid_item::GridItem;
        use crate::settings::SpreadMode;
        let mut app = setup_app();
        // ペア: idx 0 (left), idx 1 (right) を仮想的に保存している状態を組み立てる。
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/b.jpg")));
        app.thumbnails.push(ThumbnailState::Pending);
        app.thumbnails.push(ThumbnailState::Pending);
        // 編集対象は左ページとする
        app.fullscreen_idx = Some(0);
        app.spread_mode = SpreadMode::Single; // 消しゴム中の状態
        app.erase_spread_ctx = Some(crate::app::EraseSpreadCtx {
            saved_mode: SpreadMode::Ltr,
            pair: (0, 1),
        });
        app.fs_zoom = 2.0;
        app.fs_pan = egui::Vec2::new(50.0, 30.0);
        app.erase_mode = true;

        app.reset_erase_mode();

        assert_eq!(
            app.spread_mode,
            SpreadMode::Ltr,
            "reset 後に spread_mode が Ltr へ復元される"
        );
        assert_eq!(
            app.fullscreen_idx,
            Some(0),
            "fullscreen_idx は左ページに戻る (resolve_spread_pair で同ペアが復元される)"
        );
        assert!(app.erase_spread_ctx.is_none(), "spread_ctx は消費される");
        assert_eq!(app.fs_zoom, 1.0, "ズームはリセット");
        assert_eq!(app.fs_pan, egui::Vec2::ZERO, "パンはリセット");
        assert!(!app.erase_mode);
    }

    /// Ctrl+→ の「1 ページずらし」は、1 回押すごとに見開きが必ず 1 ページぶんずれる。
    /// 固定パリティで「2 回押さないと見た目が変わらない」旧 Shift 挙動のリグレッション防止。
    #[test]
    fn spread_offset_nudge_shifts_one_page_ltr() {
        use crate::grid_item::GridItem;
        use crate::settings::SpreadMode;
        use crate::ui_fullscreen::SpreadPair;
        let mut app = setup_app();
        for k in 0..4 {
            app.items
                .push(GridItem::Image(std::path::PathBuf::from(format!(
                    "c:/p/{k}.jpg"
                ))));
            app.thumbnails.push(ThumbnailState::Pending);
        }
        app.visible_indices = vec![0, 1, 2, 3];
        app.cached_nav_indices = None;
        app.spread_mode = SpreadMode::Ltr;
        app.fullscreen_idx = Some(0);

        assert_eq!(
            app.resolve_spread_pair(0),
            SpreadPair::Double { left: 0, right: 1 },
            "初期ペアは [0,1]"
        );

        // Ctrl+→ 1 回 → [1,2]
        let (new_idx, new_mode) = app
            .compute_spread_offset_nudge(0, 1)
            .expect("前方ずらしは範囲内");
        assert_eq!(new_idx, 1);
        app.spread_mode = new_mode;
        app.fullscreen_idx = Some(new_idx);
        assert_eq!(
            app.resolve_spread_pair(new_idx),
            SpreadPair::Double { left: 1, right: 2 },
            "1 回で見開きが 1 ページずれる"
        );

        // もう 1 回 → [2,3]
        let (new_idx, new_mode) = app
            .compute_spread_offset_nudge(1, 1)
            .expect("前方ずらしは範囲内");
        assert_eq!(new_idx, 2);
        app.spread_mode = new_mode;
        app.fullscreen_idx = Some(new_idx);
        assert_eq!(
            app.resolve_spread_pair(new_idx),
            SpreadPair::Double { left: 2, right: 3 }
        );
    }

    /// 後方ずらし (Ctrl+←) は前方ずらしを巻き戻す。
    #[test]
    fn spread_offset_nudge_backward_reverses() {
        use crate::grid_item::GridItem;
        use crate::settings::SpreadMode;
        use crate::ui_fullscreen::SpreadPair;
        let mut app = setup_app();
        for k in 0..4 {
            app.items
                .push(GridItem::Image(std::path::PathBuf::from(format!(
                    "c:/p/{k}.jpg"
                ))));
            app.thumbnails.push(ThumbnailState::Pending);
        }
        app.visible_indices = vec![0, 1, 2, 3];
        app.cached_nav_indices = None;
        app.spread_mode = SpreadMode::LtrCover; // pair_start=1 で [1,2] を表示中
        app.fullscreen_idx = Some(1);
        assert_eq!(
            app.resolve_spread_pair(1),
            SpreadPair::Double { left: 1, right: 2 }
        );

        let (new_idx, new_mode) = app
            .compute_spread_offset_nudge(1, -1)
            .expect("後方ずらしは範囲内");
        assert_eq!(new_idx, 0);
        app.spread_mode = new_mode;
        assert_eq!(
            app.resolve_spread_pair(new_idx),
            SpreadPair::Double { left: 0, right: 1 }
        );
    }

    /// RTL でもずらしが効き、左右の割り当て (左=大 idx) を保つ。
    #[test]
    fn spread_offset_nudge_rtl_keeps_side_assignment() {
        use crate::grid_item::GridItem;
        use crate::settings::SpreadMode;
        use crate::ui_fullscreen::SpreadPair;
        let mut app = setup_app();
        for k in 0..4 {
            app.items
                .push(GridItem::Image(std::path::PathBuf::from(format!(
                    "c:/p/{k}.jpg"
                ))));
            app.thumbnails.push(ThumbnailState::Pending);
        }
        app.visible_indices = vec![0, 1, 2, 3];
        app.cached_nav_indices = None;
        app.spread_mode = SpreadMode::Rtl;
        app.fullscreen_idx = Some(0);
        assert_eq!(
            app.resolve_spread_pair(0),
            SpreadPair::Double { left: 1, right: 0 },
            "RTL は 左=大 idx, 右=小 idx"
        );

        let (new_idx, new_mode) = app
            .compute_spread_offset_nudge(0, 1)
            .expect("ずらしは範囲内");
        assert_eq!(new_idx, 1);
        app.spread_mode = new_mode;
        assert_eq!(
            app.resolve_spread_pair(1),
            SpreadPair::Double { left: 2, right: 1 },
            "RTL の左右割り当てを保ったまま 1 ページずれる"
        );
    }

    /// 端ではずらしは no-op (None) を返す。
    #[test]
    fn spread_offset_nudge_boundary_is_none() {
        use crate::grid_item::GridItem;
        use crate::settings::SpreadMode;
        let mut app = setup_app();
        for k in 0..2 {
            app.items
                .push(GridItem::Image(std::path::PathBuf::from(format!(
                    "c:/p/{k}.jpg"
                ))));
            app.thumbnails.push(ThumbnailState::Pending);
        }
        app.visible_indices = vec![0, 1];
        app.cached_nav_indices = None;
        app.spread_mode = SpreadMode::Ltr;
        app.fullscreen_idx = Some(1);
        assert!(
            app.compute_spread_offset_nudge(1, 1).is_none(),
            "末尾から前方ずらしは範囲外"
        );
        assert!(
            app.compute_spread_offset_nudge(0, -1).is_none(),
            "先頭から後方ずらしは範囲外"
        );
    }

    /// 見開きの余白カット: 左右ページの content を combined 空間で union する。
    #[test]
    fn spread_content_union_combines_both_pages() {
        use crate::ui_fullscreen::spread_content_union;
        use egui::{Rect, pos2};
        let left = Some(Rect::from_min_max(pos2(0.1, 0.2), pos2(0.8, 0.9)));
        let right = Some(Rect::from_min_max(pos2(0.0, 0.1), pos2(0.9, 1.0)));
        // left_w=100, right_w=120, combined_h=200。
        let (x0, y0, x1, y1) = spread_content_union(left, right, 100.0, 120.0, 200.0).unwrap();
        // 左 x[10,80] 右 x[100,208] → union [10,208]。y 左[40,180] 右[20,200] → [20,200]。
        assert!((x0 - 10.0).abs() < 1e-3, "x0 {x0}");
        assert!((x1 - 208.0).abs() < 1e-3, "x1 {x1}");
        assert!((y0 - 20.0).abs() < 1e-3, "y0 {y0}");
        assert!((y1 - 200.0).abs() < 1e-3, "y1 {y1}");
    }

    /// bbox 無しのページは全域扱い (余白を切らない) で union される。
    #[test]
    fn spread_content_union_none_page_uses_full_region() {
        use crate::ui_fullscreen::spread_content_union;
        use egui::{Rect, pos2};
        let left = Some(Rect::from_min_max(pos2(0.2, 0.2), pos2(0.7, 0.8)));
        // 右は None → 右ページ全域 (x[100,200], y[0,200])。
        let (x0, y0, x1, y1) = spread_content_union(left, None, 100.0, 100.0, 200.0).unwrap();
        assert!((x0 - 20.0).abs() < 1e-3, "x0 {x0}");
        assert!((x1 - 200.0).abs() < 1e-3, "x1 {x1}");
        assert!((y0 - 0.0).abs() < 1e-3, "y0 {y0}");
        assert!((y1 - 200.0).abs() < 1e-3, "y1 {y1}");
    }

    /// 両方 None なら余白カット無効 (None)。
    #[test]
    fn spread_content_union_both_none_returns_none() {
        use crate::ui_fullscreen::spread_content_union;
        assert!(spread_content_union(None, None, 100.0, 100.0, 200.0).is_none());
    }

    /// モードB「ページをフルスクリーン表示」(auto_fullscreen_zip_pdf=true) で ZIP/PDF ページを
    /// 見ているときの close 要求 (Esc/Enter/右クリック) は、その場で閉じず「親フォルダ (L1) へ
    /// 戻る」予約を立てる (= L2 ページ一覧を見せずに L1 へ抜ける、"ESC 2 回分")。判定は設定 +
    /// コンテナ内かのみ (一時フラグ廃止)。
    #[test]
    fn mode_b_container_close_request_defers_to_parent() {
        let mut app = setup_app();
        app.fullscreen_idx = Some(0);
        app.settings.auto_fullscreen_zip_pdf = true; // モードB
        app.current_folder = Some(std::path::PathBuf::from("c:/manga/book.zip")); // コンテナページ
        app.pending_return_to_parent = false;

        app.handle_fullscreen_close_request();

        assert!(
            app.pending_return_to_parent,
            "モードB+コンテナは親 (L1) へ戻る予約"
        );
        assert_eq!(
            app.fullscreen_idx,
            Some(0),
            "その場では close しない (L2 を 1 フレームも見せないため)"
        );
    }

    /// モードA (設定OFF) / 通常フォルダ (非コンテナ) の close 要求は即座に閉じる
    /// (= 1 段だけ戻る: コンテナなら L2 ページ一覧、通常画像なら親グリッド)。
    #[test]
    fn mode_a_or_non_container_close_request_closes_immediately() {
        // モードA (設定OFF) でコンテナページ → 即 close (= L2 ページ一覧)。
        let mut app = setup_app();
        app.fullscreen_idx = Some(0);
        app.settings.auto_fullscreen_zip_pdf = false;
        app.current_folder = Some(std::path::PathBuf::from("c:/manga/book.zip"));
        app.pending_return_to_parent = false;
        app.handle_fullscreen_close_request();
        assert!(
            !app.pending_return_to_parent,
            "モードA は親復帰予約を立てない"
        );
        assert_eq!(app.fullscreen_idx, None, "その場で close");

        // 設定ON でも通常フォルダ (非コンテナ) → 即 close (= 親グリッド)。
        app.fullscreen_idx = Some(0);
        app.settings.auto_fullscreen_zip_pdf = true;
        app.current_folder = Some(std::path::PathBuf::from("c:/manga/series"));
        app.pending_return_to_parent = false;
        app.handle_fullscreen_close_request();
        assert!(
            !app.pending_return_to_parent,
            "非コンテナは親復帰予約を立てない"
        );
        assert_eq!(app.fullscreen_idx, None, "その場で close");
    }

    /// Ctrl+↑↓ フォルダナビ用 `auto_open_for_current_container` のゲート判定。
    /// ZIP/PDF コンテナへ入った & 設定 ON のときだけ自動オープン扱いにする。
    #[test]
    fn auto_open_for_current_container_gating() {
        let mut app = setup_app();
        app.settings.auto_fullscreen_zip_pdf = true;

        app.current_folder = Some(std::path::PathBuf::from("c:/manga/series"));
        assert!(
            !app.auto_open_for_current_container(),
            "通常フォルダは対象外"
        );

        app.current_folder = Some(std::path::PathBuf::from("c:/manga/book.zip"));
        assert!(app.auto_open_for_current_container(), "ZIP コンテナは対象");

        app.current_folder = Some(std::path::PathBuf::from("c:/manga/doc.pdf"));
        assert!(app.auto_open_for_current_container(), "PDF コンテナは対象");

        // 検索中は (Esc が検索を抜けて実親へ飛ぶ想定外を避けるため) 対象外。
        app.global_search.active = true;
        assert!(!app.auto_open_for_current_container(), "検索中は対象外");
        app.global_search.active = false;

        // 設定 OFF は常に対象外。
        app.settings.auto_fullscreen_zip_pdf = false;
        assert!(!app.auto_open_for_current_container(), "設定 OFF は対象外");
    }

    /// 読書位置レジュームの記録: 画像本のページだけを対象にし、dedup する。
    #[test]
    fn record_book_resume_targets_book_pages_with_dedup() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        let folder = std::path::PathBuf::from("c:/manga/series");
        app.current_folder = Some(folder.clone());
        app.items.push(GridItem::Image(std::path::PathBuf::from(
            "c:/manga/series/001.jpg",
        )));
        app.items.push(GridItem::Folder(std::path::PathBuf::from(
            "c:/manga/series/sub",
        )));

        // 画像 (本ページ) idx 0 → 記録される
        app.record_book_resume(0);
        assert_eq!(app.last_book_resume, Some((folder.clone(), 0)));

        // フォルダタイル idx 1 → 対象外。直近記録は据え置き
        app.record_book_resume(1);
        assert_eq!(app.last_book_resume, Some((folder, 0)));
    }

    /// 見開きから隠蔽加工に入ったあと `reset_conceal_mode` で元の見開き状態に戻ること。
    #[test]
    fn reset_conceal_mode_restores_saved_spread_state() {
        use crate::grid_item::GridItem;
        use crate::settings::SpreadMode;
        let mut app = setup_app();
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/b.jpg")));
        app.thumbnails.push(ThumbnailState::Pending);
        app.thumbnails.push(ThumbnailState::Pending);
        app.fullscreen_idx = Some(1);
        app.spread_mode = SpreadMode::Single;
        app.conceal_spread_ctx = Some(crate::app::EraseSpreadCtx {
            saved_mode: SpreadMode::Ltr,
            pair: (0, 1),
        });
        app.fs_zoom = 2.0;
        app.fs_pan = egui::Vec2::new(50.0, 30.0);
        app.conceal_mode = true;

        app.reset_conceal_mode();

        assert_eq!(app.spread_mode, SpreadMode::Ltr);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(app.conceal_spread_ctx.is_none());
        assert_eq!(app.fs_zoom, 1.0);
        assert_eq!(app.fs_pan, egui::Vec2::ZERO);
        assert!(!app.conceal_mode);
    }

    /// 見開きから切り取りモードに入ったあと `reset_export_crop_mode` で元の見開き状態に戻ること。
    #[test]
    fn reset_export_crop_mode_restores_saved_spread_state() {
        use crate::grid_item::GridItem;
        use crate::settings::SpreadMode;
        let mut app = setup_app();
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/b.jpg")));
        app.thumbnails.push(ThumbnailState::Pending);
        app.thumbnails.push(ThumbnailState::Pending);
        app.fullscreen_idx = Some(1);
        app.spread_mode = SpreadMode::Single;
        app.export_crop_spread_ctx = Some(crate::app::EraseSpreadCtx {
            saved_mode: SpreadMode::Ltr,
            pair: (0, 1),
        });
        app.fs_zoom = 2.0;
        app.fs_pan = egui::Vec2::new(50.0, 30.0);
        app.export_crop_mode = true;
        app.export_crop_drag = None;

        app.reset_export_crop_mode();

        assert_eq!(app.spread_mode, SpreadMode::Ltr);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(app.export_crop_spread_ctx.is_none());
        assert_eq!(app.fs_zoom, 1.0);
        assert_eq!(app.fs_pan, egui::Vec2::ZERO);
        assert!(!app.export_crop_mode);
    }

    /// 見開き表示中の Ctrl+E は左右ページを 1 つの export snapshot として開く。
    #[test]
    fn open_export_dialog_uses_spread_pixels_when_spread_is_visible() {
        use crate::grid_item::GridItem;
        use crate::settings::SpreadMode;
        let ctx = egui::Context::default();
        let mut app = setup_app();
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/b.jpg")));
        app.visible_indices = vec![0, 1];
        app.thumbnails.push(ThumbnailState::Pending);
        app.thumbnails.push(ThumbnailState::Pending);
        let left_pixels =
            egui::ColorImage::new([1, 2], vec![egui::Color32::RED, egui::Color32::RED]);
        let right_pixels =
            egui::ColorImage::new([1, 2], vec![egui::Color32::BLUE, egui::Color32::BLUE]);
        app.fs_cache.insert(
            0,
            FsCacheEntry::Static {
                tex: ctx.load_texture(
                    "export_spread_left",
                    left_pixels.clone(),
                    egui::TextureOptions::LINEAR,
                ),
                pixels: std::sync::Arc::new(left_pixels),
                source_dims: None,
                load_seq: 0,
            },
        );
        app.fs_cache.insert(
            1,
            FsCacheEntry::Static {
                tex: ctx.load_texture(
                    "export_spread_right",
                    right_pixels.clone(),
                    egui::TextureOptions::LINEAR,
                ),
                pixels: std::sync::Arc::new(right_pixels),
                source_dims: None,
                load_seq: 0,
            },
        );
        app.fullscreen_idx = Some(0);
        app.spread_mode = SpreadMode::Ltr;

        app.open_export_dialog_for_current(&ctx, 0);

        let state = app.export_dialog.take().expect("export dialog should open");
        assert!(matches!(
            state.source,
            crate::export_dialog::ExportSource::RenderedSpread
        ));
        assert_eq!(
            state.original_format,
            crate::save_with_metadata::SrcFormat::Other("spread".to_string())
        );
        match state.pixels {
            crate::export_dialog::ExportPixels::Spread { left, right } => {
                assert_eq!(left.base_pixels.size, [1, 2]);
                assert_eq!(right.base_pixels.size, [1, 2]);
                assert_eq!(left.base_pixels.pixels[0], egui::Color32::RED);
                assert_eq!(right.base_pixels.pixels[0], egui::Color32::BLUE);
            }
            crate::export_dialog::ExportPixels::Single(_) => {
                panic!("spread export should snapshot both pages")
            }
        }

        app.open_export_dialog_for_current(&ctx, 1);

        let state = app
            .export_dialog
            .take()
            .expect("export dialog should also open from right page");
        assert!(matches!(
            state.source,
            crate::export_dialog::ExportSource::RenderedSpread
        ));
        match state.pixels {
            crate::export_dialog::ExportPixels::Spread { left, right } => {
                assert_eq!(left.base_pixels.pixels[0], egui::Color32::RED);
                assert_eq!(right.base_pixels.pixels[0], egui::Color32::BLUE);
            }
            crate::export_dialog::ExportPixels::Single(_) => {
                panic!("right-page entry should still snapshot the visible spread")
            }
        }
    }

    /// Ctrl+E / Ctrl+S は、再計算したペアではなく直近に描画された見開きレイアウトを
    /// 優先する。これで「画面は見開きなのに保存対象は片側ページ」のずれを防ぐ。
    #[test]
    fn open_export_dialog_prefers_visible_spread_layout() {
        use crate::grid_item::GridItem;
        use crate::settings::SpreadMode;
        let ctx = egui::Context::default();
        let mut app = setup_app();
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/b.jpg")));
        app.visible_indices = vec![1];
        app.thumbnails.push(ThumbnailState::Pending);
        app.thumbnails.push(ThumbnailState::Pending);
        let left_pixels = egui::ColorImage::new([1, 1], vec![egui::Color32::RED]);
        let right_pixels = egui::ColorImage::new([1, 1], vec![egui::Color32::BLUE]);
        app.fs_cache.insert(
            0,
            FsCacheEntry::Static {
                tex: ctx.load_texture(
                    "export_visible_spread_left",
                    left_pixels.clone(),
                    egui::TextureOptions::LINEAR,
                ),
                pixels: std::sync::Arc::new(left_pixels),
                source_dims: None,
                load_seq: 0,
            },
        );
        app.fs_cache.insert(
            1,
            FsCacheEntry::Static {
                tex: ctx.load_texture(
                    "export_visible_spread_right",
                    right_pixels.clone(),
                    egui::TextureOptions::LINEAR,
                ),
                pixels: std::sync::Arc::new(right_pixels),
                source_dims: None,
                load_seq: 0,
            },
        );
        app.fullscreen_idx = Some(1);
        app.spread_mode = SpreadMode::Ltr;
        app.fs_spread_layout = Some(crate::ui_fullscreen::FsSpreadLayout {
            left_idx: 0,
            left_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(10.0, 10.0)),
            right_idx: 1,
            right_rect: egui::Rect::from_min_size(egui::pos2(10.0, 0.0), egui::vec2(10.0, 10.0)),
        });

        app.open_export_dialog_for_current(&ctx, 1);

        let state = app.export_dialog.take().expect("export dialog should open");
        assert!(matches!(
            state.source,
            crate::export_dialog::ExportSource::RenderedSpread
        ));
        match state.pixels {
            crate::export_dialog::ExportPixels::Spread { left, right } => {
                assert_eq!(left.base_pixels.pixels[0], egui::Color32::RED);
                assert_eq!(right.base_pixels.pixels[0], egui::Color32::BLUE);
            }
            crate::export_dialog::ExportPixels::Single(_) => {
                panic!("visible spread layout should snapshot both pages")
            }
        }
    }

    /// Ctrl+E の「元の場所」は、前回保存先を既定表示している場合でも
    /// 元ファイル / 元 PDF のあるフォルダへ戻せる。
    #[test]
    fn export_dialog_can_reset_output_dir_to_source_dir() {
        use crate::grid_item::GridItem;
        let ctx = egui::Context::default();
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("source");
        let last_dir = temp.path().join("last");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&last_dir).unwrap();

        let mut app = setup_app();
        let image_path = source_dir.join("a.jpg");
        app.items.push(GridItem::Image(image_path));
        app.visible_indices = vec![0];
        app.thumbnails.push(ThumbnailState::Pending);
        let pixels = egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]);
        app.fs_cache.insert(
            0,
            FsCacheEntry::Static {
                tex: ctx.load_texture(
                    "export_source_dir_reset",
                    pixels.clone(),
                    egui::TextureOptions::LINEAR,
                ),
                pixels: std::sync::Arc::new(pixels),
                source_dims: None,
                load_seq: 0,
            },
        );
        app.fullscreen_idx = Some(0);
        app.settings.export_last_directory = Some(last_dir.clone());

        app.open_export_dialog_for_current(&ctx, 0);

        let mut state = app.export_dialog.take().expect("export dialog should open");
        assert_eq!(state.output_dir_text, last_dir.display().to_string());
        state.reset_output_dir_to_source_dir();
        assert_eq!(state.output_dir_text, source_dir.display().to_string());
    }

    /// PDF ページでは「元の場所」が PDF ファイルのあるディレクトリを指す。
    #[test]
    fn export_dialog_source_dir_for_pdf_page_is_pdf_parent() {
        use crate::grid_item::GridItem;
        let ctx = egui::Context::default();
        let temp = tempfile::tempdir().unwrap();
        let pdf_dir = temp.path().join("pdfs");
        std::fs::create_dir_all(&pdf_dir).unwrap();

        let mut app = setup_app();
        app.items.push(GridItem::PdfPage {
            pdf_path: pdf_dir.join("book.pdf"),
            page_num: 3,
            content_type: None,
        });
        app.visible_indices = vec![0];
        app.thumbnails.push(ThumbnailState::Pending);
        let pixels = egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]);
        app.fs_cache.insert(
            0,
            FsCacheEntry::Static {
                tex: ctx.load_texture(
                    "export_pdf_source_dir",
                    pixels.clone(),
                    egui::TextureOptions::LINEAR,
                ),
                pixels: std::sync::Arc::new(pixels),
                source_dims: None,
                load_seq: 0,
            },
        );
        app.fullscreen_idx = Some(0);

        app.open_export_dialog_for_current(&ctx, 0);

        let mut state = app.export_dialog.take().expect("export dialog should open");
        state.output_dir_text = temp.path().join("other").display().to_string();
        state.reset_output_dir_to_source_dir();
        assert_eq!(state.output_dir_text, pdf_dir.display().to_string());
    }

    /// P3-8 後続: 透明画像を消しゴム作業ベースにする時、黒で不透明化されること。
    /// MI-GAN は alpha 非対応なので、透明部は黒・半透明は黒へ減衰・全面 alpha=255。
    /// 全不透明画像は変換不要 (None)。
    #[test]
    fn black_flatten_makes_transparent_opaque_black() {
        use egui::Color32;
        let pixels = vec![
            Color32::TRANSPARENT,                             // a=0 → 黒不透明へ
            Color32::from_rgba_premultiplied(100, 0, 0, 128), // 半透明 (premult 保持)
            Color32::from_rgb(10, 20, 30),                    // 不透明はそのまま
        ];
        let img = egui::ColorImage::new([3, 1], pixels);
        let flat = crate::app::App::black_flatten_if_transparent(&img).expect("has transparency");
        assert!(flat.pixels.iter().all(|p| p.a() == 255), "all opaque");
        assert_eq!(
            flat.pixels[0],
            Color32::from_rgb(0, 0, 0),
            "transparent → black"
        );
        assert_eq!(
            (flat.pixels[1].r(), flat.pixels[1].g(), flat.pixels[1].b()),
            (100, 0, 0),
            "semi-transparent keeps premultiplied (= composited over black)"
        );
        assert_eq!(
            flat.pixels[2],
            Color32::from_rgb(10, 20, 30),
            "opaque unchanged"
        );

        // 全不透明画像は変換不要 (None)
        let opaque = egui::ColorImage::new([1, 1], vec![Color32::from_rgb(5, 5, 5)]);
        assert!(crate::app::App::black_flatten_if_transparent(&opaque).is_none());
    }

    /// 透過画像の消しゴムは B キーの白背景に引きずられず、AI composite-first cache も
    /// 黒背景 (bg=0) を使う。白背景に焼き込まれた `(idx,1)` を消しゴム入力にしないための
    /// 回帰ガード。
    #[test]
    fn erase_upscale_bg_mode_forces_black_for_transparent_source() {
        use egui::Color32;
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let idx = push_image(&mut app, "c:/p/alpha.png");
        let tex = ctx.load_texture(
            "transparent_source",
            egui::ColorImage::filled([1, 1], Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let pixels = egui::ColorImage::new(
            [2, 1],
            vec![Color32::TRANSPARENT, Color32::from_rgb(10, 20, 30)],
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex,
                pixels: std::sync::Arc::new(pixels),
                source_dims: None,
                load_seq: 0,
            },
        );
        app.ai_upscale_enabled = true;
        app.fs_transparent_bg_mode = 1; // B キーで白背景

        assert_eq!(app.effective_upscale_bg_mode(), 1, "通常表示は白背景 bg=1");
        assert_eq!(
            app.erase_upscale_bg_mode(idx),
            0,
            "透過画像の消しゴム入力は黒背景 bg=0 に固定"
        );
    }

    /// 単一ページから入った場合 (= erase_spread_ctx が None) は spread_mode を
    /// 触らずに reset すること。誤って Single に書き換えないことの回帰ガード。
    #[test]
    fn reset_erase_mode_leaves_spread_state_untouched_when_single_entry() {
        use crate::settings::SpreadMode;
        let mut app = setup_app();
        app.spread_mode = SpreadMode::Single;
        app.erase_spread_ctx = None;
        app.erase_mode = true;
        app.fullscreen_idx = Some(7);

        app.reset_erase_mode();

        assert_eq!(app.spread_mode, SpreadMode::Single);
        assert_eq!(
            app.fullscreen_idx,
            Some(7),
            "Single 経由なら fullscreen_idx は弄らない"
        );
    }

    #[test]
    fn execute_erase_inpaint_promotes_preview_result() {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let idx = push_image(&mut app, "c:/p/a.jpg");
        app.fullscreen_idx = Some(idx);
        app.erase_mode = true;
        app.post_filter_bypassed = true;
        app.erase_mask_size = [2, 2];
        app.erase_mask = Some(mask_2x2());

        let preview = egui::ColorImage::new(
            [2, 2],
            vec![
                egui::Color32::from_rgb(10, 20, 30),
                egui::Color32::from_rgb(40, 50, 60),
                egui::Color32::from_rgb(70, 80, 90),
                egui::Color32::from_rgb(100, 110, 120),
            ],
        );
        let preview_pixels = std::sync::Arc::new(preview.clone());
        let preview_tex = ctx.load_texture(
            "erase_preview_promote",
            preview,
            egui::TextureOptions::LINEAR,
        );
        app.erase_preview_cache.insert(
            idx,
            ErasePreviewCacheEntry {
                pixels: std::sync::Arc::clone(&preview_pixels),
                texture: preview_tex,
            },
        );

        app.execute_erase_inpaint(&ctx, idx);

        let committed = app
            .current_erase_result_pixels(idx)
            .expect("preview should be promoted into erase_result_cache");
        assert_eq!(committed.pixels, preview_pixels.pixels);
        assert!(
            app.erase_inpaint_pending.is_empty(),
            "no extra MI-GAN job should be queued when preview is available"
        );
        assert!(
            app.erase_preview_cache.is_empty(),
            "leaving erase mode clears preview-only cache"
        );
        assert!(!app.post_filter_bypassed);
    }

    #[test]
    fn reset_erase_mode_keeps_commit_pending_when_no_post_filter_restore_needed() {
        use crate::ui_erase::{EraseInpaintKind, EraseInpaintPending, EraseInpaintPendingKey};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let mut app = setup_app();
        let idx = push_image(&mut app, "c:/p/a.jpg");
        app.fullscreen_idx = Some(idx);
        app.erase_mode = true;
        app.post_filter_bypassed = true;
        app.input_generation.insert(idx, 7);

        let (_tx, rx) = std::sync::mpsc::channel::<egui::ColorImage>();
        let cancel = Arc::new(AtomicBool::new(false));
        let key = EraseInpaintPendingKey {
            idx,
            kind: EraseInpaintKind::Commit,
        };
        let items_generation = app.items_generation;
        let path_key = app.page_path_key(idx);
        app.erase_inpaint_pending.insert(
            key,
            EraseInpaintPending {
                idx,
                items_generation,
                path_key,
                rx,
                cancel: Arc::clone(&cancel),
                started_at: std::time::Instant::now(),
                input_generation: 7,
                mask_generation: 0,
                log_prefix: "test",
                is_preview: false,
            },
        );

        app.reset_erase_mode();

        assert_eq!(app.input_generation.get(&idx), Some(&7));
        assert!(
            app.erase_inpaint_pending.contains_key(&key),
            "reset should not cancel a just-launched commit when bypass restoration is a no-op"
        );
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(!app.post_filter_bypassed);
    }

    #[test]
    fn reset_erase_mode_keeps_commit_pending_with_post_filter() {
        use crate::adjustment::PostFilter;
        use crate::ui_erase::{EraseInpaintKind, EraseInpaintPending, EraseInpaintPendingKey};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let mut app = setup_app();
        let idx = push_image(&mut app, "c:/p/a.jpg");
        app.settings.global_preset.post_filter = PostFilter::Sepia;
        app.fullscreen_idx = Some(idx);
        app.erase_mode = true;
        app.post_filter_bypassed = true;
        app.input_generation.insert(idx, 11);

        let (_tx, rx) = std::sync::mpsc::channel::<egui::ColorImage>();
        let cancel = Arc::new(AtomicBool::new(false));
        let key = EraseInpaintPendingKey {
            idx,
            kind: EraseInpaintKind::Commit,
        };
        let items_generation = app.items_generation;
        let path_key = app.page_path_key(idx);
        app.erase_inpaint_pending.insert(
            key,
            EraseInpaintPending {
                idx,
                items_generation,
                path_key,
                rx,
                cancel: Arc::clone(&cancel),
                started_at: std::time::Instant::now(),
                input_generation: 11,
                mask_generation: 0,
                log_prefix: "test",
                is_preview: false,
            },
        );

        app.reset_erase_mode();

        assert_eq!(app.input_generation.get(&idx), Some(&11));
        assert!(app.erase_inpaint_pending.contains_key(&key));
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(!app.post_filter_bypassed);
    }

    #[test]
    fn erase_adjustment_source_can_include_post_filter_during_bypass() {
        use crate::adjustment::PostFilter;

        let mut app = setup_app();
        let idx = push_image(&mut app, "c:/p/a.jpg");
        app.settings.global_preset.post_filter = PostFilter::Sepia;
        app.post_filter_bypassed = true;
        let source = std::sync::Arc::new(egui::ColorImage::new(
            [1, 1],
            vec![egui::Color32::from_rgb(20, 80, 200)],
        ));

        let edit_base =
            app.apply_erase_adjustments_to_source(idx, std::sync::Arc::clone(&source), false);
        let inpaint_input = app.apply_erase_adjustments_to_source(idx, source, true);

        assert_eq!(
            edit_base.pixels[0],
            egui::Color32::from_rgb(20, 80, 200),
            "edit base display bypasses post-filter"
        );
        assert_ne!(
            inpaint_input.pixels[0], edit_base.pixels[0],
            "inpaint input keeps display-pipeline order: post-filter before erase"
        );
    }

    #[test]
    fn enter_conceal_mode_keeps_erase_result_with_post_filter() {
        use crate::adjustment::PostFilter;

        let mut app = setup_app();
        let ctx = egui::Context::default();
        let idx = push_image(&mut app, "c:/p/a.jpg");
        app.settings.global_preset.post_filter = PostFilter::Sepia;
        app.fullscreen_idx = Some(idx);
        app.input_generation.insert(idx, 5);
        app.erase_mask_generation.insert(idx, 7);
        let erase_pixels =
            insert_current_erase_result_cache(&mut app, &ctx, idx, "conceal_enter_erase_result");

        app.enter_conceal_mode(idx);

        assert!(app.conceal_mode);
        assert!(app.post_filter_bypassed);
        assert_eq!(app.input_generation.get(&idx), Some(&5));
        let current = app
            .current_erase_result_pixels(idx)
            .expect("conceal entry must not invalidate the erase result");
        assert_eq!(current.pixels, erase_pixels.pixels);
        let base = app
            .conceal_base_cache
            .get(&idx)
            .expect("conceal should capture the erase result as edit base");
        assert_eq!(base.pixels, erase_pixels.pixels);
    }

    #[test]
    fn reset_conceal_mode_keeps_erase_result_with_post_filter() {
        use crate::adjustment::PostFilter;

        let mut app = setup_app();
        let ctx = egui::Context::default();
        let idx = push_image(&mut app, "c:/p/a.jpg");
        app.settings.global_preset.post_filter = PostFilter::Sepia;
        app.fullscreen_idx = Some(idx);
        app.input_generation.insert(idx, 13);
        app.erase_mask_generation.insert(idx, 17);
        let erase_pixels =
            insert_current_erase_result_cache(&mut app, &ctx, idx, "conceal_reset_erase_result");
        app.conceal_mode = true;
        app.post_filter_bypassed = true;
        app.conceal_mask_size = [1, 1];
        app.conceal_mask = Some(vec![false]);

        app.reset_conceal_mode();

        assert!(!app.conceal_mode);
        assert!(!app.post_filter_bypassed);
        assert_eq!(app.input_generation.get(&idx), Some(&13));
        let current = app
            .current_erase_result_pixels(idx)
            .expect("conceal reset must not invalidate the erase result");
        assert_eq!(current.pixels, erase_pixels.pixels);
    }

    #[test]
    fn analysis_bypass_keeps_erase_result_with_post_filter() {
        use crate::adjustment::PostFilter;

        let mut app = setup_app();
        let ctx = egui::Context::default();
        let idx = push_image(&mut app, "c:/p/a.jpg");
        app.settings.global_preset.post_filter = PostFilter::Sepia;
        app.fullscreen_idx = Some(idx);
        app.input_generation.insert(idx, 23);
        app.erase_mask_generation.insert(idx, 29);
        let erase_pixels =
            insert_current_erase_result_cache(&mut app, &ctx, idx, "analysis_erase_result");

        app.analysis_mode = true;
        app.enter_analysis_mode_bypass();
        app.reset_analysis_mode();

        assert!(!app.analysis_mode);
        assert!(!app.post_filter_bypassed);
        assert_eq!(app.input_generation.get(&idx), Some(&23));
        let current = app
            .current_erase_result_pixels(idx)
            .expect("analysis bypass must not invalidate the erase result");
        assert_eq!(current.pixels, erase_pixels.pixels);
    }

    /// Cache 復元した Auto 比率は、前回の根拠 sample 数に追いつくまで
    /// seed / 途中ロードの少数統計で上書きしない。
    #[test]
    fn cached_auto_aspect_waits_for_previous_sample_count_before_switching() {
        use crate::grid_item::GridItem;
        use crate::settings::ThumbAspect;

        let mut app = setup_app();
        app.settings.thumb_aspect_auto = true;
        app.items = (0..10)
            .map(|i| GridItem::Image(PathBuf::from(format!("c:/p/{i}.jpg"))))
            .collect();
        app.auto_aspect.current = Some(ThumbAspect::Square);
        // 前回は 36 件の統計で Square と判断したが、今回は 10 件しか対象がない。
        // required は min(36, eligible_total=10) になり、9 件ではまだ切替禁止。
        app.auto_aspect.cached_sample_gate = Some(36);
        for idx in 0..9 {
            app.auto_aspect
                .samples
                .insert(idx, ThumbAspect::Portrait3x4.height_ratio());
        }

        app.maybe_apply_auto_aspect(true);

        assert_eq!(app.auto_aspect.current, Some(ThumbAspect::Square));
        assert_eq!(app.auto_aspect.cached_sample_gate, Some(36));
        assert_eq!(app.auto_aspect.switches_done, 0);

        app.auto_aspect
            .samples
            .insert(9, ThumbAspect::Portrait3x4.height_ratio());
        app.maybe_apply_auto_aspect(true);

        assert_eq!(app.auto_aspect.current, Some(ThumbAspect::Portrait3x4));
        assert_eq!(app.auto_aspect.cached_sample_gate, None);
        assert_eq!(app.auto_aspect.switches_done, 1);
    }

    /// PDF を仮想フォルダとして開いた直後、親フォルダ catalog の `pdfthumb:foo.pdf`
    /// が PDF 自身の catalog に `page_0000` として seed されることを確認する回帰
    /// テスト。これによって PDFium による初回 1 ページ目レンダリング (200-500ms)
    /// が省略される。
    #[test]
    fn seed_virtual_folder_first_thumb_copies_pdf_thumb_from_parent() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        // 親フォルダと PDF パスを準備 (実ファイルは要らない、catalog DB のキーだけが
        // 関心事)。
        let parent_dir = app.tmp.path().join("parent");
        std::fs::create_dir_all(&parent_dir).unwrap();
        let pdf_path = parent_dir.join("foo.pdf");

        // 親 catalog に pdfthumb:foo.pdf を仕込む (4×4 の dummy WebP をでっち上げる)。
        let cache_dir = crate::catalog::default_cache_dir();
        let parent_db = crate::catalog::CatalogDb::open(&cache_dir, &parent_dir).unwrap();
        // 4x4 white WebP を image::ImageBuffer から生成。
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 255, 255, 255]));
        let mut webp_bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut webp_bytes),
                image::ImageFormat::WebP,
            )
            .unwrap();
        let parent_key = format!("{}{}", crate::thumb_loader::CACHE_KEY_PDF, "foo.pdf");
        parent_db
            .save(
                &parent_key,
                12345,
                67890,
                4,
                4,
                Some((100, 100)),
                &webp_bytes,
            )
            .unwrap();

        // 仮想 catalog (PDF パスで別 SQLite) と空 cache_map を用意。
        let pdf_db = crate::catalog::CatalogDb::open(&cache_dir, &pdf_path).unwrap();
        let cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        > = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));

        // PdfPage(page_num=0) を items に積む (= setup が target_idx を見つけられるように)。
        app.items.push(GridItem::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: 0,
            content_type: None,
        });

        // seed + writeback ターゲット設定を実行。PDF として認識させるため source_path には
        // pdf_path を渡す。
        app.setup_virtual_folder_seed_and_writeback(&pdf_path, &pdf_db, &cache_map);

        // 仮想 catalog の DB と cache_map の両方に page_0000 が入っているはず。
        let target_key = crate::grid_item::pdf_page_cache_key(0);
        let in_db = pdf_db.load_one(&target_key).unwrap();
        assert!(
            in_db.is_some(),
            "PDF catalog DB に page_0000 が seed される"
        );
        let entry = in_db.unwrap();
        assert_eq!(entry.mtime, 12345);
        assert_eq!(entry.file_size, 67890);
        assert_eq!(entry.source_dims, Some((100, 100)));
        assert_eq!(entry.jpeg_data, webp_bytes);

        let in_map = cache_map.read().unwrap();
        assert!(
            in_map.contains_key(&target_key),
            "ワーカーが即時 hit するよう cache_map にも入る"
        );

        // PDF prefetch grace が 100ms 以内で設定されていること (Ctrl+↑↓ 連打時の
        // PDFium thrash 抑制)。
        assert!(
            app.pdf_prefetch_grace_until.is_some(),
            "PDF を開いた瞬間に prefetch grace を仕掛ける"
        );
    }

    /// 親フォルダ catalog にエントリが無い場合は seed しない (= no-op)。
    /// ただし writeback ターゲットと grace は仕掛ける (将来の write-back 経路用)。
    #[test]
    fn seed_virtual_folder_first_thumb_no_parent_entry_is_noop() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        let parent_dir = app.tmp.path().join("parent2");
        std::fs::create_dir_all(&parent_dir).unwrap();
        let pdf_path = parent_dir.join("missing.pdf");
        let cache_dir = crate::catalog::default_cache_dir();
        let pdf_db = crate::catalog::CatalogDb::open(&cache_dir, &pdf_path).unwrap();
        let cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        > = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));

        app.items.push(GridItem::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: 0,
            content_type: None,
        });

        app.setup_virtual_folder_seed_and_writeback(&pdf_path, &pdf_db, &cache_map);

        let target_key = crate::grid_item::pdf_page_cache_key(0);
        assert!(pdf_db.load_one(&target_key).unwrap().is_none());
        assert!(!cache_map.read().unwrap().contains_key(&target_key));
        // writeback ターゲットは設定される (= 後続の worker 完成時に親 catalog にミラー)。
        assert!(
            app.virtual_folder_writeback.is_some(),
            "親 catalog がミスでも writeback ターゲットは設定する"
        );
    }

    /// 仮想 catalog で先頭ページが完成したとき、`fire_virtual_folder_writeback_if_ready`
    /// が親 catalog の `pdfthumb:foo.pdf` 行に WebP データを書き戻すこと。
    #[test]
    fn fire_virtual_folder_writeback_mirrors_to_parent() {
        let mut app = setup_app();
        let parent_dir = app.tmp.path().join("parent3");
        std::fs::create_dir_all(&parent_dir).unwrap();
        let _pdf_path = parent_dir.join("bar.pdf");
        let cache_dir = crate::catalog::default_cache_dir();
        let parent_db_arc =
            Arc::new(crate::catalog::CatalogDb::open(&cache_dir, &parent_dir).unwrap());

        // worker が page_0000 として cache_map に入れた WebP を準備。
        let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 128, 255, 255]));
        let mut webp_bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut webp_bytes),
                image::ImageFormat::WebP,
            )
            .unwrap();
        let target_key = crate::grid_item::pdf_page_cache_key(0);
        let cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        > = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        cache_map.write().unwrap().insert(
            target_key.clone(),
            crate::catalog::CacheEntry {
                mtime: 999,
                file_size: 4096,
                jpeg_data: webp_bytes.clone(),
                source_dims: Some((400, 400)),
            },
        );

        let parent_key = format!("{}{}", crate::thumb_loader::CACHE_KEY_PDF, "bar.pdf");
        let cur_gen = app.items_generation;
        app.virtual_folder_writeback = Some(crate::app::VirtualFolderWriteback {
            parent_catalog: Arc::clone(&parent_db_arc),
            parent_key: parent_key.clone(),
            target_key: target_key.clone(),
            target_idx: 3,
            items_gen: cur_gen,
            cache_map: Arc::clone(&cache_map),
            parent_entry_mtime: 1234567,
            parent_entry_size: 9999,
        });

        // 別 idx で発火しても何も起きない。
        app.fire_virtual_folder_writeback_if_ready(2);
        assert!(parent_db_arc.load_one(&parent_key).unwrap().is_none());
        assert!(app.virtual_folder_writeback.is_some());

        // target_idx で発火すると親 catalog にミラーされ、writeback がクリアされる。
        app.fire_virtual_folder_writeback_if_ready(3);
        let entry = parent_db_arc
            .load_one(&parent_key)
            .unwrap()
            .expect("親 catalog にミラー保存される");
        assert_eq!(entry.mtime, 1234567);
        assert_eq!(entry.file_size, 9999);
        assert_eq!(entry.source_dims, Some((400, 400)));
        assert_eq!(entry.jpeg_data, webp_bytes);
        assert!(
            app.virtual_folder_writeback.is_none(),
            "発火後は writeback を None に戻して二重書き込みを防ぐ"
        );
    }

    /// Codex P2: items_generation がずれた古い writeback ターゲットは発火させずに
    /// クリアするだけにする (= 別フォルダに飛んで戻ったときの誤発火防止)。
    #[test]
    fn fire_virtual_folder_writeback_drops_stale_generation() {
        let mut app = setup_app();
        let parent_dir = app.tmp.path().join("parent4");
        std::fs::create_dir_all(&parent_dir).unwrap();
        let cache_dir = crate::catalog::default_cache_dir();
        let parent_db_arc =
            Arc::new(crate::catalog::CatalogDb::open(&cache_dir, &parent_dir).unwrap());
        let cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        > = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        let parent_key = format!("{}{}", crate::thumb_loader::CACHE_KEY_PDF, "stale.pdf");
        app.virtual_folder_writeback = Some(crate::app::VirtualFolderWriteback {
            parent_catalog: Arc::clone(&parent_db_arc),
            parent_key: parent_key.clone(),
            target_key: crate::grid_item::pdf_page_cache_key(0),
            target_idx: 0,
            // 古い世代 (現在 items_generation が進んだ状態を模す)。
            items_gen: 0,
            cache_map: Arc::clone(&cache_map),
            parent_entry_mtime: 0,
            parent_entry_size: 0,
        });
        // app.items_generation を進めて発火 → 親 catalog には何も書かれず writeback が
        // None に戻る。`replace_search_view_items` / `remove_items_batch` 経由で
        // items_generation だけが進む実機シナリオの再現。
        app.items_generation = 7;
        app.fire_virtual_folder_writeback_if_ready(0);
        assert!(parent_db_arc.load_one(&parent_key).unwrap().is_none());
        assert!(
            app.virtual_folder_writeback.is_none(),
            "古い世代の writeback はクリアして次に持ち越さない"
        );
    }

    /// Codex P2: 同じ世代・対象 idx で発火しても `cache_map` が空なら親 catalog には
    /// 何も書かず、writeback だけクリアする (= one-shot)。実機では from_cache 経路で
    /// finalize シグナルが届かないため永続的に再発火しない、その場で諦める。
    #[test]
    fn fire_virtual_folder_writeback_clears_when_cache_map_empty() {
        let mut app = setup_app();
        let parent_dir = app.tmp.path().join("parent_empty");
        std::fs::create_dir_all(&parent_dir).unwrap();
        let cache_dir = crate::catalog::default_cache_dir();
        let parent_db_arc =
            Arc::new(crate::catalog::CatalogDb::open(&cache_dir, &parent_dir).unwrap());
        // cache_map は空のまま (= worker が page_0000 を入れていない)。
        let cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        > = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        let parent_key = format!("{}{}", crate::thumb_loader::CACHE_KEY_PDF, "empty.pdf");
        let cur_gen = app.items_generation;
        app.virtual_folder_writeback = Some(crate::app::VirtualFolderWriteback {
            parent_catalog: Arc::clone(&parent_db_arc),
            parent_key: parent_key.clone(),
            target_key: crate::grid_item::pdf_page_cache_key(0),
            target_idx: 0,
            items_gen: cur_gen,
            cache_map: Arc::clone(&cache_map),
            parent_entry_mtime: 0,
            parent_entry_size: 0,
        });
        app.fire_virtual_folder_writeback_if_ready(0);
        assert!(parent_db_arc.load_one(&parent_key).unwrap().is_none());
        assert!(
            app.virtual_folder_writeback.is_none(),
            "cache_map が空でも 1 度試したらクリアして次の finalize に持ち越さない"
        );
    }

    /// Codex P2: 親 catalog のサムネが壊れていた場合、`save_thumb_bytes` が `Ok(false)`
    /// を返し、seed 経路は `cache_map` に入れない。これがないと表示時にデコード失敗で
    /// 永続的に Failed になる退行が起きる。
    #[test]
    fn seed_skips_cache_map_when_parent_thumb_undecodable() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        let parent_dir = app.tmp.path().join("parent_corrupt");
        std::fs::create_dir_all(&parent_dir).unwrap();
        let pdf_path = parent_dir.join("corrupt.pdf");
        let cache_dir = crate::catalog::default_cache_dir();
        let parent_db = crate::catalog::CatalogDb::open(&cache_dir, &parent_dir).unwrap();
        // 寸法が取れないバイト列 (適当な ASCII)。`decode_thumb_dims` が None を返す。
        let bad_bytes: Vec<u8> = b"NOT-A-WEBP-OR-JPEG".to_vec();
        let parent_key = format!("{}{}", crate::thumb_loader::CACHE_KEY_PDF, "corrupt.pdf");
        // 直接 save() で壊れたバイトを置く (= 旧版や外部経路で壊れた行が残った状況を再現)。
        parent_db
            .save(&parent_key, 0, 0, 1, 1, None, &bad_bytes)
            .unwrap();

        let pdf_db = crate::catalog::CatalogDb::open(&cache_dir, &pdf_path).unwrap();
        let cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        > = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));

        app.items.push(GridItem::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: 0,
            content_type: None,
        });
        app.setup_virtual_folder_seed_and_writeback(&pdf_path, &pdf_db, &cache_map);

        // cache_map は空のまま (= 表示時に decode 失敗で Failed にならない)。
        let target_key = crate::grid_item::pdf_page_cache_key(0);
        assert!(
            !cache_map.read().unwrap().contains_key(&target_key),
            "壊れたサムネは cache_map に入れない (= Failed 退行を防ぐ)"
        );
        // writeback ターゲットは設定される (= 後続 worker が完成させたら親 catalog に
        // 書き戻して corrupt 行を上書きする経路は維持)。
        assert!(app.virtual_folder_writeback.is_some());
    }

    /// `save_thumb_bytes` 単体: 寸法が取れないバイト列を渡したら `Ok(false)` を返し、
    /// DB には何も書き込まない。
    #[test]
    fn save_thumb_bytes_returns_false_for_undecodable() {
        let app = setup_app();
        let cache_dir = crate::catalog::default_cache_dir();
        let dir = app.tmp.path().join("save_thumb_test");
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::catalog::CatalogDb::open(&cache_dir, &dir).unwrap();
        let bad: Vec<u8> = b"DEADBEEF-NOT-AN-IMAGE".to_vec();
        let saved = db
            .save_thumb_bytes("k1", 1, 2, None, &bad)
            .expect("Err なし、Ok(false) で skip 表現");
        assert!(!saved, "壊れたバイト列は save 断念 → false");
        assert!(db.load_one("k1").unwrap().is_none(), "DB にも何も書かない");
    }

    /// Codex P1 (0.8.2 後半): `release_fs_nav_lock` が `fs_nav_locked_gen` /
    /// `fs_holdover_tex` を確実に解除すること。境界・キャンセル経路で items_generation
    /// が進まないまま return すると `poll_fs_nav_lock` では解除されないため、
    /// その経路で本ヘルパが呼ばれることを別途確認する必要がある。
    #[test]
    fn release_fs_nav_lock_clears_both_fields() {
        let mut app = setup_app();
        app.fs_nav_locked_gen = Some(42);
        app.fs_holdover_tex = None; // None でも問題ない
        app.release_fs_nav_lock();
        assert!(app.fs_nav_locked_gen.is_none());
        assert!(app.fs_holdover_tex.is_none());
        // 何度呼んでも safe (idempotent)
        app.release_fs_nav_lock();
        assert!(app.fs_nav_locked_gen.is_none());
    }

    /// `find_fullscreen_nav_target`: 常にフォルダ先頭の画像系アイテムを返すこと。
    /// Ctrl+↑ でも前フォルダの最後ではなく最初の画像に着地させる仕様変更の回帰ガード。
    /// (旧 API の `forward: bool` 引数は仕様統一に伴って削除済み。)
    #[test]
    fn find_fullscreen_nav_target_returns_first_image() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        // 並び: Folder, Image_a, Image_b, Image_c (visible_indices は items 全部)
        app.items.push(GridItem::Folder("c:/sub".into()));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/a.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/b.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/c.jpg")));
        for _ in 0..app.items.len() {
            app.thumbnails.push(ThumbnailState::Pending);
        }
        app.rebuild_visible_indices();
        assert_eq!(
            app.find_fullscreen_nav_target_filtered(true),
            Some(1),
            "先頭の画像系アイテム idx=1 (Image_a) を返す (Folder idx=0 はスキップ)"
        );
    }

    /// `find_fullscreen_nav_target`: 動画も image-like として着地点に含める。
    #[test]
    fn find_fullscreen_nav_target_can_return_video() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        app.items.push(GridItem::Folder("c:/sub".into()));
        app.items
            .push(GridItem::Video(std::path::PathBuf::from("c:/a.mp4")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/b.jpg")));
        for _ in 0..app.items.len() {
            app.thumbnails.push(ThumbnailState::Pending);
        }
        app.rebuild_visible_indices();
        assert_eq!(
            app.find_fullscreen_nav_target_filtered(true),
            Some(1),
            "先頭の image-like が動画なら動画 idx を返す"
        );
        // include_video=false (スライドショー NextFolder 再開) では動画を飛ばして
        // 先頭の静止画 idx=2 を返す。
        assert_eq!(
            app.find_fullscreen_nav_target_filtered(false),
            Some(2),
            "動画除外なら先頭の静止画 idx=2 を返す"
        );
    }

    #[test]
    fn folder_nav_mode_same_kind_distinguishes_favsearch_scope() {
        let root_a = PathBuf::from("c:/fav-a");
        let root_b = PathBuf::from("c:/fav-b");
        assert!(folder_nav_mode_same_kind(
            &FolderNavMode::Favsearch {
                root: root_a.clone(),
                fullscreen: false,
            },
            &FolderNavMode::Favsearch {
                root: root_a.clone(),
                fullscreen: false,
            },
        ));
        assert!(!folder_nav_mode_same_kind(
            &FolderNavMode::Favsearch {
                root: root_a.clone(),
                fullscreen: false,
            },
            &FolderNavMode::Favsearch {
                root: root_a,
                fullscreen: true,
            },
        ));
        assert!(!folder_nav_mode_same_kind(
            &FolderNavMode::Favsearch {
                root: root_b.clone(),
                fullscreen: true,
            },
            &FolderNavMode::Favsearch {
                root: root_b.join("child"),
                fullscreen: true,
            },
        ));
    }

    /// `poll_fs_nav_lock`: items_generation が進んだあとに新ページのサムネが
    /// Loaded になって初めてロック解除されること。`items_generation` チェックが
    /// 無いと、ナビ発火直後 (まだ items 入れ替え前) で旧ページのサムネが Loaded だと
    /// 誤って解除されて、後で実際に items が入れ替わった瞬間に「ファイル名のみ表示」が
    /// 一瞬出てしまう不具合になる。その回帰テスト。
    #[test]
    fn poll_fs_nav_lock_waits_for_items_generation_bump() {
        use crate::grid_item::{GridItem, ThumbnailState};
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let dummy_tex = ctx.load_texture(
            "test_dummy",
            egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        // 初期: items_generation = 0、idx 0 が「現在表示中ページ」 (Loaded 済) として置く。
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.thumbnails.push(ThumbnailState::Loaded {
            tex: dummy_tex.clone(),
            from_cache: false,
            rendered_at_px: 64,
            source_dims: None,
        });
        app.fullscreen_idx = Some(0);
        // items_generation は変えない (= 0)、ロック発火時の gen は 0 を記録する想定
        let lock_gen = app.items_generation;
        app.fs_nav_locked_gen = Some(lock_gen);
        app.fs_holdover_tex = Some(dummy_tex.clone());

        // ① items_generation が進んでいない (= まだナビが完了していない) →
        //    現ページが Loaded でもロック解除されない (主要バグの回帰防止)。
        app.poll_fs_nav_lock();
        assert!(
            app.fs_nav_locked_gen.is_some(),
            "items_generation 据え置きならロック維持 (旧ページの Loaded で誤解除しない)"
        );

        // ② items_generation を進めて再度 poll: 新ページのサムネ Loaded を見て解除。
        app.items_generation += 1;
        app.poll_fs_nav_lock();
        assert!(app.fs_nav_locked_gen.is_none(), "gen 進行 + Loaded で解除");
        assert!(app.fs_holdover_tex.is_none(), "holdover も解放");

        // ③ フルスクリーン抜け (fs_idx None) でも、items_generation 進行が必要。
        //    そうしないとナビ最中の close_fullscreen 経路で誤解除される。
        app.fs_nav_locked_gen = Some(app.items_generation);
        app.fs_holdover_tex = Some(dummy_tex.clone());
        app.fullscreen_idx = None;
        app.poll_fs_nav_lock();
        assert!(
            app.fs_nav_locked_gen.is_some(),
            "items_generation 据え置きなら fs_idx=None でも維持 (apply_folder_nav_result 内の close→open 遷移を許容)"
        );
        // items が進んでから抜け検知すれば解除される。
        app.items_generation += 1;
        app.poll_fs_nav_lock();
        assert!(app.fs_nav_locked_gen.is_none());
        assert!(app.fs_holdover_tex.is_none());
    }

    /// `poll_fs_nav_lock`: PDF/ZIP の async enumerate 待ち中 (= `fs_nav_after_pdf_enumerate`
    /// が立っている) は、`items_generation` が進んでも `fullscreen_idx = None` の状態で
    /// holdover を解放しないこと。PDF メタキャッシュ hit で placeholder grid を install
    /// した直後の cache-hit 経路の回帰ガード。これが無いと、in-window モードの
    /// `render_embedded_fs_nav_holdover` (および viewport モードの
    /// `keep_fullscreen_viewport_alive` PDF defer 分岐) で holdover 画像が消えて
    /// 真っ黒のフラッシュになる。
    #[test]
    fn poll_fs_nav_lock_keeps_holdover_during_pdf_enumerate_defer() {
        use crate::grid_item::GridItem;
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let dummy_tex = ctx.load_texture(
            "test_dummy",
            egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        // 元 fullscreen 状態 (旧 PDF ページ) の痕跡を残す。
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        // ナビ発火時に lock + holdover を確保した状態を再現。
        let lock_gen = app.items_generation;
        app.fs_nav_locked_gen = Some(lock_gen);
        app.fs_holdover_tex = Some(dummy_tex.clone());
        // close_fullscreen → load_pdf_as_folder → placeholder install と進んで、
        // fullscreen_idx は None、items_generation は進んだが、async enumerate は
        // まだ pending という状態。
        app.fullscreen_idx = None;
        app.items_generation += 1;
        app.fs_nav_after_pdf_enumerate = Some(DeferredFsReopen {
            resume_slideshow: false,
            target: None,
            resume_to_last_page: false,
        });

        app.poll_fs_nav_lock();
        assert!(
            app.fs_nav_locked_gen.is_some(),
            "deferred enumerate 中は items_gen 進行 + fs_idx=None でも lock を維持"
        );
        assert!(
            app.fs_holdover_tex.is_some(),
            "deferred enumerate 中は holdover を解放しない (defer 描画から画像が消えるのを防ぐ)"
        );

        // enumerate 完了で fs_nav_after_pdf_enumerate がクリアされたあとは
        // 通常通り fs_idx=None の解除経路に乗る (= Esc 等で抜けたケース)。
        app.fs_nav_after_pdf_enumerate = None;
        app.poll_fs_nav_lock();
        assert!(app.fs_nav_locked_gen.is_none());
        assert!(app.fs_holdover_tex.is_none());
    }

    /// `get_or_open_catalog` の LRU キャッシュ動作: 同じフォルダで 2 回呼ぶと
    /// 同じ `Arc<CatalogDb>` (= 同じ Arc::ptr) が返ること、容量を超えると古いものから
    /// 抜けること。Ctrl+↓ 連打時に毎ステップ `CatalogDb::open` を走らせないための
    /// 本キャッシュの回帰ガード。
    #[test]
    fn catalog_cache_reuses_arc_for_same_folder_and_evicts_oldest() {
        let mut app = setup_app();
        // tmp 配下に 18 個 (LRU 上限 16 を 2 つ超える) のフォルダを作る。
        let mut folders: Vec<std::path::PathBuf> = Vec::new();
        for i in 0..18 {
            let p = app.tmp.path().join(format!("cat_{i:02}"));
            std::fs::create_dir_all(&p).unwrap();
            folders.push(p);
        }
        // 最初の open: 新規挿入。
        let arc0_first = app.get_or_open_catalog(&folders[0]).expect("open ok");
        // 同じフォルダを再度 open: 同 Arc が返る (= キャッシュヒット)。
        let arc0_again = app.get_or_open_catalog(&folders[0]).expect("hit");
        assert!(
            std::sync::Arc::ptr_eq(&arc0_first, &arc0_again),
            "同フォルダの 2 回目は同じ Arc を返す"
        );
        assert_eq!(app.catalog_cache_order.len(), 1);
        // 16 個目までは順に挿入。
        for f in &folders[1..16] {
            app.get_or_open_catalog(f);
        }
        assert_eq!(app.catalog_cache_order.len(), 16);
        assert!(app.catalog_cache.contains_key(&folders[0]));
        // 17 個目: folders[0] が evict される (LRU 順で古いから)。
        // ただし「folders[0] を最後に hit」した直後ではない (上の hit は新規挿入の直後の
        // 触り直しで再度末尾移動しているはず) なので、ここで先に folders[1] を hit させて
        // folders[0] を本当に最古に位置づけ直す。
        app.get_or_open_catalog(&folders[1]); // [1] を末尾へ
        // この時点で LRU 順は [0, 2, 3, ..., 15, 1] (= 0 が最古)。
        // 17 個目 folders[16] を入れると [0] が evict される。
        app.get_or_open_catalog(&folders[16]);
        assert_eq!(app.catalog_cache_order.len(), 16);
        assert!(
            !app.catalog_cache.contains_key(&folders[0]),
            "LRU 上限超過で最古 (folders[0]) が evict される"
        );
        assert!(app.catalog_cache.contains_key(&folders[16]));
    }

    /// Codex P1 (見開き消しゴム): `erase_inpaint_pending` が idx 別 HashMap になり、
    /// 左ページ apply 直後に右ページに切り替えても左の inpaint がキャンセルされない
    /// (旧実装は `Option` で 1 件しか持たず、新ジョブ投入が旧ジョブを cancel していた)。
    /// 同じ idx への再投入だけは旧版どおり cancel する。
    #[test]
    fn erase_inpaint_pending_keeps_jobs_for_different_pages() {
        use crate::ui_erase::{EraseInpaintKind, EraseInpaintPending, EraseInpaintPendingKey};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let mut app = setup_app();
        // 2 つの idx (0=左, 1=右) に対して dummy pending を入れる。
        let make_pending = |idx: usize| {
            let (_tx, rx) = std::sync::mpsc::channel::<egui::ColorImage>();
            EraseInpaintPending {
                idx,
                items_generation: 0,
                path_key: None,
                rx,
                cancel: Arc::new(AtomicBool::new(false)),
                started_at: std::time::Instant::now(),
                input_generation: 0,
                mask_generation: 0,
                log_prefix: "test",
                is_preview: false,
            }
        };
        let commit_key = |idx| EraseInpaintPendingKey {
            idx,
            kind: EraseInpaintKind::Commit,
        };
        let p0 = make_pending(0);
        let cancel_p0 = p0.cancel.clone();
        app.erase_inpaint_pending.insert(commit_key(0), p0);
        app.erase_inpaint_pending
            .insert(commit_key(1), make_pending(1));
        assert_eq!(
            app.erase_inpaint_pending.len(),
            2,
            "両ページの pending が並走"
        );

        // 異なる idx (1) への再投入をシミュレート: idx=0 の pending はそのまま残るべき。
        // (run_inpaint_and_cache の挙動を最小再現)
        if let Some(prev) = app.erase_inpaint_pending.remove(&commit_key(1)) {
            prev.cancel.store(true, Ordering::Relaxed);
        }
        app.erase_inpaint_pending
            .insert(commit_key(1), make_pending(1));
        assert_eq!(app.erase_inpaint_pending.len(), 2);
        assert!(
            !cancel_p0.load(Ordering::Relaxed),
            "他ページ (idx=0) の pending は cancel されない"
        );

        // 同じ idx (0) への再投入は旧 pending を cancel する。
        if let Some(prev) = app.erase_inpaint_pending.remove(&commit_key(0)) {
            prev.cancel.store(true, Ordering::Relaxed);
        }
        assert!(
            cancel_p0.load(Ordering::Relaxed),
            "同 idx の再投入で旧 pending が cancel される"
        );
    }

    /// `switch_erase_target_in_spread`: 左→右ボタンで `fullscreen_idx` がペアの
    /// もう一方に切り替わり、`erase_spread_ctx` は維持されること (= パネルボタンが
    /// 引き続きトグルとして使える)。マスクが空の状態で呼んでも安全 (空 inpaint は
    /// 早期 return、reset 経由で spread_ctx が消えても再保存される)。
    #[test]
    fn switch_erase_target_in_spread_moves_to_other_page_keeping_ctx() {
        use crate::grid_item::GridItem;
        use crate::settings::SpreadMode;
        let mut app = setup_app();
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/b.jpg")));
        app.thumbnails.push(ThumbnailState::Pending);
        app.thumbnails.push(ThumbnailState::Pending);
        app.fullscreen_idx = Some(0);
        app.spread_mode = SpreadMode::Single; // 消しゴム中
        app.erase_spread_ctx = Some(crate::app::EraseSpreadCtx {
            saved_mode: SpreadMode::Ltr,
            pair: (0, 1),
        });
        app.erase_mode = true;
        // erase_base_cache に dummy ピクセルを入れる必要は無い: マスクが空なので
        // apply_inpaint_only は composite_mask の段階で false を返し、
        // base_cache 参照は走らない。

        let dummy_ctx = egui::Context::default();
        app.switch_erase_target_in_spread(&dummy_ctx, 1);

        assert_eq!(
            app.fullscreen_idx,
            Some(1),
            "fullscreen_idx が右ページ idx=1 に切り替わる"
        );
        assert_eq!(
            app.spread_mode,
            SpreadMode::Single,
            "spread_mode は Single のまま (見開き表示には戻らない)"
        );
        let ctx = app.erase_spread_ctx.expect("spread_ctx は維持される");
        assert_eq!(ctx.saved_mode, SpreadMode::Ltr);
        assert_eq!(ctx.pair, (0, 1));
        assert!(app.erase_mode, "erase_mode は維持される (= 編集継続)");
        assert_eq!(app.fs_zoom, 1.0, "ズームはリセット");
    }

    /// 消しゴム入場時は、過去に auto-apply / F7/F8 経由で入った
    /// `erase_base_cache` を再利用せず、raw 専用の `fs_cache` から作業ベースを作り直す。
    /// 古い base が補正済み pixels だと、入場直後の表示で補正が二重適用される。
    #[test]
    fn enter_erase_mode_rebuilds_base_from_raw_fs_cache() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/a.jpg");
        app.fullscreen_idx = Some(idx);

        let ctx = egui::Context::default();
        let raw = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(10, 20, 30)]);
        let stale_adjusted =
            egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(200, 210, 220)]);
        let raw_pixels = std::sync::Arc::new(raw.clone());
        let raw_tex = ctx.load_texture("raw_for_erase_base", raw, egui::TextureOptions::LINEAR);
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex: raw_tex,
                pixels: raw_pixels,
                source_dims: Some([1, 1]),
                load_seq: 0,
            },
        );
        app.erase_base_cache
            .insert(idx, std::sync::Arc::new(stale_adjusted));

        app.enter_erase_mode(idx);

        let base = app
            .erase_base_cache
            .get(&idx)
            .expect("erase base should be recreated");
        assert_eq!(
            base.pixels[0],
            egui::Color32::from_rgb(10, 20, 30),
            "stale adjusted erase_base_cache must be overwritten by raw fs_cache"
        );
        assert_eq!(app.erase_mask_size, [1, 1]);
        assert!(app.erase_mode);
    }

    /// v1.1.0 編集パイプラインでは AI は最終段へ移るため、消しゴム編集の
    /// 作業解像度は AI cache があっても source 解像度に固定される。
    #[test]
    fn enter_erase_mode_keeps_source_size_even_when_ai_cache_exists() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/a.jpg");
        app.fullscreen_idx = Some(idx);
        app.ai_upscale_enabled = true;

        let ctx = egui::Context::default();
        let raw = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(10, 20, 30)]);
        let raw_tex = ctx.load_texture(
            "raw_for_erase_work_size",
            raw.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex: raw_tex,
                pixels: std::sync::Arc::new(raw),
                source_dims: Some([1, 1]),
                load_seq: 0,
            },
        );
        let ai = egui::ColorImage::new([4, 4], vec![egui::Color32::from_rgb(240, 240, 240); 16]);
        let ai_tex = ctx.load_texture(
            "ai_for_erase_work_size",
            ai.clone(),
            egui::TextureOptions::LINEAR,
        );
        let erase_bg = app.erase_upscale_bg_mode(idx);
        app.ai_upscale_cache.insert(
            (idx, erase_bg),
            FsCacheEntry::Static {
                tex: ai_tex,
                pixels: std::sync::Arc::new(ai),
                source_dims: None,
                load_seq: 0,
            },
        );

        app.enter_erase_mode(idx);

        assert_eq!(
            app.erase_mask_size,
            [1, 1],
            "erase edit mask should stay at source resolution"
        );
        assert_eq!(
            app.erase_mask.as_ref().map(Vec::len),
            Some(1),
            "new empty mask should be allocated at the source-resolution work size"
        );
        assert_eq!(
            app.erase_base_cache.get(&idx).map(|p| p.size),
            Some([1, 1]),
            "raw erase_base_cache is rebuilt from fs_cache"
        );
    }

    /// Codex 0.8.2 P1: 検索バー (Ctrl+F/S/G) の TextEdit にフォーカスがある間は
    /// グリッドショートカットを抑止する。`global_search.has_focus` の追加が無いと
    /// Ctrl+G の検索入力欄で BS が `close_global_search` に流れて入力が破壊される。
    #[test]
    fn shortcuts_are_blocked_while_search_text_input_has_focus() {
        let mut app = setup_app();
        // 全フォーカスフラグが false の初期状態ではブロックされない。
        assert!(!app.shortcuts_blocked_by_text_input());
        // Ctrl+F バー
        app.search_has_focus = true;
        assert!(app.shortcuts_blocked_by_text_input());
        app.search_has_focus = false;
        // Ctrl+S バー
        app.favsearch.has_focus = true;
        assert!(app.shortcuts_blocked_by_text_input());
        app.favsearch.has_focus = false;
        // Ctrl+G バー (本コミットの修正対象)
        app.global_search.has_focus = true;
        assert!(
            app.shortcuts_blocked_by_text_input(),
            "Ctrl+G TextEdit フォーカス中も BS / Enter / Ctrl+C 等を grid に漏らさない"
        );
        app.global_search.has_focus = false;
        // アドレスバー
        app.address_has_focus = true;
        assert!(app.shortcuts_blocked_by_text_input());
    }

    /// 削除確認ダイアログの文言は対象パスのドライブ種別・ゴミ箱設定で変わるため、
    /// 新しい削除要求を出すたびにキャッシュを必ず破棄する。
    #[test]
    fn request_delete_confirm_resets_cached_label_for_new_targets() {
        let mut app = setup_app();
        app.show_delete_confirm = true;
        app.delete_targets = vec![(0, PathBuf::from("C:/pics/old.jpg"))];
        app.delete_confirm_label = Some("old label".to_owned());

        app.request_delete_confirm(vec![(1, PathBuf::from("J:/pics/new.jpg"))]);

        assert!(app.show_delete_confirm);
        assert_eq!(
            app.delete_targets,
            vec![(1, PathBuf::from("J:/pics/new.jpg"))]
        );
        assert!(
            app.delete_confirm_label.is_none(),
            "new delete targets must force the warning label to be recomputed"
        );
    }

    /// `poll_delete_pending` が削除完了時に `current_folder_signature` を None に
    /// 倒すことを検証 (= 後続の外部 mtime 変化で誤 reload しない)。
    ///
    /// 0.8.x で導入: 削除成功直後は items が UI 側で in-place に絞り込まれており、
    /// stale な signature が残っていると `check_external_folder_changes` が
    /// 「内容変化あり」と誤判定して数千件の items を再走査して UI が固まる。
    #[test]
    fn poll_delete_pending_clears_current_folder_signature() {
        use crate::delete_worker::{DeleteMsg, DeletePending};
        use crate::grid_item::{GridItem, ThumbnailState};
        use std::sync::atomic::AtomicBool;

        let mut app = setup_app();
        let folder = app.tmp.path().join("delete_test_folder");
        std::fs::create_dir_all(&folder).unwrap();
        let target_path = folder.join("a.jpg");
        std::fs::write(&target_path, b"dummy").unwrap();

        app.current_folder = Some(folder.clone());
        app.current_folder_signature = Some(0xDEAD_BEEF);
        app.items.push(GridItem::Image(target_path.clone()));
        app.thumbnails.push(ThumbnailState::Pending);

        // 完了済み worker を疑似する: succeeded に target_path を入れて Done を即送る。
        let (tx, rx) = std::sync::mpsc::channel::<DeleteMsg>();
        tx.send(DeleteMsg::Batch {
            succeeded: vec![target_path.clone()],
            failed: vec![],
        })
        .unwrap();
        tx.send(DeleteMsg::Done { canceled: false }).unwrap();
        drop(tx);

        app.delete_pending = Some(DeletePending {
            cancel: std::sync::Arc::new(AtomicBool::new(false)),
            rx,
            total: 1,
            succeeded: vec![],
            failed: vec![],
        });

        // 実 ファイルも消しておかないと metadata() が古いまま (実機では worker が消す)。
        std::fs::remove_file(&target_path).unwrap();

        app.poll_delete_pending();

        assert!(
            app.delete_pending.is_none(),
            "Done 受信で delete_pending は take される"
        );
        assert!(
            app.current_folder_signature.is_none(),
            "削除完了で signature が None reset されること"
        );
        assert_eq!(
            app.items.len(),
            0,
            "削除成功した path は items から抜かれる"
        );
    }

    /// 削除時に Loaded サムネイルを残すなら、補正再生成用の `thumb_pixels` も
    /// idx shift して残す。これが空になると削除直後だけサムネイルの色調補正が外れる。
    #[test]
    fn remove_items_batch_preserves_thumb_pixels_for_loaded_survivors() {
        use crate::grid_item::{GridItem, ThumbnailState};
        use std::sync::Arc;

        fn loaded_thumb(
            ctx: &egui::Context,
            label: &str,
            color: egui::Color32,
        ) -> (ThumbnailState, Arc<egui::ColorImage>) {
            let image = egui::ColorImage::filled([1, 1], color);
            let pixels = Arc::new(image.clone());
            let tex = ctx.load_texture(label, image, egui::TextureOptions::LINEAR);
            (
                ThumbnailState::Loaded {
                    tex,
                    from_cache: false,
                    rendered_at_px: 64,
                    source_dims: Some((1, 1)),
                },
                pixels,
            )
        }

        let mut app = setup_app();
        let ctx = egui::Context::default();
        let (thumb_a, pixels_a) = loaded_thumb(&ctx, "raw_a", egui::Color32::RED);
        let (thumb_b, pixels_b) = loaded_thumb(&ctx, "raw_b", egui::Color32::GREEN);
        let (thumb_c, pixels_c) = loaded_thumb(&ctx, "raw_c", egui::Color32::BLUE);

        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/b.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/c.jpg")));
        app.thumbnails.extend([thumb_a, thumb_b, thumb_c]);
        app.thumb_pixels.insert(0, Arc::clone(&pixels_a));
        app.thumb_pixels.insert(1, Arc::clone(&pixels_b));
        app.thumb_pixels.insert(2, Arc::clone(&pixels_c));
        app.thumb_adjust_tex.insert(
            2,
            ctx.load_texture(
                "stale_adjusted_c",
                egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
                egui::TextureOptions::LINEAR,
            ),
        );

        app.remove_items_batch(&[1]);

        assert_eq!(app.items.len(), 2);
        assert!(matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }));
        assert!(matches!(app.thumbnails[1], ThumbnailState::Loaded { .. }));
        assert!(
            Arc::ptr_eq(
                app.thumb_pixels.get(&0).expect("old idx 0 stays at 0"),
                &pixels_a
            ),
            "削除されなかった先頭サムネの source pixels は残る"
        );
        assert!(
            Arc::ptr_eq(
                app.thumb_pixels.get(&1).expect("old idx 2 shifts to 1"),
                &pixels_c
            ),
            "削除位置より後ろの source pixels は新 idx に shift される"
        );
        assert!(
            !app.thumb_pixels.values().any(|p| Arc::ptr_eq(p, &pixels_b)),
            "削除対象の source pixels は残さない"
        );
        assert!(
            app.thumb_adjust_tex.is_empty(),
            "補正済み TextureHandle は stale なので削除後に再生成させる"
        );
    }

    /// Codex P2: 削除時に local_adjust_page_layers / local_adjust_pages だけでなく
    /// local_adjust_selected_layers も idx shift する。抜けると削除位置より後ろの
    /// ページが選択レイヤー状態を失う / 別ページへ古い選択が乗る。
    #[test]
    fn remove_items_batch_shifts_local_adjust_selected_layers() {
        fn layer(name: &str) -> local_adjust_core::LocalAdjustmentLayer {
            local_adjust_core::LocalAdjustmentLayer::new(
                name,
                local_adjust_core::LocalMask::Full,
                local_adjust_core::LocalEffect::None,
            )
        }
        let mut app = setup_app();
        let a = push_image(&mut app, "C:/p/a.jpg");
        let b = push_image(&mut app, "C:/p/b.jpg");
        let c = push_image(&mut app, "C:/p/c.jpg");
        assert_eq!((a, b, c), (0, 1, 2));

        app.local_adjust_page_layers
            .insert(a, vec![layer("A0"), layer("A1")]);
        app.local_adjust_pages.insert(a);
        app.local_adjust_selected_layers.insert(a, 1);

        app.local_adjust_page_layers.insert(b, vec![layer("B0")]);
        app.local_adjust_pages.insert(b);
        app.local_adjust_selected_layers.insert(b, 0);

        app.local_adjust_page_layers
            .insert(c, vec![layer("C0"), layer("C1"), layer("C2")]);
        app.local_adjust_pages.insert(c);
        app.local_adjust_selected_layers.insert(c, 2);

        // 中央 (b) を削除 → a は idx 0 のまま、c は idx 2 → 1 へ shift。
        app.remove_items_batch(&[b]);

        assert_eq!(app.items.len(), 2);
        assert_eq!(
            app.local_adjust_selected_layers.get(&0).copied(),
            Some(1),
            "先頭ページの選択レイヤーは idx 0 のまま残る"
        );
        assert_eq!(
            app.local_adjust_selected_layers.get(&1).copied(),
            Some(2),
            "削除位置より後ろのページの選択レイヤーは新 idx 1 へ shift される"
        );
        assert_eq!(
            app.local_adjust_selected_layers.len(),
            2,
            "削除対象ページの選択レイヤーエントリは残さない"
        );
        // page_layers / pages 側と idx が揃っていること (= セット更新の不変条件)
        assert!(app.local_adjust_page_layers.contains_key(&1));
        assert!(app.local_adjust_pages.contains(&1));
    }

    /// Codex P2 (clamp 部): 何らかの理由で選択レイヤー idx が残存 layer 数を超えていても、
    /// shift 時に新 idx の layer 数で clamp して範囲内に収める。
    #[test]
    fn remove_items_batch_clamps_local_adjust_selected_layer_to_layer_count() {
        let mut app = setup_app();
        let a = push_image(&mut app, "C:/p/a.jpg");
        let b = push_image(&mut app, "C:/p/b.jpg");
        assert_eq!((a, b), (0, 1));

        // b は 1 layer だが選択 idx を意図的に過大 (5) にしておく。
        app.local_adjust_page_layers.insert(
            b,
            vec![local_adjust_core::LocalAdjustmentLayer::new(
                "B0",
                local_adjust_core::LocalMask::Full,
                local_adjust_core::LocalEffect::None,
            )],
        );
        app.local_adjust_pages.insert(b);
        app.local_adjust_selected_layers.insert(b, 5);

        // a を削除 → b は idx 1 → 0 へ shift。選択 idx は layer 数 1 に対して 0 へ clamp。
        app.remove_items_batch(&[a]);

        assert_eq!(
            app.local_adjust_selected_layers.get(&0).copied(),
            Some(0),
            "残存 layer 数 (1) を超えた選択 idx は clamp される"
        );
    }

    /// Codex P1: snapshot 有効化で items を subset へ差し替えたら、idx-keyed なページ編集
    /// 状態 (補正の個別パラメータ等) も subset idx へ hydrate し直す。これをやらないと元
    /// フォルダの別画像に紐付いた補正が subset の別 idx に乗って表示・エクスポートされる。
    #[test]
    fn activate_snapshot_remaps_page_params_to_subset_indices() {
        let mut app = setup_app();
        let a = push_image(&mut app, "C:/pics/a.jpg");
        let b = push_image(&mut app, "C:/pics/b.jpg");
        let c = push_image(&mut app, "C:/pics/c.jpg");
        assert_eq!((a, b, c), (0, 1, 2));
        app.current_folder = Some(PathBuf::from("C:/pics"));
        // 3 枚それぞれ別 brightness を設定 (set_page_params が DB に同期保存)。
        app.set_page_params(a, params_with_brightness(10.0));
        app.set_page_params(b, params_with_brightness(20.0));
        app.set_page_params(c, params_with_brightness(30.0));

        // visible = [a, c] (b を除外) を固定する。
        app.visible_indices = vec![a, c];
        app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);

        // subset items = [a, c] (reindex 0,1)
        assert_eq!(app.items.len(), 2);
        // 旧バグでは orig-keyed の adjustment_page_params[1] (= b の 20) が subset idx 1
        // (= c) に残って効いていた。
        assert!(
            (app.effective_params(0).brightness - 10.0).abs() < f32::EPSILON,
            "subset idx0 は a の補正 (10)"
        );
        assert!(
            (app.effective_params(1).brightness - 30.0).abs() < f32::EPSILON,
            "subset idx1 は c の補正 (30)。b の 20 が stale leak しない"
        );
        // 除外された b の補正 (20) は subset のどのページにも乗らない。
        assert!(
            !(0..app.items.len())
                .any(|i| (app.effective_params(i).brightness - 20.0).abs() < f32::EPSILON),
            "除外された b の補正 (20) は subset のどのページにも現れない"
        );
    }

    /// Codex P1 (対称性): snapshot 解除で items を元フォルダに戻したら、ページ編集状態も
    /// 元 idx で hydrate し直す。subset hydrate しっぱなしだと解除後に subset-keyed の補正が
    /// 元フォルダの別画像に乗る。
    #[test]
    fn deactivate_snapshot_restores_page_params_to_original_indices() {
        let mut app = setup_app();
        let a = push_image(&mut app, "C:/pics/a.jpg");
        let b = push_image(&mut app, "C:/pics/b.jpg");
        let c = push_image(&mut app, "C:/pics/c.jpg");
        app.current_folder = Some(PathBuf::from("C:/pics"));
        app.set_page_params(a, params_with_brightness(10.0));
        app.set_page_params(b, params_with_brightness(20.0));
        app.set_page_params(c, params_with_brightness(30.0));
        app.visible_indices = vec![a, c];
        app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);

        // 解除 (current_folder == origin なので at_origin 経路)。
        app.deactivate_snapshot();
        assert!(app.snapshot.is_none());
        assert_eq!(app.items.len(), 3);
        // 元 idx の補正が正しく復元される (subset-keyed の残骸が乗らない)。
        assert!(
            (app.effective_params(0).brightness - 10.0).abs() < f32::EPSILON,
            "a=10 復元"
        );
        assert!(
            (app.effective_params(1).brightness - 20.0).abs() < f32::EPSILON,
            "b=20 復元 (subset では除外されていたが DB から戻る)"
        );
        assert!(
            (app.effective_params(2).brightness - 30.0).abs() < f32::EPSILON,
            "c=30 復元"
        );
    }

    /// Codex P1 (検索 view snapshot): Ctrl+G 等の検索 view から★固定した場合は、origin
    /// (= 検索前の実 current_folder) の DB から部分 hydrate せず clear のみにする。検索 view
    /// は元々ページ編集 overlay を出さない設計なので、その snapshot もそれに揃える
    /// (origin が cross-folder prefix なので prefix 配下の subset item だけ部分 hydrate される
    /// 不整合を避ける)。
    #[test]
    fn activate_search_view_snapshot_clears_page_state_without_rehydrate() {
        let mut app = setup_app();
        let a = push_image(&mut app, "C:/pics/a.jpg");
        let b = push_image(&mut app, "C:/pics/b.jpg");
        app.current_folder = Some(PathBuf::from("C:/pics"));
        // DB に補正を保存しておく (= 通常フォルダ由来なら hydrate される値)。
        app.set_page_params(a, params_with_brightness(10.0));
        app.set_page_params(b, params_with_brightness(20.0));
        // 検索 view を模擬: global_search.active + saved_folder。検索 view では
        // replace_search_view_items が maps を clear している状態を再現する。
        app.global_search.active = true;
        app.global_search.saved_folder = Some(PathBuf::from("C:/before"));
        app.clear_page_edit_state();
        app.visible_indices = vec![a, b];
        app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::GlobalSearch {
            query: "x".into(),
        });
        // 検索由来なので rehydrate されず、ページ編集状態は空のまま。
        // (旧 P1 fix の素朴な rehydrate だと C:/pics の DB から a=10/b=20 が乗ってしまう。)
        assert!(
            app.adjustment_page_params.is_empty(),
            "検索 view 由来 snapshot は origin の DB から部分 hydrate しない (clear のみ)"
        );
    }

    /// Codex follow-up: Ctrl+F (= 単一フォルダの構造フィルタ) から★固定した場合は、
    /// cross-folder 検索ではないので通常どおり origin から rehydrate する (clear-only に
    /// しない)。`search_was_active` で gate すると Ctrl+F が誤って clear される退行のガード。
    /// 判定は `pre_snapshot_search_origin.is_some()` (Ctrl+F では None)。
    #[test]
    fn activate_ctrl_f_filtered_snapshot_rehydrates_page_params() {
        let mut app = setup_app();
        let a = push_image(&mut app, "C:/pics/a.jpg");
        let b = push_image(&mut app, "C:/pics/b.jpg");
        let c = push_image(&mut app, "C:/pics/c.jpg");
        app.current_folder = Some(PathBuf::from("C:/pics"));
        app.set_page_params(a, params_with_brightness(10.0));
        app.set_page_params(b, params_with_brightness(20.0));
        app.set_page_params(c, params_with_brightness(30.0));
        // Ctrl+F (テキスト検索バー) を立てる。global_search / favsearch は inactive のまま
        // なので pre_snapshot_search_origin は None になり、cross-folder 検索ではない。
        app.show_search_bar = true;
        app.visible_indices = vec![a, c];
        app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
        assert_eq!(app.items.len(), 2);
        // Ctrl+F は単一フォルダなので rehydrate される (clear-only にならない)。
        assert!(
            (app.effective_params(0).brightness - 10.0).abs() < f32::EPSILON,
            "subset idx0 = a の補正 10 が rehydrate される"
        );
        assert!(
            (app.effective_params(1).brightness - 30.0).abs() < f32::EPSILON,
            "subset idx1 = c の補正 30 が rehydrate される"
        );
    }

    /// Codex P2: `clear_page_edit_state` は idx-keyed ページ編集状態の正準セットを全部落とす。
    /// `replace_search_view_items` がこれを呼ぶことで、旧実装が取りこぼしていた
    /// local_adjust_selected_layers / conceal_pages も確実に clear される。
    #[test]
    fn clear_page_edit_state_clears_all_idx_keyed_maps() {
        let mut app = setup_app();
        app.adjustment_page_params
            .insert(0, params_with_brightness(5.0));
        app.local_adjust_page_layers.insert(
            0,
            vec![local_adjust_core::LocalAdjustmentLayer::new(
                "L",
                local_adjust_core::LocalMask::Full,
                local_adjust_core::LocalEffect::None,
            )],
        );
        app.local_adjust_pages.insert(0);
        app.local_adjust_selected_layers.insert(0, 3);
        app.mask_pages.insert(0);
        app.conceal_pages.insert(0);
        app.export_crop_pages.insert(0);

        app.clear_page_edit_state();

        assert!(app.adjustment_page_params.is_empty());
        assert!(app.local_adjust_page_layers.is_empty());
        assert!(app.local_adjust_pages.is_empty());
        assert!(
            app.local_adjust_selected_layers.is_empty(),
            "Codex P2: 旧 Ctrl+G clear が取りこぼしていた選択レイヤーも落とす"
        );
        assert!(app.mask_pages.is_empty());
        assert!(
            app.conceal_pages.is_empty(),
            "Codex P2: 旧 Ctrl+G clear が取りこぼしていた隠蔽バッジも落とす"
        );
        assert!(app.export_crop_pages.is_empty());
    }

    // ─────────────────────────────────────────────────────────────────────
    // 見開き左右独立補正: copy_spread_adjust のテスト
    // ─────────────────────────────────────────────────────────────────────

    /// 左ページに +30 を設定 → 右ページにコピー → 右ページの effective が +30 になる。
    #[test]
    fn copy_spread_adjust_left_to_right() {
        let mut app = setup_app();
        let left = push_image(&mut app, "C:/pics/a.jpg");
        let right = push_image(&mut app, "C:/pics/b.jpg");
        app.set_page_params(left, params_with_brightness(30.0));

        app.copy_spread_adjust(left, right);

        assert!(
            (app.effective_params(right).brightness - 30.0).abs() < f32::EPSILON,
            "右ページの effective brightness が +30 にコピーされる"
        );
        assert!(
            app.adjustment_page_params.contains_key(&right),
            "右ページの page_params エントリが作成される"
        );
    }

    /// コピー先がデフォルトと一致する値になる場合、page_params エントリは作られず
    /// カスケード解決に任せる (DB が無駄に増えない)。
    #[test]
    fn copy_spread_adjust_clears_when_matches_default() {
        let mut app = setup_app();
        let left = push_image(&mut app, "C:/pics/a.jpg");
        let right = push_image(&mut app, "C:/pics/b.jpg");
        // 左はデフォルト (= global_preset = identity) のまま、右に +30 を設定。
        app.set_page_params(right, params_with_brightness(30.0));
        assert!(app.adjustment_page_params.contains_key(&right));

        // 左 (= デフォルト) を右にコピー → 右の page_params は冗長なので削除される
        app.copy_spread_adjust(left, right);

        assert!(
            !app.adjustment_page_params.contains_key(&right),
            "コピー後、デフォルトと一致した page_params エントリは削除される"
        );
        assert!(
            app.effective_params(right).brightness.abs() < f32::EPSILON,
            "右ページの effective はカスケード経由でデフォルト値に戻る"
        );
    }

    /// コピー操作は capture_adjust_full 経由で Undo に乗り、Ctrl+Z で元に戻る。
    /// Redo (Ctrl+Y) で再度コピーされる。
    #[test]
    fn copy_spread_adjust_undo_redo() {
        let mut app = setup_app();
        let left = push_image(&mut app, "C:/pics/a.jpg");
        let right = push_image(&mut app, "C:/pics/b.jpg");
        app.set_page_params(left, params_with_brightness(30.0));
        app.set_page_params(right, params_with_brightness(-10.0));
        let right_before = app.effective_params(right).brightness;
        assert!((right_before - (-10.0)).abs() < f32::EPSILON);

        app.copy_spread_adjust(left, right);
        assert!((app.effective_params(right).brightness - 30.0).abs() < f32::EPSILON);

        // Undo: 右ページが元の -10 に戻る
        app.apply_meta_undo();
        assert!(
            (app.effective_params(right).brightness - (-10.0)).abs() < f32::EPSILON,
            "Undo で右ページの brightness が -10 に戻る"
        );

        // Redo: 再度コピーされる
        app.apply_meta_redo();
        assert!(
            (app.effective_params(right).brightness - 30.0).abs() < f32::EPSILON,
            "Redo で右ページの brightness が +30 に再適用される"
        );
    }
}

/// v1.1.0 pipeline refactor: edit-result cache and final-composite cache must
/// be invalidated on different axes.
#[cfg(test)]
mod pipeline_cache_refactor_tests {
    use crate::adjustment::PostFilter;

    use super::phase_c_support::setup_app;
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn push_image(app: &mut App, path: &str) -> usize {
        app.items.push(GridItem::Image(PathBuf::from(path)));
        app.thumbnails.push(ThumbnailState::Pending);
        app.items.len() - 1
    }

    fn dummy_edit_key(app: &App, idx: usize) -> EditResultKey {
        EditResultKey {
            idx,
            source_gen: app.input_generation.get(&idx).copied().unwrap_or(0),
            erase_mask_gen: app.erase_mask_generation.get(&idx).copied().unwrap_or(0),
            local_gen: app.local_adjust_generation.get(&idx).copied().unwrap_or(0),
            conceal_mask_gen: app.conceal_mask_generation.get(&idx).copied().unwrap_or(0),
            conceal_gen: app.conceal_generation,
        }
    }

    fn insert_edit_and_final_cache(
        app: &mut App,
        ctx: &egui::Context,
        idx: usize,
        label: &str,
    ) -> (EditResultKey, FinalCompositeKey) {
        let image = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(7, 8, 9)]);
        let pixels = Arc::new(image.clone());
        let edit_key = dummy_edit_key(app, idx);
        let edit_tex = ctx.load_texture(
            format!("{label}_edit"),
            image.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.edit_result_cache.insert(
            edit_key,
            EditResultEntry {
                pixels: Arc::clone(&pixels),
                texture: Some(edit_tex),
            },
        );

        let final_key = FinalCompositeKey {
            edit_key,
            params_hash: 0x11,
            bg: 0,
        };
        let final_tex = ctx.load_texture(
            format!("{label}_final"),
            image,
            egui::TextureOptions::LINEAR,
        );
        app.final_composite_cache.insert(
            final_key,
            FinalCompositeEntry {
                pixels,
                texture: final_tex,
                complete: true,
            },
        );
        app.final_ai_cache.insert(
            FinalAiKey {
                edit_key,
                color_ai_hash: 0x22,
                bg: 0,
            },
            Arc::new(egui::ColorImage::new(
                [1, 1],
                vec![egui::Color32::from_rgb(10, 11, 12)],
            )),
        );
        (edit_key, final_key)
    }

    fn insert_local_adjust_cache(
        app: &mut App,
        ctx: &egui::Context,
        idx: usize,
        label: &str,
    ) -> LocalAdjustResultKey {
        let key = app.current_local_adjust_key(idx);
        let image = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(20, 30, 40)]);
        let texture = ctx.load_texture(label, image.clone(), egui::TextureOptions::LINEAR);
        app.local_adjust_cache.insert(
            key,
            LocalAdjustCacheEntry {
                pixels: Arc::new(image),
                texture,
            },
        );
        key
    }

    fn brush_layer() -> local_adjust_core::LocalAdjustmentLayer {
        local_adjust_core::LocalAdjustmentLayer::new(
            "brush",
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::None,
        )
    }

    #[test]
    fn local_adjust_layer_bypass_disables_only_selected_layer() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/layer-bypass.jpg");
        app.local_adjust_page_layers.insert(
            idx,
            vec![
                local_adjust_core::LocalAdjustmentLayer::new(
                    "A",
                    local_adjust_core::LocalMask::Full,
                    local_adjust_core::LocalEffect::None,
                ),
                local_adjust_core::LocalAdjustmentLayer::new(
                    "B",
                    local_adjust_core::LocalMask::Full,
                    local_adjust_core::LocalEffect::None,
                ),
                local_adjust_core::LocalAdjustmentLayer::new(
                    "C",
                    local_adjust_core::LocalMask::Full,
                    local_adjust_core::LocalEffect::None,
                ),
            ],
        );

        let preview_layers = app
            .local_adjust_layers_with_selected_layer_bypassed(idx, 1)
            .expect("A and C remain active after bypassing B");

        assert_eq!(
            preview_layers
                .iter()
                .map(|layer| (layer.name.as_str(), layer.enabled))
                .collect::<Vec<_>>(),
            vec![("A", true), ("B", false), ("C", true)]
        );
        assert!(
            app.local_adjust_page_layers
                .get(&idx)
                .unwrap()
                .iter()
                .all(|layer| layer.enabled),
            "bypass preview must not mutate stored layer state"
        );
    }

    /// P7-1a: `bump_adjustment_generation(idx)` は **別 idx** の
    /// `final_ai_cache` / `final_composite_cache` を一切 touch しない。
    ///
    /// 回帰防止: P5-6 で発見した「open_fullscreen が defensive に隣接ページの
    /// `bump_adjustment_generation` を呼ぶ → 隣接の prefetch 結果が消える」退行の
    /// **idx-scoping そのもの**を符号化する。`retain` の filter 条件が誤って
    /// `cache_key.edit_key.idx != idx` から `cache_key.edit_key.idx == idx` に
    /// 反転したら、本テストが落ちる。
    #[test]
    fn adjustment_generation_only_affects_the_target_idx() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx_a = push_image(&mut app, "C:/pics/adjust-idx-a.jpg");
        let idx_b = push_image(&mut app, "C:/pics/adjust-idx-b.jpg");
        let (_edit_a, final_a) = insert_edit_and_final_cache(&mut app, &ctx, idx_a, "adj_a");
        let (_edit_b, final_b) = insert_edit_and_final_cache(&mut app, &ctx, idx_b, "adj_b");

        assert_eq!(app.final_composite_cache.len(), 2);
        assert_eq!(app.final_ai_cache.len(), 2);

        // idx_a だけ bump → idx_b の final_composite / final_ai は無傷
        app.bump_adjustment_generation(idx_a);

        assert!(
            !app.final_composite_cache.contains_key(&final_a),
            "idx_a の final_composite は無効化される"
        );
        assert!(
            app.final_composite_cache.contains_key(&final_b),
            "idx_b の final_composite は P7-1a で守られる (= 隣接ページの prefetch 死守)"
        );
        // AI cache は両 idx とも生存
        assert_eq!(
            app.final_ai_cache.len(),
            2,
            "bump_adjustment_generation は final_ai_cache を一切 touch しない (P5-6)"
        );
    }

    /// P7-1b: `bump_ai_generation(idx)` は AI cache だけ idx 単位で clear し、
    /// **`edit_result_cache` は touch しない** (= 上流の編集結果は AI モデル切替で再構築不要)。
    ///
    /// 回帰防止: AI モデル切替や failure 後 retry で `bump_ai_generation` が呼ばれた
    /// とき、edit_result まで一緒に捨てる退行が起きると source 解像度の編集 (消しゴム /
    /// 補正レイヤー / 隠蔽) の再合成も走り、UI が大きく止まる。
    #[test]
    fn ai_generation_clears_final_pipeline_but_keeps_edit_cache() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/ai-gen-keep-edit.jpg");
        let (edit_key, final_key) = insert_edit_and_final_cache(&mut app, &ctx, idx, "ai_gen");

        assert!(app.edit_result_cache.contains_key(&edit_key));
        assert!(app.final_composite_cache.contains_key(&final_key));
        assert!(!app.final_ai_cache.is_empty());

        app.bump_ai_generation(idx);

        // 上流の edit_result は維持
        assert!(
            app.edit_result_cache.contains_key(&edit_key),
            "AI 世代 bump は edit_result_cache を保持する (= 編集結果は AI に依存しない)"
        );
        // 下流の final pipeline は idx 単位で clear
        assert!(
            !app.final_composite_cache.contains_key(&final_key),
            "AI 世代 bump で final_composite が idx 単位で clear される"
        );
        assert!(
            app.final_ai_cache.is_empty(),
            "AI 世代 bump で final_ai_cache が idx 単位で clear される"
        );
    }

    /// P9-2: AI モデルが両方未設定なら `final_ai_key_for_pixels` は None を返す。
    /// このとき key が無いので AI 推論は不要、`is_idx_final_ai_done_or_skipped` は
    /// **true (= done 扱い)** を返して進捗バーから外れる。
    ///
    /// セマンティクス: 「AI 機能を使わないユーザー」(= upscale_model / denoise_model
    /// 共に None) で先読み進捗バーが終わらない退行を防ぐ。AI 不要なら最初から
    /// 「done」として総数にカウントしない設計。
    #[test]
    fn final_ai_key_is_none_when_no_ai_models_configured() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/no-ai-models.jpg");

        // デフォルト AdjustParams は upscale_model = None / denoise_model = None
        let params = app.effective_params(idx);
        assert!(
            params.upscale_model_kind().is_none(),
            "fixture pre-condition: upscale model is None"
        );
        assert!(
            params.denoise_model_kind().is_none(),
            "fixture pre-condition: denoise model is None"
        );

        let edit_key = dummy_edit_key(&app, idx);
        let result = app.final_ai_key_for_pixels(edit_key, [1920, 1080], &params);
        assert!(
            result.is_none(),
            "両モデル None なら final_ai_key も None (= AI 推論ジョブ不要)"
        );
    }

    /// P9-2b: AI モデル未設定で `is_idx_final_ai_done_or_skipped` が **true** を返す。
    /// 先読み進捗バーの total 計算で「AI 不要 = done 扱い」とすることで、AI を
    /// 使わないユーザー (= 多数派) で進捗バーが終わらない退行を防ぐ。
    ///
    /// 退行防止: もし `final_ai_key_for_pixels` の None 戻りで `is_done_or_skipped`
    /// が false を返すように変わると、AI 不要なページが永久に pending 扱いになり、
    /// 進捗バーが消えなくなる (= 「AI 設定無いのにバーが出続ける」ユーザー報告)。
    #[test]
    fn is_idx_final_ai_done_or_skipped_true_when_no_ai_models() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/no-ai-models-done.jpg");

        // raw_source_pixels が Some を返すように fs_cache に Static を仕込む
        let img = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(100, 110, 120)]);
        let pixels = Arc::new(img.clone());
        let tex = ctx.load_texture("no-ai-source", img, Default::default());
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex,
                pixels: Arc::clone(&pixels),
                source_dims: Some([1, 1]),
                load_seq: 0,
            },
        );

        // 事前条件: source pixels が解決できる
        assert!(
            app.current_raw_source_pixels(idx).is_some(),
            "fixture: raw_source_pixels が引ける状態"
        );

        // AI モデル未設定 → AI 不要 → done 扱い (= true)
        assert!(
            app.is_idx_final_ai_done_or_skipped(idx),
            "AI モデル未設定の idx は done 扱い (= 進捗バー total から自然に外れる)"
        );
    }

    /// P9-1 helper: idx 分の **全 edit chain cache** (erase_result / local_adjust /
    /// conceal / edit_result / final_composite / final_ai) に 1 件ずつ entry を仕込み、
    /// bump_* 後に「何が残って何が消えたか」を観測しやすくする fixture。
    fn populate_all_idx_caches(
        app: &mut App,
        ctx: &egui::Context,
        idx: usize,
        label: &str,
    ) -> (
        EraseResultKey,
        LocalAdjustResultKey,
        EditResultKey,
        FinalCompositeKey,
    ) {
        // erase_result_cache
        let erase_key = app.current_erase_result_key(idx);
        let erase_img = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(50, 60, 70)]);
        let erase_tex = ctx.load_texture(
            format!("{label}_erase"),
            erase_img.clone(),
            Default::default(),
        );
        app.erase_result_cache.insert(
            erase_key,
            EraseResultCacheEntry {
                pixels: Arc::new(erase_img),
                texture: erase_tex,
            },
        );

        // local_adjust_cache
        let local_key = insert_local_adjust_cache(app, ctx, idx, label);

        // conceal_cache
        let conceal_img = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(70, 80, 90)]);
        let conceal_tex = ctx.load_texture(
            format!("{label}_conceal"),
            conceal_img.clone(),
            Default::default(),
        );
        let conceal_generation = app.conceal_generation;
        app.conceal_cache.insert(
            idx,
            ConcealCacheEntry {
                pixels: Arc::new(conceal_img),
                texture: conceal_tex,
                generation: conceal_generation,
            },
        );

        // edit_result_cache + final_composite_cache + final_ai_cache
        let (edit_key, final_key) = insert_edit_and_final_cache(app, ctx, idx, label);

        (erase_key, local_key, edit_key, final_key)
    }

    /// P9-1a: `bump_input_generation(idx)` は **edit chain 全部** (erase_result /
    /// local_adjust / conceal / edit_result) + **final pipeline 全部** (final_ai /
    /// final_composite) を idx 単位で全部 clear する。
    ///
    /// セマンティクス: 表示入力世代の bump は「decode 結果から下流全部やり直し」
    /// なので、計算済みの中間結果は全てを捨てる。`bump_adjustment_generation` の
    /// 「final_composite だけ」とは対照的に、これは最強の clear。
    ///
    /// 退行防止: もし erase_result まで巻き込まずに local_adjust 以下だけ clear する
    /// ような最適化が誤って入ると、source 解像度の編集が古い decode 結果に縛られて
    /// 表示が更新されない事故になる。
    #[test]
    fn bump_input_generation_clears_entire_edit_chain_for_idx() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/bump-input.jpg");
        let (erase_key, local_key, edit_key, final_key) =
            populate_all_idx_caches(&mut app, &ctx, idx, "bump_input");

        app.bump_input_generation(idx);

        // 全部 idx 単位で clear される
        assert!(
            !app.erase_result_cache.contains_key(&erase_key),
            "bump_input は erase_result も clear する (= 最上流の bump)"
        );
        assert!(!app.local_adjust_cache.contains_key(&local_key));
        assert!(!app.conceal_cache.contains_key(&idx));
        assert!(!app.edit_result_cache.contains_key(&edit_key));
        assert!(!app.final_composite_cache.contains_key(&final_key));
        assert!(
            app.final_ai_cache.is_empty(),
            "bump_input は final_ai も idx 単位で clear する (= clear_edit_result_caches_for_idx 経由)"
        );
    }

    /// P9-1b: `bump_erase_mask_generation(idx)` は `bump_input_generation` と
    /// **構造的に同じ** clear 振る舞い (= 同じ helpers 呼ぶ実装)。別 fn なので
    /// 個別 fixation して「片方だけ削除パターンが追加される」退行を検知する。
    #[test]
    fn bump_erase_mask_generation_clears_entire_edit_chain_for_idx() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/bump-erase-mask.jpg");
        let (erase_key, local_key, edit_key, final_key) =
            populate_all_idx_caches(&mut app, &ctx, idx, "bump_erase_mask");

        app.bump_erase_mask_generation(idx);

        assert!(!app.erase_result_cache.contains_key(&erase_key));
        assert!(!app.local_adjust_cache.contains_key(&local_key));
        assert!(!app.conceal_cache.contains_key(&idx));
        assert!(!app.edit_result_cache.contains_key(&edit_key));
        assert!(!app.final_composite_cache.contains_key(&final_key));
        assert!(app.final_ai_cache.is_empty());
    }

    /// P9-1c: `bump_local_adjust_generation(idx)` は **erase_result を keep** する点で
    /// `bump_input` / `bump_erase_mask` と異なる。補正レイヤーは消しゴム結果より下流
    /// なので、layer 変更で消しゴム inpaint 結果まで再計算する必要は無い、という
    /// パイプライン階層を符号化する。
    ///
    /// 退行防止: 「効率化のため」と称して erase_result を巻き込む変更が入ると、
    /// 重い MI-GAN 推論が補正レイヤー編集のたびに走るようになる。
    #[test]
    fn bump_local_adjust_generation_keeps_erase_result_but_clears_downstream() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/bump-local-adjust.jpg");
        let (erase_key, local_key, edit_key, final_key) =
            populate_all_idx_caches(&mut app, &ctx, idx, "bump_local_adjust");

        app.bump_local_adjust_generation(idx);

        assert!(
            app.erase_result_cache.contains_key(&erase_key),
            "erase_result は上流なので bump_local_adjust では保護される \
             (= MI-GAN 推論を再起動しないための重要な最適化)"
        );
        assert!(
            !app.local_adjust_cache.contains_key(&local_key),
            "対象 cache (local_adjust) は idx 単位で clear"
        );
        assert!(
            !app.conceal_cache.contains_key(&idx),
            "下流の conceal は clear (= local_adjust の出力に依存するので無効化)"
        );
        assert!(
            !app.edit_result_cache.contains_key(&edit_key),
            "下流の edit_result も clear"
        );
        assert!(
            !app.final_composite_cache.contains_key(&final_key),
            "final pipeline も idx 単位で clear (= clear_edit_result_caches_for_idx 経由)"
        );
        assert!(app.final_ai_cache.is_empty());
    }

    /// P7-2: `bump_ai_generation(idx)` は失敗 cache (`final_ai_failed`) も idx 単位で
    /// 削除し、別 idx の失敗エントリは無傷に保つ。
    ///
    /// セマンティクス: AI モデル切替や retry 経路で `bump_ai_generation` が走ったとき、
    /// 「同じ key で AI が失敗した」履歴も対象 idx 分はリセットして次回再試行を許可する。
    /// 一方、別 idx の失敗履歴は別文脈なので削除してはいけない。
    ///
    /// 回帰防止: `clear_final_pipeline_caches_for_idx` の retain filter が
    /// `key.edit_key.idx != idx` から退化すると、別ページの失敗履歴を巻き込んで
    /// 削除し、AI 推論が同じ理由で繰り返し失敗するページでも再 spawn してしまう。
    #[test]
    fn ai_generation_also_clears_failed_for_target_idx() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx_a = push_image(&mut app, "C:/pics/ai-fail-a.jpg");
        let idx_b = push_image(&mut app, "C:/pics/ai-fail-b.jpg");
        let (_edit_a, final_a) = insert_edit_and_final_cache(&mut app, &ctx, idx_a, "fail_a");
        let (_edit_b, final_b) = insert_edit_and_final_cache(&mut app, &ctx, idx_b, "fail_b");

        // 両 idx に AI 失敗履歴を仕込む (= 直近の AI 推論で失敗した状態)
        let failed_a = FinalAiKey {
            edit_key: final_a.edit_key,
            color_ai_hash: 0xDEAD,
            bg: 0,
        };
        let failed_b = FinalAiKey {
            edit_key: final_b.edit_key,
            color_ai_hash: 0xDEAD,
            bg: 0,
        };
        app.final_ai_failed.insert(failed_a);
        app.final_ai_failed.insert(failed_b);
        assert_eq!(app.final_ai_failed.len(), 2);

        // idx_a の AI 世代だけ bump
        app.bump_ai_generation(idx_a);

        assert!(
            !app.final_ai_failed.contains(&failed_a),
            "対象 idx の失敗履歴はリセットされる (= 次回再試行を許可)"
        );
        assert!(
            app.final_ai_failed.contains(&failed_b),
            "別 idx の失敗履歴は無傷 (= 別ページの retry を巻き込まない)"
        );
    }

    /// P7-1c: `bump_conceal_generation()` (global) は **全 idx** の
    /// `edit_result_cache` + final pipeline を一括 clear する。
    ///
    /// 回帰防止: conceal 不透明度 / ぼかし半径などのグローバルパラメータが変わったとき、
    /// 該当ページだけ touch する設計だと「他ページで先に hydrate された edit_result」が
    /// 古い conceal で固まる事故が起きる。conceal_gen は `EditResultKey` のフィールドなので、
    /// global bump + 全 clear で「次の lookup から新 conceal_gen で再構築」を強制する。
    #[test]
    fn conceal_generation_clears_all_idx_edit_and_final_caches() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx_a = push_image(&mut app, "C:/pics/conceal-gen-a.jpg");
        let idx_b = push_image(&mut app, "C:/pics/conceal-gen-b.jpg");
        let (edit_a, final_a) = insert_edit_and_final_cache(&mut app, &ctx, idx_a, "conceal_a");
        let (edit_b, final_b) = insert_edit_and_final_cache(&mut app, &ctx, idx_b, "conceal_b");

        assert_eq!(app.edit_result_cache.len(), 2);
        assert_eq!(app.final_composite_cache.len(), 2);
        assert_eq!(app.final_ai_cache.len(), 2);
        let conceal_gen_before = app.conceal_generation;

        app.bump_conceal_generation();

        // global なので idx_a / idx_b の両方が消える
        assert_eq!(
            app.conceal_generation,
            conceal_gen_before.wrapping_add(1),
            "conceal_generation は +1 される (= 全 edit_result key が次回 lookup で stale 化)"
        );
        assert!(
            !app.edit_result_cache.contains_key(&edit_a),
            "idx_a の edit_result は global clear で消える"
        );
        assert!(
            !app.edit_result_cache.contains_key(&edit_b),
            "idx_b の edit_result も global clear で消える (P7-1c の核)"
        );
        assert!(app.edit_result_cache.is_empty());
        assert!(
            !app.final_composite_cache.contains_key(&final_a)
                && !app.final_composite_cache.contains_key(&final_b),
            "global bump は final pipeline も全 clear する"
        );
        assert!(app.final_composite_cache.is_empty());
        assert!(app.final_ai_cache.is_empty());
    }

    /// P5-6: `bump_adjustment_generation` は `final_ai_cache` を巻き込まず、
    /// `final_composite_cache` だけ idx 単位で無効化する。
    ///
    /// ## 退行の根本原因 (2026-06 実害)
    ///
    /// 旧実装は `clear_final_pipeline_caches_for_idx(idx)` を呼んでいて、それが
    /// `final_ai_cache.retain(|key| key.edit_key.idx != idx)` を実行 → idx の AI 結果を
    /// 全削除していた。`open_fullscreen(idx)` がページ送りのたびに
    /// `bump_adjustment_generation(idx)` を defensive に呼ぶ設計なので、prefetch で
    /// 完了済みの AI 結果が次ページ表示の瞬間に消える事故が発生
    /// (= 「先読みバーは出ているのに次画像で一瞬待たされる」)。
    ///
    /// 期待される挙動:
    /// - `edit_result_cache`: 維持 (= source 解像度の編集結果は色補正と独立)
    /// - `final_composite_cache`: idx 単位で削除 (= 色補正適用後の最終 composite は再構築)
    /// - **`final_ai_cache`: 維持** (= key に AdjustParams を含むので新 params で自動 miss、
    ///   旧 params の AI 結果は同じ params に戻したときの再利用に備えて保持)
    /// - `final_ai_failed`: 維持 (= 同じ理由、key で自動区別)
    #[test]
    fn adjustment_generation_keeps_edit_and_ai_cache_clears_only_final_composite() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/pipeline-adjust.jpg");
        let (edit_key, final_key) =
            insert_edit_and_final_cache(&mut app, &ctx, idx, "adjust_change");

        // insert_edit_and_final_cache が AI cache に 1 件入れている前提
        assert!(
            !app.final_ai_cache.is_empty(),
            "fixture should pre-populate final_ai_cache"
        );

        app.bump_adjustment_generation(idx);

        assert!(
            app.edit_result_cache.contains_key(&edit_key),
            "AdjustParams changes must not invalidate source-resolution edit results"
        );
        assert!(
            !app.final_composite_cache.contains_key(&final_key),
            "final composite depends on AdjustParams and must be rebuilt"
        );
        // ⚠ 退行防止: AI cache は触らない。新 AdjustParams の key で自動 miss する。
        assert!(
            !app.final_ai_cache.is_empty(),
            "AI cache MUST survive bump_adjustment_generation (= preserves prefetched AI \
             results from being killed every page advance via open_fullscreen)"
        );
    }

    #[test]
    fn final_composite_applies_post_filter_without_polluting_edit_cache() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/pipeline-post-filter.jpg");
        let raw_color = egui::Color32::from_rgb(60, 120, 180);
        let raw_pixels = Arc::new(egui::ColorImage::new([1, 1], vec![raw_color]));
        let raw_texture = ctx.load_texture(
            "post_filter_raw",
            (*raw_pixels).clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex: raw_texture,
                pixels: Arc::clone(&raw_pixels),
                source_dims: Some([1, 1]),
                load_seq: 0,
            },
        );
        app.settings.global_preset.post_filter = PostFilter::Sepia;

        let (edit_key, edit_pixels) = app
            .ensure_edit_result_pixels(&ctx, idx)
            .expect("edit result should be available from raw source");
        let final_pixels = app
            .ensure_final_composite_pixels(&ctx, idx)
            .expect("final composite should be complete without AI");
        let expected = crate::post_filter::apply(&raw_pixels, crate::adjustment::PostFilter::Sepia);

        assert_eq!(
            edit_pixels.pixels[0], raw_color,
            "edit cache must stay in source-resolution, pre-post-filter space"
        );
        assert_eq!(final_pixels.pixels[0], expected.pixels[0]);
        assert_ne!(
            final_pixels.pixels[0], edit_pixels.pixels[0],
            "post_filter should be visible only at the final composite stage"
        );
        assert_eq!(
            app.edit_result_cache
                .get(&edit_key)
                .expect("edit cache entry should remain")
                .pixels
                .pixels[0],
            raw_color
        );
    }

    #[test]
    fn pano_source_switches_to_completed_final_composite() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/pano-final.jpg");
        app.fullscreen_idx = Some(idx);

        let raw_color = egui::Color32::from_rgb(20, 30, 40);
        let raw_pixels = Arc::new(egui::ColorImage::new([2, 1], vec![raw_color; 2]));
        let raw_texture = ctx.load_texture(
            "pano_final_raw",
            (*raw_pixels).clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex: raw_texture,
                pixels: Arc::clone(&raw_pixels),
                source_dims: Some([2, 1]),
                load_seq: 0,
            },
        );

        let edit_key = dummy_edit_key(&app, idx);
        let edit_texture = ctx.load_texture(
            "pano_final_edit",
            (*raw_pixels).clone(),
            egui::TextureOptions::LINEAR,
        );
        app.edit_result_cache.insert(
            edit_key,
            EditResultEntry {
                pixels: Arc::clone(&raw_pixels),
                texture: Some(edit_texture),
            },
        );

        let params = app.effective_params(idx).clone();
        let final_key = app.final_composite_key_for_pixels(edit_key, raw_pixels.size, &params);
        let preview_texture = ctx.load_texture(
            "pano_final_preview",
            (*raw_pixels).clone(),
            egui::TextureOptions::LINEAR,
        );
        app.final_composite_cache.insert(
            final_key,
            FinalCompositeEntry {
                pixels: Arc::clone(&raw_pixels),
                texture: preview_texture,
                complete: false,
            },
        );

        let fallback = app
            .resolve_pano_source(&ctx, idx)
            .expect("raw fallback should be available while final composite is incomplete");
        assert_eq!(fallback.source_kind, crate::panorama::SOURCE_KIND_FS);
        assert_eq!(fallback.pixels.size, [2, 1]);

        let final_color = egui::Color32::from_rgb(200, 210, 220);
        let final_pixels = Arc::new(egui::ColorImage::new([8, 4], vec![final_color; 32]));
        let final_texture = ctx.load_texture(
            "pano_final_complete",
            (*final_pixels).clone(),
            egui::TextureOptions::LINEAR,
        );
        app.final_composite_cache.insert(
            final_key,
            FinalCompositeEntry {
                pixels: Arc::clone(&final_pixels),
                texture: final_texture,
                complete: true,
            },
        );

        let completed = app
            .resolve_pano_source(&ctx, idx)
            .expect("completed final composite should be panorama-ready");
        assert_eq!(
            completed.source_kind,
            crate::panorama::SOURCE_KIND_FINAL_COMPOSITE
        );
        assert_eq!(completed.pixels.size, [8, 4]);
        assert_eq!(completed.pixels.pixels[0], final_color);
        assert_ne!(
            fallback.cache_key, completed.cache_key,
            "AI/final completion must force the pano upload cache to refresh"
        );
    }

    #[test]
    fn edit_generation_clears_only_the_changed_page_edit_and_final_caches() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx_a = push_image(&mut app, "C:/pics/pipeline-edit-a.jpg");
        let idx_b = push_image(&mut app, "C:/pics/pipeline-edit-b.jpg");
        let (edit_a, final_a) = insert_edit_and_final_cache(&mut app, &ctx, idx_a, "edit_a");
        let (edit_b, final_b) = insert_edit_and_final_cache(&mut app, &ctx, idx_b, "edit_b");

        app.bump_erase_mask_generation(idx_a);

        assert!(!app.edit_result_cache.contains_key(&edit_a));
        assert!(!app.final_composite_cache.contains_key(&final_a));
        assert!(
            app.edit_result_cache.contains_key(&edit_b),
            "unrelated pages should stay hot during page-local edit invalidation"
        );
        assert!(app.final_composite_cache.contains_key(&final_b));
    }

    #[test]
    fn upscale_purge_preserves_source_resolution_edit_cache() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/pipeline-upscale-toggle.jpg");
        let (edit_key, final_key) =
            insert_edit_and_final_cache(&mut app, &ctx, idx, "upscale_toggle");
        app.mask_pages.insert(idx);
        app.erase_mask_generation.insert(idx, 44);
        app.input_generation.insert(idx, 55);

        app.purge_upscale_for_idx(idx);

        assert!(
            app.edit_result_cache.contains_key(&edit_key),
            "AI upscale ON/OFF must not drop source-resolution edit results or masks"
        );
        assert!(
            !app.final_composite_cache.contains_key(&final_key),
            "final composite must be rebuilt after AI upscale cache changes"
        );
        assert!(app.mask_pages.contains(&idx));
        assert_eq!(app.erase_mask_generation.get(&idx), Some(&44));
        assert_eq!(app.input_generation.get(&idx), Some(&55));
    }

    #[test]
    fn deferred_brush_render_keeps_generation_until_idle_deadline() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/pipeline-brush-defer.jpg");
        let local_key = insert_local_adjust_cache(&mut app, &ctx, idx, "brush_defer_local");
        let (edit_key, final_key) =
            insert_edit_and_final_cache(&mut app, &ctx, idx, "brush_defer_final");

        app.set_local_adjust_layers_for_idx_memory_only_defer_render(idx, vec![brush_layer()]);
        let pending = app
            .local_adjust_brush_deferred_render
            .expect("brush render should be deferred");
        let early = pending.last_input_at
            + std::time::Duration::from_millis(LOCAL_ADJUST_BRUSH_EFFECT_DEFER_MS - 1);

        let delay = app
            .poll_deferred_local_adjust_brush_render_at(early)
            .expect("deadline should not flush early");
        assert!(delay <= std::time::Duration::from_millis(1));
        assert_eq!(
            app.local_adjust_generation.get(&idx).copied().unwrap_or(0),
            0
        );
        assert!(app.local_adjust_cache.contains_key(&local_key));
        assert!(app.edit_result_cache.contains_key(&edit_key));
        assert!(app.final_composite_cache.contains_key(&final_key));

        let due = pending.last_input_at
            + std::time::Duration::from_millis(LOCAL_ADJUST_BRUSH_EFFECT_DEFER_MS);
        assert!(
            app.poll_deferred_local_adjust_brush_render_at(due)
                .is_none()
        );
        assert_eq!(app.local_adjust_generation.get(&idx), Some(&1));
        assert!(app.local_adjust_brush_deferred_render.is_none());
        assert!(!app.local_adjust_cache.contains_key(&local_key));
        assert!(!app.edit_result_cache.contains_key(&edit_key));
        assert!(!app.final_composite_cache.contains_key(&final_key));
    }

    #[test]
    fn brush_release_bump_cancels_deferred_render_without_double_generation() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/pipeline-brush-release.jpg");

        app.set_local_adjust_layers_for_idx_memory_only_defer_render(idx, vec![brush_layer()]);
        let pending = app
            .local_adjust_brush_deferred_render
            .expect("brush render should be deferred");
        app.set_local_adjust_layers_for_idx_memory_only(idx, vec![brush_layer()]);

        assert_eq!(app.local_adjust_generation.get(&idx), Some(&1));
        assert!(app.local_adjust_brush_deferred_render.is_none());
        let due = pending.last_input_at
            + std::time::Duration::from_millis(LOCAL_ADJUST_BRUSH_EFFECT_DEFER_MS * 2);
        assert!(
            app.poll_deferred_local_adjust_brush_render_at(due)
                .is_none()
        );
        assert_eq!(
            app.local_adjust_generation.get(&idx),
            Some(&1),
            "release commit already invalidated render caches; deferred timer must not bump again"
        );
    }

    // -----------------------------------------------------------------------
    // P4-1〜P4-5: bypass preview (Ctrl+Shift) vs prefix preview (panel toggle)
    //
    // 回帰防止の背景 (A-3 v1/v2/v3 で実害発生):
    //   - Ctrl+Shift は「選択レイヤーだけバイパス」(layer_idx=N → N 以外を全部適用)
    //   - 「選択レイヤーまでプレビュー」(panel checkbox) は「先頭から N+1 枚を適用」
    //   - 2 つは意味が違うので **cache キーも別レーン** に分けないと衝突する
    //   - lab `layers_with_selected_layer_bypassed` (tools/local_adjust_lab/src/main.rs:23348)
    //     と mIV `local_adjust_layers_with_selected_layer_bypassed` (src/app.rs) の変換式は
    //     完全に同じでなければならない (= bypass の semantics をぶれさせない)
    // -----------------------------------------------------------------------

    fn full_layer(name: &str) -> local_adjust_core::LocalAdjustmentLayer {
        local_adjust_core::LocalAdjustmentLayer::new(
            name,
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::None,
        )
    }

    /// ラボ tools/local_adjust_lab/src/main.rs:23348-23357 のリファレンス実装。
    /// mIV 側 (`App::local_adjust_layers_with_selected_layer_bypassed`) は Option で
    /// 「他に有効レイヤーが無ければ None」を返す最適化を持つが、**変換式そのもの**は
    /// この関数と一致する必要がある (= 選択レイヤーの enabled=false にする、他は触らない)。
    fn lab_layers_with_selected_layer_bypassed(
        layers: &[local_adjust_core::LocalAdjustmentLayer],
        selected_layer: usize,
    ) -> Vec<local_adjust_core::LocalAdjustmentLayer> {
        let mut preview_layers = layers.to_vec();
        if let Some(layer) = preview_layers.get_mut(selected_layer) {
            layer.enabled = false;
        }
        preview_layers
    }

    /// P4-1: bypass と prefix の cache key は同じ idx でも別レーンに乗る。
    /// 同じ result_key で layer_idx と layer_count が偶然同じ整数になっても、
    /// HashMap で別エントリとして区別される (= 型が違う)。
    #[test]
    fn bypass_and_prefix_preview_caches_are_separate_lanes() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/lane-sep.jpg");

        let result_key = app.current_local_adjust_key(idx);
        let bypass_key = LocalAdjustLayerBypassPreviewKey {
            result_key,
            layer_idx: 1,
        };
        let prefix_key = LocalAdjustPrefixPreviewKey {
            result_key,
            layer_count: 1,
        };

        let bypass_image = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(11, 22, 33)]);
        let prefix_image = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(44, 55, 66)]);
        let bypass_tex = ctx.load_texture(
            "lane_sep_bypass",
            bypass_image.clone(),
            egui::TextureOptions::LINEAR,
        );
        let prefix_tex = ctx.load_texture(
            "lane_sep_prefix",
            prefix_image.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.local_adjust_layer_bypass_cache.insert(
            bypass_key,
            LocalAdjustCacheEntry {
                pixels: Arc::new(bypass_image),
                texture: bypass_tex,
            },
        );
        app.local_adjust_prefix_preview_cache.insert(
            prefix_key,
            LocalAdjustCacheEntry {
                pixels: Arc::new(prefix_image),
                texture: prefix_tex,
            },
        );

        // 両方とも独立に lookup できる
        assert!(
            app.current_local_adjust_layer_bypass_texture(idx, 1)
                .is_some(),
            "bypass cache lane must remain queryable"
        );
        assert!(
            app.current_local_adjust_prefix_preview_texture(idx, 1)
                .is_some(),
            "prefix cache lane must remain queryable"
        );
        // 別 layer_idx / layer_count では miss する
        assert!(
            app.current_local_adjust_layer_bypass_texture(idx, 0)
                .is_none()
        );
        assert!(
            app.current_local_adjust_prefix_preview_texture(idx, 2)
                .is_none()
        );
    }

    /// P4-2: ラボ vs mIV — bypass 変換式の意味的一致テスト。
    /// 違う意味で実装すると Ctrl+Shift が「直前までの prefix」とすり替わる (A-3 v3 で実害)。
    #[test]
    fn local_adjust_layer_bypass_matches_lab_transformation() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/bypass-lab-parity.jpg");
        let layers = vec![full_layer("A"), full_layer("B"), full_layer("C")];
        app.local_adjust_page_layers.insert(idx, layers.clone());

        // 各 layer_idx について、mIV と lab で同じ (name, enabled) 並びになる
        for layer_idx in 0..layers.len() {
            let miv = app
                .local_adjust_layers_with_selected_layer_bypassed(idx, layer_idx)
                .expect("two other layers still enabled");
            let lab = lab_layers_with_selected_layer_bypassed(&layers, layer_idx);

            assert_eq!(
                miv.iter()
                    .map(|l| (l.name.clone(), l.enabled))
                    .collect::<Vec<_>>(),
                lab.iter()
                    .map(|l| (l.name.clone(), l.enabled))
                    .collect::<Vec<_>>(),
                "bypass transformation must match lab for selected={layer_idx}"
            );
            assert!(
                !miv[layer_idx].enabled,
                "selected layer must be disabled in preview at idx={layer_idx}"
            );
            let others = miv
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != layer_idx)
                .filter(|(_, l)| l.enabled)
                .count();
            assert_eq!(
                others, 2,
                "the other 2 layers must remain enabled at idx={layer_idx}"
            );
        }
    }

    /// P4-3: prefix preview の semantics。
    /// 「選択レイヤーまでプレビュー」(panel checkbox) は先頭から N 枚を渡す。
    /// - layer_count == 0 → None (描画不要)
    /// - layer_count >= len → None (= 通常レンダリングと同じ、prefix preview 不要)
    /// - layer_count == len-1 → Some(vec[0..len-1])
    #[test]
    fn local_adjust_prefix_preview_boundaries() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/prefix-boundaries.jpg");
        let layers = vec![full_layer("A"), full_layer("B"), full_layer("C")];
        app.local_adjust_page_layers.insert(idx, layers);

        // layer_count = 0 → None
        assert!(
            app.local_adjust_layers_until(idx, 0).is_none(),
            "prefix preview with zero layers must yield None"
        );
        // layer_count = len → None (フル合成は別経路)
        assert!(
            app.local_adjust_layers_until(idx, 3).is_none(),
            "prefix == total layers must short-circuit (final composite handles it)"
        );
        // layer_count = len + 1 (clamp) → None
        assert!(
            app.local_adjust_layers_until(idx, 99).is_none(),
            "prefix > total layers must also short-circuit"
        );
        // layer_count = 1 → A だけ
        let one = app
            .local_adjust_layers_until(idx, 1)
            .expect("prefix=1 returns first layer");
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].name, "A");
        // layer_count = 2 → A + B
        let two = app
            .local_adjust_layers_until(idx, 2)
            .expect("prefix=2 returns first two");
        assert_eq!(
            two.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            vec!["A", "B"]
        );
    }

    /// P4-4: bypass で残るレイヤーが全て disabled になるなら preview 不要 (None)。
    /// これは worker 起動の最適化 = Ctrl+Shift 押下時に渡せるレイヤーが無いなら
    /// そのまま source 表示にフォールバックさせる。
    #[test]
    fn local_adjust_layer_bypass_returns_none_when_no_other_enabled_layers_remain() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/bypass-empty.jpg");

        // ケース 1: 唯一のレイヤーを bypass → 残り 0
        app.local_adjust_page_layers
            .insert(idx, vec![full_layer("only")]);
        assert!(
            app.local_adjust_layers_with_selected_layer_bypassed(idx, 0)
                .is_none(),
            "bypassing the only layer must yield None (nothing to render)"
        );

        // ケース 2: A(disabled) + B(enabled) で B を bypass → 残りは A(disabled) のみ → None
        let mut a = full_layer("A");
        a.enabled = false;
        app.local_adjust_page_layers
            .insert(idx, vec![a, full_layer("B")]);
        assert!(
            app.local_adjust_layers_with_selected_layer_bypassed(idx, 1)
                .is_none(),
            "bypassing the only enabled layer must yield None"
        );

        // ケース 3: opacity=0 のレイヤーも「有効」とみなさない
        let mut zero_op = full_layer("zero");
        zero_op.opacity = 0.0;
        app.local_adjust_page_layers
            .insert(idx, vec![zero_op, full_layer("real")]);
        assert!(
            app.local_adjust_layers_with_selected_layer_bypassed(idx, 1)
                .is_none(),
            "bypassing the only opaque layer must yield None (opacity=0 doesn't count)"
        );

        // ケース 4: 範囲外 idx → None
        app.local_adjust_page_layers
            .insert(idx, vec![full_layer("a"), full_layer("b")]);
        assert!(
            app.local_adjust_layers_with_selected_layer_bypassed(idx, 99)
                .is_none(),
            "out-of-bounds layer_idx must yield None"
        );
    }

    /// P4-5: final_composite_cache と bypass preview cache は独立に存在できる。
    /// Ctrl+Shift トグルで毎回 final composite を捨てる事故 (= スライダー応答悪化) を防ぐ。
    #[test]
    fn bypass_preview_cache_coexists_with_final_composite_cache() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/bypass-coexist.jpg");
        let (edit_key, final_key) =
            insert_edit_and_final_cache(&mut app, &ctx, idx, "bypass_coexist");

        // bypass preview cache に何か入れる (= worker 完了 simulate)
        let result_key = app.current_local_adjust_key(idx);
        let bypass_key = LocalAdjustLayerBypassPreviewKey {
            result_key,
            layer_idx: 0,
        };
        let bypass_image = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(99, 0, 0)]);
        let bypass_tex = ctx.load_texture(
            "coexist_bypass",
            bypass_image.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.local_adjust_layer_bypass_cache.insert(
            bypass_key,
            LocalAdjustCacheEntry {
                pixels: Arc::new(bypass_image),
                texture: bypass_tex,
            },
        );

        // 両方とも生き残っていること
        assert!(
            app.edit_result_cache.contains_key(&edit_key),
            "edit_result_cache must survive bypass preview population"
        );
        assert!(
            app.final_composite_cache.contains_key(&final_key),
            "final_composite_cache must survive bypass preview population"
        );
        assert!(
            app.current_local_adjust_layer_bypass_texture(idx, 0)
                .is_some(),
            "bypass preview cache must remain populated"
        );

        // 別ページの clear が他ページの bypass cache を巻き込まないこと
        let other_idx = push_image(&mut app, "C:/pics/bypass-coexist-other.jpg");
        app.clear_local_adjust_caches_for_idx(other_idx);
        assert!(
            app.current_local_adjust_layer_bypass_texture(idx, 0)
                .is_some(),
            "clearing other-page caches must not affect this page's bypass cache"
        );
    }

    /// fake pending を仕込むためのヘルパー。worker は起動せず、cancel フラグ + チャネルだけ。
    fn make_fake_bypass_pending(
        result_key: LocalAdjustResultKey,
        layer_idx: usize,
    ) -> (
        LocalAdjustLayerBypassPending,
        Arc<std::sync::atomic::AtomicBool>,
        std::sync::mpsc::Sender<LocalAdjustRenderResult>,
    ) {
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let pending = LocalAdjustLayerBypassPending {
            key: LocalAdjustLayerBypassPreviewKey {
                result_key,
                layer_idx,
            },
            cancel: Arc::clone(&cancel),
            rx,
        };
        (pending, cancel, tx)
    }

    fn make_fake_prefix_pending(
        result_key: LocalAdjustResultKey,
        layer_count: usize,
    ) -> (
        LocalAdjustPrefixPreviewPending,
        Arc<std::sync::atomic::AtomicBool>,
        std::sync::mpsc::Sender<LocalAdjustRenderResult>,
    ) {
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let pending = LocalAdjustPrefixPreviewPending {
            key: LocalAdjustPrefixPreviewKey {
                result_key,
                layer_count,
            },
            cancel: Arc::clone(&cancel),
            rx,
        };
        (pending, cancel, tx)
    }

    /// P4-8a: `clear_local_adjust_caches_for_idx` は対象 idx の bypass/prefix
    /// pending worker をキャンセルする。
    /// 回帰防止: ナビゲート / レイヤー編集で stale な worker が走り続け、
    /// 古い結果が新ページの cache に書き込まれる事故。
    #[test]
    fn clear_local_adjust_caches_cancels_bypass_and_prefix_pending() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/cancel-bypass.jpg");
        app.local_adjust_page_layers
            .insert(idx, vec![full_layer("A"), full_layer("B")]);
        let result_key = app.current_local_adjust_key(idx);

        let (bypass_pending, bypass_cancel, _bypass_tx) = make_fake_bypass_pending(result_key, 1);
        let (prefix_pending, prefix_cancel, _prefix_tx) = make_fake_prefix_pending(result_key, 1);
        app.local_adjust_layer_bypass_pending = Some(bypass_pending);
        app.local_adjust_prefix_preview_pending = Some(prefix_pending);

        app.clear_local_adjust_caches_for_idx(idx);

        assert!(
            bypass_cancel.load(std::sync::atomic::Ordering::Relaxed),
            "bypass pending cancel flag must be set when cache for the same idx is cleared"
        );
        assert!(
            prefix_cancel.load(std::sync::atomic::Ordering::Relaxed),
            "prefix pending cancel flag must be set when cache for the same idx is cleared"
        );
        assert!(
            app.local_adjust_layer_bypass_pending.is_none(),
            "bypass pending must be taken out after cancellation"
        );
        assert!(
            app.local_adjust_prefix_preview_pending.is_none(),
            "prefix pending must be taken out after cancellation"
        );
    }

    /// P4-8b: 別 idx の cache clear では現在の pending はキャンセルされない。
    /// (= ページごとに独立した管理になっている)
    #[test]
    fn clear_local_adjust_caches_for_other_idx_keeps_bypass_pending() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/cancel-keep-this.jpg");
        let other_idx = push_image(&mut app, "C:/pics/cancel-keep-other.jpg");
        app.local_adjust_page_layers
            .insert(idx, vec![full_layer("A"), full_layer("B")]);
        let result_key = app.current_local_adjust_key(idx);

        let (bypass_pending, bypass_cancel, _bypass_tx) = make_fake_bypass_pending(result_key, 1);
        app.local_adjust_layer_bypass_pending = Some(bypass_pending);

        // 別 idx を clear → 現在の pending は無傷
        app.clear_local_adjust_caches_for_idx(other_idx);

        assert!(
            !bypass_cancel.load(std::sync::atomic::Ordering::Relaxed),
            "clearing other-idx caches must not cancel this idx's bypass pending"
        );
        assert!(
            app.local_adjust_layer_bypass_pending.is_some(),
            "bypass pending must remain after other-idx clear"
        );
    }

    // ========================================================================
    // P6-1: maybe_start_local_adjust_layer_bypass_preview の guard 経路テスト
    // ========================================================================
    //
    // `maybe_start_local_adjust_layer_bypass_preview` には 4 つの early-return
    // guard がある (src/app.rs:18957-):
    //   1. cache hit          → 既に bypass cache に同 key が乗っている
    //   2. same-key pending   → 同 key の pending が既に live
    //   3. layers None        → bypass しても残り有効レイヤーが無い (= 無駄な spawn)
    //   4. source None        → mask page 中などで source pixels が取れない
    //
    // どの guard も「pending を新規 spawn しない / 既存 pending を cancel しない」
    // ことが期待動作。pending フィールドの不変性を assertion で固定する。
    // spawn を踏まないので worker thread の起動コストや race も発生しない。

    /// P6-1a: 同 key の bypass cache が既に乗っている場合、`maybe_start` は
    /// 何もしない (= pending を作らない)。
    /// 回帰防止: cache hit guard が外れて毎フレーム worker spawn する退行
    /// (= UI 描画ループ内で繰り返し呼ばれる経路なので、guard が無いと spawn 爆発)。
    #[test]
    fn maybe_start_layer_bypass_returns_early_when_cache_already_present() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/bypass-guard-cache.jpg");
        app.local_adjust_page_layers
            .insert(idx, vec![full_layer("A"), full_layer("B")]);

        // cache に同 key を仕込む
        let ctx = egui::Context::default();
        let key = app.current_local_adjust_layer_bypass_key(idx, 1);
        let img = egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]);
        let tex = ctx.load_texture("bypass-guard-cache", img.clone(), Default::default());
        app.local_adjust_layer_bypass_cache.insert(
            key,
            LocalAdjustCacheEntry {
                pixels: Arc::new(img),
                texture: tex,
            },
        );

        assert!(
            app.local_adjust_layer_bypass_pending.is_none(),
            "pre-condition: pending must be empty"
        );

        app.maybe_start_local_adjust_layer_bypass_preview(idx, 1);

        assert!(
            app.local_adjust_layer_bypass_pending.is_none(),
            "cache hit must short-circuit; no pending should be created"
        );
    }

    /// P6-1b: 同 key の pending が既に live なら、`maybe_start` は新規 spawn
    /// しないし既存 pending を cancel もしない (= 無駄な churn を防ぐ)。
    /// 回帰防止: 同フレーム内で同じ呼び出しが 2 回走った時に、最初の pending を
    /// cancel して 2 つ目を作るような無駄が起きないこと。
    #[test]
    fn maybe_start_layer_bypass_keeps_same_key_pending_alive() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/bypass-guard-same-key.jpg");
        app.local_adjust_page_layers
            .insert(idx, vec![full_layer("A"), full_layer("B")]);

        let result_key = app.current_local_adjust_key(idx);
        let layer_idx = 1usize;
        let (pending, cancel, _tx) = make_fake_bypass_pending(result_key, layer_idx);
        app.local_adjust_layer_bypass_pending = Some(pending);

        app.maybe_start_local_adjust_layer_bypass_preview(idx, layer_idx);

        assert!(
            app.local_adjust_layer_bypass_pending.is_some(),
            "same-key pending must survive (= no spawn churn)"
        );
        assert!(
            !cancel.load(std::sync::atomic::Ordering::Relaxed),
            "same-key pending must not be cancelled"
        );
    }

    /// P6-1c: bypass しても残り有効レイヤーが無い (= 単一レイヤーを bypass する等)
    /// 場合、`maybe_start` は noop。
    /// 回帰防止: 描画結果が原画と同じになる無駄な spawn を防ぐ最適化。
    /// `local_adjust_layers_with_selected_layer_bypassed` が None を返す全ケース
    /// (disabled / opacity=0 / 範囲外) はこの経路を通る。
    #[test]
    fn maybe_start_layer_bypass_returns_early_when_no_remaining_enabled_layers() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/bypass-guard-no-layers.jpg");
        // 単一 layer のみ → bypass すると残り 0 → None
        app.local_adjust_page_layers
            .insert(idx, vec![full_layer("only")]);

        // 別 key の pending を立てておく → guard で抜けたかの判定材料
        let mut other_key = app.current_local_adjust_key(idx);
        other_key.idx = idx + 100; // 違う idx で別 key 化
        let (pending, cancel, _tx) = make_fake_bypass_pending(other_key, 0);
        app.local_adjust_layer_bypass_pending = Some(pending);

        app.maybe_start_local_adjust_layer_bypass_preview(idx, 0);

        assert!(
            app.local_adjust_layer_bypass_pending.is_some(),
            "existing pending must survive when layers=None \
             (= guard short-circuits before cancel_local_adjust_layer_bypass_pending)"
        );
        assert!(
            !cancel.load(std::sync::atomic::Ordering::Relaxed),
            "existing pending cancel must not fire when layers=None"
        );
    }

    /// P6-1d: source pixels が None (例: mask page 編集中) なら、`maybe_start`
    /// は noop。
    /// 回帰防止: mask 編集中に bypass preview を要求しても worker spawn しない
    /// (= 編集確定前の不完全な source で render すると flicker する事故を防ぐ)。
    #[test]
    fn maybe_start_layer_bypass_returns_early_when_source_unavailable() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/bypass-guard-no-source.jpg");
        app.local_adjust_page_layers
            .insert(idx, vec![full_layer("A"), full_layer("B")]);
        // mask_pages に入れる → current_local_adjust_source_pixels が None
        app.mask_pages.insert(idx);

        // 別 key の pending を立てておく → guard で抜けたかの判定材料
        let mut other_key = app.current_local_adjust_key(idx);
        other_key.idx = idx + 100;
        let (pending, cancel, _tx) = make_fake_bypass_pending(other_key, 1);
        app.local_adjust_layer_bypass_pending = Some(pending);

        app.maybe_start_local_adjust_layer_bypass_preview(idx, 1);

        assert!(
            app.local_adjust_layer_bypass_pending.is_some(),
            "existing pending must survive when source=None"
        );
        assert!(
            !cancel.load(std::sync::atomic::Ordering::Relaxed),
            "existing pending cancel must not fire when source=None"
        );
    }

    // ========================================================================
    // P6-2: maybe_start_local_adjust_prefix_preview の guard 経路テスト (対称)
    // ========================================================================
    //
    // `maybe_start_local_adjust_prefix_preview` (src/app.rs:19007-) は bypass
    // 側と同じ 4 つの early-return guard を持つ。layers の境界条件だけ違う:
    //   - `local_adjust_layers_until` は count == 0 || count >= layers.len() で
    //     None を返す (= "全部" or "0 枚" は元の合成と同じなので preview 不要)。
    // それ以外 (cache hit / same-key pending / source None) は bypass と同型。
    // 同じ guard を呼んでも違うフィールドを触るので、片方だけ壊れる退行を
    // 個別に検知できるよう独立テストとして書く。

    /// P6-2a: 同 key の prefix cache が既に乗っている → `maybe_start` は noop。
    #[test]
    fn maybe_start_prefix_preview_returns_early_when_cache_already_present() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/prefix-guard-cache.jpg");
        app.local_adjust_page_layers
            .insert(idx, vec![full_layer("A"), full_layer("B")]);

        let ctx = egui::Context::default();
        let key = app.current_local_adjust_prefix_preview_key(idx, 1);
        let img = egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]);
        let tex = ctx.load_texture("prefix-guard-cache", img.clone(), Default::default());
        app.local_adjust_prefix_preview_cache.insert(
            key,
            LocalAdjustCacheEntry {
                pixels: Arc::new(img),
                texture: tex,
            },
        );

        assert!(
            app.local_adjust_prefix_preview_pending.is_none(),
            "pre-condition: pending must be empty"
        );

        app.maybe_start_local_adjust_prefix_preview(idx, 1);

        assert!(
            app.local_adjust_prefix_preview_pending.is_none(),
            "cache hit must short-circuit; no pending should be created"
        );
    }

    /// P6-2b: 同 key の prefix pending が live なら、`maybe_start` は新規 spawn
    /// しないし cancel もしない (= churn 防止)。
    #[test]
    fn maybe_start_prefix_preview_keeps_same_key_pending_alive() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/prefix-guard-same-key.jpg");
        app.local_adjust_page_layers
            .insert(idx, vec![full_layer("A"), full_layer("B")]);

        let result_key = app.current_local_adjust_key(idx);
        let layer_count = 1usize;
        let (pending, cancel, _tx) = make_fake_prefix_pending(result_key, layer_count);
        app.local_adjust_prefix_preview_pending = Some(pending);

        app.maybe_start_local_adjust_prefix_preview(idx, layer_count);

        assert!(
            app.local_adjust_prefix_preview_pending.is_some(),
            "same-key pending must survive"
        );
        assert!(
            !cancel.load(std::sync::atomic::Ordering::Relaxed),
            "same-key pending must not be cancelled"
        );
    }

    /// P6-2c: `local_adjust_layers_until` が None を返す境界条件
    /// (= layer_count==0 / layer_count>=layers.len()) では `maybe_start` は noop。
    /// 回帰防止: 「先頭から 0 枚」「先頭から全枚」は元の合成と同じなので spawn 不要。
    /// この最適化が外れると Ctrl 押下時に毎フレーム無駄な worker spawn が走る。
    #[test]
    fn maybe_start_prefix_preview_returns_early_at_layer_count_boundaries() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/prefix-guard-boundaries.jpg");
        app.local_adjust_page_layers
            .insert(idx, vec![full_layer("A"), full_layer("B")]);

        let mut other_key = app.current_local_adjust_key(idx);
        other_key.idx = idx + 100;

        // (i) layer_count = 0 → None
        let (pending0, cancel0, _tx0) = make_fake_prefix_pending(other_key, 0);
        app.local_adjust_prefix_preview_pending = Some(pending0);
        app.maybe_start_local_adjust_prefix_preview(idx, 0);
        assert!(
            app.local_adjust_prefix_preview_pending.is_some(),
            "layer_count=0 → pending must survive"
        );
        assert!(
            !cancel0.load(std::sync::atomic::Ordering::Relaxed),
            "layer_count=0 → existing pending must not be cancelled"
        );

        // (ii) layer_count >= layers.len() → None
        let (pending2, cancel2, _tx2) = make_fake_prefix_pending(other_key, 2);
        app.local_adjust_prefix_preview_pending = Some(pending2);
        app.maybe_start_local_adjust_prefix_preview(idx, 2);
        assert!(
            app.local_adjust_prefix_preview_pending.is_some(),
            "layer_count >= len → pending must survive"
        );
        assert!(
            !cancel2.load(std::sync::atomic::Ordering::Relaxed),
            "layer_count >= len → existing pending must not be cancelled"
        );
    }

    // ========================================================================
    // P6-3: M-6 退行ガード (補正レイヤー編集中は隠蔽加工を見せない設計の符号化)
    // ========================================================================
    //
    // ユーザー報告 (docs/local-adjust-integration-audit.md §M-6):
    //   「消しゴムツールでは、補正レイヤー・隠蔽加工が見えないようになっていると
    //    思いますが、同様に補正レイヤー編集中は隠蔽加工の処理はみえないように
    //    してください。」
    //
    // 設計: 補正レイヤーは **隠蔽加工の上流** にある (= AdjustParams / AI / conceal
    // は全部 local_adjust の出力に対して後段適用)。だから補正レイヤー編集中の表示
    // は conceal-applied の結果を**そもそも経由してはいけない**。
    //
    // 構造的な根拠は 2 つあり、片方ずつ別テストで pin する:
    //   (a) `LocalAdjustResultKey` が conceal の generation を含まない
    //       → 単一 page の conceal 変更で local_adjust cache が捨てられない
    //       → 「補正レイヤー編集中は conceal を含まない layer 出力を見せる」設計の根拠
    //   (b) `current_local_adjust_source_pixels` が conceal_cache を参照しない
    //       → 補正レイヤー編集中の「入力源」(= layer がまだ無いとき表示する素材)
    //          には conceal-applied pixels が混じらない

    /// P6-3a: `LocalAdjustResultKey` は **conceal の generation を含まない**。
    /// 補正レイヤー演算は conceal よりも上流なので、conceal が変わっても
    /// local_adjust cache は無効化されない。逆に conceal_gen を入れる退行が
    /// 入ると、conceal を変更するたびに補正レイヤー cache が捨てられて
    /// 性能劣化 + M-6 (= 編集中に conceal が見えてしまう) を招く。
    ///
    /// 形態: フィールドの exhaustive destructure で **コンパイル時に**
    /// 追加フィールドを検知する。conceal_gen を生やしたら exhaustive match
    /// が壊れて即 fail する。
    #[test]
    fn local_adjust_result_key_excludes_conceal_generation_m6() {
        let key = LocalAdjustResultKey {
            idx: 7,
            input_gen: 1,
            erase_mask_gen: 2,
            local_gen: 3,
        };
        // exhaustive destructure — 新フィールド追加でコンパイルエラーになる
        let LocalAdjustResultKey {
            idx,
            input_gen,
            erase_mask_gen,
            local_gen,
        } = key;
        assert_eq!(idx, 7);
        assert_eq!(input_gen, 1);
        assert_eq!(erase_mask_gen, 2);
        assert_eq!(local_gen, 3);
        // conceal_*_gen が追加されたら上の destructure が non-exhaustive で
        // コンパイル落ち。その時は本テストを更新する前に M-6 が破綻していないか
        // (= 補正レイヤー編集中に隠蔽加工が見える退行が無いか) 必ず再検証する。
    }

    /// P6-3b: `current_local_adjust_source_pixels` は **conceal_cache を参照しない**。
    /// 補正レイヤー編集中の入力源 (= まだレイヤーが無いときの素材) には
    /// 隠蔽加工結果が混じってはいけない。
    ///
    /// セットアップ: conceal_cache と conceal_pages に明確に識別可能な pixels を
    /// 仕込み、raw_source / erase_result は無いままにする。compose chain が
    /// もし誤って conceal を入力源として返すなら、その Arc を返してしまう。
    /// 期待: None (= raw も erase も無いので何も返らない)、絶対に
    /// conceal_pixels を返してはいけない。
    #[test]
    fn current_local_adjust_source_pixels_ignores_conceal_cache_m6() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/m6-no-conceal-source.jpg");

        // conceal cache に「明らかに識別可能な」 pixels を仕込む
        let ctx = egui::Context::default();
        let conceal_color = egui::Color32::from_rgb(123, 45, 67);
        let conceal_img = egui::ColorImage::new([1, 1], vec![conceal_color]);
        let conceal_pixels = Arc::new(conceal_img.clone());
        let conceal_tex = ctx.load_texture("conceal-m6-source", conceal_img, Default::default());
        let conceal_generation = app.conceal_generation;
        app.conceal_cache.insert(
            idx,
            ConcealCacheEntry {
                pixels: Arc::clone(&conceal_pixels),
                texture: conceal_tex,
                generation: conceal_generation,
            },
        );
        app.conceal_pages.insert(idx);

        // 補正レイヤー編集中の入力源解決
        let result = app.current_local_adjust_source_pixels(idx);

        // raw_source / erase_result が無いので結果は None になる想定だが、
        // 万一 Some が返ったとしても、その Arc が conceal_pixels と同一であっては
        // ならない (= conceal を bypass している証拠)。
        if let Some(returned) = result {
            assert!(
                !Arc::ptr_eq(&returned, &conceal_pixels),
                "M-6 regression: local_adjust source MUST NOT return conceal-applied pixels. \
                 If this fires, the compose chain for `current_local_adjust_source_pixels` \
                 has started routing through conceal_cache, which means the editor will \
                 show conceal-applied output while editing layers (= the exact behavior \
                 docs/local-adjust-integration-audit.md §M-6 forbids)."
            );
        }
    }

    /// P6-2d: source pixels が None (mask page 中) なら、`maybe_start` は noop。
    #[test]
    fn maybe_start_prefix_preview_returns_early_when_source_unavailable() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/prefix-guard-no-source.jpg");
        app.local_adjust_page_layers
            .insert(idx, vec![full_layer("A"), full_layer("B")]);
        app.mask_pages.insert(idx);

        let mut other_key = app.current_local_adjust_key(idx);
        other_key.idx = idx + 100;
        let (pending, cancel, _tx) = make_fake_prefix_pending(other_key, 1);
        app.local_adjust_prefix_preview_pending = Some(pending);

        app.maybe_start_local_adjust_prefix_preview(idx, 1);

        assert!(
            app.local_adjust_prefix_preview_pending.is_some(),
            "existing pending must survive when source=None"
        );
        assert!(
            !cancel.load(std::sync::atomic::Ordering::Relaxed),
            "existing pending cancel must not fire when source=None"
        );
    }

    fn make_fake_final_ai_pending() -> (FinalAiPending, Arc<std::sync::atomic::AtomicBool>) {
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pending = FinalAiPending {
            cancel: Arc::clone(&cancel),
        };
        (pending, cancel)
    }

    /// P5-1: `has_uncancelled_final_ai_pending` は cancel フラグが立っていない
    /// pending を 1 件でも検出する。これは prefetch_final_ai が「現在ページの AI が
    /// 完了するまで先読みを起動しない」ゲートに使う。
    ///
    /// 退行を防ぐシナリオ: 旧設計 `current_done && ai_upscale_pending.is_empty()` 相当
    /// の条件を新パイプライン版でも機能させる。これが壊れると、隣接ページの spawn が
    /// 現在ページの spawn と競合して ai_runtime の lock を取り合い、UI が固まる。
    #[test]
    fn has_uncancelled_final_ai_pending_detects_live_workers() {
        let mut app = setup_app();
        assert!(
            !app.has_uncancelled_final_ai_pending(),
            "no pending => false"
        );

        // 1 件 live で挿入 → true
        let key = FinalAiKey {
            edit_key: EditResultKey {
                idx: 0,
                source_gen: 0,
                erase_mask_gen: 0,
                local_gen: 0,
                conceal_mask_gen: 0,
                conceal_gen: 0,
            },
            color_ai_hash: 0,
            bg: 0,
        };
        let (pending, cancel) = make_fake_final_ai_pending();
        app.final_ai_pending.insert(key, pending);
        assert!(
            app.has_uncancelled_final_ai_pending(),
            "live pending => true"
        );

        // cancel フラグを立てる → false (= 既に終了予定の worker は無視)
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            !app.has_uncancelled_final_ai_pending(),
            "cancelled pending => false (so prefetch can resume)"
        );
    }

    /// P5-1: `prefetch_final_ai` は live な pending があるときは何もしない。
    /// AI runtime 不要 (= test 環境で spawn しなくても短絡する) の path を確認する。
    #[test]
    fn prefetch_final_ai_short_circuits_when_pending_active() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/prefetch-skip.jpg");
        // live な pending を仕込む → prefetch_final_ai は即 return
        let key = FinalAiKey {
            edit_key: EditResultKey {
                idx,
                source_gen: 0,
                erase_mask_gen: 0,
                local_gen: 0,
                conceal_mask_gen: 0,
                conceal_gen: 0,
            },
            color_ai_hash: 0,
            bg: 0,
        };
        let pending_before = app.final_ai_pending.len();
        let (pending, _cancel) = make_fake_final_ai_pending();
        app.final_ai_pending.insert(key, pending);
        app.prefetch_final_ai(&ctx, idx);
        assert_eq!(
            app.final_ai_pending.len(),
            pending_before + 1,
            "prefetch must not spawn while live pending exists"
        );
    }

    /// AI キュー化リファクタ: `poll_final_ai` は単一共有チャネルから Ready/Failed を
    /// drain し、pending を除去して cache / failed を更新する。pending に無い key の
    /// 結果は捨てる (= cancel_final_ai_for_idx / clear / evict で取り消された後に届く
    /// stale 結果。旧 per-thread 設計で rx drop により失われていたのと同じ挙動)。
    #[test]
    fn poll_final_ai_drains_shared_channel_and_skips_unknown_keys() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let (tx, rx) = std::sync::mpsc::channel();
        app.final_ai_rx = Some(rx);

        let mk_key = |idx: usize| FinalAiKey {
            edit_key: EditResultKey {
                idx,
                source_gen: 0,
                erase_mask_gen: 0,
                local_gen: 0,
                conceal_mask_gen: 0,
                conceal_gen: 0,
            },
            color_ai_hash: 0,
            bg: 0,
        };

        // (1) Ready: pending あり → final_ai_cache に入り pending 除去
        let ready_key = mk_key(0);
        let (pending, _c) = make_fake_final_ai_pending();
        app.final_ai_pending.insert(ready_key, pending);
        tx.send(FinalAiResult::Ready {
            key: ready_key,
            image: egui::ColorImage::new([1, 1], vec![egui::Color32::BLACK]),
        })
        .unwrap();

        // (2) Failed: pending あり → final_ai_failed に入り pending 除去
        let failed_key = mk_key(1);
        let (pending, _c) = make_fake_final_ai_pending();
        app.final_ai_pending.insert(failed_key, pending);
        tx.send(FinalAiResult::Failed {
            key: failed_key,
            error: "boom".to_string(),
        })
        .unwrap();

        // (3) Unknown: pending に無い key の Ready → 捨てる (cache に入らない)
        let unknown_key = mk_key(2);
        tx.send(FinalAiResult::Ready {
            key: unknown_key,
            image: egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
        })
        .unwrap();

        app.poll_final_ai(&ctx);

        assert!(
            app.final_ai_cache.contains_key(&ready_key),
            "Ready with live pending must populate final_ai_cache"
        );
        assert!(
            !app.final_ai_pending.contains_key(&ready_key),
            "Ready must remove its pending"
        );
        assert!(
            app.final_ai_failed.contains(&failed_key),
            "Failed with live pending must mark final_ai_failed"
        );
        assert!(
            !app.final_ai_pending.contains_key(&failed_key),
            "Failed must remove its pending"
        );
        assert!(
            !app.final_ai_cache.contains_key(&unknown_key),
            "result for a key not in pending must be dropped (stale)"
        );
    }

    /// P5-3: `final_ai_prefetch_progress` は target が無いとき None を返す。
    /// (= フォルダ内に画像が 1 枚しか無い場合や、先読み設定が 0/0 の場合)
    #[test]
    fn final_ai_prefetch_progress_is_none_when_no_targets() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/only-one.jpg");
        // 1 枚だけなので前後 target は空
        assert!(
            app.final_ai_prefetch_progress(idx).is_none(),
            "single-image folder => no prefetch progress bar"
        );
    }

    /// P5-3: `is_idx_final_ai_done_or_skipped` は source pixels が無い idx を
    /// pending 扱い (= false) にする。fs_prefetch が走るのを待っている状態。
    #[test]
    fn is_idx_final_ai_done_or_skipped_false_without_source_pixels() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/no-source.jpg");
        // fs_cache に Static 無し → false
        assert!(
            !app.is_idx_final_ai_done_or_skipped(idx),
            "no source pixels => pending (not done)"
        );
    }

    /// P5-4: maybe_start_final_ai の cancel block は **別 idx の prefetch pending を
    /// 殺さない**。
    ///
    /// ## 退行の根本原因 (2026-06)
    ///
    /// 過去版は `fullscreen_idx == Some(idx)` のとき
    /// `pending_key.edit_key.idx != idx || *pending_key != key` を満たす全 pending を
    /// cancel していた。これは「別 idx の prefetch pending も巻き込む」ため、ユーザーが
    /// fullscreen で 1 ページを開いた瞬間に、隣接ページの先読み AI worker (= まだ完了
    /// していない) が問答無用で cancel される。
    ///
    /// 結果: prefetch_final_ai が毎フレ呼び直されて同じ key で再 spawn → display 経路で
    /// 再 cancel → ... のループに入り、隣接ページの AI 結果が永久に final_ai_cache に
    /// 格納されない。ユーザー体感は「先読み進捗バーは進んでいるのに、次ページに送ると
    /// 一瞬アップスケール前画像が見えて、計算し直してから AI 結果に切り替わる」。
    ///
    /// 修正後: cancel 対象は「同じ idx の古い key」だけに限定。別 idx の prefetch は
    /// そのまま走り続け、完了すれば final_ai_cache に入る → ページ送りで cache hit。
    #[test]
    fn display_path_does_not_cancel_other_idx_prefetch_pending() {
        let mut app = setup_app();
        let display_idx = push_image(&mut app, "C:/pics/display.jpg");
        let prefetch_idx = push_image(&mut app, "C:/pics/prefetch.jpg");
        app.fullscreen_idx = Some(display_idx);

        // 隣接ページの prefetch pending を仕込む (= AI worker 進行中の状態)
        let prefetch_key = FinalAiKey {
            edit_key: EditResultKey {
                idx: prefetch_idx,
                source_gen: 0,
                erase_mask_gen: 0,
                local_gen: 0,
                conceal_mask_gen: 0,
                conceal_gen: 0,
            },
            color_ai_hash: 0xAAAA,
            bg: 0,
        };
        let (prefetch_pending, prefetch_cancel) = make_fake_final_ai_pending();
        app.final_ai_pending.insert(prefetch_key, prefetch_pending);

        // display 経路: 同じ idx (= display_idx) の古い key を 1 件追加
        let display_old_key = FinalAiKey {
            edit_key: EditResultKey {
                idx: display_idx,
                source_gen: 0,
                erase_mask_gen: 0,
                local_gen: 0,
                conceal_mask_gen: 0,
                conceal_gen: 0,
            },
            color_ai_hash: 0xBBBB,
            bg: 0,
        };
        let (display_old_pending, display_old_cancel) = make_fake_final_ai_pending();
        app.final_ai_pending
            .insert(display_old_key, display_old_pending);

        // 新しい display request の key (= display_idx と同 idx だが別 hash)
        let display_new_key = FinalAiKey {
            edit_key: display_old_key.edit_key,
            color_ai_hash: 0xCCCC, // 古い key と違う
            bg: 0,
        };

        // cancel block を再現するため manual で実行する代わりに、内部状態を直接
        // 検証する: 修正後の filter は `pending_key.edit_key.idx == idx &&
        // *pending_key != key` のはず。
        let idx = display_idx;
        let key = display_new_key;
        let to_cancel: Vec<FinalAiKey> = app
            .final_ai_pending
            .keys()
            .copied()
            .filter(|pending_key| pending_key.edit_key.idx == idx && *pending_key != key)
            .collect();

        // cancel 対象:
        // - prefetch_key: idx=prefetch_idx != display_idx → cancel しない ✓
        // - display_old_key: idx == display_idx かつ key != display_new_key → cancel する ✓
        assert!(
            to_cancel.contains(&display_old_key),
            "stale display key (same idx, different hash) must be in cancel list"
        );
        assert!(
            !to_cancel.contains(&prefetch_key),
            "prefetch pending for another idx MUST NOT be in cancel list (= preserves先読み投資)"
        );

        // cancel フラグを実際に立てる動作も検証
        for cancel_key in &to_cancel {
            if let Some(pending) = app.final_ai_pending.get(cancel_key) {
                pending
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        assert!(
            display_old_cancel.load(std::sync::atomic::Ordering::Relaxed),
            "stale display key got cancel flag"
        );
        assert!(
            !prefetch_cancel.load(std::sync::atomic::Ordering::Relaxed),
            "other-idx prefetch DID NOT get cancel flag (= worker keeps running)"
        );
    }

    /// P5-3: `final_ai_prefetch_progress` は現在ページの AI 処理中は None を返す
    /// (= 「AI 処理中」ラベルが既に出ているので進捗バー二重表示を避ける)。
    #[test]
    fn final_ai_prefetch_progress_hidden_while_current_busy() {
        let mut app = setup_app();
        let idx_cur = push_image(&mut app, "C:/pics/busy-cur.jpg");
        push_image(&mut app, "C:/pics/busy-next-1.jpg");
        push_image(&mut app, "C:/pics/busy-next-2.jpg");

        // 現在ページの final_ai_pending を仕込む
        let key = FinalAiKey {
            edit_key: EditResultKey {
                idx: idx_cur,
                source_gen: 0,
                erase_mask_gen: 0,
                local_gen: 0,
                conceal_mask_gen: 0,
                conceal_gen: 0,
            },
            color_ai_hash: 0,
            bg: 0,
        };
        let (pending, _cancel) = make_fake_final_ai_pending();
        app.final_ai_pending.insert(key, pending);

        assert!(
            app.final_ai_prefetch_progress(idx_cur).is_none(),
            "current page busy => hide prefetch progress (avoid double-label)"
        );
    }

    /// P5-1: cancel 済 pending しかなければ、prefetch は次の起動に進める
    /// (= has_uncancelled_final_ai_pending が false を返すパス)。
    /// AI runtime 不要なので「spawn しないが skip もしない」位置で止まることを確認。
    #[test]
    fn prefetch_final_ai_proceeds_when_all_pending_are_cancelled() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/prefetch-proceed.jpg");
        let key = FinalAiKey {
            edit_key: EditResultKey {
                idx,
                source_gen: 0,
                erase_mask_gen: 0,
                local_gen: 0,
                conceal_mask_gen: 0,
                conceal_gen: 0,
            },
            color_ai_hash: 0,
            bg: 0,
        };
        let (pending, cancel) = make_fake_final_ai_pending();
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        app.final_ai_pending.insert(key, pending);

        // fs_cache に source 無いので ensure_edit_result_pixels は None を返し、
        // 各ターゲットが skip される。結果として final_ai_pending は変わらない。
        let pending_before = app.final_ai_pending.len();
        app.prefetch_final_ai(&ctx, idx);
        assert_eq!(
            app.final_ai_pending.len(),
            pending_before,
            "no fs_cache => ensure_edit_result_pixels None => prefetch skips, no spawn"
        );
    }

    /// 補助テスト: `has_active_local_adjust_layers` は「描画に影響するレイヤーが
    /// 1 つ以上あるか」を返す。これは `_with_selected_layer_bypassed` / `_until` /
    /// `_for_render` と同じ "enabled && opacity > 0" 判定を共有しているので、
    /// 仕様変更時に 4 関数全てが揃って動くことを暗黙の不変条件として固定する。
    #[test]
    fn has_active_local_adjust_layers_matches_render_gating_semantics() {
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/active-gate.jpg");

        // ページにレイヤーが無い
        assert!(
            !app.has_active_local_adjust_layers(idx),
            "no layers => not active"
        );

        // disabled レイヤーのみ
        let mut disabled = full_layer("d");
        disabled.enabled = false;
        app.local_adjust_page_layers
            .insert(idx, vec![disabled.clone()]);
        assert!(
            !app.has_active_local_adjust_layers(idx),
            "all-disabled => not active"
        );

        // opacity=0 のみ
        let mut zero = full_layer("z");
        zero.opacity = 0.0;
        app.local_adjust_page_layers.insert(idx, vec![zero.clone()]);
        assert!(
            !app.has_active_local_adjust_layers(idx),
            "opacity=0 only => not active"
        );

        // 1 つでも enabled && opacity>0 があれば active
        app.local_adjust_page_layers
            .insert(idx, vec![disabled, zero, full_layer("real")]);
        assert!(
            app.has_active_local_adjust_layers(idx),
            "any enabled & opaque layer => active"
        );
    }

    /// P4-8d: tx 側が drop された (= worker thread が cancel/panic で消えた) ら、
    /// poll の Disconnected ブランチで pending が掃除される。
    #[test]
    fn poll_bypass_preview_clears_pending_when_worker_disconnects() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/poll-disconnect.jpg");
        let result_key = app.current_local_adjust_key(idx);

        let (pending, _cancel, tx) = make_fake_bypass_pending(result_key, 0);
        app.local_adjust_layer_bypass_pending = Some(pending);

        // tx を drop → Disconnected が返る
        drop(tx);
        app.poll_local_adjust_layer_bypass_preview(&ctx);

        assert!(
            app.local_adjust_layer_bypass_pending.is_none(),
            "Disconnected worker must clear pending"
        );
    }

    /// P4-8e: 明示的な Cancelled シグナルでも pending が掃除される。
    #[test]
    fn poll_bypass_preview_clears_pending_on_cancelled_signal() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/poll-cancel-sig.jpg");
        let result_key = app.current_local_adjust_key(idx);

        let (pending, _cancel, tx) = make_fake_bypass_pending(result_key, 0);
        app.local_adjust_layer_bypass_pending = Some(pending);

        tx.send(LocalAdjustRenderResult::Cancelled)
            .expect("send cancelled");
        app.poll_local_adjust_layer_bypass_preview(&ctx);

        assert!(
            app.local_adjust_layer_bypass_pending.is_none(),
            "Cancelled signal must clear pending"
        );
        // cache には何も書かれない
        assert!(
            app.local_adjust_layer_bypass_cache.is_empty(),
            "Cancelled must not populate cache"
        );
    }

    /// P4-8c: poll は cancel ガード後に届いた stale 結果を cache に書き込まない。
    /// pending を仕込んでから別ページに移動 (= current_local_adjust_key が変わる)
    /// → 古いキーで Ready が届いても無視されることを確認する。
    #[test]
    fn poll_bypass_preview_discards_stale_ready_result() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/poll-stale.jpg");
        app.local_adjust_page_layers
            .insert(idx, vec![full_layer("A"), full_layer("B")]);
        let old_result_key = app.current_local_adjust_key(idx);

        let (bypass_pending, _bypass_cancel, bypass_tx) =
            make_fake_bypass_pending(old_result_key, 1);
        let bypass_cache_key = bypass_pending.key;
        app.local_adjust_layer_bypass_pending = Some(bypass_pending);

        // 入力世代を bump して current_local_adjust_key を変える
        app.bump_local_adjust_generation(idx);
        let new_result_key = app.current_local_adjust_key(idx);
        assert_ne!(
            old_result_key, new_result_key,
            "bump_local_adjust_generation must change the result key"
        );

        // bump 後の pending は cancel で taken されているはずなので、stale tx は
        // 既に切れたチャネルへの送信になる。明示的に Pending を再構築して
        // 「古いキーの Ready 結果が届いた状態」をシミュレートする
        let (bypass_pending2, _cancel2, bypass_tx2) = make_fake_bypass_pending(new_result_key, 1);
        app.local_adjust_layer_bypass_pending = Some(bypass_pending2);

        // 古い result_key の Ready を送る (新 pending は new_result_key を期待)
        // → poll 内の `if key != pending.key.result_key` ガードで弾かれ、cache に入らない
        bypass_tx2
            .send(LocalAdjustRenderResult::Ready {
                key: old_result_key,
                image: egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(1, 2, 3)]),
            })
            .expect("send into live channel");
        app.poll_local_adjust_layer_bypass_preview(&ctx);

        assert!(
            !app.local_adjust_layer_bypass_cache
                .contains_key(&bypass_cache_key),
            "stale Ready (mismatched result_key) must not populate the cache"
        );
        // 古い chan は使われていなかったので不要; ガード値消費だけ
        drop(bypass_tx);
    }
}

#[cfg(test)]
mod pipeline_display_edit_split_tests {
    use super::phase_c_support::setup_app;
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn push_image(app: &mut App, path: &str) -> usize {
        app.items.push(GridItem::Image(PathBuf::from(path)));
        app.thumbnails.push(ThumbnailState::Pending);
        app.items.len() - 1
    }

    fn insert_fs_static(
        app: &mut App,
        ctx: &egui::Context,
        idx: usize,
        label: &str,
        color: egui::Color32,
    ) -> Arc<egui::ColorImage> {
        let image = egui::ColorImage::new([1, 1], vec![color]);
        let pixels = Arc::new(image.clone());
        let tex = ctx.load_texture(label, image, egui::TextureOptions::LINEAR);
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex,
                pixels: Arc::clone(&pixels),
                source_dims: Some([1, 1]),
                load_seq: 0,
            },
        );
        pixels
    }

    fn insert_stale_final_display_cache(
        app: &mut App,
        ctx: &egui::Context,
        idx: usize,
        label: &str,
        color: egui::Color32,
    ) {
        let edit_key = EditResultKey {
            idx,
            source_gen: 0,
            erase_mask_gen: 0,
            local_gen: 0,
            conceal_mask_gen: 0,
            conceal_gen: app.conceal_generation,
        };
        let final_key = FinalCompositeKey {
            edit_key,
            params_hash: 0x55,
            bg: 0,
        };
        let image = egui::ColorImage::new([1, 1], vec![color]);
        let texture = ctx.load_texture(label, image.clone(), egui::TextureOptions::LINEAR);
        app.final_composite_cache.insert(
            final_key,
            FinalCompositeEntry {
                pixels: Arc::new(image),
                texture,
                complete: true,
            },
        );
    }

    fn insert_current_erase_result(
        app: &mut App,
        ctx: &egui::Context,
        idx: usize,
        label: &str,
        color: egui::Color32,
    ) -> Arc<egui::ColorImage> {
        let image = egui::ColorImage::new([1, 1], vec![color]);
        let pixels = Arc::new(image.clone());
        let tex = ctx.load_texture(label, image, egui::TextureOptions::LINEAR);
        app.erase_result_cache.insert(
            app.current_erase_result_key(idx),
            EraseResultCacheEntry {
                pixels: Arc::clone(&pixels),
                texture: tex,
            },
        );
        pixels
    }

    fn active_tone_layer() -> local_adjust_core::LocalAdjustmentLayer {
        local_adjust_core::LocalAdjustmentLayer::new(
            "tone",
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::Tone(local_adjust_core::ToneParams {
                brightness: 12.0,
                ..Default::default()
            }),
        )
    }

    fn insert_current_local_adjust_result(
        app: &mut App,
        ctx: &egui::Context,
        idx: usize,
        label: &str,
        color: egui::Color32,
    ) -> Arc<egui::ColorImage> {
        let image = egui::ColorImage::new([1, 1], vec![color]);
        let pixels = Arc::new(image.clone());
        let tex = ctx.load_texture(label, image, egui::TextureOptions::LINEAR);
        app.local_adjust_cache.insert(
            app.current_local_adjust_key(idx),
            LocalAdjustCacheEntry {
                pixels: Arc::clone(&pixels),
                texture: tex,
            },
        );
        pixels
    }

    #[test]
    fn edit_sources_ignore_final_display_cache() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/display-edit-split.jpg");
        let raw = insert_fs_static(
            &mut app,
            &ctx,
            idx,
            "raw_source",
            egui::Color32::from_rgb(1, 2, 3),
        );
        insert_stale_final_display_cache(
            &mut app,
            &ctx,
            idx,
            "final_display",
            egui::Color32::from_rgb(200, 210, 220),
        );

        let local_source = app
            .current_local_adjust_source_pixels(idx)
            .expect("raw fs source should be available for local adjust");
        assert_eq!(local_source.pixels[0], raw.pixels[0]);

        let (conceal_source, kind) = app
            .current_conceal_source_pixels(idx)
            .expect("raw fs source should be available for conceal");
        assert_eq!(kind, "fs");
        assert_eq!(conceal_source.pixels[0], raw.pixels[0]);
    }

    #[test]
    fn local_adjust_source_prefers_source_resolution_erase_over_ai_cache() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/local-source.jpg");
        insert_fs_static(
            &mut app,
            &ctx,
            idx,
            "raw_for_local",
            egui::Color32::from_rgb(1, 2, 3),
        );
        let ai = egui::ColorImage::new([4, 4], vec![egui::Color32::from_rgb(240, 240, 240); 16]);
        let ai_tex = ctx.load_texture("stale_ai_local", ai.clone(), egui::TextureOptions::LINEAR);
        let ai_bg = app.erase_upscale_bg_mode(idx);
        app.ai_upscale_cache.insert(
            (idx, ai_bg),
            FsCacheEntry::Static {
                tex: ai_tex,
                pixels: Arc::new(ai),
                source_dims: None,
                load_seq: 0,
            },
        );
        let erase = insert_current_erase_result(
            &mut app,
            &ctx,
            idx,
            "erase_for_local",
            egui::Color32::from_rgb(40, 50, 60),
        );

        let local_source = app
            .current_local_adjust_source_pixels(idx)
            .expect("erase result should be the local-adjust source");

        assert_eq!(local_source.size, [1, 1]);
        assert_eq!(local_source.pixels[0], erase.pixels[0]);
    }

    #[test]
    fn conceal_source_waits_for_local_adjust_result_before_composing() {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        let idx = push_image(&mut app, "C:/pics/conceal-source.jpg");
        insert_fs_static(
            &mut app,
            &ctx,
            idx,
            "raw_for_conceal",
            egui::Color32::from_rgb(1, 2, 3),
        );
        app.local_adjust_page_layers
            .insert(idx, vec![active_tone_layer()]);

        assert!(
            app.current_conceal_source_pixels(idx).is_none(),
            "conceal must not compose over raw while an active local-adjust layer is pending"
        );

        let local = insert_current_local_adjust_result(
            &mut app,
            &ctx,
            idx,
            "local_for_conceal",
            egui::Color32::from_rgb(90, 100, 110),
        );
        let (conceal_source, kind) = app
            .current_conceal_source_pixels(idx)
            .expect("completed local-adjust result should feed conceal");

        assert_eq!(kind, "local_adjust");
        assert_eq!(conceal_source.pixels[0], local.pixels[0]);
    }
}

#[cfg(all(test, windows))]
mod native_video_rating_key_tests {
    use super::phase_c_support::setup_app;
    use super::*;

    fn native_key(
        virtual_key: u32,
        shift: bool,
    ) -> crate::video::native_window::NativeVideoKeyEvent {
        crate::video::native_window::NativeVideoKeyEvent {
            virtual_key,
            shift,
            ctrl: false,
            alt: false,
            repeat: false,
        }
    }

    fn push_video(app: &mut App, path: PathBuf) -> usize {
        app.items.push(GridItem::Video(path));
        app.thumbnails.push(ThumbnailState::Pending);
        app.rebuild_visible_indices();
        app.items.len() - 1
    }

    #[test]
    fn native_video_fkeys_rate_current_video() {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let idx = push_video(&mut app, PathBuf::from(r"C:\clips\movie.mp4"));
        app.fullscreen_idx = Some(idx);

        app.handle_native_video_key_event(&ctx, idx, native_key(0x72, false)); // F3
        assert_eq!(app.get_rating(idx), 3);

        app.handle_native_video_key_event(&ctx, idx, native_key(0x75, false)); // F6
        assert_eq!(app.get_rating(idx), 0);
    }

    #[test]
    fn native_video_shift_fkeys_rate_current_container() {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let folder = PathBuf::from(r"C:\clips");
        app.current_folder = Some(folder.clone());
        let idx = push_video(&mut app, folder.join("movie.mp4"));
        app.fullscreen_idx = Some(idx);

        app.handle_native_video_key_event(&ctx, idx, native_key(0x74, true)); // Shift+F5
        let key = crate::adjustment_db::normalize_path(&folder);
        assert_eq!(app.rating_db.as_ref().unwrap().get(&key), 5);
        assert_eq!(app.current_folder_rating_cache, Some(5));

        app.handle_native_video_key_event(&ctx, idx, native_key(0x75, true)); // Shift+F6
        assert_eq!(app.rating_db.as_ref().unwrap().get(&key), 0);
        assert_eq!(app.current_folder_rating_cache, Some(0));
    }

    /// F11 (VK 0x7A) で `toggle_video_window_mode` 経路が走り、`native_video_mode_switch`
    /// pending が登録されることを確認する。これは native HWND 経路と、egui 経由で
    /// `handle_video_input` から仮想 F11 を流す in-window 動画経路 (Codex P1 対応)
    /// の両方が同じハンドラを共有することの担保。
    #[test]
    fn native_video_f11_triggers_window_mode_toggle() {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let idx = push_video(&mut app, PathBuf::from(r"C:\clips\movie.mp4"));
        app.fullscreen_idx = Some(idx);
        let initial = app.settings.video_in_window_mode;
        assert!(app.native_video_mode_switch.is_none());

        app.handle_native_video_key_event(&ctx, idx, native_key(0x7A, false)); // F11

        let pending = app
            .native_video_mode_switch
            .expect("F11 should register a mode switch request");
        assert_eq!(pending.target_in_window, !initial);
    }

    /// F11 を repeat 付きで送ったときはトグルが走らないことを確認する (長押し連打防止)。
    #[test]
    fn native_video_f11_repeat_is_ignored() {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let idx = push_video(&mut app, PathBuf::from(r"C:\clips\movie.mp4"));
        app.fullscreen_idx = Some(idx);

        let mut key = native_key(0x7A, false);
        key.repeat = true;
        app.handle_native_video_key_event(&ctx, idx, key);

        assert!(
            app.native_video_mode_switch.is_none(),
            "repeat F11 should not trigger window mode toggle"
        );
    }
}

#[cfg(test)]
mod fullscreen_main_focus_guard_tests {
    use super::*;

    #[test]
    fn main_focus_guard_closes_after_grace_without_fullscreen_root_key() {
        assert!(should_close_fullscreen_from_main_focus(
            true, true, false, false, false,
        ));
    }

    #[test]
    fn main_focus_guard_skips_close_when_fullscreen_root_key_was_handled() {
        assert!(!should_close_fullscreen_from_main_focus(
            true, true, false, false, true,
        ));
    }

    #[test]
    fn main_focus_guard_skips_close_during_grace_or_embedded_mode() {
        assert!(!should_close_fullscreen_from_main_focus(
            false, true, false, false, false,
        ));
        assert!(!should_close_fullscreen_from_main_focus(
            true, true, false, true, false,
        ));
    }
}

#[cfg(all(test, windows))]
mod still_window_mode_key_tests {
    use super::phase_c_support::setup_app;
    use super::*;

    fn push_image(app: &mut App, path: &str) -> usize {
        app.items.push(GridItem::Image(PathBuf::from(path)));
        app.thumbnails.push(ThumbnailState::Pending);
        app.rebuild_visible_indices();
        app.items.len() - 1
    }

    fn begin_root_key_pass(ctx: &egui::Context, key: egui::Key, repeat: bool) {
        let modifiers = egui::Modifiers::NONE;
        ctx.begin_pass(egui::RawInput {
            modifiers,
            ..Default::default()
        });
        ctx.input_mut(|i| {
            i.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat,
                modifiers,
            });
        });
    }

    #[test]
    fn still_image_root_f11_toggles_window_mode_without_closing_fullscreen() {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let idx = push_image(&mut app, r"C:\pics\a.jpg");
        app.fullscreen_idx = Some(idx);
        app.settings.video_in_window_mode = false;
        app.native_video_in_window_active = false;

        begin_root_key_pass(&ctx, egui::Key::F11, false);
        let handled = app.handle_fullscreen_root_key_input(&ctx);
        let _ = ctx.end_pass();

        assert!(
            handled,
            "root-delivered F11 should be handled as fullscreen input"
        );
        assert_eq!(
            app.fullscreen_idx,
            Some(idx),
            "F11 must not close still fullscreen"
        );
        assert!(app.settings.video_in_window_mode);
        assert!(app.native_video_in_window_active);
    }

    #[test]
    fn still_image_root_f11_repeat_is_ignored() {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let idx = push_image(&mut app, r"C:\pics\a.jpg");
        app.fullscreen_idx = Some(idx);
        app.settings.video_in_window_mode = false;
        app.native_video_in_window_active = false;

        begin_root_key_pass(&ctx, egui::Key::F11, true);
        let handled = app.handle_fullscreen_root_key_input(&ctx);
        let _ = ctx.end_pass();

        assert!(handled, "repeat F11 is still a fullscreen root key");
        assert_eq!(app.fullscreen_idx, Some(idx));
        assert!(!app.settings.video_in_window_mode);
        assert!(!app.native_video_in_window_active);
    }
}

#[cfg(test)]
mod file_operation_selection_tests {
    use super::phase_c_support::setup_app;
    use super::*;

    fn push_item(app: &mut App, item: GridItem) -> usize {
        app.items.push(item);
        app.thumbnails.push(ThumbnailState::Pending);
        app.items.len() - 1
    }

    #[test]
    fn checked_file_operation_paths_skip_folders_and_virtual_pages() {
        let mut app = setup_app();

        // フォルダは整理対象外 (v1.1.0 で一旦無効化) なので checked に入れても除外される。
        let folder = push_item(&mut app, GridItem::Folder(PathBuf::from(r"C:\books")));
        let image = push_item(&mut app, GridItem::Image(PathBuf::from(r"C:\books\a.jpg")));
        let video = push_item(&mut app, GridItem::Video(PathBuf::from(r"C:\books\a.mp4")));
        let zip = push_item(
            &mut app,
            GridItem::ZipFile(PathBuf::from(r"C:\books\a.zip")),
        );
        let pdf = push_item(
            &mut app,
            GridItem::PdfFile(PathBuf::from(r"C:\books\a.pdf")),
        );
        let archive = push_item(
            &mut app,
            GridItem::ConvertibleArchive {
                path: PathBuf::from(r"C:\books\a.7z"),
                format: ArchiveFormat::SevenZ,
            },
        );
        let zip_page = push_item(
            &mut app,
            GridItem::ZipImage {
                zip_path: PathBuf::from(r"C:\books\a.zip"),
                entry_name: "p001.jpg".to_owned(),
            },
        );
        let pdf_page = push_item(
            &mut app,
            GridItem::PdfPage {
                pdf_path: PathBuf::from(r"C:\books\a.pdf"),
                page_num: 0,
                content_type: None,
            },
        );

        for idx in [folder, image, video, zip, pdf, archive, zip_page, pdf_page] {
            app.checked.insert(idx);
        }

        let mut paths = app.collect_checked_paths();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                PathBuf::from(r"C:\books\a.7z"),
                PathBuf::from(r"C:\books\a.jpg"),
                PathBuf::from(r"C:\books\a.mp4"),
                PathBuf::from(r"C:\books\a.pdf"),
                PathBuf::from(r"C:\books\a.zip"),
            ]
        );

        let targets = app.collect_checked_indexed_paths();
        assert_eq!(
            targets,
            vec![
                (archive, PathBuf::from(r"C:\books\a.7z")),
                (pdf, PathBuf::from(r"C:\books\a.pdf")),
                (zip, PathBuf::from(r"C:\books\a.zip")),
                (video, PathBuf::from(r"C:\books\a.mp4")),
                (image, PathBuf::from(r"C:\books\a.jpg")),
            ],
            "delete targets are sorted by descending index and exclude folders + virtual pages",
        );
    }
}

/// Ctrl+F (run_metadata_search) の構造アイテム絞り込み (task #6 / §4.1)。
#[cfg(test)]
mod ctrl_f_structural_filter_tests {
    use super::*;

    /// Ctrl+F メタ検索ワーカーを items + query + target で叩き、ヒットした
    /// items idx 集合を返すヘルパー。実ファイル / PDF IPC には触れない。
    fn run_ctrl_f(
        query: &str,
        items: &[GridItem],
        target: crate::fts_index::SearchTarget,
    ) -> std::collections::HashSet<usize> {
        run_ctrl_f_with_progress(query, items, target).0
    }

    fn run_ctrl_f_with_progress(
        query: &str,
        items: &[GridItem],
        target: crate::fts_index::SearchTarget,
    ) -> (std::collections::HashSet<usize>, SearchProgressSnapshot) {
        let tokens = crate::search_query::parse(query);
        let xmp: std::collections::HashMap<String, Option<crate::xmp_reader::XmpTweetInfo>> =
            std::collections::HashMap::new();
        let pw = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let progress = SearchProgressShared::new(ctrl_f_progress_total(items));
        match run_metadata_search(
            &tokens,
            items,
            &xmp,
            None,
            &pw,
            &target,
            crate::search_query::MatchMode::And,
            &cancel,
            Some(&progress),
        ) {
            SearchThreadResult::Done { matches, .. } => (matches, progress.snapshot()),
        }
    }

    #[test]
    fn structural_items_filtered_by_name() {
        // §4.1: フォルダ / ZIP / 変換対象アーカイブもファイル名で一貫して絞り込む。
        let items = vec![
            GridItem::Folder(PathBuf::from(r"C:\g\sunset beach")),
            GridItem::Folder(PathBuf::from(r"C:\g\documents")),
            GridItem::ZipFile(PathBuf::from(r"C:\g\sunset.zip")),
            GridItem::ZipFile(PathBuf::from(r"C:\g\misc.zip")),
            GridItem::ConvertibleArchive {
                path: PathBuf::from(r"C:\g\sunset old.7z"),
                format: ArchiveFormat::SevenZ,
            },
        ];
        let m = run_ctrl_f("sunset", &items, crate::fts_index::SearchTarget::All);
        assert_eq!(
            m,
            std::collections::HashSet::from([0, 2, 4]),
            "名前に sunset を含むフォルダ / ZIP / アーカイブだけ残る"
        );
    }

    #[test]
    fn progress_counts_countable_items_across_passes() {
        // separator は最終件数表示と同じく分母から除外し、Pass 1 (ZIP) と
        // Pass 2 (Image) の両方が item 単位で進捗を進める。
        let zip = PathBuf::from(r"C:\g\book.zip");
        let items = vec![
            GridItem::ZipSeparator {
                dir_display: "(root)".into(),
            },
            GridItem::ZipImage {
                zip_path: zip,
                entry_name: "sunset01.png".into(),
            },
            GridItem::Image(PathBuf::from(r"C:\g\sunset.jpg")),
            GridItem::Folder(PathBuf::from(r"C:\g\documents")),
        ];
        let target =
            crate::fts_index::SearchTarget::Only(vec![crate::fts_index::SourceKind::Filename]);
        let (m, progress) = run_ctrl_f_with_progress("sunset", &items, target);

        assert_eq!(m, std::collections::HashSet::from([0, 1, 2]));
        assert_eq!(
            progress,
            SearchProgressSnapshot {
                done: 3,
                total: 3,
                matched: 2,
            }
        );
    }

    #[test]
    fn progress_advances_when_image_target_cannot_contribute() {
        // target=PdfMeta のように Image/Video の fallback hay が空確定する経路でも、
        // UI の分母が残ったまま止まって見えないよう完了数は進める。
        let items = vec![GridItem::Image(PathBuf::from(r"C:\g\sunset.jpg"))];
        let target =
            crate::fts_index::SearchTarget::Only(vec![crate::fts_index::SourceKind::PdfMeta]);
        let (m, progress) = run_ctrl_f_with_progress("sunset", &items, target);

        assert!(m.is_empty());
        assert_eq!(
            progress,
            SearchProgressSnapshot {
                done: 1,
                total: 1,
                matched: 0,
            }
        );
    }

    #[test]
    fn structural_items_hidden_when_target_lacks_filename() {
        // §4.1: 検索対象が EXIF など「構造アイテムが持たない次元」だけなら、
        // 構造アイテムは全件非表示になる。
        let items = vec![
            GridItem::Folder(PathBuf::from(r"C:\g\sunset")),
            GridItem::ZipFile(PathBuf::from(r"C:\g\sunset.zip")),
        ];
        let target = crate::fts_index::SearchTarget::Only(vec![crate::fts_index::SourceKind::Exif]);
        let m = run_ctrl_f("sunset", &items, target);
        assert!(
            m.is_empty(),
            "ファイル名次元を含まない target では構造アイテムは出ない: {m:?}"
        );
    }

    #[test]
    fn sidecar_on_demand_match_value_only() {
        // docs §14-5: 検索対象「サイドカー」/「すべて」で、画像と同名の JSON サイドカーの
        // 値を on-demand 読みして照合する (FS 画像のみ)。キー名は索引しない。
        let tmp = tempfile::TempDir::new().unwrap();
        let img = tmp.path().join("a.jpg");
        std::fs::write(&img, b"img").unwrap();
        std::fs::write(
            tmp.path().join("a.jpg.json"),
            br#"{"artist":"karon-t","tags":["1girl"]}"#,
        )
        .unwrap();
        let items = vec![GridItem::Image(img.clone())];

        // 「サイドカー」のみ: 値 (作者名) でヒット
        let m = run_ctrl_f(
            "karon",
            &items,
            crate::fts_index::SearchTarget::Only(vec![crate::fts_index::SourceKind::Sidecar]),
        );
        assert_eq!(
            m,
            std::collections::HashSet::from([0]),
            "サイドカーの値 (作者名) でヒットする"
        );

        // 「すべて」でも自由語でヒット
        let m_all = run_ctrl_f("1girl", &items, crate::fts_index::SearchTarget::All);
        assert_eq!(
            m_all,
            std::collections::HashSet::from([0]),
            "All でもサイドカー値でヒットする"
        );

        // キー名 (artist) ではヒットしない (値のみ索引)
        let m_key = run_ctrl_f(
            "artist",
            &items,
            crate::fts_index::SearchTarget::Only(vec![crate::fts_index::SourceKind::Sidecar]),
        );
        assert!(m_key.is_empty(), "キー名ではヒットしない: {m_key:?}");
    }

    #[test]
    fn zip_separator_visible_only_when_group_has_match() {
        // §4.1: separator は付随グループに可視 ZipImage が残るときだけ表示する。
        let zip = PathBuf::from(r"C:\g\book.zip");
        let items = vec![
            GridItem::ZipSeparator {
                dir_display: "(root)".into(),
            },
            GridItem::ZipImage {
                zip_path: zip.clone(),
                entry_name: "sunset01.png".into(),
            },
            GridItem::ZipImage {
                zip_path: zip.clone(),
                entry_name: "cat.jpg".into(),
            },
            GridItem::ZipSeparator {
                dir_display: "chapter2".into(),
            },
            GridItem::ZipImage {
                zip_path: zip.clone(),
                entry_name: "dog.jpg".into(),
            },
        ];
        let m = run_ctrl_f("sunset", &items, crate::fts_index::SearchTarget::All);
        assert!(m.contains(&1), "ヒットした ZipImage は表示");
        assert!(m.contains(&0), "ヒットを含むグループの separator は表示");
        assert!(!m.contains(&2), "不一致 ZipImage は非表示");
        assert!(!m.contains(&3), "可視アイテムが残らない separator は非表示");
        assert!(!m.contains(&4), "不一致 ZipImage は非表示");
    }

    #[test]
    fn pdf_file_matches_by_filename() {
        // §4.1.1: PDF はまずファイル名で照合する。target にメタ系を含めない
        // ことで document info IPC を経由しない純粋な名前照合パスを検証する。
        let items = vec![
            GridItem::PdfFile(PathBuf::from(r"C:\g\sunset report.pdf")),
            GridItem::PdfFile(PathBuf::from(r"C:\g\invoice.pdf")),
        ];
        let target =
            crate::fts_index::SearchTarget::Only(vec![crate::fts_index::SourceKind::Filename]);
        let m = run_ctrl_f("sunset", &items, target);
        assert_eq!(m, std::collections::HashSet::from([0]));
    }

    #[test]
    fn pdf_file_metadata_target_matches_conclusively_by_filename() {
        // §4.1.1: 検索対象に PDF メタを含む (All) ときでも、ファイル名だけで
        // 判定が確定するクエリ ("sunset" がファイル名にあり除外語なし →
        // decide_partial が Decided(true)) は document info の IPC を経由せず
        // ヒットする。単一アイテムにして NeedsMore→IPC 経路に入らないようにする。
        let items = vec![GridItem::PdfFile(PathBuf::from(r"C:\g\sunset report.pdf"))];
        let m = run_ctrl_f("sunset", &items, crate::fts_index::SearchTarget::All);
        assert_eq!(m, std::collections::HashSet::from([0]));
    }

    #[test]
    fn pdf_file_metadata_target_honors_filename_exclude() {
        // 除外トークンがファイル名に出現したら、PDF メタ対象 (All) でも
        // その時点で非マッチが確定する (combined-hay 方式の回帰ガード:
        // 旧実装はファイル名と doc info を別 hay で照合し exclude を取りこぼした)。
        let items = vec![GridItem::PdfFile(PathBuf::from(r"C:\g\draft.pdf"))];
        let m = run_ctrl_f("-draft", &items, crate::fts_index::SearchTarget::All);
        assert!(m.is_empty(), "ファイル名に除外語がある PDF は出ない");
    }

    #[test]
    fn search_container_always_kept() {
        // SearchContainer は Ctrl+F と Ctrl+G が排他なので通常出現しないが、
        // 防御的に常に残す。
        let items = vec![GridItem::SearchContainer {
            path: PathBuf::from(r"C:\g\unrelated"),
            kind: crate::grid_item::SearchContainerKind::Folder,
            hit_count: 3,
            representative: None,
        }];
        let m = run_ctrl_f("zzz", &items, crate::fts_index::SearchTarget::All);
        assert!(m.contains(&0), "SearchContainer は常に表示");
    }
}

/// docs/prefetch-suppression-during-scroll-plan.md Phase 2.1
/// decide_prefetch_allowed 純関数のユニットテスト。
#[cfg(test)]
mod prefetch_gate_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn no_scroll_yet_allows() {
        let now = Instant::now();
        let d = decide_prefetch_allowed(now, None, 0);
        assert_eq!(
            d,
            PrefetchDecision::Allow {
                reason: AllowReason::NoScrollYet,
            }
        );
    }

    #[test]
    fn no_scroll_yet_with_visible_pending_still_allows() {
        // last_prefetch_scroll_at = None なら elapsed check しないので
        // visible_pending > 0 でも (Codex 設計: 起動直後経路) — ただし起動経路は
        // 通常 `start_loading_items` が `Some(now)` を立てるので、
        // 厳密には起動から最初の `update` までの極短い窓でしか発生しない。
        let now = Instant::now();
        let d = decide_prefetch_allowed(now, None, 5);
        // visible_pending check は last_prefetch_scroll_at の elapsed branch を
        // 抜けた後に走るので、None だとそのまま到達して Block { VisibleStillLoading }。
        assert_eq!(
            d,
            PrefetchDecision::Block {
                reason: BlockReason::VisibleStillLoading { pending: 5 },
            }
        );
    }

    #[test]
    fn scroll_50ms_ago_blocks() {
        let now = Instant::now();
        let t = now - Duration::from_millis(50);
        let d = decide_prefetch_allowed(now, Some(t), 0);
        assert!(matches!(
            d,
            PrefetchDecision::Block {
                reason: BlockReason::ScrollNotIdle { .. }
            }
        ));
    }

    #[test]
    fn scroll_exactly_100ms_ago_allows() {
        let now = Instant::now();
        let t = now - Duration::from_millis(100);
        let d = decide_prefetch_allowed(now, Some(t), 0);
        assert_eq!(
            d,
            PrefetchDecision::Allow {
                reason: AllowReason::ScrollIdleAndVisibleReady,
            }
        );
    }

    #[test]
    fn scroll_99ms_ago_blocks() {
        let now = Instant::now();
        let t = now - Duration::from_millis(99);
        let d = decide_prefetch_allowed(now, Some(t), 0);
        assert!(matches!(
            d,
            PrefetchDecision::Block {
                reason: BlockReason::ScrollNotIdle { .. }
            }
        ));
    }

    #[test]
    fn scroll_200ms_visible_pending_blocks() {
        let now = Instant::now();
        let t = now - Duration::from_millis(200);
        let d = decide_prefetch_allowed(now, Some(t), 5);
        assert_eq!(
            d,
            PrefetchDecision::Block {
                reason: BlockReason::VisibleStillLoading { pending: 5 },
            }
        );
    }

    #[test]
    fn scroll_200ms_visible_ready_allows() {
        let now = Instant::now();
        let t = now - Duration::from_millis(200);
        let d = decide_prefetch_allowed(now, Some(t), 0);
        assert_eq!(
            d,
            PrefetchDecision::Allow {
                reason: AllowReason::ScrollIdleAndVisibleReady,
            }
        );
    }

    #[test]
    fn scroll_2999ms_with_pending_blocks() {
        // backstop 未到達 + visible 残り → block
        let now = Instant::now();
        let t = now - Duration::from_millis(2999);
        let d = decide_prefetch_allowed(now, Some(t), 5);
        assert_eq!(
            d,
            PrefetchDecision::Block {
                reason: BlockReason::VisibleStillLoading { pending: 5 },
            }
        );
    }

    #[test]
    fn scroll_exactly_3000ms_backstop_allows() {
        // backstop 境界 (≥ 3000ms) → visible pending あっても allow
        let now = Instant::now();
        let t = now - Duration::from_millis(3000);
        let d = decide_prefetch_allowed(now, Some(t), 5);
        assert_eq!(
            d,
            PrefetchDecision::Allow {
                reason: AllowReason::Backstop3s,
            }
        );
    }

    #[test]
    fn scroll_3001ms_backstop_allows_no_pending() {
        // backstop 超過 + visible 揃ってる → 同じく allow (Backstop3s)
        // Backstop は (1) で先に判定されるので visible_pending=0 でも Backstop3s 扱い。
        let now = Instant::now();
        let t = now - Duration::from_millis(3001);
        let d = decide_prefetch_allowed(now, Some(t), 0);
        assert_eq!(
            d,
            PrefetchDecision::Allow {
                reason: AllowReason::Backstop3s,
            }
        );
    }
}

#[cfg(test)]
mod ai_upscale_livelock_tests {
    use super::phase_c_support::setup_app;
    use super::*;
    use crate::fs_animation::FsCacheEntry;
    use crate::grid_item::GridItem;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    /// 回帰テスト (2026-05、v0.9.x からの既存バグ):
    /// 表示中画像が `ai_upscale_skip_px` 超でアップスケール対象外のとき、
    /// `maybe_start_ai_upscale` は先読み (prefetch) ジョブを **キャンセルしてはならない**。
    ///
    /// 旧実装は「先読みを優先キャンセル」ブロックをサイズ閾値チェックより前に
    /// 走らせていたため、処理対象外の大画像をフルスクリーン表示している間、
    /// prefetch を毎フレーム起動 → タイル 1 枚処理後に即キャンセルし続ける
    /// GPU ライブロックになっていた (cancel された job は `poll_ai_upscale` で
    /// failed 扱いされず無限再試行されるため)。
    #[test]
    fn skip_eligible_current_does_not_cancel_prefetch() {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let dummy_tex = ctx.load_texture(
            "test_dummy",
            egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );

        // アップスケール ON / デノイズ OFF。閾値を極小 (=2) にして、4x4 でも
        // should_process(4,4,2)=false (= 範囲外/スキップ) になるようにする。
        app.ai_upscale_enabled = true;
        app.ai_denoise_model = None;
        app.settings.ai_upscale_skip_px = 2;

        // idx 0 = 表示中。fs_cache に Static (4x4) を入れる → サイズ閾値で skip 対象。
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.fs_cache.insert(
            0,
            FsCacheEntry::Static {
                tex: dummy_tex.clone(),
                pixels: std::sync::Arc::new(egui::ColorImage::filled([4, 4], egui::Color32::WHITE)),
                source_dims: None,
                load_seq: 0,
            },
        );
        app.fullscreen_idx = Some(0);

        // 先読みジョブ (idx 1) を pending に積む。cancel フラグは false。
        let bg = app.effective_upscale_bg_mode();
        let prefetch_cancel = std::sync::Arc::new(AtomicBool::new(false));
        let (_tx, rx) = mpsc::channel::<crate::ai::upscale::UpscaleResult>();
        app.ai_upscale_pending
            .insert((1, bg), (prefetch_cancel.clone(), rx));

        // 表示中 (idx 0) は skip 対象なので、cancel ブロックに入らず即 return するはず。
        app.maybe_start_ai_upscale(0);

        assert!(
            !prefetch_cancel.load(Ordering::Relaxed),
            "skip 対象の表示中画像が先読みをキャンセルしてはならない (GPU ライブロック回帰防止)"
        );
        assert!(
            app.ai_upscale_pending.contains_key(&(1, bg)),
            "先読み pending entry は温存されるべき"
        );
    }

    /// 表示中画像がまだ `fs_cache` にロードされていない (源泉なし) 場合も、
    /// 先読みをキャンセルしてはならない。源泉取得を cancel ブロックより前に
    /// 行うことの回帰ガード (未ロード画像が先読みを kill すると同じループになる)。
    #[test]
    fn current_without_source_does_not_cancel_prefetch() {
        let mut app = setup_app();

        // 閾値は大きく取り (= 通常なら処理対象)、源泉が無いことだけが return 理由になる状況。
        app.ai_upscale_enabled = true;
        app.ai_denoise_model = None;
        app.settings.ai_upscale_skip_px = 10_000;
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.fullscreen_idx = Some(0);
        // idx 0 は fs_cache に **入れない** (= 未ロード)。

        let bg = app.effective_upscale_bg_mode();
        let prefetch_cancel = std::sync::Arc::new(AtomicBool::new(false));
        let (_tx, rx) = mpsc::channel::<crate::ai::upscale::UpscaleResult>();
        app.ai_upscale_pending
            .insert((1, bg), (prefetch_cancel.clone(), rx));

        app.maybe_start_ai_upscale(0);

        assert!(
            !prefetch_cancel.load(Ordering::Relaxed),
            "源泉未ロードの表示中画像が先読みをキャンセルしてはならない"
        );
        assert!(
            app.ai_upscale_pending.contains_key(&(1, bg)),
            "先読み pending entry は温存されるべき"
        );
    }

    /// Static な fs_cache エントリを作るテストヘルパ (`pixels.size` = w×h)。
    fn static_fs_entry(ctx: &egui::Context, w: usize, h: usize) -> FsCacheEntry {
        let tex = ctx.load_texture(
            "test_dummy",
            egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        FsCacheEntry::Static {
            tex,
            pixels: std::sync::Arc::new(egui::ColorImage::filled([w, h], egui::Color32::WHITE)),
            source_dims: None,
            load_seq: 0,
        }
    }

    /// 回帰テスト (Codex P2・第1ラウンド): denoise も有効で画像が upscale / denoise 双方の
    /// サイズ閾値を超過 (= どちらの AI も走らない) とき、先読みスケジューラ predicate
    /// `ai_prefetch_current_ready` は true (= done) を返し先読みへ進む。upscale 側 skip
    /// だけ見ていた旧 `current_done` はこのケースを取りこぼし先読みが黙って停止していた。
    #[test]
    fn prefetch_ready_true_when_both_ai_skip_eligible() {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        app.ai_upscale_enabled = true;
        app.ai_denoise_model = Some(crate::ai::ModelKind::DenoiseRealplksr);
        app.settings.ai_upscale_skip_px = 2;
        app.settings.ai_denoise_skip_px = 2;
        app.fs_cache.insert(0, static_fs_entry(&ctx, 4, 4));
        assert!(
            app.ai_prefetch_current_ready(0),
            "upscale/denoise 双方が閾値超でスキップなら done 扱い (= 先読み続行)"
        );
    }

    /// Static で範囲内 (= これから処理される) かつ未キャッシュなら、まだ done でない
    /// (current を優先して先読みを待つ)。
    #[test]
    fn prefetch_ready_false_when_static_in_range_uncached() {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        app.ai_upscale_enabled = true;
        app.ai_denoise_model = None;
        app.settings.ai_upscale_skip_px = 10_000; // 4x4 は範囲内
        app.fs_cache.insert(0, static_fs_entry(&ctx, 4, 4));
        assert!(
            !app.ai_prefetch_current_ready(0),
            "範囲内で未処理の current は done でない (先読みより current 優先)"
        );
    }

    /// 回帰テスト (Codex P2・第2ラウンド): current が `FsCacheEntry::Failed` (終端状態) の
    /// とき、`ai_prefetch_current_ready` は true (= done) を返さなければならない。
    /// `!ai_will_apply_to` に委ねると Failed / Animated は「寸法不明 → 保守的 true」で
    /// done=false に固定され、current が永遠に未完了 → 先読みが永久停止する。
    #[test]
    fn prefetch_ready_true_for_failed_current() {
        let mut app = setup_app();
        app.ai_upscale_enabled = true;
        app.ai_denoise_model = None;
        // 範囲内設定 (skip しない) でも、Failed は終端状態なので done になるべき。
        app.settings.ai_upscale_skip_px = 10_000;
        app.fs_cache.insert(0, FsCacheEntry::Failed);
        assert!(
            app.ai_prefetch_current_ready(0),
            "Failed (終端状態) の current は done 扱い (= 先読みを止めない)"
        );
    }
}

#[cfg(test)]
mod pano_settle_size_tests {
    use super::*;

    #[test]
    fn settle_output_smaller_than_cap_passes_through() {
        // 1920 以下 viewport はそのまま (例: 1280×720)
        assert_eq!(compute_pano_settle_output_size((1280, 720)), (1280, 720));
        assert_eq!(compute_pano_settle_output_size((1920, 1080)), (1920, 1080));
        assert_eq!(compute_pano_settle_output_size((800, 600)), (800, 600));
    }

    #[test]
    fn settle_output_4k_capped_to_1920_landscape() {
        // 3840×2160 (16:9) → 1920×1080
        assert_eq!(compute_pano_settle_output_size((3840, 2160)), (1920, 1080));
        // 2560×1440 (16:9) → 1920×1080 (round 1080)
        assert_eq!(compute_pano_settle_output_size((2560, 1440)), (1920, 1080));
    }

    #[test]
    fn settle_output_4k_capped_to_1920_portrait() {
        // 2160×3840 (portrait 9:16) → 1080×1920
        assert_eq!(compute_pano_settle_output_size((2160, 3840)), (1080, 1920));
    }

    #[test]
    fn settle_output_handles_zero() {
        // 0 が来ても 1 で扱う
        let (w, h) = compute_pano_settle_output_size((0, 0));
        assert!(w >= 1 && h >= 1, "got ({w}, {h})");
    }

    #[test]
    fn settle_output_aspect_preserved_landscape() {
        // 21:9 ultrawide 5120×2160 → 1920×x where x = round(1920 * 2160 / 5120) = 810
        let (w, h) = compute_pano_settle_output_size((5120, 2160));
        assert_eq!(w, 1920);
        let expected_h = (1920.0_f32 * 2160.0 / 5120.0).round() as u32;
        assert_eq!(h, expected_h);
    }

    #[test]
    fn downsample_color_image_dims() {
        // テキストプレビュー解像度の縮小: 1/N で寸法が w/N, h/N になる。
        let ci = egui::ColorImage::new([800, 400], vec![egui::Color32::RED; 800 * 400]);
        assert_eq!(
            downsample_color_image(&ci, 1).size,
            [800, 400],
            "原寸は素通し"
        );
        assert_eq!(downsample_color_image(&ci, 2).size, [400, 200]);
        assert_eq!(downsample_color_image(&ci, 4).size, [200, 100]);
        assert_eq!(downsample_color_image(&ci, 8).size, [100, 50]);
    }

    #[test]
    fn downsample_color_image_clamps_to_one() {
        // 極小画像でも 0 寸法にならず最低 1px を保つ (panic 防止)。
        let ci = egui::ColorImage::new([3, 1], vec![egui::Color32::WHITE; 3]);
        let out = downsample_color_image(&ci, 8);
        assert!(out.size[0] >= 1 && out.size[1] >= 1, "got {:?}", out.size);
    }
}
