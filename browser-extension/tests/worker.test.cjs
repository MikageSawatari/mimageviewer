"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const {
  createWorker, selectTarget, exactRule, frameBytes, rawBudget, readImage,
  IMAGE_EXTENSIONS, HOST_ERRORS, MAX_FRAME_BYTES, PENDING_KEY, RULE_ID, MENU_ID, SCRIPT_ID
} = require("../worker.js");

const root = path.resolve(__dirname, "../..");
const storeIds = Object.values(JSON.parse(fs.readFileSync(path.join(root, "browser-extension/store-keys.json"), "utf8"))).map(store => store.id);
const clone = value => structuredClone(value);
const turn = () => new Promise(resolve => setImmediate(resolve));
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const info = { menuItemId: MENU_ID, srcUrl: "https://images.example/image.png", pageUrl: "https://page.example/article?private=1" };
const tab = { id: 7, windowId: 3, url: info.pageUrl };
function geometry() {
  return {
    screenX: -900, screenY: 20, outerWidth: 1200, outerHeight: 900,
    innerWidth: 900, innerHeight: 700, devicePixelRatio: 1.5,
    context_event: {
      screenX: -550, screenY: 400, clientX: 100, clientY: 200,
      win_screenX: -900, win_screenY: 20, innerWidth: 900, innerHeight: 700, devicePixelRatio: 1.5
    }
  };
}
function response({ chunks = [Uint8Array.of(137, 80, 78)], length = null, ok = true,
  url = info.srcUrl, type = "image/png", error } = {}) {
  const stats = { reads: 0, cancelled: 0, released: 0 };
  let at = 0;
  return {
    ok, url, stats,
    headers: { get: key => ({ "Content-Length": length, "Content-Type": type })[key] ?? null },
    body: {
      cancel: async () => { stats.cancelled++; },
      getReader: () => ({
        read: async () => {
          stats.reads++;
          if (error) throw error;
          return at < chunks.length ? { done: false, value: chunks[at++] } : { done: true };
        },
        cancel: async () => { stats.cancelled++; },
        releaseLock: () => { stats.released++; }
      })
    }
  };
}

