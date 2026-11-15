# MPEG-TS / PS の keyframe 確認付き seek (§1.785)

状態: 設計 第 4 版 (2026-11-15、設計担当 ClaudeCode Opus)。第 1〜3 版は実装前の独立レビュー (gpt-6.1-sol / xhigh)
で REJECT になり、その指摘 (§8) を反映した。対象は v5.3.0。経緯は [バックログ §1.785](next-release-backlog.md)、
時刻の契約は [動画の時刻原点](video-timeline-origin-plan.md)。

## 1. 観測と原因

観測はすべて同梱 FFmpeg DLL (`vendor/ffmpeg/bin`、avformat-61) を使った headless probe で、製品アプリは起動していない。
素材は lavfi の合成映像 12 秒、30 fps、GOP 15 (0.5 秒)、B frame 2 枚。TS は H.264 + AAC / MPEG-2 + MP2、PS は MPEG-2 + MP2。

- `av_seek_frame(video_stream, t, AVSEEK_FLAG_BACKWARD)` は、TS・PS とも **dts が t 以下の映像 packet** に着地する。
  その packet は key とは限らない (ほとんどの場合 key ではない)。例: TS (H.264) で原点 + 0.5 秒を求めると
  dts=1.967 / pts=2.000 の非 key packet に着地し、最初の key は pts=2.467 (次の GOP)。
- 目標が最後の keyframe より後ろなら、着地点から EOF まで key packet が無く、frame が 1 枚も復号されない。
- 原因は FFmpeg の timestamp seek の仕組み: `mpegts` / `mpeg` は `read_timestamp` だけを持ち、`seek_frame_internal` は
  binary search (`ff_seek_frame_binary` / `ff_gen_search`) で「時刻が目標以下の位置」を探すが、その位置が key かは確かめない。
  `mpegts_get_dts` は探索中に読んだ任意の packet を `AVINDEX_KEYFRAME` として索引へ登録し、PS も通常の読み取りで
  索引を足す (`mpegps_read_pes_header`)。`AVSEEK_FLAG_ANY` を付けないこと・`avformat_seek_file` の範囲指定・
  `seek2any`・`GENPTS` / `IGNIDX` などの flag では、この経路に key の確認は加わらない (レビュー担当が FFmpeg n7.1 の
  `seek.c` で確認)。MP4・MKV のような本物の索引を持つ形式には起きない。
- 目標の時刻が key packet の pts と一致しても外れる: 探索は dts で比べるので、key の手前 (dts が近い非 key) に着地する。
- `seek(stream.start_time)` も先頭の key を飛ばす: start_time は先頭 key の **pts** で、その dts は pts より前にある
  (H.264 TS で先頭 key は pts 1.467 / dts 1.400、seek 後の最初の key は 1.967)。
- 開始時刻 0 の TS でも起き、時刻原点 (`MediaTimeline`) の補正とは独立している。
- PS では key packet の `pos` が -1 のことが多く、pts も欠けることがある (dts だけ)。さらに PS の parser は、どこから
  読み始めたかで packet の時刻の割り当てがずれることがある。**byte 位置への seek や、見つけた key の時刻へ seek し直す
  方式は PS で成り立たない**。同じ引数で seek し直す方式も、確認の走査で索引が増え、次の binary search の境界が変わり
  得るので、同じ packet 列に戻る保証がない (probe の 525 点では違いは出なかったが、保証はない)。

## 2. 方針: 手前で確かめ、確かめた packet 列をそのまま渡す

目標より手前で seek し、そこから読んだ packet だけで「最初に読める映像 key から目標までを復号できるか」を確かめる。
確かめられたら **seek し直さず**、確認の走査で読んだ packet (全 stream、映像の非 key packet も含む) を後段へそのまま渡し、
その後は同じ Input の続きを読ませる。復号は行わず、FFmpeg の索引も書き換えない。

