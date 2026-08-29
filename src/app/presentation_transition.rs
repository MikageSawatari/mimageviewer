use super::ViewerPresentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetachedHostDisposition {
    None,
    KeepLive { hwnd: u64 },
    RetireOutgoing { hwnd: u64 },
}

impl DetachedHostDisposition {
    fn for_transition(
        current: ViewerPresentation,
        target: ViewerPresentation,
        current_host_hwnd: u64,
    ) -> Self {
        match (current, target) {
            (ViewerPresentation::DetachedWindow, ViewerPresentation::DetachedWindow) => {
                Self::KeepLive {
                    hwnd: current_host_hwnd,
                }
            }
            (ViewerPresentation::DetachedWindow, _) => Self::RetireOutgoing {
                hwnd: current_host_hwnd,
            },
            _ => Self::None,
        }
    }

    fn current_hwnd(self) -> u64 {
        match self {
            Self::None => 0,
            Self::KeepLive { hwnd } | Self::RetireOutgoing { hwnd } => hwnd,
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
    pub(crate) detached_host: DetachedHostDisposition,
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
    AwaitingHost,
    ReadyToPrepare {
        host_hwnd: u64,
    },
    AwaitingNative {
        host_hwnd: u64,
    },
    Aborting {
        aborted_request_id: u64,
        aborted_target: ViewerPresentation,
        aborted_candidate_hwnd: u64,
        aborted_candidate_host_hwnd: u64,
        next_host_hwnd: u64,
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
        next_host_hwnd: u64,
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
        hwnd: u64,
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
        host_hwnd: u64,
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
        hwnd: u64,
    },
    ApplyPresentation {
        request: PresentationRequest,
        candidate_generation: u64,
    },
    CloseDetachedSession {
        request_id: u64,
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
                progress: PreparingProgress::AwaitingHost,
            }
        )
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
        current_detached_host_hwnd: u64,
        ready_host_hwnd: u64,
    ) -> u64 {
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        let id = self.next_request_id;
        let current = self.current();
        let (state, effects) = reduce_presentation_request(
            self.state,
            PresentationRequest {
                id,
                current,
                target,
                activate,
                announce_main_hint,
                outgoing_presenter_hwnd,
                detached_host: DetachedHostDisposition::for_transition(
                    current,
                    target,
                    current_detached_host_hwnd,
                ),
            },
            ready_host_hwnd,
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

fn initial_progress(request: PresentationRequest, ready_host_hwnd: u64) -> PreparingProgress {
    if matches!(request.target, ViewerPresentation::DetachedWindow) && ready_host_hwnd == 0 {
        PreparingProgress::AwaitingHost
    } else {
        PreparingProgress::ReadyToPrepare {
            host_hwnd: ready_host_hwnd,
        }
    }
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

fn transition_host_hwnd(state: PresentationTransitionState) -> u64 {
    match state {
        PresentationTransitionState::Preparing {
            progress:
                PreparingProgress::ReadyToPrepare { host_hwnd }
                | PreparingProgress::AwaitingNative { host_hwnd },
            ..
        } => host_hwnd,
        PresentationTransitionState::Ready { candidate, .. }
        | PresentationTransitionState::Committing { candidate, .. } => candidate.host_hwnd,
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
    ready_host_hwnd: u64,
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
                    next_host_hwnd: ready_host_hwnd,
                },
            },
            effects,
        );
    }
    // 更に置換されても、native が走らせている retire には触らない。後継だけ差し替える。
    if let PresentationTransitionState::Preparing {
        progress:
            PreparingProgress::RetiringCommitted {
                retiring,
                retiring_generation,
                ..
            },
        ..
    } = state
    {
        return (
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::RetiringCommitted {
                    retiring,
                    retiring_generation,
                    next_host_hwnd: ready_host_hwnd,
                },
            },
            effects,
        );
    }
    // 既に abort が飛んでいるなら、native が持っているのは「中止した側」であって、
    // ここに居る後継ではない。差し替えてよいのは後継だけで、in-flight な abort の
    // identity には触らない。二重の `AbortNative` も出さない (R-01)。
    if let PresentationTransitionState::Preparing {
        progress:
            PreparingProgress::Aborting {
                aborted_request_id,
                aborted_target,
                aborted_candidate_hwnd,
                aborted_candidate_host_hwnd,
                ..
            },
        ..
    } = state
    {
        return (
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::Aborting {
                    aborted_request_id,
                    aborted_target,
                    aborted_candidate_hwnd,
                    aborted_candidate_host_hwnd,
                    next_host_hwnd: ready_host_hwnd,
                },
            },
            effects,
        );
    }
    let old_request = request_from_state(state).unwrap();
    let aborted_candidate_host_hwnd = (old_request.current != ViewerPresentation::DetachedWindow
        && old_request.target == ViewerPresentation::DetachedWindow)
        .then(|| transition_host_hwnd(state))
        .unwrap_or(0);
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
                    aborted_request_id: old_request.id,
                    aborted_target: old_request.target,
                    aborted_candidate_hwnd,
                    aborted_candidate_host_hwnd,
                    next_host_hwnd: ready_host_hwnd,
                },
            },
            effects,
        );
    }
    reduce_unprepared_replacement(
        old_request,
        request,
        aborted_candidate_host_hwnd,
        ready_host_hwnd,
        effects,
    )
}

