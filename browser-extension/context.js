// Geometry only, in the extension's isolated top-frame world. No page content.
function mivReadView() {
  return {
    screenX: window.screenX, screenY: window.screenY,
    outerWidth: window.outerWidth, outerHeight: window.outerHeight,
    innerWidth: window.innerWidth, innerHeight: window.innerHeight,
    devicePixelRatio: window.devicePixelRatio,
    context_event: globalThis.__mivBrowserContext?.event ?? null
  };
}

(() => {
  if (typeof window === "undefined" || window !== window.top) return;
  if (globalThis.__mivBrowserContext) return;
  const state = { event: null };
  globalThis.__mivBrowserContext = state;
  window.addEventListener("contextmenu", event => {
    state.event = {
      screenX: event.screenX, screenY: event.screenY,
      clientX: event.clientX, clientY: event.clientY,
      win_screenX: window.screenX, win_screenY: window.screenY,
      innerWidth: window.innerWidth, innerHeight: window.innerHeight,
      devicePixelRatio: window.devicePixelRatio
    };
  }, { capture: true, passive: true });
})();

if (typeof module !== "undefined") module.exports = { mivReadView };
