# 実装ブリーフ: 360 度動画の投影パス (§1.112 第 2 段)

対象 worktree: `C:\home\mimageviewer-pano` (branch `panorama-projection`)。
**この worktree で他の codex を並行させないこと。**

## 0. 先に読む

1. [docs/next-release-backlog.md](../next-release-backlog.md) **§1.112** — 段階と決定事項の正本。
   第 1 段 (判定) は完了済み。この作業は**第 2 段 (描画) だけ**。
2. [docs/panorama-360-view-plan.md](../panorama-360-view-plan.md) **§13** — 投影方式 4 種の数式。
   **WGSL の `projection_theta` をそのまま HLSL へ移す。**
3. [docs/video-upscale-shader-plan.md](../video-upscale-shader-plan.md) — §1.47 で入った
   「表示解像度 swap chain + シェーダ解決」の構造。投影はこのステージに載る。

## 1. スコープ

**やること**: presenter に equirectangular 投影パスを足し、`set_panorama_pose()` で
姿勢を渡されている間はそれで最終解決する。

**やらないこと (第 3 段)**: 見回しドラッグ / ホイール FOV / タッチ / HUD ボタン /
KeyAction / 設定 UI。**入力と UI には一切触らない。** この段の検証は
`set_panorama_pose` を呼ぶ単体テストと、姿勢固定での描画確認まで。

`src/ui_fullscreen.rs` と `src/app.rs` は**触らない** (レーン B とのマージ衝突を避ける)。

## 2. 既に決まっていること (変更しない)

- **実行時 `D3DCompile` を復活させない。** presenter のシェーダは build.rs が FXC で
  `.cso` 化して `include_bytes!` する ([build.rs](../../build.rs) の
  `compile_video_presenter_shaders`)。投影シェーダも同じ `SHADERS` 表に 1 行足す。
  実行時コンパイルは placement 切替のたびに数秒固まる (backlog §1.122)。
- **`VideoResampleMode` に variant を足さない。** あの enum は設定から決まる filter の
  選択で、`select_video_resample_mode` と多数の perf イベント名 match に紐づく。
  毎フレーム変わる姿勢を混ぜない。**投影は別パイプライン**にして resolve 直前で分岐する。
- **投影と表示スケーラー (Anime4K / Lanczos / NIS / nearest) は排他。** 両方が
  「表示解像度の最終出力」を所有するため。投影が有効な間はスケーラーを走らせない。
  ⚠ **利用者の設定値 (`VideoScaleFilter`) は書き換えない。** 実効描画モードとして
  走らせないだけにする (UI 側の案内は第 3 段)。
- **姿勢の型は静止画の `crate::panorama::PanoPose` をそのまま使う**
  (yaw / pitch / fov_y / `PanoProjection`)。型を分けると stale 判定と画角の丸めが 2 つになる。
- **部分 FOV の UV も静止画と同じ `crate::panorama::PanoUvTransform`**。
  第 1 段の [spherical_metadata.rs](../../src/video/spherical_metadata.rs) が既にこの型を作る。

## 3. 作るもの

### 3.1 `src/video/native_presenter/shaders/video_panorama.hlsl` (新規)

[src/panorama_wgpu.rs](../../src/panorama_wgpu.rs) の `SHADER` を HLSL へ移植する。

- `vs_main` は `video_resample.hlsl` と同じフルスクリーン三角形でよい。
- `ps_main` は WGSL の `fs_main` と同じ手順:
  NDC → 像面座標 → 半径 → `projection_theta` → カメラ方向 → pitch/yaw 回転 →
  経緯度 → 球面 UV → crop 変換 → サンプル。
- **`projection_theta` は WGSL 版と 1:1 で移す。** 方式コード `PROJ_*` は
  `PanoProjection::shader_code()` と同じ番号 (0=透視 / 1=立体射影 / 2=等距離 / 3=等立体角)。
- **定義域外は不透明の黒**。WGSL と同じく**早期 return せず、最後の色選択でだけ判定する**。
- **seam の扱いを壊さない。** WGSL は経度の branch cut を跨ぐ U 勾配を `round` で
  巻き戻して `textureSampleGrad` に明示している。HLSL でも同じことを
  `SampleGrad` で行う (`ddx`/`ddy` を uniform control flow 内で評価する)。
- **crop 時の軸別 half-texel inset clamp** も WGSL と同じに移す (垂直 crop だけのときは
  U を clamp せず Repeat の seam を保つ)。
- サンプラは 2 つ用意する: U=Repeat/V=Clamp と U/V=Clamp (WGSL 側と同じ選択規則)。
- 定数バッファは WGSL の `Params` と同じ 3 × float4:
  `pose(yaw, pitch, fov_y, aspect)` / `crop(u_off, v_off, u_scale, v_scale)` /
  `proj(mode_code, k, 0, 0)`。**`k` は CPU 側 `ProjectionMap::coefficient()` を渡す**
  (GPU で `tan` を引き直すと静止画側と幾何が僅かにずれる)。

