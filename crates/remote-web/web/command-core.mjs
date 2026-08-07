export const CommandName = Object.freeze({
  NEXT_PAGE: "next_page",
  PREV_PAGE: "prev_page",
  FIRST_PAGE: "first_page",
  LAST_PAGE: "last_page",
  ZOOM_IN: "zoom_in",
  ZOOM_OUT: "zoom_out",
  ZOOM_RESET: "zoom_reset",
  FIT_CYCLE: "fit_cycle",
  FIT_PAGE: "fit_page",
  FIT_WIDTH: "fit_width",
  FIT_ORIGINAL: "fit_original",
  FIT_TOGGLE_PAGE_ORIGINAL: "fit_toggle_page_original",
  SPREAD_CYCLE: "spread_cycle",
  SPREAD_SINGLE: "spread_single",
  SPREAD_LTR: "spread_ltr",
  SPREAD_LTR_COVER: "spread_ltr_cover",
  SPREAD_RTL: "spread_rtl",
  SPREAD_RTL_COVER: "spread_rtl_cover",
  SET_TRANSFORM: "set_transform",
  PAN_BY: "pan_by",
  TOGGLE_MENU: "toggle_menu",
  TOGGLE_VIEWER_BARS: "toggle_viewer_bars",
  OPEN_GESTURE_HELP: "open_gesture_help",
  OPEN_LOCAL_SETTINGS: "open_local_settings",
  SET_RATING: "set_rating",
  TOGGLE_BOOKMARK: "toggle_bookmark",
  BACK: "back",
  FORWARD: "forward",
  PARENT_FOLDER: "parent_folder",
  OPEN_HOME: "open_home",
  OPEN: "open",
  OPEN_SELECTED: "open_selected",
  TOGGLE_FULLSCREEN: "toggle_fullscreen",
  RELOAD_APP: "reload_app",
  GRID_LEFT: "grid_left",
  GRID_RIGHT: "grid_right",
  GRID_UP: "grid_up",
  GRID_DOWN: "grid_down",
  GRID_FIRST: "grid_first",
  GRID_LAST: "grid_last",
  GRID_PAGE_PREV: "grid_page_prev",
  GRID_PAGE_NEXT: "grid_page_next",
  GRID_SELECT: "grid_select",
  MEDIA_TOGGLE_PLAY: "media_toggle_play",
  MEDIA_SEEK_ABSOLUTE: "media_seek_absolute",
  MEDIA_SEEK_RELATIVE: "media_seek_relative",
  MEDIA_VOLUME: "media_volume",
  MEDIA_QUALITY: "media_quality",
});

export const FitMode = Object.freeze({
  PAGE: "page",
  WIDTH: "width",
  ORIGINAL: "original",
});

export const SpreadMode = Object.freeze({
  SINGLE: "single",
  LTR: "ltr",
  LTR_COVER: "ltr_cover",
  RTL: "rtl",
  RTL_COVER: "rtl_cover",
});

export const ReadingDirection = Object.freeze({
  LTR: "ltr",
  RTL: "rtl",
});

export function remoteSessionAcquireDecision(status, trigger) {
  if (status === "active") return "use_current";
  if (trigger === "explicit_reconnect") return "acquire";
  if (
    (status === "inactive" || status === "not_acquired") &&
    (trigger === "initial" || trigger === "user_operation")
  ) {
    return "acquire";
  }
  if (status === "expired" && trigger === "user_operation") {
    return "acquire";
  }
  return "blocked";
}

// A 409/local_in_use response from the acquire endpoint means that the previous
// owner is still draining. Keep the same user acquisition intent alive until
// that drain completes; there is no safe wall-clock deadline for releasing a
// streaming worker. Back off after the fast path so a long drain does not poll
// the core four times per second indefinitely.
export function remoteSessionAcquireRetryDelay(httpStatus, sessionStatus, attempt = 0) {
  if (Number(httpStatus) !== 409 || sessionStatus !== "local_in_use") return null;
  const retry = Math.max(0, Math.trunc(Number(attempt) || 0));
  if (retry < 8) return 250;
  if (retry < 16) return 500;
  return 1000;
}

export function remoteSessionControlTransition(current, status, message = "") {
  const previous = current ?? {
    status: "inactive",
    message: "",
    phase: "inactive",
    blocksInteraction: false,
    disconnectReason: null,
  };
  const nextStatus = String(status || "unavailable");
  const nextMessage = String(message || "");

  if (nextStatus === "active" || nextStatus === "inactive") {
    return {
      status: nextStatus,
      message: nextMessage,
      phase: nextStatus,
      blocksInteraction: false,
      disconnectReason: null,
    };
  }

  if (nextStatus === "local_in_use" || nextStatus === "other_device") {
    return {
      status: nextStatus,
      message: nextMessage,
      phase: "disconnected",
      blocksInteraction: true,
      disconnectReason: nextStatus,
    };
  }

  // Once the server has explicitly revoked ownership, only a successful acquire
  // may expose the application again. In particular, a reconnect attempt and a
  // transient failure during that attempt must keep the existing modal boundary.
  if (previous.blocksInteraction) {
    return {
      status: nextStatus,
      message: nextMessage,
      phase: nextStatus === "acquiring" ? "reconnecting" : "disconnected",
      blocksInteraction: true,
      disconnectReason: previous.disconnectReason,
    };
  }

  return {
    status: nextStatus,
    message: nextMessage,
    phase: "recoverable",
    blocksInteraction: false,
    disconnectReason: null,
  };
}

const REMOTE_SESSION_TELEMETRY_OBSERVERS = new Set([
  "acquire",
  "ping",
  "video_poll",
  "video_request",
  "page_request",
  "api_request",
  "auth",
  "client",
]);

const REMOTE_SESSION_OBSERVED_STATUSES = new Set([
  "request_started",
  "active",
  "local_in_use",
  "superseded",
  "expired",
  "not_acquired",
  "unavailable",
  "session_id_mismatch",
  "network_error",
  "pin_required",
  "inactive",
  "unknown",
]);

/// Build the privacy-bounded record for an actual control-state change. Server messages,
/// resource URLs, client/session IDs and request bodies are deliberately excluded.
export function remoteSessionTransitionTelemetry(previous, next, observation = {}) {
  if (!previous || !next) return null;
  const changed =
    previous.status !== next.status ||
    previous.phase !== next.phase ||
    previous.blocksInteraction !== next.blocksInteraction ||
    previous.disconnectReason !== next.disconnectReason;
  if (!changed) return null;

  const requestedObserver = String(observation.observer ?? "client");
  const observer = REMOTE_SESSION_TELEMETRY_OBSERVERS.has(requestedObserver)
    ? requestedObserver
    : "client";
  const requestedObservedStatus = String(observation.observedStatus ?? "unknown");
  const observedStatus = REMOTE_SESSION_OBSERVED_STATUSES.has(requestedObservedStatus)
    ? requestedObservedStatus
    : "unknown";
  const numericHttpStatus = Number(observation.httpStatus);
  const httpStatus = Number.isInteger(numericHttpStatus) &&
      numericHttpStatus >= 100 && numericHttpStatus <= 599
    ? numericHttpStatus
    : null;

  return {
    type: "remote_session",
    action: "control_transition",
    observer,
    observed_status: observedStatus,
    http_status: httpStatus,
    from_status: previous.status,
    from_phase: previous.phase,
    from_blocks_interaction: previous.blocksInteraction,
    to_status: next.status,
    to_phase: next.phase,
    to_blocks_interaction: next.blocksInteraction,
    disconnect_reason: next.disconnectReason,
  };
}

/// Only events whose diagnostic value is lost when the current JS turn fails bypass the
/// five-second batch. Expected HTTP/image failures remain batched.
export function telemetryDeliveryMode(event) {
  if (
    event?.type === "app_version" &&
    event?.action === "session_acquire" &&
    event?.update_outcome === "automatic_reload"
  ) {
    return "immediate";
  }
  if (
    event?.type === "remote_session" &&
    event?.action === "control_transition"
  ) {
    return "immediate";
  }
  if (
    event?.type === "error" &&
    (
      event?.category === "window_error" ||
      event?.category === "unhandled_rejection" ||
      event?.category === "video_stream_hls_fatal" ||
      event?.category === "video_stream_media_element_error" ||
      event?.category === "video_stream_playback_stalled"
    )
  ) {
    return "immediate";
  }
  if (
    event?.type === "video_health" &&
    [
      "waiting_threshold",
      "stall_terminal",
      "hls_fatal",
      "media_error",
      "play_rejected",
    ].includes(
      event?.trigger
    )
  ) {
    return "immediate";
  }
  return "batch";
}

