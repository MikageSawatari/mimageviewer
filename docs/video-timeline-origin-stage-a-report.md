# Video timeline origin: stage A checkpoint

2026-11-14. Implementation is incomplete and **not compiled or runtime-verified**. All changes remain in this worktree on `fix-video-timeline-origin`; no product binary was launched.

## Commits

None. Explicit-path `git add` failed before staging: unable to create `C:/home/mimageviewer/.git/worktrees/mimageviewer-timeline-origin/index.lock` (permission denied). A fixture commit message is prepared at `target/timeline-fixtures-commit.txt`.

## Open design decision

The independent reviewer found a remaining P2 at `src/app/book_resume_meter.rs:228`: a `LegacyRaw` resume value is still divided by media duration. E.g. raw 5.4 at origin 1.4 must display media 4.0, not 5.4. `src/app/metadata_ops.rs:1814` can return a catalog hit without opening FFmpeg; `src/catalog.rs:214` has no origin field. Therefore propagating a field from the existing FFmpeg probe alone cannot fix old catalog hits.

Proposed extension, awaiting design-lead approval: add a nullable origin column to the released/rebuildable catalog metadata; probe an old readable metadata row once through the existing worker when its origin is unknown, then reuse it. This adds no UI-thread I/O but increases the first background probe for old cached metadata. Keep the settings map basis unchanged. No meter suppression, guessed zero origin, or new synchronous probe was implemented.

## Verification

Added 25 tests: MediaTimeline policy/roundtrip/ticks (2), decoder/player/filter/late-track/identity-format regressions (6), thumbnail/index/range-audio pixel/pulse regressions (8), Remote actual fMP4 origin/segment/pixel regressions (2), selective cache migration (4), released resume migration (3). These have not run.

| Exact command | Result |
|---|---|
| `python .\scripts\ui-smoke\generate_timeline_origin_fixture.py .\testdata\timeline-origin` | PASS: 8 fixtures, 828457 bytes; FFmpeg 9.0.2/lavfi. Fixture README records independent ffprobe, pixel, pulse and interlacing measurements. |
| `cargo fmt` | PASS (workspace, no arguments). |
| `cargo fmt --check` | PASS. |
| `git -c core.safecrlf=false diff --check` | PASS. |
| `powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "timeline-origin codex" -WorkDir G:\home\mimageviewer-timeline-origin -Command "cargo check -p mimageviewer --bin mimageviewer-core"` | BLOCKED before compilation: Cargo package fingerprint traversal cannot read `testdata/timeline-origin/timeline-origin-zn9u0i7e` (OS error 5). Runner used its authorized no-lock fallback, BelowNormal/6 jobs; cargo exit 101. |
| Focused Rust tests | NOT RUN: blocked by the same package traversal. No Rust test pass/fail counts available. |
| Full lib suite | NOT RUN. Required once after focused checks and the pending design correction. |
| Verification binary (`scripts/build-dev.ps1`) | NOT BUILT: compilation prerequisite remains blocked. |

After the scratch directory is removed and the remaining design correction is approved, use the heavy runner for each Cargo build/test. Focused filters: `test(/timeline_origin/)`, `test(/media_timeline/)`, `test(/timeline_migration/)`, `test(/video_resume_media_basis|video_resume_legacy_json|resume_migration/)`; broaden to the affected video/audio/settings/app suites. Final full-lib invocation: `powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "timeline-origin codex" -WorkDir G:\home\mimageviewer-timeline-origin -FullTest -Command ".\scripts\test-lib-full.ps1"`.

Deletion of only the generator scratch directory was automatically rejected (`blocked by policy`); a contained nonrecursive `os.rmdir` was also denied by its ACL. No ACL/security changes or permission bypasses were attempted. The current generator pipes chapter metadata via stdin and creates no scratch in its output directory.

## FFmpeg boundary inventory

The exact plan section 3 search was rerun at this checkpoint: 270 matches across 58 files. A = input absolute (convert to media), S = seek locator (convert media to raw immediately before FFmpeg), R = raw locator kept raw, O = output-own (unchanged), D = difference/duration only (unchanged), M = upper media API (not an FFmpeg boundary), N = not a time/no time operation. All matching lines are included, including false positives. Supplemental helper exits follow because the search does not select every timestamp consumer.

