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

## Generate Labels

Labels may be drafted with stronger external tools, including tools that are too
large or too license-sensitive to ship with mIV. The lab app no longer starts a
heavy model sidecar by itself; generate or edit the JSON outside the Rust app,
check the spans by ear, and pass the finished file to `vocal_eval`.

If an external tool has trouble with a Unicode-heavy video path on Windows,
extract a temporary WAV and point `tracks[].path` at that WAV. Keep the original
source path in your own notes or optional metadata if needed.

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

Dump predicted / teacher spans for boundary inspection:

```powershell
cargo run -p music_lab --bin vocal_eval -- labels/vocal.json --thresholds 0.03 --dump-segments 0.03
```

The output is tab-separated:

```text
file    threshold   precision   recall   f1   tp_s   fp_s   fn_s   tn_s
```

The seconds columns are weighted by timeline bin duration, so longer errors count
more than short boundary errors.

For the lightweight DSP detector, do not tune against teacher data as if it were
perfect ground truth. Strong external labels are useful, but they can include
vocal-like effects, reverb tails, or extra sections. The target for the built-in
DSP is a plausible, responsive hint: keep false positives low enough that
instrumental gaps remain readable, and accept that strongly processed vocals or
rap-like delivery may be missed.

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
