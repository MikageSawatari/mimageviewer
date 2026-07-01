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

### 2.2 本体 `VideoPlayer` を再利用（再生・VST3・normalize）— **最大の de-risk 発見**

調査で判明した最重要点: **本体の `VideoPlayer` は既に音声のみファイルを再生できる**。

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
| D8 | 永続化 | 正本 = 中央 `audio_analysis.db`（SQLite）。`TimelineAnalysis` + `BeatGrid` + ブックマーク + ユーザー補正を保存。row texture / spectrum frame / playback buffer は transient（保存しない）。 |
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

### 5.3 解析ワーカー + `audio_analysis.db`（UI スレッド外）
- `docs/async-architecture.md` のワーカーテンプレ（`XxxPending { cancel, rx }` + `start_xxx` /
  `poll_xxx` + cancel 3 箇所）で **解析ワーカー**を新設。入力 = path + cancel、出力 =
  `Probed` / `PartialTimeline`（約5秒単位の部分解析）/ `Loaded`（全尺確定）/ `Failed`。
- ワーカー内: **FFmpeg で PCM decode → インターリーブ stereo f32 → `analyze_stereo_timeline`**。
  decode は `src/video/decoder.rs` の FFmpeg 経路を「音声全尺 decode-to-PCM」モードで流用するか、
  軽量 avformat/avcodec decode を新設（**Inc 2 で確定、§11**）。
- 永続化 = `src/audio_analysis_db.rs`（新設）。key = **path + size + mtime + duration +
  sample_rate + channels + `analysis_version`**（ラボ計画の DB key 契約）。hit しても
  `analysis_version` が古ければ再解析。
- UI スレッドは受信を **1 frame 上限件数**で処理し、部分解析を merge、該当 row version だけ
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

---

## 6. 永続化設計（D8 / D11）

- **正本 = `audio_analysis.db`**（`src/audio_analysis_db.rs`、新設）:
  - `analysis` テーブル: key（path/size/mtime/duration/sample_rate/channels/analysis_version）+
    `TimelineAnalysis` JSON（`BeatGrid` 含む）。`PRAGMA user_version` + JSON `analysis_version`。
  - `bookmarks` テーブル: `MusicBookmark { id, position_secs, title }` を音声 path key ごとに保持。
    ユーザー命名・並べ替え・削除・改名。
  - 壊れ JSON / no-row = 空扱い（クラッシュさせない、comic §6.1 と同流儀）。
- **キー生成**: 実ファイルは path そのもの。動画→音声モード（`VideoAudioOnly`）は動画 path を key に。
  **ZIP/PDF 内の音声は対象外**（初期スコープ = **実ファイル音声 + 実ファイル動画のみ**。ユーザー確定 2026-07-01）。
- **未リリース = マイグレーション不要**（D11）。開発中に溜まったテスト DB は手動削除で足りる。
- **ブックマークは動画機構を再利用**（D5.1、ユーザー確定 2026-07-01「フォーマットは動画と同じ」）:
  保存 = `src/video_bookmarks.rs` の経路を音声 path key で共有（`audio_analysis.db` に別テーブルは作らない）。
  import/export フォーマット = `src/video_bookmarks_parser.rs`（`mm:ss タイトル`）。埋め込みチャプター
  からの取り込みは初期スコープ外（動画側にも無いので揃える）。

---

## 7. 機能パリティ・チェックリスト（2枚: モデル項目 §7.1-7.8 + 配線コントラクト §7.9）

### 7.1 タイムライン表示（`WaveformBin` ベース）
- [ ] 上段 DJ 風カラー波形（`band_energy` / `transient` / `transient_band`）
- [ ] loudness+bass root レーン（高さ=loudness、色=`bass_pitch_class` 五度圏）
- [ ] key レーン（`key_pitch_class` / `key_confidence`、低 confidence は淡色）
- [ ] vocal hint 独立レーン（`vocal_score` / `center_ratio`）
- [ ] Row 秒数切替（解析キャッシュ再生成せず raster のみ作り直し）
- [ ] メトリクスレーンのホバー（レーン名/値/時刻/推定音名）
- [ ] 再生カーソル行の自動スクロール（手動スクロール中は追従しない）

### 7.2 108-band spectrum（下段）
- [ ] 20Hz–18kHz / 約1 semitone 幅、多解像度 FFT（`SpectrumAnalyzer`）
- [ ] 鍵盤ハイライト（相対突出評価、A0–C8 明色/範囲外グレー）
- [ ] ホバーで周波数 Hz + 近似音名
- [ ] 再生バッファ短窓 tap（全尺解析を待たない）

### 7.3 beat/bar grid
- [ ] `BeatGrid` 表示（低 confidence は非表示 or 淡色、`BeatTrackingStatus`）
- [ ] 手動 BPM / first beat 補正の永続化（`UserCorrected`）

### 7.4 再生制御
- [ ] Play / Pause / Stop / seek / volume / mute（`VideoPlayer` メソッド）
- [ ] 上情報バー常時表示 / 下シークバー常時表示（D3）
- [ ] Open / D&D 直後の自動再生（全尺解析を待たない）
- [ ] normalize gain（既存 audio pump）

