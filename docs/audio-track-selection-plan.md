# 動画の複数音声トラック選択 設計 (backlog §1.251)

- 状態: 設計案 (独立レビュー前)
- 出典: [next-release-backlog.md §1.251](next-release-backlog.md) (>>429)
- 担当: 設計・検収 = ClaudeCode Opus / 実装 = Codex Sol / 独立レビュー = 別の Sol
- 関連: [video-architecture.md](video-architecture.md) (decoder 3-thread 構成・seek 調停・audio.rs・Norm)、
  [async-architecture.md](async-architecture.md)、[ui-responsiveness.md §4](ui-responsiveness.md)、
  [detached-rework-plan.md §2](detached-rework-plan.md)

## 1. 目的と範囲

複数の音声 stream を持つ動画で、再生する音声トラックを利用者が選べるようにする。

対象:

- ローカル再生の `VideoPlayer` (フルスクリーン / ウィンドウ内 / F12 別ウィンドウ / 複数ウィンドウ /
  動画→音声モード)。
- 同じ再生中の音声に依存する解析: 音量正規化 (Norm) の測定値、seek strip の波形、音声モードの解析表示。

対象外 (本設計では変更しない。§11 に理由):

- mIV Remote の配信 (時計なし transcode は既定トラックのまま)。IPC protocol version も変えない。
- 詳細表示 (一覧の詳細列・`probe_audio_details`) はファイル単位の情報として既定トラックのまま。
- 字幕。字幕機能は存在しない。本機能の状態は字幕と共有しない。
- 選択の永続化 (§9)。
- 開いた時点で既定トラックの音声初期化に失敗した動画での、別トラックへの切り替え (§7.4)。

## 2. 現状 (コード確認 2026-09-26)

- 音声 stream は demux thread の `run_decoder` 内で `input.streams().best(MediaType::Audio)` の 1 本だけを
  選ぶ (`decoder.rs:2192`)。index は `AudioSetup.stream_idx` と demux loop の不変 `let
  audio_stream_idx_for_demux` (`decoder.rs:2650`) に入り、packet routing (`decoder.rs:3254`) だけが使う。
  他の音声 stream の packet は読んで捨てている。
- thread 構成: `video-demux` / `video-decode` / `video-audio-decode` + `audio-pump` + cpal callback。
  demux → audio decode は `audio_pkt_tx` (bounded 64) と `audio_ctl_tx` (`AudioControlMsg::Flush`、bounded 8、
  `select_biased!` で優先受信)。
- `AudioSetup` (decoder + resampler/fast downmix + time base) は demux thread で作られ、audio decode thread へ
  move される (Send)。resampler の**出力**は常に f32 packed stereo / device 既定 rate
  (`mod.rs:8691` の `target_rate`)。cpal stream・pump の limiter / stretcher / normalize ramp も device rate で
  作られ、音源に依存しない。**別トラックへの切り替えで作り直す必要があるのは `AudioSetup` だけ。**
- seek は `seek_serial` (AvClock と EngineActor が共有する `Arc<AtomicU64>`) で世代管理され、packet /
  frame / chunk / engine event がすべて serial を持ち、各段が旧世代を捨てる。seek 時は UI thread で
  `request_seek` → `clear_audio_output_buffer` → `engine.handle_seek_request` (latch 再初期化)、demux thread で
  `av_seek_frame` → 両 decode thread へ `Flush` → `notify_seek_completed` → `SeekCompleted`。pump は新世代の
  最初の frame で VST (`reset_plugins_sync`)・limiter・stretcher を reset する。
- 一時停止・速度・音量・mute は `AvClock` / `EngineActor` が持ち、audio thread 側は状態を持たない。
- 同一 source を位置を保って開き直す仕組みは存在しない。decoder 内での差し替えの前例は video decoder の
  SW fallback (`decoder.rs:3809-3958`)。
- `VideoInfo` の音声情報は `audio_codec` / `audio_bit_rate_bps` / `has_audio` だけで、一度だけ `info_tx` で届く。
- 既定トラックを独自に `best(Audio)` で選ぶ箇所: `normalize_scanner.rs:117`、`audio_decode.rs:194`
  (seek strip 波形・音楽解析)、`clockless_transcode.rs:1524` (Remote)、`app/metadata_ops.rs:1621` (詳細)、
  `bin/normalize_probe.rs:60`。
