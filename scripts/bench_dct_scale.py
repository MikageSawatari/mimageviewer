"""DCT-scale vs full-decode JPEG thumbnail benchmark.

For each test JPEG:
  Path A: full decode -> Lanczos resize to target_px max edge
  Path B: DCT scale via Pillow draft() -> Lanczos resize to target_px max edge

Times each path multiple iterations, reports median.
Computes PSNR between A and B outputs (quality delta).
Writes side-by-side comparison PNGs to OUT_DIR for visual inspection.

Pillow uses libjpeg-turbo on Windows (verified). Image.draft("JPEG", (w,h))
calls jpeg_resync_to_restart() with scale_num/scale_denom set so the
decoder produces a scaled image directly. This is the same mechanism mIV
would use through TurboJPEG's tj3SetScalingFactor.
"""
import sys, io, os, glob, time, random, statistics
sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8')

from PIL import Image
import numpy as np

SRC_DIR    = 'D:/home/photo/2025PEN'
OUT_DIR    = 'H:/home/mimageviewer_old/testimage/dct_test'
TARGET_PX  = 512      # thumbnail target max-edge (typical mIV display_px)
SAMPLES    = 30       # number of JPGs to benchmark
ITERATIONS = 5        # timing repetitions per file (take median)
SAVE_VISUAL_SAMPLES = 6  # how many A/B side-by-side PNGs to write

os.makedirs(OUT_DIR, exist_ok=True)
random.seed(42)
files = sorted(glob.glob(SRC_DIR + '/*.JPG'))
sample = random.sample(files, min(SAMPLES, len(files)))

def path_a_full_decode(path, target):
    """Full decode then Lanczos resize. Mimics zune-jpeg path in mIV."""
    img = Image.open(path)
    img.load()                              # force decode
    w, h = img.size
    scale = target / max(w, h)
    new_size = (max(1, round(w * scale)), max(1, round(h * scale)))
    return img.resize(new_size, Image.LANCZOS)

def path_b_dct_scale(path, target):
    """DCT-scale via draft() then Lanczos resize. Mimics proposed TurboJPEG path."""
    img = Image.open(path)
    # draft() finds the largest DCT scale where output stays >= target on both axes.
    # libjpeg-turbo accepts 1/8, 1/4, 3/8, 1/2, 5/8, 3/4, 7/8, 1 (and 2..16).
    img.draft('RGB', (target, target))
    img.load()                              # decode at scaled resolution
    w, h = img.size
    scale = target / max(w, h)
    if scale < 0.999:
        new_size = (max(1, round(w * scale)), max(1, round(h * scale)))
        img = img.resize(new_size, Image.LANCZOS)
    return img

def psnr(a, b):
    a = np.asarray(a, dtype=np.float32)
    b = np.asarray(b, dtype=np.float32)
    # Resize b to match a if different (DCT path may end up 1px off)
    if a.shape != b.shape:
        from PIL import Image as I
        b = np.asarray(I.fromarray(b.astype(np.uint8)).resize(
            (a.shape[1], a.shape[0]), I.LANCZOS), dtype=np.float32)
    mse = float(np.mean((a - b) ** 2))
    if mse == 0:
        return float('inf')
    return 20.0 * np.log10(255.0 / np.sqrt(mse))

def time_call(fn, *args, n=ITERATIONS):
    # 1 warmup, then n timed
    fn(*args)
    times = []
    for _ in range(n):
        t0 = time.perf_counter()
        fn(*args)
        times.append((time.perf_counter() - t0) * 1000.0)
    return statistics.median(times), min(times), max(times)

print(f'Source:    {SRC_DIR}')
print(f'Output:    {OUT_DIR}')
print(f'Target px: {TARGET_PX} (max edge)')
print(f'Samples:   {len(sample)} files x {ITERATIONS} iterations (median)')
print(f'Pillow {Image.__version__} (libjpeg-turbo backend)')
print()
print(f'{"file":18}{"size":>9}{"WxH":>14}{"DCTscl":>9}{"A(full)":>12}{"B(dct)":>12}{"speedup":>10}{"PSNR":>9}')
print('-' * 105)

a_times, b_times, psnrs, ratios = [], [], [], []
visual_indices = sorted(random.sample(range(len(sample)), min(SAVE_VISUAL_SAMPLES, len(sample))))

for idx, path in enumerate(sample):
    size_mb = os.path.getsize(path) / 1e6
    img_info = Image.open(path)
    src_size = img_info.size
    img_info.close()

    a_med, a_min, a_max = time_call(path_a_full_decode, path, TARGET_PX)
    b_med, b_min, b_max = time_call(path_b_dct_scale,   path, TARGET_PX)

    # Quality: compute PSNR between A and B outputs
    out_a = path_a_full_decode(path, TARGET_PX).convert('RGB')
    out_b = path_b_dct_scale  (path, TARGET_PX).convert('RGB')
    p = psnr(out_a, out_b)

    # DCT scale actually used (1/N where N is the smallest where dim >= target)
    max_src = max(src_size)
    dct_n = 8
    for n in (8, 4, 8/3, 2, 8/5, 4/3, 8/7, 1):
        if max_src / n >= TARGET_PX:
            dct_n = n
            break
    dct_label = f'1/{dct_n:g}' if dct_n != 1 else 'none'

    speedup = a_med / b_med if b_med > 0 else 0
    a_times.append(a_med); b_times.append(b_med); psnrs.append(p); ratios.append(speedup)

    name = os.path.basename(path)
    print(f'{name:18}{size_mb:7.1f}MB{src_size[0]:>6}x{src_size[1]:<6}{dct_label:>9}{a_med:9.1f}ms{b_med:9.1f}ms{speedup:8.2f}x{p:7.1f}dB')

    if idx in visual_indices:
        # Save A and B side by side as PNG for visual comparison
        out_path = os.path.join(OUT_DIR, f'{os.path.splitext(name)[0]}_compare.png')
        from PIL import ImageDraw, ImageFont
        # Stack horizontally: [A | B | diff]
        h = max(out_a.height, out_b.height)
        composite = Image.new('RGB', (out_a.width + out_b.width + 20, h + 30), (40, 40, 40))
        composite.paste(out_a, (0, 30))
        composite.paste(out_b, (out_a.width + 20, 30))
        draw = ImageDraw.Draw(composite)
        draw.text((4, 4), f'A: full-decode + resize ({a_med:.0f}ms)', fill='white')
        draw.text((out_a.width + 24, 4), f'B: DCT-scale {dct_label} + resize ({b_med:.0f}ms, PSNR {p:.1f}dB)', fill='white')
        composite.save(out_path, optimize=False)

print('-' * 105)
print(f'{"MEDIAN":18}{"":>9}{"":>14}{"":>9}{statistics.median(a_times):9.1f}ms{statistics.median(b_times):9.1f}ms{statistics.median(ratios):8.2f}x{statistics.median(psnrs):7.1f}dB')
print(f'{"MEAN":18}{"":>9}{"":>14}{"":>9}{statistics.mean(a_times):9.1f}ms{statistics.mean(b_times):9.1f}ms{statistics.mean(ratios):8.2f}x{statistics.mean(psnrs):7.1f}dB')
print()
print(f'PSNR interpretation: >40dB = visually identical, 30-40dB = good, <30dB = visible difference')
print(f'Visual samples saved to: {OUT_DIR}')
