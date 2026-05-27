# 隠蔽加工機能 + 統一エクスポート (Ctrl+E) 設計

汎用的な「隠蔽加工」ツールを「消しゴムツール」の双子サブシステムとして実装する。
モザイク・白塗り・黒塗り・ぼかしの 4 タイプを切り替えて使え、AI 画像投稿の修正から
個人情報マスキング、SNS スクショの顔隠しまで幅広くカバーする。

副次的効果として、隠蔽加工・消しゴム・色補正の編集結果を **元画像と同じフォルダに元形式で**
保存する汎用エクスポート機能 (`Ctrl+E`) を併せて導入する。

関連: [docs/preset-and-adjustment.md](preset-and-adjustment.md), [src/ui_erase.rs](../src/ui_erase.rs),
[src/mask_db.rs](../src/mask_db.rs)

---

## 1. 背景・目的

画像の一部を隠す処理を統一インターフェースで提供する。主要ユースケース:

- AI 生成イラストの修正 (モザイク、投稿サイトの規定対応はユーザー判断)
- スクリーンショットの顔・名前・住所などの個人情報マスキング (ぼかし / 黒塗り)
- 印刷物の見栄え調整 (白塗りで名前タグを消す等)
- 形状を残したまま内容を隠したい場合 (シルエットモザイク)

mImageViewer は既に消しゴムツールで「マスク描画 → 結果を `fs_cache` に反映 → `Ctrl+S` で保存」
というフローを持っているため、隠蔽加工も同じ枠組みに収めれば学習コストが低い。

同時に、現状の `Ctrl+S` (キャプチャ保存) は `capture_output_dir` 設定で指定したフォルダに
連番保存する設計で、**元画像のメタデータ (AI prompt / EXIF) を破棄してしまう**。
AI 画像整理では「元画像と同じフォルダに、元と同じ形式で、AI prompt を残して」保存したい
ニーズがあるため、汎用エクスポート機能を別ホットキー (`Ctrl+E`) として用意する。

## 2. 業界慣行の調査結果 (モザイクタイプに限定)

モザイクタイプの初期値設計のため、国内の画像投稿サイトで広く採用されている慣行を
技術的事実として整理する。
**ただしどのサイトの基準も時期によって変わり得るため、本アプリは「この設定なら投稿可能」
といった判定や誘導を行わない。**判断は利用者に委ねる前提で、設計はあくまで
「これらの慣行をユーザーが選択肢として表現できる」ことを目指す。

| 項目 | 慣行 |
| --- | --- |
| タイルサイズ | 長辺 ÷ 100、下限 4px の式が広く知られる |
| タイル整列 | 画像原点 (0,0) 基準の固定グリッド (マスク形状で切り抜かない) |
| タイル色 | タイル正方形内の **全画素** の平均 RGB (Photoshop の「モザイク」フィルタ等と同じ) |
| 部分カバー | 完全不透明が一般的。透過モザイク・薄ぼかしを禁止する規約改定例あり |
| レイヤー扱い | 画像本体にフラット化 (別レイヤーで上載せ不可、というルール例あり) |

これらは「ある時点の規約から導かれた数値・運用」であり、**永続的な保証ではない**。
設計上はこれらの慣行を「ありがちな初期値」として採用するが、UI 上で「この設定はどこそこの
基準に合う」と表示することはしない。

## 3. アーキテクチャ

### 3.1 表示パイプライン (キャッシュ階層) の拡張

現状: `adjustment_cache > ai_upscale_cache > fs_cache`

新: **`conceal_cache > adjustment_cache > ai_upscale_cache > fs_cache`**

`conceal_cache[idx]` = 「adjustment_cache (なければ ai_upscale_cache、なければ fs_cache) を
入力にして、現在のマスク + 隠蔽タイプ + 各パラメータで隠蔽効果を焼き込んだ結果」。

これにより `Ctrl+S` (既存キャプチャ保存) は display pixels を読むので自動的に隠蔽加工込みで
保存される。新規コードはほぼ不要。

### 3.2 新規ファイル

| ファイル | 役割 | 想定行数 |
| --- | --- | --- |
| `src/ui_conceal.rs` | モード状態機械、ツール処理、パネル UI、合成処理 (4 タイプ) | ~2500 |
| `src/conceal_db.rs` | SQLite 永続化 (mask_db のコピペベース、スロット機構含む) | ~400 |
| `src/conceal_compose.rs` | 4 タイプ別の合成アルゴリズム (mosaic / white / black / blur) | ~600 |
| `src/export_dialog.rs` | `Ctrl+E` のエクスポートダイアログ UI | ~400 |
| `src/save_with_metadata.rs` | JPEG / PNG / WebP のメタデータ保持エンコード | ~500 |

### 3.3 既存モジュールからの再利用 (共有)

消しゴムとツールパレットを **完全一致 (8 種)** させる方針なので、`mask_db.rs` を
`Shape` enum 対応に拡張し、両ツールから共有する:

```rust
// src/conceal_db.rs / src/ui_conceal.rs / src/ui_erase.rs から:
use crate::mask_db::{Shape, LineKind, rasterize_shape_into, scanline_fill_polygon};
use crate::vector_edit::{HoverTarget, hit_test, cursor_icon_for, draw_handles, apply_drag};
```

| 共有要素 | 出元 | 共有理由 |
| --- | --- | --- |
| `Shape` enum (Line / Rect / Ellipse) | `mask_db.rs` (既存 LineObject から拡張) | 全 8 ツールが生成するベクタオブジェクトの統一表現 |
| `LineKind` enum (Vert / Horiz / Diag) | `mask_db.rs` | Line variant 内で使用、ツールパレット統一に伴い共有 |
| `rasterize_shape_into` 関数 (新規) | `mask_db.rs` | Shape variant ごとのラスタライズ (Line/Rect=多角形、Ellipse=楕円方程式) |
| `scanline_fill_polygon` 関数 | `mask_db.rs` (既存) | 筆 / 囲み / Line / Rect 共通の多角形塗り |
| `scanline_fill_ellipse` 関数 (新規) | `mask_db.rs` | Ellipse variant 専用 (~50 行) |
| `vector_edit.rs` (新規モジュール) | 新規 | ハンドル操作・カーソル選択・ドラッグ状態機械 (両ツール共通) |

ツールハンドラ (どのストローク → どのマスクに書き込むか) は分岐するので
`ui_conceal.rs` / `ui_erase.rs` 側に独自実装するが、ラスタライズと選択操作は共有関数に
委譲する。

#### 既存消しゴムマスクの後方互換性 (リリース済みデータマイグレーション)

mask_db は既にリリース済みで、旧 `LineObject` の素 JSON が DB / サイドカーに
保存されている。CLAUDE.md「永続データ・スキーマ変更時の判断」§リリース済み に従い、
以下のマイグレーション対応が必須:

- `Shape` の `Deserialize` を **タグ付き (`{"type": "rect", ...}`) と タグなし旧 LineObject
  (`{"kind": "Diag", "p0": ..., "p1": ..., "thickness": ...}`) の両方を読める** カスタム実装にする
- `Serialize` は常にタグ付き形式で書く (`Line` も `{"type": "line", ...}` 形式に)
- 一度新版で開いて保存し直すと自動的に新形式に移行
- 旧 JSON が読めるテストを `mask_db::tests` に追加 (released データの互換性検証)

### 3.4 App 状態追加

```rust
// モード状態
conceal_mode: bool,
conceal_tool: ConcealTool,                  // Select / Brush / Lasso / VertLine / HorizLine / Line
                                            // = EraseTool と完全同型 (alias でも独立 enum でも可)
conceal_paint_mode: bool,                   // 描画 / 消去 トグル
conceal_spread_ctx: Option<ConcealSpreadCtx>,  // 見開き Double 時の左右

// マスク (現在の編集対象 idx、全タイプ共通)
conceal_mask: Option<Vec<bool>>,
conceal_mask_size: [usize; 2],
conceal_vectors: Vec<LineObject>,           // LineKind::{Vert, Horiz, Diag} 全種類

// 描画ツール設定 (グローバル、settings.json に永続化)
conceal_brush_radius: f32,
conceal_line_width: f32,

// 隠蔽パラメータ (グローバル、settings.json に永続化)
conceal_type: ConcealType,                  // Mosaic / WhiteFill / BlackFill / Blur
// モザイク用
conceal_mosaic_tile_mode: TileSizeMode,     // LongEdgeRatio(0.25..=5.0, 0.25刻み) or FixedPx(4..=200)
conceal_mosaic_boundary: MosaicBoundary,    // Opaque / Translucent / MaskShape
// 白塗り / 黒塗り用
conceal_fill_opacity_percent: u8,           // 1..=100、1% 刻み (両 fill タイプで共有)
conceal_fill_edge: FillEdge,                // Sharp / Feathered
// ぼかし用
conceal_blur_radius_px: f32,                // 5..=100、1px 刻み、デフォルト 20px
conceal_blur_mode: BlurMode,                // AsMask / ExtendByRadius / InsideOnly
conceal_blur_feather: bool,                 // 境界フェード ON/OFF

// パラメータプリセット 4 スロット (グローバル、settings.json)
// 上記パラメータ一式を ConcealPreset { name, conceal_type, ... } として 4 つ保存
// (詳細は §8.3)

// キャッシュ
conceal_cache: HashMap<usize, FsCacheEntry>,
conceal_base_cache: HashMap<usize, Arc<ColorImage>>,  // 右 Ctrl プレビュー用

// Undo (マスク編集のみ。パラメータ変更 / プリセット適用は Undo 対象外)
conceal_undo_stack: VecDeque<ConcealSnapshot>,
conceal_redo_stack: VecDeque<ConcealSnapshot>,
conceal_last_undo_at: Option<Instant>,

// バッジ判定 (タイプ問わず、マスクがあるページ)
conceal_pages: HashSet<usize>,
```

