# ネスト ZIP ツリーナビゲーション設計計画 (v1.3.0)

**状態**: 設計確定 / 実装着手前 (2026-06-10)
**ブランチ**: `feat/nested-zip-tree`
**進め方**: Claude Code 実装 + 各フェーズ末で Codex CLI レビュー → 最終 Codex GUI レビュー (Approach 2)

このドキュメントは Claude と Codex が独立に到達した同一設計 (= 後述「Strategy A」) を
両者の作業の出発点として固定したもの。実装中に設計を変えたら本書も同時更新する。

関連: [virtual-folders.md](virtual-folders.md) (ZIP/PDF 分岐表) / [display-pipeline.md](display-pipeline.md) (見開き描画)。

---

## 1. 目的と背景

### 何を変えるか
ネスト ZIP (ZIP 内に別の ZIP / サブフォルダがある構造) を、現在の **フラット展開**
(全画像を 1 本の線形リストに平坦化) から、**ツリーをメモリに持ち、その内部を
「検索ドリルダウンのように」移動する** 方式に変える。

### なぜ変えるか (見開きとの相性問題)
見開きペアリングは 2 経路とも **平坦な並び順の偶奇 (`pos % 2`) だけ** でページを組む:
- ページ送り: [`resolve_spread_pair`](../src/ui_fullscreen.rs) (`get_nav_indices` ベース)
- 連続読み: [`continuous_reading_units_and_pos`](../src/ui_fullscreen.rs)
  (`build_image_reading_indices` は ZipSeparator を捨てる)

このため複数の本 (内側 ZIP / サブフォルダ) を 1 つの ZIP に入れると:
1. 本 A のページ数が奇数だと、本 B の 1 ページ目が本 A の最終ページと対向ページになる
2. 「表紙は単独表示」が ZIP 全体の先頭 1 ページにしか効かない
3. 見開きモード (右綴じ/左綴じ/表紙有無) が外側 ZIP 単位で本ごとに変えられない
4. 読書再開・連続読みが ZIP 全体で 1 本扱い

根本原因: **ネスト ZIP は概念的に複数の本なのに、フラット展開が 1 冊の巨大な本にする**。

### フラット採用の経緯 (調査結果)
**機能上の障害が原因ではない。** 最初の ZIP 対応 (commit `ad4e4b4f`) の時点で
「サブフォルダはフラット + セパレータ」が出発点で、ネスト ZIP (commit `aac30139`, v0.7.0)
はそれを内側 ZIP に延長した簡易拡張 ([plan-v0.7.0.md](plan-v0.7.0.md) §4、見積 1〜2 日)。
ツリー実装を試して撤退した形跡はない。

関連する過去の実害 (実装時に踏まないよう注意):
- `d1a6e99f`: ネスト ZIP 約 1100 ファイルの列挙が **UI スレッドを 2.3 秒ブロック** →
  非同期列挙に移行。**ツリー構築も非同期 (UI スレッド外) で行う制約は維持する。**
- `d136e055` (CBZ 境界) / `531527f8` (separator/fullscreen): 文字列パス境界と
  ZipSeparator 特別扱いに由来する修正が続いている。

---

## 2. 設計方針 (Strategy A) — 表示層のみ追加・DB 移行ゼロ

### 中核原則: `entry_name` を一切変えない
ページの永続データ **7 系統すべてが `entry_name` (フル文字列 "outer/ch01.zip/page01.jpg")
をキーに埋め込む**:
回転 / 補正 / レーティング / 消しゴム / 隠蔽 / ローカル調整 / タグ / サイドカー /
サムネカタログ / 検索索引。集約点は [`adjustment_db::zip_entry_key`](../src/adjustment_db.rs)、
分岐は [`page_path_key`](../src/app.rs) / [`sidecar_relative_key`](../src/app.rs)。

→ **`entry_name` (= ページの正体) を変えず、「どのページを今表示するか」という
表示・ナビゲーション層だけを足す。** これにより:
- DB マイグレーション **完全不要** (既存の補正・評価・回転・サムネが全部生存)
- 見開きペアリングのロジック改変 **不要** (items が現在の本だけになれば自動で直る)

### ユーザーの構想との一致
「ZIP を開くとメモリ上にファイル一覧をツリー状にもって、検索結果のようにその内部で
移動できる」= 既存の **Ctrl+G 検索ドリルダウン** ([`drill_into_container`](../src/global_search_ui.rs) /
`drill_into_subfolder` / `drill_back_one_level`) と同型。これを ZIP に転用する。

