import {
  CommandName,
  FitMode,
  ReadingDirection,
  SpreadMode,
  ViewerGesture,
  ViewerPanelAction,
  adjustmentResetVisible,
  command,
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
  isRtlReadingDirection,
  nextFitMode,
  nextSpreadMode,
  pagePrefetchPlan,
  planSpreadIntent,
  reduceViewerTransform,
  readingProgressBatchTransition,
  rangeValueFromNormalized,
  rangeValueToNormalized,
  relativeRangeDragValue,
  appUpdateNotice,
  resolveGridReturnViewport,
  sessionOwnerBadge,
  snappedGridOffset,
  thumbnailBindingMatches,
  thumbnailRequestConcurrency,
  thumbnailRequestStartCount,
  thumbnailRetryDecision,
  togglePageOriginalFitMode,
  shouldShowGridCursor,
  shouldShowLoadingIndicator,
  shouldShowKeyboardShortcuts,
  viewerGestureDecision,
  viewerPanelGestureAction,
  viewerPanelTransition,
  viewerResizePlan,
  viewerVerticalScrollDecision,
  viewerTapCommand,
  viewerTapSequenceTransition,
  viewerImageLayout,
  viewerBoundaryMessage,
  viewerSeekGroupIndex,
  viewerSeekState,
  viewerSpreadLayout,
  viewerWheelCommand,
} from "./command-core.mjs";
import {
  loadLocalSettings,
  saveLocalSettings,
} from "./local-settings.mjs";
import { VideoStreamViewer } from "./video-stream.mjs";

const app = document.querySelector("#app");
const hudElement = document.querySelector("#telemetry-hud");
const sessionOwnerBadgeElement = document.querySelector(
  "#remote-session-owner-badge"
);
const TELEMETRY_ENABLED = true;
const SERVER_HEAVY_IPC_LIMIT = 4;
const THUMBNAIL_MAX_CONCURRENCY = thumbnailRequestConcurrency(
  SERVER_HEAVY_IPC_LIMIT,
  1
);
const PAGE_LOADING_INDICATOR_DELAY_MS = 225;
const PAGE_BOUNDARY_MESSAGE_DURATION_MS = 2400;
const VIEWER_PANEL_ANIMATION_MS = 180;
// 要求幅是正後の実測約 457 KiB/ページなら、進行方向 8 ページで約 3.6 MiB。
// 逆方向 1 ページと表示中を含めても 12 件なら約 5.4 MiB で、32 MiB 上限に十分余裕がある。
// prefetch は直列かつ foreground が active request を abort するため表示要求を待たせない。
const PAGE_PREFETCH_AHEAD = 8;
const PAGE_PREFETCH_BEHIND = 1;
const PAGE_RESOURCE_CACHE_LIMIT = 12;
const PAGE_RESOURCE_CACHE_MAX_BYTES = 32 * 1024 * 1024;
const SESSION_PING_INTERVAL_MS = 30_000;
const READING_PROGRESS_INTERVAL_MS = 30_000;
const AI_FOREGROUND_POLL_MS = 500;
const AI_RETRY_DELAYS_MS = Object.freeze([1000, 2000, 5000]);
const AI_TERMINAL_STATES = new Set([
  "ready",
  "superseded",
  "cancelled_by_user",
  "discarded_by_host",
  "background_expired",
  "failed",
]);
const RUNTIME_TEST_MODE = globalThis.__MIV_RUNTIME_TEST_MODE__ === true;
const REMOTE_CLIENT_ID = loadRemoteClientId();
const LOCAL_SETTINGS_LOAD = loadLocalSettings();

class AuthenticationRequiredError extends Error {}

class RequestLimiter {
  constructor(limit) {
    this.limit = Math.max(1, Number(limit) || 1);
    this.active = 0;
    this.queue = [];
  }

  run(task, signal) {
    return new Promise((resolve, reject) => {
      const entry = { task, signal, resolve, reject, abort: null };
      entry.abort = () => {
        const index = this.queue.indexOf(entry);
        if (index >= 0) this.queue.splice(index, 1);
        const error = new Error("Aborted");
        error.name = "AbortError";
        reject(error);
      };
      if (signal?.aborted) {
        entry.abort();
        return;
      }
      signal?.addEventListener("abort", entry.abort, { once: true });
      this.queue.push(entry);
      this.drain();
    });
  }

  drain() {
    while (thumbnailRequestStartCount(this.active, this.queue.length, this.limit) > 0) {
      const entry = this.queue.shift();
      if (entry.signal?.aborted) {
        entry.abort();
        continue;
      }
      entry.signal?.removeEventListener("abort", entry.abort);
      this.active += 1;
      Promise.resolve()
        .then(entry.task)
        .then(entry.resolve, entry.reject)
        .finally(() => {
          this.active -= 1;
          this.drain();
        });
    }
  }
}

/// 進行中を中断せず、待機中は最後の 1 件だけを残す直列キュー。
/// 補正プレビューが UI 入力の回数だけ IPC を詰まらせないために使う。
export class LatestOnlyTaskQueue {
  constructor(run, onError = () => {}) {
    this.run = run;
    this.onError = onError;
    this.running = false;
    this.latest = null;
  }

  enqueue(value) {
    this.latest = value;
    if (!this.running) this.pump();
  }

  clear() {
    this.latest = null;
  }

  async pump() {
    if (this.running) return;
    this.running = true;
    try {
      while (this.latest !== null) {
        const value = this.latest;
        this.latest = null;
        try {
          await this.run(value);
        } catch (error) {
          if (error?.name !== "AbortError") this.onError(error);
        }
      }
    } finally {
      this.running = false;
      if (this.latest !== null) this.pump();
    }
  }
}

class PageResourceCache {
  constructor(
    limit = PAGE_RESOURCE_CACHE_LIMIT,
    byteLimit = PAGE_RESOURCE_CACHE_MAX_BYTES
  ) {
    this.limit = Math.max(1, Number(limit) || 1);
    this.byteLimit = Math.max(1, Number(byteLimit) || 1);
    this.ready = new Map();
    this.pending = [];
    this.active = null;
  }

  async loadForeground(request, signal) {
    const cached = this.ready.get(request.cacheKey);
    if (cached) {
      this.ready.delete(request.cacheKey);
      this.ready.set(request.cacheKey, cached);
      return { ...cached, prefetchStatus: "hit" };
    }
    if (this.active?.key === request.cacheKey) {
      const resource = await awaitWithAbort(this.active.promise, signal);
      return { ...resource, prefetchStatus: "in_flight" };
    }
    if (this.active) this.active.controller.abort();
    this.pending = this.pending.filter((item) => item.cacheKey !== request.cacheKey);
    const resource = await fetchPageResource(request, signal, false);
    this.remember(request.cacheKey, resource);
    return { ...resource, prefetchStatus: "miss" };
  }

  schedule(requests) {
    const unique = [];
    const seen = new Set();
    for (const request of requests) {
      if (!request?.cacheKey || seen.has(request.cacheKey) || this.ready.has(request.cacheKey)) {
        continue;
      }
      seen.add(request.cacheKey);
      unique.push(request);
    }
    this.pending = unique;
    if (this.active && !seen.has(this.active.key)) this.active.controller.abort();
    this.pump();
  }

  pump() {
    if (this.active) return;
    const request = this.pending.shift();
    if (!request) return;
    if (this.ready.has(request.cacheKey)) {
      this.pump();
      return;
    }
    const controller = new AbortController();
    const active = {
      key: request.cacheKey,
      controller,
      promise: null,
    };
    active.promise = fetchPageResource(request, controller.signal, true)
      .then((resource) => {
        if (!controller.signal.aborted) {
          this.remember(request.cacheKey, resource);
          rememberMediaImageInfo(request, resource.info);
          enqueueTelemetry({
            type: "page_prefetch",
            status: "ready",
            fetch_ms: roundMs(resource.fetchMs),
            bytes: resource.blob.size,
            requested_width: request.width,
          });
        }
        return resource;
      })
      .catch((error) => {
        if (error?.status === 503) this.pending = [];
        if (error?.name !== "AbortError") {
          enqueueTelemetry({
            type: "page_prefetch",
            status: "failed",
            message: limitText(error instanceof Error ? error.message : error, 240),
          });
        }
        throw error;
      })
      .finally(() => {
        if (this.active === active) {
          this.active = null;
          this.pump();
        }
      });
    // A rejected background promise must be observed even when no foreground load joins it.
    active.promise.catch(() => {});
    this.active = active;
  }

  remember(key, resource) {
    this.ready.delete(key);
    this.ready.set(key, resource);
    while (
      this.ready.size > this.limit ||
      [...this.ready.values()].reduce((sum, value) => sum + value.blob.size, 0) >
        this.byteLimit
    ) {
      this.ready.delete(this.ready.keys().next().value);
    }
  }

  clear() {
    this.pending = [];
    this.active?.controller.abort();
    this.active = null;
    this.ready.clear();
  }
}

function awaitWithAbort(promise, signal) {
  if (!signal) return promise;
  if (signal.aborted) return Promise.reject(abortError());
  return new Promise((resolve, reject) => {
    const abort = () => reject(abortError());
    signal.addEventListener("abort", abort, { once: true });
    promise.then(
      (value) => {
        signal.removeEventListener("abort", abort);
        resolve(value);
      },
      (error) => {
        signal.removeEventListener("abort", abort);
        reject(error);
      }
    );
  });
}

function abortError() {
  const error = new Error("Aborted");
  error.name = "AbortError";
  return error;
}

const thumbnailRequestLimiter = new RequestLimiter(THUMBNAIL_MAX_CONCURRENCY);
const pageResourceCache = new PageResourceCache();

const telemetryState = {
  queue: [],
  flushing: false,
  authenticated: false,
};

const hudState = {
  lastImage: null,
  lastGrid: null,
  displayDurations: [],
  errors: [],
};

const state = {
  authenticated: false,
  favorites: [],
  home: { places: [], smart_folders: [] },
  homeLoadError: "",
  homeTab: "places",
  collection: null,
  container: null,
  gridReturnHash: "#home/places",
  gridHash: "#home/places",
  thumbAspectHeightRatio: 1,
  favoriteId: null,
  favoriteName: "",
  folderPath: "",
  entries: [],
  images: [],
  imageIndex: -1,
  pageGroups: [],
  seekPageGroups: [],
  pageGroupIndex: -1,
  spreadMode: SpreadMode.SINGLE,
  effectiveSpreadMode: SpreadMode.SINGLE,
  readingDirection: ReadingDirection.LTR,
  spreadPageGapPx: 0,
  forceSinglePage: false,
  localSettings: LOCAL_SETTINGS_LOAD.settings,
  localSettingsStorageAvailable: LOCAL_SETTINGS_LOAD.storageAvailable,
  localSettingsDialog: null,
  gestureHelpDialog: null,
  viewerBarsVisible: true,
  viewerPanelTab: "functions",
  coarsePointer: Boolean(globalThis.matchMedia?.("(pointer: coarse)")?.matches),
  keyboardInputSeen: false,
  fitMode: FitMode.PAGE,
  viewerTapSequence: null,
  imageInfoCache: new Map(),
  containerImageInfoHints: new Map(),
  pageDirection: 1,
  requestController: null,
  folderContainerLoad: null,
  virtualGrid: null,
  thumbnailTracker: null,
  viewer: null,
  commandMenu: null,
  remoteAiController: null,
  thumbnailNotice: null,
  screenContext: "loading",
  gridIndex: 0,
  authCountdownTimer: 0,
  remoteSessionStatus: "inactive",
  remoteSessionOwner: null,
  remoteSessionMessage: "",
  remoteSessionAcquirePromise: null,
  remoteSessionUserActive: false,
  remoteSessionTimer: 0,
  viewerItemState: null,
  viewerItemStateSequence: 0,
  pageRenderRevision: 0,
  gridViewerReturn: null,
  appAssetToken: "",
  appUpdateDismissedToken: null,
  appUpdateBanner: null,
};

let recentPointerSource = { source: "mouse", at: 0 };
if (!RUNTIME_TEST_MODE) {
  updateKeyboardAvailability();
  window.addEventListener("popstate", () => {
    acquireRemoteSession("browser_history")
      .then(() => dispatchRoute())
      .catch(() => {});
  });
  window.addEventListener("keydown", onGlobalKeyDown);
  window.addEventListener(
    "pointerdown",
    (event) => {
      state.remoteSessionUserActive = true;
      recentPointerSource = {
        source: pointerInputSource(event.pointerType),
        at: performance.now(),
      };
    },
    true
  );

  if (TELEMETRY_ENABLED) {
    installTelemetry();
  } else {
    hudElement.hidden = true;
  }
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "visible") {
      state.remoteAiController?.suspend();
    } else if (state.authenticated) {
      acquireRemoteSession("visibility_resume")
        .then(() => {
          if (state.viewer?.isVideoStreamViewer) {
            return state.viewer.handleVisibilityResume();
          }
          return state.remoteAiController?.handleForegroundResume();
        })
        .catch(() => {});
    }
  });
  boot();
}

async function boot() {
  renderLoading("接続を確認しています");
  try {
    const response = await fetch("/api/auth/status", {
      credentials: "same-origin",
      headers: { Accept: "application/json" },
    });
    if (!response.ok) throw new Error(`認証状態を確認できません (HTTP ${response.status})。`);
    const status = await response.json();
    if (!status.authenticated) {
      renderPinLogin(status.lockout_remaining_seconds ?? 0);
      return;
    }
    await enterAuthenticatedApp();
  } catch (error) {
    renderError(error);
  }
}

async function enterAuthenticatedApp() {
  state.authenticated = true;
  telemetryState.authenticated = true;
  renderLoading("リモートセッションを取得しています");
  await acquireRemoteSession("authenticated");
  startSessionPing();
  startAppUpdateWatch().catch(() => {});
  renderLoading("お気に入りを読み込んでいます");
  const data = await apiJson("/api/favorites");
  state.favorites = data.favorites ?? [];
  try {
    state.home = await apiJson("/api/home");
    state.homeLoadError = "";
  } catch (error) {
    state.home = { places: [], smart_folders: [] };
    state.homeLoadError =
      error instanceof Error ? error.message : "mIV 本体から一覧を取得できませんでした。";
  }
  if (!location.hash) {
    history.replaceState({ mivRoute: true }, "", "#home/places");
  } else {
    history.replaceState({ ...(history.state ?? {}), mivRoute: true }, "", location.href);
  }
  await dispatchRoute();
}

async function acquireRemoteSession(reason = "operation") {
  if (state.remoteSessionAcquirePromise) return state.remoteSessionAcquirePromise;
  if (state.remoteSessionStatus !== "active") {
    setRemoteSessionStatus("acquiring", "操作権を取得しています…");
  }
  state.remoteSessionAcquirePromise = (async () => {
    try {
      const response = await fetch("/api/session/acquire", {
        method: "POST",
        credentials: "same-origin",
        headers: remoteHeaders({ Accept: "application/json" }),
      });
      if (response.status === 401) {
        renderPinLogin(0);
        throw new AuthenticationRequiredError("PIN 認証が必要です。");
      }
      const result = await response.json().catch(() => ({}));
      if (!response.ok || result.status !== "active") {
        throw new Error(result.message || `操作権を取得できません (HTTP ${response.status})。`);
      }
      setRemoteSessionStatus("active", "");
      state.remoteSessionUserActive = false;
      enqueueTelemetry({ type: "remote_session", action: "acquire", reason });
      return true;
    } catch (error) {
      if (error instanceof AuthenticationRequiredError) throw error;
      setRemoteSessionStatus(
        "unavailable",
        error instanceof Error ? error.message : "操作権を取得できません。"
      );
      throw error;
    } finally {
      state.remoteSessionAcquirePromise = null;
    }
  })();
  return state.remoteSessionAcquirePromise;
}

const APP_UPDATE_POLL_INTERVAL_MS = 5 * 60 * 1000;

/// 走っている版と配信されている版を突き合わせる。
///
/// 画面遷移がハッシュ変更なので、開きっぱなしのタブは自分の script を二度と取りに行かない。
/// 見ている物が古いことは中からは分からないので、こちらから知らせる。
async function readServedAssetToken() {
  const data = await apiJson("/api/app-version");
  return String(data.asset_token ?? "");
}

async function startAppUpdateWatch() {
  try {
    state.appAssetToken = await readServedAssetToken();
  } catch {
    // 起動時に取れなくても本題ではない。次の巡回で拾う。
    return;
  }
  window.setInterval(() => {
    checkForAppUpdate().catch(() => {});
  }, APP_UPDATE_POLL_INTERVAL_MS);
  // 端末を伏せている間に配り直されるのが一番多い経路なので、復帰時に必ず見る。
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") {
      checkForAppUpdate().catch(() => {});
    }
  });
}

async function checkForAppUpdate() {
  if (!state.authenticated) return;
  const notice = appUpdateNotice({
    runningToken: state.appAssetToken,
    servedToken: await readServedAssetToken(),
    dismissedToken: state.appUpdateDismissedToken,
  });
  if (notice.kind !== "update_available") return;
  showAppUpdateBanner(notice.servedToken);
}

/// 勝手に再読み込みはしない。動画や読書の途中で画面が飛ぶ方が害が大きいので、
/// 踏むかどうかは利用者が決める。
function showAppUpdateBanner(servedToken) {
  if (state.appUpdateBanner) return;
  const banner = element("div", "app-update-banner");
  banner.setAttribute("role", "status");
  // 375px 幅で本文・ボタンが 1 行に収まる長さにしている。これより長くすると
  // 本文が折り返し、末尾数文字だけが 2 行目に残る (実測: 14 文字で 2 行)。
  banner.append(textElement("span", "新しい版があります"));
  const reload = textElement("button", "再読み込み", "app-update-banner-reload");
  reload.addEventListener("click", () => reloadApplication());
  const dismiss = textElement("button", "後で", "app-update-banner-dismiss");
  dismiss.addEventListener("click", () => {
    state.appUpdateDismissedToken = servedToken;
    banner.remove();
    state.appUpdateBanner = null;
  });
  banner.append(reload, dismiss);
  document.body.append(banner);
  state.appUpdateBanner = banner;
}

function startSessionPing() {
  if (state.remoteSessionTimer) return;
  state.remoteSessionTimer = window.setInterval(() => {
    pingRemoteSession().catch(() => {});
  }, SESSION_PING_INTERVAL_MS);
}

async function pingRemoteSession() {
  if (
    !state.authenticated ||
    state.remoteSessionStatus !== "active" ||
    document.visibilityState === "hidden"
  ) {
    return;
  }
  const userActive = state.remoteSessionUserActive;
  state.remoteSessionUserActive = false;
  const mediaPlaying = [...document.querySelectorAll("video, audio")].some(
    (media) => !media.paused && !media.ended
  );
  const response = await fetch("/api/session/ping", {
    method: "POST",
    credentials: "same-origin",
    headers: remoteHeaders({ "Content-Type": "application/json", Accept: "application/json" }),
    body: JSON.stringify({ user_active: userActive, media_playing: mediaPlaying }),
  });
  if (response.status === 401) {
    renderPinLogin(0);
    return;
  }
  const result = await response.json().catch(() => ({}));
  if (!response.ok || result.status !== "active") {
    setRemoteSessionStatus(
      sessionStatusFromResponse(result.status, response.status),
      result.message || "リモートセッションが切断されました。操作すると再接続します。"
    );
  }
}

function setRemoteSessionStatus(status, message) {
  state.remoteSessionStatus = status;
  state.remoteSessionMessage = message;
  if (status === "active" || status === "other_device") {
    state.remoteSessionOwner = status;
  }
  updateRemoteSessionOwnerBadge();
  let element = document.querySelector("#remote-session-status");
  if (!element) {
    element = document.createElement("div");
    element.id = "remote-session-status";
    element.className = "remote-session-status";
    document.body.append(element);
  }
  element.hidden = status === "active" || status === "inactive";
  element.dataset.status = status;
  element.textContent = message;
}

function updateRemoteSessionOwnerBadge() {
  sessionOwnerBadgeElement.hidden =
    !state.authenticated || state.remoteSessionOwner === null;
  if (sessionOwnerBadgeElement.hidden) return;
  const presentation = sessionOwnerBadge(state.remoteSessionOwner);
  sessionOwnerBadgeElement.dataset.owner = presentation.owner;
  sessionOwnerBadgeElement.textContent = presentation.label;
}

function sessionStatusFromHttp(status) {
  if (status === 409) return "local_in_use";
  if (status === 428) return "expired";
  return "unavailable";
}

function sessionStatusFromResponse(sessionStatus, httpStatus) {
  if (sessionStatus === "superseded") return "other_device";
  return sessionStatusFromHttp(httpStatus);
}

function renderPinLogin(initialRemainingSeconds = 0) {
  cleanupScreen();
  state.screenContext = "pin";
  state.authenticated = false;
  state.remoteSessionOwner = null;
  telemetryState.authenticated = false;
  updateRemoteSessionOwnerBadge();
  hudElement.hidden = true;
  document.title = "PIN 認証 — mIV Remote";

  const screen = element("section", "pin-screen");
  const card = element("div", "pin-card");
  const form = document.createElement("form");
  form.className = "pin-form";
  const pin = document.createElement("input");
  pin.className = "pin-input";
  pin.type = "password";
  pin.inputMode = "numeric";
  pin.autocomplete = "current-password";
  pin.minLength = 6;
  pin.required = true;
  pin.placeholder = "6桁以上の PIN";
  pin.setAttribute("aria-label", "PIN");

  const forgetLabel = element("label", "pin-forget");
  const forget = document.createElement("input");
  forget.type = "checkbox";
  forgetLabel.append(forget, document.createTextNode("この端末を記憶しない"));
  const submit = textElement("button", "接続する", "pin-submit");
  submit.type = "submit";
  const message = textElement("p", "", "pin-message");
  form.append(pin, forgetLabel, submit, message);
  card.append(
    textElement("h1", "mIV Remote"),
    textElement("p", "接続用 PIN を入力してください。", "pin-description"),
    form
  );
  screen.append(card);
  app.append(screen);

  let lockedUntil = performance.now() + Math.max(0, initialRemainingSeconds) * 1000;
  const updateLockout = () => {
    const remaining = Math.max(0, Math.ceil((lockedUntil - performance.now()) / 1000));
    submit.disabled = remaining > 0;
    pin.disabled = remaining > 0;
    if (remaining > 0) {
      message.textContent = `試行回数が上限に達しました。あと ${remaining} 秒お待ちください。`;
      message.classList.add("error");
    } else if (message.dataset.lockout === "true") {
      message.textContent = "再試行できます。";
      message.classList.remove("error");
      message.dataset.lockout = "false";
      pin.focus();
    }
  };
  if (initialRemainingSeconds > 0) message.dataset.lockout = "true";
  updateLockout();
  state.authCountdownTimer = window.setInterval(updateLockout, 250);
  if (!initialRemainingSeconds) window.setTimeout(() => pin.focus(), 0);

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (submit.disabled) return;
    submit.disabled = true;
    message.textContent = "確認しています…";
    message.classList.remove("error");
    const candidate = pin.value;
    pin.value = "";
    try {
      const response = await fetch("/api/auth/pin", {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json", Accept: "application/json" },
        body: JSON.stringify({ pin: candidate, remember: !forget.checked }),
      });
      const result = await response.json().catch(() => ({}));
      if (response.ok && result.authenticated) {
        clearInterval(state.authCountdownTimer);
        state.authCountdownTimer = 0;
        hudElement.hidden = !TELEMETRY_ENABLED;
        await enterAuthenticatedApp();
        return;
      }
      const remaining = Number(result.lockout_remaining_seconds) || 0;
      if (response.status === 429 && remaining > 0) {
        lockedUntil = performance.now() + remaining * 1000;
        message.dataset.lockout = "true";
        updateLockout();
      } else {
        message.textContent = "PIN が違います。確認してもう一度お試しください。";
        message.classList.add("error");
        submit.disabled = false;
        pin.focus();
      }
    } catch {
      message.textContent = "サーバーに接続できませんでした。";
      message.classList.add("error");
      submit.disabled = false;
    }
  });
}

async function dispatchRoute() {
  if (!state.authenticated) return;
  const route = parseRoute(location.hash);
  rememberGridViewerReturnForRoute(route);
  if (route.kind === 'home') state.gridViewerReturn = null;
  if (
    state.screenContext === "viewer" &&
    route.kind !== "media" &&
    route.kind !== "image"
  ) {
    await flushReadingProgress();
  }
  if (route.kind !== "media") pageResourceCache.clear();
  try {
    if (route.kind === "home") {
      renderHome(route.tab);
      return;
    }
    if (route.kind === "collection") {
      await showCollection(route);
      return;
    }
    if (route.kind === "folder") {
      await showFolder(route.favoriteId, route.path);
      return;
    }
    if (route.kind === "container") {
      await showContainer(route.address);
      return;
    }
    if (route.kind === "media") {
      const parentAddress = parentContainerAddress(route.address);
      if (route.address.subresource.kind === "file") {
        const loaded = await loadFolder(
          parentAddress.favorite_id,
          parentAddress.relative_path,
          performance.now()
        );
        if (!loaded) return;
        const entryIndex = state.entries.findIndex(
          (entry) => addressIdentity(entryAddress(entry)) === addressIdentity(route.address)
        );
        const entry = state.entries[entryIndex];
        if (!entry) {
          throw new Error("フォルダ内のメディアが見つかりませんでした。");
        }
        if (entry.kind === "video") {
          state.gridIndex = entryIndex;
          renderVideoViewer(entry);
          return;
        }
        const imageIndex = await activateFolderContainerForImage(route.address, entryIndex);
        if (imageIndex < 0) {
          throw new Error("表示できるメディアが見つかりませんでした。");
        }
        renderImageViewer(imageIndex, performance.now());
        return;
      }
      await loadContainer(parentAddress);
      const entry = state.entries.find(
        (entry) => addressIdentity(entryAddress(entry)) === addressIdentity(route.address)
      );
      if (!entry) {
        throw new Error("コンテナ内のページが見つかりませんでした。");
      }
      const index = state.images.findIndex(
        (image) => entryIdentity(image) === entryIdentity(entry)
      );
      if (index < 0) throw new Error("表示できるメディアが見つかりませんでした。");
      renderImageViewer(index, performance.now());
      return;
    }
    if (route.kind === "image") {
      const separator = route.path.lastIndexOf("/");
      const folderPath = separator >= 0 ? route.path.slice(0, separator) : "";
      const address = {
        favorite_id: route.favoriteId,
        relative_path: folderPath,
        subresource: { kind: "file" },
      };
      await loadContainer(address);
      const pageAddress = {
        favorite_id: route.favoriteId,
        relative_path: route.path,
        subresource: { kind: "file" },
      };
      const index = state.images.findIndex(
        (entry) =>
          addressIdentity(entryAddress(entry)) === addressIdentity(pageAddress)
      );
      if (index < 0) {
        throw new Error("画像が見つかりませんでした。");
      }
      renderImageViewer(index, performance.now());
      return;
    }
    renderHome("places");
  } catch (error) {
    renderError(error);
  }
}

