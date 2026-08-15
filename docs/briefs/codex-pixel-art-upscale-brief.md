# ドット絵拡大 (pixel-AA) ポストフィルタ 追加ブリーフ

## 0. 依頼概要

`PostFilter` に **ドット絵拡大** を 1 つ追加する。既存の拡大アルゴリズム
(`None` = Lanczos3 / `Nearest` / `UpscaleSharp` = NIS / `UpscaleAnime` = Anime4K) の
挙動・既定値・cache identity は**一切変更しない**。純粋な追加のみ。

用途は「ドット絵・レトロ画面キャプチャ・線が硬い低解像度画像を、任意倍率で
ドットの粒が揃ったまま拡大する」こと。現状の穴は次のとおり:

- `Nearest` は非整数倍率でドットの大きさが不揃いになる (2px と 3px が混在)
- `None` (Lanczos3) はぼけとリンギングが出る
- 中間が無い

## 1. 前提として読むもの

- [docs/display-pipeline.md](../display-pipeline.md) §2.4.1 (フルスクリーン静止画の
  GPU Lanczos3 縮小・標準拡大と選択式拡大) — **正本**
- [docs/upscale-algorithm-selection.md](../upscale-algorithm-selection.md) 末尾の
  `UpscaleSharp` / `UpscaleAnime` の節 — 追記先
- [src/gpu_lanczos.rs](../../src/gpu_lanczos.rs) の `UpscaleNis` 経路 — **今回の実装テンプレート**
  (1 pass fragment shader + uniform 1 個 + source texture 1 個)
- [src/gpu_nis.wgsl](../../src/gpu_nis.wgsl) / [src/gpu_lanczos_visible_upscale.wgsl](../../src/gpu_lanczos_visible_upscale.wgsl)

## 2. 決定事項 (この通りに実装する。設計判断を作り直さない)

| 項目 | 決定 |
| --- | --- |
| enum variant | `PostFilter::UpscalePixelArt` |
| UI ラベル | `ドット絵拡大` |
| wire 値 (serde snake_case) | `upscale_pixel_art` |
| scale branch | `FullscreenPaintScaleBranch::UpscalePixelArt`、`as_str()` = `"upscale_pixel_art"` |
| KeyAction | `FsPostFilterUpscalePixelArt` (既定キーなし。他の `FsPostFilter*` 直接指定と同じ) |
| shader ファイル | `src/gpu_pixel_aa.wgsl` (新規) |
| UI 上の位置 | 「基本」グループの `UpscaleAnime` の**直後**。`PostFilter::ALL` / パネル / keymap 一覧でも同じ順 |
| アルゴリズム | pixel-AA (面積被覆重み付きの分離可能 2 タップ補間)。詳細は §3 |
| シャープネス | **固定 1.0** (= 厳密な面積平均)。設定項目は追加しない |
| 色空間 | **この経路の中だけ** sRGB EOTF/OETF を通してリニア空間で混合する (§3.4) |
| 適用範囲 | 物理倍率 > 1.0 のときだけ。1.0 は `OriginalOneToOne`、< 1.0 は `DownscaleLanczos` (既存のまま) |
| source 長辺上限 | 設けない (`UpscaleAnime` のような上限設定は追加しない)。出力上限は既存と共通 |

### 2.1 やってはいけないこと

- 既存 3 種の拡大シェーダ・縮小シェーダ・`smoothing_percent` の意味を変える
- 既定の `PostFilter::None` の見え方を変える
- egui / eframe / vendor 配下を触る
- 新しい設定項目・新しい永続フィールドを増やす (§5 の downgrade stash を除く)
- 症状パッチ (guard / retry / 追加 repaint) で辻褄を合わせる

## 3. アルゴリズム仕様

出力 1 ピクセルごとに、source の 2×2 テクセルを**軸ごとの面積被覆重み**で混ぜる。
重みが 0/1 に飽和する領域が広いため、テクセルの内側は完全に平坦 (= Nearest と同じ) で、
テクセル境界だけが**常に出力 1 ピクセル幅**で滑らかにつながる。倍率が上がっても
にじみ幅は 1 ピクセルのままなので、高倍率でもぼけない。

### 3.1 座標

`params.source_region` = 可視 source 領域 (source ピクセル単位、`[min_x, min_y, width, height]`)。
既存の `UpscaleNis` / `VisibleUpscale` と同じ `source_region_pixels()` の出力を使う。

