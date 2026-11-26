# 魚眼ズームと写真の板の 3D 傾け (§1.917・§1.819) — 設計書

- 対象: backlog §1.917 (静止画のズームに魚眼レンズで近寄った変形を加える、利用者要望 2026-11-27) と
  §1.819 (平面画像を傾けて 3D 回転する表示、5ch >>816、backlog-on-hold)。
- ブランチ / worktree: `fisheye-917` / `G:\home\mimageviewer-fisheye917` (基点 master `f2e332bb8`)。
- 役割: 設計・作業指示・検収 = ClaudeCode Opus 5.5、実装 = Codex GPT-6.1 Sol / high、
  独立レビュー = 実装者とは別の GPT-6.1 Sol / xhigh (AGENTS.md)。
- 状態: 設計 (R0)。製品コードは未変更。

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
| タッチのピンチ・ゲームパッド | 今どおり普通のズーム。要望が出てから |
| 3D 写真の深度を使う透視 | 別機能の組み合わせになる。必要になれば別の段 |
| 魚眼中の Ctrl+ホイール等のカーソル基準の補正 | §3.2 の「既知の挙動」。普通のズームの計算のまま |

### 段 2 (傾け、§1.819)

§9 にまとめる。段 1 の実装・実機確認の後に、残りの論点 (傾けられる範囲・裏面・回転の中心) を利用者に確かめてから着手する。

## 2. 今の作り (コード確認、master `f2e332bb8`)

- 全画面の静止画の幾何は `DisplayedImageTransform` (src/displayed_image_transform.rs) が `fs_zoom`・`fs_pan`・
  `fs_free_rotation`・90 度回転・表示トリム・フィットから 1 回で確定する ([表示パイプライン §2.4](display-pipeline.md))。
  `fs_zoom` / `fs_pan` は viewer context ごと (viewer_context_registry.rs の退避対象) で、ページを移ると
  `fs_zoom = 1.0`・`fs_pan = 0` に戻る (app.rs の画像切り替え時のリセット)。
- 一表示一投影の持ち主 `FsExclusiveView { None, Panorama, Depth }` (src/fs_exclusive_view.rs) があり、
  遷移は `App::set_fs_exclusive_view` (src/app/depth_view.rs) だけが行う。占有中かの集約は
  `fs_exclusive_view_active(fs_idx) -> Option<ExclusiveViewKind>`。`FsExclusiveView` は viewer context の束の中にあり、
  context の退避・切替に乗っている。
- 3D 写真 (src/depth_parallax_wgpu.rs) は egui_wgpu の callback で画面全体の三角形を描き、フラグメントの物理画素から
  「2D と同じ四角形の局所座標 → 90 度回転 → トリム → UV」と逆にたどる ([3D の表示変換 §3.2](depth-view-transform-plan.md))。
  元テクスチャは `DepthGpu::copy_color` で mipmap 付きの `Rgba8Unorm` へ GPU だけで写す。
  書き出しは同じ WGSL を `DepthFrameRenderer` (src/depth_parallax_wgpu/export.rs) でオフスクリーンに描き、
  透過の下地 (黒・白・市松) も shader で合成する ([3D 写真 §7.2](depth-parallax-plan.md))。
- 360 度パノラマ (src/panorama.rs `PanoProjection` / `ProjectionMap`、src/panorama_wgpu.rs `projection_theta`) に、
  立体射影・等距離・等立体角の「画面の半径 ↔ 入射角」の式が CPU / WGSL の 1:1 の対で既にある。
- ホイール: 全画面の静止画では `modifiers.ctrl` だけを見てズームに振る (ui_fullscreen.rs `should_zoom_fullscreen_wheel`)。
  したがって今の Ctrl+Shift+ホイールは Ctrl+ホイールと同じズーム。360 度は修飾に関係なくホイールを画角に使い、
  Z の全画面ズーム・分析モード・編集モードは先にホイールを取る。
- 修飾だけの押している間の操作: Shift = ルーペ (`FsLoupeHold`)、Alt = ナビゲータ (`FsNavigatorHold`)、
  右 Ctrl = 元画像表示 (`FsOriginalPreviewHold`)。いずれも「その修飾だけ」が押されているときに働く (keymap.rs `matches_only`)。