**設計判断: すべてのパラメータ (隠蔽タイプ、タイル倍率、不透明度、ぼかし半径、
境界モード等) は「グローバル設定」**。ページ個別にはしない。これで「ツールの好み」と
「ページごとのマスク内容」が分離される。複数の好みを保持したいときはプリセット 4 スロット
を使う (§8.3)。

```rust
enum ConcealType { Mosaic, WhiteFill, BlackFill, Blur }
enum MosaicBoundary { Opaque, Translucent, MaskShape }  // モザイク専用
enum FillEdge { Sharp, Feathered }                       // 白塗り / 黒塗り専用
enum BlurMode { AsMask, ExtendByRadius, InsideOnly }     // ぼかし専用
```

## 4. キー割り当て

### 4.1 フルスクリーン (隠蔽加工モード外)

| キー | 動作 |
| --- | --- |
| `Ctrl+M` | 隠蔽加工モード入口/退場 (M はルーペで予約) |
| `Ctrl+S` | キャプチャ保存 (既存、`capture_output_dir` + `capture_format`) |
| `Ctrl+E` | エクスポート (新規、ダイアログ付き、元フォルダ + 元形式既定) |

### 4.2 隠蔽加工モード中

| キー | 動作 |
| --- | --- |
| `Esc` | モード終了 (マスク保持、DB 書込) |
| `S` / `B` / `L` / `I` / `V` / `H` / `R` / `O` | ツール切替 (選択 / 筆 / 囲み / 直線 / 縦線 / 横線 / 矩形 / 楕円) — 消しゴムと完全同一 |
| `D` / `F` | 描画 / 消去 |
| `1` / `2` / `3` / `4` | パラメータプリセット 1〜4 を適用 |
| `T` | 隠蔽タイプを順次切替 (Mosaic → WhiteFill → BlackFill → Blur → Mosaic …) |
| Ctrl+wheel | (Mosaic 時) タイル倍率 ±0.25x / (Blur 時) ぼかし半径 ±5px / (Fill 時) 不透明度 ±5% |
| `Ctrl+Z` / `Ctrl+Shift+Z` | Undo / Redo (マスク編集のみ) |
| 矢印 / Ctrl+矢印 | マスク平行移動 (1px / 10px、消しゴムと同じ) |
| `[` / `]` / Ctrl+`[` / Ctrl+`]` | マスク回転 ±0.1° / ±1° (消しゴムと同じ) |

**スロット (マスク 1/2) は隠蔽加工モード内のパネルボタンからのみ操作可能** (保存 / ロード)。
消しゴムの F7/F8 viewing-mode quick apply に相当する機能は持たせない。
理由:
- viewing-mode F7/F8 を隠蔽加工と消しゴム両方で持つとキー衝突
- 主用途 (差分画像生成) は「画像 A でマスク作成 → スロットに保存 → 画像 B でモード入り → スロットからロード」のフローで、いずれにせよモード入退場するため viewing-mode quick apply の利得が小さい
- 必要が出てきたら v2 で Shift+F7/F8 等で追加可能

実装時に他の `Ctrl+M` / `Ctrl+E` 割当てが無いことを `ui_fullscreen.rs` で確認すること。
モード内の `M` / `1`〜`4` も Ctrl/Shift なしで使うので、テキスト入力可能なフォーカスが
パネル内にあるとき (プリセット名編集中) は通常入力としてスルーするよう注意 (消しゴムも
同じ問題に対処済み、`App::ime_input_active` や text edit response の has_focus 判定を使う)。

## 5. UI 設計 (左サイドパネル)

```
┌─ 隠蔽加工 ───────────────────────────────────┐
│ ツール:                                          │
│  [選] [筆] [囲] [直] [縦] [横] [矩] [楕]         │  ← S/B/L/I/V/H/R/O キー (消しゴムと同一)
│  ● 描画   ○ 消去                                │  ← D/F キー
│  太さ: ──●─── 12px                              │
│                                                  │
│ 処理タイプ:                                      │
│  ● モザイク   ○ 白塗り                          │  ← T キーで順次切替
│  ○ 黒塗り     ○ ぼかし                          │
│                                                  │
│ ┌─── (モザイク選択時のみ表示) ────────────────┐│
│ │ タイルサイズ:                                  ││
│ │  ● 長辺比率モード                              ││
│ │     倍率: ──●─── 1.00x  (= 14px @ 1400px 長辺)││
│ │     ※ 画像長辺の 1/100 を 1 倍、最小 4px       ││
│ │  ○ 固定 px モード                              ││
│ │     サイズ: ──●─── 16px                        ││
│ │ 境界処理:                                      ││
│ │  ● マスクを含むタイルを不透明で描画           ││
│ │  ○ マスクを含むタイルをマスクの                ││
│ │     割合に応じた不透明度で描画                 ││
│ │  ○ マスクの形に沿って描画                      ││
│ └────────────────────────────────────────────────┘│
│                                          │
│ ┌─── (白塗り / 黒塗り選択時のみ) ──────┐│
│ │ 不透明度: ──●─── 100%   (1% 刻み)    ││
│ │ 境界処理:                              ││
│ │  ● マスクの形をシャープに描画          ││
│ │  ○ マスクの形に境界フェードを掛けて    ││
│ └────────────────────────────────────────┘│
│                                          │
│ ┌─── (ぼかし選択時のみ) ───────────────┐│
│ │ ぼかし半径: ──●─── 20px              ││
│ │ ぼかしモード:                          ││
│ │  ● マスク通り (外画素を参照してぼかす)││
│ │  ○ マスク拡張 (半径ぶん広げて描画)     ││
│ │  ○ マスク内のみ (外画素を参照しない)   ││
│ │ ☐ 境界フェードを掛ける                 ││
│ └────────────────────────────────────────┘│
│                                          │
│ プリセット:                              │
│  [1: 投稿用 1x  ] [適用] [💾]            │  ← 1〜4 キーで適用
│  [2: 投稿用 2x  ] [適用] [💾]            │
│  [3: 強め       ] [適用] [💾]            │
│  [4: 整理用ぼかし] [適用] [💾]           │
│                                          │
│ マスクスロット:                          │
│  スロット 1: [保存] [ロード]             │  ← 差分画像生成用
│  スロット 2: [保存] [ロード]             │
│                                          │
│ [↶ Undo] [↷ Redo]                       │
│ [マスク全削除]                           │
└──────────────────────────────────────────┘
```

**UI 上で「この設定はどこそこの基準を満たす」という表示はしない**。
タイルサイズの実寸は `(= {N}px @ {long_edge}px 長辺)` のように **px 数値のみ** を出し、
判定は利用者に委ねる。境界処理モードにも「配信用」「配信不可」といったラベルは付けない。

「処理タイプ」で選んだものに応じてパラメータ群が条件表示される (egui の `if` 分岐で
該当タイプのコントロールだけ描画)。これでパネルが必要以上に縦長にならない。

### 5.1 モード説明のスタイル

ラジオボタンのラベルは **処理内容を具体的に書く**。「強い隠蔽」「投稿用」のような曖昧
あるいは用途判定的な表現は避ける。利用者が「自分の用途に合うか」を判断できるように
するため。

| 内部名 | UI ラベル (具体的な処理) |
| --- | --- |
| `Opaque` | マスクを含むタイルを不透明で描画 |
| `Translucent` | マスクを含むタイルをマスクの割合に応じた不透明度で描画 |
| `MaskShape` | マスクの形に沿って描画 (マスク内の各画素をその画素が属するタイルの平均色で塗る) |

マニュアルでも同じ書き方をする。「強い」「弱い」「綺麗」のような評価語ではなく、
**処理の機構**を書くこと。これがないと利用者は自分の用途 (投稿先の規定など) に
合っているかを判断できない。

## 6. ツール 8 種仕様 (消しゴムと完全同一)

両ツール (消しゴム / 隠蔽加工) で同じ 8 種パレットを採用。`Shape` enum / rasterizer / UI
パネル骨子を共有することで実装重複を排除。順序は **描画系 → 拘束系 → bbox 系** の流れに
そろえる:

