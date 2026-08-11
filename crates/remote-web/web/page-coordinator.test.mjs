import test from "node:test";
import assert from "node:assert/strict";

import {
  PageCancelCause,
  PageCoordinatorEffectType,
  PageDisplayCoordinator,
  PageGroupFailureReason,
  PageIgnoredReason,
  PageJobPriority,
  PageJobState,
  pageResourceKey,
} from "./page-coordinator.mjs";

const SETTLED_JOB_HISTORY_LIMIT = 256;

const BASE_RESOURCE = Object.freeze({
  address: "folder/book.zip::page-1",
  targetPx: 2048,
  renderRevision: 7,
  generation: "generation-a",
  sessionId: "session-a",
  sessionCacheEpoch: "epoch-a",
  renderContext: {
    context_address: "folder/book.zip",
    display_slot: "spread_left",
    spread_partner: "folder/book.zip::page-2",
  },
  adjustmentPreview: null,
});

function resource(overrides = {}) {
  return {
    ...BASE_RESOURCE,
    renderContext: { ...BASE_RESOURCE.renderContext },
    ...overrides,
  };
}

function effectsOf(effects, type) {
  return effects.filter((effect) => effect.type === type);
}

function startFor(effects, keyId) {
  return effects.find((effect) =>
    effect.type === PageCoordinatorEffectType.START && effect.keyId === keyId
  );
}

test("pageResourceKey changes for every byte-affecting field", () => {
  const baseline = pageResourceKey(resource()).id;
  const variants = [
    resource({ address: "folder/book.zip::page-9" }),
    resource({ targetPx: 4096 }),
    resource({ renderRevision: "preview-8" }),
    resource({ generation: "generation-b" }),
    resource({ sessionId: "session-b" }),
    resource({ sessionCacheEpoch: "epoch-b" }),
    resource({ renderContext: { ...BASE_RESOURCE.renderContext,
      context_address: "folder/other.zip" } }),
    resource({ renderContext: { ...BASE_RESOURCE.renderContext,
      display_slot: "spread_right" } }),
    resource({ renderContext: { ...BASE_RESOURCE.renderContext,
      spread_partner: "folder/book.zip::page-3" } }),
    resource({ adjustmentPreview: { brightness: 0.2 } }),
  ];
  for (const variant of variants) {
    assert.notEqual(pageResourceKey(variant).id, baseline);
  }
});

test("pageResourceKey recursively normalizes object key order", () => {
  const left = pageResourceKey(resource({
    renderContext: {
      spread_partner: "partner",
      nested: { z: 1, a: { y: 2, b: 3 } },
      display_slot: "spread_left",
      context_address: "context",
    },
    adjustmentPreview: { z: 1, a: { d: 4, c: 3 } },
  }));
  const right = pageResourceKey(resource({
    renderContext: {
      context_address: "context",
      display_slot: "spread_left",
      nested: { a: { b: 3, y: 2 }, z: 1 },
      spread_partner: "partner",
    },
    adjustmentPreview: { a: { c: 3, d: 4 }, z: 1 },
  }));
  assert.equal(left.id, right.id);
});

test("pageResourceKey keeps JSON field boundaries unambiguous", () => {
  const embeddedSeparators = pageResourceKey(resource({
    address: `a\nb"c`,
    targetPx: 1,
  }));
  const neighboringField = pageResourceKey(resource({
    address: "a",
    targetPx: `b"c\n1`,
  }));
  assert.notEqual(embeddedSeparators.id, neighboringField.id);
  assert.doesNotThrow(() => JSON.parse(embeddedSeparators.id));
});

test("adjustment previews are distinct non-cacheable resources", () => {
  const normal = pageResourceKey(resource());
  const first = pageResourceKey(resource({ adjustmentPreview: { contrast: 0.1 } }));
  const second = pageResourceKey(resource({ adjustmentPreview: { contrast: 0.2 } }));
  assert.equal(normal.cacheable, true);
  assert.equal(first.cacheable, false);
  assert.equal(second.cacheable, false);
  assert.notEqual(first.id, second.id);
});

