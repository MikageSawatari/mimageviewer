import {
  CommandName,
  FitMode,
  GridViewportAnchor,
  GridViewportMemory,
  IMAGE_QUALITY_PRESETS,
  MAX_VIEWER_VISIBLE_PAGES,
  ReadingDirection,
  SpreadMode,
  ViewerGesture,
  ViewerGroupLoadCompletionAction,
  ViewerGroupLoadOutcome,
  ViewerPanelAction,
  VIEWER_PANEL_ANIMATION_MS,
  adjustmentResetVisible,
  addressedPostRequest,
  command,
  commandFromKey,
  createReadingProgressBatch,
  gridColumnOverrideFieldForViewport,
  gridColumnOverrideForViewport,
  gridColumnsAfterPinch,
  gridLabelHeightForEntries,
  gridLayoutForWidth,
  gridScrollExtent,
  gridIndexForCommand,
  imageQualityPreset,
  isRtlReadingDirection,
  latestPageLoadRequestPlan,
  pageLoadQueueBusyTransition,
  nextFitMode,
  nextSpreadMode,
  normalizeVisualViewportScale,
  pageResponseGenerationAttestation,
  pageResponseIdentityAttestation,
  pagePrefetchFailurePlan,
  FOREGROUND_ADMISSION_RETRY_LIMIT,
  pageAdmissionRetryDelayMs,
  pageRequestIsTransientlyBusy,
  pagePrefetchHudPlan,
  pagePrefetchIndicatorSummary,
  pagePrefetchBudgetAllowsStart,
  pagePrefetchPlan,
  pagePrefetchStartCount,
  pageResourceAdmissionPlan,
  pageResourceCacheBudget,
  planSpreadIntent,
  reduceViewerTransform,
  remoteStateGenerationTransition,
  readingProgressBatchTransition,
  remoteSessionAcquireDecision,
  remoteSessionAcquireRetryDelay,
  remoteSessionControlTransition,
  remoteSessionFailureStatus,
  remoteSessionTransitionTelemetry,
  remoteAddressIdentity,
  rangeValueFromNormalized,
  rangeValueToNormalized,
  relativeRangeDragValue,
  seekRangeAbsoluteValue,
  seekRangePointerGestureDecision,
  appUpdateNotice,
  resolveGridReturnViewport,
  sessionOwnerBadge,
  sessionOwnerBadgeTransition,
  snappedGridOffset,
  thumbnailBindingMatches,
  thumbnailRequestConcurrency,
  thumbnailRequestStartCount,
  thumbnailRetryDecision,
  telemetryDeliveryMode,
  telemetryEventForTier,
  telemetrySessionCorrelation,
  shouldShowGridCursor,
  shouldShowLoadingIndicator,
  shouldShowKeyboardShortcuts,
  viewerGestureDecision,
  viewerGroupLoadCompletionPlan,
  viewerDragOwnershipDecision,
  pinchTransformDecision,
  VIEWER_MAX_SCALE,
  VIEWER_MIN_SCALE,
  viewerPanelGestureAction,
  viewerPanelTransition,
  viewerResizePlan,
  ViewerDragOwner,
  viewerTapCommand,
  viewerImageLayout,
  viewerLayoutTelemetry,
  viewerPageDisplayHistoryEvent,
  viewerPageDisplaySlot,
  viewerPageGroupGenerationSnapshot,
  viewerPageGroupRequestMatches,
  ViewerPagePositionEvent,
  viewerPagePositionFeedback,
  viewerPagePositionTransition,
  viewerPostDisplayRefreshPlan,
  viewerSpreadPartnerIndex,
  viewerBoundaryMessage,
  viewerSeekGroupIndex,
  viewerSeekRelativeDragValue,
  viewerSeekState,
  viewerSpreadLayout,
  viewerTransformTelemetry,
  viewerWheelCommand,
  visualViewportScaleTransition,
} from "./command-core.mjs";
import {
  ADJUSTMENT_PANEL_TABS,
  loadLocalSettings,
  saveLocalSettings,
} from "./local-settings.mjs";
import {
  appendPageTimingSample,
  averagePageTimings,
  formatPageTimingAverage,
  loadPageTimingHistory,
  pageTimingSample,
  savePageTimingHistory,
  shouldCountPageTiming,
} from "./page-timings.mjs";
import { installDocumentDoubleTapOwner } from "./document-double-tap.mjs";
import { VideoStreamViewer } from "./video-stream.mjs";

export { ADJUSTMENT_PANEL_TABS };

// index.html の受け皿への合図。ここまで来たということは、必要なファイルが全部読めて
// このモジュールが実行され始めたということ。この後の遅さはアプリ自身が画面で伝える。
if (typeof window !== "undefined") window.__mivRemoteAppStarted = true;

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
// history.go(-N) は範囲外だと何も起こさない。popstate が来ないことでそれを知る。
const VIEWER_EXIT_HISTORY_TIMEOUT_MS = 250;
// Safari では端末メモリを取得できないため、圧縮 JPEG Blob の固定 64 MiB を先読み開始の
// 門にする。12/4 は窓の上限にすぎず、実際の深さは保持済みバイト数で決まる。
const PAGE_PREFETCH_AHEAD = 12;
const PAGE_PREFETCH_BEHIND = 4;
const PAGE_PREFETCH_CONCURRENCY = 2;
const PAGE_RESOURCE_CACHE_LIMIT =
  MAX_VIEWER_VISIBLE_PAGES + PAGE_PREFETCH_AHEAD + PAGE_PREFETCH_BEHIND;
const PAGE_RESOURCE_CACHE_CONFIGURED_BYTES = 64 * 1024 * 1024;
const PAGE_RESOURCE_CACHE_MAX_BYTES = pageResourceCacheBudget({
  configuredBytes: PAGE_RESOURCE_CACHE_CONFIGURED_BYTES,
}).byteLimit;
const CONTAINER_SPREAD_REFRESH_FAILURE_MESSAGE =
  "見開き表示を更新できませんでした。";
export const ContainerSpreadRefreshExitReason = Object.freeze({
  VIEWER_CHANGED_BEFORE_LOAD: "viewer_changed_before_load",
  CONTAINER_MISSING_BEFORE_LOAD: "container_missing_before_load",
  CONTAINER_CHANGED_BEFORE_LOAD: "container_changed_before_load",
  VIEWER_CHANGED_DURING_LOAD: "viewer_changed_during_load",
  CONTAINER_MISSING_DURING_LOAD: "container_missing_during_load",
  CONTAINER_CHANGED_DURING_LOAD: "container_changed_during_load",
  CONTAINER_LOAD_ABORTED: "container_load_aborted",
  CONTAINER_LOAD_FAILED: "container_load_failed",
  CONTAINER_LOAD_NOT_APPLIED: "container_load_not_applied",
  CURRENT_PAGE_MISSING: "current_page_missing",
  GROUP_MISSING: "group_missing",
  DISPLAY_SUPERSEDED: "display_superseded",
  DISPLAY_FAILED: "display_failed",
  UNEXPECTED_ERROR: "unexpected_error",
});
export const ViewerImageUpdateExitReason = Object.freeze({
  GROUP_MISSING_BEFORE_LOAD: "group_missing_before_load",
  VIEWER_MISSING_BEFORE_LOAD: "viewer_missing_before_load",
  VIEWER_CHANGED_BEFORE_GROUP_LOAD: "viewer_changed_before_group_load",
  SESSION_CHANGED_BEFORE_GROUP_LOAD: "session_changed_before_group_load",
  CACHE_EPOCH_CHANGED_BEFORE_GROUP_LOAD: "cache_epoch_changed_before_group_load",
  GROUP_CHANGED_BEFORE_GROUP_LOAD: "group_changed_before_group_load",
  VIEWER_CHANGED_ON_ERROR: "viewer_changed_on_error",
  PRELOAD_FAILED: "preload_failed",
  PRELOAD_ABORTED: "preload_aborted",
  GROUP_LOAD_THROWN: "group_load_thrown",
  GROUP_LOAD_ABORTED: "group_load_aborted",
  GROUP_LOAD_SUPERSEDED: "group_load_superseded",
  LOAD_REQUEST_CHANGED_AFTER_GROUP_LOAD: "load_request_changed_after_group_load",
  VIEWER_CHANGED_AFTER_GROUP_LOAD: "viewer_changed_after_group_load",
});
const SESSION_PING_INTERVAL_MS = 30_000;
const READING_PROGRESS_INTERVAL_MS = 30_000;
const AI_FOREGROUND_POLL_MS = 500;
const ARCHIVE_FOREGROUND_POLL_MS = 500;
const AI_RETRY_DELAYS_MS = Object.freeze([1000, 2000, 5000]);
const AI_TERMINAL_STATES = new Set([
  "ready",
  "superseded",
  "cancelled_by_user",
  "discarded_by_host",
  "background_expired",
  "failed",
]);
const ARCHIVE_TERMINAL_STATES = new Set([
  "ready",
  "declined_by_user",
  "superseded",
  "cancelled_by_user",
  "discarded_by_host",
  "background_expired",
  "failed",
]);
const APP_ASSET_TOKEN_PATTERN = /^[a-f0-9]{16}$/;
const APP_UPDATE_RELOAD_ATTEMPT_KEY = "miv-remote-app-update-reload-attempt";
const RUNTIME_TEST_MODE = globalThis.__MIV_RUNTIME_TEST_MODE__ === true;
let runtimeTestErrorObserver = null;
const REMOTE_CLIENT_ID = loadRemoteClientId();
const LOCAL_SETTINGS_LOAD = loadLocalSettings();
let pageTimingHistory = loadPageTimingHistory().history;

class AuthenticationRequiredError extends Error {}
class RemoteSessionBlockedError extends Error {}

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
  constructor(run, onError = () => {}, sameTask = () => false) {
    this.run = run;
    this.onError = onError;
    this.sameTask = sameTask;
    this.running = false;
    this.active = null;
    this.latest = null;
  }

  enqueue(value) {
    if (this.active && this.sameTask(this.active.value, value)) {
      return this.active.promise;
    }
    if (this.latest && this.sameTask(this.latest.value, value)) {
      return this.latest.promise;
    }
    let resolveTicket;
    const promise = new Promise((resolve) => {
      resolveTicket = resolve;
    });
    const ticket = { value, promise, resolve: resolveTicket };
    if (this.latest) this.latest.resolve(VIEWER_GROUP_LOAD_SUPERSEDED);
    this.latest = ticket;
    if (!this.running) this.pump();
    return promise;
  }

  clear() {
    if (this.latest) this.latest.resolve(VIEWER_GROUP_LOAD_SUPERSEDED);
    this.latest = null;
  }

  async pump() {
    if (this.running) return;
    this.running = true;
    try {
      while (this.latest !== null) {
        const ticket = this.latest;
        this.latest = null;
        this.active = ticket;
        try {
          const result = await this.run(ticket.value);
          if (result === undefined) {
            ticket.resolve(VIEWER_GROUP_LOAD_APPLIED);
          } else {
            viewerGroupLoadCompletionPlan(result);
            ticket.resolve(result);
          }
        } catch (error) {
          let mappedFailure = null;
          if (error?.name !== "AbortError") {
            try {
              mappedFailure = this.onError(error, ticket.value);
            } catch {}
          }
          ticket.resolve(
            mappedFailure?.outcome === ViewerGroupLoadOutcome.FAILED &&
              typeof mappedFailure.message === "string" &&
              mappedFailure.message.trim()
              ? mappedFailure
              : {
                outcome: ViewerGroupLoadOutcome.FAILED,
                message: error instanceof Error ? error.message : String(error),
              }
          );
        } finally {
          if (this.active === ticket) this.active = null;
        }
      }
    } finally {
      this.running = false;
      if (this.latest !== null) this.pump();
    }
  }
}

/// 前景ページ描画専用。実行中の core IPC は完了させ、待機中は最新のページ群だけを残す。
const VIEWER_GROUP_LOAD_SUPERSEDED = Object.freeze({
  outcome: ViewerGroupLoadOutcome.SUPERSEDED,
});
const VIEWER_GROUP_LOAD_APPLIED = Object.freeze({
  outcome: ViewerGroupLoadOutcome.APPLIED,
});

export class LatestPageLoadQueue {
  constructor(
    run,
    onSupersede = () => {},
    onBusyChange = () => {},
    onDiscard = () => {}
  ) {
    this.run = run;
    this.onSupersede = onSupersede;
    this.onBusyChange = onBusyChange;
    this.onDiscard = onDiscard;
    this.active = null;
    this.pending = null;
  }

  isBusy() {
    return this.active !== null || this.pending !== null;
  }

  notifyBusyTransition(previousBusy) {
    const transition = pageLoadQueueBusyTransition(previousBusy, this.isBusy());
    if (transition.action !== "unchanged") this.onBusyChange(transition.busy);
  }

  request(value) {
    return new Promise((resolve, reject) => {
      const wasBusy = this.isBusy();
      const incoming = { value, resolve, reject, superseded: false };
      const plan = latestPageLoadRequestPlan(
        { active: this.active, pending: this.pending },
        incoming
      );
      if (plan.supersededPending) {
        plan.supersededPending.superseded = true;
        this.onDiscard(plan.supersededPending.value, "pending_superseded");
        plan.supersededPending.resolve(VIEWER_GROUP_LOAD_SUPERSEDED);
      }
      this.pending = plan.pending;
      if (plan.supersedeActive) {
        this.active.superseded = true;
        this.onSupersede();
      }
      if (plan.start) this.active = plan.start;
      this.notifyBusyTransition(wasBusy);
      if (plan.start) this.runActive(plan.start);
    });
  }

  clear() {
    const wasBusy = this.isBusy();
    if (this.pending) {
      this.pending.superseded = true;
      this.onDiscard(this.pending.value, "queue_cleared");
      this.pending.resolve(VIEWER_GROUP_LOAD_SUPERSEDED);
      this.pending = null;
    }
    if (this.active) this.active.superseded = true;
    this.notifyBusyTransition(wasBusy);
  }

  async runActive(ticket) {
    try {
      const result = await this.run(ticket.value);
      if (ticket.superseded) {
        ticket.resolve(VIEWER_GROUP_LOAD_SUPERSEDED);
      } else {
        viewerGroupLoadCompletionPlan(result);
        ticket.resolve(result);
      }
    } catch (error) {
      if (ticket.superseded) ticket.resolve(VIEWER_GROUP_LOAD_SUPERSEDED);
      else ticket.reject(error);
    } finally {
      const wasBusy = this.isBusy();
      if (this.active === ticket) this.active = null;
      const next = this.pending;
      this.pending = null;
      if (next) this.active = next;
      this.notifyBusyTransition(wasBusy);
      if (next) this.runActive(next);
    }
  }
}

export class PageResourceCache {
  constructor(
    limit = PAGE_RESOURCE_CACHE_LIMIT,
    byteLimit = PAGE_RESOURCE_CACHE_MAX_BYTES,
    prefetchConcurrency = PAGE_PREFETCH_CONCURRENCY,
    fetchResource = fetchPageResource,
    onStatusChange = () => {}
  ) {
    this.limit = Math.max(1, Number(limit) || 1);
    this.byteLimit = Math.max(1, Number(byteLimit) || 1);
    this.prefetchConcurrency = Math.max(1, Number(prefetchConcurrency) || 1);
    this.fetchResource = fetchResource;
    this.onStatusChange = onStatusChange;
    this.ready = new Map();
    this.readyBytes = 0;
    this.pending = [];
    this.active = new Map();
    this.visibleKeys = new Set();
    this.prefetchProtectedKeys = [];
    this.prefetchProtectLimit = Math.max(
      0,
      this.limit - MAX_VIEWER_VISIBLE_PAGES
    );
    this.retryTimer = 0;
  }

  statusForKeys(keys = []) {
    return (Array.isArray(keys) ? keys : []).map((key) => {
      if (this.ready.has(key)) return "ready";
      const active = this.active.get(key);
      return active && !active.controller.signal.aborted ? "active" : "missing";
    });
  }

  notifyStatusChange(type, key = "") {
    try {
      this.onStatusChange({ type, key });
    } catch {}
  }

  async loadForeground(request, signal) {
    const cached = this.ready.get(request.cacheKey);
    if (cached) {
      this.ready.delete(request.cacheKey);
      this.ready.set(request.cacheKey, cached);
      return { ...cached, prefetchStatus: "hit" };
    }
    const joined = this.active.get(request.cacheKey);
    if (joined) joined.foregroundWaiters += 1;
    for (const active of this.active.values()) {
      if (active.key !== request.cacheKey && active.foregroundWaiters === 0) {
        active.controller.abort();
      }
    }
    if (joined) {
      try {
        const resource = await awaitWithAbort(joined.promise, signal);
        return { ...resource, prefetchStatus: "in_flight" };
      } catch (error) {
        if (
          signal?.aborted ||
          !pageRequestIsTransientlyBusy(error?.status)
        ) throw error;
        // A foreground join must not inherit a temporary prefetch admission failure.
      } finally {
        joined.foregroundWaiters -= 1;
        this.abortUnownedActive(joined);
      }
    }
    this.pending = this.pending.filter((item) => item.cacheKey !== request.cacheKey);
    const resource = await this.fetchForegroundResource(request, signal);
    this.remember(request.cacheKey, resource);
    return { ...resource, prefetchStatus: "miss" };
  }

  /// 表示するページを admission の一時的な満杯で失敗させない。見開きは 2 ページを
  /// 同時に要求するので、先読みが枠を持っている瞬間に 2 枚目だけ弾かれ得る。
  async fetchForegroundResource(request, signal) {
    for (let attempt = 0; ; attempt += 1) {
      try {
        return await this.fetchResource(request, signal, false);
      } catch (error) {
        if (
          attempt >= FOREGROUND_ADMISSION_RETRY_LIMIT ||
          signal?.aborted ||
          !pageRequestIsTransientlyBusy(error?.status)
        ) throw error;
        await delayWithAbort(
          pageAdmissionRetryDelayMs(error?.retryAfterMs, attempt),
          signal
        );
      }
    }
  }

  schedule(requests, visibleKeys = []) {
    const unique = [];
    const seen = new Set();
    for (const request of requests) {
      if (!request?.cacheKey || seen.has(request.cacheKey)) continue;
      seen.add(request.cacheKey);
      if (this.ready.has(request.cacheKey)) continue;
      unique.push(request);
    }
    this.pending = unique;
    this.visibleKeys = new Set(
      (visibleKeys ?? []).filter((key) => typeof key === "string" && key)
    );
    this.prefetchProtectedKeys = [...seen].slice(0, this.prefetchProtectLimit);
    for (const active of this.active.values()) {
      active.prefetchPlanned = seen.has(active.key);
      this.abortUnownedActive(active);
    }
    this.pump();
  }

  abortUnownedActive(active) {
    if (
      this.active.get(active.key) === active &&
      !active.prefetchPlanned &&
      active.foregroundWaiters === 0
    ) {
      active.controller.abort();
    }
  }

  pump() {
    if (this.retryTimer) return;
    let starts = pagePrefetchStartCount(
      this.active.size,
      this.pending.length,
      this.prefetchConcurrency
    );
    if (starts <= 0) return;
    if (
      this.ready.size >= this.limit ||
      !pagePrefetchBudgetAllowsStart(this.readyBytes, this.byteLimit)
    ) {
      const candidate = this.pending.find(
        (request) =>
          request?.cacheKey &&
          !this.ready.has(request.cacheKey) &&
          !this.active.has(request.cacheKey)
      );
      if (!candidate) return;
      const admission = pageResourceAdmissionPlan({
        entries: [...this.ready].map(([key, resource]) => ({
          key,
          bytes: resource.blob.size,
        })),
        // The candidate and every plan entry nearer than it stay protected. A
        // farther planned entry may be evicted to make the next nearer request
        // admissible, but a nearer entry can never be traded for a farther one.
        protectedKeys: this.protectedKeys(candidate.cacheKey),
        limit: this.limit,
        byteLimit: this.byteLimit,
        retainedBytes: this.readyBytes,
      });
      for (const key of admission.evictKeys) this.deleteReady(key);
      if (!admission.allowStart) return;
    }
    while (starts > 0) {
      const request = this.pending.shift();
      if (!request) return;
      if (this.ready.has(request.cacheKey) || this.active.has(request.cacheKey)) {
        starts = pagePrefetchStartCount(
          this.active.size,
          this.pending.length,
          this.prefetchConcurrency
        );
        continue;
      }
      this.startPrefetch(request);
      starts -= 1;
    }
  }

  startPrefetch(request) {
    const controller = new AbortController();
    const active = {
      key: request.cacheKey,
      controller,
      prefetchPlanned: true,
      foregroundWaiters: 0,
      promise: null,
    };
    active.promise = this.fetchResource(request, controller.signal, true)
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
        const failure = controller.signal.aborted
          ? { retry: false, pendingRequests: this.pending }
          : pagePrefetchFailurePlan({
            pendingRequests: this.pending,
            failedRequest: request,
            status: error?.status,
            errorCode: error?.code,
          });
        this.pending = failure.pendingRequests;
        if (failure.retry) this.scheduleRetry(error?.retryAfterMs);
        if (error?.name !== "AbortError") {
          enqueueTelemetry({
            type: "page_prefetch",
            status: failure.retry ? "admission_busy" : "failed",
            retry_planned: failure.retry,
            message: limitText(error instanceof Error ? error.message : error, 240),
          });
        }
        throw error;
      })
      .finally(() => {
        if (this.active.get(active.key) === active) this.active.delete(active.key);
        this.notifyStatusChange(
          controller.signal.aborted ? "discard" : "settled",
          active.key
        );
        this.pump();
      });
    // A rejected background promise must be observed even when no foreground load joins it.
    active.promise.catch(() => {});
    this.active.set(active.key, active);
    this.notifyStatusChange("start", active.key);
  }

  scheduleRetry(delayMs) {
    if (this.retryTimer) return;
    const delay = Math.max(100, Math.min(10_000, Number(delayMs) || 1000));
    this.retryTimer = setTimeout(() => {
      this.retryTimer = 0;
      this.pump();
    }, delay);
  }

  remember(key, resource) {
    const previous = this.ready.get(key);
    if (previous) this.readyBytes -= previous.blob.size;
    this.ready.delete(key);
    this.ready.set(key, resource);
    this.readyBytes += resource.blob.size;
    this.trimUnprotected();
    this.notifyStatusChange("ready", key);
  }

  /// 予算は「開始の門」だが、保持そのものは常に有界でなければならない。`pump()` は
  /// 開始枠が無いと破棄まで到達しないので (1 ページだけのコンテナを順に開くと計画が
  /// 空で毎回そこで返る)、保持を増やした側でも保護集合の外を削る。
  trimUnprotected() {
    if (
      this.ready.size <= this.limit &&
      pagePrefetchBudgetAllowsStart(this.readyBytes, this.byteLimit)
    ) {
      return;
    }
    const admission = pageResourceAdmissionPlan({
      entries: [...this.ready].map(([key, resource]) => ({
        key,
        bytes: resource.blob.size,
      })),
      protectedKeys: this.protectedKeys(),
      limit: this.limit,
      byteLimit: this.byteLimit,
      retainedBytes: this.readyBytes,
    });
    for (const key of admission.evictKeys) this.deleteReady(key);
  }

  protectedKeys(candidateKey = null) {
    let plannedKeys = this.prefetchProtectedKeys;
    if (candidateKey) {
      const candidateIndex = plannedKeys.indexOf(candidateKey);
      if (candidateIndex >= 0) {
        plannedKeys = plannedKeys.slice(0, candidateIndex + 1);
      }
    }
    return new Set([...this.visibleKeys, ...plannedKeys]);
  }

  deleteReady(key) {
    const resource = this.ready.get(key);
    if (!resource) return false;
    this.ready.delete(key);
    this.readyBytes = Math.max(0, this.readyBytes - resource.blob.size);
    this.notifyStatusChange("evict", key);
    return true;
  }

  clear() {
    this.pending = [];
    for (const active of this.active.values()) active.controller.abort();
    this.active.clear();
    clearTimeout(this.retryTimer);
    this.retryTimer = 0;
    this.ready.clear();
    this.readyBytes = 0;
    this.visibleKeys.clear();
    this.prefetchProtectedKeys = [];
    this.notifyStatusChange("clear");
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

function delayWithAbort(delayMs, signal) {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(abortError());
      return;
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", abort);
      resolve();
    }, delayMs);
    function abort() {
      clearTimeout(timer);
      reject(abortError());
    }
    signal?.addEventListener("abort", abort, { once: true });
  });
}

const thumbnailRequestLimiter = new RequestLimiter(THUMBNAIL_MAX_CONCURRENCY);
const pageResourceCache = new PageResourceCache(
  PAGE_RESOURCE_CACHE_LIMIT,
  PAGE_RESOURCE_CACHE_MAX_BYTES,
  PAGE_PREFETCH_CONCURRENCY,
  fetchPageResource,
  ({ type }) => {
    if (type === "clear") hudState.pagePrefetch = null;
    updateHud();
  }
);

const countedPageTimingRequestIds = new Set();

function recordSuccessfulPageTiming(request, resource, decodeMs) {
  if (!request?.imageQualityPresetId) return;
  if (!shouldCountPageTiming(countedPageTimingRequestIds, resource?.requestId)) {
    return;
  }
  const sample = pageTimingSample({
    totalFetchMs: resource?.fetchMs,
    generationMs: resource?.pageRenderMs,
    decodeMs,
  });
  if (!sample) return;
  pageTimingHistory = appendPageTimingSample(
    pageTimingHistory,
    request.imageQualityPresetId,
    sample
  );
  pageTimingHistory = savePageTimingHistory(pageTimingHistory).history;
}

const telemetryState = {
  queue: [],
  flushing: false,
  authenticated: false,
  nextSequence: 1,
  visualViewportScale: normalizeVisualViewportScale(
    globalThis.window?.visualViewport?.scale
  ),
  visualViewportTimer: 0,
  visualViewportObservedAtMs: 0,
  nextBrowserTapPairSequence: 1,
  lastBrowserTapPair: null,
};

const hudState = {
  lastImage: null,
  lastGrid: null,
  video: null,
  displayDurations: [],
  errors: [],
  pagePrefetch: null,
};

class RemoteSessionControlOwner {
  constructor() {
    this.snapshot = remoteSessionControlTransition(null, "inactive", "");
    this.listeners = new Set();
  }

  transition(status, message) {
    this.snapshot = remoteSessionControlTransition(this.snapshot, status, message);
    for (const listener of this.listeners) {
      try {
        listener(this.snapshot);
      } catch {}
    }
    return this.snapshot;
  }

  subscribe(listener) {
    this.listeners.add(listener);
    try {
      listener(this.snapshot);
    } catch {}
    return () => this.listeners.delete(listener);
  }
}

const remoteSessionControlOwner = new RemoteSessionControlOwner();
// module scope の singleton は起動ブロック (`if (!RUNTIME_TEST_MODE)`) より前で
// 初期化しておくこと。boot() は最初の await より前に renderLoading → cleanupScreen
// まで同期的に到達するので、後ろで宣言すると TDZ でモジュール評価ごと落ちる。
const containerSpreadRefreshOwner = new LatestOnlyTaskQueue(
  performContainerSpreadRefresh,
  (error, request) => reportContainerSpreadRefreshError(error, request, {
    reason: ContainerSpreadRefreshExitReason.UNEXPECTED_ERROR,
    stage: "owner_completion",
  }),
  (left, right) =>
    left.viewer === right.viewer &&
    left.addressIdentity === right.addressIdentity &&
    left.currentIdentity === right.currentIdentity &&
    left.forceSinglePage === right.forceSinglePage
);

const state = {
  authenticated: false,
  favorites: [],
  home: { places: [], smart_folders: [] },
  homeLoadError: "",
  homeTab: "places",
  favoriteSearch: { query: "", kind: "all" },
  tagBrowse: null,
  tagBrowseLoadError: "",
  tagBrowseFilter: { query: "", kind: "all" },
  collection: null,
  container: null,
  gridSortState: null,
  gridSortScope: null,
  gridSortWritePending: false,
  gridReturnHash: "#home/places",
  gridHash: "#home/places",
  thumbAspectHeightRatio: 1,
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
  videoPanelTab: "functions",
  coarsePointer: Boolean(globalThis.matchMedia?.("(pointer: coarse)")?.matches),
  keyboardInputSeen: false,
  fitMode: FitMode.PAGE,
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
  archiveOpenController: null,
  thumbnailNotice: null,
  gridActionNotice: null,
  screenContext: "loading",
  gridIndex: 0,
  authCountdownTimer: 0,
  remoteSessionOwner: null,
  remoteSessionAcquirePromise: null,
  remoteSessionId: "",
  remoteSessionCorrelation: "",
  remoteSessionCacheEpoch: "",
  remoteSessionUserActive: false,
  remoteSessionTimer: 0,
  viewerItemState: null,
  viewerItemStateSequence: 0,
  pageRenderRevision: 0,
  remoteStateGeneration: "",
  gridViewportMemory: new GridViewportMemory(),
  appAssetToken: readRunningAssetToken(),
  appUpdateDismissedToken: null,
  appUpdateBanner: null,
  appUpdateWatchStarted: false,
  appVersionReportedPair: "",
};

let recentPointerSource = { source: "mouse", at: 0 };
if (!RUNTIME_TEST_MODE) {
  installDocumentDoubleTapOwner(document, {
    onDecision: recordBrowserDoubleTapDecision,
  });
  if ("serviceWorker" in navigator) {
    navigator.serviceWorker
      .register("/service-worker.js", { scope: "/", updateViaCache: "none" })
      .catch(() => {});
  }
  updateKeyboardAvailability();
  window.addEventListener("popstate", () => {
    if (remoteSessionControlOwner.snapshot.status === "active") {
      dispatchRoute().catch(() => {});
    }
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
      state.remoteSessionUserActive = false;
      state.remoteAiController?.suspend();
      state.archiveOpenController?.suspend();
    } else if (state.authenticated) {
      if (remoteSessionControlOwner.snapshot.status !== "active") return;
      if (state.viewer?.isVideoStreamViewer) {
        state.viewer.handleVisibilityResume().catch(() => {});
      } else {
        state.remoteAiController?.handleForegroundResume().catch(() => {});
        state.archiveOpenController?.handleForegroundResume().catch(() => {});
      }
    }
  });
  recordRestoredFocusOnLoad();
  boot();
}