- 左ドラッグ: 修飾なし = パン、Ctrl (+ほかの修飾) = 自由回転 (ui_fullscreen.rs の通常モードのドラッグ)。
  今の Shift+左ドラッグはパン (Shift でルーペも出る)、Ctrl+Shift+左ドラッグは回転。
- 保存の入口: Ctrl+S / リングの「キャプチャ保存」= `save_image_capture_to_file` → `prepare_capture_pixel_work` →
  worker で `capture::run_pixel_work` → `save_rgba_unique_with_matte`。Ctrl+E = `open_export_dialog` →
  `ExportComposite::{Single, Spread}`。画像のコピー = `FsCopyImage` (既定キーなし、リング等)。範囲コピーは
  `begin_capture_region_selection`。

## 3. 操作

### 3.1 状態

`FsExclusiveView` に 4 つ目の値 `PlaneView(PlaneViewState)` を足す (`ExclusiveViewKind::PlaneView` も足す)。

```rust
pub(crate) struct PlaneViewState {
    /// 入ったときのページ。描く直前の照合 (§4.2) で今のページと違えば終わる。
    anchor: PlaneViewAnchor,            // fs_idx と、その項目を同一と判定できる既存の identity
    /// 0 < strength <= 1。0 にする操作はこの状態を None へ戻す (strength 0 の PlaneView は作らない)。
    strength: f32,
    /// レンズの中心。表示領域 (canvas) に対する 0..1 の位置。窓の大きさが変わっても同じ割合の位置に残る。
    lens_center: egui::Vec2,
    /// 段 2 で使う。段 1 では常に 0 (shader へは渡す)。
    tilt: PlaneTilt,                    // { yaw: f32, pitch: f32 }
    /// mip 付きの色の写し (§5.2)。表示テクスチャの TextureId を鍵にする。
    color: PlaneColorPhase,
    /// Shift+左ドラッグの開始点 (押した位置と、そのときの lens_center)。
    lens_drag: Option<(egui::Pos2, egui::Vec2)>,
}
```

- 倍率・パン・回転は **2D の状態そのもの** (`fs_zoom` / `fs_pan` / `fs_free_rotation`) を使い、PlaneView 専用の
  変換は持たない (3D 写真の [表示変換 §3.1](depth-view-transform-plan.md) と同じ判断)。ズームの持ち主は `FsZoomOwner::Image`。
- `strength` と `tilt` が両方 0 になったら PlaneView を終える (段 1 では strength だけ)。「何も変形しない PlaneView」は作らない。
- レンズの種類は `Settings` (§3.5) にあり、状態には持たない。

### 3.2 Ctrl+Shift+ホイール — 寄る / 離れる

1 ノッチ (ホイールの 1 段。普通のズームと同じ単位) ごとに:

- **上 (寄る)**: PlaneView でなく入口条件 (§4.1) を通れば、`strength = 1/8`、`lens_center = カーソル位置`
  (カーソルが表示領域の外なら中央) で入る。PlaneView 中なら `strength = min(1, strength + 1/8)`。
  どちらも、倍率を Ctrl+ホイールと同じ 1 段上げる (`apply_wheel_zoom`、`fullscreen_wheel_zoom_step`)。
  **ズームの基準点はカーソルではなくレンズの中心** にする。レンズの中心は歪みの不動点で、そこでの倍率は 2D の倍率と
  同じ (§5.1) なので、2D の「基準点を固定するズーム」の計算がそのまま正しい。
  strength が 1 になった後の上ノッチは倍率だけ上がる。
- **下 (離れる)**: PlaneView 中なら `strength = max(0, strength - 1/8)` と倍率を 1 段下げる (基準点はレンズの中心)。
  strength が 0 になったノッチで PlaneView を終える。PlaneView でないときの下ノッチは、今の Ctrl+Shift+ホイールと
  同じ普通の縮小 (カーソル基準)。
