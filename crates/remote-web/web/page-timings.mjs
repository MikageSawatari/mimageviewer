import { IMAGE_QUALITY_PRESETS } from "./command-core.mjs";

export const PAGE_TIMINGS_STORAGE_KEY = "miv-remote-page-timings";
export const PAGE_TIMINGS_VERSION = 1;
export const PAGE_TIMINGS_LIMIT = 10;

const IMAGE_QUALITY_IDS = new Set(IMAGE_QUALITY_PRESETS.map(({ id }) => id));

export function emptyPageTimingHistory() {
  return { version: PAGE_TIMINGS_VERSION, presets: {} };
}

function finiteNonnegative(value) {
  if (value === null || value === undefined || value === "") return null;
  const number = Number(value);
  return Number.isFinite(number) && number >= 0 ? number : null;
}

function normalizeStoredSample(value) {
  if (!value || typeof value !== "object") return null;
  const generationMs = finiteNonnegative(value.generationMs);
  const transferMs = finiteNonnegative(value.transferMs);
  const decodeMs = finiteNonnegative(value.decodeMs);
  if (generationMs === null || transferMs === null || decodeMs === null) return null;
  return { generationMs, transferMs, decodeMs };
}

export function pageTimingSample({ totalFetchMs, generationMs, decodeMs } = {}) {
  const total = finiteNonnegative(totalFetchMs);
  const generation = finiteNonnegative(generationMs);
  const decode = finiteNonnegative(decodeMs);
  if (total === null || generation === null || decode === null) return null;
  return {
    generationMs: generation,
    transferMs: Math.max(0, total - generation),
    decodeMs: decode,
  };
}

export function normalizePageTimingHistory(value) {
  if (!value || typeof value !== "object" || value.version !== PAGE_TIMINGS_VERSION) {
    return emptyPageTimingHistory();
  }
  const presets = {};
  for (const preset of IMAGE_QUALITY_PRESETS) {
    const samples = Array.isArray(value.presets?.[preset.id])
      ? value.presets[preset.id]
          .map(normalizeStoredSample)
          .filter(Boolean)
          .slice(-PAGE_TIMINGS_LIMIT)
      : [];
    if (samples.length) presets[preset.id] = samples;
  }
  return { version: PAGE_TIMINGS_VERSION, presets };
}

export function appendPageTimingSample(history, presetId, sample) {
  const normalized = normalizePageTimingHistory(history);
  const normalizedSample = normalizeStoredSample(sample);
  if (!IMAGE_QUALITY_IDS.has(presetId) || !normalizedSample) return normalized;
  return {
    version: PAGE_TIMINGS_VERSION,
    presets: {
      ...normalized.presets,
      [presetId]: [
        ...(normalized.presets[presetId] ?? []),
        normalizedSample,
      ].slice(-PAGE_TIMINGS_LIMIT),
    },
  };
}

export const PAGE_TIMING_COUNTED_MEMORY = 64;

/// The same fetched resource can be displayed again (back/forward, fit change,
/// resize). Counting it again would fill "the last N pages" with one blob.
/// Mutates `countedIds`; returns whether this display is a new sample.
export function shouldCountPageTiming(
  countedIds,
  requestId,
  memory = PAGE_TIMING_COUNTED_MEMORY
) {
  if (typeof requestId !== "string" || !requestId) return true;
  if (countedIds.has(requestId)) return false;
  countedIds.add(requestId);
  while (countedIds.size > Math.max(1, Math.floor(Number(memory) || 1))) {
    countedIds.delete(countedIds.values().next().value);
  }
  return true;
}

export function averagePageTimings(history, presetId) {
  const samples = normalizePageTimingHistory(history).presets[presetId] ?? [];
  if (!samples.length) return null;
  const totals = samples.reduce(
    (sum, sample) => ({
      generationMs: sum.generationMs + sample.generationMs,
      transferMs: sum.transferMs + sample.transferMs,
      decodeMs: sum.decodeMs + sample.decodeMs,
    }),
    { generationMs: 0, transferMs: 0, decodeMs: 0 }
  );
  return {
    count: samples.length,
    generationMs: totals.generationMs / samples.length,
    transferMs: totals.transferMs / samples.length,
    decodeMs: totals.decodeMs / samples.length,
  };
}

/// 標本数はそのまま述べる。3 件しか無いのに「10 ページの平均」と書くと嘘になる。
export function formatPageTimingAverage(average) {
  if (!average) return "";
  const seconds = (milliseconds) => `${(milliseconds / 1000).toFixed(1)} 秒`;
  return `直近 ${average.count} ページの平均 — 生成 ${seconds(average.generationMs)}`
    + ` / 転送 ${seconds(average.transferMs)} / 展開 ${seconds(average.decodeMs)}`;
}

export function loadPageTimingHistory(storage) {
  try {
    const target = arguments.length ? storage : globalThis.localStorage;
    if (!target) return { history: emptyPageTimingHistory(), storageAvailable: false };
    const raw = target.getItem(PAGE_TIMINGS_STORAGE_KEY);
    return {
      history: normalizePageTimingHistory(raw ? JSON.parse(raw) : null),
      storageAvailable: true,
    };
  } catch {
    return { history: emptyPageTimingHistory(), storageAvailable: false };
  }
}

export function savePageTimingHistory(history, storage) {
  const normalized = normalizePageTimingHistory(history);
  try {
    const target = arguments.length >= 2 ? storage : globalThis.localStorage;
    if (!target) return { history: normalized, saved: false };
    target.setItem(PAGE_TIMINGS_STORAGE_KEY, JSON.stringify(normalized));
    return { history: normalized, saved: true };
  } catch {
    return { history: normalized, saved: false };
  }
}