const NORMAL_TELEMETRY_OMIT_KEYS = new Set([
  "address",
  "client_id",
  "device_id",
  "diagnostic_message",
  "entry_name",
  "favorite_id",
  "filename",
  "load_error_message",
  "message",
  "name",
  "path",
  "relative_path",
  "remote_address",
  "remote_client_id",
  "remote_session_correlation",
  "remote_session_id",
  "resource",
  "server_message",
  "session",
  "session_correlation",
  "session_id",
  "stack",
  "stream_session",
  "url",
]);

function redactTelemetryValue(value, secrets) {
  if (typeof value === "string") {
    let redacted = value;
    for (const secret of secrets) {
      if (secret) redacted = redacted.replaceAll(secret, "[redacted-secret]");
    }
    return redacted;
  }
  if (Array.isArray(value)) return value.map((item) => redactTelemetryValue(item, secrets));
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(
    Object.entries(value).map(([key, item]) => [key, redactTelemetryValue(item, secrets)])
  );
}

function normalTelemetryValue(value) {
  if (Array.isArray(value)) return value.map(normalTelemetryValue);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(
    Object.entries(value)
      .filter(([key]) => !NORMAL_TELEMETRY_OMIT_KEYS.has(key))
      .map(([key, item]) => [key, normalTelemetryValue(item)])
  );
}

/// Apply the collection tier before an event enters either the immediate or batched queue.
/// The server logger still performs its permanent-secret redaction as the final disk boundary.
export function telemetryEventForTier(event, {
  detailed = false,
  clientId = "",
  sessionCorrelation = "",
  sensitiveValues = [],
} = {}) {
  const secrets = Array.from(sensitiveValues, (value) => String(value ?? ""))
    .filter(Boolean);
  const source = redactTelemetryValue(event ?? {}, secrets);
  if (!detailed) {
    return {
      ...normalTelemetryValue(source),
      telemetry_tier: "normal",
    };
  }
  const result = {
    ...source,
    telemetry_tier: "debug",
  };
  const normalizedClientId = String(clientId ?? "");
  if (/^[A-Za-z0-9_-]{8,128}$/.test(normalizedClientId)) {
    result.client_id = normalizedClientId;
  }
  const normalizedCorrelation = String(sessionCorrelation ?? "");
  if (/^[a-f0-9]{24}$/.test(normalizedCorrelation)) {
    result.remote_session_correlation = normalizedCorrelation;
  }
  return result;
}

export async function telemetrySessionCorrelation(sessionId, subtle = globalThis.crypto?.subtle) {
  const value = String(sessionId ?? "");
  if (!value || !subtle?.digest || typeof TextEncoder !== "function") return "";
  try {
    const digest = await subtle.digest("SHA-256", new TextEncoder().encode(value));
    return Array.from(new Uint8Array(digest).slice(0, 12), (byte) =>
      byte.toString(16).padStart(2, "0")
    ).join("");
  } catch {
    return "";
  }
}

export function remoteSessionFailureStatus({
  sessionStatus,
  httpStatus,
  errorCode,
} = {}) {
  if (sessionStatus === "superseded") return "other_device";
  if (sessionStatus === "local_in_use") return "local_in_use";
  if (sessionStatus === "expired") return "expired";
  if (sessionStatus === "not_acquired") return "not_acquired";
  if (errorCode !== "session_required") return null;
  if (Number(httpStatus) === 409) return "local_in_use";
  if (Number(httpStatus) === 428) return "expired";
  return null;
}

export function remoteStateGenerationTransition(current, observed) {
  const previous = String(current ?? "");
  const generation = String(observed ?? "");
  if (!generation) {
    return { generation: previous, changed: false, initialized: Boolean(previous) };
  }
  const previousSeparator = previous.lastIndexOf("-");
  const generationSeparator = generation.lastIndexOf("-");
  if (
    previousSeparator > 0 &&
    generationSeparator > 0 &&
    previous.slice(0, previousSeparator) === generation.slice(0, generationSeparator)
  ) {
    const previousCounter = Number(previous.slice(previousSeparator + 1));
    const generationCounter = Number(generation.slice(generationSeparator + 1));
    if (
      Number.isSafeInteger(previousCounter) &&
      Number.isSafeInteger(generationCounter) &&
      generationCounter < previousCounter
    ) {
      return { generation: previous, changed: false, initialized: true };
    }
  }
  return {
    generation,
    changed: Boolean(previous) && previous !== generation,
    initialized: true,
  };
}

export function pageResponseGenerationAttestation(requestedGeneration, responseGeneration) {
  const requested = String(requestedGeneration ?? "");
  const observed = String(responseGeneration ?? "");
  return {
    requested,
    observed,
    matches: Boolean(requested) && requested === observed,
  };
}

export function viewerPageGroupGenerationSnapshot(generation, pageCount) {
  const value = String(generation ?? "");
  const count = Math.max(0, Math.floor(Number(pageCount) || 0));
  return {
    generation: value,
    pages: Array.from({ length: count }, () => value),
  };
}

export const ViewerGesture = Object.freeze({
  TAP: "tap",
  SWIPE_LEFT: "swipe_left",
  SWIPE_RIGHT: "swipe_right",
  SWIPE_UP: "swipe_up",
  SWIPE_DOWN: "swipe_down",
  PAN: "pan",
});

export const ViewerPanelOrientation = Object.freeze({
  PORTRAIT: "portrait",
  LANDSCAPE: "landscape",
});

export const ViewerPanelAction = Object.freeze({
  OPEN: "open",
  CLOSE: "close",
});

export const VIEWER_PANEL_ANIMATION_MS = 180;

export const VIDEO_QUALITY_PRESETS = Object.freeze([
  Object.freeze({ id: "minimum", label: "最小", traffic: "約 210 MB / 時" }),
  Object.freeze({ id: "low", label: "低", traffic: "約 400 MB / 時" }),
  Object.freeze({ id: "standard", label: "標準", traffic: "約 730 MB / 時" }),
  Object.freeze({ id: "high", label: "高", traffic: "約 1.4 GB / 時" }),
]);

export const IMAGE_QUALITY_PRESETS = Object.freeze([
  Object.freeze({ id: "high", label: "高品質", maxLongSide: 8192 }),
  Object.freeze({ id: "standard", label: "標準", maxLongSide: 4096 }),
  Object.freeze({ id: "light", label: "軽量", maxLongSide: 2048 }),
  Object.freeze({ id: "minimum", label: "最軽量", maxLongSide: 1024 }),
]);

export function imageQualityPreset(quality) {
  return IMAGE_QUALITY_PRESETS.find((preset) => preset.id === quality) ??
    IMAGE_QUALITY_PRESETS.find((preset) => preset.id === "standard");
}

export function command(name, payload = {}) {
  return { name, payload };
}