| ツール | キー | 動作 | データ反映先 |
| --- | --- | --- | --- |
| 選択 | S | ベクタ本体を hit test、ハンドルドラッグで編集 (`vector_edit.rs` 参照) | `Shape` 編集 |
| 筆 | B | 半径 `brush_radius`、ストロークで `scanline_fill_polygon` ラスタライズ | ビットマップ |
| 囲み | L | ポリゴン → `scanline_fill_polygon` | ビットマップ |
| 直線 | I | `Shape::Line { p0, p1, thickness }`、両端ドラッグで端点編集 | `Shape` vec |
| 縦線 | V | `Shape::Line { kind: Vert, .. }`、生成時に縦に拘束 | `Shape` vec |
| 横線 | H | `Shape::Line { kind: Horiz, .. }`、生成時に横に拘束 | `Shape` vec |
| 矩形 | R | `Shape::Rect { center, half_w, half_h, rotation_rad }`、corner→corner ドラッグで作成 | `Shape` vec |
| 楕円 | O | `Shape::Ellipse { center, rx, ry, rotation_rad }`、内接 bbox ドラッグで作成 | `Shape` vec |

ベクタオブジェクト編集の挙動 (ハンドル方式、Ctrl/Shift/Alt 修飾子) は消しゴムと完全一致。
選択ツール中は未選択 Shape も編集用アウトラインで表示し、`Add` と `Subtract` を色分けする。
`Subtract` は合成済みマスクでは透明になるため、このアウトラインはクリック対象の存在を伝える
補助表示として `vector_edit.rs` 側で共有する。コード差分を最小化することで、将来両ツールに
同時に修正を入れる際のバグ機会が減る。

### 6.1 用途別の使い分け

| シーン | 主に使うツール |
| --- | --- |
| AI イラスト R18 修正 | 筆、囲み、矩形 |
| スキャン書籍のゴミ取り (消しゴム用途) | 縦線、横線、直線、筆 |
| 写真のゴミ・映り込み消去 (消しゴム用途) | 矩形、楕円、筆 |
| 個人情報マスキング (顔・名前・住所) | 矩形 (テキストブロック)、楕円 (顔) |
| ナンバープレート / 看板の隠蔽 | 矩形、直線 |

**消しゴム側にも矩形・楕円を追加**: 写真加工でのゴミ除去 (映り込みオブジェクト) など
スキャン以外の用途で必要。データモデル共通化のメリットも大きい。

### 6.2 データモデル (Shape enum)

```rust
// mask_db.rs (両ツール共有)
pub enum ShapeOp { Add, Subtract }

pub enum Shape {
    Line { op: ShapeOp, kind: LineKind, p0: (f32, f32), p1: (f32, f32), thickness: f32 },
    Rect { op: ShapeOp, center: (f32, f32), half_w: f32, half_h: f32, rotation_rad: f32 },
    Ellipse { op: ShapeOp, center: (f32, f32), rx: f32, ry: f32, rotation_rad: f32 },
}
```

JSON シリアライズ: `#[serde(tag = "type")]` で `{ "type": "rect", ... }` 形式。
`op` は `add` / `subtract`。`add` は省略され、既存 JSON のように `op` が無い場合も
`add` として読む。`subtract` のみ `{ "op": "subtract" }` を書く。
**既存リリース済み消しゴムマスク (`LineObject` の素 JSON) との後方互換性**:

- `serde(untagged)` 階層化で「タグ付き Shape」「タグなし旧 `LineObject`」の両方を読める
  `Deserialize` を実装
- `Serialize` は常にタグ付き形式で書く (一度新版で開いて保存し直すと自動的に新形式に移行)
- 旧 JSON が読めるテストを `mask_db::tests` に追加 (released データの互換性検証)

CLAUDE.md「永続データ・スキーマ変更時の判断」§リリース済みのため、上記マイグレーション
が必須。

### 6.3 ラスタライザ

`mask_db.rs` に `rasterize_shape_into(shape, mask, w, h)` を新設し、`Shape` の各
variant に対応:

- `Line`: 既存 `rasterize_vectors_into` (corners → 多角形塗り) を再利用
- `Rect`: 4 corners 計算 → `scanline_fill_polygon`
- `Ellipse`: 軸並行楕円なら走査線で in/out 判定、回転楕円なら回転逆変換後の楕円方程式判定
  → 専用 `scanline_fill_ellipse` 関数を新設 (~50 行)

`compose_mask` (ビットマップ + Shape 群の合成) は、ビットマップを下地にして Shape 群を
作成順に適用する。`Add` はマスクを足し、`Subtract` はそれ以前の結果を削る。

## 7. タイプ別の合成アルゴリズム

### 7.1 Mosaic タイプ — 3 つの境界モード

```rust
enum MosaicBoundary {
    /// マスクを含むタイルを不透明で描画 (デフォルト)
    /// coverage > 0.0 のタイルは矩形全体を平均色で塗りつぶす
    Opaque,
    /// マスクを含むタイルをマスクの割合に応じた不透明度で描画
    /// alpha = coverage × 255 で元画像と alpha ブレンド
    Translucent,
    /// マスクの形に沿って描画
    /// マスク内の各画素を、その画素が属するタイルの平均色で塗る (画素単位、不透明)
    MaskShape,
}
```

UI ラベルは内部名ではなく **処理内容を具体的に書いた文**を使う (§5.1 表を参照)。
「強い隠蔽」「投稿用」「綺麗」のような評価語・用途判定的な表現は使わない。

### 7.2 Mosaic: タイル平均色の事前計算

```rust
// 画像 (0,0) 原点固定グリッド。タイルが画像端を超えるときは画像内領域だけで平均。
fn compute_tile_means(src: &ColorImage, tile: u32) -> Vec<Color32> {
    // 並列化: rayon で row-band 単位
    // 各タイル正方形の全画素 RGB の平均、alpha は 255 固定
}
```

### 7.3 Mosaic: Opaque / Translucent (タイル単位描画)

```rust
for (tx, ty, rect) in tile_iter(src.size(), tile) {
    let coverage = mask_coverage_in_rect(mask, mask_size, rect);  // 0.0..=1.0
    if coverage == 0.0 { continue; }
    let alpha = match mode {
        Opaque => 255,
        Translucent => (coverage * 255.0).round() as u8,
        _ => unreachable!(),
    };
    let color = tile_means[ty * cols + tx];
    blend_rect(&mut out, rect, color, alpha);  // 元画像と alpha ブレンド
}
```

### 7.4 Mosaic: MaskShape (画素単位描画)

```rust
for y in 0..h {
    for x in 0..w {
        if !mask[y * mw + x] { continue; }
        let (tx, ty) = (x / tile as usize, y / tile as usize);
        out[y * w + x] = tile_means[ty * cols + tx];  // 不透明で置換
    }
}
```

タイル色は通常通りタイル平均だが、描画範囲がマスク画素そのもの。
色はピクセル化されているがシルエットはマスクの形そのまま、という見え方になる。

### 7.5 Mosaic: タイルサイズ計算 (2 モード)

タイルサイズは **長辺比率モード** と **固定 px モード** の 2 方式から選択:

```rust
enum TileSizeMode {
    LongEdgeRatio(f32),    // 0.25..=5.0、0.25 刻み (画像長辺に対する比率)
    FixedPx(u32),          // 4..=200、1 刻み (画像サイズによらず固定 px)
}

fn compute_tile_size(image_long_edge: u32, mode: TileSizeMode) -> u32 {
    match mode {
        TileSizeMode::LongEdgeRatio(multiplier) => {
            let base = ((image_long_edge as f32 / 100.0).round().max(4.0)) as u32;
            ((base as f32 * multiplier).round() as u32).max(4)  // 二重に 4px floor
        }
        TileSizeMode::FixedPx(px) => px.max(4),                  // 4px floor
    }
}
```

#### 各モードの用途

| モード | 使い分け |
| --- | --- |
| LongEdgeRatio | 「画像によらず同じ粗さの見た目」が欲しい (隠蔽の強さが画像サイズに比例) |
| FixedPx | 「複数画像で見た目を揃えたい」「規定が px で決まっている」「長辺によらず常に 16px タイル」 |

#### 長辺比率モードの実寸表 (0.25 刻み、抜粋)

| 長辺 | 0.25x | 0.5x | 1.0x | 2.0x | 5.0x |
| --- | --- | --- | --- | --- | --- |
| 400px | 4px | 4px | 4px | 8px | 20px |
| 1400px | 4px | 7px | 14px | 28px | 70px |
| 1920px | 5px | 10px | 19px | 38px | 95px |
| 4000px | 10px | 20px | 40px | 80px | 200px |

UI ヒント:
- 長辺比率モード時: スライダー横に `(= {tile}px @ {long_edge}px 長辺)` を表示 (**px 数値のみ**)
- 長辺比率モード時: 計算方法の説明「画像長辺の 1/100 を 1 倍、最小 4px に補正」を併記
- 固定 px モード時: 単純に「{px}px」表示
- **「どの倍率がどの基準に合う」という表示はしない** (基準は時期で変わる、判断は利用者に委ねる)

#### プリセットへの保存

`ConcealPreset` に `mosaic_tile_mode: TileSizeMode` を持たせる。
これで「比率 1x プリセット / 固定 8px プリセット / 固定 16px プリセット」のような
使い分けが可能。

### 7.6 WhiteFill / BlackFill タイプ

マスク領域を単色 (白 = `#FFFFFF` または 黒 = `#000000`) で塗りつぶす。
**不透明度スライダー** (1〜100%、1% 刻み、デフォルト 100%) で全体の透明度を調整可能。
**境界モード**は `FillEdge::{Sharp, Feathered}` の 2 種類。

