import {
  CommandName,
  VIDEO_QUALITY_PRESETS,
  VIEWER_PANEL_ANIMATION_MS,
  ViewerGesture,
  ViewerPanelAction,
  bufferingQualitySuggestion,
  command,
  videoAbsoluteSeekCommand,
  videoHttpStatusDecision,
  videoPlaybackDecision,
  videoQualityPreset,
  videoSeekPlan,
  videoStartSeekTarget,
  videoStartupDecision,
  videoTapCommand,
  shouldReanchorVideoTimeline,
  videoTimelineAnchor,
  videoTimelinePosition,
  viewerGestureDecision,
  viewerPanelGestureAction,
  viewerPanelShellTransition,
} from "./command-core.mjs";

const HLS_MIME = "application/vnd.apple.mpegurl";
const HLS_SCRIPT_PATH = "/vendor/hls.min.js";
const VIDEO_STATE_POLL_MS = 1000;
const WAITING_SUGGESTION_MS = 3000;
const STARTUP_MEDIA_SEGMENT_TIMEOUT_MS = 15000;
const PLAYLIST_RECOVERY_TIMEOUT_MS = 15000;
const PLAYLIST_RECOVERY_BACKOFF_BASE_MS = 250;
const PLAYLIST_RECOVERY_BACKOFF_MAX_MS = 2000;
const SEEK_THUMBNAIL_POLL_MS = 120;
const SEEK_THUMBNAIL_MATCH_TOLERANCE_SECS = 1.25;
const VIDEO_LOOP_BOUNDARY_TOLERANCE_SECS = 0.020;

export const VIDEO_PANEL_TABS = Object.freeze([
  Object.freeze({ id: "functions", label: "機能" }),
  Object.freeze({ id: "jump", label: "ジャンプ" }),
]);

let hlsScriptPromise = null;

export function hlsBufferConfig(bufferTargetSecs) {
  const target = Math.max(1, Number(bufferTargetSecs) || 1);
  return {
    backBufferLength: target,
    maxBufferLength: target,
    maxMaxBufferLength: target,
    startPosition: 0,
    startOnSegmentBoundary: true,
  };
}

export function videoAudioProcessingPresentation(status = {}) {
  const requested = Boolean(status?.vst3_requested);
  const active = Boolean(status?.vst3_active);
  const activeSlots = Math.max(0, Math.floor(Number(status?.vst3_active_slots) || 0));
  const warning = typeof status?.vst3_warning === "string"
    ? status.vst3_warning.trim()
    : "";
  return {
    requested,
    active,
    activeSlots,
    warning,
    detail: active
      ? `VST3 ${activeSlots}`
      : requested
        ? (warning ? "VST3 未適用" : "VST3 バイパス")
        : "",
  };
}

export function preventVideoNativeZoom(event) {
  if (event?.target?.closest?.("button, input, select, textarea, a")) return false;
  if (!event?.cancelable) return false;
  event.preventDefault?.();
  return true;
}

function normalizedVideoLoopBoundaries(behavior) {
  const values = behavior?.kind === "loop" ? behavior.boundary_starts_secs : [];
  const normalized = [...new Set((values ?? [])
    .map(Number)
    .filter((value) => Number.isFinite(value) && value >= 0)
    .sort((left, right) => left - right))];
  return normalized.length ? normalized : [0];
}

function videoLoopInterval(behavior, positionSecs) {
  const position = Math.max(0, Number(positionSecs) || 0);
  const starts = normalizedVideoLoopBoundaries(behavior);
  let start = 0;
  for (const candidate of starts) {
    if (candidate > position) break;
    start = candidate;
  }
  return {
    start,
    end: starts.find((candidate) => candidate > start) ?? null,
  };
}

/// Resolve the terminal/boundary action published by the PC-side settings owner.
/// `ended=false` mirrors the local chapter/bookmark boundary tick; `ended=true` handles EOF.
export function videoEndDecision({
  behavior,
  previousPositionSecs = null,
  positionSecs = 0,
  ended = false,
  playing = true,
  toleranceSecs = VIDEO_LOOP_BOUNDARY_TOLERANCE_SECS,
} = {}) {
  if (behavior?.kind === "next") {
    return ended
      ? { kind: "next", wrap: Boolean(behavior.wrap) }
      : { kind: "continue" };
  }
  if (behavior?.kind !== "loop") {
    return ended ? { kind: "stop" } : { kind: "continue" };
  }
  if (!ended && !playing) return { kind: "continue" };

  const position = Math.max(0, Number(positionSecs) || 0);
  if (ended) {
    const previous = Number(previousPositionSecs);
    const intervalPosition = Number.isFinite(previous) && previous <= position
      ? previous
      : position;
    return {
      kind: "loop",
      positionSecs: videoLoopInterval(behavior, intervalPosition).start,
    };
  }
  const previous = Number(previousPositionSecs);
  if (!Number.isFinite(previous) || position < previous || position <= previous + 1.0e-9) {
    return { kind: "continue" };
  }
  const interval = videoLoopInterval(behavior, previous);
  if (
    interval.end !== null &&
    previous < interval.end &&
    position >= interval.end - Math.max(0, Number(toleranceSecs) || 0)
  ) {
    return { kind: "loop", positionSecs: interval.start };
  }
  return { kind: "continue" };
}

function element(tag, className = "") {
  const node = document.createElement(tag);
  if (className) node.className = className;
  return node;
}

function textElement(tag, text, className = "") {
  const node = element(tag, className);
  node.textContent = text;
  return node;
}

function loadHlsJs() {
  if (globalThis.Hls) return Promise.resolve(globalThis.Hls);
  if (hlsScriptPromise) return hlsScriptPromise;
  hlsScriptPromise = new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = HLS_SCRIPT_PATH;
    script.async = true;
    script.addEventListener("load", () => {
      if (globalThis.Hls) resolve(globalThis.Hls);
      else reject(new Error("hls.js を初期化できませんでした。"));
    }, { once: true });
    script.addEventListener("error", () => {
      hlsScriptPromise = null;
      reject(new Error("hls.js を読み込めませんでした。"));
    }, { once: true });
    document.head.append(script);
  });
  return hlsScriptPromise;
}

function retryAfterSeconds(response) {
  const parsed = Number(response?.headers?.get?.("Retry-After"));
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 1;
}

function hlsHttpStatus(data) {
  return Number(
    data?.response?.code ??
    data?.networkDetails?.status ??
    data?.networkDetails?.statusCode ??
    0
  ) || 0;
}

function hlsRetryAfter(data) {
  const header = data?.networkDetails?.getResponseHeader?.("Retry-After");
  const parsed = Number(header);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 1;
}

function hlsHttpErrorCode(data) {
  let body = data?.response?.data;
  if (body instanceof Uint8Array) body = new TextDecoder().decode(body);
  if (typeof body === "string") {
    try { body = JSON.parse(body); } catch { return ""; }
  }
  return typeof body?.error === "string" ? body.error : "";
}

