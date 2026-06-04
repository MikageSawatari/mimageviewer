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
2. memory に `snapshot_items: Vec<SnapshotEntry>` (ordered) + `snapshot_membership: HashMap<SnapshotKey, usize>` (O(1) lookup) として保持
3. snapshot 起点となった base path (= 現在のフォルダパス) を `snapshot_origin: PathBuf` に保持 (= 解除時 / 表示用)
4. **`snapshot_active = true`** に切替。grid 描画は `visible_indices` の代わりに **`snapshot_items` を render** する (= top-level grid 表示の凍結)

#### filter UI / state の扱い

| 観点 | 挙動 |
|---|---|
| **★ filter UI** | **操作可能** (= disable しない、ユーザー自由) |
| **filter state** | **常に有効** (= bypass しない、suspend layer なし) |
| **top-level grid 表示** | snapshot_items を render するので、filter を変えても影響を受けない (= 凍結) |
| **captured folder の中身** | 通常通り filter が適用される (= ★ filter / text_query / Ctrl+S 結果が中まで効く) |

#### 理由 — 自然な操作と P1-2 (Codex) 回避

`snapshot_active` 中に filter を bypass する設計だと:
- origin grid (= snapshot 表示画面そのもの) まで unfilter されて「結果が凍結」の約束が壊れる
- `settings.rating_filter` の書き換えが必要で、`settings.save()` 経路で永続化されるリスクあり
- 「中身も filter したい」要望にも対応できなくなる

代わりに本設計では:
- **filter は常に普通に効く** (= 既存実装そのまま、suspend layer 不要)
- **snapshot は top-level grid 表示だけを凍結** (= `visible_indices` 切替えで完結)
- ユーザーが代表 use case (= ★5 folder の中の無印画像 slideshow) を実現したい場合、ユーザー自身が ★ filter を「なし」に変更する (= 1 クリックで済む、操作も自然)
- snapshot 中に filter を変えても top-level grid は凍結を保つので、`[★固定]` を解除しない限り snapshot scope は維持される

#### snapshot 中の filter 変更検出 (= ステータスバー表示)

snapshot ON 時の filter 状態 (`snapshot_filter_at_capture: FilterState`) を記録し、現在 filter と差があるかを判定:

| 状態 | フォルダパス suffix 表示 |
|---|---|
| filter 不変 | `(スナップショット中 N件)` |
| filter 変更後 | `(スナップショット中 N件 / filter 変更後)` |

これにより「snapshot 中だが filter は別状態」をユーザーが認識できる (= §4.3 参照)。

#### memory フットプリント

- snapshot は **現在の visible_indices にある items の path だけ** を保持 (= 入れ子の中身は保持しない)
- 例: 100 フォルダ中 ★5 が 12 件なら snapshot は 12 path のみ
- Ctrl+G で thousand-paths × `Vec<PathBuf>` = 数 MB に届く可能性 (= P3 指摘)、許容範囲
- `HashMap<SnapshotKey, usize>` 併設で O(1) membership lookup (= path prefix match 高速化、§4.6 P1-1 解決)

#### OFF 動作 (= 再クリック / グリッド中の Esc)

1. `snapshot_items` クリア
2. `snapshot_origin` クリア
3. filter UI 復活 (= disabled 解除)
4. フォルダ表示は **snapshot 解除直前のフォルダのまま** (= ユーザーが snapshot 中に navigate していたフォルダ)

#### 永続化

**しない** (= App field に持つだけ、settings.db には書かない)。アプリ再起動で消える。

### 4.3 視覚的指標 — フォルダパスへの追記

フォルダパス表示欄 (= フォルダバー) に snapshot 中サフィックスを追加:

| 状態 | 表示例 |
|---|---|
| 通常 | `e:\path\to` |
| 固定中 (filter 不変) | `e:\path\to  (スナップショット中 12件)` |
| 固定中 (filter 変更後) | `e:\path\to  (スナップショット中 12件 / filter 変更後)` |

