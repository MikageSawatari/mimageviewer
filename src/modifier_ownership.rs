//! Queue-scoped modifier ownership types.
//!
//! This module deliberately contains only the S0 contract. L1 will connect the
//! Win32/egui producers and application key/wheel consumers to these types.

// Temporary until L1 wires every producer and consumer; remove this one module-level allow then.
#![allow(dead_code)]

use std::{marker::PhantomData, rc::Rc};

const MODIFIER_SIDE_COUNT: usize = 6;

/// One physical side of the three application modifier families.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ModifierSide {
    LeftControl,
    RightControl,
    LeftShift,
    RightShift,
    LeftAlt,
    RightAlt,
}

impl ModifierSide {
    const fn index(self) -> usize {
        match self {
            Self::LeftControl => 0,
            Self::RightControl => 1,
            Self::LeftShift => 2,
            Self::RightShift => 3,
            Self::LeftAlt => 4,
            Self::RightAlt => 5,
        }
    }
}

/// A known level for one physical modifier side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ModifierLevel {
    Released,
    Pressed,
}

/// Knowledge held for one physical modifier side in the current epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ModifierBit {
    Known(ModifierLevel),
    Unknown,
}

/// Whether the sided state can be interpreted as an AltGr sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AltGrStatus {
    NotPossible,
    PossibleAltGr,
}

/// Aggregate modifier families used by today's exact chord grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ModifierFamily {
    Control,
    Shift,
    Alt,
}

/// Typed output of a seed/current-state probe.
///
/// Callers supply per-side knowledge, never six unlabelled booleans. This is
/// input to the owner; it cannot itself create a delivery or current snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModifierSeed {
    bits: [ModifierBit; MODIFIER_SIDE_COUNT],
}

impl ModifierSeed {
    pub(crate) const fn unknown() -> Self {
        Self {
            bits: [ModifierBit::Unknown; MODIFIER_SIDE_COUNT],
        }
    }

    pub(crate) const fn known_released() -> Self {
        Self {
            bits: [ModifierBit::Known(ModifierLevel::Released); MODIFIER_SIDE_COUNT],
        }
    }

    pub(crate) const fn from_sides(
        left_control: ModifierBit,
        right_control: ModifierBit,
        left_shift: ModifierBit,
        right_shift: ModifierBit,
        left_alt: ModifierBit,
        right_alt: ModifierBit,
    ) -> Self {
        Self {
            bits: [
                left_control,
                right_control,
                left_shift,
                right_shift,
                left_alt,
                right_alt,
            ],
        }
    }

    pub(crate) const fn bit(self, side: ModifierSide) -> ModifierBit {
        self.bits[side.index()]
    }

    const fn with_transition(mut self, transition: ModifierTransition) -> Self {
        self.bits[transition.side().index()] = ModifierBit::Known(transition.level());
        self
    }

    const fn first_unknown(self) -> Option<ModifierSide> {
        let sides = [
            ModifierSide::LeftControl,
            ModifierSide::RightControl,
            ModifierSide::LeftShift,
            ModifierSide::RightShift,
            ModifierSide::LeftAlt,
            ModifierSide::RightAlt,
        ];
        let mut index = 0;
        while index < sides.len() {
            let side = sides[index];
            if matches!(self.bit(side), ModifierBit::Unknown) {
                return Some(side);
            }
            index += 1;
        }
        None
    }

    pub(crate) const fn alt_gr_status(self) -> AltGrStatus {
        if matches!(
            self.bit(ModifierSide::LeftControl),
            ModifierBit::Known(ModifierLevel::Pressed)
        ) && matches!(
            self.bit(ModifierSide::RightAlt),
            ModifierBit::Known(ModifierLevel::Pressed)
        ) {
            AltGrStatus::PossibleAltGr
        } else {
            AltGrStatus::NotPossible
        }
    }

    pub(crate) const fn aggregate(self, family: ModifierFamily) -> ModifierBit {
        let (left, right) = match family {
            ModifierFamily::Control => (
                self.bit(ModifierSide::LeftControl),
                self.bit(ModifierSide::RightControl),
            ),
            ModifierFamily::Shift => (
                self.bit(ModifierSide::LeftShift),
                self.bit(ModifierSide::RightShift),
            ),
            ModifierFamily::Alt => (
                self.bit(ModifierSide::LeftAlt),
                self.bit(ModifierSide::RightAlt),
            ),
        };
        match (left, right) {
            (ModifierBit::Known(ModifierLevel::Pressed), _)
            | (_, ModifierBit::Known(ModifierLevel::Pressed)) => {
                ModifierBit::Known(ModifierLevel::Pressed)
            }
            (ModifierBit::Unknown, _) | (_, ModifierBit::Unknown) => ModifierBit::Unknown,
            _ => ModifierBit::Known(ModifierLevel::Released),
        }
    }
}

