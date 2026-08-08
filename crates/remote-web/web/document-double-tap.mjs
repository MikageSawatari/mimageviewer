import {
  BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS,
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
/// 基準は「2 打目を止めると本当に失われる操作があるか」だけ。2 打目の `touchend` を
/// 止めると合成 click も落ちるので、click で起動するもの (ページ送りの ‹ › など) は
/// 素早く 2 回押したときに 2 回目が効かなくなる。文字入力は選択・キャレットの既定が要る。
///
/// `select` と `label` はここに入れない。以前「native の部品だからブラウザは拡大しない」
/// として除外したが、実際には拡大する。どちらもこちらが click で処理しておらず、
/// 止めて失うのは 2 打目の起動転送だけなので、拡大を許す理由にならない。
const KEEPS_DEFAULT_TAP_BEHAVIOUR = [
  "button",
  "a[href]",
  "textarea",
  "[contenteditable]",
  '[role="button"]',
  '[role="link"]',
  '[role="tab"]',
  '[role="option"]',
  // range / checkbox / radio / button 系は文字入力ではなく、拡大だけが残る。
  'input:not([type="range"]):not([type="checkbox"]):not([type="radio"])' +
    ':not([type="button"]):not([type="submit"])',
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

    const transition = doubleTapSequenceTransition(
      previousTap,
      {
        x: touch.clientX,
        y: touch.clientY,
        atMs: now(),
      },
      // アプリのジェスチャではなく、ブラウザが拡大と見なす窓に合わせる。
      { maxDelayMs: BROWSER_DOUBLE_TAP_ZOOM_MAX_DELAY_MS }
    );
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
