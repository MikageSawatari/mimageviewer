# ★固定 (Snapshot Lock) 機能 設計ドキュメント

ステータス: **設計確定、Codex 2nd review 反映済み、実装着手可能** (2026-06-04)
対象リリース: **v1.1.0** (= 規模により v1.2.0 送り判断あり、Step 1-3 完了時点で再評価)
規模見積もり: **~1,500-1,700 行、2-3 commit** (= Codex P2-7 + 残指摘修正後)

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
- ボタンクリックで「現在の絞り込み結果 (= visible_indices)」を path list に凍結 → top-level grid 表示が snapshot_items の render に切替
- **filter UI は操作可能** (= filter は普通に効く。snapshot は top-level grid 表示の凍結のみが役割、§4.2.1)
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

snapshot ON 時の filter 状態 `snapshot_filter_at_capture: FilterState` を記録し、現在 filter と差があるかを判定:

```rust
#[derive(Clone, PartialEq, Eq)]
pub struct FilterState {
    /// ★ filter (= snapshot 中も操作可能、これが主な変化観察対象)
    rating_filter: [bool; 6],  // index 0=未評価, 1=★1, ..., 5=★5
}
```

**含めない**もの (= mutual exclusion で consume されるため、snapshot 中は常に「無効」状態):
- `text_query` (Ctrl+F) — snapshot ON 時に clear、snapshot 中に新規 query すると snapshot 解除されるので比較不要
- `favsearch_query` (Ctrl+S) — 同上
- `global_search_query` (Ctrl+G) — 同上

代わりに **snapshot がどんな source から作られたか** は `SnapshotSourceLabel` で別途記録:

```rust
#[derive(Clone)]
pub enum SnapshotSourceLabel {
    RatingFilter { active_levels: Vec<u8> },  // 例: [5] = ★5 filter から
    TextSearch { query: String },              // Ctrl+F の query
    FavSearch { query: String },               // Ctrl+S の query
    GlobalSearch { query: String },            // Ctrl+G の query
    Mixed,                                     // 複数 source の組み合わせ
}
```

これは tooltip / debug log で「この snapshot は何から来たか」を示すために使う (= ユーザーが「★5 で固定したやつ」と思い出せる)。

| 状態 | フォルダパス suffix 表示 |
|---|---|
| filter 不変 (= `rating_filter` 一致) | `(スナップショット中 N件)` |
| filter 変更後 (= `rating_filter` 差あり) | `(スナップショット中 N件 / filter 変更後)` |

これにより「snapshot 中だが filter は別状態」をユーザーが認識できる (= §4.3 参照)。

#### memory フットプリント

- snapshot は **現在の visible_indices にある items の path だけ** を保持 (= 入れ子の中身は保持しない)
- 例: 100 フォルダ中 ★5 が 12 件なら snapshot は 12 path のみ
- Ctrl+G で thousand-paths × `Vec<PathBuf>` = 数 MB に届く可能性 (= P3 指摘)、許容範囲
- `HashMap<SnapshotKey, usize>` 併設で O(1) membership lookup (= path prefix match 高速化、§4.6 P1-1 解決)

#### OFF 動作 (= 再クリック / グリッド中の Esc)

1. `snapshot_items` / `snapshot_membership` / `snapshot_origin` / `snapshot_filter_at_capture` クリア
2. `snapshot_active = false` で grid 表示を `visible_indices` ベースに戻す (= 現在の filter state が反映される)
3. フォルダ表示は **snapshot 解除直前のフォルダのまま** (= ユーザーが snapshot 中に navigate していたフォルダ)
4. **検索 mode の state は consume 済み** (= snapshot ON 時に clear したものは復元しない、§4.5 mutual exclusion の対称性)。Ctrl+F/S/G の query を残したい場合はユーザーが再入力

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

**順序が重要** (= state 上書きで取りこぼし防止):