---

## 3. データモデル

### 3.1 メモリ上のツリー (新規)
列挙結果 (`Vec<ZipImageEntry>`, entry_name は `/` 区切り) からトライ木を構築する。
`.zip/` `.cbz/` 境界は entry_name 中で既に `/` 区切りなので、**entry_name を `/` で split
するだけ** でツリーになる (`.zip`/`.cbz` で終わるセグメント = 内側アーカイブ。構造上は
ただのディレクトリ階層。バイト読み出しの `.zip/` 特別扱いは既存 `read_entry_bytes` が担当)。

```
ZipTree {
    zip_path: PathBuf,
    root: ZipTreeNode,
}
ZipTreeNode {
    // セグメント名 (entry_name の元の文字列をそのまま保持。case 等を改変しない)
    dirs: BTreeMap<String, ZipTreeNode>,   // 子ディレクトリ / 内側アーカイブ
    images: Vec<ZipImageEntry>,             // この階層直下の画像 (entry_name はフル)
}
```
- 純データ・I/O なし → ユニットテスト容易。
- leaf の `entry_name` は **元の列挙文字列を完全保持** (DB キー・read_entry_bytes 互換)。

### 3.2 GridItem 新バリアント (D2: 後述で要確認)
```
GridItem::ZipDir {
    zip_path: PathBuf,
    dir_prefix: String,   // "chapters/" や "chapters/ch01.zip/" (末尾 '/')
    is_archive: bool,     // セグメントが .zip/.cbz か (バッジ区別用)
}
```
- 「入れるディレクトリ / 内側アーカイブ」を表す 1 セル。Enter/ダブルクリックで階層を降りる。
- 画像 leaf は **既存の `ZipImage { zip_path, entry_name }` のまま** (identity 不変)。
- 代表サムネ = 部分木の先頭画像 (in-memory ツリーから選定、bytes は worker が遅延読み)。

### 3.3 ナビゲーション状態 (Phase 3a 実装、Ctrl+G DrillState 相当)
```
ZipNavState {
    tree: Arc<ZipTree>,            // 開いている外側 ZIP のツリー (列挙完了時に構築)
    stack: Vec<Vec<String>>,       // 「描画した実効 prefix」のスタック。stack[0]=collapse([])、常に非空
}
```
- **実効 prefix スタック方式** (Phase 1 当初の「単一 logical prefix + 都度 collapse」案から変更)。
  各エントリは collapse 済みの実効 prefix。`enter(dir_prefix)` は子の実効 prefix を push、
  `back()` は pop (底=ルートなら false で ZIP を抜ける)。これで「root が単一ラッパーへ
  collapse する木で Backspace が同じ画面に戻り続ける」罠を構造的に回避する (Codex P2)。
  詳細は `collapse_redundant` の doc-comment 参照。
- 別フォルダ / 別 ZIP / PDF へ移ると破棄。クリアの単一チョークポイントは
  `start_loading_items` 先頭 + leaving 経路 (`load_folder` / `load_zip` / `load_pdf` の
  `clear_nested_cache` 隣 / `enter_drive_list` / `replace_search_view_items`)。
  `finalize_zip_enumerate` が `start_loading_items` の **後**で再設定する。

---

## 4. 列挙とツリー構築 (非同期)

- 既存 [`load_zip_as_folder`](../src/app.rs) → 非同期 `enumerate_image_entries` の経路は維持。
- [`finalize_zip_enumerate`](../src/app.rs) で、**現在: 単一フラットリスト生成** を
  **変更: (a) `ZipTree` 構築 → (b) ルート階層を materialize** に分離する。
- **ツリー構築は列挙完了後のワーカー側 or 受信直後の軽量変換**で行い、UI スレッドを
  ブロックしない (`d1a6e99f` の教訓)。1100 エントリ規模でも split+trie は数 ms。

---

## 5. 階層 materialize (items 生成)

`materialize_level(tree, prefix, sort) -> (items, image_metas)`:
1. prefix を辿って当該 `ZipTreeNode` を得る。
2. 子ディレクトリ → `GridItem::ZipDir` セル (sort_order でソート、コンテナ先頭慣習)。
3. 直下画像 → `GridItem::ZipImage` セル (sort_order でソート)。
4. グリッド慣習に合わせ **コンテナ (ZipDir) を先・画像を後**。

