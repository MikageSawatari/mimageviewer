# トーン漫画の縮小モアレ対策 / GPU mipmap 実装

**ステータス: v2.10.0 までの履歴資料。通常静止画の最終表示方式と設定は v2.11.0 計画で置き換え済み。**

> [!IMPORTANT]
> 現在の正本は [dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md)。
> 通常静止画の縮小表示は表示サイズへの Lanczos3 直接リサンプルへ移行し、ここで設計した
> trilinear の ON/OFF と LOD bias は削除した。v2.10.0 の旧設定値は読込時に無視する。
> GPU mipmap generator は削除せず、wipe/diff 比較表示と 360 度パノラマが使い続ける。

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
比較準備のCPU payloadも描画経路別に分け、Windowsのwipe/diffはshader入力のRGBA 2枚、
PinnedNormalはpinned 1枚、CPU fallbackのwipeは2枚、diffは差分1枚だけを保持する。
途中cancel不能なcompare workerは`Preparing` / `Draining`で1本に直列化し、失効後も完了を回収するまで
次を開始しない。見開き表示中は固定解像度上限を設けず、現在ページ1枚だけを比較キャンバスにする。
画面解像度で描く360度パノラマのsettle overlayは1 mipのままとする。
360度パノラマは水平フル/垂直cropではU方向Repeat、水平cropではU方向ClampToEdgeの
bind groupを選び、低LODで部分画像の反対端が混ざらないようにする。
また、`atan2`の経度シームでUが1から0へ飛ぶ差を周期1でwrapした明示微分を
`textureSampleGrad`へ渡し、シーム付近だけ過度に粗いmipが選ばれることを防ぐ。

サムネイル、animated GIF/APNG/WebP の各 frame、動画、mask、checker、UI/font preview、
`PostFilter::Nearest` は opt-in しない。これにより小 texture や頻繁に更新する texture の
生成コストと VRAM 増加を避け、pixel-art の明示的な nearest 表示も維持する。

### 2.3 シャープ優先切替と調整可能な LOD 補正

画像補正パネルの「フィルタ」に、全表示共通のチェック
`縮小表示のモアレを抑制する` と、配下の `より強く抑制`（0.0〜1.5、0.1 刻み）を置く。

- 既定 ON / 0.0 は GPU の標準 LOD 選択を維持し、v2.7.0 以降の既定画質と同一にする。
- OFF は完全な mip chain を作ったまま、shader で `textureSampleLevel(..., 0.0)` 相当を使い
  level 0 固定で読む。従来の bilinear 1-mip 縮小に近いシャープ優先表示になる。
- ON かつ正値は従来の LOD bias と同じ。0.5 は約半 level、1.0 は 1 level 粗い mip へ
  寄せるため、値を上げるほどモアレは減る一方で細部が軟らかくなる。

managed 表示 texture と wipe/diff 比較 callback は明示フラグで `textureSampleLevel` と
`textureSampleBias` を切り替える。360度パノラマも同じフラグを持ち、ON では経度シーム補正済みの
explicit gradient を `2^bias` 倍して `textureSampleGrad` の level 選択を粗い側へ寄せ、OFF では
level 0 固定で読む。

ON/OFF と値は renderer / callback uniform だけをライブ更新する。texture や mip chain の
作り直し、再 upload、cache invalidation、CPU resize は発生しない。mipmap 非対象 texture、
`PostFilter::Nearest`、動画、サムネイルには表示上の影響を与えない。

## 3. 描画・キャッシュ不変条件

- mip level は 1 個の `wgpu::Texture` 内にあるため、`egui::TextureHandle::size_vec2()` は level 0
  の寸法を返し続ける。見開き、連結読み、ズーム、ルーペ、pixel grid の論理座標を変更しない。
- 表示 texture の優先順位と `edit -> color -> final AI -> smart sharpen -> post_filter` の
  合成順序を変更しない。
- texture cache の invalidation は従来どおり `TextureHandle` 単位。再 upload 時に mip chain も
  一緒に作り直されるため、LOD 専用の世代管理は追加しない。
- `PostFilter::Nearest` は level 0 + nearest sampler のままとし、意図したドット表示を守る。
- 連結読みの texel 予算は、mImageViewer 全体の VRAM pool をフルスクリーン中の表示系へ 80%
  配分し、RGBA8 の 4 byte/texel で割った値を HIGH、HIGH の 75% を LOW とする。raw static の
  完全な mip chain に加え、同時保持する erase、local-adjust（レイヤー比較 preview を含む）、
  conceal、edit、final composite、comic、補正 texture も `TextureId` で重複排除して実寸計上する。
  HIGH 超過時だけ非可視 keep-set を遠い側から外し、LOW 以下まで退去する。投機的な final-effect
  先読みは使用量が LOW 未満のときだけ許可する。ただし厳密可視の前後 1 ユニットの準備帯と
  可視ページは水位をバイパスし、texel trim の退去候補からも除外する。これらの編集 cache は
  keep-set eviction にも追従し、animated frame は従来どおり level 0 だけを数える。0% 設定では
  HIGH/LOW を設けず、この texel trim と投機的先読みの水位判定を無効化する。
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
- 既定 ON / 0.0 で従来の uniform bias 0.0 と `textureSampleBias` 経路を維持すること。
- OFF で level 0 固定分岐へ入り、線がくっきりする一方でモアレ / ちらつきが戻りやすいこと。
- ウィンドウ幅を連続的に変え、ON の 0.0 / 0.5 / 1.0 を比較でき、値を上げるとモアレが
  減って細部が段階的に軟らかくなること。
- ON/OFF と強度の変更で表示 texture の再 upload や cache invalidation が起きないこと。
- fit、見開き、縦横連結、ズーム往復、ルーペ、pixel grid の寸法と位置が変わらないこと。
- 補正、AI、消しゴム、隠蔽、注釈の結果更新後も古い mip level が残らないこと。
- Windowsのwipe/diff比較と360度パノラマを大縮小してもモアレが再発しないこと。
- wipe/diffで多数の高解像度画像を切り替えても、比較callbackのVRAMが過去組数に比例して
  増えず、比較解除後に現在組が解放されること。
- 360度パノラマの経度シームを画面中央へ置いても、シーム沿いだけ粗いmipによる縦線・ぼけが
  出ないこと。
- 水平cropされた部分パノラマを広角表示しても、欠落領域へ画像の反対端が混ざらないこと。
- 連結読みの texel 集計が完全な mip chain と後段表示 texture を含み、共有 pool 由来の HIGH 超過で LOW (HIGH の 75%) 以下まで退去すること。
- `Nearest`、animated image、動画、サムネイルの挙動が変わらないこと。
- 旧 `downscale2x` / `downscale4x` を含む保存設定が `None` でロードできること。

## 6. 参照

- `vendor/egui-wgpu/src/renderer.rs` — mip level allocation、sampler、upload 後の生成呼び出し
- `vendor/egui-wgpu/src/mipmap.rs` / `mipmap.wgsl` — GPU mip chain generator
- `src/app.rs` — `DISPLAY_IMAGE_TEXTURE_OPTIONS` と静止画 upload 経路
- `docs/display-pipeline.md` — 表示 texture の優先順位と合成順序
- `docs/preset-and-adjustment.md` — post-filter 仕様と旧設定の移行
