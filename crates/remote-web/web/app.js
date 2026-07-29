const app = document.querySelector("#app");
const hudElement = document.querySelector("#telemetry-hud");
const TELEMETRY_ENABLED = true;

const telemetryState = {
  queue: [],
  flushing: false,
};

const hudState = {
  lastImage: null,
  lastGrid: null,
  displayDurations: [],
  errors: [],
};

const state = {
  favorites: [],
  favoriteId: null,
  favoriteName: "",
  folderPath: "",
  entries: [],
  images: [],
  imageIndex: -1,
  requestController: null,
  virtualGrid: null,
  thumbnailTracker: null,
  viewer: null,
};

window.addEventListener("popstate", () => dispatchRoute());

if (TELEMETRY_ENABLED) {
  installTelemetry();
} else {
  hudElement.hidden = true;
}
boot();

async function boot() {
  renderLoading("お気に入りを読み込んでいます");
  try {
    const data = await apiJson("/api/favorites");
    state.favorites = data.favorites ?? [];
    if (!location.hash) {
      history.replaceState(null, "", "#favorites");
    }
    await dispatchRoute();
  } catch (error) {
    renderError(error);
  }
}

async function dispatchRoute() {
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

function navigate(hash) {
  if (location.hash === hash) {
    dispatchRoute();
    return;
  }
  history.pushState(null, "", hash);
  dispatchRoute();
}

function cleanupScreen() {
  state.requestController?.abort();
  state.requestController = null;
  state.virtualGrid?.destroy();
  state.virtualGrid = null;
  state.thumbnailTracker?.destroy();
  state.thumbnailTracker = null;
  state.viewer?.destroy();
  state.viewer = null;
  app.replaceChildren();
}

function renderFavorites() {
  cleanupScreen();
  exitBrowserFullscreen();
  document.title = "mIV Remote";

  const screen = element("section", "screen");
  const content = element("div", "page-content");
  const hero = element("header", "hero");
  hero.append(
    textElement("h1", "mIV Remote"),
    textElement("p", "お気に入りから閲覧するフォルダを選んでください。")
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
      button.addEventListener("click", () => navigate(folderHash(favorite.id, "")));
      list.append(button);
    }
    content.append(list);
  }
  screen.append(content);
  app.append(screen);
}

async function showFolder(favoriteId, path) {
  renderLoading("フォルダを読み込んでいます");
  await loadFolder(favoriteId, path);
  renderFolder();
}

async function loadFolder(favoriteId, path) {
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
}

function renderFolder() {
  const renderStartedAt = performance.now();
  cleanupScreen();
  exitBrowserFullscreen();
  document.title = `${state.favoriteName} — mIV Remote`;

  const screen = element("section", "screen");
  const topbar = element("header", "topbar");
  const back = textElement("button", "‹", "icon-button");
  back.type = "button";
  back.setAttribute("aria-label", "戻る");
  back.addEventListener("click", () => {
    const parent = parentPath(state.folderPath);
    if (state.folderPath) {
      navigate(folderHash(state.favoriteId, parent));
    } else {
      navigate("#favorites");
    }
  });
  topbar.append(back, buildBreadcrumbs());

  const scroll = element("div", "grid-scroll");
  const space = element("div", "virtual-space");
  const windowElement = element("div", "virtual-window");
  space.append(windowElement);
  scroll.append(space);
  screen.append(topbar, scroll);
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
    (entry) => createGridTile(entry, imageIndexes, state.thumbnailTracker),
    (initialItems) => state.thumbnailTracker?.begin(initialItems)
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
    button.addEventListener("click", () =>
      navigate(folderHash(state.favoriteId, crumb.path))
    );
    breadcrumbs.append(button);
  });
  requestAnimationFrame(() => {
    breadcrumbs.scrollLeft = breadcrumbs.scrollWidth;
  });
  return breadcrumbs;
}