export function commandFromKey(input, context) {
  if (input.editable) return null;
  const key = String(input.key ?? "");
  const ctrlOrMeta = Boolean(input.ctrlKey || input.metaKey);
  const plain = !ctrlOrMeta && !input.altKey;

  if (plain && key === "?" && !input.repeat) {
    return command(CommandName.TOGGLE_MENU);
  }
  if (plain && key === "F11" && !input.repeat) {
    return command(CommandName.TOGGLE_FULLSCREEN);
  }
  if (plain && key === "Escape" && input.menuOpen && !input.repeat) {
    return command(CommandName.TOGGLE_MENU);
  }

  if (context === "viewer") {
    if (plain && key === "ArrowRight") {
      return command(input.rtl ? CommandName.PREV_PAGE : CommandName.NEXT_PAGE);
    }
    if (plain && key === "ArrowLeft") {
      return command(input.rtl ? CommandName.NEXT_PAGE : CommandName.PREV_PAGE);
    }
    if (plain && ["ArrowDown", "PageDown"].includes(key)) {
      return command(CommandName.NEXT_PAGE);
    }
    if (plain && ["ArrowUp", "PageUp"].includes(key)) {
      return command(CommandName.PREV_PAGE);
    }
    if (plain && key === "Home") return command(CommandName.FIRST_PAGE);
    if (plain && key === "End") return command(CommandName.LAST_PAGE);
    if (plain && ["Backspace", "Enter", "Escape"].includes(key) && !input.repeat) {
      return command(CommandName.BACK);
    }
    if (plain && ["+", "="].includes(key)) return command(CommandName.ZOOM_IN);
    if (plain && key === "-") return command(CommandName.ZOOM_OUT);
    if (plain && key === "0") return command(CommandName.FIT_CYCLE);
    if (plain && key === "1") return command(CommandName.SPREAD_SINGLE);
    if (plain && key === "2") return command(CommandName.SPREAD_LTR);
    if (plain && key === "3") return command(CommandName.SPREAD_LTR_COVER);
    if (plain && key === "4") return command(CommandName.SPREAD_RTL);
    if (plain && key === "5") return command(CommandName.SPREAD_RTL_COVER);
    return null;
  }

  if (context === "media") {
    if (plain && key === " " && !input.repeat) {
      return command(CommandName.MEDIA_TOGGLE_PLAY);
    }
    if (plain && key === "ArrowLeft") {
      return command(CommandName.MEDIA_SEEK_RELATIVE, { seconds: -10 });
    }
    if (plain && key === "ArrowRight") {
      return command(CommandName.MEDIA_SEEK_RELATIVE, { seconds: 10 });
    }
    if (plain && key === "ArrowDown") return command(CommandName.NEXT_PAGE);
    if (plain && key === "ArrowUp") return command(CommandName.PREV_PAGE);
    if (plain && ["Backspace", "Enter", "Escape"].includes(key) && !input.repeat) {
      return command(CommandName.BACK);
    }
    return null;
  }

  if (context === "grid") {
    if (plain && key === "Backspace" && !input.repeat) {
      return command(CommandName.PARENT_FOLDER);
    }
    if (input.altKey && !ctrlOrMeta && key === "ArrowUp" && !input.repeat) {
      return command(CommandName.PARENT_FOLDER);
    }
    if (input.altKey && !ctrlOrMeta && key === "ArrowLeft" && !input.repeat) {
      return command(CommandName.BACK);
    }
    if (input.altKey && !ctrlOrMeta && key === "ArrowRight" && !input.repeat) {
      return command(CommandName.FORWARD);
    }
    if (plain && key === "Escape" && !input.repeat) return command(CommandName.BACK);
    if (plain && key === "ArrowLeft") return command(CommandName.GRID_LEFT);
    if (plain && key === "ArrowRight") return command(CommandName.GRID_RIGHT);
    if (plain && key === "ArrowUp") return command(CommandName.GRID_UP);
    if (plain && key === "ArrowDown") return command(CommandName.GRID_DOWN);
    if (plain && key === "Home") return command(CommandName.GRID_FIRST);
    if (plain && key === "End") return command(CommandName.GRID_LAST);
    if (plain && key === "PageUp") return command(CommandName.GRID_PAGE_PREV);
    if (plain && key === "PageDown") return command(CommandName.GRID_PAGE_NEXT);
    if (plain && key === "Enter" && !input.repeat) {
      return command(CommandName.OPEN_SELECTED);
    }
  }
  return null;
}

export function nextFitMode(mode) {
  if (mode === FitMode.PAGE) return FitMode.WIDTH;
  if (mode === FitMode.WIDTH) return FitMode.ORIGINAL;
  return FitMode.PAGE;
}

export function togglePageOriginalFitMode(mode, { scale = 1 } = {}) {
  if ((Number(scale) || 1) > 1.01) return FitMode.PAGE;
  return mode === FitMode.ORIGINAL ? FitMode.PAGE : FitMode.ORIGINAL;
}

export function viewerTapZone(clientX, width) {
  const ratio = Math.max(0, Math.min(1, Number(clientX) / Math.max(1, Number(width) || 1)));
  if (ratio < 0.34) return "left";
  if (ratio > 0.66) return "right";
  return "center";
}

/// Only center touches participate in double-tap recognition. Edge taps are returned immediately
/// and clear any center candidate, so page navigation never waits for the double-tap window.
export function viewerTapSequenceTransition(
  previous,
  { x, y, atMs, width, inputSource = "touch" },
  { maxDelayMs = 320, maxDistancePx = 36 } = {}
) {
  const zone = viewerTapZone(x, width);
  const current = {
    x: Number(x) || 0,
    y: Number(y) || 0,
    atMs: Number(atMs) || 0,
    inputSource,
    zone,
  };
  if (inputSource !== "touch") {
    return { action: "single_tap", next: null, commitPrevious: Boolean(previous) };
  }
  if (zone !== "center") {
    return { action: "edge_tap", next: null, commitPrevious: Boolean(previous) };
  }
  const elapsed = current.atMs - Number(previous?.atMs);
  const distance = Math.hypot(
    current.x - (Number(previous?.x) || 0),
    current.y - (Number(previous?.y) || 0)
  );
  if (
    previous?.inputSource === "touch" &&
    previous?.zone === "center" &&
    elapsed >= 0 &&
    elapsed <= maxDelayMs &&
    distance <= maxDistancePx
  ) {
    return { action: "double_tap", next: null, commitPrevious: false };
  }
  return {
    action: "pending_center_tap",
    next: current,
    commitPrevious: Boolean(previous),
  };
}

export function nextSpreadMode(mode) {
  const cycle = [
    SpreadMode.SINGLE,
    SpreadMode.LTR,
    SpreadMode.LTR_COVER,
    SpreadMode.RTL,
    SpreadMode.RTL_COVER,
  ];
  const index = cycle.indexOf(mode);
  return cycle[(index < 0 ? 0 : index + 1) % cycle.length];
}

export function isRtlSpread(mode) {
  return mode === SpreadMode.RTL || mode === SpreadMode.RTL_COVER;
}

export function isRtlReadingDirection(direction) {
  return direction === ReadingDirection.RTL;
}

export function readingDirectionForSpreadMode(mode, currentDirection) {
  if (mode === SpreadMode.RTL || mode === SpreadMode.RTL_COVER) {
    return ReadingDirection.RTL;
  }
  if (mode === SpreadMode.LTR || mode === SpreadMode.LTR_COVER) {
    return ReadingDirection.LTR;
  }
  return currentDirection === ReadingDirection.RTL
    ? ReadingDirection.RTL
    : ReadingDirection.LTR;
}

/// 正方形は横持ち側に倒し、縦が横を 1px でも上回ると表示限定 Single にする。
export function isPortraitViewport(width, height) {
  return Math.max(0, Number(height) || 0) > Math.max(0, Number(width) || 0);
}

/// 静止画パネルと、パネルを除いた画像 viewport の寸法を同じ判定から導く。
/// 縦持ちは下 50%、横持ち（正方形を含む）は左 40% をパネルが使う。
export function viewerPanelLayout({ viewportWidth, viewportHeight, open = true } = {}) {
  const width = Math.max(0, Number(viewportWidth) || 0);
  const height = Math.max(0, Number(viewportHeight) || 0);
  const orientation = isPortraitViewport(width, height)
    ? ViewerPanelOrientation.PORTRAIT
    : ViewerPanelOrientation.LANDSCAPE;
  if (!open) {
    return {
      orientation,
      panel: { x: 0, y: height, width: 0, height: 0 },
      image: { x: 0, y: 0, width, height },
    };
  }
  if (orientation === ViewerPanelOrientation.PORTRAIT) {
    const imageHeight = height * 0.5;
    return {
      orientation,
      panel: { x: 0, y: imageHeight, width, height: height - imageHeight },
      image: { x: 0, y: 0, width, height: imageHeight },
    };
  }
  const panelWidth = width * 0.4;
  return {
    orientation,
    panel: { x: 0, y: 0, width: panelWidth, height },
    image: { x: panelWidth, y: 0, width: width - panelWidth, height },
  };
}

/// 静止画と動画で共有する panel shell の open / close / resize transition。
export function viewerPanelShellTransition(
  current,
  { action = "resize", viewportWidth, viewportHeight } = {}
) {
  const wasOpen = Boolean(current?.open);
  const open = action === ViewerPanelAction.OPEN
    ? true
    : action === ViewerPanelAction.CLOSE
      ? false
      : wasOpen;
  const layout = viewerPanelLayout({ viewportWidth, viewportHeight, open });
  const orientationChanged = Boolean(current?.orientation) &&
    current.orientation !== layout.orientation;
  const openChanged = wasOpen !== open;
  return {
    open,
    orientation: layout.orientation,
    layout,
    layoutChanged: openChanged || (open && orientationChanged),
  };
}

/// 静止画固有の refit 指示を shell transition に重ねる。
export function viewerPanelTransition(
  current,
  { action = "resize", viewportWidth, viewportHeight } = {}
) {
  const { layoutChanged, ...next } = viewerPanelShellTransition(current, {
    action,
    viewportWidth,
    viewportHeight,
  });
  const wasOpen = Boolean(current?.open);
  return {
    ...next,
    shouldRefit: layoutChanged,
    resetTransform: action === ViewerPanelAction.OPEN && !wasOpen,
  };
}

/// Describe how a viewport resize updates the mounted viewer shell.
/// Spread rematerialization updates the mounted viewer instead of replacing its panel owner.
export function viewerResizePlan({
  hasContainer = false,
  forceSinglePageChanged = false,
  panelOpen = false,
} = {}) {
  const refreshContainer = Boolean(hasContainer && forceSinglePageChanged);
  return {
    refreshContainer,
    rebuildViewer: false,
    keepPanelOpen: Boolean(panelOpen),
  };
}

