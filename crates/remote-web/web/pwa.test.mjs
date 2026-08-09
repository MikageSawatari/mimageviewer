import test from "node:test";
import assert from "node:assert/strict";
import { readFile as readFileRaw } from "node:fs/promises";
import { runInNewContext } from "node:vm";

// 検査は原文の構造を見るので、行末の違いで結果が変わってはいけない。
// 作業コピーの行末は core.autocrlf と書き手の都合で LF にも CRLF にもなる。
const readFile = async (url, encoding) => {
  const contents = await readFileRaw(url, encoding);
  return typeof contents === "string"
    ? contents.split("\r\n").join("\n")
    : contents;
};

const here = new URL("./", import.meta.url);

async function loadServiceWorker({ fetchImpl, cachedResponse }) {
  const listeners = new Map();
  const worker = await readFile(new URL("service-worker.js", here), "utf8");
  runInNewContext(worker, {
    self: {
      addEventListener(type, listener) {
        listeners.set(type, listener);
      },
      skipWaiting: async () => {},
      clients: { claim: async () => {} },
    },
    caches: {
      open: async () => ({ add: async () => {} }),
      keys: async () => [],
      delete: async () => true,
      match: async () => cachedResponse,
    },
    fetch: fetchImpl,
    Request,
    Response,
  });
  return listeners;
}

test("manifest describes the standalone shell and every supplied icon", async () => {
  const manifest = JSON.parse(
    await readFile(new URL("manifest.webmanifest", here), "utf8")
  );
  assert.equal(manifest.name, "mIV Remote");
  assert.ok(manifest.short_name);
  assert.equal(manifest.display, "standalone");
  assert.equal(manifest.start_url, "./");
  assert.equal(manifest.scope, "./");
  assert.equal(manifest.background_color, "#111318");
  assert.equal(manifest.theme_color, "#111318");

  const expected = new Map([
    ["icons/icon-192.png", [192, "any"]],
    ["icons/icon-512.png", [512, "any"]],
    ["icons/maskable-192.png", [192, "maskable"]],
    ["icons/maskable-512.png", [512, "maskable"]],
  ]);
  assert.equal(manifest.icons.length, expected.size);
  for (const icon of manifest.icons) {
    const [size, purpose] = expected.get(icon.src) ?? [];
    assert.ok(size, `unexpected manifest icon: ${icon.src}`);
    assert.equal(icon.sizes, `${size}x${size}`);
    assert.equal(icon.type, "image/png");
    assert.equal(icon.purpose, purpose);
    assert.deepEqual(await pngDimensions(icon.src), [size, size]);
  }
  assert.deepEqual(await pngDimensions("icons/icon-180.png"), [180, 180]);
});

test("HTML advertises the PWA and iOS standalone metadata", async () => {
  const html = await readFile(new URL("index.html", here), "utf8");
  assert.match(html, /name="viewport"[^>]*viewport-fit=cover/);
  assert.match(html, /name="apple-mobile-web-app-capable" content="yes"/);
  assert.match(
    html,
    /name="apple-mobile-web-app-status-bar-style" content="black-translucent"/
  );
  assert.match(html, /rel="manifest" href="\/manifest\.webmanifest"/);
  assert.match(
    html,
    /rel="apple-touch-icon" sizes="180x180" href="\/icons\/icon-180\.png"/
  );
  assert.match(
    html,
    /name="miv-remote-asset-token" content="__MIV_REMOTE_ASSET_TOKEN__"/
  );
});

test("the shell registers a root-scoped service worker without script caching", async () => {
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.match(
    app,
    /navigator\.serviceWorker\s*\.register\("\/service-worker\.js",\s*\{\s*scope:\s*"\/",\s*updateViaCache:\s*"none"\s*\}\)/
  );
});

test("the service worker only falls back for failed page navigations", async () => {
  const worker = await readFile(new URL("service-worker.js", here), "utf8");
  assert.match(worker, /request\.method !== "GET" \|\| request\.mode !== "navigate"/);
  assert.match(
    worker,
    /fetch\(request\)[\s\S]*response\.status < 500[\s\S]*offlineNavigation/
  );
  assert.match(worker, /\.catch\(\(\) => offlineNavigation\(\)\)/);
});

