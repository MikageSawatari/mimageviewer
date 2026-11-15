# MPEG-TS / PS の keyframe 確認付き seek (§1.785)

状態: 設計 (2026-11-15、設計担当 ClaudeCode Opus)。対象は v5.3.0。経緯は
[バックログ §1.785](next-release-backlog.md)、時刻の契約は [動画の時刻原点](video-timeline-origin-plan.md)。

## 1. 観測と原因

観測はすべて同梱 FFmpeg DLL (`vendor/ffmpeg/bin`、avformat-61) を使った headless probe で、製品アプリは起動していない。
素材は lavfi の合成映像 12 秒、30 fps、GOP 15 (0.5 秒)、B frame 2 枚、TS は H.264 + AAC / MPEG-2 + MP2、PS は MPEG-2 + MP2。

- `av_seek_frame(video_stream, t, AVSEEK_FLAG_BACKWARD)` は、TS・PS とも **dts が t 以下の最後の映像 packet** に着地する。
  その packet は key とは限らない (ほとんどの場合 key ではない)。例: TS (H.264) で原点 + 0.5 秒を求めると
  dts=1.967 / pts=2.000 の非 key packet に着地し、最初の key は pts=2.467 (次の GOP)。
- 目標が最後の keyframe より後ろなら、着地点から EOF まで key packet が無く、frame が 1 枚も復号されない。
- 原因は FFmpeg の timestamp seek の仕組み: `mpegts` / `mpeg` は `read_timestamp` だけを持ち、`seek_frame_internal` は
  binary search (`ff_seek_frame_binary`) で「時刻が目標以下の位置」を探すが、その位置が key かは確かめない。
  `mpegts_get_dts` は探索中に読んだ任意の packet を `AVINDEX_KEYFRAME` として索引へ登録する
  (FFmpeg n7.1 の `mpegts.c` / `seek.c`)。MP4・MKV のような本物の索引を持つ形式には起きない。
- 目標の時刻が key packet の pts と一致しても外れる: 探索は dts で比べるので、key の手前 (dts が近い非 key) に着地する。
- 開始時刻 0 の TS でも起き、時刻原点 (`MediaTimeline`) の補正とは独立している。
- PS では key packet の `pos` が -1 のことが多く、pts も欠けることがある (dts だけ)。さらに PS の parser は、どこから
  読み始めたかで非 key packet の時刻の割り当てがずれることがある。**byte 位置への seek や、見つけた key の時刻へ
  seek し直す方式は PS で成り立たない** (probe で key の dts が読み直しでずれ、期待した key に着かない例があった)。
- 同じ Input で **同じ引数の `av_seek_frame` を繰り返すと、着地点とそこから読める key packet は同じ**だった
  (TS 2 種・PS 1 種で目標 175 点ずつ、別 Input を開き直しても同じ。違いが出たのは PS の非 key packet の時刻の
  半 frame の差だけで、key packet は同じ)。

## 2. 方針: 確かめてから手前へ戻す (verify-and-back-off)

「目標より手前で seek し、そこから最初に読める key が目標以前かを packet だけで確かめる。確かめられた seek の
引数で seek し直す」。復号は行わず、FFmpeg の索引も書き換えない。

```
target: 求める時刻 (映像 stream の time base の raw tick)
start:  映像 stream の開始 (stream.start_time、無ければ format の start_time を換算。どちらも無ければ確認なし)
for back in [1, 2, 4, 8, 16, 32] 秒:
    s = max(target - back, start)
    av_seek_frame(input, video_stream, s, BACKWARD)   (失敗したら次の back へ)
    映像 stream の packet を読む:
        最初の key packet:  時刻 (pts、無ければ dts) <= target → 成功
                           時刻 > target → この back では失敗
        key より前に dts > target の packet → 失敗 (窓の中に key が無い)
        EOF / 読み取りエラー → 失敗
    成功: av_seek_frame(input, video_stream, s, BACKWARD) をもう一度呼んで返す (Verified)
    s == start で失敗: start へ seek して返す (先頭から。最初の frame が目標より後ろでも、それが最も近い)
    呼び出し側の取消が立っていたら、そこで止めて返す (Aborted)
32 秒で確かめられない: 今までと同じ av_seek_frame(target) を呼び、ログに出して返す (Unverified)
```