/// viewerGestureDecision の結果から panel 専用の open / close だけを選ぶ。
/// open はホームインジケータやブラウザ端ジェスチャを避けたコンテンツ内開始に限定する。
export function viewerPanelGestureAction({
  gesture,
  panelOpen = false,
  startY,
  contentTop = 0,
  contentBottom,
  edgeInset = 48,
  contentScrolled = false,
} = {}) {
  if (panelOpen) {
    return gesture === ViewerGesture.SWIPE_DOWN && !contentScrolled
      ? ViewerPanelAction.CLOSE
      : null;
  }
  if (gesture !== ViewerGesture.SWIPE_UP) return null;
  const top = Number(contentTop) || 0;
  const bottom = Number(contentBottom);
  const y = Number(startY);
  const inset = Math.max(0, Number(edgeInset) || 0);
  if (!Number.isFinite(bottom) || !Number.isFinite(y)) return null;
  return y >= top + inset && y <= bottom - inset
    ? ViewerPanelAction.OPEN
    : null;
}

/// 画面向きによる描画限定 Single と、利用者が明示した永続書き込みを分離する。
export function planSpreadIntent({
  address = null,
  selectedMode = null,
  currentDirection = ReadingDirection.LTR,
  portraitSinglePage = true,
  viewportWidth = 0,
  viewportHeight = 0,
} = {}) {
  const forceSinglePage = Boolean(portraitSinglePage) &&
    isPortraitViewport(viewportWidth, viewportHeight);
  const validModes = Object.values(SpreadMode);
  const writeRequest = selectedMode !== null && validModes.includes(selectedMode)
    ? {
        kind: "set_spread",
        address,
        spread_mode: selectedMode,
        reading_direction: readingDirectionForSpreadMode(selectedMode, currentDirection),
      }
    : null;
  return { forceSinglePage, writeRequest };
}

export function viewerImageLayout({
  mode,
  sourceWidth,
  sourceHeight,
  viewportWidth,
  viewportHeight,
  devicePixelRatio,
  maxRequestWidth = 32768,
}) {
  const width = Math.max(1, Number(sourceWidth) || 1);
  const height = Math.max(1, Number(sourceHeight) || 1);
  const availableWidth = Math.max(1, Number(viewportWidth) || 1);
  const availableHeight = Math.max(1, Number(viewportHeight) || 1);
  const dpr = Math.max(0.25, Number(devicePixelRatio) || 1);
  let cssWidth;
  if (mode === FitMode.ORIGINAL) {
    cssWidth = width;
  } else if (mode === FitMode.WIDTH) {
    cssWidth = availableWidth;
  } else {
    cssWidth = Math.min(availableWidth, availableHeight * (width / height));
  }
  const requestScale = mode === FitMode.ORIGINAL ? 1 : dpr;
  return {
    cssWidth,
    cssHeight: cssWidth * (height / width),
    requestWidth: Math.max(
      1,
      Math.min(maxRequestWidth, Math.ceil(cssWidth * requestScale))
    ),
  };
}

export function viewerSpreadLayout({
  mode,
  pages,
  viewportWidth,
  viewportHeight,
  devicePixelRatio,
  gap = 0,
  maxRequestWidth = 8192,
}) {
  const sources = (pages ?? []).map((page) => ({
    width: Math.max(1, Number(page?.width) || 1),
    height: Math.max(1, Number(page?.height) || 1),
  }));
  if (!sources.length) return { pages: [], gap: 0 };
  if (sources.length === 1) {
    const page = viewerImageLayout({
      mode,
      sourceWidth: sources[0].width,
      sourceHeight: sources[0].height,
      viewportWidth,
      viewportHeight,
      devicePixelRatio,
      maxRequestWidth,
    });
    return {
      pages: [page],
      gap: 0,
      cssWidth: page.cssWidth,
      cssHeight: page.cssHeight,
    };
  }
  const availableWidth = Math.max(1, Number(viewportWidth) || 1);
  const availableHeight = Math.max(1, Number(viewportHeight) || 1);
  const resolvedGap = Math.max(0, Number(gap) || 0);
  const contentWidth = Math.max(1, availableWidth - resolvedGap);
  const sourceHeight = Math.max(...sources.map((page) => page.height));
  const normalizedWidths = sources.map(
    (page) => page.width * sourceHeight / page.height
  );
  const sourceWidth = normalizedWidths.reduce((sum, width) => sum + width, 0);
  const scale = mode === FitMode.ORIGINAL
    ? 1
    : mode === FitMode.WIDTH
      ? contentWidth / sourceWidth
      : Math.min(contentWidth / sourceWidth, availableHeight / sourceHeight);
  const dpr = Math.max(0.25, Number(devicePixelRatio) || 1);
  const pageLayouts = sources.map((page, index) => {
    const cssWidth = normalizedWidths[index] * scale;
    return {
      cssWidth,
      cssHeight: sourceHeight * scale,
      requestWidth: Math.max(
        1,
        Math.min(maxRequestWidth, Math.ceil(cssWidth * (mode === FitMode.ORIGINAL ? 1 : dpr)))
      ),
    };
  });
  const cssHeight = sourceHeight * scale;
  return {
    pages: pageLayouts,
    gap: resolvedGap,
    cssWidth: pageLayouts.reduce((sum, page) => sum + page.cssWidth, 0) + resolvedGap,
    cssHeight,
  };
}

// PageGroup.pages is supplied in physical left-to-right order, including RTL books.
export function viewerPageDisplaySlot(pageCount, pageIndex) {
  const count = Math.max(0, Math.floor(Number(pageCount) || 0));
  const index = Math.floor(Number(pageIndex));
  if (count !== 2 || (index !== 0 && index !== 1)) return "single";
  return index === 0 ? "spread_left" : "spread_right";
}

export function viewerSpreadPartnerIndex(pageCount, pageIndex) {
  const count = Math.max(0, Math.floor(Number(pageCount) || 0));
  const index = Math.floor(Number(pageIndex));
  if (count !== 2 || (index !== 0 && index !== 1)) return null;
  return index === 0 ? 1 : 0;
}

export function viewerBoundaryMessage({
  currentIndex,
  count,
  delta,
  readingDirection = ReadingDirection.LTR,
}) {
  const index = Math.floor(Number(currentIndex));
  const length = Math.max(0, Math.floor(Number(count) || 0));
  const step = Math.sign(Number(delta) || 0);
  if (!Number.isInteger(index) || index < 0 || index >= length || !step) return null;
  const atStart = step < 0 && index === 0;
  const atEnd = step > 0 && index === length - 1;
  if (!atStart && !atEnd) return null;
  if (readingDirection === ReadingDirection.RTL) {
    return atStart
      ? "先頭ページです（右→左綴じ：次は左をタップ）"
      : "最終ページです（右→左綴じ：前は右をタップ）";
  }
  return atStart ? "先頭ページです" : "最終ページです";
}

export function viewerTapCommand(clientX, width, rtl = false) {
  const zone = viewerTapZone(clientX, width);
  if (zone === "left") return command(rtl ? CommandName.NEXT_PAGE : CommandName.PREV_PAGE);
  if (zone === "right") return command(rtl ? CommandName.PREV_PAGE : CommandName.NEXT_PAGE);
  return command(CommandName.TOGGLE_VIEWER_BARS);
}

export function videoTapCommand(clientX, width) {
  const ratio = Math.max(0, Math.min(1, Number(clientX) / Math.max(1, Number(width) || 1)));
  if (ratio < 0.34) {
    return command(CommandName.MEDIA_SEEK_RELATIVE, { seconds: -10 });
  }
  if (ratio > 0.66) {
    return command(CommandName.MEDIA_SEEK_RELATIVE, { seconds: 10 });
  }
  return command(CommandName.MEDIA_TOGGLE_PLAY);
}

export function videoPlaybackDecision({
  nativeHlsCanPlayType = "",
  mediaSourceSupported = false,
  managedMediaSourceSupported = false,
  hlsJsSupported,
} = {}) {
  const mseSupported = mediaSourceSupported || managedMediaSourceSupported;
  if (mseSupported && hlsJsSupported !== false) {
    return { mode: "hls_js", loadHlsJs: true };
  }
  const nativeHls = ["maybe", "probably"].includes(
    String(nativeHlsCanPlayType ?? "").toLowerCase()
  );
  if (nativeHls) return { mode: "native", loadHlsJs: false };
  return {
    mode: "unsupported",
    loadHlsJs: false,
    reason: "browser_has_no_supported_hls_playback_path",
  };
}

export function videoStartupDecision({
  mediaSegmentsLoaded = 0,
  readyState = 0,
  elapsedMs = 0,
  timeoutMs = 15000,
} = {}) {
  if (Number(mediaSegmentsLoaded) > 0 || Number(readyState) >= 2) {
    return { kind: "started" };
  }
  const timeout = Math.max(1, Number(timeoutMs) || 1);
  const elapsed = Math.max(0, Number(elapsedMs) || 0);
  if (elapsed < timeout) {
    return { kind: "waiting", remainingMs: timeout - elapsed };
  }
  return {
    kind: "no_media_segment",
    internalReason: "no_media_segment_loaded_before_deadline",
  };
}

