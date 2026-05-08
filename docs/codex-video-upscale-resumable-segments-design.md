# Codex Design: Resumable Offline Video Upscale

Date: 2026-05-02
Reviewer target: ClaudeCode

## Goal

Offline AI video upscale jobs can take hours or days. The current single-output
`.miv.mkv.part` export loses all progress if the user cancels, the app exits, or
Windows restarts. This design changes export into resumable segment work while
keeping the final user-visible output as:

- `<stem>.miv.mkv`
- `<stem>.miv.json`

The Phase E UX is a persistent background task queue:

- `この動画をアップスケール登録`
- `この動画のアップスケールを削除`
- separator
- `アップスケールタスク表示`

Registration opens a small options dialog, then queues the task and shows a toast. Progress is
checked in the task window. The release UI intentionally keeps the feature conservative: it uses
the fast general-purpose upscale model only, keeps segment execution serial, and exposes only
scale and compression presets. Segment files are still used for pause/resume durability.

## Why Segment-Based Resume

Do not append to a partially written AV1/MKV file. AV1 GOP structure, muxer indexes,
cluster metadata, timestamps, and audio interleaving make append-style resume brittle.

Instead:

1. Encode short independent video-only segments.
2. Mark a segment complete only after its file is closed and validated.
3. On resume, skip completed segments and continue from the next source position.
4. At finalization, concatenate video segments into a final temporary file.
5. Mux/copy source audio into the final output when audio support is enabled.
6. Write the normal `.miv.json` sidecar and atomically publish `<stem>.miv.mkv`.

This makes each completed segment durable. A crash loses only the currently running
segment.

## Storage Layout

For source `movie.mp4` in a normal folder:

```text
movie.mp4
movie.miv.work/
  job.miv-upscale.json
  segments/
    000000.mkv
    000001.mkv
    000002.mkv
  final.mkv.part
movie.miv.mkv
movie.miv.json
```

Rules:

- Work data is stored next to the source because segments can be large.
- The work directory name is deterministic: `<stem>.miv.work`.
- After a successful publish, `<stem>.miv.work/` is deleted automatically. It is kept for
  incomplete, failed, or paused tasks so they can resume or be inspected.
- A segment file is written as `000123.mkv.part`, then renamed to `000123.mkv`.
- The final output is still written through `<stem>.miv.mkv.part`, then renamed.
- `final.mkv.part` inside the work directory may be used as an internal temporary during
  finalization. The preferred Phase D path writes directly to `<stem>.miv.mkv.part`.
- `この動画のアップスケールを削除` removes `<stem>.miv.mkv`, `<stem>.miv.json`, and
  `<stem>.miv.work/` after confirmation.

## Persistent Queue

Add a small appdata JSON queue file:

```text
%APPDATA%/mimageviewer/video_upscale_tasks.json
```

The queue file is an index, not the source of truth for completed segment data.
Each task points at a work manifest:

```json
{
  "schema": 1,
  "paused": false,
  "tasks": [
    {
      "task_id": "20260502-123456-abcdef",
      "source_path": "E:/videos/movie.mp4",
      "manifest_path": "E:/videos/movie.miv.work/job.miv-upscale.json",
      "state": "queued",
      "failure_reason": null,
      "created_unix_ms": 1777721696000,
      "updated_unix_ms": 1777721696000
    }
  ]
}
```

Task states:

- `queued`
- `planning`
- `running`
- `paused`
- `canceling`
- `failed`
- `done`

Failures should use `state: "failed"` plus a separate `failure_reason` string instead of adding
many top-level states. Initial reasons:

- `schema_mismatch`
- `stale_source`
- `audio_mux`
- `no_space`
- `plan_drift`
- `io`

Queue writes must be atomic (`.tmp` + rename). UI reads and writes happen on the UI
thread only for the small queue JSON. Segment and manifest file I/O runs in workers.

`task_id` must be collision-resistant. Use UUID v4 or at least 8 random bytes encoded
as hex; do not rely on a timestamp suffix alone.

