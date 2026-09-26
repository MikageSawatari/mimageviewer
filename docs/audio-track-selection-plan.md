# 動画の複数音声トラック選択 設計 (backlog §1.251)

- 状態: 第7版で独立レビュー ACCEPT (2026-09-27)。実装は §10 の段階順に進める
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
- **選択の記憶** (§9): ファイルごとに選んだトラックを保存し、PC で開き直したときも Remote で続きを見るときも
  同じトラックで始める (利用者決定 2026-09-26)。
- **mIV Remote** (§9A): Remote の配信も選択中のトラックで始め、Remote 側でもトラックを選べるようにする
  (利用者決定 2026-09-26。「家で見た続きを外出先で見るとき、聴くべきトラックが聴けないと体験が悪い」)。

対象外 (本設計では変更しない。§11 に理由):

- 詳細表示 (一覧の詳細列・`probe_audio_details`) はファイル単位の情報として既定トラックのまま。
- 字幕。字幕機能は存在しない。本機能の状態は字幕と共有しない。
- 開いた時点で既定トラックの音声初期化に失敗した動画での、別トラックへの切り替え (§7.4)。

範囲に加えたもの (利用者決定 2026-09-26):

- **Remote で見た再生位置の PC への書き戻し** (§9B)。そのために、Remote 接続を受け付けた時点で PC の閲覧
  ウィンドウをすべて閉じる。

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
    /// 音声 stream の中での 1 始まりの順番 (表示用。decoder の無い stream も数える = 他のプレイヤーの番号と揃う)。
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
pub default_audio_stream_index: Option<usize>, // 常に best(Audio) の結果 (実際に開いたかどうかとは無関係)
pub opened_audio_stream_index: Option<usize>,  // demux が open 時に実際に開いたトラック (音声出力 device の成否とは無関係)
```

- 列挙は demux thread が open 時に行う (`info_tx` 送出の前)。decoder が見つからない stream は列挙しない
  (backlog の「再生可能な音声 stream」)。
- 取得できない情報は `None` にする。言語名への変換 (「jpn」→「日本語」) は UI 層の固定表で行い、表に無い
  code はそのまま表示する。推測で埋めない。
- 「既定トラック」は常に `best(Audio)` の結果を指す (表示の「(既定)」と Norm の旧テーブル互換読みに使う)。
  実際に開いた / 鳴っているトラックは selection の `applied` で表す (§4.2)。
- 既存の `audio_codec` / `audio_bit_rate_bps` は、実装どおり「open 時に開いた `AudioSetup` のトラック」の値として
  残す (保存された選択で非既定トラックを開いた場合はそのトラックの値)。HUD の右パネルは `applied` のトラックの
  情報を `audio_tracks` から引く (§8.3)。
- 初期トラックの決定は純粋関数 `resolve_initial_audio_track(tracks, default, saved_choice) -> Option<usize>` に
  置き、demux (ローカル再生) と Remote の配信開始 (§9A) の両方がこれを使う。音声出力 device の成否に依存しない。

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

- 初期値: `desired = applied = (0, 開いたトラック)`。開いたトラックは §9.3 の保存された選択に一致したもの、
  無ければ `default_audio_stream_index`。音声が無い / 音声初期化に失敗した player は
  selection を持たない (`Option<Arc<...>>` = None)。
- 書き手: `desired` は UI thread (`VideoPlayer::select_audio_track`) だけ、`applied` / `last_failure` は
  demux thread だけ。1 つの `Mutex` で守る (保持区間は値のコピーのみ)。
- 読み手: HUD / metadata / App は `snapshot()` で値をコピーして読む。

表示上の状態は snapshot から導出し、別 flag を持たない:

| 導出状態 | 条件 |
|---|---|
| 確定 | `desired.gen == applied.gen` |
| 保留 | `desired.gen > applied.gen` で、player が末尾の保留条件 (§7.3) にある |
| 切り替え中 | `desired.gen > applied.gen` かつ `last_failure.gen != desired.gen` かつ保留でない |
| 失敗 | `last_failure.gen == desired.gen` (routing は `applied` のまま) |

## 5. 切り替えの手順

### 5.1 UI thread (`VideoPlayer::select_audio_track(stream_index)`)

1. `stream_index` が `audio_tracks` に無い、または selection が無い player なら何もせず `Rejected` を返す。
2. `desired.stream == stream_index` かつ失敗状態でないなら no-op (`Unchanged`)。
3. `desired = (desired.gen + 1, stream_index)` を書く。Norm ON で `stream_index` が未解決なら、表の既定値
   `Pending` が seek の公開より前から効いている (§6.1 の遷移 1)。
4. 末尾の保留条件 (§7.3) に当たるなら seek は発行しない。`Deferred` を返す。
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

切り替えを試みる条件は `desired.gen > applied.gen` **かつ `last_failure.gen != desired.gen`**。一度失敗した選択は、
次に利用者が選択し直す (generation が進む) まで、後続の通常 seek で自動再試行しない (失敗表示中のトラックへ
別位置の seek で突然切り替わることを防ぐ)。

1. 上の条件を満たし、`desired.stream != applied.stream` なら、`input.stream(desired.stream)` から
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
   - 送信失敗 (audio decode thread が終了している): 切り替えの失敗ではなく、§5.3.1 の「音声 lane の喪失」として
     扱う (routing を外し、映像があれば映像を続ける)。selection には `last_failure = {.., reason: WorkerGone}` を
     記録し、以後の切り替えは `Rejected` にする。
4. 条件を満たし `desired.stream == applied.stream` (元のトラックへ戻した等) は `AudioSetup` を
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
付ける。更新規則:

- demux が seek 要求を処理し、**存在する decode lane すべて**の `Flush` の送信が受理されたら、**seek の成否に
  かかわらず** `demux_serial` をその要求の serial に進める。video が無い (audio-only) なら audio の `Flush` だけ、
  audio が無いなら video の `Flush` だけが条件。seek 失敗時も既存どおり trim なしの `Flush` を送って再開するので、
  同じ規則で進める。
- `Flush` の送信失敗は、cancel が立っていれば既存の終端 (`break 'outer`)。cancel が無い audio lane の切断は
  §5.3.1 の「音声 lane の喪失」として扱い、存在する lane (= video) の `Flush` が受理されれば `demux_serial` を進める。
- `SeekCompleted` の再送 (`pending_seek_completed`) は event lane 側の仕組みで、`demux_serial` とは独立。
- 初期値は open 時の serial (0)。

- seek 要求の公開前に読まれた packet は旧 `demux_serial` を持ち、decode thread の既存判定
  (`serial != current || serial != live` で破棄、`decoder.rs:5613`) で捨てられる。
