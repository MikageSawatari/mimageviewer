# ★固定 (Snapshot Lock) 機能 設計ドキュメント

ステータス: **設計確定、Codex レビュー待ち** (2026-06-04)
対象リリース: **v1.1.0**
規模見積もり: **~500-600 行、1-2 commit**

---

## 1. 背景・解決したい課題

### ユーザー報告

> フォルダに対して、★5 をつけておき、★5 でフィルタした後、そのフォルダの中をスライドショーして、自動的に次のフォルダに行ったときや、Ctrl+↓したとき、イメージとしては ★5 フォルダの次を開いてほしいです。ですが実際には次のフォルダに行き、その中に ★5 のアイテムがないので結果なしの画面になってしまいます。

### 根本原因

`folder_should_stop` (`src/folder_tree.rs:127`) は「画像/動画/ZIP/PDF を含むか」のみで判定し、**`rating_filter` を一切見ていない**。

スライドショー末尾 / Ctrl+↓ の DFS は:
1. 「画像があるフォルダ」を全部 stop 候補にする
2. → 次フォルダに進む
3. → そのフォルダで `rating_filter` を適用
4. → ★5 アイテムが 0 件なら「結果なし」表示

### ユーザーの真の希望

「**お気に入りフォルダだけを巡回**したい」(= 数千フォルダ中の数件レーティング済みフォルダ間を行き来したい)

---

## 2. 検討した案と却下理由

### 案 A: rating-aware folder_should_stop (skip_limit ベース)

| 項目 | 内容 |
|---|---|
| 仕様 | `folder_should_stop` に `rating_filter` を効かせる。「コンテナ自身が ★N pass」or「内部に ★N アイテム ≥ 1 件」のフォルダで stop。`folder_skip_limit` (デフォルト 5) で最悪 case を頭打ち |
| 性能 | 1 フォルダ判定 ≒ 5-10ms (rating_db indexed prefix scan + LIMIT 1)、最悪 50ms |
| **却下理由** | **数千フォルダで数件 rating の現実 (= ★ が疎)** だと skip_limit 5 では到達できない。上限を 30 や 100 に上げても根本解決にならず、「上限超えたら結果なし → ユーザーが理由を理解できず困る」UX 問題が残る |

### 案 B: 親フォルダスコープ snapshot

| 項目 | 内容 |
|---|---|
| 仕様 | 親フォルダで filter active 中に子フォルダをフルスクリーンで開く → 親の `visible_indices` を暗黙に snapshot して保持、その範囲で巡回 |
| **却下理由** | **親 → 子 → 孫の階層変動で snapshot がどこの階層か追跡できず混乱**。「★ filter を変えた瞬間に snapshot」案も、その後 階層移動すると snapshot が「過去のフォルダ」を指していて分かりにくい |

### 案 C: 永続的な仮想フォルダ (= Smart Folder / Smart Collection)

| 項目 | 内容 |
|---|---|
| 仕様 | 条件 (rating_filter / text_query / base_path 等) を保存して名前付き登録、開くたびに再 walk して結果再構築 (= Lightroom Smart Collection 風)、または結果 path list を保存 (= Apple Photos Album 風) |
| 規模 | ~580-900 行 (新 DB テーブル、命名 dialog、一覧 dialog、編集 UI) |
| **却下理由** | **規模が大きく v1.1.0 に入らない**。「★ だけのために大げさ」感もユーザーから指摘あり。条件保存方式は条件多重で複雑、結果リスト保存方式は static (= ファイル増減追従なし) で「再生成」操作が必要 |

---

## 3. 採用案: 一時的 ★固定 (Lock-mode snapshot)

### 基本コンセプト

- **永続化しない一時的な snapshot** (= アプリ再起動で消える)
- ボタンクリックで「現在の絞り込み結果」を凍結 → スナップショット中
- スナップショット中は filter UI が disabled (= 結果が変わらないことを保証)
- グリッド / フルスクリーン両方で「snapshot 範囲内のみ navigation」(= 検索結果ビューと同じセマンティクス)
- もう一度ボタンクリック (またはグリッドで Esc) で解除