fn reduce_unprepared_replacement(
    old_request: PresentationRequest,
    request: PresentationRequest,
    aborted_candidate_host_hwnd: u64,
    ready_host_hwnd: u64,
    mut effects: Vec<PresentationTransitionEffect>,
) -> (
    PresentationTransitionState,
    Vec<PresentationTransitionEffect>,
) {
    if aborted_candidate_host_hwnd != 0 && request.target != ViewerPresentation::DetachedWindow {
        effects.push(PresentationTransitionEffect::DestroyHost {
            request_id: old_request.id,
            target: old_request.target,
            hwnd: aborted_candidate_host_hwnd,
        });
    }
    if request.target == request.current {
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
                request,
                progress: initial_progress(request, ready_host_hwnd),
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
                progress: PreparingProgress::ReadyToPrepare { host_hwnd },
            },
            PresentationTransitionEvent::Drive,
        ) => {
            effects.push(PresentationTransitionEffect::PrepareNative { request, host_hwnd });
            (
                PresentationTransitionState::Preparing {
                    request,
                    progress: PreparingProgress::AwaitingNative { host_hwnd },
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
                request,
                progress: PreparingProgress::AwaitingHost,
            },
            PresentationTransitionEvent::HostReady { request_id, hwnd },
        ) if request.id == request_id && hwnd != 0 => (
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::ReadyToPrepare { host_hwnd: hwnd },
            },
            effects,
        ),
        (
            PresentationTransitionState::Preparing {
                request,
                progress: PreparingProgress::AwaitingNative { host_hwnd },
            },
            PresentationTransitionEvent::NativeReady {
                request_id,
                candidate,
            },
        ) if request.id == request_id
            && (request.target != ViewerPresentation::DetachedWindow
                || candidate.host_hwnd == host_hwnd) =>
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
        pair => reduce_completion_transition(pair.0, pair.1, effects),
    }
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
            if let DetachedHostDisposition::RetireOutgoing { hwnd } = request.detached_host {
                effects.push(PresentationTransitionEffect::CloseDetachedSession {
                    request_id: request.id,
                });
                effects.push(PresentationTransitionEffect::DestroyHost {
                    request_id: request.id,
                    target: request.target,
                    hwnd,
                });
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
                        next_host_hwnd,
                    },
            },
            PresentationTransitionEvent::NativeRetired {
                request_id,
                candidate_generation,
            },
        ) if retiring.id == request_id && retiring_generation == candidate_generation => {
            finish_retired_then_start(retiring, request, next_host_hwnd, effects)
        }
        // retire が失敗で終わっても、commit 済みの表示が残らないことに変わりはない。
        // ここで拾わないと後継が永久に始まらない (`Aborting` の同型)。
        (
            PresentationTransitionState::Preparing {
                request,
                progress:
                    PreparingProgress::RetiringCommitted {
                        retiring,
                        next_host_hwnd,
                        ..
                    },
            },
            PresentationTransitionEvent::NativeFailed { request_id },
        ) if retiring.id == request_id => {
            finish_retired_then_start(retiring, request, next_host_hwnd, effects)
        }
        pair => reduce_abort_transition(pair.0, pair.1, effects),
    }
}

