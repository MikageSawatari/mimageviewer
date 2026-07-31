import test from "node:test";
import assert from "node:assert/strict";

import {
  CommandName,
  FitMode,
  commandFromKey,
  containerPageTargetPx,
  gridLayoutForWidth,
  gridScrollExtent,
  gridIndexForCommand,
  reduceViewerTransform,
  viewerTapCommand,
  nextFitMode,
  pagePrefetchPlan,
  snappedGridOffset,
  thumbnailBindingMatches,
  thumbnailRetryDecision,
  shouldShowLoadingIndicator,
  sessionOwnerBadge,
  viewerImageLayout,
  viewerWheelCommand,
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
  assert.equal(commandFromKey(key("0"), "viewer").name, CommandName.FIT_CYCLE);
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
  assert.equal(viewerTapCommand(150, 300).name, CommandName.TOGGLE_MENU);
  assert.equal(viewerTapCommand(290, 300).name, CommandName.NEXT_PAGE);
  assert.equal(viewerWheelCommand(120, false).name, CommandName.NEXT_PAGE);
  assert.equal(viewerWheelCommand(-120, true).name, CommandName.ZOOM_IN);
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

test("thumbnail responses apply only to the tile generation and item that requested them", () => {
  assert.equal(thumbnailBindingMatches(4, "album/a.jpg", 4, "album/a.jpg"), true);
  assert.equal(thumbnailBindingMatches(5, "album/a.jpg", 4, "album/a.jpg"), false);
  assert.equal(thumbnailBindingMatches(4, "album/b.jpg", 4, "album/a.jpg"), false);
});

test("thumbnail retry policy retries only transient failures with a bounded backoff", () => {
  assert.deepEqual(thumbnailRetryDecision(502, "ipc_protocol_error", 0), {
    retry: true,
    exhausted: false,
    delayMs: 200,
  });
  assert.deepEqual(thumbnailRetryDecision(503, "miv_not_running", 2), {
    retry: true,
    exhausted: false,
    delayMs: 800,
  });
  assert.deepEqual(thumbnailRetryDecision(502, "ipc_protocol_error", 3), {
    retry: false,
    exhausted: true,
    delayMs: 0,
  });
  assert.equal(thumbnailRetryDecision(404, "not_found", 0).retry, false);
  assert.equal(thumbnailRetryDecision(422, "generation_failed", 0).retry, false);
  assert.equal(
    thumbnailRetryDecision(503, "protocol_version_mismatch", 0).retry,
    false
  );
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
