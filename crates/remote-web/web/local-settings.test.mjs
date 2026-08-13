import test from "node:test";
import assert from "node:assert/strict";

import {
  ADJUSTMENT_PANEL_TABS,
  LOCAL_SETTINGS_VERSION,
  LOCAL_SETTINGS_STORAGE_KEY,
  defaultLocalSettings,
  loadLocalSettings,
  normalizeAdjustmentTab,
  parseLocalSettings,
  saveLocalSettings,
  serializeLocalSettings,
} from "./local-settings.mjs";
import { IMAGE_QUALITY_PRESETS } from "./command-core.mjs";

test("local settings use the current defaults when no value exists", () => {
  assert.deepEqual(parseLocalSettings(null), {
    version: 1,
    imageQuality: "standard",
    portraitSinglePage: true,
    gestureHelpDismissed: false,
    gridColumnsPortrait: 0,
    gridColumnsLandscape: 0,
    prefetchAhead: 8,
    prefetchBehind: 4,
    telemetryDebugDetails: false,
    diagnosticHudVisible: false,
    adjustmentTab: "color_tone",
  });
});

test("invalid local settings fall back without throwing", () => {
  assert.deepEqual(parseLocalSettings("not json"), defaultLocalSettings());
  assert.deepEqual(
    parseLocalSettings(JSON.stringify({ version: 1, portraitSinglePage: "yes" })),
    defaultLocalSettings()
  );
  assert.deepEqual(
    parseLocalSettings(JSON.stringify({ version: 99, portraitSinglePage: false })),
    defaultLocalSettings()
  );
});

test("local settings serialize and restore as one versioned value", () => {
  const settings = {
    version: 1,
    imageQuality: "high",
    portraitSinglePage: false,
    gestureHelpDismissed: true,
    gridColumnsPortrait: 3,
    gridColumnsLandscape: 7,
    prefetchAhead: 14,
    prefetchBehind: 6,
    telemetryDebugDetails: true,
    diagnosticHudVisible: true,
    adjustmentTab: "colorize",
  };
  assert.deepEqual(parseLocalSettings(serializeLocalSettings(settings)), settings);
});

test("storage failures use in-memory defaults and never escape", () => {
  const unavailable = {
    getItem() { throw new Error("unavailable"); },
    setItem() { throw new Error("unavailable"); },
  };
  assert.deepEqual(loadLocalSettings(unavailable), {
    settings: defaultLocalSettings(),
    storageAvailable: false,
  });
  assert.deepEqual(saveLocalSettings({
    version: 1,
    imageQuality: "light",
    portraitSinglePage: false,
    gestureHelpDismissed: true,
    gridColumnsPortrait: 4,
    gridColumnsLandscape: 6,
    prefetchAhead: 12,
    prefetchBehind: 5,
    telemetryDebugDetails: true,
    diagnosticHudVisible: false,
    adjustmentTab: "ai",
  }, unavailable), {
    settings: {
      version: 1,
      imageQuality: "light",
      portraitSinglePage: false,
      gestureHelpDismissed: true,
      gridColumnsPortrait: 4,
      gridColumnsLandscape: 6,
      prefetchAhead: 12,
      prefetchBehind: 5,
      telemetryDebugDetails: true,
      diagnosticHudVisible: false,
      adjustmentTab: "ai",
    },
    saved: false,
  });
});

test("storage helpers use one aggregate key", () => {
  const values = new Map();
  const storage = {
    getItem(key) { return values.get(key) ?? null; },
    setItem(key, value) { values.set(key, value); },
  };
  const saved = saveLocalSettings({
    version: 1,
    imageQuality: "minimum",
    portraitSinglePage: false,
    gestureHelpDismissed: true,
    gridColumnsPortrait: 2,
    gridColumnsLandscape: 8,
    telemetryDebugDetails: false,
    diagnosticHudVisible: false,
    adjustmentTab: "color_tone",
  }, storage);
  assert.equal(saved.saved, true);
  assert.equal(values.size, 1);
  assert.ok(values.has(LOCAL_SETTINGS_STORAGE_KEY));
  assert.deepEqual(loadLocalSettings(storage).settings, saved.settings);
});

test("older version-one values add the gesture help default", () => {
  assert.deepEqual(
    parseLocalSettings(JSON.stringify({ version: 1, portraitSinglePage: false })),
    {
      version: 1,
      imageQuality: "standard",
      portraitSinglePage: false,
      gestureHelpDismissed: false,
      gridColumnsPortrait: 0,
      gridColumnsLandscape: 0,
      prefetchAhead: 8,
      prefetchBehind: 4,
      telemetryDebugDetails: false,
      diagnosticHudVisible: false,
      adjustmentTab: "color_tone",
    }
  );
});