/// A modifier-key transition folded before its key envelope is stamped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ModifierTransition {
    Pressed(ModifierSide),
    Released(ModifierSide),
}

impl ModifierTransition {
    const fn side(self) -> ModifierSide {
        match self {
            Self::Pressed(side) | Self::Released(side) => side,
        }
    }

    const fn level(self) -> ModifierLevel {
        match self {
            Self::Pressed(_) => ModifierLevel::Pressed,
            Self::Released(_) => ModifierLevel::Released,
        }
    }
}

/// Exact aggregate modifier requirements for one application chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChordModifiers {
    control: ModifierLevel,
    shift: ModifierLevel,
    alt: ModifierLevel,
}

impl ChordModifiers {
    pub(crate) const fn exact(
        control: ModifierLevel,
        shift: ModifierLevel,
        alt: ModifierLevel,
    ) -> Self {
        Self {
            control,
            shift,
            alt,
        }
    }

    const fn required(self, family: ModifierFamily) -> ModifierLevel {
        match family {
            ModifierFamily::Control => self.control,
            ModifierFamily::Shift => self.shift,
            ModifierFamily::Alt => self.alt,
        }
    }
}

/// Why an application chord cannot be decided safely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IndeterminateReason {
    Acquiring(AcquisitionCause),
    InternallyAttached,
    ExternallyAttached,
    UnknownModifier(ModifierSide),
    PossibleAltGr,
}

/// Three-valued exact chord result. Indeterminate always carries provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChordMatch {
    Match,
    NoMatch,
    Indeterminate(IndeterminateReason),
}

/// Monotonic queue-local routing epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ModifierEpoch(u64);

impl ModifierEpoch {
    pub(crate) const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeliveryStamp {
    Stable {
        epoch: ModifierEpoch,
        modifiers: ModifierSeed,
    },
    Indeterminate(IndeterminateReason),
}

/// Modifier truth at the queue position occupied by a delivered packet.
///
/// The tuple field is private. Only [`QueueModifierOwner`] can issue this type,
/// and there is intentionally no conversion to or from [`CurrentModifiers`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeliveryModifiers(DeliveryStamp);

impl DeliveryModifiers {
    pub(crate) const fn epoch(self) -> Option<ModifierEpoch> {
        match self.0 {
            DeliveryStamp::Stable { epoch, .. } => Some(epoch),
            DeliveryStamp::Indeterminate(_) => None,
        }
    }

    pub(crate) const fn bit(self, side: ModifierSide) -> Option<ModifierBit> {
        match self.0 {
            DeliveryStamp::Stable { modifiers, .. } => Some(modifiers.bit(side)),
            DeliveryStamp::Indeterminate(_) => None,
        }
    }

    pub(crate) fn match_chord(self, chord: ChordModifiers) -> ChordMatch {
        let modifiers = match self.0 {
            DeliveryStamp::Stable { modifiers, .. } => modifiers,
            DeliveryStamp::Indeterminate(reason) => {
                return ChordMatch::Indeterminate(reason);
            }
        };

        if matches!(modifiers.alt_gr_status(), AltGrStatus::PossibleAltGr) {
            return ChordMatch::Indeterminate(IndeterminateReason::PossibleAltGr);
        }
        if let Some(side) = modifiers.first_unknown() {
            return ChordMatch::Indeterminate(IndeterminateReason::UnknownModifier(side));
        }

        let families = [
            ModifierFamily::Control,
            ModifierFamily::Shift,
            ModifierFamily::Alt,
        ];
        let mut index = 0;
        while index < families.len() {
            let family = families[index];
            if modifiers.aggregate(family) != ModifierBit::Known(chord.required(family)) {
                return ChordMatch::NoMatch;
            }
            index += 1;
        }
        ChordMatch::Match
    }
}

/// Modifier truth sampled for current-level/hold behavior.
///
/// This is a distinct opaque type, not a projection of delivery truth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CurrentModifiers(ModifierSeed);

impl CurrentModifiers {
    pub(crate) const fn bit(self, side: ModifierSide) -> ModifierBit {
        self.0.bit(side)
    }

