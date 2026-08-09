# ブリーフ: 比較シェーダがズーム / パンで潰れる (§1.60 の残り)

対象: v2.13.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode / 実機確認 = 利用者。

正本: [docs/next-release-backlog.md](next-release-backlog.md) §1.60。
直前のコミット `a26f76da` で fraction の定義は揃えたが、**シェーダの配置がズーム / パンに
対応していない**ことが実機で判明した。その残りを直す。

前提: master の作業ツリー。着手前に `git log --oneline -3` で HEAD を確認すること。

---

## 1. 症状 (実機 2026-08-09、スクリーンショット 2 枚で確認済み)

1. **`Shift+C` のワイプでズームすると、縦横比が崩れ、シェーダの継ぎ目が白線とずれる**
2. **`Alt+C` でパンすると、合成画像全体が画面上部の帯に潰れて入る**

## 2. 原因 (特定済み。再調査不要)

`CompareShaderCallback::paint` ([src/compare_wgpu.rs:386](../src/compare_wgpu.rs)) は
**`PaintCallbackInfo` を一切見ず**、NDC ±1 の全面 quad (`draw(0..6)`) を描くだけ。
位置決めは egui-wgpu が callback rect から行う `set_viewport` に完全に依存している
([vendor/egui-wgpu/src/renderer.rs:564](../vendor/egui-wgpu/src/renderer.rs) の
`info.viewport_in_pixels()` → `render_pass.set_viewport`)。

`compare_shader_shape` は callback rect に **`draw_rect` をそのまま**渡している
([src/ui_fullscreen.rs:21986](../src/ui_fullscreen.rs) 付近)。ズームやパンで `draw_rect` が
画面外へ出ると、egui 側が viewport を画面内へクランプするため、**合成画像の全体が
残った領域へ押し込まれる**。これが「縦横比が崩れる」「帯に潰れる」の正体で、
継ぎ目が白線とずれるのも同じ理由 (線は `draw_rect` 基準、シェーダはクランプ後の矩形基準)。

**これは元からの制限**であり、`a26f76da` が作った退行ではない。等倍かつ画像が画面内に
収まっているときだけ両者が一致していた。

## 3. 直し方

**コールバック矩形を画面内に収め、切り落とした分を uv 窓としてシェーダへ渡す。**

1. `compare_shader_shape` で `visible = draw_rect.intersect(image_rect)` を求める
   (`image_rect` = 呼び出し側が渡すビューポート矩形)。空なら `None` を返す
2. `visible` が `draw_rect` のどの範囲かを 0..1 の uv 窓として求める
   (`(visible.min - draw_rect.min) / draw_rect.size()` と max 側も同様)
3. callback rect には **`visible`** を渡す
4. uniform に uv 窓 (`vec4`) を足し、WGSL のサンプル座標を
   `mix(uv_min, uv_max, in.uv)` にする
5. **ワイプの継ぎ目は合成画像の座標系のまま判定する**。つまりシェーダ内で使う比較値は
   quad ローカルの `in.uv.x` ではなく、上で求めた**合成画像上の u** にする。こうすれば
   継ぎ目の画面 x は `compare_wipe_screen_x(draw_rect, fraction)` と一致する

### やらないこと

- `set_viewport` へ描画先より大きい矩形を渡す (wgpu の検証エラーになる)
- ズーム倍率やパン範囲を制限して症状を避ける
- 比較中だけシェーダ経路をやめて CPU 合成へ落とす
- 既存の CPU fallback (`#[cfg(not(windows))]` / 準備済みテクスチャ) の挙動を変える。
  こちらは `painter.image` なので egui が正しくクリップしており、元から問題ない

### 影響範囲の注意

ナビゲータの比較描画 (`draw_compare_navigator_content`) は `zoom_pan = None` で
パネル矩形へ描くので、常に `visible == draw_rect` になり挙動は変わらないはず。
**変わらないことを確認して報告すること。**

## 4. テスト

1. **純関数**: `(draw_rect, viewport)` → `(callback_rect, uv 窓)` を返す関数を切り出し、
   次を固定する。
   - `draw_rect` が完全に内側 → callback rect は `draw_rect`、uv 窓は `(0,0)-(1,1)`
   - ズームで四辺とも外へ出る → callback rect はビューポート、uv 窓は内側の部分区間
   - 左へパンして左半分が外 → uv 窓の `min.x` だけが増える
   - 交差なし → `None`
2. **継ぎ目の一致**: 上の uv 窓から逆算した継ぎ目の画面 x が、
   `compare_wipe_screen_x(draw_rect, fraction)` と一致すること (等倍 / ズーム / パンの 3 条件)。
   これが今回の症状そのものの回帰ガードになる。
3. `uniform_bytes` の既存テストを uv 窓込みへ更新する。

## 5. 完了条件

- `cargo fmt` (引数なし)
- `cargo test -p mimageviewer --lib` が全件 / `cargo test -p mimageviewer --test ui_snapshot`
- `cargo check -p mimageviewer --bin mimageviewer-core`
- `python scripts/check_ui_glyphs.py` が 0 件
- 非 Windows を壊さないこと
- **バックログ §1.60 の実装記録へ、この残件と原因を追記**する

## 6. 制約

- **アプリを起動しないこと。** 検証ビルドと実機依頼は ClaudeCode が行う
- **ブランチ操作・コミットをしないこと。** master の作業ツリーで未コミットのまま残す

---

完了したら次を報告すること:

1. uv 窓の導出と、継ぎ目が `draw_rect` 基準のままである根拠
2. ナビゲータ経路が変わらないことの確認結果
3. テスト結果
4. **実機で確認してほしいこと**
