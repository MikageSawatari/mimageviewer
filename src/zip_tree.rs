//! ネスト ZIP のメモリ常駐ツリー (v1.3.0、`docs/nested-zip-tree-plan.md` Strategy A)。
//!
//! 外側 ZIP の列挙結果 (`Vec<ZipImageEntry>`, `entry_name` は `/` 区切りのフルパス)
//! を、ディレクトリ階層を保持したトライ木に組み替える純データ構造。フラット展開
//! (全画像を 1 本の線形リストに平坦化) と違い、内側 ZIP / サブフォルダを「本」の
//! 境界として保持するので、見開きペアリングを本ごとにリセットできる。
//!
//! # 中核原則: `entry_name` を一切変えない
//!
//! 葉ノードに入る画像は **元の列挙文字列 (`"chapters/ch01.zip/page01.jpg"` のような
//! フルパス) をそのまま保持**する。回転 / 補正 / レーティング / 消しゴム / 隠蔽 /
//! ローカル調整 / タグ / サイドカー / サムネカタログ / 検索索引の永続キー 7 系統は
//! すべて `entry_name` をキーに埋め込むため、これを変えなければ DB マイグレーションは
//! 完全に不要になる (既存ユーザーデータが全部生存する)。ツリーは「どのページを今
//! 表示するか」という **表示・ナビゲーション層** だけを追加するものに留める。
//!
//! `.zip/` `.cbz/` 境界は `entry_name` 中で既に `/` 区切りになっているので、
//! `entry_name` を `/` で split するだけでツリーになる (`.zip`/`.cbz` で終わる
//! セグメントは構造上ただのディレクトリ階層。バイト読み出しの `.zip/` 特別扱いは
//! 既存 `zip_loader::read_entry_bytes` が担う)。
//!
//! このモジュールは **I/O を一切行わない純ロジック**なので、UI スレッドで列挙完了
//! 直後に構築してもブロックしない (`d1a6e99f` の 2.3 秒 UI ブロック教訓: 重い列挙
//! 自体は引き続きワーカーで行い、ここでの split+trie は 1100 エントリでも数 ms)。

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::zip_loader::{ZipImageEntry, entry_basename, entry_dir};

/// 開いている外側 ZIP の階層ツリー (列挙完了時に構築し、ナビ中は保持)。
#[derive(Debug)]
pub struct ZipTree {
    /// 外側 ZIP の実ファイルパス (= 仮想フォルダのルート identity)。
    pub zip_path: PathBuf,
    /// ルート階層 (ZIP 直下)。
    pub root: ZipTreeNode,
}

/// ツリーの 1 階層。子ディレクトリ (内側アーカイブ含む) と、その階層直下の画像を持つ。
#[derive(Debug, Default)]
pub struct ZipTreeNode {
    /// 子ディレクトリ / 内側アーカイブ。
    /// キーは `entry_name` のセグメント文字列を**そのまま** (case 等を改変せず) 保持する。
    /// `BTreeMap` は決定的な構築順を与えるが、表示用のソートは別途 `SortOrder` で行う
    /// (= ここでの並びは表示順ではない)。
    pub dirs: BTreeMap<String, ZipTreeNode>,
    /// この階層直下の画像。`entry_name` はフルパスを保持する (DB キー・
    /// `read_entry_bytes` 互換)。並びは列挙順 (= ZIP 内出現順)。
    pub images: Vec<ZipImageEntry>,
}

