# 魚眼ズームと写真の板の 3D 傾け (§1.917・§1.819) — 設計書

- 対象: backlog §1.917 (静止画のズームに魚眼レンズで近寄った変形を加える、利用者要望 2026-11-27) と
  §1.819 (平面画像を傾けて 3D 回転する表示、5ch >>816、backlog-on-hold)。
- ブランチ / worktree: `fisheye-917` / `G:\home\mimageviewer-fisheye917` (基点 master `f2e332bb8`)。
- 役割: 設計・作業指示・検収 = ClaudeCode Opus 5.5、実装 = Codex GPT-6.1 Sol / high、
  独立レビュー = 実装者とは別の GPT-6.1 Sol / xhigh (AGENTS.md)。
- 状態: 段 1 実装済み (ブランチ `fisheye-917`、独立実装レビュー I2 で「取り込み可」、§15)。利用者が試した結果を受けて **改訂 1 (余白の出ない魚眼、§16) は独立設計レビュー R10 で「実装開始可」**、実装中。改訂 1 の独立設計レビュー・実装の後に、master への取り込み・全件テスト・実機確認を統括が行う。

## 0. 利用者の決定 (2026-11-27、設計担当のセッションで直接)

| 論点 | 決定 |
| --- | --- |
| 操作の形 | **寄ると倍率と歪みが一緒に強くなる** (案 A)。1 つの操作で「レンズを持って近寄る」 |
| レンズの中心 | 最初に回したときのカーソル位置で固定 (案 A)。利用者の補足:「端で始めると後から中心をずらせないのでは。修飾キー付きのドラッグで写真だけ / レンズだけを動かせるとよい」→ **Shift+左ドラッグでレンズだけ動かす** を足す (§3.3) |
| 修飾キー | **Ctrl+Shift+ホイール** (案 A) |
| キーの既定 | 「Ctrl+Shift+. / , はなくても良いかもしれません」→ キー操作は `KeyAction` として用意するが **既定キーなし** (§3.4) |
| 書き出し・キャプチャ | 「結果を SNS にあげたいケースがあるとおもうので、最初から含めたい」→ **見えているとおりの絵を保存・コピー・書き出しできる** (§6) |
| §1.819 との関係 | 「まとめて進める」→ 設計は 1 本、実装は 2 段 (段 1 = 魚眼、段 2 = 傾け。§9) |
| 余白 (改訂 1、段 1 を試した後) | 「画像の周囲に余白がすぐにできてしまう」→ **余白の出ない範囲に歪みを自動で抑える**。画像の端が見えているときは **最初の 1 ノッチで画面を覆う倍率まで拡大** (§16) |
| 縮小しても歪みを残す (改訂 2、改訂 1 を試した後) | 「中央部分のかなり極端なズームになるケースが多くなった」「今の Ctrl+Shift+ホイールの動きはそのままに、一度魚眼モードに入った後、Ctrl+ホイール操作をすると、歪みをのこしたまま縮小する (以前の周囲がかける状態のような形になる) ようなことはできるでしょうか」→ **その時点の歪みを固定**して倍率だけ変える。対象は **通常のズーム操作すべて** (ドラッグでの移動は今のまま)。固定した後の左ドラッグは **固定した歪みのまま** (R13 の後に確認) (§17) |
| 段 2 の傾け (改訂 2 の実機確認の後) | 範囲は **制限なし、裏も見せる** (裏は反転した絵)。回転の中心は **レンズの中心の下の点**。傾けて出る余白は **許す** (倍率を自動で変えない)。遠近感は **普通のカメラ程度** (対角の画角が約 50 度。定数で調整) (§18) |

設計担当が既定で決めたこと (利用者へ報告済み、異論なし): 対象は 1 ページ表示の静止画だけ、
レンズの種類は環境設定 (既定 等距離)、ページを移ると終わる、3D 写真の深度を使った「本当に近寄る」見え方は入れない。

**利用者への説明の訂正 (R1)**: 仕様案の段階で「今の Ctrl+Shift+ホイールは普通のズームなので挙動が変わる」と説明したが誤り。
今の全画面の静止画では Ctrl+Shift+ホイールは**何もしない** (§2)。したがって魚眼を割り当てても既存の挙動は変わらない。
統括経由で利用者へ訂正を伝える。

## 1. 範囲

### 段 1 (魚眼) に含める

- 1 ページ表示の静止画 (通常フォルダ・ZIP / RAR などの書庫内・PDF のページ・RAW を含む) の全画面表示。
  MainWindow (一覧の中の閲覧)・Fullscreen・別ウィンドウ (F12、独立窓を含む) のどれでも、その viewer context の中で使える。
- Ctrl+Shift+ホイールで寄る / 離れる、Shift+左ドラッグでレンズを動かす、既定キーなしの `KeyAction` 2 つ。
- 環境設定のレンズの種類 (等距離 / 立体射影 / 等立体角)。
- 魚眼中の Ctrl+S (キャプチャ)・画像のコピー・Ctrl+E (書き出し) を、見えているとおりの絵で出す。
- shader は最初から「傾けた板との交点」の形で書き、段 1 では傾き 0 を渡す (段 2 で shader を書き直さない)。
  **→ §18.2 で置き換えた**: 段 2 では、shader の §5.1 の 1〜5 を「魚眼 → 遠近」の 2 段の合成に書き直す (R15)。

### 段 1 に含めない (理由)

| 項目 | 理由 |
| --- | --- |
| 見開き・縦横連結 | 2 枚以上のテクスチャを 1 つのレンズで描く必要がある。まず 1 ページで使い方を確かめる |
| 動画・音声・アニメーション画像 | 動画は native presenter の別経路。アニメーションは毎フレームの mip 作り直しが要る |
| タッチ・ゲームパッドからの魚眼の操作 | 今どおり普通のズーム・パン (§4.3 の touch の配送)。要望が出てから |
| 3D 写真の深度を使う透視 | 別機能の組み合わせになる。必要になれば別の段 |
| 魚眼中の Ctrl+ホイール等の基準点の補正 | §3.2 の「既知の挙動」。普通のズームの計算のまま |

### 段 2 (傾け、§1.819)

§9 にまとめる。段 1 の実装・実機確認の後に、残りの論点 (傾けられる範囲・裏面・回転の中心・透視の画角) を利用者に確かめてから着手する。

## 2. 今の作り (コード確認、master `f2e332bb8`)

- 全画面の静止画の幾何は `DisplayedImageTransform` (src/displayed_image_transform.rs) が `fs_zoom`・`fs_pan`・
  `fs_free_rotation`・90 度回転・表示トリム・フィットから 1 回で確定する ([表示パイプライン §2.4](display-pipeline.md))。
  `fs_zoom` / `fs_pan` は viewer context ごと (viewer_context_registry.rs の退避対象) で、ページを移ると
  `fs_zoom = 1.0`・`fs_pan = 0` に戻る (app.rs の画像切り替え時のリセット)。
- 一表示一投影の持ち主 `FsExclusiveView { None, Panorama, Depth }` (src/fs_exclusive_view.rs) があり、
  遷移は `App::set_fs_exclusive_view` (src/app/depth_view.rs:924) だけが行う。占有中かの集約は
  `fs_exclusive_view_active(fs_idx) -> Option<ExclusiveViewKind>` (同 951)。`FsExclusiveView` は viewer context の束の中にある。
  Depth はこの持ち主のほかに、fork 前 (viewer_context_registry.rs:4454)、独立窓の live pass 引き渡し
  (depth_view.rs:893)、ParkedLive の snapshot 前 (app.rs:71523 付近) などに専用の終了処理を持つ。
- 3D 写真 (src/depth_parallax_wgpu.rs) は egui_wgpu の callback で画面全体の三角形を描き、フラグメントの物理画素から
  「自由回転を戻した四角形の局所座標 `in_uv` → 90 度回転 → トリム → source UV」と逆にたどる
  ([3D の表示変換 §3.2](depth-view-transform-plan.md))。画像の外の判定は **source UV ではなく `in_uv`** で行う (同 44・68 行)。
  元テクスチャは `DepthGpu::copy_color` で mipmap 付きの `Rgba8Unorm` へ GPU だけで写す (egui のテクスチャは
  `Rgba8Unorm`、vendor/egui-wgpu/src/renderer.rs:782)。renderer への登録が UI 処理の後になる場合は `TexturePending`
  として 2D で描く (depth_view.rs:1470)。単ページ描画は Depth の採用を resampler の準備より前に決め、配置の確定は共通の境界を通す
  (ui_fullscreen.rs:47013・47107)。書き出しは同じ WGSL を `DepthFrameRenderer` (src/depth_parallax_wgpu/export.rs) で
  オフスクリーンに描き、描画時に記録した snapshot をページ・世代・resource ID で照合してから使う (src/app/depth_export.rs:575・741)。
- 360 度パノラマ (src/panorama.rs `PanoProjection`) に、立体射影・等距離・等立体角の表がある。ただしパノラマの係数は
  画角の定義が違い (立体射影 `k = tan(fov/4)` など、係数 2 を含まない。panorama.rs:168)、**係数はそのまま流用できない**。
  流用するのは「方式ごとに `θ` の定義域外を明示的に拒否する」書き方 (panorama.rs:214 の `θ > π` の拒否など)。
- ホイール: 共通の投影 (src/wheel_input.rs:70) が egui の `horizontal_scroll_modifier` (既定 Shift、
  vendor/egui/src/input_state/mod.rs:118) を見て、**Shift 付きの縦の回転量を横へ移す**。全画面の静止画は縦の量だけを使うので
  (ui_fullscreen.rs:42343)、**今の Ctrl+Shift+ホイールは何もしない**。Ctrl だけならズーム (`should_zoom_fullscreen_wheel`)。
  隣り合う同じ修飾のイベントは 1 つの segment に合算される (wheel_input.rs:115)。360 度は修飾に関係なくホイールを画角に使い、
  Z の全画面ズーム・分析モード・編集モードは先にホイールを取る。
- ズームの 1 段: `apply_wheel_zoom` (ui_fullscreen.rs:16205) は回転量 `wheel_y` から連続の倍率を出す
  (`WHEEL_ZOOM_POINTS_PER_NOTCH = 40` points で 1 ノッチ)。キーの段階ズーム (`FsZoomIn/Out`) は**表示中央基準** (同 15891)。
- 修飾だけの押している間の操作 (Shift = ルーペ `FsLoupeHold`、Alt = ナビゲータ、右 Ctrl = 元画像表示) は、
  **指定の修飾キーの物理状態だけを読み、ほかの修飾があっても成立する** (keymap.rs:10560・10743。`matches_only` は切り取り専用の helper)。
  したがって Ctrl+Shift を押している間もルーペの押下は成立し、右 Ctrl+Shift なら元画像表示も成立する。
- 左ドラッグ: 通常モードのドラッグは **毎フレームの `mods.ctrl`** でパン / 回転を切り替え、Ctrl 中はパンの起点を調整し直す
  (ui_fullscreen.rs:42919・42936)。開始時に所有を固定する規則ではない。
- タッチの `PageSide` のタップはマウスの「変換あり」判定を通らず直接ページを送る (ui_fullscreen.rs:42004)。
- 表示トリムの編集 (`view_trim_mode`) は、占有モードの上部バーの処理で false にされる (ui_fullscreen.rs:30692)。
- 保存の入口: Ctrl+S / リングの「キャプチャ保存」= `save_image_capture_to_file` → `prepare_capture_pixel_work` →
  worker で `capture::run_pixel_work` → `save_rgba_unique_with_matte`。Ctrl+E = `open_export_dialog` →
  `ExportComposite::{Single, Spread}` → 描いた後に `scale_export_pixels` で縮小 (export_dialog.rs:1215)。
  画像のコピー (`FsCopyImage`) は共通の pending の排他を持たず、毎回 worker を作る (ui_fullscreen.rs:58394)。
- VRAM の会計 (src/app/vram_accounting.rs:146) は `FsExclusiveView::Depth` だけを数えている。

## 3. 操作

### 3.1 状態

`FsExclusiveView` に 4 つ目の値 `PlaneView(PlaneViewState)` を足す (`ExclusiveViewKind::PlaneView` も足す)。

```rust
pub(crate) struct PlaneViewState {
    /// 入ったときの表示単位。fs_idx・項目の identity・fullscreen_page_slice (横長分割の左右半ページ) を含む。
    anchor: PlaneViewAnchor,
    /// 0 < strength <= 1。0 にする操作はこの状態を None へ戻す (strength 0 の PlaneView は作らない)。
    strength: f32,
    /// レンズの中心。表示領域 (canvas) に対する 0..1 の位置。窓の大きさが変わっても同じ割合の位置に残る。
    lens_center: egui::Vec2,
    /// 段 2 で使う。段 1 では常に 0 (shader へは渡す)。
    tilt: PlaneTilt,                    // { yaw: f32, pitch: f32 }
    /// mip 付きの色の写し (§5.2)。
    color: PlaneColorPhase,
    /// 直近に実際に描いた frame の不変 snapshot (§5.3)。保存・コピー・書き出しはこれだけを使う。
    last_drawn: Option<Arc<PlaneFrameSnapshot>>,
}

enum PlaneColorPhase {
    /// まだ写していない。描く frame は 2D (§5.2)。
    Empty,
    /// 選ばれた素材の TextureHandle はあるが renderer への登録がまだ。
    /// 前の素材 (幾何と一体) があればそれで描く (§5.2)。
    Pending { texture_id: egui::TextureId, previous: Option<Arc<PlaneMaterial>> },
    Ready(Arc<PlaneMaterial>),
}

/// GPU の写し。鍵は写し元の TextureId だけ。
struct PlaneGpuCopy {
    texture_id: egui::TextureId,
    gpu: PlaneGpuResources,             // mip の写しと、それを参照する不変の bind group (group 0、§5.3)
}

/// 素材の記述。写しと、その写しを正しく描くための幾何を一体で持つ。別々に持たない。不変。
struct PlaneMaterial {
    copy: Arc<PlaneGpuCopy>,
    source_size: [usize; 2],            // 採用時の source / sampler の寸法
    coverage: egui::Rect,               // 素材が収録する元画像の範囲 (部分素材なら全体の一部)
    sampler_roi: egui::Rect,            // native の sampler の範囲 (2D の material_source の計算と同じ入力)
    display_basis: MaterialDisplayBasis, // 採用時に受け入れた表示の基準 (PDF の page-box と raster の reference_size を含む)
    material_kind: FullscreenPageMaterialKind, // Complete / Provisional (初期配置の確定に使う)
}
```

- **鍵を 2 つに分ける**: GPU の写しは `TextureId` で照合し、`PlaneMaterial` は記述全体 (coverage・sampler の幾何・
  表示の基準・material kind) で照合する。テクスチャを変えずに PDF の表示の基準だけを更新する経路がある
  (`readopt_loaded_pdf_thumbnail_basis`、app.rs:22735 付近) ので、`TextureId` が同じでも記述が変われば、
  **写しを共有した新しい不変の `PlaneMaterial`** を作る。旧い snapshot は旧い記述を持ったまま。

- 倍率・パン・回転は **2D の状態そのもの** (`fs_zoom` / `fs_pan` / `fs_free_rotation`) を使い、PlaneView 専用の
  変換は持たない (3D 写真の [表示変換 §3.1](depth-view-transform-plan.md) と同じ判断)。ズームの持ち主は `FsZoomOwner::Image`。
- `strength` と `tilt` が両方 0 になったら PlaneView を終える (段 1 では strength だけ)。strength 0 の PlaneView は作らない。
  (改訂 1 では strength が正でも、画像の端が画面に見えているあいだは歪みが 0 になる。§16.2)
- レンズの種類は `Settings` (§3.5) にあり、状態には持たない。
- Shift+左ドラッグの所有は、既存のパン / 回転と同じ所有の型に 1 つの値として足す (§3.3)。`PlaneViewState` には持たない。

### 3.2 Ctrl+Shift+ホイール — 寄る / 離れる

**入力の取り方**: Ctrl+Shift の segment (`ctrl && shift && !alt`) だけは、共通の投影で横へ移される前の縦の回転量
(`wheel_input` の正規化済みの値) を使う。共通の投影に「軸を移さない縦の量」を返す口を 1 つ足し、パネル・ポップアップ等の
所有と消費の規則は今の segment のまま使う。チルト (物理的な横の回転) は使わない。予測と実適用は同じ量を使う。

**量**: segment の縦の量 `wheel_y` を `notches = wheel_y / WHEEL_ZOOM_POINTS_PER_NOTCH` に換算する (合算された segment・
タッチパッドの小さな量もそのまま比例で扱う。segment の数で数えない)。

- **寄る (notches > 0)**: PlaneView でなく入口条件 (§4.1) を通れば、`lens_center = カーソル位置` (表示領域の外なら中央)、
  `strength = 0` から始めて次の規則で増やし、PlaneView に入る。PlaneView 中も同じ規則:
  `strength = min(1, strength + notches / 8)` (**改訂 1 で 1/4 刻みに変更**、画面を覆う処理も足す。§16.3)。倍率は同じ `wheel_y` で `apply_wheel_zoom` を通して上げる
  (`fullscreen_wheel_zoom_step` の設定どおり)。**ズームの基準点はカーソルではなくレンズの中心**。レンズの中心は歪みの不動点で、
  そこでの倍率は 2D の倍率と同じ (§5.1) なので、2D の「基準点を固定するズーム」の計算がそのまま正しい。
  strength が 1 になった後は倍率だけ上がる。
- **離れる (notches < 0)**: PlaneView 中なら、先に使う量 `used = max(notches, −8 × strength)` を決め、
  `strength = strength + used / 8`、倍率も `used` ぶんだけ下げる (`apply_wheel_zoom` に `used × WHEEL_ZOOM_POINTS_PER_NOTCH`
  を渡す。基準点はレンズの中心)。strength が 0 になったら PlaneView を終え、残り (`notches − used`) は使わない。
  これで分割した segment と合算した segment が同じ結果になる。PlaneView でないときは何もしない (今と同じ)。
- 既存の segment は逆向きのイベントも合算するので、「分割と合算で同じ結果」は同じ向き・飽和しない範囲での性質とする (§10)。
- 入口条件を満たさないときの寄る操作は何もしない (今と同じ)。理由は既存の `show_fullscreen_nav_noop` の流儀で出し、
  同じ理由の続けての表示の扱いも既存に従う (新しい時間窓は作らない)。
- 先にホイールを取る既存のモード (360 度・Z の全画面ズーム・分析・編集モード・パネル上・ポップアップ) は今のまま
  (ここへは届かない)。

**遷移を 1 つの純粋な関数にする (同じ pass の予測と実適用の一致)**: `plane_view_wheel_transition(input) -> PlaneViewWheelOutcome`
を作る。入力は「今の PlaneView (無し / strength・中心・anchor)・今の表示単位・zoom・pan・カーソル・canvas・`notches`・入口条件の結果」、
出力は「PlaneView の有無・strength・中心・zoom・pan」。今の予測 (`predict_fs_wheel_frame`) と実適用
(`apply_fs_wheel_frame_proposal`) の両方がこれを呼び、クリックの分類 (変換の有無) もこの出力を含めて決める
(予測の結果に PlaneView の有無を足す)。

**ページ送りをまたぐ frame の規則 (組み合わせを減らす)**: 同じ frame の wheel segment の中で、ページ送りを要求する action の
**後に来た** Ctrl+Shift の量は、**予測でも実適用でも使わない** (捨てる)。キーの `FsFisheyeIn/Out` は今のキーの段
(ホイールより先に処理される。ui_fullscreen.rs:29442 → 29519) で先に適用し、この破棄の規則の対象にしない
(raw event の前後の順と処理の順が違っても、キーの段 → ホイールの段の順で決まる)。予測は今のページのまま
後続を評価するが、実適用はループの中で実際にページを移すので、ページ送りの後の魚眼の遷移は両者で食い違う
(例: 適格な静止画 A → 通常のホイールで動画 B → Ctrl+Shift 上。予測は A で入場、実適用は B で拒否)。
ページ送りの前の量は通常どおり使い、ページが移ったら §4.2 で PlaneView は終わる。クリックの分類は、その frame の
計画元の表示単位に束縛し、実適用の時点で表示単位が変わっていれば予測の結果を再利用しない。
実適用のループが途中でページを移した場合も、次の segment の前に anchor を照合し、違えば PlaneView を終える。

魚眼中のほかのズーム入力 (Ctrl+ホイール・中ボタン・Ctrl+Space のスクラブはカーソル基準、`FsZoomIn/Out` は表示中央基準。
ナビゲータは休止中) は strength を変えず、倍率だけを今の計算で変える。**既知の挙動**: 基準点がレンズの中心でないときは、
歪みのため基準点の下の絵が少しずれる。段 1 では直さない。

### 3.3 ドラッグ

| 操作 | PlaneView 中 | PlaneView でないとき |
| --- | --- | --- |
| 左ドラッグ (修飾なし) | 写真を動かす (今のパン、§1.806 の制限も今どおり) | 今どおり |
| **Shift+左ドラッグ** | **レンズだけ動かす**。写真は止まったまま、`lens_center` が画面上をドラッグ量だけ動く (表示領域の中に制限) | 今どおり (パン) |
| Ctrl+左ドラッグ | 自由回転 (今どおり) | 今どおり |
| Ctrl+Shift+左ドラッグ | 段 1: 今どおり (自由回転)。段 2 で傾けに変える | 段 1: 今どおり |

- **レンズの移動だけは、押した時点で所有を決める新しい規則**にする。押した時点で PlaneView 中かつ Shift あり・Ctrl なし
  (OS から読む既存の修飾の helper) なら、そのドラッグはレンズの移動が所有し、離すまで途中の修飾の変化 (Shift を離す・
  Ctrl を押す) でパン / 回転へ移らない。既存のパン / 回転の「毎フレームの修飾で切り替える」規則は変えない。
- 所有の表し方: 既存のパンの起点 (`fs_pan_drag_start`) と回転の起点 (`fs_rotation_drag_start`) と同時に立たないよう、
  レンズの移動の起点を同じ所有の型の 1 つの値として持つ (3 つの Option を並べない。実装担当は既存の 2 つの Option の扱いを
  変えずに済む最小の形を選び、§11 に書く)。
- 終わり: 離したとき、PlaneView が終わったとき、窓のフォーカスを失ったとき、modal が開いたとき、ページが変わったとき。
  どれもレンズの位置はその時点のまま残す。
- PlaneView 中はルーペを描かない (§4.3) ので、Shift を押してもルーペは出ない。

### 3.4 キー (`KeyAction`)

- `FsFisheyeIn` (魚眼で寄る) / `FsFisheyeOut` (魚眼で離れる): `KeyContext::FsImage`、`KeyTrigger::Press`、
  **既定キーなし** (利用者の判断)。1 回の押下は 1 ノッチ (`notches = ±1`) と同じで、§3.2 の純粋な関数を通る。
  PlaneView でないときの `FsFisheyeIn` はレンズの中心を表示領域の中央にして入る (キーにはカーソル位置の意味が無いため)。
- keymap の決まりどおり `ini_name()` / `context()` / `trigger()` / `default_chords()` (EMPTY) / `ALL_ACTIONS` /
  説明文 / 呼び出し側 helper / `docs/keymap.ini.default` / `docs/keymap-spec.md` を揃える。リング・マウスボタンの
  候補には段 1 では出さない。
- Ctrl+Shift+ホイールと Shift+左ドラッグは、今の Ctrl+ホイール・ドラッグと同じく **固定入力** (keymap の対象外)。
  `docs/keymap-spec.md` の固定入力の表に理由 (ポインター位置を使う連続操作) を書く。
- **修飾だけの押している間の操作との重なり**: Ctrl+Shift を押している間もルーペの押下 (既定 Shift) は成立するが、
  PlaneView 中はルーペを描かない。入る前の最初のノッチまでは今どおりルーペが出る (そのまま)。右 Ctrl+Shift の場合は
  元画像表示 (既定 右 Ctrl) も成立する。**元画像表示中の PlaneView は、2D がその pass に選んだ素材 (元画像) をそのまま
  レンズで描く** (素材の選び方を PlaneView 側で変えない)。保存は §6 のとおり「実際に描いた frame」なので、元画像表示中に
  保存すれば元画像の魚眼になる。割り当てを変えた場合も、PlaneView は押下の解決に関与しない (同じ規則)。

### 3.5 設定

- `Settings` に `plane_view_lens: PlaneLensKind` (`Equidistant` 既定 / `Stereographic` / `EquisolidAngle`) を足す。
  未リリースの項目なので serde の既定値で読め、移行は不要 (コミットメッセージに一言書く)。
- 環境設定の画面: 全画面の表示の設定の近くに 1 行 (置き場所は実装担当が既存の構成を見て決め、§11 に書く)。
  変えたら次の描画から効く (状態は持たないので作り直しは要らない)。
- 段 1 では strength の刻み (1 ノッチ 1/8)、最大の強さ (§5.1 の `h_min`) は定数。実機で見て詰める。
  **改訂 1**: 刻みは 1/4、最大の強さは「角の入射角 `θ_cap = 80°`」の定数 (§16.2・§16.3)。

### 3.6 終わり方

| きっかけ | 動作 |
| --- | --- |
| Ctrl+Shift+ホイール下 / `FsFisheyeOut` で strength 0 | 終わる |
| 変換中のダブルクリック (`FsCanvasClickAction::ResetTransform`) | 倍率・パン・回転と一緒に終わる |
| フィット方式の切替・リセット (`reset_fullscreen_fit_transform`) | 終わる |
| ページ移動・同じ項目の左右半ページ (slice) の移動 (どの入口でも) | 描く直前の照合 (§4.2) と、ホイールの実適用ループ内の照合 (§3.2) で終わる。各移動の関数に終了処理を足さない |
| 360 度 (V)・3D 写真 (D) に入る | `set_fs_exclusive_view` が PlaneView を捨てて切り替える |
| **Depth を終える detached / session の全経路** | 同じ helper で PlaneView も終える (下の注)。通知はしない (強さはホイールですぐ戻せる) |
| 別ウィンドウの中の F11、窓の大きさ・DPI の変更、一時的な context の mount (scoped mount) | 続ける。`lens_center` は割合なので同じ位置に残る |
| 閲覧の終了 (Esc・一覧へ戻る) | 既存の閲覧終了で終わる |

注: Depth を終える経路は、F11 (MainWindow ⇔ Fullscreen)・F12 (別ウィンドウの ON/OFF)・session の独立性の実切替の共通確定点、
fork 前 (viewer_context_registry.rs:4454)、独立窓への live pass の引き渡し (`retire_main_depth_for_independent_activation`)、
ParkedLive の snapshot 前、pause・park・terminal close。実装担当は Depth の終了処理の全出現を列挙し (§11)、
「Depth 専用の理由で終える経路」(モデル確認・書き出しの modal 等) と「表示の持ち主が変わるので終える経路」を分け、後者を
`FsExclusiveView` の占有の値 (Depth と PlaneView) に共通の helper `retire_exclusive_view_for_owner_change` へまとめる。
scoped mount で保持する経路は、Depth が保持しているものと同じにする。
共通 helper の対象は **Depth と PlaneView だけ** とし、Panorama の既存の終了・intent の保持と、Depth の再入場の通知は変えない
(独立設計レビュー R2 の合意の条件。実装レビューで確かめる)。

## 4. ほかの機能との関係 (状態の組み合わせを減らす)

### 4.1 入口条件 (1 つの関数)

`plane_view_entry_allowed(fs_idx) -> Result<(), PlaneViewEntryBlock>` を 1 つ作り、ホイール・キーが同じ答えを使う。
弾く条件: 画像の閲覧中でない、動画・音声・音楽表示、アニメーション画像、見開きの 2 ページ表示、連結読み、
wgpu の描画状態が無い、編集系のモード (消しゴム・隠蔽加工・補正レイヤー・テキスト注釈・切り取り・SNS 分割)、
**表示トリムの編集 (`view_trim_mode`)** (適用済みのトリムは弾かない)、補正パネル、分析、比較 (Wipe・差分・ピン表示)、
Z の全画面ズーム (照準・確定)、紙めくりの最中、3D 写真・360 度が有効。

- **入るときにほかのモードを閉じない** (ホイールの 1 ノッチで別のモードが閉じるのは驚きが大きい)。弾いて理由を出す。
  3D 写真の入口が補正パネル等を閉じるのとは違う判断 (あちらはキー / ボタンでの明示の入場)。
- PlaneView 中は、上部バーの占有モードの処理が `view_trim_mode` を false にする既存の分岐 (ui_fullscreen.rs:30692) に
  PlaneView を含めず、代わりに表示トリムの編集の入口を PlaneView 中は弾く (「入る時に閉じない」と揃える)。
- 3D 写真と違い、ほかの context が PlaneView を持っていても弾かない (GPU の資源を context ごとに持つため。§5.3)。

### 4.2 続けてよいかの照合

3D 写真の `depth_view_may_continue` と同じ形の `plane_view_may_continue(fs_idx)` を作り、**同じ 2 か所**
(viewer の pass の先頭、描画分岐の直前) から呼ぶ。加えて §3.2 の実適用ループの segment 間でも anchor を照合する。
入口条件のうちページと表示の状態に関わるもの (静止画・1 ページ・連結でない・編集系でない・anchor が今の表示単位) を
満たさなければ `set_fs_exclusive_view(None)` で終わる。通知はページ・slice の移動では出さず、それ以外
(同じページで見開きに切り替わった等) は 1 回出す。

### 4.3 占有中の扱い

