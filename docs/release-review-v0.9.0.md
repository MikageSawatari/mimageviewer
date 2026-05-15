# v0.9.0 Release Review Notes

Last updated: 2026-05-14

This file accumulates pre-release review findings for v0.9.0. Findings are
grouped by feature area so they can be fixed in one batch after cross-checking
with the ClaudeCode review.

## Severity

- P1: Release blocker or data-corruption risk.
- P2: Likely user-visible bug or persistent bad state.
- P3: Low-risk bug, robustness issue, test gap, or documentation drift.

## Open Findings

### Video Offline Upscale

#### R-VUP-001 [P1] Resume manifest is reused without validating source/options

- Area: Offline video upscale resumable segments.
- Status: Open.
- Files:
  - `src/video/upscale/job.rs:620`
  - `src/video/upscale/job.rs:1347`
- Problem:
  - Existing `job.miv-upscale.json` is loaded whenever it exists, but the loaded
    manifest is not checked against the current source identity, output size, or
    encode options.
  - Completed segments are considered reusable when the segment file exists and
    its size matches the manifest entry.
- Impact:
  - If a source video is replaced with another file using the same stem, or if
    the user retries with different scale/quality after a failed run, old
    segments can be mixed into the final output.
- Suggested fix:
  - Validate `manifest.source`, `manifest.output`, and `manifest.options`
    against the current `VideoUpscaleJob` before calling `ensure_plan`.
  - If mismatched, fail as stale/plan drift or discard the work directory and
    create a fresh manifest.
  - Strengthen `segment_done_and_reusable` to validate the manifest path/state
    in the context of a validated manifest.

#### R-VUP-002 [P2] Final video is published before sidecar write succeeds

- Area: Offline video upscale finalization.
- Status: Open.
- Files:
  - `src/video/upscale/job.rs:347`
  - `src/video/upscale/job.rs:380`
- Problem:
  - The final `.miv.mkv.part` is renamed to `.miv.mkv` before the `.miv.json`
    sidecar is written.
- Impact:
  - If sidecar serialization or write fails, the task is reported as failed but
    the final video already exists. Retry can then be blocked by the existing
    output file unless overwrite is enabled.
- Suggested fix:
  - Write the sidecar to a temporary file first.
  - Publish final video and sidecar together in a recoverable order, or clean up
    the final video if sidecar publication fails.

### Video Tile / Seek Thumbnails

#### R-VTT-001 [P2] Tile thumbnail worker spawn failure keeps repainting forever

- Area: Native video tile thumbnails.
- Status: Open.
- Files:
  - `src/video/tile_thumbnails.rs:72`
  - `src/app/native_video.rs:2178`
- Problem:
  - `std::thread::Builder::spawn` failure is converted to `None` with `.ok()`,
    but `finished` stays `false`.
- Impact:
  - If thread creation fails, the tile overlay remains in an unfinished state and
    requests repaint every 80 ms.
- Suggested fix:
  - On spawn error, set `finished = true` and optionally store an error marker
    for the overlay/log.

#### R-VTT-002 [P2] Thumbnail extraction ignores best-effort timestamps

- Area: Hover seek thumbnail and video tile thumbnail extraction.
- Status: Open.
- Files:
  - `src/video/thumbnail.rs:300`
  - `src/video/tile_thumbnails.rs:290`
  - `src/video/decoder.rs:413`
- Problem:
  - Thumbnail workers use `frame.pts().unwrap_or(0)` while playback decode uses
    `video_frame_timestamp`, which falls back to FFmpeg
    `best_effort_timestamp`.
- Impact:
  - Old AVI/DivX/ASF-like files with missing PTS may play correctly but show
    wrong or repeated hover/tile thumbnails.
- Suggested fix:
  - Reuse `crate::video::decoder::video_frame_timestamp(&frame)` in both
    thumbnail workers.

#### R-VTT-005 [P2] Native hover preview can keep retrying an unseekable tail thumbnail

- Area: Native seek hover preview.
- Status: Open.
- Files:
  - `src/video/native_presenter/mod.rs:4837`
  - `src/app/native_video.rs:1626`
  - `src/video/mod.rs:3034`
  - `src/video/mod.rs:4504`
  - `src/video/thumbnail.rs:306`
- Problem:
  - The native presenter computes hover preview requests as
    `duration_secs * frac`, so the right edge can request exactly the container
    duration.
  - The actual seek path later clamps through `VideoPlayer::clamp_seek_target`,
    but `request_native_hover_thumbnail` only applies `max(0.0)`.
  - The thumbnail worker now accepts only the first decoded frame whose PTS is
    `>= target_secs`. For files whose container duration is after the final
    video frame PTS, a right-edge request can reach EOF without producing a
    thumbnail.
  - When the pointer leaves the HUD, the overlay clears its local
    `hover_preview_target_secs`, but `VideoPlayer::native_hover_thumbnail_target_secs`
    is never cleared. `pump_native_hover_thumbnail` therefore keeps
    re-requesting the same missing target on later polls.
- Impact:
  - Hovering near the right edge can leave the preview showing a stale image
    with the "シーク中" box, and can repeatedly seek/decode the tail in the
    background while playback continues.
- Suggested fix:
  - Clamp hover preview requests with the same seek target rule used by
    `VideoPlayer::seek`, or expose a preview-specific clamp that avoids
    duration-past-last-frame targets.
  - Add an overlay command/event to clear the native hover thumbnail target when
    the preview is hidden.
  - On EOF without a frame `>= target_secs`, either publish the last decoded
    frame with its actual PTS when appropriate, or negative-cache that bucket so
    the worker does not retry it every poll.

#### R-VTT-006 [P3] Fast source switch still carries some previous hover request state

