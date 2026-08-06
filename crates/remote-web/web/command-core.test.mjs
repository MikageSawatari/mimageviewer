import test from "node:test";
import assert from "node:assert/strict";

import {
  CommandName,
  FitMode,
  GRID_VIEWPORT_MEMORY_LIMIT,
  GridViewportAnchor,
  GridViewportMemory,
  ReadingDirection,
  SpreadMode,
  ViewerGesture,
  ViewerPanelAction,
  ViewerPanelOrientation,
  IMAGE_QUALITY_PRESETS,
  VIDEO_QUALITY_PRESETS,
  bufferingQualitySuggestion,
  clampGridColumnOverride,
  commandFromKey,
  createReadingProgressBatch,
  gridColumnOverrideFieldForViewport,
  gridColumnOverrideForViewport,
  gridColumnsAfterPinch,
  gridLabelHeightForEntries,
  gridLayoutForWidth,
  gridScrollExtent,
  gridIndexForCommand,
  isPortraitViewport,
  isRtlReadingDirection,
  isRtlSpread,
  imageQualityPreset,
  nextSpreadMode,
  readingDirectionForSpreadMode,
  readingProgressBatchTransition,
  remoteSessionAcquireDecision,
  remoteSessionAcquireRetryDelay,
  remoteSessionControlTransition,
  remoteSessionFailureStatus,
  remoteSessionTransitionTelemetry,
  remoteStateGenerationTransition,
  reduceViewerTransform,
  resolveGridReturnViewport,
  togglePageOriginalFitMode,
  viewerTapCommand,
  viewerTapSequenceTransition,
  viewerTapZone,
  nextFitMode,
  pagePrefetchPlan,
  pageResponseGenerationAttestation,
  planSpreadIntent,
  snappedGridOffset,
  thumbnailBindingMatches,
  thumbnailRequestConcurrency,
  thumbnailRequestStartCount,
  thumbnailRetryDecision,
  telemetryDeliveryMode,
  telemetryEventForTier,
  telemetrySessionCorrelation,
  shouldShowGridCursor,
  shouldShowLoadingIndicator,
  shouldShowKeyboardShortcuts,
  sessionOwnerBadge,
  viewerImageLayout,
  viewerPageDisplaySlot,
  viewerPageGroupGenerationSnapshot,
  viewerSpreadPartnerIndex,
  viewerBoundaryMessage,
  viewerGestureDecision,
  viewerPanelGestureAction,
  viewerPanelLayout,
  viewerPanelShellTransition,
  viewerPanelTransition,
  viewerResizePlan,
  viewerVerticalScrollDecision,
  viewerSeekGroupIndex,
  viewerSeekRelativeDragValue,
  viewerSeekState,
  viewerSpreadLayout,
  viewerWheelCommand,
  videoAbsoluteSeekCommand,
  videoHttpStatusDecision,
  videoPlaybackDecision,
  videoQualityPreset,
  videoSeekPlan,
  videoStartSeekTarget,
  videoStartupDecision,
  videoTapCommand,
  appUpdateNotice,
  adjustmentResetVisible,
  rangeValueFromNormalized,
  rangeValueToNormalized,
  relativeRangeDragValue,
  shouldReanchorVideoTimeline,
  videoTimelineAnchor,
  videoTimelinePosition,
} from "./command-core.mjs";

const key = (value, extra = {}) => ({ key: value, ...extra });

test("relative range drag follows travel, clamps, and snaps independently of press position", () => {
  const drag = (startClientX, currentClientX) => relativeRangeDragValue({
    startValue: 0,
    startClientX,
    currentClientX,
    trackWidth: 200,
    min: -100,
    max: 100,
    step: 1,
  });
  assert.equal(drag(10, 30), 20);
  assert.equal(drag(150, 170), 20);
  assert.equal(drag(90, 90), 0);
  assert.equal(relativeRangeDragValue({
    startValue: 1,
    startClientX: 20,
    currentClientX: 31,
    trackWidth: 100,
    min: 0.2,
    max: 5,
    step: 0.01,
    logarithmic: true,
  }), 1.42);
  assert.equal(relativeRangeDragValue({
    startValue: 250,
    startClientX: 0,
    currentClientX: 100,
    trackWidth: 100,
    min: 0,
    max: 254,
    step: 1,
  }), 254);
});

test("positive logarithmic adjustment ranges match egui normalized positions", () => {
  assert.equal(rangeValueToNormalized({
    value: 1,
    min: 0.2,
    max: 5,
    logarithmic: true,
  }), 0.5);
  assert.equal(rangeValueFromNormalized({
    normalized: 0.5,
    min: 0.2,
    max: 5,
    step: 0.01,
    logarithmic: true,
  }), 1);
  assert.equal(rangeValueToNormalized({
    value: 1,
    min: 0.1,
    max: 10,
    logarithmic: true,
  }), 0.5);
  assert.equal(rangeValueFromNormalized({
    normalized: 0,
    min: 0.2,
    max: 5,
    logarithmic: true,
  }), 0.2);
  assert.equal(rangeValueFromNormalized({
    normalized: 1,
    min: 0.2,
    max: 5,
    logarithmic: true,
  }), 5);
  assert.equal(rangeValueToNormalized({ value: 0, min: -100, max: 100 }), 0.5);
});

test("adjustment reset visibility follows defaults, epsilon, and disabled state", () => {
  assert.equal(adjustmentResetVisible({ value: 0, defaultValue: 0 }), false);
  assert.equal(adjustmentResetVisible({ value: 2, defaultValue: 0 }), true);
  assert.equal(adjustmentResetVisible({ value: 2, defaultValue: 0, disabled: true }), false);
  assert.equal(adjustmentResetVisible({ value: 1.0005, defaultValue: 1, epsilon: 0.001 }), false);
  assert.equal(adjustmentResetVisible({ value: 1.002, defaultValue: 1, epsilon: 0.001 }), true);
});

test("session acquisition policy separates passive detection from explicit recovery", () => {
  assert.equal(remoteSessionAcquireDecision("active", "user_operation"), "use_current");
  assert.equal(remoteSessionAcquireDecision("inactive", "initial"), "acquire");
  assert.equal(remoteSessionAcquireDecision("not_acquired", "user_operation"), "acquire");
  assert.equal(remoteSessionAcquireDecision("expired", "user_operation"), "acquire");
  assert.equal(remoteSessionAcquireDecision("expired", "passive"), "blocked");
  assert.equal(remoteSessionAcquireDecision("local_in_use", "user_operation"), "blocked");
  assert.equal(remoteSessionAcquireDecision("other_device", "user_operation"), "blocked");
  assert.equal(remoteSessionAcquireDecision("local_in_use", "explicit_reconnect"), "acquire");
  assert.equal(remoteSessionAcquireDecision("other_device", "explicit_reconnect"), "acquire");
});

test("one acquisition intent keeps waiting for a draining owner without a fixed deadline", () => {
  assert.equal(remoteSessionAcquireRetryDelay(409, "local_in_use", 0), 250);
  assert.equal(remoteSessionAcquireRetryDelay(409, "local_in_use", 8), 500);
  assert.equal(remoteSessionAcquireRetryDelay(409, "local_in_use", 16), 1000);
  assert.equal(remoteSessionAcquireRetryDelay(409, "local_in_use", 80), 1000);
  assert.equal(remoteSessionAcquireRetryDelay(409, "superseded", 0), null);
  assert.equal(remoteSessionAcquireRetryDelay(428, "local_in_use", 0), null);
});

test("session control blocks only explicit ownership revocation", () => {
  const active = remoteSessionControlTransition(null, "active", "");
  const localDisconnect = remoteSessionControlTransition(
    active,
    "local_in_use",
    "本体が操作を取り戻しました。"
  );
  assert.deepEqual(localDisconnect, {
    status: "local_in_use",
    message: "本体が操作を取り戻しました。",
    phase: "disconnected",
    blocksInteraction: true,
    disconnectReason: "local_in_use",
  });
  assert.equal(
    remoteSessionControlTransition(active, "other_device", "").blocksInteraction,
    true
  );
  assert.equal(
    remoteSessionControlTransition(active, "expired", "").blocksInteraction,
    false
  );
  assert.equal(
    remoteSessionControlTransition(active, "not_acquired", "").blocksInteraction,
    false
  );
  assert.equal(
    remoteSessionControlTransition(active, "unavailable", "").blocksInteraction,
    false
  );
});

test("a host-disconnect response becomes one blocking control transition", () => {
  const active = remoteSessionControlTransition(null, "active", "");
  const status = remoteSessionFailureStatus({
    sessionStatus: "local_in_use",
    httpStatus: 409,
  });
  const disconnected = remoteSessionControlTransition(
    active,
    status,
    "本体で切断されました。再接続してください。"
  );

  assert.equal(status, "local_in_use");
  assert.equal(disconnected.status, "local_in_use");
  assert.equal(disconnected.blocksInteraction, true);
  assert.equal(disconnected.disconnectReason, "local_in_use");
});

