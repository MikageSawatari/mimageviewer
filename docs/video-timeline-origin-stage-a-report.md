# Video timeline origin: stage A verification record

2026-11-14. Stage A implementation and independent-review fixes are applied. The review-fix focused run passed1014/1014; the requested full lib run completed once with17629 passed,6 failed,1 timed out,76 skipped. All nine newly added review regressions passed in its JUnit. Complete failures and subsequent test-harness correction/diagnostics are recorded below; this is not a successful full gate. No product binary has been launched; all playback observations below are noninteractive decoder/FFmpeg tests or source inspection.

## Acceptance by the design lead (880718eb0, outside the Codex sandbox)

- Independent implementation review (GPT-6.1 Sol / xhigh, thread `01a1d7f1-e551-71f1-93bd-7714a9678ea5`): "accept after fixes" on 553140833, then **accept** on 880718eb0.
- `run-heavy.ps1 -FullTest .\scripts\test-lib-full.ps1` at 880718eb0: 17636 run, 3 failed, all in `app::folder_pane_refresh_tests` at about 115 s each while other heavy jobs shared the PC. Re-run alone (`cargo nextest run -p mimageviewer --lib -E 'test(/folder_pane_refresh_tests/)'`): 48/48 passed. The Codex-sandbox failures listed below (agent_ipc named pipes, scan deadlines, log-writer barrier) did not recur; `agent_ipc` alone: 176/176 passed.
- `.\scripts\build-dev.ps1` at 880718eb0: passed.
- Runtime behaviour in the real app is not verified (agents do not launch the product binary); the user checks it with the dev-runtime build.

## Commits and ownership

- `30030f112` - `feat(video): Rebase MPEG-TS/PS timestamps to media time (WIP checkpoint)` (committed by the design lead).
- `553140833` - `fix(video): Finish the media-time origin stage A (Codex follow-up)` (committed by the design lead before this review-fix round).
- Follow-up edits remain in this worktree, `fix-video-timeline-origin`. The design lead performs commits; the implementer does not run git add/commit.
- The design lead removed the inaccessible generator scratch directory. Cargo now compiles in this worktree.

## Independent review fixes after 553140833

The separate design-lead review accepted after fixes P2-1, P2-2, P2-3, P3-4 and P3-5. The changes below address those findings; follow-up independent Sol/xhigh source review found no remaining P1/P2 issue. These are headless checks and source review, not live-product observations.

| Finding | Root correction | Regression coverage |
|---|---|---|
| P2-1: unresolved saved resume erased before metadata | One `VideoPlayer::resume_position_for_save(audio_mode_without_vst) -> Option<f64>` owns both position selection and save availability. While the player owns a saved row without VideoInfo it returns None. Every local producer omits the resume update and thumbnail request, leaving the existing value, LegacyRaw/Media basis and watched state untouched. After metadata it uses the existing save rules. No new pending state is added. | Three actual App regressions cover repeated periodic/save-all, metadata adoption enabling the ordinary periodic migration, detached exit harvesting and passive-context teardown. The Player metadata test also asserts save-query availability before/after adoption. Both typed bases and an initially invalid Media value are covered. |
| P2-2 / P2-3: format origin rounding creates tiny negative source times | `MediaTimeline` retains the AV_TIME_BASE integer origin. For nonzero effective origins it maps only converted values in `(-1 us, 0)` to zero. Exact -1 us and genuinely negative preroll remain negative; O_eff=0 returns source seconds unchanged. Audio end validity is independent of start validity. No strip-specific fallback is added. | Precision boundary/identity unit coverage; late-audio first stream (index1) startSome(0), known end1079040/90000, availability before/after end; a genuine negative-start/known-positive-end metadata unit; actual indexed-strip worker first-cell pixel check on an audio-less TS. |
| P3-4: integer warmup seek loses a microsecond through f64 | Strip warmup retains duration/2 in integer AV_TIME_BASE units and uses `MediaTimeline::media_av_time_base_to_raw`. No seconds round trip. The integer origin is the canonical stored origin, with seconds derived for seconds callers. | 2003500 stays2003500, plus signed/extreme O_eff=0 values and positive/negative-origin integer translations. |
| P3-5: positive origin Remote case cannot prove seek conversion | Add actual clockless transcode of wrap-origin.ts requested at media4s, whose raw locator is about-0.621333s. A missing conversion instead seeks raw+4s, past the requested scene/window. | Existing fMP4 packet timestamps, client position and first-frame magenta pixels are all asserted for the new negative-origin case as well. |

### Every local resume-save route

| Producer / caller | Shared query or route |
|---|---|
| `src/app.rs:100530` periodic poll | Calls `resume_position_for_save` before creating MediaResumeUpdate. |
| `src/app.rs:98844` save-all | Calls the same query before row/thumbnail work. |
| `src/app.rs:15597` context teardown plan | Calls the same query; media paths are captured before skipping an unavailable resume update, so disposal still occurs. |
| `src/app.rs:66621` passive/parked teardown | Uses the context teardown plan. |
| `src/app.rs:66952` detached exit harvesting | Uses the context teardown plan. |
| `src/app/runtime_ops.rs:593,595` application exit | Uses save-all and detached exit harvesting. |
| `src/tray_integration.rs:423,425` tray exit | Uses save-all and detached exit harvesting. |
| `src/app.rs:66832,66834` Remote handoff | Uses save-all and detached exit harvesting. |
| `src/app.rs:80830,80976,82299,82961` replacement/prefetch/fullscreen close | Uses save-all. |
| `src/app/native_video.rs:3423,15321`; `src/ui_fullscreen.rs:47624` native swap/continuous-reading prefetch | Uses save-all. |

