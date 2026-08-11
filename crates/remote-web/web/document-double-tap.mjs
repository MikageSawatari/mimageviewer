import { browserDoubleTapZoomDecision } from "./command-core.mjs";

const DOCUMENT_TAP_MAX_TRAVEL_PX = 12;

function touchByIdentifier(touches, identifier) {
  for (const touch of Array.from(touches ?? [])) {
    if (touch.identifier === identifier) return touch;
  }
  return null;
}

/// 既定動作を残す対象。
///
/// 基準は「2 打目を止めたあと click だけを安全に送り直せるか」。リンクの navigation と
/// 文字入力の選択・キャレットは click の再送だけでは再現できないので既定へ残す。
///
/// `select` と `label` はここに入れない。以前「native の部品だからブラウザは拡大しない」
/// として除外したが、実際には拡大する。どちらもこちらが click で処理しておらず、
/// 止めて失うのは 2 打目の起動転送だけなので、拡大を許す理由にならない。
const DEFAULT_TAP_POLICIES = [
  // navigation / caret は click だけでは再現できないので、button 系より先に判定する。
  ["link", "a[href]", "exclude"],
  ["textarea", "textarea", "exclude"],
  ["contenteditable", "[contenteditable]", "exclude"],
  // range / checkbox / radio / button 系は文字入力ではなく、拡大だけが残る。
  [
    "text_input",
    'input:not([type="range"]):not([type="checkbox"]):not([type="radio"])' +
      ':not([type="button"]):not([type="submit"])',
    "exclude",
  ],
  ["button", "button", "replay_click"],
  ["role_button", '[role="button"]', "replay_click"],
  // button 自体が role を持つ場合は click 再送でよい。それ以外の ARIA 部品は現状維持。
  ["role_link", '[role="link"]', "exclude"],
  ["role_tab", '[role="tab"]', "exclude"],
  ["role_option", '[role="option"]', "exclude"],
];

function defaultTapPolicy(target) {
  for (const [reason, selector, action] of DEFAULT_TAP_POLICIES) {
    const owner = target?.closest?.(selector);
    if (!owner) continue;
    return action === "replay_click"
      ? { exclusionReason: null, clickReplayTarget: owner }
      : { exclusionReason: reason, clickReplayTarget: null };
  }
  return { exclusionReason: null, clickReplayTarget: null };
}

