import test from "node:test";
import assert from "node:assert/strict";
import {
  FOREGROUND_ADMISSION_RETRY_LIMIT,
  ViewerGroupLoadOutcome,
} from "./command-core.mjs";

class FakeElement {
  constructor(tag = "div") {
    this.tagName = tag.toUpperCase();
    this.style = {};
    this.dataset = {};
    const classes = new Set();
    this.classList = {
      add(...names) { names.forEach((name) => classes.add(name)); },
      remove(...names) { names.forEach((name) => classes.delete(name)); },
      toggle(name, force) {
        const enabled = force === undefined ? !classes.has(name) : Boolean(force);
        if (enabled) classes.add(name);
        else classes.delete(name);
        return enabled;
      },
      contains(name) { return classes.has(name); },
    };
    this.hidden = false;
    this.clientWidth = 430;
    this.clientHeight = 800;
    this.naturalWidth = 1200;
    this.naturalHeight = 1800;
    this.children = [];
    this.replacedWith = null;
    this.listeners = new Map();
  }

  addEventListener(type, listener) {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }
  removeEventListener(type, listener) {
    this.listeners.set(
      type,
      (this.listeners.get(type) ?? []).filter((candidate) => candidate !== listener)
    );
  }
  dispatchEvent(event) {
    for (const listener of this.listeners.get(event.type) ?? []) listener(event);
  }
  setAttribute() {}
  append(...nodes) { this.children.push(...nodes); }
  replaceChildren(...nodes) { this.children = nodes; }
  replaceWith(node) { this.replacedWith = node; }
  remove() { this.removed = true; }
  async decode() { return FakeElement.decodeHook?.(this); }
}

const app = new FakeElement("main");
const hud = new FakeElement("pre");
globalThis.__MIV_RUNTIME_TEST_MODE__ = true;
globalThis.document = {
  querySelector(selector) {
    return selector === "#app" ? app : hud;
  },
  createElement(tag) {
    return new FakeElement(tag);
  },
  createElementNS(_namespace, tag) {
    return new FakeElement(tag);
  },
};
let reloadCalls = 0;
const testLocation = {
  origin: "http://127.0.0.1:8787",
  hash: "",
  href: "http://127.0.0.1:8787/",
  reload() { reloadCalls += 1; },
};
globalThis.window = {
  innerWidth: 430,
  innerHeight: 800,
  devicePixelRatio: 2,
  location: testLocation,
  addEventListener() {},
  removeEventListener() {},
};
globalThis.location = testLocation;
globalThis.requestAnimationFrame = (callback) => {
  callback(performance.now());
  return 1;
};
globalThis.cancelAnimationFrame = () => {};
const TEST_SESSION_ID = "0123456789abcdef0123456789abcdef";
const testPath = (relative = "") => relative ? `C:/miv-test/${relative}` : "C:/miv-test";
const TEST_PAGE_ADDRESS = {
  path: testPath("books/test.pdf"),
  subresource: { kind: "pdf_page", page_number: 0 },
};
const pageIdentityHeader = (address) => encodeURIComponent(JSON.stringify(address));
const imageFetch = async () => new Response(new Blob([new Uint8Array([1, 2, 3])]), {
  status: 200,
  headers: {
    "Content-Type": "image/jpeg",
    "X-mIV-Request-Id": "runtime-test",
    "X-mIV-Image-Width": "1200",
    "X-mIV-Image-Height": "1800",
    "X-mIV-Page-Render-Ms": "600",
    "X-mIV-Remote-State-Generation": "test-1",
    "X-mIV-Remote-Session": TEST_SESSION_ID,
    "X-mIV-Page-Identity": pageIdentityHeader(TEST_PAGE_ADDRESS),
  },
});
globalThis.fetch = imageFetch;

const {
  ADJUSTMENT_PANEL_TABS,
  ContainerSpreadRefreshExitReason,
  ImageViewer,
  DecodedPageUnitCache,
  LatestOnlyTaskQueue,
  LatestPageLoadQueue,
  PageDemandAdapter,
  PageResourceCache,
  RemoteAiController,
  ViewerAdjustmentPanel,
  ViewerImageUpdateExitReason,
  ViewerViewTrimPanel,
  VIEWER_MENU_MAX_ACTIONS,
  VIEWER_PANEL_TABS,
  activateFolderContainerForImage,
  applyRemoteSessionId,
  browserDoubleTapTelemetryEvent,
  commandTelemetryEvent,
  containerInitialImageIndex,
  createRemoteHomeDataRefreshCoordinator,
  createFavoriteSearchForm,
  createGridTile,
  createPinLoginInput,
  filterRemoteTags,
  favoriteSearchEmptyMessage,
  favoriteSearchHash,
  favoriteSearchResultTitle,
  gridReturnItemIdentity,
  invalidateViewerPendingLoad,
  isStreamMediaKind,
  loadFolder,
  normalizeRemoteAdjustmentValues,
  normalizeRemoteBookBookmarkList,
  normalizeRemoteColorizeParams,
  normalizeRemoteGridSortState,
  normalizeRemoteViewTrimState,
  parentContainerAddress,
  parseRoute,
  reloadApplication,
  reportContainerSpreadRefreshError,
  renderResolvedMediaOpen,
  remoteArchiveProgressText,
  remoteAiCompletionMessage,
  remoteAiProgressText,
  remoteAiPollingDelay,
  remoteBookBookmarkDisplayPage,
  remoteBookBookmarkTargetEntryIndex,
  rootOpenReturnHash,
  resolveLegacyImageOpenRoute,
  resolveMediaOpenRoute,
  selectRecoverableRemoteAiJob,
  selectRecoverableRemoteArchiveJob,
  setViewTrimSpreadSeparate,
  setRuntimeTestErrorObserver,
  showUnsupportedRemoteEntryNotice,
  tagBrowsePresentation,
  tagItemsEmptyMessage,
  tagItemsHash,
  tagItemsResultTitle,
  thumbnailAddressForEntry,
  thumbnailRequestQueryForEntry,
  unsupportedRemoteEntryMessage,
  videoFileTargetIndex,
  viewTrimSpreadControlKeys,
  viewerMenuDefinitions,
  viewerImageUpdateContextExitReason,
} = await import("./app.js");

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

test("PIN login uses a text-capable mobile keyboard without text correction", () => {
  const pin = createPinLoginInput();
  assert.equal(pin.type, "password");
  assert.equal(pin.autocomplete, "current-password");
  assert.notEqual(pin.inputMode, "numeric");
  assert.equal(pin.autocapitalize, "none");
  assert.equal(pin.spellcheck, false);
  assert.equal(pin.placeholder, "6文字以上の PIN");
});

const viewTrimState = () => ({
  apply_mode: "book",
  book_settings: {
    enabled: true,
    spread_separate: false,
    single: { left: 0.01, top: 0.02, right: 0.03, bottom: 0.04 },
    spread_linked: { top: 0.05, bottom: 0.06, inner: 0.07, outer: 0.08 },
    spread_left: { left: 0, top: 0, right: 0, bottom: 0 },
    spread_right: { left: 0, top: 0, right: 0, bottom: 0 },
  },
});

test("view trim keeps the core book shape and switches spread controls without page state", () => {
  const normalized = normalizeRemoteViewTrimState(viewTrimState());
  assert.deepEqual(viewTrimSpreadControlKeys(normalized.book_settings, false), ["single"]);
  assert.deepEqual(viewTrimSpreadControlKeys(normalized.book_settings, true), ["spread_linked"]);

  const separate = setViewTrimSpreadSeparate(normalized, true);
  assert.deepEqual(viewTrimSpreadControlKeys(separate.book_settings, true), [
    "spread_left",
    "spread_right",
  ]);
  assert.deepEqual(separate.book_settings.spread_linked, normalized.book_settings.spread_linked);
  assert.equal("page_override" in separate, false);
});

test("sort state accepts only server-provided options and keeps a visible lock reason", () => {
  const sort = normalizeRemoteGridSortState({
    selected: "DateDesc",
    options: [
      { value: "FileName", label: "ファイル名順", short_label: "名前" },
      { value: "DateDesc", label: "日付順（新しい順）", short_label: "日付↓" },
    ],
    locked_reason: "本として表示中は名前順固定です",
  });
  assert.equal(sort.selected, "DateDesc");
  assert.equal(sort.options.length, 2);
  assert.equal(sort.locked_reason, "本として表示中は名前順固定です");
  assert.equal(normalizeRemoteGridSortState({ selected: "Numeric", options: [] }), null);
});

test("session identity revocation tolerates a video viewer without page-load invalidation", () => {
  assert.doesNotThrow(() => invalidateViewerPendingLoad({ isVideoStreamViewer: true }));

  let invalidations = 0;
  invalidateViewerPendingLoad({
    invalidatePendingLoad() { invalidations += 1; },
  });
  assert.equal(invalidations, 1);
});

test("continuous video navigation stops at the end or wraps to the first video", () => {
  assert.equal(videoFileTargetIndex(0, 3, 1, false), 1);
  assert.equal(videoFileTargetIndex(2, 3, 1, false), -1);
  assert.equal(videoFileTargetIndex(2, 3, 1, true), 0);
});
test("open telemetry records the requested kind, media kind, and reached route", () => {
  const event = commandTelemetryEvent(
    {
      name: "open",
      payload: { kind: "media", mediaKind: "video" },
    },
    { detail: "grid_tile", openRoute: "media_video" },
    "mouse",
    "grid",
    true
  );

  assert.equal(event.payload.kind, "media");
  assert.equal(event.mediaKind, "video");
  assert.equal(event.open_route, "media_video");
  assert.equal(event.handled, true);

  const rejected = commandTelemetryEvent(
    { name: "open", payload: { kind: "image" } },
    { openRoute: "legacy_image_rejected" },
    "keyboard",
    "grid",
    false
  );
  assert.equal(rejected.payload.kind, "image");
  assert.equal(rejected.mediaKind, null);
  assert.equal(rejected.open_route, "legacy_image_rejected");
  assert.equal(rejected.handled, false);
});

test("browser double-tap telemetry keeps the recognized observation", () => {
  const event = browserDoubleTapTelemetryEvent({
    decision: "pair_recognized",
    elapsedMs: 222.34,
    distancePx: 5.678,
    isDoubleTap: true,
    suppressed: false,
    excluded: false,
    exclusionReason: null,
    cancelable: true,
  }, 7);

  assert.deepEqual(event, {
    type: "browser_double_tap",
    action: "suppression_decision",
    decision: "pair_recognized",
    tap_pair_sequence: 7,
    previous_tap_elapsed_ms: 222.3,
    previous_tap_distance_px: 5.7,
    recognized_double_tap: true,
    suppressed: false,
    excluded: false,
    exclusion_reason: null,
    event_cancelable: true,
  });
});

test("favorite search route preserves encoded query and kind", () => {
  for (const kind of ["all", "folder", "zip", "pdf"]) {
    const query = "表紙 / final% -draft";
    assert.deepEqual(parseRoute(favoriteSearchHash(query, kind)), {
      kind: "search",
      searchKind: kind,
      query,
    });
  }
});

test("drive list uses the core-owned collection route", () => {
  assert.deepEqual(parseRoute("#collection/drive_list"), {
    kind: "collection",
    collectionKind: "drive_list",
    value: "",
  });
});

test("favorite search distinguishes empty, disabled, and unavailable states", () => {
  assert.equal(
    favoriteSearchEmptyMessage("ready", 0),
    "一致するフォルダ・ZIP・PDF はありませんでした。"
  );
  assert.match(favoriteSearchEmptyMessage("disabled", 0), /コンテナ索引が設定されていません/);
  assert.match(favoriteSearchEmptyMessage("unavailable", 0), /まだ利用できません/);
  assert.equal(favoriteSearchEmptyMessage("disabled", 1), "");
});

test("favorite search result title keeps the searched words on screen", () => {
  assert.equal(favoriteSearchResultTitle("表紙"), "「表紙」の検索結果");
  assert.equal(favoriteSearchResultTitle("  表紙  "), "「表紙」の検索結果");
  assert.equal(favoriteSearchResultTitle(""), "検索結果");
  // 丸めるのは表示だけ。文字単位で数えるので、サロゲートペアを割らない。
  const long = "あ".repeat(40);
  assert.equal(favoriteSearchResultTitle(long), `「${"あ".repeat(30)}…」の検索結果`);
  assert.equal(
    favoriteSearchResultTitle("🐈".repeat(31)),
    `「${"🐈".repeat(30)}…」の検索結果`
  );
});

test("favorite search runs only when the form is submitted", () => {
  const submissions = [];
  const controls = createFavoriteSearchForm(
    { query: "前回の語句", kind: "zip" },
    (value) => submissions.push(value)
  );
  assert.equal(controls.query.value, "前回の語句");
  assert.equal(controls.kind.value, "zip");

  controls.query.value = "次の語句";
  controls.query.dispatchEvent({ type: "input" });
  controls.kind.value = "pdf";
  controls.kind.dispatchEvent({ type: "change" });
  assert.deepEqual(submissions, []);

  let prevented = 0;
  controls.form.dispatchEvent({
    type: "submit",
    preventDefault() { prevented += 1; },
  });
  assert.equal(prevented, 1);
  assert.deepEqual(submissions, [{ query: "次の語句", kind: "pdf" }]);
});

test("tag route preserves encoded tag and every item kind", () => {
  for (const kind of [
    "all",
    "folder",
    "image",
    "video",
    "audio",
    "zip",
    "pdf",
    "archive",
  ]) {
    const tag = "表紙 / 仕上げ% #候補";
    assert.deepEqual(parseRoute(tagItemsHash(tag, kind)), {
      kind: "tag",
      tagKind: kind,
      tag,
    });
  }
});

test("tag filtering is local partial matching and switches to one flat section", () => {
  const all = [
    { name: "Landscape", count: 2 },
    { name: "夜景", count: 4 },
    { name: "風景", count: 6 },
  ];
  assert.deepEqual(filterRemoteTags(all, "景"), [all[1], all[2]]);
  assert.deepEqual(filterRemoteTags(all, "ｌａｎｄ"), [all[0]]);

  const filtered = tagBrowsePresentation({ all }, "景");
  assert.equal(filtered.mode, "flat");
  assert.equal(filtered.sections.length, 1);
  assert.deepEqual(filtered.sections[0].choices, [all[1], all[2]]);

  const sections = tagBrowsePresentation({
    pinned: [all[0]],
    recent: [all[1]],
    popular: [all[2]],
    all,
  }, "");
  assert.equal(sections.mode, "sections");
  assert.deepEqual(sections.sections.map((section) => section.title), [
    "ピン留め",
    "最近",
    "よく使う",
  ]);
});

