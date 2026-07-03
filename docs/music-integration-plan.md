# 音楽ビュー(music)機能 — 本体 mIV 統合計画

Status: 計画 v1（着手前レビュー用のドラフト）。
実装は **Claude Code**、各インクリメントのレビューは **Codex**（前回=comic 統合と同じ運用、
`docs/comic-integration-plan.md` §0/§9 を踏襲）。仕上げの実機目視は **ユーザーが GUI で手作業**。
対象ブランチ: `master`（lab を初回 `--no-ff` マージ。`crates/music-core` は本体の依存）。

ラボ（`tools/music_lab` + `crates/music-core`、実装 Codex）で作った音声再生・解析・タイムライン
表示機能を、本体 mImageViewer（`C:\home\mimageviewer`、master）へ統合する計画。

> 本書は **契約書**。実装はインクリメント順・受け入れ基準に従う。「全部結合して」を避けるためのもの。
> 差分が出たら本書を先に更新してから実装する。着手前にユーザーがレビュー＆承認する
> （特に §4 設計決定・§7 パリティ表＋配線コントラクト・§8 インクリメント）。
> mIV 側の参照行番号は **目安**（master が並行更新中。関数名で探し実装時に再確認）。

---

## 0. このドキュメントの使い方・運用ルール

- comic 統合（`docs/comic-integration-plan.md`）が「ラボ→本体」で完全パリティを達成した実績が
  あるので、その運用（3原則 + 小インクリメント + モデル項目パリティ表 + 毎回 Codex レビュー）を
  そのまま踏襲する。
- **music 固有の追加**: comic は「状態レスの overlay を1枚焼く」だけだったが、music は
  **再生・デコード・並行ワーカー・共有サブシステム（動画エンジン / VST3 / detached viewer）**が
  絡む。よってパリティ表（§7）に加えて **サブシステム配線コントラクト（§7.9）**をもう1枚持つ。
  バグはモデル項目の付け忘れではなく、配線部（並行処理・再生ライフサイクル）に出る。

---

## 1. 前回（comic / 補正レイヤー）の反省と再発防止の原則

前回までの手戻り＝「一部の機能だけ／違う形で結合された」。原因は **曖昧な指示** と
**網羅リスト・完成定義の不在**。対策（comic §1 の3原則 + music 用の第4）:

1. **再実装しない（再利用する）**: 解析ロジックは `crates/music-core`（egui / cpal / FFmpeg / VST3
   非依存の純ロジック、`rustfft` + `serde` のみ）。mIV から依存追加して**書き直さず**呼ぶ。
   再実装しなければ「違う形」になりようがない。
2. **モデル項目ベースのパリティ表**（§7）で一つずつ消し込む。「一部だけ」を機械的に防ぐ。
3. **小インクリメント＋受け入れ基準＋毎回レビュー**（§8〜9）。各単位で「ラボと同じ見え/音」を
   実機確認 → Codex レビュー → コミット。
4. **（music 固有）サブシステム配線コントラクト**（§7.9）: 再利用できない部分（再生=動画エンジン、
   デコード=FFmpeg、VST3、worker/cancel 境界、grid/viewer ライフサイクル）を別チェックリストで
   網羅。ラボ計画書の「本体統合用の非同期境界」表を本体の実識別子に接続する。

---

## 2. 基本方針: 二段の再利用で最大 de-risk

### 2.1 `music-core` を書き換えず再利用（解析・データモデル）

- `crates/music-core` は **egui 非依存の純ロジック**。公開契約（`crates/music-core/src/lib.rs`）:
  - `analyze_stereo_timeline(stereo_samples: &[f32], sample_rate: u32, config: AnalysisConfig) -> TimelineAnalysis`
    — **インターリーブ stereo f32 PCM を食わせるだけ**。デコーダ非依存（これが後述の de-risk の核）。
  - `TimelineAnalysis { analysis_version, stream, config, bins: Vec<WaveformBin>, beat_grid }`
    （`TIMELINE_ANALYSIS_VERSION = 1`）— キャッシュする確定物。
  - `SpectrumAnalyzer`（FFT plan / window を再利用する常駐アナライザ）、
    `spectrum_analysis_from_stereo_window` / `spectrum_bands_from_stereo_window`。
  - 型: `MediaVisualMode { Video, Music }` / `MusicModeSource { AudioFile, VideoAudioOnly }` /
    `MusicBookmark { id, position_secs, title }` / `MusicTimelineLayout` / `BeatGrid` /
    `EffectChain`（VST3 差し込み境界の trait）。
- **許容される追加**: ロジック書き換えは不可。**純粋・加法的なヘルパー + 単体テスト**は可
  （comic の `scale_scene` 方式）。
- `tools/music_lab`（egui アプリ、158KB の `main.rs`）は **UI 参照実装**。mIV 側 UI は mIV 作法で
  書き直す。**symphonia は本体に入れない**（lab 専用。本体のデコードは FFmpeg。§2.2）。

### 2.2 本体 `VideoPlayer` を再利用（再生・VST3・normalize）

> ⚠️ **2026-07-02 訂正（重要）**: 当初「`VideoPlayer` は音声のみファイルをそのまま再生できる」と
> 記載したが**誤り**だった。`run_decoder`（demux thread、`src/video/decoder.rs:1919`）は
> `input.streams().best(Video)` が None だと `"動画ストリームが見つかりません"` で即エラー終了する。
> 素の音声ファイル（映像トラック無し）は現状の engine では再生できない。**再生前提として
> engine の audio-only 対応が必要**（下記 §2.2.1）。それ以外（audio pump / VST3 / normalize /
> clock の wall・audio フォールバック / position・duration・seek）は既に audio-only を許容する。

#### 2.2.1 engine の audio-only 対応（Inc 3 の前提・実装方針確定 2026-07-02）

engine は **音声側は既に Option 設計**（`audio_setup: Option<AudioSetup>`、`has_audio` で
clock を wall/audio 切替、無音動画で `mark_audio_inactive()`）。**映像側に同じ Option 設計を
入れる**のが確定方針（案A・in-place 統一。ユーザー承認 2026-07-02）:

- `run_decoder` 冒頭で `video_stream` を **Option 化**（None でも即エラーにしない）。video も
  audio も無ければそこで初めてエラー。
- 映像セットアップ（`decoder.rs:1926-2036`: params / HW decode open / src・dst 寸法 / fps / sar /
  interlaced）を **`Option<VideoSetup>` にまとめる**（既存 `Option<AudioSetup>` と対称。
  video 有り = Some、audio-only = None）。
- 映像 decode thread spawn（`2455-2508`）、demux ループの video packet routing、join/cleanup、
  `VideoInfo`（width/height/fps/sar は None 時 0、codec は "none"）、perf ログを **`video_setup`
  の有無で gate**。`video_setup=Some` の分岐は現状と**バイト等価**に保つ（動画リグレッション回避）。
- **統一の効能（Inc 7 と一本化）**: gate を `video_active = video_setup.is_some() && !video_output_disabled`
  にすれば、**「映像トラック無し（audio-only ファイル）」と「Inc 7 の映像 OFF トグル」が同一機構**に
  なる。Inc 7 は video_stream を持つファイルに `video_output_disabled` フラグを立てるだけで、
  映像 decode/描画をスキップして音声継続（＝audio-only と同じコード経路）。
- **リスクと担保**: コア動画経路への in-place 改修なので、① Some 分岐を現状等価に保つ ②
  Codex レビュー（decoder 差分を重点）③ ユーザー実機で「音声が鳴る」＋「既存動画が無傷」を検証。
  実機テストできないため盲目実装。native presenter は audio では使わない（`native_output_config=None`）
  ので presenter の frame 依存問題は回避される。
- 影響ファイル: `src/video/decoder.rs`（run_decoder 1919/1926-2036/2229-2253/2455-2508/demux loop/
  join）、`src/video/mod.rs`（VideoInfo `has_video`、InfoReceived 配線、headless EOF drain、
  audio-only の output 失敗をエラー化）、`src/video/engine/state.rs` + `engine/actor.rs`（下記
  ReadinessLatch 対称化）。clock / audio.rs は変更不要（既に audio-only 許容）。

##### 2.2.1a 設計レビュー反映（2026-07-02、Codex round 1 + 自己調査）

decoder.rs の Option 化だけでは**再生が開始しない/末尾が切れる/音楽ファイルが誤判定される**。
設計レビューで判明した追加の video-mandatory 結合と対応（実装対象に追加）:

- **【P1】engine の ReadinessLatch が FirstFrameReady 必須**（最重要）。
  `engine/state.rs` の `ReadinessLatch::is_ready(has_audio)` は has_audio に関係なく常に
  `first_frame`(=FirstFrameReady) を要求する。FirstFrameReady は表示済み video frame 由来
  （`mod.rs` の `flush_first_frame_ready`）なので、映像 thread の無い audio-only では永久に
  来ない → engine が Buffering から抜けず**再生開始しない**（buffering timeout の抜け道も無い）。
  対応 = `has_audio` と対称に **`has_video` を導入**: `DecoderEvent::InfoReceived` に `has_video`
  を載せ、`EngineActor.has_video`（既定 true で既存経路は不変）を保持、
  `is_ready(has_audio, has_video)` を「has_video のときだけ first_frame 要求 / has_audio のときだけ
  buffer_ready+anchor 要求 / どちらも無ければ false」に。`try_transition_from_buffering` の
  anchor 選択は既存の「has_audio→audio anchor / else→first_frame anchor」で audio-only は
  audio anchor を選ぶ（is_ready 以外は変更不要）。
- **【P1】添付画像(cover art)を video stream と誤認**。MP3/FLAC/M4A の埋め込みジャケットは
  FFmpeg で `AV_DISPOSITION_ATTACHED_PIC` の video stream として見える。`best(Video).is_some()`
  だと**大半の音楽ファイルが「映像あり」と誤判定**され、静止画 1 枚を HW decode しに行く。
  対応 = video stream 選択時に `ATTACHED_PIC` disposition を除外する（= 「timed playable video」
  だけを video とみなす）。除外後に video が無ければ audio-only 扱い。