// APIs are promises except the actual callback APIs. lastError exists only while
// the callback is on the stack, matching Chrome rather than a persistent mock.
function harness(options = {}) {
  const log = [], hooks = {}, titles = [], badges = [], native = [], fetches = [];
  const storage = options.storage || {};
  let rules = clone(options.rules || []), registered = [], grants = ["https://page.example/*"];
  let callbackError;
  const events = {};
  const record = (name, fallback) => (...args) => {
    log.push({ name, args: clone(args) });
    return hooks[name] ? hooks[name](...args) : fallback(...args);
  };
  const event = name => events[name] = {
    listeners: [], addListener(listener) { this.listeners.push(listener); },
    emit(...args) { return this.listeners.map(listener => listener(...args)); }
  };
  function callback(error, fn, value) {
    callbackError = error ? { message: error } : undefined;
    try { fn(value); } finally { callbackError = undefined; }
  }
  const api = {
    i18n: { getMessage: key => key },
    runtime: {
      id: storeIds[0],
      get lastError() { return callbackError; },
      onInstalled: event("installed"), onStartup: event("startup"),
      sendNativeMessage(host, message, fn) {
        log.push({ name: "native", args: [host, clone(message)] });
        native.push(clone(message));
        if (hooks.native) hooks.native(host, message, fn, callback);
        else callback(null, fn, { ok: true, path: "C:\\Pictures\\saved.png" });
      }
    },
    action: {
      setBadgeText: record("badge", async value => { badges.push(value.text); }),
      setTitle: record("title", async value => { titles.push(value.title.split("\n").at(-1)); })
    },
    storage: { session: {
      get: record("get", async key => ({ [key]: clone(storage[key]) })),
      set: record("set", async value => { Object.assign(storage, clone(value)); }),
      remove: record("remove", async key => { delete storage[key]; })
    } },
    permissions: {
      request: record("request", async () => true),
      contains: record("contains", async () => true),
      getAll: record("getAll", async () => ({ origins: clone(grants) })),
      onAdded: event("added"), onRemoved: event("removed")
    },
    declarativeNetRequest: {
      getSessionRules: record("getRules", async () => clone(rules)),
      updateSessionRules: record("rules", async change => {
        rules = rules.filter(rule => !(change.removeRuleIds || []).includes(rule.id));
        rules.push(...clone(change.addRules || []));
      })
    },
    tabs: {
      TAB_ID_NONE: -1,
      getZoom: record("zoom", async () => 1.25),
      query: record("query", async () => [{ id: 7 }, { id: -1 }, { id: 9 }])
    },
    windows: { get: record("window", async () => ({ left: -900, top: 20, width: 1200, height: 900, state: "normal", id: 3, ignored: true })) },
    scripting: {
      executeScript: (...args) => {
        // Functions cannot be structured-cloned; retain these call arguments.
        log.push({ name: "execute", args });
        return hooks.execute ? hooks.execute(...args) : Promise.resolve([{ frameId: 0, result: geometry() }]);
      },
      getRegisteredContentScripts: record("registered", async () => clone(registered)),
      registerContentScripts: record("register", async scripts => { registered.push(...clone(scripts)); }),
      updateContentScripts: record("update", async scripts => { registered = clone(scripts); }),
      unregisterContentScripts: record("unregister", async () => { registered = []; })
    },
    contextMenus: {
      onClicked: event("click"),
      removeAll: fn => { log.push({ name: "menusRemove" }); callback(null, fn); },
      create: (value, fn) => { log.push({ name: "menu", args: [clone(value)] }); callback(null, fn); }
    }
  };
  const fetchImage = (...args) => {
    log.push({ name: "fetch", args: clone(args) }); fetches.push(clone(args));
    return hooks.fetch ? hooks.fetch(...args) : Promise.resolve(response());
  };
  const worker = createWorker(api, fetchImage, options.worker);
  return {
    api, worker, hooks, log, titles, badges, native, fetches, events, storage,
    names: () => log.map(item => item.name),
    grants: value => { grants = value; },
    rules: () => clone(rules), registered: () => clone(registered),
    click: (value = info, browserTab = tab) => worker.click(clone(value), clone(browserTab))
  };
}

test("permissions are requested synchronously before pending recovery/storage; rapid clicks share one owner", async () => {
  const gate = deferred();
  const h = harness();
  // Startup has invoked get, but its continuation has not yet inspected rules.
  h.hooks.getRules = () => gate.promise;
  const first = h.click();
  assert.equal(h.names().at(-1), "request");
  assert.equal(h.names().includes("set"), false);
  await h.click();
  assert.equal(h.names().filter(name => name === "request").length, 1);
  assert.equal(h.fetches.length, 0);
  gate.resolve([]);
  await first;
  assert.equal(h.native.length, 1);
  assert.ok(h.names().indexOf("request") < h.names().indexOf("set"));
  assert.deepEqual(h.storage, {});
  await h.click();
  assert.equal(h.native.length, 2);
});

test("unrelated context menu events do not request permissions or create a marker", async () => {
  const h = harness(); await h.worker.ready;
  await h.click({ ...info, menuItemId: "other" });
  assert.equal(h.names().includes("request"), false);
  assert.equal(h.names().includes("set"), false);
});

test("restart consumes only its own marker/rule and reports interruption once without replay", async () => {
  const storage = { [PENDING_KEY]: { v: 1, image_url: info.srcUrl }, unrelated: "keep" };
  const h = harness({ storage, rules: [{ id: RULE_ID }, { id: 44 }] });
  await h.worker.ready;
  assert.deepEqual(h.rules(), [{ id: 44 }]);
  assert.deepEqual(storage, { unrelated: "keep" });
  assert.deepEqual(h.titles, ["interrupted"]);
  assert.equal(h.fetches.length + h.native.length, 0);
  const next = harness({ storage, rules: h.rules() }); await next.worker.ready;
  assert.deepEqual(next.titles, []);
  assert.equal(next.fetches.length + next.native.length, 0);
});