test("tag item states and title keep the requested tag visible", () => {
  assert.equal(
    tagItemsEmptyMessage("ready", 0),
    "このタグの項目は見つかりませんでした。"
  );
  assert.equal(tagItemsEmptyMessage("empty", 0), "タグはまだ 1 つもありません。");
  assert.equal(tagItemsEmptyMessage("unavailable", 0), "タグをまだ利用できません。");
  assert.equal(tagItemsEmptyMessage("unavailable", 1), "");
  assert.equal(tagItemsResultTitle("風景"), "「#風景」の項目");
  assert.equal(tagItemsResultTitle("#風景"), "「#風景」の項目");
  const long = "あ".repeat(40);
  assert.equal(
    tagItemsResultTitle(long),
    `「#${"あ".repeat(29)}…」の項目`
  );
});

test("legacy image telemetry resolves rejection and folder route without entryIndex", () => {
  assert.equal(
    resolveLegacyImageOpenRoute({ kind: "image", imageIndex: -1 }, 3, false),
    "legacy_image_rejected"
  );
  assert.equal(
    resolveLegacyImageOpenRoute({ kind: "image", imageIndex: 1 }, 3, false),
    "folder_image"
  );
  assert.equal(
    resolveLegacyImageOpenRoute({ kind: "image", imageIndex: 1 }, 3, true),
    "collection_image"
  );
});

test("a folder-list video uses its absorbed sidecar as the thumbnail source", () => {
  const video = {
    path: testPath("movies/clip.mp4"),
    subresource: { kind: "file" },
  };
  const sidecar = {
    path: testPath("movies/clip.jpg"),
    subresource: { kind: "file" },
  };
  assert.equal(
    thumbnailAddressForEntry({ address: video, thumbnail_address: sidecar }),
    sidecar
  );
  assert.equal(thumbnailAddressForEntry({ address: video }), video);
  assert.deepEqual(
    thumbnailRequestQueryForEntry(
      { address: video, thumbnail_address: sidecar },
      { w: 256, epoch: 7 }
    ),
    {
      path: video.path,
      w: 256,
      epoch: 7,
      thumbnail_source_path: sidecar.path,
    }
  );
});

test("grid return identity matches collection and addressed forms of the same item", () => {
  const address = {
    path: testPath("album/child"),
    subresource: { kind: "file" },
  };
  assert.equal(
    gridReturnItemIdentity({
      kind: "folder",
      path: address.path,
    }),
    gridReturnItemIdentity({ kind: "folder", address })
  );
});

test("book bookmark rows preserve DB order and keep hint separate from resolved target", () => {
  const address = (pageNumber) => ({
    path: testPath("book.pdf"),
    subresource: { kind: "pdf_page", page_number: pageNumber },
  });
  const context = {
    path: testPath("book.pdf"),
    subresource: { kind: "file" },
  };
  const list = normalizeRemoteBookBookmarkList({
    supported: true,
    rows: [
      {
        id: 20,
        title: "後半",
        page_index_hint: 99,
        page_label: "100 ページ",
        target: {
          address: address(4),
          context_address: context,
          item_index: 4,
        },
      },
      {
        id: 10,
        title: null,
        page_index_hint: 7,
        page_label: "missing.jpg",
        target: null,
      },
    ],
  });

  assert.equal(list.supported, true);
  assert.deepEqual(list.rows.map((row) => row.id), [20, 10]);
  assert.equal(remoteBookBookmarkDisplayPage(list.rows[0]), 5);
  assert.equal(remoteBookBookmarkDisplayPage(list.rows[1]), 8);
  assert.equal(list.rows[1].target, null);
  assert.equal(
    remoteBookBookmarkTargetEntryIndex(
      [{ kind: "image", address: address(3) }, { kind: "image", address: address(4) }],
      list.rows[0].target.address
    ),
    1
  );
  assert.equal(remoteBookBookmarkTargetEntryIndex([], list.rows[0].target.address), -1);
});


test("container opening resumes the matching page and otherwise falls back safely", () => {
  const page = (pageNumber) => ({
    kind: "image",
    address: {
      path: testPath("book.pdf"),
      subresource: { kind: "pdf_page", page_number: pageNumber },
    },
  });
  const images = [page(0), page(1), page(2)];

  assert.equal(containerInitialImageIndex({
    openMode: "resume_page",
    resumePage: images[1].address,
    images,
  }), 1);
  assert.equal(containerInitialImageIndex({
    openMode: "resume_page",
    resumePage: page(20).address,
    images,
  }), 0);
  assert.equal(containerInitialImageIndex({
    openMode: "first_page",
    resumePage: images[2].address,
    images,
  }), 0);
  assert.equal(containerInitialImageIndex({
    openMode: "grid",
    resumePage: images[1].address,
    images,
  }), -1);
});

test("a collection root returns to its list without exposing a root kind", () => {
  assert.equal(rootOpenReturnHash({
    hasCollection: true,
    atFavoriteRoot: true,
    collectionHash: "#collection/rating/5",
    fallbackHash: "#folder/root",
  }), "#collection/rating/5");
  assert.equal(rootOpenReturnHash({
    hasCollection: true,
    atFavoriteRoot: false,
    collectionHash: "#collection/rating/5",
    fallbackHash: "#folder/favorite/books",
  }), "#folder/favorite/books");
});

test("every iPhone viewer menu page stays within the fixed action limit", () => {
  for (const hasContainer of [false, true]) {
    const definitions = viewerMenuDefinitions({ hasContainer, barsVisible: true });
    for (const [page, definition] of Object.entries(definitions)) {
      assert.ok(
        definition.actions.length <= VIEWER_MENU_MAX_ACTIONS,
        `${page} has ${definition.actions.length} actions`
      );
    }
    const main = definitions.main.actions;
    const mainNames = main.map(([name]) => name);
    assert.equal(mainNames.includes("prev_page"), false);
    assert.equal(mainNames.includes("next_page"), false);
    assert.equal(mainNames.includes("zoom_in"), false);
    assert.equal(mainNames.includes("zoom_out"), false);
    assert.equal(
      main.filter(([, , , payload]) => payload?.menuPage === "rating").length,
      1
    );
  }
});

test("viewer display menu contains only controls for the current view", () => {
  const definitions = viewerMenuDefinitions({
    hasContainer: true,
    barsVisible: true,
  });
  const displayEntry = definitions.main.actions.find(
    ([, , , payload]) => payload?.menuPage === "display"
  );
  assert.deepEqual(displayEntry.slice(1, 3), ["表示", "フィット / 原寸"]);
  assert.equal(definitions.display.title, "表示");
  assert.deepEqual(
    definitions.display.actions.map(([name, label]) => [name, label]),
    [
      ["menu_page_back", "操作メニューへ戻る"],
      ["zoom_reset", "ズームを戻す"],
      ["fit_page", "全体フィット"],
      ["fit_width", "幅フィット"],
      ["fit_original", "原寸 (100%)"],
    ]
  );
});

test("still-image panel exposes the agreed tabs in desktop order", () => {
  assert.deepEqual(
    VIEWER_PANEL_TABS.map(({ id, label }) => [id, label]),
    [
      ["functions", "機能"],
      ["adjustment", "画像補正"],
      ["view_trim", "表示トリム"],
      ["bookmarks", "ブックマーク"],
    ]
  );
});

test("adjustment panel exposes only implemented sections in desktop order", () => {
  assert.deepEqual(
    ADJUSTMENT_PANEL_TABS.map(({ id, label }) => [id, label]),
    [
      ["color_tone", "色調"],
      ["ai", "AI"],
      ["colorize", "カラー化"],
    ]
  );
});

test("adjustment tabs keep shared actions outside and preserve range pointer handlers", () => {
  const panel = new ViewerAdjustmentPanel();
  assert.deepEqual(panel.root.children, [
    panel.targetRow,
    panel.scopeFieldset,
    panel.tabList,
    panel.colorTonePanel,
    panel.aiSection,
    panel.colorizeSection,
    panel.resetButton,
    panel.status,
  ]);
  assert.equal(panel.selectedTab, "color_tone");
  assert.equal(panel.colorTonePanel.hidden, false);
  assert.equal(panel.aiSection.hidden, true);
  assert.equal(panel.colorizeSection.hidden, true);

  panel.selectTab("ai");
  assert.equal(panel.colorTonePanel.hidden, true);
  assert.equal(panel.aiSection.hidden, false);
  assert.equal(panel.colorizeSection.hidden, true);
  for (const controls of [panel.controls, panel.colorizeControls]) {
    for (const { input } of controls.values()) {
      assert.ok(input.listeners.has("pointerdown"));
      assert.ok(input.listeners.has("pointermove"));
      assert.ok(input.listeners.has("pointerup"));
    }
  }
});

test("adjustment touch cancellation restores the starting value without committing", () => {
  const panel = new ViewerAdjustmentPanel();
  let previewCalls = 0;
  let commitCalls = 0;
  panel.queuePreview = () => { previewCalls += 1; };
  panel.commitCurrent = async () => { commitCalls += 1; };

  const cases = [
    {
      input: panel.controls.get("brightness").input,
      read: () => panel.values.brightness,
    },
    {
      input: panel.colorizeControls.get("mono_tolerance").input,
      read: () => panel.values.colorize.mono_tolerance,
    },
  ];
  let pointerId = 10;
  for (const target of cases) {
    const input = target.input;
    input.disabled = false;
    input.getBoundingClientRect = () => ({ left: 0, width: 200 });
    input.focus = () => {};
    input.setPointerCapture = () => {};
    input.hasPointerCapture = () => false;
    input.releasePointerCapture = () => {};
    const startingValue = target.read();
    const previewsBefore = previewCalls;
    let prevented = 0;
    const pointerEvent = (type, clientX, cancelable = true) => ({
      type,
      pointerId,
      pointerType: "touch",
      isPrimary: true,
      button: 0,
      clientX,
      clientY: 20,
      cancelable,
      preventDefault() { prevented += 1; },
      stopPropagation() {},
    });

    input.dispatchEvent(pointerEvent("pointerdown", 80));
    input.dispatchEvent(pointerEvent("pointermove", 120));
    assert.notEqual(target.read(), startingValue);
    input.dispatchEvent(pointerEvent("pointercancel", 120, false));

    assert.equal(target.read(), startingValue);
    assert.equal(panel.dirty, false);
    assert.equal(previewCalls - previewsBefore, 2);
    assert.equal(commitCalls, 0);
    assert.equal(prevented, 0);
    pointerId += 1;
  }
});

test("adjustment horizontal touch drag still previews and commits", () => {
  const panel = new ViewerAdjustmentPanel();
  const input = panel.controls.get("brightness").input;
  input.getBoundingClientRect = () => ({ left: 0, width: 200 });
  input.focus = () => {};
  input.setPointerCapture = () => {};
  input.hasPointerCapture = () => true;
  input.releasePointerCapture = () => {};
  let previewCalls = 0;
  let commitCalls = 0;
  let prevented = 0;
  panel.queuePreview = () => { previewCalls += 1; };
  panel.commitCurrent = async () => { commitCalls += 1; };
  const pointerEvent = (type, clientX) => ({
    type,
    pointerId: 20,
    pointerType: "touch",
    isPrimary: true,
    button: 0,
    clientX,
    clientY: 20,
    cancelable: true,
    preventDefault() { prevented += 1; },
    stopPropagation() {},
  });

  input.dispatchEvent(pointerEvent("pointerdown", 80));
  input.dispatchEvent(pointerEvent("pointermove", 120));
  input.dispatchEvent(pointerEvent("pointerup", 120));

  assert.notEqual(panel.values.brightness, 0);
  assert.equal(panel.dirty, false);
  assert.equal(previewCalls, 1);
  assert.equal(commitCalls, 1);
  assert.equal(prevented, 0);
});

test("adjustment pointer tap is absolute while drag remains relative", () => {
  const panel = new ViewerAdjustmentPanel();
  let previewCalls = 0;
  let commitCalls = 0;
  panel.queuePreview = () => { previewCalls += 1; };
  panel.commitCurrent = async () => { commitCalls += 1; };
  const cases = [
    {
      input: panel.controls.get("brightness").input,
      read: () => panel.values.brightness,
      tapValue: 50,
    },
    {
      input: panel.colorizeControls.get("mono_tolerance").input,
      read: () => panel.values.colorize.mono_tolerance,
      tapValue: 48,
    },
  ];
  let pointerId = 30;
  for (const target of cases) {
    const input = target.input;
    input.disabled = false;
    input.getBoundingClientRect = () => ({ left: 0, width: 200 });
    input.focus = () => {};
    input.setPointerCapture = () => {};
    input.hasPointerCapture = () => false;
    input.releasePointerCapture = () => {};
    const pointerEvent = (type, clientX) => ({
      type,
      pointerId,
      pointerType: "mouse",
      isPrimary: true,
      button: 0,
      clientX,
      clientY: 20,
      cancelable: true,
      preventDefault() {},
      stopPropagation() {},
    });
    input.dispatchEvent(pointerEvent("pointerdown", 150));
    input.dispatchEvent(pointerEvent("pointerup", 153));
    assert.equal(target.read(), target.tapValue);
    pointerId += 1;
  }
  assert.equal(previewCalls, 2);
  assert.equal(commitCalls, 2);

  const input = panel.controls.get("brightness").input;
  input.dispatchEvent({
    type: "pointerdown",
    pointerId,
    pointerType: "mouse",
    isPrimary: true,
    button: 0,
    clientX: 180,
    clientY: 20,
    cancelable: true,
    preventDefault() {},
    stopPropagation() {},
  });
  input.dispatchEvent({
    type: "pointerup",
    pointerId,
    pointerType: "mouse",
    clientX: 200,
    clientY: 20,
    cancelable: true,
    preventDefault() {},
    stopPropagation() {},
  });
  assert.equal(panel.values.brightness, 70, "drag adds travel to the starting value");
});