- Area: Native presenter fast source swap and seek hover preview.
- Status: Open.
- Files:
  - `src/app/native_video.rs:3224`
  - `src/video/mod.rs:1634`
  - `src/video/native_presenter/mod.rs:3132`
  - `src/video/native_presenter/mod.rs:4924`
- Problem:
  - Fast video/tile navigation reuses the existing `NativeVideoOutput` and sends
    `SwitchSource` to the presenter.
  - The `SwitchSource` handler clears playback status, metadata, timeline
    markers, jump entries, and the current hover thumbnail image.
  - It still does not clear the presenter's hover preview request state
    (`hover_preview_target_secs`, `last_thumbnail_request_secs`, and related
    request timing).
- Impact:
  - After switching videos while the pointer remains around the seek bar, the
    new source can inherit a previous-source request target/timing and briefly
    suppress or mis-sequence the first hover-thumbnail request for the new
    video.
- Suggested fix:
  - During `SwitchSource`, reset all transient hover preview state
    (`hover_preview_target_secs`, `last_thumbnail_request_secs`,
    `last_thumbnail_request_at`, `last_seek_target_secs`, and any
    source-local pinned-preview state).
  - Consider making hover thumbnails source-epoch aware so a queued
    `SetHoverThumbnail` cannot be applied after a future source switch.

#### R-VTT-003 [P3] Tile thumbnail cache invalidation only uses mtime seconds

- Area: Persistent video tile thumbnail cache.
- Status: Open.
- Files:
  - `src/ui_video_tile.rs:149`
- Problem:
  - Cache invalidation passes `modified().as_secs()` only.
- Impact:
  - Rapid overwrite within the same second, or tools that preserve mtime, can
    leave stale tile thumbnails.
- Suggested fix:
  - Include at least file size and millisecond mtime in the cache identity.

#### R-VTT-004 [P3] Tile thumbnail worker comments still say Drop joins

- Area: Documentation/comment drift.
- Status: Open.
- Files:
  - `src/video/tile_thumbnails.rs:14`
  - `src/video/tile_thumbnails.rs:117`
- Problem:
  - Module comments say Drop joins the worker, but the implementation now
    detaches the thread to avoid blocking the UI thread.
- Impact:
  - Low runtime risk, but misleading for future maintenance.
- Suggested fix:
  - Update comments to describe cancel-and-detach semantics.

### VST3 / Video Audio DSP

#### R-VST-001 [P2] Chain rebuild workers can interleave and build a mixed VST3 chain

- Area: VST3 startup load, chain preset load, and preferences rebuild.
- Status: Open.
- Files:
  - `src/ui_dialogs/vst3_actions.rs:237`
  - `src/app.rs:3869`
  - `src/video/dsp/mod.rs:761`
  - `src/video/dsp/mod.rs:879`
  - `src/video/dsp/mod.rs:2204`
- Problem:
  - `kick_off_vst3_chain_rebuild_impl` spawns a worker that runs
    `disable -> enable -> add_plugin...` against the shared `DspBridge`, but
    there is no rebuild generation, cancellation token, or chain-mutation lock.
  - The startup loader mutates the same bridge through the same `add_plugin`
    API.
  - `add_plugin` chooses the target bridge and slot id from the current
    `inner.slots`, performs slow IPC/load work, and only appends the finished
    slot later.
- Impact:
  - Rapid preset loads, a preferences change while startup load is still
    running, or repeated rebuild requests can interleave.
  - The final runtime chain can mix entries from old and new requests, or split
    slots across multiple bridge processes. `process_block` only uses the first
    active bridge, so split slots can be silently ignored.
- Suggested fix:
  - Serialize whole-chain mutations, or introduce a monotonic rebuild generation
    where older workers stop before `disable`, before each `add_plugin`, and
    before publishing GUI visibility.
  - Treat startup load and preset/preferences rebuild as the same "last request
    wins" operation.
  - Consider making `add_plugin` reject calls whose generation no longer owns
    the bridge.

#### R-VST-002 [P2] VST3 process failure is treated as successful silence/latency

- Area: VST3 audio processing and failure recovery.
- Status: Open.
- Files:
  - `crates/vst3-host/src/main.cpp:1655`
  - `src/video/dsp/bridge.rs:900`
  - `src/video/dsp/bridge.rs:937`
  - `src/video/audio.rs:1004`
  - `src/video/audio.rs:1017`
- Problem:
  - The C++ bridge sends `process_block failed` and stops its audio loop when a
    plugin process call fails.
  - On the Rust side, `pull_audio` timeout or short reads return a partial
    sample count, and `process_audio_blocking` zero-fills the missing tail while
    still returning `Ok(())`.
  - The audio pump therefore treats the block as a successful VST block, keeps
    VST latency/limiter accounting active, and does not disable or bypass the
    failed bridge. If `process_block` does return `Err`, the dry fallback still
    uses plugin latency and marks the VST chain active.
- Impact:
  - A bad or crashed plugin can turn playback into persistent silence/dropouts
    instead of clean dry fallback or a visible session-level disable.
  - Dry fallback after a hard IPC error can still shift audio timing because PDC
    latency is applied to unprocessed samples.
- Suggested fix:
  - Surface short reads/timeouts as a distinct process failure instead of
    success-with-zero-fill.
  - Drain or observe bridge `Event::Error` on the audio failure path.
  - On failure, either auto-disable the bridge for the session or bypass the
    chain after a small consecutive-failure threshold.
  - For dry fallback chunks, use zero VST PDC and do not mark the VST chain
    active unless the samples actually came from the plugin chain.

#### R-VST-003 [P2] VST3 state snapshots can block UI actions for up to N seconds

- Area: VST3 plugin state save and chain preset save/rebuild.
- Status: Open.
- Files:
  - `src/video/dsp/mod.rs:924`
  - `src/ui_dialogs/vst3_actions.rs:148`
  - `src/ui_dialogs/vst3_actions.rs:242`
  - `src/ui_dialogs/preferences.rs:625`
