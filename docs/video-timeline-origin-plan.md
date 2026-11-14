# 動画の時刻原点 (コンテナ開始時刻が 0 でないファイル) の設計

2026-11-14 作成。設計担当 ClaudeCode Opus 5.5。対象ブランチ `fix-video-timeline-origin`。
実装前の独立レビュー (GPT-6.1 Sol / xhigh、thread `01a1d763-b4c7-73d2-9329-59e5af4393df`) の指摘で改訂済み。

## 1. 観測と原因

### 1.1 観測の出どころ

- コード参照 (下の表) と、同梱 FFmpeg 7.1 で合成 TS を読んだ値 (別セッションの記録):
  AAC だけの TS は O=1.400000 / D=3.818667、MPEG-2 映像 + AAC の TS は O=1.578667 / D=4.021333。
  O>0 の MKV (`offset_tracks.mka`、T1 ブランチの fixture) は O=10 / D=14 (内容は約 4 秒)。
- 開発機の ffprobe で作った合成 TS (`-output_ts_offset 36000`): format start_time=36001.376778、duration=6.023222。
- 同梱 avformat で `AVFMT_TS_DISCONT` を持つ demuxer を列挙した結果 (2026-11-14、python ctypes):
  `mpegts`、`mpegtsraw`、`mpeg` (MPEG-PS)、`ogg`、`m4v`、`dhav`、`live_flv`、`hls`、`rcwt`、`ty`。
  この flag は原点の判定には使わない (§2)。
- 実機での再生挙動は**未確認** (エージェントは製品バイナリを起動しない)。下の「壊れ方」はコードから導いたもの。

O = コンテナの開始時刻 (`AVFormatContext.start_time`)、D = `VideoInfo.duration_secs` (= `AVFormatContext.duration`)。

### 1.2 原因

再生側の時刻が 2 つの原点を混ぜている。

- FFmpeg から来る時刻は raw PTS 秒で、O を含む: 映像 `decoder.rs` の `pts * time_base`、音声 `audio_frame_timestamp`、
  チャプター `AVChapter.start`、音声トラック範囲 `stream.start_time()`。seek も `av_seek_frame(-1, target*1e6)` の
  raw 値。`VideoPlayer::position()` はこの raw 値で進む。
- シークバー・時刻表示・seek の clamp・続きから再生の判定・EOF の凍結位置・frame step の端判定は、位置を
  `[0, D]` として扱う。

**D が O を含むかどうかは demuxer ごとに違う。**

- MPEG-TS / MPEG-PS のように時刻が自由に走る時計の値で、コンテナに時刻 0 が無い形式は、FFmpeg が PTS から
  `end − start` として長さを推定する。D は O を含まない。位置は `[O, O+D]` で、UI と clamp の `[0, D]` と食い違う。
- Matroska は Segment の Duration をそのまま使い、FFmpeg の muxer はそこに最大終了時刻を書く。O=10 秒・内容 4 秒で
  D=14 になる。コンテナ自身に 0 があり、先頭 10 秒は「何も無い区間」として位置 `[10, 14]` ⊂ `[0, 14]` で整合する。
- MP4 (edit list)、MP3 (encoder delay で start_time≈0.025)、MKV は、コンテナ自身の 0 が原点で、今の挙動で整合している。

壊れているのは前者 (時計の値の形式) だけ。O>0 の MPEG-TS / M2TS / MPEG-PS で次のように壊れる (コード由来、未確認)。