```rust
enum FillEdge {
    /// マスクの形をシャープに描画 (鋭い境界)
    Sharp,
    /// マスクの形に境界フェードを掛けて描画
    /// 境界から内側へ feather_radius_px 以内の画素で alpha を線形補間
    Feathered,
}
```

```rust
fn compose_solid_fill(
    src: &ColorImage,
    mask: &[bool], mw: usize, mh: usize,
    color: Color32,
    opacity_percent: u8,       // 1..=100
    edge: FillEdge,
) -> ColorImage {
    let base_alpha = (opacity_percent as f32 * 2.55).round() as u8;  // 1% → ~2.55 alpha
    let mut out = src.clone();
    
    let edge_alpha_map: Option<Vec<u8>> = match edge {
        FillEdge::Sharp => None,
        FillEdge::Feathered => Some(compute_edge_feather_alpha(mask, mw, mh, FEATHER_RADIUS_PX)),
        // FEATHER_RADIUS_PX は固定値 (例: 8px) または将来スライダー化
    };
    
    for y in 0..src.height() {
        for x in 0..src.width() {
            if !mask[y * mw + x] { continue; }
            let alpha = match &edge_alpha_map {
                None => base_alpha,
                Some(amap) => (base_alpha as u16 * amap[y * mw + x] as u16 / 255) as u8,
            };
            blend_pixel(&mut out, x, y, color, alpha);
        }
    }
    out
}
```

`compute_edge_feather_alpha` はマスクの境界からの距離変換 (distance transform) で
**境界画素は 0、内側 FEATHER_RADIUS_PX px までは線形に 255 まで上昇** する alpha マップを
生成する。境界フェードは「全体不透明度」とは独立で乗算合成されるため、両方使うと
「半透明 + 境界フェード」の効果になる。

不透明度 100% + Sharp = 完全な単色塗り (個人情報マスキングの定番)。
不透明度 50% + Sharp = 半透明オーバーレイ (内容が透けて見える整理用途)。
不透明度 100% + Feathered = エッジが滑らかな塗り (印刷物の見栄え調整)。

### 7.7 Blur タイプ

マスク領域に Gaussian ぼかしを掛ける。**ぼかし半径** (5〜100px、1px 刻み、デフォルト 20px) と
**ぼかしモード** (3 種) + **境界フェードチェック** (オプション)。

```rust
enum BlurMode {
    /// マスク通り: 鋭い境界、Gaussian カーネルは元画像 (マスク外も含む) から sampling
    /// 標準的なぼかし。隣接画素の色が混ざりこむ
    AsMask,
    /// マスク拡張: マスクをぼかし半径ぶん膨張させて描画
    /// 元のマスクの外側にも (半径ぶん) ぼかし結果が広がる
    /// オブジェクトの輪郭を曖昧にしたいときに有効
    ExtendByRadius,
    /// マスク内のみ: Gaussian カーネルがマスク外画素を参照しない (= 鏡像/0 で外挿)
    /// 隣接する別オブジェクトの色がぼかしに漏れ込まない
    /// 顔ぼかしに隣接した名前タグの色が混ざる、などの事故を防ぐ
    InsideOnly,
}
```

#### 共通: マスク bbox + 余裕

ぼかしは全画像走査だと重い (4K で sigma=20 のとき ~300ms)。実装上はマスクの bounding box を
計算し、`blur_radius * 3` ピクセル拡張して、その bbox 内だけ Gaussian を計算する。
これで多くの実用ケース (顔ぼかしなど局所マスク) で 50〜150ms に収まる。

```rust
fn compose_blur(
    src: &ColorImage,
    mask: &[bool], mw: usize, mh: usize,
    radius_px: f32,
    mode: BlurMode,
    feather_boundary: bool,
) -> ColorImage {
    let working_mask = match mode {
        BlurMode::ExtendByRadius => dilate_mask(mask, mw, mh, radius_px as u32),
        _ => mask.to_vec(),
    };
    let bbox = mask_bbox(&working_mask, mw, mh)
        .expand(radius_px as u32 * 3)
        .clip_to(src.size());
    
    let sampling = match mode {
        BlurMode::InsideOnly => GaussianSampling::MaskedOnly(&working_mask, mw, mh),
        _ => GaussianSampling::Whole,
    };
    let blurred_bbox = gaussian_blur_separable(src, bbox, radius_px, sampling);
    
    let edge_alpha_map = if feather_boundary {
        Some(compute_edge_feather_alpha(&working_mask, mw, mh, FEATHER_RADIUS_PX))
    } else {
        None
    };
    
    let mut out = src.clone();
    for (x, y) in bbox.iter_pixels() {
        if !working_mask[y * mw + x] { continue; }
        let alpha = edge_alpha_map.as_ref()
            .map(|m| m[y * mw + x])
            .unwrap_or(255);
        blend_pixel(&mut out, x, y, blurred_bbox.at(x, y), alpha);
    }
    out
}
```

- `gaussian_blur_separable`: 横方向 + 縦方向の 2-pass で O(W*H*radius) ではなく O(W*H) で済む
- `GaussianSampling::MaskedOnly`: kernel が外を参照したら 0 寄与 + 正規化重みも縮める
- `dilate_mask`: 半径 r の円形 SE で膨張 (rayon 並列、bbox 内のみ計算)

#### ぼかしモードの使い分け

| モード | 用途例 | 視覚的特徴 |
| --- | --- | --- |
| AsMask | 一般的なぼかし (Photoshop の標準動作) | マスク境界がシャープ、内容が隣接画素と混ざってブレンド |
| ExtendByRadius | オブジェクトの輪郭そのものを曖昧にしたい (顔の輪郭を残したくない) | マスクの外側にもぼけが広がる |
| InsideOnly | 隣接する別要素の色を混ぜたくない (顔ぼかしに名前タグの文字色が漏れない) | マスク境界の内側がやや暗くなりやすい (kernel 寄与が縮むため) |

#### 性能上の注意

**Codex P1 指摘により最初から worker thread 化**。`docs/ui-responsiveness.md` §4
チェックリストに従う。

- 推定コスト判定: `estimated_cost_ms = (bbox_area_px / 1e6) * (radius_px / 20.0) * 10.0`
  程度の概算 (実測でチューニング)
- 閾値 80ms 未満なら同期 (Mosaic/Fill と同じ流れで `conceal_cache` に書く)、
  80ms 以上なら worker:
  ```rust
  if estimated_cost_ms > 80 {
      spawn_blur_worker(idx, ...);  // ConcealBlurPending { cancel, rx } パターン
  } else {
      compose_blur_sync(idx, ...);
  }
  ```
- `ConcealBlurPending` は `ai_upscale_pending` と同形態:
  ```rust
  struct ConcealBlurPending {
      cancel: Arc<AtomicBool>,
      rx: mpsc::Receiver<ConcealBlurResult>,
      target_idx: usize,
      generation: u64,  // 結果到着時に generation が変わっていたら破棄
  }
  ```
- cancel タイミング: フォルダ移動 / モード退場 / generation bump / fullscreen idx 移動
- 統合テストでキャンセル/進捗を確認 (Codex P1 指摘)

## 8. 永続化

### 8.1 conceal.db (SQLite)

```sql
CREATE TABLE conceal_entries (
    page_path     TEXT PRIMARY KEY,    -- 通常ページキー or "__slot_1" / "__slot_2"
    bitmap_w      INTEGER NOT NULL,
    bitmap_h      INTEGER NOT NULL,
    bitmap_data   BLOB NOT NULL,    -- 1bit/pixel + deflate (mask_db と同じ)
    vectors_json  TEXT NOT NULL DEFAULT '[]'
);
```

**DB に保存するのはマスク (ビットマップ + ベクタ) のみ**。隠蔽タイプ・各種パラメータは
すべてグローバル設定 (`settings.json`) または プリセット (§8.3) に保存される。
ページとパラメータが疎結合になるため、「同じマスクで異なるパラメータの結果を Ctrl+E で
複数保存」のワークフローが自然に成立する。

`settings.json` に追加するフィールド:

```rust
struct Settings {
    // …既存…
    
    // 隠蔽加工の現在状態 (グローバル、終了時に保持されてアプリ再起動後も継続)
    conceal_type: ConcealType,                  // デフォルト Mosaic
    conceal_mosaic_tile_mode: TileSizeMode,     // デフォルト LongEdgeRatio(1.0)
    conceal_mosaic_boundary: MosaicBoundary,    // デフォルト Opaque
    conceal_fill_opacity_percent: u8,           // デフォルト 100
    conceal_fill_edge: FillEdge,                // デフォルト Sharp
    conceal_blur_radius_px: f32,                // デフォルト 20.0
    conceal_blur_mode: BlurMode,                // デフォルト AsMask
    conceal_blur_feather: bool,                 // デフォルト false
    
    // 描画ツール (消しゴムと同じ扱い)
    conceal_brush_radius: f32,                  // デフォルト 12.0
    conceal_line_width: f32,                    // デフォルト 12.0
    
    // パラメータプリセット 4 スロット (§8.3)
    conceal_presets: [Option<ConcealPreset>; 4],
}
```

`page_path` のキー規則は mask_db と同一 (`App::page_path_key(idx)` で生成、
ZIP/PDF 含めて一意化済み)。

