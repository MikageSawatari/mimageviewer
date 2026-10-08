# 複数音声トラックのテスト素材

`scripts/ui-smoke/generate_audio_tracks_fixture.py` が、システムの `ffmpeg` と lavfi の合成映像・音声だけから生成した素材です。私有素材は含みません。トラック列挙、切り替え、音量正規化テストに使います。基本の映像付き素材は 160×90、10 fps、6 秒の MPEG-4 映像です。範囲テスト用の長尺素材は下記の寸法です。`audio-only.flac` と `multi-audio.m4a` は音声のみです。

| ファイル | 音声の順 | 周波数 | channels | sample rate | time base | codec | language | title | default | 音量設定 |
| --- | ---: | ---: | ---: | ---: | --- | --- | --- | --- | --- | --- |
| `multi.mkv` | 1 | 440 Hz | 2 | 48000 Hz | 1/1000 | aac | jpn | 日本語 440Hz | いいえ | sine に +6 dB |
| `multi.mkv` | 2 | 880 Hz | 6 | 44100 Hz | 1/1000 | ac3 | eng | English 880Hz | はい | sine に 0 dB |
| `multi.mkv` | 3 | 1320 Hz | 1 | 32000 Hz | 1/1000 | flac | なし | なし | いいえ | sine に -6 dB |
| `multi-timebase.mp4` | 1 | 440 Hz | 2 | 48000 Hz | 1/48000 | aac | jpn | なし | いいえ | sine に +6 dB |
| `multi-timebase.mp4` | 2 | 880 Hz | 6 | 44100 Hz | 1/44100 | aac | eng | なし | はい | sine に 0 dB |
| `multi-timebase.mp4` | 3 | 1320 Hz | 1 | 32000 Hz | 1/32000 | aac | なし | なし | いいえ | sine に -6 dB |
| `single.mp4` | 1 | 440 Hz | 1 | 48000 Hz | 1/48000 | aac | なし | なし | はい | sine 既定 |
| `short-audio.mp4` | 1 | 440 Hz | 1 | 48000 Hz | 1/48000 | aac | なし | なし | はい | sine 既定 |
| `silent.mp4` | なし | — | — | — | — | — | — | — | — | — |
| `audio-only.flac` | 1 | 440 Hz | 1 | 48000 Hz | 1/48000 | flac | なし | なし | いいえ | sine 既定 |
| `multi-audio.m4a` | 1 | 440 Hz | 1 | 48000 Hz | 1/48000 | aac | jpn | なし | いいえ | sine 既定 |
| `multi-audio.m4a` | 2 | 880 Hz | 1 | 44100 Hz | 1/44100 | aac | eng | なし | はい | sine 既定 |
| `range-mp4.mp4` | 1 (A) | 440 Hz | 1 | 48000 Hz | 1/48000 | aac | なし | なし | はい | sine 既定 |
| `range-mp4.mp4` | 2 (B) | 880 Hz | 1 | 44100 Hz | 1/44100 | aac | なし | なし | いいえ | sine 既定 |
| `range-mkv.mkv` | 1 (A) | 440 Hz | 1 | 48000 Hz | 1/1000 | aac | なし | なし | はい | sine 既定 |
| `range-mkv.mkv` | 2 (B) | 880 Hz | 1 | 44100 Hz | 1/1000 | flac | なし | なし | いいえ | sine 既定 |
| `range-mkv.mkv` | 3 (C) | 1320 Hz | 1 | 32000 Hz | 1/1000 | flac | なし | なし | いいえ | sine 既定 |
| `tail-audio.mkv` | 1 | 440 Hz | 1 | 48000 Hz | 1/1000 | aac | なし | なし | はい | sine 既定 |
| `tail-audio.mkv` | 2 | 880 Hz | 1 | 44100 Hz | 1/1000 | aac | なし | なし | いいえ | sine 既定 |

`sine` の既定ピークは約 -18 dBFS です。`multi.mkv` は入力に異なる gain をかけ、後続の LUFS 差のテストでも区別できるようにしています。
`multi-timebase.mp4` は同じ周波数・音量・channels / sample rate の組み合わせを AAC で作り、異なる time base 間の切り替えを検証します (267,351 bytes)。
MP4 muxer は指定した `title` を `name` タグとして格納するため、この素材の列挙上の title はありません。
`short-audio.mp4` は映像が 6 秒、音声が 1 秒です。音声 lane 切断後も映像が進み、seek できることを確認します。

`range-mp4.mp4` (120,368 bytes) と `range-mkv.mkv` (233,219 bytes) は 64×36、2 fps、20 秒の映像と長尺 A (440 Hz、約 20 秒) を持ちます。映像の長さは demux の先読み制限より長くしています。MP4 の短尺 B (880 Hz、AAC) は約 3.976〜9.999 秒で、stream の `start_time` と `duration` から両端を取得できます。MKV の短尺 B (880 Hz、FLAC) は 0〜6.000 秒で、stream metadata の `DURATION` タグから終端を取得します。MKV の追加トラック C (1320 Hz、FLAC) は 4.000〜10.000 秒ですが、開始が 0 でない `DURATION` タグは終了時刻にも長さにも解釈できるため、列挙時の `end_secs` は `None` です。各ファイルの A と B の間で保留と再選択を検証します。
`tail-audio.mkv` (220,233 bytes) は最後の映像フレームが 5.9 秒で、440 / 880 Hz の音声が約 6.82 秒まで続きます。最後のフレーム直前で切り替えても準備量が残る状態を検証します。

PowerShell でリポジトリのルートから再生成:

```powershell
python .\scripts\ui-smoke\generate_audio_tracks_fixture.py .\testdata\audio-tracks
```

`ffmpeg` と `ffprobe` は生成・検査用の PATH 上のツールで、アプリの配布物には含めません。生成済みの動画は Git で追跡し、Rust ライブラリテストは FFmpeg CLI 無しで実行できます。
