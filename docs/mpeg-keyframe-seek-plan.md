# MPEG-TS / PS の keyframe 確認付き seek (§1.785)

状態: 設計 第 2 版 (2026-11-15、設計担当 ClaudeCode Opus)。第 1 版は実装前の独立レビュー (gpt-6.1-sol / xhigh) で
REJECT になり、その指摘 (§8) を反映した。対象は v5.3.0。経緯は [バックログ §1.785](next-release-backlog.md)、
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

目標より手前で seek し、そこから読んだ packet だけで「最初に読める映像 key が目標以前か」を確かめる。確かめられたら
**seek し直さず**、確認の走査で読んだ packet を後段へそのまま渡し、その後は同じ Input の続きを読ませる。復号は行わず、
FFmpeg の索引も書き換えない。

```
target: 求める時刻 (映像 stream の time base の raw tick)
head:   先頭へ戻すときの seek = av_seek_frame(input, -1, 0, AVSEEK_FLAG_BYTE)  (TS / PS とも file 先頭は packet の境界)
stream_start: 映像 stream の start_time (無ければ format の start_time を換算。どちらも無ければ確認なし = Direct と同じ)
              録画の途中から始まる TS では先頭の key より前のことがあるが、その場合も下の back で先頭へ戻る
target <= stream_start → head へ seek して返す (StartOfStream)
for back in [1, 2, 4, 8, 16, 32] 秒:
    s = target - back
    s <= stream_start → head へ seek して返す (StartOfStream)
    av_seek_frame(input, video_stream, s, BACKWARD)   (失敗したら次の back へ)
    packet を 1 つずつ読む (読むたびに取消と予算を確かめる):
        映像以外の packet           → 保持する
        映像の非 key packet         → 捨てる (key より前は復号に使えない)。dts > target なら この back では失敗
        映像の最初の key packet     → 判定 (下)。成功なら保持して終わり、失敗ならこの back では失敗
        EOF / 読み取りエラー        → この back では失敗
    成功: 保持した packet 列を返す (Verified)。呼び出し側は decoder を flush した後、この列を先に読み、続けて Input を読む
32 秒・予算で確かめられない → 今までと同じ av_seek_frame(-1, target, BACKWARD) を呼び、ログに出して返す (Unverified)
取消 → 何も返さずに止める (Aborted)。Input の位置は不定で、呼び出し側は次の要求で seek し直す
```

### key の判定

FFmpeg の key の印は「復号を始められる候補」で、表示時刻の保証ではない。判定は次の 2 段にする。

- pts がある key: `pts <= target` で成功。
- pts が無い key (PS で多い): dts は表示時刻ではない (B frame があれば pts > dts)。`dts + reorder <= target` で成功とする。
  `reorder` = `codecpar.video_delay` (無ければ 2) × 1 frame の時間 (`avg_frame_rate`、無ければ `r_frame_rate`、
  どちらも無ければ 1/25 秒)。満たさなければこの back では失敗 (次の back で前の GOP から読む)。
- MPEG-2 の open GOP の B frame と HEVC の CRA の後の RASL は、key より pts が小さく、復号されないか捨てられる。
  pts >= key の pts の frame は復号できるので、key の pts <= target ならば目標の frame までは復号できる。
- H.264 の recovery point (`recovery_frame_cnt > 0`) や intra refresh の stream は、key の印の後の数 frame が出ない。
  packet だけでは確かめられないので割り切る (§5)。その場合の症状は「最初の frame が目標より少し後ろ」で、今より悪くはならない。

### 予算と取消

- **予算** (全 back の合計): 読んだ packet の byte 数の合計 64 MiB、または packet 数 20,000 を超えたら Unverified に戻す。
  時間ではなく量で決める (同じファイル・同じ目標なら結果が同じになるように。ディスクの速さで動きを変えない)。
  映像 packet が長く途切れる・時刻が欠ける・音声だけの長い区間でも、この予算で必ず止まる。保持する packet 列も
  この予算の中に収まる (映像の非 key packet は保持しないので、実際は音声などの数百 KB)。
- 通常の GOP (0.5〜2 秒) では 1〜2 回目で確かめられ、読む量は数 MB。back は確認に失敗したときだけ倍にする
  (固定秒数の preroll ではない)。全ファイル走査はしない。
- **取消**: 呼び出し側から「この要求はもう要らないか」を問う関数を受け取り、packet を読むたびと seek の前に確かめる。
  FFmpeg の 1 回の `av_seek_frame` (binary search) や 1 回の read の途中では止められない (今の seek と同じ)。
- 復号の量: 復号は保持した key から目標まで、最大で `back` 秒 + 1 GOP。目標より前の frame を表示しない既存の処理
  (`drop_before_secs` など) が受け持つ。

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

