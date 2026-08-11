import test from "node:test";
import assert from "node:assert/strict";

import {
  ViewerGroupLoadCompletionAction,
  ViewerGroupLoadOutcome,
  viewerPageGroupRequestMatches,
} from "./command-core.mjs";
import { ViewerPositionOwner } from "./viewer-position.mjs";

function bookSnapshots(names, { viewer = {}, contextIdentity = "book" } = {}) {
  const pageGroups = names.map((name) => ({
    anchor: { name },
    entries: [{ name }],
  }));
  return pageGroups.map((group, groupIndex) => ({
    viewer,
    pageGroups,
    group,
    groupIndex,
    groupIdentity: group.anchor.name,
    contextIdentity,
  }));
}

const FAILED = Object.freeze({
  outcome: ViewerGroupLoadOutcome.FAILED,
  message: "ページを表示できませんでした。",
});

test("settle follows superseded, current request, applied, and requested/displayed order", () => {
  const [a, b, c] = bookSnapshots(["a", "b", "c"]);
  const owner = new ViewerPositionOwner();
  owner.open(a);
  owner.request(b);

  assert.equal(owner.settle(
    { outcome: ViewerGroupLoadOutcome.SUPERSEDED },
    { loadRequest: b }
  ).action, ViewerGroupLoadCompletionAction.IGNORE);
  assert.equal(owner.settle(FAILED, { loadRequest: c }).action,
    ViewerGroupLoadCompletionAction.IGNORE);
  assert.equal(owner.settle(
    { outcome: ViewerGroupLoadOutcome.APPLIED },
    { loadRequest: b }
  ).action, ViewerGroupLoadCompletionAction.POST_DISPLAY);

  const rollback = owner.settle(FAILED, { loadRequest: b });
  assert.equal(rollback.action, ViewerGroupLoadCompletionAction.ROLLBACK);
  assert.equal(rollback.history, "replace");
  assert.match(rollback.message, /前のページに戻りました。$/);
  assert.ok(viewerPageGroupRequestMatches(
    owner.current().requested,
    owner.current().displayed
  ));

  assert.equal(owner.settle(FAILED, { loadRequest: a }).action,
    ViewerGroupLoadCompletionAction.REPORT_FAILURE);
});

test("settle reports failure instead of rollback when displayed is unresolved", () => {
  const [a, b] = bookSnapshots(["a", "b"]);
  const owner = new ViewerPositionOwner();
  owner.reanchor({ requested: b, displayed: null });
  const completion = owner.settle(FAILED, { loadRequest: b });
  assert.equal(completion.action, ViewerGroupLoadCompletionAction.REPORT_FAILURE);
  assert.equal(completion.history, "none");
  assert.equal(owner.current().requested, b);
  assert.equal(owner.current().displayed, null);
  assert.notEqual(owner.current().requested, a);
});

test("rewind is expected-scoped and idempotent", () => {
  const [a, b, c] = bookSnapshots(["a", "b", "c"]);
  const owner = new ViewerPositionOwner();
  owner.open(a);
  owner.request(b);
  owner.request(c);

  const stale = owner.rewind({ expected: b });
  assert.equal(stale.rewound, false);
  assert.equal(stale.history, "none");
  assert.equal(owner.current().requested, c);

  const current = owner.rewind({ expected: c });
  assert.deepEqual(current, { rewound: true, to: a, history: "replace" });
  assert.deepEqual(owner.rewind({ expected: a }), {
    rewound: false,
    to: a,
    history: "none",
  });
});

test("only request pushes history and only an effective rewind replaces it", () => {
  const [a, b, c] = bookSnapshots(["a", "b", "c"]);
  const owner = new ViewerPositionOwner();
  assert.equal(owner.open(a).history, "none");
  assert.equal(owner.request(b).history, "push");
  assert.equal(owner.request(b).history, "none");
  assert.equal(owner.display(b).history, "none");
  assert.equal(owner.reanchor({ requested: b, displayed: b }).history, "none");
  assert.equal(owner.request(c).history, "push");
  assert.equal(owner.rewind({ expected: c }).history, "replace");
  assert.equal(owner.rewind({ expected: b }).history, "none");
});

test("reanchor replaces both grouping identities and marks missing sides unresolved", () => {
  const viewer = {};
  const [oldA, oldB] = bookSnapshots(["a", "b"], { viewer });
  const [newA, newB] = bookSnapshots(["a", "b"], { viewer });
  const owner = new ViewerPositionOwner();
  owner.open(oldA);
  owner.request(oldB);

  assert.deepEqual(owner.reanchor({ requested: newB, displayed: newA }), {
    requested: "resolved",
    displayed: "resolved",
    history: "none",
  });
  assert.deepEqual(owner.current(), { requested: newB, displayed: newA });
  assert.deepEqual(owner.reanchor({ requested: newB, displayed: null }), {
    requested: "resolved",
    displayed: "unresolved",
    history: "none",
  });
  assert.deepEqual(owner.current(), { requested: newB, displayed: null });
});

test("request never moves displayed and display never moves requested", () => {
  const [a, b, c] = bookSnapshots(["a", "b", "c"]);
  const owner = new ViewerPositionOwner();
  owner.open(a);
  owner.request(b);
  assert.deepEqual(owner.current(), { requested: b, displayed: a });
  owner.display(c);
  assert.deepEqual(owner.current(), { requested: b, displayed: c });
});

test("display(null) leaves the displayed snapshot unchanged", () => {
  const [a, b] = bookSnapshots(["a", "b"]);
  const owner = new ViewerPositionOwner();
  owner.open(a);
  owner.request(b);

  assert.deepEqual(owner.display(null), {
    displayed: false,
    ignored: "invalid_snapshot",
    history: "none",
  });
  assert.deepEqual(owner.current(), { requested: b, displayed: a });
});