- 入口条件を満たさないとき (見開き・連結・動画・3D 写真中など) の上ノッチは、今どおり普通の拡大。理由の案内は既存の
  `show_fullscreen_nav_noop` の流儀で出す (同じ理由の連続表示の扱いも既存に従う。新しい時間窓は作らない)。
- 先にホイールを取る既存のモード (360 度・Z の全画面ズーム・分析・編集モード・パネル上・ポップアップ) では、
  今と同じ扱いのまま (ここへは届かない)。
- 判定は wheel segment の `modifiers` (`ctrl && shift && !alt`) で行う。同じ pass の予測 (`predict_fs_wheel_frame`) と
  実適用 (`apply_fs_wheel_frame_proposal`) は同じ分類を使う (3D の §3.5 と同じ規則)。

魚眼中のほかのズーム入力 (Ctrl+ホイール・`FsZoomIn/Out`・中ボタン・Ctrl+Space のスクラブ・ナビゲータは休止中) は
strength を変えず、倍率だけを今の計算 (カーソル基準) で変える。**既知の挙動**: 歪みのため、カーソルの下の点が少しずれる
(レンズの中心にカーソルがあればずれない)。段 1 では直さない。

### 3.3 ドラッグ

| 操作 | PlaneView 中 | PlaneView でないとき |
| --- | --- | --- |
| 左ドラッグ (修飾なし) | 写真を動かす (今のパン、§1.806 の制限も今どおり) | 今どおり |
| **Shift+左ドラッグ** | **レンズだけ動かす**。写真は止まったまま、`lens_center` が画面上をドラッグ量だけ動く (表示領域の中に制限) | 今どおり (パン) |
| Ctrl+左ドラッグ | 自由回転 (今どおり) | 今どおり |
| Ctrl+Shift+左ドラッグ | 段 1: 今どおり (自由回転)。段 2 で傾けに変える | 段 1: 今どおり |

- Shift+左ドラッグの判定は押した瞬間の修飾で決め、ドラッグの途中で Shift を離しても同じドラッグはレンズの移動のまま
  (既存の「開始で所有を決める」規則と同じ)。修飾はフルスクリーンの stale な修飾を避けるため OS から読む既存の helper を使う。
- PlaneView 中はルーペを描かない (§4.3) ので、Shift を押してもルーペは出ない。

### 3.4 キー (`KeyAction`)

- `FsFisheyeIn` (魚眼で寄る) / `FsFisheyeOut` (魚眼で離れる): `KeyContext::FsImage`、`KeyTrigger::Press`、
  **既定キーなし** (利用者の判断)。1 回の押下が 1 ノッチと同じ。PlaneView でないときの `FsFisheyeIn` はレンズの中心を
  表示領域の中央にして入る (キーにはカーソル位置の意味が無いため)。
- keymap の決まりどおり `ini_name()` / `context()` / `trigger()` / `default_chords()` (EMPTY) / `ALL_ACTIONS` /
  説明文 / 呼び出し側 helper / `docs/keymap.ini.default` / `docs/keymap-spec.md` を揃える。リング・マウスボタンの
  候補には段 1 では出さない。
- Ctrl+Shift+ホイールと Shift+左ドラッグは、今の Ctrl+ホイール・ドラッグと同じく **固定入力** (keymap の対象外)。
  `docs/keymap-spec.md` の固定入力の表に理由 (ポインター位置を使う連続操作) を書く。

### 3.5 設定

- `Settings` に `plane_view_lens: PlaneLensKind` (`Equidistant` 既定 / `Stereographic` / `EquisolidAngle`) を足す。
  未リリースの項目なので serde の既定値で読め、移行は不要 (コミットメッセージに一言書く)。
- 環境設定の画面: 全画面の表示の設定の近くに 1 行 (置き場所は実装担当が既存の構成を見て決め、実装記録に書く)。
  変えたら次の描画から効く (状態は持たないので作り直しは要らない)。
- 段 1 では strength の刻み (1/8)、最大の強さ (§5.1 の `h_min`)、基準の画角 (§5.1 の `α0`) は定数。実機で見て詰める。

### 3.6 終わり方

