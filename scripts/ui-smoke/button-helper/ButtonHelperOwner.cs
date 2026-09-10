using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Threading;
using Miv.UiSmoke.ButtonDraft;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    internal interface IButtonClock
    {
        ulong NowTick { get; }
    }

    internal interface IAppProcessLease
    {
        uint ProcessId { get; }
        ulong CreationIdentity { get; }
        bool IsAlive { get; }
    }

    // The owner copies the decoded wire DTO before any input call. Wire objects
    // remain mutable for decoding, while the gesture owner and backend only see
    // this immutable same-point click target.
    internal sealed class ButtonGestureTarget
    {
        internal readonly ulong ParentHwnd;
        internal readonly ulong InputHwnd;
        internal readonly int ScreenX;
        internal readonly int ScreenY;
        internal readonly ulong SourceEpoch;
        internal readonly ulong PlacementGeneration;
        internal readonly ulong HostIncarnation;
        internal readonly ulong BackendToken;

        internal ButtonGestureTarget(ButtonWireTarget source)
        {
            if (source == null) throw new ArgumentNullException("source");
            ParentHwnd = source.ParentHwnd;
            InputHwnd = source.InputHwnd;
            ScreenX = source.ScreenX;
            ScreenY = source.ScreenY;
            SourceEpoch = source.SourceEpoch;
            PlacementGeneration = source.PlacementGeneration;
            HostIncarnation = source.HostIncarnation;
            BackendToken = source.BackendToken;
        }

        internal bool Matches(ButtonWireTarget other)
        {
            return other != null
                && ParentHwnd == other.ParentHwnd
                && InputHwnd == other.InputHwnd
                && ScreenX == other.ScreenX
                && ScreenY == other.ScreenY
                && SourceEpoch == other.SourceEpoch
                && PlacementGeneration == other.PlacementGeneration
                && HostIncarnation == other.HostIncarnation
                && BackendToken == other.BackendToken;
        }
    }

    internal sealed class ButtonGestureTuple
    {
        internal readonly Guid SessionNonce;
        internal readonly ulong GestureId;
        internal readonly ulong DeadlineTick;
        internal readonly uint AppProcessId;
        internal readonly ulong AppCreationIdentity;
        internal readonly ButtonGestureTarget Target;
        internal readonly ulong DownTag;
        internal readonly ulong UpTag;

        internal ButtonGestureTuple(ButtonWireRequest source)
        {
            if (source == null) throw new ArgumentNullException("source");
            SessionNonce = source.SessionNonce;
            GestureId = source.GestureId;
            DeadlineTick = source.DeadlineTick;
            AppProcessId = source.AppProcessId;
            AppCreationIdentity = source.AppCreationIdentity;
            Target = new ButtonGestureTarget(source.Target);
            DownTag = source.DownTag;
            UpTag = source.UpTag;
        }

        internal bool Matches(ButtonWireRequest other)
        {
            return other != null
                && SessionNonce == other.SessionNonce
                && GestureId == other.GestureId
                && DeadlineTick == other.DeadlineTick
                && AppProcessId == other.AppProcessId
                && AppCreationIdentity == other.AppCreationIdentity
                && DownTag == other.DownTag
                && UpTag == other.UpTag
                && Target.Matches(other.Target);
        }
    }

    internal interface IButtonReplySink
    {
        bool TryPublish(ButtonWireReply reply);
    }

    internal sealed class ButtonHelperOwner
    {
        private readonly int ownerThreadId;
        private readonly Guid sessionNonce;
        private readonly IAppProcessLease appLease;
        private readonly IButtonClock clock;
        private readonly IButtonInputBackend input;
        private readonly IButtonDeliveryObserver deliveryObserver;
        private readonly IButtonReplySink replies;
        private readonly ulong cleanupBudgetTicks;
        private ButtonGestureTuple first;
        private ObservedButtonDownAnchor downAnchor;
        private ButtonGestureReducer reducer;
        private uint lastWireStep;
        private bool terminal;
        private bool replyFaulted;
        private bool appDeathLatched;
        private ulong nextCleanupRetryTick;

        internal ButtonHelperOwner(
            Guid sessionNonce,
            IAppProcessLease appLease,
            IButtonClock clock,
            IButtonInputBackend input,
            IButtonDeliveryObserver deliveryObserver,
            IButtonReplySink replies,
            ulong cleanupBudgetTicks)
        {
            if (sessionNonce == Guid.Empty) throw new ArgumentException("empty nonce", "sessionNonce");
            if (appLease == null) throw new ArgumentNullException("appLease");
            if (clock == null) throw new ArgumentNullException("clock");
            if (input == null) throw new ArgumentNullException("input");
            if (deliveryObserver == null) throw new ArgumentNullException("deliveryObserver");
            if (replies == null) throw new ArgumentNullException("replies");
            if (cleanupBudgetTicks == 0)
                throw new ArgumentOutOfRangeException("cleanupBudgetTicks");
            ownerThreadId = Thread.CurrentThread.ManagedThreadId;
            this.sessionNonce = sessionNonce;
            this.appLease = appLease;
            this.clock = clock;
            this.input = input;
            this.deliveryObserver = deliveryObserver;
            this.replies = replies;
            this.cleanupBudgetTicks = cleanupBudgetTicks;
        }

        internal bool IsTerminal { get { return terminal; } }
        internal bool HasBegun { get { return first != null; } }
        internal bool HasOutstandingRelease
        {
            get { return reducer != null && reducer.HasOutstandingRelease; }
        }
        internal GesturePhase Phase
        {
            get { return reducer == null ? GesturePhase.ReadyForDown : reducer.Phase; }
        }
        internal string PrimaryFailure
        {
            get { return reducer == null ? null : reducer.PrimaryFailure; }
        }
        internal int CleanupAttempts
        {
            get { return reducer == null ? 0 : reducer.CleanupAttempts; }
        }

        internal void Handle(ButtonWireRequest request)
        {
            RequireOwnerThread();
            if (request == null) throw new ArgumentNullException("request");
            request.ValidateShape();
            if (request.SessionNonce != sessionNonce)
                throw new ButtonWireProtocolException("session nonce mismatch");
            if (terminal) throw new ButtonWireProtocolException("gesture is terminal");

            if (first == null)
            {
                Begin(request);
                return;
            }
            ValidateContinuation(request);
            ValidateCommandForPhase(request.Kind);
            lastWireStep = request.Step;
            switch (request.Kind)
            {
                case ButtonWireCommandKind.DownDispatchReceipt:
                    HandleDownReceipt(request);
                    break;
                case ButtonWireCommandKind.ReleaseGesture:
                    HandleRelease(request);
                    break;
                case ButtonWireCommandKind.UpSemanticReceipt:
                    HandleUpReceipt(request);
                    break;
                case ButtonWireCommandKind.CancelGesture:
                    ApplyAction(reducer.Cancel(CancelText(request.Flags), clock.NowTick), request.Step);
                    PublishState(request.Step);
                    break;
                case ButtonWireCommandKind.Shutdown:
                    ApplyAction(reducer.Cancel("pipe shutdown", clock.NowTick), request.Step);
                    PublishState(request.Step);
                    break;
                default:
                    throw new ButtonWireProtocolException("command is invalid after begin");
            }
        }

        internal void ObserveReleaseEvidence(ButtonReleaseEvidence evidence)
        {
            RequireOwnerThread();
            if (first == null || terminal || evidence == null || downAnchor == null) return;
            if (evidence.Scope == ButtonObservationScope.Normal && !appLease.IsAlive)
            {
                appDeathLatched = true;
                ApplyAction(
                    reducer.Cancel(
                        "exact application process exited before normal release evidence",
                        clock.NowTick),
                    lastWireStep);
                PublishState(lastWireStep);
                return;
            }
            GesturePhase before = reducer.Phase;
            int attemptsBefore = reducer.CleanupAttempts;
            ApplyReleaseEvidence(evidence);
            if (before != reducer.Phase
                || attemptsBefore != reducer.CleanupAttempts
                || terminal)
            {
                PublishState(lastWireStep);
            }
        }

        internal void OnPipeEof()
        {
            CancelFromOwner("pipe EOF");
        }

        internal void OnReaderFault(string detail)
        {
            CancelFromOwner(String.IsNullOrWhiteSpace(detail) ? "pipe reader failed" : detail);
        }

        internal void OnHostCancel(string detail)
        {
            CancelFromOwner(String.IsNullOrWhiteSpace(detail) ? "runner cancelled helper" : detail);
        }

        internal void Tick()
        {
            RequireOwnerThread();
            if (first == null || terminal) return;
            ulong now = clock.NowTick;
            GesturePhase before = reducer.Phase;
            int attemptsBefore = reducer.CleanupAttempts;
            if (!appLease.IsAlive && !appDeathLatched)
            {
                appDeathLatched = true;
                ApplyAction(reducer.Cancel("exact application process exited", now), lastWireStep);
            }
            if (terminal)
            {
                PublishState(lastWireStep);
                return;
            }
            DriverAction action = reducer.AdvanceDeadline(now);
            ApplyAction(action, lastWireStep);
            if (!terminal
                && reducer.Phase == GesturePhase.CleanupRequired
                && now >= nextCleanupRetryTick)
            {
                ApplyAction(reducer.PlanCleanupRetry(now), lastWireStep);
            }
            if (before != reducer.Phase
                || attemptsBefore != reducer.CleanupAttempts
                || terminal)
            {
                PublishState(lastWireStep);
            }
        }

        private void Begin(ButtonWireRequest request)
        {
            if (request.Kind != ButtonWireCommandKind.BeginGesture || request.Step != 1)
                throw new ButtonWireProtocolException("first command must be begin step 1");
            if (request.DeadlineTick <= clock.NowTick)
                throw new ButtonWireProtocolException("gesture deadline already expired");
            if (request.AppProcessId != appLease.ProcessId
                || request.AppCreationIdentity != appLease.CreationIdentity)
            {
                throw new ButtonWireProtocolException("application process lease mismatch");
            }
            if (!appLease.IsAlive)
                throw new ButtonWireProtocolException("application process lease is not alive");
            ButtonGestureTuple owned = new ButtonGestureTuple(request);

            first = owned;
            lastWireStep = request.Step;
            reducer = new ButtonGestureReducer(
                owned.GestureId,
                owned.DeadlineTick,
                cleanupBudgetTicks);
            DriverAction action = reducer.BeginDown(clock.NowTick);
            if (action != DriverAction.SendDown)
            {
                ApplyAction(action, request.Step);
                PublishState(request.Step);
                return;
            }
            ButtonSendAttempt attempt = input.SendNormal(
                new ButtonNormalSendRequest(ButtonNormalSendEdge.Down, owned, owned.DownTag),
                delegate
                {
                    return CheckNormalSendPermission(owned, GesturePhase.SendingDown);
                });
            // The reducer records the outstanding release before a reply can be queued.
            ApplyAction(
                reducer.RecordDownInsertion(
                    attempt.Inserted,
                    clock.NowTick,
                    attempt.FailureDetail),
                request.Step);
            ApplyPostCallFault(attempt, "ButtonDown", request.Step);
            PublishState(request.Step);
        }

        private void ValidateContinuation(ButtonWireRequest request)
        {
            if (!first.Matches(request))
                throw new ButtonWireProtocolException("immutable gesture tuple changed");
            if (request.Step != lastWireStep + 1)
                throw new ButtonWireProtocolException("wire step was duplicate or out of order");
            if (!appLease.IsAlive)
            {
                appDeathLatched = true;
                ApplyAction(
                    reducer.Cancel(
                        "exact application process exited before queued command",
                        clock.NowTick),
                    lastWireStep);
                PublishState(lastWireStep);
                throw new ButtonWireProtocolException(
                    "exact application process lease is not alive for queued command");
            }
            if (request.DeadlineTick <= clock.NowTick)
            {
                ApplyAction(reducer.Cancel("wire command arrived after deadline", clock.NowTick), lastWireStep);
                PublishState(lastWireStep);
                throw new ButtonWireProtocolException("wire command arrived after deadline");
            }
        }

        private void ValidateCommandForPhase(ButtonWireCommandKind kind)
        {
            if (kind == ButtonWireCommandKind.CancelGesture
                || kind == ButtonWireCommandKind.Shutdown)
            {
                return;
            }
            bool valid = (reducer.Phase == GesturePhase.HeldAwaitingDownReceipt
                    && kind == ButtonWireCommandKind.DownDispatchReceipt)
                || (reducer.Phase == GesturePhase.HeldReadyForUp
                    && kind == ButtonWireCommandKind.ReleaseGesture)
                || ((reducer.Phase == GesturePhase.UpInsertedAwaitingReceiptAndOs
                        || reducer.Phase == GesturePhase.UpInsertedAwaitingReceipt)
                    && kind == ButtonWireCommandKind.UpSemanticReceipt);
            if (!valid)
                throw new ButtonWireProtocolException(
                    "command " + kind + " is invalid in phase " + reducer.Phase);
        }

        private void HandleDownReceipt(ButtonWireRequest request)
        {
            ButtonGestureTuple observedGesture = first;
            GesturePhase observedPhase = reducer.Phase;
            ObservedButtonDownAnchor observedAnchor = null;
            string observationFailure = null;
            if (request.RawDeliveryObserved)
            {
                TaggedButtonDelivery delivery = DeliveryFromRequest(
                    request,
                    ButtonTaggedEdgeKind.Down);
                ButtonDownObservationResult observed = deliveryObserver.ObserveTaggedDown(
                    first,
                    delivery);
                ButtonDownObserved valid = observed as ButtonDownObserved;
                if (valid != null
                    && Object.ReferenceEquals(valid.Anchor.Gesture, first)
                    && Object.ReferenceEquals(valid.Anchor.Delivery, delivery))
                {
                    if (downAnchor != null)
                        throw new ButtonWireProtocolException("Down anchor was already established");
                    observedAnchor = valid.Anchor;
                }
                else
                {
                    ButtonDownUnconfirmed unavailable = observed as ButtonDownUnconfirmed;
                    observationFailure = unavailable == null
                        ? "Down observation returned an unknown result"
                        : unavailable.Kind + ": " + unavailable.Detail;
                }
            }
            else
            {
                observationFailure = "raw tagged ButtonDown delivery was not observed";
            }
            if (!RevalidateNormalObservationCommit(
                observedGesture,
                observedPhase,
                request.Step,
                "ButtonDown observation"))
            {
                return;
            }
            if (observedAnchor != null)
                downAnchor = observedAnchor;
            bool accepted = request.ReceiptAccepted && downAnchor != null;
            AsyncOutcome outcome = reducer.RecordDownReceipt(
                request.GestureId,
                accepted,
                accepted
                    ? null
                    : (observationFailure ?? "application rejected ButtonDown dispatch"),
                clock.NowTick);
            ApplyAction(outcome.Action, request.Step);
            PublishState(request.Step);
        }

        private void HandleRelease(ButtonWireRequest request)
        {
            if (!appLease.IsAlive)
            {
                ApplyAction(reducer.Cancel("exact application process exited before ButtonUp", clock.NowTick), request.Step);
                PublishState(request.Step);
                return;
            }
            DriverAction action = reducer.BeginUp(clock.NowTick);
            if (action != DriverAction.SendUp)
            {
                ApplyAction(action, request.Step);
                PublishState(request.Step);
                return;
            }
            ButtonGestureTuple owned = first;
            ButtonSendAttempt attempt = input.SendNormal(
                new ButtonNormalSendRequest(ButtonNormalSendEdge.Up, owned, owned.UpTag),
                delegate
                {
                    return CheckNormalSendPermission(owned, GesturePhase.SendingUp);
                });
            ApplyAction(
                reducer.RecordUpInsertion(
                    attempt.Inserted,
                    clock.NowTick,
                    attempt.FailureDetail),
                request.Step);
            ApplyPostCallFault(attempt, "ButtonUp", request.Step);
            PublishState(request.Step);
        }

        private void HandleUpReceipt(ButtonWireRequest request)
        {
            ButtonGestureTuple observedGesture = first;
            GesturePhase observedPhase = reducer.Phase;
            ButtonReleaseObserved valid = null;
            if (request.RawDeliveryObserved && downAnchor != null)
            {
                ButtonReleaseObservationResult observed = deliveryObserver.ObserveNormalUp(
                    downAnchor,
                    new NormalButtonUpInsertion(first.GestureId, first.UpTag, 1),
                    DeliveryFromRequest(request, ButtonTaggedEdgeKind.Up));
                valid = observed as ButtonReleaseObserved;
            }
            if (!RevalidateNormalObservationCommit(
                observedGesture,
                observedPhase,
                request.Step,
                "ButtonUp observation"))
            {
                return;
            }
            if (valid != null)
                ApplyReleaseEvidence(valid.Evidence);
            AsyncOutcome outcome = reducer.RecordUpReceipt(
                request.GestureId,
                request.ReceiptAccepted,
                request.ReceiptAccepted ? null : "application rejected ButtonUp",
                clock.NowTick);
            ApplyAction(outcome.Action, request.Step);
            PublishState(request.Step);
        }

        private bool RevalidateNormalObservationCommit(
            ButtonGestureTuple observedGesture,
            GesturePhase observedPhase,
            uint step,
            string operation)
        {
            ulong now = clock.NowTick;
            if (!Object.ReferenceEquals(first, observedGesture)
                || reducer.Phase != observedPhase)
            {
                ApplyAction(
                    reducer.Cancel(operation + " no longer belongs to the current phase", now),
                    step);
                PublishState(step);
                return false;
            }
            if (!appLease.IsAlive)
            {
                appDeathLatched = true;
                ApplyAction(
                    reducer.Cancel(
                        "exact application process exited before " + operation + " commit",
                        now),
                    step);
                PublishState(step);
                return false;
            }
            if (now >= first.DeadlineTick)
            {
                ApplyAction(reducer.AdvanceDeadline(now), step);
                PublishState(step);
                return false;
            }
            return true;
        }

        private void ApplyReleaseEvidence(ButtonReleaseEvidence evidence)
        {
            if (evidence == null || downAnchor == null) return;
            if (!Object.ReferenceEquals(evidence.Anchor, downAnchor)) return;
            ButtonObservationScope expected;
            if (reducer.Phase == GesturePhase.CleanupUpInsertedAwaitingOs)
            {
                expected = ButtonObservationScope.OwnedCleanup;
            }
            else if (reducer.Phase == GesturePhase.UpInsertedAwaitingReceiptAndOs
                || reducer.Phase == GesturePhase.UpInsertedAwaitingOs)
            {
                expected = ButtonObservationScope.Normal;
            }
            else
            {
                return;
            }
            if (ButtonEvidenceDispatch.Classify(expected, first.GestureId, evidence)
                != ButtonEvidenceDispatchDisposition.Accepted)
            {
                return;
            }
            AsyncOutcome outcome = expected == ButtonObservationScope.OwnedCleanup
                ? reducer.RecordCleanupOsObservation(
                    first.GestureId,
                    OsButtonObservation.ReleasedAfterObservedDown,
                    clock.NowTick)
                : reducer.RecordUpOsObservation(
                    first.GestureId,
                    OsButtonObservation.ReleasedAfterObservedDown,
                    clock.NowTick);
            ApplyAction(outcome.Action, lastWireStep);
        }

        private static TaggedButtonDelivery DeliveryFromRequest(
            ButtonWireRequest request,
            ButtonTaggedEdgeKind edge)
        {
            ButtonWireDelivery source = request.Delivery;
            return new TaggedButtonDelivery(
                request.GestureId,
                source.ObservedTag,
                edge,
                source.ReceiverHwnd,
                source.ReceiverProcessId,
                source.ReceiverThreadId,
                source.ActualClientX,
                source.ActualClientY,
                source.ActualScreenX,
                source.ActualScreenY);
        }

        private void CancelFromOwner(string reason)
        {
            RequireOwnerThread();
            if (first == null || terminal) return;
            ApplyAction(reducer.Cancel(reason, clock.NowTick), lastWireStep);
            PublishState(lastWireStep);
        }

        private void ApplyAction(DriverAction action, uint step)
        {
            while (true)
            {
                switch (action)
                {
                    case DriverAction.None:
                        return;
                    case DriverAction.SendCleanupUp:
                        ButtonSendAttempt attempt = input.SendOwnedCleanupUp(
                            new ButtonCleanupSendRequest(first, first.UpTag));
                        int inserted = attempt.Inserted;
                        action = reducer.RecordCleanupInsertion(
                            inserted,
                            inserted == 1 ? null : attempt.FailureDetail);
                        nextCleanupRetryTick = SaturatingAdd(clock.NowTick, 1);
                        if (inserted == 1
                            && String.IsNullOrWhiteSpace(attempt.PostCallFault)
                            && downAnchor != null)
                        {
                            ButtonReleaseObservationResult observed =
                                deliveryObserver.ObserveOwnedCleanupUp(
                                    downAnchor,
                                    new OwnedCleanupButtonUpInsertion(
                                        first.GestureId,
                                        first.UpTag,
                                        inserted));
                            ButtonReleaseObserved valid = observed as ButtonReleaseObserved;
                            if (valid != null
                                && Object.ReferenceEquals(valid.Evidence.Anchor, downAnchor)
                                && ButtonEvidenceDispatch.Classify(
                                    ButtonObservationScope.OwnedCleanup,
                                    first.GestureId,
                                    valid.Evidence) == ButtonEvidenceDispatchDisposition.Accepted)
                            {
                                action = reducer.RecordCleanupOsObservation(
                                    first.GestureId,
                                    OsButtonObservation.ReleasedAfterObservedDown,
                                    clock.NowTick).Action;
                            }
                        }
                        continue;
                    case DriverAction.ReportSuccess:
                    case DriverAction.ReportFailure:
                        terminal = true;
                        return;
                    default:
                        throw new InvalidOperationException("unexpected reducer action: " + action);
                }
            }
        }

        private ButtonSendPermission CheckNormalSendPermission(
            ButtonGestureTuple expectedGesture,
            GesturePhase expectedPhase)
        {
            if (!Object.ReferenceEquals(first, expectedGesture))
                return ButtonSendPermission.WrongGesture;
            if (reducer.Phase != expectedPhase)
                return ButtonSendPermission.WrongPhase;
            if (!appLease.IsAlive)
                return ButtonSendPermission.ProcessExited;
            if (clock.NowTick >= expectedGesture.DeadlineTick)
                return ButtonSendPermission.DeadlineExpired;
            return ButtonSendPermission.Allowed;
        }

        private void ApplyPostCallFault(
            ButtonSendAttempt attempt,
            string operation,
            uint step)
        {
            if (attempt == null
                || String.IsNullOrWhiteSpace(attempt.PostCallFault)
                || terminal)
            {
                return;
            }
            ApplyAction(
                reducer.Cancel(
                    operation + " native scope cleanup failed after insertion: "
                        + attempt.PostCallFault,
                    clock.NowTick),
                step);
        }

        private void PublishState(uint step)
        {
            if (replyFaulted || first == null) return;
            ButtonWireReplyKind kind = terminal
                ? (reducer.Phase == GesturePhase.Succeeded
                    ? ButtonWireReplyKind.TerminalSuccess
                    : ButtonWireReplyKind.TerminalFailure)
                : ButtonWireReplyKind.Accepted;
            ButtonWireReply reply = new ButtonWireReply();
            reply.Kind = kind;
            reply.SessionNonce = sessionNonce;
            reply.GestureId = first.GestureId;
            reply.Step = step;
            reply.Phase = (uint)reducer.Phase;
            reply.Detail = (uint)reducer.CleanupAttempts;
            if (replies.TryPublish(reply)) return;

            replyFaulted = true;
            if (!terminal)
            {
                ApplyAction(reducer.Cancel("bounded reply queue is unavailable", clock.NowTick), step);
            }
        }

        private static string CancelText(uint flags)
        {
            ButtonCancelCode code = (ButtonCancelCode)flags;
            switch (code)
            {
                case ButtonCancelCode.RunnerCancelled: return "runner cancelled";
                case ButtonCancelCode.ApplicationExiting: return "application exiting";
                case ButtonCancelCode.ScenarioDeadline: return "scenario deadline reached";
                case ButtonCancelCode.PipeClosing: return "pipe closing";
                default: throw new ButtonWireProtocolException("invalid cancellation code");
            }
        }

        private void RequireOwnerThread()
        {
            if (Thread.CurrentThread.ManagedThreadId != ownerThreadId)
                throw new InvalidOperationException("button reducer must run on its owner thread");
        }

        private static ulong SaturatingAdd(ulong value, ulong addend)
        {
            ulong result = value + addend;
            return result < value ? UInt64.MaxValue : result;
        }
    }

    internal interface IButtonReplyTransport : IDisposable
    {
        void Write(byte[] frame, CancellationToken cancellationToken);
    }

    // The owner only performs a bounded TryAdd. A dedicated writer may block in
    // transport I/O, but it cannot call the input backend or reducer.
    internal sealed class BoundedAsyncReplySink : IButtonReplySink, IDisposable
    {
        private readonly BlockingCollection<byte[]> queue;
        private readonly IButtonReplyTransport transport;
        private readonly CancellationTokenSource cancellation;
        private readonly Thread writer;
        private volatile string fault;
        private bool stopRequested;
        private bool resourcesDisposed;

        internal BoundedAsyncReplySink(IButtonReplyTransport transport, int capacity)
        {
            if (transport == null) throw new ArgumentNullException("transport");
            if (capacity <= 0) throw new ArgumentOutOfRangeException("capacity");
            this.transport = transport;
            queue = new BlockingCollection<byte[]>(capacity);
            cancellation = new CancellationTokenSource();
            writer = new Thread(WriteLoop);
            writer.IsBackground = true;
            writer.Name = "miv-ui-smoke-button-reply-writer";
            writer.Start();
        }

        internal string Fault { get { return fault; } }

        public bool TryPublish(ButtonWireReply reply)
        {
            if (fault != null || queue.IsAddingCompleted) return false;
            byte[] payload = ButtonWireCodec.EncodeReply(reply);
            return queue.TryAdd(payload);
        }

        public void Dispose()
        {
            if (!TryJoinAndDispose(1000))
                throw new TimeoutException("button reply writer did not stop before disposal");
        }

        internal void RequestStop()
        {
            if (stopRequested) return;
            stopRequested = true;
            queue.CompleteAdding();
        }

        internal bool TryJoinAndDispose(int timeoutMilliseconds)
        {
            if (timeoutMilliseconds < 0)
                throw new ArgumentOutOfRangeException("timeoutMilliseconds");
            if (resourcesDisposed) return true;
            RequestStop();
            if (!writer.Join(timeoutMilliseconds))
            {
                cancellation.Cancel();
                if (!writer.Join(0)) return false;
            }
            transport.Dispose();
            cancellation.Dispose();
            queue.Dispose();
            resourcesDisposed = true;
            return true;
        }

        private void WriteLoop()
        {
            try
            {
                foreach (byte[] payload in queue.GetConsumingEnumerable(cancellation.Token))
                {
                    transport.Write(payload, cancellation.Token);
                }
            }
            catch (OperationCanceledException) { }
            catch (ObjectDisposedException) { }
            catch (Exception error)
            {
                fault = error.GetType().Name + ": " + error.Message;
            }
        }
    }
}