test("session transition telemetry records the observer before UI side effects", () => {
  const active = remoteSessionControlTransition(null, "active", "");
  const disconnected = remoteSessionControlTransition(
    active,
    "local_in_use",
    "本体で切断されました。再接続してください。"
  );
  assert.deepEqual(
    remoteSessionTransitionTelemetry(active, disconnected, {
      observer: "video_poll",
      observedStatus: "local_in_use",
      httpStatus: 409,
      message: "must not be copied",
      sessionId: "must-not-be-copied",
    }),
    {
      type: "remote_session",
      action: "control_transition",
      observer: "video_poll",
      observed_status: "local_in_use",
      http_status: 409,
      from_status: "active",
      from_phase: "active",
      from_blocks_interaction: false,
      to_status: "local_in_use",
      to_phase: "disconnected",
      to_blocks_interaction: true,
      disconnect_reason: "local_in_use",
    }
  );
  assert.equal(
    remoteSessionTransitionTelemetry(disconnected, disconnected, {
      observer: "video_poll",
      observedStatus: "local_in_use",
      httpStatus: 409,
    }),
    null,
    "repeated poll failures must not create immediate telemetry traffic"
  );
});

test("session transition telemetry rejects unbounded observation values", () => {
  const inactive = remoteSessionControlTransition(null, "inactive", "");
  const acquiring = remoteSessionControlTransition(inactive, "acquiring", "");
  const event = remoteSessionTransitionTelemetry(inactive, acquiring, {
    observer: "/api/private/path?token=secret",
    observedStatus: "secret-status",
    httpStatus: 999,
  });
  assert.equal(event.observer, "client");
  assert.equal(event.observed_status, "unknown");
  assert.equal(event.http_status, null);
  assert.doesNotMatch(JSON.stringify(event), /private|secret|token/);
});

test("uncaught JS, terminal playback failures and session transitions are immediate", () => {
  assert.equal(telemetryDeliveryMode({
    type: "error",
    category: "window_error",
  }), "immediate");
  assert.equal(telemetryDeliveryMode({
    type: "error",
    category: "unhandled_rejection",
  }), "immediate");
  assert.equal(telemetryDeliveryMode({
    type: "remote_session",
    action: "control_transition",
  }), "immediate");
  for (const category of [
    "video_stream_hls_fatal",
    "video_stream_media_element_error",
    "video_stream_playback_stalled",
  ]) {
    assert.equal(telemetryDeliveryMode({ type: "error", category }), "immediate");
  }
  assert.equal(telemetryDeliveryMode({
    type: "error",
    category: "fetch_non_2xx",
  }), "batch");
  assert.equal(telemetryDeliveryMode({
    type: "error",
    category: "image_load_error",
  }), "batch");
  assert.equal(telemetryDeliveryMode({
    type: "error",
    category: "video_stream_hls_error",
  }), "batch");
  assert.equal(telemetryDeliveryMode({
    type: "remote_session",
    action: "acquire",
  }), "batch");
  assert.equal(telemetryDeliveryMode({
    type: "video_health",
    trigger: "periodic",
  }), "batch");
  assert.equal(telemetryDeliveryMode({
    type: "video_health",
    trigger: "waiting_threshold",
  }), "immediate");
  assert.equal(telemetryDeliveryMode({
    type: "video_health",
    trigger: "play_rejected",
  }), "immediate");
});

test("normal telemetry keeps health facts but removes path, message and identity fields", () => {
  const event = telemetryEventForTier({
    type: "video_health",
    trigger: "periodic",
    current_time_secs: 42.5,
    buffer_ahead_secs: 8.25,
    ready_state: 2,
    message: "server message C:/private/movie.mp4",
    stack: "at C:/private/app.js:10",
    resource: "/private/movie.mp4",
    remote_address: { favorite_id: "fav", relative_path: "private/movie.mp4" },
    client_id: "browser-client-secret",
    remote_session_correlation: "0123456789abcdef01234567",
  });

  assert.deepEqual(event, {
    type: "video_health",
    trigger: "periodic",
    current_time_secs: 42.5,
    buffer_ahead_secs: 8.25,
    ready_state: 2,
    telemetry_tier: "normal",
  });
});

test("debug telemetry keeps diagnostic context but redacts the live session capability", () => {
  const rawSession = "0123456789abcdef0123456789abcdef";
  const event = telemetryEventForTier({
    type: "video_health",
    trigger: "periodic",
    remote_address: { favorite_id: "fav", relative_path: "movie.mp4" },
    server_message: `session ${rawSession} failed`,
  }, {
    detailed: true,
    clientId: "browser-client-1234",
    sessionCorrelation: "00112233445566778899aabb",
    sensitiveValues: [rawSession],
  });

  assert.equal(event.telemetry_tier, "debug");
  assert.equal(event.client_id, "browser-client-1234");
  assert.equal(event.remote_session_correlation, "00112233445566778899aabb");
  assert.equal(event.remote_address.relative_path, "movie.mp4");
  assert.equal(event.server_message, "session [redacted-secret] failed");
  assert.doesNotMatch(JSON.stringify(event), new RegExp(rawSession));
});

test("remote session correlation is a stable truncated SHA-256 derivative", async () => {
  const subtle = {
    async digest(algorithm, bytes) {
      assert.equal(algorithm, "SHA-256");
      assert.equal(new TextDecoder().decode(bytes), "remote-session");
      return Uint8Array.from({ length: 32 }, (_, index) => index).buffer;
    },
  };
  assert.equal(
    await telemetrySessionCorrelation("remote-session", subtle),
    "000102030405060708090a0b"
  );
  assert.equal(await telemetrySessionCorrelation("remote-session", null), "");
});

test("session failure detection ignores feature-level 409 responses", () => {
  assert.equal(remoteSessionFailureStatus({
    sessionStatus: "superseded",
    httpStatus: 409,
  }), "other_device");
  assert.equal(remoteSessionFailureStatus({
    errorCode: "session_required",
    httpStatus: 409,
  }), "local_in_use");
  assert.equal(remoteSessionFailureStatus({
    errorCode: "session_required",
    httpStatus: 428,
  }), "expired");
  assert.equal(remoteSessionFailureStatus({
    errorCode: "stream_session_mismatch",
    httpStatus: 409,
  }), null);
  assert.equal(remoteSessionFailureStatus({
    errorCode: "not_ready",
    httpStatus: 409,
  }), null);
  assert.equal(remoteSessionFailureStatus({
    errorCode: "session_closing",
    httpStatus: 409,
  }), null);
});

test("session control remains modal through reconnect until acquisition succeeds", () => {
  const disconnected = remoteSessionControlTransition(
    null,
    "other_device",
    "別の端末が接続しました。"
  );
  const reconnecting = remoteSessionControlTransition(
    disconnected,
    "acquiring",
    "操作権を取得しています…"
  );
  assert.deepEqual(reconnecting, {
    status: "acquiring",
    message: "操作権を取得しています…",
    phase: "reconnecting",
    blocksInteraction: true,
    disconnectReason: "other_device",
  });
  const failed = remoteSessionControlTransition(
    reconnecting,
    "unavailable",
    "通信できません。"
  );
  assert.equal(failed.blocksInteraction, true);
  assert.equal(failed.disconnectReason, "other_device");
  assert.equal(
    remoteSessionControlTransition(failed, "active", "").blocksInteraction,
    false
  );
});

test("session owner badge keeps active and superseded ownership explicit", () => {
  assert.deepEqual(sessionOwnerBadge("active"), {
    owner: "active",
    label: "操作中",
  });
  assert.deepEqual(sessionOwnerBadge("other_device"), {
    owner: "other_device",
    label: "別の端末が操作中",
  });
  assert.deepEqual(sessionOwnerBadge("acquiring"), sessionOwnerBadge("active"));
});

test("viewer keys map to the shared page and menu commands", () => {
  assert.equal(commandFromKey(key("ArrowRight"), "viewer").name, CommandName.NEXT_PAGE);
  assert.equal(commandFromKey(key("PageUp"), "viewer").name, CommandName.PREV_PAGE);
  assert.equal(commandFromKey(key("Backspace"), "viewer").name, CommandName.BACK);
  assert.equal(
    commandFromKey(key("?", { shiftKey: true }), "viewer").name,
    CommandName.TOGGLE_MENU
  );
  assert.equal(commandFromKey(key("F11"), "viewer").name, CommandName.TOGGLE_FULLSCREEN);
  assert.equal(commandFromKey(key("i"), "viewer"), null);
  assert.equal(commandFromKey(key("+"), "viewer").name, CommandName.ZOOM_IN);
  assert.equal(commandFromKey(key("-"), "viewer").name, CommandName.ZOOM_OUT);
  assert.equal(commandFromKey(key("0"), "viewer").name, CommandName.FIT_CYCLE);
  assert.equal(commandFromKey(key("1"), "viewer").name, CommandName.SPREAD_SINGLE);
  assert.equal(commandFromKey(key("5"), "viewer").name, CommandName.SPREAD_RTL_COVER);
});

test("media keys and tap zones map to the existing media command layer", () => {
  assert.deepEqual(commandFromKey(key(" "), "media"), {
    name: CommandName.MEDIA_TOGGLE_PLAY,
    payload: {},
  });
  assert.equal(
    commandFromKey(key("ArrowLeft"), "media").payload.seconds,
    -10
  );
  assert.equal(
    commandFromKey(key("ArrowRight"), "media").payload.seconds,
    10
  );
  assert.equal(commandFromKey(key("ArrowUp"), "media").name, CommandName.PREV_PAGE);
  assert.equal(commandFromKey(key("ArrowDown"), "media").name, CommandName.NEXT_PAGE);
  assert.equal(videoTapCommand(20, 300).payload.seconds, -10);
  assert.equal(videoTapCommand(150, 300).name, CommandName.MEDIA_TOGGLE_PLAY);
  assert.equal(videoTapCommand(280, 300).payload.seconds, 10);
});