- 動画 HUD (native presenter の egui overlay) に汎用の「…」メニューは無い。popup の雛形は seek strip メニュー
  (`render_core.rs:1628` `draw_native_seek_strip_menu`) と速度 popup。
- 音量正規化の測定値 DB `audio_normalize.db` はリリース済み (v0.9.0〜)。主キー
  `(path_lower, file_size, mtime_ms, target_lufs_milli)` に stream の区別は無い。

## 3. 方式の決定: 同じ player 内で「位置を保つ seek + 音声 stream の差し替え」

選択肢:

| 案 | 内容 | 判定 |
|---|---|---|
| A. source の開き直し | 新しい `VideoPlayer` を作り、native output を移し、現在位置から開き直す | 不採用。`MAX_LIVE_VIDEO_DECODE_THREADS=1` のため旧 decoder の終了待ち (`NativeVideoSourceSwapPending`) が挟まり、video decoder・presenter source・HW frame pool まで作り直す。seek strip / Norm / resume / placement の既存 owner も source 切り替えとして動いてしまう。音声だけの変更に対して影響範囲が大きすぎる |
| B. audio worker だけ別 input で交換 | 音声用に 2 本目の `AVFormatContext` を開き、別 thread で音声だけ読む | 不採用。demux が 2 本になり、EOF・seek 調停・back-pressure の単一 puller 構造 (seek は demux が唯一の puller) を崩す |
| **C. 同一 demux 内で routing と `AudioSetup` を差し替え、同じ seek 経路で flush する** | 選択を共有状態に書き、現在位置への seek を 1 回発行する。demux は seek 要求を取り出したときに選択を読み、新しい `AudioSetup` を作ってから routing を変え、`Flush` に載せて audio decode thread へ渡す | **採用** |

C を採用する理由:

- 旧トラックのデータが残らない境界は、既存 seek の世代境界がすでに全段 (packet queue / avcodec /
  audio_tx / pump の raw・processed / VST・limiter・stretcher / cpal callback / A/V clock anchor /
  readiness latch) で保証している。音声だけの切り替えも「その世代以降の音声は新トラック」と定義すれば、
  新しい世代境界を発明しなくてよい。
- 新しいトラックの packet は demux の現在位置より前にある (旧トラックの packet の分だけ demux は先に
  読んでいる)。現在位置から新トラックを鳴らすには demux を戻す必要があり、どの案でも seek は避けられない。
- UI thread は共有状態の書き込みと seek 発行だけを行い、decoder の終了・作成を待たない。

代償: 切り替え時に映像も通常の seek と同じく keyframe から target まで preroll し直す
(再生中なら数百 ms 程度、最後に表示したフレームを保持)。同種のプレイヤーも音声トラック切り替えで
同じ refresh seek を行っており、許容する。

## 4. データモデル

### 4.1 トラック情報 (静的、open 時に 1 回)

`VideoInfo` に追加:

```rust
pub struct AudioTrackInfo {
    /// AVStream index。選択 command と routing の同一性に使う唯一の key。
    pub stream_index: usize,
    /// 音声 stream の中での 1 始まりの順番 (表示用)。
    pub ordinal: usize,
    /// metadata "language"。無い / "und" / 空は None。
    pub language: Option<String>,
    /// metadata "title"。無い / 空は None。handler_name 等から推測しない。
    pub title: Option<String>,
    /// decoder 名ではなく codec 名 (例 "aac" / "ac3" / "opus")。
    pub codec: String,
    /// codecpar のチャンネル数。0 (不明) は None。
    pub channels: Option<u32>,
    /// codecpar の sample rate。0 は None。
    pub sample_rate: Option<u32>,
    /// AV_DISPOSITION_DEFAULT が立っているか (表示用の事実。選択規則には使わない)。
    pub disposition_default: bool,
}

// VideoInfo
pub audio_tracks: Vec<AudioTrackInfo>,        // 再生可能 (decoder が見つかる) 音声 stream のみ、stream 順
pub default_audio_stream_index: Option<usize>, // open 時に best(Audio) で選ばれ、実際に開いた stream
```

