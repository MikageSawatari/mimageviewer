# 複数音声トラックのテスト素材

`scripts/ui-smoke/generate_audio_tracks_fixture.py` が、システムの `ffmpeg` と lavfi の合成映像・音声だけから生成した素材です。私有素材は含みません。S1 のトラック列挙テストと、後続段階の切り替え・音量正規化テストに使います。各動画は 160×90、10 fps、6 秒の MPEG-4 映像です。

| ファイル | 音声の順 | 周波数 | channels | sample rate | codec | language | title | default | 音量設定 |
| --- | ---: | ---: | ---: | ---: | --- | --- | --- | --- | --- |
| `multi.mkv` | 1 | 440 Hz | 2 | 48000 Hz | aac | jpn | 日本語 440Hz | いいえ | sine に +6 dB |
| `multi.mkv` | 2 | 880 Hz | 6 | 44100 Hz | ac3 | eng | English 880Hz | はい | sine に 0 dB |
| `multi.mkv` | 3 | 1320 Hz | 1 | 32000 Hz | flac | なし | なし | いいえ | sine に -6 dB |
| `single.mp4` | 1 | 440 Hz | 1 | 48000 Hz | aac | なし | なし | はい | sine 既定 |
| `silent.mp4` | なし | — | — | — | — | — | — | — | — |

`sine` の既定ピークは約 -18 dBFS です。`multi.mkv` は入力に異なる gain をかけ、後続の LUFS 差のテストでも区別できるようにしています。

PowerShell でリポジトリのルートから再生成:

```powershell
python .\scripts\ui-smoke\generate_audio_tracks_fixture.py .\testdata\audio-tracks
```

`ffmpeg` と `ffprobe` は生成・検査用の PATH 上のツールで、アプリの配布物には含めません。生成済みの動画は Git で追跡し、Rust ライブラリテストは FFmpeg CLI 無しで実行できます。