Remote progress updates have their own resolved source-position snapshot; they do not sample an unresolved local player and keep that existing ownership. The shared store mutation policy itself is unchanged.

### Precision and fixture evidence

New `rounded-origin-video-only.ts`: 83472 bytes, SHA256 `f4efbc860b24afe91a807f5386c8cab10bc6f7e7878274eda5f17bb0a12f6d15`. It has no audio,144 I/key frames and144 PTS=DTS packets; first raw PTS3240150000/90000=36001.666666666664, format origin36001.666667, D12. The first center pixel is(253,0,0). This is system FFmpeg/lavfi generation and headless measurement. Regeneration uses the existing exact command `python .\scripts\ui-smoke\generate_timeline_origin_fixture.py .\testdata\timeline-origin`; all eight existing fixture SHA256 values stayed unchanged. Total nine fixture files:834047 bytes. Generator/README describe this added precision case.

The indexed-strip regression explicitly installs the144 actual verified key-packet raw DTS/PTS/byte-position/size locators with av_add_index_entry into a test-owned Input, then requests the real worker's first indexed cell. A first narrow run found the test-setup assumption that TS demux alone would populate the full index was false (6 entries, expected144). Setup was corrected without weakening that assertion. It explicitly checks Ready/red rather than accepting DecodeFailed or a cold TimeGrid fallback. No production scan or new seek path is introduced. Independent follow-up review accepted this test-only correction.

The integer audit found the introduced integer -> seconds -> integer FFmpeg locator round trip only at strip warmup. Playback/thumbnail/screenshot take seconds from their APIs; strip decode, range audio and Remote also start with seconds and retain their existing rounding rules. Upscale source locators already use raw integer rescaling and remain unchanged. Waveform quantization/cache keys are not FFmpeg seek locators and were not changed.

Chapter conversion remains in production. MPEG-TS/MPEG-PS do not carry chapters, so a rebased-container chapter regression is unavailable. The positive-origin MKV regression verifies unchanged O_eff=0 chapter times; no artificial chapter test or claim of rebased chapter runtime coverage is added.

### Review-fix verification commands and results