for (const residual of ["marker", "rule"]) {
  test(`restart reports an isolated residual ${residual}`, async () => {
    const h = harness({ storage: residual === "marker" ? { [PENDING_KEY]: {} } : {}, rules: residual === "rule" ? [{ id: RULE_ID }] : [] });
    await h.worker.ready;
    assert.deepEqual(h.titles, ["interrupted"]);
    assert.deepEqual(h.rules(), []);
    assert.deepEqual(h.storage, {});
  });
}

test("permission denial stops; an existing image grant permits fallback when the page grant is absent", async () => {
  const denied = harness(); await denied.worker.ready;
  denied.hooks.request = async () => false;
  denied.hooks.contains = async () => false;
  await denied.click();
  assert.equal(denied.titles.at(-1), "permission_denied");
  assert.equal(denied.fetches.length + denied.native.length, 0);
  const partial = harness(); await partial.worker.ready;
  partial.hooks.request = async () => false;
  partial.hooks.contains = async value => value.origins[0] === "https://images.example/*";
  await partial.click();
  assert.equal(partial.native[0].view.context_event, null);
  assert.equal(partial.titles.at(-1), "success");
  assert.deepEqual(partial.log.find(item => item.name === "request").args[0], {
    origins: ["https://images.example/*", "https://page.example/*"]
  });
});

test("same-origin requests deduplicate the origin grant", async () => {
  const h = harness(); await h.worker.ready;
  await h.click({ ...info, srcUrl: "https://page.example/image.png" });
  assert.deepEqual(h.log.find(item => item.name === "request").args[0], { origins: ["https://page.example/*"] });
});

test("pending permission and native replies retain exclusive ownership through final completion", async () => {
  const h = harness(); await h.worker.ready;
  const permission = deferred(), native = deferred();
  h.hooks.request = () => permission.promise;
  h.hooks.native = async (_host, _message, fn, callback) => {
    await native.promise;
    callback(null, fn, { ok: true, path: "C:\\saved.png" });
  };
  const first = h.click(); await turn();
  assert.equal(h.storage[PENDING_KEY].image_url, info.srcUrl);
  assert.deepEqual(Object.keys(h.storage[PENDING_KEY]).sort(), ["v", "tab_id", "image_url", "page_url"].sort());
  await h.click(); assert.equal(h.fetches.length, 0);
  permission.resolve(true); await turn();
  assert.equal(h.native.length, 1);
  await h.click();
  assert.equal(h.fetches.length, 1);
  assert.equal(h.names().filter(name => name === "request").length, 1);
  native.resolve(); await first;
  delete h.hooks.request; delete h.hooks.native;
  await h.click(); assert.equal(h.native.length, 2);
});

test("permission rejection is handled immediately while startup recovery remains pending", async () => {
  const h = harness(), gate = deferred();
  h.hooks.getRules = () => gate.promise;
  h.hooks.request = async () => { throw new Error("denied API"); };
  const task = h.click();
  await turn();
  assert.equal(h.fetches.length, 0);
  gate.resolve([]); await task;
  assert.equal(h.titles.at(-1), "permission_failed");
});

test("image links use exactly the shared Rust extension set, preserving selected URL identity", async () => {
  const source = fs.readFileSync(path.join(root, "crates/capture-file/src/image_formats.rs"), "utf8");
  const rust = [...source.matchAll(/extensions:\s*&\[([^\]]+)\]/g)].flatMap(match => [...match[1].matchAll(/"([a-z]+)"/g)].map(item => item[1]));
  assert.ok(rust.length > 0);
  assert.deepEqual([...IMAGE_EXTENSIONS].sort(), rust.sort());
  for (const extension of rust) {
    const linked = `https://user:secret@linked.example/full.${extension.toUpperCase()}?size=1#fragment`;
    const selected = selectTarget({ ...info, linkUrl: linked }, tab);
    assert.equal(selected.image.href, `https://linked.example/full.${extension.toUpperCase()}?size=1`);
  }
  for (const linked of ["https://linked.example/page.html", "https://linked.example/photo.png/more", "data:image/png,x", "file:///photo.png", "bad url"]) {
    assert.equal(selectTarget({ ...info, linkUrl: linked }, tab).image.href, info.srcUrl);
  }
  const h = harness(); await h.worker.ready;
  h.hooks.fetch = async () => { throw new Error("network"); };
  await h.click({ ...info, linkUrl: "https://linked.example/full.jpg" });
  assert.equal(h.fetches.length, 1);
  assert.equal(h.fetches[0][0], "https://linked.example/full.jpg");
  assert.equal(h.native.length, 0);
});

