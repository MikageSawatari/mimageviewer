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
  `harmnet_weights.h` by `crates/music-core/examples/pitch_lab/convert_weights.py`).