test("view trim pointer tap is absolute, drag is relative, and cancellation restores", () => {
  const panel = new ViewerViewTrimPanel();
  panel.serverState = normalizeRemoteViewTrimState(viewTrimState());
  let commitCalls = 0;
  let prevented = 0;
  panel.commit = async () => { commitCalls += 1; };
  const row = panel.marginControl("single", "left");
  const input = row.children[1];
  input.getBoundingClientRect = () => ({ left: 0, width: 200 });
  input.focus = () => {};
  input.setPointerCapture = () => {};
  input.hasPointerCapture = () => false;
  input.releasePointerCapture = () => {};
  let pointerId = 40;
  const pointerEvent = (type, clientX) => ({
    type,
    pointerId,
    pointerType: "touch",
    isPrimary: true,
    button: 0,
    clientX,
    clientY: 20,
    cancelable: type !== "pointercancel",
    preventDefault() { prevented += 1; },
    stopPropagation() {},
  });

  input.dispatchEvent(pointerEvent("pointerdown", 100));
  input.dispatchEvent(pointerEvent("pointerup", 103));
  assert.equal(panel.serverState.book_settings.single.left, 0.1);
  assert.equal(commitCalls, 1);

  pointerId += 1;
  input.dispatchEvent(pointerEvent("pointerdown", 150));
  input.dispatchEvent(pointerEvent("pointermove", 170));
  input.dispatchEvent(pointerEvent("pointerup", 170));
  assert.equal(panel.serverState.book_settings.single.left, 0.12);
  assert.equal(commitCalls, 2);

  pointerId += 1;
  input.dispatchEvent(pointerEvent("pointerdown", 120));
  input.dispatchEvent(pointerEvent("pointermove", 180));
  assert.notEqual(panel.serverState.book_settings.single.left, 0.12);
  input.dispatchEvent(pointerEvent("pointercancel", 180));
  assert.equal(panel.serverState.book_settings.single.left, 0.12);
  assert.equal(commitCalls, 2);
  assert.equal(prevented, 0, "touch must remain available to the vertical pan owner");
});

test("adjustment preview keeps one request in flight and coalesces to the latest value", async () => {
  let releaseFirst;
  const firstGate = new Promise((resolve) => { releaseFirst = resolve; });
  const started = [];
  const completed = [];
  let active = 0;
  let maxActive = 0;
  const queue = new LatestOnlyTaskQueue(async (value) => {
    active += 1;
    maxActive = Math.max(maxActive, active);
    started.push(value);
    if (value === 1) await firstGate;
    completed.push(value);
    active -= 1;
  });

  const first = queue.enqueue(1);
  await Promise.resolve();
  const second = queue.enqueue(2);
  const third = queue.enqueue(3);
  releaseFirst();
  for (let attempt = 0; attempt < 20 && completed.length < 2; attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }

  assert.deepEqual(started, [1, 3]);
  assert.deepEqual(completed, [1, 3]);
  assert.equal(maxActive, 1);
  assert.deepEqual(await Promise.all([first, second, third]), [
    { outcome: ViewerGroupLoadOutcome.APPLIED },
    { outcome: ViewerGroupLoadOutcome.SUPERSEDED },
    { outcome: ViewerGroupLoadOutcome.APPLIED },
  ]);
});

test("container-style latest-only refresh always displays the last grouping", async () => {
  const firstGate = deferred();
  const displayedGroups = [];
  const queue = new LatestOnlyTaskQueue(async ({ mode, groups }) => {
    if (mode === "single") await firstGate.promise;
    displayedGroups.push(groups);
  });

  const single = queue.enqueue({ mode: "single", groups: [[0], [1], [2], [3]] });
  await Promise.resolve();
  const staleSpread = queue.enqueue({ mode: "ltr", groups: [[0, 1], [2, 3]] });
  const latestSpread = queue.enqueue({ mode: "rtl", groups: [[1, 0], [3, 2]] });
  firstGate.resolve();

  assert.deepEqual(await Promise.all([single, staleSpread, latestSpread]), [
    { outcome: ViewerGroupLoadOutcome.APPLIED },
    { outcome: ViewerGroupLoadOutcome.SUPERSEDED },
    { outcome: ViewerGroupLoadOutcome.APPLIED },
  ]);
  assert.deepEqual(displayedGroups, [
    [[0], [1], [2], [3]],
    [[1, 0], [3, 2]],
  ]);
});

test("latest-only refresh distinguishes failure from supersede", async () => {
  const firstGate = deferred();
  const queue = new LatestOnlyTaskQueue(async (value) => {
    if (value === "active") await firstGate.promise;
    if (value === "failure") throw new Error("container refresh failed");
  });

  const active = queue.enqueue("active");
  await Promise.resolve();
  const superseded = queue.enqueue("superseded");
  const failure = queue.enqueue("failure");
  firstGate.resolve();

  assert.deepEqual(await active, { outcome: ViewerGroupLoadOutcome.APPLIED });
  assert.deepEqual(await superseded, { outcome: ViewerGroupLoadOutcome.SUPERSEDED });
  assert.deepEqual(await failure, {
    outcome: ViewerGroupLoadOutcome.FAILED,
    message: "container refresh failed",
  });
});

test("container refresh records the internal exception but returns only Japanese feedback", async () => {
  const errors = [];
  setRuntimeTestErrorObserver((event) => errors.push(event));
  const internal = new TypeError("cancelPendingCenterTap is not a function");
  const request = { renderTrigger: "spread_refresh" };
  const queue = new LatestOnlyTaskQueue(
    async () => { throw internal; },
    (error, failedRequest) => reportContainerSpreadRefreshError(
      error,
      failedRequest,
      {
        reason: ContainerSpreadRefreshExitReason.UNEXPECTED_ERROR,
        stage: "viewer_update",
      }
    )
  );
  try {
    const result = await queue.enqueue(request);
    assert.deepEqual(result, {
      outcome: ViewerGroupLoadOutcome.FAILED,
      reason: ContainerSpreadRefreshExitReason.UNEXPECTED_ERROR,
      message: "見開き表示を更新できませんでした。",
    });
    assert.doesNotMatch(result.message, /cancelPendingCenterTap|TypeError|not a function/);
    assert.equal(errors.length, 1);
    assert.equal(errors[0].category, "spread_refresh_error");
    assert.equal(errors[0].error, internal);
    assert.match(errors[0].error.stack, /cancelPendingCenterTap is not a function/);
    assert.deepEqual(errors[0].extra, {
      reason: ContainerSpreadRefreshExitReason.UNEXPECTED_ERROR,
      stage: "viewer_update",
      render_trigger: "spread_refresh",
    });
  } finally {
    setRuntimeTestErrorObserver(null);
  }
});

test("viewer update context exits keep each pre-load reason distinct", () => {
  assert.equal(
    viewerImageUpdateContextExitReason({ viewerMatches: false }),
    ViewerImageUpdateExitReason.VIEWER_CHANGED_BEFORE_GROUP_LOAD
  );
  assert.equal(
    viewerImageUpdateContextExitReason({ sessionMatches: false }),
    ViewerImageUpdateExitReason.SESSION_CHANGED_BEFORE_GROUP_LOAD
  );
  assert.equal(
    viewerImageUpdateContextExitReason({ cacheEpochMatches: false }),
    ViewerImageUpdateExitReason.CACHE_EPOCH_CHANGED_BEFORE_GROUP_LOAD
  );
  assert.equal(
    viewerImageUpdateContextExitReason({ groupMatches: false }),
    ViewerImageUpdateExitReason.GROUP_CHANGED_BEFORE_GROUP_LOAD
  );
  assert.equal(viewerImageUpdateContextExitReason(), null);
});

test("identical container-style refreshes join the active owner", async () => {
  const gate = deferred();
  let runs = 0;
  const sameTask = (left, right) =>
    left.viewer === right.viewer &&
    left.address === right.address &&
    left.forceSinglePage === right.forceSinglePage;
  const queue = new LatestOnlyTaskQueue(async () => {
    runs += 1;
    await gate.promise;
  }, () => {}, sameTask);
  const viewer = {};
  const first = queue.enqueue({ viewer, address: "book", forceSinglePage: false });
  const duplicate = queue.enqueue({ viewer, address: "book", forceSinglePage: false });

  assert.strictEqual(duplicate, first);
  gate.resolve();
  assert.deepEqual(await Promise.all([first, duplicate]), [
    { outcome: ViewerGroupLoadOutcome.APPLIED },
    { outcome: ViewerGroupLoadOutcome.APPLIED },
  ]);
  assert.equal(runs, 1);
});

test("page load queue emits one busy interval across pending replacements", async () => {
  const started = [];
  const busy = [];
  const events = [];
  let releaseFirst;
  const firstGate = new Promise((resolve) => { releaseFirst = resolve; });
  const queue = new LatestPageLoadQueue(
    async (value) => {
      started.push(value);
      events.push(`run:${value}`);
      if (value === 1) await firstGate;
      return { outcome: ViewerGroupLoadOutcome.APPLIED };
    },
    () => {},
    (value) => {
      busy.push(value);
      events.push(`busy:${value}`);
    }
  );

  const first = queue.request(1);
  const second = queue.request(2);
  const third = queue.request(3);
  assert.deepEqual(started, [1]);
  assert.deepEqual(busy, [true]);
  assert.deepEqual(events, ["busy:true", "run:1"]);
  releaseFirst();
  assert.deepEqual(await Promise.all([first, second, third]), [
    { outcome: ViewerGroupLoadOutcome.SUPERSEDED },
    { outcome: ViewerGroupLoadOutcome.SUPERSEDED },
    { outcome: ViewerGroupLoadOutcome.APPLIED },
  ]);
  assert.deepEqual(started, [1, 3]);
  assert.deepEqual(busy, [true, false]);
  assert.deepEqual(events, ["busy:true", "run:1", "run:3", "busy:false"]);
});

test("page load queue clear resolves active and pending requests as superseded", async () => {
  let releaseActive;
  const activeGate = new Promise((resolve) => { releaseActive = resolve; });
  const discarded = [];
  const queue = new LatestPageLoadQueue(
    async (value) => {
      if (value === "active") await activeGate;
      return { outcome: ViewerGroupLoadOutcome.APPLIED };
    },
    () => {},
    () => {},
    (value, reason) => discarded.push({ value, reason })
  );
  const active = queue.request("active");
  const pending = queue.request("pending");

  queue.clear();
  releaseActive();
  assert.deepEqual(await Promise.all([active, pending]), [
    { outcome: ViewerGroupLoadOutcome.SUPERSEDED },
    { outcome: ViewerGroupLoadOutcome.SUPERSEDED },
  ]);
  assert.deepEqual(discarded, [
    { value: "pending", reason: "queue_cleared" },
  ]);
});

test("page load queue rejects a current run error but hides an old failed result", async () => {
  const expected = new Error("queue worker failed");
  const rejecting = new LatestPageLoadQueue(async () => { throw expected; });
  await assert.rejects(rejecting.request("current"), expected);

  let releaseOld;
  const oldGate = new Promise((resolve) => { releaseOld = resolve; });
  const queue = new LatestPageLoadQueue(async (value) => {
    if (value === "old") {
      await oldGate;
      return {
        outcome: ViewerGroupLoadOutcome.FAILED,
        message: "old request failed",
      };
    }
    return { outcome: ViewerGroupLoadOutcome.APPLIED };
  });
  const old = queue.request("old");
  const current = queue.request("current");
  releaseOld();
  assert.deepEqual(await Promise.all([old, current]), [
    { outcome: ViewerGroupLoadOutcome.SUPERSEDED },
    { outcome: ViewerGroupLoadOutcome.APPLIED },
  ]);
});

function adapterPageRequest(cacheKey) {
  return {
    cacheKey,
    cacheable: true,
    url: "/api/page?generation=test-1&w=1024",
    address: TEST_PAGE_ADDRESS,
    remoteStateGeneration: "test-1",
    remoteSessionId: TEST_SESSION_ID,
    width: 1024,
  };
}

function pendingPageFetch(started) {
  return (request, signal, prefetch) => {
    started.push({ request, signal, prefetch });
    return new Promise((_resolve, reject) => {
      signal.addEventListener("abort", () => {
        const error = new Error("Aborted");
        error.name = "AbortError";
        reject(error);
      }, { once: true });
    });
  };
}

async function waitForAdapterIdle(adapter) {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (adapter.jobs.size === 0) return;
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  assert.equal(adapter.jobs.size, 0, "page jobs did not settle");
}

test("display leases are all registered before the first page fetch begins", () => {
  const cache = new PageResourceCache(8);
  let adapter;
  const protectedAtStart = [];
  adapter = new PageDemandAdapter({
    cache,
    wirePrefix: "test",
    fetchResource: (_request, _signal, _prefetch) => {
      protectedAtStart.push(adapter.coordinator.protectedKeyIds());
      return new Promise(() => {});
    },
    postDemand: async () => ({}),
  });
  const requests = [adapterPageRequest("left"), adapterPageRequest("right")];
  adapter.openDisplay({
    requestId: "spread",
    groupKey: "spread-group",
    requests,
  });

  assert.deepEqual(protectedAtStart, [
    ["left", "right"],
    ["left", "right"],
  ]);
  adapter.releaseDisplay("spread");
});

test("releasing one overlapping display does not cancel their shared page job", async () => {
  const started = [];
  const demandBodies = [];
  const cache = new PageResourceCache(8);
  const adapter = new PageDemandAdapter({
    cache,
    wirePrefix: "test",
    fetchResource: pendingPageFetch(started),
    postDemand: async (body) => { demandBodies.push(body); },
  });
  const request = adapterPageRequest("shared");
  adapter.openDisplay({ requestId: "first", groupKey: "g1", requests: [request] });
  adapter.openDisplay({ requestId: "second", groupKey: "g2", requests: [request] });

  adapter.releaseDisplay("first");
  await Promise.resolve();
  assert.equal(started[0].signal.aborted, false);
  assert.deepEqual(demandBodies, []);

  adapter.releaseDisplay("second");
  await Promise.resolve();
  assert.equal(started[0].signal.aborted, true);
  assert.deepEqual(demandBodies, [{
    promote: [],
    release: [{ job: "test-j1", cause: "no_demand" }],
  }]);
});

