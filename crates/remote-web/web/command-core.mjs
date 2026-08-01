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
  BACK: "back",
  FORWARD: "forward",
  PARENT_FOLDER: "parent_folder",
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

export const ViewerGesture = Object.freeze({
  TAP: "tap",
  SWIPE_LEFT: "swipe_left",
  SWIPE_RIGHT: "swipe_right",
  SWIPE_UP: "swipe_up",
  SWIPE_DOWN: "swipe_down",
  PAN: "pan",
});

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
  const ratio = Math.max(0, Math.min(1, clientX / Math.max(1, width)));
  if (ratio < 0.34) return command(rtl ? CommandName.NEXT_PAGE : CommandName.PREV_PAGE);
  if (ratio > 0.66) return command(rtl ? CommandName.PREV_PAGE : CommandName.NEXT_PAGE);
  return command(CommandName.TOGGLE_VIEWER_BARS);
}

export function viewerSeekGroupIndex(physicalValue, groupCount, rtl = false) {
  const count = Math.max(0, Math.floor(Number(groupCount) || 0));
  if (!count) return -1;
  const physicalIndex = Math.max(
    0,
    Math.min(count - 1, Math.round(Number(physicalValue) || 0))
  );
  return rtl ? count - 1 - physicalIndex : physicalIndex;
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
    value: rtl ? groups.length - 1 - groupIndex : groupIndex,
    groupIndex,
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

export function shouldShowKeyboardShortcuts({
  coarsePointer = false,
  keyboardUsed = false,
} = {}) {
  return !coarsePointer || Boolean(keyboardUsed);
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
  labelHeight = 38
) {
  const width = Math.max(1, Number(containerWidth) || 1);
  const inset = width >= 900 ? 20 : 10;
  const availableWidth = Math.max(1, width - inset * 2);
  const compact = availableWidth < 600;
  const gap = compact ? 8 : 12;
  const targetCellWidth = compact ? 132 : availableWidth < 1000 ? 180 : 210;
  const columns = Math.max(
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

export function thumbnailRetryDecision(
  status,
  errorCode,
  retryCount,
  maxRetries = 3
) {
  const numericStatus = Number(status) || 0;
  const transient =
    numericStatus === 0 ||
    numericStatus === 502 ||
    (numericStatus === 503 && errorCode !== "protocol_version_mismatch");
  if (!transient) {
    return { retry: false, exhausted: false, delayMs: 0 };
  }
  const retries = Math.max(0, Math.floor(Number(retryCount) || 0));
  const maximum = Math.max(0, Math.floor(Number(maxRetries) || 0));
  if (retries >= maximum) {
    return { retry: false, exhausted: true, delayMs: 0 };
  }
  return {
    retry: true,
    exhausted: false,
    delayMs: Math.min(4000, 200 * 2 ** retries),
  };
}

export function pagePrefetchPlan({
  visibleIndexes,
  itemCount,
  direction,
  ahead = 8,
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

export function containerPageTargetPx({
  requestWidth,
  sourceWidth,
  sourceHeight,
  minimum = 256,
  maximum = 8192,
}) {
  const width = Math.max(1, Number(sourceWidth) || 1);
  const height = Math.max(1, Number(sourceHeight) || 1);
  const physicalWidth = Math.max(1, Number(requestWidth) || 1);
  const physicalHeight = physicalWidth * height / width;
  return Math.max(
    minimum,
    Math.min(maximum, Math.ceil(Math.max(physicalWidth, physicalHeight)))
  );
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
      label: "別の端末が操作中 (操作すると取得します)",
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
