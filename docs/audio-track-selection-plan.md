# 動画の複数音声トラック選択 設計 (backlog §1.251)

- 状態: 設計案 第2版 (独立レビュー1回目 REVISE を反映)
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
4. demux が末尾に達している (`clock.is_eof_reached()`。engine の `Eof` 確定前の末尾 drain 中も含む) なら
   seek は発行しない (§7.3)。`Deferred` を返す。
5. それ以外は、**3 の後で**、位置を保つ seek を 1 回発行する:
   - 基準位置: 一時停止中 (frame-step pause を含む) は `last_displayed_pts_secs()`、それ以外は
     `user_seek_base_secs()` (coalesce 中の pending target を優先、無ければ `position()`)。
   - 再生状態: `seek_with_play_state(base, self.intent_playing())`。一時停止中は一時停止のまま、再生中は再生のまま。
     `seek_with_play_state` は coalesce を挟まず即時に `request_seek` する。coalesce 待ちの pending target が
     あれば基準位置としてそれを使い、その pending は clear する (同じ位置への seek を 2 回出さない)。
6. `Requested` を返す。UI thread はここで終わり、decoder の終了・作成・完了を待たない。

速度・音量・mute・Norm の ON/OFF・ループ設定は `AvClock` / `EngineActor` / App が持っており、この手順は触らない。

### 5.2 demux thread (seek 要求の取り出し時)

`take_seek_request()` で要求を取り出した直後に selection を読み、次の順で処理する。**routing と `applied` の
確定は、seek と `Flush` の受理が両方成立した後に限る。**

1. `desired.gen > applied.gen` かつ `desired.stream != applied.stream` なら、`input.stream(desired.stream)` から
   新しい `AudioSetup` を組む (open 時と同じ関数を stream 指定で呼べるように分離する)。この時点では routing も
   `applied` も変えない。構築時間は perf event に残す。
   - 構築失敗: `last_failure = Some({gen: desired.gen, stream, reason: SetupFailed})`。以降は通常の seek として
     続ける (旧トラックのまま同じ位置から再開する)。
2. `av_seek_frame` (既存の backward → forward fallback)。
   - seek 失敗: 1 で組んだ `AudioSetup` を捨て、`last_failure = Some({.., reason: SeekFailed})`。既存の
     seek 失敗経路 (trim なしの `Flush`・`clear_seek_target_override`) をそのまま通る。旧トラックの routing のまま、
     demux の位置は変わらない。
3. seek 成功なら、video overflow 破棄 → video `Flush` → audio `Flush { .., replace_setup }` を送る。
   audio `Flush` の送信結果を見る (現状は捨てている `decoder.rs:3054`)。
   - 送信成功: ここで初めて `audio_stream_idx_for_demux` と audio time base を差し替え、`applied = desired` を書く。
   - 送信失敗 (audio decode thread が終了している): routing を変えず、`last_failure = {.., reason: WorkerGone}`。
4. `desired.gen > applied.gen` かつ `desired.stream == applied.stream` (元のトラックへ戻した等) は `AudioSetup` を
   作らず、3 の送信成功時に `applied.gen = desired.gen` だけ進める。
5. 以降は通常の seek と同じ (`notify_seek_completed` → `SeekCompleted`)。

`AudioControlMsg::Flush` に `replace_setup: Option<Box<AudioSetup>>` を追加する。audio decode thread は
`Flush` 受信時、`replace_setup` があれば旧 `AudioSetup` (avcodec context・resampler・fast downmix) を drop して
差し替えてから、既存の flush 処理 (serial / trim 下限 / target / `next_audio_pts_secs` の更新 / EOF drain 状態・
保留中 packet の破棄) を行う。旧トラックの packet が新 decoder に入らないことは §5.3 の serial 規則で保証する。

`AudioSetup` の構築を demux thread で行う理由: 成否を routing 変更の前に確定でき、失敗時に audio decode
thread 側が「decoder の無い状態」を持たずに済む。構築は codec open と swr init のみだが所要時間は未測定なので、
perf event で計測し、S2 の受け入れ試験で素材ごとの値を記録する。UI thread は止めない。

### 5.3 packet の世代番号は demux が処理済みの seek 世代から付ける (既存の競合の修正)