test("MSE selects hls.js even when canPlayType reports maybe", () => {
  assert.deepEqual(videoPlaybackDecision({
    nativeHlsCanPlayType: "maybe",
    mediaSourceSupported: true,
  }), {
    mode: "hls_js",
    loadHlsJs: true,
  });
  assert.deepEqual(videoPlaybackDecision({
    nativeHlsCanPlayType: "probably",
    managedMediaSourceSupported: true,
  }), {
    mode: "hls_js",
    loadHlsJs: true,
  });
});

test("native HLS is the fallback when iOS has no MSE", () => {
  assert.deepEqual(videoPlaybackDecision({ nativeHlsCanPlayType: "probably" }), {
    mode: "native",
    loadHlsJs: false,
  });
  assert.deepEqual(videoPlaybackDecision({ nativeHlsCanPlayType: "maybe" }), {
    mode: "native",
    loadHlsJs: false,
  });
  assert.deepEqual(videoPlaybackDecision({
    nativeHlsCanPlayType: "probably",
    mediaSourceSupported: true,
    hlsJsSupported: false,
  }), {
    mode: "native",
    loadHlsJs: false,
  });
});

test("missing MSE and native HLS reports unsupported playback", () => {
  assert.deepEqual(videoPlaybackDecision({ nativeHlsCanPlayType: "" }), {
    mode: "unsupported",
    loadHlsJs: false,
    reason: "browser_has_no_supported_hls_playback_path",
  });
  assert.deepEqual(videoPlaybackDecision({
    nativeHlsCanPlayType: "",
    mediaSourceSupported: true,
    hlsJsSupported: false,
  }), {
    mode: "unsupported",
    loadHlsJs: false,
    reason: "browser_has_no_supported_hls_playback_path",
  });
});

test("startup detects a deadline with no fetched media segment", () => {
  assert.deepEqual(videoStartupDecision({
    mediaSegmentsLoaded: 0,
    readyState: 0,
    elapsedMs: 14999,
    timeoutMs: 15000,
  }), { kind: "waiting", remainingMs: 1 });
  assert.deepEqual(videoStartupDecision({
    mediaSegmentsLoaded: 0,
    readyState: 0,
    elapsedMs: 15000,
    timeoutMs: 15000,
  }), {
    kind: "no_media_segment",
    internalReason: "no_media_segment_loaded_before_deadline",
  });
  assert.deepEqual(videoStartupDecision({
    mediaSegmentsLoaded: 1,
    readyState: 0,
    elapsedMs: 15000,
    timeoutMs: 15000,
  }), { kind: "started" });
});

test("a newer build is announced once, and never interrupts on its own", () => {
  assert.deepEqual(
    appUpdateNotice({ runningToken: "a", servedToken: "a" }),
    { kind: "current" }
  );
  assert.deepEqual(
    appUpdateNotice({ runningToken: "a", servedToken: "b" }),
    { kind: "update_available", servedToken: "b" }
  );
  assert.deepEqual(
    appUpdateNotice({ runningToken: "a", servedToken: "b", dismissedToken: "b" }),
    { kind: "dismissed" },
    "同じ更新を閉じたら黙る"
  );
  assert.deepEqual(
    appUpdateNotice({ runningToken: "a", servedToken: "c", dismissedToken: "b" }),
    { kind: "update_available", servedToken: "c" },
    "さらに新しい版が出たら改めて知らせる"
  );
  // token が取れないとき (回線断・起動直後) に更新扱いしない。
  assert.deepEqual(appUpdateNotice({ runningToken: "", servedToken: "b" }), { kind: "current" });
  assert.deepEqual(appUpdateNotice({ runningToken: "a", servedToken: "" }), { kind: "current" });
});

test("the timeline is anchored once per generation, not on every state poll", () => {
  // generation edge は端末 playhead ではない。同じ generation の生成が進んでも
  // source origin は固定し、media element の currentTime だけで位置を進める。
  assert.equal(
    shouldReanchorVideoTimeline({ anchoredGeneration: null, stateGeneration: 7 }),
    true,
    "初回は基準点が無いので置く"
  );
  assert.equal(
    shouldReanchorVideoTimeline({ anchoredGeneration: 7, stateGeneration: 7 }),
    false,
    "同じ世代の poll では引き直さない"
  );
  assert.equal(
    shouldReanchorVideoTimeline({ anchoredGeneration: 7, stateGeneration: 8 }),
    true,
    "世代が変わったら置き直す"
  );

  const anchor = videoTimelineAnchor({
    sourceOriginSecs: 240,
    durationSecs: 600,
  });
  const jittered = videoTimelineAnchor({
    sourceOriginSecs: 240,
    generatedEndSecs: 302,
    durationSecs: 600,
  });
  assert.deepEqual(anchor, jittered, "生成端は timeline anchor に影響しない");
  assert.equal(
    videoTimelinePosition({
      anchorSourcePositionSecs: anchor.sourcePositionSecs,
      anchorMediaTimeSecs: anchor.mediaTimeSecs,
      mediaCurrentTimeSecs: 2,
      durationSecs: 600,
    }),
    242,
    "固定した基準点なら 2 秒進んだぶんだけ素直に進む"
  );
});

test("whole-video position composes source origin with the media element playhead", () => {
  const anchor = videoTimelineAnchor({
    sourceOriginSecs: 240,
    durationSecs: 600,
  });
  assert.deepEqual(anchor, { sourcePositionSecs: 240, mediaTimeSecs: 0 });
  assert.equal(videoTimelinePosition({
    anchorSourcePositionSecs: anchor.sourcePositionSecs,
    anchorMediaTimeSecs: anchor.mediaTimeSecs,
    mediaCurrentTimeSecs: 57,
    durationSecs: 600,
  }), 297);

  assert.deepEqual(videoSeekPlan({
    targetPositionSecs: 270,
    durationSecs: 600,
    anchorSourcePositionSecs: anchor.sourcePositionSecs,
    anchorMediaTimeSecs: anchor.mediaTimeSecs,
    seekableRanges: [[0, 60]],
  }), { kind: "local", positionSecs: 270, mediaTimeSecs: 30 });
  assert.deepEqual(videoSeekPlan({
    targetPositionSecs: 200,
    durationSecs: 600,
    anchorSourcePositionSecs: anchor.sourcePositionSecs,
    anchorMediaTimeSecs: anchor.mediaTimeSecs,
    seekableRanges: [[0, 60]],
  }), { kind: "remote", positionSecs: 200, mediaTimeSecs: null });
});

test("seek bar keeps its absolute target across delayed session acquisition", () => {
  const positionWhenReleased = 45;
  const positionWhenExecuted = 52;
  const targetPosition = 0;
  const oldRelativeResult = positionWhenExecuted + (targetPosition - positionWhenReleased);
  assert.equal(oldRelativeResult, 7, "the old relative command reproduced the mid-track restart");

  assert.deepEqual(videoAbsoluteSeekCommand(targetPosition), {
    name: CommandName.MEDIA_SEEK_ABSOLUTE,
    payload: { positionSecs: 0 },
  });
  assert.deepEqual(videoSeekPlan({
    targetPositionSecs: targetPosition,
    durationSecs: 600,
    anchorSourcePositionSecs: 0,
    anchorMediaTimeSecs: 0,
    seekableRanges: [[0, 60]],
  }), { kind: "local", positionSecs: 0, mediaTimeSecs: 0 });
});

test("explicit zero restart overrides saved resume while an initial open preserves it", () => {
  assert.equal(videoStartSeekTarget({
    requestedPositionSecs: null,
    sourceOriginSecs: 35.3,
    durationSecs: 180,
  }), null, "normal open preserves the core resume position");
  assert.equal(videoStartSeekTarget({
    requestedPositionSecs: 0,
    sourceOriginSecs: 35.3,
    durationSecs: 180,
  }), 0, "restartAt(0) must issue a server seek even though the value is zero");
  assert.equal(videoStartSeekTarget({
    requestedPositionSecs: 35.31,
    sourceOriginSecs: 35.3,
    durationSecs: 180,
  }), null, "a matching start origin does not create another generation");
});

test("three seconds of waiting only proposes one lower quality", () => {
  assert.equal(bufferingQualitySuggestion({
    waitingSinceMs: 1000,
    nowMs: 3999,
    quality: "standard",
  }), null);
  assert.equal(bufferingQualitySuggestion({
    waitingSinceMs: 1000,
    nowMs: 4000,
    quality: "standard",
  }).id, "low");
  assert.equal(bufferingQualitySuggestion({
    waitingSinceMs: 1000,
    nowMs: 5000,
    quality: "minimum",
  }), null);
});