/// 走っている版と配信されている版が違うかを判定する。
///
/// 画面遷移がハッシュ変更なので、開きっぱなしのタブは自分の script を二度と取りに行かない。
/// 中からは見分けが付かず、実際に「修正が入っていないコードで確認していた」往復が 1 度起きた。
///
/// セッション取得直後だけは、以前の本体が持っていた session / stream を引き継げないため
/// 自動再読込できる。視聴中の巡回は従来どおり通知に留める。自動再読込を一度試したタブは、
/// 現行版を実際に読み込めるまで二度目を踏まず、経路上の古い応答による loop を防ぐ。
export function appUpdateNotice({
  runningToken,
  servedToken,
  dismissedToken = null,
  trigger = "watch",
  reloadAttempt = null,
}) {
  const running = String(runningToken ?? "");
  const served = String(servedToken ?? "");
  if (!running || !served || running === served) return { kind: "current" };
  if (trigger === "session_acquired") {
    const attemptedRunning = String(reloadAttempt?.runningToken ?? "");
    const attemptedServed = String(reloadAttempt?.servedToken ?? "");
    if (attemptedRunning && attemptedServed) {
      return {
        kind: "update_available",
        servedToken: served,
        reason: "reload_already_attempted",
      };
    }
    return { kind: "reload_required", servedToken: served };
  }
  if (dismissedToken != null && String(dismissedToken) === served) {
    return { kind: "dismissed" };
  }
  return { kind: "update_available", servedToken: served };
}

export function videoQualityPreset(quality) {
  return VIDEO_QUALITY_PRESETS.find((preset) => preset.id === quality) ??
    VIDEO_QUALITY_PRESETS.find((preset) => preset.id === "standard");
}

export function videoTimelineAnchor({
  sourceOriginSecs,
  durationSecs,
}) {
  const duration = Math.max(0, Number(durationSecs) || 0);
  const sourceOrigin = Math.max(0, Number(sourceOriginSecs) || 0);
  return {
    sourcePositionSecs: clampNumber(sourceOrigin, 0, duration || sourceOrigin),
    mediaTimeSecs: 0,
  };
}

/// 基準点は世代ごとに 1 度だけ source origin へ置く。生成端は端末より先行するため、世代の
/// 中では端末自身の media currentTime だけで実 playhead を進める。
export function shouldReanchorVideoTimeline({
  anchoredGeneration,
  stateGeneration,
}) {
  if (anchoredGeneration == null) return true;
  return Number(anchoredGeneration) !== Number(stateGeneration);
}

export function videoTimelinePosition({
  anchorSourcePositionSecs,
  anchorMediaTimeSecs,
  mediaCurrentTimeSecs,
  durationSecs,
}) {
  const duration = Math.max(0, Number(durationSecs) || 0);
  const position =
    (Number(anchorSourcePositionSecs) || 0) +
    (Number(mediaCurrentTimeSecs) || 0) -
    (Number(anchorMediaTimeSecs) || 0);
  return clampNumber(position, 0, duration || Math.max(0, position));
}

export function videoSeekPlan({
  targetPositionSecs,
  durationSecs,
  anchorSourcePositionSecs,
  anchorMediaTimeSecs,
  seekableRanges = [],
}) {
  const duration = Math.max(0, Number(durationSecs) || 0);
  const positionSecs = clampNumber(
    Number(targetPositionSecs) || 0,
    0,
    duration || Math.max(0, Number(targetPositionSecs) || 0)
  );
  const mediaTimeSecs =
    (Number(anchorMediaTimeSecs) || 0) +
    positionSecs -
    (Number(anchorSourcePositionSecs) || 0);
  const local = seekableRanges.some((range) => {
    const start = Number(Array.isArray(range) ? range[0] : range?.start);
    const end = Number(Array.isArray(range) ? range[1] : range?.end);
    return Number.isFinite(start) && Number.isFinite(end) &&
      mediaTimeSecs >= start && mediaTimeSecs <= end;
  });
  return local
    ? { kind: "local", positionSecs, mediaTimeSecs }
    : { kind: "remote", positionSecs, mediaTimeSecs: null };
}

export function videoAbsoluteSeekCommand(targetPositionSecs) {
  const positionSecs = Number(targetPositionSecs);
  return command(CommandName.MEDIA_SEEK_ABSOLUTE, {
    positionSecs: Number.isFinite(positionSecs) ? Math.max(0, positionSecs) : 0,
  });
}

/// An omitted position means a normal open and preserves the core's saved resume position.
/// Any explicit position, including zero, must win over a different origin returned by start.
export function videoStartSeekTarget({
  requestedPositionSecs,
  sourceOriginSecs,
  durationSecs,
  toleranceSecs = 0.25,
}) {
  if (requestedPositionSecs === null || requestedPositionSecs === undefined) return null;
  const requested = Number(requestedPositionSecs);
  if (!Number.isFinite(requested)) return null;
  const duration = Math.max(0, Number(durationSecs) || 0);
  const target = clampNumber(requested, 0, duration || Math.max(0, requested));
  const origin = clampNumber(
    Number(sourceOriginSecs) || 0,
    0,
    duration || Math.max(0, Number(sourceOriginSecs) || 0)
  );
  return Math.abs(target - origin) > Math.max(0, Number(toleranceSecs) || 0)
    ? target
    : null;
}

export function bufferingQualitySuggestion({
  waitingSinceMs,
  nowMs,
  quality,
  thresholdMs = 3000,
}) {
  if (!Number.isFinite(waitingSinceMs) ||
      Number(nowMs) - Number(waitingSinceMs) < Math.max(0, Number(thresholdMs) || 0)) {
    return null;
  }
  const index = VIDEO_QUALITY_PRESETS.findIndex((preset) => preset.id === quality);
  if (index <= 0) return null;
  return VIDEO_QUALITY_PRESETS[index - 1];
}

export function videoHttpStatusDecision(status, retryAfterSeconds = 1, errorCode = "") {
  const code = Number(status) || 0;
  if (code === 503) {
    return {
      kind: "waiting",
      retry: true,
      retryDelayMs: Math.max(1, Number(retryAfterSeconds) || 1) * 1000,
      message: "配信の準備を待っています。",
    };
  }
  if (code === 410) {
    return {
      kind: "gone",
      retry: false,
      retryDelayMs: 0,
      message: "再生を続けられませんでした。現在位置からもう一度お試しください。",
    };
  }
  if (code === 409) {
    if (errorCode === "stream_generation_mismatch") {
      return {
        kind: "generation_mismatch",
        retry: true,
        retryDelayMs: 0,
        message: "動画が更新されたため、再生を準備しています。",
      };
    }
    return {
      kind: "session_mismatch",
      retry: false,
      retryDelayMs: 0,
      message: "動画の配信が終了しました。もう一度開いてください。",
    };
  }
  if (code === 404) {
    return {
      kind: "not_found",
      retry: false,
      retryDelayMs: 0,
      message: "動画を読み込めませんでした。",
    };
  }
  return {
    kind: "error",
    retry: false,
    retryDelayMs: 0,
    message: "動画を読み込めませんでした。もう一度お試しください。",
  };
}

function clampNumber(value, minimum, maximum) {
  return Math.max(minimum, Math.min(maximum, value));
}

/// Matches egui 0.33's positive, finite logarithmic slider mapping.
/// Endpoints are handled before the logarithm, just as egui does.
export function rangeValueToNormalized({
  value,
  min,
  max,
  logarithmic = false,
}) {
  const minimum = Number(min);
  const maximum = Number(max);
  if (!Number.isFinite(minimum) || !Number.isFinite(maximum)) return 0;
  if (minimum === maximum) return 0.5;
  if (minimum > maximum) {
    return 1 - rangeValueToNormalized({
      value,
      min: maximum,
      max: minimum,
      logarithmic,
    });
  }
  const current = Number(value);
  if (!Number.isFinite(current) || current <= minimum) return 0;
  if (current >= maximum) return 1;
  if (logarithmic && minimum > 0) {
    const minLog = Math.log10(minimum);
    return clampNumber(
      (Math.log10(current) - minLog) / (Math.log10(maximum) - minLog),
      0,
      1
    );
  }
  return clampNumber((current - minimum) / (maximum - minimum), 0, 1);
}

