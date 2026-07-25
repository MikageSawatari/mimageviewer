# トーン漫画の縮小モアレ対策 / GPU mipmap 実装

**ステータス: mipmap + 調整可能な LOD 補正を実装済み、実機検証待ち (2026-07-25)。**

フルスクリーンの大縮小時に発生するモアレについて、原因、採用した実装、互換方針をまとめる。

## 1. 原因

- サムネイルは表示寸法に近いサイズへ Lanczos3 で縮小してからアップロードするため、GPU 側の
  追加縮小は小さい。
- 静止画フルスクリーンは最大 8192px の表示テクスチャを保持し、従来は 1 mip の bilinear
  sampling で画面へ縮小していた。縮小率が 0.5 を下回ると入力 texel を飛ばし、スクリーントーンの
  高周波が低周波へ折り返すことが主因だった。
- egui 0.33.3 の `TextureOptions` には `mipmap_mode` があるが、stock `egui-wgpu` 0.33.3 は
  `mip_level_count = 1` 固定で、sampler の `mipmap_filter` にも値を渡さない。
- 完全な mip chain を用意しても、GPU の標準 LOD は微分から最も近い 2 level を選ぶ。原稿の
  周期と中間縮小率の組み合わせによっては、標準 LOD で選ばれる level の box 平均だけでは
  高周波を十分に落とせず、ウィンドウを広げて少し拡大寄りにした境界でモアレが再発しうる。

## 2. 採用方式

`vendor/egui-wgpu` に、`TextureOptions::mipmap_mode` を尊重する最小パッチを置く。
アプリの `TextureHandle` / cache 構造と描画時の論理サイズは変更しない。

### 2.1 egui-wgpu 側

- `mipmap_mode` が `Some` の managed `Rgba8Unorm` texture だけ完全な mip chain を確保する。
- level 0 の upload 後、専用 render pipeline が level N-1 から N を順番に生成する。
- シェーダーは destination texel が覆う source texel の面積加重平均を取る。奇数寸法でも末尾の
  行・列を捨てず、各段を 1x1 まで生成する。
- sampler の `mipmap_filter` は `TextureFilter::{Nearest, Linear}` に追従する。
- `mipmap_mode = None` の texture view は level 0 だけを公開し、従来挙動を保つ。
- partial update を受けた mip texture は全下位 level を再生成する。
- 同じ `Rgba8Unorm` 生成器を公開APIとして、比較 callback と360度パノラマの独自
  wgpu textureでも共用する。

生成は `Queue::write_texture` と同じ queue に submit するため順序が保たれる。CPU resize や
追加 I/O は行わない。完全な mip chain の追加 VRAM は元 texture の約 1/3。

### 2.2 アプリ側の opt-in

`src/app.rs::DISPLAY_IMAGE_TEXTURE_OPTIONS` を次の表示用静止画に使う。

- `fs_cache` の通常画像 / ZIP 画像 / PDF ページ / static panorama
- edit、消しゴム、補正レイヤー、隠蔽、注釈、AI、final composite の表示 texture
- 比較表示の pinned/current/diff texture

Windowsのwipe/diff比較と360度パノラマはmanaged `TextureHandle`を使わず独自に
`Rgba8Unorm` textureを作るため、同じGPU生成器で完全なmip chainを構築し、trilinear samplingする。
wipe/diffのcallback resourceは現在の比較組（pinned/currentの2枚）だけを保持し、比較解除・
再準備時には旧組を新規確保前にdropする。右下のピン表示はpin workerで72x54以下へ縮小した
専用textureを使い、インジケーターだけのためにフル解像度mip chainを保持しない。
画面解像度で描く360度パノラマのsettle overlayは1 mipのままとする。
360度パノラマは水平フル/垂直cropではU方向Repeat、水平cropではU方向ClampToEdgeの
bind groupを選び、低LODで部分画像の反対端が混ざらないようにする。
また、`atan2`の経度シームでUが1から0へ飛ぶ差を周期1でwrapした明示微分を
`textureSampleGrad`へ渡し、シーム付近だけ過度に粗いmipが選ばれることを防ぐ。

サムネイル、animated GIF/APNG/WebP の各 frame、動画、mask、checker、UI/font preview、
`PostFilter::Nearest` は opt-in しない。これにより小 texture や頻繁に更新する texture の
生成コストと VRAM 増加を避け、pixel-art の明示的な nearest 表示も維持する。

### 2.3 調整可能な LOD 補正

画像補正パネルの「フィルタ」に、全表示共通の `LOD 補正` を 0.0〜1.5、0.1 刻みで置く。
既定 0.0 は GPU の標準 LOD 選択を維持する。0.5 は約半 level、1.0 は 1 level 粗い mip へ
寄せるため、標準選択で残る中間縮小率のモアレを原稿ごとに抑えられる。大きくするほど
モアレは減る一方で細部が軟らかくなる。