Only one mIV instance may own the task queue at a time. Use a queue lock in the same
appdata directory or a Windows global mutex. If the lock cannot be acquired, show the
task UI read-only and do not start workers. This avoids two app instances writing the
same queue file or the same source-side work directory.

If a future manifest/queue schema version is not supported, mark the task as failed with
`failure_reason: "schema_mismatch"` and ask the user to register it again. Do not attempt
automatic migration in MVP.

The queue stores absolute `source_path` and `manifest_path`. Moving the source folder while a task
exists is not supported in MVP. On startup, if `manifest_path` no longer exists, the task should be
marked failed with `failure_reason: "stale_source"` and the user should re-register it from the new
folder.

## Job Manifest

The work manifest is authoritative for resume:

```json
{
  "schema": 1,
  "task_id": "20260502-123456-abcdef",
  "source": {
    "file_name": "movie.mp4",
    "size": 123456789,
    "mtime_unix_ms": 1777561200000,
    "head_tail_sha256": "...",
    "time_base": [1, 24000]
  },
  "output": {
    "final_path": "movie.miv.mkv",
    "sidecar_path": "movie.miv.json",
    "width": 2560,
    "height": 1440
  },
  "options": {
    "scale": 4,
    "model": "realesrgan_anime6b",
    "quality_level": 5,
    "container": "mkv",
    "video_codec": "av1",
    "encoder": "libsvtav1"
  },
  "progress": {
    "estimated_frames": 2156,
    "completed_frames": 480,
    "next_output_frame_index": 480
  },
  "segments": [
    {
      "index": 0,
      "path": "segments/000000.mkv",
      "state": "done",
      "output_frame_start": 0,
      "output_frame_count": 120,
      "output_total_pts_ticks": 120120,
      "output_time_base": [1, 24000],
      "source_start_pts": 0,
      "source_last_pts": 119119,
      "size": 12345678,
      "mtime_unix_ms": 1777721700000
    }
  ],
  "updated_unix_ms": 1777721700000
}
```

Notes:

- `source` uses the existing v1 sidecar identity fields and must be validated before resume.
- `mtime_unix_ms` is informational and not used for source identity.
- `source.time_base` belongs to the source stream and is stored once, not per segment.
- `source_last_pts` lets resume seek slightly before the next frame and drop already encoded
  frames, avoiding duplicates after imprecise FFmpeg seeks.
- Segment files are considered reusable only when manifest says `done` and the file exists
  with matching `size`.
- `output_total_pts_ticks` is the segment's encoded video duration in `output_time_base` ticks.
  Final concat uses it to build cumulative packet timestamp offsets. This is required for VFR
  and avoids assuming that `output_frame_count * constant_frame_duration` is always correct.
- Manifest paths such as `output.final_path`, `output.sidecar_path`, and `segments[].path`
  should be relative to the source folder or work directory as documented by each field. Avoid
  absolute paths inside the work manifest unless a field explicitly represents the original
  source path.

## Segment Sizing

Use an explicit segment plan created before the first segment worker starts. The plan should
prefer source keyframe/GOP boundaries when they are cheaply available, then fall back to
time/frame-based boundaries.

Why:

- FFmpeg seeks are usually keyframe-oriented.
- Parallel segment workers must be able to start independently without decoding from the
  beginning.
- Aligning segment starts to source keyframes reduces duplicated decode/drop work.
- Output segments are re-encoded, so each segment will start with a fresh encoder keyframe
  regardless of the source. Source keyframes are used for efficient input seeking, not because
  the output depends on the original GOP layout.

Planning algorithm:

1. Probe the source stream and collect video key packet PTS/DTS positions when possible.
2. Build target segment boundaries at about 5 seconds.
3. Snap each boundary to the nearest usable source keyframe near the target time.
4. Keep boundaries monotonic and enforce minimum/maximum segment frame counts.
5. If keyframe scanning fails or produces too sparse a plan, fall back to frame/time boundaries
   plus seek preroll.

The keyframe scan should read packets only and must not decode frames. It can run in the worker
after the task is registered. For short files it is cheap; for very large/network files the UI
should show `セグメント計画中...`.