### 8.2 マスクスロット機構 (差分画像生成サポート)

差分画像生成 (同一マスクを複数画像に適用) のため、消しゴムと同じ 2 スロット機構を持つ。

- キー: `__slot_1` / `__slot_2` (`conceal_db::slot_key(slot: u8) -> String` で生成)
- API (mask_db のスロット API と同じシグネチャ):
  - `get_slot_full(slot) -> Option<(bitmap, vectors, w, h)>`
  - `set_slot(slot, bitmap, vectors, w, h)`
  - `delete_slot(slot)`
- 機能セット: **保存 / ロード のみ** (消しゴムにある F7/F8 viewing-mode quick apply は持たない、§4.2 参照)
- スロット内容: ビットマップ + ベクタ (`LineKind::{Vert, Horiz, Diag}` 全種類)
- **隠蔽タイプや各パラメータは保存しない** (グローバル設定 + プリセットで管理)
- パネル UI: 「マスクスロット 1 [保存] [ロード]」「マスクスロット 2 [保存] [ロード]」
- ロード時の挙動: スロットのマスク内容で **現在のマスクを差し替える** (OR マージしない)。
  消しゴムと同じ仕様で、取り違えロードを Ctrl+Z で戻せる + 過剰マスクを防ぐ
- 解像度差: スロット保存元と適用先で画像サイズが違う場合、`get_full` がロード時に
  画像サイズへ nearest-neighbor でリスケール (`mask_db::get_full` と同じ挙動)
- バッジ判定への影響: `conceal_pages` にはスロットキー (`__slot_*`) を **含めない**
- サイドカー: スロットは中央 DB のみで管理、サイドカーには書かない (= フォルダ依存しない)

### 8.3 パラメータプリセット 4 スロット

「サイズ違いで複数バージョン保存」「R18 用 / 整理用で使い分け」のために、
隠蔽タイプ + 各パラメータ一式を 4 つまで名前付きで保存できる。VST のスロットに倣う設計。

```rust
struct ConcealPreset {
    name: String,                          // ユーザー編集可能、空なら "プリセット N"
    conceal_type: ConcealType,
    mosaic_tile_mode: TileSizeMode,        // LongEdgeRatio(f32) or FixedPx(u32)
    mosaic_boundary: MosaicBoundary,
    fill_opacity_percent: u8,              // 1..=100
    fill_edge: FillEdge,
    blur_radius_px: f32,
    blur_mode: BlurMode,
    blur_feather: bool,
}

Settings.conceal_presets: [Option<ConcealPreset>; 4]
```

#### UI と操作

パネルのプリセット行 (§5 モック参照):
- `[名前]` ボタン: クリックで適用、ダブルクリックで名前編集 (テキスト編集中はモード内
  ホットキーをスルー、`App::ime_input_active` で判定)
- `[💾]` ボタン: 現在のパラメータをこのスロットに保存
- ホットキー: モード内で `1` / `2` / `3` / `4` (修飾なし) を押すと該当プリセットを適用

#### 適用時の挙動

```rust
fn apply_preset(&mut self, slot: usize) {
    let Some(preset) = self.settings.conceal_presets[slot].clone() else { return };
    self.settings.conceal_type = preset.conceal_type;
    self.settings.conceal_mosaic_tile_mode = preset.mosaic_tile_mode;
    self.settings.conceal_mosaic_boundary = preset.mosaic_boundary;
    self.settings.conceal_fill_opacity_percent = preset.fill_opacity_percent;
    self.settings.conceal_fill_edge = preset.fill_edge;
    self.settings.conceal_blur_radius_px = preset.blur_radius_px;
    self.settings.conceal_blur_mode = preset.blur_mode;
    self.settings.conceal_blur_feather = preset.blur_feather;
    self.clear_all_conceal_caches();  // 全パラメータが変わる可能性があるため
}
```

**プリセット適用は Undo 対象外**。理由: パラメータはマスク本体ではなく「表示の好み」
であり、保存スロット間を行き来する操作で Undo スタックが汚染されると逆に使いにくい。
誤適用したら別のプリセットに切り替えるか、手動で値を戻す。
これは画像補正のスロット (Ctrl+0..9) が Undo 対象 (`UndoEntry::Adjustment`) になっている
のと違うが、補正は「ページ個別状態」を変えるのに対し、隠蔽プリセットは「グローバル
設定」を変えるだけなので方針が異なる。

#### 永続化

`settings.json` の `conceal_presets` フィールドに書く。サイドカーには書かない
(プリセットはフォルダ依存しないグローバル状態)。空スロット (`None`) も 4 つ分エントリ
保持。`Some(ConcealPreset { name: "", ... })` と `None` の違い:
- `None`: ボタンがグレーアウト、`[空] [💾]` 表示
- `Some` with empty name: `[プリセット N] [適用] [💾]` 表示

### 8.4 サイドカー (mimageviewer.dat)

`SidecarFile` に `conceal` フィールドを追加:

```json
{
  "items": {
    "ai_image_001.png": {
      "adjust": { ... },
      "mask":   { ... },
      "conceal": {
        "bitmap_w": 1024, "bitmap_h": 1024,
        "bitmap_data": "base64-deflated-1bit-stream",
        "vectors": [{ "kind": "Diag", "p0": [10, 20], "p1": [100, 200], "thickness": 15 }]
      }
    }
  }
}
```

書き込みタイミングは mask_db と同じ確定点のみ:
- ESC 終了時 (`save_conceal_with_sidecar`)
- マスク全削除ボタン (`delete_conceal_with_sidecar`)
- 5 秒アイドル時の `flush_idle_sidecars` (dirty 時のみ)

ストロークごとには書かない。Undo はメモリ内 `conceal_undo_stack` で十分。
プリセットとパラメータは `settings.json` 側に書かれるので、サイドカーには含めない。

## 9. キャッシュ無効化ルール

`docs/preset-and-adjustment.md` §4 の表を以下で拡張する。

| 変更内容 | `conceal_cache` | `fs_cache` | `adjustment_cache` | `ai_upscale_cache` |
| --- | --- | --- | --- | --- |
| マスク確定 (ESC / 全削除) | 該当 idx クリア | 触らない | 触らない | 触らない |
| 隠蔽タイプ変更 (グローバル) | **全クリア** | 触らない | 触らない | 触らない |
| Mosaic: タイル倍率変更 | **全クリア** | 触らない | 触らない | 触らない |
| Mosaic: 境界モード変更 | **全クリア** | 触らない | 触らない | 触らない |
| Fill: 不透明度変更 | **全クリア** | 触らない | 触らない | 触らない |
| Fill: 境界 Sharp/Feathered 変更 | **全クリア** | 触らない | 触らない | 触らない |
| Blur: 半径変更 | **全クリア** | 触らない | 触らない | 触らない |
| Blur: モード変更 | **全クリア** | 触らない | 触らない | 触らない |
| Blur: 境界フェード変更 | **全クリア** | 触らない | 触らない | 触らない |
| プリセット適用 | **全クリア** (パラメータが一気に変わる) | 触らない | 触らない | 触らない |
| 色系補正変更 (ページ個別) | 該当 idx クリア (入力変化) | 触らない | 該当 idx クリア | 触らない |
| ポストフィルタ変更 | 該当 idx クリア (入力変化) | 触らない | 該当 idx クリア | 触らない |
| AI モデル変更 | 該当 idx クリア (入力変化) | 触らない | 該当 idx クリア | 該当 idx クリア + cancel |
| 消しゴム inpaint 完了 | 該当 idx クリア (入力変化) | 上書き | 該当 idx クリア | 該当 idx クリア |
| 右 Ctrl プレビュー中 | バイパス (素表示) | — | — | — |
| フォルダ切替 | 全クリア | 全クリア | 全クリア | 全クリア |
| keep_range eviction | 該当 idx evict | 該当 idx evict | 該当 idx evict | — |

「全クリア」が多いように見えるが、グローバルパラメータ変更は全ページに影響するため
正しい挙動。スライダードラッグ中は色補正と同様に **drag-release granularity** を採用し、
リリース時に 1 回だけ全クリアする (毎フレーム再合成しない)。drag 中は最新の表示中 idx
のみ即時再合成して preview を維持する。

### 9.1 ヘルパー関数 (新規)

**Codex P2 指摘により generation-keyed + lazy eviction 方式を採用**。
`HashMap<usize, FsCacheEntry>` を一気に drop すると `Arc<ColorImage>` + GPU
テクスチャの解放で UI ヒッチが出るため (`fs_cache.clear` に計装が入っているのも
同じ理由)、世代番号で論理的に invalidate して必要に応じて lazy に再合成する。

```rust
impl App {
    fn clear_conceal_caches(&mut self, idx: usize);   // 個別 idx 用 (マスク確定時等)
    fn bump_conceal_generation(&mut self);            // グローバルパラメータ変更時
    fn current_conceal_generation(&self) -> u64;
}

// キャッシュエントリ自体に世代を持たせる
struct ConcealCacheEntry {
    pixels: Arc<ColorImage>,
    texture: TextureHandle,
    generation: u64,  // この値と current_conceal_generation が一致しない = stale
}
```

