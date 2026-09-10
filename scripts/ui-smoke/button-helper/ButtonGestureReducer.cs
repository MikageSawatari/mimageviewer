using System;
using System.Collections.Generic;

namespace Miv.UiSmoke.ButtonDraft
{
    internal enum GesturePhase
    {
        ReadyForDown,
        SendingDown,
        HeldAwaitingDownReceipt,
        HeldReadyForUp,
        SendingUp,
        UpInsertedAwaitingReceiptAndOs,
        UpInsertedAwaitingReceipt,
        UpInsertedAwaitingOs,
        SendingCleanupUp,
        CleanupUpInsertedAwaitingOs,
        CleanupRequired,
        Succeeded,
        FailedBeforeDown,
        FailedAfterObservedUp,
        FailedAfterObservedCleanup,
        FailedCleanupUnresolved,
    }

    internal enum DriverAction
    {
        None,
        SendDown,
        SendUp,
        SendCleanupUp,
        ReportSuccess,
        ReportFailure,
    }

    internal enum AsyncDisposition
    {
        Applied,
        LateIgnored,
        WrongGesture,
    }

    // Only a producer that has separately validated the interactive desktop and
    // observed the Down delivery may construct ReleasedAfterObservedDown.
    // GetAsyncKeyState returning zero by itself is not this proof.
    internal enum OsButtonObservation
    {
        ReleasedAfterObservedDown,
        StillDownOnValidatedDesktop,
        DesktopUnavailable,
        Indeterminate,
    }

    internal sealed class AsyncOutcome
    {
        internal AsyncOutcome(AsyncDisposition disposition, DriverAction action)
        {
            Disposition = disposition;
            Action = action;
        }

        internal AsyncDisposition Disposition { get; private set; }
        internal DriverAction Action { get; private set; }
    }

    // This reducer is deliberately USER32-free. One helper thread owns it and
    // performs each returned send action synchronously before recording the
    // insertion result. Watchdogs enqueue cancellation reasons to that owner;
    // they never send ButtonUp themselves. A queued cancellation is processed
    // only after a synchronous send and its insertion result form one turn.
    internal sealed class ButtonGestureReducer
    {
        private readonly ulong gestureId;
        private readonly ulong gestureDeadlineTick;
        private readonly ulong cleanupBudgetTicks;
        private GesturePhase phase;
        private ulong? cleanupDeadlineTick;
        private string primaryFailure;
        private string lastCleanupFailure;
        private int cleanupAttempts;

        internal ButtonGestureReducer(
            ulong gestureId,
            ulong gestureDeadlineTick,
            ulong cleanupBudgetTicks)
        {
            if (gestureId == 0) throw new ArgumentOutOfRangeException("gestureId");
            if (cleanupBudgetTicks == 0) throw new ArgumentOutOfRangeException("cleanupBudgetTicks");
            this.gestureId = gestureId;
            this.gestureDeadlineTick = gestureDeadlineTick;
            this.cleanupBudgetTicks = cleanupBudgetTicks;
            phase = GesturePhase.ReadyForDown;
        }

        internal GesturePhase Phase { get { return phase; } }
        internal ulong GestureId { get { return gestureId; } }
        internal ulong? CleanupDeadlineTick { get { return cleanupDeadlineTick; } }
        internal string PrimaryFailure { get { return primaryFailure; } }
        internal string LastCleanupFailure { get { return lastCleanupFailure; } }
        internal int CleanupAttempts { get { return cleanupAttempts; } }

        internal bool HasOutstandingRelease
        {
            get
            {
                switch (phase)
                {
                    case GesturePhase.ReadyForDown:
                    case GesturePhase.SendingDown:
                    case GesturePhase.Succeeded:
                    case GesturePhase.FailedBeforeDown:
                    case GesturePhase.UpInsertedAwaitingReceipt:
                    case GesturePhase.FailedAfterObservedUp:
                    case GesturePhase.FailedAfterObservedCleanup:
                        return false;
                    default:
                        return true;
                }
            }
        }

        internal DriverAction BeginDown(ulong nowTick)
        {
            RequirePhase(GesturePhase.ReadyForDown);
            if (DeadlineReached(nowTick, gestureDeadlineTick))
            {
                FailFirst("gesture deadline expired before ButtonDown");
                phase = GesturePhase.FailedBeforeDown;
                return DriverAction.ReportFailure;
            }
            phase = GesturePhase.SendingDown;
            return DriverAction.SendDown;
        }