- `demux_serial` の新しい packet は、同じ世代の `Flush` を control channel へ送った後にしか queue に入らない。
  decode thread は control を優先受信する (`select_biased!`) ので、新世代の packet を見る前に必ず `Flush` を処理する。
- video 側も同じ規則にする (旧位置の frame が新世代として表示される同型の競合を同時に塞ぐ)。
- この変更は既存の seek 全体に効くので、S2 の最初に単独で入れ、既存の seek テストを通してから切り替えを載せる。

### 5.3.1 音声 lane の喪失 (cancel を伴わない audio の切断)

- 現状、PC の音声出力を開けないと `VideoPlayer::open` は `audio_rx` を捨てて「映像のみ再生」にする
  (`mod.rs:8728-8754`) が、audio decode thread は `audio_tx` の切断で終わり、demux は次の音声 packet の送信で切断を
  `Cancelled` と扱って終了する (`decoder.rs` の `send_audio_packet_with_video_drain`)。音声が続く素材では映像も止まる
  既存の不具合で、§5.3 で audio `Flush` の失敗を終端にすると seek でも同じ停止が起きる (S2 区切り A の独立レビュー P1)。
- 規則: **cancel が立っていない audio lane の切断 (packet 送信・`Flush` 送信のどちらでも) は「音声 lane の喪失」とし、
  demux は音声の routing を外して (`audio_stream_idx_for_demux = None` 相当) 映像を続ける。映像 lane が無い素材
  (音声のみ) では終端。** cancel が立っている場合は従来どおり終端。
- 音声 lane を外した後は、以後の seek の `Flush` 条件も video だけになる (§5.3 の「存在する lane」)。
- engine への通知: 音声 lane を外した demux は、engine の既存の `AudioEvent::AudioInactive` 遷移 (`actor.rs:697-704`、
  現在は本番の送り手が無い) へ event lane 経由で通知する。event lane が満杯なら `SeekCompleted` と同じく保留して
  再送し、取りこぼさない。engine は `has_audio=false` として readiness を映像だけに作り直し、以後の seek で
  `BufferReady` を待たない。`AvClock` も既存の `mark_audio_inactive` で wall clock に移す。
- 音声出力側の後始末: audio pump は入力 channel の切断で終わるとき、`raw_pending` / processed の残りを破棄し、
  音声 buffer の会計 (`AudioBookkeeping`) を 0 として公表する。残量が残ったままだと EOF・ループの quiet 判定
  (`mod.rs:10910-10922`) が成立しない。
- テストは実 engine を通す: 再生中に音声 lane を失った後の seek が Playing に戻る (Buffering に固着しない)、
  event lane が満杯のときも `AudioInactive` が届く、`raw_pending` が残った状態から EOF / ループへ進む。
- 音声出力を開けなかった player は selection を持たない (§7.4) ので、切り替えは起きない。切り替え中に audio decode
  thread が終わった場合は §5.2 の `WorkerGone`。

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
| Norm gain | 追加: §6.1。gain はトラックごとの値で、pump が frame のトラックで選ぶ。未決定のトラックの frame だけ processed にせず、トラックが替わる境界では ramp せず snap する |
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

#### 切り替え時の gain — トラックごとの値として pump が選ぶ

gain は「その音声がどのトラックのものか」で決まる値にする。App が「現在の gain」を 1 つ書き換える方式
(第2版) は、切り替えの成否・末尾の drain・再選択・旧 scan の完了と、グローバルな gain / preroll suspension の
所有が絡み、解除漏れや誤解除の経路が残る (第2回レビュー P1)。そこで、gain をトラックごとの値として持ち、
**pump が処理中の frame のトラックに応じて選ぶ**。

- `AudioFrame` に `stream_index` を持たせる (audio decode thread が現在の `AudioSetup.stream_idx` を付ける)。
- player ごとに Norm gain 表 `NormalizeGainTable` を持つ (`AvClock` の単一 `normalize_gain_bits` を置き換える。
  同じ意味を 2 か所に持たない)。値はトラックごとに:

  | 状態 | 意味 | pump の扱い |
  |---|---|---|
  | `Pending` | このトラックの gain がまだ決まっていない | **このトラックの frame だけ** processed にしない (raw に保持)。他のトラックの frame は通常どおり処理する |
  | `Gain(g)` | 測定済み (確定 / 仮)、未測定で unity と決めた、または Norm OFF の unity | g |

  表は「明示の値」と「表に無いトラックの既定値」を持つ。既定値は Norm OFF なら `Gain(1.0)`、Norm ON なら `Pending`。
  これにより、demux がどのトラックを開くかに関係なく、Norm ON のとき**まだ解決していないトラックの最初の frame は
  必ず止まる** (open 時・切り替え時とも、demux や選択 API が Pending を書く順序に依存しない)。

- snap と ramp:
  - 処理する frame のトラックが直前に処理したトラックと異なるとき、ramp せずにそのトラックの gain へ snap する。
  - あるトラックが `Pending` から `Gain` になって最初に処理する frame も snap する。既存の「scan 待ち
    (グローバル suspension) の解除時は最初の gain へ snap する」(`audio.rs:1535-1547`) もそのまま維持する。
  - それ以外の同じトラックの gain 変更 (手動 ON/OFF、仮 → 確定) は既存どおり 4 秒 ramp。
- `Pending` のトラックの frame が raw の先頭を塞いでいる間は `BufferReady` を出さない。既存の pump は processed が
  0 でも demux が EOF なら `BufferReady` を出す (`audio.rs:1821-1842`) が、raw の先頭が `Pending` で止まっている
  間はこの EOF 例外も抑止する (短い素材の末尾で待機を抜けて、未処理の音を捨てたまま Playing にしない)。
  旧トラックの frame (末尾の drain、切り替え失敗時の再開) は影響を受けない。
- `Pending` で raw に保持する量は既存の raw 先読み上限 (5 秒) に従い、上限に達すれば既存どおり bounded queue の
  back-pressure が decoder / demux へ返る (新しい上限は作らない)。
- 既存のグローバル `audio_preroll_suspended` と未測定 scan の仕組みは、そのまま「scan 待ち」専用として残す。
  トラック切り替えのためにグローバル suspension は使わない。

遷移 (表の書き込みは player の API 経由だけ。lookup の起動・結果の適用は App の Norm owner):

1. **切り替え時。** 表に S の明示の値が無ければ、Norm ON の既定値 `Pending` がそのまま効く (seek の公開より前から
   止まっている。既定値は最初の frame の gate)。`VideoPlayer::select_audio_track(S)` は戻り値に「S が未解決か」を
   含め、App はそれを見て lookup を起動する。
   - lookup を起動するとき、App は表の S に明示の `Pending{request}` を書く (トラックごとの最新の要求と、実行中で
     あることの記録)。S に既に実行中の `Pending{request}` があれば新しい lookup は起動しない (A → B → A のように
     同じトラックを選び直しても lookup は重複しない)。
   - S が既に解決済み (`Gain`) なら lookup しない。再選択・`Unchanged` で表は変わらない (何も解除しない)。
   - `Deferred` (末尾) でも同じ (S の frame は次の seek まで来ないので、旧トラックの drain は止まらない)。
