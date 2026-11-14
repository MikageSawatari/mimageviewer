# 動画の時刻原点のテスト素材

`scripts/ui-smoke/generate_timeline_origin_fixture.py` がシステムの `ffmpeg` と lavfi の合成映像・音声だけから生成した素材です。私有素材は含みません。生成済み素材を Git で追跡するため、アプリ側のテスト実行に FFmpeg CLI は不要です。

映像は 96×64、12 fps、144 フレーム、内容の長さは 12 秒です。フレーム間隔は `1/12` 秒 (約 0.083333 秒)。MPEG-2、`-g 1 -bf 0 -flags:v +low_delay` の全フレーム intra で、すべての映像素材について 144/144 フレームが `key_frame=1`、`pict_type=I`、144/144 映像 packet が PTS=DTS であることを検査しています。通常の TS 音声は 48 kHz mono AAC、32 kbit/s です。

全 intra は時刻原点の契約を、既存の TS keyframe seek 不具合から切り離して検査するための素材側の選択です。通常 GOP の seek が直ったことを示す素材ではありません。その不具合は [次のリリースのバックログ §1.785](../../docs/next-release-backlog.md) で別に扱います。12 fps は素材を小さく保ちつつ、最終 frame と EOF の間隔を 0.1 秒未満にするためです。

MPEG-2 の sequence extension に `low_delay=1` を設定し、decoder の frame reorder delay を不要にしています。B frame がなくても通常の encoder 出力では DTS が PTS より 1 frame 早く、TS の DTS 基準 seek が低 fps の 1 frame 分先の画へ着くことがあります。素材では low delay により PTS=DTS を揃え、原点検査へこの量子化差を持ち込みません。インターレース素材では `+low_delay+ilme+ildct` を併用します。製品の seek 実装や既存 assertion の許容差はこの素材変更では変えません。

## ファイルと測定値

生成: FFmpeg `9.0.2-full_build-www.gyan.dev`。下表は PATH 上の ffprobe `N-124300-gdba0b078c8-20260502` での測定値です。別バージョンで再生成・測定すると encoder delay、推定 duration、サイズは多少変わり得ます。O は format の `start_time`、D は format の `duration` です。

| ファイル | bytes | demuxer | O (秒) | D (秒) | 用途 |
| --- | ---: | --- | ---: | ---: | --- |
| `small-origin.ts` | 99,076 | mpegts | 1.400000 | 12.021333 | muxer の自然な正の原点 |
| `large-origin.ts` | 99,076 | mpegts | 36001.378667 | 12.021333 | `-output_ts_offset 36000` |
| `wrap-origin.ts` | 99,076 | mpegts | -4.621333 | 12.021333 | `2^33/90000 - 6` 秒の offset、33 bit PTS の折り返し |
| `interlaced-origin.ts` | 101,332 | mpegts | 36001.378667 | 12.021333 | top field first のインターレース MPEG-2、bwdif 経路 |
| `late-audio-origin.ts` | 108,100 | mpegts | 36001.378667 | 12.021333 | 4 秒後から始まる 2 本目の AAC トラック |
| `rounded-origin-video-only.ts` | 83,472 | mpegts | 36001.666667 | 12.000000 | 音声なし、先頭映像より format 原点が 1/3 μs 後ろへ丸まる境界 |
| `large-origin.mpg` | 136,192 | mpeg | 35999.989978 | 12.010022 | MPEG-PS、MPEG-2 + 48 kHz mono MP2 / 64 kbit/s |
| `positive-origin.mkv` | 59,391 | matroska,webm | 10.000000 | 22.000000 | 原点を引かない対照、FLAC 音声、チャプター付き |
| `encoder-delay.mp3` | 48,332 | mp3 | 0.023021 | 12.000000 | 原点を引かない対照、48 kHz mono MP3 / 32 kbit/s、Xing header |

TS の映像・音声 time base は `1/90000`。音声付き TS の先頭映像は O + 0.021333 秒、最終映像は O + 11.938000 秒です。先頭音声 packet は O で、AAC priming による約 21 ms の区間の後に内容が始まります。`wrap-origin.ts` は raw の 33 bit 時刻が途中で折り返しますが、FFmpeg が unwrap した映像時刻は -4.600000〜7.316667 秒の単調な列になります。メディア時刻へ直すと他の音声付き TS と同じです。