Planning is a cancelable worker state. The task moves from `queued` to `planning` while probing and
keyframe scanning. If the app exits or the user cancels while planning, the manifest may contain no
plan or a partial plan; resume must treat that as incomplete and restart/continue planning before
encoding segments.

Adaptive limits are time-based, not fixed-frame-count based:

- default target: about 5 seconds of source video
- minimum: about 1 second
- maximum: about 10 seconds

The implementation can derive an approximate frame count from the probed fps only for planning
and UI estimates. For example, 30 fps targets about 150 frames and 60 fps targets about 300
frames. The exact segment ownership is still defined by the planned frame/PTS range.

For very slow high-resolution jobs, losing the current 30-120 frames on crash is acceptable.
Future versions can expose an internal/debug setting, but MVP should not expose segment size.

### Segment Plan Schema

The manifest should store planned segment ranges separately from completed segment outputs:

```json
{
  "plan": {
    "strategy": "source_keyframe_snap",
    "state": "complete",
    "scan_progress_pts": 120120,
    "segments": [
      {
        "index": 0,
        "target_start_frame": 0,
        "target_end_frame_exclusive": 120,
        "target_start_pts": 0,
        "target_end_pts": 120120,
        "seek_start_frame": 0,
        "seek_start_pts": 0
      }
    ]
  }
}
```

Definitions:

- `strategy` is one of `source_keyframe_snap`, `time_based`, or `frame_based`.
- `state` is `planning`, `complete`, or `failed`.
- `scan_progress_pts` is optional progress for long keyframe scans. If `state != "complete"`,
  workers must not encode from the plan; they must resume planning first.
- `target_*` is the exact output range this segment owns.
- `seek_start_frame` / `seek_start_pts` is where the worker seeks in the source, usually the
  nearest previous keyframe. The worker decodes and drops frames until it reaches the target
  range.
- Source PTS values use `source.time_base` from the manifest.
- Completed segment metadata still records actual `output_frame_start` and
  `output_frame_count`.

If actual encoded frame count differs from `target_end_frame_exclusive - target_start_frame`, stop
before finalization. MVP may either adjust subsequent planned ranges if the drift is mechanically
obvious, or mark the task failed with `failure_reason: "plan_drift"` and ask the user to rebuild
the plan. Silent frame skip/overlap is not allowed.

## Worker Pipeline

### New Modules

- `src/video/upscale/manifest.rs`
- `src/video/upscale/queue.rs`
- `src/video/upscale/segment.rs`
- existing `src/video/upscale/job.rs` is refactored to call segment worker APIs

### Run Flow

1. Register task from context menu.
2. Create or load `<stem>.miv.work/job.miv-upscale.json`.
3. Validate source identity and options.
4. Estimate required temporary disk space and warn if the source drive appears too small.
5. Move the task to `planning` and build or load the segment plan.
6. Pick the next planned segment whose output is not done.
7. Seek to that segment's `seek_start_pts`.
8. Decode and drop frames until the segment's target start.
9. Create a fresh encoder/muxer for this segment.
10. Encode the segment's target frame range into `segments/NNNNNN.mkv.part`.
11. Close muxer and validate the segment file.
12. Compare actual encoded frame count with the planned count, then rename `.part` to `.mkv`.
13. Atomically update manifest.
14. Repeat until all planned segments are done or pause/cancel.
15. Finalize into `<stem>.miv.mkv.part` by remuxing segment video and source audio when enabled.
16. If audio is disabled or explicitly skipped, finalize video-only.
17. Rename final `.part` to `<stem>.miv.mkv`.
18. Write `<stem>.miv.json`, delete `<stem>.miv.work/`, mark queue task `done`, and reload the
    visible folder if needed.

Each segment must use a fresh encoder context. In the current 1-pass CRF design, opening a new
libsvtav1 encoder for a segment makes the first encoded frame a keyframe. Do not use 2-pass
encoding for segment resume; 2-pass rate-control state would need to span segments and would make
independent retry/parallel execution brittle.

Segment validation for MVP:

1. file exists and `metadata().len() > 0`
2. `avformat_open_input` can open the segment

Packet/frame-count validation is useful but can be deferred. On resume, any segment marked `done`
must be rechecked for existence and matching size; if it is missing or mismatched, mark it
`pending` and re-encode it.

The encoded frame count must match the planned frame count for that segment. If it does not, mark
the task failed with `failure_reason: "plan_drift"` unless a conservative plan rewrite is
implemented. This catches source/PTS surprises before concat creates a visible gap or overlap.

### Resume Seek

Resume should not decode the whole source from the beginning.

Algorithm:

1. Pick the next planned segment that is not `done`.
2. Seek to its `seek_start_pts`, normally the nearest previous source keyframe.
3. Decode frames in presentation order.
4. Drop frames until the planned `target_start_frame` / `target_start_pts` is reached.
5. Encode only the planned target range.

For VFR files, source PTS is advisory and output frame index remains authoritative.
If FFmpeg seek cannot find the target accurately, the worker may decode forward and drop, but it
must have a limit. MVP limit: about 10 seconds of source video. If the worker would need to
decode/drop more than that, roll back one completed segment:

1. mark the previous segment `pending`
2. delete its segment file if present
3. rebuild the local resume point from that previous segment's planned `seek_start_pts`
4. report the rollback in progress/log output

Losing one completed 5-second segment is acceptable and prevents "resume" from looking hung on
files with sparse keyframes or unusual timestamps.

## Parallel Segment Workers

Parallel segment workers are possible because each planned segment owns an independent target
frame range and writes an independent segment file. This should be implemented after single-worker
resume is correct.

### Parallelism Control

Expose queue-level parallelism in the task window:

- `自動`
- `1`
- `2`
- `3`
- `4`
- `5`

Default should be `1`. Phase F may show `自動`, but MVP can either hide it or define `自動 = 1`
until there is enough benchmark data for a real heuristic. Users can raise the numeric value
when they see low GPU utilization.

The dispatcher should enforce:

- workers only run segments from the same task after the segment plan exists
- no two workers may claim the same segment
- a claimed segment is written as `NNNNNN.mkv.part.<worker_id>` and marked `running` in the
  manifest before encoding starts
- `running` segments record `worker_id`, `worker_pid`, and `worker_started_unix_ms`
- on crash/restart, `running` segments whose worker process is no longer alive are reset to
  `pending` and their `.part.<worker_id>` files are deleted
- final concat waits until all segments are `done`

### Expected Performance

Parallelism can help when the AI stage underutilizes the GPU. A single segment worker may show
low GPU utilization because it alternates CPU tile extraction, AI inference, CPU blending, and
encoding. Multiple workers can overlap those phases and feed more work to the GPU.

Risks:

- Each worker has its own FFmpeg decoder/encoder state.
- Multiple AI sessions or TensorRT worker processes increase VRAM usage.
- Multiple SVT-AV1 encoders can saturate CPU threads and erase GPU-side gains.
- HDD/network sources can get slower when several workers read the same file.
- Thermal/power limits can lower clocks when too many workers are active.

Implementation should start with `parallel_segments = 2` as an experimental option, then test
`1..5` on representative files. If `2` helps but `4/5` do not, keep the UI range but let a future
`自動` cap lower. If no speedup appears, keep the feature hidden or default to `1`.

### Worker Resource Model

Do not share mutable FFmpeg contexts between workers. Either:

1. give each segment worker its own `AiRuntime`/model session, or
2. route all AI inference through a bounded worker pool.

The first version can be simpler but must measure VRAM. Current TensorRT acceleration is routed
through a global worker pool/IPC path, so AI inference may still be serialized even when multiple
segment workers are running. In that case segment parallelism can only overlap decode, tile
preparation, blending, and encode around the AI bottleneck. DirectML/ORT sessions may have
different behavior because each `AiRuntime` can own a separate session. Benchmarks must report
which backend is active.

## Finalization

### Video Concatenation And Final Mux

MVP should use FFmpeg libraries, not shelling out to `ffmpeg.exe`.