`fs_exclusive_view_active` が `PlaneView` を返すようにし、既存の判定箇所を 3D 写真と同じ方法で分類する
(3D の [§6.5](depth-parallax-plan.md) と [表示変換の実装記録](depth-view-transform-plan.md#13-実装記録) の分類表が先例)。
**実装担当は `is_depth()`・`ExclusiveViewKind::Depth`・`fs_exclusive_view_active` の全出現を表にし (箇所・PlaneView での扱い・理由)、
§11 の実装記録に置く。** 決めておく扱い:

| 相手 | PlaneView 中 |
| --- | --- |
| ズーム・パン・自由回転・ページ内スクロール | 2D と共用 (`FsZoomOwner::Image`) |
| 修飾なしのマウスのクリック | 変換ありとして扱う (ページを送らない、画像の外のクリックで閉じない)。ダブルクリックでリセット (§3.6) |
| タッチ | `PageSide` のタップによるページ送りは止める (マウスと同じ「変換あり」)。ピンチ・2 本指のパン・上下のバーなど chrome の操作は今どおり (ピンチは倍率だけを変え strength は変えない)。入る前から続いている接触は、その接触が終わるまで今の配送のまま (接触の途中で配送を切り替えない) |
| ルーペ・ピクセルグリッド・ナビゲータ (固定表示・Alt 押下) | 描かない (カーソル ↔ 画素の対応が変わるため)。トグルの状態は変えず、抜ければ戻る |
| 紙めくり | 対象外 (`page_turn_view_eligible` を占有の述語で判定。3D と同じ)。ページ移動は紙めくり無しで切り替わり、§4.2 で終わる |
| スライドショー | 止めない。次のページで §4.2 により終わる |
| 範囲コピー | 弾く (理由: 魚眼中は使えない) |
| Ctrl+S・画像のコピー・Ctrl+E | 見えているとおりの絵 (§6) |
| フィットの循環 (`0`)・上部のメニュー | 受け付ける。フィットが変わったら終わる (§3.6) |
| 表示トリムの編集 | 入口で弾く (§4.1) |
| 透過の背景 (Shift+B) | 受け付ける。shader の下地に反映 (§5.1) |
| 元画像の押している間の表示 | 2D が選んだ素材をそのまま描く (§3.4) |
| 補正・AI 拡大・ポストフィルタの効果 | 最終の表示テクスチャをそのまま使うので効く。テクスチャが変われば mip を作り直す (§5.2) |
| 拡大方式 (シャープ / アニメ塗り / ドット絵 / Lanczos 縮小) | 効かない。PlaneView は `FullscreenPaintResource` の resampler を通らず、元の `TextureHandle` から mip を作る |
| PDF の再ラスタ | レンズの中心の倍率 = 2D の倍率なので、今の需要のまま。再ラスタで素材が変われば §5.2 の規則で写し直す |
| 左右のパネル (メタデータ等) | 今どおり |
| mIV Remote・MCP・操作のスクリプト | 端末ローカルの見た目だけで、共有する状態を変えないので対象外 |

### 4.4 別ウィンドウ (detached) — リワーク中の規則との関係

- 状態は既存の `FsExclusiveView` の中に置くので、**viewer context の退避項目を足さない**。detached の述語・viewport の経路・
  入力の配送は変えない。終了は §3.6 の注のとおり、Depth が既に持つ終了経路を「表示の持ち主の交代」の共通 helper にまとめ、
  PlaneView もそれを通す。変更範囲は「新しい値を既存の持ち主に置き、既存の終了経路をその値にも効かせる」ことに限る。
- 独立設計レビュー R1 は「既存 owner 内への追加であり、時間窓や症状 guard を足す設計ではない」として、構造上の追加という分類に
  **合意** した (§14)。ClaudeCode も同じ判断。CLAUDE.md の規則どおり、実装レビューでも合意を確かめ、結果を
  [detached-rework-plan.md](detached-rework-plan.md) に記録する。

## 5. 描画

### 5.1 式 — 魚眼のカメラで傾けた写真の板を見る

記号 (すべて物理画素):

- `canvas`: その pass の画像の表示領域 (2D の `full_rect`)。`R = 0.5 × min(canvas の幅, 高さ)`。
- `c = canvas.min + lens_center × canvas.size`。画面の点 `p` について `u = (p − c) / R`、`r = |u|`。
- レンズの式 `g` (いずれも `g(0) = 0`、`g'(0) = 1`。**パノラマの係数とは別**、§2):
  等距離 `g(θ) = θ`、立体射影 `g(θ) = 2·tan(θ/2)`、等立体角 `g(θ) = 2·sin(θ/2)`。
- 画角 (**改訂 1 で置き換え**: `k = s × k_fill`、§16.2。以下は段 1 の式で、改訂 1 の実装で削除する): `r = 1` に写る入射角 `α(s) = s × α_max`、`k = g(α(s))`。`α_max` は「地平線 (θ = 90°) の円の半径が
  `h_min = 0.8`」になる値、すなわち `g(α_max) = g(90°) / 0.8`: 等距離 **112.5°**、立体射影 **102.6804°**、
  等立体角 **124.2289°** (どれも定義域の内側)。定数として持ち、テストで固定する。
  (R0 の `α0 = 30°` の下限は削除した。`s → 0` で恒等にならず、入った瞬間に段差が出るため。§14 の P1-2)

1 画素の計算 (無効な画素も**有限な代用値**を通してから微分する):

1. 入射角 `θ = g⁻¹(k · r)`。定義域: 等距離・立体射影は `θ ∈ [0, π)`、等立体角は `k·r ≤ 2` (`θ ≤ π`)。
   外なら無効の印を立て、代用値 `θ = 0` で続ける。
2. 方向の横成分 `σ = sin θ / r` (中心 `r < ε` では解析的な極限 `σ = k`。`θ ≈ k·r` のため)。
   カメラの向き (z が前、画面の y は下向きのまま): `d = (σ · u, cos θ)`。
3. 板: カメラから距離 1 の点 `P0 = (0, 0, 1)` を通り、法線 `n = Rot(tilt) · (0, 0, 1)` の平面。
   `den = n · d`。`den ≤ ε` (板と平行・裏側・地平線の外) なら無効の印を立て、代用値 `den = ε` で続ける。
   交点 `t = (n · P0) / den`。`t ≤ 0` も無効 (代用値で続ける)。
4. 板の局所座標 `L = (Rot(tilt)⁻¹ · (t·d − P0)).xy`。傾き 0 なら `L = tan θ · u / r` (中心では 0)。
5. **2D の画面の点** `q = c + (R / k) · L`。
6. `q` を 2D の逆写像にかける: 自由回転を戻した **paint 四角形の局所座標 `in_uv`** → 90 度回転 → トリム → source UV
   (3D 写真と同じ式)。
7. **source UV → 素材の texture UV**: 2D の `SourceRegionTextureQuad` (displayed_image_transform.rs:986〜1005) と同じく、
   `material_source = normalized_sub_rect(coverage, sampler_roi)` を使って
   `tex_uv = (source_uv − material_source.min) / material_source.size` とする。`source_uv` が `coverage ∩ material_source` の外なら無効
   (部分素材が収録していない範囲。例: coverage `[.25,.75]` で source UV `.3` は texture UV `.1`、source UV `.2` は無効)。
   `coverage`・`sampler_roi` は描く素材の `PlaneMaterial` の値を uniform に渡す。
8. 色は `textureSampleGrad(tex_uv, dpdx(tex_uv), dpdy(tex_uv))`。微分は 1〜7 の非線形写像の最終結果 (`tex_uv`) に対して、
   一様な制御フローで (分岐や早い return の前に) 取る。
9. 最後に有効性の印を適用する: 1・3・7 の無効、または `in_uv ∉ [0,1]²` (**表示トリムの外を含む。source UV の範囲では判定しない**)
   なら透明 (premultiplied で 0)。透過部分には下地 (黒・白・市松) を合成する。下地は **canvas の物理座標** で計算する
   (升目の原点は 2D の画像の左上、升目の大きさは画面の値のまま)。書き出しでも出力の画素を canvas の座標へ戻してから同じ式を使うので、
   升目を出力の寸法の比で拡大縮小し直さない (§6.1)。透明の画素は egui が描いている背景がそのまま見える。

性質 (テストで固定する):

- 傾き 0 では `q = c + (p − c) · W(r)`、`W(r) = tan θ(r) / (k · r)`、`W(0) = 1`、`W` は単調増加。
  レンズの中心の倍率は 2D の倍率と同じで、周りほど広い範囲が押し込まれて見える。
- `s → 0` で `k → 0`、`W → 1` (一様に)。入った直後の 1 ノッチ目は小さな歪みから始まり、段差は出ない。
- 平らな板なので、カメラが寄ることと 2D の拡大は同じになる。寄るほど歪むのは、対象が画面の歪む部分に広がるため。
  §3.2 で倍率と強さを一緒に上げるのはこのため。
- 90 度回転・自由回転・トリム・横長分割・フィットは 6 の 2D の逆写像の中にあるので、そのまま効く。

### 5.2 shader と色の写し

- egui_wgpu の `CallbackTrait` で、callback の矩形は **canvas と painter の clip の交わり** (レンズの中心が画像の外にある場合や
  傾けた場合は、画像が 2D の四角形の外に出るため)。頂点は画面全体の三角形 (3D 写真と同じ)。
- 画面の点の取り方: uniform の「出力の画素 → canvas の物理画素」の写像 (倍率と原点) で `@builtin(position)` を canvas の座標へ
  移してから 5.1 を計算する。画面では恒等 (原点 0、倍率 1)、書き出し (§6) では出力の寸法から決める。
- 色: 2D がその pass に選んだ素材の元の `TextureHandle` を、`DepthGpu::copy_color` と同じ方法で mip 付きの `Rgba8Unorm` へ写す。
  写す処理 (blit・mip 生成) は 3D 写真と共用の関数に括り出す (WGSL・blit を 2 つ持たない)。
- **素材の採用 (通常の準備待ちとして扱う。まれな失敗ではない)**:
  - PlaneView の採用は、単ページ描画の中で resampler の準備より前に決め (3D 写真の採用と同じ位置)、パン・初期配置の確定は
    共通の境界 (`commit_fs_single_page_placement`) を通す。PlaneView 中も 2D の素材選択 (final / provisional / 元画像の
    押している間 / RAW の現像前後 / PDF の再ラスタ前後) は変えない。
  - 選ばれた素材の記述が `Ready` の `PlaneMaterial` と同じなら、それで描く。`TextureId` だけ同じで記述が違えば、写しを共有した
    新しい `PlaneMaterial` に置き換える。`TextureId` が違えば写し直して置き換える (§3.1)。
  - renderer への登録がまだ (3D 写真の `TexturePending` と同じ状況。新しく読み込んだ素材は UI の処理の後に登録される) なら
    `Pending` にし、**所有する viewport の次の pass を要求する** (`ctx.request_repaint_of(viewport)`、depth_view.rs:1470 と同じ。
    タイマーや再試行ではなく、通常の登録の境界を進めるため)。その frame は:
    - 前の素材 (`previous`) があれば、2D の変換を **前の素材の記述と今の frame の入力** で解決し直す。
      - 前の素材から取るもの: source / sampler の寸法、coverage、sampler_roi、受け入れた表示の基準、material kind。
      - 今の frame から取るもの: zoom・pan、保存された 90 度回転・自由回転、表示トリム、フィットの設定と拡大・縮小の制限、
        canvas、`pixels_per_point`、1 ページの配置 (singleton placement)。
    - **使ってよい条件は、sampler を contain する前の配置枠の一致で判定する**: 今の素材で解決した変換と、前の素材で解決した
      変換の **配置枠** (sampler の縦横比に合わせて contain し物理画素へ丸める前の枠。指定倍率では `layout_full_image_rect`、
      legacy の SourceLayout では resolver の中の中間の配置枠。実装担当は resolver からこの 1 つの値を返す口を作り、両方の基準で使う)
      が一致するとき。**一致は物理画素で測った各辺の差が 1/64 物理画素以下** とする (配置枠は `reference_size` で正規化した
      寸法に fit の倍率を掛けて作るので、同じ配置でも `f32` の誤差が残る。例: 下の page-box・canvas・倍率 1 で reference の長辺が
      `409 → 819` のとき `min.y` が `0 → −0.0000305`。raw な浮動小数点の完全一致にはしない)。この許容は比較だけに使い、
      需要・描画の値を近似しない。contain・丸めの後の `full_image_rect` では比べない (PDF の再ラスタで raster の縦横比の丸めが変わると、
      配置枠が同じでも描画矩形が 1 物理画素ずれる。例: page-box 比 `613125:791875`、canvas `1200×900`、Page fit・倍率 1 で、
      raster `317×409 → 634×819` のとき描画矩形は `696×899 → 696×900`。display_basis_tests.rs:1943 に同種の素材がある)。
      表示の基準の項目どうしも `==` で比べない。一致すれば前の素材で魚眼のまま描き、描画・配置の確定・`last_drawn`・書き出しには
      **前の素材自身の contain・丸め後の変換** を使う。一致しなければ (原寸表示・拡大の制限つきのフィットで配置が変わる場合、
      PDF の metadata が無く配置枠が raster に依存する場合など) その frame は 2D で描く。
      PDF の倍率由来の再ラスタ・RAW の現像完了・補正の完了で、魚眼の途中に歪みの無い絵が 1 frame 挟まらないようにするため
      (通常の操作で毎回起き得るので、まれな失敗ではない)。
    - `previous` が無いときも、その frame は 2D で描く。
    - **配置の確定**: 共通の `commit_fs_single_page_placement` には、**実際に描いた素材の material kind** と、その素材で解決した
      変換を渡す。PlaneView で描いたことだけを理由に Complete として扱わない (初期配置の要求の消費・保持が material kind で決まるため。
      initial_placement.rs:263)。
    - **PDF の需要**: 需要は **常に今の素材で解決した変換から作る**。需要の経路は 2 つある: 倍率から作る `maybe_rerender_pdf`
      (ui_fullscreen.rs:16241) と、指定倍率で描画後に返された変換の `layout_full_image_rect` から作る経路 (同 30032・33322・33347)。
      - **`fullscreen_page_layout` には実際に描いた素材の変換を記録する** (前の素材で描いた frame では前の素材の変換)。
        `FullscreenPageLayout` は「実際に描いたページと screen / source の変換」の持ち主で、PDF 専用ではなく、パンの制限なども
        その描画矩形を使う (displayed_image_transform.rs:1150、ui_fullscreen.rs:4139)。需要の値をここへ混ぜない。
      - 今の素材で解決した需要用の配置枠は、単ページ描画の戻り値に **描いた変換と並べて** 返し、指定倍率の需要の入口だけへ渡す
        (frame をまたぐ新しい状態は作らない)。前の素材で描いていない frame では、両者は同じ変換から取る。
  - 素材が `NativeOnly` 等で元の `TextureHandle` が無い場合は 2D で描き、PlaneView は続ける。
  - **描いた結果の記録**: 各 frame の描画の確定時に、PlaneView で描いたなら `last_drawn = Some(snapshot)`、2D で描いたなら
    `last_drawn = None` にする。2D で描いた frame の後に古い魚眼の snapshot を保存しないため (§6.1)。
  - 写しを作れない (形式が `Rgba8Unorm` でない等) ときは PlaneView を終えて理由を通知する (silent fallback にしない)。
- sampler は linear + mipmap linear。異方性フィルタは使える環境なら 16 (実装担当が wgpu の features を確かめ、無ければ無し)。
- 2D の `paint_image_underlay` は PlaneView で描いた frame では描かない (shader が描く)。2D で描いた frame は今どおり。
- WGSL は 1 つ。画面用と書き出し用 (§6) で同じ文字列を使う。CPU 版 (テスト・§6 の検証用) と 1:1 にし、naga で parse / validate
  するテストを置く (panorama_wgpu の先例)。

### 5.3 資源の持ち主と寿命

- **bind group を 2 つに分ける**: group 0 = 素材 (mip の写しの view と sampler。不変)、group 1 = uniform。
  3D 写真の bind group は uniform を同じ group に含む (depth_parallax_wgpu.rs:785) ので、そのまま流用しない。
- **共有するもの**: 不変の pipeline (**target format ごと**。画面の surface は `Bgra8Unorm` も選ばれ
  (vendor/egui-wgpu/src/lib.rs:537)、書き出しは `Rgba8Unorm` 固定のため)・blit / mip の copier。renderer ごとに `CallbackResources`
  に置いてよい。書き出しの worker には、書き出し用の format の pipeline の不変の `Arc` を渡す。
- **context ごとのもの**: `PlaneMaterial` (mip の写し・group 0 の bind group・幾何) を `Arc` で `PlaneColorPhase` が持つ。
  egui_wgpu の `CallbackResources` に context 共通の slot を置かない (3D 写真の slot は 1 context 前提。PlaneView は複数の窓で
  同時に使え、同じ renderer を共有する viewport が並ぶ)。
- **uniform は callback ごと**: 画面の callback は自分の uniform buffer と group 1 の bind group を持ち (`Arc` で callback が保持)、
  `prepare` でそれに書く。書き出しは別の uniform と group 1 を持つ (§6)。画面と書き出しで uniform を共有しない。
- **frame snapshot**: 描画で PlaneView を採用した frame で、`PlaneFrameSnapshot` (context・anchor の表示単位・
  描いた `Arc<PlaneMaterial>` (実際に描いた素材。§5.2 の前の素材で描いた frame ではそれ)・canvas の物理矩形と `pixels_per_point`・
  5.1 の全 uniform の値・下地と背景の色) を不変の `Arc` として確定し、callback と `PlaneViewState::last_drawn` が同じものを持つ。
- 解放: PlaneView を抜けたとき (`set_fs_exclusive_view` で state が drop)、写し直したとき (前の写しは、それを持つ callback・
  snapshot・保存の要求が落ちた時点で解放)。callback が持つ Arc は描画の終わりで落ちる (Lanczos の出力の寿命と同じ考え方。
  [表示パイプライン §2.4.1](display-pipeline.md))。
- **VRAM の会計**: `vram_accounting` に PlaneView の写しを足す。数えるのは実際の確保の寿命 (同じ `Arc` を複数の持ち主が持つときは
  重ねて数えない)。保存・コピー・書き出しの要求が持つ写しも、状態が終わった後に残る間は数える。
- メモリ: 写し 1 枚は表示テクスチャの約 1.33 倍 (6000×4000 で約 128 MB)。PlaneView 1 つにつき通常 1 枚。写し直しの間と、
  保存・コピー・書き出しの要求が旧い素材を持っている間は、**持たれている素材の世代の数だけ増える** (コピーは毎回 worker を作るので、
  素材が変わるたびにコピーを続けると 3 世代以上になり得る)。会計は同じ `Arc` を重ねずに数える。

### 5.4 負荷

- 1 画素あたり、三角関数数回 + `textureSampleGrad` 1 回。4K の画面 (約 830 万画素) でも 3D 写真 (1 画素約 45 サンプル) よりずっと軽い。
  描き直しは操作中だけ (今の 2D と同じ再描画の規則)。
- 写しは GPU だけで、読み戻しなし。UI スレッドで待たない。ただし CPU 側の確保・encode・submit の時間があるので、
  `perf::event` を **写し (CPU 側の時間も含む)**・書き出しの描画・読み戻しに入れる ([ui-responsiveness.md](ui-responsiveness.md) §4
  のチェックリストを通す)。数値は実装後に測って §11 に書く。

## 6. 書き出し・キャプチャ・コピー (見えているとおりの絵)

PlaneView 中だけ、3 つの出口の **元の素材を「見えているとおりの絵」に替える**。PlaneView でないときは今と同じ。

### 6.1 何を出すか — 実際に描いた frame

- 保存の操作は次の描画より前に処理されるので、要求は **`PlaneViewState::last_drawn` (直近に実際に描いた frame snapshot)** から作る。
  `last_drawn` が `None` (直近の frame を 2D で描いた。§5.2)、または snapshot の context・表示単位 (anchor) が今の PlaneView と
  一致しないときは、保存せず「魚眼の表示の準備中です」と通知する (3D 写真の照合と同じ考え方)。
- `PlaneFrameRequest` = snapshot の `Arc` + 出力の寸法 + 出力の画素から canvas の物理座標への写像。worker はこれだけを見る。
- 出力の範囲は **canvas そのもの** (上下のバー・パネル・HUD は入れない)。縦横比は canvas と同じ。共通の WGSL は
  fragment の座標 (`@builtin(position).xy`、画素の中心で既に +0.5 を含む) をそのまま使い、
  `p_canvas = canvas.min + fragment_position.xy × canvas.size / output.size` で canvas の物理座標へ戻してから §5.1 を計算する
  (画面では `output = canvas` の大きさ・原点 0 の写像を渡すので恒等。3D 写真の depth_parallax_wgpu.rs:42 と同じ扱い)。
  CPU 版で整数の画素の番号 `i` を使うときだけ `i + 0.5` を fragment の座標とする。canvas の原点が 0 でない
  MainWindow の一覧の中の閲覧でも、canvas の左上が出力の原点になる。
- 画像の外と透過部分は不透明に合成する (3D 写真の書き出しと同じ。JPEG・PNG で挙動を分けない)。画像の外の色は snapshot の背景の色。
  透過部分の下地は §5.1 の 9 のとおり canvas の座標で計算するので、出力の寸法の比で升目を拡大縮小し直さない
  (2 倍の出力では升目も 2 倍の画素になり、画面と同じ見た目になる)。

### 6.2 描画

- 3D 写真の `DepthFrameRenderer` と同じ型の `PlaneFrameRenderer`: 同じ WGSL、書き出し専用の uniform・bind group (pipeline は共有の
  不変のものでよい)、オフスクリーンの `Rgba8Unorm` に描いて読み戻す。worker が encode → submit → `poll(Wait)` する (UI スレッドは待たない)。
- **寸法の resolver を 1 つにする**: `resolve_plane_output_size(canvas_px, choice, device_limits) -> [u32; 2]`。
  device の texture の最大寸法と、行の詰め物 (256 バイト境界) を含む読み戻しの buffer の上限の両方に収まるよう、縦横比を保って縮める。
  3 つの出口とダイアログの表示はこの結果だけを使う。

### 6.3 3 つの出口

| 出口 | PlaneView 中の動作 |
| --- | --- |
| Ctrl+S / リングの「キャプチャ保存」 | `CapturePixelWork` に PlaneView の要求を足し、worker で描いて読み戻した RGBA を今の `save_rgba_unique_with_matte` へ渡す。寸法 = canvas の物理画素 (resolver を通す)。保存先・形式・既存の `capture_pending` の排他は今どおり。名前に `_fisheye` を付ける |
| 画像のコピー (`FsCopyImage` とその入口) | 同じ要求で描いた RGBA を今のクリップボードの経路へ渡す。寸法は canvas の物理画素。今のコピーの経路 (毎回 worker、共通の pending の排他なし) は変えない。要求は snapshot の `Arc` を持つだけで写しを複製しない |
| Ctrl+E (書き出しダイアログ) | `ExportComposite` に `PlaneView(PlaneFrameRequest)` を足す。基準 (ダイアログの「元の大きさ」) = canvas の物理画素の 2 倍。既存の倍率 (1/2・1/4・長辺指定) を選べ、**実際の出力の寸法 (resolver の結果) をダイアログに表示する** (上限で縮められた場合、1/2 が画面と同じとは限らないため)。PlaneView はその最終寸法へ直接描き、**既存の `scale_export_pixels` の縮小段を通さない** (二重の縮小を避ける)。隠蔽プリセットの出力 (`_1`〜`_4`) は出さない。メタデータの埋め込みは元画像の EXIF を引き継がない (見た目の作り直しのため)。ダイアログの表示は「魚眼の見た目」と分かる文言にする |

- 範囲コピーは §4.3 のとおり弾く。
- 書き出しダイアログ・保存中の扱い (モーダル・既存の pending の排他) は今の各出口のまま。
- まれな失敗 (読み戻し・保存の失敗) は既定の割り切り: ログとエラー通知だけ (CLAUDE.md)。

## 7. 設計の簡素化の検討 (CLAUDE.md)

| 検討 | 判断 |
| --- | --- |
| 魚眼専用の倍率・パンを持つか | 持たない。2D の状態を共用する (3D 写真と同じ)。持つと全ズーム入力に書き先が 2 つになる |
| 入るときにほかのモードを閉じるか | 閉じずに弾く (§4.1)。ホイール 1 ノッチの副作用を小さくする |
| ページを移っても続けるか | 続けない。倍率と同じ寿命にし、終わりの照合は描く直前の 1 関数とホイールの segment 間 (§3.2・§4.2) に集める |
| F11 / F12・fork・park で引き継ぐか | 引き継がない。Depth と同じ経路で終える。経路は共通の helper にまとめる (§3.6) |
| 見開き・連結・アニメーション | 段 1 は弾く (組み合わせを増やさない) |
| ホイールの遷移を予測と実適用で別々に書くか | 書かない。1 つの純粋な関数を両方が呼ぶ (§3.2) |
| GPU 資源を context 共通の slot に置くか | 置かない。Arc を状態・snapshot・callback が持ち、context 間の取り違えを型で無くす |
| 保存は今の状態から作るか、描いた frame から作るか | 描いた frame の snapshot から作る。保存の時点で状態から組み立て直すと、次の描画との食い違いが出る |
| 書き出しの寸法の選択肢 | 既存のダイアログの倍率をそのまま使い、PlaneView の基準と resolver だけ足す |
| strength 0 の PlaneView | 作らない (「何もしていないのに占有中」を無くす)。改訂 1 で歪みが 0 の PlaneView はあり得るが、描画の結果で状態を変えない (§16.2) |
| レンズの移動のドラッグ | 押した時点で所有を固定する。途中の修飾で切り替える組み合わせを作らない |

## 8. まれな失敗

既定の割り切り (ログ + 通知)。写しの失敗は入らない / 終わる + 通知、書き出しの失敗はエラー通知。再試行はしない。
通常の準備待ち (renderer への登録待ち・素材の差し替え) はまれな失敗ではなく、§5.2 の規則で扱う。

## 9. 段 2 — 写真の板の 3D 傾け (§1.819)

**段 2 の設計の正本は §18** (2026-11-27、利用者の決定の後)。この節は最初の案として残す。§18 と食い違うところは §18 が優先する。

- 操作: **Ctrl+Shift+左ドラッグで板を傾ける** (Hamana と同じ組み合わせ)。左右のドラッグで縦軸まわり (yaw)、
  上下で横軸まわり (pitch)。今の Ctrl+Shift+左ドラッグ (自由回転) から変わる (1 ページ表示の静止画だけ。利用者へ報告済み)。
- 状態は段 1 の `PlaneViewState::tilt`。strength 0 でも傾きがあれば PlaneView を続ける。
- shader は段 1 のまま (§5.1 の 3・4 が傾けを扱う)。書き出しも同じ snapshot に傾きが入るだけ。
- 段 2 で決めること: **透視の画角** (改訂 1 の余白なしの条件は傾き 0 の放射状の写像に依る。傾けると板の端が見えるので、傾けの余白の扱いも段 2 で決める。段 1 の式では strength 0 で `k → 0` となり透視が消えるので、傾けには strength と別の
  最小の画角が要る。strength と傾けを同時に持つときの画角の合成も含め、`s → 0` で段差を出さない形を段 2 の設計で決める)、
  傾けられる範囲 (案: 縦横それぞれ ±75 度)、裏側を見せるか、回転の中心 (レンズの中心の下の点か、画像の中心か)、
  見開きを含めるか、キーでのリセット。利用者へ確かめてから着手する。

## 10. テスト

- (改訂 1 の `k_fill`・画面を覆う処理・刻みのテストは §16.5。`α_max` と半径 `0.8 R` の地平線を固定するテストは改訂 1 で削除する)
- 式 (CPU 版): 傾き 0 で `W(0) = 1`・単調・`s → 0` で `W → 1`、3 種のレンズの `α_max` (112.5° / 102.6804° / 124.2289°)、
  地平線の判定、`θ > π` と等立体角の定義域外の拒否、中心・地平線の近くで値が有限なこと、傾き 0 の交点の式が `tan θ · u / r` と一致すること、
  トリムの外 (`in_uv ∉ [0,1]²` だが source UV はテクスチャ内) が透明になること (トリム・横長分割・90 度回転の組み合わせ)。
- WGSL: naga で parse / validate (画面用・書き出し用は同じ文字列)。CPU 版と同じ入力で係数が一致すること。
- ホイールの入力: Shift の軸の付け替えを通る**実際の handler** で、Ctrl+Shift の縦の回転が届くこと、チルトは使わないこと、
  合算された segment と分割された segment で同じ結果になること (`notches` の比例)、パネル上・ポップアップ・編集モードでは今どおり。
- 純粋な遷移関数: 入る (lens_center = カーソル)、8 ノッチで strength 1、その後は倍率だけ、下で戻って 0 で終わりその segment の残りを使わない、
  PlaneView でない下ノッチは何もしない、入口条件で弾く各条件 (見開き・連結・動画・アニメーション・3D・360・編集・表示トリムの編集・比較・
  分析・Z・紙めくり中) で何もしない。
- 同じ pass: 入場・終了だけが起きる segment で、予測と実適用の結果 (クリックの分類を含む) が一致すること。segment の間でページが移ったとき、
  次の segment が古い anchor で強さを足さないこと。同じ項目の左右半ページ (slice) の移動で終わること。
- 状態遷移: ページ移動・ダブルクリック・フィット切替・V・D で終わる。F11・F12・fork・live pass の引き渡し・park・terminal close で終わり、
  scoped mount では続く。
- ドラッグ: Shift+左ドラッグでレンズだけ動く (pan 不変)、途中で Shift を離す・Ctrl を押しても同じドラッグ、離す・PlaneView の終了・
  フォーカス喪失・modal・ページ変更で所有が終わる、PlaneView でないときは今どおり (パン / 毎フレームの Ctrl で回転)。
- 占有: ルーペ・ピクセルグリッド・ナビゲータを描かない、紙めくりの対象外、マウスのクリックとタッチの `PageSide` でページを送らない、
  ピンチは倍率だけ。Ctrl+Shift 中・右 Ctrl+Shift 中の押している間の操作の扱い (ルーペを描かない、元画像の素材をそのまま描く)、割り当てを変えた場合も同じ。
- 素材: 登録待ちで次の pass を要求すること、前の素材をその素材自身の記述で描くこと (配置の矩形が一致するとき) / 一致しない・無ければ 2D、
  素材の差し替え (PDF の再ラスタ・RAW の現像完了) で写し直す、写せない形式で終わって通知。
  - Page fit の PDF の再ラスタ (`reference_size` が変わる) では魚眼のまま描くこと。**raster の縦横比の丸めで描画矩形が 1 物理画素変わる場合** (page-box 比 `613125:791875`、canvas `1200×900`、raster `317×409 → 634×819`) も魚眼のまま描くこと。原寸表示・拡大の制限つきのフィットで配置枠が変わる場合は 2D で描くこと。
  - 指定倍率の表示で、PDF の需要 (`maybe_rerender_pdf` と `layout_full_image_rect` の経路の両方) が今の素材の変換から作られ、前の素材で描いた frame でも旧い値に戻らないこと。
  - 前の素材で描いた frame で、`fullscreen_page_layout`・`last_drawn` は前の素材の変換、指定倍率の需要は今の素材の配置枠になること。
  - 配置枠の比較の許容: 倍率 1 と 1.1、reference の長辺が違う (例 `409` と `819`) PDF の再ラスタで、`f32` の誤差があっても一致と判定されること。1 物理画素以上違う配置枠は一致としないこと。
  - 同じ `TextureId` のまま PDF の metadata (表示の基準) だけが更新された場合、写しを共有した新しい `PlaneMaterial` になり、旧い snapshot は旧い記述を持つこと。
  - 前の素材で描いた frame の配置の確定に、その素材の material kind (Provisional なら初期配置の要求を保持) が渡ること。
- キーとホイールの順: 同じ frame に `FsFisheyeIn` とページ送りのホイールがあるとき、raw event の前後に関係なく、キーの段 → ホイールの段の順で決まること。
- 出力の座標: canvas と同じ寸法の出力が、画面と同じ画素の中心を参照すること (半画素ずれないこと)。
- 部分素材: coverage が全体の一部の素材 (切り取り済みの編集プレビュー。回帰例は src/ui_fullscreen/coverage_tests.rs:16) で、
  texture UV の変換と coverage の外の除外が 2D の `SourceRegionTextureQuad` と一致すること。
- 保存の失効: 2D で描いた frame の後 (`last_drawn = None`) に保存しないこと。前の素材で描いた frame の snapshot は、実際に描いた素材を持つこと。
- ページ送りをまたぐ frame: 「通常のホイールで魚眼の対象外のページ (動画) へ移る → 同じ frame の Ctrl+Shift 上」で、予測も実適用も
  入場しないこと。クリックの分類が計画元の表示単位と違うときに再利用されないこと。
- 離れる操作の量: `strength = 1/8` で `notches = −2` を 1 つの segment で渡した場合と `−1, −1` に分けた場合で、strength・倍率が同じになること。
- 下地: 画面と 2 倍の出力で、市松の配置 (canvas の座標で見た升目の位置) が同じになること。
- pipeline: 画面の target が `Bgra8Unorm` の場合と書き出しの `Rgba8Unorm` で、それぞれの format の pipeline を使うこと。
- 会計: 3 世代以上の素材が要求に持たれている場合も、同じ `Arc` を重ねずに数えること。
- 資源: 同じ renderer を共有する 2 つの context が同時に PlaneView を持ち、互いの状態・資源・uniform を変えないこと (sibling context 不変)。
  状態が drop された後も、callback・snapshot・保存の要求が持つ写しが生きていて、それらが落ちると解放されること。VRAM の会計が重ねて数えないこと。
- 書き出し: canvas の原点が 0 でない場合、`pixels_per_point` が 1 でない場合、出力が canvas の 2 倍・1/2・長辺指定の場合に、
  オフスクリーン描画の画素が CPU 版の式の結果と一致すること (中心・周辺・地平線の外・透過の下地)。寸法の resolver が texture と読み戻しの
  上限の両方を守ること。`last_drawn` が無い / 一致しないときに保存しないこと。`scale_export_pixels` を通らないこと。
  GPU を使うテストは既存の `test-group gpu` に入れる。
- keymap: 2 つの Action の既定が空、ini の往復、競合検出、`docs/keymap.ini.default` との一致。
- UI スナップショット: 環境設定の 1 行と書き出しダイアログの文言を変えたら更新する ([ui-snapshot-policy.md](ui-snapshot-policy.md))。

## 11. 実装記録

### 実装の範囲と未検証の境界 (2026-11-27)

段 1 の差分を `fisheye-917` (着手時 `57d54274a`) に作成。段 2 の傾け入力は追加していない。
`PlaneTilt` は uniform に保持するが、製品の状態生成は常に yaw/pitch 0。
ビルド・test・check・clippy は利用者の指示により実行していない。コミット・アプリ起動も行っていない。
以下はソース上の実装記録であり、動作の成功や受入の記録ではない。

### 設計との差分 (既存コードの事実)

1. §3.2 の前提と異なり、既存の `should_handle_fullscreen_wheel` は通常パネル上の Ctrl+ホイールも通す
   (`ctrl_wheel_is_handled_even_over_panels` が既存の回帰)。通常の入力規則は保ち、PlaneView の segment の
   入場だけを既存のパネル位置の判定で拒否した。popup/modal の先行所有・消費は既存のまま。
2. §3.6 の注が列挙する `pause_mounted_background_work_keep_current_frame` そのものには、着手時には Depth の
   終了処理が無かった。上位の park 処理が終了を行っていた。段 1 では pause の入口も共通 helper を通す。
   snapshot を作る既存の順序や detached の配送は変えず、Depth/PlaneView の資源だけを終了させる。
   この追加経路を含む既存 Depth 回帰は共有 runner で確認が必要。

設計判断待ちで止めた箇所はない。追加の recovery・タイマー・待機・無条件 repaint は作っていない。
renderer 未登録の次 pass 要求は §5.2 の登録境界だけで使う。

### 状態・入力・描画・出力

- 既存の 2 つの通常ドラッグ起点を `FsCanvasDrag::Image { pan, rotation }` にまとめ、
  `PlaneLens { anchor, start, center }` と排他にした。通常の毎 frame の Ctrl による回転/パンの規則は保持する。
  レンズ所有中は通常ドラッグ・クリック・scrub へ渡さず、release pass の消費も context ごとの frame key で保持する。
  release、ページ/slice/世代変更、projection の終了、focus 喪失、modal で所有を終了する。
- `WheelSegment::vertical_before_axis_mapping` は既存の未消費 event 投影で unit を換算してから集約する。
  Shift による軸交換より前の縦だけを魚眼に渡す。予測/実適用は `plane_view_wheel_transition` を共用し、
  `used`、fractional notches、navigation 後の破棄と click anchor を揃える。
  遅延した segment 自身の page_owner も予測/実適用で照合する。キーは既存のキーの段で先に適用する。
- 設定の位置は全画面の表示の `spread/wheel-zoom-step` の直後、anchor `spread/plane-view-lens`。
  値は serde default の等距離。既定キーなしの `FsFisheyeIn/Out` を action registry、INI、表示、trigger/context、
  既定、競合、位置を変えない action の意味の網羅表へ追加した。
- `PlaneGpuCopy` は TextureId と GPU 写しの生存期間、`PlaneMaterial` は不変の記述全体を所有する。
  source サイズに加え、2D resolver の独立入力である texture サイズも記述に保持する。
  metadata の変更だけなら写しを共有し、新 TextureId なら共通 `ColorMipCopier` で blit/mip を作る。
- `DisplayedImageGeometry::placement_frame` を比較し、各辺 1/64 物理画素以内だけ前の素材を採用する。
  前の素材自身のサイズ・基準・kind と今の入力で描き直し、配置の確定には実際の kind を渡す。
  `FsSinglePageDraw { transform, pdf_demand_rect }` で実描画と現在素材の PDF 需要を分け、
  `fullscreen_page_layout` と snapshot に需要用の変換を混ぜない。
- 画面/出力は `PLANE_SHADER` を共用する。素材 group 0、private uniform group 1、target format 別 pipeline。
  CPU 参照式は `camera_point` / `texture_point`。下地と出力は canvas の物理座標で写す。
  登録待ちで素材が無い/配置が違う/NativeOnly の frame は既存 2D へ描画を戻し、last_drawn を消す。
- callback/snapshot/出力要求は Arc で実際に描いた素材を保持する。出力要求は context とページ/slice anchor を照合する。
  保存・コピーは canvas、Ctrl+E は canvas 2 倍から最終選択を解決し、texture と padded readback buffer の両上限を守る。
  worker が描画と readback を行い、PlaneView は追加の `scale_export_pixels` を通らない。
  書き出しは現在の見た目 1 種、basename `_fisheye`、元 EXIF を引き継がない。
- VRAM は写しごとの RAII allocation を `VramSubsystem::PlaneView` に一度加算する。
  3 世代以上と、状態終了後に callback/保存要求だけが保持する写しも計上し、同じ Arc を重複しない。
  renderer cache は pipeline/copier だけで、context 共通の可変素材や uniform は持たない。

### 表示所有者の交代と終了処理の振り分け

| 境界 | 段 1 の扱い | 既存の通知/他モード |
| --- | --- | --- |
| `commit_viewer_session_deferred_feedback` (F11/F12/session 確定) | `retire_exclusive_view_for_owner_change` | was_depth のときだけ既存再入場案内。PlaneView は追加通知なし |
| `retire_main_depth_for_independent_activation` (live pass 引渡し) | 同 helper | setup の無効化は従来どおり。main の条件/配送は変更なし |
| `split_current_context_preserving_main_grid_payload` (両 fork) | identity を動かす前に同 helper | Panorama の state/intent の既存 transfer は維持 |
| `reset_detached_pause_foreground_modes` (park) | 同 helper、後段の Panorama 終了は従来どおり | Panorama intent を残す |
| `pause_mounted_background_work_keep_current_frame` | 同 helper を追加 | 上記「設計との差分」参照。sibling は触らない |
| parked live media の snapshot 作成/退避 (`park_active_live_media`) | 同 helper | snapshot/viewport の順序は維持 |
| `close_fullscreen` と `ViewerContextBundle::cancel_all_context_work`/Drop | 既存の全 variant の終了に含まれる | bundle の terminal は App を mount せず自分の所有物だけ drop |
| `depth_view_may_continue` / D の終了 / Depth panel の閉じる | Depth 固有のまま | PlaneView は独自の継続照合・入力の終了を使う |
| scoped mount / `with_viewer_context` | 終了しない | 状態をその owner の bundle に往復するだけ |
| PlaneView の強さ 0・ダブルクリック・fit reset・ページ/slice 不一致 | `set_fs_exclusive_view(None)` | ページ/slice は無通知、それ以外の継続拒否は1回通知 |
| V / D の新 projection の採用 | `set_fs_exclusive_view` が前の所有物を drop | Panorama/Depth の固有通知の規則は変更しない |

### 占有判定の全出現の分類

対象は `src/**/*.rs` の呼び出し/variant/定義。行番号はこの実装差分のもの。
同一行に複数の対象表記がある場合は1行にまとめる。試験コードも別表に全行を列挙する。
`is_depth` の意味自体は変えない。PlaneView を含める境界と、Depth 固有の境界を区別する。

| 箇所 | 表記 | PlaneView の扱い・理由 |
| --- | --- | --- |
| `src/app/depth_view.rs:821` `depth_slideshow_prefetch_active` | `is_depth()` | Depth 固有の計算先読みのみ。PlaneView は既存の slideshow。 |
| `src/app/depth_view.rs:872` `depth_slideshow_gate` | `is_depth()` | Depth 固有の計算待ちのみ。PlaneView は既存の slideshow。 |
| `src/app/depth_view.rs:950` `fs_exclusive_view_active` | `fs_exclusive_view_active` | PlaneView variant を返す。Depth 判定の意味は維持。 |
| `src/app/depth_view.rs:952` `fs_exclusive_view_active` | `ExclusiveViewKind::Depth` | PlaneView variant を返す。Depth 判定の意味は維持。 |
| `src/app/depth_view.rs:969` `depth_entry_allowed` | `is_depth()` | 他 context の Depth だけを拒否。PlaneView は sibling の Depth/PlaneView を妨げない。 |
| `src/app/depth_view.rs:998` `depth_view_may_continue` | `is_depth()` | Depth の継続/通知だけ。PlaneView は独自の継続照合。 |
| `src/app/depth_view.rs:1077` `toggle_depth_parallax` | `is_depth()` | Depth のトグル終了だけ。PlaneView からの D は新 Depth 採用時に置き換える。 |
| `src/app/gamepad_input.rs:6143` `change_ring_picker_value` | `is_depth()` | Depth の fit 制限だけ。PlaneView は通常の fit 入力へ。 |
| `src/app/gamepad_input.rs:6363` `preview_image_picker_row` | `is_depth()` | Depth の fit 制限だけ。PlaneView は通常の fit 入力へ。 |
| `src/app/gamepad_input.rs:6561` `enter_ring_picker_list_for_current_row` | `is_depth()` | Depth の fit 制限だけ。PlaneView は通常の fit 入力へ。 |
| `src/app/gamepad_input.rs:6684` `set_ring_picker_row_value_selection` | `is_depth()` | Depth の fit 制限だけ。PlaneView は通常の fit 入力へ。 |
| `src/app/gamepad_input.rs:7750` `apply_image_picker_state` | `is_depth()` | Depth の fit 制限だけ。PlaneView の fit 確定は共通 setter で終了。 |
| `src/app/gamepad_input.rs:8469` `apply_ring_action` | `fs_exclusive_view_active` | Depth 固有の panel/キー制限を維持。PlaneView は通常入口、Z は共通入口、比較へ変われば継続照合で終了。 |
| `src/app/gamepad_input.rs:8470` `apply_ring_action` | `ExclusiveViewKind::Depth` | Depth 固有の panel/キー制限を維持。PlaneView は通常入口、Z は共通入口、比較へ変われば継続照合で終了。 |
| `src/app/page_overview.rs:225` `page_overview_unavailable_reason` | `is_depth()` | 占有に含めページ一覧との同時表示を拒否 (既存の集約した理由の経路)。 |
| `src/app/page_overview.rs:233` `page_overview_unavailable_reason` | `fs_exclusive_view_active` | 占有に含めページ一覧との同時表示を拒否 (既存の集約した理由の経路)。 |
| `src/app/plane_view.rs:143` `plane_view_entry_allowed` | `is_depth()` | Depth/360 を入口で拒否。自分の PlaneView は継続可。 |
| `src/app/plane_view.rs:188` `retire_exclusive_view_for_owner_change` | `is_depth()` | Depth の戻り値は既存通知のためだけ。実際の終了は Depth/PlaneView を共通に扱う。 |
| `src/app.rs:54666` `fullscreen_adjustment_panel_draw_reachable` | `fs_exclusive_view_active` | 占有として背面の補正/分析描画を抑止。パネル状態が変われば継続照合で終了。 |
| `src/app.rs:54670` `fullscreen_adjustment_panel_draw_reachable` | `fs_exclusive_view_active` | 占有として背面の補正/分析描画を抑止。パネル状態が変われば継続照合で終了。 |
| `src/app.rs:63334` `notify_depth_slideshow_folder_nav_exit` | `is_depth()` | Depth 固有の再入場通知だけ。PlaneView は通常の slideshow を継続し次ページで終了。 |
| `src/app.rs:82668` `resume_original_material_entry` | `fs_exclusive_view_active` | 編集用の原寸素材の遅延再入場を占有中は拒否。元画像ホールドの選択とは別の経路。 |
| `src/app.rs:82769` `page_edit_target_switch_is_current` | `fs_exclusive_view_active` | 占有中の編集対象の再入場を拒否。 |
| `src/app.rs:95267` `pdf_native_rerender_pending` | `is_depth()` | Depth のみ倍率を除外。PlaneView は通常の PDF 倍率需要を維持。 |
| `src/app.rs:98949` `panorama_entry_allowed` | `is_depth()` | Depth 固有の拒否を維持。PlaneView は既存の V 入口へ進み採用時に終了。 |
| `src/app.rs:99366` `apply_panorama_mode_toggle` | `is_depth()` | Depth 固有の拒否を維持。PlaneView は Panorama の採用で置き換える。 |
| `src/ui_fullscreen.rs:15378` `fs_zoom_mode_context_ok` | `fs_exclusive_view_active` | PlaneView も新しい Z モードの入口の占有に含める。 |
| `src/ui_fullscreen.rs:15449` `fs_zoom_reset_for_blocked_context` | `is_depth()` | PlaneView を含め、退避中の Z と PDF の倍率需要を破壊しない。 |
| `src/ui_fullscreen.rs:15843` `fs_zoom_owner` | `is_depth()` | PlaneView は通常の Image の倍率/パンを共用。 |
| `src/ui_fullscreen.rs:16312` `maybe_rerender_pdf` | `is_depth()` | Depth の抑止だけ。PlaneView は倍率の需要を続ける。 |
| `src/ui_fullscreen.rs:23227` `similar_preview_session` | `fs_exclusive_view_active` | 占有に含め、2D の画素位置を使う類似プレビューを出さない。 |
| `src/ui_fullscreen.rs:26262` `fs_front_menu_presented_for_viewport` | `fs_exclusive_view_active` | PlaneView は占有の制限から除き、通常の fit/回転メニューを使う。 |
| `src/ui_fullscreen.rs:26343` `still_info_panel_eligible_for_idx` | `fs_exclusive_view_active` | PlaneView は Panorama/Depth のパネル抑止から除き、情報パネルを使う。 |
| `src/ui_fullscreen.rs:29328` `render_fullscreen_viewport_with_root_input` | `fs_exclusive_view_active` | 占有として分析用の PDF 矩形を選ばず、通常の PDF 需要を使う。 |
| `src/ui_fullscreen.rs:29362` `render_fullscreen_viewport_with_root_input` | `fs_exclusive_view_active` | 占有として分析用の PDF 矩形を選ばず、通常の PDF 需要を使う。 |
| `src/ui_fullscreen.rs:29715` `render_fullscreen_viewport_with_root_input` | `fs_exclusive_view_active` | PlaneView は通常の上部メニュー/overflow/情報パネルを許可する例外。Depth/Panorama の制限は維持。 |
| `src/ui_fullscreen.rs:29981` `render_fullscreen_viewport_with_root_input` | `is_depth()` | Depth の素材採用/専用パネル/ボタン状態だけ。PlaneView は別の描画・通常パネル。 |
| `src/ui_fullscreen.rs:30613` `render_fullscreen_viewport_with_root_input` | `is_depth()` | Depth の素材採用/専用パネル/ボタン状態だけ。PlaneView は別の描画・通常パネル。 |
| `src/ui_fullscreen.rs:30765` `render_fullscreen_viewport_with_root_input` | `fs_exclusive_view_active` | PlaneView は通常の上部メニュー/overflow/情報パネルを許可する例外。Depth/Panorama の制限は維持。 |
| `src/ui_fullscreen.rs:30863` `render_fullscreen_viewport_with_root_input` | `is_depth()` | Depth の素材採用/専用パネル/ボタン状態だけ。PlaneView は別の描画・通常パネル。 |
| `src/ui_fullscreen.rs:31966` `fs_page_turn_ordinary_context_blocker` | `fs_exclusive_view_active` | 占有に含め紙めくりを抑止。通常のページ移動は継続。 |
| `src/ui_fullscreen.rs:33423` `reconcile_specified_pdf_demand_with_single` | `is_depth()` | Depth のみ需要を抑止。PlaneView は現在素材の別戻り値で需要を継続。 |
| `src/ui_fullscreen.rs:35773` `apply_still_reader_escape` | `fs_exclusive_view_active` | 占有中の Esc は既存の閲覧終了へ。 |
| `src/ui_fullscreen.rs:35946` `begin_page_turn_viewer_frame` | `is_depth()` | Depth 固有の閉じる通知。PlaneView は追加通知なしの既存終了。 |
| `src/ui_fullscreen.rs:37418` `handle_fs_key_input_with_operation` | `fs_exclusive_view_active` | 占有キー制限に含む。slideshow/背景は Panorama 固有、metadata は PlaneView で通常パネル、Depth focus/panel は Depth のみ。 |
| `src/ui_fullscreen.rs:37427` `handle_fs_key_input_with_operation` | `fs_exclusive_view_active` | 占有キー制限に含む。slideshow/背景は Panorama 固有、metadata は PlaneView で通常パネル、Depth focus/panel は Depth のみ。 |
| `src/ui_fullscreen.rs:37436` `handle_fs_key_input_with_operation` | `is_depth()` | 占有キー制限に含む。slideshow/背景は Panorama 固有、metadata は PlaneView で通常パネル、Depth focus/panel は Depth のみ。 |
| `src/ui_fullscreen.rs:38174` `handle_fs_key_input_with_operation` | `is_depth()` | 占有キー制限に含む。slideshow/背景は Panorama 固有、metadata は PlaneView で通常パネル、Depth focus/panel は Depth のみ。 |
| `src/ui_fullscreen.rs:38940` `fs_navigator_allowed_for_display_unit` | `is_depth()` | PlaneView も固定表示/Alt ナビゲータを抑止。設定のトグルは保持。 |
| `src/ui_fullscreen.rs:41316` `handle_fs_wheel_and_click_frame` | `fs_exclusive_view_active` | PlaneView は通常の左右パネル。Depth panel の矩形/フォーカスだけは Depth 固有。 |
| `src/ui_fullscreen.rs:41598` `handle_fs_wheel_and_click_frame` | `is_depth()` | PlaneView は通常の左右パネル。Depth panel の矩形/フォーカスだけは Depth 固有。 |
| `src/ui_fullscreen.rs:43642` `plan_fs_canvas_click` | `is_depth()` | Depth focus/閉じるパネルは Depth のみ。PlaneView は別の plane_active で変換ありに分類。 |
| `src/ui_fullscreen.rs:43693` `plan_fs_canvas_click` | `is_depth()` | Depth focus/閉じるパネルは Depth のみ。PlaneView は別の plane_active で変換ありに分類。 |
| `src/ui_fullscreen.rs:44201` `advance_slideshow` | `is_depth()` | Depth の先読み待ちだけ。PlaneView の slideshow は通常どおり。 |
| `src/ui_fullscreen.rs:44492` `current_slideshow_frame_ready` | `is_depth()` | Depth の ready 判定だけ。PlaneView の slideshow は通常どおり。 |
| `src/ui_fullscreen.rs:44523` `sync_slideshow_anchor_for_frame` | `is_depth()` | Depth の input-only 描画待ちだけ。PlaneView は通常のタイマー。 |
| `src/ui_fullscreen.rs:45116` `complete_book_slideshow_target` | `is_depth()` | Depth の専用 target 準備だけ。PlaneView は通常のページ移動後に終了。 |
| `src/ui_fullscreen.rs:45749` `fullscreen_edit_mode_entry_allowed` | `is_depth()` | PlaneView はこの分岐より前に魚眼の理由で拒否。Depth/Panorama の理由は維持。 |
| `src/ui_fullscreen.rs:45753` `fullscreen_edit_mode_entry_allowed` | `fs_exclusive_view_active` | PlaneView はこの分岐より前に魚眼の理由で拒否。Depth/Panorama の理由は維持。 |
| `src/ui_fullscreen.rs:46667` `handle_fs_navigation_with_render_perf` | `is_depth()` | Depth の素材先読みだけ。PlaneView の通常 navigation は保持し継続照合で終了。 |
| `src/ui_fullscreen.rs:47185` `draw_fs_image_with_initial_placement` | `is_depth()` | Depth の需要復帰の境界は維持。PlaneView は PDF 需要を止めず別の shader 採用を行う。 |
| `src/ui_fullscreen.rs:48347` `set_fullscreen_fit_mode_for_current` | `is_depth()` | Depth の変更拒否のみ。PlaneView は変更を受け付け reset で終了。 |
| `src/ui_fullscreen.rs:48396` `cycle_fullscreen_fit_mode` | `fs_exclusive_view_active` | Depth の変更拒否のみ。PlaneView は循環を受け付け reset で終了。 |
| `src/ui_fullscreen.rs:48397` `cycle_fullscreen_fit_mode` | `ExclusiveViewKind::Depth` | Depth の変更拒否のみ。PlaneView は循環を受け付け reset で終了。 |
| `src/ui_fullscreen.rs:52925` `draw_fs_loupe_if_active` | `fs_exclusive_view_active` | 占有に含め、割当/物理修飾にかかわらずルーペを描かない。 |
| `src/ui_video_tile.rs:69` `toggle_video_tile_mode` | `fs_exclusive_view_active` | 占有に含め、静止画の projection から動画タイルを開始しない。 |
| `src/ui_view_trim.rs:993` `view_trim_original_request_is_current` | `fs_exclusive_view_active` | 占有中に表示トリム編集の遅延再入場を行わない。 |

試験コードの全出現 (各行の期待は Depth 固有のまま。既存テストの期待を PlaneView へ広げていない。追加の PlaneView 試験は占有を検査):

| 箇所 | 対象行 (各行を列挙) | PlaneView の扱い・理由 |
| --- | --- | --- |
| `src/app/book_composition_permission_tests.rs` | `939` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/app/collection_navigation/depth_slideshow_tests.rs` | `158`, `237`, `306` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/app/depth_export_main_window_tests.rs` | `300`, `741` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/app/depth_main_window_tests.rs` | `584`, `690`, `992`, `1117`, `1121` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/app/depth_setup_entry_tests.rs` | `38`, `64`, `125`, `134`, `205`, `225`, `230`, `244`, `275`, `301`, `336`, `340`, `349` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/app/depth_view.rs` | `664`, `726` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/app/depth_view_tests.rs` | `699`, `700`, `704`, `749`, `752`, `883`, `945`, `1366`, `1493`, `1647`, `1648`, `1689`, `1826`, `1889`, `2093`, `2098`, `2127`, `2212`, `2227`, `2703`, `2755`, `2997`, `3074`, `3122`, `3189`, `3254`, `3350`, `3386`, `3419` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/app/detached_media_exchange_tests.rs` | `219` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/app/multiwindow_scenario_tests.rs` | `1017` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/app/tests/startup_restore.rs` | `822` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/app/tests.rs` | `100596`, `100608` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/app.rs` | `110895`, `110907`, `110986`, `111046`, `111049`, `111125` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/ui_fullscreen/depth_slideshow_tests.rs` | `826`, `1036`, `1052`, `1145`, `1189` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/ui_fullscreen/depth_view_transform_tests.rs` | `267`, `283` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/ui_fullscreen/page_turn.rs` | `2003` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |
| `src/ui_fullscreen/plane_view_tests/gpu_tests/additional_coverage.rs` | `148` | PlaneView の占有を維持し、元画像ホールドでも専用の描画を続ける。 |
| `src/ui_fullscreen/plane_view_tests.rs` | `545` | Depth 回帰の対象を維持。新規 PlaneView の置換試験では Depth への採用を検査。 |

上記は呼び出し等 143 行 (製品 67、試験 76) と定義1件。

### 変更ファイル一覧

全パスを列挙する。既存ドラッグ/クリック/配置の回帰ファイルは、所有型の accessor 化、新しい引数/variant、戻り値の `.transform` 取得へ合わせ、既存期待値は維持する。

- `docs/architecture-overview.md`
- `docs/detached-rework-plan.md`
- `docs/display-pipeline.md`
- `docs/fisheye-plane-view-plan.md`
- `docs/fullscreen-double-click-close-plan.md`
- `docs/gpu-test-groups.md`
- `docs/key-customization-impl-plan.md`
- `docs/keymap-spec.md`
- `docs/keymap.ini.default`
- `docs/next-release-backlog.md`
- `docs/settings-export-import-plan.md`
- `docs/spec.md`
- `docs/ui-responsiveness.md`
- `docs/ui-snapshot-policy.md`
- `htdocs/mimageviewer/index.html`
- `htdocs/mimageviewer/manual/fullscreen.html`
- `src/app.rs`
- `src/app/depth_view.rs`
- `src/app/plane_view.rs`
- `src/app/search_nav_entry_inventory_tests.rs`
- `src/app/tests.rs`
- `src/app/viewer_context_registry.rs`
- `src/app/vram_accounting.rs`
- `src/books.rs`
- `src/capture.rs`
- `src/context_menu_model.rs`
- `src/depth_parallax_wgpu.rs`
- `src/displayed_image_transform.rs`
- `src/export_dialog.rs`
- `src/fs_exclusive_view.rs`
- `src/gpu_color_copy.rs`
- `src/keymap.rs`
- `src/lib.rs`
- `src/plane_view.rs`
- `src/plane_view_wgpu.rs`
- `src/plane_view_wgpu/export.rs`
- `src/plane_view_wgpu/gpu_tests.rs`
- `src/settings.rs`
- `src/settings_transfer.rs`
- `src/settings_transfer/stage1.rs`
- `src/touch_input.rs`
- `src/ui_adjustment_panel/click_timing_1679_tests.rs`
- `src/ui_conceal.rs`
- `src/ui_crop.rs`
- `src/ui_dialogs/preferences/pages.rs`
- `src/ui_dialogs/context_menu.rs`
- `src/ui_dialogs/context_menu/plane_copy_tests.rs`
- `src/ui_dialogs/preferences/search_index.rs`
- `src/ui_erase.rs`
- `src/ui_fullscreen.rs`
- `src/ui_fullscreen/depth_focus_tests.rs`
- `src/ui_fullscreen/depth_tests.rs`
- `src/ui_fullscreen/depth_view_transform_tests.rs`
- `src/ui_fullscreen/double_click_close_tests.rs`
- `src/ui_fullscreen/double_click_close_tests/suppression_906_tests.rs`
- `src/ui_fullscreen/in_page_scroll/drag_bound_handler_tests.rs`
- `src/ui_fullscreen/in_page_scroll/drag_bound_input_tests.rs`
- `src/ui_fullscreen/in_page_scroll/handler_tests.rs`
- `src/ui_fullscreen/in_page_scroll/wheel_zoom_tests.rs`
- `src/ui_fullscreen/initial_placement/tests.rs`
- `src/ui_fullscreen/initial_placement/tests/viewport_input/canvas_click.rs`
- `src/ui_fullscreen/overlay_navigation_tests.rs`
- `src/ui_fullscreen/page_turn/prepare_boundary_tests/pan_bounds_tests.rs`
- `src/ui_fullscreen/page_turn/resident_placement_tests.rs`
- `src/ui_fullscreen/page_turn/wait_indicator_tests.rs`
- `src/ui_fullscreen/pan_bounds/tests.rs`
- `src/ui_fullscreen/plane_view_tests.rs`
- `src/ui_fullscreen/plane_view_tests/gpu_tests/additional_coverage.rs`
- `src/ui_fullscreen/plane_view_tests/gpu_tests/transition_and_click_tests.rs`
- `src/ui_fullscreen/side_panel_secondary_input_tests.rs`
- `src/ui_sns_split.rs`
- `src/vram_budget.rs`
- `src/wheel_input.rs`

### 共通 runner のコンパイル診断への修正 1

設計担当から受領した `cargo check -p mimageviewer --bin mimageviewer-core --tests --message-format short`
(exit 101、lib test 28 errors) に対応する差分を追加した。実装担当による cargo check/test の再実行はしていない。

- 既存の単ページ描画試験は、結果の `.transform` を取り出して従来の寸法・配置・pan の全検査を維持する。
- shader 検証は既存の依存の再公開 `wgpu::naga` を使う。
- 設定転送には `plane_view_lens` を通常の enum と同じ検証付きで追加し、網羅の分解・件数・変化検査・往復/欠落/不正値の試験を揃えた。
- `PlaneEntry` は RawDevelopment の通知と同じく、拒否の具体的な理由を title に表示し、不要な補足文は付けない。
- キーの geometry 計算は seek geometry を先にローカルへ確定する。レンズ入力の上部バー判定は、既存の panel 更新と top_bar_visible の計算後へ移す。clone は追加しない。
- scoped mount の試験は既存の `with_window_context_for_test` を使い、可視性は広げない。
- texture delta は TextureId ごとの SmallVec を順に renderer へ渡す。レンズの frame ID は生成/参照とも `Id::unique` に揃える。

### 共通 runner のコンパイル診断への修正 2

`PlaneLensKind` の `Stage1Value` に3候補を追加し、表示名は環境設定と共通の `label()` を使う。
試験の `TexturesDelta` は `set` を `std::mem::take` で取り出し、Drop のある所有者から直接 move しない。
実装担当による cargo check/test の再実行はしていない。

### 関連試験の失敗への修正 3

設計担当から、共通実行口の cargo check 成功と、関連 nextest 1577件中1566成功・11失敗の報告を受領した。
下表はコードを照合した原因と変更。修正後の cargo check/test は実装担当では実行していない。
PNG の生成は設計担当へ引き継いだまま。レビューの判定は記録していない。

| 失敗 | 原因の分類 | 変更・維持する検査 |
| --- | --- | --- |
| stage1 の全キー/anchor | 試験の旧件数 | export 173、rows 230 へ更新。全キー/重複/anchor 検査は残し、レンズ3候補の wire 値と環境設定と同じ表示名も検査する。 |
| real_preferences_controls_ok_persistence | 製品の accessible name 欠落 | レンズ ComboBox の Response を表示ラベルへ labelled_by で結び付ける。accessibility check は有効のまま。 |
| preferences_wheel_zoom_ok_save | 同じ製品の欠落 | 同じ共通 renderer の修正が適用される。既存の入力/保存検査は変更しない。 |
| plane_view_lens_settings_light | 同じ製品の欠落 | 同じ labelled_by。PNG は未生成なので設計担当が生成・目視・通常比較する。 |
| plane_view_lens_settings_dark | 同じ製品の欠落 | 同上。accessibility check は有効のまま。 |
| plane_export_size_choices_snapshot_dark | 試験成果物が未生成 | コード/検査は変更しない。PNG の生成・目視・通常比較は設計担当。 |
| plane_key_stage_precedes_wheel | 試験の位相の前提 | viewer の navigation は画像の描画後、frame tail で確定する。旧静止画でキーが先に strength 1/8 を作った証拠を検査し、次の pass の描画前の照合で終了することを検査する。raw event の両順と同一 pass の下ホイールで終了する検査は残す。終了を個々の移動関数へ追加しない。 |
| plane_lens_owner_survives_modifiers | 試験の focus 前提 | 初回 pass が作る300msの focus-restoring click 猶予が press を拒否していた。既存 drag 試験同様に fs_focus_regained_at を消して既に focused の press を作る。既存 synthetic timeline を使い、handler 前の物理 Shift/非Ctrl/primary press も検査する。owner を試験側で直接作らない。 |
| plane_pending_pdf_reraster | 試験の旧素材との比較 | legacy の layout_full_image_rect は contain/丸め後なので再ラスタで変わる。需要は現在素材の独立した resolver 結果との完全一致、描画は前素材、比較の配置枠は1/64物理画素以内と検査する。旧素材の需要と違う fixture であることも検査する。 |
| plane_deferred_segment | 試験で直接起きた再帰 lock / 製品にも同型あり | 試験の ctx.input の内側の ctx.options を外へ出す。製品のレンズ frame 記録も ctx.data_mut 内の ctx.cumulative_frame_nr を外へ出す。 |
| plane_same_pass_prediction | 試験で直接起きた再帰 lock / 製品にも同型あり | 同じ内側の ctx.options を先に取得する。製品の semantic input も frame key・frame number を ctx.data の外で確定する。予測/実適用/クリックの一致検査は維持。 |

`edit_request_owner_context` は App の context table を読むだけで egui Context は読まないが、semantic input の key も closure 前へ移した。
`plane_view_wheel_transition` は Context を持たない計算であり、予測・適用側の Context 読みと切り離している。
既存 `egui_context_closures_do_not_hand_ctx_to_helpers` は `(ctx)` / `(ctx,` の引数渡しだけを調べ、
`ctx.cumulative_frame_nr()` / `ctx.options(...)` の直接呼び出しを検出しない。opener も `ctx.input(` などの
連続した文字列なので `ctx` と `.input` の間の改行を見落とす。
新規 adapter と harness の直接再帰/改行形を検出する補助試験
`plane_context_lock_closures_do_not_read_context_again` を追加した。旧走査は残す。

### 残りの検索索引と未使用警告への修正 4

環境設定の `spread/plane-view-lens` を検索索引へ追加し、表示と同じ「魚眼のレンズ」、3候補名、魚眼ズーム・Ctrl/Shift/ホイール等を検索語にした。anchor の網羅検査は維持し、個々の検索語から同じ項目が見つかる回帰試験を追加。

**設計との差分 (表現の整理)**: 未使用だった Pending の `texture_id` と snapshot の独立した `pixels_per_point` を削除した。Pending の採用判定は毎 pass の現在の TextureId を使い、snapshot の描画・出力は凍結済みの物理座標 uniform を使う。素材の所有・DPI・出力寸法の計算は変えていない。未使用の `PlaneColorPhase::material()` も削除した。

CPU 参照式 (`theta` / `camera_point` / `texture_point`) と検査用の pipeline format は `#[cfg(test)]` に限定した。GPU の寿命を保つ所有 field は `MipColor::_texture` と `PlaneGpuResources::_color` に改名し、所有の理由をコメントした。資源の保持・VRAM の会計は維持する。`STAGE1_MARKED_ANCHORS` は着手時の HEAD から同じ宣言・参照だったので触れていない。

修正後の cargo check/test は実装担当では実行していない。未生成の3枚のPNGは設計担当が生成する。

### コピー配送・入力の副作用・境界試験への修正 5

対象の基点は `6c83a526d`。cargo の実行・コミットは実装担当では行っていない。

- Fullscreen の `MenuCommand::CopyImageToClipboard` で PlaneView を判定し、既存の `copy_image_capture_to_clipboard` → `prepare_capture_pixel_work` → `plane_frame_request(Canvas)` へ配送する。キーの `FsCopyImage`、右クリックの native/egui メニュー、リング (ゲームパッドを含む) はこの配送点を共有する。snapshot が無い・context/anchor が違う場合は準備中の通知とし、元ファイルのコピーへ戻さない。PDF は Fullscreen の PlaneView のときだけ画像コピーの候補に加える。Grid と通常 Fullscreen の適格性と処理は維持する。
- コピー試験は実際の action・メニューコマンド・リングから worker 入口まで通す。試験で明示的に有効にした thread-local sink だけが `CapturePixelWork` を受け取り、OS のクリップボードへは書かない。snapshot の Arc、Canvas の出力寸法、未描画・context/slice の不一致を検査する。
- キーとホイールの成功時の適用を `apply_plane_view_outcome` にまとめた。拒否は通知だけ、pose/zoom/pan が同じ no-op は何も書かない。実際に表示が変わるときだけ pan の clamp、初期配置要求の取消、PDF の需要更新を行う。予測も同じ差分条件で clamp する。古い anchor の pose は純粋な遷移と同じく比較の基準から外し、古い owner の終了を新ページのユーザー変換として扱わない。初期配置要求が残ることと pan が変わらないことをキー/ホイール各 handler の実入力で検査する。
- 既存のクリック名の試験にも本物の primary press/release を加えた。入場/終了と同一 pass のクリック/ダブルクリックを両クリック設定で送り、ページ送り・閉じる・リセットの結果を検査する。さらに実入力から作った配送待ちの click proposal が、実際の navigation handler によるページ/slice の交代後に PageSide/リセットとして再束縛されないことを検査する。
- ModifierHold は Ctrl/Alt のカスタム割当と物理 synthetic timeline で、通常素材/元画像素材の選択、PlaneView の継続、ルーペの抑止を検査する。ピンチは2接点の raw Touch イベントを通し、倍率だけが変わり lens/page は変わらないことを検査する。
- PDF は `GridItem::PdfPage` と PDF metadata の実描画経路で、未登録の現在素材の代わりに前素材を描いたまま、ホイールの `maybe_rerender_pdf` と指定倍率の描画後の需要がそれぞれ現在素材から決まることを検査する。登録待ちは余分な repaint の無い pass を先に確認し、所有 viewport の次 pass 要求と登録後の採用を検査する。
- `docs/spec.md` の魚眼の節を、既存のフォルダ背景色の見出し・本文の後へ移した。本文自体は変更しない。

追加・強化した試験 (いずれも実装担当では未実行):

- `context_menu_model::tests::plane_copy_pdf_eligibility_is_limited_to_the_fullscreen_snapshot`
- `ui_dialogs::context_menu::plane_copy_tests::plane_copy_action_menu_and_ring_reject_a_frame_that_has_not_been_drawn`
- 同 `gpu_tests::{plane_copy_action_menu_and_ring_submit_the_same_frozen_canvas_frame, plane_copy_action_rejects_stale_context_and_page_slice_snapshots}`
- `ui_fullscreen::plane_view_tests::gpu_tests::transition_and_click_tests::{plane_key_noop_and_rejection_preserve_pending_initial_placement_and_pan, plane_wheel_noop_and_rejection_preserve_pending_initial_placement_and_pan, plane_same_pass_entry_exit_clicks_turn_close_or_reset_from_the_predicted_view, plane_deferred_real_clicks_do_not_rebind_to_another_page_or_slice}`
- 同 `gpu_tests::additional_coverage::{plane_modifier_hold_selects_original_with_custom_binding_and_keeps_plane_paint, plane_actual_two_contact_pinch_changes_image_zoom_without_changing_lens_or_page, plane_real_pdf_routes_keep_both_demands_current_while_previous_material_is_painted, plane_registration_wait_requests_the_owning_viewport_next_pass}`
- 既存の `plane_real_frame_drops_fisheye_after_wheel_page_request_and_does_not_rebind_click` / `plane_same_pass_prediction_and_application_use_the_same_pose_and_click_classification` を実クリック入力付きに強化。

共有 runner の対象:

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/plane_/) | test(/context_menu_model::tests::/) | test(/wheel_frame_tests::/) | test(/ui_fullscreen::initial_placement::/) | test(/double_click_close_tests::/)'
```

新しい統合テスト・UI snapshot の更新は不要。GPU 試験は `gpu_tests` の子 module に置き、既存 nextest の gpu group に入る。既定 RightCtrl の OS 側物理キー判定は synthetic timeline が左右の modifier を注入できず、この追加試験では直接確認していない。新しい入力注入の仕組みは作らず、実キーでの確認は設計担当へ引き継ぐ。

### 追加試験の前提と同一 pass のクリック受付への修正 6

設計担当から関連 nextest 2746件中2739成功・新規7件失敗の結果を受領。以下はソースで照合した原因と修正であり、実装担当による cargo の再実行結果ではない。

| 試験 | 原因の分類 | 修正・維持する検査 |
| --- | --- | --- |
| plane_key_noop_and_rejection_preserve_pending_initial_placement_and_pan | 試験の通知先の前提 | 拒否は `show_fullscreen_nav_noop` → `fs_boundary_hint::NavNoOp` に出る。トーストの unwrap を理由付き境界ヒントの検査へ直す。実キー消費、初期配置要求・pan の保持、古い slice の no-op の検査を維持する。 |
| plane_wheel_noop_and_rejection_preserve_pending_initial_placement_and_pan | 同じ試験の前提 | 実 wheel handler 後の境界ヒントと `PlaneEntry(Trim)` を検査する。初期配置要求と pan が変わらない検査は維持。 |
| plane_deferred_real_clicks_do_not_rebind_to_another_page_or_slice | 試験の移動要求の前提 | 失敗は配送待ち click の適用前。`FsPageNav::Delta` は `signum()` で隣接方向を表すため、3番への明示移動は `Target(3)` にする。移動後の click 破棄、ページ/slice/zoom/pan/rotation の保持はそのまま検査する。 |
| plane_same_pass_entry_exit_clicks_turn_close_or_reset_from_the_predicted_view | 製品の通常クリック受付 | `accept_backing_click` が pass ごとに一度だけ加算していたため、同じ pass の受理済み2クリックを1回と数えた。所有・前面 layer・focus・egui の受理を離しごとに照合し、従来の3で飽和する回数を保って畳む。予測された魚眼状態でのリセット/終了分類と、両クリック設定の実入力検査は維持する。 |
| plane_actual_two_contact_pinch_changes_image_zoom_without_changing_lens_or_page | 試験の backend 入力列の前提 | 製品の touch correlation は先頭接点と pointer promotion の相関を要求する。egui-winit と既存 touch 試験に合わせ、先頭接点の開始・移動・終了へ pointer mirror を加える。2接点の Touch から倍率が変わり、pose/page が変わらない検査は維持する。 |
| plane_modifier_hold_selects_original_with_custom_binding_and_keeps_plane_paint | 試験の keyboard owner の前提 | 単独で作った pass に owner の発行が無く、`keymap_owner_blocks_shortcuts` が拒否していた。実 `keyboard_owner_for_pass` を通してから hold の成立とルーペ無描画を両方検査する。素材IDとposeの検査も維持。 |
| plane_real_pdf_routes_keep_both_demands_current_while_previous_material_is_painted | 試験の GPU 登録の前提 | RootDepthViewer は FullOutput の texture delta を破棄する。描画後に tex manager を drain しても登録できないため、実 final material と保持画素で renderer 登録を再現する。初回は描画前、差し替え完了時は現在素材を登録する。両 PDF 需要入口・前素材の描画・現在素材の採用の検査は維持。 |

**設計との差分 (既存クリック実装の事実)**: fullscreen-double-click-close-plan §4.3/§5 の「従来経路を変えない」前提には、通常 PageTurn の同一 pass 複数クリックを一度しか数えない実装も含まれていた。§1.917 の同一 pass のダブルクリック検査に合わせ、その受付の粒度だけを離しごとに修正する。PageTurn の3で飽和する規則と DoubleClickClose の2回で組を区切る規則は混ぜない。detached の述語・viewport 経路・配送は変更しない。

通常 PageTurn の受付に `page_turn_batched_double_and_triple_click_reset_the_moved_canvas`、`page_turn_list_open_double_click_still_counts_only_the_canvas_release`、`page_turn_batched_canvas_click_acceptance_is_idempotent`、`page_turn_batched_front_and_canvas_clicks_cannot_form_a_double_click`、`page_turn_batched_clicks_on_both_sides_of_focus_loss_cannot_form_a_double_click` を追加した。既存の Depth/初期配置/パネルの試験は front snapshot を helper へ渡すだけで検査を維持する。

共有 runner は修正5の filter に加え、`test(/depth_focus_tests::/) | test(/depth_view_transform_tests::/) | test(/side_panel_secondary_input_tests::/)` を含める。実装担当は cargo check/test・コミット・アプリ起動を行っていない。

### 時刻と PDF 需要列の試験前提への修正 7

設計担当から関連3630件中3628成功・新規2件失敗の結果を受領。以下の2件は試験の前提を訂正し、製品コードは変更していない。

- ModifierHold の単独 `run_pass` が time 未指定だったため、egui は f32 の predicted_dt で進み、次の RootDepthViewer の f64 `frame_counter / 60` より僅かに先へ進んでいた。既存の viewport 入力試験と同じく frame_counter を加算し、その f64 時刻を明示する。前 pass より進むことも検査する。実 hold・ルーペ抑止・素材ID・pose の検査は維持する。
- PDF の991は `render_fullscreen_viewport` の入力前の `ensure_pdf_display_resolution` → `pdf_display_render_long_edge_for_idx` → `request_pdf_rerender_at_target` から出る。vector ページの寸法は現在の `fs_cache` の634×819、表示先は1200×900。Page fit の900物理画素へ既存の1.10 headroomを掛けた f32 の ceil で991となる。現在の819長辺は991の90%未満なので要求する。その後、Ctrl+ホイールの適用が `maybe_rerender_pdf` → `request_pdf_rerender` を通り、新倍率約1.21に対応する1200を要求する。どちらも現在素材から作り、`FsSinglePageDraw.transform` (前素材の描画変換) は参照しない。
- PDF 試験は素材採用直後に要求がまだ無いことを検査し、実 frame の期待を `[(0, fit_target), (0, wheel_target)]` の正確な列へ直す。両 target は現在素材と表示先から独立に算出し、異なる値であることも検査する。指定倍率の描画後の需要、前素材の描画と現在素材の採用の検査は維持する。

共有 runner: `cargo nextest run -p mimageviewer --lib -E 'test(/plane_modifier_hold_selects_original_with_custom_binding_and_keeps_plane_paint|plane_real_pdf_routes_keep_both_demands_current_while_previous_material_is_painted/)'`。
実装担当は cargo check/test・アプリ起動・コミットを行っていない。

### 検証の引き継ぎ

実装担当では cargo の build/test/check/clippy を実行していない。
許可された `cargo fmt` は正常終了。`python scripts/check_ui_glyphs.py` は危険な字形なし、
`python scripts/check_site_cards.py` は54カード・超過0。最終差分の再確認も同じ結果。
`git diff --check` の指摘なし。
GPU/perf の実測値は未取得。PNGは生成・目視・通常比較とも未実施。

共有 runner の対象 (GPU 子 module は既存 nextest の `test(/(^|::)gpu_tests::/)` により gpu group):

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/plane_/) | test(/ui_dialogs::preferences::search_index::tests/)'
cargo nextest run -p mimageviewer --lib -E 'test(/depth_/) | test(/displayed_image_transform::/) | test(/wheel_input::/) | test(/touch_input::/) | test(/keymap::tests::/) | test(/search_nav_entry_inventory_tests::/) | test(/settings_transfer::tests::/)'
cargo nextest run -p mimageviewer --lib -E 'test(/ui_fullscreen::in_page_scroll::/) | test(/ui_fullscreen::initial_placement::/) | test(/double_click_close_tests::/) | test(/overlay_navigation_tests::/) | test(/side_panel_secondary_input_tests::/) | test(/edited_clipboard_copy::/) | test(/vram_accounting::/) | test(/viewer_context_registry::/)'
cargo nextest run -p mimageviewer --test export_integration --test settings_integration --test ui_snapshot
```

追加/変更した試験の中心は `plane_view::tests`、`plane_view_wgpu::{tests,gpu_tests}`、
`app::plane_view::tests`、`ui_fullscreen::plane_view_tests`、`touch_input::tests`、`keymap::tests`、
`ui_dialogs::preferences::pages::plane_view_lens_tests::gpu_tests`、`settings_transfer::tests`。新しい統合テストファイルはない。
UI PNGは `plane_view_lens_settings_light`、`plane_view_lens_settings_dark`、`plane_export_size_choices_dark` の3枚。
生成指定ありで作成・目視した後、指定なしで比較する。書き出しダイアログ全体の文言も受入で確認する。
マニュアル変更に伴う同梱 MCP help の抽出/検索/transport 回帰も既存の共通実行口で確認する。
変更が入力・描画・資源/終了・出力にまたがるため、focused 結果の後は既存の full gate が必要。
利用者の指示に従い、実装担当から追加の cargo 実行やアプリ起動は行わない。

実機では通常/別ウィンドウ、DPI、PDF の再ラスタ、RAW の差替え、右 Ctrl+Shift と変更した hold 割当、
旧来の Ctrl+Shift+左ドラッグ、3つの出力、scope をまたがない資源解放と perf を確認する。
承認のない対話テストは開始していない。検証用 build は共有 runner 側へ引き継ぐ。

### 改訂 1 の実装記録 (前提の矛盾により停止)

着手時 `1e0ca2f01`。§16 を通読し、純粋な係数・覆う遷移・GPU 参照試験・文書の差分を途中まで作ったが、§16.3 の固定の余裕と §16.5 の明示例が両立しないため停止した。症状に合わせた補正・例外・回復処理は追加していない。

**設計との差分となるコードの事実**: `quantize_points_to_physical_pixels` は f32 の乗算・加算・floor・除算を行い、`snap_rect_to_physical_pixels` はその原点と別に求めたサイズから max を作る。整数画素への丸めだけでなく、非常に大きい全体矩形での乗除算の誤差も残る。Height fit では、GPU 上限内の細長い画像でも全体矩形が非常に大きくなる。以下はソースと同じ f32 演算による算術確認であり、Rust のテスト・GPU・実アプリでの観測ではない。

| 条件 (pan・トリム・回転はなし) | 連続配置から描画配置への辺の差 |
| --- | --- |
| 画像4096×1、canvas1600×900 points、Height fit、zoom49、ppp1.75 | min.x は −90316000 → −90315992、max.x は90317600 →90317608 points。各8 points = 14物理画素。 |
| 画像8192×3、canvas1600×6301.53857421875 points、Height fit、zoom50、ppp1.3 | min.x は −430184256 → −430184224、max.x は430185856 →430185952 points。右辺96 points = 約124.8物理画素。canvas の物理高さは約8192で surface 上限内。 |

したがって δ=4、32、64 では一律に保証できない。一方、上記を覆う固定 δ≥124.8 にすると、R8 の明示例 (canvas200×200、画像1000×1000、Page fit、横トリム[0,0.028]) の連続 Q の幅は倍率1あたり5.6なので、`z_cover ≥ (200 + 2×124.8)/5.6 ≈ 80.286 > ZOOM_MAX(50)`。この例で canvas⁺ を覆うという §16.5 の条件を満たせなくなる。δ の固定値を上げるだけでは、精度の条件と明示例を同時に保てない。入力範囲、計算の精度、余裕の決め方、または上限に届かない場合の仕様について設計側の判断が必要。

製品コードと公開文書の途中の変更は着手時の内容へ戻した。未完の差分は `docs/fisheye-plane-view-revision1-wip.patch` に保存した。このパッチは統合途中で、未作成の試験 module への参照と未接続の API を含み、そのまま適用・ビルドする成果物ではない。対象は `src/plane_view.rs`、`src/plane_view_wgpu.rs`、`src/plane_view_wgpu/gpu_tests.rs`、`docs/display-pipeline.md`、`docs/spec.md`、`htdocs/mimageviewer/manual/fullscreen.html`。§13 と実際の入力配送・受理済み素材の記録は未変更。

ビルド・test・check・clippy・アプリ起動・コミットは行っていない。δ の一律の上限と、改訂1全体の試験は未確認。補助算術に許可一覧外の Python 実行があったため、以後の検証実行は停止した。

### 改訂 1 の実装記録 (範囲を限定した設計での続き)

着手時 `96d8817cf`。上の停止記録は経緯として残す。作業用パッチから再開し、§16.2〜§16.6 の実装・試験・文書を接続した。作業用パッチは最終差分に含めず削除する。

**保証範囲の扱い**: 表示の縦横比32以下、遷移後の連続配置の座標の絶対値2²¹物理画素以下は、まだ候補である。δ は4物理画素のまま。**範囲はテストで確定する**。実装担当は候補の成否を未実行の算術で確定したとは扱わない。範囲外にも同じ覆う式を使い、誤差の上限を求める式・範囲ごとの別処理・保証用の新しい状態は追加していない。

根拠を確認する `plane_view::precision_tests` は、実際の `resolve_fs_image_transform` を Proportional/Texels の両方で通す。矩形の両軸から倍率を再計算して平均する順序、その値とpppから物理サイズを作る順序を実出力と照合する。その後、trim・部分素材・保存回転・自由回転・screenと同じ物理座標化を含む Q の各辺を、その法線へ投影して総誤差4以下を検査する。理想実数の丸めだけの上限や、元の要求倍率を代わりに使った検査ではない。候補の端、PDF/RAWのSourceLayout、過去の範囲外反例を試験へ含めた。候補が失敗した場合は、設計担当の結果を受けて範囲を狭め、ここへ確定値を記録する。

実装した経路:

- 有効 Q は実際の transform の source/screen 写像を使い、表示した paint の範囲と coverage、material source (coverage と sampler_roi) の交差から作る。`plane_fill_coefficient` は4隅×4辺と80度上限から決め、画面の uniform 生成だけが計算する。`perf::event` の `plane_view/fill_coefficient` で区間を記録し、キャッシュは持たない。alpha_max/coefficient を削除し、CPU参照cameraも k を受け取る。出力は凍結した snapshot の k を使う。
- `FullscreenPageLayout` に表示単位の `PlaneCoverRecord` を付けた。PlaneView の記録は、実際に描いた素材の old_input/old_layout と description を採用する。通常の単ページは採用した paint payload から記録し、別の holdover geometry へ束縛されたものを新しい現在素材として記録しない。begin/clear と表示単位の anchor の照合で寿命を揃える。viewer context の新しい退避項目は増やさない。
- producer は受理済み素材だけを前の記録から取り、canvas/ppp/回転/trim/fit/左右配置とgapは今のpassから組み立てる。キーも同じproducerを使う。予測の前に確定した PlaneCoverInput を proposal に持たせ、遅延適用とsegment再分割でもその値を使う。記録の無い入力は量を捨て、通知もしない。
- 連続の resolver の Q で canvas⁺ を覆う位置・倍率に揃え、source点を固定した通常ズームを行う。strength は1/4、離れる量は max(notches, −4×strength)。最後の最小重なり補正も同じ連続配置で一度だけ行い、魚眼の予測・実適用の layout 再clampを削除した。ほかの入力のclamp、PDF需要の分離、入口・終了・ドラッグ・detached・出力配送は維持する。

**設計との差分**: 新たな仕様上の差分はない。保証範囲の数値は設計が許す候補のままで、未確定である。受理済みの素材情報は既存の表示単位の記録に同居させ、独立した viewer 状態やキャッシュにはしなかった。

変更したファイル:

- `src/plane_view.rs`、`src/plane_view/revision_tests.rs`、`src/plane_view/precision_tests.rs`
- `src/plane_view_wgpu.rs`、`src/plane_view_wgpu/gpu_tests.rs`
- `src/app/plane_view.rs`、`src/displayed_image_transform.rs`、`src/ui_fullscreen.rs`
- `src/ui_fullscreen/plane_view_tests.rs`、同 `gpu_tests/transition_and_click_tests.rs`、同 `gpu_tests/revision_input_tests.rs`
- `docs/display-pipeline.md`、`docs/spec.md`、`htdocs/mimageviewer/manual/fullscreen.html`、本書§11・§13

試験は§16.5の全画素/細分境界参照/最大性、3レンズ・中心・aspect・回転・trim・部分素材の組合せ、R7〜R9の数値例、SourceLayout、分割と合算、resize、最大倍率と欠落記録を追加した。GPU試験は既存 `gpu_tests::` 内に置き、snapshot k の出力不変、覆う/覆わない範囲外の実画素を区別する。入力の回帰には予測・即時適用・layoutと素材が変わった後の遅延適用の同一結果、現在のDPI/表示条件、未描画frameのキー/ホイール破棄、PDFの前素材の記録を含む。旧1/8刻み、固定alphaと半径0.8Rの試験は置き換えた。

設計担当の共有実行口へ引き継ぐ:

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/plane_view::/) | test(/plane_view_wgpu::/) | test(/plane_view_tests::/)'
cargo nextest run -p mimageviewer --lib -E 'test(/depth_/) | test(/displayed_image_transform::/) | test(/ui_fullscreen::in_page_scroll::/) | test(/ui_fullscreen::initial_placement::/) | test(/wheel_frame_tests::/) | test(/double_click_close_tests::/)'
```

新しい統合試験ファイルやUI PNGの更新は不要。今回の `cargo fmt` は正常終了、`python scripts/check_ui_glyphs.py` は危険な字形なし、`python scripts/check_site_cards.py` は54カード・超過0。ビルド・test・check・clippy・アプリ起動・コミット、算術用スクリプトは実装担当では実行していない。保証範囲の確定、Rustのコンパイルと試験、GPUの画素結果・性能・実機での見た目は未確認。

### 改訂 1 の実装記録 (最初の関連試験で判明した不具合の修正)

着手時 `8a96647ed`。設計担当の `test(/plane_view|fisheye|displayed_image_transform/)` のログ (124件中100件成功、24件失敗) とソースを照合した。今回の実装担当は Rust のビルド・試験を実行していない。

**受理済み素材の寿命の不具合 (製品、22件)**: メインの単ページ経路は描画前に layout を clear し、描画後にも begin(Single) する。従来の `record_plane_cover_material` は前者で Empty になった kind を条件にし、仮に記録できても後者で消していた。このため次の入力の producer が常に None となり、キーもホイールも入場しなかった。`FsSinglePageDraw` に実際に描いた素材の `PlaneCoverRecord` を載せ、呼び出し元が `publish_single` で begin・push の後にページと同時に公開する形へ修正した。記録を作る helper は layout を変更しない。PlaneView は old_input/old_layout、通常表示は実際に採用した payload を引き続き使う。

- `plane_view_tests.rs:43` で停止した15件 (touch、ModifierHold、PDF両需要、登録待ち、入口条件、出力snapshot、レンズ所有2件、素材待ち、占有、終了、PDF前素材、縦ホイール、遅延クリック、同passの入場/終了クリック) は、入力検査の前に共通の `enter` が素材記録を得られず停止していた。これらの期待値は変更しない。
- `plane_key_stage_precedes_wheel_regardless_of_raw_order`、`plane_real_fractional_segments_match_and_downward_key_only_spends_strength` は、記録欠落でキー/ホイールの入場が起きず pose が None だった。同じ修正で入口を接続し、期待値は維持する。
- `plane_same_pass_prediction_and_application_use_the_same_pose_and_click_classification` の `expected.3` も、予測に渡す cover が None のため false だった。予測/適用やクリック分類の期待値は維持する。
- `plane_revision_unpainted_page_discards_wheel_and_assigned_key_until_material_record_exists` と `plane_revision_cover_producer_rebinds_current_canvas_dpi_rotation_and_fit` は、実描画後も記録が残らなかった。同じ公開境界の修正で扱い、未描画入力の破棄と現在条件への組み替えの検査を維持する。
- `plane_key_noop_and_rejection_preserve_pending_initial_placement_and_pan` と `plane_wheel_noop_and_rejection_preserve_pending_initial_placement_and_pan` は、記録なしの入力を使わない §16.3 の経路で、Trim の入口拒否まで届かなかった。実描画の記録を残す修正で入口拒否に届くようにし、理由の通知と初期配置/panの保持を引き続き検査する。

ほかの単ページ経路も確認した。Z/照準の描画は独自の入力所有、book mode surface と display unit holdover は Captured の記述であり、現在素材の記録を採用しない。比較用の代替 transform も記録なしで公開する。これらや erase、page turn、連結読みの layout 再構築は begin/clear の消去を維持する。in_page_scroll 内の layout 代入はテストの新規 layout への置換で、古い記録を引き継がない。push/push_occurrence/push_spread_occurrence でも既存の記録を消し、追加のページへ古い記録を付け替えない。同じページの occurrence のみを束縛し直す rebind_occurrences は記録を保つ。実描画の普通/PlaneViewの公開と、これらの再構築境界を回帰試験へ追加する。

**最小重なりの不具合 (製品、1件)**: `plane_revision_r9_resize_keeps_pure_result_and_terminal_overlap_uses_same_input` の失敗は、終端補正へ viewport と交差済みの `hit_rect` を渡したため。完全に画面外の配置では補正に必要な元の寸法が失われる。既存の通常 pan は UV の四隅を変換した未clip AABBを使っている。§16.3 の「同じ最小重なりの規則」に合わせ、同じ `PlaneCoverInput` の連続配置の paint quad の未clip AABB を終端で一度だけ使う。試験の重なり対象もこの未clip矩形に直し、47.99以上の重なりと追加補正ゼロの期待値を維持した。画面外の各方向・保存回転・自由回転・trimの回帰も追加する。

**範囲外の反例の前提の誤り (テスト、1件)**: `plane_outside_precision_preserves_known_large_coordinate_counterexamples` の4096×1の例は、論理座標差8 pointsにppp1.75を掛けた値を、そのまま実描画のQの差と見なしていた。§16.2 のQは画面用の物理座標化まで含む。同じ順序で両Qをf32で物理座標にすると16画素の格子へ丸まり、差が相殺される。停止記録の「14物理画素」はその中間値についての算術であり、この例の実描画Qの差としては訂正する。この例は論理差と物理相殺の両方を検査する独立試験に分け、別に倍率の再計算・両軸の平均まで含めて本当に覆いを失う範囲外の例を加える。覆わない場合の `k_fill = 0` と2Dと同じ写像の検査は維持する。新しい例の算術もRust試験として設計担当に渡し、未実行の結果を確認済みとはしない。

**設計との差分**: 仕様の変更はない。変更した試験の根拠は上記の §16.2 の実描画Qと §16.3 の既存と同じ最小重なりの規則である。δ=4と保証範囲の候補は変えていない。前回ログにある成功は再実行の成功とは扱わず、修正後の試験と保証範囲の確定は設計担当の実行で確認する。

変更ファイル: `src/displayed_image_transform.rs`、`src/ui_fullscreen.rs`、`src/plane_view.rs`、`src/plane_view/revision_tests.rs`、`src/plane_view/precision_tests.rs`、`src/ui_fullscreen/plane_view_tests.rs`、同 `gpu_tests/revision_input_tests.rs`、本書。実装担当でのビルド・test・check・clippy・実アプリ・GPU画素の確認、コミットは未実施。再実行のfilterは上と同じ `test(/plane_view|fisheye|displayed_image_transform/)` を用いる。

新しい範囲外の例は画像8192×1、canvas1600×900、Height fit、zoom50、ppp1、自由回転90度、pan=(-21632, 67108864)。短辺の座標差から再計算した倍率と長辺側の倍率の平均により、量子化後の長辺が縮み、回転中心も移る経路を通す。連続Qがcanvas⁺を覆うことと、実Qは覆わないことを同じ例で検査する。数値は試験のassertに置き、実行結果で確認する。

追加した回帰は `plane_revision_real_paint_publishes_material_and_transform_together`、`plane_revision_layout_replacements_retire_accepted_material_before_the_next_input`、`plane_revision_record_creation_is_independent_of_layout_and_publication_is_explicit` (既存gpu_tests内)、`plane_revision_terminal_overlap_uses_unclipped_paint_for_each_offscreen_direction`、`plane_outside_precision_distinguishes_logical_shift_from_physical_q_error`、`plane_outside_precision_scale_reconstruction_can_lose_actual_cover`。設計担当は前回と同じ `cargo nextest run -p mimageviewer --lib -E 'test(/plane_view|fisheye|displayed_image_transform/)'` で再検証できる。PNG更新や新しい統合試験は不要。

変更ファイル: `src/displayed_image_transform.rs`、`src/ui_fullscreen.rs`、`src/plane_view.rs`、`src/plane_view/precision_tests.rs`、`src/plane_view/revision_tests.rs`、`src/ui_fullscreen/plane_view_tests.rs`、同 `gpu_tests/revision_input_tests.rs`、本書§11。`cargo fmt` は正常終了、UI字形検査は危険な字形なし、サイト検査は54カード・超過0、`git diff --check` も正常終了。修正後のコンパイル・試験、新しい数値例、GPU画素と実機の見た目は未確認。アプリ起動・コミットは行っていない。

### 改訂 2 の実装記録

§0・§17 を通読し、通常ズームで画面に見えている歪みを固定する動作、固定後のドラッグでの維持、Ctrl+Shift の寄る操作での自動調整への復帰を文書へ反映した。縮小や移動による余白を許す `Locked` と、余白なしの制限を使う `Auto` を区別し、改訂1の無条件な「端が入ると歪みが消える」という説明を `Auto` の場合へ限定した。

設計で定めた接続は、`PlaneViewState` が描画の `PlaneFillMode` を所有し、poseと純粋な覆う遷移へ入れない形。固定値は最後のsnapshotが保持する `k_fill` を使い、強さから割り戻さない。倍率の変わらない通常入力やsnapshot欠落では新たに固定しない。寄る操作では「受理」と「幾何の変化」を区別し、受理されれば `Auto` へ戻す。即時・遅延適用の描画順は維持し、遅延用の候補はcontext・anchorを照合して終了・所有者交代で破棄する。固定中も中心・canvasから80°上限を適用し、出力はsnapshotの `k` を使う。

**通常ズームの入口一覧** (着手時 `6f9785302` の全 `fs_zoom` 書き込みと呼び出し元を調査):

| 入力 | 倍率を書き込む境界と接続 |
| --- | --- |
| Ctrl+ホイール | `apply_fs_wheel_frame_proposal` の通常Imageズームの changed 分岐。予測前に写した `plane_fill_lock` 候補を共通の固定処理へ渡す |
| 設定で通常ホイールがズーム (`SettingZoom`) | 同じ通常Imageズームの分岐。Ctrlの有無で固定の扱いを分けない |
| 中ボタンのドラッグ | `handle_drag_zoom` の `FsZoomOwner::Image` の倍率書き込みで、直前のsnapshot候補を共通の固定処理へ渡す |
| Ctrl+Spaceスクラブ | 同じドラッグズームのImage所有の書き込み。修飾キーの取得・配送・終了処理は既存のまま |
| `FsZoomIn/Out` | `apply_fs_key_zoom_step` → `apply_touch_zoom_factor_about_pivot` のImage所有の倍率書き込みで固定 |
| 2本指ピンチ | 実Touch入力 → `TouchCommand::Zoom` → 同じ `apply_touch_zoom_factor_about_pivot` で固定 |
| ゲームパッド | `apply_gamepad_fullscreen_zoom` の倍率書き込みで固定。既存の微小差の戻り値の規則は変えず、実際に倍率を書き換えたときに固定する |

固定の処理は `lock_plane_fill_for_zoom` の1か所。旧倍率と新倍率が同じなら何もせず、候補のcontext・anchorと現在のPlaneViewを照合して、AutoだけをLockedへ変える。すでにLockedなら値を維持する。静的な `apply_wheel_zoom` 自体はCtrl+Shiftの遷移も使うため変更しない。中ボタンとスクラブの関数名は既存ソースのものを使い、キー・ピンチ・ゲームパッドの配送は変更しない。

占有中に停止するナビゲータは対象外。分析モードとの倍率引き継ぎ、連結読みの描画倍率同期、fit/初期配置/reset、viewport復元は通常ズームの入力ではなく、それぞれ既存の所有・終了の規則を維持する。追加の通常ズーム入口は見つからなかった。

`FsWheelFrameProposal` は予測前にsnapshotの `k_fill` とcontext/anchorを写す。実適用ではsnapshotを読み直さず、残ったsegmentを順に処理する。各segmentの継続確認後と魚眼遷移の直後に候補の所有を照合し、終了で捨てる。同じanchorで入り直しても捨てた候補は再採取しない。永続の世代や描画用の予測状態は追加しない。純粋な遷移の `accepted` は幾何の変更とは独立で、受理した寄る入力は `changes_view` の早期returnより前にAutoへ戻す。姿勢・倍率・覆う判定・panの計算は変更していない。

**設計との差分**: 仕様の差分はない。上限は内部のf64の計算を共有し、AutoもLockedも最後に同じf32への丸めを一度だけ適用する。既存の純粋試験で入力の分割と合算の結果全体を比較していた箇所は、終了後の負のsegmentは無視されるため新しいacceptedが異なる。倍率・pan・pose・拒否理由の一致の検査は維持し、受理/無視を別に検査する。detachedの述語・viewport経路・swapの対象は変更せず、既存の `fs_exclusive_view` の所有に乗せる。

**試験の追加**: `plane_view::revision_tests` にLockedのsupport非依存、共有capと3レンズの中心移動/resize、strengthゼロでも独立なfillの記録、受理と幾何の区別の4件。`plane_view_wgpu::gpu_tests` に固定後の画面/copy/exportの実画素とsnapshotの不変を1件。`ui_fullscreen::plane_view_tests::gpu_tests::revision2_input_tests` に通常ズーム7入口の変更/上限、即時/遅延、2回目の固定保持、下/上と幾何同値の受理、欠落記録、Locked(0)、各ドラッグ/回転/resize、同passの終了再入場/操作順、context照合とF11/scoped mount、出力snapshotの13件。終了再入場の試験はwheelのsourceを分けて3segmentを確認し、逆向きの量がゼロへ合算されただけでは通らない。`ui_dialogs::context_menu::plane_copy_tests::gpu_tests` に固定した状態の実コピーaction/menu/ringの1件を追加。GPUを使う試験は全て既存の `gpu_tests::` のgroupに入る。新規統合試験やUI PNG更新は不要。

**変更ファイル**: `src/plane_view.rs`、`src/plane_view/revision_tests.rs`、`src/plane_view_wgpu.rs`、`src/plane_view_wgpu/gpu_tests.rs`、`src/app/plane_view.rs`、`src/ui_fullscreen.rs`、`src/ui_fullscreen/plane_view_tests.rs`、新規 `src/ui_fullscreen/plane_view_tests/gpu_tests/revision2_input_tests.rs`、`src/ui_dialogs/context_menu/plane_copy_tests.rs`、`docs/display-pipeline.md`、`docs/spec.md`、`htdocs/mimageviewer/manual/fullscreen.html`、本書§11/§13。

**未検証の範囲**: ビルド・test・check・clippy・算術用スクリプト・実アプリ起動・コミットは実装担当では行っていない。改訂2のコンパイル・自動試験・GPU画素・実機の見た目は未確認。以前の改訂の成功記録を、今回の変更後の検証結果として扱わない。設計担当の共通の実行口へ次を引き継ぐ。

実装担当で許可された検査は `cargo fmt` 正常終了、`python scripts/check_ui_glyphs.py` は危険な字形なし、`python scripts/check_site_cards.py` は54カード・超過0、`git diff --check` 正常終了。

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/plane_view|fisheye|displayed_image_transform|plane_copy/)'
cargo nextest run -p mimageviewer --lib -E 'test(/ui_fullscreen|in_page_scroll|page_turn|depth|ui_erase|gamepad/)'
```

