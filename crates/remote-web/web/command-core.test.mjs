import test from "node:test";
import assert from "node:assert/strict";

import {
  CommandName,
  FitMode,
  ReadingDirection,
  SpreadMode,
  ViewerGesture,
  VIDEO_QUALITY_PRESETS,
  bufferingQualitySuggestion,
  clampGridColumnOverride,
  commandFromKey,
  containerPageTargetPx,
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
  nextSpreadMode,
  readingDirectionForSpreadMode,
  readingProgressBatchTransition,
  reduceViewerTransform,
  resolveGridReturnViewport,
  viewerTapCommand,
  nextFitMode,
  pagePrefetchPlan,
  planSpreadIntent,
  snappedGridOffset,
  thumbnailBindingMatches,
  thumbnailRequestConcurrency,
  thumbnailRequestStartCount,
  thumbnailRetryDecision,
  shouldShowGridCursor,
  shouldShowLoadingIndicator,
  shouldShowKeyboardShortcuts,
  sessionOwnerBadge,
  viewerImageLayout,
  viewerBoundaryMessage,
  viewerGestureDecision,
  viewerVerticalScrollDecision,
  viewerSeekGroupIndex,
  viewerSeekState,
  viewerSpreadLayout,
  viewerWheelCommand,
  videoHttpStatusDecision,
  videoPlaybackDecision,
  videoQualityPreset,
  videoSeekPlan,
  videoStartupDecision,
  videoTapCommand,
  shouldReanchorVideoTimeline,
  videoTimelineAnchor,
  videoTimelinePosition,
} from "./command-core.mjs";

const key = (value, extra = {}) => ({ key: value, ...extra });

test("session owner badge keeps the two non-blocking ownership states explicit", () => {
  assert.deepEqual(sessionOwnerBadge("active"), {
    owner: "active",
    label: "操作中",
  });
  assert.deepEqual(sessionOwnerBadge("other_device"), {
    owner: "other_device",
    label: "別の端末が操作中 (操作すると取得します)",
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

test("the timeline is anchored once per generation, not on every state poll", () => {
  // liveLag = seekableEnd - currentTime は segment 到着ごとに 0 と segment 長の間を
  // 往復するので、毎回の poll で anchor を引き直すと表示位置がその幅だけ前後する。
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

  // 同じ世代で liveLag だけが揺れても、表示位置は端末自身の再生位置で進む。
  const anchor = videoTimelineAnchor({
    serverPositionSecs: 300,
    mediaCurrentTimeSecs: 55,
    seekableEndSecs: 60,
    durationSecs: 600,
  });
  const jittered = videoTimelineAnchor({
    serverPositionSecs: 302,
    mediaCurrentTimeSecs: 57,
    seekableEndSecs: 62,
    durationSecs: 600,
  });
  assert.notDeepEqual(anchor, jittered, "poll ごとの anchor は揺れる (だから固定する)");
  assert.equal(
    videoTimelinePosition({
      anchorSourcePositionSecs: anchor.sourcePositionSecs,
      anchorMediaTimeSecs: anchor.mediaTimeSecs,
      mediaCurrentTimeSecs: 57,
      durationSecs: 600,
    }),
    297,
    "固定した基準点なら 2 秒進んだぶんだけ素直に進む"
  );
});

test("whole-video position composes server state with the HLS window", () => {
  const anchor = videoTimelineAnchor({
    serverPositionSecs: 300,
    mediaCurrentTimeSecs: 55,
    seekableEndSecs: 60,
    durationSecs: 600,
  });
  assert.deepEqual(anchor, { sourcePositionSecs: 295, mediaTimeSecs: 55 });
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
      label: "2-3 / 5",
    }
  );
});

test("viewer seek reverses its physical endpoints for RTL books", () => {
  const groups = [[0], [1, 2], [3, 4]];
  assert.equal(viewerSeekState({
    groupPageIndexes: groups,
    currentGroupIndex: 0,
    pageCount: 5,
    rtl: true,
  }).value, 2);
  assert.equal(viewerSeekState({
    groupPageIndexes: groups,
    currentGroupIndex: 2,
    pageCount: 5,
    rtl: true,
  }).value, 0);
  assert.equal(viewerSeekGroupIndex(0, groups.length, true), 2);
  assert.equal(viewerSeekGroupIndex(2, groups.length, true), 0);
  assert.equal(viewerSeekGroupIndex(0, groups.length, false), 0);
  assert.equal(viewerSeekGroupIndex(99, groups.length, false), 2);
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
      sourceContext: 'folder-a',
      destinationContext: 'folder-a',
      viewedItemIdentity: 'item-22',
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
    sourceContext: 'folder-a',
    destinationContext: 'folder-a',
    viewedItemIdentity: 'deleted-item',
    itemIdentities: Array.from({ length: 20 }, (_, index) => 'item-' + index),
    previousScrollTop: 900,
    columns: 2,
    rowPitch: 100,
    viewportHeight: 300,
  });
  assert.deepEqual(result, { targetIndex: -1, scrollTop: 700 });
  assert.notEqual(result.scrollTop, 0);
});

test('grid return does not restore across folders', () => {
  assert.equal(
    resolveGridReturnViewport({
      sourceContext: 'folder-a',
      destinationContext: 'folder-b',
      viewedItemIdentity: 'item-10',
      itemIdentities: ['item-10'],
      previousScrollTop: 600,
      columns: 1,
      rowPitch: 100,
      viewportHeight: 300,
    }),
    null
  );
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
    [11, 12, 13, 14, 15, 16, 17, 18, 9]
  );
  assert.deepEqual(
    pagePrefetchPlan({ visibleIndexes: [10], itemCount: 20, direction: -1 }),
    [9, 8, 7, 6, 5, 4, 3, 2, 11]
  );
  assert.deepEqual(
    pagePrefetchPlan({ visibleIndexes: [10, 11], itemCount: 20, direction: 1 }),
    [12, 13, 14, 15, 16, 17, 18, 19, 9]
  );
  assert.deepEqual(
    pagePrefetchPlan({ visibleIndexes: [0], itemCount: 3, direction: -1 }),
    [1]
  );
});

test("container page target uses the rendered width and source aspect", () => {
  assert.equal(
    containerPageTargetPx({
      requestWidth: 1250,
      sourceWidth: 2665,
      sourceHeight: 3840,
    }),
    1802
  );
  assert.equal(
    containerPageTargetPx({
      requestWidth: 2400,
      sourceWidth: 1600,
      sourceHeight: 900,
    }),
    2400
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
