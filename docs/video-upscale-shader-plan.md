# 動画の拡大・縮小を mIV のシェーダで行う

ステータス: **Phase A / Phase B 実装済み / Phase B 実機画質・再生安定性確認待ち**
対象: [next-release-backlog.md](next-release-backlog.md) §1.47
関連: [upscale-algorithm-selection.md](upscale-algorithm-selection.md) (静止画側の正本) /
[video-architecture.md](video-architecture.md) / [display-pipeline.md](display-pipeline.md) /
[dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md)

2026-08-22 に Phase A を実装した。`OS に任せる` / `標準` / `ニアレスト` / `シャープ` を
動画左パネルとキー操作から切り替えられる。実機比較前の挙動不変を優先し、既定は
`OS に任せる` のままとする。Anime4K、GPU 時間計測、モデル自動選択は Phase B で行う。

静止画は v2.11.0 で縮小に GPU Lanczos、v2.12.0 で拡大に Lanczos3 / NIS / Anime4K を入れた。
Phase A 導入前の動画は**表示構造が違うため mIV のシェーダを一切通っておらず**、拡大縮小を
DWM / DComp に任せていた。本書は、その構造を変えた設計と残る Phase B を定める。

---

## 1. 導入前の構造 — なぜ mIV のシェーダが通らなかったか

```
デコード → ID3D11VideoProcessor で NV12→BGRA8 (動画解像度のまま)
        → 共有テクスチャ (KEYEDMUTEX + fence)
        → presenter が CopySubresourceRegion で 1:1 コピー
             ↓
           swap chain  ← ★動画解像度で作られている (例 1920x1080)
             ↓
           IDCompositionVisual::SetTransform2 で倍率を設定
             ↓
           ★DWM / DComp がここで拡大縮小する
```

- swap chain のサイズ = フレームサイズ
  ([create_video_swap_chain](../src/video/native_presenter/render_core.rs))
- 倍率は行列で DComp へ渡すだけ
  ([compute_video_visual_transform](../src/video/native_presenter/render_core.rs))
- mIV のシェーダが走るのは色調 / Creative LUT を使ったときだけで、それも**動画解像度で走る**
  ため拡大には関与しない ([grade_pipeline.rs](../src/video/native_presenter/grade_pipeline.rs))

したがって「拡大アルゴリズムを差し替える」には、まず**映像を mIV のシェーダに通す場所を作る**
必要がある。これがコストではなく構造の問題と呼んでいた中身である。

---

## 2. 採る構造 — 表示解像度サーフェス

**swap chain のサイズを「映像の表示矩形の物理ピクセルサイズ」にし、シェーダで
ソース解像度から表示解像度へ直接解決する。** DComp の transform は位置合わせだけになる。

```
共有テクスチャ(1920x1080) → シェーダ (Lanczos3 / NIS / Anime4K)
                          → swap chain (表示矩形サイズ、例 3840x2160)
                          → DComp は M11=M22=1 + オフセットのみ
```

### 2.1 なぜ整数倍サーフェスにしないのか

検討途中では「swap chain を動画解像度 × 2 にして、端数倍率は DComp に残す」案を採っていた。
これを捨てた理由は、**mIV の Anime4K 実装が既に任意の目標解像度へ直接解決できるから**である。

- NIS: `fs_nis` は 1 パスで `scale = source_extent / target_size`、倍率任意
  ([gpu_nis.wgsl](../src/gpu_nis.wgsl))
- Anime4K: 最終 resolve パスが `params.output_size` へ出力する。x2 固定ではない
  ([gpu_anime4k.wgsl](../src/gpu_anime4k.wgsl) の `fs_anime4k_resolve`)

整数倍に縛ると、1〜2 倍の中間倍率 (1080p を 1440p 画面で見る等) で「2 倍にしてから DWM が
縮小する」ことになり、一度作った情報を捨てる。任意倍率で出せるなら縛る理由がない。

### 2.2 リサイズ中はサーフェスを差し替えない

表示矩形サイズにすると、ウィンドウリサイズのたびに swap chain 再構築が要るように見える。
これは**やらない**。

- リサイズ中は既存サーフェスのまま、`compute_video_visual_transform` が今までどおり DComp に
  伸ばさせる (= 従来画質だが**追加コストゼロ**)
- リサイズが落ち着いてから 1 回だけ差し替える

`compute_video_visual_transform` は**サーフェスサイズを引数に取る**ので、
「シェーダで出した表示解像度サーフェス」も「リサイズ中の古いサーフェス」も**同じ式で正しい
倍率が出る**。この関数は無改造でよい。

