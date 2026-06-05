# comic_lab 吹き出し形状の拡充（FramePlanner 参考）

Status: Phase1 + Phase2 + Phase3 実装済み（2026-06）。
対象: `crates/comic-core`（形状・ラスタ） + `tools/comic_lab`（UI）。

## 背景・分析

参考: **FramePlanner**（[jonigata/FramePlanner2](https://github.com/jonigata/FramePlanner2)、
**MIT**、Svelte + HTML canvas + paper.js）。その吹き出しピッカーは内部的に 1 つの
`drawBubble` ディスパッチで、**ベクター形状**と**線群エフェクト**の 2 系統を持つ。
本ツールには同等の形状を**自前のテッセレーション/ラスタで再実装**した（コードの移植では
なく、形状仕様の再現。MIT なので参考自体も問題なし）。

FramePlanner 内部名 → 表示 → 本ツール対応:

| FramePlanner | 表示 | 本ツール |
|---|---|---|
| square / rounded / ellipse / harsh / thought | 四角形/角丸/楕円/トゲトゲ/思考 | 既存 (RoundRect/Ellipse/Burst/Cloud) |
| **soft** | やわらか | Phase1: `BubbleShape::Soft`（角丸+波打ち） |
| **polygon** | 多角形 | Phase1: `Polygon` |
| **diamond** | ダイヤ | Phase1: `Diamond` |
| **heart** | ハート | Phase1: `Heart` |
| **arrow** | 矢印 | Phase1: `Arrow`（向き可変） |
| shout | シャウト | 既存 Burst プリセット「叫び」で代替 |
| **motion-lines** | 集中線 | Phase2: `MotionLines`（線群） |
| **speed-lines** | 流線 | Phase2: `SpeedLines`（線群） |
| **concentration** | 意識 | Phase3: `Concentration`（ぼかし縁の楕円） |
| **strokes** | 線 | Phase3: `Strokes`（手描き風の多重ストローク） |
| **double-strokes** | 二重線 | Phase3: `DoubleStroke`（二重 stroke） |
| ***-mind** | ◯？（思考） | Phase3: 既存形状 + `TailKind::Thought`（プリセット「思考(楕円)」） |
| **none** | なし | Phase3: `TextOnly`（テキストのみ・枠なし） |

## Phase1: ベクター形状（既存パイプラインに乗る）

`tessellate_bubble` が輪郭点を返すだけで、塗り/枠/しっぽ/本文/自動サイズ/選択ハンドル/
hit-test は既存機構がそのまま効く。追加した変種と幾何:

- **Polygon** `{rx,ry,sides}`: rx/ry 楕円に内接する正 N 角形（頂点上向き）。
- **Diamond** `{half_w,half_h}`: 4 軸点の菱形。
- **Heart** `{rx,ry}`: 16sin³t ハート曲線を rx/ry にフィット（y 反転で画面座標）。
- **Arrow** `{half_w,half_h,dir_rad}`: シャフト+ヘッドの 7 点矢印を `dir_rad` 方向へ回転。
- **Soft** `{half_w,half_h,corner_px,shape_seed}`: `round_rect_dense`（辺も分割した角丸長方形）の
  周を低振幅 sine で膨らませた柔らか吹き出し。

`fit_bubble_shape`（自動サイズ）は各形状の内接率に合わせた係数で外接を拡大し、本文が収まる
ようにする。`shape_half_extents` / raster の `short_side` にも arm を追加。

lab: `BubblePreset` に やわらか/多角形/ダイヤ/ハート/矢印 を追加（ピッカー・プリセット行・
右パネルのパラメータスライダ=辺の数 / 向き(度) / 角丸 等）。corner 一様スケール・hit-test 対応。

## Phase2: 線群エフェクト（新レンダリング）

集中線/流線は塗り多角形ではなく**多数の線**で、中央に本文用の**クリア楕円**を残す。

- データ: `MotionLines {rx,ry,count,shape_seed}` / `SpeedLines {half_w,half_h,dir_rad,count,shape_seed}`。
  rx/ry（half_w/half_h）は**外接**。`tessellate_bubble` は外接楕円を返す（= AABB / hit-test /
  自動サイズの土台）。クリア中心は外接の `LINE_FIELD_CLEAR_RATIO`（=0.55）。
- 自動サイズ: 本文はクリア中心に収める → 外接 = 文字外接 × √2 / 0.55。
- ラスタ: `draw_bubble_parts` で線群形状を検出したら fill/stroke をスキップして
  `draw_line_field` を呼ぶ（本文は通常どおり中央に焼く）。
  - **集中線**: クリア楕円→外接へ放射する**先細りの三角ストローク**を `count` 本（角度は均等
    +シード由来の小ジッタ、外側で太く中心へ向かって点に）。
  - **流線**: `dir_rad` 方向の平行線を `count` 本。クリア楕円を横切る中央区間は飛ばして両端
    の 2 セグメントを描く（楕円交点で gap を計算）。
  - 線の色/太さは `bubble.outline`。しっぽは無し（`tail_kind` で None）。
- 回帰テスト: `line_field_shapes_bake_pixels_with_clear_center`（線が描かれ、中心ピクセルは透明）。

## Phase3: ぼかし縁 / 手描き / テキストのみ（実装済み）

既存のベクター / 線群パイプラインに、塗り多角形でも線群でもない描画モードを 4 つ追加。

- **意識** `Concentration {rx,ry,shape_seed}`: ぼかし縁の楕円。`draw_bubble_parts` が
  検出して `draw_concentration` を呼ぶ（per-pixel）。内側 `CONCENTRATION_SOLID_RATIO`
  (=0.78) までは不透明、そこから縁に向かって羽化（feather）。縁の半径は seed 由来の
  低周波 sine 2 波で揺らして「もやもや」感を出す。縁付近にソフトな ring（枠色）を重ねる。
  しっぽ無し。自動サイズは文字外接を内側 0.78 楕円に収める（外接 = 文字外接 × √2 / 0.78）。
- **線** `Strokes {half_w,half_h,corner_px,shape_seed}`: 角丸長方形の輪郭を、各頂点を
  外向き法線に沿って seed 由来の滑らかな波でずらした多重 stroke（2 pass、位相違い）で
  描く＝手描き風。塗り・本文・自動サイズ・しっぽ・hit-test は通常パイプラインのまま。
  滑らかな摂動なので自己交差しない。
- **二重線** `DoubleStroke {half_w,half_h,corner_px,gap_px}`: 角丸長方形の外側ラインに加え、
  `gap_px` 内側にもう 1 本（body のみ、しっぽは 1 本）。自動サイズは文字が内側ラインに
  収まるよう外接を gap 分広げる。
- **なし** `TextOnly {half_w,half_h}`: 枠も塗りもしっぽも描かず、中央寄せ本文のみ。
  `half_w/half_h` は移動・選択領域 + 自動サイズ用。bubble パイプラインに乗るので後から
  任意の形状へ切替できる（Text オブジェクトと違い吹き出しとして扱える）。

◯？（思考系）は新形状ではなく既存形状 + `TailKind::Thought`。プリセット「思考(楕円)」
（`MindEllipse` = Ellipse + Thought）を追加（もくもく `Cloud` との対比でクリーンな楕円）。

回帰テスト: tessellate `phase3_shapes_have_sane_outlines` / `fit_preserves_phase3_variants`、
raster `text_only_with_empty_text_draws_nothing` / `concentration_fills_center_and_fades_outside`
/ `double_stroke_draws_more_than_single` / `sketch_strokes_bake_pixels_within_box`。

## 検証
`cargo test -p comic-core`（78）/ `cargo test -p comic_lab --bin comic_lab`（4）green。
lab worktree（branch `lab`）で実装。