test("a planned prefetch promotes once and batches the display identity", async () => {
  const started = [];
  const demandBodies = [];
  const cache = new PageResourceCache(8);
  const adapter = new PageDemandAdapter({
    cache,
    wirePrefix: "test",
    fetchResource: pendingPageFetch(started),
    postDemand: async (body) => { demandBodies.push(body); },
  });
  const request = adapterPageRequest("planned");
  adapter.setPlan([request]);
  adapter.openDisplay({ requestId: "reader-1", groupKey: "g1", requests: [request] });
  adapter.openDisplay({ requestId: "reader-2", groupKey: "g2", requests: [request] });
  await Promise.resolve();

  assert.equal(started.length, 1);
  assert.equal(started[0].prefetch, true);
  assert.match(started[0].request.url, /[?&]job=test-j1(?:&|$)/);
  assert.deepEqual(demandBodies, [{
    promote: [{ job: "test-j1", display: "test-dreader-1" }],
    release: [],
  }]);

  adapter.releaseDisplay("reader-1");
  adapter.releaseDisplay("reader-2");
  adapter.setPlan([]);
});

test("only jobs whose final demand disappears are released in one tick", async () => {
  const started = [];
  const demandBodies = [];
  const cache = new PageResourceCache(8);
  const adapter = new PageDemandAdapter({
    cache,
    wirePrefix: "test",
    fetchResource: pendingPageFetch(started),
    postDemand: async (body) => { demandBodies.push(body); },
  });
  const first = adapterPageRequest("first");
  const kept = adapterPageRequest("kept");
  adapter.setPlan([first, kept]);
  adapter.setPlan([kept]);
  await Promise.resolve();

  assert.equal(started[0].signal.aborted, true);
  assert.equal(started[1].signal.aborted, false);
  assert.deepEqual(demandBodies, [{
    promote: [],
    release: [{ job: "test-j1", cause: "no_demand" }],
  }]);
  adapter.setPlan([]);
});

test("Cancelled 409 is terminal and never enters the busy retry loop", async () => {
  let attempts = 0;
  const cache = new PageResourceCache(8);
  const adapter = new PageDemandAdapter({
    cache,
    wirePrefix: "test",
    fetchResource: async () => {
      attempts += 1;
      const error = new Error("cancelled");
      error.status = 409;
      error.code = "miv_media_error";
      throw error;
    },
    postDemand: async () => ({}),
  });
  const request = adapterPageRequest("cancelled");
  adapter.openDisplay({
    requestId: "cancelled-display",
    groupKey: "cancelled-group",
    requests: [request],
  });

  await assert.rejects(adapter.waitForDisplay("cancelled-display"));
  assert.equal(attempts, 1);
  adapter.releaseDisplay("cancelled-display");
});

test("page count retention stays bounded and protects coordinator-owned keys", () => {
  const protectedKeys = new Set(["displayed"]);
  const cache = new PageResourceCache(
    2,
    () => [...protectedKeys]
  );
  const resource = (name) => ({
    blob: new Blob([new Uint8Array(3)]),
    requestId: name,
    fetchMs: 1,
  });
  cache.remember("displayed", resource("displayed"));
  cache.remember("old", resource("old"));
  cache.remember("new", resource("new"));

  assert.equal(cache.hasBytes("displayed"), true);
  assert.equal(cache.hasBytes("old"), false);
  assert.equal(cache.hasBytes("new"), true);
  assert.ok(cache.ready.size <= 2);
  assert.equal(cache.readyBytes, 6);
  cache.clear();
});

test("every cache deletion reports its reason and retained byte accounting", () => {
  const events = [];
  const cache = new PageResourceCache(3, () => [], (event) => events.push(event));
  const resource = (bytes) => ({
    blob: new Blob([new Uint8Array(bytes)]),
    requestId: `bytes-${bytes}`,
    fetchMs: 1,
  });
  cache.remember("first", resource(7));
  cache.remember("second", resource(11));
  assert.equal(cache.prefetchAdmits("third"), true);
  assert.equal(cache.readyBytes, 18, "bytes do not close count admission");

  cache.clear();
  assert.deepEqual(
    events.filter(({ type }) => type === "evict"),
    [
      {
        type: "evict",
        key: "first",
        reason: "clear",
        retainedCount: 1,
        retainedBytes: 11,
      },
      {
        type: "evict",
        key: "second",
        reason: "clear",
        retainedCount: 0,
        retainedBytes: 0,
      },
    ]
  );
});

test("deep page prefetch is limited by page count and not retained bytes", async () => {
  const started = [];
  const requests = Array.from({ length: 18 }, (_, index) =>
    adapterPageRequest(`page-${index}`)
  );
  const cache = new PageResourceCache(18);
  const adapter = new PageDemandAdapter({
    cache,
    prefetchConcurrency: 1,
    wirePrefix: "budget",
    fetchResource: async (request, _signal, prefetch) => {
      started.push({ cacheKey: request.cacheKey, prefetch });
      return {
        blob: new Blob([new Uint8Array(3)]),
        requestId: `request-${request.cacheKey}`,
        fetchMs: 1,
        pageRenderMs: 0.5,
        info: null,
      };
    },
    postDemand: async () => ({}),
  });

  adapter.setPlan(requests.slice(0, 16));
  await waitForAdapterIdle(adapter);
  assert.deepEqual(
    started.map(({ cacheKey }) => cacheKey),
    requests.slice(0, 16).map(({ cacheKey }) => cacheKey)
  );
  assert.equal(cache.readyBytes, 48);

  adapter.openDisplay({
    requestId: "display-page-1",
    groupKey: "display-page-1",
    requests: [requests[1]],
  });
  await adapter.waitForDisplay("display-page-1");
  adapter.commitDisplay("display-page-1");
  adapter.setPlan(requests.slice(2, 18));
  await waitForAdapterIdle(adapter);

  assert.deepEqual(
    started.map(({ cacheKey }) => cacheKey),
    requests.map(({ cacheKey }) => cacheKey)
  );
  assert.equal(cache.hasBytes("page-0"), true);
  assert.equal(cache.hasBytes("page-1"), true, "the displayed page stays protected");
  assert.equal(adapter.resourceForKey("page-1").requestId, "request-page-1");
  assert.equal(
    started.filter(({ cacheKey }) => cacheKey === "page-1").length,
    1,
    "a protected fetched page must not be discarded and fetched again"
  );

  adapter.releaseDisplay("display-page-1");
  adapter.setPlan([]);
  cache.clear();
});

test("full count limit evicts a farther planned page before fetching a nearer one", async () => {
  const started = [];
  const resource = (key) => ({
    blob: new Blob([new Uint8Array(3)]),
    requestId: `request-${key}`,
    fetchMs: 1,
    info: null,
  });
  const cache = new PageResourceCache(2);
  cache.ready.set("nearer", resource("nearer"));
  cache.ready.set("farther", resource("farther"));
  cache.readyBytes = 6;
  const adapter = new PageDemandAdapter({
    cache,
    prefetchConcurrency: 1,
    wirePrefix: "near",
    fetchResource: async (request) => {
      started.push(request.cacheKey);
      return resource(request.cacheKey);
    },
    postDemand: async () => ({}),
  });

  adapter.setPlan([
    adapterPageRequest("nearer"),
    adapterPageRequest("target"),
    adapterPageRequest("farther"),
  ]);
  await waitForAdapterIdle(adapter);

  assert.deepEqual(started, ["target"]);
  assert.equal(cache.hasBytes("nearer"), true);
  assert.equal(cache.hasBytes("target"), true);
  assert.equal(cache.hasBytes("farther"), false);
  adapter.setPlan([]);
  cache.clear();
});

test("full count limit never evicts a nearer page to start a farther request", () => {
  const resource = (key) => ({
    blob: new Blob([new Uint8Array(3)]),
    requestId: `request-${key}`,
    fetchMs: 1,
    info: null,
  });
  const started = [];
  const cache = new PageResourceCache(2);
  cache.ready.set("near-1", resource("near-1"));
  cache.ready.set("near-2", resource("near-2"));
  cache.readyBytes = 6;
  const adapter = new PageDemandAdapter({
    cache,
    prefetchConcurrency: 1,
    wirePrefix: "far",
    fetchResource: async (request) => {
      started.push(request.cacheKey);
      return resource(request.cacheKey);
    },
    postDemand: async () => ({}),
  });

  adapter.setPlan([
    adapterPageRequest("near-1"),
    adapterPageRequest("near-2"),
    adapterPageRequest("far-target"),
  ]);

  assert.deepEqual(started, []);
  assert.equal(adapter.jobs.size, 0);
  assert.equal(cache.hasBytes("near-1"), true);
  assert.equal(cache.hasBytes("near-2"), true);
  adapter.setPlan([]);
  cache.clear();
});

test("retained pages stay bounded even when the prefetch plan is empty", async () => {
  const cache = new PageResourceCache(4);
  const adapter = new PageDemandAdapter({
    cache,
    wirePrefix: "single",
    fetchResource: async (request) => ({
      blob: new Blob([new Uint8Array(4)]),
      requestId: `request-${request.cacheKey}`,
      fetchMs: 1,
      info: null,
    }),
    postDemand: async () => ({}),
  });
  let finalRequestId = null;
  for (let index = 0; index < 6; index += 1) {
    const requestId = `single-display-${index}`;
    finalRequestId = requestId;
    adapter.openDisplay({
      requestId,
      groupKey: requestId,
      requests: [adapterPageRequest(`single-${index}`)],
    });
    await adapter.waitForDisplay(requestId);
    adapter.commitDisplay(requestId);
    adapter.setPlan([]);
  }

  assert.ok(cache.ready.size <= 4, `retained ${cache.ready.size} entries`);
  assert.equal(cache.readyBytes, 16);
  assert.equal(cache.hasBytes("single-5"), true);
  adapter.releaseDisplay(finalRequestId);
  cache.clear();
});

test("a foreground admission retry stops when its own load is aborted", async () => {
  const firstAttempt = deferred();
  let attempts = 0;
  const cache = new PageResourceCache(4);
  const adapter = new PageDemandAdapter({
    cache,
    wirePrefix: "abort-retry",
    fetchResource: async () => {
      attempts += 1;
      firstAttempt.resolve();
      const error = new Error("busy");
      error.status = 503;
      error.code = "ipc_busy";
      error.retryAfterMs = 10_000;
      throw error;
    },
    postDemand: async () => ({}),
  });
  adapter.openDisplay({
    requestId: "aborted-display",
    groupKey: "aborted-group",
    requests: [adapterPageRequest("aborted")],
  });
  const waiting = adapter.waitForDisplay("aborted-display");
  await firstAttempt.promise;
  adapter.releaseDisplay("aborted-display");

  await assert.rejects(waiting, (error) => error.name === "AbortError");
  await waitForAdapterIdle(adapter);
  assert.equal(attempts, 1);
  cache.clear();
});

test("page demand adapter reports HUD states and refreshes on start settle cancel and evict", async () => {
  const readyGate = deferred();
  let hudRefreshes = 0;
  const cacheEvents = [];
  const cache = new PageResourceCache(
    4,
    () => [],
    (event) => {
      cacheEvents.push(event);
      hudRefreshes += 1;
    }
  );
  const adapter = new PageDemandAdapter({
    cache,
    wirePrefix: "hud",
    fetchResource: (request, signal) => {
      if (request.cacheKey === "ready-page") return readyGate.promise;
      return new Promise((_resolve, reject) => {
        signal.addEventListener("abort", () => {
          const error = new Error("Aborted");
          error.name = "AbortError";
          reject(error);
        }, { once: true });
      });
    },
    postDemand: async () => ({}),
    onStatusChange: () => { hudRefreshes += 1; },
  });

  adapter.setPlan([adapterPageRequest("ready-page")]);
  assert.deepEqual(adapter.statusForKeys(["ready-page", "absent-page"]), [
    "active",
    "missing",
  ]);
  const afterStart = hudRefreshes;
  assert.ok(afterStart > 0);

  readyGate.resolve({
    blob: new Blob(["ready"]),
    requestId: "ready",
    fetchMs: 1,
    info: null,
  });
  await waitForAdapterIdle(adapter);
  assert.deepEqual(adapter.statusForKeys(["ready-page"]), ["ready"]);
  assert.ok(hudRefreshes > afterStart, "settle refreshes the HUD");
  assert.ok(cacheEvents.some((event) => event.type === "ready"));

  adapter.setPlan([adapterPageRequest("discarded-page")]);
  assert.deepEqual(adapter.statusForKeys(["discarded-page"]), ["active"]);
  const beforeCancel = hudRefreshes;
  adapter.setPlan([]);
  assert.deepEqual(adapter.statusForKeys(["discarded-page"]), ["missing"]);
  assert.ok(hudRefreshes > beforeCancel, "cancel refreshes the HUD");
  await waitForAdapterIdle(adapter);

  const beforeEvict = hudRefreshes;
  assert.equal(cache.deleteReady("ready-page", "window_out"), true);
  assert.deepEqual(adapter.statusForKeys(["ready-page"]), ["missing"]);
  assert.ok(hudRefreshes > beforeEvict, "eviction refreshes the HUD");
  assert.ok(cacheEvents.some((event) =>
    event.type === "evict" &&
    event.key === "ready-page" &&
    event.reason === "window_out" &&
    event.retainedCount === 0 &&
    event.retainedBytes === 0
  ));
  cache.clear();
});
test("remote adjustment normalization keeps valid local slider defaults and bounds", () => {
  assert.deepEqual(normalizeRemoteAdjustmentValues(), {
    brightness: 0,
    contrast: 0,
    gamma: 1,
    saturation: 0,
    temperature: 0,
    black_point: 0,
    white_point: 255,
    midtone: 1,
    auto_mode: null,
    colorize: {
      mode: "disabled",
      mono_tolerance: 12,
      palette: "legacy4_color",
      control_points: [
        { color: [0, 0, 0], strength: 3 },
        { color: [75, 0, 130], strength: 1 },
        { color: [205, 92, 92], strength: 1 },
        { color: [245, 222, 179], strength: 1 },
        { color: [240, 248, 255], strength: 1 },
      ],
      luminance_weight: 100,
      density_normalization_strength: 0,
      tone_method: "off",
      tone_radius: 1,
      tone_strength: 100,
    },
    ai: null,
  });
  assert.equal(normalizeRemoteAdjustmentValues({ black_point: 255 }).black_point, 254);
  assert.equal(normalizeRemoteAdjustmentValues({ white_point: 0 }).white_point, 1);
  assert.deepEqual(
    normalizeRemoteAdjustmentValues({
      ai: { upscale_model: "auto", denoise_model: "denoise_realplksr" },
    }).ai,
    { upscale_model: "auto", denoise_model: "denoise_realplksr" }
  );
});