function createGridTile(entry, imageIndexes, thumbnailTracker) {
  const tile = element("button", "grid-tile");
  tile.type = "button";
  tile.title = entry.name;
  const preview = element("span", "tile-preview");

  if (entry.kind === "dir") {
    preview.append(textElement("span", "◆", "folder-glyph"));
    preview.append(textElement("span", "folder", "type-badge"));
    tile.addEventListener("click", () =>
      navigate(folderHash(state.favoriteId, entry.path))
    );
  } else {
    preview.append(textElement("span", "◇", "file-glyph"));
    const image = document.createElement("img");
    image.alt = "";
    image.loading = "lazy";
    image.decoding = "async";
    image.dataset.telemetryObserved = "true";
    loadThumbnail(image, entry, thumbnailTracker);
    preview.append(image);
    tile.addEventListener("click", () => {
      const interactionStartedAt = performance.now();
      tryEnterBrowserFullscreen();
      const index = imageIndexes.get(entry.path);
      if (index !== undefined) {
        history.pushState(null, "", imageHash(state.favoriteId, entry.path));
        renderImageViewer(index, interactionStartedAt);
      }
    });
  }
  tile.append(preview, textElement("span", entry.name, "tile-label"));
  return tile;
}

function renderImageViewer(index, interactionStartedAt = performance.now()) {
  cleanupScreen();
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
  top.append(close, title);

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
  app.append(viewerRoot);

  state.viewer = new ImageViewer({
    root: viewerRoot,
    image,
    title,
    counter,
    previous,
    next,
    onClose: () => navigate(folderHash(state.favoriteId, state.folderPath)),
    onStep: (delta) => changeImage(delta),
  });
  close.addEventListener("click", (event) => {
    event.stopPropagation();
    navigate(folderHash(state.favoriteId, state.folderPath));
  });
  previous.addEventListener("click", (event) => {
    event.stopPropagation();
    changeImage(-1);
  });
  next.addEventListener("click", (event) => {
    event.stopPropagation();
    changeImage(1);
  });
  updateViewerImage(interactionStartedAt);
}

function changeImage(delta) {
  const nextIndex = state.imageIndex + delta;
  if (nextIndex < 0 || nextIndex >= state.images.length) {
    return;
  }
  state.imageIndex = nextIndex;
  const entry = state.images[nextIndex];
  history.pushState(null, "", imageHash(state.favoriteId, entry.path));
  updateViewerImage(performance.now());
}

function updateViewerImage(interactionStartedAt = performance.now()) {
  const entry = state.images[state.imageIndex];
  document.title = `${entry.name} — mIV Remote`;
  const request = imageRequest(entry.path);
  state.viewer.load({
    name: entry.name,
    request,
    index: state.imageIndex,
    count: state.images.length,
    interactionStartedAt,
  });
  const nextEntry = state.images[state.imageIndex + 1];
  if (nextEntry) {
    const preload = new Image();
    preload.decoding = "async";
    preload.src = imageRequest(nextEntry.path).url;
  }
}

function imageRequest(path) {
  const cssWidth = Math.max(320, window.visualViewport?.width ?? window.innerWidth);
  const dpr = window.devicePixelRatio || 1;
  const width = Math.min(4096, Math.ceil(cssWidth * dpr));
  return {
    url: apiUrl("/api/image", { fav: state.favoriteId, path, w: width }),
    width,
    cssWidth,
    dpr,
  };
}

