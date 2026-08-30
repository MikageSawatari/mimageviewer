"""縮小表示のボケ (backlog §1.0e) を実機で判定するためのテストチャートを作る。

    python scripts/make_downscale_check_chart.py --viewport 3840x2160

出力先は既定で C:\\tmp\\miv-lanczos-check。生成物は次の 4 つ:

- lanczos-check-<W>x<H>.png -- mIV のフルスクリーン (ページフィット) で開く現物
- expected-fixed.png / expected-v3.3.0.png -- 修正前後の見え方 (オフライン再現)
- compare-*.png -- 等倍切り出しの並べ比べ

## なぜこの寸法なのか

リサンプラは floor(source * physical_scale) でテクスチャサイズを決めていた。
physical_scale は f32 なので、たとえば 1440/1600 は 0.899999976 になり、**理論上は
ちょうど収まるはずの辺が 1px 短くなる**。1439px のテクスチャを 1440px の矩形へ貼るので
GPU がもう一度バイリニアを掛け、全面がボケる。

**整数倍ではこのバグは出ない。** 倍率がちょうど 1/2 や 1/3 なら f32 に誤差が乗らず、
floor しても正しいサイズになる。判定用の画像はむしろ「理論値がぎりぎり整数を下回る」
寸法を選ぶ必要がある。--search はその寸法を総当たりで探す。

## 何を見るか

旧コードは 1439px を 1440px へ引き伸ばすので、再サンプルの位相が左端 0 -> 中央 0.5 ->
右端 1 と流れる。位相 0.5 が 2 タップ平均になって最もボケるため、**細かい縞のコントラスト
が中央で落ちる**。上下方向も同じで、画像の縦中央が最悪点。修正後は全面で均一になる。
"""

import argparse
import os

import numpy as np

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:  # pragma: no cover
    raise SystemExit("Pillow が要ります: pip install pillow")

f32 = np.float32


def emulate(src_w, src_h, view_w, view_h, ppp):
    """アプリと同じ順序・同じ型で描画サイズを再現する。

    logical_scale は論理ポイントでの min、physical_scale はそれに ppp を掛けたもの
    (gpu_lanczos::physical_scale)。旧実装は floor、現行は整数から 1e-3 以内なら丸め。
    """
    ls = f32(min(f32(view_w) / f32(src_w), f32(view_h) / f32(src_h)))
    ps = f32(ls * f32(ppp))
    result = {"scale": float(ps)}
    for axis, src in (("x", src_w), ("y", src_h)):
        exact = float(np.float64(src) * np.float64(ps))
        nearest = round(exact)
        result[axis] = {
            "exact": exact,
            "old": max(int(np.floor(exact)), 1),
            "new": nearest if abs(exact - nearest) <= 1e-3 else int(np.floor(exact)),
        }
    return result


def dpi_setups(view_w, view_h):
    """同じ物理ビューポートを、よくある表示倍率で見たときの (論理サイズ, ppp)。"""
    return [
        ("100%", view_w, view_h, 1.0),
        ("125%", round(view_w / 1.25), round(view_h / 1.25), 1.25),
        ("150%", round(view_w / 1.5), round(view_h / 1.5), 1.5),
    ]


def triggers_on_both_axes(src_w, src_h, view_w, view_h):
    """どの表示倍率でも両軸とも 1px 短くなるか。"""
    for _, vw, vh, ppp in dpi_setups(view_w, view_h):
        r = emulate(src_w, src_h, vw, vh, ppp)
        if r["scale"] >= 1.0:
            return False
        if not (r["x"]["old"] < r["x"]["new"] and r["y"]["old"] < r["y"]["new"]):
            return False
    return True


def search(view_w, view_h, limit=10):
    found = []
    for h in range(int(view_h * 1.05), int(view_h * 3.0)):
        for w in range(int(h * 0.55), min(int(h * 0.80), view_w)):
            if triggers_on_both_axes(w, h, view_w, view_h):
                found.append((w, h))
                break
        if len(found) >= limit:
            break
    return found