現状、demux は packet に `clock.current_seek_serial()` (live serial) を付ける (`decoder.rs:3272`)。
`request_seek` は serial を進めてから seek 要求を公開する (`clock.rs:613` → 要求の mutex) ため、その隙間に
demux が**旧 routing・旧位置**で読んだ packet が**新しい serial** を持って queue に入り得る。audio decode thread
は `Flush` を優先受信した後、その packet を新世代として受け入れる。通常の seek では旧位置の音が一瞬混ざる
程度だが、トラック切り替えでは旧トラック (別 codec) の packet が新 decoder に入る。

修正: demux が保持する「処理済み seek 世代」(`demux_serial`) を導入し、packet (audio / video とも) にはこれを
付ける。`demux_serial` は demux が seek 要求を処理して両 `Flush` を送った後にだけ、その要求の serial に更新する。

- seek 要求の公開前に読まれた packet は旧 `demux_serial` を持ち、decode thread の既存判定
  (`serial != current || serial != live` で破棄、`decoder.rs:5613`) で捨てられる。
- `demux_serial` の新しい packet は、同じ世代の `Flush` を control channel へ送った後にしか queue に入らない。
  decode thread は control を優先受信する (`select_biased!`) ので、新世代の packet を見る前に必ず `Flush` を処理する。
- video 側も同じ規則にする (旧位置の frame が新世代として表示される同型の競合を同時に塞ぐ)。
- この変更は既存の seek 全体に効くので、S2 の最初に単独で入れ、既存の seek テストを通してから切り替えを載せる。

### 5.4 most-recent-wins の保証

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

### 5.5 旧トラックのデータが残らない境界 (段ごと)

| 段 | 境界の仕組み (既存 / 追加) |
|---|---|
| demux → audio packet queue | 修正: packet は demux の処理済み seek 世代を持つ (§5.3)。audio decode thread は serial 不一致を捨てる (既存)。送信待ち中の旧 packet は `SeekPending` で破棄 (既存) |
| avcodec decoder | 追加: `Flush.replace_setup` で旧 context ごと drop。差し替えない場合は既存の `decoder.flush()` |
| resampler / fast downmix | 追加: `AudioSetup` ごと差し替え (旧 swr の delay に残ったサンプルも一緒に捨てる)。fast downmix は状態を持たない変換。同じトラックのままの seek は既存どおり swr を保持する (同じトラックの数 ms の delay が残るのは既存 seek と同じで、本機能の境界ではない) |
| audio_tx (decoded frame) | 既存: `AudioFrame.seek_serial` を pump が clock serial と比べて捨てる |
| pump raw / processed | 既存: UI thread の `clear_audio_output_buffer` と、pump の新世代検出での clear |
| VST / limiter / time stretcher | 既存: pump が新世代の最初の frame で reset |
| Norm gain | 追加: §6.1。新トラックの gain が確定するまで processed を作らず、確定後は ramp せず snap する |
| cpal callback | 既存: `pump_seek_serial < clock_serial` の間は silence |
| A/V clock | 既存: `notify_seek_completed` と `BufferReady` による Audio anchor の張り直し |
| engine readiness | 既存: `handle_seek_request` の latch 再初期化 |

## 6. 解析系の追従

### 6.1 音量正規化 (Norm)

測定値はトラックごとに異なるので、選択中トラックの測定値を使う。

#### DB (リリース済み `audio_normalize.db`)

- 既存テーブル `audio_normalize` の主キーは変えない。変えると旧版へ戻したときに旧版の
  `ON CONFLICT (4 列)` が失敗し、downgrade で Norm が壊れる。
- 追加テーブル `audio_normalize_track (path_lower, file_size, mtime_ms, target_lufs_milli, stream_index,
  gain_db, integrated_lufs, true_peak_db, scanned_at, PRIMARY KEY(5 列))` を作る (`CREATE TABLE IF NOT EXISTS`、
  旧版からは見えないだけで害が無い)。
- 規則は 1 つ:
  - **新規保存は常に `audio_normalize_track` へ、stream index を明示して書く** (既定トラックも含む)。
    `best(Audio)` の選択は FFmpeg の版で変わり得るので、「既定トラック」を永続 identity にしない。
  - **読み出しは `audio_normalize_track` を先に引く。無く、かつ対象 stream が open 時に開いた既定トラック
    (`default_audio_stream_index`) のときだけ、旧 `audio_normalize` の行を後方互換として使う。** 旧行は
    「その行を書いた版が `best(Audio)` で選んだトラック」の測定値で、この互換読みはその前提に依存することを
    コメントに残す。旧テーブルへは書かない。