test("AI progress shows only server counters and never invents a percentage", () => {
  const text = remoteAiProgressText({
    page_count: 2,
    progress: {
      phase: "upscaling",
      page_index: 1,
      page_count: 2,
      stage_index: 1,
      stage_count: 2,
      completed_tiles: 3,
      total_tiles: 8,
    },
  });
  assert.equal(text, "拡大しています · ページ 2 / 2 · 処理 2 / 2 · 進み具合 3 / 8");
  assert.equal(text.includes("%"), false);
  assert.equal(
    remoteAiProgressText({ progress: { phase: "loading_model", page_count: 1 } }),
    "準備しています"
  );
});

test("archive progress uses only monotonic core counters and no invented percentage", () => {
  const text = remoteArchiveProgressText({
    state: "converting",
    progress: {
      files_done: 7,
      files_total: 20,
      bytes_written: 3 * 1024 * 1024,
    },
  });
  assert.equal(text, "アーカイブを変換しています · 7 / 20 ファイル · 3.0 MiB 書き込み済み");
  assert.equal(text.includes("%"), false);
  assert.equal(
    remoteArchiveProgressText({ state: "waiting_for_local_drain" }),
    "PC 側の処理を待っています"
  );
});

test("archive recovery requires the exact idempotency request id", () => {
  const jobs = [
    { request_id: "miv-archive:a:old", created_unix_ms: 10 },
    { request_id: "miv-archive:a:current", created_unix_ms: 20 },
    { request_id: "miv-archive:a:current", created_unix_ms: 30 },
  ];
  assert.equal(
    selectRecoverableRemoteArchiveJob(jobs, "miv-archive:a:current")?.created_unix_ms,
    30
  );
  assert.equal(selectRecoverableRemoteArchiveJob(jobs, "missing"), null);
});

test("AI recovery prefers the exact request and otherwise only resumes a running group", () => {
  const jobs = [
    { request_id: "miv-ai:group:old", state: "ready", created_unix_ms: 30 },
    { request_id: "miv-ai:group:cancelling", state: "cancelling", created_unix_ms: 50 },
    { request_id: "miv-ai:group:running", state: "upscaling", created_unix_ms: 20 },
    { request_id: "miv-ai:other:running", state: "denoising", created_unix_ms: 40 },
  ];
  assert.equal(
    selectRecoverableRemoteAiJob(jobs, "group")?.request_id,
    "miv-ai:group:running"
  );
  assert.equal(
    selectRecoverableRemoteAiJob(jobs, "group", "miv-ai:group:old")?.state,
    "ready"
  );
});

test("AI polling stops in background and uses the agreed foreground backoff", () => {
  assert.equal(remoteAiPollingDelay({ visibilityState: "visible", terminal: false }), 500);
  assert.equal(
    remoteAiPollingDelay({ visibilityState: "hidden", terminal: false, failureCount: 0 }),
    null
  );
  assert.equal(remoteAiPollingDelay({ visibilityState: "visible", terminal: true }), null);
  assert.deepEqual(
    [0, 1, 2, 3].map((failureCount) => remoteAiPollingDelay({
      visibilityState: "visible",
      terminal: false,
      failureCount,
    })),
    [1000, 2000, 5000, 5000]
  );
});

test("automatic AI completion stays silent only when every page is not applicable", () => {
  assert.equal(remoteAiCompletionMessage({
    readyCount: 0,
    notApplicableCount: 1,
  }), null);
  assert.equal(remoteAiCompletionMessage({
    readyCount: 1,
    notApplicableCount: 0,
  }), "AI 処理が完了しました。");
  assert.equal(remoteAiCompletionMessage({
    readyCount: 1,
    notApplicableCount: 1,
  }), "AI 処理が完了しました。一部のページは元の表示です。");
});

test("remote colorize normalization preserves custom points and clamps desktop ranges", () => {
  const colorize = normalizeRemoteColorizeParams({
    mode: "monochrome_only",
    mono_tolerance: 99,
    palette: "custom",
    control_points: [
      { color: [-1, 20, 999], strength: 12 },
      { color: [200, 180, 160], strength: 0.5 },
    ],
    luminance_weight: -5,
    density_normalization_strength: 120,
    tone_method: "gaussian",
    tone_radius: 8,
    tone_strength: 42,
  });
  assert.deepEqual(colorize, {
    mode: "monochrome_only",
    mono_tolerance: 64,
    palette: "custom",
    control_points: [
      { color: [0, 20, 255], strength: 10 },
      { color: [200, 180, 160], strength: 0.5 },
    ],
    luminance_weight: 0,
    density_normalization_strength: 100,
    tone_method: "gaussian",
    tone_radius: 4,
    tone_strength: 42,
  });
});

test("plain image media routes resolve their containing folder", () => {
  assert.deepEqual(
    parentContainerAddress({
      path: testPath("books/volume/page-002.jpg"),
      subresource: { kind: "file" },
    }),
    {
      path: testPath("books/volume"),
      subresource: { kind: "file" },
    }
  );
});

test("an absolute image path opens by its file address", () => {
  const dispatched = [];
  const tile = createGridTile(
    { kind: "image", name: "outside.jpg", path: testPath("outside.jpg") },
    0,
    { get: () => 0 },
    null,
    180,
    (requested, meta) => dispatched.push({ requested, meta })
  );

  tile.dispatchEvent({ type: "click", detail: 1, pointerType: "touch" });

  assert.equal(dispatched.length, 1);
  assert.deepEqual(dispatched[0].requested.payload, {
    kind: "image",
    path: testPath("outside.jpg"),
    imageIndex: 0,
    entryIndex: 0,
  });
});

test("tapping a video grid tile preserves the video route", () => {
  const address = {
    path: testPath("clips/movie.mp4"),
    subresource: { kind: "file" },
  };
  const dispatched = [];
  const tile = createGridTile(
    { kind: "video", name: "movie.mp4", address },
    4,
    new Map(),
    null,
    180,
    (requested, meta) => dispatched.push({ requested, meta })
  );

  tile.dispatchEvent({ type: "click", detail: 1, pointerType: "touch" });

  assert.equal(dispatched.length, 1);
  assert.equal(dispatched[0].requested.name, "open");
  assert.deepEqual(dispatched[0].requested.payload, {
    kind: "media",
    mediaKind: "video",
    address,
    entryIndex: 4,
  });
  assert.equal(dispatched[0].meta.source, "touch");
  assert.equal(dispatched[0].meta.detail, "grid_tile");
  assert.equal(
    resolveMediaOpenRoute(
      dispatched[0].requested.payload.mediaKind,
      { kind: "video", address },
      -1
    ),
    "video"
  );
  assert.equal(resolveMediaOpenRoute("video", { kind: "image", address }, 0), null);
});

test("tapping an audio grid tile opens the shared media viewer route", () => {
  const address = {
    path: testPath("music/track.flac"),
    subresource: { kind: "file" },
  };
  const dispatched = [];
  const tile = createGridTile(
    { kind: "audio", name: "track.flac", address },
    2,
    new Map(),
    null,
    180,
    (requested, meta) => dispatched.push({ requested, meta })
  );

  const preview = tile.children[0];
  assert.equal(preview.children.some((child) => child.tagName === "SVG"), true);
  assert.equal(preview.children.some((child) => child.tagName === "IMG"), false);
  assert.equal(tile._thumbnailBinding, undefined);

  tile.dispatchEvent({ type: "click", detail: 1, pointerType: "touch" });

  assert.equal(dispatched.length, 1);
  assert.deepEqual(dispatched[0].requested.payload, {
    kind: "media",
    mediaKind: "audio",
    address,
    entryIndex: 2,
  });
  assert.equal(unsupportedRemoteEntryMessage("audio"), "");
  const notice = new FakeElement("p");
  notice.hidden = true;
  assert.equal(showUnsupportedRemoteEntryNotice(notice, "audio"), false);
  assert.equal(notice.hidden, true);
  assert.equal(resolveMediaOpenRoute("audio", { kind: "audio", address }, -1), "audio");
});

test("session acquisition refreshes favorites and home once without taking the viewer", async () => {
  const previousHome = {
    places: [{ kind: "folder", label: "以前の場所" }],
    smart_folders: [{ id: "old", name: "以前のスマートフォルダ" }],
  };
  const appState = {
    favorites: [{ name: "以前のお気に入り" }],
    home: previousHome,
    homeLoadError: "",
    screenContext: "viewer",
    homeTab: "places",
  };
  const requests = [];
  const pending = new Map();
  const renderedTabs = [];
  const observedGenerations = [];
  const coordinator = createRemoteHomeDataRefreshCoordinator({
    requestJson(endpoint) {
      requests.push(endpoint);
      const request = deferred();
      pending.set(endpoint, request);
      return request.promise;
    },
    appState,
    applyGeneration(generation, options) {
      observedGenerations.push({ generation, options });
    },
    renderHomeScreen(tab) {
      renderedTabs.push(tab);
    },
  });

  const firstRefresh = coordinator.refreshAfterSessionAcquire();
  const consecutiveRefresh = coordinator.refreshAfterSessionAcquire();
  assert.deepEqual(requests, ["/api/favorites", "/api/home"]);

  const nextFavorites = [{ name: "更新後のお気に入り" }];
  const nextHome = {
    places: [{ kind: "folder", label: "更新後の場所" }],
    smart_folders: [{ id: "new", name: "更新後のスマートフォルダ" }],
  };
  pending.get("/api/favorites").resolve({
    remote_state_generation: "generation-2",
    favorites: nextFavorites,
  });
  pending.get("/api/home").resolve(nextHome);
  await Promise.all([firstRefresh, consecutiveRefresh]);

  assert.strictEqual(appState.favorites, nextFavorites);
  assert.strictEqual(appState.home, nextHome);
  assert.equal(appState.screenContext, "viewer");
  assert.deepEqual(renderedTabs, []);
  assert.deepEqual(observedGenerations, [
    { generation: "generation-2", options: { reloadViewer: false } },
  ]);
});

test("initial loading consumes an already completed acquisition refresh without duplicate requests", async () => {
  const appState = {
    favorites: [],
    home: { places: [], smart_folders: [] },
    homeLoadError: "",
    screenContext: "loading",
    homeTab: "places",
  };
  const requests = [];
  const pending = new Map();
  const coordinator = createRemoteHomeDataRefreshCoordinator({
    requestJson(endpoint) {
      requests.push(endpoint);
      const request = deferred();
      pending.set(endpoint, request);
      return request.promise;
    },
    appState,
    applyGeneration() {},
    renderHomeScreen() {},
  });

  const acquisitionRefresh = coordinator.refreshAfterSessionAcquire();
  assert.deepEqual(requests, ["/api/favorites", "/api/home"]);

  pending.get("/api/favorites").resolve({
    remote_state_generation: "generation-1",
    favorites: [],
  });
  pending.get("/api/home").resolve({ places: [], smart_folders: [] });
  await acquisitionRefresh;
  await coordinator.loadInitial();

  assert.deepEqual(requests, ["/api/favorites", "/api/home"]);
});

test("successful session home refresh redraws only the visible home data tab", async () => {
  const appState = {
    favorites: [],
    home: { places: [], smart_folders: [] },
    homeLoadError: "previous startup failure",
    screenContext: "home",
    homeTab: "smart",
  };
  const renderedTabs = [];
  const nextHome = {
    places: [{ kind: "folder", label: "更新後の場所" }],
    smart_folders: [{ id: "new", name: "更新後のスマートフォルダ" }],
  };
  const coordinator = createRemoteHomeDataRefreshCoordinator({
    requestJson(endpoint) {
      if (endpoint === "/api/home") return Promise.resolve(nextHome);
      return Promise.resolve({ remote_state_generation: "generation-4", favorites: [] });
    },
    appState,
    applyGeneration() {},
    renderHomeScreen(tab) {
      renderedTabs.push(tab);
    },
  });

  await coordinator.refreshAfterSessionAcquire();

  assert.strictEqual(appState.home, nextHome);
  assert.equal(appState.homeLoadError, "");
  assert.deepEqual(renderedTabs, ["smart"]);
});

test("failed session home refresh preserves the last good places and smart folders", async () => {
  const previousHome = {
    places: [{ kind: "folder", label: "残る場所" }],
    smart_folders: [{ id: "kept", name: "残るスマートフォルダ" }],
  };
  const appState = {
    favorites: [{ name: "以前のお気に入り" }],
    home: previousHome,
    homeLoadError: "",
    screenContext: "home",
    homeTab: "places",
  };
  const renderedTabs = [];
  const coordinator = createRemoteHomeDataRefreshCoordinator({
    requestJson(endpoint) {
      if (endpoint === "/api/home") return Promise.reject(new Error("temporary failure"));
      return Promise.resolve({
        remote_state_generation: "generation-3",
        favorites: [{ name: "更新後のお気に入り" }],
      });
    },
    appState,
    applyGeneration() {},
    renderHomeScreen(tab) {
      renderedTabs.push(tab);
    },
  });

  const results = await coordinator.refreshAfterSessionAcquire();

  assert.deepEqual(results.map((result) => result.status), ["fulfilled", "rejected"]);
  assert.strictEqual(appState.home, previousHome);
  assert.deepEqual(appState.home.places, previousHome.places);
  assert.deepEqual(appState.home.smart_folders, previousHome.smart_folders);
  assert.equal(appState.homeLoadError, "");
  assert.deepEqual(renderedTabs, []);
  assert.deepEqual(appState.favorites, [{ name: "更新後のお気に入り" }]);
});

test("initial home failure keeps its empty-state behavior separate from session refresh", async () => {
  const appState = {
    favorites: [],
    home: {
      places: [{ kind: "folder", label: "起動前の仮データ" }],
      smart_folders: [{ id: "temporary", name: "起動前の仮データ" }],
    },
    homeLoadError: "",
    screenContext: "loading",
    homeTab: "places",
  };
  const coordinator = createRemoteHomeDataRefreshCoordinator({
    requestJson(endpoint) {
      if (endpoint === "/api/home") return Promise.reject(new Error("initial failure"));
      return Promise.resolve({ remote_state_generation: "generation-1", favorites: [] });
    },
    appState,
    applyGeneration() {},
    renderHomeScreen() {},
  });

  await coordinator.loadInitial();

  assert.deepEqual(appState.home, { places: [], smart_folders: [] });
  assert.equal(appState.homeLoadError, "initial failure");
});

test("losing a session does not refresh home data until a new session is acquired", () => {
  const refreshes = [];
  applyRemoteSessionId(TEST_SESSION_ID, () => refreshes.push("acquired"));
  applyRemoteSessionId("", () => refreshes.push("lost"));

  assert.deepEqual(refreshes, ["acquired"]);
});