表示パイプライン入口で `entry.generation == self.current_conceal_generation()` を
判定し、一致しないエントリは「miss 扱い (再合成キューに積む)」+ lazy に drop
(keep_range eviction で順次解放)。これで visible idx だけが即時再合成され、
キャッシュにいる他ページは「次に表示されるとき」または「keep_range 外に出たとき」
に解放される。

### 9.2 idx-keyed cache lifecycle checklist (Codex P1)

**`conceal_cache` / `conceal_pages` は他の idx-keyed cache と並んで maintain しないと
他ページの隠蔽が表示される privacy 事故になる**。以下の全サイトで漏れなく更新:

| サイト | 必要な操作 |
| --- | --- |
| `start_loading_items` (フォルダロード) | `conceal_cache.clear()`, `conceal_pages` を `conceal_db::load_conceal_keys` で hydrate |
| `replace_search_view_items` (Ctrl+G 検索ビュー切替) | 上に同じ (items_generation +1 経路) |
| `remove_items_batch` (削除直後の idx 詰め) | 既存の idx シフトロジックに `conceal_cache` / `conceal_pages` を含める |
| `close_fullscreen` / `drop_fs_cache` | `conceal_cache` は keep_range 管理下なので fs_cache と同じタイミングで evict |
| `save_adjustment_result_into_cache` (色補正完了) | 該当 idx の `conceal_cache` clear (入力が変化) |
| `apply_ai_upscale_result` (AI 完了) | 同上 |
| `apply_erase_inpaint_result` (消しゴム inpaint 完了) | 同上 (`fs_cache` 上書き + 該当 idx clear) |
| `spread_display` (見開き) | 左右 idx それぞれで `conceal_cache` lookup、片方 stale 時の挙動を明記 |
| `start_fs_load` (フルスクリーン入り直し) | 該当 idx の generation 確認、stale なら再合成 |

これらは既存の `fs_cache` / `adjustment_cache` / `ai_upscale_cache` の maintenance
サイトと一致させる (`docs/preset-and-adjustment.md` §4 の表を更新)。

統合テストで以下を検証:
- フォルダ A (conceal あり) → フォルダ B (conceal なし) 切替で B の表示に A の隠蔽が
  出ない
- Ctrl+G 検索結果に conceal あり画像が出るとき、conceal_pages バッジが正しく出る
- ファイル削除で idx がシフトしても conceal が他ページにずれない

## 10. Ctrl+E エクスポート (汎用機能)

**隠蔽加工機能の文脈で導入するが、隠蔽加工専用ではない**。消しゴム済み画像・補正のみ・
何もしていない画像のいずれでも動作する。「現在のフルスクリーン表示画像 (= 全ての編集結果が
反映された display pixels) を、元と同じ場所に元の形式で保存」する汎用機能。

### 10.1 ダイアログ (バリエーション一括対応)

```
┌─ エクスポート ──────────────────────────────────────┐
│ ファイル名 (ベース): [original_edited]              │
│ 保存先:              C:\Users\...\AI\               │  ← 元と同じフォルダ既定
│ 形式:                JPEG (元形式)                   │
│                                                       │
│ 出力する設定 (チェックした分だけ生成):                │
│  ☑ 現在の設定                  → ..._0.jpg          │
│  ☑ プリセット 1 (投稿用 1x)    → ..._1.jpg          │
│  ☑ プリセット 2 (投稿用 2x)    → ..._2.jpg          │
│  ☐ プリセット 3 (強め)         → ..._3.jpg          │
│  ☐ プリセット 4 (整理用ぼかし) → ..._4.jpg          │
│                                                       │
│ ☑ AI プロンプト / EXIF を埋め込む                    │  ← 前回値を記憶
│                                                       │
│ ⚠ 元形式 (.heic) はメタデータ書込非対応です         │  ← 非対応形式のみ
│   [ JPEG にフォールバック ] [ PNG にフォールバック ] │  ← 非対応形式のみ
│                                                       │
│              [ キャンセル ] [ 保存 ]                  │
└───────────────────────────────────────────────────────┘
```

「同じマスクで複数バージョンを一括生成」は AI 画像整理ワークフローの差別化機能。
チェックした各設定について、隠蔽合成 + メタデータ付与 + ディスク書込を順次実行する。

#### ファイル名規則

| エントリ | 接尾辞 | 例 |
| --- | --- | --- |
| 現在の設定 | `_0` | `foo_edited_0.jpg` |
| プリセット 1 | `_1` | `foo_edited_1.jpg` |
| プリセット 2 | `_2` | `foo_edited_2.jpg` |
| プリセット 3 | `_3` | `foo_edited_3.jpg` |
| プリセット 4 | `_4` | `foo_edited_4.jpg` |

#### 衝突回避

既存ファイルが `_0` 等で衝突するときは **セッション番号** を挟む:
- 初回: `foo_edited_0.jpg`, `foo_edited_1.jpg`, ...
- 2 回目 (衝突): `foo_edited_0001_0.jpg`, `foo_edited_0001_1.jpg`, ...
- セッション番号 `_NNNN_` は 4 桁、衝突しない最小値を選ぶ

これでバリエーショングループが視認できる + 上書き事故を防ぐ。

#### 前回チェック状態の永続化

```rust
struct Settings {
    // …
    export_batch_selection: [bool; 5],  // [現在, p1, p2, p3, p4] の前回チェック状態
}
```

ダイアログを開いた時点で前回値を復元。「いつも同じ組合せで出す」ユーザーがクリック数最小で完了。

#### 進捗ダイアログ (同期実行)

複数エントリの生成中は **モーダル進捗ダイアログ** で状況を表示:

```
┌─ エクスポート中 ──────────┐
│  2 / 3 完了                │
│  ──────●─────              │
│  foo_edited_1.jpg 生成中…  │
│         [ キャンセル ]      │
└────────────────────────────┘
```

- 同期処理 (UI スレッドで順次実行 + 各エントリ完了時に `ctx.request_repaint()`)
- 1 エントリの生成時間目安: モザイク ~50ms + メタデータ ~50ms = ~100ms。
  ぼかし大きめなら ~200ms。5 エントリで合計 0.5-2 秒。許容範囲
- キャンセル時は処理中エントリ完了後に中断 (生成済みファイルはそのまま残す)
- **フォルダ内画像への multi-image batch は v1 では入れない** (v2 で検討、UI 複雑度が大幅に増えるため)

### 10.2 Settings 追加

```rust
struct Settings {
    // 既存…
    export_embed_metadata: bool,             // デフォルト true
    export_last_directory: Option<PathBuf>,  // 直近で元フォルダ以外を選んだ場合の記憶
    export_fallback_format: ExportFallbackFormat,  // 非対応形式時のフォールバック (Jpeg95/Png)
}
```

「保存先」のデフォルトは **元画像のフォルダ** (`GridItem::Image(p)` なら `p.parent()`)。
ユーザーが参照ボタンで他フォルダを選んだ場合は `export_last_directory` に記憶し、
次回ダイアログのデフォルトを「元フォルダ → last_directory の順で fallback」とする。
ただし元フォルダ自体は変動するので、`export_last_directory` は「直近の上書き選択」を
弱い記憶として持つだけ。

### 10.3 ファイル形式別動作

| 元拡張子 | 出力 | メタデータ |
| --- | --- | --- |
| `.jpg` / `.jpeg` | JPEG (turbojpeg q=95) | EXIF (APP1) + XMP (APP1) を元から移植 |
| `.png` | PNG (image crate) | tEXt / iTXt / zTXt を元から移植 (AI prompt 含む) |
| `.webp` (静止画) | WebP (`webp` クレート + 自前 RIFF mux) | EXIF / XMP / ICCP を VP8X 拡張で移植 |
| `.webp` (アニメ) | エラーダイアログ「アニメーション WebP は対応していません」 | — |
| `.heic` / `.avif` / `.jxl` / `.tiff` / RAW | JPEG / PNG フォールバック (ダイアログで確認) | フォールバック先の形式に合わせる |
| ZIP 内画像 | ZIP の親フォルダに、エントリの拡張子で保存 | エントリ形式に合わせる |
| PDF ページ | PDF の親フォルダに、PNG 既定で保存 | メタデータチェック disable |

### 10.4 ファイル名規則

- 通常画像: `{元 stem}_edited_{seq:04}.{元 ext}` (連番でユニーク化)
- ZIP エントリ: `{zip_stem}__{entry_stem}_edited_{seq:04}.{entry_ext}` (ダブルアンダースコアで境界明示)
- PDF ページ: `{pdf_stem}__p{page+1}_edited_{seq:04}.png`

### 10.5 save_with_metadata.rs の API

```rust
pub enum SrcFormat { Jpeg, Png, Webp, Other(String) }

pub enum SaveError {
    UnsupportedFormat,
    AnimatedWebpNotSupported,
    MetadataReadFailed(String),
    EncodingFailed(String),
    IoError(io::Error),
}

pub fn save_image_with_metadata(
    pixels: &ColorImage,
    src_path: Option<&Path>,   // None: メタデータ無し (ZIP/PDF 等で原本にアクセス不可な時)
    src_bytes: Option<&[u8]>,  // ZIP 内エントリ用: バイト列から直接メタデータ抽出
    dst_path: &Path,
    src_format: SrcFormat,
    include_metadata: bool,
) -> Result<(), SaveError>;
```

