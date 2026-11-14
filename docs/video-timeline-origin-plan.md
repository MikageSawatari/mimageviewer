# 動画の時刻原点 (コンテナ開始時刻が 0 でないファイル) の設計

2026-11-14 作成。設計担当 ClaudeCode Opus 5.5。対象ブランチ `fix-video-timeline-origin`。

## 1. 観測と原因

### 1.1 観測の出どころ

- コード参照 (下の表) と、同梱 FFmpeg 7.1 で合成 TS を読んだ値 (別セッションの記録):
  AAC だけの TS は O=1.400000 / D=3.818667、MPEG-2 映像 + AAC の TS は O=1.578667 / D=4.021333。
- 開発機の ffprobe で作った合成 TS (`-output_ts_offset 36000`): format start_time=36001.376778、duration=6.023222。
- 実機での再生挙動は**未確認** (エージェントは製品バイナリを起動しない)。下の「壊れ方」はコードから導いたもの。

O = コンテナの開始時刻 (`AVFormatContext.start_time`)、D = `AVFormatContext.duration` (開始を含まない長さ)。

### 1.2 原因

再生側の時刻が 2 つの原点を混ぜている。

- FFmpeg から来る時刻は raw PTS 秒で、O を含む: 映像 `decoder.rs` の `pts * time_base`、音声 `audio_frame_timestamp`、
  チャプター `AVChapter.start`、音声トラック範囲 `stream.start_time()`。seek も `av_seek_frame(-1, target*1e6)` の
  raw 値。`VideoPlayer::position()` はこの raw 値で進む。
- 長さ `VideoInfo.duration_secs` は D で、O を含まない。シークバー・時刻表示・seek の clamp・続きから再生の判定・
  EOF の凍結位置・frame step の端判定は、位置を `[0, D]` として扱う。

O=0 のファイル (MP4 / MKV の大半) では 2 つが一致するので表に出ない。O>0 の MPEG-TS / M2TS / MPEG-PS で、
位置は `[O, O+D]`、UI と clamp は `[0, D]` になり、次のように壊れる (コード由来、未確認)。

| 箇所 | 壊れ方 (小さな O / 録画 TS のような大きな O) |
| --- | --- |
| `VideoPlayer::clamp_seek_target` `[0, D-0.1]`、`EngineActor::handle_seek_request` の clamp | 最後の約 O 秒へ移れない / どの seek も先頭へ戻る |
| シークバー塗り・時刻表示 (`render_core.rs`、`bottom_hud.rs`、`overlay_draw.rs`) | O 秒早く満ちる / 常に 100%、「2:46:40 / 1:00:00」 |
| シークバーのクリック・ホバー縮小画像・シークストリップ (`duration * frac`) | O 秒手前へ飛ぶ / 先頭へ飛ぶ |
| 相対 seek の境界 (`seek_relative`) | 末尾の O 秒に届かない / → が常に AtEnd、← の AtStart が出ない |
| frame step の端判定・`clamp_exact_frame_target` | 端の判定がずれる / 前進が拒否される |
| EOF の凍結 `EofReached { duration }` | 位置が O+D から D へ戻る |
| ブックマーク・ピン追加 (`finite_video_target_secs` で `[0, D]` に clamp) | 末尾 O 秒の位置が D に張り付く / 全部 D になる |
| 続きから再生の保存 (`save_video_resume_position`、`position >= D-5` で削除) | 末尾付近で早めに視聴済み / **毎回削除され視聴済みになる** |
| 続きから再生の復元 (`sanitize_resume_for_duration`) | 末尾付近を捨てる / 全部捨てる |
| チャプター・CH/BM ループ (`clamp_loop_seek_target`) | 末尾付近のチャプターへ届かない / ループが毎回 0 へ戻る |
| Remote (`source_origin_secs` は raw、クライアントは `[0, D]`) | スライダーが O 秒ずれる / 位置が D に張り付く |
| SMTC・Now Playing の位置 | 長さを超える |
| 一覧のサムネイル・タイル・シークストリップの worker (`thumbnail.rs`、`tile_thumbnails.rs`、`seek_strip_thumbs.rs`) | 求めた時刻より O 秒前の画 / 先頭の画 |

## 2. 契約