test("a display group becomes ready exactly once after every member settles", () => {
  const coordinator = new PageDisplayCoordinator();
  const opened = coordinator.openDisplay({
    requestId: "display",
    groupKey: "group-1",
    keys: ["left", "right"],
  });
  const left = startFor(opened, "left");
  const right = startFor(opened, "right");
  assert.ok(left);
  assert.ok(right);
  assert.deepEqual(coordinator.settle(left.jobId, { status: "ready" }), []);
  assert.deepEqual(coordinator.settle(right.jobId, { status: "ready" }), [
    { type: "group_ready", requestId: "display" },
  ]);
  assert.deepEqual(coordinator.settle(right.jobId, { status: "ready" }), [{
    type: "ignored",
    reason: "already_settled",
    jobId: right.jobId,
    keyId: "right",
  }]);
});

test("a fully cached display group is ready without starting work", () => {
  const ready = new Set(["left", "right"]);
  const coordinator = new PageDisplayCoordinator({
    hasBytes: (keyId) => ready.has(keyId),
  });
  assert.deepEqual(coordinator.openDisplay({
    requestId: "cached",
    groupKey: "group-cached",
    keys: ["left", "right"],
  }), [{ type: "group_ready", requestId: "cached" }]);
  assert.equal(coordinator.jobFor("left"), null);
  assert.equal(coordinator.jobFor("right"), null);
});

test("one failed spread member fails the group without discarding its sibling", () => {
  const coordinator = new PageDisplayCoordinator();
  const opened = coordinator.openDisplay({
    requestId: "spread",
    groupKey: "spread-1",
    keys: ["left", "right"],
  });
  const left = startFor(opened, "left");
  const right = startFor(opened, "right");
  assert.deepEqual(coordinator.settle(left.jobId, {
    status: "failed",
    reason: "server_error",
  }), [{
    type: "group_failed",
    requestId: "spread",
    keyId: "left",
    reason: "member_failed",
  }]);
  assert.deepEqual(coordinator.settle(right.jobId, { status: "ready" }), []);
  assert.deepEqual(coordinator.jobFor("right"), {
    jobId: right.jobId,
    priority: "foreground",
    state: "ready",
  });
});

test("an aborted member has a typed group failure reason", () => {
  const coordinator = new PageDisplayCoordinator();
  const opened = coordinator.openDisplay({
    requestId: "aborted",
    groupKey: "group-aborted",
    keys: ["page"],
  });
  const job = startFor(opened, "page");
  assert.deepEqual(coordinator.settle(job.jobId, { status: "aborted" }), [{
    type: "group_failed",
    requestId: "aborted",
    keyId: "page",
    reason: PageGroupFailureReason.MEMBER_ABORTED,
  }]);
});

test("a new display request retries a failed key while plan demand remains", () => {
  const coordinator = new PageDisplayCoordinator();
  const firstJob = startFor(coordinator.setPlan(["a"]), "a");
  coordinator.openDisplay({ requestId: "r1", groupKey: "g1", keys: ["a"] });
  assert.deepEqual(coordinator.settle(firstJob.jobId, { status: "failed" }), [{
    type: "group_failed",
    requestId: "r1",
    keyId: "a",
    reason: "member_failed",
  }]);
  assert.equal(
    coordinator.settle(firstJob.jobId, { status: "failed" })[0].reason,
    PageIgnoredReason.ALREADY_SETTLED
  );
  assert.equal(coordinator.jobFor("a"), null);
  assert.deepEqual(coordinator.releaseDisplay("r1", "no_demand"), []);
  assert.deepEqual(coordinator.setPlan(["a"]), [], "plan alone must not retry");

  const retry = coordinator.openDisplay({
    requestId: "r2",
    groupKey: "g2",
    keys: ["a"],
  });
  const retryJob = startFor(retry, "a");
  assert.ok(retryJob);
  assert.equal(retryJob.priority, PageJobPriority.FOREGROUND);
  assert.notEqual(retryJob.jobId, firstJob.jobId);
  assert.equal(effectsOf(retry, "group_failed").length, 0);
  assert.deepEqual(coordinator.settle(retryJob.jobId, { status: "ready" }), [{
    type: "group_ready",
    requestId: "r2",
  }]);
});

