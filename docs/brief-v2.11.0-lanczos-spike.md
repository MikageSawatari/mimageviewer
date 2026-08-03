# ブリーフ: GPU Lanczos3 の spike (v2.11.0 段階3)

対象: v2.11.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode。

**正本は [docs/dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md)。
着手前に §3.2 / §3.3 / §4.3 / §4.4 / §9 を必ず読むこと。**

段階1・2 (物理 1:1 + ピクセルスナップ + 見開き高さ合わせの 1:1 例外) は実装・実機確認済み
(commit `b38610d5`)。本ブリーフは **spike のみ**で、本実装 (段階4) は別途。

---

## 1. spike の目的

**本実装に入る前に、案C が成立するかを判断する。** 成立しないと分かった時点で止めるのが
このタスクの価値であり、動くものを作り切ることではない。

答えを出すべき問いは 3 つ。

### 問い A (go/no-go): `size_vec2()` の分離が成立するか — **最優先**

2026-07-08 の調査メモ (`git show 2b68c27f:docs/downscale-moire-lod-plan.md` §4.3) が
警告しており、**当時 mipmap を選んだ最大の理由**がこれ。

> `draw_fs_image` は `handle.size_vec2()` を画像の論理サイズとして使っており、この用法は
> `ui_fullscreen.rs` 内に 10 箇所以上ある。縮小段のテクスチャをそのまま渡すと
> 「画像そのものが 1/4 サイズ」と誤解し、原寸表示・ズーム・ルーペ・pixel grid がずれる。

現在 `size_vec2()` は `ui_fullscreen.rs` だけで 29 箇所。**表示サイズのテクスチャを貼る案C は
この罠を正面から踏む。**

やること:

1. `size_vec2()` の全使用箇所を洗い出し、次に分類する
   - フルスクリーン静止画の表示経路上にあり、差し替えたテクスチャを見るもの
   - そのうち「論理画像サイズ」として使っており、差し替えると壊れるもの
   - 無関係なもの (サムネイル / UI / 動画 / mask 等)
2. 壊れるものについて、`DisplayedImageTransform` の `source_size` / `texture_size` 経由へ
   寄せられるか、現実的な工数で判断する
3. **ルーペは原寸固定、pixel grid は論理サイズ必須**なので、縮小テクスチャの対象から
   外す方法も併せて検討する

### 問い B: どちらの統合方式を採るか

2 案あり、spike で比較して**推奨を 1 つ出す**こと。

| 案 | 方式 | 利点 | 懸念 |
| --- | --- | --- | --- |
| **C-1** | 表示サイズの自前 wgpu texture を `register_native_texture` で登録し、貼るテクスチャを差し替える | 描画経路 (`paint_texture` の Mesh) を変えない | 問い A の分離が必須 |
| **C-2** | `egui_wgpu::CallbackTrait` の paint callback 内でリサンプルして直接描く | `size_vec2()` に影響しない | 自由回転・表示トリムの UV crop・見開き・tint・クリップを callback 側で再現する必要がある |

どちらも **egui-wgpu の公開 API だけで実装可能**で、**新規の vendoring は不要**
(`register_native_texture` は renderer.rs:823、`CallbackTrait` は
`src/compare_wgpu.rs` / `src/panorama_wgpu.rs` に実運用例が 3 箇所ある)。

### 問い C: GPU コストはどれくらいか

開発機 (RTX 4090) で実測する。低スペック GPU の実測は行わない (正本 §3.3 の A案)。

代わりに、**ミップ前縮小でタップ数が設計どおり有界になっている**ことをコードと計測で
確認すること。正本 §4.3 の見積もりでは、ソース解像度によらず 30〜120M フェッチに収まる。

---

## 2. 実装するもの (最小限)

判断に必要な最小のプロトタイプだけ。製品品質にしない。

- separable Lanczos3 の 2 パス (WGSL)。**縮小時はカーネルを 1/scale 倍に引き伸ばすこと**
  (正本 §4.3 の「実装上の必須事項 その2」。固定幅 6 タップにするとローパスが効かず
  モアレが残り、§3.3 の測定結果が再現しない)
- ミップ前縮小: `L = floor(log2(1/s))` で残り比率を 0.5〜1.0 に収める
- 雛形は `vendor/egui-wgpu/src/mipmap.rs` (183 行) + `mipmap.wgsl` (48 行)。
  render-to-texture パイプラインの構造をそのまま使える
- 一時的なフラグや専用の確認経路で構わない。既存の表示経路を壊さないこと

## 3. 品質の確認方法

**CPU 参照実装と見比べる。** 素材は `C:\tmp\miv-downscale-compare\`
(バックアップ `C:\home\mimageviewer_testdata_downscale\`)。

- `src_2480x3508.png` を mIV で表示し、`full\r0.41_lanczos3.png` (CPU 参照) と比較する
- 特に **網点トーン領域が参照実装と同じく平坦になる**こと。残留格子 (モアレ) が出たら
  カーネルの 1/scale 引き伸ばしを疑う
- 1px 細線が現行 (mip+trilinear) より残ること

実機での目視は利用者が行う。**Codex はアプリを起動しないこと。** 検証用ビルドは
ClaudeCode が用意する。数値比較で確認できる部分があれば、テストとして残してよい。

## 4. 報告してほしいこと

1. **問い A の結論** — 分離が成立するか。成立するなら壊れる箇所と対処方針、工数感。
   **成立しないなら、その根拠を示して止めること** (案C の見直しへ戻す)
2. **問い B の推奨** — C-1 / C-2 のどちらか。理由と、選ばなかった方の懸念
3. **問い C の実測値** — 代表的な縮小率 (0.63 / 0.41 / 0.25) での所要時間。
   ミップ前縮小の有無での差。タップ数が有界になっていることの確認
4. **品質比較の結果** — CPU 参照実装との差
5. 段階4 (本実装) の工数見積もりと、想定されるリスク

## 5. 制約

- **段階4 の本実装には進まないこと。** spike の結論を出したら止めて報告する
- detached-rework 凍結ルールは有効。触れた範囲は
  [detached-rework-plan.md](detached-rework-plan.md) へ記録する
- 表示テクスチャの優先順位と `edit -> color -> final AI -> smart sharpen -> post_filter` の
  合成順序は変更しない ([display-pipeline.md](display-pipeline.md))
- 拡大側 (物理倍率 1.0 超) は対象外。正本 §4.4 の 3 分岐を守る
- サムネイル / animated frame / 動画 / mask / checker / UI は対象外
- 連結読みの texel 予算 ([final-composite-budget-thrash-plan.md](final-composite-budget-thrash-plan.md))
  への計上が必要になる点は、段階4 の設計課題として報告に含める
- ブランチ操作・コミットは不要
- `cargo fmt` (引数なし) と `cargo test -p mimageviewer --lib` を通すこと

## 6. 参照

- [dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md) — **正本**
- `git show 2b68c27f:docs/downscale-moire-lod-plan.md` — 2026-07-08 の調査メモ (§4.3 の警告)
- [downscale-moire-lod-plan.md](downscale-moire-lod-plan.md) — v2.7.0 の mipmap 実装
- `vendor/egui-wgpu/src/mipmap.rs` / `mipmap.wgsl` — render-to-texture の雛形
- `src/compare_wgpu.rs` / `src/panorama_wgpu.rs` — CallbackTrait の実運用例
- `C:\tmp\miv-downscale-compare\gen_downscale_compare.py` — CPU 参照実装 (`resample_1d` の
  `filt_scale` がカーネル引き伸ばしの該当箇所)