navigation でこの関数を呼ぶだけ (再列挙なし = in-memory)。見開きは items が
現在階層の画像だけになるため **ペアリングが本ごとに自動リセット** される。

### D1 (要確認): 冗長ラッパー階層の自動降下
「ZIP > 単一フォルダ > ページ群」のような、**子ディレクトリ 1 個・直下画像 0 枚**の
階層は自動で降りる (redundant wrapper collapse)。これで単一の本の余計な 1 段を
スキップしつつ、真の複数本アーカイブだけツリーになる。**推奨デフォルト = 有効。**

---

## 6. ナビゲーション (Ctrl+G ドリル流用)

| 操作 | 挙動 |
| --- | --- |
| ZipDir を Enter/ダブルクリック | `prefix.push(seg)` → `materialize_level` で再表示 |
| Backspace | `prefix.pop()`。空なら ZIP を出て実フォルダ親へ ([`effective_folder`](../src/app.rs)`.parent()`) |
| アドレスバー/パンくず | `vol.zip > chapters > ch01.zip` (Ctrl+G drill breadcrumb 形式) |
| Ctrl+↑↓ | (要検討 D3) 現階層内のサムネ移動 or 本またぎ |

- 状態リセット境界: 同一 ZIP 内のナビでは `ZipNavState.tree` を保持し再列挙しない。
  別 ZIP/フォルダへ出るときのみ `clear_nested_cache` + tree 破棄。

### ⚠ 実効 prefix スタック方式で Backspace 罠を回避 (Phase 1 Codex P2 → Phase 3a 実装で確定)
当初案は「`ZipNavState.prefix` に論理 prefix を持ち、collapse は materialize 直前の
view 変換としてのみ適用 (結果を保存し直さない)」だったが、**実装では実効 prefix の
スタック方式**を採った (同じ罠回避をより単純に達成できるため)。

- `stack` の各エントリは「実際に描画した実効 prefix」(= collapse 済み)。
- `enter(dir_prefix)`: 子の実効 prefix を `collapse_redundant` で算出して push。
- `back()`: pop。底 (= `stack.len()==1`、ルート表示) なら `false` を返し、呼び出し側が
  ZIP を抜けて実フォルダ親へ遷移する。
- これで `[] → pop → [] → collapse → ["vol01"]` のように同じ画面へ戻り続ける罠が
  構造的に起きない (各 back は「直前に描画した実効 prefix」へ正確に戻る)。
- root が単一ラッパー (`vol01/`) へ collapse する木では、開いた時点で `stack[0]=["vol01"]`
  となり `at_root()`。ここで Backspace すると即 ZIP を抜ける (= ラッパーは「入った」扱いに
  しない)。

---

## 7. 見開き (改修不要で直る理由)

`get_nav_indices` / `build_image_reading_indices` は `self.items` + `self.visible_indices`
から導出される。tree ナビで `self.items` が **現在の本のページだけ** になるため、
`pos % 2` のペアリングと「表紙単独 (`pair_start`)」が **その本の先頭から数え直される**。
→ 本またぎペア崩れ・表紙単独の誤適用が自然に解消。**ペアリングコードは原則無改修。**

---

## 8. フルスクリーン本またぎ (D3: 要確認 / 最難所)

- 現階層内: 見開き・連続読みはその階層の画像で完結。
- 本を読み終えて次の本へ: **既存のコンテナ間 Ctrl+↑↓ ナビ** に乗せる
  ([virtual-folders.md §2.3](virtual-folders.md) の連続読書経路)。
- 自動フルスクリーン (モード B) で複数本 ZIP を開いたときの「先頭ページ」定義を確定する
  (推奨: 最初の本の 1 ページ目に着地)。
- 境界ヒント (`fs_boundary_hint`)・退出ルーティングとの整合を確認。

---

## 9. サムネイル

