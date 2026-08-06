import test from "node:test";
import assert from "node:assert/strict";

import {
  VideoGenerationSwitchOwner,
  VIDEO_PANEL_TABS,
  VideoSeekPreviewOwner,
  VideoStreamViewer,
  hlsBufferConfig,
  preventVideoNativeZoom,
  resolveVideoPlaylist,
  videoAudioProcessingPresentation,
  videoEndDecision,
  videoUserErrorMessage,
} from "./video-stream.mjs";

test("blocked remote session stops playback once and prevents another poll", () => {
  const calls = [];
  const viewer = {
    remoteSessionState: { blocksInteraction: false },
    remoteSessionResume: null,
    destroyed: false,
    playRequested: true,
    currentPosition: () => 42.5,
    clearPoll: () => calls.push("clear_poll"),
    generationSwitch: { cancel: () => calls.push("cancel_switch") },
    seekThumbnailAbort: { abort: () => calls.push("abort_thumbnail") },
    clearWaiting: () => calls.push("clear_waiting"),
    stopPlaylistPlayback: () => calls.push("stop_playback"),
  };
  VideoStreamViewer.prototype.applyRemoteSessionState.call(viewer, {
    blocksInteraction: true,
  });
  assert.deepEqual(viewer.remoteSessionResume, {
    positionSecs: 42.5,
    restorePlaying: true,
  });
  assert.deepEqual(calls, [
    "clear_poll",
    "cancel_switch",
    "abort_thumbnail",
    "clear_waiting",
    "stop_playback",
  ]);
  VideoStreamViewer.prototype.applyRemoteSessionState.call(viewer, {
    blocksInteraction: true,
  });
  assert.equal(calls.length, 5);

  let cleared = false;
  VideoStreamViewer.prototype.schedulePoll.call({
    clearPoll: () => { cleared = true; },
    destroyed: false,
    remoteSessionState: { blocksInteraction: true },
    session: 7,
  });
  assert.equal(cleared, true);
});

test("video reconnect resumes the captured position and play intent", async () => {
  const restarted = [];
  const viewer = {
    remoteSessionResume: { positionSecs: 42.5, restorePlaying: false },
    remoteSessionState: { blocksInteraction: false },
    destroyed: false,
    restartAt: async (...args) => restarted.push(args),
  };
  assert.equal(
    await VideoStreamViewer.prototype.resumeAfterRemoteSessionReconnect.call(viewer),
    true
  );
  assert.deepEqual(restarted, [[42.5, false]]);
  assert.equal(viewer.remoteSessionResume, null);
});

test("stage 4 video panel exposes functions and jump tabs in that order", () => {
  assert.deepEqual(VIDEO_PANEL_TABS, [
    { id: "functions", label: "機能" },
    { id: "jump", label: "ジャンプ" },
  ]);
});

test("VST3 processing failures remain visible while the video keeps a playable status", () => {
  assert.deepEqual(videoAudioProcessingPresentation({
    vst3_requested: true,
    vst3_active: false,
    vst3_active_slots: 0,
    vst3_warning: "VST3 を適用できないため、配信を継続しています。",
  }), {
    requested: true,
    active: false,
    activeSlots: 0,
    warning: "VST3 を適用できないため、配信を継続しています。",
    detail: "VST3 未適用",
  });
  assert.equal(videoAudioProcessingPresentation({
    vst3_requested: true,
    vst3_active: true,
    vst3_active_slots: 5,
  }).detail, "VST3 5");
});

test("video EOF follows the PC continuous OFF and loop OFF stop rule", () => {
  assert.deepEqual(videoEndDecision({
    behavior: { kind: "stop" },
    positionSecs: 284.5,
    ended: true,
  }), { kind: "stop" });
});

test("video EOF advances or wraps for the two PC continuous modes", () => {
  assert.deepEqual(videoEndDecision({
    behavior: { kind: "next", wrap: false },
    positionSecs: 284.5,
    ended: true,
  }), { kind: "next", wrap: false });
  assert.deepEqual(videoEndDecision({
    behavior: { kind: "next", wrap: true },
    positionSecs: 284.5,
    ended: true,
  }), { kind: "next", wrap: true });
});