- **【P1】headless(non-native) EOF が audio drain を待たない**。native 経路（`mod.rs:5688-`）は
  `audio_drained`+`quiet_ticks` で末尾音声を出し切るが、audio-only が通る non-native 経路
  （`mod.rs:5885-`）は `is_eof_reached() && future_frames.is_empty() && latest_renderable.is_none()`
  で**即** EofReached を発火する。audio-only では後者 2 条件が常に true なので、demux が読み切った
  瞬間（＝pump にまだ数秒の buffered audio が残る時点）に停止し**末尾が切れる**。対応 = non-native
  EOF 条件に native と同じ audio-drain gate を足す（has_audio 有効時のみ）。
- **【P2/対応】audio-only で audio output 起動失敗**（`self.audio.is_none()`）は has_video=false と
  重なると playable output ゼロ。Buffering 固着より **open エラー**として表面化させる
  （player error/`DecoderEvent::Failed`）。
- **【P2/留意・実機検証項目】paused-seek / frame-step**: 表示 frame が無い audio-only では seek
  override 解消が video と別経路。seek-while-playing は BufferReady 再 promote で成立見込み。
  frame-step は音声では無意味操作。Inc 3 では seek/一時停止を実機検証し、paused-seek が固着する
  場合のみ追加対応（当面は video と同一 readiness 経路に委ねる）。
- **構造判断**: Codex は `Option<VideoRuntime>`（sender/queue/join を束ねる）を推奨したが、
  「Some 経路をバイト等価に保ち動画リグレッションを避ける」を最優先し、**video routing block
  （最ホットな ~130 行）を字面等価に保つ**ため、video 用 channel は常に生成し、audio-only では
  rx を drop して thread を spawn しない構成にする（`video_stream_idx: Option<usize>` で routing を
  自然に unreachable 化 + seek Flush / EOF send / join の 3 箇所だけ gate）。「死んだ receiver への
  send」は上記 gate と routing block の unreachable 化で封じ、その旨コメントを残す。

**実装完了 (2026-07-02)**: 上記方針で `src/video/{decoder.rs,mod.rs,engine/state.rs,engine/actor.rs}`
を改修。build 緑 + lib 2009 / engine 104 / bin 3109 test 緑 + fmt clean。Codex code review
(round 2) = **P1 なし**、video 経路のバイト等価・engine 対称化・gate・drop 順を確認済み。P2 2 件を
反映: ① attached-pic 除外は `best(Video)` filter だけでなく非 attached-pic stream を find() で
再探索する形に強化 (real video + cover art で real を取り逃さない) ② audio-only は
`displayed_frame_seq==0` を preparing 扱いしない (has_video gate、Paused/Eof での 50ms repaint spin
を回避)。P3 (headless EOF drain が compressed audio_pkt_rx の消費完了までは観測しない) は既存 native
gate と同一制約のため据え置き。実機検証 (音が鳴る / seek / 一時停止 / 既存動画無傷) はユーザー担当。

以下は audio-only engine 対応が入った後に成立する再利用のまとめ:

調査で判明した最重要点: engine を audio-only 対応にすれば **`VideoPlayer` が音声のみファイルも再生できる**。

- `VideoPlayer::open(...)`（`src/video/mod.rs:4031`）は FFmpeg で開き、`EngineActor.has_audio`
  （`src/video/engine/actor.rs:92`）で音声トラック有無を追跡。音声は
  `start_audio_output(clock, audio_rx, dsp_bridge)`（`src/video/audio.rs:586`）→ audio pump が
  **normalize gain → VST3 `process_block`（`dsp_bridge`）→ limiter → cpal ring buffer** を実施
  （`src/video/audio.rs:1096` 以降）。
- 公開メソッド（`src/video/mod.rs`）: `toggle_play` / `set_playing` / `seek(secs)` /
  `position_secs` / `duration` / `volume` / `set_volume` / `is_muted` / `set_muted` /
  `normalize_gain` / `set_normalize_gain` / `set_playback_speed`。
- **帰結**: ラボの cpal 簡易プレイヤーは本体に持ち込まない。音声ファイル再生は
  「**GPU 映像出力を無効化した `VideoPlayer`**」で行い、**再生・seek・音量・normalize・VST3 が
  そのまま手に入る**。ラボ計画書の「本体統合時は cpal を動画エンジンへ置き換える」がこれで確定。
- これにより music 統合の再利用率が comic 並みに上がり、新規実装は「解析ワーカー + 音楽ビュー UI +
  ブックマーク + 動画→音声モード」に絞られる。

---

## 3. 機能の全体像

### 3.1 ラボで完成済み（`music-core` + `music_lab`）
- stereo waveform bin / low-mid-high energy / loudness / 簡易 BPM・beat/bar grid（`WaveformBin`/`BeatGrid`）
- 30〜60秒 1 行の DJ 風カラー波形タイムライン + メトリクスレーン（loudness+bass root / key / vocal hint）
- 108-band spectrum analyzer（`SpectrumAnalyzer`、多解像度 FFT）
- 部分解析ストリーミング（decode 中に約5秒単位で timeline を先出し）

### 3.2 本体でのユーザー要件（2026-07-01 確定）
- **音声ファイルのサムネ**: **音楽アイコン固定**（波形サムネ・代表画像なし＝サムネワーカー不要）。
- **右パネル**: 動画のように **タグ表示・設定部分 ＋ 音楽データの情報表示**（format / 長さ /
  sample rate / channels / bitrate / 埋め込みメタ）。
- **左パネル**: **ブックマーク一覧**（代表サムネ・チャプター表示なし）。**ユーザーが名称設定可能**、
  **ブックマークのインポートあり**（追加/削除/改名/クリックでその位置へジャンプ）。
- **上の情報バー ＋ 下のシークバーは常に表示**（動画の HUD 自動 hide と違い、音楽ビューは常時表示）。
- **VST**: 動画と同様に **上バーで切り替え可能**。
- **動画→音声モード**: 動画再生中に音声モードにしたら **動画の映像をカットして** この音声再生機能
  （タイムライン/スペクトラム）に切り替える。位置・音量・VST 状態は引き継ぐ。

---

## 4. 確定した設計決定（ユーザー承認待ち）

| # | 項目 | 決定 |
|---|---|---|
| D1 | 再生エンジン | **本体 `VideoPlayer` を再利用**（映像出力を無効化した headless 再生）。ラボの cpal プレイヤーは本体に持ち込まない。デコードは **FFmpeg**（symphonia は本体に導入しない）。§2.2 |
| D2 | サムネ | **音楽アイコン固定**。サムネ生成ワーカー無し。`ThumbnailState` は Audio を特別扱いして固定アイコンを描く。 |
| D3 | フルスクリーン表示形態 | 音声ファイルは **native video presenter を使わず**（映像面が無い）、通常 egui viewport に「タイムライン canvas + 上情報バー + 下シークバー + 左右パネル」を描く。**上情報バー・下シークバーは常時表示**（自動 hide しない）。 |
| D4 | 右パネル | **動画アイテムの右パネル経路（タグ chips / ★ / 設定）をミラー** ＋ 音楽情報ブロックを追加。`src/ui_metadata_panel.rs` は現状 **画像のみ**（動画/ZIP/PDF は None、`ui_metadata_panel.rs:475` の注記）なので、動画のタグ/設定がどの経路で出ているか実装時に確認して合わせる（§11）。 |
| D5 | 左パネル | **ブックマーク一覧専用**（音声時のみ有効）。`ui_adjustment_panel.rs` の左パネル geometry（`LEFT_PANEL_WIDTH=292.0`）とタブ機構を流用し、音声用のブックマークタブ/パネルとして描く。代表サムネ・チャプターは出さない。命名/追加/削除/改名/ジャンプ。**インポート/エクスポートは動画の既存機構と同一フォーマットを再利用**（D5.1）。 |
| D5.1 | ブックマーク import/export | **動画のブックマーク機構をそのまま再利用**（ユーザー確定 2026-07-01「フォーマットは動画と同じ」）。フォーマットは純関数モジュール `src/video_bookmarks_parser.rs`（`parse_chapter_text` / `format_chapter_lines`）= `mm:ss タイトル` / `h:mm:ss タイトル`（markdown リンク耐性・ms 精度・秒 floor の互換モード）。import = 動画と同じ一括登録ダイアログ（貼り付け→プレビュー→エラー行表示）、export = クリップボード（`seconds_only` トグル）、現在位置に追加 = 動画の `KeyAction::VideoBookmark`（既定 `B`）と同系。保存も動画のブックマーク保存経路を音声 path key で共有（別テーブルを作らず動画機構に相乗り、`music-core::MusicBookmark` は左パネル表示用の変換型）。 |
| D6 | VST3 | **既存 audio pump のチェーンを共有**。`VideoPlayer::open(...)` に `dsp_bridge` を渡すだけで normalize→VST3→limiter を通る。上バーに VST3 トグル（`overlay_draw.rs` の `NativeOverlayCommand::ToggleVst3Gui` / `NativeTopButtonGlyph::Vst3` を音楽ビューにも）。追加配線はほぼ不要。 |
| D7 | 動画→音声モード | `MediaVisualMode::Music` + `MusicModeSource::VideoAudioOnly`。**同一 `VideoPlayer` の映像面のみ停止/隠蔽**し、音声は継続。位置・音量・VST を引き継ぐ。逆トグルで映像復帰。**状態は記憶しない（セッション中の一時トグル、永続スキーマ無し）**（ユーザー確定 2026-07-01）。**最難関につき単独 Inc（§8 Inc 7）に隔離**。 |
| D8 | 永続化 | **タイムライン解析は永続化しない = in-memory LRU（2026-07-03 方針転換）**。当初は中央 `audio_analysis.db`（SQLite）に `TimelineAnalysis` を保存する設計だったが、**spectrum が再生位置 ±1 秒窓のため全尺 PCM を毎回デコードする**ので、永続キャッシュが節約するのは解析パスだけで実利が薄い（progressive 表示で miss 体験も滑らか）。→ 直近 N 曲（`MUSIC_ANALYSIS_LRU_MAX=6` 件 or `MUSIC_ANALYSIS_LRU_MAX_BINS=150万` bin 予算のどちらか先）だけ `Arc<TimelineAnalysis>` をメモリ保持し、セッション内の A/B 切替を即時にする（`src/app.rs` `music_analysis_lru`）。ブックマークは動画 `video_bookmarks.rs` 経路に相乗り（別テーブル無し、D5.1）。row texture / spectrum frame / playback buffer は transient。 |
| D9 | 解析経路 | **再生と独立した解析ワーカー**が FFmpeg でファイルを PCM decode → `analyze_stereo_timeline`。再生 pacing と非同期。spectrum は playback ring buffer から短窓を tap。UI スレッドで解析しない。 |
| D10 | ロジック | `music-core` を再利用（§2.1）。加法ヘルパーのみ許容。 |
| D11 | マイグレーション | **新機能＝未リリース**。旧 mIV データからの移行は不要（コミットにその旨記載）。ラボ `.music.json` からの取り込みは別途指示があるまで行わない。 |
| D12 | 運用 | 実装 = Claude Code、レビュー = Codex（各 Inc、read-only、同一 Inc は resume）、仕上げ実機 = ユーザー GUI。 |
| D13 | 音量ノーマライズ | **動画と同じ設定に従う**（音声専用トグルは新設しない。既存ノーマライズ設定が ON なら音声も曲ごとに初回スキャンして音量を揃える）（ユーザー確定 2026-07-01）。 |
| D14 | 音声を開く振る舞い | **動画と同じ = ダブルクリックで全画面音楽ビュー ＋ 自動再生**（ユーザー確定 2026-07-01）。 |