### 案 C との違い

- **永続化なし** (= DB 不要、名前付け不要、一覧 dialog 不要) → 規模 1/3 以下
- **「条件保存 / 結果保存」の議論が不要** (= memory に path 一覧を持つだけ)
- **「複数フォルダのスライドショー専用」用途に絞る** (= 用途が明確、UX 設計の悩み少ない)

---

## 4. 仕様詳細

### 4.1 UI — `[★固定]` ボタン

#### 配置

★レーティングフィルタツールバーの **一番右**、区切り `|` なしで隣接させる:

```
★: [なし] [★1] [★2] [★3] [★4] [★5] [★固定]
```

`[★固定]` はタグバッジのように **枠で囲った文字ボタン**。フォントは既存のツールバー文字ボタンと同じ。

#### 状態表示

| 状態 | 見た目 | tooltip |
|---|---|---|
| **inactive (= 通常)** | 通常配色の枠付きボタン | 「現在の絞り込み結果を固定 (★/Ctrl+F/S/G の結果をスナップショットして範囲内のみ巡回)」 |
| **active (= 固定中)** | **背景色を変えて active 表示** (緑系 or 青系)、文字は強調 | 「★ 固定を解除 (N 件)」 |
| **disabled** | グレーアウト | 「Ctrl+G の結果取得中は使用不可」(= ストリーミング中) |

### 4.2 動作

#### ON 動作 (= ボタンクリックで snapshot)

1. 現在の `visible_indices` から GridItem の path 一覧を抽出 (= Folder / ZipFile / PdfFile / Image / Video / ZipImage / PdfPage を path に正規化)
2. memory に `snapshot_items: Vec<SnapshotEntry>` として保持
3. snapshot 起点となった base path (= 現在のフォルダパス) を `snapshot_origin: PathBuf` に保持 (= 解除時 / 表示用)
4. **filter UI を disabled** にする:
   - ★ filter ボタン群 (なし / ★1〜★5): disabled
   - Ctrl+F / Ctrl+S / Ctrl+G 入力欄: 既存検索が active ならその結果が固定対象、新規検索は disabled (= 後述の 4.5 参照)
5. **filter state を論理的に suspend** する (= 4.2.1 参照、重要)

### 4.2.1 filter の役割 — capture 時のみ、navigation 中は suspend

★固定 中の `rating_filter` / `text_query` / `Ctrl+S` 結果リストは、**「どの top-level item を snapshot に入れるか」を決定するためにだけ使う**。snapshot ON 後、各 snapshot folder に入った場合、その **フォルダ内部の表示には filter を適用しない** (= 全 items を見せる)。

#### 理由 — 原 use case の解決

ユーザーの代表的な use case は「**フォルダ自体が ★5、中身は無印画像**」 (= 数千フォルダから気に入った数フォルダを ★5 マークしておく) で、その中の画像を slideshow したい:

```
★5 folder A (folder itself is ★5)
  ├ image_001.jpg (no rating)
  ├ image_002.jpg (no rating)
  └ ...
```

snapshot 中 folder A に入ったとき、★5 filter が中まで効くと「★5 image が 0 件 → 結果なし」となり、**snapshot の意味がない**。よって filter は capture 時にのみ効かせ、navigation 中は suspend する。

#### 実装上の含意

- `snapshot_active` 中は `passes_rating_filter` / `passes_text_query` を **bypass** する (= 結果として常に `true` を返すラッパー、または filter state を temp に退避)
- snapshot OFF で filter state 復活 (= 解除後、元の絞り込み状態に戻る)
- 「snapshot 中の各フォルダ内でも filter したい」要望が後で来たら v1.2.0 で option 化 (= §8 後続作業を参照)

#### memory フットプリント