def build_chart(src_w, src_h, dst_w, dst_h, out_dir):
    scale = dst_h / src_h
    img = np.full((src_h, src_w), 0.5)
    xs, ys = np.meshgrid(
        np.arange(src_w, dtype=np.float64), np.arange(src_h, dtype=np.float64)
    )

    def stripes(y0, height, target_period, vertical):
        # 正弦波にするのは、ソース側の画素格子に縞が乗らないようにするため。矩形波だと
        # ソース自身のジッタが混ざり、何を見ているのか分からなくなる。
        period = target_period / scale
        mask = (ys >= y0) & (ys < y0 + height)
        axis = xs[mask] if vertical else ys[mask]
        img[mask] = 0.5 + 0.47 * np.cos(2 * np.pi * axis / period)

    label_h = 62
    # 横縞の帯は画像の縦中央をちょうど跨がせる。上下方向の最悪点がそこだから。
    h_height = round(src_h * 0.22)
    h_top = src_h // 2 - h_height // 2
    band_h = round(src_h * 0.135)
    plan = [
        (
            round(src_h * 0.063),
            band_h,
            2.5,
            True,
            "縦縞 2.5px 相当 — 左右方向。左端・中央・右端でコントラストを比べる",
        ),
        (round(src_h * 0.227), band_h, 3.0, True, "縦縞 3.0px 相当"),
        (h_top, h_height, 2.5, False, "横縞 2.5px 相当 — 上下方向。この帯の中央が最悪点"),
        (round(src_h * 0.637), band_h, 4.0, True, "縦縞 4.0px 相当"),
    ]
    for y0, height, period, vertical, _ in plan:
        stripes(y0, height, period, vertical)

    zp_top = round(src_h * 0.801)
    zp_h = src_h - zp_top - round(src_h * 0.035)
    cx, cy = src_w / 2, zp_top + zp_h / 2
    mask = (ys >= zp_top) & (ys < zp_top + zp_h)
    img[mask] = 0.5 + 0.47 * np.cos(((xs[mask] - cx) ** 2 + (ys[mask] - cy) ** 2) / 700.0)

    im = Image.fromarray((np.clip(img, 0, 1) * 255 + 0.5).astype(np.uint8), "L").convert(
        "RGB"
    )
    draw = ImageDraw.Draw(im)
    try:
        font = ImageFont.truetype("C:/Windows/Fonts/YuGothM.ttc", 30)
        big = ImageFont.truetype("C:/Windows/Fonts/YuGothB.ttc", 38)
    except OSError:
        font = big = ImageFont.load_default()

    def label(y_band_top, text):
        draw.rectangle(
            [0, y_band_top - label_h, src_w, y_band_top - 4], fill=(255, 255, 255)
        )
        draw.text((24, y_band_top - label_h + 14), text, fill=(200, 0, 0), font=font)

    for y0, _, _, _, text in plan:
        label(y0, text)
    label(zp_top, "ゾーンプレート — 中心付近のリングの見え方")
    draw.rectangle([0, 0, src_w, 88], fill=(255, 255, 255))
    draw.text(
        (24, 20),
        "縞のコントラストが中央で落ちたら旧挙動 / 全面で均一なら修正済み",
        fill=(200, 0, 0),
        font=big,
    )
    draw.rectangle([0, src_h - 80, src_w, src_h], fill=(255, 255, 255))
    draw.text(
        (24, src_h - 62),
        f"{src_w}x{src_h} / ページフィットで {dst_w}x{dst_h} / 表示倍率 100%-150% で有効",
        fill=(200, 0, 0),
        font=font,
    )

    os.makedirs(out_dir, exist_ok=True)
    chart = os.path.join(out_dir, f"lanczos-check-{src_w}x{src_h}.png")
    im.save(chart)

    fixed = im.resize((dst_w, dst_h), Image.LANCZOS)
    # 旧挙動 = 1px 小さいテクスチャを作ってから、描画矩形へバイリニアで引き伸ばす。
    old = im.resize((dst_w - 1, dst_h - 1), Image.LANCZOS).resize(
        (dst_w, dst_h), Image.BILINEAR
    )
    fixed.save(os.path.join(out_dir, "expected-fixed.png"))
    old.save(os.path.join(out_dir, "expected-v3.3.0.png"))
    return chart, fixed, old, plan, scale


def contrast_profile(pil, y0, y1, axis):
    arr = np.asarray(pil.convert("L"), dtype=np.float64)[y0:y1]
    return [seg.std() for seg in np.array_split(arr, 8, axis=axis)]


