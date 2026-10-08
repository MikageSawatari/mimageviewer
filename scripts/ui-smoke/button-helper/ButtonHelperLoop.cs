using System;
using Miv.UiSmoke.ButtonDraft;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    internal interface IButtonReleaseEvidenceSource
    {
        bool TryTake(out ButtonReleaseEvidence evidence);
    }

    internal interface IButtonHostCancellationSource
    {
        bool TryGetReason(out string reason);
    }

    internal enum ButtonHelperLoopResult
    {
        Succeeded,
        GestureFailed,
        ConnectionEndedBeforeGesture,
        ProtocolFailedBeforeGesture,
        CancelledBeforeGesture,
        AppExitedBeforeGesture,
    }

    // This loop runs on the same thread that constructed ButtonHelperOwner.
    // Pipe reader/writer threads have no input backend. Each turn samples
    // process/deadline state before and after at most one parsed request and one
    // typed OS observation, then blocks only on the reader activity event for a
    // short bounded interval.
    internal sealed class ButtonHelperLoop
    {
        private readonly ButtonHelperOwner owner;
        private readonly BoundedPipeRequestReader reader;
        private readonly IButtonReleaseEvidenceSource osEvidence;
        private readonly int maximumWaitMilliseconds;
        private readonly IButtonHostCancellationSource hostCancellation;
        private readonly IAppProcessLease appLease;
        private bool sawGesture;
        private bool connectionEnded;
        private bool hostCancellationDelivered;

        internal ButtonHelperLoop(
            ButtonHelperOwner owner,
            BoundedPipeRequestReader reader,
            IButtonReleaseEvidenceSource osEvidence,
            int maximumWaitMilliseconds)
            : this(owner, reader, osEvidence, maximumWaitMilliseconds, null, null)
        {
        }

        internal ButtonHelperLoop(
            ButtonHelperOwner owner,
            BoundedPipeRequestReader reader,
            IButtonReleaseEvidenceSource osEvidence,
            int maximumWaitMilliseconds,
            IButtonHostCancellationSource hostCancellation,
            IAppProcessLease appLease)
        {
            if (owner == null) throw new ArgumentNullException("owner");
            if (reader == null) throw new ArgumentNullException("reader");
            if (osEvidence == null) throw new ArgumentNullException("osEvidence");
            if (maximumWaitMilliseconds <= 0 || maximumWaitMilliseconds > 100)
                throw new ArgumentOutOfRangeException("maximumWaitMilliseconds");
            this.owner = owner;
            this.reader = reader;
            this.osEvidence = osEvidence;
            this.maximumWaitMilliseconds = maximumWaitMilliseconds;
            this.hostCancellation = hostCancellation;
            this.appLease = appLease;
        }

        internal ButtonHelperLoopResult Run()
        {
            while (true)
            {
                ButtonHelperLoopResult? external = ServiceExternalLifecycle();
                if (external.HasValue) return external.Value;
                // Sample the pinned process and fixed deadlines before consuming
                // any queued positive receipt.  A final App or OS receipt must
                // not turn an already-observable process death into success.
                owner.Tick();
                if (owner.IsTerminal) return ResultForTerminalOrFailure();

                bool servicedInput = false;
                if (!connectionEnded && reader.Fault != null)
                {
                    if (!sawGesture)
                        return ButtonHelperLoopResult.ProtocolFailedBeforeGesture;
                    owner.OnReaderFault("pipe reader failed: " + reader.Fault);
                    connectionEnded = true;
                    if (owner.IsTerminal) return ResultForTerminalOrFailure();
                }

                ButtonWireRequest request;
                if (reader.TryDequeue(out request))
                {
                    try
                    {
                        owner.Handle(request);
                        sawGesture = true;
                    }
                    catch (ButtonWireProtocolException error)
                    {
                        if (!sawGesture)
                            return ButtonHelperLoopResult.ProtocolFailedBeforeGesture;
                        owner.OnReaderFault("wire protocol rejected: " + error.Message);
                        connectionEnded = true;
                        if (owner.IsTerminal) return ResultForTerminalOrFailure();
                    }
                    servicedInput = true;
                    if (owner.IsTerminal) return ResultForTerminalOrFailure();
                }


                external = ServiceExternalLifecycle();
                if (external.HasValue) return external.Value;

                ButtonReleaseEvidence evidence;
                if (osEvidence.TryTake(out evidence))
                {
                    owner.ObserveReleaseEvidence(evidence);
                    servicedInput = true;
                    if (owner.IsTerminal) return ResultForTerminalOrFailure();
                }

                // Lifecycle service is mandatory after every bounded dispatch turn.
                // A stream of irrelevant requests or OS samples must not postpone
                // application-death handling, gesture deadlines, or cleanup retry.
                owner.Tick();
                if (owner.IsTerminal) return ResultForTerminalOrFailure();

                // EOF is checked after draining requests that were fully decoded
                // before the reader observed stream closure.
                if (!connectionEnded && reader.Eof)
                {
                    // EOF is published after all decoded requests are queued,
                    // but it can race the first nonblocking dequeue above.
                    if (!reader.HasPending)
                    {
                        if (!sawGesture)
                            return ButtonHelperLoopResult.ConnectionEndedBeforeGesture;
                        owner.OnPipeEof();
                        connectionEnded = true;
                        if (owner.IsTerminal) return ResultForTerminalOrFailure();
                    }
                }
                if (servicedInput || (!connectionEnded && reader.HasPending)) continue;
                reader.WaitForActivity(maximumWaitMilliseconds);
            }
        }

        private ButtonHelperLoopResult? ServiceExternalLifecycle()
        {
            string reason;
            if (!hostCancellationDelivered
                && hostCancellation != null
                && hostCancellation.TryGetReason(out reason))
            {
                if (!owner.HasBegun) return ButtonHelperLoopResult.CancelledBeforeGesture;
                hostCancellationDelivered = true;
                connectionEnded = true;
                owner.OnHostCancel(reason);
                if (owner.IsTerminal) return ResultForTerminalOrFailure();
            }
            if (!owner.HasBegun && appLease != null && !appLease.IsAlive)
                return ButtonHelperLoopResult.AppExitedBeforeGesture;
            if (owner.HasBegun && appLease != null && !appLease.IsAlive)
            {
                connectionEnded = true;
                owner.Tick();
                if (owner.IsTerminal) return ResultForTerminalOrFailure();
            }
            return null;
        }

        private ButtonHelperLoopResult ResultForTerminalOrFailure()
        {
            return owner.IsTerminal && owner.Phase == GesturePhase.Succeeded
                ? ButtonHelperLoopResult.Succeeded
                : ButtonHelperLoopResult.GestureFailed;
        }
    }
}
