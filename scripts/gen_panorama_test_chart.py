#!/usr/bin/env python3
"""360 度ビューの投影方式を目視で検証するための equirectangular テストチャート生成。

docs/panorama-360-view-plan.md §13 の 4 方式 (透視 / 立体射影 / 等距離 / 等立体角) は、
「見え方が変わった」ことは分かっても「正しく変わったか」は普通の風景写真では判定できない。
各方式が持つ幾何的な不変条件を、画像側にあらかじめ埋め込んでおくためのチャートを作る。

出力 (既定 4096x2048、mIV の 2:1 判定と MAX_TEXTURE_DIM に収まる):

- graticule.png       経緯線グリッド。**直線性** (透視) と **等角性** (立体射影) を見る
- equal-solid-angle.png  1 マスの立体角が一定の市松。**等面積性** (等立体角) を見る
- equidistant-rings.png  極を中心とする等角度リング。**半径の等間隔性** (等距離) を見る

使い方:
    python scripts/gen_panorama_test_chart.py [出力ディレクトリ] [--size 4096]

判定方法は docs/panorama-360-view-plan.md §13.7 を参照。
"""

from __future__ import annotations

import argparse
import math
import os
import sys

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:  # pragma: no cover - 環境依存
    sys.exit("Pillow が必要です: python -m pip install Pillow")


# 経度 30 度ごとに色相を変えて、どの向きを見ているかを分かるようにする。
OCTANT_COLORS = [
    (198, 64, 64),
    (198, 128, 48),
    (176, 176, 48),
    (96, 176, 64),
    (48, 168, 152),
    (56, 128, 200),
    (104, 88, 200),
    (168, 72, 160),
    (150, 90, 70),
    (110, 110, 110),
    (70, 130, 110),
    (140, 70, 110),
]


def _lat_to_y(lat_deg: float, height: int) -> float:
    """緯度 (度、北が正) を equirect の y 座標へ。"""
    return (0.5 - lat_deg / 180.0) * height


def _lon_to_x(lon_deg: float, width: int) -> float:
    """経度 (度) を equirect の x 座標へ。"""
    return (lon_deg / 360.0 + 0.5) * width


def _load_font(px: int) -> ImageFont.ImageFont:
    for name in ("arial.ttf", "DejaVuSans.ttf", "seguisb.ttf"):
        try:
            return ImageFont.truetype(name, px)
        except OSError:
            continue
    return ImageFont.load_default()