export function rangeValueFromNormalized({
  normalized,
  min,
  max,
  step = null,
  logarithmic = false,
}) {
  const minimum = Number(min);
  const maximum = Number(max);
  if (!Number.isFinite(minimum) || !Number.isFinite(maximum)) return 0;
  if (minimum === maximum) return minimum;
  if (minimum > maximum) {
    return rangeValueFromNormalized({
      normalized: 1 - Number(normalized),
      min: maximum,
      max: minimum,
      step,
      logarithmic,
    });
  }
  const position = clampNumber(Number(normalized) || 0, 0, 1);
  let value;
  if (position <= 0) {
    value = minimum;
  } else if (position >= 1) {
    value = maximum;
  } else if (logarithmic && minimum > 0) {
    const minLog = Math.log10(minimum);
    value = 10 ** (minLog + (Math.log10(maximum) - minLog) * position);
  } else {
    value = minimum + (maximum - minimum) * position;
  }
  value = clampNumber(value, minimum, maximum);
  const increment = Number(step);
  if (Number.isFinite(increment) && increment > 0) {
    value = minimum + Math.round((value - minimum) / increment) * increment;
  }
  return clampNumber(Number(value.toFixed(12)), minimum, maximum);
}

/// Maps pointer travel to a range value without using the pointer-down position as a value.
/// Moving by one full track width traverses the normalized track; the result follows egui's
/// selected curve and min-anchored step grid.
export function relativeRangeDragValue({
  startValue,
  startClientX,
  currentClientX,
  trackWidth,
  min,
  max,
  step = 1,
  logarithmic = false,
}) {
  const parsedMin = Number(min);
  const parsedMax = Number(max);
  if (!Number.isFinite(parsedMin) || !Number.isFinite(parsedMax)) return 0;
  const minimum = Math.min(parsedMin, parsedMax);
  const maximum = Math.max(parsedMin, parsedMax);
  const parsedStart = Number(startValue);
  const start = clampNumber(
    Number.isFinite(parsedStart) ? parsedStart : minimum,
    minimum,
    maximum
  );
  const delta = Number(currentClientX) - Number(startClientX);
  if (!Number.isFinite(delta) || delta === 0) return start;
  const width = Number(trackWidth);
  if (!Number.isFinite(width) || width <= 0 || maximum === minimum) return start;
  const startPosition = rangeValueToNormalized({
    value: start,
    min: minimum,
    max: maximum,
    logarithmic,
  });
  return rangeValueFromNormalized({
    normalized: startPosition + delta / width,
    min: minimum,
    max: maximum,
    step,
    logarithmic,
  });
}

// Keep this below the viewer's general 12 px tap radius: misclassifying a drag
// as a tap can seek across the whole range, so the seek control favors drag.
export const SEEK_RANGE_DRAG_THRESHOLD_PX = 6;

export function seekRangePointerGestureDecision({
  startClientX = 0,
  startClientY = 0,
  currentClientX = 0,
  currentClientY = 0,
  maxDistancePx = 0,
  cancelled = false,
  dragThresholdPx = SEEK_RANGE_DRAG_THRESHOLD_PX,
} = {}) {
  const distance = Math.hypot(
    Number(currentClientX) - Number(startClientX),
    Number(currentClientY) - Number(startClientY)
  );
  const maximum = Math.max(
    Math.max(0, Number(maxDistancePx) || 0),
    Number.isFinite(distance) ? distance : 0
  );
  if (cancelled) return { kind: "cancel", maxDistancePx: maximum };
  const threshold = Math.max(
    0.01,
    Number(dragThresholdPx) || SEEK_RANGE_DRAG_THRESHOLD_PX
  );
  return {
    kind: maximum >= threshold ? "drag" : "tap",
    maxDistancePx: maximum,
  };
}

export function seekRangeAbsoluteValue({
  clientX = 0,
  trackLeft = 0,
  trackWidth = 0,
  min = 0,
  max = 1,
  step = 1,
  direction = ReadingDirection.LTR,
} = {}) {
  const width = Math.max(1, Number(trackWidth) || 1);
  let normalized = (Number(clientX) - Number(trackLeft)) / width;
  if (direction === ReadingDirection.RTL) normalized = 1 - normalized;
  return rangeValueFromNormalized({ normalized, min, max, step });
}

export function adjustmentResetVisible({
  value,
  defaultValue,
  disabled = false,
  epsilon = 0,
}) {
  if (disabled) return false;
  const current = Number(value);
  const baseline = Number(defaultValue);
  if (!Number.isFinite(current) || !Number.isFinite(baseline)) return false;
  return Math.abs(current - baseline) > Math.max(0, Number(epsilon) || 0);
}

export function viewerSeekGroupIndex(controlValue, groupCount) {
  const count = Math.max(0, Math.floor(Number(groupCount) || 0));
  if (!count) return -1;
  return Math.max(
    0,
    Math.min(count - 1, Math.round(Number(controlValue) || 0))
  );
}

/// Maps relative horizontal travel to the seek range's logical group index.
/// The native range and pointer drag share ReadingDirection, so RTL geometry is
/// decided in one place while logical indexes continue to increase in book order.
export function viewerSeekRelativeDragValue({
  startGroupIndex,
  startClientX,
  currentClientX,
  trackWidth,
  groupCount,
  direction = ReadingDirection.LTR,
}) {
  const count = Math.max(0, Math.floor(Number(groupCount) || 0));
  if (!count) return -1;
  const startX = Number(startClientX);
  const currentX = Number(currentClientX);
  const physicalDelta = currentX - startX;
  const logicalDelta = direction === ReadingDirection.RTL
    ? -physicalDelta
    : physicalDelta;
  return viewerSeekGroupIndex(relativeRangeDragValue({
    startValue: startGroupIndex,
    startClientX: startX,
    currentClientX: startX + logicalDelta,
    trackWidth,
    min: 0,
    max: count - 1,
    step: 1,
  }), count);
}

export function viewerSeekState({
  groupPageIndexes,
  currentGroupIndex,
  pageCount,
  rtl = false,
}) {
  const groups = Array.isArray(groupPageIndexes) ? groupPageIndexes : [];
  const total = Math.max(0, Math.floor(Number(pageCount) || 0));
  if (!groups.length) {
    return {
      visible: false,
      min: 0,
      max: 0,
      value: 0,
      groupIndex: -1,
      direction: rtl ? ReadingDirection.RTL : ReadingDirection.LTR,
      label: total ? `1 / ${total}` : "0 / 0",
    };
  }
  const groupIndex = Math.max(
    0,
    Math.min(groups.length - 1, Math.floor(Number(currentGroupIndex) || 0))
  );
  const pages = [...new Set((groups[groupIndex] ?? [])
    .map((value) => Math.floor(Number(value)))
    .filter((value) => Number.isInteger(value) && value >= 0 && value < total))]
    .sort((left, right) => left - right);
  const consecutive = pages.every(
    (page, index) => index === 0 || page === pages[index - 1] + 1
  );
  const pageLabel = pages.length === 1
    ? String(pages[0] + 1)
    : pages.length > 1 && consecutive
      ? `${pages[0] + 1}-${pages[pages.length - 1] + 1}`
      : pages.map((page) => page + 1).join(",");
  return {
    visible: total > 1,
    min: 0,
    max: groups.length - 1,
    value: groupIndex,
    groupIndex,
    direction: rtl ? ReadingDirection.RTL : ReadingDirection.LTR,
    label: `${pageLabel || groupIndex + 1} / ${total}`,
  };
}

export function viewerGestureDecision({
  dx,
  dy,
  elapsedMs,
  moved = false,
  zoomed = false,
  contentScrolled = false,
  edgeGuarded = false,
  cancelled = false,
  pinched = false,
  swipeThreshold = 52,
  axisDominance = 1.25,
  tapDistance = 12,
  tapDurationMs = 450,
}) {
  if (cancelled || pinched) return null;
  const horizontal = Number(dx) || 0;
  const vertical = Number(dy) || 0;
  const absX = Math.abs(horizontal);
  const absY = Math.abs(vertical);
  const distance = Math.hypot(horizontal, vertical);
  const swipe = Math.max(0, Number(swipeThreshold) || 0);
  const dominance = Math.max(1, Number(axisDominance) || 1);
  const tapRadius = Math.max(0, Number(tapDistance) || 0);

  if (zoomed && (moved || distance >= tapRadius)) return ViewerGesture.PAN;
  if (
    !edgeGuarded &&
    absX > swipe &&
    absX > absY * dominance
  ) {
    return horizontal < 0
      ? ViewerGesture.SWIPE_LEFT
      : ViewerGesture.SWIPE_RIGHT;
  }
  if (contentScrolled && moved) return ViewerGesture.PAN;
  if (
    !edgeGuarded &&
    absY > swipe &&
    absY > absX * dominance
  ) {
    return vertical < 0
      ? ViewerGesture.SWIPE_UP
      : ViewerGesture.SWIPE_DOWN;
  }
  if (
    !moved &&
    distance < tapRadius &&
    Number(elapsedMs) < Math.max(0, Number(tapDurationMs) || 0)
  ) {
    return ViewerGesture.TAP;
  }
  return moved ? ViewerGesture.PAN : null;
}