1. **capture first**: 現在の `visible_indices` (= 検索結果) から SnapshotEntry list + SnapshotKey membership map を構築。この時点で local 変数に持つ
2. **close search**: 検索 mode を解除 (= `favsearch.active = false` / `search_pending = false` / Ctrl+F query クリア / Ctrl+G results クリア / search bar UI を hide)
3. **activate snapshot**: `snapshot_active = true`, `snapshot_items` / `snapshot_membership` / `snapshot_origin` / `snapshot_filter_at_capture` をセット
4. **render switch**: grid 表示は次フレームから snapshot_items を render (= 見た目は検索結果と同じ)
5. toast 「検索結果をスナップショットに固定しました (N 件)」

検索 state は **consume** する (= snapshot OFF 時に restore しない、§4.5 mutual exclusion の対称性。検索を続けたい場合はユーザーが再入力)。これは Ctrl+S → Ctrl+G の検索乗り換え時と同じ pattern。

#### dirty/pending 検索の snapshot 禁止 (= P2-new)

Ctrl+F / Ctrl+S / Ctrl+G いずれも、**pending 状態 (= query 入力中で未確定) では `[★固定]` ボタンを disabled** にする。stale 結果を snapshot しないため:

| 検索種 | pending 判定 | disabled tooltip |
|---|---|---|
| Ctrl+G | `global_search.is_searching()` | 「Ctrl+G の結果取得中は使用不可」 |
| Ctrl+F | `search_pending` または `query != last_executed` | 「検索結果の確定後にお試しください」 |
| Ctrl+S | `favsearch_pending` または `query != last_executed` | 「検索結果の確定後にお試しください」 |

ユーザーが query を確定する (= Enter を押す or debounce 完了) と pending が解除され、`[★固定]` が enable される。代替案 (= ボタン押下時に強制実行) も検討したが、UX が「ボタン押したのに何も起きない数秒」になるため disable で統一。

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

#### `SnapshotKey` 厳密定義 (= P1-1 解決の前提)

`SnapshotKey` は path を normalize した hash 可能な値で、Windows の path 比較の落とし穴を全部解消する:

```rust
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub enum SnapshotKey {
    /// 通常の filesystem path (= Folder, Image, Video)
    Fs(String),
    /// アーカイブ inner path (= ZipImage / PdfPage)
    Archive { container: String, inner: String },
}

fn snapshot_key_from_path(path: &Path) -> SnapshotKey {
    // 1. ZipFile / PdfFile 内かを判定
    if let Some((container, inner)) = split_archive_path(path) {
        return SnapshotKey::Archive {
            container: normalize_fs(container),
            inner: normalize_inner(inner),
        };
    }
    SnapshotKey::Fs(normalize_fs(path))
}

fn normalize_fs(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = strip_extended_prefix(&s);  // `\\?\C:\...` → `C:\...`
    let s = s.replace('/', "\\");        // unix-style → windows-style
    let s = strip_trailing_separator(&s); // `C:\foo\` → `C:\foo`
    s.to_lowercase()                      // Windows case-insensitive
}
```

