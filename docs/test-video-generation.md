# テスト動画 (sync test) の生成方法

動画再生サブシステムの開発・回帰テスト用に、`testimage/movie/` 配下に
`test_<fps>fps_<resolution>_sync.mp4` という名前のシンセティック動画を
置いている。本ドキュメントはこれらの再生成手順をまとめたもの。

## 目的と内容

- A/V 同期 (映像と音声の頭出し精度) を目で耳で確認できる
- 高 fps (60 / 120 / 240 fps) でのデコード・呈示パイプラインを叩ける
- 解像度・コーデック設定はシンプルかつ予測可能 (Constrained Baseline H.264)

中身は以下の合成パターン:

- **映像**: FFmpeg の `testsrc2` フィルタ
  - 色バー + 斜めスイープ + 左上にタイムスタンプ `HH:MM:SS.mmm`
- **音声**: FFmpeg の `sine` フィルタ
  - 1 kHz の連続サイン波 + 1 秒ごとに 4 kHz のビープ (`beep_factor=4`)
  - mono 44.1 kHz

## 既存ファイル一覧

| ファイル | 解像度 | fps | 長さ | 動画 bitrate | サイズ | 用途 |
|---|---|---|---|---|---|---|
| `test_24fps_480p_sync.mp4` | 854×480 | 24 | 30s | ~1.0 Mbps | ~4 MB | 低解像度・低 fps |
| `test_30fps_720p_sync.mp4` | 1280×720 | 30 | 30s | ~2.0 Mbps | ~8 MB | 中解像度 |
| `test_60fps_1080p_sync.mp4` | 1920×1080 | 60 | 30s | ~4.9 Mbps | ~19 MB | 1080p60 ベースライン |
| `test_120fps_1080p_sync.mp4` | 1920×1080 | 120 | 30s | ~12.9 Mbps | ~47 MB | 高 fps デコード負荷 |
| `test_240fps_1080p_sync.mp4` | 1920×1080 | 240 | 30s | ~25.5 Mbps | ~92 MB | 超高 fps デコード負荷 |

配置先 (開発機): `h:/home/mimageviewer_old/testimage/movie/`

## 前提

- `ffmpeg.exe` (libx264 または libopenh264 を含むビルド)
  - 開発機の場合、`C:\Program Files\Steinberg\Cubase 15\Externals\FFmpeg\5.1.1\ffmpeg.exe`
    に GPL ビルドがあり、libx264 が使える
  - これが無ければ [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds/releases)
    の `ffmpeg-master-latest-win64-gpl.zip` を取得して PATH を通す

## 生成コマンド

### 既存パターン (1080p60 を再生成する場合)

```bash
FFMPEG="/c/Program Files/Steinberg/Cubase 15/Externals/FFmpeg/5.1.1/ffmpeg.exe"
"$FFMPEG" -y \
  -f lavfi -i "testsrc2=size=1920x1080:rate=60:duration=30" \
  -f lavfi -i "sine=frequency=1000:beep_factor=4:duration=30:sample_rate=44100" \
  -c:v libx264 -profile:v baseline -pix_fmt yuv420p -preset medium -crf 23 \
  -c:a aac -b:a 128k -ac 1 -ar 44100 \
  -movflags +faststart \
  "h:/home/mimageviewer_old/testimage/movie/test_60fps_1080p_sync.mp4"
```

### 高 fps 版 (1080p120 / 1080p240)

`rate=` の値だけ差し替える:

```bash
# 1080p120
"$FFMPEG" -y \
  -f lavfi -i "testsrc2=size=1920x1080:rate=120:duration=30" \
  -f lavfi -i "sine=frequency=1000:beep_factor=4:duration=30:sample_rate=44100" \
  -c:v libx264 -profile:v baseline -pix_fmt yuv420p -preset medium -crf 23 \
  -c:a aac -b:a 128k -ac 1 -ar 44100 \
  -movflags +faststart \
  "h:/home/mimageviewer_old/testimage/movie/test_120fps_1080p_sync.mp4"

# 1080p240
"$FFMPEG" -y \
  -f lavfi -i "testsrc2=size=1920x1080:rate=240:duration=30" \
  -f lavfi -i "sine=frequency=1000:beep_factor=4:duration=30:sample_rate=44100" \
  -c:v libx264 -profile:v baseline -pix_fmt yuv420p -preset medium -crf 23 \
  -c:a aac -b:a 128k -ac 1 -ar 44100 \
  -movflags +faststart \
  "h:/home/mimageviewer_old/testimage/movie/test_240fps_1080p_sync.mp4"
```