    pub(crate) const fn aggregate(self, family: ModifierFamily) -> ModifierBit {
        self.0.aggregate(family)
    }

    pub(crate) const fn alt_gr_status(self) -> AltGrStatus {
        self.0.alt_gr_status()
    }
}

/// Why a queue must discard its ambiguous fold before becoming stable again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AcquisitionCause {
    RoutingAcquired,
    InternalDetach,
    ExternalDetach,
    SeedRecovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StableQueueAuthority {
    epoch: ModifierEpoch,
    modifiers: ModifierSeed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AcquiringQueueAuthority {
    pending_epoch: ModifierEpoch,
    cause: AcquisitionCause,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AttachedQueueAuthority {
    target: AttachmentTarget,
}

/// The single source of truth for queue authority.
///
/// Stable modifier state is present only in `Stable`; the other variants
/// cannot accidentally expose it through a parallel boolean or optional field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueueAuthorityState {
    Stable(StableQueueAuthority),
    Acquiring(AcquiringQueueAuthority),
    InternallyAttached(AttachedQueueAuthority),
    ExternallyAttached(AttachedQueueAuthority),
}

/// Read-only phase projection used for diagnostics and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueueAuthorityPhase {
    Stable,
    Acquiring,
    InternallyAttached,
    ExternallyAttached,
}

impl QueueAuthorityState {
    const fn phase(self) -> QueueAuthorityPhase {
        match self {
            Self::Stable(_) => QueueAuthorityPhase::Stable,
            Self::Acquiring(_) => QueueAuthorityPhase::Acquiring,
            Self::InternallyAttached(_) => QueueAuthorityPhase::InternallyAttached,
            Self::ExternallyAttached(_) => QueueAuthorityPhase::ExternallyAttached,
        }
    }
}

/// Result of the owner's synchronous unfiltered `PM_NOREMOVE` probe.
///
/// `Empty` means that `PeekMessageW` returned false at this linearization
/// point. It is not a promise that the queue will remain empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueueDrainProbe {
    MessagesRemain,
    Empty(ModifierSeed),
}

/// Outcome of one synchronous probe/reseed/transition call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrainTransition {
    StillAcquiring,
    Stabilized(ModifierEpoch),
}

/// Whether an attach/detach API call succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachApiResult {
    Succeeded,
    Failed,
}

/// Whether the process can observe every dequeue while attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachmentTarget {
    Internal,
    External,
}

/// A topology which never became attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UnattachedTopology {
    phase: QueueAuthorityPhase,
}

impl UnattachedTopology {
    pub(crate) const fn phase(self) -> QueueAuthorityPhase {
        self.phase
    }
}

/// Proof that the topology remains joined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AttachedTopology {
    target: AttachmentTarget,
}

impl AttachedTopology {
    pub(crate) const fn target(self) -> AttachmentTarget {
        self.target
    }
}

/// Proof that a successful detach moved the local queue to `Acquiring`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SplitTopology {
    acquisition: AcquisitionCause,
}

impl SplitTopology {
    pub(crate) const fn acquisition(self) -> AcquisitionCause {
        self.acquisition
    }
}

/// Typed result of an attach or detach transaction.
///
/// In particular, `DetachFailed` can carry only [`AttachedTopology`]. There is
/// no representation in which a failed detach publishes [`SplitTopology`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachTransactionOutcome {
    AttachFailed(UnattachedTopology),
    Attached(AttachedTopology),
    Detached(SplitTopology),
    DetachFailed(AttachedTopology),
}

/// Invalid owner transition requested by a producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnerTransitionError {
    NotAcquiring,
    AlreadyAttached,
    NotAttached,
}

/// The queue-local owner and the only factory for modifier snapshots/envelopes.
///
/// `Rc` in the marker makes this owner `!Send + !Sync`; all mutating APIs need
/// `&mut self`. A producer therefore cannot move probe/reseed/transition work
/// to another thread or overlap it through the safe API.
#[derive(Debug)]
pub(crate) struct QueueModifierOwner {
    authority: QueueAuthorityState,
    next_epoch: u64,
    owning_thread: PhantomData<Rc<()>>,
}

impl QueueModifierOwner {
    pub(crate) fn new(initial_seed: ModifierSeed) -> Self {
        Self {
            authority: QueueAuthorityState::Stable(StableQueueAuthority {
                epoch: ModifierEpoch(0),
                modifiers: initial_seed,
            }),
            next_epoch: 1,
            owning_thread: PhantomData,
        }
    }

