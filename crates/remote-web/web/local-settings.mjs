import { imageQualityPreset } from "./command-core.mjs";

export const LOCAL_SETTINGS_STORAGE_KEY = "miv-remote-local-settings";
export const LOCAL_SETTINGS_VERSION = 1;
export const ADJUSTMENT_PANEL_TABS = Object.freeze([
  Object.freeze({ id: "color_tone", label: "色調" }),
  Object.freeze({ id: "ai", label: "AI" }),
  Object.freeze({ id: "colorize", label: "カラー化" }),
]);

export function normalizeAdjustmentTab(value) {
  return ADJUSTMENT_PANEL_TABS.some((tab) => tab.id === value)
    ? value
    : ADJUSTMENT_PANEL_TABS[0].id;
}

export function defaultLocalSettings() {
  return {
    version: LOCAL_SETTINGS_VERSION,
    imageQuality: "standard",
    portraitSinglePage: true,
    gestureHelpDismissed: false,
    gridColumnsPortrait: 0,
    gridColumnsLandscape: 0,
    telemetryDebugDetails: false,
    adjustmentTab: ADJUSTMENT_PANEL_TABS[0].id,
  };
}

function normalizeGridColumns(value, fallback) {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  const columns = Math.round(value);
  if (columns === 0) return 0;
  return Math.min(8, Math.max(2, columns));
}

export function normalizeLocalSettings(value) {
  const defaults = defaultLocalSettings();
  if (!value || typeof value !== "object" || value.version !== LOCAL_SETTINGS_VERSION) {
    return defaults;
  }
  return {
    version: LOCAL_SETTINGS_VERSION,
    imageQuality: imageQualityPreset(value.imageQuality).id,
    portraitSinglePage:
      typeof value.portraitSinglePage === "boolean"
        ? value.portraitSinglePage
        : defaults.portraitSinglePage,
    gestureHelpDismissed:
      typeof value.gestureHelpDismissed === "boolean"
        ? value.gestureHelpDismissed
        : defaults.gestureHelpDismissed,
    gridColumnsPortrait: normalizeGridColumns(
      value.gridColumnsPortrait,
      defaults.gridColumnsPortrait
    ),
    gridColumnsLandscape: normalizeGridColumns(
      value.gridColumnsLandscape,
      defaults.gridColumnsLandscape
    ),
    telemetryDebugDetails:
      typeof value.telemetryDebugDetails === "boolean"
        ? value.telemetryDebugDetails
        : defaults.telemetryDebugDetails,
    adjustmentTab: normalizeAdjustmentTab(value.adjustmentTab),
  };
}

export function parseLocalSettings(raw) {
  if (typeof raw !== "string" || !raw) return defaultLocalSettings();
  try {
    return normalizeLocalSettings(JSON.parse(raw));
  } catch {
    return defaultLocalSettings();
  }
}

export function serializeLocalSettings(value) {
  return JSON.stringify(normalizeLocalSettings(value));
}

export function loadLocalSettings(storage) {
  try {
    const target = arguments.length ? storage : globalThis.localStorage;
    if (!target) return { settings: defaultLocalSettings(), storageAvailable: false };
    return {
      settings: parseLocalSettings(target.getItem(LOCAL_SETTINGS_STORAGE_KEY)),
      storageAvailable: true,
    };
  } catch {
    return { settings: defaultLocalSettings(), storageAvailable: false };
  }
}

export function saveLocalSettings(value, storage) {
  const settings = normalizeLocalSettings(value);
  try {
    const target = arguments.length >= 2 ? storage : globalThis.localStorage;
    if (!target) return { settings, saved: false };
    target.setItem(LOCAL_SETTINGS_STORAGE_KEY, serializeLocalSettings(settings));
    return { settings, saved: true };
  } catch {
    return { settings, saved: false };
  }
}