| 箇所 | 壊れ方 (小さな O / 録画 TS のような大きな O) |
| --- | --- |
| `VideoPlayer::clamp_seek_target` `[0, D-0.1]`、`EngineActor::handle_seek_request` の clamp | 最後の約 O 秒へ移れない / どの seek も先頭へ戻る |
| シークバー塗り・時刻表示 (`render_core.rs`、`bottom_hud.rs`、`overlay_draw.rs`) | O 秒早く満ちる / 常に 100%、「2:46:40 / 1:00:00」 |
| シークバーのクリック・ホバー縮小画像・シークストリップ (`duration * frac`) | O 秒手前へ飛ぶ / 先頭へ飛ぶ |
| 相対 seek の境界 (`seek_relative`) | 末尾の O 秒に届かない / → が常に AtEnd、← の AtStart が出ない |
| frame step の端判定・`clamp_exact_frame_target` | 端の判定がずれる / 前進が拒否される |
| EOF の凍結 `EofReached { duration }` | 位置が O+D から D へ戻る |
| ブックマーク・ピン追加 (`finite_video_target_secs` で `[0, D]` に clamp) | 末尾 O 秒の位置が D に張り付く / O ≥ D なら全部 D になる |
| 続きから再生の保存 (`save_video_resume_position`、`position >= D-5` で削除) | 末尾付近で早めに視聴済み / O ≥ D−5 なら毎回削除され視聴済みになる |
| 続きから再生の復元 (`sanitize_resume_for_duration`) | 末尾付近を捨てる / 全部捨てる |
| チャプター・CH/BM ループ (`clamp_loop_seek_target`) | 末尾付近のチャプターへ届かない / ループが毎回 0 へ戻る |
| Remote (`source_origin_secs` は raw、クライアントは `[0, D]`) | スライダーが O 秒ずれる / 位置が D に張り付く |
| SMTC・Now Playing の位置 | 長さを超える |
| 一覧のサムネイル・タイル・シークストリップの worker | 求めた時刻より O 秒前の画 / 先頭の画 |

## 2. 契約

**mIV の動画の時刻は、すべて「メディア時刻」t で扱う。t = raw PTS 秒 − O_eff。表示上の時刻 = seek の引数 =
保存値 = メディア時刻で、seek できる範囲は `[0, D]`。**

- **O_eff** (原点) は次で決める。
  - demuxer 名 (`AVInputFormat.name`) が `mpegts`・`mpegtsraw`・`mpeg` (MPEG-PS) のどれか: O_eff =
    `AVFormatContext.start_time`。`AV_NOPTS_VALUE` または有限でない値なら 0。これらは時刻が自由に走る時計の値で
    コンテナに 0 が無く、長さを持つ header も無いので、FFmpeg が PTS から `end − start` として長さを推定する。
  - それ以外 (MP4・MKV・MP3・Ogg・FLV・AVI・FLAC 等): O_eff = 0 (今の挙動のまま)。
  対象 demuxer の一覧は `MediaTimeline` の中だけに 1 つ置き、「長さが `end − start` であることを確かめた
  demuxer」として理由を書く。足すときは、その demuxer の長さの決め方を FFmpeg のソースで確かめてからにする。
  `AVFMT_TS_DISCONT` (時刻の不連続を許すだけの flag) では判定しない。`live_flv` のように flag を持ちながら
  metadata の長さや最後の tag の raw 時刻を長さにする demuxer があり、補正すると EOF と長さが食い違う。
  Ogg も flag を持ち、`-output_ts_offset` で開始 ≠ 0 の Ogg を作れるが、通常のファイルは開始 0 で今の不具合の
  報告も無いので対象外にする。
- O_eff が 0 のファイル (TS/PS 以外の全部と、開始 0 の TS/PS) では変換は恒等で、挙動は変わらない。
- raw PTS は **FFmpeg を直接呼ぶ箇所の中だけ**に閉じる。変換は新しい型 `MediaTimeline`
  (`src/video/media_timeline.rs`) のメソッドだけで行い、`start_time` を読む箇所・O_eff を足し引きする箇所を
  他に作らない。`MediaTimeline::from_input(&Input)` が O_eff を決める唯一の場所。
- 再生 decoder では、demux thread が `Input` を開いたときに 1 つ作り、不変値として映像・音声の decode lane へ渡す
  (lane は `Input` を持たない)。別に `Input` を開く補助 worker (サムネイル・シークストリップ・Remote の transcode・
  範囲デコード) は、自分の `Input` から独立に作る (同じファイルなら同じ O_eff になる)。
- それより上 (clock、engine、`VideoPlayer`、App、native presenter、Remote、保存値、Now Playing、SMTC、
  文字起こし) はメディア時刻だけを見る。既存の clamp `[0, D-0.1]` と EOF 凍結 D がそのまま正しくなる。