### 7.5 ブックマーク（左パネル、動画機構を再利用 D5.1）
- [ ] 一覧表示（代表サムネ・チャプターなし）
- [ ] 現在位置に追加（`KeyAction::VideoBookmark`、既定 `B`）/ 削除 / 改名（ユーザー命名）/ クリックでジャンプ
- [ ] インポート（動画と同一の一括登録ダイアログ + `parse_chapter_text`、`mm:ss タイトル`）
- [ ] エクスポート（クリップボード、`format_chapter_lines` + `seconds_only` トグル）
- [ ] シークバー上のマーカー表示

### 7.6 右パネル
- [ ] タグ chips（動画経路ミラー）/ ★ レーティング / 設定部分
- [ ] 音楽情報（format/duration/sample rate/channels/bitrate/埋め込みメタ）

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
| decode/timeline 解析 | loader worker(symphonia) | 新解析ワーカー(FFmpeg→`analyze_stereo_timeline`) | [ ] UI スレッド非同期 / cancel 3 箇所 / 部分解析 merge / path+generation で stale 破棄 |
| 再生 | cpal 簡易プレイヤー | `VideoPlayer`(headless) + audio pump | [ ] seek/volume/mute/normalize が動く / 映像 decode を走らせない |
| VST3 | `NoopEffectChain` | `DspBridge`(audio pump 既存) | [ ] realtime callback で直接呼ばない / 失敗時 auto-disable / 動画と同一 state |
| row raster | raster worker | 移植 raster worker | [ ] cache key+generation+row version / 1 frame 少量 upload / 古い結果を採用側で破棄 |
| spectrum | spectrum worker | 常駐 spectrum worker + ring buffer tap | [ ] 全尺解析と独立 / pending 中は旧結果保持 |
| repaint | — | `request_repaint_after` | [ ] pending worker / 再生中のみ。待ち無しで busy repaint しない |
| grid/viewer lifecycle | — | GridItem::Audio / fs_cache / detached | [ ] 新ファイルで旧 worker cancel / フォルダ移動・close で漏れなし |

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

- **Inc 2: 解析ワーカー + `audio_analysis.db`（UI スレッド外）**
  - FFmpeg decode → `analyze_stereo_timeline` → `TimelineAnalysis`。worker + cancel + mpsc。
  - `audio_analysis.db`（key 契約、version gate）。
  - **決定性テスト**: 固定 PCM バッファ → `analyze_stereo_timeline` が安定（`analysis_version` gate）。
    FFmpeg decode PCM がラボ symphonia とほぼ一致（許容誤差、1 本）で「デコーダ差で解析がズレない」を担保。
  - 受け入れ: 音声を開くと解析がバックグラウンドで走り DB 保存、再起動でキャッシュヒット。
    UI スレッド同期 I/O なし（perf 計装 + `analyze_perf.py hitches`）。

- **Inc 3: 音楽ビュー骨格 — `VideoPlayer` 再生 + timeline canvas + 上情報バー/下シークバー常時**
  - 音声フルスクリーンで `VideoPlayer`（headless）を作り自動再生。egui viewport に timeline canvas
    （row raster worker、部分解析対応）+ 上情報バー常時 + 下シークバー常時。play/pause/seek/volume/mute。
  - 受け入れ: 音声を開くと自動再生 + timeline 表示 + seek/play/pause + 上下バー常時。ラボと同じ見え/音。

- **Inc 4: 108-band spectrum worker（下段）**
  - `SpectrumAnalyzer` 常駐 worker + playback ring buffer tap。
  - 受け入れ: 下段アナライザがラボと同じ挙動。全尺解析を待たない。

- **Inc 5: 右パネル（タグ+設定+音楽情報）+ 左パネル（ブックマーク）**
  - 右: 動画のタグ/★/設定経路ミラー + 音楽情報ブロック。左: ブックマーク一覧（命名/追加/削除/改名/
    ジャンプ/インポート、シークバーマーカー）。ブックマーク永続化。
  - 受け入れ: 右にタグ+設定+音楽情報、左にブックマーク一覧、命名/ジャンプ/インポート動作。

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

- **解析デコード経路**（Inc 2）: `src/video/decoder.rs` の FFmpeg 経路を「音声全尺 decode-to-PCM」
  モードで流用するか、軽量 avformat/avcodec decode を新設するか。全尺 decode のメモリ（長尺曲）と
  部分解析ストリーミングの両立方法。
- **`SUPPORTED_AUDIO_EXTENSIONS` の確定**（Inc 1）: FFmpeg LGPL build が確実に開ける範囲。
- **右パネルのタグ/設定経路**（Inc 5 / §11）: 動画アイテムがタグ chips / ★ / 設定をどこで描いているか
  （`ui_metadata_panel.rs` は画像のみなので別経路の可能性）。実装時に確認して音声もミラー。
- ~~ブックマークのインポートフォーマット~~: **確定** — 動画機構（`video_bookmarks_parser.rs` /
  `video_bookmarks.rs`）をそのまま再利用（D5.1）。新規フォーマット判断は不要。
- **動画→音声モードの presenter 戦略**（Inc 7 / §5.7）: native presenter 残置 overlay 案 vs 切替案。
  着手前に Codex 設計相談。
- **playback ring buffer tap の口**（Inc 4）: `src/video/audio.rs` の `AudioBuffer` を read-only で
  覗く安全な経路。
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
