# 360 度パノラマビュー機能 — 現行仕様と設計経緯

ChatGPT / DALL-E や 360 カメラ (Insta360 / RICOH Theta 等) が出力する
**equirectangular projection** の画像を、フルスクリーンでドラッグして自由視点で見られる
ようにする機能。マウスドラッグで yaw / pitch、ホイールで FOV (ズーム)、
リセットでデフォルト向きへ。

> **現況 (2026-07)**
>
> - **コードで実装確認済み**: GPano XMP と negative cache、2:1 fallback、部分 FOV UV、
>   `KeyAction::FsPanorama` の V キー、ホバーバー、WGSL、GPU upload、complete mip chain、
>   final composite 優先入力、修飾キー不問の wheel FOV、settle-refinement と高解像度 gate。
> - **lifecycle**: 通常ナビでは pose を保持し、非 360 ページで非アクティブ化する。明示 V/× の
>   OFF と fullscreen 退出では state を破棄する。360 入場時は slideshow を停止する。
> - **限定対応**: PDF は GPano XMP を抽出せず、2:1 fallback の 8K `BaseOnly` 表示のみ。
>   ZIP は XMP / base 表示に対応するが high-res settle は行わない。
> - **未実装 / 将来候補**: slideshow による連続 equirect pose 連動、慣性ドラッグ、
>   ミニコンパス、アニメーション、巨大 JPEG の部分デコード。
> - **360 動画は実装済み** (2026-08、backlog §1.112 の 3 段)。この冒頭は長く
>   「未実装」と書いたままだったが、同じ文書の §13.8 が実装済みの投影を書いている。
>   動画側の正本は backlog §1.112 と §13。
> - **手動検証**: 2K / 11K・12K 実素材、200 MP 超、実機性能の完了記録は確認できない。
>   §8 の手動チェックは未確認のまま残す。
>
> §2〜§7 と §13 は現行仕様を優先し、§8 の完了済み code checklist は実装記録、未チェックの
> manual checklist は未確認、§9〜§12 は明示ラベルどおり設計経緯 / 将来候補として読む。
> **§13 (投影方式) は動画側 (backlog §1.112) へ移す数式の正本**でもある。

---

## 1. 背景: equirectangular とは

緯度経度を 2D 画像に展開する標準投影方式:

- 画像幅 (X 軸) を経度 [-π, π] (360°)、画像高 (Y 軸) を緯度 [-π/2, π/2] (180°) に対応させる
- フル球面画像のアスペクト比は **2:1**。GPano `CroppedArea*` を持つ部分 FOV 画像は
  2:1 でない場合がある
- 中央 (赤道) が真っ直ぐ、上下 (極) で激しく横方向に歪む
- 画像の左端と右端は連続する (シームレスにラップ)

ChatGPT の出力は 1024×512 / 2048×1024 等の 2:1 PNG。実カメラの出力は
4096×2048 / 5760×2880 など。いずれも 8192 以下なので `MAX_TEXTURE_DIM` の
範囲内で 1 枚テクスチャに収まる。

---

## 2. 検出方法

### 2.1 アスペクト比

`FsCacheEntry::Static.source_dims` から **w/h が 1.95 ～ 2.05 の範囲なら equirectangular
候補** と判定する (`docs/display-pipeline.md §2.2` 参照)。`source_dims` は clamp 前の
原寸なので、wgpu の 8192 上限で縮められた後の `pixels.size` ではなくこちらを使う。

### 2.2 XMP メタデータ (GPano namespace)