test("stream HTTP statuses distinguish session and generation conflicts", () => {
  assert.deepEqual(videoHttpStatusDecision(503, 4), {
    kind: "waiting",
    retry: true,
    retryDelayMs: 4000,
    message: "配信の準備を待っています。",
  });
  assert.equal(videoHttpStatusDecision(410).kind, "gone");
  assert.equal(
    videoHttpStatusDecision(409, 1, "stream_generation_mismatch").kind,
    "generation_mismatch"
  );
  assert.equal(
    videoHttpStatusDecision(409, 1, "stream_session_mismatch").kind,
    "session_mismatch"
  );
  assert.equal(
    videoHttpStatusDecision(409, 1, "stream_session_mismatch").message,
    "動画の配信が終了しました。もう一度開いてください。"
  );
  assert.equal(
    videoHttpStatusDecision(409).message,
    "動画の配信が終了しました。もう一度開いてください。"
  );
  assert.equal(videoHttpStatusDecision(409).kind, "session_mismatch");
  assert.equal(videoHttpStatusDecision(404).kind, "not_found");
  assert.equal(
    videoHttpStatusDecision(500).message,
    "動画を読み込めませんでした。もう一度お試しください。"
  );
});

test("quality presets keep their traffic estimates attached to the command value", () => {
  assert.deepEqual(
    VIDEO_QUALITY_PRESETS.map(({ id, label, traffic }) => [id, label, traffic]),
    [
      ["minimum", "最小", "約 210 MB / 時"],
      ["low", "低", "約 400 MB / 時"],
      ["standard", "標準", "約 730 MB / 時"],
      ["high", "高", "約 1.4 GB / 時"],
    ]
  );
  assert.equal(videoQualityPreset("unknown").id, "standard");
});

test("image quality presets keep four deterministic long-side limits", () => {
  assert.deepEqual(
    IMAGE_QUALITY_PRESETS.map(({ id, label, maxLongSide }) => [id, label, maxLongSide]),
    [
      ["high", "高品質", 8192],
      ["standard", "標準", 4096],
      ["light", "軽量", 2048],
      ["minimum", "最軽量", 1024],
    ]
  );
  assert.equal(imageQualityPreset("high").maxLongSide, 8192);
  assert.equal(imageQualityPreset("unknown").id, "standard");
});

test("RTL reverses only physical horizontal viewer keys and tap zones", () => {
  assert.equal(
    commandFromKey(key("ArrowLeft", { rtl: true }), "viewer").name,
    CommandName.NEXT_PAGE
  );
  assert.equal(
    commandFromKey(key("ArrowRight", { rtl: true }), "viewer").name,
    CommandName.PREV_PAGE
  );
  assert.equal(commandFromKey(key("PageDown", { rtl: true }), "viewer").name, CommandName.NEXT_PAGE);
  assert.equal(viewerTapCommand(10, 300, true).name, CommandName.NEXT_PAGE);
  assert.equal(viewerTapCommand(290, 300, true).name, CommandName.PREV_PAGE);
});

test("spread cycle, RTL predicate and portrait fallback match the viewer contract", () => {
  assert.equal(nextSpreadMode(SpreadMode.SINGLE), SpreadMode.LTR);
  assert.equal(nextSpreadMode(SpreadMode.LTR), SpreadMode.LTR_COVER);
  assert.equal(nextSpreadMode(SpreadMode.LTR_COVER), SpreadMode.RTL);
  assert.equal(nextSpreadMode(SpreadMode.RTL), SpreadMode.RTL_COVER);
  assert.equal(nextSpreadMode(SpreadMode.RTL_COVER), SpreadMode.SINGLE);
  assert.equal(isRtlSpread(SpreadMode.RTL_COVER), true);
  assert.equal(isRtlSpread(SpreadMode.LTR), false);
  assert.equal(isRtlReadingDirection(ReadingDirection.RTL), true);
  assert.equal(isRtlReadingDirection(ReadingDirection.LTR), false);
  assert.equal(
    readingDirectionForSpreadMode(SpreadMode.SINGLE, ReadingDirection.RTL),
    ReadingDirection.RTL
  );
  assert.equal(
    readingDirectionForSpreadMode(SpreadMode.LTR_COVER, ReadingDirection.RTL),
    ReadingDirection.LTR
  );
  assert.equal(isPortraitViewport(430, 932), true);
  assert.equal(isPortraitViewport(932, 430), false);
  assert.equal(isPortraitViewport(800, 800), false);
});

test("opening a container in portrait plans no persistent spread write", () => {
  const plan = planSpreadIntent({
    currentDirection: ReadingDirection.RTL,
    portraitSinglePage: true,
    viewportWidth: 430,
    viewportHeight: 932,
  });
  assert.equal(plan.forceSinglePage, true);
  assert.equal(plan.writeRequest, null);
});

test("an explicit portrait selection writes the selected mode, not effective Single", () => {
  const address = {
    favorite_id: "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2",
    relative_path: "books/book.pdf",
    subresource: { kind: "file" },
  };
  const plan = planSpreadIntent({
    address,
    selectedMode: SpreadMode.RTL_COVER,
    currentDirection: ReadingDirection.LTR,
    portraitSinglePage: true,
    viewportWidth: 430,
    viewportHeight: 932,
  });
  assert.equal(plan.forceSinglePage, true);
  assert.deepEqual(plan.writeRequest, {
    kind: "set_spread",
    address,
    spread_mode: SpreadMode.RTL_COVER,
    reading_direction: ReadingDirection.RTL,
  });
});

test("fit mode cycle and request width use the actual rendered image width", () => {
  assert.equal(nextFitMode(FitMode.PAGE), FitMode.WIDTH);
  assert.equal(nextFitMode(FitMode.WIDTH), FitMode.ORIGINAL);
  assert.equal(nextFitMode(FitMode.ORIGINAL), FitMode.PAGE);
  const portrait = viewerImageLayout({
    mode: FitMode.PAGE,
    sourceWidth: 1000,
    sourceHeight: 2000,
    viewportWidth: 1200,
    viewportHeight: 800,
    devicePixelRatio: 2,
  });
  assert.equal(portrait.cssWidth, 400);
  assert.equal(portrait.requestWidth, 800);
  const widthFit = viewerImageLayout({
    mode: FitMode.WIDTH,
    sourceWidth: 1000,
    sourceHeight: 2000,
    viewportWidth: 1200,
    viewportHeight: 800,
    devicePixelRatio: 2,
  });
  assert.equal(widthFit.requestWidth, 2400);
  const original = viewerImageLayout({
    mode: FitMode.ORIGINAL,
    sourceWidth: 1000,
    sourceHeight: 2000,
    viewportWidth: 1200,
    viewportHeight: 800,
    devicePixelRatio: 2,
  });
  assert.equal(original.cssWidth, 1000);
  assert.equal(original.requestWidth, 1000);
});

test("double-tap fit toggles screen fit and original size", () => {
  assert.equal(togglePageOriginalFitMode(FitMode.PAGE), FitMode.ORIGINAL);
  assert.equal(togglePageOriginalFitMode(FitMode.ORIGINAL), FitMode.PAGE);
  assert.equal(togglePageOriginalFitMode(FitMode.WIDTH), FitMode.ORIGINAL);
  assert.equal(
    togglePageOriginalFitMode(FitMode.PAGE, { scale: 2.4 }),
    FitMode.PAGE,
    "pinch zoom must settle on page fit instead of flashing page fit and applying original size"
  );
});

test("only center touches enter the double-tap window", () => {
  const first = viewerTapSequenceTransition(null, {
    x: 120,
    y: 240,
    atMs: 1_000,
    width: 300,
    inputSource: "touch",
  });
  assert.equal(first.action, "pending_center_tap");
  assert.deepEqual(first.next, {
    x: 120,
    y: 240,
    atMs: 1_000,
    inputSource: "touch",
    zone: "center",
  });
  assert.equal(first.commitPrevious, false);

  const second = viewerTapSequenceTransition(first.next, {
    x: 126,
    y: 246,
    atMs: 1_240,
    width: 300,
    inputSource: "touch",
  });
  assert.equal(second.action, "double_tap");
  assert.equal(second.next, null);

  const late = viewerTapSequenceTransition(first.next, {
    x: 120,
    y: 240,
    atMs: 1_500,
    width: 300,
    inputSource: "touch",
  });
  assert.equal(late.action, "pending_center_tap");
  assert.equal(late.commitPrevious, true);
  assert.notEqual(late.next, null);

  const mouse = viewerTapSequenceTransition(null, {
    x: 120,
    y: 240,
    atMs: 2_000,
    width: 300,
    inputSource: "mouse",
  });
  assert.deepEqual(mouse, { action: "single_tap", next: null, commitPrevious: false });
});

test("edge taps stay immediate and never become a double-tap", () => {
  const left = viewerTapSequenceTransition(null, {
    x: 10,
    y: 240,
    atMs: 1_000,
    width: 300,
    inputSource: "touch",
  });
  const secondLeft = viewerTapSequenceTransition(left.next, {
    x: 12,
    y: 242,
    atMs: 1_100,
    width: 300,
    inputSource: "touch",
  });
  assert.deepEqual(left, { action: "edge_tap", next: null, commitPrevious: false });
  assert.deepEqual(secondLeft, { action: "edge_tap", next: null, commitPrevious: false });
  assert.equal(viewerTapZone(10, 300), "left");
  assert.equal(viewerTapZone(150, 300), "center");
  assert.equal(viewerTapZone(290, 300), "right");
});