        // The helper calls this immediately after synchronous SendInput returns,
        // before it sends any reply or drains a queued cancellation.
        internal DriverAction RecordDownInsertion(
            int inserted,
            ulong nowTick,
            string failure = null)
        {
            RequirePhase(GesturePhase.SendingDown);
            if (inserted != 1)
            {
                FailFirst(RequiredFailure(
                    failure,
                    "ButtonDown insertion count was " + inserted));
                phase = GesturePhase.FailedBeforeDown;
                return DriverAction.ReportFailure;
            }
            phase = GesturePhase.HeldAwaitingDownReceipt;
            if (DeadlineReached(nowTick, gestureDeadlineTick))
            {
                return BeginCleanup(
                    "gesture deadline expired after ButtonDown insertion",
                    nowTick);
            }
            return DriverAction.None;
        }

        internal AsyncOutcome RecordDownReceipt(
            ulong replyGestureId,
            bool accepted,
            string failure,
            ulong nowTick)
        {
            AsyncOutcome ignored = ClassifyAsyncReply(replyGestureId);
            if (ignored != null) return ignored;
            if (phase != GesturePhase.HeldAwaitingDownReceipt) return Late();
            if (!accepted)
            {
                return Applied(BeginCleanup(
                    RequiredFailure(failure, "ButtonDown receipt failed"),
                    nowTick));
            }
            if (DeadlineReached(nowTick, gestureDeadlineTick))
            {
                return Applied(BeginCleanup(
                    "gesture deadline expired after ButtonDown receipt",
                    nowTick));
            }
            phase = GesturePhase.HeldReadyForUp;
            return Applied(DriverAction.None);
        }

        internal DriverAction BeginUp(ulong nowTick)
        {
            RequirePhase(GesturePhase.HeldReadyForUp);
            if (DeadlineReached(nowTick, gestureDeadlineTick))
            {
                return BeginCleanup("gesture deadline expired before ButtonUp", nowTick);
            }
            phase = GesturePhase.SendingUp;
            return DriverAction.SendUp;
        }

        internal DriverAction RecordUpInsertion(
            int inserted,
            ulong nowTick,
            string failure = null)
        {
            RequirePhase(GesturePhase.SendingUp);
            if (inserted != 1)
            {
                return BeginCleanup(
                    RequiredFailure(failure, "ButtonUp insertion count was " + inserted),
                    nowTick);
            }
            // Insertion is not delivery. Keep the cleanup obligation until both
            // the app receipt and validated OS release observation arrive.
            phase = GesturePhase.UpInsertedAwaitingReceiptAndOs;
            if (DeadlineReached(nowTick, gestureDeadlineTick))
            {
                return BeginCleanup(
                    "gesture deadline expired after ButtonUp insertion",
                    nowTick);
            }
            return DriverAction.None;
        }

        internal AsyncOutcome RecordUpReceipt(
            ulong replyGestureId,
            bool accepted,
            string failure,
            ulong nowTick)
        {
            AsyncOutcome ignored = ClassifyAsyncReply(replyGestureId);
            if (ignored != null) return ignored;
            if (phase != GesturePhase.UpInsertedAwaitingReceiptAndOs
                && phase != GesturePhase.UpInsertedAwaitingReceipt)
            {
                return Late();
            }
            if (!accepted)
            {
                if (phase == GesturePhase.UpInsertedAwaitingReceipt)
                {
                    FailFirst(RequiredFailure(failure, "ButtonUp receipt failed"));
                    phase = GesturePhase.FailedAfterObservedUp;
                    return Applied(DriverAction.ReportFailure);
                }
                return Applied(BeginCleanup(
                    RequiredFailure(failure, "ButtonUp receipt failed"),
                    nowTick));
            }
            if (DeadlineReached(nowTick, gestureDeadlineTick))
            {
                if (phase == GesturePhase.UpInsertedAwaitingReceipt)
                {
                    FailFirst("gesture deadline expired after ButtonUp receipt");
                    phase = GesturePhase.FailedAfterObservedUp;
                    return Applied(DriverAction.ReportFailure);
                }
                return Applied(BeginCleanup(
                    "gesture deadline expired after ButtonUp receipt",
                    nowTick));
            }
            if (phase == GesturePhase.UpInsertedAwaitingReceipt)
            {
                phase = GesturePhase.Succeeded;
                return Applied(DriverAction.ReportSuccess);
            }
            phase = GesturePhase.UpInsertedAwaitingOs;
            return Applied(DriverAction.None);
        }