All commands use the required outer runner:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "timeline-origin codex" -WorkDir G:\home\mimageviewer-timeline-origin -Command "<inner command>"
```

The shared lock directory remains unwritable in the sandbox; the runner reports and uses its authorized BelowNormal/6-job fallback. No dependency settings or personal configuration are changed.

| Exact inner command / phase | Result |
|---|---|
| `cargo fmt` | PASS, workspace-wide, no arguments. |
| Initial narrow run: `cargo nextest run -p mimageviewer --lib -E 'test(/saved_resume_|origin_precision_|integer_seek_locators|enumerate_audio_track_end_survives|timeline_origin_first_audio_track|timeline_origin_rounded_first_indexed|remote_timeline_origin_wrap|player_resume_media_and_legacy/ )' --no-fail-fast --status-level fail --final-status-level fail` | 11 run:10 passed,1 failed in2.125s (c79de616-b946-440f-8107-8271bbb456f8). Sole failure was the TS index preparation assumption, corrected as described above. |
| Review-fix focused before full: `cargo nextest run -p mimageviewer --lib -E 'test(/timeline_origin|media_timeline|timeline_migration|video_resume|resume_migration|resume_for_open|sanitize_resume|tile_thumb_cache|seek_strip|audio_decode|clockless|enumerate_audio|audio_track|thumbnail|screenshot|video::tests::|engine::actor|saved_resume_/)' --no-fail-fast --status-level fail --final-status-level fail` | PASS:1014 run,1014 passed,16698 not selected in75.354s (56595f9b-cbcb-459d-8fda-10636abac799). |
| Full lib, exactly once: outer runner with `-FullTest -Command ".\scripts\test-lib-full.ps1"` | FAIL:17636 run,17629 passed,6 failed,1 timed out,76 skipped in1756.772s;60 slow,2 leaky. JUnit a18dfa16-aeb9-4445-b29b-f5cb115ad2e9. |
| Post-full diagnostic: `cargo nextest run -p mimageviewer --lib -E 'test(/no_resident_smart_pdf_missing_root_row_rolls_back_history|visibility_settings_ok_retires_extracting_and_grouping_on_worker_scan_path|rotation_stall_and_snapshot_handle_preserve_saved_prefix|timeline_origin_ts_decoder_first_frames_seek_tail_colour_and_eof/)' --no-fail-fast --status-level fail --final-status-level fail` | PASS:4 run,4 passed in2.934s (8baeeb77-6fea-4a79-9399-a590b473de52). Includes corrected EOF harness. The three other assertions passed unchanged in isolation; their full-run failures remain failures with unconfirmed causes. |
| `cargo fmt`; `cargo fmt --check`; `git diff --check` | PASS after the test-only harness correction. Run before the diagnostic by ignored `target/timeline-origin-review-diagnostics.ps1`, supplied to the same runner. |
| Final focused after the EOF harness correction: same exact1014-test command above, through `target/timeline-origin-review-focused.ps1` | PASS:1014 run,1014 passed,16698 not selected in76.667s (4be74915-224b-4c9b-b5a1-0502371065c3). This is the final-source focused result. |
| Verification build: same outer runner with `-Command ".\target\timeline-origin-build-dev.ps1"` | PASS:exit0 at13:06:21 JST. Normal-feature core3m35s; Remote0.93s, EPUB0.73s, MCP0.60s, music-video5.79s. Runtime/PE checks passed:runtime=4 / pe=5. Uses the process-only explicit/static native prerequisite environment documented in the historical build record below, then `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`. No application launch. |

The final focused invocation is wrapped by ignored local `target/timeline-origin-review-focused.ps1`, which runs `cargo fmt`, checks its exit code, then executes exactly the focused command above. The wrapper itself is passed as the runner's inner command. Nine tests are newly added in this round:3 App save lifecycle,2 MediaTimeline precision/integer,1 independent audio end,1 late-audio metadata,1 indexed strip pixel,1 negative-origin Remote. Existing Player resume assertions are also expanded. No assertions were loosened.

| Newly added test | File:line |
|---|---|
| saved_resume_pending_metadata_survives_periodic_and_save_all | src/app/tests.rs:82469 |
| saved_resume_metadata_resolution_enables_periodic_migration | src/app/tests.rs:82514 |
| saved_resume_pending_metadata_survives_detached_exit_and_passive_teardown | src/app/tests.rs:82550 |
| origin_precision_zeroes_only_sub_microsecond_negative_rounding | src/video/media_timeline.rs:153 |
| integer_seek_locators_preserve_microseconds_and_identity | src/video/media_timeline.rs:179 |
| enumerate_audio_track_end_survives_a_genuinely_negative_start | src/video/decoder.rs:9987 |
| timeline_origin_first_audio_track_keeps_end_when_start_rounds_to_zero | src/video/timeline_origin_tests.rs:265 |
| timeline_origin_rounded_first_indexed_strip_cell_returns_first_colour | src/video/seek_strip_thumbs.rs:3329 |
| remote_timeline_origin_wrap_ts_seeks_negative_raw_locator_and_relative_segments | src/video/clockless_transcode.rs:4490 |

Full evidence: `target/test-lib-full/runs/20261114T031625908Z-ca627b6d/{stderr.log,junit.xml}`, source hash `68fd0a25a0c5eca1ad80d9af90e10a9f1f664eb65e62bf2e5d7b93156ddd1b89`. Source stayed unchanged throughout that run. Its JUnit confirms all nine new review regressions passed. After the run, only the existing EOF test's harness configuration (described below), documentation and ignored diagnostic scripts changed; production code did not change.

| Full failure / location at that snapshot | Exact observation / disposition |
|---|---|
| `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`, `src/agent_ipc/pipe.rs:334` | Windows code5 / PermissionDenied connecting to the named pipe. Same known sandbox restriction; design lead separately reports176/176 IPC tests passed outside the sandbox. |
| `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`, `src/agent_ipc/tests.rs:762` | Windows code5 / PermissionDenied preparing fixture server. Same known sandbox restriction. |
| `app::tests::smart_folder_transition_tests::no_resident_smart_pdf_missing_root_row_rolls_back_history`, `src/app/tests.rs:103139` | Existing5s scan deadline: `missing row scan timed out`. Test/Smart-PDF path unchanged; no video player is created. Same named failure is recorded in docs/egui-0.37-stage1a-verification.md:353. Current causal diagnosis remains unconfirmed. |
| `app::folder_scan::section1375_scan_perf_tests::section1375_large_physical_scan_10k_100k`, `src/app/folder_scan.rs:3605` | Nextest terminated this existing performance test at600.258s. No assertion/deadline/source changes; not rerun as part of the lightweight diagnostics. |
| `filename_stack_ui::tests::visibility_settings_ok_retires_extracting_and_grouping_on_worker_scan_path`, `src/filename_stack_ui.rs:2392`, assertion `src/app/tests.rs:1208` | Existing10s listing-identity wait did not settle. This unchanged test creates PNG/JPEG entries and no video player. The helper's deadline remains unchanged. |
| `log_writer::tests::rotation_stall_and_snapshot_handle_preserve_saved_prefix`, `src/log_writer/tests.rs:184` | `service.shutdown(Instant::now() + BARRIER_WAIT)` returned false; BARRIER_WAIT is2s (`src/log_writer.rs:18`). Unchanged log-writer files; no saved-prefix assertion was reached. |
| `video::decoder::timeline_origin_tests::timeline_origin_ts_decoder_first_frames_seek_tail_colour_and_eof`, `src/video/timeline_origin_tests.rs:173` | Final-frame distance from D failed the unchanged0.1s assertion. The harness supplied engine PLAYING although it consumes frames without a paced presenter; production intentionally drops full-queue frames in PLAYING (`decoder.rs` CPU enqueue path). Corrected the test setup to existing lossless BUFFERING backpressure; frame-step stays SEEKING. Retained the skipped-frame counter and strengthened checks to zero skips and144 delivered frames. No product path or EOF tolerance changed. Independent follow-up review accepted the harness correction. |

The initial full-result status filter missed TIMEOUT lines; the final accounting above is from complete nextest summary/JUnit and includes the600s timeout. No failure is omitted. The same runner's full-test lock was observed to belong to another live full run started12:25:07 while this run used its authorized no-slot fallback. This records observed overlap, not proof of the cause of each timing failure. No other worktree/process/lock was changed. Subsequent diagnostics queued behind that full-test lock.

The three unchanged assertion failures passed in the single4-test diagnostic; their full-run causes remain unconfirmed. The600s performance timeout was not rerun. The EOF harness correction passed both the diagnostic and final1014-test focused run with zero drops/all144 frames and the unchanged tail tolerance. No second full run was made, as requested. Thus the full result above is the pre-harness-correction snapshot of the same production code, not a claimed successful full gate for the final test source.

## Accepted stored-value display exception

Per plan section 4 and the 2026-11-14 design-lead decision, the grid meter and cached resume tile preview retain a LegacyRaw stored value until that video is opened and the ordinary periodic save writes a Media value. No catalog origin column or metadata re-probe is added. The former open catalog/meter item is closed by this explicit decision.

## Resume owner and behavior before the first frame

The checkpoint always supplied OpenOptions.resume_secs=None. That froze both engine and shared clocks at zero even for a known Media row, removing the previous immediate resume anchor while metadata loaded. It also left an unused engine metadata resume path alongside VideoPlayer's actual restore path.

The follow-up keeps one restoration owner: VideoPlayer retains the typed pending row, resolves it once at VideoInfo, and issues the existing synchronous clock request plus actor seek transition. Engine OpenOptions now has only an immutable initial_position_secs scalar; its metadata resume branch and corresponding obsolete tests are removed. Metadata adoption is synchronous after any valid restore seek and before self.info is published: rejected Media rows become Buffering(0) in the same tick, so Remote cannot start from the discarded initial anchor. This is smaller than giving the engine a typed pending row: it needs no new InfoReceived origin field, second pending owner, or Remote/DSP cancellation interfaces. Existing seek generation and the existing DSP Exact state supersede an old pending resume without adding state.

| Phase / visible consumer (source-inspected) | Media row | LegacyRaw row |
|---|---|---|
| Before VideoInfo: position()/position_secs(), both clocks | Frozen at the stored finite nonnegative media value immediately, restoring the released engine behavior and correcting the checkpoint's zero anchor. | Frozen at 0. Unlike released raw resume, the unconverted stored value is never shown by the playback clock. This also applies to legacy rows of O_eff=0 formats until metadata arrives. |
| Native HUD timeline inputs | The sampled clock position is the Media value. | The sampled clock position is 0 until metadata resolves it. |
| Seek bar before duration metadata | Duration is still unknown/0, so position controls/progress are unavailable in both cases. Loading an initial position does not enable seeking early. | Same. |
| Native center status before first presented frame | Existing preparing status remains; no new loading/status behavior is introduced. Visible time labels, when drawn, format the sampled HUD position described above. | Same preparing status; a visible position label uses 0 before metadata and resolved media after metadata. |
| After VideoInfo but before first post-seek frame | A valid row synchronously publishes its unchanged media target and increments the seek epoch once; invalid/finished rows start at0. | A valid row converts raw through MediaTimeline, then validates and synchronously publishes that media target once. Values below origin or invalid/finished rows start at0. |
| Remote source origin | remote_stream_start_inputs returns None while metadata/pending resume is unavailable; once resolved it returns the chosen media position, even for a paused player without a first rendered frame. | Same; no raw source origin is published. |
| A newer seek before metadata | Existing seek generation wins; saved resume is consumed without another seek. | Same. |

These are source and headless state-transition assertions, not observations of live native UI. References: VideoPlayer::open/open_with_output_consumer, initial_position_for_resume, tick's VideoInfo adoption, remote_stream_start_inputs; actor::new/begin_loading/InfoReceived; native presenter samples source.clock.now_secs() in mod.rs and gates progress by video_position_controls_available(duration); center preparing status is guarded by !first_frame_presented.

## Pre-existing seek-preroll finding (out of scope, backlog 1.780)

The original checkpoint30030f112 fixtures were 25 fps with inter-predicted GOPs. Linked avformat-61/avcodec-61 headless probes on those original fixtures found the following pre-existing timestamp-seek behavior (not product runs). These measurements describe the original fixtures, not the regenerated all-intra files:

| Source / request in media seconds | Observed ordinary backward seek | Consequence |
|---|---|---|
| small/large/wrap TS, 0.5 | First video packet PTS 0.501333 / DTS 0.461333 is non-key; next key PTS 1.021333. | Screenshot returns 1.021333; thumbnail tolerance rejects it; requested colour is unavailable. |
| TS, 11.5 | Lands on a non-key packet with no later keyframe before EOF. | Flushed decoder cannot produce the requested tail frame. |
| TS after full packet demux | No persistent index entries; timestamp seek generates falsely flagged key entries. | Reading the file once does not make subsequent timestamp seek safe. |
| TS with all true packet keys inserted via av_add_index_entry | Ordinary timestamp seek still lands on the non-key target packet. | Merely warming/populating AVIndexEntry is insufficient. |
| TS with verified true key byte locator | Byte seek returns key media 0.021333 / 4.021333 / 11.021333; decoding yields red / magenta / gray. | A real preceding key locator can supply correct preroll. |
| PS | Ordinary seek also lands on non-key packets; some true keys have packet.pos=-1 and byte restart can shift the parser timestamp by a frame. | A TS-only byte shortcut is not a complete common PS solution. |

This is a seek/preroll defect, not a wrong origin or an incorrect expected colour. FFmpeg n7.1 [mpegts_get_dts](https://github.com/FFmpeg/FFmpeg/blob/n7.1/libavformat/mpegts.c#L3146-L3149) labels timestamp probes `AVINDEX_KEYFRAME /* FIXME keyframe? */`; [seek.c](https://github.com/FFmpeg/FFmpeg/blob/n7.1/libavformat/seek.c#L581-L584) selects binary search for read_timestamp demuxers. These explain why AVSEEK_FLAG_BACKWARD alone does not establish the required keyframe invariant.

The design lead classified this as a pre-existing, origin-independent defect, including O=0 TS, outside this branch (2026-11-14). It is recorded as [backlog 1.780](next-release-backlog.md). A future fix needs a common owner for verified key locators and bounded cold-seek/cancellation/performance design, including PS parser restart and missing packet positions. This branch adds no seek owner, byte locator, scans, offsets or retries.

For this branch, the lead explicitly selected all-intra origin fixtures. The final files use 12 fps, -g1, -bf0 and MPEG-2 low_delay so all video packets are true keys and PTS=DTS. Twelve fps keeps frame-quantization below the existing strict0.1s EOF assertion; low_delay avoids a PTS/DTS one-frame seek bias crossing the unchanged0.05s tail assertion. Interlaced material remains interlaced. The fixture README records the final origins/durations/sizes and the reason for this isolation. Assertions were not loosened; exact bwdif neighboring-frame expectations were recalculated for the 12fps grid. Existing separate inter-frame decoder/filter regressions remain unchanged.

Diagnostic scripts remain under ignored `target/`: `timeline_aux_probe.py`, `timeline_aux_seek_modes.py`, `timeline_aux_byte_decode.py` (linked DLL probes, no product launch).

## Checkpoint verification through 553140833 (historical)

The original implementation added 25 tests (the follow-up also adds three Player metadata-adoption tests and replaces obsolete actor resume tests): MediaTimeline policy/roundtrip/ticks (2), decoder/player/filter/late-track/identity-format regressions (6), thumbnail/index/range-audio pixel/pulse regressions (8), Remote actual fMP4 origin/segment/pixel regressions (2), selective cache migration (4), released resume migration (3).

All Cargo builds/tests use `powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "timeline-origin codex" -WorkDir G:\home\mimageviewer-timeline-origin -Command "<command>"`. The runner uses its authorized BelowNormal / 6-job fallback when its shared lock directory is not writable.

| Owner / exact inner command | Result |
|---|---|
| Fixture worker, final regeneration: `python .\scripts\ui-smoke\generate_timeline_origin_fixture.py .\testdata\timeline-origin` | PASS: 8 fixtures, 750575 bytes. All 144 frames per video are I/key frames; all 144 video packet PTS/DTS agree; the interlaced fixture remains interlaced. README records FFmpeg/ffprobe versions, pixel/pulse/interlacing measurements and regeneration. |
| Design lead: `cargo nextest run -p mimageviewer --lib -E 'test(/timeline_origin|media_timeline|timeline_migration|video_resume|resume_migration|resume_for_open|sanitize_resume|tile_thumb_cache|seek_strip|audio_decode|clockless|enumerate_audio|audio_track|thumbnail|screenshot/)'` | 795 run: 789 passed, 6 failed. Bootstrap test setup plus screenshot/hover/tile/strip/tail-seek regressions. |
| Implementer: `cargo nextest run -p mimageviewer --lib -E 'test(/timeline_origin_ts_decoder_first/)' --no-capture` | Compiled successfully in 4m09s. 1 run: 0 passed, 1 failed; reproduced no tail seek frame for small-origin.ts (nextest run 1cfb05da-4898-450a-9c1d-17fe3936f724). |
| Implementer (intermediate, before same-tick metadata correction): `cargo nextest run -p mimageviewer --lib -E 'test(/player_resume|player_premetadata_seek|resume_migration|video_resume_media_basis|video_resume_legacy_json|engine::actor|metadata_stage|metadata_before|remote_start|remote_stream_start_inputs/)' --status-level fail --final-status-level fail` | 99 run: 99 passed (6aee39d5-b654-4ac1-88ce-1fb5bafd1c66). Superseded for final resume acceptance by the following run. |
| Implementer (after synchronous metadata correction): `cargo nextest run -p mimageviewer --lib -E 'test(/video::tests::|engine::actor|video_resume_media_basis|video_resume_legacy_json|remote.*(resume|origin|start|opening)/)' --status-level fail --final-status-level fail` | 276 run: 276 passed in 74.993s (e1d51143-9f96-4974-a8d4-e2fabbc517ef). Includes invalid first-tick Remote origin, Media/Legacy pre-frame positions, repeated metadata, newer-seek precedence, DSP Current/Exact continuations and released DB migration. |
| Implementer, before all-intra regeneration: `cargo nextest run -p mimageviewer --lib -E 'test(/timeline_origin|media_timeline|timeline_migration|video_resume|resume_migration|resume_for_open|sanitize_resume|tile_thumb_cache|seek_strip|audio_decode|clockless|enumerate_audio|audio_track|thumbnail|screenshot|video::tests::|engine::actor/)' --no-fail-fast --status-level fail --final-status-level fail` | 1005 run: 1000 passed, 5 failed in 75.825s (99c6120c-c946-4b70-8b90-6b4a3c8277b3). Historical inter-GOP seek/preroll failures, superseded by final fixture runs below. |
| Implementer, final origin/migration run: `cargo nextest run -p mimageviewer --lib -E 'test(/timeline_origin|media_timeline|timeline_migration|video_resume_media_basis|video_resume_legacy_json|resume_migration/)' --no-fail-fast --status-level fail --final-status-level fail` | PASS: 25 run, 25 passed in 0.675s (bab692a5-1c01-4ca7-9830-346d970ea44b). |
| Implementer, final combined focused run: `cargo nextest run -p mimageviewer --lib -E 'test(/timeline_origin|media_timeline|timeline_migration|video_resume|resume_migration|resume_for_open|sanitize_resume|tile_thumb_cache|seek_strip|audio_decode|clockless|enumerate_audio|audio_track|thumbnail|screenshot|video::tests::|engine::actor/)' --no-fail-fast --status-level fail --final-status-level fail` | PASS: 1005 run, 1005 passed in 75.789s (2008ff43-e42e-4f24-aea9-bc9ea6dd9294). |
| Final follow-up: `cargo fmt --check`; `git diff --check` | PASS. |
| Full lib suite, once: same runner with `-FullTest -Command ".\scripts\test-lib-full.ps1"` | FAIL: 17627 run, 17625 passed, 2 failed, 76 skipped in 1045.332s; 19 slow and 3 leaky reported by nextest (4fd9ba50-ff95-4160-ab92-80071cacf5fa). Both failures are existing IPC PermissionDenied cases below. No rerun or exclusions. |
| Verification binary, initial Cargo attempt: same runner with `-Command ".\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0"` | FAIL: turbojpeg-sys1.2.0 CMake/MSBuild exit1 at Checking Build System; Cargo exit101, before product Rust compilation. No detailed MSB3191/C1041 diagnostic was printed. |
| Native prerequisite: same runner with `-Command "cmake --build target/dev-runtime/build/turbojpeg-sys-ad9ab6b47bbc75e3/out/build --target install --config RelWithDebInfo --parallel 1 -- /verbosity:normal"` | PASS: exit0, 0 warnings, 0 errors, 2m49s. Same pinned source and Cargo-generated RelWithDebInfo/MT/SIMD configuration; no Debug/MTd library reused. |
| Verification binary, final invocation: same runner with `-Command ".\target\timeline-origin-build-dev.ps1"` | PASS: exit0. Normal-feature core8m42s, Remote49.57s, EPUB worker29.91s, MCP16.27s, music-video editor20.76s. Runtime/PE check PASS: runtime=4 / pe=5. This ignored local script supplies the process environment below, then invokes build-dev.ps1. No product binary was launched. |

The DB migration test setup now saves Settings::default() before reconstructing the old table, establishing the released bootstrap_complete invariant without weakening migration assertions. An intermediate DB/engine/player run passed 99/99 before the reviewer found and we corrected a same-tick invalid-Media Remote-origin bug. The final DB/engine/player follow-up passed 276/276 after that correction. After the accepted all-intra regeneration, all 1005 combined focused tests passed. Independent follow-up review accepted the final resume, fixture and scope changes with no additional P1/P2 findings. The one-owner resume cleanup is described above.

Full-lib evidence is in `target/test-lib-full/runs/20261114T015914449Z-e453bdc4/{stderr.log,junit.xml}` (ignored local artifacts), source hash `d7d15010ece19397751879d6458feabd59fc109a54470a3e88676435944b068c`. The source was unchanged during the run. Final report-only edits follow this result.

| Full-lib failure / location | Observed failure and scope |
|---|---|
| `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`, `src/agent_ipc/pipe.rs:334` | Named-pipe client open returns Windows code5 / PermissionDenied. Server creation succeeded; the failure is the client connection. |
| `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`, `src/agent_ipc/tests.rs:762` | Fixture-server preparation returns Windows code5 / PermissionDenied before the child executable is spawned. |

These IPC files are unchanged by this branch. The same two failures are documented for earlier implementation-environment full runs in [local-vlm-caption-plan.md](local-vlm-caption-plan.md) (lines2680-2686) and [effetune-fork-plan.md](effetune-fork-plan.md) (lines981-987). This records the observed access restriction and matching baseline; the precise Windows token/ACL cause was not diagnosed. The full gate did not pass in this environment. No product-code workaround, skipped test or identical full rerun was introduced.

The initial verification-build invocation omitted `-WaitForOtherBuildsMinutes 0` and waited in the script's native-build preflight without invoking Cargo. It was interrupted after repeated samples showed unchanged CPU time for all listed MSBuild processes. The second invocation uses the script's documented wait option; other worktrees and processes were untouched. `-PreserveRuntime` prevents this build from stopping a resident staged product process.

The native prerequisite failure and single-job build match the earlier procedure recorded in [search-result-navigation-682-688-plan.md](search-result-navigation-682-688-plan.md) (lines467-497). The initial failure's specific MSBuild cause remains unconfirmed; the single-job build result establishes that the same source/configuration can build here. The final invocation copies the successfully built `out/lib/turbojpeg-static.lib` to `target/timeline-origin-native-runtime/lib/turbojpeg.lib`, verifies equal SHA256, and uses only this PowerShell-process environment:

```powershell
$env:X86_64_PC_WINDOWS_MSVC_TURBOJPEG_SOURCE = 'explicit'
$env:X86_64_PC_WINDOWS_MSVC_TURBOJPEG_STATIC = '1'
$env:X86_64_PC_WINDOWS_MSVC_TURBOJPEG_LIB_DIR = 'G:\home\mimageviewer-timeline-origin\target\timeline-origin-native-runtime\lib'
$env:X86_64_PC_WINDOWS_MSVC_TURBOJPEG_INCLUDE_DIR = 'G:\home\mimageviewer-timeline-origin\target\dev-runtime\build\turbojpeg-sys-ad9ab6b47bbc75e3\out\include'
$env:CXXFLAGS_x86_64_pc_windows_msvc = '/IG:\home\mimageviewer-timeline-origin\target\dev-runtime\build\turbojpeg-sys-ad9ab6b47bbc75e3\out\include'
$env:MSBUILDDISABLENODEREUSE = '1'
.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0
```

The ordinary include spelling supplements the canonical Windows extended-path include passed downstream by turbojpeg-sys, as documented in that existing procedure. Cargo features, dependency sources, repository build scripts and personal configuration are unchanged.

## Verification handoff

The latest normal-feature `dev-runtime` build, including the review corrections, completed successfully at13:06:21 JST on2026-11-14. The core and Remote service, EPUB PDF worker, MCP adapter and music-video editor are ready; FFmpeg/VCRT companions are staged. The existing PE check passed for all five executables and four VCRT files; its local report is `target/vcrt-pe-reports/dev-runtime.json`. This build does not enable `portable` or `test-script`. The earlier11:42 build remains recorded only in the historical checkpoint table above.

The user launches the normal-profile core from this worktree:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

Close the installed/tray-resident mImageViewer first: the single-instance mutex is shared. This fast build uses the normal `%APPDATA%\mimageviewer` profile by default and can update real settings/data. No agent-driven product launch or live native/Remote/HUD observation is claimed.

Concrete manual scenarios (not yet performed):

1. Open `testdata/timeline-origin/large-origin.mpg`: duration is about12s and the playback clock starts near0, rather than the raw10-hour origin. Seek around4.5s (magenta) and11.5s (gray); verify position labels and the seek bar agree with the scene. Frame-step in the interlaced fixture can be checked if TS direct-open is supported in the chosen route.
2. Let the existing periodic resume save run around4-5s, close, then reopen. The Media resume position should be visible while loading and playback should restore there once. A paused Remote start should use that media position as its source origin while encoded segment timestamps start at the session origin.
3. Open `positive-origin.mkv`: preserve container0, duration22s, first picture around10s, and chapter times10/14/18s. The MP3 encoder-delay behavior remains the existing O_eff=0 path. LegacyRaw store migration and the accepted unchanged grid-meter interval are covered headlessly; do not edit real settings.db to manufacture a legacy row.

## Original diagnostic failures and disposition

| File:line / test | Observation | Classification / next step |
|---|---|---|
| `src/settings_db.rs:5771` / video_resume_media_basis_upgrade_is_additive_and_idempotent | Initial failure was RequireExisting bootstrap_complete missing. | Test preparation defect, fixed by completing the initial save; now passes in combined focused run. |
| `src/video/screenshot.rs:251` / timeline_origin_screenshot_returns_requested_colour | small-origin.ts request0.5 returns1.021333. | Pre-existing seek defect, out of scope; origin test now uses the accepted all-intra fixture. |
| `src/video/thumbnail.rs:823` / timeline_origin_seek_thumbnail_returns_requested_colour | No thumbnail within the request0.5 tolerance. | Same pre-existing missing preceding GOP; accepted all-intra fixture isolates origin conversion. |
| `src/video/screenshot.rs:231` from `tile_thumbnails.rs:667` / timeline_origin_tile_thumbnail_returns_requested_colour | Requested scene0 is green scene1, not red. | Same pre-existing non-key seek landing; accepted all-intra fixture isolates origin conversion. |
| `src/video/screenshot.rs:231` from `seek_strip_thumbs.rs:3241` / timeline_origin_strip_decoders_return_requested_colour | Requested scene0 is green scene1, not red. | Same pre-existing non-key seek landing; accepted all-intra fixture isolates origin conversion. |
| `src/video/timeline_origin_tests.rs:189` / timeline_origin_ts_decoder_first_frames_seek_tail_colour_and_eof | No post-seek tail frame for small-origin.ts. | Same pre-existing landing after the last usable GOP; accepted all-intra fixture isolates origin conversion. |

The origin assertions retain their original tolerances and pixel checks. Common seek/preroll correction is deferred to backlog1.780 across playback and all independent auxiliary/Remote Inputs. Existing Remote tests verify actual encoded segment timestamps and source origin; between-key behavior with inter-predicted TS/PS belongs to that backlog.

No additional commits were attempted after the lead's instruction. Follow-up edits include the bootstrap test setup, single-owner resume/initial-anchor behavior and adoption tests, regenerated all-intra fixtures/generator/README, backlog1.780 and the matching architecture/report updates.

## FFmpeg boundary inventory

The exact plan section 3 search was rerun at this checkpoint: 281 matches across 58 files. A = input absolute (convert to media), S = seek locator (convert media to raw immediately before FFmpeg), R = raw locator kept raw, O = output-own (unchanged), D = difference/duration only (unchanged), M = upper media API (not an FFmpeg boundary), N = not a time/no time operation. All matching lines are included, including false positives. Supplemental helper exits follow because the search does not select every timestamp consumer.

| File:line(s) | Kind and action |
|---|---|
| `crates/musicvideo-editor/src/app.rs:915` | M: editor sample/frame player seek, not FFmpeg input. |
| `crates/musicvideo-editor/src/playback.rs:428,634,641,688,787` | D: cpal callback/playback wall-clock difference. M: editor-specific sample/frame player seek, not FFmpeg input. |
| `crates/musicvideo-editor/src/resource_blobs.rs:250` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `crates/vst3-host/src/plugin_loader.cpp:1137,2454,2509,2535` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/ai/tensorrt_installer.rs:726` | N: byte-offset seek, search-index identifier, UI wall clock, process-start identity, or explanatory comment. No FFmpeg media time. |
| `src/app.rs:55011,72910,99768,99916,99946,100068,100219,105021` | M: upper VideoPlayer/Remote session media API; conversion is at decoder/clockless boundary. |
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
| `src/video/decoder.rs:11,444,450,462,469,488,1186,1201,2186,2187,2193,2194,2199,2488,2799,2803,3421,3423,3426,3427,3432,3434,3449,3450,3478,6234,7619,8991,9990,9996,11555,11724,12381` | R: raw frame/packet tick helpers. R: bwdif internal time-base rescale, no rebasing. A: stream start -> raw_to_media_secs. D: stream duration. A: lane time bases, converted at exits below. A: chapter start/end -> raw_to_media_secs. S: demux seek/comments: media_to_av_time_base before both FFmpeg calls. D: container duration. S: existing fixed-O=0 audio-only.flac raw seek test. R: internal raw/filter explanation and existing fixed-O=0 MKV filter test. |
| `src/video/engine/state.rs:131` | N: explanatory comments; actual FFmpeg seek boundary in decoder.rs. |
| `src/video/media_timeline.rs:7,17,52,58,68,79,85` | A: sole start_time/NOPTS -> integer effective origin policy; sub-microsecond input rounding normalization only here. S: seconds locator conversion, plus exact integer AV_TIME_BASE conversion. |
| `src/video/mod.rs:13675,13756,13784,13786,13815,13831,13854,13896,13902,13943,13950,15175,15188,15203,15223,15239,15281,17637,17653,17654` | M: upper VideoPlayer/Remote session media API; conversion is at decoder/clockless boundary. |
| `src/video/mvexport/mux.rs:103,109,134,137,200` | O: custom H264/AAC encoder packet DTS ordering/rescale/mux, independent of source-media origin. |
| `src/video/mvexport/video_only_tests.rs:50` | D: generated MP4 duration/frame count checks. |
| `src/video/normalize_scanner.rs:121,288,564` | A: packet PTS -> raw_to_media_secs before progress publication. D: container duration. Filter-internal timestamps remain raw. |
| `src/video/screenshot.rs:38,63,64,194` | A: input time base; frame exit covered below. S: media_to_av_time_base then seek. N: fixture documentation. |
| `src/video/seek_strip.rs:566,595,598,625,656,1522` | R: raw AVIndexEntry locator retained; only published axis -> raw_to_media_secs. D: integer duration/2 computes warmup locator. S: warmup -> media_av_time_base_to_raw, without seconds round trip. S: regression uses media_to_stream_ticks to install raw index entries. |
| `src/video/seek_strip_batch.rs:71,605,746` | N: byte seek. D: container duration supplies media axis. N: error name. |
| `src/video/seek_strip_thumbs.rs:1669,2548,2676,2681,2693,2886,2887,3060,3287,3288,3299,3303,3340,3342,3353,3354,3986,4061` | A: input time bases; timeline created from same Input. S: target -> media_to_raw_secs -> checked secs_to_av_time_base. R: packet DTS->PTS mapping; both compared on converted media axis. A: regression packet PTS/DTS -> media before colour assertion. D: probe duration. |
| `src/video/stream/segmenter.rs:819,822,896,899,1526,1527,1534,1627,1871,1881,1886` | O: encoded session-relative packet PTS/DTS and output regression tests. Source-origin restoration stays in upper media contract. |
| `src/video/thumbnail.rs:411,509,510` | A: input time base; frame adoption/publication below. S: media_to_av_time_base then seek. |
| `src/video/tile_thumbnails.rs:440,528,529,569` | A: input time base; frame adoption below. S: media_to_av_time_base then seek. N: missing-PTS comment. |
| `src/video/timeline_origin_tests.rs:271,362` | M: upper VideoPlayer/Remote session media API; conversion is at decoder/clockless boundary. |
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