### 低解像度版 (24fps_480p / 30fps_720p)

```bash
# 480p24
"$FFMPEG" -y \
  -f lavfi -i "testsrc2=size=854x480:rate=24:duration=30" \
  -f lavfi -i "sine=frequency=1000:beep_factor=4:duration=30:sample_rate=44100" \
  -c:v libx264 -profile:v baseline -pix_fmt yuv420p -preset medium -crf 23 \
  -c:a aac -b:a 128k -ac 1 -ar 44100 \
  -movflags +faststart \
  "h:/home/mimageviewer_old/testimage/movie/test_24fps_480p_sync.mp4"

# 720p30
"$FFMPEG" -y \
  -f lavfi -i "testsrc2=size=1280x720:rate=30:duration=30" \
  -f lavfi -i "sine=frequency=1000:beep_factor=4:duration=30:sample_rate=44100" \
  -c:v libx264 -profile:v baseline -pix_fmt yuv420p -preset medium -crf 23 \
  -c:a aac -b:a 128k -ac 1 -ar 44100 \
  -movflags +faststart \
  "h:/home/mimageviewer_old/testimage/movie/test_30fps_720p_sync.mp4"
```

## 検証

生成後 `ffprobe` で仕様を確認:

```bash
ffprobe -v error -show_entries \
  stream=codec_name,width,height,r_frame_rate,nb_frames,duration,bit_rate \
  "h:/home/mimageviewer_old/testimage/movie/test_120fps_1080p_sync.mp4"
```

期待値:

- `codec_name=h264`, `width=1920`, `height=1080`
- `r_frame_rate=120/1` (240fps 版なら `240/1`)
- `nb_frames` = fps × 30 (例: 120fps なら 3600, 240fps なら 7200)
- `duration=30.000000`

## 注意点

### エンコーダの選択 (libx264 vs libopenh264)

元の 60fps / 30fps / 24fps 版は **libopenh264** で encode されており、
本ドキュメントの再生成手順は **libx264** (GPL build) を使う。

mIV の FFmpeg デコード経路からは両者とも H.264 Baseline として透過的に
扱えるので、テスト用途では差は出ない。bit-for-bit で libopenh264 にこだわる
場合は、libopenh264 入りビルドの ffmpeg を別途用意して `-c:v libopenh264`
に置き換える。

### x264 のライセンス

libx264 は GPL なので、生成された動画ファイルそのものに GPL 汚染は無いが、
**生成に使った ffmpeg バイナリを mIV の配布物に同梱してはいけない**。
本ドキュメントの手順は「開発機ローカルでテスト用素材を作るだけ」の用途。

mIV 自体に同梱する FFmpeg は LGPL build に限る (CLAUDE.md「FFmpeg LGPL DLL
管理」節を参照)。

### Constrained Baseline と x264 の `-profile:v baseline`

元の動画は H.264 Constrained Baseline (`avc1.42c02a` の `c0` constraint flags)。
x264 で `-profile:v baseline` を指定すると Baseline (Constrained Baseline の
スーパーセット) になる。デコード側からはどちらも同じく B-frame なし・
シンプルな I/P-frame 構造に見えるので、テスト動画として等価。

### 高 fps と H.264 Level

H.264 Level の上限は 1080p の場合:

- Level 4.2: 1080p60
- Level 5.1: 1080p120
- Level 5.2: 1080p240

x264 は出力サイズ・fps から自動で Level を決めるので、明示指定は不要。
ffprobe で `level=` を確認するとそれぞれ 42 / 51 / 52 が入っている
(60 / 120 / 240 fps の場合)。