test("spread layout fits the combined pages and preserves the configured gap", () => {
  const layout = viewerSpreadLayout({
    mode: FitMode.PAGE,
    pages: [
      { width: 1200, height: 1800 },
      { width: 1200, height: 1800 },
    ],
    viewportWidth: 1600,
    viewportHeight: 1000,
    devicePixelRatio: 2,
    gap: 12,
  });
  assert.equal(layout.gap, 12);
  assert.equal(Math.round(layout.pages[0].cssWidth), 667);
  assert.equal(Math.round(layout.pages[0].cssHeight), 1000);
  assert.equal(layout.pages[0].requestWidth, 1334);
  assert.equal(layout.pages[1].requestWidth, 1334);
  assert.equal(Math.round(layout.cssWidth), 1345);
  assert.equal(Math.round(layout.cssHeight), 1000);

  const unequal = viewerSpreadLayout({
    mode: FitMode.PAGE,
    pages: [
      { width: 1000, height: 2000 },
      { width: 1200, height: 1200 },
    ],
    viewportWidth: 1000,
    viewportHeight: 800,
    devicePixelRatio: 2,
    gap: 20,
  });
  assert.equal(Math.round(unequal.cssWidth), 1000);
  assert.equal(Math.round(unequal.cssHeight), 653);
  assert.equal(Math.round(unequal.pages[0].cssWidth), 327);
  assert.equal(Math.round(unequal.pages[1].cssWidth), 653);
  assert.equal(unequal.pages[0].requestWidth, 654);
  assert.equal(unequal.pages[1].requestWidth, 1307);

  const widthFit = viewerSpreadLayout({
    mode: FitMode.WIDTH,
    pages: [
      { width: 1200, height: 1800 },
      { width: 1200, height: 1800 },
    ],
    viewportWidth: 1600,
    viewportHeight: 1000,
    devicePixelRatio: 2,
    gap: 12,
  });
  assert.equal(Math.round(widthFit.cssWidth), 1600);
  assert.ok(widthFit.cssHeight > 1000);

  const single = viewerSpreadLayout({
    mode: FitMode.PAGE,
    pages: [{ width: 1200, height: 1800 }],
    viewportWidth: 430,
    viewportHeight: 932,
    devicePixelRatio: 2,
    gap: 20,
  });
  assert.equal(single.gap, 0);
  assert.equal(single.pages[0].requestWidth, 860);
  assert.equal(single.cssWidth, 430);
  assert.equal(single.cssHeight, 645);

  const landscapeSingle = viewerSpreadLayout({
    mode: FitMode.PAGE,
    pages: [{ width: 2000, height: 1000 }],
    viewportWidth: 430,
    viewportHeight: 932,
    devicePixelRatio: 2,
    gap: 20,
  });
  assert.equal(landscapeSingle.pages.length, 1);
  assert.equal(landscapeSingle.cssWidth, 430);
  assert.equal(landscapeSingle.cssHeight, 215);
  assert.equal(landscapeSingle.gap, 0);

  const singleNearSquareRequest = viewerSpreadLayout({
    mode: FitMode.PAGE,
    pages: [{ width: 1000, height: 1001 }],
    viewportWidth: 1001,
    viewportHeight: 1000,
    devicePixelRatio: 2,
  });
  const spreadNearSquareRequest = viewerSpreadLayout({
    mode: FitMode.PAGE,
    pages: [
      { width: 1000, height: 1001 },
      { width: 1000, height: 1001 },
    ],
    viewportWidth: 1001,
    viewportHeight: 1000,
    devicePixelRatio: 2,
    gap: 4,
  });
  assert.equal(singleNearSquareRequest.pages[0].requestWidth, 1999);
  assert.equal(spreadNearSquareRequest.pages[0].requestWidth, 997);
  assert.ok(
    spreadNearSquareRequest.pages[0].requestWidth <
      singleNearSquareRequest.pages[0].requestWidth * 0.51
  );
});

test("page display slots follow the physical left-to-right group order", () => {
  assert.equal(viewerPageDisplaySlot(1, 0), "single");
  assert.equal(viewerPageDisplaySlot(2, 0), "spread_left");
  assert.equal(viewerPageDisplaySlot(2, 1), "spread_right");
  assert.equal(viewerPageDisplaySlot(2, 2), "single");
  assert.equal(viewerSpreadPartnerIndex(1, 0), null);
  assert.equal(viewerSpreadPartnerIndex(2, 0), 1);
  assert.equal(viewerSpreadPartnerIndex(2, 1), 0);
  assert.equal(viewerSpreadPartnerIndex(2, 2), null);
});

test("remote state generation ignores stale observations and changes only after initialization", () => {
  assert.deepEqual(remoteStateGenerationTransition("", "boot-4"), {
    generation: "boot-4",
    changed: false,
    initialized: true,
  });
  assert.deepEqual(remoteStateGenerationTransition("boot-4", "boot-5"), {
    generation: "boot-5",
    changed: true,
    initialized: true,
  });
  assert.deepEqual(remoteStateGenerationTransition("boot-5", "boot-4"), {
    generation: "boot-5",
    changed: false,
    initialized: true,
  });
});

test("a page response attests exactly the generation requested by its viewer group", () => {
  assert.deepEqual(pageResponseGenerationAttestation("boot-7", "boot-7"), {
    requested: "boot-7",
    observed: "boot-7",
    matches: true,
  });
  assert.equal(pageResponseGenerationAttestation("boot-7", "").matches, false);
  assert.equal(pageResponseGenerationAttestation("boot-7", "boot-8").matches, false);
});

test("every page in one viewer group snapshots the same remote state generation", () => {
  assert.deepEqual(viewerPageGroupGenerationSnapshot("boot-7", 2), {
    generation: "boot-7",
    pages: ["boot-7", "boot-7"],
  });
});

test("page edge messages distinguish start, end and RTL guidance", () => {
  assert.equal(
    viewerBoundaryMessage({ currentIndex: 0, count: 5, delta: -1 }),
    "先頭ページです"
  );
  assert.equal(
    viewerBoundaryMessage({ currentIndex: 4, count: 5, delta: 1 }),
    "最終ページです"
  );
  assert.equal(
    viewerBoundaryMessage({
      currentIndex: 0,
      count: 5,
      delta: -1,
      readingDirection: ReadingDirection.RTL,
    }),
    "先頭ページです（右→左綴じ：次は左をタップ）"
  );
  assert.equal(
    viewerBoundaryMessage({
      currentIndex: 4,
      count: 5,
      delta: 1,
      readingDirection: ReadingDirection.RTL,
    }),
    "最終ページです（右→左綴じ：前は右をタップ）"
  );
  assert.equal(viewerBoundaryMessage({ currentIndex: 2, count: 5, delta: 1 }), null);
});

test("grid keys mirror parent, history, selection and page defaults", () => {
  assert.equal(commandFromKey(key("Backspace"), "grid").name, CommandName.PARENT_FOLDER);
  assert.equal(
    commandFromKey(key("ArrowUp", { altKey: true }), "grid").name,
    CommandName.PARENT_FOLDER
  );
  assert.equal(commandFromKey(key("Enter"), "grid").name, CommandName.OPEN_SELECTED);
  assert.equal(commandFromKey(key("PageDown"), "grid").name, CommandName.GRID_PAGE_NEXT);
  assert.equal(
    commandFromKey(key("ArrowLeft", { altKey: true }), "grid").name,
    CommandName.BACK
  );
  assert.equal(
    commandFromKey(key("ArrowRight", { altKey: true }), "grid").name,
    CommandName.FORWARD
  );
});

test("editable controls suppress every shortcut", () => {
  assert.equal(commandFromKey(key("ArrowRight", { editable: true }), "viewer"), null);
  assert.equal(commandFromKey(key("Enter", { editable: true }), "grid"), null);
  assert.equal(commandFromKey(key("?", { editable: true }), "viewer"), null);
});

test("escape closes the shared menu in every screen context", () => {
  assert.equal(
    commandFromKey(key("Escape", { menuOpen: true }), "favorites").name,
    CommandName.TOGGLE_MENU
  );
});

test("viewer tap zones and wheel inputs emit the same commands", () => {
  assert.equal(viewerTapCommand(10, 300).name, CommandName.PREV_PAGE);
  assert.equal(viewerTapCommand(150, 300).name, CommandName.TOGGLE_VIEWER_BARS);
  assert.equal(viewerTapCommand(290, 300).name, CommandName.NEXT_PAGE);
  assert.equal(viewerWheelCommand(120, false).name, CommandName.NEXT_PAGE);
  assert.equal(viewerWheelCommand(-120, true).name, CommandName.ZOOM_IN);
});

test("viewer seek uses one physical tick per page group and real page labels", () => {
  assert.deepEqual(
    viewerSeekState({
      groupPageIndexes: [[0], [1], [2]],
      currentGroupIndex: 1,
      pageCount: 3,
    }),
    {
      visible: true,
      min: 0,
      max: 2,
      value: 1,
      groupIndex: 1,
      direction: ReadingDirection.LTR,
      label: "2 / 3",
    }
  );
  assert.deepEqual(
    viewerSeekState({
      groupPageIndexes: [[0], [1, 2], [3, 4]],
      currentGroupIndex: 1,
      pageCount: 5,
    }),
    {
      visible: true,
      min: 0,
      max: 2,
      value: 1,
      groupIndex: 1,
      direction: ReadingDirection.LTR,
      label: "2-3 / 5",
    }
  );
});