/// Own browser double-tap suppression once for the whole document. The first tap and every
/// non-matching touchend keep their defaults; only the second tap of a recognized pair is
/// prevented. Multi-touch gestures never enter the tap sequence, leaving browser pinch intact.
/// Preventing a touchend also drops its synthesized click. Button-like targets therefore receive
/// one owner-generated click, while links and text editing keep their browser defaults. An
/// optional callback exposes only the fixed decision facts; the caller owns any logging or
/// correlation policy.
export function installDocumentDoubleTapOwner(
  eventTarget,
  {
    now = () => performance.now(),
    maxTapTravelPx = DOCUMENT_TAP_MAX_TRAVEL_PX,
    onDecision = null,
    createClickEvent = (type, init) => new MouseEvent(type, init),
  } = {}
) {
  let gesture = null;
  let previousTap = null;
  let pendingBrowserClick = null;
  let pendingBrowserClickTimer = null;
  const replayedClicks = new WeakSet();

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

  const clearPendingBrowserClick = () => {
    if (pendingBrowserClickTimer !== null) {
      clearTimeout(pendingBrowserClickTimer);
      pendingBrowserClickTimer = null;
    }
    pendingBrowserClick = null;
  };

  const targetBelongsTo = (target, owner) =>
    target === owner || Boolean(owner?.contains?.(target));

  // preventDefault 済みの touchend から browser click が通常は生成されない。それでも
  // WebKit の挙動差などで届いた場合、owner が再送した click と二重起動させない。
  // 0ms timer より後の click は次の利用者操作なので対象にしない。
  const onClick = (event) => {
    if (replayedClicks.has(event)) return;
    if (
      !pendingBrowserClick ||
      !targetBelongsTo(event.target, pendingBrowserClick.target)
    ) return;
    clearPendingBrowserClick();
    if (event.cancelable) event.preventDefault();
    event.stopImmediatePropagation?.();
    event.stopPropagation?.();
  };

  const replayClick = (target, touch) => {
    if (!target || target.disabled === true) return;
    const click = createClickEvent("click", {
      bubbles: true,
      cancelable: true,
      composed: true,
      clientX: touch.clientX,
      clientY: touch.clientY,
      button: 0,
      buttons: 0,
      detail: 1,
      view: target.ownerDocument?.defaultView ?? null,
    });
    replayedClicks.add(click);
    target.dispatchEvent(click);
  };

  const expectSuppressedBrowserClick = (target) => {
    clearPendingBrowserClick();
    pendingBrowserClick = { target };
    pendingBrowserClickTimer = setTimeout(clearPendingBrowserClick, 0);
  };

  const onTouchStart = (event) => {
    if (event.touches?.length !== 1) {
      gesture = { multiTouch: true };
      previousTap = null;
        return;
    }
    const touch = event.touches[0];
    const tapPolicy = defaultTapPolicy(event.target);
    gesture = {
      identifier: touch.identifier,
      startX: touch.clientX,
      startY: touch.clientY,
      maxTravelPx: 0,
      exclusionReason: tapPolicy.exclusionReason,
      clickReplayTarget: tapPolicy.clickReplayTarget,
      multiTouch: false,
    };
    // 除外は「この 1 打の既定動作を残す」だけの話で、履歴は捨てない。ブラウザは対象の
    // 種類で対を作るかを変えないので、こちらも変えない。
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
    // 抑止の判定と記録は同じ 1 つの結果から出す。対を消費しなくなったので、観測用に
    // 別の履歴を持つ理由も無くなった。
    const decision = browserDoubleTapZoomDecision(previousTap, currentTap);
    if (gesture.exclusionReason) {
      notifyDecision({
        decision: "excluded_target",
        atMs,
        elapsedMs: decision.elapsedMs,
        distancePx: decision.distancePx,
        isDoubleTap: decision.zooms,
        suppressed: false,
        excluded: true,
        exclusionReason: gesture.exclusionReason,
        cancelable: eventCancelable,
      });
      gesture = null;
      // 除外対象でも「タップが 1 回あった」ことは変わらない。ブラウザは次のタップと
      // 組にして拡大するので、候補として残す。
      previousTap = decision.next;
      return;
    }
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

    const clickReplayTarget = gesture.clickReplayTarget;
    gesture = null;
    previousTap = decision.next;
    const suppressed = decision.zooms && eventCancelable;
    if (suppressed) {
      event.preventDefault();
      if (clickReplayTarget) {
        replayClick(clickReplayTarget, touch);
        expectSuppressedBrowserClick(clickReplayTarget);
      }
    }
    notifyDecision({
      decision: decision.zooms
        ? (suppressed ? "pair_suppressed" : "pair_not_cancelable")
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
    passive: false,
  });
  eventTarget.addEventListener("touchcancel", onTouchCancel, {
    capture: true,
    passive: true,
  });
  eventTarget.addEventListener("click", onClick, {
    capture: true,
    passive: false,
  });

  return {
    destroy() {
      eventTarget.removeEventListener("touchstart", onTouchStart, true);
      eventTarget.removeEventListener("touchmove", onTouchMove, true);
      eventTarget.removeEventListener("touchend", onTouchEnd, true);
      eventTarget.removeEventListener("touchcancel", onTouchCancel, true);
      eventTarget.removeEventListener("click", onClick, true);
      clearPendingBrowserClick();
      rejectGesture();
    },
  };
}