test("exact DNR rule escapes regex syntax and matches only the initial URL and extension request", () => {
  const url = "https://img.example/a+b.(png)?size=1&other=[x]";
  const rule = exactRule(url, "https://page.example", -1, storeIds[0]);
  const regex = new RegExp(rule.condition.regexFilter);
  assert.equal(regex.test(url), true);
  for (const other of [`${url}&extra=1`, url.replace("a+b", "aaab"), url.replace("img.example", "imgXexample"), `x${url}`]) assert.equal(regex.test(other), false, other);
  assert.deepEqual(rule, {
    id: RULE_ID, priority: 1,
    action: { type: "modifyHeaders", requestHeaders: [{ header: "Referer", operation: "set", value: "https://page.example/" }] },
    condition: { regexFilter: rule.condition.regexFilter, isUrlFilterCaseSensitive: true, tabIds: [-1], initiatorDomains: [storeIds[0]], resourceTypes: ["xmlhttprequest"], requestMethods: ["get"] }
  });
});

// Only the condition subset used by this product. DNR domains match the
// initiator's hostname (including subdomains), not the requested URL's host.
function matchesImageRule(rule, request) {
  const condition = rule.condition;
  if (!condition.tabIds.includes(request.tabId) ||
      !condition.requestMethods.includes(request.method.toLowerCase()) ||
      !condition.resourceTypes.includes(request.type) ||
      !new RegExp(condition.regexFilter, condition.isUrlFilterCaseSensitive ? "" : "i").test(request.url)) return false;
  if (condition.initiatorDomains) {
    let host;
    try { host = new URL(request.initiator).hostname; } catch { return false; }
    return condition.initiatorDomains.some(domain => host === domain || host.endsWith(`.${domain}`));
  }
  return true;
}

test("DNR excludes Web service workers at the same URL GET and TAB_ID_NONE", () => {
  for (const extensionId of storeIds) {
    const rule = exactRule(info.srcUrl, "https://page.example", -1, extensionId);
    const own = { url: info.srcUrl, method: "GET", tabId: -1, type: "xmlhttprequest", initiator: `chrome-extension://${extensionId}` };
    assert.equal(matchesImageRule(rule, own), true);
    const unscoped = clone(rule);
    delete unscoped.condition.initiatorDomains;
    for (const initiator of ["https://page.example", "https://images.example"]) {
      const web = { ...own, initiator };
      assert.equal(matchesImageRule(unscoped, web), true, "the previous condition admits this Web request");
      assert.equal(matchesImageRule(rule, web), false);
    }
    const otherId = storeIds.find(id => id !== extensionId);
    for (const initiator of [`chrome-extension://${otherId}`, "null", undefined]) {
      assert.equal(matchesImageRule(rule, { ...own, initiator }), false);
    }
    for (const changed of [{ tabId: 7 }, { method: "POST" }, { url: `${info.srcUrl}?other=1` }, { type: "image" }]) {
      assert.equal(matchesImageRule(rule, { ...own, ...changed }), false);
    }
  }
});

test("admission scopes the live fetch rule to the running Chrome or Edge extension ID", async () => {
  for (const extensionId of storeIds) {
    const h = harness();
    h.api.runtime.id = extensionId;
    await h.worker.ready;
    h.hooks.fetch = async url => {
      const rule = h.rules().find(rule => rule.id === RULE_ID);
      assert.deepEqual(rule.condition.initiatorDomains, [extensionId]);
      const request = { url, method: "GET", tabId: -1, type: "xmlhttprequest", initiator: `chrome-extension://${extensionId}` };
      assert.equal(matchesImageRule(rule, request), true);
      assert.equal(matchesImageRule(rule, { ...request, initiator: "https://page.example" }), false);
      return response();
    };
    await h.click();
    assert.equal(h.native.length, 1);
    assert.equal(h.titles.at(-1), "success");
    assert.deepEqual(h.rules(), []);
  }
});