Phase A の実コードでは Lanczos pass が SAR と orientation を表示解像度サーフェスへ焼き込む。
そのため関数本体は無変更のまま、表示解像度 content の呼び出しだけ 1/1・identity の幾何を
渡す。リサイズ中の古い表示解像度サーフェスは同じ関数が現 viewport へ stretch し、最後の
`GeometryChanged` から 150ms 新しい寸法 sample がないときだけ settled edge を 1 回発行する。
interactive / child / maximize / programmatic resize に共通する信頼できる終了イベントがないためで、
失敗を再試行する timer ではなく入力 stream の終端判定である。

主用途である全画面再生ではそもそもリサイズが起きないので、常用経路では差し替えが一度も
発生しない。

### 2.2.1 表示 surface の上限

Phase A は長辺 8192px、総画素数 4096×4096 = 16,777,216px を上限とする。4K の表示矩形は
入るが、8K UHD (7680×4320 = 33,177,600px) は `OS に任せる` へ fallback する。これは 3-buffer
BGRA swap chain、Lanczos の中間 RT、grade RT、retired surface、共有 frame が同時に存在する際の
VRAM spike と allocation failure を抑える保守的な固定 backstop であり、画質上の境界ではない。

reported VRAM へ単純比例させない。Windows の adapter memory は共有・他 process 使用・budget 変動を
含み、搭載量だけでは現在の安全な確保量にならない。将来広げるなら、現在の DXGI budget と既存
mIV resource を計測し、固定 reserve と絶対上限を併用する policy を実機 telemetry で決める。
Phase A では比較データがないため上限値を変更しない。
### 2.3 却下した案

| 案 | 内容 | 却下理由 |
| --- | --- | --- |
| ウィンドウ解像度サーフェス | swap chain をウィンドウ全体サイズにする | 黒帯まで描く。表示矩形サイズにすれば同じ制御力で無駄がない (= §2 が上位互換) |
| 整数倍サーフェス | 動画解像度 × 2 | §2.1。中間倍率で情報を捨てる |
| デコーダ側 blit で拡大 | `blit_nv12_to_rgba` の出力を最初から拡大する | 共有テクスチャプールが 16 枚あるため VRAM が 4 倍 (1080p で 530MB)。`frame.width/height` が実解像度と食い違い、SAR・HUD の解像度表示・スクリーンショット・幾何計算が全部嘘をつく。推論コストがデコードスレッドに乗り直列化する |

---

## 3. Phase 分割

**Phase A と Phase B は別リリースに分ける。** Phase A は安く全解像度で使え、構造がここで
確定する。Phase B は重く、性能の実測が前提になる。

### Phase A — 標準 / シャープ / ニアレスト + 縮小

| 項目 | 内容 |
| --- | --- |
| 標準 | 拡大・縮小とも Lanczos3。静止画の `PostFilter::None` と同じ考え方 |
| シャープ | 拡大に NVIDIA Image Scaling。縮小は Lanczos3 |
| ニアレスト | 拡大 NEAREST、縮小 Lanczos3 (静止画と同じ規則) |
| 物理等倍 | リサンプルせず元テクスチャを直接コピー。**今の `CopySubresourceRegion` がまさにそれ**なので、1.0 倍は現状動作を維持する |
| OS に任せる | 従来動作 (DComp 任せ)。GPU 固有の問題が出たときの退避先として明示的に選べるようにする |

**縮小が動画では新規の利点になる。** 4K 動画を 1080p ウィンドウで見るときのモアレは現在
DWM 任せで抑えられていない。静止画で v2.11.0 に入れた縮小 Lanczos の動画版にあたる。

「縮小時のなめらかさ」は静止画と**別設定**にする (負荷特性と素材傾向が違うため)。

### Phase B — Anime4K

Phase A の構造をそのまま使う。§4 以降が対象。

---

## 4. Anime4K の変種と、実装の一般化

### 4.1 変種の実測構成

配布シェーダ (`Anime4K_Upscale_CNN_x2_*.glsl`) から数えた実際の構成:

| 版 | 入力層 | 中段 3×3 conv | 特徴ch | 1×1 統合 | 中間テクスチャ | 演算量(VL比) | 実測コスト | 中間VRAM(1080p) |
| --- | ---: | --- | ---: | --- | ---: | ---: | ---: | ---: |
| S | 1 | 3 層 | 8 | なし | 4 枚 | 11% | 未測定 | 66 MB |
| M | 1 | 6 層 | 8 | 1 (56ch) | 8 枚 | 24% | +0.77 ms | 133 MB |
| L | 2 | 7 層 | 16 | なし | 9 枚 | 50% | 未測定 | 149 MB |
| VL | 2 | 12 層 | 16 | 3 (112ch) | 17 枚 | 100% | +1.72 ms | 282 MB |
| UL | 3 | 18 層 | 24 | 3 (120ch) | 24 枚 | 204% | 未測定 | 398 MB |

