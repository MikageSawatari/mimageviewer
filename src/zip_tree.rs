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
use std::sync::Arc;

use crate::grid_item::GridItem;
use crate::settings::SortOrder;
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
    /// ⚠ **Phase 3 ナビ契約 (Codex P2)**: collapse は「次に降りる階層の実効 prefix」を
    /// 算出するためだけに使う。Phase 3 は **collapse 後の実効 prefix を nav スタックに
    /// 積む** (= スタックの各エントリは描画した実効 prefix)。Backspace はスタックを 1 段
    /// pop する (= 直前に描画した実効 prefix へ戻る)。スタック最下段 (ルート) で Backspace
    /// すると ZIP を抜けて実フォルダ親へ戻る。
    ///
    /// 「論理 prefix を 1 セグメントずつ pop して毎回 collapse し直す」方式は採らない。
    /// それだと root が単一ラッパーへ collapse する木で
    /// `[] -> 表示は ["vol01"] -> back で [] -> 再 collapse で ["vol01"]` と同じ画面に
    /// 戻り続けて抜けられなくなる。スタック方式ならこの罠を構造的に回避できる。
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

    /// `prefix` (= **既に collapse 済みの実効 prefix**) の階層を `GridItem` 列に変換する。
    ///
    /// 戻り値は `(items, image_metas)` で、`image_metas[i]` は `items[i]` に対応する
    /// `Some((mtime, uncompressed_size))` (コンテナ ZipDir は `None`)。
    /// `finalize_zip_enumerate` / Phase 3 ナビが `start_loading_items` にそのまま渡せる
    /// 形にする (= 既存フラット経路と同じ型)。
    ///
    /// 並び: グリッド慣習に従い **子コンテナ (ZipDir) を先・画像 (ZipImage) を後**。
    /// どちらも `sort` で並べる。ZipDir のソートキーはセグメント名 (mtime なし)、
    /// ZipImage は basename + mtime。
    ///
    /// collapse はここでは行わない (純粋な「この階層を描く」関数)。ラッパー自動降下が
    /// 必要な呼び出し側は `collapse_redundant` で実効 prefix を出してから渡すこと。
    pub fn materialize_level(
        &self,
        prefix: &[String],
        sort: SortOrder,
    ) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
        let mut items: Vec<GridItem> = Vec::new();
        let mut metas: Vec<Option<(i64, i64)>> = Vec::new();
        let Some(node) = self.node_at(prefix) else {
            return (items, metas);
        };

        // 現在階層までの prefix 文字列 ("chapters/" や ""、末尾 '/' 付き)。
        let prefix_str = if prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", prefix.join("/"))
        };

        // 1) 子コンテナ (ZipDir)。セグメント名で sort。
        let mut dirs: Vec<(&String, &ZipTreeNode)> = node.dirs.iter().collect();
        dirs.sort_by(|(a, _), (b, _)| {
            sort.compare(a, 0, b, 0, crate::ui_helpers::natural_sort_key)
        });
        for (seg, child) in dirs {
            // 代表画像。build() は画像エントリの経路上にしかノードを作らないので、
            // 任意の子ノードは部分木に必ず 1 枚以上画像を持つ → rep は実質的に常に Some。
            let rep = representative_image(child, sort);
            items.push(GridItem::ZipDir {
                zip_path: self.zip_path.clone(),
                dir_prefix: format!("{prefix_str}{seg}/"),
                is_archive: segment_is_archive(seg),
                representative: rep.map(|e| e.entry_name.clone()),
            });
            // 代表画像の mtime/size を meta に載せる。これがないと enqueue 経路
            // (image_metas[i]==None を skip) が ZipDir のサムネ要求を出さず、セルが
            // 永久 Pending → 毎フレーム repaint になる (Phase 2 Codex P2)。size 列に
            // 代表画像サイズが出るのは許容 (date 列は details_created_time_path で None)。
            metas.push(rep.map(|e| (e.mtime, e.uncompressed_size as i64)));
        }

        // 2) 直下画像 (ZipImage)。basename + mtime で sort。
        let mut imgs: Vec<&ZipImageEntry> = node.images.iter().collect();
        imgs.sort_by(|a, b| {
            let an = entry_basename(&a.entry_name);
            let bn = entry_basename(&b.entry_name);
            sort.compare(
                an,
                a.mtime,
                bn,
                b.mtime,
                crate::ui_helpers::natural_sort_key,
            )
        });
        for e in imgs {
            items.push(GridItem::ZipImage {
                zip_path: self.zip_path.clone(),
                entry_name: e.entry_name.clone(),
            });
            metas.push(Some((e.mtime, e.uncompressed_size as i64)));
        }

        (items, metas)
    }

    /// ツリー全階層の catalog cache キーを集める (Phase 3 の `existing_keys` 用)。
    ///
    /// = 全 leaf 画像の `entry_name` + 全ディレクトリの `zipdir:{prefix}`。
    /// フラット実装は finalize が全 entry_name を `existing_keys` に入れて `delete_missing`
    /// のプルーン基準にしていた。ツリーでは finalize がルート階層しか materialize しない
    /// ため、ここで**全階層**のキーを集めて渡さないと、深い階層のサムネが再オープン時に
    /// stale 削除されて再生成ループになる (Phase 2 Codex P2)。
    pub fn all_cache_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        let mut prefix: Vec<String> = Vec::new();
        collect_cache_keys(&self.root, &mut prefix, &mut keys);
        keys
    }
}