test("one credentialed fetch is bracketed by DNR completion; redirect URL and only host schema are sent", async () => {
  const h = harness(); await h.worker.ready;
  const add = deferred(), remove = deferred();
  h.hooks.rules = change => change.addRules ? add.promise : remove.promise;
  h.hooks.fetch = async () => response({ url: "https://cdn.example/final.png#part" });
  const task = h.click(); await turn();
  assert.equal(h.fetches.length, 0);
  add.resolve(); await turn();
  assert.equal(h.native.length, 0);
  assert.deepEqual(h.fetches, [[info.srcUrl, { credentials: "include", cache: "no-store" }]]);
  remove.resolve(); await task;
  assert.equal(h.native.length, 1);
  const message = h.native[0];
  assert.equal(message.final_url, "https://cdn.example/final.png");
  assert.equal(message.image_url, info.srcUrl);
  assert.equal(message.page_url, info.pageUrl);
  assert.equal(message.content_type, "image/png");
  assert.equal(message.data_b64, "iVBO");
  assert.equal(message.v, 1); assert.equal(message.kind, "open_bytes");
  assert.deepEqual(Object.keys(message).sort(), ["v", "kind", "data_b64", "content_type", "page_url", "image_url", "final_url", "view", "zoom", "window"].sort());
  assert.deepEqual(message.window, { left: -900, top: 20, width: 1200, height: 900, state: "normal" });
  assert.equal(h.log.find(item => item.name === "native").args[0], "to.mikage.mimageviewer");
  assert.ok(h.names().lastIndexOf("rules") < h.names().indexOf("native"));
});

for (const failure of ["http", "network", "body", "invalid_final", "too_large"]) {
  test(`fetch ${failure} removes the temporary rule and never invokes native messaging`, async () => {
    const h = harness(); await h.worker.ready;
    h.hooks.fetch = async () => {
      if (failure === "network") throw new Error("fetch failed");
      return response({ ok: failure !== "http", error: failure === "body" ? new Error("stream") : undefined,
        url: failure === "invalid_final" ? "file:///tmp/a.png" : info.srcUrl,
        length: failure === "too_large" ? String(MAX_FRAME_BYTES) : null });
    };
    await h.click();
    assert.equal(h.fetches.length, 1); assert.equal(h.native.length, 0);
    const rules = h.log.filter(item => item.name === "rules");
    assert.equal(rules.length, 2);
    assert.deepEqual(rules[1].args[0], { removeRuleIds: [RULE_ID] });
    assert.deepEqual(h.storage, {});
  });
}

test("Content-Length rejects before reads and streaming overflow cancels immediately", async () => {
  const large = response({ length: " 7 " });
  await assert.rejects(readImage(large, 6), error => error.status === "too_large");
  assert.deepEqual(large.stats, { reads: 0, cancelled: 1, released: 0 });
  const streamed = response({ chunks: [Uint8Array.of(1, 2, 3), Uint8Array.of(4), Uint8Array.of(5)] });
  await assert.rejects(readImage(streamed, 3), error => error.status === "too_large");
  assert.deepEqual(streamed.stats, { reads: 2, cancelled: 1, released: 1 });
  const exact = response({ chunks: [Uint8Array.of(1), Uint8Array.of(2, 3)], length: "3" });
  assert.equal(await readImage(exact, 3), "AQID");
  assert.deepEqual(exact.stats, { reads: 3, cancelled: 0, released: 1 });
  const missing = response(); missing.body = null;
  await assert.rejects(readImage(missing, 3), error => error.status === "fetch_failed");
});

