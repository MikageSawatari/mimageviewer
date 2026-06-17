# 詳細表示モード ＋ スマートフィルタ 設計プラン

> **ステータス: Ph1〜Ph4 + Ph5 画像/動画/作成日時遅延列まで実装済み (2026-06-09)**。`GridViewMode`、列セクション内の詳細切替、
> 固定列の詳細一覧骨格、右クリック列表示メニュー、`DetailsSortKey`、`details_order`、列ヘッダ 3 トグルソート、
> 詳細表示中のキーボード移動 / Shift 範囲選択の表示順対応、軽量 `FacetFilter`
> (種類/拡張子/★/タグ/日付/サイズ/状態/AI モデル/生成ツール) とチップ式フィルタバー、遅延列 worker、
> 進捗表示、作成日時列、画像解像度列、動画長さ/動画解像度/動画コーデック列
> (Ready までソート無効)、詳細ソート順を反映したフルスクリーン前後移動 / Home/End /
> 先読み / 連結読み / スライドショー、列幅ドラッグ変更、列ヘッダドラッグによる列順入れ替え、
> 左端プレビュー列のホバーサムネイル、日時の秒表示オプションは実装済み。
> AI モデル/生成ツール facet は v1.7.0 で session-local の遅延メタデータ facet として追加済み。
> EXIF 撮影日時/カメラ、PDF ページ数、
> アーカイブ内枚数の列は後続フェーズで同じ遅延列基盤へ追加する。
> 実装フェーズごとに `spec.md` / `htdocs/mimageviewer/` / 本書を
> 同時更新する (CLAUDE.md「コード修正時のドキュメント同時更新」)。

ファイル選択画面 (グリッド) に、通常サムネイルを並べず Explorer の「詳細表示」のように
**ファイル名・サイズ・更新日時＋mIV 独自項目**を行で並べて選択できるモードを追加する。
あわせて画面上部に **Excel のオートフィルタ風のスマートフィルタ** (種別・タグ・★・日付・
サイズ・編集状態での絞り込み) を追加する。

---

## 1. 背景・目的

- サムネイル一覧は内容把握に強いが、**大量ファイルの整理・検索・名前/日付での突き合わせ**には
  Explorer 詳細表示のほうが速い場面がある。
- mIV は Explorer に無い情報 (★レーティング / タグ / 補正・マスク・隠蔽・注釈の有無) を
  持っている。これを**一覧の列**と**絞り込み条件**の両方に出すと、
  「補正済みだけ」「未整理 (タグ★無し) だけ」「特定タグだけ」を一覧で潰せる、
  他ビューアに無い価値になる。
- 単純なソートだけでなく、**面 (facet) ベースの絞り込み**を上部に常設したい
  (ユーザー要望 2026-06-09)。

---

## 2. 用語・スコープ

| 用語 | 意味 |
| --- | --- |
| **サムネモード** | 既存のサムネイルグリッド表示 (`GridViewMode::Thumbnail`) |
| **詳細モード** | 本書で追加する行ベースのテキスト一覧 (`GridViewMode::Details`) |
| **FacetFilter** | 本書で追加する面ベースの絞り込み状態 (種別/タグ/★/日付/サイズ/編集フラグ…) |
| **facet (面)** | 1 つの絞り込み軸。Excel オートフィルタの「列ごとのドロップダウン」に相当 |
| **第一弾 / 第二弾** | 第一弾 = 追加 I/O 不要 (在メモリ判定)。第二弾 = stat/decode/probe が要る遅延項目 |

スコープに**含む**: 表示モード切替・詳細モードの行描画・列ソート・FacetFilter・
サムネ/詳細両モードでのフィルタ適用・アクティブフィルタの可視化。

スコープに**含まない (将来)**: グループ化表示・列ピン留め・フィルタプリセットの保存名管理・
Explorer 互換のカラム並びインポート。

---

## 3. 現状の関連実装 (コード接地)

新規実装ではなく**既存の合流点に乗せる**のが本設計の肝。関連箇所:

- **アイテム配列**: `App.items: Vec<GridItem>` / 種別は `GridItem` バリアント
  ([grid_item.rs](../src/grid_item.rs))。
- **メタ配列**: `App.image_metas: Vec<Option<(i64,i64)>>` = `(mtime_secs, file_size)`
  ([app.rs](../src/app.rs) のフィールド定義付近、ロード時確定は `load_folder_with_scan`)。
  - **フォルダ・通常フォルダ内の動画も値を持つ**。動画の値は詳細表示用で、
    `make_load_request` は `Video` をサムネ要求対象にしない。
  - Ctrl+S 検索結果の動画は検索 DB に size が無いため、mtime のみを持つ。
- **可視集合の再計算**: `App::rebuild_visible_indices`
  ([app.rs](../src/app.rs))。現状 **`search_filter`(Ctrl+F/G の `HashSet<usize>`) AND
  `rating_filter`(`[bool;6]`)** を合成して `visible_indices` を作る。**ここが拡張点**。
- **★フィルタ**: `settings.rating_filter: [bool;6]` ([settings.rs](../src/settings.rs))。
  ツールバーの `draw_rating_filter_button` / 一時解除 `rating_filter_suppressed_at` /
  `effective_rating_filter()` と密結合。
- **`visible_indices` は昇順前提**: `idx_visible` の `binary_search`
  ([app.rs](../src/app.rs))、`checked.retain`、先読み距離計算が昇順を仮定 (§7 の落とし穴)。
- **列数ツールバー**: `show_cols` セクション ([ui_main.rs](../src/ui_main.rs) の
  ツールバー描画)。`ToolbarSectionDisplay { Buttons, Dropdown }`
  ([settings.rs](../src/settings.rs)) の切替に対応。
- **グリッド描画ループ**: `compute_cell_size` → 仮想スクロール (`first_row`/`last_row`/
  `scroll_offset_y`) → `crate::app::draw_cell` ([ui_main.rs](../src/ui_main.rs))。
- **ソート種別**: `SortOrder { FileName, Numeric, DateAsc, DateDesc }`
  ([settings.rs](../src/settings.rs))。**サイズ順は未定義**。
- **検索 3 モード**: Ctrl+S / Ctrl+F / Ctrl+G ([search-architecture.md](search-architecture.md))。
  Ctrl+F = 現在地フィルタ (`search_filter` を生成)。