/// 指の移動方向に対して、実際の scrollTop が変化できるかを判定する。
/// 上端で下へ、下端で上へ引く操作は pan にせず viewer swipe へ渡す。
export function viewerVerticalScrollDecision({
  scrollTop,
  scrollHeight,
  clientHeight,
  dragDeltaY,
  epsilon = 0.5,
}) {
  const tolerance = Math.max(0, Number(epsilon) || 0);
  const maximum = Math.max(
    0,
    (Number(scrollHeight) || 0) - (Number(clientHeight) || 0)
  );
  const top = Math.max(0, Math.min(maximum, Number(scrollTop) || 0));
  const delta = Number(dragDeltaY) || 0;
  const scrollable = maximum > tolerance;
  const atStart = top <= tolerance;
  const atEnd = top >= maximum - tolerance;
  const canConsume = scrollable && (
    (delta < 0 && !atEnd) ||
    (delta > 0 && !atStart)
  );
  return { scrollable, canConsume, atStart, atEnd, maximum };
}

export function createReadingProgressBatch() {
  return {
    latest: null,
    lastEmittedIdentity: "",
    nextDueAt: 0,
  };
}

/// 読書位置の latest-only batching。effect が非 null のときだけ 1 request 送る。
export function readingProgressBatchTransition(
  current,
  event,
  intervalMs = 30_000
) {
  const interval = Math.max(1, Number(intervalMs) || 1);
  const state = {
    latest: current?.latest ?? null,
    lastEmittedIdentity: String(current?.lastEmittedIdentity ?? ""),
    nextDueAt: Math.max(0, Number(current?.nextDueAt) || 0),
  };
  const now = Math.max(0, Number(event?.now) || 0);
  if (event?.type === "observe") {
    state.latest = event.value ?? null;
  }
  if (event?.type === "reset") {
    return { state: createReadingProgressBatch(), effect: null };
  }
  const identity = String(state.latest?.identity ?? "");
  const forced = event?.type === "flush";
  const due = event?.type === "observe" || event?.type === "tick"
    ? state.nextDueAt === 0 || now >= state.nextDueAt
    : false;
  if (identity && identity !== state.lastEmittedIdentity && (forced || due)) {
    const effect = state.latest;
    state.lastEmittedIdentity = identity;
    state.nextDueAt = now + interval;
    return { state, effect };
  }
  return { state, effect: null };
}

export function shouldShowKeyboardShortcuts({
  coarsePointer = false,
  keyboardUsed = false,
} = {}) {
  return !coarsePointer || Boolean(keyboardUsed);
}

export function shouldShowGridCursor({ keyboardAvailable = false } = {}) {
  return Boolean(keyboardAvailable);
}

export function viewerWheelCommand(deltaY, zoomModifier) {
  if (!Number.isFinite(deltaY) || deltaY === 0) return null;
  if (zoomModifier) {
    return command(deltaY < 0 ? CommandName.ZOOM_IN : CommandName.ZOOM_OUT);
  }
  return command(deltaY < 0 ? CommandName.PREV_PAGE : CommandName.NEXT_PAGE);
}

export function gridLayoutForWidth(
  containerWidth,
  aspectHeightRatio,
  labelHeight = 38,
  columnOverride = 0
) {
  const width = Math.max(1, Number(containerWidth) || 1);
  const inset = width >= 900 ? 20 : 10;
  const availableWidth = Math.max(1, width - inset * 2);
  const compact = availableWidth < 600;
  const gap = compact ? 8 : 12;
  const targetCellWidth = compact ? 132 : availableWidth < 1000 ? 180 : 210;
  const resolvedColumnOverride = clampGridColumnOverride(columnOverride);
  const columns = resolvedColumnOverride || Math.max(
    1,
    Math.ceil((availableWidth + gap) / (targetCellWidth + gap))
  );
  const cellWidth = Math.max(
    1,
    (availableWidth - gap * (columns - 1)) / columns
  );
  const requestedRatio = Number(aspectHeightRatio);
  const ratio =
    Number.isFinite(requestedRatio) && requestedRatio > 0
      ? requestedRatio
      : 1;
  const resolvedLabelHeight = Math.max(1, Math.round(Number(labelHeight) || 38));
  const previewHeight = Math.max(32, Math.round(cellWidth * ratio));
  const tileHeight = previewHeight + resolvedLabelHeight;
  return {
    columns,
    cellWidth,
    previewHeight,
    labelHeight: resolvedLabelHeight,
    tileHeight,
    rowPitch: tileHeight + gap,
    gap,
    inset,
    targetCellWidth,
  };
}

export function clampGridColumnOverride(value) {
  const requested = Math.round(Number(value));
  if (!Number.isFinite(requested) || requested === 0) return 0;
  return Math.min(8, Math.max(2, requested));
}

export function gridColumnsAfterPinch(currentColumns, scale) {
  const columns = Math.max(1, Math.round(Number(currentColumns) || 1));
  const ratio = Number(scale);
  if (!Number.isFinite(ratio) || ratio <= 0) return columns;

  // Use reciprocal limits so pinch-in and pinch-out need the same proportional
  // travel. After a change the gesture owner rebases its starting distance.
  const step = 1.12;
  if (ratio >= step) {
    if (columns <= 2) return columns;
    return Math.max(2, Math.min(8, columns - 1));
  }
  if (ratio <= 1 / step) {
    if (columns >= 8) return columns;
    return Math.max(2, Math.min(8, columns + 1));
  }
  return columns;
}

export function gridColumnOverrideFieldForViewport(
  viewportWidth,
  viewportHeight
) {
  const width = Math.max(0, Number(viewportWidth) || 0);
  const height = Math.max(0, Number(viewportHeight) || 0);
  return width >= height
    ? "gridColumnsLandscape"
    : "gridColumnsPortrait";
}

export function gridColumnOverrideForViewport(
  viewportWidth,
  viewportHeight,
  settings = {}
) {
  const field = gridColumnOverrideFieldForViewport(
    viewportWidth,
    viewportHeight
  );
  return clampGridColumnOverride(settings[field]);
}

export function gridLabelHeightForEntries(entries) {
  const hasDetail = Array.isArray(entries) && entries.some(
    (entry) => Boolean(entry?.detail) || Boolean(entry?.rating)
  );
  return hasDetail ? 56 : 38;
}

export function gridScrollExtent(rowCount, rowPitch, viewportHeight) {
  const rows = Math.max(0, Math.floor(Number(rowCount) || 0));
  const pitch = Math.max(1, Number(rowPitch) || 1);
  const viewport = Math.max(0, Number(viewportHeight) || 0);
  const naturalHeight = rows * pitch;
  if (naturalHeight <= viewport) {
    return {
      naturalHeight,
      maxOffset: 0,
      totalHeight: viewport,
    };
  }
  const maxOffset = Math.ceil((naturalHeight - viewport) / pitch) * pitch;
  return {
    naturalHeight,
    maxOffset,
    totalHeight: maxOffset + viewport,
  };
}

export function snappedGridOffset(scrollTop, rowPitch, maxOffset) {
  const offset = Math.max(0, Number(scrollTop) || 0);
  const pitch = Math.max(1, Number(rowPitch) || 1);
  const maximum = Math.max(0, Number(maxOffset) || 0);
  return Math.max(0, Math.min(maximum, Math.round(offset / pitch) * pitch));
}

export const GRID_VIEWPORT_MEMORY_LIMIT = 64;

const GridViewportAnchorKind = Object.freeze({
  NONE: "none",
  ITEM: "item",
});

/// 操作カーソルとは別に、一覧へ戻ったとき可視化すべき明示 target だけを所有する。
export class GridViewportAnchor {
  constructor(itemIdentities) {
    this.itemIdentities = Array.isArray(itemIdentities)
      ? itemIdentities.slice()
      : [];
    this.state = { kind: GridViewportAnchorKind.NONE };
  }

  select(index) {
    if (!Number.isInteger(index) || index < 0 || index >= this.itemIdentities.length) {
      return false;
    }
    const identity = this.itemIdentities[index];
    if (identity == null) return false;
    this.state = {
      kind: GridViewportAnchorKind.ITEM,
      identity: String(identity),
    };
    return true;
  }

  snapshot(previousScrollTop) {
    return {
      previousScrollTop: Math.max(0, Number(previousScrollTop) || 0),
      targetItemIdentity:
        this.state.kind === GridViewportAnchorKind.ITEM
          ? this.state.identity
          : null,
    };
  }
}

