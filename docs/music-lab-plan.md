# 音楽ビュー / music_lab 初期実装メモ

## 目的

本体の 1 回 4 分級ビルドを避けるため、音楽再生・解析・表示の探索を
`crates/music-core` と `tools/music_lab` に分離する。補正レイヤー / comic lab と同じく、
UI の手触りとデータモデルを固めてから本体へ統合する。

## 方針

- `music-core` は GUI / cpal / FFmpeg / VST3 に依存しない純ロジックにする。
- `music_lab` は軽量 eframe アプリとして、音声ファイル decode、再生、30 秒 1 行の
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

## 解析エンジン候補

- ライセンス方針:
  - AGPL / GPL / NC 条件のモデルは本体同梱しない。
  - ラボでは外部 Python ツールとして精度確認することは許容するが、配布候補とは分ける。
  - ONNX / `ort` 統合に進む前に、コード・モデル重み・学習データ由来条件を個別に監査する。
- Beat / downbeat:
  - All-In-One Music Structure Analyzer: MIT。beats / downbeats / segment labels を一括で返すため本命候補。
    ただし実行依存に PyTorch / NATTEN / madmom / Demucs が絡むため、まず外部 JSON 連携で評価する。
    学習済み weights の再配布条件が明確になるまでは本体同梱しない。
  - beat-this / BeatNet: permissive license なら候補。モデル重みの条件確認が必要。
  - madmom: beat / downbeat の研究実装として有力だが、モデル/データが NC 条件なので本体同梱しない。
  - Essentia: BPM / beat ticks / confidence を返せるが AGPLv3 / 商用ライセンス条件に注意し、本体同梱しない。
  - aubio: GPLv3 のため本体同梱しない。
  - bpm-analyzer / beat-detector: Rust ネイティブの軽量候補。ライセンスと精度を確認する。
  - librosa: 試作と検証に向く。製品組み込みより Python sidecar / offline analyzer 向き。
- Clean MVP:
  - Beat / tempo は onset envelope、autocorrelation / comb filter、phase fitting を `music-core` 側で実装する。
  - Beat grid は誤検出前提で confidence、手動 BPM、first beat、downbeat offset を保存できる形にする。
  - Downbeat / section label は自動判定を急がず、まず補正可能なグリッドとマーカーを優先する。
- Section:
  - まずは chorus / verse の意味ラベルではなく、segment boundary / repeated section として扱う。
  - All-In-One は intro / verse / chorus / bridge / outro などの functional label を返すため優先評価する。
  - librosa の recurrence / agglomerative clustering、MSAF 系を試作候補にする。
  - 意味ラベルは自動判定だけに頼らず、ユーザー補正・色分け・保存を前提にする。
- Vocal interval:
  - 曲冒頭シーク用途では音源分離ではなく、vocal activity 区間だけを検出する。
  - Clean MVP は数秒窓の mid-band energy、harmonicity、spectral flatness、onset density から
    vocal-likelihood を作り、歌い出し候補時刻だけをキャッシュする。
  - 初期実装は `WaveformBin.vocal_score` として、軽量 DSP の中域比率 / zero-crossing /
    crest / transient 抑制 / 約 1 秒の持続性から 0..1 のスコアを作る。
    タイムライン下段はラウドネスと混色せず、vocal hint を独立レーンとして表示する。
  - 散発的な誤反応を避けるため、短い断片は捨て、約 2 秒以上まとまる候補だけを表示する。
    autocorrelation による有声音の周期性も加えて、ノイズ的な中域反応を抑える。
    倍音/周期性はギターやシンセにも出るため、軽量 DSP だけではインストとの完全分離は期待しない。
    次の改善候補は formant 風の中域包絡や YAMNet / PANNs sidecar との比較。
    短いギャップは bridge し、明確な終了では release を速める。
    男性ボーカル / ラップ / 強い加工声では高精度ラベルと軽量 DSP のズレが大きくなりやすいので、
    評価セットに含める。軽量 DSP は完全一致ではなく、低 FP 寄りの「それっぽいヒント」
    を合格ラインにする。
  - DSP の調整は [music-lab-vocal-eval.md](music-lab-vocal-eval.md) の教師ラベル JSON と
    `cargo run -p music_lab --bin vocal_eval -- labels.json` で precision / recall を見ながら進める。
    教師ラベルは手入力を正本にせず外部高精度ツールで作ってよいが、重いモデルの起動コードは
    lab 本体に持たせず、生成済み JSON を明示的に評価 CLI へ渡す。
  - All-In-One の demixed vocal stem や embeddings を利用できるか評価する。
  - 重い音源分離 / 高精度モデルは lab 本体から起動せず、必要なら外部で評価ラベルを生成して持ち込む。
  - inaSpeechSegmenter は MIT だが singing voice は music 扱いなので、歌あり区間の検出にはそのまま使わない。
  - SingingVoiceDetection 系の MIT 実装や music tagging モデルを、ONNX 化できるか確認する。