### 3.1 データ可用性 (列・facet 設計の前提)

**追加 I/O 不要 (第一弾に入れてよい)**:

| 項目 | 取得元 |
| --- | --- |
| 名前 | `item.name()` |
| 種別 | `GridItem` バリアント |
| 拡張子 | パス |
| 更新日時 | `image_metas[idx]` (フォルダ可・**動画 None**) |
| サイズ | `image_metas[idx]` (同上。動画は `video_items`) |
| ★レーティング | `get_rating(idx)` |
| タグ | `cell_tag_list(idx)` |
| 補正/ローカル補正/マスク/隠蔽/注釈/回転 | `adjustment_page_params` / `adjusted_page_keys` / `local_adjust_page_keys` / `mask_page_keys` / `conceal_page_keys` / `comic_page_keys` / `rotation_page_keys` |
| 📌ピン | `folder_pin_map` |

**追加 I/O 必要 (第二弾。遅延ロード前提)**:

| 項目 | 取得元 | コスト |
| --- | --- | --- |
| 解像度 W×H | カタログ `source_dims` / `ThumbnailState::Loaded.source_dims` | カタログ参照 or デコード |
| 動画の長さ・解像度・コーデック | video_thumb / ffmpeg probe | worker 必須 |
| 撮影日時・カメラ (EXIF) | `exif_reader` | ファイル読み |
| PDF ページ数 | カタログ `get_pdf_meta` | DB/IPC |

> **UI 応答性の鉄則** ([ui-responsiveness.md](ui-responsiveness.md) §4): 第二弾は UI スレッドから
> 同期で stat/decode/read_dir/probe しない。サムネと同じく背景 worker で埋め、未取得行は
> 「—」「…」表示にする。遅延列のフィルタ/ソートは、その列が `Ready` になるまで無効化する
> (§6.6 / §16.6)。

---

## 4. 表示モード切替

- 設定に `grid_view_mode: GridViewMode { Thumbnail, Details }` を追加 (既定 `Thumbnail`)。
- ツールバー先頭、`列:` セクションの**直前**に `selectable_label` 2択
  `[ サムネ | 詳細 ]` を置く。
  - グリフ方針 (CLAUDE.md「UI 文字列の Unicode グリフ選定ルール」): 環境依存の絵文字/記号を
    新規 UI に採用しない。**テキストラベルで実装**する。
- **詳細モード時は `show_cols` (列数) と `比率` セクションを自動非表示**にする
  (どちらも詳細モードでは無意味)。空いたスペースに「表示する列」設定ボタンを出してもよい。
- `toolbar_settings.rs` (ツールバーカスタマイズ) にモード切替セクションの表示/Buttons-Dropdown
  項目を 1 つ追加 (既存 `toolbar_*_display` に倣う)。

---

## 5. 詳細モードの列

**第一弾 (I/O 不要・既定 ON)**:

1. プレビュー (画像アイコン。ホバー時だけ対象サムネイルを外部ツールチップで表示)
2. 名前
3. ★ (レーティング)
4. タグ (`#a #b …` 省略表示)
5. 種別 / 拡張子
6. サイズ
7. 更新日時
8. 編集フラグ (補正/ローカル補正/マスク/隠蔽/注釈/回転 を小アイコン束で 1 列)

**第二弾 (I/O 必要・既定 OFF・列設定でオプトイン)**:

9. 解像度 (W×H, MP)
10. 動画長さ
11. 撮影日時 (EXIF)
12. PDF ページ数

> Explorer 互換列 (2,5,6,7) ＋ **mIV 独自列 (3 ★ / 4 タグ / 8 編集フラグ)** の構成。
> 独自列が「ただの Explorer 詳細表示」との差別化点。特に 8 は
> 「この画像 補正済みだっけ / マスク入れたっけ」を一覧で潰せる。

列操作:

- ヘッダクリックでソート (昇順/降順トグル、▲▼ 表示) — §7。
- ヘッダ境界ドラッグで列幅変更。
- 列設定 (表示/非表示・順序) を `details_columns` に永続化 (§9)。

### 5.1 描画は既存グリッド機構の流用

詳細モードは**新しい描画パスを足すのではなく `cols=1`・行高固定にして既存ループを流用**する:

- `compute_cell_size` を迂回し、行高 `row_h` (≈ 22–28px) を固定、`cols=1`。
- 仮想スクロール (`first_row`/`last_row`/`scroll_offset_y`/`total_rows`) は**そのまま動く**
  (1 アイテム 1 行)。
- セル内描画だけ `draw_cell` の代わりに「列レイアウトのテキスト行」レンダラを呼ぶ。
- **サムネをデコード/GPU アップロードしないぶん、サムネモードより軽い** (テクスチャ生成ゼロ)。
- キーボードナビ: `cols=1` なので ↑↓ が自然に効く。←→ は無効 or 列フォーカス用に予約。

ヘッダ行 (列見出し＋▼) は `ScrollArea` の**外**に固定する (CLAUDE.md「パネル」節:
スクロールバーがヘッダに重なる退行を避ける)。

---

## 6. スマートフィルタ (FacetFilter)

### 6.1 核心: 表示モードから独立した共有 FilterState を 1 つの合流点へ

`rebuild_visible_indices` の合成に `facet_filter` を **AND で 1 本足す**だけにする:

```text
visible = (0..items.len()).filter(|i|
       search_filter.map_or(true, |s| s.contains(i))   // Ctrl+F / Ctrl+G (既存)
    && passes_rating_filter(...)                        // ★ (既存・facet へ吸収可)
    && facet_filter.matches(self, i)                    // ★今回追加
)
```

`facet_filter.matches` は **§3.1 の在メモリ項目だけ**を見るので **worker 不要・同期で安全**。
これがこの機能を現実的にしている最大ポイント。

```rust
struct FacetFilter {
    kinds: Option<HashSet<ItemKind>>,        // Folder/Image/Video/Zip/Pdf...
    exts:  Option<HashSet<String>>,          // jpg/png/mp4...
    tags:  (HashSet<String>, TagMode),       // 選択タグ + AND/OR (+「タグ無し」特別項目)
    rating: [bool; 6],                       // 既存 rating_filter を吸収
    mtime_range: Option<(i64, i64)>,         // 日付レンジ / プリセット
    size_range:  Option<(u64, u64)>,
    edits: EditFlags,                        // 補正/マスク/隠蔽/注釈/回転/タグ有/★有
    // 第二弾: dim_range / duration_range (遅延データが揃った行だけ判定 — §6.6)
}
```