impl ZipTree {
    /// 列挙結果からツリーを構築する。
    ///
    /// 各 `entry_name` を `entry_dir` (親ディレクトリ) で辿ってノードを作り、葉ノードの
    /// `images` に push する。画像の `entry_name` は一切改変しない。
    ///
    /// 防御的処理:
    /// - basename が空のエントリ (例: `"dir/"`、通常は列挙されない) はスキップ。
    /// - 連続スラッシュ (`"a//b.jpg"`) は空セグメントを除去して `a > b.jpg` 扱い。
    ///   これは意図的なナビ正規化で、`"a/b.jpg"` と `"a//b.jpg"` は同じ階層ノードに
    ///   alias される (= 同じ navigation prefix を共有)。ただし葉の `entry_name` は
    ///   元の文字列のまま (DB キー identity は崩さない) (Codex P3)。
    pub fn build(zip_path: PathBuf, entries: Vec<ZipImageEntry>) -> Self {
        let mut root = ZipTreeNode::default();
        for entry in entries {
            // ファイル名が無い (ディレクトリ的) エントリは構造に意味がないので捨てる。
            if entry_basename(&entry.entry_name).is_empty() {
                continue;
            }
            let dir = entry_dir(&entry.entry_name);
            let mut node = &mut root;
            for seg in dir.split('/').filter(|s| !s.is_empty()) {
                node = node.dirs.entry(seg.to_string()).or_default();
            }
            node.images.push(entry);
        }
        Self { zip_path, root }
    }

    /// `prefix` (セグメント列、空 = ルート) が指す階層ノードを返す。
    /// 途中のセグメントが存在しなければ `None`。
    pub fn node_at(&self, prefix: &[String]) -> Option<&ZipTreeNode> {
        let mut node = &self.root;
        for seg in prefix {
            node = node.dirs.get(seg)?;
        }
        Some(node)
    }

    /// 冗長ラッパー階層の自動降下 (D1)。
    ///
    /// `start` から始めて、「直下画像 0 枚・子ディレクトリちょうど 1 個」の階層を
    /// その唯一の子へ降り続け、画像を含む階層 or 分岐する階層に達したら止まる。
    /// 戻り値は降下後の実効 prefix。
    ///
    /// 例: `ZIP > vol01 > [pages]` のような単一フォルダラッパーは `collapse_redundant(&[])`
    /// で `["vol01"]` まで降りて、本体のページ階層を直接見せる。真に複数本を含む
    /// アーカイブ (`ZIP > {ch01.zip, ch02.zip}`) は分岐で止まるのでツリーのまま残る。
    ///
    /// `start` のノードが存在しない場合は `start` をそのまま返す (判定は呼び出し側)。
    ///
    /// ⚠ **Phase 3 ナビ契約 (Codex P2)**: これは **表示 (materialize) 時のみ**適用する
    /// 純粋な view 変換であり、**戻り値を navigation prefix として保存し直してはならない**。
    /// `ZipNavState.prefix` は常に「ユーザーが明示的に降りた論理 prefix」を保持し、
    /// collapse は描画直前に都度かけること。保存し直すと Backspace が
    /// `["vol01"] -> pop -> [] -> collapse -> ["vol01"]` でループして抜けられなくなる。
    /// 論理 prefix が空 (= ルート) のとき collapse 後の表示から Backspace すると ZIP を
    /// 抜けて実フォルダ親へ戻る (= ラッパーは「入った」扱いにしない) のが正しい挙動。
    pub fn collapse_redundant(&self, start: &[String]) -> Vec<String> {
        let mut prefix: Vec<String> = start.to_vec();
        loop {
            let Some(node) = self.node_at(&prefix) else {
                break;
            };
            if !node.images.is_empty() || node.dirs.len() != 1 {
                break;
            }
            // 子が 1 個・画像 0 枚 → その子に降りる。
            let only = node
                .dirs
                .keys()
                .next()
                .expect("dirs.len()==1 guarantees one key")
                .clone();
            prefix.push(only);
        }
        prefix
    }
}

impl ZipTreeNode {
    /// この部分木に画像が 1 枚でもあるか。
    pub fn has_any_image(&self) -> bool {
        !self.images.is_empty() || self.dirs.values().any(|d| d.has_any_image())
    }

    /// この部分木に含まれる画像の総数 (再帰)。
    pub fn total_image_count(&self) -> usize {
        self.images.len()
            + self
                .dirs
                .values()
                .map(|d| d.total_image_count())
                .sum::<usize>()
    }

