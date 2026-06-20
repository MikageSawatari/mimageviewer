//! ファイル名 prefix スタックの純ロジック (`docs/filename-stack-plan.md`)。
//!
//! フォルダ内のメディア (画像 + 動画) を「投稿 = スタック」に畳むためのグループ化を行う。
//! pixiv / danbooru 等のダウンローダは 1 投稿に複数画像があるとファイル名の前半 (prefix)
//! が共通で後半 (suffix) だけ変わる (例: `12345678_p0.jpg` / `12345678_p1.jpg`)。これを
//! prefix でまとめて一覧の見通しを上げる。
//!
//! # グループ化キー
//! ファイル名 (拡張子を除いた basename) の「**末尾の区切り文字の前**」。既定の区切り文字は
//! `_` (設定 `stack_separator`)。例: `12345678_p0` → 末尾 `_` の前 = `12345678`。区切り文字が
//! 無いファイルは basename 全体 (= 単独スタック)。区切り文字の手前が空になるケース
//! (例 `_p0`) は意味のある prefix が無いので basename 全体にフォールバックする。
//!
//! # 動画
//! 動画は **決してスタックに混ぜない** (plan §3.2/§4)。各動画を必ず単独グループにする。
//!
//! # 純データ
//! I/O を一切行わない純ロジックなので UI スレッドで呼んでもブロックしない。表示用の
//! `GridItem` 生成や `self.items` への適用は呼び出し側 (App) が行う。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::grid_item::GridItem;
use crate::settings::SortOrder;

/// グループ化対象の 1 メディア。
#[derive(Clone, Debug, PartialEq)]
pub struct StackMember {
    pub path: PathBuf,
    /// 更新日時 (秒)。日付ソート用。
    pub mtime: i64,
    /// ファイルサイズ (バイト)。
    pub size: i64,
    /// 動画か (動画は単独グループに固定する)。
    pub is_video: bool,
}

/// 1 スタック (= フォルダ内の一時的な仮想本)。`members` は表示 sort 済みで常に非空。
#[derive(Clone, Debug, PartialEq)]
pub struct StackGroup {
    /// グループ化キー (prefix)。単独グループや動画は内部用の一意キーになることもある
    /// (表示には使わない)。
    pub key: String,
    /// メンバー。表示 sort 順。`len() >= 1`。
    pub members: Vec<StackMember>,
}

impl StackGroup {
    /// 代表メンバー (sort 先頭)。サムネ・グループソートのキーに使う。
    pub fn representative(&self) -> &StackMember {
        &self.members[0]
    }

    /// メンバー数 (バッジ表示用)。
    pub fn count(&self) -> usize {
        self.members.len()
    }

    /// バッジ付きで「畳んだスタック」として描くグループか。
    /// = 画像が 2 枚以上。単独 (1 枚) や動画グループは通常サムネイルとして描く (バッジなし)。
    pub fn is_stack(&self) -> bool {
        self.members.len() >= 2 && !self.members[0].is_video
    }
}

/// 拡張子を除いた basename (`stem`) から prefix (末尾区切り文字の前) を取り出す。
///
/// - 区切り文字が無ければ `stem` 全体。
/// - 末尾が区切り文字 (例 `foo_`) のときはその手前 (`foo`)。
/// - 区切り文字の手前が空になる (例 `_p0`、先頭が区切り文字で末尾でもない場合は
///   rfind が 0 を返す) ときは意味のある prefix が無いので `stem` 全体を返す。
pub fn prefix_of(stem: &str, separator: char) -> &str {
    match stem.rfind(separator) {
        Some(0) | None => stem,
        Some(pos) => &stem[..pos],
    }
}

/// `path` の拡張子を除いた basename を返す (無ければ空文字列)。
fn stem_of(path: &Path) -> &str {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("")
}

/// `path` のファイル名 (拡張子付き) を返す (sort 比較用、無ければ空文字列)。
fn name_of(path: &Path) -> &str {
    path.file_name().and_then(|s| s.to_str()).unwrap_or("")
}