- 列挙は demux thread が open 時に行う (`info_tx` 送出の前)。decoder が見つからない stream は列挙しない
  (backlog の「再生可能な音声 stream」)。
- 取得できない情報は `None` にする。言語名への変換 (「jpn」→「日本語」) は UI 層の固定表で行い、表に無い
  code はそのまま表示する。推測で埋めない。
- 既存の `audio_codec` / `audio_bit_rate_bps` は「既定トラック (open 時に開いたもの)」の意味のまま残す。
  HUD の右パネルは選択中トラックの情報を `audio_tracks` から引く (§8.3)。

### 4.2 選択状態 (動的) — 単一 owner `AudioTrackSelection`

`VideoPlayer` ごとに 1 つ、`Arc<AudioTrackSelection>` を持ち、demux thread と共有する。App 側に新しい
bool / Option / pending を追加しない ([detached-rework-plan §2](detached-rework-plan.md) の BA-7 に抵触しない)。

```rust
struct AudioTrackSelectionState {
    /// 利用者が最後に選んだトラック。generation は選択ごとに +1。
    desired: (u64 /*generation*/, usize /*stream_index*/),
    /// demux が実際に routing している stream と、それを確定させた desired の generation。
    applied: (u64, usize),
    /// 直近の切り替え失敗。generation は失敗した desired の generation。
    last_failure: Option<AudioTrackSwitchFailure>,
}
```

- 初期値: `desired = applied = (0, default_audio_stream_index)`。音声が無い / 音声初期化に失敗した player は
  selection を持たない (`Option<Arc<...>>` = None)。
- 書き手: `desired` は UI thread (`VideoPlayer::select_audio_track`) だけ、`applied` / `last_failure` は
  demux thread だけ。1 つの `Mutex` で守る (保持区間は値のコピーのみ)。
- 読み手: HUD / metadata / App は `snapshot()` で値をコピーして読む。

表示上の状態は snapshot から導出し、別 flag を持たない:

| 導出状態 | 条件 |
|---|---|
| 確定 | `desired.gen == applied.gen` |
| 切り替え中 | `desired.gen > applied.gen` かつ `last_failure.gen != desired.gen` |
| 失敗 | `last_failure.gen == desired.gen` (routing は `applied` のまま) |

## 5. 切り替えの手順

### 5.1 UI thread (`VideoPlayer::select_audio_track(stream_index)`)

1. `stream_index` が `audio_tracks` に無い、または selection が無い player なら何もせず `Rejected` を返す。
2. `desired.stream == stream_index` かつ失敗状態でないなら no-op (`Unchanged`)。
3. `desired = (desired.gen + 1, stream_index)` を書く。
4. engine の published state が `Eof` なら seek は発行しない (§7.3)。`Deferred` を返す。
5. それ以外は、**3 の後で**、位置を保つ seek を既存の user seek 経路で 1 回発行する:
   - 基準位置: 一時停止中 (frame-step pause を含む) は `last_displayed_pts_secs()`、それ以外は
     `user_seek_base_secs()` (coalesce 中の pending target を優先、無ければ `position()`)。
   - 再生状態: `seek_with_play_state(base, self.intent_playing())`。一時停止中は一時停止のまま、再生中は再生のまま。
   - coalesce によって `request_seek` がすぐに出ない場合も、後で発行される seek が 3 の値を読むので
     取りこぼさない (§5.3)。
6. `Requested` を返す。UI thread はここで終わり、decoder の終了・作成・完了を待たない。

速度・音量・mute・Norm の ON/OFF・ループ設定は `AvClock` / `EngineActor` / App が持っており、この手順は触らない。

### 5.2 demux thread (seek 要求の取り出し時)

`take_seek_request()` で要求を取り出した直後、`av_seek_frame` の前に selection を読む:

1. `desired.gen > applied.gen` かつ `desired.stream != applied.stream` なら、`input.stream(desired.stream)` から
   新しい `AudioSetup` を組む (open 時と同じ関数を stream 指定で呼べるように分離する)。
   - 成功: `audio_stream_idx_for_demux` と time base をその場で差し替え、`applied = desired` とし、
     新 `AudioSetup` を `AudioControlMsg::Flush` に載せる (下記)。
   - 失敗: routing は変えない。`last_failure = Some({gen: desired.gen, stream, reason})` を書く。seek 自体は
     そのまま続ける (UI 側はすでに buffer を clear しているので、旧トラックで同じ位置から再開する)。
2. `desired.gen > applied.gen` かつ `desired.stream == applied.stream` (元のトラックへ戻した等) なら
   `AudioSetup` は作らず `applied.gen = desired.gen` だけ進める。
3. 以降は通常の seek と同じ (`av_seek_frame` → video overflow 破棄 → `Flush` 送出 →
   `notify_seek_completed` → `SeekCompleted`)。

`AudioControlMsg::Flush` に `replace_setup: Option<Box<AudioSetup>>` を追加する。audio decode thread は
`Flush` 受信時、`replace_setup` があれば旧 `AudioSetup` (avcodec context・resampler・fast downmix) を drop して
差し替えてから、既存の flush 処理 (serial / trim 下限 / target / `next_audio_pts_secs` の更新) を行う。
旧 serial の packet は既存どおり serial 不一致で捨てるので、旧トラックの packet が新 decoder に入ることはない。

`AudioSetup` の構築を demux thread で行う理由: 成否を routing 変更の前に確定でき、失敗時に audio decode
thread 側が「decoder の無い状態」を持たずに済む。構築は codec open と swr init のみ (数 ms) で、demux thread を
一時的に止めるだけで UI thread は止めない。

### 5.3 most-recent-wins の保証

- `desired` は latest-value。連続選択 A → B → C は途中の値を上書きし、demux が次に seek 要求を取り出したときの
  値 (C) だけが反映される。
- seek 要求 (`AvClock.seek_request`) も latest-value で、切り替えの seek の後に利用者の通常 seek が来て要求が
  上書きされても、demux はその通常 seek の取り出し時に `desired` を読むので切り替えは失われない。
- 順序: UI thread は `desired` を書いてから `request_seek` する。demux は `take_seek_request` の後で `desired` を
  読む。seek 要求の mutex を介して、「切り替え後に発行された seek を取り出した demux は必ずその切り替え以降の
  `desired` を見る」。
- 古い完了が新しい選択を上書きしない: `applied` / `last_failure` は desired の generation を持ち、
  導出状態 (§4.2) は generation 比較だけで決まる。遅れて書かれた旧 generation の失敗は `desired.gen` と一致
  しないので表示されない。App 側の通知 (トースト) も「失敗 generation が現在の desired generation と一致し、
  まだ通知していない」ときだけ 1 回出す (通知済み generation を `VideoPlayer` 内に持つ)。
- 失敗時の desired の扱い: 失敗を表示したあと、`desired` を書き戻さない (書き戻すと利用者の最新操作と競合
  する)。UI は「失敗」状態として選択中の行に失敗表示を出し、実際に鳴っているのは `applied` のトラックである
  ことを示す。次の選択で通常どおり上書きされる。

### 5.4 旧トラックのデータが残らない境界 (段ごと)

| 段 | 境界の仕組み (既存 / 追加) |
|---|---|
| demux → audio packet queue | 既存: packet は取り出し時の `seek_serial` を持ち、audio decode thread は serial 不一致を捨てる。送信待ち中の旧 packet は `SeekPending` で破棄 |
| avcodec decoder | 追加: `Flush.replace_setup` で旧 context ごと drop。差し替えない場合は既存の `decoder.flush()` |
| resampler / fast downmix | 追加: `AudioSetup` ごと差し替え (旧 swr の delay に残ったサンプルも一緒に捨てる)。差し替えない場合は既存どおり (§12 の既知事項) |
| audio_tx (decoded frame) | 既存: `AudioFrame.seek_serial` を pump が clock serial と比べて捨てる |
| pump raw / processed | 既存: UI thread の `clear_audio_output_buffer` と、pump の新世代検出での clear |
| VST / limiter / time stretcher | 既存: pump が新世代の最初の frame で reset |
| Norm gain ramp | 追加: §6.1。新トラックの gain へ ramp せず snap する |
| cpal callback | 既存: `pump_seek_serial < clock_serial` の間は silence |
| A/V clock | 既存: `notify_seek_completed` と `BufferReady` による Audio anchor の張り直し |
| engine readiness | 既存: `handle_seek_request` の latch 再初期化 |