test("a tokenless redraw of requested B still rolls back to displayed A", () => {
  const [a, b] = bookSnapshots(["a", "b"]);
  const owner = new ViewerPositionOwner();
  owner.open(a);
  owner.request(b);

  const redrawLoadRequest = b;
  const completion = owner.settle(FAILED, { loadRequest: redrawLoadRequest });
  assert.equal(completion.action, ViewerGroupLoadCompletionAction.ROLLBACK);
  assert.deepEqual(owner.current(), { requested: a, displayed: a });
});

test("a tokenless bookmark-style jump to C rolls back to displayed A", () => {
  const [a, , c] = bookSnapshots(["a", "b", "c"]);
  const owner = new ViewerPositionOwner();
  owner.open(a);
  owner.request(c);
  const completion = owner.settle(FAILED, { loadRequest: c });
  assert.equal(completion.action, ViewerGroupLoadCompletionAction.ROLLBACK);
  assert.deepEqual(owner.current(), { requested: a, displayed: a });
});

test("a non-position redraw reports failure without moving position", () => {
  const [a] = bookSnapshots(["a"]);
  const owner = new ViewerPositionOwner();
  owner.open(a);
  const completion = owner.settle(FAILED, { loadRequest: a });
  assert.equal(completion.action, ViewerGroupLoadCompletionAction.REPORT_FAILURE);
  assert.deepEqual(owner.current(), { requested: a, displayed: a });
  assert.doesNotMatch(completion.message, /前のページ/);
});

test("late settlement from an old grouping cannot select the new group at its index", () => {
  const viewer = {};
  const [oldA, oldB] = bookSnapshots(["old-a", "old-b"], { viewer });
  const [newA, newB] = bookSnapshots(["new-a", "new-b"], { viewer });
  const owner = new ViewerPositionOwner();
  owner.open(oldA);
  owner.request(oldB);
  owner.reanchor({ requested: newB, displayed: newA });

  const completion = owner.settle(FAILED, { loadRequest: oldB });
  assert.equal(completion.action, ViewerGroupLoadCompletionAction.IGNORE);
  assert.deepEqual(owner.current(), { requested: newB, displayed: newA });
});

const EXHAUSTIVE_SNAPSHOTS = bookSnapshots(["a", "b", "c"]);
const [EXHAUSTIVE_A, EXHAUSTIVE_B, EXHAUSTIVE_C] = EXHAUSTIVE_SNAPSHOTS;
const EXHAUSTIVE_OPERATIONS = Object.freeze([
  { name: "open-a", run: (owner) => owner.open(EXHAUSTIVE_A) },
  { name: "open-b", run: (owner) => owner.open(EXHAUSTIVE_B) },
  { name: "request-a", run: (owner) => owner.request(EXHAUSTIVE_A) },
  { name: "request-c", run: (owner) => owner.request(EXHAUSTIVE_C) },
  { name: "display-a", run: (owner) => owner.display(EXHAUSTIVE_A) },
  { name: "display-b", run: (owner) => owner.display(EXHAUSTIVE_B) },
  { name: "rewind", run: (owner) => owner.rewind() },
  {
    name: "rewind-expected-b",
    run: (owner) => owner.rewind({ expected: EXHAUSTIVE_B }),
  },
  {
    name: "reanchor-b-a",
    run: (owner) => owner.reanchor({
      requested: EXHAUSTIVE_B,
      displayed: EXHAUSTIVE_A,
    }),
  },
  {
    name: "reanchor-c-unresolved",
    run: (owner) => owner.reanchor({
      requested: EXHAUSTIVE_C,
      displayed: null,
    }),
  },
  {
    name: "settle-current-failed",
    run(owner) {
      return owner.settle(FAILED, {
        loadRequest: owner.current().requested,
      });
    },
  },
  {
    name: "settle-a-applied",
    run: (owner) => owner.settle(
      { outcome: ViewerGroupLoadOutcome.APPLIED },
      { loadRequest: EXHAUSTIVE_A }
    ),
  },
  {
    name: "settle-superseded",
    run: (owner) => owner.settle(
      { outcome: ViewerGroupLoadOutcome.SUPERSEDED },
      { loadRequest: EXHAUSTIVE_C }
    ),
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

function assertPositionInvariants(owner, result, scenario) {
  const { requested, displayed } = owner.current();
  assert.ok(
    requested === null || EXHAUSTIVE_SNAPSHOTS.includes(requested),
    `${scenario}: requested must be a known snapshot or null`
  );
  assert.ok(
    displayed === null || EXHAUSTIVE_SNAPSHOTS.includes(displayed),
    `${scenario}: displayed must be a known snapshot or null`
  );
  if (result?.action === ViewerGroupLoadCompletionAction.ROLLBACK) {
    assert.ok(
      viewerPageGroupRequestMatches(requested, displayed),
      `${scenario}: rollback must immediately converge requested and displayed`
    );
  }
}

test("all position operation sequences through length four preserve invariants", () => {
  let sequenceCount = 0;
  for (let length = 1; length <= 4; length += 1) {
    for (const indexes of sequenceIndexes(length)) {
      sequenceCount += 1;
      const owner = new ViewerPositionOwner();
      const names = [];
      for (const index of indexes) {
        const operation = EXHAUSTIVE_OPERATIONS[index];
        names.push(operation.name);
        const result = operation.run(owner);
        assertPositionInvariants(owner, result, names.join(" -> "));
      }
    }
  }
  assert.equal(sequenceCount, 30940);
});