### 6.2 2 つの面・1 つの状態

- **詳細モード**: 各列ヘッダのフィルタボタン → ポップアップに「ソート (昇順/降順)」＋
  「その列の facet」。facet は **その列自身の条件だけを外し、他のフィルタを適用した集合**から
  distinct 値を集める。これにより「種別=jpg」に絞った後でも、種別メニューから png を
  再追加できる。Date/Size はレンジ or プリセット (今日/7日/30日/今年/カスタム)。
- **サムネモード**: 列ヘッダが無いので、ツールバー下に**フィルタバー (チップ式)**
  `[種別▼][タグ▼][★▼][日付▼][サイズ▼][編集▼]`。中身は詳細モードの列ドロップダウンと
  **同じ FacetFilter を編集**。

→ これで「詳細モードの Excel 風フィルタ」と「サムネモードのフィルタ」が**実装も挙動も一致**し、
二重メンテにならない。

### 6.3 タグ facet (要望の核)

- 現在表示中の全 `cell_tag_list` から **distinct タグ＋出現数**を集計 (在メモリ、軽い)。
- チェックで選択、**AND/OR トグル** (既存検索の OR トグルと同じ語彙)。
- 「タグ無し」特別項目 (未整理画像の洗い出し用)。

### 6.4 種別/拡張子/編集/AI facet

- 種別・拡張子: distinct 値チェックボックス。
- 編集: `補正 / ローカル補正 / マスク / 隠蔽 / 注釈 / 回転 / タグあり / ★あり` の有無。
  「補正済みだけ」「未整理 (タグ★無し) だけ」の一覧化が即できる。
- 補正系状態の親ロールアップ: `補` / `レ` / `消` / `隠` / `文` / `回` は、画像ページ自身だけでなく、
  そのページを含む ZIP / PDF / 変換アーカイブ / フォルダも対象にする。判定は各編集 DB の
  page_path キー (`adjustment.db` / `local_adjust.db` / `mask.db` / `conceal.db` / `comic.db` /
  `rotation.db`) を exact/prefix 判定する。フォルダは既定では直下の画像 / 直下の書庫だけを対象にし、
  `子フォルダも対象` ON 時だけ子孫フォルダ配下も対象にする。ファイルシステムや書庫の中身は
  フィルタ時に走査しない。グローバル標準・お気に入り標準の補正は対象外で、ページ個別設定
  (`このフォルダの全画像に適用` によるコピーを含む) だけを `補` として扱う。
- サムネイル左上の編集バッジ (`補` / `レ` / `消` / `隠` / `文`) と詳細表示の状態列
  (`回` を含む) も同じ編集 DB キー集合から判定する。ただし表示上のロールアップは
  「1 つ上の見える親」だけに限定し、子フォルダや直下書庫内ページをフォルダセルへさらに伝播しない。
  通常の ZIP 内仮想フォルダは直下ページのみ、内側アーカイブとして表示される ZipDir はその本の
  内部ページをまとめて示す。
- AI: v1.7.0 で `png_metadata::AiMetadata` 由来の「モデル」「生成ツール」を追加。
  現在グリッドの遅延メタデータから distinct 値を集計し、グローバル索引には保存しない。

### 6.5 既存★フィルタとの関係

`rating_filter` はツールバー・`rebuild_visible_indices`・★一時解除
(`rating_filter_suppressed_at` / `effective_rating_filter()`) と密結合している。
**いきなり廃止せず、FacetFilter の `rating` フィールドが同じ `[bool;6]` を参照**し、
UI 表面だけフィルタバー/列ドロップダウンに見せる。**既存の★一時解除挙動を壊さないこと**
(Codex P1-2 fix の不変条件)。

### 6.6 第二弾 facet (遅延データ) の扱い

解像度/動画長さ/EXIF を facet にする場合、未取得行をどう扱うか:

- ClaudeCode 初案では「未取得は通す (= フィルタ判定対象外)」「ソートは未取得行を末尾固定」
  としていた。
- **統合後の採用方針は §16.3 / §16.6 を正とする**:
  遅延列は列全体が `Ready` になるまで、その列のソート/フィルタを disabled にする。
- Loading 中はセルに `…` を出し、進捗 UI とヘッダメニューで「読み込み中」を明示する。

### 6.7 free-text 名前検索の住み分け

名前の部分一致は既存 **Ctrl+F (現在地フィルタ)** が `search_filter` 経由で担当しており、
クエリ構文 (AND/OR/NOT/フレーズ) も持つ。**FacetFilter には名前ボックスを足さず**、
free-text は Ctrl+F に任せる (重複回避)。両者は同じ合流点で AND されるので併用は自然。
→ §14 の決定 3。

### 6.8 アクティブフィルタの可視化

「なぜ 3 件しか出てないのか」を防ぐため、**適用中 facet のチップ列＋[全解除]** を常時表示。
Ctrl+F フィルタ中の親移動制限と同様、絞り込み中であることを UI で明示する。

---

## 7. ソート設計 (落とし穴: `visible_indices` 昇順前提)

詳細モードの「列クリックソート」は、**`visible_indices` が昇順である前提に依存する既存コードと
衝突する**: `idx_visible` の `binary_search`、`checked.retain`、先読み距離計算が昇順を仮定。

**対策**: `visible_indices` (昇順・フィルタ済み) は触らず、**詳細モード専用の表示順
`details_order: Vec<usize>` を別に持つ**。

- フィルタ → `visible_indices` (両モード共通、昇順維持)。
- ソート → 詳細モードのみ `details_order = visible_indices をソートキーで並べ替え`。
  描画とキーボードナビは `details_order` を参照。
- サムネモードのソートは現状どおり**ロード時の `sort_order`**を維持 (先読み等は無傷)。

副産物: 詳細モードのサイズ列ソートで実質「サイズ順」が増える (`SortOrder` には未定義)。
`SortOrder` を拡張するか、詳細モードのソートキーを独立 enum
(`DetailsSortKey { Name, Type, Ext, Size, Mtime, Rating, … }` + `ascending: bool`) にするかは
実装時判断 (独立 enum 推奨。`SortOrder` の既存テスト/UI に波及させない)。

---

## 8. 仮想フォルダ・検索ビューでの扱い