## 6. 解析系の追従

### 6.1 音量正規化 (Norm)

測定値はトラックごとに異なるので、選択中トラックの測定値を使う。

- DB: 既存テーブル `audio_normalize` の意味を「既定トラック (FFmpeg が `best(Audio)` で選ぶ stream) の測定値」と
  明文化し、変更しない。既定以外のトラックは**追加の新テーブル**
  `audio_normalize_track (path_lower, file_size, mtime_ms, target_lufs_milli, stream_index, gain_db,
  integrated_lufs, true_peak_db, scanned_at, PRIMARY KEY(...5 列))` に保存する。
  - 既存テーブルの主キーを変える移行は行わない。旧版へ戻したときに旧版の `ON CONFLICT (4 列)` が失敗する
    ため (downgrade で Norm が壊れる)。新テーブルは旧版から見えないだけで害が無い。
  - key の規則は 1 つ: 「トラックが既定トラックなら `audio_normalize`、それ以外は `audio_normalize_track`」。
    判定は `stream_index == default_audio_stream_index` だけで行う。
  - `clear_all` / `count` は両テーブルを対象にする。
- scanner: `normalize_scanner` に対象 stream index を渡せるようにし、`best(Audio)` の独自選択をやめる
  (既定トラックでも open 時に確定した index を渡す)。
- App: `NormalizeScanState` に対象 stream index を持たせ、完了時の stale 判定を (file path, stream index) で
  行う。scan 中にトラックが替わったら既存の「別動画の scan が残っている」場合と同じく旧 scan を cancel する。
- 切り替え時: Norm が全体 ON のとき、UI thread は `select_audio_track` の**前**に新トラックの測定値を引く。
  - 測定済み: gain を新しい値にし、pump の ramp を新世代で snap させる (`AvClock` の normalize gain に
    「次の世代の最初の chunk から snap」する指示を足す。ramp の既存挙動 = 手動 ON/OFF 時の 4 秒 ramp は変えない)。
  - 未測定: 既存の「未測定動画の再生 intent」経路 (`maybe_start_normalize_scan_for_play_intent`) を新トラックで
    通す。一時停止中は scan を始めない (既存規則どおり play intent で始まる)。
  - DB lookup の I/O は既存の open 時 lookup と同じ扱い (§12 の既知事項)。

### 6.2 seek strip 波形・音声モードの解析

- `audio_decode::AudioRangeDecoder::open` と音楽解析の decode 入口に stream index 指定を追加し、呼び出し側は
  player の `applied.stream` を渡す。
- 波形 session の identity (現状: owner fs index / 動画パス / source epoch / items generation) に
  `audio stream index` を加える。トラックが確定 (`applied` が変わる) したら既存の identity 不一致経路で worker を
  作り直す。切り替え中 (desired ≠ applied) は旧波形を表示し続け、確定時に差し替える。
- 失敗表示時 (routing は旧トラック) は `applied` を見るので波形は旧トラックのまま正しい。

## 7. ライフサイクル上の扱い

### 7.1 再生中 / 一時停止中 / seek 直後

§5.1 のとおり、play intent を保って seek を 1 回発行するだけ。seek 直後 (前の seek がまだ表示されていない) は
既存の coalesce に従い、後続の seek で `desired` が反映される。

### 7.2 連続切り替え

§5.3。seek 要求は coalesce され、`AudioSetup` の構築は demux が取り出した要求 1 回ごとに最大 1 回。

### 7.3 再生終了 (EOF) と重なった場合

- engine が `Eof` の間に選択された場合、seek は発行せず `desired` だけ更新する (表示は「切り替え中」)。
  末尾への seek は既存の「シーク中...固着」経路 (`seek_eof_stuck_since`) を踏むため避ける。
- 次に seek が発生したとき (利用者の seek、ループ再生の先頭 seek、再生ボタンによる先頭からの再開) に demux が
  反映する。
