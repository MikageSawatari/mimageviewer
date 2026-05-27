# 隠蔽加工機能 — 実装プラン (Codex レビュー依頼)

このドキュメントは、[`docs/conceal-feature-plan.md`](conceal-feature-plan.md) の設計に基づいた
**実装計画のレビュー依頼**です。設計仕様自体は確定済みで、ここでは「どの順序で・どのコード境界で・
どの回帰リスクに気をつけて」着手すべきかについて第二意見をいただきたい。

## 0. レビュー観点 (Codex への質問事項)

下記プランを読んで、優先度順に評価してほしい:

1. **Phase 0 のマイグレーション設計** (§2): `Shape` enum の `serde(untagged)` で旧
   `LineObject` JSON を読む設計は、`Serialize` の対称性 (常に新形式で書く) と
   `set_raw` (サイドカー import で既存 JSON 文字列をそのまま DB に貼る経路) との
   両立で抜けはないか? `vectors_from_json` が「タグ付き Shape JSON だけ」を期待
   していて、過去 DB の素 LineObject 文字列を `set_raw` 経由でそのまま戻す
   ケースで壊れないか。
2. **Phase 0b と Phase 2 の順序** (§3): 消しゴム側に矩形/楕円ツールを足してから
   `vector_edit.rs` の新 UX に置換する流れにしているが、「先に vector_edit.rs を
   作り Conceal 側で先に運用 → 消しゴム側に矩形/楕円を後で輸入」の方がリスクが
   小さいかどうか。
3. **ベクタオブジェクト UX 改変の影響範囲** (§4): 既存消しゴムの Ctrl+ドラッグ
   複合操作 (垂直=回転 / 水平=太さ) を廃止して、ハンドルドラッグ + Shift/Alt
   モディファイア方式に切り替える。これはリリース済みユーザーへの破壊的 UX 変更
   なので、CHANGELOG 明記以外に取れる緩和策 (旧操作も併存 / 設定で切替 / 移行
   トースト) があるか。
4. **Phase 3a のキャッシュ無効化の幅** (§5): グローバル設定 (隠蔽タイプ、タイル
   倍率、境界モード、不透明度、ぼかし半径、モード) は変更時に
   `conceal_cache` を**全クリア**する方針。drag-release granularity で軽減する
   が、worst case (フォルダ内 100 画像が `conceal_cache` を持つ状態でスライダー
   release) で UI ヒッチが出ないか。
5. **Phase 3c の Blur 同期実装** (§6): 4K + radius=100 + bbox 画像全体で
   ~300ms が予想される。当初は同期で開始 (`estimated_cost_ms > 200` で worker
   化判定は後追い TODO) としているが、最初から worker thread に出した方が
   良いか。
6. **Phase 5 WebP の自前 RIFF mux** (§7): `webp` クレートは encode しか持って
   いないので RIFF コンテナ (VP8X + ICCP + EXIF + XMP) を自前で組み立てる必要
   がある。実装複雑度 ~80-150 行 + 多形式 roundtrip テストで足りるか、もっと
   先行プロトタイプが必要か。
7. **Phase 6 の同期エクスポート** (§8): 5 エントリ生成 = ~0.5-2 秒を UI スレッド
   同期 + 進捗ダイアログで処理する方針。worker thread + mpsc にする方が
   `docs/ui-responsiveness.md` 方針に整合するが、ダイアログのキャンセル/進捗
   伝達が複雑になる。閾値 (Blur で重いケース等) でだけ worker 化する選択肢
   含めて意見が欲しい。
8. **見落とし**: 上記以外で、リリース済みユーザーデータ・UI 互換性・
   並行処理・キャッシュ整合・テスト不足の観点で抜けがないか。

回答は **P1 (修正必須) / P2 (修正推奨) / P3 (検討事項)** で severity 付け、
ファイル/プラン§番号を明記してほしい。


## 1. 設計仕様の前提 (要点だけ)

