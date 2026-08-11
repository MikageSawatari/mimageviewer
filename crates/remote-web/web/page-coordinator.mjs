export const PageCoordinatorEffectType = Object.freeze({
  START: "start",
  PROMOTE: "promote",
  CANCEL: "cancel",
  GROUP_READY: "group_ready",
  GROUP_FAILED: "group_failed",
  IGNORED: "ignored",
});

export const PageJobPriority = Object.freeze({
  FOREGROUND: "foreground",
  PREFETCH: "prefetch",
});

export const PageJobState = Object.freeze({
  RUNNING: "running",
  READY: "ready",
  FAILED: "failed",
  ABORTED: "aborted",
  CANCELLED: "cancelled",
});

export const PageCancelCause = Object.freeze({
  NO_DEMAND: "no_demand",
  SESSION_INVALIDATED: "session_invalidated",
  CONTEXT_RESET: "context_reset",
});

export const PageGroupFailureReason = Object.freeze({
  MEMBER_FAILED: "member_failed",
  MEMBER_ABORTED: "member_aborted",
  SESSION_INVALIDATED: "session_invalidated",
  CONTEXT_RESET: "context_reset",
});

export const PageIgnoredReason = Object.freeze({
  UNKNOWN_JOB: "unknown_job",
  STALE_JOB: "stale_job",
  ALREADY_SETTLED: "already_settled",
  UNKNOWN_REQUEST: "unknown_request",
  DUPLICATE_REQUEST_ID: "duplicate_request_id",
  INVALID_REQUEST: "invalid_request",
  INVALID_PLAN: "invalid_plan",
  INVALID_OUTCOME: "invalid_outcome",
  INVALID_CAUSE: "invalid_cause",
});

const SETTLED_JOB_HISTORY_LIMIT = 256;

const EFFECT_ORDER = Object.freeze({
  [PageCoordinatorEffectType.IGNORED]: 0,
  [PageCoordinatorEffectType.CANCEL]: 1,
  [PageCoordinatorEffectType.PROMOTE]: 2,
  [PageCoordinatorEffectType.START]: 3,
  [PageCoordinatorEffectType.GROUP_READY]: 4,
  [PageCoordinatorEffectType.GROUP_FAILED]: 4,
});

const TERMINAL_JOB_STATES = new Set([
  PageJobState.READY,
  PageJobState.FAILED,
  PageJobState.ABORTED,
]);

const RELEASE_CAUSES = new Set(Object.values(PageCancelCause));
const INVALIDATION_CAUSES = new Set([
  PageCancelCause.SESSION_INVALIDATED,
  PageCancelCause.CONTEXT_RESET,
]);

function stableJsonValue(value) {
  if (Array.isArray(value)) return value.map(stableJsonValue);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, stableJsonValue(value[key])])
    );
  }
  return value;
}

/// Every value that can affect the response bytes is represented in a fixed
/// position. JSON array framing keeps field boundaries unambiguous.
export function pageResourceKey({
  address,
  targetPx,
  renderRevision,
  generation,
  sessionId,
  sessionCacheEpoch,
  renderContext = null,
  adjustmentPreview = null,
} = {}) {
  return {
    id: JSON.stringify([
      "miv-page-resource-v1",
      address,
      targetPx,
      renderRevision,
      generation,
      sessionId,
      sessionCacheEpoch,
      stableJsonValue(renderContext),
      stableJsonValue(adjustmentPreview),
    ]),
    cacheable: adjustmentPreview == null,
  };
}

function orderedEffects(effects) {
  return effects
    .map((effect, index) => ({ effect, index }))
    .sort((left, right) =>
      (EFFECT_ORDER[left.effect.type] ?? Number.MAX_SAFE_INTEGER)
        - (EFFECT_ORDER[right.effect.type] ?? Number.MAX_SAFE_INTEGER)
      || left.index - right.index
    )
    .map(({ effect }) => effect);
}

function uniqueKeys(keys) {
  return [...new Set(keys)];
}

function validKeyId(keyId) {
  return typeof keyId === "string" && keyId.length > 0;
}

export class PageDisplayCoordinator {
  #hasBytes;
  #prefetchAdmits;
  #prefetchConcurrency;
  #displayRequestSequence;
  #jobSequence;
  #requests;
  #seenRequestIds;
  #displayDemands;
  #planKeys;
  #planKeySet;
  #jobsById;
  #currentJobIds;
  #terminalFailureKeys;

