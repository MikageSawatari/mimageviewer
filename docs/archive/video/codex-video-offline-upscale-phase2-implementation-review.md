# Codex Review Request: Offline Video AI Upscale Phase 2 MVP

## Scope

This change adds the first usable MVP path for offline video AI upscale:

- Context-menu entry for video files: `AI動画アップスケール...`
- Preflight dialog with output resolution, estimated size/time, 8K UHD guard, and audio omission notice
- User choices:
  - scale: `2x` or `4x`
  - model: `汎用 (高速)`, `アニメ`, `写真`
  - quality: five levels mapped to SVT-AV1 CRF/preset/pixel format
- Background worker:
  - decodes source video with FFmpeg
  - converts each frame to RGBA
  - runs existing ONNX upscale pipeline
  - downscales 4x model output to 2x when requested
  - encodes video-only MKV with `libsvtav1`
  - writes `.miv.json` sidecar after successful output finalization

## Key Files

- `src/video/upscale/job.rs`
  - options, preflight, progress, FFmpeg decode/encode worker
- `src/video/upscale/sidecar.rs`
  - existing Phase 1 schema reused for final metadata
- `src/ui_dialogs/video_upscale.rs`
  - dialog state, UI, worker launch, progress polling
- `src/ui_dialogs/context_menu.rs`
  - video context-menu entry
- `src/app.rs`
  - dialog state and render hook

## Important Design Choices

- MVP output is video-only. Audio copy/remux is intentionally deferred.
- Output container is MKV; video codec is AV1 via `libsvtav1`.
- Quality levels 1-2 use `yuv420p10le`; levels 3-5 use `yuv420p`.
- Output dimensions above 8K UHD (`long <= 7680`, `short <= 4320`) are blocked.
- Output path remains `<stem>.miv.mkv`; temporary file is `<stem>.miv.mkv.part`.
- Sidecar path remains `<stem>.miv.json`.
- `2x` currently runs the 4x model then downscales with Lanczos3, matching the earlier MVP decision.

## Review Focus

1. FFmpeg encode API usage:
   - stream time base / encoder time base
   - packet timestamp rescaling
   - `copy_parameters_from_context` timing before `write_header`
   - `libsvtav1` options (`crf`, `preset`, `film-grain=0`)
2. Frame conversion correctness:
   - decoded frame -> RGBA row copy
   - egui `ColorImage` -> RGBA frame
   - RGBA -> encoder pixel format via swscale
3. Cancellation behavior:
   - worker checks cancellation between expensive steps
   - `.part` file cleanup
4. UI responsiveness:
   - preflight and encode run on worker threads
   - dialog polls via channel and atomics
5. MVP limitations:
   - video-only output wording is clear enough
   - no automatic grid pairing/playback replacement yet
   - no resume support

## Verification Done

```text
cargo fmt --check
cargo test --lib video::upscale --target-dir target\codex-video-upscale2
```

Result: 7/7 video upscale tests passed. Existing unrelated `txt_norms` dead-code warning remains.

## Follow-up After ClaudeCode Review

Applied fixes:

- Packet timestamp rescale now uses the muxer-selected stream time base after `write_header`.
- GOP length now uses rounded fps (`fps_num / fps_den`) instead of raw numerator, so NTSC rates get roughly 2-second keyframes.
- Closing/canceling a running job now enters a canceling phase and keeps the dialog state until the worker finishes, avoiding a second worker writing the same `.part` path.
- Preflight reads width/height from codec parameters instead of opening a decoder.
- `send_packet` handles `EAGAIN` by draining decoded frames and retrying.
- 2x time estimate is no longer cheaper than 4x because both currently run the 4x model.

Deferred:

- AV1 D3D11VA decode reuse is Phase 2.5. The export path still decodes through the simple FFmpeg CPU path and then runs CPU RGBA conversion before AI inference.

Manual test checklist:

- Confirm output duration and playback speed match source.
- Confirm seeking is reasonable for 29.97 fps and 23.976 fps sources.
- Try short CFR 30 fps MP4, NTSC 29.97 fps MP4/MOV, and AV1 input.
- Try Japanese directory names, emoji/combining-character filenames, and UNC paths.
