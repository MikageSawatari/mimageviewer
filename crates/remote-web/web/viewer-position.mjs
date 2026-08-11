import {
  ViewerGroupLoadCompletionAction,
  viewerGroupLoadCompletionPlan,
  viewerPageGroupRequestMatches,
} from "./command-core.mjs";

export const ViewerPositionIgnoredReason = Object.freeze({
  INVALID_SNAPSHOT: "invalid_snapshot",
  EXPECTED_MISMATCH: "expected_mismatch",
  DISPLAYED_UNRESOLVED: "displayed_unresolved",
});

function validSnapshot(snapshot) {
  return Boolean(snapshot && typeof snapshot === "object");
}

/// Pure requested/displayed position owner. Snapshots are opaque identity
/// values; all equality decisions go through viewerPageGroupRequestMatches.
export class ViewerPositionOwner {
  #requested;
  #displayed;

  constructor() {
    this.#requested = null;
    this.#displayed = null;
  }

  current() {
    return {
      requested: this.#requested,
      displayed: this.#displayed,
    };
  }

  open(snapshot) {
    if (!validSnapshot(snapshot)) {
      return {
        ignored: ViewerPositionIgnoredReason.INVALID_SNAPSHOT,
        history: "none",
      };
    }
    this.#requested = snapshot;
    this.#displayed = snapshot;
    return { ok: true, history: "none" };
  }

  request(snapshot) {
    if (!validSnapshot(snapshot)) {
      return {
        moved: false,
        ignored: ViewerPositionIgnoredReason.INVALID_SNAPSHOT,
        history: "none",
      };
    }
    if (viewerPageGroupRequestMatches(this.#requested, snapshot)) {
      return { moved: false, history: "none", to: this.#requested };
    }
    this.#requested = snapshot;
    return { moved: true, history: "push", to: snapshot };
  }

  display(snapshot) {
    if (!validSnapshot(snapshot)) {
      return {
        displayed: false,
        ignored: ViewerPositionIgnoredReason.INVALID_SNAPSHOT,
        history: "none",
      };
    }
    if (viewerPageGroupRequestMatches(this.#displayed, snapshot)) {
      return { displayed: false, history: "none", to: this.#displayed };
    }
    this.#displayed = snapshot;
    return { displayed: true, history: "none", to: snapshot };
  }

  settle(result, { loadRequest = null } = {}) {
    const completion = viewerGroupLoadCompletionPlan(result, {
      loadRequest,
      currentRequest: this.#requested,
      displayedRequest: this.#displayed,
    });
    if (completion.action !== ViewerGroupLoadCompletionAction.ROLLBACK) {
      return { ...completion, history: "none" };
    }
    return {
      ...completion,
      ...this.rewind({ expected: loadRequest }),
    };
  }

  rewind({ expected = null } = {}) {
    if (
      expected !== null &&
      !viewerPageGroupRequestMatches(expected, this.#requested)
    ) {
      return {
        rewound: false,
        ignored: ViewerPositionIgnoredReason.EXPECTED_MISMATCH,
        to: this.#requested,
        history: "none",
      };
    }
    if (!validSnapshot(this.#displayed)) {
      return {
        rewound: false,
        ignored: ViewerPositionIgnoredReason.DISPLAYED_UNRESOLVED,
        to: this.#requested,
        history: "none",
      };
    }
    if (viewerPageGroupRequestMatches(this.#requested, this.#displayed)) {
      return { rewound: false, to: this.#displayed, history: "none" };
    }
    this.#requested = this.#displayed;
    return { rewound: true, to: this.#displayed, history: "replace" };
  }

  reanchor({ requested = null, displayed = null } = {}) {
    this.#requested = validSnapshot(requested) ? requested : null;
    this.#displayed = validSnapshot(displayed) ? displayed : null;
    return {
      requested: this.#requested ? "resolved" : "unresolved",
      displayed: this.#displayed ? "resolved" : "unresolved",
      history: "none",
    };
  }
}
