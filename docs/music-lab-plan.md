# 音楽ビュー / music_lab 初期実装メモ

## 目的

本体の 1 回 4 分級ビルドを避けるため、音楽再生・解析・表示の探索を
`crates/music-core` と `tools/music_lab` に分離する。補正レイヤー / comic lab と同じく、
UI の手触りとデータモデルを固めてから本体へ統合する。

## 方針

- `music-core` は GUI / cpal / FFmpeg / VST3 に依存しない純ロジックにする。
- `music_lab` は軽量 eframe アプリとして、音声ファイル decode、再生、1 分 1 行の
  DJ 風タイムライン、簡易 BPM 推定、ブックマーク UI を試す。
- 本体統合時は `GridItem::Audio` を追加し、動画と同じ media viewer 枠で
  `MediaVisualMode::Music` を表示する。
- 動画は通常 `MediaVisualMode::Video`、映像 OFF / 音楽モード時は同じ音声 source を
  `MediaVisualMode::Music` で表示する。

## VST3 統合を見越した境界

`music-core::effects::EffectChain` を音声処理の差し込み境界にする。

- lab は `NoopEffectChain` で進める。
- 本体では既存 `src/video/dsp::DspBridge` を adapter で `EffectChain` 相当に接続する。
- VST3 IPC は既存動画設計と同じく audio pump thread で行い、cpal の realtime callback
  から直接呼ばない。
- 動画 / 音楽の両方で同じ VST3 チェーン、同じ plugin state、同じ GUI 管理を使う。

## 初期スコープ

1. `music-core`
   - stereo waveform bin
   - low/mid/high energy
   - loudness/RMS/peak
   - 簡易 BPM / beat/bar grid
   - media visual mode 型
   - VST3 用 effect-chain trait
2. `music_lab`
   - 音声ファイルを開く
   - 全尺 decode + timeline analysis
   - Play/Pause/Stop/seek 用の最小プレイヤー
   - 左 bookmark / 右 details / 中央 1 分 1 行 timeline
     - 上段: 周波数色分けした DJ 風の塗り波形
     - 下段: ラウドネス面グラフ
   - 下段 50-band analyzer

## 本体統合時の注意

- 解析 / DB / waveform 生成は UI thread で行わない。
- 解析結果は `audio_analysis.db` へ path + size + mtime + analysis_version で保存する。
- 小節頭と BPM は誤検出がある前提で、手動補正を永続化する。
- VST3 有効時の PDC は動画と同じく再生 clock / decoder pacing へ反映する。