test("whole and section loop policies return to the PC-owned interval start", () => {
  assert.deepEqual(videoEndDecision({
    behavior: { kind: "loop", boundary_starts_secs: [0] },
    positionSecs: 284.5,
    ended: true,
  }), { kind: "loop", positionSecs: 0 });
  assert.deepEqual(videoEndDecision({
    behavior: { kind: "loop", boundary_starts_secs: [12, 42.5, 90] },
    previousPositionSecs: 42.49,
    positionSecs: 42.51,
  }), { kind: "loop", positionSecs: 12 });
  assert.deepEqual(videoEndDecision({
    behavior: { kind: "loop", boundary_starts_secs: [12, 42.5, 90] },
    previousPositionSecs: 42.49,
    positionSecs: 42.51,
    playing: false,
  }), { kind: "continue" });
  assert.deepEqual(videoEndDecision({
    behavior: { kind: "loop", boundary_starts_secs: [12, 42.5, 90] },
    previousPositionSecs: 89.99,
    positionSecs: 90,
    ended: true,
  }), { kind: "loop", positionSecs: 42.5 });
});

test("hls.js starts at the requested generation origin and uses every buffer limit", () => {
  assert.deepEqual(hlsBufferConfig(60), {
    backBufferLength: 60,
    maxBufferLength: 60,
    maxMaxBufferLength: 60,
    startPosition: 0,
    startOnSegmentBoundary: true,
  });
});

test("seek preview advances from seeking through decoded thumbnail to playback", () => {
  const owner = new VideoSeekPreviewOwner({ matchToleranceSecs: 1 });
  const request = owner.request(42.5);
  assert.equal(owner.current().kind, "seeking");

  assert.equal(owner.acceptThumbnail(request, {
    actualPtsSecs: 42.466,
    objectUrl: "blob:thumbnail",
    width: 320,
    height: 180,
  }), true);
  assert.deepEqual(owner.current(), {
    kind: "thumbnail",
    revision: request.revision,
    targetSecs: 42.5,
    label: "シーク中",
    actualPtsSecs: 42.466,
    objectUrl: "blob:thumbnail",
    width: 320,
    height: 180,
  });

  assert.equal(owner.playbackStarted(), true);
  assert.deepEqual(owner.current(), { kind: "playback" });
});

test("seek target remains the displayed position until replacement playback lands", () => {
  const owner = new VideoSeekPreviewOwner();
  assert.equal(owner.displayedPosition(18), 18);
  owner.request(72.5, "seeking");
  assert.equal(owner.displayedPosition(18.2), 72.5);
  owner.acceptThumbnail(owner.current(), {
    actualPtsSecs: 72.466,
    objectUrl: "blob:thumbnail",
  });
  assert.equal(owner.displayedPosition(18.4), 72.5);
  owner.playbackStarted();
  assert.equal(owner.displayedPosition(72.6), 72.6);
});

test("repeated relative seek previews accumulate from the latest requested position", () => {
  const owner = new VideoSeekPreviewOwner();
  const first = owner.requestRelative(42.774, -10, 284.5);
  const second = owner.requestRelative(43.012, -10, 284.5);
  const third = owner.requestRelative(43.251, -10, 284.5);

  assert.ok(Math.abs(first.targetSecs - 32.774) < 1e-9);
  assert.ok(Math.abs(second.targetSecs - 22.774) < 1e-9);
  assert.ok(Math.abs(third.targetSecs - 12.774) < 1e-9);
  assert.ok(Math.abs(owner.displayedPosition(43.5) - 12.774) < 1e-9);
});

test("relative seek preview returns to the landed playback position", () => {
  const owner = new VideoSeekPreviewOwner();
  const request = owner.requestRelative(42.774, -10, 284.5);

  assert.ok(Math.abs(owner.displayedPosition(43) - 32.774) < 1e-9);
  assert.equal(owner.bindGeneration(request, 8), true);
  assert.equal(owner.playbackGenerationStarted(7), false);
  assert.ok(Math.abs(owner.displayedPosition(43.1) - 32.774) < 1e-9);
  assert.equal(owner.playbackGenerationStarted(8), true);
  assert.equal(owner.displayedPosition(32.8), 32.8);
});

test("failed relative seek does not strand or clear a newer requested position", () => {
  const owner = new VideoSeekPreviewOwner();
  const stale = owner.requestRelative(42.774, -10, 284.5);
  const latest = owner.requestRelative(43, -10, 284.5);

  assert.equal(owner.requestFailed(stale), false);
  assert.ok(Math.abs(owner.displayedPosition(43.2) - 22.774) < 1e-9);
  assert.equal(owner.requestFailed(latest), true);
  assert.equal(owner.displayedPosition(43.2), 43.2);
});