- **ZIP 内画像 / PDF ページ**: 実ファイルの mtime/size を持たず、`image_metas` には
  ZIP entry の `uncompressed_size`・PDF ページの mtime/size が入る。詳細モードは表示できるが
  「日付＝アーカイブ由来」になる点を仕様 (manual) に明記。
- **Ctrl+G 集約ビュー (`SearchContainer`)**: 既に★フィルタを抑制している。詳細モード/facet も
  同様に「コンテナ一覧では一部 facet 無効」を踏襲する。
- **`ZipSeparator`**: 詳細モードでは見出し行として描画 (列ではなくフル幅見出し)。

---

## 9. 永続化 (settings)

追加候補:

| キー | 内容 | 持ち越し方針 |
| --- | --- | --- |
| `grid_view_mode` | Thumbnail / Details | 永続 |
| `details_columns` | 表示列・順序・幅 | 永続 |
| `details_sort` | ソート列 + 昇降 | 永続 |
| `facet_filter` | 絞り込み**条件** (種別/タグ集合/日付/サイズ/編集/★) | 種類ごとに保持方針を分ける |

- **コンテナへ入ると退避 / 戻ると復元**: ZIP/PDF/フォルダなどを絞り込みで見つけて開いた場合、
  親階層の `facet_filter` をスタックに退避して内側は filter なしで始める。内側では新しい
  filter を通常どおり設定でき、Backspace などで anchor 外へ戻ったら退避していた親条件を復元する。
  種別=ZIP / 拡張子=zip のまま ZIP 内画像が 0 件になる事故を防ぐため。
- **表示候補の再集計**: タグ、拡張子、カメラ名などの無界の値集合 facet は階層ごとに候補を
  再集計する。退避中は「親絞り込み退避中」チップで可視化する。
- **表示候補 (distinct 値) は都度再集計**する。列ヘッダの候補は、その列自身の条件だけを外し、
  他のフィルタを適用した集合から集計する (§15.10)。
- **未リリース機能なのでマイグレーション不要** (CLAUDE.md「永続データ・スキーマ変更時の判断」)。
  破壊的変更を許容し、コミットメッセージにその旨を残す。
- 設定の読み書きは `settings.rs` / `settings_db.rs` (SQLite) 経路に追加。

---

## 10. パフォーマンス / UI 応答性

- **第一弾 facet は全て在メモリ** → `rebuild_visible_indices` は O(n) の述語評価で済む。
  数千件でも 1 フレーム内。distinct 値集計 (タグ/種別/拡張子) も在メモリで軽いが、
  巨大フォルダ向けに**ドロップダウンを開いたときだけ集計**してキャッシュする。
- **詳細モード描画はサムネより軽い** (テクスチャ生成ゼロ) ので新規ボトルネックは出にくい。
- **第二弾の遅延項目は必ず worker 化**。UI スレッドから同期 stat/decode/probe/read_dir 禁止
  ([ui-responsiveness.md](ui-responsiveness.md) §4 チェックリストを通す)。
- 計装: フィルタ適用・distinct 集計・遅延列充填に `perf::event` を差す
  (悪化を `analyze_perf.py` で検知できるように)。

---

## 11. 段階的実装プラン (PR 粒度)

- **Ph1 詳細モード骨格**: `GridViewMode` + ツールバー切替 + `cols=1`・行高固定で §5 第一弾列を
  描画 (ソート/フィルタ無し)。最小で「サムネ無し一覧」が出る。
- **Ph2 列ソート**: `details_order` 導入、ヘッダクリックソート、列の表示/幅設定。
- **Ph3 FacetFilter (在メモリ facet のみ)**: `rebuild_visible_indices` に AND 合流 →
  サムネモードのフィルタバー＋詳細モードの列ドロップダウン (タグ/種別/★/日付/サイズ/編集)。
  アクティブフィルタ表示＋全解除。
- **Ph4 遅延列・遅延 facet**: 解像度/動画長さ/EXIF/PDF ページ数を worker で充填。
  統合後の方針 (§16) では、対象列が `Ready` になるまでその列のソート/フィルタは disabled。
- **Ph5 テスト/スナップショット**: §12。

各フェーズで `spec.md` / `htdocs/mimageviewer/manual/` / `htdocs/mimageviewer/index.html` を
同時更新。

---

## 12. テスト

- `rebuild_visible_indices` の合成テスト: `search_filter` × `rating_filter` × `facet_filter`
  の AND が正しいか (既存テストに facet ケースを追加)。
- `details_order` のソート安定性 (同値は名前で tiebreak、昇降トグル)。
- distinct 集計の正しさ (タグ/種別/拡張子の件数)。
- 遅延 facet で「Loading/Cancelled 中は対象列のソート/フィルタ disabled、Ready 後に有効化」
  される不変条件。
- UI スナップショット (`egui_kittest`, [ui-snapshot-policy.md](ui-snapshot-policy.md)):
  詳細モード行・ヘッダ・フィルタバー・列ドロップダウンの見た目回帰。
- グリフ lint (`scripts/check_ui_glyphs.py`): 新規 UI 文字列に tofu 化文字が無いこと。

---

## 13. ドキュメント同時更新

- `htdocs/mimageviewer/manual/` — 表示モード切替・詳細列・フィルタ操作を追記。
  バージョンタグ・内部用語を書かない (CLAUDE.md「マニュアル・製品ページの記述方針」)。
- `htdocs/mimageviewer/index.html` — 機能一覧に「詳細表示 / 絞り込み」を追加。
- `docs/spec.md` — 設定項目 (`grid_view_mode` / `details_columns` / `facet_filter` …) を反映。
- `docs/search-architecture.md` — FacetFilter が `rebuild_visible_indices` の合流に加わる旨を
  追記 (Ctrl+F/G/★ との関係)。
- 本書 — 設計変更があれば追従。

---

## 14. 未決定の論点 (推奨つき)

1. **第一弾スコープ**: Ph1〜Ph3 (I/O 不要の詳細列＋在メモリフィルタ) を最初の一塊にする。
   解像度/動画長さ/EXIF (Ph4) は後追い。 → **推奨: この区切り**。
