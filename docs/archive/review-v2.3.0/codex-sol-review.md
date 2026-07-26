# GPT-5.6-Sol 独立レビュー (2026-07-10、新モデル初回)

対象: 複数ウィンドウ周り全体 (未コミット差分 + detached アーキテクチャ)。新セッション・先入観なし。

## 初回レビュー (P1=0 / P2=3)

No P1 findings. I found three P2 issues.

- [P2] [src/app.rs:42458](C:/home/mimageviewer/src/app.rs:42458) — The new success toast is emitted when a native placement request is merely queued, not when `PlacementSwitched` confirms it; if presenter reconstruction times out or fails, the UI says it switched to the main window while playback remains detached.

- [P2] [src/tag_ops.rs:305](C:/home/mimageviewer/src/tag_ops.rs:305) — `tag_toast_label` and `tag_toast_surface` are single global slots for an asynchronously queued worker, so a viewer tag operation followed by a main-grid tag operation before the worker becomes idle overwrites routing/label state and reports the combined completion on the wrong surface with the wrong tag name. Completion consumes the overwritten slots at [src/tag_ops.rs:729](C:/home/mimageviewer/src/tag_ops.rs:729).

- [P2] [src/app.rs:29645](C:/home/mimageviewer/src/app.rs:29645) — Reopening the currently active detached audio item returns “handled” but neither focuses its egui media window nor raises a native presenter: the focus branch covers detached stills and the raise branch only `GridItem::Video`, leaving `GridItem::Audio` unchanged and apparently unresponsive.

The examined park/promote paths, third-state routing predicates, filename-stack aggregation gate, and synchronous-I/O exposure otherwise looked internally consistent. I found no newly introduced UI-thread filesystem I/O in the reviewed paths.
## 修正ラウンド 1 (トースト確定化 / 面混在フォールバック / Audio focus)

One issue remains:

- [P2] [src/tag_ops.rs:501](C:/home/mimageviewer/src/tag_ops.rs:501) — `pending_tag_undos.is_empty()` is not a reliable in-flight proxy. `clear_meta_undo()` deliberately clears it while tag worker jobs continue ([src/undo_ops.rs:135](C:/home/mimageviewer/src/undo_ops.rs:135)). Scenario: start a Viewer tag batch → navigate/open fullscreen, clearing pending undo state → start a MainWindow batch while the first worker remains busy; the helper treats the queue as idle and routes the combined completion only to MainWindow. The proxy should derive from the tag worker/counter lifecycle, such as `tag_write_handle.is_busy()` or the unreset batch counters.

The other two fixes are clean:

- All production `NativeVideoModeSwitchPending` constructors initialize `announce_main_hint: false`.
- Only the verified newly armed F12 request sets it true.
- Exact-match and converged-target confirmation arms both consume the pending entry and conditionally announce.
- Timeout and mismatched-target paths do not announce.
- Audio now follows the correct egui viewport focus path.

`git diff --check` passed. The requested test compiled but `cargo test ... --lib` ran zero tests because this test resides in the binary test target.
## 修正ラウンド 2 (is_busy proxy)

One issue remains:

- [P2] [src/tag_ops.rs:506](C:/home/mimageviewer/src/tag_ops.rs:506) — `TagWriteHandle::is_busy()` becomes false before the previous batch is consumed by the UI. The worker increments `done` before sending the result ([src/tag_write_worker.rs:237](C:/home/mimageviewer/src/tag_write_worker.rs:237)); moreover, native-video events can enqueue a new Viewer batch before `poll_tag_write_results` runs that frame. The helper can therefore treat an unconsumed MainWindow batch as complete, overwrite the surface with `Viewer`, and later route the combined completion only there. Use the unreset batch lifecycle (`handle.total > 0`, reset only after polling completion) rather than `is_busy()` as the prior-batch proxy.

All requested binary-target tests passed:

- `media_window_main_hint_fires_on_placement_switch_confirmation`
- `always_new_media_f12_is_temporary_and_does_not_change_default_open_mode`
- Four `fullfeature_media_window*` tests

`git diff --check` also passed.
## 修正ラウンド 3 (has_unconsumed_batch) — クリーン

Clean. No remaining issues found.

`has_unconsumed_batch()` correctly spans queued work, the done-before-send window, and completed-but-unpolled results; counters reset only after completion aggregation. All tag submission paths call the surface helper before submitting new jobs.

Verification passed:

- `git diff --check`
- 2 `tag_write_worker` tests
- `media_window_main_hint_fires_on_placement_switch_confirmation`
- `always_new_media_f12_is_temporary_and_does_not_change_default_open_mode`