- `VideoInfo` に `timeline_origin_secs` (= O_eff) を持たせる。用途は §4 の移行とログ・診断だけで、時刻の変換には
  使わない (変換は `MediaTimeline` だけ)。

mpv の `--rebase-start-time` / ffmpeg CLI の入力 offset と同じ考え方を、長さの意味が `end − start` と一致する
形式に限って適用する。

### 2.1 採らなかった案

- **全形式で `start_time` を原点にする (mpv と同じ)**: Matroska では D が O を含むので、内容が `[0, D−O]` に縮み、
  EOF と D が食い違う (今は整合している形式を壊す)。MP3・Ogg Opus の encoder delay / pre-skip は、音声 decoder に
  `pkt_timebase` を渡していないため FFmpeg が PTS を補正せず、`−O` で先頭の可聴サンプルが負時刻になる。
  形式ごとに D の意味を補正する処理が要り、今壊れていない形式の挙動も変える。採らない。
- **raw のまま、`VideoInfo` に O を足して UI 側で引く**: O を引く・足す箇所が上の表の 20 か所以上と Remote
  クライアントに散り、1 か所でも漏れると今と同じ食い違いになる。採らない。
- **`VideoPlayer` の公開 API だけ 0 起点にする**: engine actor の clamp、EOF 凍結、resume、frame step、Remote の
  transcode、サムネイル worker が raw のまま残り、変換箇所が engine の内外に分かれる。採らない。

### 2.2 状態の組み合わせ

原点を 1 つにすることで「raw と 0 起点の組み合わせ」自体を無くす。O_eff は `Input` を開いたときに決まる定数で、
再生中に変わらない。新しい状態・フラグ・分岐は §4 の移行の印以外に足さない。

## 3. FFmpeg 境界 (変換を入れる場所)

時刻を 4 種類に分け、扱いを決める。

| 種類 | 例 | 扱い |
| --- | --- | --- |
| 入力の絶対時刻 (FFmpeg → mIV) | 映像・音声 frame PTS、filter 出力 PTS を秒にしたもの、packet PTS を秒で比較する箇所、チャプター、`AudioTrackInfo.start_secs/end_secs`、stream の `start_time`、ログ・perf event の時刻 | 秒にした時点で `MediaTimeline` で media へ |
| seek の locator (mIV → FFmpeg) | `av_seek_frame` / `input.seek` / stream 指定 seek の target、media 秒を stream tick に戻して比較する箇所 | 渡す直前に `MediaTimeline` で raw へ |
| raw のまま持つ locator | `AVIndexEntry.timestamp` (`seek_strip.rs` の索引、索引 warmup seek)、packet DTS→PTS 対応 (`seek_strip_thumbs.rs`)、upscale manifest に保存した source seek PTS (`upscale/manifest.rs`、`upscale/job.rs` の stream 指定 seek) | raw のまま内部で使う。mIV の時刻として外へ出す・mIV の時刻と比べる箇所だけ変換する。upscale manifest は raw の locator なので移行不要 |
| 出力独自の時刻 | encoder の PTS (`mvexport`、upscale の encoder)、元音声 packet の remux 出力、Remote の encoded segment | 変えない。O_eff を足さない |

**Remote**: 入力の source PTS と `source_origin_secs` は media。encoded segment の時刻は今どおり配信開始位置を
引いた session 相対 (`stream/timeline.rs` の `StreamTimeline::relative_secs`)。クライアントは
`source_origin + currentTime` で動画位置を復元するので、クライアントは変えない。

実装担当は次の検索で FFmpeg と時刻をやり取りする箇所を**全部列挙し、各箇所を上の 4 種類のどれとして扱ったかを
実装報告に表で書く** (時刻の差だけを使うので不要、時刻を使わない、も含める)。

```
rg -n "time_base\(\)|av_seek_frame|avformat_seek_file|\.seek\(|best_effort_timestamp|\.pts\(\)|\.dts\(\)|\.timestamp\(\)|start_time|\.chapters\(\)|AV_TIME_BASE|index_entries|AVIndexEntry" src crates
```