```
target:  求める時刻 (呼び出し側の locator を映像 stream の time base の raw tick へ整数で換算したもの)
head:    先頭へ戻すときの seek = av_seek_frame(input, -1, 0, AVSEEK_FLAG_BYTE)
         (TS / PS とも demuxer が file 先頭から再同期する。録画の途中から始まる TS / PS でも最初に読める key へ戻ることを
         レビュー担当が probe で確認)
stream_start: 映像 stream の start_time (無ければ format の start_time を換算。どちらも無ければ確認なし = Direct と同じ)
target <= stream_start → head へ seek して返す (StartOfStream)
for back in [1, 2, 4, 8, 16, 32] 秒:
    s = target - back
    s <= stream_start → head へ seek して返す (StartOfStream)
    av_seek_frame(input, video_stream, s, BACKWARD)   (失敗したら次の back へ)
    窓 = 着地点から「映像 packet の dts > target」まで。packet を 1 つずつ読み (読むたびに取消と予算を確かめる)、
        全 stream の packet を順に保持し、窓の中の映像 key packet を記録する
        EOF / 読み取りエラー → 窓はそこまで
    判定 (下) が成り立つ → 保持した列 (窓の終わりで読んだ dts > target の映像 packet も含む) を返す (Verified)
    成り立たない → 保持した列を捨てて次の back へ (失敗した back の packet は後段へ混ぜない)
32 秒・予算で確かめられない → 呼び出し側の元の seek (同じ stream index・同じ単位の locator) を 1 回呼び、ログに出して返す (Unverified)
取消 → 何も返さずに止める (Aborted)。Input の位置は不定で、呼び出し側は次の要求で seek し直す
```

- 後段は decoder を flush した後、保持した列を先頭から読み、尽きたら Input を読む。復号は着地点の後の最初の key から始まる。
  最初の key より前の映像 packet も渡す (SPS / PPS・SEI・extradata の更新を含み得るので、codec ごとに捨てる判定をしない)。
  表示の抑制は既存の「目標より前の frame を出さない」処理が受け持つ。
- 保持した列は Input から読んだ packet と同じ順・同じ中身 (side data を含む)。各 stream の中の順序は必ず保つ。
- **再生 decoder の demux thread での接続**: 音声の送信が詰まったとき (`send_audio_packet_with_video_drain`) に救出できる
  映像は `pending_video_packets` にあるものだけなので、保持列の映像が音声の後ろに残ると届かない。そこで Verified のときは、
  保持列の**映像 packet を全部、順に `pending_video_packets` へ入れて** (バイト数の計上も既存のとおり) から、保持列の
  **映像以外の packet を順に既存の配送へ流し**、その後に Input の続きを読む。stream をまたぐ順序は、既存の
  `pending_video_packets` と同じく緩めてよい。こうすると音声待ちの間も既存の drain が保持列の映像を届ける。
  他の worker (サムネイルなど) は映像だけを復号するので、読み口から順に読めばよい。

### key の判定

FFmpeg の key の印は「復号を始められる位置の候補」で、表示時刻の保証ではない。

判定に使う集合 J = 窓の中の映像 key packet のうち、**dts が分かり dts <= target のもの** (窓の終わりの dts > target の
packet は、保持して後段へ渡すが J には入れない)。成功の条件は次の真偽式で、それ以外はすべてこの back では失敗:

```
success = A or B
A = J の中に、pts が分かり pts <= target の key がある
B = 映像の codec が MPEG-1 / MPEG-2 video であり、かつ J の key がすべて pts 不明であり、かつ |J| >= 2
```

- **A の根拠**: key (random access point) どうしは表示の順が復号の順と同じなので、復号を始める最初の key K1 の表示時刻は、
  pts <= target の key の表示時刻以下。K1 が pts 不明でもよい。
- **B の根拠 (MPEG-1 / MPEG-2 に限る)**: MPEG-1 / MPEG-2 の並べ替えは「I / P (anchor) の表示は次の anchor の復号の時」で、
  K1 の表示時刻 = K1 の次の anchor の dts <= K2 の dts <= target (K2 も anchor)。これは codec の規格の並べ替えの規則で、
  GOP の長さや B frame の数によらない。H.264 / HEVC は B frame の階層などでこの規則が成り立たないので、pts 不明の key だけ
  では成功にしない (次の back へ進み、最後は Unverified = 今の seek)。
- J に pts の分かる key が 1 つでもあれば B は使わない。pts の分かる key がすべて target より後なら、key の数によらず失敗
  (pts と dts の差が大きい stream で、key の数から成功にしない)。
- pts の無い key の表示時刻を dts や `video_delay` から推定しない (MPEG-2 の `video_delay` は B frame の数ではなく、推定は
  表示時刻の上限にならない。レビュー担当が PS 素材で確認)。
- MPEG-2 の open GOP の B frame と HEVC の CRA の後の RASL は、key より pts が小さく、復号されないか捨てられる。
  pts >= key の pts の frame は復号できる。