実測コストは [upscale-algorithm-selection.md](upscale-algorithm-selection.md) §4.6 の
512²→2048² (4MP 出力 / RTX 4090 / libplacebo) における bilinear からの増分。

**演算量比と実測比は一致しない** (M は机上 24% に対し実測 45%)。最終 Depth-to-Space パスが
全版共通で固定費として乗るためである。**したがって机上比で性能を予測してはならない** (§5)。

品質差は SSIM 平均順位で VL 2.79 / M 3.19 (14 候補中)。**両者の差より、両者と 3 位以下の
差の方が大きい**。動いている絵ではこの差はさらに見えにくい。

### 4.2 変種トポロジーの生成 (B1 実装済み)

[convert_anime4k_glsl_to_wgsl.py](../scripts/convert_anime4k_glsl_to_wgsl.py) は
S / M / L / VL / UL の各 GLSL について `//!SAVE` と `//!BIND` を解析し、次を一括生成する。

- 変種ごとの WGSL。各ファイルは入力 GLSL の MIT ライセンスヘッダーを保持する
- [gpu_anime4k_generated.rs](../src/gpu_anime4k_generated.rs) の変種別 const データ:
  shader、最大入力 binding 数、各 convolution pass と最終 resolve pass の入力リスト
- 入力リストは `Source` または「何番目の中間出力か」で表し、未知・未来の `//!BIND`、
  重複 `//!SAVE`、未対応 resolve 形状は生成エラーにする

[gpu_anime4k.rs](../src/gpu_anime4k.rs) は `Anime4kVariant` を値として受け取り、生成データから
pipeline 数、中間 texture 数、bind group layout、pass ごとの texture view 配線を構築する。
VL 固有だった `INTERMEDIATE_COUNT` / `INPUT_BINDING_COUNT` と pass 番号分岐は無くなった。
静止画経路の選択値は引き続き VL 固定で、表示結果は変更しない。

[test_convert_anime4k_glsl_to_wgsl.py](../scripts/test_convert_anime4k_glsl_to_wgsl.py) は、
一般化した converter による VL 再生成結果と committed `gpu_anime4k.wgsl` の完全一致を
golden test にし、5 変種すべての shader / topology も committed 生成物と比較する。
HLSL 出力と native video presenter への接続は B2、計測と選択 policy は B3 で実装した。

### 4.3 HLSL 生成 (B2 / B3 実装済み)

presenter は wgpu ではなく生の D3D11 なので WGSL はそのまま使えないが、**手書き移植は
持たない**。[build.rs](../build.rs) が B1 で生成した S / M / L / VL / UL の WGSL を Naga の
HLSL backend へ通し、全 fragment entry point を Shader Model 5 HLSL へ変換する。Naga が
行列の `mul()` 順と `textureLoad` の `Texture2D.Load` lowering を担うため、係数とアルゴリズムの
正本は WGSL の一組だけである。

Windows target の build では、変換した各 entry point を Windows SDK の `fxc` で `ps_5_0`
bytecode にし、生成した variant table から `include_bytes!` する。`fxc` が無い場合は SDK の
導入または `MIV_FXC_PATH` の設定を示して build を失敗させる。非 Windows target は HLSL
生成まで行うが `fxc` を起動せず、Ubuntu CI を Windows SDK へ依存させない。

実装上の注意:

- VL は texture が `t0`〜`t13`、UL は `t0`〜`t14` まで必要になる。WGSL binding 番号を
  uniform の HLSL register にもそのまま使うと VL が `b14` となり、SM5 pixel shader の
  `b0`〜`b13` 上限を超える。texture と constant buffer は別 register 空間なので、Naga の
  binding map で Anime4K params だけ `b0` へ割り当てる
- bytecode 化するのは pixel shader だけ。全 pass の vertex shader は Phase A で実機確認した
  native D3D fullscreen vertex shader を使う。Naga 生成 `vs_main` は WebGPU 向きの winding の
  ため D3D11 default rasterizer に cull される
- D3D11 側も B1 の生成 topology を読み、pass 数、中間 texture 数、各 `tN` の入力を決める。
  VL 固有の pass 番号分岐は持たない
- 静止画にある可視領域の切り出し (`RECEPTIVE_MARGIN` / `process_origin`) は、動画では常に
  フレーム全体 (`process_origin = 0`) とする。orientation は最終 resolve の inverse mapping で
  正規化し、追加の target-size copy は持たない

B3 では S / L / UL を GPU timestamp で測り、M / VL を演算量で内挿する。自動プリセットは
フレーム時間予算と中間画像のメモリ予算をともに満たす最大モデルを選び、固定プリセットは
メモリ・ソース上限だけを守って指定モデルを選ぶ。

---

## 5. 測定とモデル選択

### 5.1 なぜ測るのか