    pub(crate) const fn phase(&self) -> QueueAuthorityPhase {
        self.authority.phase()
    }

    fn allocate_epoch(&mut self) -> ModifierEpoch {
        let epoch = ModifierEpoch(self.next_epoch);
        self.next_epoch = self
            .next_epoch
            .checked_add(1)
            .expect("modifier epoch counter exhausted");
        epoch
    }

    fn enter_acquiring(&mut self, cause: AcquisitionCause) {
        if matches!(self.authority, QueueAuthorityState::Acquiring(_)) {
            return;
        }
        let pending_epoch = self.allocate_epoch();
        self.authority = QueueAuthorityState::Acquiring(AcquiringQueueAuthority {
            pending_epoch,
            cause,
        });
    }

    pub(crate) fn begin_acquisition(
        &mut self,
        cause: AcquisitionCause,
    ) -> Result<(), OwnerTransitionError> {
        match self.authority {
            QueueAuthorityState::Stable(_) => self.enter_acquiring(cause),
            QueueAuthorityState::Acquiring(_) => {}
            QueueAuthorityState::InternallyAttached(_)
            | QueueAuthorityState::ExternallyAttached(_) => {
                return Err(OwnerTransitionError::AlreadyAttached);
            }
        }
        Ok(())
    }

    /// Run queue-empty probe, reseed, and transition as one owner-thread call.
    ///
    /// The closure must perform the unfiltered `PM_NOREMOVE` probe and, only
    /// when it reports empty, sample the six sided modifier bits. A message
    /// dequeued before this call commits still sees `Acquiring`; a message
    /// dequeued after it commits receives the returned new epoch.
    pub(crate) fn probe_reseed_and_transition<F>(
        &mut self,
        probe: F,
    ) -> Result<DrainTransition, OwnerTransitionError>
    where
        F: FnOnce() -> QueueDrainProbe,
    {
        let acquiring = match self.authority {
            QueueAuthorityState::Acquiring(acquiring) => acquiring,
            _ => return Err(OwnerTransitionError::NotAcquiring),
        };

        match probe() {
            QueueDrainProbe::MessagesRemain => Ok(DrainTransition::StillAcquiring),
            QueueDrainProbe::Empty(modifiers) => {
                self.authority = QueueAuthorityState::Stable(StableQueueAuthority {
                    epoch: acquiring.pending_epoch,
                    modifiers,
                });
                Ok(DrainTransition::Stabilized(acquiring.pending_epoch))
            }
        }
    }

    /// Prepare fail-closed state, invoke `AttachThreadInput`, then commit or
    /// restore the exact pre-attach state without exposing an intermediate gap.
    pub(crate) fn attach_transaction<F>(
        &mut self,
        target: AttachmentTarget,
        attach: F,
    ) -> Result<AttachTransactionOutcome, OwnerTransitionError>
    where
        F: FnOnce() -> AttachApiResult,
    {
        let previous = self.authority;
        if matches!(
            previous,
            QueueAuthorityState::InternallyAttached(_) | QueueAuthorityState::ExternallyAttached(_)
        ) {
            return Err(OwnerTransitionError::AlreadyAttached);
        }

        let attached = AttachedQueueAuthority { target };
        self.authority = match target {
            AttachmentTarget::Internal => QueueAuthorityState::InternallyAttached(attached),
            AttachmentTarget::External => QueueAuthorityState::ExternallyAttached(attached),
        };

        match attach() {
            AttachApiResult::Succeeded => {
                Ok(AttachTransactionOutcome::Attached(AttachedTopology {
                    target,
                }))
            }
            AttachApiResult::Failed => {
                self.authority = previous;
                Ok(AttachTransactionOutcome::AttachFailed(UnattachedTopology {
                    phase: previous.phase(),
                }))
            }
        }
    }

    /// Invoke detach while remaining fail-closed, then either enter Acquiring
    /// or retain attached topology on failure.
    pub(crate) fn detach_transaction<F>(
        &mut self,
        detach: F,
    ) -> Result<AttachTransactionOutcome, OwnerTransitionError>
    where
        F: FnOnce() -> AttachApiResult,
    {
        let target = match self.authority {
            QueueAuthorityState::InternallyAttached(attached)
            | QueueAuthorityState::ExternallyAttached(attached) => attached.target,
            _ => return Err(OwnerTransitionError::NotAttached),
        };

        match detach() {
            AttachApiResult::Failed => {
                Ok(AttachTransactionOutcome::DetachFailed(AttachedTopology {
                    target,
                }))
            }
            AttachApiResult::Succeeded => {
                let acquisition = match target {
                    AttachmentTarget::Internal => AcquisitionCause::InternalDetach,
                    AttachmentTarget::External => AcquisitionCause::ExternalDetach,
                };
                self.enter_acquiring(acquisition);
                Ok(AttachTransactionOutcome::Detached(SplitTopology {
                    acquisition,
                }))
            }
        }
    }