test("resolved audio and video opens enter the media viewer instead of the image viewer", () => {
  for (const kind of ["audio", "video"]) {
    const entry = {
      kind,
      name: kind === "audio" ? "track.flac" : "movie.mp4",
      address: {
        path: testPath(kind === "audio" ? "music/track.flac" : "clips/movie.mp4"),
        subresource: { kind: "file" },
      },
    };
    const calls = [];
    const mediaRoute = resolveMediaOpenRoute(kind, entry, -1);
    const renderedViewer = renderResolvedMediaOpen(
      mediaRoute,
      entry,
      -1,
      123,
      (addressedEntry) => {
        calls.push({ viewer: "media", entry: addressedEntry });
        return true;
      },
      (imageIndex, startedAt) => calls.push({ viewer: "image", imageIndex, startedAt })
    );

    assert.equal(isStreamMediaKind(kind), true);
    assert.equal(renderedViewer, "media");
    assert.deepEqual(calls, [{ viewer: "media", entry }]);
  }
  assert.equal(isStreamMediaKind("image"), false);
  assert.equal(isStreamMediaKind("archive"), false);
});

test("tapping an archive grid tile starts the archive job route without a thumbnail request", () => {
  const address = {
    path: testPath("books/book.rar"),
    subresource: { kind: "file" },
  };
  const dispatched = [];
  const tile = createGridTile(
    { kind: "archive", name: "book.rar", address },
    3,
    new Map(),
    null,
    180,
    (requested, meta) => dispatched.push({ requested, meta })
  );

  tile.dispatchEvent({ type: "click", detail: 1, pointerType: "touch" });

  assert.equal(dispatched.length, 1);
  assert.deepEqual(dispatched[0].requested.payload, {
    kind: "archive",
    address,
    name: "book.rar",
    entryIndex: 3,
  });
  assert.equal(tile._thumbnailBinding, undefined);
  assert.equal(unsupportedRemoteEntryMessage("archive"), "");
  const notice = new FakeElement("p");
  notice.hidden = true;
  assert.equal(showUnsupportedRemoteEntryNotice(notice, "archive"), false);
  assert.equal(notice.hidden, true);
});

test("image viewer applies seek direction to the native range control", () => {
  const seekInput = new FakeElement("input");
  const viewer = new ImageViewer({
    root: new FakeElement("section"),
    stage: new FakeElement("div"),
    image: new FakeElement("img"),
    title: new FakeElement("div"),
    counter: new FakeElement("output"),
    seek: new FakeElement("div"),
    seekInput,
    previous: new FakeElement("button"),
    next: new FakeElement("button"),
    loadingIndicator: new FakeElement("div"),
  });

  viewer.setSeekState({
    visible: true,
    min: 0,
    max: 2,
    value: 0,
    direction: "rtl",
    label: "1 / 3",
  });
  assert.equal(seekInput.dir, "rtl");
  assert.equal(seekInput.value, "0");

  viewer.setSeekState({
    visible: true,
    min: 0,
    max: 2,
    value: 2,
    direction: "ltr",
    label: "3 / 3",
  });
  assert.equal(seekInput.dir, "ltr");
  assert.equal(seekInput.value, "2");
});

test("viewer generation invalidation cancels fetch and rejects a late decode replacement", () => {
  const loadingIndicator = new FakeElement("div");
  loadingIndicator.hidden = false;
  const viewer = new ImageViewer({
    root: new FakeElement("section"),
    stage: new FakeElement("div"),
    image: new FakeElement("img"),
    title: new FakeElement("div"),
    counter: new FakeElement("output"),
    loadingIndicator,
  });
  let aborted = false;
  viewer.legacyFetchController = { abort() { aborted = true; } };
  viewer.loadSequence = 7;
  viewer.loadingTimer = setTimeout(() => {}, 1000);

  viewer.invalidatePendingLoad();

  assert.equal(viewer.loadSequence, 8);
  assert.equal(aborted, true);
  assert.equal(viewer.legacyFetchController, null);
  assert.equal(viewer.loadingTimer, 0);
  assert.equal(loadingIndicator.hidden, true);
});

test("decoded unit cancellation releases an out-of-window decode and never starts without bytes", async () => {
  const releaseDecodes = [];
  let createCalls = 0;
  const revoked = [];
  const cache = new DecodedPageUnitCache({
    createImage: () => {
      createCalls += 1;
      const image = new FakeElement("img");
      image.decode = () => new Promise((resolve) => { releaseDecodes.push(resolve); });
      return image;
    },
    createObjectUrl: (_blob) => `blob:test-${createCalls}`,
    revokeObjectUrl: (url) => revoked.push(url),
  });
  cache.setWindow(["pending"]);
  const pending = cache.prepare({
    unitKey: "pending",
    pages: [
      {
        requestKey: "pending-left",
        resource: { blob: new Blob(["left"]) },
        alt: "left",
      },
      {
        requestKey: "pending-right",
        resource: { blob: new Blob(["right"]) },
        alt: "right",
      },
    ],
  });
  await Promise.resolve();
  assert.equal(createCalls, 2);

  cache.setWindow(["other"]);
  assert.deepEqual(revoked, ["blob:test-0", "blob:test-1"]);
  releaseDecodes.forEach((release) => release());
  assert.deepEqual(await pending, { started: true, reason: "window_out" });
  assert.equal(cache.retainedUnitCount(), 0);

  const skipped = await cache.prepare({
    unitKey: "other",
    pages: [{ requestKey: "missing-page", resource: null }],
  });
  assert.deepEqual(skipped, { started: false, reason: "bytes_missing" });
  assert.equal(createCalls, 2, "missing bytes must not create or decode an image");
  assert.equal(cache.tryAcquire("other", ["missing-page"]).reason, "bytes_missing");
  cache.clear();
});

test("viewer reuses retained image elements and reports reuse and miss timings", async () => {
  let predecodeCalls = 0;
  let displayDecodeCalls = 0;
  const reports = [];
  const cache = new DecodedPageUnitCache({
    createImage: () => {
      const image = new FakeElement("img");
      image.decode = async () => { predecodeCalls += 1; };
      return image;
    },
    createObjectUrl: (blob) => URL.createObjectURL(blob),
    revokeObjectUrl: (url) => URL.revokeObjectURL(url),
  });
  const retainedResource = {
    blob: new Blob([new Uint8Array([1, 2, 3])]),
    requestId: "decode-ahead-ready",
    fetchMs: 5,
    pageRenderMs: 3,
    prefetchStatus: "prefetched",
  };
  cache.setWindow(["ready-unit"]);
  await cache.prepare({
    unitKey: "ready-unit",
    pages: [{
      requestKey: "ready-page",
      resource: retainedResource,
      alt: "ready",
      info: { width: 1200, height: 1800 },
    }],
  });
  const retainedImage = cache.units.get("ready-unit").pages[0].image;
  const stage = new FakeElement("div");
  const viewer = new ImageViewer({
    root: new FakeElement("section"),
    stage,
    image: new FakeElement("img"),
    title: new FakeElement("div"),
    counter: new FakeElement("span"),
    loadingIndicator: new FakeElement("div"),
    decodedUnitCache: cache,
    onDecodeAheadDisplay: (event) => reports.push(event),
  });
  const request = (cacheKey, url) => ({
    url,
    cacheKey,
    remoteStateGeneration: "test-1",
    remoteSessionId: TEST_SESSION_ID,
    address: TEST_PAGE_ADDRESS,
    width: 1800,
    cssWidth: 430,
    dpr: 2,
    layout: { cssWidth: 430, cssHeight: 645 },
    fitMode: "page",
  });
  FakeElement.decodeHook = () => { displayDecodeCalls += 1; };
  try {
    const reused = await viewer.load({
      name: "Ready page",
      request: request("ready-page", "/api/page?decode-ahead=ready"),
      info: { width: 1200, height: 1800 },
      fitMode: "page",
      index: 0,
      count: 2,
      interactionStartedAt: performance.now(),
      decodedUnitKey: "ready-unit",
    });
    assert.deepEqual(reused, { outcome: ViewerGroupLoadOutcome.APPLIED });
    assert.equal(viewer.image, retainedImage);
    assert.equal(predecodeCalls, 1);
    assert.equal(displayDecodeCalls, 0, "a retained image must not be decoded again");
    assert.equal(reports[0].reused, true);
    assert.equal(reports[0].reason, "retained");
    assert.equal(reports[0].retained_unit_count, 1);
    assert.equal(typeof reports[0].tap_to_display_ms, "number");

    cache.setWindow(["missing-unit"]);
    await cache.prepare({
      unitKey: "missing-unit",
      pages: [{ requestKey: "missing-page", resource: null }],
    });
    const missed = await viewer.load({
      name: "Missing page",
      request: request("missing-page", "/api/page?decode-ahead=missing"),
      info: { width: 1200, height: 1800 },
      fitMode: "page",
      index: 1,
      count: 2,
      interactionStartedAt: performance.now(),
      decodedUnitKey: "missing-unit",
    });
    assert.deepEqual(missed, { outcome: ViewerGroupLoadOutcome.APPLIED });
    assert.equal(displayDecodeCalls, 1);
    assert.equal(reports[1].reused, false);
    assert.equal(reports[1].reason, "bytes_missing");
    assert.equal(typeof reports[1].tap_to_display_ms, "number");
    assert.equal(reports[1].retained_unit_count, 1);
  } finally {
    FakeElement.decodeHook = null;
    viewer.destroy();
    cache.clear();
  }
});

test("viewer load executes fetch, decode, layout and atomic replacement", async () => {
  const stage = new FakeElement("div");
  const initialImage = new FakeElement("img");
  const loadingIndicator = new FakeElement("div");
  loadingIndicator.hidden = true;
  const viewer = new ImageViewer({
    root: new FakeElement("section"),
    stage,
    image: initialImage,
    title: new FakeElement("div"),
    counter: new FakeElement("span"),
    previous: new FakeElement("button"),
    next: new FakeElement("button"),
    loadingIndicator,
  });
  const displayed = await viewer.load({
    name: "Page 1",
    request: {
      url: "/api/page?test=1",
      cacheKey: "page-1@1800",
      remoteStateGeneration: "test-1",
      remoteSessionId: TEST_SESSION_ID,
      address: TEST_PAGE_ADDRESS,
      width: 1800,
      cssWidth: 430,
      dpr: 2,
      layout: { cssWidth: 430 },
      fitMode: "page",
      dynamicInfo: true,
      infoCacheKey: "page-1",
      containerInfoKey: "book-1",
    },
    info: { width: 1200, height: 1800 },
    fitMode: "page",
    index: 0,
    count: 2,
    interactionStartedAt: performance.now(),
  });

  assert.deepEqual(displayed, { outcome: ViewerGroupLoadOutcome.APPLIED });
  assert.equal(viewer.pageLayer.children.length, 1);
  assert.equal(viewer.pageLayer.children[0], viewer.image);
  assert.equal(viewer.image.style.width, "430px");
  assert.equal(viewer.image.style.height, "645px");
  assert.equal(viewer.pageLayer.style.width, "430px");
  assert.equal(viewer.pageLayer.style.height, "645px");
  assert.equal(viewer.image.dataset.sourceWidth, "1200");
  assert.equal(loadingIndicator.hidden, true);

  viewer.scale = 2;
  viewer.panX = 24;
  viewer.panY = -18;
  stage.clientWidth = 390;
  stage.clientHeight = 422;
  assert.equal(viewer.refitVisibleContent(), true);
  assert.equal(Math.round(parseFloat(viewer.image.style.width)), 281);
  assert.equal(parseFloat(viewer.image.style.height), 422);
  assert.equal(viewer.scale, 1);
  assert.equal(viewer.panX, 0);
  assert.equal(viewer.panY, 0);

  stage.clientHeight = 844;
  assert.equal(viewer.refitVisibleContent(), true);
  assert.equal(parseFloat(viewer.image.style.width), 390);
  assert.equal(parseFloat(viewer.image.style.height), 585);
  viewer.destroy();
});

test("rapid page loads finish the active request and start only the latest pending request", async () => {
  const title = new FakeElement("div");
  title.textContent = "Displayed page";
  const counter = new FakeElement("span");
  counter.textContent = "0 / 3";
  const viewer = new ImageViewer({
    root: new FakeElement("section"),
    stage: new FakeElement("div"),
    image: new FakeElement("img"),
    title,
    counter,
    previous: new FakeElement("button"),
    next: new FakeElement("button"),
    loadingIndicator: new FakeElement("div"),
  });
  let releaseFirst;
  const firstGate = new Promise((resolve) => { releaseFirst = resolve; });
  const requested = [];
  let activeFetches = 0;
  let maximumActiveFetches = 0;
  globalThis.fetch = async (input) => {
    const url = new URL(input, testLocation.origin);
    const page = Number(url.searchParams.get("rapid"));
    requested.push(page);
    activeFetches += 1;
    maximumActiveFetches = Math.max(maximumActiveFetches, activeFetches);
    if (page === 1) await firstGate;
    activeFetches -= 1;
    return new Response(new Blob([new Uint8Array([page])]), {
      status: 200,
      headers: {
        "Content-Type": "image/jpeg",
        "X-mIV-Request-Id": `rapid-${page}`,
        "X-mIV-Image-Width": "1200",
        "X-mIV-Image-Height": "1800",
        "X-mIV-Remote-State-Generation": "test-1",
        "X-mIV-Remote-Session": TEST_SESSION_ID,
        "X-mIV-Page-Identity": pageIdentityHeader(TEST_PAGE_ADDRESS),
      },
    });
  };
  const load = (page) => viewer.load({
    name: `Page ${page}`,
    request: {
      url: `/api/page?rapid=${page}`,
      remoteStateGeneration: "test-1",
      remoteSessionId: TEST_SESSION_ID,
      address: TEST_PAGE_ADDRESS,
      width: 1800,
      cssWidth: 430,
      dpr: 2,
      layout: { cssWidth: 430 },
      fitMode: "page",
    },
    info: { width: 1200, height: 1800 },
    fitMode: "page",
    index: page - 1,
    count: 3,
    interactionStartedAt: performance.now(),
  });

  try {
    const first = load(1);
    const second = load(2);
    const third = load(3);
    assert.deepEqual(requested, [1]);
    assert.equal(title.textContent, "Page 3");
    assert.equal(counter.textContent, "3 / 3");
    assert.equal(counter.classList.contains("is-pending"), true);
    releaseFirst();
    assert.deepEqual(await Promise.all([first, second, third]), [
      { outcome: ViewerGroupLoadOutcome.SUPERSEDED },
      { outcome: ViewerGroupLoadOutcome.SUPERSEDED },
      { outcome: ViewerGroupLoadOutcome.APPLIED },
    ]);
    assert.deepEqual(requested, [1, 3]);
    assert.equal(maximumActiveFetches, 1);
    assert.equal(title.textContent, "Page 3");
    assert.equal(counter.textContent, "3 / 3");
    assert.equal(counter.classList.contains("is-pending"), false);
  } finally {
    globalThis.fetch = imageFetch;
    viewer.destroy();
  }
});