- `clear_all` / `count` は両テーブルを対象にする。

#### scanner と App の状態

- `normalize_scanner` に対象 stream index を渡し、`best(Audio)` の独自選択をやめる (既定トラックでも open 時に
  確定した index を渡す)。
- App の Norm 状態 (per fs_idx の `normalize_ui_states` / `NormalizeScanState`、provisional 結果、自動 scan 抑止) の
  key を (fs_idx, file path) から (fs_idx, file path, stream index) に広げる。scan の provisional / 完了 / 抑止は
  すべて stream index 一致で照合し、不一致は stale として捨てる。scan 中に `applied` のトラックが替わったら
  既存の「別動画の scan が残っている」場合と同じく旧 scan を cancel する。

#### 切り替え時の gain — `applied` に結び付ける

gain は「実際に鳴っているトラック」(`applied`) の測定値でなければならない。切り替えが失敗して旧トラックが鳴り
続けるのに新トラックの gain を当てる、あるいは新トラックの音を旧 gain で鳴らす、のどちらも起こさない。

Norm が全体 ON のとき:

1. App の Norm owner は `select_audio_track` を呼ぶ**前**に既存の `audio_preroll_suspended` を立てる
   (pump は processed を作らず、cpal callback は silence。既存の未測定 scan 待ちと同じ仕組み。`BufferReady` も
   出ないので engine は Buffering で待つ)。先に立てるのは、seek 発行から suspension までの間に新トラックの
   processed が旧 gain で作られる隙間を作らないため。
   - 戻り値が `Requested` / `Deferred` なら、その player の Norm 状態を「トラック確定待ち (desired generation,
     stream)」にする。`Deferred` (末尾) は次の seek で `applied` が変わるまで待つ (末尾では pump に処理する音が
     無いので suspension は害が無い)。
   - `Rejected` / `Unchanged` なら suspension を直ちに解き、状態は変えない。
2. 同時に、測定値の lookup を worker で始める (file metadata 取得 + SQLite。UI thread で I/O しない)。
   結果は (fs_idx, file path, stream index, desired generation) を持って返る。
3. App は毎 tick、selection の snapshot と lookup 結果を突き合わせる:
   - 同じ generation で `applied` が新トラックに確定し、lookup 済み:
     - 測定済み → gain をその値にして pump に「次の chunk から snap」を指示し、suspension を解く。
     - 未測定 → 既存の未測定経路 (`maybe_start_normalize_scan_for_play_intent`、play intent が無ければ scan は
       始めず unity gain で解く) へ新トラックで渡す。suspension の扱いはその既存経路に従う。
   - 同じ generation の失敗 (`last_failure.gen == gen`) → gain は変えず (旧トラックの値のまま)、suspension を解く。
   - generation が古くなった (より新しい選択が来た) → その結果を捨てる。新しい選択の 1〜3 が引き継ぐ。
4. Norm OFF のときは何もしない (gain は unity のまま)。

- snap: 既存の `NormalizeGainRamp` は手動 ON/OFF・仮→確定の差を 4 秒 ramp する。トラック切り替えでは別の音源に
  なるので ramp しない。`AvClock` の normalize gain 更新に「snap」種別を足し、pump は次の chunk から新 gain を
  そのまま使う。既存の ramp 経路は変えない。
- 「トラック確定待ち」は既存の Norm 状態 enum の 1 状態として足し、App に別の bool / Option を足さない。
- 自動 scan 抑止 (cancel / 失敗後) は stream 単位で持つ。あるトラックの抑止は別トラックに及ばない。

### 6.2 seek strip 波形・音声モードの解析

波形と音楽解析は、decoder の入口だけでなく**結果を保存・再利用するすべての key** がファイル単位なので、
その全部に stream index を通す。1 か所でも漏れると別トラックの結果が再利用される。