- snapshot は **現在の visible_indices にある items の path だけ** を保持 (= 入れ子の中身は保持しない)
- 例: 100 フォルダ中 ★5 が 12 件なら snapshot は 12 path のみ
- 100 フォルダ全部 ★5 でも 100 path、最悪 case でも数千 path = 数百 KB 程度

#### OFF 動作 (= 再クリック / グリッド中の Esc)

1. `snapshot_items` クリア
2. `snapshot_origin` クリア
3. filter UI 復活 (= disabled 解除)
4. フォルダ表示は **snapshot 解除直前のフォルダのまま** (= ユーザーが snapshot 中に navigate していたフォルダ)

#### 永続化

**しない** (= App field に持つだけ、settings.db には書かない)。アプリ再起動で消える。

### 4.3 視覚的指標 — フォルダパスへの追記

フォルダパス表示欄 (= フォルダバー) に snapshot 中サフィックスを追加:

```
通常: e:\path\to
固定中: e:\path\to  (スナップショット中 12件)
```

`(スナップショット中 N件)` は `snapshot_items.len()` を反映、解除で消える。

上部 status bar の追加は **不要** (= フォルダパス追記で十分伝わる、ユーザー合意済み)。

### 4.4 範囲外フォルダ操作の disable

snapshot 中は **「検索結果ビューと同じ一時的な仮想フォルダ」**として扱う。範囲外への移動・操作を封じる:

| 操作 | snapshot 中の挙動 |
|---|---|
| **フォルダパス入力欄** | `disabled` (= 編集不可) |
| **BS / Alt+↑ (親フォルダへ)** | 無効、トースト「スナップショット中は親フォルダに移動できません (★固定を解除してください)」 — Ctrl+S / Ctrl+G 結果中の BS と同型 |
| **Alt+← / → (履歴 戻る / 進む)** | 無効、同型のトースト |
| **フォルダツリー panel** | 全部 disabled (= クリック不可、簡易版)。snapshot 解除すれば即操作可能。 ※「snapshot 範囲内のフォルダのみクリック可能」は実装複雑なので MVP では採用しない |
| **お気に入りクリック** | 無効、同型 |
| **Ctrl+S / Ctrl+F / Ctrl+G 検索開始** | **自動解除して検索開始** (= snapshot OFF してから検索 UI を出す)。Esc 2 回押す手間を省く |
| **その他のキーボード操作 (R / E / Ctrl+M 等)** | 既存挙動を維持 (= snapshot は表示範囲だけの concept、編集操作は影響しない) |

### 4.5 Ctrl+F / Ctrl+S / Ctrl+G と ★固定 の連携

#### snapshot 化対象

★ filter だけでなく、Ctrl+F / Ctrl+S / Ctrl+G の結果も snapshot 化可能 (= ボタン押下時の `visible_indices` から path 抽出するので、絞り込み元が何であれ同じ仕組み)。

具体例:
- `Ctrl+G "風景"` で全フォルダ横断検索 → 100 件ヒット → `[★固定]` → 100 件のスナップショット → スライドショー
- `Ctrl+F "夏"` で現在フォルダ内 30 件絞り込み → `[★固定]` → 30 件のスナップショット
- `★5 filter ON` → 50 件 → `[★固定]` → 50 件のスナップショット

#### Ctrl+G ストリーミング中の制限

Ctrl+G (グローバル検索) は **streaming で結果が増えていく** ので、snapshot するタイミングが不安定。ストリーミング中は `[★固定]` ボタンを **disabled** にする。

- 判定: `App` に Ctrl+G ストリーミング中フラグ (`global_search.streaming` 等) があるはず、それを参照
- disabled 中の tooltip: 「Ctrl+G の結果取得中は使用不可 (取得完了後にお試しください)」

### 4.6 フルスクリーン navigation の snapshot mode

snapshot active 中、フルスクリーンで開いた path が `snapshot_items` 内に含まれる場合、既存の `FolderNavMode::Favsearch` 経路と同型の動作:

| 操作 | snapshot mode 中の挙動 |
|---|---|
| `Ctrl+↑↓` | `snapshot_items` 内の **次/前の Folder/ZipFile/PdfFile** へ移動 |
| `Ctrl+PageUp/Down` | `snapshot_items` 内の **次/前の image-like** へ移動 (= 兄弟移動相当、ただし scope は snapshot) |
| 画像 `←→` `↑↓` | 現在開いてるフォルダ内 image-like 移動 (= snapshot 範囲内のフォルダなので既存挙動でほぼ動く、ただしフォルダ末尾で次フォルダに進む際は snapshot scope を使う) |
| **スライドショー末尾の自動次フォルダ** | **snapshot 範囲内の次のフォルダへ** (= 本機能のキモ) |
| `Esc` | フルスクリーン解除のみ (= snapshot は維持、グリッドに戻る) |
| `Enter` | 既存 P10-1 どおり、フルスクリーン解除 |

**重要**: snapshot folder に入った後、その **フォルダ内部の image-like 一覧は filter なしで全表示** される (= §4.2.1 参照)。「フォルダ自体が ★5 + 中身は無印画像」が代表 use case なので、中身まで filter が効くと意味がなくなるため。

snapshot 範囲外の path をフルスクリーンで開いた場合 (= 例: snapshot 中にお気に入りクリックは disabled なので発生しないはずだが、エッジケース対策):
- 通常 mode の DFS を使う (= snapshot は影響しない)
- ただし設計上、snapshot 中はナビゲーション全般を disable しているのでこのケースは起きない想定

### 4.7 永続化方針

| 観点 | 方針 |
|---|---|
| **App 再起動** | snapshot は消える (= memory only) |
| **フォルダ移動** | snapshot 中は disable なので発生しない |
| **`settings.db` への書き出し** | なし |
| **スナップショット保存・命名・編集** | 提供しない (= v1.2.0 で「保存済み一覧」機能として正式実装する場合の別議論) |

---

## 5. 実装ステップ

| Step | 内容 | 規模 |
|---|---|---|
| **1** | `App` に `snapshot_active: bool`, `snapshot_items: Vec<SnapshotEntry>`, `snapshot_origin: PathBuf` フィールド追加 + Default init | ~30 行 |
| **2** | `SnapshotEntry` 型定義 (= `{ path: PathBuf, kind: SnapshotEntryKind }` のような、GridItem を path-only に正規化) | ~50 行 |
| **3** | `[★固定]` ボタン UI を ★ filter ツールバーの右に追加 (= `src/ui_main.rs` の `draw_rating_filter_button` 付近) | ~80 行 |
| **4** | ON/OFF コマンド (= ボタン handler、`visible_indices` → path list 抽出 + state 切替) | ~80 行 |
| **5** | フォルダパス表示への `(スナップショット中 N件)` 追記 | ~30 行 |
| **6** | 範囲外操作の disable: BS / Alt+↑ / Alt+←/→ / フォルダツリー / フォルダパス入力欄 / お気に入りクリック | ~80 行 |
| **7** | Ctrl+S/F/G の自動解除動作 (= 検索開始前に snapshot OFF を呼ぶ) | ~30 行 |
| **8** | Ctrl+G ストリーミング中の `[★固定]` disabled 判定 | ~20 行 |
| **9** | フルスクリーン Ctrl+↑↓ / スライドショー末尾の snapshot mode 接続 (= `FolderNavMode::Favsearch` 流用 or 新 mode) | ~80 行 |
| **10** | グリッド中の Esc で解除 (= 既存検索 Esc と同じパターン) | ~30 行 |
| **11** | unit test (= snapshot 生成 / 解除 / nav 移動 / 範囲外操作の disabled / Ctrl+G streaming gate / Ctrl+F/S/G 自動解除) | ~120 行 |
| **12** | docs (= `docs/keymap-spec.md` + `docs/fullscreen-navigation-consistency.md` + マニュアル `htdocs/.../shortcuts.html` `grid.html`) | ~50 行 |

**合計: ~680 行、1-2 commit**

---

## 6. テスト計画