- **対象外 (この修正では保証しない)**: H.264 の recovery point で `recovery_frame_cnt > 0` のもの、intra refresh の stream。
  FFmpeg はこれらにも key の印を付けるが、印の後の数 frame は出力されない。packet だけでは見分けられず、mIV が扱う
  `.mpg` (中身は主に MPEG-2、または IDR を持つ H.264) では稀なので、設計担当の判断で範囲外にする。症状は「最初の frame が
  目標より数 frame 後ろ」で、今の動き (次の GOP、または frame が出ない) より悪くはならない。

### 予算と取消

- **保持の予算**: 保持する packet の大きさの合計 64 MiB。**読み取りの予算**: `AVIOContext.bytes_read` の増分の合計
  (全 back) 128 MiB。どちらかを超えたら Unverified に戻す。時間ではなく量で決める (同じファイル・同じ目標なら結果が
  同じになるように。ディスクの速さで動きを変えない)。
- 予算は「mIV が呼ぶ 1 回の `av_seek_frame` / `av_read_frame` の後」に確かめる。FFmpeg の 1 回の呼び出しの中の読み取り
  (binary search の探索、TS で packet が完成するまでの読み取り、null packet だけの区間) は mIV からは止められず、今の
  seek と同じ。映像の無い長い区間などの異常な stream ではそこが長くなり得るが、まれな stream の異常として割り切る。
- 通常の GOP (0.5〜2 秒) では 1〜2 回目で確かめられ、読む量は数 MB。back は確認に失敗したときだけ倍にする
  (固定秒数の preroll ではない)。窓は back 秒分で、ファイル全体を走査することはない。
- **取消**: 呼び出し側から「この要求はもう要らないか」を問う関数を受け取り、各 seek の前と各 packet の読み取りの後に
  確かめる。取消の遅れは、最大で FFmpeg の 1 回の呼び出しの長さ。
- 復号の量: 復号は保持した列の最初の key から目標まで、最大で `back` 秒 + 1 GOP。

### 対象の demuxer

`mpegts` / `mpegtsraw` / `mpeg` (MPEG-PS) だけ。名前は `MediaTimeline::rebases_demuxer` と同じ 3 つだが、問いが違う
(「duration が end − start か」と「timestamp seek が key に着かないか」) ので、述語は seek の所有者の中に別に置き、
理由をコメントに書く。他の `read_timestamp` を持つ形式 (ogg・nut など) は本物の seek を別に持つか mIV の対象外なので
入れない。拡張子ではなく、開いた Input の demuxer 名で決める (`.mpg` の中身が TS / PS のもの)。映像 stream の無い Input
(音声だけ) は対象外。

## 3. 所有: 共通の seek owner と packet の読み口

新しい `src/video/keyframe_seek.rs` が、Input ごとの「目標以前の keyframe へ seek する」と「その後の packet の読み口」を
一手に持つ。状態は seek 1 回分の保持 packet 列だけで、Input をまたいだキャッシュや共有の状態は持たない
(worker ごとに自分の Input と自分の読み口を持ち、互いに影響しない)。

- 入口: 「呼び出し側の元の seek の引数 (stream index と locator)」「映像 stream index」「取消の問い合わせ」を受け取り、結果
  (`Direct` / `Verified { back, held_packets }` / `StartOfStream` / `Unverified` / `Aborted` / seek 失敗) を返す。
  時刻の変換は `MediaTimeline` に任せ、この module では原点を扱わない (元の locator は呼び出し側が今と同じ方法で作る)。
  対象外の demuxer の `Direct` と `Unverified` の fallback は、元の引数のまま seek する。
- 読み口: 各 worker の packet の読み取り (`input.packets()`) を、「保持した列を先に返し、尽きたら Input を読む」読み口へ
  置き換える。保持した列は次の seek・flush・取消で捨てる。読み口の型を 1 つにし、保持した列を読み飛ばす経路を作らない。
- 呼び出し側の後処理の契約: seek の後の decoder flush、目標より前を捨てる処理、frame step の `seek_start` / `base`、
  `audio_adoption_start` は、**要求したメディア時刻** のまま (helper の back の位置や保持列の先頭の時刻を渡さない)。
- `Aborted` は seek 失敗とは別に扱い、forward での再試行をしない (§3.1)。
- 結果・試行回数・読んだ packet 数と byte 数・時間を `crate::logger` と `perf::event` (cat=`video`) に出す
  ([UI 応答性](ui-responsiveness.md) の計装)。すべて worker / demux thread の中で起き、UI スレッドは通らない。

### 3.1 再生 decoder の demux thread