- `ZipImage` のサムネキー = `entry_name` 直 → **不変** (既存キャッシュ命中)。
- `ZipDir` の代表サムネ = 新キー `zipdir:{dir_prefix}` (zip の catalog 配下、**additive**)。
  選定は in-memory tree の部分木先頭画像 (sort 準拠)。worker が bytes 遅延読み。
  - Phase 1 の `ZipTreeNode::first_image_in_subtree` は **sort 非対応の fallback**
    (直下画像=列挙順先頭、子=BTreeMap キー順)。Phase 2b の `representative_image` は
    表示 `SortOrder` 準拠の先頭画像を選ぶ (Phase 1 Codex P3 対応)。
  - **代表選定は「直下画像優先」** (混在ノードでは ZipDir 表示順より前に直下画像を採る)。
    表紙はルート直下の loose ファイルが普通なので、materialize の表示順 (ZipDir 先) とは
    意図的に非一致 (Phase 2 Codex P3、`materialize_representative_prefers_direct_image_over_subdir`)。
  - **Phase 2c thumbnail 配線 (Phase 2 Codex P2 対応)**: `materialize_level` は ZipDir の
    `image_metas` に代表画像の `(mtime, size)` を載せる (None だと enqueue 経路が要求を
    出さずセルが永久 Pending → 毎フレーム repaint)。`make_load_request` の ZipDir 分岐は
    `path=zip_path` / `zip_entry=Some(representative)` / `cache_key_override=zipdir_cache_key`。
    `zipdir:` prefix は folder/zip thumb prefix と一致しないので、worker はコンテナ列挙では
    なく「zip_entry を直接読む」経路に乗り、代表バイトを zipdir: キーで保存する。
  - **Phase 3 で existing_keys を全ツリーから構築する**: フラット実装は finalize が全
    entry_name を `existing_keys` に入れて `delete_missing` のプルーン基準にしていた。
    ツリーでは finalize がルート階層しか materialize しないため、`existing_keys` は
    **ツリー全体を walk して全 leaf `entry_name` + 全 dir の `zipdir:{prefix}`** を集める
    こと。さもないと深い階層の thumbnail が再オープン時に stale 削除される (Phase 2 Codex P2)。
- **`is_archive` バッジは suffix 推定 (accepted ambiguity)**: 葉の `entry_name` 文字列
  だけからは「実在するネスト .zip アーカイブ」と「`.zip` という名前のただのフォルダ」
  を区別できない (`enumerate_recursive` が両者を同じ `"foo.zip/img.jpg"` 文字列に
  畳むため。フラット実装からの継承)。Phase 2 の `ZipDir.is_archive` はセグメント
  suffix (`.zip`/`.cbz`) で判定し、同一 stem 衝突は許容する。バッジは表示のみで
  バイト読み出しは既存 `read_entry_bytes` の `.zip/` 境界判定に委ねる。Phase 2 で
  suffix 判定のテストを 1 本足す (Phase 1 Codex P3)。

---

## 10. 影響範囲 (修正漏れ注意)

`GridItem` への新バリアント `ZipDir` 追加は src 全体に波及する (`GridItem::` 参照 **905 箇所 /
21 ファイル**)。多くは `_ =>` 吸収だが、以下の load-bearing 箇所は **明示処理が必須**:

- [grid_item.rs](../src/grid_item.rs): `name` / `display_path` / `perf_key` / `is_checkable` /
  `container_path` / `file_operation_path` / `drag_source_path` / `accepts_rating` 等
- [ui_main.rs](../src/ui_main.rs): セル描画 (`draw_cell`)・クリック/ダブルクリック/Enter・選択
- [ui_fullscreen.rs](../src/ui_fullscreen.rs): nav index 構築・ZipDir はフルスクリーン対象外扱い
- ナビ: Ctrl+↑↓ DFS・Backspace・アドレスバー・スライドショー (ZipDir スキップ)
- [snapshot.rs](../src/snapshot.rs): ナビ状態の復元 (entry_name 不変なので leaf は互換)
- [folder_thumb_pins.rs](../src/folder_thumb_pins.rs): ZipDir をピン対象にするか (初版は対象外可)
- [context_menu.rs](../src/ui_dialogs/context_menu.rs) / gamepad / undo / search drill

---

## 11. 要確認の設計判断 (推奨デフォルトで進行)

| ID | 判断 | 推奨デフォルト | 代替 |
| --- | --- | --- | --- |
| D1 | サブフォルダも一様にツリー化するか | **する** + 冗長ラッパー自動降下 | 内側 ZIP 境界のみツリー化 |
| D2 | 新 `GridItem::ZipDir` 追加 vs `ZipSeparator` 役割拡張 | **新バリアント** | セパレータ二役 |
| D3 | 本またぎ連続読み | 現階層完結 + Ctrl+↑↓ で次本 | 全本を 1 ストリーム維持 |
| D4 | 読書再開/見開きモードの粒度 | **当面 ZIP 単位** (per-book は将来) | per-(zip+prefix) 拡張 |

これらは設計ドキュメント上の可逆判断。Codex / ユーザーレビューで覆して良い。