- **最初の key で判定する理由**: 復号は着地点の後の最初の key から始まる。その key が目標以前なら、目標までの frame は
  すべて復号できる (後ろの key も含めて)。最後の key を探す必要はなく、走査は最初の key で止められる。
- **seek し直す位置を `s` にする理由**: 見つけた key の時刻や byte 位置へ seek し直すと、PS で成り立たない (§1)。
  同じ引数の seek は同じ key へ戻ることを probe で確かめた。
- **走査の量の上限**: 1 回の試行の走査は「最初の key」か「dts > target」で止まるので、多くても `back` 秒 + 1 GOP。
  back は確認に失敗したときだけ倍にする (固定秒数の preroll ではない)。合計は最大でも約 63 秒分の packet で、
  32 秒を超える GOP や key の印の無い stream は今までと同じ seek に戻す (全ファイル走査はしない)。
  通常の GOP (0.5〜2 秒) では 1〜2 回目で確かめられる。
- 復号の量: 復号は `s` の後の最初の key から目標まで、最大で `back` 秒 + 1 GOP。今の MP4 で長い GOP を
  seek したときと同じ種類の手間で、既存の「目標より前の frame を表示しない」処理 (`drop_before_secs` など) が受け持つ。
- 音声: seek は映像 stream で行う。音声 packet は `s` から届き、既存の「目標より前の音声を捨てる」処理が受け持つ。
  映像の無い Input (音声だけ) は対象外で、今の seek のまま。

### 対象の demuxer

`mpegts` / `mpegtsraw` / `mpeg` (MPEG-PS) だけ。名前は `MediaTimeline::rebases_demuxer` と同じ 3 つだが、問いが違う
(「duration が end − start か」と「timestamp seek が key に着かないか」) ので、述語は seek の所有者の中に別に置き、
理由をコメントに書く。他の `read_timestamp` を持つ形式 (ogg・nut など) は、本物の seek を別に持つか mIV の対象外なので
入れない。拡張子ではなく、開いた Input の demuxer 名で決める (`.mpg` の中身が TS / PS のもの)。

## 3. 所有: 共通の seek owner

新しい `src/video/keyframe_seek.rs` が、Input ごとの「目標以前の keyframe へ seek する」を一手に持つ。
各 worker は自分の Input を開いた後にこれを作り、今の `av_seek_frame(-1, target, BACKWARD)` を置き換える。
対象外の demuxer では今の呼び出しとまったく同じ動きにする (引数・stream index・時刻の単位も今のまま)。

- 入口は 1 つ: 「メディア時刻 (または raw locator) の目標」と「取消の問い合わせ」を受け取り、結果
  (`Direct` / `Verified { back }` / `StartOfStream` / `Unverified` / `Aborted` / 失敗) を返す。
  時刻の変換は `MediaTimeline` に任せ、この module では原点を扱わない。
- 呼び出し側の後処理 (decoder の flush、目標より前を捨てる処理、forward での再試行) は変えない。
  `Aborted` は呼び出し側の既存の取消 / 置き換えの扱いにつなぐ。
- 結果と試行回数・走査した packet 数・時間を `crate::logger` と `perf::event` (cat=`video`) に出す
  ([UI 応答性](ui-responsiveness.md) の計装)。すべて worker / demux thread の中で起き、UI スレッドは通らない。

### 置き換える呼び出し (全部)

| 経路 | 場所 | 取消 |
| --- | --- | --- |
| 再生の seek (シークバーのクリック・キー・ホイール・frame step・続きから再生・チャプター・ブックマーク・ループ) | `src/video/decoder.rs` demux thread の seek 処理 (1 か所に集まっている) | 新しい seek 要求・停止 |
| hover / サムネイル | `src/video/thumbnail.rs` | `cancel` と request の supersede |
| タイル | `src/video/tile_thumbnails.rs` | 既存の取消 |
| シークストリップの枠 | `src/video/seek_strip_thumbs.rs` | 既存の取消 |
| スクリーンショット | `src/video/screenshot.rs` | なし (1 回きり) |
| upscale の切り出し | `src/video/upscale/job.rs` | 既存の取消 |
| Remote の配信 (時計なし transcode) | `src/video/clockless_transcode.rs` | 既存の取消 |