- Problem:
  - `snapshot_all_plugin_states` now queries slots sequentially because chain
    bridge slots share one stdout/event stream.
  - The timeout is one second per slot, and callers invoke it directly from UI
    actions such as chain slot save, preferences OK/rebuild, and VST3 disable.
  - The comments still describe the old parallel per-plugin-bridge behavior and
    say this is not a UI hot path.
- Impact:
  - A 10-slot chain with unresponsive `getState` calls can freeze the UI for up
    to about 10 seconds while saving a chain slot or applying preferences.
- Suggested fix:
  - Move runtime state snapshotting into a worker with visible progress/disabled
    controls, or keep a background cached state snapshot refreshed outside the
    UI path.
  - If synchronous snapshotting remains, cap the total deadline rather than
    multiplying one second by slot count.
  - Update comments to match the chain-bridge sequential behavior.

#### R-VST-004 [P3] VST3 comments still describe the pre-chain-bridge design

- Area: Documentation/comment drift.
- Status: Open.
- Files:
  - `src/video/dsp/mod.rs:8`
  - `src/video/dsp/mod.rs:912`
  - `docs/vst3-integration.md:211`
- Problem:
  - The module header still says each slot owns an independent bridge process
    and estimates per-slot IPC round trips.
  - `snapshot_all_plugin_states` comments still describe parallel bridge
    queries even though the implementation is now sequential.
  - `docs/vst3-integration.md` references `chain_process`, which no longer
    exists in `src/video/dsp/mod.rs`.
- Impact:
  - Low runtime risk, but misleading during release stabilization because VST3
    behavior is now centered on one bridge process per chain.
- Suggested fix:
  - Update the module header, snapshot comments, and integration doc to describe
    the current one-chain-bridge model.

### Video Engine / Decoder / Clock

#### R-VENG-001 [P2] One-shot engine events can be dropped when the bounded event lane is full

- Area: Video engine event delivery, seek readiness.
- Status: Open.
- Files:
  - `src/video/mod.rs:2766`
  - `src/video/decoder.rs:2514`
  - `src/video/audio.rs:1183`
  - `src/video/engine/actor.rs:477`
  - `src/video/mod.rs:3017`
- Problem:
  - `engine_event_tx` is a bounded channel with capacity 64.
  - Critical decoder events such as `SeekCompleted` are sent with a one-shot
    `try_send`; if the channel is full, the event is silently dropped.
  - The audio pump emits `BufferReady` as a level event while enough processed
    audio is buffered, so a UI stall or bursty seek period can fill the same
    lane with repeatable audio readiness events.
  - `EngineActor` requires `SeekCompleted` to move from `Seeking` to
    `Buffering`; `FirstFrameReady` and `BufferReady` do not recover that state
    if the actor is still in `Seeking`.
- Impact:
  - A dropped `SeekCompleted` can leave the engine state stuck at `Seeking`.
    Because audio output silences and does not drain while engine state is not
    `Playing`, playback can remain muted or appear stuck until another seek or
    state-changing action occurs.
  - The non-native `emit_first_frame_event` path also marks an epoch as sent
    before the `try_send`, so a full channel can lose `FirstFrameReady` for that
    epoch.
- Suggested fix:
  - Make critical decoder events reliable: use a separate priority/unbounded
    lane, retry pending `SeekCompleted`/`FirstFrameReady` until delivered, or
    otherwise coalesce/drop only repeatable `BufferReady` events.
  - Mirror the native presenter retry pattern used for native
    `FirstFrameReady`.
  - Add a regression test that fills the engine event channel with
    `BufferReady`, then verifies that `SeekCompleted` is still eventually
    processed.

#### R-VENG-002 [P2] EOF stops the clock but never transitions EngineActor to Eof

- Area: EOF state synchronization.
- Status: Open.
- Files:
  - `src/video/decoder.rs:2746`
  - `src/video/engine/actor.rs:497`
  - `src/video/mod.rs:4135`
  - `src/video/mod.rs:4270`
- Problem:
  - The demux EOF path calls `clock.notify_eof_reached()` and sends EOF markers
    to the decode workers, but it does not send `DecoderEvent::EofReached` to
    `EngineActor`.
  - `EngineActor` has an `EofReached` handler and tests around it, but the live
    decoder path does not appear to exercise it.
  - The native and non-native EOF completion blocks directly call
    `clock.set_position_at_eof(...)` and `clock.set_playing(false)` after their
    drain checks, but they do not update the engine state.
- Impact:
  - After EOF, `AvClock` reports stopped while `engine_state_atomic` can remain
    `Playing`. Any code that treats `EngineActor` as the source of truth for
    pause/EOF/decoder parking sees a stale state.
  - This can skew perf/HUD diagnostics and makes future engine-driven behavior
    fragile; the documented `Eof` state is effectively bypassed in the main
    playback path.
- Suggested fix:
  - Synchronize `EngineActor` at the same drain-complete point that currently
    stops the clock, rather than immediately on demux EOF if preserving tail
    audio drain is required.
  - Alternatively, wire `DecoderEvent::EofReached` from the demux path and make
    the actor's EOF transition respect the existing drain latch.
  - Add an integration-style test or actor-facing harness that drives playback
    to EOF and asserts `engine_state_code() == EOF` after the EOF stop point.

### Playback Speed / Audio Normalize

#### R-VNORM-001 [P2] Native video fast-swap skips normalize DB lookup and gain application

- Area: Per-file audio loudness normalization during video-to-video navigation.
- Status: Open.
- Files:
  - `src/app.rs:9936`
  - `src/app.rs:9946`
  - `src/app.rs:11585`
  - `src/app/native_video.rs:1501`
  - `src/app/native_video.rs:3228`
  - `src/app/native_video.rs:3236`
