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
  SET_TRANSFORM: "set_transform",
  PAN_BY: "pan_by",
  TOGGLE_MENU: "toggle_menu",
  BACK: "back",
  FORWARD: "forward",
  PARENT_FOLDER: "parent_folder",
  OPEN: "open",
  OPEN_SELECTED: "open_selected",
  TOGGLE_FULLSCREEN: "toggle_fullscreen",
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
    if (plain && ["ArrowRight", "ArrowDown", "PageDown"].includes(key)) {
      return command(CommandName.NEXT_PAGE);
    }
    if (plain && ["ArrowLeft", "ArrowUp", "PageUp"].includes(key)) {
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

export function viewerTapCommand(clientX, width) {
  const ratio = Math.max(0, Math.min(1, clientX / Math.max(1, width)));
  if (ratio < 0.34) return command(CommandName.PREV_PAGE);
  if (ratio > 0.66) return command(CommandName.NEXT_PAGE);
  return command(CommandName.TOGGLE_MENU);
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