- seek 要求の処理 (`src/video/decoder.rs` の demux thread、シークバーのクリック・キー・ホイール・frame step・
  続きから再生・チャプター・ブックマーク・ループ・A-B ループ・音声トラック切り替えがここへ集まる) の `backward` を
  helper に置き換える。取消の問い合わせは「より新しい seek 要求が来ている、または処理中の serial と現在の serial が
  違う」か「停止」。
- `Aborted` (より新しい seek 要求): 古い要求のために準備した音声トラックの setup と保持 packet 列を捨て、古い要求を
  失敗として通知せず採用もせず (`SeekCompleted` も送らない)、最新の要求の処理へ戻る。forward 再試行はしない。
  停止なら終了へ進む。serial が要求の公開より先に進む区間、音声トラック切り替え中の supersede も含めて、既存の
  「新しい要求が古い要求を置き換える」経路と同じ結果になることを確かめる。
- seek 失敗 (helper が `av_seek_frame` 自体の失敗を返したとき) の forward 再試行は今のまま。

### 3.2 置き換える呼び出し (全部)

| 経路 | 場所 | 取消 | 後処理で直すこと |
| --- | --- | --- | --- |
| 再生の seek | `src/video/decoder.rs` demux thread (§3.1) | 新しい seek 要求・serial の不一致・停止 | なし |
| hover / サムネイル | `src/video/thumbnail.rs` | `cancel` と request の supersede | EOF で decoder を drain する (下)。既存の許容秒数の規則は今のまま |
| タイル | `src/video/tile_thumbnails.rs` | 既存の取消 | EOF で decoder を drain する (下) |
| シークストリップの枠 | `src/video/seek_strip_thumbs.rs` | 既存の取消 | なし (keyframe-only の先行と許容範囲の規則は今のまま) |
| スクリーンショット | `src/video/screenshot.rs` | なし (1 回きり) | 240 packet / frame の上限 (下) |
| Remote の配信 (時計なし transcode) | `src/video/clockless_transcode.rs` | 既存の取消 | なし |

- **EOF の drain (サムネイル・タイル)**: 今は EOF で decoder を drain せず、それまでの最後の frame を返す。最後の key より
  後ろの目標では、遅延して出る最後の数 frame が要るので、EOF で `send_eof` して残りの frame を受け取ってから採用規則を当てる。
- **スクリーンショット**: 今は映像 packet 240 個で打ち切り、手前の frame を返し得る (60 fps・GOP 6 秒では正常な preroll
  だけで 240 を超える)。packet 数の上限を外し、「目標に届いた」「EOF まで drain した (目標が最後の frame より後ろなら
  最後の frame が正しい結果)」「読み取りエラー」で終える。復号を始める位置は seek owner が目標から有界な手前に決めるので、
  上限を外しても復号の量は有界 (MP4 などの Direct では今と同じく目標の直前の key から)。
- **upscale は今回は対象外** (`src/video/upscale/job.rs` は今の seek のまま)。upscale の分割は、key の packet 順の番号
  (`KeyframePoint.frame`) と復号後の表示順の番号を同じものとして扱っていて、open GOP の stream では境界の frame が
  ずれ得る (レビュー担当の probe で MPEG-2 TS の 2 個目の key が packet 順 13 番・表示順 15 番)。これは seek の着地とは
  別の upscale の番号付けの問題で、seek だけを差し替えても直らないので、バックログ §1.795 に分けて扱う。今回の変更で
  upscale の動きは変わらない。
- `src/video/seek_strip.rs` の warmup (索引を作るための捨てシーク) は置き換えない (§4)。
- `src/audio_decode.rs` (音声の範囲 decode) は音声 stream の seek で、音声 packet はすべて key なので対象外。
- music video editor の seek は PCM 再生、Remote の crate は IPC で、FFmpeg の seek を持たない (レビュー担当の確認)。
- 実装担当は上の表が全部かを grep (`av_seek_frame` / `avformat_seek_file` / `Input::seek` / `.seek(`) で確かめ、漏れがあれば足す。

## 4. シークストリップの軸

TS / PS の `AVIndexEntry` は `mpegts_get_dts` などが探索中に登録した点で keyframe ではないので、`mpegts` / `mpegtsraw` /
`mpeg` の Input では索引を読まずに時刻格子 (`StripAxis::TimeGrid`) を使う (新しい `TimeGridReason` を足す)。warmup の
捨てシークもこれらでは不要になる。各枠の画は §3 の seek で目標以前の key から復号される。今は `KeyframesTooSparse` で
全部の枠が空になる (サブ PC の依頼 51 の観測)。