test("API requests bypass the worker while a 502 navigation uses the cached guide", async () => {
  let networkCalls = 0;
  const listeners = await loadServiceWorker({
    fetchImpl: async () => {
      networkCalls += 1;
      return new Response("", { status: 502 });
    },
    cachedResponse: new Response("<h1>cached guide</h1>", {
      headers: { "Content-Type": "text/html; charset=utf-8" },
    }),
  });
  const onFetch = listeners.get("fetch");

  let apiIntercepted = false;
  onFetch({
    request: { method: "GET", mode: "cors", url: "https://example.test/api/page" },
    respondWith() {
      apiIntercepted = true;
    },
  });
  assert.equal(apiIntercepted, false);
  assert.equal(networkCalls, 0);

  let navigationResponse;
  onFetch({
    request: { method: "GET", mode: "navigate", url: "https://example.test/" },
    respondWith(response) {
      navigationResponse = response;
    },
  });
  assert.equal(await (await navigationResponse).text(), "<h1>cached guide</h1>");
  assert.equal(networkCalls, 1);
});

test("successful navigations always return the current network response", async () => {
  const listeners = await loadServiceWorker({
    fetchImpl: async () => new Response("<h1>fresh shell</h1>"),
    cachedResponse: new Response("<h1>cached guide</h1>"),
  });
  let navigationResponse;
  listeners.get("fetch")({
    request: { method: "GET", mode: "navigate", url: "https://example.test/" },
    respondWith(response) {
      navigationResponse = response;
    },
  });
  assert.equal(await (await navigationResponse).text(), "<h1>fresh shell</h1>");
});

test("the offline cache contains only the static connection guide", async () => {
  const worker = await readFile(new URL("service-worker.js", here), "utf8");
  assert.match(worker, /const OFFLINE_URL = "\/offline\.html"/);
  assert.match(
    worker,
    /cache\.add\(new Request\(OFFLINE_URL, \{ cache: "reload" \}\)\)/
  );
  assert.doesNotMatch(worker, /cache\.put|addAll/);
  assert.doesNotMatch(worker, /\/api\/|\/app\.js|\/styles\.css|\/icons\/|thumbnail/);
});

test("the offline guide is standalone, actionable, and uses the manifest background", async () => {
  const html = await readFile(new URL("offline.html", here), "utf8");
  assert.match(html, /mIV に接続できません/);
  assert.match(html, /PC で mIV が起動していること/);
  assert.match(html, /リモート接続が有効/);
  assert.match(html, /href="\/">もう一度試す/);
  assert.match(html, /background:\s*#111318/);
  assert.doesNotMatch(html, /service worker|502|プロキシ|ポート|IPC/i);
});

test("the hidden attribute always overrides component display rules", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  assert.match(
    css,
    /(?:^|\n)\[hidden\]\s*\{[^}]*display:\s*none\s*!important\s*;/
  );
});