/// 一覧 context ごとの scroll / 選択対象を、session 内だけで有界に保持する。
/// Map の挿入順を LRU 順として使い、参照・更新した context を末尾 (MRU) へ移す。
export class GridViewportMemory {
  constructor(limit = GRID_VIEWPORT_MEMORY_LIMIT) {
    this.limit = Math.max(
      1,
      Math.floor(Number(limit) || GRID_VIEWPORT_MEMORY_LIMIT)
    );
    this.entries = new Map();
  }

  remember(context, { previousScrollTop = 0, targetItemIdentity = null } = {}) {
    const key = String(context ?? "");
    if (!key) return;
    const snapshot = {
      previousScrollTop: Math.max(0, Number(previousScrollTop) || 0),
      targetItemIdentity:
        targetItemIdentity == null ? null : String(targetItemIdentity),
    };
    this.entries.delete(key);
    this.entries.set(key, snapshot);
    while (this.entries.size > this.limit) {
      this.entries.delete(this.entries.keys().next().value);
    }
  }

  updateTarget(context, targetItemIdentity) {
    const key = String(context ?? "");
    if (!key || targetItemIdentity == null) return;
    const current = this.entries.get(key);
    this.remember(key, {
      previousScrollTop: current?.previousScrollTop ?? 0,
      targetItemIdentity,
    });
  }

  recall(context) {
    const key = String(context ?? "");
    const snapshot = this.entries.get(key);
    if (!snapshot) return null;
    this.entries.delete(key);
    this.entries.set(key, snapshot);
    return { ...snapshot };
  }

  get size() {
    return this.entries.size;
  }
}

/// 本体の App::apply_scroll_to_selected と同じく、対象行が画面外のときだけ
/// 上端または下端まで最小限動かす。対象が消えた場合は既存 offset の範囲 clamp だけ行う。
export function resolveGridReturnViewport({
  targetItemIdentity,
  itemIdentities,
  previousScrollTop,
  columns,
  rowPitch,
  viewportHeight,
}) {
  const identities = Array.isArray(itemIdentities) ? itemIdentities : [];
  const columnCount = Math.max(1, Math.floor(Number(columns) || 1));
  const pitch = Math.max(1, Number(rowPitch) || 1);
  const viewport = Math.max(0, Number(viewportHeight) || 0);
  const rowCount = Math.ceil(identities.length / columnCount);
  const { maxOffset } = gridScrollExtent(rowCount, pitch, viewport);
  let scrollTop = snappedGridOffset(previousScrollTop, pitch, maxOffset);
  const targetIndex = targetItemIdentity == null
    ? -1
    : identities.indexOf(targetItemIdentity);

  if (targetIndex >= 0) {
    const rowTop = Math.floor(targetIndex / columnCount) * pitch;
    const rowBottom = rowTop + pitch;
    if (rowTop < scrollTop) {
      scrollTop = rowTop;
    } else if (rowBottom > scrollTop + viewport) {
      scrollTop = Math.ceil((rowBottom - viewport) / pitch) * pitch;
    }
    scrollTop = Math.max(0, Math.min(maxOffset, scrollTop));
  }

  return { targetIndex, scrollTop };
}

export function thumbnailBindingMatches(
  currentGeneration,
  currentPath,
  responseGeneration,
  responsePath
) {
  return (
    Number(currentGeneration) === Number(responseGeneration) &&
    String(currentPath ?? "") === String(responsePath ?? "")
  );
}

export function thumbnailRequestConcurrency(serverHeavyLimit, reservedHeavySlots = 1) {
  const serverLimit = Math.max(1, Math.floor(Number(serverHeavyLimit) || 1));
  const reserved = Math.max(
    0,
    Math.min(serverLimit - 1, Math.floor(Number(reservedHeavySlots) || 0))
  );
  return Math.max(1, serverLimit - reserved);
}

export function thumbnailRequestStartCount(activeCount, queuedCount, concurrencyLimit) {
  const limit = Math.max(1, Math.floor(Number(concurrencyLimit) || 1));
  const active = Math.max(0, Math.floor(Number(activeCount) || 0));
  const queued = Math.max(0, Math.floor(Number(queuedCount) || 0));
  return Math.min(queued, Math.max(0, limit - active));
}

export function thumbnailRetryDecision(
  status,
  errorCode,
  retryCount,
  maxRetries = 3
) {
  const numericStatus = Number(status) || 0;
  if (numericStatus === 503 && errorCode === "ipc_busy") {
    return {
      retry: true,
      exhausted: false,
      delayMs: 0,
      consumeRetryBudget: false,
    };
  }
  const transient =
    numericStatus === 0 ||
    numericStatus === 502 ||
    (numericStatus === 503 && errorCode !== "protocol_version_mismatch");
  if (!transient) {
    return {
      retry: false,
      exhausted: false,
      delayMs: 0,
      consumeRetryBudget: false,
    };
  }
  const retries = Math.max(0, Math.floor(Number(retryCount) || 0));
  const maximum = Math.max(0, Math.floor(Number(maxRetries) || 0));
  if (retries >= maximum) {
    return {
      retry: false,
      exhausted: true,
      delayMs: 0,
      consumeRetryBudget: false,
    };
  }
  return {
    retry: true,
    exhausted: false,
    delayMs: Math.min(4000, 200 * 2 ** retries),
    consumeRetryBudget: true,
  };
}

export function pagePrefetchPlan({
  visibleIndexes,
  itemCount,
  direction,
  ahead = 3,
  behind = 1,
}) {
  const count = Math.max(0, Math.floor(Number(itemCount) || 0));
  const visible = [...new Set((visibleIndexes ?? [])
    .map((value) => Math.floor(Number(value)))
    .filter((value) => Number.isInteger(value) && value >= 0 && value < count))]
    .sort((left, right) => left - right);
  if (!visible.length) return [];
  const step = Number(direction) < 0 ? -1 : 1;
  const forwardEdge = step > 0 ? visible[visible.length - 1] : visible[0];
  const backwardEdge = step > 0 ? visible[0] : visible[visible.length - 1];
  const result = [];
  const push = (index) => {
    if (index >= 0 && index < count && !visible.includes(index) && !result.includes(index)) {
      result.push(index);
    }
  };
  for (let offset = 1; offset <= Math.max(0, Math.floor(Number(ahead) || 0)); offset += 1) {
    push(forwardEdge + step * offset);
  }
  for (let offset = 1; offset <= Math.max(0, Math.floor(Number(behind) || 0)); offset += 1) {
    push(backwardEdge - step * offset);
  }
  return result;
}

export function shouldShowLoadingIndicator(
  pending,
  elapsedMs,
  thresholdMs = 225
) {
  return Boolean(pending) &&
    Number(elapsedMs) >= Math.max(0, Number(thresholdMs) || 0);
}

export function sessionOwnerBadge(status) {
  if (status === "other_device") {
    return {
      owner: "other_device",
      label: "別の端末が操作中",
    };
  }
  return { owner: "active", label: "操作中" };
}

export function reduceViewerTransform(current, requested) {
  let scale = current.scale;
  let panX = current.panX;
  let panY = current.panY;
  if (requested.name === CommandName.ZOOM_IN) {
    scale = Math.min(6, scale * 1.2);
  } else if (requested.name === CommandName.ZOOM_OUT) {
    scale = Math.max(1, scale / 1.2);
  } else if (requested.name === CommandName.ZOOM_RESET) {
    return { scale: 1, panX: 0, panY: 0 };
  } else if (requested.name === CommandName.SET_TRANSFORM) {
    scale = Math.max(1, Math.min(6, requested.payload.scale));
    panX = requested.payload.panX;
    panY = requested.payload.panY;
  } else if (requested.name === CommandName.PAN_BY) {
    panX += requested.payload.dx;
    panY += requested.payload.dy;
  } else {
    return null;
  }
  if (scale <= 1.01) return { scale: 1, panX: 0, panY: 0 };
  return { scale, panX, panY };
}

export function gridIndexForCommand({ current, count, columns, pageRows, name }) {
  if (count <= 0) return -1;
  const safeCurrent = Math.max(0, Math.min(count - 1, current));
  const columnCount = Math.max(1, columns);
  const rows = Math.max(1, pageRows);
  const deltas = {
    [CommandName.GRID_LEFT]: -1,
    [CommandName.GRID_RIGHT]: 1,
    [CommandName.GRID_UP]: -columnCount,
    [CommandName.GRID_DOWN]: columnCount,
    [CommandName.GRID_PAGE_PREV]: -columnCount * rows,
    [CommandName.GRID_PAGE_NEXT]: columnCount * rows,
  };
  if (name === CommandName.GRID_FIRST) return 0;
  if (name === CommandName.GRID_LAST) return count - 1;
  const delta = deltas[name];
  if (delta === undefined) return safeCurrent;
  return Math.max(0, Math.min(count - 1, safeCurrent + delta));
}
