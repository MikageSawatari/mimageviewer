using System;
using System.Runtime.InteropServices;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    public enum ButtonHelperReleaseState
    {
        Unknown,
        NoOwnedDown,
        ConfirmedReleased,
        ConfirmedOutstanding,
    }

    /// Runner-visible terminal snapshot.  A successful owner state is reported
    /// separately from the permission to terminate the App because an unresolved
    /// release obligation must always win over process cleanup.
    public sealed class ButtonHelperRunnerStatus
    {
        public bool Joined { get; internal set; }
        public bool SafeToTerminateApp { get; internal set; }
        public bool Succeeded { get; internal set; }
        public bool HasOutstandingRelease { get; internal set; }
        public ButtonHelperReleaseState ReleaseState { get; internal set; }
        public string TerminalKind { get; internal set; }
        public string OwnerPhase { get; internal set; }
        public string PrimaryFailure { get; internal set; }
        public string Detail { get; internal set; }
        public int CleanupAttempts { get; internal set; }
    }

    /// Public PowerShell facade.  All native input objects are constructed on
    /// the dedicated owner thread inside ButtonHelperHost.
    public sealed class ButtonHelperRunnerHandle
    {
        private ButtonHelperHost host;

        internal ButtonHelperRunnerHandle(ButtonHelperHost host)
        {
            this.host = host;
        }

        public ButtonHelperRunnerStatus CancelAndJoin(string reason, int timeoutMilliseconds)
        {
            if (host == null) throw new InvalidOperationException("button helper was already joined");
            if (timeoutMilliseconds <= 0) throw new ArgumentOutOfRangeException("timeoutMilliseconds");
            ulong deadline = SystemButtonClock.SaturatingAdd(
                SystemButtonClock.ReadNow(),
                unchecked((ulong)timeoutMilliseconds));
            ButtonHelperHostJoinResult result = host.CancelAndJoin(reason, deadline);
            ButtonHelperHostStillRunning running = result as ButtonHelperHostStillRunning;
            if (running != null)
            {
                host = running.RetainedHost;
                return StillRunningStatus(running.FirstCancellationReason);
            }

            ButtonHelperHostJoinedTerminal joined = result as ButtonHelperHostJoinedTerminal;
            if (joined == null) throw new InvalidOperationException("unknown button helper join result");
            host = null;
            ButtonHelperHostTerminal terminal = joined.Terminal;
            bool succeeded = terminal.Kind == ButtonHelperHostTerminalKind.Succeeded
                && terminal.OwnerTerminal
                && terminal.OwnerPhase == Miv.UiSmoke.ButtonDraft.GesturePhase.Succeeded
                && !terminal.HasOutstandingRelease;
            ButtonHelperReleaseState releaseState = terminal.HasOutstandingRelease
                ? ButtonHelperReleaseState.ConfirmedOutstanding
                : (!terminal.GestureBegan
                    || terminal.OwnerPhase == Miv.UiSmoke.ButtonDraft.GesturePhase.ReadyForDown
                    || terminal.OwnerPhase == Miv.UiSmoke.ButtonDraft.GesturePhase.SendingDown
                    || terminal.OwnerPhase == Miv.UiSmoke.ButtonDraft.GesturePhase.FailedBeforeDown
                        ? ButtonHelperReleaseState.NoOwnedDown
                        : ButtonHelperReleaseState.ConfirmedReleased);
            return new ButtonHelperRunnerStatus
            {
                Joined = true,
                SafeToTerminateApp = !terminal.HasOutstandingRelease,
                Succeeded = succeeded,
                HasOutstandingRelease = terminal.HasOutstandingRelease,
                ReleaseState = releaseState,
                TerminalKind = terminal.Kind.ToString(),
                OwnerPhase = terminal.OwnerPhase.ToString(),
                PrimaryFailure = terminal.PrimaryFailure,
                Detail = terminal.Detail,
                CleanupAttempts = terminal.CleanupAttempts
            };
        }

        internal static ButtonHelperRunnerStatus StillRunningStatus(string firstReason)
        {
            return new ButtonHelperRunnerStatus
            {
                Joined = false,
                SafeToTerminateApp = false,
                Succeeded = false,
                // The owner has not joined, so the runner cannot truthfully say
                // whether a release obligation exists. Unknown still closes the
                // App-kill interlock without inventing typed evidence.
                HasOutstandingRelease = false,
                ReleaseState = ButtonHelperReleaseState.Unknown,
                TerminalKind = "StillRunning",
                PrimaryFailure = firstReason,
                Detail = "fixed helper join deadline expired"
            };
        }
    }

    public static class ButtonHelperRunnerApi
    {
        // Runner process/App failures are primary.  Helper cleanup/protocol
        // success can only add evidence; it cannot overwrite an App timeout or
        // nonzero exit observed first by the PowerShell owner.
        public static bool ScenarioSucceeded(
            bool appTimedOut,
            int appExitCode,
            ButtonHelperRunnerStatus helper)
        {
            return !appTimedOut
                && appExitCode == 0
                && helper != null
                && helper.Joined
                && helper.SafeToTerminateApp
                && helper.Succeeded
                && !helper.HasOutstandingRelease
                && helper.ReleaseState == ButtonHelperReleaseState.ConfirmedReleased;
        }

        public static ButtonHelperRunnerHandle Start(
            string pipeName,
            string sessionNonce,
            uint expectedAppProcessId,
            int acceptTimeoutMilliseconds)
        {
            Guid session;
            if (!Guid.TryParseExact(sessionNonce, "D", out session) || session == Guid.Empty)
                throw new ArgumentException("button helper session nonce is invalid", "sessionNonce");
            if (acceptTimeoutMilliseconds <= 0)
                throw new ArgumentOutOfRangeException("acceptTimeoutMilliseconds");
            SystemButtonClock clock = new SystemButtonClock();
            ButtonHelperHostOptions options = new ButtonHelperHostOptions(
                pipeName,
                session,
                expectedAppProcessId,
                SystemButtonClock.SaturatingAdd(
                    clock.NowTick,
                    unchecked((ulong)acceptTimeoutMilliseconds)),
                2000,
                25,
                8,
                8);
            return new ButtonHelperRunnerHandle(
                ButtonHelperHost.Start(options, clock, new NativeButtonHostComponentFactory()));
        }
    }

    internal sealed class SystemButtonClock : IButtonClock
    {
        public ulong NowTick { get { return ReadNow(); } }

        internal static ulong ReadNow()
        {
            return ButtonHelperRunnerNative.GetTickCount64();
        }

        internal static ulong SaturatingAdd(ulong value, ulong addend)
        {
            ulong result = value + addend;
            return result < value ? UInt64.MaxValue : result;
        }
    }

    internal sealed class NativeButtonHostComponentFactory : IButtonHelperHostComponentFactory
    {
        public IAppProcessLease OpenAppLease(uint expectedProcessId)
        {
            return new PinnedAppProcessLease(expectedProcessId);
        }

        public ButtonHelperHostOwnerComponents CreateOwnerComponents()
        {
            NativeButtonObservationPlatform platform = new NativeButtonObservationPlatform();
            ButtonOsObserver observer = new ButtonOsObserver(platform);
            Win32ButtonInputBackend input = new Win32ButtonInputBackend(
                new NativeButtonInputFactSource(platform),
                new NativeButtonEventInserter());
            return new ButtonHelperHostOwnerComponents(
                input,
                observer,
                EmptyButtonReleaseEvidenceSource.Instance);
        }

        public IButtonReplyTransport CreateReplyTransport(System.IO.Stream stream)
        {
            return new PipeReplyTransport(stream);
        }
    }

    internal sealed class EmptyButtonReleaseEvidenceSource : IButtonReleaseEvidenceSource
    {
        internal static readonly EmptyButtonReleaseEvidenceSource Instance =
            new EmptyButtonReleaseEvidenceSource();

        public bool TryTake(out ButtonReleaseEvidence evidence)
        {
            evidence = null;
            return false;
        }
    }

    internal static class ButtonHelperRunnerNative
    {
        [DllImport("kernel32.dll")]
        internal static extern ulong GetTickCount64();
    }
}