| 映像素材 | 最初の raw PTS 秒 | 最後の raw PTS 秒 | frame 数 / keyframe 数 |
| --- | ---: | ---: | ---: |
| `small-origin.ts` | 1.421333 | 13.338000 | 144 / 144 |
| `large-origin.ts` | 36001.400000 | 36013.316667 | 144 / 144 |
| `wrap-origin.ts` | -4.600000 | 7.316667 | 144 / 144 |
| `interlaced-origin.ts` | 36001.400000 | 36013.316667 | 144 / 144 |
| `late-audio-origin.ts` | 36001.400000 | 36013.316667 | 144 / 144 |
| `rounded-origin-video-only.ts` | 36001.666667 | 36013.583333 | 144 / 144 |
| `large-origin.mpg` | 36000.000000 | 36011.916667 | 144 / 144 |
| `positive-origin.mkv` | 10.000000 | 21.917000 | 144 / 144 |

`interlaced-origin.ts` は 144/144 映像フレームの `interlaced_frame=1`、`top_field_first=1` と stream の `field_order=tt` を確認済みです。単色なので両 field の色は同じです。他の映像素材は progressive です。

同梱 `avformat-61.dll` の `av_seek_frame(-1, target, AVSEEK_FLAG_BACKWARD)` でも、large TS と interlaced TS のメディア時刻 11.521333 秒への seek 後、最初の映像 packet が PTS=DTS=11.521333 秒になることを確認しています。0.5 秒への backward seek では 0.438 秒から読み始め、次の 0.521333 秒の frame へ進めます。

`late-audio-origin.ts` の第 1 音声は `jpn`、第 2 音声は `eng`。第 2 音声の raw start は 36005.378667 秒、メディア時刻の start は 4 秒です。内容は 6 秒で、約 10 秒に終わり、12 秒の映像はその後も続きます。ffprobe が TS から推定する第 2 音声 stream duration は 5.824000 秒です。実際の decode sample 数や厳密な終端とこの推定値は区別してください。

第 1 音声の start は `3240124080/90000 = 36001.3786666667` 秒、stream duration は `1079040/90000 = 11.9893333333` 秒です。format 原点 `36001.378667` 秒との差は約 −1/3 μs で、メディア時刻の start は丸め誤差だけを補正して 0、end は変換した start と stream duration から `1079040/90000` 秒になります。end の計算を start の非負判定に依存させず、start が負という理由で start/end 両方を不明にしないことを、decoder の metadata と availability 判定で検査します。

`rounded-origin-video-only.ts` は `-output_ts_offset 36000.266667` で作った音声なし素材です。先頭 raw PTS は `3240150000/90000 = 36001.6666666667` 秒で、format 原点 `36001.666667` 秒を引くと約 −1/3 μs になります。AV_TIME_BASE の丸め差だけを 0 へ補正し、実際の負の preroll は保持します。全 144 映像 packet が PTS=DTS、全 144 frame が intra です。回帰テストは実際に読んだ key packet の raw DTS・PTS・byte位置を記録し、テスト専用の Input に索引 entry を追加して worker へ渡します。indexed 経路の最初のセルが失敗せず赤い画素を返すことを検査します。TS の全 packet 読取だけでは索引は揃わないため、この準備はテスト内に限定し、製品へ索引走査や seek の変更は加えません。これだけで cold probe の索引 coverage 判定や通常 GOP seek の修正を示すものではありません。

PS は小さな PES packet (`-packetsize 128`) で duration の推定を安定させています。先頭映像は 36000.000000 秒、メディア時刻 0.010022 秒です。今回の全 intra 素材では最終フレームを含む 144/144 frame に `best_effort_timestamp_time` があり、最終メディア時刻は約 11.926689 秒です。

MKV の映像・音声 time base は `1/1000`、先頭映像は 10 秒、最終映像は 21.917 秒です。PTS は 1 ms 単位に丸められるので frame 間隔は 0.083/0.084 秒になります。チャプターは 10〜14、14〜18、18〜22 秒。O を引かないため D=22 とこれらの時刻がそのまま保たれます。MP3 も O を引かず、encoder delay / skip sample の従来の扱いを保ちます。

## 画素と音声の期待値

内容時刻 u の `[n, n+1)` 秒ごとの単色は次のとおりです。音声付き TS のメディア時刻では u = t - 0.021333、音声なし TS では u ≈ t、PS では u = t - 0.010022、MKV では u = t - 10 です。色変化の境界から離れた `u=n+0.5` を検査します。`D-0.5` の色は全映像素材で最後の灰色です。

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
