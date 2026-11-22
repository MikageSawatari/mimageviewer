"use strict";

if (typeof importScripts === "function") importScripts("context.js");
const readView = typeof module !== "undefined" ? require("./context.js").mivReadView : mivReadView;
const HOST = "to.mikage.mimageviewer";
const MAX_FRAME_BYTES = 64 * 1024 * 1024;
const PENDING_KEY = "browserViewPendingV1";
const RULE_ID = 818;
const SCRIPT_ID = "miv-browser-context-v1";
const MENU_ID = "view-in-miv";
// Conformance is checked against crates/capture-file/src/image_formats.rs.
const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif", "avif", "heic", "heif", "jxl"];
const HOST_ERRORS = Object.freeze({
  invalid_origin: "host_invalid_origin",
  unsupported_version: "host_unsupported_version",
  invalid_request: "host_invalid_request",
  message_too_large: "host_message_too_large",
  invalid_image: "host_invalid_image",
  invalid_url: "host_invalid_url",
  invalid_placement: "host_invalid_placement",
  registration_missing: "host_registration_missing",
  launcher_missing: "host_launcher_missing",
  invalid_destination: "host_invalid_destination",
  save_failed: "host_save_failed",
  motw_failed: "host_motw_failed",
  launch_failed: "host_launch_failed"
});

class Failure extends Error {
  constructor(status) { super(status); this.status = status; }
}
function httpUrl(value) {
  let url;
  try { url = new URL(value); } catch { throw new Failure("invalid_url"); }
  if (url.protocol !== "http:" && url.protocol !== "https:") throw new Failure("invalid_url");
  url.username = "";
  url.password = "";
  url.hash = "";
  return url;
}
function selectTarget(info, tab) {
  let linked = null;
  try {
    const url = httpUrl(info.linkUrl);
    const extension = url.pathname.slice(url.pathname.lastIndexOf(".") + 1).toLowerCase();
    if (IMAGE_EXTENSIONS.includes(extension)) linked = url;
  } catch { /* A non-image link does not change the image menu's source. */ }
  const image = linked ?? httpUrl(info.srcUrl);
  const page = httpUrl(info.pageUrl || tab.url);
  if (!Number.isInteger(tab.id) || tab.id < 0 || !Number.isInteger(tab.windowId)) {
    throw new Failure("position_failed");
  }
  return { image, page, tab };
}
function exactRule(image, pageOrigin, tabNone) {
  return {
    id: RULE_ID, priority: 1,
    action: { type: "modifyHeaders", requestHeaders: [{ header: "Referer", operation: "set", value: `${pageOrigin}/` }] },
    condition: {
      regexFilter: `^${image.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`,
      isUrlFilterCaseSensitive: true, tabIds: [tabNone],
      resourceTypes: ["xmlhttprequest"], requestMethods: ["get"]
    }
  };
}
function frameBytes(message) { return new TextEncoder().encode(JSON.stringify(message)).byteLength; }
function rawBudget(message, maximum) {
  const available = maximum - frameBytes({ ...message, data_b64: "" });
  if (available < 4) throw new Failure("too_large");
  return Math.floor(available / 4) * 3;
}
function encodeBase64(chunks) {
  const strings = [];
  for (const chunk of chunks) {
    for (let at = 0; at < chunk.length; at += 0x8000) {
      strings.push(String.fromCharCode(...chunk.subarray(at, at + 0x8000)));
    }
  }
  return btoa(strings.join(""));
}
async function readImage(response, budget) {
  const length = response.headers.get("Content-Length");
  if (length !== null && /^\d+$/.test(length.trim()) && Number(length) > budget) {
    await response.body?.cancel();
    throw new Failure("too_large");
  }
  if (!response.ok || !response.body) {
    await response.body?.cancel();
    throw new Failure("fetch_failed");
  }
  const reader = response.body.getReader();
  let size = 0;
  const chunks = [];
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > budget) {
        await reader.cancel();
        throw new Failure("too_large");
      }
      chunks.push(value);
    }
  } finally { reader.releaseLock(); }
  return encodeBase64(chunks);
}
function validView(view, zoom, windowInfo) {
  const positive = [view?.innerWidth, view?.innerHeight, view?.outerWidth, view?.outerHeight,
    view?.devicePixelRatio, zoom, windowInfo.width, windowInfo.height];
  if (![view?.screenX, view?.screenY, windowInfo.left, windowInfo.top].every(Number.isFinite) ||
      !positive.every(value => Number.isFinite(value) && value > 0) ||
      !["normal", "maximized", "fullscreen", "minimized"].includes(windowInfo.state)) {
    throw new Failure("position_failed");
  }
}

