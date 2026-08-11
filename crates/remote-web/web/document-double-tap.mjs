import { browserDoubleTapZoomDecision } from "./command-core.mjs";

const DOCUMENT_TAP_MAX_TRAVEL_PX = 12;

function touchByIdentifier(touches, identifier) {
  for (const touch of Array.from(touches ?? [])) {
    if (touch.identifier === identifier) return touch;
  }
  return null;
}

/// Observe browser double-tap pairs once for the whole document without preventing any default.
/// Multi-touch gestures never enter the tap sequence. Target type does not affect recognition,
/// so buttons, links, inputs, and plain elements follow the same path. An optional callback
/// exposes only the fixed decision facts; the caller owns any logging or correlation policy.
export function installDocumentDoubleTapOwner(
  eventTarget,
  {
    now = () => performance.now(),
    maxTapTravelPx = DOCUMENT_TAP_MAX_TRAVEL_PX,
    onDecision = null,
  } = {}
) {
  let gesture = null;
  let previousTap = null;

  // Keep this module independent from telemetry. The caller may observe the fixed, content-free
  // decision object, and an observer failure must never change browser gesture ownership.
  const notifyDecision = (decision) => {
    if (typeof onDecision !== "function") return;
    try {
      onDecision(decision);
    } catch {}
  };

  const rejectGesture = () => {
    gesture = null;
    previousTap = null;
  };

  const onTouchStart = (event) => {
    if (event.touches?.length !== 1) {
      gesture = { multiTouch: true };
      previousTap = null;
      return;
    }
    const touch = event.touches[0];
    gesture = {
      identifier: touch.identifier,
      startX: touch.clientX,
      startY: touch.clientY,
      maxTravelPx: 0,
      multiTouch: false,
    };
  };

  const onTouchMove = (event) => {
    if (!gesture || gesture.multiTouch) return;
    if (event.touches?.length !== 1) {
      gesture.multiTouch = true;
      previousTap = null;
      return;
    }
    const touch = touchByIdentifier(event.touches, gesture.identifier);
    if (!touch) {
      rejectGesture();
      return;
    }
    gesture.maxTravelPx = Math.max(
      gesture.maxTravelPx,
      Math.hypot(touch.clientX - gesture.startX, touch.clientY - gesture.startY)
    );
  };

  const onTouchEnd = (event) => {
    if (!gesture) return;
    if (gesture.multiTouch) {
      if (event.touches?.length === 0) gesture = null;
      previousTap = null;
      return;
    }
    if (event.touches?.length !== 0 || event.changedTouches?.length !== 1) {
      rejectGesture();
      return;
    }
    const touch = touchByIdentifier(event.changedTouches, gesture.identifier);
    const endTravelPx = touch
      ? Math.hypot(touch.clientX - gesture.startX, touch.clientY - gesture.startY)
      : 0;
    if (!touch) {
      rejectGesture();
      return;
    }
    const atMs = now();
    const eventCancelable = Boolean(event.cancelable);
    const currentTap = {
      x: touch.clientX,
      y: touch.clientY,
      atMs,
    };
    // ブラウザと同じく常に直前の tap と比べ、成立した対も次の観測候補として残す。
    const decision = browserDoubleTapZoomDecision(previousTap, currentTap);
    if (Math.max(gesture.maxTravelPx, endTravelPx) > maxTapTravelPx) {
      notifyDecision({
        decision: "travel_exceeded",
        atMs,
        elapsedMs: null,
        distancePx: null,
        isDoubleTap: false,
        suppressed: false,
        excluded: false,
        exclusionReason: null,
        cancelable: eventCancelable,
      });
      rejectGesture();
      return;
    }

    gesture = null;
    previousTap = decision.next;
    // 2 打目の既定は止めない。止めても iOS の拡大は起きる (suppressed の 46ms 後に
    // scale 1 -> 1.03 を実測) 一方で、合成 click が落ちてページ送りが消える
    // (pair_suppressed なのに command が出ていないタップを実測)。拡大は viewport の
    // maximum-scale / user-scalable で止める。ここは対の認識だけを所有し、観測に徹する。
    const suppressed = false;
    notifyDecision({
      decision: decision.zooms
        ? "pair_recognized"
        : (decision.elapsedMs === null ? "candidate_started" : "pair_rejected"),
      atMs,
      elapsedMs: decision.elapsedMs,
      distancePx: decision.distancePx,
      isDoubleTap: decision.zooms,
      suppressed,
      excluded: false,
      exclusionReason: null,
      cancelable: eventCancelable,
    });
  };

  const onTouchCancel = () => rejectGesture();
  eventTarget.addEventListener("touchstart", onTouchStart, {
    capture: true,
    passive: true,
  });
  eventTarget.addEventListener("touchmove", onTouchMove, {
    capture: true,
    passive: true,
  });
  eventTarget.addEventListener("touchend", onTouchEnd, {
    capture: true,
    passive: true,
  });
  eventTarget.addEventListener("touchcancel", onTouchCancel, {
    capture: true,
    passive: true,
  });

  return {
    destroy() {
      eventTarget.removeEventListener("touchstart", onTouchStart, true);
      eventTarget.removeEventListener("touchmove", onTouchMove, true);
      eventTarget.removeEventListener("touchend", onTouchEnd, true);
      eventTarget.removeEventListener("touchcancel", onTouchCancel, true);
      rejectGesture();
    },
  };
}
