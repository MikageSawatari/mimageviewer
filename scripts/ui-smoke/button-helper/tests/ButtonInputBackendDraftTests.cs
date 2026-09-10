using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    internal static class ButtonInputBackendDraftTests
    {
        internal static int RunAll()
        {
            int passed = 0;
            Console.WriteLine("RUN NormalBackendAcceptsOnlyCoherentEdgeFacts");
            NormalBackendAcceptsOnlyCoherentEdgeFacts();
            passed++;
            Console.WriteLine("RUN NormalPolicyRejectsOwnerReceiverAndStateMismatches");
            NormalPolicyRejectsOwnerReceiverAndStateMismatches();
            passed++;
            Console.WriteLine("RUN OwnerPermissionRunsAfterFactsAndBeforeInsertion");
            OwnerPermissionRunsAfterFactsAndBeforeInsertion();
            passed++;
            Console.WriteLine("RUN OwnerPermissionRejectsDeathAndDeadlineAfterFacts");
            OwnerPermissionRejectsDeathAndDeadlineAfterFacts();
            passed++;
            Console.WriteLine("RUN ActualOwnerRechecksPermissionAfterFacts");
            ActualOwnerRechecksPermissionAfterFacts();
            passed++;
            Console.WriteLine("RUN NativeFactAcquisitionFailureCannotCallInsertion");
            NativeFactAcquisitionFailureCannotCallInsertion();
            passed++;
            Console.WriteLine("RUN TypedAttemptPreservesInsertedCountAcrossPostCallFault");
            TypedAttemptPreservesInsertedCountAcrossPostCallFault();
            passed++;
            Console.WriteLine("RUN CountZeroCannotCreateReleaseObligation");
            CountZeroCannotCreateReleaseObligation();
            passed++;
            Console.WriteLine("RUN OwnedCleanupDoesNotRevalidateHistoricalTarget");
            OwnedCleanupDoesNotRevalidateHistoricalTarget();
            passed++;
            Console.WriteLine("RUN OwnedCleanupRequiresInputDesktopLease");
            OwnedCleanupRequiresInputDesktopLease();
            passed++;
            Console.WriteLine("RUN TagValidationIsExactBeforeInsertion");
            TagValidationIsExactBeforeInsertion();
            passed++;
            Console.WriteLine("RUN MouseInputAbiIsOneExactTaggedEdge");
            MouseInputAbiIsOneExactTaggedEdge();
            passed++;
            return passed;
        }

        private static void NormalPolicyRejectsOwnerReceiverAndStateMismatches()
        {
            ButtonGestureTuple gesture = Gesture(0x4d490001UL, 0x4d490002UL);
            ButtonNormalFrame valid = Frame(gesture, 0);
            ButtonAccessFrame access = valid.Access;
            ButtonReceiverFrame receiver = valid.Receiver;
            ButtonNormalFrame[] wrongStableFrames = new ButtonNormalFrame[]
            {
                new ButtonNormalFrame(
                    Access(gesture, gesture.Target.ParentHwnd + 1, gesture.AppProcessId,
                        gesture.AppCreationIdentity, false), receiver),
                new ButtonNormalFrame(
                    Access(gesture, gesture.Target.ParentHwnd, gesture.AppProcessId + 1,
                        gesture.AppCreationIdentity, false), receiver),
                new ButtonNormalFrame(
                    Access(gesture, gesture.Target.ParentHwnd, gesture.AppProcessId,
                        gesture.AppCreationIdentity + 1, false), receiver),
                new ButtonNormalFrame(access,
                    Receiver(gesture, gesture.Target.InputHwnd + 1,
                        gesture.Target.ParentHwnd, gesture.AppProcessId, 800, 0)),
                new ButtonNormalFrame(access,
                    Receiver(gesture, gesture.Target.InputHwnd,
                        gesture.Target.ParentHwnd + 1, gesture.AppProcessId, 800, 0)),
                new ButtonNormalFrame(access,
                    Receiver(gesture, gesture.Target.InputHwnd,
                        gesture.Target.ParentHwnd, gesture.AppProcessId + 1, 800, 0)),
                new ButtonNormalFrame(access,
                    Receiver(gesture, gesture.Target.InputHwnd,
                        gesture.Target.ParentHwnd, gesture.AppProcessId, 0, 0)),
                new ButtonNormalFrame(access,
                    Receiver(gesture, gesture.Target.InputHwnd,
                        gesture.Target.ParentHwnd, gesture.AppProcessId, 800,
                        gesture.Target.InputHwnd)),
                new ButtonNormalFrame(
                    Access(gesture, gesture.Target.ParentHwnd, gesture.AppProcessId,
                        gesture.AppCreationIdentity, true), receiver),
            };
            for (int i = 0; i < wrongStableFrames.Length; i++)
            {
                string failure = ButtonNormalSendPolicy.Validate(
                    new ButtonNormalSendRequest(
                        ButtonNormalSendEdge.Down,
                        gesture,
                        gesture.DownTag),
                    ProbeFrom(
                        wrongStableFrames[i],
                        wrongStableFrames[i],
                        gesture,
                        ButtonPressedState.None,
                        ButtonPressedState.None));
                True(failure != null, "stable ownership mismatch " + i + " is refused");
            }

            string changedFrame = ButtonNormalSendPolicy.Validate(
                new ButtonNormalSendRequest(ButtonNormalSendEdge.Down, gesture, gesture.DownTag),
                ProbeFrom(
                    valid,
                    new ButtonNormalFrame(
                        access,
                        Receiver(gesture, gesture.Target.InputHwnd,
                            gesture.Target.ParentHwnd, gesture.AppProcessId, 801, 0)),
                    gesture,
                    ButtonPressedState.None,
                    ButtonPressedState.None));
            True(changedFrame != null, "receiver change across preflight is refused");

            string changedState = ButtonNormalSendPolicy.Validate(
                new ButtonNormalSendRequest(ButtonNormalSendEdge.Down, gesture, gesture.DownTag),
                ProbeFrom(
                    valid,
                    valid,
                    gesture,
                    ButtonPressedState.None,
                    ButtonPressedState.Control));
            True(changedState != null, "input-state change across preflight is refused");

            string wrongLeftLevel = ButtonNormalSendPolicy.Validate(
                new ButtonNormalSendRequest(ButtonNormalSendEdge.Down, gesture, gesture.DownTag),
                ProbeFrom(
                    valid,
                    valid,
                    gesture,
                    ButtonPressedState.Left,
                    ButtonPressedState.Left));
            True(wrongLeftLevel != null, "held physical Left refuses normal Down");

            ButtonNormalSendProbe swappedState = new ButtonNormalSendProbe(
                valid,
                valid,
                new ButtonScreenPoint(gesture.Target.ScreenX, gesture.Target.ScreenY),
                new ButtonScreenPoint(gesture.Target.ScreenX, gesture.Target.ScreenY),
                new ButtonInputStateFrame(true, ButtonPressedState.None),
                new ButtonInputStateFrame(true, ButtonPressedState.None),
                new ButtonPointTolerance(1, 1));
            True(ButtonNormalSendPolicy.Validate(
                    new ButtonNormalSendRequest(
                        ButtonNormalSendEdge.Down,
                        gesture,
                        gesture.DownTag),
                    swappedState) != null,
                "swapped physical mapping refuses normal Down");

            ButtonNormalFrame uncapturedUp = Frame(gesture, 0);
            True(ButtonNormalSendPolicy.Validate(
                    new ButtonNormalSendRequest(
                        ButtonNormalSendEdge.Up,
                        gesture,
                        gesture.UpTag),
                    ProbeFrom(
                        uncapturedUp,
                        uncapturedUp,
                        gesture,
                        ButtonPressedState.Left,
                        ButtonPressedState.Left)) != null,
                "normal Up requires capture by the immutable input HWND");
        }

        private static void NormalBackendAcceptsOnlyCoherentEdgeFacts()
        {
            ButtonGestureTuple gesture = Gesture(0x4d490001UL, 0x4d490002UL);
            FakeInputFacts facts = new FakeInputFacts(
                Probe(gesture, ButtonNormalSendEdge.Down, ButtonPressedState.None, 0));
            FakeNativeInserter inserter = new FakeNativeInserter();
            Win32ButtonInputBackend backend = new Win32ButtonInputBackend(facts, inserter);
            ButtonSendAttempt accepted = backend.SendNormal(
                new ButtonNormalSendRequest(ButtonNormalSendEdge.Down, gesture, gesture.DownTag),
                delegate { return ButtonSendPermission.Allowed; });
            True(accepted.WasCalled && accepted.Inserted == 1, "coherent Down calls inserter once");
            Equal(1, inserter.Calls.Count, "coherent Down call count");

            FakeNativeInserter movedInserter = new FakeNativeInserter();
            ButtonNormalSendProbe moved = Probe(
                gesture,
                ButtonNormalSendEdge.Down,
                ButtonPressedState.None,
                0);
            moved = new ButtonNormalSendProbe(
                moved.Before,
                moved.After,
                new ButtonScreenPoint(gesture.Target.ScreenX + 20, gesture.Target.ScreenY),
                moved.CursorAfter,
                moved.StateBefore,
                moved.StateAfter,
                moved.ScreenPointTolerance);
            ButtonSendAttempt movedResult = new Win32ButtonInputBackend(
                new FakeInputFacts(moved),
                movedInserter).SendNormal(
                    new ButtonNormalSendRequest(ButtonNormalSendEdge.Down, gesture, gesture.DownTag),
                    delegate { return ButtonSendPermission.Allowed; });
            True(!movedResult.WasCalled, "moved cursor refuses normal Down");
            Equal(0, movedInserter.Calls.Count, "moved cursor makes no native call");

            FakeNativeInserter modifierInserter = new FakeNativeInserter();
            ButtonSendAttempt modifier = new Win32ButtonInputBackend(
                new FakeInputFacts(Probe(
                    gesture,
                    ButtonNormalSendEdge.Down,
                    ButtonPressedState.Control,
                    0)),
                modifierInserter).SendNormal(
                    new ButtonNormalSendRequest(ButtonNormalSendEdge.Down, gesture, gesture.DownTag),
                    delegate { return ButtonSendPermission.Allowed; });
            True(!modifier.WasCalled, "held modifier refuses normal Down");
            Equal(0, modifierInserter.Calls.Count, "held modifier makes no native call");

            FakeNativeInserter upInserter = new FakeNativeInserter();
            ButtonSendAttempt up = new Win32ButtonInputBackend(
                new FakeInputFacts(Probe(
                    gesture,
                    ButtonNormalSendEdge.Up,
                    ButtonPressedState.Left,
                    gesture.Target.InputHwnd)),
                upInserter).SendNormal(
                    new ButtonNormalSendRequest(ButtonNormalSendEdge.Up, gesture, gesture.UpTag),
                    delegate { return ButtonSendPermission.Allowed; });
            True(up.WasCalled && up.Inserted == 1, "coherent held/captured Up calls inserter");
            Equal(ButtonNormalSendEdge.Up, upInserter.Calls[0].Edge, "normal Up edge");
        }

        private static void OwnerPermissionRunsAfterFactsAndBeforeInsertion()
        {
            ButtonGestureTuple gesture = Gesture(0x4d490001UL, 0x4d490002UL);
            List<string> order = new List<string>();
            FakeInputFacts facts = new FakeInputFacts(
                Probe(gesture, ButtonNormalSendEdge.Down, ButtonPressedState.None, 0),
                null,
                order);
            FakeNativeInserter inserter = new FakeNativeInserter(order);
            Win32ButtonInputBackend backend = new Win32ButtonInputBackend(facts, inserter);
            ButtonSendAttempt refused = backend.SendNormal(
                new ButtonNormalSendRequest(ButtonNormalSendEdge.Down, gesture, gesture.DownTag),
                delegate
                {
                    order.Add("permission");
                    return ButtonSendPermission.DeadlineExpired;
                });
            True(!refused.WasCalled, "denied owner permission is Refused");
            Equal("facts", order[0], "facts precede permission");
            Equal("permission", order[1], "permission follows all facts");
            Equal("finish", order[2], "native scope is closed after refusal");
            Equal(0, inserter.Calls.Count, "denied permission makes no native call");
        }

        private static void OwnerPermissionRejectsDeathAndDeadlineAfterFacts()
        {
            ButtonGestureTuple gesture = Gesture(0x4d490001UL, 0x4d490002UL);
            ButtonSendPermission[] denied = new ButtonSendPermission[]
            {
                ButtonSendPermission.ProcessExited,
                ButtonSendPermission.DeadlineExpired,
            };
            for (int i = 0; i < denied.Length; i++)
            {
                FakeNativeInserter inserter = new FakeNativeInserter();
                ButtonSendPermission reason = denied[i];
                ButtonSendAttempt result = new Win32ButtonInputBackend(
                    new FakeInputFacts(Probe(
                        gesture,
                        ButtonNormalSendEdge.Down,
                        ButtonPressedState.None,
                        0)),
                    inserter).SendNormal(
                        new ButtonNormalSendRequest(
                            ButtonNormalSendEdge.Down,
                            gesture,
                            gesture.DownTag),
                        delegate { return reason; });
                True(!result.WasCalled, reason + " after native facts is Refused");
                True(result.FailureDetail.Contains(reason.ToString()),
                    reason + " is retained in the typed refusal");
                Equal(0, inserter.Calls.Count, reason + " makes no native call");
            }
        }

        private static void NativeFactAcquisitionFailureCannotCallInsertion()
        {
            ButtonGestureTuple gesture = Gesture(0x4d490001UL, 0x4d490002UL);
            FakeInputFacts facts = new FakeInputFacts(null);
            facts.ThrowIfOpened = true;
            FakeNativeInserter inserter = new FakeNativeInserter();
            ButtonSendAttempt result = new Win32ButtonInputBackend(facts, inserter)
                .SendNormal(
                    new ButtonNormalSendRequest(
                        ButtonNormalSendEdge.Down,
                        gesture,
                        gesture.DownTag),
                    delegate { return ButtonSendPermission.Allowed; });
            True(!result.WasCalled, "unavailable DPI/desktop/native facts are Refused");
            True(result.FailureDetail.Contains("preflight"),
                "native fact failure retains preflight classification");
            Equal(0, inserter.Calls.Count, "fact acquisition failure makes no native call");
        }

        private static void ActualOwnerRechecksPermissionAfterFacts()
        {
            ButtonWireRequest deathBegin = Request(0x4d490001UL, 0x4d490002UL);
            ButtonGestureTuple deathGesture = new ButtonGestureTuple(deathBegin);
            FakeAppLease deadLease = new FakeAppLease(
                deathBegin.AppProcessId,
                deathBegin.AppCreationIdentity);
            FakeInputFacts deathFacts = new FakeInputFacts(Probe(
                deathGesture,
                ButtonNormalSendEdge.Down,
                ButtonPressedState.None,
                0));
            deathFacts.AfterOpen = delegate { deadLease.Alive = false; };
            FakeNativeInserter deathInserter = new FakeNativeInserter();
            ButtonHelperOwner deathOwner = new ButtonHelperOwner(
                deathBegin.SessionNonce,
                deadLease,
                new FakeClock(10),
                new Win32ButtonInputBackend(deathFacts, deathInserter),
                new NeverObserveDelivery(),
                new AcceptingSink(),
                5);
            deathOwner.Handle(deathBegin);
            Equal(0, deathInserter.Calls.Count,
                "actual owner rejects process death after facts before Down insertion");
            Equal(Miv.UiSmoke.ButtonDraft.GesturePhase.FailedBeforeDown,
                deathOwner.Phase,
                "post-facts process death fails before Down");
            True(!deathOwner.HasOutstandingRelease,
                "post-facts process death creates no release obligation");
            True(deathOwner.PrimaryFailure.Contains("ProcessExited"),
                "typed process-death refusal reaches reducer primary failure");

            ButtonWireRequest deadlineBegin = Request(0x4d490011UL, 0x4d490012UL);
            ButtonGestureTuple deadlineGesture = new ButtonGestureTuple(deadlineBegin);
            FakeClock deadlineClock = new FakeClock(10);
            FakeInputFacts deadlineFacts = new FakeInputFacts(Probe(
                deadlineGesture,
                ButtonNormalSendEdge.Down,
                ButtonPressedState.None,
                0));
            deadlineFacts.AfterOpen = delegate { deadlineClock.Now = deadlineBegin.DeadlineTick; };
            FakeNativeInserter deadlineInserter = new FakeNativeInserter();
            ButtonHelperOwner deadlineOwner = new ButtonHelperOwner(
                deadlineBegin.SessionNonce,
                new FakeAppLease(
                    deadlineBegin.AppProcessId,
                    deadlineBegin.AppCreationIdentity),
                deadlineClock,
                new Win32ButtonInputBackend(deadlineFacts, deadlineInserter),
                new NeverObserveDelivery(),
                new AcceptingSink(),
                5);
            deadlineOwner.Handle(deadlineBegin);
            Equal(0, deadlineInserter.Calls.Count,
                "actual owner rejects deadline reached after facts before Down insertion");
            True(deadlineOwner.PrimaryFailure.Contains("DeadlineExpired"),
                "typed deadline refusal reaches reducer primary failure");

            ButtonWireRequest upBegin = Request(0x4d490021UL, 0x4d490022UL);
            ButtonGestureTuple upGesture = new ButtonGestureTuple(upBegin);
            FakeClock upClock = new FakeClock(10);
            FakeInputFacts upFacts = new FakeInputFacts(Probe(
                upGesture,
                ButtonNormalSendEdge.Down,
                ButtonPressedState.None,
                0));
            upFacts.UpProbe = Probe(
                upGesture,
                ButtonNormalSendEdge.Up,
                ButtonPressedState.Left,
                upGesture.Target.InputHwnd);
            upFacts.AfterOpenRequest = delegate(ButtonNormalSendRequest request)
            {
                if (request.Edge == ButtonNormalSendEdge.Up)
                    upClock.Now = upBegin.DeadlineTick;
            };
            FakeNativeInserter upInserter = new FakeNativeInserter();
            upInserter.Results.Enqueue(new ButtonNativeInsertion(1, 0));
            upInserter.Results.Enqueue(new ButtonNativeInsertion(0, 5));
            ButtonHelperOwner upOwner = new ButtonHelperOwner(
                upBegin.SessionNonce,
                new FakeAppLease(upBegin.AppProcessId, upBegin.AppCreationIdentity),
                upClock,
                new Win32ButtonInputBackend(upFacts, upInserter),
                new ImmediateDownObserver(),
                new AcceptingSink(),
                5);
            upOwner.Handle(upBegin);
            upOwner.Handle(DownReceipt(upBegin));
            upOwner.Handle(Continuation(upBegin, ButtonWireCommandKind.ReleaseGesture, 3));
            Equal(2, upInserter.Calls.Count,
                "deadline-refused normal Up makes only the earlier Down and one cleanup Up call");
            Equal(Miv.UiSmoke.ButtonDraft.GesturePhase.CleanupRequired,
                upOwner.Phase,
                "deadline-refused normal Up enters owned cleanup after cleanup count zero");
            True(upOwner.PrimaryFailure.Contains("DeadlineExpired"),
                "normal Up refusal reason reaches reducer before cleanup");
        }

        private static void TypedAttemptPreservesInsertedCountAcrossPostCallFault()
        {
            ButtonWireRequest begin = Request(0x4d490001UL, 0x4d490002UL);
            ButtonGestureTuple gesture = new ButtonGestureTuple(begin);
            FakeInputFacts facts = new FakeInputFacts(
                Probe(gesture, ButtonNormalSendEdge.Down, ButtonPressedState.None, 0),
                "DPI restore failed",
                null);
            FakeNativeInserter inserter = new FakeNativeInserter();
            inserter.Results.Enqueue(new ButtonNativeInsertion(1, 87));
            inserter.Results.Enqueue(new ButtonNativeInsertion(1, 0));
            Win32ButtonInputBackend backend = new Win32ButtonInputBackend(facts, inserter);
            FakeClock clock = new FakeClock(10);
            ButtonHelperOwner owner = new ButtonHelperOwner(
                begin.SessionNonce,
                new FakeAppLease(begin.AppProcessId, begin.AppCreationIdentity),
                clock,
                backend,
                new NeverObserveDelivery(),
                new AcceptingSink(),
                5);
            owner.Handle(begin);
            Equal(2, inserter.Calls.Count,
                "count-one Down is recorded before post-call fault starts one cleanup Up");
            Equal(ButtonNormalSendEdge.Down, inserter.Calls[0].Edge, "first call is Down");
            Equal(ButtonNormalSendEdge.Up, inserter.Calls[1].Edge, "second call is cleanup Up");
            True(owner.HasOutstandingRelease, "post-call fault cannot lose Down release obligation");
            True(owner.Phase != Miv.UiSmoke.ButtonDraft.GesturePhase.Succeeded,
                "post-call fault cannot publish success");
            clock.Now = 200;
            owner.Tick();
            True(owner.IsTerminal, "unobserved cleanup reaches bounded failure terminal");

            FakeNativeInserter residualInserter = new FakeNativeInserter();
            residualInserter.Results.Enqueue(new ButtonNativeInsertion(1, 87));
            ButtonSendAttempt residual = new Win32ButtonInputBackend(
                new FakeInputFacts(Probe(
                    gesture,
                    ButtonNormalSendEdge.Down,
                    ButtonPressedState.None,
                    0)),
                residualInserter).SendNormal(
                    new ButtonNormalSendRequest(
                        ButtonNormalSendEdge.Down,
                        gesture,
                        gesture.DownTag),
                    delegate { return ButtonSendPermission.Allowed; });
            ButtonSendCalled called = residual as ButtonSendCalled;
            True(called != null && called.Inserted == 1,
                "count one remains Called when the residual last error is nonzero");
            Equal(87, called.LastError, "residual last error is diagnostic metadata");
            True(called.FailureDetail == null,
                "residual last error does not turn count one into insertion failure");

            FakeInputFacts throwingFinishFacts = new FakeInputFacts(Probe(
                gesture,
                ButtonNormalSendEdge.Down,
                ButtonPressedState.None,
                0));
            throwingFinishFacts.ThrowOnFinish = true;
            FakeNativeInserter throwingFinishInserter = new FakeNativeInserter();
            throwingFinishInserter.Results.Enqueue(new ButtonNativeInsertion(1, 0));
            ButtonSendAttempt throwingFinish = new Win32ButtonInputBackend(
                throwingFinishFacts,
                throwingFinishInserter).SendNormal(
                    new ButtonNormalSendRequest(
                        ButtonNormalSendEdge.Down,
                        gesture,
                        gesture.DownTag),
                    delegate { return ButtonSendPermission.Allowed; });
            True(throwingFinish.WasCalled && throwingFinish.Inserted == 1,
                "scope cleanup exception cannot erase the returned insertion count");
            True(throwingFinish.PostCallFault.Contains("scope cleanup threw"),
                "scope cleanup exception is separate post-call metadata");
        }

        private static void CountZeroCannotCreateReleaseObligation()
        {
            ButtonWireRequest begin = Request(0x4d490001UL, 0x4d490002UL);
            ButtonGestureTuple gesture = new ButtonGestureTuple(begin);
            FakeNativeInserter inserter = new FakeNativeInserter();
            inserter.Results.Enqueue(new ButtonNativeInsertion(0, 5));
            ButtonHelperOwner owner = new ButtonHelperOwner(
                begin.SessionNonce,
                new FakeAppLease(begin.AppProcessId, begin.AppCreationIdentity),
                new FakeClock(10),
                new Win32ButtonInputBackend(
                    new FakeInputFacts(Probe(
                        gesture,
                        ButtonNormalSendEdge.Down,
                        ButtonPressedState.None,
                        0)),
                    inserter),
                new NeverObserveDelivery(),
                new AcceptingSink(),
                5);
            owner.Handle(begin);
            Equal(1, inserter.Calls.Count, "count-zero Down makes no cleanup Up call");
            True(owner.IsTerminal, "count-zero Down is terminal");
            True(!owner.HasOutstandingRelease,
                "count-zero Down cannot create an owned release obligation");
            Equal(Miv.UiSmoke.ButtonDraft.GesturePhase.FailedBeforeDown,
                owner.Phase,
                "count-zero Down fails before an owned press exists");
            True(owner.PrimaryFailure.Contains("last_error=5"),
                "Called-zero last error reaches reducer primary failure");
        }

        private static void OwnedCleanupDoesNotRevalidateHistoricalTarget()
        {
            ButtonGestureTuple gesture = Gesture(0x4d490001UL, 0x4d490002UL);
            FakeInputFacts facts = new FakeInputFacts(null);
            facts.ThrowIfOpened = true;
            FakeNativeInserter inserter = new FakeNativeInserter();
            ButtonSendAttempt cleanup = new Win32ButtonInputBackend(facts, inserter)
                .SendOwnedCleanupUp(new ButtonCleanupSendRequest(gesture, gesture.UpTag));
            True(cleanup.WasCalled && cleanup.Inserted == 1,
                "owned cleanup sends one global Up without old target facts");
            Equal(0, facts.OpenCount, "cleanup does not open normal target facts");
            Equal(1, facts.CleanupOpenCount, "cleanup opens one input-desktop-only scope");
            Equal(ButtonNormalSendEdge.Up, inserter.Calls[0].Edge, "cleanup edge is LeftUp");

            ButtonGestureTuple overflow = Gesture(0x4d490001UL, (ulong)UInt32.MaxValue + 1UL);
            FakeNativeInserter overflowInserter = new FakeNativeInserter();
            ButtonSendAttempt badTag = new Win32ButtonInputBackend(facts, overflowInserter)
                .SendOwnedCleanupUp(new ButtonCleanupSendRequest(overflow, overflow.UpTag));
            True(!badTag.WasCalled, "non-u32 cleanup tag is refused before native call");
            Equal(0, overflowInserter.Calls.Count, "overflow tag makes no native call");
        }

        private static void OwnedCleanupRequiresInputDesktopLease()
        {
            ButtonGestureTuple gesture = Gesture(0x4d490001UL, 0x4d490002UL);

            FakeInputFacts unavailable = new FakeInputFacts(null);
            unavailable.CleanupThrowIfOpened = true;
            FakeNativeInserter refusedInserter = new FakeNativeInserter();
            ButtonSendAttempt refused = new Win32ButtonInputBackend(unavailable, refusedInserter)
                .SendOwnedCleanupUp(new ButtonCleanupSendRequest(gesture, gesture.UpTag));
            True(!refused.WasCalled,
                "cleanup refuses insertion when the current input desktop cannot be certified");
            Equal(0, refusedInserter.Calls.Count,
                "cleanup access failure happens before native insertion");

            List<string> order = new List<string>();
            FakeInputFacts facts = new FakeInputFacts(null, null, order);
            facts.CleanupFinishFault = "input desktop handle close failed";
            FakeNativeInserter inserter = new FakeNativeInserter(order);
            ButtonSendAttempt called = new Win32ButtonInputBackend(facts, inserter)
                .SendOwnedCleanupUp(new ButtonCleanupSendRequest(gesture, gesture.UpTag));
            True(called.WasCalled && called.Inserted == 1,
                "post-call cleanup fault cannot erase an inserted cleanup Up");
            True(called.PostCallFault.Contains("handle close failed"),
                "cleanup scope fault remains separate post-call evidence");
            Equal("cleanup_facts,insert,cleanup_finish,finish",
                String.Join(",", order.ToArray()),
                "cleanup input desktop scope surrounds exactly one insertion");

            FakeInputFacts throwing = new FakeInputFacts(null);
            throwing.CleanupThrowOnFinish = true;
            FakeNativeInserter throwingInserter = new FakeNativeInserter();
            ButtonSendAttempt throwingFinish = new Win32ButtonInputBackend(
                throwing,
                throwingInserter).SendOwnedCleanupUp(
                    new ButtonCleanupSendRequest(gesture, gesture.UpTag));
            True(throwingFinish.WasCalled && throwingFinish.Inserted == 1,
                "cleanup scope exception cannot erase the returned insertion count");
            True(throwingFinish.PostCallFault.Contains("scope cleanup threw"),
                "cleanup scope exception remains separate post-call evidence");
        }

        private static void TagValidationIsExactBeforeInsertion()
        {
            ButtonGestureTuple gesture = Gesture(0xF1234567UL, 0xF1234568UL);
            FakeInputFacts facts = new FakeInputFacts(Probe(
                gesture,
                ButtonNormalSendEdge.Down,
                ButtonPressedState.None,
                0));
            ulong[] invalid = new ulong[] { 0, gesture.UpTag, (ulong)UInt32.MaxValue + 1UL };
            for (int i = 0; i < invalid.Length; i++)
            {
                FakeNativeInserter inserter = new FakeNativeInserter();
                ButtonSendAttempt result = new Win32ButtonInputBackend(facts, inserter)
                    .SendNormal(
                        new ButtonNormalSendRequest(
                            ButtonNormalSendEdge.Down,
                            gesture,
                            invalid[i]),
                        delegate { return ButtonSendPermission.Allowed; });
                True(!result.WasCalled, "invalid or wrong-edge tag " + i + " is refused");
                Equal(0, inserter.Calls.Count, "invalid tag makes no native call");
            }
            FakeNativeInserter valid = new FakeNativeInserter();
            ButtonSendAttempt accepted = new Win32ButtonInputBackend(facts, valid)
                .SendNormal(
                    new ButtonNormalSendRequest(
                        ButtonNormalSendEdge.Down,
                        gesture,
                        gesture.DownTag),
                    delegate { return ButtonSendPermission.Allowed; });
            True(accepted.WasCalled && accepted.Inserted == 1,
                "sign-bit-set nonzero u32 tag is accepted");
            Equal(unchecked((uint)gesture.DownTag), valid.Calls[0].Tag,
                "full sign-bit-set u32 tag reaches the native seam");
        }

        private static void MouseInputAbiIsOneExactTaggedEdge()
        {
            int expectedMouseSize = IntPtr.Size == 8 ? 32 : 24;
            int expectedInputSize = IntPtr.Size == 8 ? 40 : 28;
            int expectedUnionOffset = IntPtr.Size == 8 ? 8 : 4;
            int expectedTagOffset = IntPtr.Size == 8 ? 24 : 20;
            Equal(expectedMouseSize, Marshal.SizeOf(typeof(ButtonInputNative.MouseInput)),
                "MOUSEINPUT architecture size");
            Equal(expectedInputSize, Marshal.SizeOf(typeof(ButtonInputNative.Input)),
                "INPUT architecture size");
            Equal(expectedUnionOffset,
                Marshal.OffsetOf(typeof(ButtonInputNative.Input), "Data").ToInt32(),
                "INPUT union offset");
            Equal(expectedTagOffset,
                Marshal.OffsetOf(typeof(ButtonInputNative.MouseInput), "ExtraInfo").ToInt32(),
                "MOUSEINPUT ULONG_PTR offset");

            const uint tag = 0xF1234567U;
            ButtonInputNative.Input down = NativeButtonEventInserter.BuildInput(
                ButtonNormalSendEdge.Down,
                tag);
            Equal((uint)0, down.Type, "INPUT_MOUSE type");
            Equal((uint)0, down.Data.Mouse.MouseData, "button mouseData zero");
            Equal(0, down.Data.Mouse.Dx, "button dx zero");
            Equal(0, down.Data.Mouse.Dy, "button dy zero");
            Equal((uint)0, down.Data.Mouse.Time, "button time zero");
            Equal(ButtonInputNative.MouseEventLeftDown, down.Data.Mouse.Flags,
                "Down has only LEFTDOWN flag");
            Equal((ulong)tag, down.Data.Mouse.ExtraInfo.ToUInt64(), "full u32 Down tag");
            ButtonInputNative.Input up = NativeButtonEventInserter.BuildInput(
                ButtonNormalSendEdge.Up,
                tag);
            Equal(ButtonInputNative.MouseEventLeftUp, up.Data.Mouse.Flags,
                "Up has only LEFTUP flag");
            Equal((ulong)tag, up.Data.Mouse.ExtraInfo.ToUInt64(), "full u32 Up tag");
        }

        private static ButtonGestureTuple Gesture(ulong downTag, ulong upTag)
        {
            return new ButtonGestureTuple(Request(downTag, upTag));
        }

        private static ButtonWireRequest Request(ulong downTag, ulong upTag)
        {
            ButtonWireRequest request = new ButtonWireRequest();
            request.Kind = ButtonWireCommandKind.BeginGesture;
            request.SessionNonce = new Guid("11223344-5566-7788-99aa-bbccddeeff00");
            request.GestureId = 41;
            request.Step = 1;
            request.DeadlineTick = 100;
            request.AppProcessId = 500;
            request.AppCreationIdentity = 600;
            request.Target = new ButtonWireTarget();
            request.Target.ParentHwnd = 700;
            request.Target.InputHwnd = 701;
            request.Target.ScreenX = 702;
            request.Target.ScreenY = 703;
            request.Target.PlacementGeneration = 704;
            request.Target.HostIncarnation = 705;
            request.Target.BackendToken = 706;
            request.DownTag = downTag;
            request.UpTag = upTag;
            request.Delivery = new ButtonWireDelivery();
            return request;
        }

        private static ButtonWireRequest Continuation(
            ButtonWireRequest begin,
            ButtonWireCommandKind kind,
            uint step)
        {
            ButtonWireRequest request = Request(begin.DownTag, begin.UpTag);
            request.Kind = kind;
            request.SessionNonce = begin.SessionNonce;
            request.GestureId = begin.GestureId;
            request.Step = step;
            request.DeadlineTick = begin.DeadlineTick;
            request.AppProcessId = begin.AppProcessId;
            request.AppCreationIdentity = begin.AppCreationIdentity;
            request.Target = begin.Target;
            request.Flags = 0;
            request.Delivery = new ButtonWireDelivery();
            return request;
        }

        private static ButtonWireRequest DownReceipt(ButtonWireRequest begin)
        {
            ButtonWireRequest request = Continuation(
                begin,
                ButtonWireCommandKind.DownDispatchReceipt,
                2);
            request.Flags = 3;
            request.Delivery.ObservedTag = begin.DownTag;
            request.Delivery.ReceiverHwnd = begin.Target.InputHwnd;
            request.Delivery.ReceiverProcessId = begin.AppProcessId;
            request.Delivery.ReceiverThreadId = 800;
            request.Delivery.ActualClientX = 10;
            request.Delivery.ActualClientY = 11;
            request.Delivery.ActualScreenX = begin.Target.ScreenX;
            request.Delivery.ActualScreenY = begin.Target.ScreenY;
            return request;
        }

        private static ButtonNormalSendProbe Probe(
            ButtonGestureTuple gesture,
            ButtonNormalSendEdge edge,
            ButtonPressedState pressed,
            ulong capture)
        {
            ButtonNormalFrame frame = Frame(gesture, capture);
            return new ButtonNormalSendProbe(
                frame,
                frame,
                new ButtonScreenPoint(gesture.Target.ScreenX, gesture.Target.ScreenY),
                new ButtonScreenPoint(gesture.Target.ScreenX, gesture.Target.ScreenY),
                new ButtonInputStateFrame(false, pressed),
                new ButtonInputStateFrame(false, pressed),
                new ButtonPointTolerance(1, 1));
        }

        private static ButtonNormalFrame Frame(ButtonGestureTuple gesture, ulong capture)
        {
            return new ButtonNormalFrame(
                Access(
                    gesture,
                    gesture.Target.ParentHwnd,
                    gesture.AppProcessId,
                    gesture.AppCreationIdentity,
                    false),
                Receiver(
                    gesture,
                    gesture.Target.InputHwnd,
                    gesture.Target.ParentHwnd,
                    gesture.AppProcessId,
                    800,
                    capture));
        }

        private static ButtonAccessFrame Access(
            ButtonGestureTuple gesture,
            ulong foregroundHwnd,
            uint foregroundPid,
            ulong creationIdentity,
            bool swapped)
        {
            return new ButtonAccessFrame(
                900,
                new ButtonForegroundIdentity(
                    foregroundHwnd,
                    foregroundPid,
                    801,
                    creationIdentity,
                    1,
                    "S-1-5-21-input-backend-test",
                    0x2000),
                swapped);
        }

        private static ButtonReceiverFrame Receiver(
            ButtonGestureTuple gesture,
            ulong inputHwnd,
            ulong parentHwnd,
            uint processId,
            uint threadId,
            ulong capture)
        {
            return new ButtonReceiverFrame(
                inputHwnd,
                parentHwnd,
                processId,
                threadId,
                capture);
        }

        private static ButtonNormalSendProbe ProbeFrom(
            ButtonNormalFrame before,
            ButtonNormalFrame after,
            ButtonGestureTuple gesture,
            ButtonPressedState stateBefore,
            ButtonPressedState stateAfter)
        {
            return new ButtonNormalSendProbe(
                before,
                after,
                new ButtonScreenPoint(gesture.Target.ScreenX, gesture.Target.ScreenY),
                new ButtonScreenPoint(gesture.Target.ScreenX, gesture.Target.ScreenY),
                new ButtonInputStateFrame(false, stateBefore),
                new ButtonInputStateFrame(false, stateAfter),
                new ButtonPointTolerance(1, 1));
        }

        private static void True(bool value, string label)
        {
            if (!value) throw new Exception("FAILED: " + label);
        }

        private static void Equal<T>(T expected, T actual, string label)
        {
            if (!EqualityComparer<T>.Default.Equals(expected, actual))
                throw new Exception("FAILED: " + label + "; expected=" + expected + ", actual=" + actual);
        }

        private sealed class FakeInputFacts : IButtonInputFactSource
        {
            private readonly ButtonNormalSendProbe probe;
            private readonly string finishFault;
            private readonly List<string> order;
            internal bool ThrowIfOpened;
            internal bool ThrowOnFinish;
            internal Action AfterOpen;
            internal Action<ButtonNormalSendRequest> AfterOpenRequest;
            internal ButtonNormalSendProbe UpProbe;
            internal int OpenCount;
            internal bool CleanupThrowIfOpened;
            internal bool CleanupThrowOnFinish;
            internal string CleanupFinishFault;
            internal int CleanupOpenCount;

            internal FakeInputFacts(ButtonNormalSendProbe probe)
                : this(probe, null, null)
            {
            }

            internal FakeInputFacts(
                ButtonNormalSendProbe probe,
                string finishFault,
                List<string> order)
            {
                this.probe = probe;
                this.finishFault = finishFault;
                this.order = order;
            }

            public IButtonNormalSendLease OpenNormalSend(ButtonNormalSendRequest request)
            {
                OpenCount++;
                if (ThrowIfOpened) throw new Exception("normal facts must not be opened");
                if (order != null) order.Add("facts");
                if (AfterOpen != null) AfterOpen();
                if (AfterOpenRequest != null) AfterOpenRequest(request);
                ButtonNormalSendProbe selected = request.Edge == ButtonNormalSendEdge.Up
                    && UpProbe != null
                    ? UpProbe
                    : probe;
                return new FakeNormalSendLease(selected, finishFault, order, ThrowOnFinish);
            }

            public IButtonCleanupSendLease OpenCleanupSend()
            {
                CleanupOpenCount++;
                if (CleanupThrowIfOpened)
                    throw new Exception("cleanup input desktop is unavailable");
                if (order != null) order.Add("cleanup_facts");
                return new FakeCleanupSendLease(
                    CleanupFinishFault,
                    order,
                    CleanupThrowOnFinish);
            }
        }

        private sealed class FakeCleanupSendLease : IButtonCleanupSendLease
        {
            private readonly string finishFault;
            private readonly List<string> order;
            private readonly bool throwOnFinish;

            internal FakeCleanupSendLease(
                string finishFault,
                List<string> order,
                bool throwOnFinish)
            {
                this.finishFault = finishFault;
                this.order = order;
                this.throwOnFinish = throwOnFinish;
            }

            public string Finish()
            {
                if (order != null) order.Add("cleanup_finish");
                if (throwOnFinish) throw new Exception("injected cleanup finish exception");
                if (order != null) order.Add("finish");
                return finishFault;
            }
        }

        private sealed class FakeNormalSendLease : IButtonNormalSendLease
        {
            private readonly ButtonNormalSendProbe probe;
            private readonly string finishFault;
            private readonly List<string> order;
            private readonly bool throwOnFinish;
            internal FakeNormalSendLease(
                ButtonNormalSendProbe probe,
                string finishFault,
                List<string> order,
                bool throwOnFinish)
            {
                this.probe = probe;
                this.finishFault = finishFault;
                this.order = order;
                this.throwOnFinish = throwOnFinish;
            }
            public ButtonNormalSendProbe Probe { get { return probe; } }
            public string Finish()
            {
                if (order != null) order.Add("finish");
                if (throwOnFinish) throw new Exception("injected finish exception");
                return finishFault;
            }
        }

        private sealed class FakeNativeInserter : IButtonNativeInserter
        {
            internal sealed class Call
            {
                internal readonly ButtonNormalSendEdge Edge;
                internal readonly uint Tag;
                internal Call(ButtonNormalSendEdge edge, uint tag) { Edge = edge; Tag = tag; }
            }
            internal readonly List<Call> Calls = new List<Call>();
            internal readonly Queue<ButtonNativeInsertion> Results =
                new Queue<ButtonNativeInsertion>();
            private readonly List<string> order;
            internal FakeNativeInserter() { }
            internal FakeNativeInserter(List<string> order) { this.order = order; }
            public ButtonNativeInsertion SendOne(ButtonNormalSendEdge edge, uint tag)
            {
                Calls.Add(new Call(edge, tag));
                if (order != null) order.Add("insert");
                return Results.Count == 0 ? new ButtonNativeInsertion(1, 0) : Results.Dequeue();
            }
        }

        private sealed class FakeClock : IButtonClock
        {
            internal FakeClock(ulong now) { Now = now; }
            internal ulong Now;
            public ulong NowTick { get { return Now; } }
        }

        private sealed class FakeAppLease : IAppProcessLease
        {
            internal FakeAppLease(uint pid, ulong creation)
            {
                ProcessId = pid;
                CreationIdentity = creation;
                Alive = true;
            }
            public uint ProcessId { get; private set; }
            public ulong CreationIdentity { get; private set; }
            internal bool Alive;
            public bool IsAlive { get { return Alive; } }
        }

        private sealed class ImmediateDownObserver : IButtonDeliveryObserver
        {
            public ButtonDownObservationResult ObserveTaggedDown(
                ButtonGestureTuple gesture,
                TaggedButtonDelivery delivery)
            {
                return new ButtonDownObserved(new ObservedButtonDownAnchor(
                    gesture,
                    delivery,
                    Frame(gesture, gesture.Target.InputHwnd)));
            }

            public ButtonReleaseObservationResult ObserveNormalUp(
                ObservedButtonDownAnchor anchor,
                NormalButtonUpInsertion insertion,
                TaggedButtonDelivery delivery)
            {
                return new ButtonReleaseUnconfirmed(
                    ButtonObservationFailureKind.DesktopUnavailable,
                    "not used by owner permission test");
            }

            public ButtonReleaseObservationResult ObserveOwnedCleanupUp(
                ObservedButtonDownAnchor anchor,
                OwnedCleanupButtonUpInsertion insertion)
            {
                return new ButtonReleaseUnconfirmed(
                    ButtonObservationFailureKind.DesktopUnavailable,
                    "not used by owner permission test");
            }
        }

        private sealed class NeverObserveDelivery : IButtonDeliveryObserver
        {
            public ButtonDownObservationResult ObserveTaggedDown(
                ButtonGestureTuple gesture,
                TaggedButtonDelivery delivery)
            {
                return new ButtonDownUnconfirmed(
                    ButtonObservationFailureKind.DesktopUnavailable,
                    "not used by backend insertion test");
            }
            public ButtonReleaseObservationResult ObserveNormalUp(
                ObservedButtonDownAnchor anchor,
                NormalButtonUpInsertion insertion,
                TaggedButtonDelivery delivery)
            {
                return new ButtonReleaseUnconfirmed(
                    ButtonObservationFailureKind.DesktopUnavailable,
                    "not used by backend insertion test");
            }
            public ButtonReleaseObservationResult ObserveOwnedCleanupUp(
                ObservedButtonDownAnchor anchor,
                OwnedCleanupButtonUpInsertion insertion)
            {
                return new ButtonReleaseUnconfirmed(
                    ButtonObservationFailureKind.DesktopUnavailable,
                    "not used by backend insertion test");
            }
        }

        private sealed class AcceptingSink : IButtonReplySink
        {
            public bool TryPublish(ButtonWireReply reply) { return true; }
        }
    }
}
