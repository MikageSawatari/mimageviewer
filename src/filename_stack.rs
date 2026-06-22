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
    // 組み込み既定ルール = 各メディアの prefix (末尾区切りの前) をグループキーにする。
    // 動画は group_by_keys が一意キーへ上書きするので、ここでの値は使われない。
    let keys: Vec<String> = media
        .iter()
        .map(|m| {
            let stem = stem_of(&m.path);
            let p = prefix_of(stem, separator);
            if p.is_empty() {
                stem.to_string()
            } else {
                p.to_string()
            }
        })
        .collect();
    group_by_keys(media, &keys, sort)
}

/// 既に算出済みのグループキー (`media` と同じ長さ) からグループを組み立てる。
///
/// - 同じキー = 同じグループ。グループ順はキーの **初出順** (HashMap 反復順は非決定
///   なので `order` で安定化)。
/// - **動画は keys に関わらず必ず単独グループ** (path 由来の一意キーへ上書き。plan §3.2/§4)。
/// - グループ内メンバー / グループ自体の並びは `sort` に従う。
///
/// ユーザー定義スクリプト ([`crate::filename_stack_script`]) も組み込み既定
/// ([`group_media`]) も、このキー → グループ変換を共有する。
pub fn group_by_keys(media: Vec<StackMember>, keys: &[String], sort: SortOrder) -> Vec<StackGroup> {
    debug_assert_eq!(media.len(), keys.len());
    let mut map: HashMap<String, Vec<StackMember>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (i, m) in media.into_iter().enumerate() {
        let key = if m.is_video {
            // 動画はスタックに混ぜない。path を一意キーにして必ず単独グループにする。
            format!("\u{0}video\u{0}{}", m.path.display())
        } else {
            keys.get(i).cloned().unwrap_or_default()
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
    let mut keyed_groups: Vec<_> = groups
        .into_iter()
        .map(|group| {
            let rep = group.representative();
            let key = sort.name_key(name_of(&rep.path));
            let mtime = rep.mtime;
            (group, key, mtime)
        })
        .collect();
    keyed_groups
        .sort_by(|(_, ak, a_mt), (_, bk, b_mt)| sort.compare_name_keys(ak, *a_mt, bk, *b_mt));
    keyed_groups
        .into_iter()
        .map(|(group, _, _)| group)
        .collect()
}

/// スタックモードのビュー状態 (App が `Option<StackView>` で保持)。
///
/// グリッドは常に**集約ビュー** (1 グループ = 1 セル。複数枚はスタックセル + バッジ、単独は
/// 通常セル) を表示する。スタック/画像セルを開くと、フルスクリーンは **フラット読書ビュー**
/// (= 全画像を 1 本の並びに展開) を読む: `↓↑` は境界を越えて順送り、`Shift+↓↑` で次/前のスタック
/// の先頭へジャンプ、`Ctrl+↓↑` はフォルダ移動 (据え置き)。メンバーグリッドは設けない
/// (1 枚スタックの割合が高く、毎回中間グリッドを挟むと煩雑なため。実機フィードバック 2026-06-20)。
///
/// `passthrough` は画像以外 (フォルダ / ZIP / PDF / 変換アーカイブ) のセル列で、両ビューの先頭に
/// 置く (= 通常レイアウトのコンテナ先頭慣習を踏襲、plan §4 「素通し表示」)。`groups` はメディア
/// (画像 + 動画) を prefix でまとめたもの (動画は単独固定)。
/// (`GridItem` が `Debug` 非対応のため `Debug` は derive しない。)
#[derive(Clone)]
pub struct StackView {
    /// このビューが束縛されている実フォルダ。別フォルダへ移動したら破棄する。
    pub folder: PathBuf,
    /// グループ化区切り文字 (構築時の値)。
    pub separator: char,
    /// 構築時のソート順 (グループ/メンバーの並びはこれに従う)。
    pub sort: SortOrder,
    /// 画像以外のコンテナセル (両ビュー先頭の passthrough)。
    pub passthrough: Vec<GridItem>,
    /// `passthrough` と同インデックスの `(mtime, size)`。
    pub passthrough_metas: Vec<Option<(i64, i64)>>,
    /// メディアグループ (表示順)。
    pub groups: Vec<StackGroup>,
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
        Self::from_groups(
            folder,
            passthrough,
            passthrough_metas,
            separator,
            sort,
            groups,
        )
    }

    /// 既に算出済みのグループ列から `StackView` を作る (集約状態)。
    /// ユーザー定義スクリプト経由 ([`crate::filename_stack_script`] + [`group_by_keys`]) の
    /// グループをそのまま載せるための入口。
    pub fn from_groups(
        folder: PathBuf,
        passthrough: Vec<GridItem>,
        passthrough_metas: Vec<Option<(i64, i64)>>,
        separator: char,
        sort: SortOrder,
        groups: Vec<StackGroup>,
    ) -> Self {
        Self {
            folder,
            separator,
            sort,
            passthrough,
            passthrough_metas,
            groups,
        }
    }

    /// 畳めるスタック (画像 2 枚以上のグループ) が 1 つでもあるか。
    /// 集約しても全部単独セル = 通常一覧と同じ見た目なら、トグルしても無意味なので
    /// 呼び出し側がトーストで知らせる等に使える。
    pub fn has_collapsible_stack(&self) -> bool {
        self.groups.iter().any(|g| g.is_stack())
    }

    /// 集約ビュー (グリッド表示) の `(items, image_metas)` を作る。
    /// passthrough (コンテナ) → グループセル (1 グループ 1 セル) の順。
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

    /// フラット読書ビュー (フルスクリーン用) の `(items, image_metas)` を作る。
    /// passthrough → 全グループのメンバーを順に展開 (実 `Image`/`Video` セル)。
    /// これにより `↓↑` はスタック境界を越えて全画像を順送りできる。
    pub fn materialize_flat(&self) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
        let mut items = self.passthrough.clone();
        let mut metas = self.passthrough_metas.clone();
        for g in &self.groups {
            for m in &g.members {
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

    /// フラットビューでのグループ g の先頭メンバーの index。範囲外なら `None`。
    /// = passthrough 長 + g より前のグループのメンバー総数。
    pub fn flat_start_of_group(&self, g: usize) -> Option<usize> {
        if g >= self.groups.len() {
            return None;
        }
        let before: usize = self.groups[..g].iter().map(|x| x.members.len()).sum();
        Some(self.passthrough.len() + before)
    }

    /// フラットビューの index `flat_idx` が属するグループ index。passthrough 領域や
    /// 範囲外なら `None`。`Shift+↓↑` のジャンプ元判定 / 閉じたときの集約セル再選択に使う。
    pub fn group_of_flat_index(&self, flat_idx: usize) -> Option<usize> {
        let pt = self.passthrough.len();
        if flat_idx < pt {
            return None;
        }
        let mut offset = pt;
        for (g, group) in self.groups.iter().enumerate() {
            let end = offset + group.members.len();
            if flat_idx < end {
                return Some(g);
            }
            offset = end;
        }
        None
    }

    /// 集約ビューの index `agg_idx` をフラットビューでの先頭メンバー index へ写す。
    /// passthrough 領域 (コンテナ) は `None` (= 開く動作はフルスクリーンでなく通常ナビ)。
    pub fn flat_index_for_aggregated(&self, agg_idx: usize) -> Option<usize> {
        let pt = self.passthrough.len();
        if agg_idx < pt {
            return None;
        }
        self.flat_start_of_group(agg_idx - pt)
    }

    /// グループ g に対応する集約ビューの index (= passthrough 長 + g)。
    pub fn aggregated_index_of_group(&self, g: usize) -> usize {
        self.passthrough.len() + g
    }

    /// `Shift+↓↑` のジャンプ先 (フラットビュー index)。フラットビューの現在地 `cur` から:
    /// - `forward`: 次のスタックの先頭メンバー。最後のスタックなら `None`。
    /// - `backward`: スタック途中なら現スタックの先頭、既に先頭なら前のスタックの先頭。
    ///   先頭スタックの先頭なら `None`。
    /// `cur` が passthrough 領域 (コンテナ) のときは `None`。
    pub fn stack_jump_target(&self, cur: usize, forward: bool) -> Option<usize> {
        let g = self.group_of_flat_index(cur)?;
        if forward {
            self.flat_start_of_group(g + 1)
        } else {
            let start = self.flat_start_of_group(g)?;
            if cur > start {
                Some(start)
            } else {
                g.checked_sub(1).and_then(|pg| self.flat_start_of_group(pg))
            }
        }
    }

    /// `key` を持つグループの index を返す。
    pub fn group_index_by_key(&self, key: &str) -> Option<usize> {
        self.groups.iter().position(|g| g.key == key)
    }

    /// `path` をメンバーに含むグループの集約ビュー index を返す。スタックトグル時に「カーソル
    /// 位置の画像が含まれるスタックセル」を選択し直すのに使う。
    pub fn aggregated_index_for_member_path(&self, path: &Path) -> Option<usize> {
        self.groups
            .iter()
            .position(|g| g.members.iter().any(|m| m.path == path))
            .map(|g| self.aggregated_index_of_group(g))
    }
}

/// グループ内メンバーを表示 sort 順に並べる。
fn sort_members(members: &mut [StackMember], sort: SortOrder) {
    let mut keyed: Vec<_> = members
        .iter()
        .cloned()
        .map(|member| {
            let key = sort.name_key(name_of(&member.path));
            (member, key)
        })
        .collect();
    keyed.sort_by(|(a, ak), (b, bk)| sort.compare_name_keys(ak, a.mtime, bk, b.mtime));
    for (slot, (member, _)) in members.iter_mut().zip(keyed) {
        *slot = member;
    }
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
    fn materialize_flat_expands_all_members_after_passthrough() {
        // フォルダ 1 + スタック "post"(2枚) + 単独 "solo"。フラット = [folder, post_p0, post_p1, solo]。
        let media = vec![img("solo.jpg"), img("post_p1.jpg"), img("post_p0.jpg")];
        let sv = StackView::build(
            PathBuf::from(r"C:\dl"),
            vec![folder("sub")],
            vec![None],
            media,
            '_',
            SortOrder::FileName,
        );
        let (items, metas) = sv.materialize_flat();
        assert_eq!(items.len(), metas.len());
        assert!(matches!(items[0], GridItem::Folder(_)));
        let names: Vec<String> = items[1..].iter().map(item_name).collect();
        assert_eq!(names, vec!["post_p0.jpg", "post_p1.jpg", "solo.jpg"]);
        assert!(items[1..].iter().all(|i| matches!(i, GridItem::Image(_))));
    }

    #[test]
    fn flat_index_mapping_round_trips_with_groups() {
        // passthrough 1 (folder) + groups: post(2), solo(1)。
        // フラット index: 0=folder, 1=post_p0, 2=post_p1, 3=solo。
        // 集約 index:     0=folder, 1=Stack(post), 2=Image(solo)。
        let media = vec![img("post_p0.jpg"), img("post_p1.jpg"), img("solo.jpg")];
        let sv = StackView::build(
            PathBuf::from(r"C:\dl"),
            vec![folder("sub")],
            vec![None],
            media,
            '_',
            SortOrder::FileName,
        );
        // グループ起点。
        assert_eq!(sv.flat_start_of_group(0), Some(1)); // post → flat 1
        assert_eq!(sv.flat_start_of_group(1), Some(3)); // solo → flat 3
        assert_eq!(sv.flat_start_of_group(2), None);
        // 集約 index → フラット先頭。
        assert_eq!(sv.flat_index_for_aggregated(0), None); // passthrough folder
        assert_eq!(sv.flat_index_for_aggregated(1), Some(1)); // Stack(post) → flat 1
        assert_eq!(sv.flat_index_for_aggregated(2), Some(3)); // Image(solo) → flat 3
        // フラット index → グループ。
        assert_eq!(sv.group_of_flat_index(0), None); // passthrough
        assert_eq!(sv.group_of_flat_index(1), Some(0)); // post
        assert_eq!(sv.group_of_flat_index(2), Some(0)); // post の 2 枚目
        assert_eq!(sv.group_of_flat_index(3), Some(1)); // solo
        assert_eq!(sv.group_of_flat_index(99), None);
        // グループ → 集約 index。
        assert_eq!(sv.aggregated_index_of_group(0), 1);
        assert_eq!(sv.aggregated_index_of_group(1), 2);
    }

    #[test]
    fn stack_jump_target_moves_between_stacks() {
        // passthrough 1(folder) + post(2 @1,2) + solo(1 @3)。
        let media = vec![img("post_p0.jpg"), img("post_p1.jpg"), img("solo.jpg")];
        let sv = StackView::build(
            PathBuf::from(r"C:\dl"),
            vec![folder("sub")],
            vec![None],
            media,
            '_',
            SortOrder::FileName,
        );
        // forward: 現スタックのどこからでも次スタック先頭へ。最後は None。
        assert_eq!(sv.stack_jump_target(1, true), Some(3)); // post p0 → solo
        assert_eq!(sv.stack_jump_target(2, true), Some(3)); // post p1 → solo
        assert_eq!(sv.stack_jump_target(3, true), None); // solo (最後) → 端
        // backward: 途中なら現スタック先頭、先頭なら前スタック先頭、先頭スタック先頭は None。
        assert_eq!(sv.stack_jump_target(2, false), Some(1)); // post p1 → post 先頭
        assert_eq!(sv.stack_jump_target(1, false), None); // post 先頭 (最初) → 端
        assert_eq!(sv.stack_jump_target(3, false), Some(1)); // solo → 前スタック post 先頭
        // passthrough (コンテナ) からは None。
        assert_eq!(sv.stack_jump_target(0, true), None);
        assert_eq!(sv.stack_jump_target(0, false), None);
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

    #[test]
    fn aggregated_index_for_member_path_finds_containing_stack() {
        // 集約: [0]=folder, [1]=Stack(post 2枚), [2]=Image(solo)。
        let media = vec![img("post_p0.jpg"), img("post_p1.jpg"), img("solo.jpg")];
        let sv = StackView::build(
            PathBuf::from(r"C:\dl"),
            vec![folder("sub")],
            vec![None],
            media,
            '_',
            SortOrder::FileName,
        );
        // スタック内のどのメンバーからでもそのスタックセル (集約 index 1) に解決。
        assert_eq!(
            sv.aggregated_index_for_member_path(&PathBuf::from(r"C:\dl\post_p0.jpg")),
            Some(1)
        );
        assert_eq!(
            sv.aggregated_index_for_member_path(&PathBuf::from(r"C:\dl\post_p1.jpg")),
            Some(1)
        );
        // 単独画像は自身のセル (集約 index 2)。
        assert_eq!(
            sv.aggregated_index_for_member_path(&PathBuf::from(r"C:\dl\solo.jpg")),
            Some(2)
        );
        // 含まれないパスは None。
        assert_eq!(
            sv.aggregated_index_for_member_path(&PathBuf::from(r"C:\dl\nope.jpg")),
            None
        );
    }
}