---

## 5. アーキテクチャ配置マップ（mIV のどこに何を差すか・実識別子に接続）

### 5.1 GridItem::Audio + 検出
- `src/grid_item.rs:36` の `GridItem::Video(PathBuf)` の隣に **`Audio(PathBuf)`** を追加。
- `src/folder_tree.rs:73` の `SUPPORTED_VIDEO_EXTENSIONS` の隣に **`SUPPORTED_AUDIO_EXTENSIONS`**
  （例: `mp3` / `flac` / `wav` / `m4a` / `aac` / `ogg` / `opus` / `wma`。FFmpeg が開ける範囲で確定）。
- 検出 2 箇所: `src/search_walker.rs:302`（`CandidateKind` に Audio 追加）/
  `src/app.rs:10194` 付近のフォルダ読み込み（`is_video` 分岐の隣に audio 分岐）。
- **要更新の網羅 match**（`GridItem` の exhaustive match）: `grid_item.rs` の
  `has_page_data()`(146) / `is_rating_leaf()`(157) / `name()`(218) / `display_path()`(242) /
  `perf_key()`(321) ＋ `app.rs` 内で `GridItem::Video(_)` を処理する全アーム。
  **方針**: 多くは「Video と同じ扱い」で足りる（実ファイル・ソート・★・タグ facet）。
  ただし「ページを持つ」系（`has_page_data`）は false、編集モード系は無効。

### 5.2 再生 = `VideoPlayer` 再利用（headless audio）
- `fs_video_player(fs_idx) -> Option<&VideoPlayer>`（`src/ui_fullscreen.rs:16102`、
  `FsCacheEntry::Video { player, .. }`）と同じ骨格で、音声用に `VideoPlayer` を作る。
- `VideoPlayer::open(...)` に `native_output_config = None` 相当で **GPU 映像出力を持たせない**
  （映像デコード/アップロードを走らせない）。`dsp_bridge` は VST3 用に渡す（D6）。
- 再生制御は §2.2 の公開メソッドをそのまま呼ぶ。`step_frame` は音声では使わない（seek で代替）。

### 5.3 解析ワーカー + in-memory LRU（UI スレッド外）
- `docs/async-architecture.md` のワーカーテンプレ（`XxxPending { cancel, rx }` + `start_xxx` /
  `poll_xxx` + cancel 3 箇所）で **解析ワーカー**を新設。入力 = path + cancel + `want_analysis` +
  `hit_meta`、出力 = `Probe` / `Timeline`（progressive 部分解析、display 専用）/
  `TimelineComplete { analysis, meta }`（全尺確定・LRU に載せる。`meta` = ワーカー検証済み
  (mtime,size)）/ `Pcm`（spectrum 用）。
- ワーカー内: **FFmpeg で PCM decode → インターリーブ stereo f32 → `analyze_stereo_timeline`**。
  decode = `src/audio_decode.rs`（`decode_audio_file_to_stereo_f32_streaming`、48kHz stereo）。
- **永続化はしない（2026-07-03 方針転換、D8）**。旧 `src/audio_analysis_db.rs`（SQLite）は削除。
  代わりに UI スレッドが `Arc<TimelineAnalysis>` を **in-memory LRU**（`music_analysis_lru`）で保持
  する。LRU キー = path (正規化) + size + mtime。
  `ensure_music_analysis` が **`image_metas` の (mtime,size) で楽観的に LRU を lookup**（UI スレッド
  stat しない）。ヒットならタイムラインを即セット + ワーカーを `want_analysis=false` で起動、miss なら
  `want_analysis=true`。**ワーカーは背景で実ファイルを fresh stat** し、`want_analysis=false` でも
  ヒットに使った `hit_meta` と食い違えば（外部更新）解析し直す（stale ヒット補正、Codex code P2）。
  LRU への挿入キーは**ワーカーが返す検証済み (mtime,size)** を使う（`image_metas` スナップショットが
  stale でも正しい key）。`TimelineComplete` 受信時のみ LRU へ挿入（progressive partial は載せない）。
  `meta=None`（stat 失敗 / size=0）はキャッシュしない。
- UI スレッドは受信を **1 frame 上限件数**で処理し、progressive 部分解析で該当 row version を
  invalidate（`docs/ui-responsiveness.md` §4 準拠）。

### 5.4 音楽ビュー（フルスクリーン）— timeline canvas + 上情報バー + 下シークバー
- 音声フルスクリーンは通常 egui viewport に描く（D3）。構成:
  - **上情報バー（常時）**: ファイル名 / 再生位置・長さ / 音量・ミュート / **VST3 トグル** /
    normalize / Row 秒数切替。動画は `native_presenter/overlay_draw.rs` の `draw_native_top_bar`
    （`:2592`）だが、音楽は native 面が無いので **egui で自前の上バー**を描く（VST3 は
    `NativeOverlayCommand::ToggleVst3Gui` を流用）。
  - **中央 timeline canvas**: row raster worker（下記）が焼いた行画像を 1 frame 少量ずつ
    texture upload。再生カーソル行 → 可視範囲 → 近傍 の順で raster 要求（黒待ち回避）。
  - **下シークバー（常時）**: 再生位置表示 + クリック/ドラッグ seek + ブックマーク位置マーカー。
  - **下段 108-band spectrum**（§5.6）。
- **row raster worker**: `TimelineAnalysis` + row index + row secs + cache key + generation +
  row version → row `ColorImage`。ラボの raster worker をそのまま移植（`docs/async-architecture.md`
  テンプレ）。古い key / generation / row version の結果は UI 採用側で捨てる（最終防衛線）。

### 5.5 右パネル / 左パネル
- **右パネル**（D4）: `src/ui_metadata_panel.rs` の `draw_metadata_panel(...)`（:60）の item-kind
  分岐（:529）に **`GridItem::Audio` アーム**を追加。動画の タグ/★/設定 経路をミラー（実装時に
  動画がどこで出しているか確認、§11）＋ 音楽情報ブロック（format/duration/sr/ch/bitrate/埋め込み
  メタ = FFmpeg avformat の標準メタデータ。**外部ツール名は書かない**、CLAUDE.md ポリシー）。
- **左パネル**（D5）: `src/ui_adjustment_panel.rs` の `draw_adjustment_panel(...)`（:11412）の
  タブ機構（`FullscreenLeftPanelTab::Adjustment` / `::ViewTrim`、:11479-）に **音声用ブックマーク
  パネル**を足す。`LEFT_PANEL_WIDTH=292.0`（:42）geometry 流用。音声時のみ有効、画像編集の
  `can_overlay_edit`（:11503）は Audio では false。
  - **ブックマークのデータ・フォーマット・import/export は動画機構を再利用**（D5.1）:
    `src/video_bookmarks_parser.rs`（`parse_chapter_text` / `format_chapter_lines`）、
    `src/video_bookmarks.rs`（保存）、一括登録ダイアログ、クリップボード export
    （`ExportBookmarksToClipboard { seconds_only }`、`src/app/native_video.rs:2460` 付近）、
    現在位置追加 `KeyAction::VideoBookmark`（既定 `B`）。**音声はこれらを path key で共有**し、
    左パネルは同じブックマークデータを一覧描画するだけ（動画はシークバー/HUD 上に出す差だけ）。

### 5.6 108-band spectrum（下段アナライザ）
- `SpectrumAnalyzer` 常駐 worker。再生位置周辺の短い PCM window を **playback ring buffer から
  取得**（`src/video/audio.rs` の `AudioBuffer` を read-only で覗く経路を用意）。全尺 timeline
  解析の完了を待たない（長尺ロード中でも動く）。

### 5.7 動画→音声モード（映像カット + 引き継ぎ）
- `MediaVisualMode`（現状 **本体未参照**、music-core にのみ存在）を本体で初めて使う。
- 動画フルスクリーン再生中に「音声モード」トグル → **同一 `VideoPlayer` の映像面のみ停止/隠蔽**
  （native presenter の video surface を止める）し、音声継続。音楽ビュー（timeline/spectrum）を
  overlay として描く。逆トグルで映像復帰。
- **未確定（Inc 7 で確定、Codex 設計相談推奨）**: (a) native presenter window を残して egui
  timeline を overlay する案 vs (b) native presenter を畳んで egui viewport の音楽ビューへ切替える案。
  video surface / owner window / DPI / resize の扱いが絡むので、CLAUDE.md「設計判断で複数案あるとき
  Codex に意見」に従い着手前に第二意見を取る。
- **UI 一致の前提（ユーザー確定 2026-07-02、Inc 5 FB）**: 動画↔音声の切替前後で上下バー/パネルが
  視覚的にジャンプしないよう、**動画 HUD と音楽 HUD の描画コードを共通化する**（下記 §5.8）。
  これにより Inc 7 が (a)/(b) どちらの presenter 戦略でも見た目は同一コードで揃う。

