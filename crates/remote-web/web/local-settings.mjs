export const LOCAL_SETTINGS_STORAGE_KEY = "miv-remote-local-settings";
export const LOCAL_SETTINGS_VERSION = 1;

export function defaultLocalSettings() {
  return {
    version: LOCAL_SETTINGS_VERSION,
    portraitSinglePage: true,
    gestureHelpDismissed: false,
  };
}

export function normalizeLocalSettings(value) {
  const defaults = defaultLocalSettings();
  if (!value || typeof value !== "object" || value.version !== LOCAL_SETTINGS_VERSION) {
    return defaults;
  }
  return {
    version: LOCAL_SETTINGS_VERSION,
    portraitSinglePage:
      typeof value.portraitSinglePage === "boolean"
        ? value.portraitSinglePage
        : defaults.portraitSinglePage,
    gestureHelpDismissed:
      typeof value.gestureHelpDismissed === "boolean"
        ? value.gestureHelpDismissed
        : defaults.gestureHelpDismissed,
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