def graticule(width: int, height: int) -> Image.Image:
    """経緯線グリッド。

    - 経線 (子午線) と赤道は **大円**。透視投影では大円は必ず直線に写る。
    - 交点は必ず直角。立体射影は等角写像なので、周辺でも直角のまま。
    """
    img = Image.new("RGB", (width, height), (24, 24, 28))
    draw = ImageDraw.Draw(img)

    # 経度 30 度ごとの帯で向きを色分けする (淡く塗って線を見やすく保つ)。
    for i in range(12):
        x0 = _lon_to_x(-180 + i * 30, width)
        x1 = _lon_to_x(-180 + (i + 1) * 30, width)
        r, g, b = OCTANT_COLORS[i]
        draw.rectangle([x0, 0, x1, height], fill=(r // 3, g // 3, b // 3))

    minor = (150, 150, 158)
    major = (245, 245, 250)
    equator = (255, 96, 96)
    prime = (96, 200, 255)

    thin = max(1, width // 2048)
    thick = max(2, width // 700)

    # 緯線: 10 度ごと。30 度ごとを太く。
    for lat in range(-80, 81, 10):
        y = _lat_to_y(lat, height)
        if lat == 0:
            draw.line([(0, y), (width, y)], fill=equator, width=thick)
        else:
            w = thick if lat % 30 == 0 else thin
            draw.line([(0, y), (width, y)], fill=major if lat % 30 == 0 else minor, width=w)

    # 経線: 10 度ごと。30 度ごとを太く。本初子午線は青。
    for lon in range(-180, 181, 10):
        x = _lon_to_x(lon, width)
        if lon == 0:
            draw.line([(x, 0), (x, height)], fill=prime, width=thick)
        else:
            w = thick if lon % 30 == 0 else thin
            draw.line([(x, 0), (x, height)], fill=major if lon % 30 == 0 else minor, width=w)

    # 極の目印 (天頂 / 天底)。極付近は equirect で極端に引き伸ばされるので帯で示す。
    draw.rectangle([0, 0, width, height * 0.01], fill=(255, 220, 90))
    draw.rectangle([0, height * 0.99, width, height], fill=(120, 220, 255))

    font = _load_font(max(14, height // 40))
    for lon in range(-180, 180, 30):
        x = _lon_to_x(lon + 15, width)
        draw.text((x, _lat_to_y(0, height) + height * 0.012), f"{lon + 15:+d}",
                  fill=(255, 255, 255), font=font, anchor="ma")
    for lat in range(-60, 61, 30):
        if lat == 0:
            continue
        draw.text((_lon_to_x(15, width), _lat_to_y(lat, height)), f"{lat:+d}",
                  fill=(255, 255, 255), font=font, anchor="lm")
    draw.text((_lon_to_x(0, width), _lat_to_y(0, height) - height * 0.02),
              "FRONT (lon 0)", fill=(150, 230, 255), font=font, anchor="md")
    return img


def equal_solid_angle(width: int, height: int, cells_lon: int = 24, cells_lat: int = 12) -> Image.Image:
    """1 マスの立体角が一定になる市松。

    立体角要素は `dOmega = dlon * d(sin lat)` なので、**経度を等分し、sin(緯度) を等分**
    すればすべてのマスが同じ立体角になる。equirect 画像の上では、極へ近づくほど
    マスが縦に伸びる (緯度の刻みが粗くなる) のが正しい姿。
    """
    img = Image.new("RGB", (width, height), (20, 20, 24))
    draw = ImageDraw.Draw(img)
    lat_edges = [math.degrees(math.asin(-1.0 + 2.0 * i / cells_lat)) for i in range(cells_lat + 1)]
    for j in range(cells_lat):
        y0 = _lat_to_y(lat_edges[j + 1], height)
        y1 = _lat_to_y(lat_edges[j], height)
        for i in range(cells_lon):
            x0 = _lon_to_x(-180 + 360.0 * i / cells_lon, width)
            x1 = _lon_to_x(-180 + 360.0 * (i + 1) / cells_lon, width)
            if (i + j) % 2 == 0:
                shade = 235
            else:
                shade = 45
            draw.rectangle([x0, y0, x1, y1], fill=(shade, shade, shade))
    # 赤道と本初子午線だけ色を残して向きを分かるようにする。
    thick = max(2, width // 700)
    draw.line([(0, _lat_to_y(0, height)), (width, _lat_to_y(0, height))],
              fill=(255, 96, 96), width=thick)
    draw.line([(_lon_to_x(0, width), 0), (_lon_to_x(0, width), height)],
              fill=(96, 200, 255), width=thick)
    return img


def equidistant_rings(width: int, height: int, step_deg: int = 10) -> Image.Image:
    """天頂 (北極) からの角距離が等間隔になる同心リング。

    等距離射影で天頂を正面に置くと、この輪の**半径が等間隔**になる。
    立体射影では外側ほど広がり、等立体角では外側ほど詰まる。
    """
    img = Image.new("RGB", (width, height), (24, 24, 28))
    draw = ImageDraw.Draw(img)
    thick = max(2, width // 700)
    for k, lat in enumerate(range(90 - step_deg, -91, -step_deg)):
        y = _lat_to_y(lat, height)
        # 30 度ごとに強調して、数えやすくする。
        polar_angle = 90 - lat
        if polar_angle % 30 == 0:
            color = (255, 200, 80)
            w = thick * 2
        else:
            color = (220, 220, 228)
            w = thick
        draw.line([(0, y), (width, y)], fill=color, width=w)
    # 経線は方位の手がかりとして薄く残す。
    for lon in range(-180, 181, 15):
        x = _lon_to_x(lon, width)
        draw.line([(x, 0), (x, height)], fill=(90, 90, 100), width=max(1, width // 2048))
    draw.rectangle([0, 0, width, height * 0.012], fill=(255, 220, 90))
    font = _load_font(max(14, height // 40))
    draw.text((_lon_to_x(0, width), height * 0.03), "ZENITH (look up)",
              fill=(255, 220, 90), font=font, anchor="ma")
    return img


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("out_dir", nargs="?", default="dist/panorama-test-charts")
    parser.add_argument("--size", type=int, default=4096,
                        help="出力幅 (高さはその半分。既定 4096)")
    args = parser.parse_args()

    width = args.size
    height = width // 2
    os.makedirs(args.out_dir, exist_ok=True)

    charts = {
        "graticule.png": graticule(width, height),
        "equal-solid-angle.png": equal_solid_angle(width, height),
        "equidistant-rings.png": equidistant_rings(width, height),
    }
    for name, img in charts.items():
        path = os.path.join(args.out_dir, name)
        img.save(path)
        print(f"wrote {path} ({width}x{height})")
    print("\n判定方法: docs/panorama-360-view-plan.md §13.7")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