function createWorker(api, fetchImage = globalThis.fetch, { maxFrameBytes = MAX_FRAME_BYTES } = {}) {
  let active = null;
  let attached = false;
  let contentUpdates = Promise.resolve();
  const text = key => api.i18n.getMessage(key);
  async function status(code) {
    try {
      await api.action.setBadgeText({ text: text(code === "busy" ? "badgeBusy" : code === "success" ? "badgeSuccess" : "badgeError") });
      await api.action.setTitle({ title: `${text("extName")}\n${text(code)}` });
    } catch { console.warn("mIV: status could not be displayed"); }
  }
  // Runs once per worker lifetime, before an admitted request writes its marker.
  const ready = (async () => {
    try {
      const previous = (await api.storage.session.get(PENDING_KEY))[PENDING_KEY];
      const rules = await api.declarativeNetRequest.getSessionRules();
      const residual = rules.some(rule => rule.id === RULE_ID);
      if (residual) await api.declarativeNetRequest.updateSessionRules({ removeRuleIds: [RULE_ID] });
      if (previous) await api.storage.session.remove(PENDING_KEY);
      if (previous || residual) await status("interrupted");
      return true;
    } catch {
      await status("state_failed");
      return false;
    }
  })();

  function nativeMessage(message) {
    return new Promise((resolve, reject) => {
      try {
        api.runtime.sendNativeMessage(HOST, message, reply => {
          const error = api.runtime.lastError;
          if (error) {
            const absent = /native messaging host not found|native host.*not found/i.test(error.message || "");
            reject(new Failure(absent ? "unregistered" : "native_failed"));
          } else if (reply?.ok === false && Object.hasOwn(HOST_ERRORS, reply.error)) {
            reject(new Failure(HOST_ERRORS[reply.error]));
          } else if (reply?.ok === true && typeof reply.path === "string" && reply.path.length > 0) {
            resolve(reply);
          } else reject(new Failure("native_failed"));
        });
      } catch { reject(new Failure("native_failed")); }
    });
  }
  async function open(target, permission) {
    const permissionResult = await permission;
    if (permissionResult.error) throw new Failure("permission_failed");
    const imageOrigin = `${target.image.origin}/*`;
    const pageOrigin = `${target.page.origin}/*`;
    if (!await api.permissions.contains({ origins: [imageOrigin] })) throw new Failure("permission_denied");
    const pageAllowed = await api.permissions.contains({ origins: [pageOrigin] });
    let view, zoom, windowInfo;
    try {
      const measured = await api.scripting.executeScript({ target: { tabId: target.tab.id, frameIds: [0] }, world: "ISOLATED", func: readView });
      view = measured.find(result => result.frameId === 0)?.result;
      zoom = await api.tabs.getZoom(target.tab.id);
      windowInfo = await api.windows.get(target.tab.windowId);
      validView(view, zoom, windowInfo);
    } catch { throw new Failure("position_failed"); }
    if (!pageAllowed) view.context_event = null;
    // The host checks geometry identity and computes both recorded and fallback positions.
    const message = {
      v: 1, kind: "open_bytes", data_b64: "", content_type: "",
      page_url: target.page.href, image_url: target.image.href, final_url: "",
      view, zoom,
      window: { left: windowInfo.left, top: windowInfo.top, width: windowInfo.width, height: windowInfo.height, state: windowInfo.state }
    };
    try {
      await api.declarativeNetRequest.updateSessionRules({
        removeRuleIds: [RULE_ID], addRules: [exactRule(target.image.href, target.page.origin, api.tabs.TAB_ID_NONE)]
      });
    } catch { throw new Failure("rule_failed"); }
    try {
      const response = await fetchImage(target.image.href, { credentials: "include", cache: "no-store" });
      message.final_url = httpUrl(response.url).href;
      message.content_type = response.headers.get("Content-Type") || "";
      message.data_b64 = await readImage(response, rawBudget(message, maxFrameBytes));
    } catch (error) {
      throw error instanceof Failure ? error : new Failure("fetch_failed");
    } finally {
      try { await api.declarativeNetRequest.updateSessionRules({ removeRuleIds: [RULE_ID] }); }
      catch { throw new Failure("rule_failed"); }
    }
    if (frameBytes(message) > maxFrameBytes) throw new Failure("too_large");
    await nativeMessage(message);
  }

  // Non-async entry: the latch and permission call run within the click gesture.
  function click(info, tab) {
    if (info.menuItemId !== MENU_ID) return Promise.resolve();
    if (active) return status("busy");
    const owner = {};
    active = owner;
    let target, permission;
    try {
      target = selectTarget(info, tab);
      const origins = [...new Set([`${target.image.origin}/*`, `${target.page.origin}/*`])];
      // Attach both handlers now, including while startup recovery is still pending.
      permission = Promise.resolve(api.permissions.request({ origins }))
        .then(value => ({ value }), () => ({ error: true }));
    } catch (error) {
      active = null;
      return status(error instanceof Failure ? error.status : "permission_failed");
    }
    return (async () => {
      let terminal = "success";
      try {
        if (!await ready) throw new Failure("state_failed");
        await api.storage.session.set({ [PENDING_KEY]: {
          v: 1, tab_id: tab.id, image_url: target.image.href, page_url: target.page.href
        } });
        await status("busy");
        await open(target, permission);
      } catch (error) {
        terminal = error instanceof Failure ? error.status : "state_failed";
      } finally {
        try { await api.storage.session.remove(PENDING_KEY); }
        catch { terminal = "state_failed"; }
        await status(terminal);
        if (active === owner) active = null;
      }
    })();
  }

  async function refresh() {
    // A single registration ID avoids duplicate scripts across startup/permission events.
    const grants = await api.permissions.getAll();
    const matches = (grants.origins || []).filter(origin => /^https?:\/\//.test(origin));
    const registered = await api.scripting.getRegisteredContentScripts({ ids: [SCRIPT_ID] });
    if (matches.length) {
      const script = { id: SCRIPT_ID, matches, js: ["context.js"], allFrames: false,
        world: "ISOLATED", runAt: "document_start", persistAcrossSessions: true };
      if (registered.length) await api.scripting.updateContentScripts([script]);
      else await api.scripting.registerContentScripts([script]);
      const tabs = await api.tabs.query({ url: matches });
      for (const tab of tabs) {
        if (!Number.isInteger(tab.id) || tab.id < 0) continue;
        // Navigation/closed tabs can race this best-effort existing-page injection.
        try {
          await api.scripting.executeScript({ target: { tabId: tab.id, frameIds: [0] }, world: "ISOLATED", files: ["context.js"] });
        } catch { /* Future documents are covered by the registration. */ }
      }
    } else if (registered.length) await api.scripting.unregisterContentScripts({ ids: [SCRIPT_ID] });
  }
  function refreshContent() {
    contentUpdates = contentUpdates.then(refresh, refresh).catch(() => status("context_failed"));
    return contentUpdates;
  }
  async function installed() {
    await ready;
    await new Promise((resolve, reject) => api.contextMenus.removeAll(() => {
      if (api.runtime.lastError) reject(new Failure("context_failed")); else resolve();
    }));
    await new Promise((resolve, reject) => api.contextMenus.create({ id: MENU_ID, title: text("menuOpen"), contexts: ["image"] }, () => {
      if (api.runtime.lastError) reject(new Failure("context_failed")); else resolve();
    }));
    await refreshContent();
  }
  function attach() {
    if (attached) return;
    attached = true;
    api.contextMenus.onClicked.addListener(click);
    api.runtime.onInstalled.addListener(() => { installed().catch(() => status("context_failed")); });
    api.runtime.onStartup.addListener(() => { refreshContent(); });
    api.permissions.onAdded.addListener(() => { refreshContent(); });
    api.permissions.onRemoved.addListener(() => { refreshContent(); });
  }
  return { ready, click, attach, installed, refreshContent };
}

if (typeof module !== "undefined") module.exports = {
  createWorker, selectTarget, exactRule, frameBytes, rawBudget, readImage,
  IMAGE_EXTENSIONS, HOST_ERRORS, MAX_FRAME_BYTES, PENDING_KEY, RULE_ID, MENU_ID, SCRIPT_ID
};
if (typeof chrome !== "undefined") createWorker(chrome).attach();