/// `all_cache_keys` の再帰本体。`prefix` は現在辿っている dir セグメント列。
fn collect_cache_keys(node: &ZipTreeNode, prefix: &mut Vec<String>, out: &mut Vec<String>) {
    for img in &node.images {
        out.push(img.entry_name.clone());
    }
    for (seg, child) in &node.dirs {
        prefix.push(seg.clone());
        // materialize_level の dir_prefix と同形式 ("a/b/"、末尾 '/') にする。
        let dir_prefix = format!("{}/", prefix.join("/"));
        out.push(crate::grid_item::zipdir_cache_key(&dir_prefix));
        collect_cache_keys(child, prefix, out);
        prefix.pop();
    }
}

/// 開いている外側 ZIP のツリーナビ状態 (Phase 3)。
///
/// スタックの各エントリは「**実際に描画した実効 prefix**」(= `collapse_redundant` 済み)。
/// `stack[0]` はルートを collapse した初期表示で、スタックは常に非空。
/// 「論理 prefix を 1 セグメントずつ pop して毎回 collapse し直す」方式の Backspace 罠
/// (`[] -> 表示 ["vol01"] -> back [] -> 再 collapse ["vol01"]` の無限ループ) を、
/// 描画した実効 prefix をそのまま積む/pop することで構造的に回避する (Codex P2)。
///
/// `Clone` は ★固定 (snapshot) の退避用 (`SnapshotState::saved_zip_nav`)。`tree` は
/// `Arc` 共有なので clone は stack の浅いコピーだけで済む。
#[derive(Clone)]
pub struct ZipNavState {
    /// 開いている外側 ZIP のツリー (列挙完了時に構築、ナビ中は保持)。
    pub tree: Arc<ZipTree>,
    /// 実効 prefix のスタック。`stack[0]` = `collapse_redundant(&[])`。常に非空。
    stack: Vec<Vec<String>>,
}

impl ZipNavState {
    /// ZIP を開いたときの初期状態。ルートを collapse した実効 prefix を底に積む。
    pub fn new(tree: Arc<ZipTree>) -> Self {
        let root_effective = tree.collapse_redundant(&[]);
        Self {
            tree,
            stack: vec![root_effective],
        }
    }

    /// 現在描画すべき実効 prefix。
    pub fn current(&self) -> &[String] {
        self.stack.last().expect("ZipNavState stack is never empty")
    }

    /// 現在が ZIP のルート表示か (= スタック底)。Backspace で ZIP を抜けるか判定に使う。
    pub fn at_root(&self) -> bool {
        self.stack.len() == 1
    }

    /// 現在階層を親から見たときの `ZipDir.dir_prefix` 相当 prefix を返す。
    ///
    /// `current()` は collapse 後の実効 prefix なので、`bookB/` に入って
    /// `bookB/only/` まで自動降下した場合は `["bookB", "only"]` になる。親階層で
    /// 表示されるセルは literal な `bookB/` なので、親 prefix + 次の 1 セグメントだけを
    /// 返す。ルート表示では親セルが無いため `None`。
    pub fn current_parent_zipdir_prefix(&self) -> Option<Vec<String>> {
        if self.stack.len() < 2 {
            return None;
        }
        let parent = &self.stack[self.stack.len() - 2];
        let current = self.current();
        if current.len() <= parent.len() || !current.starts_with(parent) {
            return None;
        }
        let mut prefix = parent.clone();
        prefix.push(current[parent.len()].clone());
        Some(prefix)
    }

    /// `ZipDir` セルへ降りる。`dir_prefix` は `GridItem::ZipDir.dir_prefix`
    /// ("a/b/" 形式、ルートからの絶対パス、末尾 '/')。子の実効 prefix を `collapse_redundant`
    /// で算出してスタックに積む。
    pub fn enter(&mut self, dir_prefix: &str) {
        let segs = split_prefix(dir_prefix);
        let effective = self.tree.collapse_redundant(&segs);
        self.stack.push(effective);
    }