- Problem:
  - The normal video open path inserts `FsCacheEntry::Video` and then calls
    `init_normalize_state_for_opened_video`, which performs the
    `audio_normalize.db` lookup and applies the stored gain when global
    normalize is enabled.
  - `start_native_video_source_swap` builds a new `VideoPlayer`, inserts it
    directly into `fs_cache`, and then calls `open_fullscreen(target_idx)`.
  - Because the entry already exists, `open_fullscreen` takes the video
    cache-hit branch and does not call `init_normalize_state_for_opened_video`.
- Impact:
  - Moving from one video to another through native fast-swap or tile fast-swap
    can leave normalize UI state at the default `Off` and leave player gain at
    `1.0`, even though global normalize is enabled and the target file has a DB
    measurement.
- Suggested fix:
  - Call `init_normalize_state_for_opened_video(target_idx)` after the fast-swap
    insertion, or move the "video entry became current" initialization into a
    shared helper used by both `start_fs_load` and fast-swap.
  - Add a regression test or harness that enables normalize, seeds a DB hit for
    the target video, performs a video-to-video fast-swap, and asserts the
    target player's normalize gain/UI state are applied.

### Video Pins / Bookmarks / Frame Capture

#### R-VPIN-001 [P2] Updating a pin before its new thumbnail is ready can permanently keep the old image

- Area: Video frame pin DB and grid thumbnail override.
- Status: Open.
- Files:
  - `src/video_pins.rs:110`
  - `src/video_pins.rs:126`
  - `src/app/native_video.rs:2455`
  - `src/app/native_video.rs:2473`
  - `src/app/native_video.rs:2501`
- Problem:
  - `VideoPinDb::set_pin` updates `pin_pts_secs` immediately, but when the
    incoming WebP is empty it preserves the previous `thumb_webp`.
  - `set_native_video_pin` intentionally allows this path when
    `nearest_seek_thumbnail(pts)` has not produced the new frame yet, then
    relies on `pending_pin_thumb_refresh` to replace the WebP later.
  - If the thumbnail worker cannot produce that frame, or the 10 second pending
    refresh times out, the row keeps the new PTS with the old WebP. The schema
    does not record which PTS the stored WebP belongs to, so grid and folder-pin
    consumers cannot detect the mismatch.
- Impact:
  - Re-pinning a video near an unextractable tail frame, or during a thumbnail
    extraction failure, can leave the grid/folder representative image showing
    the previous pin frame even though the marker and jump entry point to the
    new time.
- Suggested fix:
  - Store the thumbnail's source PTS alongside `thumb_webp`, or clear the WebP
    when `pin_pts_secs` changes and the new thumbnail is not available.
  - Treat "pin exists but thumbnail pending/missing" distinctly in grid/folder
    thumbnail resolution so stale representative images are not reused.
  - Add a regression test for `set_pin(old pts + webp) -> set_pin(new pts +
    empty webp)` that verifies stale WebP is either rejected by consumers or
    marked as belonging to the old PTS.

#### R-VBM-001 [P3] Native source switch can carry an old bookmark title editor into the new source

- Area: Native bookmark edit modal and video-to-video source swap.
- Status: Open.
- Files:
  - `src/video/mod.rs:1634`
  - `src/video/native_presenter/mod.rs:4165`
  - `src/video/native_presenter/mod.rs:5472`
  - `src/video/native_presenter/overlay_draw.rs:637`
  - `src/video/native_presenter/overlay_draw.rs:762`
  - `src/app/native_video.rs:2370`
  - `src/video_bookmarks.rs:215`
- Problem:
  - `SwitchSource` clears source-local metadata, timeline markers, jump entries,
    and hover thumbnails, but it does not clear
    `NativeEguiOverlay::bookmark_title_edit`.
  - The edit modal stores only the bookmark `id` and title text. If the modal is
    still alive after a source switch, saving it emits a fresh
    `SetBookmarkTitle` command under the new source epoch.
  - The app-side handler accepts that command as current and updates
    `video_bookmarks` by global `id` only; it does not verify that the bookmark
    belongs to the currently displayed video path.
- Impact:
  - A stale edit modal can rename a bookmark from the previous video while the
    user is already looking at the next video. The normal epoch stale-event
    guard does not catch this case because the command is generated after the
    switch.
- Suggested fix:
  - Clear `bookmark_title_edit` and other source-local modal state during
    `SwitchSource`.
  - Add a path-scoped DB update/delete helper, or make the app handler verify
    that `id` is present in the current video's marker cache before applying a
    bookmark mutation.

### Native Presenter / D3D11 GPU Path

#### R-VGPU-001 [P2] GPU blit failures can leak shared-output pool slots until the GPU path stalls

- Area: D3D11VA GPU blit and native presenter shared texture pool.
- Status: Open.
- Files:
  - `src/video/gpu_renderer/d3d11_device.rs:487`
  - `src/video/gpu_renderer/d3d11_device.rs:512`
  - `src/video/gpu_renderer/d3d11_device.rs:532`
  - `src/video/gpu_renderer/d3d11_device.rs:638`
  - `src/video/gpu_renderer/d3d11_device.rs:661`
  - `src/video/gpu_renderer/d3d11_device.rs:680`
  - `src/video/decoder.rs:3557`
- Problem:
  - `blit_nv12_to_rgba` acquires a shared output slot before creating the input
    view/output view and before running `VideoProcessorBlt`, `ReleaseSync`, and
    fence `Signal`.
  - If any of those later steps returns `Err`, the function exits before a
    `BlitOutput`/`D3d11Frame` is constructed.
  - The slot's `in_use` flag is only reset by `D3d11Frame::Drop` on the success
    path. The early-error path drops the local `Arc<AtomicBool>` without
    storing `false` or notifying the pool condition variable.
  - The decoder then logs "GPU path failed, fallback to CPU readback" and keeps
    playing, so repeated transient GPU failures can silently consume up to all
    24 shared-output slots.