| 対象 | 現状の key | 変更 |
|---|---|---|
| decode 入口 `audio_decode::AudioRangeDecoder::open` / 音楽解析の `decode_audio_file_progressive` 等 | 内部で `best(Audio)` | stream index を必須引数にし、呼び出し側は player の `applied.stream` を渡す |
| 波形 session identity (owner fs index / 動画パス / source epoch / items generation) と holdover | 同左 | stream index を加える |
| `WaveFileIdentity` (path / size / mtime、`seek_strip_wave.rs:49`) と worker 内 LRU | ファイル単位 | stream index を加える |
| 波形の永続キャッシュ `video_wave_chunks` (`tile_thumb_cache.rs:194`、リリース済み) | path / bin 幅 / chunk / mtime / size | Norm と同じ規則: 新規保存は stream index を持つ追加テーブルへ。読み出しは追加テーブル優先、既定トラックに限り旧テーブルを後方互換で読む |
| 音楽解析 LRU `MusicAnalysisKey` (path / size / mtime、`app.rs:11241`)、進行中結果、spectrum PCM、seek strip への完成解析受け渡し | ファイル単位 | stream index を加える |

- 切り替え中 (desired ≠ applied) は旧トラックの波形・解析を表示し続け、`applied` が変わった時点で既存の
  identity 不一致経路で worker を作り直す。失敗時 (routing は旧トラック) は `applied` を見るので旧トラックのまま正しい。
- 実装者は上表以外にファイル単位の key で音声解析結果を保存・再利用している箇所が無いかを grep で確認し、
  あれば同じ扱いにして報告する。

## 7. ライフサイクル上の扱い

### 7.1 再生中 / 一時停止中 / seek 直後

§5.1 のとおり、play intent を保って seek を 1 回発行するだけ。seek 直後 (前の seek がまだ表示されていない、
または coalesce 待ちの pending target がある) は、その target を基準位置にして即時に seek する。demux がまだ前の
seek を取り出していなければ要求は上書きされ (latest-value)、取り出した後なら次の seek として処理される。
どちらでも切り替えは最後の seek の取り出し時に反映される。

### 7.2 連続切り替え

§5.4。seek 要求は latest-value で上書きされ、`AudioSetup` の構築は demux が取り出した要求 1 回ごとに最大 1 回。

### 7.3 再生終了 (EOF) と重なった場合

- 判定は engine の `Eof` ではなく demux の末尾到達 (`clock.is_eof_reached()`) で行う。demux の EOF 通知
  (`decoder.rs:3318`) から engine の `Eof` 確定 (`mod.rs:10874` 以降、末尾音声の drain と quiet 判定の後) までの
  間も「末尾」として扱う。この間に seek すると末尾の音声を切り、末尾への seek は既存の「シーク中...固着」経路
  (`seek_eof_stuck_since`) も踏むため。
- 末尾到達中に選択された場合、seek は発行せず `desired` だけ更新する (表示は「切り替え中」)。
- 一時停止中 (frame-step pause を含む) で demux が末尾に達していない場合は、通常どおり表示中の PTS へ seek する
  (末尾付近でも、利用者がその位置へ seek したのと同じ扱い)。
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
  - ParkedLive の窓では、HUD command は source epoch の検査 (`native_video.rs:5459`) より前に活性化へ変換される
    (`native_video.rs:5360`)。変換後は command 自体を実行しないので、旧 source の選択 event が別の player の
    stream を切り替えることは無い。活性化は「利用者がその窓の HUD をクリックした」事実への応答で、source epoch に
    依らず正しい。この順序は既存の全 HUD command に共通で、本機能では変えない。
  - 活性化でない通常経路では、`SelectAudioTrack` は source epoch 検査の**後**で処理する。`NavigateItem` のような
    epoch 不一致の許容例外には入れない (stream index は source ごとの値で、旧 source の index を新 source に
    適用してはならない)。handler はさらに `stream_index` が現在の player の `audio_tracks` に含まれることを確認する
    (§5.1 の `Rejected`)。
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
  App の handler → (Norm が ON なら §6.1 の Norm owner の手順を通して) `VideoPlayer::select_audio_track`。
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

- 最初に §5.3 (packet の世代番号を demux の処理済み seek 世代から付ける) を単独で入れ、既存の seek テストを
  通してからコミットする (既存 seek 全体に効く修正のため)。
- 続いて `AudioTrackSelection`、`VideoPlayer::select_audio_track`、demux の差し替え (§5.2 の確定順序)、
  `Flush.replace_setup`、失敗経路 (SetupFailed / SeekFailed / WorkerGone)、末尾の保留 (§7.3)、構築時間の perf event。
