import test from "node:test";
import assert from "node:assert/strict";

class FakeElement {
  constructor(tag = "div") {
    this.tagName = tag.toUpperCase();
    this.style = {};
    this.dataset = {};
    this.classList = { add() {}, remove() {} };
    this.hidden = false;
    this.clientWidth = 430;
    this.clientHeight = 800;
    this.naturalWidth = 1200;
    this.naturalHeight = 1800;
    this.children = [];
    this.replacedWith = null;
  }

  addEventListener() {}
  removeEventListener() {}
  setAttribute() {}
  append(...nodes) { this.children.push(...nodes); }
  replaceChildren(...nodes) { this.children = nodes; }
  replaceWith(node) { this.replacedWith = node; }
  async decode() {}
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
    "Content-Type": "image/webp",
    "X-mIV-Request-Id": "runtime-test",
    "X-mIV-Image-Width": "1200",
    "X-mIV-Image-Height": "1800",
  },
});
globalThis.fetch = imageFetch;

const {
  ImageViewer,
  activateFolderContainerForImage,
  loadFolder,
  parentContainerAddress,
  reloadApplication,
} = await import("./app.js");

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
    assert.equal(loaded.metrics.entryCount, 3);
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