function parseRoute(hash) {
  if (!hash) {
    return { kind: "home", tab: "places" };
  }
  if (hash === "#favorites") {
    return { kind: "home", tab: "favorites" };
  }
  const home = hash.match(/^#home\/(favorites|smart|places)$/);
  if (home) {
    return { kind: "home", tab: home[1] };
  }
  const collection = hash.match(
    /^#collection\/(reading_history|bookmarks|bookshelf|rating|smart)(?:\/([^/]+))?$/
  );
  if (collection) {
    return {
      kind: "collection",
      collectionKind: collection[1],
      value: collection[2] ?? "",
    };
  }
  const addressed = hash.match(/^#(container|media)\/(.*)$/);
  if (addressed) {
    try {
      return { kind: addressed[1], address: decodeAddress(addressed[2]) };
    } catch {
      return { kind: "home", tab: "places" };
    }
  }
  const match = hash.match(/^#(folder|image)\/([^/]+)\/(.*)$/);
  if (!match) {
    return { kind: "home", tab: "places" };
  }
  try {
    return {
      kind: match[1],
      favoriteId: match[2],
      path: decodeURIComponent(match[3]),
    };
  } catch {
    return { kind: "home", tab: "places" };
  }
}

function homeHash(tab) {
  return `#home/${tab}`;
}

function collectionHash(kind, value = "") {
  return `#collection/${kind}${value ? `/${encodeURIComponent(value)}` : ""}`;
}

function folderHash(favoriteId, path) {
  return `#folder/${favoriteId}/${encodeURIComponent(path)}`;
}

function imageHash(favoriteId, path) {
  return `#image/${favoriteId}/${encodeURIComponent(path)}`;
}

function containerHash(address) {
  return "#container/" + encodeAddress(address);
}

function mediaHash(address) {
  return "#media/" + encodeAddress(address);
}

function encodeAddress(address) {
  return encodeURIComponent(JSON.stringify(address));
}

function decodeAddress(encoded) {
  const address = JSON.parse(decodeURIComponent(encoded));
  if (
    !address ||
    typeof address.favorite_id !== "string" ||
    typeof address.relative_path !== "string" ||
    !address.subresource ||
    typeof address.subresource.kind !== "string"
  ) {
    throw new Error("invalid address");
  }
  return address;
}

function navigate(hash, routeState = {}) {
  if (location.hash === hash) {
    dispatchRoute();
    return;
  }
  history.pushState(
    { mivRoute: true, navigatedInApp: true, ...routeState },
    "",
    hash
  );
  dispatchRoute();
}

export function commandTelemetryEvent(requested, meta, source, context, handled = true) {
  const event = {
    type: "command",
    command: requested.name,
    input_source: source,
    input_detail: meta.detail ? limitText(meta.detail, 80) : undefined,
    context,
  };
  if (requested.name === CommandName.OPEN) {
    event.payload = {
      kind: requested.payload?.kind ?? "missing",
    };
    event.mediaKind = requested.payload?.mediaKind ?? null;
    event.open_route = meta.openRoute ?? "not_reached";
    event.handled = Boolean(handled);
  }
  return event;
}

function dispatchCommand(requested, meta = {}) {
  if (!requested?.name || !state.authenticated) return false;
  const source = meta.source ?? "mouse";
  if (
    requested.name === CommandName.OPEN_LOCAL_SETTINGS ||
    requested.name === CommandName.OPEN_GESTURE_HELP ||
    requested.name === CommandName.RELOAD_APP
  ) {
    state.commandMenu?.close(false);
    if (requested.name === CommandName.OPEN_LOCAL_SETTINGS) {
      openLocalSettingsDialog();
    } else if (requested.name === CommandName.OPEN_GESTURE_HELP) {
      openGestureHelpDialog();
    } else {
      reloadApplication();
    }
    if (meta.telemetry !== false) {
      enqueueTelemetry(
        commandTelemetryEvent(requested, meta, source, state.screenContext)
      );
    }
    return true;
  }
  state.remoteSessionUserActive = true;
  if (meta.sessionRetry !== true) {
    acquireRemoteSession("command")
      .then(() => dispatchCommand(requested, { ...meta, sessionRetry: true }))
      .catch(() => {});
    return true;
  }
  const context = state.screenContext;
  let handled = false;

  if (requested.name === CommandName.TOGGLE_MENU) {
    handled = Boolean(state.commandMenu?.toggle());
  } else if (requested.name === CommandName.BACK) {
    if (state.commandMenu?.isOpen()) {
      state.commandMenu.close();
      handled = true;
    } else if (state.screenContext === "viewer") {
      exitBrowserFullscreen();
      leaveViewerForGrid().catch(() => {});
      handled = true;
    } else if (state.screenContext === "grid") {
      if (history.state?.navigatedInApp) {
        history.back();
      } else {
        dispatchCommand(command(CommandName.PARENT_FOLDER), {
          source,
          detail: "back_fallback",
          telemetry: false,
        });
      }
      handled = true;
    }
  } else if (requested.name === CommandName.FORWARD && state.screenContext === "grid") {
    history.forward();
    handled = true;
  } else if (
    requested.name === CommandName.PARENT_FOLDER &&
    state.screenContext === "grid"
  ) {
    const target = state.container
      ? state.gridReturnHash
      : state.collection
      ? state.gridReturnHash
      : state.folderPath
        ? folderHash(state.favoriteId, parentPath(state.folderPath))
        : homeHash("favorites");
    navigate(target);
    handled = true;
  } else if (
    requested.name === CommandName.OPEN_HOME &&
    state.screenContext === "grid"
  ) {
    const tab = state.collection?.kind === "smart"
      ? "smart"
      : state.favoriteId
        ? "favorites"
        : "places";
    navigate(homeHash(tab));
    handled = true;
  } else if (requested.name === CommandName.OPEN) {
    handled = executeOpenCommand(requested.payload, meta);
  } else if (
    requested.name === CommandName.OPEN_SELECTED &&
    state.screenContext === "grid"
  ) {
    handled = openGridEntry(state.gridIndex, meta);
  } else if (requested.name === CommandName.TOGGLE_FULLSCREEN) {
    toggleBrowserFullscreen();
    handled = true;
  } else if (requested.name === CommandName.GRID_SELECT) {
    const index = Number(requested.payload.index);
    if (state.screenContext === "grid" && Number.isInteger(index)) {
      state.gridIndex = clamp(index, 0, Math.max(0, state.entries.length - 1));
      state.virtualGrid?.focusIndex(state.gridIndex, false);
      handled = true;
    }
  } else if (requested.name.startsWith("grid_") && state.screenContext === "grid") {
    handled = executeGridNavigation(requested.name);
  } else if (state.screenContext === "viewer") {
    if (requested.name === CommandName.TOGGLE_VIEWER_BARS) {
      state.viewerBarsVisible = !state.viewerBarsVisible;
      state.viewer?.setBarsVisible(state.viewerBarsVisible);
      state.commandMenu?.setActionLabel(
        CommandName.TOGGLE_VIEWER_BARS,
        state.viewerBarsVisible ? "上下バーを隠す" : "上下バーを表示"
      );
      handled = true;
    } else if (state.viewer?.isVideoStreamViewer) {
      if (requested.name === CommandName.NEXT_PAGE) {
        const endingViewer = state.viewer;
        const result = changeVideoFile(1, Boolean(requested.payload.wrap));
        handled = result.handled;
        if (
          meta.detail === "video_ended" &&
          !result.advanced &&
          state.viewer === endingViewer
        ) {
          endingViewer.setPlaying(false).catch(() => {});
        }
      } else if (requested.name === CommandName.PREV_PAGE) {
        handled = changeVideoFile(-1).handled;
      } else handled = state.viewer.execute(requested);
    } else if (requested.name === CommandName.NEXT_PAGE) handled = changeImage(1);
    else if (requested.name === CommandName.PREV_PAGE) handled = changeImage(-1);
    else if (requested.name === CommandName.FIRST_PAGE) handled = changeImageTo(0);
    else if (requested.name === CommandName.LAST_PAGE) {
      handled = changeImageTo(state.pageGroups.length - 1);
    } else if (requested.name === CommandName.SPREAD_CYCLE) {
      handled = requestSpreadMode(nextSpreadMode(state.spreadMode));
    } else if (requested.name === CommandName.SET_RATING) {
      const stars = Number(requested.payload.stars);
      if (Number.isInteger(stars) && stars >= 0 && stars <= 5) {
        setViewerRating(stars).catch(() => {});
        handled = true;
      }
    } else if (requested.name === CommandName.TOGGLE_BOOKMARK) {
      toggleViewerBookmark().catch(() => {});
      handled = true;
    } else if (requested.name.startsWith("spread_")) {
      const spreadModes = {
        [CommandName.SPREAD_SINGLE]: SpreadMode.SINGLE,
        [CommandName.SPREAD_LTR]: SpreadMode.LTR,
        [CommandName.SPREAD_LTR_COVER]: SpreadMode.LTR_COVER,
        [CommandName.SPREAD_RTL]: SpreadMode.RTL,
        [CommandName.SPREAD_RTL_COVER]: SpreadMode.RTL_COVER,
      };
      handled = requestSpreadMode(spreadModes[requested.name]);
    } else {
      let fitMode = null;
      if (requested.name === CommandName.FIT_CYCLE) {
        fitMode = nextFitMode(state.fitMode);
      } else if (requested.name === CommandName.FIT_PAGE) {
        fitMode = FitMode.PAGE;
      } else if (requested.name === CommandName.FIT_WIDTH) {
        fitMode = FitMode.WIDTH;
      } else if (requested.name === CommandName.FIT_ORIGINAL) {
        fitMode = FitMode.ORIGINAL;
      } else if (requested.name === CommandName.FIT_TOGGLE_PAGE_ORIGINAL) {
        fitMode = togglePageOriginalFitMode(state.fitMode, {
          scale: state.viewer?.scale,
        });
      }
      if (fitMode) {
        state.fitMode = fitMode;
        updateViewerImage(performance.now()).catch(renderError);
        handled = true;
      } else {
        handled = Boolean(state.viewer?.execute(requested));
      }
    }
  }

  if ((handled || requested.name === CommandName.OPEN) && meta.telemetry !== false) {
    enqueueTelemetry(commandTelemetryEvent(requested, meta, source, context, handled));
  }
  return handled;
}

export function resolveMediaOpenRoute(requestedKind, addressedEntry, imageIndex) {
  if (!["image", "video"].includes(requestedKind)) return null;
  if (!addressedEntry || addressedEntry.kind !== requestedKind) return null;
  if (requestedKind === "image" && imageIndex < 0) return null;
  return requestedKind;
}

function executeOpenCommand(payload, meta) {
  payload = payload ?? {};
  meta = meta ?? {};
  if (payload.kind === "favorite" || payload.kind === "folder") {
    meta.openRoute = payload.kind;
    navigate(folderHash(payload.favoriteId, payload.path ?? ""));
    return true;
  }
  if (payload.kind === "container" && payload.address) {
    meta.openRoute = "container";
    navigate(containerHash(payload.address));
    return true;
  }
  const addressedMediaEntry = payload.kind === "media" && payload.address
    ? state.entries.find(
        (entry) => addressIdentity(entryAddress(entry)) === addressIdentity(payload.address)
      )
    : null;
  const requestedMediaKind = payload.kind === "media" ? payload.mediaKind : payload.kind;
  if (
    requestedMediaKind === "image" &&
    (payload.kind === "image" || addressedMediaEntry?.kind === "image") &&
    !state.collection &&
    !state.container &&
    state.folderContainerLoad
  ) {
    rememberGridViewerReturn(addressedMediaEntry);
    meta.openRoute = "folder_container_image";
    return openFolderImageFromGrid(payload, meta);
  }
  if (payload.kind === "media" && payload.address) {
    const addressedEntry = addressedMediaEntry;
    const imageIndex = state.images.findIndex(
      (entry) => addressIdentity(entryAddress(entry)) === addressIdentity(payload.address)
    );
    const mediaRoute = resolveMediaOpenRoute(requestedMediaKind, addressedEntry, imageIndex);
    if (!mediaRoute) {
      meta.openRoute = "media_open_route_rejected";
      recordClientError("media_open_route_rejected", "メディアの表示経路を解決できませんでした", {
        expected_kind: requestedMediaKind ?? "missing",
        resolved_kind: addressedEntry?.kind ?? "missing",
        entry_found: Boolean(addressedEntry),
        image_found: imageIndex >= 0,
        screen_context: state.screenContext,
      });
      return false;
    }
    meta.openRoute = `media_${mediaRoute}`;
    rememberGridViewerReturn(addressedEntry);
    if (Number.isInteger(payload.entryIndex)) state.gridIndex = payload.entryIndex;
    tryEnterBrowserFullscreen();
    history.pushState(
      {
        mivRoute: true,
        navigatedInApp: true,
        viewerFromGrid: true,
        viewerDepth: 1,
        returnHash: state.gridHash,
      },
      "",
      mediaHash(payload.address)
    );
    if (mediaRoute === "video") {
      if (!renderVideoViewer(addressedEntry)) {
        meta.openRoute = "video_viewer_entry_rejected";
        return false;
      }
    } else {
      renderImageViewer(imageIndex, meta.at ?? performance.now());
    }
    return true;
  }
  if (
    payload.kind !== "image" ||
    !Number.isInteger(payload.imageIndex) ||
    payload.imageIndex < 0 ||
    payload.imageIndex >= state.images.length
  ) {
    return false;
    meta.openRoute = "legacy_image_rejected";
  }
  rememberGridViewerReturn(state.images[payload.imageIndex]);
  if (state.collection) {
    tryEnterBrowserFullscreen();
    meta.openRoute = "collection_image";
    navigate(imageHash(payload.favoriteId, payload.path), {
      viewerFromGrid: true,
      viewerDepth: 1,
      returnHash: location.hash,
    });
    return true;
  }
  if (Number.isInteger(payload.entryIndex)) {
  meta.openRoute = "folder_image";
    state.gridIndex = payload.entryIndex;
  }
  tryEnterBrowserFullscreen();
  history.pushState(
    {
      mivRoute: true,
      navigatedInApp: true,
      viewerFromGrid: true,
      viewerDepth: 1,
      returnHash: folderHash(state.favoriteId, state.folderPath),
    },
    "",
    imageHash(state.favoriteId, payload.path)
  );
  renderImageViewer(payload.imageIndex, meta.at ?? performance.now());
  return true;
}

function openFolderImageFromGrid(payload, meta) {
  const pageAddress = payload.address ?? {
    favorite_id: payload.favoriteId ?? state.favoriteId,
    relative_path: payload.path,
    subresource: { kind: "file" },
  };
  if (!pageAddress.favorite_id || !pageAddress.relative_path) return false;
  if (Number.isInteger(payload.entryIndex)) state.gridIndex = payload.entryIndex;

  const folderLoad = state.folderContainerLoad;
  const returnHash = folderHash(state.favoriteId, state.folderPath);
  tryEnterBrowserFullscreen();
  history.pushState(
    {
      mivRoute: true,
      navigatedInApp: true,
      viewerFromGrid: true,
      viewerDepth: 1,
      returnHash,
    },
    "",
    pageAddress.subresource.kind === "file"
      ? imageHash(pageAddress.favorite_id, pageAddress.relative_path)
      : mediaHash(pageAddress)
  );
  renderLoading("ビューアを準備しています", folderLoad.controller);
  activateFolderContainerForImage(pageAddress, payload.entryIndex)
    .then((imageIndex) => {
      if (imageIndex < 0) {
        throw new Error("画像が見つかりませんでした。");
      }
      renderImageViewer(imageIndex, meta.at ?? performance.now());
    })
    .catch(renderError);
  return true;
}

export async function activateFolderContainerForImage(pageAddress, gridIndex) {
  const folderAddress = parentContainerAddress(pageAddress);
  const identity = addressIdentity(folderAddress);
  const forceSinglePage = containerForceSinglePage();
  const folderLoad = state.folderContainerLoad;

  if (
    !folderLoad ||
    folderLoad.identity !== identity ||
    folderLoad.forceSinglePage !== forceSinglePage
  ) {
    const loaded = await loadContainer(folderAddress, {
      forceSinglePage,
      gridIndex,
    });
    if (!loaded) return -1;
  } else {
    const data = await folderLoad.promise;
    if (
      folderLoad.controller.signal.aborted ||
      state.folderContainerLoad !== folderLoad ||
      state.requestController !== folderLoad.controller
    ) {
      throw abortError();
    }
    applyContainerData(folderAddress, data, forceSinglePage, { gridIndex });
    state.folderContainerLoad = null;
    state.requestController = null;
  }

  return state.images.findIndex(
    (entry) =>
      addressIdentity(entryAddress(entry)) === addressIdentity(pageAddress)
  );
}

function openGridEntry(index, meta) {
  const entry = state.entries[index];
  if (!entry) return false;
  const favoriteId = entry.favorite_id ?? state.favoriteId;
  const path = entryPath(entry);
  if (entryIsFolder(entry)) {
    return executeOpenCommand(
      { kind: "folder", favoriteId, path },
      meta
    );
  }
  if (entry.kind === "zip" || entry.kind === "pdf") {
    return executeOpenCommand(
      { kind: "container", address: entryAddress(entry), entryIndex: index },
      meta
    );
  }
  if (entry.kind === "directory" && entry.address) {
    return executeOpenCommand(
      { kind: "container", address: entry.address, entryIndex: index },
      meta
    );
  }
  if (entry.kind === "video") {
    return executeOpenCommand(
      {
        kind: "media",
        mediaKind: "video",
        address: entryAddress(entry),
        entryIndex: index,
      },
      meta
    );
  }
  if (entry.kind !== "image") return false;
  const imageIndex = state.images.findIndex(
    (image) => entryIdentity(image) === entryIdentity(entry)
  );
  return executeOpenCommand(
    entry.address
      ? {
          kind: "media",
          mediaKind: "image",
          address: entry.address,
          imageIndex,
          entryIndex: index,
        }
      : { kind: "image", favoriteId, path, imageIndex, entryIndex: index },
    meta
  );
}

function executeGridNavigation(name) {
  if (!state.virtualGrid || !state.entries.length) return false;
  const nextIndex = gridIndexForCommand({
    current: state.gridIndex,
    count: state.entries.length,
    columns: state.virtualGrid.columns,
    pageRows: state.virtualGrid.visibleRowCount(),
    name,
  });
  if (nextIndex < 0) return false;
  state.gridIndex = nextIndex;
  state.virtualGrid.focusIndex(nextIndex, true);
  return true;
}

function onGlobalKeyDown(event) {
  state.remoteSessionUserActive = true;
  if (!state.keyboardInputSeen) {
    state.keyboardInputSeen = true;
    updateKeyboardAvailability();
  }
  if (!state.authenticated || event.isComposing) return;
  if (
    isCommandInteractiveTarget(event.target) &&
    !["Escape", "?"].includes(event.key)
  ) {
    return;
  }
  const requested = commandFromKey(
    {
      key: event.key,
      code: event.code,
      altKey: event.altKey,
      ctrlKey: event.ctrlKey,
      metaKey: event.metaKey,
      shiftKey: event.shiftKey,
      repeat: event.repeat,
      editable: isShortcutBlockedTarget(event.target),
      menuOpen: Boolean(state.commandMenu?.isOpen()),
      rtl: isRtlReadingDirection(state.readingDirection),
    },
    state.viewer?.isVideoStreamViewer ? "media" : state.screenContext
  );
  if (!requested) return;
  event.preventDefault();
  dispatchCommand(requested, { source: "keyboard", detail: event.key });
}

function updateKeyboardAvailability() {
  const keyboardAvailable = shouldShowKeyboardShortcuts({
    coarsePointer: state.coarsePointer,
    keyboardUsed: state.keyboardInputSeen,
  });
  state.commandMenu?.setKeyboardAvailable(keyboardAvailable);
  app.classList.toggle(
    "grid-cursor-visible",
    shouldShowGridCursor({ keyboardAvailable })
  );
}

function isShortcutBlockedTarget(target) {
  if (!(target instanceof Element)) return false;
  return Boolean(
    target.closest('input, textarea, select, [contenteditable="true"]')
  );
}

function isCommandInteractiveTarget(target) {
  if (!(target instanceof Element) || target.closest(".grid-tile")) return false;
  return Boolean(target.closest('button, a, [role="menu"]'));
}

function pointerInputSource(pointerType) {
  return pointerType === "mouse" ? "mouse" : "touch";
}

function inputSourceFromEvent(event) {
  if (event.detail === 0) return "keyboard";
  if (typeof event.pointerType === "string" && event.pointerType) {
    return pointerInputSource(event.pointerType);
  }
  if (performance.now() - recentPointerSource.at < 1500) {
    return recentPointerSource.source;
  }
  return "mouse";
}

function rememberGridViewerReturn(entry) {
  if (
    state.screenContext !== 'grid' ||
    !state.virtualGrid ||
    !state.gridHash ||
    !entry
  ) {
    return;
  }
  state.gridViewerReturn = {
    sourceContext: state.gridHash,
    viewedItemIdentity: entryIdentity(entry),
    previousScrollTop: state.virtualGrid.scrollTop(),
  };
}

function rememberGridViewerReturnForRoute(route) {
  if (state.screenContext !== 'grid' || state.gridViewerReturn) return;
  let targetAddress = null;
  if (route.kind === 'media') {
    targetAddress = route.address;
  } else if (route.kind === 'image') {
    targetAddress = {
      favorite_id: route.favoriteId,
      relative_path: route.path,
      subresource: { kind: 'file' },
    };
  }
  if (!targetAddress) return;
  const identity = addressIdentity(targetAddress);
  const entry = state.entries.find(
    (candidate) => addressIdentity(entryAddress(candidate)) === identity
  );
  rememberGridViewerReturn(entry);
}

function updateGridViewerReturnItem(entry) {
  if (state.gridViewerReturn && entry) {
    state.gridViewerReturn.viewedItemIdentity = entryIdentity(entry);
  }
}

function menuCommand(event, name, payload = {}) {
  dispatchCommand(command(name, payload), {
    source: inputSourceFromEvent(event),
    detail: "menu",
  });
}

function cleanupScreen(preserveRequestController = null) {
  clearInterval(state.authCountdownTimer);
  state.authCountdownTimer = 0;
  if (
    state.requestController &&
    state.requestController !== preserveRequestController
  ) {
    state.requestController.abort();
    state.requestController = null;
  }
  if (
    state.folderContainerLoad?.controller !== preserveRequestController
  ) {
    state.folderContainerLoad = null;
  }
  state.virtualGrid?.destroy();
  state.virtualGrid = null;
  state.thumbnailTracker?.destroy();
  state.thumbnailTracker = null;
  state.thumbnailNotice = null;
  state.commandMenu?.destroy();
  state.commandMenu = null;
  state.localSettingsDialog?.destroy();
  state.localSettingsDialog = null;
  state.gestureHelpDialog?.destroy();
  state.gestureHelpDialog = null;
  state.viewerTapSequence = null;
  state.remoteAiController?.destroy();
  state.remoteAiController = null;
  state.viewer?.destroy();
  state.viewer = null;
  state.screenContext = "loading";
  app.replaceChildren();
}

function renderHome(tab = "places") {
  cleanupScreen();
  state.screenContext = "home";
  state.homeTab = ["favorites", "smart", "places"].includes(tab) ? tab : "places";
  state.collection = null;
  state.container = null;
  exitBrowserFullscreen();
  document.title = "mIV Remote";

  const screen = element("section", "screen");
  const content = element("div", "page-content");
  const hero = element("header", "hero hero-with-menu");
  const heroText = element("div");
  heroText.append(
    textElement("h1", "mIV Remote"),
    textElement("p", "読みたい場所を選んでください。")
  );
  hero.append(
    heroText,
    createMenuButton("操作メニュー")
  );
  content.append(hero, createHomeTabs(state.homeTab));
  if (state.homeTab === "favorites") renderFavoriteTab(content);
  else if (state.homeTab === "smart") renderSmartFolderTab(content);
  else renderPlacesTab(content);
  screen.append(content);
  state.commandMenu = new CommandMenu(screen, "home");
  app.append(screen);
}

function createHomeTabs(active) {
  const tabs = element("nav", "home-tabs");
  tabs.setAttribute("aria-label", "ホームの表示切替");
  for (const [id, label] of [
    ["favorites", "お気に入り"],
    ["smart", "スマートフォルダ"],
    ["places", "場所"],
  ]) {
    const button = textElement("button", label, "home-tab");
    button.type = "button";
    button.classList.toggle("active", id === active);
    button.setAttribute("aria-current", id === active ? "page" : "false");
    button.addEventListener("click", () => navigate(homeHash(id)));
    tabs.append(button);
  }
  return tabs;
}

function renderFavoriteTab(content) {
  if (!state.favorites.length) {
    content.append(textElement("p", "お気に入りが登録されていません。", "empty-state"));
    return;
  }
  const list = element("div", "favorite-list");
  for (const favorite of state.favorites) {
    const button = homeCard("◆", favorite.name);
    button.addEventListener("click", (event) => {
      dispatchCommand(
        command(CommandName.OPEN, {
          kind: "favorite",
          favoriteId: favorite.id,
          path: "",
        }),
        { source: inputSourceFromEvent(event), detail: "favorite", at: performance.now() }
      );
    });
    list.append(button);
  }
  content.append(list);
}

function renderSmartFolderTab(content) {
  if (state.homeLoadError) {
    content.append(ipcUnavailableMessage());
    return;
  }
  const definitions = state.home.smart_folders ?? [];
  if (!definitions.length) {
    content.append(textElement("p", "スマートフォルダが登録されていません。", "empty-state"));
    return;
  }
  const list = element("div", "favorite-list");
  for (const definition of definitions) {
    const button = homeCard("◇", definition.name);
    button.addEventListener("click", () => {
      navigate(collectionHash("smart", definition.id), {
        returnHash: homeHash("smart"),
      });
    });
    list.append(button);
  }
  content.append(list);
}

function renderPlacesTab(content) {
  if (state.homeLoadError) {
    content.append(ipcUnavailableMessage());
    return;
  }
  const list = element("div", "favorite-list place-list");
  for (const place of state.home.places ?? []) {
    if (place.kind === "rating") {
      const group = element("section", "rating-card");
      group.append(
        textElement("span", "★", "favorite-icon"),
        textElement("span", place.name, "favorite-name")
      );
      const stars = element("div", "rating-stars");
      for (let rating = 5; rating >= 1; rating -= 1) {
        const button = textElement("button", `★${rating}`, "rating-star-button");
        button.type = "button";
        button.addEventListener("click", () =>
          navigate(collectionHash("rating", String(rating)), {
            returnHash: homeHash("places"),
          })
        );
        stars.append(button);
      }
      group.append(stars);
      list.append(group);
      continue;
    }
    const icon = {
      reading_history: "↻",
      bookshelf: "▥",
      bookmarks: "🔖",
    }[place.kind] ?? "◇";
    const button = homeCard(icon, place.name);
    button.addEventListener("click", () =>
      navigate(collectionHash(place.kind), { returnHash: homeHash("places") })
    );
    list.append(button);
  }
  if (!list.childElementCount) {
    list.append(textElement("p", "表示する場所がありません。", "empty-state"));
  }
  content.append(list);
}

function homeCard(icon, name) {
  const button = element("button", "favorite-card");
  button.type = "button";
  button.append(
    textElement("span", icon, "favorite-icon"),
    textElement("span", name, "favorite-name"),
    textElement("span", "›", "favorite-arrow")
  );
  return button;
}

function ipcUnavailableMessage() {
  const status = element("div", "home-ipc-error");
  status.append(
    textElement("strong", "mIV 本体が起動していません"),
    textElement("p", "mIV を --remote-ipc 付きで起動すると、この一覧を利用できます。")
  );
  return status;
}

async function showCollection(route) {
  renderLoading("一覧を読み込んでいます");
  const params = collectionRequestParams(route);
  const data = await apiJson("/api/collection", params);
  state.collection = {
    kind: route.collectionKind,
    value: route.value,
    title: data.title ?? "一覧",
    truncated: Boolean(data.truncated),
    entryLimit: Number(data.entry_limit) || 0,
  };
  state.container = null;
  state.gridReturnHash =
    route.collectionKind === "smart" ? homeHash("smart") : homeHash("places");
  state.favoriteId = null;
  state.gridHash = location.hash;
  state.favoriteName = data.title ?? "一覧";
  state.folderPath = "";
  state.thumbAspectHeightRatio =
    Number.isFinite(Number(data.thumb_aspect_height_ratio)) &&
    Number(data.thumb_aspect_height_ratio) > 0
      ? Number(data.thumb_aspect_height_ratio)
      : 1;
  state.entries = data.entries ?? [];
  state.images = state.entries.filter((entry) => entry.kind === "image");
  setSinglePageGroups();
  state.gridIndex = 0;
  renderFolder();
}

function collectionRequestParams(route) {
  if (route.collectionKind === "rating") {
    return { kind: "rating", stars: route.value };
  }
  if (route.collectionKind === "smart") {
    return { kind: "smart_folder", id: route.value };
  }
  return { kind: route.collectionKind };
}

async function showFolder(favoriteId, path) {
  const startedAt = performance.now();
  renderLoading("フォルダを読み込んでいます");
  const loaded = await loadFolder(favoriteId, path, startedAt);
  if (!loaded) return;
  renderFolder(loaded.metrics, loaded.requestController);
}

async function showContainer(address) {
  renderLoading("コンテナを読み込んでいます");
  const loaded = await loadContainer(address);
  if (!loaded) return;
  const initialImageIndex = containerInitialImageIndex({
    openMode: state.container?.openMode,
    resumePage: state.container?.resumePage,
    images: state.images,
  });
  const gridAlreadyShown =
    history.state?.containerGridReady === state.gridHash;
  if (initialImageIndex < 0 || gridAlreadyShown) {
    renderFolder();
    return;
  }

  const entry = state.images[initialImageIndex];
  history.replaceState(
    {
      ...(history.state ?? {}),
      mivRoute: true,
      containerGridReady: state.gridHash,
    },
    "",
    location.hash
  );
  tryEnterBrowserFullscreen();
  history.pushState(
    {
      mivRoute: true,
      navigatedInApp: true,
      viewerFromGrid: true,
      viewerDepth: 1,
      returnHash: state.gridHash,
      autoOpenedContainer: true,
    },
    "",
    mediaHash(entryAddress(entry))
  );
  renderImageViewer(initialImageIndex, performance.now());
}

async function loadContainer(address, options = {}) {
  const forceSinglePage = containerForceSinglePage(options);
  state.requestController?.abort();
  state.folderContainerLoad = null;
  const controller = new AbortController();
  state.requestController = controller;
  const data = await apiJson(
    "/api/container",
    addressQueryParams(address, {
      single: forceSinglePage ? 1 : 0,
    }),
    controller.signal
  );
  if (
    controller.signal.aborted ||
    state.requestController !== controller
  ) {
    return false;
  }
  state.requestController = null;
  applyContainerData(address, data, forceSinglePage, options);
  return true;
}

function containerForceSinglePage(options = {}) {
  if (options.forceSinglePage !== undefined) {
    return Boolean(options.forceSinglePage);
  }
  return planSpreadIntent({
    currentDirection: state.readingDirection,
    portraitSinglePage: state.localSettings.portraitSinglePage,
    viewportWidth: window.innerWidth,
    viewportHeight: window.innerHeight,
  }).forceSinglePage;
}

function applyContainerData(address, data, forceSinglePage, options = {}) {
  const effectiveAddress = data.effective_address ?? address;
  state.collection = null;
  state.container = {
    kind: data.kind,
    title: data.title ?? "コンテナ",
    address: effectiveAddress,
    requestedAddress: address,
    truncated: Boolean(data.truncated),
    entryLimit: Number(data.entry_limit) || 0,
    configuredSpreadMode: data.configured_spread_mode ?? SpreadMode.SINGLE,
    effectiveSpreadMode: data.effective_spread_mode ?? SpreadMode.SINGLE,
    readingDirection: data.reading_direction ?? ReadingDirection.LTR,
    forceSinglePage,
    resumePage: data.resume_page ?? null,
    openMode: data.open_mode ?? "grid",
  };
  state.favoriteId = effectiveAddress.favorite_id;
  state.favoriteName =
    state.favorites.find((favorite) => favorite.id === state.favoriteId)?.name ??
    "お気に入り";
  state.folderPath = effectiveAddress.relative_path;
  state.entries = data.entries ?? [];
  state.images = state.entries.filter((entry) => entry.kind === "image");
  state.thumbAspectHeightRatio =
    Number.isFinite(Number(data.thumb_aspect_height_ratio)) &&
    Number(data.thumb_aspect_height_ratio) > 0
      ? Number(data.thumb_aspect_height_ratio)
      : 1;
  state.spreadMode = state.container.configuredSpreadMode;
  state.effectiveSpreadMode = state.container.effectiveSpreadMode;
  state.readingDirection = state.container.readingDirection;
  state.spreadPageGapPx = Math.max(0, Number(data.spread_page_gap_px) || 0);
  state.forceSinglePage = forceSinglePage;
  setContainerPageGroups(data.page_groups ?? []);
  const resumeEntryIndex = state.container.resumePage
    ? state.entries.findIndex(
        (entry) =>
          addressIdentity(entryAddress(entry)) ===
          addressIdentity(state.container.resumePage)
      )
    : -1;
  state.gridIndex = Number.isInteger(options.gridIndex)
    ? clamp(options.gridIndex, 0, Math.max(0, state.entries.length - 1))
    : Math.max(0, resumeEntryIndex);
  state.gridHash =
    data.kind === "folder"
      ? folderHash(effectiveAddress.favorite_id, effectiveAddress.relative_path)
      : containerHash(effectiveAddress);
  state.gridReturnHash = containerParentHash(address);
}

export function containerInitialImageIndex({ openMode, resumePage, images }) {
  if (openMode === "grid" || !images.length) return -1;
  if (openMode === "resume_page" && resumePage) {
    const resumeIndex = images.findIndex(
      (entry) =>
        addressIdentity(entryAddress(entry)) === addressIdentity(resumePage)
    );
    if (resumeIndex >= 0) return resumeIndex;
  }
  return 0;
}

function setSinglePageGroups() {
  state.pageGroups = state.images.map((entry) => ({
    anchor: entry,
    entries: [entry],
  }));
  state.seekPageGroups = state.images.map((_, index) => [index]);
  state.pageGroupIndex = -1;
  state.spreadMode = SpreadMode.SINGLE;
  state.effectiveSpreadMode = SpreadMode.SINGLE;
  state.readingDirection = ReadingDirection.LTR;
  state.spreadPageGapPx = 0;
  state.forceSinglePage = false;
}

function setContainerPageGroups(groups) {
  const byAddress = new Map(
    state.images.map((entry) => [addressIdentity(entryAddress(entry)), entry])
  );
  state.pageGroups = groups
    .map((group) => {
      const entries = (group.pages ?? [])
        .map((address) => byAddress.get(addressIdentity(address)))
        .filter(Boolean);
      const anchor = byAddress.get(addressIdentity(group.anchor));
      if (!anchor || entries.length !== (group.pages ?? []).length) return null;
      return { anchor, entries };
    })
    .filter(Boolean);
  if (!state.pageGroups.length && state.images.length) {
    state.pageGroups = state.images.map((entry) => ({ anchor: entry, entries: [entry] }));
  }
  const imageIndexes = new Map(
    state.images.map((entry, index) => [entryIdentity(entry), index])
  );
  state.seekPageGroups = state.pageGroups.map((group) =>
    group.entries
      .map((entry) => imageIndexes.get(entryIdentity(entry)))
      .filter((index) => Number.isInteger(index))
  );
  state.pageGroupIndex = -1;
}

function pageGroupIndexForEntry(entry) {
  const identity = entryIdentity(entry);
  return state.pageGroups.findIndex((group) =>
    group.entries.some((page) => entryIdentity(page) === identity)
  );
}

function currentPageGroup() {
  return state.pageGroups[state.pageGroupIndex] ?? null;
}

function viewerSeekSnapshot(groupIndex = state.pageGroupIndex) {
  return viewerSeekState({
    groupPageIndexes: state.seekPageGroups,
    currentGroupIndex: groupIndex,
    pageCount: state.images.length,
    rtl: isRtlReadingDirection(state.readingDirection),
  });
}

let spreadWriteTail = Promise.resolve();
let spreadWriteSequence = 0;
let readingProgressBatch = createReadingProgressBatch();
let readingProgressContextIdentity = "";
let readingProgressTimer = 0;
let readingProgressWriteTail = Promise.resolve();

function currentRemotePageTarget() {
  const group = currentPageGroup();
  const entry = group?.anchor;
  if (!entry) return null;
  const address = entryAddress(entry);
  const contextAddress = state.container?.address ?? {
    favorite_id: state.favoriteId,
    relative_path: state.folderPath,
    subresource: { kind: "file" },
  };
  const pageIndex = state.entries.findIndex(
    (candidate) => entryIdentity(candidate) === entryIdentity(entry)
  );
  if (!address?.favorite_id || !contextAddress?.favorite_id || pageIndex < 0) return null;
  return { address, contextAddress, pageIndex };
}

function readingProgressValue() {
  const target = currentRemotePageTarget();
  if (!target) return null;
  const imageIndex = state.images.findIndex(
    (entry) => addressIdentity(entryAddress(entry)) === addressIdentity(target.address)
  );
  const contextIdentity = addressIdentity(target.contextAddress);
  return {
    contextIdentity,
    identity: `${contextIdentity}\n${addressIdentity(target.address)}`,
    request: {
      kind: "record_reading_progress",
      address: target.address,
      context_address: target.contextAddress,
      page_index: target.pageIndex,
      page_number: Math.max(1, imageIndex + 1),
      page_count: state.images.length,
      record_resume: true,
      record_history: true,
    },
  };
}

function enqueueReadingProgress(effect) {
  if (!effect?.request) return readingProgressWriteTail;
  readingProgressWriteTail = readingProgressWriteTail
    .catch(() => {})
    .then(() => apiPostJson("/api/write", effect.request))
    .catch((error) => {
      recordClientError("reading_progress_write_error", error, {
        page: limitText(effect.identity, 240),
      });
    });
  return readingProgressWriteTail;
}

function scheduleReadingProgressTick() {
  clearTimeout(readingProgressTimer);
  readingProgressTimer = 0;
  if (
    !readingProgressBatch.latest ||
    readingProgressBatch.latest.identity === readingProgressBatch.lastEmittedIdentity
  ) {
    return;
  }
  const delay = Math.max(0, readingProgressBatch.nextDueAt - performance.now());
  readingProgressTimer = setTimeout(() => {
    const transition = readingProgressBatchTransition(
      readingProgressBatch,
      { type: "tick", now: performance.now() },
      READING_PROGRESS_INTERVAL_MS
    );
    readingProgressBatch = transition.state;
    enqueueReadingProgress(transition.effect);
    scheduleReadingProgressTick();
  }, delay);
}

function observeReadingProgress() {
  const value = readingProgressValue();
  if (!value) return;
  if (
    readingProgressContextIdentity &&
    readingProgressContextIdentity !== value.contextIdentity
  ) {
    const final = readingProgressBatchTransition(
      readingProgressBatch,
      { type: "flush", now: performance.now() },
      READING_PROGRESS_INTERVAL_MS
    );
    enqueueReadingProgress(final.effect);
    readingProgressBatch = createReadingProgressBatch();
  }
  readingProgressContextIdentity = value.contextIdentity;
  const transition = readingProgressBatchTransition(
    readingProgressBatch,
    { type: "observe", value, now: performance.now() },
    READING_PROGRESS_INTERVAL_MS
  );
  readingProgressBatch = transition.state;
  enqueueReadingProgress(transition.effect);
  scheduleReadingProgressTick();
}

async function refreshViewerItemState() {
  const target = currentRemotePageTarget();
  const menu = state.commandMenu;
  if (!target || state.screenContext !== "viewer") {
    state.viewerItemState = null;
    menu?.setItemState(null, false);
    return null;
  }
  const identity = addressIdentity(target.address);
  const sequence = ++state.viewerItemStateSequence;
  state.viewerItemState = null;
  menu?.setItemState(null, true);
  try {
    const response = await apiPostJson("/api/write", {
      kind: "get_item_state",
      address: target.address,
      context_address: target.contextAddress,
      page_index: target.pageIndex,
      bookmark_supported: false,
    });
    if (
      sequence !== state.viewerItemStateSequence ||
      state.screenContext !== "viewer" ||
      addressIdentity(currentRemotePageTarget()?.address) !== identity
    ) {
      return null;
    }
    const current = response.item_state;
    if (!current || !Number.isInteger(Number(current.rating))) {
      throw new Error("現在のレーティングとブックマークを取得できませんでした。");
    }
    state.viewerItemState = {
      identity,
      rating: clamp(Number(current.rating), 0, 5),
      bookmarkSupported: Boolean(current.bookmark_supported),
      bookmarked: Boolean(current.bookmarked),
    };
    state.commandMenu?.setItemState(state.viewerItemState, false);
    return state.viewerItemState;
  } catch (error) {
    if (sequence === state.viewerItemStateSequence) {
      state.viewerItemState = null;
      state.commandMenu?.setItemState(null, false);
    }
    throw error;
  }
}

async function setViewerRating(stars) {
  const target = currentRemotePageTarget();
  if (!target) return;
  state.commandMenu?.setItemState(null, true);
  let failure = null;
  try {
    await apiPostJson("/api/write", {
      kind: "set_rating",
      address: target.address,
      stars,
    });
  } catch (error) {
    failure = error;
  }
  try {
    await refreshViewerItemState();
  } catch (error) {
    failure ??= error;
  }
  if (failure) {
    state.viewer?.showBoundaryMessage(
      failure instanceof Error ? failure.message : "レーティングを保存できませんでした。"
    );
  }
}

async function toggleViewerBookmark() {
  const target = currentRemotePageTarget();
  const current = state.viewerItemState;
  const identity = addressIdentity(target?.address);
  if (!target || !current?.bookmarkSupported || current.identity !== identity) return;
  state.commandMenu?.setItemState(null, true);
  let failure = null;
  try {
    await apiPostJson("/api/write", {
      kind: "set_bookmark",
      address: target.address,
      context_address: target.contextAddress,
      page_index: target.pageIndex,
      bookmarked: !current.bookmarked,
    });
  } catch (error) {
    failure = error;
  }
  try {
    await refreshViewerItemState();
  } catch (error) {
    failure ??= error;
  }
  if (failure) {
    state.viewer?.showBoundaryMessage(
      failure instanceof Error ? failure.message : "ブックマークを変更できませんでした。"
    );
  }
}

async function flushReadingProgress(reset = true) {
  clearTimeout(readingProgressTimer);
  readingProgressTimer = 0;
  const transition = readingProgressBatchTransition(
    readingProgressBatch,
    { type: "flush", now: performance.now() },
    READING_PROGRESS_INTERVAL_MS
  );
  readingProgressBatch = transition.state;
  enqueueReadingProgress(transition.effect);
  await readingProgressWriteTail;
  if (reset) {
    readingProgressBatch = createReadingProgressBatch();
    readingProgressContextIdentity = "";
  }
}

async function leaveViewerForGrid() {
  await flushReadingProgress();
  const viewerDepth = Number(history.state?.viewerDepth) || 0;
  if (history.state?.viewerFromGrid && viewerDepth > 0) {
    history.go(-viewerDepth);
  } else {
    history.replaceState({ mivRoute: true }, "", state.gridHash);
    await dispatchRoute();
  }
}

function shouldForceSinglePageForViewport() {
  return planSpreadIntent({
    currentDirection: state.readingDirection,
    portraitSinglePage: state.localSettings.portraitSinglePage,
    viewportWidth: window.innerWidth,
    viewportHeight: window.innerHeight,
  }).forceSinglePage;
}

function requestSpreadMode(mode) {
  if (!state.container || !Object.values(SpreadMode).includes(mode)) return false;
  const address = state.container.requestedAddress;
  const spreadIntent = planSpreadIntent({
    address,
    selectedMode: mode,
    currentDirection: state.readingDirection,
    portraitSinglePage: state.localSettings.portraitSinglePage,
    viewportWidth: window.innerWidth,
    viewportHeight: window.innerHeight,
  });
  const writeRequest = spreadIntent.writeRequest;
  if (!writeRequest) return false;
  const readingDirection = writeRequest.reading_direction;
  const identity = addressIdentity(address);
  const sequence = ++spreadWriteSequence;
  state.spreadMode = mode;
  state.readingDirection = readingDirection;
  spreadWriteTail = spreadWriteTail.then(async () => {
    await apiPostJson("/api/write", writeRequest);
    if (
      sequence === spreadWriteSequence &&
      state.container &&
      addressIdentity(state.container.requestedAddress) === identity
    ) {
      await refreshContainerSpread();
    }
  }).catch(async (error) => {
    if (
      sequence === spreadWriteSequence &&
      state.container &&
      addressIdentity(state.container.requestedAddress) === identity
    ) {
      await refreshContainerSpread().catch(() => {});
      state.viewer?.showBoundaryMessage(
        error instanceof Error ? error.message : "見開き設定を保存できませんでした。"
      );
    }
  });
  return true;
}

async function refreshContainerSpread(
  forceSinglePage = shouldForceSinglePageForViewport()
) {
  if (!state.container) return;
  const viewer = state.viewer;
  const current = currentPageGroup()?.anchor ?? state.images[state.imageIndex];
  const currentIdentity = current ? entryIdentity(current) : "";
  const address = state.container.requestedAddress;
  const loaded = await loadContainer(address, {
    forceSinglePage,
  });
  if (!loaded || state.viewer !== viewer || !viewer || !currentIdentity) return;
  const imageIndex = state.images.findIndex(
    (entry) => entryIdentity(entry) === currentIdentity
  );
  if (imageIndex < 0) return;
  const groupIndex = pageGroupIndexForEntry(state.images[imageIndex]);
  if (groupIndex < 0) return;
  const group = state.pageGroups[groupIndex];
  state.pageGroupIndex = groupIndex;
  state.imageIndex = state.images.findIndex(
    (entry) => entryIdentity(entry) === entryIdentity(group.anchor)
  );
  updateGridViewerReturnItem(group.anchor);
  viewer.cancelPendingCenterTap();
  viewer.setSeekState(viewerSeekSnapshot());
  await updateViewerImage(performance.now());
}

export async function loadFolder(
  favoriteId,
  path,
  interactionStartedAt = performance.now()
) {
  const fetchStartedAt = performance.now();
  const requestedPath = path ?? "";
  const sameFolder =
    state.favoriteId === favoriteId && state.folderPath === requestedPath;
  state.requestController?.abort();
  state.folderContainerLoad = null;
  const controller = new AbortController();
  state.requestController = controller;
  const folderAddress = {
    favorite_id: favoriteId,
    relative_path: requestedPath,
    subresource: { kind: "file" },
  };
  const forceSinglePage = containerForceSinglePage();
  const listPromise = apiJson(
    "/api/list",
    { fav: favoriteId, path: requestedPath },
    controller.signal
  );
  const containerLoad = {
    address: folderAddress,
    identity: addressIdentity(folderAddress),
    forceSinglePage,
    controller,
    promise: null,
  };
  containerLoad.promise = apiJson(
    "/api/container",
    addressQueryParams(folderAddress, {
      single: forceSinglePage ? 1 : 0,
    }),
    controller.signal
  );
  // The list is useful without spread metadata. Observe background failures here;
  // opening an image awaits the same promise and reports its error if necessary.
  containerLoad.promise.catch(() => {});
  state.folderContainerLoad = containerLoad;

  const data = await listPromise;
  if (
    controller.signal.aborted ||
    state.requestController !== controller ||
    state.folderContainerLoad !== containerLoad
  ) {
    return null;
  }
  state.collection = null;
  state.container = null;
  state.gridReturnHash = homeHash("favorites");
  state.favoriteId = favoriteId;
  state.favoriteName =
    state.favorites.find((favorite) => favorite.id === favoriteId)?.name ?? "お気に入り";
  state.folderPath = data.path ?? "";
  state.gridHash = folderHash(favoriteId, state.folderPath);
  state.thumbAspectHeightRatio =
    Number.isFinite(Number(data.thumb_aspect_height_ratio)) &&
    Number(data.thumb_aspect_height_ratio) > 0
      ? Number(data.thumb_aspect_height_ratio)
      : 1;
  state.entries = (data.entries ?? []).filter(
    (entry) =>
      entry.kind === "dir" ||
      entry.kind === "image" ||
      entry.kind === "video" ||
      entry.kind === "zip" ||
      entry.kind === "pdf"
  );
  state.images = state.entries.filter((entry) => entry.kind === "image");
  setSinglePageGroups();
  state.gridIndex = sameFolder
    ? clamp(state.gridIndex, 0, Math.max(0, state.entries.length - 1))
    : 0;
  return {
    metrics: {
      interactionStartedAt,
      fetchMs: performance.now() - fetchStartedAt,
      entryCount: state.entries.length,
      containerCount: state.entries.filter((entry) =>
        ["zip", "pdf"].includes(entry.kind)
      ).length,
    },
    requestController: controller,
    containerLoad,
  };
}

function renderFolder(listMetrics = null, preserveRequestController = null) {
  const renderStartedAt = performance.now();
  const gridViewerReturn = state.gridViewerReturn;
  state.gridViewerReturn = null;
  cleanupScreen(preserveRequestController);
  state.screenContext = "grid";
  exitBrowserFullscreen();
  document.title =
    (state.collection?.title ?? state.container?.title ?? state.favoriteName) +
    " — mIV Remote";

  const screen = element("section", "screen");
  const topbar = element("header", "topbar");
  const parent = textElement("button", "↑", "icon-button");
  parent.classList.add("navigation-icon");
  parent.type = "button";
  parent.setAttribute("aria-label", "親フォルダへ");
  parent.addEventListener("click", (event) => {
    dispatchCommand(command(CommandName.PARENT_FOLDER), {
      source: inputSourceFromEvent(event),
      detail: "toolbar",
    });
  });
  const home = textElement("button", "⌂", "icon-button");
  home.classList.add("navigation-icon");
  home.type = "button";
  home.setAttribute("aria-label", "ホームへ");
  home.addEventListener("click", (event) => {
    dispatchCommand(command(CommandName.OPEN_HOME), {
      source: inputSourceFromEvent(event),
      detail: "toolbar",
    });
  });
  topbar.append(parent, home, buildBreadcrumbs(), createMenuButton("操作メニュー"));

  const scroll = element("div", "grid-scroll");
  const thumbnailNotice = textElement("p", "", "thumbnail-service-notice");
  thumbnailNotice.hidden = true;
  state.thumbnailNotice = thumbnailNotice;
  const collectionLimitNotice = textElement(
    "p",
    (state.collection ?? state.container)?.truncated
      ? "件数が多いため先頭 " +
        (state.collection ?? state.container).entryLimit +
        " 件を表示しています。"
      : "",
    "thumbnail-service-notice"
  );
  collectionLimitNotice.hidden = !(state.collection ?? state.container)?.truncated;
  const space = element("div", "virtual-space");
  const windowElement = element("div", "virtual-window");
  space.append(windowElement);
  scroll.append(space);
  screen.append(topbar, collectionLimitNotice, thumbnailNotice, scroll);
  screen.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    dispatchCommand(command(CommandName.TOGGLE_MENU), {
      source: "mouse",
      detail: "contextmenu",
    });
  });
  state.commandMenu = new CommandMenu(screen, "grid");
  app.append(screen);
  state.thumbnailTracker = new ThumbnailGridTracker(
    renderStartedAt,
    state.entries.length
  );

  if (!state.entries.length) {
    const empty = textElement(
      "p",
      state.container
        ? "このコンテナには表示できるページがありません。"
        : "このフォルダには表示できるサブフォルダまたは画像がありません。",
      "empty-state center-status"
    );
    scroll.replaceChildren(empty);
    state.thumbnailTracker.begin([]);
    return;
  }

  const imageIndexes = new Map(
    state.images.map((entry, index) => [entryIdentity(entry), index])
  );
  const labelHeight = gridLabelHeightForEntries(state.entries);
  state.virtualGrid = new VirtualGrid(
    scroll,
    space,
    windowElement,
    state.entries,
    (entry, index, cellWidth) =>
      createGridTile(entry, index, imageIndexes, state.thumbnailTracker, cellWidth),
    (initialItems) => state.thumbnailTracker?.begin(initialItems),
    state.thumbAspectHeightRatio,
    labelHeight
  );
  const returnViewport = resolveGridReturnViewport({
    ...gridViewerReturn,
    destinationContext: state.gridHash,
    itemIdentities: state.entries.map(entryIdentity),
    columns: state.virtualGrid.columns,
    rowPitch: state.virtualGrid.rowHeight,
    viewportHeight: scroll.clientHeight,
  });
  if (returnViewport) {
    // VirtualGrid の layout で列数・行高・最大 offset が確定した後、最初の仮想セルを
    // materialize する前に復元する。DOM セルの完成待ちや 1-frame 遅延は不要で、先頭が
    // 一瞬描かれることもない。
    state.virtualGrid.restoreScrollTop(returnViewport.scrollTop);
    if (returnViewport.targetIndex >= 0) {
      state.gridIndex = returnViewport.targetIndex;
      state.virtualGrid.focusIndex(state.gridIndex, false);
    }
  } else {
    state.virtualGrid.focusIndex(state.gridIndex, false);
  }
  if (listMetrics) {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        enqueueTelemetry({
          type: "folder_list",
          fetch_ms: roundMs(listMetrics.fetchMs),
          first_paint_ms: roundMs(
            performance.now() - listMetrics.interactionStartedAt
          ),
          entry_count: listMetrics.entryCount,
          container_count: listMetrics.containerCount,
        });
      });
    });
  }
}

