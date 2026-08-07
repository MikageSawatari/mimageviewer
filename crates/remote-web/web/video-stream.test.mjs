import test from "node:test";
import assert from "node:assert/strict";

import {
  VideoGenerationSwitchOwner,
  VIDEO_PANEL_TABS,
  VideoPlaybackControlState,
  VideoSeekPreviewOwner,
  VideoStreamViewer,
  hlsBufferConfig,
  hlsErrorTelemetryDetails,
  hlsFragmentLoadMetrics,
  preventVideoNativeZoom,
  resolveVideoPlaylist,
  videoAudioProcessingPresentation,
  videoEndDecision,
  videoPlayRejectionDecision,
  videoPlaybackControlTransition,
  videoPlaybackStallDecision,
  videoHealthSample,
  videoHealthSamplingDecision,
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
    clearHealthTelemetry: () => calls.push("clear_health"),
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
    "clear_health",
    "cancel_switch",
    "abort_thumbnail",
    "clear_waiting",
    "stop_playback",
  ]);
  VideoStreamViewer.prototype.applyRemoteSessionState.call(viewer, {
    blocksInteraction: true,
  });
  assert.equal(calls.length, 6);

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

test("video start ends a busy attempt visibly instead of retrying forever", async () => {
  const notices = [];
  const failures = [];
  let startRequests = 0;
  const busy = Object.assign(new Error("busy"), {
    status: 503,
    code: "stream_busy",
  });
  const viewer = {
    playbackControlState: VideoPlaybackControlState.PLAY_REQUESTED,
    destroyed: false,
    address: { favorite_id: "favorite", relative_path: "movie.mp4" },
    quality: "standard",
    abortController: { signal: new AbortController().signal },
    clearPoll: () => {},
    clearHealthTelemetry: () => {},
    rememberPlaybackError: () => {},
    showNotice: (...args) => notices.push(args),
    apiPostJson: async () => {
      startRequests += 1;
      throw busy;
    },
    requestWithWaiting: () => {
      throw new Error("start must not enter the unbounded waiting loop");
    },
    showStartFailure: (error) => failures.push(error),
    transitionPlaybackControl(event) {
      return VideoStreamViewer.prototype.transitionPlaybackControl.call(this, event);
    },
  };

  await VideoStreamViewer.prototype.start.call(viewer);

  assert.equal(startRequests, 1);
  assert.deepEqual(notices, [["動画を準備しています。", "waiting"]]);
  assert.deepEqual(failures, [busy]);
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

test("hls error telemetry keeps typed playback facts and drops URLs and exception text", () => {
  const ranges = {
    length: 1,
    start: () => 10,
    end: () => 42.5,
  };
  const details = hlsErrorTelemetryDetails({
    type: "mediaError",
    details: "bufferAppendError",
    fatal: true,
    error: {
      name: "QuotaExceededError",
      message: "secret https://example.test/stream/session-id/media.m3u8?t=token",
    },
    frag: {
      sn: 91,
      type: "main",
      url: "https://example.test/private/movie.m4s",
    },
  }, {
    currentTime: 40,
    readyState: 2,
    networkState: 2,
    error: { code: 3 },
    buffered: ranges,
    seekable: ranges,
  });

  assert.deepEqual(details, {
    hls_error_type: "mediaError",
    hls_error_details: "bufferAppendError",
    fatal: true,
    http_status: null,
    error_name: "QuotaExceededError",
    fragment_type: "main",
    fragment_sequence: 91,
    ready_state: 2,
    network_state: 2,
    media_error_code: 3,
    current_time_secs: 40,
    buffered_range_count: 1,
    buffered_ahead_secs: 2.5,
    seekable_range_count: 1,
  });
  assert.doesNotMatch(JSON.stringify(details), /example|private|secret|session-id|token/);
});

test("playback issue reports omit the remote session credential", () => {
  const reports = [];
  const viewer = {
    session: "remote-session-secret",
    generation: 6,
    reportPlaybackIssue: (report) => reports.push(report),
  };

  VideoStreamViewer.prototype.recordPlaybackIssue.call(
    viewer,
    "video_stream_hls_fatal",
    "hls_error_event",
    { hls_error_details: "bufferAppendError" }
  );

  assert.deepEqual(reports, [{
    category: "video_stream_hls_fatal",
    internalReason: "hls_error_event",
    generation: 6,
    hls_error_details: "bufferAppendError",
  }]);
  assert.doesNotMatch(JSON.stringify(reports), /remote-session-secret/);
});

test("video health samples distinguish buffer starvation from a stopped playback layer", () => {
  const ranges = {
    length: 2,
    start: (index) => index === 0 ? 10 : 80,
    end: (index) => index === 0 ? 54.25 : 90,
  };
  const sample = videoHealthSample({
    trigger: "periodic",
    video: {
      currentTime: 50,
      readyState: 2,
      networkState: 2,
      errorCode: 0,
      paused: false,
      ended: false,
      buffered: ranges,
      playbackQuality: { droppedVideoFrames: 7, totalVideoFrames: 1200 },
    },
    sourcePositionSecs: 350,
    playRequested: true,
    playbackMode: "hls_js",
    generation: 6,
    hls: {
      bandwidthEstimate: 5_432_100,
      loadingEnabled: false,
      bufferingEnabled: false,
    },
    fragment: { load_ms: 82.34, sequence: 117 },
    connection: { effectiveType: "4g", rtt: 51, downlink: 8.25 },
    waiting: true,
    playbackAttempts: {
      attempts: 2,
      successes: 1,
      rejections: 1,
      pending: 0,
      lastRejectionName: "NotAllowedError",
    },
  });

  assert.deepEqual(sample, {
    type: "video_health",
    trigger: "periodic",
    current_time_secs: 50,
    source_position_secs: 350,
    buffer_ahead_secs: 4.25,
    buffered_range_count: 2,
    ready_state: 2,
    network_state: 2,
    media_error_code: 0,
    paused: false,
    ended: false,
    play_requested: true,
    waiting: true,
    playback_mode: "hls_js",
    generation: 6,
    dropped_video_frames: 7,
    total_video_frames: 1200,
    hls_bandwidth_bps: 5_432_100,
    hls_loading_enabled: false,
    hls_buffering_enabled: false,
    last_fragment_load_ms: 82.3,
    last_fragment_sequence: 117,
    connection_effective_type: "4g",
    connection_rtt_ms: 51,
    connection_downlink_mbps: 8.25,
    play_attempt_count: 2,
    play_success_count: 1,
    play_rejection_count: 1,
    play_pending_count: 0,
    last_play_rejection_name: "NotAllowedError",
  });
});

test("video health normal tier has no path while debug context adds a bounded remote address", () => {
  const base = {
    video: { buffered: { length: 0 } },
    detailedContext: {
      enabled: true,
      address: {
        favorite_id: "favorite-1",
        relative_path: "private/movie.mp4",
        subresource: { kind: "file" },
      },
      serverMessage: "server detail",
      diagnosticMessage: "decoder detail",
    },
  };
  const detailed = videoHealthSample(base);
  assert.equal(detailed.remote_address.relative_path, "private/movie.mp4");
  assert.equal(detailed.server_message, "server detail");
  assert.equal(detailed.diagnostic_message, "decoder detail");
  const normal = videoHealthSample({
    ...base,
    detailedContext: { ...base.detailedContext, enabled: false },
  });
  assert.equal(normal.remote_address, undefined);
  assert.equal(normal.server_message, undefined);
  assert.doesNotMatch(JSON.stringify(normal), /private|server detail|decoder detail/);
});

test("video health periodic eligibility follows play intent, not media time progress", () => {
  assert.equal(videoHealthSamplingDecision({
    session: 6,
    playRequested: true,
  }), true);
  assert.equal(videoHealthSamplingDecision({
    session: 6,
    playRequested: true,
    blocked: true,
  }), false);
  assert.equal(videoHealthSamplingDecision({
    session: 6,
    playRequested: false,
  }), false);
  assert.equal(videoHealthSamplingDecision({
    session: null,
    playRequested: true,
  }), false);
});

test("video health timer emits every ten seconds even when playback time is unchanged", () => {
  const scheduled = [];
  const originalSetTimeout = globalThis.setTimeout;
  const originalClearTimeout = globalThis.clearTimeout;
  globalThis.setTimeout = (callback, delay) => {
    scheduled.push({ callback, delay });
    return scheduled.length;
  };
  globalThis.clearTimeout = () => {};
  try {
    const captures = [];
    const viewer = {
      destroyed: false,
      session: 6,
      playRequested: true,
      remoteSessionState: { blocksInteraction: false },
      healthTelemetryTimer: 0,
      clearHealthTelemetry: () => { viewer.healthTelemetryTimer = 0; },
      captureVideoHealth: (...args) => captures.push(args),
    };
    viewer.syncHealthTelemetry = () =>
      VideoStreamViewer.prototype.syncHealthTelemetry.call(viewer);

    viewer.syncHealthTelemetry();
    assert.equal(scheduled[0].delay, 10_000);
    scheduled[0].callback();
    assert.deepEqual(captures, [["periodic", { telemetry: true }]]);
    assert.equal(scheduled[1].delay, 10_000, "unchanged playback must schedule the next sample");
  } finally {
    globalThis.setTimeout = originalSetTimeout;
    globalThis.clearTimeout = originalClearTimeout;
  }
});

test("fragment load metrics use hls loading timestamps without copying its URL", () => {
  const metrics = hlsFragmentLoadMetrics({
    frag: { sn: 117, url: "https://example.test/private/117.m4s" },
    stats: { loading: { start: 100.25, end: 184.75 } },
  });
  assert.deepEqual(metrics, { load_ms: 84.5, sequence: 117 });
  assert.doesNotMatch(JSON.stringify(metrics), /example|private/);
});

test("runtime waiting has one bounded terminal instead of an infinite buffering notice", () => {
  assert.deepEqual(videoPlaybackStallDecision({
    active: true,
    playRequested: true,
    progressedMediaSecs: 0.25,
  }), { kind: "resolved" });
  assert.equal(videoPlaybackStallDecision({
    active: true,
    playRequested: true,
    progressedMediaSecs: 0.249,
  }).kind, "waiting");
  assert.deepEqual(videoPlaybackStallDecision({
    active: true,
    playRequested: true,
    elapsedSinceProgressMs: 14999,
    timeoutMs: 15000,
  }), { kind: "waiting", retryDelayMs: 1 });
  assert.deepEqual(videoPlaybackStallDecision({
    active: true,
    playRequested: true,
    elapsedSinceProgressMs: 15000,
    timeoutMs: 15000,
  }), {
    kind: "stalled",
    internalReason: "playback_progress_timeout",
    timeoutMs: 15000,
  });
  assert.equal(videoPlaybackStallDecision({
    active: true,
    playRequested: false,
    elapsedSinceProgressMs: 20000,
  }).kind, "cancel");
  assert.equal(videoPlaybackStallDecision({
    active: true,
    playRequested: true,
    awaitingUserActivation: true,
    elapsedSinceProgressMs: 20000,
  }).kind, "cancel");
  assert.equal(videoPlaybackStallDecision({
    active: true,
    playRequested: true,
    hidden: true,
    elapsedSinceProgressMs: 20000,
  }).kind, "defer");
  assert.equal(videoPlaybackStallDecision({
    active: true,
    playRequested: true,
    switching: true,
    elapsedSinceProgressMs: 20000,
  }).kind, "defer");
});

test("media play rejection names have distinct recovery owners", () => {
  assert.deepEqual(videoPlayRejectionDecision("NotAllowedError"), {
    kind: "user_activation_required",
  });
  assert.deepEqual(videoPlayRejectionDecision("AbortError"), {
    kind: "interrupted",
  });
  assert.deepEqual(videoPlayRejectionDecision("NotSupportedError"), {
    kind: "failed",
  });
});

test("a paused stalled event cannot create a waiting owner without play intent", () => {
  const viewer = {
    destroyed: false,
    playbackStallWatch: null,
    playRequested: false,
    generationSwitch: { isSwitching: () => false },
    video: { currentTime: 0, ended: false, paused: true },
    remoteSessionState: { blocksInteraction: false },
  };

  VideoStreamViewer.prototype.beginWaiting.call(viewer, "stalled");
  assert.equal(viewer.playbackStallWatch, null);
});

test("stable playback progress resolves the waiting owner and its notice", () => {
  const now = performance.now();
  const watch = {
    generation: 8,
    startedAt: now - 1000,
    lastProgressAt: now - 1000,
    lastMediaTimeSecs: 10,
    progressedMediaSecs: 0,
    trigger: "waiting",
    warningTimer: 1,
    terminalTimer: 1,
  };
  const calls = [];
  const viewer = {
    destroyed: false,
    playbackStallWatch: watch,
    generation: 8,
    playRequested: true,
    video: { currentTime: 10.03, paused: false, ended: false },
    remoteSessionState: { blocksInteraction: false },
    generationSwitch: { isSwitching: () => false },
    clearWaiting() {
      calls.push("clear_waiting");
      this.playbackStallWatch = null;
    },
    schedulePlaybackStallCheck: () => calls.push("schedule"),
  };
  viewer.advancePlaybackStallWatch = (owner) =>
    VideoStreamViewer.prototype.advancePlaybackStallWatch.call(viewer, owner);

  VideoStreamViewer.prototype.notePlaybackProgress.call(viewer);
  assert.deepEqual(calls, []);
  viewer.video.currentTime = 10.12;
  VideoStreamViewer.prototype.notePlaybackProgress.call(viewer);
  assert.deepEqual(calls, ["schedule"]);
  viewer.video.currentTime = 10.27;
  VideoStreamViewer.prototype.notePlaybackProgress.call(viewer);

  assert.deepEqual(calls, ["schedule", "clear_waiting"]);
  assert.equal(viewer.playbackStallWatch, null);
});

test("autoplay wait survives playback observations until an explicit play succeeds", () => {
  let state = videoPlaybackControlTransition(
    VideoPlaybackControlState.PLAY_REQUESTED,
    { type: "play_rejected_user_activation" }
  );
  assert.equal(state, VideoPlaybackControlState.USER_ACTIVATION_REQUIRED);

  for (const event of [
    { type: "media_playing" },
    { type: "request_play" },
    { type: "synchronize_intent", playRequested: true },
    { type: "play_succeeded", userInitiated: false },
  ]) {
    state = videoPlaybackControlTransition(state, event);
    assert.equal(state, VideoPlaybackControlState.USER_ACTIVATION_REQUIRED);
  }

  assert.equal(
    videoPlaybackControlTransition(state, {
      type: "play_succeeded",
      userInitiated: true,
    }),
    VideoPlaybackControlState.PLAY_REQUESTED
  );
  assert.equal(
    videoPlaybackControlTransition(state, { type: "request_pause" }),
    VideoPlaybackControlState.STOPPED
  );
});

test("a native playing event cannot dismiss the autoplay activation notice", () => {
  const calls = [];
  const viewer = {
    playbackControlState: VideoPlaybackControlState.USER_ACTIVATION_REQUIRED,
    noticeKind: "autoplay",
    transitionPlaybackControl(event) {
      return VideoStreamViewer.prototype.transitionPlaybackControl.call(this, event);
    },
    checkPlaybackStartupProgress: () => calls.push("check_startup"),
    hideNotice: () => calls.push("hide_notice"),
    clearWaiting: () => calls.push("clear_waiting"),
    finishSeekPreviewForAttachedGeneration: () => calls.push("finish_preview"),
    captureVideoHealth: (trigger) => calls.push(["health", trigger]),
  };
  Object.defineProperty(viewer, "awaitingUserActivation", {
    get() {
      return this.playbackControlState === VideoPlaybackControlState.USER_ACTIVATION_REQUIRED;
    },
  });

  VideoStreamViewer.prototype.handleMediaPlaying.call(viewer);

  assert.equal(
    viewer.playbackControlState,
    VideoPlaybackControlState.USER_ACTIVATION_REQUIRED
  );
  assert.deepEqual(calls, ["check_startup", ["health", "hud"]]);
});

test("an autoplay rejection waits for one explicit tap and leaves stall monitoring", async () => {
  const rejection = new Error("gesture required");
  rejection.name = "NotAllowedError";
  const calls = [];
  let noticeAction;
  let allowPlayback = false;
  const viewer = {
    destroyed: false,
    remoteSessionState: { blocksInteraction: false },
    video: {
      src: "blob:stream",
      paused: true,
      play: () => {
        calls.push("play");
        return allowPlayback ? Promise.resolve() : Promise.reject(rejection);
      },
    },
    hls: null,
    playbackControlState: VideoPlaybackControlState.PLAY_REQUESTED,
    playbackAttempts: {
      attempts: 0,
      successes: 0,
      rejections: 0,
      pending: 0,
      lastRejectionName: "",
    },
    rememberPlaybackError: (error) => calls.push(["remember", error.name]),
    playbackLayerDiagnostics: () => ({ paused: true, ready_state: 4 }),
    recordPlaybackIssue: (...args) => calls.push(["issue", ...args]),
    captureVideoHealth: (...args) => calls.push(["health", ...args]),
    clearWaiting: () => calls.push("clear_waiting"),
    showNotice: (message, kind, actionLabel, action) => {
      calls.push(["notice", message, kind, actionLabel]);
      noticeAction = action;
    },
    hideNotice: () => calls.push("hide_notice"),
    noticeKind: "autoplay",
  };
  Object.defineProperties(viewer, {
    playRequested: {
      get() {
        return this.playbackControlState !== VideoPlaybackControlState.STOPPED;
      },
    },
    awaitingUserActivation: {
      get() {
        return this.playbackControlState === VideoPlaybackControlState.USER_ACTIVATION_REQUIRED;
      },
    },
  });
  viewer.transitionPlaybackControl = (event) =>
    VideoStreamViewer.prototype.transitionPlaybackControl.call(viewer, event);
  viewer.playIfRequested = (options) =>
    VideoStreamViewer.prototype.playIfRequested.call(viewer, options);

  await VideoStreamViewer.prototype.playIfRequested.call(viewer);

  assert.equal(calls.filter((entry) => entry === "play").length, 1);
  assert.deepEqual(viewer.playbackAttempts, {
    attempts: 1,
    successes: 0,
    rejections: 1,
    pending: 0,
    lastRejectionName: "NotAllowedError",
  });
  assert.equal(
    viewer.playbackControlState,
    VideoPlaybackControlState.USER_ACTIVATION_REQUIRED
  );
  assert.deepEqual(calls.slice(-3), [
    ["health", "play_rejected", { telemetry: true }],
    "clear_waiting",
    ["notice", "自動再生が制限されています。", "autoplay", "タップして再生"],
  ]);

  await VideoStreamViewer.prototype.playIfRequested.call(viewer);
  assert.equal(calls.filter((entry) => entry === "play").length, 1);

  allowPlayback = true;
  await noticeAction();
  assert.equal(calls.filter((entry) => entry === "play").length, 2);
  assert.equal(viewer.playbackControlState, VideoPlaybackControlState.PLAY_REQUESTED);
  assert.equal(viewer.playbackAttempts.successes, 1);
  assert.equal(calls.at(-2), "hide_notice");
  assert.deepEqual(calls.at(-1), ["health", "hud"]);
});

test("an interrupted play promise does not masquerade as an autoplay block", async () => {
  const rejection = new Error("source changed");
  rejection.name = "AbortError";
  const calls = [];
  const viewer = {
    destroyed: false,
    remoteSessionState: { blocksInteraction: false },
    video: {
      src: "blob:stream",
      play: () => Promise.reject(rejection),
    },
    hls: null,
    playbackControlState: VideoPlaybackControlState.PLAY_REQUESTED,
    playbackAttempts: {
      attempts: 0,
      successes: 0,
      rejections: 0,
      pending: 0,
      lastRejectionName: "",
    },
    rememberPlaybackError: () => {},
    playbackLayerDiagnostics: () => ({}),
    recordPlaybackIssue: () => {},
    captureVideoHealth: () => {},
    showNotice: () => calls.push("notice"),
    finishPlaybackLayerFailure: () => calls.push("terminal"),
  };
  Object.defineProperties(viewer, {
    playRequested: {
      get() {
        return this.playbackControlState !== VideoPlaybackControlState.STOPPED;
      },
    },
    awaitingUserActivation: {
      get() {
        return this.playbackControlState === VideoPlaybackControlState.USER_ACTIVATION_REQUIRED;
      },
    },
  });
  viewer.transitionPlaybackControl = (event) =>
    VideoStreamViewer.prototype.transitionPlaybackControl.call(viewer, event);

  await VideoStreamViewer.prototype.playIfRequested.call(viewer);

  assert.equal(viewer.playbackControlState, VideoPlaybackControlState.PLAY_REQUESTED);
  assert.deepEqual(calls, []);
});

test("terminal playback failure clears the waiting owner before showing reconnect", () => {
  const calls = [];
  let reconnect;
  const viewer = {
    playRequested: true,
    currentPosition: () => 42.5,
    clearPlaybackStartupWatch: () => calls.push("clear_startup"),
    clearWaiting: () => calls.push("clear_waiting"),
    hls: { stopLoad: () => calls.push("stop_load") },
    video: { pause: () => calls.push("pause") },
    showNotice: (message, kind, actionLabel, action) => {
      calls.push([message, kind, actionLabel]);
      reconnect = action;
    },
    restartAt: (...args) => calls.push(["restart", ...args]),
  };

  VideoStreamViewer.prototype.finishPlaybackLayerFailure.call(
    viewer,
    "動画の再生が停止しました。"
  );
  assert.deepEqual(calls, [
    "clear_startup",
    "clear_waiting",
    "stop_load",
    "pause",
    ["動画の再生が停止しました。", "error", "現在位置から再接続"],
  ]);
  reconnect();
  assert.deepEqual(calls.at(-1), ["restart", 42.5, true]);
});

test("fatal hls errors are recorded before entering the reconnect terminal", () => {
  const calls = [];
  const source = {};
  const viewer = {
    destroyed: false,
    hls: source,
    video: {
      currentTime: 42.5,
      readyState: 2,
      networkState: 2,
      buffered: { length: 0 },
      seekable: { length: 0 },
    },
    hlsErrorTelemetryAt: new Map(),
    playbackLayerDiagnostics: () => ({ hls_loading_enabled: false }),
    rememberPlaybackError: () => {},
    captureVideoHealth: (...args) => calls.push(["health", ...args]),
    recordPlaybackIssue: (...args) => calls.push(["telemetry", ...args]),
    clearPlaybackStartupWatch: () => calls.push(["clear_startup"]),
    finishPlaybackLayerFailure: (message) => calls.push(["terminal", message]),
  };

  VideoStreamViewer.prototype.onHlsError.call(viewer, source, {
    type: "mediaError",
    details: "bufferAppendError",
    fatal: true,
    error: { name: "QuotaExceededError" },
  });

  assert.equal(calls[0][0], "telemetry");
  assert.equal(calls[0][1], "video_stream_hls_fatal");
  assert.equal(calls[0][3].hls_error_details, "bufferAppendError");
  assert.deepEqual(calls.slice(1), [
    ["health", "hls_fatal", { telemetry: true }],
    ["clear_startup"],
    ["terminal", "動画を再生できませんでした。もう一度お試しください。"],
  ]);
});

test("an attached visible playback with no progress reaches the reconnect terminal", () => {
  const now = performance.now();
  const watch = {
    generation: 8,
    startedAt: now - 16000,
    lastProgressAt: now - 15001,
    trigger: "waiting",
    terminalTimer: 1,
  };
  const calls = [];
  const viewer = {
    destroyed: false,
    playbackStallWatch: watch,
    generation: 8,
    playRequested: true,
    video: { ended: false },
    remoteSessionState: { blocksInteraction: false },
    generationSwitch: { isSwitching: () => false },
    playbackLayerDiagnostics: () => ({ hls_loading_enabled: false }),
    recordPlaybackIssue: (...args) => calls.push(["telemetry", ...args]),
    captureVideoHealth: (...args) => calls.push(["health", ...args]),
    finishPlaybackLayerFailure: (message) => calls.push(["terminal", message]),
  };
  viewer.advancePlaybackStallWatch = (owner) =>
    VideoStreamViewer.prototype.advancePlaybackStallWatch.call(viewer, owner);

  VideoStreamViewer.prototype.checkPlaybackStall.call(viewer, watch);

  assert.equal(calls[0][0], "telemetry");
  assert.equal(calls[0][1], "video_stream_playback_stalled");
  assert.equal(calls[0][2], "playback_progress_timeout");
  assert.equal(calls[0][3].hls_loading_enabled, false);
  assert.deepEqual(calls[1], ["health", "stall_terminal", { telemetry: true }]);
  assert.deepEqual(calls[2], [
    "terminal",
    "動画の再生が停止しました。現在位置から再接続できます。",
  ]);
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