**mIV の動画の時刻は、すべて「メディア時刻」t ∈ [0, D] で扱う。t = raw PTS 秒 − O。**

- O は `AVFormatContext.start_time` (AV_TIME_BASE 単位)。`AV_NOPTS_VALUE` または有限でない値なら O = 0
  (今の挙動と同じ)。O が負でも同じ式で扱う (開始 0 に揃う)。
- raw PTS は **FFmpeg を直接呼ぶ箇所の中だけ**に閉じる。FFmpeg から時刻を受け取った時点でメディア時刻へ、
  FFmpeg へ時刻を渡す直前に raw へ変換する。変換は新しい型 `MediaTimeline` (`src/video/media_timeline.rs`) の
  メソッドだけで行い、`start_time` を読む箇所・O を足し引きする箇所を他に作らない。
- `MediaTimeline` は `Input` ごとに作る (worker が別に開いた `Input` も、同じファイルなら同じ O になる)。
  O を worker 間で受け渡さない。
- それより上 (clock、engine、`VideoPlayer`、App、native presenter、Remote、保存値、Now Playing、SMTC、
  文字起こし) はメディア時刻だけを見る。**表示上の時刻 = seek の引数 = 保存値 = メディア時刻**で、
  seek できる範囲は `[0, D]` (既存の clamp `[0, D-0.1]` と EOF 凍結 D がそのまま正しくなる)。
- O=0 のファイルでは変換は恒等で、挙動は変わらない。

mpv の `--rebase-start-time=yes` (既定) と同じ考え方。

### 2.1 採らなかった案

- **raw のまま、`VideoInfo` に O を足して UI 側で引く**: O を引く・足す箇所が上の表の 20 か所以上と Remote
  クライアントに散り、1 か所でも漏れると今と同じ食い違いになる。保存値も raw のままで、同じ動画を作り直すと
  (remux 等) 位置が合わなくなる。採らない。
- **`VideoPlayer` の公開 API だけ 0 起点にする**: engine actor の clamp、EOF 凍結、resume、frame step、Remote の
  transcode、サムネイル worker が raw のまま残り、変換箇所が engine の内外に分かれる。採らない。

### 2.2 状態の組み合わせ

原点を 1 つにすることで「raw と 0 起点の組み合わせ」自体を無くす。新しい状態・フラグ・分岐は足さない
(O は `Input` を開いたときに決まる定数で、再生中に変わらない)。

## 3. FFmpeg 境界 (変換を入れる場所)

実装担当は、次の検索で FFmpeg と時刻をやり取りする箇所を**全部列挙し、各箇所の扱いを実装報告に表で書く**
(変換した / 時刻の差だけを使うので不要 / 時刻を使わない)。

```
rg -n "time_base\(\)|av_seek_frame|avformat_seek_file|\.seek\(|best_effort_timestamp|\.pts\(\)|\.timestamp\(\)|start_time\(\)|\.chapters\(\)|AV_TIME_BASE" src crates
```

既知の箇所 (網羅ではない):

- `src/video/decoder.rs`: 映像 frame PTS (`pts_secs = pts * tb`、filter 出力 PTS、frame-step の選別)、
  音声 frame PTS と resample timeline、packet PTS、seek (`av_seek_frame` / `input.seek`)、チャプター、
  `AudioTrackInfo.start_secs/end_secs`、ログ・perf event の時刻。
- サムネイル系 worker: `thumbnail.rs`、`tile_thumbnails.rs`、`seek_strip_thumbs.rs`、`seek_strip_batch.rs`、
  `seek_strip.rs`、`screenshot.rs`、`seek_strip_wave.rs` が使う `src/audio_decode.rs` の範囲デコード。
- Remote: `clockless_transcode.rs` (要求位置と `source_start` を media で扱い、出力 segment の時刻も media)。
- その他 FFmpeg を開いて時刻を扱うもの (`mvexport`、`upscale`、`normalize_scanner` 等) は列挙で判断する。

注意:

- frame PTS を media へ変換する位置より下流 (preroll trim、frame step、pacing、A/V 同期、音声の連続性判定) は
  media で揃う。**media 秒を stream tick へ戻して比較している箇所**があれば、その変換も `MediaTimeline` を通す。