動画で効く制約は静止画と入れ替わる。

| | 効く制約 | 正しいつまみ |
| --- | --- | --- |
| 静止画 | **面積** (中間テクスチャの VRAM)。1 回作ればキャッシュされる | ソース上限 → 標準拡大へ退避 |
| 動画 | **時間** (1 フレームの締切)。毎フレーム再計算、キャッシュ不可 | **モデルの選択** |

そして GPU 性能は 4090 とノート iGPU で 10 倍以上違う。**設計時に固定しきい値を決めても
大半の利用者に対して外れる**。解像度で切り替える案も検討したが、フレームレートを見ていない
(720p60 は 1080p24 と同じくらい重い) うえ GPU を見ていないので採らない。

なお mIV には「実行時状態で動作を変えない」方針があるが、それが禁じたのは
**実行時状態で動く / 動かないが変わる**ことである。締切のある処理で締切を守るために品質を
選ぶのは別問題として扱う。加えて、下記のとおり**再生中は一切変えない**ので、
1 本の再生中の挙動は決定的である。

### 5.2 何を測るか

**モデル × ソースサイズの表を作る。**

- サイズは **960×540 と 1920×1080 の 2 点**とする (コストはソース画素数にほぼ線形)。
  出力はそれぞれ 1920×1080 と 3840×2160 とし、測定結果にも両寸法を記録する
- **2160p は測らない**。VL で中間 1.13GB、UL で 1.59GB になり中級 GPU では確保できない。
  4K ソースを更に拡大する場面 (= 4K 超のディスプレイ) は稀で、Anime4K の値打ちも最小。
  **総画素数が 1920×1080 を超えるソースは標準拡大へ落とす**。長辺だけの判定にしないため、
  縦長・横長動画も同じメモリ上限になる
- モデルは **S / L / UL の 3 つを実測し、M / VL は内挿**でよい (実測点に挟まれる)。
  組数は 3×2 = 6

計測は `ID3D11Query` (`TIMESTAMP` + `TIMESTAMP_DISJOINT`) でアップスケールパスだけを測る。
フレーム時間ではデコード・他アプリ・コンポジタが混ざる。

**各組で最初の 3 回を捨て、次の 7 回の中央値を採る。** 初回描画では
ドライバの最終コンパイル・テクスチャ初回タッチ・GPU クロックの立ち上がりが乗り、
2〜3 倍悲観的に出る。しかもシェーダ本数が多い大きいモデルほどこの偏りが大きいので、
**1 フレーム比較は大きいモデルを構造的に不利に判定する** (ノイズではなく系統誤差)。

出力サイズにも依存するため、**測定時の出力サイズを記録**する。現実装は表示解像度の代表点
(2倍拡大) で測り、ソース画素数に対して線形補間する。表示サイズを独立変数にした3変数回帰は、
実機比較で代表点からの誤差が問題になった場合の拡張とする。

### 5.3 いつ測るか

**利用者が Anime4K を選んだ瞬間**に測る。再生開始のたびに測ると毎回 0.5 秒待たされて煩わしく、
かつ再生開始直後は最も測定条件が悪い。

- **再生中に選んだ** → その場で測定する。デコードとの GPU 競合込みで測れるので実条件に近い
- **再生外 (環境設定など) で選んだ** → 保留。「未測定」と表示し、次に動画を再生した時点で
  測定する。この 1 本だけは測定中を標準拡大で再生し、完了後に切り替わる

**測定中も再生を止めない。**

- 計測全体を**専用ワーカースレッド**へ置く。再生用とは別の `ID3D11Device` /
  immediate context を同じ adapter 上に作り、GPU thread priority を最低値へ下げる
- 計測は**オフスクリーン**で行い、画面には現在の選択を出し続ける。でないと測定中に
  S → L → UL と画質が階段状に変わるのが見える
- 再生 render thread は query 完了を待たず、測定 worker の progress channel を非同期に読む。
  再生位置・clock・decode queue は触らない
- パネルに「測定中 (3/6)」を出す

### 5.4 キャッシュと無効化

**測定結果は永続化する。** 起動のたびに測り直すのは煩わしい。

- キーに **adapter LUID / vendor / device / subsystem / revision / driver version /
  dedicated・shared memory / description** を含める
- キーが一致しなければ破棄して再測定
- 記録するのは「モデル × ソース画素数 → GPU 時間」と、測定時の出力サイズ

**永続化には既知の弱点がある**: 他アプリが重いときに測った悲観的な値が起動をまたいで
焼き付き、本人には分からない。これは**再測定ボタンを回復手段として明示する**ことで受ける。
自動での再測定・自動降格は入れない (§5.6)。

### 5.5 予算プリセット