既知の箇所 (網羅ではない): `src/video/decoder.rs`、`thumbnail.rs`、`tile_thumbnails.rs`、`seek_strip.rs`、
`seek_strip_thumbs.rs`、`seek_strip_batch.rs`、`screenshot.rs`、`clockless_transcode.rs`、`src/audio_decode.rs`
(範囲デコード。全量デコードはサンプル数基準なので変えない)、`upscale/`、`mvexport/`、`normalize_scanner.rs`。

注意:

- 時刻の差 (duration、間隔、連続性、resampler のサンプル数) は O_eff の影響を受けない。差にだけ足し引きしない。
- bwdif・frame-step の filter time base 換算と resample timeline は、raw の内部表現を保ち、秒の絶対時刻として
  mIV 側へ出す点で変換するのが最小。変換点を lane の中で 1 か所に寄せる。
- media 時刻が僅かに負になる frame (open GOP の先頭) は、今の 0 秒開始と同じ扱いでよい。
- TS の 33bit PTS 折り返しは FFmpeg が unwrap する (fixture で確認する)。放送 TS の途中の時刻不連続は対象外。

## 4. 保存済みデータ

ブックマーク (`video_bookmarks.pts_secs`)、ピン (`video_pins.pin_pts_secs` / `thumb_pts_secs`)、続きから再生
(`video_resume_positions.position_secs`) はリリース済みのデータ。修正後はメディア時刻として読む。影響は
O_eff ≠ 0 のファイル (MPEG-TS / PS 系) だけ。

今の保存値の中身 (コード由来):

- 続きから再生: 常に再生位置 (raw)。`position >= D−5` で削除されるので、O ≥ D−5 のファイルには行が無い。
  O < D の録画 TS (放送の時計値は 0〜26.5 時間でばらつく) やカメラの TS (O は数秒以下) には raw 値の行がある。
- ブックマーク・ピン: 再生位置から付けたもの (raw を `[0, D]` に clamp)、時刻の貼り付け
  (`native_video.rs` の一括登録) や時刻指定 (`add_native_video_bookmark` の `target_secs`) で付けたもの
  (0 起点の値) が同じ列に混ざり、区別できない。一律に O を引くと後者を壊す。

**判断 (利用者決定、2026-11-14、統括経由)**: 続きから再生だけ移行し、ブックマーク・ピンは移行しない。
初回は「O が大きいと有効な保存値はほぼ無い」という誤った前提で「移行しない」と答えてもらったため、上の訂正を
添えて取り直した。

- 続きから再生: `video_resume_positions` に「メディア時刻で保存した」印の列を足す (旧行は印なし)。新しく保存する
  行は印付き。**印は「その値が実際にメディア時刻で書かれた」ときだけ付ける**。今の保存は表を全削除して map の
  全件を INSERT し直す (`settings_db.rs` の全量保存) ので、map と全量の読み書きでも旧 raw / メディア時刻の区別を
  保ち、開いていない動画の旧 raw 行を別の設定保存のついでに印付きへ変えない。読み込んだ値は「メディア時刻」か「旧 raw」かを型で持ったまま (bool や sentinel にしない) player
  へ渡し、O_eff が分かる既存の復元判定の時点 (`sanitize_resume_for_duration` の呼び出し元、`VideoInfo` 受領時) で
  旧 raw を `value − O_eff` に変換してから判定する。O_eff = 0 なら値はそのまま。値 < O_eff の旧 raw は再生位置と
  してあり得ないので復元しない。行の書き直しは専用の経路を作らず、既存の定期保存 (印付き) に任せる
  (保存されないまま閉じた旧行は、次に開いたときも同じ変換を受けるだけ)。
  続きから再生の縮小画像は下のキャッシュの扱いで作り直される。
- ブックマーク・ピン: 再生位置から付けたもの (raw) と、時刻の貼り付け・時刻指定で付けたもの (0 起点) を区別
  できないので移行しない。O_eff ≠ 0 のファイルで再生位置から付けたものは、修正後に O_eff だけ後ろの位置へ移る。

