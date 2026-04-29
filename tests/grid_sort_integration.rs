//! グリッド構築側のソート挙動の回帰防止テスト。
//!
//! `App::load_folder_inner` のフォルダ系ブロック (Folder / ZipFile / PdfFile /
//! ConvertibleArchive) ソートは [`mimageviewer::grid_item::sort_folder_block`] に
//! 切り出されており、本番経路もテストもこの公開関数を直接呼ぶ。
//!
//! ヘルパー経由で `SortOrder::compare` だけ叩く形だと「`load_folder_inner` が
//! 関数呼び出しを外した」「name() 固定に戻った」等の回帰を検出できないため、
//! ここでは本物の `GridItem` を渡して end-to-end の並びを assert する。

use std::path::PathBuf;

use mimageviewer::grid_item::{sort_folder_block, GridItem};
use mimageviewer::settings::SortOrder;

fn run(items: Vec<(GridItem, i64)>, order: SortOrder) -> Vec<String> {
    let mut folders: Vec<GridItem> = items.iter().map(|(g, _)| g.clone()).collect();
    let mut metas: Vec<Option<(i64, i64)>> =
        items.iter().map(|(_, mt)| Some((*mt, 100))).collect();
    sort_folder_block(&mut folders, &mut metas, order);
    folders.iter().map(|f| f.name().to_string()).collect()
}

fn folder(name: &str) -> GridItem {
    GridItem::Folder(PathBuf::from(name))
}
fn zipf(name: &str) -> GridItem {
    GridItem::ZipFile(PathBuf::from(name))
}
fn pdff(name: &str) -> GridItem {
    GridItem::PdfFile(PathBuf::from(name))
}

#[test]
fn folder_block_follows_date_desc() {
    let names = run(
        vec![
            (folder("alpha-folder"), 1000),
            (folder("zeta-folder"), 3000),
            (zipf("middle-zip.zip"), 2000),
            (pdff("kappa.pdf"), 4000),
        ],
        SortOrder::DateDesc,
    );
    assert_eq!(
        names,
        vec!["kappa.pdf", "zeta-folder", "middle-zip.zip", "alpha-folder"]
    );
}

#[test]
fn folder_block_follows_date_asc() {
    let names = run(
        vec![
            (folder("zeta-folder"), 3000),
            (folder("alpha-folder"), 1000),
            (pdff("kappa.pdf"), 4000),
            (zipf("middle-zip.zip"), 2000),
        ],
        SortOrder::DateAsc,
    );
    assert_eq!(
        names,
        vec!["alpha-folder", "middle-zip.zip", "zeta-folder", "kappa.pdf"]
    );
}

#[test]
fn date_desc_tiebreak_by_name_ascending() {
    // mtime_secs は秒精度。同一秒のファイル群は read_dir 順 (FS 依存) に
    // 流れないよう name 昇順で安定化する不変条件を回帰防止する。
    let names = run(
        vec![
            (zipf("ZZZ.zip"), 1000),
            (folder("aaa-folder"), 1000),
            (pdff("MMM.pdf"), 1000),
            (folder("bbb-folder"), 1000),
        ],
        SortOrder::DateDesc,
    );
    assert_eq!(names, vec!["aaa-folder", "bbb-folder", "MMM.pdf", "ZZZ.zip"]);
}

#[test]
fn date_asc_tiebreak_by_name_ascending() {
    let names = run(
        vec![
            (zipf("ZZZ.zip"), 1000),
            (folder("aaa-folder"), 1000),
            (pdff("MMM.pdf"), 1000),
            (folder("bbb-folder"), 1000),
        ],
        SortOrder::DateAsc,
    );
    assert_eq!(names, vec!["aaa-folder", "bbb-folder", "MMM.pdf", "ZZZ.zip"]);
}

#[test]
fn folder_block_follows_filename_case_insensitive() {
    let names = run(
        vec![
            (pdff("Zeta.pdf"), 1000),
            (folder("alpha-folder"), 2000),
            (zipf("BETA.zip"), 3000),
        ],
        SortOrder::FileName,
    );
    assert_eq!(names, vec!["alpha-folder", "BETA.zip", "Zeta.pdf"]);
}

#[test]
fn folder_block_numeric_natural_order() {
    let names = run(
        vec![
            (folder("vol10"), 1000),
            (folder("vol2"), 1000),
            (folder("vol1"), 1000),
            (folder("vol11"), 1000),
            (folder("vol9"), 1000),
        ],
        SortOrder::Numeric,
    );
    assert_eq!(names, vec!["vol1", "vol2", "vol9", "vol10", "vol11"]);
}

#[test]
fn meta_none_is_treated_as_mtime_zero() {
    // `App::load_folder_inner` で metadata 取得失敗時は `None` が入り、
    // sort_folder_block 内で 0 として扱われる。DateDesc では最も古いとして末尾に並ぶ。
    let mut folders = vec![
        folder("missing-meta"),
        folder("recent"),
        folder("old"),
    ];
    let mut metas: Vec<Option<(i64, i64)>> = vec![None, Some((3000, 100)), Some((1000, 100))];
    sort_folder_block(&mut folders, &mut metas, SortOrder::DateDesc);
    let names: Vec<_> = folders.iter().map(|f| f.name().to_string()).collect();
    assert_eq!(names, vec!["recent", "old", "missing-meta"]);
}
