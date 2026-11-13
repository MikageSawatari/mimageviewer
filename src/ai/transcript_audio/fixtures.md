`delayed_tracks.mka` contains two synthetic FLAC/mono/16kHz tracks. Track 0 is
four seconds of 400Hz tone; track 1 is one second of 1000Hz tone starting at
playback time three seconds. There is no speech or private material.

Generated with FFmpeg (development CLI, not a product executable):

```powershell
ffmpeg -nostdin -hide_banner -loglevel error -f lavfi -i 'sine=frequency=400:sample_rate=16000:duration=4' -itsoffset 3 -f lavfi -i 'sine=frequency=1000:sample_rate=16000:duration=1' -map 0:a -map 1:a -c:a flac -metadata:s:a:1 title=delayed -y src/ai/transcript_audio/delayed_tracks.mka
```

The regression receives the accepted job's in-memory choice snapshot, uses the
playback track resolver, and asserts three seconds of exact digital silence
followed by one second of nonzero decoded PCM. The companion in-memory graph
test checks the exact boundary and value with constant s16 input.

`offset_tracks.mka` is a remux of the same fixture with every stream timestamp
shifted ten seconds. The container start is 10s and the second track starts at
13s. The regression compares all extracted PCM bytes with the zero-origin
fixture, including its three seconds of silence.

```powershell
ffmpeg -nostdin -hide_banner -loglevel error -i src/ai/transcript_audio/delayed_tracks.mka -map 0 -c copy -output_ts_offset 10 -y src/ai/transcript_audio/offset_tracks.mka
```