- 時刻の差 (duration、間隔、連続性) は O の影響を受けない。差にだけ O を足し引きしない。
- media 時刻が僅かに負になる frame (open GOP の先頭、音声の priming) は、今の 0 秒開始と同じ扱いでよい
  (既存の負の audible PTS の扱いを変えない)。
- TS の 33bit PTS 折り返しは FFmpeg が unwrap する。放送 TS の途中の時刻不連続は対象外 (今も D が壊れる)。

## 4. 保存済みデータ

ブックマーク (`video_bookmarks.pts_secs`)、ピン (`video_pins.pin_pts_secs` / `thumb_pts_secs`)、続きから再生
(`video_resume_positions.position_secs`)、続きから再生の縮小画像 (`video_resume_thumbs.timestamp_ms`) は
リリース済みのデータ。修正後はメディア時刻として読む。

- O=0 のファイル: 値の意味は変わらない。
- O が大きいファイル (録画 TS): 今のコードではブックマーク・ピンは D に clamp され、続きから再生は保存されず
  視聴済みになっている。失う有効なデータはほぼ無い。
- O が小さいファイル (〜2 秒): 既存の位置が O 秒後ろへずれる。

**移行しない (利用者判断待ち、2026-11-14 に統括経由で質問済み)**。移行するには保存先 3 つに旧形式の印と、
開いたときに O を引いて書き直す経路が要り、得られるのは O が小さいファイルの数秒以下の補正だけ。
回答で移行が必要になったら、この節を書き換えて別の区切りで足す。

誤って付いた「視聴済み」の印 (`video_watched_to_end`) も戻さない (どれが誤りか区別できない)。

## 5. 文字起こし T1 との関係

T1 (`feat-asr-t1`) は文字起こしの時刻を「コンテナ開始 = 0」で保存し、`src/video/transcript_time.rs` で
`+ O` して raw PTS へ変換している。この修正後は再生側もメディア時刻なので、その変換は恒等になる。
T1 側のファイルはこのブランチで触らない。master へ入る順に応じて、T1 側で変換を外す (統括へ連絡済み)。

## 6. テスト (受入条件)

- `MediaTimeline` の単体: NOPTS → 0、raw ↔ media の往復、AV_TIME_BASE と stream tick の丸め、負の O。
- fixture: `testdata/timeline-origin/` に、システムの ffmpeg と lavfi だけで作る小さな素材を置き、生成スクリプトを
  `scripts/ui-smoke/generate_timeline_origin_fixture.py` に置く (既存 `generate_audio_tracks_fixture.py` と同じ流儀)。
  - 小さな O の TS (MPEG-2 + AAC、自然な O≈1.4〜1.6 秒)
  - 大きな O の TS (`-output_ts_offset 36000`)
  - O>0 でチャプター付きの MKV (ffmetadata で 2〜3 チャプター)
- decoder 層: 各 fixture で `VideoInfo.duration_secs` = D、最初の映像・音声 frame の時刻が 0 付近、
  `D-0.5` への seek の後の最初の frame が `[D-0.5-許容, D]`、チャプター時刻が media。
- `VideoPlayer` / engine の状態遷移: 大きな O の素材で、`seek(D-1)` が clamp されずに届く、`seek_relative(+)` が
  末尾手前で AtEnd を返さない、EOF の凍結位置が D、resume の保存が `position < D-5` で削除されない。
- サムネイル worker: 時刻 t を求めたときに返る frame の時刻が t 付近 (先頭の画ではない)。
- Remote: `clockless_transcode` の開始位置 t の最初の segment が media の t から始まる。
- O=0 の既存 fixture のテストが変わらず通る。

## 7. 文書

- `docs/video-architecture.md` に「時刻の原点 (メディア時刻)」の節を足し、契約と `MediaTimeline` の所有を書く。
  既存の「`info.duration_secs` はコンテナ尺」等の記述と矛盾しないか確かめる。
- `docs/web-remote-video-streaming-plan.md` の `source_origin_secs` の説明をメディア時刻に揃える。
- 公開文書 (マニュアル・製品ページ) は、時刻の表示が正しくなるだけなので変えない。更新履歴は公開作業で書く。

## 8. まれな失敗

O を読めないファイルは O=0 として今と同じに扱う (ログに出す)。それ以外の回復処理は足さない
(まれな失敗は割り切り)。
