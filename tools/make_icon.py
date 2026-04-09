"""
mImageViewer アイコン生成スクリプト
  - "mIV" テキスト、ピンク系グラデーション背景、丸角
  - 256/128/64/48/32/16 px の全サイズを ICO に収録
出力: assets/icon.ico
"""

import os
import numpy as np
from PIL import Image, ImageDraw, ImageFont

SIZES = [256, 128, 64, 48, 32, 16]

# ピンク系グラデーション: #ff60c0 → #d63384 (斜め)
C1 = np.array([0xff, 0x60, 0xc0], dtype=np.float32)  # 左上
C2 = np.array([0xd6, 0x33, 0x84], dtype=np.float32)  # 右下


def make_frame(size: int) -> Image.Image:
    # ── グラデーション背景 ──────────────────────────────────────────
    xs = np.linspace(0, 1, size, dtype=np.float32)
    ys = np.linspace(0, 1, size, dtype=np.float32)
    xg, yg = np.meshgrid(xs, ys)
    t = (xg + yg) / 2.0                             # 0 (左上) → 1 (右下)
    rgb = (C1 * (1 - t[..., None]) + C2 * t[..., None]).astype(np.uint8)
    alpha = np.full((size, size, 1), 255, dtype=np.uint8)
    rgba = np.concatenate([rgb, alpha], axis=2)
    img = Image.fromarray(rgba, "RGBA")

    # ── 丸角マスク ───────────────────────────────────────────────
    radius = max(size // 5, 2)
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [0, 0, size - 1, size - 1], radius=radius, fill=255
    )
    img.putalpha(mask)

    # ── フォント選択 ─────────────────────────────────────────────
    font_size = max(int(size * 0.40), 6)
    font = None
    candidates = [
        r"C:\Windows\Fonts\arialbd.ttf",
        r"C:\Windows\Fonts\calibrib.ttf",
        r"C:\Windows\Fonts\verdanab.ttf",
        r"C:\Windows\Fonts\segoeui.ttf",
    ]
    for path in candidates:
        if os.path.exists(path):
            try:
                font = ImageFont.truetype(path, font_size)
                break
            except Exception:
                pass
    if font is None:
        font = ImageFont.load_default()

    # ── テキスト描画 ─────────────────────────────────────────────
    draw = ImageDraw.Draw(img)
    text = "mIV"
    bbox = draw.textbbox((0, 0), text, font=font)
    tw = bbox[2] - bbox[0]
    th = bbox[3] - bbox[1]
    x = (size - tw) // 2 - bbox[0]
    y = (size - th) // 2 - bbox[1]

    # 影（大きいサイズのみ）
    if size >= 48:
        shadow_off = max(size // 48, 1)
        draw.text((x + shadow_off, y + shadow_off), text,
                  fill=(0, 0, 0, 70), font=font)

    # 白テキスト
    draw.text((x, y), text, fill=(255, 255, 255, 255), font=font)

    return img


def main():
    out_dir = os.path.join(os.path.dirname(__file__), "..", "assets")
    os.makedirs(out_dir, exist_ok=True)
    out_path = os.path.join(out_dir, "icon.ico")

    frames = [make_frame(s) for s in SIZES]

    # PIL の ICO 保存: sizes リストに各フレームのサイズを指定
    frames[0].save(
        out_path,
        format="ICO",
        sizes=[(s, s) for s in SIZES],
        append_images=frames[1:],
    )
    print(f"Saved: {out_path}")
    for s, f in zip(SIZES, frames):
        print(f"  {s:3d}x{s:3d}  mode={f.mode}")


if __name__ == "__main__":
    main()