- 入口: 「目標 (raw locator)」「映像 stream index」「取消の問い合わせ」を受け取り、結果
  (`Direct` / `Verified { back, held_packets }` / `StartOfStream` / `Unverified` / `Aborted` / seek 失敗) を返す。
  時刻の変換は `MediaTimeline` に任せ、この module では原点を扱わない。対象外の demuxer の `Direct` は今の呼び出しと
  まったく同じ引数で seek する。
- 読み口: 各 worker の packet の読み取り (`input.packets()`) を、「保持した列を先に返し、尽きたら Input を読む」読み口へ
  置き換える。保持した列は次の seek・flush・取消で捨てる。読み口の型を 1 つにし、保持した列を読み飛ばす経路を作らない。
- 呼び出し側の後処理の契約: seek の後の decoder flush、目標より前を捨てる処理、frame step の `seek_start` / `base` は、
  **要求したメディア時刻** のまま (helper の back の位置を渡さない)。
- `Aborted` は seek 失敗とは別に扱い、forward での再試行をしない (§3.1)。
- 結果・試行回数・読んだ packet 数と byte 数・時間を `crate::logger` と `perf::event` (cat=`video`) に出す
  ([UI 応答性](ui-responsiveness.md) の計装)。すべて worker / demux thread の中で起き、UI スレッドは通らない。

### 3.1 再生 decoder の demux thread

- seek 要求の処理 (`src/video/decoder.rs` の demux thread、シークバーのクリック・キー・ホイール・frame step・
  続きから再生・チャプター・ブックマーク・ループ・A-B ループ・音声トラック切り替えがここへ集まる) の `backward` を
  helper に置き換える。取消の問い合わせは「より新しい seek 要求が来ている」か「停止」。
- `Aborted` (より新しい seek 要求): 古い要求のために準備した音声トラックの setup と保持 packet 列を捨て、古い要求を
  失敗として通知せず採用もせず (`SeekCompleted` も送らない)、最新の要求の処理へ戻る。forward 再試行はしない。
  停止なら終了へ進む。serial が要求の公開より先に進む区間、音声トラック切り替え中の supersede も含めて、既存の
  「新しい要求が古い要求を置き換える」経路と同じ結果になることを確かめる。
- seek 失敗 (helper が `av_seek_frame` 自体の失敗を返したとき) の forward 再試行は今のまま。

### 3.2 置き換える呼び出し (全部)

| 経路 | 場所 | 取消 | 後処理で直すこと |
| --- | --- | --- | --- |
| 再生の seek | `src/video/decoder.rs` demux thread (§3.1) | 新しい seek 要求・停止 | なし |
| hover / サムネイル | `src/video/thumbnail.rs` | `cancel` と request の supersede | なし (既存の許容秒数の規則は今のまま) |
| タイル | `src/video/tile_thumbnails.rs` | 既存の取消 | なし |
| シークストリップの枠 | `src/video/seek_strip_thumbs.rs` | 既存の取消 | なし (keyframe-only の先行と許容範囲の規則は今のまま) |
| スクリーンショット | `src/video/screenshot.rs` | なし (1 回きり) | 240 packet / frame の上限 (下) |
| upscale の切り出し | `src/video/upscale/job.rs` `seek_input_to_source_pts` | 既存の取消 | frame 番号の起点 (下) |
| Remote の配信 (時計なし transcode) | `src/video/clockless_transcode.rs` | 既存の取消 | なし |

- **スクリーンショット**: 今は映像 packet 240 個で打ち切り、手前の frame を返し得る。60 fps・GOP 6 秒では正常な
  preroll だけで 240 を超える。打ち切りを「目標に届いた」「EOF まで drain した」「予算切れ」に分け、予算は
  helper が使った back + 1 GOP 分を処理できる量にする。予算切れでは手前の画を成功として返さず、失敗にする。
  EOF (目標が最後の frame より後ろ) では最後の frame が正しい結果。
- **upscale**: 今は seek 後の最初の復号 frame を `planned.seek_start_frame` と見なして番号を数える。helper は計画の
  key より前の key から読ませることがあるので、**pts が `planned.seek_start_pts` より前の復号 frame を番号を振らずに
  捨て**、seek_start_pts の frame から数え始める。manifest の raw locator に `MediaTimeline` の原点を足さない。
  forward fallback (`input.seek`) は別の入口として今のまま残す。
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
- H.264 の recovery point (`recovery_frame_cnt > 0`)・intra refresh の stream は packet だけでは確かめず、最初の frame が
  目標より少し後ろになることがある (今の動きより悪くはならない)。