### 5.8 動画/音楽 HUD・パネルの描画コード共通化（Inc 5 FB、B 案）

ユーザー実機 FB（倍速/音量/下バーボタン/ブックマークパネルが動画と違う）を受けて、
**動画 native overlay の egui 描画を共有モジュールへ抽出し、動画と音楽ビューが同一コードで描く**
方針を確定（ユーザー選択 2026-07-02）。動画 overlay は既に「描画関数が `metadata` を受け取り
`NativeOverlayCommand` を push する」構造なので、描画とコマンド処理が分離済み＝共有しやすい。

- **共有対象**: コントロール行（頭出し/再生/ループ/前後マーカー/時間/**速度ポップアップ**/ミュート/
  **音量スライダー(dB 表示)**/normalize）・シーク行（バー + マーカー + ホバー）・メタ/タグ/★ パネル・
  ジャンプ/ブックマークパネル・**一括登録ダイアログ**・上バー。音楽側は発行コマンドを player/self
  操作へ翻訳する薄いアダプタを持つ。
- **動画専用（共有しない）**: ホバーサムネプレビュー・タイルモード・Perf グラフ・（動画のみの）
  prev/next file・capture パレット等。音楽側では出さない。
- **ブックマークパネル**: 見た目は動画ジャンプパネルに寄せ、一括登録は動画と同じダイアログを共有
  （IME も動画実装に揃う）。中身は音声にあるブックマークのみ（サムネ/ピン/チャプター欄は出さない）。
- **リスク管理**: released な動画 overlay の改修になるので、小インクリメント（コントロール行 →
  パネル → 一括ダイアログ）に分け、各段で build + test + Codex レビュー。動画側の見た目・挙動を
  バイト等価に保つことを最優先。
- **Inc 7 との関係**: 共通化しておくと Inc 7（動画→音声モード）で映像をカットして音声モードに
  切り替えても HUD/パネルが同一コードで描かれ、切替前後の視覚ジャンプが起きない。

#### 5.8.1 実装 handoff（2026-07-02、動画HUD描画のマップ結果 = 次セッションの着手点）

動画 native overlay の描画構造を調査した結果。共有モジュール（新設 `src/video/hud_drawing/`（仮）
または `src/ui_music_panels.rs` から呼べる `pub(crate)` 群）へ抽出する際の起点。

**すでに独立関数（純 egui・そのまま共有しやすい）**（`src/video/native_presenter/overlay_draw.rs`）:
- `draw_native_top_bar(...)`（:2520 付近）+ `draw_native_top_button(...)`（:1344）+ 各 `draw_overlay_*_icon`
  （play/pause/replay/loop/continuous/skip_to_marker/arrow/save/camera/close/window_toggle/tile/perf/vst3、
  `draw_overlay_button_bg`）。アイコンは全部 painter だけの純関数。
- `draw_native_jump_panel(...)`（:390）+ `draw_native_jump_row(...)`（:586）= 左ジャンプ/ブックマークパネル。
  per-row = サムネ + 種別ラベル(PIN/BM/CH) + 時刻 + タイトル + 編集/削除ボタン。クリックで `Seek`。
- `draw_native_bulk_bookmark_dialog(...)`（:1059、サイズ計算 `native_bulk_bookmark_dialog_size` :890）= 一括登録
  ダイアログ。egui `TextEdit::multiline` + `parse_chapter_text` プレビュー + 登録/エクスポート/全削除。
  **IME は標準 egui TextEdit + `pending_paste` フィールド経由**（presenter 非依存）。
- コマンドは全部 `commands: &mut Vec<NativeOverlayCommand>` に push する分離構造 → 音楽側は同じ関数を
  呼び、発行 command を music の実操作（`music_seek_to`/`add_music_bookmark_at_current`/`delete_music_bookmark`/
  `rename_music_bookmark`/`import_music_bookmarks`/`export_music_bookmarks`）へ翻訳するアダプタを書くだけ。

**モノリシック（inline、要抽出）**: 下 HUD コントロール行（replay/play/loop/continuous/prev-next file/
prev-next marker/capture palette + 右クラスタ: 時間 / 速度ボタン+ポップアップ / mute / normalize /
音量スライダー(dB 表示)）は `src/video/native_presenter/mod.rs` の `NativeEguiOverlay::run` に
**ベタ書き（おおよそ :6184-7587）**。シーク行（バー + マーカー + ホバーサムネ）も同 Area 内 inline。
→ 抽出が最重量。state スナップショット（position/duration/is_playing/volume/muted/playback_speed/
loop_mode/continuous_mode）+ command sink を取る関数へ切り出す。速度ポップアップと音量スライダーは
inline のカスタム描画（標準 egui slider ではない）。

**共有しないバリア（動画専用＝音楽側は無視/省略）**: コンパクション階層（幅でボタン間引き。純関数
`calc_tier(width)` に切り出し、音楽は常に Full か指定 tier）/ `last_drawn_*_rect`（native HWND の
SetWindowRgn 用。関数は rect を返し、呼び出し側が使うか無視するか決める）/ ホバーサムネ texture
（native の wgpu surface 依存。`Option<TextureId>` を渡し音楽は None）/ Perf グラフ / VST3 パネル /
normalize 進捗ダイアログ / 動画のみの prev-next file・capture palette。

**⚠️ command 名は実 enum で要確認**（マップ調査は近似名を報告）。実際は `ToggleLoop`/`ToggleContinuous`/
`JumpMarker{next}`/`NavigateItem{delta}`/`SetPlaybackSpeed`/`SetVolume{volume,persist}`/`ToggleMute`/
`SetRating`/`AddBookmarkAt`/`DeleteBookmark`/`SetBookmarkTitle`/`BulkAddBookmarks`/
`ExportBookmarksToClipboard`/`ClearAllBookmarksForCurrent` 等（`src/video/native_presenter/mod.rs` の
`NativeOverlayCommand` 定義を参照）。

**推奨増分順（各段: build + test + Codex、動画側はバイト等価維持）**:
- **Inc 5c-A（左ブックマークパネル + 一括ダイアログ + IME）✅ 完了（2026-07-02、commit 518c00b6 +
  P2 fix fa714a40、Codex P1/P2/P3 クリア、test 3126 green）**:
  - `draw_native_jump_panel` の本体を `draw_native_jump_panel_body(ui, panel_rect, opts, ...)` に抽出。
    `NativeJumpPanelOptions`（title/empty_text/show_pin_button/show_bulk_button/show_pins/
    show_chapters/show_section_headers/show_thumbnails）で音声=ブックマークのみに切替。動画は
    `VIDEO_JUMP_PANEL_OPTIONS`（全 true）で呼ぶだけ＝バイト等価。`draw_native_jump_row` に
    `show_thumbnail` 追加（false=サムネ列省略・行高 52px・テキスト +12px）。
  - `NativeBulkBookmarkDialog`/`NativeBookmarkTitleEdit` と body/bulk dialog/title editor/size fn を
    `pub(crate)` 化、`overlay_draw` を `pub(crate) mod` 化。
  - 音楽側は `draw_fs_music_bookmarks_panel` → `draw_music_bookmark_ui` に置換。共有 body を音声 opts で
    呼び、独自インライン import 欄を廃止。改名・一括登録は動画と同一の中央モーダル
    （`draw_native_bookmark_title_editor` / `draw_native_bulk_bookmark_dialog`）を使い IME・貼り付けが
    動画実装に揃う（Inc 5 FB ⑤ 解消）。発行 `NativeOverlayCommand` を music 実操作へ翻訳
    （Seek/AddBookmarkAt/DeleteBookmark/SetBookmarkTitle/BulkAddBookmarks/Export/ClearAll）。
  - App フィールド: `music_bookmark_rename/import_open/import_text/export_seconds_only` を撤去し
    `music_bookmark_title_edit` / `music_bulk_bookmark_dialog`（動画と同型）を追加。
  - モーダル表示中は端ホバーパネル非表示 + timeline seek 抑止 + FS ショートカット抑止
    （`music_bookmark_modal_open()`）+ 半透明バックドロップ（`Order::Middle`）で背後クリック吸収 +
    HUD 操作を `interactive=false` で明示停止（多層防御、Codex P2）。
  - **未確定/フォロー**: 中央モーダルの中心決めは (0,0) 原点前提（fullscreen viewport / 埋め込みとも
    `full_rect.min≈(0,0)` を確認済み。検証で万一 window mode でズレたら shared fn に origin を渡す）。
    共有ジャンプ行の "BM" 種別ラベルは音声でも各行に出る（動画と同一 anatomy、冗長だが許容）。