function buildBreadcrumbs() {
  const breadcrumbs = element("nav", "breadcrumbs");
  breadcrumbs.setAttribute("aria-label", "パンくず");
  if (state.collection) {
    breadcrumbs.append(textElement("h1", state.collection.title));
    return breadcrumbs;
  }
  if (state.container && state.container.kind !== "folder") {
    const relativeSegments = state.container.address.relative_path
      .split("/")
      .filter(Boolean);
    const fileName = relativeSegments.pop() ?? state.container.title;
    const crumbs = [
      {
        label: state.favoriteName,
        command: {
          kind: "folder",
          favoriteId: state.favoriteId,
          path: "",
        },
      },
    ];
    let folderPath = "";
    for (const segment of relativeSegments) {
      folderPath = folderPath ? folderPath + "/" + segment : segment;
      crumbs.push({
        label: segment,
        command: {
          kind: "folder",
          favoriteId: state.favoriteId,
          path: folderPath,
        },
      });
    }
    const rootAddress = {
      favorite_id: state.favoriteId,
      relative_path: state.container.address.relative_path,
      subresource: { kind: "file" },
    };
    crumbs.push({
      label: fileName,
      command: { kind: "container", address: rootAddress },
    });
    const subresource = state.container.address.subresource;
    if (subresource.kind === "zip_directory") {
      let prefix = "";
      for (const segment of subresource.prefix.split("/").filter(Boolean)) {
        prefix += segment + "/";
        crumbs.push({
          label: segment,
          command: {
            kind: "container",
            address: {
              favorite_id: state.favoriteId,
              relative_path: state.container.address.relative_path,
              subresource: { kind: "zip_directory", prefix },
            },
          },
        });
      }
    }
    crumbs.forEach((crumb, index) => {
      if (index) breadcrumbs.append(textElement("span", "›", "crumb-separator"));
      const button = textElement("button", crumb.label, "crumb");
      button.type = "button";
      button.addEventListener("click", (event) => {
        dispatchCommand(command(CommandName.OPEN, crumb.command), {
          source: inputSourceFromEvent(event),
          detail: "breadcrumb",
        });
      });
      breadcrumbs.append(button);
    });
    requestAnimationFrame(() => {
      breadcrumbs.scrollLeft = breadcrumbs.scrollWidth;
    });
    return breadcrumbs;
  }
  const segments = state.folderPath ? state.folderPath.split("/") : [];
  const crumbs = [{ label: state.favoriteName, path: "" }];
  let accumulated = "";
  for (const segment of segments) {
    accumulated = accumulated ? `${accumulated}/${segment}` : segment;
    crumbs.push({ label: segment, path: accumulated });
  }

  crumbs.forEach((crumb, index) => {
    if (index) {
      breadcrumbs.append(textElement("span", "›", "crumb-separator"));
    }
    const button = textElement("button", crumb.label, "crumb");
    button.type = "button";
    button.addEventListener("click", (event) => {
      dispatchCommand(
        command(CommandName.OPEN, {
          kind: "folder",
          favoriteId: state.favoriteId,
          path: crumb.path,
        }),
        { source: inputSourceFromEvent(event), detail: "breadcrumb" }
      );
    });
    breadcrumbs.append(button);
  });
  requestAnimationFrame(() => {
    breadcrumbs.scrollLeft = breadcrumbs.scrollWidth;
  });
  return breadcrumbs;
}

export function createGridTile(
  entry,
  entryIndex,
  imageIndexes,
  thumbnailTracker,
  cellWidth,
  commandDispatcher = dispatchCommand
) {
  const tile = element("button", "grid-tile");
  tile.type = "button";
  tile.title = entry.name;
  tile.dataset.entryIndex = String(entryIndex);
  tile.classList.toggle("page-tile", Boolean(state.container) && entry.kind === "image");
  tile.classList.toggle("image-tile", entry.kind === "image");
  tile.classList.toggle("grid-active", entryIndex === state.gridIndex);
  tile.addEventListener("focus", () => {
    commandDispatcher(command(CommandName.GRID_SELECT, { index: entryIndex }), {
      source: "keyboard",
      detail: "focus",
      telemetry: false,
    });
  });
  const preview = element("span", "tile-preview");
  const image = document.createElement("img");
  image.alt = "";
  image.loading = "lazy";
  image.decoding = "async";
  image.dataset.telemetryObserved = "true";

  const favoriteId = entry.favorite_id ?? state.favoriteId;
  if (entryIsFolder(entry)) {
    preview.append(textElement("span", "◆", "folder-glyph"));
    preview.append(image);
    preview.append(textElement("span", "folder", "type-badge"));
    tile.addEventListener("click", (event) => {
      commandDispatcher(
        command(CommandName.OPEN, {
          kind: "folder",
          favoriteId,
          path: entryPath(entry),
          entryIndex,
        }),
        { source: inputSourceFromEvent(event), detail: "grid_tile" }
      );
    });
  } else {
    preview.append(textElement("span", "◇", "file-glyph"));
    preview.append(image);
    if (entry.kind !== "image") {
      preview.append(textElement("span", entryTypeLabel(entry.kind), "type-badge"));
    }
    if (entry.kind === "image") {
      tile.addEventListener("click", (event) => {
        const index = imageIndexes.get(entryIdentity(entry));
        if (index !== undefined) {
          const payload = entry.address
            ? {
                kind: "media",
                mediaKind: "image",
                address: entry.address,
                imageIndex: index,
                entryIndex,
              }
            : {
                kind: "image",
                favoriteId,
                path: entryPath(entry),
                imageIndex: index,
                entryIndex,
              };
          commandDispatcher(command(CommandName.OPEN, payload), {
            source: inputSourceFromEvent(event),
            detail: "grid_tile",
            at: performance.now(),
          });
        }
      });
    } else if (entry.kind === "video") {
      tile.addEventListener("click", (event) => {
        commandDispatcher(command(CommandName.OPEN, {
          kind: "media",
          mediaKind: "video",
          address: entryAddress(entry),
          entryIndex,
        }), {
          source: inputSourceFromEvent(event),
          detail: "grid_tile",
          at: performance.now(),
        });
      });
    } else if (["zip", "pdf", "directory"].includes(entry.kind)) {
      tile.addEventListener("click", (event) => {
        commandDispatcher(
          command(CommandName.OPEN, {
            kind: "container",
            address: entryAddress(entry),
            entryIndex,
          }),
          {
            source: inputSourceFromEvent(event),
            detail: "grid_tile",
            at: performance.now(),
          }
        );
      });
    }
  }
  const label = element("span", "tile-label");
  label.append(textElement("span", entry.name, "tile-name"));
  const detail = entry.detail || (entry.rating ? "★" + entry.rating : "");
  if (detail) {
    label.append(
      textElement("span", detail, "entry-detail-badge")
    );
  }
  if (detail) label.title = entry.name + " — " + detail;
  tile.append(preview, label);
  tile._thumbnailBinding = { image, entry, tracker: thumbnailTracker, cellWidth };
  return tile;
}

function entryPath(entry) {
  return entry.relative_path ?? entry.path ?? "";
}

function entryAddress(entry) {
  if (entry.address) return entry.address;
  return {
    favorite_id: entry.favorite_id ?? state.favoriteId,
    relative_path: entryPath(entry),
    subresource: { kind: "file" },
  };
}
export function thumbnailAddressForEntry(entry) {
  return entry.thumbnail_address ?? entryAddress(entry);
}

function thumbnailBindingKey(entry) {
  return `${entryIdentity(entry)}\nthumbnail\n${addressIdentity(thumbnailAddressForEntry(entry))}`;
}


function addressIdentity(address) {
  const target = address?.subresource ?? {};
  const inner =
    target.kind === "zip_entry"
      ? target.entry_name
      : target.kind === "zip_directory"
        ? target.prefix
        : target.kind === "pdf_page"
          ? String(target.page_number)
          : "";
  return [
    address?.favorite_id ?? "",
    address?.relative_path ?? "",
    target.kind ?? "",
    inner,
  ].join("\n");
}

function entryIdentity(entry) {
  return entry.address
    ? addressIdentity(entry.address)
    : `${entry.favorite_id ?? state.favoriteId ?? ""}\n${entryPath(entry)}`;
}

function addressQueryParams(address, extra = {}) {
  const params = {
    fav: address.favorite_id,
    path: address.relative_path,
    ...extra,
  };
  const target = address.subresource;
  if (target.kind === "zip_entry") params.entry = target.entry_name;
  else if (target.kind === "zip_directory") params.prefix = target.prefix;
  else if (target.kind === "pdf_page") params.page = target.page_number;
  return params;
}

export function parentContainerAddress(address) {
  if (address.subresource.kind === "file") {
    const separator = address.relative_path.lastIndexOf("/");
    return {
      favorite_id: address.favorite_id,
      relative_path:
        separator >= 0 ? address.relative_path.slice(0, separator) : "",
      subresource: { kind: "file" },
    };
  }
  if (address.subresource.kind === "pdf_page") {
    return {
      favorite_id: address.favorite_id,
      relative_path: address.relative_path,
      subresource: { kind: "file" },
    };
  }
  const segments = address.subresource.entry_name.split("/");
  segments.pop();
  const prefix = segments.length ? segments.join("/") + "/" : "";
  return {
    favorite_id: address.favorite_id,
    relative_path: address.relative_path,
    subresource: prefix
      ? { kind: "zip_directory", prefix }
      : { kind: "file" },
  };
}

function containerParentHash(requestedAddress) {
  if (requestedAddress.subresource.kind === "zip_directory") {
    const segments = requestedAddress.subresource.prefix.split("/").filter(Boolean);
    segments.pop();
    const prefix = segments.length ? segments.join("/") + "/" : "";
    const parentAddress = {
      favorite_id: requestedAddress.favorite_id,
      relative_path: requestedAddress.relative_path,
      subresource: prefix
        ? { kind: "zip_directory", prefix }
        : { kind: "file" },
    };
    return containerHash(parentAddress);
  }
  const separator = requestedAddress.relative_path.lastIndexOf("/");
  const parentPath =
    separator >= 0 ? requestedAddress.relative_path.slice(0, separator) : "";
  return folderHash(requestedAddress.favorite_id, parentPath);
}

function entryIsFolder(entry) {
  return entry.kind === "dir" || entry.kind === "folder";
}

function entryTypeLabel(kind) {
  return {
    video: "video",
    audio: "audio",
    zip: "zip",
    pdf: "pdf",
    directory: "folder",
    archive: "archive",
    other: "file",
  }[kind] ?? kind;
}

function renderVideoViewer(entry) {
  updateGridViewerReturnItem(entry);
  if (!entry || entry.kind !== "video") {
    recordClientError("video_viewer_entry_rejected", "動画ビューアに動画以外が渡されました", {
      entry_found: Boolean(entry),
      resolved_kind: entry?.kind ?? "missing",
      screen_context: state.screenContext,
    });
    return false;
  }
  cleanupScreen();
  state.screenContext = "viewer";
  state.viewerItemState = null;
  state.viewerItemStateSequence += 1;
  state.imageIndex = -1;
  document.title = `${entry.name} — mIV Remote`;
  const viewer = new VideoStreamViewer({
    entry,
    address: entryAddress(entry),
    dispatch: (requested, meta) => dispatchCommand(requested, meta),
    inputSource: inputSourceFromEvent,
    apiJson,
    apiPostJson,
    reportPlaybackIssue: ({ category, internalReason, ...details }) => {
      recordClientError(category, internalReason, {
        internal_reason: internalReason,
        ...details,
      });
    },
    keyboardAvailable: shouldShowKeyboardShortcuts({
      coarsePointer: state.coarsePointer,
      keyboardUsed: state.keyboardInputSeen,
    }),
  });
  if (!state.viewerBarsVisible) viewer.setBarsVisible(false);
  state.viewer = viewer;
  state.commandMenu = viewer.menu;
  app.append(viewer.root);
  viewer.start().catch((error) => {
    if (state.viewer === viewer) {
      viewer.showOperationalError(error, "動画を開始できませんでした");
    }
  });
  return true;
}

export function videoFileTargetIndex(currentIndex, count, delta, wrap = false) {
  const length = Math.max(0, Math.floor(Number(count) || 0));
  const current = Math.floor(Number(currentIndex));
  const step = Math.sign(Number(delta) || 0);
  if (!Number.isInteger(current) || current < 0 || current >= length || !step) return -1;
  const target = current + step;
  if (target >= 0 && target < length) return target;
  if (!wrap || !length) return -1;
  return target < 0 ? length - 1 : 0;
}

function changeVideoFile(delta, wrap = false) {
  const viewer = state.viewer;
  if (!viewer?.isVideoStreamViewer) return { handled: false, advanced: false };
  const videos = state.entries.filter((entry) => entry.kind === "video");
  const current = videos.findIndex(
    (entry) => addressIdentity(entryAddress(entry)) === addressIdentity(viewer.address)
  );
  const nextIndex = videoFileTargetIndex(current, videos.length, delta, wrap);
  if (nextIndex < 0) {
    viewer.showBoundaryMessage(Number(delta) < 0 ? "先頭の動画です" : "最後の動画です");
    return { handled: true, advanced: false };
  }
  const entry = videos[nextIndex];
  const entryIndex = state.entries.findIndex(
    (candidate) => entryIdentity(candidate) === entryIdentity(entry)
  );
  if (entryIndex >= 0) state.gridIndex = entryIndex;
  history.pushState(
    {
      ...(history.state ?? {}),
      mivRoute: true,
      viewerFromGrid: Boolean(history.state?.viewerFromGrid),
      viewerDepth: (Number(history.state?.viewerDepth) || 0) + 1,
    },
    "",
    mediaHash(entryAddress(entry))
  );
  renderVideoViewer(entry);
  return { handled: true, advanced: true };
}