予算の分母は**動画のフレーム間隔** (1/fps)。表示リフレッシュではない。アップスケールは
デコードされたフレームごとに 1 回走る。

| 表示 | 予算 | 説明の方向 |
| --- | ---: | --- |
| 速度優先 | 20% | 他のアプリを使いながらでも安定して再生します |
| 標準 | 40% | 通常の使い方向けの既定です |
| 画質優先 | 60% | 動画の再生に専念する前提で、最も高品質なモデルを選びます |
| 固定 (S/M/L/VL/UL) | — | 測定せず指定したモデルを使います |

20 / 40 / 60% を採用する。B3 開発機の GPU timestamp 中央値は
540p→1080p が S=0.055ms / L=0.163ms / UL=0.462ms、1080p→4K が
S=0.189ms / L=0.606ms / UL=2.213ms だった。高速な1台の絶対時間を閾値にはせず、
標準40%で decode・compositor・一時的な競合へ60%を残す。速度優先はその余裕を80%へ、
画質優先は40%へ振る。

**% は UI に出さない。** 予算の余裕とは「他アプリが一時的に暴れたときの吸収代」なので、
利用者にはその言葉で説明する。デコードも同じ GPU を使うため、80% のような値は実質
オーバーコミットになる。

「固定」を残すのは、決定性を重視する使い方を壊さないため。実装コストはほぼゼロ。

### 5.6 再生中は変えない

**選択したモデルは 1 本の再生中に変更しない。** 自動降格も自動昇格も入れない。

理由:

- 素朴なフィードバック制御はハンチングする (他アプリ負荷・サーマル・シーン複雑度が全部入る)。
  数秒ごとに画質が揺れる
- 切り替えのたびにシェーダとテクスチャを作り直すので、§6 の不変条件を再生中に何度も破る

負荷が変わった場合の回復手段は**再測定ボタン**である。サーマルスロットリングや電源プロファイル
変更も同じ扱いにする。自動で再評価するのは新しい動画の source 寸法 / fps が確定した時だけで、
同じ動画の再生中に負荷を監視して自動昇降格はしない。予算変更と再測定は利用者の明示操作なので、
完了後に prepare / commit 境界で切り替える。

---

## 6. 切替時に固まらせないための不変条件

**フィルタやモデルを切り替える瞬間に、シェーダのコンパイルもテクスチャ確保も一切しない。**

これを守れば切替で映像は途切れない。サーフェス差し替え自体は既に「新 swap chain に正しい
フレームを Present してから `SetContent` + `SetTransform2` + `Commit` を 1 回で原子的に行う」
設計になっており ([present_with_surface_swap](../src/video/native_presenter/render_core.rs))、
連続再生で解像度の違う動画へ移るたびに毎回走っている実績のある経路である。音声は別スレッド・
別クロックなので影響を受けない。

守らなかった場合に起きること: Phase A の小さい shader は render pipeline 作成時に
`D3DCompile` できるが、Anime4K UL は 25 pass + 中間 texture 24 枚である。同じ作りにすると
**設定を触った瞬間に数百 ms〜秒単位で固まる**。

具体的な要求:

- Anime4K のシェーダはビルド時 `fxc` でバイトコード化 (§4.3)
- 中間テクスチャは切替前に準備し、完成してから原子的に有効化する。B3 は Phase A と同じ
  render-thread prepare / allocation-free commit 境界へ選択 variant を載せる。候補の全枚数を
  prepare できた後だけ commit を発行し、成功時に旧 variant の中間画像を解放する
- 一時停止中の切り替えも即反映する。既存の `SetVideoGrade` が持つ「Visible なら 1 回だけ
  再提示」の仕組みをそのまま使う (`FramePresentationState`)

Phase A の全シェーダは `NativeRenderCore` 作成時にコンパイルする。切替は prepare / commit の
2 段階で、D3D resource の owner である render thread 上の prepare が現在の source / target 寸法用の
中間 texture と、必要なら visual 未接続の次 surface を作る。寸法を含む request signature が一致した
ときだけ App が commit command を publish し、commit と一時停止中の held frame 再提示には
コンパイルも確保も残さない。prepare 後に
geometry が変わった request は stale として破棄し、古い surface を接続しない。
Anime4K は全 variant の pixel shader を build 時に bytecode 化し、B3 では全67 shader object
を `NativeRenderCore` 作成時に bytecode からロードする。runtime compile はしない。
同じ開発機の WARP 測定では VL 18本の runtime compile が713.8ms、VL bytecode loadが5.0ms、
全5 variantのbytecode loadが16.8msだったため、shader objectは全variantを常備できる。
設定 prepare は現在の source 寸法について選択 variant 1つ分の intermediate だけを完成させ、
commit は既存 resource の選択だけを行う。1080p source の中間画像は S / M / L / VL / UL の順に
約63 / 127 / 142 / 269 / 380 MiB で、同時保持しない。VRAM安全予算は adapter が報告する
dedicated / shared memory の大きい方の25%とする。

