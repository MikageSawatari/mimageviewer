import {
  CommandName,
  VIDEO_QUALITY_PRESETS,
  ViewerGesture,
  bufferingQualitySuggestion,
  command,
  videoHttpStatusDecision,
  videoPlaybackDecision,
  videoQualityPreset,
  videoSeekPlan,
  videoTapCommand,
  videoTimelineAnchor,
  videoTimelinePosition,
  viewerGestureDecision,
} from "./command-core.mjs";

const HLS_MIME = "application/vnd.apple.mpegurl";
const HLS_SCRIPT_PATH = "/vendor/hls.min.js";
const VIDEO_STATE_POLL_MS = 1000;
const WAITING_SUGGESTION_MS = 3000;
const PLAYLIST_RECOVERY_MAX_ATTEMPTS = 6;
const PLAYLIST_RECOVERY_TIMEOUT_MS = 15000;

let hlsScriptPromise = null;

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

function lastSeekableEnd(media) {
  return media.seekable.length
    ? media.seekable.end(media.seekable.length - 1)
    : Number.NaN;
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

export async function resolveVideoPlaylist({
  initialUrl,
  session,
  fetchPlaylist,
  fetchState,
  signal,
  delay = abortableDelay,
  now = () => Date.now(),
  maxAttempts = PLAYLIST_RECOVERY_MAX_ATTEMPTS,
  timeoutMs = PLAYLIST_RECOVERY_TIMEOUT_MS,
  onDecision = () => {},
}) {
  let url = String(initialUrl ?? "");
  let latestState = null;
  const startedAt = now();
  const attempts = Math.max(1, Number(maxAttempts) || 1);
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    if (attempt > 0 && now() - startedAt >= Math.max(1, Number(timeoutMs) || 1)) break;
    const response = await fetchPlaylist(url, signal);
    if (response.ok) {
      return { ok: true, url, state: latestState, attempts: attempt + 1 };
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
      if (!Number.isFinite(currentSession) || !Number.isFinite(generation)) break;
      url = `/stream/${currentSession}/${generation}/index.m3u8`;
      continue;
    }
    if (decision.kind === "waiting") {
      await delay(decision.retryDelayMs, signal);
      continue;
    }
    return { ok: false, decision, attempts: attempt + 1 };
  }
  return {
    ok: false,
    attempts,
    decision: {
      kind: "playlist_recovery_exhausted",
      retry: false,
      retryDelayMs: 0,
      message: "プレイリストを取得できませんでした。",
    },
  };
}