  constructor({
    hasBytes = () => false,
    prefetchAdmits = () => true,
    prefetchConcurrency = 2,
  } = {}) {
    this.#hasBytes = hasBytes;
    this.#prefetchAdmits = prefetchAdmits;
    const concurrency = Number(prefetchConcurrency);
    this.#prefetchConcurrency = Number.isFinite(concurrency)
      ? Math.max(0, Math.floor(concurrency))
      : 2;
    this.#displayRequestSequence = 0;
    this.#jobSequence = 0;
    this.#requests = new Map();
    this.#seenRequestIds = new Set();
    this.#displayDemands = new Map();
    this.#planKeys = [];
    this.#planKeySet = new Set();
    this.#jobsById = new Map();
    this.#currentJobIds = new Map();
    this.#terminalFailureKeys = new Set();
  }

  nextDisplayRequestId() {
    do {
      this.#displayRequestSequence += 1;
    } while (this.#seenRequestIds.has(String(this.#displayRequestSequence)));
    return String(this.#displayRequestSequence);
  }

  openDisplay({ requestId, groupKey, keys } = {}) {
    if (
      typeof requestId !== "string" ||
      requestId.length === 0 ||
      !Array.isArray(keys) ||
      keys.length < 1 ||
      keys.length > 2 ||
      keys.some((keyId) => !validKeyId(keyId))
    ) {
      return [{ type: "ignored", reason: "invalid_request", requestId }];
    }
    if (this.#seenRequestIds.has(requestId)) {
      return [{ type: "ignored", reason: "duplicate_request_id", requestId }];
    }
    const requestKeys = uniqueKeys(keys);
    // A new display request is an explicit reader retry. It must never inherit
    // a terminal result from an attempt that finished before it opened.
    for (const keyId of requestKeys) this.#terminalFailureKeys.delete(keyId);
    const request = {
      requestId,
      groupKey,
      keys: requestKeys,
      members: new Map(),
      outcome: null,
    };
    this.#seenRequestIds.add(requestId);
    this.#requests.set(requestId, request);
    for (const keyId of requestKeys) {
      const demands = this.#displayDemands.get(keyId) ?? new Set();
      demands.add(requestId);
      this.#displayDemands.set(keyId, demands);
      request.members.set(keyId, this.#availableMemberState(keyId));
    }
    const effects = [];
    this.#reconcile(effects, PageCancelCause.NO_DEMAND);
    this.#collectGroupOutcomes(effects);
    return orderedEffects(effects);
  }

  releaseDisplay(requestId, cause) {
    const request = this.#requests.get(requestId);
    if (!request) {
      return [{ type: "ignored", reason: "unknown_request", requestId }];
    }
    if (!RELEASE_CAUSES.has(cause)) {
      return [{ type: "ignored", reason: "invalid_cause", requestId, cause }];
    }
    this.#requests.delete(requestId);
    for (const keyId of request.keys) {
      const demands = this.#displayDemands.get(keyId);
      demands?.delete(requestId);
      if (demands?.size === 0) this.#displayDemands.delete(keyId);
    }
    const effects = [];
    this.#reconcile(effects, cause);
    this.#collectGroupOutcomes(effects);
    return orderedEffects(effects);
  }

  setPlan(keys) {
    if (!Array.isArray(keys) || keys.some((keyId) => !validKeyId(keyId))) {
      return [{ type: "ignored", reason: "invalid_plan" }];
    }
    this.#planKeys = uniqueKeys(keys);
    this.#planKeySet = new Set(this.#planKeys);
    const effects = [];
    this.#reconcile(effects, PageCancelCause.NO_DEMAND);
    this.#collectGroupOutcomes(effects);
    return orderedEffects(effects);
  }

  settle(jobId, outcome) {
    if (!outcome || !TERMINAL_JOB_STATES.has(outcome.status)) {
      return [{ type: "ignored", reason: "invalid_outcome", jobId }];
    }
    const job = this.#jobsById.get(jobId);
    if (!job) return [{ type: "ignored", reason: "unknown_job", jobId }];
    if (job.state !== PageJobState.RUNNING) {
      if (TERMINAL_JOB_STATES.has(job.state)) {
        return [{
          type: "ignored",
          reason: "already_settled",
          jobId,
          keyId: job.keyId,
        }];
      }
      return [{ type: "ignored", reason: "stale_job", jobId, keyId: job.keyId }];
    }
    if (this.#currentJobIds.get(job.keyId) !== jobId) {
      return [{
        type: "ignored",
        reason: "stale_job",
        jobId,
        keyId: job.keyId,
      }];
    }
    job.state = outcome.status;
    if (
      outcome.status === PageJobState.FAILED ||
      outcome.status === PageJobState.ABORTED
    ) {
      this.#currentJobIds.delete(job.keyId);
      this.#terminalFailureKeys.add(job.keyId);
    }
    for (const request of this.#requests.values()) {
      if (request.members.get(job.keyId) !== "pending") continue;
      request.members.set(job.keyId, outcome.status);
    }
    const effects = [];
    this.#reconcile(effects, PageCancelCause.NO_DEMAND);
    this.#collectGroupOutcomes(effects);
    return orderedEffects(effects);
  }

  invalidate(cause) {
    if (!INVALIDATION_CAUSES.has(cause)) {
      return [{ type: "ignored", reason: "invalid_cause", cause }];
    }
    const effects = [];
    for (const [keyId, jobId] of this.#currentJobIds) {
      const job = this.#jobsById.get(jobId);
      if (job?.state === PageJobState.RUNNING) {
        job.state = PageJobState.CANCELLED;
        effects.push({ type: "cancel", jobId, keyId, cause });
      }
    }
    for (const request of this.#requests.values()) {
      if (request.outcome !== null) continue;
      request.outcome = "failed";
      effects.push({ type: "group_failed", requestId: request.requestId, reason: cause });
    }
    this.#requests.clear();
    this.#displayDemands.clear();
    this.#planKeys = [];
    this.#planKeySet.clear();
    this.#currentJobIds.clear();
    this.#terminalFailureKeys.clear();
    this.#pruneJobHistory();
    return orderedEffects(effects);
  }

  protectedKeyIds() {
    const protectedIds = [];
    const seen = new Set();
    for (const request of this.#requests.values()) {
      for (const keyId of request.keys) {
        if (seen.has(keyId)) continue;
        seen.add(keyId);
        protectedIds.push(keyId);
      }
    }
    for (const keyId of this.#planKeys) {
      if (seen.has(keyId)) continue;
      seen.add(keyId);
      protectedIds.push(keyId);
    }
    return protectedIds;
  }

  jobFor(keyId) {
    const jobId = this.#currentJobIds.get(keyId);
    const job = jobId ? this.#jobsById.get(jobId) : null;
    return job
      ? { jobId: job.jobId, priority: job.priority, state: job.state }
      : null;
  }

  openRequestIds() {
    return [...this.#requests.keys()];
  }

  #hasDemand(keyId) {
    return Boolean(this.#displayDemands.get(keyId)?.size)
      || this.#planKeySet.has(keyId);
  }

  #availableMemberState(keyId) {
    if (this.#hasBytes(keyId)) return PageJobState.READY;
    const job = this.#currentJob(keyId);
    if (job && TERMINAL_JOB_STATES.has(job.state)) return job.state;
    return "pending";
  }

  #currentJob(keyId) {
    const jobId = this.#currentJobIds.get(keyId);
    return jobId ? this.#jobsById.get(jobId) ?? null : null;
  }

  #foregroundKeys() {
    const keys = [];
    const seen = new Set();
    for (const request of this.#requests.values()) {
      for (const keyId of request.keys) {
        if (seen.has(keyId)) continue;
        seen.add(keyId);
        keys.push(keyId);
      }
    }
    return keys;
  }

  #pendingDisplayRequestId(keyId) {
    for (const requestId of this.#displayDemands.get(keyId) ?? []) {
      const request = this.#requests.get(requestId);
      if (
        request?.outcome === null &&
        request.members.get(keyId) === "pending"
      ) {
        return requestId;
      }
    }
    return undefined;
  }

  #reconcile(effects, cancelCause) {
    for (const keyId of this.#terminalFailureKeys) {
      if (!this.#hasDemand(keyId)) this.#terminalFailureKeys.delete(keyId);
    }
    for (const [keyId, jobId] of [...this.#currentJobIds]) {
      const job = this.#jobsById.get(jobId);
      if (this.#hasDemand(keyId)) continue;
      this.#currentJobIds.delete(keyId);
      if (job?.state !== PageJobState.RUNNING) continue;
      job.state = PageJobState.CANCELLED;
      effects.push({ type: "cancel", jobId, keyId, cause: cancelCause });
    }
    for (const keyId of this.#foregroundKeys()) {
      const job = this.#currentJob(keyId);
      if (
        job?.state === PageJobState.RUNNING &&
        job.priority === PageJobPriority.PREFETCH
      ) {
        job.priority = PageJobPriority.FOREGROUND;
        effects.push({
          type: "promote",
          jobId: job.jobId,
          keyId,
          requestId: this.#pendingDisplayRequestId(keyId),
        });
      }
    }
    for (const keyId of this.#foregroundKeys()) {
      if (this.#terminalFailureKeys.has(keyId)) continue;
      if (this.#currentJob(keyId) || this.#hasBytes(keyId)) continue;
      const requestId = this.#pendingDisplayRequestId(keyId);
      this.#startJob(keyId, PageJobPriority.FOREGROUND, effects, requestId);
    }
    let activePrefetches = [...this.#currentJobIds.values()]
      .map((jobId) => this.#jobsById.get(jobId))
      .filter((job) =>
        job?.state === PageJobState.RUNNING &&
        job.priority === PageJobPriority.PREFETCH
      )
      .length;
    for (const keyId of this.#planKeys) {
      if (activePrefetches >= this.#prefetchConcurrency) break;
      if (this.#terminalFailureKeys.has(keyId)) continue;
      if (this.#displayDemands.get(keyId)?.size) continue;
      if (this.#currentJob(keyId) || this.#hasBytes(keyId)) continue;
      if (!this.#prefetchAdmits(keyId)) break;
      this.#startJob(keyId, PageJobPriority.PREFETCH, effects);
      activePrefetches += 1;
    }
    this.#pruneJobHistory();
  }

  #startJob(keyId, priority, effects, requestId) {
    this.#jobSequence += 1;
    const jobId = String(this.#jobSequence);
    const job = {
      jobId,
      keyId,
      priority,
      state: PageJobState.RUNNING,
    };
    this.#jobsById.set(jobId, job);
    this.#currentJobIds.set(keyId, jobId);
    effects.push({
      type: "start",
      jobId,
      keyId,
      priority,
      ...(requestId ? { requestId } : {}),
    });
  }

  #pruneJobHistory() {
    let settledCount = 0;
    for (const job of this.#jobsById.values()) {
      if (job.state !== PageJobState.RUNNING) settledCount += 1;
    }
    if (settledCount <= SETTLED_JOB_HISTORY_LIMIT) return;

    // Running jobs are never evicted, and neither is a settled job that is still
    // the current one for its key: this bound is a memory limit and must not
    // rewrite live routing. Those are bounded by the demanded key count and
    // become prunable as soon as `#reconcile` drops the demand, so the history
    // may sit slightly above the limit until then. Once an old terminal identity
    // does age out, a late settle degrades from the more precise
    // stale/already-settled reason to the still-typed unknown_job reason.
    for (const [jobId, job] of this.#jobsById) {
      if (job.state === PageJobState.RUNNING) continue;
      if (this.#currentJobIds.get(job.keyId) === jobId) continue;
      this.#jobsById.delete(jobId);
      settledCount -= 1;
      if (settledCount <= SETTLED_JOB_HISTORY_LIMIT) break;
    }
  }

  #collectGroupOutcomes(effects) {
    for (const request of this.#requests.values()) {
      for (const keyId of request.keys) {
        if (request.members.get(keyId) !== "pending") continue;
        request.members.set(keyId, this.#availableMemberState(keyId));
      }
      if (request.outcome !== null) continue;
      const failedKey = request.keys.find((keyId) =>
        request.members.get(keyId) === "failed" ||
        request.members.get(keyId) === "aborted"
      );
      if (failedKey) {
        request.outcome = "failed";
        effects.push({
          type: "group_failed",
          requestId: request.requestId,
          keyId: failedKey,
          reason: request.members.get(failedKey) === "aborted"
            ? "member_aborted"
            : "member_failed",
        });
        continue;
      }
      if (request.keys.every((keyId) => request.members.get(keyId) === "ready")) {
        request.outcome = "ready";
        effects.push({ type: "group_ready", requestId: request.requestId });
      }
    }
  }
}