残る 1 つの避けられないコスト: 差し替え時の `WaitForCommitCompletion()` + `DwmFlush()` で
レンダースレッドが最大 1 コンポジタ tick (8〜16ms) 止まる。切替の瞬間だけで、既に
`commit_sync_ms` としてログに出ている。

---

## 7. UI

### 7.1 置き場所

**動画フルスクリーンの左パネル →「画像補正」→「フィルタ」タブ**
([overlay_draw.rs](../src/video/native_presenter/overlay_draw.rs) の
`NativeVideoAdjustmentTab::Filter`、現在は Creative LUT を置いている)。

- 拡大方法 (OS に任せる / 標準 / ニアレスト / シャープ / アニメ塗り)
- Anime4K を選んだときだけ、予算プリセットと**再測定ボタン**を同じ場所に出す
- 現在選ばれているモデルと根拠 (`Anime4K L / 予測 6.2ms / 予算 16.7ms`) をその場に表示する
- 測定中は「測定中 (3/6)」

**右パネル (メタ情報) には置かない。** メタ情報は下の方になりがちで、設定を触る場所と
離れてしまう。静止画側も「サイズ制限で実行されない」旨を**設定箇所そのものに**出しており
([ui_adjustment_panel.rs](../src/ui_adjustment_panel.rs) の
`crate::ui_helpers::processing_size_outside_note`)、その慣習に合わせる。

### 7.2 制限に当たったときの表示

ソースが測定上限を超えて標準拡大へ落ちる場合、静止画と同じ helper の書式で選択肢の直下に出す:

> （Anime4K は処理対象サイズ 総画素数 2073600px 以下の範囲外なので実行されません）

`processing_size_outside_note_for` で静止画側と同じ書式を共有する。

### 7.3 キー操作

CLAUDE.md の方針どおり `KeyAction` に足す:

- 動画の拡大方法の切り替え (`VideoScaleFilterNext`、既定 T)
- **アップスケール品質の再測定** (`VideoAnime4kRemeasure`、既定割り当てなし)

`ini_name()` / `context()` / `trigger()` / `default_chords()` / `ALL_ACTIONS` / 呼び出し側
helper / [keymap.ini.default](keymap.ini.default) を揃える。

---

## 8. 性能見積もり (外挿。実測が要る)

§4.1 の実測値をテクスチャフェッチ会計で動画サイズへ**外挿**した概算:

| 入力 | 出力 | RTX 4090 概算 (VL) | 中級 GPU 概算 (VL) |
| --- | --- | ---: | ---: |
| 480p | 960p | 約 2 ms | 約 6 ms |
| 720p | 1440p | 約 4 ms | 約 13 ms |
| 1080p | 2160p | 約 9 ms | 約 28 ms |

Lanczos3 は 4K 出力で約 +0.4ms、NIS は約 +0.9ms。**Phase A は実質タダで、重いのは
Anime4K だけ**である。

**この表は実測ではない。** 元の 1.72ms は libplacebo の最適化実装で測った値であり、
mIV の実装 (1 パス = 1 フラグメントシェーダ、中間をフル解像度で保持) はもっと重い可能性が
高い。[upscale-algorithm-selection.md](upscale-algorithm-selection.md) §1 が記録している
「1 つの測定で順位を決めて 2 回間違えた」のと同じ轍を踏まないため、**この数字を根拠に
既定値やしきい値を決めない**。

§5 の測定機構は、まさに「1080p VL が現実的かどうか」を**利用者ごとに自動で答える**ための
ものである。開発側でしきい値を当てにいく必要はない。

VRAM は §4.1 のとおり。detached で動画を 2 面同時再生することはない (動画は常に 1 つ) ので、
予算の分割は考えない。静止画側の Anime4K とは一時的に競合しうるが、ズーム操作の瞬間だけの
非定常負荷なので、測定は中央値で吸収し、再生中は「他アプリの負荷」と同じ扱いにする。

---

## 9. 検証項目

### 9.1 自動テスト

- 目標サーフェスサイズの決定 (**純関数**): フィルタ / 表示倍率 / 上限 / リサイズ中の各場合
- モデル選択 (**純関数**): 測定表 / 予算 / ソース画素数 / fps → 変種。GPU 不要
- `compute_video_visual_transform` の既存テストに、表示解像度サーフェス時 (M11=M22=1) を追加
- HLSL がシェーダモデル 5 でコンパイルできること
  (`grade_hlsl_compiles_for_shader_model_5` と同じ形)
- 生成した HLSL と既存 WGSL が同じ係数を持つこと (コンバータのゴールデンテスト)