- managed 表示 texture: `egui.wgsl` の通常画像 sampling に `textureSampleBias` を使う。
- wipe/diff 比較 callback: 同じ bias を比較 uniform へ渡し、両画像へ適用する。
- 360度パノラマ: 経度シーム補正済みの explicit gradient を `2^bias` 倍し、
  `textureSampleGrad` の level 選択を同量だけ粗い側へ寄せる。

値は renderer uniform だけをライブ更新する。texture や mip chain の作り直し、cache
invalidation、CPU resize は発生しない。mipmap 非対象 texture、`PostFilter::Nearest`、
動画、サムネイルには適用しない。

## 3. 描画・キャッシュ不変条件

- mip level は 1 個の `wgpu::Texture` 内にあるため、`egui::TextureHandle::size_vec2()` は level 0
  の寸法を返し続ける。見開き、連結読み、ズーム、ルーペ、pixel grid の論理座標を変更しない。
- 表示 texture の優先順位と `edit -> color -> final AI -> smart sharpen -> post_filter` の
  合成順序を変更しない。
- texture cache の invalidation は従来どおり `TextureHandle` 単位。再 upload 時に mip chain も
  一緒に作り直されるため、LOD 専用の世代管理は追加しない。
- `PostFilter::Nearest` は level 0 + nearest sampler のままとし、意図したドット表示を守る。
- 連結読みの320M texel上限は、raw staticの完全なmip chainに加え、同時保持するerase、
  local-adjust（レイヤー比較previewを含む）、conceal、edit、final composite、comic、補正textureも
  TextureIdで重複排除して見積もる。これらの編集cacheはkeep-set evictionにも追従する。
  animated frameは従来どおりlevel 0だけを数える。
- 表示トリムは画像全体から生成したmipを部分UVで描く。強い縮小時は、切り落とした余白色が
  境界の低LOD texelへ混ざる可能性があるが、通常は画面上1〜2px程度であり、専用crop textureの
  キャッシュ複雑化を避けるためv2.7.0では既知制約として受容する。

## 4. 旧手動縮小フィルタの撤去

一時回避策だった `PostFilter::Downscale2x` / `Downscale4x` と、対応する UI、key action、
ゲームパッド項目、CPU Lanczos resize を削除した。保存済み JSON/DB の文字列
`downscale2x` / `downscale4x` は serde alias で `PostFilter::None` として読み込み、設定全体を
壊さず「フィルタなし」へフォールバックする。旧 key action は未知 action として既存の keymap
正規化経路で破棄される。

## 5. 検証項目

- GPU validation error なしで通常画像、ZIP 画像、PDF ページを開けること。
- 1/2 より大きい縮小率のトーン画像で、従来より周期的なモアレが減ること。
- ウィンドウ幅を連続的に変え、少し拡大寄りになる中間縮小率でも `LOD 補正` 0.0 / 0.5 /
  1.0 を比較でき、値を上げるとモアレが減って細部が段階的に軟らかくなること。
- `LOD 補正` の変更で表示 texture の再 upload や cache invalidation が起きないこと。
- fit、見開き、縦横連結、ズーム往復、ルーペ、pixel grid の寸法と位置が変わらないこと。
- 補正、AI、消しゴム、隠蔽、注釈の結果更新後も古い mip level が残らないこと。
- Windowsのwipe/diff比較と360度パノラマを大縮小してもモアレが再発しないこと。
- wipe/diffで多数の高解像度画像を切り替えても、比較callbackのVRAMが過去組数に比例して
  増えず、比較解除後に現在組が解放されること。
- 360度パノラマの経度シームを画面中央へ置いても、シーム沿いだけ粗いmipによる縦線・ぼけが
  出ないこと。
- 水平cropされた部分パノラマを広角表示しても、欠落領域へ画像の反対端が混ざらないこと。
- 連結読みの320M texel上限が完全なmip chainと後段表示textureを含むこと。
- `Nearest`、animated image、動画、サムネイルの挙動が変わらないこと。
- 旧 `downscale2x` / `downscale4x` を含む保存設定が `None` でロードできること。

## 6. 参照

- `vendor/egui-wgpu/src/renderer.rs` — mip level allocation、sampler、upload 後の生成呼び出し
- `vendor/egui-wgpu/src/mipmap.rs` / `mipmap.wgsl` — GPU mip chain generator
- `src/app.rs` — `DISPLAY_IMAGE_TEXTURE_OPTIONS` と静止画 upload 経路
- `docs/display-pipeline.md` — 表示 texture の優先順位と合成順序
- `docs/preset-and-adjustment.md` — post-filter 仕様と旧設定の移行