- demux が EOF idle wait 中に `desired` だけ変わっても起床は不要 (seek 要求で起床する既存の設計どおり)。

### 7.4 音声なし / 単一音声 / 開いた時点で音声が無効

- 音声 stream が 0 本、または `audio_tracks.len() < 2` なら UI を出さない (keymap action は no-op + 何も表示しない)。
- 既定トラックの音声初期化に失敗した (`mark_audio_inactive`) / 出力 device を開けなかった (`self.audio == None`)
  player は selection を持たない。別トラックへの切り替えには audio decode thread の途中起動と、engine の
  master clock を Wall → Audio へ切り替える経路が要るが、どちらも現在の engine に production 経路が無い
  (`AudioInactive` / `has_audio` は open 時 1 回)。本設計では扱わず、UI も出さない。

### 7.5 動画の切り替え・player の破棄

- selection は `VideoPlayer` が所有し、player の drop とともに消える。source swap で作られる新 player は既定
  トラックから始まる (§9)。
- 旧 player の demux thread が drop 中に selection を書いても、Arc はその player にしか共有されないので他の
  player に影響しない。
- 旧 player の native output event は既存どおり fs_idx / source epoch で捨てられる。

### 7.6 F12 別ウィンドウ・複数ウィンドウ

- 状態は player (= viewer context の `fs_cache` 内) にあるので、context ごとに独立。App に新 field は足さない。
- placement switch (F12 の live 切り替え) は decoder を保持するので選択も保持される。
- HUD のクリックは既存の `NativeVideoOutputEvent` 経路で、その player の event bus からだけ届く。ParkedLive の
  窓でのクリックは既存 filter で「窓の活性化」になる。新しい event variant は
  `native_video_output_event_is_parked_live_hud_click_activation` で `true` (HUD click) に分類する。
  - これは detached 経路の述語に variant を 1 つ加える変更なので、CLAUDE.md「Detached viewer リワーク中の
    ルール」に従い、独立レビューで「症状パッチではなく、新しい HUD 操作の分類を既存規則どおり加えるだけの
    構造的変更」であることに合意を取り、[detached-rework-plan.md](detached-rework-plan.md) §11 に記録する。
- 同時に生きる decoder は 1 本 (`MAX_LIVE_VIDEO_DECODE_THREADS=1`) で、本機能は decoder を増やさない。

### 7.7 動画→音声モード

- 同じ player を使い続けるので選択は保持される。音声モードの HUD (egui、`draw_music_bottom_hud`) に同じ選択
  UI を置く (§8.2)。音声モードの解析は §6.2 で `applied` に追従する。

### 7.8 VST

- VST chain は app 全体で 1 つ、音源 stream の状態を持たない。新世代の最初の frame で `reset_plugins_sync` が
  走る既存経路で、旧トラックの尾 (reverb 等) は切れる。追加の処理は要らない。

## 8. UI

### 8.1 動画 HUD (native presenter)

- 下部 HUD の音量群 (mute / Norm / 音量) の近くに、`audio_tracks.len() >= 2` のときだけ「音声 N」の text
  ボタンを出す (N = 選択中トラックの ordinal)。環境依存グリフ・絵文字は使わない。HUD の縮小段では
  capture 系より後、速度より先に隠す (具体的な段は実装時に既存の縮小表で決め、スナップショットで固定する)。
- クリックで popup。雛形は seek strip メニュー (`draw_native_seek_strip_menu`) と同じ
  「行 = (label, is_current, command)」形式。open flag / 描画 rect は `NativeEguiOverlay` に持ち、
  `compute_hud_regions` に rect を加える (HUD HWND の `SetWindowRgn` がクリックを透過しないように)。
- 行ラベル (取得できた項目だけを並べる): `N: <言語> <title> — <codec> <channels>ch (既定)`
  - 言語: 固定表 (jpn→日本語、eng→英語、… 少数) で変換、表に無い code はそのまま。
  - `(既定)` は `stream_index == default_audio_stream_index` の行だけ。
  - 導出状態が「切り替え中」の行には「(切り替え中)」、「失敗」の行には「(切り替えできません)」を添える。