2. **open 時も同じ規則にする (動画と音声ファイル / 音楽ビューの両方)。** Norm ON で開いた player の表は既定値
   `Pending` から始まるので、開いたトラックの最初の frame は解決まで止まる。App は `VideoInfo` を受け取った時点で、
   demux が実際に開いたトラック (`VideoInfo.opened_audio_stream_index`、§4.1) の lookup を起動する。
   既存の open 時の同期 lookup・「未測定なら autoplay を一時 false」・初期 gain の受け渡しは、動画の入口
   (`app.rs:58971-58990`) と音声ファイル / 音楽ビューの入口 (`app.rs:59350-59390`) の両方で、この非同期解決に
   置き換える (open 時にはまだどのトラックを開くか確定していないため、path 単位の lookup では保存された非既定
   トラックに正しい gain を結び付けられない)。置き換え後の遷移は、既存の Norm のテスト (open 時の測定済み即時
   適用・未測定の deferred scan・キャッシュ hit の grid 再開・音楽ビュー) の期待と対応付けて維持を示す。
   保存したトラックを開けず既定トラックで開き直した場合も、実際に開いたトラックについて同じ規則で解決する。
3. **lookup の要求と結果の照合。** `request = (table_epoch, request_seq, stream, target_lufs_milli)`。
   `table_epoch` は Norm の ON / OFF で表を作り直すたびに進む。lookup 結果は `request` をそのまま持って返り、App は
   player の表の S の値が**同じ `request` の `Pending{request}` と完全に一致するときだけ**適用する。OFF → ON で
   作り直された表、別の目標 LUFS、後から起動された別の要求、すでに scan 等で解決された値には適用しない
   (古い lookup が後から完了しても新しい値を上書きしない)。
   適用内容:
   - 測定済み → S = `Gain(g)`。
   - 未測定 → 既存の未測定経路に S を渡す。scan を始める条件 (play intent・fullscreen・S の自動 scan 抑止なし・
     S の UI 状態が `OnUnmeasured`) を満たす場合は、既存どおりグローバル preroll suspension を立てて scan を
     始めてから S = `Gain(1.0)` にする (suspension が先なので unity の音は出ない)。scan を始めない / 始められない
     (一時停止中・抑止中・開始失敗) 場合は S = `Gain(1.0)` にし、S の UI 状態を `OnUnmeasured` にする。
     「トラック確定待ち」から既存経路へ渡すため、S の UI 状態は lookup 結果の適用時にまず `OnUnmeasured` に
     してから既存の開始条件を評価する。
   - lookup 失敗 (I/O エラー等) → ログを残して S = `Gain(1.0)`、UI 状態は `OnUnmeasured`。
   どの分岐でも S は `Pending` のまま残らない。
4. scan の完了・仮結果は、その scan の対象トラック (scan state が持つ stream index) の値だけを更新する。
   別トラックの値・再生 intent は触らない (既存の「仮 gain 適用後のバックグラウンド scan は再生 intent を
   再度横取りしない」も維持)。
5. 選択されたトラックと別のトラックを対象にした**ブロッキング中の** scan (仮結果の前、グローバル suspension を
   持っている) は、選択時に既存の cancel 経路で止める (既存の cancel 経路が再生 intent と suspension を戻す)。
   仮結果適用後のバックグラウンド scan は続けてよい (完了は 4 のとおり対象トラックの値だけを更新する)。
6. Norm 全体 OFF: 明示の値を空にし、既定値を `Gain(1.0)` にして `table_epoch` を進める (`Pending` も解消)。
   Norm 全体 ON (再生の途中): **既存の ON 経路をそのまま使う**。鳴っているトラック (`applied`) は既知なので、
   既存どおり同じ操作の中で測定値を引き、測定済みなら ramp で適用、未測定なら同じ操作の中で scan を始める
   (再生中なら preroll を止めて一時停止する既存の挙動)。書き込み先が単一の gain から「表の `applied` の値」に
   なるだけで、未補正の音が流れる時間は現状から増えない。その後、既定値を `Pending` にして `table_epoch` を進める
   (他のトラックは切り替え時に解決する)。この lookup は既存の ON 操作の UI thread I/O で、本機能で新たに足す
   ものではない (§12)。

- 「トラック確定待ち」のための App の bool / Option は足さない。状態は player の表と、既存の per-fs_idx Norm 状態
  (stream index を key に含めたもの) だけにある。

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

保留 (`Deferred`) にするのは次の 2 つだけ:

| 状態 | 判定 | 扱い |
|---|---|---|
| 再生中の末尾 drain | demux が末尾に達し (`clock.is_eof_reached()`)、再生 intent がある (engine の `Eof` 確定前) | 保留。この間に seek すると末尾の音声を切り、既存の「シーク中...固着」経路 (`seek_eof_stuck_since`) も踏む。drain は旧トラックのまま完了させる |
| 末尾で停止済み | engine の published state が `Eof` | 保留。末尾への seek は同じ固着経路を踏む |

- 一時停止中 (frame-step pause を含む) は、demux が末尾に達していても保留しない。表示中の PTS へ一時停止の
  まま seek して即時に反映する (一時停止中は drain が進まず engine の `Eof` も確定しないので、保留すると
  次の seek まで無期限に待つことになる)。
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

- selection は `VideoPlayer` が所有し、player の drop とともに消える。source swap で作られる新 player は §9.3 の
  保存された選択 (無ければ既定トラック) から始まる。
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
  しか取れていないので、`applied` が open 時に開いたトラックのときだけ表示する。
- トラックが 2 本以上ある場合は「音声トラック: N 本」を添える。

### 8.4 keymap

- `KeyAction::VideoNextAudioTrack` (FsVideo、既定キーなし、`ChordList::EMPTY`、ini は `# VideoNextAudioTrack = none`)。
  次の ordinal へ循環し、切り替えたらトーストで新しいトラックのラベルを出す。トラックが 1 本以下なら何もしない。
- native VK 経路 (`dispatch_native_video_key_event`) と egui fallback (`handle_video_input`) の両方に配線する。
- `ini_name()` / `description()` / `context()` / `trigger()` / `default_chords()` / `ALL_ACTIONS` /
  `docs/keymap.ini.default` / `docs/keymap-spec.md` を揃える。

## 9. 選択の記憶 (ファイルごと)

利用者決定 (2026-09-26): 選んだトラックを再生位置と同じくファイルごとに保存し、PC で開き直したときも、
Remote で続きを見るときも、同じトラックで始める。

