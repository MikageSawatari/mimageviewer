use super::{DetachedHostClaim, DetachedSessionLease, ViewerPresentation};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DetachedHostLease {
    pub(crate) session: DetachedSessionLease,
    pub(crate) host: Option<DetachedHostClaim>,
}

impl DetachedHostLease {
    fn without_host(self) -> Self {
        Self {
            session: self.session,
            host: None,
        }
    }

    fn hwnd(self) -> u64 {
        self.host.map_or(0, |claim| claim.hwnd)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetachedTargetLease {
    None,
    Candidate(DetachedHostLease),
    KeepLive(DetachedHostLease),
    Transferred(DetachedHostLease),
}

impl DetachedTargetLease {
    fn lease(self) -> Option<DetachedHostLease> {
        match self {
            Self::None => None,
            Self::Candidate(lease) | Self::KeepLive(lease) | Self::Transferred(lease) => {
                Some(lease)
            }
        }
    }

    fn session(self) -> Option<DetachedSessionLease> {
        self.lease().map(|lease| lease.session)
    }

    fn host(self) -> Option<DetachedHostClaim> {
        self.lease().and_then(|lease| lease.host)
    }

    fn with_host(self, host: Option<DetachedHostClaim>) -> Self {
        match self {
            Self::None => Self::None,
            Self::Candidate(lease) => Self::Candidate(DetachedHostLease {
                session: lease.session,
                host,
            }),
            Self::KeepLive(lease) => Self::KeepLive(DetachedHostLease {
                session: lease.session,
                host,
            }),
            Self::Transferred(lease) => Self::Transferred(DetachedHostLease {
                session: lease.session,
                host,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PresentationRequest {
    pub(crate) id: u64,
    pub(crate) current: ViewerPresentation,
    pub(crate) target: ViewerPresentation,
    pub(crate) activate: bool,
    pub(crate) announce_main_hint: bool,
    pub(crate) outgoing_presenter_hwnd: u64,
    pub(crate) outgoing_detached: Option<DetachedHostLease>,
    pub(crate) target_detached: DetachedTargetLease,
}

impl PresentationRequest {
    fn target_session(self) -> Option<DetachedSessionLease> {
        self.target_detached.session()
    }

    fn target_host(self) -> Option<DetachedHostClaim> {
        self.target_detached.host()
    }

    fn with_target_host(mut self, host: Option<DetachedHostClaim>) -> Self {
        self.target_detached = self.target_detached.with_host(host);
        self
    }

    fn transfer_target(mut self, lease: DetachedHostLease) -> Self {
        self.target_detached = DetachedTargetLease::Transferred(lease.without_host());
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PresentationCandidate {
    pub(crate) presenter_hwnd: u64,
    pub(crate) native_generation: u64,
    pub(crate) host_hwnd: u64,
    pub(crate) requires_retire: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparingProgress {
    AwaitingHost {
        lease: DetachedSessionLease,
    },
    ReadyToPrepare {
        host: Option<DetachedHostClaim>,
    },
    AwaitingNative {
        host: Option<DetachedHostClaim>,
    },
    Aborting {
        aborted: PresentationRequest,
        aborted_candidate_hwnd: u64,
    },
    /// commit 済みの retire を待ちながら、次の要求を控えている。
    ///
    /// `NativeCommitted` は所有権の受け渡し点なので、そこから先の置換は**巻き戻しでは
    /// ない**。native は既に retire を走らせており、ここで abort を出すと同じ request に
    /// 対して `[Retire, Abort]` が並ぶ。native は retire を選ぶので abort は解決されず、
    /// それを待つ側が永久に止まる。`Aborting` と別の状態にしてあるのは、待つ相手
    /// (retire) も、終わったときに畳む物 (commit 済みの host) も違うため
    /// (2026-08-29 レビュー R-17 の裁定)。
    RetiringCommitted {
        retiring: PresentationRequest,
        retiring_generation: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommittingProgress {
    AwaitingNativeCommit,
    AwaitingRetire,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentationTransitionState {
    Stable {
        current: ViewerPresentation,
    },
    Preparing {
        request: PresentationRequest,
        progress: PreparingProgress,
    },
    Ready {
        request: PresentationRequest,
        candidate: PresentationCandidate,
    },
    Committing {
        request: PresentationRequest,
        candidate: PresentationCandidate,
        progress: CommittingProgress,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentationTransitionEvent {
    Drive,
    HostReady {
        request_id: u64,
        claim: DetachedHostClaim,
    },
    HostUnavailable {
        request_id: u64,
        claim: DetachedHostClaim,
    },
    NativeReady {
        request_id: u64,
        candidate: PresentationCandidate,
    },
    NativeCommitted {
        request_id: u64,
        candidate_generation: u64,
    },
    NativeRetired {
        request_id: u64,
        candidate_generation: u64,
    },
    NativeAborted {
        request_id: u64,
    },
    NativeFailed {
        request_id: u64,
    },
    TerminalClose,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentationTransitionEffect {
    PrepareNative {
        request: PresentationRequest,
        host: Option<DetachedHostClaim>,
    },
    PublishNative {
        request: PresentationRequest,
        candidate: PresentationCandidate,
    },
    AbortNative {
        request_id: u64,
        candidate_hwnd: u64,
    },
    RetireOutgoing {
        request: PresentationRequest,
        candidate: PresentationCandidate,
    },
    SetHostVisible {
        request: PresentationRequest,
        hwnd: u64,
    },
    FocusHost {
        request: PresentationRequest,
        hwnd: u64,
    },
    DestroyHost {
        request_id: u64,
        target: ViewerPresentation,
        lease: DetachedSessionLease,
        hwnd: u64,
    },
    ApplyPresentation {
        request: PresentationRequest,
        candidate_generation: u64,
    },
    CloseDetachedSession {
        request_id: u64,
        lease: DetachedSessionLease,
    },
    TerminalSessionClose {
        request_id: u64,
        target: ViewerPresentation,
    },
    SetZOrderRecoveryPermit(bool),
}

/// terminal 効果の実行が、通常の teardown まで到達したか。
///
/// `TerminalSessionClose` の実行は `close_fullscreen` を呼び、遷移が既に `Stable` なので
/// そのまま `close_fullscreen_now` まで走る。**呼び出し側が「まだ閉じていない」と見なして
/// 二度目を呼ぶと、初回 close が整えた状態を巻き戻す。**閉じたかどうかを bool ではなく
/// 名前で返すのは、呼び出し側が「何を確かめて分岐したのか」を読めるようにするため。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentationEffectsOutcome {
    /// 効果の実行中に `close_fullscreen` まで到達した。呼び出し側は再度閉じない。
    ClosedFullscreen,
    /// 到達していない。閉じる責任は呼び出し側にある。
    DidNotClose,
}

#[derive(Clone, Debug)]
pub(crate) struct PresentationTransitionOwner {
    next_request_id: u64,
    state: PresentationTransitionState,
    effects: std::collections::VecDeque<PresentationTransitionEffect>,
}

impl Default for PresentationTransitionOwner {
    fn default() -> Self {
        Self::stable(ViewerPresentation::MainWindow)
    }
}

impl PresentationTransitionOwner {
    pub(crate) fn stable(current: ViewerPresentation) -> Self {
        Self {
            next_request_id: 0,
            state: PresentationTransitionState::Stable { current },
            effects: std::collections::VecDeque::new(),
        }
    }

    /// R-27 以降、production は状態そのものではなく `awaited_detached_lease` /
    /// `request_id` の型付きの問いを使う。生の状態を読むのは reducer のテストだけ。
    #[cfg(test)]
    pub(crate) fn state(&self) -> PresentationTransitionState {
        self.state
    }

    pub(crate) fn is_transitioning(&self) -> bool {
        !matches!(self.state, PresentationTransitionState::Stable { .. })
    }

    pub(crate) fn z_order_recovery_permitted(&self) -> bool {
        matches!(self.state, PresentationTransitionState::Stable { .. })
    }

    /// いま画面を持っている表示。
    ///
    /// `NativeCommitted` を受理した時点で `ApplyPresentation` は既に出ており、画面は
    /// target 側へ移っている。retire を待っている間もそこは変わらないので、**その間に
    /// 来た要求は target を起点にする**。退場側を返すと、後継が持つ `current` が最初から
    /// 誤りになり、host の始末先も分岐も狂う (Codex Q1)。
    pub(crate) fn current(&self) -> ViewerPresentation {
        if let PresentationTransitionState::Committing {
            request,
            progress: CommittingProgress::AwaitingRetire,
            ..
        } = self.state
        {
            return request.target;
        }
        request_from_state(self.state)
            .map(|request| request.current)
            .unwrap_or_else(|| match self.state {
                PresentationTransitionState::Stable { current } => current,
                _ => unreachable!(),
            })
    }

    pub(crate) fn target(&self) -> ViewerPresentation {
        request_from_state(self.state)
            .map(|request| request.target)
            .unwrap_or_else(|| match self.state {
                PresentationTransitionState::Stable { current } => current,
                _ => unreachable!(),
            })
    }

    pub(crate) fn request_id(&self) -> Option<u64> {
        request_from_state(self.state).map(|request| request.id)
    }

    pub(crate) fn awaiting_detached_host(&self) -> bool {
        matches!(
            self.state,
            PresentationTransitionState::Preparing {
                request: PresentationRequest {
                    target: ViewerPresentation::DetachedWindow,
                    ..
                },
                progress: PreparingProgress::AwaitingHost { .. },
            }
        )
    }

    pub(crate) fn awaited_detached_lease(&self) -> Option<DetachedSessionLease> {
        match self.state {
            PresentationTransitionState::Preparing {
                progress: PreparingProgress::AwaitingHost { lease },
                ..
            } => Some(lease),
            _ => None,
        }
    }

    pub(crate) fn set_announce_main_hint(&mut self, request_id: u64) {
        match &mut self.state {
            PresentationTransitionState::Preparing { request, .. }
            | PresentationTransitionState::Ready { request, .. }
            | PresentationTransitionState::Committing { request, .. }
                if request.id == request_id =>
            {
                request.announce_main_hint = true;
            }
            _ => {}
        }
    }

    pub(crate) fn sync_stable(&mut self, current: ViewerPresentation) {
        if matches!(self.state, PresentationTransitionState::Stable { .. }) {
            self.state = PresentationTransitionState::Stable { current };
        }
    }

    #[cfg(test)]
    pub(crate) fn reset_stable(&mut self, current: ViewerPresentation) {
        self.state = PresentationTransitionState::Stable { current };
        self.effects.clear();
    }

    pub(crate) fn request_transition(
        &mut self,
        target: ViewerPresentation,
        activate: bool,
        announce_main_hint: bool,
        outgoing_presenter_hwnd: u64,
        current_detached: Option<DetachedHostLease>,
        target_detached: Option<DetachedHostLease>,
    ) -> u64 {
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        let id = self.next_request_id;
        let current = self.current();
        let same_detached_lease = current_detached
            .zip(target_detached)
            .is_some_and(|(current, target)| current.session == target.session);
        let outgoing_detached = (current == ViewerPresentation::DetachedWindow
            && !same_detached_lease)
            .then_some(current_detached)
            .flatten();
        let target_detached = if target != ViewerPresentation::DetachedWindow {
            DetachedTargetLease::None
        } else if same_detached_lease {
            DetachedTargetLease::KeepLive(target_detached.unwrap())
        } else {
            target_detached
                .map(DetachedTargetLease::Candidate)
                .unwrap_or(DetachedTargetLease::None)
        };
        let (state, effects) = reduce_presentation_request(
            self.state,
            PresentationRequest {
                id,
                current,
                target,
                activate,
                announce_main_hint,
                outgoing_presenter_hwnd,
                outgoing_detached,
                target_detached,
            },
        );
        self.state = state;
        self.effects.extend(effects);
        id
    }

    pub(crate) fn dispatch(&mut self, event: PresentationTransitionEvent) {
        let (state, effects) = reduce_presentation_transition(self.state, event);
        self.state = state;
        self.effects.extend(effects);
    }

    pub(crate) fn drive(&mut self) {
        self.dispatch(PresentationTransitionEvent::Drive);
    }

    pub(crate) fn take_effects(&mut self) -> Vec<PresentationTransitionEffect> {
        self.effects.drain(..).collect()
    }
}

fn request_from_state(state: PresentationTransitionState) -> Option<PresentationRequest> {
    match state {
        PresentationTransitionState::Stable { .. } => None,
        PresentationTransitionState::Preparing { request, .. }
        | PresentationTransitionState::Ready { request, .. }
        | PresentationTransitionState::Committing { request, .. } => Some(request),
    }
}

fn initial_progress(request: PresentationRequest) -> PreparingProgress {
    match request.target_session() {
        Some(lease) => match request.target_host() {
            Some(host) => PreparingProgress::ReadyToPrepare { host: Some(host) },
            None => PreparingProgress::AwaitingHost { lease },
        },
        None => PreparingProgress::ReadyToPrepare { host: None },
    }
}

fn request_already_has_current_target(request: PresentationRequest) -> bool {
    request.target == request.current
        && (request.target != ViewerPresentation::DetachedWindow
            || matches!(request.target_detached, DetachedTargetLease::KeepLive(_)))
}

fn pending_successor_session_is_protected(
    session: DetachedSessionLease,
    protected_requests: &[PresentationRequest],
) -> bool {
    protected_requests.iter().any(|request| {
        request.target_session() == Some(session)
            || request
                .outgoing_detached
                .is_some_and(|lease| lease.session == session)
    })
}

/// Replace a successor that has not crossed the native boundary yet.
///
/// Its outgoing lease still belongs to the published presentation and must not be closed here.
/// A distinct candidate viewport, however, is owned only by the discarded successor. Leases also
/// named by the in-flight abort/retire request stay with that request until its terminal event.
fn replace_pending_successor(
    discarded: PresentationRequest,
    mut replacement: PresentationRequest,
    protected_requests: &[PresentationRequest],
    effects: &mut Vec<PresentationTransitionEffect>,
) -> PresentationRequest {
    match discarded.target_detached {
        DetachedTargetLease::Transferred(lease)
            if replacement.target_session() == Some(lease.session) =>
        {
            replacement = replacement.transfer_target(lease);
        }
        DetachedTargetLease::Transferred(lease)
            if !pending_successor_session_is_protected(lease.session, protected_requests) =>
        {
            push_detached_release(discarded.id, discarded.target, lease, effects);
        }
        DetachedTargetLease::Candidate(lease)
            if replacement.target_session() != Some(lease.session)
                && !pending_successor_session_is_protected(lease.session, protected_requests) =>
        {
            push_detached_destroy(discarded.id, discarded.target, lease, effects);
        }
        DetachedTargetLease::None
        | DetachedTargetLease::Candidate(_)
        | DetachedTargetLease::KeepLive(_)
        | DetachedTargetLease::Transferred(_) => {}
    }
    replacement
}

fn candidate_hwnd(state: PresentationTransitionState) -> u64 {
    match state {
        PresentationTransitionState::Ready { candidate, .. }
        | PresentationTransitionState::Committing { candidate, .. } => candidate.presenter_hwnd,
        PresentationTransitionState::Preparing {
            progress:
                PreparingProgress::Aborting {
                    aborted_candidate_hwnd,
                    ..
                },
            ..
        } => aborted_candidate_hwnd,
        _ => 0,
    }
}

/// この state が持つ `request` は、既に native へ渡してあるか。
///
/// **`Aborting` を含めてはならない。**`Aborting` の `request` は「abort が終わったら
/// 始める後継」であって、native はそれを知らない。含めると、後継に対して abort を
/// 出し、in-flight な abort の identity をその後継 id で上書きしてしまう。上書きすると
/// native が返す元 request の `NativeAborted` / `NativeFailed` がどの照合にも一致せず、
/// 遷移が永久に待つ (2026-08-29 レビュー R-01)。`Aborting` は
/// [`reduce_replaced_request`] と [`terminal_close`] が専用の分岐で扱う。
fn native_prepare_was_issued(state: PresentationTransitionState) -> bool {
    matches!(
        state,
        PresentationTransitionState::Preparing {
            progress: PreparingProgress::AwaitingNative { .. },
            ..
        } | PresentationTransitionState::Ready { .. }
            | PresentationTransitionState::Committing { .. }
    )
}
fn reduce_replaced_request(
    state: PresentationTransitionState,
    request: PresentationRequest,
    mut effects: Vec<PresentationTransitionEffect>,
) -> (
    PresentationTransitionState,
    Vec<PresentationTransitionEffect>,
) {
    // commit 済みの retire を待っている間の置換は巻き戻しではない。abort を出すと
    // 同じ request へ `[Retire, Abort]` が並び、native は retire を選ぶので abort は
    // 解決されない。retire の完了を待って後継を始める (Codex Q2)。
    if let PresentationTransitionState::Committing {
        request: retiring,
        candidate,
        progress: CommittingProgress::AwaitingRetire,
    } = state
    {
        return (
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::RetiringCommitted {
                    retiring,
                    retiring_generation: candidate.native_generation,
                },
            },
            effects,
        );
    }
    // 更に置換されても、native が走らせている retire には触らない。後継だけ差し替える。
    if let PresentationTransitionState::Preparing {
        request: discarded,
        progress:
            PreparingProgress::RetiringCommitted {
                retiring,
                retiring_generation,
                ..
            },
        ..
    } = state
    {
        let request = replace_pending_successor(discarded, request, &[retiring], &mut effects);
        return (
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::RetiringCommitted {
                    retiring,
                    retiring_generation,
                },
            },
            effects,
        );
    }
    // 既に abort が飛んでいるなら、native が持っているのは「中止した側」であって、
    // ここに居る後継ではない。差し替えてよいのは後継だけで、in-flight な abort の
    // identity には触らない。二重の `AbortNative` も出さない (R-01)。
    if let PresentationTransitionState::Preparing {
        request: discarded,
        progress:
            PreparingProgress::Aborting {
                aborted,
                aborted_candidate_hwnd,
            },
        ..
    } = state
    {
        let request = replace_pending_successor(discarded, request, &[aborted], &mut effects);
        return (
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::Aborting {
                    aborted,
                    aborted_candidate_hwnd,
                },
            },
            effects,
        );
    }
    let old_request = request_from_state(state).unwrap();
    if native_prepare_was_issued(state) {
        let aborted_candidate_hwnd = candidate_hwnd(state);
        effects.push(PresentationTransitionEffect::AbortNative {
            request_id: old_request.id,
            candidate_hwnd: aborted_candidate_hwnd,
        });
        return (
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::Aborting {
                    aborted: old_request,
                    aborted_candidate_hwnd,
                },
            },
            effects,
        );
    }
    finish_replaced_then_start(old_request, request, false, effects)
}

fn finish_replaced_then_start(
    old_request: PresentationRequest,
    mut request: PresentationRequest,
    crossed_native_boundary: bool,
    mut effects: Vec<PresentationTransitionEffect>,
) -> (
    PresentationTransitionState,
    Vec<PresentationTransitionEffect>,
) {
    match old_request.target_detached {
        DetachedTargetLease::Transferred(old) if request.target_session() == Some(old.session) => {
            request = request.transfer_target(old);
        }
        DetachedTargetLease::Transferred(old) => {
            push_detached_release(old_request.id, old_request.target, old, &mut effects);
        }
        DetachedTargetLease::Candidate(old) if request.target_session() != Some(old.session) => {
            push_detached_destroy(old_request.id, old_request.target, old, &mut effects);
        }
        DetachedTargetLease::None
        | DetachedTargetLease::Candidate(_)
        | DetachedTargetLease::KeepLive(_) => {}
    }
    if request_already_has_current_target(request) {
        effects.push(PresentationTransitionEffect::SetZOrderRecoveryPermit(true));
        (
            PresentationTransitionState::Stable {
                current: request.current,
            },
            effects,
        )
    } else {
        (
            PresentationTransitionState::Preparing {
                request: if crossed_native_boundary {
                    request.with_target_host(None)
                } else {
                    request
                },
                progress: if crossed_native_boundary {
                    initial_progress(request.with_target_host(None))
                } else {
                    initial_progress(request)
                },
            },
            effects,
        )
    }
}

fn reduce_presentation_transition(
    state: PresentationTransitionState,
    event: PresentationTransitionEvent,
) -> (
    PresentationTransitionState,
    Vec<PresentationTransitionEffect>,
) {
    let mut effects = Vec::new();
    match (state, event) {
        (
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::ReadyToPrepare { host },
            },
            PresentationTransitionEvent::Drive,
        ) => {
            effects.push(PresentationTransitionEffect::PrepareNative { request, host });
            (
                PresentationTransitionState::Preparing {
                    request,
                    progress: PreparingProgress::AwaitingNative { host },
                },
                effects,
            )
        }
        (
            PresentationTransitionState::Ready { request, candidate },
            PresentationTransitionEvent::Drive,
        ) => {
            effects.push(PresentationTransitionEffect::PublishNative { request, candidate });
            (
                PresentationTransitionState::Committing {
                    request,
                    candidate,
                    progress: CommittingProgress::AwaitingNativeCommit,
                },
                effects,
            )
        }
        (
            PresentationTransitionState::Preparing {
                mut request,
                progress: PreparingProgress::AwaitingHost { lease },
            },
            PresentationTransitionEvent::HostReady { request_id, claim },
        ) if request.id == request_id && claim.lease == lease && claim.hwnd != 0 => {
            request = request.with_target_host(Some(claim));
            (
                PresentationTransitionState::Preparing {
                    request,
                    progress: PreparingProgress::ReadyToPrepare { host: Some(claim) },
                },
                effects,
            )
        }
        (
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::AwaitingNative { host: Some(host) },
            },
            PresentationTransitionEvent::HostUnavailable { request_id, claim },
        ) if request.id == request_id && claim == host => {
            let request = request.with_target_host(None);
            (
                PresentationTransitionState::Preparing {
                    request,
                    progress: PreparingProgress::AwaitingHost { lease: host.lease },
                },
                effects,
            )
        }
        (
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::AwaitingNative { host },
            },
            PresentationTransitionEvent::NativeReady {
                request_id,
                candidate,
            },
        ) if request.id == request_id
            && (request.target != ViewerPresentation::DetachedWindow
                || host.is_some_and(|claim| candidate.host_hwnd == claim.hwnd)) =>
        {
            (
                PresentationTransitionState::Ready { request, candidate },
                effects,
            )
        }
        pair => reduce_late_transition(pair.0, pair.1, effects),
    }
}

fn reduce_late_transition(
    state: PresentationTransitionState,
    event: PresentationTransitionEvent,
    mut effects: Vec<PresentationTransitionEffect>,
) -> (
    PresentationTransitionState,
    Vec<PresentationTransitionEffect>,
) {
    match (state, event) {
        (
            PresentationTransitionState::Committing {
                request,
                candidate,
                progress: CommittingProgress::AwaitingNativeCommit,
            },
            PresentationTransitionEvent::NativeCommitted {
                request_id,
                candidate_generation,
            },
        ) if request.id == request_id && candidate.native_generation == candidate_generation => {
            effects.push(PresentationTransitionEffect::ApplyPresentation {
                request,
                candidate_generation,
            });
            if request.target == ViewerPresentation::DetachedWindow
                && request.current != request.target
            {
                effects.push(PresentationTransitionEffect::SetHostVisible {
                    request,
                    hwnd: candidate.host_hwnd,
                });
                if request.activate {
                    effects.push(PresentationTransitionEffect::FocusHost {
                        request,
                        hwnd: candidate.host_hwnd,
                    });
                }
            }
            // NativeCommitted is emitted only after the candidate has been attached, primed,
            // and published by the pump. That publication transfers presenter ownership. The
            // outgoing presenter may wait for that boundary, but not for a later App::update to
            // observe the egui host's OS visibility. Keep the host commands ahead of retire in
            // the effect order while making their completion independent.
            finish_or_retire(request, candidate, effects)
        }
        (
            PresentationTransitionState::Committing {
                request,
                progress: CommittingProgress::AwaitingRetire,
                ..
            },
            PresentationTransitionEvent::NativeFailed { request_id },
        ) if request.id == request_id => {
            if let Some(outgoing) = request.outgoing_detached {
                push_detached_release(request.id, request.target, outgoing, &mut effects);
            }
            effects.push(PresentationTransitionEffect::SetZOrderRecoveryPermit(true));
            (
                PresentationTransitionState::Stable {
                    current: request.target,
                },
                effects,
            )
        }
        pair => reduce_completion_transition(pair.0, pair.1, effects),
    }
}

fn push_detached_destroy(
    request_id: u64,
    target: ViewerPresentation,
    lease: DetachedHostLease,
    effects: &mut Vec<PresentationTransitionEffect>,
) {
    if effects.iter().any(|effect| {
        matches!(
            effect,
            PresentationTransitionEffect::DestroyHost {
                lease: existing,
                ..
            } if *existing == lease.session
        )
    }) {
        return;
    }
    effects.push(PresentationTransitionEffect::DestroyHost {
        request_id,
        target,
        lease: lease.session,
        hwnd: lease.hwnd(),
    });
}

fn push_detached_release(
    request_id: u64,
    target: ViewerPresentation,
    lease: DetachedHostLease,
    effects: &mut Vec<PresentationTransitionEffect>,
) {
    if !effects.iter().any(|effect| {
        matches!(
            effect,
            PresentationTransitionEffect::CloseDetachedSession {
                lease: existing,
                ..
            } if *existing == lease.session
        )
    }) {
        effects.push(PresentationTransitionEffect::CloseDetachedSession {
            request_id,
            lease: lease.session,
        });
    }
    push_detached_destroy(request_id, target, lease, effects);
}

fn reduce_completion_transition(
    state: PresentationTransitionState,
    event: PresentationTransitionEvent,
    mut effects: Vec<PresentationTransitionEffect>,
) -> (
    PresentationTransitionState,
    Vec<PresentationTransitionEffect>,
) {
    match (state, event) {
        (
            PresentationTransitionState::Committing {
                request,
                candidate,
                progress: CommittingProgress::AwaitingRetire,
            },
            PresentationTransitionEvent::NativeRetired {
                request_id,
                candidate_generation,
            },
        ) if request.id == request_id && candidate.native_generation == candidate_generation => {
            if let Some(outgoing) = request.outgoing_detached {
                push_detached_release(request.id, request.target, outgoing, &mut effects);
            }
            effects.push(PresentationTransitionEffect::SetZOrderRecoveryPermit(true));
            (
                PresentationTransitionState::Stable {
                    current: request.target,
                },
                effects,
            )
        }
        (
            PresentationTransitionState::Preparing {
                request,
                progress:
                    PreparingProgress::RetiringCommitted {
                        retiring,
                        retiring_generation,
                    },
            },
            PresentationTransitionEvent::NativeRetired {
                request_id,
                candidate_generation,
            },
        ) if retiring.id == request_id && retiring_generation == candidate_generation => {
            finish_retired_then_start(retiring, request, effects)
        }
        // retire が失敗で終わっても、commit 済みの表示が残らないことに変わりはない。
        // ここで拾わないと後継が永久に始まらない (`Aborting` の同型)。
        (
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::RetiringCommitted { retiring, .. },
            },
            PresentationTransitionEvent::NativeFailed { request_id },
        ) if retiring.id == request_id => finish_retired_then_start(retiring, request, effects),
        pair => reduce_abort_transition(pair.0, pair.1, effects),
    }
}

/// commit 済みの retire が終わったので、控えていた要求を始める。
///
/// 畳む物は `Committing` の retire 完了と同じ。違うのは、そのまま `Stable` にせず
/// 後継へ進むこと。
fn finish_retired_then_start(
    retiring: PresentationRequest,
    mut successor: PresentationRequest,
    mut effects: Vec<PresentationTransitionEffect>,
) -> (
    PresentationTransitionState,
    Vec<PresentationTransitionEffect>,
) {
    if let Some(outgoing) = retiring.outgoing_detached {
        if successor.target_session() == Some(outgoing.session) {
            successor = successor.transfer_target(outgoing);
        } else {
            push_detached_release(retiring.id, retiring.target, outgoing, &mut effects);
        }
    }
    if request_already_has_current_target(successor) {
        effects.push(PresentationTransitionEffect::SetZOrderRecoveryPermit(true));
        return (
            PresentationTransitionState::Stable {
                current: successor.current,
            },
            effects,
        );
    }
    (
        PresentationTransitionState::Preparing {
            request: successor.with_target_host(None),
            progress: initial_progress(successor.with_target_host(None)),
        },
        effects,
    )
}

fn finish_or_retire(
    request: PresentationRequest,
    candidate: PresentationCandidate,
    mut effects: Vec<PresentationTransitionEffect>,
) -> (
    PresentationTransitionState,
    Vec<PresentationTransitionEffect>,
) {
    if candidate.requires_retire {
        effects.push(PresentationTransitionEffect::RetireOutgoing { request, candidate });
        (
            PresentationTransitionState::Committing {
                request,
                candidate,
                progress: CommittingProgress::AwaitingRetire,
            },
            effects,
        )
    } else {
        if let Some(outgoing) = request.outgoing_detached {
            push_detached_release(request.id, request.target, outgoing, &mut effects);
        }
        effects.push(PresentationTransitionEffect::SetZOrderRecoveryPermit(true));
        (
            PresentationTransitionState::Stable {
                current: request.target,
            },
            effects,
        )
    }
}

fn reduce_abort_transition(
    state: PresentationTransitionState,
    event: PresentationTransitionEvent,
    mut effects: Vec<PresentationTransitionEffect>,
) -> (
    PresentationTransitionState,
    Vec<PresentationTransitionEffect>,
) {
    match (state, event) {
        (
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::Aborting { aborted, .. },
            },
            // native が abort を「失敗」で終えた場合も、その request が終わったこと
            // に変わりはない。ここで拾わないと、下の汎用 `NativeFailed` 分岐が後継の
            // id と照合して外し、catch-all に落ちて永久に待つ (R-01 と同じ取り違え。
            // Codex の Q5 指摘で発覚)。
            PresentationTransitionEvent::NativeAborted { request_id }
            | PresentationTransitionEvent::NativeFailed { request_id },
        ) if aborted.id == request_id => {
            finish_replaced_then_start(aborted, request, true, effects)
        }
        (active_state, PresentationTransitionEvent::NativeFailed { request_id })
            if request_from_state(active_state).is_some_and(|request| request.id == request_id) =>
        {
            let request = request_from_state(active_state).unwrap();
            match request.target_detached {
                DetachedTargetLease::Candidate(lease) => {
                    push_detached_destroy(request_id, request.target, lease, &mut effects);
                }
                DetachedTargetLease::Transferred(lease) => {
                    push_detached_release(request_id, request.target, lease, &mut effects);
                }
                DetachedTargetLease::None | DetachedTargetLease::KeepLive(_) => {}
            }
            effects.push(PresentationTransitionEffect::SetZOrderRecoveryPermit(true));
            (
                PresentationTransitionState::Stable {
                    current: request.current,
                },
                effects,
            )
        }
        (state, PresentationTransitionEvent::TerminalClose) => terminal_close(state, effects),
        (state, _) => (state, effects),
    }
}

fn terminal_close(
    state: PresentationTransitionState,
    mut effects: Vec<PresentationTransitionEffect>,
) -> (
    PresentationTransitionState,
    Vec<PresentationTransitionEffect>,
) {
    let request = request_from_state(state);
    let request_id = request.map_or(0, |request| request.id);
    let retiring_is_running = matches!(
        state,
        PresentationTransitionState::Committing {
            progress: CommittingProgress::AwaitingRetire,
            ..
        } | PresentationTransitionState::Preparing {
            progress: PreparingProgress::RetiringCommitted { .. },
            ..
        }
    );
    if native_prepare_was_issued(state) && !retiring_is_running {
        effects.push(PresentationTransitionEffect::AbortNative {
            request_id,
            candidate_hwnd: candidate_hwnd(state),
        });
    }
    match state {
        PresentationTransitionState::Preparing {
            request,
            progress: PreparingProgress::Aborting { aborted, .. },
        } => {
            push_terminal_request_cleanup(aborted, false, &mut effects);
            push_terminal_request_cleanup(request, false, &mut effects);
        }
        PresentationTransitionState::Preparing {
            request,
            progress: PreparingProgress::RetiringCommitted { retiring, .. },
        } => {
            push_terminal_request_cleanup(retiring, true, &mut effects);
            push_terminal_request_cleanup(request, false, &mut effects);
        }
        PresentationTransitionState::Committing {
            request,
            progress: CommittingProgress::AwaitingRetire,
            ..
        } => push_terminal_request_cleanup(request, true, &mut effects),
        PresentationTransitionState::Preparing { request, .. }
        | PresentationTransitionState::Ready { request, .. }
        | PresentationTransitionState::Committing { request, .. } => {
            push_terminal_request_cleanup(request, false, &mut effects);
        }
        PresentationTransitionState::Stable { .. } => {}
    }
    let current = match state {
        PresentationTransitionState::Stable { current } => current,
        PresentationTransitionState::Committing {
            request,
            progress: CommittingProgress::AwaitingRetire,
            ..
        } => request.target,
        PresentationTransitionState::Preparing {
            progress: PreparingProgress::RetiringCommitted { retiring, .. },
            ..
        } => retiring.target,
        _ => request.unwrap().current,
    };
    let target = request.map_or(current, |request| request.target);
    effects.push(PresentationTransitionEffect::TerminalSessionClose { request_id, target });
    effects.push(PresentationTransitionEffect::SetZOrderRecoveryPermit(true));
    (PresentationTransitionState::Stable { current }, effects)
}

fn push_terminal_request_cleanup(
    request: PresentationRequest,
    target_committed: bool,
    effects: &mut Vec<PresentationTransitionEffect>,
) {
    if let Some(outgoing) = request.outgoing_detached {
        push_detached_release(request.id, request.target, outgoing, effects);
    }
    match request.target_detached {
        DetachedTargetLease::None => {}
        DetachedTargetLease::Candidate(lease) if target_committed => {
            push_detached_release(request.id, request.target, lease, effects);
        }
        DetachedTargetLease::Candidate(lease) => {
            push_detached_destroy(request.id, request.target, lease, effects);
        }
        DetachedTargetLease::KeepLive(lease) | DetachedTargetLease::Transferred(lease) => {
            push_detached_release(request.id, request.target, lease, effects);
        }
    }
}

fn reduce_presentation_request(
    state: PresentationTransitionState,
    request: PresentationRequest,
) -> (
    PresentationTransitionState,
    Vec<PresentationTransitionEffect>,
) {
    let mut effects = Vec::new();
    if matches!(state, PresentationTransitionState::Stable { .. }) {
        effects.push(PresentationTransitionEffect::SetZOrderRecoveryPermit(false));
        return (
            PresentationTransitionState::Preparing {
                request,
                progress: initial_progress(request),
            },
            effects,
        );
    }
    reduce_replaced_request(state, request, effects)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTGOING_PRESENTER: u64 = 0x101;
    const OUTGOING_WINDOW: u64 = 0x201;
    const OUTGOING_HOST: u64 = 0x202;
    const CANDIDATE_PRESENTER: u64 = 0x303;
    const CANDIDATE_WINDOW: u64 = 0x403;
    const CANDIDATE_HOST: u64 = 0x404;
    const MAIN_HOST: u64 = 0x505;
    const GENERATION: u64 = 7;

    fn request(
        owner: &mut PresentationTransitionOwner,
        target: ViewerPresentation,
        ready_host: u64,
    ) -> u64 {
        let current = owner.current();
        let current_detached = (current == ViewerPresentation::DetachedWindow)
            .then(|| host_lease(OUTGOING_WINDOW, OUTGOING_HOST, 1));
        let target_detached = (target == ViewerPresentation::DetachedWindow).then(|| {
            if ready_host == OUTGOING_HOST {
                host_lease(OUTGOING_WINDOW, OUTGOING_HOST, 1)
            } else {
                DetachedHostLease {
                    session: DetachedSessionLease {
                        window_id: CANDIDATE_WINDOW,
                    },
                    host: (ready_host != 0).then_some(DetachedHostClaim {
                        lease: DetachedSessionLease {
                            window_id: CANDIDATE_WINDOW,
                        },
                        incarnation: 1,
                        hwnd: ready_host,
                    }),
                }
            }
        });
        owner.request_transition(
            target,
            true,
            false,
            OUTGOING_PRESENTER,
            current_detached,
            target_detached,
        )
    }

    fn host_lease(window_id: u64, hwnd: u64, incarnation: u64) -> DetachedHostLease {
        let session = DetachedSessionLease { window_id };
        DetachedHostLease {
            session,
            host: Some(DetachedHostClaim {
                lease: session,
                incarnation,
                hwnd,
            }),
        }
    }

    fn candidate(host_hwnd: u64) -> PresentationCandidate {
        PresentationCandidate {
            presenter_hwnd: CANDIDATE_PRESENTER,
            native_generation: GENERATION,
            host_hwnd,
            requires_retire: true,
        }
    }

    fn drive_to_native_wait(owner: &mut PresentationTransitionOwner) {
        owner.take_effects();
        owner.drive();
        assert!(matches!(
            owner.state(),
            PresentationTransitionState::Preparing {
                progress: PreparingProgress::AwaitingNative { .. },
                ..
            }
        ));
        assert!(matches!(
            owner.take_effects().as_slice(),
            [PresentationTransitionEffect::PrepareNative { .. }]
        ));
    }

    fn drive_to_committing(
        owner: &mut PresentationTransitionOwner,
        request_id: u64,
        host_hwnd: u64,
    ) {
        owner.dispatch(PresentationTransitionEvent::NativeReady {
            request_id,
            candidate: candidate(host_hwnd),
        });
        assert!(matches!(
            owner.state(),
            PresentationTransitionState::Ready { .. }
        ));
        owner.drive();
        assert!(matches!(
            owner.take_effects().as_slice(),
            [PresentationTransitionEffect::PublishNative { .. }]
        ));
    }

    fn explicit_request(
        owner: &mut PresentationTransitionOwner,
        target: ViewerPresentation,
        current_detached: Option<DetachedHostLease>,
        target_detached: Option<DetachedHostLease>,
    ) -> u64 {
        owner.request_transition(
            target,
            true,
            false,
            OUTGOING_PRESENTER,
            current_detached,
            target_detached,
        )
    }

    fn begin_transferred_successor(owner: &mut PresentationTransitionOwner) -> u64 {
        let old = host_lease(OUTGOING_WINDOW, OUTGOING_HOST, 1);
        let retiring_id = explicit_request(owner, ViewerPresentation::Fullscreen, Some(old), None);
        drive_to_native_wait(owner);
        drive_to_committing(owner, retiring_id, 0);
        owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id: retiring_id,
            candidate_generation: GENERATION,
        });
        owner.take_effects();

        let successor_id =
            explicit_request(owner, ViewerPresentation::DetachedWindow, None, Some(old));
        assert_eq!(
            detached_cleanup_counts(&owner.take_effects(), old.session),
            (0, 0),
            "requesting an alias successor must not release the retiring lease early"
        );
        owner.dispatch(PresentationTransitionEvent::NativeRetired {
            request_id: retiring_id,
            candidate_generation: GENERATION,
        });
        let effects = owner.take_effects();
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::CloseDetachedSession { .. }
                | PresentationTransitionEffect::DestroyHost { .. }
        )));
        assert!(matches!(
            owner.state(),
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::AwaitingHost { lease },
            } if request.id == successor_id && lease == old.session
        ));
        successor_id
    }

