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

[ui_fullscreen.rs:7057](../../src/ui_fullscreen.rs:7057) の `zip_cursor_image_px`
(連結表示 / 見開き合成) は `z_cursor_image_px` の**逐語的な複製**である。
**1 つの helper に寄せる。**

呼び出し元は 2 箇所で、**どちらも view rect を手元に持っている**ので引数追加は容易:

- [displayed_image_transform.rs:571](../../src/displayed_image_transform.rs:571) —
  `input.image.viewport_rect` がすぐ上にある
- [ui_fullscreen.rs:7240](../../src/ui_fullscreen.rs:7240) — `image_rect` がそれ

## 4. テスト

既存の 2 本は前提が変わるので更新する:

- `zip_cursor_image_px_maps_band_to_image_and_clamps`
  ([ui_fullscreen.rs:43166](../../src/ui_fullscreen.rs:43166))
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

## 6. 完了条件

- `cargo fmt` 済み / `cargo test -p mimageviewer --lib` が緑
- `cargo check -p mimageviewer --bin mimageviewer-core` が通る
- **`z_cursor_image_px` と `zip_cursor_image_px` が 1 つになっていること**
