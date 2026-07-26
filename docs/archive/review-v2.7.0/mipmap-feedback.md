# v2.7.0 ミップマップセッション向け 第3回レビューフィードバック

作成: 2026-07-22
比較基準: `v2.6.0` (`0d504f6d`) .. `61218736`
前回確認コミット: `20627fb4`

## 結論

**追加のコード指摘はありません。** 前回レビュー基準 `f8b8ff78` 以降、ミップマップ、比較表示、
パノラマ、fullscreen、vendor/license/install の対象ファイルに変更はありません。前回確認した修正は
現在の HEAD でも維持されています。

## 維持を確認した内容

- 比較表示は GPU 側の full mip-chain pair を現在の1組だけ保持し、新規確保前に旧組を drop する。
- pin indicator は縮小済み専用 texture を使い、原寸 texture を重複 upload しない。
- 360度パノラマは seam をまたぐ U 勾配を wrap した `textureSampleGrad` を使う。
- crop と full 360 の sampler 方針を分け、crop の反対端混入を避ける。
- vendored `egui-wgpu` の MIT/Apache-2.0 license は installer/portable/About/README から確認できる。

## 自動検証結果

- `cargo test --lib panorama_shader_uses_wrapped_explicit_gradients`: 1 passed
- `cargo test -p mimageviewer --bin mimageviewer-core mipmapped_texture_texel_estimate`: 1 passed
- `cargo fmt --all -- --check`: passed
- `cargo test --workspace`: passed（失敗0）

## 出荷前に残る portable smoke

通常の `%APPDATA%` を使う実行ファイルは起動せず、`scripts/prepare-portable-smoke.ps1` で作った
`target/portable-smoke/mimageviewer.exe` だけを使用してください。

1. 4K～8K画像で Wipe/Diff/pin変更を繰り返し、VRAMが履歴数に比例して増え続けない。
2. 高周波模様を通常 fullscreen/Wipe/Diff で縮小し、モアレ低減を確認する。
3. full 360画像を seam 正面でズーム/yawし、seamだけがぼけない。
4. 水平/垂直 crop で反対端の色が混ざらない。
5. portable directory と About 画面で egui の両ライセンスを確認する。

## 完了条件

上記 smoke が成功すれば、このセッションは「コード変更なし・検収完了」で構いません。失敗時だけ、
再現画像寸法、操作列、比較 mode、VRAM の開始/最大/終了値、GPU、perf log を添えて返してください。
