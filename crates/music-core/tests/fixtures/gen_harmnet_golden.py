"""Generate the HarmNet golden fixture from EffeTune DSP (pip install effetune==0.13.0 numpy).

The input signal is defined by a formula so the Rust test can regenerate identical samples:
each segment sums tones; tone midi m with harmonic count H and exponent p contributes
0.1 * h**-p * sin(2*pi*f*h*n/48000) for h=1..H (f = 440*2**((m-69)/12)), multiplied by a
linear fade of 240 samples at both segment ends. n is the absolute sample index.
Output file harmnet_golden_effetune_dsp_0.13.0.bin (little endian):
  u32 frame_count, then frame_count rows of 88 f32 (age 0 / head 0, max over 5 cells),
  then frame_count rows of 88 f32 (age 8 revised levels; 0 when not yet available).
Row i corresponds to Rust frame L = i + 1 (window ending at sample L*960).
"""
import struct, sys
import numpy as np
import effetune as e

SR = 48000
SEGMENTS = [  # (start_s, end_s, [(midi, harmonics, exponent), ...])
    (0.30, 1.10, [(57, 8, 1.0)]),
    (1.10, 1.70, [(60, 6, 1.3), (64, 6, 1.3), (67, 6, 1.3)]),
    (2.00, 2.60, [(40, 10, 1.0)]),
]
TOTAL_S = 2.80


def signal():
    n_total = int(round(TOTAL_S * SR))
    y = np.zeros(n_total)
    for st, en, tones in SEGMENTS:
        a, b = int(round(st * SR)), int(round(en * SR))
        n = np.arange(a, b, dtype=np.float64)
        fade = np.minimum(1.0, np.minimum((n - a) / 240.0, (b - n) / 240.0))
        for m, harmonics, p in tones:
            f = 440.0 * 2 ** ((m - 69) / 12)
            for h in range(1, harmonics + 1):
                y[a:b] += 0.1 * h ** -p * np.sin(2 * np.pi * f * h * n / SR) * fade
    return y.astype(np.float32)


def main(out):
    y = signal()
    x = np.stack([y, y]).copy()
    frames = []
    e.Chain([e.NoteSpectrogram(minimum_midi=21, maximum_midi=108)]).process(
        x, sample_rate=SR, block_size=960,
        on_telemetry=lambda fr: frames.append(fr) if fr.kind == 'noteSpectrogram' else None)
    h0 = np.array([np.array(fr.levels).reshape(88, 5).max(1) for fr in frames], dtype='<f4')
    r8 = np.array([np.array(fr.revised_levels).reshape(88, 5).max(1) if fr.revised_levels
                   else np.zeros(88) for fr in frames], dtype='<f4')
    with open(out, 'wb') as f:
        f.write(struct.pack('<I', len(frames)))
        f.write(h0.tobytes())
        f.write(r8.tobytes())
    print(len(frames), 'frames')


if __name__ == '__main__':
    main(sys.argv[1] if len(sys.argv) > 1 else 'harmnet_golden_effetune_dsp_0.13.0.bin')
