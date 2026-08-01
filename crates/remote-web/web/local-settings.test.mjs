import test from "node:test";
import assert from "node:assert/strict";

import {
  LOCAL_SETTINGS_STORAGE_KEY,
  defaultLocalSettings,
  loadLocalSettings,
  parseLocalSettings,
  saveLocalSettings,
  serializeLocalSettings,
} from "./local-settings.mjs";

test("local settings use the current defaults when no value exists", () => {
  assert.deepEqual(parseLocalSettings(null), {
    version: 1,
    portraitSinglePage: true,
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
  const settings = { version: 1, portraitSinglePage: false };
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
  assert.deepEqual(saveLocalSettings({ version: 1, portraitSinglePage: false }, unavailable), {
    settings: { version: 1, portraitSinglePage: false },
    saved: false,
  });
});

test("storage helpers use one aggregate key", () => {
  const values = new Map();
  const storage = {
    getItem(key) { return values.get(key) ?? null; },
    setItem(key, value) { values.set(key, value); },
  };
  const saved = saveLocalSettings({ version: 1, portraitSinglePage: false }, storage);
  assert.equal(saved.saved, true);
  assert.equal(values.size, 1);
  assert.ok(values.has(LOCAL_SETTINGS_STORAGE_KEY));
  assert.deepEqual(loadLocalSettings(storage).settings, saved.settings);
});
