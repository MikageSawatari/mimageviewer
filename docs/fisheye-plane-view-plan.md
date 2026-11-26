# 魚眼ズームと写真の板の 3D 傾け (§1.917・§1.819) — 設計書

- 対象: backlog §1.917 (静止画のズームに魚眼レンズで近寄った変形を加える、利用者要望 2026-11-27) と
  §1.819 (平面画像を傾けて 3D 回転する表示、5ch >>816、backlog-on-hold)。
- ブランチ / worktree: `fisheye-917` / `G:\home\mimageviewer-fisheye917` (基点 master `f2e332bb8`)。
- 役割: 設計・作業指示・検収 = ClaudeCode Opus 5.5、実装 = Codex GPT-6.1 Sol / high、
  独立レビュー = 実装者とは別の GPT-6.1 Sol / xhigh (AGENTS.md)。
- 状態: 設計 R5、**独立設計レビュー R6 で「実装開始可」** (2026-11-27、§14)。段 1 の実装に着手。

## 0. 利用者の決定 (2026-11-27、設計担当のセッションで直接)

| 論点 | 決定 |
| --- | --- |
| 操作の形 | **寄ると倍率と歪みが一緒に強くなる** (案 A)。1 つの操作で「レンズを持って近寄る」 |
| レンズの中心 | 最初に回したときのカーソル位置で固定 (案 A)。利用者の補足:「端で始めると後から中心をずらせないのでは。修飾キー付きのドラッグで写真だけ / レンズだけを動かせるとよい」→ **Shift+左ドラッグでレンズだけ動かす** を足す (§3.3) |
| 修飾キー | **Ctrl+Shift+ホイール** (案 A) |
| キーの既定 | 「Ctrl+Shift+. / , はなくても良いかもしれません」→ キー操作は `KeyAction` として用意するが **既定キーなし** (§3.4) |
| 書き出し・キャプチャ | 「結果を SNS にあげたいケースがあるとおもうので、最初から含めたい」→ **見えているとおりの絵を保存・コピー・書き出しできる** (§6) |
| §1.819 との関係 | 「まとめて進める」→ 設計は 1 本、実装は 2 段 (段 1 = 魚眼、段 2 = 傾け。§9) |

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
- `strength` と `tilt` が両方 0 になったら PlaneView を終える (段 1 では strength だけ)。「何も変形しない PlaneView」は作らない。
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
  `strength = min(1, strength + notches / 8)`。倍率は同じ `wheel_y` で `apply_wheel_zoom` を通して上げる
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
- 画角: `r = 1` に写る入射角 `α(s) = s × α_max`、`k = g(α(s))`。`α_max` は「地平線 (θ = 90°) の円の半径が
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
| strength 0 の PlaneView | 作らない (「何もしていないのに占有中」を無くす) |
| レンズの移動のドラッグ | 押した時点で所有を固定する。途中の修飾で切り替える組み合わせを作らない |

## 8. まれな失敗

既定の割り切り (ログ + 通知)。写しの失敗は入らない / 終わる + 通知、書き出しの失敗はエラー通知。再試行はしない。
通常の準備待ち (renderer への登録待ち・素材の差し替え) はまれな失敗ではなく、§5.2 の規則で扱う。

## 9. 段 2 — 写真の板の 3D 傾け (§1.819)

- 操作: **Ctrl+Shift+左ドラッグで板を傾ける** (Hamana と同じ組み合わせ)。左右のドラッグで縦軸まわり (yaw)、
  上下で横軸まわり (pitch)。今の Ctrl+Shift+左ドラッグ (自由回転) から変わる (1 ページ表示の静止画だけ。利用者へ報告済み)。
- 状態は段 1 の `PlaneViewState::tilt`。strength 0 でも傾きがあれば PlaneView を続ける。
- shader は段 1 のまま (§5.1 の 3・4 が傾けを扱う)。書き出しも同じ snapshot に傾きが入るだけ。
- 段 2 で決めること: **透視の画角** (段 1 の式では strength 0 で `k → 0` となり透視が消えるので、傾けには strength と別の
  最小の画角が要る。strength と傾けを同時に持つときの画角の合成も含め、`s → 0` で段差を出さない形を段 2 の設計で決める)、
  傾けられる範囲 (案: 縦横それぞれ ±75 度)、裏側を見せるか、回転の中心 (レンズの中心の下の点か、画像の中心か)、
  見開きを含めるか、キーでのリセット。利用者へ確かめてから着手する。

## 10. テスト

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

(実装担当が書く。`is_depth()` / `ExclusiveViewKind::Depth` / `fs_exclusive_view_active` の全出現の分類表、Depth の終了処理の全出現と
§3.6 の共通 helper への振り分け、レンズの移動の所有の形、環境設定の置き場所、変更ファイル、設計との差分、実行したテストと結果、perf の数値。)

## 12. 文書

- [display-pipeline.md](display-pipeline.md): PlaneView の節 (§5 の要約と §6 の出口)。
- [keymap-spec.md](keymap-spec.md)・`docs/keymap.ini.default`: Action 2 つ、固定入力 2 つ (Ctrl+Shift+ホイール、PlaneView 中の Shift+左ドラッグ)。
- [architecture-overview.md](architecture-overview.md): 新しいモジュール。
- マニュアル (全画面の表示のページ) と製品ページ: 機能の説明 (版固有の説明・実装用語を書かない)。
- [detached-rework-plan.md](detached-rework-plan.md): §4.4 の記録。
- backlog: §1.917 と §1.819 の状態をこの設計書へ向ける。
- CHANGELOG の下書き (§13)。

## 13. CHANGELOG の下書き

(段 1 の実装後に書く。)

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
