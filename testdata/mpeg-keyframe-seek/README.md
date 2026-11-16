# MPEG-TS / PS の keyframe seek 回帰素材

`scripts/ui-smoke/generate_mpeg_keyframe_seek_fixture.py` がシステムの FFmpeg と
lavfi の合成映像・音声だけから生成した素材です。私有素材は含みません。
生成済みの映像と独立正解 JSON を追跡するため、Rust の回帰試験に FFmpeg CLI は不要です。
製品アプリは生成・検査時に起動しません。

生成と全復号・packet 検査は同じ配布物の FFmpeg / ffprobe
`9.0.2-full_build-www.gyan.dev` を使用しました。PATH の ffprobe が別版のことがあるため、
生成スクリプトは ffmpeg と同じディレクトリの ffprobe を優先します。
PS の実 seek 窓、長 GOP の back、PTS 反例の窓は、これらとは別に同梱の
`avformat-61.dll` / `avcodec-61.dll` / `avutil-59.dll` を ctypes から直接呼んで検査しています。
この probe は製品の seek helper を呼びません。

## 素材

すべて 96×64、YUV420p、B frame ありです。通常は 12 秒・30 fps・GOP 15 frame
(0.5 秒)、音声は 48 kHz mono の連続 1 kHz sine です。AAC は 32 kbit/s、MP2 は
64 kbit/s。JSON は各素材の**拡張子を残した名前に `.json` を加えたファイル**です。

| 素材 | bytes | 全復号 frame / 全映像 packet | 用途 |
| --- | ---: | ---: | --- |
| `h264-small.ts` | 175,028 | 360 / 360 | H.264 + AAC、自然な小さい原点、IDR |
| `mpeg2-large.ts` | 253,800 | 360 / 360 | MPEG-2 + MP2、open GOP、`-output_ts_offset 36000` |
| `hevc-open.ts` | 234,060 | 360 / 360 | HEVC + AAC、open GOP、CRA 23 個と RASL 46 個 |
| `mpeg2-missing-pts.mpg` | 176,000 | 360 / 360 | MPEG-2 + MP2、PES 128 bytes、全 packet 中に PTS 不明の key 4 個 |
| `mid-gop.ts` | 172,772 | 345 / 357 | `h264-small.ts` の先頭 2,256 bytes を切断、最初の映像 packet は非 key |
| `mid-gop.mpg` | 173,184 | 345 / 353 | PS の先頭 2,816 bytes を pack 境界で切断、最初の映像 packet は非 key |
| `long-gop.ts` | 362,464 | 1,080 / 1,080 | 18 秒・60 fps・GOP 360 frame (6 秒)、H.264 + AAC |
| `pts-delayed.ts` | 341,408 | 360 / 360 | 大原点 MPEG-2 TS の映像 PTS だけを `setts=pts=PTS+2/TB` で 2 秒遅延 |
| `short-key-gap.ts` | 253,988 | 360 / 360 | MPEG-2 TS、key の ID 180→183 の一か所だけ間隔が 3 frame |
| `control.mp4` | 75,497 | 360 / 360 | `h264-small.ts` と同じ番号映像・encoder 設定、MP4 の対照 |

通常素材の frame ID は 0〜359、途中開始の 2 本は 15〜359、長 GOP は 0〜1,079。
全素材の全 frame で ID が 1 ずつ増え、全復号の `best_effort_timestamp` が存在して
厳密に増加することを生成時に検査します。PS は大きな PES にすると最終 frame の時刻が
欠けたり、parser の時刻割当てが一部逆行したりするため、上の条件と後述の B が両立する
128 bytes を使っています。`GENPTS` は正解の生成に使いません。

## 画素から frame ID を読む

上半分 (`y < 32`) に 16 bit の little-endian の番号を置きます。bit b は
`x = 6*b .. 6*b+5` の縦帯で、入力 Y は 0 が 32、1 が 224、Cb/Cr は 128。
各 bit の **`x = 4+6*b, y = 16`** を読み、gray / Y / RGB の R 成分が **128 以上なら 1**
として `sum(bit << b)` を求めます。下半分は左が暗・右が明の固定参照です。

```text
id = 0
for b in 0..16:
    if pixel(4 + 6*b, 16).red >= 128:
        id |= 1 << b
```

リサイズした画では元の座標を出力寸法の比率で変換してください。生成時の全 gray 復号で
閾値からの最小距離は 102〜108 で、少なくとも 64 あることを検査しています。
ID を復号する関数は生成スクリプト内の `frame_id` です。

## JSON の正解と時刻