test("single and spread loads classify success, fetch, decode, and current abort outcomes", async () => {
  const responseFor = (requestId) => new Response(
    new Blob([new Uint8Array([1, 2, 3])]),
    {
      status: 200,
      headers: {
        "Content-Type": "image/jpeg",
        "X-mIV-Request-Id": requestId,
        "X-mIV-Image-Width": "1200",
        "X-mIV-Image-Height": "1800",
        "X-mIV-Remote-State-Generation": "test-1",
        "X-mIV-Remote-Session": TEST_SESSION_ID,
        "X-mIV-Page-Identity": pageIdentityHeader(TEST_PAGE_ADDRESS),
      },
    }
  );
  const requestFor = (name, page) => ({
    url: `/api/page?outcome=${name}-${page}`,
    remoteStateGeneration: "test-1",
    remoteSessionId: TEST_SESSION_ID,
    address: TEST_PAGE_ADDRESS,
    width: 1800,
    cssWidth: 430,
    dpr: 2,
    layout: { cssWidth: 430, cssHeight: 645 },
    fitMode: "page",
  });
  const scenarios = [
    { name: "success", outcome: "applied", reason: "dom_committed" },
    { name: "fetch", outcome: "not_applied", reason: "fetch_failed" },
    { name: "decode", outcome: "not_applied", reason: "decode_failed" },
    { name: "abort", outcome: "not_applied", reason: "abort" },
  ];

  try {
    for (const kind of ["single", "spread"]) {
      for (const scenario of scenarios) {
        let fetchIndex = 0;
        globalThis.fetch = async () => {
          fetchIndex += 1;
          if (scenario.name === "fetch") {
            return new Response("failed", { status: 500 });
          }
          if (scenario.name === "abort") {
            throw new DOMException("aborted", "AbortError");
          }
          return responseFor(`${kind}-${scenario.name}-${fetchIndex}`);
        };
        FakeElement.decodeHook = scenario.name === "decode"
          ? async () => { throw new Error(`${kind} decode failed`); }
          : null;
        const viewer = new ImageViewer({
          root: new FakeElement("section"),
          stage: new FakeElement("div"),
          image: new FakeElement("img"),
          title: new FakeElement("div"),
          counter: new FakeElement("span"),
          previous: new FakeElement("button"),
          next: new FakeElement("button"),
          loadingIndicator: new FakeElement("div"),
        });
        const history = [];
        viewer.recordPageDisplay = (...args) => history.push(args);
        const pages = (kind === "single" ? [1] : [1, 2]).map((page) => ({
          entry: { name: `${kind} page ${page}` },
          info: { width: 1200, height: 1800 },
          request: requestFor(`${kind}-${scenario.name}`, page),
        }));

        const result = await viewer.loadGroup({
          pages,
          name: `${kind} ${scenario.name}`,
          fitMode: "page",
          gap: kind === "spread" ? 12 : 0,
          index: 0,
          count: 2,
          pageNumbers: pages.map((_, index) => index + 1),
          interactionStartedAt: performance.now(),
        });
        assert.equal(
          result.outcome,
          scenario.name === "success"
            ? ViewerGroupLoadOutcome.APPLIED
            : ViewerGroupLoadOutcome.FAILED,
          `${kind} ${scenario.name}`
        );
        if (scenario.name === "abort") {
          // 中断した事実だけを述べる。位置を戻したかは完了処理が決めるので、
          // ここで「前のページに戻りました」と書くと戻さない経路で嘘になる。
          assert.match(result.message, /表示が中断されました/);
          assert.doesNotMatch(result.message, /前のページに戻りました/);
        }
        assert.equal(history.length, 1, `${kind} ${scenario.name} telemetry count`);
        assert.equal(history[0][1], scenario.outcome);
        assert.equal(history[0][2], scenario.reason);
        if (scenario.name === "success") {
          assert.deepEqual(history[0][3], history[0][4]);
          assert.equal(history[0][3].length, pages.length);
        } else {
          assert.deepEqual(history[0][4] ?? [], []);
        }
        viewer.destroy();
      }
    }
  } finally {
    FakeElement.decodeHook = null;
    globalThis.fetch = imageFetch;
  }
});

test("a superseded spread cannot replace the newer spread outcome", async () => {
  let releaseOld;
  const oldGate = new Promise((resolve) => { releaseOld = resolve; });
  let oldFetches = 0;
  globalThis.fetch = async (input) => {
    const url = new URL(input, testLocation.origin);
    const generation = url.searchParams.get("spread-outcome");
    if (generation === "old") {
      oldFetches += 1;
      await oldGate;
    }
    return new Response(new Blob([generation]), {
      status: 200,
      headers: {
        "Content-Type": "image/jpeg",
        "X-mIV-Request-Id": `${generation}-${oldFetches}`,
        "X-mIV-Image-Width": "1200",
        "X-mIV-Image-Height": "1800",
        "X-mIV-Remote-State-Generation": "test-1",
        "X-mIV-Remote-Session": TEST_SESSION_ID,
        "X-mIV-Page-Identity": pageIdentityHeader(TEST_PAGE_ADDRESS),
      },
    });
  };
  const viewer = new ImageViewer({
    root: new FakeElement("section"),
    stage: new FakeElement("div"),
    image: new FakeElement("img"),
    title: new FakeElement("div"),
    counter: new FakeElement("span"),
    previous: new FakeElement("button"),
    next: new FakeElement("button"),
    loadingIndicator: new FakeElement("div"),
  });
  const load = (generation, index) => viewer.loadGroup({
    pages: [1, 2].map((page) => ({
      entry: { name: `${generation} ${page}` },
      info: { width: 1200, height: 1800 },
      request: {
        url: `/api/page?spread-outcome=${generation}&page=${page}`,
        remoteStateGeneration: "test-1",
        remoteSessionId: TEST_SESSION_ID,
        address: TEST_PAGE_ADDRESS,
        width: 1800,
        cssWidth: 430,
        dpr: 2,
        fitMode: "page",
      },
    })),
    name: generation,
    fitMode: "page",
    gap: 12,
    index,
    count: 2,
    interactionStartedAt: performance.now(),
  });
  try {
    const old = load("old", 0);
    const current = load("current", 1);
    releaseOld();
    assert.deepEqual(await Promise.all([old, current]), [
      { outcome: ViewerGroupLoadOutcome.SUPERSEDED },
      { outcome: ViewerGroupLoadOutcome.APPLIED },
    ]);
    assert.equal(viewer.displayedGroupIndex(), 1);
  } finally {
    globalThis.fetch = imageFetch;
    viewer.destroy();
  }
});

test("AI status stays compact until tapped and keeps errors readable", () => {
  const viewerRoot = new FakeElement("section");
  const controller = new RemoteAiController(
    { root: viewerRoot },
    new FakeElement("div"),
    () => () => {}
  );
  controller.show("拡大しています · 進み具合 3 / 8");
  assert.equal(controller.shortLabel.textContent, "AI 処理中");
  assert.equal(controller.details.hidden, true);
  assert.equal(controller.spinner.hidden, false);

  controller.toggleButton.dispatchEvent({
    type: "click",
    stopPropagation() {},
  });
  assert.equal(controller.details.hidden, false);
  assert.equal(controller.message.textContent, "拡大しています · 進み具合 3 / 8");

  controller.showRequestError(new Error("network"));
  assert.equal(controller.root.classList.contains("is-error"), true);
  assert.equal(controller.toggleButton.hidden, true);
  assert.equal(controller.spinner.hidden, true);
  assert.equal(controller.details.hidden, false);
  assert.equal(controller.message.textContent, "AI 処理を開始できませんでした。");
});

test("the AI status offers no cancel control, matching the desktop viewer", () => {
  const controller = new RemoteAiController(
    { root: new FakeElement("section") },
    new FakeElement("div"),
    () => () => {}
  );
  assert.equal(controller.cancel, undefined);
  assert.equal(controller.cancelButton, undefined);
  controller.show("拡大しています");
  controller.setExpanded(true);
  assert.deepEqual(
    controller.details.children.map((child) => child.tagName.toLowerCase()),
    ["span"]
  );
});

test("AI result identity mismatch is not applied or automatically fetched again", async () => {
  const requestedIdentity = {
    path: testPath("books/ai-source.pdf"),
    subresource: { kind: "pdf_page", page_number: 1 },
  };
  const responseIdentity = {
    path: testPath("books/other.pdf"),
    subresource: { kind: "pdf_page", page_number: 1 },
  };
  let replaceCalls = 0;
  const controller = new RemoteAiController(
    {
      root: new FakeElement("section"),
      async replacePageBlobs() {
        replaceCalls += 1;
        return true;
      },
    },
    new FakeElement("div"),
    () => () => {}
  );
  const snapshot = {
    job_id: "identity-job",
    state: "ready",
    page_outcomes: [{ page_index: 0, state: "ready" }],
  };
  controller.pages = [{ address: requestedIdentity, target_px: 1800, name: "AI page" }];
  controller.displayVersion = 1;
  controller.job = snapshot;
  let fetchCount = 0;
  const errors = [];
  setRuntimeTestErrorObserver((event) => errors.push(event));
  globalThis.fetch = async () => {
    fetchCount += 1;
    return new Response(new Blob([new Uint8Array([1, 2, 3])]), {
      status: 200,
      headers: {
        "Content-Type": "image/jpeg",
        "X-mIV-Page-Identity": pageIdentityHeader(responseIdentity),
      },
    });
  };
  try {
    await assert.rejects(
      controller.applyReady(snapshot, controller.generation),
      (error) => error?.code === "page_identity_mismatch"
    );
    await controller.applyReady(snapshot, controller.generation);
    assert.equal(fetchCount, 1);
    assert.equal(replaceCalls, 0);
    assert.equal(errors.length, 1);
    assert.deepEqual(errors[0].extra.requested_page_identity, requestedIdentity);
    assert.deepEqual(errors[0].extra.response_page_identity, responseIdentity);
  } finally {
    setRuntimeTestErrorObserver(null);
    globalThis.fetch = imageFetch;
    controller.destroy();
  }
});

test("viewer refuses a page response without a generation attestation", async () => {
  const initialImage = new FakeElement("img");
  const title = new FakeElement("div");
  const viewer = new ImageViewer({
    root: new FakeElement("section"),
    stage: new FakeElement("div"),
    image: initialImage,
    title,
    counter: new FakeElement("span"),
    previous: new FakeElement("button"),
    next: new FakeElement("button"),
    loadingIndicator: new FakeElement("div"),
  });
  globalThis.fetch = async () => new Response(new Blob([new Uint8Array([1, 2, 3])]), {
    status: 200,
    headers: {
      "Content-Type": "image/jpeg",
      "X-mIV-Image-Width": "1200",
      "X-mIV-Image-Height": "1800",
      "X-mIV-Remote-Session": TEST_SESSION_ID,
      "X-mIV-Page-Identity": pageIdentityHeader(TEST_PAGE_ADDRESS),
    },
  });
  try {
    const displayed = await viewer.load({
      name: "Unattested page",
      request: {
        url: "/api/page?test=unattested",
        cacheKey: "page-unattested@1800",
        remoteStateGeneration: "test-1",
        remoteSessionId: TEST_SESSION_ID,
        address: TEST_PAGE_ADDRESS,
        width: 1800,
        cssWidth: 430,
        dpr: 2,
        layout: { cssWidth: 430 },
        fitMode: "page",
        dynamicInfo: true,
        infoCacheKey: "page-unattested",
        containerInfoKey: "book-1",
      },
      info: { width: 1200, height: 1800 },
      fitMode: "page",
      index: 0,
      count: 1,
      interactionStartedAt: performance.now(),
    });
    assert.equal(displayed.outcome, ViewerGroupLoadOutcome.FAILED);
    assert.match(displayed.message, /状態版/);
    assert.equal(viewer.pageLayer.children[0], initialImage);
  } finally {
    globalThis.fetch = imageFetch;
    viewer.destroy();
  }
});

test("viewer rejects a mismatched page identity without display or retry and reports both identities", async () => {
  const requestedIdentity = {
    path: testPath("books/first.pdf"),
    subresource: { kind: "pdf_page", page_number: 1 },
  };
  const responseIdentity = {
    path: testPath("books/other.pdf"),
    subresource: { kind: "pdf_page", page_number: 1 },
  };
  const initialImage = new FakeElement("img");
  const title = new FakeElement("div");
  const viewer = new ImageViewer({
    root: new FakeElement("section"),
    stage: new FakeElement("div"),
    image: initialImage,
    title,
    counter: new FakeElement("span"),
    previous: new FakeElement("button"),
    next: new FakeElement("button"),
    loadingIndicator: new FakeElement("div"),
  });
  let fetchCount = 0;
  const errors = [];
  setRuntimeTestErrorObserver((event) => errors.push(event));
  globalThis.fetch = async () => {
    fetchCount += 1;
    return new Response(new Blob([new Uint8Array([1, 2, 3])]), {
      status: 200,
      headers: {
        "Content-Type": "image/jpeg",
        "X-mIV-Image-Width": "1200",
        "X-mIV-Image-Height": "1800",
        "X-mIV-Remote-State-Generation": "test-1",
        "X-mIV-Remote-Session": TEST_SESSION_ID,
        "X-mIV-Page-Identity": pageIdentityHeader(responseIdentity),
      },
    });
  };
  try {
    const displayed = await viewer.load({
      name: "First PDF page 2",
      request: {
        url: "/api/page?test=identity-mismatch",
        cacheKey: "page-identity-mismatch@1800",
        remoteStateGeneration: "test-1",
        remoteSessionId: TEST_SESSION_ID,
        address: requestedIdentity,
        width: 1800,
        cssWidth: 430,
        dpr: 2,
        layout: { cssWidth: 430 },
        fitMode: "page",
      },
      info: { width: 1200, height: 1800 },
      fitMode: "page",
      index: 0,
      count: 1,
      interactionStartedAt: performance.now(),
    });
    assert.equal(displayed.outcome, ViewerGroupLoadOutcome.FAILED);
    assert.match(displayed.message, /identity/);
    assert.equal(fetchCount, 1);
    assert.equal(viewer.pageLayer.children[0], initialImage);
    assert.equal(errors.length, 1);
    assert.equal(errors[0].category, "page_identity_mismatch");
    assert.deepEqual(errors[0].extra.requested_page_identity, requestedIdentity);
    assert.deepEqual(errors[0].extra.response_page_identity, responseIdentity);
  } finally {
    setRuntimeTestErrorObserver(null);
    globalThis.fetch = imageFetch;
    viewer.destroy();
  }
});