test("base64 raw budget includes complete UTF-8 JSON overhead at exact boundaries", async () => {
  const message = { data_b64: "", page_url: "https://page.example/日本語", content_type: "image/写真" };
  const overhead = Buffer.byteLength(JSON.stringify(message), "utf8");
  assert.equal(frameBytes(message), overhead);
  assert.ok(overhead > JSON.stringify(message).length);
  for (let available = 4; available <= 16; available++) {
    const budget = rawBudget(message, overhead + available);
    assert.equal(budget, Math.floor(available / 4) * 3);
    const encoded = Buffer.alloc(budget).toString("base64");
    assert.ok(frameBytes({ ...message, data_b64: encoded }) <= overhead + available);
    assert.ok(frameBytes({ ...message, data_b64: Buffer.alloc(budget + 1).toString("base64") }) > overhead + available);
  }
  assert.throws(() => rawBudget(message, overhead + 3), error => error.status === "too_large");
  const baseline = harness(); await baseline.worker.ready; await baseline.click();
  const size = frameBytes(baseline.native[0]);
  const exact = harness({ worker: { maxFrameBytes: size } }); await exact.worker.ready; await exact.click();
  assert.equal(exact.native.length, 1);
  const over = harness({ worker: { maxFrameBytes: size - 1 } }); await over.worker.ready; await over.click();
  assert.equal(over.native.length, 0); assert.equal(over.titles.at(-1), "too_large");
});

test("recorded, absent, moved, resized and DPR-changed geometry uses the unchanged host wire schema", async () => {
  for (const change of [() => {}, view => { view.context_event = null; }, view => { view.screenX += 50; }, view => { view.innerWidth += 20; }, view => { view.devicePixelRatio = 2; }]) {
    const view = geometry(); change(view);
    const h = harness(); await h.worker.ready;
    h.hooks.execute = async () => [{ frameId: 12, result: { wrong: true } }, { frameId: 0, result: clone(view) }];
    await h.click();
    assert.deepEqual(h.native[0].view, view);
    assert.equal(h.native[0].zoom, 1.25);
    const injection = h.log.find(item => item.name === "execute").args[0];
    assert.deepEqual(injection.target, { tabId: 7, frameIds: [0] });
    assert.equal(injection.world, "ISOLATED"); assert.equal(typeof injection.func, "function");
    assert.equal("placement" in h.native[0], false);
  }
});

test("native error mapping exactly covers serialized Rust ErrorCode and both locales", () => {
  const source = fs.readFileSync(path.join(root, "crates/browser-host/src/lib.rs"), "utf8");
  const body = source.match(/pub enum ErrorCode\s*\{([^}]+)\}/)[1];
  const errors = body.split(",").map(item => item.trim()).filter(Boolean)
    .map(item => item.replace(/[A-Z]/g, (letter, index) => `${index ? "_" : ""}${letter.toLowerCase()}`));
  assert.deepEqual(Object.keys(HOST_ERRORS).sort(), errors.sort());
  for (const locale of ["ja", "en"]) {
    const messages = JSON.parse(fs.readFileSync(path.join(__dirname, `../_locales/${locale}/messages.json`), "utf8"));
    for (const key of [...Object.values(HOST_ERRORS), "busy", "success", "permission_denied", "permission_failed", "position_failed", "fetch_failed", "too_large", "rule_failed", "state_failed", "interrupted", "unregistered", "native_failed", "context_failed", "invalid_url", "badgeBusy", "badgeSuccess", "badgeError", "extName", "menuOpen"]) {
      assert.equal(typeof messages[key]?.message, "string", `${locale}: ${key}`);
      assert.ok(messages[key].message.length > 0, `${locale}: ${key}`);
    }
  }
});

for (const [code, status] of Object.entries(HOST_ERRORS)) {
  test(`native host ${code} response reports localized status and releases ownership`, async () => {
    const h = harness(); await h.worker.ready;
    h.hooks.native = (_host, _message, fn, callback) => callback(null, fn, { ok: false, error: code, path: "C:\\saved.png" });
    await h.click(); assert.equal(h.titles.at(-1), status);
    delete h.hooks.native; await h.click();
    assert.equal(h.native.length, 2); assert.equal(h.titles.at(-1), "success");
  });
}