```
target_coord = vec2<f32>(vec2<u32>(in.position.xy))       // 出力ピクセルの整数 index
scale        = vec2<f32>(params.target_size) / params.source_region.zw
tx_per_px    = 1.0 / scale                                 // 出力 1px あたりの source テクセル数
center       = params.source_region.xy + (target_coord + 0.5) / scale
coord        = center - 0.5                                // テクセル中心を整数に写す
base         = floor(coord)
frac         = coord - base
```

### 3.2 軸ごとの重み (固定シャープネス 1.0)

```
// sharpness s = 1.0 のとき遷移帯は 0.5 を中心に幅 tx_per_px。
// = 出力ピクセルの箱が隣のテクセルを覆う面積そのもの (厳密な box フィルタ)。
lb = 0.5 - 0.5 * tx_per_px
ub = 0.5 + 0.5 * tx_per_px
w  = clamp((frac - lb) / (ub - lb), 0.0, 1.0)      // ub - lb <= 0 のときは step(0.5, frac)
```

- `tx_per_px -> 0` (高倍率) で階段関数 = Nearest に収束する
- `tx_per_px == 1` (等倍) で `w == frac` = 素のバイリニアに一致する
- 分母がゼロに潰れる場合のガードを入れる (`ub - lb < 1e-6`)

これは libretro の `pixel_aa` (fishku) が使う slopestep の `slope = 1.0` / シャープネス 1.0 の
場合と数学的に同一で、libretro 側の "pixellate" 相当。`slope > 1` への一般化は将来の
拡張点なので、**式の由来をコメントに残す**こと (実装はしない)。

### 3.3 タップ

`base + (0,0) / (1,0) / (0,1) / (1,1)` を `textureLoad` する。
座標は **source テクスチャ全体** (`0 .. source_size-1`) に clamp する
(可視領域の外でもテクスチャ内なら正しい隣接画素なので、そのまま使う。
`gpu_lanczos_visible_upscale.wgsl` の clamp と同じ考え方)。

### 3.4 色空間 (この経路の中だけリニア)

egui のテクスチャは **sRGB エンコード済み・premultiplied alpha** の `Rgba8Unorm`。
そのまま重み付き平均すると、隣接コントラストが最大なドット絵で
「白線が細り黒線が太る」古典的な誤差が出る。この新経路の中だけで正しく処理する:

1. 各タップを un-premultiply する (`rgb / max(a, eps)`、`a == 0` のタップは rgb = 0)
2. sRGB EOTF でリニアへ (`vendor/egui-wgpu/src/egui.wgsl` の `linear_from_gamma_rgb` と
   **同じ区分関数**を使う。`pow(2.2)` 近似は使わない)
3. リニア空間で premultiply し直し、`w` で 2 段 mix する (x 方向 → y 方向)。alpha も同じ `w` で mix
4. 混合後に un-premultiply → sRGB OETF (`gamma_from_linear_rgb` と同じ区分関数) → alpha で premultiply
5. 最後に `rgb = min(rgb, vec3(a))` で premultiplied 不変条件を保つ
   (既存 NIS / Anime4K と同じ「premultiplied RGB を alpha 以下に保つ」規約)

**既存経路には EOTF 対応を入れない。** 既定 Lanczos3 / NIS / Anime4K / 縮小 / サムネイルは
現状のまま (gamma 空間) で据え置く。理由: 見え方が変わる退行になるため。
新経路だけなら退行が原理的に起きない。

### 3.5 uniform レイアウト

`nis_params_uniform` と同じ 32 byte レイアウトの `pixel_aa_params_uniform` を追加する
(共用せず別関数にする。将来 sharpness を足すときに NIS を巻き込まないため)。

```wgsl
struct PixelAaParams {
    target_size: vec2<u32>,   //  0..8
    source_size: vec2<u32>,   //  8..16
    source_region: vec4<f32>, // 16..32  (min_x, min_y, width, height)
};
```

`texture_fetches` (perf 用の推定値) は `target_pixels * 4`。

## 4. 実装チェックリスト (これを全部通す)

以下は `UpscaleAnime` / `UpscaleNis` を grep して洗い出した接続点。**漏らすと
「メニューには出るが効かない」「キーが効かない」形で出る**。