/// メディア列を prefix でグループ化する。
///
/// - 画像: stem の末尾区切り文字の前でグループ化。
/// - 動画: 各々を単独グループに固定 (path を一意キーにする)。
/// - 各グループ内のメンバーを `sort` で並べる。
/// - グループ自体を代表メンバー (`members[0]`) で `sort` で並べる。
///
/// 戻り値のグループ順 = 表示順。`materialize` 系の呼び出し側はこの順でセルを並べる。
pub fn group_media(media: Vec<StackMember>, separator: char, sort: SortOrder) -> Vec<StackGroup> {
    let mut map: HashMap<String, Vec<StackMember>> = HashMap::new();
    // 挿入順を保持して、後段のグループ sort が安定するようにする (HashMap 反復順は非決定)。
    let mut order: Vec<String> = Vec::new();
    for m in media {
        let key = if m.is_video {
            // 動画はスタックに混ぜない。path を一意キーにして必ず単独グループにする。
            // (basename だと別ディレクトリ同名で衝突しうるが、ここは単一フォルダなので
            //  path 全体で十分に一意。)
            format!("\u{0}video\u{0}{}", m.path.display())
        } else {
            let stem = stem_of(&m.path);
            let p = prefix_of(stem, separator);
            if p.is_empty() {
                stem.to_string()
            } else {
                p.to_string()
            }
        };
        if !map.contains_key(&key) {
            order.push(key.clone());
        }
        map.entry(key).or_default().push(m);
    }

    let mut groups: Vec<StackGroup> = Vec::with_capacity(order.len());
    for key in order {
        let mut members = map.remove(&key).expect("key inserted into map");
        sort_members(&mut members, sort);
        groups.push(StackGroup { key, members });
    }

    // グループを代表メンバーで sort (= 表示順)。
    groups.sort_by(|a, b| {
        let ra = a.representative();
        let rb = b.representative();
        sort.compare(
            name_of(&ra.path),
            ra.mtime,
            name_of(&rb.path),
            rb.mtime,
            crate::ui_helpers::natural_sort_key,
        )
    });
    groups
}

/// スタックモードのビュー状態 (App が `Option<StackView>` で保持)。
///
/// `drilled` で 2 状態を表す:
/// - `None` = 集約ビュー (1 グループ = 1 セル。複数枚はスタックセル + バッジ、単独は通常セル)。
/// - `Some(g)` = メンバーグリッド (groups[g] のメンバーを実 `Image`/`Video` セルで展開)。
///
/// `passthrough` は画像以外 (フォルダ / ZIP / PDF / 変換アーカイブ) のセル列で、集約ビューの
/// 先頭に置く (= 通常レイアウトのコンテナ先頭慣習を踏襲、plan §4 「素通し表示」)。`groups` は
/// メディア (画像 + 動画) を prefix でまとめたもの (動画は単独固定)。
/// (`GridItem` が `Debug` 非対応のため `Debug` は derive しない。)
#[derive(Clone)]
pub struct StackView {
    /// このビューが束縛されている実フォルダ。別フォルダへ移動したら破棄する。
    pub folder: PathBuf,
    /// グループ化区切り文字 (構築時の値)。
    pub separator: char,
    /// 構築時のソート順 (グループ/メンバーの並びはこれに従う)。
    pub sort: SortOrder,
    /// 画像以外のコンテナセル (集約ビュー先頭の passthrough)。
    pub passthrough: Vec<GridItem>,
    /// `passthrough` と同インデックスの `(mtime, size)`。
    pub passthrough_metas: Vec<Option<(i64, i64)>>,
    /// メディアグループ (表示順)。
    pub groups: Vec<StackGroup>,
    /// ドリル状態。`None` = 集約、`Some(g)` = groups[g] のメンバーグリッド。
    pub drilled: Option<usize>,
}

impl StackView {
    /// メディア (画像 + 動画) を prefix でグループ化して `StackView` を作る (集約状態)。
    pub fn build(
        folder: PathBuf,
        passthrough: Vec<GridItem>,
        passthrough_metas: Vec<Option<(i64, i64)>>,
        media: Vec<StackMember>,
        separator: char,
        sort: SortOrder,
    ) -> Self {
        let groups = group_media(media, separator, sort);
        Self {
            folder,
            separator,
            sort,
            passthrough,
            passthrough_metas,
            groups,
            drilled: None,
        }
    }