    /// 1 段戻る。スタックを pop する。戻れたら `true`、既にルート (底) なら `false`
    /// (= 呼び出し側が ZIP を抜けて実フォルダ親へ遷移する)。
    pub fn back(&mut self) -> bool {
        if self.stack.len() <= 1 {
            return false;
        }
        self.stack.pop();
        true
    }

    /// Ctrl+↑↓ の ZIP 内ナビを **DFS 前順トラバーサル** で 1 ノード進める/戻す。
    /// 実フォルダの Ctrl+↑↓ (深さ優先) と同じ規則をツリーノード (= 階層/本) に適用する:
    /// - 次 (`forward=true`) = 最初の子 → 次の兄弟 → 祖先の次の兄弟 (再帰)
    /// - 前 (`forward=false`) = 前の兄弟の最後の子孫 → 親
    ///
    /// ルート表示でも子の本があれば降りる (ZIP の中身を実フォルダツリーと同様に扱う)。
    /// 進めたら stack を更新して `true`。ツリーの端 (forward=最後のノードの後 /
    /// backward=ルートの前) では **stack を変更せず** `false` を返す (= 呼び出し側が
    /// ZIP を抜けて実フォルダの DFS ナビへ脱出する)。forward の遡行探索は途中で pop
    /// しながら進むため、作業はローカルコピーで行い確定時のみ書き戻す (端で抜ける
    /// ケースに巻き戻り状態が残ると、以降の BS / ピン / 見開きキーが表示とずれる)。
    ///
    /// 各降下は `collapse_redundant` を通す (単一ラッパーは中間段として現れない)。
    /// 兄弟列・子列の並びは `materialize_level` の ZipDir 並びと同じ sort 順。
    pub fn dfs_step(&mut self, forward: bool, sort: SortOrder) -> bool {
        let mut stack = self.stack.clone();
        let moved = if forward {
            dfs_forward_step(&self.tree, &mut stack, sort)
        } else {
            dfs_backward_step(&self.tree, &mut stack, sort)
        };
        if moved {
            self.stack = stack;
        }
        moved
    }

    /// 現在階層を materialize した `(items, image_metas)` を返す薄いラッパー。
    pub fn materialize_current(&self, sort: SortOrder) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
        self.tree.materialize_level(self.current(), sort)
    }
}

/// `node` の子ディレクトリ名を表示 sort 順 (= `materialize_level` の ZipDir 並び) で返す。
fn sorted_dirs(node: &ZipTreeNode, sort: SortOrder) -> Vec<&String> {
    let mut dirs: Vec<&String> = node.dirs.keys().collect();
    dirs.sort_by(|a, b| sort.compare(a, 0, b, 0, crate::ui_helpers::natural_sort_key));
    dirs
}

/// `stack` 最上段ノードについて、親 (`stack[len-2]`) の sort 済み子ディレクトリ列と、
/// その中での現在ノードの位置を返す。スタック深さ < 2 (= ルート表示) や tree との
/// 不整合時は None。collapse で深く降りた実効 prefix でも、親から見た分岐セグメント
/// は `parent.len()` 位置にある (実効 prefix は親の実効 prefix の真の延長のため)。
fn parent_dirs_and_pos<'t>(
    tree: &'t ZipTree,
    stack: &[Vec<String>],
    sort: SortOrder,
) -> Option<(Vec<&'t String>, usize)> {
    if stack.len() < 2 {
        return None;
    }
    let parent = &stack[stack.len() - 2];
    let seg = stack.last()?.get(parent.len())?;
    let parent_node = tree.node_at(parent)?;
    let dirs = sorted_dirs(parent_node, sort);
    let pos = dirs.iter().position(|d| *d == seg)?;
    Some((dirs, pos))
}

/// DFS 前順で次のノードへ。進めたら `true` (stack 更新済み)。ツリー末尾なら `false` —
/// このとき stack は遡行 pop で壊れているので、**呼び出し側は必ずコピーに対して使う**
/// (`ZipNavState::dfs_step` がその規約を担保する)。
fn dfs_forward_step(tree: &ZipTree, stack: &mut Vec<Vec<String>>, sort: SortOrder) -> bool {
    // 1) 子があれば sort 先頭の子へ降りる (ルートの本一覧からも最初の本へ降りる)。
    let current = stack.last().expect("zip nav stack is never empty").clone();
    if let Some(node) = tree.node_at(&current) {
        if let Some(first) = sorted_dirs(node, sort).first() {
            let mut child = current;
            child.push((*first).clone());
            stack.push(tree.collapse_redundant(&child));
            return true;
        }
    }
    // 2) 葉: 次の兄弟へ。無ければ祖先を遡って「祖先の次の兄弟」を探す。
    loop {
        let Some((dirs, pos)) = parent_dirs_and_pos(tree, stack, sort) else {
            return false; // ルートまで遡って次が無い = ツリーの端
        };
        if let Some(next_seg) = dirs.get(pos + 1) {
            let next_seg = (*next_seg).clone();
            let mut child = stack[stack.len() - 2].clone();
            child.push(next_seg);
            let effective = tree.collapse_redundant(&child);
            stack.pop();
            stack.push(effective);
            return true;
        }
        stack.pop(); // この階層に次の兄弟なし → 親へ遡って続行
    }
}