function renderImageViewer(index, interactionStartedAt = performance.now()) {
  const previousIndex = state.imageIndex;
  const requestedEntry = state.images[index];
  const groupIndex = pageGroupIndexForEntry(requestedEntry);
  if (!requestedEntry || groupIndex < 0) return;
  updateGridViewerReturnItem(state.pageGroups[groupIndex].anchor);
  cleanupScreen();
  state.screenContext = "viewer";
  state.viewerItemState = null;
  state.viewerItemStateSequence += 1;
  if (previousIndex >= 0 && previousIndex !== index) {
    state.pageDirection = index > previousIndex ? 1 : -1;
  }
  state.pageGroupIndex = groupIndex;
  const group = currentPageGroup();
  const imageEntry = group.anchor;
  state.imageIndex = state.images.findIndex(
    (entry) => entryIdentity(entry) === entryIdentity(imageEntry)
  );
  document.title = `${imageEntry.name} — mIV Remote`;

  const viewerRoot = element("section", "image-viewer");
  if (!state.viewerBarsVisible) viewerRoot.classList.add("viewer-bars-hidden");
  const stage = element("div", "viewer-stage");
  const pageLayer = element("div", "viewer-pages");
  const image = element("img", "viewer-image");
  image.alt = imageEntry.name;
  image.draggable = false;
  image.dataset.telemetryObserved = "true";
  const loadingIndicator = element("div", "viewer-loading-indicator");
  loadingIndicator.hidden = true;
  loadingIndicator.setAttribute("role", "progressbar");
  loadingIndicator.setAttribute("aria-label", "次のページを読み込んでいます");
  loadingIndicator.append(element("span", "viewer-loading-indicator-bar"));
  const boundaryMessage = element("div", "viewer-boundary-message");
  boundaryMessage.hidden = true;
  boundaryMessage.setAttribute("role", "status");
  boundaryMessage.setAttribute("aria-live", "polite");
  pageLayer.append(image);
  stage.append(pageLayer, loadingIndicator, boundaryMessage);

  const top = element("div", "viewer-ui top");
  const close = textElement("button", "×", "viewer-button");
  close.type = "button";
  close.setAttribute("aria-label", "フォルダへ戻る");
  const title = textElement("div", imageEntry.name, "viewer-title");
  top.append(close, title, createMenuButton("操作メニュー", "viewer-button"));

  const bottom = element("div", "viewer-ui bottom");
  const previous = textElement("button", "‹", "viewer-button");
  previous.type = "button";
  previous.setAttribute("aria-label", "前の画像");
  const seek = element("div", "viewer-seek");
  const counter = textElement("output", "", "viewer-counter");
  const seekInput = element("input", "viewer-seek-input");
  seekInput.type = "range";
  seekInput.step = "1";
  seekInput.setAttribute("aria-label", "ページ位置");
  seek.append(counter, seekInput);
  const next = textElement("button", "›", "viewer-button");
  next.type = "button";
  next.setAttribute("aria-label", "次の画像");
  bottom.append(previous, seek, next);
  viewerRoot.append(stage, top, bottom);
  state.commandMenu = new CommandMenu(viewerRoot, "viewer", viewerRoot);
  app.append(viewerRoot);

  state.viewer = new ImageViewer({
    root: viewerRoot,
    stage,
    pageLayer,
    image,
    title,
    counter,
    seek,
    seekInput,
    previous,
    next,
    loadingIndicator,
    boundaryMessage,
  });
  state.remoteAiController = new RemoteAiController(state.viewer, stage);
  close.addEventListener("click", (event) => {
    event.stopPropagation();
    dispatchCommand(command(CommandName.BACK), {
      source: inputSourceFromEvent(event),
      detail: "toolbar",
    });
  });
  previous.addEventListener("click", (event) => {
    event.stopPropagation();
    dispatchCommand(command(isRtlReadingDirection(state.readingDirection)
      ? CommandName.NEXT_PAGE
      : CommandName.PREV_PAGE), {
      source: inputSourceFromEvent(event),
      detail: "toolbar",
    });
  });
  next.addEventListener("click", (event) => {
    event.stopPropagation();
    dispatchCommand(command(isRtlReadingDirection(state.readingDirection)
      ? CommandName.PREV_PAGE
      : CommandName.NEXT_PAGE), {
      source: inputSourceFromEvent(event),
      detail: "toolbar",
    });
  });
  seekInput.addEventListener("input", (event) => {
    event.stopPropagation();
    const groupIndex = viewerSeekGroupIndex(
      seekInput.value,
      state.pageGroups.length,
      isRtlReadingDirection(state.readingDirection)
    );
    state.viewer?.setSeekState(viewerSeekSnapshot(groupIndex));
  });
  seekInput.addEventListener("change", (event) => {
    event.stopPropagation();
    const groupIndex = viewerSeekGroupIndex(
      seekInput.value,
      state.pageGroups.length,
      isRtlReadingDirection(state.readingDirection)
    );
    if (!changeImageTo(groupIndex)) {
      state.viewer?.setSeekState(viewerSeekSnapshot());
    }
  });
  const viewer = state.viewer;
  updateViewerImage(interactionStartedAt).then(() => {
    if (
      state.viewer === viewer &&
      !state.localSettings.gestureHelpDismissed &&
      !state.gestureHelpDialog
    ) {
      openGestureHelpDialog();
    }
  }).catch(renderError);
}

function changeImage(delta) {
  const message = viewerBoundaryMessage({
    currentIndex: state.pageGroupIndex,
    count: state.pageGroups.length,
    delta,
    readingDirection: state.readingDirection,
  });
  if (message) {
    state.viewer?.showBoundaryMessage(message);
    return true;
  }
  return changeImageTo(state.pageGroupIndex + delta);
}

function changeImageTo(nextGroupIndex) {
  if (nextGroupIndex < 0 || nextGroupIndex >= state.pageGroups.length) {
    return false;
  }
  if (nextGroupIndex === state.pageGroupIndex) return false;
  state.viewer?.hideBoundaryMessage();
  state.pageDirection = nextGroupIndex > state.pageGroupIndex ? 1 : -1;
  state.pageGroupIndex = nextGroupIndex;
  state.viewer?.setSeekState(viewerSeekSnapshot(nextGroupIndex));
  const entry = state.pageGroups[nextGroupIndex].anchor;
  updateGridViewerReturnItem(entry);
  state.imageIndex = state.images.findIndex(
    (image) => entryIdentity(image) === entryIdentity(entry)
  );
  const viewerDepth = (Number(history.state?.viewerDepth) || 0) + 1;
  const targetHash = entry.address
    ? mediaHash(entry.address)
    : imageHash(state.favoriteId, entry.path);
  history.pushState(
    {
      ...(history.state ?? {}),
      mivRoute: true,
      viewerFromGrid: Boolean(history.state?.viewerFromGrid),
      viewerDepth,
    },
    "",
    targetHash
  );
  updateViewerImage(performance.now()).catch(renderError);
  return true;
}

async function updateViewerImage(interactionStartedAt = performance.now()) {
  const group = currentPageGroup();
  const viewer = state.viewer;
  if (!group || !viewer) return;
  const identity = group.entries.map(entryIdentity).join("\n");
  const infos = await Promise.all(group.entries.map(imageInfo));
  if (
    state.viewer !== viewer ||
    currentPageGroup()?.entries.map(entryIdentity).join("\n") !== identity
  ) {
    return;
  }
  const layout = viewerSpreadLayout({
    mode: state.fitMode,
    pages: infos,
    viewportWidth: viewer.stage.clientWidth || window.innerWidth,
    viewportHeight: viewer.stage.clientHeight || window.innerHeight,
    devicePixelRatio: window.devicePixelRatio || 1,
    gap: group.entries.length > 1 ? state.spreadPageGapPx : 0,
  });
  const pages = group.entries.map((entry, pageIndex) => ({
    entry,
    info: infos[pageIndex],
    request: imageRequest(entry, infos[pageIndex], viewer.stage, {
      layout: layout.pages[pageIndex],
    }),
  }));
  document.title = `${group.anchor.name} — mIV Remote`;
  const displayed = await viewer.loadGroup({
    pages,
    name: group.entries.map((entry) => entry.name).join(" / "),
    fitMode: state.fitMode,
    gap: layout.gap,
    index: state.pageGroupIndex,
    count: state.pageGroups.length,
    seekState: viewerSeekSnapshot(),
    interactionStartedAt,
  });
  if (!displayed || state.viewer !== viewer) return;
  state.remoteAiController?.displayGroup(
    pages.map(({ entry, request }) => ({
      address: entryAddress(entry),
      target_px: request.width,
      name: entry.name,
    }))
  ).catch((error) => state.remoteAiController?.showRequestError(error));
  state.commandMenu?.refreshAdjustment();
  observeReadingProgress();
  if (group.entries.every((entry) => entry.address)) {
    schedulePagePrefetch(viewer).catch(() => {});
    return;
  }
  const nextEntry = state.images[state.imageIndex + 1];
  if (nextEntry) {
    imageInfo(nextEntry).then((nextInfo) => {
      if (state.viewer !== viewer) return;
      const preload = new Image();
      preload.decoding = "async";
      preload.src = imageRequest(nextEntry, nextInfo, viewer.stage).url;
    }).catch(() => {});
  }
}

async function schedulePagePrefetch(viewer) {
  const group = currentPageGroup();
  const currentIdentity = group?.entries.map(entryIdentity).join("\n") ?? "";
  const visibleIndexes = (group?.entries ?? [])
    .map((entry) => state.images.findIndex((image) => entryIdentity(image) === entryIdentity(entry)))
    .filter((index) => index >= 0);
  const indexes = pagePrefetchPlan({
    visibleIndexes,
    itemCount: state.images.length,
    direction: state.pageDirection,
    ahead: PAGE_PREFETCH_AHEAD,
    behind: PAGE_PREFETCH_BEHIND,
  });
  const requests = await Promise.all(
    indexes.map(async (index) => {
      const entry = state.images[index];
      if (!entry?.address) return null;
      const groupIndex = pageGroupIndexForEntry(entry);
      const targetGroup = state.pageGroups[groupIndex];
      if (!targetGroup) return null;
      const infos = await Promise.all(targetGroup.entries.map(imageInfo));
      const layout = viewerSpreadLayout({
        mode: state.fitMode,
        pages: infos,
        viewportWidth: viewer.stage.clientWidth || window.innerWidth,
        viewportHeight: viewer.stage.clientHeight || window.innerHeight,
        devicePixelRatio: window.devicePixelRatio || 1,
        gap: targetGroup.entries.length > 1 ? state.spreadPageGapPx : 0,
      });
      const pageIndex = targetGroup.entries.findIndex(
        (page) => entryIdentity(page) === entryIdentity(entry)
      );
      return imageRequest(entry, infos[pageIndex], viewer.stage, {
        prefetch: true,
        layout: layout.pages[pageIndex],
      });
    })
  );
  if (
    state.viewer !== viewer ||
    currentPageGroup()?.entries.map(entryIdentity).join("\n") !== currentIdentity
  ) {
    return;
  }
  pageResourceCache.schedule(requests.filter(Boolean));
}

function imageRequest(
  entry,
  info,
  stage,
  {
    prefetch = false,
    layout = null,
    targetPxOverride = null,
    adjustmentPreview = null,
    previewRevision = null,
  } = {}
) {
  const dpr = window.devicePixelRatio || 1;
  if (entry.address) {
    const resolvedLayout = layout ?? viewerImageLayout({
        mode: state.fitMode,
        sourceWidth: info.width,
        sourceHeight: info.height,
        viewportWidth: stage.clientWidth || window.innerWidth,
        viewportHeight: stage.clientHeight || window.innerHeight,
        devicePixelRatio: dpr,
        maxRequestWidth: 8192,
      });
    const targetPx = targetPxOverride ?? containerPageTargetPx({
      requestWidth: resolvedLayout.requestWidth,
      sourceWidth: info.width,
      sourceHeight: info.height,
      minimum: 256,
      maximum: 8192,
    });
    const infoCacheKey = mediaImageInfoKey(entry.address);
    return {
      url: apiUrl(
        "/api/page",
        addressQueryParams(entry.address, {
          w: targetPx,
          ...(prefetch ? { prefetch: 1 } : {}),
          rev: previewRevision ?? state.pageRenderRevision,
          ...(adjustmentPreview
            ? { adjustment_preview: JSON.stringify(adjustmentPreview) }
            : {}),
        })
      ),
      cacheKey: adjustmentPreview
        ? null
        : `${infoCacheKey}\n${targetPx}\n${state.pageRenderRevision}`,
      width: targetPx,
      cssWidth: resolvedLayout.cssWidth,
      dpr,
      layout: resolvedLayout,
      fitMode: state.fitMode,
      dynamicInfo: true,
      infoCacheKey,
      containerInfoKey: mediaContainerInfoKey(entry.address),
      prefetch,
    };
  }
  const resolvedLayout = layout ?? viewerImageLayout({
      mode: state.fitMode,
      sourceWidth: info.width,
      sourceHeight: info.height,
      viewportWidth: stage.clientWidth || window.innerWidth,
      viewportHeight: stage.clientHeight || window.innerHeight,
      devicePixelRatio: dpr,
    });
  return {
    url: apiUrl("/api/image", {
      fav: state.favoriteId,
      path: entry.path,
      w: resolvedLayout.requestWidth,
    }),
    width: resolvedLayout.requestWidth,
    cssWidth: resolvedLayout.cssWidth,
    dpr,
    layout: resolvedLayout,
    fitMode: state.fitMode,
  };
}

function imageInfo(entry) {
  if (entry.address) {
    const key = mediaImageInfoKey(entry.address);
    if (state.imageInfoCache.has(key)) return state.imageInfoCache.get(key);
    const hint = state.containerImageInfoHints.get(mediaContainerInfoKey(entry.address));
    if (hint) return Promise.resolve({ ...hint, dynamic: true, estimated: true });
    return Promise.resolve({
      width: Math.max(1, window.innerWidth),
      height: Math.max(1, window.innerHeight),
      dynamic: true,
    });
  }
  const path = entry.path;
  const key = `${state.favoriteId}\n${path}\n${entry?.mtime ?? ""}\n${entry?.size ?? ""}`;
  if (!state.imageInfoCache.has(key)) {
    const pending = apiJson("/api/image-info", { fav: state.favoriteId, path }).catch(
      (error) => {
        state.imageInfoCache.delete(key);
        throw error;
      }
    );
    state.imageInfoCache.set(key, pending);
  }
  return state.imageInfoCache.get(key);
}

function mediaImageInfoKey(address) {
  return `media\n${addressIdentity(address)}`;
}

function mediaContainerInfoKey(address) {
  return `${address.favorite_id}\n${address.relative_path}`;
}

function rememberMediaImageInfo(request, info) {
  if (!info?.width || !info?.height) return;
  const resolved = { width: info.width, height: info.height };
  if (request.infoCacheKey) {
    state.imageInfoCache.set(request.infoCacheKey, Promise.resolve(resolved));
  }
  if (request.containerInfoKey) {
    state.containerImageInfoHints.set(request.containerInfoKey, resolved);
  }
}

function bindThumbnail(image, entry, tracker, cellWidth) {
  disposeThumbnailBinding(image);
  const generation = (Number(image._thumbnailGeneration) || 0) + 1;
  image._thumbnailGeneration = generation;
  image._thumbnailPath = thumbnailBindingKey(entry);
  image._thumbnailSettled = false;
  image.classList.remove("thumb-ready", "thumb-missing", "thumb-retry-exhausted");
  image.parentElement?.classList.remove("thumb-loaded");
  image.parentElement?.removeAttribute("data-retry-exhausted");
  image.parentElement?.removeAttribute("data-unavailable");
  const controller = new AbortController();
  image._thumbnailController = controller;
  const targetPx = clamp(
    Math.ceil(Math.max(1, Number(cellWidth) || 1) * (window.devicePixelRatio || 1)),
    32,
    4096
  );
  // VirtualGrid 自身が requestAnimationFrame 内でセルを materialize するため、
  // 次 frame まで thumbnail fetch を遅らせると、ラベルだけの初回 paint が必ず先行する。
  image._thumbnailStartFrame = requestAnimationFrame(() => {
    image._thumbnailStartFrame = 0;
    if (!controller.signal.aborted) {
      loadThumbnail(image, entry, tracker, generation, targetPx, controller.signal);
    }
  });
}

function setGridTileThumbnailVisible(tile, visible) {
  const binding = tile?._thumbnailBinding;
  if (!binding) return;
  const { image, entry, tracker, cellWidth } = binding;
  if (visible) {
    if (
      !image._thumbnailController &&
      !image._thumbnailStartFrame &&
      !image._thumbnailSettled
    ) {
      bindThumbnail(image, entry, tracker, cellWidth);
    }
    return;
  }
  if (image._thumbnailController || image._thumbnailStartFrame) {
    image._thumbnailGeneration = (Number(image._thumbnailGeneration) || 0) + 1;
    disposeThumbnailBinding(image);
  }
}

function thumbnailResponseIsCurrent(image, generation, path) {
  return thumbnailBindingMatches(
    image._thumbnailGeneration,
    image._thumbnailPath,
    generation,
    path
  );
}

function disposeThumbnailBinding(image) {
  cancelAnimationFrame(image._thumbnailStartFrame || 0);
  image._thumbnailStartFrame = 0;
  image._thumbnailController?.abort();
  image._thumbnailController = null;
  if (image._thumbnailObjectUrl) {
    URL.revokeObjectURL(image._thumbnailObjectUrl);
    image._thumbnailObjectUrl = null;
  }
  image.removeAttribute("src");
}

function disposeGridTile(tile) {
  const image = tile?.querySelector(".tile-preview img");
  if (!image) return;
  image._thumbnailGeneration = (Number(image._thumbnailGeneration) || 0) + 1;
  disposeThumbnailBinding(image);
}

function showThumbnailServiceNotice(message) {
  if (!state.thumbnailNotice) return;
  state.thumbnailNotice.textContent = message;
  state.thumbnailNotice.hidden = false;
}

function clearThumbnailServiceNotice() {
  if (!state.thumbnailNotice) return;
  state.thumbnailNotice.hidden = true;
  state.thumbnailNotice.textContent = "";
}

async function loadThumbnail(image, entry, tracker, generation, targetPx, signal) {
  const bindingKey = thumbnailBindingKey(entry);
  const url = apiUrl(
    "/api/thumb",
    addressQueryParams(thumbnailAddressForEntry(entry), { w: targetPx })
  );
  try {
    const result = await fetchThumbnailWithRetry(url, signal);
    const { response, detail, exhausted } = result;
    if (!response.ok) {
      if (
        response.status === 503 &&
        ["miv_not_running", "protocol_version_mismatch"].includes(detail.error)
      ) {
        showThumbnailServiceNotice(
          detail.message || "mIV 本体が起動していません。"
        );
      }
      if (!thumbnailResponseIsCurrent(image, generation, bindingKey)) return;
      if (response.status === 423) {
        image.parentElement?.setAttribute("data-unavailable", "パスワード保護");
      }
      image.classList.add("thumb-missing");
      image.classList.toggle("thumb-retry-exhausted", exhausted);
      if (exhausted) {
        image.parentElement?.setAttribute("data-retry-exhausted", "true");
      }
      image.parentElement?.classList.remove("thumb-loaded");
      tracker?.settled(bindingKey, { notFound: response.status === 404 });
      return;
    }
    const blob = await response.blob();
    const objectUrl = URL.createObjectURL(blob);
    if (!thumbnailResponseIsCurrent(image, generation, bindingKey)) {
      URL.revokeObjectURL(objectUrl);
      return;
    }
    if (image._thumbnailObjectUrl) URL.revokeObjectURL(image._thumbnailObjectUrl);
    image._thumbnailObjectUrl = objectUrl;
    image.src = objectUrl;
    await image.decode();
    await nextFrame();
    if (!thumbnailResponseIsCurrent(image, generation, bindingKey)) {
      URL.revokeObjectURL(objectUrl);
      if (image._thumbnailObjectUrl === objectUrl) image._thumbnailObjectUrl = null;
      return;
    }
    image.classList.remove("thumb-missing", "thumb-retry-exhausted");
    image.parentElement?.removeAttribute("data-retry-exhausted");
    image.parentElement?.removeAttribute("data-unavailable");
    image.classList.add("thumb-ready");
    image.parentElement?.classList.add("thumb-loaded");
    clearThumbnailServiceNotice();
    tracker?.settled(bindingKey);
  } catch (error) {
    if (error?.name === "AbortError") return;
    if (!thumbnailResponseIsCurrent(image, generation, bindingKey)) return;
    image.classList.remove("thumb-ready");
    image.classList.add("thumb-missing");
    image.classList.toggle("thumb-retry-exhausted", Boolean(error?.retryExhausted));
    if (error?.retryExhausted) {
      image.parentElement?.setAttribute("data-retry-exhausted", "true");
    }
    image.parentElement?.classList.remove("thumb-loaded");
    tracker?.settled(bindingKey);
    recordClientError("image_load_error", error, {
      resource: safeResourcePath(url),
    });
  } finally {
    if (
      thumbnailResponseIsCurrent(image, generation, bindingKey) &&
      image._thumbnailController?.signal === signal
    ) {
      image._thumbnailController = null;
      image._thumbnailSettled = true;
    }
  }
}

async function fetchThumbnailWithRetry(url, signal) {
  let retryCount = 0;
  let admissionWaitCount = 0;
  while (true) {
    let response;
    try {
      response = await thumbnailRequestLimiter.run(
        () =>
          observedFetch(url, {
            credentials: "same-origin",
            cache: "force-cache",
            signal,
          }),
        signal
      );
    } catch (error) {
      if (error?.name === "AbortError") throw error;
      const decision = thumbnailRetryDecision(0, "network_error", retryCount);
      if (!decision.retry) {
        error.retryExhausted = decision.exhausted;
        error.retryCount = retryCount;
        throw error;
      }
      enqueueTelemetry({
        type: "thumbnail_retry",
        status: 0,
        retry_count: retryCount + 1,
      });
      await abortableDelay(decision.delayMs, signal);
      if (decision.consumeRetryBudget) retryCount += 1;
      continue;
    }

    if (response.ok) {
      return { response, detail: {}, retryCount, exhausted: false };
    }
    const detail = await response.clone().json().catch(() => ({}));
    const decision = thumbnailRetryDecision(
      response.status,
      detail.error,
      retryCount
    );
    if (!decision.retry) {
      return {
        response,
        detail,
        retryCount,
        exhausted: decision.exhausted,
      };
    }
    if (!decision.consumeRetryBudget) admissionWaitCount += 1;
    enqueueTelemetry({
      type: "thumbnail_retry",
      status: response.status,
      error: detail.error,
      retry_count: retryCount + Number(decision.consumeRetryBudget),
      admission_wait_count: admissionWaitCount,
    });
    response.body?.cancel().catch(() => {});
    const retryAfterSeconds = Number(response.headers.get("Retry-After"));
    const retryAfterMs =
      Number.isFinite(retryAfterSeconds) && retryAfterSeconds > 0
        ? Math.min(10000, retryAfterSeconds * 1000)
        : 0;
    await abortableDelay(Math.max(decision.delayMs, retryAfterMs), signal);
    if (decision.consumeRetryBudget) retryCount += 1;
  }
}

function abortableDelay(delayMs, signal) {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      const error = new Error("Aborted");
      error.name = "AbortError";
      reject(error);
      return;
    }
    const timer = window.setTimeout(done, delayMs);
    function done() {
      signal?.removeEventListener("abort", aborted);
      resolve();
    }
    function aborted() {
      window.clearTimeout(timer);
      signal?.removeEventListener("abort", aborted);
      const error = new Error("Aborted");
      error.name = "AbortError";
      reject(error);
    }
    signal?.addEventListener("abort", aborted, { once: true });
  });
}

class VirtualGrid {
  constructor(
    scroller,
    space,
    windowElement,
    items,
    renderCell,
    onInitialItems,
    aspectHeightRatio,
    labelHeight
  ) {
    this.scroller = scroller;
    this.space = space;
    this.windowElement = windowElement;
    this.items = items;
    this.renderCell = renderCell;
    this.onInitialItems = onInitialItems;
    this.aspectHeightRatio = aspectHeightRatio;
    this.requestedLabelHeight = labelHeight;
    this.initialItemsReported = false;
    this.cells = new Map();
    this.columns = 1;
    this.rowHeight = 1;
    this.tileHeight = 1;
    this.previewHeight = 1;
    this.cellWidth = 1;
    this.labelHeight = 1;
    this.gap = 0;
    this.maxScrollOffset = 0;
    this.lastRange = "";
    this.frame = 0;
    this.snapTimer = 0;
    this.activePointers = new Map();
    this.pinch = null;
    this.blockClickUntil = 0;
    this.wheelPinchScale = 1;
    this.wheelPinchTimer = 0;
    this.onScroll = () => {
      this.schedule();
      this.scheduleRowSnap();
    };
    this.onScrollEnd = () => this.snapToRow();
    this.onWheel = (event) => this.handleWheel(event);
    this.onPointerDown = (event) => this.handlePointerDown(event);
    this.onPointerMove = (event) => this.handlePointerMove(event);
    this.onPointerEnd = (event) => this.handlePointerEnd(event);
    this.onTouchMove = (event) => {
      if (event.touches.length < 2) return;
      event.preventDefault();
    };
    this.onClickCapture = (event) => {
      if (performance.now() >= this.blockClickUntil) return;
      event.preventDefault();
      event.stopImmediatePropagation();
    };
    this.onNativeGesture = (event) => event.preventDefault();
    this.resizeObserver = new ResizeObserver(() => this.layout());
    this.scroller.addEventListener("scroll", this.onScroll, { passive: true });
    this.scroller.addEventListener("scrollend", this.onScrollEnd, {
      passive: true,
    });
    this.scroller.addEventListener("wheel", this.onWheel, { passive: false });
    this.scroller.addEventListener("pointerdown", this.onPointerDown, {
      passive: false,
    });
    this.scroller.addEventListener("pointermove", this.onPointerMove, {
      passive: false,
    });
    this.scroller.addEventListener("pointerup", this.onPointerEnd, {
      passive: true,
    });
    this.scroller.addEventListener("pointercancel", this.onPointerEnd, {
      passive: true,
    });
    this.scroller.addEventListener("touchmove", this.onTouchMove, {
      passive: false,
    });
    this.scroller.addEventListener("click", this.onClickCapture, {
      capture: true,
    });
    this.scroller.addEventListener("gesturestart", this.onNativeGesture, {
      passive: false,
    });
    this.scroller.addEventListener("gesturechange", this.onNativeGesture, {
      passive: false,
    });
    this.resizeObserver.observe(this.scroller);
    this.layout();
  }

  handlePointerDown(event) {
    if (event.pointerType === "mouse" && event.button !== 0) return;
    this.activePointers.set(event.pointerId, {
      pointerType: event.pointerType,
      x: event.clientX,
      y: event.clientY,
    });
    clearTimeout(this.snapTimer);
    this.snapTimer = 0;
    const touches = [...this.activePointers.entries()].filter(
      ([, pointer]) => pointer.pointerType === "touch"
    );
    if (touches.length !== 2) return;
    const [[firstId, first], [secondId, second]] = touches;
    this.pinch = {
      pointerIds: [firstId, secondId],
      distance: Math.max(1, distance(first, second)),
    };
    event.preventDefault();
  }

  handlePointerMove(event) {
    const previous = this.activePointers.get(event.pointerId);
    if (!previous) return;
    this.activePointers.set(event.pointerId, {
      ...previous,
      x: event.clientX,
      y: event.clientY,
    });
    if (!this.pinch) return;
    const [firstId, secondId] = this.pinch.pointerIds;
    const first = this.activePointers.get(firstId);
    const second = this.activePointers.get(secondId);
    if (!first || !second) return;

    event.preventDefault();
    const currentDistance = Math.max(1, distance(first, second));
    const nextColumns = gridColumnsAfterPinch(
      this.columns,
      currentDistance / this.pinch.distance
    );
    if (this.applyColumnOverride(nextColumns)) {
      this.pinch.distance = currentDistance;
    }
  }

  handlePointerEnd(event) {
    if (!this.activePointers.has(event.pointerId)) return;
    this.activePointers.delete(event.pointerId);
    if (this.pinch?.pointerIds.includes(event.pointerId)) {
      this.blockClickUntil = performance.now() + 300;
      this.pinch = null;
    }
    if (this.activePointers.size === 0) this.scheduleRowSnap();
  }

  applyColumnOverride(nextColumns) {
    if (nextColumns === this.columns) return false;
    const field = gridColumnOverrideFieldForViewport(
      this.scroller.clientWidth,
      this.scroller.clientHeight
    );
    const saved = saveLocalSettings({
      ...state.localSettings,
      [field]: nextColumns,
    });
    state.localSettings = saved.settings;
    state.localSettingsStorageAvailable = saved.saved;
    this.layout();
    return true;
  }

  layout() {
    const previousColumns = this.columns;
    const previousRowHeight = this.rowHeight;
    const anchorIndex =
      previousRowHeight > 1
        ? Math.floor(this.scroller.scrollTop / previousRowHeight) * previousColumns
        : 0;
    const layout = gridLayoutForWidth(
      this.scroller.clientWidth,
      this.aspectHeightRatio,
      this.requestedLabelHeight,
      gridColumnOverrideForViewport(
        this.scroller.clientWidth,
        this.scroller.clientHeight,
        state.localSettings
      )
    );
    if (
      layout.columns !== this.columns ||
      layout.rowPitch !== this.rowHeight
    ) {
      this.columns = layout.columns;
      this.rowHeight = layout.rowPitch;
      this.lastRange = "";
    }
    this.tileHeight = layout.tileHeight;
    this.previewHeight = layout.previewHeight;
    this.cellWidth = layout.cellWidth;
    this.labelHeight = layout.labelHeight;
    this.gap = layout.gap;
    const rows = Math.ceil(this.items.length / this.columns);
    const extent = gridScrollExtent(
      rows,
      this.rowHeight,
      this.scroller.clientHeight
    );
    this.maxScrollOffset = extent.maxOffset;
    this.space.style.height = `${extent.totalHeight}px`;
    this.windowElement.style.setProperty(
      "--grid-inline-inset",
      `${layout.inset}px`
    );
    this.windowElement.style.gap = `${layout.gap}px`;
    this.windowElement.style.setProperty(
      "--grid-preview-height",
      `${layout.previewHeight}px`
    );
    this.windowElement.style.setProperty(
      "--grid-label-height",
      `${layout.labelHeight}px`
    );
    this.windowElement.style.gridTemplateColumns = `repeat(${this.columns}, minmax(0, 1fr))`;
    this.windowElement.style.gridAutoRows = `${this.tileHeight}px`;
    if (
      layout.columns !== previousColumns ||
      layout.rowPitch !== previousRowHeight
    ) {
      const anchorRow = Math.floor(anchorIndex / this.columns);
      this.scroller.scrollTop = Math.min(
        this.maxScrollOffset,
        anchorRow * this.rowHeight
      );
    }
    this.schedule();
  }

  handleWheel(event) {
    if (event.ctrlKey || event.metaKey) {
      event.preventDefault();
      if (!Number.isFinite(event.deltaY) || event.deltaY === 0) return;
      clearTimeout(this.wheelPinchTimer);
      const delta = Math.max(-50, Math.min(50, event.deltaY));
      this.wheelPinchScale *= Math.exp(-delta * 0.01);
      const nextColumns = gridColumnsAfterPinch(
        this.columns,
        this.wheelPinchScale
      );
      if (this.applyColumnOverride(nextColumns)) this.wheelPinchScale = 1;
      this.wheelPinchTimer = window.setTimeout(() => {
        this.wheelPinchScale = 1;
        this.wheelPinchTimer = 0;
      }, 180);
      return;
    }
    if (!Number.isFinite(event.deltaY) || event.deltaY === 0) {
      return;
    }
    event.preventDefault();
    clearTimeout(this.snapTimer);
    this.snapTimer = 0;
    const current = snappedGridOffset(
      this.scroller.scrollTop,
      this.rowHeight,
      this.maxScrollOffset
    );
    const target = current + Math.sign(event.deltaY) * this.rowHeight;
    this.scroller.scrollTop = Math.max(
      0,
      Math.min(this.maxScrollOffset, target)
    );
    this.schedule();
  }

  scheduleRowSnap() {
    if (this.activePointers.size > 0) return;
    clearTimeout(this.snapTimer);
    this.snapTimer = window.setTimeout(() => this.snapToRow(), 140);
  }

  snapToRow() {
    if (this.activePointers.size > 0) return;
    clearTimeout(this.snapTimer);
    this.snapTimer = 0;
    const snapped = snappedGridOffset(
      this.scroller.scrollTop,
      this.rowHeight,
      this.maxScrollOffset
    );
    if (Math.abs(this.scroller.scrollTop - snapped) > 0.5) {
      this.scroller.scrollTop = snapped;
    }
    this.schedule();
  }

