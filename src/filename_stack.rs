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
}
