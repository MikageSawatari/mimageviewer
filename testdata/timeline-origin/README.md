# 動画の時刻原点のテスト素材

`scripts/ui-smoke/generate_timeline_origin_fixture.py` がシステムの `ffmpeg` と lavfi の合成映像・音声だけから生成した素材です。私有素材は含みません。生成済み素材を Git で追跡するため、アプリ側のテスト実行に FFmpeg CLI は不要です。

映像は 96×64、25 fps、300 フレーム、内容の長さは 12 秒です。MPEG-2、B フレームなし、GOP は最大 25 フレームです。通常の TS 音声は 48 kHz mono AAC、32 kbit/s です。

## ファイルと測定値

生成: FFmpeg `9.0.2-full_build-www.gyan.dev`。下表は PATH 上の ffprobe `N-124300-gdba0b078c8-20260502` での測定値です。別バージョンで再生成・測定すると encoder delay、推定 duration、サイズは多少変わり得ます。O は format の `start_time`、D は format の `duration` です。

| ファイル | bytes | demuxer | O (秒) | D (秒) | 用途 |
| --- | ---: | --- | ---: | ---: | --- |
| `small-origin.ts` | 119,004 | mpegts | 1.418667 | 12.021333 | muxer の自然な正の原点 |
| `large-origin.ts` | 119,004 | mpegts | 36001.378667 | 12.021333 | `-output_ts_offset 36000` |
| `wrap-origin.ts` | 119,004 | mpegts | -4.621333 | 12.021333 | `2^33/90000 - 6` 秒の offset、33 bit PTS の折り返し |
| `interlaced-origin.ts` | 119,004 | mpegts | 36001.378667 | 12.021333 | top field first のインターレース MPEG-2、bwdif 経路 |
| `late-audio-origin.ts` | 128,028 | mpegts | 36001.378667 | 12.021333 | 4 秒後から始まる 2 本目の AAC トラック |
| `large-origin.mpg` | 127,488 | mpeg | 35999.989978 | 12.000000 | MPEG-PS、MPEG-2 + 48 kHz mono MP2 / 64 kbit/s |
| `positive-origin.mkv` | 48,593 | matroska,webm | 10.000000 | 22.000000 | 原点を引かない対照、FLAC 音声、チャプター付き |
| `encoder-delay.mp3` | 48,332 | mp3 | 0.023021 | 12.000000 | 原点を引かない対照、48 kHz mono MP3 / 32 kbit/s、Xing header |

TS の映像・音声 time base は `1/90000`。先頭映像は O + 0.021333 秒、最終映像は O + 11.981333 秒です。先頭音声 packet は O で、AAC priming による約 21 ms の区間の後に内容が始まります。`wrap-origin.ts` は raw の 33 bit 時刻が途中で折り返しますが、FFmpeg が unwrap した映像時刻は -4.600000〜7.360000 秒の単調な列になります。メディア時刻へ直すと他の TS と同じです。

`interlaced-origin.ts` は全映像フレームの `interlaced_frame=1`、`top_field_first=1` と stream の `field_order=tt` を確認済みです。単色なので両 field の色は同じです。

`late-audio-origin.ts` の第 1 音声は `jpn`、第 2 音声は `eng`。第 2 音声の raw start は 36005.378667 秒、メディア時刻の start は 4 秒です。内容は 6 秒で、約 10 秒に終わり、12 秒の映像はその後も続きます。ffprobe が TS から推定する第 2 音声 stream duration は 5.824000 秒です。実際の decode sample 数や厳密な終端とこの推定値は区別してください。

PS は小さな PES packet (`-packetsize 128`) で duration の推定を安定させています。先頭映像は 36000.000000 秒、メディア時刻 0.010022 秒です。PS はすべての映像 packet に PTS を持つ形式ではなく、ffprobe の最終 decoded frame では `best_effort_timestamp_time` が欠けます。厳密な最終フレーム時刻の検査には TS を使い、PS は追加の原点・seek・画素検査に使います。

MKV の映像・音声 time base は `1/1000`、先頭映像は 10 秒、最終映像は 21.96 秒です。チャプターは 10〜14、14〜18、18〜22 秒。O を引かないため D=22 とこれらの時刻がそのまま保たれます。MP3 も O を引かず、encoder delay / skip sample の従来の扱いを保ちます。

## 画素と音声の期待値

内容時刻 u の `[n, n+1)` 秒ごとの単色は次のとおりです。TS のメディア時刻では u = t - 0.021333、PS では u = t - 0.010022、MKV では u = t - 10 です。色変化の境界から離れた `u=n+0.5` を検査します。`D-0.5` の色は全映像素材で最後の灰色です。

| n | 入力 RGB (hex) | decode 後の中心画素 RGB (測定値) |
| ---: | --- | --- |
| 0 | ff0000 | (253, 0, 0) |
| 1 | 00ff00 | (0, 254, 0) |
| 2 | 0000ff | (0, 0, 254) |
| 3 | ffff00 | (253, 253, 0) |
| 4 | ff00ff | (254, 0, 253) |
| 5 | 00ffff | (0, 255, 255) |
| 6 | ffffff | (255, 255, 255) |
| 7 | 000000 | (0, 0, 0) |
| 8 | ff8000 | (254, 127, 0) |
| 9 | 8000ff | (127, 0, 254) |
| 10 | 008080 | (0, 126, 127) |
| 11 | 808080 | (128, 128, 128) |

YUV 変換・decoder の差を許すため、入力 RGB の各成分に少なくとも ±3 の許容差を取ります。時刻の返り値だけでなく画素を判定してください。

通常音声の入力は u=2〜2.1、6〜6.1、10〜10.1 秒の 1 kHz sine、ピーク 0.5、それ以外は無音です。`AudioRangeDecoder` の検査ではパルス中央と離れた無音区間を比較できます。TS のメディア時刻では約 2.021〜2.121、6.021〜6.121、10.021〜10.121 秒、PS では約 2.010〜2.110、6.010〜6.110、10.010〜10.110 秒、MKV では 12〜12.1、16〜16.1、20〜20.1 秒です。MP3 は skip sample 適用済みの CLI decode で 2〜2.1、6〜6.1、10〜10.1 秒にパルスを確認しています。lossy codec は端で微小な ringing があるため、端の完全な無音や一試料単位の一致は要求しません。

第 2 音声の入力パルスはローカル時刻 0.5〜0.6、2.5〜2.6、4.5〜4.6 秒、メディア時刻では約 4.521〜4.621、6.521〜6.621、8.521〜8.621 秒です。

## 再生成と検査

PowerShell でリポジトリのルートから再生成します。出力先は省略可能です。チャプター metadata は stdin で渡し、出力先へ一時素材を残しません。

```powershell
python .\scripts\ui-smoke\generate_timeline_origin_fixture.py .\testdata\timeline-origin
ffprobe -v error -show_format -show_streams -show_chapters -of json .\testdata\timeline-origin\positive-origin.mkv
ffprobe -v error -select_streams v:0 -show_frames -show_entries frame=best_effort_timestamp_time,interlaced_frame,top_field_first -of json .\testdata\timeline-origin\interlaced-origin.ts
```

`ffmpeg` と `ffprobe` は PATH 上の生成・検査用ツールで、アプリの配布物には含めません。生成と上記の非対話検査では mImageViewer を起動しません。