時刻をキーにしたキャッシュ (`tile_thumb_cache.rs` のタイル縮小画像・続きから再生の縮小画像・波形) は、O_eff ≠ 0
のファイルで旧 raw 時刻の画を持っている。キャッシュなので、DB の `PRAGMA user_version` を使う gate で 1 回だけ
**MPEG-TS / PS 系の拡張子の行を捨てて作り直す** (全動画の作り直しは一覧の表示が一時的に遅くなるので避ける。
拡張子と中身が食い違うファイルは古い画が残り得るが、キャッシュなので割り切る)。O_eff の対象 demuxer
(`mpegts`・`mpegtsraw`・`mpeg`) と拡張子の範囲を揃える (例: ts・m2ts・mts・m2t・tp・trp・tts・mpg・mpeg・m2p・vob・
mod・tod・evo。通常一覧・Remote の対応動画拡張子と突き合わせた結果、現在重なるのは mpg・mpeg。
直接開いたファイル等の旧キャッシュも対象にするため、上の TS/PS 拡張子はすべて保持する)。一回だけの削除は、全体の
`THUMB_RENDER_VERSION` を上げる方法と混ぜない (上げると他形式の行も miss して全部作り直しになる)。ブックマーク・ピンの縮小画像 BLOB は
付けた時点の表示 frame なので変えない。

**実装時の前提訂正・設計担当決定 (2026-11-14)**: 既存コードに DB 全体の version gate はなく、
行の render/format version 判定だけだった。未使用の `PRAGMA user_version` を gate として新設する。
値 0 の旧 DB は、タイル・resume・旧波形・トラック別波形の 4 表から上記拡張子の行だけを削除し、
同じトランザクションで値 1 に進める。値 1 以上では削除せず、再生成した TS/PS の行と将来の版を保持する。
`THUMB_RENDER_VERSION` と波形の format version は変更しない。

**一覧の続きから再生メーター (設計担当決定、2026-11-14)**: 一覧のメーター (`book_resume_meter.rs` の
`thumbnail_resume_meter`) は FFmpeg を開かずに catalog の長さで割るので、旧 raw 行の O_eff を知らない。旧 raw 行は、
その動画を開いて定期保存されるまで、今と同じ値 (raw ÷ D) で描く。O_eff = 0 のファイルでは値は正しく、O_eff ≠ 0 の
ファイルでも修正前と同じ表示で退行ではなく、一度再生すれば直る。catalog に原点の列を足して旧行を再 probe する案は、
全動画の catalog 行の再 probe が要るのに直るのは一時的な表示だけなので採らない。

## 5. 文字起こし T1 との関係

T1 (`feat-asr-t1`、未出荷) は文字起こしの時刻を「コンテナ開始 = 0」で保存し、`src/video/transcript_time.rs` で
`+ container_start_secs` して再生側の raw 秒へ変換している。この修正後の再生側の時刻はメディア時刻なので、
T1 の保存時刻もメディア時刻 (O_eff 基準) に揃えるのが一貫する。TS/PS では O_eff = container start で変換は恒等、
MKV では O_eff = 0 なので T1 の「コンテナ開始 = 0」と食い違う (O>0 の MKV で文字起こしが O だけずれる)。
T1 側のファイルはこのブランチで触らない。master へ後から入る側が合わせる (統括と合意済み)。

## 6. テスト (受入条件)

fixture は `testdata/timeline-origin/` に、システムの ffmpeg と lavfi だけで作る小さな素材を置き、生成スクリプトを
`scripts/ui-smoke/generate_timeline_origin_fixture.py` に置く (既存 `generate_audio_tracks_fixture.py` と同じ流儀)。
**1 秒ごとに単色が変わる映像** (時刻を画素で判定できる) で、長さは 12 秒程度 (続きから再生の保存可能区間
`3 <= position < D−5` を作るため D > 8)。

**fixture の分離 (2026-11-14 設計担当決定)**: TS/PS の timestamp seek が非 key packet に着地する既存不具合は
[バックログ §1.780](next-release-backlog.md) へ分離し、このブランチでは修正しない。原点の変換漏れを検出する
素材は 12 fps・全 intra (`-g 1 -bf 0`) にし、低 fps で PTS/DTS の一フレーム差が seek の厳密な期待値へ
影響しないよう MPEG-2 の low delay も指定する。色と tail の許容誤差は緩めず、frame-step の隣接 frame 時刻は
この frame grid に合わせる。インターレース属性は維持し、既存の別素材による inter-frame / filter 回帰も残す。