- 機能名: 「隠蔽加工」、バッジ [隠] (紫)、入退場 `Ctrl+M`、タイプ切替 `T`
- 4 タイプ: Mosaic / WhiteFill / BlackFill / Blur
- 8 ツール (消しゴムと完全統一): Select(S) / Brush(B) / Lasso(L) / Line(I) /
  VertLine(V) / HorizLine(H) / Rect(R) / Ellipse(O)
- データモデル: `Shape` enum (Line / Rect / Ellipse) を `mask_db.rs` に新設、
  両ツール共有。**既存 LineObject JSON との後方互換性必須** (リリース済み)
- グローバル設定 + 4 パラメータプリセット (`1`〜`4` キー)、2 マスクスロット
- 汎用 `Ctrl+E` (元と同じ場所に元形式で保存、メタデータ保持)、バリエーション
  一括出力 (`_0`〜`_4`、衝突時 4 桁セッション番号)
- **特定の投稿サイト名・基準名・基準への適合判定を UI/ドキュメントに書かない**
- UI ラベルは処理内容を具体的に記述 (「マスクを含むタイルを不透明で描画」等)

詳細: [`docs/conceal-feature-plan.md`](conceal-feature-plan.md)


## 2. Phase 0 — `mask_db` Shape enum + 後方互換マイグレーション

**前提**: `mask_db` は既にリリース済み (v0.9.x で配布、消しゴムマスクが各ユーザーの
`%APPDATA%/mimageviewer/mask.db` とサイドカー `mimageviewer.dat` に蓄積)。
`CLAUDE.md`「永続データ・スキーマ変更時の判断」§リリース済みに従い、無痛
マイグレーションが必須。

### 2.1 新規 enum と関数 (src/mask_db.rs)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Shape {
    Line { kind: LineKind, p0: (f32, f32), p1: (f32, f32), thickness: f32 },
    Rect { center: (f32, f32), half_w: f32, half_h: f32, rotation_rad: f32 },
    Ellipse { center: (f32, f32), rx: f32, ry: f32, rotation_rad: f32 },
}

impl Shape {
    pub fn center(&self) -> (f32, f32);
    pub fn translate(&mut self, dx: f32, dy: f32);
    pub fn rotate_around(&mut self, cx: f32, cy: f32, angle: f32);
    pub fn bbox_corners(&self, extra: f32) -> [(f32, f32); 4]; // hit/draw 用
}

pub fn rasterize_shape_into(mask: &mut [bool], shape: &Shape, w: usize, h: usize);
pub fn scanline_fill_ellipse(mask: &mut [bool], shape: &Shape, w: usize, h: usize, value: bool);
```

### 2.2 Deserialize の後方互換性

旧形式は `LineObject` の素 JSON 配列:

```json
[{"kind":"diag","p0":[10,20],"p1":[100,200],"thickness":15}]
```

新形式はタグ付き:

```json
[{"type":"line","kind":"diag","p0":[10,20],"p1":[100,200],"thickness":15},
 {"type":"rect","center":[200,300],"half_w":50,"half_h":30,"rotation_rad":0.0}]