    /// 畳めるスタック (画像 2 枚以上のグループ) が 1 つでもあるか。
    /// 集約しても全部単独セル = 通常一覧と同じ見た目なら、トグルしても無意味なので
    /// 呼び出し側がトーストで知らせる等に使える。
    pub fn has_collapsible_stack(&self) -> bool {
        self.groups.iter().any(|g| g.is_stack())
    }

    /// 集約ビューの `(items, image_metas)` を作る。passthrough (コンテナ) → グループセルの順。
    pub fn materialize_aggregated(&self) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
        let mut items = self.passthrough.clone();
        let mut metas = self.passthrough_metas.clone();
        for g in &self.groups {
            let rep = g.representative();
            if g.is_stack() {
                items.push(GridItem::Stack {
                    key: g.key.clone(),
                    representative: rep.path.clone(),
                    count: g.count(),
                });
            } else if rep.is_video {
                items.push(GridItem::Video(rep.path.clone()));
            } else {
                items.push(GridItem::Image(rep.path.clone()));
            }
            metas.push(Some((rep.mtime, rep.size)));
        }
        (items, metas)
    }

    /// groups[g] のメンバーグリッド `(items, image_metas)` を作る。
    /// 範囲外の g は空を返す。メンバーは実 `Image`/`Video` セル (展開後は通常操作可能)。
    pub fn materialize_member(&self, g: usize) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
        let mut items = Vec::new();
        let mut metas = Vec::new();
        if let Some(group) = self.groups.get(g) {
            for m in &group.members {
                items.push(if m.is_video {
                    GridItem::Video(m.path.clone())
                } else {
                    GridItem::Image(m.path.clone())
                });
                metas.push(Some((m.mtime, m.size)));
            }
        }
        (items, metas)
    }

    /// `key` を持つグループの index を返す (集約セルのクリック → ドリル先解決)。
    pub fn group_index_by_key(&self, key: &str) -> Option<usize> {
        self.groups.iter().position(|g| g.key == key)
    }
}