test("video surface consumes native zoom gestures but preserves interactive controls", () => {
  let prevented = 0;
  const surfaceEvent = {
    cancelable: true,
    target: { closest: () => null },
    preventDefault: () => { prevented += 1; },
  };
  assert.equal(preventVideoNativeZoom(surfaceEvent), true);
  assert.equal(prevented, 1);
  assert.equal(preventVideoNativeZoom({
    ...surfaceEvent,
    target: { closest: () => ({ tagName: "BUTTON" }) },
  }), false);
  assert.equal(preventVideoNativeZoom({ ...surfaceEvent, cancelable: false }), false);
  assert.equal(prevented, 1);
});

test("playback is never held for a missing seek thumbnail", () => {
  const owner = new VideoSeekPreviewOwner();
  owner.request(90);
  assert.equal(owner.playbackStarted(), true);
  assert.equal(owner.current().kind, "playback");
});

test("seek drag keeps only the latest thumbnail request and rejects stale or wrong PTS", () => {
  const owner = new VideoSeekPreviewOwner({ matchToleranceSecs: 0.5 });
  const stale = owner.request(10, "移動先を確認中");
  const latest = owner.request(35, "移動先を確認中");

  assert.equal(owner.acceptThumbnail(stale, {
    actualPtsSecs: 10,
    objectUrl: "blob:stale",
  }), false);
  assert.equal(owner.acceptThumbnail(latest, {
    actualPtsSecs: 32,
    objectUrl: "blob:wrong-position",
  }), false);
  assert.equal(owner.acceptThumbnail(latest, {
    actualPtsSecs: 35.033,
    objectUrl: "blob:latest",
  }), true);
  assert.equal(owner.current().actualPtsSecs, 35.033);
});

const jsonResponse = (status, body) => new Response(JSON.stringify(body), {
  status,
  headers: {
    "Content-Type": "application/json",
    "Retry-After": "1",
  },
});

test("playlist 409 refreshes generation from state and recovers", async () => {
  const requested = [];
  let stateCalls = 0;
  const result = await resolveVideoPlaylist({
    initialUrl: "/stream/2/1/index.m3u8",
    session: 2,
    fetchPlaylist: async (url) => {
      requested.push(url);
      return requested.length === 1
        ? jsonResponse(409, { error: "stream_generation_mismatch" })
        : new Response("#EXTM3U", { status: 200 });
    },
    fetchState: async () => {
      stateCalls += 1;
      return { session: 2, generation: 3 };
    },
    delay: async () => {},
  });

  assert.equal(result.ok, true);
  assert.equal(result.url, "/stream/2/3/index.m3u8");
  assert.deepEqual(requested, [
    "/stream/2/1/index.m3u8",
    "/stream/2/3/index.m3u8",
  ]);
  assert.equal(stateCalls, 1);
});

test("four seconds of playlist 503 remains waiting inside the recovery budget", async () => {
  let nowMs = 0;
  let playlistCalls = 0;
  const result = await resolveVideoPlaylist({
    initialUrl: "/stream/1/1/index.m3u8",
    session: 1,
    timeoutMs: 15000,
    now: () => nowMs,
    fetchPlaylist: async () => {
      playlistCalls += 1;
      return nowMs < 4000
        ? jsonResponse(503, { error: "stream_not_ready" })
        : new Response("#EXTM3U", { status: 200 });
    },
    fetchState: async () => ({ session: 1, generation: 1 }),
    delay: async (delayMs) => { nowMs += delayMs; },
  });

  assert.equal(result.ok, true);
  assert.equal(playlistCalls, 5);
  assert.equal(nowMs, 5000);
});

test("playlist recovery fails only when its elapsed-time budget is exhausted", async () => {
  let nowMs = 0;
  let playlistCalls = 0;
  const decisions = [];
  const result = await resolveVideoPlaylist({
    initialUrl: "/stream/1/1/index.m3u8",
    session: 1,
    timeoutMs: 4500,
    now: () => nowMs,
    fetchPlaylist: async () => {
      playlistCalls += 1;
      return jsonResponse(503, { error: "stream_not_ready" });
    },
    fetchState: async () => ({ session: 1, generation: 1 }),
    delay: async (delayMs) => { nowMs += delayMs; },
    onDecision: (decision) => decisions.push(decision.kind),
  });

  assert.equal(result.ok, false);
  assert.equal(result.decision.kind, "playlist_recovery_exhausted");
  assert.equal(result.decision.internalReason, "playlist_recovery_budget_exhausted");
  assert.equal(result.decision.message, "動画の再生を続けられませんでした。");
  assert.equal(result.decision.elapsedMs, 4500);
  assert.equal(nowMs, 4500);
  assert.equal(playlistCalls, 4);
  assert.deepEqual(decisions, ["waiting", "waiting", "waiting", "waiting"]);
});