```

実装方針 (Codex に確認したい点):

```rust
impl<'de> Deserialize<'de> for Shape {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Tagged(TaggedShape),       // {"type":"...", ...}
            LegacyLine(LegacyLineObj), // {"kind":"...", "p0":[...], ...}
        }
        match Wire::deserialize(d)? {
            Wire::Tagged(t) => Ok(t.into()),
            Wire::LegacyLine(l) => Ok(Shape::Line {
                kind: l.kind, p0: l.p0, p1: l.p1, thickness: l.thickness,
            }),
        }
    }
}
```

- `vectors_to_json(&[Shape])` は常に新形式で出力
- `vectors_from_json(&str) -> Vec<Shape>` は両形式を読む
- 既存の `LineObject` 型自体は当面残し (まだ消しゴム / sidecar / app が使用中)、
  Phase 0b と Phase 2b で順次 `Shape` に置換していく。Phase 0 完了時点では
  `Shape <-> LineObject` の相互変換ヘルパー (`Shape::as_legacy_line()`,
  `From<LineObject> for Shape`) を提供して、共存期間を許容する。

### 2.3 サイドカー (`set_raw`) 経路への影響

`mask_db::set_raw(key, compressed_bitmap, vectors_json: Option<&str>, w, h)` は
**JSON 文字列を再パースせずそのまま DB に貼る**経路 (サイドカー復元時に再
圧縮を避けるため)。サイドカーは過去 LineObject JSON を持っている可能性が
高いので、`set_raw` は旧形式 JSON も受け入れ続ける必要がある (= 何もしない、
DB に旧形式が入る → 次回 `get_full` の `vectors_from_json` がタグなし読込)。

**懸念**: 一度 Conceal で Rect/Ellipse を追加して保存し直すと、サイドカーには
タグ付き新形式が書かれる。サイドカーを古い mIV (v0.9.x 等) で開かれるリスクは
あるが、ロールバック (v0.9.x への戻し) はユーザー操作なので非対象とする。

### 2.4 `get_full` のスケール変換拡張

PDF 再レンダリング等で画像サイズが変わったときの座標スケーリングを `Shape` の
全 variant に対応:

| variant | 変換 |
|---|---|
| Line | 既存通り (p0/p1 を sx,sy で伸縮、thickness は (sx+sy)/2) |
| Rect | center を sx,sy で伸縮、half_w*=sx, half_h*=sy、rotation_rad はそのまま (※非等比スケーリングで rotation を保つかは要検討、設計仕様では「画像比率は変わらない前提」だが PDF のページ単位 DPI 違いで非等比が起きうる) |
| Ellipse | 同上 (rx*=sx, ry*=sy) |

**Codex 観点**: 非等比スケーリング (sx ≠ sy) で回転矩形/楕円が崩れる問題は
当面無視で OK か、最初から「回転 0 か isotropic スケールのみ正しい変換」と
明文化すべきか。

### 2.5 回帰テスト (`mask_db::tests` 追加)

- `legacy_line_json_roundtrip`: 旧 LineObject JSON を deserialize → Shape::Line で復元
- `mixed_legacy_and_tagged_array`: `[{"kind":"diag",...}, {"type":"rect",...}]` 混在配列
- `serialize_always_tagged`: Shape::Line を to_string → `"type":"line"` を含む
- `set_raw_preserves_legacy_string`: set_raw で旧 JSON を貼る → get_full で読める
- `rasterize_shape_rect_axis_aligned`: 0 度矩形のラスタライズ画素境界
- `rasterize_shape_rect_rotated_45`: 45 度回転矩形 (4 corners 計算 + polygon fill)
- `rasterize_shape_ellipse_axis_aligned`: 軸並行楕円のラスタライズ
- `rasterize_shape_ellipse_rotated`: 30 度回転楕円
- `get_full_rescale_rect`: 1000x1000 → 2000x1500 で Rect の half_w/half_h が正しくスケール
- `get_full_rescale_ellipse`: 同上で rx/ry

**工数**: 2-3 日


## 3. Phase 0b — 消しゴム矩形/楕円ツール追加

`ui_erase.rs` (~2861 行) に R/O ツール追加。Phase 0 で `Shape` enum と
`rasterize_shape_into` が揃っているので、commit 経路はそれを使う。

### 3.1 EraseTool enum 拡張

```rust
pub(crate) enum EraseTool {
    Select, Brush, Lasso,
    VertLine, HorizLine, Line,
    Rect,    // 新規 R キー
    Ellipse, // 新規 O キー
}
```

### 3.2 ドラッグ作成フロー

- **Rect**: corner→corner ドラッグ。pressed で開始点固定、released で
  `Shape::Rect { center=(p0+p1)/2, half_w=|dx|/2, half_h=|dy|/2, rotation_rad=0 }`
  を commit。Shift で正方形拘束 (half_w == half_h)、Alt で開始点を中心扱い。
- **Ellipse**: 内接 bbox ドラッグ。Rect と同じ bbox → `Shape::Ellipse { center,
  rx=half_w, ry=half_h, rotation_rad=0 }` を commit。修飾子は Rect と同じ。

### 3.3 既存マスクへの影響

- ベクタ vec のフィールド型を `Vec<LineObject>` → `Vec<Shape>` に変更
  (`app.rs`、`sidecar.rs`、`ui_erase.rs`、`ui_adjustment_panel.rs`、
  `app/tests.rs` の 7 ファイル参照)。
- 旧 LineObject ベースのコードパス (corners 計算等) は `Shape::Line` variant
  でのみ動作するように分岐するか、`Shape::bbox_corners` で汎用化する。
- 既存マスクは Phase 0 のマイグレーションで自動的に `Shape::Line` として読める。

### 3.4 Select モードの hit_test 拡張

矩形/楕円の本体・角ハンドル・辺中点ハンドルは Phase 2 の `vector_edit.rs` で
本格対応するが、Phase 0b 時点では従来の hit_test (本体のみ) で動かす。

### 3.5 回帰テスト

- 既存 6 ツール (S/B/L/V/H/I) の挙動が変わらない (snapshot test 含む)
- 既存マスクが「マスクなし」になっていないか (load → 再描画)
- ESC で save → 再起動 → load roundtrip
- 矩形/楕円作成 → ビットマップに正しくラスタライズ

**工数**: 1-2 日


## 4. Phase 1 — Conceal モード骨組み

### 4.1 App 状態追加 (詳細は plan §3.4)

`App` 構造体に conceal_* フィールド 20 個程度を追加。`Default` impl で初期化。
モードフラグ・ツール選択・マスク・vectors・各種パラメータ・キャッシュ・Undo。

### 4.2 settings.json への追加

```rust
struct Settings {
    // …既存…
    conceal_type: ConcealType,                // default Mosaic
    conceal_mosaic_tile_mode: TileSizeMode,   // default LongEdgeRatio(1.0)
    conceal_mosaic_boundary: MosaicBoundary,  // default Opaque
    conceal_fill_opacity_percent: u8,         // default 100
    conceal_fill_edge: FillEdge,              // default Sharp
    conceal_blur_radius_px: f32,              // default 20.0
    conceal_blur_mode: BlurMode,              // default AsMask
    conceal_blur_feather: bool,               // default false
    conceal_brush_radius: f32,                // default 12.0
    conceal_line_width: f32,                  // default 12.0
    conceal_presets: [Option<ConcealPreset>; 4],  // default [None; 4]