| 正規化観点 | 処理 |
|---|---|
| **Windows 大文字小文字** | `to_lowercase()` で吸収 (= `e:\Foo` と `E:\foo` は同一) |
| **separator** | `/` を `\` に正規化 |
| **trailing separator** | `\\foo\\` → `\\foo` |
| **`\\?\` extended prefix** | drive path に正規化 (= `\\?\C:\foo` → `C:\foo`) |
| **UNC path** | `\\server\share\foo` はそのまま (= `\\?\UNC\server\share\foo` も `\\server\share\foo` に正規化) |
| **ZipFile / PdfFile inner path** | container と inner を分離 (`Archive { container, inner }`)。container は fs と同じ normalize、inner は内部 separator のみ統一 |
| **inner path の例** | `e:\foo.zip\sub\image.png` → `Archive { container: "e:\\foo.zip", inner: "sub\\image.png" }` |

`split_archive_path()` は既存の zip_loader / pdf_loader の path 判定ロジックを流用 (= 拡張子 `.zip` / `.pdf` の境界を path 中から検出)。

#### `snapshot_owner_entry` (= P1-1 解決)

snapshot は **Folder/Zip/Pdf の path** を持つが、fullscreen で開かれるのは **その中の image path** (= snapshot に直接含まれない)。よって「現在の path が snapshot 内のどの entry に属するか」を判定する必要がある:

```rust
fn snapshot_owner_entry(app: &App, current_path: &Path) -> Option<usize> {
    let key = snapshot_key_from_path(current_path);
    // 1. 完全一致 (= image/video/zipimage/pdfpage entry の場合)
    if let Some(&idx) = app.snapshot_membership.get(&key) {
        return Some(idx);
    }
    // 2. prefix 一致 (= Folder/Zip/Pdf entry の場合)
    //    SnapshotKey::Archive にも対応: container 一致なら owner
    for (idx, entry) in app.snapshot_items.iter().enumerate() {
        match &entry.key {
            SnapshotKey::Fs(fs_path) if matches!(entry.kind, Folder | ZipFile | PdfFile) => {
                // current が fs_path 配下か (= separator 境界で判定、sibling false positive 防止)
                if is_inside_fs(&key, fs_path) { return Some(idx); }
            }
            SnapshotKey::Archive { container, inner: entry_inner } if matches!(entry.kind, ZipFile) => {
                // ZipFile entry に対する inner image lookup (= rare、通常 ZipFile は inner なしで snapshot)
                if let SnapshotKey::Archive { container: cur_c, inner: cur_i } = &key {
                    if cur_c == container && cur_i.starts_with(entry_inner) { return Some(idx); }
                }
            }
            _ => {}
        }
    }
    None
}

