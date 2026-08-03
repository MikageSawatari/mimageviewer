#!/usr/bin/env python3
"""FAQ の「縮小モアレ」説明用サンプル画像を生成する。

出力 (htdocs/mimageviewer/manual/images/):
  moire-source.webp      - 原寸 (100%) の一部。スクリーントーンと細線がどう並んでいるか
  moire-aliased.webp     - 上を 1/3 へ縮小 (点サンプリング相当)。モアレが出た状態
  moire-suppressed.webp  - 上を 1/3 へ縮小 (面積平均相当)。モアレが消えた状態

3 枚とも同じ表示サイズで並べられるよう、縮小後の 2 枚は同じ画素数にする。
モアレそのものは低周波なので、ブラウザ側で多少拡大されても消えない。

これは現象の説明用の合成画像であり、mImageViewer の画面キャプチャではない。
再生成: python scripts/gen-moire-sample.py
"""

import math
import pathlib

from PIL import Image, ImageDraw

OUT_DIR = pathlib.Path(__file__).resolve().parent.parent / "htdocs" / "mimageviewer" / "manual" / "images"

# 原寸。縮小後がちょうど 1/3 になる寸法にする。
SRC_W, SRC_H = 1200, 810
DST_W, DST_H = SRC_W // 3, SRC_H // 3

# スクリーントーンの周期 (px)。3 で割ると割り切れない値にすると干渉が出やすい。
TONE_PERIOD = 5.0


def build_source() -> Image.Image:
    """スクリーントーン + 線画からなる合成原画。

    上半分をトーン (モアレが出る部分)、下半分を線画 (縮小で柔らかくなる部分) に
    分けて、どちらの現象も 1 枚で見えるようにする。
    """
    img = Image.new("L", (SRC_W, SRC_H), 255)
    px = img.load()
    tone_bottom = int(SRC_H * 0.60)

    # 網点: 左から右へゆるやかに濃度が上がるグラデーショントーン。
    # 濃度をゆるやかにするほど、点サンプリングでの干渉が「広い縞」として見える。
    for y in range(tone_bottom):
        for x in range(SRC_W):
            density = 0.34 + 0.30 * (x / SRC_W)
            cx = (x % TONE_PERIOD) - TONE_PERIOD / 2.0
            cy = (y % TONE_PERIOD) - TONE_PERIOD / 2.0
            if math.hypot(cx, cy) <= TONE_PERIOD * 0.5 * density * 1.7:
                px[x, y] = 0

    draw = ImageDraw.Draw(img)

    # トーンの上に重なる線画 (輪郭)。トーンと線が同居する漫画のページに近づける。
    draw.ellipse(
        [SRC_W * 0.06, SRC_H * 0.06, SRC_W * 0.46, SRC_H * 0.52], outline=0, width=4
    )
    draw.arc([SRC_W * 0.52, SRC_H * 0.04, SRC_W * 0.96, SRC_H * 0.50], 200, 340, fill=0, width=4)

    # 下半分は白地の線画。太さ違いの線を並べ、縮小でどこから潰れるかを見る。
    base_y = tone_bottom + 40
    for i, w in enumerate((5, 4, 3, 2, 1, 1, 1)):
        y = base_y + i * 26
        draw.line([(70, y), (SRC_W - 70, y)], fill=0, width=w)

    # 細い縦線の束 (1px)。線の密度が高い部分の縮小結果を見る。
    for x in range(int(SRC_W * 0.62), SRC_W - 70, 5):
        draw.line([(x, base_y - 18), (x, SRC_H - 30)], fill=0, width=1)

    return img.convert("RGB")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    src = build_source()

    # 原寸の一部を、縮小画像と同じ画素数だけ切り出す (等倍で何が写っているか)。
    crop_box = (0, 0, DST_W, DST_H)
    src.crop(crop_box).save(OUT_DIR / "moire-source.webp", quality=92, method=6)

    # 点サンプリング相当: 縮小率ぶんの画素を捨てるので、細かい周期が干渉して縞になる。
    src.resize((DST_W, DST_H), Image.NEAREST).save(
        OUT_DIR / "moire-aliased.webp", quality=92, method=6
    )

    # 十分に広いフィルタを通してから縮小した状態。捨てる画素も含めて均すので
    # 縞は出ないが、細部は柔らかくなる。
    #
    # 箱平均 (BOX) は 1/3 縮小だとトーンの周期 5px と割り切れず残留パターンが出るため、
    # 説明図としては支持幅の広い LANCZOS を使う。GPU の mipmap も段階的に平均を重ねる
    # ぶん実効の支持幅が広く、見え方はこちらに近い。
    src.resize((DST_W, DST_H), Image.LANCZOS).save(
        OUT_DIR / "moire-suppressed.webp", quality=92, method=6
    )

    for name in ("moire-source", "moire-aliased", "moire-suppressed"):
        p = OUT_DIR / f"{name}.webp"
        print(f"wrote {p} ({p.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