    export_embed_metadata: bool,              // default true
    export_last_directory: Option<PathBuf>,   // default None
    export_fallback_format: ExportFallbackFormat,  // default Jpeg95
    export_batch_selection: [bool; 5],        // default [true, false, false, false, false]
}
```

`settings_db` の SQLite 永続化経路 (Phase 3 で JSON → SQLite 移行済み) に
対応する migration を追加。これらは**未リリースなので**簡易追加で OK
(default 値が無いカラム = NULL 扱い、起動時に default を入れて UPDATE)。

### 4.3 conceal_db.rs

`mask_db.rs` のクローンベース。テーブル名を `conceal_entries`、key スキーマは
同一 (page_path / `__slot_1` / `__slot_2`)。Shape ベクタは Phase 0 で実装した
タグ付き JSON で書く。`set_raw` も同梱 (サイドカー復元用)。

### 4.4 Ctrl+M モード入退場

- `ui_fullscreen.rs` の入力ハンドラに Ctrl+M を追加 (既存 Ctrl 系と衝突なし
  確認済み — 設計時点で確認、実装時に再確認)
- 入退場時の動作:
  - 入場: 現ページの conceal マスクを `conceal_db` から hydrate
  - 退場 (Esc / Ctrl+M 再押下): 確定保存 (`save_conceal_with_sidecar`)、
    Undo stack クリア、モードフラグ false
- 空パネル表示 (Phase 2 でツールパレット実装)

### 4.5 IME / テキスト入力の扱い

`App::ime_input_active()` (既存) で IME 変換中はホットキー無効化。
プリセット名編集中 (TextEdit が focus) もホットキー (1〜4 / T / B/L/S 等) を
スルー。`response.has_focus()` で判定。

### 4.6 回帰テスト

- Ctrl+M で入退場できること
- 退場時に DB へ書かれること
- フォルダ移動でモードが解除されること

**工数**: 2-3 日


## 5. Phase 2 — `vector_edit.rs` + 8 ツール (ベクタ UX 改変含む)

これが最も大きい Phase。`ui_erase.rs` の Select モード相当の機能を新モジュール
に切り出し、Conceal モードと共有しつつ UX を改善する。

### 5.1 新 UX 要件 (ユーザー確認済み)

| 操作 | 旧 (消しゴム) | 新 (両ツール共通) |
|---|---|---|
| 全体移動 | 本体ドラッグ | **同じ** (本体ドラッグ) |
| 線/矩形のサイズ変更 | Ctrl+ドラッグ (水平方向) | 端点ハンドル (Line) / 角ハンドル (Rect/Ellipse) / 辺中点ハンドル (Rect/Ellipse) |
| 回転 | Ctrl+ドラッグ (垂直方向) | 専用回転ハンドル (Rect/Ellipse の bbox 上辺の中点から伸びる棒の先の丸) |
| 軸拘束 | なし | **Shift+ハンドルドラッグ** (水平/垂直/45°、Rect/Ellipse の角は等比) |
| 中心固定リサイズ | なし | **Alt+ハンドルドラッグ** (反対側の辺/角も連動) |
| 角度スナップ | なし | **Shift+回転ハンドル** (15° 刻み) |
| 端点編集 (Line) | 端点ハンドル | **同じ** |
| カーソル形状 | デフォルトのまま | ハンドル上で `ResizeHorizontal` / `ResizeVertical` / `ResizeNeSw` / `ResizeNwSe` / 回転ハンドル上で `PointingHand` + `ui.painter()` で ↻ アイコン重ね描き |

**破壊的 UX 変更**: 旧消しゴムの Ctrl+ドラッグ複合操作 (水平=太さ・垂直=回転) を
**廃止**。リリース済みユーザーへの影響は CHANGELOG (README.md 更新履歴 + リリース
notes) で「消しゴムの Ctrl+ドラッグ複合操作はハンドル方式に変更されました」を明記。

### 5.2 モジュール構成

```rust
// src/vector_edit.rs (新規、~600 行想定)