/// DFS 逆前順で前のノードへ: 前の兄弟が居ればその「最後の子孫」へ降り切り、居なければ
/// 親へ上がる。ルート表示では `false` (stack 不変、= ZIP の前へ抜ける)。
fn dfs_backward_step(tree: &ZipTree, stack: &mut Vec<Vec<String>>, sort: SortOrder) -> bool {
    let Some((dirs, pos)) = parent_dirs_and_pos(tree, stack, sort) else {
        return false;
    };
    let Some(prev_pos) = pos.checked_sub(1) else {
        // 最初の子: 前順の 1 つ前 = 親階層そのもの。
        stack.pop();
        return true;
    };
    let prev_seg = dirs[prev_pos].clone();
    let mut effective = stack[stack.len() - 2].clone();
    effective.push(prev_seg);
    let mut effective = tree.collapse_redundant(&effective);
    stack.pop();
    stack.push(effective.clone());
    // 前の兄弟の「最後の子孫」へ、各段 stack に積みながら降り切る (enter と同じ形を
    // 保つので、着地後の BS が 1 段ずつ正しく戻れる)。
    loop {
        let Some(last) = tree
            .node_at(&effective)
            .and_then(|n| sorted_dirs(n, sort).last().map(|s| (*s).clone()))
        else {
            break;
        };
        effective.push(last);
        effective = tree.collapse_redundant(&effective);
        stack.push(effective.clone());
    }
    true
}

