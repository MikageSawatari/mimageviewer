using System;
using Miv.UiSmoke.ButtonDraft;

public static class ButtonGestureReducerTests
{
    private const ulong Gesture = 41;
    private static int passed;

    public static string RunAll()
    {
        DownInsertionImmediatelyOwnsRelease();
        NormalUpNeedsAppAndOsEvidenceInEitherOrder();
        UpInsertionAloneNeverDischargesRelease();
        CancellationBeforeDownNeedsNoCleanup();
        QueuedCancellationWaitsForSynchronousSendResult();
        LateAndWrongGestureRepliesAreTypedAndHarmless();
        UnconfirmedOrUnavailableOsStateNeverSucceeds();
        ObservedOsReleaseNeedsNoExtraUpWhenAppFails();
        DeadlineAtEitherFinalProofCannotSucceed();
        CleanupDeadlineStartsOnceAtCleanupAndNeverExtends();
        CleanupInsertionNeedsOsObservation();
        CleanupObservationTimeoutTerminatesUnconfirmed();
        EveryWaitPhaseHasAnExplicitDeadlineBoundary();
        TerminalPhasesIgnoreLateWrongCancelAndTimer();
        NormalUpZeroAlwaysEntersCleanup();
        CleanupObservationsKeepTheFirstFixedDeadline();
        return "PASS: " + passed + " button reducer tests";
    }

    private static void DownInsertionImmediatelyOwnsRelease()
    {
        var reducer = NewReducer();
        var sender = new FakeButtonSender(new[] { 1 }, new int[0]);
        Equal(DriverAction.SendDown, reducer.BeginDown(10), "begin down");
        Equal(DriverAction.None, reducer.RecordDownInsertion(sender.SendDown(), 11), "down result");
        True(reducer.HasOutstandingRelease, "successful down must immediately own release");
        Equal(GesturePhase.HeldAwaitingDownReceipt, reducer.Phase, "await down receipt");
        Pass();
    }

