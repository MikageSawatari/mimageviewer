# -*- coding: utf-8 -*-
"""X カルーセル計測用テスト画像ジェネレータ。

各出力画像は 1536x2048 (= 3:4)。N 枚を横に並べた合成キャンバス上の x 座標を
色に埋め込むので、投稿後にスクリーンショットの任意の画素から「元画像の
どこか」を 1px 精度で復元できる。X が縮小しても連続階調なので壊れない。

  R = round(255 * x / (W-1))   粗 (単調。周期の曖昧さを消す)
  G = x mod 256                密 (256px 周期。1px 分解能)
  B = 96                       固定 (X の背景色から離した識別用の目印)

復元:  coarse = R/255*(W-1);  x = round((coarse - G)/256)*256 + G
"""

import os
from PIL import Image, ImageDraw, ImageFont

TILE_W, TILE_H = 1536, 2048  # 3:4
TOP_BAND = 280
BOT_BAND = 280
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "out")

FONT = ImageFont.truetype("C:/Windows/Fonts/arial.ttf", 22)
FONT_BIG = ImageFont.truetype("C:/Windows/Fonts/arialbd.ttf", 96)


def encode_row(w):
    row = bytearray()
    last = max(w - 1, 1)
    for x in range(w):
        row += bytes((round(255 * x / last), x % 256, 96))
    return bytes(row)


def build_combined(n):
    w = TILE_W * n
    img = Image.new("RGB", (w, TILE_H), (255, 255, 255))

    band_h = TILE_H - TOP_BAND - BOT_BAND
    band = Image.frombytes("RGB", (w, band_h), encode_row(w) * band_h)
    img.paste(band, (0, TOP_BAND))

    d = ImageDraw.Draw(img)
    for x in range(0, w + 1, 10):
        if x % 100 == 0:
            ln, col = 40, (0, 0, 0)
        elif x % 50 == 0:
            ln, col = 26, (110, 110, 110)
        else:
            ln, col = 14, (170, 170, 170)
        d.line([(x, 0), (x, ln)], fill=col)
        d.line([(x, TILE_H - 1), (x, TILE_H - 1 - ln)], fill=col)
        if x % 200 == 0 and x < w:
            d.text((x + 4, 46), str(x), font=FONT, fill=(0, 0, 0))
            d.text((x + 4, TILE_H - 74), str(x), font=FONT, fill=(0, 0, 0))

    # 継ぎ目 (= 出力画像の境界) をルーラー帯にだけマゼンタで示す
    for k in range(n + 1):
        x = min(k * TILE_W, w - 3)
        d.rectangle([x, 0, x + 2, TOP_BAND - 1], fill=(255, 0, 255))
        d.rectangle([x, TILE_H - TOP_BAND, x + 2, TILE_H - 1], fill=(255, 0, 255))

    for k in range(n):
        cx = k * TILE_W + TILE_W // 2
        label = f"{k + 1} / {n}"
        bbox = d.textbbox((0, 0), label, font=FONT_BIG)
        d.text((cx - (bbox[2] - bbox[0]) // 2, TILE_H - 210), label,
               font=FONT_BIG, fill=(0, 0, 0))
    return img


def main():
    os.makedirs(OUT, exist_ok=True)
    for n in (2, 3, 4):
        combined = build_combined(n)
        combined.save(os.path.join(OUT, f"ref_x{n}_combined.png"))
        for k in range(n):
            tile = combined.crop((k * TILE_W, 0, (k + 1) * TILE_W, TILE_H))
            d = ImageDraw.Draw(tile)
            # 上下だけ 3px マゼンタ線。縦の切り取りが起きたら消える。
            # 左右に引くと継ぎ目の計測で隙間へ混ざるので引かない
            # (横方向の切り取りは色帯の端の復号値で分かる)
            d.rectangle([0, 0, TILE_W - 1, 2], fill=(255, 0, 255))
            d.rectangle([0, TILE_H - 3, TILE_W - 1, TILE_H - 1], fill=(255, 0, 255))
            path = os.path.join(OUT, f"x{n}_{k + 1}.png")
            tile.save(path)
            print(path, tile.size, os.path.getsize(path) // 1024, "KB")


if __name__ == "__main__":
    main()
