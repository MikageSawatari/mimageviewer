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
  - probe 結果の duration / sample rate / channels で先に行枠を確保し、decode + timeline analysis は loader worker で継続する
  - 再生は全尺解析完了を待たず、Open / D&D 直後に cpal stream を作り、loader worker が decode したチャンクを再生バッファへ順次流し込んで自動再生する
  - 新しいファイルを開いたら旧 loader へ cancel を立て、UI へ古い結果を書き戻さない
  - timeline は decode 中も約 5 秒単位の部分解析を受け取り、解析済み区間から順次表示する。完了後は全尺 timeline analysis の確定版で置き換える
  - timeline 行テクスチャの raster は worker で行い、UI thread は完成画像を少量ずつ texture upload する。spectrum は全尺 decode 完了を待たず、再生バッファに溜まった短い窓を worker へ渡して表示する
  - Row 秒数変更などで timeline 行テクスチャを作り直すときは、再生カーソル行と現在の可視範囲を優先して raster 要求を出し、画面外の古い順処理で黒待ちが長くならないようにする
  - 長尺ロード中の音切れは `PlaybackSnapshot.decoded_secs` / `buffer_ahead_secs` / `underrun_count` で計測し、perf log と右パネルに出す。mIV 本体統合時は既存 video audio pump 側の queue / normalize / VST latency 計測へ接続する
   - Play/Pause/Stop/seek 用の最小プレイヤー
   - 左 bookmark / 右 details / 中央 30 秒 1 行 timeline
     - 上段: 周波数色分けした DJ 風の塗り波形
     - 音量の縦ラインを低 / 中 / 高域で分割して塗り、強い立ち上がりだけ transient accent として重ねる
     - 下段: 大きめの Loudness+Bass root レーンと、小さめの Key レーン
     - Key レーンは補助情報として細めにし、Loudness+Bass root より控えめに表示する
     - Loudness+Bass root は高さを loudness、色を bass root の pitch class とする
     - メトリクスレーンは上段波形より明るくなりすぎないよう、最大明度と alpha を抑える
     - Bass root / Key は 12 音の pitch class を五度圏カラーで表示し、オクターブ違いは同じ色にする
     - Bass root / Key は transient から作る 50-100ms 程度の表示用リズムグリッド境界でだけ色を変える
     - Key は短時間の chroma 変動ではなく、transient / density を弱めた長めの周辺窓 chroma を Krumhansl-Schmuckler / Temperley 系 major/minor profile と照合し、リズムグリッドへスナップして表示する
     - Key は曖昧な区間も完全には消さず、低 confidence の候補として淡く表示する
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
     - 再生バッファから切り出す短い移動窓は、同じ長さ・同じ中心位置でも中身が毎回変わるため、FFT plan は再利用しつつ窓の power cache は毎回更新する
     - 5 段階 FFT は高域を高頻度、低域を低頻度で更新し、直近結果を合成して描画する
   - Top bar に FPS / frame ms を表示して描画負荷を確認する

## 解析結果データ契約

- `TimelineAnalysis.analysis_version` は `music-core::TIMELINE_ANALYSIS_VERSION` と一致する結果だけを現行キャッシュとして扱う。
- `TimelineAnalysis::default()` と `analyze_stereo_timeline()` は常に現行 `analysis_version` を入れる。古い serialized cache に `analysis_version` が無い場合は `0` 扱いになり、現行結果としては採用しない。
- `WaveformBin` は表示に必要な時系列メトリクスを 1 bin に集約する:
  - 波形/音量: `peak`, `rms`, `loudness_db`
  - DJ 風カラー波形: `band_energy`, `transient`, `transient_band`
  - 構成ヒント: `brightness`, `transient_density`, `novelty`
  - 音程ヒント: `bass_pitch_class`, `bass_pitch_confidence`, `key_pitch_class`, `key_confidence`, `bass_chroma`, `chroma`
  - ボーカル試作値: `center_ratio`, `vocal_score`
- `BeatGrid` は `TimelineAnalysis` に含める。低 confidence の自動推定は表示側で隠せるが、キャッシュ上は推定結果と confidence を保持する。
- Row 秒数や表示幅は解析結果に含めず、表示時の row texture raster 条件として扱う。Row 切替は解析キャッシュの再生成条件にしない。
- 本体統合時の DB key は path / size / mtime / duration / sample_rate / channels / `analysis_version` を最低限の一致条件にする。

