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
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event);
      if (event.immediatePropagationStopped) break;
    }
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

function fakeClickEvent(type, init) {
  return {
    type,
    ...init,
    target: null,
    defaultPrevented: false,
    immediatePropagationStopped: false,
    preventDefault() {
      if (this.cancelable) this.defaultPrevented = true;
    },
    stopImmediatePropagation() {
      this.immediatePropagationStopped = true;
    },
    stopPropagation() {},
  };
}

function dispatchBrowserClick(eventTarget, target) {
  const event = fakeClickEvent("click", {
    bubbles: true,
    cancelable: true,
    composed: true,
  });
  event.target = target;
  eventTarget.dispatch("click", event);
  return event;
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

test("a suppressed pair does not consume its second tap", () => {
  // 実機で、抑止した対の直後のタップが前のタップと組になって拡大した。ブラウザは
  // こちらの「対を消費する」規則を知らないので、常に直前のタップと比べる。
  const first = browserDoubleTapZoomDecision(null, { x: 50, y: 50, atMs: 1_000 });
  const second = browserDoubleTapZoomDecision(first.next, { x: 52, y: 51, atMs: 1_150 });
  assert.equal(second.zooms, true);
  assert.deepEqual(second.next, { x: 52, y: 51, atMs: 1_150 });

  const third = browserDoubleTapZoomDecision(second.next, { x: 53, y: 52, atMs: 1_300 });
  assert.equal(third.zooms, true, "the third tap still pairs with the second");
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

test("document owner leaves pinch, links, and text editing defaults untouched", () => {
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

  for (const [reason, selectors] of [
    ["link", ['a[href]', '[role="button"]']],
    ["contenteditable", ["[contenteditable]"]],
  ]) {
    const target = {
      closest: (candidate) => selectors.includes(candidate) ? { tagName: "SPAN" } : null,
    };
    nowMs += 1_000;
    dispatchTap(
      eventTarget,
      touch(5, 40, 80),
      () => { prevented += 1; },
      target
    );
    nowMs += 200;
    dispatchTap(
      eventTarget,
      touch(6, 42, 82),
      () => { prevented += 1; },
      target
    );
    assert.equal(decisions.at(-1).decision, "excluded_target");
    assert.equal(decisions.at(-1).exclusionReason, reason);
    assert.equal(decisions.at(-1).suppressed, false);
  }
  assert.equal(prevented, 0, "link and editing defaults must not be prevented");
  owner.destroy();
});

test("button-like targets suppress the pair and replay the second click exactly once", () => {
  for (const [reason, selectors] of [
    ["button", ["button"]],
    ["role_button", ['[role="button"]']],
    ["button_role_tab", ["button", '[role="tab"]']],
  ]) {
    const eventTarget = new FakeEventTarget();
    let nowMs = 1_000;
    let prevented = 0;
    let clicks = 0;
    const observedClicks = [];
    const decisions = [];
    const button = {
      disabled: false,
      contains: (candidate) => candidate === button,
      closest: (candidate) => selectors.includes(candidate) ? button : null,
      dispatchEvent(event) {
        event.target = button;
        eventTarget.dispatch(event.type, event);
        return !event.defaultPrevented;
      },
    };
    const owner = installDocumentDoubleTapOwner(eventTarget, {
      now: () => nowMs,
      onDecision: (decision) => decisions.push(decision),
      createClickEvent: fakeClickEvent,
    });
    eventTarget.addEventListener("click", (event) => {
      clicks += 1;
      observedClicks.push(event);
    });

    dispatchTap(eventTarget, touch(1, 50, 90), () => { prevented += 1; }, button);
    dispatchBrowserClick(eventTarget, button);
    assert.equal(clicks, 1, "the first browser click must pass through");

    nowMs = 1_180;
    dispatchTap(eventTarget, touch(2, 51, 91), () => { prevented += 1; }, button);
    assert.equal(prevented, 1, `${reason} must suppress browser double-tap zoom`);
    assert.equal(clicks, 2, `${reason} must receive one replayed second click`);
    assert.equal(observedClicks.at(-1).bubbles, true);
    assert.equal(observedClicks.at(-1).cancelable, true);
    assert.equal(observedClicks.at(-1).composed, true);
    assert.equal(decisions.length, 2);
    assert.equal(decisions[0].decision, "candidate_started");
    assert.equal(decisions[1].decision, "pair_suppressed");
    assert.equal(decisions[1].elapsedMs, 180);
    assert.equal(decisions[1].distancePx, Math.hypot(1, 1));
    assert.equal(decisions[1].isDoubleTap, true);
    assert.equal(decisions[1].suppressed, true);
    assert.equal(decisions[1].excluded, false);
    assert.equal(decisions[1].exclusionReason, null);

    const unexpectedBrowserClick = dispatchBrowserClick(eventTarget, button);
    assert.equal(clicks, 2, "an unexpected browser click must not activate twice");
    assert.equal(unexpectedBrowserClick.defaultPrevented, true);
    owner.destroy();
  }
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