- Impact:
  - After enough GPU blit errors, `acquire_shared_output` can report
    `shared output pool exhausted waiting for free slot`, causing persistent
    GPU-path failure and repeated CPU fallback/stutter until the player/device
    is recreated.
- Suggested fix:
  - Introduce an RAII guard returned by `acquire_shared_output` that resets
    `in_use` and notifies the pool on drop unless it is explicitly disarmed
    when building `BlitOutput`.
  - Cover failures after slot acquisition, including input view creation,
    output view creation, `VideoProcessorBlt`, keyed mutex release, and fence
    signal.
  - Add a unit/harness test around the pool bookkeeping if possible, or at
    least a debug assertion/perf counter proving `in_use` returns to false on
    injected blit errors.

### Settings DB / Video Resume Persistence

#### R-SET-001 [P2] Ambiguous settings directory enumeration can still fall through to clean install

- Area: SQLite settings migration boot decision tree.
- Status: Open.
- Files:
  - `src/settings_db.rs:614`
  - `src/settings.rs:1892`
  - `src/settings_db.rs:1798`
  - `src/settings_db.rs:1823`
- Problem:
  - `settings_db_family_exists` and `legacy_json_family_exists` return a plain
    `bool`.
  - After per-file `metadata` misses, both helpers fall through to `false` if
    `read_dir(data_dir)` itself fails.
  - `boot_settings_db_inner` treats `false` for both families as a confirmed
    clean install and creates a fresh `settings.db` with defaults.
- Impact:
  - If `%APPDATA%/mimageviewer` is transiently unenumerable, or if the same
    transient NotFound class that motivated the SQLite migration affects both
    per-file checks and directory enumeration, v0.9.0 can still create a default
    `settings.db`. Once that DB exists, later boots load it and no longer
    migrate the old `settings.json` family.
- Suggested fix:
  - Replace the boolean helpers with a tri-state result such as
    `Present / ConfirmedAbsent / Ambiguous`.
  - Only take the clean-install path for `ConfirmedAbsent`; on `Ambiguous`,
    retry briefly and then return `FailedFallbackDefault` with save suppressed.
  - Treat `read_dir` `NotFound` for a missing data directory as clean install,
    but treat permission/I/O/unknown errors as ambiguous.

#### R-VRES-001 [P3] The 5-second video resume timer updates memory but does not persist to disk

- Area: Video resume positions and SettingsDb persistence.
- Status: Open.
- Files:
  - `src/app.rs:2447`
  - `src/app.rs:14333`
  - `src/app.rs:14514`
  - `src/app.rs:13137`
- Problem:
  - `video_resume_last_save` is documented as a 5-second auto-save timer.
  - In `poll_video`, the timer only copies current player positions into
    `self.settings.video_resume_positions`; it does not call `settings.save()`
    or a SettingsDb row upsert.
  - The data is persisted later only when another settings save happens, such
    as folder navigation, tray hide, or normal app exit.
- Impact:
  - A crash, forced kill, or power loss after playback can lose the recent
    resume position even though the auto-save timer has fired.
- Suggested fix:
  - If crash resilience is intended, add a lightweight
    `upsert_video_resume_position` / `remove_video_resume_position` path and
    call it from the 5-second timer.
  - If only in-memory handoff was intended, rename the field/comment so future
    reviewers do not assume disk persistence.

#### R-SET-002 [P3] Preferences OK can replay stale runtime video settings

- Area: Preferences dialog settings merge.
- Status: Open.
- Files:
  - `src/ui_dialogs/preferences.rs:389`
  - `src/ui_dialogs/preferences.rs:571`
  - `src/ui_dialogs/preferences.rs:573`
  - `src/settings.rs:880`
  - `src/settings.rs:925`
  - `src/settings.rs:2410`
  - `src/app/native_video.rs:1194`
  - `src/app/native_video.rs:1269`
  - `src/app/native_video.rs:3130`
- Problem:
  - `PreferencesState` clones all settings when the dialog opens, then OK
    replaces `self.settings` with that snapshot after
    `overwrite_non_preferences_from`.
  - The merge helper does not preserve runtime-only video settings such as
    `video_tile_columns` and `audio_normalize_enabled`.
  - Both fields are changed and saved from native video controls outside the
    preferences page.
- Impact:
  - If a preferences window is open while the user changes tile column count or
    toggles global audio normalize from the native video UI, pressing OK in
    preferences can restore the older snapshot and persist it again.
- Suggested fix:
  - Add these runtime-managed fields to `overwrite_non_preferences_from`, or
    track per-page dirty fields so OK only writes settings that the preferences
    dialog actually changed.

### Release Packaging / FFmpeg Runtime

#### R-REL-001 [P1] The distributable launcher still reports and extracts as version 0.8.2

- Area: v0.9.0 release packaging, launcher runtime extraction.
- Status: Open.
- Files:
  - `Cargo.toml:6`
  - `crates/launcher/Cargo.toml:3`
  - `crates/launcher/src/main.rs:17`
  - `crates/launcher/src/main.rs:106`
  - `crates/launcher/build.rs:116`
  - `installer/mimageviewer.iss:5`
- Problem:
  - The root package and installer are set to `0.9.0`, but
    `crates/launcher/Cargo.toml` still has `version = "0.8.2"`.
  - The distributable `mimageviewer.exe` is the launcher, and it uses
    `env!("CARGO_PKG_VERSION")` both for `%APPDATA%/mimageviewer/runtime/<version>/`
    and for the Windows `FileVersion` / `ProductVersion` resource.