test("only requests waiting on the failed attempt receive its terminal result", () => {
  const coordinator = new PageDisplayCoordinator();
  const opened = coordinator.openDisplay({
    requestId: "r1",
    groupKey: "g1",
    keys: ["a"],
  });
  const job = startFor(opened, "a");
  coordinator.openDisplay({ requestId: "r2", groupKey: "g2", keys: ["a"] });
  assert.deepEqual(coordinator.settle(job.jobId, { status: "failed" }), [
    { type: "group_failed", requestId: "r1", keyId: "a", reason: "member_failed" },
    { type: "group_failed", requestId: "r2", keyId: "a", reason: "member_failed" },
  ]);
});

test("a retry starts for the new request while the failed request remains open", () => {
  const coordinator = new PageDisplayCoordinator();
  const firstJob = startFor(coordinator.setPlan(["a"]), "a");
  coordinator.openDisplay({ requestId: "r1", groupKey: "g1", keys: ["a"] });
  coordinator.settle(firstJob.jobId, { status: "failed" });

  const retry = coordinator.openDisplay({
    requestId: "r2",
    groupKey: "g2",
    keys: ["a"],
  });
  const retryJob = startFor(retry, "a");
  assert.equal(retryJob.requestId, "r2");
  assert.equal(effectsOf(retry, "group_failed").length, 0);
  assert.deepEqual(coordinator.settle(retryJob.jobId, { status: "ready" }), [{
    type: "group_ready",
    requestId: "r2",
  }]);
  assert.deepEqual(coordinator.openRequestIds(), ["r1", "r2"]);
});

test("plan failure memory clears after no demand and invalidate", () => {
  const leftWindow = new PageDisplayCoordinator();
  const first = startFor(leftWindow.setPlan(["a"]), "a");
  assert.deepEqual(leftWindow.settle(first.jobId, { status: "failed" }), []);
  assert.deepEqual(leftWindow.setPlan(["a"]), []);
  assert.deepEqual(leftWindow.setPlan([]), []);
  assert.ok(startFor(leftWindow.setPlan(["a"]), "a"));

  const invalidated = new PageDisplayCoordinator();
  const failed = startFor(invalidated.setPlan(["a"]), "a");
  invalidated.settle(failed.jobId, { status: "failed" });
  assert.deepEqual(invalidated.invalidate(PageCancelCause.CONTEXT_RESET), []);
  assert.ok(startFor(invalidated.setPlan(["a"]), "a"));
});

test("settled job history is bounded without evicting a running job", () => {
  const coordinator = new PageDisplayCoordinator();
  const live = startFor(coordinator.openDisplay({
    requestId: "live",
    groupKey: "live-group",
    keys: ["live"],
  }), "live");
  let oldestSettledJobId = null;
  let newestSettledJobId = null;
  for (let index = 0; index < SETTLED_JOB_HISTORY_LIMIT + 4; index += 1) {
    const keyId = `history-${index}`;
    const job = startFor(coordinator.setPlan([keyId]), keyId);
    oldestSettledJobId ??= job.jobId;
    newestSettledJobId = job.jobId;
    coordinator.setPlan([]);
  }
  assert.deepEqual(coordinator.jobFor("live"), {
    jobId: live.jobId,
    priority: "foreground",
    state: "running",
  });
  assert.equal(
    coordinator.settle(oldestSettledJobId, { status: "aborted" })[0].reason,
    PageIgnoredReason.UNKNOWN_JOB
  );
  assert.equal(
    coordinator.settle(newestSettledJobId, { status: "aborted" })[0].reason,
    PageIgnoredReason.STALE_JOB
  );
});

