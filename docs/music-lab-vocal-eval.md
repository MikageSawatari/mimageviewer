# music_lab vocal evaluation

`tools/music_lab` has a small offline evaluator for tuning the lightweight DSP
vocal detector without embedding any external model in mIV.

## Label Format

Use a JSON file with one or more tracks:

```json
{
  "tracks": [
    {
      "path": "media/example-song.mp4",
      "vocal": [
        { "start": 18.2, "end": 42.8 },
        { "start": 58.1, "end": 86.4 }
      ],
      "ignore": [
        { "start": 42.8, "end": 45.0 }
      ]
    }
  ]
}
```

- `path` may be absolute or relative to the JSON file.
- `vocal` contains seconds where vocal activity should be considered present.
- `ignore` is optional and excludes ambiguous boundaries, spoken intros, crowd
  noise, or model-generated labels that should not count as either positive or
  negative.

## Generate Teacher Labels

Teacher labels should normally be generated from a stronger external tool rather
than authored from scratch by hand. For the first lab pass, use Demucs to create
a vocal stem and convert the stem envelope into `vocal` intervals:

```powershell
$venv = "$env:TEMP\miv_demucs_venv"
python -m venv --system-site-packages $venv
& "$venv\Scripts\python.exe" -m pip install demucs soundfile
& "$venv\Scripts\python.exe" tools/music_lab/scripts/demucs_vocal_teacher.py `
  "C:\path\to\song-or-video.mp4" `
  --out "$env:TEMP\miv_music_eval\song.demucs_teacher.json" `
  --work-dir "$env:TEMP\miv_music_eval" `
  --reuse
```

The helper intentionally runs outside the Rust application. Demucs, PyTorch,
model weights, and Python runtime dependencies are only used to generate local
teacher data and are not bundled with mIV.

`demucs_vocal_teacher.py` writes:

- `metadata.source_media`: original file used to create the teacher data.
- `metadata.evaluation_audio`: extracted WAV used for repeatable evaluation.
- `metadata.vocal_stem`: generated vocal stem.
- `tracks[0].vocal`: generated vocal intervals.
- `tracks[0].ignore`: small boundary spans excluded from scoring.

When `vocal_eval` has trouble opening a Unicode-heavy video path on Windows, run
the generator with `--track-path extracted`. The label JSON then points to the
extracted WAV while keeping the original source path in metadata:

```powershell
& "$venv\Scripts\python.exe" tools/music_lab/scripts/demucs_vocal_teacher.py `
  "C:\path\to\song-or-video.mp4" `
  --out "$env:TEMP\miv_music_eval\song.demucs_teacher.eval_wav.json" `
  --work-dir "$env:TEMP\miv_music_eval" `
  --track-path extracted `
  --reuse
```

Thresholds such as `--vocal-db` and `--ratio-db` are part of the teacher-data
conversion from vocal stem to intervals. If the generated spans are too broad or
too sparse, regenerate the JSON with adjusted thresholds and compare by ear.

## Run Evaluation

Copy `tools/music_lab/vocal_eval.example.json`, replace `path` and `vocal`
spans, then run:

```powershell
cargo run -p music_lab --bin vocal_eval -- labels/vocal.json
```

Custom thresholds:

```powershell
cargo run -p music_lab --bin vocal_eval -- labels/vocal.json --thresholds 0.15,0.2,0.25,0.3
```

The output is tab-separated:

```text
file    threshold   precision   recall   f1   tp_s   fp_s   fn_s   tn_s
```

The seconds columns are weighted by timeline bin duration, so longer errors count
more than short boundary errors.

## music_lab UI Integration

When `music_lab` opens an audio or video file, including file drag-and-drop, it
also starts the Demucs teacher helper in a background worker when the script is
available. The UI thread never waits for Demucs. While the sidecar is running,
the top bar and right details panel show `Teacher: analyzing ...`; when it
finishes, generated vocal spans are overlaid on the timeline loudness row in
cyan and listed in the right panel.

Python resolution order:

1. `MIV_MUSIC_TEACHER_PYTHON`
2. `%TEMP%\miv_demucs_venv_py313\Scripts\python.exe`
3. `python` from `PATH`

The generated JSON, extracted WAV, and vocal stem are stored under
`%TEMP%\miv_music_lab_teacher`. This is lab-only cache data and should not be
committed.

## Teacher Data Policy

External high-accuracy tools should be used to create or draft the label JSON,
including tools that are not suitable for bundling with mIV. The important
boundary is that their code, model weights, and generated runtime dependencies
are not linked into or redistributed with mIV.

Recommended workflow:

1. Generate coarse labels with an external model.
2. Check the generated spans by ear and put uncertain boundaries in `ignore`.
3. Run `vocal_eval` and record threshold metrics.
4. Tune the DSP detector.
5. Re-run the same labels to verify whether precision / recall actually moved.

This keeps model-license experimentation separate from the distributable
implementation.