test("same-generation switch requests share one recovery loop", async () => {
  let runs = 0;
  let release;
  const owner = new VideoGenerationSwitchOwner({
    stopCurrent: () => {},
    runSwitch: async () => {
      runs += 1;
      return new Promise((resolve) => { release = resolve; });
    },
  });
  const target = { session: 4, generation: 5, url: "/stream/4/5/index.m3u8" };

  const first = owner.request(target);
  await Promise.resolve();
  const second = owner.request(target);

  assert.strictEqual(second, first);
  assert.equal(runs, 1);
  assert.equal(owner.attachedTarget(), null);
  release(true);
  assert.equal(await first, true);
  assert.equal(owner.attachedTarget().generation, 5);
  assert.equal(owner.currentTarget().generation, 5);
});

test("newer generation replaces and aborts the older recovery loop", async () => {
  const operations = [];
  const owner = new VideoGenerationSwitchOwner({
    stopCurrent: () => {},
    runSwitch: async (operation) => new Promise((resolve) => {
      operations.push({ operation, resolve });
      operation.signal.addEventListener("abort", () => resolve(false), { once: true });
    }),
  });

  const older = owner.request({
    session: 4,
    generation: 5,
    url: "/stream/4/5/index.m3u8",
  });
  await Promise.resolve();
  const newer = owner.request({
    session: 4,
    generation: 6,
    url: "/stream/4/6/index.m3u8",
  });
  await Promise.resolve();
  const stale = owner.request({
    session: 4,
    generation: 5,
    url: "/stream/4/5/index.m3u8",
  });

  assert.notStrictEqual(newer, older);
  assert.strictEqual(stale, newer);
  assert.equal(operations.length, 2);
  assert.equal(operations[0].operation.signal.aborted, true);
  assert.equal(await older, false);
  operations[1].resolve(true);
  assert.equal(await newer, true);
  assert.equal(owner.currentTarget().generation, 6);
});

test("generation switch silences the old HLS instance before recovery starts", async () => {
  let recoveryStarted = false;
  let release;
  const oldHls = {
    destroyed: false,
    destroy() { this.destroyed = true; },
  };
  const owner = new VideoGenerationSwitchOwner({
    stopCurrent: () => oldHls.destroy(),
    runSwitch: async () => {
      recoveryStarted = true;
      return new Promise((resolve) => { release = resolve; });
    },
  });

  const switched = owner.request({
    session: 4,
    generation: 5,
    url: "/stream/4/5/index.m3u8",
  });

  assert.equal(oldHls.destroyed, true);
  assert.equal(recoveryStarted, false);
  await Promise.resolve();
  assert.equal(recoveryStarted, true);
  release(true);
  assert.equal(await switched, true);
});

test("generation switch reports failure when its one owner budget expires", async () => {
  let deadline;
  let exhausted = 0;
  let activeOperation;
  const owner = new VideoGenerationSwitchOwner({
    stopCurrent: () => {},
    runSwitch: async (operation) => {
      activeOperation = operation;
      return new Promise((_resolve, reject) => {
        operation.signal.addEventListener("abort", () => {
          const error = new Error("aborted");
          error.name = "AbortError";
          reject(error);
        }, { once: true });
      });
    },
    onBudgetExhausted: () => { exhausted += 1; },
    setTimer: (callback) => {
      deadline = callback;
      return 1;
    },
    clearTimer: () => {},
  });

  const switched = owner.request({
    session: 4,
    generation: 5,
    url: "/stream/4/5/index.m3u8",
  });
  await Promise.resolve();
  assert.equal(exhausted, 0);

  deadline();

  assert.equal(await switched, false);
  assert.equal(activeOperation.abortReason, "budget");
  assert.equal(exhausted, 1);
  assert.equal(owner.currentTarget(), null);
});

test("user video errors hide internal stage details and keep code-based guidance", () => {
  const error = new Error("要求された動画の player が 2 秒以内に準備できませんでした");
  error.code = "stream_start_seek_timeout";
  const message = videoUserErrorMessage(error, "動画を操作できませんでした");

  assert.equal(message, "動画を開始できませんでした。もう一度お試しください。");
  for (const term of ["player", "seek", "2 秒", "予算", "内部状態", "状態が一致"]) {
    assert.equal(message.includes(term), false, term);
  }
  assert.equal(
    videoUserErrorMessage(
      { code: "stream_session_mismatch", message: "動画配信の状態が一致しません" },
      "動画を操作できませんでした"
    ),
    "動画の配信が終了しました。もう一度開いてください。"
  );
});