- Impact:
  - A v0.9.0 installer can ship an outer `mimageviewer.exe` whose file version
    and runtime extraction directory are still `0.8.2`.
  - This makes release verification confusing and can make different installed
    releases overwrite/reuse the same runtime extraction directory, relying only
    on hash sidecars to repair the mismatch.
- Suggested fix:
  - Bump `crates/launcher/Cargo.toml` to `0.9.0` before building release
    artifacts, or derive the launcher version from the workspace/root package
    so the two cannot drift.
  - Add a release check that compares root package version, launcher package
    version, and `installer/mimageviewer.iss` `MyAppVersion`.
  - Update launcher/build comments that still say the FFmpeg bundle has 5 DLLs;
    the current runtime embeds 6 (`avfilter-10.dll` included).

#### R-REL-002 [P2] Bash release build skips the VST3 bridge rebuild and cache cleanup

- Area: Release build scripts and embedded VST3 bridge.
- Status: Open.
- Files:
  - `scripts/build-release.sh:34`
  - `scripts/build-release.sh:40`
  - `scripts/build-release.sh:43`
  - `scripts/build-release.ps1:8`
  - `scripts/build-release.ps1:208`
  - `scripts/build-release.ps1:262`
  - `build.rs:148`
- Problem:
  - The PowerShell release wrapper rebuilds the C++ VST3 bridge before building
    `mimageviewer-core`, then removes the extracted `%APPDATA%` bridge cache.
  - The Bash wrapper still performs only the old two-step Rust build
    (`mimageviewer-core` then launcher). It does not rebuild
    `vendor/vst3-host/mimageviewer-vst3-host.exe`, even though the core embeds
    that file with `include_bytes!`.
  - It also does not clear the extracted VST3 bridge cache, so local release
    smoke tests can keep using an older bridge until the hash changes and
    extraction logic runs.
- Impact:
  - Anyone building the release through `bash scripts/build-release.sh` can ship
    a v0.9.0 core that embeds a stale VST3 host bridge, or can test against a
    stale extracted bridge and miss bridge-side fixes.
- Suggested fix:
  - Bring the Bash wrapper to parity with the PowerShell wrapper, or make it
    fail with a clear message directing release builds to the PowerShell script
    until parity is implemented.
  - Add a release verification step that compares the embedded bridge timestamp
    or hash against the freshly built `crates/vst3-host` output.

#### R-REL-003 [P3] The core executable Windows resource still says 0.1.0.0

- Area: Windows version metadata.
- Status: Open.
- Files:
  - `Cargo.toml:6`
  - `build.rs:29`
  - `build.rs:32`
- Problem:
  - The root package version is `0.9.0`, but the core executable resource sets
    `FileVersion` and `ProductVersion` to `0.1.0.0`.
  - The same resource also sets `OriginalFilename` to `mimageviewer.exe` even
    though this binary is built as `mimageviewer-core.exe`.
- Impact:
  - Runtime crash dumps, file properties, and support screenshots can report the
    embedded core as `0.1.0.0`, making v0.9.0 release verification and user
    diagnostics confusing.
- Suggested fix:
  - Derive the core resource version from `CARGO_PKG_VERSION`, matching the
    launcher resource strategy.
  - Set `OriginalFilename` to `mimageviewer-core.exe` for the core binary.

### AI / TensorRT

#### R-AI-001 [P2] TensorRT pack detection ignores the pack version and completeness

- Area: TensorRT acceleration pack activation.
- Status: Open.
- Files:
  - `src/ai/tensorrt_pack.rs:29`
  - `src/ai/tensorrt_pack.rs:63`
  - `src/app.rs:12426`
  - `src/app.rs:12498`
  - `src/ai/runtime.rs:147`
- Problem:
  - `EXPECTED_TRT_PACK_VERSION` is `3`, and the installer writes a JSON
    `INSTALL_OK` sentinel with the pack version and engine pack id.
  - Runtime/UI activation only calls `is_pack_installed()`, which checks that
    `INSTALL_OK` and `onnxruntime.dll` exist.
  - A stale v1/v2 sentinel, a manually copied pack, or a partially damaged pack
    with only the sentinel and ORT DLL still counts as installed.
- Impact:
  - v0.9.0 can try to start the TensorRT worker with an incompatible or
    incomplete pack, then fail late during worker startup/model load instead of
    offering a clear reinstall path.
  - This is especially risky because older pack revisions were explicitly
    withdrawn or changed the model set.
- Suggested fix:
  - Parse `INSTALL_OK` and require `version == EXPECTED_TRT_PACK_VERSION` and
    the expected `manifest_format`.
  - Verify the required pack DLLs and selected engine cache contents, or expose
    a stronger `pack_status()` enum that distinguishes valid, missing, stale,
    and corrupt installs.
  - Use that status in preferences, install completion, runtime startup, and
    worker lazy start.

#### R-AI-002 [P2] TensorRT worker timeouts do not detach or kill the stuck worker

- Area: TensorRT worker pool failure handling.
- Status: Open.
- Files:
  - `src/ai/trt_worker_pool.rs:73`
  - `src/ai/trt_worker_pool.rs:195`
  - `src/ai/trt_worker_pool.rs:202`
  - `src/ai/trt_worker_pool.rs:389`
  - `src/ai/runtime.rs:421`
  - `src/ai/runtime.rs:430`
  - `src/ai/upscale.rs:616`
- Problem:
  - `recv_resp_with_timeout()` returns an error such as
    `worker 応答 timeout (...)` when the child stops responding.
  - `classify_io_error()` marks the pool dead only for stdin/stdout I/O errors
    and EOF, not for response timeouts or disconnected reader errors.
  - `AiRuntime::infer_via_worker()` only detaches and raises the fallback notice
    when `pool.is_dead()` is true.