2. **フィルタ適用範囲**: サムネ／詳細**両モード共通**。 → **推奨: 共通** (実装も挙動も統一)。
3. **free-text 名前検索**: facet に名前ボックスを足さず Ctrl+F に任せる。 → **推奨: Ctrl+F に集約**。
4. **★フィルタ UI**: 既存ツールバー★ボタンを残しつつ facet にも出す (並存) か、フィルタバーへ
   集約するか。 → **推奨: まず並存** (既存挙動を壊さない)。後で統合を検討。

> これらは設計合意の対象。確定したら本書 §14 に決定を追記し、該当節へ反映する。

---

## 15. Codex 案 (追記 2026-06-09)

ClaudeCode 案は、既存の `GridItem` / `visible_indices` / ツールバー / 仮想スクロールへ素直に
接続しており、特に **`visible_indices` を昇順のまま保ち、詳細表示だけ `details_order` を持つ**
設計は採用すべき。Codex 側では、ユーザー追記の「重い I/O が必要な列も入れる」「非同期進捗を
出す」「ロード完了まではその列のソート/フィルタを無効化する」を前提に、次を補強案とする。

### 15.1 表示モードは「列数」ではなく独立状態

UI 上は列数の近くに置いてよいが、内部状態は `grid_cols` の特殊値にしない。

```rust
enum GridViewMode {
    Thumbnail,
    Details,
}
```

- ツールバー表示: 列セクションの `1..10` に `詳細` を追加する。内部状態は `GridViewMode`
  の独立値で、`grid_cols` の特殊値にはしない。
- 詳細モード中は列数・比率 UI を隠す/disabled にする。
- 詳細モードでも選択・チェック・右クリック・D&D・Enter/ダブルクリックの意味は既存グリッドと
  そろえる。
- 実装では、詳細モードへ入ると `keep_range` / `keep_set` を空にして新規サムネ要求を止め、
  既存 `Loaded` テクスチャも 1 回だけ `Evicted` にして VRAM を解放する。ただし動画サムネは
  フォルダロード時の専用 worker でしか取得しないため、詳細モード中も `Loaded` を保持する。
  プレビュー列のホバー中は、その 1 件だけを一時的な `keep_set` としてサムネイル worker に
  要求し、ホバーが外れたら非動画のテクスチャは再び破棄する。
  毎フレーム O(n) で drain/evict しないよう `details_thumb_suppression_applied` で抑制済み状態を持つ。

### 15.2 列は「軽量列」と「遅延列」を同じ列定義で扱う

列を第一弾/第二弾で概念分離しすぎると、UI と設定が二重化しやすい。最初から列定義は
1 つにして、値の取得方法だけを分ける。

```rust
enum DetailsColumnKind {
    Name,
    Rating,
    Tags,
    Kind,
    Extension,
    Size,
    Modified,
    EditState,
    ImageDimensions,
    VideoDuration,
    VideoDimensions,
    VideoCodec,
    ExifTakenAt,
    ExifCamera,
    PdfPageCount,
    ArchiveItemCount,
}

enum DetailsColumnCost {
    Immediate,
    LazyIo,
}
```

既定表示は `Name / Rating / Tags / Kind / Size / Modified / EditState` とし、遅延列は
列設定で ON にできる。ただし、列一覧には最初から入れる。これにより「重い列も列として存在する」
要望を満たしつつ、初回表示の軽さを守る。

### 15.3 遅延ロード列の状態機械

遅延列は「値が無い」ではなく、列ごとの readiness を明示する。

```rust
enum LazyColumnState {
    Disabled,                 // 列非表示。worker 対象外
    NotRequested,             // 表示ONだがまだ開始していない
    Loading { done: usize, total: usize },
    Ready { failed: usize },
    Cancelled,
}
```

- `Loading` 中のセル表示は `…`。
- 取得できない/対象外のセル表示は `-`。
- `Ready { failed > 0 }` のときはヘッダ tooltip に「取得できなかった N 件」を出す。
- フォルダ切替、検索結果ビュー差し替え、列設定変更で worker を cancel し、`items_generation`
  と照合して古い結果を捨てる。

### 15.4 遅延メタデータは idx ではなく安定キーに保存する

詳細表示のソートやフィルタで並び順が変わっても、またフォルダ再読込で idx が変わっても
流用できるよう、結果は `idx` だけに紐づけない。

```rust
struct DetailsLazyMetaKey(String); // 新規 helper で生成する安定キー

struct DetailsLazyMeta {
    image_dims: Option<(u32, u32)>,
    video_duration_secs: Option<f64>,
    video_dims: Option<(u32, u32)>,
    video_codec: Option<String>,
    exif_taken_at: Option<i64>,
    exif_camera: Option<String>,
    pdf_page_count: Option<u32>,
    archive_item_count: Option<u32>,
    failed_fields: DetailsLazyFieldFlags,
}
```

- UI は `item -> key -> meta` で参照する。
- worker 結果には `items_generation` と `source_key` を必ず含める。
- 同じキーの既存値は上書きしない。ただし mtime/size が変わった場合は無効化する。
- `GridItem::cache_key` という既存メソッドは無い。`details_lazy_meta_key_for_item(item, meta)`
  のような helper を新設し、通常ファイル、ZIP 内画像、PDF ページ、検索結果 item で衝突しない
  キーを 1 箇所に集約する。
- 再 probe 地獄を避けるため、Ph4 着手前に永続化方針を確定する。画像解像度は catalog
  `source_dims`、PDF は `pdf_meta`、同一コンテキスト中の EXIF は既存 `exif_cache` /
  `metadata_cache` を優先し、動画長さ/コーデックは catalog 拡張または details 専用 DB で
  永続化するかを決める。

### 15.5 worker は「列ごと」ではなく「必要フィールド集合」で起動する

列ごとに worker を増やすと、同じファイルを EXIF 用・寸法用に別々に開く事故が起きる。
表示中の遅延列から必要フィールドをまとめて、1 本の詳細メタ worker に渡す。

```rust
struct DetailsMetaRequest {
    generation: u64,
    items: Vec<DetailsMetaTarget>,
    fields: DetailsLazyFieldFlags,
}
```

処理順の推奨:

1. 既存 DB / メモリから取れるものを先に埋める。
   `ThumbnailState::Loaded.source_dims`、catalog の `source_dims`、PDF meta cache、既存
   `exif_cache` / `metadata_cache` など。同一フォルダ内で既に読んだ値は再利用する。
2. それでも不足するものだけ I/O する。
   画像ヘッダ/EXIF、FFmpeg probe、PDF/ZIP 列挙など。