## 本体統合用の非同期境界

music lab の非同期処理は、mIV 本体の `docs/async-architecture.md` と同じく
「UI thread は状態反映と少量の GPU upload だけ」に寄せる。本体統合時は lab 固有の
`cpal` 簡易プレイヤーを動画音声 engine へ置き換えるが、worker / channel / cancel の境界は
以下を維持する。

| 境界 | 入力 | 出力 | UI thread の責務 | 本体統合時の接続先 |
| --- | --- | --- | --- | --- |
| probe / decode / timeline analysis worker | path, cancel token, streaming sink | `Probed`, `PartialTimeline`, `Loaded`, `Failed` | 受信を 1 frame 上限件数で処理し、部分解析を merge、該当 row version だけ invalidate | 動画 decode / audio pump 由来の PCM を同じ analysis worker へ渡す |
| streaming playback buffer | decode 済み PCM chunk, seek/play state | `PlaybackSnapshot` (`decoded_secs`, `buffer_ahead_secs`, `underrun_count`) | 右パネルと perf log に表示するだけ。音声出力 callback や VST を直接触らない | 既存 video audio pump / normalize / VST3 PDC 計測へ接続 |
| timeline row raster worker | `TimelineAnalysis`, row index, row seconds, texture cache key, row version | row `ColorImage` | 完成 row を 1 frame 少量ずつ texture upload。古い key / generation / row version の結果は捨てる | mIV 側でも GPU texture は transient cache。DB には保存しない |
| spectrum worker | 再生位置周辺の短い PCM window, sample rate | spectrum bands / piano highlight / compute ms | 最新結果を表示し、pending 中は古い結果を保持。全尺 timeline 完了を待たせない | realtime analyzer として維持。永続化対象外 |

### stale / cancel ルール

- 新しいファイルを開いたら、旧 loader と旧 raster worker に cancel を立て、旧 `LoadMsg` は
  path / generation / key の一致で捨てる。
- `PartialTimeline` は確定済み row texture を即破棄せず、該当 row の `row_version` だけを上げる。
  worker が新しい row を返すまでは古い texture を残し、ロード中の黒待ちを避ける。
- Row 秒数、表示幅、theme、解析結果 pointer が変わる変更は `TimelineTextureCacheKey` を変え、
  raster worker generation を進めて古い結果を破棄する。
- worker result は UI が最後に採用した cache key / row version より古ければ適用しない。
  部分解析と Row 切替が同時に走っても、UI thread 側の採用判定を最終防衛線にする。

### 優先度とペーシング

- loader message は 1 frame あたり上限件数だけ処理する。decode が速くても UI thread で
  `PartialTimeline` を無制限 merge しない。
- row texture upload は 1 frame あたり少数に制限する。Row 10m などの大きな切替では、
  再生カーソル行、可視範囲、近傍 row の順に raster request を出す。
- spectrum worker は全尺解析と独立させる。長尺ロード中でも再生バッファから短い window が取れるなら
  下段 analyzer を動かし続ける。
- `ctx.request_repaint_after(...)` は pending worker / pending raster / 再生中のときだけ使い、
  待ちが無い状態で不要な busy repaint を増やさない。

### 永続化とキャッシュ

- 永続化するのは `TimelineAnalysis` とユーザー補正値だけにする。row texture、spectrum frame、
  piano highlight、playback buffer はセッション内の transient state とする。
- `TimelineAnalysis` の DB record には path / size / mtime / duration / sample_rate / channels /
  `analysis_version` を含める。hit しても `analysis_version` が古ければ worker で再解析する。
- mIV 本体の音声ノーマライズ、VST3 chain、動画の音声モードは playback adapter 側の責務であり、
  `music-core` の timeline analysis 型へ混ぜない。

## 本体統合時の注意

- 解析 / DB / waveform 生成は UI thread で行わない。
- 解析結果は `audio_analysis.db` へ path + size + mtime + duration + sample_rate + channels + analysis_version で保存する。
- 小節頭と BPM は誤検出がある前提で、手動補正を永続化する。
- lab の簡易ビート推定は低信頼度ならグリッドを描かず、本体統合時は
  beat/downbeat 専用エンジンか手動補正 UI に置き換える。
- VST3 有効時の PDC は動画と同じく再生 clock / decoder pacing へ反映する。