### 改訂 2 の実装記録 (遅延ズーム試験の取り込み境界の修正)

着手時 `607bf573c`。設計担当の153件中1件の失敗をログとソースで照合した。`plane_revision2_delayed_zoom_uses_the_candidate_captured_before_paint` は `frame_with_before` の中で `collect_actual_merge` を呼び、ホイールを消費した後に、同じpassの実viewerがもう一度前面入力を取り込んでいた。そのためbegin-passで保存したnavigatorの3事象と、消費後の前面入力の1事象の所有列が食い違った。製品経路の不具合ではなく、テストが入力の取り込み境界を二重に実行したことが原因。

既存の遅延適用試験と同じく `ctx.run_ui` で予測・保留・入力のclaimを一度行い、通常の `viewer.frame` で旧状態を描画した後に保留したproposalを実適用する形へ変更した。新しい描画snapshotであることと旧uniform全体の一致を確認し、snapshotを消してから適用しても事前の候補でLockedになること、次frameは新しい倍率の配置と固定した `k_fill`/`k` を描くことを検査する。時刻は既存viewerと同じframe_counter由来のf64で単調に進める。製品コードと設計は変更していない。変更は当該試験と本記録のみで、コンパイル・試験の再実行は設計担当へ引き継ぐ。

`cargo fmt` と `git diff --check` は正常終了。Rust/GPU試験は実行していない。