    /// この部分木の代表画像 (= ZipDir セルのサムネ候補) を返す。
    ///
    /// 深さ優先で「この階層の直下画像 → 子ディレクトリ (BTreeMap キー順)」の順に探し、
    /// 最初に見つかった画像を返す。これは決定的だが **表示ソート順の先頭ではない**
    /// (直下画像は列挙順の先頭、子は BTreeMap キー順)。
    ///
    /// 代表サムネ選定は cosmetic なので Phase 1 ではこの決定的順序で十分。**これは
    /// あくまで sort 非対応の fallback** であり、Phase 2 の ZipDir 代表サムネ選定は
    /// 表示 `SortOrder` に準拠した先頭画像を別途選ぶべき (Codex P3)。
    pub fn first_image_in_subtree(&self) -> Option<&ZipImageEntry> {
        if let Some(img) = self.images.first() {
            return Some(img);
        }
        for child in self.dirs.values() {
            if let Some(img) = child.first_image_in_subtree() {
                return Some(img);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用に `entry_name` だけ指定して `ZipImageEntry` を作る。
    fn e(name: &str) -> ZipImageEntry {
        ZipImageEntry {
            entry_name: name.to_string(),
            uncompressed_size: 0,
            mtime: 0,
        }
    }

    fn tree(names: &[&str]) -> ZipTree {
        ZipTree::build(
            PathBuf::from("C:/test/outer.zip"),
            names.iter().map(|n| e(n)).collect(),
        )
    }

    fn img_names(node: &ZipTreeNode) -> Vec<&str> {
        node.images.iter().map(|i| i.entry_name.as_str()).collect()
    }

    fn dir_keys(node: &ZipTreeNode) -> Vec<&str> {
        node.dirs.keys().map(|s| s.as_str()).collect()
    }

    #[test]
    fn build_flat_root() {
        let t = tree(&["a.jpg", "b.jpg", "c.jpg"]);
        assert!(t.root.dirs.is_empty());
        assert_eq!(img_names(&t.root), vec!["a.jpg", "b.jpg", "c.jpg"]);
    }

    #[test]
    fn build_one_subfolder() {
        let t = tree(&["work1/a.jpg", "work1/b.jpg"]);
        assert!(t.root.images.is_empty());
        assert_eq!(dir_keys(&t.root), vec!["work1"]);
        let sub = t.root.dirs.get("work1").unwrap();
        assert_eq!(img_names(sub), vec!["work1/a.jpg", "work1/b.jpg"]);
    }

    #[test]
    fn build_nested_zip_path_preserves_full_entry_name() {
        // .zip/ 境界はただのディレクトリ階層として組まれる。葉の entry_name は不変。
        let t = tree(&["chapters/ch01.zip/page01.jpg"]);
        let node = t
            .node_at(&["chapters".into(), "ch01.zip".into()])
            .expect("nested node exists");
        // ★ DB キー identity: 葉の entry_name は元のフル文字列をそのまま保持する。
        assert_eq!(img_names(node), vec!["chapters/ch01.zip/page01.jpg"]);
    }

    #[test]
    fn build_multiple_books() {
        let t = tree(&["ch01.zip/p01.jpg", "ch01.zip/p02.jpg", "ch02.zip/p01.jpg"]);
        assert!(t.root.images.is_empty());
        assert_eq!(dir_keys(&t.root), vec!["ch01.zip", "ch02.zip"]);
        assert_eq!(t.root.dirs.get("ch01.zip").unwrap().images.len(), 2);
        assert_eq!(t.root.dirs.get("ch02.zip").unwrap().images.len(), 1);
    }

    #[test]
    fn build_mixed_dir_and_images_same_level() {
        let t = tree(&["a.jpg", "sub/b.jpg"]);
        assert_eq!(img_names(&t.root), vec!["a.jpg"]);
        assert_eq!(dir_keys(&t.root), vec!["sub"]);
        assert_eq!(
            img_names(t.root.dirs.get("sub").unwrap()),
            vec!["sub/b.jpg"]
        );
    }

    #[test]
    fn node_at_root_is_root() {
        let t = tree(&["a.jpg"]);
        let node = t.node_at(&[]).unwrap();
        assert_eq!(img_names(node), vec!["a.jpg"]);
    }

    #[test]
    fn node_at_missing_returns_none() {
        let t = tree(&["work1/a.jpg"]);
        assert!(t.node_at(&["nope".into()]).is_none());
        assert!(t.node_at(&["work1".into(), "deeper".into()]).is_none());
    }

    #[test]
    fn collapse_single_wrapper() {
        // ZIP > vol01 > [pages]: 単一フォルダラッパーは vol01 まで自動降下。
        let t = tree(&["vol01/p01.jpg", "vol01/p02.jpg"]);
        assert_eq!(t.collapse_redundant(&[]), vec!["vol01".to_string()]);
    }

    #[test]
    fn collapse_deep_chain() {
        // ZIP > a > b > [pages]: 連続する単一ラッパーをまとめて降下。
        let t = tree(&["a/b/p01.jpg", "a/b/p02.jpg"]);
        assert_eq!(
            t.collapse_redundant(&[]),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn collapse_stops_at_branch() {
        // 複数本を含むアーカイブは分岐で止まる (ツリーのまま)。
        let t = tree(&["ch01.zip/p.jpg", "ch02.zip/p.jpg"]);
        assert_eq!(t.collapse_redundant(&[]), Vec::<String>::new());
    }

    #[test]
    fn collapse_stops_when_images_present() {
        // ルート直下に画像があれば降下しない (本体がそこにある)。
        let t = tree(&["cover.jpg", "extra/p.jpg"]);
        assert_eq!(t.collapse_redundant(&[]), Vec::<String>::new());
    }

    #[test]
    fn collapse_from_subprefix() {
        // 途中階層からの降下も同様に働く。
        let t = tree(&["chapters/only/p01.jpg", "chapters/only/p02.jpg"]);
        assert_eq!(
            t.collapse_redundant(&["chapters".to_string()]),
            vec!["chapters".to_string(), "only".to_string()]
        );
    }

    #[test]
    fn collapse_empty_tree_is_noop() {
        let t = tree(&[]);
        assert_eq!(t.collapse_redundant(&[]), Vec::<String>::new());
    }

    #[test]
    fn first_image_in_subtree_prefers_direct_then_dfs() {
        // 直下画像があればそれを優先。
        let t = tree(&["root.jpg", "sub/child.jpg"]);
        assert_eq!(
            t.root.first_image_in_subtree().unwrap().entry_name,
            "root.jpg"
        );
        // 直下に無い場合は子 (BTreeMap 順) を DFS。
        let t2 = tree(&["b_dir/x.jpg", "a_dir/y.jpg"]);
        assert_eq!(
            t2.root.first_image_in_subtree().unwrap().entry_name,
            "a_dir/y.jpg"
        );
    }

    #[test]
    fn total_image_count_recurses() {
        let t = tree(&[
            "a.jpg",
            "ch01.zip/p01.jpg",
            "ch01.zip/p02.jpg",
            "ch02.zip/sub/p01.jpg",
        ]);
        assert_eq!(t.root.total_image_count(), 4);
        assert_eq!(t.root.dirs.get("ch01.zip").unwrap().total_image_count(), 2);
        assert_eq!(t.root.dirs.get("ch02.zip").unwrap().total_image_count(), 1);
    }

    #[test]
    fn has_any_image() {
        assert!(tree(&["a.jpg"]).root.has_any_image());
        assert!(tree(&["d/a.jpg"]).root.has_any_image());
        assert!(!tree(&[]).root.has_any_image());
    }

    #[test]
    fn defensive_empty_basename_skipped() {
        // "dir/" のような basename 空エントリは構造に入れない。
        let t = tree(&["dir/", "real/p.jpg"]);
        assert_eq!(dir_keys(&t.root), vec!["real"]);
        assert_eq!(t.root.total_image_count(), 1);
    }

    #[test]
    fn defensive_double_slash_collapsed() {
        // 連続スラッシュは空セグメント除去で a > b.jpg 扱い。
        let t = tree(&["a//b.jpg"]);
        let node = t.node_at(&["a".to_string()]).unwrap();
        // 葉の entry_name は元のまま (改変しない)。
        assert_eq!(img_names(node), vec!["a//b.jpg"]);
    }
}