- Impact:
  - If the TensorRT child hangs in `LoadModel` or `Infer`, the current tile fails,
    but the pool remains attached.
  - Subsequent upscale/denoise tiles can keep routing to the same stuck worker
    and pay the 10-second timeout repeatedly instead of falling back to DirectML.
- Suggested fix:
  - Treat worker response timeout and stdout-reader disconnection as fatal for
    the current pool.
  - Kill/reap the child or call `shutdown()` before marking the pool dead, then
    detach so the next inference uses the DirectML path and the existing restart
    notification flow can run.

### Tags / Metadata

#### R-TAG-001 [P2] Tag writes can be abandoned during application shutdown

- Area: XMP tag write worker lifecycle.
- Status: Open.
- Files:
  - `src/tag_write_worker.rs:134`
  - `src/tag_write_worker.rs:189`
  - `src/tag_write_worker.rs:216`
  - `src/tag_write_worker.rs:256`
  - `src/rating_write_worker.rs:76`
- Problem:
  - `TagWriteHandle::Drop` only sets the shutdown flag and then drops the
    `JoinHandle`, detaching the worker thread.
  - The worker loop is designed to drain queued jobs and flush the Tantivy
    writer on shutdown, but the application does not wait for that drain/flush
    to finish.
  - The rating XMP worker handles the same shutdown problem by joining the
    thread in `Drop`.
- Impact:
  - If the user closes mIV immediately after applying tags, queued XMP sidecar
    or embedded-XMP writes and the follow-up search-index commit can be lost.
  - This leaves the UI's optimistic cache/undo expectations out of sync with
    files on disk and Ctrl+G results after restart.
- Suggested fix:
  - Join the tag worker during `Drop` after setting shutdown, matching
    `RatingWriteHandle`.
  - If a full join can block too long, add an explicit app-close drain phase
    with progress/cancel semantics and keep ordinary handle replacement
    non-blocking only where that is required.

### Search / Thumbnail Results

#### R-SEARCH-001 [P2] Ctrl+S synthetic results can collide on basename-only container thumbnail keys

- Area: Favorite/name search result thumbnails.
- Status: Open.
- Files:
  - `src/app.rs:4766`
  - `src/app.rs:4788`
  - `src/app.rs:4822`
  - `src/app.rs:16653`
  - `src/app.rs:16676`
  - `src/app.rs:16701`
  - `src/thumb_loader.rs:74`
  - `src/app.rs:16725`
- Problem:
  - Ctrl+S search results are loaded into the shared synthetic
    `search_results_synthetic_path()` catalog.
  - Folder/ZIP/PDF result items still use normal browsing keys:
    `folderthumb:{dirname}`, `zipthumb:{filename}`, and `pdfthumb:{filename}`.
  - In a real folder those names are unique, but a synthetic result list can
    contain `C:\A\cover.zip` and `D:\B\cover.zip` at the same time.
  - Ctrl+G `SearchContainer` thumbnails already avoid the same class of bug by
    including the full representative path in `CACHE_KEY_SEARCH_REP`.
- Impact:
  - Ctrl+S results can show another folder/archive's thumbnail for same-named
    containers, or keep overwriting/churning the same catalog row when mtime or
    size differs.
- Suggested fix:
  - Give Ctrl+S synthetic result items a search-result thumbnail key that
    includes the full container path, matching the `SearchContainer` pattern.
  - Alternatively, pass the current source/catalog context into
    `make_load_request` and switch to full-path keys only when
    `source_path == search_results_synthetic_path()`.

#### R-SEARCH-002 [P2] Turning off a name index can race with the old supervisor's final upsert

- Area: Ctrl+S favorite name index supervisor lifecycle.
- Status: Open.
- Files:
  - `src/app.rs:3783`
  - `src/app.rs:3786`
  - `src/app.rs:3809`
  - `src/name_index_supervisor.rs:582`
  - `src/name_index_supervisor.rs:588`
  - `src/name_index_supervisor.rs:591`
  - `src/name_index_supervisor.rs:684`
  - `src/name_index_supervisor.rs:687`
  - `src/search_index_db.rs:135`
  - `src/search_index_db.rs:221`
- Problem:
  - `apply_favorite_name_index_change(false)` signals the old supervisor to
    stop, moves the join into a background thread, and immediately calls
    `clear_for_favorite`.
  - The supervisor checks `cancel` immediately before `upsert_children`, but an
    upsert that has already passed that check can still be waiting on the
    `SearchIndexDb` mutex while the UI thread clears the favorite rows.
  - If `clear_for_favorite` acquires the mutex first, the old supervisor can
    acquire it afterwards and reinsert rows for a favorite that was just
    disabled or removed.
- Impact:
  - Ctrl+S can continue to return stale entries after the user turns off name
    indexing for a favorite or deletes that favorite, until a later full scan or
    manual cleanup happens.
- Suggested fix:
  - Make OFF/removal wait for the supervisor to fully join before clearing, with
    a progress/non-blocking UI handoff if needed.
  - Alternatively, move `clear_for_favorite` into the joiner after the old
    thread exits, or add a DB-level generation/disabled-favorite guard checked
    under the same mutex as `upsert_children`.

### External Launch / Update Notice

#### R-EXT-001 [P1] External player launch shells user-controlled paths through cmd.exe

- Area: External player / folder launch integration.
- Status: Open.
- Files:
  - `src/ui_helpers.rs:602`
  - `src/ui_helpers.rs:607`
  - `src/ui_helpers.rs:608`
  - `src/app.rs:8862`
  - `src/app/native_video.rs:2597`
  - `src/ui_fullscreen.rs:5590`
