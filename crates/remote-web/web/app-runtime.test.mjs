import test from "node:test";
import assert from "node:assert/strict";

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
    "X-mIV-Remote-State-Generation": "test-1",
    "X-mIV-Remote-Session": TEST_SESSION_ID,
    "X-mIV-Page-Identity": pageIdentityHeader(TEST_PAGE_ADDRESS),
  },
});
globalThis.fetch = imageFetch;

const {
  ADJUSTMENT_PANEL_TABS,
  ImageViewer,
  LatestOnlyTaskQueue,
  LatestPageLoadQueue,
  RemoteAiController,
  ViewerAdjustmentPanel,
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
  renderResolvedMediaOpen,
  remoteAiCompletionMessage,
  remoteAiProgressText,
  remoteAiPollingDelay,
  remoteBookBookmarkDisplayPage,
  remoteBookBookmarkTargetEntryIndex,
  rootOpenReturnHash,
  resolveLegacyImageOpenRoute,
  resolveMediaOpenRoute,
  selectRecoverableRemoteAiJob,
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

test("browser double-tap telemetry keeps the measured suppression decision", () => {
  const event = browserDoubleTapTelemetryEvent({
    decision: "pair_suppressed",
    elapsedMs: 222.34,
    distancePx: 5.678,
    isDoubleTap: true,
    suppressed: true,
    excluded: false,
    exclusionReason: null,
    cancelable: true,
  }, 7);

  assert.deepEqual(event, {
    type: "browser_double_tap",
    action: "suppression_decision",
    decision: "pair_suppressed",
    tap_pair_sequence: 7,
    previous_tap_elapsed_ms: 222.3,
    previous_tap_distance_px: 5.7,
    recognized_double_tap: true,
    suppressed: true,
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

  queue.enqueue(1);
  await Promise.resolve();
  queue.enqueue(2);
  queue.enqueue(3);
  releaseFirst();
  for (let attempt = 0; attempt < 20 && completed.length < 2; attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }

  assert.deepEqual(started, [1, 3]);
  assert.deepEqual(completed, [1, 3]);
  assert.equal(maxActive, 1);
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
      return true;
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
  assert.deepEqual(await Promise.all([first, second, third]), [false, false, true]);
  assert.deepEqual(started, [1, 3]);
  assert.deepEqual(busy, [true, false]);
  assert.deepEqual(events, ["busy:true", "run:1", "run:3", "busy:false"]);
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

test("tapping an archive grid tile shows the same shared unsupported notice route", () => {
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
    kind: "unsupported",
    entryKind: "archive",
    entryIndex: 3,
  });
  assert.equal(
    unsupportedRemoteEntryMessage("archive"),
    "この端末ではアーカイブを開けません。"
  );
  const notice = new FakeElement("p");
  notice.hidden = true;
  assert.equal(showUnsupportedRemoteEntryNotice(notice, "archive"), true);
  assert.equal(notice.hidden, false);
  assert.equal(notice.textContent, "この端末ではアーカイブを開けません。");
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
  viewer.fetchController = { abort() { aborted = true; } };
  viewer.loadSequence = 7;
  viewer.loadingTimer = setTimeout(() => {}, 1000);

  viewer.invalidatePendingLoad();

  assert.equal(viewer.loadSequence, 8);
  assert.equal(aborted, true);
  assert.equal(viewer.fetchController, null);
  assert.equal(viewer.loadingTimer, 0);
  assert.equal(loadingIndicator.hidden, true);
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

  assert.equal(displayed, true);
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
    assert.deepEqual(await Promise.all([first, second, third]), [false, false, true]);
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
    assert.equal(displayed, false);
    assert.equal(viewer.pageLayer.children[0], initialImage);
    assert.match(title.textContent, /状態版/);
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
    assert.equal(displayed, false);
    assert.equal(fetchCount, 1);
    assert.equal(viewer.pageLayer.children[0], initialImage);
    assert.match(title.textContent, /identity/);
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

  assert.equal(displayed, true);
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
  assert.equal(singleDisplayed, true);
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
    assert.equal(displayed, false);
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