- 選択 → `NativeOverlayCommand::SelectAudioTrack { stream_index }` → `NativeVideoOutputEvent::SelectAudioTrack` →
  App の handler → `VideoPlayer::select_audio_track` (Norm が ON なら §6.1 の lookup を先に行う)。
- 失敗時は App が既存のトーストで 1 回通知する (「音声トラックを切り替えられませんでした」)。

### 8.2 音声モード HUD (egui)

- `draw_music_bottom_hud` に同じ選択 UI (ComboBox 相当の popup、wheel passthrough 抑止は CLAUDE.md の
  popup 規則どおり)。行ラベルは 8.1 と同じ関数で作る (表示文言の owner を 1 つにする)。

### 8.3 右パネル (動画メタデータ)

- 「音声」行を選択中トラック (`applied`) の codec / channels / 言語 / title に切り替える。bitrate は既定トラック
  しか取れていないので、既定トラック選択時だけ表示する。
- トラックが 2 本以上ある場合は「音声トラック: N 本」を添える。

### 8.4 keymap

- `KeyAction::VideoNextAudioTrack` (FsVideo、既定キーなし、`ChordList::EMPTY`、ini は `# VideoNextAudioTrack = none`)。
  次の ordinal へ循環し、切り替えたらトーストで新しいトラックのラベルを出す。トラックが 1 本以下なら何もしない。
- native VK 経路 (`dispatch_native_video_key_event`) と egui fallback (`handle_video_input`) の両方に配線する。
- `ini_name()` / `description()` / `context()` / `trigger()` / `default_chords()` / `ALL_ACTIONS` /
  `docs/keymap.ini.default` / `docs/keymap-spec.md` を揃える。

## 9. 選択を覚えるか

**覚えない (本設計の範囲)。** 選択は player の寿命の間だけ有効で、同じ動画を開き直すと既定トラックに戻る。

- 理由: 新しい永続データを増やさずに要望 (再生中の切り替え) を満たせる。既定トラックは FFmpeg が
  disposition default を考慮して選ぶので、多くのファイルでは開いた時点で意図どおりになる。
- ファイル単位の記憶より「優先言語」設定のほうが、シリーズを続けて見る使い方に合う可能性がある。どちらを
  作るかは利用者の要望を確認してから別項目として扱う (backlog に起票する)。

## 10. 段階と受け入れ条件

各段は、実装 → ライブラリのテスト全体 (`cargo test -p mimageviewer --lib`) と関係する統合テスト → 独立 Sol
レビュー (ACCEPT) → コミット、の順で進める。

### S1: テスト素材とトラック列挙

- `scripts/ui-smoke/generate_audio_tracks_fixture.py` (または `.ps1`) と `testdata/audio-tracks/README.md`。
  ffmpeg の lavfi だけで作る (私有素材を使わない):
  - `multi.mkv`: testsrc2 映像 6 秒 + 音声 3 本。周波数で識別できる sine
    (440 Hz / 880 Hz / 1320 Hz)、channels (2 / 6 / 1)、sample rate (48000 / 44100 / 32000)、codec
    (aac / ac3 / opus)、language (jpn / eng / 無し)、title (有 / 有 / 無し)、disposition default は 2 本目。
  - `single.mp4` (音声 1 本)、`silent.mp4` (音声なし)。
  - サイズは各 数百 KB 以下。`.gitignore` の `/testdata/*` に `!/testdata/audio-tracks/` を加えて追跡する。
- `VideoInfo.audio_tracks` / `default_audio_stream_index` の列挙。
- テスト: `multi.mkv` の列挙結果 (3 本、各項目、欠けた項目が None、既定 = 2 本目)、`single` / `silent`。

### S2: 切り替えの中核

- `AudioTrackSelection`、`VideoPlayer::select_audio_track`、demux の差し替え、`Flush.replace_setup`、
  失敗経路、EOF の保留。