### 4.1 `src/adjustment.rs`
- [ ] `PostFilter::UpscalePixelArt` を `UpscaleAnime` の直後に追加
- [ ] `POST_FILTER_GROUP_BASIC` に追加
- [ ] `PostFilter::ALL` に追加
- [ ] `display_label()` = `"ドット絵拡大"`
- [ ] `rewrites_pixels()` の `false` 側 (`None | Nearest | UpscaleSharp | UpscaleAnime`) に追加
- [ ] `needs_nearest_sampler()` は **false のまま** (追加しない)
- [ ] enum の doc コメントを更新

### 4.2 `src/post_filter.rs`
- [ ] CPU stage の identity clone 分岐 (29 行付近) に追加
- [ ] 分類テーブル (1967 行付近) に追加
- [ ] `upscale_anime_is_identity_clone_on_the_cpu_stage` と同型のテストを追加

### 4.3 `src/gpu_lanczos.rs`
- [ ] `FullscreenPaintScaleBranch::UpscalePixelArt` を追加、`uses_resampler()` / `as_str()`
- [ ] `fullscreen_paint_scale_branch()` の `match post_filter` に追加
- [ ] `PixelArtUpscalePlan` を `NisUpscalePlan` と同型で追加 (`texture_fetches` だけ違う)
- [ ] `LanczosWorkPlan` / `LanczosWorkJob` に variant 追加 + 各 `match` を網羅
- [ ] `work_plan_for_key()` に分岐追加 (可視領域が無い場合の full-rect fallback は NIS と同じ)
- [ ] `target_and_source_region_for_branch()` の拡大 3 branch の列挙に追加
- [ ] `lanczos_target_decision()` の拡大上限判定に追加
- [ ] `prune_upscale_pixels()` の 2 か所の拡大 branch 列挙に追加
- [ ] `Lanczos3Resampler` に `pixel_aa_pipeline` + `prepare_pixel_aa_job` + `encode_pixel_aa`
      (NIS と同じ `NisJob` 形の 1 pass。`PixelAaJob` を別に作ってよい)
- [ ] `pixel_aa_params_uniform` 追加
- [ ] `scale_branch_with_anime_source_limit()` は **触らない** (Anime 固有)

### 4.4 `src/gpu_pixel_aa.wgsl` (新規)
- [ ] §3 の仕様どおり。entry point は `vs_main` / `fs_pixel_aa`
- [ ] `linear_from_gamma_rgb` / `gamma_from_linear_rgb` は egui.wgsl と同じ区分関数を写す。
      「egui.wgsl と同一に保つこと」のコメントを付ける
      (`gpu_lanczos_visible_upscale.wgsl` の sinc 重複コメントと同じ運用)

### 4.5 `src/keymap.rs`
- [ ] `KeyAction::FsPostFilterUpscalePixelArt` を `FsPostFilterUpscaleAnime` の直後に追加
- [ ] `ini_name()` / 説明文 (`"ポストフィルタをドット絵拡大にする"`) / `context()` / `trigger()` /
      `default_chords()` (なし) / `ALL_ACTIONS` / 4531・4932・5370 行付近の列挙 / 9874 行付近のテスト
- [ ] `docs/keymap.ini.default` の該当箇所

### 4.6 `src/ui_fullscreen.rs`
- [ ] `FS_POST_FILTER_DIRECT_ACTIONS` に `(FsPostFilterUpscalePixelArt, UpscalePixelArt)` を追加
- [ ] 16363 / 16391 行付近の Anime 上限フォールバック通知は **Anime 固有なので触らない**

### 4.7 `src/ui_adjustment_panel.rs`
- [ ] 8866 行付近の「基本」ラジオ列挙に追加 (Anime の直後)

### 4.8 `src/settings.rs` — **最重要**
- [ ] `PostFilterDowngradeStash` に `#[serde(default)] pixel_art_upscale: bool` を追加し、
      `stash_for_persist` / `restore_after_load` を 3 変数対応にする
- [ ] `restore_after_load` の優先順は既存 (anime → sharp) の後ろに pixel_art を足す形でよいが、
      **同時に 2 つ立つことは無い**不変条件をコードで維持する
- [ ] `post_filter_stash_keeps_persisted_presets_v211_compatible` を更新
      (`upscale_pixel_art` が JSON に出ないことも assert)
- [ ] `post_filter_stash_restores_each_new_variant_independently` の配列に追加