`N件` は `snapshot_items.len()` を反映、解除で消える。`filter 変更後` は `snapshot_filter_at_capture` と現在 filter の比較で判定 (= §4.2.1 「filter 変更検出」参照)。

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
| **Ctrl+S / Ctrl+F / Ctrl+G 検索開始** | **snapshot を解除して検索 mode に切替** (= §4.5 mutual exclusion 参照、toast で通知)。Esc 2 回押す手間を省く |
| **★ filter ボタン群** | **操作可能** (= snapshot 中も filter は普通に変更できる、§4.2.1 参照)。filter 変更は top-level grid 表示に影響しない (= snapshot が source of truth)、ただし captured folder の中身には作用する |
| **captured folder への click / fullscreen open** | 通常通り (= 中に入って閲覧可能、その際 filter が中の表示に作用する) |
| **その他のキーボード操作 (R / E / Ctrl+M 等)** | 既存挙動を維持 (= snapshot は表示範囲だけの concept、編集操作は影響しない) |

#### snapshot 内 captured folder の中の child folder

captured folder (= snapshot に含まれる Folder/Zip/Pdf) に入った後、その中の child folder への navigation は **許可** する (= 既存挙動どおり、snapshot は top-level grid だけの concept)。

例:
- snapshot = [★5 folder A, ★5 folder B]
- A に入る → A 内に child folder A_child があれば、A_child クリックで普通に入れる
- ただし parent (= snapshot より上) への navigation (BS / Alt+↑) は disable (= snapshot scope 維持)

これは「snapshot 範囲は top-level 集合の identity を凍結するだけ、各 entry の中身は自由に探索できる」という方針 (= P3 指摘の captured folder 内の child folder 入れる/入れないの明示)。

### 4.5 Ctrl+F / Ctrl+S / Ctrl+G と ★固定 の連携 — scope mutual exclusion

検索 (= Ctrl+F / Ctrl+S / Ctrl+G) と snapshot は **両方とも grid 表示に作用する scope** なので、**同時に active にしない** (= mutual exclusion)。両者の遷移は対称な「close-then-open」パターン:

#### 検索 active 中 → `[★固定]` 押下 (= 検索 → snapshot へ昇格)

1. 現在の `visible_indices` (= 検索結果) から path 一覧を抽出
2. **検索 mode を解除** (= 検索バーを閉じる、`favsearch.active = false` / Ctrl+F query クリア / Ctrl+G results クリア)
3. snapshot mode に切替 (= `snapshot_active = true`, `snapshot_items` に検索結果 paths を保存)
4. grid 表示は snapshot_items を render (= 見た目は検索結果と同じ)
5. toast 「検索結果をスナップショットに固定しました (N 件)」

これにより「検索結果のスライドショー」需要に応える:
- `Ctrl+F "夏"` で現在フォルダ内 30 件絞り込み → `[★固定]` → 30 件 snapshot → スライドショーで 30 件巡回
- `Ctrl+G "風景"` で全フォルダ横断 100 件 → `[★固定]` → 100 件 snapshot → スライドショー
- `★5 filter ON` で 50 件 → `[★固定]` → 50 件 snapshot

#### snapshot active 中 → Ctrl+F/S/G 発動 (= snapshot → 検索へ降格)

1. snapshot 解除 (= `snapshot_active = false`, `snapshot_items` クリア)
2. 検索 UI を出す (= 既存の Ctrl+F/S/G ハンドラ呼び出し)
3. toast 「★固定を解除して検索を開始しました」

両者は完全に対称な mutual exclusion。常に「検索 active」「snapshot active」「通常」の 3 状態のいずれか 1 つ。

#### Ctrl+G ストリーミング中の `[★固定]` 制限