### 9.1 保存形式 (`settings.db`)

- 新テーブル `video_audio_track_choices (path_normalized TEXT PRIMARY KEY, stream_index INTEGER NOT NULL,
  codec TEXT NOT NULL, language TEXT, channels INTEGER, title TEXT, updated_at INTEGER NOT NULL)`
  (`CREATE TABLE IF NOT EXISTS`)。`Settings.video_audio_track_choices: HashMap<String, AudioTrackChoice>`、
  key は再生位置と同じ `adjustment_db::normalize_path`。
- 再生位置の表 (`video_resume_positions`) の列にしない理由: 再生位置は先頭 3 秒未満・末尾 5 秒以内・EOF で
  行ごと消える (`save_video_resume_position`)。トラックの選択は見終わった後も (次に開いたときも) 有効であるべきで、
  寿命が違う。
- 読み書きは再生位置と同じ `COMPLEX_FIELDS` 方式 (起動時に全件読み込み、`save_full` で DELETE + 明示列 INSERT)。
  新しい表なので既存データの移行は無い。
- 旧版との関係: リリース済みの v4.1.0 と開発中の版は同じ版番号なので、`reject_newer_app_version` による旧版の
  起動拒否には頼れない。旧版がこの DB を開いても安全である根拠は、旧版の settings.db 処理が既知の表だけを明示列で
  読み書きし、知らない表には触れないこと (実装者がコードで確認し、S5 の報告に根拠の行を示す)。
- 件数の上限は再生位置と同じく設けない。環境設定「保存済み位置の管理」の「動画・音声の再生位置をすべてクリア」
  で、この表も一緒にクリアする (文言も「再生位置と音声トラックの選択」に合わせる)。
- path のライフサイクルは再生位置の map と同じ owner で処理する: ファイル削除時の破棄
  (`purge_video_resume_positions_for_removed_paths`、`app.rs:32641`) とリネーム時の移行
  (`migrate_video_resume_positions_for_renamed_path`、`app.rs:34820`) に、この map も含める。

### 9.2 いつ保存するか

保存の契機は「利用者の明示の選択が成功したとき」で、次の 2 つ (書き手は core の UI thread の App だけ。Remote plan
§2.1 の不変条件どおり):

- `select_audio_track` が `Requested` を返した選択が、その generation で `applied` になったとき
  (App は毎 tick、player の selection snapshot の `applied.gen` を「最後に保存した generation」と比べる。最後に保存
  した generation は player が持つ)。失敗・保留中・古い generation は保存しない。
  - **player の退役境界でも収穫する。** 確定の直後、次の tick の前に player が閉じられる (close・evict・source swap・
    teardown・Remote 取得時の全閉じ) と tick の収穫を逃す。player を退役させる経路はすべて再生位置を保存している
    (§9B.1 の書き手の一覧) ので、再生位置の保存の単位 (`MediaResumeUpdate` と teardown の plan) に「確定済みで
    未保存の選択」を載せ、同じ書き手で保存する。
- `select_audio_track` が `Unchanged` を返したとき (いま鳴っているトラックを明示的に選び直した。既定トラックを
  選び直した場合を含む)。その時点の `applied` のトラックを保存する。
- Remote での選択は §9A.3 の確定時点で保存する。
- メモリ上の map を更新し、ディスクへは既存どおり次の `Settings::save()` で書く。

### 9.3 開くときの初期トラック

- `VideoPlayer::open` に `initial_audio_track: Option<AudioTrackChoice>` を渡す。demux は open 時にトラックを
  列挙した後、`resolve_initial_audio_track` (§4.1) で開くトラックを決める。
- 一致規則: 保存された選択の `stream_index` のトラックがあり、codec が同じで、保存時に language / channels /
  title があればそれぞれ同じこと。保存時に無かった項目は比較しない。
  - ファイル identity (size / mtime) は比較しない。再生位置も path だけで引いており、同じ構成のファイル (再
    エンコード等) で同じ index・codec・言語・チャンネル数・title を持つトラックは、利用者にとって同じトラックと
    みなしてよい。構成が変わった場合は上の項目のいずれかが変わって一致しない。
- 一致しない / 無い → 既定トラック (`best(Audio)`) で開く。保存された行は消さない (次の選択で上書きされる)。
- 一致したが開けなかった (decoder open 失敗) → 既定トラックで開き直す。これは切り替えの失敗 (`last_failure`) とは
  別の、open 時 1 回だけの通知 `open_notice: Option<SavedTrackUnavailable>` として selection に持つ (初期状態
  `desired = applied = (0, 既定)` は「確定」で、失敗表示とは重ならない)。App はこれを 1 回トーストで通知する。
- `applied` の初期値は実際に開いたトラック。
- source swap・通常 open・タイル・遅延 open・Remote の headless player のすべてが `build_video_player_for_open` を
  通るので、そこで表を引いて渡す (メモリ上の HashMap の参照だけで I/O は無い)。

## 9A. mIV Remote

### 9A.1 現状

- Remote の動画は core の時計なし transcode (`clockless_transcode.rs`) が再生器とは別にファイルを開き、
  `best(Audio)` の 1 本だけを AAC にして fMP4 / HLS で配る (`clockless_transcode.rs:1522-1526`)。PC と Remote の
  トラックは今は偶然一致しているだけ。
- 開始位置は PC 側の再生位置の表から取る (headless player 経由)。Remote から再生位置は書き戻さない (§9B で対応)。
- 画質変更は `RemoteVideoStreamingSession::change_quality` が現在位置から新しい generation の transcode を始める。
  配信リソース (playlist / segment) の古い generation への要求は 409 で拒否される (`session.rs:440-448`)。
  一方、control 自体は session ID だけで照合され generation を見ない (`http.rs:1495-1500`、`ui.rs:1731-1744`)。
- protocol は `crates/remote-ipc` の `PROTOCOL_VERSION = 61` で、完全一致を要求する。

### 9A.2 開始時

- transcode の選択: `ClocklessTranscodeOptions` に `audio_stream_index: usize` を足し、`best(Audio)` の独自選択を
  やめる。値は session が持つ「選択中トラック」。
- 開始時の選択中トラック: headless player の `VideoInfo.opened_audio_stream_index` (demux が保存された選択と
  `resolve_initial_audio_track` で決め、開けなければ既定で開き直した結果) を使う。headless player の selection
  (音声出力 device の成否に依存する) は使わない。PC に音声出力 device が無くても Remote は選択どおりに始まる。
  start の要求に `audio_track: Option<usize>` があれば (§9A.3 の 409 後の再開)、それが `audio_tracks` に含まれる
  ときに限り保存された選択より優先する。