### 段 2 の実装記録

§0・§18を通読し、Ctrl+Shift+左ドラッグによる写真の板の傾けと、傾きを戻すキーの登録・文書を追加した。範囲は制限せず裏側も反転した絵として見せること、レンズ中心の下の点を回転中心とすること、傾けによる余白を許して倍率を自動で変えないことを反映した。

**キーの登録**: `FsPlaneTiltReset` を `FsImage` / `Press` / 既定なしで追加した。enum・`ALL_ACTIONS`・INI名・説明・context・trigger・defaultを揃え、`docs/keymap.ini.default` の同じ順に `none` の参照行を追加した。表示名は「写真の板の傾きを戻す」。通常のCommandSpecと操作APIの一覧・編集は `KeyAction::all` と同じ名前解決を使うため、専用のAPI形式や別allowlistは追加していない。割り当て・解除・settings往復・FsImageと動画のscope分離・魚眼の寄る操作との競合・生成参照の試験と、操作APIの取得/再割り当て/解除の試験を `src/keymap.rs` に追加した。キーの段で `reset_plane_tilt(ctx)` へ配送し、非ナビゲーション操作のinventoryにも追加した。

**所有の型と入口** (着手時 `ee7421548`): `FsCanvasDrag` のImage/PlaneLensと排他的な `PlaneTilt { anchor, start, previous }` を追加した。`handle_plane_drag` は押下時にOSのCtrl/Shiftと既存の入口条件・前面UIの可否で所有を決め、PlaneViewが無くてもTiltを保持する。最初の非ゼロ移動でstrength 0・押した位置のレンズ中心・Autoのstateを作る。専用の `enter_plane_view_from_tilt_drag` は所有を保持して公開する。普通の `set_fs_exclusive_view` はPlaneのドラッグを退役させる。両者はprivateの `publish_fs_exclusive_view` を共有し、Panoramaの後処理やDepthのdrop、通知は既存どおりである。入場のために所有を一旦退避して戻す処理は作らない。