Ctrl+G は **streaming で結果が増えていく** ので、snapshot するタイミングが不安定 (= 取った瞬間の件数で確定してしまう)。ストリーミング中は `[★固定]` ボタンを **disabled**:

- 判定: **`global_search.is_searching()`** (= pending worker + debounce-wait まで包む、P2-4 解決)
- disabled 中の tooltip: 「Ctrl+G の結果取得中は使用不可 (取得完了後にお試しください)」

#### Ctrl+G aggregated `SearchContainer` items

Ctrl+G の結果は件数が多い場合 `SearchContainer` (= 親フォルダ単位の集約 view) で表示される。本機能では:

| 方針 | 内容 |
|---|---|
| **MVP (= v1.1.0)** | aggregated view では `[★固定]` ボタンを **disabled** にする。tooltip 「集約表示中はスナップショットできません (展開してください)」 |
| **将来 (= v1.2.0)** | `SnapshotEntry::SearchContainer { container_path, expanded_paths }` を導入して、展開済み path 一覧を snapshot 化 |

MVP で disable する理由: `SearchContainer` は単一 path に正規化できず、`SnapshotEntry` のキー化が複雑 (= P2-5 への現実的対応)。展開すれば既存 path-based snapshot で対応可能。

### 4.6 フルスクリーン navigation の snapshot mode

#### owner-entry lookup (= P1-1 解決)

snapshot は **Folder/Zip/Pdf の path** を持つが、fullscreen で開かれるのは **その中の image path** (= snapshot に直接含まれない)。よって「現在の path が snapshot 内のどの entry に属するか」を判定する必要がある:

```rust
// 擬似コード
fn snapshot_owner_entry(snapshot: &[SnapshotEntry], current_path: &Path) -> Option<usize> {
    // 1. 完全一致 (= image/video entry の場合)
    if let Some(idx) = snapshot_membership.get(&snapshot_key(current_path)) {
        return Some(*idx);
    }
    // 2. prefix 一致 (= Folder/Zip/Pdf entry の場合)
    for (idx, entry) in snapshot.iter().enumerate() {
        if matches!(entry.kind, Folder | ZipFile | PdfFile)
            && current_path.starts_with(&entry.path) {
            return Some(idx);
        }
    }
    None
}
```

- ZIP/PDF entry には `entry.path + "/"` を prefix にする (= ZipImage の path 形式は `archive.zip/inner/image.png`)
- HashMap の併設で完全一致は O(1)、prefix 一致は entry 数の linear scan (= 最悪 1000 entry でも μs オーダー)

#### 混合 entry navigation の挙動 (= P1-3 解決)

snapshot は image / video / Folder / ZipFile / PdfFile が混在する可能性がある (= 例: `[★5 image X, ★5 folder A, ★5 image Y]`)。操作別の rule:

| 操作 | rule |
|---|---|
| **`Ctrl+↑↓`** | **次/前の snapshot entry** へ。entry が Folder/Zip/Pdf なら最初の image-like で fullscreen 開く。entry が image/video なら直接 open |
| **`Ctrl+PageUp/Down`** | **次/前の image-like entry** (= 直接 image/video) のみへ。Folder/Zip/Pdf entry は skip |
| **画像 `←→` `↑↓`** | 現在 owner entry の中身 image-like 移動 (= 既存挙動、snapshot scope は使わない)。owner が Folder なら A 内の image を順番に。owner が image entry なら 1 件だけなのでフォルダ境界扱い |
| **owner 末尾到達** (= ←→ で画像最終枚) | **次の playable snapshot entry** へ進む (= Folder なら最初の image、image entry なら直接 open) |
| **スライドショー末尾の自動次フォルダ** | 同上 (= 「次の playable snapshot entry」を解決して連続再生) |
| **`Esc`** | フルスクリーン解除のみ (= snapshot は維持、グリッドに戻る) |
| **`Enter`** | 既存 P10-1 どおり、フルスクリーン解除 |