Google が定義した [Photo Sphere XMP Metadata](https://developers.google.com/streetview/spherical-metadata) の
名前空間 `http://ns.google.com/photos/1.0/panorama/`:

| タグ | 意味 | mIV での扱い |
| --- | --- | --- |
| `GPano:ProjectionType` | `"equirectangular"` 等 | これがあれば確実 |
| `GPano:UsePanoramaViewer` | `"True"` ならビューア推奨 | 自動起動の判断 |
| `GPano:FullPanoWidthPixels` / `FullPanoHeightPixels` | 元解像度 | 部分パノラマの full sphere 寸法として使用 |
| `GPano:CroppedAreaImageWidthPixels` / `CroppedAreaImageHeightPixels` / `CroppedAreaLeftPixels` / `CroppedAreaTopPixels` | クロップ範囲 | `PanoUvTransform` で部分 FOV を補正 |
| `GPano:PosePitchDegrees` / `PoseRollDegrees` / `PoseHeadingDegrees` | 初期向き | 初期 yaw/pitch の hint |

`src/xmp_reader.rs` は **quick-xml** で XMP packet を一度抽出し、`xtw:` と GPano を
同じ metadata worker でパースする。単独 API の `read_panorama_info(path)` /
`read_panorama_info_from_bytes(bytes)` もあり、通常画像と ZIP 内画像の双方で利用できる。

### 2.3 判定の使い分け

| 条件 | 検出 | アクション |
| --- | --- | --- |
| GPano `ProjectionType=equirectangular` + `UsePanoramaViewer=True` | **`Auto`** (強シグナル) | 案内トースト「V キーで 360° ビューワー (XMP 検出)」 + ホバーバーに 360 ボタンを通常表示 |
| GPano `ProjectionType=equirectangular` のみ | **`Hint`** (中シグナル) | 案内トースト「V キーで 360° ビューワー」 + 360 ボタン表示 |
| アスペクト比 2:1 のみ (GPano なし、ChatGPT 出力の典型) | **`Hint`** (弱シグナル) | 同上 |
| それ以外 (アスペクト比が 2:1 範囲外 / 動画 / 見開き Double 中) | 検出なし | 360 ボタンは disabled 表示 (押せない) |

**自動 ON はしない方針** (フィードバック反映で廃止、機能制限モードへの強制遷移
は違和感が大きいため)。**ユーザーは V キーまたは 360 ボタンで明示的にトグル**する。
案内トーストは `open_fullscreen` / `poll_metadata_load` (XMP 到着) /
`poll_prefetch` (fs_cache 完了) のいずれか早いタイミングで一度だけ表示する
(`pano_toast_shown_for_current_fs` フラグで重複抑止)。

**アスペクト比 2:1 単独で検出する理由**: ChatGPT / DALL-E が出力する equirect
画像は GPano XMP を持たないため、アスペクト判定で拾わないと案内できない。
2:1 の通常パノラマ写真や横長壁紙を 360 候補と扱うリスクはあるが、自動 ON では
無くトースト案内 + ボタン任意クリックなので誤検出のコストは低い。

### 2.4 メタデータ取得経路

XMP は **`start_metadata_load` ワーカー側で**読む。
`src/xmp_reader.rs::read_tweet_info` と同じ並列度で GPano パーサを呼ぶ。

**ZIP 内画像対応**: 既存の `read_tweet_info` は `_from_bytes` 版を持つ
(`src/xmp_reader.rs::read_tweet_info_from_bytes` / `extract_xmp_packet`)。これに
揃えて 2 つの関数を用意する:

```rust
pub fn read_panorama_info(path: &Path) -> Option<XmpPanoramaInfo>;
pub fn read_panorama_info_from_bytes(bytes: &[u8]) -> Option<XmpPanoramaInfo>;
```

`start_metadata_load` 内で `GridItem::Image` / `Video` はパス版、`ZipImage` は
zip_loader でバイト取得 → bytes 版、`PdfPage` は対象外。既存の XMP 抽出処理 (= XML
packet 切り出し) は完全に共通化できる。

**結果の保持キー**: idx ベースの `HashMap<usize, ...>` は並び替え / 再スキャン /
仮想フォルダで idx がずれる脆さがあるため、**既存の `App::metadata_cache_key(idx)`
(`src/app.rs:12144`) と同じ String キー**を使う:

```rust
pub(crate) xmp_panorama_info: HashMap<String, Option<XmpPanoramaInfo>>,
// キー例:
//   Image:    "c:/foo/bar.jpg"                      (normalize_path 済み)
//   ZipImage: "c:/foo/bar.zip::内部/画像.jpg"        (entry_name lowercase)
//   PdfPage:  GPano XMP は対象外。fs_cache source_dims の 2:1 fallback は利用可能
```

内側の `None` は「metadata load 済みだが GPano property なし」という negative cache。

UI スレッドからは `metadata_cache_key(idx)` でキーを解決して引く。並び替えや
items_generation バンプの影響を受けない。

UI スレッドからの同期 I/O は禁止 (CLAUDE.md「UI スレッドでの同期 I/O は即 worker
化する」)。

---

## 3. 描画アルゴリズム

### 3.1 方式比較

| 方式 | Pros | Cons | 採用 |
| --- | --- | --- | --- |
| (A) 内側スフィアメッシュ + テクスチャマッピング | OpenGL 系チュートリアルの定番 | 頂点バッファ管理、極の三角形密度問題、コードが膨らむ | ✗ |
| **(B) フルスクリーン三角形 + フラグメントシェーダで逆射影** | コード最小、`compare_wgpu.rs` とほぼ同じ構造、極の歪みも自然 | per-pixel に `atan2`/`asin` (現代 GPU は無問題) | **✓ 採用** |
| (C) cubemap 変換してから cubemap サンプリング | 標準的、ミップマップ品質良好 | 初回変換コスト (6 面アップロード)、メモリ 6× | 不要 |

**方式 (B) を採用**。`src/compare_wgpu.rs` が既にフルスクリーン quad + WGSL のテンプレートを
提供しているので、そのまま型を流用できる。

### 3.2 数学

カメラ空間の視線ベクトルから equirectangular UV への逆射影:

1. **NDC → カメラ視線**
   ```
   aspect    = viewport.w / viewport.h
   tan_half  = tan(fov_y * 0.5)
   ndc       = (uv.x * 2 - 1, 1 - uv.y * 2)
   cam_dir   = normalize(vec3(ndc.x * tan_half * aspect,
                              ndc.y * tan_half,
                              -1.0))           // -Z 前方
   ```

2. **pitch (X 軸回転) → yaw (Y 軸回転) を適用**
   ```
   p1 = rotate_x(cam_dir, pitch)
   wd = rotate_y(p1, yaw)                       // ワールド視線
   ```

3. **ワールド視線 → 経度緯度**
   ```
   lon = atan2(wd.x, -wd.z)                     // [-π, π]
   lat = asin(wd.y)                             // [-π/2, π/2]
   ```

4. **経度緯度 → equirectangular UV**
   ```
   u = lon / (2π) + 0.5
   v = 0.5 - lat / π
   ```

5. **テクスチャサンプル**
   - U 方向は `AddressMode::Repeat` (経度のシームを跨ぐ)
   - V 方向は `AddressMode::ClampToEdge` (極で外挿しない)

### 3.3 WGSL スニペット (試作)

```wgsl
struct Params {
    yaw: f32,           // [-π, π]
    pitch: f32,         // [-π/2, π/2]
    fov_y: f32,         // [0.2, 2.6] rad ≒ [11°, 150°]
    aspect: f32,        // viewport.w / viewport.h
};

@group(0) @binding(0) var pano_tex: texture_2d<f32>;
@group(0) @binding(1) var samp:     sampler;
@group(0) @binding(2) var<uniform> params: Params;

const PI: f32 = 3.141592653589793;
const INV_TWO_PI: f32 = 0.15915494309189535;
const INV_PI: f32 = 0.3183098861837907;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tan_half = tan(params.fov_y * 0.5);
    let ndc = vec2(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
    let cam_dir = normalize(vec3(ndc.x * tan_half * params.aspect,
                                 ndc.y * tan_half,
                                 -1.0));

    let cp = cos(params.pitch); let sp = sin(params.pitch);
    let p1 = vec3(cam_dir.x,
                  cp * cam_dir.y - sp * cam_dir.z,
                  sp * cam_dir.y + cp * cam_dir.z);

    let cy = cos(params.yaw); let sy = sin(params.yaw);
    let wd = vec3(cy * p1.x + sy * p1.z,
                  p1.y,
                  -sy * p1.x + cy * p1.z);

    let lon = atan2(wd.x, -wd.z);
    let lat = asin(clamp(wd.y, -1.0, 1.0));
    let uv = vec2(lon * INV_TWO_PI + 0.5, 0.5 - lat * INV_PI);
    return textureSample(pano_tex, samp, uv);
}
```

頂点シェーダは `compare_wgpu.rs` のフルスクリーン 6 頂点トライアングルをそのまま流用。

### 3.4 サンプリング品質

- **ミップマップ**: 広角 (FOV 大) で 1 ピクセルが多数のテクセルを覆うとモアレが
  出る。8K baseは`vendor/egui-wgpu`と共用するGPU生成器で、level 0 upload後に完全な
  mip chainを生成し、Linearのlevel間補間でsampleする。画面解像度相当のsettle overlayは
  縮小表示しないため1 mipのままとする。水平cropされた部分パノラマではU方向を
  ClampToEdgeへ切り替え、選択されたmip levelの反対端が混ざるのを防ぐ。
- **異方性フィルタ**: wgpu 0.x で wgpu の `Sampler` に `anisotropy_clamp` を渡せるが、
  optional feature の `ANISOTROPIC_FILTERING` が必要 (`docs/video-architecture.md` で
  すでに使われているか要確認、未使用なら追加負担あり)。**Phase 1 では mipmap +
  Linear で十分**。
- **シーム**: U=0 / U=1 の繋ぎ目で `AddressMode::Repeat` を効かせれば、ハードウェア
  バイリニアが自動でラップする。
- **極**: pitch が ±π/2 に近づくと 1 ピクセルが緯度線方向に伸びる (V の偏微分が
  発散) が、equirect の宿命なので受容する。

### 3.5 高解像度ソースの扱い (8K 超 / 16K)

実カメラの典型出力と既存上限の関係:

| ソース | 解像度 | 8192 単一で OK? | 16384 単一で OK? |
| --- | --- | --- | --- |
| ChatGPT / DALL-E | 1024×512 〜 2048×1024 | ✓ | ✓ |
| GoPro Max | 5760×2880 | ✓ | ✓ |
| RICOH Theta Z1 | 6720×3360 | ✓ | ✓ |
| Insta360 X3 stitched | 11968×5984 | ✗ | ✓ |
| Insta360 X4 | 8192×4096〜11968×5984 | △ | ✓ |
| プロ機 / 32K stitched | 16384×8192〜32768×16384 | ✗ | ✗ (タイル必要) |

mIV 既存の `MAX_TEXTURE_DIM = 8192` (`src/app.rs:20587`) は wgpu の絶対上限ではなく
**wgpu の控えめなデフォルト** + mIV 自身の `clamp_dynamic_for_gpu` (`app.rs:14026 /
14203 / 14332`) で 2 層に強制している値。**現代の GPU (D3D12 / Vulkan / Metal) は
ほぼ全て 16384 をサポート**し、RTX 4090 は D3D12 で 32768 まで対応する。

#### Phase 1 の方針 (確定): **8K 単一テクスチャ**

Phase 1 では既存の `fs_cache` (8K clamp 後) を 360 ベーステクスチャに**そのまま
流用**する。新規の `pano_source_pixels` / `clamp_panorama_for_gpu` /
`MAX_PANORAMA_DIM` / wgpu アダプタリミット引き上げは **いずれも実装しない**。

- 8K base のアップロードは 134 MB / ~10-30 ms 級で既存 `ctx.load_texture` 経路と
  同等
- 8K で品質が足りない領域 (>8K source) は **Phase 2a の settle-refinement** で
  カバー。full source から 4K viewport を直接サンプルするほうが、16K proxy を
  介するより高品質
- 実カメラの ~12K (Insta360 X3 等) は Phase 1 では 8K にダウンサンプルされ、
  Phase 2a の settle で停止時に高品質 viewport が出る

#### 検討して不採用にした案 (16K base / タイル分割)

実装着手前のレビューサイクルで、以下の案も検討したが Phase 1 では採用しない:

- **16K 単一テクスチャ**: 512 MB の単発 `queue.write_texture` が UI スレッドを
  50-150 ms ブロックする問題があり、Phase 1 のスコープから外した。詳細な議論経緯と
  案 B (バンド分割) / 案 C (idle 限定) の不採用理由は **§12.1** に集約
- **>16K の 2 タイル水平分割**: シーム処理 / 経度ラップと内部シームの AddressMode
  混同 / mipmap 不整合 / テスト素材確保の難しさから、実需が出てから Phase 3 で
  検討する

上記は将来再検討する余地がある (実カメラの 16K-32K 級素材が増えた / >16K の特殊な
ユースケースが固まった等)。再着手する時点で本節を更新する。

#### 全体上限 (`MAX_TEXTURE_DIM`) を上げない理由 (Q&A メモ)

「いっそ 360 専用ではなく全体パイプラインで 16K 化したらどうか」という案も検討した
が、影響範囲が広いため Phase 1 では見送る:

| 影響 | 内容 |
| --- | --- |
| CPU RAM | `fs_cache.pixels` が 1 枚あたり 4× (16K で 512 MB) |
| UI ヒッチ | `ctx.load_texture` のアップロードが 100ms 級になり得て、フレームを 6 フレーム飛ばす |
| AI ガード | 執筆時は `ai_upscale_skip_px=2048` ×4 = 8188 で 8K に収まる前提だった。現在はサイズ上限 (`ai_upscale_size_limit`) が 4096 系まで選べる代わりに、`run_final_ai_job` が ×4 出力を 8192 以下へ縮小して格納する (docs/ai-processing-size-threshold-plan.md)。16K 化するならこの縮小段も含めて再設計が必要 |
| 補正 / mask / erase / undo | 内部寸法仮定の点検が必要 |
| 利得 | フィット表示では 16K→4K にダウンサンプルされるので、8K→4K と見え方はほぼ同じ。100% ピクセル等倍時のみ差が出る |

将来的に「高解像度モード」を環境設定にトグル追加する余地は残す (デフォルト OFF、
AI ガード等を併せて再設計)。

<!-- 旧 Phase 1 の 16K 設計テキスト (削除済み、Codex 第 4 ラウンド指摘 P1 反映)。
     設計判断の詳細は §12.1 に集約。 -->

### 3.6 高解像度ソースの品質補完: settle-refinement (Phase 2a)

8K base では >8K source (= ChatGPT 出力以外のほぼすべての実カメラ output) の
ドラッグ中画質が下がる。これを **「視点が止まった瞬間に画面解像度ぶんだけ CPU で
フル解像度から再サンプリング**」してオーバーレイ表示する方式で補完する。

settle-refinement は実装済み。**現行の発動条件 =
「stationary (500 ms 静止) かつ `settle_enabled(state, policy) == true`」**。
`settle_enabled` は (a) state が `SettleReady` / `SettleApproved` のいずれか、かつ
(b) policy が `EnabledFromRaw` / `EnabledWithColorAdjustments` のいずれか、の両方を満たすときだけ
true (§3.6.2.1)。BaseOnly / NeedsUserConfirmation や AI 有効中 / post_filter ON / auto_mode
ON では settle スキップ。

#### 3.6.1 業界標準アルゴリズムとの比較

| 方式 | 採用例 | mIV にとっての評価 |
| --- | --- | --- |
| マルチ解像度タイルピラミッド | Krpano / Pannellum / Marzipano / Google Street View | 事前処理が必要 = 「開いた瞬間に表示」の流儀に合わない |
| キューブマップ変換 | Unity / Unreal / VRChat | 26K で 6 面 × 16K = 1.5 GB VRAM、動画 / AI / PDF と取り合うので避けたい |
| **settle-refinement** | Photo Sphere Viewer / Microsoft Photos | **mIV と相性◎**: ローカルファイル + 即時表示 + VRAM 増分 33 MB |

#### 3.6.2 動作シナリオ (Phase 2a、ユーザー判断 + 解像度ゲートに再設計)

**重要な前提 (Codex P1 第 5 ラウンド + ユーザー要望反映)**:

- Phase 2a のロード経路は **`start_fs_load` の通常デコードより前にヘッダ判定** を
  入れる (二重デコード回避)
- **RAM 検出ベースの tier 判定は廃止**。代わりに **ソース解像度の固定ゲート (200 MP)**
  と **ユーザー確認プロンプト**で分岐する
- テスト困難な RAM 動的判定 / sysinfo 依存 / 4 GB 環境テストは設計から除外

ロード経路の判定:

```
ヘッダ判定 (~1 ms): width / height / format
   ↓
360 候補判定 (アスペクト 2:1 or GPano XMP)
   ├─ No  → 通常パスへ (本機能とは無関係)
   └─ Yes ↓
       source_pixels = width * height
   ┌───────────────────────────────────────────────────┐
   ↓                       ↓                            ↓
[SettleReady]            [NeedsUserConfirmation]      [BaseOnly]
≤ 200 MP                 > 200 MP かつ                > 200 MP かつ
(consumer cameras +      未承認 or 旧承認超過        ユーザーが
ChatGPT 等)                  ↓                        「最大 8K(軽量)」
   ↓                     バナー表示:                   選択 (または
通常パス + tee で         "大きな画像です、高画質?"   NeedsUserConfirmation
HighResSource::Decoded     ↓                          のまま放置)
を作る                   ユーザー選択待ち                 ↓
   ↓                       ├─ フル解像度(高画質)        通常パスのみ、
fs_cache + HighResSource    │   → SettleApproved        HighResSource は
両方保持                    │   → HighResSource         作らない、settle OFF
   ↓                       │     ロード開始
settle ON                  └─ 最大 8K(軽量) → BaseOnly
                              → 通常パスのみ
```

**ユーザー確認の永続化 (Codex P2 第 9 ラウンド反映で改善)**:

単純な `bool` だと「201 MP を承認したら 537 MP も無確認になる」過度な許可になる。
**承認した最大ピクセル数を記憶**することで、前回より大幅に大きい画像では再確認する:

```rust
pub(crate) pano_session_approved_max_pixels: u64,  // 起動時 0、設定永続化しない
```

判定ロジック:

```rust
fn needs_user_confirmation(source_pixels: u64, approved_max: u64) -> bool {
    if source_pixels <= PANO_SETTLE_MAX_PIXELS { return false; }  // 200 MP 以下
    // 承認済み最大 × 1.25 を超える場合は再確認 (= 25% 以上大きいなら聞き直す)
    source_pixels > approved_max.saturating_mul(125) / 100
}
```

例:
- 201 MP 承認 → `approved_max = 201 MP`
- 次に 220 MP (= 201 × 1.09) → 確認不要 (許容範囲内)
- 次に 338 MP (= 201 × 1.68) → 再確認バナー表示
- 538 MP 承認 → `approved_max = 538 MP`
- 次に 600 MP (= 538 × 1.12) → 確認不要

バナーの「今後も高画質モードで開く」チェックボックスは
`pano_session_approved_max_pixels = source_pixels` を立てる動作になる。

mIV 再起動で 0 にリセット。将来 Phase 3 で「常に高品質」を環境設定の永続トグルと
して追加可能 (その場合は `u64::MAX` 相当を立てる)。

通常時の動作シナリオ (SettleReady = 200 MP 以下の典型ケース):

1. ユーザーが equirect (12K Insta360 X3 等、~72 MP) を開く
2. ヘッダ判定 → 72 MP < 200 MP → **SettleReady 状態確定**
3. ワーカーがフルデコード (1 回) → DynamicImage に ~290 MB
4. **同じ DynamicImage から 2 つを作る** (二重デコード回避):
   - `clamp_dynamic_for_gpu` で 8K に縮めて ColorImage → `fs_cache[idx]` (既存と同じ)
   - フル RGBA を `Arc<Vec<u8>>` 化 → `pano_high_res_source[key] = Decoded { rgba, w, h }`
5. DynamicImage を drop → CPU 定常: 134 MB (fs_cache) + 290 MB (HighResSource) = ~424 MB
6. 360 モード ON: `fs_cache[idx]` の 8K ColorImage を `color_image_to_rgba` で変換 →
   wgpu raw テクスチャアップロード。**初回だけ 1 フレーム落ちの可能性あり**、計測で
   判断し必要なら worker 化 (§4.1.1 のヒッチ可能性節)
7. ドラッグ停止 → 500 ms 経過で **settle 検出**
8. 別スレッドで `render_settle_overlay` を実行 (rayon 並列、§3.6.3、source = full RGBA)
9. 完成オーバーレイを `wgpu::Texture` にアップロード (~33 MB / 5 ms)、8K base と
   alpha ブレンドして画面に表示
10. ドラッグ再開を検出 → オーバーレイ即 drop、進行中なら cancel-token で中断、
    8K base 単独表示に戻る

大画像時のシナリオ (NeedsUserConfirmation → SettleApproved、26K source 等):

1. ユーザーが 26K equirect (~338 MP) を開く
2. ヘッダ判定 → 338 MP > 200 MP、かつ `needs_user_confirmation(338M, approved_max_pixels)
   == true` → **NeedsUserConfirmation**
3. **通常パスは普通に走り、`fs_cache` に 8K base が入る** (既存挙動、ユーザーには
   即座に 8K 表示が出る)
4. 360 モード ON → 8K base で 360 描画開始 + フルスクリーン上部にバナー表示:
   ```
   ⚠ 大きな 360° 画像です (26000 × 13000、約 338 MP)
   高品質モードで表示するには 約 1.35 GB のメモリを使います。
        [ フル解像度(高画質) ]   [ 最大 8K(軽量) ]   □ 今後も高画質モードで開く
   ```
5. ユーザー選択:
   - **「フル解像度(高画質)」**: 状態が SettleApproved に遷移 → worker thread でフル
     RGBA を**再デコード** (この時点で初めて 1.35 GB ピーク発生、~3-5 秒) →
     `pano_high_res_source` 格納 → settle 機能有効化。チェックボックス ON なら
     `pano_session_approved_max_pixels = source_pixels` (= 338 MP) を記録
   - **「最大 8K(軽量)」**: 状態が BaseOnly に遷移 → バナー閉じる、以降 8K のまま

**重要 (Codex P1 二重デコード回避との関係)**: SettleReady (200 MP 以下) の場合は tee
で 1 回デコード。SettleApproved (200 MP 超) の場合は、**8K base ロードと高品質
ロードを 2 段階に分ける** ことになる (デコードは 2 回)。これは意図的な妥協で、
理由は:

- 200 MP 超画像は元々デコードが遅い (秒オーダー) ため、即時 8K 表示の優先度が高い
- ユーザー判断を待つ間にフル RGBA を投機的に作るのは無駄になりうる
- 2 段階デコードのコストは、200 MP 超画像のユーザー体験では許容範囲

200 MP 以下では tee 経路 (1 回デコード) で最適化、200 MP 超ではユーザー判断後に
追加デコード、という設計トレードオフ。

#### 3.6.2.1 settle overlay の適用範囲 (2026-08 現行仕様)

settle worker は原本ファイルを再デコードし、元 RGBA を sample する。再現できるのは
「原本」と「原本へ通常の色調補正を再適用した結果」だけである。360 の 8K base は通常表示と
同じ完成済み最終合成を最優先し、次の順でソースを選ぶ。

1. 完成済み `final_composite_cache`
2. `adjustment_cache`
3. 対象ページで AI が実効的な場合の `ai_upscale_cache`
4. `fs_cache`

`final_composite_cache` には色調補正、AI、ポストフィルタ、スマートシャープ、自動補正に加え、
消しゴム・隠蔽加工・補正レイヤーも含まれ得る。AI / ポストフィルタ / スマートシャープ /
自動補正が有効なときは base を維持したまま settle を無効にする。一方、画素編集だけを理由に
settle は無効化しない。このため画素編集を含む base から原本由来の settle overlay へ切り替わると
編集が見えなくなるが、AI とポストフィルタを含む既存の 360 表示を優先し、この差は仕様として
受け入れる。画素編集があり、かつ settle が実際に動く状態ではステータス行に注意書きを表示する。

| 状態 | settle policy | 理由 |
| --- | --- | --- |
| AI が対象ページで実効的 | `Disabled { AiApplied }` | AI 結果を原本 sample から再現できない |
| `auto_mode` が有効 | `Disabled { AutoAdjustmentApplied }` | 自動補正を同じ条件で再計算できない |
| post filter が有効 | `Disabled { PostFilterApplied }` | settle renderer の色調再適用範囲外 |
| smart sharpen が有効 | `Disabled { SmartSharpenApplied }` | 原本 sample から再現できない |
| `fs_cache` + 色調恒等 | `EnabledFromRaw` | 原本をそのまま sample できる |
| `fs_cache` + 色調補正あり | `Disabled { WaitingForColorAdjustments }` | 補正 cache 完成までの過渡状態 |
| `adjustment_cache` + 色調補正あり | `EnabledWithColorAdjustments { params }` | sample 後に同じ色調補正を再適用できる |
| `final_composite_cache` + 色調恒等 | `EnabledFromRaw` | 原本 sample と視覚的に一致する前提 |
| `final_composite_cache` + 色調補正あり | `EnabledWithColorAdjustments { params }` | sample 後に色調補正を再適用する |

`PanoramaSettlePolicy::Disabled` の理由は `PanoramaSettleDisabledReason` で保持する。
表示側は `disabled_reason()` を読むだけとし、policy の判定木や理由表を複製しない。

#### 3.6.2.2 ソース選択と invalidation の不変条件

`resolve_pano_source` は pixels、`source_kind`、`cache_key`、settle policy を一度に決める。
`source_kind` は `0=fs_cache`、`1=adjustment_cache`、`2=ai_upscale_cache`、
`3=AI 実効時の adjustment fallback`、`4=final_composite_cache` とする。種別 3 は
AI 由来 pixels を意味しない。`adjustment_cache` の唯一の writer は
`App::apply_sync_adjustment` であり、その呼び出し元はいずれも raw `fs_cache` の pixels を渡す。
AI 実効時に adjustment fallback を選んだ事実を policy へ伝え、settle を無効にするための印である。

`cache_key` には補正世代、AI 世代、完成済み最終合成の key hash を含める。設定変更や最終合成の
完成時に、古い upload / overlay を同一状態と誤認しないためである。
画素編集の有無を表す `App::has_pano_excluded_pixel_edits` はソース選択には使わず、
高画質化で編集が見えなくなり得る場合のステータス注意書きだけに使う。

#### 3.6.3 CPU レンダの中身 (独自 sampler、settle policy 反映、Codex P1 第 11 反映)

settle render は 2 ステップ構成:

1. **球面サンプリング**: rayon par_chunks で bilinear sample → Vec<u8> overlay
2. **補正の再適用** (`EnabledWithColorAdjustments` のみ): overlay を `ColorImage` 化して
   **既存 `crate::adjustment::apply_adjustments_fast` を直接呼ぶ**

`build_lut` / `apply_lut` という独自関数は作らない (実コード `src/adjustment.rs` には
存在せず、内部の `build_u8_lut` は private)。`apply_adjustments_fast` は public
(`src/adjustment.rs:394`) で `&ColorImage → ColorImage` の API、内部で LUT 経路 / f32
パイプラインを自動選択する。これを通すことで **既存補正パイプラインの結果と整合**する。

```rust
fn render_settle_overlay(
    src_rgba: &[u8], src_w: u32, src_h: u32,    // 26000×13000 RGBA8 想定
    yaw: f32, pitch: f32, fov_y: f32,
    out_w: u32, out_h: u32,                     // 画面解像度 (3840×2160 等)
    policy: &PanoramaSettlePolicy,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    let aspect = out_w as f32 / out_h as f32;
    let tan_half = (fov_y * 0.5).tan();

    match policy {
        PanoramaSettlePolicy::EnabledFromRaw
            | PanoramaSettlePolicy::EnabledWithColorAdjustments { .. } => {},
        PanoramaSettlePolicy::Disabled { .. } => {
            debug_assert!(false, "settle render called with Disabled policy");
            return None;
        }
    }

    // === ステップ 1: 球面サンプリング ===
    let mut out = vec![0u8; (out_w * out_h * 4) as usize];
    out.par_chunks_exact_mut((out_w * 4) as usize)
        .enumerate()
        .try_for_each(|(y, row)| {
            if cancel.load(Ordering::Relaxed) { return Err(()); }
            let v_ndc = 1.0 - (y as f32 + 0.5) / out_h as f32 * 2.0;
            for x in 0..out_w {
                let u_ndc = (x as f32 + 0.5) / out_w as f32 * 2.0 - 1.0;
                let (u, v) = ndc_to_equirect_uv(u_ndc, v_ndc, aspect, tan_half, yaw, pitch);
                let rgba = sample_bilinear_equirect(src_rgba, src_w, src_h, u, v);
                let off = x as usize * 4;
                row[off..off+4].copy_from_slice(&rgba);
            }
            Ok(())
        })
        .ok()?;

    // === ステップ 2: 補正再適用 (EnabledWithColorAdjustments のみ) ===
    if let PanoramaSettlePolicy::EnabledWithColorAdjustments { params } = policy {
        if cancel.load(Ordering::Relaxed) { return None; }
        // overlay を ColorImage に変換 (Color32 の RGBA bytes に詰め直し)
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [out_w as usize, out_h as usize],
            &out,
        );
        // 既存補正パイプラインをそのまま呼ぶ
        // (apply_adjustments_fast は internal で LUT or f32 経路を自動選択、
        //  EnabledWithColorAdjustments では auto_mode は除外済みなので no-op)
        let adjusted = crate::adjustment::apply_adjustments_fast(&ci, params);
        out = crate::capture::color_image_to_rgba(&adjusted);
    }

    Some(out)
}
```

**実装トレードオフ** (Codex P2 第 11 反映):

- **補間 → 補正の順序**: settle は「球面 sample (bilinear) → 補正」。一方 8K base は
  「補正済み 8K → GPU で線形拡大」(= 補正 → bilinear)。**gamma 等の非線形変換が
  入る場合、厳密には完全一致しない**
- **完全一致を狙うなら逆順**: full RGBA に先に補正を掛けてから sample する。ただし
  full RGBA は最大 2.15 GB、補正切替の度に再計算で 100 ms+ ブロック → 不採用
- **現実的な目標**: 単純色調補正の source だけを settle 対象にし、AI / post_filter /
  auto_mode を含む source では base を保ったまま settle を無効にする (§3.6.2.1)。
  単純色調補正なら順序差は実用上気づきにくい範囲

**性能見積もり** (4K viewport × `apply_adjustments_fast`):

- 球面 sample: 50-100 ms (前回見積もり、§3.6.3 既存)
- ColorImage 化 + apply_adjustments_fast + color_image_to_rgba: 20-50 ms 追加
- 合計: 70-150 ms。実測ベースで判断、超過時は出力解像度を一段下げる

/// equirect 用のバイリニアサンプラ。U は `rem_euclid` で経度ラップ、V は clamp。
fn sample_bilinear_equirect(src: &[u8], w: u32, h: u32, u: f32, v: f32) -> [u8; 4] {
    let x = u * w as f32 - 0.5;
    let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let x0w = x0.rem_euclid(w as i32) as usize;          // 経度ラップ
    let x1w = (x0 + 1).rem_euclid(w as i32) as usize;
    let y0c = y0.clamp(0, h as i32 - 1) as usize;        // 緯度クランプ
    let y1c = (y0 + 1).clamp(0, h as i32 - 1) as usize;
    // 4 texel lookup + bilinear blend
    // ... (省略)
}
```

**サンプラを `fast_resize` 流用にしない理由 (Codex P2 反映)**: 既存
`src/fast_resize.rs::resize_rgba8_exact` は **矩形 → 矩形のリサイズ専用**であり、
任意の (u, v) を per-pixel に問い合わせる API は持たない。settle-refinement は
出力ピクセルごとに球面座標から逆引きする処理なので、専用の bilinear sampler
(上記) を新規実装する。

**性能見積もり (実測前提)**: 4K 出力 (3840×2160 = 8.3 M pixels) × bilinear (4 texel
+ 16 乗算 + blend) を rayon で 16 並列実行した場合の **見積もりは 30-100 ms**。
ただしこれは未実測の概算値で、AVX2 を使えるか / メモリ帯域 / SrcRGBA のキャッシュ
ヒット率に強く依存する。**Phase 2a の最初のマイルストーンは「実機で計測し、4K
viewport で 100 ms 以下に収まることを確認」とする**。100 ms 超なら:

- 出力解像度を一段下げる (例: 1920×1080 + GPU 側で線形拡大、品質微減)
- bicubic を断念して bilinear のみとする
- 並列度を上げる (chunk サイズ調整)

bicubic 化は Phase 3 の任意拡張。Phase 2a は bilinear で確定。

#### 3.6.4 判定ロジック (解像度ゲート + ユーザー確認、第 7 ラウンド確定)

**重要な前提**:

- ヘッダ判定だけで決める (デコード前)。`image::ImageReader::with_format` で原寸を
  probe (~1 ms)、`width * height` から `source_pixels` を計算
- **RAM 検出 (`sysinfo` クレート / 動的判定) は使わない** — テスト性と挙動の説明
  しやすさを優先 (詳細 §12.3)
- バナーの「想定 RAM 消費」表示は worst case ピーク (W*H*4*2) を出すが、これは
  **判定には使わず情報提示のみ**

判定ロジック:

| 条件 | 状態 |
| --- | --- |
| `source_pixels <= 200 MP` (形式不問) | **SettleReady** (自動、確認なし) |
| `source_pixels > 200 MP` かつ `source_pixels <= approved_max × 1.25` | **SettleApproved** (自動承認、確認なし) |
| `source_pixels > 200 MP` かつ `source_pixels > approved_max × 1.25` | **NeedsUserConfirmation** (バナー表示、前回承認より 25% 以上大きい) |
| バナーで「フル解像度(高画質)」選択 | **SettleApproved** (auto_approve トグル ON ならセッション継続) |
| バナーで「最大 8K(軽量)」選択 | **BaseOnly** (settle OFF) |

**200 MP ゲートの根拠**:

| 例 | 解像度 | MP | デフォルト挙動 |
| --- | --- | --- | --- |
| ChatGPT 出力 | 2048×1024 | 2 MP | SettleReady (自動) |
| GoPro Max | 5760×2880 | 17 MP | SettleReady (自動) |
| RICOH Theta Z1 | 6720×3360 | 23 MP | SettleReady (自動) |
| Insta360 X3 | 11968×5984 | 72 MP | SettleReady (自動) |
| 16K stitched | 16384×8192 | 134 MP | SettleReady (自動) |
| 26K プロ用 | 26000×13000 | 338 MP | NeedsUserConfirmation → 都度プロンプト |
| 32K stitched | 32768×16384 | 537 MP | NeedsUserConfirmation → 都度プロンプト |

**ユーザー確認 UI 仕様**:

- 表示位置: フルスクリーン上部のバナー (動画 HUD と同じ階層)
- 内容: 解像度 / MP / 想定 RAM 消費を **数値で明示**
- 選択肢: 「フル解像度(高画質)」/「最大 8K(軽量)」の 2 ボタン
- チェックボックス: 「今後も高画質モードで開く」(デフォルト OFF)
- 表示タイミング: 360 モード ON 直後、8K base 表示は先行して既に出ている
- 既定動作 (= バナー無視): しばらく表示後にフェードアウト。バナーは消えるが
  `pano_quality_state` は `NeedsUserConfirmation` のまま (BaseOnly には自動遷移
  しない)。ユーザーが「最大 8K(軽量)」を**明示クリック**したときだけ BaseOnly になる

**BaseOnly / SettleApproved 後の切替導線** (Codex P2 第 9 ラウンド + 2026-05 後続反映):

「最大 8K(軽量)」を選んだ後に「やっぱり高画質で見たい」、あるいは逆に「高画質モード
を解除してメモリを解放したい」というケースがある。**画面下部中央の status indicator
(pill バッジ) 内に切替ボタンを 1 つだけ常設**して、`pano_quality_state` を直接
`SettleApproved` ↔ `BaseOnly` に切り替える:

- **`BaseOnly` のとき**: `[高画質に切替]` ボタン
  - クリックで state → `SettleApproved`、`start_pano_high_res_load(fs_idx, cache_key)`
    を kick。**`pano_session_approved_max_pixels` は bump しない** (= この 1 枚だけの
    切替、次の > 200 MP の新画像はバナーで再確認)。session-wide 承認はバナーの
    チェックボックスに限定するため
  - 表示条件: `is_plain_image (= GridItem::Image)` かつ
    `!pano_high_res_failed.contains(&source_key)` かつ `policy_enabled`
    (= ZIP/PDF/Video や decode 失敗履歴あり / AI 中・補正中の画像では非表示。
    押下しても `start_pano_high_res_load` が即 return して "高画質 ロード中…" で
    永久 stall するのを避ける)
  - ホバー: `約 X.X GB の RAM を使います` (= `W × H × 4 × 2 / 1e9`)
- **`SettleReady` / `SettleApproved` のとき (= settle 経路 active)**: `[8K 軽量に切替]`
  ボタン
  - クリックで state → `BaseOnly`、`pano_high_res_pending` を drain & cancel +
    `pano_high_res_source.remove(&source_key)` + `clear_pano_refinement()`。
    フル RGBA メモリ (1-2 GB) を即解放
  - ホバー: `約 X.X GB を解放します`
- **`NeedsUserConfirmation` (= バナー表示中) / `policy_enabled=false` / state 未設定**:
  ボタン非表示。バナーまたは AI/補正設定の解除が先

旧設計の「上部 360 ボタン横の ⚙ 高品質化アイコンで `NeedsUserConfirmation` に戻す」
導線は廃止 (= 直接遷移の方がワンクリックで完結するため)。

**RAM 検出不要 (旧 tier 判定との比較)**:

- ❌ 旧: `sysinfo` クレートで空き RAM 取得、× 0.3 で閾値判定
- ✅ 新: 解像度のみで判定、`sysinfo` クレート不要、設計から削除
- ❌ 旧: 4 GB / 8 GB / 16 GB それぞれの環境でテスト必要
- ✅ 新: 200 MP 境界のテスト 1 種類で OK (CI で自動化可能)
- ❌ 旧: 動的な空き RAM 変動で判定が不安定
- ✅ 新: ソース解像度は静的 = 判定が決定的

#### 3.6.4.1 ピーク使用量内訳 (Phase 2a 時点、解像度ゲート版)

「既存ピーク」= mIV が 360 機能無しでもこのファイルを開けば発生する量。
「360 追加分」= 360 機能を入れることで増える分。

| 状態 | ソース例 | 既存ピーク | 360 追加分 (ピーク、保守的) | 360 追加分 (定常) | settle |
| --- | --- | --- | --- | --- | --- |
| SettleReady | Insta360 X3 (72 MP) | 290 MB (1 回フルデコード) | 0 〜 +290 MB (tee 時、§3.6.4.2) | 290 MB (HighResSource) + 33 MB (overlay) | ✓ |
| SettleReady (上限近く) | 16K stitched (134 MP) | 540 MB | 0 〜 +540 MB | 540 MB + 33 MB | ✓ |
| SettleApproved | 26K (338 MP)、ユーザー承認 | 1.35 GB (再デコード時) | 0 〜 +1.35 GB | 1.35 GB + 33 MB | ✓ |
| SettleApproved | 32K (537 MP)、ユーザー承認 | 2.15 GB (再デコード時) | 0 〜 +2.15 GB | 2.15 GB + 33 MB | ✓ |
| BaseOnly | 26K+、ユーザー却下 | 1.35 GB+ (既存通り) | 0 (専用 cache 追加なし) | 0 (fs_cache 流用) | ✗ |

**ユーザー承認モード (NeedsUserConfirmation → SettleApproved) の特殊性**:

8K base ロード時点で既存パス通り 1 回デコード (peak: source 寸法に応じた量)。
高品質ボタン押下後の再デコードでもう一度同じピークが立つ。**8K と高品質の両方を
持っている瞬間が一番重い**: 134 MB (8K) + decode peak (~1.35 GB at worst) + 1.35 GB
(HighResSource) ≈ 2.8 GB transient。

これを許容するのは:
- ユーザーが明示的に「フル解像度(高画質)」を選んだケースのみ
- ボタン押下から数秒の transient で、定常状態に落ち着けば 1.5 GB 程度
- メモリ消費はバナーで事前告知済み (ユーザー同意済み)

#### 3.6.4.2 デコード時の追加ピーク詳細 (SettleReady / SettleApproved 共通、Codex P1 第 6 ラウンド反映)

`DynamicImage` から `Arc<Vec<u8>>` (RGBA8) を作る経路はデコーダが返す
`DynamicImage` の variant によって追加バッファを要する:

| 入力形式 | image crate の DynamicImage variant | RGBA8 化に必要な追加 | tee 時の瞬間ピーク |
| --- | --- | --- | --- |
| JPEG (RGB) | `ImageRgb8(RgbImage)` | RGBA8 への変換で **新バッファ ~1.35 GB** | ~2.7 GB |
| PNG (RGBA、典型) | `ImageRgba8(RgbaImage)` | 不要 (`.into_raw()` でゼロコピー) | ~1.35 GB |
| PNG (RGB) | `ImageRgb8` | RGBA8 への変換で新バッファ | ~2.7 GB |
| WebP / TIFF | variant 多様 | 多くの場合 RGBA8 変換が必要 | ~2.7 GB |
| 16-bit PNG | `ImageRgba16` | 8-bit RGBA8 変換 + 新バッファ | ~2.0 GB |

**推奨実装順序** (SettleReady / SettleApproved worker 内):

```rust
// worker thread 内
let dyn_img: DynamicImage = decode(...);  // フルデコード、~1.35 GB

// 1. RgbaImage 化 (variant により追加バッファ発生、最大 1.35 GB の瞬間ピーク)
let rgba_img: RgbaImage = dyn_img.into_rgba8();
// この時点でピーク最大: ~2.7 GB (ImageRgb8 → RgbaImage の場合)
// drop(dyn_img) は into_rgba8 内で実施済み、ピーク後は ~1.35 GB に落ちる

// 2. Arc<Vec<u8>> 化 (ゼロコピー、ピーク変化なし)
let (w, h) = rgba_img.dimensions();
let rgba_vec: Vec<u8> = rgba_img.into_raw();
let high_res_rgba: Arc<Vec<u8>> = Arc::new(rgba_vec);
// 定常: 1.35 GB (HighResSource 保持)

// 3. 同じ Arc<Vec<u8>> から fast_resize で 8K ColorImage を作る
//    (fast_resize は src/dst 別バッファなので 8K 分の追加 = 134 MB)
let ci_8k = clamp_via_fast_resize(&high_res_rgba, w, h, 8192);
// 定常: 1.35 GB + 134 MB = ~1.5 GB

// FsLoadResult::StaticPanorama { ci: ci_8k, source_dims: [w, h], high_res: Decoded { rgba: high_res_rgba, w, h } }
```

**保守的な実装ポリシー**:

- ピーク見積もりは「入力が `ImageRgb8` (= RGBA8 へ変換必要)」を**前提**に立てる
- バナー表示の「想定 RAM 消費」は worst case で表示 (ユーザーに正確に伝えるため):
  `est_ram_gb = (width * height * 4 * 2) / 1e9` (×2 は RGBA8 変換の transient 含む)
- SettleApproved の 26K JPEG (RGB) で worst case ~2.7 GB → ユーザーには「約 2.7 GB」
  と表示し、判断を委ねる

#### 3.6.4.3 BaseOnly 状態の挙動詳細 (Codex P1 第 4 ラウンド反映)

BaseOnly = 200 MP 超かつユーザーが「最大 8K(軽量)」を選択したケース (またはあとから
status indicator の `[8K 軽量に切替]` で BaseOnly に戻したケース)。**360 機能による
追加メモリゼロ**、`pano_high_res_source` にエントリも作らない:

1. `start_fs_load` の 360 候補分岐は通常パス通りで、`fs_cache[idx]` に 8K 縮小版が
   入る (既存挙動)
2. 既存挙動の通り、巨大画像 (26K PNG 等) ではフルデコード時のピーク (~1.35 GB) が
   一瞬発生する。**これは mIV 既存のフルスクリーン表示の挙動と同じで、360 機能で
   新規発生するものではない** (本機能のスコープ外)
3. UI の 360 ボタンは押せる (= 360 モード自体は ON、8K base で描画)
4. **360 ベーステクスチャは `resolve_pano_source` が選んだ final / AI / adjustment / raw
   pixels から `color_image_to_rgba` で RGBA8 に変換し、raw `wgpu::Texture` を
   新規作成してアップロード**する
   (`compare_wgpu.rs` 同じ経路、§4.3 参照)。
   ⚠️ **egui の `TextureHandle` 内部の wgpu リソースを流用しようとしない** —
   egui-wgpu renderer の texture map は内部実装で、直接アクセスする公開 API は無く、
   将来の egui 更新で壊れる
5. 結果として 360 view の品質は 8K equirect 相当
6. settle-refinement は OFF

#### 3.6.5 中断と再開のセマンティクス

- **キャンセル**: `Arc<AtomicBool>` キャンセルトークン (CLAUDE.md「並列処理」準拠)。
  ドラッグ再開 / yaw・pitch・fov 変化 / フルスクリーン退出 / フォルダ切替で発火
- **デバウンス**: settle 検出は 500 ms (固定)。連続マウス操作で render が起動しっぱなしに
  ならない
- **オーバーレイの寿命**: 視点が変わったら即 drop。次の settle で再生成
- **失敗 (キャンセル) は静かに**: ユーザーに通知しない (キャンセルは正常動作)
- **完成 (成功) もユーザーに通知しない**: オーバーレイがフェードインで自然に切替
  (~150 ms フェード)。トースト等は出さない

#### 3.6.6 Phase 2a のキャッシュ enum と Phase 3 の拡張余地

`pano_high_res_source[key]` は Phase 2a では Decoded variant のみ:

```rust
enum HighResSource {
    Decoded {
        rgba: Arc<Vec<u8>>, w: u32, h: u32,
    },
}
```

**Phase 3 拡張案 (任意、実需が出てから検討)**: 巨大 JPEG (例: 32K+) のロード時間が
ボトルネックになるケース向けに、`turbojpeg-sys::tj3SetCroppingRegion` を使った部分
デコード経路を追加することは可能。実装の複雑性 (iMCU 境界 / 360 視野の u=0/1 跨ぎ /
専用 sampler) は大きいので Phase 2a スコープからは外す。

Phase 3 で追加するなら以下の variant を併設:

```rust
enum HighResSource {
    Decoded { rgba: Arc<Vec<u8>>, w: u32, h: u32 },
    // Phase 3 オプション (実需確認後):
    JpegBytes {
        bytes: Arc<Vec<u8>>,
        original_w: u32, original_h: u32,
    },
}
```

**API 注意**: 安全な `turbojpeg` crate (`src/thumb_loader.rs:419` で使用中) は
`decompress_image` までしか公開しておらず、cropping region は持たない。低位の
**`turbojpeg-sys::tj3SetCroppingRegion`** を unsafe wrapper で叩く必要があり、
**iMCU 境界制約** (YUV 4:2:0 で 16 px、YUV 4:4:4 で 8 px) で **左端は MCU 境界に
スナップ**する必要がある。

**360 視野が経度 u=0/1 を跨ぐケース**: 視野中心が yaw=±π 付近 + 広 FOV のとき、
視野は経度的に「右端 + 左端」の 2 領域に分かれる。単純な (u,v) min/max を crop に
渡すと **左端〜右端まで全幅デコードに化ける**ため、視野範囲を 2 つの region に分割し、
専用 sampler で読み込む実装が必要。

**Phase 2a での扱い (確定)**: 上記 partial decode 経路は **Phase 3 で実需確認後に
検討**。Phase 2a は **Decoded variant のみ**で実装する。

これによりキャンセル安全性 / iMCU 境界 / u=0/1 跨ぎ / 専用 sampler などの複雑さは
Phase 3 に押し出され、Phase 2a は **「フル RGBA から bilinear」のみ**に集中できる。

参考: Photo Sphere Viewer はもっと汎用の解として **マルチレベルタイル方式**
([Equirectangular tiles](https://photo-sphere-viewer.js.org/guide/adapters/equirectangular-tiles.html))
を採用している。mIV も将来 Phase 3 で「事前に分割したタイル群があるならそれを使う」
経路を考慮できるが、現状はローカルファイル即時表示の流儀を優先して settle-refinement
側を磨く。

---

## 4. wgpu 実装の構成

`src/compare_wgpu.rs` を**テンプレートとして 1:1 でコピー**できる構造になっている。

### 4.1 callback と upload の分離 (Codex P2 反映)

**callback はテクスチャ実体を持たない**。アップロード済みの wgpu リソースは App 側で
管理し、callback はそれを参照する識別子だけを持つ。これにより `Arc<Vec<u8>>` を
callback の構造体に詰めて毎フレーム mpsc に乗る悪パターンを回避する。

#### App 側のアップロード済みテクスチャ保管

```rust
pub(crate) pano_uploaded: Option<Arc<UploadedPanoTexture>>,
//                                ^^^ Codex P2 第 5 ラウンド反映: Arc で統一
//                                LRU 1 (= active のみ)

pub struct UploadedPanoTexture {
    pub source_key: String,                    // metadata_cache_key (どの画像か)
    pub cache_key:  u64,                        // §4.1.2 のパックキー
    pub texture:    wgpu::Texture,
    pub view:       wgpu::TextureView,
    pub bind_group: wgpu::BindGroup,           // pipeline と互換、prepare 不要
    pub width:      u32,
    pub height:     u32,
}
```

**LRU を 1 (active のみ) にする理由 (Codex P1 反映)**: 16K テクスチャ 1 枚で 512 MB
VRAM を食う。8-16 枚保持すると 4-8 GB 級になり、動画 / AI / PDF と取り合って VRAM
不足を招く。360 モードは「いま見ている 1 枚」だけが描画対象であり、前後画像へナビ
したら新キーで即時アップロードすればよい (§4.1.1)。**複数キャッシュの恩恵は捨てて
VRAM 安全を優先**。

#### Callback 側 (テクスチャ実体を持たない)

```rust
pub struct PanoramaShaderCallback {
    pub source_key:    String,          // App 側 pano_uploaded をルックアップする鍵
    pub cache_key:     u64,             // §4.1.2 packed key (source_kind+adjust_gen+ai_gen)
                                        // Codex P1 第 8 ラウンド反映: stale 二重チェック
    pub yaw:           f32,
    pub pitch:         f32,
    pub fov_y:         f32,
    pub aspect:        f32,
    pub target_format: wgpu::TextureFormat,
}

impl egui_wgpu::CallbackTrait for PanoramaShaderCallback {
    fn prepare(&self, _device, queue, _sd, _enc, callback_resources) -> Vec<CommandBuffer> {
        // pipeline 等の static リソース init (初回のみ)
        if !callback_resources.contains::<PanoStaticGpu>() { ... }
        // uniform 更新だけ (yaw/pitch/fov/aspect)
        let resources = callback_resources.get_mut::<PanoStaticGpu>().unwrap();
        queue.write_buffer(&resources.uniform, 0, &uniform_bytes(self.yaw, self.pitch, ...));
        // 重要: ここで queue.write_texture は呼ばない (= 大型 upload は別経路、§4.1.1)
        Vec::new()
    }

    fn paint(&self, _info, render_pass, callback_resources) {
        // App から渡された UploadedPanoTexture を callback_resources 経由で取り出す
        let uploaded = callback_resources.get::<UploadedPanoTextureRef>();
        let Some(uploaded) = uploaded else { return; };  // まだ未アップロード = 何も描画しない (8K プロキシは別レイヤーで描画済み)
        // **stale guard: source_key と cache_key の両方一致を要求** (Codex P1 第 8 ラウンド反映)
        // App 側 §4.2 の ready 判定と paint 時の二重チェックで race を排除
        if uploaded.source_key != self.source_key { return; }
        if uploaded.cache_key  != self.cache_key  { return; }
        let resources = callback_resources.get::<PanoStaticGpu>().unwrap();
        render_pass.set_pipeline(&resources.pipeline);
        render_pass.set_bind_group(0, &uploaded.bind_group, &[]);
        render_pass.draw(0..6, 0..1);
    }
}
```

**実 API (Codex P2 第 4 ラウンド反映)**: `egui_wgpu::CallbackResources` への挿入は
**`RenderState.renderer` を `write()` ロックして `.callback_resources` (型は
`TypeMap` 相当) に `insert` / `get_mut`** する経路を使う。`Painter::ctx().memory_mut(...)`
は egui の memory ストアであり別物。具体例 (`src/app.rs` の compare 経路と同じ
パターン):

```rust
// アップロード完了時 / 毎フレームの App::update 内などで
if let Some(render_state) = &self.wgpu_render_state {
    let mut renderer = render_state.renderer.write();
    let resources: &mut egui_wgpu::CallbackResources = &mut renderer.callback_resources;
    // 静的リソース (pipeline / layout / sampler) を初回挿入
    if resources.get::<PanoStaticGpu>().is_none() {
        resources.insert(PanoStaticGpu::new(&render_state.device, target_format));
    }
    // 動的: 最新のアップロード結果を Arc で挿入
    // (pano_uploaded: Option<Arc<UploadedPanoTexture>> の Arc をそのままクローン)
    if let Some(uploaded) = &self.pano_uploaded {
        resources.insert(UploadedPanoTextureRef(Arc::clone(uploaded)));
    } else {
        // アップロード未完了 / 360 OFF: 古い entry が残らないように除去
        resources.remove::<UploadedPanoTextureRef>();
    }
}
```

**layout 初期化と upload の順序** (Codex P2 反映で明記):

1. **初回 360 アクティブ化のフレーム**で、`PanoStaticGpu::new` が
   `BindGroupLayout` + `RenderPipeline` + `Sampler` を作成し `callback_resources` に
   挿入する (= layout が存在する状態になる)
2. **その同じフレーム or 次フレーム**で、App 側のアップロード経路が `BindGroupLayout`
   を参照して `BindGroup` を作る (`device.create_bind_group(&BindGroupDescriptor {
   layout: &resources.get::<PanoStaticGpu>().unwrap().bind_group_layout, ... })`)
3. **`UploadedPanoTexture` 構築時点で `bind_group` を一緒に持っておく**ので、callback
   の `paint()` は bind_group をそのまま使える (再生成不要)
4. layout 更新は target_format 変化時のみ。**format 変化を検出したら `PanoStaticGpu` を
   再構築 + 既存 `UploadedPanoTexture.bind_group` を破棄して再生成**する

`UploadedPanoTextureRef` は `Arc<UploadedPanoTexture>` のラッパ (newtype)。
**App と callback の所有関係が明確**になり、古いテクスチャを誤って使う事故が出にくい。

### 4.1.1 アップロードペーシング (案 A 確定、簡略版)

**Codex 第 3 ラウンド指摘 (P1)**:

> 512MB アップロードは `prepare()` から外しても、1 回で `queue.write_texture` すれば
> まだ UI ヒッチします。位置を変えただけで、単発 512MB 転送自体は UI スレッド上に
> 残ります。16K upload は band/tile に分けて数フレームに分割するか、16K は idle 時
> のみ、既定は 8K/Auto にするのが堅いです。

これは正当な指摘で、**「`prepare()` から `App::update` 末尾に移しただけ」では UI
フリーズは消えない**。`queue.write_texture` 自体が UI スレッド上で 50-150 ms 級
ブロックする。

選択肢は 3 つあり、Phase 1 の骨格を変える判断なので別途決定する (本ドキュメント
末尾 §13 「未決定の設計判断」参照):

| 案 | 概要 | Phase 1 への影響 |
| --- | --- | --- |
| A. 16K base を諦める | 8K equirect (= 既存 fs_cache のまま) をベースに、settle-refinement で品質を担保。wgpu limit upgrade も不要 | Phase 1 大幅に簡素化、main.rs 変更最小 |
| B. 16K base をバンド分割アップロード | 16K テクスチャを N 本の水平バンドに分け、N フレームかけて `queue.write_texture` を分割実行 | Phase 1 にバンド分割実装 + 中間状態 (一部だけ 16K) の表示制御を追加。複雑度上昇 |
| C. 16K base を idle 限定 | 平時は 8K、ユーザーが N 秒静止したら 16K にアップグレード。実質「16K = settle-refinement の一段階前」 | settle-refinement と概念的に重複。simpler than B、ややA寄り |

**確定 (ユーザー承認済み)**: **案 A (16K base を諦め、8K + settle に統合)**。
8K アップロードは 134 MB と既存の `ctx.load_texture` 経路と同等の負荷で、Phase 1 の
スコープに収まる。案 B/C は不採用 (詳細は §12.1)。

#### Phase 1 のアップロード経路 (確定)

```
ファイル選択
   ↓
start_fs_load (既存ワーカー、変更なし) → fs_cache[idx] に 8K RGBA 入る
   ↓
360 モード ON
   ↓ (フルスクリーン入場時 or 360 ボタン ON 時)
fs_cache[idx] の ColorImage → color_image_to_rgba → 8K RGBA (~134 MB)
   ↓
wgpu::Texture 新規作成 + queue.write_texture
   ↓
UploadedPanoTexture として App::pano_uploaded に保持
   ↓
callback で参照 → equirect 描画
```

#### 8K アップロードのヒッチ可能性 (Codex P2 第 4 ラウンド反映)

「ヒッチほぼなし」と楽観しない。実態は以下のコスト:

| 工程 | コスト | 場所 |
| --- | --- | --- |
| `color_image_to_rgba` (134 MB バイト並べ替え) | ~30-80 ms (実測必要) | 呼び出し場所による (UI スレッド or worker) |
| `queue.write_texture` (134 MB PCIe 転送) | ~10-30 ms | UI スレッド (= App::update 内) |
| `device.create_texture` + `create_bind_group` | ~1 ms | UI スレッド |

合計 ~40-110 ms 級になり得る。**Phase 1 実装着手時はまず素直に UI スレッドで実行
して計測**し、以下を判断する:

- **計測 < 16 ms**: そのままで OK (1 フレ落ち以内)
- **計測 16-50 ms**: 360 トグル ON の初回 1 フレ落ちは許容、トースト等で「準備中」を
  軽く出す
- **計測 > 50 ms**: **`color_image_to_rgba` を worker thread に移し、結果の
  `Arc<Vec<u8>>` を mpsc で受け取って次フレームで `write_texture`** に変更。
  既存の `start_metadata_load` 等の worker パターンを流用 (CLAUDE.md「UI スレッドでの
  同期 I/O は即 worker 化する」§4 チェックリスト準拠)

worker 化の経路はオプションで Phase 1 に含める (テスト時に必要なら入れる、不要なら
省く)。**「アップロード完了まで callback shape を出さない」(§4.2) によって、ユーザー
体験は「数フレ平らな equirect → 360 描画開始」になるだけで破綻はしない**。

**案 B/C は不採用** (§12.1 参照)。本ドキュメントから関連記述 (`pano_upload_state` /
バンド分割 / idle アップロード) は省略する。実装時に再検討するなら §12.1 + git 履歴
を参照。

### 4.1.2 cache_key の設計

**`compare_wgpu.rs::ensure_pair` は size 変化でしか再構築判定しない**問題がある
(補正後 / AI 後で `pixels.size()` が同じだと古いテクスチャが残る)。本機能はそれを
踏襲しない。`UploadedPanoTexture.cache_key: u64` は以下 4 要素を u64 に畳む:

```rust
// 64-bit packed:
//   [63..48]: idx_hash16   (path-derived metadata key の crc16)
//   [47..32]: source_kind  (0=fs_cache, 1=raw+adjustment, 2=AI,
//                           3=AI 実効時の adjustment fallback, 4=final composite)
//                            high-res RGBA は
//                            base texture には入れず settle 専用ストア
//                            (`pano_high_res_source`) で別管理
//   [31..16]: adjust_gen16 (App::adjustment_generation[idx] の下位 16bit、変更で +1)
//   [15..0]:  ai_gen16     (App::ai_upscale_generation[idx] の下位 16bit、変更で +1)
fn make_pano_cache_key(
    idx_hash: u16,
    source_kind: u16,
    adjust_gen: u16,
    ai_gen: u16,
) -> u64 { ... }
```

完成済み final composite を選んだ場合は、さらに `FinalCompositeKey` の hash を
`cache_key` へ畳み込み、画素編集や最終段の完成差し替えも再アップロード条件に含める。

再アップロード判定:

1. `pairs.get(&key)` でヒットしない → 新規アップロード
2. ヒットして size が違う → 再アップロード (古い entry は `pairs.remove`)
3. ヒットして **Arc::ptr_eq(&pair.rgba, &callback.rgba) が false** → 再アップロード
   (世代キーが共存ぶつかりした稀ケースの保険)
4. それ以外 → スキップ

`App` 側で `adjustment_generation: HashMap<String, u32>` / `ai_upscale_generation:
HashMap<String, u32>` をすでに持っていれば流用 (なければ補正適用 / AI 完了の場所で
インクリメントする最小実装を追加)。`source_kind` は描画時に「今どのキャッシュ層を
入力にしているか」で決まる (§4.3)。

#### bit 配分の wrap リスクと Phase 3 拡張余地 (Codex P2 第 16 反映)

現状の bit 配分は `idx_hash 16 + source_kind 16 + adjust_gen 16 + ai_gen 16 = 64`。
**16 bit gen は 65,536 回で wrap する**ため、理論的には:

- 1 秒に 1 回 generation 更新するセッション (= 高頻度スライダー操作) → 18.2 時間で wrap
- 補正アニメーション系 (毎フレ更新) → 約 18 分で wrap

wrap が起きると、**過去に同じ cache_key が観測された組み合わせ**で stale guard が
誤って「一致」と判定し、古いテクスチャを描画する可能性。実害は低 (補正状態が
65,536 操作前と完全一致するレアケース) だが、長時間セッションでは無視できない。

**Phase 3 拡張案**:

- `source_kind` を必要な値数に合わせて 3 bit に縮める
- 余った bit を `adjust_gen` / `ai_gen` に再配分し、
  数日連続使用に耐える)
- または `cache_key` を `u64` packed から `struct CacheKey { idx_hash, source_kind,
  adjust_gen, ai_gen }` に変更 (各 u32 で完全に wrap free、`Eq` で比較)

**Phase 2a での扱い**: 現状の 16 bit gen のまま実装。実害が顕在化したら Phase 3 で
bit 再配分または struct 化する。回帰テストに「長時間セッション (4 時間以上スライダー
操作) で正しく cache 無効化されること」を追加するのが望ましい (実用上ほぼ問題ない
範囲だが、unit test なら mock generation で 65,537 回 increment して wrap 動作を
確認できる)。

### 4.2 呼び出し元: `src/ui_fullscreen.rs` (Codex P2 第 4 ラウンド反映)

`render_fullscreen_viewport` の描画分岐は **「アップロード完了済みのときだけ
callback を出す」**形に作る。callback はテクスチャ実体を持たず、`source_key` だけを
持つ (§4.1 の分離)。

```rust
if let Some(pano) = self.panorama_state.as_ref()
    && self.is_panorama_mode_active(fs_idx)
{
    let target_format = self.wgpu_render_state.as_ref()?.target_format;

    // §4.3 の resolve_pano_source 1 箇所で source / cache_key / pixels を解決
    // (Codex P2 第 9 ラウンド反映: stale guard の一貫性を保証)
    let resolution = self.resolve_pano_source(ctx, fs_idx)?;

    // アップロード済みかチェック (App::pano_uploaded、LRU 1)
    // source_key と cache_key の両方一致が必要 (Codex P1 第 6 ラウンド反映)
    let uploaded_ready = self.pano_uploaded
        .as_ref()
        .map(|u| u.source_key == resolution.source_key && u.cache_key == resolution.cache_key)
        .unwrap_or(false);

    if uploaded_ready {
        // 360 shader を出す。callback にも同じ resolution の cache_key を渡し、
        // paint 時に二重チェック (Codex P1 第 8 ラウンド反映)
        let callback = PanoramaShaderCallback {
            source_key: resolution.source_key.clone(),
            cache_key: resolution.cache_key,
            yaw:     pano.yaw,
            pitch:   pano.pitch,
            fov_y:   pano.fov_y,
            aspect:  image_rect.width() / image_rect.height(),
            target_format,
        };
        let shape = egui::Shape::Callback(
            egui_wgpu::Callback::new_paint_callback(image_rect, callback));
        /* paint shape, skip rotation/zoom/pan/spread blocks */
    } else {
        // まだアップロード中: 通常パスで `fs_cache[idx]` を平面表示してロードを待つ
        // (= 360 トグル ON でも、アップロード完了まで equirect が「平らに」見える状態)
        // → 通常の draw_fs_image / draw_fs_spread にフォールバック
    }
}
```

このブロックは `§2.3 表示テクスチャの優先順位` (`docs/display-pipeline.md`) の
**通常パスとは独立した第 0 層**として動かす。回転 / pan / zoom / spread はバイパス
する (360 ビュー自身が yaw/pitch/fov を持つため)。入力は §4.3 の解決結果を使い、
完成済み最終合成、通常補正、AI 結果、元画像の順にフォールバックする。

### 4.3 ピクセル取得経路 (通常解像度: ≤ 8192)

`resolve_pano_source(ctx, fs_idx)` がソース・cache key・settle policy を同時に決め、
呼び出し側はその結果だけを使う。通常表示の final pipeline を画素編集の有無にかかわらず
最優先する。

```rust
pub struct PanoSourceResolution {
    pub source_key: String,
    pub cache_key: u64,
    pub pixels: Arc<egui::ColorImage>,
    pub source_kind: u16, // fs / adjustment / AI / AI 実効時 adjustment / final
    pub settle_policy: PanoramaSettlePolicy,
}

fn resolve_pano_source(&mut self, ctx: &egui::Context, fs_idx: usize)
    -> Option<PanoSourceResolution>
{
    let final_source = self.ensure_final_composite_pixels_with_key(ctx, fs_idx);
    let ai_source = self.ai_will_apply_to(fs_idx)
        .then(|| self.ai_upscale_cache.get(&(fs_idx, self.effective_upscale_bg_mode())))
        .flatten();
    let (pixels, source_kind) = final_source
        .map(|(_, pixels)| (pixels, SOURCE_KIND_FINAL_COMPOSITE))
        .or_else(|| self.adjustment_cache.get(&fs_idx)
            .map(|entry| {
                let kind = if self.ai_will_apply_to(fs_idx) {
                    SOURCE_KIND_AI_ADJUST
                } else {
                    SOURCE_KIND_ADJUST_RAW
                };
                (entry.pixels(), kind)
            }))
        .or_else(|| ai_source.map(|entry| (entry.pixels(), SOURCE_KIND_AI)))
        .or_else(|| self.fs_cache.get(&fs_idx)
            .map(|entry| (entry.pixels(), SOURCE_KIND_FS)))?;
    // cache_key と settle_policy もここで構築
    // ...
}
```

`adjustment_cache` は raw 由来だが、AI が実効的なページでこの fallback が選ばれた場合は
`SOURCE_KIND_AI_ADJUST` を付ける。これは AI pixels の由来を表すのではなく、settle を
`Disabled { AiApplied }` に保つための状態印である。色調補正が有効なのに raw `fs_cache` へ
落ちた過渡期は `compute_settle_policy` が `WaitingForColorAdjustments` を返す。

`ColorImage` の byte 化は `capture::color_image_to_rgba` を使う。ソース選択を upload、
callback、settle の各経路で再実装してはならない。回転 (`rotation_db`) は 360 表示中の
pixels には焼き込まず、360 自身の yaw / pitch / fov を使う。

### 4.4 高解像度キャッシュ (Phase 1 では不使用、案 A 確定で削除)

**案 A (8K base) 確定により、Phase 1 では専用の高解像度キャッシュ層を作らない**。
360 ベーステクスチャは、完成済み final composite、adjustment cache、実効 AI cache、
8K clamp 後の raw cache の順に流用する。画素編集があっても final composite を除外しない。
`pano_source_pixels` / `clamp_panorama_for_gpu` / `max_pano_dim` は実装不要。

ベーステクスチャアップロードの経路:

```rust
// 360 モード ON 時、fullscreen 入場 or fs_cache[idx] が更新されたタイミングで:
let Some(FsCacheEntry::Static { pixels, .. }) = self.fs_cache.get(&idx) else { return; };
let rgba: Arc<Vec<u8>> = Arc::new(crate::capture::color_image_to_rgba(pixels));
// → §4.1 の UploadedPanoTexture に流し、wgpu::Texture::create_texture + queue.write_texture
// 初回のみフレーム落ちの可能性あり (~40-110 ms)、§4.1.1 で実測判断
```

**`fs_cache` のエビクションとの整合**: `fs_cache[idx]` は通常のエビクション
ポリシーで管理されているので、360 ベーステクスチャの「アップロード元」が消える
可能性がある。`UploadedPanoTexture` はアップロード済みの wgpu リソースのみを保持
する (`Arc<Vec<u8>>` は持たない) ため、CPU 側 RGBA が消えても GPU 側は無事。
別画像へのナビ時に再生成されるのは `fs_cache` のエビクションと同じタイミングで OK。

>8K source の高画質経路は **§4.6 の `pano_high_res_source` + settle-refinement**
が担当する (こちらは Phase 2a で本格実装、Phase 1 では枠だけ用意)。

### 4.5 wgpu アダプタリミットの引き上げ (Phase 1 では不要、案 A 確定で削除)

**案 A により wgpu の `max_texture_dimension_2d` を 16384 に上げる必要はなくなった**
(8K base なら wgpu のデフォルト上限 8192 の範囲内)。`src/main.rs:826` の
`WgpuConfiguration` は **触らない**。

将来 Phase 3 等で 16K base を再検討するなら、その時点で本節を復活させる。

### 4.6 settle-refinement 用キャッシュ (Phase 2a、Codex P1/P2 反映)

#### 4.6.0 worker → UI 経路: `FsLoadResult` の拡張 (Codex P1 第 6 ラウンド反映)

現状の `FsLoadResult` (`src/fs_animation.rs:21`) は `Static { ci, source_dims }` /
`Animated` / `Failed` / `DimsOnly` の 4 variants で、ColorImage と寸法しか UI に運ば
ない。`HighResSource::Decoded` を渡す経路がないので、Phase 2a で **新 variant を追加**
する:

```rust
pub enum FsLoadResult {
    DimsOnly { source_dims: [usize; 2] },
    Static { ci: egui::ColorImage, source_dims: [usize; 2] },
    /// 360 パノラマ用 (Phase 2a、SettleReady or SettleApproved の経路)。
    /// 通常の Static と同じ ColorImage を持ちつつ、フル解像度 RGBA を追加で運ぶ。
    /// worker は同じ DynamicImage から tee で両方を生成する (二重デコード回避、§3.6.2)。
    StaticPanorama {
        ci: egui::ColorImage,
        source_dims: [usize; 2],
        high_res: HighResSource,
    },
    Animated(Vec<(egui::ColorImage, f64)>),
    Failed,
}
```

`poll_prefetch` (`src/app.rs:16604`) 側は新 variant を以下のように処理:

```rust
match result {
    FsLoadResult::Static { ci, source_dims } => {
        // 既存処理: fs_cache に Static エントリ作成 (BaseOnly / NeedsUserConfirmation
        // 時に worker が選ぶ variant)
    }
    FsLoadResult::StaticPanorama { ci, source_dims, high_res } => {
        // SettleReady or SettleApproved 時に worker が選ぶ variant
        // 1. fs_cache には Static エントリとして格納 (既存と同じ)
        // 2. pano_high_res_source[metadata_cache_key(idx)] = high_res
        // 3. fs_upload_backlog 経由でテクスチャアップロード (既存と同じ)
    }
    // ... 他 variant
}
```

別 channel ではなく単一 variant にする理由: ロード結果の整合性 (= 8K と HighResSource が
**必ず同じデコード由来**) を型レベルで担保するため。別 channel だと「8K だけ届いて
HighResSource が来ない」ような半端な状態が起こり得る。

**worker が経路を選ぶ判定** (Codex P3 第 10 ラウンド反映で表現修正):

- SettleReady (≤ 200 MP) → `FsLoadResult::StaticPanorama` で 1 回デコード + tee
- SettleApproved (> 200 MP かつ前回承認サイズの 1.25 倍以内) →
  `FsLoadResult::StaticPanorama` で 1 回デコード + tee (NeedsUserConfirmation を経由せず
  最初から SettleApproved 状態)
- NeedsUserConfirmation (> 200 MP かつ前回承認超過 or 未承認) → 通常の
  `FsLoadResult::Static` で 8K base のみ返す
- **ユーザー承認後の追加ロード** → **`FsLoadResult` ではなく専用 channel経由で
  `PanoHighResReady` 別メッセージ型を送る** (下記)。既に `fs_cache` に 8K base が
  あるので、それを上書きしないように HighResSource だけを運ぶ
- BaseOnly (ユーザー却下後) → 通常の `Static` のまま、追加リクエストは発火しない

#### 追加 channel: `pano_high_res_pending` (NeedsUserConfirmation → SettleApproved 経路用)

**設計判断 (Codex P1 第 9 ラウンド反映)**: `FsLoadResult` に
`PanoramaHighResReady` variant を追加して既存 `poll_prefetch` に流す案は、既存実装
(`src/app.rs:17089-17159` 周辺) が `FsLoadResult → FsCacheEntry` 変換 → `fs_cache.insert`
という構造前提のため、特殊扱いが目立つ。代わりに **専用 channel を新設**する:

```rust
// App フィールドに追加
pub(crate) pano_high_res_rx: std::sync::mpsc::Receiver<PanoHighResReady>,
pub(crate) pano_high_res_tx: std::sync::mpsc::Sender<PanoHighResReady>,
pub(crate) pano_high_res_pending: HashMap<String, PanoHighResRequest>,
//                                        ^^^^^^ source_key (metadata_cache_key)

pub struct PanoHighResReady {
    pub source_key: String,
    pub cache_key: u64,         // §4.1.2 と整合、stale 検出用
    pub high_res: HighResSource,
}

pub struct PanoHighResRequest {
    pub started_at: Instant,
    pub cancel: Arc<AtomicBool>,
    pub cache_key: u64,           // リクエスト発火時の cache_key snapshot。
                                  // 重複リクエスト検出 (= 既に同じ cache_key で
                                  // 走っている worker がある場合は新規 spawn しない)
                                  // 用途のみ。stale 検出は poll で **現在の
                                  // resolution.cache_key** との比較で行う
                                  // (Codex P2 第 14 ラウンド反映)
}
```

`FsLoadResult` には `StaticPanorama` のみ追加 (= SettleReady / 初回 SettleApproved
の tee 経路で使用):

```rust
pub enum FsLoadResult {
    DimsOnly { source_dims: [usize; 2] },
    Static { ci: egui::ColorImage, source_dims: [usize; 2] },
    StaticPanorama {        // SettleReady / 初回 SettleApproved (tee)
        ci: egui::ColorImage,
        source_dims: [usize; 2],
        high_res: HighResSource,
    },
    Animated(Vec<(egui::ColorImage, f64)>),
    Failed,
}
```

`poll_prefetch` には触らず、`App::update` の専用 poll を新設:

```rust
fn poll_pano_high_res(&mut self, ctx: &egui::Context) {
    while let Ok(ready) = self.pano_high_res_rx.try_recv() {
        // stale チェック 1: 進行中リクエストが残っているか
        let Some(req) = self.pano_high_res_pending.remove(&ready.source_key) else {
            continue;  // 既にキャンセル済み or 別画像へナビ済み
        };
        // stale チェック 2: キャンセルトークン
        if req.cancel.load(Ordering::Relaxed) {
            continue;  // キャンセル後に到着した結果
        }

        // **現在状態の取得** (Codex P1 第 14 ラウンド反映、P2 第 14 反映):
        // ready.cache_key vs req.cache_key の比較は worker 内で生成された同一値
        // なので stale 検出にならない。**現在の resolution.cache_key と比較**する
        // 必要がある。さらに成功パスでも state / policy の再確認が必要 (ユーザーが
        // ロード中に「8K でよい」を押したケースの巻き戻り防止)。
        let current = self.fs_idx_of_source_key(&ready.source_key)
            .and_then(|fs_idx| self.resolve_pano_source(ctx, fs_idx).map(|r| (fs_idx, r)));
        let Some((fs_idx, resolution)) = current else {
            continue;  // 別画像へナビ済み等で resolution が取れない
        };
        let state = self.pano_quality_state.get(&resolution.source_key).cloned();
        // **再要求対象は SettleApproved のみ** (Codex P2 第 15 反映):
        // NeedsUserConfirmation は「まだ承認していない」状態で、バナーが残っている。
        // ユーザーがバナーで押せば start_pano_high_res_load が改めて発火するので、
        // ここで自動再要求するのは強すぎる
        let user_still_wants_high_res = matches!(state,
            Some(PanoramaQualityState::SettleApproved)
        );
        let settle_will_use_it = state.as_ref()
            .map(|s| settle_enabled(s, &resolution.settle_policy))
            .unwrap_or(false);

        // stale チェック 3: 現在 cache_key と一致するか + state/policy 整合
        if ready.cache_key != resolution.cache_key {
            // 補正 / AI 切替で cache_key が変わった = ready の結果は古い
            //
            // ⚠ **重い処理**: raw high-res RGBA は補正状態に依存しないので本来再利用
            // 可能だが、現設計では cache_key 別に save する pool 構造を持たない
            // (`HashMap<source_key, HighResSource>` で source_key 単位)。
            // Phase 2a では単純化のため再デコード許容、Phase 3 で pool 化検討
            // (§3.6.6 拡張案、Codex P2 第 11/12 ラウンド指摘点)
            //
            // **再要求 gate (Codex P1 第 13 反映)**: settle_will_use_it が false なら
            // (= 色調補正 cache の準備待ち) 再要求しない。準備完了後に
            // 同じ HighResSource を利用できる
            if user_still_wants_high_res && settle_will_use_it {
                self.start_pano_high_res_load(fs_idx, resolution.cache_key);
            }
            continue;
        }

        // **成功パスでも state / policy を再確認** (Codex P1 第 14 ラウンド反映):
        // ロード中にユーザーが「8K でよい」を押した、色調補正 cache が
        // 一時的に未完成になった等で
        // settle が不要になっているケースを検出する。
        // - BaseOnly → state を上書きしない、結果も破棄
        // - settle_will_use_it == false → state を上書きしない、結果は HighResSource
        //   に格納するが settle render は呼ばれない (memory cost のみ。将来 OFF→ON 切替
        //   時に再利用できる可能性があるので保持する判断)
        if matches!(state, Some(PanoramaQualityState::BaseOnly)) {
            // ユーザーが明示的に拒否 → 何もせず結果破棄
            continue;
        }

        // settle 用ソースを格納 (fs_cache は触らない)
        self.pano_high_res_source.insert(ready.source_key.clone(), ready.high_res);

        if settle_will_use_it {
            // settle が動作する状態なら state を SettleApproved に確定
            self.pano_quality_state.insert(
                ready.source_key.clone(),
                PanoramaQualityState::SettleApproved,
            );
            // バナーを閉じる
            self.dismiss_pano_confirmation_banner(&ready.source_key);
            ctx.request_repaint();
        }
        // settle_will_use_it == false (= 色調補正 cache の準備待ち) のとき:
        //   state は触らない (= NeedsUserConfirmation のまま、バナーも残す)。
        //   cache 完成で settle policy が変わると HighResSource が即活用される
    }
}
```

**再要求の意義**: ユーザーが「高品質」を押した後、デコード中 (数秒) に補正を切り替えた
ケースで、古い cache_key の結果が捨てられても **次の load が自動で発火**する。これで
「ボタン押したのに永久に高品質にならない」UX バグを回避。

**raw RGBA の再利用**: 現状の `HighResSource::Decoded` は補正/AI 適用前の生 RGBA を
保持しているので、本来は cache_key 差で捨てる必要はない。ただし**現設計では `cache_key`
別に save する pool 構造を持たない** (`HashMap<source_key, HighResSource>` で source_key
単位)。Phase 3 で「補正切替時の raw RGBA 再利用」を最適化する余地あり (`HashMap<source_key,
HighResSourcePool>` 化等、§3.6.6 拡張枠)。Phase 2a では再デコードを許容する。

これにより:

- 既存 `poll_prefetch` / `fs_cache.insert` の構造に手を入れない
- `fs_upload_backlog` (UI スレッドアップロードペーシング) と完全に独立
- ユーザー承認後の追加デコードでも **8K base テクスチャの再アップロードは発生しない**
- キャンセル: 別画像へナビ / フルスクリーン退出時に `pano_high_res_pending` 内の
  全エントリの `cancel` を立てる + map をクリア

#### 4.6.1 App フィールド (解像度ゲート版)

§3.6 の settle-refinement を実装するための App フィールド。**キーは String** に統一:

```rust
pub(crate) pano_high_res_source: HashMap<String, HighResSource>,
//                               ^^^^^^ metadata_cache_key
pub(crate) pano_refinement: Option<PanoramaRefinement>,
pub(crate) pano_quality_state: HashMap<String, PanoramaQualityState>,
// 画像ごとの判定結果 (解像度ゲート + ユーザー判断)
pub(crate) pano_session_approved_max_pixels: u64,
// バナーで「今後も高品質で開く」を選んだ時の最大ピクセル数を記憶。
// 前回承認 × 1.25 を超える画像では再確認 (§3.6.2)。再起動で 0 にリセット。

pub enum HighResSource {
    Decoded { rgba: Arc<Vec<u8>>, w: u32, h: u32 },
    // Phase 3 で JpegBytes バリアントを追加検討 (§3.6.6)
}

pub enum PanoramaQualityState {
    SettleReady,                 // ≤ 200 MP: 自動承認、settle ON 確定
    NeedsUserConfirmation {      // > 200 MP: バナー表示中
        source_pixels: u64,
        est_ram_gb: f32,
    },
    SettleApproved,              // > 200 MP かつユーザーが「フル解像度(高画質)」選択
    BaseOnly,                    // > 200 MP かつユーザーが「最大 8K(軽量)」選択
}

pub struct PanoramaRefinement {
    pub source_key: String,                  // 対象画像の metadata_cache_key
    pub settle_since: Option<Instant>,
    pub last_pose: (f32, f32, f32),          // (yaw, pitch, fov) snapshot
    pub last_cache_key: u64,                 // ★ Codex P1 第 12 反映: 補正/AI 変化検出用
    pub rendering: Option<RenderingHandle>,  // channel-based、§4.6.2
    pub overlay_tex: Option<wgpu::Texture>,
    pub overlay_pose: Option<(f32, f32, f32)>,    // 描画時に pose 一致確認
    pub overlay_cache_key: Option<u64>,           // ★ 描画時に cache_key 一致確認
                                                  //   両方一致しないと overlay drop
    pub overlay_fade_start: Option<Instant>, // 150ms フェードイン
}

pub const PANO_SETTLE_MAX_PIXELS: u64 = 200_000_000; // 200 MP
```

settle 機能が有効になる判定 (§3.6.2.1 で再掲、両軸 AND):

```rust
fn settle_enabled(state: &PanoramaQualityState, policy: &PanoramaSettlePolicy) -> bool {
    matches!(policy, PanoramaSettlePolicy::EnabledFromRaw
                   | PanoramaSettlePolicy::EnabledWithColorAdjustments { .. })
        && matches!(state, PanoramaQualityState::SettleReady
                         | PanoramaQualityState::SettleApproved)
}
```

#### 4.6.2 RenderingHandle: channel ベース (Codex P2 反映)

`std::thread::JoinHandle::try_join` は標準 API に存在しない (`is_finished()` のみ)。
既存 mIV のワーカーパターン (CLAUDE.md「並列処理」+ `thumb_loader` 等) と揃えて
**mpsc channel で結果を返す** 設計に修正:

```rust
pub struct RenderingHandle {
    pub cancel: Arc<AtomicBool>,
    pub rx: std::sync::mpsc::Receiver<SettleRenderResult>,
    pub started_at: Instant,
    pub for_source_key: String,
    pub for_pose: (f32, f32, f32),     // 開始時の pose snapshot
    pub for_cache_key: u64,            // ★ Codex P1 第 12 反映: 開始時の cache_key
                                       //   stale (補正/AI 変更) を検出
}

pub struct SettleRenderResult {
    pub source_key: String,            // どの画像に対する結果か
    pub pose: (f32, f32, f32),         // どの pose で計算したか
    pub cache_key: u64,                // ★ どの source_kind / gen で計算したか
    pub rgba: Vec<u8>,                 // 完成オーバーレイ (4K viewport)
    pub width: u32,
    pub height: u32,
}
```

**poll 経路** (`App::update` 中で、Codex P1 第 12 ラウンドで cache_key 追加):

```rust
if let Some(refinement) = self.pano_refinement.as_mut() {
    if let Some(handle) = refinement.rendering.as_mut() {
        match handle.rx.try_recv() {
            Ok(result) => {
                refinement.rendering = None;
                // 古い結果の破棄: source_key / pose / cache_key の 3 つすべて一致が必要
                // - source_key 一致: 別画像へナビしていない
                // - pose 一致: ユーザーが動いていない
                // - cache_key 一致: 補正 / AI / post_filter 状態が変わっていない
                if result.source_key == refinement.source_key
                    && result.pose == refinement.last_pose
                    && result.cache_key == refinement.last_cache_key
                {
                    self.upload_settle_overlay(ctx, result);
                    // upload 完了時に refinement.overlay_pose / overlay_cache_key を
                    // result の値で更新する
                }
                // 不一致なら静かに捨てる (= 描画状態が変わった)
            }
            Err(TryRecvError::Empty) => {} // まだ実行中
            Err(TryRecvError::Disconnected) => {
                // ワーカーが panic or cancel で abort
                refinement.rendering = None;
            }
        }
    }
}
```

**描画時の overlay stale 判定 (新規)**: `ui_fullscreen.rs` の 360 描画分岐で
`overlay_tex` をブレンドする際、**現在の `resolution.cache_key` と
`overlay_cache_key` が一致しない場合は overlay を drop して 8K base 単独表示に戻す**。

```rust
if let Some(refinement) = self.pano_refinement.as_ref() {
    let now_pose = (pano.yaw, pano.pitch, pano.fov_y);
    let now_cache_key = resolution.cache_key;
    let overlay_ok = refinement.overlay_tex.is_some()
        && refinement.overlay_pose == Some(now_pose)
        && refinement.overlay_cache_key == Some(now_cache_key);
    if overlay_ok {
        // 8K base と overlay を alpha ブレンド
    } else {
        // overlay が無い or stale → 8K base 単独
        // (stale overlay は次の poll サイクルか settle 再起動で drop)
    }
}
```

これにより **「静止中に補正を切り替えた瞬間に古い高品質 overlay が残る」バグを防ぐ**。
補正切替で `cache_key` が変化 → 次フレームで stale 判定 → overlay 即非表示 → 8K base
(補正済み) のみ表示。settle タイマー再起動で新 cache_key の overlay が再生成される。

### 4.6.3 ライフサイクル (解像度ゲート + ユーザー確認版)

1. **ロード時** (`start_fs_load` の 360 分岐):
   - ヘッダ読みで原寸取得 → 解像度 + approved_max_pixels から `PanoramaQualityState`
     初期値を決定 (§3.6.4)
   - **SettleReady** (≤ 200 MP) → image crate でデコード 1 回 + tee で
     `FsLoadResult::StaticPanorama` 返却
   - **SettleApproved** (大画像かつ前回承認の 1.25 倍以内) → 同上
   - **NeedsUserConfirmation** (大画像、未承認) → 通常 `FsLoadResult::Static`
     のみ返却。`pano_high_res_source` にはエントリを作らない
   - **BaseOnly** (大画像、ユーザー却下後) → 同上
2. **フルスクリーン入場 + 360 アクティブ化**:
   - `pano_refinement = Some(default)` で有効化
   - §4.1.1 の経路で 8K base テクスチャをアップロード
   - `PanoramaQualityState::NeedsUserConfirmation` ならフルスクリーン上部にバナー表示
3. **ユーザーがバナーで選択した場合**:
   - 「フル解像度(高画質)」: 状態 → `SettleApproved`、`start_pano_high_res_load(idx)`
     で追加の worker を起動してフルデコード再実行 → `pano_high_res_source` 格納 →
     settle 有効化。チェックボックス ON なら `pano_session_approved_max_pixels =
     source_pixels` を記録
   - 「最大 8K(軽量)」: 状態 → `BaseOnly`、何もしない (settle 無効のまま)
   - **以降は画面下部 status indicator 内の `[高画質に切替]` / `[8K 軽量に切替]`
     ボタンでいつでも切替可能** (§3.6.4 末尾の切替導線参照、`SettleApproved` ↔
     `BaseOnly` に直接遷移、`pano_session_approved_max_pixels` は bump しない)
4. **`App::update` 末尾**: settle 状態を更新 (yaw/pitch/fov が前フレームと一致したら
   タイマー進行、変化があれば reset + 進行中レンダ cancel + overlay drop)。
   `settle_enabled(state, &resolution.settle_policy) == false` なら settle 処理スキップ
   (色調補正 cache がまだ無い過渡期だけ)
5. **タイマー到達 (500 ms) + settle_enabled**: バックグラウンドレンダ起動。
   `std::thread::spawn` で `render_settle_overlay` を呼び、結果は mpsc 経由で返す

   **HighResSource 既存時の自動復帰**: `pano_high_res_source[source_key]` が既に
   存在し、色調補正 cache の準備が完了した場合は、**追加 worker 起動なしで
   settle render が走る**。
   復帰条件のチェーン:
   - `adjustment_cache` が完成する
   - 次フレームの `resolve_pano_source` で `settle_policy` が
     `EnabledWithColorAdjustments` に遷移
   - `settle_enabled(state, &policy)` が true に変化
   - settle タイマーが定常状態なら 500 ms 後に発火、新しい cache_key で
     `render_settle_overlay` が起動 (worker は `pano_high_res_source` の Arc<Vec<u8>>
     を参照するだけなので、デコード再実行は不要)

   これにより、補正準備中に高解像度ロードが完了しても追加デコードせず再利用できる。
6. **次以降のフレーム**: `handle.rx.try_recv()` で完了確認。完了したら
   `upload_settle_overlay` でテクスチャアップロード (4K = ~33 MB なので 1 回で
   問題なし)
7. **描画**: `ui_fullscreen.rs` の 360 分岐で、`overlay_tex` があり `overlay_pose` が
   現 pose と一致するならフェード進捗に応じて **8K base と alpha ブレンド**して描画。
   pose が変わった瞬間に overlay を drop して 8K base 単独描画に戻る
8. **退出**: フルスクリーン退出 / フォルダ切替 / 別画像へナビで `pano_refinement = None`、
   `pano_high_res_source` の該当エントリ remove (SettleReady / SettleApproved のみ
   エントリあり)。**フル RGBA (最大 2.15 GB) が drop されることをテストで確認**

`pano_high_res_source` は 1.35 GB クラスになりうるので、**360 モード退出時に drop する**
ことを設計レビューで必ず確認する (CLAUDE.md「永続データ・スキーマ変更時の判断」と
同じ意識でメモリリークを検知)。

---

## 5. App 状態と入力

### 5.1 新規 App フィールド

```rust
pub(crate) panorama_state: Option<PanoramaState>,    // fullscreen 内のみ Some

pub struct PanoramaState {
    pub yaw:     f32,                     // [-π, π], 初期 0 (or GPano hint)
    pub pitch:   f32,                     // [-π/2 + ε, π/2 - ε]
    pub fov_y:   f32,                     // 初期 1.2 rad ≒ 69°
    pub drag_active: bool,
    pub last_pointer: Option<egui::Pos2>,
    pub initial_yaw: f32,                  // reset 用 GPano hint
    pub initial_pitch: f32,
}

pub(crate) xmp_panorama_info: HashMap<String, Option<XmpPanoramaInfo>>,
// キーは metadata_cache_key(idx) (§2.4)、idx ベースだと並び替えで破綻
```

V / 上部 × による明示 OFF とフルスクリーン退出時に `panorama_state = None`。
通常ナビでは state を保持し、`is_panorama_mode_active(fs_idx)` が検出結果のあるページだけを
active にする。非 360 ページでは非アクティブになり、同じセッションで 360 ページへ戻ると
yaw / pitch / fov を引き継ぐ。
360 モードと相互排他: compare / erase / analysis / spread / free_rotation は
360 アクティブ中に `early-return` で抑止する。

### 5.2 入力ハンドリング (Codex P2 反映)

矢印によるフルスクリーン移動と Esc によるフルスクリーン終了は維持する。一方、360 中の
wheel は修飾キーに関係なくすべて FOV 操作として consume し、意図しないページ送りを防ぐ。

| 入力 | 360 モード中の動作 | 既存動作との関係 |
| --- | --- | --- |
| 左ドラッグ | `pointer.delta()` を `(d_yaw, d_pitch) = (-Δx * s, +Δy * s)` (`s = fov_y / viewport_h`) で加算 | 既存: 静止画フルスクリーンで左ドラッグはズーム時のパン。360 中は yaw/pitch に転用 (panorama 自身が pan の意味を持つため自然) |
| **Wheel (修飾キー不問)** | FOV を加減算。`fov_y = clamp(fov_y * exp(-wheel * 0.0015), 0.2, 2.6)` | 既存: 画像ズーム / 前後送り。360 中は **Ctrl 有無に関わらず常に FOV 操作に転用** (2026-05 ユーザー要望: 拡大縮小のつもりでホイールを回して画像が切り替わる事故を防ぐ)。前後ナビは矢印キーで |
| 矢印キー | **奪わない** (= フルスクリーンナビ / 動画 seek) | 既存挙動維持 |
| <kbd>Esc</kbd> | **奪わない** (= フルスクリーン終了) | フルスクリーン全体を閉じる場合は Esc |
| **上部バーの × ボタン** | **360 モード OFF** (フルスクリーンは維持) | 2026-05 ユーザー要望で挙動を切替。元設計の「× = フルスクリーン終了」は **360 モード OFF 中のみ**。360 ON 中は × で 360 解除、Esc で全体終了 |
| ダブルクリック (画像領域内) | yaw/pitch/fov を初期値 (or GPano hint) に戻す | 既存: 動画はトグル再生 (要競合確認)、静止画ではフィット戻し |
| 既存のフルスクリーン操作 (BS / Ctrl+↑↓ / Enter / 矢印) | 通常どおり | フォルダ / ファイル移動は 360 モードフラグを保持。新ファイルが equirect なら yaw/pitch 引き継ぎ、非 equirect なら自動 OFF。**Wheel は奪う** (上記参照) |

**pitch のクランプ**: `±(π/2 - 0.001)` を上限にして極を直視できないようにする
(asin の数値誤差で天井 / 床のテクセルが暴れるのを防ぐ)。

**キーボードによる yaw/pitch 操作は実装しない** (矢印が前後ナビと衝突するため)。
マウス駆動の機能と割り切る。

### 5.3 トグル UI (Codex P1 反映 + 2026-05 ユーザー要望反映)

主導線は **ツールバー / 上部ホバーバーの「360°」ボタン** (OFF→ON 経路のみ) + **× ボタン**
(ON→OFF 経路)。

- **画像フルスクリーンの上部ホバーバーに 360 ボタンを追加** (`ui_fullscreen/draw_icons.rs`
  の既存アイコンに 1 つ追加)。クリックで 360 モード ON
- ボタンの**表示と有効化条件**:
  - **360 モード ON 時は 360 ボタンを隠す** (2026-05 ユーザー要望、× が 360 解除を
    兼ねるため。元設計の「常時表示 + 強調背景」は廃止)
  - Auto (GPano `UsePanoramaViewer=True` 検出): 通常ボタン + ツールチップ「360° 画像 (XMP 検出) [V]」
  - Hint (GPano `ProjectionType` のみ or アスペクト 2:1): 通常ボタン + ツールチップ「360° ビューワーで開く (アスペクト比から推定) [V]」
  - 検出なし: **disabled 表示** (グレーアウト、押せない)、ツールチップ「360° 画像ではありません」
- **360 モード OFF 経路** (2026-05 ユーザー要望):
  - **上部バーの ×**: 360 モード OFF (フルスクリーンは維持)。ツールチップ
    「360° モードを抜ける (フルスクリーンを閉じるには Esc)」
  - <kbd>V</kbd> キー: トグル (ON ↔ OFF 両方)
  - <kbd>Esc</kbd> キー: フルスクリーン全体を閉じる (= 360 も巻き取られて終了)
- **自動 ON はしない** (フィードバック反映で廃止)。代わりに `open_fullscreen` /
  `poll_metadata_load` / `poll_prefetch` で「V キーで 360° ビューワー」案内
  トーストを 1 度だけ表示。ユーザーは V キーまたはボタンクリックで明示的にトグル

**キーボードショートカット (フィードバック反映で V キー採用)**:
画像フルスクリーン中の <kbd>V</kbd> で 360 モードトグル。`detect_panorama(fs_idx).is_some()`
のとき (= 360 候補画像) だけ反応し、非対応画像で押しても no-op。消しゴムモード中は
`ui_erase` 側が V (Vertical line tool) を先に consume するので衝突しない (mode-scoped 共存)。

検討経緯 (採用しなかった候補):

- <kbd>P</kbd>: 静止画フルスクリーンで `set_folder_thumb_pin` 既存使用
  (`src/ui_fullscreen.rs:2904-2910`)
- <kbd>3</kbd>: PDF の見開き設定で既存使用、衝突回避
- <kbd>S</kbd>: 画像スライドショー / 動画タイルモードで既存使用
- <kbd>Shift</kbd>+<kbd>P</kbd>: 衝突しないが「Sphere」のイニシャル<kbd>V</kbd>のほうが
  覚えやすいと判断 (View / VR の連想)

---

## 6. UI / UX

### 6.1 トリガ (フィードバック反映で自動 ON 廃止)

**360 モードに入る経路は 2 つだけ** (どちらもユーザーの明示操作):

1. **V キー押下**: 画像フルスクリーン中で `detect_panorama(fs_idx).is_some()`
   なら `toggle_panorama_mode(fs_idx)`。消しゴムモード中は ui_erase が先に
   V を consume するので衝突しない (mode-scoped)
2. **ホバーバー 360 ボタンクリック**: 同上

**案内トースト**: GPano XMP 検出 / アスペクト 2:1 検出時に **「V キーで 360°
ビューワー」を一度だけ表示** (§2.3)。Auto / Hint で文言を分岐。フラグ
`pano_toast_shown_for_current_fs` で同一フルスクリーンセッション内の重複を抑止。
表示タイミングは:
- `open_fullscreen`: XMP / fs_cache が cache 済みなら即発火
- `poll_metadata_load`: 到着 XMP が現フルスクリーン source_key と一致したら補完
- `poll_prefetch`: fs_cache Static 完了で aspect 2:1 を新たに判定可能になったら補完

### 6.2 表示中の UI 要素

- **上部ホバーバーの 360 ボタン** (主導線) + ON 中はリセットボタン併設
- **左上に「YAW xxx° PITCH yyy° FOV zzz°」の小さなオーバーレイ** (操作中だけ表示、
  1 秒フェード。Phase 2b)
- **右下にミニコンパス** (Phase 2b、北 = yaw=0 を示す矢印)
- ステータスバー (動画 HUD パターン) は 360 モードでは隠す

### 6.3 退出条件

- **上部バーの ×** → 360 モード OFF (フルスクリーンは維持、2026-05 ユーザー要望)。
  元設計の「× = フルスクリーン終了 / 360 解除は専用ボタン」はユーザーが見つけにくい
  と判明したため、× を 360 解除に転用。
- <kbd>V</kbd> キー → トグル (360 ON → OFF 復路)
- <kbd>Esc</kbd> / システム終了 → **既存どおりフルスクリーン全体を閉じる**。360 中に
  Esc を押した場合は 360 + フルスクリーン同時終了 (Esc は途中で 360 だけを止めない)
- 360 でない画像へナビした際は、**`panorama_state` を保持しつつ非アクティブ
  化**。次に equirect 画像に戻ったら yaw / pitch / fov を引き継いで再開する。
  (これは「セッション内の 360 表示記憶」程度の軽い扱い。永続化はしない)

### 6.4 補正 / AI との関係

- 360 度ビューは通常表示と同じ全体補正、AI upscale / denoise、自動補正、
  ポストフィルタ、スマートシャープと、消しゴム・隠蔽加工・補正レイヤーを 8K base に反映する。
- AI / 自動補正 / ポストフィルタ / スマートシャープが有効なときは settle を無効にし、
  base の表示を維持する。純粋な色調補正では settle 側へ同じ補正を再適用する。
- 画素編集だけがある場合は settle を有効のままにするため、高画質へ切り替わると編集が
  見えなくなる。この挙動は仕様として受け入れ、画素編集があり settle が実際に動く状態だけ
  ステータスの 1 行下へ「高画質化には編集は反映されません」と表示する。入場トーストは出さない。
- 360 中は消しゴム・隠蔽加工・補正レイヤー・テキスト注釈の各編集モードへ入れない。
  すべて `fullscreen_edit_mode_entry_allowed` を通し、理由付き no-op を表示する。

---

## 7. 既存システムとの相互作用 (チェックリスト)

`display-pipeline.md §4.1` のテーブルを 360 用に追補する想定。

| 機能 | 360 モード中の挙動 | 理由 |
| --- | --- | --- |
| rotation_db (R/L キー) | 既存どおり (R/L は奪わない)、ただし**描画には反映しない** | 360 が独自向きを持つ。キー操作は受け付けて DB に保存はする (360 OFF で復帰) |
| free_rotation (Ctrl+ドラッグ) | 描画に反映しない | 同上 |
| spread (見開き) | 無効 (forced single) | 2 枚を並べる意味がない |
| zoom (Ctrl+ホイール) | **FOV 操作にモード依存で切替** | "拡大方向の操作" という直感を保つ |
| pan (左ドラッグ) | yaw/pitch ドラッグに転用 | panorama 自身が pan の意味を持つ |
| ルーペ (M / Shift) | 360 中は無効 | 球面投影中は平面画像の cursor→UV 変換と一致しないため |
| 前後送り (通常 Wheel) | 360 中は無効。Wheel は修飾キー不問で FOV 操作に転用 | 意図せず別画像へ移動する事故を防ぐ |
| 矢印キー | **既存どおりフルスクリーンナビ / 動画 seek** | 奪わない |
| <kbd>Esc</kbd> | **既存どおりフルスクリーン全体を閉じる** | 360 解除はトグルボタン |
| compare (X→C/Shift+C/Alt+C) | 360 中は無効 | パイプライン分岐の単純化 |
| analysis | 360 中は無効 | 360 は閲覧専用の機能制限モード |
| 消しゴム | 8K base に反映するが、高画質化には反映しない。編集モードへ入れない | settle は原本を再デコードするため。高画質化が動くときだけ注意書きを表示 |
| 隠蔽加工（モザイク等） | 8K base に反映するが、高画質化には反映しない。編集モードへ入れない | 同上 |
| 補正レイヤー | 8K base に反映するが、高画質化には反映しない。編集モードへ入れない | 同上 |
| テキスト注釈 | 編集モードへ入れない | 360 中の編集入口を共通化する |
| AI upscale / denoise | 反映する。settle は無効 | AI 結果を原本 sample から再現できないため |
| プリセットの単純色調補正 | 適用する | settle 側で同じ色調補正を再適用できるため |
| 自動補正 / スマートシャープ / ポストフィルタ | 反映する。settle は無効 | 8K base を保ち、高解像度差し替えだけを止める |
| スライドショー | 360 入場時に停止 | 連続 equirect の pose 連動は未実装の将来候補 |
| アニメーション (GIF/APNG) | **将来検討**。当面は静的フレームのみ (`FsCacheEntry::Animated` は 360 対象外) | GIF 360 はレアケース |
| 動画 | **対象外** (FFmpeg 経路で 360 動画再生は別議論) | スコープ外 |
| ZIP 内画像 | サポート (パス解決は `metadata_cache_key` 経由) | パイプライン上は同じ |
| PDF ページ | 2:1 aspect fallback による 8K base 表示のみ | GPano XMP 抽出と high-res settle は行わず `BaseOnly` |

---

## 8. 実装フェーズ

### Phase 1: MVP (1 週程度、案 A 確定で大幅簡素化)

> **実装記録**: 下の code 項目は実装済み。工数と Phase 分けは当時の計画値。
> 最後の実素材テストだけは実施記録を確認できないため未チェックのまま残す。

**スコープ**: 8K base + WGSL equirect 描画 + UI トグル + GPano XMP 検出。
**含まれないもの (Phase 2a 以降)**: settle-refinement / フル RGBA 保持 /
ミニコンパス / 慣性ドラッグ。mipmapは共通GPU生成器の導入後に8K baseへ追加済み。

- [x] `src/xmp_reader.rs` に GPano パーサ追加: `read_panorama_info(path)` +
      `read_panorama_info_from_bytes(bytes)` + `XmpPanoramaInfo` 型 (Codex P2 反映)
- [x] `start_metadata_load` ワーカーで GPano XMP を読み、結果を
      `App::xmp_panorama_info: HashMap<String, Option<XmpPanoramaInfo>>` に格納
      (キーは `metadata_cache_key`、Codex P2 反映)
- [x] App 状態: `panorama_state` / `xmp_panorama_info` / `pano_uploaded` 追加
- [x] **`adjustment_generation: HashMap<String, u32>` 必須追加** (Codex P1 第 12 反映):
      `metadata_cache_key` をキーに u32 世代カウンタ。**粒度: 該当 source_key で
      +1** (adjustment_cache の idx 単位 clear と整合、Codex P3 第 17 反映)。
      **bump 箇所**:
      (a) `apply_sync_adjustment` で `adjustment_cache` に書き込む直前
      (b) `clear_adjustment_caches(idx)` / `clear_all_adjustment_and_ai_caches(idx)` /
          バルク補正クリア
      (c) AI 完了後の補正自動適用 (`apply_sync_adjustment` line 15458)
- [x] **`ai_upscale_generation: HashMap<String, u32>` 必須追加** (Codex P1 第 12 反映):
      同じく u32 世代カウンタ。**粒度: AI cache 全体 clear 時は全 entry を +1**
      (ai_upscale_cache の全体 clear と整合、Codex P3 第 17 反映)。
      ⚠️ **`.clear()` ではなく `for v in values_mut() { *v += 1 }`** にすること
      (clear すると過去 cache_key と衝突するリスク、§3.6.2.2 参照)。
      **bump 箇所**:
      (a) `ai_upscale_cache.insert` (AI 完了時、`src/app.rs:15463`) → 該当 source_key を +1
      (b) `ai_upscale_cache` の clear / モデル切替 / `bg_mode` 切替 →
          `bump_all_ai_generations()` で全 entry +1
- [x] **両 generation の不変条件**: 該当 idx の cache が無効化された時点で必ず bump。
      cache 内容が変わるのに generation が同じだと cache_key 衝突で stale guard が機能しない
- [x] **`clear_caches_and_bump_generation(idx)` ヘルパの導入** (Codex P2 第 16 +
      P3 第 17 反映):
      cache clear と generation bump を **同じヘルパで一括処理**、かつ粒度を実コード
      と整合させる:
      - `clear_all_adjustment_and_ai_caches(idx)` を呼ぶ (adjustment は idx 単位、
        AI は全体 clear)
      - `adjustment_generation[source_key] += 1` (idx 単位)
      - `bump_all_ai_generations()` で全 AI gen entry を +1 (全体)
      AI ON/OFF / モデル切替 / bg_mode 切替の全経路でこのヘルパを呼び出し
- [x] 検出ヘルパ `App::detect_panorama(fs_idx) -> Option<PanoramaTrigger>` (Auto / Hint / None)
- [x] `src/panorama_wgpu.rs` (compare_wgpu.rs をテンプレートに WGSL を §3.3 のものに差替え、
      §4.1 のキャッシュキー設計 + `Arc::ptr_eq` 保険、Codex P1 反映)
- [x] **callback は `UploadedPanoTexture` 参照、テクスチャ実体を持たない** (§4.1)
- [x] アップロード経路: `resolve_pano_source` が選んだ final / AI / adjustment / raw pixels を `color_image_to_rgba` 変換 →
      raw wgpu テクスチャ生成 → `pano_uploaded` に格納 (§4.3 / §4.1.1)
- [x] `ui_fullscreen.rs` で 360 モード分岐 (描画 + 入力)
- [x] **左ドラッグで yaw/pitch、修飾キー不問の Wheel で FOV、矢印 / Esc は奪わない**
      (§5.2、2026-05 フィードバック反映)
- [x] **トグル経路: ホバーバーの 360 ボタン + V キー** (フィードバック反映で
      V キー採用、§5.3)。検出済み (= `detect_panorama(fs_idx).is_some()`) のときだけ反応
- [x] **360 モード中の機能制限**: メタデータパネル / 補正パネル / 分析パネル / 比較 /
      見開き / VST3 GUI を全て抑止。上バーは × / ウィンドウ切替 / 360 ボタンのみ
      (フィードバック反映、`is_panorama_mode_active(fs_idx)` で判定)
- [x] 補正・AI テクスチャの優先順位を 360 入力に反映 (display-pipeline.md §2.3 と整合、
      source_kind を §4.3 のとおりキーに焼き付け)
- [x] **検出 (`detect_panorama`)**: GPano `UsePanoramaViewer=True` → Auto、
      GPano `ProjectionType` のみ or アスペクト 2:1 → Hint。**自動 ON は廃止**
      (フィードバック反映)、代わりに `open_fullscreen` / `poll_metadata_load` /
      `poll_prefetch` から `maybe_show_panorama_hint_toast` で「V キーで 360°
      ビューワー」案内トーストを 1 度だけ表示 (`pano_toast_shown_for_current_fs`
      フラグで重複抑止)
- [x] **ホバーバーの 360 ボタンは常時表示**、非対応画像では disabled (グレーアウト)
      (フィードバック反映で「検出時のみ表示」から変更、§5.3)
- [x] サムネは equirect のまま表示 (グリッドでは平面のままで OK)
- [ ] テスト: ChatGPT 出力サンプル (2K) + Insta360 X3 等の 11K 画像 (8K に縮小されて
      表示されることを確認) + ZIP 内 equirect + 補正 / AI 切替時の再アップロード確認

**Phase 1 で実装しなくてよくなったもの (案 A 確定により)**:

- ~~`main.rs` の `WgpuConfiguration` に 16384 を要求~~ (8K で済むので不要)
- ~~`clamp_panorama_for_gpu` ヘルパの新規追加~~ (`clamp_dynamic_for_gpu` を流用)
- ~~`pano_source_pixels` キャッシュ層~~ (`fs_cache` 直接流用)
- ~~`max_pano_dim` 起動時取得~~ (8192 固定)
- ~~古い iGPU フォールバック~~ (8K は全 GPU でサポート)

### Phase 1.5: 部分 FOV equirect (実装済み)

GPano `CroppedArea*` 宣言で「フル球面の一部しか覆っていない」equirect 画像
(DSLR + nodal panhead 撮影で天頂・地面が抜けているケース等) を正しく球に貼る。
**Phase 2a settle 実装前に組み込み** することで、settle の CPU sampler 設計が
最初から UV 変換込みになり、後で signature 拡張する手戻りを避けた。

実装範囲:

- [x] `panorama::PanoUvTransform { u_offset, v_offset, u_scale, v_scale }` 型
- [x] `PanoUvTransform::from_gpano(info)` で `FullPano*` / `CroppedArea*` から UV 変換を計算
  - 必須フィールド欠落 / 範囲外 → `None` (= IDENTITY fallback)
  - `CroppedAreaLeft/TopPixels` 未指定は中央寄せと解釈
  - 差が 0.5% 以下なら identity に丸めて無駄な UV 変換を回避
- [x] WGSL `Params` 構造体を `(pose: vec4, crop: vec4)` の 2 つに拡張、uniform 32 bytes
- [x] WGSL fragment で `texture_uv = (sphere_uv - crop.xy) / crop.zw` を最終 UV に
- [x] `PanoramaShaderCallback.uv_transform: PanoUvTransform` フィールド
- [x] `App::compute_pano_uv_transform(fs_idx)` で XMP から導出 (`is_equirectangular()` ガード
      で非 equirect は IDENTITY、Codex P3 第 21 ラウンド反映)
- [x] `try_paint_panorama` で callback に焼き付け
- [x] テスト: identity / DSLR 部分 FOV 例 / 中央寄せ default / 必須欠落 / 範囲外 / 微差丸め

**欠落領域の埋め方** (Codex P2 第 21/22/23 ラウンドで段階的に精度向上):

- **Sampler 設定**: 2つのbind groupを同じtexture/uniformに対して作り、描画時に選ぶ。
  - フル equirect / 垂直cropのみ: `U=Repeat` / `V=ClampToEdge`。経度シームを自然にwrapする。
  - 水平cropあり: `U=ClampToEdge` / `V=ClampToEdge`。選択された低LOD mip自身の端でclampし、
    反対端の色が混ざるのを防ぐ。
- **シェーダで軸別の half-texel inset clamp** (第 23 ラウンド反映):
  ```wgsl
  let u_crop = (crop.z < 0.999) || (abs(crop.x) > 0.001);
  let v_crop = (crop.w < 0.999) || (abs(crop.y) > 0.001);
  let dims = vec2<f32>(textureDimensions(pano_tex));
  let half_texel = 0.5 / dims;
  // u_crop が真のとき U を [0.5/W, 1 - 0.5/W] に clamp、偽なら Repeat に任せる
  // v_crop が真のとき V を [0.5/H, 1 - 0.5/H] に clamp、偽なら ClampToEdge に任せる
  ```
  - **DSLR 三脚 nodal panhead (= 水平フル + 垂直 partial)**: `u_crop=false, v_crop=true`
    → U は Repeat の seam wrap が維持され、V のみ shader clamp で端色を引き伸ばす ✓
  - **水平 crop only (稀)**: `u_crop=true, v_crop=false` → U のみ clamp で
    反対端 wrap を防ぐ、V は ClampToEdge sampler に任せる
  - **両方 crop**: 両方 clamp
  - **フル equirect (IDENTITY)**: 両方とも偽、`texture_uv_raw` 素通しで U Repeat
    + V ClampToEdge の「平時」挙動を維持
- **mipmap LODの経度シーム補正**: `atan2`由来のU座標はシームで1→0へ不連続になるため、
  暗黙微分のままでは約1.0の差として過度に粗いmipを選ぶ。`dpdx` / `dpdy`のU成分を周期1で
  `[-0.5, 0.5]`へwrapし、crop scaleを反映した明示微分を`textureSampleGrad`へ渡す。
- **Linear filter 対応 (half-texel inset)**: level 0では最外側texelの中心に対応する
  `[0.5/W, 1 - 0.5/W]` へclampする。mipmap導入後はlevel 0基準のinsetだけでは低LODの
  最外texel中心にならないため、水平crop用ClampToEdge samplerが各LODの端処理を担当する。
- **結果**: 欠落視野は端 texel の色 (空 / 地面っぽい色になりやすい) が均一に
  引き伸ばされ、反対端の画像が混ざる現象が出ない。垂直 crop only の典型ケースで
  U の seam wrap が無駄に無効化されることもない。

Phase 3 で「黒で埋める」「透過」オプションを追加する余地あり。

**実装影響**: ハッピーパス (フル equirect) も crop パスも、追加 ALU は **全フラグメント
で同量** (= ~8 演算: textureDimensions 1 + divide 2 + subtract 2 + clamp 4) 走る。
WGSL の `select(raw, clamped, cond)` は分岐ではなく **両辺を評価して値を選ぶ命令** で、
`clamped` 側の計算 (clamp / divide / textureDimensions) は cond の真偽に関わらず実行
される (Codex P3 第 24 ラウンドで表現訂正、第 23 で「実質コストゼロ」と書いたのは
不正確だった)。とはいえ追加 ALU は per-fragment で固定 8 命令、現代 GPU の compute
スループットから見ると実用上は無視できるオーバーヘッド (4K viewport の equirect
フラグメントシェーダ全体で見ると 1% 未満のコスト増)。

### Phase 2a: settle-refinement (実装済み、当初見積もり 1〜2 週)

§3.6 / §4.6 の高解像度品質補完。code checklist は実装済み、末尾の performance / 実素材
checklist は実施状況未確認として残す。

**Phase 2a で実装する範囲**:

- **解像度ゲート (200 MP) + ユーザー確認バナー** (§3.6.2 / §3.6.4)
- **SettleReady (≤ 200 MP)**: 自動で tee デコード → settle 有効化
- **NeedsUserConfirmation (> 200 MP)**: バナー表示 → 「高品質」選択で SettleApproved
  に遷移して追加 worker でフル RGBA ロード → settle 有効化
- **BaseOnly (> 200 MP かつユーザー却下)**: 8K base のみ、settle なし
- **pano_session_approved_max_pixels** (バナーのチェックボックスで記録、再起動でリセット、前回承認 × 1.25 超で再確認)

実装から外れたもの (Phase 3 以降):

- RAM ベース tier 判定 (`sysinfo` クレート不要、4 GB RAM 環境テスト不要)
- `PanoramaMemoryTier::Compressed` (旧設計の turbojpeg scaled decode)
- turbojpeg-sys 部分デコード (iMCU 境界 / u=0/1 跨ぎ / 専用 sampler) — §3.6.6 の通り
  巨大 JPEG 最適化として将来検討

- [x] `App` フィールド追加: `pano_high_res_source: HashMap<String, HighResSource>` /
      `pano_refinement: Option<PanoramaRefinement>` /
      `pano_quality_state: HashMap<String, PanoramaQualityState>` /
      `pano_session_approved_max_pixels: u64` (キーは `metadata_cache_key`、§4.6.1)
      + 追加: `pano_high_res_rx/tx` channel、`pano_high_res_pending`、`pano_banner_remember_session`
- [x] **`FsLoadResult::StaticPanorama` variant 追加** (`src/fs_animation.rs`、§4.6.0)
- [x] **ヘッダ読みで原寸取得** → `source_pixels <= 200 MP` で SettleReady、それ超え
      かつ `needs_user_confirmation(source_pixels, approved_max_pixels) == true` で
      NeedsUserConfirmation を初期状態に
      (§3.6.2 / §3.6.4)
      - 実装: `start_fs_load` の冒頭で `pano_intent` を作って worker に渡し、worker は
        `probe_dims` の結果 + intent からその場で tee 判定する。State 設定は
        `poll_prefetch` 側 (StaticPanorama → SettleReady/SettleApproved、Static →
        `maybe_update_pano_quality_state_from_static` で NeedsUserConfirmation)。
- [x] `start_fs_load` の 360 分岐:
  - [x] **SettleReady / SettleApproved**: image crate でフルデコード 1 回 → 同じ
        DynamicImage から `into_rgba8()` → Arc<Vec<u8>> (HighResSource::Decoded) +
        fast_resize で 8K リサイズした ColorImage を作って `FsLoadResult::StaticPanorama` で返却
  - [x] **NeedsUserConfirmation / BaseOnly**: 通常 `FsLoadResult::Static` のみ返却。
        `pano_high_res_source` にエントリを追加しない
- [x] **NeedsUserConfirmation バナー UI** (`draw_pano_confirmation_banner`):
  - [x] フルスクリーン上部 (上ホバーバー下) に半透明バナー
  - [x] 解像度 / MP / 想定 RAM 消費 (W*H*4*2 / 1e9) を数値表示
  - [x] 「フル解像度(高画質)」/「最大 8K(軽量)」ボタン
  - [x] チェックボックス「今後も高画質モードで開く」 → `pano_banner_remember_session`
  - [x] 「フル解像度(高画質)」選択時: state → SettleApproved、
        `start_pano_high_res_load(fs_idx, cache_key)` で worker 起動 + (チェック ON なら)
        `pano_session_approved_max_pixels = source_pixels`
- [x] **`PanoramaSettlePolicy` enum + `compute_settle_policy` 実装** (§3.6.2.1):
  - [x] `EnabledFromRaw` / `EnabledWithColorAdjustments { params }` /
        `Disabled { reason }` の 3 形
  - [x] 対象ページの実効機能と source kind から settle の再現可否を判定し、AI /
        post_filter / auto_mode / smart sharpen と色調補正 cache 待ちの理由を型で保持
  - [x] Disabled 理由を `PanoramaSettleDisabledReason` で保持し、表示側の手書き判定表を削除
- [x] **`resolve_pano_source` を `settle_policy` 込みに拡張** (§4.3):
  - [x] `PanoSourceResolution` に `settle_policy: PanoramaSettlePolicy` フィールドを追加
  - [x] `compute_settle_policy(fs_idx, source_kind)` を呼び込み済み
  - [x] 画素編集の有無にかかわらず final composite、adjustment、実効 AI、raw の順に選択
  - [x] final composite の key hash も cache key へ畳み、完成差し替えを再アップロード
- [x] `settle_enabled(state, policy)` で 2 軸 AND 判定 (§3.6.2.1)
- [x] CPU 並列レンダ `render_settle_overlay` + **独自の bilinear sampler**
      (`sample_bilinear_equirect`、§3.6.3)。`policy = EnabledWithColorAdjustments` で
      `crate::adjustment::apply_adjustments_fast(&ci, params)` を再適用。
      `PanoUvTransform` を入力に取って軸別 half-texel inset clamp を CPU 側でも適用。
- [x] 静止検出 (500 ms デバウンス、`PanoramaRefinement.settle_since`)、姿勢変化で reset
- [x] バックグラウンド render thread + cancel-token + **mpsc channel** で結果返却
      (`spawn_settle_render` / `RenderingHandle` / `SettleRenderResult`、§4.6.2)
- [x] 結果受信時に `source_key` + `pose` + **`cache_key`** で stale チェック
      (`update_pano_refinement` 内の 3 軸 guard)
- [x] 描画時にも `overlay_cache_key == resolution.cache_key` を確認
      (`PanoramaRefinement::overlay_ok_for(pose, cache_key)`)、不一致なら overlay を skip
      (= 8K base 単独表示)。settle 再起動で新 cache_key の overlay 生成
- [x] オーバーレイ wgpu テクスチャアップロード (`upload_settle_overlay` + `SettleOverlayGpu`) +
      フェードイン 150 ms ブレンド (`SettleOverlayCallback.alpha`)
- [x] 退出時のキャッシュ drop (`close_fullscreen` / `open_fullscreen` 時に
      `pano_refinement = None` + `pano_high_res_source.clear()` + `pano_high_res_pending` 全 cancel)
- [ ] **性能実測**: 4K viewport で 100 ms 以下 (200 MP 以下) / 400 ms 以下 (537 MP)
      に収まるかをマイルストーン直後に計測 (Phase 2a 実装後の TODO)

#### Phase 2a 実装後のフィードバック反映 (2026-05)

ユーザー実機テスト + Codex 第 2 ラウンドレビューで以下を改修:

- **status indicator (`draw_pano_status_indicator`)**: 画面下部中央に「● 高画質 ON
  (源解像度)」「○ 高画質 待機中」「○ 高画質 描画中…」「○ 8K 表示」等を毎フレ表示。
  ユーザーが高画質が効いているか目視で確認できるように。2026-08 以降、OFF 理由は
  `PanoramaSettlePolicy::disabled_reason()` から表示し、別の判定表は持たない
- **AI 効力ベース判定 (`ai_will_apply_to(fs_idx)`)**: 設定 ON でもサイズ閾値超で
  AI がスキップされる画像 (例: 11968×5984 の 360 写真) は AI 由来扱いにせず、settle を
  有効化。`compute_settle_policy` と `resolve_pano_source` の両方で `ai_feature_active`
  から `ai_will_apply_to(fs_idx)` に切替
- **× / V キー / Esc の役割整理 (§5.2 / §5.3 / §6.3 参照)**: 上部バー × が 360 OFF、
  Esc がフルスクリーン全体終了。360 アイコンは 360 OFF 時のみ表示
- **ホイール = FOV ズーム (修飾不問)**: 拡大縮小のつもりで画像送りを誤発火する事故を防止
- **NeedsUserConfirmation バナーの文言改善**: 「高品質で表示」→「フル解像度(高画質)」/
  「8K でよい」→「最大 8K(軽量)」、チェックボックス「今後も高画質モードで開く」、
  ホバーツールチップ追加 (2026-05 ユーザーフィードバックで「高画質化なし」は元画像を
  改変しているように誤読されるとの指摘を受け「軽量」表記に統一)
- **status indicator 内の切替ボタン** (§3.6.4 末尾): `BaseOnly` のとき
  `[高画質に切替]` (= `SettleApproved` 化 + `start_pano_high_res_load` 直接 kick、
  `pano_session_approved_max_pixels` は bump しない)、`SettleReady`/`SettleApproved`
  のとき `[8K 軽量に切替]` (= `BaseOnly` 化 + `pano_high_res_pending` drain &
  cancel + `pano_high_res_source.remove` + `clear_pano_refinement`、フル RGBA メモリ
  1-2 GB を即解放) を 1 つだけ pill バッジ右側に表示。ホバーで RAM 想定量
  (`W × H × 4 × 2 / 1e9` GB) を表示。`[高画質に切替]` は `is_plain_image &&
  !pano_high_res_failed && policy_enabled` のときだけ表示 (= ZIP/PDF/Video、decode
  失敗履歴、または選択 source を settle で再現できず `settle_policy` Disabled の画像では非表示。
  押下しても `start_pano_high_res_load` が即 return して "高画質 ロード中…" で
  永久 stall するのを防ぐ、Codex P1/P2 第 4 ラウンド指摘)。これにより旧案の
  「上部 360 ボタン横に ⚙ 高品質化アイコン → `NeedsUserConfirmation` に戻す」
  導線は廃止 (= ワンクリックで直接遷移、バナー再表示は不要)
- **HighResSource の保持ポリシー**: `open_fullscreen` の clear() を **新 idx の
  source_key だけ残す** retain に変更。prefetch tee 済み画像で settle が永久ロード
  中になる問題を解消 (Codex P1 第 2)
- **HighResSource 自動再要求**: `update_pano_refinement` で `can_settle &&
  !high_res_loaded` を検出したら `start_pano_high_res_load(fs_idx, cache_key)` を
  自動 kick (cache hit / 戻る経路でも settle 復活、Codex P1 第 2)
- **viewport-aware settle render**: 出力サイズを viewport の aspect から計算
  (`compute_pano_settle_output_size`、長辺 1920 で cap)。`RenderingHandle` /
  `SettleRenderResult` / `PanoramaRefinement` に `viewport_size` を追加し、
  リサイズで stale 化 (Codex P1 第 2)
- **SettleOverlay target_format guard**: `UploadedSettleOverlay` に `target_format`
  を持たせ、`SettleOverlayGpu` 再構築時に既存 overlay を drop (Codex P2 第 2)
- **XMP 遅延到着時の state 再評価**: `poll_metadata_load` で XMP が現フルスクリーン
  source_key に到着したら `maybe_update_pano_quality_state_from_static(fs_idx)` を
  呼んで、partial-FOV equirect (aspect 非 2:1 + 後追い XMP) でも SettleReady を立てる
  (Codex P2 第 2)
- **`maybe_update_pano_quality_state_from_static` の挙動拡張**: ≤ 200 MP の 360 候補
  にも `SettleReady` を立てるよう変更 (元: > 200 MP の NeedsUserConfirmation のみ)。
  これで late-XMP partial-FOV 画像の settle 経路が動く

#### Codex 第 3 ラウンドレビュー反映 (2026-05)

- **P2: settle overlay の `target_format` guard を描画経路にも追加**:
  `SettleOverlayCallback::prepare` で `UploadedSettleOverlay.target_format` と
  `self.target_format` の不一致を検出したら `SettleOverlayRef` を CallbackResources
  から remove し、その後 paint() で `set_bind_group(古い layout 焼付け)` が走らない
  よう保証する。元の `upload_settle_overlay` 内 drop と二重保険になり、target_format
  変化と settle 完了の race を確実に塞ぐ
- **P2: ZIP / PDF など `GridItem::Image` 以外の 360 候補は `BaseOnly` 固定**:
  `maybe_update_pano_quality_state_from_static` で `GridItem::Image` 以外なら即
  `BaseOnly` を入れる。元実装では SettleReady を立てて `update_pano_refinement` の
  自動 kick → `start_pano_high_res_load` が ZIP/PDF を即 return → status が永久に
  「高画質 ロード中…」のまま残る問題を解消。ZIP/PDF 内の equirect は Phase 3 拡張で
  検討
- **P3: viewport 変化で進行中 render を確実に cancel**: `try_paint_panorama` の
  `last_viewport_size` 直接代入を **`note_state(pose, cache_key, Some(viewport_size))`**
  経由に変更。これで viewport サイズ差を検出し、進行中の `RenderingHandle.cancel` を
  即立てる + `settle_since` を reset する。リサイズ直前の古い render の完了待ちで
  settle 復帰が遅れる問題を解消

#### Codex 第 5 ラウンドレビュー反映 (15 件指摘の一括対応、2026-05)

Phase 2a 完成後の包括的レビューで 15 件の指摘 (P0-P3) を網羅対応した。以下は
ハイライト:

- **P0 #1**: `start_pano_high_res_load` worker decode 失敗時の `pano_high_res_pending`
  リーク。`PanoHighResReady.high_res` を `Option<HighResSource>` に変更し、`SendGuard`
  Drop で **早期 return / panic 時も必ず `None` を送る**。`pano_high_res_failed: HashSet`
  で失敗履歴管理。これで「高画質 ロード中…」永久表示を解消
- **P0 #2**: `pano_refinement = None` 時に settle worker への cancel 信号が抜けていた。
  `clear_pano_refinement()` ヘルパで cancel.store(true) → drop の順を保証。4 サイト
  で置換。さらに `wgpu CallbackResources` から `SettleOverlayRef` も remove
- **P1 #3**: `maybe_update_pano_quality_state_from_static` が `pano_session_approved_max_pixels`
  を無視していた → 「セッション中ずっと高画質モード」が tee 不採用パスで効かない。
  `panorama::needs_user_confirmation()` 経由に統一
- **P1 #4**: `pano_session_approved_max_pixels = source_pixels` 直代入で、後で小さい
  画像を承認すると stored が下がる。`.max()` で monotonic に
- **P1 #5**: `poll_pano_high_res` respawn race + `#11` spawn-before-insert TOCTOU を
  **per-spawn `request_id` 識別子**で根治。`u64` カウンタを毎 spawn 時に発行し、
  pending + message 両方に焼き付け、poll は `request_id` 一致時のみ処理。ABBA-pattern
  (close → reopen で同じ source_key + 同じ cache_key の連続 spawn) の stale message
  race を完全に排除
- **P1 #6**: `viewport stale` → spawn 判定を `update_pano_refinement` (App::update 中) から
  `try_spawn_settle_render` (try_paint_panorama 中、`note_state` 後) に切り出して、
  当フレ最新の viewport で起動判定する設計に
- **P1 #9 + #10**: ≤8K source で tee 不要 + state cleanup。tee は `source > 8K` 限定、
  `pano_quality_state` は `close_fullscreen` で SettleReady / failure-induced BaseOnly を
  drop して unbounded growth + sticky 状態を回避
- **P1 #13**: `open_fullscreen` None-branch が **全 HighResSource を clear** していた
  → prefetch 結果を消し飛ばす。None フォールバックは no-op に
- **P2 #1**: SendGuard が cancel-aware に。cancelled worker は None send をスキップ
  (= 新 worker の pending を誤って消費しない)
- **P2 #7**: `pixels.size` fallback (CLAMPED 8K dim) で >200 MP 画像が NeedsUserConfirmation
  をバイパスする可能性があった。`source_dims` だけ信頼するように
- **P2 #8 + #12**: NaN / Inf 防御。`sample_bilinear_equirect` は `is_finite()` ガード +
  `saturating_add` で panic 回避、結果は透明黒。`PanoramaState::sanitize()` を drag /
  wheel 後に呼び pose が NaN に化けないことを保証。`from_gpano` も `u_scale < 0.001`
  を identity に倒す
- **P2 #15**: `ai_will_apply_to` が `fs_cache` のみを参照していた → cache evict 時に
  AI 有効と誤判定。`adjustment_cache` / `ai_upscale_cache` の fallback を追加
- **P2 R8 #1 (LRU 1 強化)**: `poll_prefetch` の StaticPanorama を non-fullscreen の場合
  即 Static にダウングレード (high_res Arc を即 drop)。`start_fs_load` 側でも
  `is_current_fs == false` なら tee path に入らないよう pano_intent を None に
  (= 同時並行 prefetch decode の memory peak を解消)
- **P2 R8 #2 (request_repaint gating)**: `pano_refinement` が Some の間ずっと
  request_repaint していた → idle 状態でも CPU/GPU を回し続ける。settle render
  in-flight / overlay fade in / settle_since 経過待ち の 3 条件に絞った。さらに
  `upload_settle_overlay` で `settle_since = None` に clear して overlay 完成後の
  永久 repaint も止めた

新規ユニットテスト:
- `panorama_state_sanitize_replaces_nan` / `panorama_state_new_handles_nan_inputs`:
  NaN / Inf 入力の正規化
- `sample_bilinear_equirect_handles_nan_inputs`: NaN / Inf 入力 → 透明黒
- `uv_transform_returns_none_when_scale_too_small`: u_scale 異常に小さい値を identity に
- 5 件の `compute_pano_settle_output_size` テスト

#### Codex 第 4 ラウンドレビュー反映 (2026-05)

- **P2 続報: `overlay_ok_for` に `target_format` 軸を追加**:
  第 3 ラウンドで `SettleOverlayCallback::prepare` 内に format mismatch guard を入れたが、
  App 側 `overlay_ok_for(pose, cache_key, viewport_size)` は format を見ていないため、
  format 変化後も「overlay 有効」と判断され続けて `update_pano_refinement` の
  再 spawn が走らない (= callback が毎フレ skip し、姿勢を変えるまで永久に 8K base
  単独表示)。
  - `PanoramaRefinement.overlay_target_format: Option<wgpu::TextureFormat>` を追加
  - `overlay_ok_for(pose, cache_key, viewport_size, target_format)` に拡張
  - `upload_settle_overlay` で `target_format` snapshot を焼き込み
  - `update_pano_refinement` で `wgpu_render_state.target_format` を取得して
    `overlay_ok_for` に渡す
  - `try_paint_panorama` も同様に渡す
  - `static_gpu_rebuilt` 経路 (= format 切替検出) で `overlay_target_format` も
    `None` に reset
  - これで format 変化 → 次フレで `overlay_ok_for` が false → `ready_to_render` が
    500 ms 経過判定で true → 新 format の overlay を CPU + wgpu で再生成する
  - panorama.rs に `wgpu` を import (元は wgpu 非依存方針だったが、TextureFormat
    比較のため最小限の依存を許容)
- [ ] テスト:
  - [ ] **2K ChatGPT 出力**: SettleReady 自動、settle 即実行
  - [ ] **12K Insta360 X3 (72 MP)**: SettleReady 自動、settle 動作
  - [ ] **26K 画像 (338 MP)**: NeedsUserConfirmation バナー表示、
        「フル解像度(高画質)」選択でフル RGBA ロード開始、settle 動作
  - [ ] **26K 画像で「最大 8K(軽量)」選択**: BaseOnly に遷移、settle なし、
        `pano_high_res_source` に何も入らないこと
  - [ ] **status indicator 内の切替ボタン**:
    - [ ] BaseOnly 状態 + plain image + 未 failed + policy_enabled → `[高画質に切替]`
          表示、押下で SettleApproved + worker spawn、`pano_session_approved_max_pixels`
          は **bump されない** (= 次の > 200 MP 新画像はバナー再表示)
    - [ ] BaseOnly 状態 + ZIP/PDF/Video → `[高画質に切替]` 非表示 (押下で stall 防止)
    - [ ] BaseOnly 状態 + `pano_high_res_failed` 該当 → `[高画質に切替]` 非表示
    - [ ] BaseOnly 状態 + 選択 source が settle_policy=Disabled →
          `[高画質に切替]` 非表示。再現可能な source へ変わった次フレームで表示復活
    - [ ] SettleReady/SettleApproved 状態 → `[8K 軽量に切替]` 表示、押下で
          BaseOnly + `pano_high_res_pending` cancel + `pano_high_res_source.remove` +
          `clear_pano_refinement` (RAM 解放)
    - [ ] NeedsUserConfirmation 状態 → ボタン非表示 (バナー側で選択)
  - [ ] **approved_max_pixels の動作**: 220 MP 承認後、240 MP (× 1.09) はバナー
        出ず、340 MP (× 1.55) はバナー再表示
  - [ ] **settle_policy 切替の動作** (Codex P1 第 10 ラウンド):
    - [ ] raw source 選択 → settle 動作 (EnabledFromRaw)
    - [ ] adjustment source + 単純色調のみ → settle 動作 + `apply_adjustments_fast` 適用、
          本体と視覚的に同等 (EnabledWithColorAdjustments、順序差で gamma 等は微小差あり)
    - [x] post_filter / auto_mode / smart_sharpen を含む final source → 加工を反映し、settle は Disabled
    - [x] AI source → AI を反映し、settle は Disabled
    - [x] 消しゴム mask がある → final composite を無視し、AI / adjustment / raw へフォールバック
  - [ ] **stale 再要求の動作** (Codex P2 第 11 ラウンド):
    - [ ] 「高品質」押下後にデコード中 (数秒間) に補正を切り替え → 到着結果は捨てら
          れて新 cache_key で再要求が走り、最終的に高品質が出る
    - [ ] 設定変更で cache_key が変化 → stale 検出 → 必要なら新 load 発火
  - [ ] **静止中の補正/AI 切替で overlay が即更新** (Codex P1 第 12 反映):
    - [ ] 静止して settle overlay 表示中 → 補正スライダー操作 → overlay 即非表示 →
          8K base (補正済み) のみ表示 → settle 再起動で新 overlay 生成
    - [ ] 静止して settle overlay 表示中 → AI トグル ON → overlay 即非表示 →
          AI または final base を再 upload し、settle は Disabled
  - [ ] **transient 時の settle 整合** (Codex P1 第 13 反映):
    - [ ] 補正パラメータが非 identity で adjustment_cache 未生成の瞬間 (transient) →
          settle policy が `Disabled` になり、ズレた色の overlay が出ないこと
    - [ ] adjustment_cache 完成後 → cache_key 変化で再評価され、
          `EnabledWithColorAdjustments` に遷移 → 正しい色の overlay 生成
  - [x] **final / AI cache の選択**:
    - [x] 画素編集あり + AI 実効 + final composite 完了済み → final を選ぶ
    - [x] final 未完成 + AI 実効 + adjustment cache あり → AI 実効時 adjustment として選ぶ
    - [x] AI 非実効で AI cache の残骸だけがある → AI を無視して adjustment / raw を選ぶ
  - [ ] **high-res 完了時の state guard** (Codex P1 第 14 反映):
    - [ ] ロード中に「8K でよい」(BaseOnly) → 結果到着で state は上書きされず、
          HighResSource にも格納されない (= 完全破棄)
    - [ ] ロード中に色調補正 cache 待ちへ遷移 → HighResSource は格納し、
          cache 完成後に再利用
    - [ ] ロード中に補正切替 (cache_key 変化) → ready は捨てられ、必要なら新 cache_key
          で再要求 (ただし state=SettleApproved のみ、Codex P2 第 15 反映)
  - [ ] **HighResSource の自動復帰**:
    - [ ] settle 無効の source 選択中も HighResSource を保持
    - [ ] 再現可能な source へ変わった後、追加デコードなしで settle が自動復活 (既存 Arc 再利用)
  - [x] **adjustment cache の由来**:
    - [x] writer は `apply_sync_adjustment` だけで、2 呼び出し元はいずれも raw pixels を渡す
    - [x] AI 実効時に adjustment fallback を選んだ場合は AI-adjust 種別で settle を無効化
  - [ ] **cache_key wrap の動作確認** (Codex P2 第 16 反映、unit test):
    - [ ] mock generation で 65,537 回 increment して cache_key の wrap 動作を
          確認 (実害は低いが long session の保険)
  - [ ] **フル RGBA drop**: 別画像へのナビ後にエントリ remove されること

### Phase 3 (settle 拡張): 巨大 JPEG 最適化 (未実装の将来候補)

Codex P1 で指摘された turbojpeg-sys 経路を実装するフェーズ。

- [ ] `turbojpeg-sys` を Cargo.toml に追加 (既存の `turbojpeg` crate と併存可能)
- [ ] `tj3SetCroppingRegion` + `tjDecompress2` を unsafe wrapper 化
- [ ] iMCU 境界スナップ (subsampling から MCU 幅を算出)
- [ ] 360 視野の u=0/1 跨ぎ検出 + 2 領域分割の `compute_crop_regions`
- [ ] Compressed tier 用の partial-source sampler (region offset 込み)
- [ ] 等価性テスト: Full tier と Compressed tier で同じ pose の settle 出力を比較
      (PSNR ≥ 40 dB 等)

### Phase 2b: 品質と利便性 (一部実装済み + 将来候補)

- [x] base texture の complete mip chain 生成 + trilinear filtering
- [ ] 右下ミニコンパス
- [ ] 慣性ドラッグ (`fs_drag_inertia` パターン)
- [ ] 矢印キー操作
- [ ] yaw/pitch/fov の表示オーバーレイ (1 秒フェード)
- [ ] スライドショー連動 (連続 equirect で yaw/pitch を引き継ぎ)

### Phase 3: 拡張 (将来 / 実需が出てから)

- [ ] **>16K ソース対応 (2 タイル水平分割)** — 32K stitched 等のレア素材向け。
      §3.5 末尾のタイル方式 + 1px オーバーラップでシーム処理 + mipmap 対応
- [ ] little planet (stereographic 下向き) モード
- [ ] cubemap 変換オプション (フィルタ品質、シーム完全排除)
- [ ] 部分パノラマの欠落領域を「黒」「透過」等から選ぶ表示オプション
- [ ] アニメーション GIF / APNG の equirect 再生
- [ ] 360 動画 (FFmpeg 経路、別議論)
- [ ] サムネのインポスタ表示 (バッジで「360°」と表示)
- [ ] **全体上限の引き上げ (任意)**: 環境設定に「高解像度モード」(MAX_TEXTURE_DIM
      = 16384) を ON/OFF できるトグル。AI ガード再設計 + UI ヒッチ計測 + フォルダ
      切替時のメモリプレッシャー確認が前提

---

## 9. 実装前レビュー記録と残存リスク (履歴)

以下は実装前レビューの指摘を追跡した履歴である。「解決済み」は現行コードへ反映済み、
未チェックまたは将来 Phase を明記した項目だけが残存候補であり、現行仕様は §2〜§8 を
優先する。

この表には解決済みの設計レビュー記録と、手動計測・将来実装が必要な現行リスクが混在する。
「解決済み」は履歴、「未実測」「Phase 3」は現在も残る事項として読む。

| 項目 | 内容 | 軽減策 |
| --- | --- | --- |
| **キャッシュキー無効化** (Codex P1) | size 据え置きで補正 / AI 結果だけ変わるケースで `compare_wgpu` 流儀の size-only 判定だと古いテクスチャが残る | §4.1 のキャッシュキー設計に従い `(idx_hash, source_kind, adjust_gen, ai_gen)` を u64 にパック + `Arc::ptr_eq` の保険チェックで完全カバー |
| **メタデータ idx キーの脆さ** (Codex P2) | `HashMap<usize, _>` は並び替え / 仮想フォルダで idx ずれ | §2.4 のとおり `metadata_cache_key(idx)` (String) に統一 |
| **ZIP 内 equirect** (Codex P2) | `read_panorama_info(path)` だけでは対応不可 | `read_panorama_info_from_bytes` を追加し、metadata worker の既存 ZIP bytes 経路に乗せる |
| **ColorImage 内部表現依存** (Codex P2) | 「`Color32` の内部表現が連続 RGBA8」は egui 実装詳細であり将来変動の可能性 | `src/capture.rs:393` の `color_image_to_rgba` を明示的に経由 (§4.3)。compare_wgpu の本流に揃える |
| **入力衝突** (解決済み) | Wheel / 矢印 / Esc の既存操作と競合 | §5.2 のとおり、360 中は wheel を修飾キー不問で FOV に統一し、矢印 / Esc は既存動作を維持 |
| **<kbd>P</kbd> キー衝突** (解決済み) | 静止画フルスクリーン P は `set_folder_thumb_pin` で既に consume | `KeyAction::FsPanorama` の既定 V とホバーバーボタンを実装 |
| **wgpu mipmap 生成** (解決済み) | `queue.write_texture` だけでは自動生成されない | `egui_wgpu::MipmapGenerator` で complete mip chain を生成し trilinear sampling |
| **ColorImage → Arc<Vec<u8>>** | `Color32` の内部表現に依存しない明示変換が必要 | `src/capture.rs:393` の `color_image_to_rgba` を経由 (§4.3 で確定、`compare_wgpu` 本流と同じ) |
| **アスペクト誤検出** | 2:1 スティッチ写真や横長壁紙を equirect 扱いしないか | アスペクト単独では自動起動せず、ヒント止まり (§2.3) |
| **PDF / ZIP 内の equirect 画像** | container 内では metadata / high-res source の取得経路が通常画像と異なる | ZIP は GPano XMP と base 表示、PDF は 2:1 fallback の `BaseOnly`。どちらも high-res settle は行わない |
| **rotation_db との競合** | 既に 90° 回転されている equirect は壊れる (アスペクト 2:1 でなくなる) | 360 モード中は `rotation_db` の角度を無視。アスペクト判定は `source_dims` の生値で行う |
| **タスクバーサムネ / DWM** | 動画フルスクリーン用の dwm_iconic_thumbnail は静止画 360 でも害なし | 触らない |
| **メモリ (~8K)** | 5760×2880 ≒ 66 MB RGBA。8K equirect (8192×4096) ≒ 134 MB | wgpu 8192 制限内で `clamp_dynamic_for_gpu` の既定範囲。問題なし |
| ~~**メモリ (16K)**~~ | ~~16384×8192 = 512 MB CPU + 512 MB VRAM~~ | **解決済み**: 案 A 確定で Phase 1 は 8K のみ。CPU/VRAM 各 134 MB に縮小 |
| **メモリ (大画像、Phase 2a 以降)** | SettleReady 上限 (16K = 134 MP) で 540 MB / SettleApproved の 26K で 1.35 GB / 32K で 2.15 GB の HighResSource を保持 | §3.6.4.1 のとおり 200 MP 超は NeedsUserConfirmation でユーザーに事前承認を取る。SettleApproved 後の追加デコードはバナーで RAM 想定を明示済み。BaseOnly 選択時は専用 cache 追加なし |
| ~~**VRAM LRU 上限**~~ (Codex P1 第 2 ラウンド) | ~~16K テクスチャ 8-16 枚保持で 4-8 GB VRAM が消費~~ | **解決済み**: 案 A 確定 + LRU 1 (= active のみ) で 8K × 1 = 134 MB に縮小 |
| **アップロード UI ヒッチ** (Codex P1 第 2 ラウンド) | 大型 `queue.write_texture` が `Callback::prepare` で同期実行されると UI フリーズ | `Callback::prepare` の中ではアップロードしない (§4.1.1) |
| ~~**アップロード UI ヒッチ — 単発 512 MB**~~ (Codex P1 第 3 ラウンド) | ~~`prepare` から外しても単発 512 MB write_texture は UI スレッドを 50-150 ms ブロックする~~ | **解決済み**: §12.1 案 A 確定で 16K 自体を Phase 1 から外したため。8K base (134 MB) でも 1 フレ落ちはあり得るので実測ベースで worker 化を判断 (§4.1.1) |
| **巨大 PNG/WebP のピーク** (Codex P1 第 3 ラウンド) | PNG/WebP は scaled decode 経路がないので、フル展開でピークが出る | §3.6.4.3 のとおり BaseOnly では新規プロキシを作らず既存 `fs_cache` を流用。NeedsUserConfirmation バナーで RAM 想定をユーザーに事前提示 |
| **callback と upload の決合** (Codex P2 第 3 ラウンド) | callback が `Arc<Vec<u8>>` を持つと毎フレーム mpsc に乗る悪パターン | §4.1 のとおり App 側 `UploadedPanoTexture` で wgpu リソースを保持し、callback は `source_key` + `cache_key` 参照のみ |
| ~~**Compressed tier の事前フルデコード**~~ (Codex P1 第 5) | ~~image crate を通した時点でピークメモリ発生~~ | **不要になった (第 7 ラウンドで RAM tier 廃止)**: Compressed tier そのものを削除。Phase 3 で巨大 JPEG 最適化として再検討する場合のみ復活 |
| **turbojpeg API** (Phase 3 拡張時の課題) | 安全な `turbojpeg::decompress_image` には partial decode なし | §3.6.6 のとおり Phase 3 オプション扱い、Phase 2a スコープ外 |
| **JoinHandle::try_join 不存在** (Codex P2) | `std::thread::JoinHandle` に `try_join` はない (`is_finished` のみ) | §4.6.2 の mpsc channel + source_key + pose snapshot で stale 検出 |
| **fast_resize 流用不可** (Codex P2) | `fast_resize` は矩形 → 矩形のリサイズ専用 | 独自の `sample_bilinear_equirect` を新規実装 (§3.6.3) |
| **settle 性能未実測** (Codex P2) | 30-80 ms 見積もりは未検証 | Phase 2a の最初のマイルストーンで実機計測。100 ms 超なら出力解像度を一段下げる等のフォールバック |
| ~~**アダプタリミット**~~ | ~~古い iGPU が 16384 をサポートしない~~ | **解決済み**: 案 A 確定で 8K のみ使用、wgpu デフォルトリミット内 |
| **タイル方式のシーム** | u=0.5 で 1px ghost / mipmap 不整合 | Phase 3 まで採用しない。実需 (>16K ソース) が出てから 1px オーバーラップ + ClampToEdge で対応 |
| **settle のキャンセル漏れ** | render thread が CPU を持ち続ける | Arc<AtomicBool> を `par_chunks_exact_mut` 内で行単位チェック (§3.6.3)。ドラッグ再開で即 drop |
| **settle のメモリリーク** | 1.35 GB が drop されない | フルスクリーン退出 / フォルダ切替 / 別画像ナビで明示的に `pano_high_res_source.remove(idx)`。テスト項目に追加 |
| **Full tier の二重デコード** (Codex P1 第 5 ラウンド) | 通常パスで 8K fs_cache を作った後にもう一度フルデコードすると 1.35 GB ピークが 2 回出る | §3.6.2 のとおり 1 回のフルデコードから DynamicImage を **tee** で fs_cache (clamp) と HighResSource::Decoded の両方を作る |
| **tier 判定の順序** (Codex P1 第 5 ラウンド) | 通常 `fs_cache` 生成後に判定すると、巨大画像のピーク回避にならない | §3.6.2 / §3.6.4 のとおり **`start_fs_load` の通常デコードより前にヘッダ判定**。経路分岐は SettleReady / NeedsUserConfirmation / SettleApproved / BaseOnly で決定 |
| **BaseOnly の効果範囲** (Codex P1 第 5 ラウンド) | 「Off tier はピーク回避」と曖昧に書くと過大評価される | §3.6.4.3 のとおり BaseOnly は **360 機能による追加ピークをゼロ**にするだけで、**既存 `start_fs_load` のピーク自体は消さない**ことを明文化 |
| **pano_uploaded の所有型** (Codex P2 第 5 ラウンド) | `Option<UploadedPanoTexture>` と `Arc::clone` 使用が不整合 | §4.1 のとおり `Option<Arc<UploadedPanoTexture>>` に統一。CallbackResources への挿入は `Arc::clone` でゼロコピー |
| **uploaded_ready 判定が source_key のみ** (Codex P1 第 6 ラウンド) | 同じ画像で補正 / AI 結果だけ変わった場合に古い 360 テクスチャを描画 | §4.2 のとおり **`source_key + cache_key` 両方一致**でチェック。cache_key には source_kind / adjust_gen / ai_gen が畳まれているので stale を完全検出 |
| **HighResSource を UI へ運ぶ経路** (Codex P1 第 6 ラウンド) | `FsLoadResult::Static` は ColorImage + 寸法しか運ばない | §4.6.0 のとおり **`FsLoadResult::StaticPanorama { ci, source_dims, high_res }`** 新 variant を追加。同じ DynamicImage 由来であることを型で担保 |
| **Full tier の追加ピーク見積もり** (Codex P1 第 6 ラウンド) | `DynamicImage → Arc<Vec<u8>>` は入力形式により RGBA8 変換で追加 1.35 GB 発生 | §3.6.4.2 のとおり入力 variant 別の追加ピークを明文化 (worst case ~2.7 GB)。バナーで worst case の値を明示してユーザー判断に委ねる |
| **RAM 判定のテスト困難** (ユーザー第 7 ラウンド指摘) | 4 GB RAM 環境を CI で再現できない、動的判定が不安定 | §12.3 のとおり RAM 検出を廃止、200 MP 固定ゲート + ユーザー確認バナーに変更。`sysinfo` クレート依存も削除 |
| **巨大画像のユーザー透明性** (ユーザー第 7 ラウンド設計改善) | 黙って画質を落とすと「自分が選んでいない感」がある | §3.6.2 のバナー UI で「想定 RAM 消費」を明示、「高品質 / 8K でよい」をユーザーが選択 |
| **callback の stale guard** (Codex P1 第 8 ラウンド) | callback 側が `source_key` のみチェックで、補正 / AI 切替時に古いテクスチャを描画する可能性 | §4.1 のとおり callback struct に `cache_key: u64` を追加し、paint() 内で `source_key + cache_key` の両方一致を二重チェック |
| **追加 high-res load の経路** (Codex P2 第 8 ラウンド) | NeedsUserConfirmation 後に `StaticPanorama` で 8K base を再生成すると無駄 | §4.6.0 のとおり追加 worker は **専用 channel `pano_high_res_pending`** 経由で `PanoHighResReady` を運ぶ。fs_cache / fs_upload_backlog は触らない (Codex P1 第 9 ラウンドで channel 分離に再修正) |
| **settle と補正 / AI の整合** (Codex P1 第 9-10 ラウンド) | 8K base は補正済み、settle source は元 RGBA で色が一致しない | §3.6.2.1 のとおり、AI / post_filter / auto_mode / smart sharpen は base に反映したまま Disabled。画素編集だけの差は仕様として許容し、実際に切替が起こるときは注意書きを表示 |
| **adjustment_cache の AI 由来という誤認** (2026-08 再確認) | 旧設計は adjustment cache に AI 結果も入ると仮定していた | 唯一の writer `apply_sync_adjustment` と 2 呼び出し元を確認し、raw pixels だけが入力と確定。AI-adjust 種別は由来ではなく AI 実効時の fallback 状態を示す |
| **resolve_pano_source の実型不一致** (Codex P1 第 10 ラウンド) | 旧擬似コードが `ai_upscale_cache.get(&fs_idx)` / `Arc<ColorImage>` 直返しで実型 (`HashMap<(usize, u8), FsCacheEntry>`) と不一致 | §4.3 のとおり実型に合わせて書き直し: `(fs_idx, bg)` キー、`FsCacheEntry::Static { pixels, .. }` パターンマッチ、優先順位は final → adjustment → AI → raw |
| **post_filter / auto_mode の再現困難** (Codex P1 第 10 ラウンド) | `apply_adjustments_fast` + `post_filter::apply` の 2 段階 + NEAREST サンプラ要件 + `auto_mode` の再計算ずれ | §3.6.2.1 のとおり post_filter / auto_mode が active なときは Disabled に倒し、settle なしで本体のみ表示 |
| **`PanoramaHighResReady` の channel 分離** (Codex P1 第 9 ラウンド) | `FsLoadResult` variant 追加では既存 `poll_prefetch` の fs_cache 変換構造と衝突 | §4.6.0 のとおり専用 channel `pano_high_res_rx` を新設、`poll_pano_high_res` で別経路処理 |
| **resolve_pano_source 関数の集約** (Codex P2 第 9 ラウンド) | cache_key 計算とピクセル解決が分かれているとズレで stale guard が崩壊 | §4.3 のとおり `resolve_pano_source(ctx, fs_idx) -> PanoSourceResolution` で source_key / cache_key / pixels / source_kind / settle_policy を一体決定 |
| **PanoHighResRequest の cache_key 欠落** (Codex P2 第 10 ラウンド) | `PanoHighResReady` には cache_key があるが request 側が source_key のみで stale 検出が弱い | §4.6.0 の `PanoHighResRequest` に `cache_key: u64` を追加し、`poll_pano_high_res` で `ready.cache_key == req.cache_key` の 3 段階 stale check |
| **`PanoramaHighResReady` 別 variant 表現の混乱** (Codex P3 第 10 ラウンド) | 「別 variant」と書いたが実体は専用 channel の別メッセージ型 | 該当箇所を「専用 channel 経由で `PanoHighResReady` 別メッセージ型を送る」に修正 |
| **`build_lut` / `apply_lut` API の不在** (Codex P1 第 11 ラウンド) | 設計の独自関数名は実コードに存在しない (`apply_adjustments_fast` が public、`build_u8_lut` は private) | §3.6.3 のとおり settle render の補正適用は **`crate::adjustment::apply_adjustments_fast(&ci, params)` を直接呼ぶ**経路に統一。overlay を一旦 `ColorImage` 化してから掛ける |
| **`params.auto_mode` の型誤り** (Codex P1 第 11 ラウンド) | `bool` で擬似コードを書いていたが実型は `Option<AutoMode>` | §3.6.2.1 で `params.auto_mode.is_some()` に修正 |
| **`EnabledWithColorAdjustments` (旧名 `EnabledWithLut`) の「完全一致」表現** (Codex P2 第 11 ラウンド) | gamma 等の非線形補正で「補間 → 補正」と「補正 → 補間」は厳密一致しない | §3.6.2.1 一致性列を「視覚的に同等 (順序差で gamma 等は微小差)」に修正。§3.6.3 で実装トレードオフを明記 |
| **AI 由来 adjustment_cache 判定の脆さ** (Codex P2 第 11 ラウンド) | AI 由来も入るという前提自体が誤りだった | 2026-08 の writer / call site 再確認で raw 由来だけと確定し、由来推定を削除 |
| **cache_key 不一致時の再要求動作** (Codex P2 第 11 ラウンド) | 古い結果を捨てるだけだとユーザーが「高品質」を押しても永久に反映されない | §4.6.0 `poll_pano_high_res` で stale 検出時に **現在の resolution で再キック** する経路を追加 |
| **settle overlay の cache_key 欠落** (Codex P1 第 12 ラウンド) | `PanoramaRefinement.overlay_pose` だけだと、静止中に補正/AI を切り替えても古い overlay が居座る | §4.6.1 で `last_cache_key` / `overlay_cache_key` を追加、`RenderingHandle.for_cache_key` / `SettleRenderResult.cache_key` で start-to-end 一貫した stale 判定。描画時にも cache_key 一致を確認 |
| **adjustment_generation / ai_upscale_generation** (解決済み、Codex P1 第 12 ラウンド) | 同じ source_kind・同じサイズで補正だけ変わったケースを cache_key で検出できない | 両 generation と bump 経路を実装し、`resolve_pano_source` の cache key に含めた |
| **effective_params が参照を返すため clone 必須** (Codex P2 第 12 ラウンド) | `EnabledWithColorAdjustments { params }` (当時は `EnabledWithLut`) で所有権要求、`&AdjustParams` のまま渡せない | §3.6.2.1 で `params: params.clone()` に修正、`src/app.rs:16244` の実型コメント追加 |
| **high-res 再デコードの重さ** (Codex P2 第 12 ラウンド) | 補正スライダー変更で 26K JPEG が毎回再デコードされる | §4.6.0 のコメントで明示。Phase 3 で `HighResSource` の source_key 単位再利用 pool 化を検討する余地を明文化 |
| **settle policy と source_kind の整合** (Codex P1 第 13 ラウンド) | params 有効だが adjustment_cache 未生成の transient で 8K base = raw、settle overlay = 補正済みになりズレる | §3.6.2.1 で `compute_settle_policy(fs_idx, source_kind)` シグネチャに変更。source_kind=0 + params 非 identity は **Disabled (transient 待ち)** に倒す。adjustment_cache 完成後に cache_key 変化で再評価 |
| **high-res 再要求の gate 不足** (Codex P1 第 13 ラウンド) | state が SettleApproved でも AI / post_filter ON のとき再デコードが走る | §4.6.0 の再要求ロジックに `settle_enabled(state, &policy)` チェックを追加。settle 不使用なら 26K 再デコードしない |
| **AI 有効判定が cache 存在依存** (Codex P2 第 13 ラウンド) | AI OFF 後に cache 残骸で誤判定の可能性 | §3.6.2.1 / §4.3 で対象ページの `effective_upscale_request(effective_params(idx))` を使う。UI 同期グローバルや cache 存在だけで判定しない |
| **旧名 `EnabledWithLut` の誤解誘発** (Codex P3 第 13 ラウンド) | 実装は `apply_adjustments_fast` 経由、LUT は内部実装の一形態 | **現名 `EnabledWithColorAdjustments { params }`** にリネーム済み。全参照箇所を修正 |
| **ai_upscale_cache 分岐の AI 機能 flag gate** (Codex P1 第 14 ラウンド) | `ai_upscale_cache.get` 分岐が `ai_feature_active` で gate されておらず、AI OFF 後の cache 残骸を 360 入力に拾う可能性 | §4.3 で `ai_cache_entry` を `if ai_feature_active { ... } else { None }` で挟む。adjustment_cache 分岐の `source_kind` 判定とも整合 |
| **high-res 完了時の state 上書き** (Codex P1 第 14 ラウンド) | ロード中にユーザーが「8K でよい」を押しても、完了時に SettleApproved に上書きされる | §4.6.0 `poll_pano_high_res` で **成功パスにも guard 追加**。state=BaseOnly なら結果破棄、settle_will_use_it=false なら state を上書きせず HighResSource のみ保持 (将来 OFF→ON で再利用可能) |
| **cache_key の stale 比較対象** (Codex P2 第 14 ラウンド) | `ready.cache_key != req.cache_key` は worker 内同一値で stale 検出にならない | §4.6.0 で **`ready.cache_key != current resolution.cache_key`** に修正。`PanoHighResRequest.cache_key` は重複リクエスト検出専用と用途明確化 |
| **source_kind=3 の意味食い違い** (Codex P2 第 14 ラウンド) | `3=pano_high_res` と `3=ai+adj` が混在していた | high-res は `pano_high_res_source` 別ストアで管理。3 は AI 由来 pixels ではなく、AI 実効時に adjustment fallback を選んだ状態だけを示す |
| **NeedsUserConfirmation 状態での再要求** (Codex P2 第 15 ラウンド) | `user_still_wants_high_res` が `SettleApproved \| NeedsUserConfirmation` の両方を受けると、未承認状態でも 26K 再デコードが走る | §4.6.0 で `SettleApproved` のみに絞る。`NeedsUserConfirmation` はバナーが残っているので、ユーザーが押せば改めて `start_pano_high_res_load` が走る |
| **HighResSource 既存時の自動復帰条件** (Codex P2 第 15 ラウンド) | AI / post_filter 解除時に既存 HighResSource を再利用する条件がライフサイクル未明記 | §4.6.3 step 5 に自動復帰チェーンを明示: AI/post_filter OFF → `settle_policy` 遷移 → 次フレ `settle_enabled` 変化 → 既存 Arc を参照して追加デコードなしで settle 起動 |
| **AI OFF 時 adjustment_cache clear 不変条件** (Codex P2 第 16 ラウンド) | AI 由来 adjustment があるという前提で clear 依存を置いていた | adjustment cache は raw 由来だけなので settle correctness はこの clear に依存しない。AI cache は実効 AI 判定で gate する |
| **cache_key の 16bit gen wrap リスク** (Codex P2 第 16 ラウンド) | 長時間セッション (補正アニメ等で約 18 分、通常スライダー操作で約 18 時間) で wrap し、stale guard が誤って一致判定する可能性 | §4.1.2 末尾で wrap 試算を明記。Phase 3 拡張案として `source_kind` を 4 bit に縮めて gen を 22 bit に再配分、または `cache_key` を struct 化を提示 |
| **旧名 `EnabledWithLut` 表記の混在** (Codex P3 第 16 ラウンド) | リスク表に旧名が残り、実装者が現名と取り違える可能性 | リスク表の該当行に「(旧名 `EnabledWithLut`)」または「現名 `EnabledWithColorAdjustments`」を明記 |
| **clear 粒度の表記が実コードと不整合** (Codex P3 第 17 ラウンド) | 設計で「該当 idx で clear」と書いていたが、実コード `clear_all_adjustment_and_ai_caches` は adjustment は idx、AI は全体の混合粒度 | §3.6.2.2 に粒度表 (adjustment=idx、AI=全体) を追記、Phase 1 チェックリストの generation bump も同じ粒度に整合 |
| **`ai_upscale_generation.clear()` の衝突リスク** (Codex P3 第 17 ラウンド) | clear するとリセット後の値が過去 cache_key と衝突する可能性 | §3.6.2.2 / Phase 1 で **`.clear()` ではなく全 entry `+= 1`** (`bump_all_ai_generations`) と明記 |
| **session 承認の単純 bool** (Codex P2 第 9 ラウンド) | bool だと 201 MP 承認後に 537 MP も無確認になる | §3.6.2 のとおり `pano_session_approved_max_pixels: u64` に変更、前回承認 × 1.25 超で再確認 |
| **BaseOnly からの復帰導線** (Codex P2 第 9 ラウンド + 2026-05 後続反映) | バナーで「最大 8K(軽量)」を選んだ後、高画質に戻す手段がない | §3.6.4 末尾のとおり画面下部 status indicator 内に `[高画質に切替]` ボタン常設、ワンクリックで `SettleApproved` に直接遷移 (旧案の「⚙ 高品質化アイコンで `NeedsUserConfirmation` に戻す」は廃止)。逆方向の `[8K 軽量に切替]` ボタンも同じ場所に対称配置 |
| **8K アップロードの「ヒッチなし」表現** (Codex P3 第 9 ラウンド) | 「8K なら UI ヒッチ無し」は言い切りすぎ | §12.1 / §4.1.1 で「16K より十分軽いが 1 フレ落ちはあり得る、実測で worker 化判断」に統一 |
| ~~**JPEG 部分デコードの非対応形式**~~ | ~~PNG / WebP の 26K で Compressed tier に降格できない~~ | **不要になった**: 第 7 ラウンドで RAM tier 廃止、PNG/WebP 巨大も NeedsUserConfirmation バナー経由でユーザー判断 |
| **HDR / EXR** | RICOH Theta などで HDR equirect が出る | スコープ外 (色管理自体が未実装) |

### 実装前に挙げた問い (履歴。現行仕様は §2〜§8 を優先)

1. ~~**ホットキー**: <kbd>P</kbd> でよいか?~~ → **Codex P1 で衝突確認済 (`set_folder_thumb_pin`)。
   Phase 1 はキー割当なし、トグルボタン主導で確定**
2. **自動起動の閾値**: GPano `UsePanoramaViewer=True` のみで自動起動?
   それとも `ProjectionType=equirectangular` だけでも自動起動?
3. **サムネのバッジ**: 360 画像と検出されたものに「360°」アイコンを出す? 出すならどの段階の検出 (XMP のみ / アスペクトも) で?
4. **設定 UI**: 環境設定に「2:1 画像を 360 として自動起動する」トグル?
   (デフォルト OFF、XMP のみで自動起動が安全)
5. **generation カウンタの新設**: `adjustment_generation` / `ai_upscale_generation` が
   既存にあるか、無ければ最小実装で追加するか (= 補正適用と AI 完了の場所で +1 する
   `HashMap<String, u32>`)。実装時に既存コードベースをサーベイ

---

## 10. 実装時に計画した関連ドキュメント更新 (履歴)

CLAUDE.md「コード修正時のドキュメント同時更新」に従い、実装時は以下を併せて更新:

- `docs/display-pipeline.md` §2 / §4 — 360 モード分岐の追加、テクスチャ優先順位への注記
- `docs/architecture-overview.md` §2 — `panorama_wgpu.rs` をモジュールマップに追加
- `docs/keymap-spec.md` — 360 トグル UI の記載 (ホバーバーボタン主導、キー割当は Phase 1 では追加なし)
- `docs/spec.md` — 機能一覧に「360 度パノラマビュー」を追加
- `htdocs/mimageviewer/manual/` — ユーザーマニュアルに 1 ページ追加
- `htdocs/mimageviewer/index.html` — 機能紹介に追加
- README.md 更新履歴 — リリース版で 1 エントリ

---

## 11. 検討した代替案 (採用せず)

### 11.1 egui Mesh + `LINEAR_REPEAT` サンプラで実装

**案の内容**: WGSL シェーダを書かず、egui の `Mesh` (UV 付き三角形群) を生成して
通常の egui テクスチャに `wrap_mode = LINEAR_REPEAT` でマッピング、CPU 側で各頂点に
sphere 内側からの UV を持たせて描画する方式。

**利点**:

- 既存フルスクリーン描画パイプラインに無改造で乗る
- WGSL / wgpu API を直接触らないので保守側面の学習コストが低い
- MVP の最短実装

**不採用の理由**:

- `compare_wgpu.rs` という WGSL の足場が既にあるため、WGSL 側の追加コストが事前
  想定より小さい
- Mesh 方式は **頂点間で線形補間される UV** に依存するため、極付近で歪みが目に
  見える (テッセレーション密度を上げると軽減できるが頂点数が爆発)
- FOV ズームイン時、各三角形が画面で大きくなり「ねじれ」が出る (パースペクティブ
  補正が不完全)
- WGSL なら per-pixel 逆射影で常に正確、テッセレーション不要

Codex レビューでは「最初から WGSL でいくならキャッシュキー・アップロード・入力衝突
を先に潰した設計に」との指摘があり、本ドキュメントは §4.1 / §4.3 / §5.2 でそれぞれ
対応済み。WGSL 路線を維持する。

### 11.2 すべて単一テクスチャでスケール (タイル化しない)

**案の内容**: 26K-50K のソースも全部 wgpu の 1 テクスチャに収めるアプローチ
(adapter の限界まで使う)。

**不採用の理由**:

- 16384 を超える `max_texture_dimension_2d` は D3D12 では 32768 まで可能だが、
  Vulkan / Metal / 古い iGPU で互換性がまだら
- 単一巨大テクスチャは VRAM の連続領域を要求するためフラグメンテーションに弱い
- §3.6 の settle-refinement の方が、ドラッグ中の VRAM 圧迫を回避できる

---

## 12. 実装前の設計判断 (決定済みの経緯)

### 12.1 16K base 採用の是非 (**案 A 確定**)

§4.1.1 で記載した通り、512 MB 単発アップロードは UI スレッドで 50-150 ms 級フリーズ
する。既存の `fs_upload_backlog` は egui の `ctx.load_texture` の同期コストを
ペーシングしているが、wgpu の raw `queue.write_texture` を 512 MB 一気に実行すると
PCIe 転送がそのままブロッキングする。

選択肢:

| 案 | 内容 | Pros | Cons |
| --- | --- | --- | --- |
| **A (推奨)** | **16K base を諦め、8K + settle に統合**。`fs_cache` の 8K を 360 ベースとして流用、品質は settle-refinement で確保 | (1) Phase 1 大幅簡素化 (wgpu limit upgrade 不要、`pano_source_pixels` 不要)。(2) **16K 単発 upload より十分軽い**: 8K = 134 MB の upload + color_image_to_rgba は実測で 1 フレーム落ちは起こりうるが許容範囲、必要なら worker 化で対応 (§4.1.1)。(3) >16K source の品質はむしろ向上 (4K viewport を 26K full から sample > 16K proxy から sample) | 8K-16K source (Insta360 X3 等) で **ドラッグ中の画質は 8K** に下がる。settle で補完 |
| B | 16K base + バンド分割アップロード (N=8 程度) | ドラッグ中も最終的に 16K の精細さ | バンド充填中の中間状態管理が必要。実装複雑度高 |
| C | 16K base + idle 限定アップロード | A と B の中間 | settle と概念重複、結局 8K で動く時間が長い |

**ユーザー判断**: 案 A を採用 (Codex 第 3 ラウンドレビュー後の議論で確定)。

**案 A 採用理由**:

1. **settle-refinement が既に高品質経路を提供**: stationary 時に full source から
   4K viewport を bilinear sample すれば、16K proxy 経由より直接的に高画質
2. **ChatGPT 等のメイン用途 (~2K) では 8K で過剰**: 16K base のメリットはほぼなし
3. **8K base 採用で Phase 1 が圧縮**: main.rs の `WgpuConfiguration` 触らない、
   `pano_source_pixels` キャッシュ層が不要、`clamp_panorama_for_gpu` ヘルパも不要
4. **wgpu limit upgrade の互換性懸念も消える**: 古い iGPU での fallback ロジックが
   不要に
5. **メモリ全体が下がる**: VRAM 512MB → 134MB、CPU 同サイズ削減

**案 A 採用時の Phase 1 変更点**:

- 削除: `pano_source_pixels` / `max_pano_dim` / `clamp_panorama_for_gpu` /
  `WgpuConfiguration` の limit 引き上げ
- 維持: 360 検出 / equirect WGSL シェーダ / 入力ハンドリング / UI トグル / GPano XMP
- 360 ベーステクスチャは `resolve_pano_source` が選んだ final / AI / adjustment / raw
  pixels を `color_image_to_rgba` 変換した上で毎回新規アップロード (LRU 1)
- Phase 2a 以降: 「settle が無いと >8K source は荒い」現象を許容するか、settle 不可な
  Off tier でも別経路 (例: 平面ズーム的なフォールバック) を出すか、別途検討

### 12.2 BaseOnly 状態の挙動 (確定済み、解像度ゲート版)

§3.6.4 / §3.6.4.3 のとおり、**BaseOnly は新規プロキシを作らず、既存 `fs_cache` を
そのまま流用**する。**360 機能による追加ピークはゼロ**だが、**既存 `start_fs_load`
通常パスの巨大画像デコードピーク (1.35 GB 等) は残る**: これは mIV の既存挙動で、
360 機能で新規発生するものではない (本機能のスコープ外)。

### 12.3 RAM 検出を廃止した理由 (ユーザー判断版に移行)

第 6 ラウンドまでの設計では `sysinfo` クレートでシステム RAM を検出し、Full /
Compressed / Off の 3 tier に分岐していたが、以下の理由で **解像度固定ゲート + ユーザー
確認プロンプト** に変更:

- **テストが困難**: 4 GB RAM 環境を再現するには実機 or Windows Job Object が必要、
  CI で機械的に検証できない
- **動的に変動する**: ロード直前に空き 6 GB だったが瞬間的に 3 GB に減るケースが
  あり、判定が不安定
- **判定境界が経験則**: 「空き RAM × 0.3」係数の最適値はユーザーごとに異なる
- **黙って劣化するより、ユーザーに判断機会を与えるほうが透明**

新方式 (200 MP 固定ゲート + バナー):

- ソース解像度は静的 = 判定が決定的、テストは境界値 1 種類で十分
- 「200 MP 超は約 X GB のメモリを使う」と数値で明示してユーザーに選択を委ねる
- セッション内で「今後も自動で高品質」を選べるので、繰り返しの確認は最小化

---

## 13. 投影方式 (現行仕様)

`backlog §1.59` の実装。透視固定だった逆射影を、方式を選べる形に一般化した。
**動画側 (`backlog §1.112`) の実行時 HLSL へそのまま移すことを前提に設計している**ので、
式・分岐位置・uniform の持ち方を変えるときは動画側の移植も同時に考えること。

### 13.1 4 方式と写像

半径 `r` (光軸からの像面上の距離) と入射角 `θ` (光軸からの角度) の対応だけが違う。

| 方式 | `r = g(θ)` | 逆変換 (`k = g(fov_y/2)`) | 定義域 |
| --- | --- | --- | --- |
| 透視 (既定) | `r = f·tan θ` | `θ = atan(r·k)`、`k = tan(fov/2)` | 常に有効 (`θ < π/2`) |
| 立体射影 | `r = 2f·tan(θ/2)` | `θ = 2·atan(r·k)`、`k = tan(fov/4)` | 常に有効 (`θ < π`) |
| 等距離 | `r = f·θ` | `θ = r·k`、`k = fov/2` | `r·k ≤ π` |
| 等立体角 | `r = 2f·sin(θ/2)` | `θ = 2·asin(r·k)`、`k = sin(fov/4)` | `r·k ≤ 1` |

正本は [`PanoProjection`](../src/panorama.rs) と [`ProjectionMap::theta`](../src/panorama.rs)。
WGSL 側の `projection_theta` ([panorama_wgpu.rs](../src/panorama_wgpu.rs)) が同じ分岐を持ち、
方式コード (`PROJ_*`) は `PanoProjection::shader_code` と 1:1 で対応する。番号がずれると
別の投影で描くので、両者を突き合わせる unit test を置いてある。

### 13.2 視野角の意味を全方式で共通にする

**「画面の上下端 (`r = 1`) に写る点の入射角が `fov_y / 2`」**と定義する。これを満たすように
`k = g(fov_y / 2)` を選ぶので、方式を切り替えても画角スライダの読みが連続する。

カメラ方向は `dir = (p̂·sin θ, -cos θ)`、`p = (ndc.x·aspect, ndc.y)`、`p̂ = p/|p|`。
透視ではこれが導入前の `normalize(ndc.x·tan_half·aspect, ndc.y·tan_half, -1)` と**恒等**に
なる (格子点で検証する回帰テストあり)。**既定の見え方は変えない**。

`k` は CPU 側で 1 回だけ計算して uniform (`proj.y`) へ載せる。シェーダで `fov_y` から
引き直さないのは、CPU の settle overlay と GPU の base が別々に `tan` を評価して幾何が
わずかにずれるのを避けるため。

### 13.3 画角上限

| 方式 | 上限 | 理由 |
| --- | --- | --- |
| 透視 | `FOV_MAX` = 2.6 rad (約 149°) | `tan` が 180° で発散する。**導入前と同じ値** |
| 非透視 3 方式 | `FOV_MAX_WIDE` = 5.93 rad (約 340°) | 180° を超えても発散しない。360° にしないのは立体射影の `tan(fov/4)` が 360° で発散するため |

方式を切り替えるときは必ず `PanoProjection::clamp_fov` を通す。立体射影で 300° まで
引いた状態から透視へ戻す経路が発散しないための不変条件で、ホイール / ピンチ /
ナビゲータ選択の 3 経路すべてが同じ helper を使う。

### 13.4 定義域外の画素

等距離と等立体角は、広い画角では**画面隅が定義域を出る** (球の裏側より遠い / イメージ
サークルの外)。そこは**不透明の黒**で塗る。魚眼レンズの出力そのものであり、透明にすると
overlay の下の base が透けて二重像になる。CPU (`render_settle_overlay`) と GPU (WGSL) で
同じ値を書く。

⚠ **WGSL は早期 return しない**。seam をまたぐ U 勾配の `dpdx` / `dpdy` を uniform
control flow 内で評価する契約 (§3.3) があるため、定義域外の判定は**最後の色選択**でだけ
行う。`fs_main` の `return` が 1 つであることを unit test で固定している。

ナビゲータの視野枠 ([ui_fullscreen.rs](../src/ui_fullscreen.rs)) も同じ写像を使い、
定義域外のサンプルでは折れ線を切る (存在しない点を通して線を引かない)。

### 13.5 状態の持ち方

- **`PanoramaState.projection` はセッション state**。画像の属性ではない。
- **`Settings::panorama_projection` が開始値**。360 表示中の切り替え
  (`KeyAction::FsPanoramaProjection` = 既定 <kbd>Shift</kbd>+<kbd>V</kbd>、および上部バーの
  投影ボタン) は**この既定値を書き換えない**。1 枚だけ試す操作を永続化しないため。
- `PanoramaState::reset` は投影方式を戻さない。リセットは「視点を初期向きへ」であって
  見え方の選択をやめる操作ではない。
- settle overlay の stale 判定キーは `PanoPose { yaw, pitch, fov_y, projection }`。
  以前は `(f32, f32, f32)` タプルだった。**視点の要素を足したときに比較へ入れ忘れると、
  古い投影で焼いた overlay がそのまま残る**ので 1 つの型に閉じた。新しい要素を足すときも
  この構造体へ足せば全ての比較経路が追随する。

### 13.6 UI

- 上部バーの**投影ボタンは 360 表示中だけ出る**。360 ボタンは ON 中に隠れる (× が解除を
  兼ねる、§6.1) ので、位置を奪い合わない。
- **クリックで一覧を開き、そこから選ぶ** (見開きボタンと同じ形)。当初は押すたびに順送り
  する形にしたが、4 方式あって「今どれを見ていて、次に何になるか」が押す前に分からない
  ため、2026-08 の利用者フィードバックで一覧へ変えた。順送りはキー
  (`FsPanoramaProjection`) にだけ残してある。
- 静止画の一覧は `App::panorama_projection_popup_open`、動画の一覧は native presenter の
  `NativeEguiOverlay` が開閉 state と実描画 rect を持つ。静止画は**見開き / フィット /
  スライドショー / 回転の各ポップアップと同じ guard 群**へ登録し、動画は実描画 rect を
  `compute_hud_regions` の HUD HWND 入力 region へ登録する。1 つでも登録から漏れると、一覧を
  クリックした入力が背面のキャンバスへ抜ける。360 を抜けたとき、または動画が音声モードへ
  移ったときは、持ち主のボタンごと消えるので各 state owner が必ず一覧を閉じる。
- 一覧から選ぶ経路とキーの順送りは `App::set_panorama_projection` へ合流させる。片方だけ
  画角の丸め (§13.3) や通知を落とさないため。
- **一覧の位置と大きさは実測して画面内へクランプする** (静止画は
  `clamp_bar_popup_rect`、動画は `native_panorama_projection_popup_rect`)。上バーの
  ボタンは右端から左へ並ぶので、投影ボタン (× / 鍵の隣) から左寄せで吊るすと画面外へ出る
  (2026-08 の利用者報告で実際に切れていた)。幅は方式名と説明の実測値から決める:
  UI 表示倍率 (`Settings::ui_scale_factor`) とフォント設定で文字幅が変わるため、固定値だと
  枠から出る。クランプは位置だけを動かし、大きさは変えない。
  - 既存の見開き / フィット / スライドショーの一覧は、ボタンがもっと左にあるので現状は
    画面内に収まっている (1280 幅で見開きの一覧は x=958〜1180)。同型の危険はあるが実害が
    出ていないので、この修正では触っていない。
- ボタンの絵は**その方式の半径写像そのもの**で描く。球のアイコンに引く経度線の位置を
  `θ = 30° / 60°` を `θ_max = 80°` で正規化した相対半径に置くので、透視は線が外周へ寄り、
  等距離は等間隔、等立体角は中心寄りになる。方式名はツールチップと切り替え時のトーストで
  示す (UI 文字列に環境依存グリフを使わない方針のため、記号は描かない)。
- 既定値は環境設定 →「見開き・表示」→「360 度ビュー」。

### 13.7 見え方が正しいかを確かめる (テストチャート)

**普通の風景写真では「変わった」ことしか分からない**。4 方式はどれも中心付近ではほぼ
同じに見え、違いは周辺と広い画角にしか出ないため、判断できるのは各方式が持つ幾何的な
不変条件だけである。それを画像側に埋め込んだチャートを生成する:

```
python scripts/gen_panorama_test_chart.py C:/tmp/miv-pano-test
```

4096×2048 (2:1) の PNG が 3 枚出る。GPano XMP は付かないが、アスペクト比 2:1 なので
`PanoramaTrigger::Hint` で 360 ボタンが有効になる (§2.1)。

| チャート | 何を見るか | 手順と、正しいときの見え方 |
| --- | --- | --- |
| `graticule.png` | **透視の直線性** | 視点を水平 (pitch 0) にして赤い赤道を画面に入れ、画角を最大近くまで広げる。**透視だけ赤道が完全な直線**。他の 3 方式は必ず弓なりに曲がる。経線 (縦線) も大円なので同じことが言える |
| `graticule.png` | **立体射影の等角性** | 同じ状態で経線と緯線の**交点の角度**を見る。立体射影は等角写像なので、画面の隅でも直角のまま。等距離・等立体角は隅で明らかに歪む (菱形になる) |
| `equidistant-rings.png` | **等距離の等間隔性** | 上を向いて (pitch を最大まで) 天頂を画面中心に置く。輪が同心円になる。**等距離だけ輪の半径が等間隔**。立体射影は外側ほど広がり、等立体角は外側ほど詰まる。太い輪が天頂から 30° / 60° / 90° |
| `equal-solid-angle.png` | **等立体角の等面積性** | 天頂を中心にして画角を広げる。市松の 1 マスは立体角が一定なので、**等立体角だけどのマスも画面上でほぼ同じ面積**になる。透視は外側ほど極端に大きく、等距離は中間 |
| いずれか | **リトルプラネット** | 真下 (pitch を下限まで) を向いて立体射影で画角を最大まで引く。地面が中心の円になり、周囲が地平線として囲む |

補助的な確認:

- **既定の見え方が変わっていないこと**が最優先。透視で開いたときの絵は投影モード導入前と
  ビット単位で同じはず (格子点の恒等性はテストで固定してあるが、実機でも見る)。
- **画角上限**: 透視でホイールを引き切ると約 149° で止まり、他の 3 方式は約 340° まで引ける。
  広げた状態から透視へ戻すと画角が自動で狭まる。
- **定義域外**: 等距離 / 等立体角で画角を広げると画面の四隅が黒くなる (§13.4)。透視と立体射影
  では黒くならない。
- **高画質化との一致**: 静止して settle overlay が焼き上がった瞬間に絵が動かないこと。
  動いたら CPU 側と GPU 側で写像がずれている。
- **ナビゲータ**: <kbd>Alt</kbd> の視野枠が実際に見えている範囲と合っていること。広い画角では
  枠が途切れる (定義域外で折れ線を切っている、§13.4)。

実写で試すなら、方式ごとに向く題材はこうなる (見え方の説明であって優劣ではない):

| 方式 | 向く題材 |
| --- | --- |
| 透視 | 建築・室内。直線が直線のまま写るので、柱や窓枠の歪みで違和感が出ない |
| 立体射影 | 屋外の広い場所を真下 / 真上から。リトルプラネットとして成立する |
| 等距離 | 魚眼レンズで撮った絵と見比べたいとき。全天の角度分布を素直に見る |
| 等立体角 | 空の被覆率など、面積の比を目で見積もりたいとき |

### 13.8 360 度動画の入力と導線

`backlog §1.112` 第 3 段を 2026-08-27 に実装した。動画も静止画と同じ
`App::panorama_state` と `PanoPose` を使い、ファイル移動時の保持、非 360 動画での一時的な
非アクティブ化、明示 OFF / フルスクリーン退出での破棄を同じ述語で管理する。動画用の yaw / pitch /
FOV state は持たない。native presenter は App から同期された pose の mirror だけを描画と入力判定に使う。

- マウス左ドラッグとタッチドラッグは `PanoramaState::apply_drag_delta`、ホイールは
  `apply_wheel_delta` へ合流する。360 中のホイールは修飾キーやカーソル位置、FOV の上下限に
  関係なく必ず消費し、ファイル移動へ漏らさない。
- タッチは presenter / HUD の開始領域 ownership を接触列の最後まで保持する。キャンバス側は
  12 logical pt を越えた時点で見回しへ latch し、確定フレームに DOWN からの全差分を含める。
  2 本目で pending tap を取り消し、開始位置へ戻ってもタップへ戻さない。ダブルタップは視点
  リセットに使わず、リセットは上バーの専用ボタンだけで行う。
- 上バーの 360 ボタンは固定スロットで、ON 中も強調したまま残る。非対応時は同じ位置で理由付き
  disabled。ON 中だけ投影方式と視点リセットを隣へ出す。動画の × は常に close であり、360 OFF
  には転用しない。
- タイル一覧は同じキャンバスの所有者なので 360 ON と排他にする。動画スケーラーは投影中だけ
  実効描画を休止し「360 投影中のため一時停止しています」と表示するが、利用者が選んだ
  `VideoScaleFilter` は書き換えない。色調 / LUT と再生 HUD は維持する。音声モード中は見えない
  視点が動かないよう入力だけを休止し、映像へ戻ると同じ pose を再開する。

---

## 14. 参考リンク

- [Google Photo Sphere XMP Metadata](https://developers.google.com/streetview/spherical-metadata)
- [Equirectangular projection (Wikipedia)](https://en.wikipedia.org/wiki/Equirectangular_projection)
- [Photo Sphere Viewer: Equirectangular tiles](https://photo-sphere-viewer.js.org/guide/adapters/equirectangular-tiles.html)
  — 超高解像度向けタイル方式の参考実装 (Phase 3 拡張候補)
- [turbojpeg-sys: tj3SetCroppingRegion](https://docs.rs/turbojpeg-sys/latest/turbojpeg_sys/fn.tj3SetCroppingRegion.html)
  — Compressed tier 部分デコードに使う API (Phase 3)

**コードリファレンス**:

- 既存実装テンプレ: `src/compare_wgpu.rs` (`CompareShaderCallback` の構造)
- ピクセル変換: `src/capture.rs:393` (`color_image_to_rgba`)
- メタデータキー規約: `src/app.rs:12144` (`metadata_cache_key`)
- 既存 wgpu 設定: `src/main.rs:826` (`WgpuConfiguration`)
- 既存入力ハンドリング: `src/ui_fullscreen.rs:3613-3622` (Wheel / Ctrl+Wheel)、
  `src/ui_fullscreen.rs:2904-2910` (P キーの consume)
- アップロードペーシング: `src/app.rs:16604` (`poll_prefetch`、`fs_upload_backlog`
  パターン、§4.1.1 で流用)
- 既存サムネリサイズ: `src/fast_resize.rs:37` (`resize_rgba8_exact`、矩形専用、
  §3.6.3 で **流用しない理由**)
- 既存 turbojpeg 利用箇所: `src/thumb_loader.rs:419` (safe crate のみ、Phase 3 で
  turbojpeg-sys に拡張する基準点)