for (const [label, reply, error, expected] of [
  ["absent host", undefined, "Specified native messaging host not found.", "unregistered"],
  ["transport error", undefined, "Native host has exited.", "native_failed"],
  ["lastError wins over reply", { ok: true, path: "C:\\saved.png" }, "connection closed", "native_failed"],
  ["missing response", undefined, null, "native_failed"],
  ["null response", null, null, "native_failed"],
  ["unknown error", { ok: false, error: "new_error" }, null, "native_failed"],
  ["missing path", { ok: true }, null, "native_failed"],
  ["empty path", { ok: true, path: "" }, null, "native_failed"],
  ["non-string path", { ok: true, path: 7 }, null, "native_failed"],
  ["non-boolean result", { ok: "true", path: "C:\\saved.png" }, null, "native_failed"]
]) {
  test(`native ${label} respects callback-scoped lastError`, async () => {
    const h = harness(); await h.worker.ready;
    h.hooks.native = (_host, _message, fn, callback) => callback(error, fn, reply);
    await h.click(); assert.equal(h.api.runtime.lastError, undefined);
    assert.equal(h.titles.at(-1), expected);
    delete h.hooks.native; await h.click(); assert.equal(h.titles.at(-1), "success");
  });
}

for (const [name, setup, expected] of [
  ["invalid URL", h => {}, "invalid_url"],
  ["permission throw", h => { h.hooks.request = () => { throw new Error("request"); }; }, "permission_failed"],
  ["permission rejection", h => { h.hooks.request = async () => { throw new Error("request"); }; }, "permission_failed"],
  ["permission denied", h => { h.hooks.contains = async () => false; }, "permission_denied"],
  ["storage write", h => { h.hooks.set = async () => { throw new Error("storage"); }; }, "state_failed"],
  ["geometry rejection", h => { h.hooks.execute = async () => { throw new Error("closed"); }; }, "position_failed"],
  ["invalid geometry", h => { h.hooks.execute = async () => [{ frameId: 0, result: { ...geometry(), devicePixelRatio: 0 } }]; }, "position_failed"],
  ["zoom", h => { h.hooks.zoom = async () => { throw new Error("closed"); }; }, "position_failed"],
  ["window", h => { h.hooks.window = async () => { throw new Error("closed"); }; }, "position_failed"],
  ["rule add", h => { h.hooks.rules = async () => { throw new Error("dnr"); }; }, "rule_failed"],
  ["rule removal", h => { h.hooks.rules = async change => { if (!change.addRules) throw new Error("dnr"); }; }, "rule_failed"],
  ["HTTP failure", h => { h.hooks.fetch = async () => response({ ok: false }); }, "fetch_failed"],
  ["network failure", h => { h.hooks.fetch = async () => { throw new Error("network"); }; }, "fetch_failed"],
  ["image too large", h => { h.hooks.fetch = async () => response({ length: String(MAX_FRAME_BYTES) }); }, "too_large"],
  ["native synchronous throw", h => { h.hooks.native = () => { throw new Error("native"); }; }, "native_failed"],
  ["marker removal", h => { h.hooks.remove = async () => { throw new Error("storage"); }; }, "state_failed"]
]) {
  test(`${name} terminal releases the click latch`, async () => {
    const h = harness(); await h.worker.ready; setup(h);
    await h.click(name === "invalid URL" ? { ...info, srcUrl: "file:///a.png" } : info);
    assert.equal(h.titles.at(-1), expected);
    for (const key of Object.keys(h.hooks)) delete h.hooks[key];
    const requests = h.names().filter(value => value === "request").length;
    await h.click();
    assert.equal(h.names().filter(value => value === "request").length, requests + 1);
    assert.equal(h.titles.at(-1), "success");
  });
}

test("startup cleanup failure fails closed without replay and does not retain the synchronous latch", async () => {
  const h = harness(); h.hooks.getRules = async () => { throw new Error("dnr"); };
  assert.equal(await h.worker.ready, false);
  await h.click(); await h.click();
  assert.equal(h.names().filter(value => value === "request").length, 2);
  assert.equal(h.fetches.length + h.native.length, 0);
  assert.equal(h.titles.at(-1), "state_failed");
});