#### snapshot 範囲外 path の fullscreen open

設計上、snapshot 中は範囲外 navigation を disable しているのでこのケースは原則発生しないが、エッジケース (= 例: 履歴経由) では:
- `snapshot_owner_entry()` が `None` を返す
- このときは通常 mode の DFS を使う (= snapshot は影響しない)
- toast 「現在の path はスナップショット範囲外です」(任意、UX 調整)

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
| **1** | `App` に `snapshot_active: bool`, `snapshot_items: Vec<SnapshotEntry>`, `snapshot_membership: HashMap<SnapshotKey, usize>`, `snapshot_origin: PathBuf`, `snapshot_filter_at_capture: FilterState` フィールド追加 + Default init | ~40 行 |
| **2** | `SnapshotEntry` / `SnapshotEntryKind` / `SnapshotKey` 型定義 (= GridItem を path + kind に正規化、Hash/Eq impl) | ~60 行 |
| **3** | `[★固定]` ボタン UI を ★ filter ツールバーの右に追加 (= `src/ui_main.rs` の `draw_rating_filter_button` 付近) | ~80 行 |
| **4** | ON/OFF コマンド (= ボタン handler、`visible_indices` → SnapshotEntry list 抽出 + membership map 構築 + filter capture + state 切替) | ~90 行 |
| **5** | grid 描画の切替 (= snapshot_active なら `snapshot_items` を render する、visible_indices ではなく) | ~80 行 |
| **6** | フォルダパス表示への `(スナップショット中 N件)` / `... / filter 変更後` 追記 + filter 変化検出 | ~50 行 |
| **7** | 範囲外操作の disable: BS / Alt+↑ / Alt+←/→ / フォルダツリー / フォルダパス入力欄 / お気に入りクリック (= toast 含む) | ~100 行 |
| **8** | Ctrl+S/F/G の自動解除動作 (= 検索開始前に snapshot OFF + toast) | ~40 行 |
| **9** | Ctrl+G ストリーミング中 + aggregated view の `[★固定]` disabled 判定 (= `global_search.is_searching()` + aggregated 検出) | ~40 行 |
| **10** | `snapshot_owner_entry()` 純関数 + HashMap O(1) lookup + prefix match for Folder/Zip/Pdf | ~80 行 |
| **11** | **新 `FolderNavMode::Snapshot { current_idx, ordered_entries, resume_slideshow }` variant 追加** (= P2-2、Favsearch 流用しない)。fullscreen Ctrl+↑↓ / Ctrl+PageUp/Down / 末尾→次 entry の resolver | ~200 行 |
| **12** | スライドショー末尾の snapshot mode 接続 (= 次の playable snapshot entry を resolve) | ~60 行 |
| **13** | グリッド中の Esc で解除 (= 既存検索 Esc と同じパターン、優先度: 検索 dismiss → snapshot dismiss) | ~40 行 |
| **14** | unit test: snapshot 生成 / 解除 / membership lookup / owner_entry / 混合 nav resolver / Ctrl+G streaming gate / Ctrl+F/S/G 自動解除 / filter 変化検出 / scroll 不変 / rating add/remove で snapshot 不変 (= P2-6 規制テスト含む) | ~250 行 |
| **15** | integration test: snapshot lock/unlock roundtrip / fullscreen 混合 nav / slideshow NextFolder / aggregated SearchContainer disable | ~100 行 |
| **16** | docs: `docs/keymap-spec.md` + `docs/spec.md` + マニュアル `shortcuts.html` `grid.html` `search.html` `slideshow.html` `rating.html` + `show_toolbar_rating` で隠れるケース注記 | ~80 行 |

**合計: ~1,390 行、1-2 commit**

#### v1.1.0 リリース判断