  schedule() {
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.render();
    });
  }

  render() {
    const overscan = 3;
    const visibleRows = Math.ceil(this.scroller.clientHeight / this.rowHeight);
    const firstVisibleRow = Math.max(
      0,
      Math.floor(this.scroller.scrollTop / this.rowHeight)
    );
    const totalRows = Math.ceil(this.items.length / this.columns);
    const endVisibleRow = Math.min(
      totalRows,
      Math.ceil(
        (this.scroller.scrollTop + this.scroller.clientHeight) / this.rowHeight
      )
    );
    const firstRow = Math.max(0, firstVisibleRow - overscan);
    const endRow = Math.min(totalRows, firstRow + visibleRows + overscan * 2);
    const startIndex = firstRow * this.columns;
    const endIndex = Math.min(this.items.length, endRow * this.columns);
    const visibleStartIndex = firstVisibleRow * this.columns;
    const visibleEndIndex = Math.min(this.items.length, endVisibleRow * this.columns);
    const range = `${startIndex}:${endIndex}:${visibleStartIndex}:${visibleEndIndex}:${this.columns}`;
    if (range === this.lastRange) return;
    this.lastRange = range;
    if (!this.initialItemsReported) {
      this.initialItemsReported = true;
      this.onInitialItems?.(this.items.slice(visibleStartIndex, visibleEndIndex));
    }
    this.windowElement.style.top = `${firstRow * this.rowHeight}px`;
    const fragment = document.createDocumentFragment();
    for (let index = startIndex; index < endIndex; index += 1) {
      let cell = this.cells.get(index);
      if (!cell) {
        cell = this.renderCell(this.items[index], index, this.cellWidth);
        this.cells.set(index, cell);
      }
      setGridTileThumbnailVisible(
        cell,
        index >= visibleStartIndex && index < visibleEndIndex
      );
      fragment.append(cell);
    }
    for (const [index, cell] of this.cells) {
      if (index < startIndex || index >= endIndex) {
        setGridTileThumbnailVisible(cell, false);
      }
    }
    this.windowElement.replaceChildren(fragment);
    const cacheLimit = Math.max(128, (endIndex - startIndex) * 4);
    if (this.cells.size > cacheLimit) {
      const center = (startIndex + endIndex) / 2;
      const candidates = [...this.cells.keys()]
        .filter((index) => index < startIndex || index >= endIndex)
        .sort((left, right) => Math.abs(right - center) - Math.abs(left - center));
      while (this.cells.size > cacheLimit && candidates.length) {
        const index = candidates.shift();
        disposeGridTile(this.cells.get(index));
        this.cells.delete(index);
      }
    }
  }

  visibleRowCount() {
    return Math.max(1, Math.floor(this.scroller.clientHeight / this.rowHeight));
  }

  scrollTop() {
    return this.scroller.scrollTop;
  }

  restoreScrollTop(scrollTop) {
    this.scroller.scrollTop = Math.max(
      0,
      Math.min(this.maxScrollOffset, Number(scrollTop) || 0)
    );
    this.lastRange = '';
    this.schedule();
  }

  focusIndex(index, shouldFocus) {
    const row = Math.floor(index / this.columns);
    const top = row * this.rowHeight;
    const bottom = top + this.rowHeight;
    let scrolled = false;
    if (top < this.scroller.scrollTop) {
      this.scroller.scrollTop = top;
      scrolled = true;
    } else if (bottom > this.scroller.scrollTop + this.scroller.clientHeight) {
      const firstFullyVisibleRow = Math.ceil(
        (bottom - this.scroller.clientHeight) / this.rowHeight
      );
      this.scroller.scrollTop = Math.min(
        this.maxScrollOffset,
        Math.max(0, firstFullyVisibleRow * this.rowHeight)
      );
      scrolled = true;
    }
    let tile = this.windowElement.querySelector(`[data-entry-index="${index}"]`);
    if (!tile || scrolled) {
      this.lastRange = "";
      this.render();
      tile = this.windowElement.querySelector(`[data-entry-index="${index}"]`);
    }
    for (const tile of this.windowElement.querySelectorAll(".grid-active")) {
      tile.classList.remove("grid-active");
    }
    tile?.classList.add("grid-active");
    if (shouldFocus) tile?.focus({ preventScroll: true });
  }

  destroy() {
    cancelAnimationFrame(this.frame);
    clearTimeout(this.snapTimer);
    clearTimeout(this.wheelPinchTimer);
    this.scroller.removeEventListener("scroll", this.onScroll);
    this.scroller.removeEventListener("scrollend", this.onScrollEnd);
    this.scroller.removeEventListener("wheel", this.onWheel);
    this.scroller.removeEventListener("pointerdown", this.onPointerDown);
    this.scroller.removeEventListener("pointermove", this.onPointerMove);
    this.scroller.removeEventListener("pointerup", this.onPointerEnd);
    this.scroller.removeEventListener("pointercancel", this.onPointerEnd);
    this.scroller.removeEventListener("touchmove", this.onTouchMove);
    this.scroller.removeEventListener("click", this.onClickCapture, true);
    this.scroller.removeEventListener("gesturestart", this.onNativeGesture);
    this.scroller.removeEventListener("gesturechange", this.onNativeGesture);
    this.resizeObserver.disconnect();
    this.activePointers.clear();
    this.pinch = null;
    for (const cell of this.cells.values()) disposeGridTile(cell);
    this.cells.clear();
  }
}

class ThumbnailGridTracker {
  constructor(startedAt, folderEntryCount) {
    this.startedAt = startedAt;
    this.folderEntryCount = folderEntryCount;
    this.pending = new Set();
    this.expected = 0;
    this.notFoundCount = 0;
    this.completed = false;
    this.destroyed = false;
  }

  begin(items) {
    if (this.destroyed || this.expected) return;
    for (const entry of items) {
      this.pending.add(thumbnailBindingKey(entry));
    }
    this.expected = this.pending.size;
    if (!this.expected) this.finish();
  }

  settled(path, { notFound = false } = {}) {
    if (this.destroyed || this.completed || !this.pending.delete(path)) return;
    if (notFound) this.notFoundCount += 1;
    if (!this.pending.size) this.finish();
  }

  finish() {
    if (this.destroyed || this.completed) return;
    this.completed = true;
    const event = {
      type: "thumbnail_grid",
      duration_ms: roundMs(performance.now() - this.startedAt),
      rendered_count: this.expected,
      folder_entry_count: this.folderEntryCount,
      not_found_count: this.notFoundCount,
    };
    enqueueTelemetry(event);
    hudState.lastGrid = event;
    updateHud();
  }

  destroy() {
    this.destroyed = true;
  }
}

function createMenuButton(label, extraClass = "icon-button") {
  const button = textElement("button", "☰", `${extraClass} menu-trigger`);
  button.type = "button";
  button.setAttribute("aria-label", label);
  button.setAttribute("aria-haspopup", "dialog");
  button.addEventListener("click", (event) => {
    event.stopPropagation();
    menuCommand(event, CommandName.TOGGLE_MENU);
  });
  return button;
}

const MenuPageAction = Object.freeze({
  BACK: "menu_page_back",
  RATING: "menu_page_rating",
  DISPLAY: "menu_page_display",
  SPREAD: "menu_page_spread",
  POSITION: "menu_page_position",
});

export const VIEWER_PANEL_TABS = Object.freeze([
  Object.freeze({ id: "functions", label: "機能" }),
  Object.freeze({ id: "adjustment", label: "画像補正" }),
  Object.freeze({ id: "view_trim", label: "表示トリム" }),
  Object.freeze({ id: "bookmarks", label: "ブックマーク" }),
]);

export const VIEWER_MENU_MAX_ACTIONS = 11;

export function viewerMenuDefinitions({ hasContainer, barsVisible }) {
  const back = [MenuPageAction.BACK, "操作メニューへ戻る", "戻る"];
  const mainActions = [
    [CommandName.TOGGLE_BOOKMARK, "ブックマークを読み込み中…", "現在のページ"],
    [MenuPageAction.RATING, "レーティング", "★を選択", { menuPage: "rating" }],
    [MenuPageAction.DISPLAY, "表示サイズ", "フィット / 原寸", { menuPage: "display" }],
    ...(hasContainer
      ? [[MenuPageAction.SPREAD, "見開き設定", "1〜5", { menuPage: "spread" }]]
      : []),
    [
      CommandName.TOGGLE_VIEWER_BARS,
      barsVisible ? "上下バーを隠す" : "上下バーを表示",
      "中央タップ",
    ],
    [CommandName.TOGGLE_FULLSCREEN, "全画面表示", "F11"],
    [MenuPageAction.POSITION, "ページ位置", "先頭 / 最後", { menuPage: "position" }],
    [CommandName.BACK, "一覧へ戻る", "Backspace / Enter / Esc"],
    [CommandName.OPEN_LOCAL_SETTINGS, "端末の設定", "メニュー"],
    [CommandName.OPEN_GESTURE_HELP, "操作方法を見る", "メニュー"],
    [CommandName.RELOAD_APP, "再読み込み", "メニュー"],
  ];
  return {
    main: {
      title: "機能",
      actions: mainActions,
      shortcuts: [
        ["前 / 次", "← ↑ PageUp / → ↓ PageDown"],
        ["ズーム", "+ / −"],
        ["表示モード", "0 (全体 → 幅 → 原寸)"],
        ...(hasContainer
          ? [["見開き", "1〜5 (単 / LTR / LTR表紙 / RTL / RTL表紙)"]]
          : []),
        ["操作メニュー", "?"],
        ["先頭 / 最後", "Home / End"],
        ["一覧へ戻る", "Backspace / Enter / Esc"],
        ["全画面", "F11"],
      ],
    },
    rating: {
      title: "レーティング",
      actions: [
        back,
        ...[0, 1, 2, 3, 4, 5].map((stars) => [
          CommandName.SET_RATING,
          stars === 0 ? "レーティングを解除" : `★${stars}`,
          stars === 0 ? "0" : `★${stars}`,
          { stars },
        ]),
      ],
    },
    display: {
      title: "表示サイズ",
      actions: [
        back,
        [CommandName.ZOOM_RESET, "ズームを戻す", "メニュー"],
        [CommandName.FIT_PAGE, "全体フィット", "0 で切替"],
        [CommandName.FIT_WIDTH, "幅フィット", "0 で切替"],
        [CommandName.FIT_ORIGINAL, "原寸 (100%)", "0 で切替"],
      ],
    },
    spread: {
      title: "見開き設定",
      actions: [
        back,
        [CommandName.SPREAD_SINGLE, "1ページ表示", "1"],
        [CommandName.SPREAD_LTR, "見開き 左→右", "2"],
        [CommandName.SPREAD_LTR_COVER, "見開き 左→右 (表紙あり)", "3"],
        [CommandName.SPREAD_RTL, "見開き 右→左", "4"],
        [CommandName.SPREAD_RTL_COVER, "見開き 右→左 (表紙あり)", "5"],
      ],
    },
    position: {
      title: "ページ位置",
      actions: [
        back,
        [CommandName.FIRST_PAGE, "先頭の画像", "Home"],
        [CommandName.LAST_PAGE, "最後の画像", "End"],
      ],
    },
  };
}

function menuDefinition(context, page = "main") {
  if (context === "viewer") {
    const definitions = viewerMenuDefinitions({
      hasContainer: Boolean(state.container),
      barsVisible: state.viewerBarsVisible,
    });
    return definitions[page] ?? definitions.main;
  }
  if (context === "grid") {
    return {
      title: "一覧の操作",
      actions: [
        [CommandName.PARENT_FOLDER, "親フォルダへ", "Backspace / Alt+↑"],
        [CommandName.BACK, "履歴を戻る", "Alt+← / Esc"],
        [CommandName.FORWARD, "履歴を進む", "Alt+→"],
        [CommandName.GRID_FIRST, "先頭へ", "Home"],
        [CommandName.GRID_LAST, "末尾へ", "End"],
        [CommandName.TOGGLE_FULLSCREEN, "全画面表示", "F11"],
        [CommandName.OPEN_LOCAL_SETTINGS, "端末の設定", "メニュー"],
        [CommandName.RELOAD_APP, "再読み込み", "メニュー"],
      ],
      shortcuts: [
        ["項目を移動", "← ↑ → ↓"],
        ["選択項目を開く", "Enter"],
        ["親フォルダ", "Backspace / Alt+↑"],
        ["履歴", "Alt+← / Alt+→ / Esc"],
        ["1画面移動", "PageUp / PageDown"],
        ["先頭 / 末尾", "Home / End"],
        ["操作メニュー", "?"],
        ["全画面", "F11"],
      ],
    };
  }
  return {
    title: "操作",
    actions: [
      [CommandName.TOGGLE_FULLSCREEN, "全画面表示", "F11"],
      [CommandName.OPEN_LOCAL_SETTINGS, "端末の設定", "メニュー"],
      [CommandName.RELOAD_APP, "再読み込み", "メニュー"],
    ],
    shortcuts: [
      ["操作メニュー", "?"],
      ["全画面", "F11"],
    ],
  };
}

const REMOTE_ADJUSTMENT_CONTROLS = Object.freeze([
  ["brightness", "明るさ", -100, 100, 1, false],
  ["contrast", "コントラスト", -100, 100, 1, false],
  ["gamma", "ガンマ", 0.2, 5, 0.01, true],
  ["saturation", "彩度", -100, 100, 1, false],
  ["temperature", "色温度", -100, 100, 1, false],
  ["black_point", "黒点", 0, 254, 1, false],
  ["white_point", "白点", 1, 255, 1, false],
  ["midtone", "中間点", 0.1, 10, 0.01, true],
]);

const DEFAULT_REMOTE_COLORIZE_CONTROL_POINTS = Object.freeze([
  Object.freeze({ color: [0, 0, 0], strength: 3 }),
  Object.freeze({ color: [75, 0, 130], strength: 1 }),
  Object.freeze({ color: [205, 92, 92], strength: 1 }),
  Object.freeze({ color: [245, 222, 179], strength: 1 }),
  Object.freeze({ color: [240, 248, 255], strength: 1 }),
]);

const DEFAULT_REMOTE_COLORIZE_VALUES = Object.freeze({
  mode: "disabled",
  mono_tolerance: 12,
  palette: "legacy4_color",
  control_points: DEFAULT_REMOTE_COLORIZE_CONTROL_POINTS,
  luminance_weight: 100,
  density_normalization_strength: 0,
  tone_method: "off",
  tone_radius: 1,
  tone_strength: 100,
});

const DEFAULT_REMOTE_ADJUSTMENT_VALUES = Object.freeze({
  brightness: 0,
  contrast: 0,
  gamma: 1,
  saturation: 0,
  temperature: 0,
  black_point: 0,
  white_point: 255,
  midtone: 1,
  auto_mode: null,
  colorize: DEFAULT_REMOTE_COLORIZE_VALUES,
});

