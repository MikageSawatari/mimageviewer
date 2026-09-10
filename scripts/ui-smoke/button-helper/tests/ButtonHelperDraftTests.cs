using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Threading;
using Miv.UiSmoke.ButtonDraft;
using Miv.UiSmoke.ButtonHelperDraft;

public static class ButtonHelperDraftTests
{
    private static int passed;

    public static string RunAll()
    {
        Console.WriteLine("RUN WireCodecIsFixedBoundedAndStrict");
        WireCodecIsFixedBoundedAndStrict();
        Console.WriteLine("RUN OwnerRecordsDownObligationBeforeReply");
        OwnerRecordsDownObligationBeforeReply();
        Console.WriteLine("RUN DownDeliveryAnchorSurvivesNegativeCompletion");
        DownDeliveryAnchorSurvivesNegativeCompletion();
        Console.WriteLine("RUN MissingAndLateDownDeliveryCannotCreateAnAnchor");
        MissingAndLateDownDeliveryCannotCreateAnAnchor();
        Console.WriteLine("RUN NegativeUpAlwaysPreservesReleaseEvidenceOrdering");
        NegativeUpAlwaysPreservesReleaseEvidenceOrdering();
        Console.WriteLine("RUN OwnerRejectsWrongPointAndStaleTypedEvidence");
        OwnerRejectsWrongPointAndStaleTypedEvidence();
        Console.WriteLine("RUN AppAndOsReceiptsRemainSeparate");
        AppAndOsReceiptsRemainSeparate();
        Console.WriteLine("RUN DuplicateWrongOrderAndWrongTupleNeverResend");
        DuplicateWrongOrderAndWrongTupleNeverResend();
        Console.WriteLine("RUN ProcessDeathAndEofCleanupBeforeRunnerKill");
        ProcessDeathAndEofCleanupBeforeRunnerKill();
        Console.WriteLine("RUN ReplyQueueFailureStillRunsOwnedCleanup");
        ReplyQueueFailureStillRunsOwnedCleanup();
        Console.WriteLine("RUN BlockingWriterCannotStopOwnerDeadlineCleanup");
        BlockingWriterCannotStopOwnerDeadlineCleanup();
        Console.WriteLine("RUN FixedGestureAndCleanupDeadlinesTerminate");
        FixedGestureAndCleanupDeadlinesTerminate();
        Console.WriteLine("RUN LocalSidPipeAuthenticatesBothProcessEnds");
        LocalSidPipeAuthenticatesBothProcessEnds();
        Console.WriteLine("RUN ProcessLeasePinsHandleAndCreationIdentity");
        ProcessLeasePinsHandleAndCreationIdentity();
        Console.WriteLine("RUN PipeReaderOwnerWriterRunOneFakeGesture");
        PipeReaderOwnerWriterRunOneFakeGesture();
        Console.WriteLine("RUN ProtocolFaultAfterDownStillJoinsCleanup");
        ProtocolFaultAfterDownStillJoinsCleanup();
        Console.WriteLine("RUN LoopServicesLifecycleUnderEvidenceFlood");
        LoopServicesLifecycleUnderEvidenceFlood();
        Console.WriteLine("RUN DeadAppCleanupStillAdvancesWithoutOsEvidence");
        DeadAppCleanupStillAdvancesWithoutOsEvidence();
        Console.WriteLine("RUN OwnerCopiesImmutableGestureTupleBeforeInput");
        OwnerCopiesImmutableGestureTupleBeforeInput();
        Console.WriteLine("RUN DeadAppBeforeFinalPositiveReceiptCannotSucceed");
        DeadAppBeforeFinalPositiveReceiptCannotSucceed();
        Console.WriteLine("RUN OwnerRevalidatesAfterSynchronousObservation");
        OwnerRevalidatesAfterSynchronousObservation();
        Console.WriteLine("RUN OsObserverSeparatesNormalAndOwnedCleanupEvidence");
        OsObserverSeparatesNormalAndOwnedCleanupEvidence();
        Console.WriteLine("RUN OsObserverRejectsStaleDeadAndUncertainSamples");
        OsObserverRejectsStaleDeadAndUncertainSamples();
        Console.WriteLine("RUN OsObserverJoinsGestureDeliveryAndLiveFrames");
        OsObserverJoinsGestureDeliveryAndLiveFrames();
        Console.WriteLine("RUN OwnedCleanupEvidenceClosesFailureButNeverSuccess");
        OwnedCleanupEvidenceClosesFailureButNeverSuccess();
        passed += ButtonInputBackendDraftTests.RunAll();
        passed += ButtonHelperHostDraftTests.RunAll();
        return "PASS: " + passed + " external button helper draft tests";
    }

    private static void WireCodecIsFixedBoundedAndStrict()
    {
        FakeClock clock = new FakeClock(10);
        ButtonWireRequest request = Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock);
        byte[] encoded = ButtonWireCodec.EncodeRequest(request);
        Equal(176, ButtonWireCodec.RequestPayloadLength, "v2 request byte budget");
        Equal(ButtonWireCodec.RequestPayloadLength, encoded.Length, "fixed request length");
        ButtonWireRequest decoded = ButtonWireCodec.DecodeRequest(encoded);
        True(decoded.MatchesImmutableGesture(request), "wire round trip immutable tuple");
        Equal(request.Target.ScreenX, decoded.Target.ScreenX, "wire x");
        Equal(request.Target.ScreenY, decoded.Target.ScreenY, "wire y");

        ButtonWireRequest receipt = Request(
            ButtonWireCommandKind.DownDispatchReceipt,
            2,
            1,
            new FakeClock(10));
        ButtonWireRequest receiptRoundTrip = ButtonWireCodec.DecodeRequest(
            ButtonWireCodec.EncodeRequest(receipt));
        True(receiptRoundTrip.RawDeliveryObserved, "receipt preserves raw-delivery flag");
        Equal(receipt.Delivery.ObservedTag, receiptRoundTrip.Delivery.ObservedTag, "wire actual tag");
        Equal(receipt.Delivery.ReceiverHwnd, receiptRoundTrip.Delivery.ReceiverHwnd, "wire actual HWND");
        Equal(receipt.Delivery.ReceiverThreadId, receiptRoundTrip.Delivery.ReceiverThreadId, "wire actual TID");
        Equal(receipt.Delivery.ActualClientX, receiptRoundTrip.Delivery.ActualClientX, "wire client x");
        Equal(receipt.Delivery.ActualScreenY, receiptRoundTrip.Delivery.ActualScreenY, "wire screen y");

        ButtonWireRequest acceptedWithoutRaw = Request(
            ButtonWireCommandKind.DownDispatchReceipt,
            2,
            1,
            new FakeClock(10));
        acceptedWithoutRaw.Flags = 1;
        acceptedWithoutRaw.Delivery = new ButtonWireDelivery();
        ThrowsProtocol(
            delegate { ButtonWireCodec.EncodeRequest(acceptedWithoutRaw); },
            "accepted receipt without raw delivery");
        ButtonWireRequest fieldsWithoutRaw = Request(
            ButtonWireCommandKind.DownDispatchReceipt,
            2,
            0,
            new FakeClock(10));
        fieldsWithoutRaw.Flags = 0;
        ThrowsProtocol(
            delegate { ButtonWireCodec.EncodeRequest(fieldsWithoutRaw); },
            "delivery fields without observed flag");
        ButtonWireRequest fieldsOnBegin = Request(
            ButtonWireCommandKind.BeginGesture,
            1,
            0,
            new FakeClock(10));
        fieldsOnBegin.Delivery.ObservedTag = fieldsOnBegin.DownTag;
        ThrowsProtocol(
            delegate { ButtonWireCodec.EncodeRequest(fieldsOnBegin); },
            "delivery fields on non-receipt");