- この段では UI から呼ばない (§5 の API とテストだけ)。
- テスト (lib、実 decoder を headless で動かす。GPU は使わない):
  - 切り替え後に pump / `AudioFrame` へ届く音声の周波数が新トラックのもの (零交差数で判定)、serial が新しい。
  - 旧トラックの周波数を持つ frame が切り替え後の世代に 1 つも無い。
  - **serial 公開前の packet 競合**: `request_seek` が serial を進めた後、要求の公開前に demux が旧 routing で
    packet を読む割り込み順を、テスト用 seam で固定して再現し、その packet が新世代として decode されないこと。
  - 一時停止中の切り替えで一時停止が保たれ、位置が変わらない。frame-step pause 中も同じ。
  - seek 直後 (前の seek 未表示・coalesce 待ち) の切り替え、連続 3 回の切り替えで最後の選択だけが `applied` になる。
  - 切り替えの seek の後に通常 seek を重ねても切り替えが反映される。frame-step seek・ループの先頭 seek でも同じ。
  - 末尾到達中 (demux EOF 済み・engine の `Eof` 確定前の drain 中を含む) の選択は seek を出さず、次の seek で反映される。
  - `AudioSetup` 構築失敗・`av_seek_frame` 失敗・audio `Flush` 送信失敗 (それぞれテスト用の注入 seam) で routing と
    `applied` が変わらず、失敗が desired.gen と理由つきで記録され、後から来た古い失敗が新しい選択の表示を上書きしない。
  - 速度・音量・mute が切り替えで変わらない。
  - 異なる sample rate / channel 数 / time base のトラック間 (素材の 3 本) で、切り替え後の audio PTS と
    A/V clock が連続している (切り替え前後の位置差が seek 誤差の範囲)。
  - 導出状態 (§4.2) の純粋関数テスト。
- 作ったテストのうち周波数判定・serial 競合・most-recent-wins・失敗 generation は、対象処理を一時的に外すと
  落ちることを実装者が確かめ、報告に書く。

### S3: 解析系の追従

UI より先に入れる (UI から切り替えられるようになった時点で、Norm と波形が正しいトラックを見ているようにする)。

- 6.1 (Norm: 追加テーブル、scanner の stream 指定、App の Norm 状態の key 拡張、トラック確定待ち、snap、
  worker での lookup) と 6.2 (波形・音楽解析の全 key への stream index)。
- テスト: DB の新旧テーブルの読み分け (新規保存は追加テーブル、既定トラックだけ旧行を読む、非既定では旧行を
  読まない)・`clear_all`/`count`、scanner が指定 stream を測る (sine の振幅をトラックごとに変えて LUFS 差で判定)、
  Norm の確定待ち (成功 → snap、未測定 → 既存経路、失敗 → 旧 gain のまま解除、古い generation の結果を捨てる)、
  scan 中のトラック変更で旧 scan が cancel される、抑止が stream 単位、波形・音楽解析の key に stream index が
  入り別トラックの結果が再利用されない (永続キャッシュ・LRU とも)。

### S4: UI と操作

- 8.1〜8.4。`NativeVideoOutputEvent` の追加と ParkedLive 分類 (§7.6、detached-rework-plan §11 に記録)。
- テスト: App handler-level (event → Norm owner → `select_audio_track`、fs_idx 不一致で無視、source epoch 不一致で
  無視、`stream_index` が現在の player に無ければ無視、ParkedLive で活性化扱い)、失敗トーストが generation ごとに
  1 回、行ラベル生成、keymap の表と ini の整合 (既存テスト)、UI スナップショット (音声モード HUD の選択 UI)。

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

## 12. 判断済みの事項

1. 同じトラックのままの seek では resampler を reset しない (既存 seek と同じ)。トラックを替える seek では
   `AudioSetup` ごと差し替えるので、旧トラックのサンプルは swr の delay にも残らない。
2. Norm の測定値 lookup は UI thread で行わない (§6.1 の worker)。既存の open 時 lookup も UI thread で
   `std::fs::metadata` を伴うが、それは本機能の範囲外の既存事項として backlog に記録する。
3. `AudioSetup` の構築は demux thread で行い、所要時間を perf event で計測する (S2 の受け入れで素材ごとに記録)。
4. 切り替えの seek で HUD の「シーク中...」が出る場合があるが、通常の seek と同じ表示のままにする。
