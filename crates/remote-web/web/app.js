import {
  CommandName,
  FitMode,
  command,
  commandFromKey,
  gridLayoutForWidth,
  gridIndexForCommand,
  nextFitMode,
  reduceViewerTransform,
  viewerTapCommand,
  viewerImageLayout,
  viewerWheelCommand,
} from "/command-core.mjs";

const app = document.querySelector("#app");
const hudElement = document.querySelector("#telemetry-hud");
const TELEMETRY_ENABLED = true;

class AuthenticationRequiredError extends Error {}

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
  thumbAspectHeightRatio: 1,
  favoriteId: null,
  favoriteName: "",
  folderPath: "",
  entries: [],
  images: [],
  imageIndex: -1,
  fitMode: FitMode.PAGE,
  imageInfoCache: new Map(),
  requestController: null,
  virtualGrid: null,
  thumbnailTracker: null,
  viewer: null,
  commandMenu: null,
  screenContext: "loading",
  gridIndex: 0,
  authCountdownTimer: 0,
};

window.addEventListener("popstate", () => dispatchRoute());
window.addEventListener("keydown", onGlobalKeyDown);

let recentPointerSource = { source: "mouse", at: 0 };
window.addEventListener(
  "pointerdown",
  (event) => {
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
boot();

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
  renderLoading("お気に入りを読み込んでいます");
  const data = await apiJson("/api/favorites");
  state.favorites = data.favorites ?? [];
  state.thumbAspectHeightRatio =
    Number.isFinite(Number(data.thumb_aspect_height_ratio)) &&
    Number(data.thumb_aspect_height_ratio) > 0
      ? Number(data.thumb_aspect_height_ratio)
      : 1;
  if (!location.hash) {
    history.replaceState({ mivRoute: true }, "", "#favorites");
  } else {
    history.replaceState({ ...(history.state ?? {}), mivRoute: true }, "", location.href);
  }
  await dispatchRoute();
}

function renderPinLogin(initialRemainingSeconds = 0) {
  cleanupScreen();
  state.screenContext = "pin";
  state.authenticated = false;
  telemetryState.authenticated = false;
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
  if (!state.favorites.length && location.hash !== "#favorites") {
    return;
  }
  const route = parseRoute(location.hash);
  try {
    if (route.kind === "folder") {
      await showFolder(route.favoriteId, route.path);
      return;
    }
    if (route.kind === "image") {
      const separator = route.path.lastIndexOf("/");
      const folderPath = separator >= 0 ? route.path.slice(0, separator) : "";
      await loadFolder(route.favoriteId, folderPath);
      const index = state.images.findIndex((entry) => entry.path === route.path);
      if (index < 0) {
        throw new Error("画像が見つかりませんでした。");
      }
      renderImageViewer(index, performance.now());
      return;
    }
    renderFavorites();
  } catch (error) {
    renderError(error);
  }
}

function parseRoute(hash) {
  if (!hash || hash === "#favorites") {
    return { kind: "favorites" };
  }
  const match = hash.match(/^#(folder|image)\/([^/]+)\/(.*)$/);
  if (!match) {
    return { kind: "favorites" };
  }
  try {
    return {
      kind: match[1],
      favoriteId: match[2],
      path: decodeURIComponent(match[3]),
    };
  } catch {
    return { kind: "favorites" };
  }
}

function folderHash(favoriteId, path) {
  return `#folder/${favoriteId}/${encodeURIComponent(path)}`;
}

function imageHash(favoriteId, path) {
  return `#image/${favoriteId}/${encodeURIComponent(path)}`;
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

function dispatchCommand(requested, meta = {}) {
  if (!requested?.name || !state.authenticated) return false;
  const source = meta.source ?? "mouse";
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
      const viewerDepth = Number(history.state?.viewerDepth) || 0;
      if (history.state?.viewerFromGrid && viewerDepth > 0) {
        history.go(-viewerDepth);
      } else {
        history.replaceState(
          { mivRoute: true },
          "",
          folderHash(state.favoriteId, state.folderPath)
        );
        dispatchRoute();
      }
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
    const target = state.folderPath
      ? folderHash(state.favoriteId, parentPath(state.folderPath))
      : "#favorites";
    navigate(target);
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
    if (requested.name === CommandName.NEXT_PAGE) handled = changeImage(1);
    else if (requested.name === CommandName.PREV_PAGE) handled = changeImage(-1);
    else if (requested.name === CommandName.FIRST_PAGE) handled = changeImageTo(0);
    else if (requested.name === CommandName.LAST_PAGE) {
      handled = changeImageTo(state.images.length - 1);
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
        updateViewerImage(performance.now()).catch(renderError);
        handled = true;
      } else {
        handled = Boolean(state.viewer?.execute(requested));
      }
    }
  }

  if (handled && meta.telemetry !== false) {
    enqueueTelemetry({
      type: "command",
      command: requested.name,
      input_source: source,
      input_detail: meta.detail ? limitText(meta.detail, 80) : undefined,
      context,
    });
  }
  return handled;
}

function executeOpenCommand(payload, meta) {
  if (payload.kind === "favorite" || payload.kind === "folder") {
    navigate(folderHash(payload.favoriteId, payload.path ?? ""));
    return true;
  }
  if (
    payload.kind !== "image" ||
    !Number.isInteger(payload.imageIndex) ||
    payload.imageIndex < 0 ||
    payload.imageIndex >= state.images.length
  ) {
    return false;
  }
  if (Number.isInteger(payload.entryIndex)) {
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

function openGridEntry(index, meta) {
  const entry = state.entries[index];
  if (!entry) return false;
  if (entry.kind === "dir") {
    return executeOpenCommand(
      { kind: "folder", favoriteId: state.favoriteId, path: entry.path },
      meta
    );
  }
  const imageIndex = state.images.findIndex((image) => image.path === entry.path);
  return executeOpenCommand(
    { kind: "image", path: entry.path, imageIndex, entryIndex: index },
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
    },
    state.screenContext
  );
  if (!requested) return;
  event.preventDefault();
  dispatchCommand(requested, { source: "keyboard", detail: event.key });
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

function menuCommand(event, name, payload = {}) {
  dispatchCommand(command(name, payload), {
    source: inputSourceFromEvent(event),
    detail: "menu",
  });
}

function cleanupScreen() {
  clearInterval(state.authCountdownTimer);
  state.authCountdownTimer = 0;
  state.requestController?.abort();
  state.requestController = null;
  state.virtualGrid?.destroy();
  state.virtualGrid = null;
  state.thumbnailTracker?.destroy();
  state.thumbnailTracker = null;
  state.commandMenu?.destroy();
  state.commandMenu = null;
  state.viewer?.destroy();
  state.viewer = null;
  state.screenContext = "loading";
  app.replaceChildren();
}

function renderFavorites() {
  cleanupScreen();
  state.screenContext = "favorites";
  exitBrowserFullscreen();
  document.title = "mIV Remote";

  const screen = element("section", "screen");
  const content = element("div", "page-content");
  const hero = element("header", "hero hero-with-menu");
  const heroText = element("div");
  heroText.append(
    textElement("h1", "mIV Remote"),
    textElement("p", "お気に入りから閲覧するフォルダを選んでください。")
  );
  hero.append(
    heroText,
    createMenuButton("操作メニュー")
  );
  content.append(hero);

  if (!state.favorites.length) {
    content.append(
      textElement("p", "お気に入りが登録されていません。", "empty-state")
    );
  } else {
    const list = element("div", "favorite-list");
    for (const favorite of state.favorites) {
      const button = element("button", "favorite-card");
      button.type = "button";
      button.append(
        textElement("span", "◆", "favorite-icon"),
        textElement("span", favorite.name, "favorite-name"),
        textElement("span", "›", "favorite-arrow")
      );
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
  screen.append(content);
  state.commandMenu = new CommandMenu(screen, "favorites");
  app.append(screen);
}

async function showFolder(favoriteId, path) {
  renderLoading("フォルダを読み込んでいます");
  await loadFolder(favoriteId, path);
  renderFolder();
}

async function loadFolder(favoriteId, path) {
  const requestedPath = path ?? "";
  const sameFolder =
    state.favoriteId === favoriteId && state.folderPath === requestedPath;
  state.requestController?.abort();
  const controller = new AbortController();
  state.requestController = controller;
  const data = await apiJson(
    "/api/list",
    { fav: favoriteId, path },
    controller.signal
  );
  if (controller.signal.aborted) {
    return;
  }
  state.requestController = null;
  state.favoriteId = favoriteId;
  state.favoriteName =
    state.favorites.find((favorite) => favorite.id === favoriteId)?.name ?? "お気に入り";
  state.folderPath = data.path ?? "";
  // ZIP/PDF/video/audio remain API classifications only in this PoC. The
  // browsing UI intentionally exposes directories and ordinary images.
  state.entries = (data.entries ?? []).filter(
    (entry) => entry.kind === "dir" || entry.kind === "image"
  );
  state.images = state.entries.filter((entry) => entry.kind === "image");
  state.gridIndex = sameFolder
    ? clamp(state.gridIndex, 0, Math.max(0, state.entries.length - 1))
    : 0;
}

function renderFolder() {
  const renderStartedAt = performance.now();
  cleanupScreen();
  state.screenContext = "grid";
  exitBrowserFullscreen();
  document.title = `${state.favoriteName} — mIV Remote`;

  const screen = element("section", "screen");
  const topbar = element("header", "topbar");
  const back = textElement("button", "‹", "icon-button");
  back.type = "button";
  back.setAttribute("aria-label", "戻る");
  back.addEventListener("click", (event) => {
    dispatchCommand(command(CommandName.PARENT_FOLDER), {
      source: inputSourceFromEvent(event),
      detail: "toolbar",
    });
  });
  topbar.append(back, buildBreadcrumbs(), createMenuButton("操作メニュー"));

  const scroll = element("div", "grid-scroll");
  const space = element("div", "virtual-space");
  const windowElement = element("div", "virtual-window");
  space.append(windowElement);
  scroll.append(space);
  screen.append(topbar, scroll);
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
      "このフォルダには表示できるサブフォルダまたは画像がありません。",
      "empty-state center-status"
    );
    scroll.replaceChildren(empty);
    state.thumbnailTracker.begin([]);
    return;
  }

  const imageIndexes = new Map(state.images.map((entry, index) => [entry.path, index]));
  state.virtualGrid = new VirtualGrid(
    scroll,
    space,
    windowElement,
    state.entries,
    (entry, index) => createGridTile(entry, index, imageIndexes, state.thumbnailTracker),
    (initialItems) => state.thumbnailTracker?.begin(initialItems),
    state.thumbAspectHeightRatio
  );
}

function buildBreadcrumbs() {
  const breadcrumbs = element("nav", "breadcrumbs");
  breadcrumbs.setAttribute("aria-label", "パンくず");
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

function createGridTile(entry, entryIndex, imageIndexes, thumbnailTracker) {
  const tile = element("button", "grid-tile");
  tile.type = "button";
  tile.title = entry.name;
  tile.dataset.entryIndex = String(entryIndex);
  tile.classList.toggle("grid-active", entryIndex === state.gridIndex);
  tile.addEventListener("focus", () => {
    dispatchCommand(command(CommandName.GRID_SELECT, { index: entryIndex }), {
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

  if (entry.kind === "dir") {
    preview.append(textElement("span", "◆", "folder-glyph"));
    preview.append(image);
    preview.append(textElement("span", "folder", "type-badge"));
    loadThumbnail(image, entry, thumbnailTracker);
    tile.addEventListener("click", (event) => {
      dispatchCommand(
        command(CommandName.OPEN, {
          kind: "folder",
          favoriteId: state.favoriteId,
          path: entry.path,
          entryIndex,
        }),
        { source: inputSourceFromEvent(event), detail: "grid_tile" }
      );
    });
  } else {
    preview.append(textElement("span", "◇", "file-glyph"));
    loadThumbnail(image, entry, thumbnailTracker);
    preview.append(image);
    tile.addEventListener("click", (event) => {
      const index = imageIndexes.get(entry.path);
      if (index !== undefined) {
        dispatchCommand(
          command(CommandName.OPEN, {
            kind: "image",
            path: entry.path,
            imageIndex: index,
            entryIndex,
          }),
          {
            source: inputSourceFromEvent(event),
            detail: "grid_tile",
            at: performance.now(),
          }
        );
      }
    });
  }
  tile.append(preview, textElement("span", entry.name, "tile-label"));
  return tile;
}

function renderImageViewer(index, interactionStartedAt = performance.now()) {
  cleanupScreen();
  state.screenContext = "viewer";
  state.imageIndex = index;
  const imageEntry = state.images[index];
  document.title = `${imageEntry.name} — mIV Remote`;

  const viewerRoot = element("section", "image-viewer");
  const stage = element("div", "viewer-stage");
  const image = element("img", "viewer-image");
  image.alt = imageEntry.name;
  image.draggable = false;
  image.dataset.telemetryObserved = "true";
  stage.append(image);

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
  const counter = textElement("span", "", "viewer-counter");
  const next = textElement("button", "›", "viewer-button");
  next.type = "button";
  next.setAttribute("aria-label", "次の画像");
  bottom.append(previous, counter, next);
  viewerRoot.append(stage, top, bottom);
  state.commandMenu = new CommandMenu(viewerRoot, "viewer", viewerRoot);
  app.append(viewerRoot);

  state.viewer = new ImageViewer({
    root: viewerRoot,
    stage,
    image,
    title,
    counter,
    previous,
    next,
  });
  close.addEventListener("click", (event) => {
    event.stopPropagation();
    dispatchCommand(command(CommandName.BACK), {
      source: inputSourceFromEvent(event),
      detail: "toolbar",
    });
  });
  previous.addEventListener("click", (event) => {
    event.stopPropagation();
    dispatchCommand(command(CommandName.PREV_PAGE), {
      source: inputSourceFromEvent(event),
      detail: "toolbar",
    });
  });
  next.addEventListener("click", (event) => {
    event.stopPropagation();
    dispatchCommand(command(CommandName.NEXT_PAGE), {
      source: inputSourceFromEvent(event),
      detail: "toolbar",
    });
  });
  updateViewerImage(interactionStartedAt).catch(renderError);
}

function changeImage(delta) {
  return changeImageTo(state.imageIndex + delta);
}

function changeImageTo(nextIndex) {
  if (nextIndex < 0 || nextIndex >= state.images.length) {
    return false;
  }
  if (nextIndex === state.imageIndex) return false;
  state.imageIndex = nextIndex;
  const entry = state.images[nextIndex];
  const viewerDepth = (Number(history.state?.viewerDepth) || 0) + 1;
  history.pushState(
    {
      ...(history.state ?? {}),
      mivRoute: true,
      viewerFromGrid: Boolean(history.state?.viewerFromGrid),
      viewerDepth,
    },
    "",
    imageHash(state.favoriteId, entry.path)
  );
  updateViewerImage(performance.now()).catch(renderError);
  return true;
}

async function updateViewerImage(interactionStartedAt = performance.now()) {
  const entry = state.images[state.imageIndex];
  const viewer = state.viewer;
  if (!entry || !viewer) return;
  document.title = `${entry.name} — mIV Remote`;
  const info = await imageInfo(entry.path);
  if (state.viewer !== viewer || state.images[state.imageIndex]?.path !== entry.path) return;
  const request = imageRequest(entry.path, info, viewer.stage);
  viewer.load({
    name: entry.name,
    request,
    info,
    fitMode: state.fitMode,
    index: state.imageIndex,
    count: state.images.length,
    interactionStartedAt,
  });
  const nextEntry = state.images[state.imageIndex + 1];
  if (nextEntry) {
    imageInfo(nextEntry.path).then((nextInfo) => {
      if (state.viewer !== viewer) return;
      const preload = new Image();
      preload.decoding = "async";
      preload.src = imageRequest(nextEntry.path, nextInfo, viewer.stage).url;
    }).catch(() => {});
  }
}

function imageRequest(path, info, stage) {
  const dpr = window.devicePixelRatio || 1;
  const layout = viewerImageLayout({
    mode: state.fitMode,
    sourceWidth: info.width,
    sourceHeight: info.height,
    viewportWidth: stage.clientWidth || window.innerWidth,
    viewportHeight: stage.clientHeight || window.innerHeight,
    devicePixelRatio: dpr,
  });
  return {
    url: apiUrl("/api/image", { fav: state.favoriteId, path, w: layout.requestWidth }),
    width: layout.requestWidth,
    cssWidth: layout.cssWidth,
    dpr,
    layout,
    fitMode: state.fitMode,
  };
}

function imageInfo(path) {
  const entry = state.images.find((candidate) => candidate.path === path);
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

async function loadThumbnail(image, entry, tracker) {
  const url = apiUrl("/api/thumb", {
    fav: state.favoriteId,
    path: entry.path,
  });
  try {
    const response = await observedFetch(url, {
      credentials: "same-origin",
      cache: "force-cache",
    });
    if (!response.ok) {
      image.classList.add("thumb-missing");
      tracker?.settled(entry.path, { notFound: response.status === 404 });
      return;
    }
    const blob = await response.blob();
    const objectUrl = URL.createObjectURL(blob);
    image.src = objectUrl;
    try {
      await image.decode();
      await nextFrame();
      image.classList.add("thumb-ready");
      tracker?.settled(entry.path);
    } finally {
      URL.revokeObjectURL(objectUrl);
    }
  } catch (error) {
    image.classList.add("thumb-missing");
    tracker?.settled(entry.path);
    recordClientError("image_load_error", error, {
      resource: safeResourcePath(url),
    });
  }
}

class VirtualGrid {
  constructor(
    scroller,
    space,
    windowElement,
    items,
    renderCell,
    onInitialItems,
    aspectHeightRatio
  ) {
    this.scroller = scroller;
    this.space = space;
    this.windowElement = windowElement;
    this.items = items;
    this.renderCell = renderCell;
    this.onInitialItems = onInitialItems;
    this.aspectHeightRatio = aspectHeightRatio;
    this.initialItemsReported = false;
    this.cells = new Map();
    this.columns = 1;
    this.rowHeight = 1;
    this.cellHeight = 1;
    this.gap = 0;
    this.lastRange = "";
    this.frame = 0;
    this.onScroll = () => this.schedule();
    this.resizeObserver = new ResizeObserver(() => this.layout());
    this.scroller.addEventListener("scroll", this.onScroll, { passive: true });
    this.resizeObserver.observe(this.scroller);
    this.layout();
  }

  layout() {
    const layout = gridLayoutForWidth(
      this.scroller.clientWidth,
      this.aspectHeightRatio
    );
    if (
      layout.columns !== this.columns ||
      layout.rowPitch !== this.rowHeight
    ) {
      this.columns = layout.columns;
      this.rowHeight = layout.rowPitch;
      this.lastRange = "";
    }
    this.cellHeight = layout.cellHeight;
    this.gap = layout.gap;
    const rows = Math.ceil(this.items.length / this.columns);
    this.space.style.height = `${Math.max(
      this.scroller.clientHeight,
      rows * this.rowHeight
    )}px`;
    this.windowElement.style.left = `${layout.inset}px`;
    this.windowElement.style.right = `${layout.inset}px`;
    this.windowElement.style.gap = `${layout.gap}px`;
    this.windowElement.style.gridTemplateColumns = `repeat(${this.columns}, minmax(0, 1fr))`;
    this.windowElement.style.gridAutoRows = `${this.cellHeight}px`;
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
    const firstRow = Math.max(0, Math.floor(this.scroller.scrollTop / this.rowHeight) - overscan);
    const totalRows = Math.ceil(this.items.length / this.columns);
    const endRow = Math.min(totalRows, firstRow + visibleRows + overscan * 2);
    const startIndex = firstRow * this.columns;
    const endIndex = Math.min(this.items.length, endRow * this.columns);
    const range = `${startIndex}:${endIndex}:${this.columns}`;
    if (range === this.lastRange) return;
    this.lastRange = range;
    if (!this.initialItemsReported) {
      this.initialItemsReported = true;
      this.onInitialItems?.(this.items.slice(startIndex, endIndex));
    }
    this.windowElement.style.top = `${firstRow * this.rowHeight + this.gap / 2}px`;
    const fragment = document.createDocumentFragment();
    for (let index = startIndex; index < endIndex; index += 1) {
      let cell = this.cells.get(index);
      if (!cell) {
        cell = this.renderCell(this.items[index], index);
        this.cells.set(index, cell);
      }
      fragment.append(cell);
    }
    this.windowElement.replaceChildren(fragment);
    const cacheLimit = Math.max(128, (endIndex - startIndex) * 4);
    if (this.cells.size > cacheLimit) {
      const center = (startIndex + endIndex) / 2;
      const candidates = [...this.cells.keys()]
        .filter((index) => index < startIndex || index >= endIndex)
        .sort((left, right) => Math.abs(right - center) - Math.abs(left - center));
      while (this.cells.size > cacheLimit && candidates.length) {
        this.cells.delete(candidates.shift());
      }
    }
  }

  visibleRowCount() {
    return Math.max(1, Math.floor(this.scroller.clientHeight / this.rowHeight));
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
      this.scroller.scrollTop = bottom - this.scroller.clientHeight;
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
    this.scroller.removeEventListener("scroll", this.onScroll);
    this.resizeObserver.disconnect();
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
      if (entry.kind === "image" || entry.kind === "dir") this.pending.add(entry.path);
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

function menuDefinition(context) {
  if (context === "viewer") {
    return {
      title: "画像の操作",
      actions: [
        [CommandName.PREV_PAGE, "前の画像", "← / ↑ / PageUp"],
        [CommandName.NEXT_PAGE, "次の画像", "→ / ↓ / PageDown"],
        [CommandName.ZOOM_IN, "拡大", "+"],
        [CommandName.ZOOM_OUT, "縮小", "−"],
        [CommandName.ZOOM_RESET, "ズームを戻す", "メニュー"],
        [CommandName.FIT_PAGE, "全体フィット", "0 で切替"],
        [CommandName.FIT_WIDTH, "幅フィット", "0 で切替"],
        [CommandName.FIT_ORIGINAL, "原寸 (100%)", "0 で切替"],
        [CommandName.FIRST_PAGE, "先頭の画像", "Home"],
        [CommandName.LAST_PAGE, "最後の画像", "End"],
        [CommandName.TOGGLE_FULLSCREEN, "全画面表示", "F11"],
        [CommandName.BACK, "一覧へ戻る", "Backspace / Enter / Esc"],
      ],
      shortcuts: [
        ["前 / 次", "← ↑ PageUp / → ↓ PageDown"],
        ["ズーム", "+ / −"],
        ["表示モード", "0 (全体 → 幅 → 原寸)"],
        ["操作メニュー", "?"],
        ["先頭 / 最後", "Home / End"],
        ["一覧へ戻る", "Backspace / Enter / Esc"],
        ["全画面", "F11"],
      ],
    };
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
    actions: [[CommandName.TOGGLE_FULLSCREEN, "全画面表示", "F11"]],
    shortcuts: [
      ["操作メニュー", "?"],
      ["全画面", "F11"],
    ],
  };
}

class CommandMenu {
  constructor(host, context, owner = host) {
    this.owner = owner;
    this.opened = false;
    this.previousFocus = null;
    const definition = menuDefinition(context);
    this.root = element("div", "command-menu-layer");
    this.root.hidden = true;

    const scrim = element("button", "command-menu-scrim");
    scrim.type = "button";
    scrim.setAttribute("aria-label", "操作メニューを閉じる");
    scrim.addEventListener("click", (event) => menuCommand(event, CommandName.TOGGLE_MENU));

    const panel = element("section", "command-menu");
    panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-modal", "true");
    panel.setAttribute("aria-label", definition.title);
    const header = element("header", "command-menu-header");
    const close = textElement("button", "×", "command-menu-close");
    close.type = "button";
    close.setAttribute("aria-label", "操作メニューを閉じる");
    close.addEventListener("click", (event) => menuCommand(event, CommandName.TOGGLE_MENU));
    header.append(textElement("h2", definition.title), close);
    this.closeButton = close;

    const actions = element("div", "command-menu-actions");
    actions.setAttribute("role", "menu");
    for (const [name, label, keys] of definition.actions) {
      const button = element("button", "command-menu-action");
      button.type = "button";
      button.setAttribute("role", "menuitem");
      button.append(textElement("span", label), textElement("kbd", keys));
      button.addEventListener("click", (event) => {
        this.close(false);
        menuCommand(event, name);
      });
      actions.append(button);
    }

    const shortcutTitle = textElement("h3", "有効なキー", "command-shortcut-title");
    const shortcuts = element("dl", "command-shortcuts");
    for (const [label, keys] of definition.shortcuts) {
      shortcuts.append(textElement("dt", label), textElement("dd", keys));
    }
    panel.append(header, actions, shortcutTitle, shortcuts);
    this.root.append(scrim, panel);
    host.append(this.root);
  }

  isOpen() {
    return this.opened;
  }

  toggle() {
    if (this.opened) this.close();
    else this.open();
    return true;
  }

  open() {
    if (this.opened) return;
    this.opened = true;
    this.previousFocus = document.activeElement;
    this.root.hidden = false;
    this.owner.classList.add("menu-open");
    requestAnimationFrame(() => this.closeButton.focus());
  }

  close(restoreFocus = true) {
    if (!this.opened) return;
    this.opened = false;
    this.root.hidden = true;
    this.owner.classList.remove("menu-open");
    if (restoreFocus && this.previousFocus instanceof HTMLElement) {
      this.previousFocus.focus({ preventScroll: true });
    }
  }

  destroy() {
    this.close(false);
    this.root.remove();
  }
}

class ImageViewer {
  constructor({ root, stage, image, title, counter, previous, next }) {
    this.root = root;
    this.stage = stage;
    this.image = image;
    this.title = title;
    this.counter = counter;
    this.previous = previous;
    this.next = next;
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

  load({ name, request, info, fitMode, index, count, interactionStartedAt }) {
    this.resetTransform();
    this.setLayout(fitMode, request.layout, info);
    this.title.textContent = name;
    this.image.alt = name;
    this.counter.textContent = `${index + 1} / ${count}`;
    this.previous.disabled = index === 0;
    this.next.disabled = index === count - 1;
    this.loadMeasuredImage(request, interactionStartedAt, name);
  }

  setLayout(fitMode, layout, info) {
    this.fitMode = fitMode;
    this.stage.dataset.fitMode = fitMode;
    this.image.style.width = `${layout.cssWidth}px`;
    this.image.style.height = "auto";
    this.image.style.maxWidth = "none";
    this.image.style.maxHeight = "none";
    this.image.dataset.sourceWidth = String(info.width);
    this.image.dataset.sourceHeight = String(info.height);
    this.stage.scrollTop = 0;
    this.stage.scrollLeft = 0;
  }

  async loadMeasuredImage(request, interactionStartedAt, name) {
    const sequence = ++this.loadSequence;
    this.fetchController?.abort();
    const controller = new AbortController();
    this.fetchController = controller;
    const fetchStartedAt = performance.now();
    let pendingObjectUrl = null;
    try {
      const response = await observedFetch(request.url, {
        signal: controller.signal,
        credentials: "same-origin",
      });
      if (!response.ok) {
        throw new Error(`画像取得に失敗しました (HTTP ${response.status})。`);
      }
      const blob = await response.blob();
      const fetchMs = performance.now() - fetchStartedAt;
      const requestId = response.headers.get("X-mIV-Request-Id");
      if (sequence !== this.loadSequence) return;

      pendingObjectUrl = URL.createObjectURL(blob);
      this.image.src = pendingObjectUrl;
      const decodeStartedAt = performance.now();
      await this.image.decode();
      const decodeMs = performance.now() - decodeStartedAt;
      await nextFrame();
      if (sequence !== this.loadSequence) {
        URL.revokeObjectURL(pendingObjectUrl);
        return;
      }
      if (this.objectUrl) URL.revokeObjectURL(this.objectUrl);
      this.objectUrl = pendingObjectUrl;
      pendingObjectUrl = null;

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
      };
      enqueueTelemetry(event);
      hudState.lastImage = event;
      hudState.displayDurations.push(event.tap_to_display_ms);
      if (hudState.displayDurations.length > 20) hudState.displayDurations.shift();
      updateHud();
    } catch (error) {
      if (pendingObjectUrl) URL.revokeObjectURL(pendingObjectUrl);
      if (sequence !== this.loadSequence) return;
      if (error?.name === "AbortError") return;
      recordClientError("image_load_error", error, {
        resource: safeResourcePath(request.url),
      });
    }
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
    this.image.style.transform = `translate3d(${this.panX}px, ${this.panY}px, 0) scale(${this.scale})`;
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
      this.stage.scrollTop -= event.clientY - previous.y;
      this.single.lastX = event.clientX;
      this.single.lastY = event.clientY;
      this.single.moved = true;
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
      };
      this.pinch = null;
      return;
    }
    if (this.pointers.size > 0) return;

    const source = pointerInputSource(event.pointerType);
    if (!cancelled && !this.pinched && single) {
      const dx = event.clientX - single.startX;
      const dy = event.clientY - single.startY;
      const elapsed = performance.now() - single.startedAt;
      if (
        this.scale <= 1.01 &&
        !single.edgeGuarded &&
        Math.abs(dx) > 52 &&
        Math.abs(dx) > Math.abs(dy) * 1.25
      ) {
        dispatchCommand(
          command(dx < 0 ? CommandName.NEXT_PAGE : CommandName.PREV_PAGE),
          { source, detail: "swipe" }
        );
      } else if (!single.moved && Math.hypot(dx, dy) < 12 && elapsed < 450) {
        dispatchCommand(viewerTapCommand(event.clientX, this.root.clientWidth), {
          source,
          detail: "tap_zone",
        });
      } else if (single.moved) {
        dispatchCommand(command(CommandName.PAN_BY, { dx: 0, dy: 0 }), {
          source,
          detail: "pan",
        });
      }
    } else if (!cancelled && this.pinched) {
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
    this.loadSequence += 1;
    this.fetchController?.abort();
    if (this.objectUrl) URL.revokeObjectURL(this.objectUrl);
    this.objectUrl = null;
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
    throw new Error(`読み込みに失敗しました (HTTP ${response.status})。`);
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

function renderLoading(message) {
  cleanupScreen();
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
  const home = textElement("button", "お気に入りへ戻る", "icon-button");
  home.type = "button";
  home.addEventListener("click", () => navigate("#favorites"));
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