## 5. 割り切り

- 32 秒を超える GOP・key の印が無い stream・予算切れ・seek の失敗は、今までと同じ seek に戻してログに出す
  (まれな失敗は割り切り)。
- FFmpeg の 1 回の呼び出しの中の長い読み取り (映像の無い長い区間など異常な stream) は止められない (§2)。
- H.264 の recovery point・intra refresh は範囲外 (§2 の key の判定)。
- 確認の走査中の読み取りエラーは「その back では窓の終わり」として扱い、回復処理は足さない。

## 6. テスト

素材は新しい `testdata/mpeg-keyframe-seek/` に、システムの `ffmpeg` と lavfi だけで作る (生成スクリプトは
`scripts/ui-smoke/generate_mpeg_keyframe_seek_fixture.py`、既存の `generate_timeline_origin_fixture.py` と同じ流儀)。

- 映像は **frame ごとに表示順の番号を画素から読み取れる** もの (例: `geq` で番号を数個の単色ブロックへ 2 進で符号化し、
  圧縮後も読み取れる段差にする)。
- **正解表は実装と独立に作る**: 生成スクリプトが、全 frame を先頭から復号した結果 (表示順の番号・`best_effort_timestamp`)
  と全映像 packet (key の印・pts・dts) を JSON にして素材と一緒に置く。テストは seek の結果をこの表と画素の番号で判定し、
  実装と同じ推定を正解に使わない。
- 素材 (通常の GOP、B frame あり):
  - TS: H.264 + AAC、GOP 0.5 秒、小さな原点
  - TS: MPEG-2 + MP2、GOP 0.5 秒、open GOP、`-output_ts_offset 36000`
  - TS: HEVC + AAC、open GOP (CRA と RASL を含む)
  - PS: MPEG-2 + MP2、GOP 0.5 秒 (pts の無い key を含むことを生成時に検査する)
  - 録画の途中から始まる TS と PS (先頭が GOP の途中。既存の素材の先頭を切る)
  - 長い GOP の TS: GOP 6 秒・60 fps (back を 2 回以上倍にする経路、スクリーンショットの 240 を超える preroll)
  - 反例: pts だけを 2 秒遅らせた MPEG-2 TS (`setts`。key が 2 つ以上でも pts > target なら失敗すること)、key の間隔が
    一か所だけ 3 frame の MPEG-2 TS (窓の終わりの key を J に数えないこと)
  - 対照の MP4 (同じ映像) — 動きが変わらないこと

受入条件 (frame は表示順の番号で判定する。1 秒ごとの色では 0.5 秒のずれを見逃すので使わない):

- `keyframe_seek` の単体 (実素材): 目標を 0.1 秒刻み・各 key の pts ちょうど・各 key の dts ちょうど・pts の無い key の直前と
  直後・先頭 GOP の中・先頭より前・最後の key より後ろ・D 以後に置き、読み口から順に復号したとき、正解表で「目標の時刻に
  表示される frame」(表示時刻が目標以下の最後の frame) が出力される。先頭 GOP 以前は StartOfStream で、正解表の最初に
  復号できる frame から出る。長い GOP で back が倍になる。予算を小さくすると Unverified になり、元の seek と同じ着地
  (大きな原点の 90 kHz 素材で、元の要求位置) になる。取消で `Aborted`。MP4 は `Direct` で今と同じ着地。読み口の列が、
  成功した back の着地点からの Input の packet 列と同じ (失敗した back の packet が混ざらない、side data を含む)。
- 復号 (worker 層): サムネイル・スクリーンショット・タイル・ストリップの枠で、時刻 t を求めて返る frame の番号が、正解表と
  各 worker の既存の採用規則 (許容秒数・keyframe-only の先行) から決まる期待値になる。最後の key より後ろ・最後の遅延
  frame (D − 1 frame) でも画が返る。スクリーンショットは 240 を超える preroll でも t の frame を返す。
- 再生 decoder: TS / PS / HEVC で seek(t) の後の最初の表示 frame の番号が正解表の期待値。最初に採用される音声 sample の
  時刻が今の採用規則どおりで、その後の音声が途切れない。frame step の前後、D − 0.3 秒への seek の後の最初の frame と
  EOF の時刻。音声 queue が詰まった状態 (Buffering 中の音声 backpressure) で、seek の直前に `pending_video_packets` が空、目標の
  映像が保持列の音声 packet より後ろにある場合でも、映像が届いて表示が進む (既存の queue の drain だけでは合格しない
  ように状態を作る)。
  連続した seek の supersede (`Aborted`) で古い要求の `SeekCompleted` が出ず、最新の要求が採用される。音声トラック
  切り替え中の supersede。