    fn delivery_modifiers(&self) -> DeliveryModifiers {
        let stamp = match self.authority {
            QueueAuthorityState::Stable(stable) => DeliveryStamp::Stable {
                epoch: stable.epoch,
                modifiers: stable.modifiers,
            },
            QueueAuthorityState::Acquiring(acquiring) => {
                DeliveryStamp::Indeterminate(IndeterminateReason::Acquiring(acquiring.cause))
            }
            QueueAuthorityState::InternallyAttached(_) => {
                DeliveryStamp::Indeterminate(IndeterminateReason::InternallyAttached)
            }
            QueueAuthorityState::ExternallyAttached(_) => {
                DeliveryStamp::Indeterminate(IndeterminateReason::ExternallyAttached)
            }
        };
        DeliveryModifiers(stamp)
    }

    pub(crate) fn sample_current(&self, sample: ModifierSeed) -> CurrentModifiers {
        CurrentModifiers(sample)
    }

    /// Stamp a key after folding its own modifier transition, if any.
    pub(crate) fn stamp_key<T>(
        &mut self,
        payload: T,
        transition: Option<ModifierTransition>,
    ) -> KeyEnvelope<T> {
        if let (QueueAuthorityState::Stable(stable), Some(transition)) =
            (&mut self.authority, transition)
        {
            stable.modifiers = stable.modifiers.with_transition(transition);
        }
        KeyEnvelope {
            payload,
            delivery: self.delivery_modifiers(),
        }
    }

    pub(crate) fn stamp_wheel<T>(&self, payload: T) -> WheelEnvelope<T> {
        WheelEnvelope {
            payload,
            delivery: self.delivery_modifiers(),
        }
    }

    pub(crate) fn stamp_button_down<T>(&self, payload: T) -> ButtonDownEnvelope<T> {
        ButtonDownEnvelope {
            payload,
            delivery: self.delivery_modifiers(),
        }
    }

    pub(crate) fn stamp_button_up<T>(&self, payload: T) -> ButtonUpEnvelope<T> {
        ButtonUpEnvelope {
            payload,
            delivery: self.delivery_modifiers(),
        }
    }
}

macro_rules! delivery_envelope {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) struct $name<T> {
            payload: T,
            delivery: DeliveryModifiers,
        }

        impl<T> $name<T> {
            pub(crate) const fn payload(&self) -> &T {
                &self.payload
            }

            pub(crate) const fn delivery(&self) -> DeliveryModifiers {
                self.delivery
            }

            pub(crate) fn into_parts(self) -> (T, DeliveryModifiers) {
                (self.payload, self.delivery)
            }
        }
    };
}

// Delivery-stamped key packet.
delivery_envelope!(KeyEnvelope);
// Delivery-stamped wheel packet (one packet, never a frame aggregate).
delivery_envelope!(WheelEnvelope);
// Delivery-stamped button-down packet.
delivery_envelope!(ButtonDownEnvelope);
// Delivery-stamped button-up packet.
delivery_envelope!(ButtonUpEnvelope);

/// Modifier ownership carried by a gesture.
///
/// `start` remains the originating button-down delivery stamp for clicks,
/// double-clicks, and gesture selection. `current` may change every frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GestureModifiers {
    start: DeliveryModifiers,
    current: CurrentModifiers,
}

impl GestureModifiers {
    pub(crate) const fn start(self) -> DeliveryModifiers {
        self.start
    }

    pub(crate) const fn current(self) -> CurrentModifiers {
        self.current
    }

    pub(crate) fn update_current(&mut self, current: CurrentModifiers) {
        self.current = current;
    }
}