## 初期スコープ

1. `music-core`
   - stereo waveform bin (lab default 10 ms)
   - low/mid/high energy
   - loudness/RMS/peak
   - 簡易 BPM / beat/bar grid
   - media visual mode 型
   - VST3 用 effect-chain trait
2. `music_lab`
  - 音声ファイル、または動画ファイル内の音声トラックを開く
  - Open ダイアログとファイル D&D の両方で読み込みを開始する
  - 全尺 decode + timeline analysis
   - Play/Pause/Stop/seek 用の最小プレイヤー
   - 左 bookmark / 右 details / 中央 30 秒 1 行 timeline
     - 上段: 周波数色分けした DJ 風の塗り波形
     - 音量の縦ラインを低 / 中 / 高域で分割して塗り、強い立ち上がりだけ transient accent として重ねる
     - 下段: Loudness / Bass root / Key / Brightness / Drums / Change / Vocal hint の
       独立メトリクスレーン
     - Bass root / Key は 12 音の pitch class を五度圏カラーで表示し、オクターブ違いは同じ色にする
     - Bass root / Key は値の高さより色の比較を優先し、検出中はレーン高いっぱいに塗る。
       値はホバー表示と透明度/明るさへ残し、Change が高い区間は少し強調する
     - Brightness は高域量そのものだけでなく、周辺時間との差分も混ぜて構成変化のヒントにする
     - Drums は単発 transient ではなく、短時間窓の transient density として表示する
     - Change は loudness / bass / brightness / drum density / vocal hint / chroma の
       前後差から section boundary 候補を示す
     - メトリクスレーンはホバーでレーン名、値、時刻、意味、推定音名を表示する
     - 再生位置の行全体が表示中に見切れる / 画面外へ出る場合だけ自動スクロールし、手動スクロール中は追従しない
   - 下段 108-band analyzer + 減衰背景
     - 20 Hz - 18 kHz を約 1 semitone 幅で分割する想定
     - ホバー時に周波数 Hz と近似音名を表示する
     - 鍵盤は実ピアノ範囲 A0-C8 を明るく、範囲外をグレーで表示する
     - 鍵盤ハイライトはスペクトラムの絶対音量とは分け、近傍より突出した音階だけを相対評価で強調する
     - 鍵盤ハイライトは短い広帯域トランジェントより、数フレーム続く音階を優先する
     - バー表示は重低音が視覚的に勝ちすぎないよう、軽い知覚寄り dB 補正で低域を抑え中域を少し持ち上げる
     - 分解能優先のため、108-band 時は長めの解析窓で少し鈍い反応にする
     - 90 Hz 以下は極低域の安定感と一拍感のバランスを取り、32768-sample FFT に 16384-sample FFT を混ぜる
     - 解析は UI thread で直接行わず、常駐 spectrum worker で FFT plan / buffer を再利用する
     - 5 段階 FFT は高域を高頻度、低域を低頻度で更新し、直近結果を合成して描画する
   - Top bar に FPS / frame ms を表示して描画負荷を確認する

## 本体統合時の注意

- 解析 / DB / waveform 生成は UI thread で行わない。
- 解析結果は `audio_analysis.db` へ path + size + mtime + analysis_version で保存する。
- 小節頭と BPM は誤検出がある前提で、手動補正を永続化する。
- lab の簡易ビート推定は低信頼度ならグリッドを描かず、本体統合時は
  beat/downbeat 専用エンジンか手動補正 UI に置き換える。
- VST3 有効時の PDC は動画と同じく再生 clock / decoder pacing へ反映する。