test("the history bound never rewrites the current job of a demanded key", () => {
  const coordinator = new PageDisplayCoordinator();
  const kept = startFor(coordinator.openDisplay({
    requestId: "kept",
    groupKey: "kept-group",
    keys: ["kept"],
  }), "kept");
  assert.deepEqual(coordinator.settle(kept.jobId, { status: "ready" }), [
    { type: "group_ready", requestId: "kept" },
  ]);
  for (let index = 0; index < SETTLED_JOB_HISTORY_LIMIT + 4; index += 1) {
    const keyId = `history-${index}`;
    coordinator.setPlan([keyId]);
    coordinator.setPlan([]);
  }
  // The bound is a memory limit. A settled job that is still the current one for
  // a demanded key is live routing state: dropping it would restart a page the
  // reader already has.
  assert.deepEqual(coordinator.jobFor("kept"), {
    jobId: kept.jobId,
    priority: "foreground",
    state: "ready",
  });
  assert.deepEqual(coordinator.openDisplay({
    requestId: "again",
    groupKey: "again-group",
    keys: ["kept"],
  }), [{ type: "group_ready", requestId: "again" }]);
});

test("a prefetch job promotes once and never demotes", () => {
  const coordinator = new PageDisplayCoordinator();
  const job = startFor(coordinator.setPlan(["page"]), "page");
  assert.equal(job.priority, PageJobPriority.PREFETCH);
  assert.deepEqual(coordinator.openDisplay({
    requestId: "first",
    groupKey: "group-first",
    keys: ["page"],
  }), [{ type: "promote", jobId: job.jobId, keyId: "page" }]);
  assert.deepEqual(coordinator.openDisplay({
    requestId: "second",
    groupKey: "group-second",
    keys: ["page"],
  }), []);
  assert.deepEqual(coordinator.releaseDisplay("first", "no_demand"), []);
  assert.deepEqual(coordinator.releaseDisplay("second", "no_demand"), []);
  assert.deepEqual(coordinator.jobFor("page"), {
    jobId: job.jobId,
    priority: "foreground",
    state: "running",
  });
  assert.deepEqual(coordinator.setPlan([]), [{
    type: "cancel",
    jobId: job.jobId,
    keyId: "page",
    cause: "no_demand",
  }]);
});

test("foreground jobs start at foreground priority and never promote", () => {
  const coordinator = new PageDisplayCoordinator();
  const opened = coordinator.openDisplay({
    requestId: "display",
    groupKey: "group",
    keys: ["page"],
  });
  assert.equal(startFor(opened, "page").priority, PageJobPriority.FOREGROUND);
  assert.equal(effectsOf(opened, PageCoordinatorEffectType.PROMOTE).length, 0);
  assert.deepEqual(coordinator.openDisplay({
    requestId: "joined",
    groupKey: "joined-group",
    keys: ["page"],
  }), []);
});

test("work is cancelled only when its last display and plan demand disappear", () => {
  const coordinator = new PageDisplayCoordinator();
  const job = startFor(coordinator.setPlan(["page"]), "page");
  coordinator.openDisplay({
    requestId: "display",
    groupKey: "group",
    keys: ["page"],
  });
  assert.deepEqual(coordinator.setPlan([]), []);
  assert.deepEqual(coordinator.releaseDisplay("display", "no_demand"), [{
    type: "cancel",
    jobId: job.jobId,
    keyId: "page",
    cause: "no_demand",
  }]);
  assert.equal(coordinator.jobFor("page"), null);
});

