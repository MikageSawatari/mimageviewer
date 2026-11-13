# 文字起こしの時刻変換用 TS fixture

`fixtures/origin-video.ts` は人工の映像・正弦波だけから作った MPEG-TS です。
4 秒、64×64、5 fps の MPEG-2 映像と 48 kHz の AAC 音声を含みます。
T1 区切り 3 の調査で作成した `target/asr-t1-chunk3/origin-video.ts` を、そのままコピーしました。

生成コマンドは次のとおりです。既存ファイルの上書きを防ぐため `-n` を使います。

```powershell
ffmpeg -hide_banner -loglevel error -n -f lavfi -i testsrc2=size=64x64:rate=5:duration=4 -f lavfi -i sine=duration=4:sample_rate=48000 -c:v mpeg2video -threads:v 1 -c:a aac -f mpegts src/video/transcript_time/fixtures/origin-video.ts
```

- サイズ: 96,820 bytes
- SHA-256: `214e5226cec7878e74eefe5f57b20e58954cf9cd8376d210b3b0cac8efdfe10e`

同梱 FFmpeg `n7.1.5-16-g9a4bb2c579-20260816` の調査ログでは、コンテナ原点は
`1.578667` 秒、duration は `4.021333` 秒でした。
`transcript_nonzero_ts_origin_uses_existing_player_seek_clamp_without_bypass` は
固定値を VideoInfo に注入せず、同梱 decoder の通常の metadata 読取りを使います。

保存時刻 3 秒は raw PTS の `4.578667` 秒へ正しく変換されます。
ただし既存の `VideoPlayer::seek` は `[0, duration - 0.1]` に制限するため、
発行 target は `3.921333` 秒、逆変換した保存時刻は `2.342666` 秒になります。
これは既存のシークバー・ブックマーク・チャプターにも共通する制約で、T1 では変更しません。
テストは専用 clamp、duration への原点加算、既存 clamp の迂回を行いません。

この回帰は実ファイルの metadata と非対話の player transport を検査するものです。
製品アプリの表示・実機の seek 到着時刻を検証した証拠にはしません。