- 規模が ~1,400 行に膨らんだ (= Codex P2-7 指摘) ため、週末リリースに含めるか v1.2.0 送りか判断必要
- scope を **「★ filter 結果のみ snapshot 化、Ctrl+F/S/G 除外」** に絞れば §4.5 大半が消え、~900 行に収まる可能性。ただし v1.2.0 で再設計が必要になる懸念あり
- 推奨: 規模見積もりは Codex の値を採用 (= 過小評価リスクを取らない)。週末リリース可否は実装ペースを Step 1-3 完了時点で再評価する

---

## 6. テスト計画

### 6.1 unit test (= 純関数 / pub(crate) helper)

- `snapshot_on_captures_current_visible_indices_paths` — 現在 visible_indices の path 一覧が正しく抽出される
- `snapshot_on_builds_membership_hashmap_with_correct_indices` — `snapshot_membership` が `snapshot_items` と一致する
- `snapshot_off_clears_state_and_keeps_current_folder` — 解除後、フォルダ表示は維持される
- `snapshot_blocks_parent_navigation_via_bs` — BS が無効化される (= toast 表示の意図 flag)
- `snapshot_blocks_folder_tree_clicks` — フォルダツリー panel が disabled
- `snapshot_auto_off_on_ctrl_s_open` — Ctrl+S で snapshot 自動解除
- `snapshot_button_disabled_during_global_search_streaming` — ストリーミング中は disabled (= `is_searching()` true で disable)
- `snapshot_button_disabled_for_aggregated_search_container_view` — Ctrl+G aggregated view で disable
- **`snapshot_filter_remains_active_for_inner_folder_contents`** — snapshot 中 folder に入ると、その中身には filter が **普通に適用** される (= §4.2.1 の挙動検証、新設計)
- **`snapshot_top_level_grid_is_frozen_against_filter_change`** — snapshot 中に filter を変えても top-level grid は snapshot_items のまま (= 凍結保証)
- **`snapshot_filter_change_detection_reflected_in_path_label`** — `snapshot_filter_at_capture` と現在 filter の差が「filter 変更後」suffix に反映される
- **`snapshot_owner_entry_image_exact_match`** — image path → 完全一致で owner index 返す (O(1))
- **`snapshot_owner_entry_inner_path_prefix_match`** — Folder 内の image path → prefix match で owner Folder の index 返す
- **`snapshot_owner_entry_outside_returns_none`** — 範囲外 path → None
- **`snapshot_mixed_nav_ctrl_arrow_jumps_to_next_entry`** — `[image, folder, image]` で Ctrl+↑↓ が次 entry へ (= 混合 nav rule)
- **`snapshot_mixed_nav_ctrl_pageup_skips_folder_entries`** — Ctrl+PageUp/Down は image-like のみ
- **`snapshot_unaffected_by_rating_add_remove`** — snapshot 中に rating 変更しても `snapshot_items` 不変
- **`snapshot_unaffected_by_grid_scroll`** — grid scroll で snapshot 不変 (= keep-set 変動の影響受けない)

### 6.2 integration test (= 既存パターン応用)

- `snapshot_lock_and_unlock_roundtrip` — ON → OFF で state がきれいに戻る
- `snapshot_fullscreen_navigation_stays_in_scope` — フルスクリーン Ctrl+↓ で snapshot 範囲内のみ移動
- `snapshot_slideshow_end_jumps_to_next_in_scope_mixed_entries` — `[image, folder, image]` 混合 snapshot で slideshow `NextFolder` が次 playable entry へ
- `snapshot_inner_image_path_routes_through_owner_folder_entry` — folder 内 image path で fullscreen 開いた際 owner_entry が正しく解決される

### 6.3 実機確認項目 (= manual)

