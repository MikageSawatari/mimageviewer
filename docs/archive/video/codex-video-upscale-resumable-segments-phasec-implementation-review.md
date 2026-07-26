# Codex Implementation Review: Resumable Video Upscale Phase C/D/E

Date: 2026-05-02
Reviewer target: ClaudeCode

## Scope

Phase C changes the existing offline video upscale worker from a single long `.miv.mkv.part`
encode into resumable video-only segments. Phase D adds final audio stream-copy muxing from the
original source after all video segments are complete. Phase E adds the first persistent queue UI
and sequential background dispatcher.

Implemented:

- Create/load `<stem>.miv.work/job.miv-upscale.json`.
- Create a simple time-based segment plan, targeting about 5 seconds per segment.
- Encode each segment into `<stem>.miv.work/segments/NNNNNN.mkv.part`, validate it, then rename to
  `NNNNNN.mkv`.
- Record completed segment metadata in the manifest after each segment.
- Resume by skipping manifest entries whose segment file still exists with the recorded size.
- Concatenate completed video-only segments into `<stem>.miv.mkv.part`.
- When the source has audio streams, write final output in one mux pass with re-timestamped
  segment video packets and stream-copied source audio packets.
- Finalization checks cancellation periodically while remuxing packets.
- Keep existing `.miv.json` sidecar publication after final output succeeds.
- Delete `<stem>.miv.work/` after final output and sidecar publication succeed.
- Add `video_upscale_tasks.json` queue loading/saving in appdata and keep one OS-level queue lock
  per data directory.
- Convert the video context menu action to queue registration: options dialog -> queued task -> task
  window.
- Add a compact two-row task window with running progress, cancel/remove/retry actions, and one
  background task at a time.
- Add queue pause/resume, queued task up/down controls, typed failure reason display, and corrupt
  `video_upscale_tasks.json` backup-on-load.
- On app exit, signal the running worker, mark the task back to `queued`, and return immediately so
  next launch resumes from completed segments.
- Hide `<stem>.miv.work/` directories from folder scan results.
- Add context-menu artifact delete for `<stem>.miv.mkv`, `<stem>.miv.json`, and `<stem>.miv.work/`.

Not implemented yet:

- Source keyframe scan / GOP snapping.
- Efficient seek to `seek_start_pts` for later segments.
- Parallel segment workers.

## Important Known Limitation

The Phase C segment encoder currently decodes from the beginning of the source for each pending
segment and drops frames until `target_start_frame`.

This is correct enough for the first resumable MVP but not efficient for late segments. It means:

- Resume avoids redoing completed AI/encode work.
- Resume may still spend decode/drop time before a later segment.
- The next optimization should use the planned `seek_start_pts` and source PTS/keyframe scan to
  avoid decoding from the beginning.

This limitation is intentionally left visible rather than hidden behind a fragile approximate seek.

## Files Changed

- `src/video/upscale/job.rs`
  - segment manifest integration
  - time-based plan generation
  - per-segment encode loop
  - segment validation and manifest updates
  - video-only segment concat/remux
  - final audio stream-copy mux for sources with audio
  - cancellation checks during final remux
  - successful publish work directory cleanup
  - unit tests for plan partitioning, timestamp rescaling, packet ordering, and segment reuse checks
- `src/video/upscale/{manifest,queue,paths,disk}.rs`
  - Phase B data layer from the previous review round
- `src/ui_dialogs/video_upscale.rs`
  - queue registration dialog
  - compact task window
  - sequential dispatcher and startup/exit recovery hooks
- `src/ui_dialogs/context_menu.rs`
  - register/delete/task-window video actions
- `src/app.rs`
  - persistent queue state
  - folder scan skip for `.miv.work`
  - polling and exit hooks
- `docs/archive/video/codex-video-upscale-resumable-segments-design.md`
  - final P3 cleanup and Phase E decisions

## Review Focus

1. Is the time-based segment plan safe as an MVP fallback before keyframe snapping?
2. Does `run_segmented_video_only` correctly skip reusable completed segments and update progress?
3. Is segment completion ordering safe: encode `.part` -> validate -> drift check -> rename ->
   manifest update?
4. Is `concat_video_segments` packet timestamp offset logic acceptable for CFR AV1 segment outputs?
5. Should Phase C block on implementing `seek_start_pts`, or is the current decode/drop limitation
   acceptable until the keyframe scan work?
6. Any concern with keeping final sidecar publication in `run_job` while the segment manifest lives
   in the work directory?
7. Does the final audio copy path correctly preserve stream parameters and write video/audio packets
   in timestamp order for MKV?
8. Does Phase E avoid blocking UI work while still saving the small queue JSON safely?
9. Is the app-exit behavior acceptable: mark running task queued, signal cancel, and exit without
   waiting for worker cleanup?
10. Are cancel/delete semantics clear enough: task cancel discards work data; context-menu delete
    removes final output, sidecar, work dir, and queue entries?

## Verification

Commands run:

```powershell
cargo fmt --check
$env:LIBCLANG_PATH='C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\Llvm\x64\bin'; cargo test --lib video::upscale
$env:LIBCLANG_PATH='C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\Llvm\x64\bin'; cargo check --lib
```

Result:

- `cargo fmt --check`: pass
- `cargo test --lib video::upscale`: 38 passed
- `cargo check --lib`: pass