test("viewer seek keeps logical values and delegates RTL geometry to the native range", () => {
  const groups = [[0], [1, 2], [3, 4]];
  const start = viewerSeekState({
    groupPageIndexes: groups,
    currentGroupIndex: 0,
    pageCount: 5,
    rtl: true,
  });
  assert.equal(start.value, 0);
  assert.equal(start.direction, ReadingDirection.RTL);
  const end = viewerSeekState({
    groupPageIndexes: groups,
    currentGroupIndex: 2,
    pageCount: 5,
    rtl: true,
  });
  assert.equal(end.value, 2);
  assert.equal(end.direction, ReadingDirection.RTL);
  assert.equal(viewerSeekGroupIndex(0, groups.length), 0);
  assert.equal(viewerSeekGroupIndex(2, groups.length), 2);
  assert.equal(viewerSeekGroupIndex(99, groups.length), 2);
});

test("viewer seek relative drag mirrors physical RTL travel", () => {
  const drag = (direction, currentClientX, groupCount = 31) => viewerSeekRelativeDragValue({
    startGroupIndex: 15,
    startClientX: 100,
    currentClientX,
    trackWidth: 300,
    groupCount,
    direction,
  });
  assert.equal(drag(ReadingDirection.LTR, 200), 25);
  assert.equal(drag(ReadingDirection.LTR, 0), 5);
  assert.equal(drag(ReadingDirection.RTL, 200), 5);
  assert.equal(drag(ReadingDirection.RTL, 0), 25);
  assert.equal(drag(ReadingDirection.LTR, 140, 300), 55);
  assert.equal(drag(ReadingDirection.RTL, 140, 300), 0);
  assert.equal(drag(ReadingDirection.LTR, 100, 0), -1);
});

test("viewer seek hides only its range for a one-image sequence", () => {
  assert.deepEqual(
    viewerSeekState({
      groupPageIndexes: [[0]],
      currentGroupIndex: 0,
      pageCount: 1,
    }),
    {
      visible: false,
      min: 0,
      max: 0,
      value: 0,
      groupIndex: 0,
      direction: ReadingDirection.LTR,
      label: "1 / 1",
    }
  );
});

test("viewer gesture separates vertical and horizontal swipes", () => {
  assert.equal(
    viewerGestureDecision({ dx: -80, dy: 40, elapsedMs: 180 }),
    ViewerGesture.SWIPE_LEFT
  );
  assert.equal(
    viewerGestureDecision({ dx: 40, dy: -80, elapsedMs: 180 }),
    ViewerGesture.SWIPE_UP
  );
  assert.equal(
    viewerGestureDecision({ dx: 40, dy: 80, elapsedMs: 180 }),
    ViewerGesture.SWIPE_DOWN
  );
  assert.equal(
    viewerGestureDecision({ dx: 60, dy: 50, elapsedMs: 180 }),
    null
  );
});

test("viewer panel dimensions follow portrait bottom-half and landscape left-40-percent rules", () => {
  assert.deepEqual(
    viewerPanelLayout({ viewportWidth: 390, viewportHeight: 844 }),
    {
      orientation: ViewerPanelOrientation.PORTRAIT,
      panel: { x: 0, y: 422, width: 390, height: 422 },
      image: { x: 0, y: 0, width: 390, height: 422 },
    }
  );
  assert.deepEqual(
    viewerPanelLayout({ viewportWidth: 844, viewportHeight: 390 }),
    {
      orientation: ViewerPanelOrientation.LANDSCAPE,
      panel: { x: 0, y: 0, width: 337.6, height: 390 },
      image: { x: 337.6, y: 0, width: 506.4, height: 390 },
    }
  );
});

test("viewer panel open and close transitions request image refits", () => {
  const closed = viewerPanelTransition(null, {
    action: ViewerPanelAction.CLOSE,
    viewportWidth: 390,
    viewportHeight: 844,
  });
  assert.equal(closed.shouldRefit, false);
  assert.deepEqual(closed.layout.image, { x: 0, y: 0, width: 390, height: 844 });

  const opened = viewerPanelTransition(closed, {
    action: ViewerPanelAction.OPEN,
    viewportWidth: 390,
    viewportHeight: 844,
  });
  assert.equal(opened.shouldRefit, true);
  assert.equal(opened.resetTransform, true);
  assert.deepEqual(opened.layout.image, { x: 0, y: 0, width: 390, height: 422 });

  const closedAgain = viewerPanelTransition(opened, {
    action: ViewerPanelAction.CLOSE,
    viewportWidth: 390,
    viewportHeight: 844,
  });
  assert.equal(closedAgain.shouldRefit, true);
  assert.deepEqual(closedAgain.layout.image, { x: 0, y: 0, width: 390, height: 844 });
});

test("viewer panel shell transition shares open and orientation state without image policy", () => {
  const opened = viewerPanelShellTransition(null, {
    action: ViewerPanelAction.OPEN,
    viewportWidth: 390,
    viewportHeight: 844,
  });
  assert.equal(opened.open, true);
  assert.equal(opened.orientation, ViewerPanelOrientation.PORTRAIT);
  assert.equal(opened.layoutChanged, true);
  assert.equal("shouldRefit" in opened, false);

  const landscape = viewerPanelShellTransition(opened, {
    viewportWidth: 844,
    viewportHeight: 390,
  });
  assert.equal(landscape.open, true);
  assert.equal(landscape.orientation, ViewerPanelOrientation.LANDSCAPE);
  assert.equal(landscape.layoutChanged, true);
});

test("an open viewer panel changes side and refits when orientation changes", () => {
  const portrait = viewerPanelTransition(null, {
    action: ViewerPanelAction.OPEN,
    viewportWidth: 390,
    viewportHeight: 844,
  });
  const landscape = viewerPanelTransition(portrait, {
    action: "resize",
    viewportWidth: 844,
    viewportHeight: 390,
  });
  assert.equal(landscape.open, true);
  assert.equal(landscape.orientation, ViewerPanelOrientation.LANDSCAPE);
  assert.equal(landscape.shouldRefit, true);
  assert.deepEqual(landscape.layout.image, {
    x: 337.6,
    y: 0,
    width: 506.4,
    height: 390,
  });
});

test("orientation spread refresh keeps the mounted viewer and its open panel", () => {
  assert.deepEqual(
    viewerResizePlan({
      hasContainer: true,
      forceSinglePageChanged: true,
      panelOpen: true,
    }),
    {
      refreshContainer: true,
      rebuildViewer: false,
      keepPanelOpen: true,
    }
  );
});

test("viewer panel swipe uses the existing gesture arbitration without stealing taps, paging, or pan", () => {
  const swipeUp = viewerGestureDecision({ dx: 8, dy: -80, elapsedMs: 180 });
  assert.equal(
    viewerPanelGestureAction({
      gesture: swipeUp,
      startY: 300,
      contentTop: 0,
      contentBottom: 844,
    }),
    ViewerPanelAction.OPEN
  );
  assert.equal(
    viewerPanelGestureAction({
      gesture: swipeUp,
      startY: 810,
      contentTop: 0,
      contentBottom: 844,
    }),
    null
  );
  assert.equal(
    viewerPanelGestureAction({
      gesture: viewerGestureDecision({ dx: -80, dy: 8, elapsedMs: 180 }),
      startY: 300,
      contentBottom: 844,
    }),
    null
  );
  assert.equal(
    viewerPanelGestureAction({
      gesture: viewerGestureDecision({ dx: 2, dy: 2, elapsedMs: 120 }),
      startY: 300,
      contentBottom: 844,
    }),
    null
  );
  assert.equal(
    viewerPanelGestureAction({
      gesture: viewerGestureDecision({
        dx: 0,
        dy: -100,
        elapsedMs: 180,
        moved: true,
        zoomed: true,
      }),
      startY: 300,
      contentBottom: 844,
    }),
    null
  );
  assert.equal(
    viewerPanelGestureAction({
      gesture: ViewerGesture.SWIPE_DOWN,
      panelOpen: true,
    }),
    ViewerPanelAction.CLOSE
  );
  assert.equal(
    viewerPanelGestureAction({
      gesture: ViewerGesture.SWIPE_DOWN,
      panelOpen: true,
      contentScrolled: true,
    }),
    null
  );
});

test("viewer gesture rejects sub-threshold motion and prioritizes pan", () => {
  assert.equal(
    viewerGestureDecision({ dx: 0, dy: -52, elapsedMs: 180 }),
    null
  );
  assert.equal(
    viewerGestureDecision({ dx: 0, dy: -53, elapsedMs: 180 }),
    ViewerGesture.SWIPE_UP
  );
  assert.equal(
    viewerGestureDecision({
      dx: 0,
      dy: -100,
      elapsedMs: 180,
      moved: true,
      zoomed: true,
    }),
    ViewerGesture.PAN
  );
  assert.equal(
    viewerGestureDecision({
      dx: 0,
      dy: -100,
      elapsedMs: 180,
      moved: true,
      contentScrolled: true,
    }),
    ViewerGesture.PAN
  );
});

