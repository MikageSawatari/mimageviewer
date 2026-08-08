import {
  doubleTapSequenceTransition,
} from "./command-core.mjs";

const DOCUMENT_TAP_MAX_TRAVEL_PX = 12;

function touchByIdentifier(touches, identifier) {
  for (const touch of Array.from(touches ?? [])) {
    if (touch.identifier === identifier) return touch;
  }
  return null;
}

/// 既定動作を残す対象。
///
/// 操作部品は `click` で動くものがあり (ページ送りの ‹ › など)、2 打目を止めると
/// その操作だけ効かなくなる。素早く 2 回押すのは普通の使い方なので落とせない。
/// これらは自分自身に `touch-action` を宣言済みか native の部品で、そもそも
/// ブラウザが double-tap zoom をしないため、止める必要も無い。
/// 文字入力は選択・キャレット操作の既定が要る。
const KEEPS_DEFAULT_TAP_BEHAVIOUR = [
  "button",
  "a[href]",
  "select",
  "input",
  "textarea",
  "label",
  "[contenteditable]",
  '[role="button"]',
  '[role="link"]',
  '[role="tab"]',
  '[role="option"]',
].join(", ");

function keepsDefaultTapBehaviour(target) {
  return Boolean(target?.closest?.(KEEPS_DEFAULT_TAP_BEHAVIOUR));
}

/// Own browser double-tap suppression once for the whole document. The first tap and every
/// non-matching touchend keep their defaults; only the second tap of a recognized pair is
/// prevented. Multi-touch gestures never enter the tap sequence, leaving browser pinch intact.
/// Preventing a touchend also drops its synthesized click, so activatable targets are left
/// alone (see KEEPS_DEFAULT_TAP_BEHAVIOUR).
export function installDocumentDoubleTapOwner(
  eventTarget,
  {
    now = () => performance.now(),
    maxTapTravelPx = DOCUMENT_TAP_MAX_TRAVEL_PX,
  } = {}
) {
  let gesture = null;
  let previousTap = null;

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
    const ignored = keepsDefaultTapBehaviour(event.target);
    gesture = {
      identifier: touch.identifier,
      startX: touch.clientX,
      startY: touch.clientY,
      maxTravelPx: 0,
      ignored,
      multiTouch: false,
    };
    if (ignored) previousTap = null;
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
    if (
      !touch ||
      gesture.ignored ||
      Math.max(gesture.maxTravelPx, endTravelPx) > maxTapTravelPx
    ) {
      rejectGesture();
      return;
    }

    const transition = doubleTapSequenceTransition(previousTap, {
      x: touch.clientX,
      y: touch.clientY,
      atMs: now(),
    });
    gesture = null;
    previousTap = transition.next;
    if (transition.isDoubleTap && event.cancelable !== false) {
      event.preventDefault();
    }
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
    passive: false,
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