test("setPlan cancels active work but not a plan member that never started", () => {
  const coordinator = new PageDisplayCoordinator({ prefetchConcurrency: 1 });
  const planned = coordinator.setPlan(["near", "far"]);
  const near = startFor(planned, "near");
  assert.equal(startFor(planned, "far"), undefined);
  assert.deepEqual(coordinator.setPlan([]), [{
    type: "cancel",
    jobId: near.jobId,
    keyId: "near",
    cause: "no_demand",
  }]);
});

test("prefetch admission gates only prefetch and resumes on a later mutation", () => {
  let admits = false;
  const coordinator = new PageDisplayCoordinator({
    prefetchAdmits: () => admits,
  });
  assert.deepEqual(coordinator.setPlan(["planned"]), []);
  const foreground = coordinator.openDisplay({
    requestId: "display",
    groupKey: "group",
    keys: ["foreground"],
  });
  assert.equal(startFor(foreground, "foreground").priority, "foreground");
  assert.equal(startFor(foreground, "planned"), undefined);
  admits = true;
  const resumed = coordinator.setPlan(["planned"]);
  assert.equal(startFor(resumed, "planned").priority, "prefetch");
});

test("prefetch concurrency is bounded and the next plan member starts on settle", () => {
  const coordinator = new PageDisplayCoordinator({ prefetchConcurrency: 2 });
  const opened = coordinator.setPlan(["a", "b", "c"]);
  assert.deepEqual(
    effectsOf(opened, "start").map(({ keyId }) => keyId),
    ["a", "b"]
  );
  const settled = coordinator.settle(
    coordinator.jobFor("a").jobId,
    { status: "ready" }
  );
  assert.deepEqual(effectsOf(settled, "start").map(({ keyId }) => keyId), ["c"]);
  const running = ["a", "b", "c"]
    .map((keyId) => coordinator.jobFor(keyId))
    .filter((job) => job?.state === "running" && job.priority === "prefetch");
  assert.equal(running.length, 2);
});

test("late completion after release is ignored", () => {
  const coordinator = new PageDisplayCoordinator();
  const opened = coordinator.openDisplay({
    requestId: "released",
    groupKey: "group",
    keys: ["page"],
  });
  const oldJob = startFor(opened, "page");
  assert.equal(effectsOf(
    coordinator.releaseDisplay("released", "no_demand"),
    "cancel"
  ).length, 1);
  assert.deepEqual(coordinator.settle(oldJob.jobId, { status: "ready" }), [{
    type: "ignored",
    reason: "stale_job",
    jobId: oldJob.jobId,
    keyId: "page",
  }]);
  assert.deepEqual(coordinator.openRequestIds(), []);
});

test("an old job cannot affect a newer job for the same key", () => {
  const coordinator = new PageDisplayCoordinator();
  const first = coordinator.openDisplay({
    requestId: "first",
    groupKey: "group-first",
    keys: ["page"],
  });
  const oldJob = startFor(first, "page");
  coordinator.releaseDisplay("first", "no_demand");
  const second = coordinator.openDisplay({
    requestId: "second",
    groupKey: "group-second",
    keys: ["page"],
  });
  const newJob = startFor(second, "page");
  assert.notEqual(oldJob.jobId, newJob.jobId);
  assert.equal(coordinator.settle(oldJob.jobId, { status: "failed" })[0].reason, "stale_job");
  assert.deepEqual(coordinator.jobFor("page"), {
    jobId: newJob.jobId,
    priority: "foreground",
    state: "running",
  });
});

test("invalidate cancels every running job and fails each pending group once", () => {
  const coordinator = new PageDisplayCoordinator();
  const display = coordinator.openDisplay({
    requestId: "display",
    groupKey: "spread",
    keys: ["left", "right"],
  });
  const planned = coordinator.setPlan(["near", "far"]);
  const jobIds = [...effectsOf(display, "start"), ...effectsOf(planned, "start")]
    .map(({ jobId }) => jobId);
  const invalidated = coordinator.invalidate(PageCancelCause.SESSION_INVALIDATED);
  assert.deepEqual(
    effectsOf(invalidated, "cancel").map(({ cause }) => cause),
    jobIds.map(() => "session_invalidated")
  );
  assert.deepEqual(effectsOf(invalidated, "group_failed"), [{
    type: "group_failed",
    requestId: "display",
    reason: "session_invalidated",
  }]);
  assert.deepEqual(coordinator.openRequestIds(), []);
  assert.deepEqual(coordinator.protectedKeyIds(), []);
  for (const jobId of jobIds) {
    assert.equal(coordinator.settle(jobId, { status: "aborted" })[0].reason, "stale_job");
  }
});

