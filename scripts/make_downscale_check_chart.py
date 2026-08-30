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

**修正後は、どの帯も全面で均一。旧挙動はムラが出る。** ムラの形は原因によって違う:

- 原因 1 (テクスチャが 1px 短い) が効く軸は、位相が端から端へ 0 -> 1 と流れる。位相 0.5 が
  2 タップ平均で最もボケるので、**中央がいちばん薄い**山なりになる。帯がその最悪点に
  重なっていれば帯ごと薄くなる。
- 原因 2 (矩形が小数サイズ) が効く軸は、位相が 0 から端数ぶんだけ流れる。端数が 0.43 なら
  **片側から反対側へ一方向に薄くなる**。

どちらも「均一かどうか」で判定できる。方向や形を覚える必要はない。
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


# 原因 2 を炙り出すのに必要な、描画矩形の端数の下限 (物理ピクセル)。0.5 に近いほど
# 追加バイリニアの 2 タップ平均が効き、コントラストの落ち方が大きくなる。
CROSS_AXIS_FRACTION_MIN = 0.3


def triggers_both_causes(src_w, src_h, view_w, view_h):
    """どの表示倍率でも、**原因 1 と原因 2 の両方**が出るか。

    - 原因 1 (丸め): フィットする軸で `floor` が 1px 短くする。理論値がちょうど
      ビューポート幅/高さなので、f32 の誤差でぎりぎり整数を下回るときに起きる。
    - 原因 2 (矩形の端数): もう一方の軸で描画矩形が小数サイズになる。テクスチャは
      整数なので、寄せないと横方向だけ再サンプルされる。

    **両方を要求するのが要点。** 「両軸とも 1px 短くなる」だけを条件にすると、もう一方の
    軸も整数近傍に固定されてしまい、**原因 2 が出ない寸法しか選ばれない** (実際、最初の
    版で選ばれた 1612x2418 は矩形の端数が 0.00002px しかなく、サイズ寄せを外しても
    見た目が変わらなかった)。
    """
    for _, vw, vh, ppp in dpi_setups(view_w, view_h):
        r = emulate(src_w, src_h, vw, vh, ppp)
        if r["scale"] >= 1.0:
            return False
        fit_axis, cross_axis = ("y", "x") if vh / src_h < vw / src_w else ("x", "y")
        if r[fit_axis]["old"] >= r[fit_axis]["new"]:
            return False  # 原因 1 が出ない
        cross = r[cross_axis]["exact"]
        if abs(cross - round(cross)) < CROSS_AXIS_FRACTION_MIN:
            return False  # 原因 2 が出ない
    return True


def search(view_w, view_h, limit=10):
    found = []
    for h in range(int(view_h * 1.05), int(view_h * 3.0)):
        for w in range(int(h * 0.55), min(int(h * 0.80), view_w)):
            if triggers_both_causes(w, h, view_w, view_h):
                found.append((w, h))
                break
        if len(found) >= limit:
            break
    return found


def draw_into_fractional_rect(texture, rect_w, rect_h):
    """整数サイズのテクスチャを、**小数サイズの矩形**へ貼った結果を再現する。

    これが原因 2 の本体。GPU は出力画素の中心 `(i + 0.5)` を矩形座標とみなしてテクスチャを
    サンプルするので、矩形幅が 1187.43px でテクスチャが 1187px だと、テクセル中心が端から
    端へ最大 0.43px ずれていく。整数リサイズでは再現できない (両者とも 1187 になる)。
    """
    tex = np.asarray(texture.convert("L"), dtype=np.float64)
    tex_h, tex_w = tex.shape
    out_w, out_h = round(rect_w), round(rect_h)
    us = (np.arange(out_w) + 0.5) * (tex_w / rect_w) - 0.5
    vs = (np.arange(out_h) + 0.5) * (tex_h / rect_h) - 0.5
    us = np.clip(us, 0, tex_w - 1)
    vs = np.clip(vs, 0, tex_h - 1)
    u0 = np.floor(us).astype(int)
    v0 = np.floor(vs).astype(int)
    u1 = np.minimum(u0 + 1, tex_w - 1)
    v1 = np.minimum(v0 + 1, tex_h - 1)
    fu = (us - u0)[None, :]
    fv = (vs - v0)[:, None]
    top = tex[np.ix_(v0, u0)] * (1 - fu) + tex[np.ix_(v0, u1)] * fu
    bottom = tex[np.ix_(v1, u0)] * (1 - fu) + tex[np.ix_(v1, u1)] * fu
    out = top * (1 - fv) + bottom * fv
    return Image.fromarray(np.clip(out, 0, 255).astype(np.uint8), "L").convert("RGB")


def build_chart(src_w, src_h, dst_w, dst_h, out_dir, old_size=None, rect_size=None):
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
            "縦縞 2.5px 相当 — 左右方向。左端から右端までコントラストが均一か",
        ),
        (round(src_h * 0.227), band_h, 3.0, True, "縦縞 3.0px 相当"),
        (h_top, h_height, 2.5, False, "横縞 2.5px 相当 — 上下方向。帯ごと薄くなっていないか"),
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
        "どの帯も全面で均一なら修正済み / ムラがあれば旧挙動",
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
    # 旧挙動 = 旧規則で決まるサイズのテクスチャを作ってから、**寄せていない小数サイズの
    # 矩形**へ貼る。原因 1 (テクスチャが 1px 短い) と原因 2 (矩形が小数) の両方が乗る。
    old_size = old_size or (dst_w - 1, dst_h - 1)
    rect_size = rect_size or (float(dst_w), float(dst_h))
    old = draw_into_fractional_rect(
        im.resize(old_size, Image.LANCZOS), rect_size[0], rect_size[1]
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

    if not triggers_both_causes(src_w, src_h, view_w, view_h):
        print(f"警告: {src_w}x{src_h} は全ての表示倍率で原因 1 と 2 の両方を出しません")
    r0 = emulate(src_w, src_h, view_w, view_h, 1.0)
    for axis in ("x", "y"):
        exact = r0[axis]["exact"]
        print(
            f"  {axis}: 理論値 {exact:.5f}  旧 {r0[axis]['old']}  新 {r0[axis]['new']}"
            f"  矩形の端数 {abs(exact - round(exact)):.5f}px"
        )

    r = emulate(src_w, src_h, view_w, view_h, 1.0)
    dst_w, dst_h = r["x"]["new"], r["y"]["new"]
    chart, fixed, old, plan, scale = build_chart(
        src_w,
        src_h,
        dst_w,
        dst_h,
        args.out,
        old_size=(r["x"]["old"], r["y"]["old"]),
        rect_size=(r["x"]["exact"], r["y"]["exact"]),
    )
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