Implementation options:

1. Preferred: open `<stem>.miv.mkv.part` once with video plus copied audio streams, then remux
   segment video packets and original audio packets into that output.
2. Fallback: first create an internal video-only temporary (`final.mkv.part` or
   `concat-video.mkv.part`) from the segments, then mux that with source audio. Use this only if
   one-pass packet interleaving is too awkward with the current wrapper.

Do not rely on external `ffmpeg.exe`.

One-pass final mux algorithm:

1. Open the output context for `<stem>.miv.mkv.part`.
2. Add one video stream from the segment stream parameters and audio streams copied from the
   original source.
3. Choose a single output video time base.
4. Prepare a packet source for segment video and one for source audio.
5. Write packets in increasing output timestamp order when practical, using `write_interleaved`.
   Avoid writing all video and then all audio if it makes the muxer buffer excessively.

Video timestamp algorithm:

1. Keep `cumulative_pts_offset = 0` in that output time base.
2. For each segment in index order:
   - open the segment with libavformat
   - rescale each packet PTS/DTS/duration from the segment stream time base into the output
     stream time base
   - add `cumulative_pts_offset` to packet PTS and DTS
   - enqueue/write the packet
   - add the manifest's `segments[].output_total_pts_ticks` to `cumulative_pts_offset`

Audio timestamp algorithm:

1. Open the original source.
2. Copy selected audio streams into the output context.
3. Rescale audio packet timestamps from source stream time base to output audio stream time base.
4. Interleave with video packets by output timestamp.

If the one-pass approach proves risky, the two-step temporary video fallback remains valid.
Document the reason if the implementation takes that fallback because it costs one extra final-size
temporary file and another remux pass.
The video timestamp algorithm is shared by both the one-pass path and the two-step fallback; the
only difference is whether audio is interleaved into the same output context immediately or muxed
in a later pass.

`output_total_pts_ticks` should be recorded after the segment is encoded, using the same output
time base that concat will use. For CFR sources it may equal frame count times frame duration; for
VFR sources it must be derived from actual encoded packet/frame timing rather than assumed from an
average fps.

### Audio

Segment files should be video-only. Audio is copied from the original source during final
muxing, not per segment. This avoids audio discontinuities and timestamp drift at segment
boundaries.

Initial implementation may keep existing audio-copy behavior if it is already stable, but the
target design is:

1. Remux segment video packets into `<stem>.miv.mkv.part`.
2. Stream-copy original source audio packets into the same output with timestamp rescale.
3. If audio mux fails, surface an error and keep resumable work data. Do not silently publish a
   video-only final unless the user explicitly chose video-only.

If audio mux fails in MVP, mark the task failed with `failure_reason: "audio_mux"` and keep the
work directory. The error dialog/task row should offer an explicit "publish video-only" action and
a cancel/close action. Only the explicit video-only action may publish a final without audio.

Subtitles/chapters/metadata remain future work.

## Disk Space Preflight

Task registration should estimate temporary storage before the worker starts. The estimate does
not need to be exact, but it must warn before a multi-hour job fills the source drive.

MVP estimate:

```text
estimated_video_bytes = output_width * output_height * estimated_bits_per_pixel_per_frame
                        * estimated_frames / 8
required_bytes = estimated_video_bytes * 1.25
```

The `1.25` factor assumes one-pass final mux and accounts for segment files plus the final `.part`.
If the implementation uses the two-step temporary video fallback, use at least `1.5` instead. A
simpler bitrate-based estimate is also acceptable if it uses duration, quality level, and output
resolution. Query free space on the output/source drive with `GetDiskFreeSpaceExW`. This is a tiny
synchronous call and is acceptable during task registration.

If available space is below the estimate, show a warning and let the user continue or cancel. If
the job later hits `ENOSPC`, mark the task failed with `failure_reason: "no_space"` and keep the
work directory so completed segments can be reused after the user frees space.

## Cancellation And Pause

There are two different operations:

- Pause: finish the current segment if possible, update manifest, stop before starting another.
- Cancel/remove: stop as soon as safe, delete the current `.part`; completed segments remain
  unless the user chooses delete/remove.

The queue-level pause flag should be checked:

- before starting a task
- before starting/continuing planning; if pause is requested during planning, finish the current
  cheap scan chunk, save `scan_progress_pts`, then stop
- before starting each segment
- after each segment manifest update

The per-task cancel flag should be checked:

- before decode packet handling
- after RGBA conversion
- before and after AI upscale
- before encoder send
- before final concat/mux

Pause semantics:

| Task state | Pause all behavior |
|---|---|
| queued | do not start new work |
| planning | save partial plan progress and stop before more scanning |
| running | finish current segment, save manifest, then stop before next segment |
| canceling | continue cancel cleanup |
| failed/done | unchanged |

Cancel completion removes the task from the queue. Phase E treats cancel as discard, so the current
task work directory is deleted after the worker reaches a safe stop point. App exit is different:
the running task is marked back to `queued`, the process exits without waiting, and the next launch
resumes from the completed segments.

No blocking wait on the UI thread. No `try_lock + sleep`.

## UI Plan

### Context Menu

For video files:

- `この動画をアップスケール登録`
  - opens an options dialog, then creates a queued task
  - if final output exists: the dialog requires overwrite to be enabled
- `この動画のアップスケールを削除`
  - deletes final output, sidecar, work directory, and queued task after confirmation
- separator
- `アップスケールタスク表示`

These commands live in the top-level `動画` menu rather than the grid context menu. Commands that
act on "this video" are disabled unless the selected grid item is a video.

### Task Window

Phase E uses a compact two-row layout per task:

- row 1: source file name, state, and action buttons
- row 2: progress bar / frames / fps when running, plus scale / quality

Controls:

- pause/resume all. Phase E pauses before starting the next task and also stops the current task
  from starting another segment after the active segment finishes.
- move task up/down when queued
- cancel running task
- remove failed/done/pending task
- open output folder for done tasks

When pause is requested while a segment is still active, the task row shows `一時停止中` and keeps
the spinner visible. Once the worker has reached the pause boundary and is idle, the row should stop
showing a transient status/spinner while keeping the resume button available.

MVP starts with a compact egui window and no drag-and-drop. Phase E shows a per-video ETA for the
currently running task from `frames_done / elapsed`; until progress has enough data, show
`計算中...` instead of a misleading zero estimate. A later phase can switch this to a moving average
over recent segments so resume does not overreact to the first few frames.

## Folder Pairing

Final pairing remains the existing `.miv.mkv` + `.miv.json` behavior. Work segments are never
shown in the grid:

- folder scan ignores `<stem>.miv.work/`
- `.miv.mkv.part` and segment `.part` files are ignored
- completed segment files live inside the work directory and are not media items
- when `<stem>.miv.mkv` and `<stem>.miv.json` both exist, folder scan hides the
  single matching original video and same-stem sidecar images with that stem. If
  multiple source videos share that stem (for example `movie.mp4` and `movie.avi`),
  scan keeps them visible rather than guessing which one the derivative belongs to.
- completed upscaled videos show a compact `UP` badge in the grid, matching the
  existing short file-type badge style.

Implementation point: the folder scan loop should skip directory entries whose names end with
`.miv.work` before creating folder grid items. The final pairing filter uses sibling file names
only, so it does not add sidecar parsing or source hashing work to the UI-thread scan path.

When a task completes in the currently visible folder, reload the folder and select the original
or derived item using the existing derived pairing logic.

## Data Integrity

- Manifest writes are atomic.
- Segment completion is two-phase: close file -> validate -> rename -> manifest update.
- Final publication is two-phase: write final `.part` -> write sidecar -> rename final.
- If finalization fails, keep work dir and mark task `failed` so the user can retry without
  redoing completed segments.
- If source identity no longer matches, mark task failed with `failure_reason: "stale_source"`;
  do not resume.
- If options differ from the existing manifest, require restart into a clean work dir.

Log these events to `mimageviewer.log` with task id, source path, and segment index where
applicable:

- task registered/resumed/paused/canceled
- segment started/completed/failed/rolled back
- final video concat started/completed/failed
- audio mux started/completed/failed
- stale source, schema mismatch, or no-space failure

## Implementation Phases

### Phase A: Design Review

- Add this document.
- Ask ClaudeCode to review:
  - resume seek correctness
  - manifest schema
  - final concat/mux feasibility with `ffmpeg-the-third`
  - UI/task queue scope
  - audio final mux plan

### Phase B: Data Layer

- Add manifest structs + serde tests.
- Add queue structs + atomic save/load tests.
- Add queue lock/global mutex handling.
- Add plan schema/state handling (`planning`, partial plan resume, `plan_drift` failure).
- Add a named `TimeBase { num, den }` Rust type that serializes as `[num, den]` in JSON.
- Add path helpers for work dir, segment path, final temp path.
- Add cleanup helpers with path containment checks.
- Add disk-space estimate/free-space helpers.

### Phase C: Single-Task Segment Worker

- Refactor current export worker to encode one segment at a time.
- Persist completed segment metadata.
- Resume from last completed segment.
- Implement video-only segment concat first, before audio mux, to isolate resume correctness.
- Verify actual segment frame count against planned frame count.
- Keep current dialog as a per-task progress view.
- Manual test: start, pause/cancel, restart app, resume.

### Phase D: Finalization

- Harden video segment concat and timestamp continuity.
- Implement one-pass final mux/copy when possible; document and test any two-step fallback.
- Write existing `.miv.json` sidecar only after final output succeeds.
- Manual test: output duration, playback speed, audio presence, seeking.

### Phase E: Queue UI

- Add persistent queue.
- Add top-level video menu register/delete/task-window actions.
- Add sequential background dispatcher.
- Add compact two-row task window with pause/resume, queued task reorder, cancel/remove/retry
  controls, ETA, and failure reason display.
- Hide `<stem>.miv.work/` from folder scan results.
- On app exit, mark the running task back to `queued`, signal cancellation, and exit without waiting;
  startup recovery resumes it.
- Delete final output, sidecar, and work directory from the video menu delete action.
- Keep the normal UI serial. Advanced segment parallelism was tested but is not exposed because
  speedups were workload-dependent and confusing.

### Phase F: Keyframe-Snapped Segment Planning

- Build a keyframe-snapped segment plan before workers start so resume/pause does not require
  decoding from the beginning for each segment.
- Keep segment execution serial in the release UI. Parallel segment workers may remain as an
  internal experiment, but normal tasks should behave as one active segment at a time.
- Low-bitrate or noisy sources should be described as limited-benefit inputs in the registration
  dialog; the feature is primarily intended for already-clean low/mid-resolution videos viewed on
  higher-resolution displays.

## Review Questions For ClaudeCode

1. Is segment-based resume the right approach versus append-style resume?
2. Is storing work data in `<stem>.miv.work/` next to the source acceptable?
3. Is the manifest schema sufficient to resume without duplicating/skipping frames?
4. Should segment boundaries be frame-count based, timestamp based, or a hybrid?
5. Is final audio mux from the original source safer than per-segment audio copy?
6. Can final concat/mux be implemented cleanly with current `ffmpeg-the-third`, or should a small
   FFI wrapper be planned?
7. Should Phase C ship video-only resume first, or should audio final mux be blocking?
8. Are queue JSON and work manifest enough, or should the queue use SQLite from the start?

---

## 実装状況 (v0.9.0 リリース時点)

Phase A〜F まで実装済み。`src/video/upscale/` 配下のファイル構成:

| ファイル | 行数 | 責務 |
|---|---:|---|
| `mod.rs` | 6 | 公開 API (再エクスポート) |
| `job.rs` | 2551 ⚠ | scale / model preset / quality enum + Options + Preflight + run_job + segment 並列実装 + keyframe snap planning + concat/mux |
| `queue.rs` | 465 | TaskQueue / VideoUpscaleTask / TaskState / FailureReason / QueueLock + 永続化 |
| `manifest.rs` | 408 | JobManifest + SegmentPlan + SegmentEntry + JSON atomic save/load |
| `sidecar.rs` | 284 | `.miv.json` sidecar 読み書き |
| `disk.rs` | 92 | ディスク空き容量チェック |
| `paths.rs` | 188 | work dir / segment path / final temp path 計算 |
| `ui_dialogs/video_upscale.rs` | 1122 | 登録ダイアログ + タスクウィンドウ UI |

### `job.rs` 2551 行は最大の負債

Phase A〜F で機能追加するたびに job.rs に積み上げてきた結果、以下の責務が同居:

1. **公開 enum 定義群** (`VideoUpscaleScale` / `VideoUpscaleModelPreset` /
   `VideoUpscaleQuality`、~140 行)
2. **Options / Preflight 構造体** (~100 行)
3. **`probe_video_info`** (FFmpeg avformat 経由のメタデータ取得、~70 行)
4. **`run_job` メイン関数** (state machine 駆動の job 実行、~100 行)
5. **`run_segmented_video_only`** (シリアル segment loop)
6. **`run_segments_parallel`** (並列 segment loop、UI からは公開していない実験版)
7. **Keyframe snap planning** (`build_keyframe_snap_plan` / `scan_source_keyframes` /
   `plan_segments_from_keyframes`、~200 行)
8. **Segment lifecycle helpers** (frame_to_pts / segment_done_and_reusable /
   cleanup_segment_parts 等、~100 行)
9. **後段の concat / final mux**

自然な分割案 (Phase 10+):

```
upscale/
├── job/mod.rs              # VideoUpscaleJob struct + 公開 API + run_job
├── job/options.rs          # 公開 enum + VideoUpscaleOptions + VideoUpscalePreflight
├── job/probe.rs            # probe_video_info (FFmpeg avformat 経由)
├── job/segment_serial.rs   # run_segmented_video_only
├── job/segment_parallel.rs # run_segments_parallel (実験版)
├── job/plan.rs             # keyframe snap planning + plan_segments_from_keyframes
└── job/finalize.rs         # video concat + final mux + sidecar 書き込み
```

### レイヤ評価

| レイヤ | 状態 | 評価 |
|---|---|---|
| 永続キュー (`queue.rs`) | ✅ 良好 | TaskQueue / QueueLock の責務が明確、JSON atomic save、起動時 recovery 経路あり |
| マニフェスト (`manifest.rs`) | ✅ 良好 | JobManifest schema が clean、`save_json_atomic` / `load_json` のみ汎用化 |
| サイドカー (`sidecar.rs`) | ✅ 良好 | 単一責務、`.miv.json` 読み書きだけ |
| ジョブ実行 (`job.rs`) | ⚠⚠ 肥大 | 上述。機能 set としては正しく動いているが、ファイルが太い |
| UI (`ui_dialogs/video_upscale.rs`) | ✅ 良好 | 1122 行は登録ダイアログ + タスクウィンドウ + 進捗表示で妥当 |

### 抽象化リーク懸念

Job 実行が **同期 FFmpeg API を呼ぶ別スレッドで完結している**ため、UI スレッドが
job を直接観測することはない。`VideoUpscaleProgressShared` を mpsc 経由で UI に
push するため、抽象化リークは無い。

### 計画的負債

Phase G 候補としてあり得るもの (Phase 10+ で機能追加するなら):

- **並列 segment ワーカーを正式機能化**: `run_segments_parallel` は実装済みだが UI に
  公開していない (= 並列度が GPU メモリ / VRAM に依存して安定しないため)。`run_segments_parallel`
  単体テストを増やしてから公開を再検討
- **GeneralV3 以外のモデル選択**: 現状はリリース UI で「高速汎用」(`UpscaleRealEsrGeneralV3`)
  のみ。Anime6B / x4plus 等を有効化する選択肢は残してあるが UI から触れない
- **音声トラックの選択 / 字幕保持**: 現状は単一 audio stream を concat 後に mux で copy する。
  multi-audio / 字幕の保持は未対応