test("release is idempotent, IDs are unique, and duplicate keys count once", () => {
  const coordinator = new PageDisplayCoordinator();
  assert.equal(coordinator.nextDisplayRequestId(), "1");
  assert.equal(coordinator.nextDisplayRequestId(), "2");
  const opened = coordinator.openDisplay({
    requestId: "same",
    groupKey: "group",
    keys: ["page", "page"],
  });
  assert.equal(effectsOf(opened, "start").length, 1);
  assert.deepEqual(coordinator.protectedKeyIds(), ["page"]);
  assert.equal(coordinator.openDisplay({
    requestId: "same",
    groupKey: "other",
    keys: ["other"],
  })[0].reason, PageIgnoredReason.DUPLICATE_REQUEST_ID);
  assert.equal(effectsOf(
    coordinator.releaseDisplay("same", "no_demand"),
    "cancel"
  ).length, 1);
  assert.deepEqual(coordinator.releaseDisplay("same", "no_demand"), [{
    type: "ignored",
    reason: "unknown_request",
    requestId: "same",
  }]);
});

test("protected keys put every display demand before the plan order", () => {
  const coordinator = new PageDisplayCoordinator({ prefetchConcurrency: 0 });
  coordinator.setPlan(["plan-near", "shared", "plan-far"]);
  coordinator.openDisplay({
    requestId: "first",
    groupKey: "first-group",
    keys: ["visible-a", "shared"],
  });
  coordinator.openDisplay({
    requestId: "second",
    groupKey: "second-group",
    keys: ["visible-b"],
  });
  assert.deepEqual(coordinator.protectedKeyIds(), [
    "visible-a",
    "shared",
    "visible-b",
    "plan-near",
    "plan-far",
  ]);
});

const EFFECT_RANK = Object.freeze({
  ignored: 0,
  cancel: 1,
  promote: 2,
  start: 3,
  group_ready: 4,
  group_failed: 4,
});

const REQUEST_KEYS = Object.freeze({
  A: ["a"],
  B: ["a", "b"],
});

const EXHAUSTIVE_OPERATIONS = Object.freeze([
  {
    name: "open-A",
    run: (coordinator) => coordinator.openDisplay({
      requestId: "A",
      groupKey: "group-A",
      keys: REQUEST_KEYS.A,
    }),
    update(model, coordinator) {
      if (!model.seen.has("A")) {
        model.seen.add("A");
        model.open.set("A", requestAttemptModel(coordinator, REQUEST_KEYS.A));
      }
    },
  },
  {
    name: "open-B",
    run: (coordinator) => coordinator.openDisplay({
      requestId: "B",
      groupKey: "group-B",
      keys: REQUEST_KEYS.B,
    }),
    update(model, coordinator) {
      if (!model.seen.has("B")) {
        model.seen.add("B");
        model.open.set("B", requestAttemptModel(coordinator, REQUEST_KEYS.B));
      }
    },
  },
  {
    name: "plan-a-b-c",
    run: (coordinator) => coordinator.setPlan(["a", "b", "c"]),
    update(model) { model.plan = ["a", "b", "c"]; },
  },
  {
    name: "plan-b",
    run: (coordinator) => coordinator.setPlan(["b"]),
    update(model) { model.plan = ["b"]; },
  },
  {
    name: "settle-a-ready",
    run(coordinator) {
      return coordinator.settle(
        coordinator.jobFor("a")?.jobId ?? "missing-a",
        { status: "ready" }
      );
    },
    update() {},
  },
  {
    name: "settle-a-failed",
    run(coordinator) {
      return coordinator.settle(
        coordinator.jobFor("a")?.jobId ?? "missing-a",
        { status: "failed", reason: "enumerated_failure" }
      );
    },
    update() {},
  },
  {
    name: "release-A",
    run: (coordinator) => coordinator.releaseDisplay("A", "no_demand"),
    update(model) {
      if (model.open.delete("A")) model.released.add("A");
    },
  },
  {
    name: "release-B",
    run: (coordinator) => coordinator.releaseDisplay("B", "no_demand"),
    update(model) {
      if (model.open.delete("B")) model.released.add("B");
    },
  },
  {
    name: "invalidate",
    run: (coordinator) => coordinator.invalidate("context_reset"),
    update(model) {
      model.open.clear();
      model.plan = [];
    },
  },
]);