test("spread waits for both pages and atomically replaces the page layer", async () => {
  const stage = new FakeElement("div");
  stage.clientWidth = 1600;
  stage.clientHeight = 1000;
  const pageLayer = new FakeElement("div");
  const initialImage = new FakeElement("img");
  pageLayer.append(initialImage);
  const viewer = new ImageViewer({
    root: new FakeElement("section"),
    stage,
    pageLayer,
    image: initialImage,
    title: new FakeElement("div"),
    counter: new FakeElement("span"),
    previous: new FakeElement("button"),
    next: new FakeElement("button"),
    loadingIndicator: new FakeElement("div"),
  });
  const page = (number) => ({
    entry: { name: `Page ${number}` },
    info: { width: 1200, height: 1800 },
    request: {
      url: `/api/page?test=${number}`,
      cacheKey: `page-${number}@1334`,
      remoteStateGeneration: "test-1",
      remoteSessionId: TEST_SESSION_ID,
      address: TEST_PAGE_ADDRESS,
      width: 1334,
      cssWidth: 667,
      dpr: 2,
      layout: { cssWidth: 667 },
      fitMode: "page",
      dynamicInfo: true,
      infoCacheKey: `page-${number}`,
      containerInfoKey: "book-1",
    },
  });
  const displayed = await viewer.loadGroup({
    pages: [page(1), page(2)],
    name: "Page 1 / Page 2",
    fitMode: "page",
    gap: 12,
    index: 0,
    count: 3,
    interactionStartedAt: performance.now(),
  });

  assert.deepEqual(displayed, { outcome: ViewerGroupLoadOutcome.APPLIED });
  assert.equal(pageLayer.children.length, 2);
  assert.equal(viewer.images.length, 2);
  assert.equal(pageLayer.style.gap, "12px");
  assert.equal(Math.round(parseFloat(pageLayer.style.width)), 1345);
  assert.equal(Math.round(parseFloat(pageLayer.style.height)), 1000);
  assert.equal(Math.round(parseFloat(viewer.images[0].style.width)), 667);
  assert.equal(Math.round(parseFloat(viewer.images[0].style.height)), 1000);

  const originalPages = viewer.images.slice();
  let releaseDecode;
  const decodeGate = new Promise((resolve) => { releaseDecode = resolve; });
  FakeElement.decodeHook = () => decodeGate;
  const replacement = viewer.replacePageBlobs([
    { pageIndex: 0, blob: new Blob(["left"]), alt: "AI left" },
  ]);
  await Promise.resolve();
  assert.equal(pageLayer.children[0], originalPages[0]);
  assert.equal(pageLayer.children[1], originalPages[1]);
  releaseDecode();
  assert.equal(await replacement, true);
  FakeElement.decodeHook = null;
  assert.notEqual(pageLayer.children[0], originalPages[0]);
  assert.equal(pageLayer.children[1], originalPages[1]);

  const singleDisplayed = await viewer.loadGroup({
    pages: [page(3)],
    name: "Page 3",
    fitMode: "page",
    gap: 0,
    index: 1,
    count: 3,
    interactionStartedAt: performance.now(),
  });
  assert.deepEqual(singleDisplayed, { outcome: ViewerGroupLoadOutcome.APPLIED });
  assert.equal(pageLayer.children.length, 1);
  assert.equal(viewer.images.length, 1);
  assert.equal(Math.round(parseFloat(pageLayer.style.width)), 667);
  assert.equal(Math.round(parseFloat(pageLayer.style.height)), 1000);

  viewer.showBoundaryMessage("先頭ページです");
  assert.equal(viewer.boundaryMessage.hidden, false);
  assert.equal(viewer.boundaryMessage.textContent, "先頭ページです");
  viewer.hideBoundaryMessage();
  assert.equal(viewer.boundaryMessage.hidden, true);
  viewer.destroy();
});

test("spread rejects one mismatched page before replacing either side", async () => {
  const requested = (pageNumber) => ({
    path: testPath("books/spread.pdf"),
    subresource: { kind: "pdf_page", page_number: pageNumber },
  });
  const initialImage = new FakeElement("img");
  const pageLayer = new FakeElement("div");
  pageLayer.append(initialImage);
  const viewer = new ImageViewer({
    root: new FakeElement("section"),
    stage: new FakeElement("div"),
    pageLayer,
    image: initialImage,
    title: new FakeElement("div"),
    counter: new FakeElement("span"),
    previous: new FakeElement("button"),
    next: new FakeElement("button"),
    loadingIndicator: new FakeElement("div"),
  });
  let fetchCount = 0;
  const errors = [];
  setRuntimeTestErrorObserver((event) => errors.push(event));
  globalThis.fetch = async (input) => {
    fetchCount += 1;
    const pageNumber = Number(new URL(input, testLocation.origin).searchParams.get("spread-mismatch"));
    const identity = pageNumber === 1
      ? requested(0)
      : {
        path: testPath("books/other.pdf"),
        subresource: { kind: "pdf_page", page_number: 1 },
      };
    return new Response(new Blob([new Uint8Array([pageNumber])]), {
      status: 200,
      headers: {
        "Content-Type": "image/jpeg",
        "X-mIV-Image-Width": "1200",
        "X-mIV-Image-Height": "1800",
        "X-mIV-Remote-State-Generation": "test-1",
        "X-mIV-Remote-Session": TEST_SESSION_ID,
        "X-mIV-Page-Identity": pageIdentityHeader(identity),
      },
    });
  };
  const page = (number) => ({
    entry: { name: `Page ${number}` },
    info: { width: 1200, height: 1800 },
    request: {
      url: `/api/page?spread-mismatch=${number}`,
      cacheKey: `spread-identity-mismatch-${number}`,
      remoteStateGeneration: "test-1",
      remoteSessionId: TEST_SESSION_ID,
      address: requested(number - 1),
      width: 1334,
      cssWidth: 667,
      dpr: 2,
      layout: { cssWidth: 667 },
      fitMode: "page",
    },
  });
  try {
    const displayed = await viewer.loadGroup({
      pages: [page(1), page(2)],
      name: "Mismatched spread",
      fitMode: "page",
      gap: 12,
      index: 0,
      count: 1,
      interactionStartedAt: performance.now(),
    });
    assert.equal(displayed.outcome, ViewerGroupLoadOutcome.FAILED);
    assert.match(displayed.message, /identity/);
    assert.equal(fetchCount, 2);
    assert.deepEqual(pageLayer.children, [initialImage]);
    assert.equal(errors.length, 1);
    assert.equal(errors[0].category, "page_identity_mismatch");
  } finally {
    setRuntimeTestErrorObserver(null);
    globalThis.fetch = imageFetch;
    viewer.destroy();
  }
});

test("folder list becomes renderable before spread metadata and open waits for it", async () => {
  let resolveContainer;
  let containerRequested = false;
  globalThis.fetch = async (input) => {
    const url = new URL(input, testLocation.origin);
    if (url.pathname === "/api/list") {
      return Response.json({
        path: testPath("book"),
        thumb_aspect_height_ratio: 1,
        entries: [
          { kind: "dir", name: "child", path: testPath("book/child") },
          { kind: "image", name: "001.jpg", path: testPath("book/001.jpg") },
          { kind: "image", name: "002.jpg", path: testPath("book/002.jpg") },
          { kind: "video", name: "clip.mp4", path: testPath("book/clip.mp4") },
        ],
      });
    }
    if (url.pathname === "/api/container") {
      containerRequested = true;
      return new Promise((resolve) => { resolveContainer = resolve; });
    }
    throw new Error(`unexpected request: ${url.pathname}`);
  };

  try {
    const loaded = await loadFolder(testPath("book"), performance.now());
    assert.equal(containerRequested, true);
    assert.equal(loaded.metrics.entryCount, 4);
    assert.equal(loaded.metrics.containerCount, 0);

    const pageAddress = {
      path: testPath("book/002.jpg"),
      subresource: { kind: "file" },
    };
    let viewerPreparationSettled = false;
    const viewerPreparation = activateFolderContainerForImage(pageAddress, 2)
      .then((value) => {
        viewerPreparationSettled = true;
        return value;
      });
    await Promise.resolve();
    assert.equal(viewerPreparationSettled, false);

    const folderAddress = parentContainerAddress(pageAddress);
    const page = (name) => ({
      kind: "image",
      name,
      address: {
        path: testPath(`book/${name}`),
        subresource: { kind: "file" },
      },
    });
    resolveContainer(Response.json({
      kind: "folder",
      title: "book",
      effective_address: folderAddress,
      entries: [page("002.jpg"), page("001.jpg")],
      configured_spread_mode: "rtl",
      effective_spread_mode: "rtl",
      reading_direction: "rtl",
      spread_page_gap_px: 8,
      page_groups: [
        {
          anchor: page("002.jpg").address,
          pages: [page("001.jpg").address, page("002.jpg").address],
        },
      ],
      entry_limit: 1000,
      truncated: false,
    }));
    assert.equal(await viewerPreparation, 0);
  } finally {
    globalThis.fetch = imageFetch;
  }
});

test("a previous folder container result cannot satisfy the current folder", async () => {
  const containerResolvers = new Map();
  globalThis.fetch = async (input) => {
    const url = new URL(input, testLocation.origin);
    const path = url.searchParams.get("path");
    if (url.pathname === "/api/list") {
      return Response.json({
        path,
        thumb_aspect_height_ratio: 1,
        entries: [
          { kind: "image", name: "001.jpg", path: `${path}/001.jpg` },
        ],
      });
    }
    if (url.pathname === "/api/container") {
      return new Promise((resolve) => { containerResolvers.set(path, resolve); });
    }
    throw new Error(`unexpected request: ${url.pathname}`);
  };

  const containerResponse = (path) => {
    const address = {
      path: testPath(path),
      subresource: { kind: "file" },
    };
    const pageAddress = {
      path: testPath(`${path}/001.jpg`),
      subresource: { kind: "file" },
    };
    return {
      pageAddress,
      response: Response.json({
        kind: "folder",
        title: path,
        effective_address: address,
        entries: [{ kind: "image", name: "001.jpg", address: pageAddress }],
        configured_spread_mode: "single",
        effective_spread_mode: "single",
        reading_direction: "ltr",
        spread_page_gap_px: 0,
        page_groups: [{ anchor: pageAddress, pages: [pageAddress] }],
        entry_limit: 1000,
        truncated: false,
      }),
    };
  };

  try {
    const oldFolder = await loadFolder(testPath("old"));
    const currentFolder = await loadFolder(testPath("current"));
    assert.equal(oldFolder.requestController.signal.aborted, true);

    const current = containerResponse("current");
    let currentPreparationSettled = false;
    const currentPreparation = activateFolderContainerForImage(
      current.pageAddress,
      0
    ).then((value) => {
      currentPreparationSettled = true;
      return value;
    });
    containerResolvers.get(testPath("old"))(containerResponse("old").response);
    await oldFolder.containerLoad.promise;
    await Promise.resolve();
    assert.equal(currentPreparationSettled, false);

    containerResolvers.get(testPath("current"))(current.response);
    assert.equal(await currentPreparation, 0);
    assert.equal(currentFolder.requestController.signal.aborted, false);
  } finally {
    globalThis.fetch = imageFetch;
  }
});

test("standalone reload is local and preserves the current hash", () => {
  const before = reloadCalls;
  testLocation.hash = "#image/C%3A%2Fmiv-test%2Fbook%2F002.jpg";
  reloadApplication();
  assert.equal(reloadCalls, before + 1);
  assert.equal(testLocation.hash, "#image/C%3A%2Fmiv-test%2Fbook%2F002.jpg");
});

test("a coordinated display retries momentary admission refusal with the same lease", async () => {
  let attempts = 0;
  const cache = new PageResourceCache(4);
  const adapter = new PageDemandAdapter({
    cache,
    wirePrefix: "retry",
    fetchResource: async (request) => {
      attempts += 1;
      assert.match(request.url, /[?&]job=retry-j1(?:&|$)/);
      if (attempts < 3) {
        const error = new Error("busy");
        error.status = 503;
        error.code = "ipc_busy";
        error.retryAfterMs = 1;
        throw error;
      }
      return {
        blob: new Blob(["ready"]),
        requestId: "ok",
        fetchMs: 1,
        info: null,
      };
    },
    postDemand: async () => ({}),
  });
  const request = adapterPageRequest("retry");
  adapter.openDisplay({
    requestId: "retry-display",
    groupKey: "retry-group",
    requests: [request],
  });

  await adapter.waitForDisplay("retry-display");
  assert.equal(adapter.resourceForKey("retry").requestId, "ok");
  assert.equal(attempts, 3);
  assert.ok(attempts <= FOREGROUND_ADMISSION_RETRY_LIMIT + 1);
  adapter.releaseDisplay("retry-display");
});

test("latest-only preview supersession exposes a cancellation hook", async () => {
  const firstStarted = deferred();
  const cancelled = [];
  const queue = new LatestOnlyTaskQueue(
    async (job) => {
      if (job.id === "preview-2") return;
      firstStarted.resolve();
      await new Promise((resolve) => {
        job.controller.signal.addEventListener("abort", resolve, { once: true });
      });
    },
    () => {},
    () => false,
    (job) => {
      job.controller.abort();
      cancelled.push(job.id);
    }
  );
  const first = { id: "preview-1", controller: new AbortController() };
  const second = { id: "preview-2", controller: new AbortController() };
  queue.enqueue(first);
  await firstStarted.promise;
  queue.enqueue(second);

  assert.deepEqual(cancelled, ["preview-1"]);
  assert.equal(first.controller.signal.aborted, true);
});