- **Inc 5c-B（下 HUD コントロール行）← 次はここ。最重量・最高リスク**: モノリスを state スナップショット +
  command sink 関数へ抽出。速度ポップアップ・音量スライダー(dB 表示)・各ボタンを共有。動画/音楽の両方が
  呼ぶ。②③解消。現行の暫定 `draw_music_bottom_hud`（`ui_music_panels.rs`）をこの共有版へ置換する。
  - **精査済みの正確な境界（2026-07-02）**: 下 HUD Area は `NativeEguiOverlay::run` 内
    `src/video/native_presenter/mod.rs` **6187–7542**（`if bottom_hud_visible { egui::Area::new("native_video_seek_hud")…show(ctx,|ui|{…}) }`）に
    インライン。この 1 closure に **シーク行（bar + hover サムネプレビュー = ピン/ブックマークの action 付き
    ~6960-7130）**、**コンパクション階層 `CompactionTier`（幅でボタン間引き 6308-6336）**、**左クラスタ 4 グループ
    （replay/play | loop/continuous/prev-next-file | prev-next-marker | capture palette 4 ボタン）**、**右クラスタ
    （time / speed ボタン / mute / normalize / volume dB スライダー + label + limiter indicator）** が全部入り。
    速度ポップアップ本体は別途 5214/6901 付近（`video_speed_popup_open` state）。
  - **動画専用（音楽は無視）**: hover サムネプレビュー + RequestSeekThumbnail、CompactionTier、capture palette、
    prev/next-file、normalize/limiter。→ 抽出関数は `show_*` フラグ + `Option<TextureId>` + tier で gate。
  - **推奨サブ分割（各段バイト等価 + Codex）**: (5c-B1) ✅ **完了（2026-07-02、Codex P1/P2/P3
    クリア、test 3121 green）** 右クラスタの **volume dB スライダー**を `pub(crate)` helper
    `draw_overlay_volume_slider(ui, painter, vol_rect, volume, id, tooltip, &mut last_volume_target)
    -> Option<(volume, persist)>`（`overlay_draw.rs`）へ抽出し動画/音楽で共有。動画側はインライン
    （track + fill + dB tick + click/drag/right-click reset + drag_stopped persist）を丸ごと helper 呼び出しへ
    置換しバイト等価（`volume` の finite シャドウは呼び出し側で維持しラベルへ伝播）。音楽側は暫定
    slider（accent fill + circle handle のみ）を共有版へ置換 → dB 目盛り・0dB 右クリックリセット・
    フェーダーマッピングが動画と一致。App に `music_hud_last_volume_target` を追加（drag 確定用の
    frame 跨ぎ state）。`!interactive`（モーダル中）は dummy target に逃がして自 state を汚さない
    （Codex P3）。→
    (5c-B2) ✅ **完了（2026-07-02、Codex P1/P3 なし・P2 fix 済み、test 3117 green）** speed ボタン +
    プリセット popup を `pub(crate)` helper `draw_overlay_speed_control(ctx, ui, painter, speed_rect,
    text_center_y, playback_speed, button_id, popup_area_id, container_left, container_width, hud_top,
    &mut popup_open, &mut popup_rect_out) -> Option<speed>`（`overlay_draw.rs`）へ抽出。動画は inline
    （button bg + ラベル + 左クリック toggle / 右クリック x1 reset + `PLAYBACK_SPEED_CHOICES` popup +
    外クリック close + HWND rect 記録）を丸ごと helper 呼び出しへ置換しバイト等価（`video_speed_popup_open`
    と `last_drawn_speed_popup_rect` のローカルを `&mut` で渡す）。音楽は暫定の「クリックで巡回」
    （`MUSIC_SPEED_PRESETS` / `format_speed`）を撤去し共有 popup に統一 → ラベルが `format_playback_speed`
    （"x1.5"）に、選択肢が動画と同じ 11 段に揃う。App に `music_speed_popup_open` を追加。popup 位置は
    `hud_rect.left()/width()/top()` を渡す。`!interactive`（モーダル中）は dummy popup_open に逃がす
    （B1 と同じ多層防御）。popup X clamp を正規化して狭幅 panic を防止（Codex P2、正常幅では挙動不変）。→
    (5c-B3) ✅ **完了（2026-07-02、Codex P1/P2/P3 なし、test 3117 green）** 各ボタン
    （頭出し/再生/前後マーカー/ループ/ミュート）を `draw_overlay_*_icon` + `draw_overlay_button_bg` の
    共有 primitive で音楽 HUD を描き直し。7 関数（button_bg / play / pause / replay / loop / speaker /
    skip_to_marker icon）を `pub(super)` → `pub(crate)` に広げる（可視性拡大のみ = 動画は挙動不変）。
    音楽の暫定ローカル（`draw_double_triangle` / `draw_loop_icon` / `draw_speaker_icon` / `btn_bg` closure）を
    撤去。マッピング: 頭出し = replay ↺（seek-to-start+play で動画 replay と同義）、前後ブックマーク =
    skip-to-marker |◀ / ▶|（`markers_present` で淡色）、ループ = active blue bg + loop icon、
    ミュート = speaker icon。背景が動画同様「idle 透明 / hover / active blue」に、アイコンが白系に揃う。→
    (5c-B4) 余力があればコントロール行全体を snapshot+sink 関数へ。**一括抽出は 1350 行 released video の
    バイト等価維持が困難なので必ず段階化**。
- **Inc 5c-C（上バー）**: `draw_native_top_bar` を音楽の上バーに流用（音楽向けにボタン集合を調整。
  Row stepper は音楽専用で足す）。
- **Inc 5c-D（シーク行）**: seek 行を共有関数へ。

残る音楽ビュー暫定 UI（`ui_music_panels.rs` の `draw_music_bottom_hud`）は 5c-B/C/D で共有版へ
順次置換していく。置換完了までは暫定 UI が動く（ユーザー「仮なら可」）。左ブックマーク UI は
5c-A で共有版に置換済み。

---

## 6. 永続化設計（D8 / D11）

- **正本 = in-memory LRU（永続化しない、2026-07-03 方針転換、D8）**:
  - `src/app.rs` の `music_analysis_lru: Vec<(MusicAnalysisKey, Arc<TimelineAnalysis>)>`。
    キー = path（正規化）+ size + mtime。件数上限 `MUSIC_ANALYSIS_LRU_MAX=6` と bin 総数予算
    `MUSIC_ANALYSIS_LRU_MAX_BINS=150万` のどちらか先に達した方で古い方から追い出す。単曲が
    予算超（数時間コンサート ~67 万 bin は入るが桁違いの巨大曲）なら LRU に載せない（小さい
    有用エントリを追い出さない、Codex P2）。`music_analysis_lru_insert_bounded` はユニットテスト
    済み（件数/予算/超過スキップ/move-to-front）。
  - `TimelineComplete`（全尺確定）だけを LRU に載せる。progressive partial は display 専用で
    載せない（途中 prefix を確定版として誤キャッシュしない）。
  - ルックアップは `image_metas`（フォルダスキャン済み (mtime,size)）で楽観的に行い UI スレッド
    stat を避ける（設計 Codex P2）。ただし `image_metas` は stale になり得るので、**ワーカーが
    背景で fresh stat して検証**し、ヒットに使った値と食い違えば解析し直す + LRU 挿入キーは検証済み
    (mtime,size) を使う（外部更新時の stale ヒット補正、code Codex P2）。size 不明は LRU 不使用。
  - **なぜ永続 DB をやめたか**: spectrum が再生位置 ±1 秒窓のため全尺 PCM を毎回デコードする
    ので、永続キャッシュが節約するのは解析パスだけで実利が薄い。progressive 表示で miss 体験
    も滑らか。→ セッション内の A/B 切替を即時にする in-memory LRU で十分。
  - **旧 `audio_analysis.db`（`src/audio_analysis_db.rs`、u16 量子化 BLOB + deflate、削除 UI）は
    撤去（superseded）**。未リリースにつきマイグレーション不要（D11）。旧経緯は git 履歴参照
    （commit b00b26f8/47584c44 で 23x 縮小したが、上記の理由で永続化自体をやめた）。
- **ブックマークは動画機構を再利用**（D5.1、ユーザー確定 2026-07-01「フォーマットは動画と同じ」）:
  保存 = `src/video_bookmarks.rs` の経路を音声 path key で共有（専用テーブルは作らない）。
  import/export フォーマット = `src/video_bookmarks_parser.rs`（`mm:ss タイトル`）。埋め込みチャプター
  からの取り込みは初期スコープ外（動画側にも無いので揃える）。
- **対象範囲**: 実ファイル音声 + 実ファイル動画のみ。動画→音声モード（`VideoAudioOnly`）は動画
  path を key に。**ZIP/PDF 内の音声は対象外**（ユーザー確定 2026-07-01）。

---

## 7. 機能パリティ・チェックリスト（2枚: モデル項目 §7.1-7.8 + 配線コントラクト §7.9）

### 7.1 タイムライン表示（`WaveformBin` ベース）
- [x] 上段 DJ 風カラー波形（`band_energy` / `transient` / `transient_band`）（Inc 3b、実機目視待ち）
- [x] **上段波形を L/R ステレオ表示**（上半分=左ch / 下半分=右ch、2026-07-03 Inc 5c-C）。
  `WaveformBin` に `peak_l/rms_l/peak_r/rms_r` を additive 追加（music-core §2.1、`#[serde(default)]`）。
  mono 素材は L==R で従来どおり対称。L/R が全ゼロの既定/旧 bin は mono にフォールバック描画。
  in-memory 化 (§6/D8) 済みなのでディスク増はゼロ。
- [x] loudness+bass root レーン（高さ=loudness、色=`bass_pitch_class` 五度圏）（Inc 3b）
- [x] key レーン（`key_pitch_class` / `key_confidence`、低 confidence は淡色）（Inc 3b）
- [ ] vocal hint 独立レーン（`vocal_score` / `center_ratio`）（未着手）
- [x] Row 秒数切替（解析キャッシュ再生成せず raster のみ作り直し）（Inc 3b、上バー巡回）
- [x] メトリクスレーンのホバー（レーン名/値/時刻/推定音名）（Inc 3b）
- [x] 再生カーソル行の自動スクロール（手動スクロール中は追従しない）（Inc 3b、follow_playhead）

### 7.2 108-band spectrum（下段）
- [x] 20Hz–18kHz / 約1 semitone 幅、多解像度 FFT（`SpectrumAnalyzer`）（Inc 4、常駐ワーカー）
- [x] 鍵盤ハイライト（相対突出評価、A0–C8 明色/範囲外グレー）（Inc 4、`draw_pitch_keyboard`）
- [x] ホバーで周波数 Hz + 近似音名（Inc 4、`draw_spectrum_hover`）
- [x] 再生位置周辺 ±1s の PCM 窓を tap（Inc 4、案A = 展開済み PCM をスライス。§11 参照）

### 7.3 beat/bar grid
- [x] `BeatGrid` 表示（低 confidence は非表示 or 淡色、`BeatTrackingStatus`）（Inc 3b、draw_beat_grid）
- [ ] 手動 BPM / first beat 補正の永続化（`UserCorrected`）（未着手）