test("width-fit vertical pan requires real direction-specific scroll room", () => {
  assert.deepEqual(
    viewerVerticalScrollDecision({
      scrollTop: 0,
      scrollHeight: 800,
      clientHeight: 800,
      dragDeltaY: -80,
    }),
    { scrollable: false, canConsume: false, atStart: true, atEnd: true, maximum: 0 }
  );
  assert.equal(viewerVerticalScrollDecision({
    scrollTop: 200,
    scrollHeight: 1600,
    clientHeight: 800,
    dragDeltaY: -80,
  }).canConsume, true);
  assert.equal(viewerVerticalScrollDecision({
    scrollTop: 0,
    scrollHeight: 1600,
    clientHeight: 800,
    dragDeltaY: 80,
  }).canConsume, false);
  assert.equal(viewerVerticalScrollDecision({
    scrollTop: 0,
    scrollHeight: 1600,
    clientHeight: 800,
    dragDeltaY: -80,
  }).canConsume, true);
  assert.equal(viewerVerticalScrollDecision({
    scrollTop: 800,
    scrollHeight: 1600,
    clientHeight: 800,
    dragDeltaY: -80,
  }).canConsume, false);
  assert.equal(viewerVerticalScrollDecision({
    scrollTop: 800,
    scrollHeight: 1600,
    clientHeight: 800,
    dragDeltaY: 80,
  }).canConsume, true);
});

test("reading progress batching emits latest only and force-flushes the final position", () => {
  let batch = createReadingProgressBatch();
  let transition = readingProgressBatchTransition(batch, {
    type: "observe",
    now: 1_000,
    value: { identity: "page-1", page: 1 },
  });
  batch = transition.state;
  assert.equal(transition.effect.page, 1);

  transition = readingProgressBatchTransition(batch, {
    type: "observe",
    now: 2_000,
    value: { identity: "page-2", page: 2 },
  });
  batch = transition.state;
  assert.equal(transition.effect, null);
  transition = readingProgressBatchTransition(batch, {
    type: "observe",
    now: 3_000,
    value: { identity: "page-3", page: 3 },
  });
  batch = transition.state;
  assert.equal(transition.effect, null);

  transition = readingProgressBatchTransition(batch, { type: "tick", now: 31_000 });
  batch = transition.state;
  assert.equal(transition.effect.page, 3);

  transition = readingProgressBatchTransition(batch, {
    type: "observe",
    now: 32_000,
    value: { identity: "page-4", page: 4 },
  });
  batch = transition.state;
  assert.equal(transition.effect, null);
  transition = readingProgressBatchTransition(batch, { type: "flush", now: 33_000 });
  assert.equal(transition.effect.page, 4);
  assert.equal(
    readingProgressBatchTransition(transition.state, { type: "flush", now: 34_000 }).effect,
    null
  );
});

test("keyboard help defaults to pointer capability and remembers real key input", () => {
  assert.equal(shouldShowKeyboardShortcuts({ coarsePointer: false }), true);
  assert.equal(shouldShowKeyboardShortcuts({ coarsePointer: true }), false);
  assert.equal(
    shouldShowKeyboardShortcuts({ coarsePointer: true, keyboardUsed: true }),
    true
  );
});

test("grid cursor uses the same keyboard-availability signal as shortcut hints", () => {
  for (const input of [
    { coarsePointer: false, keyboardUsed: false },
    { coarsePointer: true, keyboardUsed: false },
    { coarsePointer: true, keyboardUsed: true },
  ]) {
    const keyboardAvailable = shouldShowKeyboardShortcuts(input);
    assert.equal(
      shouldShowGridCursor({ keyboardAvailable }),
      keyboardAvailable
    );
  }
});

test("grid navigation uses columns, page rows and clamps to valid entries", () => {
  const base = { current: 5, count: 20, columns: 4, pageRows: 3 };
  assert.equal(gridIndexForCommand({ ...base, name: CommandName.GRID_DOWN }), 9);
  assert.equal(gridIndexForCommand({ ...base, name: CommandName.GRID_PAGE_NEXT }), 17);
  assert.equal(
    gridIndexForCommand({ ...base, current: 18, name: CommandName.GRID_PAGE_NEXT }),
    19
  );
  assert.equal(gridIndexForCommand({ ...base, name: CommandName.GRID_FIRST }), 0);
});

test("grid layout derives columns from target width and applies the tile aspect", () => {
  const phone = gridLayoutForWidth(390, 1);
  assert.equal(phone.columns, 3);
  assert.equal(phone.cellWidth, 118);
  assert.equal(phone.previewHeight, 118);
  assert.equal(phone.labelHeight, 38);
  assert.equal(phone.tileHeight, 156);
  assert.equal(phone.rowPitch, 164);

  const portraitPhone = gridLayoutForWidth(390, 1.5);
  assert.equal(portraitPhone.columns, 3);
  assert.equal(portraitPhone.previewHeight, 177);
  assert.equal(portraitPhone.tileHeight, 215);

  const landscapePhone = gridLayoutForWidth(390, 9 / 16);
  assert.equal(landscapePhone.previewHeight, 66);
  assert.equal(landscapePhone.tileHeight, 104);

  assert.equal(gridLayoutForWidth(768, 1).columns, 4);
  assert.equal(gridLayoutForWidth(1280, 1).columns, 6);
  assert.equal(gridLayoutForWidth(1920, 1).columns, 9);
  assert.equal(gridLayoutForWidth(390, Number.NaN).previewHeight, 118);
});

test("grid label height is chosen once from collection metadata", () => {
  const plainContainerEntries = [
    { kind: "image", name: "1.jpg" },
    { kind: "image", name: "2.jpg" },
  ];
  assert.equal(gridLabelHeightForEntries(plainContainerEntries), 38);
  assert.equal(
    gridLabelHeightForEntries([
      plainContainerEntries[0],
      { kind: "pdf", name: "book.pdf", detail: "20 ページ" },
      plainContainerEntries[1],
    ]),
    56
  );
  assert.equal(
    gridLabelHeightForEntries([
      plainContainerEntries[0],
      { kind: "image", name: "rated.jpg", rating: 4 },
    ]),
    56
  );
  const sharedHeight = gridLabelHeightForEntries([
    { name: "plain" },
    { name: "progress", detail: "3 / 20 ページ" },
  ]);
  const tileHeights = [1, 2].map(
    () => gridLayoutForWidth(440, 1, sharedHeight).tileHeight
  );
  assert.equal(new Set(tileHeights).size, 1);
  assert.equal(tileHeights[0], 155);
});

test("grid column override clamps, returns to auto, and selects by measured size", () => {
  assert.equal(clampGridColumnOverride(0), 0);
  assert.equal(clampGridColumnOverride(1), 2);
  assert.equal(clampGridColumnOverride(-5), 2);
  assert.equal(clampGridColumnOverride(20), 8);
  assert.equal(gridLayoutForWidth(440, 1, 38, 2).columns, 2);
  assert.equal(gridLayoutForWidth(440, 1, 38, 8).columns, 8);
  const overridden = gridLayoutForWidth(440, 1.5, 56, 8);
  assert.equal(overridden.cellWidth, 45.5);
  assert.equal(overridden.previewHeight, 68);
  assert.equal(overridden.tileHeight, 124);
  assert.equal(overridden.rowPitch, 132);
  assert.equal(
    gridLayoutForWidth(440, 1, 38, 0).columns,
    gridLayoutForWidth(440, 1).columns
  );

  const settings = {
    gridColumnsPortrait: 3,
    gridColumnsLandscape: 7,
  };
  assert.equal(gridColumnOverrideForViewport(440, 900, settings), 3);
  assert.equal(gridColumnOverrideForViewport(956, 440, settings), 7);
  assert.equal(gridColumnOverrideForViewport(440, 440, settings), 7);
});

test("grid column override field selects by measured viewport size", () => {
  assert.equal(
    gridColumnOverrideFieldForViewport(440, 900),
    "gridColumnsPortrait"
  );
  assert.equal(
    gridColumnOverrideFieldForViewport(956, 440),
    "gridColumnsLandscape"
  );
  assert.equal(
    gridColumnOverrideFieldForViewport(440, 440),
    "gridColumnsLandscape"
  );
});

test("grid pinch changes one column only after a symmetric scale threshold", () => {
  assert.equal(gridColumnsAfterPinch(4, 1.11), 4);
  assert.equal(gridColumnsAfterPinch(4, 0.9), 4);
  assert.equal(gridColumnsAfterPinch(4, 1.12), 3);
  assert.equal(gridColumnsAfterPinch(4, 1 / 1.12), 5);
  assert.equal(gridColumnsAfterPinch(2, 1.5), 2);
  assert.equal(gridColumnsAfterPinch(8, 0.5), 8);
  assert.equal(gridColumnsAfterPinch(9, 1.5), 8);
  assert.equal(gridColumnsAfterPinch(9, 0.5), 9);
  assert.equal(gridColumnsAfterPinch(4, Number.NaN), 4);
});

test("grid scroll extent and snapping stay on whole row boundaries", () => {
  const extent = gridScrollExtent(100, 164, 700);
  assert.deepEqual(extent, {
    naturalHeight: 16400,
    maxOffset: 15744,
    totalHeight: 16444,
  });
  assert.equal(extent.maxOffset % 164, 0);
  assert.equal(snappedGridOffset(250, 164, extent.maxOffset), 328);
  assert.equal(snappedGridOffset(20000, 164, extent.maxOffset), 15744);
  assert.deepEqual(gridScrollExtent(2, 164, 700), {
    naturalHeight: 328,
    maxOffset: 0,
    totalHeight: 700,
  });
});

