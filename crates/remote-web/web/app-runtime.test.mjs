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
globalThis.window = {
  innerWidth: 430,
  innerHeight: 800,
  devicePixelRatio: 2,
  addEventListener() {},
  removeEventListener() {},
};
globalThis.location = { origin: "http://127.0.0.1:8787" };
globalThis.requestAnimationFrame = (callback) => {
  callback(performance.now());
  return 1;
};
globalThis.cancelAnimationFrame = () => {};
globalThis.fetch = async () => new Response(new Blob([new Uint8Array([1, 2, 3])]), {
  status: 200,
  headers: {
    "Content-Type": "image/webp",
    "X-mIV-Request-Id": "runtime-test",
    "X-mIV-Image-Width": "1200",
    "X-mIV-Image-Height": "1800",
  },
});

const { ImageViewer } = await import("./app.js");

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
  assert.equal(initialImage.replacedWith, viewer.image);
  assert.equal(viewer.image.style.width, "430px");
  assert.equal(viewer.image.dataset.sourceWidth, "1200");
  assert.equal(loadingIndicator.hidden, true);
  viewer.destroy();
});