function formatVideoTime(value) {
  const seconds = Math.max(0, Math.floor(Number(value) || 0));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remainder = seconds % 60;
  return hours
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(remainder).padStart(2, "0")}`
    : `${minutes}:${String(remainder).padStart(2, "0")}`;
}

function seekableRanges(media) {
  const ranges = [];
  for (let index = 0; index < media.seekable.length; index += 1) {
    ranges.push([media.seekable.start(index), media.seekable.end(index)]);
  }
  return ranges;
}

function abortableDelay(delayMs, signal) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(resolve, Math.max(0, Number(delayMs) || 0));
    if (!signal) return;
    const aborted = () => {
      clearTimeout(timer);
      const error = new Error("aborted");
      error.name = "AbortError";
      reject(error);
    };
    if (signal.aborted) aborted();
    else signal.addEventListener("abort", aborted, { once: true });
  });
}

export function videoUserErrorMessage(error, fallback = "動画を操作できませんでした") {
  const code = String(error?.code ?? "");
  if (code.startsWith("stream_start_")) {
    return "動画を開始できませんでした。もう一度お試しください。";
  }
  if (code === "stream_session_mismatch" || code === "stream_generation_mismatch") {
    return "動画の配信が終了しました。もう一度開いてください。";
  }
  if (code === "stream_not_ready" ||
      code === "stream_busy" ||
      code === "stream_resource_timeout") {
    return "動画を準備しています。しばらくしてからもう一度お試しください。";
  }
  if (code === "stream_not_found" ||
      code === "stream_favorite_not_found" ||
      code === "stream_path_rejected") {
    return "動画が見つかりませんでした。";
  }
  if (code === "stream_unsupported") {
    return "この動画は再生できません。";
  }
  const summary = String(fallback || "動画を操作できませんでした").replace(/。+$/, "");
  return summary + "。もう一度お試しください。";
}

export class VideoSeekPreviewOwner {
  constructor({ matchToleranceSecs = SEEK_THUMBNAIL_MATCH_TOLERANCE_SECS } = {}) {
    this.matchToleranceSecs = Math.max(0, Number(matchToleranceSecs) || 0);
    this.revision = 0;
    this.state = { kind: "playback" };
  }

  request(targetSecs, label = "シーク中") {
    this.revision += 1;
    this.state = {
      kind: "seeking",
      revision: this.revision,
      targetSecs: Math.max(0, Number(targetSecs) || 0),
      label: String(label),
    };
    return { ...this.state };
  }

  requestRelative(playbackPositionSecs, deltaSecs, durationSecs, label = "シーク中") {
    const basePositionSecs = this.displayedPosition(playbackPositionSecs);
    const delta = Number(deltaSecs);
    const duration = Number(durationSecs);
    const targetSecs = Math.max(
      0,
      basePositionSecs + (Number.isFinite(delta) ? delta : 0)
    );
    return this.request(
      Number.isFinite(duration) && duration > 0
        ? Math.min(duration, targetSecs)
        : targetSecs,
      label
    );
  }

  acceptThumbnail(request, { actualPtsSecs, objectUrl, width = 0, height = 0 }) {
    const actual = Number(actualPtsSecs);
    if (
      this.state.kind === "playback" ||
      request?.revision !== this.state.revision ||
      !Number.isFinite(actual) ||
      Math.abs(actual - this.state.targetSecs) > this.matchToleranceSecs
    ) {
      return false;
    }
    this.state = {
      ...this.state,
      kind: "thumbnail",
      actualPtsSecs: actual,
      objectUrl: String(objectUrl || ""),
      width: Math.max(0, Number(width) || 0),
      height: Math.max(0, Number(height) || 0),
    };
    return true;
  }

  bindGeneration(request, generation) {
    const expectedGeneration = Number(generation);
    if (
      this.state.kind === "playback" ||
      request?.revision !== this.state.revision ||
      !Number.isFinite(expectedGeneration)
    ) {
      return false;
    }
    this.state = { ...this.state, expectedGeneration };
    return true;
  }

  playbackGenerationStarted(generation) {
    const attachedGeneration = Number(generation);
    if (
      this.state.kind === "playback" ||
      !Number.isFinite(attachedGeneration) ||
      attachedGeneration !== this.state.expectedGeneration
    ) {
      return false;
    }
    return this.playbackStarted({ revision: this.state.revision });
  }

  playbackStarted(request = null) {
    if (request !== null && request?.revision !== this.state.revision) return false;
    const wasActive = this.state.kind !== "playback";
    this.revision += 1;
    this.state = { kind: "playback" };
    return wasActive;
  }

  requestFailed(request) {
    return this.playbackStarted(request);
  }

  current() {
    return { ...this.state };
  }

  displayedPosition(playbackPositionSecs) {
    return this.state.kind === "playback"
      ? Math.max(0, Number(playbackPositionSecs) || 0)
      : this.state.targetSecs;
  }
}

export function createPlaylistRecoveryBudget({
  startedAtMs = Date.now(),
  timeoutMs = PLAYLIST_RECOVERY_TIMEOUT_MS,
} = {}) {
  const started = Number(startedAtMs) || 0;
  const timeout = Math.max(1, Number(timeoutMs) || 1);
  return Object.freeze({
    startedAtMs: started,
    deadlineMs: started + timeout,
    timeoutMs: timeout,
  });
}

function playlistRecoveryRemainingMs(budget, nowMs) {
  return Math.max(0, Number(budget?.deadlineMs) - Number(nowMs));
}

function playlistRecoveryDelayMs(attempt, retryDelayMs) {
  const exponent = Math.min(3, Math.max(0, Number(attempt) - 1));
  const backoff = Math.min(
    PLAYLIST_RECOVERY_BACKOFF_MAX_MS,
    PLAYLIST_RECOVERY_BACKOFF_BASE_MS * 2 ** exponent
  );
  return Math.max(backoff, Math.max(0, Number(retryDelayMs) || 0));
}

function playlistRecoveryExhausted(budget, attempts, nowMs) {
  return {
    ok: false,
    attempts,
    decision: {
      kind: "playlist_recovery_exhausted",
      retry: false,
      retryDelayMs: 0,
      message: "動画の再生を続けられませんでした。",
      internalReason: "playlist_recovery_budget_exhausted",
      timeoutMs: Number(budget.timeoutMs) || PLAYLIST_RECOVERY_TIMEOUT_MS,
      elapsedMs: Math.max(0, Number(nowMs) - Number(budget.startedAtMs)),
    },
  };
}

export async function resolveVideoPlaylist({
  initialUrl,
  session,
  fetchPlaylist,
  fetchState,
  signal,
  delay = abortableDelay,
  now = () => Date.now(),
  timeoutMs = PLAYLIST_RECOVERY_TIMEOUT_MS,
  budget,
  onDecision = () => {},
  onTarget = () => {},
  onAttempt = () => {},
}) {
  let url = String(initialUrl ?? "");
  let latestState = null;
  const recoveryBudget = budget ?? createPlaylistRecoveryBudget({
    startedAtMs: now(),
    timeoutMs,
  });
  let attempts = 0;
  while (playlistRecoveryRemainingMs(recoveryBudget, now()) > 0) {
    attempts += 1;
    onAttempt(attempts, url);
    const response = await fetchPlaylist(url, signal);
    const responseAt = now();
    if (playlistRecoveryRemainingMs(recoveryBudget, responseAt) <= 0) {
      return playlistRecoveryExhausted(recoveryBudget, attempts, responseAt);
    }
    if (response.ok) {
      return { ok: true, url, state: latestState, attempts };
    }
    const detail = await response.clone().json().catch(() => ({}));
    const decision = videoHttpStatusDecision(
      response.status,
      retryAfterSeconds(response),
      detail.error
    );
    onDecision(decision, detail);
    if (decision.kind === "generation_mismatch") {
      latestState = await fetchState(session, signal);
      const currentSession = Number(latestState?.session ?? session);
      const generation = Number(latestState?.generation);
      if (!Number.isFinite(currentSession) || !Number.isFinite(generation)) {
        return {
          ok: false,
          attempts,
          decision: {
            kind: "error",
            retry: false,
            retryDelayMs: 0,
            message: "動画を読み込めませんでした。もう一度お試しください。",
            internalReason: "playlist_generation_state_invalid",
          },
        };
      }
      url = `/stream/${currentSession}/${generation}/index.m3u8`;
      onTarget({
        session: currentSession,
        generation,
        url,
        state: latestState,
      });
    } else if (decision.kind !== "waiting") {
      return { ok: false, decision, attempts };
    }

    const remainingMs = playlistRecoveryRemainingMs(recoveryBudget, now());
    if (remainingMs <= 0) {
      return playlistRecoveryExhausted(recoveryBudget, attempts, now());
    }
    const retryDelayMs = playlistRecoveryDelayMs(attempts, decision.retryDelayMs);
    await delay(Math.min(retryDelayMs, remainingMs), signal);
  }
  return playlistRecoveryExhausted(recoveryBudget, attempts, now());
}

function normalizePlaylistTarget(target = {}) {
  const session = Number(target.session);
  const generation = target.generation === null || target.generation === undefined
    ? null
    : Number(target.generation);
  return {
    session: Number.isFinite(session) ? session : null,
    generation: Number.isFinite(generation) ? generation : null,
    url: String(target.url ?? ""),
  };
}

function targetsShareSession(left, right) {
  return left.session !== null && left.session === right.session;
}

export class VideoGenerationSwitchOwner {
  constructor({
    stopCurrent,
    runSwitch,
    onBudgetExhausted = () => {},
    now = () => Date.now(),
    timeoutMs = PLAYLIST_RECOVERY_TIMEOUT_MS,
    setTimer = (callback, delayMs) => setTimeout(callback, delayMs),
    clearTimer = (timer) => clearTimeout(timer),
  }) {
    this.stopCurrent = stopCurrent;
    this.runSwitch = runSwitch;
    this.onBudgetExhausted = onBudgetExhausted;
    this.now = now;
    this.timeoutMs = timeoutMs;
    this.setTimer = setTimer;
    this.clearTimer = clearTimer;
    this.state = { kind: "idle" };
  }

  isSwitching() {
    return this.state.kind === "switching";
  }

  currentTarget() {
    if (this.state.kind === "idle") return null;
    const target = this.state.kind === "switching"
      ? this.state.operation.target
      : this.state.target;
    return { ...target };
  }

  attachedTarget() {
    return this.state.kind === "attached" ? { ...this.state.target } : null;
  }

  request(target, { force = false, initialRetryDelayMs = 0 } = {}) {
    const requested = normalizePlaylistTarget(target);
    if (this.state.kind === "switching") {
      const active = this.state.operation;
      if (this.shouldJoin(active.target, requested)) {
        if (active.target.generation === null && requested.generation !== null) {
          active.updateTarget(requested);
        }
        return active.promise;
      }
      active.abortReason = "superseded";
      this.clearTimer(active.deadlineTimer);
      active.controller.abort();
    } else if (
      !force &&
      this.state.kind === "attached" &&
      targetsShareSession(this.state.target, requested) &&
      requested.generation !== null &&
      this.state.target.generation !== null &&
      requested.generation <= this.state.target.generation
    ) {
      return Promise.resolve(true);
    }

    const controller = new AbortController();
    const operation = {
      target: requested,
      controller,
      signal: controller.signal,
      budget: createPlaylistRecoveryBudget({
        startedAtMs: this.now(),
        timeoutMs: this.timeoutMs,
      }),
      promise: null,
      abortReason: "",
      attempts: 0,
      initialRetryDelayMs: Math.max(0, Number(initialRetryDelayMs) || 0),
      deadlineTimer: 0,
      owns: () => (
        this.state.kind === "switching" &&
        this.state.operation === operation
      ),
      isCurrent: () => (
        operation.owns() &&
        !operation.signal.aborted
      ),
      updateTarget: (nextTarget) => {
        const next = normalizePlaylistTarget(nextTarget);
        if (
          targetsShareSession(operation.target, next) &&
          operation.target.generation !== null &&
          next.generation !== null &&
          next.generation < operation.target.generation
        ) {
          return;
        }
        operation.target = next;
      },
    };
    this.state = { kind: "switching", operation };
    operation.promise = (async () => {
      await Promise.resolve();
      try {
        const attached = Boolean(await this.runSwitch(operation));
        if (operation.isCurrent()) {
          this.state = attached
            ? { kind: "attached", target: { ...operation.target } }
            : { kind: "idle" };
        }
        return attached;
      } catch (error) {
        if (operation.abortReason === "budget" && operation.owns()) {
          this.onBudgetExhausted(operation);
          this.state = { kind: "idle" };
          return false;
        }
        if (operation.owns()) this.state = { kind: "idle" };
        if (operation.signal.aborted) return false;
        throw error;
      } finally {
        this.clearTimer(operation.deadlineTimer);
      }
    })();
    operation.deadlineTimer = this.setTimer(() => {
      if (!operation.owns()) return;
      operation.abortReason = "budget";
      operation.controller.abort();
    }, operation.budget.timeoutMs);
    this.stopCurrent(requested);
    return operation.promise;
  }

  shouldJoin(active, requested) {
    if (!targetsShareSession(active, requested)) return false;
    if (requested.generation === null) return true;
    if (active.generation === null) return true;
    return requested.generation <= active.generation;
  }

  cancel() {
    if (this.state.kind === "switching") {
      this.state.operation.abortReason = "cancelled";
      this.clearTimer(this.state.operation.deadlineTimer);
      this.state.operation.controller.abort();
    }
    this.state = { kind: "idle" };
  }
}

class VideoStreamMenu {
  constructor(
    host,
    dispatch,
    inputSource,
    mediaState,
    {
      getSelectedTab = () => "functions",
      setSelectedTab = () => {},
      loadJumpList = async () => ({ sections: [] }),
    } = {}
  ) {
    this.host = host;
    this.dispatch = dispatch;
    this.inputSource = inputSource;
    this.mediaState = mediaState;
    this.getSelectedTab = getSelectedTab;
    this.setSelectedTab = setSelectedTab;
    this.loadJumpList = loadJumpList;
    this.opened = false;
    this.previousFocus = null;
    this.keyboardElements = [];
    this.panelState = null;
    this.panelPointer = null;
    this.panelMotionTimer = 0;
    this.panelMotionListener = null;
    this.suppressPanelClick = false;
    this.tabButtons = new Map();
    this.streamSession = null;
    this.jumpLoadSession = null;
    this.jumpLoadState = "idle";
    this.jumpObserver = null;
    this.jumpThumbnailLoader = null;
    this.root = element("div", "command-menu-layer viewer-command-menu-layer");
    this.root.hidden = true;

    const scrim = element("button", "command-menu-scrim");
    scrim.type = "button";
    scrim.setAttribute("aria-label", "操作メニューを閉じる");
    scrim.addEventListener("click", () => this.close());

    this.panel = element("section", "command-menu viewer-command-menu");
    this.panel.setAttribute("role", "dialog");
    this.panel.setAttribute("aria-modal", "true");
    const header = element("header", "command-menu-header");
    this.title = textElement("h2", "動画の操作");
    this.closeButton = textElement("button", "×", "command-menu-close");
    this.closeButton.type = "button";
    this.closeButton.setAttribute("aria-label", "操作メニューを閉じる");
    this.closeButton.addEventListener("click", (event) => {
      this.send(event, CommandName.TOGGLE_MENU);
    });
    header.append(this.title, this.closeButton);
    this.actions = element("div", "command-menu-actions");
    this.actions.setAttribute("role", "menu");
    this.shortcutTitle = textElement("h3", "有効なキー", "command-shortcut-title");
    this.shortcuts = element("dl", "command-shortcuts");
    const tabs = element("nav", "viewer-panel-tabs");
    tabs.setAttribute("role", "tablist");
    tabs.setAttribute("aria-label", "動画パネルのタブ");
    for (const tab of VIDEO_PANEL_TABS) {
      const button = textElement("button", tab.label, "viewer-panel-tab");
      button.type = "button";
      button.setAttribute("role", "tab");
      button.addEventListener("click", (event) => {
        event.stopPropagation();
        this.selectTab(tab.id);
      });
      this.tabButtons.set(tab.id, button);
      tabs.append(button);
    }
    this.tabs = tabs;
    this.panelBody = element("div", "viewer-command-menu-body");
    this.functionsPanel = element("section", "video-functions-panel");
    this.functionsPanel.append(this.actions, this.shortcutTitle, this.shortcuts);
    this.jumpPanel = element("section", "video-jump-panel");
    this.jumpPanel.hidden = true;
    this.panelBody.append(this.functionsPanel, this.jumpPanel);
    this.panel.append(header, tabs, this.panelBody);
    this.root.append(scrim, this.panel);
    host.append(this.root);
    this.panelPointerDown = (event) => this.onPanelPointerDown(event);
    this.panelPointerUp = (event) => this.onPanelPointerUp(event, false);
    this.panelPointerCancel = (event) => this.onPanelPointerUp(event, true);
    this.panelClickCapture = (event) => {
      if (!this.suppressPanelClick) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      this.suppressPanelClick = false;
    };
    this.viewportResize = () => this.updatePanelLayout();
    this.root.addEventListener("pointerdown", this.panelPointerDown);
    this.root.addEventListener("pointerup", this.panelPointerUp);
    this.root.addEventListener("pointercancel", this.panelPointerCancel);
    this.root.addEventListener("click", this.panelClickCapture, true);
    window.addEventListener("resize", this.viewportResize);
    this.updatePanelLayout();
    this.selectTab(this.getSelectedTab());
  }

  send(event, name, payload = {}) {
    this.dispatch(command(name, payload), {
      source: this.inputSource(event),
      detail: "menu",
    });
  }

  definition(page) {
    if (page === "controls") {
      return {
        title: "音量と画質",
        actions: [
          ["menu_back", "動画の操作へ戻る", "戻る"],
          [CommandName.MEDIA_TOGGLE_PLAY, "再生 / 一時停止", "Space"],
          ...VIDEO_QUALITY_PRESETS.map((preset) => [
            CommandName.MEDIA_QUALITY,
            `${preset.label} — ${preset.traffic}`,
            "画質",
            { quality: preset.id },
          ]),
        ],
        shortcuts: [
          ["再生 / 一時停止", "Space"],
          ["10 秒戻る / 進む", "← / →"],
        ],
        volume: true,
      };
    }
    const media = this.mediaState();
    return {
      title: "動画の操作",
      actions: [
        [CommandName.MEDIA_TOGGLE_PLAY, "再生 / 一時停止", "Space"],
        ["menu_controls", "音量と画質", "通信量の目安を表示"],
        [
          CommandName.TOGGLE_VIEWER_BARS,
          media.barsVisible === false ? "上下バーを表示" : "上下バーを隠す",
          "メニュー",
        ],
        [CommandName.TOGGLE_FULLSCREEN, "全画面表示", "F11"],
        [CommandName.BACK, "一覧へ戻る", "Backspace / Enter / Esc"],
        [CommandName.OPEN_LOCAL_SETTINGS, "端末の設定", "メニュー"],
        [CommandName.RELOAD_APP, "再読み込み", "メニュー"],
      ],
      shortcuts: [
        ["再生 / 一時停止", "Space"],
        ["10 秒戻る / 進む", "← / →"],
        ["前 / 次の動画", "↑ / ↓"],
        ["操作メニュー", "?"],
        ["全画面", "F11"],
      ],
    };
  }

  selectTab(tabId) {
    const tab = VIDEO_PANEL_TABS.find((candidate) => candidate.id === tabId) ??
      VIDEO_PANEL_TABS[0];
    this.setSelectedTab(tab.id);
    this.syncTabButtons(tab.id);
    if (tab.id === "functions") {
      this.showPage("main");
      return;
    }
    this.page = `tab:${tab.id}`;
    this.title.textContent = tab.label;
    this.panel.setAttribute("aria-label", tab.label);
    this.functionsPanel.hidden = true;
    this.jumpPanel.hidden = false;
    this.ensureJumpList();
  }

  syncTabButtons(selectedId) {
    for (const [id, button] of this.tabButtons) {
      const selected = id === selectedId;
      button.classList.toggle("is-selected", selected);
      button.setAttribute("aria-selected", String(selected));
      button.tabIndex = selected ? 0 : -1;
    }
  }

  showPage(page) {
    this.setSelectedTab("functions");
    this.syncTabButtons("functions");
    this.functionsPanel.hidden = false;
    this.jumpPanel.hidden = true;
    this.page = page;
    const definition = this.definition(page);
    this.title.textContent = definition.title;
    this.panel.setAttribute("aria-label", definition.title);
    this.actions.replaceChildren();
    this.shortcuts.replaceChildren();
    this.keyboardElements = [this.shortcutTitle, this.shortcuts];
    this.qualityButtons = new Map();
    this.actionLabels = new Map();
    for (const [name, label, hint, payload = {}] of definition.actions) {
      const button = element("button", "command-menu-action");
      button.type = "button";
      button.setAttribute("role", "menuitem");
      const actionLabel = textElement("span", label);
      button.append(actionLabel, textElement("kbd", hint));
      if (!this.actionLabels.has(name)) this.actionLabels.set(name, actionLabel);
      if (name === CommandName.MEDIA_QUALITY) {
        this.qualityButtons.set(payload.quality, button);
      }
      button.addEventListener("click", (event) => {
        if (name === "menu_back") return this.showPage("main");
        if (name === "menu_controls") return this.showPage("controls");
        this.close(false);
        this.send(event, name, payload);
      });
      this.actions.append(button);
      this.keyboardElements.push(button.lastElementChild);
    }
    if (definition.volume) this.renderVolume();
    for (const [label, keys] of definition.shortcuts) {
      this.shortcuts.append(textElement("dt", label), textElement("dd", keys));
    }
    this.setMediaState(this.mediaState());
  }

  renderVolume() {
    const media = this.mediaState();
    const field = element("label", "video-menu-volume");
    const row = element("span", "video-menu-volume-label");
    const output = textElement("output", `${Math.round((media.volume ?? 1) * 100)}%`);
    row.append(textElement("span", "音量"), output);
    const input = element("input");
    input.type = "range";
    input.min = "0";
    input.max = "1";
    input.step = "0.01";
    input.value = String(media.volume ?? 1);
    input.setAttribute("aria-label", "音量");
    input.addEventListener("input", () => {
      output.value = `${Math.round(Number(input.value) * 100)}%`;
      output.textContent = output.value;
    });
    input.addEventListener("change", (event) => {
      this.send(event, CommandName.MEDIA_VOLUME, { volume: Number(input.value) });
    });
    field.append(row, input);
    this.actions.append(field);
  }

  setMediaState(media = {}) {
    for (const [quality, button] of this.qualityButtons ?? []) {
      const selected = quality === media.quality;
      button.classList.toggle("is-current", selected);
      button.setAttribute("aria-pressed", String(selected));
    }
  }

  setActionLabel(name, label) {
    const target = this.actionLabels?.get(name);
    if (target) target.textContent = label;
  }

  setKeyboardAvailable(available) {
    for (const target of this.keyboardElements) target.hidden = !available;
  }

  setSession(session) {
    const normalized = Number.isSafeInteger(Number(session)) && Number(session) > 0
      ? Number(session)
      : null;
    if (normalized === this.streamSession) return;
    this.streamSession = normalized;
    this.resetJumpList();
    if (this.getSelectedTab() === "jump") this.ensureJumpList();
  }

  resetJumpList() {
    this.disconnectJumpObserver();
    this.jumpLoadSession = null;
    this.jumpLoadState = "idle";
    this.jumpPanel.replaceChildren();
  }

  ensureJumpList() {
    const session = this.streamSession;
    if (!session) {
      this.renderJumpStatus("動画の準備完了後に一覧を表示します。");
      return;
    }
    if (this.jumpLoadSession === session && ["loading", "loaded"].includes(this.jumpLoadState)) {
      return;
    }
    this.jumpLoadSession = session;
    this.jumpLoadState = "loading";
    this.renderJumpStatus("ジャンプ一覧を読み込んでいます…");
    Promise.resolve(this.loadJumpList(session))
      .then((payload) => {
        if (this.streamSession !== session || this.jumpLoadSession !== session) return;
        this.jumpLoadState = "loaded";
        this.renderJumpList(payload);
      })
      .catch((error) => {
        if (this.streamSession !== session || this.jumpLoadSession !== session) return;
        if (error?.name === "AbortError") return;
        this.jumpLoadState = "error";
        this.renderJumpStatus(
          error instanceof Error ? error.message : "ジャンプ一覧を取得できませんでした。"
        );
      });
  }

  renderJumpStatus(message) {
    this.disconnectJumpObserver();
    const status = textElement("p", message, "video-jump-status");
    this.jumpPanel.replaceChildren(status);
  }

  renderJumpList(payload) {
    this.disconnectJumpObserver();
    const fragment = document.createDocumentFragment();
    let rowCount = 0;
    for (const section of payload?.sections ?? []) {
      const rows = [];
      for (const entry of section?.entries ?? []) {
        const position = Number(entry?.position_secs);
        if (!Number.isFinite(position) || position < 0) continue;
        const row = element("button", "video-jump-row");
        row.type = "button";
        const thumbnail = element("span", "video-jump-thumbnail");
        if (typeof entry.thumbnail_url === "string" && entry.thumbnail_url) {
          const image = element("img");
          image.alt = "";
          image.dataset.src = entry.thumbnail_url;
          thumbnail.append(image);
        } else {
          thumbnail.classList.add("is-empty");
        }
        const copy = element("span", "video-jump-copy");
        copy.append(
          textElement("span", String(entry.display_time ?? ""), "video-jump-time"),
          textElement("span", String(entry.title ?? ""), "video-jump-title")
        );
        row.append(thumbnail, copy);
        row.addEventListener("click", (event) => {
          event.stopPropagation();
          this.dispatch(videoAbsoluteSeekCommand(position), {
            source: this.inputSource(event),
            detail: "jump_list",
          });
        });
        rows.push(row);
        rowCount += 1;
      }
      if (!rows.length) continue;
      const group = element("section", "video-jump-section");
      group.dataset.kind = String(section.kind ?? "");
      group.append(textElement("h3", String(section.label ?? ""), "video-jump-section-title"), ...rows);
      fragment.append(group);
    }
    if (!rowCount) {
      this.jumpPanel.replaceChildren(
        textElement("p", "ピン・ブックマーク・チャプターはまだありません", "video-jump-status")
      );
      return;
    }
    this.jumpPanel.replaceChildren(fragment);
    this.observeJumpThumbnails();
  }

  observeJumpThumbnails() {
    const images = [...this.jumpPanel.querySelectorAll("img[data-src]")];
    if (!images.length) return;
    const loader = {
      active: 0,
      cancelled: false,
      queue: [],
    };
    this.jumpThumbnailLoader = loader;
    const drain = () => {
      if (loader.cancelled) return;
      while (loader.active < 2 && loader.queue.length) {
        const image = loader.queue.shift();
        if (!image?.dataset.src) continue;
        loader.active += 1;
        let finished = false;
        const finish = (failed = false) => {
          if (finished) return;
          finished = true;
          loader.active -= 1;
          if (failed) {
            image.parentElement?.classList.add("is-empty");
            image.remove();
          }
          drain();
        };
        image.addEventListener("load", () => finish(), { once: true });
        image.addEventListener("error", () => finish(true), { once: true });
        image.src = image.dataset.src;
        delete image.dataset.src;
      }
    };
    const enqueue = (image) => {
      if (!image?.dataset.src || loader.queue.includes(image)) return;
      loader.queue.push(image);
      drain();
    };
    if (typeof IntersectionObserver !== "function") {
      for (const image of images) enqueue(image);
      return;
    }
    this.jumpObserver = new IntersectionObserver((entries, observer) => {
      for (const observed of entries) {
        if (!observed.isIntersecting) continue;
        const image = observed.target;
        observer.unobserve(image);
        enqueue(image);
      }
    }, { root: this.panelBody });
    for (const image of images) this.jumpObserver.observe(image);
  }

  disconnectJumpObserver() {
    this.jumpObserver?.disconnect();
    this.jumpObserver = null;
    if (this.jumpThumbnailLoader) {
      this.jumpThumbnailLoader.cancelled = true;
      this.jumpThumbnailLoader.queue.length = 0;
      this.jumpThumbnailLoader = null;
    }
  }

  isOpen() { return this.opened; }

  toggle() {
    if (this.opened) this.close();
    else this.open();
    return true;
  }

  open() {
    if (this.opened) return;
    this.cancelPanelMotion();
    this.selectTab(this.getSelectedTab());
    this.opened = true;
    this.previousFocus = document.activeElement;
    this.root.hidden = false;
    this.host.classList.add("menu-open");
    this.updatePanelLayout(ViewerPanelAction.OPEN);
    this.startPanelMotion("opening");
    requestAnimationFrame(() => this.closeButton.focus());
  }

  close(restoreFocus = true, animate = true) {
    if (!this.opened) return;
    this.opened = false;
    if (animate) {
      this.startPanelMotion("closing", () => this.finishClose(restoreFocus));
      return;
    }
    this.cancelPanelMotion();
    this.finishClose(restoreFocus);
  }

  finishClose(restoreFocus) {
    this.updatePanelLayout(ViewerPanelAction.CLOSE);
    this.root.hidden = true;
    this.host.classList.remove("menu-open");
    if (restoreFocus && this.previousFocus instanceof HTMLElement) {
      this.previousFocus.focus({ preventScroll: true });
    }
  }

  startPanelMotion(motion, onFinished = () => {}) {
    this.cancelPanelMotion();
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
    this.panelMotionTimer = setTimeout(finish, VIEWER_PANEL_ANIMATION_MS + 80);
  }

  cancelPanelMotion() {
    if (this.panelMotionListener) {
      this.panel.removeEventListener("animationend", this.panelMotionListener);
    }
    clearTimeout(this.panelMotionTimer);
    this.panelMotionTimer = 0;
    this.panelMotionListener = null;
    delete this.root.dataset.motion;
  }

  updatePanelLayout(action = "resize") {
    const next = viewerPanelShellTransition(this.panelState, {
      action,
      viewportWidth: window.innerWidth,
      viewportHeight: window.innerHeight,
    });
    this.panelState = next;
    this.root.dataset.orientation = next.orientation;
    this.host.classList.toggle("viewer-panel-open", next.open);
    this.host.classList.toggle("viewer-panel-portrait", next.orientation === "portrait");
    this.host.classList.toggle("viewer-panel-landscape", next.orientation === "landscape");
  }

  onPanelPointerDown(event) {
    if (event.isPrimary === false) return;
    if (["mouse", "pen"].includes(event.pointerType) && event.button !== 0) return;
    this.panelPointer = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      startedAt: performance.now(),
      scrollTop: this.panelBody.scrollTop,
    };
  }

  onPanelPointerUp(event, cancelled) {
    const pointer = this.panelPointer;
    if (!pointer || pointer.pointerId !== event.pointerId) return;
    this.panelPointer = null;
    const contentScrolled = Math.abs(this.panelBody.scrollTop - pointer.scrollTop) > 0.5;
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

  destroy() {
    this.opened = false;
    this.cancelPanelMotion();
    this.updatePanelLayout(ViewerPanelAction.CLOSE);
    this.root.hidden = true;
    this.host.classList.remove("menu-open");
    this.root.removeEventListener("pointerdown", this.panelPointerDown);
    this.root.removeEventListener("pointerup", this.panelPointerUp);
    this.root.removeEventListener("pointercancel", this.panelPointerCancel);
    this.root.removeEventListener("click", this.panelClickCapture, true);
    window.removeEventListener("resize", this.viewportResize);
    this.disconnectJumpObserver();
    this.root.remove();
  }
}

export class VideoStreamViewer {
  constructor({
    entry,
    address,
    dispatch,
    inputSource,
    apiJson,
    apiPostJson,
    subscribeRemoteSessionState = () => () => {},
    reportPlaybackIssue = () => {},
    keyboardAvailable = true,
    getPanelTab = () => "functions",
    setPanelTab = () => {},
  }) {
    this.isVideoStreamViewer = true;
    this.entry = entry;
    this.address = address;
    this.dispatch = dispatch;
    this.inputSource = inputSource;
    this.apiJson = apiJson;
    this.apiPostJson = apiPostJson;
    this.reportPlaybackIssue = reportPlaybackIssue;
    this.quality = "standard";
    this.volume = 1;
    this.duration = 0;
    this.bufferTargetSecs = null;
    this.session = null;
    this.generation = null;
    this.encoder = "";
    this.codecs = "";
    this.audioProcessing = videoAudioProcessingPresentation();
    this.lastAnnouncedAudioWarning = "";
    this.lastState = null;
    this.timelineAnchor = { sourcePositionSecs: 0, mediaTimeSecs: 0 };
    this.timelineAnchorGeneration = null;
    this.playRequested = true;
    this.barsVisible = true;
    this.destroyed = false;
    this.restarting = false;
    this.draggingSeek = false;
    this.hls = null;
    this.pollTimer = 0;
    this.waitingTimer = 0;
    this.waitingSince = null;
    this.startupWatch = null;
    this.noticeKind = "";
    this.abortController = new AbortController();
    this.pendingTimers = new Set();
    this.pointers = new Map();
    this.singlePointer = null;
    this.seekPreviewOwner = new VideoSeekPreviewOwner();
    this.seekThumbnailAbort = null;
    this.seekThumbnailObjectUrl = "";
    this.seekThumbnailClear = Promise.resolve();
    this.endBehavior = { kind: "stop" };
    this.endPositionSample = null;
    this.endTransitionPending = false;
    this.remoteSessionState = { blocksInteraction: false };
    this.remoteSessionResume = null;
    this.unsubscribeRemoteSessionState = () => {};

    this.root = element("section", "image-viewer video-stream-viewer");
    this.stage = element("div", "viewer-stage video-stream-stage");
    this.video = element("video", "stream-video");
    this.video.playsInline = true;
    this.video.preload = "auto";
    this.video.setAttribute("playsinline", "");
    this.video.setAttribute("webkit-playsinline", "");
    this.video.setAttribute("aria-label", entry.name);
    this.seekPreview = element("div", "video-seek-preview");
    this.seekPreview.hidden = true;
    this.seekPreview.setAttribute("role", "status");
    this.seekPreview.setAttribute("aria-live", "polite");
    this.seekPreviewImage = element("img", "video-seek-preview-image");
    this.seekPreviewImage.alt = "";
    this.seekPreviewImage.hidden = true;
    this.seekPreviewLabel = textElement("div", "シーク中", "video-seek-preview-label");
    this.seekPreview.append(this.seekPreviewImage, this.seekPreviewLabel);
    this.generationSwitch = new VideoGenerationSwitchOwner({
      stopCurrent: () => this.stopPlaylistPlayback(),
      runSwitch: (operation) => this.performGenerationSwitch(operation),
      onBudgetExhausted: (operation) => this.handleGenerationSwitchFailure(
        playlistRecoveryExhausted(
          operation.budget,
          operation.attempts,
          performance.now()
        ),
        operation
      ),
      now: () => performance.now(),
    });
    // Native controls intentionally remain disabled. Every operation is dispatched
    // through the same command layer as touch and keyboard input.

    this.notice = element("div", "video-stream-notice");
    this.notice.hidden = true;
    this.notice.setAttribute("role", "status");
    this.notice.setAttribute("aria-live", "polite");
    this.stage.append(this.video, this.seekPreview, this.notice);

    const top = element("div", "viewer-ui top");
    const close = textElement("button", "×", "viewer-button");
    close.type = "button";
    close.setAttribute("aria-label", "一覧へ戻る");
    this.title = textElement("div", entry.name, "viewer-title");
    const menuTrigger = textElement("button", "☰", "viewer-button menu-trigger");
    menuTrigger.type = "button";
    menuTrigger.setAttribute("aria-label", "操作メニュー");
    menuTrigger.setAttribute("aria-haspopup", "dialog");
    top.append(close, this.title, menuTrigger);

    const bottom = element("div", "viewer-ui bottom video-stream-bottom");
    const previous = textElement("button", "‹", "viewer-button");
    previous.type = "button";
    previous.setAttribute("aria-label", "前の動画");
    const seek = element("div", "viewer-seek video-stream-seek");
    this.counter = textElement("output", "0:00 / 0:00", "viewer-counter");
    this.seekInput = element("input", "viewer-seek-input");
    this.seekInput.type = "range";
    this.seekInput.min = "0";
    this.seekInput.max = "0";
    this.seekInput.step = "0.1";
    this.seekInput.value = "0";
    this.seekInput.setAttribute("aria-label", "動画の再生位置");
    this.diagnostics = textElement("div", "配信を開始しています…", "video-stream-diagnostics");
    seek.append(this.counter, this.seekInput, this.diagnostics);
    const next = textElement("button", "›", "viewer-button");
    next.type = "button";
    next.setAttribute("aria-label", "次の動画");
    bottom.append(previous, seek, next);
    this.root.append(this.stage, top, bottom);

    this.menu = new VideoStreamMenu(
      this.root,
      dispatch,
      inputSource,
      () => this.menuState(),
      {
        getSelectedTab: getPanelTab,
        setSelectedTab: setPanelTab,
        loadJumpList: (session) => this.apiJson(
          "/api/video/jumps",
          { session },
          this.abortController.signal
        ),
      }
    );
    this.menu.setKeyboardAvailable(keyboardAvailable);

    const send = (event, requested, detail) => {
      event.stopPropagation();
      dispatch(requested, { source: inputSource(event), detail });
    };
    close.addEventListener("click", (event) => {
      send(event, command(CommandName.BACK), "toolbar");
    });
    menuTrigger.addEventListener("click", (event) => {
      send(event, command(CommandName.TOGGLE_MENU), "toolbar");
    });
    previous.addEventListener("click", (event) => {
      send(event, command(CommandName.PREV_PAGE), "toolbar");
    });
    next.addEventListener("click", (event) => {
      send(event, command(CommandName.NEXT_PAGE), "toolbar");
    });
    this.seekInput.addEventListener("input", () => {
      this.draggingSeek = true;
      const target = Number(this.seekInput.value);
      this.updateCounter(target);
      this.beginSeekPreview(target, "移動先を確認中");
    });
    this.seekInput.addEventListener("change", (event) => {
      const target = Number(this.seekInput.value);
      this.draggingSeek = false;
      send(
        event,
        videoAbsoluteSeekCommand(target),
        "seek_bar"
      );
    });

    this.onTimeUpdate = () => {
      this.checkPlaybackStartupProgress();
      this.updateProgress();
      this.handlePlaybackBoundary(false);
    };
    this.onPlaying = () => {
      this.checkPlaybackStartupProgress();
      this.clearWaiting();
      this.finishSeekPreviewForAttachedGeneration();
    };
    this.onCanPlay = () => {
      this.checkPlaybackStartupProgress();
      this.playIfRequested();
    };
    this.onLoadedData = () => {
      this.checkPlaybackStartupProgress();
      if (!this.playRequested) this.finishSeekPreviewForAttachedGeneration();
    };
    this.onWaiting = () => this.beginWaiting();
    this.onEnded = () => this.handlePlaybackBoundary(true);
    this.onNativeError = () => {
      if (
        !this.destroyed &&
        !this.remoteSessionState.blocksInteraction &&
        !this.hls &&
        !this.generationSwitch.isSwitching()
      ) {
        const startup = this.startupWatch;
        this.clearPlaybackStartupWatch();
        this.recordPlaybackIssue(
          "video_stream_native_playback_error",
          "native_hls_media_error",
          {
            playback_mode: startup?.mode ?? "native",
            ready_state: Number(this.video.readyState) || 0,
            network_state: Number(this.video.networkState) || 0,
            media_error_code: Number(this.video.error?.code) || 0,
          }
        );
        this.showNotice(
          "再生データを読み込めません。現在位置から再接続できます。",
          "error",
          "再接続",
          () => this.restartAt(this.currentPosition())
        );
      }
    };
    this.video.addEventListener("timeupdate", this.onTimeUpdate);
    this.video.addEventListener("playing", this.onPlaying);
    this.video.addEventListener("canplay", this.onCanPlay);
    this.video.addEventListener("loadeddata", this.onLoadedData);
    this.video.addEventListener("waiting", this.onWaiting);
    this.video.addEventListener("ended", this.onEnded);
    this.video.addEventListener("error", this.onNativeError);

    this.pointerDown = (event) => this.onPointerDown(event);
    this.pointerMove = (event) => this.onPointerMove(event);
    this.pointerUp = (event) => this.onPointerUp(event, false);
    this.pointerCancel = (event) => this.onPointerUp(event, true);
    // The video surface deliberately owns repeated taps (relative seek) and has no zoom mode.
    // Suppress WebKit's native double-tap/pinch page zoom while leaving controls native.
    this.nativeGesture = (event) => preventVideoNativeZoom(event);
    this.stage.addEventListener("pointerdown", this.pointerDown);
    this.stage.addEventListener("pointermove", this.pointerMove);
    this.stage.addEventListener("pointerup", this.pointerUp);
    this.stage.addEventListener("pointercancel", this.pointerCancel);
    this.stage.addEventListener("touchend", this.nativeGesture, { passive: false });
    this.stage.addEventListener("gesturestart", this.nativeGesture, { passive: false });
    this.stage.addEventListener("gesturechange", this.nativeGesture, { passive: false });
    this.stage.addEventListener("gestureend", this.nativeGesture, { passive: false });
    this.stage.addEventListener("dblclick", this.nativeGesture);
    this.unsubscribeRemoteSessionState = subscribeRemoteSessionState(
      (snapshot) => this.applyRemoteSessionState(snapshot)
    );
  }

  applyRemoteSessionState(snapshot) {
    const wasBlocked = this.remoteSessionState.blocksInteraction;
    this.remoteSessionState = snapshot ?? { blocksInteraction: false };
    if (!this.remoteSessionState.blocksInteraction || this.destroyed) return;
    if (wasBlocked) return;
    this.remoteSessionResume = {
      positionSecs: this.currentPosition(),
      restorePlaying: this.playRequested,
    };
    this.clearPoll();
    this.generationSwitch.cancel();
    this.seekThumbnailAbort?.abort();
    this.clearWaiting();
    this.stopPlaylistPlayback();
  }

  async resumeAfterRemoteSessionReconnect() {
    const resume = this.remoteSessionResume;
    if (!resume || this.destroyed || this.remoteSessionState.blocksInteraction) return false;
    this.remoteSessionResume = null;
    await this.restartAt(resume.positionSecs, resume.restorePlaying);
    return true;
  }

  menuState() {
    return {
      quality: this.quality,
      volume: this.volume,
      playing: !this.video.paused,
      barsVisible: this.barsVisible,
    };
  }

  setBarsVisible(visible) {
    this.barsVisible = Boolean(visible);
    this.root.classList.toggle("viewer-bars-hidden", !visible);
  }

  async start(positionSecs = null, restorePlaying = true) {
    this.playRequested = restorePlaying;
    this.clearPoll();
    this.showNotice("動画を準備しています。", "waiting");
    let started;
    try {
      // Start is a state-creating POST. A retryable 503 can be returned before the core admits
      // it, but an HTTP timeout cannot prove that a prior attempt created no stream. End this
      // attempt visibly and let the user retry instead of keeping the viewer in an unbounded
      // "preparing" loop.
      started = await this.apiPostJson(
        "/api/video/start",
        {
          fav: this.address.favorite_id,
          path: this.address.relative_path,
          quality: this.quality,
        },
        this.abortController.signal
      );
    } catch (error) {
      if (error?.name === "AbortError" || this.destroyed) return;
      this.showStartFailure(error);
      return;
    }
    if (this.destroyed) return;
    this.session = started.session;
    this.menu.setSession(this.session);
    this.generation = started.generation;
    let playlistUrl = started.playlist;
    this.duration = Math.max(0, Number(started.duration_secs) || 0);
    this.bufferTargetSecs = hlsBufferConfig(started.buffer_target_secs).maxBufferLength;
    this.encoder = String(started.encoder ?? "");
    this.codecs = String(started.codec ?? "");
    this.updateAudioProcessing(started.audio_processing, false);
    this.endBehavior = started.end_behavior ?? { kind: "stop" };
    this.endPositionSample = null;
    this.endTransitionPending = false;
    this.seekInput.max = String(this.duration);
    this.timelineAnchor = {
      sourcePositionSecs: Math.max(0, Number(started.source_origin_secs) || 0),
      mediaTimeSecs: 0,
    };
    this.timelineAnchorGeneration = this.generation;
    this.updateDiagnostics(started);

    const startSeekTarget = videoStartSeekTarget({
      requestedPositionSecs: positionSecs,
      sourceOriginSecs: started.source_origin_secs,
      durationSecs: this.duration,
    });
    if (startSeekTarget !== null) {
      try {
        const sought = await this.requestWithWaiting(() => this.apiPostJson(
          "/api/video/seek",
          { session: this.session, position_secs: startSeekTarget },
          this.abortController.signal
        ));
        this.generation = sought.generation;
        playlistUrl = sought.playlist;
        this.timelineAnchor.sourcePositionSecs = startSeekTarget;
        this.timelineAnchorGeneration = this.generation;
      } catch (error) {
        if (error?.name === "AbortError" || this.destroyed) return;
        this.showOperationalError(error, "同じ位置から再開できませんでした");
        return;
      }
    }

    const attached = await this.switchGeneration({
      session: this.session,
      generation: this.generation,
      url: playlistUrl,
    });
    if (!attached || this.destroyed) return;
    this.announceAudioProcessingWarning();
    if (!restorePlaying) {
      await this.setPlaying(false);
    } else {
      this.playIfRequested();
    }
    this.schedulePoll(0);
  }

  async requestWithWaiting(operation) {
    while (!this.destroyed) {
      try {
        return await operation();
      } catch (error) {
        if (error?.name === "AbortError") throw error;
        const decision = videoHttpStatusDecision(
          error?.status,
          error?.retryAfterSeconds,
          error?.code
        );
        if (decision.kind !== "waiting") throw error;
        this.showNotice(decision.message, "waiting");
        await abortableDelay(decision.retryDelayMs, this.abortController.signal);
      }
    }
    const error = new Error("aborted");
    error.name = "AbortError";
    throw error;
  }

  switchGeneration(target, options) {
    if (this.destroyed) return Promise.resolve(false);
    return this.generationSwitch.request(target, options);
  }

  switchToCurrentGeneration() {
    return this.switchGeneration({
      session: this.session,
      generation: null,
      url: "",
    });
  }

  async performGenerationSwitch(operation) {
    if (operation.initialRetryDelayMs > 0) {
      const remainingMs = playlistRecoveryRemainingMs(
        operation.budget,
        performance.now()
      );
      await abortableDelay(
        Math.min(operation.initialRetryDelayMs, remainingMs),
        operation.signal
      );
      if (!operation.isCurrent()) return false;
    }
    let target = { ...operation.target };
    if (target.generation === null || !target.url) {
      const beforeState = performance.now();
      if (playlistRecoveryRemainingMs(operation.budget, beforeState) <= 0) {
        const exhausted = playlistRecoveryExhausted(operation.budget, 0, beforeState);
        this.handleGenerationSwitchFailure(exhausted, operation);
        return false;
      }
      let mediaState;
      try {
        mediaState = await this.apiJson(
          "/api/video/state",
          { session: this.session },
          operation.signal
        );
      } catch (error) {
        if (
          operation.isCurrent() &&
          videoHttpStatusDecision(
            error?.status,
            error?.retryAfterSeconds,
            error?.code
          ).kind === "session_mismatch"
        ) {
          await this.restartAt(this.currentPosition());
          return false;
        }
        throw error;
      }
      if (!operation.isCurrent()) return false;
      const afterState = performance.now();
      if (playlistRecoveryRemainingMs(operation.budget, afterState) <= 0) {
        const exhausted = playlistRecoveryExhausted(operation.budget, 0, afterState);
        this.handleGenerationSwitchFailure(exhausted, operation);
        return false;
      }
      const currentSession = Number(mediaState?.session ?? this.session);
      const generation = Number(mediaState?.generation);
      if (!Number.isFinite(currentSession) || !Number.isFinite(generation)) {
        this.showStatusDecision({
          kind: "error",
          message: "動画を読み込めませんでした。もう一度お試しください。",
        });
        return false;
      }
      operation.updateTarget({
        session: currentSession,
        generation,
        url: `/stream/${currentSession}/${generation}/index.m3u8`,
      });
      target = { ...operation.target };
      if (!this.serverStatePrecedesTarget(mediaState, target)) {
        this.applyServerState(mediaState);
      }
    }

    const result = await this.probePlaylist(target.url, operation);
    if (!operation.isCurrent()) return false;
    if (!result.ok) {
      this.handleGenerationSwitchFailure(result, operation);
      return false;
    }
    if (result.state) {
      if (!this.serverStatePrecedesTarget(result.state, operation.target)) {
        this.applyServerState(result.state);
      }
    }
    operation.updateTarget({
      ...operation.target,
      url: result.url,
    });
    const attached = await this.attachResolvedPlaylist(result.url, operation);
    if (attached && operation.isCurrent()) this.playIfRequested();
    return attached;
  }

  handleGenerationSwitchFailure(result, operation) {
    if (!operation.owns()) return;
    if (result.decision?.kind === "playlist_recovery_exhausted") {
      this.recordPlaybackIssue(
        "video_stream_generation_switch_failed",
        result.decision.internalReason,
        {
          timeout_ms: result.decision.timeoutMs,
          elapsed_ms: Math.round(result.decision.elapsedMs),
          attempts: result.attempts,
          target_generation: operation.target.generation,
        }
      );
    }
    this.showStatusDecision(result.decision);
  }

  async probePlaylist(url, operation) {
    const result = await resolveVideoPlaylist({
      initialUrl: url,
      session: this.session,
      signal: operation.signal,
      budget: operation.budget,
      now: () => performance.now(),
      fetchPlaylist: (playlistUrl, signal) => fetch(playlistUrl, {
        credentials: "same-origin",
        cache: "no-store",
        headers: { Accept: HLS_MIME },
        signal,
      }),
      fetchState: (session, signal) => this.apiJson(
        "/api/video/state",
        { session },
        signal
      ),
      onDecision: (decision) => {
        if (operation.isCurrent() && decision.retry) {
          this.showNotice(decision.message, "waiting");
        }
      },
      onTarget: (target) => operation.updateTarget(target),
      onAttempt: (attempt) => { operation.attempts = attempt; },
    });
    return result;
  }

  async attachResolvedPlaylist(url, operation) {
    if (!url || this.destroyed || !operation.isCurrent()) return false;
    const capabilities = {
      nativeHlsCanPlayType: this.video.canPlayType(HLS_MIME),
      mediaSourceSupported: typeof globalThis.MediaSource === "function",
      managedMediaSourceSupported:
        typeof globalThis.ManagedMediaSource === "function",
    };
    let Hls = null;
    if (capabilities.mediaSourceSupported || capabilities.managedMediaSourceSupported) {
      try {
        Hls = await loadHlsJs();
      } catch (error) {
        this.recordPlaybackIssue(
          "video_stream_hls_js_load_failed",
          "hls_js_script_load_failed",
          { load_error_message: String(error?.message ?? error) }
        );
      }
    }
    if (this.destroyed || !operation.isCurrent()) return false;
    const hlsJsSupported = Boolean(Hls?.isSupported?.());
    const playback = videoPlaybackDecision({ ...capabilities, hlsJsSupported });
    if (playback.mode === "unsupported") {
      this.recordPlaybackIssue(
        "video_stream_playback_unsupported",
        playback.reason,
        {
          native_hls_can_play_type: capabilities.nativeHlsCanPlayType,
          media_source_supported: capabilities.mediaSourceSupported,
          managed_media_source_supported: capabilities.managedMediaSourceSupported,
          hls_js_supported: hlsJsSupported,
        }
      );
      this.showNotice("このブラウザでは動画を再生できません。", "error");
      return false;
    }
    if (playback.mode === "native") {
      this.beginPlaybackStartupWatch("native");
      this.video.src = url;
      this.video.load();
      return true;
    }

    if (this.destroyed) return false;
    const hls = new Hls({
      ...hlsBufferConfig(this.bufferTargetSecs),
      manifestLoadingMaxRetry: 0,
      levelLoadingMaxRetry: 0,
      fragLoadingMaxRetry: 0,
      xhrSetup(xhr) { xhr.withCredentials = true; },
    });
    this.hls = hls;
    hls.on(Hls.Events.ERROR, (_event, data) => this.onHlsError(hls, data));
    hls.on(Hls.Events.FRAG_LOADED, (_event, data) => {
      if (data?.frag && data.frag.sn !== "initSegment") {
        this.markPlaybackMediaSegmentLoaded();
      }
    });
    this.beginPlaybackStartupWatch("hls_js");
    hls.loadSource(url);
    hls.attachMedia(this.video);
    return true;
  }

  onHlsError(source, data) {
    if (this.destroyed || this.hls !== source) return;
    const status = hlsHttpStatus(data);
    if (status) {
      const decision = videoHttpStatusDecision(
        status,
        hlsRetryAfter(data),
        hlsHttpErrorCode(data)
      );
      if (decision.kind === "waiting") {
        this.showNotice(decision.message, "waiting");
        const target = this.generationSwitch.currentTarget();
        if (target) {
          this.switchGeneration(target, {
            force: true,
            initialRetryDelayMs: decision.retryDelayMs,
          }).catch((error) => {
            this.showOperationalError(error, "動画の再生を再開できませんでした");
          });
        }
        return;
      }
      if (decision.kind === "generation_mismatch") {
        this.showNotice(decision.message, "waiting");
        this.switchToCurrentGeneration().catch((error) => {
          this.showOperationalError(error, "動画の再生を再開できませんでした");
        });
        return;
      }
      if (decision.kind === "gone" || decision.kind === "not_found") {
        this.clearPlaybackStartupWatch();
        this.hls?.stopLoad();
        this.video.pause();
        this.showStatusDecision(decision);
        return;
      }
    }
    if (!data?.fatal) return;
    this.clearPlaybackStartupWatch();
    this.hls?.stopLoad();
    this.showNotice(
      "動画を再生できませんでした。もう一度お試しください。",
      "error",
      "再接続",
      () => this.restartAt(this.currentPosition())
    );
  }

  showStatusDecision(decision) {
    if (decision.kind === "gone") {
      this.showNotice(
        decision.message,
        "gone",
        "現在位置から再接続",
        () => this.restartAt(this.currentPosition())
      );
      return;
    }
    if (decision.kind === "playlist_recovery_exhausted") {
      this.showNotice(
        decision.message,
        "error",
        "再接続",
        () => this.restartAt(this.currentPosition())
      );
      return;
    }
    this.showNotice(decision.message, "error");
  }

  async refreshGeneration() {
    if (!this.session || this.destroyed) return;
    await this.switchToCurrentGeneration();
  }

  execute(requested) {
    if (requested.name === CommandName.MEDIA_TOGGLE_PLAY) {
      this.togglePlaying().catch((error) => {
        this.showOperationalError(error, "再生状態を変更できませんでした");
      });
      return true;
    }
    if (requested.name === CommandName.MEDIA_SEEK_RELATIVE) {
      const request = this.beginRelativeSeekPreview(
        Number(requested.payload.seconds || 0),
        "シーク中"
      );
      if (!request) return true;
      this.seekTo(request.targetSecs, request)
        .catch((error) => this.showOperationalError(error, "再生位置を変更できませんでした"));
      return true;
    }
    if (requested.name === CommandName.MEDIA_SEEK_ABSOLUTE) {
      this.seekTo(Number(requested.payload.positionSecs) || 0)
        .catch((error) => this.showOperationalError(error, "再生位置を変更できませんでした"));
      return true;
    }
    if (requested.name === CommandName.MEDIA_VOLUME) {
      this.setVolume(requested.payload.volume).catch((error) => {
        this.showOperationalError(error, "音量を変更できませんでした");
      });
      return true;
    }
    if (requested.name === CommandName.MEDIA_QUALITY) {
      this.setQuality(requested.payload.quality).catch((error) => {
        this.showOperationalError(error, "画質を変更できませんでした");
      });
      return true;
    }
    return false;
  }

  async togglePlaying() {
    const shouldPlay = this.video.paused || !this.lastState?.play_intent;
    await this.setPlaying(shouldPlay);
  }

  async setPlaying(playing) {
    this.playRequested = Boolean(playing);
    if (this.session && Boolean(this.lastState?.play_intent) !== this.playRequested) {
      await this.apiPostJson(
        "/api/video/control",
        { session: this.session, action: this.playRequested ? "play" : "pause" },
        this.abortController.signal
      );
    }
    if (this.lastState) this.lastState.play_intent = this.playRequested;
    if (this.playRequested) await this.playIfRequested();
    else this.video.pause();
  }

  handlePlaybackBoundary(ended) {
    if (this.destroyed || this.endTransitionPending) return;
    const position = this.currentPosition();
    const previous = this.endPositionSample?.generation === this.generation
      ? this.endPositionSample.positionSecs
      : null;
    this.endPositionSample = {
      generation: this.generation,
      positionSecs: position,
    };
    const decision = videoEndDecision({
      behavior: this.endBehavior,
      previousPositionSecs: previous,
      positionSecs: position,
      ended,
      playing: this.playRequested && !this.video.paused,
    });
    if (decision.kind === "continue") return;
    this.endTransitionPending = true;

    if (decision.kind === "stop") {
      this.setPlaying(false).catch(() => {});
      this.updateProgress();
      return;
    }
    if (decision.kind === "next") {
      // app.js either opens the target through the existing start route, or stops this ended
      // viewer when there is no target. Do not publish a pause before a successful transition:
      // that would race the next stream's autoplay intent.
      this.dispatch(command(CommandName.NEXT_PAGE, { wrap: decision.wrap }), {
        source: "media",
        detail: "video_ended",
        telemetry: false,
      });
      return;
    }

    this.playRequested = true;
    this.seekTo(decision.positionSecs)
      .then(() => this.playIfRequested())
      .catch((error) => this.showOperationalError(error, "ループ再生を続けられませんでした"))
      .finally(() => {
        this.endPositionSample = null;
        this.endTransitionPending = false;
      });
  }

  async playIfRequested() {
    if (
      !this.playRequested ||
      this.destroyed ||
      this.remoteSessionState.blocksInteraction ||
      !this.video.src && !this.hls
    ) return;
    try {
      await this.video.play();
      if (["autoplay", "waiting"].includes(this.noticeKind)) this.hideNotice();
    } catch {
      this.showNotice("中央をタップして再生してください。", "autoplay");
    }
  }

  currentPosition() {
    return videoTimelinePosition({
      anchorSourcePositionSecs: this.timelineAnchor.sourcePositionSecs,
      anchorMediaTimeSecs: this.timelineAnchor.mediaTimeSecs,
      mediaCurrentTimeSecs: this.video.currentTime,
      durationSecs: this.duration,
    });
  }

  beginSeekPreview(targetSecs, label = "シーク中") {
    if (!this.session || this.destroyed) return null;
    const request = this.seekPreviewOwner.request(targetSecs, label);
    return this.activateSeekPreview(request);
  }

  beginRelativeSeekPreview(deltaSecs, label = "シーク中") {
    if (!this.session || this.destroyed) return null;
    const request = this.seekPreviewOwner.requestRelative(
      this.currentPosition(),
      deltaSecs,
      this.duration,
      label
    );
    return this.activateSeekPreview(request);
  }

  activateSeekPreview(request) {
    this.seekThumbnailAbort?.abort();
    this.seekThumbnailAbort = new AbortController();
    this.clearSeekThumbnailObjectUrl();
    this.renderSeekPreview();
    this.updateProgress();
    const controller = this.seekThumbnailAbort;
    Promise.resolve(this.seekThumbnailClear)
      .then(() => this.pollSeekThumbnail(request, controller))
      .catch((error) => {
        if (error?.name !== "AbortError" && !this.destroyed) {
          this.recordPlaybackIssue(
            "video_seek_thumbnail_failed",
            "seek_thumbnail_request_failed",
            { error_message: String(error?.message ?? error) }
          );
        }
      });
    return request;
  }

  async pollSeekThumbnail(request, controller) {
    while (!this.destroyed && !controller.signal.aborted) {
      if (this.seekPreviewOwner.current().revision !== request.revision) return;
      const response = await fetch("/api/video/thumbnail", {
        method: "POST",
        credentials: "same-origin",
        cache: "no-store",
        headers: {
          Accept: "image/webp, application/json",
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          session: this.session,
          position_secs: request.targetSecs,
        }),
        signal: controller.signal,
      });
      if (response.status === 202) {
        await abortableDelay(SEEK_THUMBNAIL_POLL_MS, controller.signal);
        continue;
      }
      if (!response.ok) {
        const error = new Error(`seek thumbnail HTTP ${response.status}`);
        error.status = response.status;
        throw error;
      }
      const actualPtsSecs = Number(response.headers.get("X-mIV-Video-Thumbnail-PTS"));
      const width = Number(response.headers.get("X-mIV-Video-Thumbnail-Width"));
      const height = Number(response.headers.get("X-mIV-Video-Thumbnail-Height"));
      const objectUrl = URL.createObjectURL(await response.blob());
      if (this.seekPreviewOwner.acceptThumbnail(
        request,
        { actualPtsSecs, objectUrl, width, height }
      )) {
        this.seekThumbnailObjectUrl = objectUrl;
        this.renderSeekPreview();
        return;
      }
      URL.revokeObjectURL(objectUrl);
      await abortableDelay(SEEK_THUMBNAIL_POLL_MS, controller.signal);
    }
  }

  renderSeekPreview() {
    const preview = this.seekPreviewOwner.current();
    if (preview.kind === "playback") {
      this.seekPreview.hidden = true;
      this.seekPreviewImage.hidden = true;
      this.seekPreviewImage.removeAttribute("src");
      return;
    }
    this.seekPreview.hidden = false;
    if (preview.kind === "thumbnail") {
      this.seekPreviewImage.src = preview.objectUrl;
      this.seekPreviewImage.hidden = false;
      this.seekPreviewLabel.textContent = `移動先 ${formatVideoTime(preview.actualPtsSecs)}`;
    } else {
      this.seekPreviewImage.hidden = true;
      this.seekPreviewImage.removeAttribute("src");
      this.seekPreviewLabel.textContent = preview.label;
    }
  }

  clearSeekThumbnailObjectUrl() {
    if (this.seekThumbnailObjectUrl) {
      URL.revokeObjectURL(this.seekThumbnailObjectUrl);
      this.seekThumbnailObjectUrl = "";
    }
  }

  finishSeekPreview(request = null) {
    this.releaseSeekPreview(this.seekPreviewOwner.playbackStarted(request));
  }

  finishSeekPreviewForAttachedGeneration() {
    const generation = this.generationSwitch.attachedTarget()?.generation;
    this.releaseSeekPreview(
      this.seekPreviewOwner.playbackGenerationStarted(generation)
    );
  }

  cancelSeekPreview(request) {
    this.releaseSeekPreview(this.seekPreviewOwner.requestFailed(request));
  }

  releaseSeekPreview(released) {
    if (!released) return;
    this.seekThumbnailAbort?.abort();
    this.seekThumbnailAbort = null;
    this.clearSeekThumbnailObjectUrl();
    this.renderSeekPreview();
    this.updateProgress();
    const session = this.session;
    if (!session || this.destroyed) return;
    this.seekThumbnailClear = fetch("/api/video/thumbnail", {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
      headers: {
        Accept: "application/json",
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ session, position_secs: null }),
    }).catch(() => {});
  }

  async seekTo(targetPositionSecs, previewRequest = null) {
    if (!this.session || this.destroyed) return;
    // Browser equivalent of the PC player's seek-serial baseline reset: a paused/manual
    // seek across a chapter boundary is not normal playback crossing that boundary.
    this.endPositionSample = null;
    const plan = videoSeekPlan({
      targetPositionSecs,
      durationSecs: this.duration,
      anchorSourcePositionSecs: this.timelineAnchor.sourcePositionSecs,
      anchorMediaTimeSecs: this.timelineAnchor.mediaTimeSecs,
      seekableRanges: seekableRanges(this.video),
    });
    if (plan.kind === "local") {
      this.video.currentTime = plan.mediaTimeSecs;
      this.finishSeekPreview(previewRequest);
      this.updateProgress();
      return;
    }
    const request = previewRequest ?? this.beginSeekPreview(
      plan.positionSecs,
      "シーク中"
    );
    if (!request) return;
    if (this.noticeKind === "waiting" || this.noticeKind === "buffering") this.hideNotice();
    let sought;
    try {
      sought = await this.requestWithWaiting(() => this.apiPostJson(
        "/api/video/seek",
        { session: this.session, position_secs: plan.positionSecs },
        this.abortController.signal
      ));
    } catch (error) {
      this.cancelSeekPreview(request);
      if (error?.status === 409) {
        await this.restartAt(plan.positionSecs);
        return;
      }
      throw error;
    }
    this.generation = sought.generation;
    this.seekPreviewOwner.bindGeneration(request, sought.generation);
    this.timelineAnchor = {
      sourcePositionSecs: plan.positionSecs,
      mediaTimeSecs: 0,
    };
    this.timelineAnchorGeneration = sought.generation;
    try {
      const attached = await this.switchGeneration({
        session: this.session,
        generation: sought.generation,
        url: sought.playlist,
      });
      if (!attached) this.cancelSeekPreview(request);
    } catch (error) {
      this.cancelSeekPreview(request);
      throw error;
    }
  }

  async setVolume(requestedVolume) {
    if (!this.session || this.destroyed) return;
    const volume = Math.max(0, Math.min(1, Number(requestedVolume) || 0));
    await this.apiPostJson(
      "/api/video/control",
      { session: this.session, action: "volume", volume },
      this.abortController.signal
    );
    this.volume = volume;
    this.video.volume = volume;
    if (this.lastState) this.lastState.volume = volume;
    this.menu.setMediaState(this.menuState());
  }

  async setQuality(quality) {
    const preset = videoQualityPreset(quality);
    if (!this.session || this.destroyed || preset.id !== quality || quality === this.quality) return;
    this.showNotice(`${preset.label}画質へ切り替えています。`, "waiting");
    const positionSecs = this.currentPosition();
    try {
      await this.apiPostJson(
        "/api/video/control",
        {
          session: this.session,
          action: "quality",
          quality,
          position_secs: positionSecs,
        },
        this.abortController.signal
      );
    } catch (error) {
      if (error?.status === 409) {
        this.quality = quality;
        await this.restartAt(this.currentPosition());
        return;
      }
      throw error;
    }
    this.quality = quality;
    this.menu.setMediaState(this.menuState());
    await this.refreshGeneration();
  }

  applyServerState(mediaState) {
    this.lastState = mediaState;
    this.generation = mediaState.generation;
    this.duration = Math.max(0, Number(mediaState.duration_secs) || this.duration);
    this.bufferTargetSecs = Math.max(
      1,
      Number(mediaState.buffer_target_secs) || this.bufferTargetSecs
    );
    this.volume = Math.max(0, Math.min(1, Number(mediaState.volume) || 0));
    this.video.volume = this.volume;
    this.encoder = String(mediaState.encoder ?? this.encoder);
    this.codecs = String(mediaState.codecs ?? this.codecs);
    this.updateAudioProcessing(mediaState.audio_processing, true);
    this.playRequested = Boolean(mediaState.play_intent);
    this.seekInput.max = String(this.duration);
    if (
      shouldReanchorVideoTimeline({
        anchoredGeneration: this.timelineAnchorGeneration,
        stateGeneration: mediaState.generation,
      })
    ) {
      this.timelineAnchor = videoTimelineAnchor({
        sourceOriginSecs: mediaState.source_origin_secs,
        durationSecs: this.duration,
      });
      this.timelineAnchorGeneration = mediaState.generation;
    }
    this.updateProgress();
    this.updateDiagnostics(mediaState);
    this.menu.setMediaState(this.menuState());
  }

  serverStatePrecedesTarget(mediaState, target = this.generationSwitch.currentTarget()) {
    if (!target || target.generation === null) return false;
    const stateSession = Number(mediaState?.session ?? this.session);
    const stateGeneration = Number(mediaState?.generation);
    return (
      Number.isFinite(stateSession) &&
      Number.isFinite(stateGeneration) &&
      stateSession === target.session &&
      stateGeneration < target.generation
    );
  }

  updateProgress() {
    const position = this.currentPosition();
    const displayed = this.draggingSeek
      ? Number(this.seekInput.value)
      : this.seekPreviewOwner.displayedPosition(position);
    if (!this.draggingSeek) this.seekInput.value = String(displayed);
    this.updateCounter(displayed);
  }

  updateCounter(position) {
    this.counter.value = `${formatVideoTime(position)} / ${formatVideoTime(this.duration)}`;
    this.counter.textContent = this.counter.value;
    this.seekInput.setAttribute("aria-valuetext", this.counter.value);
  }

  updateAudioProcessing(status, announce) {
    this.audioProcessing = videoAudioProcessingPresentation(status);
    if (!this.audioProcessing.warning) this.lastAnnouncedAudioWarning = "";
    if (announce) this.announceAudioProcessingWarning();
  }

  announceAudioProcessingWarning() {
    const warning = this.audioProcessing.warning;
    if (!warning || warning === this.lastAnnouncedAudioWarning) return;
    this.lastAnnouncedAudioWarning = warning;
    this.showNotice(warning, "warning");
    this.schedule(() => {
      if (this.noticeKind === "warning" && this.audioProcessing.warning === warning) {
        this.hideNotice();
      }
    }, 6000);
  }

  updateDiagnostics(source = {}) {
    const size = source.video_size;
    const dimensions = size?.width && size?.height ? `${size.width}×${size.height}` : "";
    const bitrate = Number(source.effective_bitrate_bps) > 0
      ? `${(Number(source.effective_bitrate_bps) / 1_000_000).toFixed(1)} Mbps`
      : "";
    const preset = videoQualityPreset(this.quality);
    this.diagnostics.textContent = [
      `画質 ${preset.label} (${preset.traffic})`,
      dimensions,
      bitrate,
      this.audioProcessing.detail,
    ].filter(Boolean).join(" · ");
    this.diagnostics.classList.toggle("is-warning", Boolean(this.audioProcessing.warning));
  }

  schedulePoll(delayMs = VIDEO_STATE_POLL_MS) {
    this.clearPoll();
    if (this.destroyed || this.remoteSessionState.blocksInteraction || !this.session) return;
    this.pollTimer = setTimeout(() => {
      this.pollTimer = 0;
      this.pollState().catch((error) => {
        if (
          error?.name !== "AbortError" &&
          !this.destroyed &&
          !this.remoteSessionState.blocksInteraction
        ) {
          this.showOperationalError(error, "動画の状態を確認できませんでした");
          this.schedulePoll();
        }
      });
    }, Math.max(0, delayMs));
  }

  clearPoll() {
    clearTimeout(this.pollTimer);
    this.pollTimer = 0;
  }

  async pollState() {
    if (!this.session || this.destroyed) return;
    try {
      const mediaState = await this.apiJson(
        "/api/video/state",
        { session: this.session },
        this.abortController.signal
      );
      if (this.destroyed) return;
      if (this.serverStatePrecedesTarget(mediaState)) {
        this.schedulePoll();
        return;
      }
      const generationChanged = Number(mediaState.generation) !== Number(this.generation);
      this.applyServerState(mediaState);
      if (generationChanged) {
        await this.switchGeneration({
          session: this.session,
          generation: mediaState.generation,
          url: `/stream/${this.session}/${mediaState.generation}/index.m3u8`,
        });
      }
      this.schedulePoll();
    } catch (error) {
      if (error?.name === "AbortError") throw error;
      const decision = videoHttpStatusDecision(
        error?.status,
        error?.retryAfterSeconds,
        error?.code
      );
      if (decision.kind === "waiting") {
        this.showNotice(decision.message, "waiting");
        this.schedulePoll(decision.retryDelayMs);
        return;
      }
      if (decision.kind === "generation_mismatch") {
        await this.restartAt(this.currentPosition());
        return;
      }
      if (decision.kind === "session_mismatch") {
        this.hls?.stopLoad();
        this.video.pause();
        this.showStatusDecision(decision);
        return;
      }
      throw error;
    }
  }

  async handleVisibilityResume() {
    if (!this.session || this.destroyed || this.remoteSessionState.blocksInteraction) return;
    const position = this.currentPosition();
    const restorePlaying = this.playRequested;
    try {
      const mediaState = await this.apiJson(
        "/api/video/state",
        { session: this.session },
        this.abortController.signal
      );
      if (this.destroyed) return;
      if (this.serverStatePrecedesTarget(mediaState)) {
        this.playIfRequested();
        this.schedulePoll();
        return;
      }
      const generationChanged = Number(mediaState.generation) !== Number(this.generation);
      this.applyServerState(mediaState);
      if (generationChanged) {
        await this.switchGeneration({
          session: this.session,
          generation: mediaState.generation,
          url: `/stream/${this.session}/${mediaState.generation}/index.m3u8`,
        });
      }
      this.playIfRequested();
      this.schedulePoll();
    } catch (error) {
      if (error?.name === "AbortError") return;
      if ([404, 409, 410].includes(Number(error?.status))) {
        this.playRequested = restorePlaying;
        await this.restartAt(position);
        return;
      }
      const decision = videoHttpStatusDecision(
        error?.status,
        error?.retryAfterSeconds,
        error?.code
      );
      if (decision.kind === "waiting") {
        this.showNotice(decision.message, "waiting");
        this.schedulePoll(decision.retryDelayMs);
        return;
      }
      this.showOperationalError(error, "バックグラウンドから復帰できませんでした");
    }
  }

  async restartAt(positionSecs, restorePlaying = this.playRequested) {
    if (this.restarting || this.destroyed) return;
    this.restarting = true;
    const oldSession = this.session;
    this.clearPoll();
    this.generationSwitch.cancel();
    this.stopPlaylistPlayback();
    this.session = null;
    this.menu.setSession(null);
    this.generation = null;
    if (oldSession) {
      this.apiPostJson(
        "/api/video/stop",
        { session: oldSession },
        this.abortController.signal
      ).catch(() => {});
    }
    try {
      await this.start(positionSecs, restorePlaying);
    } finally {
      this.restarting = false;
    }
  }

  beginWaiting() {
    if (this.destroyed || this.waitingSince !== null) return;
    this.waitingSince = performance.now();
    clearTimeout(this.waitingTimer);
    this.waitingTimer = setTimeout(() => {
      this.waitingTimer = 0;
      if (this.waitingSince === null || this.destroyed) return;
      const suggested = bufferingQualitySuggestion({
        waitingSinceMs: this.waitingSince,
        nowMs: performance.now(),
        quality: this.quality,
        thresholdMs: WAITING_SUGGESTION_MS,
      });
      if (suggested) {
        this.showNotice(
          `通信待ちが 3 秒続いています。${suggested.label}画質 (${suggested.traffic}) を試せます。`,
          "buffering",
          `${suggested.label}画質にする`,
          () => this.dispatch(
            command(CommandName.MEDIA_QUALITY, { quality: suggested.id }),
            { source: "touch", detail: "buffering_suggestion" }
          )
        );
      } else {
        this.showNotice("通信待ちが 3 秒続いています。", "buffering");
      }
    }, WAITING_SUGGESTION_MS);
  }

  beginPlaybackStartupWatch(mode) {
    this.clearPlaybackStartupWatch();
    const watch = {
      mode,
      startedAt: performance.now(),
      mediaSegmentsLoaded: 0,
      timer: 0,
    };
    this.startupWatch = watch;
    this.schedulePlaybackStartupCheck(watch, STARTUP_MEDIA_SEGMENT_TIMEOUT_MS);
  }

  schedulePlaybackStartupCheck(watch, delayMs) {
    watch.timer = setTimeout(
      () => this.checkPlaybackStartup(watch),
      Math.max(1, Number(delayMs) || 1)
    );
  }

  markPlaybackMediaSegmentLoaded() {
    const watch = this.startupWatch;
    if (!watch) return;
    watch.mediaSegmentsLoaded += 1;
    this.checkPlaybackStartup(watch);
  }

  checkPlaybackStartupProgress() {
    const watch = this.startupWatch;
    if (watch) this.checkPlaybackStartup(watch);
  }

  checkPlaybackStartup(watch) {
    if (this.destroyed || this.startupWatch !== watch) return;
    clearTimeout(watch.timer);
    watch.timer = 0;
    const elapsedMs = performance.now() - watch.startedAt;
    const decision = videoStartupDecision({
      mediaSegmentsLoaded: watch.mediaSegmentsLoaded,
      readyState: this.video.readyState,
      elapsedMs,
      timeoutMs: STARTUP_MEDIA_SEGMENT_TIMEOUT_MS,
    });
    if (decision.kind === "started") {
      this.clearPlaybackStartupWatch();
      return;
    }
    if (decision.kind === "waiting") {
      this.schedulePlaybackStartupCheck(watch, decision.remainingMs);
      return;
    }

    this.clearPlaybackStartupWatch();
    this.hls?.stopLoad();
    this.video.pause();
    this.clearWaiting();
    this.recordPlaybackIssue(
      "video_stream_start_no_segment",
      decision.internalReason,
      {
        playback_mode: watch.mode,
        timeout_ms: STARTUP_MEDIA_SEGMENT_TIMEOUT_MS,
        elapsed_ms: Math.round(elapsedMs),
        media_segments_loaded: watch.mediaSegmentsLoaded,
        ready_state: Number(this.video.readyState) || 0,
        network_state: Number(this.video.networkState) || 0,
      }
    );
    this.showNotice(
      "このブラウザでは動画の再生を開始できませんでした。",
      "error",
      "再接続",
      () => this.restartAt(this.currentPosition())
    );
  }

  clearPlaybackStartupWatch() {
    if (!this.startupWatch) return;
    clearTimeout(this.startupWatch.timer);
    this.startupWatch = null;
  }

  recordPlaybackIssue(category, internalReason, details = {}) {
    try {
      this.reportPlaybackIssue({
        category,
        internalReason,
        session: this.session,
        generation: this.generation,
        ...details,
      });
    } catch {}
  }

  clearWaiting() {
    clearTimeout(this.waitingTimer);
    this.waitingTimer = 0;
    this.waitingSince = null;
    if (this.noticeKind === "buffering" || this.noticeKind === "waiting") {
      this.hideNotice();
    }
  }

  showNotice(message, kind = "info", actionLabel = "", action = null) {
    if (
      this.seekPreviewOwner.current().kind !== "playback" &&
      (kind === "waiting" || kind === "buffering")
    ) {
      return;
    }
    this.noticeKind = kind;
    this.notice.dataset.kind = kind;
    this.notice.replaceChildren(textElement("span", message));
    if (actionLabel && action) {
      const button = textElement("button", actionLabel);
      button.type = "button";
      button.addEventListener("click", (event) => {
        event.stopPropagation();
        action();
      });
      this.notice.append(button);
    }
    this.notice.hidden = false;
  }

  hideNotice() {
    this.noticeKind = "";
    this.notice.hidden = true;
    this.notice.replaceChildren();
  }

  showStartFailure(error) {
    if (this.remoteSessionState.blocksInteraction) return;
    const preset = videoQualityPreset(this.quality);
    this.diagnostics.textContent = `画質 ${preset.label} (${preset.traffic})`;
    this.showNotice(
      videoUserErrorMessage(
        { code: error?.code || "stream_start_failed" },
        "動画を開始できませんでした"
      ),
      "error",
      "再試行",
      () => this.start(this.currentPosition(), this.playRequested)
    );
  }

  showOperationalError(error, prefix) {
    if (
      error?.name === "AbortError" ||
      this.destroyed ||
      this.remoteSessionState.blocksInteraction
    ) return;
    this.showNotice(videoUserErrorMessage(error, prefix), "error");
  }

  showBoundaryMessage(message) {
    this.showNotice(message, "boundary");
    this.schedule(() => {
      if (this.noticeKind === "boundary") this.hideNotice();
    }, 2400);
  }

  onPointerDown(event) {
    if (event.target.closest?.("button, input, .video-stream-notice")) return;
    if (["mouse", "pen"].includes(event.pointerType) && event.button !== 0) return;
    this.stage.setPointerCapture?.(event.pointerId);
    this.pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (this.pointers.size === 1) {
      this.singlePointer = {
        startX: event.clientX,
        startY: event.clientY,
        startedAt: performance.now(),
        edgeGuarded: event.clientX <= 32,
        moved: false,
      };
    } else {
      this.singlePointer = null;
    }
  }

  onPointerMove(event) {
    if (!this.pointers.has(event.pointerId)) return;
    const pointer = this.pointers.get(event.pointerId);
    this.pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (this.singlePointer && Math.hypot(
      event.clientX - pointer.x,
      event.clientY - pointer.y
    ) > 2) {
      this.singlePointer.moved = true;
    }
  }

  onPointerUp(event, cancelled) {
    if (!this.pointers.has(event.pointerId)) return;
    const single = this.singlePointer;
    this.pointers.delete(event.pointerId);
    if (this.stage.hasPointerCapture?.(event.pointerId)) {
      this.stage.releasePointerCapture(event.pointerId);
    }
    if (this.pointers.size || !single) return;
    const gesture = viewerGestureDecision({
      dx: event.clientX - single.startX,
      dy: event.clientY - single.startY,
      elapsedMs: performance.now() - single.startedAt,
      moved: single.moved,
      edgeGuarded: single.edgeGuarded,
      cancelled,
    });
    const meta = {
      source: event.pointerType === "mouse" ? "mouse" : "touch",
      detail: "video_gesture",
    };
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
    if (gesture === ViewerGesture.SWIPE_LEFT) {
      this.dispatch(command(CommandName.NEXT_PAGE), meta);
    } else if (gesture === ViewerGesture.SWIPE_RIGHT) {
      this.dispatch(command(CommandName.PREV_PAGE), meta);
    } else if (panelAction === ViewerPanelAction.OPEN) {
      this.dispatch(command(CommandName.TOGGLE_MENU), {
        ...meta,
        detail: "swipe_up",
      });
    } else if (gesture === ViewerGesture.TAP) {
      this.dispatch(videoTapCommand(event.clientX, this.root.clientWidth), meta);
    }
    this.singlePointer = null;
  }

  schedule(callback, delayMs) {
    const timer = setTimeout(() => {
      this.pendingTimers.delete(timer);
      if (!this.destroyed) Promise.resolve(callback()).catch(() => {});
    }, Math.max(0, Number(delayMs) || 0));
    this.pendingTimers.add(timer);
  }

  destroyHls() {
    this.clearPlaybackStartupWatch();
    const hls = this.hls;
    this.hls = null;
    hls?.destroy();
  }

  stopPlaylistPlayback() {
    this.destroyHls();
    this.video.pause();
    this.video.removeAttribute("src");
    this.video.load();
  }

  destroy() {
    if (this.destroyed) return;
    this.destroyed = true;
    this.unsubscribeRemoteSessionState();
    this.abortController.abort();
    this.seekThumbnailAbort?.abort();
    this.seekThumbnailAbort = null;
    this.clearSeekThumbnailObjectUrl();
    this.generationSwitch.cancel();
    this.clearPoll();
    clearTimeout(this.waitingTimer);
    for (const timer of this.pendingTimers) clearTimeout(timer);
    this.pendingTimers.clear();
    this.stopPlaylistPlayback();
    this.video.removeEventListener("timeupdate", this.onTimeUpdate);
    this.video.removeEventListener("playing", this.onPlaying);
    this.video.removeEventListener("canplay", this.onCanPlay);
    this.video.removeEventListener("loadeddata", this.onLoadedData);
    this.video.removeEventListener("waiting", this.onWaiting);
    this.video.removeEventListener("ended", this.onEnded);
    this.video.removeEventListener("error", this.onNativeError);
    this.stage.removeEventListener("pointerdown", this.pointerDown);
    this.stage.removeEventListener("pointermove", this.pointerMove);
    this.stage.removeEventListener("pointerup", this.pointerUp);
    this.stage.removeEventListener("pointercancel", this.pointerCancel);
    this.stage.removeEventListener("touchend", this.nativeGesture);
    this.stage.removeEventListener("gesturestart", this.nativeGesture);
    this.stage.removeEventListener("gesturechange", this.nativeGesture);
    this.stage.removeEventListener("gestureend", this.nativeGesture);
    this.stage.removeEventListener("dblclick", this.nativeGesture);
    this.menu.destroy();
    if (this.session) {
      fetch("/api/video/stop", {
        method: "POST",
        credentials: "same-origin",
        keepalive: true,
        headers: {
          Accept: "application/json",
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ session: this.session }),
      }).catch(() => {});
    }
  }
}