### 7.4 再生制御
- [x] Play / Pause / Stop / seek / volume / mute（`VideoPlayer` メソッド）（Inc 3a/3b）
- [x] 上情報バー常時表示 / 下シークバー常時表示（D3）（Inc 3a/3b）
- [x] Open / D&D 直後の自動再生（全尺解析を待たない）（Inc 3a）
- [x] normalize gain（既存 audio pump）（build_audio_player_for_open で cache 値適用）
- [x] **ループ 3 モード**（Off / 全体 / ブックマーク間）（2026-07-03 Inc 5c-C）。**状態は動画と共有**
  （`settings.video_loop_mode`、ユーザー確定「音声＝映像なし動画」）。音声はチャプター無しなので
  `cycle_loop_mode(has_ch=false)` が自然に Off→全体→ブックマーク間を巡回。`L` キー
  （`KeyAction::VideoLoop` 共有）/ 下 HUD ループボタンで切替。ブックマーク区間ループの境界 seek は
  `tick_music_loop_boundary`（映像 `tick_native_video_loop_boundary` の音声版・egui 経路なので cfg 不問）。
  ⚠️ **旧 `music_loop_enabled`（2 値トグル）は `poll_video` が音声のループ設定を毎フレーム
  `video_loop_mode`（既定 Off）で上書きしていたため実質機能していなかった**（音声ガード追加で修正）。
- [x] **連続再生**（Off / 連続 / 連続+ループ）（2026-07-03 Inc 5c-C）。**状態は動画と共有**
  （`video_continuous_mode`）。下 HUD の連続再生ボタンで巡回。EOF で display 順の次 `GridItem::Audio` を
  `open_fullscreen_from_fs_navigation` で自動再生（`find_next_audio_in_display_order` /
  `handle_music_continuous_eof`、映像 `handle_video_continuous_eof` の音声版）。連続再生中はループ無効。
  seek は全経路を `music_seek_to`（&mut self 化）へ集約し `apply_music_loop_mode` を内包
  （ブックマーク区間ループ target の再計算漏れを防ぐ）。ブックマーク CRUD 後も `apply_music_loop_mode`。

### 7.5 ブックマーク（左パネル、動画機構を再利用 D5.1。5c-A で動画 UI を共有）
- [x] 一覧表示（代表サムネ・チャプターなし）（Inc 5 → 5c-A で `draw_native_jump_panel_body` 共有）
- [x] 現在位置に追加（`KeyAction::VideoBookmark`、既定 `B`）/ 削除 / 改名（中央モーダル改名ダイアログ）/ クリックでジャンプ（Inc 5 / 5c-A）
- [x] インポート（動画と同一の中央モーダル一括登録ダイアログ = `draw_native_bulk_bookmark_dialog`、`parse_chapter_text` + プレビュー。IME・貼り付けが動画実装に揃う）（5c-A）
- [x] エクスポート（一括ダイアログ内、`format_chapter_lines` + `seconds_only` トグル）（Inc 5 / 5c-A）
- [x] シークバー上のマーカー表示（Inc 5、黄色縦線）

### 7.6 右パネル
- [x] タグ chips（画像パネルと同一 UI を再利用 `draw_music_tag_section`）/ ★ レーティング（`get_rating`/`set_rating`）（Inc 5）
- [x] 音楽情報（format/duration/sample rate/channels/bitrate/埋め込みメタ = avformat probe）（Inc 5）

### 7.7 VST3
- [ ] audio pump チェーン共有（normalize→VST3→limiter）
- [ ] 上バーで切替（`ToggleVst3Gui`）/ GUI トグル
- [ ] 動画と同じ plugin state / チェーン管理

### 7.8 動画→音声モード
- [ ] 上バーで音声モードトグル（映像カット）
- [ ] 位置・音量・VST 状態の引き継ぎ（同一 `VideoPlayer`）
- [ ] 逆トグルで映像復帰

### 7.9 サブシステム配線コントラクト（music 固有・バグの巣）
ラボ計画書「本体統合用の非同期境界」表を本体の実識別子へ接続。各行が「ラボと同じ挙動 + mIV 作法」に
なったらチェック。

| 境界 | ラボ | 本体接続先 | チェック観点 |
|---|---|---|---|
| decode/timeline 解析 | loader worker(symphonia) | 新解析ワーカー(FFmpeg→`analyze_stereo_timeline`) | [x] UI スレッド非同期 / cancel(新ファイル+close) / path 一致で stale 破棄（Inc 3b。部分解析 merge は未実装=miss は全尺待ち） |
| 再生 | cpal 簡易プレイヤー | `VideoPlayer`(headless) + audio pump | [x] seek/volume/mute/normalize が動く / 映像 decode を走らせない（Inc 3a、engine audio-only） |
| VST3 | `NoopEffectChain` | `DspBridge`(audio pump 既存) | [ ] realtime callback で直接呼ばない / 失敗時 auto-disable / 動画と同一 state（Inc 6） |
| row raster | raster worker | 移植 raster worker | [x] cache key+generation+row version / 1 frame 少量 upload / 古い結果を採用側で破棄（Inc 3b） |
| spectrum | spectrum worker | 常駐 spectrum worker + 展開済み PCM スライス | [x] 全尺解析と独立の常駐ワーカー / pending 中は旧 bands 保持 / 高々 1 in-flight で coalesce（Inc 4、案A。ring buffer では ±1s 窓に足りず PCM をスライス、§11） |
| repaint | — | `request_repaint_after` | [x] pending worker は request_repaint_after(50ms) / 再生中は VideoPlayer が駆動（Inc 3b、busy spin 回避） |
| grid/viewer lifecycle | — | GridItem::Audio / fs_cache / detached | [x] 新ファイルで旧 worker cancel / close_fullscreen で worker+raster cache 破棄（Inc 3b） |

---

## 8. インクリメント分割（独立してビルド・テスト・実機確認できる単位）

各 Inc の完了条件: **ビルド緑 ＋ テスト緑 ＋ 実機でラボと同じ見え/音 ＋ Codex レビュー P1/P2 なし ＋
コミット**。受け入れを満たさなければ次へ進まない。リスクの高い「動画→音声モード」は最後に隔離。

- **Inc 0: lab→master マージ + 依存配線 + アセット方針**
  - `lab`→`master` を `--no-ff` マージ（`crates/music-core` + `tools/music_lab` + 統合 docs）。
    lab は本体 `src/`/`build.rs`/`vendor/` 不変（`merge-tree` dry-run で確認、comic Inc0 と同じ）。
  - `Cargo.toml` に `music-core = { path = "crates/music-core" }`（symphonia は本体に足さない）。
  - 音楽アイコンのアセット方針決定（内蔵グリフ or 同梱 PNG。グリフ lint 対象）。
  - 受け入れ: `cargo build --bin mimageviewer-core` 緑 / 既存テスト緑 / music-core テスト緑 / `cargo fmt --check` clean。

- **Inc 1: GridItem::Audio + 検出 + 固定アイコンサムネ**
  - `GridItem::Audio` 追加、`SUPPORTED_AUDIO_EXTENSIONS`、検出 2 箇所、全 match アーム更新（§5.1）。
  - グリッドで音楽アイコン固定表示（サムネワーカー不使用、`ThumbnailState` 特別扱い）。
  - 受け入れ: 音声ファイルが一覧に音楽アイコンで出る / ソート・選択・★・タグ facet が Video 同等に動く。
    ダブルクリックは Inc 3 まで no-op で可。

- **Inc 2: 解析データ層（decode + `analyze` + `audio_analysis.db`）** — ✅ **実装済み（2026-07-02）**
  - ✅ `src/audio_decode.rs`: FFmpeg (avformat + avcodec + swresample) で 1 ファイルを全尺
    デコードして 48kHz interleaved stereo f32 PCM を作る（`decode_audio_file_to_stereo_f32`）。
    packed f32 抽出手順は `video/decoder.rs` の実績実装を踏襲。`analyze_audio_file` が
    decode → `analyze_stereo_timeline` を合成。
  - ~~`src/audio_analysis_db.rs`: `TimelineAnalysis` の SQLite キャッシュ~~ **→ 撤去（2026-07-03、
    §6/D8 参照）**。当初は path+size+mtime+`analysis_version` の SQLite キャッシュだったが、
    永続化自体をやめて in-memory LRU（`music_analysis_lru`）に置き換えた（spectrum が全尺 PCM を
    毎回デコードするので永続キャッシュの実利が薄い）。以降の Inc 3b/4/5 の記述で
    `audio_analysis_db` 参照/保存とあるのは、この LRU 経路に読み替える。
  - ✅ **決定性テスト**: 固定合成 PCM → `analyze_stereo_timeline` が安定（bins/version 一致）。
    DB roundtrip / stale / version-gate / 壊れ JSON テスト計 7 本。
  - **📌 増分境界の調整（2026-07-02）**: 当初 Inc 2 に含めた「解析ワーカー（背景スレッド +
    cancel + poll）＋ トリガ」は **Inc 3 へ移動**した。理由 = 解析の起動契機は「音楽ビューを
    開いたとき」で、その consumer（タイムライン表示）と open 経路は Inc 3 で作るため。フォルダ
    閲覧のたびに全音声を pre-decode するのは無駄なので Inc 2 では起動しない。Inc 2 は
    「decode + analyze + DB」の**データ層**（呼べば動く純関数群）に閉じる。
  - ⚠️ FFmpeg decode の**実動作は実機検証項目**（DLL + 実ファイルが要る）。コンパイル・API
    整合は確認済み、解析と DB は機械なしでテスト済み。
  - 受け入れ（データ層）: build 緑 + DB/決定性テスト緑。ワーカー化・「開くと解析」・UI 応答性は
    Inc 3 で担保する。