test("context script is idempotent, top-frame-only, and never reads DOM/element content", () => {
  const source = fs.readFileSync(path.join(__dirname, "../context.js"), "utf8");
  const listeners = [];
  const window = { ...geometry(), addEventListener: (...args) => listeners.push(args) };
  window.top = window;
  Object.defineProperty(window, "document", { get() { throw new Error("DOM must not be read"); } });
  const context = vm.createContext({ window });
  Object.defineProperty(context, "document", { get() { throw new Error("DOM must not be read"); } });
  vm.runInContext(source, context); vm.runInContext(source, context);
  assert.equal(listeners.length, 1);
  assert.equal(listeners[0][0], "contextmenu");
  assert.deepEqual(JSON.parse(JSON.stringify(listeners[0][2])), { capture: true, passive: true });
  assert.equal(vm.runInContext("mivReadView().context_event", context), null);
  const event = { screenX: 50, screenY: 100, clientX: 10, clientY: 20 };
  Object.defineProperty(event, "target", { get() { throw new Error("target must not be read"); } });
  listeners[0][1](event);
  const view = JSON.parse(vm.runInContext("JSON.stringify(mivReadView())", context));
  assert.deepEqual(view, { ...geometry(), context_event: { ...event, win_screenX: -900, win_screenY: 20, innerWidth: 900, innerHeight: 700, devicePixelRatio: 1.5 } });
  listeners[0][1]({ screenX: 60, screenY: 110, clientX: 20, clientY: 30 });
  assert.equal(vm.runInContext("mivReadView().context_event.screenX", context), 60);
  const childWindow = { ...window, top: {} };
  const child = vm.createContext({ window: childWindow });
  vm.runInContext(source, child);
  assert.equal(listeners.length, 1);
  assert.equal(vm.runInContext("globalThis.__mivBrowserContext", child), undefined);
});

test("install/startup/permission changes register and reinject idempotently with correct isolated scope", async () => {
  const h = harness(); await h.worker.ready;
  h.worker.attach(); h.worker.attach();
  for (const event of Object.values(h.events)) assert.equal(event.listeners.length, 1);
  h.events.installed.emit(); await turn(); await h.worker.refreshContent();
  assert.deepEqual(h.log.find(item => item.name === "menu").args[0], { id: MENU_ID, title: "menuOpen", contexts: ["image"] });
  assert.ok(h.names().indexOf("menusRemove") < h.names().indexOf("menu"));
  h.events.startup.emit(); h.events.added.emit(); await h.worker.refreshContent();
  assert.equal(h.log.filter(item => item.name === "register").length, 1);
  assert.equal(h.registered().length, 1);
  assert.deepEqual(h.registered()[0], { id: SCRIPT_ID, matches: ["https://page.example/*"], js: ["context.js"], allFrames: false, world: "ISOLATED", runAt: "document_start", persistAcrossSessions: true });
  const injections = h.log.filter(item => item.name === "execute");
  assert.ok(injections.length >= 6);
  for (const { args: [injection] } of injections) {
    assert.equal(injection.world, "ISOLATED"); assert.deepEqual(injection.files, ["context.js"]);
    assert.deepEqual(injection.target.frameIds, [0]); assert.ok([7, 9].includes(injection.target.tabId));
  }
  h.grants([]); h.events.removed.emit(); await h.worker.refreshContent();
  assert.deepEqual(h.registered(), []);
  assert.equal(h.log.filter(item => item.name === "unregister").length, 1);
});

test("closed existing tabs do not prevent other injections or persistent registration", async () => {
  const h = harness(); await h.worker.ready;
  h.hooks.execute = async value => { if (value.target.tabId === 7) throw new Error("tab closed"); };
  h.grants(["https://page.example/*", "file:///*"]);
  await h.worker.refreshContent();
  assert.deepEqual(h.registered()[0].matches, ["https://page.example/*"]);
  assert.deepEqual(h.log.filter(item => item.name === "execute").map(item => item.args[0].target.tabId), [7, 9]);
});