### 6.1 unit test (= 純関数 / pub(crate) helper)

- `snapshot_active_disables_filter_ui` — snapshot ON 中は ★ filter / 入力欄が disabled flag になる
- `snapshot_on_captures_current_visible_indices_paths` — 現在 visible_indices の path 一覧が正しく抽出される
- `snapshot_off_clears_state_and_keeps_current_folder` — 解除後、フォルダ表示は維持される
- `snapshot_blocks_parent_navigation_via_bs` — BS が無効化される (= toast 表示の意図 flag)
- `snapshot_blocks_folder_tree_clicks` — フォルダツリー panel が disabled
- `snapshot_auto_off_on_ctrl_s_open` — Ctrl+S で snapshot 自動解除
- `snapshot_button_disabled_during_global_search_streaming` — ストリーミング中は disabled
- `snapshot_path_contains_check` — フルスクリーン open 時の「path が snapshot 内か」判定
- **`snapshot_suspends_rating_filter_for_inner_folder_contents`** — snapshot 中 folder に入ると、その中身は filter が bypass される (= §4.2.1 の挙動検証)
- **`snapshot_off_restores_filter_state`** — 解除後に元の rating_filter / text_query が復活する

### 6.2 integration test (= 既存パターン応用)

- `snapshot_lock_and_unlock_roundtrip` — ON → OFF で state がきれいに戻る
- `snapshot_fullscreen_navigation_stays_in_scope` — フルスクリーン Ctrl+↓ で snapshot 範囲内のみ移動
- `snapshot_slideshow_end_jumps_to_next_in_scope` — スライドショー末尾で snapshot 内の次フォルダへ

### 6.3 実機確認項目 (= manual)

- ★5 filter → `[★固定]` → スライドショー → 末尾で次の ★5 フォルダへ進むこと
- **★5 folder (中身は無印画像) を snapshot 化 → folder を開く → 中身画像が全表示される** (= filter suspend 確認、§4.2.1)
- Ctrl+G "風景" → 結果取得完了 → `[★固定]` → スライドショーで結果範囲内のみ巡回
- snapshot 中の BS / フォルダツリークリックで toast が出ること
- snapshot 中の Ctrl+S → 自動解除して検索開始
- snapshot 中のフォルダパスに `(スナップショット中 N件)` が出ること
- snapshot 解除後、元の絞り込み state (★ filter / Ctrl+F query) が復活すること

---

## 7. Codex レビュー時の論点

### 設計の妥当性 (= 採用案 D が筋か)

- 案 A/B/C を却下した理由は正当か?
- 案 D (= 一時的 snapshot) の用途限定 (= スライドショー専用) は適切か?
- 永続化なしの判断は妥当か? (= v1.2.0 で「保存済み一覧」として正式実装する余地を残す前提)

### 既存機能との衝突

- `FolderNavMode::Favsearch` (= Ctrl+S 経路) と snapshot mode の重複・矛盾はないか?
- `FolderNavMode` enum に新 variant `Snapshot` を追加すべきか、Favsearch を再利用すべきか?
- グリッド中の Esc が「snapshot 解除」になることで既存の Esc 動作 (= 検索解除など) と衝突しないか?
  - 検索中 + snapshot 中: 検索が active のはず (= snapshot に取り込まれる)、その状態の Esc は?
- フォルダツリー panel の disable と、既存「フルスクリーン中はツリー操作 disable」状態の整合性
- Ctrl+G ストリーミング中の `[★固定]` disabled 判定で、ストリーミング完了フラグの取り方は?

### UX の細部

- **filter suspend の妥当性** (= §4.2.1): snapshot folder 内部で filter を bypass する設計は妥当か? ユーザーが「snapshot 中も filter かけたい」と思う case は v1.2.0 まで先送りで OK か?
- snapshot 中の `←→` (画像ページめくり) で同フォルダ末尾に達したとき、自動で snapshot 内の次フォルダ先頭へ進む? それともフォルダ末尾でとまる? (= 既存「フォルダ末尾の境界 hint」との関係)
- snapshot 解除後、フォルダ表示は維持 (= 仕様) だが、ユーザーが「snapshot 中に開いたフォルダ」が解除元と違う場合、解除後どこに居るのが自然?
  - 案 A: 解除直前のフォルダのまま
  - 案 B: snapshot 起点 (`snapshot_origin`) のフォルダに戻す