async function loadThumbnail(image, entry, tracker) {
  const url = apiUrl("/api/thumb", {
    fav: state.favoriteId,
    path: entry.path,
  });
  try {
    const response = await observedFetch(url, { credentials: "same-origin" });
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
  constructor(scroller, space, windowElement, items, renderCell, onInitialItems) {
    this.scroller = scroller;
    this.space = space;
    this.windowElement = windowElement;
    this.items = items;
    this.renderCell = renderCell;
    this.onInitialItems = onInitialItems;
    this.initialItemsReported = false;
    this.columns = 1;
    this.rowHeight = 180;
    this.lastRange = "";
    this.frame = 0;
    this.onScroll = () => this.schedule();
    this.resizeObserver = new ResizeObserver(() => this.layout());
    this.scroller.addEventListener("scroll", this.onScroll, { passive: true });
    this.resizeObserver.observe(this.scroller);
    this.layout();
  }

  layout() {
    const width = Math.max(1, this.scroller.clientWidth - 20);
    const minCellWidth = width < 420 ? 128 : 148;
    const columns = Math.max(1, Math.floor((width + 12) / (minCellWidth + 12)));
    if (columns !== this.columns) {
      this.columns = columns;
      this.lastRange = "";
    }
    const rows = Math.ceil(this.items.length / this.columns);
    this.space.style.height = `${Math.max(this.scroller.clientHeight, rows * this.rowHeight + 12)}px`;
    this.windowElement.style.gridTemplateColumns = `repeat(${this.columns}, minmax(0, 1fr))`;
    this.windowElement.style.gridAutoRows = `${this.rowHeight - 12}px`;
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
    this.windowElement.style.top = `${firstRow * this.rowHeight + 6}px`;
    const fragment = document.createDocumentFragment();
    for (let index = startIndex; index < endIndex; index += 1) {
      fragment.append(this.renderCell(this.items[index]));
    }
    this.windowElement.replaceChildren(fragment);
  }

  destroy() {
    cancelAnimationFrame(this.frame);
    this.scroller.removeEventListener("scroll", this.onScroll);
    this.resizeObserver.disconnect();
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
      if (entry.kind === "image") this.pending.add(entry.path);
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

class ImageViewer {
  constructor({ root, image, title, counter, previous, next, onClose, onStep }) {
    this.root = root;
    this.image = image;
    this.title = title;
    this.counter = counter;
    this.previous = previous;
    this.next = next;
    this.onClose = onClose;
    this.onStep = onStep;
    this.scale = 1;
    this.panX = 0;
    this.panY = 0;
    this.pointers = new Map();
    this.single = null;
    this.pinch = null;
    this.pinched = false;
    this.resizeTimer = 0;
    this.loadSequence = 0;
    this.fetchController = null;
    this.objectUrl = null;

    this.pointerDown = (event) => this.onPointerDown(event);
    this.pointerMove = (event) => this.onPointerMove(event);
    this.pointerUp = (event) => this.onPointerUp(event);
    this.wheel = (event) => this.onWheel(event);
    this.keyDown = (event) => this.onKeyDown(event);
    this.resize = () => {
      clearTimeout(this.resizeTimer);
      this.resizeTimer = setTimeout(() => {
        const entry = state.images[state.imageIndex];
        if (entry) {
          this.loadMeasuredImage(
            imageRequest(entry.path),
            performance.now(),
            entry.name
          );
        }
      }, 180);
    };

    root.addEventListener("pointerdown", this.pointerDown);
    root.addEventListener("pointermove", this.pointerMove);
    root.addEventListener("pointerup", this.pointerUp);
    root.addEventListener("pointercancel", this.pointerUp);
    root.addEventListener("wheel", this.wheel, { passive: false });
    window.addEventListener("keydown", this.keyDown);
    window.addEventListener("resize", this.resize);
  }

  load({ name, request, index, count, interactionStartedAt }) {
    this.resetTransform();
    this.title.textContent = name;
    this.image.alt = name;
    this.counter.textContent = `${index + 1} / ${count}`;
    this.previous.disabled = index === 0;
    this.next.disabled = index === count - 1;
    this.loadMeasuredImage(request, interactionStartedAt, name);
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

  applyTransform() {
    this.image.style.transform = `translate3d(${this.panX}px, ${this.panY}px, 0) scale(${this.scale})`;
  }

  onPointerDown(event) {
    if (event.target.closest("button")) return;
    this.root.setPointerCapture?.(event.pointerId);
    this.pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (this.pointers.size === 1) {
      this.single = {
        startX: event.clientX,
        startY: event.clientY,
        lastX: event.clientX,
        lastY: event.clientY,
        startedAt: performance.now(),
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
      this.scale = clamp(this.pinch.scale * ratio, 1, 6);
      this.panX = this.pinch.panX + center.x - this.pinch.center.x;
      this.panY = this.pinch.panY + center.y - this.pinch.center.y;
      this.applyTransform();
      return;
    }

    if (this.scale > 1.01 && this.single && previous) {
      this.panX += event.clientX - previous.x;
      this.panY += event.clientY - previous.y;
      this.single.lastX = event.clientX;
      this.single.lastY = event.clientY;
      this.applyTransform();
    }
  }

  onPointerUp(event) {
    if (!this.pointers.has(event.pointerId)) return;
    const single = this.single;
    this.pointers.delete(event.pointerId);

    if (this.pointers.size === 1) {
      const [remaining] = [...this.pointers.values()];
      this.single = {
        startX: remaining.x,
        startY: remaining.y,
        lastX: remaining.x,
        lastY: remaining.y,
        startedAt: performance.now(),
      };
      this.pinch = null;
      return;
    }
    if (this.pointers.size > 0) return;

    if (!this.pinched && single) {
      const dx = event.clientX - single.startX;
      const dy = event.clientY - single.startY;
      const elapsed = performance.now() - single.startedAt;
      if (this.scale <= 1.01 && Math.abs(dx) > 52 && Math.abs(dx) > Math.abs(dy) * 1.25) {
        this.onStep(dx < 0 ? 1 : -1);
      } else if (Math.hypot(dx, dy) < 12 && elapsed < 450) {
        this.root.classList.toggle("ui-hidden");
      }
    }
    this.single = null;
    this.pinch = null;
    this.pinched = false;
  }

  onWheel(event) {
    event.preventDefault();
    const factor = event.deltaY < 0 ? 1.14 : 1 / 1.14;
    this.scale = clamp(this.scale * factor, 1, 6);
    if (this.scale === 1) {
      this.panX = 0;
      this.panY = 0;
    }
    this.applyTransform();
  }

  onKeyDown(event) {
    if (event.key === "ArrowLeft") this.onStep(-1);
    if (event.key === "ArrowRight") this.onStep(1);
    if (event.key === "Escape" && !document.fullscreenElement) this.onClose();
  }

  destroy() {
    clearTimeout(this.resizeTimer);
    this.loadSequence += 1;
    this.fetchController?.abort();
    if (this.objectUrl) URL.revokeObjectURL(this.objectUrl);
    this.objectUrl = null;
    this.root.removeEventListener("pointerdown", this.pointerDown);
    this.root.removeEventListener("pointermove", this.pointerMove);
    this.root.removeEventListener("pointerup", this.pointerUp);
    this.root.removeEventListener("pointercancel", this.pointerUp);
    this.root.removeEventListener("wheel", this.wheel);
    window.removeEventListener("keydown", this.keyDown);
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
    throw new Error("認証できませんでした。起動時に表示された ?t= 付き URL から開き直してください。");
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
  const status = element("div", "center-status");
  status.append(element("div", "spinner"), textElement("div", message));
  app.append(status);
}

function renderError(error) {
  if (error?.name === "AbortError") return;
  cleanupScreen();
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
  if (!TELEMETRY_ENABLED) return;
  telemetryState.queue.push({
    client_event_timestamp_ms: Date.now(),
    ...event,
  });
  if (telemetryState.queue.length > 200) {
    telemetryState.queue.splice(0, telemetryState.queue.length - 200);
  }
}

async function flushTelemetry(useBeacon) {
  if (!telemetryState.queue.length || (!useBeacon && telemetryState.flushing)) return;
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