/// ハンドル位置の論理識別子。
pub enum HoverTarget {
    Body,                      // 本体内部 (移動)
    Endpoint { which_p1: bool }, // Line 専用
    Corner { idx: u8 },        // Rect/Ellipse の bbox 4 角 (0=NW,1=NE,2=SE,3=SW)
    EdgeMidpoint { idx: u8 },  // Rect/Ellipse の bbox 4 辺中点 (0=N,1=E,2=S,3=W)
    RotateHandle,              // 専用回転ハンドル (bbox 上辺中点から距離 R)
}

/// ドラッグ状態。
pub enum DragState {
    Pan      { idx: usize, base: Shape, origin: (f32, f32) },
    Endpoint { idx: usize, base: Shape, which_p1: bool, origin: (f32, f32) },
    Resize   { idx: usize, base: Shape, target: HoverTarget, origin: (f32, f32),
               anchor: (f32, f32) /* Alt=center, normal=opposite corner */ },
    Rotate   { idx: usize, base: Shape, origin: (f32, f32),
               center: (f32, f32), start_angle: f32 },
}

/// ハンドルのスクリーン座標を計算。draw_handles と hit_test で共有。
pub struct HandleLayout {
    pub body_corners: [(f32, f32); 4],
    pub endpoints: Option<[(f32, f32); 2]>,    // Line のみ
    pub corners: Option<[(f32, f32); 4]>,      // Rect/Ellipse のみ
    pub edge_midpoints: Option<[(f32, f32); 4]>, // Rect/Ellipse のみ
    pub rotate_handle: Option<(f32, f32)>,     // Rect/Ellipse のみ
}

