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

## Run

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

## Teacher Data Policy

External high-accuracy tools can be used to create or draft the label JSON,
including tools that are not suitable for bundling with mIV. The important
boundary is that their code, model weights, and generated runtime dependencies
are not linked into or redistributed with mIV.

Recommended workflow:

1. Create coarse labels by hand or with an external model.
2. Put uncertain boundaries in `ignore`.
3. Run `vocal_eval` and record threshold metrics.
4. Tune the DSP detector.
5. Re-run the same labels to verify whether precision / recall actually moved.

This keeps model-license experimentation separate from the distributable
implementation.