- Problem:
  - `open_external_player` opens files and folders by running
    `cmd /c start "" <path>`.
  - The helper is reachable from Shift+Enter on video items in grid,
    fullscreen, and native fullscreen paths, so the argument can be a
    user-controlled media path.
  - `cmd.exe` parses metacharacters such as `&` before launching the target.
    Windows filenames can contain those characters, so a crafted file or
    directory name can change the command executed by `cmd`.
- Impact:
  - Opening a maliciously named local video with the external-player shortcut
    can execute an unintended shell command under the user's account.
- Suggested fix:
  - Avoid `cmd.exe` for path opening. Use `opener::open(path)` or a direct
    `ShellExecuteW` wrapper that receives the path as data.
  - Keep URL opening on the existing `external_links::open_url` HTTP(S)-only
    path, and use a separate safe helper for filesystem paths.

## Verification Log

### 2026-05-14 Codex review pass 1

- `cargo test tile_thumb_cache --lib`: passed.
- `cargo test video::tile_thumbnails --lib`: passed.
- `cargo test --bin mimageviewer-core ui_video_tile`: passed.
- `cargo test --lib video::engine`: passed.
- `cargo test --lib settings_db`: passed.
- `cargo test --lib video::upscale`: passed, 45 tests.
- `cargo test --lib video::frame_selection`: passed, 11 tests.
- `cargo test --lib video::clock`: passed, 6 tests.

### 2026-05-14 Codex review pass 2

- Static review of VST3 startup/rebuild, chain bridge, and video audio pump.
- No automated tests were run in this pass because only review notes were
  updated.

### 2026-05-14 Codex review pass 3

- Static review of native seek hover preview, thumbnail request/clamp flow,
  presenter-to-player thumbnail target lifecycle, and fast source swap
  presenter state reset.
- No automated tests were run in this pass because only review notes were
  updated.

### 2026-05-14 Codex review pass 4

- Static review of committed `HEAD` only: video engine event delivery,
  decoder seek completion, EOF handling, and `AvClock`/`EngineActor` state
  synchronization.
- Uncommitted worktree fixes were intentionally excluded from this pass.
- No automated tests were run in this pass because only review notes were
  updated.

### 2026-05-14 Codex review pass 5

- Static review of committed `HEAD` only: playback speed wiring, audio
  stretcher/clock accounting, audio loudness normalize scan/app state, video
  bookmark/pin DBs, marker cache, pin thumbnail refresh, and frame capture
  helper.
- Uncommitted worktree fixes were intentionally excluded from this pass.
- No automated tests were run in this pass because only review notes were
  updated.

### 2026-05-14 Codex review pass 6

- Static review of committed `HEAD` only: D3D11VA GPU blit path, shared output
  texture pool, native presenter fence/keyed-mutex import path, and CPU
  fallback boundary after GPU blit failure.
- Uncommitted worktree fixes were intentionally excluded from this pass.
- No automated tests were run in this pass because only review notes were
  updated.

### 2026-05-14 Codex review pass 7

- Static review of committed `HEAD` only: FFmpeg DLL loader expectations,
  custom AVIO open-progress path, launcher extraction/versioning, Inno Setup
  packaging notes, and release-build scripts.
- Uncommitted worktree fixes were intentionally excluded from this pass.
- No automated tests were run in this pass because only review notes were
  updated.

### 2026-05-14 Codex review pass 8

- Static review of committed `HEAD` only: fullscreen/native navigation
  consistency, video-to-video native fast-swap, tile fast-swap, cache-hit video
  open path, and per-video initialization that should run after source swaps.
- Uncommitted worktree fixes were intentionally excluded from this pass.
- No automated tests were run in this pass because only review notes were
  updated.

### 2026-05-14 Codex review pass 9

- Static review of committed `HEAD` only: native bookmark modal/source-switch
  state, SQLite settings boot/migration decision tree, video resume persistence
  timer, and release helper script parity.
- Uncommitted worktree fixes were intentionally excluded from this pass.
- No automated tests were run in this pass because only review notes were
  updated.

### 2026-05-14 Codex review pass 10

- Static review of committed `HEAD` only: TensorRT pack installer/runtime
  activation checks, worker pool IPC timeout handling, main-process DirectML
  fallback design, and routed upscale inference behavior.
- Uncommitted worktree fixes were intentionally excluded from this pass.
- No automated tests were run in this pass because only review notes were
  updated.

### 2026-05-14 Codex review pass 11

- Static review of committed `HEAD` only: tag operation targeting, optimistic
  tag cache/Undo finalization, tag write worker shutdown behavior, and contrast
  with the rating XMP worker lifecycle.
- Uncommitted worktree fixes were intentionally excluded from this pass.
- No automated tests were run in this pass because only review notes were
  updated.

### 2026-05-14 Codex review pass 12

- Static review of committed `HEAD` only: Ctrl+S favorite/name search synthetic
  result loading, container thumbnail key generation, Ctrl+G representative
  thumbnail collision handling, and folder/ZIP/PDF pin-aware cache keys.
- Uncommitted worktree fixes were intentionally excluded from this pass.
- No automated tests were run in this pass because only review notes were
  updated.

### 2026-05-14 Codex review pass 13

- Static review of committed `HEAD` only: update check dialog/link handling,
  external URL validation, external player/folder launch helpers, native and
  egui video Shift+Enter launch paths, tray restore/update flow, and
  single-instance activation path.
- Uncommitted worktree fixes were intentionally excluded from this pass.
- No automated tests were run in this pass because only review notes were
  updated.

### 2026-05-14 Codex review pass 14

- Static review of committed `HEAD` only: folder thumbnail pin DB/source
  validation, pin cascade resolution, video-pin WebP seeding, SettingsDb save
  and preferences merge flow, Ctrl+S name bulk indexer, name index supervisor
  stop/clear sequencing, and search_index.db update pruning.
- Uncommitted worktree fixes were intentionally excluded from this pass.
- No automated tests were run in this pass because only review notes were
  updated.