### 10.6 各形式の実装メモ

#### JPEG
- `turbojpeg::compress_image` で encode
- 元ファイル / 元バイト列から APP1 セグメント (EXIF + XMP) を生バイトで抽出
- 出力 JPEG の SOI (0xFFD8) 直後に APP1 を挿入
- `include_metadata == false` なら素の JPEG をそのまま出力

#### PNG
- `image` クレートの PngEncoder で IHDR / IDAT / IEND を書く
- 元から `png_metadata::read_png_text_chunks` でテキストチャンクを取り、IHDR と IDAT の間に
  生バイトで挿入
- AI prompt (A1111/Forge/ComfyUI/Midjourney) は既存 `png_metadata.rs` が解釈済みなので、
  生のチャンクをそのまま転記すれば prompt も自動で残る

#### WebP
- `webp::Encoder::new` で VP8 (lossy) または VP8L (lossless) にエンコード
- 元 WebP の RIFF コンテナを解析して ICCP / EXIF / XMP チャンクを抽出 (~80 行の RIFF パーサ)
- 出力 WebP コンテナを自前で構築:
  ```
  RIFF<size>WEBP
    VP8X<size> flags+canvas  ← EXIF/XMP/ICCP フラグビットを立てる
    [ICCP<size>...]  (元にあれば)
    VP8 or VP8L<size>...  (新エンコード結果)
    [EXIF<size>...]  (元にあれば)
    [XMP <size>...]  (元にあれば)
  ```
- 元が VP8 (lossy) だったか VP8L (lossless) だったかを保持する (チャンク識別子で判別)
- アニメーション WebP (`ANIM` / `ANMF` チャンク含む) は **エラーで弾く**
  → モザイク自体が静止画前提なので妥当

#### 非対応形式 (HEIC / AVIF / JXL / TIFF / RAW)
- mImageViewer は WIC でデコードできるが書き出しは持っていない
- ダイアログで「この形式 (.heic) はメタデータ書込非対応です。JPEG / PNG にフォールバックしますか?」と確認
- ユーザーが選んだフォールバック形式で保存。**メタデータは形式変換時に失われる**ことを明示
  (XMP 経由で何か残せるかは将来検討)

### 10.7 ZIP / PDF 元の特別扱い

- **ZIP 内画像**: 元バイト列 (`src_bytes`) からメタデータ抽出、出力は ZIP の親フォルダ
- **PDF ページ**: PDF にはページごとの EXIF / AI prompt が無いため、メタデータチェックボックスを
  disable + ツールチップ「PDF ページはメタデータがありません」
- どちらも書き出し先は ZIP/PDF と並ぶ位置 (parent dir)

## 11. Undo / Redo

`conceal_undo_stack: VecDeque<ConcealSnapshot>` (最大 20、消しゴムと同じ):

```rust
struct ConcealSnapshot {
    bitmap: Vec<bool>,
    bitmap_size: [usize; 2],
    vectors: Vec<LineObject>,
}
```

**マスク (bitmap + vectors) のみが Undo 対象**。パラメータ変更・タイプ切替・プリセット
適用は Undo 対象外 (§8.3 参照)。

- ストローク開始時に push、`conceal_last_undo_at` で 100ms 以内の重複 push を抑制
- `Ctrl+Z` で pop_back → 現在のマスクに復元 → redo stack に push
- `Ctrl+Shift+Z` で redo stack から pop
- モード入退場 / フォルダ切替 / fullscreen idx 移動で stack クリア (消しゴムと同じ境界)

**タイル倍率と境界モードは Undo 対象外** (グローバル設定なので)。
画像補正 (色系) の Undo は既存の `meta_undo` が処理 → 影響なし。
隠蔽加工モード中は `meta_undo` も `clear_meta_undo` で境界クリア (消しゴムと同じ理由)。

## 12. バッジ表示