`frames` は **先頭から全復号した表示順**です。ffprobe の `-show_frames` の結果と、
ffmpeg の `-fps_mode passthrough -pix_fmt gray -f rawvideo` による全復号画像を、
frame 数と並びで照合して各 ID を読みます。seek 結果や製品と同じ時刻推定は正解に使いません。
`packets` は先頭からの **全映像 packet の demux 順**で、ffprobe の `-show_packets
-show_data_hash sha256` から作ります。音声 packet はこの配列に含めません。

`schema_version = 1` の主なフィールド:

| フィールド | 意味 |
| --- | --- |
| `file`, `sha256`, `width`, `height` | 素材の識別・全ファイル SHA-256・寸法 |
| `generator_tools` | 全復号・全 packet 正解を作った CLI の版 |
| `frame_id_encoding` | bit 数・順序・画素座標・閾値 |
| `format.name` | 実 demuxer 名 (拡張子から判定しない) |
| `format.start_time_us`, `duration_us` | ffprobe format の原点・duration、整数 μs |
| `format.container_start_ticks`, `duration_ticks` | 上の値を映像 time base へ丸めた参考値 |
| `video_stream` | `index`, `codec_name`, `time_base`, `start_pts`, `duration_ts`, `avg_frame_rate` |
| `audio_streams` | 各音声の `index`, `codec_name`, `time_base`, `start_pts`, `duration_ts`, `sample_rate` |
| `frames[]` | `display_index`, `frame_id`, `best_effort_timestamp`, `pts`, `pkt_dts`, `key_frame`, `pict_type` |
| `packets[]` | `packet_index`, `key`, `data_sha256`, `pts`, `dts`, `pos`, `size`, `duration` |
| `validation` | 全復号数・packet 数・B 数・PTS 不明 key 数・画素余裕・open GOP / CRA / RASL / 切断位置の検査 |
| `special_targets` | 次節の境界・反例の raw target と独立 probe 結果 |

時刻は μs と明記した項目以外、**映像 stream の raw tick** (JSON 整数) です。
不明な `pts` / `pkt_dts` / `dts` 等は `null`。全 frame の `best_effort_timestamp` は既知。
`time_base` は `"1/90000"` などの有理数文字列です。MP4 は `1/15360`。
`pos = -1` は有効な「byte 位置不明」で、frame の識別に使いません。
format 原点は μs に丸まるため、tick に換算する参考値には 1 tick 未満の丸めがあり得ます。
比較は raw tick を基本とし、製品のメディア時刻への変換には既存の `MediaTimeline` 契約を使います。

## 実際に通る窓と反例

PS の `special_targets.missing_pts_b` は fresh Input で実際に seek して検査した窓です。
target `579000`、back 1 秒、seek `489000`。J の key は **PTS がすべて null** で、
DTS は `498000`, `534000`, `579000` の 3 個。A は false、B は true です。
この packet 列を clone してそのまま独立 decoder へ渡すと、最初の I の画像は **ID 150**、
表示時刻は **498000**。全ファイルの packet payload SHA-256 で最初の key の位置を照合し、
先頭からの全復号の同じ I 画像・表示時刻と一致すること、時刻が target 以下であることも検査します。
PS では seek 後に時刻の割当てが変わるため、全 packet 表から窓の PTS を推測していません。

長 GOP の `special_targets.preroll_over_240` は ID 690、raw target `1164000`。
直前の key は ID 360 で差は 330 frame。実 DLL の back 1 / 2 / 4 秒では J が空、
8 秒で初めて手前の key が入り、3 回の倍増を要することを記録しています。

PTS 反例の `special_targets.known_pts_after_target` は raw target `3240567000`、back 1 秒。
実 DLL の窓には key が 3 個ありますが、PTS はすべて target より後です。
key の個数だけでは成功と判定できない例です。

短い間隔の `special_targets.window_end_key_is_not_j` は raw target `668999`。
続く key の DTS は `669000` なので、窓の終わりとして保持しても J に数えません。
この例の end key と一つ前の key の表示順 ID は 183 / 180 です。

## 再生成と検査

Windows x64、FFmpeg CLI の libx264 / libx265 / mpeg2video と同梱 FFmpeg DLL が必要です。
出力先は省略可能で、既存の README は書き換えません。

```powershell
python .\scripts\ui-smoke\generate_mpeg_keyframe_seek_fixture.py .\testdata\mpeg-keyframe-seek
```

CLI を選ぶ場合は `--ffmpeg <path> --ffprobe <path>`、同梱 DLL の場所は
`--bundled-dll-dir <directory>` を渡せます。生成条件が変わって B の実窓・全 frame の時刻・
open GOP・CRA/RASL・番号画素のいずれかを満たさなくなった場合は生成を失敗させ、
条件を緩めて正解 JSON を作りません。encoder や FFmpeg 版を替えて再生成したときは、
素材の bytes・probe の具体値・この README の測定値も見直してください。