- ★5 filter → `[★固定]` → スライドショー → 末尾で次の ★5 フォルダへ進むこと
- **★5 folder (中身は無印画像) を snapshot 化 → ★ filter を「なし」に変更 → folder を開く → 中身画像が全表示される** (= 新設計、§4.2.1)
- **snapshot 中に ★ filter を変更 → top-level grid 表示は不変 (= 凍結保証) → フォルダパス suffix に `filter 変更後` が出る**
- Ctrl+G "風景" → 結果取得完了 → `[★固定]` → スライドショーで結果範囲内のみ巡回
- Ctrl+G aggregated view → `[★固定]` が disabled (tooltip 「展開してください」)
- snapshot 中の BS / フォルダツリークリックで toast が出ること
- snapshot 中の Ctrl+S → 自動解除して検索開始 (= toast「★固定を解除して検索を開始しました」)
- snapshot 中のフォルダパスに `(スナップショット中 N件)` が出ること
- snapshot 解除後、元の絞り込み state (★ filter / Ctrl+F query) が復活すること
- 混合 snapshot `[image, folder, image]` で Ctrl+↑↓ が想定通り移動 (folder は最初の image で開く / image entry は直接 open)

---

## 6.5 Codex P1/P2 対応状況 (= 2026-06-04 review 反映)

| Codex 指摘 | 対応 |
|---|---|
| **P1-1** path 判定 (owner-entry lookup) | §4.6「owner-entry lookup」節を追加、`snapshot_owner_entry()` 純関数 + HashMap + prefix match を Step 10 で実装 |
| **P1-2** filter suspend が広すぎる | §4.2.1 を全面書き換え。**filter suspend 概念ごと廃止**。snapshot は top-level grid を凍結するだけ、filter は普通に効く (= ユーザー提案採用) |
| **P1-3** 混合 entry navigation | §4.6「混合 entry navigation」節を追加、Ctrl+↑↓ / Ctrl+PageUp/Down / ←→ / slideshow の rule 明示 |
| **P2-1** §4.4 vs §4.2 矛盾 | §4.5 を「scope mutual exclusion」として全面書き換え。検索↔snapshot は対称な close-then-open。§4.4 のテーブルも §4.5 を参照する形に整理 |
| **P2-2** Favsearch 流用 NG | Step 11 で **新 `FolderNavMode::Snapshot` variant** に変更 |
| **P2-3** `settings.rating_filter` 書き換え禁止 | P1-2 解決により filter suspend 自体が消え、settings 操作なし |
| **P2-4** `global_search.is_searching()` 使用 | §4.5 / Step 9 で明記 |
| **P2-5** SearchContainer aggregated | §4.5 で MVP は disable、v1.2.0 で対応 |
| **P2-6** 追加 regression テスト | §6.1 / §6.2 に 8 件追加 (owner_entry / 混合 nav / rating 不変 / scroll 不変 / aggregated disable / filter 変化 / top-level 凍結 / inner image routing) |
| **P2-7** 規模見積もり過小 | Step 表を全面更新、~680 行 → **~1,390 行** に修正 |
| **P3** memory + child folder navigability + docs 不足 | §4.2.1 (memory 数 MB 言及 + HashMap), §4.4 (child folder 入れる), Step 16 (docs/spec.md + 各マニュアル) で反映 |

## 7. 旧版 Codex レビュー時の論点 (= 2026-06-04 初回 review に対する論点リスト、参考)

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
| 2026-06-04 | Codex P1-P3 review 反映: §4.2.1 を **filter suspend 廃止 → top-level grid 凍結のみ** に全面書き換え (P1-2 解決)。§4.6 に owner-entry lookup (P1-1) + 混合 nav rule (P1-3) 追加。§4.5 を **scope mutual exclusion** (= 検索↔snapshot 対称切替) に書き換え (P2-1 解決)。Step 11 で **新 `FolderNavMode::Snapshot` variant** に変更 (P2-2)。`is_searching()` (P2-4) / aggregated disable (P2-5) / 追加 regression テスト (P2-6) / 規模見積もり ~1,390 行に修正 (P2-7)。§6.5 に Codex 対応状況一覧表追加 |