> **なぜ必須か**: 古い mIV が新しい設定 JSON を読んだとき、未知の enum 値は
> `Incompatible` 扱いになり保存抑止 / quarantine を誘発する。この stash は
> そのための既存の仕組みで、新変種を足すたびに更新しないと守れない。
> `stash_post_filter_variants_for_persist()` の呼び出し元を全部辿り、
> `PostFilter` が永続化される経路が他に無いかも確認すること。

### 4.9 `src/remote_ipc/`
- [ ] ポストフィルタ一覧は `POST_FILTER_GROUPS` から自動生成されるので実装変更は不要のはず。
      `parse_post_filter_wire()` が新値を往復できることを確認する
- [ ] `src/remote_ipc/mod.rs:721` 付近のテスト期待値配列に `"upscale_pixel_art"` を追加
- [ ] remote protocol version の bump が必要かを判断し、**不要と判断したらその理由を
      コミットメッセージに残す** (一覧はデータとして送られるだけ、が想定)

## 5. テスト (必ず追加する)

1. `gpu_lanczos.rs` の既存 branch テストと同型:
   物理 2.0 倍 → `UpscalePixelArt` / 1.0 倍 → `OriginalOneToOne` / 0.75 倍 → `DownscaleLanczos`
2. **重み関数の純関数テスト**: §3.2 の式を Rust 側にも `pixel_aa_axis_weight(frac, tx_per_px)`
   として置き (shader と同一に保つコメント付き)、次を検証する
   - `tx_per_px == 1.0` で `w == frac` (バイリニア一致)
   - `tx_per_px -> 0` で 0.5 未満は 0.0 / 0.5 超は 1.0 (Nearest 一致)
   - `frac` に対して単調非減少、値域 [0,1]
   - `tx_per_px = 0.25` (4 倍) で遷移帯が `frac in [0.375, 0.625]` に収まる
3. `post_filter.rs`: CPU stage が identity clone であること
4. `adjustment.rs`: `rewrites_pixels() == false`
5. `keymap.rs`: ini 名の往復、`ALL_ACTIONS` 包含、context が既存 `FsPostFilter*` と同じ
6. `settings.rs`: §4.8 の 2 テスト
7. `remote_ipc`: 期待値配列

`cargo test -p mimageviewer --lib` が緑になること。UI スナップショットに影響が出たら
[docs/ui-snapshot-policy.md](../ui-snapshot-policy.md) の手順で更新する。

## 6. ドキュメント更新 (コードと同時)

- [ ] `docs/display-pipeline.md` §2.4.1 — 分岐一覧に「1.0 超かつ `PostFilter::UpscalePixelArt`」の
      項目を追加。perf の `scale_branch` 列挙にも `upscale_pixel_art` を足す。
      **この経路だけリニア空間で混合する**ことと、その理由 (新経路なので退行が起きない) を明記
- [ ] `docs/upscale-algorithm-selection.md` — `UpscaleAnime` の節の後ろに新節。
      pixel-AA を選んだ理由 (非整数倍率でドット粒が揃う / smoothstep 単体は倍率非依存で
      高倍率でぼける / パターン認識系 (hqx・xBR・MMPX) は整数倍固定で本物のドット絵限定なので
      今回は採らない) と、シャープネス固定 1.0 の根拠を残す
- [ ] `docs/keymap-spec.md` 419 行付近の例示に追加
- [ ] `docs/spec.md` のポストフィルタ一覧
- [ ] `htdocs/mimageviewer/manual/adjustment.html` — ポストフィルタ「基本」の説明に追加。
      **バージョン番号・実装用語 (pixel-AA / シェーダ / EOTF 等) は書かない**。
      「ドットの大きさを揃えたまま拡大します。等倍でないズームでもドットが不揃いになりません」
      のような効果ベースの記述にする
- [ ] `htdocs/mimageviewer/manual/settings.html` / `remote.html` にポストフィルタ一覧が
      あれば同様に追加
- [ ] `README.md` の更新履歴は**触らない** (リリース時にまとめる)

## 7. 完了報告に含めるもの

- 変更ファイル一覧と、§4 チェックリストの消化状況
- `cargo test -p mimageviewer --lib` の結果
- remote protocol version を bump しなかった場合はその判断根拠
- 実装中に見つけた、ブリーフと食い違う既存構造 (あれば)