Lens/Tiltの抑止は `plane_drag_owned` とcontextごとの同じpass使用済み記録で共用し、離したpassもパン・自由回転・単クリックへ渡さない。ダブルクリックによる明示のResetTransformだけは、§18.3どおり所有中でも通す。primaryのedge列を順にfoldし、同じpassの離す→押すを別の操作として扱う。`finish_plane_drag` はrelease時だけ `snap_identity` を呼び、focus/modal/anchor不一致での取消では寄せずに所有を解く。resetは即座に所有を解いて恒等へ戻し、strength 0なら終了する。対象外の表示はTiltを取らず、既存の自由回転の可否に委ねる。

`PlaneTilt { q: [w,x,y,z] }` は手書きDefaultの[1,0,0,0]。回転ベクトルから1つの四元数を作って左から掛け、各回を正規化する。恒等の判定は共通の `is_identity` のxyzのノルムで行い、0.01度以下でも操作中は寄せない。描画の恒等専用経路は許容誤差を使わないexact-I判定とし、微小な生きた回転は遠近へ通す。

**式と終了の境界**: CPU/WGSLを従来の放射状魚眼→対角50度のピンホールと板の交点へ合成した。傾きが厳密な恒等なら、従来の演算順とk近傍ゼロの恒等経路をそのまま返す。遠近のdenは符号を保った有限な代用値とし、無効tは1を使って微分後にマスクする。負のdenを潰さず、真横の退化はepsilon内を無効にする。既存13個のuniform indexは変えず、13〜15へ回転行列の3列を追加した。同じuniformのsnapshotを画面・出力に使う。魚眼の係数・覆う配置・pan制約・固定の規則と、2D倍率によるPDF需要は維持する。

純粋なホイール遷移は `tilt_is_identity` と `tilt_drag_owned` を受け取り、strength 0でも傾きか所有があればposeを保つ。予測前の両値を `PlaneWheelEndInput` としてproposalに写し、segmentの再分割や描画後の適用でも同じ値を使う。段2で新たに可能となるstrength 0のまま離れるsegmentはused=0なので、受理してposeの保持/終了だけを決め、倍率とpanを完全に維持する。未消費の量で逆写像の丸めやpan補正を走らせない。既存strength正の遷移・改訂1/2の幾何は変更していない。

**detachedの範囲**: 述語・viewport経路・配送・swapの項目は変更していない。四元数は既存のfs_exclusive_view、Tiltの所有は既存のfs_canvas_drag全体の退避/引き継ぎに含まれる。変更した表示所有者の公開helperは入力所有の退役と公開を分ける構造上の入口で、detached用のguard・delay・retryや追加フィールドではない。

**文書**: キー仕様にCtrl+Shift+左ドラッグの固定入力とkeymap対象外の理由を追加した。表示パイプラインと仕様書は魚眼と遠近の2段の合成、恒等/裏側/真横、四元数と所有・終了条件を更新した。マニュアルは操作・対象・余白・裏側・resetと出力の仕様を説明し、内部用語、panの反転やPDFのぼやけの既知の制限は書いていない。§13の下書きにも傾けを追加した。設計本文§18は変更していない。

**設計との差分**: 仕様上の差分はない。旧yaw/pitch型と旧カメラの板との交点の式は§18で指定された置き換え対象。新規のused=0境界も「segmentの残りを使わない」規則の実装であり、既存の正のstrengthの計算を変えていない。回復・局所拡大率によるPDF需要・傾きに合わせたパン/ズーム補正は追加していない。

**試験と変更ファイル**: 純粋な式と四元数の10件を新規 `src/plane_view/tilt_tests.rs`、GPUの全画素比較とsnapshot出力の2件を `src/plane_view_wgpu/gpu_tests.rs`、実viewerの所有/終了/reset/対象外/複数context/出力の16件を新規 `src/ui_fullscreen/plane_view_tests/gpu_tests/tilt_input_tests.rs`、keymapと操作APIの2件を `src/keymap.rs` に追加した。既存のPDF需要と前素材の試験を傾けだけのケースへも展開し、既存assertを維持した。GPU利用は全て既存gpu_testsのgroupに入る。

ほかの変更ファイルは `src/plane_view.rs`、`src/plane_view/revision_tests.rs`、`src/plane_view_wgpu.rs`、`src/app/plane_view.rs`、`src/app/depth_view.rs`、`src/app/search_nav_entry_inventory_tests.rs`、`src/ui_fullscreen.rs`、`src/ui_fullscreen/plane_view_tests.rs`、同 `gpu_tests/additional_coverage.rs`、`docs/keymap.ini.default`、`docs/keymap-spec.md`、`docs/display-pipeline.md`、`docs/spec.md`、`docs/architecture-overview.md`、`htdocs/mimageviewer/manual/fullscreen.html`、本書§11/§13。

**未検証の範囲**: `cargo fmt` は正常終了、UI字形検査は危険な字形なし、サイト検査は54カード・超過0、`git diff --check` は正常終了。ビルド・test・check・clippy・算術用スクリプト・アプリ起動・コミットは実装担当では実行していない。追加試験の成功、コンパイル、GPUの実画素/性能、実機の操作感と見た目は未確認。新規のUI PNGは追加しておらず、既存の操作一覧snapshotへの影響も未確認。設計担当の共通の実行口へ引き継ぐ。

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/plane_view|plane_tilt|fisheye|displayed_image_transform|plane_copy/)'
cargo nextest run -p mimageviewer --lib -E 'test(/ui_fullscreen|in_page_scroll|page_turn|depth|ui_erase|keymap|search_nav_entry_inventory/)'
```

**検証の引き継ぎ**: `cargo fmt` は正常終了、字形検査は危険な字形なし、サイト検査は54カード・超過0、`git diff --check` も正常終了。ビルド・test・check・clippy・算術用スクリプト・アプリ起動・コミットは実行していない。今回のコンパイル、WGSL検証、追加/既存試験、GPU画素、実時間性能、実機の操作感は未確認。既存の成功記録を変更後の成功とは扱わない。新しいPNGは追加しておらず、操作一覧の既存snapshotの更新要否も試験結果で確認する。設計担当の共通の実行口へ次を渡す。

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/plane_view|plane_tilt|fisheye|displayed_image_transform|plane_copy/)'
cargo nextest run -p mimageviewer --lib -E 'test(/ui_fullscreen|in_page_scroll|page_turn|depth|ui_erase|keymap|search_nav_entry_inventory/)'
```

### 段 2 の実装記録 (最初の関連試験の3件への対応)

着手時 `94f7887a0`。設計担当の共通実行口のログ `s2-test1.log` (2928件中2925件成功) と各入力・fixture・式を照合した。修正後のRust/GPU試験は実装担当では実行していない。

1. **真横近傍の帯 (テストの前提)**: 失敗文言の89は回転角、1530は板から色を取れた画素数であり、17列×90行に相当する。「1度手前ならcanvas面積の10%未満」という旧期待値には§18.2の根拠がなかった。このfixtureは板の局所半幅a=160・半高さb=90、canvas160×90、対角50度のfであり、前方投影は `v.x = f cosθ X / (f − sinθ X)`、`v.y = f Y / (f − sinθ X)`。手前側の遠近拡大を含むためcosθだけから面積は決められない。±aの投影から帯の両辺を求め、全画素の素材内外を照合する試験へ置き換えた。`f > a` と `f b / (f + a) > 45` を確認して全90行が含まれる根拠を置き、解析した列数×90との完全一致を要求する。幅 `2 f² a |cosθ| / (f² − a² sin²θ)` の単調縮小と真横への収束、帯内/両外の連続点も検査する。真横全面マスク・無効点の有限性の旧検査は維持した。期待値の変更は§18.2の遠近と「前後では細い帯」という仕様に基づく。

2. **等立体角・裏側の素材縁 (製品の数値計算経路)**: CPUはRustのasin→sin/cos、WGSLは各intrinsicを通してから遠近の除算をしていた。裏側100度・density1・strength0.75・画素(28,3)について、逆三角/三角の往復の数値差が素材縁の判定へ増幅され得る経路を除去した。非exact-Iの等立体角だけ、`r_k = k r` から `sinθ = r_k sqrt(1 − r_k²/4)`、`cosθ = 1 − r_k²/2` をCPU/WGSL共通の演算で求める。数学上は§18.2の同じ放射状写像であり、傾き恒等の旧演算順とk近傍ゼロの経路は保持する。GPU色の[24,34,50]は強いLODの端sampleでも生じるが、微分は最後の素材内外のboolを変えないため、色だけから座標差の大きさは断定しない。微分・符号付きdenの代用値・tの無効判定・素材のマスク条件と既存1/64の縁の許容値は変更していない。独立したf64半角式＋板の前方ホモグラフィの2×2逆行列で、失敗画素が左辺外となること、CPU座標との一致、実GPUの背景色を要求する専用回帰を追加した。元ログだけでdriverのasinやFMAのどれが誤差を生んだかは特定しておらず、修正後の実GPUでの成否も設計担当の再実行を待つ。

3. **画面snapshotの内側の色 (テストの前提)**: RootDepthViewerは最初の白いDepth fixtureの後、`source_texture_test_support::seed` で[60,120,180,255]の不変sourceを入れている。報告の実画素は板の内側の正しい素材色で、白という旧期待値が誤りだった。入力pixelsがその色であることと、無回転/トリムなし/全素材のfixture条件を明示して、内側の期待値をそのsource色へ修正した。§18.2のk=0のピンホールの前方写像から独立に2×2逆行列を解き、光線の有効性と板の内外をCPUの交点・source写像と照合する。各Canvas/capture/exportで内側と外側の両方を必ず検査し、内側はsource色、外側は背景色との厳密な画素一致を要求する。§18.2の6〜9と§18.4の「同じ素材・snapshotの出力」に基づく期待値の訂正で、出力経路は変更していない。

**設計との差分**: 数学的な写像と仕様の差分はない。変更した期待値の根拠は上記の§18.2/§18.4。変更ファイルは `src/plane_view.rs`、`src/plane_view/tilt_tests.rs`、`src/plane_view_wgpu.rs`、`src/plane_view_wgpu/gpu_tests.rs`、`src/ui_fullscreen/plane_view_tests/gpu_tests/tilt_input_tests.rs`、本書§11。追加した回帰は `plane_tilt_near_edge_on_band_matches_forward_projection_and_converges` と `plane_tilt_equisolid_yaw100_edge_uses_closed_form_before_projective_division`。再検証は上の関連filterで行える。コンパイル、WGSL検証、GPUの修正後の画素、実機の見た目と性能は未確認。ビルド・test・check・clippy・算術用スクリプト・コミット・アプリ起動は行っていない。

今回の許可された検査は `cargo fmt` 正常終了、UI字形は危険な字形なし、サイトは54カード・超過0、`git diff --check` 正常終了。追加した算術・画素のassertは未実行の検証条件であり、成功を確認した結果ではない。

### 段 2 の実装記録 (魚眼による入場時のTilt所有)

着手時 `c689c8cfb`。入る前のTiltが押下を所有し、まだ移動していない状態で魚眼のホイール/`FsFisheyeIn` により入ると、`apply_plane_view_outcome` → `set_fs_exclusive_view` の一律の `retire_plane_drag` でその所有が消えていた。傾けの初回移動だけが別の専用入口を使っていたためであり、予測時の `tilt_drag_owned=true` と入場後の実所有も食い違った。

今回の補足指示に従い、公開は `set_fs_exclusive_view` の1か所へ統一した。入場先がPlaneViewで、同じanchorのTiltを所有していれば、その所有全体を保持する。それ以外 (別ページ/世代/sliceのanchor、Lens、PlaneView以外への交代/終了) は従来どおり退役させる。傾け専用の `enter_plane_view_from_tilt_drag` とprivateの `publish_fs_exclusive_view` は削除し、傾けの初回移動も同じsetterを使う。Panoramaの後処理、古いprojectionのtake/dropとDepthの取消、通知などの既存の資源退役処理は同じsetterの本体に維持した。所有を退避して戻す処理や新しい入口は追加していない。

回帰は実viewerの `plane_tilt_fisheye_wheel_and_key_entry_preserve_pressed_owner_until_release`。押すだけの状態から、ホイールと割り当てたキーの両方で魚眼へ入る。予測入力の `tilt_drag_owned` と公開後の実所有を照合し、start/previousを含む所有全体の一致、pose/倍率/panの予測との一致を検査する。その後に修飾を離して移動してもパン/自由回転ではなく傾けが続き、解放で所有が解けて傾きが残ることを通す。公開境界の `plane_tilt_publication_preserves_only_the_same_anchor_tilt_and_clears_on_exit` は別ページ・世代・sliceと同じanchorのLensを退役させ、同じanchorのTiltだけを保つことと、終了で必ず解くことを検査する。GPUを使う前者は既存gpu_tests内、後者はGPUを使わない既存app/plane_viewの試験moduleに置いた。

**設計との差分**: 今回の補足により、§18.3の所有を保つ公開の考え方を、傾けの初回移動だけでなく同じanchorの魚眼入場にも適用した。新しい操作仕様の差分はない。改訂1/2の幾何・固定、式、detachedの述語/viewport/配送は変更していない。変更ファイルは `src/app/depth_view.rs`、`src/app/plane_view.rs`、`src/ui_fullscreen/plane_view_tests/gpu_tests/tilt_input_tests.rs`、本書§11。新しいUI PNGは不要。

設計担当は上の2つの試験名をnextestで指定でき、関連回帰は既存 `test(/plane_view|plane_tilt|fisheye|depth/)` で照合できる。コンパイル・試験・GPU画素・実機での確認は実装担当では未実施。コミット・アプリ起動は行っていない。

今回の許可された検査は `cargo fmt` 正常終了、UI字形は危険な字形なし、サイトは54カード・超過0、`git diff --check` 正常終了。Rustの試験・コンパイルの成功は未確認。

## 12. 文書

- [display-pipeline.md](display-pipeline.md): PlaneView の節 (§5 の要約と §6 の出口)。
- [keymap-spec.md](keymap-spec.md)・`docs/keymap.ini.default`: Action 3つ、固定入力3つ (Ctrl+Shift+ホイール、PlaneView中のShift+左ドラッグ、対象の単ページ静止画のCtrl+Shift+左ドラッグ)。
- [architecture-overview.md](architecture-overview.md): 新しいモジュール。
- マニュアル (全画面の表示のページ) と製品ページ: 機能の説明 (版固有の説明・実装用語を書かない)。
- [detached-rework-plan.md](detached-rework-plan.md): §4.4 の記録。
- backlog: §1.917 と §1.819 の状態をこの設計書へ向ける。
- CHANGELOG の下書き (§13)。

## 13. CHANGELOG の下書き

- 余白の出ない魚眼ズームを追加しました。1ページの静止画で Ctrl+Shift+ホイールを回すと、画像が画面を覆うまで寄り、余白を作らない範囲で歪みを加えます。歪みを固定していないときは、画像の端を画面に入れると歪みが消えます。極端に細長い画像や大きな拡大・移動では、通常表示と同じ余白が残ることがあります。
- 魚眼中に Ctrl+ホイールなどの通常のズームを使うと、そのとき見えている歪みを保ったまま倍率だけを変えられます。縮小すると周囲に余白が出ます。そのあとはドラッグで動かしても歪みを保ち、Ctrl+Shift+ホイール上で余白を作らない動きに戻ります。
- 下へ回すと歪みと倍率が戻ります。覆うために増えた倍率を最初のフィットへ戻すには、ダブルクリックかフィットの切り替えを使います。
- 魚眼中は Shift+左ドラッグでレンズの中心を動かせます。通常のズーム・パン・自由回転と組み合わせて使えます。レンズの種類は環境設定で選べます。
- 魚眼の見た目をキャプチャ保存・画像のコピー・エクスポートで出力できます。「魚眼で寄る」「魚眼で離れる」には操作カスタマイズでキーを割り当てられます。
- 1ページの静止画を、Ctrl+Shift+左ドラッグで板のように傾けられます。レンズの中心の下の点を軸に裏側まで回せ、裏側は反転した絵になります。傾けて出る余白はそのまま残ります。「写真の板の傾きを戻す」にキーを割り当てると、魚眼を残して傾きだけを戻せます。傾けた絵も見えているとおりに保存・コピー・書き出しできます。

## 14. 独立設計レビュー

### R1 (2026-11-27、GPT-6.1 Sol / xhigh、read-only、対象 `9e7ea3d6a`、session `01a218a0-07a6-7601-aa6f-3564f5381a09`)

判定: 要修正。P1 6 件・P2 8 件。設計担当 (ClaudeCode) が各指摘の根拠のコードを読み、全件を正しいと判断した (false positive なし)。

| # | 指摘 | 対応 (本書 R1) |
| --- | --- | --- |
| P1-1 | Shift の軸の付け替えで Ctrl+Shift+ホイールの縦の量が消え、今は何もしない (R0 の「今は普通のズーム」は誤り)。合算された segment の量を定義していない | §2 を訂正。§3.2 で軸を移す前の縦の量を使う口と `notches` の比例を定義。§0 に利用者への説明の訂正 |
| P1-2 | `α0 = 30°` のため `s → 0` で `W → 1` にならない。`α_max` の値、パノラマの係数の違い | `α(s) = s × α_max` に変更。`α_max` 3 値を明記。パノラマの係数は流用しないと明記 (§2・§5.1)。段 2 の透視の画角は §9 へ |
| P1-3 | 角度の主値範囲、中心の極限、平行光線、微分前の NaN / Inf | §5.1 の手順 1〜3・7・8 (定義域、`σ` の極限、`den` の代用値、有限な代用値を通してから微分) |
| P1-4 | source UV の範囲判定では表示トリムの外が描かれる | §5.1 の 8 で `in_uv` で判定。テストに組み合わせを追加 |
| P1-5 | 同じ pass の予測に PlaneView の遷移が無い。segment 間のページ移動、slice | §3.2 の純粋な遷移関数、segment 間の anchor 照合、anchor に slice (§3.1) |
| P1-6 | 書き出しの要求に出力座標の写像と「実際に描いた frame」の確定が無い | §5.3 の frame snapshot、§6.1 の `last_drawn` と照合、§5.2 の出力座標の写像 |
| P2-7 | 既存ドラッグは開始時に所有を決める規則ではない。レンズの移動との所有の重なり | §2 を訂正。§3.3 でレンズの移動だけを開始時に固定し、所有の型・終わりを定義 |
| P2-8 | ModifierHold は物理状態だけを読む。キーのズームは表示中央基準 | §2 を訂正。§3.4 で重なりの扱い (ルーペを描かない、元画像をそのまま描く)。§3.2 の基準点の記述を訂正 |
| P2-9 | detached の終了経路は session の確定点だけではない | §3.6 の注 (Depth の終了経路の列挙と共通 helper)、§4.4。**構造上の追加という分類には合意** |
| P2-10 | 素材の準備待ちと採用の順序が未定義 | §3.1 の `PlaneColorPhase`、§5.2 の素材の採用 |
| P2-11 | 表示トリムの編集と touch の扱い | §4.1 で表示トリムの編集を弾く、§4.3 の touch の配送 |
| P2-12 | 出力の寸法の説明が上限・既存の縮小段と食い違う | §6.2 の寸法の resolver、§6.3 で実寸の表示と縮小段を通さないこと |
| P2-13 | VRAM の会計と、状態の後も残る要求の資源 | §5.3 の会計・共有と分離 (pipeline は共有、uniform は分離)、§5.4 の写しの CPU 時間の計測 |
| P2-14 | 境界の受入テスト | §10 に追加 |