- グリッドの左上に **[隠]** バッジ (隠蔽加工タイプを問わず、マスクが保存されているページに表示)
- 色: 紫系 (#9966CC 程度)。既存のバッジと区別:
  - 補正 [補]: 青
  - 消しゴム [消]: オレンジ
  - 隠蔽加工 [隠]: **紫** ← 新規
- 判定: `App::conceal_pages: HashSet<usize>` (タイプ問わずマスクがあるページ)
- 更新タイミング:
  - フォルダロード時に `conceal_db::load_conceal_keys` で hydrate
  - `save_conceal_with_sidecar` / `delete_conceal_with_sidecar` で同期更新

サムネイル本体には **隠蔽加工結果を反映しない** (消しゴムと同じ挙動)。
バッジのみで状態表示。これで「サムネとフルスクリーンで見た目が違う」が消しゴムと
一貫し、サムネスクロール中の負荷も増えない。

## 13. 実装フェーズ

**Codex P2 (1) 指摘により Phase 順を入れ替え**: `0 → 1 → 2 → 0b → 2b → ...` の順。
`vector_edit.rs` を先に作って Conceal 側で運用してから、消しゴム側に矩形/楕円を
**ハンドル付きで一度に**輸入する (UI を 2 回触らない)。

| Phase | 内容 | 想定工数 |
| --- | --- | --- |
| 0 | **mask_db.rs の `Shape` enum 化 + マイグレーション**: 旧 `LineObject` 互換デシリアライザ (**明示的な `"type"` キー判定、untagged fallback ではない** — Codex P1)、`rasterize_shape_into` 新設、`scanline_fill_ellipse` 新設、回帰テスト (旧 JSON / 旧サイドカー / 壊れた JSON / 未知 type / 混在配列 / 空配列)。**Sidecar の `Vec<LineObject>` → `Vec<Shape>` 移行も同 Phase で対応** (Codex P1) | 3-4 日 |
| 1 | App 状態追加、`ConcealType` / `MosaicBoundary` / `FillEdge` / `BlurMode` / `TileSizeMode` enum、`conceal_db.rs` (mask_db 流用、マスクスロット API 含む)、`Ctrl+M` モード遷移 + 空パネル。**Settings は `settings.db` 経由** (Codex P2、`COMPLEX_FIELDS` 不要) | 2-3 日 |
| 2 | `vector_edit.rs` 新設 (ハンドル方式・専用回転ハンドル + ↻ アイコン・Shift/Alt 修飾子・カーソル選択・ドラッグ状態機械、両ツール共用)、Conceal 側で 8 ツール実装 (Select/Brush/Lasso/Line/Vert/Horiz/Rect/Ellipse)、マスクオーバーレイ表示 | 4-5 日 |
| 0b | **消しゴム側のツールパレット拡張**: `ui_erase.rs` に矩形 (R) / 楕円 (O) ツールを **ハンドル付きで** 追加 (Phase 2 の `vector_edit.rs` を流用)、`EraseTool` enum 拡張 | 1-2 日 |
| 2b | 消しゴム側 Select モードを `vector_edit.rs` 経由に置換。**旧 Ctrl+ドラッグ複合操作 (回転+太さ) は完全廃止** — まだ利用者が少ない段階での UX 統一なので CHANGELOG 記載のみで legacy alias は持たない (ユーザー判断)、回帰テスト | 1-2 日 |
| 3a | Mosaic 合成実装 (3 境界モード × 2 タイルサイズモード)、タイル平均計算の rayon 並列化、`conceal_cache` + 表示パイプライン統合。**`conceal_cache` は generation-keyed + lazy eviction** (Codex P2)、`clear_*_caches` の代わりに `bump_conceal_generation()`、idx-keyed cache lifecycle checklist 完備 (Codex P1) | 3-4 日 |
| 3b | WhiteFill / BlackFill 合成実装、不透明度スライダー (1% 刻み)、Feathered 境界 (distance transform) | 1-2 日 |
| 3c | Blur 合成実装 (3 モード)、bbox 最適化、Gaussian separable blur。**最初から worker thread + cancel + progress** (Codex P1)。閾値 (estimated_cost_ms > 80) で同期/非同期分岐 | 3-4 日 |
| 4 | 永続化 (DB + サイドカー)、マスクスロット 2 個 UI、パラメータプリセット 4 個 UI、`conceal_pages` バッジ、Undo / Redo | 2-3 日 |
| 5 | `save_with_metadata.rs` (JPEG / PNG / WebP 3 形式) + ユニットテスト (roundtrip)。**WebP RIFF mux は既存 `xmp_writer.rs` から RIFF パーサ部分を共通化して流用** (Codex P2)。**PNG は生 chunk read/write が必要** (`png_metadata.rs` の生 chunk 取得 API を追加、Codex P3) | 4-5 日 |
| 6 | ✅ 完了: `export_dialog.rs` (`Ctrl+E` UI、**バリエーション一括チェックリスト**、`_N` 接尾辞)。**Worker pattern で最初から実装** (Codex P1) — 既存 `fs_capture_pending` を参考に `ExportPending { cancel, rx, total, done }`、進捗モーダル、UI スレッドはポーリングのみ。**ファイル名衝突は `OpenOptions::create_new(true)` リトライ** (Codex P3) | 4-5 日 |
| 7 | ✅ 完了: 統合テスト (mask_db 旧形式 → 新形式マイグレーション、sidecar Vec<Shape> 移行 roundtrip、conceal_db ラウンドトリップ、マスクスロット、ZIP/PDF ソースの export、一括バリエーション生成のファイル名衝突、worker キャンセル/進捗)、手動 E2E | 2-3 日 |
| 8 | ✅ 完了: ドキュメント更新 (下記 §15)。UI スナップショットは既存カバレッジ維持 | 1-2 日 |

**合計概算: 約 31-43 日** (Codex 指摘の worker 化分 + generation cache + 順序入れ替えで +5-6 日増)

内訳の増分理由:
- Phase 0: 明示的 type 判定 + sidecar 移行テスト (+1 日)
- Phase 3a: generation-keyed cache + idx lifecycle checklist (+1 日)
- Phase 3c: 最初から worker (+1 日)
- Phase 5: 既存 xmp_writer RIFF 共通化 + PNG 生 chunk API 新設 (+1 日)
- Phase 6: 最初から worker + 衝突回避リファクタ (+1 日)

## 14. テスト計画

### 14.1 ユニットテスト

- `compute_tile_size`: 長辺 × multiplier × 4px floor の境界ケース
- `mask_coverage_in_rect`: 完全カバー / 完全外 / 部分カバーの coverage 値
- `compute_tile_means`: 単色画像 / グラデーション画像での平均色
- `compose_mosaic` 3 モード: それぞれのモードで境界画素が想定通り
- `save_with_metadata`:
  - JPEG: encode → メタデータ付きで読み戻し → EXIF / XMP が同一
  - PNG: encode → tEXt チャンクが同一
  - WebP: encode → VP8X フラグ + EXIF / XMP / ICCP チャンクが同一
  - アニメ WebP 入力時のエラー判定
  - 非対応形式入力時のエラー判定

### 14.2 統合テスト

- `conceal_db` ラウンドトリップ (mask_db テストと同じ枠組み)
- サイドカー復元 (空 DB + サイドカー → DB に mosaic エントリ復元)
- ZIP / PDF エントリのキー整合 (`conceal_db::normalize_path` と一致)

### 14.3 手動 E2E

- 実 AI 生成 PNG (A1111 形式 prompt) でラウンドトリップ → prompt 維持確認
- 実 AI 生成 JPEG (EXIF UserComment 形式) で同様
- 実 ComfyUI 出力 WebP (もしあれば) で同様
- 1x / 2x / 5x の各倍率でモザイクを掛けた画像をビューアで確認 (タイル目に見えること)
- 半透明モード / マスク形状モードの見た目確認
- 4K 画像で `compose_mosaic` のレスポンス (~50ms 目標、UI 同期 OK ならその場で適用)

## 15. ドキュメント同時更新

CLAUDE.md の「コード修正時のドキュメント同時更新」ルールに従って:

- [docs/preset-and-adjustment.md](preset-and-adjustment.md) — キャッシュ階層に `conceal_cache` 追加、§4 表に行追加、§5 (消しゴム) と並列に §10 (隠蔽加工) 節を追加
- [docs/architecture-overview.md](architecture-overview.md) — 新規 4 モジュールを「アプリ状態層」「Persistence」に追記
- [docs/spec.md](spec.md) — 隠蔽加工機能の節、Ctrl+E の節を追加
- [docs/keymap-spec.md](keymap-spec.md) — Ctrl+M / Ctrl+E、隠蔽加工モード内のキー一覧を追加
- [htdocs/mimageviewer/manual/](../htdocs/mimageviewer/manual/) — 隠蔽加工機能ページ、エクスポートページを新設
  (バージョン固有表記なし、内部用語回避、**特定の投稿サイト名・基準名を一切書かない**。
  「この設定がどこそこの基準に合う」という表示はしない。タイルサイズの目安は px 数値で
  示すのみ。判断は利用者に委ねる旨を明記)
- [htdocs/mimageviewer/index.html](../htdocs/mimageviewer/index.html) — 機能一覧に追記
- 既存マニュアルと新規 2 ページを含む全 17 ページのサイドバーに同じリンク一覧を追加 (CLAUDE.md リリース手順 §1-6)

## 16. 設計上の前提・確定事項

- 機能名: **「隠蔽加工」** (UI / マニュアル)。内部識別子は `conceal_*` / `ConcealType` 等
- バッジ: **[隠]** (紫系)、消しゴム[消] (オレンジ) / 補正[補] (青) と区別
- ホットキー: **`Ctrl+M`** で入退場、モード内で `T` キーは隠蔽タイプ順次切替
- 処理タイプ 4 種: **Mosaic / WhiteFill / BlackFill / Blur**。タイプごとに異なるパラメータと境界モードを持つ
- タイルサイズは **2 モード** (長辺比率モード = 0.25 刻み / 固定 px モード = 1px 刻み) から選択
- 隠蔽加工は **サムネイルには反映しない** (消しゴムと同じ挙動、[隠] バッジのみ)
- ツールパレット **8 種** を消しゴムと完全統一: 選択 (S) / 筆 (B) / 囲み (L) / 直線 (I) /
  縦線 (V) / 横線 (H) / 矩形 (R) / 楕円 (O)。`Shape` enum / rasterizer / `vector_edit.rs`
  を共有
- データモデル `Shape` enum (Line / Rect / Ellipse) を `mask_db.rs` に新設。
  **既存の `LineObject` JSON との後方互換性を確保** (CLAUDE.md「永続データ・スキーマ変更時の判断」§リリース済み準拠)
- 消しゴム側にも矩形 / 楕円ツールを追加 (写真のゴミ消し用途)。リリース済み機能拡張のため
  Phase 0 でデータマイグレーションが必須
- すべてのパラメータ (タイプ、タイルモード、不透明度、ぼかし半径、境界モード等) は
  **グローバル設定** (`settings.json` 永続化、ページ間共有)。複数の好みを保持したい
  ときは **パラメータプリセット 4 スロット** (`1`〜`4` キーで適用)
- マスクは **ページ個別** に保存 (`conceal.db` + サイドカー)
- マスク用 2 スロット (`__slot_1` / `__slot_2`) で差分画像生成をサポート
- **特定の投稿サイト名・基準名・基準への適合判定を UI / ドキュメント / ヘルプ文に書かない**。
  境界処理モードのラベルも評価語ではなく **処理内容を具体的に書く** (例:
  「マスクを含むタイルを不透明で描画」)。詳細とレビュー方法は CLAUDE.md
  「モザイク・成人向け画像処理の表記ポリシー」を参照
- `Ctrl+E` は **隠蔽加工の有無にかかわらず使える汎用エクスポート機能**
  (補正のみ、消しゴム済み、何もしていない画像でも動作)
- `Ctrl+E` ダイアログは **バリエーション一括出力対応** (現在の設定 + 4 プリセットからチェックで複数生成)。
  ファイル名末尾に `_0`〜`_4` 接尾辞、衝突時は 4 桁セッション番号挿入
- メタデータ保持の対応形式は JPEG / PNG / WebP (静止画) の 3 形式
- HEIC / AVIF / JXL / RAW / TIFF はメタデータ書き出し非対応 → フォールバックダイアログで JPEG/PNG 選択
- PDF ページはメタデータ無し → エクスポート時メタデータチェックを disable
- アニメ WebP は静止画隠蔽加工と合わないため Ctrl+E でエラー

## 17. 未確定 / 将来課題

- 非対応形式 (HEIC 等) の **書き出し対応**: 将来 WIC エンコーダ経由で書ければ、フォールバックを廃止できる
- **隠蔽加工済み画像のサムネイル反映**: ユーザーから要望があれば、消しゴム反映と一緒に別枠で対応
- **WebP の XMP に AI prompt を埋める汎用フォーマット**: 各 AI ツールの慣行を更に調査する余地あり
- **マスクスロット数の拡張 (3 個以上)**: 現状 2 スロットで消しゴムと揃えているが、用途が増えたら拡張
- **パラメータプリセット数の拡張 (5 個以上)**: 現状 4 個でロック、要望次第で 8 / 10 に拡張
- **WhiteFill / BlackFill の任意カラー対応**: 当面は白/黒固定、要望次第でカラーピッカー追加
- **Blur 境界フェード半径の可変化**: 当面は固定値 (8px 想定)、要望次第でスライダー化
- **Blur の worker thread 化**: 実測 200ms 超のケースが頻発したら pending パターンに移行
- **viewing-mode quick apply (Shift+F7/F8)**: 隠蔽加工モードに入らずマスクスロット適用したいニーズが出たら追加
- **タイル倍率のページ別オーバーライド**: グローバル設定で始めて、後でニーズが出たらページ個別を追加
- **ぼかし半径の上限拡張**: 現状 100px、超巨大なマスク (4K の人物全体など) で足りなければ拡張
- **フォルダ内マスク付き全画像への一括バリエーション生成 (multi-image batch)**: v1 では単一画像のみ、フォルダトラバーサル + 画像 × エントリ進捗集計 + ファイル名衝突管理が UI 複雑度大のため v2 で検討
- **プリセット名連動ファイル名 (`_p1_投稿用1x` 等)**: v1 では `_0`〜`_4` 固定、要望次第でユーザー設定可能化