- シークストリップ: TS / PS の軸が `TimeGrid` になり、枠が空にならない。MP4 の軸の判定は今と同じ。
- Remote: 開始位置 t の transcode で、最初の segment の最初の映像が正解表の期待値。
- 複数の Input を同時に開いても、互いの seek・保持列に影響しない。
- 計装: 各素材で seek 1 回の試行回数・読んだ量・時間を log に出し、取消から `Aborted` までの遅れを記録する (受入値は置かず、
  結果を完了報告に書く)。
- 既存の `testdata/timeline-origin/` の試験 (全 intra) は変えずに通る。upscale の既存の試験は変えずに通る。

## 7. 文書

- `docs/video-architecture.md` に「TS / PS の seek」の節を足す (§2 の契約、§3 の所有と読み口、対象 demuxer、割り切り)。
- `docs/next-release-backlog.md` §1.785 の状態を更新し、upscale の分割の番号付けを §1.795 として起票する。
  公開文書 (マニュアル・製品ページ) は、seek が正しくなるだけなので変えない。更新履歴は公開作業で書く。

## 8. 実装前レビューの記録

第 1 版 (d08acc5d7) への gpt-6.1-sol / xhigh の指摘と、第 2 版での扱い:

- P1 先頭: `stream.start_time` (pts) への seek は先頭の key を飛ばす → 先頭へは byte 0 の seek (§2)。
- P1 upscale の番号: 前の key から読むと番号がずれる → (第 3 版で対象外へ。下)
- P1 同じ引数で seek し直す方式は、走査で増えた索引で着地が変わり得る → seek し直さず、確認で読んだ packet を渡す (§2・§3)。
- P1 key の印は表示時刻の保証ではない → (第 3 版で判定を改めた。下)
- P2 上限: 量の予算と packet ごとの取消 (§2)。時間の予算は、ディスクの速さで結果が変わるので採らない。
- P2 `Aborted` の遷移 (§3.1)。P2 スクリーンショットの 240 上限 (§3.2)。P2 受入条件を既存の採用規則と frame 番号へ (§6)。

第 2 版 (a1946f41c) への指摘と、第 3 版での扱い:

- P1 pts の無い key の reorder 推定は表示時刻の上限にならない → 推定をやめ、「pts の分かる key が target 以下」か
  「窓に key が 2 つ以上」で判定 (§2)。recovery point・intra refresh は設計担当の判断で範囲外と明記。
- P1 upscale は packet 順と表示順の番号の問題が残る → seek の着地とは別の問題として今回の対象から外し、§1.795 へ (§3.2)。
- P1 映像の非 key packet を捨てるのは安全でない → 窓の packet は全部保持して渡す (§2)。
- P2 保持列の音声が映像を塞ぐ → 保持列は Input の packet と同じ配送に乗せる (§2)。backpressure の回帰を §6 へ。
- P2 予算が FFmpeg 内部の読み取りを制限しない → 保持と読み取り (`bytes_read`) の予算に分け、1 回の呼び出しの中は
  止められないことを明記 (§2)。取消の遅れを計装 (§6)。
- P2 fallback の時刻の単位 → 元の seek の引数のまま呼ぶ (§3)。
- P2 サムネイル・タイルの EOF drain とスクリーンショットの予算 → §3.2。
- P2 推定を正解にする検査 → 正解表を生成時の全復号から作る (§6)。§3.1 の取消に serial の不一致を足した。

第 3 版 (665b57dc2) への指摘と、第 4 版での扱い (範囲の判断 2 点はレビュー担当が了承):

- P1 「key が 2 つ以上」は pts と矛盾しても成功になる (pts を 2 秒遅らせた素材) → 判定を真偽式 A or B にし、B は
  MPEG-1 / MPEG-2 で J の key がすべて pts 不明のときだけ。根拠を規格の並べ替えの規則に限定 (§2)。
- P2 窓の終わりの packet と判定の集合の区別 → J を「dts が分かり dts <= target の key」と定義し、終わりの packet は保持だけ (§2)。
- P2 音声 backpressure → Verified では保持列の映像を先に `pending_video_packets` へ入れる (§2)。§6 の回帰の状態を指定。
- §6 に追加: pts を遅らせた反例素材、key の間隔が短い (3 frame) 箇所を持つ MPEG-2 TS。