test("update banner resolves viewport width before laying out text and buttons", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  const banner = css.match(/\.app-update-banner\s*\{([^}]*)\}/)?.[1] ?? "";
  assert.match(banner, /left:\s*max\(16px,\s*env\(safe-area-inset-left/);
  assert.match(banner, /right:\s*max\(16px,\s*env\(safe-area-inset-right/);
  assert.match(banner, /flex-wrap:\s*wrap/);
  assert.match(banner, /justify-content:\s*center/);
  assert.doesNotMatch(banner, /left:\s*50%|translateX\(-50%\)/);
  assert.match(
    css,
    /\.app-update-banner\s*>\s*span\s*\{[^}]*flex:\s*1\s+1\s+16ch;[^}]*min-width:\s*0;/
  );
  assert.match(css, /\.app-update-banner button\s*\{[^}]*flex:\s*none;/);
  assert.match(css, /\.viewer-boundary-message\s*\{[^}]*width:\s*max-content;/);
});

test("adjustment ranges keep normalized handles and expose their actual values", async () => {
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.match(app, /input\.min\s*=\s*"0";\s*input\.max\s*=\s*"1";\s*input\.step\s*=\s*"any";/);
  assert.match(app, /input\.setAttribute\("aria-valuemin",\s*String\(min\)\)/);
  assert.match(app, /input\.setAttribute\("aria-valuemax",\s*String\(max\)\)/);
  assert.match(app, /control\.input\.setAttribute\("aria-valuenow",\s*String\(value\)\)/);
  assert.match(app, /control\.input\.setAttribute\("aria-valuetext",\s*valueText\)/);
});

test("session epoch removes page preflight and repeated active acquisition", async () => {
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.doesNotMatch(app, /["']\/api\/remote-state["']/);
  assert.match(
    app,
    /decision === "use_current" && state\.remoteSessionId\) return true/
  );
  assert.match(
    app,
    /acquireRemoteSession[\s\S]*applyRemoteStateGeneration\(result\.remote_state_generation,\s*\{\s*reloadViewer:\s*true\s*\}\)/
  );
  assert.match(
    app,
    /pingRemoteSession[\s\S]*applyRemoteStateGeneration\(result\.remote_state_generation,\s*\{\s*reloadViewer:\s*true\s*\}\)/
  );
  assert.match(app, /commitSeekGroup[\s\S]*acquireRemoteSession\(reason\)/);
  assert.match(app, /for \(let attempt = 0; ; attempt \+= 1\)/);
  assert.doesNotMatch(app, /attempt < 80/);
  assert.match(
    app,
    /function applyRemoteSessionId[\s\S]*invalidateViewerPendingLoad\(state\.viewer\)/
  );
  assert.doesNotMatch(app, /state\.viewer\?\.invalidatePendingLoad\(\)/);
});

test("the running app version comes from the shell and acquisition can reload only once", async () => {
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.match(
    app,
    /querySelector\('meta\[name="miv-remote-asset-token"\]'\)\?\.content/
  );
  assert.match(
    app,
    /reconcileAppVersionAfterSessionAcquire\(result\.asset_token\)/
  );
  assert.match(
    app,
    /sessionStorage\?\.setItem\(APP_UPDATE_RELOAD_ATTEMPT_KEY,[\s\S]*sessionStorage\?\.getItem\(APP_UPDATE_RELOAD_ATTEMPT_KEY\) === attempt/
  );
});

test("critical telemetry is submitted before session-transition UI side effects", async () => {
  const app = await readFile(new URL("app.js", here), "utf8");
  // 関数本体だけを取り出す。閉じ括弧が桁 0 に来るのは top-level 関数の終端だけ。
  // 後ろに続く宣言を目印にすると、その宣言を足しただけでこの検査が空振りする。
  const statusOwner = app.match(
    /function setRemoteSessionStatus[\s\S]*?\n}\n/
  )?.[0] ?? "";
  assert.match(statusOwner, /remoteSessionTransitionTelemetry\(/);
  assert.match(statusOwner, /if \(transitionEvent\) enqueueTelemetry\(transitionEvent\)/);
  assert.ok(
    statusOwner.indexOf("enqueueTelemetry(transitionEvent)") <
      statusOwner.indexOf("applyRemoteSessionId"),
    "the transition beacon must be queued before cache/viewer invalidation can throw"
  );
  assert.ok(
    statusOwner.indexOf("enqueueTelemetry(transitionEvent)") <
      statusOwner.indexOf('document.querySelector("#remote-session-status")'),
    "the transition beacon must be queued before modal rendering can throw"
  );
  assert.match(
    app,
    /telemetryDeliveryMode\(stampedEvent\) === "immediate"[\s\S]*sendImmediateTelemetry\(stampedEvent\)/
  );
  assert.match(app, /navigator\.sendBeacon\([\s\S]*"\/api\/telemetry"/);
  assert.match(app, /client_event_sequence:\s*telemetryState\.nextSequence\+\+/);
  assert.match(app, /observer:\s*"ping"/);
  assert.match(app, /observer:\s*"acquire"/);
  assert.match(app, /path === "\/api\/video\/state"\) return "video_poll"/);
});

test("video health HUD and persistent debug-tier warning stay wired", async () => {
  const app = await readFile(new URL("app.js", here), "utf8");
  const css = await readFile(new URL("styles.css", here), "utf8");
  assert.match(app, /publishVideoHealth:[\s\S]*hudState\.video = snapshot/);
  assert.match(app, /buffer_ahead_secs/);
  assert.match(app, /dropped_video_frames/);
  assert.match(app, /詳細記録 ON/);
  assert.match(app, /telemetryDebugDetails[\s\S]*openLocalSettingsDialog\(\)/);
  assert.match(css, /#telemetry-hud\[data-telemetry-tier="debug"\]/);
  assert.match(app, /dataset\.viewerKind = video \? "video" : "default"/);
  assert.match(css, /#telemetry-hud\[data-viewer-kind="video"\][\s\S]*bottom:/);
});

test("colorize controls use the adjustment preview and commit path without losing custom points", async () => {
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.match(app, /normalizeRemoteColorizeParams\(source\.colorize\)/);
  assert.match(app, /control_points:\s*pointSource\.map/);
  assert.match(app, /this\.addColorizeSlider\([\s\S]*"mono_tolerance"/);
  assert.match(app, /this\.addColorizeSlider\([\s\S]*"density_normalization_strength"/);
  assert.match(app, /this\.addColorizeSlider\([\s\S]*"luminance_weight"/);
  assert.match(app, /this\.addColorizeSlider\([\s\S]*"tone_radius"/);
  assert.match(app, /this\.addColorizeSlider\([\s\S]*"tone_strength"/);
  assert.match(
    app,
    /"tone_strength",\s*"トーン密度の強さ",\s*0,\s*100,\s*1,\s*\(\) => this\.values\.colorize\.tone_method !== "off"/
  );
  assert.match(app, /adjustmentPreview:\s*\{\s*scope:\s*job\.scope,\s*values:\s*job\.values\s*\}/);
  assert.match(app, /kind:\s*"set_adjustment"[\s\S]*values:\s*normalizeRemoteAdjustmentValues/);
  assert.doesNotMatch(app, /readOnly\.colorize_enabled/);
});

test("safe areas protect portrait bars and landscape side controls", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.match(css, /\.topbar[\s\S]*safe-area-inset-top[\s\S]*safe-area-inset-right[\s\S]*safe-area-inset-left/);
  assert.match(css, /\.page-content[\s\S]*safe-area-inset-top[\s\S]*safe-area-inset-right[\s\S]*safe-area-inset-bottom[\s\S]*safe-area-inset-left/);
  assert.match(css, /\.viewer-ui[\s\S]*safe-area-inset-right[\s\S]*safe-area-inset-left/);
  assert.match(css, /\.viewer-ui\.top[\s\S]*safe-area-inset-top/);
  assert.match(css, /\.viewer-ui\.bottom[\s\S]*safe-area-inset-bottom/);
  assert.match(css, /\.viewer-seek[\s\S]*safe-area-inset-left[\s\S]*safe-area-inset-right/);
  assert.match(css, /\.command-menu[\s\S]*safe-area-inset-right[\s\S]*safe-area-inset-bottom[\s\S]*safe-area-inset-left/);
  assert.match(css, /\.virtual-window[\s\S]*safe-area-inset-right[\s\S]*safe-area-inset-left/);
  assert.match(app, /--grid-inline-inset/);
  assert.doesNotMatch(app, /windowElement\.style\.(?:left|right)\s*=/);
  assert.doesNotMatch(css, /\.screen > \.topbar > :not\(\.menu-trigger\)/);
});

test("search and tags keep flexible home tabs and iOS-safe text input sizing", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  assert.match(
    css,
    /\.home-tabs\s*\{[^}]*grid-template-columns:\s*repeat\(auto-fit,\s*minmax\(56px,\s*1fr\)\)/
  );
  assert.match(
    css,
    /\.favorite-search-input,[\s\S]*\.tag-kind-select\s*\{[^}]*font-size:\s*16px/
  );
  assert.match(css, /\.tag-filter-input/);
});

test("still-image panel reserves the agreed viewport while its transparent shield owns input", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.match(
    css,
    /\.image-viewer\.viewer-panel-open\.viewer-panel-portrait > \.viewer-stage\s*\{[^}]*bottom:\s*50%/
  );
  assert.match(
    css,
    /\.image-viewer\.viewer-panel-open\.viewer-panel-landscape > \.viewer-stage\s*\{[^}]*left:\s*40%/
  );
  assert.match(
    css,
    /\.viewer-command-menu-layer > \.viewer-command-menu\s*\{[^}]*width:\s*100%;[^}]*height:\s*50%/
  );
  assert.match(
    css,
    /\.viewer-command-menu-layer\[data-orientation="landscape"\] > \.viewer-command-menu\s*\{[^}]*width:\s*40%;[^}]*height:\s*100%/
  );
  assert.match(
    css,
    /\.viewer-command-menu-layer > \.command-menu-scrim\s*\{[^}]*background:\s*transparent/
  );
  assert.match(
    css,
    /\.viewer-command-menu-layer\[data-motion="opening"\] > \.viewer-command-menu\s*\{[^}]*animation:\s*viewer-panel-rise-in/
  );
  assert.match(
    css,
    /@keyframes viewer-panel-rise-in\s*\{[^}]*translate3d\(0,\s*100%,\s*0\)/
  );
  assert.match(
    css,
    /\.viewer-command-menu-layer\[data-motion="closing"\] > \.viewer-command-menu\s*\{[^}]*animation:\s*viewer-panel-drop-out/
  );
  assert.match(app, /classList\.add\("viewer-command-menu-layer"\)/);
  assert.match(app, /viewerPanelTab:\s*"functions"/);
});

test("adjustment slider resets keep a stable reserved layout slot", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.match(
    css,
    /\.adjustment-slider-reset-slot\s*\{[^}]*width:\s*36px;[^}]*min-height:\s*36px;/
  );
  assert.match(app, /resetSlot\.append\(resetButton\)/);
  assert.match(app, /resetButton\.hidden\s*=\s*!adjustmentResetVisible/);
});

test("adjustment ranges yield vertical pan without weakening viewer gesture owners", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  const html = await readFile(new URL("index.html", here), "utf8");
  assert.match(
    css,
    /\.adjustment-slider-row input\[type="range"\]\s*\{[^}]*touch-action:\s*pan-y/
  );
  assert.match(css, /\.viewer-seek-input\s*\{[^}]*touch-action:\s*none/);
  assert.match(css, /\.image-viewer\s*\{[^}]*touch-action:\s*none/);
  assert.match(css, /\.viewer-stage\s*\{[^}]*touch-action:\s*none/);
  assert.match(
    css,
    /html,\s*body,\s*#app\s*\{[^}]*touch-action:\s*manipulation/
  );
  assert.doesNotMatch(html, /maximum-scale|user-scalable/i);
});

test("image fit transforms only the page layer while controls stay its siblings", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.match(app, /viewerRoot\.append\(stage, top, bottom\)/);
  assert.match(app, /this\.pageLayer\.style\.transform\s*=/);
  assert.doesNotMatch(app, /(?:top|bottom)\.style\.transform\s*=/);
  assert.match(css, /\.viewer-ui\s*\{[^}]*position:\s*absolute/);
});

test("browser, viewer, and layout geometry are attached to image and refit events", async () => {
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.match(app, /visualViewport\?\.addEventListener\?\.\("resize"/);
  assert.match(app, /visualViewportScaleTransition\(/);
  assert.match(app, /\}, 250\);/);
  assert.match(app, /precedingBrowserTapPairTelemetry\(/);
  assert.match(app, /onDecision:\s*recordBrowserDoubleTapDecision/);
  assert.match(
    app,
    /fit_mode:\s*request\.fitMode,[\s\S]*?\.\.\.viewerTransformTelemetry\(this\.scale,/
  );
  assert.match(app, /type:\s*"browser_double_tap"/);
  assert.match(app, /action:\s*"pinch_end"/);
  assert.match(app, /type:\s*"viewer_layout",[\s\S]*?action:\s*"refit"/);
  assert.match(app, /renderTrigger:\s*"viewport_resize"/);
});

test("viewer taps dispatch immediately and have no application double-tap fit path", async () => {
  const app = await readFile(new URL("app.js", here), "utf8");
  const tapBranch = app.slice(
    app.indexOf("} else if (gesture === ViewerGesture.TAP)"),
    app.indexOf("} else if (gesture === ViewerGesture.PAN)")
  );
  assert.match(tapBranch, /dispatchCommand\(viewerTapCommand\(/);
  assert.doesNotMatch(tapBranch, /setTimeout|double_tap|FIT_/);
  assert.doesNotMatch(
    app,
    /viewerTapSequence|pending_center_tap|centerTapTimer|fit_toggle_page_original/
  );
});

test("page request feedback precedes load while display position commits at replacement", async () => {
  const app = await readFile(new URL("app.js", here), "utf8");
  const navigation = app.slice(
    app.indexOf("function changeImageTo("),
    app.indexOf("async function updateViewerImage(")
  );
  assert.doesNotMatch(navigation, /setSeekState\(/);
  assert.match(navigation, /updateRequestedPageGroup/);
  assert.match(app, /viewer\.setRequestedPagePresentation/);
  assert.match(
    app,
    /this\.pageLayer\.replaceChildren\(decodedImage\);[\s\S]*?this\.commitPagePresentation\(presentation\);/
  );
  assert.match(
    app,
    /this\.pageLayer\.replaceChildren\(\.\.\.decodedImages[\s\S]*?this\.commitPagePresentation\(presentation\);/
  );
});

test("image tiles preserve portrait and landscape shape below a separate label row", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  const app = await readFile(new URL("app.js", here), "utf8");
  // A percentage height in the preview's implicit auto grid row resolves to auto,
  // so the image must be taken out of that row and pinned to the preview box.
  assert.match(
    css,
    /\.tile-preview img\s*\{[^}]*position:\s*absolute;[^}]*inset:\s*0;[^}]*width:\s*100%;[^}]*height:\s*100%;/
  );
  assert.match(
    css,
    /\.folder-glyph,\s*\.file-glyph\s*\{[^}]*position:\s*absolute;[^}]*inset:\s*0;/
  );
  assert.match(css, /\.tile-preview img\s*\{[^}]*object-fit:\s*contain/);
  assert.doesNotMatch(css, /\.tile-preview img\s*\{[^}]*object-fit:\s*cover/);
  assert.match(css, /\.grid-tile\.image-tile \.tile-preview img\s*\{[^}]*object-fit:\s*contain/);
  assert.match(css, /\.grid-tile\s*\{[^}]*grid-template-rows:\s*var\(--grid-preview-height\) var\(--grid-label-height\)/);
  assert.match(css, /\.grid-cursor-visible \.grid-tile\.grid-active::after\s*\{[^}]*inset 0 0 0 3px var\(--accent\)/);
  assert.match(css, /\.grid-tile:focus-visible::after\s*\{[^}]*inset 0 0 0 3px var\(--accent\)/);
  assert.match(app, /tile\.append\(preview, label\)/);
  assert.match(app, /label\.append\([\s\S]*entry-detail-badge/);
  assert.doesNotMatch(css, /\.entry-detail-badge\s*\{[^}]*position:\s*absolute/);
});

test("the grid owns pinch scaling while one-finger vertical scroll stays native", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  const app = await readFile(new URL("app.js", here), "utf8");
  assert.match(css, /\.grid-scroll\s*\{[^}]*touch-action:\s*pan-y/);
  assert.match(
    app,
    /this\.onTouchMove = \(event\) => \{\s*if \(event\.touches\.length < 2\) return;\s*event\.preventDefault\(\);\s*\}/
  );
  assert.match(
    app,
    /addEventListener\("touchmove", this\.onTouchMove, \{\s*passive: false,?\s*\}\)/
  );
  assert.match(
    app,
    /removeEventListener\("touchmove", this\.onTouchMove\)/
  );
  assert.doesNotMatch(app, /縦持ちの列数|横持ちの列数/);
});

test("video owns repeated taps and pinch without allowing native page zoom", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  const video = await readFile(new URL("video-stream.mjs", here), "utf8");
  assert.match(
    css,
    /\.video-stream-viewer,\s*\.video-stream-stage,\s*\.stream-video\s*\{[^}]*touch-action:\s*none/
  );
  assert.match(
    video,
    /addEventListener\("touchend", this\.nativeGesture, \{\s*passive: false,?\s*\}\)/
  );
  assert.match(video, /addEventListener\("gesturestart", this\.nativeGesture/);
  assert.match(video, /addEventListener\("dblclick", this\.nativeGesture/);
  assert.match(video, /removeEventListener\("touchend", this\.nativeGesture\)/);
  assert.match(video, /removeEventListener\("gesturestart", this\.nativeGesture\)/);
  assert.match(video, /removeEventListener\("dblclick", this\.nativeGesture\)/);
});

test("image and video seek share tap-or-relative-drag handling without replacing native range semantics", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  const app = await readFile(new URL("app.js", here), "utf8");
  const video = await readFile(new URL("video-stream.mjs", here), "utf8");
  assert.match(app, /seekInput\.type = "range"/);
  assert.match(app, /seekInput\.setAttribute\("aria-label", "ページ位置"\)/);
  assert.match(app, /seekRangePointerGestureDecision\(\{/);
  assert.match(app, /seekRangeAbsoluteValue\(\{/);
  assert.match(video, /this\.seekInput\.type = "range"/);
  assert.match(
    video,
    /this\.seekInput\.setAttribute\("aria-label", entry\.kind === "audio" \? "音声の再生位置" : "動画の再生位置"\)/
  );
  assert.match(video, /this\.seekInput\.setAttribute\("aria-valuetext", this\.counter\.value\)/);
  assert.match(video, /seekRangePointerGestureDecision\(\{/);
  assert.match(video, /seekRangeAbsoluteValue\(\{/);
  assert.match(video, /addEventListener\("pointerdown", this\.onSeekPointerDown\)/);
  assert.match(video, /removeEventListener\("pointerdown", this\.onSeekPointerDown\)/);
  assert.match(
    css,
    /\.video-stream-seek \.viewer-seek-input\s*\{[^}]*height:\s*44px/
  );
  // 指で触っただけでリングを出さないための刻印。pointerdown の既定動作を止めて
  // 自前で focus を当てるため、指由来かどうかはブラウザには分からない。
  assert.match(app, /seekInput\.dataset\.pointerFocus = "true"/);
  assert.match(video, /this\.seekInput\.dataset\.pointerFocus = "true"/);
  assert.match(app, /delete seekInput\.dataset\.pointerFocus/);
  assert.match(video, /delete this\.seekInput\.dataset\.pointerFocus/);
  assert.match(
    css,
    /\.viewer-seek-input\[data-pointer-focus\]:focus-visible\s*\{[^}]*outline:\s*none/
  );
  assert.match(
    css,
    /\.viewer-seek-input:focus:not\(:focus-visible\)[^{]*\{[^}]*outline:\s*none/
  );
  assert.match(
    css,
    /\.viewer-seek-input:focus-visible\s*\{[^}]*outline:\s*3px solid var\(--accent\)/
  );
});

test("viewer status overlays belong to the HUD and leave with it", async () => {
  const css = await readFile(new URL("styles.css", here), "utf8");
  // バーを隠して読んでいる間は、本の上に状態表示を置かない。
  assert.match(
    css,
    /\.viewer-bars-hidden \.viewer-ai-status\s*\{[^}]*visibility:\s*hidden/
  );
});

test("a page that never boots says so instead of staying black", async () => {
  const html = await readFile(new URL("index.html", here), "utf8");
  const app = await readFile(new URL("app.js", here), "utf8");
  // 受け皿は module であってはいけない。本体が読めないときに動くためのもの。
  const fallback = html.match(/<script>[\s\S]*?<\/script>/)?.[0] ?? "";
  assert.match(fallback, /__mivRemoteAppStarted/);
  assert.match(fallback, /アプリを読み込めませんでした/);
  assert.match(fallback, /addEventListener\(\s*\n?\s*"error"/);
  assert.match(fallback, /読み込めなかったファイル/);
  assert.ok(
    html.indexOf("<script>") < html.indexOf('<script type="module"'),
    "the fallback must be installed before the module that may fail"
  );
  // 本体側は「実行され始めた」ことだけを伝える。以降の遅さは自分で画面に出す。
  assert.match(app, /window\.__mivRemoteAppStarted = true/);
});

test("sorting hands the bar back, and form controls keep the page typography", async () => {
  const app = await readFile(new URL("app.js", here), "utf8");
  const css = await readFile(new URL("styles.css", here), "utf8");
  const change = app.match(/async function changeGridSortOrder[\s\S]*?\n}\n/)?.[0] ?? "";
  assert.match(change, /gridSortWritePending = true/);
  // 印を付けたまま一覧を描き直すと、新しいバーが「保存中」の姿で作られて操作できなくなる。
  assert.ok(
    change.indexOf("state.gridSortWritePending = false") <
      change.indexOf("dispatchRoute()"),
    "the write flag must be cleared before the grid is rebuilt from it"
  );
  // iOS は 16px 未満のフォーム部品に focus すると画面全体を拡大する。
  assert.match(css, /(?:^|\n)select,\ninput,\ntextarea\s*\{[^}]*font:\s*inherit/);
  // tap 対象は祖先だけでなく自分自身にも宣言する、というこの repo の決まりに従う。
  assert.match(css, /(?:^|\n)select\s*\{[^}]*touch-action:\s*manipulation/);
  assert.match(css, /\.grid-sort-bar\s*\{[^}]*touch-action:\s*manipulation/);
});

async function pngDimensions(relativePath) {
  const bytes = await readFile(new URL(relativePath, here));
  assert.deepEqual(
    [...bytes.subarray(0, 8)],
    [137, 80, 78, 71, 13, 10, 26, 10],
    `${relativePath} must be a PNG`
  );
  return [bytes.readUInt32BE(16), bytes.readUInt32BE(20)];
}