- `[★固定]` ボタンの位置: ★ filter ツールバーの右端で確定だが、★ filter 自体がツールバー領域不足で hidden になる場合 (= 狭ウィンドウ) はどうする?
- snapshot 中に新規ファイルがフォルダに追加された場合、snapshot に反映されない (= 仕様) が、ユーザーに伝える必要あるか?

### 実装規模見積もりの妥当性

- 680 行は現実的か? (= 過大評価 / 過小評価)
- `FolderNavMode::Favsearch` の流用で 80 行は妥当か?
- フォルダツリー panel の disable 実装は本当に簡単か? (= 既存コード調査必要)

### テストカバレッジ

- 11 件の unit test + 3 件の integration test で十分か?
- snapshot 中の各 navigation 経路 (= Ctrl+↑↓, Ctrl+PageUp/Down, BS, Alt+↑, Alt+←/→, スライドショー末尾, フルスクリーン Esc) を網羅できているか?
- Ctrl+G ストリーミング中の disabled テストの fixture (= ストリーミング状態の作り方) は簡単か?

---

## 8. 後続作業 (= v1.1.0 以降)

### v1.1.0 出荷後の評価項目

実機運用 (= ユーザー or 開発者が日常で使う) で以下を観察し、v1.2.0 計画に反映:

- snapshot 範囲が狭すぎる / 広すぎるケースの頻度
- 「再生成したい」要望の頻度 (= 永続化要望に直結)
- snapshot 解除タイミングの混乱があるか (= UX 改善余地)

### v1.2.0 候補

- **「保存済み一覧」機能** (= 案 C のリベンジ、永続化 + 名前付け、ただし v1.1.0 ★固定が使いやすければ不要かも)
- **「★固定」中の filter 部分変更** (= 現状は全 disabled だが、★ レベルの調整くらいは許可してリアルタイム再 snapshot する案)
- **Ctrl+G ストリーミング中も逐次 snapshot** (= 現状は disabled、ただし complexity 増)

---

## 9. 関連コード位置 (= 実装時の参考)

| 機能 | ファイル / 関数 |
|---|---|
| ★ filter ツールバー UI | `src/ui_main.rs` `draw_rating_filter_button` 周辺 (row 213-) |
| `passes_rating_filter` | `src/app.rs:1129` |
| `FolderNavMode::Favsearch` | `src/folder_tree.rs` and `src/app.rs` (= 既存 Ctrl+S 経路) |
| `folder_should_stop` | `src/folder_tree.rs:127` |
| `navigate_folder_with_skip` | `src/folder_tree.rs` |
| Ctrl+S 検索バー | `src/ui_main.rs:1882` 周辺 (`raw_enter_pressed`) |
| Ctrl+F 検索バー | `src/ui_main.rs:1702` 周辺 |
| `global_search` ストリーミング状態 | `src/global_search.rs` (要調査) |
| フォルダパス表示 (フォルダバー) | `src/ui_main.rs` (= 要 grep `folder_bar` or `フォルダパス`) |
| グリッド Esc 既存挙動 | `src/app.rs` (= 検索解除等) |
| draw_cell (= snapshot 中の Folder バッジ表示と並列) | `src/app.rs:27236` |

---

## 10. 変更履歴

| 日付 | 内容 |
|---|---|
| 2026-06-04 | 初版 (= 議論経緯 + 案 D 設計確定) |
| 2026-06-04 | §4.2.1 追加 (= filter は capture 時のみ、navigation 中は suspend) + §4.6 / §6 / §7 に該当反映。ユーザー質問「★3 filter 中の入れ子★2 は除外される?」への回答として明示 |