- `VideoStreamStartPayload` / `VideoStreamStatePayload` に `audio_tracks: Vec<RemoteAudioTrack>` と
  `audio_track: Option<usize>` (選択中の stream_index) を足す。`RemoteAudioTrack` は stream_index と、core が作った
  表示ラベル `label` (§8.1 と同じ関数) と `is_default`。client は言語表などを持たない (表示文言の owner を core の
  1 つにする)。
- protocol version を 62 へ上げる (payload・control action・start の要求の追加)。

### 9A.3 Remote での切り替え

- `VideoStreamControlAction::AudioTrack { stream_index, position_secs, expected_generation }` を足す。
- core の `apply_remote_video_control` (UI thread) は、session ID に加えて `expected_generation` が session の現在の
  generation と一致することを確かめ、一致しなければ `stream_generation_mismatch` (409) で拒否する。遅れて届いた
  古い操作が新しい選択を上書きしない。`stream_index` がその stream の `audio_tracks` に含まれることも確かめる。
- 検証を通ったら session の `change_audio_track(stream_index, position_secs)` を呼ぶ。`change_quality` と同じ形で、
  選択中トラックを更新して新 generation を始め、始められなければ元に戻す。
- **選択の確定と記憶は、その generation が Ready になり、その generation の音声 stream が選択と一致した時点**。
  `start_new_generation` は worker を spawn した時点で成功を返し、stream の open・Ready は後から起こるため、
  spawn の成功では記憶しない。確定前に次の切り替えで置き換えられた generation、失敗した generation は記憶しない。
  generation が失敗した場合は既存の配信失敗の扱い (配信の停止と client のエラー表示) に従い、利用者が再開すると
  最後に確定したトラック (記憶された選択) で始まる。
- client は切り替えの操作に番号を付け、最新の操作以外の応答 (成功・409 とも) を捨てる。
- 409 のとき (session が入れ替わった等) は、画質変更と同じく `restartAt` で新しい session を始めるが、start の要求に
  選んだ `audio_track` を渡す (§9A.2)。現行の start は quality しか渡さないので追加する。
- 古い generation の transcode worker と新しい worker は既存の process-wide resource lease で直列化される。
- Remote の Norm gain: 現在は session 開始時に path 単位で 1 回引いた値を全 generation が使う
  (`app.rs:58971-58982`)。これをトラック単位にする。gain の解決は **generation の worker の中で** 行う
  (UI thread で I/O しない)。worker は App が持つ `AudioNormalizeDb` の接続とは別の読み取り専用接続を開き、
  session 開始時に取った設定の snapshot (Norm が全体 ON か、目標 LUFS) を使う。規則は §6.1 の DB 規則 (追加
  テーブル優先、既定トラックだけ旧テーブル互換読み)。未測定なら unity (Remote は scan しない現状どおり)。
- client (`crates/remote-web/web/video-stream.mjs`): 「操作」ページの画質の並びの隣にトラックの並びを置く
  (トラックが 2 本以上のときだけ)。押すと `MEDIA_AUDIO_TRACK` command → `POST /api/video/control
  {action: "audio_track", stream_index, position_secs: currentPosition(), expected_generation}` → 既存の
  `refreshGeneration()`。選択中の表示は server の state (`audio_track`) で更新する (`setMediaState` /
  `applyServerState`)。
- PC 側: Remote の受け付け時に PC の閲覧ウィンドウはすべて閉じる (§9B.2)。所有終了後に PC で開くと、§9.3 の
  記憶された選択 (Remote で選んだもの) で始まる。

## 9B. Remote で見た再生位置の PC への書き戻し

利用者決定 (2026-09-26): Remote で見進めた位置を PC の再生位置へ書き戻す。書き戻しと競合するローカルの
閲覧状態を残さないため、**Remote 接続を受け付けた時点で、ローカルの閲覧ウィンドウをすべて閉じる**。

### 9B.1 現状の競合 (コード確認 2026-09-26)

再生位置の書き手はすべて「ローカルの `VideoPlayer` の位置」を無条件に書く (変化の有無を見ない)。Remote 所有中も
ローカルの player は一時停止のまま `fs_cache` に残るので、Remote が位置を書いても次の経路で古い位置に戻される。

- `poll_video` の 5 秒周期の保存 (`app.rs:75839-76287`): 一時停止中の player も毎回書く。Remote 所有中は
  `poll_remote_session` が 1 秒ごとに repaint するので、この保存は動き続ける。main・active detached・ParkedLive の
  各 context で動く。
- Remote 終了時の `reload_after_remote_session_release` → `close_fullscreen` → `save_all_video_resume_positions`
  (`remote_ipc/ui.rs:3650`、`app.rs:61022`)。
- detached / ParkedLive の player は Remote 終了後も残り、teardown・終了時の保存で書く。
- 静止画・本のページ位置も、Remote の既存の読書位置の書き込み (`persist_remote_reading_progress`) と、ローカルに
  残った閲覧ウィンドウの状態が同じ形で競合し得る。

個々の書き手に「Remote が触った key は書かない」条件を足す方式は、書き手が多く (上記 + teardown・トレイ・
終了・source swap・evict)、静止画側にも同型の経路があり、漏れが残りやすい。

### 9B.2 取得時にすべて閉じる — 取得 barrier の条件にする

- 既存の取得 barrier (`poll_remote_session` の `acquisition_changed` → `pause_local_progress_for_remote_session`、
  AI の静止を待って `finish_acquire`、`remote_ipc/ui.rs:908-933`) に「ローカルの閲覧を閉じ終えた」を条件として
  加える。**`finish_acquire` は、この条件が成立したフレームでだけ呼ぶ。**