impl<T> ButtonDownEnvelope<T> {
    pub(crate) const fn begin_gesture(&self, current: CurrentModifiers) -> GestureModifiers {
        GestureModifiers {
            start: self.delivery,
            current,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    macro_rules! assert_not_impl_any {
        ($ty:ty: $trait:path) => {
            const _: fn() = || {
                trait AmbiguousIfImpl<A> {
                    fn marker() {}
                }
                impl<T: ?Sized> AmbiguousIfImpl<()> for T {}
                impl<T: ?Sized + $trait> AmbiguousIfImpl<u8> for T {}
                let _ = <$ty as AmbiguousIfImpl<_>>::marker;
            };
        };
    }

    assert_not_impl_any!(DeliveryModifiers: From<CurrentModifiers>);
    assert_not_impl_any!(CurrentModifiers: From<DeliveryModifiers>);
    assert_not_impl_any!(DeliveryModifiers: From<egui::Modifiers>);
    assert_not_impl_any!(CurrentModifiers: From<egui::Modifiers>);
    assert_not_impl_any!(QueueModifierOwner: Send);
    assert_not_impl_any!(QueueModifierOwner: Sync);

    const RELEASED: ModifierBit = ModifierBit::Known(ModifierLevel::Released);
    const PRESSED: ModifierBit = ModifierBit::Known(ModifierLevel::Pressed);

    fn seed_with_pressed(pressed: &[ModifierSide]) -> ModifierSeed {
        let mut seed = ModifierSeed::known_released();
        for side in pressed {
            seed = seed.with_transition(ModifierTransition::Pressed(*side));
        }
        seed
    }

    fn no_modifiers() -> ChordModifiers {
        ChordModifiers::exact(
            ModifierLevel::Released,
            ModifierLevel::Released,
            ModifierLevel::Released,
        )
    }

    fn control_only() -> ChordModifiers {
        ChordModifiers::exact(
            ModifierLevel::Pressed,
            ModifierLevel::Released,
            ModifierLevel::Released,
        )
    }

    #[test]
    fn sided_known_unknown_and_possible_alt_gr_fold_without_guessing() {
        let partly_unknown = ModifierSeed::from_sides(
            ModifierBit::Unknown,
            RELEASED,
            RELEASED,
            RELEASED,
            RELEASED,
            RELEASED,
        );
        assert_eq!(
            partly_unknown.aggregate(ModifierFamily::Control),
            ModifierBit::Unknown
        );

        let left_control_down =
            partly_unknown.with_transition(ModifierTransition::Pressed(ModifierSide::LeftControl));
        assert_eq!(
            left_control_down.aggregate(ModifierFamily::Control),
            PRESSED
        );
        assert_eq!(
            left_control_down.bit(ModifierSide::RightControl),
            RELEASED,
            "a transition resolves only its own sided bit"
        );

        let possible_alt_gr =
            left_control_down.with_transition(ModifierTransition::Pressed(ModifierSide::RightAlt));
        assert_eq!(possible_alt_gr.alt_gr_status(), AltGrStatus::PossibleAltGr);
        let no_longer_alt_gr = possible_alt_gr
            .with_transition(ModifierTransition::Released(ModifierSide::LeftControl));
        assert_eq!(no_longer_alt_gr.alt_gr_status(), AltGrStatus::NotPossible);
    }

    #[test]
    fn chord_match_indeterminate_always_carries_a_reason() {
        let unknown_owner = QueueModifierOwner::new(ModifierSeed::unknown());
        assert_eq!(
            unknown_owner
                .delivery_modifiers()
                .match_chord(no_modifiers()),
            ChordMatch::Indeterminate(IndeterminateReason::UnknownModifier(
                ModifierSide::LeftControl
            ))
        );

        let alt_gr_owner = QueueModifierOwner::new(seed_with_pressed(&[
            ModifierSide::LeftControl,
            ModifierSide::RightAlt,
        ]));
        assert_eq!(
            alt_gr_owner
                .delivery_modifiers()
                .match_chord(control_only()),
            ChordMatch::Indeterminate(IndeterminateReason::PossibleAltGr)
        );
    }

    #[test]
    fn acquiring_is_fail_closed_until_probe_reseed_transition_commits() {
        let mut owner = QueueModifierOwner::new(ModifierSeed::known_released());
        let old_epoch = owner.delivery_modifiers().epoch().expect("stable epoch");
        owner
            .begin_acquisition(AcquisitionCause::RoutingAcquired)
            .expect("enter acquisition");

        let before_commit = owner.stamp_key("before", None);
        assert_eq!(
            before_commit.delivery().match_chord(no_modifiers()),
            ChordMatch::Indeterminate(IndeterminateReason::Acquiring(
                AcquisitionCause::RoutingAcquired
            ))
        );
        assert_eq!(
            owner
                .probe_reseed_and_transition(|| QueueDrainProbe::MessagesRemain)
                .expect("probe while acquiring"),
            DrainTransition::StillAcquiring
        );
        assert_eq!(owner.phase(), QueueAuthorityPhase::Acquiring);

        let new_seed = seed_with_pressed(&[ModifierSide::RightControl]);
        let committed_epoch = match owner
            .probe_reseed_and_transition(|| QueueDrainProbe::Empty(new_seed))
            .expect("empty probe commits synchronously")
        {
            DrainTransition::Stabilized(epoch) => epoch,
            DrainTransition::StillAcquiring => panic!("empty probe must stabilize"),
        };
        assert_ne!(committed_epoch, old_epoch);

        let after_commit = owner.stamp_key("after", None);
        assert_eq!(after_commit.delivery().epoch(), Some(committed_epoch));
        assert_eq!(
            after_commit.delivery().match_chord(control_only()),
            ChordMatch::Match,
            "an event after the linearization commit belongs to the new epoch"
        );
    }

    #[test]
    fn modifier_key_envelope_is_stamped_after_its_own_transition() {
        let mut owner = QueueModifierOwner::new(ModifierSeed::known_released());
        let down = owner.stamp_key(
            "control-down",
            Some(ModifierTransition::Pressed(ModifierSide::LeftControl)),
        );
        assert_eq!(
            down.delivery().match_chord(control_only()),
            ChordMatch::Match
        );

        let up = owner.stamp_key(
            "control-up",
            Some(ModifierTransition::Released(ModifierSide::LeftControl)),
        );
        assert_eq!(up.delivery().match_chord(no_modifiers()), ChordMatch::Match);
    }

    #[test]
    fn every_discrete_packet_has_delivery_and_gesture_keeps_button_down_start() {
        let mut owner = QueueModifierOwner::new(ModifierSeed::known_released());
        owner
            .begin_acquisition(AcquisitionCause::RoutingAcquired)
            .expect("enter acquisition");

        let key = owner.stamp_key("key", None);
        let wheel = owner.stamp_wheel("wheel");
        let down = owner.stamp_button_down("down");
        let up = owner.stamp_button_up("up");
        for delivery in [
            key.delivery(),
            wheel.delivery(),
            down.delivery(),
            up.delivery(),
        ] {
            assert!(matches!(
                delivery.match_chord(no_modifiers()),
                ChordMatch::Indeterminate(IndeterminateReason::Acquiring(_))
            ));
        }

        owner
            .probe_reseed_and_transition(|| QueueDrainProbe::Empty(ModifierSeed::known_released()))
            .expect("commit acquisition");
        let current = owner.sample_current(seed_with_pressed(&[ModifierSide::LeftShift]));
        let gesture = down.begin_gesture(current);
        assert!(matches!(
            gesture.start().match_chord(no_modifiers()),
            ChordMatch::Indeterminate(IndeterminateReason::Acquiring(_))
        ));
        assert_eq!(gesture.current().aggregate(ModifierFamily::Shift), PRESSED);
    }

    #[test]
    fn detach_failure_can_only_return_attached_topology() {
        let mut owner = QueueModifierOwner::new(ModifierSeed::known_released());
        let attached = owner
            .attach_transaction(AttachmentTarget::Internal, || AttachApiResult::Succeeded)
            .expect("attach transaction");
        assert!(matches!(
            attached,
            AttachTransactionOutcome::Attached(AttachedTopology {
                target: AttachmentTarget::Internal
            })
        ));
        assert_eq!(owner.phase(), QueueAuthorityPhase::InternallyAttached);
        assert_eq!(
            owner.delivery_modifiers().match_chord(no_modifiers()),
            ChordMatch::Indeterminate(IndeterminateReason::InternallyAttached)
        );

        let failed = owner
            .detach_transaction(|| AttachApiResult::Failed)
            .expect("detach transaction");
        let still_attached: AttachedTopology = match failed {
            AttachTransactionOutcome::DetachFailed(topology) => topology,
            other => panic!("detach failure published the wrong topology: {other:?}"),
        };
        assert_eq!(still_attached.target(), AttachmentTarget::Internal);
        assert_eq!(owner.phase(), QueueAuthorityPhase::InternallyAttached);

        let detached = owner
            .detach_transaction(|| AttachApiResult::Succeeded)
            .expect("successful detach");
        let split: SplitTopology = match detached {
            AttachTransactionOutcome::Detached(topology) => topology,
            other => panic!("successful detach did not split: {other:?}"),
        };
        assert_eq!(split.acquisition(), AcquisitionCause::InternalDetach);
        assert_eq!(owner.phase(), QueueAuthorityPhase::Acquiring);
    }

    #[test]
    fn external_attach_is_named_and_fail_closed() {
        let mut owner = QueueModifierOwner::new(ModifierSeed::known_released());
        owner
            .attach_transaction(AttachmentTarget::External, || AttachApiResult::Succeeded)
            .expect("external attach");
        assert_eq!(owner.phase(), QueueAuthorityPhase::ExternallyAttached);
        assert_eq!(
            owner.delivery_modifiers().match_chord(no_modifiers()),
            ChordMatch::Indeterminate(IndeterminateReason::ExternallyAttached)
        );
    }

    struct ConstructionSourceExemption {
        path: &'static str,
        reason: &'static str,
    }

    const CONSTRUCTION_SOURCE_EXEMPTIONS: &[ConstructionSourceExemption] =
        &[ConstructionSourceExemption {
            path: "src/modifier_ownership.rs",
            reason: "queue owner implementation is the sole modifier construction boundary",
        }];

    fn collect_rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read src directory") {
            let entry = entry.expect("read src entry");
            let path = entry.path();
            if path.is_dir() {
                collect_rust_sources(&path, files);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }

    #[test]
    fn modifier_snapshot_construction_stays_inside_the_owner_module() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let src_dir = manifest_dir.join("src");
        let mut files = Vec::new();
        collect_rust_sources(&src_dir, &mut files);
        files.sort();

        for exemption in CONSTRUCTION_SOURCE_EXEMPTIONS {
            assert!(
                !exemption.reason.trim().is_empty(),
                "modifier construction exemption needs a reason: {}",
                exemption.path
            );
            assert!(
                manifest_dir.join(exemption.path).is_file(),
                "modifier construction exemption path is stale: {} ({})",
                exemption.path,
                exemption.reason
            );
        }

        let forbidden_conversions = [
            ["From<egui::", "Modifiers>forDeliveryModifiers"].concat(),
            ["From<egui::", "Modifiers>forCurrentModifiers"].concat(),
            ["From<Delivery", "Modifiers>forCurrentModifiers"].concat(),
            ["From<Current", "Modifiers>forDeliveryModifiers"].concat(),
        ];
        let return_type_pattern = regex::Regex::new(
            &[
                r"(?s)fn\s+[A-Za-z0-9_]+\s*(?:<[^{};]*>)?\s*\([^)]*\)",
                r"\s*->\s*(?:crate::modifier_ownership::)?(?:DeliveryModifiers|CurrentModifiers)",
            ]
            .concat(),
        )
        .expect("valid modifier factory regex");
        let raw_bool_self_factory_pattern = regex::Regex::new(
            &[
                r"(?s)impl\s+(?:DeliveryModifiers|CurrentModifiers)\s*\{",
                r".*?fn\s+[A-Za-z0-9_]+\s*\([^)]*\bbool\b[^)]*\)\s*->\s*Self",
            ]
            .concat(),
        )
        .expect("valid raw-bool factory regex");

        let mut violations = Vec::new();
        for path in files {
            let relative = path
                .strip_prefix(&manifest_dir)
                .expect("source under manifest directory")
                .to_string_lossy()
                .replace('\\', "/");
            let source = std::fs::read_to_string(&path).expect("read Rust source as UTF-8");
            let compact: String = source
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect();

            for forbidden in &forbidden_conversions {
                if compact.contains(forbidden) {
                    violations.push(format!(
                        "{relative}: forbidden modifier conversion `{forbidden}`"
                    ));
                }
            }
            if raw_bool_self_factory_pattern.is_match(&source) {
                violations.push(format!(
                    "{relative}: DeliveryModifiers/CurrentModifiers has a raw-bool constructor"
                ));
            }

            let exempt = CONSTRUCTION_SOURCE_EXEMPTIONS
                .iter()
                .any(|exemption| exemption.path == relative);
            if !exempt {
                for found in return_type_pattern.find_iter(&source) {
                    violations.push(format!(
                        "{relative}: owner-external modifier factory `{}`",
                        found
                            .as_str()
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                    ));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "modifier snapshots must be owner-constructed; add no raw bool/egui conversion and keep factories in src/modifier_ownership.rs:\n{}",
            violations.join("\n")
        );
    }
}