def build_comparisons(fixed, old, out_dir):
    try:
        font = ImageFont.truetype("C:/Windows/Fonts/YuGothB.ttc", 26)
        small = ImageFont.truetype("C:/Windows/Fonts/YuGothM.ttc", 22)
    except OSError:
        font = small = ImageFont.load_default()
    rows = [("修正後 (dev-runtime)", fixed), ("v3.3.0 (現行)", old)]
    pad, header, lab = 16, 44, 30
    width = fixed.size[0]

    band_y, ch, cw = round(fixed.size[1] * 0.079), 150, 300
    spots = [(20, "左端"), (width // 2 - cw // 2, "中央"), (width - cw - 20, "右端")]
    canvas = Image.new(
        "RGB",
        (pad + len(spots) * (cw + pad), header + len(rows) * (lab + ch + pad) + pad + 22),
        (245, 245, 245),
    )
    draw = ImageDraw.Draw(canvas)
    draw.text(
        (pad, 10), "縦縞 2.5px 相当 — 左右方向のボケ (等倍切り出し)", fill=(0, 0, 0), font=font
    )
    for i, (cx, name) in enumerate(spots):
        draw.text((pad + i * (cw + pad), header - 2), name, fill=(90, 90, 90), font=small)
    y = header + 22
    for name, src in rows:
        draw.text((pad, y), name, fill=(0, 0, 0), font=small)
        for i, (cx, _) in enumerate(spots):
            canvas.paste(
                src.crop((cx, band_y, cx + cw, band_y + ch)), (pad + i * (cw + pad), y + lab)
            )
        y += lab + ch + pad
    canvas.save(os.path.join(out_dir, "compare-vertical-stripes.png"))

    hb_y, ch2, cw2 = round(fixed.size[1] * 0.463), 420, 560
    canvas2 = Image.new(
        "RGB", (pad * 3 + cw2 * 2, header + lab + ch2 + pad * 2), (245, 245, 245)
    )
    draw2 = ImageDraw.Draw(canvas2)
    draw2.text(
        (pad, 10),
        "横縞 2.5px 相当 — 上下方向のボケ (画像の縦中央、等倍切り出し)",
        fill=(0, 0, 0),
        font=font,
    )
    for i, (name, src) in enumerate(rows):
        x = pad + i * (cw2 + pad)
        draw2.text((x, header), name, fill=(0, 0, 0), font=small)
        canvas2.paste(src.crop((440, hb_y, 440 + cw2, hb_y + ch2)), (x, header + lab))
    canvas2.save(os.path.join(out_dir, "compare-horizontal-stripes.png"))


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--viewport", default="3840x2160", help="物理ビューポート (既定: 4K)")
    ap.add_argument("--source", default=None, help="ソース寸法 WxH。省略時は自動で選ぶ")
    ap.add_argument("--out", default="C:\\tmp\\miv-lanczos-check")
    ap.add_argument("--search", action="store_true", help="条件を満たす寸法を列挙して終了")
    args = ap.parse_args()

    view_w, view_h = (int(v) for v in args.viewport.lower().split("x"))
    if args.search:
        for w, h in search(view_w, view_h):
            r = emulate(w, h, view_w, view_h, 1.0)
            print(f"{w}x{h}  scale={r['scale']:.9f}  -> {r['x']['new']}x{r['y']['new']}")
        return

    if args.source:
        src_w, src_h = (int(v) for v in args.source.lower().split("x"))
    else:
        found = search(view_w, view_h, limit=1)
        if not found:
            raise SystemExit(f"{view_w}x{view_h} で条件を満たす寸法が見つかりません")
        src_w, src_h = found[0]

    if not triggers_on_both_axes(src_w, src_h, view_w, view_h):
        print(f"警告: {src_w}x{src_h} は全ての表示倍率で両軸短縮にはなりません")

    r = emulate(src_w, src_h, view_w, view_h, 1.0)
    dst_w, dst_h = r["x"]["new"], r["y"]["new"]
    chart, fixed, old, plan, scale = build_chart(src_w, src_h, dst_w, dst_h, args.out)
    build_comparisons(fixed, old, args.out)
    print(f"chart: {chart}  ({src_w}x{src_h} -> {dst_w}x{dst_h}, scale {r['scale']:.9f})")
    for y0, height, period, vertical, _ in plan:
        ry0, ry1 = int(y0 * scale) + 15, int((y0 + height) * scale) - 15
        axis = 1 if vertical else 0
        kind = "縦縞" if vertical else "横縞"
        print(
            f"  {kind} {period}px  fixed : "
            + " ".join("%5.1f" % v for v in contrast_profile(fixed, ry0, ry1, axis))
        )
        print(
            f"  {kind} {period}px  v3.3.0: "
            + " ".join("%5.1f" % v for v in contrast_profile(old, ry0, ry1, axis))
        )


if __name__ == "__main__":
    main()
