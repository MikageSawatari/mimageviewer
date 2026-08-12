#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteLongJobDrainCause {
    DiscardedByHost,
    LoggedOut,
    BackgroundExpired,
    Superseded,
}

/// Session-facing lifecycle shared by long-running remote work.
///
/// Each feature keeps its own registry, snapshots, input protocol, and terminal states. The
/// session only needs to know whether any job extends liveness and how to request a drain.
pub(crate) trait RemoteLongJobRegistry: Send + Sync {
    fn has_nonterminal_jobs(&self) -> bool;
    fn on_session_drain(&self, cause: RemoteLongJobDrainCause);
}
