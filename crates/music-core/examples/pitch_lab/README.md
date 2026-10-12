# pitch-lab (調査用、製品コードではない)

2026-10-12 調査: mIV の音楽ビュー鍵盤と EffeTune Note Spectrogram (HarmNet L) の比較。

- `../keyboard_dump.rs` — mIV の SpectrumAnalyzer + `src/ui_music_spectrum.rs` の鍵盤処理を
  逐語コピーしたハーネス (`gen_example.py` が本体ファイルから関数を抜き出して生成)。
- `../harmnet_dump.rs` — EffeTune DSP 0.13.0 の HarmNet L 推論の Rust 移植試作。
  Python `effetune==0.13.0` の head0 出力と最大差 1e-4 で一致。scalar 実装で約 2ms/frame (20ms hop)。
- `gen_signals.py` 合成テスト音 (正解付き)、`run_effetune.py` 基準出力、`evaluate.py` 比較、
  `semi.py` 半音隣接比、`roll.py` ピアノロール画像、`convert_weights.py` 重み変換。

手順: `python gen_signals.py` → `python run_effetune.py <name>` (venv に effetune 0.13.0) →
`cargo run --release -p music-core --example keyboard_dump -- <name>.f32 <name>_miv.csv 16` →
`cargo run --release -p music-core --example harmnet_dump -- harmnet_l.bin <name>.f32 <name>_hn.csv` →
`python evaluate.py <names...>`。
