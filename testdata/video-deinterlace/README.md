# Deinterlace regression fixture

`interlaced-six.mkv` contains six synthetic 64x48 MPEG-2 frames at 5 fps,
with top-field-first interlacing and two B-frames. It has no audio. The frame
PTS values are 0.0, 0.2, 0.4, 0.6, 0.8 and 1.0 seconds. B-frames exercise the
codec EOF drain before bwdif is flushed. The decoder tests run the real demux,
codec, bwdif, frame-step selection and CPU output path without opening an app.

Generated with FFmpeg 8.1:

```powershell
ffmpeg -hide_banner -loglevel error -f lavfi -i testsrc2=size=64x48:rate=5 -frames:v 6 -an -c:v mpeg2video -g 6 -bf 2 -flags +ildct+ilme -top 1 -q:v 2 interlaced-six.mkv
```

The fixture is generated test imagery, with no external source material.