function finiteNumber(value, fallback) {
  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

export function normalizeRemoteColorizeParams(values = {}) {
  const source = { ...DEFAULT_REMOTE_COLORIZE_VALUES, ...values };
  const suppliedPoints = Array.isArray(source.control_points)
    ? source.control_points.slice(0, 10)
    : [];
  const pointSource = suppliedPoints.length >= 2
    ? suppliedPoints
    : DEFAULT_REMOTE_COLORIZE_CONTROL_POINTS;
  return {
    mode: ["disabled", "monochrome_only", "all_images"].includes(source.mode)
      ? source.mode
      : "disabled",
    mono_tolerance: clamp(Math.round(finiteNumber(source.mono_tolerance, 12)), 1, 64),
    palette: ["legacy4_color", "legacy_skin", "custom"].includes(source.palette)
      ? source.palette
      : "legacy4_color",
    control_points: pointSource.map((point) => ({
      color: [0, 1, 2].map((index) => clamp(
        Math.round(finiteNumber(point?.color?.[index], 0)),
        0,
        255
      )),
      strength: clamp(finiteNumber(point?.strength, 1), 0, 10),
    })),
    luminance_weight: clamp(
      Math.round(finiteNumber(source.luminance_weight, 100)),
      0,
      100
    ),
    density_normalization_strength: clamp(
      Math.round(finiteNumber(source.density_normalization_strength, 0)),
      0,
      100
    ),
    tone_method: ["off", "fast", "local_mean", "gaussian"].includes(source.tone_method)
      ? source.tone_method
      : "off",
    tone_radius: clamp(finiteNumber(source.tone_radius, 1), 0.1, 4),
    tone_strength: clamp(Math.round(finiteNumber(source.tone_strength, 100)), 0, 100),
  };
}

export function normalizeRemoteAdjustmentValues(values = {}) {
  const source = { ...DEFAULT_REMOTE_ADJUSTMENT_VALUES, ...values };
  const ai = source.ai && typeof source.ai === "object"
    ? {
        upscale_model: typeof source.ai.upscale_model === "string"
          ? source.ai.upscale_model
          : null,
        denoise_model: typeof source.ai.denoise_model === "string"
          ? source.ai.denoise_model
          : null,
      }
    : null;
  return {
    brightness: Number(source.brightness) || 0,
    contrast: Number(source.contrast) || 0,
    gamma: Number(source.gamma) || 1,
    saturation: Number(source.saturation) || 0,
    temperature: Number(source.temperature) || 0,
    black_point: clamp(Math.round(Number(source.black_point) || 0), 0, 254),
    white_point: clamp(Math.round(Number(source.white_point) || 0), 1, 255),
    midtone: Number(source.midtone) || 1,
    auto_mode: ["auto", "manga_cleanup"].includes(source.auto_mode)
      ? source.auto_mode
      : null,
    colorize: normalizeRemoteColorizeParams(source.colorize),
    ai,
  };
}

class ViewerAdjustmentPanel {
  constructor() {
    this.root = element("div", "viewer-adjustment-panel");
    this.targetIndex = 0;
    this.groupIdentity = "";
    this.scope = "standard";
    this.values = normalizeRemoteAdjustmentValues();
    this.serverState = null;
    this.refreshSequence = 0;
    this.previewEpoch = 0;
    this.previewSequence = 0;
    this.commitSequence = 0;
    this.writeTail = Promise.resolve();
    this.dirty = false;
    this.disabled = false;
    this.controls = new Map();
    this.colorizeControls = new Map();
    this.colorizePresetSlots = [null, null, null, null];
    this.previewQueue = new LatestOnlyTaskQueue(
      (job) => this.runPreview(job),
      (error) => this.showError(error)
    );
    this.build();
  }

  build() {
    this.targetRow = element("div", "adjustment-target-row");
    this.targetRow.append(textElement("span", "対象", "adjustment-section-label"));
    this.targetButtons = ["左", "右"].map((label, index) => {
      const button = textElement("button", label, "adjustment-target-button");
      button.type = "button";
      button.addEventListener("click", () => {
        if (this.targetIndex === index) return;
        this.targetIndex = index;
        this.syncTargetButtons();
        this.refresh().catch((error) => this.showError(error));
      });
      this.targetRow.append(button);
      return button;
    });

    this.scopeFieldset = element("fieldset", "adjustment-scope");
    this.scopeFieldset.append(textElement("legend", "保存先"));
    this.scopeInputs = new Map();
    const scopeGroupName = `remote-adjustment-scope-${Math.random().toString(36).slice(2)}`;
    for (const [scope, label] of [["standard", "標準"], ["page", "このページ"]]) {
      const wrapper = element("label", "adjustment-scope-option");
      const input = document.createElement("input");
      input.type = "radio";
      input.name = scopeGroupName;
      input.value = scope;
      input.addEventListener("change", () => {
        if (!input.checked) return;
        this.changeScope(scope).catch((error) => this.showError(error));
      });
      const text = textElement("span", label);
      wrapper.append(input, text);
      this.scopeFieldset.append(wrapper);
      this.scopeInputs.set(scope, { input, text });
    }

    const autoRow = element("label", "adjustment-auto-row");
    autoRow.append(textElement("span", "自動補正"));
    this.autoSelect = document.createElement("select");
    for (const [value, label] of [
      ["", "なし"],
      ["auto", "自動レベル"],
      ["manga_cleanup", "モノクロ漫画補正"],
    ]) {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      this.autoSelect.append(option);
    }
    this.autoSelect.addEventListener("change", () => {
      this.values.auto_mode = this.autoSelect.value || null;
      this.setDisabled(false);
      this.commitCurrent().catch((error) => this.showError(error));
    });
    autoRow.append(this.autoSelect);

    this.sliderList = element("div", "adjustment-slider-list");
    const sliderGroupId = `remote-adjustment-${Math.random().toString(36).slice(2)}`;
    for (const [key, label, min, max, step, logarithmic] of REMOTE_ADJUSTMENT_CONTROLS) {
      const defaultValue = DEFAULT_REMOTE_ADJUSTMENT_VALUES[key];
      const row = element("div", "adjustment-slider-row");
      const heading = element("div", "adjustment-slider-heading");
      const sliderLabel = textElement("label", label);
      const output = textElement("output", "");
      const headingActions = element("span", "adjustment-slider-heading-actions");
      const resetSlot = element("span", "adjustment-slider-reset-slot");
      const resetButton = textElement("button", "↩", "adjustment-slider-reset");
      resetButton.type = "button";
      resetButton.hidden = true;
      resetButton.title = "デフォルトに戻す";
      resetButton.setAttribute("aria-label", `${label}をデフォルトに戻す`);
      resetButton.addEventListener("pointerdown", (event) => event.stopPropagation());
      resetButton.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopPropagation();
        if (resetButton.hidden || resetButton.disabled) return;
        this.values[key] = defaultValue;
        this.dirty = false;
        this.syncControl(key);
        this.commitCurrent().catch((error) => this.showError(error));
      });
      resetSlot.append(resetButton);
      headingActions.append(output, resetSlot);
      heading.append(sliderLabel, headingActions);
      const input = document.createElement("input");
      input.type = "range";
      input.id = `${sliderGroupId}-${key}`;
      sliderLabel.htmlFor = input.id;
      // The native range owns focus, keyboard input and accessibility. Its value is the
      // normalized handle position so logarithmic controls also paint in the right place.
      input.min = "0";
      input.max = "1";
      input.step = "any";
      input.setAttribute("aria-valuemin", String(min));
      input.setAttribute("aria-valuemax", String(max));
      let pointerDrag = null;
      const applyValue = (raw) => {
        if (!Number.isFinite(raw)) return false;
        const value = rangeValueFromNormalized({
          normalized: rangeValueToNormalized({
            value: raw,
            min,
            max,
            logarithmic,
          }),
          min,
          max,
          step,
          logarithmic,
        });
        if (this.values[key] === value) return false;
        this.values[key] = value;
        this.dirty = true;
        this.syncControl(key);
        this.queuePreview();
        return true;
      };
      const applyNormalizedPosition = (position) => {
        const changed = applyValue(rangeValueFromNormalized({
          normalized: position,
          min,
          max,
          step,
          logarithmic,
        }));
        // A sub-step native move must not leave the painted handle detached from the
        // unchanged actual value.
        if (!changed) this.syncControl(key);
        return changed;
      };
      input.addEventListener("input", () => {
        // A native range must remain the keyboard/ARIA owner, but native track dragging is
        // suppressed while the relative pointer gesture owns this control.
        if (pointerDrag) {
          this.syncControl(key);
          return;
        }
        applyNormalizedPosition(Number(input.value));
      });
      const finish = (event) => {
        event.stopPropagation();
        if (!this.dirty) return;
        this.dirty = false;
        this.commitCurrent().catch((error) => this.showError(error));
      };
      const keyboardKeys = new Set([
        "ArrowLeft",
        "ArrowDown",
        "ArrowRight",
        "ArrowUp",
        "PageDown",
        "PageUp",
        "Home",
        "End",
      ]);
      let keyboardAdjusting = false;
      input.addEventListener("keydown", (event) => {
        if (input.disabled || !keyboardKeys.has(event.key)) return;
        event.preventDefault();
        event.stopPropagation();
        const current = Number(this.values[key]);
        let next = current;
        if (event.key === "Home") next = min;
        else if (event.key === "End") next = max;
        else if (event.key === "PageDown") next -= (max - min) / 10;
        else if (event.key === "PageUp") next += (max - min) / 10;
        else if (["ArrowLeft", "ArrowDown"].includes(event.key)) next -= step;
        else next += step;
        keyboardAdjusting = applyValue(next) || keyboardAdjusting;
      });
      input.addEventListener("keyup", (event) => {
        if (!keyboardKeys.has(event.key)) return;
        event.stopPropagation();
        if (!keyboardAdjusting) return;
        keyboardAdjusting = false;
        finish(event);
      });
      input.addEventListener("blur", (event) => {
        if (!keyboardAdjusting) return;
        keyboardAdjusting = false;
        finish(event);
      });
      input.addEventListener("pointerdown", (event) => {
        event.stopPropagation();
        if (input.disabled || event.isPrimary === false) return;
        if (typeof event.button === "number" && event.button !== 0) return;
        if (event.cancelable) event.preventDefault();
        pointerDrag = {
          pointerId: event.pointerId,
          startClientX: event.clientX,
          startValue: Number(this.values[key]),
          trackWidth: input.getBoundingClientRect().width,
        };
        input.focus({ preventScroll: true });
        try {
          input.setPointerCapture(event.pointerId);
        } catch (_error) {
          // Pointer capture can fail when the browser has already cancelled the pointer.
        }
      });
      input.addEventListener("pointermove", (event) => {
        if (!pointerDrag || pointerDrag.pointerId !== event.pointerId) return;
        event.stopPropagation();
        if (event.cancelable) event.preventDefault();
        applyValue(relativeRangeDragValue({
          startValue: pointerDrag.startValue,
          startClientX: pointerDrag.startClientX,
          currentClientX: event.clientX,
          trackWidth: pointerDrag.trackWidth,
          min,
          max,
          step,
          logarithmic,
        }));
      });
      const finishPointer = (event) => {
        if (pointerDrag && pointerDrag.pointerId !== event.pointerId) {
          event.stopPropagation();
          return;
        }
        if (pointerDrag) {
          if (event.cancelable) event.preventDefault();
          try {
            if (input.hasPointerCapture(event.pointerId)) {
              input.releasePointerCapture(event.pointerId);
            }
          } catch (_error) {
            // The browser may release capture before dispatching pointercancel.
          }
          pointerDrag = null;
        }
        finish(event);
      };
      input.addEventListener("pointerup", finishPointer);
      input.addEventListener("pointercancel", finishPointer);
      input.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopPropagation();
      });
      for (const eventName of ["touchend", "touchcancel", "change"]) {
        input.addEventListener(eventName, finish);
      }
      row.append(heading, input);
      this.sliderList.append(row);
      this.controls.set(key, {
        input,
        output,
        min,
        max,
        step,
        logarithmic,
        defaultValue,
        resetButton,
      });
    }

    this.colorizeSection = element("section", "adjustment-colorize");
    this.colorizeSection.append(textElement("h3", "カラー化"));

    const modeRow = element("label", "adjustment-option-row");
    modeRow.append(textElement("span", "適用対象"));
    this.colorizeModeSelect = document.createElement("select");
    for (const [value, label] of [
      ["disabled", "OFF"],
      ["monochrome_only", "モノクロ系画像だけ"],
      ["all_images", "すべての画像"],
    ]) {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      this.colorizeModeSelect.append(option);
    }
    this.colorizeModeSelect.addEventListener("change", () => {
      this.values.colorize.mode = this.colorizeModeSelect.value;
      this.syncColorizeControls();
      this.setDisabled(false);
      this.commitCurrent().catch((error) => this.showError(error));
    });
    modeRow.append(this.colorizeModeSelect);
    this.colorizeSection.append(modeRow);

    this.colorizeSettings = element("div", "adjustment-colorize-settings");
    this.colorizeSliderList = element("div", "adjustment-slider-list");
    this.addColorizeSlider(
      this.colorizeSliderList,
      "mono_tolerance",
      "モノクロ判定の許容値",
      1,
      64,
      1,
      () => this.values.colorize.mode === "monochrome_only"
    );
    this.addColorizeSlider(
      this.colorizeSliderList,
      "density_normalization_strength",
      "濃さを整える",
      0,
      100,
      1
    );

    const paletteRow = element("label", "adjustment-option-row");
    paletteRow.append(textElement("span", "パレット"));
    this.colorizePaletteSelect = document.createElement("select");
    for (const [value, label] of [
      ["legacy4_color", "4色刷り（従来互換）"],
      ["legacy_skin", "肌色（従来互換）"],
      ["custom", "カスタム"],
    ]) {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      this.colorizePaletteSelect.append(option);
    }
    this.colorizePaletteSelect.addEventListener("change", () => {
      this.values.colorize.palette = this.colorizePaletteSelect.value;
      this.syncColorizeControls();
      this.commitCurrent().catch((error) => this.showError(error));
    });
    paletteRow.append(this.colorizePaletteSelect);
    this.colorizeSettings.append(paletteRow);
    this.customPaletteNote = textElement(
      "p",
      "カスタムの制御点は PC 版で編集できます。ここでは保存済みの配色をそのまま使います。",
      "adjustment-colorize-note"
    );
    this.colorizeSettings.append(this.customPaletteNote);

    this.addColorizeSlider(
      this.colorizeSliderList,
      "luminance_weight",
      "元画像の明るさを保持",
      0,
      100,
      1
    );

    const toneRow = element("label", "adjustment-option-row");
    toneRow.append(textElement("span", "トーン密度"));
    this.colorizeToneSelect = document.createElement("select");
    for (const [value, label] of [
      ["off", "OFF（画素の輝度をそのまま使用）"],
      ["fast", "高速（縮小平均）"],
      ["local_mean", "弱（局所平均）"],
      ["gaussian", "強（ガウシアン）"],
    ]) {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      this.colorizeToneSelect.append(option);
    }
    this.colorizeToneSelect.addEventListener("change", () => {
      this.values.colorize.tone_method = this.colorizeToneSelect.value;
      this.syncColorizeControls();
      this.commitCurrent().catch((error) => this.showError(error));
    });
    toneRow.append(this.colorizeToneSelect);
    this.colorizeSettings.append(toneRow);
    this.colorizeToneDescription = textElement(
      "p",
      "",
      "adjustment-colorize-note"
    );
    this.colorizeSettings.append(this.colorizeToneDescription);

    this.addColorizeSlider(
      this.colorizeSliderList,
      "tone_radius",
      "検出スケール",
      0.1,
      4,
      0.1,
      () => this.values.colorize.tone_method !== "off"
    );
    this.addColorizeSlider(
      this.colorizeSliderList,
      "tone_strength",
      "トーン密度の強さ",
      0,
      100,
      1,
      () => this.values.colorize.tone_method !== "off"
    );
    this.colorizeSettings.append(this.colorizeSliderList);
    this.colorizeSection.append(this.colorizeSettings);

    this.colorizePresetSection = element("div", "adjustment-colorize-presets");
    this.colorizePresetSection.append(
      textElement("span", "カラー化設定保存スロット", "adjustment-section-label")
    );
    const slotButtons = element("div", "adjustment-colorize-preset-buttons");
    this.colorizePresetButtons = Array.from({ length: 4 }, (_, index) => {
      const button = textElement("button", String(index + 1));
      button.type = "button";
      button.title = `スロット${index + 1}を読み込む`;
      button.addEventListener("click", () => {
        const preset = this.colorizePresetSlots[index];
        if (!preset || button.disabled) return;
        this.values.colorize = normalizeRemoteColorizeParams(preset);
        this.dirty = false;
        this.syncColorizeControls();
        this.commitCurrent().catch((error) => this.showError(error));
      });
      slotButtons.append(button);
      return button;
    });
    this.colorizePresetSection.append(
      slotButtons,
      textElement(
        "small",
        "読み込みのみ。スロットへの保存とカスタム配色の編集は PC 版で行います。"
      )
    );
    this.colorizeSection.append(this.colorizePresetSection);

    this.aiSection = element("section", "adjustment-ai");
    this.aiSection.append(textElement("h3", "AI 処理"));
    this.aiSelects = new Map();
    for (const [key, label] of [
      ["upscale_model", "AI アップスケール"],
      ["denoise_model", "AI デノイズ"],
    ]) {
      const row = element("label", "adjustment-option-row");
      row.append(textElement("span", label));
      const select = document.createElement("select");
      select.addEventListener("change", () => {
        this.values.ai ??= { upscale_model: null, denoise_model: null };
        this.values.ai[key] = select.value || null;
        this.commitCurrent().catch((error) => this.showError(error));
      });
      row.append(select);
      this.aiSection.append(row);
      this.aiSelects.set(key, select);
    }
    this.aiAvailability = textElement("p", "", "adjustment-colorize-note");
    this.aiSection.append(this.aiAvailability);
    this.resetButton = textElement("button", "即時補正をリセット", "adjustment-reset");
    this.resetButton.type = "button";
    this.resetButton.addEventListener("click", () => {
      const colorize = this.values.colorize;
      const ai = this.values.ai;
      this.values = normalizeRemoteAdjustmentValues({
        ...DEFAULT_REMOTE_ADJUSTMENT_VALUES,
        colorize,
        ai,
      });
      this.syncControls();
      this.setDisabled(false);
      this.commitCurrent().catch((error) => this.showError(error));
    });
    this.status = textElement("p", "", "adjustment-status");
    this.root.append(
      this.targetRow,
      this.scopeFieldset,
      autoRow,
      this.sliderList,
      this.colorizeSection,
      this.aiSection,
      this.resetButton,
      this.status
    );
    this.syncControls();
    this.setDisabled(false);
  }

  addColorizeSlider(parent, key, label, min, max, step, visibleWhen = () => true) {
    const defaultValue = DEFAULT_REMOTE_COLORIZE_VALUES[key];
    const row = element("div", "adjustment-slider-row");
    if (key === "mono_tolerance") {
      row.title = "黄ばみや青みを含む画像をモノクロ系とみなす許容値です。";
    } else if (key === "density_normalization_strength") {
      row.title = "画像の輝度から黒点と白点を検出し、濃さとコントラストを整えます。";
    } else if (key === "tone_radius") {
      row.title = "長辺2048pxを基準にしたスクリーントーンの検出スケールです。";
    }
    const heading = element("div", "adjustment-slider-heading");
    const sliderLabel = textElement("label", label);
    const output = textElement("output", "");
    const headingActions = element("span", "adjustment-slider-heading-actions");
    const resetSlot = element("span", "adjustment-slider-reset-slot");
    const resetButton = textElement("button", "↩", "adjustment-slider-reset");
    resetButton.type = "button";
    resetButton.hidden = true;
    resetButton.title = "デフォルトに戻す";
    resetButton.setAttribute("aria-label", `${label}をデフォルトに戻す`);
    resetButton.addEventListener("pointerdown", (event) => event.stopPropagation());
    resetButton.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      if (resetButton.hidden || resetButton.disabled) return;
      this.values.colorize[key] = defaultValue;
      this.dirty = false;
      this.syncColorizeControl(key);
      this.commitCurrent().catch((error) => this.showError(error));
    });
    resetSlot.append(resetButton);
    headingActions.append(output, resetSlot);
    heading.append(sliderLabel, headingActions);

    const input = document.createElement("input");
    input.type = "range";
    input.id = `remote-colorize-${key}-${Math.random().toString(36).slice(2)}`;
    sliderLabel.htmlFor = input.id;
    input.min = "0";
    input.max = "1";
    input.step = "any";
    input.setAttribute("aria-valuemin", String(min));
    input.setAttribute("aria-valuemax", String(max));
    let pointerDrag = null;
    const applyValue = (raw) => {
      if (!Number.isFinite(raw)) return false;
      const value = rangeValueFromNormalized({
        normalized: rangeValueToNormalized({ value: raw, min, max, logarithmic: false }),
        min,
        max,
        step,
        logarithmic: false,
      });
      if (this.values.colorize[key] === value) return false;
      this.values.colorize[key] = value;
      this.dirty = true;
      this.syncColorizeControl(key);
      this.queuePreview();
      return true;
    };
    const applyNormalizedPosition = (position) => {
      const changed = applyValue(rangeValueFromNormalized({
        normalized: position,
        min,
        max,
        step,
        logarithmic: false,
      }));
      if (!changed) this.syncColorizeControl(key);
      return changed;
    };
    input.addEventListener("input", () => {
      if (pointerDrag) {
        this.syncColorizeControl(key);
        return;
      }
      applyNormalizedPosition(Number(input.value));
    });
    const finish = (event) => {
      event.stopPropagation();
      if (!this.dirty) return;
      this.dirty = false;
      this.commitCurrent().catch((error) => this.showError(error));
    };
    const keyboardKeys = new Set([
      "ArrowLeft",
      "ArrowDown",
      "ArrowRight",
      "ArrowUp",
      "PageDown",
      "PageUp",
      "Home",
      "End",
    ]);
    let keyboardAdjusting = false;
    input.addEventListener("keydown", (event) => {
      if (input.disabled || !keyboardKeys.has(event.key)) return;
      event.preventDefault();
      event.stopPropagation();
      const current = Number(this.values.colorize[key]);
      let next = current;
      if (event.key === "Home") next = min;
      else if (event.key === "End") next = max;
      else if (event.key === "PageDown") next -= (max - min) / 10;
      else if (event.key === "PageUp") next += (max - min) / 10;
      else if (["ArrowLeft", "ArrowDown"].includes(event.key)) next -= step;
      else next += step;
      keyboardAdjusting = applyValue(next) || keyboardAdjusting;
    });
    input.addEventListener("keyup", (event) => {
      if (!keyboardKeys.has(event.key)) return;
      event.stopPropagation();
      if (!keyboardAdjusting) return;
      keyboardAdjusting = false;
      finish(event);
    });
    input.addEventListener("blur", (event) => {
      if (!keyboardAdjusting) return;
      keyboardAdjusting = false;
      finish(event);
    });
    input.addEventListener("pointerdown", (event) => {
      event.stopPropagation();
      if (input.disabled || event.isPrimary === false) return;
      if (typeof event.button === "number" && event.button !== 0) return;
      if (event.cancelable) event.preventDefault();
      pointerDrag = {
        pointerId: event.pointerId,
        startClientX: event.clientX,
        startValue: Number(this.values.colorize[key]),
        trackWidth: input.getBoundingClientRect().width,
      };
      input.focus({ preventScroll: true });
      try {
        input.setPointerCapture(event.pointerId);
      } catch (_error) {
        // Pointer capture can fail when the browser has already cancelled the pointer.
      }
    });
    input.addEventListener("pointermove", (event) => {
      if (!pointerDrag || pointerDrag.pointerId !== event.pointerId) return;
      event.stopPropagation();
      if (event.cancelable) event.preventDefault();
      applyValue(relativeRangeDragValue({
        startValue: pointerDrag.startValue,
        startClientX: pointerDrag.startClientX,
        currentClientX: event.clientX,
        trackWidth: pointerDrag.trackWidth,
        min,
        max,
        step,
        logarithmic: false,
      }));
    });
    const finishPointer = (event) => {
      if (pointerDrag && pointerDrag.pointerId !== event.pointerId) {
        event.stopPropagation();
        return;
      }
      if (pointerDrag) {
        if (event.cancelable) event.preventDefault();
        try {
          if (input.hasPointerCapture(event.pointerId)) {
            input.releasePointerCapture(event.pointerId);
          }
        } catch (_error) {
          // The browser may release capture before dispatching pointercancel.
        }
        pointerDrag = null;
      }
      finish(event);
    };
    input.addEventListener("pointerup", finishPointer);
    input.addEventListener("pointercancel", finishPointer);
    input.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
    });
    for (const eventName of ["touchend", "touchcancel", "change"]) {
      input.addEventListener(eventName, finish);
    }

    row.append(heading, input);
    parent.append(row);
    this.colorizeControls.set(key, {
      row,
      input,
      output,
      min,
      max,
      step,
      defaultValue,
      resetButton,
      visibleWhen,
    });
  }

  currentTarget() {
    const group = currentPageGroup();
    const entry = group?.entries[this.targetIndex];
    const address = entry ? entryAddress(entry) : null;
    if (!entry || !address?.favorite_id) return null;
    return {
      entry,
      address,
      pageIndex: this.targetIndex,
      identity: `${state.pageGroupIndex}\n${addressIdentity(address)}`,
    };
  }

  async refresh() {
    const group = currentPageGroup();
    const nextGroupIdentity = `${state.pageGroupIndex}\n${(group?.entries ?? [])
      .map(entryIdentity)
      .join("\n")}`;
    if (this.groupIdentity !== nextGroupIdentity) {
      this.groupIdentity = nextGroupIdentity;
      this.targetIndex = 0;
      this.previewEpoch += 1;
      this.previewQueue.clear();
    }
    this.targetRow.hidden = (group?.entries.length ?? 0) !== 2;
    this.syncTargetButtons();
    const target = this.currentTarget();
    if (!target) {
      this.setDisabled(true);
      this.status.textContent = "このページは画像補正の対象ではありません。";
      return;
    }
    const sequence = ++this.refreshSequence;
    this.setDisabled(true);
    this.status.textContent = "現在値を読み込み中…";
    const response = await apiPostJson("/api/write", {
      kind: "get_adjustment_state",
      address: target.address,
    });
    if (sequence !== this.refreshSequence || this.currentTarget()?.identity !== target.identity) {
      return;
    }
    if (!response.adjustment_state) {
      throw new Error("画像補正の現在値を取得できませんでした。");
    }
    this.applyServerState(response.adjustment_state);
    this.setDisabled(false);
    this.status.textContent = "スライダーを離すと保存します。";
  }

  applyServerState(adjustmentState) {
    this.serverState = adjustmentState;
    this.status.classList.remove("is-error");
    this.scope = adjustmentState.selected_scope === "page" ? "page" : "standard";
    this.values = normalizeRemoteAdjustmentValues(adjustmentState.effective_values);
    this.colorizePresetSlots = Array.from({ length: 4 }, (_, index) => {
      const preset = adjustmentState.colorize_preset_slots?.[index];
      return preset ? normalizeRemoteColorizeParams(preset) : null;
    });
    const standard = this.scopeInputs.get("standard");
    standard.text.textContent = adjustmentState.standard_label || "標準";
    standard.input.disabled = !adjustmentState.standard_available;
    this.scopeInputs.get(this.scope).input.checked = true;
    this.syncAiControls(adjustmentState.ai_model_catalog);
    this.aiAvailability.textContent = adjustmentState.effective_ai_enabled
      ? "この設定では、表示時に AI 処理を行います。"
      : "この設定では、表示時に AI 処理を行いません。";
    this.syncControls();
    this.setDisabled(false);
  }

  async changeScope(scope) {
    if (scope === this.scope) return;
    const previousState = this.serverState;
    this.scope = scope;
    this.values = normalizeRemoteAdjustmentValues(
      scope === "standard"
        ? previousState?.standard_values
        : previousState?.effective_values
    );
    this.syncControls();
    this.setDisabled(false);
    if (scope === "standard" && previousState?.has_page_override) {
      await this.commitCurrent();
    }
  }

  queuePreview() {
    const target = this.currentTarget();
    if (!target) return;
    const epoch = this.previewEpoch;
    this.previewQueue.enqueue({
      ...target,
      epoch,
      sequence: ++this.previewSequence,
      scope: this.scope,
      values: normalizeRemoteAdjustmentValues(this.values),
    });
  }

  async runPreview(job) {
    if (job.epoch !== this.previewEpoch || this.currentTarget()?.identity !== job.identity) return;
    const info = await imageInfo(job.entry);
    if (job.epoch !== this.previewEpoch || this.currentTarget()?.identity !== job.identity) return;
    const request = imageRequest(job.entry, info, state.viewer.stage, {
      targetPxOverride: 768,
      adjustmentPreview: { scope: job.scope, values: job.values },
      previewRevision: `preview-${state.pageRenderRevision}-${job.sequence}`,
    });
    const response = await observedFetch(request.url, { credentials: "same-origin" });
    if (!response.ok) {
      const detail = await response.clone().json().catch(() => ({}));
      throw new Error(detail.message || `補正プレビューに失敗しました (HTTP ${response.status})。`);
    }
    const blob = await response.blob();
    if (
      job.epoch !== this.previewEpoch ||
      this.previewQueue.latest !== null ||
      this.currentTarget()?.identity !== job.identity
    ) {
      return;
    }
    await state.viewer?.replacePageBlob(job.pageIndex, blob, job.entry.name);
  }

  commitCurrent() {
    const target = this.currentTarget();
    if (!target) return Promise.resolve();
    this.previewEpoch += 1;
    this.previewQueue.clear();
    const commitId = ++this.commitSequence;
    const request = {
      kind: "set_adjustment",
      address: target.address,
      scope: this.scope,
      values: normalizeRemoteAdjustmentValues(this.values),
    };
    this.status.textContent = "保存中…";
    this.writeTail = this.writeTail.catch(() => {}).then(async () => {
      const response = await apiPostJson("/api/write", request);
      if (commitId !== this.commitSequence || this.currentTarget()?.identity !== target.identity) {
        return;
      }
      if (!response.adjustment_state) {
        throw new Error("画像補正の保存結果を取得できませんでした。");
      }
      this.applyServerState(response.adjustment_state);
      state.pageRenderRevision += 1;
      pageResourceCache.clear();
      await updateViewerImage(performance.now());
      if (commitId === this.commitSequence) this.status.textContent = "保存しました。";
    }).catch((error) => {
      if (commitId === this.commitSequence) this.showError(error);
      throw error;
    });
    return this.writeTail;
  }

  syncControl(key) {
    const control = this.controls.get(key);
    if (!control) return;
    const value = Number(this.values[key]);
    control.input.value = String(rangeValueToNormalized({
      value,
      min: control.min,
      max: control.max,
      logarithmic: control.logarithmic,
    }));
    const valueText = control.step < 1
      ? Number(this.values[key]).toFixed(2)
      : String(Math.round(this.values[key]));
    control.output.value = valueText;
    control.input.setAttribute("aria-valuenow", String(value));
    control.input.setAttribute("aria-valuetext", valueText);
    this.syncResetButton(key);
  }

  syncResetButton(key) {
    const control = this.controls.get(key);
    if (!control) return;
    const manualDisabled = this.disabled || this.values.auto_mode !== null;
    control.resetButton.disabled = manualDisabled;
    control.resetButton.hidden = !adjustmentResetVisible({
      value: this.values[key],
      defaultValue: control.defaultValue,
      disabled: manualDisabled,
      epsilon: control.step < 1 ? 0.001 : 0,
    });
  }

  syncColorizeControl(key) {
    const control = this.colorizeControls.get(key);
    if (!control) return;
    control.row.hidden = !control.visibleWhen();
    const value = Number(this.values.colorize[key]);
    control.input.value = String(rangeValueToNormalized({
      value,
      min: control.min,
      max: control.max,
      logarithmic: false,
    }));
    const valueText = control.step === 0.1
      ? value.toFixed(1)
      : String(Math.round(value));
    control.output.value = valueText;
    control.input.setAttribute("aria-valuenow", String(value));
    control.input.setAttribute("aria-valuetext", valueText);
    this.syncColorizeResetButton(key);
  }

  syncColorizeResetButton(key) {
    const control = this.colorizeControls.get(key);
    if (!control) return;
    const colorizeDisabled = this.disabled || this.values.colorize.mode === "disabled";
    control.resetButton.disabled = colorizeDisabled;
    control.resetButton.hidden = !adjustmentResetVisible({
      value: this.values.colorize[key],
      defaultValue: control.defaultValue,
      disabled: colorizeDisabled,
      epsilon: control.step < 1 ? 0.001 : 0,
    });
  }

  syncColorizeControls() {
    this.colorizeModeSelect.value = this.values.colorize.mode;
    this.colorizePaletteSelect.value = this.values.colorize.palette;
    this.colorizeToneSelect.value = this.values.colorize.tone_method;
    this.colorizeToneDescription.textContent = ({
      off: "スクリーントーンの網点を濃淡へ変換しません。",
      fast: "縮小平均で網点を濃淡化します。大きな画像でも高速ですが、細部は少し滑らかになります。",
      local_mean: "局所平均を1回適用します。網点を少しだけなじませたい場合に向きます。",
      gaussian: "局所平均を3回重ねてガウスぼかしを近似します。より広く滑らかに濃淡化します。",
    })[this.values.colorize.tone_method];
    this.customPaletteNote.hidden = this.values.colorize.palette !== "custom";
    for (const key of this.colorizeControls.keys()) this.syncColorizeControl(key);
  }

  syncControls() {
    for (const key of this.controls.keys()) this.syncControl(key);
    this.autoSelect.value = this.values.auto_mode ?? "";
    for (const [scope, value] of this.scopeInputs) value.input.checked = scope === this.scope;
    this.syncColorizeControls();
    this.syncAiControls(this.serverState?.ai_model_catalog);
  }

  syncAiControls(catalog = {}) {
    const selected = this.values.ai ?? { upscale_model: null, denoise_model: null };
    for (const [key, select] of this.aiSelects ?? []) {
      const catalogKey = key === "upscale_model" ? "upscale" : "denoise";
      const entries = Array.isArray(catalog?.[catalogKey]) ? catalog[catalogKey] : [];
      const selectedKey = selected[key] ?? "";
      const options = entries.map((entry) => {
        const option = document.createElement("option");
        option.value = entry.key ?? "";
        option.textContent = entry.label || "なし";
        option.disabled = entry.selectable === false;
        return option;
      });
      if (!options.some((option) => option.value === selectedKey)) {
        const fallback = document.createElement("option");
        fallback.value = selectedKey;
        fallback.textContent = selectedKey ? "現在の設定" : "なし";
        fallback.disabled = true;
        options.push(fallback);
      }
      select.replaceChildren(...options);
      select.value = selectedKey;
    }
  }

  syncTargetButtons() {
    this.targetButtons.forEach((button, index) => {
      const selected = index === this.targetIndex;
      button.classList.toggle("is-selected", selected);
      button.setAttribute("aria-pressed", selected ? "true" : "false");
    });
  }

  setDisabled(disabled) {
    this.disabled = disabled;
    const manualDisabled = disabled || this.values.auto_mode !== null;
    for (const [key, { input }] of this.controls) {
      input.disabled = manualDisabled;
      this.syncResetButton(key);
    }
    const colorizeDisabled = disabled || this.values.colorize.mode === "disabled";
    for (const [key, { input }] of this.colorizeControls) {
      input.disabled = colorizeDisabled;
      this.syncColorizeResetButton(key);
    }
    this.colorizeModeSelect.disabled = disabled;
    this.colorizePaletteSelect.disabled = colorizeDisabled;
    this.colorizeToneSelect.disabled = colorizeDisabled;
    this.colorizePresetButtons.forEach((button, index) => {
      button.disabled = disabled || this.colorizePresetSlots[index] === null;
    });
    this.autoSelect.disabled = disabled;
    for (const select of this.aiSelects.values()) select.disabled = disabled;
    this.resetButton.disabled = disabled;
    for (const [scope, value] of this.scopeInputs) {
      value.input.disabled = disabled || (scope === "standard" && this.serverState?.standard_available === false);
    }
  }

  showError(error) {
    this.status.textContent = error instanceof Error
      ? error.message
      : "画像補正を更新できませんでした。";
    this.status.classList.add("is-error");
  }

  destroy() {
    this.refreshSequence += 1;
    this.previewEpoch += 1;
    this.previewQueue.clear();
  }
}

class CommandMenu {
  constructor(host, context, owner = host) {
    this.context = context;
    this.owner = owner;
    this.isViewerPanel = context === "viewer";
    this.opened = false;
    this.previousFocus = null;
    this.panelState = null;
    this.panelPointer = null;
    this.panelMotionTimer = 0;
    this.panelMotionListener = null;
    this.suppressPanelClick = false;
    this.actionLabels = new Map();
    this.ratingActions = new Map();
    this.bookmarkAction = null;
    this.ratingSummaryAction = null;
    this.keyboardElements = [];
    this.viewerTabButtons = new Map();
    this.adjustmentPanel = null;
    const definition = menuDefinition(context, "main");
    this.root = element("div", "command-menu-layer");
    if (this.isViewerPanel) this.root.classList.add("viewer-command-menu-layer");
    this.root.hidden = true;

    const scrim = element("button", "command-menu-scrim");
    scrim.type = "button";
    scrim.setAttribute("aria-label", "操作メニューを閉じる");
    scrim.addEventListener("click", (event) => {
      if (this.isViewerPanel) this.close();
      else menuCommand(event, CommandName.TOGGLE_MENU);
    });

    const panel = element(
      "section",
      this.isViewerPanel ? "command-menu viewer-command-menu" : "command-menu"
    );
    panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-modal", "true");
    this.panel = panel;
    const header = element("header", "command-menu-header");
    const close = textElement("button", "×", "command-menu-close");
    close.type = "button";
    close.setAttribute("aria-label", "操作メニューを閉じる");
    close.addEventListener("click", (event) => {
      if (this.isViewerPanel) this.close();
      else menuCommand(event, CommandName.TOGGLE_MENU);
    });
    const title = textElement("h2", definition.title);
    header.append(title, close);
    this.title = title;
    this.closeButton = close;

    const actions = element("div", "command-menu-actions");
    actions.setAttribute("role", "menu");
    this.actions = actions;

    const shortcutTitle = textElement("h3", "有効なキー", "command-shortcut-title");
    const shortcuts = element("dl", "command-shortcuts");
    for (const [label, keys] of definition.shortcuts ?? []) {
      shortcuts.append(textElement("dt", label), textElement("dd", keys));
    }
    this.shortcutElements = [shortcutTitle, shortcuts];
    if (this.isViewerPanel) {
      const tabs = element("nav", "viewer-panel-tabs");
      tabs.setAttribute("role", "tablist");
      tabs.setAttribute("aria-label", "画像パネルのタブ");
      for (const tab of VIEWER_PANEL_TABS) {
        const button = textElement("button", tab.label, "viewer-panel-tab");
        button.type = "button";
        button.setAttribute("role", "tab");
        button.addEventListener("click", (event) => {
          event.stopPropagation();
          this.selectViewerTab(tab.id);
        });
        this.viewerTabButtons.set(tab.id, button);
        tabs.append(button);
      }
      this.viewerTabs = tabs;
      this.placeholder = element("section", "viewer-panel-placeholder");
      this.placeholder.hidden = true;
      const body = element("div", "viewer-command-menu-body");
      body.append(actions, this.placeholder, shortcutTitle, shortcuts);
      this.panelBody = body;
      panel.append(header, tabs, body);
    } else {
      panel.append(header, actions, shortcutTitle, shortcuts);
    }
    this.root.append(scrim, panel);
    host.append(this.root);
    if (this.isViewerPanel) {
      this.panelPointerDown = (event) => this.onPanelPointerDown(event);
      this.panelPointerUp = (event) => this.onPanelPointerUp(event, false);
      this.panelPointerCancel = (event) => this.onPanelPointerUp(event, true);
      this.panelClickCapture = (event) => {
        if (!this.suppressPanelClick) return;
        event.preventDefault();
        event.stopImmediatePropagation();
        this.suppressPanelClick = false;
      };
      this.viewportResize = () => this.updateViewerPanelLayout();
      this.root.addEventListener("pointerdown", this.panelPointerDown);
      this.root.addEventListener("pointerup", this.panelPointerUp);
      this.root.addEventListener("pointercancel", this.panelPointerCancel);
      this.root.addEventListener("click", this.panelClickCapture, true);
      window.addEventListener("resize", this.viewportResize);
      this.updateViewerPanelLayout();
      this.selectViewerTab(state.viewerPanelTab);
    } else {
      this.showPage("main");
    }
  }

  showPage(page) {
    const definition = menuDefinition(this.context, page);
    if (this.isViewerPanel) {
      state.viewerPanelTab = "functions";
      this.syncViewerTabButtons("functions");
      this.actions.hidden = false;
      this.placeholder.hidden = true;
    }
    this.currentPage = page;
    this.title.textContent = definition.title;
    this.panel.setAttribute("aria-label", definition.title);
    this.actionLabels.clear();
    this.ratingActions.clear();
    this.bookmarkAction = null;
    this.ratingSummaryAction = null;
    this.keyboardElements = [...this.shortcutElements];
    const buttons = [];
    for (const [name, label, keys, payload = {}] of definition.actions) {
      const button = element("button", "command-menu-action");
      button.type = "button";
      button.setAttribute("role", "menuitem");
      const actionLabel = textElement("span", label);
      const keyHint = textElement("kbd", keys);
      this.actionLabels.set(name, actionLabel);
      if (name === CommandName.SET_RATING) {
        this.ratingActions.set(Number(payload.stars), { button, label: actionLabel });
        button.disabled = true;
      } else if (name === CommandName.TOGGLE_BOOKMARK) {
        this.bookmarkAction = { button, label: actionLabel };
        button.disabled = true;
      } else if (name === MenuPageAction.RATING) {
        this.ratingSummaryAction = { button, label: actionLabel };
      }
      this.keyboardElements.push(keyHint);
      button.append(actionLabel, keyHint);
      button.addEventListener("click", (event) => {
        if (payload.menuPage) {
          this.showPage(payload.menuPage);
          return;
        }
        if (name === MenuPageAction.BACK) {
          this.showPage("main");
          return;
        }
        this.close(false);
        menuCommand(event, name, payload);
      });
      buttons.push(button);
    }
    this.actions.replaceChildren(...buttons);
    this.setKeyboardAvailable(shouldShowKeyboardShortcuts({
      coarsePointer: state.coarsePointer,
      keyboardUsed: state.keyboardInputSeen,
    }));
    if (this.context === "viewer") {
      this.setItemState(state.viewerItemState, state.viewerItemState === null);
    }
  }

  selectViewerTab(tabId) {
    if (!this.isViewerPanel) return;
    const tab = VIEWER_PANEL_TABS.find((candidate) => candidate.id === tabId) ??
      VIEWER_PANEL_TABS[0];
    state.viewerPanelTab = tab.id;
    this.syncViewerTabButtons(tab.id);
    if (tab.id === "functions") {
      this.showPage("main");
      return;
    }
    this.currentPage = `tab:${tab.id}`;
    this.title.textContent = tab.label;
    this.panel.setAttribute("aria-label", tab.label);
    this.actionLabels.clear();
    this.ratingActions.clear();
    this.bookmarkAction = null;
    this.ratingSummaryAction = null;
    this.keyboardElements = [];
    this.actions.hidden = true;
    this.placeholder.hidden = false;
    if (tab.id === "adjustment") {
      this.adjustmentPanel ??= new ViewerAdjustmentPanel();
      this.placeholder.replaceChildren(this.adjustmentPanel.root);
      this.adjustmentPanel.refresh().catch((error) => this.adjustmentPanel.showError(error));
    } else {
      this.placeholder.replaceChildren(textElement("h3", tab.label));
    }
    for (const target of this.shortcutElements) target.hidden = true;
  }

  syncViewerTabButtons(selectedId) {
    for (const [id, button] of this.viewerTabButtons) {
      const selected = id === selectedId;
      button.classList.toggle("is-selected", selected);
      button.setAttribute("aria-selected", selected ? "true" : "false");
      button.tabIndex = selected ? 0 : -1;
    }
  }

  isOpen() {
    return this.opened;
  }

  refreshAdjustment() {
    if (!this.opened || state.viewerPanelTab !== "adjustment") return;
    this.adjustmentPanel?.refresh().catch((error) => this.adjustmentPanel.showError(error));
  }

  toggle() {
    if (this.opened) this.close();
    else this.open();
    return true;
  }

  setActionLabel(name, label) {
    const target = this.actionLabels.get(name);
    if (target) target.textContent = label;
  }

  setKeyboardAvailable(available) {
    for (const target of this.keyboardElements) {
      target.hidden = !available;
    }
  }

  setItemState(itemState, loading = false) {
    for (const [stars, action] of this.ratingActions) {
      action.button.disabled = loading || !itemState;
      const base = stars === 0 ? "レーティングを解除" : `★${stars}`;
      action.label.textContent = itemState?.rating === stars ? `${base}（現在）` : base;
    }
    if (this.ratingSummaryAction) {
      this.ratingSummaryAction.label.textContent = loading
        ? "レーティング（読み込み中…）"
        : !itemState
          ? "レーティング"
          : itemState.rating > 0
            ? `レーティング（★${itemState.rating}）`
            : "レーティング（なし）";
    }
    if (!this.bookmarkAction) return;
    this.bookmarkAction.button.disabled =
      loading || !itemState || !itemState.bookmarkSupported;
    this.bookmarkAction.label.textContent = loading
      ? "ブックマークを読み込み中…"
      : !itemState
        ? "ブックマーク状態を取得できません"
        : !itemState.bookmarkSupported
          ? "このページはブックマーク対象外"
          : itemState.bookmarked
            ? "ブックマークを削除（登録済み）"
            : "ブックマークを追加（未登録）";
  }

  open() {
    if (this.opened) return;
    if (this.isViewerPanel) this.cancelViewerPanelMotion();
    if (this.isViewerPanel) this.selectViewerTab(state.viewerPanelTab);
    else this.showPage("main");
    this.opened = true;
    this.previousFocus = document.activeElement;
    this.root.hidden = false;
    this.owner.classList.add("menu-open");
    if (this.isViewerPanel) {
      this.updateViewerPanelLayout(ViewerPanelAction.OPEN);
      this.startViewerPanelMotion("opening");
      refreshViewerItemState().catch((error) => {
        state.viewer?.showBoundaryMessage(
          error instanceof Error ? error.message : "現在値を取得できませんでした。"
        );
      });
    }
    requestAnimationFrame(() => this.closeButton.focus());
  }

  close(restoreFocus = true, animate = true) {
    if (!this.opened) return;
    this.opened = false;
    if (this.isViewerPanel && animate) {
      this.startViewerPanelMotion("closing", () => {
        this.finishClose(restoreFocus);
      });
      return;
    }
    if (this.isViewerPanel) this.cancelViewerPanelMotion();
    this.finishClose(restoreFocus);
  }

  finishClose(restoreFocus) {
    if (this.isViewerPanel) this.updateViewerPanelLayout(ViewerPanelAction.CLOSE);
    this.root.hidden = true;
    this.owner.classList.remove("menu-open");
    if (restoreFocus && this.previousFocus instanceof HTMLElement) {
      this.previousFocus.focus({ preventScroll: true });
    }
  }

  startViewerPanelMotion(motion, onFinished = () => {}) {
    this.cancelViewerPanelMotion();
    this.root.dataset.motion = motion;
    let finished = false;
    const finish = (event) => {
      if (finished || (event?.target && event.target !== this.panel)) return;
      finished = true;
      if (this.panelMotionListener) {
        this.panel.removeEventListener("animationend", this.panelMotionListener);
      }
      clearTimeout(this.panelMotionTimer);
      this.panelMotionTimer = 0;
      this.panelMotionListener = null;
      if (this.root.dataset.motion === motion) delete this.root.dataset.motion;
      onFinished();
    };
    this.panelMotionListener = finish;
    this.panel.addEventListener("animationend", finish);
    this.panelMotionTimer = setTimeout(
      finish,
      VIEWER_PANEL_ANIMATION_MS + 80
    );
  }

  cancelViewerPanelMotion() {
    if (this.panelMotionListener) {
      this.panel.removeEventListener("animationend", this.panelMotionListener);
    }
    clearTimeout(this.panelMotionTimer);
    this.panelMotionTimer = 0;
    this.panelMotionListener = null;
    delete this.root.dataset.motion;
  }

  destroy() {
    this.close(false, false);
    if (this.isViewerPanel) this.cancelViewerPanelMotion();
    if (this.isViewerPanel) {
      this.root.removeEventListener("pointerdown", this.panelPointerDown);
      this.root.removeEventListener("pointerup", this.panelPointerUp);
      this.root.removeEventListener("pointercancel", this.panelPointerCancel);
      this.root.removeEventListener("click", this.panelClickCapture, true);
      window.removeEventListener("resize", this.viewportResize);
      this.adjustmentPanel?.destroy();
    }
    this.root.remove();
  }