        internal AsyncOutcome RecordUpOsObservation(
            ulong replyGestureId,
            OsButtonObservation observation,
            ulong nowTick)
        {
            AsyncOutcome ignored = ClassifyAsyncReply(replyGestureId);
            if (ignored != null) return ignored;
            if (phase != GesturePhase.UpInsertedAwaitingReceiptAndOs
                && phase != GesturePhase.UpInsertedAwaitingOs)
            {
                return Late();
            }
            if (observation == OsButtonObservation.DesktopUnavailable)
            {
                return Applied(BeginCleanup(
                    "interactive desktop unavailable while observing ButtonUp",
                    nowTick));
            }
            if (observation != OsButtonObservation.ReleasedAfterObservedDown)
            {
                if (DeadlineReached(nowTick, gestureDeadlineTick))
                {
                    return Applied(BeginCleanup(
                        "ButtonUp release was not observed before the gesture deadline",
                        nowTick));
                }
                return Applied(DriverAction.None);
            }
            if (DeadlineReached(nowTick, gestureDeadlineTick))
            {
                FailFirst("ButtonUp release was observed after the gesture deadline");
                phase = GesturePhase.FailedAfterObservedUp;
                return Applied(DriverAction.ReportFailure);
            }
            if (phase == GesturePhase.UpInsertedAwaitingOs)
            {
                phase = GesturePhase.Succeeded;
                return Applied(DriverAction.ReportSuccess);
            }
            phase = GesturePhase.UpInsertedAwaitingReceipt;
            return Applied(DriverAction.None);
        }

        internal DriverAction Cancel(string reason, ulong nowTick)
        {
            if (IsTerminal()) return DriverAction.None;
            if (phase == GesturePhase.SendingDown
                || phase == GesturePhase.SendingUp
                || phase == GesturePhase.SendingCleanupUp)
            {
                throw new InvalidOperationException(
                    "record the synchronous send result before cancellation");
            }
            FailFirst(RequiredFailure(reason, "gesture cancelled"));
            if (!HasOutstandingRelease)
            {
                phase = phase == GesturePhase.UpInsertedAwaitingReceipt
                    ? GesturePhase.FailedAfterObservedUp
                    : GesturePhase.FailedBeforeDown;
                return DriverAction.ReportFailure;
            }
            if (phase == GesturePhase.CleanupRequired
                || phase == GesturePhase.CleanupUpInsertedAwaitingOs)
            {
                return DriverAction.None;
            }
            return BeginCleanup(primaryFailure, nowTick);
        }

        internal DriverAction RecordCleanupInsertion(int inserted, string failure)
        {
            RequirePhase(GesturePhase.SendingCleanupUp);
            cleanupAttempts++;
            if (inserted == 1)
            {
                phase = GesturePhase.CleanupUpInsertedAwaitingOs;
                return DriverAction.None;
            }
            lastCleanupFailure = RequiredFailure(
                failure,
                "cleanup ButtonUp insertion count was " + inserted);
            phase = GesturePhase.CleanupRequired;
            return DriverAction.None;
        }

        internal AsyncOutcome RecordCleanupOsObservation(
            ulong replyGestureId,
            OsButtonObservation observation,
            ulong nowTick)
        {
            AsyncOutcome ignored = ClassifyAsyncReply(replyGestureId);
            if (ignored != null) return ignored;
            if (phase != GesturePhase.CleanupUpInsertedAwaitingOs) return Late();
            if (observation == OsButtonObservation.ReleasedAfterObservedDown)
            {
                phase = GesturePhase.FailedAfterObservedCleanup;
                return Applied(DriverAction.ReportFailure);
            }
            lastCleanupFailure = observation == OsButtonObservation.DesktopUnavailable
                ? "interactive desktop unavailable while observing cleanup ButtonUp"
                : "cleanup ButtonUp release was not observed";
            phase = GesturePhase.CleanupRequired;
            return Applied(PlanCleanupRetry(nowTick));
        }