class VideoStreamMenu {
  constructor(host, dispatch, inputSource, mediaState) {
    this.host = host;
    this.dispatch = dispatch;
    this.inputSource = inputSource;
    this.mediaState = mediaState;
    this.opened = false;
    this.previousFocus = null;
    this.keyboardElements = [];
    this.root = element("div", "command-menu-layer");
    this.root.hidden = true;

    const scrim = element("button", "command-menu-scrim");
    scrim.type = "button";
    scrim.setAttribute("aria-label", "操作メニューを閉じる");
    scrim.addEventListener("click", (event) => this.send(event, CommandName.TOGGLE_MENU));

    this.panel = element("section", "command-menu");
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
    this.panel.append(header, this.actions, this.shortcutTitle, this.shortcuts);
    this.root.append(scrim, this.panel);
    host.append(this.root);
    this.showPage("main");
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

  showPage(page) {
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

  isOpen() { return this.opened; }

  toggle() {
    if (this.opened) this.close();
    else this.open();
    return true;
  }

  open() {
    if (this.opened) return;
    this.showPage("main");
    this.opened = true;
    this.previousFocus = document.activeElement;
    this.root.hidden = false;
    this.host.classList.add("menu-open");
    requestAnimationFrame(() => this.closeButton.focus());
  }

  close(restoreFocus = true) {
    if (!this.opened) return;
    this.opened = false;
    this.root.hidden = true;
    this.host.classList.remove("menu-open");
    if (restoreFocus && this.previousFocus instanceof HTMLElement) {
      this.previousFocus.focus({ preventScroll: true });
    }
  }

  destroy() {
    this.close(false);
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
    keyboardAvailable = true,
  }) {
    this.isVideoStreamViewer = true;
    this.entry = entry;
    this.address = address;
    this.dispatch = dispatch;
    this.inputSource = inputSource;
    this.apiJson = apiJson;
    this.apiPostJson = apiPostJson;
    this.quality = "standard";
    this.volume = 1;
    this.duration = 0;
    this.session = null;
    this.generation = null;
    this.encoder = "";
    this.codecs = "";
    this.lastState = null;
    this.timelineAnchor = { sourcePositionSecs: 0, mediaTimeSecs: 0 };
    this.playRequested = true;
    this.barsVisible = true;
    this.destroyed = false;
    this.restarting = false;
    this.draggingSeek = false;
    this.hls = null;
    this.playlistUrl = "";
    this.pollTimer = 0;
    this.waitingTimer = 0;
    this.waitingSince = null;
    this.noticeKind = "";
    this.abortController = new AbortController();
    this.pendingTimers = new Set();
    this.pointers = new Map();
    this.singlePointer = null;

    this.root = element("section", "image-viewer video-stream-viewer");
    this.stage = element("div", "viewer-stage video-stream-stage");
    this.video = element("video", "stream-video");
    this.video.playsInline = true;
    this.video.preload = "auto";
    this.video.setAttribute("playsinline", "");
    this.video.setAttribute("webkit-playsinline", "");
    this.video.setAttribute("aria-label", entry.name);
    // Native controls intentionally remain disabled. Every operation is dispatched
    // through the same command layer as touch and keyboard input.

    this.notice = element("div", "video-stream-notice");
    this.notice.hidden = true;
    this.notice.setAttribute("role", "status");
    this.notice.setAttribute("aria-live", "polite");
    this.stage.append(this.video, this.notice);

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
      () => this.menuState()
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
      this.updateCounter(Number(this.seekInput.value));
    });
    this.seekInput.addEventListener("change", (event) => {
      const target = Number(this.seekInput.value);
      this.draggingSeek = false;
      send(
        event,
        command(CommandName.MEDIA_SEEK_RELATIVE, {
          seconds: target - this.currentPosition(),
        }),
        "seek_bar"
      );
    });

    this.onTimeUpdate = () => this.updateProgress();
    this.onPlaying = () => this.clearWaiting();
    this.onCanPlay = () => this.playIfRequested();
    this.onWaiting = () => this.beginWaiting();
    this.onNativeError = () => {
      if (!this.destroyed && !this.hls) {
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
    this.video.addEventListener("waiting", this.onWaiting);
    this.video.addEventListener("error", this.onNativeError);

    this.pointerDown = (event) => this.onPointerDown(event);
    this.pointerMove = (event) => this.onPointerMove(event);
    this.pointerUp = (event) => this.onPointerUp(event, false);
    this.pointerCancel = (event) => this.onPointerUp(event, true);
    this.stage.addEventListener("pointerdown", this.pointerDown);
    this.stage.addEventListener("pointermove", this.pointerMove);
    this.stage.addEventListener("pointerup", this.pointerUp);
    this.stage.addEventListener("pointercancel", this.pointerCancel);
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

  async start(positionSecs = 0, restorePlaying = true) {
    this.playRequested = restorePlaying;
    this.clearPoll();
    this.showNotice("動画エンコーダーを準備しています。", "waiting");
    let started;
    try {
      started = await this.requestWithWaiting(() => this.apiPostJson(
        "/api/video/start",
        {
          fav: this.address.favorite_id,
          path: this.address.relative_path,
          quality: this.quality,
        },
        this.abortController.signal
      ));
    } catch (error) {
      if (error?.name === "AbortError" || this.destroyed) return;
      this.showStartFailure(error);
      return;
    }
    if (this.destroyed) return;
    this.session = started.session;
    this.generation = started.generation;
    this.playlistUrl = started.playlist;
    this.duration = Math.max(0, Number(started.duration_secs) || 0);
    this.encoder = String(started.encoder ?? "");
    this.codecs = String(started.codec ?? "");
    this.seekInput.max = String(this.duration);
    this.timelineAnchor = {
      sourcePositionSecs: 0,
      mediaTimeSecs: Number(this.video.currentTime) || 0,
    };
    this.updateDiagnostics(started);

    if (Number(positionSecs) > 0.25) {
      try {
        const sought = await this.requestWithWaiting(() => this.apiPostJson(
          "/api/video/seek",
          { session: this.session, position_secs: Math.min(positionSecs, this.duration) },
          this.abortController.signal
        ));
        this.generation = sought.generation;
        this.playlistUrl = sought.playlist;
        this.timelineAnchor.sourcePositionSecs = Math.min(positionSecs, this.duration);
      } catch (error) {
        if (error?.name === "AbortError" || this.destroyed) return;
        this.showOperationalError(error, "同じ位置から再開できませんでした");
        return;
      }
    }

    const attached = await this.attachPlaylist(this.playlistUrl);
    if (!attached || this.destroyed) return;
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
        this.showNotice(
          `${decision.message} ${error?.message ?? ""}`.trim(),
          "waiting"
        );
        await abortableDelay(decision.retryDelayMs, this.abortController.signal);
      }
    }
    const error = new Error("aborted");
    error.name = "AbortError";
    throw error;
  }

