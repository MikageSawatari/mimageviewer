# ブリーフ: GPU Lanczos3 の製品統合 (v2.11.0 段階4)

対象: v2.11.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode。

**正本は [docs/dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md)。
特に §4.3 / §4.3.2 / §4.4 / §9 を必ず読むこと。**

段階3 spike は commit `96eed1ea` で完了。結論は正本 §4.3.2 に記録済み。

---

## 1. 段階4 でやること

spike で go 判定が出た **C-1 方式**で、Lanczos3 縮小を製品経路へ統合する。

### 1.1 ownership 境界 (spike の結論。ここを外すと壊れる)

1. **元の `TextureHandle` と各 cache は一切置き換えない。** `size_vec2()` の owner として残す
2. `DisplayedImageTransform` は元ハンドルの論理寸法から従来どおり解決する
3. Lanczos 出力は wgpu texture + 登録済み `TextureId` の **typed resource として別に持つ**
4. **`DisplayedImageTransform::paint_texture` に渡す `TextureId` だけを差し替える**

`paint_texture` は実テクスチャ寸法を読まず、0..1 の UV と `TextureId` だけを Mesh に載せる。
これが分離が成立する根拠なので、この性質を壊す変更をしないこと。

### 1.2 mip 前縮小は実装しない (2026-08-04 決定)

**全縮小率で level 0 から直接 Lanczos する。** 正本 §4.3.2 の「ClaudeCode 検収時の追加結論」参照。

- コストはソース画素数で有界。`fetch ≈ 6 × W_src × H_src × (1 + s)` で、縮小率が小さいほど安い
- 長辺 8192 clamp があるため最悪ケースも約 1.6ms (RTX 4090 実測スループット換算)
- box mip 経由は品質が明確に劣る (MAE 3.86〜4.38、最大差 97〜111)

したがって**切替閾値もヒステリシスも不要**。spike の `LanczosPlan` から mip 関連を落とす。

### 1.3 適用範囲 (§4.4 の 3 分岐)

| 実効の物理スケール | 処理 |
| --- | --- |
| < 1.0 (縮小) | Lanczos リサンプル → 出力 `TextureId` を貼る |
| = 1.0 (ドットバイドット) | **リサンプルなし。元テクスチャを直接貼る** |
| > 1.0 (拡大) | リサンプルなし。従来どおり (ポストフィルタ設定に従う) |

**判定は段階1・2 で導入した `physical_scale_is_near_integer` と整合させること。**
1.0 ちょうどで両者の結果が一致するので、境界で見た目が飛ばない。

### 1.4 統合する経路

単ページだけでなく、**同じ typed paint resource を通すこと**。別の bool / Option を増やさない。

- 単ページ
- 見開き (左右それぞれ。ページごとに実効倍率が違い得る)
- 連結読み (縦 / 横)
- nav holdover (`fs_nav_holdover_for_draw`)
- detached frozen snapshot

### 1.5 対象外

- **ルーペ** — 元 `TextureHandle` 経路のまま。拡大鏡なので縮小テクスチャではボケる
- **pixel grid** — 論理サイズ必須。拡大時のみ描画されるので §1.3 の縮小分岐と排他
- 比較表示 (wipe/diff) と 360 度パノラマ — 既存 callback が別 ownership。段階4 では触らない
- サムネイル / animated frame / 動画 / mask / checker / UI preview
- **旧 trilinear 表示経路と設定の削除は段階5** (別ブリーフ)。今回は追加のみ

---

## 2. 設計課題 (spike が残リスクとして挙げたもの)

1. **key 設計** — source texture の identity / generation、target size、回転・トリムの扱い。
   古い source generation の出力を採用しないこと
2. **再生成の契機** — 表示矩形が変わったとき。リサイズ中は毎フレーム変わるので、
   **target size を切り上げ量子化 (例: 64px 単位) して再確保を減らす**こと。
   量子化すると出力が表示サイズちょうどでなくなるが、その差は GPU の線形補間で吸収される。
   量子化幅は画質への影響を確認して決めること
3. **`TextureId` の解放** — `register_native_texture` で登録したものの free / update 漏れを防ぐ。
   viewer context 単位の cancel / evict と揃える
4. **VRAM 会計** — 連結読みの texel 予算
   ([final-composite-budget-thrash-plan.md](final-composite-budget-thrash-plan.md)) へ
   出力テクスチャを実寸で計上する。過去にここで thrash の実害が出ている
5. **perf 計装** — リサンプルの所要時間と再生成回数を `--perf-log` に出す。
   段階5 の判断材料と、退行検知に使う

---

## 3. 品質の確認

**CPU 参照実装と一致すること。** 素材は `C:\tmp\miv-downscale-compare\`
(バックアップ `C:\home\mimageviewer_testdata_downscale\`)。

spike では level 0 直接経路が **最大 1 階調差 / MAE 0.019〜0.030** を達成している。
統合後もこの水準を保つこと。量子化を入れる場合は、それによる差を測って報告すること。

判定に使える自動テストがあれば残すこと。実機での目視は利用者が行う。
**Codex はアプリを起動しない。** 検証ビルドは ClaudeCode が用意する。

---

## 4. テスト

- 3 分岐 (縮小 / 1.0 / 拡大) の切り替えが `physical_scale_is_near_integer` と整合すること
- 1.0 ちょうどでリサンプル経路に入らないこと (ドットバイドットを壊さない)
- 見開きで左右のページが別々の実効倍率でも正しく処理されること
- 連結読み・holdover・detached snapshot が同じ resource を通ること
- source generation が変わったら古い出力を採用しないこと
- `TextureId` が解放されること (リーク検知)
- 段階1・2 の既存回帰テストが通ること

実行: `cargo test -p mimageviewer --lib`

---

## 5. 制約

- detached-rework 凍結ルールは有効。触れた範囲を
  [detached-rework-plan.md](detached-rework-plan.md) へ記録する
- 表示テクスチャの優先順位と `edit -> color -> final AI -> smart sharpen -> post_filter` の
  合成順序は変更しない ([display-pipeline.md](display-pipeline.md))
- **新規の vendoring はしない。** `register_native_texture` (renderer.rs:823) と
  `CallbackTrait` は既存の公開 API
- ブランチ操作・コミットは不要
- `cargo fmt` (引数なし) と `cargo test -p mimageviewer --lib` を通すこと
- [display-pipeline.md](display-pipeline.md) と正本を実装に合わせて更新する

## 6. 参照

- [dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md) — **正本**
- [brief-v2.11.0-lanczos-spike.md](brief-v2.11.0-lanczos-spike.md) — 段階3 のブリーフ
- `src/gpu_lanczos_spike.rs` / `.wgsl` — spike の実装 (dev-tools gated)
- `src/bin/gpu_lanczos_spike.rs` — 計測・CPU 参照比較の probe
- `src/displayed_image_transform.rs` — `paint_texture` の分離点