function sequenceIndexes(length, prefix = []) {
  if (prefix.length === length) return [prefix];
  const sequences = [];
  for (let index = 0; index < EXHAUSTIVE_OPERATIONS.length; index += 1) {
    sequences.push(...sequenceIndexes(length, [...prefix, index]));
  }
  return sequences;
}

function requestAttemptModel(coordinator, keys) {
  return {
    keys,
    members: new Map(keys.map((keyId) => {
      const job = coordinator.jobFor(keyId);
      return [keyId, {
        status: job?.state === PageJobState.READY
          ? PageJobState.READY
          : "pending",
        jobId: job?.state === PageJobState.RUNNING ? job.jobId : null,
      }];
    })),
  };
}

function demandKeys(model) {
  const demanded = new Set(model.plan);
  for (const request of model.open.values()) {
    for (const keyId of request.keys) demanded.add(keyId);
  }
  return demanded;
}

function assertCoordinatorInvariants({
  coordinator,
  effects,
  operation,
  model,
  terminalCounts,
  priorityByJob,
  settlement,
  scenario,
}) {
  if (settlement) {
    for (const request of model.open.values()) {
      const member = request.members.get(settlement.keyId);
      if (member?.status !== "pending" || member.jobId !== settlement.jobId) continue;
      member.status = settlement.status;
    }
  }

  let previousRank = -1;
  for (const effect of effects) {
    const rank = EFFECT_RANK[effect.type];
    assert.ok(rank >= previousRank, `${scenario}: effect order`);
    previousRank = rank;
    if (effect.type === "cancel") {
      assert.ok(
        operation.name === "invalidate" || !demandKeys(model).has(effect.keyId),
        `${scenario}: cancelled demanded key ${effect.keyId}`
      );
    }
    if (effect.type === "group_ready" || effect.type === "group_failed") {
      assert.equal(
        model.released.has(effect.requestId),
        false,
        `${scenario}: result after release`
      );
      const count = (terminalCounts.get(effect.requestId) ?? 0) + 1;
      terminalCounts.set(effect.requestId, count);
      assert.ok(count <= 1, `${scenario}: duplicate terminal result`);
      const request = model.open.get(effect.requestId);
      if (effect.type === "group_ready") {
        assert.ok(request, `${scenario}: ready result for unopened request`);
        for (const member of request.members.values()) {
          assert.equal(member.status, PageJobState.READY, `${scenario}: premature ready`);
        }
      } else if (effect.keyId) {
        const member = request?.members.get(effect.keyId);
        assert.ok(member, `${scenario}: failed result for unwaited key ${effect.keyId}`);
        assert.ok(
          member.status === PageJobState.FAILED ||
            member.status === PageJobState.ABORTED,
          `${scenario}: inherited terminal failure for ${effect.keyId}`
        );
        assert.ok(member.jobId, `${scenario}: terminal failure without an attempt`);
      }
    }
    if (effect.type === "start") {
      priorityByJob.set(effect.jobId, effect.priority);
      for (const request of model.open.values()) {
        const member = request.members.get(effect.keyId);
        if (member?.status === "pending" && member.jobId === null) {
          member.jobId = effect.jobId;
        }
      }
      if (effect.requestId) {
        const owner = model.open.get(effect.requestId)?.members.get(effect.keyId);
        assert.equal(
          owner?.status,
          "pending",
          `${scenario}: start attributed to a terminal request`
        );
        assert.equal(
          owner?.jobId,
          effect.jobId,
          `${scenario}: start attributed to the wrong attempt`
        );
      }
    }
    if (effect.type === "promote") {
      assert.notEqual(priorityByJob.get(effect.jobId), PageJobPriority.FOREGROUND);
      priorityByJob.set(effect.jobId, PageJobPriority.FOREGROUND);
    }
  }

  const demanded = demandKeys(model);
  let runningPrefetches = 0;
  for (const keyId of ["a", "b", "c"]) {
    const job = coordinator.jobFor(keyId);
    if (!job) continue;
    const previous = priorityByJob.get(job.jobId);
    if (previous === PageJobPriority.FOREGROUND) {
      assert.equal(job.priority, PageJobPriority.FOREGROUND, `${scenario}: priority demoted`);
    }
    priorityByJob.set(job.jobId, job.priority);
    if (job.state === PageJobState.RUNNING) {
      assert.ok(demanded.has(keyId), `${scenario}: running job without demand`);
      if (job.priority === PageJobPriority.PREFETCH) runningPrefetches += 1;
    }
  }
  assert.ok(runningPrefetches <= 2, `${scenario}: prefetch concurrency exceeded`);

  const protectedIds = new Set(coordinator.protectedKeyIds());
  for (const request of model.open.values()) {
    for (const keyId of request.keys) {
      assert.ok(protectedIds.has(keyId), `${scenario}: unprotected display key`);
    }
  }
  assert.deepEqual(
    new Set(coordinator.openRequestIds()),
    new Set(model.open.keys()),
    `${scenario}: open request model drift`
  );

  for (const request of model.open.values()) {
    for (const [keyId, member] of request.members) {
      if (member.status !== "pending") continue;
      const job = coordinator.jobFor(keyId);
      assert.equal(
        job?.state,
        PageJobState.RUNNING,
        `${scenario}: pending ${keyId} has no running job`
      );
      assert.equal(
        job?.jobId,
        member.jobId,
        `${scenario}: pending ${keyId} changed attempt`
      );
    }
  }
}