- `src/video/seek_strip.rs` の warmup (索引を作るための捨てシーク) は置き換えない (下の §4)。
- `src/audio_decode.rs` (音声の範囲 decode) は音声 stream の seek で、音声 packet はすべて key なので対象外。
- 実装担当は上の表が全部かを grep (`av_seek_frame` / `avformat_seek_file` / `Input::seek`) で確かめ、漏れがあれば足す。

## 4. シークストリップの軸

TS / PS の `AVIndexEntry` は `mpegts_get_dts` が探索中に登録した点で keyframe ではないので、`mpegts` / `mpegtsraw` /
`mpeg` の Input では索引を読まずに時刻格子 (`StripAxis::TimeGrid`) を使う (新しい `TimeGridReason` を足す)。warmup の
捨てシークもこれらでは不要になる。各枠の画は §3 の seek で目標以前の key から復号されるので、正しい時刻の画になる。
今は `KeyframesTooSparse` で全部の枠が空になる (サブ PC の依頼 51 の観測)。

## 5. 割り切り

- 32 秒を超える GOP・key の印が無い stream・seek の失敗は、今までと同じ seek に戻してログに出す (まれな失敗は割り切り)。
- 確認の走査中の読み取りエラーは「その back では失敗」として扱い、回復処理は足さない。
- 同じ引数の seek が別の着地点へ着く場合 (probe では起きなかった) は、復号が次の GOP から始まるだけで、今と同じ症状に
  戻る。検出や再試行はしない。

## 6. テスト

素材は新しい `testdata/mpeg-keyframe-seek/` に、システムの `ffmpeg` と lavfi だけで作る (生成スクリプトは
`scripts/ui-smoke/generate_mpeg_keyframe_seek_fixture.py`、既存の `generate_timeline_origin_fixture.py` と同じ流儀)。
**1 秒ごとに単色が変わる映像** で、通常の GOP を持つもの:

- TS (H.264 + AAC、GOP 0.5 秒、B frame あり、小さな原点)
- TS (MPEG-2 + MP2、GOP 0.5 秒、B frame あり、`-output_ts_offset 36000`)
- PS (MPEG-2 + MP2、GOP 0.5 秒、B frame あり)
- 長い GOP の TS (GOP 6 秒、back を 2 回以上倍にする経路)
- 対照の MP4 (同じ映像) — 動きが変わらないこと

受入条件:

- `keyframe_seek` の単体 (実素材): 各素材の目標 (0.5 秒刻み、各 GOP の key の pts ちょうど、最後の key より後ろ、先頭より前、
  D 以後) で、seek 後に最初に読める映像 key packet の時刻 <= 目標 (先頭より前は StartOfStream)。長い GOP で back が倍になる。
  MP4 は `Direct` で今と同じ着地。取消が立つと `Aborted` で止まる。
- 復号 (worker 層): サムネイル・スクリーンショット・タイル・ストリップの枠で、時刻 t を求めて返る画の**色**が t の色。
  最後の key より後ろ (D − 0.3 秒など) でも画が返る。
- 再生 decoder: TS / PS で seek(t) の後の最初の表示 frame の時刻と色が t。frame step の前後、D − 0.3 秒への seek の後の
  最初の frame と EOF の時刻。
- シークストリップ: TS / PS の軸が `TimeGrid` になり、枠が空にならない。MP4 の軸の判定は今と同じ。
- Remote: 開始位置 t の transcode で、最初の segment の映像が t の色。
- 既存の `testdata/timeline-origin/` の試験 (全 intra) は変えずに通る。

## 7. 文書

- `docs/video-architecture.md` に「TS / PS の seek」の節を足す (§2 の契約、§3 の所有、対象 demuxer)。
- `docs/next-release-backlog.md` §1.785 の状態を更新する。公開文書 (マニュアル・製品ページ) は、seek が正しくなるだけなので
  変えない。更新履歴は公開作業で書く。