### 9.2 実機

- 再生中のフィルタ切替 / モデル切替で映像が途切れないこと (§6)
- ウィンドウリサイズ中に swap chain が差し替わらないこと、静止後に 1 回だけ差し替わること
- ウィンドウ ⇔ 全画面、detached、動画→音声モード、VST GUI 表示中
- シーク、連続再生でのファイル切替、解像度が変わるストリーム
- 一時停止中の切替が即反映されること
- マルチモニタ / DPI 違い / リフレッシュレート違いへの移動
- 低 VRAM 環境で上限に当たったときのフォールバック表示
- 測定中に再生が止まらないこと、測定表示が出ること
- 再測定ボタンで再生位置が保たれること
- 4K 動画を 1080p ウィンドウで見たときの縮小品質 (Phase A の主な利点)

### 9.3 計装

- perf event: パスの GPU 時間、選択されたモデル、測定結果、フォールバック件数、
  サーフェス差し替え回数
- `emit_vram_trace` に中間テクスチャ確保を含める
- idle health check (静止中に再測定ループが回っていないこと)

---

## 10. 実装順と規模

| 順 | 内容 | 規模 |
| --- | --- | ---: |
| A-1 | 表示解像度サーフェス構造 + リサイズ状態機械 + 物理等倍の維持 | 400〜600 行 |
| A-2 | Lanczos3 / NIS / ニアレストの HLSL + 縮小 | 400〜600 行 |
| A-3 | 設定・UI・キー・計装・フォールバック | 300〜400 行 |
| B-1 | Anime4K 実装の一般化 (コンバータ表駆動化、静止画側にも波及) | 1〜2 日 |
| B-2 | **実装済み**: Naga HLSL + build-time bytecode + VL 固定 D3D11 多段パイプライン | 600〜800 行 |
| B-3 | **実装済み**: GPU timestamp / offscreen worker / 永続キャッシュ / 純関数選択 / UI | 900〜1200 行 |

Phase A で約 2 週間、Phase B で約 2 週間 (いずれも実機往復を含まない)。
**Phase A は測定を待たずに着手してよい。**

Phase A の A-1〜A-3 は完了した。NIS / nearest、動画専用の縮小なめらかさ、左パネル、
keymap、prepare / commit 切替、typed fallback と基礎計装まで実装済みである。

---

## 10.4 実測 (2026-08-22、利用者実機 RTX 4090)

perf log の `fullscreen_present` を `video_scale_filter_selected` で区間分けして集計した。
出力は 3652x2054。`copy_ms` の 12ms 台の土台は `OS に任せる` でも同じなので vsync /
keyed mutex 待ちが畳まれたものであり、フィルタの実費は差分側に出る。

**拡大 (960x540 / 480x270 → 3652x2054) を含む全区間:**

| フィルタ | n | copy_ms 中央値 | OS 任せとの差 |
| --- | ---: | ---: | ---: |
| OS に任せる | 328 | 12.268 | — |
| 標準 | 286 | 12.403 | +0.14 |
| ニアレスト | 202 | 12.441 | +0.17 |
| シャープ | 870 | 12.805 | +0.54 |

**縮小のみ (3840x2160 → 3652x2054):**

| フィルタ | copy_ms 中央値 | fps 中央値 | late_drop |
| --- | ---: | ---: | ---: |
| OS に任せる | 0.386 | 23.37 | 0 |
| 標準 | 0.720 | 23.75 | 0 |
| ニアレスト | 0.529 | 23.87 | 0 |
| シャープ | 0.758 | 23.90 | 0 |

縮小では 4 方式とも 24fps 素材をフルレートで再生し、ドロップは全区間 0 だった。
**標準とシャープの差は縮小では消える。**`select_video_resample_mode` が NIS / nearest に
`downscaling == false` を要求しており、縮小時は 3 方式とも Lanczos3 に落ちるためで、
実測もそれを裏付けている。したがって「高解像度の動画ほどシャープが重い」ことは起きない。
NIS が走るのは拡大時、つまり低解像度素材を大きく映すときだけで、コストの上限は出力解像度
= 画面サイズで決まる。

**この測定を根拠に既定を `標準` にした** (2026-08-22、利用者判断)。縮小時のモアレ低減が
DComp 任せに対する明確な利得で、コストがフレーム落ちを生まないため。`シャープ` を既定に
しなかったのは、効果が拡大時に限られ、素材依存 (文字・線画では有利、フィルムグレインでは
過処理に見える) で、静止画側の既定 `標準（補間あり）` と揃わないため。

### Phase A 方式の性能フォールバック判断

