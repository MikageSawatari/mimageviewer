import test from "node:test";
import assert from "node:assert/strict";
import { RawPrefetchWindowPublisher, rawPrefetchWindow } from "./raw-prefetch-window.mjs";

const tick = () => new Promise((resolve) => setImmediate(resolve));
const images = Array.from({ length: 10 }, (_, index) => ({ path: `image-${index}.dng`, subresource: { kind: "file" } }));
const windowFor = (visibleIndexes, direction = 1) => rawPrefetchWindow({ images, visibleIndexes, direction, addressOf: (image) => image });
const display = (unit = "group-3", direction = 1) => ({ unit, direction, entries: windowFor([3], direction) });
function harness(options = {}) {
  const calls = [];
  const delays = [];
  const publisher = new RawPrefetchWindowPublisher({
    current: () => null,
    send: async (body, session, signal) => { calls.push({ body, session, signal }); return { status: 200 }; },
    delay: async (ms, signal) => { delays.push({ ms, signal }); },
    ...options,
  });
  publisher.setSession("session-a");
  return { publisher, calls, delays };
}

test("image-unit window has two ahead and one behind; reverse uses opposite edges", () => {
  assert.deepEqual(windowFor([3]), [images[4], images[5], images[2]]);
  assert.deepEqual(windowFor([3, 4], -1), [images[2], images[1], images[5]]);
  assert.deepEqual(windowFor([0]), [images[1], images[2]]);
});

test("spread partners and auxiliary cover slots enter only by their own image position", () => {
  assert.deepEqual(windowFor([3, 4]), [images[5], images[6], images[2]]);
  // The cover at position 0 is not carried along with a neighboring group.
  assert(!windowFor([7, 8]).includes(images[0]));
  assert.deepEqual(windowFor([8, 9]), [images[7]]);
});

test("cached initial display publishes; repeated slots/retries publish once", async () => {
  const { publisher, calls } = harness();
  publisher.commit(display());
  publisher.commit(display());
  await tick();
  publisher.commit(display());
  assert.equal(calls.length, 1);
  assert.equal(calls[0].body.window_generation, 1);
});

test("return to same display with opposite direction publishes again", async () => {
  const { publisher, calls } = harness();
  publisher.commit(display()); await tick();
  publisher.commit(display("group-4")); await tick();
  publisher.commit(display("group-3", -1)); await tick();
  assert.equal(calls.length, 3);
  assert.deepEqual(calls[2].body.entries, [images[2], images[1], images[4]]);
});

test("changed ordered addresses change the stamp even at the same display unit", async () => {
  const { publisher, calls } = harness();
  publisher.commit(display()); await tick();
  publisher.commit({ ...display(), entries: [...display().entries].reverse() }); await tick();
  assert.equal(calls.length, 2);
});

test("leaving sends empty; reopening viewer continues session generation", async () => {
  const { publisher, calls } = harness();
  publisher.commit(display()); await tick();
  publisher.leave(); await tick();
  publisher.leave(); await tick();
  publisher.commit(display()); await tick();
  assert.deepEqual(calls.map(({ body }) => body.window_generation), [1, 2, 3]);
  assert.deepEqual(calls[1].body.entries, []);
});

test("in-flight declarations coalesce to latest and abort obsolete send", async () => {
  let resolve;
  const calls = [];
  const { publisher } = harness({ send: (body, session, signal) => {
    calls.push({ body, session, signal });
    if (calls.length === 1) return new Promise((done) => { resolve = done; });
    return Promise.resolve({ status: 200 });
  } });
  publisher.commit(display("one"));
  publisher.commit(display("two"));
  publisher.commit(display("three"));
  assert(calls[0].signal.aborted);
  resolve({ status: 200 }); await tick();
  assert.deepEqual(calls.map(({ body }) => body.window_generation), [1, 3]);
});

test("network failures and busy 503 retry with captured session and same generation", async () => {
  const calls = [];
  const { publisher, delays } = harness({ send: async (body, session) => {
    calls.push({ body, session });
    if (calls.length === 1) throw new TypeError("offline");
    return { status: calls.length === 2 ? 503 : 200 };
  } });
  publisher.commit(display()); await tick();
  assert.equal(calls.length, 3);
  assert.equal(delays.length, 2);
  assert(calls.every(({ session, body }) => session === "session-a" && body.window_generation === 1));
});

for (const status of [400, 401, 409, 413, 428]) {
  test(`HTTP ${status} never retries`, async () => {
    let count = 0;
    const { publisher, delays } = harness({ send: async () => { count += 1; return { status }; } });
    publisher.commit(display()); await tick();
    assert.equal(count, 1); assert.equal(delays.length, 0);
  });
}

test("expiry during backoff aborts delay and discards body", async () => {
  let signal;
  let calls = 0;
  const { publisher } = harness({
    send: async () => { calls += 1; return { status: 503 }; },
    delay: (_, captured) => { signal = captured; return new Promise((_, reject) => captured.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")), { once: true })); },
  });
  publisher.commit(display()); await tick();
  publisher.setSession(""); await tick();
  assert(signal.aborted); assert.equal(calls, 1);
});

test("attestation or other validation errors are terminal, not network retries", async () => {
  let calls = 0;
  const { publisher, delays } = harness({ send: async () => { calls += 1; throw Object.assign(new Error("invalid session"), { retryable: false }); } });
  publisher.commit(display()); await tick();
  assert.equal(calls, 1); assert.equal(delays.length, 0);
});

test("reacquisition while sending rebuilds current window with counter reset, never old body", async () => {
  const calls = [];
  let resolve;
  let current = display("initial");
  const { publisher } = harness({ current: () => current, send: (body, session, signal) => {
    calls.push({ body, session, signal });
    if (calls.length === 1) return new Promise((done) => { resolve = done; });
    return Promise.resolve({ status: 200 });
  } });
  current = display("new", -1);
  publisher.setSession("session-b");
  resolve({ status: 503 }); await tick();
  assert(calls[0].signal.aborted);
  assert.equal(calls.length, 2);
  assert.equal(calls[1].session, "session-b");
  assert.equal(calls[1].body.window_generation, 1);
  assert.deepEqual(calls[1].body.entries, current.entries);
});