fn is_inside_fs(child: &SnapshotKey, parent_fs: &str) -> bool {
    match child {
        SnapshotKey::Fs(c) => c.starts_with(parent_fs) && c[parent_fs.len()..].starts_with('\\'),
        SnapshotKey::Archive { container, .. } => {
            container.starts_with(parent_fs) && container[parent_fs.len()..].starts_with('\\')
        }
    }
}
```

| 観点 | 処理 |
|---|---|
| 完全一致 | HashMap O(1) (= image/video entry を直接 fullscreen で開いた場合) |
| prefix 一致 | entry 数の linear scan (= 最悪 1000 entry でも μs オーダー) |
| **sibling false positive 防止** | `is_inside_fs()` で separator 境界を確認 (= `C:\foo` は `C:\foobar\baz` を own しない) |
| **case-only 差** | `SnapshotKey` 構築時に小文字化済みなので吸収 |
| **trailing separator** | normalize 済みなので吸収 |
| **ZipFile inner path** | `Archive { container, inner }` 構造で比較 (= raw `starts_with` の落とし穴回避) |

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

#### terminal rules (= 境界の挙動、P2-mixed nav 残)

| 観点 | rule |
|---|---|
| **folder 内 ordering** | **既存の grid sort + current filter を尊重** (= snapshot 中も filter は普通に効く §4.2.1)。`load_folder` 経路で得られる順序そのまま |
| **「最初の image-like」が無い folder** | scan 時に skip して次 playable entry を探す (= folder 配下に image が 0 件 / video 0 件のケース)。skip しても snapshot scope 内に playable が残っているか確認 |
| **scan 上限** | 探索は **snapshot_items 全体まで** linear scan (= 最悪 1000 entry でも μs オーダー)。1 件も playable が無ければ「snapshot 全体に再生対象なし」状態 |
| **snapshot 末尾到達** | **wrap せず stop**、フルスクリーン境界 hint (= 既存「次/前のフォルダがありません」と同形 toast) を表示。slideshow も末尾停止 (= §4.6 既存 slideshow 末尾動作と整合) |
| **snapshot 解除中の pending nav** | snapshot toggle OFF で pending folder load が cancel される (= snapshot_generation_id で識別、§5 Step 11 参照) |

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
| **1** | `App` に `snapshot_active: bool`, `snapshot_items: Vec<SnapshotEntry>`, `snapshot_membership: HashMap<SnapshotKey, usize>`, `snapshot_origin: PathBuf`, `snapshot_filter_at_capture: FilterState`, `snapshot_source_label: SnapshotSourceLabel`, `snapshot_generation_id: u64` フィールド追加 + Default init | ~50 行 |
| **2** | `SnapshotEntry` / `SnapshotEntryKind` / `SnapshotKey` 型定義 (= GridItem を path + kind に正規化、Hash/Eq impl) + `snapshot_key_from_path()` + `normalize_fs()` + `split_archive_path()` + `is_inside_fs()` 純関数群 (= §4.6 SnapshotKey 厳密定義の実装) | ~120 行 |
| **3** | `[★固定]` ボタン UI を ★ filter ツールバーの右に追加 (= `src/ui_main.rs` の `draw_rating_filter_button` 付近) | ~80 行 |
| **4** | ON/OFF コマンド (= ボタン handler、`visible_indices` → SnapshotEntry list 抽出 + membership map 構築 + filter capture + state 切替) | ~90 行 |
| **5** | grid 描画の切替 (= snapshot_active なら `snapshot_items` を render する、visible_indices ではなく) | ~80 行 |
| **6** | フォルダパス表示への `(スナップショット中 N件)` / `... / filter 変更後` 追記 + filter 変化検出 | ~50 行 |
| **7** | 範囲外操作の disable: BS / Alt+↑ / Alt+←/→ / フォルダツリー / フォルダパス入力欄 / お気に入りクリック (= toast 含む) | ~100 行 |
| **8** | Ctrl+S/F/G の自動解除動作 (= 検索開始前に snapshot OFF + toast) | ~40 行 |
| **9** | Ctrl+G ストリーミング中 + aggregated view の `[★固定]` disabled 判定 (= `global_search.is_searching()` + aggregated 検出) | ~40 行 |
| **10** | `snapshot_owner_entry()` 純関数 + HashMap O(1) lookup + prefix match for Folder/Zip/Pdf | ~80 行 |
| **11** | **新 `FolderNavMode::Snapshot { generation_id, current_idx, action_kind, media_policy }` variant 追加** (= P2-2、Favsearch 流用しない)。fullscreen Ctrl+↑↓ / Ctrl+PageUp/Down / 末尾→次 entry の resolver。`generation_id` で snapshot OFF 後の pending nav 識別、`action_kind` (= Arrow / Page / OwnerEnd / Slideshow) で resolver 切替、`media_policy` (= AllowVideo / StillOnly) で slideshow 時の動画 skip | ~230 行 |
| **12** | スライドショー末尾の snapshot mode 接続 (= 次の playable snapshot entry を resolve) | ~60 行 |
| **13** | グリッド中の Esc で解除 (= 既存検索 Esc と同じパターン、優先度: 検索 dismiss → snapshot dismiss) | ~40 行 |
| **14** | unit test: snapshot 生成 / 解除 / membership lookup / owner_entry / 混合 nav resolver / Ctrl+G streaming gate / Ctrl+F/S/G 自動解除 / filter 変化検出 / scroll 不変 / rating add/remove で snapshot 不変 + **path normalization (case fold / trailing sep / extended prefix / UNC / Zip inner) / sibling false positive 防止 / dirty Ctrl+F/S gate / no-playable folder skip / snapshot OFF 時の pending nav cancel (= generation_id 識別) / slideshow mid-folder transition** (= P2-6 規制テスト全部含む) | ~340 行 |
| **15** | integration test: snapshot lock/unlock roundtrip / fullscreen 混合 nav / slideshow NextFolder / aggregated SearchContainer disable | ~100 行 |
| **16** | docs: `docs/keymap-spec.md` + `docs/spec.md` + マニュアル `shortcuts.html` `grid.html` `search.html` `slideshow.html` `rating.html` + `show_toolbar_rating` で隠れるケース注記 | ~80 行 |

**合計: ~1,560 行、2-3 commit** (= SnapshotKey 仕様 + dirty gate + state 拡張 + 追加テスト分)

#### v1.1.0 リリース判断

- 規模が ~1,560 行に膨らんだため、週末リリースに含めるか v1.2.0 送りか判断必要
- scope を **「★ filter 結果のみ snapshot 化、Ctrl+F/S/G 除外」** に絞れば §4.5 大半が消え、~1,000 行に収まる可能性。ただし v1.2.0 で再設計が必要になる懸念あり
- 推奨: 規模見積もりは Codex の値を採用 (= 過小評価リスクを取らない)。週末リリース可否は実装ペースを Step 1-3 完了時点で再評価する
- 1 commit ではなく **2-3 commit に分割** 推奨:
  - Commit 1: Step 1-2 (型定義 + 正規化純関数) + 単体テスト (~250 行)
  - Commit 2: Step 3-10 (UI + ON/OFF + 視覚指標 + 範囲外 disable + mutual exclusion + owner_entry) + 単体テスト (~700 行)
  - Commit 3: Step 11-16 (FolderNavMode::Snapshot + 末尾→次 entry + Esc + integration test + docs) (~610 行)

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

### Codex 2nd review (= 2026-06-04 同日) の追加指摘と対応

| Codex 2nd 指摘 | 対応 |
|---|---|
| **P1-1 still weak** SnapshotKey 厳密定義不足 | §4.6 に SnapshotKey 厳密定義節を追加: case-fold / separator 正規化 / `\\?\` extended prefix / UNC / Archive { container, inner } 構造 + `is_inside_fs()` で sibling false positive 防止 |
| **P2-1 残** §4.5 lifecycle 文言矛盾 | §4.5 で「capture → close → activate」3 段の順序を明示。検索 state は **consume** (= restore しない) を明文化。OFF 動作で「state は consume 済み、復元しない」追記 |
| **P2-new** dirty/pending Ctrl+F/S で stale 結果 snapshot | §4.5 に dirty/pending 検索の disable gate 追加: `search_pending` / `favsearch_pending` / `query != last_executed` でも disabled |
| **P2-FilterState** 未定義 + §4.5 と矛盾 | §4.2.1 で `FilterState { rating_filter: [bool; 6] }` のみに絞る (= text_query 等は consume されるので除外) + `SnapshotSourceLabel` 別途定義 |
| **P1-3 残** terminal rules 不足 | §4.6 に「terminal rules」節追加: folder 内 ordering (= 既存 sort + filter)、no-playable folder skip、末尾は wrap せず stop + boundary hint、snapshot OFF 時の pending nav cancel |
| **Step 11 state 不完全** | Step 11 を `FolderNavMode::Snapshot { generation_id, current_idx, action_kind, media_policy }` に拡張 |
| **P2-6 残** 追加 regression | Step 14 unit test 数を拡張 (path normalization / sibling false positive / dirty gate / no-playable skip / pending nav cancel / slideshow mid-folder transition) |
| **P2-7 weak** header line 5 stale | header を `~1,500-1,700 行、2-3 commit` に修正、Step 表合計と整合 |
| **P3-3** 旧 filter UI disabled 記述残存 | line 64 / 151 / 447 / 463 を新仕様に修正 (filter UI 操作可 / state は consume / 関連コード位置の `要調査` 削除) |

### Codex review (= 2026-06-05、本体実装後の idx-keyed 状態 / async 経路指摘) と対応

| Codex 指摘 | 対応 |
|---|---|
| **P1** snapshot 有効化で idx-keyed ページ編集状態が stale | `activate_snapshot` は items を subset へ差し替えるが、`adjustment_page_params` / `mask_pages` / `conceal_pages` / `local_adjust_page_layers` / `local_adjust_pages` / `local_adjust_selected_layers` / `export_crop_page_settings` / `export_crop_pages` を remap せず、元フォルダの別画像の補正・マスクが subset の別 idx に乗っていた。**`App::clear_page_edit_state()` (idx-keyed ページ編集状態の正準セット clear) + `App::rehydrate_page_edit_state_for_current_items(prefix)` (clear + `load_folder` と同じ DB ロード) を新設**。`activate_snapshot` / `deactivate_snapshot` (at_origin 非検索経路) / `snapshot_return_to_list_view` の 3 箇所で対称に処理する。編集は `set_page_params` 等が DB に同期保存するので、解除時に DB から読み直せば snapshot 中の編集も復元される。child folder drill は `load_folder` 由来で既に hydrate 済み。**cross-folder 検索 view 由来 snapshot (= Ctrl+S/Ctrl+G、判定は `pre_snapshot_search_origin.is_some()`) は clear のみ**: subset が cross-folder で単一 prefix hydrate できず、かつ origin が検索前の実 current_folder なので prefix 配下の subset item だけ部分 hydrate される不整合を避ける (検索 view は元々ページ編集 overlay を出さない設計と整合)。**Ctrl+F (単一フォルダ構造フィルタ) は検索ではなく rehydrate 側** (`search_was_active` では gate しない。あれは Ctrl+F も true になり誤 clear + list 復帰との非対称を生む)。回帰テスト 4 件 (通常フォルダ activate で b の補正が subset に leak しない / deactivate で元 idx に復元 / Ctrl+G snapshot は clear のみ / Ctrl+F snapshot は rehydrate) |
| **P2** 削除時 `local_adjust_selected_layers` 未 shift | `remove_items_batch` は `local_adjust_page_layers` / `local_adjust_pages` を idx shift するが選択中レイヤー idx を残置 → 削除位置より後ろのページが選択状態を失う / 別ページへ古い選択が乗る。同じ shift マッピング + 残存 layer 数 clamp を追加。回帰テスト 2 件 |
| **P2 (async)** 非同期 ZIP/PDF 列挙をまたぐ snapshot leaf target 喪失 | `snapshot_load_and_open` で未展開 ZIP/PDF を開くとき、`load_folder` が非同期列挙を開始 → 直後の同期 target lookup が空振りし first playable に着地していた。**`DeferredFsReopen` に `target: Option<SnapshotTarget>` を追加** (Copy 外し Clone のみ)、列挙 pending 時は target を載せた deferred を `fs_nav_after_pdf_enumerate` にセット。`poll_zip_enumerate` / `poll_pdf_enumerate` 完了時に `resolve_snapshot_target_idx(target)` で対象 leaf を解決 (マッチしなければ従来どおり先頭着地に fallback)。target マッチロジックは sync/deferred 両経路で共有 helper に集約。`capture_fs_nav_holdover` 由来の nav lock + holdover が既存の folder-nav deferred と同じ機構で継続するので画面継続性も保たれる。回帰テスト 1 件 (`resolve_snapshot_target_idx` の各 leaf 種別解決) |

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
- **「★固定」中の filter 部分変更で再 snapshot** (= 現状は filter を変えても top-level 凍結だが、明示的に「再 snapshot」ボタンを出して現在 filter で取り直しできるようにする案)
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
| `global_search` ストリーミング状態 | `src/global_search.rs` の `is_searching()` (= P2-4) |
| Ctrl+F pending state | `src/app.rs` の `search_pending` 関連 (= dirty check 用) |
| Ctrl+S pending state | `src/app.rs` の `favsearch_pending` 関連 (= dirty check 用) |
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
| 2026-06-04 | Codex 2nd review (= 残 P1/P2/P3 一括反映): §4.6 SnapshotKey 厳密定義節追加 (= case-fold / separator / extended prefix / UNC / Archive 構造、P1-1 完全解決)。§4.5 lifecycle 3 段順序明示 + dirty/pending Ctrl+F/S の disable gate 追加 (P2-1 残 / P2-new 解決)。§4.2.1 `FilterState` を `rating_filter` のみに絞る + `SnapshotSourceLabel` 別途定義 (P2-FilterState 解決)。§4.6 terminal rules 節追加 (= ordering / no-playable skip / 末尾 stop + boundary hint、P1-3 残 解決)。Step 11 を `{ generation_id, current_idx, action_kind, media_policy }` に拡張 (P2-Step11)。Step 14 unit test 追加 6 件 (P2-6 残)。規模 ~1,560 行、commit 分割推奨 (P2-7)。header / 旧 filter UI disabled 記述削除 (P3-3) |