### R2 (2026-11-27、同じセッションの継続、対象 `519418066`)

判定: 要修正。R1 の 14 件は resolved 9・partially 5。新しい指摘は P1 3 件・P2 4 件・P3 1 件。設計担当がコードで確かめ、全件を正しいと判断した。
§4.4 の構造上の追加への合意は維持 (条件: 共通 helper の対象は Depth / PlaneView だけで、Panorama と Depth の通知を変えない)。

| # | 指摘 | 対応 (本書 R2) |
| --- | --- | --- |
| P1-1 | 部分素材の coverage が写像から抜けている (displayed_image_transform.rs:986〜1005) | §5.1 の 7 (source UV → texture UV、coverage の外を無効)、§3.1 の `PlaneMaterial` に coverage・sampler_roi (R3 で記述を拡張) |
| P1-2 | 2D で描いた後も古い snapshot を保存できる。`Pending.previous` で選択と描画が分かれる | §5.2 の `last_drawn` を frame ごとに Some / None で記録。前の素材は幾何と一体の `PlaneMaterial` として持ち、その素材自身の幾何で描く (レビュー担当の 2 案のうち、PDF の再ラスタのたびに 2D が 1 frame 挟まるのを避けられる方を採った) |
| P1-3 | ページ送りをまたぐ frame で予測と実適用が食い違う | §3.2 でページ送りの後の魚眼の量を予測・実適用とも捨てる。クリックの分類を計画元の表示単位に束縛 |
| P2-4 | 登録待ちの後の次の pass の起床が無い | §5.2 で所有 viewport の次の pass を要求 |
| P2-5 | 離れる操作の量が分割と合算で食い違う | §3.2 の `used = max(notches, −8 × strength)` |
| P2-6 | 書き出しで升目を二重に拡大縮小する | §5.1 の 9・§6.1 で下地を canvas の座標に統一、出力の画素の写像を式で明記 |
| P2-7 | bind group と target format の分け方 | §5.3 (group 0 = 素材、group 1 = uniform、pipeline は format ごと) |
| P3-8 | 「一時的に 2 枚」は上限ではない | §5.3 の記述を訂正、§10 に 3 世代以上の会計のテスト |

### R3 (2026-11-27、同じセッションの継続、対象 `5135c8dc1`)

判定: 要修正。R2 の 8 件は resolved 6・partially 2。新しい指摘は P1 2 件・P2 2 件。設計担当がコードで確かめ、全件を正しいと判断した。
旧い素材を今の表示状態で解決し直す方式は成立する、`last_drawn` と書き出しの方針・クリックの束縛・canvas の座標の下地・group の分離・
format ごとの pipeline に構造上の問題は無い、との評価。

| # | 指摘 | 対応 (本書 R3) |
| --- | --- | --- |
| P1-1 | `TextureId` だけでは `PlaneMaterial` の再利用を判定できない (テクスチャを変えずに PDF の表示の基準だけ更新する経路、app.rs:22735 付近) | §3.1 で鍵を 2 つに分けた (`PlaneGpuCopy` は `TextureId`、`PlaneMaterial` は記述全体)。記述だけ変われば写しを共有した新しい記述 |
| P1-2 | `layout_basis` の等価条件と、旧い素材から引き継ぐ配置の入力が未定義。material kind が無い | §5.2 で前の素材から取るもの / 今の frame から取るものを列挙。使ってよい条件は「配置の矩形の一致」(基準の `==` ではない)。配置の確定に実際に描いた素材の material kind。PDF の需要は今のまま |
| P2-3 | キーの `FsFisheyeIn/Out` はホイールより先に処理されるので「ページ送りの後」の順が無い | §3.2 でキーは今のキーの段で先に適用し、破棄の規則は wheel segment だけ |
| P2-4 | 出力の座標で +0.5 を二重に足す | §6.1 で WGSL は fragment の座標をそのまま使い、CPU 版の整数の番号だけ `i + 0.5` |

### R4 (2026-11-27、同じセッションの継続、対象 `2a79e16d7`)

判定: 要修正。R3 の 4 件は resolved 3・partially 1。新しい指摘は P1 1 件・P2 1 件。設計担当がコードで確かめ、どちらも正しいと判断した。
coverage・trim・回転は、今の入力の分担で追加の誤参照は無い、との評価。

| # | 指摘 | 対応 (本書 R4) |
| --- | --- | --- |
| P1-1 | `full_image_rect` (contain・丸めの後) の一致では、通常の PDF の再ラスタでも 1 物理画素ずれて 2D の frame が挟まる | §5.2 で、比べるのを sampler を contain する前の配置枠 (指定倍率の `layout_full_image_rect`、legacy の中間の配置枠) に変更。描画・確定・snapshot は前の素材自身の変換。§10 に丸めの回帰テスト |
| P2-2 | 指定倍率の PDF の需要は描画後の `layout_full_image_rect` の経路からも作られる | §5.2 で、需要は 2 つの経路とも常に今の素材の変換から作ると明記。配置枠の一致を条件にしたので値は同じ |

### R5 (2026-11-27、同じセッションの継続、対象 `fd8e743de`)

判定: 要修正。R4 の 2 件は resolved 1・partially 1。新しい指摘は P1 1 件・P2 1 件。設計担当がコードで確かめ、どちらも正しいと判断した。
旧い素材自身の変換を描画・配置の確定・snapshot・書き出しに使う方針は妥当、coverage・trim・回転に追加の問題は無い、との評価。

| # | 指摘 | 対応 (本書 R5) |
| --- | --- | --- |
| P1-1 | 需要用に今の素材の変換を `fullscreen_page_layout` へ入れると、実際の描画の記録 (パンの制限などが使う) が壊れる | §5.2 で `fullscreen_page_layout` には実際に描いた素材の変換を記録し、需要用の配置枠は描画の戻り値に並べて需要の入口だけへ渡すと明記。§10 にテスト |
| P2-2 | 丸める前の配置枠にも `f32` の誤差が残るので、完全一致では 2D が挟まる | §5.2 で一致を「各辺の差が 1/64 物理画素以下」と定義 (比較だけに使い、値は近似しない)。§10 に倍率と reference の長辺を変えた回帰テスト |

### R6 (2026-11-27、同じセッションの継続、対象 `cbe38d4c3`)

判定: **実装開始可**。R5 の 2 件はどちらも resolved、新しい指摘なし。レビュー担当が既報の条件を `f32` で再現し、倍率 1・1.1、
`pixels_per_point` 1・1.25・2 で配置枠の最大の辺の差が約 0.000061 物理画素 (許容 1/64 = 0.015625 の内側) であることを確かめた。
§4.4 の構造上の追加という分類への合意は R2 の条件つきで維持。

## 15. 独立実装レビュー

### I1 (2026-11-27、GPT-6.1 Sol / xhigh、read-only、実装者とは別のセッション、対象 `6c83a526d`)

判定: 要修正。設計担当がコードで確かめ、P1〜P3 はすべて正しいと判断した。
§11 の設計との差分 2 点 (パネル上の Ctrl+Shift+ホイールの入場の拒否、pause の入口への共通の終了 helper の追加) には合意。
detached への変更は既存の owner への variant の追加と終了の共通化で、症状パッチではない、との判断。
式・`alpha_max`・地平線 / 定義域のマスク・微分の位置・coverage / 回転 / trim・CPU 参照との対応、Arc の保持・callback ごとの uniform・
format ごとの pipeline・VRAM の会計・登録待ちの次 pass の要求、`ColorMipCopier` の括り出しによる 3D 写真の変化、
UI スレッドの I/O・読み戻しの待ち・Context の再帰 lock には、追加の欠陥は無い、との評価。

| # | 指摘 | 対応 |
| --- | --- | --- |
| P1-1 | 全画面の「画像をコピー」(`FsCopyImage`・右クリック・リング) が元ファイルを読み直し、魚眼が反映されない | 画像コピーの共通の配送点で PlaneView を判定し、照合済みの snapshot の出力要求 (Canvas の寸法) へつないだ。3 つの入口の試験を追加 |
| P2-2 | 魚眼が成立しないキー / ホイールでも初期配置の要求を消し、pan の clamp・PDF の需要の処理を走らせる | 遷移が成立したときだけ副作用を適用する `apply_plane_view_outcome` に集めた。拒否 / no-op で初期配置の要求と pan が残る回帰を追加 |
| P2-3 | クリックの境界の試験が実際のクリックを入力していない | 入場 / 終了と同じ pass のクリック・ダブルクリック、ページ / slice の交代後の破棄を実入力で検査 |
| P3-4 | spec.md の既存の節の見出しと本文が分かれた | 魚眼の節を既存の節の本文の後へ移した |
| (不足) | §10 の元画像ホールド・実ピンチ・PDF の需要の両入口・登録待ちの起床要求 | それぞれ試験を追加 |

修正の途中で、PageTurn の通常キャンバスのクリックが、1 pass に届いた複数の離しを 1 回と数えていた既存の欠陥が見つかった
(同じ pass の入場とダブルクリックの試験で露見)。DoubleClickClose 側の既存の数え方 (離しごとに照合) に揃えて直した
([fullscreen-double-click-close-plan.md §16](fullscreen-double-click-close-plan.md))。再レビューで名指しで確かめる。

関連の nextest (plane_・context_menu・click・double_click・touch・page_turn・depth_・preferences・capture・gamepad ほか) 3630 件が共有 runner で成功。

### I2 (2026-11-27、I1 と同じレビューのセッションの継続、対象 `0f4eeaf34`)

判定: **取り込み可**。I1 の指摘はすべて resolved、新しい指摘なし。
PageTurn の `accept_backing_click` の変更は、Response 単位の集計を所有情報つきの離し単位へ直す構造的な修正で、症状パッチではない、との判断。
別 pass の単 / 2 / 3 回のクリック、一覧から持ち越した raw count、前面の所有・layer・focus・touch の置き換えの抑止、3 回での飽和、
DoubleClickClose の 2 回ごとの区切りに退行は無い。`.expect` は vendored egui が離しごとに必ず 1 件の認定を積むため
(座標つき / 座標なしの両方、canvas の外も `None` で対応、`PointerGone` は離しを作らない)、製品の入力で panic する経路は無い、との評価。
通常の全画面・一覧のコピーは従来の経路のまま。

## 16. 改訂 1 — 余白の出ない魚眼 (対角魚眼)

### 16.1 経緯と決定 (2026-11-27、利用者が設計担当のセッションで直接)

- 利用者が段 1 の build-dev で試した結果:「画像の周囲に余白がすぐにできてしまうのが気になりました。一般的にはレンズが被写体に
  近づいて変形する形ですが、そのイメージではなくて画像全体が変形するような印象です。Ctrl+Shift+ホイールで拡大するとき、
  被写体に近づいてカーソル位置近辺で歪み＋拡大になるといいのですが」
- 原因 (§5.1 の式から。設計担当は画面を見ていない): `α_max` (等距離で `r = 1` に 112.5°) のため、`s = 1` で平面全体が半径 `0.8 R` の
  円に収まる「円周魚眼」になる。加えて 8 ノッチで倍率は約 2.14 倍にしか上がらず、画像の端が内側へ引き込まれる。
  16:9・レンズの中心が中央・画面と同じ縦横比の画像なら、画像の端による余白が角に 2 ノッチ目、上下の中央に 4 ノッチ目から出る
  (地平線の円が角に入るのは 4 ノッチ目、上下に入るのは 7 ノッチ目で、それより前に画像の端の余白が出る。R7 の検算)。
- 利用者の決定:
  1. **余白の出ない範囲に歪みを自動で抑える** (案 1。「案2 ルーペ型」は採らない)。
  2. 画像の端が画面に見えている状態 (例: 縦長の画像を横長の画面でフィット) からの寄る操作は、
     **最初の 1 ノッチで画像が画面を覆う倍率まで拡大し、以降のノッチで歪みが増える** (推奨案。「覆うまで拡大だけ」「今ある余白は広がってよい」は採らない)。
- 「今ある余白は広がってよい」を採らない理由 (利用者へ説明済み): 余白のある方向を条件から外すと、画像が画面を覆った瞬間にその方向の条件が
  加わり、歪みを 0 近くまで戻す必要が出る (連続にならない)。

この節が §3.2 の strength の刻み・§3.5 の定数・§5.1 の「画角」・§7 の「strength 0 の PlaneView」・§10 の該当テストを置き換える。
それ以外 (状態の型・入口条件・終了・ドラッグ・キー・素材・書き出しの経路・detached の扱い) は変えない。

### 16.2 係数 `k` の決め方 (§5.1 の「画角」を置き換える)

`k = s × k_fill`。`s` は今の `strength` (0 < s ≤ 1)。`k_fill` はその pass の描画の入力だけから決まる「余白を作らない最大の `k`」と上限の小さいほう。
`s → 0` で `k → 0` (恒等) なので、入った直後に段差は出ない (§5.1 の性質はそのまま)。

記号 (物理画素): canvas・`R`・`c`・`g` は §5.1 のまま。`v = p − c`。

- **有効四角形 `Q`** (R7 P1-1): shader が色を取る点の集合そのもの。描く素材の `in_uv ∈ [0,1]²` (表示トリム・横長分割の範囲) **かつ**
  source UV が `coverage ∩ material_source` の中 (§5.1 の 7・9)。2 つとも paint 四角形の局所座標 (自由回転を戻した座標) で軸に平行な長方形に
  なる (90 度回転は軸を入れ替えるだけ) ので、その交わりの長方形を自由回転で回した凸四角形が `Q`。作り方は既存の
  `DisplayedImageTransform` の source と screen の写像と、**実際に描く素材の記述** (`PlaneMaterial` の coverage・sampler_roi) を使う
  (別の式を書かない)。`k_fill`・16.3 の「覆う」の判定・テストは同じ `Q` を使う。
- **余白なしの条件**: canvas のすべての点 `p` で `q(p) ∈ Q`。`Q` の各辺を半平面 `n_j · v ≤ a_j` (`a_j` は `c` から辺までの距離) で書くと、
  傾き 0 では `q − c = v · W(|v|/R)` なので条件は `n_j · v ≤ a_j · ψ(k|v|/R)`、`ψ(x) = x / tan(g⁻¹(x))`。
  3 種のレンズとも `ψ` は凹で単調減少なので `n_j · v − a_j · ψ(k|v|/R)` は `v` について凸 (R7 の指摘)。凸関数の canvas (凸多角形) 上の最大は
  頂点で取るので、**canvas の 4 隅 × `Q` の 4 辺の 16 個の不等式を満たせば、canvas 全体で余白は出ない** (標本の取りこぼしが無い)。
- 1 つの不等式 (隅 `v`、辺 `j`) の解き方: `n_j · v ≤ 0` なら制約なし。そうでなければ `m = a_j / (n_j · v)`。
  `m < 1` (k = 0 でもその隅は `Q` の外 = 2D で画像の端が画面に見えている) なら `k_fill = 0`。それ以外は `h(θ) = tan θ / g(θ)`
  (3 種とも `[0, 90°)` で 1 から単調に増える) について `h(θ*) = m` を解き、`k ≤ g(θ*) · R / |v|`。等距離は `θ*` を数値で解き
  (二分法または Newton、固定の回数)、立体射影は `tan²(θ*/2) = 1 − 1/m`、等立体角は `sin²(θ*/2)` の 2 次式で閉じた式になる。
- `c` が `Q` の外 (`a_j < 0` の辺がある) → `k_fill = 0`。`c` が `Q` の辺の上 (`a_j = 0`) は許す (凸性の議論は `a_j ≥ 0` で成り立つ。
  その辺で `n_j · v > 0` の隅があれば `m = 0 < 1` で `k_fill = 0`、無ければ制約なし。R8 P2-5)。canvas の隅が `c` と一致する (`|v| = 0`) 不等式は飛ばす (除算しない)。
- **上限**: canvas の中で `c` から最も遠い点 (canvas の隅) の入射角が `θ_cap = 80°` を超えない: `k ≤ g(θ_cap) · R / ρ_far`。
  これで canvas の中に地平線・定義域の外は出ない (shader のマスクは安全のため残す)。`θ_cap` は定数で、実機で見て詰める。
- 数値の例 (16:9、レンズの中心が中央、画面と同じ縦横比の画像、等距離。scratchpad の検算): 倍率が「覆う倍率」の
  1.1 / 1.46 / 2.14 / 3.1 倍のとき、角の入射角は 29.7° / 54.0° / 68.7° / 76.4°。`tan 80° / (80° の弧度) ≈ 4.06` なので、
  **約 4.1 倍で上限 (80°)** に達し、その後は倍率だけが上がる (段 1 の「strength 1 の後は倍率だけ」と同じ形)。
- **計算の置き場**: 純粋な関数 `plane_fill_coefficient(canvas, c, Q, lens) -> f32` を 1 つ作り、画面の描画
  (`PlaneRenderParams::screen`) だけが呼ぶ。書き出し・キャプチャ・コピーは今どおり snapshot の uniform の `k` を使い、計算し直さない。
  `PlaneLensKind::alpha_max` / `coefficient(strength)` は削除する。
- **負荷**: 16 個の不等式と上限だけで、量は canvas の大きさに依らない。キャッシュは持たない (状態を足さない)。計算区間は `perf::event` に入れる。
- **見た目が 2D と同じ PlaneView**: `s > 0` でも `k_fill = 0` になることがある (PlaneView 中に普通のパン・ズームで画像の端を画面に入れた場合)。
  そのあいだ見た目は 2D と同じだが PlaneView のまま (§4.3 の占有は続く。クリックの分類は描画の結果ではなく pose の有無で決まるので、
  予測と実適用の食い違いは出ない。R7 で確認)。次の寄る操作で 16.3 の「覆う」が働き、歪みが戻る。終わり方は今どおり
  (strength 0・ダブルクリック・フィットの切り替え・ページ移動)。描画の結果で状態を変えない (描画から終了を起こすと状態の持ち主が 2 つになるため)。

### 16.3 寄る操作で画面を覆う (§3.2 の「寄る」を置き換える部分)

寄る segment (`notches > 0`。入場の segment と `FsFisheyeIn` を含む) の倍率と pan は、次の 2 段で決める。**順序が要点** (R7 P1-3):
先に覆う配置へ揃え、その後に今回の通常の拡大をレンズの中心の固定で行う。

1. **覆う配置へ揃える (覆っていれば何もしない)**: 今の `(z, pan)` の**連続の配置** (下の「共通の resolver」) の `Q` が、canvas を各辺 `δ` ずつ
   広げた矩形 `canvas⁺` を覆っているなら、そのまま。`δ` は「連続の配置と、描画が使う量子化した配置の `Q` の辺の差の上限」(物理画素) で、
   **`δ = 4` 物理画素** (R9 の評価: 全体矩形の原点とサイズの丸め、トリムの物理画素への丸め、潰れた帯の 1 画素の確保、回転中心の移動を
   辺の法線方向で保守的に合計して `√2 + 0.5 + 1.5 < 4`。displayed_image_transform.rs:1752・1874・1888)。
   - **保証する範囲を決め、その外は証明しない** (R11 への対応、組み合わせを減らす): `f32` の乗除算の誤差は座標の大きさと表示の縦横比で増える
     (§11 の実装担当の算術、R11 の算術)。そこで **「普通の範囲」= 表示の縦横比 (長辺 / 短辺) が 32 以下、かつ遷移の後の連続の配置の座標の絶対値が
     `2²¹` 物理画素以下** (この 2 つの値は**候補**) に限って、`δ = 4` で「連続の配置で `canvas⁺` を覆えば量子化した配置も canvas を覆う」ことを保証する。
     確定の条件は **「丸めと `f32` の演算を合わせた、`Q` の各辺の法線方向の総誤差が 4 物理画素以下」** (幾何の丸めだけで約 3.414 なので、`f32` の分の
     予算は約 0.586 物理画素。R12 P2-1)。実装担当は、量子化の経路 (倍率の再計算と平均、displayed_image_transform.rs:513・1757 を含む) と同じ演算の算術と
     16.5 の総誤差のテストで確かめ、満たさなければ範囲を狭めて確定し、§11 に確定した範囲と根拠を書く (`δ` は変えない)。
   - 普通の範囲の外 (極端に細長い画像・極端な倍率と pan) は、覆う計算は同じ式で行うが、量子化した配置が canvas を覆うことは保証しない。
     覆わなかった frame は、描画の `Q` から決まる `k_fill` が 0 になり、2D と同じ見た目になる (歪みによる余白は作らない。2D の余白は残る)。
     誤差の上限を範囲の外まで証明する計算 (座標の上限・縦横比の増幅を含む `δ` の式) は作らない。
   確定した保証範囲の内では、これで連続の配置で覆えば描画の配置も canvas を覆う。覆っていなければ:
   - 倍率 `z₀ = max(z, z_cover)`。`z_cover` は連続の配置の `Q` が (位置を問わず) `canvas⁺` を覆える最小の倍率。連続の配置では `Q` の寸法は倍率に
     比例するので、倍率 1 あたりの半寸法 `(A₁, B₁)`、自由回転 `φ`、`canvas⁺` の寸法 `(w, h)` から
     `z_cover = max((w|cos φ| + h|sin φ|) / (2A₁), (w|sin φ| + h|cos φ|) / (2B₁))` が厳密に決まる (反復しない。R8 P1-2)。
   - 倍率を `z₀` に変えるときの pan は、**レンズの中心の下の source の点を画面上で固定する** pan とする (下の「共通の resolver」)。
   - その後、`Q` の局所座標の 2 軸それぞれで、`canvas⁺` の 4 隅が入る区間へ移動量を clamp し、pan に足す (各軸独立の閉じた式)。
2. **通常の拡大**: `f` = 今の段の倍率 (`apply_wheel_zoom` と同じ式、`used × WHEEL_ZOOM_POINTS_PER_NOTCH`)。`z' = z₀ · f`、pan は
   **レンズの中心の下の source の点を画面上で固定する** pan。覆った `Q` を `c` のまわりに広げるので、覆いは保たれ、`canvas⁺` の外側に余裕ができて
   `k_fill > 0` になる。

- **共通の resolver** (R7 P1-2・R8 P1-1/P1-3): 倍率と pan から `Q` を求める計算は、単ページの描画と**同じ経路** (`resolve_fs_image_transform`。
  SourceLayout の配置枠を作ってから sampler を contain する。ui_fullscreen.rs:11118) で行い、`DisplayedImageTransform::resolve` を直接呼んだり、
  中心の単純な拡大で代用したりしない。単ページの左右配置の gap (倍率に比例しない固定の offset、displayed_image_transform.rs:448)・表示トリム・
  PDF / RAW の配置枠はこれで含まれる。
  - **遷移の計算 (覆う判定・`z_cover`・source の点の固定・pan の補正) は、同じ経路の連続の配置** (`RectPixelFit::Proportional`。物理画素へ寄せない) で行う。
    連続の配置では pan は `Q` の平行移動、倍率は固定の offset を除いて比例なので、「`c` の下の source の点を固定する pan」は閉じた式で 1 回で決まる。
    量子化した配置の逆写像を次の基準へ戻さない (戻すと丸めの誤差が積もり、分割と合算が崩れる。R8 P1-3 の例)。新しい永続の状態は足さない。
  - **描画と `k_fill` は今どおり量子化した配置** (`RectPixelFit::Texels`) を使う。確定した保証範囲の内では、連続の配置で `canvas⁺` を覆えば、量子化した配置も canvas を覆う (`δ` の定義)。
- **予測と実適用が同じ幾何を使う** (R7 P1-4・R8 P1-4): 2 つを分ける。
  - **素材の側は前の描画の記録から**: 単ページの描画 (PlaneView の分岐は実際に描いた素材の `old_input`・`old_layout` を既に持つ。ui_fullscreen.rs:47394・47475)
    で、**実際に採用した素材**の受理済みの情報 (texture・source の寸法・SourceLayout の基準・coverage・sampler_roi・material kind) を、表示単位と一緒に記録する。
    外側の「今の素材」用の入力は記録しない。
  - **表示の条件は今の pass から**: ホイールの予測より前に、記録した素材の情報と、**今の pass の** canvas・`pixels_per_point`・90 度回転・自由回転・
    表示トリム・fit の設定・配置 (単ページの左右配置の gap を含む) から、単ページの描画と同じ組み立てで resolver の入力を作り、canvas と素材の記述と
    合わせて 1 つの値 (`PlaneCoverInput`) に確定する。窓の大きさ・DPI が前の描画から変わっていても、今の条件で判定する (R8 の resize の例)。
  - 確定した値は `FsWheelFrameProposal` に持たせ、実適用 (描画後の遅延適用・segment の再分割を含む) はこの値を使う。`fullscreen_page_layout` から作り直さない。
  - **記録が無い** (ページを移った直後で、新しいページをまだ 1 度も描いていない frame) ときは、その frame の Ctrl+Shift の量を使わない
    (何もしない・通知もしない)。§3.2 の「ページ送りの後の量は捨てる」と同じ扱いで、別の作り直しの経路は作らない (組み合わせを増やさない)。
  - キーの `FsFisheyeIn` も同じ `PlaneCoverInput` の producer と、実際の viewer の canvas を使う。
- `ZOOM_MAX` で `z_cover` に届かず覆えない場合 (と、上の「普通の範囲」の外で量子化した配置が覆わなかった場合) は、届く倍率まで上げて覆う処理を
  やめる。**描画の `Q` が canvas を覆う場合は、歪みによる余白を作らない。描画の `Q` が canvas を覆わなければ `k_fill = 0` の恒等表示で、
  2D と同じ余白が残る** (R11 P2-3)。まれな場合として割り切る
  (ログも通知もしない。見た目は 2D のまま)。
- 離れる (`notches < 0`) は今どおり。覆う処理はしない。倍率が覆う倍率を下回れば `k_fill = 0` になり、見た目は 2D に戻る。
  **既知の挙動**: 覆うために飛んだ倍率は離れる操作で元に戻らない (入る前の倍率を状態に持たない。戻すにはダブルクリック・フィット)。
- **strength の刻み**: 1 ノッチ **1/4** (段 1 の 1/8 から変更。定数、実機で詰める)。離れる量は `used = max(notches, −4 × strength)`。
  `k_fill` が倍率とともに 0 から増えるので、刻みが細かいと最初の数ノッチがほとんど歪まないため。
- **分割と合算**: 1 は覆っていれば何もしない (冪等) ので、`+2` を一括で渡すのと `+1, +1` に分けるのとで、1 を 1 回通った後に
  レンズの中心を固定した拡大 `f₂ = f₁ · f₁` を行うことになり、倍率・strength・pan・描画の結果が一致する (pan は量子化の範囲で一致)。
  §10 の「分割と合算」の性質に pan と `k_fill` を含める。
- **pan の最終の制約も同じ純粋な遷移の中で決める** (R9 P1-1): 今の実装は予測・実適用の両方で、遷移の結果をその時点の
  `fullscreen_page_layout` で再クランプしている (ui_fullscreen.rs:43414・43488 の `clamp_fullscreen_pan_from_layout`)。layout の viewport と
  描画の矩形は `PlaneCoverInput` と違い得る (resize の後・描画後の遅延適用で layout が作り直された後) ので、**Ctrl+Shift の遷移の結果には
  layout による再クランプをしない**。代わりに、§1.806 の最小の重なりの制限 (今の `pan_correction_for_minimum_overlap` と同じ規則) を
  **同じ `PlaneCoverInput` の連続の配置**から求め、遷移の関数の最後に 1 回だけ適用する。覆っている配置はこの制限を満たすので動かない。
  離れる操作・`ZOOM_MAX` で覆えない場合の pan も同じ制限で決まる。予測と実適用はどちらもこの関数の結果をそのまま使う
  (アダプタで別の入力を使って直さない)。Ctrl+Shift 以外の入力 (普通のホイール・ドラッグ・キー) のクランプは今のまま。

### 16.4 変えないもの

shader (uniform の `k` を受ける形のまま)・地平線と定義域のマスク・書き出しの経路と snapshot の照合・レンズの中心の倍率 = 2D の倍率
(PDF の需要は今のまま)・Shift+左ドラッグ・入口条件・終了の規則・キー・設定 (レンズの種類)。detached / viewer context の扱いも変えない
(新しい値は `FsWheelFrameProposal` の中の入力の写しと、表示単位を描いた記録に足す resolver の入力だけで、viewer context の退避・引き継ぎの対象を足さない)。

### 16.5 テスト (§10 に足す・置き換える)

- **余白なし**: 小さな canvas (例 `160×90`・`90×160`) の全画素の中心を CPU 版の式で写し、`Q` の外に出る画素が無いこと (許容 1/64 物理画素)。
  組み合わせ: レンズ 3 種 × レンズの中心 (中央・端寄り・角寄り・canvas の隅ちょうど) × 画像 (画面と同じ縦横比・縦長・横長) × 自由回転 (0・17°) ×
  表示トリムあり × 90 度回転 × 部分素材 (coverage が全体の一部) × 倍率 (覆う倍率の 1.0・1.1・2・5 倍)。
- **4 隅で十分なこと**: 上の組み合わせで、16 個の不等式による `k_fill` が、canvas の境界を細かく標本した参照の最大値と 0.1% 以内で一致すること。
- **ほぼ最大**: `k_fill > 0` で上限に達していない場合、`k_fill × 1.01` では canvas の境界のどこか (細かく標本した参照) が `Q` の外に写ること。
  `k_fill = 0` の場合は、正の小さな係数 (例 `1e-3`) で canvas の境界のどこかが `Q` の外に写ること (R7 P2-6)。
  `c` が `Q` の辺の上でも、その辺の外へ向かう canvas の隅が無ければ `k_fill > 0` になること (R8 の例: canvas `[0,100]²`、`c = (0, 50)`、`Q = [0,500]×[−100,200]`)。
