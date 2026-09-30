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
    return new Response(JSON.stringify({ error: "ipc_busy" }), { status: calls.length === 2 ? 503 : 200 });
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
    send: async () => { calls += 1; return new Response(JSON.stringify({ error: "ipc_busy" }), { status: 503 }); },
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
  const { publisher } = harness({ send: (body, session, signal) => {
    calls.push({ body, session, signal });
    if (calls.length === 1) return new Promise((done) => { resolve = done; });
    return Promise.resolve({ status: 200 });
  } });
  publisher.commit(display("initial"));
  const current = display("new", -1);
  publisher.commit(current);
  publisher.setSession("session-b");
  resolve({ status: 503 }); await tick();
  assert(calls[0].signal.aborted);
  assert.equal(calls.length, 2);
  assert.equal(calls[1].session, "session-b");
  assert.equal(calls[1].body.window_generation, 1);
  assert.deepEqual(calls[1].body.entries, current.entries);
});

test("reacquisition before first presentation waits for the actual commit", async () => {
  const { publisher, calls } = harness();
  publisher.setSession("session-b");
  await tick();
  assert.equal(calls.length, 0);
  publisher.commit(display());
  publisher.commit(display());
  await tick();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].session, "session-b");
  assert.equal(calls[0].body.window_generation, 1);
});

test("commit without a session retains its snapshot; leaving clears it", async () => {
  const { publisher, calls } = harness();
  publisher.setSession("");
  const presentation = structuredClone(display());
  const expected = structuredClone(presentation.entries);
  publisher.commit(presentation);
  presentation.entries[0].path = "mutated-after-commit.dng";
  assert.equal(calls.length, 0);
  publisher.setSession("session-b");
  await tick();
  assert.deepEqual(calls[0].body.entries, expected);
  publisher.setSession("");
  publisher.leave();
  publisher.setSession("session-c");
  await tick();
  assert.equal(calls.length, 1);
});

for (const error of ["ipc_timeout", "ipc_busy", "admission_busy", "raw_busy", "miv_media_error"]) {
  test(`transient HTTP error ${error} retries`, async () => {
    let count = 0;
    const { publisher, delays } = harness({ send: async () => {
      count += 1;
      return new Response(JSON.stringify({ error }), { status: count === 1 ? 503 : 200 });
    } });
    publisher.commit(display());
    await tick();
    assert.equal(count, 2);
    assert.equal(delays.length, 1);
  });
}

test("network failure reading a 503 body retries the same captured declaration", async () => {
  const calls = [];
  const { publisher, delays } = harness({ send: async (body, session) => {
    calls.push({ body, session });
    if (calls.length !== 1) return new Response("{}", { status: 200 });
    const stream = new ReadableStream({
      start(controller) { controller.error(new TypeError("connection lost during response body")); },
    });
    return new Response(stream, { status: 503, headers: { "Retry-After": "1" } });
  } });
  publisher.commit(display());
  publisher.commit(display()); // The stamp cannot repair a lost declaration.
  await tick();
  assert.equal(calls.length, 2);
  assert.equal(delays.length, 1);
  assert.equal(delays[0].ms, 1000);
  assert(calls.every(({ session, body }) => session === "session-a" && body.window_generation === 1));
  assert.deepEqual(calls[1].body.entries, display().entries);
});

for (const body of ["invalid JSON", "null"]) {
  test(`unusable 503 JSON (${body}) is terminal rather than a network failure`, async () => {
    let count = 0;
    const { publisher, delays } = harness({ send: async () => {
      count += 1;
      return new Response(count === 1 ? body : "{}", { status: count === 1 ? 503 : 200 });
    } });
    publisher.commit(display());
    await tick();
    assert.equal(count, 1);
    assert.equal(delays.length, 0);
  });
}

for (const error of ["protocol_version_mismatch", "miv_not_running", "ipc_protocol_error", "unknown_error", undefined]) {
  test(`permanent 503 ${error ?? "without an error kind"} stops`, async () => {
    let count = 0;
    const { publisher, delays } = harness({ send: async () => {
      count += 1;
      // A second success also bounds the regression on the old blanket retry.
      return new Response(JSON.stringify({ error }), { status: count === 1 ? 503 : 200 });
    } });
    publisher.commit(display());
    await tick();
    assert.equal(count, 1);
    assert.equal(delays.length, 0);
  });
}

test("non-network sender errors do not retry", async () => {
  let count = 0;
  const { publisher, delays } = harness({ send: async () => {
    count += 1;
    if (count === 1) throw new Error("sender failed permanently");
    return { status: 200 };
  } });
  publisher.commit(display());
  await tick();
  assert.equal(count, 1);
  assert.equal(delays.length, 0);
});