| File:line(s) | Kind and action |
|---|---|
| `crates/musicvideo-editor/src/app.rs:915` | M: editor sample/frame player seek, not FFmpeg input. |
| `crates/musicvideo-editor/src/playback.rs:428,634,641,688,787` | D: cpal callback/playback wall-clock difference. M: editor-specific sample/frame player seek, not FFmpeg input. |
| `crates/musicvideo-editor/src/resource_blobs.rs:250` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `crates/vst3-host/src/plugin_loader.cpp:1137,2454,2509,2535` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/ai/tensorrt_installer.rs:726` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/app.rs:55026,72925,99781,99929,99959,100081,100232,105040` | M: upper VideoPlayer/Remote session media API; conversion is at decoder/clockless boundary. |
| `src/app/collection_navigation.rs:3922,9759` | M: upper VideoPlayer/Remote session media API; conversion is at decoder/clockless boundary. |
| `src/app/gamepad_input.rs:710,713,5411` | M: upper VideoPlayer/Remote session media API; conversion is at decoder/clockless boundary. N: UI gesture/toast Instant/elapsed or comments. |
| `src/app/native_video.rs:7113,7178,7208,7510,12864,13574,13611,14620,15923,15925,16206,16216` | M: upper VideoPlayer/Remote session media API; conversion is at decoder/clockless boundary. N: UI gesture/toast Instant/elapsed or comments. |
| `src/app/tests/effetune_auto_open.rs:91` | M: upper VideoPlayer/Remote session media API; conversion is at decoder/clockless boundary. |
| `src/audio_decode.rs:208,281,287,325` | A: input time base. S: range start media_to_raw_secs then AV_TIME_BASE -> seek. A: range frame timestamp -> raw_to_media_secs. Full decode is sample-count based; no absolute time API. |
| `src/bin/bench_seek_decode.rs:25` | D: duration helper. Targets are media seconds passed into production seek_strip_bench path. |
| `src/bin/normalize_probe.rs:63` | R: abuffer/filter-internal time_base setting; whole-file loudness scan neither publishes nor compares absolute source times. |
| `src/content_identity.rs:3408,3460,3623` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/depth_export/tests.rs:442,454` | O: generated MP4 frame PTS == index/30 verification. |
| `src/editing_addon_download.rs:468` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/epub_convert.rs:828,844` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/fs_animation.rs:544,556,575` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/ingest_text.rs:337` | N: chapters() only reads title metadata, never chapter timestamps. |
| `src/name_bulk_indexer.rs:177,285,291,294` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/name_index_supervisor.rs:44,831,860,880,950,954,975` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/native_exception_log.rs:450` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/png_metadata.rs:206,245,246,302,381` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/remote_ipc/collections.rs:365,385,1610,1611,1615,1618,1619,3523,3533` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/remote_ipc/ui.rs:2454` | M: upper VideoPlayer/Remote session media API; conversion is at decoder/clockless boundary. |
| `src/ring_shortcut.rs:3327,3339,3345,3355,3370,3380,3386,3394` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/search_index_db.rs:2211` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/similar_book_query_bench.rs:1018,1056` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/similar_search_array.rs:755` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/ui_dialogs/preferences/search_index.rs:1806,1868` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/ui_fullscreen.rs:27577,54602,54605,54847,54855,58680,58711,58747` | M: upper VideoPlayer/Remote session media API; conversion is at decoder/clockless boundary. N: UI gesture/toast Instant/elapsed or comments. |
| `src/ui_main.rs:31059` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/ui_music_panels.rs:772` | M: upper VideoPlayer/Remote session media API; conversion is at decoder/clockless boundary. |
| `src/video/audio.rs:4924,5057` | M: upper VideoPlayer media seek. D: existing multi-timebase.mp4 test checks time-base ratios only. |
| `src/video/avio_progress.rs:276` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/video/clock.rs:693` | N: explanatory comments; actual FFmpeg seek boundary in decoder.rs. |
| `src/video/clockless_transcode.rs:1193,1220,1255,2198,2375,2726,2730,4425,4426` | O: timed_packet consumes encoded segment DTS. A: source time bases/audio frame PTS/stream start -> raw_to_media_secs. S: media source_origin -> media_to_raw_secs then seek ticks. O: generated MP4 regression reads encoded output-relative timestamps. |
| `src/video/decoder.rs:11,444,450,462,469,488,1186,1201,2186,2187,2193,2194,2199,2486,2797,2801,3419,3421,3424,3425,3430,3432,3447,3448,3476,6232,7617,8989,11531,11700,12357` | R: raw frame/packet tick helpers. R: bwdif internal time-base rescale, no rebasing. A: stream start -> raw_to_media_secs. D: stream duration. A: lane time bases, converted at exits below. A: chapter start/end -> raw_to_media_secs. S: demux seek/comments: media_to_av_time_base before both FFmpeg calls. D: container duration. S: existing fixed-O=0 audio-only.flac raw seek test. R: internal raw/filter explanation and existing fixed-O=0 MKV filter test. |
| `src/video/engine/state.rs:131` | N: explanatory comments; actual FFmpeg seek boundary in decoder.rs. |
| `src/video/media_timeline.rs:15,36,63` | A: sole start_time/NOPTS -> effective origin policy. S: media_to_raw_secs then AV_TIME_BASE conversion. |
| `src/video/mod.rs:13556,13584,13586,13615,13631,13654,13696,13702,13743,13750,14975,14988,15003,15023,15039,15081,17437,17453,17454` | M: upper VideoPlayer/Remote session media API; conversion is at decoder/clockless boundary. |
| `src/video/mvexport/mux.rs:103,109,134,137,200` | O: custom H264/AAC encoder packet DTS ordering/rescale/mux, independent of source-media origin. |
| `src/video/mvexport/video_only_tests.rs:50` | D: generated MP4 duration/frame count checks. |
| `src/video/normalize_scanner.rs:121,288,564` | A: packet PTS -> raw_to_media_secs before progress publication. D: container duration. Filter-internal timestamps remain raw. |
| `src/video/screenshot.rs:38,63,64,194` | A: input time base; frame exit covered below. S: media_to_av_time_base then seek. N: fixture documentation. |
| `src/video/seek_strip.rs:566,592,599,602,629,660,1526` | R: raw AVIndexEntry locator retained; only published axis -> raw_to_media_secs. D: duration/2 computes warmup media position. S: warmup -> media_to_av_time_base. S: regression uses media_to_stream_ticks to install raw index entries. |
| `src/video/seek_strip_batch.rs:71,605,746` | N: byte seek. D: container duration supplies media axis. N: error name. |
| `src/video/seek_strip_thumbs.rs:1669,2548,2676,2681,2693,2886,2887,3060,3287,3288,3299,3303,3891,3966` | A: input time bases; timeline created from same Input. S: target -> media_to_raw_secs -> checked secs_to_av_time_base. R: packet DTS->PTS mapping; both compared on converted media axis. A: regression packet PTS/DTS -> media before colour assertion. D: probe duration. |
| `src/video/stream/segmenter.rs:819,822,896,899,1526,1527,1534,1627,1871,1881,1886` | O: encoded session-relative packet PTS/DTS and output regression tests. Source-origin restoration stays in upper media contract. |
| `src/video/thumbnail.rs:411,509,510` | A: input time base; frame adoption/publication below. S: media_to_av_time_base then seek. |
| `src/video/tile_thumbnails.rs:440,528,529,569` | A: input time base; frame adoption below. S: media_to_av_time_base then seek. N: missing-PTS comment. |
| `src/video/timeline_origin_tests.rs:324` | M: upper VideoPlayer/Remote session media API; conversion is at decoder/clockless boundary. |
| `src/video/upscale/job.rs:729,739,887,900,901,1622,1623,1897,1908,1991,2003,2032,2042,2043,2087,2103,2135,2140,2247,2270,2351,2383,2539,2808` | D: stream duration -> estimated frame count. R: source_time_base/manifest/keyframe/source_last_pts and stream seek use raw source locators, never published as mIV seconds. O: segment encoder/concatenation remux/original audio remux output axis remains unchanged. |
| `src/video/upscale/manifest.rs:388` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/video/upscale/sidecar.rs:139` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/xmp_reader.rs:275` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/xmp_reader_partial_read_tests.rs:577` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/zip_loader.rs:453,3346,4010,4025,4034,4077,4082,4084,4087,4093,4119,4153` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |

## Supplemental helper boundaries

| File / owner | Kind | Action |
|---|---|---|
| `src/video/decoder.rs`: demux `packet_timestamp` consumers and video first-packet diagnostics | input absolute | Convert tick-scaled seconds through `timeline.raw_to_media_secs` before queues/logs/perf. |
| `src/video/decoder.rs`: main video frame and bwdif output timestamp adoption | input absolute | Convert immediately after tick scaling; filter time-base rescaling stays raw. Frame-step selection, preroll, tap and emitted frames use media seconds. |
| `src/video/decoder.rs`: audio `emit_audio_samples` | raw internal -> input absolute | Preserve raw `AudioResampleTimeline` including EOF tail; convert once before sample trim/clock/output. Diagnostic source and resampler boundary times also convert. Packet preroll comparisons use media; packet durations remain differences. |
| `src/video/thumbnail.rs`: frame comparison and returned thumbnail PTS | input absolute | Convert through the worker's immutable timeline. |
| `src/video/tile_thumbnails.rs`: frame comparison | input absolute | Convert through the worker's immutable timeline. |
| `src/video/screenshot.rs`: frame comparison/publication | input absolute | Convert through the helper's own input timeline. |
| `src/video/seek_strip_thumbs.rs`: frame comparison and key-packet DTS -> PTS mapping | input absolute / raw locator kept raw | Retain packet tick locators; convert seconds at comparison with the published media cell axis. |
| `src/video/clockless_transcode.rs`: main and EOF video decode | input absolute | Source PTS become media before trimming/transcode. Source origin is media. Encoded segment/session timestamps remain relative through StreamTimeline. |
| `src/video/seek_strip.rs`: `read_index_keyframes` | raw locator -> input absolute | AVIndexEntry remains raw; only its published seconds on the strip axis convert. |
| `src/video/upscale/job.rs`: `frame_to_pts`, manifest source PTS and stream seek | raw locator kept raw | No migration/rebase. Encoder receives its own segment frame index; original-audio remux remains unchanged. |

The static boundary review found no further unconverted FFmpeg-facing source-time boundary. This is not evidence of runtime acceptance. The independent review's open UI meter P2 is documented above.