- **Inc 3: 音楽ビュー骨格 — `VideoPlayer` 再生 + timeline canvas + 上情報バー/下シークバー常時**
  - 音声フルスクリーンで `VideoPlayer`（headless）を作り自動再生。egui viewport に timeline canvas
    （row raster worker、部分解析対応）+ 上情報バー常時 + 下シークバー常時。play/pause/seek/volume/mute。
  - **解析ワーカー（Inc 2 から移動）**: 音楽ビューを開いた時に `audio_decode::analyze_audio_file`
    を背景スレッド（cancel + mpsc + `poll_*`）で走らせ、`audio_analysis_db` を参照/保存する
    (`docs/ui-responsiveness.md` §2 テンプレ)。cache hit は即表示、miss は解析完了まで待つ。
    新ファイルで旧ワーカー cancel。UI スレッド同期 I/O なし（perf 計装）。
  - 受け入れ: 音声を開くと自動再生 + timeline 表示 + seek/play/pause + 上下バー常時。ラボと同じ見え/音。

  **✅ Inc 3b 実装済み（2026-07-02、commit b89d779a + Codex 修正 7575071c）**:
  - **解析ワーカー**: `src/app.rs` に `MusicAnalysisPending` + `ensure_music_analysis` /
    `poll_music_analysis` / `cancel_music_analysis`。`run_music_analysis`（背景スレッド）が
    `std::fs::metadata` → `audio_analysis_db` 参照 → miss なら FFmpeg decode + analyze + DB 保存を
    **全て UI スレッド外**で行う。cache hit は 1 poll で表示、新ファイルで旧ワーカー cancel、
    `close_fullscreen` で worker + raster cache を破棄。解析 config は `bin_secs=0.010`
    （ラボと同じ、`audio_decode::analyze_audio_file_with_config`）。
  - **row raster worker + DJ 波形描画**: `src/ui_music_timeline.rs`（新規）にラボの
    `TimelineTextureCache` + raster worker + `render_timeline_row_image`（spectral 波形 /
    transient アクセント / Loudness+Bass root・Key メトリクスレーン / beat grid / playhead）+
    key/bass root 検出 + color helper を移植。`draw_music_timeline` は `ScrollArea` 子 UI 向けに
    適応し seek 要求を返す。**Codex P2 対応**: raster request は `Arc<TimelineAnalysis>` を渡し、
    行ウィンドウ切り出し（`timeline_bins_window_range`）はワーカー側で zero-copy 実施
    （UI スレッドで数千 bin をコピーしない）。`App.music_analysis` も `Arc` 保持。
  - **`draw_fs_music_view` 統合**: 解析が揃えば中央領域に `ScrollArea` + timeline
    （縦スクロール・follow-playhead・クリック/ドラッグ seek）、未了は音楽アイコン + 「解析中」。
    上情報バーに Row 秒数切替（クリック巡回、`next_row_secs`）を追加、上下バー常時維持。
  - **Codex 修正 (7575071c)**: P2 = 上記 Arc 化。P3 = spawn 失敗 / worker 切断で
    `music_analysis_error` をセット（「解析中」固着回避）、pending 中の repaint は
    `request_repaint_after(50ms)`（busy spin 回避）。
  - **据え置き（意図的、文書化）**: (a) raster FIFO 優先度 — 1 曲の row 数は 10〜30 と小さく
    各 raster も小画像のためサムネグリッド規模の問題にならない。(b) 解析フェーズの
    キャンセル — 支配的な全尺 decode は既にキャンセル可、`analyze_stereo_timeline` を
    cancel 対応にすると music-core の書き換えになるため（§2.1 で禁止）。
  - **⚠️ 実機未検証**: 実際の音が鳴る中でのタイムライン表示 / seek 追従 / Row 切替は
    ユーザーの GUI 目視待ち（FFmpeg DLL + 実ファイルが要る）。ビルド + 単体テスト緑 + fmt clean +
    Codex code review (P1/P2/P3 なし) 済み。
  - **部分解析ストリーミング（先出し）**: ✅ **実装済み（2026-07-03、Inc 5c-C）**。cache miss 時、
    `decode_audio_file_to_stereo_f32_streaming` がデコード進行に合わせて蓄積プレフィックス PCM を
    `on_partial` で渡し、`run_music_analysis` が `analyze_stereo_timeline(prefix)` を **幾何級数
    スケジュール**（初回 ~2 秒 → 倍々、wall-clock 150ms throttle、全長 50% 超で抑制）で先出し。
    poll が `Timeline` を置換するので波形が順次埋まる。x 軸幅は player duration（→ probe → 解析）
    基準で安定。timeline cache は解析 `Arc` identity 変化時に全 `row_version` を進めて既存 tile を
    再ラスタ（playhead→可視の優先描画はそのまま再利用）。partial 後に decode 失敗しても波形は消さ
    ない。music-core は無改変。Codex 設計レビュー（P1×1 / P2×4 / P3×2 すべて反映）+ code review 済み。
  - **残り（Inc 3 の未実装）**: vocal hint 独立レーン / 手動 BPM 補正は未着手。