- **0 になる場合**: 2D で画像の端が見えている・`c` が `Q` の外、で `k_fill = 0`。
- **部分素材**: canvas の 2 倍幅の paint 四角形に、中央の半分だけを収録した素材を貼った場合、`Q` は収録範囲に縮み、余白が出ないこと (R7 P1-1 の例)。
- **上限**: 最も遠い canvas の点の入射角が 80° 以下。上限に達した後は、倍率を上げても `k_fill` が変わらないこと。
- **単調**: レンズの中心を固定して倍率を上げると `k_fill` は減らないこと (自由回転 0 と 17°)。
- **覆う処理 (純粋な遷移関数)**:
  - 2:3 の縦長の画像を 16:9 でフィット → 1 ノッチで倍率 `z_cover · f`、最終の resolve で `canvas⁺` を覆い、その pass の描画で `k_fill > 0`。
  - 覆っているときは今の計算と同じ結果 (1 が何もしない)。パンで画像の端を出した後の寄る 1 ノッチで覆い直すこと。
  - 自由回転 17°・表示トリム・`pixels_per_point` 1.25・単ページの左右配置 (gap あり、R7 の「左端 0・レンズの中心 x = 10・1.1 倍」の例) で覆うこと。
  - R8 P1-2 の例 (canvas `200×200`、画像 `1000×1000`、Page fit、表示トリムの横 `[0, 0.028]`) で、1 ノッチの後の量子化した配置が canvas を覆うこと
    (`z_cover ≈ (200 + 8) / 5.6 ≈ 37 < ZOOM_MAX`)。
  - 普通の範囲の端 (縦横比 32、座標の絶対値 `2²¹` 物理画素の近く、自由回転 0・17°・90°、`pixels_per_point` 1・1.25・1.75) で、連続の配置と
    量子化した配置の `Q` の各辺の法線方向の総誤差が `δ = 4` 以下であること (確定した範囲の端で試す)。
  - 普通の範囲の外の例 (§11 の `4096×1`・`8192×3` を Height fit、R11 の `8192×1`・大きな pan・自由回転 90°) で例外にならないこと。
    描画の `Q` が canvas を覆う frame では `Q` の外の画素が無く、覆わない frame では `k_fill = 0` で 2D と同じ画素になること (2 つを分けて試す)。
  - 確定した保証範囲の内の PDF・RAW (SourceLayout の配置枠を通る素材) で、遷移の計算の `Q` と描画の `Q` の差が `δ` 以内であること。
  - R8 P1-3 の例 (canvas・画像とも `900×900`、倍率 `1.02`、pan.x = `0.23`、レンズの中心 x = `855`) で、`+2` の一括と `+0.1` × 20 の分割の画像の位置が
    `δ` 以内で一致すること。
  - 横長の canvas で覆っていた画像を縦長へ resize した直後の寄る操作で、今の canvas で判定して覆い直すこと (R8 P1-4 の例)。
  - R9 の例 (16:9 の画像、前の canvas `900×1600`・倍率 4・pan.x = `2202` から canvas を `1600×900` へ変え、レンズの中心 x = 0 で 1 ノッチ寄る) で、
    予測・即時の実適用・描画後の遅延適用 (間に layout が作り直される) のどれでも、倍率と pan が遷移の関数の結果 (倍率 4.4・pan.x ≈ `2502.2`) のままで、
    layout による再クランプで変わらないこと。離れる操作でも最小の重なりの制限が同じ入力から決まること。
  - R7 P1-3 の例 (canvas `1600×900`、画像 `600×900` 中央、レンズの中心 `(550, 450)`、標準のズーム) で、`+2` の一括と `+1, +1` の分割の
    倍率・strength・pan・`k_fill` が一致し、どちらも `k_fill > 0`。タッチパッドの小さな量 (0.1 ノッチずつ) の分割でも一致すること。
  - `ZOOM_MAX` で覆えない場合は `k_fill = 0` のまま例外にならないこと。
  - 予測と実適用で結果 (クリックの分類を含む) が一致すること。描画後の遅延適用でも、予測の時点の `PlaneCoverInput` を使うこと
    (間に layout が作り直され、素材が差し替わった場合を含む)。新しいページをまだ描いていない frame の Ctrl+Shift の量は、予測でも実適用でも使わないこと。
- **刻み**: 4 ノッチで strength 1、離れる量は `used = max(notches, −4 × strength)` (段 1 の 8 の場合のテストを置き換える)。
- 段 1 の `α_max` の値・半径 `0.8 R` の地平線を固定したテストは削除する。CPU 版の `camera_point` は `strength` ではなく `k` を受ける形にし、
  書き出しの画素一致のテストは snapshot の `k` で今どおり比べる。

### 16.6 文書

- マニュアル (`htdocs/mimageviewer/manual/fullscreen.html`): 寄るほど歪むこと、画像の端が見えているときは最初の 1 ノッチで画面を覆うまで
  拡大すること、画像の端を画面に入れると歪みが消えること。
- `docs/display-pipeline.md`・`docs/spec.md` の魚眼の記述を合わせる。CHANGELOG の下書き (§13) は「余白の出ない魚眼」の言い方にする。

### 16.7 独立設計レビュー

#### R7 (2026-11-27、R1〜R6 と同じレビューのセッションの継続、GPT-6.1 Sol / xhigh / read-only、対象 `fcb023a41`)

判定「要修正」。境界への還元・`h(θ)` の単調性・理想的な回転矩形の `z_cover` と各軸の最小の pan 補正は正しい、`k_fill = 0` の PlaneView を
残す判断と export が snapshot の係数を使う方針は妥当、との評価。対応:

| 指摘 | 内容 | 対応 |
| --- | --- | --- |
| P1-1 | `Q` が `in_uv` だけで、shader の coverage・material_source の除外を含まない。部分素材で余白が出る | `Q` を shader が色を取る集合そのもの (`in_uv` と収録範囲の交わり) に定義し、描く素材の記述から作る (16.2) |
| P1-2 | 単ページの左右配置の gap は倍率に比例せず、トリム・量子化もあるので、`Q` の単純な拡大では覆いが外れる | 倍率・pan から `Q` を求める計算を描画と同じ resolver に統一し、レンズの中心の下の source の点を固定する pan を resolver で決める (16.3) |
| P1-3 | 覆う補正の後に拡大する順序だと、一括と分割で pan と `k_fill` が大きく変わる | 先に覆う配置へ揃え、その後にレンズの中心を固定した拡大をする順序に変更。覆っていれば何もしない (冪等)。分割と合算の性質に pan と `k_fill` を含める (16.3) |
| P1-4 | 予測と実適用の間に layout が作り直されるので、各呼び出しで layout から作ると幾何が食い違う。描く前の空の layout の扱いが無い | `PlaneCoverInput` を予測の前に確定して `FsWheelFrameProposal` に持たせる。まだ描いていない frame の量は使わない (16.3) |
| P2-5 | 周の標本の計算は 4K で約 12,000 点、操作中はキャッシュが効かない | 凸性により canvas の 4 隅 × `Q` の 4 辺の 16 個の不等式で厳密に決まる。キャッシュを持たない (16.2) |
| P2-6 | 「ほぼ最大」のテストが `k_fill = 0` で成り立たない | 正の係数に限定し、0 の場合は正の小さな係数で違反することを別に試す (16.5) |
| P3-7 | 原因の説明のノッチ数と上限の倍率の数値 | 画像の端の余白は角 2・上下 4 ノッチ目、上限は約 4.1 倍に訂正 (16.1・16.2) |

#### R8 (2026-11-27、同じセッションの継続、対象 `2138127e9`)

判定「要修正」。R7 の P1-1・P2-5・P3-7 は解消、P1-2・P1-3・P1-4・P2-6 は一部解消との評価。16 個の不等式の計算・`k_fill = 0` の pose を残す判断・
export が snapshot の係数を使う方針に追加の問題は無い、との評価。対応:

| 指摘 | 内容 | 対応 |
| --- | --- | --- |
| P1-1 | `DisplayedImageTransform::resolve` は単ページの描画の経路 (`resolve_fs_image_transform`) と違い、PDF・RAW で結果が異なる | 共通化の対象を単ページの描画の経路に変更。描画の記録は実際に採用した素材の情報から作る (16.3) |
| P1-2 | 1 回の揃え直しでは、量子化で覆いが成り立たない反例がある (トリム幅 6 画素) | 遷移の計算は連続の配置で行い、`z_cover` は厳密な閉じた式。量子化の差の上限 `δ` を `canvas⁺` の余裕にする。反復しない (16.3) |
| P1-3 | 量子化した配置の逆写像を次の基準へ戻すと、分割と合算が崩れる | source の点の固定と pan は連続の配置から求め、量子化した配置を基準へ戻さない (16.3) |
| P1-4 | 前の pass の入力は今の canvas・DPI・表示設定と違い得る (resize の例) | 素材の情報だけを前の描画の記録から取り、表示の条件は今の pass から組み直して `PlaneCoverInput` を確定する (16.3) |
| P2-5 | `c` が `Q` の辺の上で 0 を返すと、最大性のテストに例外が出る | `a_j = 0` を許し、`a_j < 0` だけを外とする。テストに R8 の例を足す (16.2・16.5) |

#### R9 (2026-11-27、同じセッションの継続、対象 `cb6aa044c`)

判定「要修正」。R8 の P1-1・P1-2・P1-3・P2-5 は解消、P1-4 は一部解消。倍率に依らない量子化の誤差の上限は構成でき (`< 4` 物理画素)、
SourceLayout の枠を変えずに連続の配置へ届く、記録は実際に描いた素材の `old_input`・`old_layout` から作れる、との確認。対応:

| 指摘 | 内容 | 対応 |
| --- | --- | --- |
| P1-1 | 遷移の結果を layout で再クランプしていて、固定した `PlaneCoverInput` を迂回する (resize・遅延適用で約 120 画素ずれる例) | Ctrl+Shift の遷移には layout の再クランプをしない。最小の重なりの制限も同じ入力から遷移の関数の中で 1 回だけ適用する (16.3) |
| (確認) | 「目安 2 画素」は証明済みの上限ではない | `δ = 4` 物理画素とし、`f32` の誤差を含めて実装で確かめる (16.3) |

#### R10 (2026-11-27、同じセッションの継続、対象 `e86c3e018`)

判定 **「実装開始可」**。新しい指摘なし。R9 P1-1 は解消 (覆う配置では最小の重なりの補正が 0 になり、離れる操作・`ZOOM_MAX` も同じ入力から決まる)。
`δ = 4` が `f32` の誤差まで収まることの確認と根拠の記録は、実装の検証事項として妥当、との評価。

#### 実装中に見つかった前提の誤り と R11 (2026-11-27)

実装担当が §11 に記録して停止した: `f32` の乗除算の誤差は座標の大きさに比例し、許される入力 (画像 `8192×3` を Height fit・倍率 50 など) では
連続の配置と量子化した配置の辺の差が約 125 物理画素になる。固定の `δ` を上げると R8 の例が `ZOOM_MAX` までに覆えなくなる。
設計担当の対応: `δ` を「丸めの幾何の分 4 + 座標の大きさに比例する `f32` の分」に分け、後者は `ZOOM_MAX` と今の pan での座標の上限から入力ごとの定数にする
(16.3)。普通の画像では後者は無視でき、極端な画像は既存の「`ZOOM_MAX` で覆えない場合」の割り切りに入る。余白が出ないことは描画の `Q` から決まる `k_fill` が保つ。

#### R11 (同じセッションの継続、対象 `42a45da2f`)

判定「要修正」。

| 指摘 | 内容 | 対応 |
| --- | --- | --- |
| P1-1 | 量子化の前の倍率の再計算と平均で、短辺の誤差が縦横比に応じて長辺で増幅され、演算の数だけで決めた `K` では足りない (`8192×1`・大きな pan・自由回転 90° で 49,152 px) | `δ_f32` の式をやめる。「普通の範囲」(縦横比 32 以下・座標 `2²¹` 物理画素以下) に限って `δ = 4` を保証し、その外は証明しない (16.3) |
| P1-2 | 今の pan で求めた `M` は、遷移の後の pan での座標の上限にならない | 同上 (`M` を使わない。範囲の判定は遷移の後の連続の配置で行う) |
| P2-3 | 覆えない場合に「余白は出ない」とは言えない (描画の `Q` が canvas を覆わなければ恒等表示で 2D の余白が残る) | 説明を「歪みによる余白は作らない、覆わなければ 2D と同じ余白が残る」に直し、テストを 2 つに分ける (16.3・16.5) |

#### R12 (同じセッションの継続、対象 `02048979a`)

判定 **「実装開始可」**。範囲を限る方針は妥当。範囲の外の恒等表示はクリックの分類に問題を作らない (分類は pose の有無で決まる)、との評価。
数値の範囲 (縦横比 32・座標 `2²¹`) は証明済みとは認めないが、範囲を狭めて確定する裁量が書かれているので着手を止めない、とのこと。文の修正の指摘 2 件を反映した:

| 指摘 | 内容 | 対応 |
| --- | --- | --- |
| P2-1 | 幾何の丸めの上限が約 3.414 なので、「`f32` の誤差 1 画素以下」では 4 を超える。範囲内でも倍率の再構成で 1.59 画素の差の例がある | 範囲の値を「候補」とし、確定の条件を「丸めと `f32` を合わせた各辺の法線方向の総誤差が 4 以下」にした (16.3) |
| P3-2 | 範囲を限った後も、無条件に「覆えば覆う」と書いた文と試験が残っていた。削除した `δ_f32` への言及が残っていた | 「確定した保証範囲の内では」を付け、`δ_f32` の言及を消した (16.3・16.5) |

### 16.8 独立実装レビュー

#### I3 (2026-11-27、GPT-6.1 Sol / xhigh / read-only、実装者とは別の新しいセッション `01a21b19-4730-7f82-a91d-6d894de823b8`、対象 `96d8817cf..df9bac4b1`)

判定は **ACCEPT** で、P1・P2 の指摘はない。以下はコード上の評価で、実行時の挙動は観測していない。

- **描いた素材の記録**
  - 記録は `publish_single` で、描いたページと同時に公開される。
  - `begin`・`clear`・`push*` は記録を消す。同じページの occurrence を束ね直すときは記録を保つ。
  - 追った経路: 単ページ・PlaneView・見開き・連続・page_turn・book mode・holdover・PDF の再ラスタ待ち・Depth。
  - page_turn の演出の分岐は、全部は追っていない。
- **予測と実適用**
  - 同じ `PlaneCoverInput` を使い、遅延適用でも segment の再分割でも取り直さない。
  - キーも同じ producer を通る。
  - 記録の無い frame では、量を捨てる。
- **数式**
  - `plane_fill_coefficient`・`g*`・`θ_cap`・`z_cover`・pan の補正は、設計と合っている。
  - 最小の重なりの補正は、切り取る前の範囲で、遷移の最後に 1 回だけかける。
- **テスト**
  - §16.5 の主な項目がある。
  - 精度のテストの作り直しも妥当。
  - 縦横比 32・座標 `2²¹` は候補のままで、全域を保証したとは扱わない。
- **detached**
  - detached 固有の述語・viewport の経路は変えていない。
  - 既存の layout の所有の境界に記録を載せる、構造的な変更である。症状パッチではないことに合意した。
  - 設計担当 (ClaudeCode) も同じ判断。
- **UI スレッド**
  - 固定個数の計算で、`perf::event` もある。
  - 実時間の性能は観測していない。

テストの結果 (設計担当が共通の実行口で実行、`df9bac4b1`):
- 関連 filter `test(/plane_view|fisheye|displayed_image_transform/)` で 130 件すべて成功。
- 最初の実行では 24 件が失敗した。原因と直しは §11 にある。
- 広い filter `test(/ui_fullscreen|in_page_scroll|page_turn|depth|ui_erase/)` で 2325 件すべて成功。
- 実機の確認は、利用者の build-dev で行う。

## 17. 改訂 2 — 通常のズームで歪みを固定する

### 17.1 経緯と決定 (2026-11-27、利用者が設計担当のセッションで直接)

利用者が改訂 1 を build-dev で試し、「以前より自然な動きには見えるのですが、代わりに中央部分のかなり極端なズームになるケースが多くなった」と報告した。
要望は、Ctrl+Shift+ホイールの動きはそのままに、魚眼に入った後の Ctrl+ホイールでは**歪みを残したまま縮小**する (段 1 のような、周囲が欠ける形) こと。
設計担当が選択肢を出し、利用者が次を選んだ。

- **歪みの残し方**: 通常のズームをした時点の、画面上の歪みを固定し、倍率だけを変える。
  - 段差は出ない。
  - Ctrl+Shift+ホイール下では、固定した歪みが段階的に弱まる。
  - Ctrl+Shift+ホイール上で、改訂 1 の「余白なし」の動きに戻る。
  - 選ばなかった案は「段 1 と同じ、強さだけで決まる歪みに戻す」。こちらは最初の 1 ノッチで歪みが急に強くなり得る。
- **対象**: 通常のズーム操作すべて (入口は 17.3 で列挙する)。
  - 固定していないときの左ドラッグは今のままで、端が画面に入ると歪みが弱まる。
  - 選ばなかった案は「ズームとドラッグの移動も固定する」と「Ctrl+ホイールだけ」。
- **固定した後の左ドラッグ** (R13 P1-1 を受けて、2026-11-27 に利用者へ確かめた): **固定した歪みのまま**動かす。余白も残る。
  - 固定を解くのは Ctrl+Shift+ホイール上 (寄る操作) だけ。
  - 選ばなかった案は「動かしたら固定を解く」。縮小して余白が見えているときは、動かし始めた瞬間に歪みがほぼ消える。

### 17.2 状態: `k_fill` の出どころを 2 通りにする

16.2 の `k = s × k_fill` はそのまま使う。変えるのは `k_fill` の出どころだけで、次の 2 通りにする。

```text
PlaneFillMode = Auto            // 16.2 のとおり、その pass の描画から求める (改訂 1)
              | Locked(k_lock)  // 固定した値 (0 ≤ k_lock)。θ_cap だけは毎 pass かける
```

- 置き場所は `PlaneViewState` (`tilt` と同じ並び) で、`PlaneViewPose` には入れない。
  - 理由: `k_fill` は描画の `k` にしか効かない。倍率・pan・strength・クリックの分類・入口と終了には効かない。
  - 16.3 の遷移 (覆う判定・`z_cover`・pan・最小の重なり) は `k` を使わない。そのため、予測の pose と実適用の pose の一致の規則は今のまま使える (R13 で同意)。
  - 遷移の純粋な関数は `PlaneFillMode` を受け取らない。
- 入ったときは `Auto`。PlaneView が終われば、状態ごと消える。
- detached: `fs_exclusive_view` 全体が、すでに viewer context の退避・引き継ぎの対象である。新しい swap のフィールドは足さない (R13 で同意)。
- **`Locked` の描画**: `k_fill = min(k_lock, k_cap(c))`、`k = s × k_fill`。
  - `k_cap(c) = g(θ_cap)·R/ρ_far` は、16.2 の上限と同じ式で、毎 pass その pass の canvas と `c` から求める。
    これで、レンズの中心の移動や resize の後も、canvas の中に定義域の外は出ない (傾け 0 の段では R13 で確認)。
  - 16 個の不等式 (余白なしの制限) は、`Locked` では使わない。余白を許す。
  - `Locked(0)` (2D と同じ見た目の時点で固定した場合) も許す。クリックの分類は pose で決まるので整合する (R13)。
- 書き出し・キャプチャ・コピーは、今どおり snapshot の uniform の `k` を使う (変更なし)。

### 17.3 いつ固定し、いつ戻すか

**固定する (`Auto` → `Locked`)**

- 条件: PlaneView 中に、通常のズームの入力が倍率を実際に変えたとき。
  - 倍率が変わらない入力 (`ZOOM_MAX`・`ZOOM_MIN` で止まった入力など) では固定しない。
  - すでに `Locked` なら、値を変えない。
- `k_lock` は、**利用者が見ている絵の `k_fill`** とする。これは、今の PlaneView の最後の描画の snapshot の値である。
  - snapshot に、描画が使った `k_fill` (上限を含む) を `k` と並べて記録する。値を割り算で戻さない (strength がその間に変わり得るため)。
  - snapshot が無いときは `Auto` のままにする。この表示単位を PlaneView でまだ描いていない、素材が変わった直後のまれな場合で、割り切る。
- **値の候補と、その失効** (R13 P1-2・P2-4):
  - 描画の開始時に `last_drawn` は消える (ui_fullscreen.rs:47279)。そこで、ホイールの予測の前に、今の PlaneView の snapshot から `k_fill` を 1 回だけ読み、
    候補として `FsWheelFrameProposal` に写す。
    - 候補には context・anchor を付ける。既存の出力の要求と同じく、これらを照合する (app/plane_view.rs:375)。
  - 遅延適用は、この候補を使う。実適用の時点で snapshot を読み直さない。
  - 候補は今の PlaneView の描画記録に属する。入力列の処理の途中で PlaneView が終わったり、所有者が変わったりしたら、その pass の候補を捨てる。
    - 例: 離れて終わる → 同じ表示単位で入り直す → 通常のズーム。この場合、入り直した PlaneView には snapshot が無いので、`Auto` のまま。
    - 永続の世代のフィールドは足さない。
  - 即時に適用する入口 (キー・中ボタン・スクラブ・ピンチ・ゲームパッド) は、描画の前に処理されるので、その時点の snapshot を読む。
    読めなければ `Auto` のまま。
- **描画の順は今のまま** (R13 P1-2。描画用の予測の状態は作らない):
  - 即時の適用では、更新した状態で、同じ pass に描く。
  - 遅延適用では、同じ pass は前の状態で描く。そのあと残った入力を順に実適用し、次の pass で新しい状態を描く。
    同じ pass の描画は、倍率も `PlaneFillMode` も前の状態のままなので、見た目は一致する。
- **対象の入口** (実装担当がコードで確かめ、§11 に入口ごとに書く):
  - Ctrl+ホイール。
  - 設定によって通常のホイールがズームになる場合 (`SettingZoom`。ui_fullscreen.rs:8947)。
  - 中ボタンのドラッグのズーム、Ctrl+Space のスクラブ。
  - `FsZoomIn/Out` (`apply_fs_key_zoom_step`)。
  - 2 本指のピンチ。
  - ゲームパッドのズーム。
  - PlaneView 中に休止している入口 (ナビゲータなど) は対象外。
  - 1 つの関数ではまとめられない (R13 P3-5)。各入口で倍率を書き込む境界から、共通の 1 つの固定の処理を呼ぶ。
    静的な `apply_wheel_zoom` には固定の処理を入れない (Ctrl+Shift の遷移も呼ぶため。plane_view.rs:551)。

**戻す (`Locked` → `Auto`)**

- 条件: Ctrl+Shift の寄る入力 (`notches > 0` の segment と `FsFisheyeIn`) が**受理された**とき (R13 P2-3)。
  - 倍率・pose・pan が変わらない場合も戻す。例: strength 1・`ZOOM_MAX`・すでに覆っている状態。
  - 無視・拒否された入力 (描画の記録が無い frame・入口条件で拒否した入力) では戻さない。
  - そのため、遷移の適用の結果は「受理した」と「見た目の幾何が変わった」を区別して返す。今の `changes_view` だけで早く戻る経路 (ui_fullscreen.rs:43500) では区別できない。
- 戻したあとは、16.3 のとおり、覆う配置へ揃えてからレンズの中心の固定で拡大する。
- 寄る入力と通常のズームが同じ frame にあるときは、処理の順で後に来たほうが決める。処理の順は、キーの段 → ホイールの段、ホイールの段は segment の順。
  上の描画の順の規則で、予測・描画・実適用は一致する。

**`Locked` のままにするもの**

- 離れる segment (`notches < 0`): strength が下がるので、`k = s × k_lock` で歪みが段階的に弱まる。
  - 倍率は 16.3 のとおり。
  - strength が 0 になれば、今どおり終わる。
- 左ドラッグ (17.1 の決定)・Shift+左ドラッグ・自由回転・窓の大きさの変更: `PlaneFillMode` を変えない。
  - `Auto` の間に左ドラッグで端を画面に入れると、今どおり歪みが弱まる。
  - `Locked` の間の左ドラッグは、固定した歪みのまま余白が出る。

### 17.4 テスト (§16.5 に足す)

**純粋な関数**

- `Locked` の `k_fill = min(k_lock, k_cap(c))`。
- `k_cap` は 16.2 の上限と同じ値になる。
- `Locked` では、canvas が `Q` の外に出ても `k` が下がらない。
- 3 種のレンズそれぞれで、レンズの中心の移動・resize の後、canvas の隅の入射角が `θ_cap` 以下。

**状態遷移** (実際の viewer の pass で行う)

- Ctrl+Shift で入って 1 ノッチ → Ctrl+ホイールで縮小。
  - 即時の場合: 縮小の pass の描画の `k_fill` が、直前の pass の `k_fill` と等しい。
  - 遅延適用の場合: 縮小の pass は前の状態で描く。次の pass で、新しい倍率と固定した `k_fill` で描く。
  - 縮小の後、余白が見えている frame でも `k > 0`。
- 対象の入口それぞれで、倍率が変わると `Locked` になる。倍率が変わらないと `Auto` のまま。`SettingZoom` を含む。
- `Locked` 中の 2 回目の通常のズームでは、値を変えない。
- `Locked` 中の Ctrl+Shift 下: `Locked` のまま、`k` が `s` に比例して下がる。strength が 0 で終わる。
- `Locked` 中の Ctrl+Shift 上: `Auto` に戻り、16.3 の覆う配置になる。`FsFisheyeIn` も同じ。
  - 幾何が変わらない受理 (strength 1・`ZOOM_MAX`・覆っている状態) で、キーでもホイールでも `Auto` に戻る。
  - 記録が無くて量を捨てた frame では戻らない。
- `Auto` 中の左ドラッグでは `Auto` のまま (端が入ると `k_fill = 0`)。
- `Locked` 中の左ドラッグ・Shift+左ドラッグ・自由回転・resize では `Locked` のまま。
- `Locked(0)`: 解除 (Ctrl+Shift 上) と終了 (strength 0・ダブルクリック) が今どおり動く。
- snapshot が無い frame の通常のズームでは `Auto` のまま。
- 同じ pass で「離れて終わる → 入り直す → 通常のズーム」の場合、候補を捨てて `Auto` のまま。
- 同じ frame で「Ctrl+Shift 上 → Ctrl+ホイール」と「Ctrl+ホイール → Ctrl+Shift 上」の場合、後の操作が決める。予測・描画・実適用で一致する。
- detached: 2 つの context で `Auto` と `Locked` が混ざらない。F11・scoped mount で保たれる。

**その他**

- 書き出し・コピー: `Locked` の画面の `k` が、snapshot と出力に入る (今の照合のテストに 1 例足す)。
- 予測と実適用の pose が一致する既存のテストは、そのまま通る (pose は変えない)。

### 17.5 文書

- マニュアル `htdocs/mimageviewer/manual/fullscreen.html`:
  - 魚眼中の通常のズームでは、歪みを保ったまま倍率だけが変わる。縮小すると周囲に余白が出る。
  - そのあとは、ドラッグで動かしても歪みはそのまま。
  - Ctrl+Shift+ホイール上で、余白の出ない動きに戻る。
- `docs/display-pipeline.md`・`docs/spec.md`: `k_fill` の出どころ (`Auto` / `Locked`) を足す。
- §13 の CHANGELOG の下書きを合わせる。

### 17.6 簡素化の検討 (CLAUDE.md「状態の組み合わせを減らす」)

- 新しい状態は `PlaneFillMode` の 1 つだけ。置き場所は描画の側 (`PlaneViewState`) で、pose・遷移の関数・クリックの分類には入れない。
  そのため、予測と実適用の一致の規則 (R7〜R10) を作り直さなくてよい。
- 描画用の予測の状態は作らない。今の描画の順 (即時は同じ pass、遅延は次の pass) に乗せる (R13 の勧め)。
- 「戻す」入口は、Ctrl+Shift の寄る操作の 1 つだけにした。ほかの入力 (ドラッグ・回転・窓の大きさ) では状態を変えない。
  利用者の決定 (17.1) とも一致する。
- 固定する値は、画面に出ている snapshot の値をそのまま使う。現在の配置から計算し直す案は採らなかった。
  計算し直すと、量子化した配置と連続の配置の差の扱いが増えるため。
- 段 1 の `α_max` (強さだけで決まる歪み) は復活させない。利用者が選ばなかった案に当たるため。

### 17.7 独立設計レビュー

#### R13 (2026-11-27、R1〜R12 と同じレビューのセッションの継続、GPT-6.1 Sol / xhigh / read-only、対象 `269777a51`)

判定は **「要修正」**。`PlaneFillMode` を pose から分けること、detached の扱い (構造的な状態の追加で、症状パッチではない) には同意。

| 指摘 | 内容 | 対応 |
| --- | --- | --- |
| P1-1 | 「左ドラッグは今のまま」と「`Locked` 中のドラッグでも固定のまま」が食い違う | 利用者に確かめ、「固定した歪みのまま」に決まった (17.1) |
| P1-2 | 「同じ pass の描画が予測の状態を使う」は、今のコードでは成り立たない。描画の開始時に `last_drawn` が消える | 描画の順は今のまま (即時は同じ pass、遅延は次の pass) にした。候補は予測の前に写す (17.3) |
| P2-3 | 幾何の変わらない寄る入力でも、`Auto` へ戻す必要がある | 「受理」で戻し、無視・拒否では戻さない。適用の結果で受理を区別する (17.3・17.4) |
| P2-4 | 同じ pass の中で終了・再入場したときの候補の扱いが未定義 | 候補は今の PlaneView の描画記録に属し、終了・所有者の交代で捨てる (17.3・17.4) |
| P3-5 | `SettingZoom` の通常のホイールのズームが一覧に無い。共通の関数は無い。`apply_wheel_zoom` には入れない | 一覧に足し、各入口の書き込みの境界から共通の処理を呼ぶ形にした (17.3) |

テストの追加の勧め (3 種のレンズでの中心の移動・resize、`Locked(0)`、detached の 2 つの context・F11・scoped mount) は、17.4 に足した。

#### R14 (同じセッションの継続、対象 `db826ae4a`)

判定は **「実装開始可」**。新しい指摘はなく、R13 の 5 件はすべて設計上 resolved になった。
描画用の予測の状態を作らない形は、前面の UI が入力を消費・再分割する経路とも合う、との評価。
detached は、既存の `fs_exclusive_view` 全体の引き継ぎに乗る構造的な追加で、症状パッチではないという同意を保った。

### 17.8 独立実装レビュー

#### I4 (2026-11-27、I3 と同じレビューのセッションの継続、GPT-6.1 Sol / xhigh / read-only、対象 `6f9785302..39fdde44f`)

判定は **ACCEPT** で、P1・P2・P3 の指摘はない。以下はコード上の評価で、実行時の挙動は観測していない。

- **固定の入口**
  - 7 つの入口は、どれも倍率を書き込む境界から共通の固定の処理を呼ぶ。
  - 倍率が同じならば固定しない。すでに `Locked` なら値を保つ。
  - `apply_wheel_zoom` は変えていない。PlaneView 中に生きている、ほかの通常のズームの漏れも見つからなかった。
- **候補**
  - 予測の前に proposal へ写し、遅延適用で読み直さない。
  - context・anchor を照合し、入力列の途中で終われば捨てる。
  - 描画の順は変えていない。