  async probePlaylist(url) {
    const result = await resolveVideoPlaylist({
      initialUrl: url,
      session: this.session,
      signal: this.abortController.signal,
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
      onDecision: (decision, detail) => {
        if (decision.retry) {
          this.showNotice(
            `${decision.message} ${detail.message ?? ""}`.trim(),
            "waiting"
          );
        }
      },
    });
    if (result.ok) {
      if (result.state) this.applyServerState(result.state);
      this.generation = result.state?.generation ?? this.generation;
      return result.url;
    }
    this.showStatusDecision(result.decision);
    return null;
  }

  async attachPlaylist(url) {
    if (!url || this.destroyed) return false;
    const resolvedUrl = await this.probePlaylist(url);
    if (!resolvedUrl) return false;
    if (this.destroyed) return false;
    url = resolvedUrl;
    this.playlistUrl = url;
    this.destroyHls();
    this.video.pause();
    this.video.removeAttribute("src");
    this.video.load();
    const playback = videoPlaybackDecision(this.video.canPlayType(HLS_MIME));
    if (playback.mode === "native") {
      this.video.src = url;
      this.video.load();
      return true;
    }

    let Hls;
    try {
      Hls = await loadHlsJs();
    } catch (error) {
      this.showOperationalError(error, "HLS 再生機能を読み込めませんでした");
      return false;
    }
    if (!Hls.isSupported()) {
      this.showNotice("このブラウザは HLS 再生に対応していません。", "error");
      return false;
    }
    if (this.destroyed) return false;
    const hls = new Hls({
      backBufferLength: 60,
      maxBufferLength: 60,
      manifestLoadingMaxRetry: 0,
      levelLoadingMaxRetry: 0,
      fragLoadingMaxRetry: 0,
      xhrSetup(xhr) { xhr.withCredentials = true; },
    });
    this.hls = hls;
    hls.on(Hls.Events.ERROR, (_event, data) => this.onHlsError(data));
    hls.loadSource(url);
    hls.attachMedia(this.video);
    return true;
  }