/// 再読み込み直後のシークバーに青枠が出るという報告の切り分け用。
/// 誰も focus を当てていないので、ブラウザが復元した focus が
/// `:focus-visible` として扱われている可能性がある。挙動は変えず、
/// 読み込み直後に実際に focus を持つ要素だけを記録する。
function recordRestoredFocusOnLoad() {
  const probe = () => {
    const active = document.activeElement;
    const isBody = !active || active === document.body;
    pendingLoadFocusEvent = {
      type: "load_focus",
      followed_in_app_reload: readAppUpdateReloadAttempt() !== null,
      focused: !isBody,
      tag: isBody ? null : active.tagName.toLowerCase(),
      input_type: isBody ? null : active.getAttribute?.("type") ?? null,
      class_name: isBody ? null : limitText(active.className ?? "", 120),
      focus_visible: isBody ? null : safeMatches(active, ":focus-visible"),
    };
    flushLoadFocusEvent();
  };
  // 観測は読み込み直後に取り、送信は telemetry が認証されてからにする。
  // 認証前に enqueue しても捨てられるため、記録が残らない。
  requestAnimationFrame(() => requestAnimationFrame(probe));
}

let pendingLoadFocusEvent = null;

/// 観測と認証はどちらが先に済むか決まらないので、両方から呼んで
/// 揃った側で 1 度だけ送る。
function flushLoadFocusEvent() {
  if (!pendingLoadFocusEvent || !telemetryState.authenticated) return;
  const event = pendingLoadFocusEvent;
  pendingLoadFocusEvent = null;
  enqueueTelemetry(event);
}

function safeMatches(element, selector) {
  try {
    return Boolean(element.matches?.(selector));
  } catch {
    return null;
  }
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
  flushLoadFocusEvent();
  renderLoading("リモートセッションを取得しています");
  if (!await acquireRemoteSession("authenticated", "initial")) return;
  startSessionPing();
  startAppUpdateWatch().catch(() => {});
  renderLoading("お気に入りを読み込んでいます");
  await remoteHomeDataRefreshCoordinator.loadInitial();
  if (!location.hash) {
    history.replaceState({ mivRoute: true }, "", "#home/places");
  } else {
    history.replaceState({ ...(history.state ?? {}), mivRoute: true }, "", location.href);
  }
  await dispatchRoute();
}

async function acquireRemoteSession(reason = "operation", trigger = "user_operation") {
  const sessionControl = remoteSessionControlOwner.snapshot;
  const decision = remoteSessionAcquireDecision(sessionControl.status, trigger);
  if (decision === "use_current" && state.remoteSessionId) return true;
  if (decision === "blocked") {
    throw new RemoteSessionBlockedError(
      sessionControl.message || "再接続するまでリモート操作は停止しています。"
    );
  }
  if (state.remoteSessionAcquirePromise) return state.remoteSessionAcquirePromise;
  setRemoteSessionStatus("acquiring", "操作権を取得しています…", {
    observer: "acquire",
    observedStatus: "request_started",
  });
  state.remoteSessionAcquirePromise = (async () => {
    let response;
    let result = {};
    try {
      for (let attempt = 0; ; attempt += 1) {
        response = await fetch("/api/session/acquire", {
          method: "POST",
          credentials: "same-origin",
          headers: remoteHeaders({ Accept: "application/json" }),
        });
        if (response.status === 401) {
          renderPinLogin(0);
          throw new AuthenticationRequiredError("PIN 認証が必要です。");
        }
        result = await response.json().catch(() => ({}));
        if (response.ok && result.status === "active") break;
        const retryDelayMs = remoteSessionAcquireRetryDelay(
          response.status,
          result.status,
          attempt
        );
        // 別 owner または直前の動画 worker の drain は、この取得 intent より前の
        // 所有権を安全に返す遷移。任意の wall-clock 期限で intent を捨てず、final
        // release 後に同じ取得を成立させる。切断後の自動再取得とは別物。
        if (retryDelayMs !== null) {
          if (attempt === 0) {
            setRemoteSessionStatus(
              "acquiring",
              "前のリモート処理が安全に終了するのを待っています…",
              {
                observer: "acquire",
                observedStatus: result.status,
                httpStatus: response.status,
              }
            );
          }
          await new Promise((resolve) => window.setTimeout(resolve, retryDelayMs));
          continue;
        }
        break;
      }
      if (!response?.ok || result.status !== "active") {
        const status = sessionStatusFromResponse(result.status, response?.status ?? 0);
        setRemoteSessionStatus(
          status,
          result.message || `操作権を取得できません (HTTP ${response?.status ?? 0})。`,
          {
            observer: "acquire",
            observedStatus: result.status,
            httpStatus: response?.status,
          }
        );
        throw new RemoteSessionBlockedError(
          result.message || "リモートセッションを取得できません。"
        );
      }
      const sessionId = String(result.session_id ?? "");
      if (!/^[A-Fa-f0-9]{32}$/.test(sessionId)) {
        throw new Error("取得したリモートセッション ID が不正です。");
      }
      applyRemoteSessionId(sessionId);
      applyRemoteStateGeneration(result.remote_state_generation, { reloadViewer: true });
      setRemoteSessionStatus("active", "", {
        observer: "acquire",
        observedStatus: result.status,
        httpStatus: response.status,
      });
      state.remoteSessionUserActive = false;
      enqueueTelemetry({ type: "remote_session", action: "acquire", reason });
      return reconcileAppVersionAfterSessionAcquire(result.asset_token);
    } catch (error) {
      if (error instanceof AuthenticationRequiredError) throw error;
      if (!(error instanceof RemoteSessionBlockedError)) {
        setRemoteSessionStatus(
          "unavailable",
          error instanceof Error ? error.message : "操作権を取得できません。",
          {
            observer: "acquire",
            observedStatus: response ? result.status : "network_error",
            httpStatus: response?.status,
          }
        );
      }
      throw error;
    } finally {
      state.remoteSessionAcquirePromise = null;
    }
  })();
  return state.remoteSessionAcquirePromise;
}