        byte[] wrongVersion = (byte[])encoded.Clone();
        wrongVersion[4] = 3;
        ThrowsProtocol(delegate { ButtonWireCodec.DecodeRequest(wrongVersion); }, "wrong version");
        byte[] reserved = (byte[])encoded.Clone();
        reserved[52] = 1;
        ThrowsProtocol(delegate { ButtonWireCodec.DecodeRequest(reserved); }, "reserved field");
        ThrowsProtocol(
            delegate { ButtonWireCodec.DecodeRequest(new byte[ButtonWireCodec.RequestPayloadLength - 1]); },
            "short payload");
        using (MemoryStream frame = new MemoryStream())
        {
            frame.Write(BitConverter.GetBytes(ButtonWireCodec.MaximumPayloadLength + 1), 0, 4);
            frame.Position = 0;
            ThrowsProtocol(delegate { ButtonWireCodec.ReadFrame(frame); }, "oversize frame");
        }
        Pass();
    }

    private static void DownDeliveryAnchorSurvivesNegativeCompletion()
    {
        FakeClock clock = new FakeClock(10);
        FakeInput input = new FakeInput();
        FakeOwnerDeliveryObserver observations = new FakeOwnerDeliveryObserver(false, true);
        ButtonHelperOwner owner = NewOwner(
            clock,
            new FakeLease(),
            input,
            new CollectingSink(),
            observations);
        owner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        owner.Handle(Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 0, clock));
        True(owner.IsTerminal, "negative Down completion closes through observed owned cleanup");
        Equal(
            GesturePhase.FailedAfterObservedCleanup,
            owner.Phase,
            "raw Down survives negative dispatch as cleanup anchor");
        Equal(1, input.CleanupUpCalls, "negative Down dispatch sends one owned cleanup Up");
        Pass();
    }

    private static void MissingAndLateDownDeliveryCannotCreateAnAnchor()
    {
        FakeClock clock = new FakeClock(10);
        FakeInput input = new FakeInput();
        FakeOwnerDeliveryObserver observations = new FakeOwnerDeliveryObserver(false, true);
        ButtonHelperOwner owner = NewOwner(
            clock,
            new FakeLease(),
            input,
            new CollectingSink(),
            observations);
        owner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        ButtonWireRequest missing = Request(
            ButtonWireCommandKind.DownDispatchReceipt,
            2,
            0,
            clock);
        missing.Flags = 0;
        missing.Delivery = new ButtonWireDelivery();
        owner.Handle(missing);
        Equal(
            GesturePhase.CleanupUpInsertedAwaitingOs,
            owner.Phase,
            "raw-missing Down starts cleanup without an anchor");
        ButtonWireRequest late = Request(
            ButtonWireCommandKind.DownDispatchReceipt,
            3,
            0,
            clock);
        ThrowsProtocol(delegate { owner.Handle(late); }, "late raw Down after cleanup");
        clock.Now = 15;
        owner.Tick();
        Equal(
            GesturePhase.FailedCleanupUnresolved,
            owner.Phase,
            "late Down cannot rescue cleanup without a historical anchor");

        FakeClock deadClock = new FakeClock(10);
        FakeLease deadLease = new FakeLease();
        FakeOwnerDeliveryObserver deadObservations = new FakeOwnerDeliveryObserver(false, true);
        ButtonHelperOwner deadOwner = NewOwner(
            deadClock,
            deadLease,
            new FakeInput(),
            new CollectingSink(),
            deadObservations);
        deadOwner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, deadClock));
        deadLease.Alive = false;
        ThrowsProtocol(
            delegate
            {
                deadOwner.Handle(Request(
                    ButtonWireCommandKind.DownDispatchReceipt,
                    2,
                    1,
                    deadClock));
            },
            "raw Down from dead process lease");
        True(deadObservations.LastAnchor == null, "dead process receipt cannot publish a Down anchor");
        Pass();
    }

    private static void NegativeUpAlwaysPreservesReleaseEvidenceOrdering()
    {
        FakeClock clock = new FakeClock(10);
        FakeInput observedInput = new FakeInput();
        FakeOwnerDeliveryObserver observed = new FakeOwnerDeliveryObserver(true, false);
        ButtonHelperOwner observedOwner = NewOwner(
            clock,
            new FakeLease(),
            observedInput,
            new CollectingSink(),
            observed);
        observedOwner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        observedOwner.Handle(Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, clock));
        observedOwner.Handle(Request(ButtonWireCommandKind.ReleaseGesture, 3, 0, clock));
        observedOwner.Handle(Request(ButtonWireCommandKind.UpSemanticReceipt, 4, 0, clock));
        Equal(
            GesturePhase.FailedAfterObservedUp,
            observedOwner.Phase,
            "valid OS Up is applied before negative App completion");
        Equal(0, observedInput.CleanupUpCalls, "observed release needs no duplicate cleanup Up");

        FakeInput unconfirmedInput = new FakeInput();
        FakeOwnerDeliveryObserver unconfirmed = new FakeOwnerDeliveryObserver(false, true);
        ButtonHelperOwner unconfirmedOwner = NewOwner(
            clock,
            new FakeLease(),
            unconfirmedInput,
            new CollectingSink(),
            unconfirmed);
        unconfirmedOwner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        unconfirmedOwner.Handle(Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, clock));
        unconfirmedOwner.Handle(Request(ButtonWireCommandKind.ReleaseGesture, 3, 0, clock));
        unconfirmedOwner.Handle(Request(ButtonWireCommandKind.UpSemanticReceipt, 4, 0, clock));
        Equal(
            GesturePhase.FailedAfterObservedCleanup,
            unconfirmedOwner.Phase,
            "negative App completion still starts owned cleanup when normal OS Up is unconfirmed");
        Equal(1, unconfirmedInput.CleanupUpCalls, "unconfirmed normal Up gets one cleanup Up");
        Pass();
    }

    private static void OwnerRejectsWrongPointAndStaleTypedEvidence()
    {
        FakeClock clock = new FakeClock(10);
        FakeObservationPlatform strictPlatform = new FakeObservationPlatform();
        strictPlatform.EnqueueNormal(NormalFrame(700, 701));
        strictPlatform.EnqueueLevel(ButtonPhysicalLevel.Down);
        strictPlatform.EnqueueNormal(NormalFrame(700, 701));
        ButtonHelperOwner wrongPointOwner = NewOwner(
            clock,
            new FakeLease(),
            new FakeInput(),
            new CollectingSink(),
            new ButtonOsObserver(strictPlatform));
        wrongPointOwner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        ButtonWireRequest wrongPoint = Request(
            ButtonWireCommandKind.DownDispatchReceipt,
            2,
            1,
            clock);
        wrongPoint.Delivery.ActualScreenX += 10;
        wrongPointOwner.Handle(wrongPoint);
        True(
            wrongPointOwner.PrimaryFailure.Contains("PointMismatch"),
            "wrong actual screen point fails at observer policy");

        FakeOwnerDeliveryObserver observations = new FakeOwnerDeliveryObserver();
        ButtonHelperOwner typedOwner = NewOwner(
            clock,
            new FakeLease(),
            new FakeInput(),
            new CollectingSink(),
            observations);
        typedOwner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        typedOwner.Handle(Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, clock));
        typedOwner.Handle(Request(ButtonWireCommandKind.ReleaseGesture, 3, 0, clock));
        typedOwner.Handle(Request(ButtonWireCommandKind.UpSemanticReceipt, 4, 1, clock));
        typedOwner.ObserveReleaseEvidence(observations.CleanupEvidence());
        True(!typedOwner.IsTerminal, "cleanup-scoped evidence cannot satisfy normal Up phase");
        ObservedButtonDownAnchor unrelatedAnchor = new ObservedButtonDownAnchor(
            observations.LastAnchor.Gesture,
            observations.LastAnchor.Delivery,
            observations.LastAnchor.Observation);
        typedOwner.ObserveReleaseEvidence(
            new NormalButtonReleaseEvidence(
                unrelatedAnchor,
                TaggedUp(unrelatedAnchor.Gesture),
                NormalFrame(700, 0)));
        True(!typedOwner.IsTerminal, "same-gesture evidence from another anchor is stale");
        typedOwner.ObserveReleaseEvidence(observations.NormalEvidence());
        Equal(GesturePhase.Succeeded, typedOwner.Phase, "current typed normal evidence completes join");

        FakeClock lateClock = new FakeClock(10);
        FakeOwnerDeliveryObserver lateObservations = new FakeOwnerDeliveryObserver();
        ButtonHelperOwner lateOwner = NewOwner(
            lateClock,
            new FakeLease(),
            new FakeInput(),
            new CollectingSink(),
            lateObservations);
        lateOwner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, lateClock));
        lateOwner.Handle(Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, lateClock));
        lateOwner.Handle(Request(ButtonWireCommandKind.ReleaseGesture, 3, 0, lateClock));
        lateOwner.Handle(Request(ButtonWireCommandKind.UpSemanticReceipt, 4, 1, lateClock));
        lateClock.Now = 100;
        lateOwner.ObserveReleaseEvidence(lateObservations.NormalEvidence());
        Equal(
            GesturePhase.FailedAfterObservedUp,
            lateOwner.Phase,
            "release evidence observed at the gesture deadline cannot complete success");
        Pass();
    }

    private static void OwnerRecordsDownObligationBeforeReply()
    {
        FakeClock clock = new FakeClock(10);
        FakeLease lease = new FakeLease();
        FakeInput input = new FakeInput();
        InspectingSink sink = new InspectingSink(input);
        ButtonHelperOwner owner = NewOwner(clock, lease, input, sink);
        sink.Owner = owner;
        owner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        Equal(1, input.DownCalls, "one Down insertion");
        True(sink.SawRecordedObligation, "release obligation existed before reply publication");
        Equal(GesturePhase.HeldAwaitingDownReceipt, owner.Phase, "await Down dispatch receipt");
        Pass();
    }

    private static void AppAndOsReceiptsRemainSeparate()
    {
        FakeClock clock = new FakeClock(10);
        FakeLease lease = new FakeLease();
        FakeInput input = new FakeInput();
        CollectingSink sink = new CollectingSink();
        FakeOwnerDeliveryObserver observations = new FakeOwnerDeliveryObserver();
        ButtonHelperOwner owner = NewOwner(clock, lease, input, sink, observations);
        owner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        owner.Handle(Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, clock));
        owner.Handle(Request(ButtonWireCommandKind.ReleaseGesture, 3, 0, clock));
        owner.Handle(Request(ButtonWireCommandKind.UpSemanticReceipt, 4, 1, clock));
        True(!owner.IsTerminal, "App Up receipt alone cannot succeed");
        True(owner.HasOutstandingRelease, "OS proof remains outstanding");
        owner.ObserveReleaseEvidence(observations.NormalEvidence());
        True(owner.IsTerminal, "typed OS release completes success");
        Equal(GesturePhase.Succeeded, owner.Phase, "success phase");

        FakeInput secondInput = new FakeInput();
        FakeOwnerDeliveryObserver secondObservations = new FakeOwnerDeliveryObserver();
        ButtonHelperOwner osFirst = NewOwner(
            clock,
            lease,
            secondInput,
            new CollectingSink(),
            secondObservations);
        osFirst.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        osFirst.Handle(Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, clock));
        osFirst.Handle(Request(ButtonWireCommandKind.ReleaseGesture, 3, 0, clock));
        osFirst.ObserveReleaseEvidence(secondObservations.NormalEvidence());
        True(!osFirst.IsTerminal, "OS proof alone cannot succeed");
        osFirst.Handle(Request(ButtonWireCommandKind.UpSemanticReceipt, 4, 1, clock));
        True(osFirst.IsTerminal, "semantic receipt completes OS-first success");
        Pass();
    }

    private static void DuplicateWrongOrderAndWrongTupleNeverResend()
    {
        FakeClock clock = new FakeClock(10);
        FakeLease lease = new FakeLease();
        FakeInput input = new FakeInput();
        ButtonHelperOwner owner = NewOwner(clock, lease, input, new CollectingSink());
        owner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        ThrowsProtocol(
            delegate { owner.Handle(Request(ButtonWireCommandKind.ReleaseGesture, 2, 0, clock)); },
            "release before Down receipt");
        Equal(0, input.UpCalls, "wrong order sends no Up");

        ButtonWireRequest duplicate = Request(ButtonWireCommandKind.DownDispatchReceipt, 1, 1, clock);
        ThrowsProtocol(delegate { owner.Handle(duplicate); }, "duplicate wire step");
        ButtonWireRequest changed = Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, clock);
        changed.Target.BackendToken++;
        ThrowsProtocol(delegate { owner.Handle(changed); }, "changed target tuple");
        Equal(1, input.DownCalls, "invalid messages do not resend Down");
        Equal(0, input.UpCalls, "invalid messages do not send Up");

        FakeInput wrongLeaseInput = new FakeInput();
        ButtonHelperOwner wrongLease = NewOwner(
            clock,
            new FakeLease(),
            wrongLeaseInput,
            new CollectingSink());
        ButtonWireRequest wrongIdentity = Request(
            ButtonWireCommandKind.BeginGesture,
            1,
            0,
            clock);
        wrongIdentity.AppCreationIdentity++;
        ThrowsProtocol(delegate { wrongLease.Handle(wrongIdentity); }, "wrong process lease");
        Equal(0, wrongLeaseInput.DownCalls, "wrong process lease sends no input");
        Pass();
    }

    private static void ProcessDeathAndEofCleanupBeforeRunnerKill()
    {
        List<string> order = new List<string>();
        FakeClock clock = new FakeClock(10);
        FakeLease lease = new FakeLease();
        FakeInput input = new FakeInput(order);
        ButtonHelperOwner owner = NewOwner(clock, lease, input, new CollectingSink());
        owner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        lease.Alive = false;
        owner.Tick();
        Equal(1, input.CleanupUpCalls, "app death triggers owned Up");
        clock.Now = 15;
        owner.Tick();
        True(owner.IsTerminal, "cleanup without a Down anchor reaches bounded failure");
        Equal(GesturePhase.FailedCleanupUnresolved, owner.Phase, "missing anchor is not rescued");
        order.Add("runner-app-kill");
        Equal("down,cleanup-up,runner-app-kill", String.Join(",", order.ToArray()), "cleanup before kill");

        FakeInput eofInput = new FakeInput();
        ButtonHelperOwner eof = NewOwner(new FakeClock(10), new FakeLease(), eofInput, new CollectingSink());
        eof.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, new FakeClock(10)));
        eof.OnPipeEof();
        Equal(1, eofInput.CleanupUpCalls, "EOF triggers same cleanup owner");
        Pass();
    }

    private static void ReplyQueueFailureStillRunsOwnedCleanup()
    {
        FakeClock clock = new FakeClock(10);
        FakeInput input = new FakeInput();
        ButtonHelperOwner owner = NewOwner(clock, new FakeLease(), input, new RejectingSink());
        owner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        Equal(1, input.DownCalls, "Down was inserted");
        Equal(1, input.CleanupUpCalls, "failed bounded reply handoff triggers cleanup");
        True(owner.HasOutstandingRelease, "cleanup needs OS proof");
        Pass();
    }

    private static void BlockingWriterCannotStopOwnerDeadlineCleanup()
    {
        FakeClock clock = new FakeClock(10);
        FakeLease lease = new FakeLease();
        FakeInput input = new FakeInput();
        BlockingTransport transport = new BlockingTransport();
        using (BoundedAsyncReplySink sink = new BoundedAsyncReplySink(transport, 4))
        {
            ButtonHelperOwner owner = NewOwner(clock, lease, input, sink);
            owner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
            True(transport.Entered.WaitOne(1000), "writer reached blocking transport");
            lease.Alive = false;
            Stopwatch watch = Stopwatch.StartNew();
            owner.Tick();
            watch.Stop();
            Equal(1, input.CleanupUpCalls, "owner cleanup progressed while writer blocked");
            True(watch.ElapsedMilliseconds < 500, "owner did not join blocked writer");
            transport.Release.Set();
        }
        Pass();
    }

    private static void FixedGestureAndCleanupDeadlinesTerminate()
    {
        FakeClock clock = new FakeClock(10);
        FakeInput input = new FakeInput();
        ButtonHelperOwner owner = NewOwner(clock, new FakeLease(), input, new CollectingSink());
        owner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        clock.Now = 100;
        owner.Tick();
        Equal((ulong)105, owner.Phase == GesturePhase.CleanupUpInsertedAwaitingOs
            ? (ulong)105 : 0, "cleanup entered at fixed gesture deadline");
        Equal(1, input.CleanupUpCalls, "deadline cleanup inserted once");
        clock.Now = 105;
        owner.Tick();
        True(owner.IsTerminal, "cleanup observation deadline terminates owner");
        Equal(GesturePhase.FailedCleanupUnresolved, owner.Phase, "unconfirmed cleanup terminal");
        Pass();
    }

    private static void LocalSidPipeAuthenticatesBothProcessEnds()
    {
        string name = "miv-ui-smoke-button-draft-" + Guid.NewGuid().ToString("N");
        uint pid = (uint)Process.GetCurrentProcess().Id;
        Exception clientError = null;
        ButtonWireRequest request = Request(
            ButtonWireCommandKind.BeginGesture,
            1,
            0,
            new FakeClock(10));
        using (LocalButtonPipeServer server = LocalButtonPipeServer.Create(name, pid))
        {
            Thread clientThread = new Thread(delegate()
            {
                try
                {
                    using (NamedPipeClientStream client = new NamedPipeClientStream(
                        ".",
                        name,
                        PipeDirection.InOut,
                        PipeOptions.None))
                    {
                        client.Connect(3000);
                        LocalButtonPipeClientProof.VerifyServerProcess(client, pid);
                        ButtonWireCodec.WriteFrame(client, ButtonWireCodec.EncodeRequest(request));
                        ButtonWireReply clientReply = ButtonWireCodec.DecodeReply(
                            ButtonWireCodec.ReadFrame(client));
                        Equal(ButtonWireReplyKind.Accepted, clientReply.Kind, "loopback reply");
                    }
                }
                catch (Exception error)
                {
                    clientError = error;
                }
            });
            clientThread.IsBackground = true;
            clientThread.Start();
            try
            {
                server.WaitForAuthenticatedConnection(3000);
            }
            catch (Exception error)
            {
                throw new Exception(
                    "server connection failed; client="
                        + (clientError == null ? "pending" : clientError.ToString()),
                    error);
            }
            ButtonWireRequest received = ButtonWireCodec.DecodeRequest(
                ButtonWireCodec.ReadFrame(server.Stream));
            True(received.MatchesImmutableGesture(request), "authenticated request tuple");
            ButtonWireReply serverReply = new ButtonWireReply();
            serverReply.Kind = ButtonWireReplyKind.Accepted;
            serverReply.SessionNonce = request.SessionNonce;
            serverReply.GestureId = request.GestureId;
            serverReply.Step = request.Step;
            ButtonWireCodec.WriteFrame(server.Stream, ButtonWireCodec.EncodeReply(serverReply));
            True(clientThread.Join(3000), "loopback client completed");
        }
        if (clientError != null) throw new Exception("loopback client failed", clientError);
        Pass();
    }

    private static void ProcessLeasePinsHandleAndCreationIdentity()
    {
        uint pid = (uint)Process.GetCurrentProcess().Id;
        using (PinnedAppProcessLease lease = new PinnedAppProcessLease(pid))
        {
            Equal(pid, lease.ProcessId, "pinned process PID");
            True(lease.CreationIdentity != 0, "pinned creation identity");
            True(lease.IsAlive, "held current-process handle is alive");
        }
        Pass();
    }

    private static void PipeReaderOwnerWriterRunOneFakeGesture()
    {
        string name = "miv-ui-smoke-button-owner-" + Guid.NewGuid().ToString("N");
        uint pid = (uint)Process.GetCurrentProcess().Id;
        const ulong creation = 9001;
        Exception clientError = null;
        ManualResetEvent clientSawTerminal = new ManualResetEvent(false);
        using (LocalButtonPipeServer server = LocalButtonPipeServer.Create(name, pid))
        {
            Thread clientThread = new Thread(delegate()
            {
                try
                {
                    using (NamedPipeClientStream client = new NamedPipeClientStream(
                        ".",
                        name,
                        PipeDirection.InOut,
                        PipeOptions.None))
                    {
                        client.Connect(3000);
                        LocalButtonPipeClientProof.VerifyServerProcess(client, pid);
                        ButtonWireCodec.WriteFrame(client, ButtonWireCodec.EncodeRequest(
                            RequestForIdentity(ButtonWireCommandKind.BeginGesture, 1, 0, pid, creation)));
                        ButtonWireCodec.WriteFrame(client, ButtonWireCodec.EncodeRequest(
                            RequestForIdentity(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, pid, creation)));
                        ButtonWireCodec.WriteFrame(client, ButtonWireCodec.EncodeRequest(
                            RequestForIdentity(ButtonWireCommandKind.ReleaseGesture, 3, 0, pid, creation)));
                        ButtonWireCodec.WriteFrame(client, ButtonWireCodec.EncodeRequest(
                            RequestForIdentity(ButtonWireCommandKind.UpSemanticReceipt, 4, 1, pid, creation)));
                        while (true)
                        {
                            ButtonWireReply reply = ButtonWireCodec.DecodeReply(
                                ButtonWireCodec.ReadFrame(client));
                            if (reply.Kind == ButtonWireReplyKind.TerminalSuccess)
                            {
                                clientSawTerminal.Set();
                                return;
                            }
                            if (reply.Kind == ButtonWireReplyKind.TerminalFailure
                                || reply.Kind == ButtonWireReplyKind.ProtocolFailure)
                            {
                                throw new Exception("helper returned " + reply.Kind);
                            }
                        }
                    }
                }
                catch (Exception error)
                {
                    clientError = error;
                }
            });
            clientThread.IsBackground = true;
            clientThread.Start();
            server.WaitForAuthenticatedConnection(3000);

            FakeClock clock = new FakeClock(10);
            FakeLease lease = new FakeLease(pid, creation);
            FakeOsSource os = new FakeOsSource();
            FakeInput input = new FakeInput();
            FakeOwnerDeliveryObserver observations = new FakeOwnerDeliveryObserver();
            input.NormalUp = delegate
            {
                os.Enqueue(observations.NormalEvidence());
            };
            using (BoundedPipeRequestReader reader = new BoundedPipeRequestReader(server.Stream, 8))
            using (BoundedAsyncReplySink sink = new BoundedAsyncReplySink(
                new PipeReplyTransport(server.Stream),
                8))
            {
                ButtonHelperOwner owner = NewOwner(clock, lease, input, sink, observations);
                ButtonHelperLoop loop = new ButtonHelperLoop(owner, reader, os, 10);
                Equal(ButtonHelperLoopResult.Succeeded, loop.Run(), "pipe/owner loop result");
                True(clientSawTerminal.WaitOne(3000), "client received terminal success");
                Equal(1, input.DownCalls, "one pipe-driven Down");
                Equal(1, input.UpCalls, "one pipe-driven Up");
                Equal(0, input.CleanupUpCalls, "normal gesture needed no cleanup Up");
            }
            True(clientThread.Join(3000), "pipe owner client completed");
        }
        clientSawTerminal.Dispose();
        if (clientError != null) throw new Exception("pipe owner client failed", clientError);
        Pass();
    }

    private static void ProtocolFaultAfterDownStillJoinsCleanup()
    {
        FakeClock clock = new FakeClock(10);
        FakeOsSource os = new FakeOsSource();
        FakeInput input = new FakeInput();
        input.CleanupUp = delegate
        {
            clock.Now = 15;
        };
        MemoryStream frames = new MemoryStream();
        ButtonWireCodec.WriteFrame(frames, ButtonWireCodec.EncodeRequest(
            Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock)));
        // Step 2 must be the Down dispatch receipt. A Release here is rejected,
        // but the already-inserted Down still has to reach a terminal cleanup.
        ButtonWireCodec.WriteFrame(frames, ButtonWireCodec.EncodeRequest(
            Request(ButtonWireCommandKind.ReleaseGesture, 2, 0, clock)));
        frames.Position = 0;
        CollectingSink sink = new CollectingSink();
        using (BoundedPipeRequestReader reader = new BoundedPipeRequestReader(frames, 4))
        {
            ButtonHelperOwner owner = NewOwner(clock, new FakeLease(), input, sink);
            ButtonHelperLoop loop = new ButtonHelperLoop(owner, reader, os, 10);
            Equal(ButtonHelperLoopResult.GestureFailed, loop.Run(), "protocol cleanup result");
            Equal(1, input.DownCalls, "protocol test inserted one Down");
            Equal(1, input.CleanupUpCalls, "protocol fault cleanup inserted one Up");
            True(owner.IsTerminal, "protocol fault waits for cleanup terminal");
            Equal(GesturePhase.FailedCleanupUnresolved, owner.Phase, "protocol cleanup lacks Down anchor");
        }
        Pass();
    }

    private static void LoopServicesLifecycleUnderEvidenceFlood()
    {
        FakeClock clock = new FakeClock(10);
        FakeLease lease = new FakeLease();
        FakeInput input = new FakeInput();
        input.Down = delegate { lease.Alive = false; };
        input.CleanupUp = delegate { clock.Now = 15; };
        ButtonGestureTuple unrelatedGesture = new ButtonGestureTuple(
            Request(ButtonWireCommandKind.BeginGesture, 1, 0, new FakeClock(10)));
        ObservedButtonDownAnchor unrelatedAnchor = new ObservedButtonDownAnchor(
            unrelatedGesture,
            TaggedDown(unrelatedGesture),
            NormalFrame(700, 701));
        LifecycleFloodOsSource os = new LifecycleFloodOsSource(
            input,
            16,
            new OwnedCleanupButtonReleaseEvidence(unrelatedAnchor, AccessFrame(900)),
            null);
        MemoryStream frames = new MemoryStream();
        ButtonWireCodec.WriteFrame(frames, ButtonWireCodec.EncodeRequest(
            Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock)));
        frames.Position = 0;
        using (BoundedPipeRequestReader reader = new BoundedPipeRequestReader(frames, 4))
        {
            Stopwatch ready = Stopwatch.StartNew();
            while (!reader.HasPending && !reader.Eof && ready.ElapsedMilliseconds < 1000)
                Thread.Sleep(1);
            True(reader.HasPending, "begin request reached bounded reader");
            ButtonHelperOwner owner = NewOwner(clock, lease, input, new CollectingSink());
            ButtonHelperLoop loop = new ButtonHelperLoop(owner, reader, os, 10);
            Equal(ButtonHelperLoopResult.GestureFailed, loop.Run(), "flood cleanup result");
            True(os.WrongTaken < os.InitialWrongCount,
                "lifecycle ran before irrelevant evidence stream drained");
            Equal(1, input.CleanupUpCalls, "flood did not starve owned cleanup");
            Equal(GesturePhase.FailedCleanupUnresolved, owner.Phase,
                "missing owned anchor terminated flooded loop at its cleanup deadline");
        }
        Pass();
    }

    private static void DeadAppCleanupStillAdvancesWithoutOsEvidence()
    {
        FakeClock clock = new FakeClock(10);
        FakeLease lease = new FakeLease();
        FakeInput input = new FakeInput();
        ButtonHelperOwner owner = NewOwner(clock, lease, input, new CollectingSink());
        owner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock));
        lease.Alive = false;
        owner.Tick();
        Equal(1, input.CleanupUpCalls, "app death inserted one cleanup Up");
        Equal(GesturePhase.CleanupUpInsertedAwaitingOs, owner.Phase,
            "app death waits for OS cleanup proof");
        clock.Now = 15;
        owner.Tick();
        True(owner.IsTerminal, "dead app still advances cleanup deadline");
        Equal(GesturePhase.FailedCleanupUnresolved, owner.Phase,
            "missing OS cleanup proof terminates as unresolved");
        Equal(1, input.CleanupUpCalls, "dead app did not repeat inserted cleanup Up");
        Pass();
    }

    private static void OwnerCopiesImmutableGestureTupleBeforeInput()
    {
        FakeClock clock = new FakeClock(10);
        FakeInput input = new FakeInput();
        ButtonHelperOwner owner = NewOwner(clock, new FakeLease(), input, new CollectingSink());
        ButtonWireRequest begin = Request(ButtonWireCommandKind.BeginGesture, 1, 0, clock);
        owner.Handle(begin);
        begin.Target.ScreenX = 9001;
        begin.Target.BackendToken = 9002;
        begin.UpTag = 9003;
        owner.Handle(Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, clock));
        owner.OnPipeEof();
        Equal(702, input.LastCleanupScreenX, "cleanup uses owned target copy");
        Equal((ulong)706, input.LastCleanupBackendToken, "backend identity is immutable");
        Equal((ulong)708, input.LastCleanupTag, "cleanup tag is immutable");
        Pass();
    }

    private static void DeadAppBeforeFinalPositiveReceiptCannotSucceed()
    {
        // OS proof arrived first.  The queued App receipt must not complete the
        // gesture after the exact pinned process is already observably dead.
        FakeClock appLastClock = new FakeClock(10);
        FakeLease appLastLease = new FakeLease();
        FakeInput appLastInput = new FakeInput();
        FakeOwnerDeliveryObserver appLastObservations = new FakeOwnerDeliveryObserver();
        ButtonHelperOwner appLast = NewOwner(
            appLastClock,
            appLastLease,
            appLastInput,
            new CollectingSink(),
            appLastObservations);
        appLast.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, appLastClock));
        appLast.Handle(Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, appLastClock));
        appLast.Handle(Request(ButtonWireCommandKind.ReleaseGesture, 3, 0, appLastClock));
        appLast.ObserveReleaseEvidence(appLastObservations.NormalEvidence());
        appLastLease.Alive = false;
        MemoryStream appReceiptFrame = new MemoryStream();
        ButtonWireCodec.WriteFrame(appReceiptFrame, ButtonWireCodec.EncodeRequest(
            Request(ButtonWireCommandKind.UpSemanticReceipt, 4, 1, appLastClock)));
        appReceiptFrame.Position = 0;
        using (BoundedPipeRequestReader reader = new BoundedPipeRequestReader(appReceiptFrame, 2))
        {
            WaitForPending(reader, "final App receipt");
            ButtonHelperLoop loop = new ButtonHelperLoop(
                appLast,
                reader,
                new FakeOsSource(),
                10);
            Equal(ButtonHelperLoopResult.GestureFailed, loop.Run(),
                "dead app before final App receipt");
            True(appLast.IsTerminal, "App-last path reached failure terminal");
            Equal(GesturePhase.FailedAfterObservedUp, appLast.Phase,
                "App-last path preserves observed release as failure");
            Equal(0, appLastInput.CleanupUpCalls,
                "observed OS release needs no duplicate cleanup Up");
        }

        // App receipt arrived first.  A queued OS proof likewise cannot complete
        // success after the pinned process has died.  Cleanup may still use the
        // proof to close the helper's release obligation as a failure.
        FakeClock osLastClock = new FakeClock(10);
        FakeLease osLastLease = new FakeLease();
        FakeInput osLastInput = new FakeInput();
        FakeOwnerDeliveryObserver osLastObservations = new FakeOwnerDeliveryObserver();
        FakeOsSource finalOs = new FakeOsSource();
        osLastInput.CleanupUp = delegate
        {
            finalOs.Enqueue(osLastObservations.CleanupEvidence());
        };
        ButtonHelperOwner osLast = NewOwner(
            osLastClock,
            osLastLease,
            osLastInput,
            new CollectingSink(),
            osLastObservations);
        osLast.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, osLastClock));
        osLast.Handle(Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, osLastClock));
        osLast.Handle(Request(ButtonWireCommandKind.ReleaseGesture, 3, 0, osLastClock));
        osLast.Handle(Request(ButtonWireCommandKind.UpSemanticReceipt, 4, 1, osLastClock));
        osLastLease.Alive = false;
        using (BoundedPipeRequestReader reader = new BoundedPipeRequestReader(
            new MemoryStream(),
            2))
        {
            ButtonHelperLoop loop = new ButtonHelperLoop(osLast, reader, finalOs, 10);
            Equal(ButtonHelperLoopResult.GestureFailed, loop.Run(),
                "dead app before final OS receipt");
            True(osLast.IsTerminal, "OS-last path reached failure terminal");
            Equal(GesturePhase.FailedAfterObservedCleanup, osLast.Phase,
                "OS-last path closes owned cleanup as failure");
            Equal(1, osLastInput.CleanupUpCalls,
                "unobserved release still receives one owned cleanup Up");
        }
        Pass();
    }

    private static void OwnerRevalidatesAfterSynchronousObservation()
    {
        // A valid raw Down sample cannot become the cleanup anchor if the exact
        // process dies before the synchronous observer returns to the owner.
        FakeClock downClock = new FakeClock(10);
        FakeLease downLease = new FakeLease();
        FakeInput downInput = new FakeInput();
        FakeOwnerDeliveryObserver downObservations = new FakeOwnerDeliveryObserver(false, true);
        downObservations.BeforeDownReturn = delegate { downLease.Alive = false; };
        ButtonHelperOwner downOwner = NewOwner(
            downClock,
            downLease,
            downInput,
            new CollectingSink(),
            downObservations);
        downOwner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, downClock));
        downOwner.Handle(Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, downClock));
        True(downOwner.Phase != GesturePhase.Succeeded,
            "dead process during Down observation cannot succeed");
        Equal(1, downInput.CleanupUpCalls,
            "death after Down observation still starts owned cleanup");
        downClock.Now = 200;
        downOwner.Tick();
        True(downOwner.IsTerminal, "missing committed Down anchor resolves as failure");
        Equal(GesturePhase.FailedCleanupUnresolved, downOwner.Phase,
            "uncommitted Down proof is not retroactively accepted");

        // A valid normal Up result likewise cannot bypass a process death that
        // becomes observable while the synchronous observer is sampling.
        FakeClock deadUpClock = new FakeClock(10);
        FakeLease deadUpLease = new FakeLease();
        FakeInput deadUpInput = new FakeInput();
        FakeOwnerDeliveryObserver deadUpObservations = new FakeOwnerDeliveryObserver(true, true);
        deadUpObservations.BeforeNormalUpReturn = delegate { deadUpLease.Alive = false; };
        ButtonHelperOwner deadUpOwner = NewOwner(
            deadUpClock,
            deadUpLease,
            deadUpInput,
            new CollectingSink(),
            deadUpObservations);
        deadUpOwner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, deadUpClock));
        deadUpOwner.Handle(Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, deadUpClock));
        deadUpOwner.Handle(Request(ButtonWireCommandKind.ReleaseGesture, 3, 0, deadUpClock));
        deadUpOwner.Handle(Request(ButtonWireCommandKind.UpSemanticReceipt, 4, 1, deadUpClock));
        True(deadUpOwner.IsTerminal,
            "death during valid Up observation reaches failure terminal");
        Equal(GesturePhase.FailedAfterObservedCleanup, deadUpOwner.Phase,
            "dead process cannot commit normal release evidence as success");
        Equal(1, deadUpInput.CleanupUpCalls,
            "death during Up observation performs exactly one cleanup Up");

        // The deadline is re-read after a valid normal Up observation and before
        // either OS evidence or the positive App receipt can complete success.
        FakeClock upClock = new FakeClock(10);
        FakeLease upLease = new FakeLease();
        FakeInput upInput = new FakeInput();
        FakeOwnerDeliveryObserver upObservations = new FakeOwnerDeliveryObserver(true, true);
        upObservations.BeforeNormalUpReturn = delegate { upClock.Now = 100; };
        ButtonHelperOwner upOwner = NewOwner(
            upClock,
            upLease,
            upInput,
            new CollectingSink(),
            upObservations);
        upOwner.Handle(Request(ButtonWireCommandKind.BeginGesture, 1, 0, upClock));
        upOwner.Handle(Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 1, upClock));
        upOwner.Handle(Request(ButtonWireCommandKind.ReleaseGesture, 3, 0, upClock));
        upOwner.Handle(Request(ButtonWireCommandKind.UpSemanticReceipt, 4, 1, upClock));
        True(upOwner.IsTerminal, "deadline during Up observation reaches failure terminal");
        Equal(GesturePhase.FailedAfterObservedCleanup, upOwner.Phase,
            "deadline path uses owned cleanup rather than normal success");
        Equal(1, upInput.CleanupUpCalls,
            "deadline during Up observation performs exactly one cleanup Up");
        Pass();
    }

    private static void WaitForPending(BoundedPipeRequestReader reader, string label)
    {
        Stopwatch ready = Stopwatch.StartNew();
        while (!reader.HasPending && !reader.Eof && ready.ElapsedMilliseconds < 1000)
            Thread.Sleep(1);
        True(reader.HasPending, label + " reached bounded reader");
    }

    private static void OsObserverSeparatesNormalAndOwnedCleanupEvidence()
    {
        ButtonGestureTuple gesture = new ButtonGestureTuple(
            Request(ButtonWireCommandKind.BeginGesture, 1, 0, new FakeClock(10)));
        FakeObservationPlatform platform = new FakeObservationPlatform();
        platform.EnqueueNormal(NormalFrame(700, 701));
        platform.EnqueueLevel(ButtonPhysicalLevel.Down);
        platform.EnqueueNormal(NormalFrame(700, 701));
        ButtonOsObserver observer = new ButtonOsObserver(platform);
        ButtonDownObserved down = observer.ObserveTaggedDown(
            gesture,
            TaggedDown(gesture)) as ButtonDownObserved;
        True(down != null, "tagged Down plus capture and physical level creates anchor");

        platform.EnqueueNormal(NormalFrame(700, 0));
        platform.EnqueueLevel(ButtonPhysicalLevel.Up);
        platform.EnqueueNormal(NormalFrame(700, 0));
        ButtonReleaseObserved normal = observer.ObserveNormalUp(
            down.Anchor,
            new NormalButtonUpInsertion(gesture.GestureId, gesture.UpTag, 1),
            TaggedUp(gesture)) as ButtonReleaseObserved;
        True(normal != null, "normal tagged Up creates normal release evidence");
        Equal(ButtonObservationScope.Normal, normal.Evidence.Scope, "normal evidence scope");
        Equal(
            ButtonEvidenceDispatchDisposition.Accepted,
            ButtonEvidenceDispatch.Classify(
                ButtonObservationScope.Normal,
                gesture.GestureId,
                normal.Evidence),
            "normal evidence routes only to normal phase");
        Equal(
            ButtonEvidenceDispatchDisposition.WrongScope,
            ButtonEvidenceDispatch.Classify(
                ButtonObservationScope.OwnedCleanup,
                gesture.GestureId,
                normal.Evidence),
            "normal evidence cannot close cleanup phase");

        platform.EnqueueAccess(AccessFrame(900));
        platform.EnqueueLevel(ButtonPhysicalLevel.Up);
        platform.EnqueueAccess(AccessFrame(900));
        ButtonReleaseObserved cleanup = observer.ObserveOwnedCleanupUp(
            down.Anchor,
            new OwnedCleanupButtonUpInsertion(gesture.GestureId, gesture.UpTag, 1))
            as ButtonReleaseObserved;
        True(cleanup != null, "historical Down permits scoped owned-cleanup proof");
        Equal(ButtonObservationScope.OwnedCleanup, cleanup.Evidence.Scope, "cleanup evidence scope");
        Equal(
            ButtonEvidenceDispatchDisposition.Accepted,
            ButtonEvidenceDispatch.Classify(
                ButtonObservationScope.OwnedCleanup,
                gesture.GestureId,
                cleanup.Evidence),
            "cleanup evidence routes only to cleanup phase");
        Equal(
            ButtonEvidenceDispatchDisposition.WrongScope,
            ButtonEvidenceDispatch.Classify(
                ButtonObservationScope.Normal,
                gesture.GestureId,
                cleanup.Evidence),
            "cleanup evidence cannot claim normal success");
        Pass();
    }

    private static void OsObserverRejectsStaleDeadAndUncertainSamples()
    {
        ButtonGestureTuple gesture = new ButtonGestureTuple(
            Request(ButtonWireCommandKind.BeginGesture, 1, 0, new FakeClock(10)));
        FakeObservationPlatform platform = new FakeObservationPlatform();
        ButtonOsObserver observer = new ButtonOsObserver(platform);
        TaggedButtonDelivery stale = TaggedDown(gesture);
        stale = new TaggedButtonDelivery(
            gesture.GestureId + 1,
            stale.Tag,
            stale.Edge,
            stale.ReceiverHwnd,
            stale.ReceiverProcessId,
            stale.ReceiverThreadId,
            stale.ActualClientX,
            stale.ActualClientY,
            stale.ActualScreenX,
            stale.ActualScreenY);
        ButtonDownUnconfirmed staleResult = observer.ObserveTaggedDown(gesture, stale)
            as ButtonDownUnconfirmed;
        Equal(ButtonObservationFailureKind.WrongGesture, staleResult.Kind, "stale Down gesture");

        platform.EnqueueNormal(NormalFrame(700, 701));
        platform.EnqueueLevel(ButtonPhysicalLevel.Down);
        platform.EnqueueNormal(NormalFrame(700, 701));
        ButtonDownObserved down = observer.ObserveTaggedDown(gesture, TaggedDown(gesture))
            as ButtonDownObserved;
        True(down != null, "fixture established historical Down");

        platform.EnqueueNormalFailure(ButtonObservationFailureKind.ReceiverUnavailable);
        ButtonReleaseUnconfirmed deadNormal = observer.ObserveNormalUp(
            down.Anchor,
            new NormalButtonUpInsertion(gesture.GestureId, gesture.UpTag, 1),
            TaggedUp(gesture)) as ButtonReleaseUnconfirmed;
        Equal(
            ButtonObservationFailureKind.ReceiverUnavailable,
            deadNormal.Kind,
            "dead receiver rejects normal release");

        ButtonReleaseUnconfirmed missingAnchor = observer.ObserveOwnedCleanupUp(
            null,
            new OwnedCleanupButtonUpInsertion(gesture.GestureId, gesture.UpTag, 1))
            as ButtonReleaseUnconfirmed;
        Equal(
            ButtonObservationFailureKind.MissingHistoricalDown,
            missingAnchor.Kind,
            "physical Up without historical Down is unconfirmed");

        platform.EnqueueAccessFailure(ButtonObservationFailureKind.TokenAccessUnavailable);
        ButtonReleaseUnconfirmed tokenFailure = observer.ObserveOwnedCleanupUp(
            down.Anchor,
            new OwnedCleanupButtonUpInsertion(gesture.GestureId, gesture.UpTag, 1))
            as ButtonReleaseUnconfirmed;
        Equal(
            ButtonObservationFailureKind.TokenAccessUnavailable,
            tokenFailure.Kind,
            "token access uncertainty remains unconfirmed");

        platform.EnqueueAccess(AccessFrame(900));
        platform.EnqueueLevel(ButtonPhysicalLevel.Up);
        platform.EnqueueAccess(AccessFrame(901));
        ButtonReleaseUnconfirmed desktopChanged = observer.ObserveOwnedCleanupUp(
            down.Anchor,
            new OwnedCleanupButtonUpInsertion(gesture.GestureId, gesture.UpTag, 1))
            as ButtonReleaseUnconfirmed;
        Equal(
            ButtonObservationFailureKind.DesktopChanged,
            desktopChanged.Kind,
            "desktop change around level sample is unconfirmed");

        platform.EnqueueAccess(SwappedAccessFrame(900));
        platform.EnqueueLevel(ButtonPhysicalLevel.Up);
        platform.EnqueueAccess(SwappedAccessFrame(900));
        ButtonReleaseUnconfirmed swapped = observer.ObserveOwnedCleanupUp(
            down.Anchor,
            new OwnedCleanupButtonUpInsertion(gesture.GestureId, gesture.UpTag, 1))
            as ButtonReleaseUnconfirmed;
        Equal(
            ButtonObservationFailureKind.MappingUnsupported,
            swapped.Kind,
            "swapped mapping is unsupported rather than guessed");
        Pass();
    }

    private static void OwnedCleanupEvidenceClosesFailureButNeverSuccess()
    {
        ButtonGestureTuple gesture = new ButtonGestureTuple(
            Request(ButtonWireCommandKind.BeginGesture, 1, 0, new FakeClock(10)));
        FakeObservationPlatform platform = new FakeObservationPlatform();
        platform.EnqueueNormal(NormalFrame(700, 701));
        platform.EnqueueLevel(ButtonPhysicalLevel.Down);
        platform.EnqueueNormal(NormalFrame(700, 701));
        ButtonOsObserver observer = new ButtonOsObserver(platform);
        ButtonDownObserved down = observer.ObserveTaggedDown(gesture, TaggedDown(gesture))
            as ButtonDownObserved;
        True(down != null, "cleanup reducer fixture established historical Down");

        // The original target may disappear. Cleanup observes only the current
        // accessibility context; it never treats that foreground as a new target.
        platform.EnqueueAccess(AccessFrame(999));
        platform.EnqueueLevel(ButtonPhysicalLevel.Up);
        platform.EnqueueAccess(AccessFrame(999));
        ButtonReleaseObserved release = observer.ObserveOwnedCleanupUp(
            down.Anchor,
            new OwnedCleanupButtonUpInsertion(gesture.GestureId, gesture.UpTag, 1))
            as ButtonReleaseObserved;
        True(release != null, "dead old target does not suppress owned release proof");

        ButtonGestureReducer reducer = new ButtonGestureReducer(41, 100, 5);
        Equal(DriverAction.SendDown, reducer.BeginDown(10), "reducer cleanup fixture Down");
        Equal(DriverAction.None, reducer.RecordDownInsertion(1, 10), "Down insertion recorded");
        Equal(DriverAction.SendCleanupUp, reducer.Cancel("app exited", 11), "death starts cleanup");
        Equal(DriverAction.None, reducer.RecordCleanupInsertion(1, null), "cleanup Up insertion recorded");
        Equal(
            ButtonEvidenceDispatchDisposition.Accepted,
            ButtonEvidenceDispatch.Classify(
                ButtonObservationScope.OwnedCleanup,
                reducer.GestureId,
                release.Evidence),
            "typed cleanup evidence matches reducer cleanup scope");
        AsyncOutcome outcome = reducer.RecordCleanupOsObservation(
            reducer.GestureId,
            OsButtonObservation.ReleasedAfterObservedDown,
            12);
        Equal(DriverAction.ReportFailure, outcome.Action, "observed cleanup reports scenario failure");
        Equal(
            GesturePhase.FailedAfterObservedCleanup,
            reducer.Phase,
            "cleanup proof cannot revive the scenario as success");

        ButtonReleaseUnconfirmed noAnchor = observer.ObserveOwnedCleanupUp(
            null,
            new OwnedCleanupButtonUpInsertion(gesture.GestureId, gesture.UpTag, 1))
            as ButtonReleaseUnconfirmed;
        Equal(
            ButtonObservationFailureKind.MissingHistoricalDown,
            noAnchor.Kind,
            "death before a valid Down anchor leaves cleanup unconfirmed");
        ButtonGestureReducer unresolved = new ButtonGestureReducer(41, 100, 5);
        unresolved.BeginDown(10);
        unresolved.RecordDownInsertion(1, 10);
        unresolved.Cancel("app exited before Down observation", 11);
        unresolved.RecordCleanupInsertion(1, null);
        Equal(
            DriverAction.ReportFailure,
            unresolved.AdvanceDeadline(16),
            "missing cleanup observation reaches fixed deadline failure");
        Equal(
            GesturePhase.FailedCleanupUnresolved,
            unresolved.Phase,
            "missing Down anchor never fabricates release proof");
        Pass();
    }

    private static void OsObserverJoinsGestureDeliveryAndLiveFrames()
    {
        ButtonGestureTuple gesture = new ButtonGestureTuple(
            Request(ButtonWireCommandKind.BeginGesture, 1, 0, new FakeClock(10)));

        FakeObservationPlatform staleCreationPlatform = new FakeObservationPlatform();
        staleCreationPlatform.EnqueueNormal(NormalFrameDetailed(700, 500, 601, 800, 701));
        staleCreationPlatform.EnqueueLevel(ButtonPhysicalLevel.Down);
        staleCreationPlatform.EnqueueNormal(NormalFrameDetailed(700, 500, 601, 800, 701));
        ButtonDownUnconfirmed staleCreation = new ButtonOsObserver(staleCreationPlatform)
            .ObserveTaggedDown(gesture, TaggedDown(gesture)) as ButtonDownUnconfirmed;
        Equal(
            ButtonObservationFailureKind.WrongReceiver,
            staleCreation.Kind,
            "same PID with stale process creation identity is not the gesture owner");

        FakeObservationPlatform taggedTidPlatform = new FakeObservationPlatform();
        taggedTidPlatform.EnqueueNormal(NormalFrameDetailed(700, 500, 600, 800, 701));
        taggedTidPlatform.EnqueueLevel(ButtonPhysicalLevel.Down);
        taggedTidPlatform.EnqueueNormal(NormalFrameDetailed(700, 500, 600, 800, 701));
        TaggedButtonDelivery wrongTaggedTid = new TaggedButtonDelivery(
            gesture.GestureId,
            gesture.DownTag,
            ButtonTaggedEdgeKind.Down,
            gesture.Target.InputHwnd,
            gesture.AppProcessId,
            899,
            10,
            11,
            gesture.Target.ScreenX,
            gesture.Target.ScreenY);
        ButtonDownUnconfirmed taggedTid = new ButtonOsObserver(taggedTidPlatform)
            .ObserveTaggedDown(gesture, wrongTaggedTid) as ButtonDownUnconfirmed;
        Equal(
            ButtonObservationFailureKind.WrongReceiver,
            taggedTid.Kind,
            "tagged receiver TID must match both live receiver samples");

        FakeObservationPlatform foregroundChangedPlatform = new FakeObservationPlatform();
        foregroundChangedPlatform.EnqueueNormal(NormalFrameDetailed(700, 500, 600, 800, 701));
        foregroundChangedPlatform.EnqueueLevel(ButtonPhysicalLevel.Down);
        foregroundChangedPlatform.EnqueueNormal(NormalFrameDetailed(799, 500, 600, 800, 701));
        ButtonDownUnconfirmed foregroundChanged = new ButtonOsObserver(foregroundChangedPlatform)
            .ObserveTaggedDown(gesture, TaggedDown(gesture)) as ButtonDownUnconfirmed;
        Equal(
            ButtonObservationFailureKind.ForegroundChanged,
            foregroundChanged.Kind,
            "access change is distinct from receiver identity change");

        FakeObservationPlatform receiverChangedPlatform = new FakeObservationPlatform();
        receiverChangedPlatform.EnqueueNormal(NormalFrameDetailed(700, 500, 600, 800, 701));
        receiverChangedPlatform.EnqueueLevel(ButtonPhysicalLevel.Down);
        receiverChangedPlatform.EnqueueNormal(NormalFrameDetailed(700, 500, 600, 801, 701));
        ButtonDownUnconfirmed receiverChanged = new ButtonOsObserver(receiverChangedPlatform)
            .ObserveTaggedDown(gesture, TaggedDown(gesture)) as ButtonDownUnconfirmed;
        Equal(
            ButtonObservationFailureKind.WrongReceiver,
            receiverChanged.Kind,
            "receiver identity change is not reported as foreground change");

        FakeObservationPlatform captureChangedPlatform = new FakeObservationPlatform();
        captureChangedPlatform.EnqueueNormal(NormalFrameDetailed(700, 500, 600, 800, 701));
        captureChangedPlatform.EnqueueLevel(ButtonPhysicalLevel.Down);
        captureChangedPlatform.EnqueueNormal(NormalFrameDetailed(700, 500, 600, 800, 0));
        ButtonDownUnconfirmed captureChanged = new ButtonOsObserver(captureChangedPlatform)
            .ObserveTaggedDown(gesture, TaggedDown(gesture)) as ButtonDownUnconfirmed;
        Equal(
            ButtonObservationFailureKind.CaptureMismatch,
            captureChanged.Kind,
            "capture change is distinct from receiver identity change");

        string incoherentDescription = ButtonObservationEnvironmentDescription.Describe(
            new ButtonAccessLevelProbe(
                AccessFrameForForeground(900, 700, 500, 801, 600),
                ButtonPhysicalLevel.Up,
                AccessFrameForForeground(900, 799, 500, 801, 600)));
        Equal(
            "available=false,reason=AccessContextChanged",
            incoherentDescription,
            "read-only diagnostic does not call an incoherent access sample available");
        string changedDesktopDescription = ButtonObservationEnvironmentDescription.Describe(
            new ButtonAccessLevelProbe(
                AccessFrameForForeground(900, 700, 500, 801, 600),
                ButtonPhysicalLevel.Up,
                AccessFrameForForeground(901, 700, 500, 801, 600)));
        Equal(
            "available=false,reason=DesktopChanged",
            changedDesktopDescription,
            "read-only diagnostic distinguishes desktop changes");

        FakeObservationPlatform upPlatform = new FakeObservationPlatform();
        upPlatform.EnqueueNormal(NormalFrameDetailed(700, 500, 600, 800, 701));
        upPlatform.EnqueueLevel(ButtonPhysicalLevel.Down);
        upPlatform.EnqueueNormal(NormalFrameDetailed(700, 500, 600, 800, 701));
        ButtonOsObserver upObserver = new ButtonOsObserver(upPlatform);
        ButtonDownObserved down = upObserver.ObserveTaggedDown(gesture, TaggedDown(gesture))
            as ButtonDownObserved;
        True(down != null, "Up join fixture has a valid Down anchor");
        upPlatform.EnqueueNormal(NormalFrameDetailed(700, 500, 600, 801, 0));
        upPlatform.EnqueueLevel(ButtonPhysicalLevel.Up);
        upPlatform.EnqueueNormal(NormalFrameDetailed(700, 500, 600, 801, 0));
        ButtonReleaseUnconfirmed upReceiver = upObserver.ObserveNormalUp(
            down.Anchor,
            new NormalButtonUpInsertion(gesture.GestureId, gesture.UpTag, 1),
            TaggedUp(gesture)) as ButtonReleaseUnconfirmed;
        Equal(
            ButtonObservationFailureKind.WrongReceiver,
            upReceiver.Kind,
            "tagged Up TID must match both live receiver samples");
        Pass();
    }

    public static string RunReadOnlyButtonObservationProbe()
    {
        return new NativeButtonObservationPlatform().DescribeReadOnlyEnvironment();
    }

    public static string RunReadOnlyButtonProbes()
    {
        return RunReadOnlyButtonObservationProbe()
            + Environment.NewLine
            + NativeButtonInputEnvironmentDescription.Describe();
    }

    public static string RunButtonInputBackendTests()
    {
        return "PASS: " + ButtonInputBackendDraftTests.RunAll()
            + " native button input backend draft tests";
    }

    private static TaggedButtonDelivery TaggedDown(ButtonGestureTuple gesture)
    {
        return new TaggedButtonDelivery(
            gesture.GestureId,
            gesture.DownTag,
            ButtonTaggedEdgeKind.Down,
            gesture.Target.InputHwnd,
            gesture.AppProcessId,
            800,
            10,
            11,
            gesture.Target.ScreenX,
            gesture.Target.ScreenY);
    }

    private static TaggedButtonDelivery TaggedUp(ButtonGestureTuple gesture)
    {
        return new TaggedButtonDelivery(
            gesture.GestureId,
            gesture.UpTag,
            ButtonTaggedEdgeKind.Up,
            gesture.Target.InputHwnd,
            gesture.AppProcessId,
            800,
            10,
            11,
            gesture.Target.ScreenX,
            gesture.Target.ScreenY);
    }

    private static ButtonAccessFrame AccessFrame(ulong threadDesktop)
    {
        return AccessFrameForForeground(threadDesktop, 700, 500, 801, 600);
    }

    private static ButtonAccessFrame SwappedAccessFrame(ulong threadDesktop)
    {
        ButtonAccessFrame baseFrame = AccessFrame(threadDesktop);
        return new ButtonAccessFrame(
            baseFrame.ThreadDesktopIdentity,
            baseFrame.Foreground,
            true);
    }

    private static ButtonAccessFrame AccessFrameForForeground(
        ulong threadDesktop,
        ulong hwnd,
        uint pid,
        uint tid,
        ulong creation)
    {
        return new ButtonAccessFrame(
            threadDesktop,
            new ButtonForegroundIdentity(hwnd, pid, tid, creation, 1, "S-1-5-21-test", 0x2000),
            false);
    }

    private static ButtonNormalFrame NormalFrame(ulong foregroundHwnd, ulong captureHwnd)
    {
        return NormalFrameDetailed(foregroundHwnd, 500, 600, 800, captureHwnd);
    }

    private static ButtonNormalFrame NormalFrameDetailed(
        ulong foregroundHwnd,
        uint appProcessId,
        ulong appCreationIdentity,
        uint receiverThreadId,
        ulong captureHwnd)
    {
        return new ButtonNormalFrame(
            AccessFrameForForeground(
                900,
                foregroundHwnd,
                appProcessId,
                801,
                appCreationIdentity),
            new ButtonReceiverFrame(701, 700, appProcessId, receiverThreadId, captureHwnd));
    }

    private static ButtonHelperOwner NewOwner(
        FakeClock clock,
        FakeLease lease,
        FakeInput input,
        IButtonReplySink sink)
    {
        return NewOwner(clock, lease, input, sink, new FakeOwnerDeliveryObserver());
    }

    private static ButtonHelperOwner NewOwner(
        FakeClock clock,
        FakeLease lease,
        FakeInput input,
        IButtonReplySink sink,
        IButtonDeliveryObserver deliveryObserver)
    {
        return new ButtonHelperOwner(
            Session,
            lease,
            clock,
            input,
            deliveryObserver,
            sink,
            5);
    }

    private static readonly Guid Session = new Guid("00112233-4455-6677-8899-aabbccddeeff");

    private static ButtonWireRequest Request(
        ButtonWireCommandKind kind,
        uint step,
        uint flags,
        FakeClock clock)
    {
        return RequestForIdentity(kind, step, flags, 500, 600);
    }

    private static ButtonWireRequest RequestForIdentity(
        ButtonWireCommandKind kind,
        uint step,
        uint flags,
        uint processId,
        ulong creationIdentity)
    {
        ButtonWireRequest request = new ButtonWireRequest();
        request.Kind = kind;
        request.SessionNonce = Session;
        request.GestureId = 41;
        request.Step = step;
        request.Flags = flags;
        request.DeadlineTick = 100;
        request.AppProcessId = processId;
        request.AppCreationIdentity = creationIdentity;
        request.Target = new ButtonWireTarget();
        request.Target.ParentHwnd = 700;
        request.Target.InputHwnd = 701;
        request.Target.ScreenX = 702;
        request.Target.ScreenY = 703;
        request.Target.SourceEpoch = 0;
        request.Target.PlacementGeneration = 704;
        request.Target.HostIncarnation = 705;
        request.Target.BackendToken = 706;
        request.DownTag = 707;
        request.UpTag = 708;
        request.Delivery = new ButtonWireDelivery();
        if (kind == ButtonWireCommandKind.DownDispatchReceipt
            || kind == ButtonWireCommandKind.UpSemanticReceipt)
        {
            request.Flags |= 2U;
            request.Delivery.ObservedTag = kind == ButtonWireCommandKind.DownDispatchReceipt
                ? request.DownTag
                : request.UpTag;
            request.Delivery.ReceiverHwnd = request.Target.InputHwnd;
            request.Delivery.ReceiverProcessId = request.AppProcessId;
            request.Delivery.ReceiverThreadId = 800;
            request.Delivery.ActualClientX = 10;
            request.Delivery.ActualClientY = 11;
            request.Delivery.ActualScreenX = request.Target.ScreenX;
            request.Delivery.ActualScreenY = request.Target.ScreenY;
        }
        return request;
    }

    private sealed class FakeObservationPlatform : IButtonObservationPlatform
    {
        private readonly Queue<object> normal = new Queue<object>();
        private readonly Queue<object> access = new Queue<object>();
        private readonly Queue<ButtonPhysicalLevel> levels = new Queue<ButtonPhysicalLevel>();

        internal void EnqueueNormal(ButtonNormalFrame frame) { normal.Enqueue(frame); }
        internal void EnqueueNormalFailure(ButtonObservationFailureKind kind)
        {
            normal.Enqueue(new ButtonObservationUnavailableException(kind, "fake normal failure"));
        }
        internal void EnqueueAccess(ButtonAccessFrame frame) { access.Enqueue(frame); }
        internal void EnqueueAccessFailure(ButtonObservationFailureKind kind)
        {
            access.Enqueue(new ButtonObservationUnavailableException(kind, "fake access failure"));
        }
        internal void EnqueueLevel(ButtonPhysicalLevel level) { levels.Enqueue(level); }

        public ButtonNormalLevelProbe ProbeNormalLevel(
            ButtonGestureTuple gesture,
            ButtonCaptureExpectation captureExpectation)
        {
            ButtonNormalFrame before = TakeNormal();
            ButtonPhysicalLevel level = TakeLevel();
            ButtonNormalFrame after = TakeNormal();
            return new ButtonNormalLevelProbe(
                before,
                level,
                after,
                new ButtonPointTolerance(1, 1));
        }

        public ButtonAccessLevelProbe ProbeAccessLevel()
        {
            ButtonAccessFrame before = TakeAccess();
            ButtonPhysicalLevel level = TakeLevel();
            ButtonAccessFrame after = TakeAccess();
            return new ButtonAccessLevelProbe(before, level, after);
        }

        private ButtonNormalFrame TakeNormal()
        {
            if (normal.Count == 0) throw new Exception("fake normal frame queue is empty");
            object next = normal.Dequeue();
            ButtonObservationUnavailableException failure = next
                as ButtonObservationUnavailableException;
            if (failure != null) throw failure;
            return (ButtonNormalFrame)next;
        }

        private ButtonAccessFrame TakeAccess()
        {
            if (access.Count == 0) throw new Exception("fake access frame queue is empty");
            object next = access.Dequeue();
            ButtonObservationUnavailableException failure = next
                as ButtonObservationUnavailableException;
            if (failure != null) throw failure;
            return (ButtonAccessFrame)next;
        }

        private ButtonPhysicalLevel TakeLevel()
        {
            if (levels.Count == 0) throw new Exception("fake physical-level queue is empty");
            return levels.Dequeue();
        }
    }

    private sealed class FakeOwnerDeliveryObserver : IButtonDeliveryObserver
    {
        internal ObservedButtonDownAnchor LastAnchor;
        internal Action BeforeDownReturn;
        internal Action BeforeNormalUpReturn;
        private readonly bool observeNormalRelease;
        private readonly bool observeCleanupRelease;

        internal FakeOwnerDeliveryObserver()
            : this(false, false)
        {
        }

        internal FakeOwnerDeliveryObserver(bool observeNormalRelease, bool observeCleanupRelease)
        {
            this.observeNormalRelease = observeNormalRelease;
            this.observeCleanupRelease = observeCleanupRelease;
        }

        public ButtonDownObservationResult ObserveTaggedDown(
            ButtonGestureTuple gesture,
            TaggedButtonDelivery delivery)
        {
            LastAnchor = new ObservedButtonDownAnchor(
                gesture,
                delivery,
                NormalFrame(gesture.Target.ParentHwnd, gesture.Target.InputHwnd));
            if (BeforeDownReturn != null) BeforeDownReturn();
            return new ButtonDownObserved(LastAnchor);
        }

        public ButtonReleaseObservationResult ObserveNormalUp(
            ObservedButtonDownAnchor anchor,
            NormalButtonUpInsertion insertion,
            TaggedButtonDelivery delivery)
        {
            if (!observeNormalRelease)
            {
                return new ButtonReleaseUnconfirmed(
                    ButtonObservationFailureKind.PhysicalLevelMismatch,
                    "fake normal release is pending");
            }
            if (BeforeNormalUpReturn != null) BeforeNormalUpReturn();
            return new ButtonReleaseObserved(
                new NormalButtonReleaseEvidence(
                    anchor,
                    delivery,
                    NormalFrame(anchor.Gesture.Target.ParentHwnd, 0)));
        }

        public ButtonReleaseObservationResult ObserveOwnedCleanupUp(
            ObservedButtonDownAnchor anchor,
            OwnedCleanupButtonUpInsertion insertion)
        {
            if (!observeCleanupRelease)
            {
                return new ButtonReleaseUnconfirmed(
                    ButtonObservationFailureKind.PhysicalLevelMismatch,
                    "fake cleanup release is pending");
            }
            return new ButtonReleaseObserved(
                new OwnedCleanupButtonReleaseEvidence(anchor, AccessFrame(900)));
        }

        internal NormalButtonReleaseEvidence NormalEvidence()
        {
            if (LastAnchor == null) throw new Exception("fake Down anchor is missing");
            return new NormalButtonReleaseEvidence(
                LastAnchor,
                TaggedUp(LastAnchor.Gesture),
                NormalFrame(LastAnchor.Gesture.Target.ParentHwnd, 0));
        }

        internal OwnedCleanupButtonReleaseEvidence CleanupEvidence()
        {
            if (LastAnchor == null) throw new Exception("fake Down anchor is missing");
            return new OwnedCleanupButtonReleaseEvidence(LastAnchor, AccessFrame(900));
        }
    }

    private sealed class FakeClock : IButtonClock
    {
        internal FakeClock(ulong now) { Now = now; }
        internal ulong Now;
        public ulong NowTick { get { return Now; } }
    }

    private sealed class FakeLease : IAppProcessLease
    {
        private readonly uint processId;
        private readonly ulong creationIdentity;
        internal FakeLease() : this(500, 600) { }
        internal FakeLease(uint processId, ulong creationIdentity)
        {
            this.processId = processId;
            this.creationIdentity = creationIdentity;
        }
        internal bool Alive = true;
        internal int AliveChecks;
        public uint ProcessId { get { return processId; } }
        public ulong CreationIdentity { get { return creationIdentity; } }
        public bool IsAlive
        {
            get { AliveChecks++; return Alive; }
        }
    }

    private sealed class FakeInput : IButtonInputBackend
    {
        private readonly List<string> order;
        internal FakeInput() : this(new List<string>()) { }
        internal FakeInput(List<string> order) { this.order = order; }
        internal int DownCalls;
        internal int UpCalls;
        internal int CleanupUpCalls;
        internal Action Down;
        internal Action NormalUp;
        internal Action CleanupUp;
        internal int LastCleanupScreenX;
        internal ulong LastCleanupBackendToken;
        internal ulong LastCleanupTag;
        public ButtonSendAttempt SendNormal(
            ButtonNormalSendRequest request,
            Func<ButtonSendPermission> permission)
        {
            ButtonSendPermission allowed = permission();
            if (allowed != ButtonSendPermission.Allowed)
                return new ButtonSendRefused("fake owner permission: " + allowed);
            if (request.Edge == ButtonNormalSendEdge.Down)
            {
                DownCalls++;
                order.Add("down");
                if (Down != null) Down();
            }
            else
            {
                UpCalls++;
                order.Add("up");
                if (NormalUp != null) NormalUp();
            }
            return new ButtonSendCalled(1, 0, null);
        }

        public ButtonSendAttempt SendOwnedCleanupUp(ButtonCleanupSendRequest request)
        {
            UpCalls++;
            CleanupUpCalls++;
            LastCleanupScreenX = request.Gesture.Target.ScreenX;
            LastCleanupBackendToken = request.Gesture.Target.BackendToken;
            LastCleanupTag = request.Tag;
            order.Add("cleanup-up");
            if (CleanupUp != null) CleanupUp();
            return new ButtonSendCalled(1, 0, null);
        }
    }

    private sealed class LifecycleFloodOsSource : IButtonReleaseEvidenceSource
    {
        private readonly FakeInput input;
        private readonly ButtonReleaseEvidence wrongEvidence;
        private readonly ButtonReleaseEvidence cleanupEvidence;
        private int wrongRemaining;
        private bool released;

        internal LifecycleFloodOsSource(
            FakeInput input,
            int wrongCount,
            ButtonReleaseEvidence wrongEvidence,
            ButtonReleaseEvidence cleanupEvidence)
        {
            this.input = input;
            this.wrongEvidence = wrongEvidence;
            this.cleanupEvidence = cleanupEvidence;
            wrongRemaining = wrongCount;
            InitialWrongCount = wrongCount;
        }

        internal int InitialWrongCount { get; private set; }
        internal int WrongTaken { get; private set; }

        public bool TryTake(out ButtonReleaseEvidence evidence)
        {
            evidence = null;
            if (input.CleanupUpCalls > 0 && !released && cleanupEvidence != null)
            {
                released = true;
                evidence = cleanupEvidence;
                return true;
            }
            if (wrongRemaining <= 0) return false;
            wrongRemaining--;
            WrongTaken++;
            evidence = wrongEvidence;
            return true;
        }
    }

    private sealed class FakeOsSource : IButtonReleaseEvidenceSource
    {
        private readonly System.Collections.Concurrent.ConcurrentQueue<ButtonReleaseEvidence> queue =
            new System.Collections.Concurrent.ConcurrentQueue<ButtonReleaseEvidence>();
        internal void Enqueue(ButtonReleaseEvidence evidence)
        {
            if (evidence == null) throw new ArgumentNullException("evidence");
            queue.Enqueue(evidence);
        }
        public bool TryTake(out ButtonReleaseEvidence evidence)
        {
            return queue.TryDequeue(out evidence);
        }
    }

    private sealed class CollectingSink : IButtonReplySink
    {
        internal readonly List<ButtonWireReply> Replies = new List<ButtonWireReply>();
        public bool TryPublish(ButtonWireReply reply) { Replies.Add(reply); return true; }
    }

    private sealed class RejectingSink : IButtonReplySink
    {
        public bool TryPublish(ButtonWireReply reply) { return false; }
    }

    private sealed class InspectingSink : IButtonReplySink
    {
        private readonly FakeInput input;
        internal InspectingSink(FakeInput input) { this.input = input; }
        internal ButtonHelperOwner Owner;
        internal bool SawRecordedObligation;
        public bool TryPublish(ButtonWireReply reply)
        {
            if (input.DownCalls > 0)
                SawRecordedObligation = Owner.HasOutstandingRelease;
            return true;
        }
    }

    private sealed class BlockingTransport : IButtonReplyTransport
    {
        internal readonly ManualResetEvent Entered = new ManualResetEvent(false);
        internal readonly ManualResetEvent Release = new ManualResetEvent(false);
        public void Write(byte[] frame, CancellationToken cancellationToken)
        {
            Entered.Set();
            WaitHandle.WaitAny(new WaitHandle[] { Release, cancellationToken.WaitHandle });
            cancellationToken.ThrowIfCancellationRequested();
        }
        public void Dispose() { Release.Set(); }
    }

    private static void ThrowsProtocol(Action action, string label)
    {
        try
        {
            action();
        }
        catch (ButtonWireProtocolException)
        {
            return;
        }
        throw new Exception(label + ": expected ButtonWireProtocolException");
    }

    private static void True(bool value, string label)
    {
        if (!value) throw new Exception(label + ": expected true");
    }

    private static void Equal<T>(T expected, T actual, string label)
    {
        if (!EqualityComparer<T>.Default.Equals(expected, actual))
            throw new Exception(label + ": expected " + expected + ", actual " + actual);
    }

    private static void Pass() { passed++; }
}