pub fn compute_handle_layout(shape: &Shape, scale: f32) -> HandleLayout;
pub fn hit_test(layout: &HandleLayout, pos_screen: (f32, f32)) -> Option<HoverTarget>;
pub fn cursor_icon_for(target: HoverTarget, shape: &Shape) -> egui::CursorIcon;
pub fn draw_handles(painter: &egui::Painter, layout: &HandleLayout,
                    selected: bool, hovered: Option<HoverTarget>);

/// Shift/Alt 修飾子付きドラッグ適用。
pub fn apply_drag(
    state: &DragState,
    cur: (f32, f32),
    modifiers: &egui::Modifiers, // Shift: 拘束/比率/15°snap、Alt: 中心固定
) -> Shape;
```

### 5.3 回転ハンドルの描画

```text
        ↻ (← ui.painter().text() で描画、半透明 16px 程度)
        ●  ← 専用ハンドル (棒の先の丸)
        |  ← ハンドルから bbox 上辺中点への棒 (距離 ~28px @ 1.0x zoom)
   ┌────┼────┐
   │    │    │  ← Rect/Ellipse の bbox
   └─────────┘
```

棒の長さは画面 px で固定 (= zoom_pan に依存しない、ハンドルの操作性を保つため)。
カーソルが ↻ アイコンに近づいたら `CursorIcon::PointingHand` + ↻ を強調表示。

### 5.4 Shift/Alt 修飾子の挙動

- **Shift+Resize (角ハンドル)**: 等比 (Rect で aspect ratio 維持、Ellipse で
  rx/ry 比維持)
- **Shift+Resize (辺中点)**: なし (本来軸固定なので無意味)
- **Shift+Endpoint (Line)**: 0°/45°/90° スナップ
- **Shift+Rotate**: 15° 刻みスナップ
- **Shift+Pan**: なし (まず v1 では水平/垂直拘束は入れない、要望次第で追加)
- **Alt+Resize (角/辺)**: 中心固定 (反対側の辺/角も連動)
- **Alt+Rotate**: なし
- **Alt+Endpoint**: なし

### 5.5 Conceal モードでの 8 ツール実装

`ui_conceal.rs` (新規、~2500 行想定) に消しゴムと同じパターンで:
- ツールパネル (S/B/L/I/V/H/R/O + 描画/消去 + 太さスライダー + プリセット 4 + スロット 2)
- handle_conceal_paint (`ui_erase` と同じ構造、Select のみ vector_edit.rs に委譲)
- マスクオーバーレイ表示 (赤系半透明、消しゴムのオレンジと区別)

### 5.6 回帰テスト

- `vector_edit::tests`:
  - `hit_test_rect_corner_inside`: 角ハンドル中心と外周ヒット
  - `hit_test_rotate_handle_above_top_edge`: 回転ハンドル
  - `apply_drag_resize_shift_equal_aspect`: Shift で等比
  - `apply_drag_resize_alt_center_anchor`: Alt で中心固定
  - `apply_drag_rotate_shift_snap_15deg`: 15° スナップ
- 手動 E2E: 消しゴム + Conceal 双方で 8 ツールが UI 通り動く、ハンドルカーソル
  が想定通り変わる

**工数**: 4-5 日


## 6. Phase 2b — 消しゴム Select モードを vector_edit.rs に置換

### 6.1 既存 EraseVectorDrag 削除

`app.rs` の `EraseVectorDrag` enum を削除、`vector_edit::DragState` に置換。
`ui_erase.rs` の `update_vector_drag` / `hit_test_vector` も削除、`vector_edit::*` 呼出しに。

### 6.2 Ctrl+ドラッグ複合操作の廃止

旧 Ctrl+ドラッグ (水平=太さ・垂直=回転) を削除。コメントに「v1.0.x で
ハンドル方式に置換」と注記。

### 6.3 CHANGELOG 明記

README.md 更新履歴 + リリース notes に:

> 消しゴムの選択ツール (S) で、ベクタオブジェクトのサイズ変更・回転は
> ハンドルドラッグ方式に変更されました (旧 Ctrl+ドラッグ複合操作は廃止)。
> Shift で軸拘束/角度スナップ、Alt で中心固定リサイズが可能です。

### 6.4 回帰テスト

- 既存 Diag/Vert/Horiz 線の Pan/Endpoint 操作が動く
- マスクの保存形式が変わらない (Phase 0 マイグレーション後の Shape::Line で
  Serialize)

**工数**: 1-2 日


## 7. Phase 3a/b/c — 合成実装

Plan §7 の通り。設計上の論点は §0 の Q4/Q5/Q6/Q7 に集約。

### 7.1 Phase 3a (Mosaic) の補足

- `compose_mosaic` は `src/conceal_compose.rs` に置く (新規ファイル)
- 並列化: rayon で row-band (各タイル行) を chunk_par_mut。4K 画像で
  Opaque/Translucent が ~30-50ms、MaskShape は per-pixel なので ~80-100ms 想定
- `conceal_cache` は `HashMap<usize, FsCacheEntry>` (App 既存パターンに沿う)
- 表示パイプラインへの組込みは `display-pipeline.md` に従って `fs_cache` →
  `ai_upscale_cache` → `adjustment_cache` → `conceal_cache` の順
- `clear_all_conceal_caches()` はグローバルパラメータ変更/プリセット適用時
- drag-release granularity: スライダー drag 中は最新表示 idx だけ即時再合成
  (release で `clear_all_conceal_caches`)

### 7.2 Phase 3b/3c の補足

- `compose_solid_fill` / `compose_blur` も `conceal_compose.rs` に同居
- distance transform は 2-pass (Felzenszwalb) で O(N)、~30-50ms @ 4K
- Gaussian separable blur は別関数化 (`gaussian_blur_separable`)
- `compute_edge_feather_alpha` は Feathered (Fill) と feather_boundary (Blur)
  で共有

**工数**: 5-8 日 (3a=2-3, 3b=1-2, 3c=2-3)


## 8. Phase 5/6 — エクスポート系

### 8.1 Phase 5 メタデータ保持エンコード

詳細はプラン §10.6。

**追加メモ**:
- JPEG の APP1 抽出は既存の `exif_reader.rs` / `xmp_reader.rs` で使う
  バイトオフセット計算を流用できないか調査 (重複実装回避)
- PNG の tEXt/iTXt/zTXt は既存 `png_metadata.rs` がパースしているので、
  生バイトの再書き込みは新規実装。チャンク CRC32 計算が必要 (`crc32fast` クレート)
- WebP の自前 RIFF mux は新規。`webp` クレートが VP8/VP8L encode を返すので、
  それを RIFF コンテナで包む

### 8.2 Phase 6 ダイアログ

詳細はプラン §10.1-10.5。

**追加メモ**:
- 進捗ダイアログは `egui::Window` モーダル、`request_repaint_after(Duration)`
  で UI 更新
- キャンセル: `Arc<AtomicBool>` を共有、エントリ完了直後に check
- ファイル名衝突回避は **enumerate 段階で全候補をテスト** (atomicity は
  ベストエフォート、書き込み中に他プロセスが同名を作るレースは諦める)

**工数**: 3-4 日 × 2


## 9. Phase 7/8 — テストとドキュメント

### 9.1 統合テスト (`tests/conceal_integration.rs`)

- 旧 mask_db → 新形式マイグレーション (一括 select → set_raw → get_full
  ラウンドトリップ)
- conceal_db ラウンドトリップ (set → get_full → ベクタとビットマップ一致)
- マスクスロット (set_slot → get_slot_full → 解像度違いリスケール)
- save_with_metadata roundtrip (JPEG/PNG/WebP それぞれ実 sample で)
- export_dialog 一括バリエーション (5 エントリ生成 → 全部存在 + ファイル名衝突
  時のセッション番号挿入)

### 9.2 手動 E2E (`docs/e2e-smoke-test.md` 追記)

- Ctrl+M で隠蔽加工モード入退場
- 8 ツール作成 → 確定保存 → 再起動 → 復元
- Shift+ハンドルで軸拘束、Alt+ハンドルで中心固定、Shift+回転で 15°スナップ
- 4 プリセット保存・適用 (1〜4 キー)
- マスクスロット 2 個 (差分画像生成シナリオ)
- Ctrl+E バリエーション一括 (5 ファイル生成)
- メタデータ roundtrip: A1111 PNG / EXIF UserComment JPEG / ComfyUI WebP

### 9.3 ドキュメント

- `docs/preset-and-adjustment.md` §10 隠蔽加工追加、キャッシュ階層更新
- `docs/architecture-overview.md` 新規 5 モジュール追記 (vector_edit.rs,
  ui_conceal.rs, conceal_db.rs, conceal_compose.rs, export_dialog.rs,
  save_with_metadata.rs)
- `docs/spec.md` 隠蔽加工 + Ctrl+E 節
- `docs/keymap-spec.md` Ctrl+M/Ctrl+E + モード内キー
- `htdocs/mimageviewer/manual/conceal.html` (新規ページ)
- `htdocs/mimageviewer/manual/export.html` (新規ページ)
- 既存 14 マニュアルページのサイドバー一括更新
- `htdocs/mimageviewer/index.html` 機能一覧
- UI スナップショット更新 (8 ツール、プリセット行、Ctrl+E ダイアログ)

**工数**: 2-3 日 + 1-2 日


## 10. 全体工数 & 着手順

| Phase | 内容 | 工数 |
|---|---|---|
| 0 | mask_db Shape + マイグレーション | 2-3 日 |
| 0b | 消しゴム R/O ツール追加 | 1-2 日 |
| 1 | Conceal モード骨組み | 2-3 日 |
| 2 | vector_edit.rs + 8 ツール (Conceal) | 4-5 日 |
| 2b | 消しゴム Select 移行 | 1-2 日 |
| 3a | Mosaic 合成 | 2-3 日 |
| 3b | Fill 合成 | 1-2 日 |
| 3c | Blur 合成 | 2-3 日 |
| 4 | 永続化/プリセット/バッジ/Undo | 2-3 日 |
| 5 | save_with_metadata | 3-4 日 |
| 6 | Ctrl+E ダイアログ | 3-4 日 |
| 7 | 統合テスト | 2-3 日 |
| 8 | ドキュメント | 1-2 日 |

**合計: 26-37 日**


## 11. 確認したい主要リスク (重複再掲)

1. Phase 0 の `serde(untagged)` マイグレーションは旧 LineObject + 新 Shape
   混在配列で安全か (P1 候補)
2. Phase 2 の UX 変更で旧消しゴム Ctrl+ドラッグ廃止が破壊的すぎないか (P2 候補)
3. Phase 3a の `conceal_cache` 全クリアの hitch リスク (P2 候補)
4. Phase 3c の Blur 同期実装は最初から worker 化すべきか (P3 候補)
5. Phase 5 WebP の自前 RIFF mux 実装複雑度 (P3 候補)
6. Phase 6 の同期エクスポートで UI が止まる懸念 (P2 候補)
7. その他の見落とし (P1〜P3)