function newRemoteSessionCacheEpoch() {
  return (
    globalThis.crypto?.randomUUID?.().replaceAll("-", "") ??
    `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
  );
}

export function invalidateViewerPendingLoad(viewer) {
  if (typeof viewer?.invalidatePendingLoad === "function") {
    viewer.invalidatePendingLoad();
  }
}

export function applyRemoteSessionId(
  sessionId,
  refreshHomeData = () => remoteHomeDataRefreshCoordinator.refreshAfterSessionAcquire()
) {
  const next = String(sessionId ?? "");
  if (state.remoteSessionId === next) return;
  // Identity is the admission boundary. Publish its revocation before invoking an optional
  // image-viewer hook; video owns no pending page fetch and intentionally has no such method.
  state.remoteSessionId = next;
  state.remoteSessionCorrelation = "";
  if (next) {
    telemetrySessionCorrelation(next).then((correlation) => {
      if (state.remoteSessionId !== next) return;
      state.remoteSessionCorrelation = correlation;
      updateHud();
    });
  }
  // session_id 自体は capability なので URL へ出さない。これに従属する非 secret nonce で
  // header を付けられない <img> などの HTTP cache だけを分離する。
  state.remoteSessionCacheEpoch = next ? newRemoteSessionCacheEpoch() : "";
  invalidateViewerPendingLoad(state.viewer);
  pageResourceCache.clear();
  state.imageInfoCache.clear();
  state.containerImageInfoHints.clear();
  if (next) {
    // Remote ownership is exclusive: core settings can change only while the remote does not
    // own control. Every core-side change therefore crosses a disconnect and a new acquisition,
    // making acquisition the complete refresh signal. Do not move this back to generation.
    refreshHomeData();
  }
}

const APP_UPDATE_POLL_INTERVAL_MS = 5 * 60 * 1000;

function normalizeAppAssetToken(value) {
  const token = String(value ?? "");
  return APP_ASSET_TOKEN_PATTERN.test(token) ? token : "";
}

/// /api/app-version は「今なら配る版」であり、既に走っている script の版ではない。
/// navigation 応答へ固定された meta だけを、この page 自身の版として使う。
function readRunningAssetToken() {
  return normalizeAppAssetToken(
    document.querySelector('meta[name="miv-remote-asset-token"]')?.content
  );
}

function readAppUpdateReloadAttempt() {
  try {
    const parsed = JSON.parse(
      globalThis.sessionStorage?.getItem(APP_UPDATE_RELOAD_ATTEMPT_KEY) ?? "null"
    );
    const runningToken = normalizeAppAssetToken(parsed?.runningToken);
    const servedToken = normalizeAppAssetToken(parsed?.servedToken);
    return runningToken && servedToken ? { runningToken, servedToken } : null;
  } catch {
    return null;
  }
}

/// Reload is allowed only after the loop guard survives a storage round trip. If sessionStorage
/// is unavailable (private-mode restrictions, quota, etc.), keeping the banner is safer than
/// risking an unbounded reload loop on a phone.
function rememberAppUpdateReloadAttempt(runningToken, servedToken) {
  const attempt = JSON.stringify({ runningToken, servedToken });
  try {
    globalThis.sessionStorage?.setItem(APP_UPDATE_RELOAD_ATTEMPT_KEY, attempt);
    return globalThis.sessionStorage?.getItem(APP_UPDATE_RELOAD_ATTEMPT_KEY) === attempt;
  } catch {
    return false;
  }
}

function clearAppUpdateReloadAttempt() {
  try {
    globalThis.sessionStorage?.removeItem(APP_UPDATE_RELOAD_ATTEMPT_KEY);
  } catch {}
}

/// 走っている版と配信されている版を突き合わせる。
///
/// 画面遷移がハッシュ変更なので、開きっぱなしのタブは自分の script を二度と取りに行かない。
/// 見ている物が古いことは中からは分からないので、こちらから知らせる。
async function readServedAssetToken() {
  const data = await apiJson("/api/app-version");
  return normalizeAppAssetToken(data.asset_token);
}

async function startAppUpdateWatch() {
  if (state.appUpdateWatchStarted) return;
  state.appUpdateWatchStarted = true;
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

function recordAppVersion(servedToken, trigger, updateOutcome, { force = false } = {}) {
  const runningToken = state.appAssetToken;
  const normalizedServed = normalizeAppAssetToken(servedToken);
  const pair = `${runningToken}:${normalizedServed}`;
  if (!force && pair === state.appVersionReportedPair) return;
  state.appVersionReportedPair = pair;
  enqueueTelemetry({
    type: "app_version",
    action: trigger === "session_acquire" ? "session_acquire" : "version_check",
    running_asset_token: runningToken,
    served_asset_token: normalizedServed,
    versions_match:
      runningToken && normalizedServed ? runningToken === normalizedServed : null,
    update_outcome: updateOutcome,
  });
}

function reconcileAppVersionAfterSessionAcquire(servedToken) {
  const served = normalizeAppAssetToken(servedToken);
  const notice = appUpdateNotice({
    runningToken: state.appAssetToken,
    servedToken: served,
    trigger: "session_acquired",
    reloadAttempt: readAppUpdateReloadAttempt(),
  });
  if (notice.kind === "current") {
    const outcome =
      state.appAssetToken && served ? "current" : "version_unavailable";
    if (outcome === "current") clearAppUpdateReloadAttempt();
    recordAppVersion(served, "session_acquire", outcome, { force: true });
    return true;
  }
  if (notice.kind === "reload_required") {
    if (rememberAppUpdateReloadAttempt(state.appAssetToken, notice.servedToken)) {
      recordAppVersion(served, "session_acquire", "automatic_reload", { force: true });
      reloadApplication();
      return false;
    }
    recordAppVersion(served, "session_acquire", "banner_storage_unavailable", {
      force: true,
    });
    showAppUpdateBanner(notice.servedToken);
    return true;
  }
  recordAppVersion(served, "session_acquire", "banner_reload_already_attempted", {
    force: true,
  });
  showAppUpdateBanner(notice.servedToken);
  return true;
}

async function checkForAppUpdate() {
  if (!state.authenticated) return;
  const servedToken = await readServedAssetToken();
  const notice = appUpdateNotice({
    runningToken: state.appAssetToken,
    servedToken,
    dismissedToken: state.appUpdateDismissedToken,
  });
  recordAppVersion(
    servedToken,
    "watch",
    notice.kind === "update_available" ? "banner" : notice.kind
  );
  if (notice.kind !== "update_available") return;
  showAppUpdateBanner(notice.servedToken);
}

/// 稼働中の照合は、開発中の in-place 配布でも起きる。動画や読書を中断せず、
/// 自動再読込は session 取得直後に限定して、ここでは利用者へ選択を残す。
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
    remoteSessionControlOwner.snapshot.status !== "active" ||
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
      result.message || "リモートセッションが切断されました。再接続してください。",
      {
        observer: "ping",
        observedStatus: result.status,
        httpStatus: response.status,
      }
    );
    return;
  }
  if (result.session_id !== state.remoteSessionId) {
    setRemoteSessionStatus(
      "other_device",
      "リモートセッションが更新されました。再接続してください。",
      {
        observer: "ping",
        observedStatus: "session_id_mismatch",
        httpStatus: response.status,
      }
    );
    return;
  }
  applyRemoteStateGeneration(result.remote_state_generation, { reloadViewer: true });
}

export function createRemoteHomeDataRefreshCoordinator({
  requestJson,
  appState,
  applyGeneration,
  renderHomeScreen,
}) {
  const pendingByTarget = new Map();
  let sessionRefreshPromise = null;

  // This is the canonical set of home-screen data invalidated by a new remote session.
  // Add future home-screen data sources here so session refresh cannot omit one.
  const targets = Object.freeze([
    {
      key: "favorites",
      endpoint: "/api/favorites",
      install(data) {
        applyGeneration(data.remote_state_generation, { reloadViewer: false });
        appState.favorites = data.favorites ?? [];
        if (appState.screenContext === "home" && appState.homeTab === "favorites") {
          renderHomeScreen("favorites");
        }
        return appState.favorites;
      },
    },
    {
      key: "home",
      endpoint: "/api/home",
      install(data) {
        // A refresh replaces the last good value only after a successful request. In
        // particular, do not reuse the initial-load fallback that clears both lists.
        appState.home = data;
        appState.homeLoadError = "";
        if (
          appState.screenContext === "home" &&
          (appState.homeTab === "places" || appState.homeTab === "smart")
        ) {
          renderHomeScreen(appState.homeTab);
        }
        return appState.home;
      },
      handleInitialFailure(error) {
        // Initial loading has no last good value, so it keeps the existing empty-state error
        // behavior. A post-acquisition refresh deliberately never calls this handler.
        appState.home = { places: [], smart_folders: [] };
        appState.homeLoadError =
          error instanceof Error ? error.message : "mIV 本体から一覧を取得できませんでした。";
      },
    },
  ]);

  function refresh(target) {
    const existing = pendingByTarget.get(target.key);
    if (existing) return existing;

    const pending = requestJson(target.endpoint)
      .then((data) => target.install(data))
      .finally(() => {
        if (pendingByTarget.get(target.key) === pending) {
          pendingByTarget.delete(target.key);
        }
      });
    pendingByTarget.set(target.key, pending);
    return pending;
  }

  function refreshAfterSessionAcquire() {
    sessionRefreshPromise = Promise.allSettled(targets.map((target) => refresh(target)));
    return sessionRefreshPromise;
  }

  return {
    async loadInitial() {
      // Initial acquisition already starts this batch from applyRemoteSessionId. Retain the
      // settled promise until boot consumes it so a fast response cannot cause a second batch.
      const initialRefresh = sessionRefreshPromise ?? refreshAfterSessionAcquire();
      const results = await initialRefresh;
      if (sessionRefreshPromise === initialRefresh) sessionRefreshPromise = null;
      for (let index = 0; index < results.length; index += 1) {
        const result = results[index];
        if (result.status === "fulfilled") continue;
        const target = targets[index];
        if (!target.handleInitialFailure) throw result.reason;
        target.handleInitialFailure(result.reason);
      }
    },
    refreshAfterSessionAcquire,
  };
}

const remoteHomeDataRefreshCoordinator = createRemoteHomeDataRefreshCoordinator({
  requestJson: apiJson,
  appState: state,
  applyGeneration: applyRemoteStateGeneration,
  renderHomeScreen: renderHome,
});

function applyRemoteStateGeneration(observed, { reloadViewer = false } = {}) {
  const transition = remoteStateGenerationTransition(
    state.remoteStateGeneration,
    observed
  );
  if (!transition.initialized) return transition.generation;
  state.remoteStateGeneration = transition.generation;
  if (!transition.changed) return transition.generation;

  invalidateViewerPendingLoad(state.viewer);
  pageResourceCache.clear();
  state.imageInfoCache.clear();
  state.containerImageInfoHints.clear();
  if (reloadViewer && state.viewer) {
    const viewer = state.viewer;
    queueMicrotask(() => {
      if (state.viewer === viewer) updateViewerImage(performance.now()).catch(renderError);
    });
  }
  return transition.generation;
}

function setRemoteSessionStatus(status, message, observation = {}) {
  const previousSessionControl = remoteSessionControlOwner.snapshot;
  const sessionControl = remoteSessionControlOwner.transition(status, message);
  const transitionEvent = remoteSessionTransitionTelemetry(
    previousSessionControl,
    sessionControl,
    observation
  );
  // Persist the typed transition before cache invalidation or modal rendering. If either
  // downstream side effect throws, diagnostics can still distinguish it from detection loss.
  if (transitionEvent) enqueueTelemetry(transitionEvent);
  if (
    status === "local_in_use" ||
    status === "other_device" ||
    status === "expired" ||
    status === "not_acquired"
  ) {
    applyRemoteSessionId("");
  }
  if (status === "active" || status === "other_device") {
    state.remoteSessionOwner = status;
  }
  updateRemoteSessionOwnerBadge();
  let element = document.querySelector("#remote-session-status");
  if (!element) {
    element = document.createElement("div");
    element.id = "remote-session-status";
    element.className = "remote-session-status";
    const card = document.createElement("div");
    card.className = "remote-session-status-card";
    element.append(card);
    document.body.append(element);
  }
  const card = element.querySelector(".remote-session-status-card");
  const statusMessage = textElement("span", sessionControl.message);
  statusMessage.id = "remote-session-status-message";
  card.replaceChildren(statusMessage);
  element.hidden = sessionControl.status === "active" || sessionControl.status === "inactive";
  element.dataset.status = sessionControl.status;
  element.classList.toggle("is-modal", sessionControl.blocksInteraction);
  element.setAttribute("role", sessionControl.blocksInteraction ? "dialog" : "status");
  if (sessionControl.blocksInteraction) {
    element.setAttribute("aria-modal", "true");
    element.setAttribute("aria-labelledby", statusMessage.id);
  } else {
    element.removeAttribute("aria-modal");
    element.removeAttribute("aria-labelledby");
  }
  app.inert = sessionControl.blocksInteraction;
  if (
    sessionControl.status === "local_in_use" ||
    sessionControl.status === "other_device" ||
    sessionControl.status === "expired" ||
    sessionControl.status === "not_acquired" ||
    sessionControl.status === "unavailable"
  ) {
    const reconnect = textElement(
      "button",
      "再接続",
      "remote-session-reconnect"
    );
    reconnect.type = "button";
    reconnect.addEventListener("click", () => {
      reconnect.disabled = true;
      acquireRemoteSession("explicit_reconnect", "explicit_reconnect")
        .then(async (acquired) => {
          if (!acquired) return;
          const viewer = state.viewer;
          if (await viewer?.resumeAfterRemoteSessionReconnect?.()) return;
          await dispatchRoute();
        })
        .catch(() => {
          reconnect.disabled = false;
        });
    });
    card.append(reconnect);
    if (sessionControl.blocksInteraction) {
      queueMicrotask(() => {
        if (!element.hidden && sessionControl === remoteSessionControlOwner.snapshot) {
          reconnect.focus();
        }
      });
    }
  }
}

let sessionOwnerBadgeShownOwner = null;
let sessionOwnerBadgeHideTimer = 0;

function updateRemoteSessionOwnerBadge() {
  const owner = state.authenticated ? state.remoteSessionOwner : null;
  const transition = sessionOwnerBadgeTransition(sessionOwnerBadgeShownOwner, owner);
  if (transition.action === "unchanged") return;
  sessionOwnerBadgeShownOwner = owner;
  clearTimeout(sessionOwnerBadgeHideTimer);
  sessionOwnerBadgeHideTimer = 0;
  if (transition.action === "hide") {
    sessionOwnerBadgeElement.hidden = true;
    return;
  }
  const presentation = sessionOwnerBadge(owner);
  sessionOwnerBadgeElement.dataset.owner = presentation.owner;
  sessionOwnerBadgeElement.textContent = presentation.label;
  sessionOwnerBadgeElement.hidden = false;
  if (transition.autoHideMs > 0) {
    sessionOwnerBadgeHideTimer = window.setTimeout(() => {
      sessionOwnerBadgeHideTimer = 0;
      if (sessionOwnerBadgeShownOwner === owner) {
        sessionOwnerBadgeElement.hidden = true;
      }
    }, transition.autoHideMs);
  }
}

function sessionStatusFromHttp(status) {
  if (status === 409) return "local_in_use";
  if (status === 428) return "expired";
  return "unavailable";
}

function sessionStatusFromResponse(sessionStatus, httpStatus) {
  if (sessionStatus === "superseded") return "other_device";
  if (sessionStatus === "local_in_use") return "local_in_use";
  if (sessionStatus === "expired") return "expired";
  if (sessionStatus === "not_acquired") return "not_acquired";
  return sessionStatusFromHttp(httpStatus);
}

function renderPinLogin(initialRemainingSeconds = 0) {
  cleanupScreen();
  applyRemoteSessionId("");
  state.screenContext = "pin";
  state.authenticated = false;
  setRemoteSessionStatus("inactive", "", {
    observer: "auth",
    observedStatus: "pin_required",
  });
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
    if (route.kind === "search") {
      await showFavoriteSearch(route);
      return;
    }
    if (route.kind === "tag") {
      await showTagItems(route);
      return;
    }
    if (route.kind === "folder") {
      await showFolder(route.path);
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
          parentAddress.path,
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
        if (isStreamMediaKind(entry.kind)) {
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
      const folderPath = parentPath(route.path);
      const address = {
        path: folderPath,
        subresource: { kind: "file" },
      };
      await loadContainer(address);
      const pageAddress = {
        path: route.path,
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

export function parseRoute(hash) {
  if (!hash) {
    return { kind: "home", tab: "places" };
  }
  if (hash === "#favorites") {
    return { kind: "home", tab: "favorites" };
  }
  const home = hash.match(/^#home\/(favorites|smart|places|search|tags)$/);
  if (home) {
    return { kind: "home", tab: home[1] };
  }
  const collection = hash.match(
    /^#collection\/(drive_list|reading_history|bookmarks|bookshelf|rating|smart)(?:\/([^/]+))?$/
  );
  if (collection) {
    return {
      kind: "collection",
      collectionKind: collection[1],
      value: collection[2] ?? "",
    };
  }
  const search = hash.match(/^#search\/(all|folder|zip|pdf)\/(.*)$/);
  if (search) {
    try {
      return {
        kind: "search",
        searchKind: search[1],
        query: decodeURIComponent(search[2]),
      };
    } catch {
      return { kind: "home", tab: "search" };
    }
  }
  const tag = hash.match(
    /^#tag\/(all|folder|image|video|audio|zip|pdf|archive)\/(.*)$/
  );
  if (tag) {
    try {
      return {
        kind: "tag",
        tagKind: tag[1],
        tag: decodeURIComponent(tag[2]),
      };
    } catch {
      return { kind: "home", tab: "tags" };
    }
  }
  const addressed = hash.match(/^#(container|media)\/(.*)$/);
  if (addressed) {
    try {
      return { kind: addressed[1], address: decodeAddress(addressed[2]) };
    } catch {
      return { kind: "home", tab: "places" };
    }
  }
  const match = hash.match(/^#(folder|image)\/(.*)$/);
  if (!match) {
    return { kind: "home", tab: "places" };
  }
  try {
    return {
      kind: match[1],
      path: decodeURIComponent(match[2]),
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

export function favoriteSearchHash(query, kind = "all") {
  return `#search/${kind}/${encodeURIComponent(query)}`;
}

export function tagItemsHash(tag, kind = "all") {
  return `#tag/${kind}/${encodeURIComponent(tag)}`;
}

function folderHash(path) {
  return `#folder/${encodeURIComponent(path)}`;
}

function imageHash(path) {
  return `#image/${encodeURIComponent(path)}`;
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
    typeof address.path !== "string" ||
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

function optionalTelemetryMeasurement(value) {
  if (value === null || value === undefined) return null;
  const measurement = Number(value);
  return Number.isFinite(measurement) ? roundMs(measurement) : null;
}

const BROWSER_DOUBLE_TAP_DECISIONS = new Set([
  "candidate_started",
  "pair_rejected",
  "pair_suppressed",
  "pair_not_cancelable",
  "excluded_target",
  "travel_exceeded",
]);
const BROWSER_DOUBLE_TAP_EXCLUSIONS = new Set([
  "button",
  "link",
  "textarea",
  "contenteditable",
  "role_button",
  "role_link",
  "role_tab",
  "role_option",
  "text_input",
]);

export function browserDoubleTapTelemetryEvent(decision, tapPairSequence = null) {
  const sequence = Number(tapPairSequence);
  const decisionName = String(decision?.decision || "unknown");
  const exclusionReason = String(decision?.exclusionReason || "");
  return {
    type: "browser_double_tap",
    action: "suppression_decision",
    decision: BROWSER_DOUBLE_TAP_DECISIONS.has(decisionName)
      ? decisionName
      : "unknown",
    tap_pair_sequence:
      Number.isInteger(sequence) && sequence > 0 ? sequence : null,
    previous_tap_elapsed_ms: optionalTelemetryMeasurement(decision?.elapsedMs),
    previous_tap_distance_px: optionalTelemetryMeasurement(decision?.distancePx),
    recognized_double_tap: Boolean(decision?.isDoubleTap),
    suppressed: Boolean(decision?.suppressed),
    excluded: Boolean(decision?.excluded),
    exclusion_reason: BROWSER_DOUBLE_TAP_EXCLUSIONS.has(exclusionReason)
      ? exclusionReason
      : null,
    event_cancelable: Boolean(decision?.cancelable),
  };
}

function recordBrowserDoubleTapDecision(decision) {
  const elapsedMs = optionalTelemetryMeasurement(decision?.elapsedMs);
  const distancePx = optionalTelemetryMeasurement(decision?.distancePx);
  const hasPair = elapsedMs !== null && distancePx !== null;
  const tapPairSequence = hasPair
    ? telemetryState.nextBrowserTapPairSequence++
    : null;
  const event = browserDoubleTapTelemetryEvent(decision, tapPairSequence);
  if (tapPairSequence !== null) {
    telemetryState.lastBrowserTapPair = {
      sequence: tapPairSequence,
      atMs: Number.isFinite(Number(decision?.atMs))
        ? Number(decision.atMs)
        : performance.now(),
      elapsedMs: event.previous_tap_elapsed_ms,
      distancePx: event.previous_tap_distance_px,
      recognized: event.recognized_double_tap,
      suppressed: event.suppressed,
      excluded: event.excluded,
      exclusionReason: event.exclusion_reason,
      cancelable: event.event_cancelable,
    };
  }
  enqueueTelemetry(event);
}

function precedingBrowserTapPairTelemetry(observedAtMs) {
  const pair = telemetryState.lastBrowserTapPair;
  if (!pair) {
    return {
      preceding_tap_pair_sequence: null,
      preceding_tap_pair_age_ms: null,
    };
  }
  return {
    preceding_tap_pair_sequence: pair.sequence,
    preceding_tap_pair_age_ms: roundMs(Math.max(0, observedAtMs - pair.atMs)),
    preceding_tap_pair_elapsed_ms: pair.elapsedMs,
    preceding_tap_pair_distance_px: pair.distancePx,
    preceding_tap_pair_recognized: pair.recognized,
    preceding_tap_pair_suppressed: pair.suppressed,
    preceding_tap_pair_excluded: pair.excluded,
    preceding_tap_pair_exclusion_reason: pair.exclusionReason,
    preceding_tap_pair_cancelable: pair.cancelable,
  };
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
  if (remoteSessionControlOwner.snapshot.blocksInteraction) return true;
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
  if (meta.sessionChecked !== true) {
    const decision = remoteSessionAcquireDecision(
      remoteSessionControlOwner.snapshot.status,
      "user_operation"
    );
    if (decision === "acquire") {
      acquireRemoteSession("command", "user_operation")
        .then((acquired) => {
          if (acquired) dispatchCommand(requested, { ...meta, sessionChecked: true });
        })
        .catch(() => {});
      return true;
    }
    if (decision === "blocked" || !state.remoteSessionId) {
      return true;
    }
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
    const folderParent = state.folderPath ? parentPath(state.folderPath) : "";
    const target = state.container
      ? state.gridReturnHash
      : state.collection
      ? state.gridReturnHash
      : state.folderPath
        ? folderParent === trimmedRemotePath(state.folderPath)
          ? state.gridReturnHash
          : folderHash(folderParent)
        : state.gridReturnHash;
    rememberParentGridReturnTarget(target);
    navigate(target);
    handled = true;
  } else if (
    requested.name === CommandName.OPEN_HOME &&
    state.screenContext === "grid"
  ) {
    const tab = state.collection?.kind === "smart"
      ? "smart"
      : state.collection?.kind === "favorite_search"
        ? "search"
      : state.collection?.kind === "tag_items"
        ? "tags"
      : state.folderPath
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
      state.virtualGrid?.selectReturnAnchor(state.gridIndex);
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
      }
      if (fitMode) {
        state.fitMode = fitMode;
        updateViewerImage(performance.now(), {
          renderTrigger: "fit_mode",
        }).catch(renderError);
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

// This predicate is the canonical stream-media kind boundary. When another kind starts using
// the shared media viewer, update only this predicate instead of copying the kind set to callers.
export function isStreamMediaKind(kind) {
  return kind === "video" || kind === "audio";
}

export function resolveMediaOpenRoute(requestedKind, addressedEntry, imageIndex) {
  if (requestedKind !== "image" && !isStreamMediaKind(requestedKind)) return null;
  if (!addressedEntry || addressedEntry.kind !== requestedKind) return null;
  if (requestedKind === "image" && imageIndex < 0) return null;
  return requestedKind;
}

export function unsupportedRemoteEntryMessage(kind) {
  void kind;
  return "";
}

export function showUnsupportedRemoteEntryNotice(host, kind) {
  const message = unsupportedRemoteEntryMessage(kind);
  if (!host || !message) return false;
  host.textContent = message;
  host.hidden = false;
  return true;
}

export function resolveLegacyImageOpenRoute(payload, imageCount, isCollection) {
  if (
    payload?.kind !== "image" ||
    !Number.isInteger(payload.imageIndex) ||
    payload.imageIndex < 0 ||
    payload.imageIndex >= imageCount
  ) {
    return "legacy_image_rejected";
  }
  return isCollection ? "collection_image" : "folder_image";
}

function executeOpenCommand(payload, meta) {
  payload = payload ?? {};
  meta = meta ?? {};
  if (
    state.screenContext === "grid" &&
    Number.isInteger(payload.entryIndex) &&
    payload.entryIndex >= 0 &&
    payload.entryIndex < state.entries.length
  ) {
    state.gridIndex = payload.entryIndex;
    state.virtualGrid?.selectReturnAnchor(payload.entryIndex);
  }
  if (payload.kind === "archive" && payload.address) {
    state.archiveOpenController?.destroy();
    const controller = new RemoteArchiveOpenController(
      app,
      (listener) => remoteSessionControlOwner.subscribe(listener)
    );
    state.archiveOpenController = controller;
    meta.openRoute = "archive_job";
    controller
      .open(payload.address, payload.name || "アーカイブ")
      .catch((error) => controller.showRequestError(error));
    return true;
  }
  if (payload.kind === "unsupported") {
    if (!showUnsupportedRemoteEntryNotice(state.gridActionNotice, payload.entryKind)) {
      return false;
    }
    meta.openRoute = "unsupported_" + payload.entryKind;
    return true;
  }
  if (payload.kind === "favorite" || payload.kind === "folder") {
    meta.openRoute = payload.kind;
    navigate(folderHash(payload.path));
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
    const renderedViewer = renderResolvedMediaOpen(
      mediaRoute,
      addressedEntry,
      imageIndex,
      meta.at ?? performance.now()
    );
    if (renderedViewer === "rejected") {
      meta.openRoute = "video_viewer_entry_rejected";
      return false;
    }
    return true;
  }
  const legacyOpenRoute = resolveLegacyImageOpenRoute(
    payload,
    state.images.length,
    Boolean(state.collection)
  );
  meta.openRoute = legacyOpenRoute;
  if (legacyOpenRoute === "legacy_image_rejected") return false;
  if (state.collection) {
    tryEnterBrowserFullscreen();
    navigate(imageHash(payload.path), {
      viewerFromGrid: true,
      viewerDepth: 1,
      returnHash: location.hash,
    });
    return true;
  }
  tryEnterBrowserFullscreen();
  history.pushState(
    {
      mivRoute: true,
      navigatedInApp: true,
      viewerFromGrid: true,
      viewerDepth: 1,
      returnHash: folderHash(state.folderPath),
    },
    "",
    imageHash(payload.path)
  );
  renderImageViewer(payload.imageIndex, meta.at ?? performance.now());
  return true;
}

function openFolderImageFromGrid(payload, meta) {
  const pageAddress = payload.address ?? {
    path: payload.path,
    subresource: { kind: "file" },
  };
  if (!pageAddress.path) return false;
  const folderLoad = state.folderContainerLoad;
  const returnHash = folderHash(state.folderPath);
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
      ? imageHash(pageAddress.path)
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
  const path = entryPath(entry);
  if (entryIsFolder(entry)) {
    return executeOpenCommand(
      { kind: "folder", path, entryIndex: index },
      meta
    );
  }
  if (entry.kind === "archive") {
    return executeOpenCommand(
      {
        kind: "archive",
        address: entryAddress(entry),
        name: entry.name,
        entryIndex: index,
      },
      meta
    );
  }
  if (unsupportedRemoteEntryMessage(entry.kind)) {
    return executeOpenCommand(
      { kind: "unsupported", entryKind: entry.kind, entryIndex: index },
      meta
    );
  }
  if (["zip", "pdf"].includes(entry.kind)) {
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
  if (isStreamMediaKind(entry.kind)) {
    return executeOpenCommand(
      {
        kind: "media",
        mediaKind: entry.kind,
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
      : { kind: "image", path, imageIndex, entryIndex: index },
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
  state.virtualGrid.selectReturnAnchor(nextIndex);
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

function rememberCurrentGridViewport() {
  if (
    state.screenContext !== 'grid' ||
    !state.virtualGrid ||
    !state.virtualGrid.context
  ) {
    return;
  }
  const grid = state.virtualGrid;
  // Browser history can finish the next route's async load before this DOM is torn down.
  // Read the context/return anchor owned by the rendered grid, never the already-replaced state.
  state.gridViewportMemory.remember(
    grid.context,
    grid.viewportAnchor.snapshot(grid.scrollTop())
  );
}

function updateGridReturnTargetItem(entry) {
  const sourceContext = history.state?.returnHash;
  if (!sourceContext || !entry) return;
  state.gridViewportMemory.updateTarget(
    sourceContext,
    gridReturnItemIdentity(entry)
  );
}

function rememberParentGridReturnTarget(targetHash) {
  const returnContext = state.container
    ? containerParentHash(state.container.address)
    : targetHash;
  const targetKind = parseRoute(returnContext).kind;
  if (!["folder", "container", "collection"].includes(targetKind)) return;
  const currentAddress = state.container?.address ?? (
    !state.collection && state.folderPath
      ? {
          path: state.folderPath,
          subresource: { kind: "file" },
        }
      : null
  );
  if (!currentAddress) return;
  state.gridViewportMemory.updateTarget(
    returnContext,
    addressIdentity(currentAddress)
  );
}

function menuCommand(event, name, payload = {}) {
  dispatchCommand(command(name, payload), {
    source: inputSourceFromEvent(event),
    detail: "menu",
  });
}

function cleanupScreen(preserveRequestController = null) {
  rememberCurrentGridViewport();
  containerSpreadRefreshOwner.clear();
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
  state.gridActionNotice = null;
  state.commandMenu?.destroy();
  state.commandMenu = null;
  state.localSettingsDialog?.destroy();
  state.localSettingsDialog = null;
  state.gestureHelpDialog?.destroy();
  state.gestureHelpDialog = null;
  state.remoteAiController?.destroy();
  state.remoteAiController = null;
  state.archiveOpenController?.destroy();
  state.archiveOpenController = null;
  state.viewer?.destroy();
  state.viewer = null;
  state.screenContext = "loading";
  app.replaceChildren();
}

function renderHome(tab = "places") {
  cleanupScreen();
  state.screenContext = "home";
  state.homeTab = ["favorites", "smart", "places", "search", "tags"].includes(tab)
    ? tab
    : "places";
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
  else if (state.homeTab === "search") renderFavoriteSearchTab(content);
  else if (state.homeTab === "tags") renderTagBrowseTab(content);
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
    ["search", "検索"],
    ["tags", "タグ"],
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

function renderFavoriteSearchTab(content) {
  content.append(
    textElement(
      "p",
      "お気に入りの中から、フォルダ・ZIP・PDF を名前で探します。",
      "favorite-search-description"
    )
  );
  const controls = createFavoriteSearchForm(state.favoriteSearch, ({ query, kind }) => {
    state.favoriteSearch = { query, kind };
    navigate(favoriteSearchHash(query, kind), {
      returnHash: homeHash("search"),
    });
  });
  content.append(controls.form);
}

export function createFavoriteSearchForm(initial, onSubmit) {
  const form = element("form", "favorite-search-form");
  const query = element("input", "favorite-search-input");
  query.type = "search";
  query.name = "q";
  query.placeholder = "名前を入力";
  query.autocomplete = "off";
  query.maxLength = 200;
  query.required = true;
  query.value = initial?.query ?? "";
  query.setAttribute("aria-label", "検索語句");

  const kind = element("select", "favorite-search-kind");
  kind.name = "kind";
  kind.setAttribute("aria-label", "検索する種類");
  for (const [value, label] of [
    ["all", "すべて"],
    ["folder", "フォルダ"],
    ["zip", "ZIP"],
    ["pdf", "PDF"],
  ]) {
    const option = textElement("option", label);
    option.value = value;
    option.selected = value === (initial?.kind ?? "all");
    kind.append(option);
  }
  kind.value = ["all", "folder", "zip", "pdf"].includes(initial?.kind)
    ? initial.kind
    : "all";

  const submit = textElement("button", "検索", "favorite-search-submit");
  submit.type = "submit";
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const value = query.value.trim();
    if (!value) {
      query.focus();
      return;
    }
    onSubmit({ query: value, kind: kind.value });
  });
  form.append(query, kind, submit);
  return { form, query, kind, submit };
}

const TAG_ITEM_KIND_CHOICES = [
  ["all", "すべて"],
  ["folder", "フォルダ"],
  ["image", "画像"],
  ["video", "動画"],
  ["audio", "音声"],
  ["zip", "ZIP"],
  ["pdf", "PDF"],
  ["archive", "アーカイブ"],
];

function normalizedTagFilterText(value) {
  return String(value ?? "")
    .trim()
    .replace(/^#+/, "")
    .normalize("NFKC")
    .toLocaleLowerCase();
}

export function filterRemoteTags(tags, query) {
  const needle = normalizedTagFilterText(query);
  if (!needle) return Array.isArray(tags) ? [...tags] : [];
  return (Array.isArray(tags) ? tags : []).filter((choice) =>
    normalizedTagFilterText(choice?.name).includes(needle)
  );
}

export function tagBrowsePresentation(payload, query) {
  if (normalizedTagFilterText(query)) {
    return {
      mode: "flat",
      sections: [{ title: "", choices: filterRemoteTags(payload?.all, query) }],
    };
  }
  return {
    mode: "sections",
    sections: [
      { title: "ピン留め", choices: payload?.pinned ?? [] },
      { title: "最近", choices: payload?.recent ?? [] },
      { title: "よく使う", choices: payload?.popular ?? [] },
    ],
  };
}

let tagBrowsePromise = null;

function loadTagBrowseOnce() {
  if (state.tagBrowse || state.tagBrowseLoadError || tagBrowsePromise) return;
  tagBrowsePromise = apiJson("/api/tags")
    .then((payload) => {
      state.tagBrowse = payload;
    })
    .catch((error) => {
      state.tagBrowseLoadError =
        error instanceof Error ? error.message : "タグ一覧を読み込めませんでした。";
    })
    .finally(() => {
      tagBrowsePromise = null;
      if (state.screenContext === "home" && state.homeTab === "tags") {
        renderHome("tags");
      }
    });
}

function renderTagBrowseTab(content) {
  const form = element("form", "tag-browse-form");
  const query = element("input", "tag-filter-input");
  query.type = "search";
  query.placeholder = "タグ名で絞り込み / 直接入力";
  query.autocomplete = "off";
  query.maxLength = 200;
  query.value = state.tagBrowseFilter.query;
  query.setAttribute("aria-label", "タグ名の絞り込み");

  const kind = element("select", "tag-kind-select");
  kind.setAttribute("aria-label", "項目の種類");
  for (const [value, label] of TAG_ITEM_KIND_CHOICES) {
    const option = textElement("option", label);
    option.value = value;
    option.selected = value === state.tagBrowseFilter.kind;
    kind.append(option);
  }
  kind.value = TAG_ITEM_KIND_CHOICES.some(([value]) => value === state.tagBrowseFilter.kind)
    ? state.tagBrowseFilter.kind
    : "all";
  const submit = textElement("button", "実行", "tag-search-submit");
  submit.type = "submit";
  form.append(query, kind, submit);

  const results = element("div", "tag-browse-results");
  const refreshResults = () => renderTagBrowseChoices(results, query.value, kind.value);
  query.addEventListener("input", () => {
    state.tagBrowseFilter.query = query.value;
    refreshResults();
  });
  kind.addEventListener("change", () => {
    state.tagBrowseFilter.kind = kind.value;
  });
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const tag = query.value.trim();
    if (!tag) {
      query.focus();
      return;
    }
    state.tagBrowseFilter = { query: query.value, kind: kind.value };
    navigate(tagItemsHash(tag, kind.value), { returnHash: homeHash("tags") });
  });
  content.append(form, results);

  if (!state.tagBrowse && !state.tagBrowseLoadError) {
    results.append(textElement("p", "タグ一覧を読み込んでいます…", "empty-state"));
    loadTagBrowseOnce();
    return;
  }
  refreshResults();
}

function renderTagBrowseChoices(host, query, kind) {
  host.replaceChildren();
  if (state.tagBrowseLoadError) {
    host.append(textElement("p", state.tagBrowseLoadError, "empty-state"));
    return;
  }
  const payload = state.tagBrowse;
  if (!payload) return;
  if (payload.state === "unavailable") {
    host.append(textElement("p", "タグをまだ利用できません。", "empty-state"));
    return;
  }
  if (payload.state === "empty") {
    host.append(textElement("p", "タグはまだ 1 つもありません。", "empty-state"));
    return;
  }

  const presentation = tagBrowsePresentation(payload, query);
  for (const section of presentation.sections) {
    const group = element("section", "tag-choice-section");
    if (section.title) group.append(textElement("h2", section.title));
    const list = element("div", "tag-choice-list");
    for (const choice of section.choices) {
      const button = element("button", "tag-choice");
      button.type = "button";
      button.append(
        textElement("span", `#${choice.name}`, "tag-choice-name"),
        textElement("span", String(choice.count), "tag-choice-count")
      );
      button.addEventListener("click", () => {
        state.tagBrowseFilter.kind = kind;
        navigate(tagItemsHash(choice.name, kind), { returnHash: homeHash("tags") });
      });
      list.append(button);
    }
    if (!section.choices.length) {
      list.append(
        textElement(
          "p",
          presentation.mode === "flat"
            ? "一致するタグはありません。入力した語句はそのまま実行できます。"
            : "該当するタグはありません。",
          "empty-state tag-choice-empty"
        )
      );
    }
    group.append(list);
    host.append(group);
  }
  if (payload.all_truncated) {
    host.append(
      textElement(
        "p",
        "全タグの一覧は先頭 2000 件までです。一覧にないタグは入力欄から直接実行できます。",
        "tag-truncated-note"
      )
    );
  }
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
          path: favorite.path,
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
    if (place.kind === "separator") {
      list.append(element("hr", "place-separator"));
      continue;
    }
    if (place.kind === "rating") {
      const group = element("section", "rating-card");
      group.append(
        textElement("span", "★", "favorite-icon"),
        textElement("span", place.name, "favorite-name")
      );
      const stars = element("div", "rating-stars");
      for (const rating of place.stars ?? []) {
        if (!Number.isInteger(rating) || rating < 1 || rating > 5) continue;
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
    if (place.kind === "folder") {
      const entry = place.entry;
      if (!entry || typeof entry.path !== "string" || !entry.path) continue;
      const button = homeCard("▣", entry.name ?? entry.path);
      button.addEventListener("click", () =>
        navigate(folderHash(entry.path), { returnHash: homeHash("places") })
      );
      list.append(button);
      continue;
    }
    const icon = {
      drive_list: "▦",
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
    textElement("p", "mIV を起動すると、この一覧を利用できます。")
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
  state.gridSortState = normalizeRemoteGridSortState(data.sort_state);
  state.gridSortScope = collectionSortScope(route.collectionKind, route.value);
  state.container = null;
  state.gridReturnHash =
    route.collectionKind === "smart" ? homeHash("smart") : homeHash("places");
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

async function showFavoriteSearch(route) {
  state.favoriteSearch = { query: route.query, kind: route.searchKind };
  renderLoading("検索しています");
  const data = await apiJson("/api/search/favorites", {
    q: route.query,
    kind: route.searchKind,
  });
  const listing = data.listing ?? {};
  const entries = listing.entries ?? [];
  // 何を探した結果なのかは画面に残っていないと分からなくなる。語句は本体が返す題ではなく、
  // こちらが持っている route の値から組み立てる (本体へ語句を返送させる理由が無い)。
  const title = favoriteSearchResultTitle(route.query, listing.title);
  state.collection = {
    kind: "favorite_search",
    value: route.searchKind,
    title,
    truncated: Boolean(listing.truncated),
    entryLimit: Number(listing.entry_limit) || 0,
    emptyMessage: favoriteSearchEmptyMessage(data.index_state, entries.length),
  };
  state.gridSortState = normalizeRemoteGridSortState(listing.sort_state);
  state.gridSortScope = null;
  state.container = null;
  state.gridReturnHash = homeHash("search");
  state.gridHash = location.hash;
  state.favoriteName = title;
  state.folderPath = "";
  state.thumbAspectHeightRatio =
    Number.isFinite(Number(listing.thumb_aspect_height_ratio)) &&
    Number(listing.thumb_aspect_height_ratio) > 0
      ? Number(listing.thumb_aspect_height_ratio)
      : 1;
  state.entries = entries;
  // 今のコンテナ索引は画像を返さないが、その前提をここにもう 1 つ置かない。集約ビューと
  // 同じ導出にしておけば、返るものが変わってもセルの開き方が食い違わない。
  state.images = state.entries.filter((entry) => entry.kind === "image");
  setSinglePageGroups();
  state.gridIndex = 0;
  renderFolder();
}

async function showTagItems(route) {
  state.tagBrowseFilter = { query: route.tag, kind: route.tagKind };
  renderLoading("タグの項目を検索しています");
  const data = await apiJson("/api/tags/items", {
    tag: route.tag,
    kind: route.tagKind,
  });
  const listing = data.listing ?? {};
  const entries = listing.entries ?? [];
  const title = tagItemsResultTitle(route.tag, listing.title);
  state.collection = {
    kind: "tag_items",
    value: route.tagKind,
    title,
    truncated: Boolean(listing.truncated),
    entryLimit: Number(listing.entry_limit) || 0,
    emptyMessage: tagItemsEmptyMessage(data.state, entries.length),
  };
  state.gridSortState = normalizeRemoteGridSortState(listing.sort_state);
  state.gridSortScope = null;
  state.container = null;
  state.gridReturnHash = homeHash("tags");
  state.gridHash = location.hash;
  state.favoriteName = title;
  state.folderPath = "";
  state.thumbAspectHeightRatio =
    Number.isFinite(Number(listing.thumb_aspect_height_ratio)) &&
    Number(listing.thumb_aspect_height_ratio) > 0
      ? Number(listing.thumb_aspect_height_ratio)
      : 1;
  state.entries = entries;
  state.images = state.entries.filter((entry) => entry.kind === "image");
  setSinglePageGroups();
  state.gridIndex = 0;
  renderFolder();
}

const FAVORITE_SEARCH_TITLE_MAX_CHARS = 30;

/// 結果画面の題。長い語句はパンくずを押し出すので表示だけ丸めるが、丸めるのは見た目に
/// 限る (route と入力欄の語句はそのまま)。
export function favoriteSearchResultTitle(query, fallback = "検索結果") {
  const trimmed = (query ?? "").trim();
  if (!trimmed) return fallback;
  const characters = Array.from(trimmed);
  const shown =
    characters.length > FAVORITE_SEARCH_TITLE_MAX_CHARS
      ? `${characters.slice(0, FAVORITE_SEARCH_TITLE_MAX_CHARS).join("")}…`
      : trimmed;
  return `「${shown}」の検索結果`;
}

export function favoriteSearchEmptyMessage(indexState, entryCount = 0) {
  if (entryCount > 0) return "";
  if (indexState === "disabled") {
    return "お気に入りにコンテナ索引が設定されていません。mIV 本体のお気に入り編集で設定できます。";
  }
  if (indexState === "unavailable") {
    return "コンテナ索引をまだ利用できません。しばらくしてからもう一度検索してください。";
  }
  return "一致するフォルダ・ZIP・PDF はありませんでした。";
}

export function tagItemsResultTitle(tag, fallback = "タグの項目") {
  const trimmed = String(tag ?? "").trim().replace(/^#+/, "");
  if (!trimmed) return fallback;
  const displayTag = `#${trimmed}`;
  const characters = Array.from(displayTag);
  const shown =
    characters.length > FAVORITE_SEARCH_TITLE_MAX_CHARS
      ? `${characters.slice(0, FAVORITE_SEARCH_TITLE_MAX_CHARS).join("")}…`
      : displayTag;
  return `「${shown}」の項目`;
}

export function tagItemsEmptyMessage(indexState, entryCount = 0) {
  if (entryCount > 0) return "";
  if (indexState === "empty") return "タグはまだ 1 つもありません。";
  if (indexState === "unavailable") return "タグをまだ利用できません。";
  return "このタグの項目は見つかりませんでした。";
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

function collectionSortScope(kind, value) {
  const collection = kind === "rating"
    ? { type: "rating", stars: Number(value) }
    : kind === "smart"
      ? { type: "smart_folder", definition_id: value }
      : { type: kind };
  return { kind: "collection", collection };
}

export function normalizeRemoteGridSortState(value) {
  if (!value || typeof value !== "object" || !Array.isArray(value.options)) return null;
  const options = value.options.filter(
    (option) => option &&
      typeof option.value === "string" &&
      typeof option.label === "string" &&
      typeof option.short_label === "string"
  ).map((option) => ({
    value: option.value,
    label: option.label,
    short_label: option.short_label,
  }));
  if (!options.length || !options.some((option) => option.value === value.selected)) {
    return null;
  }
  return {
    selected: value.selected,
    options,
    locked_reason: typeof value.locked_reason === "string" && value.locked_reason
      ? value.locked_reason
      : null,
  };
}

async function showFolder(path) {
  const startedAt = performance.now();
  renderLoading("フォルダを読み込んでいます");
  const loaded = await loadFolder(path, startedAt);
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
  if (
    options.requiredAddress &&
    remoteBookBookmarkTargetEntryIndex(data.entries, options.requiredAddress) < 0
  ) {
    return false;
  }
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
  const rootReturnHash = rootOpenReturnHash({
    hasCollection: Boolean(state.collection),
    atFavoriteRoot: isFavoriteRoot(effectiveAddress.path),
    collectionHash: state.gridHash,
    fallbackHash: containerParentHash(address),
  });
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
  state.gridSortState = normalizeRemoteGridSortState(data.sort_state);
  state.gridSortScope = { kind: "address", address: effectiveAddress };
  state.favoriteName =
    data.root_name ??
    favoriteForPath(effectiveAddress.path)?.name ??
    "項目";
  state.folderPath = effectiveAddress.path;
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
      ? folderHash(effectiveAddress.path)
      : containerHash(effectiveAddress);
  state.gridReturnHash = rootReturnHash;
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

export function rootOpenReturnHash({
  hasCollection,
  atFavoriteRoot,
  collectionHash,
  fallbackHash,
}) {
  return hasCollection && atFavoriteRoot ? collectionHash : fallbackHash;
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

function pageRenderContextForEntry(entry) {
  const contextAddress = state.container?.address;
  const groupIndex = pageGroupIndexForEntry(entry);
  const group = state.pageGroups[groupIndex];
  if (!contextAddress || !group) return null;
  const pageIndex = group.entries.findIndex(
    (page) => entryIdentity(page) === entryIdentity(entry)
  );
  if (pageIndex < 0) return null;
  const partnerIndex = viewerSpreadPartnerIndex(group.entries.length, pageIndex);
  const spreadPartner = partnerIndex === null
    ? null
    : entryAddress(group.entries[partnerIndex]);
  return {
    context_address: contextAddress,
    display_slot: viewerPageDisplaySlot(group.entries.length, pageIndex),
    spread_partner: spreadPartner,
  };
}

function currentPageGroup() {
  return state.pageGroups[state.pageGroupIndex] ?? null;
}

function viewerPageContextIdentity() {
  const contextAddress = state.container?.requestedAddress;
  const locationIdentity = contextAddress
    ? addressIdentity(contextAddress)
    : String(state.folderPath ?? "");
  return `${state.screenContext ?? ""}\n${locationIdentity}`;
}

function captureViewerPageGroupRequest(
  viewer = state.viewer,
  groupIndex = state.pageGroupIndex
) {
  const group = state.pageGroups[groupIndex];
  if (!viewer || !group || !Number.isInteger(groupIndex) || groupIndex < 0) {
    return null;
  }
  return {
    viewer,
    pageGroups: state.pageGroups,
    group,
    groupIndex,
    groupIdentity: group.entries.map(entryIdentity).join("\n"),
    contextIdentity: viewerPageContextIdentity(),
  };
}

function viewerSeekSnapshot(groupIndex = state.pageGroupIndex) {
  return viewerSeekState({
    groupPageIndexes: state.seekPageGroups,
    currentGroupIndex: groupIndex,
    pageCount: state.images.length,
    rtl: isRtlReadingDirection(state.readingDirection),
  });
}

function viewerPagePresentation(groupIndex = state.pageGroupIndex) {
  const group = state.pageGroups[groupIndex];
  if (!group) return null;
  return {
    name: group.entries.map((entry) => entry.name).join(" / "),
    seekState: viewerSeekSnapshot(groupIndex),
  };
}

function updateRequestedPageGroup(groupIndex) {
  const viewer = state.viewer;
  const group = state.pageGroups[groupIndex];
  const displayedGroupIndex = viewer?.displayedGroupIndex();
  if (!group || !Number.isInteger(displayedGroupIndex)) return false;
  const position = viewerPagePositionTransition(
    {
      requestedGroupIndex: state.pageGroupIndex,
      displayedGroupIndex,
    },
    { type: ViewerPagePositionEvent.REQUEST, groupIndex }
  );
  if (position.requestedGroupIndex === state.pageGroupIndex) {
    viewer.restoreRequestedPagePresentation();
    return false;
  }
  viewer.hideBoundaryMessage();
  state.pageDirection =
    position.requestedGroupIndex > state.pageGroupIndex ? 1 : -1;
  state.pageGroupIndex = position.requestedGroupIndex;
  const entry = group.anchor;
  updateGridReturnTargetItem(entry);
  state.imageIndex = state.images.findIndex(
    (image) => entryIdentity(image) === entryIdentity(entry)
  );
  const presentation = viewerPagePresentation(position.requestedGroupIndex);
  viewer.setRequestedPagePresentation(presentation);
  document.title = `${entry.name} — mIV Remote`;
  return true;
}

function discardRequestedPageGroup(
  viewer,
  requestedGroupIndex,
  positionRequest = null
) {
  if (
    state.viewer !== viewer ||
    state.pageGroupIndex !== requestedGroupIndex
  ) return false;
  if (
    positionRequest &&
    !viewerPageGroupRequestMatches(
      positionRequest,
      captureViewerPageGroupRequest(viewer, requestedGroupIndex)
    )
  ) return false;
  const displayedGroupIndex = viewer.displayedGroupIndex();
  if (!Number.isInteger(displayedGroupIndex)) return false;
  const position = viewerPagePositionTransition(
    {
      requestedGroupIndex: state.pageGroupIndex,
      displayedGroupIndex,
    },
    { type: ViewerPagePositionEvent.DISCARD, groupIndex: requestedGroupIndex }
  );
  if (!state.pageGroups[position.requestedGroupIndex]) return false;
  if (position.requestedGroupIndex !== state.pageGroupIndex) {
    state.pageDirection =
      position.requestedGroupIndex > state.pageGroupIndex ? 1 : -1;
    state.pageGroupIndex = position.requestedGroupIndex;
    const entry = state.pageGroups[position.requestedGroupIndex].anchor;
    updateGridReturnTargetItem(entry);
    state.imageIndex = state.images.findIndex(
      (image) => entryIdentity(image) === entryIdentity(entry)
    );
    document.title = `${entry.name} — mIV Remote`;
  }
  viewer.setRequestedPagePresentation(
    viewerPagePresentation(position.requestedGroupIndex)
  );
  return true;
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
    path: state.folderPath,
    subresource: { kind: "file" },
  };
  const pageIndex = state.entries.findIndex(
    (candidate) => entryIdentity(candidate) === entryIdentity(entry)
  );
  if (!address?.path || !contextAddress?.path || pageIndex < 0) return null;
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
    .then(() => apiAddressPostJson("/api/write", effect.request))
    .catch((error) => {
      recordClientError("reading_progress_write_error", error);
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
    const response = await apiAddressPostJson("/api/write", {
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
    await apiAddressPostJson("/api/write", {
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
    await apiAddressPostJson("/api/write", {
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

async function openRemoteBookBookmarkTarget(target) {
  const viewer = state.viewer;
  if (!viewer || !target?.address || !target?.contextAddress) return false;
  const loaded = await loadContainer(target.contextAddress, {
    forceSinglePage: shouldForceSinglePageForViewport(),
    requiredAddress: target.address,
  });
  if (!loaded || state.viewer !== viewer || state.screenContext !== "viewer") return false;

  const entryIndex = remoteBookBookmarkTargetEntryIndex(state.entries, target.address);
  const entry = state.entries[entryIndex];
  const imageIndex = state.images.findIndex(
    (candidate) => addressIdentity(entryAddress(candidate)) === addressIdentity(target.address)
  );
  const groupIndex = imageIndex >= 0 ? pageGroupIndexForEntry(state.images[imageIndex]) : -1;
  const group = state.pageGroups[groupIndex];
  if (!entry || imageIndex < 0 || !group) return false;

  state.pageDirection = 1;
  state.pageGroupIndex = groupIndex;
  state.imageIndex = state.images.findIndex(
    (candidate) => entryIdentity(candidate) === entryIdentity(group.anchor)
  );
  updateGridReturnTargetItem(group.anchor);
  viewer.hideBoundaryMessage();
  const viewerDepth = (Number(history.state?.viewerDepth) || 0) + 1;
  history.pushState(
    {
      ...(history.state ?? {}),
      mivRoute: true,
      viewerFromGrid: Boolean(history.state?.viewerFromGrid),
      viewerDepth,
    },
    "",
    mediaHash(entryAddress(group.anchor))
  );
  await updateViewerImage(performance.now());
  return true;
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

/// 戻り先は `state.gridHash` として分かっている。履歴を遡るのは、ブラウザの戻る
/// スタックを素直に保つための最適化でしかない。`history.go(-N)` は N が実際の履歴数を
/// 超えると**仕様上なにも起こさない**ので、それだけに頼ると本を閉じられなくなる。
/// アプリのリロードを挟むと、履歴は作り直されるのに viewerDepth は復元された state から
/// 読まれるため、実体より大きな N で呼ぶ状態が普通に起きる。
async function leaveViewerForGrid() {
  await flushReadingProgress();
  const viewerDepth = Number(history.state?.viewerDepth) || 0;
  const traversable = Boolean(history.state?.viewerFromGrid) && viewerDepth > 0;
  if (traversable) {
    // popstate が来れば dispatchRoute が画面を替える。届かなければ go は範囲外だった。
    const traversed = await new Promise((resolve) => {
      const done = (value) => {
        window.removeEventListener("popstate", onPopState);
        clearTimeout(timer);
        resolve(value);
      };
      const onPopState = () => done(true);
      const timer = setTimeout(() => done(false), VIEWER_EXIT_HISTORY_TIMEOUT_MS);
      window.addEventListener("popstate", onPopState, { once: true });
      history.go(-viewerDepth);
    });
    // popstate が来ても画面が替わるとは限らない。popstate の購読者は操作権が
    // 有効なときだけ経路を切り替えるので、届いたことではなく出られたことで判断する。
    if (traversed) {
      await nextFrame();
      if (state.screenContext !== "viewer") return;
    }
  }
  enqueueTelemetry({
    type: "viewer_exit",
    used_history: traversable,
    viewer_depth: Math.min(999, Math.max(0, viewerDepth)),
    fell_back: traversable,
  });
  history.replaceState({ mivRoute: true }, "", state.gridHash);
  await dispatchRoute();
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
    let writeError = null;
    try {
      await apiAddressPostJson("/api/write", writeRequest);
    } catch (error) {
      writeError = error;
    }
    if (
      sequence === spreadWriteSequence &&
      state.container &&
      addressIdentity(state.container.requestedAddress) === identity
    ) {
      const refresh = await refreshContainerSpread();
      if (writeError) {
        state.viewer?.showBoundaryMessage(
          writeError instanceof Error
            ? writeError.message
            : "見開き設定を保存できませんでした。"
        );
      } else if (refresh.outcome === ViewerGroupLoadOutcome.FAILED) {
        state.viewer?.showBoundaryMessage(refresh.message);
      }
    }
  });
  return true;
}

function refreshContainerSpread(
  forceSinglePage = shouldForceSinglePageForViewport(),
  renderTrigger = "spread_refresh"
) {
  if (!state.container) return Promise.resolve(VIEWER_GROUP_LOAD_SUPERSEDED);
  const viewer = state.viewer;
  const current = currentPageGroup()?.anchor ?? state.images[state.imageIndex];
  const currentIdentity = current ? entryIdentity(current) : "";
  const address = state.container.requestedAddress;
  if (!viewer || !currentIdentity) {
    return Promise.resolve({
      outcome: ViewerGroupLoadOutcome.FAILED,
      message: "現在のページを再構成できませんでした。",
    });
  }
  return containerSpreadRefreshOwner.enqueue({
    address,
    addressIdentity: addressIdentity(address),
    currentIdentity,
    forceSinglePage,
    renderTrigger,
    viewer,
  });
}

function containerSpreadRefreshContextExitReason(request, duringLoad = false) {
  if (state.viewer !== request.viewer) {
    return duringLoad
      ? ContainerSpreadRefreshExitReason.VIEWER_CHANGED_DURING_LOAD
      : ContainerSpreadRefreshExitReason.VIEWER_CHANGED_BEFORE_LOAD;
  }
  if (!state.container) {
    return duringLoad
      ? ContainerSpreadRefreshExitReason.CONTAINER_MISSING_DURING_LOAD
      : ContainerSpreadRefreshExitReason.CONTAINER_MISSING_BEFORE_LOAD;
  }
  if (
    addressIdentity(state.container.requestedAddress) !== request.addressIdentity
  ) {
    return duringLoad
      ? ContainerSpreadRefreshExitReason.CONTAINER_CHANGED_DURING_LOAD
      : ContainerSpreadRefreshExitReason.CONTAINER_CHANGED_BEFORE_LOAD;
  }
  return null;
}

function recordContainerSpreadRefreshOutcome(request, outcome, reason, extra = {}) {
  enqueueTelemetry({
    type: "spread_refresh",
    outcome,
    reason,
    render_trigger: request?.renderTrigger ?? "spread_refresh",
    ...extra,
  });
}

function supersedeContainerSpreadRefresh(request, reason, stage) {
  recordContainerSpreadRefreshOutcome(request, "superseded", reason, { stage });
  return { outcome: ViewerGroupLoadOutcome.SUPERSEDED, reason };
}

export function reportContainerSpreadRefreshError(
  error,
  request = null,
  {
    reason = ContainerSpreadRefreshExitReason.UNEXPECTED_ERROR,
    stage = "unknown",
  } = {}
) {
  recordClientError("spread_refresh_error", error, {
    reason,
    stage,
    render_trigger: request?.renderTrigger ?? "spread_refresh",
  });
  return {
    outcome: ViewerGroupLoadOutcome.FAILED,
    reason,
    message: CONTAINER_SPREAD_REFRESH_FAILURE_MESSAGE,
  };
}

async function performContainerSpreadRefresh(request) {
  let stage = "preflight";
  try {
    const initialExit = containerSpreadRefreshContextExitReason(request, false);
    if (initialExit) {
      return supersedeContainerSpreadRefresh(request, initialExit, stage);
    }

    stage = "container_load";
    let loaded;
    try {
      loaded = await loadContainer(request.address, {
        forceSinglePage: request.forceSinglePage,
      });
    } catch (error) {
      if (error?.name === "AbortError") {
        const contextExit = containerSpreadRefreshContextExitReason(request, true);
        if (contextExit) {
          return supersedeContainerSpreadRefresh(request, contextExit, stage);
        }
        return reportContainerSpreadRefreshError(error, request, {
          reason: ContainerSpreadRefreshExitReason.CONTAINER_LOAD_ABORTED,
          stage,
        });
      }
      throw error;
    }

    const loadedContextExit = containerSpreadRefreshContextExitReason(request, true);
    if (loadedContextExit) {
      return supersedeContainerSpreadRefresh(request, loadedContextExit, stage);
    }
    if (!loaded) {
      return supersedeContainerSpreadRefresh(
        request,
        ContainerSpreadRefreshExitReason.CONTAINER_LOAD_NOT_APPLIED,
        stage
      );
    }

    stage = "resolve_current_page";
    const imageIndex = state.images.findIndex(
      (entry) => entryIdentity(entry) === request.currentIdentity
    );
    if (imageIndex < 0) {
      return reportContainerSpreadRefreshError(
        new Error("更新後のコンテナに現在のページがありません。"),
        request,
        {
          reason: ContainerSpreadRefreshExitReason.CURRENT_PAGE_MISSING,
          stage,
        }
      );
    }
    const groupIndex = pageGroupIndexForEntry(state.images[imageIndex]);
    if (groupIndex < 0) {
      return reportContainerSpreadRefreshError(
        new Error("更新後の見開きグループを特定できませんでした。"),
        request,
        {
          reason: ContainerSpreadRefreshExitReason.GROUP_MISSING,
          stage,
        }
      );
    }

    stage = "viewer_update";
    const group = state.pageGroups[groupIndex];
    state.pageGroupIndex = groupIndex;
    state.imageIndex = state.images.findIndex(
      (entry) => entryIdentity(entry) === entryIdentity(group.anchor)
    );
    updateGridReturnTargetItem(group.anchor);
    const displayResult = await updateViewerImage(performance.now(), {
      renderTrigger: request.renderTrigger,
    });
    if (displayResult?.outcome === ViewerGroupLoadOutcome.APPLIED) {
      recordContainerSpreadRefreshOutcome(request, "applied", "display_applied");
      return VIEWER_GROUP_LOAD_APPLIED;
    }
    if (displayResult?.outcome === ViewerGroupLoadOutcome.SUPERSEDED) {
      recordContainerSpreadRefreshOutcome(
        request,
        "superseded",
        ContainerSpreadRefreshExitReason.DISPLAY_SUPERSEDED,
        { stage, viewer_update_reason: displayResult.reason ?? "unknown" }
      );
      return {
        outcome: ViewerGroupLoadOutcome.SUPERSEDED,
        reason: ContainerSpreadRefreshExitReason.DISPLAY_SUPERSEDED,
      };
    }
    if (displayResult?.outcome === ViewerGroupLoadOutcome.FAILED) {
      recordClientError(
        "spread_refresh_error",
        new Error(displayResult.message || "viewer update failed"),
        {
          reason: ContainerSpreadRefreshExitReason.DISPLAY_FAILED,
          stage,
          render_trigger: request.renderTrigger,
          viewer_update_reason: displayResult.reason ?? "load_failed",
        }
      );
      recordContainerSpreadRefreshOutcome(
        request,
        "failed",
        ContainerSpreadRefreshExitReason.DISPLAY_FAILED,
        { stage, viewer_update_reason: displayResult.reason ?? "load_failed" }
      );
      return {
        outcome: ViewerGroupLoadOutcome.FAILED,
        reason: ContainerSpreadRefreshExitReason.DISPLAY_FAILED,
        message: CONTAINER_SPREAD_REFRESH_FAILURE_MESSAGE,
      };
    }
    throw new TypeError("viewer update returned an unknown outcome");
  } catch (error) {
    return reportContainerSpreadRefreshError(error, request, {
      reason: stage === "container_load"
        ? ContainerSpreadRefreshExitReason.CONTAINER_LOAD_FAILED
        : ContainerSpreadRefreshExitReason.UNEXPECTED_ERROR,
      stage,
    });
  }
}

export async function loadFolder(
  path,
  interactionStartedAt = performance.now()
) {
  const fetchStartedAt = performance.now();
  const requestedPath = path;
  const sameFolder = state.folderPath === requestedPath;
  state.requestController?.abort();
  state.folderContainerLoad = null;
  const controller = new AbortController();
  state.requestController = controller;
  const folderAddress = {
    path: requestedPath,
    subresource: { kind: "file" },
  };
  const forceSinglePage = containerForceSinglePage();
  const listPromise = apiJson(
    "/api/list",
    { path: requestedPath },
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
  const rootReturnHash = rootOpenReturnHash({
    hasCollection: Boolean(state.collection),
    atFavoriteRoot: isFavoriteRoot(requestedPath),
    collectionHash: state.gridHash,
    fallbackHash:
      typeof globalThis.history?.state?.returnHash === "string"
        ? globalThis.history.state.returnHash
        : homeHash("favorites"),
  });
  state.collection = null;
  state.container = null;
  state.gridSortState = normalizeRemoteGridSortState(data.sort_state);
  state.gridSortScope = {
    kind: "address",
    address: {
      path: data.path ?? requestedPath,
      subresource: { kind: "file" },
    },
  };
  state.gridReturnHash = rootReturnHash;
  state.favoriteName =
    data.root_name ??
    favoriteForPath(data.path ?? requestedPath)?.name ??
    "項目";
  state.folderPath = data.path ?? requestedPath;
  state.gridHash = folderHash(state.folderPath);
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
      entry.kind === "audio" ||
      entry.kind === "zip" ||
      entry.kind === "pdf" ||
      entry.kind === "archive"
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
        ["zip", "pdf", "archive"].includes(entry.kind)
      ).length,
    },
    requestController: controller,
    containerLoad,
  };
}

function renderFolder(listMetrics = null, preserveRequestController = null) {
  const renderStartedAt = performance.now();
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
  const sortBar = createGridSortBar();

  const scroll = element("div", "grid-scroll");
  const thumbnailNotice = textElement("p", "", "thumbnail-service-notice");
  thumbnailNotice.hidden = true;
  state.thumbnailNotice = thumbnailNotice;
  const gridActionNotice = textElement(
    "p",
    "",
    "thumbnail-service-notice grid-action-notice"
  );
  gridActionNotice.hidden = true;
  gridActionNotice.setAttribute("role", "status");
  gridActionNotice.setAttribute("aria-live", "polite");
  state.gridActionNotice = gridActionNotice;
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
  screen.append(
    topbar,
    sortBar,
    collectionLimitNotice,
    gridActionNotice,
    thumbnailNotice,
    scroll
  );
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
      state.collection?.emptyMessage || (state.container
        ? "このコンテナには表示できるページがありません。"
        : "このフォルダには表示できるサブフォルダまたは画像がありません。"),
      "empty-state center-status"
    );
    scroll.replaceChildren(empty);
    state.thumbnailTracker.begin([]);
    return;
  }

  const imageIndexes = new Map(
    state.images.map((entry, index) => [entryIdentity(entry), index])
  );
  const gridItemIdentities = state.entries.map(gridReturnItemIdentity);
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
    labelHeight,
    {
      context: state.gridHash,
      itemIdentities: gridItemIdentities,
    }
  );
  const gridViewport = state.gridViewportMemory.recall(state.gridHash);
  const returnViewport = resolveGridReturnViewport({
    ...gridViewport,
    itemIdentities: state.virtualGrid.itemIdentities,
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
      state.virtualGrid.selectReturnAnchor(state.gridIndex);
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

function createGridSortBar() {
  const bar = element("div", "grid-sort-bar");
  const sortState = state.gridSortState;
  if (!sortState) {
    bar.hidden = true;
    return bar;
  }
  const label = textElement("label", "並べ替え", "grid-sort-label");
  const select = document.createElement("select");
  select.className = "grid-sort-select";
  select.setAttribute("aria-label", "一覧の並べ替え");
  for (const optionState of sortState.options) {
    const option = document.createElement("option");
    option.value = optionState.value;
    option.textContent = optionState.label;
    select.append(option);
  }
  select.value = sortState.selected;
  const reason = textElement(
    "span",
    sortState.locked_reason ?? "",
    "grid-sort-reason"
  );
  reason.hidden = !sortState.locked_reason;
  select.disabled = Boolean(sortState.locked_reason) || state.gridSortWritePending;
  if (sortState.locked_reason) {
    select.title = sortState.locked_reason;
    reason.setAttribute("role", "status");
  }
  select.addEventListener("change", () => {
    const selected = select.value;
    select.disabled = true;
    reason.hidden = false;
    reason.textContent = "保存中…";
    changeGridSortOrder(selected).catch((error) => {
      select.value = sortState.selected;
      select.disabled = Boolean(sortState.locked_reason);
      reason.hidden = false;
      reason.textContent = error instanceof Error
        ? error.message
        : "並べ替えを保存できませんでした。";
      reason.classList.add("is-error");
    });
  });
  label.append(select);
  bar.append(label, reason);
  return bar;
}

async function changeGridSortOrder(sortOrder) {
  if (!state.gridSortScope || state.gridSortState?.locked_reason) return;
  state.gridSortWritePending = true;
  try {
    const response = await apiAddressPostJson("/api/write", {
      kind: "set_sort_order",
      scope: state.gridSortScope,
      sort_order: sortOrder,
    });
    const next = normalizeRemoteGridSortState(response.sort_state);
    if (!next) throw new Error("並べ替えの保存結果を取得できませんでした。");
    state.gridSortState = next;
    applyRemoteStateGeneration(response.remote_state_generation, { reloadViewer: false });
  } finally {
    // 保存はここで終わり。この後の再描画は保存の一部ではない。印を付けたまま一覧を
    // 描き直すと、新しい並べ替えバーが「保存中」の姿で作られ、二度と操作できなくなる。
    state.gridSortWritePending = false;
  }
  rememberCurrentGridViewport();
  renderLoading("並べ替えています");
  await dispatchRoute();
}

function buildBreadcrumbs() {
  const breadcrumbs = element("nav", "breadcrumbs");
  breadcrumbs.setAttribute("aria-label", "パンくず");
  if (state.collection) {
    breadcrumbs.append(textElement("h1", state.collection.title));
    return breadcrumbs;
  }
  if (state.container && state.container.kind !== "folder") {
    const pathCrumbs = remotePathBreadcrumbs(state.container.address.path);
    const file = pathCrumbs.pop();
    const fileName = file?.label ?? state.container.title;
    const crumbs = pathCrumbs.map((crumb) => ({
      label: crumb.label,
      command: { kind: "folder", path: crumb.path },
    }));
    const rootAddress = {
      path: state.container.address.path,
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
              path: state.container.address.path,
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
  const crumbs = remotePathBreadcrumbs(state.folderPath);

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

  if (entryIsFolder(entry)) {
    preview.append(textElement("span", "◆", "folder-glyph"));
    preview.append(image);
    preview.append(textElement("span", "folder", "type-badge"));
    tile.addEventListener("click", (event) => {
      commandDispatcher(
        command(CommandName.OPEN, {
          kind: "folder",
          path: entryPath(entry),
          entryIndex,
        }),
        { source: inputSourceFromEvent(event), detail: "grid_tile" }
      );
    });
  } else {
    if (entry.kind === "audio") {
      preview.append(createAudioThumbnailIcon());
    } else {
      preview.append(textElement("span", "◇", "file-glyph"));
      preview.append(image);
    }
    if (entry.kind !== "image") {
      preview.append(textElement("span", entryTypeLabel(entry.kind), "type-badge"));
    }
    if (entry.kind === "image") {
      tile.addEventListener("click", (event) => {
        const index = imageIndexes.get(entryIdentity(entry));
        if (index !== undefined) {
          const payload = entry.address || entryPath(entry) === ""
            ? {
                kind: "media",
                mediaKind: "image",
                address: entryAddress(entry),
                imageIndex: index,
                entryIndex,
              }
            : {
                kind: "image",
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
    } else if (isStreamMediaKind(entry.kind)) {
      tile.addEventListener("click", (event) => {
        commandDispatcher(command(CommandName.OPEN, {
          kind: "media",
          mediaKind: entry.kind,
          address: entryAddress(entry),
          entryIndex,
        }), {
          source: inputSourceFromEvent(event),
          detail: "grid_tile",
          at: performance.now(),
        });
      });
    } else if (entry.kind === "archive") {
      tile.addEventListener("click", (event) => {
        commandDispatcher(command(CommandName.OPEN, {
          kind: "archive",
          address: entryAddress(entry),
          name: entry.name,
          entryIndex,
        }), {
          source: inputSourceFromEvent(event),
          detail: "grid_tile",
          at: performance.now(),
        });
      });
    } else if (unsupportedRemoteEntryMessage(entry.kind)) {
      tile.addEventListener("click", (event) => {
        commandDispatcher(
          command(CommandName.OPEN, {
            kind: "unsupported",
            entryKind: entry.kind,
            entryIndex,
          }),
          {
            source: inputSourceFromEvent(event),
            detail: "grid_tile",
            at: performance.now(),
          }
        );
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
  // Audio and unopened convertible archives have no thumbnail source. Keeping
  // them out of the binding avoids guaranteed-to-fail /api/thumb requests.
  if (entry.kind !== "audio" && entry.kind !== "archive") {
    tile._thumbnailBinding = { image, entry, tracker: thumbnailTracker, cellWidth };
  }
  return tile;
}

function createAudioThumbnailIcon() {
  const namespace = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(namespace, "svg");
  svg.setAttribute("class", "audio-thumbnail-icon");
  svg.setAttribute("viewBox", "0 0 64 64");
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", "音声");

  const beam = document.createElementNS(namespace, "path");
  beam.setAttribute("d", "M22 17 H46 M22 17 V45 M46 17 V45");
  beam.setAttribute("fill", "none");
  beam.setAttribute("stroke", "currentColor");
  beam.setAttribute("stroke-width", "5");
  beam.setAttribute("stroke-linecap", "round");
  beam.setAttribute("stroke-linejoin", "round");

  const leftHead = document.createElementNS(namespace, "circle");
  leftHead.setAttribute("cx", "16");
  leftHead.setAttribute("cy", "46");
  leftHead.setAttribute("r", "8");
  leftHead.setAttribute("fill", "currentColor");

  const rightHead = document.createElementNS(namespace, "circle");
  rightHead.setAttribute("cx", "40");
  rightHead.setAttribute("cy", "46");
  rightHead.setAttribute("r", "8");
  rightHead.setAttribute("fill", "currentColor");

  svg.append(beam, leftHead, rightHead);
  return svg;
}

function entryPath(entry) {
  return entry.path ?? "";
}

function entryAddress(entry) {
  if (entry.address) return entry.address;
  return {
    path: entryPath(entry),
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
  return remoteAddressIdentity(address);
}

function entryIdentity(entry) {
  return entry.address
    ? addressIdentity(entry.address)
    : entryPath(entry);
}

export function gridReturnItemIdentity(entry) {
  return addressIdentity(entryAddress(entry));
}

function addressQueryParams(address, extra = {}) {
  const params = {
    path: address.path,
    ...extra,
  };
  const target = address.subresource;
  if (target.kind === "zip_entry") params.entry = target.entry_name;
  else if (target.kind === "zip_directory") params.prefix = target.prefix;
  else if (target.kind === "pdf_page") params.page = target.page_number;
  return params;
}

export function thumbnailRequestQueryForEntry(entry, extra = {}) {
  const address = entryAddress(entry);
  const source = thumbnailAddressForEntry(entry);
  const params = addressQueryParams(address, extra);
  if (addressIdentity(source) !== addressIdentity(address)) {
    params.thumbnail_source_path = source.path;
  }
  return params;
}

export function parentContainerAddress(address) {
  if (address.subresource.kind === "file") {
    return {
      path: parentPath(address.path),
      subresource: { kind: "file" },
    };
  }
  if (address.subresource.kind === "pdf_page") {
    return {
      path: address.path,
      subresource: { kind: "file" },
    };
  }
  const segments = address.subresource.entry_name.split("/");
  segments.pop();
  const prefix = segments.length ? segments.join("/") + "/" : "";
  return {
    path: address.path,
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
      path: requestedAddress.path,
      subresource: prefix
        ? { kind: "zip_directory", prefix }
        : { kind: "file" },
    };
    return containerHash(parentAddress);
  }
  return folderHash(parentPath(requestedAddress.path));
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
  if (!entry || !isStreamMediaKind(entry.kind)) {
    recordClientError("video_viewer_entry_rejected", "メディアビューアに未対応の項目が渡されました", {
      entry_found: Boolean(entry),
      resolved_kind: entry?.kind ?? "missing",
      screen_context: state.screenContext,
    });
    return false;
  }
  cleanupScreen();
  state.screenContext = "viewer";
  updateGridReturnTargetItem(entry);
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
    subscribeRemoteSessionState: (listener) =>
      remoteSessionControlOwner.subscribe(listener),
    reportPlaybackIssue: ({ category, internalReason, ...details }) => {
      recordClientError(category, internalReason, {
        internal_reason: internalReason,
        ...details,
      });
    },
    publishVideoHealth: (snapshot, { telemetry = false } = {}) => {
      hudState.video = snapshot;
      updateHud();
      if (telemetry && snapshot) enqueueTelemetry(snapshot);
    },
    getTelemetryDebugContext: () => ({
      enabled: state.localSettings.telemetryDebugDetails,
    }),
    keyboardAvailable: shouldShowKeyboardShortcuts({
      coarsePointer: state.coarsePointer,
      keyboardUsed: state.keyboardInputSeen,
    }),
    getPanelTab: () => state.videoPanelTab,
    setPanelTab: (tabId) => { state.videoPanelTab = tabId; },
  });
  if (!state.viewerBarsVisible) viewer.setBarsVisible(false);
  state.viewer = viewer;
  state.commandMenu = viewer.menu;
  app.append(viewer.root);
  viewer.captureVideoHealth("hud");
  viewer.start().catch((error) => {
    if (state.viewer === viewer) {
      viewer.showOperationalError(error, "動画を開始できませんでした");
    }
  });
  return true;
}

// Keep the renderer selection in one production function so tests cover the dispatch that
// follows resolveMediaOpenRoute, not just the resolver's return value.
export function renderResolvedMediaOpen(
  mediaRoute,
  addressedEntry,
  imageIndex,
  startedAt,
  renderMedia = renderVideoViewer,
  renderImage = renderImageViewer
) {
  if (isStreamMediaKind(mediaRoute)) {
    return renderMedia(addressedEntry) ? "media" : "rejected";
  }
  renderImage(imageIndex, startedAt);
  return "image";
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
  const mediaKind = viewer.entry?.kind ?? "video";
  const mediaEntries = state.entries.filter((entry) => entry.kind === mediaKind);
  const current = mediaEntries.findIndex(
    (entry) => addressIdentity(entryAddress(entry)) === addressIdentity(viewer.address)
  );
  const nextIndex = videoFileTargetIndex(current, mediaEntries.length, delta, wrap);
  if (nextIndex < 0) {
    const noun = mediaKind === "audio" ? "音声" : "動画";
    viewer.showBoundaryMessage(Number(delta) < 0 ? `先頭の${noun}です` : `最後の${noun}です`);
    return { handled: true, advanced: false };
  }
  const entry = mediaEntries[nextIndex];
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
  cleanupScreen();
  state.screenContext = "viewer";
  updateGridReturnTargetItem(state.pageGroups[groupIndex].anchor);
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
  state.viewer.initializePagePresentation(viewerPagePresentation(groupIndex));
  state.remoteAiController = new RemoteAiController(
    state.viewer,
    stage,
    (listener) => remoteSessionControlOwner.subscribe(listener)
  );
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
  let seekPointerDrag = null;
  let seekCommitSequence = 0;
  const previewSeekGroup = (rawGroupIndex) => {
    const groupIndex = viewerSeekGroupIndex(rawGroupIndex, state.pageGroups.length);
    state.viewer?.setSeekState(viewerSeekSnapshot(groupIndex));
    return groupIndex;
  };
  const commitSeekGroup = (groupIndex, reason) => {
    const viewer = state.viewer;
    if (!viewer || !updateRequestedPageGroup(groupIndex)) return;
    const positionRequest = captureViewerPageGroupRequest(viewer, groupIndex);
    const sequence = ++seekCommitSequence;
    acquireRemoteSession(reason).then((acquired) => {
      if (!acquired) {
        if (sequence === seekCommitSequence) {
          discardRequestedPageGroup(viewer, groupIndex, positionRequest);
        }
        return;
      }
      if (sequence !== seekCommitSequence || state.viewer !== viewer) return;
      if (!commitRequestedPageGroup(groupIndex, positionRequest)) {
        discardRequestedPageGroup(viewer, groupIndex, positionRequest);
      }
    }).catch(() => {
      if (sequence === seekCommitSequence && state.viewer === viewer) {
        discardRequestedPageGroup(viewer, groupIndex, positionRequest);
      }
    });
  };
  seekInput.addEventListener("input", (event) => {
    event.stopPropagation();
    if (seekPointerDrag) {
      state.viewer?.setSeekState(viewerSeekSnapshot(seekPointerDrag.groupIndex));
      return;
    }
    previewSeekGroup(seekInput.value);
  });
  seekInput.addEventListener("change", (event) => {
    event.stopPropagation();
    if (seekPointerDrag) return;
    commitSeekGroup(
      viewerSeekGroupIndex(seekInput.value, state.pageGroups.length),
      "viewer_seek_native"
    );
  });
  seekInput.addEventListener("pointerdown", (event) => {
    event.stopPropagation();
    if (seekInput.disabled || event.isPrimary === false) return;
    if (typeof event.button === "number" && event.button !== 0) return;
    if (event.cancelable) event.preventDefault();
    const seekState = viewerSeekSnapshot();
    const trackRect = seekInput.getBoundingClientRect();
    seekPointerDrag = {
      pointerId: event.pointerId,
      startClientX: event.clientX,
      startClientY: event.clientY,
      startGroupIndex: seekState.groupIndex,
      groupIndex: seekState.groupIndex,
      groupCount: state.pageGroups.length,
      trackLeft: trackRect.left,
      trackWidth: trackRect.width,
      direction: seekState.direction,
      maxDistancePx: 0,
    };
    // pointerdown の既定動作を止めているので、ブラウザはこの focus が指由来だと
    // 判断できない。読み込み直後のようにまだ pointer 操作を観測していない文書では
    // keyboard 由来として扱われ、指で触っただけでリングが出る。指由来であることは
    // ここで分かっているので、その事実を渡す。
    seekInput.dataset.pointerFocus = "true";
    seekInput.focus({ preventScroll: true });
    try {
      seekInput.setPointerCapture(event.pointerId);
    } catch (_error) {
      // Touch input generally has implicit capture; explicit capture may already be gone.
    }
  });
  seekInput.addEventListener("keydown", () => {
    delete seekInput.dataset.pointerFocus;
  });
  seekInput.addEventListener("blur", () => {
    delete seekInput.dataset.pointerFocus;
  });
  seekInput.addEventListener("pointermove", (event) => {
    if (!seekPointerDrag || seekPointerDrag.pointerId !== event.pointerId) return;
    event.stopPropagation();
    if (event.cancelable) event.preventDefault();
    const gesture = seekRangePointerGestureDecision({
      startClientX: seekPointerDrag.startClientX,
      startClientY: seekPointerDrag.startClientY,
      currentClientX: event.clientX,
      currentClientY: event.clientY,
      maxDistancePx: seekPointerDrag.maxDistancePx,
    });
    seekPointerDrag.maxDistancePx = gesture.maxDistancePx;
    if (gesture.kind !== "drag") return;
    seekPointerDrag.groupIndex = previewSeekGroup(viewerSeekRelativeDragValue({
      startGroupIndex: seekPointerDrag.startGroupIndex,
      startClientX: seekPointerDrag.startClientX,
      currentClientX: event.clientX,
      trackWidth: seekPointerDrag.trackWidth,
      groupCount: seekPointerDrag.groupCount,
      direction: seekPointerDrag.direction,
    }));
  });
  const finishSeekPointer = (event, cancelled) => {
    if (!seekPointerDrag || seekPointerDrag.pointerId !== event.pointerId) return;
    event.stopPropagation();
    if (event.cancelable) event.preventDefault();
    const drag = seekPointerDrag;
    const gesture = seekRangePointerGestureDecision({
      startClientX: drag.startClientX,
      startClientY: drag.startClientY,
      currentClientX: event.clientX,
      currentClientY: event.clientY,
      maxDistancePx: drag.maxDistancePx,
      cancelled,
    });
    try {
      if (seekInput.hasPointerCapture(event.pointerId)) {
        seekInput.releasePointerCapture(event.pointerId);
      }
    } catch (_error) {
      // The browser may implicitly release capture before pointercancel.
    }
    seekPointerDrag = null;
    if (gesture.kind === "cancel") {
      state.viewer?.restoreRequestedPagePresentation();
    } else if (gesture.kind === "tap") {
      const groupIndex = previewSeekGroup(seekRangeAbsoluteValue({
        clientX: drag.startClientX,
        trackLeft: drag.trackLeft,
        trackWidth: drag.trackWidth,
        min: 0,
        max: drag.groupCount - 1,
        step: 1,
        direction: drag.direction,
      }));
      commitSeekGroup(groupIndex, "viewer_seek_tap");
    } else {
      const groupIndex = previewSeekGroup(viewerSeekRelativeDragValue({
        startGroupIndex: drag.startGroupIndex,
        startClientX: drag.startClientX,
        currentClientX: event.clientX,
        trackWidth: drag.trackWidth,
        groupCount: drag.groupCount,
        direction: drag.direction,
      }));
      commitSeekGroup(groupIndex, "viewer_seek_drag");
    }
  };
  seekInput.addEventListener("pointerup", (event) => finishSeekPointer(event, false));
  seekInput.addEventListener("pointercancel", (event) => finishSeekPointer(event, true));
  seekInput.addEventListener("click", (event) => {
    event.preventDefault();
    event.stopPropagation();
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
  if (!updateRequestedPageGroup(nextGroupIndex)) return false;
  return commitRequestedPageGroup(nextGroupIndex);
}

function commitRequestedPageGroup(
  groupIndex,
  positionRequest = captureViewerPageGroupRequest(state.viewer, groupIndex)
) {
  const group = state.pageGroups[groupIndex];
  if (
    !group ||
    state.pageGroupIndex !== groupIndex ||
    !viewerPageGroupRequestMatches(
      positionRequest,
      captureViewerPageGroupRequest(state.viewer, groupIndex)
    )
  ) return false;
  const entry = group.anchor;
  const viewerDepth = (Number(history.state?.viewerDepth) || 0) + 1;
  const targetHash = entry.address
    ? mediaHash(entry.address)
    : imageHash(entry.path);
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
  updateViewerImage(performance.now(), { positionRequest }).catch(renderError);
  return true;
}

export function viewerImageUpdateContextExitReason({
  viewerMatches = true,
  sessionMatches = true,
  cacheEpochMatches = true,
  groupMatches = true,
} = {}) {
  if (!viewerMatches) {
    return ViewerImageUpdateExitReason.VIEWER_CHANGED_BEFORE_GROUP_LOAD;
  }
  if (!sessionMatches) {
    return ViewerImageUpdateExitReason.SESSION_CHANGED_BEFORE_GROUP_LOAD;
  }
  if (!cacheEpochMatches) {
    return ViewerImageUpdateExitReason.CACHE_EPOCH_CHANGED_BEFORE_GROUP_LOAD;
  }
  if (!groupMatches) {
    return ViewerImageUpdateExitReason.GROUP_CHANGED_BEFORE_GROUP_LOAD;
  }
  return null;
}

function supersedeViewerImageUpdate(reason, renderTrigger, stage) {
  enqueueTelemetry({
    type: "viewer_update",
    outcome: "not_applied",
    reason,
    stage,
    render_trigger: renderTrigger,
  });
  return { outcome: ViewerGroupLoadOutcome.SUPERSEDED, reason };
}

function viewerImageFailureForDisplay(result, renderTrigger) {
  if (
    result?.outcome === ViewerGroupLoadOutcome.FAILED &&
    (renderTrigger === "spread_refresh" || renderTrigger === "viewport_resize")
  ) {
    return { ...result, message: CONTAINER_SPREAD_REFRESH_FAILURE_MESSAGE };
  }
  return result;
}

async function updateViewerImage(
  interactionStartedAt = performance.now(),
  {
    adjustmentStateCurrent = false,
    renderTrigger = "page_request",
    positionRequest = null,
  } = {}
) {
  const group = currentPageGroup();
  const viewer = state.viewer;
  if (!group) {
    return supersedeViewerImageUpdate(
      ViewerImageUpdateExitReason.GROUP_MISSING_BEFORE_LOAD,
      renderTrigger,
      "preflight"
    );
  }
  if (!viewer) {
    return supersedeViewerImageUpdate(
      ViewerImageUpdateExitReason.VIEWER_MISSING_BEFORE_LOAD,
      renderTrigger,
      "preflight"
    );
  }
  const loadRequest = captureViewerPageGroupRequest(viewer, state.pageGroupIndex);
  const identity = group.entries.map(entryIdentity).join("\n");
  const remoteSessionIdSnapshot = state.remoteSessionId;
  const remoteSessionCacheEpochSnapshot = state.remoteSessionCacheEpoch;
  const generationSnapshot = viewerPageGroupGenerationSnapshot(
    state.remoteStateGeneration,
    group.entries.length
  );
  // 例外も 3 outcome の境界の内側で受ける。ここを抜けると呼び出し側の
  // .catch(renderError) まで飛び、位置を戻す判断が一度も行われない。
  let pages = [];
  let result;
  let stage = "image_info";
  let loadGroupReached = false;
  try {
    const infos = await Promise.all(group.entries.map(imageInfo));
    const contextExit = viewerImageUpdateContextExitReason({
      viewerMatches: state.viewer === viewer,
      sessionMatches: state.remoteSessionId === remoteSessionIdSnapshot,
      cacheEpochMatches:
        state.remoteSessionCacheEpoch === remoteSessionCacheEpochSnapshot,
      groupMatches:
        currentPageGroup()?.entries.map(entryIdentity).join("\n") === identity,
    });
    if (contextExit) {
      return supersedeViewerImageUpdate(
        contextExit,
        renderTrigger,
        "after_image_info"
      );
    }
    stage = "layout";
    const layout = viewerSpreadLayout({
      mode: state.fitMode,
      pages: infos,
      viewportWidth: viewer.stage.clientWidth || window.innerWidth,
      viewportHeight: viewer.stage.clientHeight || window.innerHeight,
      devicePixelRatio: window.devicePixelRatio || 1,
      gap: group.entries.length > 1 ? state.spreadPageGapPx : 0,
    });
    pages = group.entries.map((entry, pageIndex) => ({
      entry,
      info: infos[pageIndex],
      request: imageRequest(entry, infos[pageIndex], viewer.stage, {
        layout: layout.pages[pageIndex],
        remoteStateGeneration: generationSnapshot.pages[pageIndex],
        remoteSessionId: remoteSessionIdSnapshot,
        remoteSessionCacheEpoch: remoteSessionCacheEpochSnapshot,
      }),
    }));
    stage = "group_load";
    loadGroupReached = true;
    result = await viewer.loadGroup({
      pages,
      name: group.entries.map((entry) => entry.name).join(" / "),
      fitMode: state.fitMode,
      gap: layout.gap,
      index: state.pageGroupIndex,
      count: state.pageGroups.length,
      seekState: viewerSeekSnapshot(),
      pageNumbers: (state.seekPageGroups[state.pageGroupIndex] ?? [])
        .map((pageIndex) => pageIndex + 1),
      interactionStartedAt,
      renderTrigger,
    });
  } catch (error) {
    if (state.viewer !== viewer) {
      return supersedeViewerImageUpdate(
        ViewerImageUpdateExitReason.VIEWER_CHANGED_ON_ERROR,
        renderTrigger,
        stage
      );
    }
    const reason = loadGroupReached
      ? error?.name === "AbortError"
        ? ViewerImageUpdateExitReason.GROUP_LOAD_ABORTED
        : ViewerImageUpdateExitReason.GROUP_LOAD_THROWN
      : error?.name === "AbortError"
        ? ViewerImageUpdateExitReason.PRELOAD_ABORTED
        : ViewerImageUpdateExitReason.PRELOAD_FAILED;
    recordClientError("viewer_update_error", error, {
      reason,
      stage,
      render_trigger: renderTrigger,
    });
    result = {
      outcome: ViewerGroupLoadOutcome.FAILED,
      reason,
      message: "ページを表示できませんでした。",
    };
  }
  const completion = viewerGroupLoadCompletionPlan(result, {
    loadRequest,
    positionRequest,
    currentRequest: captureViewerPageGroupRequest(
      state.viewer,
      state.pageGroupIndex
    ),
  });
  if (completion.action === ViewerGroupLoadCompletionAction.IGNORE) {
    return supersedeViewerImageUpdate(
      result?.outcome === ViewerGroupLoadOutcome.SUPERSEDED
        ? ViewerImageUpdateExitReason.GROUP_LOAD_SUPERSEDED
        : ViewerImageUpdateExitReason.LOAD_REQUEST_CHANGED_AFTER_GROUP_LOAD,
      renderTrigger,
      "completion"
    );
  }
  if (completion.action === ViewerGroupLoadCompletionAction.ROLLBACK) {
    discardRequestedPageGroup(
      viewer,
      positionRequest.groupIndex,
      positionRequest
    );
    const displayedFailure = viewerImageFailureForDisplay(
      { ...result, message: completion.message },
      renderTrigger
    );
    if (state.viewer === viewer) {
      viewer.showGroupLoadFailure(displayedFailure.message);
    }
    return displayedFailure;
  }
  if (completion.action === ViewerGroupLoadCompletionAction.REPORT_FAILURE) {
    const displayedFailure = viewerImageFailureForDisplay(
      { ...result, message: completion.message },
      renderTrigger
    );
    if (state.viewer === viewer) {
      viewer.showGroupLoadFailure(displayedFailure.message);
    }
    return displayedFailure;
  }
  if (state.viewer !== viewer) {
    return supersedeViewerImageUpdate(
      ViewerImageUpdateExitReason.VIEWER_CHANGED_AFTER_GROUP_LOAD,
      renderTrigger,
      "post_display"
    );
  }
  document.title = `${group.anchor.name} — mIV Remote`;
  state.remoteAiController?.displayGroup(
    pages.map(({ entry, request }) => ({
      address: entryAddress(entry),
      target_px: request.width,
      render_context: request.renderContext,
      name: entry.name,
    }))
  ).catch((error) => state.remoteAiController?.showRequestError(error));
  const refreshPlan = viewerPostDisplayRefreshPlan({ adjustmentStateCurrent });
  if (refreshPlan.adjustment) state.commandMenu?.refreshAdjustment();
  if (refreshPlan.bookmarks) state.commandMenu?.refreshBookmarks();
  state.commandMenu?.refreshViewTrim();
  observeReadingProgress();
  if (group.entries.every((entry) => entry.address)) {
    schedulePagePrefetch(viewer, pages).catch(() => {});
    return VIEWER_GROUP_LOAD_APPLIED;
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
  return VIEWER_GROUP_LOAD_APPLIED;
}

async function schedulePagePrefetch(viewer, visiblePages = []) {
  const group = currentPageGroup();
  const currentIdentity = group?.entries.map(entryIdentity).join("\n") ?? "";
  hudState.pagePrefetch = null;
  updateHud();
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
  const hudPlan = pagePrefetchHudPlan({
    visibleIndexes,
    itemCount: state.images.length,
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
  const requestsByIndex = new Map(
    indexes.map((index, requestIndex) => [index, requests[requestIndex]])
  );
  hudState.pagePrefetch = {
    behindKeys: hudPlan.behindIndexes
      .map((index) => requestsByIndex.get(index)?.cacheKey)
      .filter(Boolean),
    aheadKeys: hudPlan.aheadIndexes
      .map((index) => requestsByIndex.get(index)?.cacheKey)
      .filter(Boolean),
  };
  pageResourceCache.schedule(
    requests.filter(Boolean),
    visiblePages.map(({ request }) => request.cacheKey).filter(Boolean)
  );
  updateHud();
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
    remoteStateGeneration = state.remoteStateGeneration,
    remoteSessionId = state.remoteSessionId,
    remoteSessionCacheEpoch = state.remoteSessionCacheEpoch,
  } = {}
) {
  const dpr = window.devicePixelRatio || 1;
  if (entry.address) {
    const renderContext = pageRenderContextForEntry(entry);
    const resolvedLayout = layout ?? viewerImageLayout({
        mode: state.fitMode,
        sourceWidth: info.width,
        sourceHeight: info.height,
        viewportWidth: stage.clientWidth || window.innerWidth,
        viewportHeight: stage.clientHeight || window.innerHeight,
        devicePixelRatio: dpr,
        maxRequestWidth: 8192,
      });
    const qualityPreset = imageQualityPreset(state.localSettings.imageQuality);
    const targetPx = targetPxOverride ?? qualityPreset.maxLongSide;
    const infoCacheKey = mediaImageInfoKey(entry.address);
    return {
      url: apiUrl(
        "/api/page",
        addressQueryParams(entry.address, {
          w: targetPx,
          generation: remoteStateGeneration,
          epoch: remoteSessionCacheEpoch,
          ...(prefetch ? { prefetch: 1 } : {}),
          rev: previewRevision ?? state.pageRenderRevision,
          ...(renderContext
            ? { render_context: JSON.stringify(renderContext) }
            : {}),
          ...(adjustmentPreview
            ? { adjustment_preview: JSON.stringify(adjustmentPreview) }
            : {}),
        })
      ),
      cacheKey: adjustmentPreview
        ? null
        : `${infoCacheKey}\n${targetPx}\n${state.pageRenderRevision}\n${remoteStateGeneration}\n${remoteSessionId}\n${JSON.stringify(renderContext)}`,
      remoteStateGeneration,
      remoteSessionId,
      address: entry.address,
      width: targetPx,
      cssWidth: resolvedLayout.cssWidth,
      dpr,
      layout: resolvedLayout,
      fitMode: state.fitMode,
      dynamicInfo: true,
      infoCacheKey,
      containerInfoKey: mediaContainerInfoKey(entry.address),
      renderContext,
      prefetch,
      imageQualityPresetId:
        targetPxOverride == null && !adjustmentPreview ? qualityPreset.id : null,
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
      path: entry.path,
      w: resolvedLayout.requestWidth,
      epoch: state.remoteSessionCacheEpoch,
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
  const key = `${path}\n${entry?.mtime ?? ""}\n${entry?.size ?? ""}`;
  if (!state.imageInfoCache.has(key)) {
    const pending = apiJson("/api/image-info", {
      path,
      epoch: state.remoteSessionCacheEpoch,
    }).catch(
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
  return address.path;
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
    thumbnailRequestQueryForEntry(entry, {
      w: targetPx,
      epoch: state.remoteSessionCacheEpoch,
    })
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
    labelHeight,
    viewportState = {}
  ) {
    this.scroller = scroller;
    this.space = space;
    this.windowElement = windowElement;
    this.items = items;
    this.renderCell = renderCell;
    this.onInitialItems = onInitialItems;
    this.aspectHeightRatio = aspectHeightRatio;
    this.requestedLabelHeight = labelHeight;
    // These values describe this rendered list. Keep them with the DOM because a route
    // load may replace the global grid state before cleanupScreen destroys this instance.
    this.context = String(viewportState.context ?? "");
    this.itemIdentities = Array.isArray(viewportState.itemIdentities)
      ? viewportState.itemIdentities.slice()
      : [];
    this.viewportAnchor = new GridViewportAnchor(this.itemIdentities);
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

  selectReturnAnchor(index) {
    this.viewportAnchor.select(index);
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

function remoteAddressLooksValid(address) {
  return Boolean(
    address &&
    typeof address.path === "string" &&
    typeof address.subresource?.kind === "string"
  );
}

export function normalizeRemoteBookBookmarkList(value) {
  const rows = Array.isArray(value?.rows) ? value.rows : [];
  return {
    supported: Boolean(value?.supported),
    rows: rows.flatMap((row) => {
      const id = Number(row?.id);
      if (!Number.isSafeInteger(id)) return [];
      const hint = Number(row?.page_index_hint);
      const targetIndex = Number(row?.target?.item_index);
      const target =
        remoteAddressLooksValid(row?.target?.address) &&
        remoteAddressLooksValid(row?.target?.context_address) &&
        Number.isSafeInteger(targetIndex) &&
        targetIndex >= 0
          ? {
              address: row.target.address,
              contextAddress: row.target.context_address,
              itemIndex: targetIndex,
            }
          : null;
      return [{
        id,
        title: typeof row?.title === "string" && row.title.length ? row.title : null,
        pageIndexHint: Number.isSafeInteger(hint) && hint >= 0 ? hint : 0,
        pageLabel: typeof row?.page_label === "string" ? row.page_label : "",
        target,
      }];
    }),
  };
}

export function remoteBookBookmarkDisplayPage(row) {
  const index = row?.target?.itemIndex ?? row?.pageIndexHint ?? 0;
  return Math.max(0, Number(index) || 0) + 1;
}

export function remoteBookBookmarkTargetEntryIndex(entries, address) {
  if (!remoteAddressLooksValid(address) || !Array.isArray(entries)) return -1;
  const identity = addressIdentity(address);
  return entries.findIndex(
    (entry) => remoteAddressLooksValid(entryAddress(entry)) &&
      addressIdentity(entryAddress(entry)) === identity
  );
}

function remoteBookContainerIdentity(address) {
  return address?.path ?? "";
}

export function viewerMenuDefinitions({ hasContainer, barsVisible }) {
  const back = [MenuPageAction.BACK, "操作メニューへ戻る", "戻る"];
  const mainActions = [
    [CommandName.TOGGLE_BOOKMARK, "ブックマークを読み込み中…", "現在のページ"],
    [MenuPageAction.RATING, "レーティング", "★を選択", { menuPage: "rating" }],
    [MenuPageAction.DISPLAY, "表示", "フィット / 原寸", { menuPage: "display" }],
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
      title: "表示",
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

function normalizedTrimMargins(value, keys) {
  if (!value || typeof value !== "object") return null;
  const result = {};
  for (const key of keys) {
    const number = Number(value[key]);
    if (!Number.isFinite(number)) return null;
    result[key] = number;
  }
  return result;
}

export function normalizeRemoteViewTrimState(value) {
  if (!value || typeof value !== "object") return null;
  if (!["none", "auto", "book"].includes(value.apply_mode)) return null;
  const book = value.book_settings;
  if (!book || typeof book !== "object") return null;
  const single = normalizedTrimMargins(book.single, ["left", "top", "right", "bottom"]);
  const linked = normalizedTrimMargins(book.spread_linked, ["top", "bottom", "inner", "outer"]);
  const left = normalizedTrimMargins(book.spread_left, ["left", "top", "right", "bottom"]);
  const right = normalizedTrimMargins(book.spread_right, ["left", "top", "right", "bottom"]);
  if (!single || !linked || !left || !right) return null;
  return {
    apply_mode: value.apply_mode,
    book_settings: {
      enabled: Boolean(book.enabled),
      spread_separate: Boolean(book.spread_separate),
      single,
      spread_linked: linked,
      spread_left: left,
      spread_right: right,
    },
  };
}

export function viewTrimSpreadControlKeys(bookSettings, isSpread) {
  if (!isSpread) return ["single"];
  return bookSettings?.spread_separate
    ? ["spread_left", "spread_right"]
    : ["spread_linked"];
}

function cloneViewTrimState(value) {
  return JSON.parse(JSON.stringify(value));
}

export function setViewTrimSpreadSeparate(stateValue, separate) {
  const next = cloneViewTrimState(stateValue);
  next.book_settings.spread_separate = Boolean(separate);
  return next;
}

export class ViewerViewTrimPanel {
  constructor() {
    this.root = element("div", "viewer-view-trim-panel");
    this.serverState = null;
    this.targetIdentity = "";
    this.refreshSequence = 0;
    this.commitSequence = 0;
    this.writeTail = Promise.resolve();
    this.build();
  }

  build() {
    const modeRow = element("label", "view-trim-mode-row");
    modeRow.append(textElement("span", "適用モード"));
    this.modeSelect = document.createElement("select");
    for (const [value, label] of [
      ["none", "トリムなし"],
      ["auto", "自動余白カット（本全体）"],
      ["book", "手動設定（本全体）"],
    ]) {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      this.modeSelect.append(option);
    }
    this.modeSelect.addEventListener("change", () => {
      if (!this.serverState) return;
      this.serverState.apply_mode = this.modeSelect.value;
      if (this.serverState.apply_mode === "book") {
        this.serverState.book_settings.enabled = true;
      }
      this.renderState();
      this.commit().catch((error) => this.showError(error));
    });
    modeRow.append(this.modeSelect);

    this.enabledRow = element("label", "view-trim-check-row");
    this.enabledInput = document.createElement("input");
    this.enabledInput.type = "checkbox";
    this.enabledInput.addEventListener("change", () => {
      if (!this.serverState) return;
      this.serverState.book_settings.enabled = this.enabledInput.checked;
      this.commit().catch((error) => this.showError(error));
    });
    this.enabledRow.append(this.enabledInput, textElement("span", "手動トリムを有効にする"));

    this.separateRow = element("label", "view-trim-check-row");
    this.separateInput = document.createElement("input");
    this.separateInput.type = "checkbox";
    this.separateInput.addEventListener("change", () => {
      if (!this.serverState) return;
      this.serverState = setViewTrimSpreadSeparate(
        this.serverState,
        this.separateInput.checked
      );
      this.renderState();
      this.commit().catch((error) => this.showError(error));
    });
    this.separateRow.append(
      this.separateInput,
      textElement("span", "見開きの左右を別々に調整")
    );

    this.controls = element("div", "view-trim-controls");
    this.status = textElement("p", "現在値を読み込んでいます…", "view-trim-status");
    this.status.setAttribute("role", "status");
    this.root.append(modeRow, this.enabledRow, this.separateRow, this.controls, this.status);
  }

  currentTarget() {
    const target = currentRemotePageTarget();
    if (!target) return null;
    return {
      ...target,
      identity: `${addressIdentity(target.contextAddress)}\n${addressIdentity(target.address)}`,
    };
  }

  async refresh() {
    const target = this.currentTarget();
    if (!target) {
      this.setDisabled(true);
      this.status.textContent = "このページは表示トリムの対象ではありません。";
      return;
    }
    this.targetIdentity = target.identity;
    const sequence = ++this.refreshSequence;
    this.setDisabled(true);
    this.status.classList.remove("is-error");
    this.status.textContent = "現在値を読み込み中…";
    const response = await apiAddressPostJson("/api/write", {
      kind: "get_view_trim_state",
      address: target.address,
      context_address: target.contextAddress,
    });
    if (sequence !== this.refreshSequence || this.currentTarget()?.identity !== target.identity) {
      return;
    }
    const next = normalizeRemoteViewTrimState(response.view_trim_state);
    if (!next) throw new Error("表示トリムの現在値を取得できませんでした。");
    this.serverState = next;
    applyRemoteStateGeneration(response.remote_state_generation, { reloadViewer: true });
    this.renderState();
    this.setDisabled(false);
    this.status.textContent = "スライダーを離すと保存します。";
  }

  renderState() {
    const value = this.serverState;
    if (!value) return;
    const isSpread = (currentPageGroup()?.entries.length ?? 0) === 2;
    const manual = value.apply_mode === "book";
    this.modeSelect.value = value.apply_mode;
    this.enabledInput.checked = value.book_settings.enabled;
    this.enabledRow.hidden = !manual;
    this.separateInput.checked = value.book_settings.spread_separate;
    this.separateRow.hidden = !manual || !isSpread;
    const groups = viewTrimSpreadControlKeys(value.book_settings, isSpread);
    const nodes = [];
    for (const group of groups) {
      const keys = group === "spread_linked"
        ? ["top", "bottom", "inner", "outer"]
        : ["left", "top", "right", "bottom"];
      const labels = {
        single: "単ページ",
        spread_linked: "見開き（連動）",
        spread_left: "左ページ",
        spread_right: "右ページ",
      };
      const section = element("section", "view-trim-control-group");
      section.append(textElement("h3", labels[group]));
      for (const key of keys) section.append(this.marginControl(group, key));
      nodes.push(section);
    }
    this.controls.replaceChildren(...nodes);
    this.controls.hidden = !manual;
    this.setDisabled(false);
  }

  marginControl(group, key) {
    const labels = { left: "左", top: "上", right: "右", bottom: "下", inner: "内側", outer: "外側" };
    const row = element("label", "view-trim-slider-row");
    row.append(textElement("span", labels[key]));
    const input = document.createElement("input");
    input.type = "range";
    input.min = "0";
    input.max = "20";
    input.step = "0.1";
    input.value = String(this.serverState.book_settings[group][key] * 100);
    input.setAttribute("aria-label", `${labels[key]}のトリム率`);
    const output = document.createElement("output");
    let pointerDrag = null;
    let dirty = false;
    const applyPercent = (rawPercent) => {
      const percent = rangeValueFromNormalized({
        normalized: rangeValueToNormalized({
          value: rawPercent,
          min: Number(input.min),
          max: Number(input.max),
        }),
        min: Number(input.min),
        max: Number(input.max),
        step: Number(input.step),
      });
      const previousPercent = this.serverState.book_settings[group][key] * 100;
      const changed = percent !== previousPercent || !this.serverState.book_settings.enabled;
      input.value = String(percent);
      output.value = `${percent.toFixed(1)}%`;
      this.serverState.book_settings[group][key] = percent / 100;
      this.serverState.book_settings.enabled = true;
      this.enabledInput.checked = true;
      dirty = changed || dirty;
      return changed;
    };
    output.value = `${Number(input.value).toFixed(1)}%`;
    input.addEventListener("input", () => {
      if (pointerDrag) {
        input.value = String(pointerDrag.targetPercent);
        return;
      }
      applyPercent(Number(input.value));
    });
    const finish = (event) => {
      event.stopPropagation();
      if (!dirty) return;
      dirty = false;
      this.commit().catch((error) => this.showError(error));
    };
    input.addEventListener("change", finish);
    input.addEventListener("pointerdown", (event) => {
      event.stopPropagation();
      if (input.disabled || event.isPrimary === false) return;
      if (typeof event.button === "number" && event.button !== 0) return;
      if (event.pointerType !== "touch" && event.cancelable) event.preventDefault();
      const trackRect = input.getBoundingClientRect();
      const startPercent = Number(input.value);
      pointerDrag = {
        pointerId: event.pointerId,
        startClientX: event.clientX,
        startClientY: event.clientY,
        startPercent,
        targetPercent: startPercent,
        startEnabled: this.serverState.book_settings.enabled,
        startDirty: dirty,
        trackLeft: trackRect.left,
        trackWidth: trackRect.width,
        maxDistancePx: 0,
      };
      input.focus({ preventScroll: true });
      try {
        input.setPointerCapture(event.pointerId);
      } catch (_error) {
        // Pointer capture can fail when the browser has already cancelled the pointer.
      }
    });
    input.addEventListener("pointermove", (event) => {
      const drag = pointerDrag;
      if (!drag || drag.pointerId !== event.pointerId) return;
      event.stopPropagation();
      if (event.pointerType !== "touch" && event.cancelable) event.preventDefault();
      const gesture = seekRangePointerGestureDecision({
        startClientX: drag.startClientX,
        startClientY: drag.startClientY,
        currentClientX: event.clientX,
        currentClientY: event.clientY,
        maxDistancePx: drag.maxDistancePx,
      });
      drag.maxDistancePx = gesture.maxDistancePx;
      if (gesture.kind !== "drag") return;
      drag.targetPercent = relativeRangeDragValue({
        startValue: drag.startPercent,
        startClientX: drag.startClientX,
        currentClientX: event.clientX,
        trackWidth: drag.trackWidth,
        min: Number(input.min),
        max: Number(input.max),
        step: Number(input.step),
      });
      applyPercent(drag.targetPercent);
    });
    const releasePointer = (event) => {
      const drag = pointerDrag;
      if (!drag) return null;
      const pointerId = typeof event.pointerId === "number" ? event.pointerId : drag.pointerId;
      try {
        if (input.hasPointerCapture(pointerId)) input.releasePointerCapture(pointerId);
      } catch (_error) {
        // Direct manipulation may release capture before pointercancel.
      }
      pointerDrag = null;
      return drag;
    };
    const finishPointer = (event) => {
      if (!pointerDrag || pointerDrag.pointerId !== event.pointerId) return;
      event.stopPropagation();
      if (event.pointerType !== "touch" && event.cancelable) event.preventDefault();
      const drag = pointerDrag;
      const gesture = seekRangePointerGestureDecision({
        startClientX: drag.startClientX,
        startClientY: drag.startClientY,
        currentClientX: event.clientX,
        currentClientY: event.clientY,
        maxDistancePx: drag.maxDistancePx,
      });
      releasePointer(event);
      const targetPercent = gesture.kind === "tap"
        ? seekRangeAbsoluteValue({
          clientX: drag.startClientX,
          trackLeft: drag.trackLeft,
          trackWidth: drag.trackWidth,
          min: Number(input.min),
          max: Number(input.max),
          step: Number(input.step),
        })
        : relativeRangeDragValue({
          startValue: drag.startPercent,
          startClientX: drag.startClientX,
          currentClientX: event.clientX,
          trackWidth: drag.trackWidth,
          min: Number(input.min),
          max: Number(input.max),
          step: Number(input.step),
        });
      applyPercent(targetPercent);
      finish(event);
    };
    const cancelPointer = (event) => {
      if (
        pointerDrag &&
        typeof event.pointerId === "number" &&
        pointerDrag.pointerId !== event.pointerId
      ) {
        event.stopPropagation();
        return;
      }
      const drag = releasePointer(event);
      event.stopPropagation();
      if (!drag) return;
      input.value = String(drag.startPercent);
      output.value = `${drag.startPercent.toFixed(1)}%`;
      this.serverState.book_settings[group][key] = drag.startPercent / 100;
      this.serverState.book_settings.enabled = drag.startEnabled;
      this.enabledInput.checked = drag.startEnabled;
      dirty = drag.startDirty;
    };
    input.addEventListener("pointerup", finishPointer);
    input.addEventListener("pointercancel", cancelPointer);
    input.addEventListener("touchcancel", cancelPointer);
    input.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
    });
    row.append(input, output);
    return row;
  }

  commit() {
    const target = this.currentTarget();
    if (!target || !this.serverState) return Promise.resolve();
    const requestState = cloneViewTrimState(this.serverState);
    const commitId = ++this.commitSequence;
    this.status.classList.remove("is-error");
    this.status.textContent = "保存中…";
    this.writeTail = this.writeTail.catch(() => {}).then(async () => {
      const response = await apiAddressPostJson("/api/write", {
        kind: "set_view_trim",
        address: target.address,
        context_address: target.contextAddress,
        state: requestState,
      });
      if (commitId !== this.commitSequence || this.currentTarget()?.identity !== target.identity) {
        return;
      }
      const next = normalizeRemoteViewTrimState(response.view_trim_state);
      if (!next) throw new Error("表示トリムの保存結果を取得できませんでした。");
      this.serverState = next;
      this.renderState();
      applyRemoteStateGeneration(response.remote_state_generation, { reloadViewer: true });
      this.status.textContent = "保存しました。";
    }).catch((error) => {
      if (commitId === this.commitSequence) this.showError(error);
      throw error;
    });
    return this.writeTail;
  }

  setDisabled(disabled) {
    for (const control of this.root.querySelectorAll("input, select")) {
      control.disabled = disabled;
    }
  }

  showError(error) {
    this.setDisabled(false);
    this.status.classList.add("is-error");
    this.status.textContent = error instanceof Error
      ? error.message
      : "表示トリム設定を保存できませんでした。";
  }

  destroy() {
    this.refreshSequence += 1;
    this.commitSequence += 1;
  }
}

export class ViewerAdjustmentPanel {
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
    autoRow.append(textElement("span", "補正モード"));
    this.autoSelect = document.createElement("select");
    for (const [value, label] of [
      ["", "手動"],
      ["auto", "自動補正"],
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
        if (event.pointerType !== "touch" && event.cancelable) event.preventDefault();
        const trackRect = input.getBoundingClientRect();
        pointerDrag = {
          pointerId: event.pointerId,
          startClientX: event.clientX,
          startClientY: event.clientY,
          startValue: Number(this.values[key]),
          startDirty: this.dirty,
          trackLeft: trackRect.left,
          trackWidth: trackRect.width,
          maxDistancePx: 0,
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
        if (event.pointerType !== "touch" && event.cancelable) event.preventDefault();
        const gesture = seekRangePointerGestureDecision({
          startClientX: pointerDrag.startClientX,
          startClientY: pointerDrag.startClientY,
          currentClientX: event.clientX,
          currentClientY: event.clientY,
          maxDistancePx: pointerDrag.maxDistancePx,
        });
        pointerDrag.maxDistancePx = gesture.maxDistancePx;
        if (gesture.kind !== "drag") return;
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
      const releasePointer = (event) => {
        if (!pointerDrag) return null;
        const activePointer = pointerDrag;
        const pointerId = typeof event.pointerId === "number"
          ? event.pointerId
          : activePointer.pointerId;
        try {
          if (input.hasPointerCapture(pointerId)) {
            input.releasePointerCapture(pointerId);
          }
        } catch (_error) {
          // Direct manipulation may release capture before pointercancel.
        }
        pointerDrag = null;
        return activePointer;
      };
      const finishPointer = (event) => {
        if (pointerDrag && pointerDrag.pointerId !== event.pointerId) {
          event.stopPropagation();
          return;
        }
        if (pointerDrag) {
          if (event.pointerType !== "touch" && event.cancelable) event.preventDefault();
          const activePointer = pointerDrag;
          const gesture = seekRangePointerGestureDecision({
            startClientX: activePointer.startClientX,
            startClientY: activePointer.startClientY,
            currentClientX: event.clientX,
            currentClientY: event.clientY,
            maxDistancePx: activePointer.maxDistancePx,
          });
          releasePointer(event);
          if (gesture.kind === "tap") {
            applyNormalizedPosition(seekRangeAbsoluteValue({
              clientX: activePointer.startClientX,
              trackLeft: activePointer.trackLeft,
              trackWidth: activePointer.trackWidth,
              min: 0,
              max: 1,
              step: 0,
            }));
          } else {
            applyValue(relativeRangeDragValue({
              startValue: activePointer.startValue,
              startClientX: activePointer.startClientX,
              currentClientX: event.clientX,
              trackWidth: activePointer.trackWidth,
              min,
              max,
              step,
              logarithmic,
            }));
          }
        }
        finish(event);
      };
      const cancelPointer = (event) => {
        if (
          pointerDrag &&
          typeof event.pointerId === "number" &&
          pointerDrag.pointerId !== event.pointerId
        ) {
          event.stopPropagation();
          return;
        }
        const cancelledDrag = releasePointer(event);
        event.stopPropagation();
        if (!cancelledDrag) return;
        const changed = this.values[key] !== cancelledDrag.startValue;
        this.values[key] = cancelledDrag.startValue;
        this.dirty = cancelledDrag.startDirty;
        this.syncControl(key);
        if (changed) this.queuePreview();
      };
      input.addEventListener("pointerup", finishPointer);
      input.addEventListener("pointercancel", cancelPointer);
      input.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopPropagation();
      });
      for (const eventName of ["touchend", "change"]) {
        input.addEventListener(eventName, finish);
      }
      input.addEventListener("touchcancel", cancelPointer);
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

    this.colorTonePanel = element(
      "section",
      "adjustment-tab-panel adjustment-color-tone"
    );
    this.colorTonePanel.append(autoRow, this.sliderList);

    this.colorizeSection = element("section", "adjustment-colorize");
    this.colorizeSection.classList.add("adjustment-tab-panel");

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
    this.aiSection.classList.add("adjustment-tab-panel");
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

    this.selectedTab = state.localSettings.adjustmentTab;
    this.tabList = element("nav", "adjustment-tabs");
    this.tabList.setAttribute("role", "tablist");
    this.tabList.setAttribute("aria-label", "画像補正のタブ");
    this.tabButtons = new Map();
    this.tabPanels = new Map([
      ["color_tone", this.colorTonePanel],
      ["ai", this.aiSection],
      ["colorize", this.colorizeSection],
    ]);
    const tabGroupId = `remote-adjustment-tab-${Math.random().toString(36).slice(2)}`;
    for (const tab of ADJUSTMENT_PANEL_TABS) {
      const button = textElement("button", tab.label, "adjustment-tab");
      const panel = this.tabPanels.get(tab.id);
      button.type = "button";
      button.id = `${tabGroupId}-${tab.id}`;
      panel.id = `${button.id}-panel`;
      button.setAttribute("role", "tab");
      button.setAttribute("aria-controls", panel.id);
      panel.setAttribute("role", "tabpanel");
      panel.setAttribute("aria-labelledby", button.id);
      button.addEventListener("click", (event) => {
        event.stopPropagation();
        this.selectTab(tab.id);
      });
      this.tabList.append(button);
      this.tabButtons.set(tab.id, button);
    }

    this.resetButton = textElement("button", "色調をリセット", "adjustment-reset");
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
      this.tabList,
      this.colorTonePanel,
      this.aiSection,
      this.colorizeSection,
      this.resetButton,
      this.status
    );
    this.syncTabVisibility();
    this.syncControls();
    this.setDisabled(false);
  }

  selectTab(tabId) {
    const selected = ADJUSTMENT_PANEL_TABS.find((tab) => tab.id === tabId) ??
      ADJUSTMENT_PANEL_TABS[0];
    if (this.selectedTab === selected.id) return;
    const saved = saveLocalSettings({
      ...state.localSettings,
      adjustmentTab: selected.id,
    });
    state.localSettings = saved.settings;
    state.localSettingsStorageAvailable = saved.saved;
    this.selectedTab = state.localSettings.adjustmentTab;
    this.syncTabVisibility();
  }

  syncTabVisibility() {
    for (const tab of ADJUSTMENT_PANEL_TABS) {
      const selected = tab.id === this.selectedTab;
      const button = this.tabButtons.get(tab.id);
      const panel = this.tabPanels.get(tab.id);
      button.classList.toggle("is-selected", selected);
      button.setAttribute("aria-selected", selected ? "true" : "false");
      button.tabIndex = selected ? 0 : -1;
      panel.hidden = !selected;
    }
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
      if (event.pointerType !== "touch" && event.cancelable) event.preventDefault();
      const trackRect = input.getBoundingClientRect();
      pointerDrag = {
        pointerId: event.pointerId,
        startClientX: event.clientX,
        startClientY: event.clientY,
        startValue: Number(this.values.colorize[key]),
        startDirty: this.dirty,
        trackLeft: trackRect.left,
        trackWidth: trackRect.width,
        maxDistancePx: 0,
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
      if (event.pointerType !== "touch" && event.cancelable) event.preventDefault();
      const gesture = seekRangePointerGestureDecision({
        startClientX: pointerDrag.startClientX,
        startClientY: pointerDrag.startClientY,
        currentClientX: event.clientX,
        currentClientY: event.clientY,
        maxDistancePx: pointerDrag.maxDistancePx,
      });
      pointerDrag.maxDistancePx = gesture.maxDistancePx;
      if (gesture.kind !== "drag") return;
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
    const releasePointer = (event) => {
      if (!pointerDrag) return null;
      const activePointer = pointerDrag;
      const pointerId = typeof event.pointerId === "number"
        ? event.pointerId
        : activePointer.pointerId;
      try {
        if (input.hasPointerCapture(pointerId)) {
          input.releasePointerCapture(pointerId);
        }
      } catch (_error) {
        // Direct manipulation may release capture before pointercancel.
      }
      pointerDrag = null;
      return activePointer;
    };
    const finishPointer = (event) => {
      if (pointerDrag && pointerDrag.pointerId !== event.pointerId) {
        event.stopPropagation();
        return;
      }
      if (pointerDrag) {
        if (event.pointerType !== "touch" && event.cancelable) event.preventDefault();
        const activePointer = pointerDrag;
        const gesture = seekRangePointerGestureDecision({
          startClientX: activePointer.startClientX,
          startClientY: activePointer.startClientY,
          currentClientX: event.clientX,
          currentClientY: event.clientY,
          maxDistancePx: activePointer.maxDistancePx,
        });
        releasePointer(event);
        if (gesture.kind === "tap") {
          applyNormalizedPosition(seekRangeAbsoluteValue({
            clientX: activePointer.startClientX,
            trackLeft: activePointer.trackLeft,
            trackWidth: activePointer.trackWidth,
            min: 0,
            max: 1,
            step: 0,
          }));
        } else {
          applyValue(relativeRangeDragValue({
            startValue: activePointer.startValue,
            startClientX: activePointer.startClientX,
            currentClientX: event.clientX,
            trackWidth: activePointer.trackWidth,
            min,
            max,
            step,
            logarithmic: false,
          }));
        }
      }
      finish(event);
    };
    const cancelPointer = (event) => {
      if (
        pointerDrag &&
        typeof event.pointerId === "number" &&
        pointerDrag.pointerId !== event.pointerId
      ) {
        event.stopPropagation();
        return;
      }
      const cancelledDrag = releasePointer(event);
      event.stopPropagation();
      if (!cancelledDrag) return;
      const changed = this.values.colorize[key] !== cancelledDrag.startValue;
      this.values.colorize[key] = cancelledDrag.startValue;
      this.dirty = cancelledDrag.startDirty;
      this.syncColorizeControl(key);
      if (changed) this.queuePreview();
    };
    input.addEventListener("pointerup", finishPointer);
    input.addEventListener("pointercancel", cancelPointer);
    input.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
    });
    for (const eventName of ["touchend", "change"]) {
      input.addEventListener(eventName, finish);
    }
    input.addEventListener("touchcancel", cancelPointer);

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
    if (!entry || !address?.path) return null;
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
    const response = await apiAddressPostJson("/api/write", {
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
    requirePageResponseIdentity(request.address, response);
    requirePageResponseGeneration(request, response);
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
      const response = await apiAddressPostJson("/api/write", request);
      if (commitId !== this.commitSequence || this.currentTarget()?.identity !== target.identity) {
        return;
      }
      if (!response.adjustment_state) {
        throw new Error("画像補正の保存結果を取得できませんでした。");
      }
      this.applyServerState(response.adjustment_state);
      state.pageRenderRevision += 1;
      pageResourceCache.clear();
      await updateViewerImage(performance.now(), {
        adjustmentStateCurrent: true,
        renderTrigger: "adjustment_commit",
      });
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

class ViewerBookmarkPanel {
  constructor() {
    this.root = element("div", "viewer-bookmark-panel");
    this.content = element("div", "viewer-bookmark-content");
    this.status = textElement("p", "", "viewer-bookmark-status");
    this.status.setAttribute("aria-live", "polite");
    this.root.append(this.content, this.status);
    this.list = null;
    this.contextIdentity = "";
    this.loading = false;
    this.busy = false;
    this.editingId = null;
    this.refreshSequence = 0;
    this.refreshController = null;
    this.render();
  }

  currentTarget() {
    return currentRemotePageTarget();
  }

  async refresh() {
    const target = this.currentTarget();
    const sequence = ++this.refreshSequence;
    this.refreshController?.abort();
    const controller = new AbortController();
    this.refreshController = controller;
    this.loading = true;
    this.clearStatus();
    this.render();
    if (!target) {
      this.list = { supported: false, rows: [] };
      this.contextIdentity = "";
      this.loading = false;
      this.render();
      return;
    }
    const contextIdentity = remoteBookContainerIdentity(target.contextAddress);
    try {
      const response = await apiAddressPostJson("/api/write", {
        kind: "list_book_bookmarks",
        address: target.address,
        context_address: target.contextAddress,
        page_index: target.pageIndex,
        bookmark_supported: false,
      }, controller.signal);
      if (sequence !== this.refreshSequence || controller.signal.aborted) return;
      if (remoteBookContainerIdentity(this.currentTarget()?.contextAddress) !== contextIdentity) {
        this.loading = false;
        this.refresh().catch(() => {});
        return;
      }
      if (!response.book_bookmarks) {
        throw new Error("ブックマーク一覧を取得できませんでした。");
      }
      this.list = normalizeRemoteBookBookmarkList(response.book_bookmarks);
      this.contextIdentity = contextIdentity;
      this.loading = false;
      this.render();
    } catch (error) {
      if (controller.signal.aborted || sequence !== this.refreshSequence) return;
      this.loading = false;
      this.showError(error);
      this.render();
      throw error;
    }
  }

  updateCurrentPage() {
    const contextIdentity = remoteBookContainerIdentity(this.currentTarget()?.contextAddress);
    if (this.contextIdentity && this.contextIdentity !== contextIdentity && !this.loading) {
      this.refresh().catch(() => {});
      return;
    }
    this.render();
  }

  render() {
    const list = this.list;
    if (this.loading && !list) {
      this.content.replaceChildren(textElement("p", "読み込み中…", "viewer-bookmark-empty"));
      return;
    }
    if (!list?.supported) {
      const message = element("div", "viewer-bookmark-unsupported");
      message.append(
        textElement("p", "この画像は本のブックマーク対象ではありません。"),
        textElement(
          "p",
          "製本、画像のみフォルダ本、ZIP・PDF・対応アーカイブで利用できます。"
        )
      );
      this.content.replaceChildren(message);
      return;
    }

    const header = element("div", "viewer-bookmark-header");
    const add = textElement("button", "追加", "viewer-bookmark-add");
    add.type = "button";
    add.disabled = this.busy;
    add.setAttribute("aria-label", "現在ページをブックマークに追加");
    add.addEventListener("click", () => this.addCurrent());
    header.append(
      textElement("strong", "この本のブックマーク"),
      textElement("span", `${list.rows.length} 件`, "viewer-bookmark-count"),
      add
    );
    const rows = element("div", "viewer-bookmark-rows");
    if (!list.rows.length) {
      rows.append(
        textElement("p", "ブックマークはまだありません。", "viewer-bookmark-empty"),
        textElement(
          "p",
          "上の追加ボタンで現在ページを追加できます。",
          "viewer-bookmark-hint"
        )
      );
    } else {
      for (const row of list.rows) rows.append(this.buildRow(row));
    }
    this.content.replaceChildren(header, rows);
  }

  buildRow(row) {
    const article = element("article", "viewer-bookmark-row");
    const currentAddress = this.currentTarget()?.address;
    const isCurrent = Boolean(
      row.target && currentAddress &&
      addressIdentity(row.target.address) === addressIdentity(currentAddress)
    );
    article.classList.toggle("is-current", isCurrent);
    article.classList.toggle("is-unresolved", !row.target);

    const main = element("button", "viewer-bookmark-main");
    main.type = "button";
    main.disabled = this.busy || !row.target;
    const thumbnail = element("span", "viewer-bookmark-thumbnail");
    thumbnail.textContent = row.target ? "…" : "!";
    if (row.target) {
      const image = document.createElement("img");
      image.alt = "";
      image.loading = "lazy";
      image.decoding = "async";
      image.addEventListener("load", () => thumbnail.classList.add("is-loaded"));
      image.addEventListener("error", () => image.remove());
      image.src = apiUrl(
        "/api/thumb",
        addressQueryParams(row.target.address, {
          w: 116,
          epoch: state.remoteSessionCacheEpoch,
        })
      );
      thumbnail.append(image);
    }
    const details = element("span", "viewer-bookmark-details");
    details.append(
      textElement("strong", row.title ?? "名称なし", "viewer-bookmark-title"),
      textElement(
        "span",
        `${remoteBookBookmarkDisplayPage(row)} ページ`,
        "viewer-bookmark-page"
      ),
      textElement("span", row.pageLabel, "viewer-bookmark-label")
    );
    if (!row.target) {
      details.append(
        textElement(
          "span",
          "ページが見つかりません",
          "viewer-bookmark-missing"
        )
      );
    }
    main.append(thumbnail, details);
    if (row.target) {
      main.addEventListener("click", () => this.openRow(row));
    }
    article.append(main);

    if (this.editingId === row.id) {
      article.append(this.buildTitleEditor(row));
    } else {
      const controls = element("div", "viewer-bookmark-controls");
      const edit = textElement("button", "名前を編集");
      edit.type = "button";
      edit.disabled = this.busy;
      edit.addEventListener("click", () => {
        this.editingId = row.id;
        this.render();
      });
      const remove = textElement("button", "削除");
      remove.type = "button";
      remove.disabled = this.busy;
      remove.addEventListener("click", () => this.removeRow(row));
      controls.append(edit, remove);
      article.append(controls);
    }
    return article;
  }

  buildTitleEditor(row) {
    const editor = element("div", "viewer-bookmark-editor");
    const input = document.createElement("input");
    input.type = "text";
    input.value = row.title ?? "";
    input.autocomplete = "off";
    input.setAttribute("aria-label", "ブックマーク名");
    const save = textElement("button", "保存");
    save.type = "button";
    save.disabled = this.busy;
    save.addEventListener("click", () => this.setTitle(row, input.value));
    const clear = textElement("button", "名称なし");
    clear.type = "button";
    clear.disabled = this.busy;
    clear.addEventListener("click", () => this.setTitle(row, ""));
    const cancel = textElement("button", "キャンセル");
    cancel.type = "button";
    cancel.disabled = this.busy;
    cancel.addEventListener("click", () => {
      this.editingId = null;
      this.render();
    });
    editor.append(input, save, clear, cancel);
    requestAnimationFrame(() => input.focus({ preventScroll: true }));
    return editor;
  }

  async openRow(row) {
    if (this.busy || !row.target) return;
    this.busy = true;
    this.clearStatus();
    this.render();
    try {
      const opened = await openRemoteBookBookmarkTarget(row.target);
      if (!opened) throw new Error("ページが見つかりません");
      this.updateCurrentPage();
    } catch (error) {
      this.showError(error instanceof Error ? error : new Error("ページが見つかりません"));
    } finally {
      this.busy = false;
      this.render();
    }
  }

  addCurrent() {
    const target = this.currentTarget();
    if (!target) return;
    this.mutate({
      kind: "set_bookmark",
      address: target.address,
      context_address: target.contextAddress,
      page_index: target.pageIndex,
      bookmarked: true,
    }, "ブックマークを追加しました。");
  }

  setTitle(row, title) {
    const target = this.currentTarget();
    if (!target) return;
    this.mutate({
      kind: "set_book_bookmark_title",
      address: target.address,
      context_address: target.contextAddress,
      page_index: target.pageIndex,
      id: row.id,
      title,
    }, "ブックマーク名を更新しました。");
  }

  removeRow(row) {
    const target = this.currentTarget();
    if (!target) return;
    this.mutate({
      kind: "remove_book_bookmark",
      address: target.address,
      context_address: target.contextAddress,
      page_index: target.pageIndex,
      id: row.id,
    }, "ブックマークを削除しました。");
  }

  async mutate(request, successMessage) {
    if (this.busy) return;
    this.busy = true;
    this.status.textContent = "保存中…";
    this.status.classList.remove("is-error");
    this.render();
    try {
      await apiAddressPostJson("/api/write", request);
      this.editingId = null;
      await refreshViewerItemState().catch(() => {});
      await this.refresh();
      this.status.textContent = successMessage;
    } catch (error) {
      this.showError(error);
    } finally {
      this.busy = false;
      this.render();
    }
  }

  clearStatus() {
    this.status.textContent = "";
    this.status.classList.remove("is-error");
  }

  showError(error) {
    this.status.textContent = error instanceof Error
      ? error.message
      : "ブックマークを更新できませんでした。";
    this.status.classList.add("is-error");
  }

  destroy() {
    this.refreshSequence += 1;
    this.refreshController?.abort();
    this.refreshController = null;
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
    this.viewTrimPanel = null;
    this.bookmarkPanel = null;
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
    } else if (tab.id === "view_trim") {
      this.viewTrimPanel ??= new ViewerViewTrimPanel();
      this.placeholder.replaceChildren(this.viewTrimPanel.root);
      this.viewTrimPanel.refresh().catch((error) => this.viewTrimPanel.showError(error));
    } else if (tab.id === "bookmarks") {
      this.bookmarkPanel ??= new ViewerBookmarkPanel();
      this.placeholder.replaceChildren(this.bookmarkPanel.root);
      this.bookmarkPanel.refresh().catch((error) => this.bookmarkPanel.showError(error));
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

  refreshViewTrim() {
    if (!this.opened || state.viewerPanelTab !== "view_trim") return;
    this.viewTrimPanel?.refresh().catch((error) => this.viewTrimPanel.showError(error));
  }

  refreshBookmarks() {
    if (!this.opened || state.viewerPanelTab !== "bookmarks") return;
    this.bookmarkPanel?.updateCurrentPage();
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
      this.viewTrimPanel?.destroy();
      this.bookmarkPanel?.destroy();
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
      {
        resetTransform: true,
        reason: `panel_${action}`,
      }
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
      "ピンチで拡大・縮小します。拡大中は1本指で画像を動かせます。",
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

function setLocalImageQuality(quality) {
  const preset = IMAGE_QUALITY_PRESETS.find(
    (candidate) => candidate.id === quality
  );
  if (!preset) return false;
  const changed = state.localSettings.imageQuality !== preset.id;
  const saved = saveLocalSettings({
    ...state.localSettings,
    imageQuality: preset.id,
  });
  state.localSettings = saved.settings;
  state.localSettingsStorageAvailable = saved.saved;
  if (changed) {
    pageResourceCache.clear();
    if (
      state.screenContext === "viewer" &&
      state.viewer &&
      !state.viewer.isVideoStreamViewer
    ) {
      updateViewerImage(performance.now()).catch(renderError);
    }
  }
  return true;
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

    const qualityGroup = element("fieldset", "local-settings-group");
    qualityGroup.append(
      textElement("legend", "画像の画質", "local-settings-group-title")
    );
    for (const preset of IMAGE_QUALITY_PRESETS) {
      const qualityOption = element("label", "local-settings-option");
      const radio = element("input");
      radio.type = "radio";
      radio.name = "image-quality";
      radio.value = preset.id;
      radio.checked = state.localSettings.imageQuality === preset.id;
      const qualityCopy = element("span", "local-settings-copy");
      const timingAverage = formatPageTimingAverage(
        averagePageTimings(pageTimingHistory, preset.id)
      );
      qualityCopy.append(
        textElement("strong", preset.label),
        textElement("small", `最大 ${preset.maxLongSide} px`)
      );
      if (timingAverage) qualityCopy.append(textElement("small", timingAverage));
      qualityOption.append(radio, qualityCopy);
      radio.addEventListener("change", () => {
        if (!radio.checked || !setLocalImageQuality(preset.id)) return;
        this.updateStorageStatus();
      });
      qualityGroup.append(qualityOption);
    }

    const telemetryGroup = element("fieldset", "local-settings-group");
    telemetryGroup.append(
      textElement("legend", "診断記録", "local-settings-group-title")
    );
    const telemetryOption = element("label", "local-settings-option");
    const telemetryCheckbox = element("input");
    telemetryCheckbox.type = "checkbox";
    telemetryCheckbox.checked = state.localSettings.telemetryDebugDetails;
    const telemetryCopy = element("span", "local-settings-copy");
    telemetryCopy.append(
      textElement("strong", "詳細記録を有効にする"),
      textElement(
        "small",
        "調査時だけ使用します。ファイルの相対パス・remote address・端末 ID・サーバーメッセージを記録します。PIN、認証 token、remote session ID の生値は記録しません。"
      )
    );
    telemetryOption.append(telemetryCheckbox, telemetryCopy);
    telemetryGroup.append(telemetryOption);

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
        refreshContainerSpread(forceSinglePage).then((result) => {
          if (result.outcome === ViewerGroupLoadOutcome.FAILED) {
            state.viewer?.showBoundaryMessage(result.message);
          }
        });
      }
    });
    telemetryCheckbox.addEventListener("change", () => {
      const saved = saveLocalSettings({
        ...state.localSettings,
        telemetryDebugDetails: telemetryCheckbox.checked,
      });
      state.localSettings = saved.settings;
      state.localSettingsStorageAvailable = saved.saved;
      hudElement.hidden = false;
      state.viewer?.captureVideoHealth?.("hud");
      updateHud();
      this.updateStorageStatus();
    });

    panel.append(header, option, qualityGroup, telemetryGroup, this.status);
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

const REMOTE_ARCHIVE_STATE_LABELS = Object.freeze({
  waiting_for_local_drain: "PC 側の処理を待っています",
  inspecting: "アーカイブの内容を確認しています",
  awaiting_confirmation: "変換の確認を待っています",
  awaiting_password: "パスワードの入力を待っています",
  waiting_for_conversion_slot: "変換の開始を待っています",
  converting: "アーカイブを変換しています",
  finalizing: "変換結果を確認しています",
  cancelling: "取り消しています",
});

function formatArchiveBytes(value) {
  const bytes = Math.max(0, Number(value) || 0);
  if (bytes < 1024) return `${Math.floor(bytes)} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  }
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
}

export function remoteArchiveProgressText(snapshot) {
  const label = REMOTE_ARCHIVE_STATE_LABELS[snapshot?.state] || "アーカイブを準備しています";
  const progress = snapshot?.progress;
  if (!progress) return label;
  const parts = [label];
  const done = Math.max(0, Number(progress.files_done) || 0);
  const total = Math.max(0, Number(progress.files_total) || 0);
  if (total > 0) parts.push(`${Math.min(done, total)} / ${total} ファイル`);
  const bytes = Math.max(0, Number(progress.bytes_written) || 0);
  if (bytes > 0) parts.push(`${formatArchiveBytes(bytes)} 書き込み済み`);
  return parts.join(" · ");
}

export function selectRecoverableRemoteArchiveJob(jobs, requestId) {
  if (!requestId) return null;
  return (Array.isArray(jobs) ? jobs : [])
    .filter((job) => job?.request_id === requestId)
    .sort((left, right) => Number(right.created_unix_ms) - Number(left.created_unix_ms))[0] ?? null;
}

function remoteArchiveSourceHash(address) {
  const source = addressIdentity(address);
  let first = 0x811c9dc5;
  let second = 0x9e3779b9;
  for (let index = 0; index < source.length; index += 1) {
    const code = source.charCodeAt(index);
    first = Math.imul(first ^ code, 0x01000193) >>> 0;
    second = Math.imul(second ^ code, 0x85ebca6b) >>> 0;
  }
  return `${first.toString(16).padStart(8, "0")}${second.toString(16).padStart(8, "0")}`;
}

function createRemoteArchiveRequestId(address) {
  const unique = globalThis.crypto?.randomUUID?.() ??
    `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  return `miv-archive:${remoteArchiveSourceHash(address)}:${unique}`;
}

export class RemoteArchiveOpenController {
  constructor(host, subscribeRemoteSessionState = () => () => {}) {
    this.host = host;
    this.address = null;
    this.requestId = null;
    this.job = null;
    this.pollTimer = 0;
    this.destroyed = false;
    this.cancelOnDestroy = true;
    this.remoteSessionState = { blocksInteraction: false };
    this.previousFocus = document.activeElement;

    this.root = element("div", "archive-open-layer");
    this.root.setAttribute("role", "presentation");
    this.panel = element("section", "archive-open-dialog");
    this.panel.tabIndex = -1;
    this.panel.setAttribute("role", "dialog");
    this.panel.setAttribute("aria-modal", "true");
    this.title = textElement("h2", "アーカイブを開く");
    this.title.id = "archive-open-title";
    this.panel.setAttribute("aria-labelledby", this.title.id);
    this.sourceName = textElement("p", "", "archive-open-source");
    this.message = textElement("p", "準備しています", "archive-open-message");
    this.message.setAttribute("role", "status");
    this.message.setAttribute("aria-live", "polite");
    this.detail = textElement("p", "", "archive-open-detail");
    this.progress = element("progress", "archive-open-progress");
    this.progress.hidden = true;
    this.actions = element("div", "archive-open-actions");
    this.panel.append(
      this.title,
      this.sourceName,
      this.message,
      this.detail,
      this.progress,
      this.actions
    );
    this.root.append(this.panel);
    this.root.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      if (this.job && !ARCHIVE_TERMINAL_STATES.has(this.job.state)) {
        this.requestCancel().catch(() => {});
      } else {
        this.close();
      }
    });
    this.host.append(this.root);
    this.unsubscribeRemoteSessionState = subscribeRemoteSessionState((snapshot) => {
      this.remoteSessionState = snapshot ?? { blocksInteraction: false };
      if (this.remoteSessionState.blocksInteraction) {
        this.clearPoll();
      } else if (this.job && !ARCHIVE_TERMINAL_STATES.has(this.job.state)) {
        this.schedulePoll(0);
      }
    });
    queueMicrotask(() => this.panel.focus?.());
  }

  async open(address, name) {
    if (this.destroyed) return;
    this.address = address;
    this.requestId = createRemoteArchiveRequestId(address);
    this.sourceName.textContent = name;
    this.showWorking("アーカイブを準備しています");
    try {
      const snapshot = await apiAddressPostJson("/api/archive/jobs", {
        request_id: this.requestId,
        source: address,
      });
      await this.handleSnapshot(snapshot);
    } catch (error) {
      const recovered = await this.recover().catch(() => null);
      if (recovered) return;
      throw error;
    }
  }

  async recover() {
    if (!this.requestId) return false;
    const jobs = await apiJson("/api/archive/jobs", { recoverable: 1 });
    const recovered = selectRecoverableRemoteArchiveJob(jobs, this.requestId);
    if (!recovered) return false;
    if (this.destroyed) {
      this.cancelSnapshotBestEffort(recovered);
      return true;
    }
    await this.handleSnapshot(recovered);
    return true;
  }

  async handleForegroundResume() {
    if (this.destroyed) return;
    this.clearPoll();
    if (await this.recover().catch(() => false)) return;
    if (this.job && !ARCHIVE_TERMINAL_STATES.has(this.job.state)) {
      await this.poll();
    }
  }

  suspend() {
    this.clearPoll();
  }

  async poll() {
    if (
      this.destroyed ||
      !this.job?.job_id ||
      document.visibilityState === "hidden" ||
      this.remoteSessionState.blocksInteraction
    ) return;
    const snapshot = await apiJson(
      `/api/archive/jobs/${encodeURIComponent(this.job.job_id)}`
    );
    await this.handleSnapshot(snapshot);
  }

  async handleSnapshot(snapshot) {
    if (!snapshot?.job_id) return;
    if (this.destroyed) {
      this.cancelSnapshotBestEffort(snapshot);
      return;
    }
    this.job = snapshot;
    this.requestId = snapshot.request_id;
    this.clearPoll();
    if (snapshot.state === "ready") {
      await this.openReady(snapshot);
      return;
    }
    if (ARCHIVE_TERMINAL_STATES.has(snapshot.state)) {
      this.showTerminal(snapshot);
      return;
    }
    const input = snapshot.awaiting_input;
    if (input?.kind === "confirmation") {
      this.showConfirmation(input.summary ?? {});
      return;
    }
    if (input?.kind === "password") {
      this.showPassword(Boolean(input.bad_password));
      return;
    }
    this.showWorking(remoteArchiveProgressText(snapshot), snapshot.progress);
    this.showCancelAction();
    this.schedulePoll(ARCHIVE_FOREGROUND_POLL_MS);
  }

  showConfirmation(summary) {
    this.message.textContent = "このアーカイブは変換してから開きます。変換には時間がかかることがあります。";
    const details = [
      `画像 ${Math.max(0, Number(summary.image_count) || 0)} 枚`,
      `展開後 ${formatArchiveBytes(summary.total_uncompressed_bytes)}`,
    ];
    const nested = Math.max(0, Number(summary.nested_archive_count) || 0);
    if (nested > 0) details.push(`入れ子アーカイブ ${nested} 個`);
    this.detail.textContent = `${details.join(" · ")}。変換後のキャッシュ ZIP は暗号化されません。`;
    this.progress.hidden = true;
    const proceed = textElement("button", "変換して開く", "archive-open-primary");
    proceed.type = "button";
    proceed.addEventListener("click", () => {
      this.submitConfirmation(true).catch((error) => this.showRequestError(error));
    });
    const decline = textElement("button", "開かない");
    decline.type = "button";
    decline.addEventListener("click", () => {
      this.submitConfirmation(false).catch((error) => this.showRequestError(error));
    });
    this.actions.replaceChildren(decline, proceed);
    queueMicrotask(() => proceed.focus());
  }

  showPassword(badPassword) {
    this.message.textContent = badPassword
      ? "パスワードが違います。もう一度入力してください。"
      : "この RAR を開くにはパスワードが必要です。";
    this.detail.textContent = "パスワードはこの送信にだけ使い、ジョブ状態・URL・ログには残しません。変換したキャッシュ ZIP は暗号化されません。";
    this.progress.hidden = true;
    const form = element("form", "archive-open-password-form");
    const input = element("input", "archive-open-password");
    input.type = "password";
    input.autocomplete = "off";
    input.required = true;
    input.maxLength = 1024;
    input.setAttribute("aria-label", "アーカイブのパスワード");
    const submit = textElement("button", "送信", "archive-open-primary");
    submit.type = "submit";
    const cancel = textElement("button", "中止");
    cancel.type = "button";
    cancel.addEventListener("click", () => {
      input.value = "";
      this.requestCancel().catch((error) => this.showRequestError(error));
    });
    form.append(input, cancel, submit);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const password = input.value;
      input.value = "";
      if (!password) return;
      submit.disabled = true;
      this.submitPassword(password).catch((error) => this.showRequestError(error));
    });
    this.actions.replaceChildren(form);
    queueMicrotask(() => input.focus());
  }

  async submitConfirmation(proceed) {
    if (!this.job?.job_id) return;
    this.showWorking(proceed ? "変換を開始します" : "変換を取りやめています");
    this.actions.replaceChildren();
    const snapshot = await apiPostJson(
      `/api/archive/jobs/${encodeURIComponent(this.job.job_id)}/confirm`,
      { proceed }
    );
    await this.handleSnapshot(snapshot);
  }

  async submitPassword(password) {
    if (!this.job?.job_id) return;
    this.showWorking("パスワードを確認しています");
    this.actions.replaceChildren();
    const snapshot = await apiPostJson(
      `/api/archive/jobs/${encodeURIComponent(this.job.job_id)}/password`,
      { password }
    );
    password = "";
    await this.handleSnapshot(snapshot);
  }

  async requestCancel() {
    if (!this.job?.job_id || ARCHIVE_TERMINAL_STATES.has(this.job.state)) {
      this.close();
      return;
    }
    this.showWorking("取り消しています");
    this.actions.replaceChildren();
    const response = await observedFetch(
      `/api/archive/jobs/${encodeURIComponent(this.job.job_id)}`,
      {
        method: "DELETE",
        credentials: "same-origin",
        headers: remoteHeaders({ Accept: "application/json" }),
      }
    );
    if (!response.ok) {
      const detail = await response.clone().json().catch(() => ({}));
      throw new Error(detail.message || "アーカイブ操作を取り消せませんでした。");
    }
    await this.handleSnapshot(await response.json());
  }

  async openReady(snapshot) {
    this.showWorking("アーカイブを開いています");
    this.actions.replaceChildren();
    const result = await apiJson(
      `/api/archive/jobs/${encodeURIComponent(snapshot.job_id)}/result`
    );
    if (this.destroyed) return;
    if (
      !result?.source ||
      addressIdentity(result.source) !== addressIdentity(this.address)
    ) {
      throw new Error("準備したアーカイブの公開アドレスを確認できませんでした。");
    }
    this.cancelOnDestroy = false;
    this.close(false);
    navigate(containerHash(result.source));
  }

  showWorking(message, progress = null) {
    if (this.destroyed) return;
    this.root.classList.remove("is-error");
    this.message.textContent = message;
    this.detail.textContent = "";
    const total = Math.max(0, Number(progress?.files_total) || 0);
    const done = Math.max(0, Number(progress?.files_done) || 0);
    this.progress.hidden = total <= 0;
    if (total > 0) {
      this.progress.max = total;
      this.progress.value = Math.min(done, total);
    }
  }

  showCancelAction() {
    const cancel = textElement("button", "中止");
    cancel.type = "button";
    cancel.addEventListener("click", () => {
      this.requestCancel().catch((error) => this.showRequestError(error));
    });
    this.actions.replaceChildren(cancel);
  }

  showTerminal(snapshot) {
    this.cancelOnDestroy = false;
    this.progress.hidden = true;
    this.message.textContent = snapshot.terminal?.message || (
      snapshot.state === "declined_by_user"
        ? "変換しませんでした。"
        : "アーカイブ操作を完了できませんでした。"
    );
    this.detail.textContent = "";
    this.root.classList.toggle("is-error", snapshot.state === "failed");
    const close = textElement("button", "閉じる", "archive-open-primary");
    close.type = "button";
    close.addEventListener("click", () => this.close());
    this.actions.replaceChildren(close);
    queueMicrotask(() => close.focus());
  }

  showRequestError(error) {
    if (this.destroyed || error?.name === "AbortError") return;
    this.clearPoll();
    this.root.classList.add("is-error");
    this.message.textContent = error instanceof Error
      ? error.message
      : "アーカイブ操作を続けられませんでした。";
    this.detail.textContent = "";
    this.progress.hidden = true;
    const close = textElement("button", "閉じる", "archive-open-primary");
    close.type = "button";
    close.addEventListener("click", () => this.close());
    this.actions.replaceChildren(close);
  }

  schedulePoll(delay) {
    this.clearPoll();
    if (
      this.destroyed ||
      document.visibilityState === "hidden" ||
      this.remoteSessionState.blocksInteraction
    ) return;
    this.pollTimer = window.setTimeout(() => {
      this.pollTimer = 0;
      this.poll().catch((error) => {
        if (this.destroyed) return;
        if (
          error instanceof AuthenticationRequiredError ||
          (Number.isFinite(Number(error?.status)) && Number(error.status) < 500)
        ) {
          this.showRequestError(error);
          return;
        }
        this.showWorking("接続を確認しています");
        this.showCancelAction();
        this.schedulePoll(Math.max(1000, ARCHIVE_FOREGROUND_POLL_MS));
      });
    }, delay);
  }

  clearPoll() {
    clearTimeout(this.pollTimer);
    this.pollTimer = 0;
  }

  cancelSnapshotBestEffort(snapshot) {
    if (!snapshot?.job_id || ARCHIVE_TERMINAL_STATES.has(snapshot.state)) return;
    observedFetch(
      "/api/archive/jobs/" + encodeURIComponent(snapshot.job_id),
      {
        method: "DELETE",
        credentials: "same-origin",
        headers: remoteHeaders({ Accept: "application/json" }),
      }
    ).catch(() => {});
  }

  close(restoreFocus = true) {
    if (this.job && ARCHIVE_TERMINAL_STATES.has(this.job.state)) {
      this.cancelOnDestroy = false;
    }
    const previousFocus = this.previousFocus;
    this.destroy();
    if (restoreFocus && previousFocus instanceof HTMLElement) {
      previousFocus.focus({ preventScroll: true });
    }
  }

  destroy() {
    if (this.destroyed) return;
    const activeJobId = this.cancelOnDestroy && this.job &&
      !ARCHIVE_TERMINAL_STATES.has(this.job.state)
      ? this.job.job_id
      : null;
    this.destroyed = true;
    this.clearPoll();
    this.unsubscribeRemoteSessionState?.();
    this.root.remove();
    if (state.archiveOpenController === this) state.archiveOpenController = null;
    if (activeJobId) {
      observedFetch(`/api/archive/jobs/${encodeURIComponent(activeJobId)}`, {
        method: "DELETE",
        credentials: "same-origin",
        headers: remoteHeaders({ Accept: "application/json" }),
      }).catch(() => {});
    }
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

/// 縮小表示の短いラベル。詳細文言とは別に持ち、呼び出し側が状態から選ぶ。
export const RemoteAiShortLabel = Object.freeze({
  WORKING: "AI 処理中",
  CONNECTING: "AI 接続確認中",
  DONE: "AI 完了",
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

export function remoteAiCompletionMessage({ readyCount, notApplicableCount }) {
  const ready = Math.max(0, Math.floor(Number(readyCount) || 0));
  if (!ready) return null;
  return Math.max(0, Math.floor(Number(notApplicableCount) || 0)) > 0
    ? "AI 処理が完了しました。一部のページは元の表示です。"
    : "AI 処理が完了しました。";
}

function remoteAiGroupHash(pages, revision = state.pageRenderRevision) {
  const source = `${revision}\n${pages.map((page) =>
    `${addressIdentity(page.address)}\n${Math.max(1, Number(page.target_px) || 1)}\n${JSON.stringify(page.render_context ?? null)}`
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
  constructor(viewer, stage, subscribeRemoteSessionState = () => () => {}) {
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
    this.expanded = false;
    this.remoteSessionState = { blocksInteraction: false };
    this.unsubscribeRemoteSessionState = () => {};

    this.root = element("div", "viewer-ai-status");
    this.root.hidden = true;
    this.root.setAttribute("role", "status");
    this.root.setAttribute("aria-live", "polite");
    this.toggleButton = element("button", "viewer-ai-status-toggle");
    this.toggleButton.type = "button";
    this.spinner = element("span", "viewer-ai-status-spinner");
    this.shortLabel = textElement("span", RemoteAiShortLabel.WORKING);
    this.toggleButton.append(this.spinner, this.shortLabel);
    this.toggleButton.setAttribute("aria-label", "AI 処理の詳細を表示");
    this.toggleButton.setAttribute("aria-expanded", "false");
    this.toggleButton.addEventListener("click", (event) => {
      event.stopPropagation();
      this.setExpanded(!this.expanded);
    });
    this.details = element("div", "viewer-ai-status-details");
    this.details.hidden = true;
    this.message = textElement("span", "", "viewer-ai-status-message");
    this.details.append(this.message);
    this.root.append(this.toggleButton, this.details);
    viewer.root.append(this.root);
    this.unsubscribeRemoteSessionState = subscribeRemoteSessionState((snapshot) => {
      this.remoteSessionState = snapshot ?? { blocksInteraction: false };
      if (this.remoteSessionState.blocksInteraction) this.clearPoll();
    });
  }

  async displayGroup(pages) {
    const normalized = pages
      .filter((page) => page?.address)
      .map((page) => ({
        address: page.address,
        target_px: Math.max(1, Math.round(Number(page.target_px) || 1)),
        render_context: page.render_context ?? null,
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
      const response = await apiAddressPostJson("/api/write", {
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
    this.show("準備しています");
    const snapshot = await apiAddressPostJson("/api/ai/jobs", {
      request_id: this.requestId,
      pages: this.pages.map(({ address, target_px, render_context }) => ({
        address,
        target_px,
        render_context,
      })),
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
    if (
      error instanceof AuthenticationRequiredError ||
      this.remoteSessionState.blocksInteraction
    ) return;
    if (error?.code === "page_identity_mismatch") {
      this.show(error.message, { error: true, shortLabel: RemoteAiShortLabel.DONE });
      return;
    }
    this.show("接続を確認しています", {
      shortLabel: RemoteAiShortLabel.CONNECTING,
    });
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
      this.show(remoteAiProgressText(snapshot));
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
    this.show(message, {
      error: snapshot.state === "failed",
      shortLabel: RemoteAiShortLabel.DONE,
    });
  }

  async applyReady(snapshot, generation) {
    const ready = (snapshot.page_outcomes ?? [])
      .filter((outcome) => outcome.state === "ready")
      .map((outcome) => Number(outcome.page_index))
      .filter((index) => Number.isInteger(index) && index >= 0 && index < this.pages.length);
    const notApplicable = (snapshot.page_outcomes ?? [])
      .filter((outcome) => outcome.state === "not_applicable");
    const completionMessage = remoteAiCompletionMessage({
      readyCount: ready.length,
      notApplicableCount: notApplicable.length,
    });
    if (!completionMessage) {
      this.hide();
      return;
    }
    const appliedIdentity = `${snapshot.job_id}:${this.displayVersion}`;
    if (this.appliedIdentity === appliedIdentity) {
      return;
    }
    this.show("表示を整えています");
    const replacements = await Promise.all(ready.map(async (pageIndex) => {
      const response = await observedFetch(
        `/api/ai/jobs/${encodeURIComponent(snapshot.job_id)}/result?page=${pageIndex}`,
        { credentials: "same-origin", headers: { Accept: "image/jpeg" } }
      );
      if (!response.ok) {
        const detail = await response.clone().json().catch(() => ({}));
        throw new Error(detail.message || "AI 処理後の画像を取得できませんでした。");
      }
      try {
        requirePageResponseIdentity(this.pages[pageIndex]?.address, response);
      } catch (error) {
        if (error?.code === "page_identity_mismatch") {
          // Any rejected page makes this exact job/display result terminal for application.
          this.appliedIdentity = appliedIdentity;
        }
        throw error;
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
    this.show(completionMessage, {
        hideAfterMs: 2400,
        shortLabel: RemoteAiShortLabel.DONE,
      });
  }

  showRequestError(error) {
    this.show(
      error?.code === "page_identity_mismatch"
        ? error.message
        : "AI 処理を開始できませんでした。",
      { error: true }
    );
  }

  show(
    message,
    {
      error = false,
      hideAfterMs = 0,
      shortLabel = RemoteAiShortLabel.WORKING,
    } = {}
  ) {
    clearTimeout(this.hideTimer);
    this.hideTimer = 0;
    const wasError = this.root.classList.contains("is-error");
    this.message.textContent = message;
    this.root.setAttribute("aria-label", message);
    this.root.classList.toggle("is-error", error);
    // 縮小表示のラベルは呼び出し側が渡す。詳細文言との文字列一致で決めると、
    // 文言を書き換えたときに黙って既定へ落ちる。
    this.shortLabel.textContent = shortLabel;
    this.spinner.hidden = error || hideAfterMs > 0;
    this.toggleButton.hidden = error;
    if (error) this.setExpanded(true);
    else if (wasError || hideAfterMs > 0) this.setExpanded(false);
    else this.setExpanded(this.expanded);
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
    this.toggleButton.hidden = false;
    this.setExpanded(false);
  }

  setExpanded(expanded) {
    this.expanded = Boolean(expanded);
    this.root.classList.toggle("is-expanded", this.expanded);
    this.details.hidden = !this.expanded;
    this.toggleButton.setAttribute("aria-expanded", String(this.expanded));
    this.toggleButton.setAttribute(
      "aria-label",
      this.expanded ? "AI 処理の詳細を閉じる" : "AI 処理の詳細を表示"
    );
  }

  schedulePoll(delay, generation) {
    this.clearPoll();
    if (
      document.visibilityState === "hidden" ||
      this.remoteSessionState.blocksInteraction ||
      !this.isCurrent(generation)
    ) return;
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
    this.unsubscribeRemoteSessionState();
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

function viewerPageDisplayFailureReason(error, phase) {
  if (error?.name === "AbortError") return "abort";
  const code = String(error?.code ?? "");
  if (new Set([
    "page_identity_mismatch",
    "remote_state_generation_unattested",
    "remote_state_generation_mismatch",
    "remote_session_unattested",
  ]).has(code)) return code;
  if (phase === "fetch") return "fetch_failed";
  if (phase === "decode") return "decode_failed";
  return "apply_failed";
}

/// 失敗そのものだけを述べる。位置を戻したかどうかは完了処理しか知らないので、
/// ここで「前のページに戻りました」と書くと、戻していない経路で嘘になる。
function viewerGroupLoadFailure(error, fallbackMessage) {
  if (error?.name === "AbortError") {
    return {
      outcome: ViewerGroupLoadOutcome.FAILED,
      message: "ページの表示が中断されました。",
    };
  }
  const message = error instanceof Error ? error.message.trim() : "";
  return {
    outcome: ViewerGroupLoadOutcome.FAILED,
    message: message || fallbackMessage,
  };
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
    this.destroyed = false;
    this.pageLoadBusy = false;
    this.requestedPagePresentation = null;
    this.displayedSeekState = null;
    this.pageLoadQueue = new LatestPageLoadQueue(
      (job) => job.kind === "spread"
        ? this.loadMeasuredSpread(
          job.pages,
          job.fitMode,
          job.gap,
          job.interactionStartedAt,
          job.presentation,
          job.renderTrigger
        )
        : this.loadMeasuredImage(
          job.request,
          job.interactionStartedAt,
          job.name,
          job.info,
          job.presentation,
          job.renderTrigger
        ),
      () => this.supersedeActiveLoad(),
      (busy) => this.setPageLoadBusy(busy),
      (job, reason) => this.recordPageDisplay(
        job?.presentation,
        "not_applied",
        reason
      )
    );

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
      const viewer = this;
      this.resizeTimer = setTimeout(() => {
        if (
          remoteSessionControlOwner.snapshot.status !== "active" ||
          !state.remoteSessionId ||
          state.viewer !== viewer
        ) {
          return;
        }
        const forceSinglePage = shouldForceSinglePageForViewport();
        const plan = viewerResizePlan({
          hasContainer: Boolean(state.container),
          forceSinglePageChanged: forceSinglePage !== state.forceSinglePage,
          panelOpen: Boolean(state.commandMenu?.isOpen()),
        });
        if (plan.refreshContainer) {
          refreshContainerSpread(forceSinglePage, "viewport_resize").then((result) => {
            if (result.outcome === ViewerGroupLoadOutcome.FAILED) {
              renderError(new Error(result.message));
            }
          });
          return;
        }
        updateViewerImage(performance.now(), {
          renderTrigger: "viewport_resize",
        }).catch(renderError);
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
    pageNumbers,
    interactionStartedAt,
    renderTrigger,
  }) {
    const resolvedSeekState = seekState ?? {
      visible: count > 1,
      min: 0,
      max: Math.max(0, count - 1),
      value: index,
      groupIndex: index,
      direction: ReadingDirection.LTR,
      label: `${index + 1} / ${count}`,
    };
    const presentation = {
      name,
      seekState: resolvedSeekState,
      pageNumbers: pageNumbers ?? [index + 1],
    };
    this.setRequestedPagePresentation(presentation);
    return this.pageLoadQueue.request({
      kind: "single",
      request,
      interactionStartedAt,
      name,
      info,
      presentation,
      renderTrigger,
    });
  }

  supersedeActiveLoad() {
    this.loadSequence += 1;
  }

  invalidatePendingLoad() {
    this.pageLoadQueue.clear();
    if (!this.pageLoadQueue.isBusy()) {
      clearTimeout(this.loadingTimer);
      this.loadingTimer = 0;
      this.loadingIndicator.hidden = true;
    }
    this.supersedeActiveLoad();
    this.fetchController?.abort();
    this.fetchController = null;
  }

  loadGroup({
    pages,
    name,
    fitMode,
    gap,
    index,
    count,
    seekState,
    pageNumbers,
    interactionStartedAt,
    renderTrigger,
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
        pageNumbers,
        interactionStartedAt,
        renderTrigger,
      });
    }
    const resolvedSeekState = seekState ?? {
      visible: count > 1,
      min: 0,
      max: Math.max(0, count - 1),
      value: index,
      groupIndex: index,
      direction: ReadingDirection.LTR,
      label: `${index + 1} / ${count}`,
    };
    const presentation = {
      name,
      seekState: resolvedSeekState,
      pageNumbers: pageNumbers ?? [index + 1],
    };
    this.setRequestedPagePresentation(presentation);
    return this.pageLoadQueue.request({
      kind: "spread",
      pages,
      fitMode,
      gap,
      interactionStartedAt,
      presentation,
      renderTrigger,
    });
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
    this.seekInput.dir = seekState.direction === ReadingDirection.RTL
      ? ReadingDirection.RTL
      : ReadingDirection.LTR;
    this.seekInput.value = String(seekState.value);
    this.seekInput.disabled = seekState.max <= seekState.min;
    this.seekInput.setAttribute("aria-valuetext", seekState.label);
  }

  initializePagePresentation(presentation) {
    if (!presentation?.seekState) return;
    this.requestedPagePresentation = {
      name: presentation.name ?? "",
      seekState: { ...presentation.seekState },
    };
    this.displayedSeekState = { ...presentation.seekState };
    this.syncPagePositionFeedback();
  }

  setRequestedPagePresentation(presentation) {
    if (!presentation?.seekState) return;
    if (!this.displayedSeekState) {
      this.initializePagePresentation(presentation);
      return;
    }
    this.requestedPagePresentation = {
      name: presentation.name ?? "",
      seekState: { ...presentation.seekState },
    };
    this.syncPagePositionFeedback();
  }

  displayedGroupIndex() {
    const groupIndex = Number(this.displayedSeekState?.groupIndex);
    return Number.isInteger(groupIndex) && groupIndex >= 0 ? groupIndex : null;
  }

  syncPagePositionFeedback() {
    const requested = this.requestedPagePresentation;
    const displayedGroupIndex = this.displayedGroupIndex();
    if (!requested?.seekState || !Number.isInteger(displayedGroupIndex)) return;
    const feedback = viewerPagePositionFeedback({
      requestedGroupIndex: requested.seekState.groupIndex,
      displayedGroupIndex,
    });
    this.title.textContent = requested.name;
    this.setSeekState(requested.seekState);
    this.counter.classList.toggle("is-pending", feedback.pending);
  }

  commitPagePresentation({ name, seekState } = {}) {
    if (!seekState) return;
    if (!this.requestedPagePresentation || !this.displayedSeekState) {
      this.initializePagePresentation({ name, seekState });
      return;
    }
    const position = viewerPagePositionTransition(
      {
        requestedGroupIndex: this.requestedPagePresentation.seekState.groupIndex,
        displayedGroupIndex: this.displayedSeekState.groupIndex,
      },
      { type: ViewerPagePositionEvent.DISPLAY, groupIndex: seekState.groupIndex }
    );
    this.displayedSeekState = {
      ...seekState,
      groupIndex: position.displayedGroupIndex,
    };
    this.syncPagePositionFeedback();
  }

  restoreRequestedPagePresentation() {
    this.syncPagePositionFeedback();
  }

  recordPageDisplay(
    presentation,
    outcome,
    reason,
    candidateImageIds = [],
    appliedImageIds = []
  ) {
    if (!presentation?.seekState) return;
    enqueueTelemetry(viewerPageDisplayHistoryEvent({
      outcome,
      reason,
      requestedGroupIndex: presentation.seekState.groupIndex,
      requestedPageNumbers: presentation.pageNumbers,
      seekGroupIndex: presentation.seekState.value,
      seekPageLabel: presentation.seekState.label,
      candidateImageIds,
      appliedImageIds,
    }));
  }

  layoutTelemetry() {
    return viewerLayoutTelemetry({
      stageWidth: this.stage.clientWidth,
      stageHeight: this.stage.clientHeight,
      viewportWidth: window.innerWidth,
      viewportHeight: window.innerHeight,
      visualViewportWidth: window.visualViewport?.width,
      visualViewportHeight: window.visualViewport?.height,
      panelOpen: this.root.classList.contains("viewer-panel-open"),
      barsVisible: !this.root.classList.contains("viewer-bars-hidden"),
    });
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
    // 始点を決めるのは placeInitialStageScroll の役目。ここで 0 に戻すと、原寸の
    // 中央寄せを毎回打ち消す。
    this.placeInitialStageScroll();
  }

  setPageLayerSize(width, height) {
    this.pageLayer.style.width = `${Math.max(1, Number(width) || 1)}px`;
    this.pageLayer.style.height = `${Math.max(1, Number(height) || 1)}px`;
  }

  /// 新しい配置を入れた直後に、どこから見せ始めるか。
  ///
  /// **配置の寸法をすべて当ててから呼ぶこと。** 画像の寸法は page layer より後に当てる
  /// 経路があり、途中で測ると内容がまだ最終の大きさではない。ページ送りだけ効いて
  /// モード切替で効かなかったのはこれが理由。
  ///
  /// 原寸は内容が画面より大きいので、始点を決める必要がある。本体アプリはページ中央から
  /// 見せるので合わせる。左上は余白であることが多く、実用でも中央のほうが良い。
  /// 他のモードは従来どおり先頭から見せる (はみ出すのが縦だけなので始点の選択が要らない)。
  placeInitialStageScroll() {
    if (this.fitMode !== FitMode.ORIGINAL) {
      this.stage.scrollTop = 0;
      this.stage.scrollLeft = 0;
      return;
    }
    this.stage.scrollLeft = Math.max(
      0,
      (this.stage.scrollWidth - this.stage.clientWidth) / 2
    );
    this.stage.scrollTop = Math.max(
      0,
      (this.stage.scrollHeight - this.stage.clientHeight) / 2
    );
  }

  refitVisibleContent(
    fitMode = FitMode.PAGE,
    { resetTransform = true, reason = "explicit_refit" } = {}
  ) {
    const previousCssWidth = Number.parseFloat(this.pageLayer.style.width);
    const previousCssHeight = Number.parseFloat(this.pageLayer.style.height);
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
    this.placeInitialStageScroll();
    if (resetTransform) {
      this.scale = 1;
      this.panX = 0;
      this.panY = 0;
    }
    this.applyTransform();
    enqueueTelemetry({
      type: "viewer_layout",
      action: "refit",
      reason,
      fit_mode: fitMode,
      previous_css_width: Number.isFinite(previousCssWidth)
        ? roundMs(previousCssWidth)
        : null,
      previous_css_height: Number.isFinite(previousCssHeight)
        ? roundMs(previousCssHeight)
        : null,
      css_width: roundMs(layout.cssWidth),
      css_height: roundMs(layout.cssHeight),
      spread_pages: this.images.length,
      ...viewerTransformTelemetry(this.scale, currentVisualViewportScale()),
      ...this.layoutTelemetry(),
    });
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

  async loadMeasuredImage(
    request,
    interactionStartedAt,
    name,
    info,
    presentation,
    renderTrigger
  ) {
    const sequence = ++this.loadSequence;
    this.fetchController?.abort();
    const controller = new AbortController();
    this.fetchController = controller;
    const fetchStartedAt = performance.now();
    let pendingObjectUrl = null;
    let resource = null;
    let phase = "fetch";
    try {
      if (request.cacheKey) {
        resource = await pageResourceCache.loadForeground(request, controller.signal);
      } else {
        const response = await observedFetch(request.url, {
          signal: controller.signal,
          credentials: "same-origin",
          sessionEpochBound: true,
        });
        if (!response.ok) {
          throw await pageResourceResponseError(response);
        }
        if (request.address) requirePageResponseIdentity(request.address, response);
        if (request.remoteStateGeneration != null) {
          requirePageResponseGeneration(request, response);
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
      if (sequence !== this.loadSequence) {
        this.recordPageDisplay(
          presentation,
          "not_applied",
          "load_sequence_mismatch",
          [requestId]
        );
        return VIEWER_GROUP_LOAD_SUPERSEDED;
      }

      phase = "decode";
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
        pendingObjectUrl = null;
        this.recordPageDisplay(
          presentation,
          "not_applied",
          "load_sequence_mismatch",
          [requestId]
        );
        return VIEWER_GROUP_LOAD_SUPERSEDED;
      }
      phase = "apply";
      this.resetTransform();
      this.setLayout(request.fitMode, resolvedLayout, resolvedInfo, decodedImage);
      decodedImage.style.transform = "none";
      const previousUrls = this.objectUrls.slice();
      this.pageLayer.replaceChildren(decodedImage);
      this.image = decodedImage;
      this.images = [decodedImage];
      this.recordPageDisplay(
        presentation,
        "applied",
        "dom_committed",
        [requestId],
        [requestId]
      );
      this.commitPagePresentation(presentation);
      this.objectUrl = pendingObjectUrl;
      this.objectUrls = [pendingObjectUrl];
      pendingObjectUrl = null;
      previousUrls.forEach((url) => URL.revokeObjectURL(url));
      await nextFrame();
      if (sequence !== this.loadSequence) return VIEWER_GROUP_LOAD_SUPERSEDED;
      recordSuccessfulPageTiming(request, resource, decodeMs);

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
        css_height: roundMs(resolvedLayout.cssHeight),
        device_pixel_ratio: roundMs(request.dpr),
        fit_mode: request.fitMode,
        render_trigger: renderTrigger,
        ...viewerTransformTelemetry(this.scale, currentVisualViewportScale()),
        ...this.layoutTelemetry(),
        prefetch_status: resource.prefetchStatus,
      };
      enqueueTelemetry(event);
      hudState.lastImage = event;
      hudState.displayDurations.push(event.tap_to_display_ms);
      if (hudState.displayDurations.length > 20) hudState.displayDurations.shift();
      updateHud();
      return VIEWER_GROUP_LOAD_APPLIED;
    } catch (error) {
      if (pendingObjectUrl) URL.revokeObjectURL(pendingObjectUrl);
      if (sequence !== this.loadSequence) {
        this.recordPageDisplay(
          presentation,
          "not_applied",
          "load_sequence_mismatch",
          [resource?.requestId]
        );
        return VIEWER_GROUP_LOAD_SUPERSEDED;
      }
      const reason = viewerPageDisplayFailureReason(error, phase);
      this.recordPageDisplay(
        presentation,
        "not_applied",
        reason,
        [resource?.requestId]
      );
      if (error?.name !== "AbortError") {
        this.root.classList.remove("viewer-ui-hidden");
      }
      if (
        error?.name !== "AbortError" &&
        error?.code !== "page_identity_mismatch"
      ) {
        recordClientError("image_load_error", error, {
          resource: safeResourcePath(request.url),
        });
      }
      return viewerGroupLoadFailure(error, "ページを表示できませんでした。");
    }
  }

  async loadMeasuredSpread(
    pages,
    fitMode,
    gap,
    interactionStartedAt,
    presentation,
    renderTrigger
  ) {
    const sequence = ++this.loadSequence;
    this.fetchController?.abort();
    const controller = new AbortController();
    this.fetchController = controller;
    const startedAt = performance.now();
    const pendingUrls = [];
    let resources = null;
    let phase = "fetch";
    try {
      resources = await Promise.all(pages.map(async ({ request }) => {
        if (request.cacheKey) {
          return pageResourceCache.loadForeground(request, controller.signal);
        }
        const fetchStartedAt = performance.now();
        const response = await observedFetch(request.url, {
          signal: controller.signal,
          credentials: "same-origin",
          sessionEpochBound: true,
        });
        if (!response.ok) {
          throw await pageResourceResponseError(response);
        }
        if (request.address) requirePageResponseIdentity(request.address, response);
        if (request.remoteStateGeneration != null) {
          requirePageResponseGeneration(request, response);
        }
        return {
          blob: await response.blob(),
          requestId: response.headers.get("X-mIV-Request-Id"),
          fetchMs: performance.now() - fetchStartedAt,
          prefetchStatus: "not_applicable",
        };
      }));
      if (sequence !== this.loadSequence) {
        this.recordPageDisplay(
          presentation,
          "not_applied",
          "load_sequence_mismatch",
          resources.map((resource) => resource.requestId)
        );
        return VIEWER_GROUP_LOAD_SUPERSEDED;
      }

      phase = "decode";
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
        this.recordPageDisplay(
          presentation,
          "not_applied",
          "load_sequence_mismatch",
          resources.map((resource) => resource.requestId)
        );
        return VIEWER_GROUP_LOAD_SUPERSEDED;
      }

      phase = "apply";
      const resolvedLayout = viewerSpreadLayout({
        mode: fitMode,
        pages: decodedImages.map((decoded) => decoded.info),
        viewportWidth: this.stage.clientWidth || window.innerWidth,
        viewportHeight: this.stage.clientHeight || window.innerHeight,
        devicePixelRatio: window.devicePixelRatio || 1,
        gap,
      });
      this.resetTransform();
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
      const previousUrls = this.objectUrls.slice();
      this.pageLayer.replaceChildren(...decodedImages.map((decoded) => decoded.image));
      this.images = decodedImages.map((decoded) => decoded.image);
      this.image = this.images[0];
      this.recordPageDisplay(
        presentation,
        "applied",
        "dom_committed",
        resources.map((resource) => resource.requestId),
        resources.map((resource) => resource.requestId)
      );
      // 中身を差し替えてから決める。差し替え前は内容がまだ DOM に入っていない。
      this.placeInitialStageScroll();
      this.commitPagePresentation(presentation);
      this.objectUrls = pendingUrls.slice();
      this.objectUrl = null;
      previousUrls.forEach((url) => URL.revokeObjectURL(url));
      this.applyTransform();
      await nextFrame();
      if (sequence !== this.loadSequence) return VIEWER_GROUP_LOAD_SUPERSEDED;

      decodedImages.forEach((decoded, index) => {
        const resource = resources[index];
        const request = pages[index].request;
        recordSuccessfulPageTiming(request, resource, decoded.decodeMs);
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
          css_height: roundMs(resolvedLayout.pages[index].cssHeight),
          device_pixel_ratio: roundMs(request.dpr),
          fit_mode: request.fitMode,
          render_trigger: renderTrigger,
          ...viewerTransformTelemetry(this.scale, currentVisualViewportScale()),
          ...this.layoutTelemetry(),
          prefetch_status: resource.prefetchStatus,
          spread_pages: pages.length,
        };
        enqueueTelemetry(event);
        hudState.lastImage = event;
        hudState.displayDurations.push(event.tap_to_display_ms);
      });
      while (hudState.displayDurations.length > 20) hudState.displayDurations.shift();
      updateHud();
      return VIEWER_GROUP_LOAD_APPLIED;
    } catch (error) {
      pendingUrls.forEach((url) => URL.revokeObjectURL(url));
      const candidateImageIds = resources?.map((resource) => resource.requestId) ?? [];
      if (sequence !== this.loadSequence) {
        this.recordPageDisplay(
          presentation,
          "not_applied",
          "load_sequence_mismatch",
          candidateImageIds
        );
        return VIEWER_GROUP_LOAD_SUPERSEDED;
      }
      const reason = viewerPageDisplayFailureReason(error, phase);
      this.recordPageDisplay(
        presentation,
        "not_applied",
        reason,
        candidateImageIds
      );
      if (error?.name !== "AbortError") {
        this.root.classList.remove("viewer-ui-hidden");
      }
      if (
        error?.name !== "AbortError" &&
        error?.code !== "page_identity_mismatch"
      ) {
        recordClientError("spread_load_error", error);
      }
      return viewerGroupLoadFailure(error, "見開きを表示できませんでした。");
    }
  }

  setPageLoadBusy(busy) {
    const nextBusy = Boolean(busy);
    if (this.destroyed || nextBusy === this.pageLoadBusy) return;
    this.pageLoadBusy = nextBusy;
    if (!nextBusy) {
      clearTimeout(this.loadingTimer);
      this.loadingTimer = 0;
      this.loadingIndicator.hidden = true;
      return;
    }
    const startedAt = performance.now();
    this.loadingIndicator.hidden = true;
    this.loadingTimer = setTimeout(() => {
      this.loadingTimer = 0;
      if (
        !this.destroyed &&
        shouldShowLoadingIndicator(
          this.pageLoadBusy,
          performance.now() - startedAt,
          PAGE_LOADING_INDICATOR_DELAY_MS
        )
      ) {
        this.loadingIndicator.hidden = false;
      }
    }, PAGE_LOADING_INDICATOR_DELAY_MS);
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

  showGroupLoadFailure(message) {
    this.title.textContent = message;
    this.root.classList.remove("viewer-ui-hidden");
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

  startSinglePointer(point, edgeGuarded) {
    return {
      startX: point.x,
      startY: point.y,
      lastX: point.x,
      lastY: point.y,
      startedAt: performance.now(),
      edgeGuarded,
      moved: false,
      dragOwnership: viewerDragOwnershipDecision({
        fitMode: this.fitMode,
        scale: this.scale,
        scrollWidth: this.stage.scrollWidth,
        clientWidth: this.stage.clientWidth,
        scrollHeight: this.stage.scrollHeight,
        clientHeight: this.stage.clientHeight,
      }),
    };
  }

  onPointerDown(event) {
    if (["mouse", "pen"].includes(event.pointerType) && event.button !== 0) return;
    this.stage.setPointerCapture?.(event.pointerId);
    this.pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (this.pointers.size === 1) {
      this.single = this.startSinglePointer(
        { x: event.clientX, y: event.clientY },
        event.clientX <= 32
      );
      this.pinched = false;
    } else if (this.pointers.size === 2) {
      const [first, second] = [...this.pointers.values()];
      // 変換前の中央位置。倍率は中央を動かさないので、今の矩形の中心からパンを引けば出る。
      // ジェスチャ開始時に 1 度だけ測り、移動中は測らない。
      const layer = this.pageLayer.getBoundingClientRect();
      this.pinch = {
        distance: distance(first, second),
        scale: this.scale,
        center: midpoint(first, second),
        panX: this.panX,
        panY: this.panY,
        originX: layer.left + layer.width / 2 - this.panX,
        originY: layer.top + layer.height / 2 - this.panY,
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
        command(CommandName.SET_TRANSFORM, pinchTransformDecision({
          startScale: this.pinch.scale,
          startPanX: this.pinch.panX,
          startPanY: this.pinch.panY,
          startCenterX: this.pinch.center.x,
          startCenterY: this.pinch.center.y,
          originX: this.pinch.originX,
          originY: this.pinch.originY,
          centerX: center.x,
          centerY: center.y,
          ratio,
        })),
        {
          source: pointerInputSource(event.pointerType),
          detail: "pinch_move",
          telemetry: false,
        }
      );
      return;
    }

    const ownership = this.single?.dragOwnership;
    if (ownership?.owner === ViewerDragOwner.TRANSFORM && previous) {
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
    } else if (ownership?.owner === ViewerDragOwner.STAGE && previous) {
      const beforeX = this.stage.scrollLeft;
      const beforeY = this.stage.scrollTop;
      if (ownership.ownsHorizontal) {
        this.stage.scrollLeft -= event.clientX - previous.x;
      }
      if (ownership.ownsVertical) {
        this.stage.scrollTop -= event.clientY - previous.y;
      }
      this.single.lastX = event.clientX;
      this.single.lastY = event.clientY;
      if (
        Math.abs(this.stage.scrollLeft - beforeX) > 0.5 ||
        Math.abs(this.stage.scrollTop - beforeY) > 0.5
      ) {
        this.single.moved = true;
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
      this.single = this.startSinglePointer(remaining, false);
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
        zoomed: single.dragOwnership.owner === ViewerDragOwner.TRANSFORM,
        dragOwnership: single.dragOwnership,
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
        dispatchCommand(viewerTapCommand(
          event.clientX,
          this.root.clientWidth,
          isRtlReadingDirection(state.readingDirection)
        ), {
          source,
          detail: "tap_zone",
        });
      } else if (gesture === ViewerGesture.PAN) {
        dispatchCommand(command(CommandName.PAN_BY, { dx: 0, dy: 0 }), {
          source,
          detail: "pan",
        });
      }
    } else if (this.pinched) {
      enqueueTelemetry({
        type: "viewer_gesture",
        action: "pinch_end",
        cancelled: Boolean(cancelled),
        fit_mode: this.fitMode ?? state.fitMode,
        ...viewerTransformTelemetry(this.scale, currentVisualViewportScale()),
      });
      if (!cancelled) {
        dispatchCommand(
          command(CommandName.SET_TRANSFORM, {
            scale: this.scale,
            panX: this.panX,
            panY: this.panY,
          }),
          { source, detail: "pinch" }
        );
      }
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
    this.destroyed = true;
    this.pageLoadBusy = false;
    clearTimeout(this.resizeTimer);
    clearTimeout(this.loadingTimer);
    clearTimeout(this.boundaryMessageTimer);
    this.loadingIndicator.hidden = true;
    this.boundaryMessage.hidden = true;
    this.pageLoadQueue.clear();
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
    error.serverMessage = typeof detail.message === "string" ? detail.message : "";
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
    error.serverMessage = typeof detail.message === "string" ? detail.message : "";
    error.retryAfterSeconds = Number(response.headers.get("Retry-After")) || 1;
    throw error;
  }
  return response.json();
}

async function apiAddressPostJson(endpoint, body, signal) {
  const request = addressedPostRequest(endpoint, body);
  return apiPostJson(request.url, request.body, signal);
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

function normalizedRemotePath(path) {
  return String(path ?? "").replaceAll("\\", "/");
}

function trimmedRemotePath(path) {
  const normalized = normalizedRemotePath(path);
  if (normalized === "/" || /^[A-Za-z]:\/$/.test(normalized)) return normalized;
  return normalized.replace(/\/+$/, "");
}

function favoriteForPath(path) {
  const candidate = trimmedRemotePath(path).toLocaleLowerCase();
  let matched = null;
  let matchedLength = -1;
  for (const favorite of state.favorites) {
    const root = trimmedRemotePath(favorite.path).toLocaleLowerCase();
    if (
      root &&
      (candidate === root || candidate.startsWith(root.endsWith("/") ? root : root + "/")) &&
      root.length > matchedLength
    ) {
      matched = favorite;
      matchedLength = root.length;
    }
  }
  return matched;
}

function isFavoriteRoot(path) {
  const favorite = favoriteForPath(path);
  return Boolean(
    favorite &&
    trimmedRemotePath(favorite.path).toLocaleLowerCase() ===
      trimmedRemotePath(path).toLocaleLowerCase()
  );
}

function remotePathBreadcrumbs(path) {
  const normalized = trimmedRemotePath(path);
  const favorite = favoriteForPath(normalized);
  if (favorite) {
    const root = trimmedRemotePath(favorite.path);
    const relative = normalized.slice(root.length).replace(/^\/+/, "");
    const crumbs = [{ label: favorite.name, path: root }];
    let accumulated = root;
    for (const segment of relative.split("/").filter(Boolean)) {
      accumulated = accumulated.endsWith("/")
        ? accumulated + segment
        : accumulated + "/" + segment;
      crumbs.push({ label: segment, path: accumulated });
    }
    return crumbs;
  }

  let root;
  let label;
  let relative;
  const drive = normalized.match(/^([A-Za-z]:)(?:\/(.*))?$/);
  const unc = normalized.match(/^\/\/([^/]+)\/([^/]+)(?:\/(.*))?$/);
  if (drive) {
    root = drive[1] + "/";
    label = drive[1] + "\\";
    relative = drive[2] ?? "";
  } else if (unc) {
    root = `//${unc[1]}/${unc[2]}`;
    label = `\\\\${unc[1]}\\${unc[2]}`;
    relative = unc[3] ?? "";
  } else {
    root = "/";
    label = "/";
    relative = normalized.replace(/^\/+/, "");
  }
  const crumbs = [{ label, path: root }];
  let accumulated = root;
  for (const segment of relative.split("/").filter(Boolean)) {
    accumulated = accumulated.endsWith("/")
      ? accumulated + segment
      : accumulated + "/" + segment;
    crumbs.push({ label: segment, path: accumulated });
  }
  return crumbs;
}

function parentPath(path) {
  const normalized = trimmedRemotePath(path);
  if (normalized === "/" || /^[A-Za-z]:\/$/.test(normalized)) return normalized;
  const uncRoot = normalized.match(/^\/\/[^/]+\/[^/]+$/);
  if (uncRoot) return normalized;
  const separator = normalized.lastIndexOf("/");
  if (separator < 0) return normalized;
  if (separator === 0) return "/";
  if (separator === 2 && /^[A-Za-z]:\//.test(normalized)) {
    return normalized.slice(0, 3);
  }
  return normalized.slice(0, separator);
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
    if (state.localSettings.telemetryDebugDetails) {
      openLocalSettingsDialog();
    } else {
      hudElement.hidden = true;
    }
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
        error_event_kind:
          event.error == null && event.message === "Script error."
            ? "opaque_script_error"
            : "exception",
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
  const visualViewport = window.visualViewport;
  visualViewport?.addEventListener?.("resize", () => {
    telemetryState.visualViewportObservedAtMs = performance.now();
    clearTimeout(telemetryState.visualViewportTimer);
    telemetryState.visualViewportTimer = setTimeout(() => {
      telemetryState.visualViewportTimer = 0;
      const transition = visualViewportScaleTransition(
        telemetryState.visualViewportScale,
        visualViewport.scale
      );
      telemetryState.visualViewportScale = transition.nextScale;
      if (transition.event) {
        enqueueTelemetry({
          ...transition.event,
          ...precedingBrowserTapPairTelemetry(
            telemetryState.visualViewportObservedAtMs
          ),
        });
      }
    }, 250);
  }, { passive: true });
  window.setInterval(() => {
    flushTelemetry(false);
    updateHud();
  }, 5000);
}

function currentVisualViewportScale() {
  return normalizeVisualViewportScale(window.visualViewport?.scale);
}

async function observedFetch(url, options = {}, sessionRecoveryAttempted = false) {
  const { sessionEpochBound = false, ...requestOptions } = options;
  const fetchOptions = {
    ...requestOptions,
    headers: remoteHeaders(requestOptions.headers),
  };
  let response;
  try {
    response = await fetch(url, fetchOptions);
  } catch (error) {
    if (error?.name !== "AbortError") {
      recordClientError("fetch_error", error, {
        resource: safeResourcePath(url),
      });
    }
    throw error;
  }
  if (!response.ok) {
    const detail = await response.clone().json().catch(() => ({}));
    if (detail.error === "remote_state_generation_mismatch") {
      applyRemoteStateGeneration(detail.remote_state_generation, { reloadViewer: true });
    }
    if (response.status === 409 || response.status === 428) {
      if (detail.error !== "remote_state_generation_mismatch") {
        const sessionStatus = remoteSessionFailureStatus({
          sessionStatus: detail.status,
          httpStatus: response.status,
          errorCode: detail.error,
        });
        if (sessionStatus) {
          setRemoteSessionStatus(
            sessionStatus,
            detail.message || "操作権がありません。再接続してください。",
            {
              observer: remoteSessionObserverForRequest(url),
              observedStatus: detail.status || sessionStatus,
              httpStatus: response.status,
            }
          );
          if (
            !sessionRecoveryAttempted &&
            state.remoteSessionUserActive &&
            remoteSessionAcquireDecision(sessionStatus, "user_operation") === "acquire"
          ) {
            let recovered = false;
            try {
              recovered = await acquireRemoteSession(
                "expired_operation",
                "user_operation"
              );
            } catch {}
            if (recovered && sessionEpochBound) {
              const viewer = state.viewer;
              queueMicrotask(() => {
                if (viewer && state.viewer === viewer) {
                  updateViewerImage(performance.now()).catch(renderError);
                }
              });
              throw new DOMException("リモートセッションを更新しました。", "AbortError");
            }
            if (recovered) return observedFetch(url, options, true);
          }
        }
      }
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
    : await observedFetch(request.url, { ...options, sessionEpochBound: true });
  if (!response.ok) {
    throw await pageResourceResponseError(response);
  }
  if (response.headers.get("X-mIV-Remote-Session") !== request.remoteSessionId) {
    const error = new Error(
      "ページ画像のリモートセッションを確認できなかったため、表示を中止しました。"
    );
    error.code = "remote_session_unattested";
    throw error;
  }
  requirePageResponseIdentity(request.address, response);
  requirePageResponseGeneration(request, response);
  const width = Number(response.headers.get("X-mIV-Image-Width"));
  const height = Number(response.headers.get("X-mIV-Image-Height"));
  const pageRenderHeader = response.headers.get("X-mIV-Page-Render-Ms");
  const pageRenderMs = pageRenderHeader === null ? null : Number(pageRenderHeader);
  return {
    blob: await response.blob(),
    requestId: response.headers.get("X-mIV-Request-Id"),
    fetchMs: performance.now() - startedAt,
    pageRenderMs:
      pageRenderMs !== null && Number.isFinite(pageRenderMs) && pageRenderMs >= 0
        ? pageRenderMs
        : null,
    info:
      Number.isFinite(width) && width > 0 && Number.isFinite(height) && height > 0
        ? { width, height }
        : null,
  };
}

function requirePageResponseIdentity(requestedAddress, response) {
  const attestation = pageResponseIdentityAttestation(
    requestedAddress,
    response.headers.get("X-mIV-Page-Identity")
  );
  if (attestation.matches) return;
  const error = new Error(
    "要求したページと応答画像の identity が一致しないため、表示を中止しました。"
  );
  error.code = "page_identity_mismatch";
  recordClientError("page_identity_mismatch", error, {
    requested_page_identity: attestation.requestedIdentity,
    response_page_identity: attestation.responseIdentity,
  });
  throw error;
}

function requirePageResponseGeneration(request, response) {
  const generationAttestation = pageResponseGenerationAttestation(
    request.remoteStateGeneration,
    response.headers.get("X-mIV-Remote-State-Generation")
  );
  if (generationAttestation.observed) {
    applyRemoteStateGeneration(generationAttestation.observed, { reloadViewer: true });
  }
  if (!generationAttestation.matches) {
    const error = new Error(
      "ページ画像の状態版を確認できなかったため、古い画像の表示を中止しました。"
    );
    error.code = "remote_state_generation_unattested";
    throw error;
  }
}

async function pageResourceResponseError(response) {
  const detail = await response.clone().json().catch(() => ({}));
  if (detail.error === "remote_state_generation_mismatch") {
    applyRemoteStateGeneration(detail.remote_state_generation, { reloadViewer: true });
  }
  if (
    (response.status === 409 || response.status === 428) &&
    detail.error !== "remote_state_generation_mismatch"
  ) {
    const sessionStatus = remoteSessionFailureStatus({
      sessionStatus: detail.status,
      httpStatus: response.status,
      errorCode: detail.error,
    });
    if (sessionStatus) {
      setRemoteSessionStatus(
        sessionStatus,
        detail.message || "操作権がありません。再接続してください。",
        {
          observer: "page_request",
          observedStatus: detail.status || sessionStatus,
          httpStatus: response.status,
        }
      );
    }
  }
  const error = new Error(
    detail.message || `画像取得に失敗しました (HTTP ${response.status})。`
  );
  error.status = response.status;
  error.code = detail.error;
  const retryAfterSeconds = Number(response.headers.get("Retry-After"));
  if (Number.isFinite(retryAfterSeconds) && retryAfterSeconds > 0) {
    error.retryAfterMs = retryAfterSeconds * 1000;
  }
  return error;
}

function remoteHeaders(initial = {}) {
  const headers = new Headers(initial);
  headers.set("X-mIV-Remote-Client", REMOTE_CLIENT_ID);
  if (state.remoteSessionId) {
    headers.set("X-mIV-Remote-Session", state.remoteSessionId);
  }
  return headers;
}

function remoteSessionObserverForRequest(value) {
  let path = "";
  try {
    path = new URL(value, location.origin).pathname;
  } catch {}
  if (path === "/api/video/state") return "video_poll";
  if (path.startsWith("/api/video/")) return "video_request";
  if (path === "/api/page") return "page_request";
  return "api_request";
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
  const tieredEvent = telemetryEventForTier(event, {
    detailed: state.localSettings.telemetryDebugDetails,
    clientId: REMOTE_CLIENT_ID,
    sessionCorrelation: state.remoteSessionCorrelation,
    sensitiveValues: [state.remoteSessionId],
  });
  const stampedEvent = {
    ...tieredEvent,
    client_event_timestamp_ms: Date.now(),
    client_event_sequence: telemetryState.nextSequence++,
  };
  if (
    telemetryDeliveryMode(stampedEvent) === "immediate" &&
    sendImmediateTelemetry(stampedEvent)
  ) {
    return;
  }
  queueTelemetryEvent(stampedEvent);
}

function queueTelemetryEvent(event, { first = false } = {}) {
  if (first) telemetryState.queue.unshift(event);
  else telemetryState.queue.push(event);
  if (telemetryState.queue.length > 200) {
    if (first) telemetryState.queue.splice(200);
    else telemetryState.queue.splice(0, telemetryState.queue.length - 200);
  }
}

function sendImmediateTelemetry(event) {
  const body = telemetryPayloadBody([event]);
  try {
    if (
      typeof navigator.sendBeacon === "function" &&
      navigator.sendBeacon(
        "/api/telemetry",
        new Blob([body], { type: "application/json" })
      )
    ) {
      return true;
    }
  } catch {}

  // sendBeacon is available on the target mobile browsers. Keepalive fetch is the fallback
  // for restricted/test environments; a failed attempt returns the same stamped event to the
  // normal queue without minting a second sequence number.
  try {
    Promise.resolve(fetch("/api/telemetry", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body,
      keepalive: true,
    })).then((response) => {
      if (!response.ok && response.status !== 429) {
        queueTelemetryEvent(event, { first: true });
        noteHudError();
      }
    }).catch(() => {
      queueTelemetryEvent(event, { first: true });
      noteHudError();
    });
    return true;
  } catch {
    return false;
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
  let body = telemetryPayloadBody(events);
  while (new Blob([body]).size > 60 * 1024 && events.length > 1) {
    telemetryState.queue.unshift(events.pop());
    body = telemetryPayloadBody(events);
  }
  return { events, body };
}

function telemetryPayloadBody(events) {
  const payload = {
    client_timestamp_ms: Date.now(),
    events,
  };
  const connection = connectionInfo();
  if (connection) payload.connection = connection;
  return JSON.stringify(payload);
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
  if (RUNTIME_TEST_MODE && typeof runtimeTestErrorObserver === "function") {
    runtimeTestErrorObserver({ category, error, extra });
  }
  const normalized = normalizeError(error);
  enqueueTelemetry({
    type: "error",
    category,
    error_name: normalized.name,
    message: normalized.message,
    stack: normalized.stack,
    ...extra,
  });
  noteHudError();
}

export function setRuntimeTestErrorObserver(observer) {
  if (!RUNTIME_TEST_MODE) return;
  runtimeTestErrorObserver = typeof observer === "function" ? observer : null;
}

function normalizeError(error) {
  const message =
    error instanceof Error ? error.message : typeof error === "string" ? error : String(error);
  const stack = error instanceof Error ? error.stack : "";
  return {
    name: limitText(redactTokenQuery(error instanceof Error ? error.name : typeof error), 120),
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
  const debugDetails = state.localSettings.telemetryDebugDetails;
  if (debugDetails && state.authenticated) hudElement.hidden = false;
  hudElement.dataset.telemetryTier = debugDetails ? "debug" : "normal";
  trimHudErrors();
  const image = hudState.lastImage;
  const grid = hudState.lastGrid;
  const video = hudState.video;
  hudElement.dataset.viewerKind = video ? "video" : "default";
  const recent = hudState.displayDurations.slice(-7);
  const heading = debugDetails ? "mIV PoC 計測 · 詳細記録 ON" : "mIV PoC 計測";
  const lines = [];
  if (video) {
    const dropped = Number.isFinite(video.dropped_video_frames)
      ? `${video.dropped_video_frames}/${video.total_video_frames ?? "—"}`
      : "—";
    lines.push(
      `動画 pos ${formatHealthSeconds(video.current_time_secs)} · buf ${formatHealthSeconds(video.buffer_ahead_secs)}`
    );
    lines.push(`ready ${video.ready_state} · drop ${dropped}`);
    const transport = [];
    if (Number.isFinite(video.hls_bandwidth_bps)) {
      transport.push(`bw ${(video.hls_bandwidth_bps / 1_000_000).toFixed(1)}Mbps`);
    }
    if (Number.isFinite(video.last_fragment_load_ms)) {
      transport.push(`seg ${formatMs(video.last_fragment_load_ms)}`);
    }
    if (video.connection_effective_type) transport.push(video.connection_effective_type);
    if (Number.isFinite(video.connection_rtt_ms)) {
      transport.push(`rtt ${Math.round(video.connection_rtt_ms)}ms`);
    }
    lines.push(transport.length ? transport.join(" · ") : "bw — · seg —");
  } else {
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
  }
  lines.push(
    `error(60s) ${hudState.errors.length}  · ${debugDetails ? "tapで設定" : "tapで隠す"}`
  );
  const header = element("span", "telemetry-hud-header");
  header.append(textElement("span", heading, "telemetry-hud-heading"));
  const prefetch = video ? null : pagePrefetchHudIndicator();
  if (prefetch) header.append(prefetch);
  hudElement.title = prefetch?.title ?? "";
  hudElement.setAttribute(
    "aria-label",
    [
      heading,
      prefetch?.title,
      debugDetails ? "タップで設定" : "タップで隠す",
    ].filter(Boolean).join("。")
  );
  const body = textElement("span", lines.join("\n"), "telemetry-hud-body");
  hudElement.replaceChildren(header, body);
}

function pagePrefetchHudIndicator() {
  const plan = hudState.pagePrefetch;
  if (!plan) return null;
  const summary = pagePrefetchIndicatorSummary({
    behind: pageResourceCache.statusForKeys(plan.behindKeys),
    ahead: pageResourceCache.statusForKeys(plan.aheadKeys),
  });
  if (!summary.behind.length && !summary.ahead.length) return null;
  const indicator = element("span", "page-prefetch-indicator");
  indicator.title = summary.accessibleLabel;
  indicator.setAttribute("role", "img");
  indicator.setAttribute("aria-label", summary.accessibleLabel);
  const appendDot = (status) => {
    const dot = textElement(
      "span",
      "●",
      `page-prefetch-dot page-prefetch-dot-${status}`
    );
    dot.setAttribute("aria-hidden", "true");
    indicator.append(dot);
  };
  summary.behind.forEach(appendDot);
  indicator.append(textElement("span", "｜", "page-prefetch-divider"));
  summary.ahead.forEach(appendDot);
  return indicator;
}

function formatHealthSeconds(value) {
  return value !== null && value !== undefined && Number.isFinite(Number(value))
    ? `${Number(value).toFixed(1)}s`
    : "—";
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