        internal DriverAction PlanCleanupRetry(ulong nowTick)
        {
            RequirePhase(GesturePhase.CleanupRequired);
            ulong deadline = cleanupDeadlineTick.Value;
            if (!DeadlineReached(nowTick, deadline))
            {
                phase = GesturePhase.SendingCleanupUp;
                return DriverAction.SendCleanupUp;
            }
            phase = GesturePhase.FailedCleanupUnresolved;
            return DriverAction.ReportFailure;
        }

        internal DriverAction AdvanceDeadline(ulong nowTick)
        {
            if (IsTerminal()) return DriverAction.None;
            if (phase == GesturePhase.SendingDown
                || phase == GesturePhase.SendingUp
                || phase == GesturePhase.SendingCleanupUp)
            {
                throw new InvalidOperationException(
                    "record the synchronous send result before advancing deadlines");
            }
            if (phase == GesturePhase.CleanupRequired
                || phase == GesturePhase.CleanupUpInsertedAwaitingOs)
            {
                if (DeadlineReached(nowTick, cleanupDeadlineTick.Value))
                {
                    FailFirst("cleanup ButtonUp was not confirmed before its deadline");
                    phase = GesturePhase.FailedCleanupUnresolved;
                    return DriverAction.ReportFailure;
                }
                return DriverAction.None;
            }
            if (!DeadlineReached(nowTick, gestureDeadlineTick))
            {
                return DriverAction.None;
            }
            FailFirst("gesture deadline expired");
            if (!HasOutstandingRelease)
            {
                phase = phase == GesturePhase.UpInsertedAwaitingReceipt
                    ? GesturePhase.FailedAfterObservedUp
                    : GesturePhase.FailedBeforeDown;
                return DriverAction.ReportFailure;
            }
            return BeginCleanup(primaryFailure, nowTick);
        }

        private DriverAction BeginCleanup(string reason, ulong nowTick)
        {
            FailFirst(reason);
            if (!cleanupDeadlineTick.HasValue)
            {
                cleanupDeadlineTick = SaturatingAdd(nowTick, cleanupBudgetTicks);
            }
            phase = GesturePhase.SendingCleanupUp;
            return DriverAction.SendCleanupUp;
        }

        private AsyncOutcome ClassifyAsyncReply(ulong replyGestureId)
        {
            if (replyGestureId != gestureId)
            {
                return new AsyncOutcome(AsyncDisposition.WrongGesture, DriverAction.None);
            }
            return null;
        }

        private static AsyncOutcome Applied(DriverAction action)
        {
            return new AsyncOutcome(AsyncDisposition.Applied, action);
        }

        private static AsyncOutcome Late()
        {
            return new AsyncOutcome(AsyncDisposition.LateIgnored, DriverAction.None);
        }

        private bool IsTerminal()
        {
            return phase == GesturePhase.Succeeded
                || phase == GesturePhase.FailedBeforeDown
                || phase == GesturePhase.FailedAfterObservedUp
                || phase == GesturePhase.FailedAfterObservedCleanup
                || phase == GesturePhase.FailedCleanupUnresolved;
        }

        private void FailFirst(string failure)
        {
            if (primaryFailure == null) primaryFailure = failure;
        }

        private void RequirePhase(GesturePhase expected)
        {
            if (phase != expected)
            {
                throw new InvalidOperationException(
                    "expected phase " + expected + ", actual " + phase);
            }
        }

        private static bool DeadlineReached(ulong nowTick, ulong deadlineTick)
        {
            return nowTick >= deadlineTick;
        }

        private static ulong SaturatingAdd(ulong value, ulong addend)
        {
            ulong result = value + addend;
            return result < value ? UInt64.MaxValue : result;
        }

        private static string RequiredFailure(string failure, string fallback)
        {
            return String.IsNullOrWhiteSpace(failure) ? fallback : failure;
        }
    }

    internal sealed class FakeButtonSender
    {
        private readonly Queue<int> downResults;
        private readonly Queue<int> upResults;

        internal FakeButtonSender(IEnumerable<int> downResults, IEnumerable<int> upResults)
        {
            this.downResults = new Queue<int>(downResults);
            this.upResults = new Queue<int>(upResults);
        }

        internal int DownCalls { get; private set; }
        internal int UpCalls { get; private set; }

        internal int SendDown()
        {
            DownCalls++;
            return downResults.Dequeue();
        }

        internal int SendUp()
        {
            UpCalls++;
            return upResults.Dequeue();
        }
    }
}