### 3.2 `src/video/native_presenter/panorama_pipeline.rs` (新規)

`resample_pipeline.rs` と同じ作法で:

- `.cso` を `include_bytes!` して VS/PS を作る。
- `draw(device, context, source: &ID3D11Texture2D, target: &ID3D11Texture2D,
   source_width, source_height, target_width, target_height, orientation,
   pose: PanoPose, uv: PanoUvTransform) -> Result<(), String>`
- **ミップは今回作らない。** まず bilinear で出して、広い画角でのエイリアスが実用に
  耐えるかを見る (backlog §1.112 の方針)。`GenerateMips` は耐えないと分かってから。
  **「将来ミップを足す」ための TODO コメントを残さない** — 判断は実測後。
- **`orientation` (回転メタデータ) をどう扱うかを決めて、決めた理由をコメントに書く。**
  回転付き 360 素材が実在するかは未確認なので、**まずは回転を投影の前段で適用する
  (= equirect のまま向きを正す)** のが素直。`resample_pipeline` の `inverse_axes` /
  `inverse_offset` と同じ考え方で source 座標を引く。

### 3.3 `render_core.rs` への接続

- `NativeRenderCore` に `panorama_pose: Option<(PanoPose, PanoUvTransform)>` を持たせ、
  **`set_video_grade` と同じ形の `set_panorama_pose(...)` を足す** (5527 行付近が前例)。
- `resample_active` / `resample_mode` を決めている **4814 行付近**で、姿勢が Some なら
  投影を選ぶ。`.draw(` の呼び出しは **4867 行 (CPU 経路) と 5008 行 (D3D11 共有経路) の
  2 箇所**にあるので、**両方**分岐させる。片方だけ直すと入力経路によって挙動が変わる。
- perf イベント名は既存の `cpu_upload_resample_*` / `d3d11_shared_resample_*` に倣って
  `*_panorama` を足す。

### 3.4 `surface_policy.rs`

- `VideoSurfaceSizeInput` に `panorama_active: bool` を足す。
- **`filter == OsDefault` の早期 return より前に**投影を判定する。投影中は filter に
  関係なく表示解像度サーフェスが要る。
- ⚠ **投影中の対象矩形は「動画のアスペクトでレターボックスした矩形」ではなく
  「表示領域そのもの」**。球はウィンドウ全体を埋める (どの 360 プレイヤーもそう)。
  `compute_video_visual_target_rect` が返すレターボックス矩形をそのまま使わないこと。
  `compute_video_visual_transform` 側も、投影サーフェスを表示領域へ 1:1 で置くようにする。
  **ここが一番間違えやすい。** 2:1 動画を 16:9 ウィンドウで開いたとき、通常表示では
  上下に黒帯が出るが、360 表示では出ない (球が全面)。
- シェーダへ渡す `aspect` も、この表示領域の `width/height` にする。

### 3.5 `video/mod.rs`

- 再生スレッドが `presenter.set_panorama_pose(...)` を呼べるようにする。
  `set_video_grade` が `cur_video_grade` を流している経路 (4071 / 4448 / 5083 行付近) と
  同じ形で、姿勢のスナップショットを流す。**まだ姿勢を変える入力は無いので、
  値の出どころは第 3 段で繋ぐ。** この段では「渡せる経路がある」ところまで。

## 4. テスト

- **HLSL のテキスト検査** (`panorama_wgpu.rs` の既存テストと同じ形):
  方式コードが `PanoProjection::shader_code()` と一致すること、`ps_main` の `return` が
  1 つだけであること (早期 return で `ddx`/`ddy` を非 uniform にしない)、`SampleGrad` を
  使っていること。
- **定数バッファのパック**: 静止画の `pano_uniform_bytes` と同じ値になること。
  同じ pose / uv から作った 48 バイトが一致するのが理想。
- **surface_policy**: 投影中は `OsDefault` でも `DisplayResolution` を返すこと。
  投影中の対象サイズが**レターボックスではなく表示領域**であること。
- **`set_panorama_pose` の state 遷移**: Some → None で通常の resample 経路へ戻ること。

`cargo test -p mimageviewer --lib` が緑になること。`cargo fmt` をかけること。
`cargo clippy` で新規ファイルに警告を出さないこと。

## 5. 実機確認は不要

この段では姿勢を変える入力が無いので、実機で 360 を見ることはできない。
**テストが緑になるところまでで止め、実機確認は第 3 段に回す。**
`scripts/build-dev.ps1` を通してビルドが成立することだけ確認する。

## 6. 迷ったら

- **症状を消す guard / delay / retry / silent fallback を入れない** (CLAUDE.md の一般原則)。
  投影パスが作れない状況 (シェーダ作成失敗など) は、`resample_pipeline_error` と同じく
  **型付きの理由を持って**通常経路へ落とす。黙って落とさない。
- 構造判断で迷ったら**実装せずに backlog §1.112 へ質問を書いて止める**。