test("grid column settings clamp per field without replacing existing values", () => {
  assert.deepEqual(
    parseLocalSettings(JSON.stringify({
      version: 1,
      portraitSinglePage: false,
      gestureHelpDismissed: true,
      gridColumnsPortrait: 1,
      gridColumnsLandscape: 99,
    })),
    {
      version: 1,
      imageQuality: "standard",
      portraitSinglePage: false,
      gestureHelpDismissed: true,
      gridColumnsPortrait: 2,
      gridColumnsLandscape: 8,
      prefetchAhead: 8,
      prefetchBehind: 4,
      telemetryDebugDetails: false,
      diagnosticHudVisible: false,
      adjustmentTab: "color_tone",
    }
  );
  assert.deepEqual(
    parseLocalSettings(JSON.stringify({
      version: 1,
      portraitSinglePage: false,
      gestureHelpDismissed: true,
      gridColumnsPortrait: 0,
      gridColumnsLandscape: "6",
    })),
    {
      version: 1,
      imageQuality: "standard",
      portraitSinglePage: false,
      gestureHelpDismissed: true,
      gridColumnsPortrait: 0,
      gridColumnsLandscape: 0,
      prefetchAhead: 8,
      prefetchBehind: 4,
      telemetryDebugDetails: false,
      diagnosticHudVisible: false,
      adjustmentTab: "color_tone",
    }
  );
});

test("prefetch depth validates each field without changing version-one settings", () => {
  assert.equal(LOCAL_SETTINGS_VERSION, 1);
  const existing = parseLocalSettings(JSON.stringify({
    version: 1,
    imageQuality: "high",
    portraitSinglePage: false,
    gridColumnsPortrait: 5,
  }));
  assert.equal(existing.imageQuality, "high");
  assert.equal(existing.portraitSinglePage, false);
  assert.equal(existing.gridColumnsPortrait, 5);
  assert.equal(existing.prefetchAhead, 8);
  assert.equal(existing.prefetchBehind, 4);

  const invalid = parseLocalSettings(JSON.stringify({
    version: 1,
    prefetchAhead: 33,
    prefetchBehind: -1,
  }));
  assert.equal(invalid.prefetchAhead, 8);
  assert.equal(invalid.prefetchBehind, 4);

  const nonNumeric = parseLocalSettings(JSON.stringify({
    version: 1,
    prefetchAhead: "12",
    prefetchBehind: null,
  }));
  assert.equal(nonNumeric.prefetchAhead, 8);
  assert.equal(nonNumeric.prefetchBehind, 4);

  const boundaries = parseLocalSettings(JSON.stringify({
    version: 1,
    prefetchAhead: 2,
    prefetchBehind: 0,
  }));
  assert.equal(boundaries.prefetchAhead, 2);
  assert.equal(boundaries.prefetchBehind, 0);
});

test("all image quality choices round-trip as device-local settings", () => {
  // Driven off the presets themselves, so a new step cannot be added without
  // being covered here.
  assert.ok(IMAGE_QUALITY_PRESETS.length >= 5);
  for (const { id: imageQuality } of IMAGE_QUALITY_PRESETS) {
    const settings = { ...defaultLocalSettings(), imageQuality };
    assert.equal(
      parseLocalSettings(serializeLocalSettings(settings)).imageQuality,
      imageQuality
    );
  }
});

test("image quality defaults to standard and rejects unknown values", () => {
  assert.equal(parseLocalSettings(null).imageQuality, "standard");
  assert.equal(
    parseLocalSettings(JSON.stringify({ version: 1, imageQuality: "unexpected" })).imageQuality,
    "standard"
  );
});

test("adjustment tab order and normalization follow the available desktop sections", () => {
  assert.deepEqual(
    ADJUSTMENT_PANEL_TABS.map(({ id, label }) => [id, label]),
    [
      ["color_tone", "色調"],
      ["ai", "AI"],
      ["colorize", "カラー化"],
    ]
  );
  for (const { id } of ADJUSTMENT_PANEL_TABS) {
    assert.equal(normalizeAdjustmentTab(id), id);
  }
  assert.equal(normalizeAdjustmentTab("post_filter"), "color_tone");
  assert.equal(normalizeAdjustmentTab(null), "color_tone");
  assert.equal(
    parseLocalSettings(JSON.stringify({ version: 1, adjustmentTab: "ai" })).adjustmentTab,
    "ai"
  );
  assert.equal(
    parseLocalSettings(JSON.stringify({ version: 1, adjustmentTab: "unknown" })).adjustmentTab,
    "color_tone"
  );
});
