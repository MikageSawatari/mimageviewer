import test from "node:test";
import assert from "node:assert/strict";

import {
  BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS,
  BROWSER_DOUBLE_TAP_ZOOM_MAX_DISTANCE_PX,
  browserDoubleTapZoomDecision,
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
const buttonTarget = {
  closest: (selector) => selector === "button" ? { tagName: "BUTTON" } : null,
};
const linkTarget = {
  closest: (selector) => selector === "a[href]" ? { tagName: "A" } : null,
};
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

test("the browser rule keeps every tap as a candidate and uses its own slop", () => {
  const first = browserDoubleTapZoomDecision(null, { x: 40, y: 80, atMs: 1_000 });
  assert.equal(first.zooms, false);
  assert.deepEqual(first.next, { x: 40, y: 80, atMs: 1_000 });

  // 実機で 148px 離れた 2 打が拡大した。アプリのジェスチャより広く取る。
  const far = browserDoubleTapZoomDecision(first.next, { x: 190, y: 80, atMs: 1_150 });
  assert.equal(far.zooms, true);
  assert.ok(far.distancePx > 36 && far.distancePx <= BROWSER_DOUBLE_TAP_ZOOM_MAX_DISTANCE_PX);

  const tooFar = browserDoubleTapZoomDecision(first.next, {
    x: 40 + BROWSER_DOUBLE_TAP_ZOOM_MAX_DISTANCE_PX + 20,
    y: 80,
    atMs: 1_150,
  });
  assert.equal(tooFar.zooms, false);

  const late = browserDoubleTapZoomDecision(first.next, {
    x: 40,
    y: 80,
    atMs: 1_000 + BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS + 60,
  });
  assert.equal(late.zooms, false);
});

test("a recognized pair does not consume its second tap", () => {
  // 実機では成立した対の直後のタップも前のタップと組になるため、観測側も常に
  // 直前のタップと比べる。
  const first = browserDoubleTapZoomDecision(null, { x: 50, y: 50, atMs: 1_000 });
  const second = browserDoubleTapZoomDecision(first.next, { x: 52, y: 51, atMs: 1_150 });
  assert.equal(second.zooms, true);
  assert.deepEqual(second.next, { x: 52, y: 51, atMs: 1_150 });

  const third = browserDoubleTapZoomDecision(second.next, { x: 53, y: 52, atMs: 1_300 });
  assert.equal(third.zooms, true, "the third tap still pairs with the second");
});

test("document observer preserves every touchend and reports recognized pairs", () => {
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
  assert.equal(prevented, 0, "a recognized pair must keep its synthetic click");
  assert.deepEqual(decisions.at(-1), {
    decision: "pair_recognized",
    atMs: 1_220,
    elapsedMs: 220,
    distancePx: Math.hypot(4, 4),
    isDoubleTap: true,
    suppressed: false,
    excluded: false,
    exclusionReason: null,
    cancelable: true,
  });

  // 窓の外であることを定数から決める。数値で書くと、窓を広げたときに黙って意味が変わる。
  nowMs = 2_000;
  dispatchTap(eventTarget, touch(3, 40, 80), () => { prevented += 1; });
  nowMs += BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS + 60;
  dispatchTap(eventTarget, touch(4, 40, 80), () => { prevented += 1; });
  assert.equal(prevented, 0, "non-matching touchends must keep their defaults");

  nowMs = 3_000;
  eventTarget.dispatch("touchstart", touchEvent({ touches: [touch(5, 40, 80)] }));
  eventTarget.dispatch("touchend", touchEvent({
    changedTouches: [touch(5, 140, 80)],
    preventDefault: () => { prevented += 1; },
  }));
  assert.equal(decisions.at(-1).decision, "travel_exceeded");
  nowMs = 3_100;
  dispatchTap(eventTarget, touch(6, 142, 80), () => { prevented += 1; });
  assert.equal(prevented, 0, "a moved gesture must keep its default and not seed a pair");
  assert.equal(decisions.at(-1).decision, "candidate_started");

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
  assert.equal(prevented, 0);
  assert.equal(decisions.at(-1).decision, "pair_recognized");
  assert.equal(decisions.at(-1).isDoubleTap, true);
  assert.equal(decisions.at(-1).suppressed, false);
  assert.equal(decisions.at(-1).cancelable, false);
  owner.destroy();
});

test("document observer leaves multi-touch out of the pair sequence", () => {
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
  assert.equal(decisions.length, 0, "multi-touch must not enter double-tap observations");

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
  assert.equal(decisions.at(-1).decision, "pair_recognized");
  assert.equal(decisions.at(-1).isDoubleTap, true);
  assert.equal(decisions.at(-1).excluded, false);
  assert.equal(decisions.at(-1).exclusionReason, null);
  owner.destroy();
});

test("buttons, links, inputs, and plain elements share the observation contract", () => {
  for (const [kind, target] of [
    ["button", buttonTarget],
    ["link", linkTarget],
    ["input", textInputTarget],
    ["plain", plainTarget],
  ]) {
    const eventTarget = new FakeEventTarget();
    let nowMs = 1_000;
    let prevented = 0;
    const decisions = [];
    const owner = installDocumentDoubleTapOwner(eventTarget, {
      now: () => nowMs,
      onDecision: (decision) => decisions.push(decision),
    });

    dispatchTap(eventTarget, touch(1, 50, 90), () => { prevented += 1; }, target);
    nowMs = 1_180;
    dispatchTap(eventTarget, touch(2, 51, 91), () => { prevented += 1; }, target);

    assert.equal(prevented, 0, `${kind} must keep both tap defaults`);
    assert.equal(decisions.length, 2, `${kind} must report both taps`);
    assert.deepEqual(decisions.map((decision) => decision.decision), [
      "candidate_started",
      "pair_recognized",
    ]);
    assert.equal(decisions[1].elapsedMs, 180);
    assert.equal(decisions[1].distancePx, Math.hypot(1, 1));
    assert.equal(decisions[1].isDoubleTap, true);
    assert.equal(decisions[1].suppressed, false);
    assert.equal(decisions[1].excluded, false);
    assert.equal(decisions[1].exclusionReason, null);
    owner.destroy();
  }
});

test("the document observer uses the browser recognition window", () => {
  const eventTarget = new FakeEventTarget();
  let nowMs = 1_000;
  let prevented = 0;
  const decisions = [];
  const owner = installDocumentDoubleTapOwner(eventTarget, {
    now: () => nowMs,
    onDecision: (decision) => decisions.push(decision),
  });

  dispatchTap(eventTarget, touch(1, 60, 120), () => { prevented += 1; });
  nowMs += BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS - 40;
  dispatchTap(eventTarget, touch(2, 61, 121), () => { prevented += 1; });
  assert.equal(prevented, 0, "a recognized pair must keep both defaults");
  assert.equal(decisions.at(-1).decision, "pair_recognized");
  assert.equal(decisions.at(-1).isDoubleTap, true);

  // ブラウザの窓も越えたら、ただの 2 回のタップ。
  nowMs += BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS + 60;
  dispatchTap(eventTarget, touch(3, 60, 120), () => { prevented += 1; });
  nowMs += BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS + 60;
  dispatchTap(eventTarget, touch(4, 61, 121), () => { prevented += 1; });
  assert.equal(prevented, 0);
  assert.equal(decisions.at(-1).decision, "pair_rejected");
  assert.equal(decisions.at(-1).isDoubleTap, false);
  owner.destroy();
});

test("pickers and labels are observed without suppressing their defaults", () => {
  for (const tagSelector of ["select", "label"]) {
    const eventTarget = new FakeEventTarget();
    let nowMs = 1_000;
    let prevented = 0;
    const decisions = [];
    const target = {
      closest: (selector) => (selector.includes(tagSelector) ? { tagName: tagSelector } : null),
    };
    const owner = installDocumentDoubleTapOwner(eventTarget, {
      now: () => nowMs,
      onDecision: (decision) => decisions.push(decision),
    });
    dispatchTap(eventTarget, touch(1, 50, 90), () => { prevented += 1; }, target);
    nowMs = 1_180;
    dispatchTap(eventTarget, touch(2, 51, 91), () => { prevented += 1; }, target);
    assert.equal(prevented, 0, `${tagSelector} must keep both tap defaults`);
    assert.equal(decisions.at(-1).decision, "pair_recognized");
    assert.equal(decisions.at(-1).isDoubleTap, true);
    assert.equal(decisions.at(-1).suppressed, false);
    assert.equal(decisions.at(-1).excluded, false);
    assert.equal(decisions.at(-1).exclusionReason, null);
    owner.destroy();
  }
});
