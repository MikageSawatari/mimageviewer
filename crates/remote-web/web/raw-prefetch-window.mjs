import { pagePrefetchPlan, pageAdmissionRetryDelayMs } from "./command-core.mjs";

// api_raw_prefetch_window uses media_admission_busy_response and
// media_ipc_error_response. A 503 miv_media_error denotes MediaErrorCode::Busy;
// permanent media errors have other HTTP statuses. Keep page request policy separate.
const TRANSIENT_HTTP_ERRORS = new Set([
  "ipc_timeout", "ipc_busy", "admission_busy", "raw_busy", "miv_media_error",
]);

// Entries are image positions, not expanded presentation groups. A partner or
// cover therefore enters only at its own position, regardless of cached bytes.
export function rawPrefetchWindow({ images, visibleIndexes, direction, addressOf }) {
  return pagePrefetchPlan({ visibleIndexes, itemCount: images.length, direction, ahead: 2, behind: 1 })
    .map((index) => addressOf(images[index]));
}

export class RawPrefetchWindowPublisher {
  constructor({ send, delay }) {
    this.send = send;
    this.delay = delay;
    this.committed = null;
    this.session = "";
    this.generation = 0;
    this.stamp = null;
    this.controller = null;
    this.pending = null;
    this.sending = false;
  }

  setSession(session) {
    if (session === this.session) return;
    this.controller?.abort();
    this.session = session;
    this.generation = 0;
    this.stamp = null;
    this.pending = null;
    // Only a presentation commit can supply this snapshot. Opening a viewer's
    // position must not publish a display that has not actually been presented.
    if (session && this.committed) this.publish(this.committed);
  }

  commit(presentation) {
    if (!presentation) return;
    const { unit, direction, entries } = presentation;
    this.committed = structuredClone({ unit, direction, entries });
    this.publish(this.committed);
  }

  publish(presentation) {
    if (!this.session) return;
    const { unit, direction, entries } = presentation;
    const stamp = JSON.stringify([unit, direction, entries]);
    if (stamp === this.stamp) return;
    this.stamp = stamp;
    this.pending = {
      session: this.session,
      body: { window_generation: ++this.generation, entries },
    };
    this.controller?.abort();
    this.flush();
  }

  leave() {
    this.committed = null;
    if (this.stamp !== null) this.publish({ unit: null, direction: 0, entries: [] });
  }

  async flush() {
    if (this.sending) return;
    this.sending = true;
    try {
      while (this.pending) {
        const declaration = this.pending;
        this.pending = null;
        const controller = new AbortController();
        this.controller = controller;
        let attempt = 0;
        while (!controller.signal.aborted && this.session === declaration.session) {
          let retryAfterMs;
          try {
            const response = await this.send(declaration.body, declaration.session, controller.signal);
            if (response.status !== 503) break;
            retryAfterMs = Number(response.headers?.get("Retry-After")) * 1000;
            // Preserve body-read network failures for the same retry handler as
            // fetch failures. Invalid JSON is a SyntaxError and remains terminal.
            const detail = await response.clone().json();
            if (!TRANSIENT_HTTP_ERRORS.has(detail?.error)) break;
          } catch (error) {
            // fetch reports network failures as TypeError; programming errors,
            // attestation failures and permanent HTTP failures must not retry.
            if (controller.signal.aborted || !(error instanceof TypeError) || error?.retryable === false) break;
          }
          if (controller.signal.aborted || this.session !== declaration.session) break;
          try {
            await this.delay(pageAdmissionRetryDelayMs(retryAfterMs, attempt++), controller.signal);
          } catch { break; }
        }
        if (this.controller === controller) this.controller = null;
      }
    } finally {
      this.sending = false;
    }
  }
}