| きっかけ | 動作 |
| --- | --- |
| Ctrl+Shift+ホイール下 / `FsFisheyeOut` で strength 0 | 終わる |
| 変換中のダブルクリック (`FsCanvasClickAction::ResetTransform`) | 倍率・パン・回転と一緒に終わる |
| フィット方式の切替・リセット (`reset_fullscreen_fit_transform`) | 終わる |
| ページ移動 (どの入口でも) | 描く直前の照合 (§4.2) で終わる。各移動の関数に終了処理を足さない |
| 360 度 (V)・3D 写真 (D) に入る | `set_fs_exclusive_view` が PlaneView を捨てて切り替える |
| F11 (MainWindow ⇔ Fullscreen)、F12 (別ウィンドウの ON/OFF)、session の独立性の実切替 | 3D 写真と同じ共通の確定点で終わる。通知はしない (強さはホイールですぐ戻せる) |
| 別ウィンドウの中の F11、窓の大きさ・DPI の変更 | 続ける。`lens_center` は割合なので同じ位置に残る |
| 閲覧の終了 (Esc・一覧へ戻る) | 既存の閲覧終了で終わる |

## 4. ほかの機能との関係 (状態の組み合わせを減らす)

### 4.1 入口条件 (1 つの関数)

`plane_view_entry_allowed(fs_idx) -> Result<(), PlaneViewEntryBlock>` を 1 つ作り、ホイール・キーが同じ答えを使う。
弾く条件: 画像の閲覧中でない、動画・音声・音楽表示、アニメーション画像、見開きの 2 ページ表示、連結読み、
wgpu の描画状態が無い、編集系のモード (消しゴム・隠蔽加工・補正レイヤー・テキスト注釈・切り取り・SNS 分割)、
補正パネル、分析、比較 (Wipe・差分・ピン表示)、Z の全画面ズーム (照準・確定)、紙めくりの最中、
3D 写真・360 度が有効。

- **入るときにほかのモードを閉じない** (ホイールの 1 ノッチで別のモードが閉じるのは驚きが大きい)。弾いて理由を出す。
  3D 写真の入口が補正パネル等を閉じるのとは違う判断 (あちらはキー / ボタンでの明示の入場)。
- 3D 写真と違い、ほかの context が PlaneView を持っていても弾かない (GPU の資源を context ごとに callback が持つため。§5.3)。

### 4.2 続けてよいかの照合

3D 写真の `depth_view_may_continue` と同じ形の `plane_view_may_continue(fs_idx)` を作り、**同じ 2 か所**
(viewer の pass の先頭、描画分岐の直前) から呼ぶ。入口条件のうちページと表示の状態に関わるもの
(静止画・1 ページ・連結でない・編集系でない・anchor が今のページ) を満たさなければ `set_fs_exclusive_view(None)` で終わる。
通知はページ移動では出さず、それ以外 (同じページで見開きに切り替わった等) は 1 回出す。

### 4.3 占有中の扱い