- 小さな O の TS (MPEG-2 + AAC、自然な O≈1.4〜1.6 秒)
- 大きな O の TS (`-output_ts_offset 36000`)
- 折り返しをまたぐ TS (`-output_ts_offset` を 2^33/90000 ≈ 95443.7 秒の数秒手前)
- インターレースの MPEG-2 TS (bwdif の経路) と、後から始まる 2 本目の音声トラックを持つ TS
- O>0 の MKV (チャプター付き) — **変わらないこと**の確認用 (O_eff = 0)
- MP3 (encoder delay あり) — 変わらないことの確認用

受入条件:

- `MediaTimeline` の単体: demuxer 名による O_eff (対象外の `ogg`・`matroska,webm`・`mov,mp4,...` は 0)、NOPTS → 0、raw ↔ media の往復、AV_TIME_BASE と
  stream tick の丸め。
- decoder 層: TS 各 fixture で最初の映像・音声 frame の時刻が 0 付近、`D−0.5` への seek の後の最初の frame の
  時刻と**画素の色**が D−0.5 付近、EOF の時刻が D 付近、チャプター・音声トラック範囲が media。
  bwdif の前後 frame-step と EOF drain が非ゼロ O で動く。後から始まる音声トラックの範囲判定と切り替えの保留が
  media で正しい。MKV / MP3 / 既存 O=0 fixture の時刻が変更前と同じ。
- `VideoPlayer` / engine の状態遷移: 大きな O の TS で `seek(D−1)` が clamp されずに届く、`seek_relative(+)` が
  末尾手前で AtEnd を返さない、EOF の凍結位置が D、続きから再生の保存 → 再読込 → 復元が media 値で通る。
- サムネイル系 worker: 時刻 t を求めて返る画の**色**が t の色 (返り値の時刻だけでは判定しない)。
  `AudioRangeDecoder` は既知の位置のパルスで判定する。
- Remote: 開始位置 t の transcode で、`source_origin_secs` = t、最初の segment の時刻 ≈ 0、端末側の位置 = t。
- キャッシュ: 旧 version の TS 系の行が捨てられ、他の行は残る。
- 続きから再生の移行: schema の列追加が既存 DB で冪等、旧 raw 行が大きな O の TS で `value − O_eff` に復元される、
  値 < O_eff の旧行は復元しない、O_eff = 0 のファイルの旧行は値が変わらない、定期保存で印付きに置き換わる、
  印付きの行は二重に変換されない。旧行の動画を開かずに別の設定を保存 → 再起動 → その動画を開くと、旧 raw として
  変換される (全量保存で印が付かない)。ブックマーク・ピンの値は変わらない。

## 7. 文書

- `docs/video-architecture.md` に「時刻の原点 (メディア時刻)」の節を足し、契約と `MediaTimeline` の所有を書く。
- `docs/web-remote-video-streaming-plan.md` の `source_origin_secs` の説明をメディア時刻に揃える。
- 公開文書 (マニュアル・製品ページ) は、時刻の表示が正しくなるだけなので変えない。更新履歴は公開作業で書く。

## 8. 対象外とまれな失敗

- TS/PS の keyframe より後ろへの seek 着地は原点とは独立した既存不具合で、§1.780 の共通 seek owner と
  bounded な cold seek 設計で別途扱う。このブランチに byte locator、走査、固定 preroll は追加しない。
- 音声 decoder に `pkt_timebase` を渡していない件 (MP3・Ogg Opus の encoder delay 補正が PTS に反映されない) は、
  この修正の原点の判定 (O_eff = 0) では影響しないので対象外。別の項目としてバックログに記録する。
- O_eff を読めないファイルは O_eff=0 として今と同じに扱う (ログに出す)。それ以外の回復処理は足さない
  (まれな失敗は割り切り)。