/// グループ内メンバーを表示 sort 順に並べる。
fn sort_members(members: &mut [StackMember], sort: SortOrder) {
    members.sort_by(|a, b| {
        sort.compare(
            name_of(&a.path),
            a.mtime,
            name_of(&b.path),
            b.mtime,
            crate::ui_helpers::natural_sort_key,
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(name: &str) -> StackMember {
        StackMember {
            path: PathBuf::from(format!(r"C:\dl\{name}")),
            mtime: 0,
            size: 0,
            is_video: false,
        }
    }

    fn img_dated(name: &str, mtime: i64) -> StackMember {
        StackMember {
            path: PathBuf::from(format!(r"C:\dl\{name}")),
            mtime,
            size: 0,
            is_video: false,
        }
    }

    fn vid(name: &str) -> StackMember {
        StackMember {
            path: PathBuf::from(format!(r"C:\dl\{name}")),
            mtime: 0,
            size: 0,
            is_video: true,
        }
    }

    fn keys(groups: &[StackGroup]) -> Vec<&str> {
        groups.iter().map(|g| g.key.as_str()).collect()
    }

    fn member_names(g: &StackGroup) -> Vec<&str> {
        g.members.iter().map(|m| name_of(&m.path)).collect()
    }

    // ── prefix_of ────────────────────────────────────────────────────

    #[test]
    fn prefix_basic() {
        assert_eq!(prefix_of("12345678_p0", '_'), "12345678");
        assert_eq!(prefix_of("12345678_p12", '_'), "12345678");
    }

    #[test]
    fn prefix_no_separator_is_full_stem() {
        assert_eq!(prefix_of("12345678", '_'), "12345678");
        assert_eq!(prefix_of("cover", '_'), "cover");
    }

    #[test]
    fn prefix_uses_last_separator() {
        assert_eq!(prefix_of("a_b_c", '_'), "a_b");
        assert_eq!(prefix_of("post_123_p4", '_'), "post_123");
    }

    #[test]
    fn prefix_trailing_separator() {
        assert_eq!(prefix_of("foo_", '_'), "foo");
    }

    #[test]
    fn prefix_leading_separator_falls_back_to_full() {
        // 先頭が区切りで手前が空 → 意味のある prefix が無いので全体。
        assert_eq!(prefix_of("_p0", '_'), "_p0");
    }

    #[test]
    fn prefix_custom_separator() {
        assert_eq!(prefix_of("12345678-1", '-'), "12345678");
        assert_eq!(prefix_of("a.b.c", '.'), "a.b");
    }

    // ── group_media ──────────────────────────────────────────────────

    #[test]
    fn groups_multi_page_post() {
        let media = vec![img("12345678_p1.jpg"), img("12345678_p0.jpg")];
        let groups = group_media(media, '_', SortOrder::FileName);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].key, "12345678");
        // メンバーは FileName 昇順。
        assert_eq!(
            member_names(&groups[0]),
            vec!["12345678_p0.jpg", "12345678_p1.jpg"]
        );
        assert_eq!(groups[0].count(), 2);
        assert!(groups[0].is_stack());
    }

    #[test]
    fn singleton_is_not_a_stack() {
        let media = vec![img("alone.jpg")];
        let groups = group_media(media, '_', SortOrder::FileName);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].count(), 1);
        assert!(!groups[0].is_stack());
    }

    #[test]
    fn cover_without_suffix_joins_its_post() {
        // "12345678.jpg" (区切りなし → key 12345678) は同 prefix のページと同じスタックに入る。
        let media = vec![
            img("12345678.jpg"),
            img("12345678_p1.jpg"),
            img("12345678_p2.jpg"),
        ];
        let groups = group_media(media, '_', SortOrder::FileName);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].key, "12345678");
        assert_eq!(groups[0].count(), 3);
    }

    #[test]
    fn videos_never_grouped_even_same_prefix() {
        // 同 prefix の動画でも別グループ。is_stack も false。
        let media = vec![vid("clip_1.mp4"), vid("clip_2.mp4")];
        let groups = group_media(media, '_', SortOrder::FileName);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.count() == 1));
        assert!(groups.iter().all(|g| !g.is_stack()));
    }

    #[test]
    fn video_not_merged_with_same_prefix_image() {
        // 画像 "clip_p0/p1.jpg" は prefix "clip" のスタック、動画 "clip.mp4" は
        // 同名でも別の単独グループ (動画は混ぜない)。
        let media = vec![img("clip_p0.jpg"), img("clip_p1.jpg"), vid("clip.mp4")];
        let groups = group_media(media, '_', SortOrder::FileName);
        // 画像スタック "clip" + 動画単独 = 2 グループ。
        assert_eq!(groups.len(), 2);
        let stack = groups.iter().find(|g| g.is_stack()).expect("image stack");
        assert_eq!(stack.key, "clip");
        assert_eq!(stack.count(), 2);
        // 動画グループはスタックでない。
        assert!(groups.iter().any(|g| !g.is_stack() && g.count() == 1));
    }

    #[test]
    fn groups_ordered_by_representative_filename() {
        let media = vec![
            img("bbb_p0.jpg"),
            img("aaa_p1.jpg"),
            img("aaa_p0.jpg"),
            img("ccc.jpg"),
        ];
        let groups = group_media(media, '_', SortOrder::FileName);
        assert_eq!(keys(&groups), vec!["aaa", "bbb", "ccc"]);
    }

    #[test]
    fn numeric_sort_natural_order_within_and_across() {
        let media = vec![img("post_p10.jpg"), img("post_p2.jpg"), img("post_p1.jpg")];
        let groups = group_media(media, '_', SortOrder::Numeric);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            member_names(&groups[0]),
            vec!["post_p1.jpg", "post_p2.jpg", "post_p10.jpg"]
        );
    }

    #[test]
    fn date_sort_orders_groups_by_representative_mtime() {
        // DateDesc: 代表 (グループ内 sort 先頭 = 最新) が新しいグループほど前。
        let media = vec![
            img_dated("old_p0.jpg", 100),
            img_dated("old_p1.jpg", 101),
            img_dated("new_p0.jpg", 200),
            img_dated("new_p1.jpg", 201),
        ];
        let groups = group_media(media, '_', SortOrder::DateDesc);
        assert_eq!(groups.len(), 2);
        // 各グループ内も DateDesc (新しい順)。
        assert_eq!(groups[0].representative().mtime, 201);
        assert_eq!(keys(&groups), vec!["new", "old"]);
    }

    #[test]
    fn empty_input_yields_no_groups() {
        let groups = group_media(Vec::new(), '_', SortOrder::FileName);
        assert!(groups.is_empty());
    }

    #[test]
    fn custom_separator_groups_dash() {
        let media = vec![img("777-a.png"), img("777-b.png"), img("888-a.png")];
        let groups = group_media(media, '-', SortOrder::FileName);
        assert_eq!(keys(&groups), vec!["777", "888"]);
        assert_eq!(groups[0].count(), 2);
    }

    // ── StackView ────────────────────────────────────────────────────

    fn folder(name: &str) -> GridItem {
        GridItem::Folder(PathBuf::from(format!(r"C:\dl\{name}")))
    }

    fn item_name(item: &GridItem) -> String {
        item.name().to_string()
    }

    #[test]
    fn aggregated_puts_passthrough_first_then_group_cells() {
        // フォルダ 1 + (画像スタック "post" 2枚) + (単独画像 "solo")。
        let media = vec![img("post_p0.jpg"), img("post_p1.jpg"), img("solo.jpg")];
        let sv = StackView::build(
            PathBuf::from(r"C:\dl"),
            vec![folder("sub")],
            vec![None],
            media,
            '_',
            SortOrder::FileName,
        );
        let (items, metas) = sv.materialize_aggregated();
        assert_eq!(items.len(), metas.len());
        // [0] = passthrough フォルダ。
        assert!(matches!(items[0], GridItem::Folder(_)));
        // [1] = スタックセル (post, count 2)。
        match &items[1] {
            GridItem::Stack { key, count, .. } => {
                assert_eq!(key, "post");
                assert_eq!(*count, 2);
            }
            _ => panic!("expected Stack cell at index 1"),
        }
        // [2] = 単独画像 (通常 Image セル、バッジなし)。
        assert!(matches!(items[2], GridItem::Image(_)));
        assert_eq!(item_name(&items[2]), "solo.jpg");
    }

    #[test]
    fn aggregated_singleton_video_is_video_cell() {
        let media = vec![vid("movie.mp4")];
        let sv = StackView::build(
            PathBuf::from(r"C:\dl"),
            Vec::new(),
            Vec::new(),
            media,
            '_',
            SortOrder::FileName,
        );
        let (items, _) = sv.materialize_aggregated();
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], GridItem::Video(_)));
    }

    #[test]
    fn materialize_member_expands_group_into_real_images() {
        let media = vec![img("post_p1.jpg"), img("post_p0.jpg")];
        let sv = StackView::build(
            PathBuf::from(r"C:\dl"),
            Vec::new(),
            Vec::new(),
            media,
            '_',
            SortOrder::FileName,
        );
        let g = sv.group_index_by_key("post").expect("group exists");
        let (items, metas) = sv.materialize_member(g);
        assert_eq!(items.len(), 2);
        assert_eq!(metas.len(), 2);
        // メンバーは実 Image セル、FileName 昇順。
        assert_eq!(item_name(&items[0]), "post_p0.jpg");
        assert_eq!(item_name(&items[1]), "post_p1.jpg");
        assert!(items.iter().all(|i| matches!(i, GridItem::Image(_))));
    }

    #[test]
    fn materialize_member_out_of_range_is_empty() {
        let sv = StackView::build(
            PathBuf::from(r"C:\dl"),
            Vec::new(),
            Vec::new(),
            vec![img("a.jpg")],
            '_',
            SortOrder::FileName,
        );
        let (items, metas) = sv.materialize_member(99);
        assert!(items.is_empty());
        assert!(metas.is_empty());
    }

    #[test]
    fn has_collapsible_stack_detects_multi_image_group() {
        let with_stack = StackView::build(
            PathBuf::from(r"C:\dl"),
            Vec::new(),
            Vec::new(),
            vec![img("p_0.jpg"), img("p_1.jpg")],
            '_',
            SortOrder::FileName,
        );
        assert!(with_stack.has_collapsible_stack());

        let only_singletons = StackView::build(
            PathBuf::from(r"C:\dl"),
            Vec::new(),
            Vec::new(),
            vec![img("a.jpg"), img("b.jpg")],
            '_',
            SortOrder::FileName,
        );
        assert!(!only_singletons.has_collapsible_stack());
    }
}
