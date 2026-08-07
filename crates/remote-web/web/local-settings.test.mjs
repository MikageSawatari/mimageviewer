import test from "node:test";
import assert from "node:assert/strict";

import {
  ADJUSTMENT_PANEL_TABS,
  LOCAL_SETTINGS_STORAGE_KEY,
  defaultLocalSettings,
  loadLocalSettings,
  normalizeAdjustmentTab,
  parseLocalSettings,
  saveLocalSettings,
  serializeLocalSettings,
} from "./local-settings.mjs";

test("local settings use the current defaults when no value exists", () => {
  assert.deepEqual(parseLocalSettings(null), {
    version: 1,
    imageQuality: "standard",
    portraitSinglePage: true,
    gestureHelpDismissed: false,
    gridColumnsPortrait: 0,
    gridColumnsLandscape: 0,
    telemetryDebugDetails: false,
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
    telemetryDebugDetails: true,
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
    telemetryDebugDetails: true,
    adjustmentTab: "ai",
  }, unavailable), {
    settings: {
      version: 1,
      imageQuality: "light",
      portraitSinglePage: false,
      gestureHelpDismissed: true,
      gridColumnsPortrait: 4,
      gridColumnsLandscape: 6,
      telemetryDebugDetails: true,
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
      telemetryDebugDetails: false,
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
      telemetryDebugDetails: false,
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
      telemetryDebugDetails: false,
      adjustmentTab: "color_tone",
    }
  );
});

test("all image quality choices round-trip as device-local settings", () => {
  for (const imageQuality of ["high", "standard", "light", "minimum"]) {
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