3. UI が今見ている範囲を優先する。
   サムネ worker と同じ `scroll_hint` / `visible_end_shared` を使うか、request 構築時に
   `visible_indices` 先頭を優先順へ並べる。

I/O は `GlobalIoSemaphore` の `Normal` または `Low` を使う。ユーザーが明示的に詳細列を
表示した直後の可視行は `Normal`、画面外の補完は `Low` に寄せる。

### 15.6 進捗表示

遅延列のロード状況は、ユーザーが見失わない場所に出す。

- フィルタバー/ツールバー直下に細いステータス行:
  `詳細情報を読み込み中 124/580  |  動画 32/120  画像情報 92/460`
- 右端に小さな停止ボタン `停止`。押すと現在の details meta worker を cancel。
- ロード中の遅延列ヘッダには小さく `…` を表示。
- ロード完了後は `詳細情報 580件完了 / 取得失敗 3件` を数秒だけ toast、またはステータス行を
  自動で畳む。

進捗は「表示中の遅延列に必要な項目数」を母数にする。非表示列の未取得分まで母数に含めると
いつまでも終わらないように見える。

### 15.7 ソート/フィルタの有効化条件

ユーザー要望を優先し、ClaudeCode 案 §6.6 の「未取得は通す」「未取得は末尾」は採用しない。
遅延列では **列全体が `Ready` になるまで、その列のソート/フィルタ操作を disabled** にする。

- `Loading` 中のヘッダメニュー:
  - ソート: disabled、tooltip `詳細情報の読み込み完了後に使えます`
  - フィルタ: disabled、同上
  - `読み込みを優先` / `読み込みを停止` は有効
- `Ready` 後:
  - ソート/フィルタ有効
  - 取得失敗セルは `-` として扱い、「取得失敗」facet を出せる
- `Cancelled`:
  - ソート/フィルタ disabled
  - メニューから `読み込み再開` を出す

このルールは UX が分かりやすい。「読み込み途中に結果がじわじわ変わる」ことを避け、フィルタ結果の
安定性も保てる。

### 15.8 詳細モードのソート順とナビゲーション

`details_order` は描画順だけでなく、詳細モード中の ↑↓ / PageUp / PageDown / Home / End /
Enter の対象順にも使う。そうしないと、画面上の並びとキーボード移動がズレる。

実装時は「キーボードナビ」だけでなく、`visible_indices` の**順序**を消費している箇所を
棚卸しする。最低限、次は詳細モード中に `details_order` を見る:

- Shift+クリックの範囲選択。
- Shift+↑↓ / Shift+PageUp / Shift+PageDown などの範囲選択ナビ。
- `scroll_to_selected` / `apply_scroll_to_selected` 相当の「選択行を画面内へ寄せる」処理。
- Enter / ダブルクリックで開く対象の前後関係。

ただしフルスクリーンの前後移動やサムネモードの先読みは、既存通り `visible_indices` 昇順を使う。
詳細モードから Enter でフルスクリーンへ入った後、フルスクリーン内の前後移動まで詳細ソート順に
合わせるかは別論点。初期実装では既存挙動維持を推奨する。

### 15.9 フィルタ UI は「列ヘッダ」と「チップバー」の両面を持つ

詳細モードだけに Excel 風 UI を閉じ込めると、サムネモードで同じ絞り込みを使えない。
実装上は `FacetFilter` を 1 つだけ持ち、編集面を 2 つ用意する。

- 詳細モード: 列ヘッダの `▼` から facet。
- サムネモード: ツールバー直下のチップバーから facet。
- アクティブ条件は常にチップ表示する。詳細モードでもチップ表示を残す。

### 15.10 列ヘッダ distinct 値の集計

Excel 風チェックリストは、毎フレーム全件集計しない。ポップアップを開いた時点で、
**その列自身の filter だけを一時的に外し、他の filter を適用した集合**から distinct 値を作る。
詳細モードで列ソート中なら、この集合を `details_order` に並べ替えてから件数を集計してよい。

- 軽量列は同期集計でよい。
- 遅延列は `Ready` のときだけ同期集計。
- `Loading` / `Cancelled` の遅延列はチェックリストを出さず、進捗と再開/停止だけを出す。

### 15.11 追加したい遅延列の優先順位

重い列は全部を一度に実装せず、利用価値と実装リスクで順番を決める。

1. 画像解像度: 既存サムネ/カタログに `source_dims` があり、最も再利用しやすい。
2. 動画長さ: ユーザー価値が高い。FFmpeg probe は worker 必須。
3. EXIF 撮影日時/カメラ: 写真整理で価値が高いが、画像ファイル読みが必要。
4. PDF ページ数: 既存 `pdf_meta` と相性がよい。
5. ZIP/アーカイブ内アイテム数: 便利だが、アーカイブ列挙の I/O が重く、後回しでもよい。

---

## 16. 統合プラン (ClaudeCode 案 + Codex 案の採用版)

ここでは、§1〜§14 の ClaudeCode 案と §15 の Codex 案を統合し、実装時に採るべき最終方針を
整理する。

### 16.1 採用する基本方針

1. **表示モードは独立設定**:
   `grid_view_mode = Thumbnail / Details` を追加する。`grid_cols` には混ぜない。
2. **詳細表示は既存選択モデルを再利用**:
   `items` / `checked` / `selected` / `visible_indices` / 右クリックメニュー / D&D を共有する。
3. **フィルタ合流点は 1 つ**:
   `rebuild_visible_indices` に `FacetFilter` を AND で加える。サムネモードと詳細モードで同じ結果を
   使う。
4. **詳細ソートは `details_order`**:
   `visible_indices` は昇順のまま保つ。詳細モードの描画・詳細モード中のキーボード移動だけ
   `details_order` を使う。
5. **遅延列は列一覧に最初から含める**:
   ただし既定表示は軽量列中心。ユーザーが遅延列を表示したら非同期ロードを開始する。
6. **遅延列のソート/フィルタは Ready まで無効**:
   Loading 中に未取得行を通す/末尾に回す方式は採らない。
7. **進捗を常時可視化**:
   遅延列ロード中はフィルタバー/ツールバー直下に進捗行を出す。

### 16.2 最終列セット

**既定 ON**:

| 列 | cost | ソート | facet |
| --- | --- | --- | --- |
| 名前 | Immediate | ○ | Ctrl+F に委譲 (列 facet は拡張子のみ) |
| ★ | Immediate | ○ | ○ |
| タグ | Immediate | △ (タグ数/文字列程度) | ○ |
| 種別 | Immediate | ○ | ○ |
| サイズ | Immediate | ○ | ○ |
| 更新日時 | Immediate | ○ | ○ |
| 編集状態 | Immediate | △ | ○ |

**列設定で ON**:

| 列 | cost | ソート/フィルタ条件 | 備考 |
| --- | --- | --- | --- |
| 画像解像度 | LazyIo | Ready 後 | catalog/source_dims 優先。ZIP 内画像は entry bytes のヘッダ probe にフォールバック |
| 動画長さ | LazyIo | Ready 後 | FFmpeg probe |
| 動画解像度 | LazyIo | Ready 後 | 動画長さと同じ probe で取得 |
| 動画コーデック | LazyIo | Ready 後 | 文字列 facet |
| 撮影日時 | LazyIo | Ready 後 | EXIF |
| カメラ | LazyIo | Ready 後 | EXIF |
| PDF ページ数 | LazyIo | Ready 後 | pdf_meta / enumerate |
| アーカイブ内枚数 | LazyIo | Ready 後 | 後回し候補 |

### 16.3 FacetFilter の最終仕様

`FacetFilter` は軽量 facet と遅延 facet を同じ構造に持つ。ただし `matches()` は、対象 facet が
Ready でない場合にはその facet 条件を設定できないよう UI で止める。したがって `matches()` 内で
「未取得を通す」分岐を持たない。

```text
visible = search_filter AND effective_rating_filter AND facet_filter
```

既存★フィルタは当面残す。★ facet とツールバー★ボタンは同じ `[bool; 6]` を編集し、
一時解除挙動も既存実装を使う。

### 16.4 遅延ロード worker の最終仕様

`DetailsMetaPending` を App に追加する。

```rust
struct DetailsMetaPending {
    generation: u64,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<DetailsMetaEvent>,
}

enum DetailsMetaEvent {
    Progress {
        done: usize,
        total: usize,
        by_group: Vec<(DetailsLazyGroup, usize, usize)>,
    },
    Item {
        key: DetailsLazyMetaKey,
        meta: DetailsLazyMetaPatch,
    },
    Finished {
        failed: usize,
    },
}
```

worker のルール:

- `items_generation` が一致しない結果は破棄。
- cancel はフォルダ切替、検索結果差し替え、列設定変更、ユーザー停止で立てる。
- `GlobalIoSemaphore` を使う。可視行優先、画面外は低優先。
- 既存キャッシュ/DB/メモリ値を先に使い、必要なものだけ I/O。
- 進捗は UI が毎フレーム `try_recv` で取り込み、`ctx.request_repaint()` する。

### 16.5 進捗 UI の最終仕様

配置はツールバー/検索バーの直下、グリッド/詳細リストの上。

```text
詳細情報を読み込み中 124/580   動画 32/120   画像情報 92/460        [停止]
```

- 遅延列が 1 つも表示されていない場合は出さない。
- 完了後は非表示。ただし失敗がある場合は短いチップ `詳細情報: 3件取得失敗` を出す。
- ヘッダにも列単位の状態を出す。例: `動画長さ …`、完了後 `動画長さ`。

### 16.6 列ヘッダメニューの最終仕様

軽量列:

```text
昇順で並べ替え
降順で並べ替え
---
フィルタ
  [ ] jpg (120)
  [ ] png (32)
  [ ] mp4 (8)
---
この列を非表示
```

遅延列 Loading 中:

```text
詳細情報の読み込み中です 124/580
[読み込みを優先]
[読み込みを停止]
---
昇順で並べ替え   disabled
フィルタ         disabled
```

遅延列 Ready 後は軽量列と同じ。取得失敗がある場合は `取得失敗 (N)` facet を追加する。

### 16.7 実装フェーズの最終順序

ClaudeCode 案の Ph1〜Ph5 をベースに、遅延列の枠組みを早める。

1. **Ph1 詳細モード骨格**:
   `GridViewMode`、列セクション内の詳細切替、固定ヘッダ、固定行高リスト、既定 ON 列の描画。
2. **Ph2 詳細ソート + details_order**:
   `DetailsSortKey`、ヘッダクリック、詳細モード内キーボード移動、Shift 範囲選択の表示順対応。
   列幅/表示設定は後続へ分離。
3. **Ph3 FacetFilter 軽量版**:
   種別/拡張子/★/タグ/日付/サイズ/編集状態、チップバー、全解除。実装済み。
   コンテナへ入ると親階層の facet を退避し、内側は filter なしで始める。内側では新しい
   facet を設定でき、戻ると親条件を復元する。候補はその facet 自身だけを外した集合から
   再集計する。
4. **Ph4 遅延列基盤**:
   `DetailsLazyMeta`、`DetailsMetaPending`、進捗 UI、遅延列表示、Ready まで sort/filter disabled。
   実装済み。初期実装では `作成日時`、`画像解像度` と動画メタ列を対象にする。
   画像解像度は `fs_cache.source_dims`、catalog `source_dims`、通常画像ヘッダ probe、
   ZIP 内画像の entry bytes ヘッダ probe の順で非同期に埋める。作成日時は filesystem metadata、
   動画長さ/解像度/コーデックは FFmpeg probe を worker で実行する。
5. **Ph5 遅延列の中身を順次追加**:
   作成日時、画像解像度、動画長さ/解像度/コーデックは実装済み。残りは
   EXIF → PDF ページ数 → アーカイブ内枚数の順で追加する。
6. **Ph6 テスト/スナップショット/ドキュメント**:
   `spec.md`、manual、製品ページ、`search-architecture.md`、本書を更新。

### 16.8 未決定事項の更新

§14 の未決定事項に対する統合後の決定:

1. **第一弾スコープ**:
   Ph1〜Ph3 を最初の実装単位にする。ただし遅延列は列定義として最初から用意する。
2. **フィルタ適用範囲**:
   サムネ/詳細で共通。
3. **free-text 名前検索**:
   Ctrl+F に集約。列ヘッダの名前 facet は入れない。
4. **★フィルタ UI**:
   既存ツールバー★ボタンと facet を並存。当面は同じ状態を編集する。