- barrier の各フレームで次を行う:
  1. 初回だけ、既存どおり全 context の再生位置を保存する (`save_all_video_resume_positions` /
     `save_detached_video_resume_positions_for_exit`、確定済みの選択の収穫も含む §9.2)。Remote が書き始める前なので正しい。
  2. 取得より前に積まれた、ローカルで閲覧を開く保留中の要求をすべて失効させる: 前回の Local 復帰で残った
     fullscreen の復元 (`pending_fullscreen_restore` / `poll_remote_fullscreen_restore`)、`NativeVideoOpenPending`・
     source swap の保留、detached の遅延 open、PDF / ZIP の列挙完了で開く予定の要求など (実装者が列挙し、S7 の
     報告に一覧を示す)。
  3. 閉鎖を要求する: すべての別ウィンドウ (active detached、passive / ParkedLive、メディア窓) を
     `close_all_detached_viewers_for_mode_change` で、main のフルスクリーンを `close_fullscreen_to_completion` で。
     両 helper は、現在ログを残して続行している失敗 (active context の close 失敗、passive context の retire 失敗
     `app.rs:45903` / `46831` 等) を型付きの結果として呼び出し元へ返すようにする (既存の呼び出し元 = 表示モード
     変更の挙動は変えず、結果を使うのは取得 barrier だけ)。
  4. 閉鎖の完了を**型付きの結果**で判定する (`LocalViewerCloseState::{Closed, InProgress, Failed(reason)}`)。
     判定は helper の戻り値の「窓があったか」ではなく、実際の状態から行う:
     - root 以外の viewer context が 1 つも残っていない (画像だけを持つ context も含む。player の有無ではなく
       context の有無で見る)。
     - main の `fullscreen_idx` が無く、`fs_cache` に player が無く、fullscreen viewport の非表示が完了している
       (`ui_fullscreen.rs:21892` の完了判定)。
     - detached の runtime / window / session が無い。
     - presentation transition が遷移中でなく、積まれて未実行の transition effect も無い。
     - helper から型付きの失敗が返っていない。
     結果:
     - `Closed` → `finish_acquire` へ進む。
     - `InProgress` (transition の完了待ち等) → 次のフレームで 3 から再評価する。
     - `Failed` → 取得を中止する (既存の取得失敗の経路で Remote へエラーを返し、phase を `Local` へ戻す)。
  5. 進まない場合の保険: 既存の取得 barrier の watchdog (静止しない worker 用の 30 秒の中止経路、
     `remote_ipc/ui.rs:26` / `956`) を閉鎖待ちにも適用し、期限内に `Closed` にならなければ取得を失敗として終える。
     経過時間は「閉じた証拠」には使わず、取得を諦める判断にだけ使う (detached §2 の時間窓禁止は「時間の経過で
     状態を推定しない」ことで、この使い方とは矛盾しない)。
- **不変条件の成立時点は「取得 barrier の完了 (`finish_acquire`) 後」**。`AcquiringRemote` の間は閉鎖の途中で
  あり得る。`RemoteActive` と `DrainingRemote` の間は、ローカルに閲覧中の player・閲覧ウィンドウが存在しない。
- **共通の open 境界で所有者を検査する。** ローカルの閲覧を開く入口 (`open_fullscreen`、detached の open、音楽
  ビューの open 等。実装者が共通境界を特定する) は、Remote の phase が `Local` でなければ開かずに拒否 (ログ) する。
  これで、取得後に届いた非同期完了 (列挙完了・遅延 open) が閲覧を開き直すことはない。
- Remote 終了時: 閉じたものを復元しない。`reload_after_remote_session_release` の fullscreen 復元は、上の不変条件に
  より対象が存在しないので働かない (復元経路自体は他の用途が無ければ撤去を検討し、残す場合は Remote 所有の
  境界で失効させる)。一覧の再読み込み (外部変更の反映) は残す。次に開いたとき、Remote が書いた位置とトラックで始まる。
- 利用者への見え方: Remote 接続を受け付けると PC の閲覧ウィンドウが閉じ、「リモート接続中」ダイアログだけになる。
  切断後は一覧に戻る。マニュアルと Remote の正本 (web-remote-plan §2.2 の「PC の player と窓を保持し、返却時に
  復元」) を更新する。
- detached 経路への影響: 所有権の境界での terminal close と、共通 open 境界の所有者検査。既存の terminal close を
  再利用する構造的変更で、症状パッチではない。独立レビューでその合意を取り、detached-rework-plan §11 に記録する。

### 9B.3 端末からの位置の報告

- 新しい書き込み `RemoteWriteRequest::RecordVideoProgress { address, position_secs, duration_secs, ended }` を
  足す (既存の `RecordReadingProgress` と同じ書き込みレーン: HTTP `/api/write` → remote service → IPC → core の
  UI thread の FIFO。所有権の検査・応答あり。`DrainingRemote` 中は既存どおり拒否)。
- client の writer: 画面遷移から独立した、module 単位の直列 writer (既存の `enqueueReadingProgress` と同じ promise の
  列) を置く。viewer はこの writer に位置の snapshot を渡すだけで、viewer の破棄 (`destroy()` の controller abort)
  に巻き込まれない。
- 送る契機:
  - 再生中は 5 秒ごと (PC の周期保存と同じ間隔)。
  - 一時停止したとき、シークが確定したとき、画質・トラックを切り替えたとき。
  - viewer を離れるとき: `cleanupScreen` → `destroy()` の**前**に位置を snapshot して writer に渡す (戻る、前後の動画
    へ移動、一覧へ)。
  - ページが隠れたとき (`visibilitychange` → hidden)。ページの破棄に備えて keepalive で送る。
  - 再生終了 (`ended`) で `ended: true`。
- 位置は `currentPosition()` (端末で実際に再生している位置)。transcode は再生より先行しているので、headless
  player や transcode の位置は使わない。
- 失われ得る範囲: 端末側に所有を手放す操作は無く (HTTP に session release は無い)、所有は PC 側の切断・別端末への
  交代・時間切れで終わる。その場合、最後の報告以降 (再生中なら最大 5 秒) の進みは残らない。既知の制限として
  マニュアルと本書に書く。

### 9B.4 core での適用

- 分類: 新 variant は `Settings.video_resume_positions` を変えるので、settings family の lease 対象
  (`remote_write_uses_settings_family`、`ui.rs:3742`) に加える。
- 検証 (適用前、拒否は応答でエラー): `address` の既存検証 (`validate_write_request`)、動画・音声ファイルであること、
  `position_secs` が有限かつ 0 以上、`duration_secs` が有限かつ正、`position_secs <= duration_secs + 1.0`
  (時計の丸めの許容)。範囲外はクランプせず拒否する。
- 適用: `apply_media_resume_updates` を `MediaResumeUpdate { path, key, position, duration, at_eof: ended }` で呼ぶ。
  PC と同じ規則 (`save_video_resume_position`: 先頭 3 秒未満・末尾 5 秒以内・EOF で削除) と読書履歴の進捗更新が
  1 か所で適用される。
- 再生位置のサムネイル: 位置が残った場合は `maybe_schedule_video_resume_thumbnail(path, key, position)` を明示的に
  呼ぶ (これは path と位置から作るので player は要らない。`apply_media_resume_updates` 自体は予約しない)。
- 読書履歴の行: 進捗の更新は既存行にしか効かない。`record_reading_history(idx)` はローカルの `self.items[idx]` を
  前提にするので、検証済みの path と種類から履歴の行を作る共通入口を抽出し (`record_reading_history` もそれを
  使う)、Remote で動画の配信を始めたときにその入口で行を作る。
- ディスクへの保存: PC の再生位置と同じく、次の `Settings::save()` / 終了時に書く (PC で再生した位置と同じ耐久性)。
  所有終了時の同期保存は足さない (現行の `save()` は UI thread で DB 全体を同期保存するため)。