- テスト (lib、実 decoder を headless で動かす。GPU は使わない):
  - 切り替え後に pump / `AudioFrame` へ届く音声の周波数が新トラックのもの (零交差数で判定)、serial が新しい。
  - 旧トラックの周波数を持つ frame が切り替え後の世代に 1 つも無い。
  - 一時停止中の切り替えで一時停止が保たれ、位置が変わらない。
  - seek 直後 (前の seek 未表示) の切り替え、連続 3 回の切り替えで最後の選択だけが `applied` になる。
  - 切り替えの seek の後に通常 seek を重ねても切り替えが反映される。
  - EOF 中の選択は seek を出さず、次の seek で反映される。
  - `AudioSetup` 構築失敗 (テスト用に失敗を注入する seam) で routing が変わらず、失敗が desired.gen で記録され、
    後から来た古い失敗が新しい選択の表示を上書きしない。
  - 速度・音量・mute が切り替えで変わらない。
  - 導出状態 (§4.2) の純粋関数テスト。
- 作ったテストのうち周波数判定・most-recent-wins・失敗 generation は、対象処理を一時的に外すと落ちることを
  実装者が確かめ、報告に書く。

### S3: UI と操作

- 8.1〜8.4。`NativeVideoOutputEvent` の追加と ParkedLive 分類 (§7.6、§11 記録)。
- テスト: App handler-level (event → `select_audio_track`、fs_idx 不一致で無視、ParkedLive で活性化扱い)、
  行ラベル生成、keymap の表とiniの整合 (既存テスト)、UI スナップショット (音声モード HUD の選択 UI、
  変更があれば `UPDATE_SNAPSHOTS`)。

### S4: 解析系の追従

- 6.1 (Norm、新テーブル、scanner の stream 指定、snap) と 6.2 (波形・音声モード解析の stream 指定と identity)。
- テスト: DB の新旧テーブルの読み分け・`clear_all`/`count`、scanner が指定 stream を測る (sine の振幅を
  トラックごとに変えて LUFS 差で判定)、scan 中のトラック変更で旧 scan が cancel される、波形 identity の変化で
  worker が作り直される。

### S5: 実アプリのシナリオと文書

- ui-smoke: `AudioTracks` シナリオ (`scripts/ui-smoke/audio-tracks.rhai`)。`multi.mkv` を開き、HUD の音声ボタン →
  2 行目を選択 (native HUD の名前付き control を `native_ui_smoke.rs` の既存方式で追加)、snapshot の
  `audio_track` (desired / applied / 導出状態) と、pump が出力した直近 chunk の周波数推定 (test-script feature
  限定の診断値) が新トラックの値になることを確認。一時停止中の切り替え、連続切り替え、F12 別ウィンドウでの
  切り替え、音声モードでの切り替えを含める。`capture(label)` で egui 側 (音声モード HUD) を保存する。
  - 実行は使い捨てコピー (`target\portable-smoke`) で、毎回利用者の了承と時間帯を確認してから。
- 文書: マニュアルの動画ページ、`docs/spec.md`、`docs/video-architecture.md` (seek 調停・Flush・Norm の節)、
  `docs/keymap-spec.md`、backlog §1.251 の状態更新。

## 11. 対象外とした事項の理由

- Remote: Remote の配信は独立した時計なし transcode で、`best(Audio)` を使う。選択を Remote へ出すには
  protocol に項目を足し、transcode 側にも stream 指定が要る。要望はローカル再生なので、本設計では Remote は
  既定トラックのままとし、backlog に別項目として残す。protocol version は変えない。
- 詳細表示 (一覧): ファイル単位の情報で、再生中の選択とは無関係。
- 開いた時点で音声が無効な player での切り替え: §7.4。

## 12. 既知事項・確認したい点 (レビューで判断を求める)

1. 同じトラックへ戻す等、`AudioSetup` を差し替えない seek では resampler は reset されない (既存の seek と同じ)。
   既存どおりで良いか。
2. §6.1 の切り替え時の Norm DB lookup は UI thread で行う (既存の open 時 lookup と同じ場所・同じ I/O)。
   既存が UI thread なのか worker なのかを実装者が確認し、UI thread なら既存と同等 (1ms 級) として許容するか。
3. `AudioSetup` を demux thread で構築する間、video packet の供給も止まる (数 ms)。seek と同じタイミングなので
   許容と考える。
4. 切り替えの seek は `seek_with_play_state` を使うので、HUD の「シーク中...」表示 (150ms 超で表示) が
   出る場合がある。専用の文言にするかどうか (本設計では既存のまま)。
