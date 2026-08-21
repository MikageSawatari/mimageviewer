# Z 照準のカーソル写像を、実際に描いている画像領域へ合わせる

正本は [next-release-backlog.md](../next-release-backlog.md) **§1.107**。
**原因特定・方針決定済み**なので、まず同項を読むこと。

## 1. ずれの正体

| | 関数 | 基準 |
| --- | --- | --- |
| カーソル → 画像座標の**写像** | `z_cursor_image_px` ([displayed_image_transform.rs:801](../../src/displayed_image_transform.rs:801)) | **`pan_band` (画面側の帯)**。画像の縦横比を見ていない |
| 照準枠の**描画** | `z_aim_frame_rect` ([displayed_image_transform.rs:883](../../src/displayed_image_transform.rs:883)) | **`content_rect`** = `view_rect` に縦横比を保って収めた領域 |

**写像と描画で基準が違う**ので、余白が大きい方向ほどカーソルと枠が離れる。
利用者報告の 3 パターン (余白なし / 縦長 / パノラマ) はこの差でそのまま説明できる。

## 2. 決定 (§1.107、2026-08-20)

**写像の基準を `content_rect ∩ pan_band` にする。**

- `content_rect` は `z_aim_frame_rect` が既に計算している
  (`fit = min(view/content)` → `Rect::from_center_size(view_rect.center(), content_size * fit)`)。
  **共通 helper へ出して同一の値を共有する。** 別々に計算するとまた乖離する。
- `pan_band` との交差を取るのは、**上下の HUD ホバー帯へカーソルが入る前に画像の上端・下端へ
  到達できるようにするため** (実機 FB 2026-06-21 で入れた既存の意図)。**これは維持する。**
- **縮退**: 交差が空、または極端に細い場合 (小さいウィンドウ + 大きい HUD 余白) は
  **従来どおり `pan_band` を使う**。狙えなくなる状態を作らない。

## 3. 同型の複製を片方だけ直さない

[ui_fullscreen.rs:7059](../../src/ui_fullscreen.rs:7059) の `zip_cursor_image_px`
(連結表示 / 見開き合成) は `z_cursor_image_px` の**逐語的な複製**である。
**1 つの helper に寄せる。**

描画側は既にそうなっている: `zip_aim_frame_rect` ([ui_fullscreen.rs:7144](../../src/ui_fullscreen.rs:7144))
は `displayed_image_transform::z_aim_frame_rect` へ委譲するだけの薄い関数である。
**写像側も同じ形にする** (委譲にするか、呼び出し元を直接 `z_cursor_image_px` へ向けて
`zip_cursor_image_px` を消すか。後者の方が関数が 1 つ減る)。

呼び出し元は 2 箇所で、**どちらも view rect を手元に持っている**ので引数追加は容易:

- [displayed_image_transform.rs:571](../../src/displayed_image_transform.rs:571) —
  `input.image.viewport_rect` がすぐ上にある
- [ui_fullscreen.rs:7242](../../src/ui_fullscreen.rs:7242) — `image_rect` がそれ

## 4. テスト

既存の 2 本は前提が変わるので更新する:

- `zip_cursor_image_px_maps_band_to_image_and_clamps`
  ([ui_fullscreen.rs:43710](../../src/ui_fullscreen.rs:43710))
- `zip_pan_band_reaches_image_edge_before_top_hover_zone`

後者が守っている**「上部ホバー帯へ入る前に画像上端へ届く」性質は新しい写像でも維持する**ことを
テストで明示すること。これは実機 FB で入れた意図なので落とさない。

追加で固定する:

- **縦長 (左右に余白)**: 画面左端付近ではなく、**描いている画像の左端**でカーソルが画像の
  左端に対応すること
- **パノラマ (上下に余白)**: 交差が `content_rect` そのものになり、上下が画像基準になること
- **余白なし**: 従来とほぼ変わらないこと
- **縮退**: 交差が空 / 極細のとき `pan_band` にフォールバックし、画像全体を狙えること
- **写像と描画が同じ基準を共有していること** (helper が 1 つであることを型で担保できるなら
  なお良い)

## 5. 範囲外

- カーソル非表示 (Z を押している間は隠す) は**独立**。今回は非表示のままでよい。
  写像を直した後に出す方が自然かどうかは実機で見てから判断する。
- トリム表示中 (`content_bbox` あり) と回転ページでも枠と写像が一致することは**確認する**が、
  そのための新しい分岐は足さない (同じ helper を通れば自然に揃うはず)。

## 5.5 実機確認用のテスト画像

`C:\tmp\miv-zaim-test-20260820\` に 7 枚生成済み (2026-08-20)。

| ファイル | 寸法 | 比 | 用途 |
| --- | --- | ---: | --- |
| `01-fit-16x9.png` | 3840x2160 | 1.78 | **余白なし**。修正前後で変わらないことの確認 (退行検出) |
| `02-portrait-1x3.png` | 1200x3600 | 0.33 | 縦長 |
| `03-portrait-1x8.png` | 800x6400 | 0.12 | 縦長 (強い) |
| `04-portrait-1x16.png` | 400x6400 | 0.06 | 縦長 (極端) |
| `05-panorama-3x1.png` | 3600x1200 | 3.00 | パノラマ |
| `06-panorama-8x1.png` | 6400x800 | 8.00 | パノラマ (強い) |
| `07-panorama-16x1.png` | 6400x400 | 16.0 | パノラマ (極端) |

各画像の中身: **赤い外枠** (画像の本当の端) / **四隅の色ブロック** (到達した隅の識別) /
**各辺中点の赤い突起** / **5% グリッド + 中心十字** / **上端・左端の % ラベル**。

判定: **カーソルを赤い外枠の左端に置いたとき、照準枠が画像の左端 (0%) に来るか。**
現状は縦長でウィンドウ端まで運ぶ必要があり、パノラマでは同じことが上下で起きる。
% ラベルにより、ずれを「左端に置いたのに 20% を狙っている」と数値で報告できる。

生成スクリプトは使い捨て (PIL)。再生成が要るなら同じ要件で作り直す。

## 6. 完了条件

- `cargo fmt` 済み / `cargo test -p mimageviewer --lib` が緑
- `cargo check -p mimageviewer --bin mimageviewer-core` が通る
- **`z_cursor_image_px` と `zip_cursor_image_px` が 1 つになっていること**