### 9B.5 開始位置 (既存どおり)

- Remote の開始位置は既存どおり PC 側の再生位置の表から取る (headless player を `from_grid=true` で開く)。
  「一覧から開いたときは先頭から」の設定が ON なら先頭から始まる (PC で一覧から開いたときと同じ意味)。

### 9B.6 テスト

- 取得 barrier: 全 context (main fullscreen / active detached / ParkedLive / メディア窓 / 音楽ビュー / 画像だけの
  context) が閉じてから `finish_acquire` される。`InProgress` の間は取得が完了しない (transition 中・未実行の effect・
  fullscreen viewport の非表示待ち)。helper の型付き失敗で取得が中止され phase が `Local` に戻る。閉鎖が進まない
  場合は watchdog で取得が失敗として終わる。
  保存が 1 回走る。
- 再オープンの防止: Local 復帰 → fullscreen 復元待ち → 再取得で、旧 restore が失効して開かない。PDF / ZIP の
  列挙完了が取得後に届いても閲覧が開かない。所有中に共通 open 境界が拒否する。
- 書き戻し: `RecordVideoProgress` が map と読書履歴を更新する。検証 (非有限・負・duration 0・範囲超過) で拒否。
  末尾・先頭・EOF の削除規則。所有中でない (Draining・Local) 書き込みは拒否。settings family の lease を通る。
  Remote が書いた位置が、所有終了後に PC で開いたときの開始位置になる。一覧に無い path でも履歴の行ができる。
  位置が残ったときサムネイルが予約される。
- client: 報告の契機 (周期・一時停止・seek 確定・離脱・hidden・ended)、`destroy()` の前の snapshot、直列化。既存の
  JS テストの仕組みで。
- 静止画: Remote で本のページ位置を書いた後、所有終了後に PC で開くとその位置になる (PC 側の古い閲覧状態が無い)。

## 10. 段階と受け入れ条件

各段は、実装 → ライブラリのテスト全体 (`cargo test -p mimageviewer --lib`) と関係する統合テスト → 独立 Sol
レビュー (ACCEPT) → コミット、の順で進める。

### S1: テスト素材とトラック列挙

- `scripts/ui-smoke/generate_audio_tracks_fixture.py` (または `.ps1`) と `testdata/audio-tracks/README.md`。
  ffmpeg の lavfi だけで作る (私有素材を使わない):
  - `multi.mkv`: testsrc2 映像 6 秒 + 音声 3 本。周波数で識別できる sine
    (440 Hz / 880 Hz / 1320 Hz)、channels (2 / 6 / 1)、sample rate (48000 / 44100 / 32000)、codec
    (aac / ac3 / flac。Opus は仕様上 48 kHz 固定で 32 kHz を作れないため flac)、language (jpn / eng / 無し)、title (有 / 有 / 無し)、disposition default は 2 本目。
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
  - 失敗した選択は、その後の通常 seek (別位置) で自動再試行されない。選択し直すと再試行される。
  - `demux_serial`: audio-only の素材 (動画なし) の seek、seek 失敗後の再開、`SeekCompleted` の再送が重なる場合で、
    新世代の packet が `Flush` より前に decode されず、旧世代の packet が新世代として decode されない。
  - audio decode thread が終了した状態での切り替えは、既存の disconnect 終端と同じ結果になる (失敗表示で
    再生を続けない)。
  - 速度・音量・mute が切り替えで変わらない。
  - 異なる sample rate / channel 数 / time base のトラック間 (素材の 3 本) で、切り替え後の audio PTS と
    A/V clock が連続している (切り替え前後の位置差が seek 誤差の範囲)。
  - 導出状態 (§4.2) の純粋関数テスト。
- 作ったテストのうち周波数判定・serial 競合・most-recent-wins・失敗 generation は、対象処理を一時的に外すと
  落ちることを実装者が確かめ、報告に書く。

### S3: 解析系の追従

UI より先に入れる (UI から切り替えられるようになった時点で、Norm と波形が正しいトラックを見ているようにする)。

- Remote の配信の Norm gain もこの段で移す。現在は headless player の単一 gain (`VideoPlayer::normalize_gain()`) を
  `RemoteStreamStartInputs` に写して transcode に渡している (`mod.rs:9457`、`remote_ipc/ui.rs:1062`) が、S3 で単一
  gain を表に置き換えるので、§9A.3 の「generation の worker 内で、開いているトラックについて lookup する」方式を
  この段で入れる。
- 同じ段で、transcode が配るトラックも明示する: `ClocklessTranscodeOptions.audio_stream_index` (§9A.2) をこの段で
  足し、transcode の独自の `best(Audio)` をやめる。session は開始時にこの値を 1 つ決め (headless player の
  `opened_audio_stream_index`)、**配る stream と Norm の lookup の対象を同じ値から取る**。S3 の時点では
  `opened_audio_stream_index` は既定トラックなので挙動は現状と同じ。S5 で保存済みの選択により headless player が
  非既定トラックを開くようになっても、配信音声と gain は同じトラックのまま一致する。
- S3 の受け入れ時点で既存の Remote の Norm が維持されていることをテストで示す。S6 はトラックの切り替えだけを足す。

- 6.1 (Norm: 追加テーブル、scanner の stream 指定、App の Norm 状態の key 拡張、トラック確定待ち、snap、
  worker での lookup) と 6.2 (波形・音楽解析の全 key への stream index)。
- テスト: DB の新旧テーブルの読み分け (新規保存は追加テーブル、既定トラックだけ旧行を読む、非既定では旧行を
  読まない)・`clear_all`/`count`、scanner が指定 stream を測る (sine の振幅をトラックごとに変えて LUFS 差で判定)、
  Norm gain 表 (`Pending` のトラックの frame だけ止まり他のトラックは流れる、トラック境界で snap、同じトラックは
  ramp、lookup の測定済み / 未測定で scan 開始 / 未測定で scan 不可 / lookup 失敗の各分岐で `Pending` が残らない、
  再選択・`Unchanged` で何も解除されない、Norm OFF で表が空になり遅れた lookup 結果を捨てる、末尾 drain 中の
  選択で drain が止まらない、ブロッキング中の別トラック scan が選択で cancel される)、
  scan 中のトラック変更で旧 scan が cancel される、抑止が stream 単位、波形・音楽解析の key に stream index が
  入り別トラックの結果が再利用されない (永続キャッシュ・LRU とも)。
  - `Pending` が seek の公開より前に立つ (選択 → seek 公開 → pump 処理の順を seam で止め、新トラックの frame が
    unity で処理されないこと)。
  - lookup 結果の照合 (Norm OFF → ON 後に OFF 前の結果を捨てる、目標 LUFS 違いの結果を捨てる)。
  - pump: `Pending` が raw の先頭を塞いでいる間は EOF でも `BufferReady` を出さない (短い素材の末尾で)、
    `Pending` → `Gain` と scan 待ち解除の最初の gain は snap、同じトラックの変更は ramp。
  - lookup の所有: 同じトラックの選び直しで lookup が重複しない、逆順に完了した古い lookup が新しい値を上書きしない。
  - open 時の非同期解決: 既存の Norm テスト (測定済みの即時適用・未測定の deferred scan・キャッシュ hit の grid
    再開) の期待を維持、保存した非既定トラックで開いたときにそのトラックの測定値で最初の音が出る、保存した
    トラックを開けず既定で開き直したときは既定トラックの測定値。音声ファイル / 音楽ビューの open も同じ。
  - Norm を再生の途中で ON にしたときの既存の挙動 (測定済み → ramp、未測定 → 同じ操作で scan) が維持される。
    OFF で全トラック unity。