    private static void NormalUpNeedsAppAndOsEvidenceInEitherOrder()
    {
        var receiptFirst = UpInserted();
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.None,
            receiptFirst.RecordUpReceipt(Gesture, true, null, 22),
            "receipt first");
        True(receiptFirst.HasOutstandingRelease, "receipt alone is not OS release proof");
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.ReportSuccess,
            receiptFirst.RecordUpOsObservation(
                Gesture,
                OsButtonObservation.ReleasedAfterObservedDown,
                23),
            "OS evidence second");
        True(!receiptFirst.HasOutstandingRelease, "both proofs complete release");

        var osFirst = UpInserted();
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.None,
            osFirst.RecordUpOsObservation(
                Gesture,
                OsButtonObservation.ReleasedAfterObservedDown,
                22),
            "OS evidence first");
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.ReportSuccess,
            osFirst.RecordUpReceipt(Gesture, true, null, 23),
            "receipt second");
        Equal(GesturePhase.Succeeded, osFirst.Phase, "normal success");
        Pass();
    }

    private static void UpInsertionAloneNeverDischargesRelease()
    {
        var reducer = UpInserted();
        True(reducer.HasOutstandingRelease, "SendInput count one is insertion, not delivery");
        Equal(GesturePhase.UpInsertedAwaitingReceiptAndOs, reducer.Phase, "await evidence");
        Equal(DriverAction.SendCleanupUp, reducer.Cancel("application exited", 30), "cleanup after app exit");
        Pass();
    }

    private static void CancellationBeforeDownNeedsNoCleanup()
    {
        var reducer = NewReducer();
        Equal(DriverAction.ReportFailure, reducer.Cancel("runner cancelled", 5), "cancel before down");
        Equal(GesturePhase.FailedBeforeDown, reducer.Phase, "failure before down");
        True(!reducer.HasOutstandingRelease, "no down means no up obligation");
        Pass();
    }

    private static void QueuedCancellationWaitsForSynchronousSendResult()
    {
        var insertedZero = NewReducer();
        Equal(DriverAction.SendDown, insertedZero.BeginDown(10), "begin zero down");
        ThrowsInvalidOperation(
            delegate { insertedZero.Cancel("queued app exit", 11); },
            "cancel is not dequeued during send");
        Equal(DriverAction.ReportFailure, insertedZero.RecordDownInsertion(0, 12), "record zero");
        Equal(DriverAction.None, insertedZero.Cancel("queued app exit", 12), "terminal queued cancel");

        var insertedOne = NewReducer();
        Equal(DriverAction.SendDown, insertedOne.BeginDown(10), "begin one down");
        Equal(DriverAction.None, insertedOne.RecordDownInsertion(1, 12), "record one");
        Equal(DriverAction.SendCleanupUp, insertedOne.Cancel("queued app exit", 12), "cleanup inserted down");
        Pass();
    }

    private static void LateAndWrongGestureRepliesAreTypedAndHarmless()
    {
        var down = HeldAwaitingReceipt();
        Equal(DriverAction.SendCleanupUp, down.Cancel("pipe EOF", 20), "cancel held down");
        Outcome(
            AsyncDisposition.LateIgnored,
            DriverAction.None,
            down.RecordDownReceipt(Gesture, true, null, 21),
            "late down receipt");
        Equal("pipe EOF", down.PrimaryFailure, "late down keeps first failure");

        var up = UpInserted();
        Equal(DriverAction.SendCleanupUp, up.Cancel("runner cancelled", 24), "cancel inserted up");
        Outcome(
            AsyncDisposition.LateIgnored,
            DriverAction.None,
            up.RecordUpReceipt(Gesture, true, null, 25),
            "late up receipt");
        Outcome(
            AsyncDisposition.WrongGesture,
            DriverAction.None,
            up.RecordCleanupOsObservation(
                Gesture + 1,
                OsButtonObservation.ReleasedAfterObservedDown,
                25),
            "wrong gesture observation");
        Equal("runner cancelled", up.PrimaryFailure, "late up keeps first failure");
        Pass();
    }

    private static void UnconfirmedOrUnavailableOsStateNeverSucceeds()
    {
        var reducer = UpInserted();
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.None,
            reducer.RecordUpOsObservation(
                Gesture,
                OsButtonObservation.StillDownOnValidatedDesktop,
                22),
            "still down");
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.None,
            reducer.RecordUpOsObservation(Gesture, OsButtonObservation.Indeterminate, 23),
            "indeterminate");
        True(reducer.HasOutstandingRelease, "unconfirmed samples retain cleanup obligation");
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.SendCleanupUp,
            reducer.RecordUpOsObservation(
                Gesture,
                OsButtonObservation.DesktopUnavailable,
                24),
            "desktop unavailable");
        True(reducer.Phase != GesturePhase.Succeeded, "desktop failure cannot pass");
        Pass();
    }

    private static void ObservedOsReleaseNeedsNoExtraUpWhenAppFails()
    {
        var rejected = UpInserted();
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.None,
            rejected.RecordUpOsObservation(
                Gesture,
                OsButtonObservation.ReleasedAfterObservedDown,
                22),
            "release before rejected app receipt");
        True(!rejected.HasOutstandingRelease, "observed OS release discharges cleanup duty");
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.ReportFailure,
            rejected.RecordUpReceipt(Gesture, false, "App handler rejected Up", 23),
            "negative app receipt after OS release");
        Equal(GesturePhase.FailedAfterObservedUp, rejected.Phase, "App failure after release");

        var eof = UpInserted();
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.None,
            eof.RecordUpOsObservation(
                Gesture,
                OsButtonObservation.ReleasedAfterObservedDown,
                22),
            "release before EOF");
        Equal(DriverAction.ReportFailure, eof.Cancel("pipe EOF", 23), "EOF after OS release");
        Equal(GesturePhase.FailedAfterObservedUp, eof.Phase, "EOF does not resend Up");
        Pass();
    }

    private static void DeadlineAtEitherFinalProofCannotSucceed()
    {
        var osLast = UpInserted();
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.None,
            osLast.RecordUpReceipt(Gesture, true, null, 99),
            "receipt before deadline");
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.ReportFailure,
            osLast.RecordUpOsObservation(
                Gesture,
                OsButtonObservation.ReleasedAfterObservedDown,
                100),
            "OS evidence at deadline");
        Equal(GesturePhase.FailedAfterObservedUp, osLast.Phase, "late OS evidence failure");

        var receiptLast = UpInserted();
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.None,
            receiptLast.RecordUpOsObservation(
                Gesture,
                OsButtonObservation.ReleasedAfterObservedDown,
                99),
            "OS evidence before deadline");
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.ReportFailure,
            receiptLast.RecordUpReceipt(Gesture, true, null, 100),
            "receipt at deadline");
        Equal(GesturePhase.FailedAfterObservedUp, receiptLast.Phase, "late receipt failure");
        Pass();
    }

    private static void CleanupDeadlineStartsOnceAtCleanupAndNeverExtends()
    {
        var reducer = HeldAwaitingReceipt();
        Equal(null, reducer.CleanupDeadlineTick, "no cleanup deadline before cancellation");
        Equal(DriverAction.SendCleanupUp, reducer.Cancel("early EOF", 20), "early cleanup");
        Equal((ulong?)30, reducer.CleanupDeadlineTick, "cleanup deadline starts at cleanup");
        Equal(DriverAction.None, reducer.RecordCleanupInsertion(0, "first zero"), "first cleanup zero");
        Equal(DriverAction.None, reducer.Cancel("later timeout", 25), "competing cancel");
        Equal((ulong?)30, reducer.CleanupDeadlineTick, "cancel cannot extend cleanup deadline");
        Equal(DriverAction.SendCleanupUp, reducer.PlanCleanupRetry(29), "retry before deadline");
        Equal(DriverAction.None, reducer.RecordCleanupInsertion(0, "second zero"), "second cleanup zero");
        Equal(DriverAction.ReportFailure, reducer.PlanCleanupRetry(30), "deadline is exclusive");
        Equal(GesturePhase.FailedCleanupUnresolved, reducer.Phase, "unconfirmed terminal failure");
        True(reducer.HasOutstandingRelease, "unresolved release remains explicit");
        Equal("early EOF", reducer.PrimaryFailure, "first failure is sticky");
        Equal(2, reducer.CleanupAttempts, "bounded cleanup attempts visible");

        var afterGestureDeadline = HeldAwaitingReceipt();
        Equal(
            DriverAction.SendCleanupUp,
            afterGestureDeadline.Cancel("gesture timeout", 101),
            "cleanup starts just after gesture deadline");
        Equal((ulong?)111, afterGestureDeadline.CleanupDeadlineTick, "cleanup gets one short budget");
        Pass();
    }

    private static void CleanupInsertionNeedsOsObservation()
    {
        var reducer = HeldAwaitingReceipt();
        Equal(DriverAction.SendCleanupUp, reducer.Cancel("application exited", 20), "start cleanup");
        Equal(DriverAction.None, reducer.RecordCleanupInsertion(1, null), "cleanup inserted");
        True(reducer.HasOutstandingRelease, "cleanup insertion is not OS observation");
        Equal(GesturePhase.CleanupUpInsertedAwaitingOs, reducer.Phase, "await cleanup OS evidence");
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.ReportFailure,
            reducer.RecordCleanupOsObservation(
                Gesture,
                OsButtonObservation.ReleasedAfterObservedDown,
                22),
            "cleanup release observed");
        Equal(GesturePhase.FailedAfterObservedCleanup, reducer.Phase, "failed after observed cleanup");
        True(!reducer.HasOutstandingRelease, "observed cleanup discharges obligation");
        Equal("application exited", reducer.PrimaryFailure, "cleanup cannot turn failure into success");
        Pass();
    }

    private static void CleanupObservationTimeoutTerminatesUnconfirmed()
    {
        var reducer = HeldAwaitingReceipt();
        Equal(DriverAction.SendCleanupUp, reducer.Cancel("application exited", 20), "start cleanup");
        Equal(DriverAction.None, reducer.RecordCleanupInsertion(1, null), "cleanup inserted");
        Equal(DriverAction.None, reducer.AdvanceDeadline(29), "wait before cleanup deadline");
        Equal(DriverAction.ReportFailure, reducer.AdvanceDeadline(30), "missing observation timeout");
        Equal(GesturePhase.FailedCleanupUnresolved, reducer.Phase, "cleanup remains unconfirmed");
        True(reducer.HasOutstandingRelease, "missing OS proof remains explicit");
        Pass();
    }

    private static void EveryWaitPhaseHasAnExplicitDeadlineBoundary()
    {
        for (int offset = 0; offset <= 1; offset++)
        {
            ulong tick = 100UL + (ulong)offset;
            AssertDeadline(
                NewReducer(),
                tick,
                DriverAction.ReportFailure,
                GesturePhase.FailedBeforeDown,
                "ready down");
            AssertDeadline(
                HeldAwaitingReceipt(),
                tick,
                DriverAction.SendCleanupUp,
                GesturePhase.SendingCleanupUp,
                "held awaiting down receipt");
            AssertDeadline(
                HeldReadyForUp(),
                tick,
                DriverAction.SendCleanupUp,
                GesturePhase.SendingCleanupUp,
                "held ready for up");
            AssertDeadline(
                UpInserted(),
                tick,
                DriverAction.SendCleanupUp,
                GesturePhase.SendingCleanupUp,
                "up awaiting both proofs");
            AssertDeadline(
                UpAwaitingReceipt(),
                tick,
                DriverAction.ReportFailure,
                GesturePhase.FailedAfterObservedUp,
                "up awaiting App receipt after OS proof");
            AssertDeadline(
                UpAwaitingOs(),
                tick,
                DriverAction.SendCleanupUp,
                GesturePhase.SendingCleanupUp,
                "up awaiting OS proof after App receipt");
        }

        var before = UpAwaitingOs();
        Equal(DriverAction.None, before.AdvanceDeadline(99), "before gesture deadline");
        Equal(GesturePhase.UpInsertedAwaitingOs, before.Phase, "wait phase before deadline");

        foreach (bool inserted in new[] { false, true })
        {
            var cleanup = CleanupWaiting(inserted);
            Equal(DriverAction.None, cleanup.AdvanceDeadline(29), "before cleanup deadline");
            Equal(
                inserted ? GesturePhase.CleanupUpInsertedAwaitingOs : GesturePhase.CleanupRequired,
                cleanup.Phase,
                "cleanup phase before deadline");
            Equal(DriverAction.ReportFailure, cleanup.AdvanceDeadline(30), "at cleanup deadline");
            Equal(GesturePhase.FailedCleanupUnresolved, cleanup.Phase, "cleanup deadline terminal");
        }
        Pass();
    }

    private static void TerminalPhasesIgnoreLateWrongCancelAndTimer()
    {
        ButtonGestureReducer[] terminals =
        {
            CompletedSuccess(),
            FailedBeforeDown(),
            FailedAfterObservedUp(),
            FailedAfterObservedCleanup(),
            FailedCleanupUnresolved(),
        };
        foreach (ButtonGestureReducer reducer in terminals)
        {
            GesturePhase original = reducer.Phase;
            Equal(DriverAction.None, reducer.Cancel("late cancel", 200), original + " cancel");
            Equal(DriverAction.None, reducer.AdvanceDeadline(200), original + " timer");
            Outcome(
                AsyncDisposition.LateIgnored,
                DriverAction.None,
                reducer.RecordDownReceipt(Gesture, true, null, 200),
                original + " late down receipt");
            Outcome(
                AsyncDisposition.LateIgnored,
                DriverAction.None,
                reducer.RecordUpReceipt(Gesture, true, null, 200),
                original + " late up receipt");
            Outcome(
                AsyncDisposition.LateIgnored,
                DriverAction.None,
                reducer.RecordUpOsObservation(
                    Gesture,
                    OsButtonObservation.ReleasedAfterObservedDown,
                    200),
                original + " late OS receipt");
            Outcome(
                AsyncDisposition.WrongGesture,
                DriverAction.None,
                reducer.RecordCleanupOsObservation(
                    Gesture + 1,
                    OsButtonObservation.ReleasedAfterObservedDown,
                    200),
                original + " wrong gesture");
            Equal(original, reducer.Phase, original + " must not revive");
        }
        Pass();
    }

    private static void NormalUpZeroAlwaysEntersCleanup()
    {
        var reducer = HeldReadyForUp();
        Equal(DriverAction.SendUp, reducer.BeginUp(20), "begin normal up");
        Equal(DriverAction.SendCleanupUp, reducer.RecordUpInsertion(0, 21), "normal up zero");
        Equal(GesturePhase.SendingCleanupUp, reducer.Phase, "zero enters cleanup send");
        True(reducer.HasOutstandingRelease, "zero cannot discharge release");
        Equal("ButtonUp insertion count was 0", reducer.PrimaryFailure, "zero is primary failure");
        Pass();
    }

    private static void CleanupObservationsKeepTheFirstFixedDeadline()
    {
        OsButtonObservation[] failures =
        {
            OsButtonObservation.StillDownOnValidatedDesktop,
            OsButtonObservation.Indeterminate,
            OsButtonObservation.DesktopUnavailable,
        };
        foreach (OsButtonObservation failure in failures)
        {
            var reducer = CleanupWaiting(true);
            Outcome(
                AsyncDisposition.Applied,
                DriverAction.SendCleanupUp,
                reducer.RecordCleanupOsObservation(Gesture, failure, 21),
                failure.ToString());
            Equal((ulong?)30, reducer.CleanupDeadlineTick, failure + " fixed deadline");
            Equal(DriverAction.None, reducer.RecordCleanupInsertion(0, "retry zero"), "retry result");
            Equal(DriverAction.ReportFailure, reducer.AdvanceDeadline(30), failure + " deadline");
        }

        var lateRelease = CleanupWaiting(true);
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.ReportFailure,
            lateRelease.RecordCleanupOsObservation(
                Gesture,
                OsButtonObservation.ReleasedAfterObservedDown,
                31),
            "release observed after cleanup deadline");
        Equal(
            GesturePhase.FailedAfterObservedCleanup,
            lateRelease.Phase,
            "late observation proves release but never success or deadline compliance");
        True(!lateRelease.HasOutstandingRelease, "late release is still physically observed");
        Pass();
    }

    private static ButtonGestureReducer NewReducer()
    {
        return new ButtonGestureReducer(Gesture, 100, 10);
    }

    private static ButtonGestureReducer HeldAwaitingReceipt()
    {
        var reducer = NewReducer();
        Equal(DriverAction.SendDown, reducer.BeginDown(10), "fixture begin down");
        Equal(DriverAction.None, reducer.RecordDownInsertion(1, 11), "fixture down inserted");
        return reducer;
    }

    private static ButtonGestureReducer HeldReadyForUp()
    {
        var reducer = HeldAwaitingReceipt();
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.None,
            reducer.RecordDownReceipt(Gesture, true, null, 12),
            "fixture down receipt");
        return reducer;
    }

    private static ButtonGestureReducer UpInserted()
    {
        var reducer = HeldReadyForUp();
        Equal(DriverAction.SendUp, reducer.BeginUp(20), "fixture begin up");
        Equal(DriverAction.None, reducer.RecordUpInsertion(1, 21), "fixture up inserted");
        return reducer;
    }

    private static ButtonGestureReducer UpAwaitingReceipt()
    {
        var reducer = UpInserted();
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.None,
            reducer.RecordUpOsObservation(
                Gesture,
                OsButtonObservation.ReleasedAfterObservedDown,
                22),
            "fixture OS evidence");
        return reducer;
    }

    private static ButtonGestureReducer UpAwaitingOs()
    {
        var reducer = UpInserted();
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.None,
            reducer.RecordUpReceipt(Gesture, true, null, 22),
            "fixture App receipt");
        return reducer;
    }

    private static ButtonGestureReducer CleanupWaiting(bool inserted)
    {
        var reducer = HeldAwaitingReceipt();
        Equal(DriverAction.SendCleanupUp, reducer.Cancel("fixture cancel", 20), "fixture cleanup");
        Equal(DriverAction.None, reducer.RecordCleanupInsertion(inserted ? 1 : 0, null), "fixture cleanup insertion");
        return reducer;
    }

    private static ButtonGestureReducer CompletedSuccess()
    {
        var reducer = UpAwaitingOs();
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.ReportSuccess,
            reducer.RecordUpOsObservation(
                Gesture,
                OsButtonObservation.ReleasedAfterObservedDown,
                23),
            "fixture success");
        return reducer;
    }

    private static ButtonGestureReducer FailedBeforeDown()
    {
        var reducer = NewReducer();
        Equal(DriverAction.ReportFailure, reducer.Cancel("fixture cancel", 1), "fixture before down failure");
        return reducer;
    }

    private static ButtonGestureReducer FailedAfterObservedUp()
    {
        var reducer = UpAwaitingReceipt();
        Equal(DriverAction.ReportFailure, reducer.Cancel("fixture cancel", 23), "fixture observed up failure");
        return reducer;
    }

    private static ButtonGestureReducer FailedAfterObservedCleanup()
    {
        var reducer = CleanupWaiting(true);
        Outcome(
            AsyncDisposition.Applied,
            DriverAction.ReportFailure,
            reducer.RecordCleanupOsObservation(
                Gesture,
                OsButtonObservation.ReleasedAfterObservedDown,
                22),
            "fixture observed cleanup failure");
        return reducer;
    }

    private static ButtonGestureReducer FailedCleanupUnresolved()
    {
        var reducer = CleanupWaiting(true);
        Equal(DriverAction.ReportFailure, reducer.AdvanceDeadline(30), "fixture unresolved cleanup");
        return reducer;
    }

    private static void AssertDeadline(
        ButtonGestureReducer reducer,
        ulong tick,
        DriverAction action,
        GesturePhase phase,
        string label)
    {
        Equal(action, reducer.AdvanceDeadline(tick), label + " action at " + tick);
        Equal(phase, reducer.Phase, label + " phase at " + tick);
    }

    private static void Outcome(
        AsyncDisposition disposition,
        DriverAction action,
        AsyncOutcome actual,
        string label)
    {
        Equal(disposition, actual.Disposition, label + " disposition");
        Equal(action, actual.Action, label + " action");
    }

    private static void ThrowsInvalidOperation(Action action, string label)
    {
        try
        {
            action();
            throw new Exception("assertion failed: " + label + "; no exception");
        }
        catch (InvalidOperationException)
        {
        }
    }

    private static void True(bool condition, string label)
    {
        if (!condition) throw new Exception("assertion failed: " + label);
    }

    private static void Equal<T>(T expected, T actual, string label)
    {
        if (!Object.Equals(expected, actual))
        {
            throw new Exception(
                "assertion failed: " + label + "; expected=" + expected + "; actual=" + actual);
        }
    }

    private static void Pass()
    {
        passed++;
    }
}