test("all coordinator operation sequences through length four preserve invariants", () => {
  let sequenceCount = 0;
  for (let length = 1; length <= 4; length += 1) {
    for (const indexes of sequenceIndexes(length)) {
      sequenceCount += 1;
      const coordinator = new PageDisplayCoordinator();
      const model = {
        seen: new Set(),
        open: new Map(),
        released: new Set(),
        plan: [],
      };
      const terminalCounts = new Map();
      const priorityByJob = new Map();
      const names = [];
      for (const index of indexes) {
        const operation = EXHAUSTIVE_OPERATIONS[index];
        names.push(operation.name);
        const currentA = coordinator.jobFor("a");
        const settlement = operation.name.startsWith("settle-a-") &&
          currentA?.state === PageJobState.RUNNING
          ? {
              jobId: currentA.jobId,
              keyId: "a",
              status: operation.name === "settle-a-ready"
                ? PageJobState.READY
                : PageJobState.FAILED,
            }
          : null;
        const effects = operation.run(coordinator);
        operation.update(model, coordinator);
        assertCoordinatorInvariants({
          coordinator,
          effects,
          operation,
          model,
          terminalCounts,
          priorityByJob,
          settlement,
          scenario: names.join(" -> "),
        });
      }
    }
  }
  assert.equal(sequenceCount, 7380);
});