/// "a/b/" 形式の dir_prefix をセグメント列に分解 (末尾 '/'・空セグメント除去)。
fn split_prefix(dir_prefix: &str) -> Vec<String> {
    dir_prefix
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// セグメント名 (entry_name の一区切り) が内側アーカイブ (.zip / .cbz) か。
/// `ZipDir.is_archive` バッジ用の suffix 推定 (accepted ambiguity、計画書 §9)。
fn segment_is_archive(seg: &str) -> bool {
    let lower = seg.to_ascii_lowercase();
    // .rar 以下は変換キャッシュ ZIP のフラットパス ("inner.rar/p01.jpg") 由来の
    // セグメント (v1.3.0 入れ子展開)。実 ZIP 内の生 RAR は列挙されないので、
    // これらが現れるのは展開済みキャッシュを開いたときだけ。
    [
        ".zip", ".cbz", ".rar", ".cbr", ".7z", ".cb7", ".lzh", ".lha",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// 部分木の代表画像 (= ZipDir 代表サムネ) を **sort 準拠**で選ぶ。
///
/// この階層に直下画像があれば sort 先頭を返す。無ければ sort 先頭の子ディレクトリへ
/// 再帰する (= 「最初の本の 1 ページ目」を表紙にする)。`ZipTreeNode::first_image_in_subtree`
/// (列挙順 fallback) と違い、表示順の先頭ページを選ぶ (Codex P3 対応)。
fn representative_image(node: &ZipTreeNode, sort: SortOrder) -> Option<&ZipImageEntry> {
    if !node.images.is_empty() {
        return node.images.iter().min_by(|a, b| {
            let an = entry_basename(&a.entry_name);
            let bn = entry_basename(&b.entry_name);
            sort.compare(
                an,
                a.mtime,
                bn,
                b.mtime,
                crate::ui_helpers::natural_sort_key,
            )
        });
    }
    node.dirs
        .iter()
        .min_by(|(a, _), (b, _)| sort.compare(a, 0, b, 0, crate::ui_helpers::natural_sort_key))
        .and_then(|(_, child)| representative_image(child, sort))
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

    // ── materialize_level / 代表サムネ / is_archive ──────────────────

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// ZipDir セルの (dir_prefix, is_archive, representative) を取り出す。
    fn as_zipdir(item: &GridItem) -> (&str, bool, Option<&str>) {
        match item {
            GridItem::ZipDir {
                dir_prefix,
                is_archive,
                representative,
                ..
            } => (dir_prefix.as_str(), *is_archive, representative.as_deref()),
            _ => panic!("expected GridItem::ZipDir"),
        }
    }

    /// ZipImage セルの entry_name を取り出す。
    fn as_zipimage(item: &GridItem) -> &str {
        match item {
            GridItem::ZipImage { entry_name, .. } => entry_name.as_str(),
            _ => panic!("expected GridItem::ZipImage"),
        }
    }

    #[test]
    fn segment_is_archive_detects_zip_cbz() {
        assert!(segment_is_archive("ch01.zip"));
        assert!(segment_is_archive("ch01.CBZ"));
        assert!(segment_is_archive("Vol.Zip"));
        assert!(!segment_is_archive("chapters"));
        // v1.3.0: 変換キャッシュのフラットパス由来セグメント (入れ子展開) もアーカイブバッジ。
        assert!(segment_is_archive("ch01.rar"));
        assert!(segment_is_archive("ch02.7z"));
        assert!(segment_is_archive("old.LZH"));
    }

    #[test]
    fn materialize_flat_root_images_only() {
        let t = tree(&["b.jpg", "a.jpg", "c.jpg"]);
        let (items, metas) = t.materialize_level(&[], SortOrder::FileName);
        // 画像のみ・FileName 昇順。
        let names: Vec<&str> = items.iter().map(as_zipimage).collect();
        assert_eq!(names, vec!["a.jpg", "b.jpg", "c.jpg"]);
        assert_eq!(items.len(), metas.len());
        assert!(metas.iter().all(|m| m.is_some()));
    }

    #[test]
    fn materialize_multiple_books_containers_first() {
        let t = tree(&[
            "ch02.zip/p01.jpg",
            "ch01.zip/p02.jpg",
            "ch01.zip/p01.jpg",
            "loose.jpg",
        ]);
        let (items, metas) = t.materialize_level(&[], SortOrder::FileName);
        assert_eq!(items.len(), metas.len());
        // コンテナ (ZipDir) が先、画像が後。ZipDir は名前順 (ch01.zip, ch02.zip)。
        let (p0, a0, rep0) = as_zipdir(&items[0]);
        assert_eq!(p0, "ch01.zip/");
        assert!(a0);
        // 代表サムネ = sort 先頭ページ (p01.jpg)。
        assert_eq!(rep0, Some("ch01.zip/p01.jpg"));
        // ZipDir meta は代表画像の (mtime, size) を載せる (enqueue gate 用)。
        // テスト entry は mtime=0/size=0。
        assert_eq!(metas[0], Some((0, 0)));
        let (p1, _, rep1) = as_zipdir(&items[1]);
        assert_eq!(p1, "ch02.zip/");
        assert_eq!(rep1, Some("ch02.zip/p01.jpg"));
        // 最後に直下画像。
        assert_eq!(as_zipimage(&items[2]), "loose.jpg");
        assert!(metas[2].is_some());
    }

    #[test]
    fn materialize_mixed_dir_then_images() {
        let t = tree(&["sub/x.jpg", "a.jpg", "b.jpg"]);
        let (items, _) = t.materialize_level(&[], SortOrder::FileName);
        // sub (ZipDir, 非アーカイブ) が先頭。
        let (prefix, is_archive, _) = as_zipdir(&items[0]);
        assert_eq!(prefix, "sub/");
        assert!(!is_archive);
        assert_eq!(as_zipimage(&items[1]), "a.jpg");
        assert_eq!(as_zipimage(&items[2]), "b.jpg");
    }

    #[test]
    fn materialize_nested_prefix_builds_full_dir_prefix() {
        let t = tree(&["chapters/ch01.zip/p01.jpg", "chapters/ch02.zip/p01.jpg"]);
        let (items, _) = t.materialize_level(&s(&["chapters"]), SortOrder::FileName);
        let (p0, a0, rep0) = as_zipdir(&items[0]);
        assert_eq!(p0, "chapters/ch01.zip/");
        assert!(a0);
        assert_eq!(rep0, Some("chapters/ch01.zip/p01.jpg"));
        let (p1, _, _) = as_zipdir(&items[1]);
        assert_eq!(p1, "chapters/ch02.zip/");
    }

    #[test]
    fn materialize_representative_sort_aware() {
        // 列挙順は [b, a] だが FileName sort では代表は a.jpg。
        let t = tree(&["book/b.jpg", "book/a.jpg"]);
        let (items, _) = t.materialize_level(&[], SortOrder::FileName);
        let (_, _, rep) = as_zipdir(&items[0]);
        assert_eq!(rep, Some("book/a.jpg"));
    }

    #[test]
    fn materialize_representative_prefers_direct_image_over_subdir() {
        // 混在ノード (直下に cover.jpg + 子 ch01/): 代表は直下画像 (cover.jpg) を優先する。
        // 設計判断 (Codex P3): 表紙は通常ルート直下の loose ファイルなので、materialize の
        // 表示順 (ZipDir 先) とは別に「表紙 = 直下画像優先」を採る。entering 時の先頭セルは
        // ch01/ になるが、代表サムネは cover.jpg になる (意図的な非一致)。
        let t = tree(&["book/cover.jpg", "book/ch01/page01.jpg"]);
        let (items, _) = t.materialize_level(&[], SortOrder::FileName);
        let (prefix, _, rep) = as_zipdir(&items[0]);
        assert_eq!(prefix, "book/");
        assert_eq!(rep, Some("book/cover.jpg"));
    }

    #[test]
    fn materialize_representative_recurses_into_first_subdir() {
        // 直下画像なし → sort 先頭の子ディレクトリ (sub_a) の先頭画像。
        let t = tree(&["wrap/sub_b/y.jpg", "wrap/sub_a/x.jpg"]);
        let (items, _) = t.materialize_level(&[], SortOrder::FileName);
        // root 直下は "wrap" のみ。
        let (prefix, _, rep) = as_zipdir(&items[0]);
        assert_eq!(prefix, "wrap/");
        assert_eq!(rep, Some("wrap/sub_a/x.jpg"));
    }

    #[test]
    fn materialize_numeric_sort_natural_order() {
        let t = tree(&["p10.jpg", "p2.jpg", "p1.jpg"]);
        let (items, _) = t.materialize_level(&[], SortOrder::Numeric);
        let names: Vec<&str> = items.iter().map(as_zipimage).collect();
        assert_eq!(names, vec!["p1.jpg", "p2.jpg", "p10.jpg"]);
    }

    #[test]
    fn materialize_missing_prefix_is_empty() {
        let t = tree(&["a.jpg"]);
        let (items, metas) = t.materialize_level(&s(&["nope"]), SortOrder::FileName);
        assert!(items.is_empty());
        assert!(metas.is_empty());
    }

    #[test]
    fn materialize_meta_values_match_entry() {
        let t = ZipTree::build(
            PathBuf::from("C:/test/outer.zip"),
            vec![ZipImageEntry {
                entry_name: "a.jpg".to_string(),
                uncompressed_size: 1234,
                mtime: 99,
            }],
        );
        let (items, metas) = t.materialize_level(&[], SortOrder::FileName);
        assert_eq!(items.len(), 1);
        assert_eq!(metas[0], Some((99, 1234)));
    }

    // ── all_cache_keys ───────────────────────────────────────────────

    #[test]
    fn all_cache_keys_covers_every_level() {
        let t = tree(&[
            "cover.jpg",
            "ch01.zip/p01.jpg",
            "ch01.zip/p02.jpg",
            "extras/sub/x.jpg",
        ]);
        let mut keys = t.all_cache_keys();
        keys.sort();
        let mut expected = vec![
            // 全 leaf 画像の entry_name (そのまま)。
            "cover.jpg".to_string(),
            "ch01.zip/p01.jpg".to_string(),
            "ch01.zip/p02.jpg".to_string(),
            "extras/sub/x.jpg".to_string(),
            // 全ディレクトリの zipdir: キー。
            "zipdir:ch01.zip/".to_string(),
            "zipdir:extras/".to_string(),
            "zipdir:extras/sub/".to_string(),
        ];
        expected.sort();
        assert_eq!(keys, expected);
    }

    #[test]
    fn all_cache_keys_zipdir_format_matches_materialize() {
        // all_cache_keys の zipdir: キーが materialize_level の cache_key_override と一致する
        // ことを保証する (= delete_missing が消さない)。
        let t = tree(&["chapters/ch01.zip/p01.jpg"]);
        let keys = t.all_cache_keys();
        // materialize で chapters を開いたときの ZipDir cache key。
        let (items, _) = t.materialize_level(&s(&["chapters"]), SortOrder::FileName);
        let (dir_prefix, _, _) = as_zipdir(&items[0]);
        let mat_key = crate::grid_item::zipdir_cache_key(dir_prefix);
        assert!(keys.contains(&mat_key), "{mat_key} not in {keys:?}");
    }

    // ── ZipNavState (スタック方式ナビ) ───────────────────────────────

    fn nav(names: &[&str]) -> ZipNavState {
        ZipNavState::new(Arc::new(tree(names)))
    }

    #[test]
    fn nav_flat_root_is_empty_prefix() {
        // ルートに画像があれば collapse しない → current = [].
        let n = nav(&["a.jpg", "b.jpg"]);
        assert_eq!(n.current(), &[] as &[String]);
        assert!(n.at_root());
        // ルートで back は false (= ZIP を抜ける)。
        let mut n = n;
        assert!(!n.back());
    }

    #[test]
    fn nav_single_wrapper_collapses_at_open() {
        // ZIP > vol01 > [pages]: 開いた時点で vol01 まで自動降下。at_root のまま。
        let n = nav(&["vol01/p01.jpg", "vol01/p02.jpg"]);
        assert_eq!(n.current(), &s(&["vol01"])[..]);
        assert!(n.at_root());
    }

    #[test]
    fn nav_multi_book_enter_and_back_no_trap() {
        // ZIP > vol01 > {ch01.zip, ch02.zip}: 単一ラッパー vol01 を skip して
        // [ch01.zip, ch02.zip] を表示。
        let mut n = nav(&["vol01/ch01.zip/p01.jpg", "vol01/ch02.zip/p01.jpg"]);
        assert_eq!(n.current(), &s(&["vol01"])[..]);
        assert!(n.at_root());

        // ch01.zip へ降りる (dir_prefix は materialize 由来の絶対パス)。
        n.enter("vol01/ch01.zip/");
        assert_eq!(n.current(), &s(&["vol01", "ch01.zip"])[..]);
        assert!(!n.at_root());

        // back で [ch01,ch02] 表示へ戻る。
        assert!(n.back());
        assert_eq!(n.current(), &s(&["vol01"])[..]);
        assert!(n.at_root());

        // ルートで back → false (ZIP を抜ける)。罠 (同じ画面に戻り続ける) は起きない。
        assert!(!n.back());
    }

    #[test]
    fn nav_enter_collapses_nested_wrapper() {
        // ch01.zip の中がさらに単一ラッパー (only/) なら enter 時に collapse する。
        let mut n = nav(&["ch01.zip/only/p01.jpg", "ch02.zip/p01.jpg"]);
        // root は分岐 (ch01.zip, ch02.zip) なので collapse なし。
        assert_eq!(n.current(), &[] as &[String]);
        n.enter("ch01.zip/");
        // ch01.zip > only > [pages] の only まで降りる。
        assert_eq!(n.current(), &s(&["ch01.zip", "only"])[..]);
    }

    #[test]
    fn nav_dfs_root_descends_into_first_book() {
        // ルート (本一覧) で Ctrl+↓ → 最初の本へ降りる (旧仕様の「即 ZIP 脱出」を廃止)。
        let mut n = nav(&["chapterA.zip/p1.png", "chapterB.zip/p1.png"]);
        assert!(n.at_root());
        assert!(n.dfs_step(true, SortOrder::FileName));
        assert_eq!(n.current(), &s(&["chapterA.zip"])[..]);
        assert!(!n.at_root());
    }

    #[test]
    fn nav_dfs_forward_walks_siblings_then_exits() {
        // 葉の本どうしは兄弟移動。最後の本の後はツリーの端 = false (stack 不変)。
        let mut n = nav(&[
            "chapterA.zip/p1.png",
            "chapterB.zip/p1.png",
            "chapterC.zip/p1.png",
        ]);
        n.enter("chapterA.zip/");
        assert!(n.dfs_step(true, SortOrder::FileName)); // → B
        assert_eq!(n.current(), &s(&["chapterB.zip"])[..]);
        assert!(n.dfs_step(true, SortOrder::FileName)); // → C
        assert_eq!(n.current(), &s(&["chapterC.zip"])[..]);
        assert!(!n.dfs_step(true, SortOrder::FileName)); // 端 = ZIP を抜ける
        // 端で false のとき stack は不変 (遡行 pop が残らない)。
        assert_eq!(n.current(), &s(&["chapterC.zip"])[..]);
        assert!(!n.at_root());
        assert!(n.dfs_step(false, SortOrder::FileName)); // ← B
        assert_eq!(n.current(), &s(&["chapterB.zip"])[..]);
    }

    #[test]
    fn nav_dfs_forward_ascends_to_uncle() {
        // 深い本の最後から「祖先の次の兄弟」へ抜ける (実フォルダ DFS と同じ)。
        // 01_arc/{a.zip,b.zip}, 02_plain/p.png: b.zip の次は 02_plain。
        let mut n = nav(&[
            "01_arc/a.zip/p1.png",
            "01_arc/b.zip/p1.png",
            "02_plain/p1.png",
        ]);
        n.enter("01_arc/");
        n.enter("01_arc/b.zip/");
        assert_eq!(n.current(), &s(&["01_arc", "b.zip"])[..]);
        assert!(n.dfs_step(true, SortOrder::FileName));
        assert_eq!(n.current(), &s(&["02_plain"])[..]);
        // 遡って移った先の深さは 1 段 (root の子)。BS 1 回で root へ。
        assert!(n.back());
        assert!(n.at_root());
    }

    #[test]
    fn nav_dfs_flat_root_exits_both_ways() {
        // フラット ZIP (子なし): 前後どちらも端 = false で従来どおり ZIP を抜ける。
        let mut n = nav(&["a.jpg", "b.jpg"]);
        assert!(n.at_root());
        assert!(!n.dfs_step(true, SortOrder::FileName));
        assert!(!n.dfs_step(false, SortOrder::FileName));
        assert!(n.at_root());
    }

    #[test]
    fn nav_dfs_backward_first_child_goes_to_parent_then_exits() {
        // 最初の本で Ctrl+↑ → 親 (本一覧) へ。ルートでもう一度 → false (ZIP の前へ抜ける)。
        let mut n = nav(&["a.zip/p1.png", "b.zip/p1.png"]);
        n.enter("a.zip/");
        assert!(n.dfs_step(false, SortOrder::FileName));
        assert!(n.at_root());
        assert!(!n.dfs_step(false, SortOrder::FileName));
        assert!(n.at_root());
    }

    #[test]
    fn nav_dfs_backward_descends_to_last_descendant() {
        // 前の兄弟が子を持つ場合、その「最後の子孫」へ降り切る (逆前順)。
        // 01_arc/{a.zip,b.zip}, 02_plain/p.png: 02_plain の前は 01_arc/b.zip。
        let mut n = nav(&[
            "01_arc/a.zip/p1.png",
            "01_arc/b.zip/p1.png",
            "02_plain/p1.png",
        ]);
        n.enter("02_plain/");
        assert!(n.dfs_step(false, SortOrder::FileName));
        assert_eq!(n.current(), &s(&["01_arc", "b.zip"])[..]);
        // 降り切った先からの BS は 1 段ずつ (01_arc → root)。
        assert!(n.back());
        assert_eq!(n.current(), &s(&["01_arc"])[..]);
        assert!(n.back());
        assert!(n.at_root());
    }

    #[test]
    fn nav_dfs_collapses_wrapper_sibling() {
        // 移動先がさらに単一ラッパーなら自動降下する (enter と同じ collapse)。
        let mut n = nav(&["b1.zip/p1.png", "b2.zip/only/p1.png"]);
        n.enter("b1.zip/");
        assert!(n.dfs_step(true, SortOrder::FileName));
        // b2.zip > only > pages の only まで降りる。
        assert_eq!(n.current(), &s(&["b2.zip", "only"])[..]);
        // 逆方向も collapse 済みの 1 ノードとして扱われ b1.zip へ戻る。
        assert!(n.dfs_step(false, SortOrder::FileName));
        assert_eq!(n.current(), &s(&["b1.zip"])[..]);
    }

    #[test]
    fn nav_dfs_mixed_node_descends_before_sibling() {
        // 直下画像 + 子コンテナの混在ノード: forward は兄弟より先に子へ降りる (前順)。
        let mut n = nav(&["05_mixed/m_01.png", "05_mixed/extra.zip/p1.png", "06/p.png"]);
        n.enter("05_mixed/");
        assert!(n.dfs_step(true, SortOrder::FileName));
        assert_eq!(n.current(), &s(&["05_mixed", "extra.zip"])[..]);
        assert!(n.dfs_step(true, SortOrder::FileName));
        assert_eq!(n.current(), &s(&["06"])[..]);
    }

    #[test]
    fn nav_dfs_full_preorder_round_trip() {
        // 前順の全順序: forward で全ノードを 1 度ずつ辿って端で false、backward で
        // 逆順に root まで戻って false (対称性 = Ctrl+↓ → Ctrl+↑ で同じ場所に戻る)。
        let mut n = nav(&[
            "01/a.zip/p1.png",
            "01/b.zip/p1.png",
            "02/p1.png",
            "root.png",
        ]);
        let sort = SortOrder::FileName;
        let expected: Vec<Vec<String>> = vec![
            s(&["01"]),
            s(&["01", "a.zip"]),
            s(&["01", "b.zip"]),
            s(&["02"]),
        ];
        let mut visited: Vec<Vec<String>> = Vec::new();
        while n.dfs_step(true, sort) {
            visited.push(n.current().to_vec());
        }
        assert_eq!(visited, expected);
        let mut back_visited: Vec<Vec<String>> = Vec::new();
        while n.dfs_step(false, sort) {
            back_visited.push(n.current().to_vec());
        }
        // 逆順 (末尾の 02 から root まで)。root は current=[] で表現される。
        let mut expected_back: Vec<Vec<String>> = expected[..expected.len() - 1]
            .iter()
            .rev()
            .cloned()
            .collect();
        expected_back.push(Vec::new());
        assert_eq!(back_visited, expected_back);
        assert!(n.at_root());
    }

    #[test]
    fn nav_materialize_current_reflects_level() {
        let mut n = nav(&["ch01.zip/p01.jpg", "ch02.zip/p01.jpg"]);
        // root: 2 つの ZipDir。
        let (items, _) = n.materialize_current(SortOrder::FileName);
        assert_eq!(items.len(), 2);
        assert!(matches!(items[0], GridItem::ZipDir { .. }));
        // ch01.zip へ降りると画像 1 枚。
        n.enter("ch01.zip/");
        let (items, _) = n.materialize_current(SortOrder::FileName);
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], GridItem::ZipImage { .. }));
    }
}