5. **遅延列の未ロード扱い**:
   Loading/Cancelled 中はその列のソート/フィルタを無効化。Ready 後に有効化。
6. **フォルダ移動時の facet 保持**:
   ZIP/PDF/フォルダなどのコンテナへ入る操作では親階層の条件を退避し、内側では別条件を
   設定できる。Backspace などで anchor 外へ戻ったら親条件を復元する。

### 16.9 実装時の注意点

- 詳細モードの `scroll_offset_y` は行高基準。サムネモードのセル高とは別に記録するか、
  モード切替時に選択行へスクロールし直す。
- `selected_cell_rect` は詳細行でも更新する。選択情報オーバーレイが不要なら詳細モードでは
  無効化する。
- 詳細モードでサムネ要求を抑制する場合、`keep_set` / `requested` / `Pending` の扱いを
  明確にし、詳細→サムネ復帰時に要求が再開することをテストする。
- 詳細モードで `keep_set` を空にしても、タグ列 / タグ facet の XMP prewarm は止めない。
  `render_details_list` が可視行近傍を `details_tag_prewarm_indices` に記録し、
  `enqueue_visible_tag_prewarms` がそれを使って可視行だけ読み進める。
- 遅延列 worker はファイルを開く処理を UI スレッドへ戻さない。EXIF/FFmpeg/PDF/ZIP は全て worker。
- 仮想フォルダ (`ZipImage` / `PdfPage`) は、実ファイル列と意味が異なる値を持つ。tooltip や
  manual で「アーカイブ内の値 / 親文書由来の値」を説明する。
- Ctrl+G 集約ビューでは、コンテナ単位で意味のある facet だけ有効にする。★/タグ/編集状態の
  leaf 前提 facet は disabled に寄せる。

### 17. ClaudeCode レビュー反映 (2026-06-09)

| 指摘 | 反映先 | 方針 |
| --- | --- | --- |
| `visible_indices` 順依存が Shift+クリックにもある | §15.8 / §16.9 | 詳細ソート中は、描画順を消費する範囲選択・ナビ・スクロール寄せを `details_order` に切替える |
| Excel 風 facet の候補元 | §6.2 / §15.10 | 候補は「その列自身の filter だけ外した集合」から集計する |
| 遅延メタの再 probe リスク | §15.4 / §15.5 | catalog / pdf_meta / 既存メモリキャッシュを優先し、動画メタは Ph4 前に永続化先を決める |
| フォルダ跨ぎ facet 永続 | §9 / §16.8 | コンテナ入場時は親 facet をスタック退避し、内側で別 filter を設定可能にする。anchor 外へ戻ったら親 facet を復元 |
| `GridItem::cache_key` が存在しない | §15.4 | `details_lazy_meta_key_for_item` helper を新設する前提に修正 |
| 列ヘッダ記号の glyph risk | §4 / §16.6 | 新規 UI はテキストラベルを基本とし、記号採用時は glyph lint と実機確認を通す |
| 詳細モードでタグ prewarm が止まる | §16.9 / 実装 | サムネ `keep_set` とタグ prewarm 対象を分離し、詳細可視行近傍でタグ XMP を読む |
| 詳細ソートの比較器コスト | §7 / 実装 | `details_order` 再構築時に decorate-sort し、rating/tag/kind/state の値生成を O(n) に抑える |

### 18. 実機レビュー反映 (2026-06-09)

| 指摘 | 反映 |
| --- | --- |
| 状態列の `補正` / `局所` がサムネバッジと対応しづらい | 一覧とチップは `補` / `レ` / `消` / `隠` / `文` / `回` の短縮表記に統一。状態メニューは `レ（補正レイヤー）` のように短縮表記 + 機能名を併記する |
| ツールバーソートと列ヘッダソートが重複する | 詳細ヘッダは `昇順 → 降順 → ソートなし` の 3 トグル。`ソートなし` は `DetailsSortKey::Toolbar` とし、`visible_indices` のツールバー順を使う。ヘッダソート中はツールバーのソート操作を disabled にする |
| `表示: サムネ/詳細` でツールバーが動く | 独立した表示セクションを廃止し、列セクションに `詳細` を追加。`Alt+-` (`GridToggleDetailsView`) でサムネ/詳細を切り替え、`Alt+1..0` は詳細中でもサムネ表示へ戻して列数を適用する |
| 列表示は詳細ヘッダ右クリックで切り替えたい | ヘッダセルの context menu から、名前以外の既定列と遅延列をトグル可能にする。名前列は常時表示 |
| 作成日時も出したい | `作成日時` を既定 OFF の遅延列として追加。実ファイル/フォルダ/検索コンテナの filesystem metadata を worker で取得し、読み込み完了まで当該列のソートは無効 |

### 19. 追加 ClaudeCode レビュー反映 (2026-06-09)

このレビューは §18 の実装前の状態に対するものだったため、現行実装との差分で再確認した。

| 指摘 | 判定 / 反映 |
| --- | --- |
| タグ facet / タグソートは `tags_cache` 依存で、未 prewarm 項目を「タグなし」と扱う | 全件タグ prewarm + 進捗/Ready gate には踏み込まず、未読のタグ対象アイテムは候補に残す。タグ到着時にタグ facet 有効なら表示集合、タグ列ソート中なら `details_order` を再構築して段階更新に追従する |
| `passes_facet_filter` が毎アイテム `FacetFilter` clone とタグ `Vec` 確保を行う | 挙動は変えず、フィルタ本体は参照、タグ列はタグ条件/Tagged/Untagged 判定に入った場合だけ参照するように変更 |
| `details_lazy_meta` がセッション中に増え続ける | 50,000 件を超えたタイミングで、現在の一覧に属さない古い遅延メタを掃除する。現在フォルダ内の再利用は維持する |
| 遅延列 readiness が単一 state | 現行 MVP として維持。作成日時/画像/動画列を同時に ON にした場合は全遅延対象の完了まで sort ready にならない。列単位 readiness は後続改善 |
| 可視優先が `IoPriority` だけで worker 自身の順序に効いていない | worker へ渡す targets を可視近傍 Normal → 画面外 Low の順に並べ、逐次処理自体も可視近傍優先に変更 |
| FFmpeg probe 中のキャンセルが 10 秒 deadline のみ | interrupt callback に cancel flag を含め、フォルダ切替/停止時に in-flight probe も早く抜けられるように変更 |