  onHlsError(data) {
    if (this.destroyed) return;
    const status = hlsHttpStatus(data);
    if (status) {
      const decision = videoHttpStatusDecision(
        status,
        hlsRetryAfter(data),
        hlsHttpErrorCode(data)
      );
      if (decision.kind === "waiting") {
        this.showNotice(decision.message, "waiting");
        this.hls?.stopLoad();
        this.schedule(() => this.attachPlaylist(this.playlistUrl), decision.retryDelayMs);
        return;
      }
      if (decision.kind === "generation_mismatch") {
        this.showNotice(decision.message, "waiting");
        this.refreshGeneration().catch((error) => {
          this.showOperationalError(error, "新しい配信を取得できませんでした");
        });
        return;
      }
      if (decision.kind === "gone" || decision.kind === "not_found") {
        this.hls?.stopLoad();
        this.video.pause();
        this.showStatusDecision(decision);
        return;
      }
    }
    if (!data?.fatal) return;
    this.hls?.stopLoad();
    this.showNotice(
      `再生データを読み込めません (${data?.details ?? "HLS エラー"})。`,
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
    let mediaState;
    try {
      mediaState = await this.apiJson(
        "/api/video/state",
        { session: this.session },
        this.abortController.signal
      );
    } catch (error) {
      if (
        videoHttpStatusDecision(
          error?.status,
          error?.retryAfterSeconds,
          error?.code
        ).kind === "session_mismatch"
      ) {
        await this.restartAt(this.currentPosition());
        return;
      }
      throw error;
    }
    this.applyServerState(mediaState);
    const playlist = `/stream/${this.session}/${mediaState.generation}/index.m3u8`;
    this.generation = mediaState.generation;
    await this.attachPlaylist(playlist);
    this.playIfRequested();
  }

  execute(requested) {
    if (requested.name === CommandName.MEDIA_TOGGLE_PLAY) {
      this.togglePlaying().catch((error) => {
        this.showOperationalError(error, "再生状態を変更できませんでした");
      });
      return true;
    }
    if (requested.name === CommandName.MEDIA_SEEK_RELATIVE) {
      this.seekTo(this.currentPosition() + Number(requested.payload.seconds || 0))
        .catch((error) => this.showOperationalError(error, "シークできませんでした"));
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
    const shouldPlay = this.video.paused || !this.lastState?.playing;
    await this.setPlaying(shouldPlay);
  }

  async setPlaying(playing) {
    this.playRequested = Boolean(playing);
    if (this.session && Boolean(this.lastState?.playing) !== this.playRequested) {
      await this.apiPostJson(
        "/api/video/control",
        { session: this.session, action: this.playRequested ? "play" : "pause" },
        this.abortController.signal
      );
    }
    if (this.lastState) this.lastState.playing = this.playRequested;
    if (this.playRequested) await this.playIfRequested();
    else this.video.pause();
  }

  async playIfRequested() {
    if (!this.playRequested || this.destroyed || !this.video.src && !this.hls) return;
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

  async seekTo(targetPositionSecs) {
    if (!this.session || this.destroyed) return;
    const plan = videoSeekPlan({
      targetPositionSecs,
      durationSecs: this.duration,
      anchorSourcePositionSecs: this.timelineAnchor.sourcePositionSecs,
      anchorMediaTimeSecs: this.timelineAnchor.mediaTimeSecs,
      seekableRanges: seekableRanges(this.video),
    });
    if (plan.kind === "local") {
      this.video.currentTime = plan.mediaTimeSecs;
      this.updateProgress();
      return;
    }
    this.showNotice("指定位置の配信を準備しています。", "waiting");
    let sought;
    try {
      sought = await this.requestWithWaiting(() => this.apiPostJson(
        "/api/video/seek",
        { session: this.session, position_secs: plan.positionSecs },
        this.abortController.signal
      ));
    } catch (error) {
      if (error?.status === 409) {
        await this.restartAt(plan.positionSecs);
        return;
      }
      throw error;
    }
    this.generation = sought.generation;
    this.playlistUrl = sought.playlist;
    this.timelineAnchor = {
      sourcePositionSecs: plan.positionSecs,
      mediaTimeSecs: 0,
    };
    await this.attachPlaylist(sought.playlist);
    this.playIfRequested();
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
    if (this.lastState) this.lastState.volume = volume;
    this.menu.setMediaState(this.menuState());
  }

  async setQuality(quality) {
    const preset = videoQualityPreset(quality);
    if (!this.session || this.destroyed || preset.id !== quality || quality === this.quality) return;
    this.showNotice(`${preset.label}画質へ切り替えています。`, "waiting");
    try {
      await this.apiPostJson(
        "/api/video/control",
        { session: this.session, action: "quality", quality },
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
    this.volume = Math.max(0, Math.min(1, Number(mediaState.volume) || 0));
    this.encoder = String(mediaState.encoder ?? this.encoder);
    this.codecs = String(mediaState.codecs ?? this.codecs);
    this.playRequested = Boolean(mediaState.playing);
    this.seekInput.max = String(this.duration);
    this.timelineAnchor = videoTimelineAnchor({
      serverPositionSecs: mediaState.position_secs,
      mediaCurrentTimeSecs: this.video.currentTime,
      seekableEndSecs: lastSeekableEnd(this.video),
      durationSecs: this.duration,
    });
    this.updateProgress();
    this.updateDiagnostics(mediaState);
    this.menu.setMediaState(this.menuState());
  }

  updateProgress() {
    const position = this.currentPosition();
    if (!this.draggingSeek) this.seekInput.value = String(position);
    this.updateCounter(this.draggingSeek ? Number(this.seekInput.value) : position);
  }

  updateCounter(position) {
    this.counter.value = `${formatVideoTime(position)} / ${formatVideoTime(this.duration)}`;
    this.counter.textContent = this.counter.value;
    this.seekInput.setAttribute("aria-valuetext", this.counter.value);
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
      this.encoder ? `エンコーダー ${this.encoder}` : "エンコーダー 準備中",
      dimensions,
      bitrate,
    ].filter(Boolean).join(" · ");
  }

  schedulePoll(delayMs = VIDEO_STATE_POLL_MS) {
    this.clearPoll();
    if (this.destroyed || !this.session) return;
    this.pollTimer = setTimeout(() => {
      this.pollTimer = 0;
      this.pollState().catch((error) => {
        if (error?.name !== "AbortError" && !this.destroyed) {
          this.showOperationalError(error, "配信状態を確認できませんでした");
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
      const generationChanged = Number(mediaState.generation) !== Number(this.generation);
      this.applyServerState(mediaState);
      if (generationChanged) {
        await this.attachPlaylist(
          `/stream/${this.session}/${mediaState.generation}/index.m3u8`
        );
        this.playIfRequested();
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
    if (!this.session || this.destroyed) return;
    const position = this.currentPosition();
    const restorePlaying = this.playRequested;
    try {
      const mediaState = await this.apiJson(
        "/api/video/state",
        { session: this.session },
        this.abortController.signal
      );
      if (this.destroyed) return;
      const generationChanged = Number(mediaState.generation) !== Number(this.generation);
      this.applyServerState(mediaState);
      if (generationChanged) {
        await this.attachPlaylist(
          `/stream/${this.session}/${mediaState.generation}/index.m3u8`
        );
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

  async restartAt(positionSecs) {
    if (this.restarting || this.destroyed) return;
    this.restarting = true;
    const restorePlaying = this.playRequested;
    const oldSession = this.session;
    this.clearPoll();
    this.destroyHls();
    this.video.pause();
    this.video.removeAttribute("src");
    this.video.load();
    this.session = null;
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

  clearWaiting() {
    clearTimeout(this.waitingTimer);
    this.waitingTimer = 0;
    this.waitingSince = null;
    if (this.noticeKind === "buffering" || this.noticeKind === "waiting") {
      this.hideNotice();
    }
  }

  showNotice(message, kind = "info", actionLabel = "", action = null) {
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
    const preset = videoQualityPreset(this.quality);
    const reason = error instanceof Error ? error.message : "理由を取得できませんでした。";
    this.diagnostics.textContent = [
      `画質 ${preset.label} (${preset.traffic})`,
      `エンコーダー ${this.encoder || "本体設定（選択前）"}`,
      `理由 ${reason}`,
    ].join(" · ");
    this.showNotice(
      `動画ストリーミングを開始できませんでした。${reason}`,
      "error",
      "再試行",
      () => this.start(this.currentPosition(), this.playRequested)
    );
  }

  showOperationalError(error, prefix) {
    if (error?.name === "AbortError" || this.destroyed) return;
    const reason = error instanceof Error ? error.message : "不明なエラーです。";
    this.showNotice(`${prefix}。${reason}`, "error");
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
    if (gesture === ViewerGesture.SWIPE_LEFT) {
      this.dispatch(command(CommandName.NEXT_PAGE), meta);
    } else if (gesture === ViewerGesture.SWIPE_RIGHT) {
      this.dispatch(command(CommandName.PREV_PAGE), meta);
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
    this.hls?.destroy();
    this.hls = null;
  }

  destroy() {
    if (this.destroyed) return;
    this.destroyed = true;
    this.abortController.abort();
    this.clearPoll();
    clearTimeout(this.waitingTimer);
    for (const timer of this.pendingTimers) clearTimeout(timer);
    this.pendingTimers.clear();
    this.destroyHls();
    this.video.pause();
    this.video.removeAttribute("src");
    this.video.load();
    this.video.removeEventListener("timeupdate", this.onTimeUpdate);
    this.video.removeEventListener("playing", this.onPlaying);
    this.video.removeEventListener("canplay", this.onCanPlay);
    this.video.removeEventListener("waiting", this.onWaiting);
    this.video.removeEventListener("error", this.onNativeError);
    this.stage.removeEventListener("pointerdown", this.pointerDown);
    this.stage.removeEventListener("pointermove", this.pointerMove);
    this.stage.removeEventListener("pointerup", this.pointerUp);
    this.stage.removeEventListener("pointercancel", this.pointerCancel);
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
