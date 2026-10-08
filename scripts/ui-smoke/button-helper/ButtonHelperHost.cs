using System;
using System.Threading;
using Miv.UiSmoke.ButtonDraft;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    internal sealed class ButtonHelperHostOwnerComponents
    {
        internal readonly IButtonInputBackend Input;
        internal readonly IButtonDeliveryObserver DeliveryObserver;
        internal readonly IButtonReleaseEvidenceSource ReleaseEvidence;

        internal ButtonHelperHostOwnerComponents(
            IButtonInputBackend input,
            IButtonDeliveryObserver deliveryObserver,
            IButtonReleaseEvidenceSource releaseEvidence)
        {
            if (input == null) throw new ArgumentNullException("input");
            if (deliveryObserver == null) throw new ArgumentNullException("deliveryObserver");
            if (releaseEvidence == null) throw new ArgumentNullException("releaseEvidence");
            Input = input;
            DeliveryObserver = deliveryObserver;
            ReleaseEvidence = releaseEvidence;
        }
    }

    internal interface IButtonHelperHostComponentFactory
    {
        IAppProcessLease OpenAppLease(uint expectedProcessId);
        ButtonHelperHostOwnerComponents CreateOwnerComponents();
        IButtonReplyTransport CreateReplyTransport(System.IO.Stream stream);
    }

    internal sealed class ButtonHelperHostOptions
    {
        internal readonly string PipeName;
        internal readonly Guid SessionNonce;
        internal readonly uint ExpectedAppProcessId;
        internal readonly ulong AcceptDeadlineTick;
        internal readonly ulong CleanupBudgetTicks;
        internal readonly int MaximumLoopWaitMilliseconds;
        internal readonly int RequestQueueCapacity;
        internal readonly int ReplyQueueCapacity;

        internal ButtonHelperHostOptions(
            string pipeName,
            Guid sessionNonce,
            uint expectedAppProcessId,
            ulong acceptDeadlineTick,
            ulong cleanupBudgetTicks,
            int maximumLoopWaitMilliseconds,
            int requestQueueCapacity,
            int replyQueueCapacity)
        {
            if (String.IsNullOrWhiteSpace(pipeName))
                throw new ArgumentException("pipe name is missing", "pipeName");
            if (sessionNonce == Guid.Empty)
                throw new ArgumentException("session nonce is empty", "sessionNonce");
            if (expectedAppProcessId == 0)
                throw new ArgumentOutOfRangeException("expectedAppProcessId");
            if (acceptDeadlineTick == 0)
                throw new ArgumentOutOfRangeException("acceptDeadlineTick");
            if (cleanupBudgetTicks == 0)
                throw new ArgumentOutOfRangeException("cleanupBudgetTicks");
            if (maximumLoopWaitMilliseconds <= 0 || maximumLoopWaitMilliseconds > 100)
                throw new ArgumentOutOfRangeException("maximumLoopWaitMilliseconds");
            if (requestQueueCapacity <= 0)
                throw new ArgumentOutOfRangeException("requestQueueCapacity");
            if (replyQueueCapacity <= 0)
                throw new ArgumentOutOfRangeException("replyQueueCapacity");
            PipeName = pipeName;
            SessionNonce = sessionNonce;
            ExpectedAppProcessId = expectedAppProcessId;
            AcceptDeadlineTick = acceptDeadlineTick;
            CleanupBudgetTicks = cleanupBudgetTicks;
            MaximumLoopWaitMilliseconds = maximumLoopWaitMilliseconds;
            RequestQueueCapacity = requestQueueCapacity;
            ReplyQueueCapacity = replyQueueCapacity;
        }
    }

    internal enum ButtonHelperHostTerminalKind
    {
        Succeeded,
        GestureFailed,
        CancelledBeforeConnection,
        AppExitedBeforeConnection,
        AcceptTimedOut,
        CancelledBeforeGesture,
        AppExitedBeforeGesture,
        ConnectionFailed,
        UnsafeOwnerFault,
    }

    internal sealed class ButtonHelperHostTerminal
    {
        internal readonly ButtonHelperHostTerminalKind Kind;
        internal readonly bool GestureBegan;
        internal readonly bool OwnerTerminal;
        internal readonly GesturePhase OwnerPhase;
        internal readonly string PrimaryFailure;
        internal readonly int CleanupAttempts;
        internal readonly bool HasOutstandingRelease;
        internal readonly string Detail;

        internal ButtonHelperHostTerminal(
            ButtonHelperHostTerminalKind kind,
            bool gestureBegan,
            bool ownerTerminal,
            GesturePhase ownerPhase,
            string primaryFailure,
            int cleanupAttempts,
            bool hasOutstandingRelease,
            string detail)
        {
            Kind = kind;
            GestureBegan = gestureBegan;
            OwnerTerminal = ownerTerminal;
            OwnerPhase = ownerPhase;
            PrimaryFailure = primaryFailure;
            CleanupAttempts = cleanupAttempts;
            HasOutstandingRelease = hasOutstandingRelease;
            Detail = detail;
        }

        internal bool SafeToCollect
        {
            get { return !GestureBegan || OwnerTerminal; }
        }
    }

    internal abstract class ButtonHelperHostJoinResult
    {
    }

    internal sealed class ButtonHelperHostJoinedTerminal : ButtonHelperHostJoinResult
    {
        internal readonly ButtonHelperHostTerminal Terminal;

        internal ButtonHelperHostJoinedTerminal(ButtonHelperHostTerminal terminal)
        {
            Terminal = terminal;
        }
    }

    internal sealed class ButtonHelperHostStillRunning : ButtonHelperHostJoinResult
    {
        internal readonly ButtonHelperHost RetainedHost;
        internal readonly ulong FixedJoinDeadlineTick;
        internal readonly string FirstCancellationReason;

        internal ButtonHelperHostStillRunning(
            ButtonHelperHost retainedHost,
            ulong fixedJoinDeadlineTick,
            string firstCancellationReason)
        {
            RetainedHost = retainedHost;
            FixedJoinDeadlineTick = fixedJoinDeadlineTick;
            FirstCancellationReason = firstCancellationReason;
        }
    }

    internal sealed class ButtonHostCancellationLatch : IButtonHostCancellationSource
    {
        private readonly object gate = new object();
        private string firstReason;

        internal string FirstReason
        {
            get { lock (gate) return firstReason; }
        }

        internal void Request(string reason)
        {
            if (String.IsNullOrWhiteSpace(reason)) reason = "runner cancelled helper";
            lock (gate)
            {
                if (firstReason == null) firstReason = reason;
            }
        }

        public bool TryGetReason(out string reason)
        {
            lock (gate)
            {
                reason = firstReason;
                return reason != null;
            }
        }
    }

    // The runner-facing object never owns input. Its dedicated thread constructs
    // every native/owner component, and CancelAndJoin only publishes a typed
    // cancellation request and joins retained resources.
    internal sealed class ButtonHelperHost
    {
        private readonly object gate = new object();
        private readonly ButtonHelperHostOptions options;
        private readonly IButtonClock clock;
        private readonly IButtonHelperHostComponentFactory components;
        private readonly ButtonHostCancellationLatch cancellation;
        private readonly Thread ownerThread;
        private LocalButtonPipeServer server;
        private BoundedPipeRequestReader reader;
        private BoundedAsyncReplySink replies;
        private IAppProcessLease appLease;
        private ButtonHelperHostTerminal terminal;
        private ulong? fixedJoinDeadlineTick;
        private bool collected;

        private ButtonHelperHost(
            ButtonHelperHostOptions options,
            IButtonClock clock,
            IButtonHelperHostComponentFactory components)
        {
            if (options == null) throw new ArgumentNullException("options");
            if (clock == null) throw new ArgumentNullException("clock");
            if (components == null) throw new ArgumentNullException("components");
            this.options = options;
            this.clock = clock;
            this.components = components;
            cancellation = new ButtonHostCancellationLatch();
            ownerThread = new Thread(RunOwnerThread);
            ownerThread.IsBackground = false;
            ownerThread.Name = "miv-ui-smoke-button-owner";
        }

        internal static ButtonHelperHost Start(
            ButtonHelperHostOptions options,
            IButtonClock clock,
            IButtonHelperHostComponentFactory components)
        {
            ButtonHelperHost host = new ButtonHelperHost(options, clock, components);
            host.ownerThread.Start();
            return host;
        }

        internal ButtonHelperHostJoinResult CancelAndJoin(
            string reason,
            ulong absoluteJoinDeadlineTick)
        {
            lock (gate)
            {
                if (collected)
                    throw new InvalidOperationException("button helper host was already collected");
                if (!fixedJoinDeadlineTick.HasValue)
                {
                    if (absoluteJoinDeadlineTick == 0)
                        throw new ArgumentOutOfRangeException("absoluteJoinDeadlineTick");
                    fixedJoinDeadlineTick = absoluteJoinDeadlineTick;
                }
            }
            cancellation.Request(reason);

            ulong deadline = fixedJoinDeadlineTick.Value;
            int remaining = RemainingMilliseconds(deadline);
            if (!ownerThread.Join(remaining))
                return StillRunning(deadline);

            ButtonHelperHostTerminal snapshot;
            lock (gate) snapshot = terminal;
            if (snapshot == null || !snapshot.SafeToCollect)
                return StillRunning(deadline);

            if (replies != null)
            {
                if (!replies.TryJoinAndDispose(RemainingMilliseconds(deadline)))
                    return StillRunning(deadline);
                snapshot = ApplyReplyTransportFault(snapshot, replies.Fault);
            }
            if (reader != null && !reader.TryJoinAndDispose(RemainingMilliseconds(deadline)))
                return StillRunning(deadline);
            if (server != null && !server.TryJoinAndDispose(RemainingMilliseconds(deadline)))
                return StillRunning(deadline);

            IDisposable disposableLease = appLease as IDisposable;
            if (disposableLease != null) disposableLease.Dispose();
            lock (gate) collected = true;
            return new ButtonHelperHostJoinedTerminal(snapshot);
        }

        // The owner reaching Succeeded only proves that it accepted both App
        // receipts and OS release evidence.  A terminal reply which never left
        // the bounded writer cannot be reported to the runner as protocol
        // success; the App-side timeout/failure remains authoritative.
        internal static ButtonHelperHostTerminal ApplyReplyTransportFault(
            ButtonHelperHostTerminal terminal,
            string replyFault)
        {
            if (terminal == null || String.IsNullOrWhiteSpace(replyFault)) return terminal;
            return new ButtonHelperHostTerminal(
                ButtonHelperHostTerminalKind.GestureFailed,
                terminal.GestureBegan,
                terminal.OwnerTerminal,
                terminal.OwnerPhase,
                String.IsNullOrWhiteSpace(terminal.PrimaryFailure)
                    ? "button helper reply transport failed"
                    : terminal.PrimaryFailure,
                terminal.CleanupAttempts,
                terminal.HasOutstandingRelease,
                terminal.Detail + "; reply_transport=" + replyFault);
        }

        private ButtonHelperHostStillRunning StillRunning(ulong deadline)
        {
            return new ButtonHelperHostStillRunning(
                this,
                deadline,
                cancellation.FirstReason);
        }

        private void RunOwnerThread()
        {
            ButtonHelperOwner owner = null;
            bool authenticated = false;
            ButtonHelperHostTerminal result = null;
            try
            {
                appLease = components.OpenAppLease(options.ExpectedAppProcessId);
                if (appLease == null
                    || appLease.ProcessId != options.ExpectedAppProcessId
                    || appLease.CreationIdentity == 0)
                {
                    throw new InvalidOperationException("component factory returned the wrong App lease");
                }
                server = LocalButtonPipeServer.Create(
                    options.PipeName,
                    options.ExpectedAppProcessId);
                server.BeginWaitForAuthenticatedConnection();
                while (!authenticated)
                {
                    string cancelReason;
                    if (cancellation.TryGetReason(out cancelReason))
                    {
                        result = BeforeGesture(
                            ButtonHelperHostTerminalKind.CancelledBeforeConnection,
                            cancelReason);
                        break;
                    }
                    if (!appLease.IsAlive)
                    {
                        result = BeforeGesture(
                            ButtonHelperHostTerminalKind.AppExitedBeforeConnection,
                            "exact application process exited before pipe connection");
                        break;
                    }
                    int remaining = RemainingMilliseconds(options.AcceptDeadlineTick);
                    if (remaining == 0)
                    {
                        result = BeforeGesture(
                            ButtonHelperHostTerminalKind.AcceptTimedOut,
                            "fixed pipe accept deadline expired");
                        break;
                    }
                    authenticated = server.PollAuthenticatedConnection(Math.Min(50, remaining));
                }

                if (result == null)
                {
                    ButtonHelperHostOwnerComponents ownerComponents =
                        components.CreateOwnerComponents();
                    reader = new BoundedPipeRequestReader(
                        server.Stream,
                        options.RequestQueueCapacity);
                    replies = new BoundedAsyncReplySink(
                        components.CreateReplyTransport(server.Stream),
                        options.ReplyQueueCapacity);
                    owner = new ButtonHelperOwner(
                        options.SessionNonce,
                        appLease,
                        clock,
                        ownerComponents.Input,
                        ownerComponents.DeliveryObserver,
                        replies,
                        options.CleanupBudgetTicks);
                    ButtonHelperLoop loop = new ButtonHelperLoop(
                        owner,
                        reader,
                        ownerComponents.ReleaseEvidence,
                        options.MaximumLoopWaitMilliseconds,
                        cancellation,
                        appLease);
                    ButtonHelperLoopResult loopResult = loop.Run();
                    result = FromLoop(owner, loopResult);
                }
            }
            catch (Exception error)
            {
                bool began = owner != null && owner.HasBegun;
                bool ownerTerminal = owner != null && owner.IsTerminal;
                result = new ButtonHelperHostTerminal(
                    began && !ownerTerminal
                        ? ButtonHelperHostTerminalKind.UnsafeOwnerFault
                        : ButtonHelperHostTerminalKind.ConnectionFailed,
                    began,
                    ownerTerminal,
                    owner == null ? GesturePhase.ReadyForDown : owner.Phase,
                    owner == null ? null : owner.PrimaryFailure,
                    owner == null ? 0 : owner.CleanupAttempts,
                    owner != null && owner.HasOutstandingRelease,
                    error.GetType().Name + ": " + error.Message);
            }
            finally
            {
                // Transport collection belongs to CancelAndJoin.  In particular,
                // the owner thread must not close the shared pipe before the
                // asynchronous writer drains its terminal reply. UnsafeOwnerFault
                // leaves every object retained and cannot authorize App kill.
                lock (gate) terminal = result;
            }
        }

        private ButtonHelperHostTerminal FromLoop(
            ButtonHelperOwner owner,
            ButtonHelperLoopResult loopResult)
        {
            ButtonHelperHostTerminalKind kind;
            switch (loopResult)
            {
                case ButtonHelperLoopResult.Succeeded:
                    kind = ButtonHelperHostTerminalKind.Succeeded;
                    break;
                case ButtonHelperLoopResult.CancelledBeforeGesture:
                    kind = ButtonHelperHostTerminalKind.CancelledBeforeGesture;
                    break;
                case ButtonHelperLoopResult.AppExitedBeforeGesture:
                    kind = ButtonHelperHostTerminalKind.AppExitedBeforeGesture;
                    break;
                default:
                    kind = ButtonHelperHostTerminalKind.GestureFailed;
                    break;
            }
            return new ButtonHelperHostTerminal(
                kind,
                owner.HasBegun,
                owner.IsTerminal,
                owner.Phase,
                owner.PrimaryFailure,
                owner.CleanupAttempts,
                owner.HasOutstandingRelease,
                loopResult.ToString());
        }

        private static ButtonHelperHostTerminal BeforeGesture(
            ButtonHelperHostTerminalKind kind,
            string detail)
        {
            return new ButtonHelperHostTerminal(
                kind,
                false,
                false,
                GesturePhase.ReadyForDown,
                null,
                0,
                false,
                detail);
        }

        private int RemainingMilliseconds(ulong deadline)
        {
            ulong now = clock.NowTick;
            if (deadline <= now) return 0;
            ulong remaining = deadline - now;
            return remaining > Int32.MaxValue ? Int32.MaxValue : unchecked((int)remaining);
        }
    }
}