- **`Auto` へ戻す**
  - 受理の判定は `changes_view` の早期 return より前にあり、幾何の変わらない寄る入力でも戻る。無視・拒否では戻らない。
  - `changes_view`・予測・クリックの分類の意味は保たれている。
- **描画**
  - `Locked` は `min(k_lock, k_cap)` で、上限の計算は `Auto` と共有している。
  - snapshot は `k_fill` と `k` を別に持つ。
  - `PlaneFillMode` は pose・遷移の関数に入っていない。
- **テスト**
  - §17.4 の主な項目がある。
  - 直した遅延のテストは、前の状態の描画 → 候補による固定 → 次の pass まで検査している。
- **detached**: 既存の `fs_exclusive_view` の引き継ぎに乗る構造的な追加で、症状パッチではないことに同意した。設計担当 (ClaudeCode) も同じ判断。

テストの結果 (設計担当が共通の実行口で実行、`39fdde44f`):
- 関連と fullscreen 全般の filter で、2586 件すべて成功した。
- 最初の実行では、テストの組み方の誤りで 1 件が失敗した。


## 18. 段 2 — 写真の板の 3D 傾け (§1.819)

### 18.1 利用者の決定 (2026-11-27、設計担当のセッションで直接)

改訂 2 を実機で試した後、「操作感良さそう」との評価をもらい、段 2 へ進むことになった。設計担当が論点を出し、利用者が次を選んだ。

| 論点 | 決定 | 選ばなかった案 |
| --- | --- | --- |
| 傾けられる範囲と裏側 | **制限なし、裏も見せる** (裏側では左右または上下が反転した絵) | ±75 度まで・裏は見せない (推奨として出した案)、±89 度まで |
| 回転の中心 | **レンズの中心の下の点** (見ている場所が動かず、まわりが奥へ倒れる) | 画像の中心 |
| 傾けて出る余白 | **許す** (倍率を自動で変えない) | 余白が出ない倍率まで自動で拡大する |
| 遠近感の強さ | **普通のカメラ程度** (魚眼の歪みが 0 のとき、対角の画角が約 50 度。定数で調整できる) | 弱め (約 25 度)、強め (約 90 度) |

操作は §9 のとおり **Ctrl+Shift+左ドラッグ**。左右のドラッグで画面の縦軸まわり、上下で画面の横軸まわりに回る。
対象は段 1 と同じく、1 ページ表示の静止画だけ (見開きは含めない。設計担当の既定)。

### 18.2 見え方の式 (§5.1 の 1〜5 を置き換える)

§5.1 の最初の案は、魚眼のカメラから出た光線と傾けた板の交点を取る形だった。この形には 2 つの問題がある。
- 魚眼の歪みが 0 (`k → 0`) のとき、遠近感も消える。
- 遠近感のために最小の画角を足すと、傾き 0 でも放射状の歪みが出る。つまり段差が出る。

そこで、次の 2 段の合成にする。

1. **魚眼 (画面の上の放射状の写像)**: 改訂 1・2 の傾き 0 の式そのまま。
   - `v = p − c` (物理画素) とし、`v₁ = v · W(|v|/R)` を求める。`k = s × k_fill` (`k_fill` は改訂 2 の `Auto` / `Locked`)。
   - `k = 0` なら `v₁ = v`。
   - 定義域と `θ_cap` も今どおり。
2. **遠近 (傾けた板をピンホールのカメラで見る)**:
   - 画角の定数は `φ_p = 50°` (対角)。`ρ_d` を canvas の対角の半分 (物理画素) とし、焦点距離を `f_p = ρ_d / tan(φ_p / 2)` とする。
   - 光線 `d = (v₁.x, v₁.y, f_p)`。板は点 `P₀ = (0, 0, f_p)` を通り、法線 `n = Rot · ẑ` の平面。
   - `den = n · d`。`|den| ≤ ε·|d|` (板と平行) なら無効。
   - `t = (n · P₀) / den`。`t ≤ 0` (光線が板に届かない) なら無効。
   - 板の局所座標 `L = (Rotᵀ · (t·d − P₀)).xy`、`q = c + L`。
   - **無効な点の扱い** (§5.1 の規則を引き継ぐ。R15 P2-5):
     - 除算の前に、有限な代用値を入れる。`den` は符号を保って大きさだけ `ε·|d|` へ寄せ、`t` は代用値 1 を使う。
     - 微分を取った後で、無効の印によって透明にする。
     - 裏側の負の `den` は有効なので、今の実装の `max(den, ε)` (plane_view_wgpu.rs:55) のように潰さない。
3. `q` から先は §5.1 の 6〜9 のまま (2D の逆写像 → source UV → 素材の UV → 色・有効性・下地)。

この合成の性質 (テストで固定する):

- **傾き 0 (`Rot = I`) では、改訂 1・2 の結果と完全に一致する**。
  - `den = f_p`、`t = 1`、`L = v₁` になるので、`q = c + v₁`。
  - 余白なしの保証・`k_fill`・覆う配置・固定は、すべて傾き 0 の 2D の枠で決まる。傾けても変わらない。
- **魚眼の歪みが 0 (`k = 0`) では、純粋な遠近だけになる**。
  - 遠近は、傾き 0 で恒等になる。したがって、`s` と傾きのどちらが 0 に近づいても段差は出ない。
- **裏側**:
  - 板の向こう側から見ている (`n_z < 0`) 場合も、`t > 0` なら交点がある。
  - `L` は反転した座標になる。例: 縦軸まわりに 180 度なら `L = (−v₁.x, v₁.y)`。
  - 裏を見せる決定 (18.1) は、無効にする判定を足さないことで満たす。
- **回転の中心**: 画面上の `c` に写る板の点 (`P₀`) を軸にする。`P₀` は板の上にあり、`L = 0` (= 2D の `c`) に写る。
  そのため、有効な交点がある姿勢 (`|n_z| > ε`) では、傾けても、レンズの中心の下の絵は動かない (18.1 の決定)。
- **真横の姿勢** (R15 P2-5): `n_z = 0` (例: 縦軸まわりに 90 度・270 度) では、`n · P₀ = 0` となり、逆写像に面積が無い。
  - 中心の光線は `den = 0`、それ以外の光線は `t = 0` になる。
  - そのため、canvas 全体が下地になる (板が線になって見えなくなる)。
  - その前後の角度では、細い帯として見える。これは仕様とする。
- **余白**: 板の外・板に届かない光線は §5.1 の 9 の透明 (下地) になる。倍率は変えない (18.1 の決定)。
- **素材の解像度の需要は、今のまま** (R15 P2-6、設計担当の判断。既知の制限):
  - PDF の再ラスタの要求寸法と表示の素材は、今どおり 2D の倍率から決める。
  - 遠近は局所的に拡大する。例: `k = 0`・縦軸まわりに 45 度で、板の座標が `X/f_p = 0.4` の点では約 1.37 倍になる。
    そのため、傾けたときの手前の側は、2D の同じ倍率よりぼやけることがある。
  - 局所の拡大を需要へ反映する仕組み (canvas と可視の範囲の頂点で最大の拡大率を求め、要求寸法に掛ける) は、段 2 では作らない。理由は 2 つある。
    - 手前の側の拡大は、傾けるほど際限なく大きくなる。そのため、既存の要求の上限にすぐ当たる。
    - 可視の範囲 (canvas と、傾けた `Q` の像の交わり) を求める計算が増える。
  - 実機で目立つなら、後で需要へ反映する (§13 の候補として残す)。

`Locked` / `Auto` の判定と `k_fill` の計算は、2D の (傾けない) 枠のままにする。傾けている間も `Auto` は 16 の不等式で傾き 0 の覆いを求め、`Locked` は固定の値を使う。
- 傾ける操作そのものは、`PlaneFillMode` を変えない。傾き 0 の枠の幾何が変わらないので、傾けている間に魚眼の歪みは変わらない。
- 改訂 2 の固定の規則 (通常のズームで固定・Ctrl+Shift の寄る操作で戻す・左ドラッグでは変えない) は、傾けている間もそのまま。
- 利用者へ選択肢を出したとき、「傾けている間は改訂 2 の固定と同じ扱い」と説明した。この式では、傾けても `k_fill` は傾き 0 の枠でしか決まらない。そのため、傾ける操作が歪みを変えないという意味で、説明と一致する。

### 18.3 傾きの状態と操作

- **状態**: `PlaneViewState::tilt` を、`{ yaw, pitch }` の角度から **回転の四元数** (正規化した `[f32; 4]`。既定は恒等) に変える。
  - 表し方 (R15 P2-3):
    - 成分の順は `[w, x, y, z]`。
    - `Default` は手で書いた恒等 `[1, 0, 0, 0]` にする (derive の 0 埋めにしない)。
    - 板の局所座標の点 `(x, y, 0)` は、カメラの座標の `P₀ + Rot · (x, y, 0)` に置かれる。
  - **恒等の判定**: 共通の 1 つの関数 `is_identity` を使う。正規化した後の**ベクトルの成分**で、`‖(x, y, z)‖ ≤ sin(θ_id / 2)` (`θ_id = 0.01°`) を条件とする (R16 P1-1)。
    - `w` で判定 (`|w| ≥ cos(θ_id/2)`) しない。`cos(0.005°)` は f32 では 1 に丸められ、0.02° の回転も恒等になってしまうため。
    - 符号に依存しない (`q` と `−q` は同じ回転で、ベクトルの成分の大きさは等しい)。
    - 予測・実適用・終わりの判定は、すべてこの関数を使う。
  - **`Default` への寄せは、操作の終わりだけで行う** (R16 P1-2):
    - 寄せるのは、傾けのドラッグを離したときと、`FsPlaneTiltReset` のとき。そのとき恒等と判定したら、ちょうど `Default` にする。
    - ドラッグの途中では寄せない。小さな移動を毎回 0 へ戻すと、たとえば 0.004° を 25 回動かしても回らない。
      一括の 0.1° と結果が食い違い、分割の一致が壊れる。
    - これで、360 度回した後や往復した後の残差も、離したときに消える。
  - 範囲に制限が無く、画面の軸まわりに重ねて回すので、角度の 2 つ組では裏返った後の操作の向きが崩れる (ジンバルの問題)。
  - 置き場所は今どおり `PlaneViewState` (pose ではない)。detached は、既存の `fs_exclusive_view` の引き継ぎに乗る (新しい swap のフィールドは足さない)。
  - 段 1 の `PlaneTilt` は、まだ値 0 しか持たない未リリースの型なので、形を変えてよい (移行は不要)。
  - shader へは回転行列 (3×3) として渡す。
    - uniform の末尾に、vec4 を 3 つ足す。今の 13 個の vec4 の index (出力の変換の固定の index を含む。plane_view_wgpu.rs:133・269) は変えない。
    - 画面と書き出しは同じ Params を使うので、worker・readback の構造は変えない。Depth の shader は別で、変えない。
- **ドラッグ** (画面の軸まわりに重ねて回す。トラックボールの形。R15 P2-4):
  - 座標は、x が右、y が下、z が画面の奥 (§5.1 と同じ)。
  - 1 frame のドラッグ量 `(dx, dy)` (物理画素) から、**回転ベクトル** `ω = κ · (dy, −dx, 0)` を作る。`κ = (π/2) / R` で、`R` 動かすと 90 度になる。
  - `ω` から、**1 つの**四元数 `ΔR` を作る (軸 `ω/|ω|`、角度 `|ω|`)。`|ω| = 0` なら恒等で、状態を変えない。
  - `Rot ← ΔR · Rot` (画面の軸で左から掛ける) とし、毎回、正規化する (恒等への寄せは、上のとおり操作の終わりだけ)。
  - 直線のドラッグは、軸が一定である。同じ軸まわりの回転は交換するので、一括でも frame ごとに分けても同じ姿勢になる。
    途中で向きを変えたドラッグは、たどった道筋に依る (トラックボールの性質で、仕様とする)。
  - 向き: 板の手前の面が、カーソルと同じ向きに動く。正本は、上の回転ベクトルと左から掛ける式とする。
    **恒等の姿勢から**の例は次のとおり (R16 P3-4)。
    - 右へドラッグする (`dx > 0`) と、板の右の端 `(Rot·x̂).z` が増える (奥へ行く)。左の端は手前に来る。
    - 下へドラッグする (`dy > 0`) と、板の下の端 `(Rot·ŷ).z` が増える。
    - ほかの姿勢では、この例は成り立たないことがある。例: すでに縦軸まわりに 180 度なら、同じ操作でその z は減る。
      これは、画面の軸で左から掛ける仕様どおりの動き。
    - 今の shader の `Ry` の符号 (plane_view_wgpu.rs:53) は引き継がない。この定義に合わせて書き直す。
  - 係数 `κ` は定数で、実機で詰める。
- **所有** (§3.3 の Shift+左ドラッグと同じ形): 押した時点で所有を決める。
  - 次の 3 つを満たせば、そのドラッグは傾けが所有する。
    - Ctrl+Shift あり (OS から読む既存の修飾の helper)
    - 対象の表示 (PlaneView の入口条件を満たす 1 ページの静止画)
    - 前面の UI が押下を持っていない
  - 離すまで、途中の修飾の変化でパン・回転・レンズの移動へ移らない。
  - 対象でない表示 (見開き・動画など) では、今の配送と可否をそのまま保つ (R15 P3-7)。
    - 自由回転が効く表示では自由回転になる。
    - 見開きのように自由回転を無効にしている表示 (ui_fullscreen.rs:43226) では、今どおり何もしない。
  - 所有の型は、§3.3 のレンズの移動と同じ 1 つの値に、`Tilt` を足す (Option を並べない)。
  - **入る前の所有と、入るときの公開の順** (R15 P2-2):
    - `Tilt` は、PlaneView が無くても所有できる。持つものは、anchor (押した表示単位)・押した位置・前の frame の位置。
      今のレンズの処理は、PlaneView が無ければ所有を解く (app/plane_view.rs:66)。`Tilt` はこの規則から外す。
    - **最初の 0 でない回転ベクトル** (`|ω| > 0`) の frame で、PlaneView を作り、その回転を傾きとして持つ (R16 P1-2)。
      - 恒等の判定より小さい回転でも入る。入る前に回転を貯める仕組みは持たない。
      - 入った後は、上の規則どおり既存の傾きに積み重ねる。操作の途中は、小さな姿勢や恒等の姿勢も許す。
      - 離したときに、寄せと終わりを決める (strength 0 かつ恒等なら終わる)。
      - そのとき、所有は `Tilt` のまま保つ。
      - 今の `set_fs_exclusive_view` は、所有を解いてから新しい状態を公開する (depth_view.rs:924)。そのため、入る操作自身の所有を解かない入口を使う。
      - 入口は、所有を保つ専用の 1 つの関数にまとめる。呼ぶのは、傾けによる入場だけ。
    - 下流の入力の抑止 (今はレンズの移動だけ。ui_fullscreen.rs:20478) を、`Lens` と `Tilt` の共通の判定にする。
      離した pass の「使用済み」の記録と、終わりの処理も共通にする。
    - 同じ pass で「離す → 押す」が来たら、別の操作として扱う。既存のスクラブの扱い (ui_fullscreen.rs:41943) と同じ。
    - anchor が今の表示単位と違えば (ページが変わったなど)、所有を解く。
  - 終わり方は §3.3 と同じ: 離したとき、PlaneView が終わったとき、フォーカスを失ったとき、modal が開いたとき、ページが変わったとき。傾きはその時点のまま残す。
- **入る**: PlaneView でないときに、傾けのドラッグが最初に 0 でない回転を作ったら、PlaneView に入る (下の「入る前の所有」)。
  - 中身は `strength = 0`、レンズの中心は押した位置、`PlaneFillMode::Auto`、傾きはその回転。
  - 入口の判定は、段 1 の入口条件と同じ関数を使う。
  - 段 1 の「strength 0 の PlaneView は作らない」を、「strength 0 かつ傾きが恒等の PlaneView は作らない」に改める。
- **終わる**: strength が 0 で、かつ傾きが恒等のとき (§2 の段 1 の規則の拡張)。
  - **傾けのドラッグを所有している間は、終わらない** (R15 P1-1)。
    - 例: strength 0 で入る → 右へ傾ける → 元の姿勢まで戻す。この操作で、ボタンを押したまま反対へ動かし続けられる。
    - 離したときに、strength 0 かつ恒等なら、そこで終わる。
    - 「何もしていない PlaneView を残さない」は、操作が終わった後に成り立つ条件とする。
  - そのため、終わりを決める共通の純粋な遷移 (今の終了の処理は plane_view.rs:614) に、「strength 0 でも保つか」を渡す。
    この値は、同じ所有者 (`Tilt` を所有しているか) から求め、予測と実適用で同じ値を使う。
  - `FsPlaneTiltReset`・ダブルクリック・ページの移動は、所有の有無によらず、すぐ終わらせる。
    reset で strength が 0 なら終わり、正なら傾きだけ戻す。
  - 傾きがあれば、Ctrl+Shift+ホイール下で strength が 0 になっても終わらない (§3.2 の「その segment の残りは使わない」は今どおり)。
  - ダブルクリック・フィットの切り替え・ページの移動は、今どおり PlaneView ごと終わる (傾きも消える)。
- **傾きを戻す**: `KeyAction::FsPlaneTiltReset` (`KeyContext::FsImage`、Press、**既定キーなし**。段 1 の魚眼のキーと同じ扱い) を足す。
  - 傾きを恒等に戻す。strength が 0 なら、そのまま終わる。
  - keymap の決まり (`ini_name()` など一式・`docs/keymap.ini.default`・`docs/keymap-spec.md`) を揃える。
  - Ctrl+Shift+左ドラッグは、固定入力の表に足す。
- **既知の挙動** (段 1 の §3.2 の既知の挙動と同じく、段 2 では直さない):
  - 傾けている間の左ドラッグ (パン) と通常のズームの基準点は、傾けない 2D の枠で計算する。
    そのため、画面上の動きは傾きの分だけずれる。裏側では、パンの向きが反転する。
  - 傾けている間に Shift+左ドラッグでレンズの中心を動かすと、回転の中心も一緒に動く。その分、板の見え方が連続して変わる。
- **予測と実適用**: ホイールの純粋な遷移・クリックの分類は、pose の有無だけを見る (傾きは見ない)。
  - 傾きは描画と、入口・終わりの判定にだけ効く。
  - 入口・終わりの判定に傾きを足すので、予測が「終わる」と判断する条件に傾きを入れる。
    予測の入力に、次の 2 つを写す (pose には入れない)。
    - その時点の傾きが恒等かどうか (`is_identity`)
    - 傾けのドラッグを所有しているか (上の「strength 0 でも保つか」)

### 18.4 書き出し・キャプチャ・コピー

今どおり、最後に描いた snapshot の uniform (傾きの回転行列を含む) で描く。裏側も、見えているとおり (反転) に出る。

### 18.5 テスト

**純粋な関数・式** (CPU 版と WGSL を同じ入力で比べる)

- 傾き恒等で、改訂 1・2 の式と一致する (全画素、3 種のレンズ、`k = 0` と `k > 0`)。
  代数の一致だけでなく、今の演算順と `k ≈ 0` の恒等の経路を保つことを、比較のテストで確かめる (R15)。
- `k = 0` で、純粋な遠近の写像になる (ホモグラフィの既知の値と比べる)。
- 縦軸まわりに 180 度で、`L = (−v₁.x, v₁.y)`。横軸まわりも同様。
- 縦軸まわりに 90 度で、`den` の判定による無効の帯が出る。
- `t ≤ 0` の光線は無効になる。
- 四元数の積み重ね:
  - 正規化が保たれる。
  - 画面の軸で左から掛ける。順序の違う 2 つの操作の例で、期待した行列になる。
  - ドラッグの係数で `R` 動かすと 90 度回る。
- 回転の中心: 有効な交点がある姿勢では、`c` に写る `q` は `c` のまま。
- 真横 (縦軸・横軸まわりに 90 度・270 度) は、全面が下地になる。その前後の角度では、有限な値の細い帯になる。
- 裏側の負の `den` が有効のまま (潰されない)。無効な点も、微分の前に有限な値になる。
- 恒等の判定:
  - `q` と `−q`、360 度回した後、往復した後、許容誤差の境界の内と外。
  - 寄せた後は、ちょうど `Default` になる。
- 恒等の判定: 0.02° は恒等にならない。0.01° の内と外の境界。f32 で計算する。
- ドラッグの合成:
  - 直線の斜めのドラッグで、一括と分割が一致する (両方向に 90 度を、一括と 45 度ずつ 2 回で比べる例を含む)。
  - 微小な移動 (例: 0.004° を 25 回と、一括の 0.1°) でも一致する。途中で寄せない。
  - 恒等を通る移動で、途中の状態が保たれる。
  - **恒等の姿勢から**、`dx > 0` で右の端が奥へ行き、`dy > 0` で下の端が奥へ行く。縦軸まわり 180 度の姿勢からは、左から掛ける式どおりの向きになる。
  - 移動 0 では状態が変わらない。
- 入る前: 最初の 0 でない移動が恒等の判定より小さくても PlaneView に入り、その回転を持つ。
- 寄せ: 離したとき・reset のときだけ `Default` に寄せる。

**状態遷移** (実際の viewer の pass)

- Ctrl+Shift+左ドラッグで入る: strength 0・レンズの中心が押した位置・`Auto`。
  - 対象でない表示では、今の配送と可否のまま (自由回転が効く表示では自由回転、見開きでは何もしない)。
- 入る前の所有と公開の順:
  - 押した直後 (まだ傾きが変わらない frame) は PlaneView が無く、所有は `Tilt`。
  - 最初の移動で PlaneView ができ、所有は `Tilt` のまま。
  - 下流のパン・回転・クリックが、その frame でも離した frame でも動かない。
- 押した時点で所有を決める: 途中で Shift・Ctrl を離しても、傾けのまま。
- 同じ pass の「離す → 押す」は別の操作になる。フォーカスの喪失・modal・ページの変更で所有が解ける。
- strength 0 で入り、往復して恒等を通っても、押したまま動かし続けられる。離した時点で恒等なら終わる。
- 傾いたまま、Ctrl+Shift+ホイール下で strength が 0 になっても終わらない。`FsPlaneTiltReset` で傾きが恒等に戻り、そこで終わる。
- `FsPlaneTiltReset`:
  - 割り当てを変えても動く。
  - PlaneView でないときは何もしない。
  - strength 0 なら終わる。strength が正なら、傾きだけを戻す。
  - 傾けの操作中に押すと、すぐ戻る。同じ pass のクリック・ホイールとの順序を、予測と実適用で一致させる。
- PDF: 傾けだけの PlaneView で、要求寸法が 2D の倍率のままである。新しい素材を待つ間、前の素材で描き続ける。
- 傾いた状態で、改訂 2 の固定 (通常のズーム)・戻す (寄る操作)・左ドラッグの規則が今どおり働く。傾ける操作では `PlaneFillMode` が変わらない。
- 予測と実適用で、終わりの判定 (傾きの有無を含む) が一致する。
- ダブルクリック・ページの移動で、傾きも消える。
- detached: 2 つの context で傾きが混ざらない。F11・scoped mount で保たれる。

**書き出し**: 傾けた状態・裏側の状態で、画面の snapshot と出力が一致する。

### 18.6 文書

- マニュアル `htdocs/mimageviewer/manual/fullscreen.html`:
  - Ctrl+Shift+左ドラッグで写真を板のように傾けられる (裏側まで回る)。
  - 傾きを戻すキーは、キー設定で割り当てる。
  - 1 ページ表示の静止画だけが対象。それ以外では、Ctrl+Shift+左ドラッグの今の動き (自由回転が効く表示では自由回転、見開きでは何もしない) を保つ (R16 P3-3)。
- `docs/keymap-spec.md`: 固定入力と新しい `KeyAction`。
- `docs/display-pipeline.md`・`docs/spec.md`: 合成の式と状態。
- §13 の CHANGELOG の下書き。
- Ctrl+Shift+左ドラッグが自由回転から変わることは、利用者へ報告済み (§9)。

### 18.7 簡素化の検討 (CLAUDE.md「状態の組み合わせを減らす」)

- 魚眼と遠近を 2 段の合成にした。こうすると、傾き 0 では改訂 1・2 がそのまま成り立つ。
  余白なしの保証・覆う配置・固定の規則を、傾きの有無で場合分けしない。
  §5.1 の最初の案 (魚眼の光線と板の交点) では、傾き 0 で遠近の画角との合成が要り、`k_fill` の式も傾きに依ってしまう。
- 状態は四元数 1 つ。角度の 2 つ組と範囲の制限は持たない (範囲に制限が無いという決定を、そのまま形にした)。
- 余白は許すので、傾けに合わせて倍率を自動で変える仕組みは作らない (18.1 の決定)。
- パン・ズームの基準点を傾きに合わせて補正する仕組みは作らない (既知の挙動として残す)。

### 18.8 独立設計レビュー

#### R15 (2026-11-27、R1〜R14 と同じレビューのセッションの継続、GPT-6.1 Sol / xhigh / read-only、対象 `2a13bafe0`)

判定は **「要修正」**。次の点には同意を得ている。
- 合成の式の骨格 (傾き恒等で改訂 1・2 と一致、`k = 0` で純粋な遠近、180 度で反転)。
- 傾ける操作で `PlaneFillMode` を変えない方針。
- 基準点のずれを既知の挙動として残す簡素化。
- shader の波及の範囲。
- 型の変更に移行が要らないこと。
- detached が既存の `fs_exclusive_view`・`fs_canvas_drag` の引き継ぎに乗る構造的な追加で、症状パッチではないこと。

| 指摘 | 内容 | 対応 |
| --- | --- | --- |
| P1-1 | 傾けだけの往復のドラッグが、恒等を通ったところで終わり、所有が失われる | 所有中は終わらせず、離したときに決める。共通の遷移に「保つか」を渡す (18.3) |
| P2-2 | 入る前の所有と、入るときの公開の順が未定義。今の `set_fs_exclusive_view` は所有を解く | 入る前の `Tilt` の所有、所有を保つ専用の入口、`Lens`・`Tilt` の共通の抑止と使用済みの記録 (18.3) |
| P2-3 | 四元数の恒等の判定が未定義 | 成分の順、手で書いた `Default`、符号に依存しない `is_identity` (0.01°)、`Default` への寄せ (18.3) |
| P2-4 | `ΔR` の合成と符号が未確定で、分割に依存し得る。今の `Ry` の符号と逆 | 回転ベクトルから 1 つの四元数を作る。符号を式で確定した (18.3) |
| P2-5 | 真横で逆写像が退化する。「どの傾きでも中心が写る」は成り立たない | 有効な姿勢に限定し、真横は全面が下地とした。無効な点の代用値、負の `den` を潰さないこと (18.2) |
| P2-6 | 遠近の局所の拡大が、PDF の解像度の需要に反映されない | 設計担当の判断で、需要は今のままとし、既知の制限として書いた (18.2) |
| P3-7 | 「対象外では自由回転」は広すぎる。§0 付近に「shader を書き直さない」が残っている | 「今の配送と可否を保つ」に直した。冒頭に §18.2 の置き換えを書いた |

テストの追加の勧め (reset のキーの各場合、恒等の比較、真横、PDF) は、18.5 に足した。

#### R16 (同じセッションの継続、対象 `142702590`)

判定は **「要修正」**。R15 の P1-1・P2-2・P2-5・P2-6 は resolved。PDF の需要を据え置く判断にも反対しない、とのこと。

| 指摘 | 内容 | 対応 |
| --- | --- | --- |
| P1-1 | `|w| ≥ cos(0.005°)` は f32 で 1 に丸められ、0.02° も恒等になる | ベクトルの成分 `‖xyz‖ ≤ sin(θ_id/2)` で判定する (18.3) |
| P1-2 | 毎回 `Default` へ寄せると、微小な移動の累積と分割の一致が壊れる。入る前の小さな移動も捨てられる | 寄せは、離したときと reset のときだけ。最初の 0 でない回転で入る (18.3) |
| P3-3 | マニュアルの項目に「それ以外では自由回転」が残っている | 今の動きを保つ書き方にした (18.6) |
| P3-4 | 向きの例は、任意の姿勢では成り立たない | 「恒等の姿勢から」に限り、正本は式とした (18.3・18.5) |

#### R17 (同じセッションの継続、対象 `65ee2252d`)

判定は **「実装開始可」**。新しい指摘はなく、R16 の 4 件はすべて設計上 resolved になった。
レビュー担当の f32 の算術モデルの結果 (実機の観測ではない) は次の 2 つ。
- 0.02° は恒等にならない。
- 0.004° を 25 回と、一括の 0.1° の四元数の差は、最大の成分で約 5.24×10⁻¹⁰。

snapshot・出力、detached の構造的な追加、PDF の需要を既知の制限として据え置く判断について、前回の同意を保った。

### 18.9 独立実装レビュー

#### I5 (2026-11-27、I3・I4 と同じレビューのセッションの継続、GPT-6.1 Sol / xhigh / read-only、対象 `ee7421548..c689c8cfb`)

判定は **REQUEST CHANGES** で、指摘は P2 が 1 件。以下はコード上の評価で、実行時の挙動は観測していない。

- **P2**: 入る前の `Tilt` の所有が、魚眼の操作による入場で失われる。
  - 筋道: 押すだけ → Ctrl+Shift+ホイール上で入場 → 通常の `set_fs_exclusive_view` が所有を解く → 移動しても傾かない。
  - 予測で写した `tilt_drag_owned` と、実際の所有も食い違っていた。
  - 対応 (`e4fae13bc`): 公開の境界で、同じ anchor の `Tilt` だけを保つ形にまとめた。回帰テストを足した (ホイールとキーでの入場、anchor が違う場合)。
- そのほかの点では整合している、との評価。
  - 式: 傾き恒等では従来の経路を使う。符号つきの `den`・代用値・微分の後のマスク・真横・uniform の index。
  - 四元数・入力・終わり・キーの規則、傾けで `PlaneFillMode` を変えないこと。
  - テストの訂正 2 件の根拠。
- **detached**: 既存の `fs_exclusive_view`・`fs_canvas_drag` の引き継ぎに乗る構造的な変更で、症状パッチではないことに同意した。設計担当 (ClaudeCode) も同じ判断。

#### I5 の再確認 (同じセッションの継続、対象 `c689c8cfb..e4fae13bc`)

判定は **ACCEPT**。
- 所有を保つのは、入場先が PlaneView で、今の所有が `Tilt`、anchor が完全に一致する場合だけ。
  `Lens`・anchor の違う所有・終わり・別の投影への交代では、保たない。
- 傾け・ホイール・キーの入場は、同じ setter を通る。予測と実際の所有は一致する。
- Panorama の後処理、古い資源の take/drop、Depth の取り消しは保たれている。

テストの結果 (設計担当が共通の実行口で実行、`e4fae13bc`):
- 関連と fullscreen 全般の filter で、2932 件すべて成功した。
- 段 2 の最初の実行では 3 件が失敗した (原因と直しは §11)。
- そのあと、テストの浮動小数のリテラルで 1 回コンパイルエラーになった。