---

## 12. フェーズ分割と工数 (実働 5〜9 日)

| Ph | 内容 | 目安 | Codex CLI レビュー |
| --- | --- | --- | --- |
| 0 | 本ドキュメント (設計固定) | 済 | 設計レビュー (GUI 既出) |
| 1 | `ZipTree` モデル + 構築 (純ロジック + ユニットテスト) | 0.5〜1d | ✓ |
| 2 | `GridItem::ZipDir` + `materialize_level` + 描画/サムネ | 1.5〜2.5d | ✓ |
| 3 | ナビ状態 + enter/back/parent + breadcrumb | 1〜1.5d | ✓ |
| 4 | フルスクリーン本またぎ + 見開き検証 + モード B | 1〜2d | ✓ (最難所) |
| 5 | テスト + docs + README 更新履歴 (v1.3.0) + glyph/snapshot lint | 1d | ✓ |

各フェーズ末: `cargo build` + `cargo test` + `cargo fmt` を通し、増分コミット。
Codex CLI は `codex exec` (1 回目) → `codex exec resume --last` (以降) で同一セッション継続。

---

## 13. 実機確認チェックリスト (ユーザー復帰時)

**テスト ZIP + 詳細チェックリストは [nested-zip-test-guide.md](nested-zip-test-guide.md)。**
`python scripts/make_nested_zip_test.py [--big]` で `dist/ziptest/` に自己説明的な
テスト ZIP を生成する (各頁画像に本名・頁番号・構造説明入り)。以下は要点の要約:

Claude はコンパイル + ユニットテストまでしか検証できない。以下は実機で要確認:
- [ ] 内側 ZIP / サブフォルダが「入れるセル」として表示され、Enter で降りられる
- [ ] Backspace で 1 段ずつ戻り、ルートで実フォルダ親へ抜ける
- [ ] 各本の中で見開きが正しく組まれる (本またぎペア崩れが消えた)
- [ ] 各本の表紙が単独表示になる (cover が本ごとに効く)
- [ ] 右綴じ/左綴じ・連続読み (縦/横) が各本で正しい
- [ ] ZipDir の代表サムネが出る / 既存ページの補正・評価・回転・タグが生存
- [ ] アドレスバーのパンくず表示が正しい
- [ ] 自動フルスクリーン (モード B) で複数本 ZIP を開いた着地が妥当
- [ ] CBZ / 深いネスト (2 段以上) / 大規模 (1000+ エントリ) で UI が固まらない
- [ ] 既存の単純 ZIP (ルート直下に画像のみ) が従来どおり動く (退行なし)

---

## 14. リスクと非目標

**リスク (中)**: `GridItem::ZipDir` の波及 (905 箇所) とフルスクリーン本またぎに集中。
永続データ破壊リスクは `entry_name` 不変のため **ゼロ**。

**実機 1 巡目フィードバックで追加実装済み (v1.3.0)**:
- 見開きモード永続化は **本 (zip+階層) ごと独立** に実装 (#1。当初 D4 将来予定を前倒し)。
- アドレス欄パンくず「元ZIPパス > ZIP内パス」(#5)。
- ソート変更 / 内側ファイルのピンで ZIP ルートに飛ぶバグを修正 (#2/#3、階層維持)。
- Ctrl+↑↓ で本またぎ移動 (グリッド/フルスクリーン両方、#4)。
- ZipDir 代表サムネの手動ピン = **本ごとピン (Model B)** (#3b。当初将来予定を前倒し)。

**非目標 (v1.3.0 では扱わない)**:
- per-book の読書再開位置 (cross-book resume、将来)
- 書き込み系 (ZIP 内ファイルの編集/移動) — 従来どおり read-only

**既知の制限 (将来)**:
- **複数本 ZIP の auto-fullscreen 着地 / cross-book resume**: ルートが ZipDir のみのとき
  自動フルスクリーンは画像を見つけられず本一覧表示に留まる。「続きから」も保存頁が
  ルート階層に無いと先頭フォールバック。先頭本への自動進入 / 深い階層への resume は将来。
- **ドリル中の Ctrl+G** は ZIP ルートに戻る (状態ロスト、データ破壊なし。ソート変更は修正済み)。
- **単一ラッパーで自動降下した本**の代表ピンは set/lookup キーがずれて反映されないことが
  ある (通常の本では一致)。ピンした本のサムネは ZIP 再オープン時に 1 枚再生成 (実害なし)。
