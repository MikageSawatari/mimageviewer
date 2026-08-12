# 保存済みトリムの基準を、ページの正準ラスタに固定する

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。`6cf7d843` の続き
(あの修正は基準の取り方が**まだ違っていた**)。

## 0. 前提 — 先に読むもの

- `src/remote_ipc/container.rs` — `StoredEditSpace` (195 行付近) と 2 つの利用箇所
- `src/pdf_loader.rs` — `canonical_pdf_raster_long_edge` (2530 行付近)、
  `render_page_canonical_raster_with` (2863 行付近、`native_dims` を返す)
- `src/export_crop.rs` — `CropSettings` (37 行付近)。**矩形しか持たず、基準寸法を持たない**
- `src/app.rs` — `export_crop_for_idx` (51774 行付近) と
  `export_crop_rect_for_pixels` (51790 行付近) のコメント「crop は source 座標で保存されている」

## 1. 事実 (実機で 2 回確認済み。再調査不要)

同じ PDF の 1 ページ目で、基準を変えるたびに違う壊れ方をした。

| 基準にした値 | 結果 |
|---|---|
| カタログの `source_*` (1/1000 ポイント) | 矩形が 1/140 に縮み、**27×44 ピクセル**を全画面へ拡大 = 青一色 |
| **remote が今描いたラスタ** (`6cf7d843`) | 等倍で当たるため、2894px 幅で作られた矩形を 5676px 幅へそのまま適用 = **左上だけが拡大** |

`CropSettings` は矩形しか保存しておらず、**どの寸法を基準に作られたかを持っていない**。通常画像は
「source」= ファイルの実ピクセルで安定しているが、**PDF ページは描画解像度で source の大きさが
変わる**ため、要求ごとに基準が変わってしまう。

`canonical_pdf_raster_long_edge` はページ固有の値 (native 長辺、8192 上限) で、要求解像度に
依存しない。`render_page_canonical_raster_with` はその寸法で描いた `native_dims` を返す。

## 2. 決定

**PDF ページの保存済み編集の基準は、そのページの正準ラスタ寸法とする。**
remote が今回何ピクセルで描いたかは基準にしない。

ここから守るべき不変条件が 1 つ出る。**これをテストの中心に据えること。**

> **同じページの保存済みトリムは、要求した解像度が変わっても同じ範囲を切り出す。**

今回の 2 つの壊れ方は、どちらもこの不変条件に違反していた。1 つ目は基準が桁違い、2 つ目は
基準が要求解像度と一緒に動いていた。**この不変条件のテストがあれば両方とも落ちる。**

## 3. 変更内容

- `StoredEditSpace::for_remote_source` の `RemoteSubresource::PdfPage` 分岐を、
  **ページの正準ラスタ寸法**から作る。`canonical_pdf_raster_long_edge` /
  `render_page_canonical_raster_with` の `native_dims` と同じ値になること
- 正準寸法が取れない場合 (vector ページなど) の扱いを決め、コメントに理由を書く。
  **黙って今の描画寸法へ落とさない** — それが今回の 2 つ目の壊れ方だった
- 通常画像の基準は変えない (ファイルの実ピクセル)
- `StoredEditSpace` の doc comment に「基準は**ページ固有で、要求解像度に依存しない**値でなければ
  ならない」と明記する。型があっても、入れる値を間違えれば同じことが起きると分かる形にする

## 4. 触らないもの

- `src/catalog.rs` の `source_*` の意味 (本体側で別途扱う。**別セッションが作業中**)
- 本体の表示経路 (`src/app.rs` / `src/ui_fullscreen.rs`)
- `CropSettings` の保存形式。基準寸法を持たせる案は本体側の判断が要るので、ここでは変えない
- 先読み・表示所有権・protocol

## 5. テスト

```
cp vendor/ffmpeg/bin/*.dll target/debug/deps/
cargo test -p mimageviewer --lib remote_ipc::
cargo test -p mimageviewer-remote
cargo fmt --all -- --check
```

**§2 の不変条件を直接テストすること。**

- 同じ PDF ページに同じ保存済みトリムがあるとき、**target_px を 1024 / 4096 / 8192 と変えても、
  切り出される範囲 (元ページに対する割合) が一致する**
- カタログの寸法が実ラスタと桁違いでも結果が変わらない (`6cf7d843` のテストを維持)
- 通常画像では従来と同じ結果になる

既存の `pdf_saved_crop_uses_rendered_raster_not_catalog_layout_dimensions` は、名前が
「描いたラスタを使う」になっていて**今回の修正と矛盾する**。不変条件を表す名前へ変え、
中身も §2 に合わせること。

## 6. ドキュメント

- plan **§14.18** を更新する。2 つ目の壊れ方 (左上拡大) と、基準は「ページ固有で要求解像度に
  依存しない値」でなければならないこと、`CropSettings` が基準寸法を持たない事実を追記する
- [`docs/virtual-folders.md`](../virtual-folders.md) の記述も揃える

## 7. 実行と報告

- §5 のコマンドを**毎回実行**して結果を報告する
- **`src/` と `crates/` に触れた箇所を全部、理由付きで報告する**
- **`scripts/build-dev.ps1` を実行しない。コミットもしない**
- ブリーフと意図的に違えた点があれば、その理由を報告する