`fs_exclusive_view_active` が `PlaneView` を返すようにし、既存の判定箇所を 3D 写真と同じ方法で分類する
(3D の [§6.5](depth-parallax-plan.md) と [表示変換の実装記録](depth-view-transform-plan.md#13-実装記録) の分類表が先例)。
**実装担当は `is_depth()`・`ExclusiveViewKind::Depth`・`fs_exclusive_view_active` の全出現を表にし (箇所・PlaneView での扱い・理由)、
§11 の実装記録に置く。** 決めておく扱い:

| 相手 | PlaneView 中 |
| --- | --- |
| ズーム・パン・自由回転・ページ内スクロール | 2D と共用 (`FsZoomOwner::Image`) |
| 修飾なしのクリック・タップ | 変換ありとして扱う (ページを送らない、画像の外のクリックで閉じない)。ダブルクリックでリセット (§3.6) |
| ルーペ・ピクセルグリッド・ナビゲータ (固定表示・Alt 押下) | 描かない (カーソル ↔ 画素の対応が変わるため)。トグルの状態は変えず、抜ければ戻る |
| 紙めくり | 対象外 (`page_turn_view_eligible` を占有の述語で判定。3D と同じ)。ページ移動は紙めくり無しで切り替わり、§4.2 で終わる |
| スライドショー | 止めない。次のページで §4.2 により終わる |
| 範囲コピー | 弾く (理由: 魚眼中は使えない) |
| Ctrl+S・画像のコピー・Ctrl+E | 見えているとおりの絵 (§6) |
| フィットの循環 (`0`)・上部のメニュー | 受け付ける。フィットが変わったら終わる (§3.6) |
| 透過の背景 (Shift+B) | 受け付ける。shader の下地に反映 (§5.1) |
| 補正・AI 拡大・ポストフィルタの効果 | 最終の表示テクスチャをそのまま使うので効く。テクスチャが変われば mip を作り直す |
| 拡大方式 (シャープ / アニメ塗り / ドット絵 / Lanczos 縮小) | 効かない。PlaneView は `FullscreenPaintResource` の resampler を通らず、元の `TextureHandle` から mip を作る |
| PDF の再ラスタ | レンズの中心の倍率 = 2D の倍率なので、今の需要のまま |
| 左右のパネル (メタデータ等) | 今どおり |
| mIV Remote・MCP・操作のスクリプト | 端末ローカルの見た目だけで、共有する状態を変えないので対象外 |

### 4.4 別ウィンドウ (detached) — リワーク中の規則との関係

- 状態は既存の `FsExclusiveView` の中に置くので、**viewer context の退避項目を足さない**。detached の述語・viewport の経路・
  入力の配送は変えない。終了の確定点 (§3.6 の F11 / F12) は 3D 写真が既に使っている共通の確定点に PlaneView を加えるだけ。
- これは症状パッチではなく、新しい表示状態を既存の持ち主に置く構造上の追加である、という主張について、
  CLAUDE.md の規則どおり独立設計レビューで合意を取り、合意の結果を [detached-rework-plan.md](detached-rework-plan.md) に記録する。

## 5. 描画

### 5.1 式 — 魚眼のカメラで傾けた写真の板を見る

記号 (すべて物理画素):

- `canvas`: その pass の画像の表示領域 (2D の `full_rect`)。`R = 0.5 × min(canvas の幅, 高さ)`。
- `c = canvas.min + lens_center × canvas.size`。画面の点 `p` について `u = (p − c) / R`、`r = |u|`、`û = u / r`。
- レンズの式 `g` と逆関数 (パノラマと同じ表): 等距離 `g(θ) = θ`、立体射影 `g(θ) = 2·tan(θ/2)`、
  等立体角 `g(θ) = 2·sin(θ/2)`。いずれも `g'(0) = 1`。
- 画角: `r = 1` に写る入射角 `α(s) = α0 + s × (α_max − α0)`、`k = g(α(s))`。
  `α0 = 30°`、`α_max` は「地平線 (θ = 90°) の円の半径が `h_min = 0.8`」になる値 (`g(α_max) = g(90°) / 0.8`。
  等距離で 112.5°)。等立体角で `g(α_max) > 2` になり逆関数の定義域を超える場合は、`α_max` を定義域の内側
  (`g(α_max) ≤ 2`) へ制限する (実装担当が定数で確定し、テストで固定する)。

1 画素の計算:

1. 入射角 `θ = g⁻¹(k · r)`。定義域の外 (`k·r` が等立体角の 2 を超える等) は画像なし。
2. カメラの向き (z が前、画面の y は下向きのまま): `d = (sin θ · û, cos θ)`。
3. 板: カメラから距離 1 の点 `P0 = (0, 0, 1)` を通り、法線 `n = Rot(tilt) · (0, 0, −1)` の平面。
   交点 `t = (n · P0) / (n · d)`。`n · d ≥ 0` 側 (板の裏・地平線の外) と `t ≤ 0` は画像なし。
4. 板の局所座標 `L = Rot(tilt)⁻¹ · (t·d − P0)` の xy。傾き 0 なら `L = tan θ · û`。
5. **2D の画面の点** `q = c + (R / k) · L`。これを今の 2D の逆写像 (3D 写真と同じ「四角形の局所座標 → 90 度回転 →
   トリム → UV」) にかけて色を引く。
6. 透過部分には下地 (黒・白・市松。画像の左上を原点とする市松の升目は 3D 写真と同じ規則) を shader で合成する。
   画像なし (1・3) と UV が [0,1]² の外は透明 (premultiplied で 0) にし、egui が描いている背景がそのまま見える。

性質 (テストで固定する):

- 傾き 0 では `q = c + (p − c) · W(r)`、`W(r) = tan θ(r) / (k · r)`、`W(0) = 1`、`W` は単調増加。
  レンズの中心の倍率は 2D の倍率と同じで、周りほど広い範囲が押し込まれて見える。
- `s → 0` で `W → 1` に近づく (段 1 では strength 0 の PlaneView は無いので、恒等は「PlaneView を抜けると 2D」で保つ)。
- 平らな板なので、カメラが寄ることと 2D の拡大は同じになる。寄るほど歪むのは、対象が画面の歪む部分に広がるため。
  §3.2 で倍率と強さを一緒に上げるのはこのため。
- 90 度回転・自由回転・トリム・フィットは 5 の 2D の逆写像の中にあるので、そのまま効く。

### 5.2 shader と色の写し

- egui_wgpu の `CallbackTrait` で、callback の矩形は **canvas と painter の clip の交わり** (レンズの中心が画像の外にある場合や
  傾けた場合は、画像が 2D の四角形の外に出るため)。頂点は画面全体の三角形 (3D 写真と同じ)。
- フラグメント: `@builtin(position)` から 5.1 の 1〜5 → `textureSampleGrad`。微分 (`dpdx` / `dpdy`) は一様な制御フローで
  先頭に取り、画像なし・範囲外は最後に `select` で差し替える (WGSL の制約。3D の §3.2 と同じ)。
- 色: 表示テクスチャ (2D が選んだ素材の元の `TextureHandle`) を `DepthGpu::copy_color` と同じ方法で mip 付きの
  `Rgba8Unorm` へ写す (縮む周辺のちらつきを防ぐ)。写す処理は 3D 写真と共用の関数に括り出す (WGSL・blit を 2 つ持たない)。
  写すのは PlaneView に入ったときと、表示テクスチャの `TextureId` が変わったときだけ。形式が `Rgba8Unorm` でない等で
  写せないときは PlaneView に入らず理由を出す (継続中に起きたら終わって通知。silent fallback にしない)。
- sampler は linear + mipmap linear。異方性フィルタは使える環境なら 16 (実装担当が wgpu の features を確かめ、無ければ無し)。
- 2D と同じ下地・背景・clip の規則を使い、2D の `paint_image_underlay` は PlaneView 中は描かない (shader が描く)。
- WGSL は 1 つ。画面用と書き出し用 (§6) で同じ文字列を使う。レンズの式はパノラマの `projection_theta` と同じ表に揃え、
  CPU 版 (テスト・§6 の検証用) と 1:1 にする。naga で parse / validate するテストを置く (panorama_wgpu の先例)。

### 5.3 資源の持ち主と寿命

- mip の写し・uniform buffer・bind group は `Arc<PlaneGpuResources>` にまとめ、**PlaneViewState が持ち、callback も Arc で持つ**。
  egui_wgpu の `CallbackResources` に context 共通の slot を置かない (3D 写真の slot は 1 context 前提のため。PlaneView は
  複数の窓で同時に使える)。uniform は callback の `prepare` で自分の buffer に書く。
- 解放: PlaneView を抜けたとき (`set_fs_exclusive_view` で state が drop)、テクスチャが変わって写し直したとき。
  callback が持つ Arc は描画の終わりで落ちる (Lanczos の出力の寿命と同じ考え方。§2.4.1)。
- メモリ: 表示テクスチャの約 1.33 倍 (6000×4000 で約 128 MB)。PlaneView 1 つにつき 1 枚。

### 5.4 負荷

- 1 画素あたり、三角関数数回 + `textureSampleGrad` 1 回。4K の画面 (約 830 万画素) でも 3D 写真 (1 画素約 45 サンプル) よりずっと軽い。
  描き直しは操作中だけ (今の 2D と同じ再描画の規則)。
- 写しは GPU だけで、読み戻しなし。UI スレッドで待たない。`perf::event` を写しと書き出しの描画・読み戻しに入れる
  ([ui-responsiveness.md](ui-responsiveness.md) §4 のチェックリストを通す)。数値は実装後に測って §11 に書く。

## 6. 書き出し・キャプチャ・コピー (見えているとおりの絵)

PlaneView 中だけ、3 つの出口の **元の素材を「見えているとおりの絵」に替える**。PlaneView でないときは今と同じ。

### 6.1 凍結した要求

入口で `PlaneFrameRequest` を作り、worker はこれだけを見る:
mip の写しの `Arc`、uniform の値 (§5.1 の `c`・`R`・`k`・レンズの種類・傾き・2D の逆写像の値)、出力の寸法、
下地の種類と市松の升目 (出力の寸法の比で拡大縮小)、画像の外の色 (画面で画像の外に見えている背景の色)。

- 出力の範囲は **canvas そのもの** (上下のバー・パネル・HUD は入れない)。縦横比は canvas と同じ。
- 画像の外と透過部分は不透明に合成する (3D 写真の書き出しと同じ。JPEG・PNG で挙動を分けない)。

### 6.2 描画

- 3D 写真の `DepthFrameRenderer` と同じ型の `PlaneFrameRenderer`: 同じ WGSL、書き出し専用の pipeline・uniform・bind group、
  オフスクリーンの `Rgba8Unorm` に描いて読み戻す。worker が encode → submit → `poll(Wait)` する (UI スレッドは待たない)。
- 寸法が device の上限を超える場合は、上限に収まるよう縮めた寸法で描く (縦横比は保つ)。

### 6.3 3 つの出口

| 出口 | PlaneView 中の動作 |
| --- | --- |
| Ctrl+S / リングの「キャプチャ保存」 | `CapturePixelWork` に PlaneView の要求の値を足し、worker で描いて読み戻した RGBA を今の `save_rgba_unique_with_matte` へ渡す。寸法 = canvas の物理画素 (画面と同じ)。保存先・形式・名前の規則は今どおり (名前に `_fisheye` を付ける) |
| 画像のコピー (`FsCopyImage` とその入口) | 同じ要求で描いた RGBA を今のクリップボードの経路へ渡す。寸法は画面と同じ |
| Ctrl+E (書き出しダイアログ) | `ExportComposite` に `PlaneView(PlaneFrameRequest)` を足す。基準の寸法 (ダイアログの「元の大きさ」) は **canvas の物理画素の 2 倍** (長辺 8192 まで)。既存の倍率 (1/2 = 画面と同じ、1/4、長辺指定) はそのまま使え、縮小は描く寸法を変えて行う (描いてから縮めない)。隠蔽プリセットの出力 (`_1`〜`_4`) は出さない。メタデータの埋め込みは元画像の EXIF を引き継がない (見た目の作り直しのため)。ダイアログの表示は「魚眼の見た目」と分かる文言にする |

- 範囲コピーは §4.3 のとおり弾く。
- 書き出しダイアログ・保存中の扱い (モーダル・既存の pending の排他) は今の各出口のまま。
- まれな失敗 (読み戻し・保存の失敗) は既定の割り切り: ログとエラー通知だけ (CLAUDE.md)。

## 7. 設計の簡素化の検討 (CLAUDE.md)

| 検討 | 判断 |
| --- | --- |
| 魚眼専用の倍率・パンを持つか | 持たない。2D の状態を共用する (3D 写真と同じ)。持つと全ズーム入力に書き先が 2 つになる |
| 入るときにほかのモードを閉じるか | 閉じずに弾く (§4.1)。ホイール 1 ノッチの副作用を小さくする |
| ページを移っても続けるか | 続けない。倍率と同じ寿命にし、終わりの照合は描く直前の 1 関数 (§4.2) に集める |
| F11 / F12 で引き継ぐか | 引き継がない。3D 写真と同じ確定点で終える |
| 見開き・連結・アニメーション | 段 1 は弾く (組み合わせを増やさない) |
| GPU 資源を context 共通の slot に置くか | 置かない。Arc を状態と callback が持ち、context 間の取り違えを型で無くす |
| 書き出しの寸法の選択肢 | 既存のダイアログの倍率をそのまま使い、PlaneView の基準だけ決める |
| strength 0 の PlaneView | 作らない (「何もしていないのに占有中」を無くす) |

## 8. まれな失敗

既定の割り切り (ログ + 通知)。写しの失敗は入らない / 終わる + 通知、書き出しの失敗はエラー通知。再試行はしない。

## 9. 段 2 — 写真の板の 3D 傾け (§1.819)

- 操作: **Ctrl+Shift+左ドラッグで板を傾ける** (Hamana と同じ組み合わせ)。左右のドラッグで縦軸まわり (yaw)、
  上下で横軸まわり (pitch)。今の Ctrl+Shift+左ドラッグ (自由回転) から変わる (1 ページ表示の静止画だけ。利用者へ報告済み)。
- 状態は段 1 の `PlaneViewState::tilt`。strength 0 でも傾きがあれば PlaneView を続ける。傾けを始めたときの PlaneView の
  入り方 (strength 0 のときは基準の画角 `α0` の普通の透視で描く) は段 2 の設計で詰める。
- shader は段 1 のまま (§5.1 の 3・4 が傾けを扱う)。書き出しも同じ要求に傾きが入るだけ。
- 段 2 の前に利用者へ確かめる点: 傾けられる範囲 (案: 縦横それぞれ ±75 度)、裏側を見せるか、回転の中心
  (レンズの中心の下の点か、画像の中心か)、見開きを含めるか、キーでのリセット。

## 10. テスト

- 式: 傾き 0 で `W(0) = 1`・単調・地平線の判定・定義域の外、3 種のレンズで CPU 版と WGSL の係数の一致、
  傾き 0 の交点の式が `tan θ · û` と一致すること。
- WGSL: naga で parse / validate (画面用・書き出し用は同じ文字列)。
- 状態遷移 (App の状態のテスト): Ctrl+Shift+ホイール上で入る (lens_center = カーソル)、8 ノッチで strength 1、その後は倍率だけ、
  下で戻って 0 で終わる、PlaneView でない下ノッチは普通の縮小、入口条件で弾く各条件 (見開き・連結・動画・アニメーション・
  3D・360・編集・比較・分析・Z・紙めくり中) で普通のズームのまま、ページ移動・ダブルクリック・フィット切替・V・D で終わる。
- ホイールの振り分け (handler-level): Ctrl+Shift と Ctrl だけの区別、パネル上・ポップアップ・編集モードでは今どおり、
  同じ pass の予測と実適用の一致。
- ドラッグ: Shift+左ドラッグでレンズだけ動く (pan 不変)、途中で Shift を離しても同じドラッグ、PlaneView でないときは今どおり。
- 占有: ルーペ・ピクセルグリッド・ナビゲータを描かない、紙めくりの対象外、クリックでページを送らない。
- keymap: 2 つの Action の既定が空、ini の往復、競合検出、`docs/keymap.ini.default` との一致。
- 別ウィンドウ: 2 つの context が同時に PlaneView を持ち、互いの状態・資源を変えないこと (sibling context 不変)。
- 書き出し: 小さな画像と既知の uniform で、オフスクリーン描画の画素が CPU 版の式の結果と一致すること (中心・周辺・地平線の外・透過の下地)。
  GPU を使うテストは既存の `test-group gpu` に入れる。
- UI スナップショット: 環境設定の 1 行と書き出しダイアログの文言を変えたら更新する ([ui-snapshot-policy.md](ui-snapshot-policy.md))。

## 11. 実装記録

(実装担当が書く。`is_depth()` / `ExclusiveViewKind::Depth` / `fs_exclusive_view_active` の全出現の分類表、変更ファイル、
設計との差分、実行したテストと結果、perf の数値。)

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

(依頼と結果を記録する。)
