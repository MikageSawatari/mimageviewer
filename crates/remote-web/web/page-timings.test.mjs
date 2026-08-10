import test from "node:test";
import assert from "node:assert/strict";

import {
  PAGE_TIMINGS_STORAGE_KEY,
  appendPageTimingSample,
  averagePageTimings,
  emptyPageTimingHistory,
  formatPageTimingAverage,
  loadPageTimingHistory,
  pageTimingSample,
  savePageTimingHistory,
} from "./page-timings.mjs";

test("page timing averages have no estimate for zero samples", () => {
  assert.equal(averagePageTimings(emptyPageTimingHistory(), "standard"), null);
  assert.equal(formatPageTimingAverage(null), "");
});

test("page timing averages one successful page", () => {
  const sample = pageTimingSample({
    totalFetchMs: 2000,
    generationMs: 1800,
    decodeMs: 300,
  });
  const history = appendPageTimingSample(
    emptyPageTimingHistory(),
    "high",
    sample
  );
  const average = averagePageTimings(history, "high");
  assert.deepEqual(average, {
    count: 1,
    generationMs: 1800,
    transferMs: 200,
    decodeMs: 300,
  });
  // 標本 1 件を「10 ページの平均」と書かない。
  assert.equal(
    formatPageTimingAverage(average),
    "直近 1 ページの平均 — 生成 1.8 秒 / 転送 0.2 秒 / 展開 0.3 秒"
  );
});

test("page timing history keeps the newest ten pages per preset", () => {
  let history = emptyPageTimingHistory();
  for (let value = 1; value <= 12; value += 1) {
    history = appendPageTimingSample(history, "standard", {
      generationMs: value,
      transferMs: value * 2,
      decodeMs: value * 3,
    });
  }
  assert.equal(history.presets.standard.length, 10);
  assert.deepEqual(averagePageTimings(history, "standard"), {
    count: 10,
    generationMs: 7.5,
    transferMs: 15,
    decodeMs: 22.5,
  });
  assert.equal(averagePageTimings(history, "high"), null);
});

test("page timing transfer duration never becomes negative", () => {
  assert.deepEqual(pageTimingSample({
    totalFetchMs: 1000,
    generationMs: 1200,
    decodeMs: 100,
  }), {
    generationMs: 1200,
    transferMs: 0,
    decodeMs: 100,
  });
  assert.equal(pageTimingSample({
    totalFetchMs: 1000,
    generationMs: null,
    decodeMs: 100,
  }), null, "a missing server timing must not become an estimated zero");
});

test("page timing storage is separate, versioned, and failure-safe", () => {
  const values = new Map();
  const storage = {
    getItem(key) { return values.get(key) ?? null; },
    setItem(key, value) { values.set(key, value); },
  };
  const history = appendPageTimingSample(emptyPageTimingHistory(), "light", {
    generationMs: 10,
    transferMs: 20,
    decodeMs: 30,
  });
  assert.equal(savePageTimingHistory(history, storage).saved, true);
  assert.ok(values.has(PAGE_TIMINGS_STORAGE_KEY));
  assert.deepEqual(loadPageTimingHistory(storage).history, history);

  const unavailable = {
    getItem() { throw new Error("unavailable"); },
    setItem() { throw new Error("unavailable"); },
  };
  assert.equal(loadPageTimingHistory(unavailable).storageAvailable, false);
  assert.equal(savePageTimingHistory(history, unavailable).saved, false);
});