- **Inc 4: 108-band spectrum worker（下段）** — ✅ **実装済み（2026-07-02）**
  - `SpectrumAnalyzer` 常駐 worker + 展開済み PCM の playhead ±1s スライス（案A、§11 で確定）。
  - ✅ `src/ui_music_spectrum.rs`（新規）: `MusicSpectrumState`（常駐ワーカー + 描画状態を封じる）+
    `MusicPcm`（Arc 共有 PCM）+ ラボの `draw_spectrum` / `draw_pitch_keyboard` / spectrum color helper
    移植。窓切り出し（`spectrum_window_range`）はワーカー側でゼロコピー。高々 1 in-flight + coalesce。
  - ✅ `run_music_analysis`（app.rs）: 全尺 PCM を **1 回だけ**デコードして timeline と spectrum で共有。
    `MusicAnalysisMsg::{Timeline, Pcm}` の 2 メッセージ化（cache hit は timeline 即送 → PCM 追送）。
    `poll_music_analysis` は Disconnected まで drain。close/新ファイルで `music_spectrum.clear()` + `music_pcm=None`。
  - ✅ `draw_fs_music_view`: 中央領域の下端に spectrum 帯（180px）を確保、`update` + `draw` を配線。
  - ✅ ビルド緑 + bin test 3122 緑（spectrum 単体 7 本）+ fmt clean + clippy 新規警告なし。
  - **⚠️ 実機未検証**: 実際に音が鳴る中でのスペクトラム挙動 / 鍵盤ハイライト / ホバーはユーザー GUI 目視待ち
    （FFmpeg DLL + 実ファイルが要る）。テスト音源 = `c:\home\youtube\movie\youtube\audio\`（115 ファイル）。
  - 受け入れ: 下段アナライザがラボと同じ挙動。長尺ロード中でも（PCM デコード完了後は）全尺 timeline 解析を待たない。

- **Inc 5: 右パネル（タグ+設定+音楽情報）+ 左パネル（ブックマーク）**
  - 右: 動画のタグ/★/設定経路ミラー + 音楽情報ブロック。左: ブックマーク一覧（命名/追加/削除/改名/
    ジャンプ/インポート、シークバーマーカー）。ブックマーク永続化。
  - 受け入れ: 右にタグ+設定+音楽情報、左にブックマーク一覧、命名/ジャンプ/インポート動作。

  **✅ Inc 5 実装済み（2026-07-02）**:
  - **音楽情報プローブ**: `src/audio_decode.rs::probe_audio_file` を新設。デコード
    (`decode_audio_file_to_stereo_f32`) は 48kHz stereo に正規化してしまい `TimelineAnalysis.stream`
    がソース値を持たないため、別途 avformat の軽量ヘッダ読みで container / codec / 実 sample rate /
    channels / bitrate / duration / 埋め込みメタ (title/artist/album/... を curated 順) を取る
    (`AudioProbe`)。解析ワーカー (`run_music_analysis`) が decode より先に `MusicAnalysisMsg::Probe`
    で追送し、`App.music_probe` に保持 (ファイル変更 / `clear_music_view_state` で破棄)。
  - **右パネル** (`src/ui_music_panels.rs::draw_fs_music_right_panel`): 音楽情報セクション + ★
    レーティング (`get_rating`/`set_rating`、音声は `is_rating_leaf`。同じ★再クリックで解除) +
    タグセクション。タグは**画像メタデータパネルと同一 UI を再利用**するため
    `ui_metadata_panel.rs` に `App::draw_music_tag_section` を追加し、`draw_tag_panel` /
    `draw_fullscreen_tag_picker_panel` / `collect_fullscreen_tag_panel_rows` /
    `fullscreen_tag_picker_*` state をそのまま共有 (IME 扱いも画像と同一)。トグルは画像メタパネルと
    同じ `show_metadata_panel`（上バーの "i" ボタン / 右端ホバー）。
  - **左パネル** (`draw_fs_music_bookmarks_panel`): 動画の `VideoBookmarkDb` を path キーで共有
    (D5.1、別テーブルを作らない)。一覧 (時刻クリックでジャンプ) / ＋現在位置 / インライン改名
    (lost_focus で確定、IME 安全) / 削除 / インポート (貼り付け欄 + `parse_chapter_text` プレビュー +
    一括 add) / エクスポート (`format_chapter_lines` + 秒単位トグル)。上バー左のリボンボタンで開閉。
  - **シークバーマーカー**: 現在ファイルのブックマークを黄色縦線で表示 (左パネルが閉じていても
    `ensure_music_bookmarks_loaded` で常時ロード)。
  - **B キー**: `handle_video_input` は `GridItem::Video` 限定なので、音声は `handle_fs_key_input` に
    `KeyAction::VideoBookmark`（既定 `B`）分岐を追加。TextEdit フォーカス中 (`wants_keyboard_input`) /
    IME 中 / コンテキストメニュー中は無効化。
  - **パネルレイアウト**: 上情報バーの下〜再生コントロールの上の帯にパネルを描き、中央の
    タイムライン/スペクトラムはパネル幅ぶん横に縮める (クリック競合回避)。上バー・下シークバーは全幅維持。
  - build 緑 + bin test 3126 緑 (ui_music_panels helper 4 本) + fmt clean + glyph lint clean。
  - **Codex レビュー完了 (5 ラウンド、P1 ゼロ)**: 反映した指摘 —
    (P2) 音声★を一等市民化 (`RatingItemKind::Audio`=db 9 / `rating_meta_for_idx` /
    `item_from_kind` / `item_from_plain_path` 拡張子 / `source_path_for_item` /
    `rating_source_path` = レーティング一覧表示 + undo capture)。
    (P2) 音声フルスクリーンで画像系ショートカット (分析/消しゴム/隠蔽/テキスト/回転/見開き 1-7,0/
    補正スロット Ctrl+1..0/clear/比較/AI/デノイズ/ポストフィルタ/pin/capture/book/export/
    slideshow/space/bg/spread-shift Ctrl+←→) を `!current_item_is_audio` で consume 抑止
    (状態が次画像へ漏れる + ブックマーク改名/インポート/タグ TextEdit への文字入力奪取を防止)。
    `I` (音楽情報パネル) と `B` (ブックマーク) は音声でも有効に残す。
    (P3) probe 失敗時に右パネルが「取得しています…」で固着しない (worker 終了後は「取得できません
    でした」)。(P3) インポートは成功時のみ欄をクリア/クローズ、DB 失敗ではテキストを残しエラー通知。
  - **⚠️ 実機未検証**: 実ファイル再生中のタグ編集 / ★ / ブックマーク追加・ジャンプ・改名・
    インポート/エクスポート / 音楽情報表示はユーザーの GUI 目視待ち（FFmpeg DLL + 実ファイル要）。
  - **未リリース = マイグレーション不要**（新機能。動画ブックマーク DB へ path キーで相乗り）。
  - **据え置き**: 「動画→音声モード」の右左パネル (Inc 7 で `VideoAudioOnly` を通す)。

- **Inc 6: VST3 共有 + 上バー切替**
  - `VideoPlayer::open` に `dsp_bridge` を渡す（既存 pump が VST3 適用）。上バーに VST3 トグル + GUI トグル。
  - 受け入れ: 音声再生で VST3 チェーンを通る / 上バー切替 / GUI トグル。動画と同じ挙動。
  - 注: audio pump は既に VST3 対応なので配線確認が主。

- **Inc 7: 動画→音声モード（映像カット + 引き継ぎ）— 最難関、単独隔離**
  - 着手前に §5.7 の 2 案を **Codex に設計相談**。native presenter video surface の停止/隠蔽 +
    egui 音楽ビュー共存 or 切替。`MediaVisualMode::Music` / `MusicModeSource::VideoAudioOnly`。
  - 受け入れ: 動画再生中に音声モード切替で映像カット + 音楽ビュー、位置/音量/VST 継続、復帰可。

- **Inc 8: 仕上げ・回帰（docs / manual / product page）**
  - `spec.md` / マニュアル新ページ / `index.html` / `video-architecture.md` 追記（音楽ビュー節）/
    `async-architecture.md`（解析・raster・spectrum ワーカー追加）/ keymap（音楽モードのキー）/
    グリフ lint / snapshot 判断。
  - 受け入れ: docs 同期、回帰緑。UI 応答性/IME/律速は Inc 2/3/4 で都度担保（Inc 8 に溜めない）。

---

## 9. 各インクリメントの作業ループ

1. 該当 Inc の受け入れ基準を確認（必要なら本書を更新）。
2. 実装（`music-core` は再利用 + 加法ヘルパーのみ。mIV 側 UI/配線）。
3. `cargo build --bin mimageviewer-core` / `cargo test` / `cargo fmt`。
4. **ユーザーが実機で目視確認**（ラボと同じ見え/音か）。
5. **Codex レビュー**（read-only、同一 Inc は `codex exec resume --last`）。P1/P2 対応。
   基準コミットは「前回 Codex レビュー地点より後」（CLAUDE.md「Codex CLI レビュー」節）。
6. コミット（pathspec、`Codex P<N> 対応` 記載）。§7 を消し込み。

---

## 10. mIV 制約の遵守チェックリスト

- **UI スレッド同期 I/O 禁止**（`docs/ui-responsiveness.md` §4）: 解析デコード・DB open・row raster・
  PCM 読み込みは worker 化 + cancel + 1 frame 予算 + perf 計装。**Inc 2/3/4 で都度担保**。
- **read_dir は `entry.file_type()`**（音声拡張子判定でも per-entry `is_file` を呼ばない）。
- **並行処理**: `try_lock + sleep` 禁止（`Mutex + Condvar` or mpsc）。新ワーカーは async-architecture テンプレ。
- **IME**: パネル内 TextEdit（ブックマーク改名等）の Enter/Escape は `dialog_enter_pressed` /
  `dialog_escape_pressed` 経由。新ビューポート入口で `update_ime_state`。
- **ダイアログ**: `default_pos`（`anchor` 禁止）、`.open(&mut open)`。
- **Unicode グリフ**（`scripts/check_ui_glyphs.py`）: 固定 UI 文言・音楽アイコンに環境依存グリフ禁止。
- **スナップショットテスト**（`docs/ui-snapshot-policy.md`）: 純描画関数化できた部分は対象化を検討。
- **永続キー**: 実ファイル path ベース（ZIP/PDF 内音声は初期スコープ外）。
- **未リリース機能**: 新規＝マイグレーション不要。コミットにその旨記載。
- **外部ツール名の非言及**（CLAUDE.md）: チャプター/メタは「FFmpeg avformat の標準メタデータ」と書く。

---

## 11. 未確定の実装詳細（該当 Inc で確定）

- ~~**解析デコード経路**（Inc 2）~~: **確定**。軽量 avformat/avcodec decode を新設
  （`src/audio_decode.rs`）。全尺 decode のメモリと部分解析ストリーミングの両立は Inc 5c-C
  （2026-07-03）で決着: **1 回のストリーミングデコード**が最終 PCM を蓄積しつつ、幾何級数
  マイルストーンで蓄積プレフィックスを都度解析して先出しする（二重デコードなし。partial の
  再解析総コストは全長 50% 抑制 + 倍々で約 2x 以内）。
- **`SUPPORTED_AUDIO_EXTENSIONS` の確定**（Inc 1）: FFmpeg LGPL build が確実に開ける範囲。
- **右パネルのタグ/設定経路**（Inc 5 / §11）: 動画アイテムがタグ chips / ★ / 設定をどこで描いているか
  （`ui_metadata_panel.rs` は画像のみなので別経路の可能性）。実装時に確認して音声もミラー。
- ~~ブックマークのインポートフォーマット~~: **確定** — 動画機構（`video_bookmarks_parser.rs` /
  `video_bookmarks.rs`）をそのまま再利用（D5.1）。新規フォーマット判断は不要。
- **動画→音声モードの presenter 戦略**（Inc 7 / §5.7）: native presenter 残置 overlay 案 vs 切替案。
  着手前に Codex 設計相談。
- ~~**playback ring buffer tap の口**（Inc 4）~~: **確定（2026-07-02、案A・ユーザー承認）**。
  調査の結果、スペクトラムは再生位置周辺 **±1 秒（2 秒幅・約 96k サンプル）** の PCM 窓を必要とする
  （多解像度 FFT の最大窓 32768）。一方 `src/video/audio.rs` の `AudioBuffer.processed` は約 100ms 分
  しか保持しないため **cpal ring buffer tap は窓幅が全く足りず不成立**。ラボ実装も実は cpal ring
  buffer を tap しておらず、**ストリーミング展開した全 PCM を playhead 周辺でスライス**していた
  （`spectrum_request_from_samples`）。よって本体でも **解析ワーカーが全尺デコードした 48kHz stereo PCM を
  `Arc<MusicPcm>` で保持し、playhead ±1s をスライスして `SpectrumAnalyzer` に渡す**（`audio.rs` の
  hot path は無改変 = 動画リグレッションリスクゼロ）。D9 の「playback ring buffer から短窓 tap」は
  この記述に読み替える。トレードオフ = 開いている 1 曲分の PCM が常駐（約 12MB/分）。**当初は
  `MUSIC_SPECTRUM_MAX_PCM_SAMPLES` = 30 分の常駐上限を設け、超過ファイルは spectrum を無効化して
  いたが撤廃した（2026-07-03）**: timeline はそもそも上限なしで全尺デコードしており（この関数の
  支配的コスト）、spectrum 用 PCM はその Vec を `move` で渡すだけで追加のピーク確保が無い。上限が
  あると「timeline は解析できるのに 30 分超で spectrum だけ黒枠」という不整合（実機 FB で NG）を
  生むため、timeline と同じく全尺 PCM を長さに依らず渡す（ラボと同挙動）。巨大ファイルでデコード
  自体が確保失敗した場合は `decode_audio_file_to_stereo_f32` の `try_reserve` が Err を返し
  timeline/spectrum とも出ない（決定的な失敗）。spectrum ワーカーは playhead ±1s 窓をスライスする
  だけ（O(窓)）で常駐 PCM 全体を毎フレーム走査しない。cache hit 時は timeline は即表示のまま、
  spectrum 用 PCM だけ背景デコードで後追い（mIV は既に timeline が全尺デコード待ちのため挙動は一貫）。実装 = `src/ui_music_spectrum.rs`（`MusicSpectrumState` / `MusicPcm` / 常駐ワーカー）+
  `run_music_analysis` の PCM 追送（`MusicAnalysisMsg::{Timeline,Pcm}`）。
- **音量 normalize と音楽ビューの関係**（Inc 3/6）: 動画と同じ normalize スキャンを音声にも適用するか。

---

## 12. 参照ドキュメント

### ラボ側（機能の正本）
- `C:\home\mimageviewer-music-lab\docs\music-lab-plan.md`（データ契約・非同期境界・解析エンジン方針）
- `music-lab-vocal-eval.md` / `music-lab-validation-checklist.md`（ラボ実機検証）
- `crates/music-core/src/lib.rs`（公開契約）

### mIV 本体側（統合先の作法）
- `docs/architecture-overview.md` / `docs/async-architecture.md`（ワーカーテンプレ）/
  `docs/ui-responsiveness.md`（§4 チェックリスト）
- `docs/video-architecture.md`（動画エンジン・native presenter）/ `docs/vst3-integration.md`
- 再生: `src/video/mod.rs`(`VideoPlayer::open`) / `src/video/audio.rs`(`start_audio_output` / audio pump) /
  `src/video/dsp/bridge.rs`(`DspBridge`) / `src/video/engine/actor.rs`(`EngineActor.has_audio`)
- HUD/上バー: `src/video/native_presenter/overlay_draw.rs`(`draw_native_top_bar` / `ToggleVst3Gui`)
- グリッド/検出: `src/grid_item.rs` / `src/folder_tree.rs` / `src/search_walker.rs` / `src/app.rs`
- パネル: `src/ui_metadata_panel.rs`(右) / `src/ui_adjustment_panel.rs`(左タブ) / `src/ui_fullscreen.rs`(`fs_video_player`)
- 前例（編集モード + 永続化の近い形）: `docs/comic-integration-plan.md`（本書の運用の親）
