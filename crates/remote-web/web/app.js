const app = document.querySelector("#app");

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
  viewer: null,
};

window.addEventListener("popstate", () => dispatchRoute());

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
      renderImageViewer(index);
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

  if (!state.entries.length) {
    const empty = textElement(
      "p",
      "このフォルダには表示できるサブフォルダまたは画像がありません。",
      "empty-state center-status"
    );
    scroll.replaceChildren(empty);
    return;
  }

  const imageIndexes = new Map(state.images.map((entry, index) => [entry.path, index]));
  state.virtualGrid = new VirtualGrid(
    scroll,
    space,
    windowElement,
    state.entries,
    (entry) => createGridTile(entry, imageIndexes)
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

function createGridTile(entry, imageIndexes) {
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
    image.src = apiUrl("/api/thumb", {
      fav: state.favoriteId,
      path: entry.path,
    });
    image.addEventListener("error", () => image.classList.add("thumb-missing"));
    preview.append(image);
    tile.addEventListener("click", () => {
      tryEnterBrowserFullscreen();
      const index = imageIndexes.get(entry.path);
      if (index !== undefined) {
        history.pushState(null, "", imageHash(state.favoriteId, entry.path));
        renderImageViewer(index);
      }
    });
  }
  tile.append(preview, textElement("span", entry.name, "tile-label"));
  return tile;
}

function renderImageViewer(index) {
  cleanupScreen();
  state.imageIndex = index;
  const imageEntry = state.images[index];
  document.title = `${imageEntry.name} — mIV Remote`;

  const viewerRoot = element("section", "image-viewer");
  const stage = element("div", "viewer-stage");
  const image = element("img", "viewer-image");
  image.alt = imageEntry.name;
  image.draggable = false;
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
  updateViewerImage();
}

function changeImage(delta) {
  const nextIndex = state.imageIndex + delta;
  if (nextIndex < 0 || nextIndex >= state.images.length) {
    return;
  }
  state.imageIndex = nextIndex;
  const entry = state.images[nextIndex];
  history.pushState(null, "", imageHash(state.favoriteId, entry.path));
  updateViewerImage();
}

function updateViewerImage() {
  const entry = state.images[state.imageIndex];
  document.title = `${entry.name} — mIV Remote`;
  state.viewer.load({
    name: entry.name,
    url: imageUrl(entry.path),
    index: state.imageIndex,
    count: state.images.length,
  });
  const nextEntry = state.images[state.imageIndex + 1];
  if (nextEntry) {
    const preload = new Image();
    preload.decoding = "async";
    preload.src = imageUrl(nextEntry.path);
  }
}

function imageUrl(path) {
  const cssWidth = Math.max(320, window.visualViewport?.width ?? window.innerWidth);
  const width = Math.min(4096, Math.ceil(cssWidth * (window.devicePixelRatio || 1)));
  return apiUrl("/api/image", { fav: state.favoriteId, path, w: width });
}

class VirtualGrid {
  constructor(scroller, space, windowElement, items, renderCell) {
    this.scroller = scroller;
    this.space = space;
    this.windowElement = windowElement;
    this.items = items;
    this.renderCell = renderCell;
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

    this.pointerDown = (event) => this.onPointerDown(event);
    this.pointerMove = (event) => this.onPointerMove(event);
    this.pointerUp = (event) => this.onPointerUp(event);
    this.wheel = (event) => this.onWheel(event);
    this.keyDown = (event) => this.onKeyDown(event);
    this.resize = () => {
      clearTimeout(this.resizeTimer);
      this.resizeTimer = setTimeout(() => {
        const entry = state.images[state.imageIndex];
        if (entry) this.setSource(imageUrl(entry.path));
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

  load({ name, url, index, count }) {
    this.resetTransform();
    this.title.textContent = name;
    this.image.alt = name;
    this.counter.textContent = `${index + 1} / ${count}`;
    this.previous.disabled = index === 0;
    this.next.disabled = index === count - 1;
    this.setSource(url);
  }

  setSource(url) {
    this.image.src = url;
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
  const response = await fetch(apiUrl(path, params), {
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
