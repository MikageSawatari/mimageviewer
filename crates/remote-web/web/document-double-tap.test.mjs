import test from "node:test";
import assert from "node:assert/strict";

import {
  BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS,
  doubleTapSequenceTransition,
} from "./command-core.mjs";
import {
  installDocumentDoubleTapOwner,
} from "./document-double-tap.mjs";

class FakeEventTarget {
  constructor() {
    this.listeners = new Map();
  }

  addEventListener(type, listener) {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type, listener) {
    this.listeners.set(
      type,
      (this.listeners.get(type) ?? []).filter((candidate) => candidate !== listener)
    );
  }

  dispatch(type, event) {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
}

const plainTarget = { closest: () => null };
const textInputTarget = {
  closest: (selector) => selector.startsWith("input:not")
    ? { tagName: "INPUT" }
    : null,
};
const touch = (identifier, clientX, clientY) => ({ identifier, clientX, clientY });

function touchEvent({
  target = plainTarget,
  touches = [],
  changedTouches = [],
  cancelable = true,
  preventDefault = () => {},
} = {}) {
  return { target, touches, changedTouches, cancelable, preventDefault };
}

function dispatchTap(eventTarget, point, preventDefault, target = plainTarget) {
  eventTarget.dispatch("touchstart", touchEvent({ target, touches: [point] }));
  eventTarget.dispatch("touchend", touchEvent({
    target,
    changedTouches: [point],
    preventDefault,
  }));
}

test("double-tap recognition accepts a nearby pair within the browser window", () => {
  const first = doubleTapSequenceTransition(null, { x: 40, y: 80, atMs: 1_000 });
  const second = doubleTapSequenceTransition(first.next, {
    x: 48,
    y: 86,
    atMs: 1_240,
  });

  assert.equal(first.isDoubleTap, false);
  assert.equal(second.isDoubleTap, true);
  assert.equal(second.next, null);
});

test("double-tap recognition rejects late or distant taps and starts over after a pair", () => {
  const first = doubleTapSequenceTransition(null, { x: 40, y: 80, atMs: 1_000 });
  const late = doubleTapSequenceTransition(first.next, {
    x: 40,
    y: 80,
    atMs: 1_000 + BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS + 60,
  });
  const distant = doubleTapSequenceTransition(first.next, { x: 100, y: 80, atMs: 1_200 });
  const second = doubleTapSequenceTransition(first.next, { x: 44, y: 82, atMs: 1_200 });
  const third = doubleTapSequenceTransition(second.next, { x: 45, y: 83, atMs: 1_300 });

  assert.equal(late.isDoubleTap, false);
  assert.equal(distant.isDoubleTap, false);
  assert.equal(second.isDoubleTap, true);
  assert.equal(third.isDoubleTap, false, "the third tap starts a fresh pair");
  assert.deepEqual(third.next, { x: 45, y: 83, atMs: 1_300 });
});

test("document owner preserves the first touchend and prevents only the matching second tap", () => {
  const eventTarget = new FakeEventTarget();
  let nowMs = 1_000;
  let prevented = 0;
  const decisions = [];
  const owner = installDocumentDoubleTapOwner(eventTarget, {
    now: () => nowMs,
    onDecision: (decision) => decisions.push(decision),
  });

  dispatchTap(eventTarget, touch(1, 40, 80), () => { prevented += 1; });
  assert.equal(prevented, 0, "the first synthetic click must remain available");
  assert.deepEqual(decisions.at(-1), {
    decision: "candidate_started",
    atMs: 1_000,
    elapsedMs: null,
    distancePx: null,
    isDoubleTap: false,
    suppressed: false,
    excluded: false,
    exclusionReason: null,
    cancelable: true,
  });

  nowMs = 1_220;
  dispatchTap(eventTarget, touch(2, 44, 84), () => { prevented += 1; });
  assert.equal(prevented, 1);
  assert.deepEqual(decisions.at(-1), {
    decision: "pair_suppressed",
    atMs: 1_220,
    elapsedMs: 220,
    distancePx: Math.hypot(4, 4),
    isDoubleTap: true,
    suppressed: true,
    excluded: false,
    exclusionReason: null,
    cancelable: true,
  });

  // 窓の外であることを定数から決める。数値で書くと、窓を広げたときに黙って意味が変わる。
  nowMs = 2_000;
  dispatchTap(eventTarget, touch(3, 40, 80), () => { prevented += 1; });
  nowMs += BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS + 60;
  dispatchTap(eventTarget, touch(4, 40, 80), () => { prevented += 1; });
  assert.equal(prevented, 1, "non-matching touchends must keep their defaults");

  nowMs = 3_000;
  eventTarget.dispatch("touchstart", touchEvent({ touches: [touch(5, 40, 80)] }));
  eventTarget.dispatch("touchend", touchEvent({
    changedTouches: [touch(5, 140, 80)],
    preventDefault: () => { prevented += 1; },
  }));
  nowMs = 3_100;
  dispatchTap(eventTarget, touch(6, 142, 80), () => { prevented += 1; });
  assert.equal(prevented, 1, "a moved gesture must not seed a double-tap pair");

  nowMs = 4_000;
  dispatchTap(eventTarget, touch(7, 70, 90), () => { prevented += 1; });
  nowMs = 4_180;
  const nonCancelable = touch(8, 71, 91);
  eventTarget.dispatch("touchstart", touchEvent({ touches: [nonCancelable] }));
  eventTarget.dispatch("touchend", touchEvent({
    changedTouches: [nonCancelable],
    cancelable: false,
    preventDefault: () => { prevented += 1; },
  }));
  assert.equal(prevented, 1);
  assert.equal(decisions.at(-1).decision, "pair_not_cancelable");
  assert.equal(decisions.at(-1).isDoubleTap, true);
  assert.equal(decisions.at(-1).suppressed, false);
  assert.equal(decisions.at(-1).cancelable, false);
  owner.destroy();
});

test("document owner leaves pinch and text editing defaults untouched", () => {
  const eventTarget = new FakeEventTarget();
  let nowMs = 1_000;
  let prevented = 0;
  const decisions = [];
  const owner = installDocumentDoubleTapOwner(eventTarget, {
    now: () => nowMs,
    onDecision: (decision) => decisions.push(decision),
  });
  const first = touch(1, 30, 60);
  const second = touch(2, 90, 60);

  eventTarget.dispatch("touchstart", touchEvent({ touches: [first] }));
  eventTarget.dispatch("touchstart", touchEvent({ touches: [first, second] }));
  eventTarget.dispatch("touchend", touchEvent({
    touches: [second],
    changedTouches: [first],
    preventDefault: () => { prevented += 1; },
  }));
  eventTarget.dispatch("touchend", touchEvent({
    changedTouches: [second],
    preventDefault: () => { prevented += 1; },
  }));
  assert.equal(prevented, 0, "multi-touch endings must never suppress browser pinch");

  nowMs = 2_000;
  dispatchTap(
    eventTarget,
    touch(3, 40, 80),
    () => { prevented += 1; },
    textInputTarget
  );
  nowMs = 2_200;
  dispatchTap(
    eventTarget,
    touch(4, 42, 82),
    () => { prevented += 1; },
    textInputTarget
  );
  assert.equal(prevented, 0, "text inputs keep selection and caret defaults");
  assert.equal(decisions.at(-1).exclusionReason, "text_input");
  assert.equal(decisions.at(-1).isDoubleTap, true);
  owner.destroy();
});

test("activatable targets keep both taps, because preventing one drops its click", () => {
  const eventTarget = new FakeEventTarget();
  let nowMs = 1_000;
  let prevented = 0;
  const decisions = [];
  // 実際の closest と同じく、対象に一致する selector のときだけ要素を返す。
  const buttonTarget = {
    closest: (selector) => (selector.includes("button") ? { tagName: "BUTTON" } : null),
  };
  const owner = installDocumentDoubleTapOwner(eventTarget, {
    now: () => nowMs,
    onDecision: (decision) => decisions.push(decision),
  });

  dispatchTap(eventTarget, touch(1, 50, 90), () => { prevented += 1; }, buttonTarget);
  nowMs = 1_180;
  dispatchTap(eventTarget, touch(2, 51, 91), () => { prevented += 1; }, buttonTarget);
  assert.equal(
    prevented,
    0,
    "rapidly tapping a page button twice must activate it twice"
  );
  assert.equal(decisions.length, 2);
  assert.equal(decisions[0].excluded, true);
  assert.equal(decisions[0].exclusionReason, "button");
  assert.equal(decisions[1].exclusionReason, "button");
  assert.equal(decisions[1].elapsedMs, 180);
  assert.equal(decisions[1].distancePx, Math.hypot(1, 1));
  assert.equal(decisions[1].isDoubleTap, true);
  assert.equal(decisions[1].suppressed, false);

  // 操作部品でなければ、これまでどおり 2 打目だけ止める。
  nowMs = 3_000;
  dispatchTap(eventTarget, touch(3, 50, 90), () => { prevented += 1; });
  nowMs = 3_180;
  dispatchTap(eventTarget, touch(4, 51, 91), () => { prevented += 1; });
  assert.equal(prevented, 1);
  owner.destroy();
});

test("the document owner uses the browser suppression window", () => {
  const eventTarget = new FakeEventTarget();
  let nowMs = 1_000;
  let prevented = 0;
  const owner = installDocumentDoubleTapOwner(eventTarget, { now: () => nowMs });

  dispatchTap(eventTarget, touch(1, 60, 120), () => { prevented += 1; });
  nowMs += BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS - 40;
  dispatchTap(eventTarget, touch(2, 61, 121), () => { prevented += 1; });
  assert.equal(prevented, 1, "a pair inside the browser window must lose browser zoom");

  // ブラウザの窓も越えたら、ただの 2 回のタップ。
  nowMs += BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS + 60;
  dispatchTap(eventTarget, touch(3, 60, 120), () => { prevented += 1; });
  nowMs += BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS + 60;
  dispatchTap(eventTarget, touch(4, 61, 121), () => { prevented += 1; });
  assert.equal(prevented, 1);
  owner.destroy();
});

test("pickers and labels are suppressed, because the browser does zoom on them", () => {
  // 実機で `select` と `<label>` の上は拡大した。click で処理していないので、
  // 止めて失うのは 2 打目の起動転送だけ。
  for (const tagSelector of ["select", "label"]) {
    const eventTarget = new FakeEventTarget();
    let nowMs = 1_000;
    let prevented = 0;
    const target = {
      closest: (selector) => (selector.includes(tagSelector) ? { tagName: tagSelector } : null),
    };
    const owner = installDocumentDoubleTapOwner(eventTarget, { now: () => nowMs });
    dispatchTap(eventTarget, touch(1, 50, 90), () => { prevented += 1; }, target);
    nowMs = 1_180;
    dispatchTap(eventTarget, touch(2, 51, 91), () => { prevented += 1; }, target);
    assert.equal(prevented, 1, `${tagSelector} must not keep browser double-tap zoom`);
    owner.destroy();
  }
});