### S4: UI と操作

- 8.1〜8.4。`NativeVideoOutputEvent` の追加と ParkedLive 分類 (§7.6、detached-rework-plan §11 に記録)。
- テスト: App handler-level (event → Norm owner → `select_audio_track`、fs_idx 不一致で無視、source epoch 不一致で
  無視、`stream_index` が現在の player に無ければ無視、ParkedLive で活性化扱い)、失敗トーストが generation ごとに
  1 回、行ラベル生成、keymap の表と ini の整合 (既存テスト)、UI スナップショット (音声モード HUD の選択 UI)。

### S5: 選択の記憶

- §9 (表・`initial_audio_track`・一致規則・保存の契機・環境設定のクリア)。
- テスト: settings.db の往復 (表の読み書き、クリアで消える、旧 DB に表が無くても起動する)、一致規則 (index 一致 +
  codec 不一致で既定へ、language / channels / title の不一致で既定へ、保存時に無かった項目は比較しない)、保存の
  契機 (`Requested` が確定したとき・`Unchanged` のとき。失敗・保留・古い generation は保存しない)、保存トラックを
  開けなかったときの既定での再 open と `open_notice` の通知 1 回 (切り替え失敗の表示と重ならない)、ファイル削除・
  リネームで map が再生位置と同じく破棄・移行される。保存済みの非既定トラックで Remote を始めると、配信音声と
  Norm gain が同じ (保存した) トラックになる。title の往復と、title が変わったファイルで既定へ戻ること。
  選択の確定直後、次の tick の前に close・evict・source swap・teardown・Remote 取得の全閉じが起きても選択が保存される。

### S6: mIV Remote

- §9A (transcode の stream 指定、session の `change_audio_track`、payload と control action、protocol 62、
  worker 内の Norm gain 解決、client の並びと command、選択の記憶)。
- テスト: transcode が指定 stream を encode する (素材の sine 周波数を出力 AAC の decode で判定)、
  `change_audio_track` が新 generation を作り開始できなければ元へ戻す、`expected_generation` 不一致の control が
  409 で拒否され新しい選択を上書きしない、control の `stream_index` 検証、選択の記憶は generation の Ready 後だけ
  (spawn 直後・失敗・置換された generation では記憶しない)、開始時に保存された選択で始まる、start の要求の
  `audio_track` が優先される、PC に音声出力 device が無い (headless player の selection が無い) 状態でも選択どおり
  に始まる、worker 内の Norm gain 解決 (別接続・設定 snapshot・追加テーブル優先)、protocol の往復 (serde)、
  client 側は既存の JS テストの仕組みがあればそれで command と state 反映を試験する (無ければ理由を報告)。

### S7: Remote 所有時の全閉じと再生位置の書き戻し

- §9B (受け付け時の全閉じ、所有中の不変条件、`RecordVideoProgress`、client の報告契機と直列化、読書履歴の行、
  所有終了時の保存)。detached 経路に触れるので、独立レビューで構造的変更であることの合意を取り、
  detached-rework-plan §11 に記録する。
- テスト: §9B.6。

### S8: 実アプリのシナリオと文書

- ui-smoke: `AudioTracks` シナリオ (`scripts/ui-smoke/audio-tracks.rhai`)。`multi.mkv` を開き、HUD の音声ボタン →
  2 行目を選択 (native HUD の名前付き control を `native_ui_smoke.rs` の既存方式で追加)、snapshot の
  `audio_track` (desired / applied / 導出状態) と、pump が出力した直近 chunk の周波数推定 (test-script feature
  限定の診断値) が新トラックの値になることを確認。一時停止中の切り替え、連続切り替え、F12 別ウィンドウでの
  切り替え、音声モードでの切り替え、開き直しで保存したトラックから始まることを含める。`capture(label)` で
  egui 側 (音声モード HUD) を保存する。
- Remote の実機確認は、Remote の PIN 入力を利用者が行う必要があるため利用者に依頼する (PC で選んだトラックで
  Remote が始まる、Remote で切り替えると PC に戻ったときもそのトラック、Remote で見進めた位置から PC で再開する、Remote 受け付け時に PC の閲覧ウィンドウが閉じる)。
  - 実行は使い捨てコピー (`target\portable-smoke`) で、毎回利用者の了承と時間帯を確認してから。
- 文書: マニュアルの動画ページと Remote のページ、`docs/spec.md`、`docs/web-remote-video-streaming-plan.md`、
  `docs/web-remote-plan.md` (§2.2 の所有遷移: 取得時の全閉じ、返却時に復元しないこと、位置の書き戻し)、
  `htdocs/mimageviewer/privacy.html` の「端末内に保存されるデータ」(選択の保存を追記)、`docs/video-architecture.md` (seek 調停・Flush・Norm の節)、
  `docs/keymap-spec.md`、backlog §1.251 の状態更新。

## 11. 対象外とした事項の理由

- 詳細表示 (一覧): ファイル単位の情報で、再生中の選択とは無関係。
- 開いた時点で音声が無効な player での切り替え: §7.4。

## 12. 判断済みの事項

1. 同じトラックのままの seek では resampler を reset しない (既存 seek と同じ)。トラックを替える seek では
   `AudioSetup` ごと差し替えるので、旧トラックのサンプルは swr の delay にも残らない。
2. 切り替え時と open 時の Norm の測定値 lookup は UI thread で行わない (§6.1 の worker)。open 時の既存の同期
   lookup はこの worker に置き換わる。再生途中に Norm を ON にする操作の既存の同期 lookup (UI thread で
   `std::fs::metadata` と SQLite) は挙動を保つために残し、既存事項として backlog に記録する。
3. `AudioSetup` の構築は demux thread で行い、所要時間を perf event で計測する (S2 の受け入れで素材ごとに記録)。
4. 切り替えの seek で HUD の「シーク中...」が出る場合があるが、通常の seek と同じ表示のままにする。