Phase A の4方式には B3 の時間測定フォールバックを広げない。Anime4K は最小Sでも11層かつ
source-resolution中間4枚を使い、モデルを下げる品質段階がある。一方 Phase A は1〜2 passで、
標準はAnime4Kが予算外のときにも使う常時利用可能な退避先である。標準まで測定待ち・自動無効化
にすると既定表示と退避先が同時に不安定になる。上の実測で表示解像度4Kでも追加中央値が
0.14〜0.54ms、late drop 0だったことから、Phase A は既存の typed なサイズ・確保失敗
fallbackと利用者が選べる「OS に任せる」を維持する。低速GPUで問題が観測された場合は、
Anime4Kのsource画素モデルを流用せず、出力画素数を軸にした別測定として追加する。

### リリース時にやること

既定変更なので [version_highlights.rs](../src/version_highlights.rs) の `TABLE` へ
「重要な変更点」を 1 件足す (`must_read`)。**版番号が決まっていないので未記入**。
更新後の初回起動で動画の見え方が変わることを利用者へ伝える文にする。

---

## 10.5 master の §1.101 (上下バー固定) と共有する seam — 解決済み

**解決前は表示矩形を決める場所が 2 つあった。** Phase A は swap chain を表示矩形の物理ピクセル
サイズにするため、[surface_policy.rs](../src/video/native_presenter/surface_policy.rs) の
`decide_video_surface_size` が表示矩形を自前で求めていた (VST の `compact` は viewport を
縦横 1/2 にする形で `compute_video_visual_transform` 側の 1/4 領域規則を写していた)。

master 側の §1.101 改訂 2 は「バーの領域を確保して映像をその手前までフィットさせる」方針で、
[briefs/video-hud-pinning.md](briefs/video-hud-pinning.md) §3.2 が
`compute_video_visual_transform` の `(target_x, target_y, target_w, target_h)` を同じ seam として
名指ししている。

**片方だけをマージすると静かに壊れる。** バー領域の確保が `compute_video_visual_transform`
側だけに入ると、映像は縮んだ矩形へフィットする一方、サーフェスはバー領域を含んだ大きさで
作られる。シェーダが解決する解像度が実際の表示サイズとずれ、Phase A の利点が消える
(無駄に大きく作って DComp が縮める)。テキスト競合は起きないので、マージだけでは気付けない。

**解決済み**: master が切り出した `compute_video_visual_target_rect` を `pub(super)` の
共通 seam とし、`compute_video_visual_transform` と `decide_video_surface_size` の両方が
`VideoVisualLayout` (compact、物理 pixel 倍率、上下バー固定、固定余白) とともに読む。
surface policy にあった viewport の独自 1/2 導出は削除した。top のみ、bottom のみ、両方を
固定したとき、display-resolution surface の高さが予約した物理 pixel 分だけ縮む回帰テストで
この一致を固定している。

---

## 10.6 出荷方針 (2026-08-22、利用者判断)

- **Phase A だけでは出さない。** 静止画側が既に Anime4K を持っているので、動画が「拡大は
  対応したが Anime4K は無い」状態で出ると機能として半端に見える。Phase A + Anime4K を
  1 つの改善としてまとめて出す。§3 の「Phase A と Phase B は別リリースに分ける」は
  この判断で上書きされた。
- **動画アップスケールは利用者要望ではない。** 急ぐ理由が無いので安定性を優先する。
  検証に不安が残るなら、締切に合わせず次版へ送る。動画再生は既に安定して動いている
  機能であり、要望されていない機能のためにそこへリスクを持ち込まない。
- 当初は v3.2.0 (2026-08-23 予定) を狙ったが、間に合わなければ v3.3.0 とする。

---

## 11. 未決事項

予算プリセットは20/40/60%、測定点は540p/1080p、測定上限は総画素数1920×1080、
中間画像のVRAM予算は報告メモリの25%に確定した。残る未決事項:

- 効果フィルタ (CRT / セピア等) を動画にも入れるか。入れる場合、色調 (grade) パスとの
  適用順序を決める必要がある。**Phase A / B の範囲外**とし、別途判断する
- Anime4K の Restore / Denoise 段を将来入れるか (現状は Upscale x2 の 1 段のみ)

---

## 12. 更新義務

実装時に同時更新する:

- [video-architecture.md](video-architecture.md) — GPU / CPU フレームの内部フロー図、
  swap chain のサイズ規則
- [display-pipeline.md](display-pipeline.md) — 動画側の変換適用ポイント
- [keymap-spec.md](keymap-spec.md) / [keymap.ini.default](keymap.ini.default) — 追加キー操作
- [upscale-algorithm-selection.md](upscale-algorithm-selection.md) §6 — 「動画は §1.47 の
  別案件」の記述を実装済みへ
- `htdocs/mimageviewer/manual/` と `htdocs/mimageviewer/index.html` — 内部用語・
  バージョン表記を出さない方針で記述する