  updateViewerPanelLayout(action = "resize") {
    if (!this.isViewerPanel) return;
    const next = viewerPanelTransition(this.panelState, {
      action,
      viewportWidth: window.innerWidth,
      viewportHeight: window.innerHeight,
    });
    this.panelState = next;
    this.root.dataset.orientation = next.orientation;
    this.owner.classList.toggle("viewer-panel-open", next.open);
    this.owner.classList.toggle(
      "viewer-panel-portrait",
      next.orientation === "portrait"
    );
    this.owner.classList.toggle(
      "viewer-panel-landscape",
      next.orientation === "landscape"
    );
    if (!next.shouldRefit) return;
    if (next.open) state.fitMode = FitMode.PAGE;
    if (state.commandMenu !== this || this.opened !== next.open) return;
    state.viewer?.refitVisibleContent(
      next.open ? FitMode.PAGE : state.fitMode,
      { resetTransform: true }
    );
  }

  onPanelPointerDown(event) {
    if (event.isPrimary === false) return;
    if (["mouse", "pen"].includes(event.pointerType) && event.button !== 0) return;
    this.panelPointer = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      startedAt: performance.now(),
      scrollTop: this.panelBody?.scrollTop ?? this.panel.scrollTop ?? 0,
    };
  }

  onPanelPointerUp(event, cancelled) {
    const pointer = this.panelPointer;
    if (!pointer || pointer.pointerId !== event.pointerId) return;
    this.panelPointer = null;
    const scrollTop = this.panelBody?.scrollTop ?? this.panel.scrollTop ?? 0;
    const contentScrolled = Math.abs(scrollTop - pointer.scrollTop) > 0.5;
    const gesture = viewerGestureDecision({
      dx: event.clientX - pointer.startX,
      dy: event.clientY - pointer.startY,
      elapsedMs: performance.now() - pointer.startedAt,
      moved: contentScrolled,
      contentScrolled,
      cancelled,
    });
    const action = viewerPanelGestureAction({
      gesture,
      panelOpen: true,
      contentScrolled,
    });
    if (action !== ViewerPanelAction.CLOSE) return;
    event.preventDefault();
    event.stopPropagation();
    this.suppressPanelClick = true;
    this.close();
    setTimeout(() => { this.suppressPanelClick = false; }, 0);
  }
}

function openGestureHelpDialog() {
  state.gestureHelpDialog?.destroy();
  state.gestureHelpDialog = new GestureHelpDialog(app);
}

class GestureHelpDialog {
  constructor(host) {
    this.previousFocus = document.activeElement;
    this.root = element("div", "command-menu-layer gesture-help-layer");

    const scrim = element("button", "command-menu-scrim");
    scrim.type = "button";
    scrim.setAttribute("aria-label", "操作方法を閉じる");
    scrim.addEventListener("click", () => this.dismiss());

    const panel = element("section", "command-menu gesture-help-dialog");
    panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-modal", "true");
    panel.setAttribute("aria-label", "画像の操作方法");

    const header = element("header", "command-menu-header");
    const close = textElement("button", "×", "command-menu-close");
    close.type = "button";
    close.setAttribute("aria-label", "操作方法を閉じる");
    close.addEventListener("click", () => this.dismiss());
    header.append(textElement("h2", "画像の操作方法"), close);

    const guide = element("div", "gesture-help-grid");
    const item = (symbol, title, description) => {
      const card = element("div", "gesture-help-item");
      card.append(
        textElement("span", symbol, "gesture-help-symbol"),
        textElement("strong", title),
        textElement("small", description)
      );
      return card;
    };
    guide.append(
      item("↔", "左右をタップ", "前後のページへ（綴じ方向に追従）"),
      item("◎", "中央をタップ", "上下のバーを表示・非表示"),
      item("↑", "上へスワイプ", "操作メニューを開く"),
      item("↓", "下へスワイプ", "一覧へ戻る")
    );
    const note = textElement(
      "p",
      "ダブルタップでページ全体と100%原寸を切り替えます。拡大中は1本指で画像を動かせます。",
      "gesture-help-note"
    );
    const done = textElement("button", "わかりました", "gesture-help-done");
    done.type = "button";
    done.addEventListener("click", () => this.dismiss());

    panel.append(header, guide, note, done);
    this.root.append(scrim, panel);
    this.root.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        this.dismiss();
      }
    });
    host.append(this.root);
    requestAnimationFrame(() => close.focus());
  }

  dismiss() {
    const saved = saveLocalSettings({
      ...state.localSettings,
      gestureHelpDismissed: true,
    });
    state.localSettings = saved.settings;
    state.localSettingsStorageAvailable = saved.saved;
    this.close();
  }

  close(restoreFocus = true) {
    this.root.remove();
    if (state.gestureHelpDialog === this) state.gestureHelpDialog = null;
    if (restoreFocus && this.previousFocus instanceof HTMLElement) {
      this.previousFocus.focus({ preventScroll: true });
    }
  }

  destroy() {
    this.close(false);
  }
}

function openLocalSettingsDialog() {
  state.localSettingsDialog?.destroy();
  state.localSettingsDialog = new LocalSettingsDialog(app);
}

class LocalSettingsDialog {
  constructor(host) {
    this.previousFocus = document.activeElement;
    this.root = element("div", "command-menu-layer local-settings-layer");

    const scrim = element("button", "command-menu-scrim");
    scrim.type = "button";
    scrim.setAttribute("aria-label", "端末の設定を閉じる");
    scrim.addEventListener("click", () => this.close());

    const panel = element("section", "command-menu local-settings-dialog");
    panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-modal", "true");
    panel.setAttribute("aria-label", "端末の設定");

    const header = element("header", "command-menu-header");
    const close = textElement("button", "×", "command-menu-close");
    close.type = "button";
    close.setAttribute("aria-label", "端末の設定を閉じる");
    close.addEventListener("click", () => this.close());
    header.append(textElement("h2", "端末の設定"), close);

    const option = element("label", "local-settings-option");
    const checkbox = element("input");
    checkbox.type = "checkbox";
    checkbox.checked = state.localSettings.portraitSinglePage;
    const copy = element("span", "local-settings-copy");
    copy.append(
      textElement("strong", "縦長画面では見開きを解除する"),
      textElement(
        "small",
        "この端末だけに保存します。OFF では縦持ちでも見開きを維持します。"
      )
    );
    option.append(checkbox, copy);

    this.status = textElement("p", "", "local-settings-status");
    this.updateStorageStatus();
    checkbox.addEventListener("change", () => {
      const saved = saveLocalSettings({
        ...state.localSettings,
        portraitSinglePage: checkbox.checked,
      });
      state.localSettings = saved.settings;
      state.localSettingsStorageAvailable = saved.saved;
      this.updateStorageStatus();
      const forceSinglePage = shouldForceSinglePageForViewport();
      if (state.container && forceSinglePage !== state.forceSinglePage) {
        refreshContainerSpread(forceSinglePage).catch((error) => {
          state.viewer?.showBoundaryMessage(
            error instanceof Error ? error.message : "表示を更新できませんでした。"
          );
        });
      }
    });

    panel.append(header, option, this.status);
    this.root.append(scrim, panel);
    this.root.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        this.close();
      }
    });
    host.append(this.root);
    requestAnimationFrame(() => checkbox.focus());
  }

  updateStorageStatus() {
    this.status.textContent = state.localSettingsStorageAvailable
      ? "設定はこの端末のブラウザに保存されます。"
      : "この環境では保存できません。このタブを閉じるまで設定を使用します。";
    this.status.classList.toggle(
      "is-warning",
      !state.localSettingsStorageAvailable
    );
  }

  close(restoreFocus = true) {
    this.root.remove();
    if (state.localSettingsDialog === this) state.localSettingsDialog = null;
    if (restoreFocus && this.previousFocus instanceof HTMLElement) {
      this.previousFocus.focus({ preventScroll: true });
    }
  }

  destroy() {
    this.close(false);
  }
}

const REMOTE_AI_PHASE_LABELS = Object.freeze({
  // 接続を取ったとき PC 側に処理が残っていると、それが終わるまで始められない。
  // 待ちが数秒続き得るので、他の phase と同じ「準備しています」に丸めず理由を出す。
  waiting_for_local_drain: "PC 側の処理を待っています",
  preparing_source: "画像を準備しています",
  loading_model: "準備しています",
  denoising: "ノイズを除去しています",
  upscaling: "拡大しています",
  finalizing: "表示を整えています",
  cancelling: "取り消しています",
});

export function remoteAiProgressText(snapshot) {
  const progress = snapshot?.progress;
  if (!progress) return "AI 処理を進めています";
  const parts = [REMOTE_AI_PHASE_LABELS[progress.phase] || "AI 処理を進めています"];
  const pageCount = Math.max(1, Number(progress.page_count) || Number(snapshot.page_count) || 1);
  const pageIndex = Math.min(pageCount - 1, Math.max(0, Number(progress.page_index) || 0));
  if (pageCount > 1) parts.push(`ページ ${pageIndex + 1} / ${pageCount}`);
  const stageCount = Math.max(1, Number(progress.stage_count) || 1);
  const stageIndex = Math.min(stageCount - 1, Math.max(0, Number(progress.stage_index) || 0));
  if (stageCount > 1) parts.push(`処理 ${stageIndex + 1} / ${stageCount}`);
  const completed = Number(progress.completed_tiles);
  const total = Number(progress.total_tiles);
  if (Number.isInteger(completed) && Number.isInteger(total) && total > 0) {
    parts.push(`進み具合 ${Math.min(completed, total)} / ${total}`);
  }
  return parts.join(" · ");
}

export function remoteAiPollingDelay({ visibilityState, terminal, failureCount = null }) {
  if (visibilityState === "hidden" || terminal) return null;
  if (failureCount === null) return AI_FOREGROUND_POLL_MS;
  return AI_RETRY_DELAYS_MS[
    Math.min(Math.max(0, Number(failureCount) || 0), AI_RETRY_DELAYS_MS.length - 1)
  ];
}

function remoteAiGroupHash(pages, revision = state.pageRenderRevision) {
  const source = `${revision}\n${pages.map((page) =>
    `${addressIdentity(page.address)}\n${Math.max(1, Number(page.target_px) || 1)}`
  ).join("\n---\n")}`;
  let first = 0x811c9dc5;
  let second = 0x9e3779b9;
  for (let index = 0; index < source.length; index += 1) {
    const code = source.charCodeAt(index);
    first = Math.imul(first ^ code, 0x01000193) >>> 0;
    second = Math.imul(second ^ code, 0x85ebca6b) >>> 0;
  }
  return `${first.toString(16).padStart(8, "0")}${second.toString(16).padStart(8, "0")}`;
}

export function selectRecoverableRemoteAiJob(jobs, groupHash, requestId = null) {
  const candidates = Array.isArray(jobs) ? jobs : [];
  if (requestId) {
    const exact = candidates.find((job) => job?.request_id === requestId);
    if (exact) return exact;
  }
  const prefix = `miv-ai:${groupHash}:`;
  return candidates
    .filter((job) =>
      typeof job?.request_id === "string" &&
      job.request_id.startsWith(prefix) &&
      job.state !== "cancelling" &&
      !AI_TERMINAL_STATES.has(job.state)
    )
    .sort((left, right) => Number(right.created_unix_ms) - Number(left.created_unix_ms))[0] ?? null;
}