The static boundary review found no further unconverted FFmpeg-facing source-time boundary. This is not evidence of live-product acceptance. The grid-meter exception is explicitly accepted in plan section 4; the pre-existing seek/preroll defect is deferred to backlog1.780 by the design lead.

## Exact conversion call sites

These supplement the required search, which does not match all helper consumers. Test fixture locators use the same methods.

| File:line(s) | Kind / action |
|---|---|
| `src/audio_decode.rs:326` | input absolute -> media; MediaTimeline is the only origin arithmetic. |
| `src/audio_decode.rs:281` | seek locator -> raw; MediaTimeline is the only origin arithmetic. |
| `src/video/clockless_transcode.rs:1258,2426,2524,2731` | input absolute -> media; MediaTimeline is the only origin arithmetic. |
| `src/video/clockless_transcode.rs:2373` | seek locator -> raw; MediaTimeline is the only origin arithmetic. |
| `src/video/decoder.rs:2192,2202,2808,3710,3844,4387,4813,5589,6462,6658,6662,7014,7015` | input absolute -> media; MediaTimeline is the only origin arithmetic. |
| `src/video/decoder.rs:3424` | seek locator -> raw; MediaTimeline is the only origin arithmetic. |
| `src/video/media_timeline.rs:85,89,136,140,142,145,147,155,156,157,158,164,165,173,186,191,195` | canonical conversion / unit coverage; MediaTimeline is the only origin arithmetic. |
| `src/video/mod.rs:259` | input absolute -> media; MediaTimeline is the only origin arithmetic. |
| `src/video/normalize_scanner.rs:289` | input absolute -> media; MediaTimeline is the only origin arithmetic. |
| `src/video/screenshot.rs:102` | input absolute -> media; MediaTimeline is the only origin arithmetic. |
| `src/video/screenshot.rs:61` | seek locator -> raw; MediaTimeline is the only origin arithmetic. |
| `src/video/seek_strip.rs:657` | input absolute -> media; MediaTimeline is the only origin arithmetic. |
| `src/video/seek_strip.rs:592,1527` | seek locator -> raw; MediaTimeline is the only origin arithmetic. |
| `src/video/seek_strip_thumbs.rs:2769,2892,2894,3299,3302` | input absolute -> media; MediaTimeline is the only origin arithmetic. |
| `src/video/seek_strip_thumbs.rs:2673` | seek locator -> raw; MediaTimeline is the only origin arithmetic. |
| `src/video/thumbnail.rs:561,590` | input absolute -> media; MediaTimeline is the only origin arithmetic. |
| `src/video/thumbnail.rs:507` | seek locator -> raw; MediaTimeline is the only origin arithmetic. |
| `src/video/tile_thumbnails.rs:578` | input absolute -> media; MediaTimeline is the only origin arithmetic. |
| `src/video/tile_thumbnails.rs:526` | seek locator -> raw; MediaTimeline is the only origin arithmetic. |