    fn reacquire_transferred_host(
        owner: &mut PresentationTransitionOwner,
        request_id: u64,
    ) -> DetachedHostClaim {
        let claim = host_lease(OUTGOING_WINDOW, CANDIDATE_HOST, 2).host.unwrap();
        owner.dispatch(PresentationTransitionEvent::HostReady { request_id, claim });
        owner.drive();
        assert!(matches!(
            owner.take_effects().as_slice(),
            [PresentationTransitionEffect::PrepareNative {
                host: Some(prepared),
                ..
            }] if *prepared == claim
        ));
        claim
    }

    fn detached_cleanup_counts(
        effects: &[PresentationTransitionEffect],
        lease: DetachedSessionLease,
    ) -> (usize, usize) {
        let closes = effects
            .iter()
            .filter(|effect| {
                matches!(
                    effect,
                    PresentationTransitionEffect::CloseDetachedSession {
                        lease: closed,
                        ..
                    } if *closed == lease
                )
            })
            .count();
        let destroys = effects
            .iter()
            .filter(|effect| {
                matches!(
                    effect,
                    PresentationTransitionEffect::DestroyHost {
                        lease: destroyed,
                        ..
                    } if *destroyed == lease
                )
            })
            .count();
        (closes, destroys)
    }

    #[test]
    fn fullscreen_to_detached_retires_on_native_commit_without_waiting_for_host_visibility() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let request_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        drive_to_native_wait(&mut owner);
        drive_to_committing(&mut owner, request_id, CANDIDATE_HOST);
        owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id,
            candidate_generation: GENERATION,
        });
        let effects = owner.take_effects();
        assert!(matches!(
            effects.as_slice(),
            [
                PresentationTransitionEffect::ApplyPresentation { .. },
                PresentationTransitionEffect::SetHostVisible {
                    hwnd: CANDIDATE_HOST,
                    ..
                },
                PresentationTransitionEffect::FocusHost {
                    hwnd: CANDIDATE_HOST,
                    ..
                },
                PresentationTransitionEffect::RetireOutgoing { .. },
            ]
        ));
        assert!(matches!(
            owner.state(),
            PresentationTransitionState::Committing {
                progress: CommittingProgress::AwaitingRetire,
                ..
            }
        ));
        owner.dispatch(PresentationTransitionEvent::NativeRetired {
            request_id,
            candidate_generation: GENERATION,
        });
        assert_eq!(
            owner.state(),
            PresentationTransitionState::Stable {
                current: ViewerPresentation::DetachedWindow
            }
        );
        // Killing mutation: move finish_or_retire back behind an incoming-host visibility event.
        // RetireOutgoing disappears from the NativeCommitted effect batch and this test fails.
    }

    #[test]
    fn happy_detached_to_fullscreen_retires_before_destroying_detached_host() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::DetachedWindow);
        let request_id = request(&mut owner, ViewerPresentation::Fullscreen, 0);
        drive_to_native_wait(&mut owner);
        drive_to_committing(&mut owner, request_id, MAIN_HOST);
        owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id,
            candidate_generation: GENERATION,
        });
        let effects = owner.take_effects();
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                PresentationTransitionEffect::RetireOutgoing { .. }
            ))
        );
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, PresentationTransitionEffect::DestroyHost { .. }))
        );
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::SetHostVisible { .. }
                | PresentationTransitionEffect::FocusHost { .. }
        )));
        owner.dispatch(PresentationTransitionEvent::NativeRetired {
            request_id,
            candidate_generation: GENERATION,
        });
        let effects = owner.take_effects();
        assert!(effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::DestroyHost {
                hwnd: OUTGOING_HOST,
                ..
            }
        )));
        assert_eq!(
            owner.state(),
            PresentationTransitionState::Stable {
                current: ViewerPresentation::Fullscreen
            }
        );
        // Killing mutation: change current == DetachedWindow to target == DetachedWindow on retire.
    }

    #[test]
    fn same_presentation_detached_transition_does_not_retire_the_live_host() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::DetachedWindow);
        let request_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            OUTGOING_HOST,
        );
        drive_to_native_wait(&mut owner);
        drive_to_committing(&mut owner, request_id, OUTGOING_HOST);
        owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id,
            candidate_generation: GENERATION,
        });
        let effects = owner.take_effects();
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                PresentationTransitionEffect::RetireOutgoing { .. }
            ))
        );
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::CloseDetachedSession { .. }
                | PresentationTransitionEffect::DestroyHost { .. }
        )));

        owner.dispatch(PresentationTransitionEvent::NativeRetired {
            request_id,
            candidate_generation: GENERATION,
        });
        let effects = owner.take_effects();
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::CloseDetachedSession { .. }
                | PresentationTransitionEffect::DestroyHost { .. }
        )));
        assert_eq!(
            owner.state(),
            PresentationTransitionState::Stable {
                current: ViewerPresentation::DetachedWindow
            }
        );
        // Killing mutation: classify DetachedWindow -> DetachedWindow as RetireOutgoing (or
        // restore the old `request.current == DetachedWindow` completion check); the live host
        // produces CloseDetachedSession + DestroyHost after native retire.
    }

    #[test]
    fn failed_candidate_keeps_outgoing_presentation_and_cleans_hidden_host() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let request_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        drive_to_native_wait(&mut owner);
        owner.dispatch(PresentationTransitionEvent::NativeFailed { request_id });
        assert_eq!(
            owner.state(),
            PresentationTransitionState::Stable {
                current: ViewerPresentation::Fullscreen
            }
        );
        let effects = owner.take_effects();
        assert!(effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::DestroyHost {
                hwnd: CANDIDATE_HOST,
                ..
            }
        )));
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::ApplyPresentation { .. }
        )));
        // Killing mutation: replace request.current with request.target in NativeFailed.
    }

    #[test]
    fn second_f12_aborts_old_generation_and_returns_to_original_presentation() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let first_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        drive_to_native_wait(&mut owner);
        let second_id = request(&mut owner, ViewerPresentation::Fullscreen, 0);
        assert!(second_id > first_id);
        assert!(matches!(
            owner.take_effects().as_slice(),
            [PresentationTransitionEffect::AbortNative {
                request_id,
                ..
            }] if *request_id == first_id
        ));
        owner.dispatch(PresentationTransitionEvent::NativeAborted {
            request_id: first_id,
        });
        assert!(owner.take_effects().iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::DestroyHost {
                hwnd: CANDIDATE_HOST,
                ..
            }
        )));
        assert_eq!(
            owner.state(),
            PresentationTransitionState::Stable {
                current: ViewerPresentation::Fullscreen
            }
        );
        assert!(owner.z_order_recovery_permitted());
        // Killing mutation: set aborted_candidate_host_hwnd to 0 in reduce_replaced_request.
    }

    #[test]
    fn escape_during_ready_is_reduced_to_abort_destroy_and_terminal_close() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let request_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        drive_to_native_wait(&mut owner);
        owner.dispatch(PresentationTransitionEvent::NativeReady {
            request_id,
            candidate: candidate(CANDIDATE_HOST),
        });
        owner.dispatch(PresentationTransitionEvent::TerminalClose);
        let effects = owner.take_effects();
        assert!(effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::AbortNative {
                request_id: actual,
                candidate_hwnd: CANDIDATE_PRESENTER,
            } if *actual == request_id
        )));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::DestroyHost {
                hwnd: CANDIDATE_HOST,
                ..
            }
        )));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::TerminalSessionClose { .. }
        )));
        // Killing mutation: remove TerminalSessionClose from terminal_close.
    }

    #[test]
    fn player_end_during_native_prepare_aborts_candidate_before_terminal_close() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let request_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        drive_to_native_wait(&mut owner);
        owner.dispatch(PresentationTransitionEvent::TerminalClose);
        let effects = owner.take_effects();
        assert!(matches!(
            effects.as_slice(),
            [
                PresentationTransitionEffect::AbortNative {
                    request_id: actual,
                    candidate_hwnd: 0,
                },
                PresentationTransitionEffect::DestroyHost {
                    hwnd: CANDIDATE_HOST,
                    ..
                },
                PresentationTransitionEffect::TerminalSessionClose { .. },
                PresentationTransitionEffect::SetZOrderRecoveryPermit(true),
            ] if *actual == request_id
        ));
        // Killing mutation: remove AwaitingNative from native_prepare_was_issued.
    }

    #[test]
    fn window_close_during_commit_aborts_published_candidate_before_session_close() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let request_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        drive_to_native_wait(&mut owner);
        drive_to_committing(&mut owner, request_id, CANDIDATE_HOST);
        owner.dispatch(PresentationTransitionEvent::TerminalClose);
        let effects = owner.take_effects();
        let abort_pos = effects
            .iter()
            .position(|effect| matches!(effect, PresentationTransitionEffect::AbortNative { .. }));
        let close_pos = effects.iter().position(|effect| {
            matches!(
                effect,
                PresentationTransitionEffect::TerminalSessionClose { .. }
            )
        });
        assert!(abort_pos.is_some_and(|abort| close_pos.is_some_and(|close| abort < close)));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::DestroyHost {
                hwnd: CANDIDATE_HOST,
                ..
            }
        )));
        // Killing mutation: invert native_prepare_was_issued in terminal_close.
    }

    /// commit 済みで retire を待っている間に閉じても、巻き戻さず publish 済みで安定する。
    ///
    /// 置換を挟むと `RetiringCommitted` が同じことをするが、**直接ここへ来る経路**は
    /// `Committing` を一括で「native へ発行済み」と数えていたため、retire が積まれた後に
    /// abort を出し、publish 前の `current` で安定していた (Codex Q5)。
    #[test]
    fn closing_while_awaiting_retire_keeps_what_was_published() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::DetachedWindow);
        let committed_id = request(&mut owner, ViewerPresentation::Fullscreen, 0);
        drive_to_native_wait(&mut owner);
        drive_to_committing(&mut owner, committed_id, 0);
        owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id: committed_id,
            candidate_generation: GENERATION,
        });
        owner.take_effects();
        assert!(matches!(
            owner.state(),
            PresentationTransitionState::Committing {
                progress: CommittingProgress::AwaitingRetire,
                ..
            }
        ));

        owner.dispatch(PresentationTransitionEvent::TerminalClose);
        let effects = owner.take_effects();

        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, PresentationTransitionEffect::AbortNative { .. })),
            "retire が積まれている request へ abort を出している: {effects:?}"
        );
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                PresentationTransitionEffect::CloseDetachedSession { request_id, .. }
                    if *request_id == committed_id
            )),
            "退場側の session が閉じられていない: {effects:?}"
        );
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                PresentationTransitionEffect::DestroyHost {
                    hwnd: OUTGOING_HOST,
                    ..
                }
            )),
            "退場側の host が畳まれていない: {effects:?}"
        );
        assert_eq!(
            owner.state(),
            PresentationTransitionState::Stable {
                current: ViewerPresentation::Fullscreen
            },
            "publish 済みの表示ではなく退場側で安定している"
        );
    }

    /// commit 後 retire 前の置換は、abort ではなく retire の完了を待つ。
    ///
    /// abort を出すと同じ request へ `[Retire, Abort]` が並ぶ。native は retire を選ぶので
    /// abort は解決されず、それを待つ側が永久に止まる。`NativeCommitted` は所有権の
    /// 受け渡し点なので巻き戻さない (2026-08-29 レビュー R-17 の裁定、Codex Q2)。
    #[test]
    fn replacing_after_commit_waits_for_the_retire_instead_of_asking_for_a_rollback() {
        // 退場側が別ウィンドウの向きにする。こうすると retire 完了で畳む物が実際にある。
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::DetachedWindow);
        let committed_id = request(&mut owner, ViewerPresentation::Fullscreen, 0);
        drive_to_native_wait(&mut owner);
        drive_to_committing(&mut owner, committed_id, 0);
        owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id: committed_id,
            candidate_generation: GENERATION,
        });
        owner.take_effects();
        assert!(matches!(
            owner.state(),
            PresentationTransitionState::Committing {
                progress: CommittingProgress::AwaitingRetire,
                ..
            }
        ));

        // publish 済みなので、ここから見た「今の表示」は退場側ではなく target。
        assert_eq!(owner.current(), ViewerPresentation::Fullscreen);

        let successor_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        let effects = owner.take_effects();
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, PresentationTransitionEffect::AbortNative { .. })),
            "commit 済みの request へ abort を出している: {effects:?}"
        );
        assert!(
            matches!(
                owner.state(),
                PresentationTransitionState::Preparing {
                    request,
                    progress: PreparingProgress::RetiringCommitted { retiring, .. },
                } if retiring.id == committed_id && request.id == successor_id
            ),
            "retire 待ちになっていない: {:?}",
            owner.state()
        );

        // retire が終わったら、退場側を畳んでから後継を始める。
        owner.dispatch(PresentationTransitionEvent::NativeRetired {
            request_id: committed_id,
            candidate_generation: GENERATION,
        });
        let effects = owner.take_effects();
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                PresentationTransitionEffect::CloseDetachedSession { request_id, .. }
                    if *request_id == committed_id
            )),
            "退場側の session が閉じられていない: {effects:?}"
        );
        assert!(
            matches!(
                owner.state(),
                PresentationTransitionState::Preparing { request, .. }
                    if request.id == successor_id
            ),
            "retire 完了後に後継が始まらない: {:?}",
            owner.state()
        );
    }

    /// 退場中の host lease を要求した後継へ移譲し、retire 後に host claim を再検証する。
    ///
    /// `Detached(H) -> Fullscreen` の retire を待っている間に `Detached` を要求すると、
    /// まだ生きている `H` が要求時の target claim として渡ってくる。retire 完了時に
    /// 同じ lease なら old session/viewport は successor が所有するため Close/Destroy を出さない。
    /// request 時点の raw H は durable ではないので捨て、`AwaitingHost` から現在の claim を取る。
    #[test]
    fn a_successor_does_not_reuse_the_host_the_same_batch_destroys() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::DetachedWindow);
        let committed_id = request(&mut owner, ViewerPresentation::Fullscreen, 0);
        drive_to_native_wait(&mut owner);
        drive_to_committing(&mut owner, committed_id, 0);
        owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id: committed_id,
            candidate_generation: GENERATION,
        });
        owner.take_effects();

        // 退場中の窓がまだ生きているので、ready host には同じ hwnd が入ってくる。
        let successor_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            OUTGOING_HOST,
        );
        assert_eq!(
            detached_cleanup_counts(
                &owner.take_effects(),
                DetachedSessionLease {
                    window_id: OUTGOING_WINDOW,
                },
            ),
            (0, 0),
            "requesting an alias successor must not release the retiring lease early"
        );

        owner.dispatch(PresentationTransitionEvent::NativeRetired {
            request_id: committed_id,
            candidate_generation: GENERATION,
        });
        let effects = owner.take_effects();
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::CloseDetachedSession { .. }
                | PresentationTransitionEffect::DestroyHost { .. }
        )));

        assert!(
            matches!(
                owner.state(),
                PresentationTransitionState::Preparing {
                    request,
                    progress: PreparingProgress::AwaitingHost { lease },
                } if request.id == successor_id
                    && lease == DetachedSessionLease { window_id: OUTGOING_WINDOW }
            ),
            "破棄した窓を準備先にしている: {:?}",
            owner.state()
        );
        assert!(
            !effects.iter().any(|effect| matches!(
                effect,
                PresentationTransitionEffect::PrepareNative {
                    host: Some(host),
                    ..
                } if host.hwnd == OUTGOING_HOST
            )),
            "破棄した窓へ prepare を出している: {effects:?}"
        );

        let wrong = host_lease(CANDIDATE_WINDOW, CANDIDATE_HOST, 2)
            .host
            .unwrap();
        owner.dispatch(PresentationTransitionEvent::HostReady {
            request_id: successor_id,
            claim: wrong,
        });
        assert!(matches!(
            owner.state(),
            PresentationTransitionState::Preparing {
                progress: PreparingProgress::AwaitingHost { lease },
                ..
            } if lease.window_id == OUTGOING_WINDOW
        ));

        let reacquired = host_lease(OUTGOING_WINDOW, CANDIDATE_HOST, 2).host.unwrap();
        owner.dispatch(PresentationTransitionEvent::HostReady {
            request_id: successor_id,
            claim: reacquired,
        });
        owner.drive();
        assert!(matches!(
            owner.take_effects().as_slice(),
            [PresentationTransitionEffect::PrepareNative {
                host: Some(host),
                ..
            }] if *host == reacquired
        ));
    }

    /// commit 後に要求した後継は、退場側ではなく publish 済みの表示を起点にする。
    ///
    /// 起点が退場側のままだと、`target == current` の判定も host の始末先も狂う。
    /// ここでは「publish 済みの表示と同じ場所へ戻す」要求を出し、retire 完了で
    /// そのまま安定することを見る。
    #[test]
    fn a_successor_asked_for_after_commit_starts_from_what_was_published() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let committed_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        drive_to_native_wait(&mut owner);
        drive_to_committing(&mut owner, committed_id, CANDIDATE_HOST);
        owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id: committed_id,
            candidate_generation: GENERATION,
        });
        owner.take_effects();

        // publish 済みの表示と同じ target を要求する。
        let published = host_lease(CANDIDATE_WINDOW, CANDIDATE_HOST, 1);
        explicit_request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            Some(published),
            Some(published),
        );
        owner.take_effects();
        owner.dispatch(PresentationTransitionEvent::NativeRetired {
            request_id: committed_id,
            candidate_generation: GENERATION,
        });

        assert!(
            matches!(
                owner.state(),
                PresentationTransitionState::Stable {
                    current: ViewerPresentation::DetachedWindow
                }
            ),
            "publish 済みの表示を起点にしていれば、同じ場所への要求はそのまま安定する: {:?}",
            owner.state()
        );
    }

    /// 連打しても、native が処理している abort の identity は動かない。
    ///
    /// `Aborting` の `request` は「abort が終わったら始める後継」で、native はそれを
    /// 知らない。3 回目の要求でそれを「発行済み」と見なすと、後継 id で
    /// `aborted_request_id` を上書きし、native が返す元 request の terminal event が
    /// どの照合にも一致しなくなって永久に待つ (2026-08-29 レビュー R-01)。
    #[test]
    fn replacing_again_while_aborting_does_not_move_the_abort_that_native_is_running() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let issued_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        drive_to_native_wait(&mut owner);

        // 2 回目: ここで初めて abort が飛ぶ。
        let _second_id = request(&mut owner, ViewerPresentation::MainWindow, MAIN_HOST);
        let effects = owner.take_effects();
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                PresentationTransitionEffect::AbortNative { request_id, .. }
                    if *request_id == issued_id
            )),
            "abort は native へ渡した request に対して出る"
        );

        // 3 回目以降: 後継が入れ替わるだけ。abort は出し直さない。
        let third_id = request(&mut owner, ViewerPresentation::Fullscreen, 0);
        let fourth_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        let effects = owner.take_effects();
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, PresentationTransitionEffect::AbortNative { .. })),
            "native へ渡していない後継に対して abort を出している"
        );
        assert_ne!(third_id, fourth_id);

        assert!(
            matches!(
                owner.state(),
                PresentationTransitionState::Preparing {
                    request,
                    progress: PreparingProgress::Aborting { aborted, .. },
                } if aborted.id == issued_id && request.id == fourth_id
            ),
            "abort の identity が後継で上書きされた: {:?}",
            owner.state()
        );

        // 元 request の terminal event は、連打の後でも受理される。
        owner.dispatch(PresentationTransitionEvent::NativeAborted {
            request_id: issued_id,
        });
        assert!(
            matches!(
                owner.state(),
                PresentationTransitionState::Preparing {
                    request,
                    progress: PreparingProgress::AwaitingHost { .. }
                        | PreparingProgress::ReadyToPrepare { .. },
                } if request.id == fourth_id
            ),
            "元 request の abort 完了で最後の後継へ進めていない: {:?}",
            owner.state()
        );
    }

    /// abort が失敗で終わっても、後継はそこで止まらない。
    ///
    /// `NativeFailed` の汎用分岐は state が持つ request と照合する。`Aborting` でそれは
    /// **後継**なので、native が中止した側について失敗を返すと照合が外れ、catch-all に
    /// 落ちて `NativeAborted` を永久に待つ。abort の終わり方が成功か失敗かは、後継を
    /// 始めてよいかどうかを変えない。
    #[test]
    fn an_abort_that_ends_in_failure_still_releases_the_successor() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let issued_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        drive_to_native_wait(&mut owner);
        let successor_id = request(&mut owner, ViewerPresentation::MainWindow, MAIN_HOST);
        owner.take_effects();

        owner.dispatch(PresentationTransitionEvent::NativeFailed {
            request_id: issued_id,
        });

        assert!(
            matches!(
                owner.state(),
                PresentationTransitionState::Preparing {
                    request,
                    progress: PreparingProgress::AwaitingHost { .. }
                        | PreparingProgress::ReadyToPrepare { .. },
                } if request.id == successor_id
            ),
            "abort が失敗で終わったら後継が始まらない: {:?}",
            owner.state()
        );
    }

    /// `Aborting` 中に閉じても、abort を二重に出さず、identity も混ぜない。
    ///
    /// 素朴に書くと `request_id` は後継 (native が知らない)、`candidate_hwnd` は中止した
    /// 側、という食い違った command になる。中止した側が作りかけた host は通常
    /// `NativeAborted` の受理で壊すが、terminal close はそこへ到達しない。
    #[test]
    fn closing_while_aborting_neither_repeats_the_abort_nor_leaves_its_host() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let issued_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        drive_to_native_wait(&mut owner);
        request(&mut owner, ViewerPresentation::MainWindow, MAIN_HOST);
        owner.take_effects();

        owner.dispatch(PresentationTransitionEvent::TerminalClose);
        let effects = owner.take_effects();
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, PresentationTransitionEffect::AbortNative { .. })),
            "既に飛んでいる abort をもう一度出している: {effects:?}"
        );
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                PresentationTransitionEffect::DestroyHost {
                    request_id,
                    hwnd: CANDIDATE_HOST,
                    ..
                } if *request_id == issued_id
            )),
            "中止した側の host が畳まれずに残る: {effects:?}"
        );
        assert!(matches!(
            owner.state(),
            PresentationTransitionState::Stable { .. }
        ));
    }

    #[test]
    fn non_alias_detached_successor_prepares_new_claim_and_retires_only_old_lease() {
        let old = host_lease(OUTGOING_WINDOW, OUTGOING_HOST, 1);
        let new = host_lease(CANDIDATE_WINDOW, CANDIDATE_HOST, 1);
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);

        let first_id = explicit_request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            None,
            Some(old),
        );
        drive_to_native_wait(&mut owner);
        drive_to_committing(&mut owner, first_id, OUTGOING_HOST);
        owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id: first_id,
            candidate_generation: GENERATION,
        });
        owner.take_effects();

        let successor_id = explicit_request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            Some(old),
            Some(new),
        );
        let request_effects = owner.take_effects();
        assert_eq!(
            detached_cleanup_counts(&request_effects, old.session),
            (0, 0)
        );
        assert_eq!(
            detached_cleanup_counts(&request_effects, new.session),
            (0, 0)
        );
        owner.dispatch(PresentationTransitionEvent::NativeRetired {
            request_id: first_id,
            candidate_generation: GENERATION,
        });
        assert!(matches!(
            owner.state(),
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::AwaitingHost { lease },
            } if request.id == successor_id && lease == new.session
        ));
        assert_eq!(
            detached_cleanup_counts(&owner.take_effects(), old.session),
            (0, 0),
            "the published old detached host remains live until the new presenter commits"
        );

        let refreshed_new = host_lease(CANDIDATE_WINDOW, CANDIDATE_HOST, 2)
            .host
            .unwrap();
        owner.dispatch(PresentationTransitionEvent::HostReady {
            request_id: successor_id,
            claim: refreshed_new,
        });
        owner.drive();
        let effects = owner.take_effects();
        assert!(matches!(
            effects.as_slice(),
            [PresentationTransitionEffect::PrepareNative {
                host: Some(host),
                ..
            }] if *host == refreshed_new
        ));
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::PrepareNative {
                host: Some(host),
                ..
            } if host.lease == old.session
        )));

        drive_to_committing(&mut owner, successor_id, CANDIDATE_HOST);
        owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id: successor_id,
            candidate_generation: GENERATION,
        });
        owner.take_effects();
        owner.dispatch(PresentationTransitionEvent::NativeRetired {
            request_id: successor_id,
            candidate_generation: GENERATION,
        });
        let effects = owner.take_effects();
        assert_eq!(detached_cleanup_counts(&effects, old.session), (1, 1));
        assert_eq!(detached_cleanup_counts(&effects, new.session), (0, 0));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::DestroyHost {
                lease,
                hwnd: OUTGOING_HOST,
                ..
            } if *lease == old.session
        )));
        assert_eq!(
            owner.state(),
            PresentationTransitionState::Stable {
                current: ViewerPresentation::DetachedWindow,
            }
        );
    }

    #[test]
    fn transferred_native_failure_releases_once_while_keep_live_failure_releases_nothing() {
        let lease = DetachedSessionLease {
            window_id: OUTGOING_WINDOW,
        };
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::DetachedWindow);
        let request_id = begin_transferred_successor(&mut owner);
        reacquire_transferred_host(&mut owner, request_id);

        owner.dispatch(PresentationTransitionEvent::NativeFailed { request_id });
        let effects = owner.take_effects();
        assert_eq!(detached_cleanup_counts(&effects, lease), (1, 1));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::DestroyHost {
                lease: destroyed,
                hwnd: CANDIDATE_HOST,
                ..
            } if *destroyed == lease
        )));
        assert_eq!(
            owner.state(),
            PresentationTransitionState::Stable {
                current: ViewerPresentation::Fullscreen,
            }
        );
        owner.dispatch(PresentationTransitionEvent::NativeFailed { request_id });
        owner.dispatch(PresentationTransitionEvent::TerminalClose);
        assert_eq!(
            detached_cleanup_counts(&owner.take_effects(), lease),
            (0, 0),
            "late terminal events must not release a transferred lease twice"
        );

        let mut keep_live = PresentationTransitionOwner::stable(ViewerPresentation::DetachedWindow);
        let live = host_lease(OUTGOING_WINDOW, OUTGOING_HOST, 1);
        let keep_live_id = explicit_request(
            &mut keep_live,
            ViewerPresentation::DetachedWindow,
            Some(live),
            Some(live),
        );
        drive_to_native_wait(&mut keep_live);
        keep_live.dispatch(PresentationTransitionEvent::NativeFailed {
            request_id: keep_live_id,
        });
        assert_eq!(
            detached_cleanup_counts(&keep_live.take_effects(), live.session),
            (0, 0),
            "a direct KeepLive failure must preserve the current detached session"
        );
    }

    #[test]
    fn transferred_lease_survives_alias_replacement_then_releases_once_on_non_alias_replacement() {
        let old = host_lease(OUTGOING_WINDOW, OUTGOING_HOST, 1);
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::DetachedWindow);
        begin_transferred_successor(&mut owner);

        explicit_request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            None,
            Some(old),
        );
        assert_eq!(
            detached_cleanup_counts(&owner.take_effects(), old.session),
            (0, 0)
        );
        assert!(matches!(
            owner.state(),
            PresentationTransitionState::Preparing {
                request: PresentationRequest {
                    target_detached: DetachedTargetLease::Transferred(_),
                    ..
                },
                progress: PreparingProgress::AwaitingHost { .. },
            }
        ));

        explicit_request(&mut owner, ViewerPresentation::MainWindow, None, None);
        let effects = owner.take_effects();
        assert_eq!(detached_cleanup_counts(&effects, old.session), (1, 1));

        explicit_request(&mut owner, ViewerPresentation::Fullscreen, None, None);
        owner.dispatch(PresentationTransitionEvent::TerminalClose);
        assert_eq!(
            detached_cleanup_counts(&owner.take_effects(), old.session),
            (0, 0)
        );
    }

    #[test]
    fn transferred_abort_terminal_events_release_once() {
        for abort_failed in [false, true] {
            let old = host_lease(OUTGOING_WINDOW, OUTGOING_HOST, 1);
            let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::DetachedWindow);
            let transferred_id = begin_transferred_successor(&mut owner);
            reacquire_transferred_host(&mut owner, transferred_id);

            explicit_request(&mut owner, ViewerPresentation::MainWindow, None, None);
            let effects = owner.take_effects();
            assert!(matches!(
                effects.as_slice(),
                [PresentationTransitionEffect::AbortNative {
                    request_id,
                    ..
                }] if *request_id == transferred_id
            ));
            assert_eq!(detached_cleanup_counts(&effects, old.session), (0, 0));

            if abort_failed {
                owner.dispatch(PresentationTransitionEvent::NativeFailed {
                    request_id: transferred_id,
                });
            } else {
                owner.dispatch(PresentationTransitionEvent::NativeAborted {
                    request_id: transferred_id,
                });
            }
            let terminal_effects = owner.take_effects();
            assert_eq!(
                detached_cleanup_counts(&terminal_effects, old.session),
                (1, 1)
            );
            assert!(terminal_effects.iter().any(|effect| matches!(
                effect,
                PresentationTransitionEffect::DestroyHost {
                    lease,
                    hwnd: CANDIDATE_HOST,
                    ..
                } if *lease == old.session
            )));

            owner.dispatch(PresentationTransitionEvent::NativeAborted {
                request_id: transferred_id,
            });
            owner.dispatch(PresentationTransitionEvent::NativeFailed {
                request_id: transferred_id,
            });
            assert_eq!(
                detached_cleanup_counts(&owner.take_effects(), old.session),
                (0, 0)
            );
        }
    }

    #[test]
    fn terminal_close_deduplicates_transferred_and_alias_successor_ownership() {
        let old = host_lease(OUTGOING_WINDOW, OUTGOING_HOST, 1);
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::DetachedWindow);
        let transferred_id = begin_transferred_successor(&mut owner);
        reacquire_transferred_host(&mut owner, transferred_id);
        explicit_request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            None,
            Some(old),
        );
        assert_eq!(
            detached_cleanup_counts(&owner.take_effects(), old.session),
            (0, 0),
            "the alias successor must not release transferred ownership before terminal close"
        );

        owner.dispatch(PresentationTransitionEvent::TerminalClose);
        let terminal_effects = owner.take_effects();
        assert_eq!(
            detached_cleanup_counts(&terminal_effects, old.session),
            (1, 1),
            "aborted transferred ownership and its alias successor must be released once"
        );
        assert!(terminal_effects.iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::DestroyHost {
                lease,
                hwnd: CANDIDATE_HOST,
                ..
            } if *lease == old.session
        )));
        owner.dispatch(PresentationTransitionEvent::TerminalClose);
        assert_eq!(
            detached_cleanup_counts(&owner.take_effects(), old.session),
            (0, 0)
        );
    }

    #[test]
    fn stale_host_unavailable_cannot_rewind_a_reacquired_claim() {
        let initial = host_lease(CANDIDATE_WINDOW, CANDIDATE_HOST, 1);
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let request_id = explicit_request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            None,
            Some(initial),
        );
        drive_to_native_wait(&mut owner);
        let old_claim = initial.host.unwrap();
        let wrong_claim = DetachedHostClaim {
            incarnation: old_claim.incarnation + 1,
            ..old_claim
        };
        let before = owner.state();
        owner.dispatch(PresentationTransitionEvent::HostUnavailable {
            request_id,
            claim: wrong_claim,
        });
        assert_eq!(owner.state(), before);
        assert!(owner.take_effects().is_empty());

        owner.dispatch(PresentationTransitionEvent::HostUnavailable {
            request_id,
            claim: old_claim,
        });
        assert!(matches!(
            owner.state(),
            PresentationTransitionState::Preparing {
                progress: PreparingProgress::AwaitingHost { lease },
                ..
            } if lease == old_claim.lease
        ));

        let new_claim = DetachedHostClaim {
            incarnation: old_claim.incarnation + 1,
            hwnd: OUTGOING_HOST,
            ..old_claim
        };
        owner.dispatch(PresentationTransitionEvent::HostReady {
            request_id,
            claim: new_claim,
        });
        owner.drive();
        owner.take_effects();
        let before_stale_rejection = owner.state();
        owner.dispatch(PresentationTransitionEvent::HostUnavailable {
            request_id,
            claim: old_claim,
        });
        assert_eq!(owner.state(), before_stale_rejection);
        assert!(owner.take_effects().is_empty());
        assert!(matches!(
            owner.state(),
            PresentationTransitionState::Preparing {
                progress: PreparingProgress::AwaitingNative {
                    host: Some(host),
                },
                ..
            } if host == new_claim
        ));
    }

    #[test]
    fn replacing_waiting_successors_destroys_only_the_discarded_candidate() {
        let old = host_lease(OUTGOING_WINDOW, OUTGOING_HOST, 1);
        let discarded = host_lease(CANDIDATE_WINDOW, CANDIDATE_HOST, 1);

        let mut retiring = PresentationTransitionOwner::stable(ViewerPresentation::DetachedWindow);
        let retiring_id = explicit_request(
            &mut retiring,
            ViewerPresentation::Fullscreen,
            Some(old),
            None,
        );
        drive_to_native_wait(&mut retiring);
        drive_to_committing(&mut retiring, retiring_id, 0);
        retiring.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id: retiring_id,
            candidate_generation: GENERATION,
        });
        retiring.take_effects();
        explicit_request(
            &mut retiring,
            ViewerPresentation::DetachedWindow,
            None,
            Some(discarded),
        );
        retiring.take_effects();
        explicit_request(&mut retiring, ViewerPresentation::MainWindow, None, None);
        let effects = retiring.take_effects();
        assert_eq!(detached_cleanup_counts(&effects, discarded.session), (0, 1));
        assert_eq!(detached_cleanup_counts(&effects, old.session), (0, 0));

        let mut aborting = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let aborted_id = explicit_request(
            &mut aborting,
            ViewerPresentation::DetachedWindow,
            None,
            Some(old),
        );
        drive_to_native_wait(&mut aborting);
        explicit_request(
            &mut aborting,
            ViewerPresentation::DetachedWindow,
            None,
            Some(discarded),
        );
        aborting.take_effects();
        explicit_request(&mut aborting, ViewerPresentation::MainWindow, None, None);
        let effects = aborting.take_effects();
        assert_eq!(detached_cleanup_counts(&effects, discarded.session), (0, 1));
        assert_eq!(detached_cleanup_counts(&effects, old.session), (0, 0));
        assert!(matches!(
            aborting.state(),
            PresentationTransitionState::Preparing {
                progress: PreparingProgress::Aborting { aborted, .. },
                ..
            } if aborted.id == aborted_id
        ));

        let mut protected_retiring =
            PresentationTransitionOwner::stable(ViewerPresentation::DetachedWindow);
        let protected_retiring_id = explicit_request(
            &mut protected_retiring,
            ViewerPresentation::Fullscreen,
            Some(old),
            None,
        );
        drive_to_native_wait(&mut protected_retiring);
        drive_to_committing(&mut protected_retiring, protected_retiring_id, 0);
        protected_retiring.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id: protected_retiring_id,
            candidate_generation: GENERATION,
        });
        protected_retiring.take_effects();
        explicit_request(
            &mut protected_retiring,
            ViewerPresentation::DetachedWindow,
            None,
            Some(old),
        );
        protected_retiring.take_effects();
        explicit_request(
            &mut protected_retiring,
            ViewerPresentation::MainWindow,
            None,
            None,
        );
        assert_eq!(
            detached_cleanup_counts(&protected_retiring.take_effects(), old.session),
            (0, 0),
            "a pending alias must stay protected by the in-flight retiring owner"
        );
        protected_retiring.dispatch(PresentationTransitionEvent::TerminalClose);
        assert_eq!(
            detached_cleanup_counts(&protected_retiring.take_effects(), old.session),
            (1, 1)
        );

        let mut protected_aborting =
            PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let protected_aborted_id = explicit_request(
            &mut protected_aborting,
            ViewerPresentation::DetachedWindow,
            None,
            Some(old),
        );
        drive_to_native_wait(&mut protected_aborting);
        explicit_request(
            &mut protected_aborting,
            ViewerPresentation::DetachedWindow,
            None,
            Some(old),
        );
        protected_aborting.take_effects();
        explicit_request(
            &mut protected_aborting,
            ViewerPresentation::MainWindow,
            None,
            None,
        );
        assert_eq!(
            detached_cleanup_counts(&protected_aborting.take_effects(), old.session),
            (0, 0),
            "a pending alias must stay protected by the in-flight aborted owner"
        );
        assert!(matches!(
            protected_aborting.state(),
            PresentationTransitionState::Preparing {
                progress: PreparingProgress::Aborting { aborted, .. },
                ..
            } if aborted.id == protected_aborted_id
        ));
        protected_aborting.dispatch(PresentationTransitionEvent::TerminalClose);
        assert_eq!(
            detached_cleanup_counts(&protected_aborting.take_effects(), old.session),
            (0, 1)
        );
    }

    #[test]
    fn retire_failure_keeps_published_target_and_releases_outgoing_lease() {
        let old = host_lease(OUTGOING_WINDOW, OUTGOING_HOST, 1);
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::DetachedWindow);
        let request_id =
            explicit_request(&mut owner, ViewerPresentation::Fullscreen, Some(old), None);
        drive_to_native_wait(&mut owner);
        drive_to_committing(&mut owner, request_id, 0);
        owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id,
            candidate_generation: GENERATION,
        });
        owner.take_effects();

        owner.dispatch(PresentationTransitionEvent::NativeFailed { request_id });
        let effects = owner.take_effects();
        assert_eq!(detached_cleanup_counts(&effects, old.session), (1, 1));
        assert_eq!(
            owner.state(),
            PresentationTransitionState::Stable {
                current: ViewerPresentation::Fullscreen,
            },
            "retire failure is terminal completion after the target was published"
        );

        let incoming = host_lease(CANDIDATE_WINDOW, CANDIDATE_HOST, 1);
        let mut incoming_owner =
            PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let incoming_id = explicit_request(
            &mut incoming_owner,
            ViewerPresentation::DetachedWindow,
            None,
            Some(incoming),
        );
        drive_to_native_wait(&mut incoming_owner);
        drive_to_committing(&mut incoming_owner, incoming_id, CANDIDATE_HOST);
        incoming_owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id: incoming_id,
            candidate_generation: GENERATION,
        });
        incoming_owner.take_effects();
        incoming_owner.dispatch(PresentationTransitionEvent::NativeFailed {
            request_id: incoming_id,
        });
        assert_eq!(
            detached_cleanup_counts(&incoming_owner.take_effects(), incoming.session),
            (0, 0),
            "a committed incoming detached lease is live, not a failed candidate"
        );
        assert_eq!(
            incoming_owner.state(),
            PresentationTransitionState::Stable {
                current: ViewerPresentation::DetachedWindow,
            }
        );
    }

    #[test]
    fn stale_ready_after_newer_request_cannot_replace_new_preparing_state() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let first_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        drive_to_native_wait(&mut owner);
        let second_id = request(&mut owner, ViewerPresentation::MainWindow, 0);
        owner.take_effects();
        owner.dispatch(PresentationTransitionEvent::NativeAborted {
            request_id: first_id,
        });
        owner.drive();
        owner.take_effects();
        let before = owner.state();
        owner.dispatch(PresentationTransitionEvent::NativeReady {
            request_id: first_id,
            candidate: candidate(0),
        });
        assert_eq!(owner.state(), before);
        assert_eq!(owner.request_id(), Some(second_id));
        assert!(owner.take_effects().is_empty());
        // Killing mutation: remove request.id == request_id from NativeReady.
    }

    #[test]
    fn stale_commit_after_newer_request_cannot_apply_old_presentation() {
        let mut owner = PresentationTransitionOwner::stable(ViewerPresentation::Fullscreen);
        let first_id = request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
        );
        drive_to_native_wait(&mut owner);
        drive_to_committing(&mut owner, first_id, CANDIDATE_HOST);
        let second_id = request(&mut owner, ViewerPresentation::MainWindow, 0);
        owner.take_effects();
        owner.dispatch(PresentationTransitionEvent::NativeAborted {
            request_id: first_id,
        });
        owner.drive();
        owner.take_effects();
        drive_to_committing(&mut owner, second_id, 0);
        let before = owner.state();
        owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id: first_id,
            candidate_generation: GENERATION,
        });
        assert_eq!(owner.state(), before);
        assert!(owner.take_effects().is_empty());
        owner.dispatch(PresentationTransitionEvent::NativeCommitted {
            request_id: second_id,
            candidate_generation: GENERATION,
        });
        assert!(owner.take_effects().iter().any(|effect| matches!(
            effect,
            PresentationTransitionEffect::ApplyPresentation { .. }
        )));
        // Killing mutation: remove request.id == request_id from NativeCommitted.
    }
}