- 確認の走査中の読み取りエラーは「その back では失敗」として扱い、回復処理は足さない。

## 6. テスト

素材は新しい `testdata/mpeg-keyframe-seek/` に、システムの `ffmpeg` と lavfi だけで作る (生成スクリプトは
`scripts/ui-smoke/generate_mpeg_keyframe_seek_fixture.py`、既存の `generate_timeline_origin_fixture.py` と同じ流儀)。
**frame ごとに番号を読み取れる映像** (例: `geq` で frame 番号を数個の単色ブロックへ符号化し、圧縮後も読み取れる段差に
する) で、通常の GOP を持つもの:

- TS (H.264 + AAC、GOP 0.5 秒、B frame あり、小さな原点)
- TS (MPEG-2 + MP2、GOP 0.5 秒、B frame あり、`-output_ts_offset 36000`)
- PS (MPEG-2 + MP2、GOP 0.5 秒、B frame あり。pts の無い key を含むことを生成時に検査する)
- 長い GOP の TS (GOP 6 秒・60 fps、back を 2 回以上倍にする経路、スクリーンショットの 240 を超える preroll)
- 対照の MP4 (同じ映像) — 動きが変わらないこと

受入条件 (時刻は frame の番号と実 frame の pts で判定する。1 秒ごとの色では 0.5 秒のずれを見逃すので使わない):

- `keyframe_seek` の単体 (実素材): 目標を 0.1 秒刻み・各 key の pts ちょうど・各 key の dts ちょうど・先頭 GOP の中・
  先頭より前・最後の key より後ろ・D 以後に置き、読み口から最初に読める映像 key の pts (無ければ推定) <= 目標
  (先頭 GOP 以前は StartOfStream で先頭の key から読める)。長い GOP で back が倍になる。予算を小さくすると Unverified に
  なり、今の seek と同じ着地になる。取消で `Aborted`。MP4 は `Direct` で今と同じ着地。保持した非映像 packet が読み口から
  順に出る。
- 復号 (worker 層): サムネイル・スクリーンショット・タイル・ストリップの枠で、時刻 t を求めて返る frame の番号が、各 worker の
  既存の採用規則 (許容秒数・keyframe-only の先行) で期待される番号になる。最後の key より後ろ・最後の遅延 frame の drain
  (D − 1 frame) でも画が返る。スクリーンショットは 240 を超える preroll でも t の frame を返し、予算切れでは失敗を返す。
- 再生 decoder: TS / PS で seek(t) の後の最初の表示 frame の番号と pts が t。frame step の前後、D − 0.3 秒への seek の後の
  最初の frame と EOF の時刻。連続した seek の supersede (`Aborted`) で古い要求の `SeekCompleted` が出ず、最新の要求が
  採用される。音声トラック切り替え中の supersede。
- upscale: TS の segment 境界で frame の重複・欠落が無い (番号の連続)、再開・結合の後も同じ。
- シークストリップ: TS / PS の軸が `TimeGrid` になり、枠が空にならない。MP4 の軸の判定は今と同じ。
- Remote: 開始位置 t の transcode で、最初の segment の最初の映像が t の frame。
- 複数の Input を同時に開いても、互いの seek・保持列に影響しない。
- 既存の `testdata/timeline-origin/` の試験 (全 intra) は変えずに通る。

## 7. 文書

- `docs/video-architecture.md` に「TS / PS の seek」の節を足す (§2 の契約、§3 の所有と読み口、対象 demuxer、割り切り)。
- `docs/next-release-backlog.md` §1.785 の状態を更新する。公開文書 (マニュアル・製品ページ) は、seek が正しくなるだけなので
  変えない。更新履歴は公開作業で書く。

## 8. 実装前レビューの記録

第 1 版 (d08acc5d7) への gpt-6.1-sol / xhigh の指摘と、この版での扱い:

- P1 先頭: `stream.start_time` (pts) への seek は先頭の key を飛ばす → 先頭へは byte 0 の seek (§2)。
- P1 upscale の番号: 前の key から読むと番号がずれる → seek_start_pts より前の frame を捨てる (§3.2)。
- P1 同じ引数で seek し直す方式は、走査で増えた索引で着地が変わり得る → seek し直さず、確認で読んだ packet を渡す (§2・§3)。
- P1 key の印は表示時刻の保証ではない → pts の無い key は reorder 分を足して判定、recovery point は割り切り (§2・§5)。
- P2 上限: 量の予算と packet ごとの取消 (§2)。時間の予算は、ディスクの速さで結果が変わるので採らない。
- P2 `Aborted` の遷移 (§3.1)。P2 スクリーンショットの 240 上限 (§3.2)。P2 受入条件を既存の採用規則と frame 番号へ (§6)。