/// commit 済みの retire が終わったので、控えていた要求を始める。
///
/// 畳む物は `Committing` の retire 完了と同じ。違うのは、そのまま `Stable` にせず
/// 後継へ進むこと。
fn finish_retired_then_start(
    retiring: PresentationRequest,
    successor: PresentationRequest,
    next_host_hwnd: u64,
    mut effects: Vec<PresentationTransitionEffect>,
) -> (
    PresentationTransitionState,
    Vec<PresentationTransitionEffect>,
) {
    if let DetachedHostDisposition::RetireOutgoing { hwnd } = retiring.detached_host {
        effects.push(PresentationTransitionEffect::CloseDetachedSession {
            request_id: retiring.id,
        });
        effects.push(PresentationTransitionEffect::DestroyHost {
            request_id: retiring.id,
            target: retiring.target,
            hwnd,
        });
    }
    if successor.target == successor.current {
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
            request: successor,
            progress: initial_progress(successor, next_host_hwnd),
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
        if let DetachedHostDisposition::RetireOutgoing { hwnd } = request.detached_host {
            effects.push(PresentationTransitionEffect::CloseDetachedSession {
                request_id: request.id,
            });
            effects.push(PresentationTransitionEffect::DestroyHost {
                request_id: request.id,
                target: request.target,
                hwnd,
            });
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
                progress:
                    PreparingProgress::Aborting {
                        aborted_request_id,
                        aborted_target,
                        aborted_candidate_host_hwnd,
                        next_host_hwnd,
                        ..
                    },
            },
            // native が abort を「失敗」で終えた場合も、その request が終わったこと
            // に変わりはない。ここで拾わないと、下の汎用 `NativeFailed` 分岐が後継の
            // id と照合して外し、catch-all に落ちて永久に待つ (R-01 と同じ取り違え。
            // Codex の Q5 指摘で発覚)。
            PresentationTransitionEvent::NativeAborted { request_id }
            | PresentationTransitionEvent::NativeFailed { request_id },
        ) if aborted_request_id == request_id => {
            if aborted_candidate_host_hwnd != 0
                && request.target != ViewerPresentation::DetachedWindow
            {
                effects.push(PresentationTransitionEffect::DestroyHost {
                    request_id,
                    target: aborted_target,
                    hwnd: aborted_candidate_host_hwnd,
                });
            }
            if request.target == request.current {
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
                        request,
                        progress: initial_progress(request, next_host_hwnd),
                    },
                    effects,
                )
            }
        }
        (active_state, PresentationTransitionEvent::NativeFailed { request_id })
            if request_from_state(active_state).is_some_and(|request| request.id == request_id) =>
        {
            let request = request_from_state(active_state).unwrap();
            if request.current != ViewerPresentation::DetachedWindow
                && request.target == ViewerPresentation::DetachedWindow
            {
                effects.push(PresentationTransitionEffect::DestroyHost {
                    request_id,
                    target: request.target,
                    hwnd: transition_host_hwnd(active_state),
                });
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
    // commit 済みで retire を待っている間に閉じた場合。retire は既に走っているので
    // abort は出さない — 出すと同じ request へ `[Retire, Abort]` を作る。画面は publish
    // 済みの target へ移っているので、退場側ではなくそこで安定させる。退場側の session と
    // host は `NativeRetired` の受理で畳むはずだったが、terminal close はそこへ到達しない。
    //
    // 置換を挟んだ場合は `RetiringCommitted` が同じことをする。**直接ここへ来る経路が
    // 抜けていた** (2026-08-29 レビュー、Codex Q5)。
    if let PresentationTransitionState::Committing {
        request,
        progress: CommittingProgress::AwaitingRetire,
        ..
    } = state
    {
        if let DetachedHostDisposition::RetireOutgoing { hwnd } = request.detached_host {
            effects.push(PresentationTransitionEffect::CloseDetachedSession {
                request_id: request.id,
            });
            effects.push(PresentationTransitionEffect::DestroyHost {
                request_id: request.id,
                target: request.target,
                hwnd,
            });
        }
        effects.push(PresentationTransitionEffect::TerminalSessionClose {
            request_id: request.id,
            target: request.target,
        });
        effects.push(PresentationTransitionEffect::SetZOrderRecoveryPermit(true));
        return (
            PresentationTransitionState::Stable {
                current: request.target,
            },
            effects,
        );
    }
    let request = request_from_state(state);
    let request_id = request.map_or(0, |request| request.id);
    if native_prepare_was_issued(state) {
        effects.push(PresentationTransitionEffect::AbortNative {
            request_id,
            candidate_hwnd: candidate_hwnd(state),
        });
    }
    // `Aborting` では abort が既に飛んでいるので二度は出さない。ここで出すと
    // `request_id` は後継 (native が知らない) なのに `candidate_hwnd` は中止した側、
    // という食い違った command になる (R-01 と同じ取り違え)。
    // 通常なら中止した側の host は `NativeAborted` の受理で壊すが、terminal close は
    // そこへ到達しないため、作りかけの host をここで畳む。
    if let PresentationTransitionState::Preparing {
        progress:
            PreparingProgress::Aborting {
                aborted_request_id,
                aborted_target,
                aborted_candidate_host_hwnd,
                ..
            },
        ..
    } = state
        && aborted_candidate_host_hwnd != 0
    {
        effects.push(PresentationTransitionEffect::DestroyHost {
            request_id: aborted_request_id,
            target: aborted_target,
            hwnd: aborted_candidate_host_hwnd,
        });
    }
    // retire を待っている間に閉じた場合も同じ。abort は出さない (retire が走っている)。
    // 退場側の session と host は `NativeRetired` の受理で畳むはずだったので、ここで畳む。
    if let PresentationTransitionState::Preparing {
        progress: PreparingProgress::RetiringCommitted { retiring, .. },
        ..
    } = state
        && let DetachedHostDisposition::RetireOutgoing { hwnd } = retiring.detached_host
    {
        effects.push(PresentationTransitionEffect::CloseDetachedSession {
            request_id: retiring.id,
        });
        effects.push(PresentationTransitionEffect::DestroyHost {
            request_id: retiring.id,
            target: retiring.target,
            hwnd,
        });
    }
    let current = request.map_or_else(
        || match state {
            PresentationTransitionState::Stable { current } => current,
            _ => unreachable!(),
        },
        |request| request.current,
    );
    let target = request.map_or(current, |request| request.target);
    let detached_host_hwnd = if current == ViewerPresentation::DetachedWindow {
        request.map_or(0, |request| request.detached_host.current_hwnd())
    } else {
        transition_host_hwnd(state)
    };
    if current == ViewerPresentation::DetachedWindow || target == ViewerPresentation::DetachedWindow
    {
        effects.push(PresentationTransitionEffect::DestroyHost {
            request_id,
            target,
            hwnd: detached_host_hwnd,
        });
    }
    effects.push(PresentationTransitionEffect::TerminalSessionClose { request_id, target });
    effects.push(PresentationTransitionEffect::SetZOrderRecoveryPermit(true));
    (PresentationTransitionState::Stable { current }, effects)
}

fn reduce_presentation_request(
    state: PresentationTransitionState,
    request: PresentationRequest,
    ready_host_hwnd: u64,
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
                progress: initial_progress(request, ready_host_hwnd),
            },
            effects,
        );
    }
    reduce_replaced_request(state, request, ready_host_hwnd, effects)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTGOING_PRESENTER: u64 = 0x101;
    const OUTGOING_HOST: u64 = 0x202;
    const CANDIDATE_PRESENTER: u64 = 0x303;
    const CANDIDATE_HOST: u64 = 0x404;
    const MAIN_HOST: u64 = 0x505;
    const GENERATION: u64 = 7;

    fn request(
        owner: &mut PresentationTransitionOwner,
        target: ViewerPresentation,
        ready_host: u64,
    ) -> u64 {
        owner.request_transition(
            target,
            true,
            false,
            OUTGOING_PRESENTER,
            OUTGOING_HOST,
            ready_host,
        )
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
                PresentationTransitionEffect::CloseDetachedSession { request_id }
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
                PresentationTransitionEffect::CloseDetachedSession { request_id }
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

    /// 退場する別ウィンドウへ戻す要求は、**その窓を再利用先にしない**。
    ///
    /// `Detached(H) -> Fullscreen` の retire を待っている間に `Detached` を要求すると、
    /// まだ生きている `H` が後継の ready host として渡ってくる (`native_video.rs` の
    /// `ready_host_hwnd` は現在の detached host をそのまま読む)。retire 完了時に同じ
    /// batch が `H` を `DestroyHost` する以上、後継の準備先に `H` を選ぶと、破棄した窓を
    /// 相手に prepare することになる。
    ///
    /// **現在このテストは落ちる。** v3.3.0 の再レビューで見つかった R-27 の再現であり、
    /// 修正はまだ入っていない。ここで期待している `AwaitingHost` が正しい着地かどうかは
    /// host の所有権を移譲するのか作り直すのかの決着待ちで、detached リワークの凍結
    /// ルール (CLAUDE.md) に従って構造的修正として合意を取ってから直す。
    /// 直したら `#[ignore]` を外すこと。backlog §1.0d。
    #[test]
    #[ignore = "R-27 の再現。修正が入ったら ignore を外す (backlog §1.0d)"]
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
        owner.take_effects();

        owner.dispatch(PresentationTransitionEvent::NativeRetired {
            request_id: committed_id,
            candidate_generation: GENERATION,
        });
        let effects = owner.take_effects();
        let destroyed: Vec<u64> = effects
            .iter()
            .filter_map(|effect| match effect {
                PresentationTransitionEffect::DestroyHost { hwnd, .. } => Some(*hwnd),
                _ => None,
            })
            .collect();
        assert_eq!(destroyed, vec![OUTGOING_HOST]);

        assert!(
            matches!(
                owner.state(),
                PresentationTransitionState::Preparing {
                    request,
                    progress: PreparingProgress::AwaitingHost,
                } if request.id == successor_id
            ),
            "破棄した窓を準備先にしている: {:?}",
            owner.state()
        );
        assert!(
            !effects.iter().any(|effect| matches!(
                effect,
                PresentationTransitionEffect::PrepareNative { host_hwnd, .. }
                    if *host_hwnd == OUTGOING_HOST
            )),
            "破棄した窓へ prepare を出している: {effects:?}"
        );
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
        request(
            &mut owner,
            ViewerPresentation::DetachedWindow,
            CANDIDATE_HOST,
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
        let second_id = request(&mut owner, ViewerPresentation::MainWindow, MAIN_HOST);
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
                    progress: PreparingProgress::Aborting {
                        aborted_request_id,
                        ..
                    },
                } if aborted_request_id == issued_id && request.id == fourth_id
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
                    progress: PreparingProgress::AwaitingHost
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
                    progress: PreparingProgress::AwaitingHost
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