test('grid return scrolls only enough to reveal the previously viewed item', () => {
  const itemIdentities = Array.from({ length: 40 }, (_, index) => 'item-' + index);
  assert.deepEqual(
    resolveGridReturnViewport({
      targetItemIdentity: 'item-22',
      itemIdentities,
      previousScrollTop: 200,
      columns: 4,
      rowPitch: 100,
      viewportHeight: 300,
    }),
    { targetIndex: 22, scrollTop: 300 }
  );
});

test('grid return clamps the old range instead of jumping to the top when the item is gone', () => {
  const result = resolveGridReturnViewport({
    targetItemIdentity: 'deleted-item',
    itemIdentities: Array.from({ length: 20 }, (_, index) => 'item-' + index),
    previousScrollTop: 900,
    columns: 2,
    rowPitch: 100,
    viewportHeight: 300,
  });
  assert.deepEqual(result, { targetIndex: -1, scrollTop: 700 });
  assert.notEqual(result.scrollTop, 0);
});

test('an untouched grid has no anchor while a real first-item selection anchors item zero', () => {
  const itemIdentities = Array.from({ length: 30 }, (_, index) => 'item' + index);
  const anchor = new GridViewportAnchor(itemIdentities);
  const untouched = anchor.snapshot(800);
  assert.deepEqual(untouched, {
    previousScrollTop: 800,
    targetItemIdentity: null,
  });
  assert.deepEqual(
    resolveGridReturnViewport({
      ...untouched,
      itemIdentities,
      columns: 3,
      rowPitch: 200,
      viewportHeight: 600,
    }),
    { targetIndex: -1, scrollTop: 800 }
  );

  assert.equal(anchor.select(0), true);
  const selectedFirst = anchor.snapshot(800);
  assert.deepEqual(selectedFirst, {
    previousScrollTop: 800,
    targetItemIdentity: 'item0',
  });
  assert.deepEqual(
    resolveGridReturnViewport({
      ...selectedFirst,
      itemIdentities,
      columns: 3,
      rowPitch: 200,
      viewportHeight: 600,
    }),
    { targetIndex: 0, scrollTop: 0 }
  );
});

test('grid viewport memory keeps multi-level round trips independent', () => {
  const memory = new GridViewportMemory(4);
  memory.remember('folder-a', {
    previousScrollTop: 200,
    targetItemIdentity: 'child-22',
  });
  memory.remember('folder-a/child-22', {
    previousScrollTop: 100,
    targetItemIdentity: 'grandchild-35',
  });

  assert.deepEqual(
    resolveGridReturnViewport({
      ...memory.recall('folder-a/child-22'),
      itemIdentities: Array.from(
        { length: 50 },
        (_, index) => index === 35 ? 'grandchild-35' : 'child-item-' + index
      ),
      columns: 5,
      rowPitch: 100,
      viewportHeight: 300,
    }),
    { targetIndex: 35, scrollTop: 500 }
  );
  assert.deepEqual(
    resolveGridReturnViewport({
      ...memory.recall('folder-a'),
      itemIdentities: Array.from(
        { length: 40 },
        (_, index) => index === 22 ? 'child-22' : 'parent-item-' + index
      ),
      columns: 4,
      rowPitch: 100,
      viewportHeight: 300,
    }),
    { targetIndex: 22, scrollTop: 300 }
  );
  assert.equal(memory.recall('folder-b'), null);
});

test('grid viewport memory keeps scroll while viewer and parent navigation update the target', () => {
  const memory = new GridViewportMemory(4);
  memory.remember('collection', {
    previousScrollTop: 700,
    targetItemIdentity: 'page-1',
  });
  memory.updateTarget('collection', 'page-9');
  assert.deepEqual(memory.recall('collection'), {
    previousScrollTop: 700,
    targetItemIdentity: 'page-9',
  });

  memory.updateTarget('unvisited-parent', 'departed-child');
  assert.deepEqual(memory.recall('unvisited-parent'), {
    previousScrollTop: 0,
    targetItemIdentity: 'departed-child',
  });
});

test('grid viewport memory evicts the least recently used context', () => {
  const memory = new GridViewportMemory(2);
  memory.remember('folder-a', { previousScrollTop: 100 });
  memory.remember('folder-b', { previousScrollTop: 200 });
  assert.equal(memory.recall('folder-a').previousScrollTop, 100);
  memory.remember('folder-c', { previousScrollTop: 300 });

  assert.equal(memory.size, 2);
  assert.equal(memory.recall('folder-b'), null);
  assert.equal(memory.recall('folder-a').previousScrollTop, 100);
  assert.equal(memory.recall('folder-c').previousScrollTop, 300);

  const defaultMemory = new GridViewportMemory();
  for (let index = 0; index <= GRID_VIEWPORT_MEMORY_LIMIT; index += 1) {
    defaultMemory.remember('default-' + index, { previousScrollTop: index });
  }
  assert.equal(defaultMemory.size, GRID_VIEWPORT_MEMORY_LIMIT);
  assert.equal(defaultMemory.recall('default-0'), null);
});

test("thumbnail responses apply only to the tile generation and item that requested them", () => {
  assert.equal(thumbnailBindingMatches(4, "album/a.jpg", 4, "album/a.jpg"), true);
  assert.equal(thumbnailBindingMatches(5, "album/a.jpg", 4, "album/a.jpg"), false);
  assert.equal(thumbnailBindingMatches(4, "album/b.jpg", 4, "album/a.jpg"), false);
});

test("thumbnail concurrency leaves one shared heavy slot for page and container work", () => {
  assert.equal(thumbnailRequestConcurrency(4), 3);
  assert.equal(thumbnailRequestConcurrency(4, 0), 4);
  assert.equal(thumbnailRequestConcurrency(1), 1);
  assert.equal(thumbnailRequestConcurrency(Number.NaN), 1);
  assert.equal(thumbnailRequestStartCount(0, 20, 3), 3);
  assert.equal(thumbnailRequestStartCount(2, 20, 3), 1);
  assert.equal(thumbnailRequestStartCount(3, 20, 3), 0);
  assert.equal(thumbnailRequestStartCount(1, 0, 3), 0);
});

test("thumbnail retry policy retries only transient failures with a bounded backoff", () => {
  assert.deepEqual(thumbnailRetryDecision(502, "ipc_protocol_error", 0), {
    retry: true,
    exhausted: false,
    delayMs: 200,
    consumeRetryBudget: true,
  });
  assert.deepEqual(thumbnailRetryDecision(503, "miv_not_running", 2), {
    retry: true,
    exhausted: false,
    delayMs: 800,
    consumeRetryBudget: true,
  });
  assert.deepEqual(thumbnailRetryDecision(502, "ipc_protocol_error", 3), {
    retry: false,
    exhausted: true,
    delayMs: 0,
    consumeRetryBudget: false,
  });
  assert.equal(thumbnailRetryDecision(404, "not_found", 0).retry, false);
  assert.equal(thumbnailRetryDecision(422, "generation_failed", 0).retry, false);
  assert.equal(
    thumbnailRetryDecision(503, "protocol_version_mismatch", 0).retry,
    false
  );
});

test("thumbnail admission busy returns to the queue without spending retry budget", () => {
  assert.deepEqual(thumbnailRetryDecision(503, "ipc_busy", 999), {
    retry: true,
    exhausted: false,
    delayMs: 0,
    consumeRetryBudget: false,
  });
});

test("page prefetch follows reading direction and accepts a future spread", () => {
  assert.deepEqual(
    pagePrefetchPlan({ visibleIndexes: [10], itemCount: 20, direction: 1 }),
    [11, 12, 13, 9]
  );
  assert.deepEqual(
    pagePrefetchPlan({ visibleIndexes: [10], itemCount: 20, direction: -1 }),
    [9, 8, 7, 11]
  );
  assert.deepEqual(
    pagePrefetchPlan({ visibleIndexes: [10, 11], itemCount: 20, direction: 1 }),
    [12, 13, 14, 9]
  );
  assert.deepEqual(
    pagePrefetchPlan({ visibleIndexes: [0], itemCount: 3, direction: -1 }),
    [1]
  );
});

test("loading indicator appears only after the stable delay threshold", () => {
  assert.equal(shouldShowLoadingIndicator(true, 224, 225), false);
  assert.equal(shouldShowLoadingIndicator(true, 225, 225), true);
  assert.equal(shouldShowLoadingIndicator(false, 500, 225), false);
});

test("viewer transform commands dispatch through one pure state transition", () => {
  const initial = { scale: 1, panX: 0, panY: 0 };
  const zoomed = reduceViewerTransform(initial, { name: CommandName.ZOOM_IN });
  assert.deepEqual(zoomed, { scale: 1.2, panX: 0, panY: 0 });
  const panned = reduceViewerTransform(zoomed, {
    name: CommandName.PAN_BY,
    payload: { dx: 12, dy: -5 },
  });
  assert.deepEqual(panned, { scale: 1.2, panX: 12, panY: -5 });
  assert.deepEqual(
    reduceViewerTransform(panned, { name: CommandName.ZOOM_RESET }),
    initial
  );
  assert.equal(reduceViewerTransform(initial, { name: CommandName.NEXT_PAGE }), null);
});