function createRemoteAiRequestId(groupHash) {
  const unique = globalThis.crypto?.randomUUID?.() ??
    `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  return `miv-ai:${groupHash}:${unique}`;
}

export class RemoteAiController {
  constructor(viewer, stage) {
    this.viewer = viewer;
    this.stage = stage;
    this.pages = [];
    this.groupHash = "";
    this.requestId = null;
    this.job = null;
    this.generation = 0;
    this.displayVersion = 0;
    this.appliedIdentity = "";
    this.pollTimer = 0;
    this.hideTimer = 0;
    this.retryIndex = 0;
    this.destroyed = false;

    this.root = element("div", "viewer-ai-status");
    this.root.hidden = true;
    this.root.setAttribute("role", "status");
    this.root.setAttribute("aria-live", "polite");
    this.message = textElement("span", "");
    this.cancelButton = textElement("button", "取り消す");
    this.cancelButton.type = "button";
    this.cancelButton.addEventListener("click", (event) => {
      event.stopPropagation();
      this.cancel().catch((error) => this.showRequestError(error));
    });
    this.root.append(this.message, this.cancelButton);
    viewer.root.append(this.root);
  }

  async displayGroup(pages) {
    const normalized = pages
      .filter((page) => page?.address)
      .map((page) => ({
        address: page.address,
        target_px: Math.max(1, Math.round(Number(page.target_px) || 1)),
        name: String(page.name || "画像"),
      }));
    if (!normalized.length || normalized.length > 2) return;
    const groupHash = remoteAiGroupHash(normalized);
    this.displayVersion += 1;
    this.clearPoll();
    if (groupHash === this.groupHash && this.job) {
      this.pages = normalized;
      await this.handleSnapshot(this.job, this.generation);
      return;
    }
    const generation = ++this.generation;
    this.pages = normalized;
    this.groupHash = groupHash;
    this.requestId = null;
    this.job = null;
    this.appliedIdentity = "";
    this.hide();
    if (document.visibilityState === "hidden") return;
    if (await this.recoverCurrent(generation)) return;
    await this.startIfEnabled(generation);
  }

  async startIfEnabled(generation) {
    const adjustmentStates = await Promise.all(this.pages.map(async (page) => {
      const response = await apiPostJson("/api/write", {
        kind: "get_adjustment_state",
        address: page.address,
      });
      return response.adjustment_state;
    }));
    if (!this.isCurrent(generation)) return;
    if (!adjustmentStates.some((adjustment) => adjustment?.effective_ai_enabled === true)) {
      this.hide();
      return;
    }
    this.requestId = createRemoteAiRequestId(this.groupHash);
    this.show("準備しています", { cancellable: true });
    const snapshot = await apiPostJson("/api/ai/jobs", {
      request_id: this.requestId,
      pages: this.pages.map(({ address, target_px }) => ({ address, target_px })),
    });
    if (!this.isCurrent(generation)) return;
    await this.handleSnapshot(snapshot, generation);
  }

  async recoverCurrent(generation = this.generation) {
    const jobs = await apiJson("/api/ai/jobs", { recoverable: 1 });
    if (!this.isCurrent(generation)) return false;
    const recovered = selectRecoverableRemoteAiJob(jobs, this.groupHash, this.requestId);
    if (!recovered || Number(recovered.page_count) !== this.pages.length) return false;
    this.requestId = recovered.request_id;
    await this.handleSnapshot(recovered, generation);
    return true;
  }

  async handleForegroundResume() {
    if (this.destroyed || !this.pages.length) return;
    this.clearPoll();
    const generation = this.generation;
    try {
      if (await this.recoverCurrent(generation)) return;
      await this.startIfEnabled(generation);
    } catch (error) {
      if (this.isCurrent(generation)) this.retryAfterFailure(error, generation);
    }
  }

  suspend() {
    this.clearPoll();
  }

  async poll(generation = this.generation) {
    if (!this.isCurrent(generation) || document.visibilityState === "hidden" || !this.job) return;
    try {
      const snapshot = await apiJson(`/api/ai/jobs/${encodeURIComponent(this.job.job_id)}`);
      if (!this.isCurrent(generation)) return;
      this.retryIndex = 0;
      await this.handleSnapshot(snapshot, generation);
    } catch (error) {
      if (this.isCurrent(generation)) this.retryAfterFailure(error, generation);
    }
  }

  retryAfterFailure(error, generation) {
    if (error instanceof AuthenticationRequiredError) return;
    this.show("接続を確認しています", { cancellable: Boolean(this.job) });
    const delay = remoteAiPollingDelay({
      visibilityState: document.visibilityState,
      terminal: false,
      failureCount: this.retryIndex,
    });
    this.retryIndex += 1;
    if (delay !== null) this.schedulePoll(delay, generation);
  }

  async handleSnapshot(snapshot, generation) {
    if (!this.isCurrent(generation) || !snapshot?.job_id) return;
    this.job = snapshot;
    this.requestId = snapshot.request_id;
    if (!AI_TERMINAL_STATES.has(snapshot.state)) {
      this.show(remoteAiProgressText(snapshot), { cancellable: snapshot.state !== "cancelling" });
      const delay = remoteAiPollingDelay({
        visibilityState: document.visibilityState,
        terminal: false,
      });
      if (delay !== null) this.schedulePoll(delay, generation);
      return;
    }
    this.clearPoll();
    if (snapshot.state === "ready") {
      await this.applyReady(snapshot, generation);
      return;
    }
    const message = snapshot.terminal?.message || "AI 処理を完了できませんでした。";
    this.show(message, { error: snapshot.state === "failed", cancellable: false });
  }

  async applyReady(snapshot, generation) {
    const ready = (snapshot.page_outcomes ?? [])
      .filter((outcome) => outcome.state === "ready")
      .map((outcome) => Number(outcome.page_index))
      .filter((index) => Number.isInteger(index) && index >= 0 && index < this.pages.length);
    const notApplicable = (snapshot.page_outcomes ?? [])
      .filter((outcome) => outcome.state === "not_applicable");
    if (!ready.length) {
      const details = notApplicable
        .map((outcome) => outcome.terminal?.message)
        .filter(Boolean);
      this.show(details.join(" / ") || "この画像では AI 処理を使いません。", {
        cancellable: false,
      });
      return;
    }
    const appliedIdentity = `${snapshot.job_id}:${this.displayVersion}`;
    if (this.appliedIdentity === appliedIdentity) {
      this.show(notApplicable.length
        ? "AI 処理が完了しました。一部のページは元の表示です。"
        : "AI 処理が完了しました。", { cancellable: false, hideAfterMs: 2400 });
      return;
    }
    this.show("表示を整えています", { cancellable: false });
    const replacements = await Promise.all(ready.map(async (pageIndex) => {
      const response = await observedFetch(
        `/api/ai/jobs/${encodeURIComponent(snapshot.job_id)}/result?page=${pageIndex}`,
        { credentials: "same-origin", headers: { Accept: "image/webp" } }
      );
      if (!response.ok) {
        const detail = await response.clone().json().catch(() => ({}));
        throw new Error(detail.message || "AI 処理後の画像を取得できませんでした。");
      }
      return {
        pageIndex,
        blob: await response.blob(),
        alt: this.pages[pageIndex]?.name || "AI 処理後の画像",
      };
    }));
    if (!this.isCurrent(generation) || this.job?.job_id !== snapshot.job_id) return;
    const replaced = await this.viewer.replacePageBlobs(replacements);
    if (!replaced || !this.isCurrent(generation)) return;
    this.appliedIdentity = appliedIdentity;
    this.show(notApplicable.length
      ? "AI 処理が完了しました。一部のページは元の表示です。"
      : "AI 処理が完了しました。", { cancellable: false, hideAfterMs: 2400 });
  }

  async cancel() {
    const job = this.job;
    if (!job || AI_TERMINAL_STATES.has(job.state)) return;
    this.show("取り消しています", { cancellable: false });
    const response = await observedFetch(
      `/api/ai/jobs/${encodeURIComponent(job.job_id)}`,
      { method: "DELETE", credentials: "same-origin", headers: { Accept: "application/json" } }
    );
    if (!response.ok) {
      const detail = await response.clone().json().catch(() => ({}));
      throw new Error(detail.message || "AI 処理を取り消せませんでした。");
    }
    await this.handleSnapshot(await response.json(), this.generation);
  }

  showRequestError(_error) {
    this.show("AI 処理を開始できませんでした。", { error: true, cancellable: false });
  }

  show(message, { error = false, cancellable = false, hideAfterMs = 0 } = {}) {
    clearTimeout(this.hideTimer);
    this.hideTimer = 0;
    this.message.textContent = message;
    this.root.classList.toggle("is-error", error);
    this.cancelButton.hidden = !cancellable;
    this.root.hidden = false;
    if (hideAfterMs > 0) {
      this.hideTimer = window.setTimeout(() => this.hide(), hideAfterMs);
    }
  }

  hide() {
    clearTimeout(this.hideTimer);
    this.hideTimer = 0;
    this.root.hidden = true;
    this.root.classList.remove("is-error");
  }

  schedulePoll(delay, generation) {
    this.clearPoll();
    if (document.visibilityState === "hidden" || !this.isCurrent(generation)) return;
    this.pollTimer = window.setTimeout(() => {
      this.pollTimer = 0;
      this.poll(generation);
    }, delay);
  }

  clearPoll() {
    clearTimeout(this.pollTimer);
    this.pollTimer = 0;
  }

  isCurrent(generation) {
    return !this.destroyed && generation === this.generation;
  }

  destroy() {
    if (this.destroyed) return;
    const activeJobId = this.job && !AI_TERMINAL_STATES.has(this.job.state)
      ? this.job.job_id
      : null;
    this.destroyed = true;
    this.generation += 1;
    this.clearPoll();
    clearTimeout(this.hideTimer);
    this.root.remove();
    if (activeJobId) {
      observedFetch(`/api/ai/jobs/${encodeURIComponent(activeJobId)}`, {
        method: "DELETE",
        credentials: "same-origin",
      }).catch(() => {});
    }
  }
}

export class ImageViewer {
  constructor({
    root,
    stage,
    pageLayer,
    image,
    title,
    counter,
    seek,
    seekInput,
    previous,
    next,
    loadingIndicator,
    boundaryMessage,
  }) {
    this.root = root;
    this.stage = stage;
    this.pageLayer = pageLayer ?? element("div", "viewer-pages");
    if (!pageLayer) {
      this.pageLayer.append(image);
      this.stage.append(this.pageLayer);
    }
    this.image = image;
    this.images = [image];
    this.title = title;
    this.counter = counter;
    this.seek = seek;
    this.seekInput = seekInput;
    this.previous = previous;
    this.next = next;
    this.loadingIndicator = loadingIndicator;
    this.boundaryMessage = boundaryMessage ?? element("div", "viewer-boundary-message");
    if (!boundaryMessage) {
      this.boundaryMessage.hidden = true;
      this.stage.append(this.boundaryMessage);
    }
    this.scale = 1;
    this.panX = 0;
    this.panY = 0;
    this.pointers = new Map();
    this.single = null;
    this.pinch = null;
    this.pinched = false;
    this.wheelDelta = 0;
    this.lastWheelCommandAt = 0;
    this.resizeTimer = 0;
    this.loadSequence = 0;
    this.fetchController = null;
    this.objectUrl = null;
    this.objectUrls = [];
    this.loadingTimer = 0;
    this.boundaryMessageTimer = 0;
    this.centerTapTimer = 0;

    this.pointerDown = (event) => this.onPointerDown(event);
    this.pointerMove = (event) => this.onPointerMove(event);
    this.pointerUp = (event) => this.onPointerUp(event, false);
    this.pointerCancel = (event) => this.onPointerUp(event, true);
    this.wheel = (event) => this.onWheel(event);
    this.contextMenu = (event) => {
      event.preventDefault();
      dispatchCommand(command(CommandName.TOGGLE_MENU), {
        source: "mouse",
        detail: "contextmenu",
      });
    };
    this.resize = () => {
      clearTimeout(this.resizeTimer);
      this.resizeTimer = setTimeout(() => {
        const forceSinglePage = shouldForceSinglePageForViewport();
        const plan = viewerResizePlan({
          hasContainer: Boolean(state.container),
          forceSinglePageChanged: forceSinglePage !== state.forceSinglePage,
          panelOpen: Boolean(state.commandMenu?.isOpen()),
        });
        if (plan.refreshContainer) {
          refreshContainerSpread(forceSinglePage).catch(renderError);
          return;
        }
        updateViewerImage(performance.now()).catch(renderError);
      }, 180);
    };

    stage.addEventListener("pointerdown", this.pointerDown);
    stage.addEventListener("pointermove", this.pointerMove);
    stage.addEventListener("pointerup", this.pointerUp);
    stage.addEventListener("pointercancel", this.pointerCancel);
    stage.addEventListener("wheel", this.wheel, { passive: false });
    stage.addEventListener("contextmenu", this.contextMenu);
    window.addEventListener("resize", this.resize);
  }

  load({
    name,
    request,
    info,
    fitMode,
    index,
    count,
    seekState,
    interactionStartedAt,
  }) {
    this.resetTransform();
    this.title.textContent = name;
    this.image.alt = name;
    this.setSeekState(seekState ?? {
      visible: count > 1,
      min: 0,
      max: Math.max(0, count - 1),
      value: index,
      label: `${index + 1} / ${count}`,
    });
    return this.loadMeasuredImage(request, interactionStartedAt, name, info);
  }

  loadGroup({
    pages,
    name,
    fitMode,
    gap,
    index,
    count,
    seekState,
    interactionStartedAt,
  }) {
    if (pages.length === 1) {
      return this.load({
        name,
        request: pages[0].request,
        info: pages[0].info,
        fitMode,
        index,
        count,
        seekState,
        interactionStartedAt,
      });
    }
    this.resetTransform();
    this.title.textContent = name;
    this.setSeekState(seekState ?? {
      visible: count > 1,
      min: 0,
      max: Math.max(0, count - 1),
      value: index,
      label: `${index + 1} / ${count}`,
    });
    return this.loadMeasuredSpread(pages, fitMode, gap, interactionStartedAt);
  }

  setBarsVisible(visible) {
    if (visible) this.root.classList.remove("viewer-bars-hidden");
    else this.root.classList.add("viewer-bars-hidden");
  }

  setSeekState(seekState) {
    if (!seekState) return;
    this.counter.textContent = seekState.label;
    if (!this.seek || !this.seekInput) return;
    this.seekInput.hidden = !seekState.visible;
    this.seekInput.min = String(seekState.min);
    this.seekInput.max = String(seekState.max);
    this.seekInput.value = String(seekState.value);
    this.seekInput.disabled = seekState.max <= seekState.min;
    this.seekInput.setAttribute("aria-valuetext", seekState.label);
  }

  setLayout(fitMode, layout, info, image = this.image) {
    this.fitMode = fitMode;
    this.stage.dataset.fitMode = fitMode;
    this.pageLayer.style.gap = "0px";
    image.style.width = `${layout.cssWidth}px`;
    image.style.height = `${layout.cssHeight}px`;
    image.style.maxWidth = "none";
    image.style.maxHeight = "none";
    this.setPageLayerSize(layout.cssWidth, layout.cssHeight);
    image.dataset.sourceWidth = String(info.width);
    image.dataset.sourceHeight = String(info.height);
    this.stage.scrollTop = 0;
    this.stage.scrollLeft = 0;
  }

  setPageLayerSize(width, height) {
    this.pageLayer.style.width = `${Math.max(1, Number(width) || 1)}px`;
    this.pageLayer.style.height = `${Math.max(1, Number(height) || 1)}px`;
  }

  refitVisibleContent(fitMode = FitMode.PAGE, { resetTransform = true } = {}) {
    const sources = this.images.map((image) => ({
      width: Number(image.dataset.sourceWidth) || image.naturalWidth,
      height: Number(image.dataset.sourceHeight) || image.naturalHeight,
    }));
    if (sources.some((source) => !(source.width > 0 && source.height > 0))) return false;
    const layout = viewerSpreadLayout({
      mode: fitMode,
      pages: sources,
      viewportWidth: this.stage.clientWidth || window.innerWidth,
      viewportHeight: this.stage.clientHeight || window.innerHeight,
      devicePixelRatio: window.devicePixelRatio || 1,
      gap: this.images.length > 1 ? Number.parseFloat(this.pageLayer.style.gap) || 0 : 0,
    });
    this.fitMode = fitMode;
    this.stage.dataset.fitMode = fitMode;
    this.pageLayer.style.gap = `${layout.gap}px`;
    this.setPageLayerSize(layout.cssWidth, layout.cssHeight);
    this.images.forEach((image, index) => {
      const page = layout.pages[index];
      image.style.width = `${page.cssWidth}px`;
      image.style.height = `${page.cssHeight}px`;
      image.style.maxWidth = "none";
      image.style.maxHeight = "none";
    });
    this.stage.scrollTop = 0;
    this.stage.scrollLeft = 0;
    if (resetTransform) {
      this.scale = 1;
      this.panX = 0;
      this.panY = 0;
    }
    this.applyTransform();
    return true;
  }

  /// 補正プレビューだけを差し替える。表示レイアウトとズームは維持する。
  async replacePageBlob(pageIndex, blob, alt = "画像補正プレビュー") {
    return this.replacePageBlobs([{ pageIndex, blob, alt }]);
  }

  /// すべて decode できた後に一度だけ DOM を更新する。指定されていないページは保持する。
  async replacePageBlobs(replacements) {
    const previousImages = this.images.slice();
    const prepared = [];
    try {
      for (const replacement of replacements) {
        const pageIndex = Number(replacement.pageIndex);
        const previous = previousImages[pageIndex];
        if (!previous || !(replacement.blob instanceof Blob)) continue;
        const objectUrl = URL.createObjectURL(replacement.blob);
        const image = element("img", "viewer-image");
        image.alt = replacement.alt || previous.alt;
        image.draggable = false;
        image.dataset.telemetryObserved = "true";
        image.style.cssText = previous.style.cssText;
        for (const [key, value] of Object.entries(previous.dataset)) {
          image.dataset[key] = value;
        }
        image.src = objectUrl;
        prepared.push({ pageIndex, image, objectUrl });
      }
      await Promise.all(prepared.map(({ image }) => image.decode()));
    } catch (error) {
      prepared.forEach(({ objectUrl }) => URL.revokeObjectURL(objectUrl));
      throw error;
    }
    if (
      !prepared.length ||
      previousImages.length !== this.images.length ||
      previousImages.some((image, index) => this.images[index] !== image)
    ) {
      prepared.forEach(({ objectUrl }) => URL.revokeObjectURL(objectUrl));
      return false;
    }
    const nextImages = previousImages.slice();
    const nextUrls = this.objectUrls.slice();
    const replacedUrls = [];
    for (const { pageIndex, image, objectUrl } of prepared) {
      nextImages[pageIndex] = image;
      if (nextUrls[pageIndex]) replacedUrls.push(nextUrls[pageIndex]);
      nextUrls[pageIndex] = objectUrl;
    }
    this.pageLayer.replaceChildren(...nextImages);
    this.images = nextImages;
    this.image = nextImages[0];
    this.objectUrls = nextUrls;
    this.objectUrl = nextImages.length === 1 ? nextUrls[0] : null;
    replacedUrls.forEach((url) => URL.revokeObjectURL(url));
    return true;
  }

  async loadMeasuredImage(request, interactionStartedAt, name, info) {
    const sequence = ++this.loadSequence;
    this.fetchController?.abort();
    const controller = new AbortController();
    this.fetchController = controller;
    const fetchStartedAt = performance.now();
    this.beginLoadingIndicator(sequence, fetchStartedAt);
    let pendingObjectUrl = null;
    try {
      let resource;
      if (request.cacheKey) {
        resource = await pageResourceCache.loadForeground(request, controller.signal);
      } else {
        const response = await observedFetch(request.url, {
          signal: controller.signal,
          credentials: "same-origin",
        });
        if (!response.ok) {
          const detail = await response.clone().json().catch(() => ({}));
          throw new Error(
            detail.message ||
              `画像取得に失敗しました (HTTP ${response.status})。`
          );
        }
        resource = {
          blob: await response.blob(),
          requestId: response.headers.get("X-mIV-Request-Id"),
          prefetchStatus: "not_applicable",
        };
      }
      const blob = resource.blob;
      const fetchMs = performance.now() - fetchStartedAt;
      const requestId = resource.requestId;
      if (sequence !== this.loadSequence) return;

      pendingObjectUrl = URL.createObjectURL(blob);
      const decodedImage = element("img", "viewer-image");
      decodedImage.alt = name;
      decodedImage.draggable = false;
      decodedImage.dataset.telemetryObserved = "true";
      decodedImage.src = pendingObjectUrl;
      const decodeStartedAt = performance.now();
      await decodedImage.decode();
      const decodeMs = performance.now() - decodeStartedAt;
      let resolvedInfo = info;
      let resolvedLayout = request.layout;
      if (request.dynamicInfo && decodedImage.naturalWidth && decodedImage.naturalHeight) {
        const actualInfo = {
          width: decodedImage.naturalWidth,
          height: decodedImage.naturalHeight,
        };
        resolvedLayout = viewerImageLayout({
          mode: request.fitMode,
          sourceWidth: actualInfo.width,
          sourceHeight: actualInfo.height,
          viewportWidth: this.stage.clientWidth || window.innerWidth,
          viewportHeight: this.stage.clientHeight || window.innerHeight,
          devicePixelRatio: request.dpr,
          maxRequestWidth: 8192,
        });
        resolvedInfo = actualInfo;
        request.cssWidth = resolvedLayout.cssWidth;
        rememberMediaImageInfo(request, actualInfo);
      }
      if (sequence !== this.loadSequence) {
        URL.revokeObjectURL(pendingObjectUrl);
        return;
      }
      this.setLayout(request.fitMode, resolvedLayout, resolvedInfo, decodedImage);
      decodedImage.style.transform = "none";
      const previousUrls = this.objectUrls.slice();
      this.pageLayer.replaceChildren(decodedImage);
      this.image = decodedImage;
      this.images = [decodedImage];
      this.objectUrl = pendingObjectUrl;
      this.objectUrls = [pendingObjectUrl];
      pendingObjectUrl = null;
      previousUrls.forEach((url) => URL.revokeObjectURL(url));
      this.endLoadingIndicator(sequence);
      await nextFrame();
      if (sequence !== this.loadSequence) return false;

      const event = {
        type: "image",
        request_id: requestId,
        name: limitText(name, 240),
        fetch_ms: roundMs(fetchMs),
        bytes: blob.size,
        decode_ms: roundMs(decodeMs),
        tap_to_display_ms: roundMs(performance.now() - interactionStartedAt),
        requested_width: request.width,
        css_width: roundMs(request.cssWidth),
        device_pixel_ratio: roundMs(request.dpr),
        fit_mode: request.fitMode,
        prefetch_status: resource.prefetchStatus,
      };
      enqueueTelemetry(event);
      hudState.lastImage = event;
      hudState.displayDurations.push(event.tap_to_display_ms);
      if (hudState.displayDurations.length > 20) hudState.displayDurations.shift();
      updateHud();
      return true;
    } catch (error) {
      if (pendingObjectUrl) URL.revokeObjectURL(pendingObjectUrl);
      this.endLoadingIndicator(sequence);
      if (sequence !== this.loadSequence) return false;
      if (error?.name === "AbortError") return false;
      this.title.textContent =
        error instanceof Error ? error.message : "ページを表示できませんでした。";
      this.root.classList.remove("viewer-ui-hidden");
      recordClientError("image_load_error", error, {
        resource: safeResourcePath(request.url),
      });
      return false;
    }
  }

  async loadMeasuredSpread(pages, fitMode, gap, interactionStartedAt) {
    const sequence = ++this.loadSequence;
    this.fetchController?.abort();
    const controller = new AbortController();
    this.fetchController = controller;
    const startedAt = performance.now();
    this.beginLoadingIndicator(sequence, startedAt);
    const pendingUrls = [];
    try {
      const resources = await Promise.all(pages.map(async ({ request }) => {
        if (request.cacheKey) {
          return pageResourceCache.loadForeground(request, controller.signal);
        }
        const fetchStartedAt = performance.now();
        const response = await observedFetch(request.url, {
          signal: controller.signal,
          credentials: "same-origin",
        });
        if (!response.ok) {
          const detail = await response.clone().json().catch(() => ({}));
          throw new Error(detail.message || `画像取得に失敗しました (HTTP ${response.status})。`);
        }
        return {
          blob: await response.blob(),
          requestId: response.headers.get("X-mIV-Request-Id"),
          fetchMs: performance.now() - fetchStartedAt,
          prefetchStatus: "not_applicable",
        };
      }));
      if (sequence !== this.loadSequence) return false;

      const decodedImages = await Promise.all(resources.map(async (resource, index) => {
        const decodedImage = element("img", "viewer-image");
        decodedImage.alt = pages[index].entry.name;
        decodedImage.draggable = false;
        decodedImage.dataset.telemetryObserved = "true";
        const objectUrl = URL.createObjectURL(resource.blob);
        pendingUrls.push(objectUrl);
        decodedImage.src = objectUrl;
        const decodeStartedAt = performance.now();
        await decodedImage.decode();
        return {
          image: decodedImage,
          decodeMs: performance.now() - decodeStartedAt,
          info: decodedImage.naturalWidth && decodedImage.naturalHeight
            ? { width: decodedImage.naturalWidth, height: decodedImage.naturalHeight }
            : pages[index].info,
        };
      }));
      if (sequence !== this.loadSequence) {
        pendingUrls.forEach((url) => URL.revokeObjectURL(url));
        return false;
      }

      const resolvedLayout = viewerSpreadLayout({
        mode: fitMode,
        pages: decodedImages.map((decoded) => decoded.info),
        viewportWidth: this.stage.clientWidth || window.innerWidth,
        viewportHeight: this.stage.clientHeight || window.innerHeight,
        devicePixelRatio: window.devicePixelRatio || 1,
        gap,
      });
      this.fitMode = fitMode;
      this.stage.dataset.fitMode = fitMode;
      this.pageLayer.style.gap = `${resolvedLayout.gap}px`;
      this.setPageLayerSize(resolvedLayout.cssWidth, resolvedLayout.cssHeight);
      decodedImages.forEach((decoded, index) => {
        const layout = resolvedLayout.pages[index];
        decoded.image.style.width = `${layout.cssWidth}px`;
        decoded.image.style.height = `${layout.cssHeight}px`;
        decoded.image.style.maxWidth = "none";
        decoded.image.style.maxHeight = "none";
        decoded.image.style.transform = "none";
        decoded.image.dataset.sourceWidth = String(decoded.info.width);
        decoded.image.dataset.sourceHeight = String(decoded.info.height);
        pages[index].request.cssWidth = layout.cssWidth;
        rememberMediaImageInfo(pages[index].request, decoded.info);
      });
      this.stage.scrollTop = 0;
      this.stage.scrollLeft = 0;
      const previousUrls = this.objectUrls.slice();
      this.pageLayer.replaceChildren(...decodedImages.map((decoded) => decoded.image));
      this.images = decodedImages.map((decoded) => decoded.image);
      this.image = this.images[0];
      this.objectUrls = pendingUrls.slice();
      this.objectUrl = null;
      previousUrls.forEach((url) => URL.revokeObjectURL(url));
      this.applyTransform();
      this.endLoadingIndicator(sequence);
      await nextFrame();
      if (sequence !== this.loadSequence) return false;

      decodedImages.forEach((decoded, index) => {
        const resource = resources[index];
        const request = pages[index].request;
        const event = {
          type: "image",
          request_id: resource.requestId,
          name: limitText(pages[index].entry.name, 240),
          fetch_ms: roundMs(resource.fetchMs ?? performance.now() - startedAt),
          bytes: resource.blob.size,
          decode_ms: roundMs(decoded.decodeMs),
          tap_to_display_ms: roundMs(performance.now() - interactionStartedAt),
          requested_width: request.width,
          css_width: roundMs(request.cssWidth),
          device_pixel_ratio: roundMs(request.dpr),
          fit_mode: request.fitMode,
          prefetch_status: resource.prefetchStatus,
          spread_pages: pages.length,
        };
        enqueueTelemetry(event);
        hudState.lastImage = event;
        hudState.displayDurations.push(event.tap_to_display_ms);
      });
      while (hudState.displayDurations.length > 20) hudState.displayDurations.shift();
      updateHud();
      return true;
    } catch (error) {
      pendingUrls.forEach((url) => URL.revokeObjectURL(url));
      this.endLoadingIndicator(sequence);
      if (sequence !== this.loadSequence || error?.name === "AbortError") return false;
      this.title.textContent = error instanceof Error
        ? error.message
        : "見開きを表示できませんでした。";
      this.root.classList.remove("viewer-ui-hidden");
      recordClientError("spread_load_error", error);
      return false;
    }
  }

  beginLoadingIndicator(sequence, startedAt) {
    clearTimeout(this.loadingTimer);
    this.loadingIndicator.hidden = true;
    this.loadingTimer = setTimeout(() => {
      if (
        sequence === this.loadSequence &&
        shouldShowLoadingIndicator(
          true,
          performance.now() - startedAt,
          PAGE_LOADING_INDICATOR_DELAY_MS
        )
      ) {
        this.loadingIndicator.hidden = false;
      }
    }, PAGE_LOADING_INDICATOR_DELAY_MS);
  }

  endLoadingIndicator(sequence) {
    if (sequence !== this.loadSequence) return;
    clearTimeout(this.loadingTimer);
    this.loadingTimer = 0;
    this.loadingIndicator.hidden = true;
  }

  showBoundaryMessage(message) {
    clearTimeout(this.boundaryMessageTimer);
    this.boundaryMessage.textContent = message;
    this.boundaryMessage.hidden = false;
    this.boundaryMessageTimer = setTimeout(() => {
      this.boundaryMessage.hidden = true;
      this.boundaryMessageTimer = 0;
    }, PAGE_BOUNDARY_MESSAGE_DURATION_MS);
  }

  hideBoundaryMessage() {
    clearTimeout(this.boundaryMessageTimer);
    this.boundaryMessageTimer = 0;
    this.boundaryMessage.hidden = true;
  }

  resetTransform() {
    this.scale = 1;
    this.panX = 0;
    this.panY = 0;
    this.applyTransform();
  }

  execute(requested) {
    const next = reduceViewerTransform(
      { scale: this.scale, panX: this.panX, panY: this.panY },
      requested
    );
    if (!next) return false;
    this.scale = next.scale;
    this.panX = next.panX;
    this.panY = next.panY;
    this.applyTransform();
    return true;
  }

  applyTransform() {
    this.pageLayer.style.transform = `translate3d(${this.panX}px, ${this.panY}px, 0) scale(${this.scale})`;
  }

  cancelPendingCenterTap() {
    clearTimeout(this.centerTapTimer);
    this.centerTapTimer = 0;
    state.viewerTapSequence = null;
  }

  commitPendingCenterTap() {
    const pending = state.viewerTapSequence;
    if (!pending || pending.zone !== "center") return;
    this.cancelPendingCenterTap();
    dispatchCommand(command(CommandName.TOGGLE_VIEWER_BARS), {
      source: pending.inputSource,
      detail: "tap_center",
    });
  }

  schedulePendingCenterTap(pending) {
    clearTimeout(this.centerTapTimer);
    this.centerTapTimer = setTimeout(() => {
      this.centerTapTimer = 0;
      if (state.viewerTapSequence !== pending) return;
      state.viewerTapSequence = null;
      dispatchCommand(command(CommandName.TOGGLE_VIEWER_BARS), {
        source: pending.inputSource,
        detail: "tap_center",
      });
    }, 320);
  }

  onPointerDown(event) {
    if (["mouse", "pen"].includes(event.pointerType) && event.button !== 0) return;
    this.stage.setPointerCapture?.(event.pointerId);
    this.pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (this.pointers.size === 1) {
      this.single = {
        startX: event.clientX,
        startY: event.clientY,
        lastX: event.clientX,
        lastY: event.clientY,
        startedAt: performance.now(),
        edgeGuarded: event.clientX <= 32,
        moved: false,
        contentScrolled: false,
      };
      this.pinched = false;
    } else if (this.pointers.size === 2) {
      const [first, second] = [...this.pointers.values()];
      this.pinch = {
        distance: distance(first, second),
        scale: this.scale,
        center: midpoint(first, second),
        panX: this.panX,
        panY: this.panY,
      };
      this.pinched = true;
    }
  }

  onPointerMove(event) {
    if (!this.pointers.has(event.pointerId)) return;
    const previous = this.pointers.get(event.pointerId);
    this.pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });

    if (this.pointers.size >= 2 && this.pinch) {
      const [first, second] = [...this.pointers.values()];
      const center = midpoint(first, second);
      const ratio = distance(first, second) / Math.max(1, this.pinch.distance);
      dispatchCommand(
        command(CommandName.SET_TRANSFORM, {
          scale: clamp(this.pinch.scale * ratio, 1, 6),
          panX: this.pinch.panX + center.x - this.pinch.center.x,
          panY: this.pinch.panY + center.y - this.pinch.center.y,
        }),
        {
          source: pointerInputSource(event.pointerType),
          detail: "pinch_move",
          telemetry: false,
        }
      );
      return;
    }

    if (this.scale > 1.01 && this.single && previous) {
      dispatchCommand(
        command(CommandName.PAN_BY, {
          dx: event.clientX - previous.x,
          dy: event.clientY - previous.y,
        }),
        {
          source: pointerInputSource(event.pointerType),
          detail: "pan_move",
          telemetry: false,
        }
      );
      this.single.lastX = event.clientX;
      this.single.lastY = event.clientY;
      this.single.moved = true;
    } else if (this.fitMode === FitMode.WIDTH && this.single && previous) {
      const dragDeltaY = event.clientY - previous.y;
      const scroll = viewerVerticalScrollDecision({
        scrollTop: this.stage.scrollTop,
        scrollHeight: this.stage.scrollHeight,
        clientHeight: this.stage.clientHeight,
        dragDeltaY,
      });
      const before = this.stage.scrollTop;
      if (scroll.canConsume) this.stage.scrollTop -= dragDeltaY;
      this.single.lastX = event.clientX;
      this.single.lastY = event.clientY;
      if (Math.abs(this.stage.scrollTop - before) > 0.5) {
        this.single.moved = true;
        this.single.contentScrolled = true;
      }
    }
  }

  onPointerUp(event, cancelled) {
    if (!this.pointers.has(event.pointerId)) return;
    const single = this.single;
    this.pointers.delete(event.pointerId);
    if (this.stage.hasPointerCapture?.(event.pointerId)) {
      this.stage.releasePointerCapture(event.pointerId);
    }

    if (this.pointers.size === 1) {
      const [remaining] = [...this.pointers.values()];
      this.single = {
        startX: remaining.x,
        startY: remaining.y,
        lastX: remaining.x,
        lastY: remaining.y,
        startedAt: performance.now(),
        edgeGuarded: false,
        moved: false,
        contentScrolled: false,
      };
      this.pinch = null;
      return;
    }
    if (this.pointers.size > 0) return;

    const source = pointerInputSource(event.pointerType);
    if (!this.pinched && single) {
      const dx = event.clientX - single.startX;
      const dy = event.clientY - single.startY;
      const elapsed = performance.now() - single.startedAt;
      const gesture = viewerGestureDecision({
        dx,
        dy,
        elapsedMs: elapsed,
        moved: single.moved,
        zoomed: this.scale > 1.01,
        contentScrolled: single.contentScrolled,
        edgeGuarded: single.edgeGuarded,
        cancelled,
      });
      const stageBounds = this.stage.getBoundingClientRect?.() ?? {
        top: 0,
        bottom: this.stage.clientHeight || window.innerHeight,
      };
      const panelAction = viewerPanelGestureAction({
        gesture,
        panelOpen: false,
        startY: single.startY,
        contentTop: stageBounds.top,
        contentBottom: stageBounds.bottom,
      });
      if (
        gesture === ViewerGesture.SWIPE_LEFT ||
        gesture === ViewerGesture.SWIPE_RIGHT
      ) {
        const swipeLeft = gesture === ViewerGesture.SWIPE_LEFT;
        dispatchCommand(
          command(
            swipeLeft
              ? (isRtlReadingDirection(state.readingDirection)
                  ? CommandName.PREV_PAGE
                  : CommandName.NEXT_PAGE)
              : (isRtlReadingDirection(state.readingDirection)
                  ? CommandName.NEXT_PAGE
                  : CommandName.PREV_PAGE)
          ),
          { source, detail: swipeLeft ? "swipe_left" : "swipe_right" }
        );
      } else if (panelAction === ViewerPanelAction.OPEN) {
        dispatchCommand(command(CommandName.TOGGLE_MENU), {
          source,
          detail: "swipe_up",
        });
      } else if (gesture === ViewerGesture.SWIPE_DOWN) {
        dispatchCommand(command(CommandName.BACK), {
          source,
          detail: "swipe_down",
        });
      } else if (gesture === ViewerGesture.TAP) {
        const tapAt = performance.now();
        const transition = viewerTapSequenceTransition(state.viewerTapSequence, {
          x: event.clientX,
          y: event.clientY,
          atMs: tapAt,
          width: this.root.clientWidth,
          inputSource: source,
        });
        if (transition.commitPrevious) this.commitPendingCenterTap();
        state.viewerTapSequence = transition.next;
        if (transition.action === "double_tap") {
          this.cancelPendingCenterTap();
          dispatchCommand(command(CommandName.FIT_TOGGLE_PAGE_ORIGINAL), {
            source,
            detail: "double_tap_fit",
          });
        } else if (transition.action === "pending_center_tap") {
          this.schedulePendingCenterTap(transition.next);
        } else {
          // Edge taps and non-touch taps are immediate. Only a touch in the center waits so the
          // same two taps cannot also toggle the bars before changing magnification.
          dispatchCommand(viewerTapCommand(
            event.clientX,
            this.root.clientWidth,
            isRtlReadingDirection(state.readingDirection)
          ), {
            source,
            detail: "tap_zone",
          });
        }
      } else if (gesture === ViewerGesture.PAN) {
        dispatchCommand(command(CommandName.PAN_BY, { dx: 0, dy: 0 }), {
          source,
          detail: "pan",
        });
      }
      if (gesture !== ViewerGesture.TAP) this.commitPendingCenterTap();
    } else if (!cancelled && this.pinched) {
      this.commitPendingCenterTap();
      dispatchCommand(
        command(CommandName.SET_TRANSFORM, {
          scale: this.scale,
          panX: this.panX,
          panY: this.panY,
        }),
        { source, detail: "pinch" }
      );
    }
    this.single = null;
    this.pinch = null;
    this.pinched = false;
  }

  onWheel(event) {
    event.preventDefault();
    const zoomModifier = event.ctrlKey || event.metaKey;
    if (zoomModifier) {
      dispatchCommand(viewerWheelCommand(event.deltaY, true), {
        source: "mouse",
        detail: "wheel_zoom",
      });
      return;
    }
    if (this.fitMode === FitMode.WIDTH) {
      const delta =
        event.deltaMode === 1
          ? event.deltaY * 16
          : event.deltaMode === 2
            ? event.deltaY * this.stage.clientHeight
            : event.deltaY;
      this.stage.scrollTop += delta;
      return;
    }
    const delta =
      event.deltaMode === 1
        ? event.deltaY * 16
        : event.deltaMode === 2
          ? event.deltaY * this.stage.clientHeight
          : event.deltaY;
    this.wheelDelta += delta;
    const now = performance.now();
    if (Math.abs(this.wheelDelta) < 48 || now - this.lastWheelCommandAt < 220) return;
    dispatchCommand(viewerWheelCommand(this.wheelDelta, false), {
      source: "mouse",
      detail: "wheel_page",
    });
    this.wheelDelta = 0;
    this.lastWheelCommandAt = now;
  }

  destroy() {
    clearTimeout(this.resizeTimer);
    clearTimeout(this.loadingTimer);
    clearTimeout(this.boundaryMessageTimer);
    clearTimeout(this.centerTapTimer);
    this.centerTapTimer = 0;
    this.loadingIndicator.hidden = true;
    this.boundaryMessage.hidden = true;
    this.loadSequence += 1;
    this.fetchController?.abort();
    if (this.objectUrl) URL.revokeObjectURL(this.objectUrl);
    for (const objectUrl of this.objectUrls) {
      if (objectUrl !== this.objectUrl) URL.revokeObjectURL(objectUrl);
    }
    this.objectUrl = null;
    this.objectUrls = [];
    this.stage.removeEventListener("pointerdown", this.pointerDown);
    this.stage.removeEventListener("pointermove", this.pointerMove);
    this.stage.removeEventListener("pointerup", this.pointerUp);
    this.stage.removeEventListener("pointercancel", this.pointerCancel);
    this.stage.removeEventListener("wheel", this.wheel);
    this.stage.removeEventListener("contextmenu", this.contextMenu);
    window.removeEventListener("resize", this.resize);
  }
}

async function apiJson(path, params = {}, signal) {
  const response = await observedFetch(apiUrl(path, params), {
    method: "GET",
    credentials: "same-origin",
    headers: { Accept: "application/json" },
    signal,
  });
  if (response.status === 401) {
    state.authenticated = false;
    telemetryState.authenticated = false;
    renderPinLogin(0);
    throw new AuthenticationRequiredError("PIN 認証が必要です。");
  }
  if (!response.ok) {
    const detail = await response.clone().json().catch(() => ({}));
    const error = new Error(
      detail.message || `読み込みに失敗しました (HTTP ${response.status})。`
    );
    error.status = response.status;
    error.code = detail.error;
    error.retryAfterSeconds = Number(response.headers.get("Retry-After")) || 1;
    throw error;
  }
  return response.json();
}

async function apiPostJson(path, body, signal) {
  const response = await observedFetch(path, {
    method: "POST",
    credentials: "same-origin",
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
    signal,
  });
  if (response.status === 401) {
    state.authenticated = false;
    telemetryState.authenticated = false;
    renderPinLogin(0);
    throw new AuthenticationRequiredError("PIN 認証が必要です。");
  }
  if (!response.ok) {
    const detail = await response.clone().json().catch(() => ({}));
    const error = new Error(
      detail.message || `保存に失敗しました (HTTP ${response.status})。`
    );
    error.status = response.status;
    error.code = detail.error;
    error.retryAfterSeconds = Number(response.headers.get("Retry-After")) || 1;
    throw error;
  }
  return response.json();
}

function apiUrl(path, params = {}) {
  const url = new URL(path, location.origin);
  for (const [key, value] of Object.entries(params)) {
    url.searchParams.set(key, String(value));
  }
  return `${url.pathname}${url.search}`;
}

function renderLoading(message, preserveRequestController = null) {
  cleanupScreen(preserveRequestController);
  state.screenContext = "loading";
  const status = element("div", "center-status");
  status.append(element("div", "spinner"), textElement("div", message));
  app.append(status);
}

function renderError(error) {
  if (error?.name === "AbortError" || error instanceof AuthenticationRequiredError) return;
  cleanupScreen();
  state.screenContext = "error";
  const status = element("div", "center-status");
  status.append(
    textElement("div", "表示できません", "error-title"),
    textElement(
      "p",
      error instanceof Error ? error.message : "不明なエラーが発生しました。",
      "status-detail"
    )
  );
  const home = textElement("button", "ホームへ戻る", "icon-button");
  home.type = "button";
  home.addEventListener("click", () => navigate(homeHash("places")));
  status.append(home);
  app.append(status);
}

function element(tag, className) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  return node;
}

function textElement(tag, text, className) {
  const node = element(tag, className);
  node.textContent = text;
  return node;
}

function parentPath(path) {
  const separator = path.lastIndexOf("/");
  return separator >= 0 ? path.slice(0, separator) : "";
}

function distance(first, second) {
  return Math.hypot(first.x - second.x, first.y - second.y);
}

function midpoint(first, second) {
  return { x: (first.x + second.x) / 2, y: (first.y + second.y) / 2 };
}

function clamp(value, minimum, maximum) {
  return Math.max(minimum, Math.min(maximum, value));
}

function tryEnterBrowserFullscreen() {
  if (!document.fullscreenElement && document.documentElement.requestFullscreen) {
    document.documentElement.requestFullscreen({ navigationUI: "hide" }).catch(() => {});
  }
}

function exitBrowserFullscreen() {
  if (document.fullscreenElement && document.exitFullscreen) {
    document.exitFullscreen().catch(() => {});
  }
}

function toggleBrowserFullscreen() {
  if (document.fullscreenElement) {
    exitBrowserFullscreen();
  } else {
    tryEnterBrowserFullscreen();
  }
}

// Standalone windows do not expose browser chrome. Reload is intentionally local:
// it must work even after the server-side session or authentication cookie expires.
export function reloadApplication() {
  window.location.reload();
}

function installTelemetry() {
  hudElement.hidden = false;
  hudElement.addEventListener("click", () => {
    hudElement.hidden = true;
  });
  updateHud();

  window.addEventListener(
    "error",
    (event) => {
      if (event.target instanceof HTMLImageElement) {
        if (event.target.dataset.telemetryObserved === "true") return;
        recordClientError("image_load_error", "<img> load failed", {
          resource: safeResourcePath(event.target.currentSrc || event.target.src),
        });
        return;
      }
      recordClientError("window_error", event.error ?? event.message, {
        resource: safeResourcePath(event.filename),
        line: event.lineno,
        column: event.colno,
      });
    },
    true
  );
  window.addEventListener("unhandledrejection", (event) => {
    recordClientError("unhandled_rejection", event.reason);
  });
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") flushTelemetry(true);
  });
  window.setInterval(() => {
    flushTelemetry(false);
    updateHud();
  }, 5000);
}

async function observedFetch(url, options = {}) {
  options = { ...options, headers: remoteHeaders(options.headers) };
  let response;
  try {
    response = await fetch(url, options);
  } catch (error) {
    if (error?.name !== "AbortError") {
      recordClientError("fetch_error", error, {
        resource: safeResourcePath(url),
      });
    }
    throw error;
  }
  if (!response.ok) {
    if (response.status === 409 || response.status === 428) {
      const detail = await response.clone().json().catch(() => ({}));
      setRemoteSessionStatus(
        sessionStatusFromResponse(detail.status, response.status),
        detail.message || "操作権がありません。次の操作時に再接続します。"
      );
    }
    recordClientError(
      "fetch_non_2xx",
      new Error(`HTTP ${response.status} ${response.statusText}`),
      {
        resource: safeResourcePath(url),
        status: response.status,
      }
    );
  }
  return response;
}

async function fetchPageResource(request, signal, prefetch) {
  const startedAt = performance.now();
  const options = {
    signal,
    credentials: "same-origin",
    headers: remoteHeaders({ Accept: "image/*" }),
    ...(prefetch ? { priority: "low" } : {}),
  };
  const response = prefetch
    ? await fetch(request.url, options)
    : await observedFetch(request.url, options);
  if (!response.ok) {
    const detail = await response.clone().json().catch(() => ({}));
    const error = new Error(
      detail.message || `画像取得に失敗しました (HTTP ${response.status})。`
    );
    error.status = response.status;
    error.code = detail.error;
    throw error;
  }
  const width = Number(response.headers.get("X-mIV-Image-Width"));
  const height = Number(response.headers.get("X-mIV-Image-Height"));
  return {
    blob: await response.blob(),
    requestId: response.headers.get("X-mIV-Request-Id"),
    fetchMs: performance.now() - startedAt,
    info:
      Number.isFinite(width) && width > 0 && Number.isFinite(height) && height > 0
        ? { width, height }
        : null,
  };
}

function remoteHeaders(initial = {}) {
  const headers = new Headers(initial);
  headers.set("X-mIV-Remote-Client", REMOTE_CLIENT_ID);
  return headers;
}

function loadRemoteClientId() {
  const key = "miv-remote-client-id";
  try {
    const existing = globalThis.localStorage?.getItem(key);
    if (existing && /^[A-Za-z0-9_-]{8,128}$/.test(existing)) return existing;
  } catch {}
  const generated =
    globalThis.crypto?.randomUUID?.() ??
    `client-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  try {
    globalThis.localStorage?.setItem(key, generated);
  } catch {}
  return generated;
}

function enqueueTelemetry(event) {
  if (!TELEMETRY_ENABLED || !telemetryState.authenticated) return;
  telemetryState.queue.push({
    client_event_timestamp_ms: Date.now(),
    ...event,
  });
  if (telemetryState.queue.length > 200) {
    telemetryState.queue.splice(0, telemetryState.queue.length - 200);
  }
}

async function flushTelemetry(useBeacon) {
  if (
    !telemetryState.authenticated ||
    !telemetryState.queue.length ||
    (!useBeacon && telemetryState.flushing)
  )
    return;
  if (useBeacon && navigator.sendBeacon) {
    while (telemetryState.queue.length) {
      const { events, body } = takeTelemetryPayload();
      const accepted = navigator.sendBeacon(
        "/api/telemetry",
        new Blob([body], { type: "application/json" })
      );
      if (!accepted) {
        telemetryState.queue.unshift(...events);
        break;
      }
    }
    return;
  }

  const { events, body } = takeTelemetryPayload();
  telemetryState.flushing = true;
  try {
    const response = await fetch("/api/telemetry", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body,
      keepalive: true,
    });
    if (!response.ok && response.status !== 429) {
      telemetryState.queue.unshift(...events);
      noteHudError();
    }
  } catch {
    telemetryState.queue.unshift(...events);
    noteHudError();
  } finally {
    telemetryState.flushing = false;
  }
}

function takeTelemetryPayload() {
  const events = telemetryState.queue.splice(0, Math.min(24, telemetryState.queue.length));
  const payload = {
    client_timestamp_ms: Date.now(),
    events,
  };
  const connection = connectionInfo();
  if (connection) payload.connection = connection;

  let body = JSON.stringify(payload);
  while (new Blob([body]).size > 60 * 1024 && events.length > 1) {
    telemetryState.queue.unshift(events.pop());
    body = JSON.stringify(payload);
  }
  return { events, body };
}

function connectionInfo() {
  const connection =
    navigator.connection || navigator.mozConnection || navigator.webkitConnection;
  if (!connection) return null;
  const info = {};
  if (typeof connection.effectiveType === "string") {
    info.effective_type = connection.effectiveType;
  }
  if (typeof connection.downlink === "number") info.downlink_mbps = connection.downlink;
  return Object.keys(info).length ? info : null;
}

function recordClientError(category, error, extra = {}) {
  const normalized = normalizeError(error);
  enqueueTelemetry({
    type: "error",
    category,
    message: normalized.message,
    stack: normalized.stack,
    ...extra,
  });
  noteHudError();
}

function normalizeError(error) {
  const message =
    error instanceof Error ? error.message : typeof error === "string" ? error : String(error);
  const stack = error instanceof Error ? error.stack : "";
  return {
    message: limitText(redactTokenQuery(message), 800),
    stack: limitText(
      redactTokenQuery(stack)
        .split("\n")
        .slice(0, 4)
        .join("\n"),
      1800
    ),
  };
}

function noteHudError() {
  hudState.errors.push(Date.now());
  trimHudErrors();
  updateHud();
}

function trimHudErrors() {
  const cutoff = Date.now() - 60_000;
  while (hudState.errors[0] < cutoff) hudState.errors.shift();
}

function updateHud() {
  if (!TELEMETRY_ENABLED) {
    hudElement.hidden = true;
    return;
  }
  trimHudErrors();
  const image = hudState.lastImage;
  const grid = hudState.lastGrid;
  const recent = hudState.displayDurations.slice(-7);
  const lines = ["mIV PoC 計測"];
  lines.push(
    image
      ? `画像 fetch ${formatMs(image.fetch_ms)} / ${formatBytes(image.bytes)}`
      : "画像 fetch — / —"
  );
  lines.push(image ? `decode ${formatMs(image.decode_ms)}` : "decode —");
  lines.push(
    grid
      ? `一覧 ${formatMs(grid.duration_ms)} (${grid.rendered_count}件)`
      : "一覧 —"
  );
  lines.push(
    recent.length
      ? `表示中央値(${recent.length}) ${formatMs(median(recent))}`
      : "表示中央値 —"
  );
  lines.push(`error(60s) ${hudState.errors.length}  · tapで隠す`);
  hudElement.textContent = lines.join("\n");
}

function safeResourcePath(value) {
  if (!value) return "";
  try {
    return new URL(value, location.origin).pathname;
  } catch {
    return limitText(redactTokenQuery(String(value)), 300);
  }
}

function redactTokenQuery(value) {
  return String(value ?? "").replace(/([?&]t=)[^&#\s)]+/gi, "$1[redacted]");
}

function limitText(value, maxLength) {
  const text = String(value ?? "");
  return text.length <= maxLength ? text : `${text.slice(0, maxLength)}…`;
}

function nextFrame() {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

function roundMs(value) {
  return Math.round(Number(value) * 10) / 10;
}

function formatMs(value) {
  return `${Math.round(value)}ms`;
}

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)}KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)}MiB`;
}

function median(values) {
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2
    ? sorted[middle]
    : (sorted[middle - 1] + sorted[middle]) / 2;
}
