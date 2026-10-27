# EffeTune Note Spectrogram (HarmNet L)

mIV's music-view keyboard uses a Rust port of the Note Spectrogram pitch estimator from
EffeTune DSP 0.13.0 (https://github.com/Frieve-A/effetune, MIT, Copyright (c) 2025-2026
Yoshiyuki Kobayashi). Source commit: 7baf521f37330e84bdee8688526ccfb6d0eaea32
(`dsp/plugins/analyzer/note_spectrogram/`).

- `LICENSE` - EffeTune's MIT license text (shown in the About dialog for every build flavor).
- `harmnet_weights.provenance.json` - upstream provenance of the trained weights
  (training datasets, calibration). The weights are embedded as
  `crates/music-core/assets/harmnet_l_effetune_dsp_0.13.0.bin` (float32 LE, 47,012 values,
  SHA-256 0164ecb8b4eb9143a20255669612aed306c9db9b670d892de1aee7492051b6a3, converted from
  `harmnet_weights.h` by `third_party/effetune-harmnet/convert_weights.py`).

The estimator and weights are part of the core in every distribution flavor, including portable.
They do not require the Mixwright bundle, its VST host, or WebView storage. Portable excludes
the Mixwright bundle and its notices; the HarmNet MIT notice is embedded separately and appears
in the About dialog alongside the other core component notices.

To regenerate the weight asset from the pinned upstream header:

```powershell
python .\third_party\effetune-harmnet\convert_weights.py .\path\to\harmnet_weights.h .\crates\music-core\assets\harmnet_l_effetune_dsp_0.13.0.bin
```

Replace the header path with the pinned upstream copy. The script has no EffeTune Python dependency.
Verify the value count, length, and SHA-256 above
after conversion. The synthetic golden fixture generator lives in `crates/music-core/tests/fixtures/`;
the temporary pitch-lab research examples are not part of the product.
