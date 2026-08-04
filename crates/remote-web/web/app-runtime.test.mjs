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
const imageFetch = async () => new Response(new Blob([new Uint8Array([1, 2, 3])]), {
  status: 200,
  headers: {
    "Content-Type": "image/jpeg",
    "X-mIV-Request-Id": "runtime-test",
    "X-mIV-Image-Width": "1200",
    "X-mIV-Image-Height": "1800",
  },
});
globalThis.fetch = imageFetch;

const {
  ImageViewer,
  LatestOnlyTaskQueue,
  VIEWER_MENU_MAX_ACTIONS,
  VIEWER_PANEL_TABS,
  activateFolderContainerForImage,
  commandTelemetryEvent,
  containerInitialImageIndex,
  createGridTile,
  loadFolder,
  normalizeRemoteAdjustmentValues,
  normalizeRemoteColorizeParams,
  parentContainerAddress,
  reloadApplication,
  remoteAiProgressText,
  remoteAiPollingDelay,
  resolveMediaOpenRoute,
  selectRecoverableRemoteAiJob,
  thumbnailAddressForEntry,
  videoFileTargetIndex,
  viewerMenuDefinitions,
} = await import("./app.js");

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

test("a folder-list video uses its absorbed sidecar as the thumbnail source", () => {
  const video = {
    favorite_id: "favorite",
    relative_path: "movies/clip.mp4",
    subresource: { kind: "file" },
  };
  const sidecar = {
    favorite_id: "favorite",
    relative_path: "movies/clip.jpg",
    subresource: { kind: "file" },
  };
  assert.equal(
    thumbnailAddressForEntry({ address: video, thumbnail_address: sidecar }),
    sidecar
  );
  assert.equal(thumbnailAddressForEntry({ address: video }), video);
});


test("container opening resumes the matching page and otherwise falls back safely", () => {
  const page = (pageNumber) => ({
    kind: "image",
    address: {
      favorite_id: "favorite",
      relative_path: "book.pdf",
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

test("viewer display menu exposes all image quality modes and marks the device choice", () => {
  const definitions = viewerMenuDefinitions({
    hasContainer: true,
    barsVisible: true,
    imageQuality: "light",
  });
  const qualityActions = definitions.display.actions
    .filter(([name]) => name === "set_image_quality");
  assert.deepEqual(
    qualityActions.map(([, label, hint, payload]) => [
      label,
      hint,
      payload.quality,
    ]),
    [
      ["高品質", "最大 8192 px", "high"],
      ["標準", "最大 4096 px", "standard"],
      ["✓ 軽量", "最大 2048 px", "light"],
      ["最軽量", "最大 1024 px", "minimum"],
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
      favorite_id: "favorite",
      relative_path: "books/volume/page-002.jpg",
      subresource: { kind: "file" },
    }),
    {
      favorite_id: "favorite",
      relative_path: "books/volume",
      subresource: { kind: "file" },
    }
  );
});

test("tapping a video grid tile preserves the video route", () => {
  const address = {
    favorite_id: "favorite",
    relative_path: "clips/movie.mp4",
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

test("folder list becomes renderable before spread metadata and open waits for it", async () => {
  let resolveContainer;
  let containerRequested = false;
  globalThis.fetch = async (input) => {
    const url = new URL(input, testLocation.origin);
    if (url.pathname === "/api/list") {
      return Response.json({
        path: "book",
        thumb_aspect_height_ratio: 1,
        entries: [
          { kind: "dir", name: "child", path: "book/child" },
          { kind: "image", name: "001.jpg", path: "book/001.jpg" },
          { kind: "image", name: "002.jpg", path: "book/002.jpg" },
          { kind: "video", name: "clip.mp4", path: "book/clip.mp4" },
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
    const loaded = await loadFolder("favorite", "book", performance.now());
    assert.equal(containerRequested, true);
    assert.equal(loaded.metrics.entryCount, 4);
    assert.equal(loaded.metrics.containerCount, 0);

    const pageAddress = {
      favorite_id: "favorite",
      relative_path: "book/002.jpg",
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
        favorite_id: "favorite",
        relative_path: `book/${name}`,
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
      favorite_id: "favorite",
      relative_path: path,
      subresource: { kind: "file" },
    };
    const pageAddress = {
      favorite_id: "favorite",
      relative_path: `${path}/001.jpg`,
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
    const oldFolder = await loadFolder("favorite", "old");
    const currentFolder = await loadFolder("favorite", "current");
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
    containerResolvers.get("old")(containerResponse("old").response);
    await oldFolder.containerLoad.promise;
    await Promise.resolve();
    assert.equal(currentPreparationSettled, false);

    containerResolvers.get("current")(current.response);
    assert.equal(await currentPreparation, 0);
    assert.equal(currentFolder.requestController.signal.aborted, false);
  } finally {
    globalThis.fetch = imageFetch;
  }
});

test("standalone reload is local and preserves the current hash", () => {
  const before = reloadCalls;
  testLocation.hash = "#image/favorite/book%2F002.jpg";
  reloadApplication();
  assert.equal(reloadCalls, before + 1);
  assert.equal(testLocation.hash, "#image/favorite/book%2F002.jpg");
});
