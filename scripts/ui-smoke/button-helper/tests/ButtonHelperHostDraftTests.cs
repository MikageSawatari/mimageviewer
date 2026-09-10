using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Threading;
using System.Threading.Tasks;
using Miv.UiSmoke.ButtonDraft;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    public static class ButtonHelperHostDraftTests
    {
        private static int passed;

        public static int RunAll()
        {
            Console.WriteLine("RUN HostCompletesOneAuthenticatedFakeGesture");
            HostCompletesOneAuthenticatedFakeGesture();
            Console.WriteLine("RUN HostCancelsAcceptWithoutCreatingInputOwner");
            HostCancelsAcceptWithoutCreatingInputOwner();
            Console.WriteLine("RUN HostDetectsPinnedAppExitBeforeConnection");
            HostDetectsPinnedAppExitBeforeConnection();
            Console.WriteLine("RUN HostCancelsAuthenticatedConnectionBeforeGesture");
            HostCancelsAuthenticatedConnectionBeforeGesture();
            Console.WriteLine("RUN HostRetainsBlockedOwnerAndFixedJoinContract");
            HostRetainsBlockedOwnerAndFixedJoinContract();
            Console.WriteLine("RUN ReaderJoinFailureRetainsSynchronizationObjects");
            ReaderJoinFailureRetainsSynchronizationObjects();
            Console.WriteLine("RUN WriterJoinFailureRetainsTransportAndQueue");
            WriterJoinFailureRetainsTransportAndQueue();
            Console.WriteLine("RUN PipeCloseAloneIsNotAcceptJoinProof");
            PipeCloseAloneIsNotAcceptJoinProof();
            Console.WriteLine("RUN ReplyTransportFaultCannotLeaveSuccessfulTerminal");
            ReplyTransportFaultCannotLeaveSuccessfulTerminal();
            Console.WriteLine("RUN HostReclassifiesTerminalReplyTransportFailure");
            HostReclassifiesTerminalReplyTransportFailure();
            Console.WriteLine("RUN EofAfterDownRunsOwnedCleanupBeforeCollection");
            EofAfterDownRunsOwnedCleanupBeforeCollection();
            Console.WriteLine("RUN ReaderFaultAfterDownRunsOwnedCleanupBeforeCollection");
            ReaderFaultAfterDownRunsOwnedCleanupBeforeCollection();
            Console.WriteLine("RUN SecondBeginAfterDownRunsOwnedCleanupBeforeCollection");
            SecondBeginAfterDownRunsOwnedCleanupBeforeCollection();
            Console.WriteLine("RUN AppDeathAfterDownRunsOwnedCleanupBeforeCollection");
            AppDeathAfterDownRunsOwnedCleanupBeforeCollection();
            Console.WriteLine("RUN UnresolvedReleaseForbidsRunnerKill");
            UnresolvedReleaseForbidsRunnerKill();
            Console.WriteLine("RUN AppFailurePrecedesHelperSuccess");
            AppFailurePrecedesHelperSuccess();
            Console.WriteLine("RUN StillRunningReleaseStateIsUnknownAndUnsafe");
            StillRunningReleaseStateIsUnknownAndUnsafe();
            return passed;
        }

        private static void HostCompletesOneAuthenticatedFakeGesture()
        {
            HostFixture fixture = new HostFixture();
            ButtonHelperHost host = fixture.Start();
            ButtonWireReply terminal;
            using (NamedPipeClientStream client = Connect(fixture))
            {
                terminal = RunSuccessfulGesture(client, fixture);
            }
            Equal(ButtonWireReplyKind.TerminalSuccess, terminal.Kind, "wire terminal");

            ButtonHelperHostJoinedTerminal joined = host.CancelAndJoin(
                "collect completed fake gesture",
                fixture.Clock.NowTick + 3000) as ButtonHelperHostJoinedTerminal;
            NotNull(joined, "completed host joins");
            Equal(ButtonHelperHostTerminalKind.Succeeded, joined.Terminal.Kind, "host terminal");
            True(joined.Terminal.GestureBegan, "gesture began");
            True(joined.Terminal.OwnerTerminal, "owner terminal");
            True(joined.Terminal.SafeToCollect, "terminal is safe to collect");
            Equal(1, fixture.Input.DownCalls, "one fake Down");
            Equal(1, fixture.Input.NormalUpCalls, "one fake normal Up");
            Equal(0, fixture.Input.CleanupUpCalls, "no cleanup Up on success");
            Equal(1, fixture.Lease.DisposeCalls, "pinned lease collected once");
            Pass();
        }

        private static void HostCancelsAcceptWithoutCreatingInputOwner()
        {
            HostFixture fixture = new HostFixture();
            ButtonHelperHost host = fixture.Start();
            True(fixture.Factory.LeaseOpened.WaitOne(3000), "app lease opened");
            ButtonHelperHostJoinedTerminal joined = host.CancelAndJoin(
                "cancel before client connection",
                fixture.Clock.NowTick + 3000) as ButtonHelperHostJoinedTerminal;
            NotNull(joined, "cancelled accept joins");
            Equal(
                ButtonHelperHostTerminalKind.CancelledBeforeConnection,
                joined.Terminal.Kind,
                "cancel before connection terminal");
            True(!joined.Terminal.GestureBegan, "no gesture before connection");
            Equal(0, fixture.Factory.CreateComponentsCalls, "no input owner constructed");
            Equal(0, fixture.Input.TotalCalls, "no fake input");
            Pass();
        }

        private static void HostDetectsPinnedAppExitBeforeConnection()
        {
            HostFixture fixture = new HostFixture();
            fixture.Lease.Alive = false;
            fixture.Lease.BlockDeadRead = true;
            ButtonHelperHost host = fixture.Start();
            True(fixture.Lease.DeadReadEntered.WaitOne(3000), "owner sampled dead pinned lease");
            fixture.Lease.ReleaseDeadRead.Set();
            ButtonHelperHostJoinedTerminal joined = host.CancelAndJoin(
                "collect app-exit terminal",
                fixture.Clock.NowTick + 3000) as ButtonHelperHostJoinedTerminal;
            NotNull(joined, "app-exit host joins");
            Equal(
                ButtonHelperHostTerminalKind.AppExitedBeforeConnection,
                joined.Terminal.Kind,
                "app exit before connection terminal");
            Equal(0, fixture.Factory.CreateComponentsCalls, "dead app creates no input owner");
            Equal(0, fixture.Input.TotalCalls, "dead app sends no input");
            Pass();
        }

        private static void HostCancelsAuthenticatedConnectionBeforeGesture()
        {
            HostFixture fixture = new HostFixture();
            ButtonHelperHost host = fixture.Start();
            using (NamedPipeClientStream client = Connect(fixture))
            {
                True(
                    fixture.Factory.ComponentsCreated.WaitOne(3000),
                    "authenticated owner components created");
                ButtonHelperHostJoinedTerminal joined = host.CancelAndJoin(
                    "cancel authenticated idle client",
                    fixture.Clock.NowTick + 3000) as ButtonHelperHostJoinedTerminal;
                NotNull(joined, "pre-gesture cancellation joins");
                Equal(
                    ButtonHelperHostTerminalKind.CancelledBeforeGesture,
                    joined.Terminal.Kind,
                    "authenticated cancellation terminal");
                True(!joined.Terminal.GestureBegan, "authenticated client sent no gesture");
                Equal(0, fixture.Input.TotalCalls, "pre-gesture cancellation sends no input");
            }
            Pass();
        }

        private static void HostRetainsBlockedOwnerAndFixedJoinContract()
        {
            HostFixture fixture = new HostFixture();
            fixture.Input.BlockDown = true;
            ButtonHelperHost host = fixture.Start();
            NamedPipeClientStream client = Connect(fixture);
            try
            {
                ButtonWireCodec.WriteFrame(
                    client,
                    ButtonWireCodec.EncodeRequest(fixture.Request(ButtonWireCommandKind.BeginGesture, 1, 0)));
                True(fixture.Input.DownEntered.WaitOne(3000), "fake Down entered owner thread");

                ulong firstDeadline = fixture.Clock.NowTick + 30;
                ButtonHelperHostStillRunning first = host.CancelAndJoin(
                    "first runner cancellation",
                    firstDeadline) as ButtonHelperHostStillRunning;
                NotNull(first, "blocked owner is retained");
                Equal(firstDeadline, first.FixedJoinDeadlineTick, "first join deadline fixed");
                Equal("first runner cancellation", first.FirstCancellationReason, "first reason fixed");
                Equal(0, fixture.Lease.DisposeCalls, "retained host keeps process lease");

                ButtonHelperHostStillRunning second = first.RetainedHost.CancelAndJoin(
                    "later reason must not replace first",
                    fixture.Clock.NowTick + 3000) as ButtonHelperHostStillRunning;
                NotNull(second, "expired fixed join cannot be extended");
                Equal(firstDeadline, second.FixedJoinDeadlineTick, "later call cannot extend deadline");
                Equal("first runner cancellation", second.FirstCancellationReason, "later reason ignored");

                fixture.Input.ReleaseDown.Set();
                True(fixture.Input.CleanupUpObserved.WaitOne(3000), "owner performed typed cleanup Up");
                ButtonHelperHostJoinedTerminal joined = WaitForCollection(second.RetainedHost, fixture);
                Equal(ButtonHelperHostTerminalKind.GestureFailed, joined.Terminal.Kind, "cancelled gesture fails");
                True(joined.Terminal.OwnerTerminal, "cleanup reached typed terminal");
                True(joined.Terminal.CleanupAttempts >= 1, "cleanup attempt recorded");
                Equal(1, fixture.Input.DownCalls, "blocked Down inserted once");
                Equal(1, fixture.Input.CleanupUpCalls, "owned cleanup Up inserted once");
                Equal(1, fixture.Lease.DisposeCalls, "lease disposed only after later collection");
            }
            finally
            {
                fixture.Input.ReleaseDown.Set();
                client.Dispose();
            }
            Pass();
        }

        private static void ReaderJoinFailureRetainsSynchronizationObjects()
        {
            DelayedReadStream stream = new DelayedReadStream();
            BoundedPipeRequestReader reader = new BoundedPipeRequestReader(stream, 1);
            True(stream.Entered.WaitOne(3000), "reader entered delayed stream");
            True(!reader.TryJoinAndDispose(0), "reader join reports still running");
            // The activity event and bounded queue remain usable while the reader
            // still owns them; the late Read completion must not Set a disposed event.
            True(!reader.WaitForActivity(0), "reader activity object retained");
            stream.Release.Set();
            True(WaitForReaderJoin(reader, 3000), "reader collects after delayed read returns");
            Equal(1, stream.DisposeCalls, "reader requested stream stop once");
            Pass();
        }

        private static void WriterJoinFailureRetainsTransportAndQueue()
        {
            DelayedReplyTransport transport = new DelayedReplyTransport();
            BoundedAsyncReplySink sink = new BoundedAsyncReplySink(transport, 1);
            True(sink.TryPublish(Reply(ButtonWireReplyKind.Accepted)), "reply queued");
            True(transport.Entered.WaitOne(3000), "writer entered delayed transport");
            True(!sink.TryJoinAndDispose(0), "writer join reports still running");
            Equal(0, transport.DisposeCalls, "join failure retains shared transport");
            transport.Release.Set();
            True(WaitForWriterJoin(sink, 3000), "writer collects after delayed write returns");
            Equal(1, transport.DisposeCalls, "joined writer disposes transport once");
            Pass();
        }

        private static void PipeCloseAloneIsNotAcceptJoinProof()
        {
            string name = "miv-ui-smoke-button-accept-join-" + Guid.NewGuid().ToString("N");
            TaskCompletionSource<bool> pending = new TaskCompletionSource<bool>();
            NamedPipeServerStream stream = new NamedPipeServerStream(
                name,
                PipeDirection.InOut,
                1,
                PipeTransmissionMode.Byte,
                PipeOptions.Asynchronous);
            LocalButtonPipeServer server = LocalButtonPipeServer.CreateJoinTestInstance(
                stream,
                pending.Task);
            True(!server.TryJoinAndDispose(0), "pipe close is not accept-task completion");
            pending.SetResult(true);
            True(server.TryJoinAndDispose(1000), "completed accept task permits collection");
            Pass();
        }

        private static void ReplyTransportFaultCannotLeaveSuccessfulTerminal()
        {
            ButtonHelperHostTerminal ownerSuccess = new ButtonHelperHostTerminal(
                ButtonHelperHostTerminalKind.Succeeded,
                true,
                true,
                GesturePhase.Succeeded,
                null,
                0,
                false,
                "Succeeded");
            ButtonHelperHostTerminal classified = ButtonHelperHost.ApplyReplyTransportFault(
                ownerSuccess,
                "IOException: terminal reply was not written");
            Equal(ButtonHelperHostTerminalKind.GestureFailed, classified.Kind,
                "writer fault overrides protocol success");
            Equal(GesturePhase.Succeeded, classified.OwnerPhase,
                "owner completion remains observable");
            True(!classified.HasOutstandingRelease,
                "transport failure does not invent a release obligation");
            True(classified.Detail.Contains("reply_transport=IOException"),
                "transport failure is retained");

            ButtonHelperHostTerminal unchanged = ButtonHelperHost.ApplyReplyTransportFault(
                ownerSuccess,
                null);
            True(Object.ReferenceEquals(ownerSuccess, unchanged),
                "no transport failure leaves terminal unchanged");
            Pass();
        }

        private static void HostReclassifiesTerminalReplyTransportFailure()
        {
            HostFixture fixture = new HostFixture();
            FailNthReplyTransport injected = null;
            fixture.Factory.ReplyTransportFactory = delegate(Stream stream)
            {
                injected = new FailNthReplyTransport(stream, 4);
                return injected;
            };
            ButtonHelperHost host = fixture.Start();
            using (NamedPipeClientStream client = Connect(fixture))
            {
                ButtonWireCommandKind[] commands = new ButtonWireCommandKind[]
                {
                    ButtonWireCommandKind.BeginGesture,
                    ButtonWireCommandKind.DownDispatchReceipt,
                    ButtonWireCommandKind.ReleaseGesture,
                };
                for (int index = 0; index < commands.Length; index++)
                {
                    uint flags = commands[index] == ButtonWireCommandKind.DownDispatchReceipt
                        ? 3U
                        : 0U;
                    ButtonWireCodec.WriteFrame(
                        client,
                        ButtonWireCodec.EncodeRequest(
                            fixture.Request(commands[index], unchecked((uint)index + 1), flags)));
                    Equal(ButtonWireReplyKind.Accepted,
                        ButtonWireCodec.DecodeReply(ButtonWireCodec.ReadFrame(client)).Kind,
                        "pre-terminal reply " + commands[index]);
                }
                ButtonWireCodec.WriteFrame(
                    client,
                    ButtonWireCodec.EncodeRequest(
                        fixture.Request(ButtonWireCommandKind.UpSemanticReceipt, 4, 3)));
                True(injected != null && injected.Faulted.WaitOne(3000),
                    "terminal reply transport fault was observed");
            }
            ButtonHelperHostJoinedTerminal joined = CollectTerminal(
                host,
                fixture,
                "collect terminal transport fault");
            Equal(GesturePhase.Succeeded, joined.Terminal.OwnerPhase,
                "owner reached success before terminal reply transport failed");
            Equal(ButtonHelperHostTerminalKind.GestureFailed, joined.Terminal.Kind,
                "CancelAndJoin reclassifies the actual writer fault");
            True(joined.Terminal.Detail.Contains("reply_transport=IOException"),
                "joined terminal retains the actual writer fault");
            True(!joined.Terminal.HasOutstandingRelease,
                "transport failure does not invent a release obligation");
            Pass();
        }

        private static void EofAfterDownRunsOwnedCleanupBeforeCollection()
        {
            HostFixture fixture = new HostFixture();
            ButtonHelperHost host = fixture.Start();
            NamedPipeClientStream client = Connect(fixture);
            SendBeginAndDownReceipt(client, fixture);
            client.Dispose();
            True(fixture.Input.CleanupUpObserved.WaitOne(3000),
                "EOF after inserted Down reaches owned cleanup");
            ButtonHelperHostJoinedTerminal joined = CollectTerminal(host, fixture, "collect EOF failure");
            Equal(ButtonHelperHostTerminalKind.GestureFailed, joined.Terminal.Kind,
                "EOF gesture is not protocol success");
            Equal(1, fixture.Input.DownCalls, "EOF path inserts one Down");
            Equal(1, fixture.Input.CleanupUpCalls, "EOF path inserts one owned cleanup Up");
            True(!joined.Terminal.HasOutstandingRelease,
                "confirmed EOF cleanup releases the App-kill interlock");
            Pass();
        }

        private static void ReaderFaultAfterDownRunsOwnedCleanupBeforeCollection()
        {
            HostFixture fixture = new HostFixture();
            ButtonHelperHost host = fixture.Start();
            using (NamedPipeClientStream client = Connect(fixture))
            {
                SendBeginAndDownReceipt(client, fixture);
                // A length over the fixed 256-byte frame cap faults the reader.
                client.Write(new byte[] { 1, 1, 0, 0 }, 0, 4);
                client.Flush();
                True(fixture.Input.CleanupUpObserved.WaitOne(3000),
                    "reader fault after inserted Down reaches owned cleanup");
            }
            ButtonHelperHostJoinedTerminal joined = CollectTerminal(
                host,
                fixture,
                "collect reader fault");
            Equal(ButtonHelperHostTerminalKind.GestureFailed, joined.Terminal.Kind,
                "reader fault cannot succeed");
            Equal(1, fixture.Input.CleanupUpCalls, "reader fault inserts cleanup Up");
            Pass();
        }

        private static void SecondBeginAfterDownRunsOwnedCleanupBeforeCollection()
        {
            HostFixture fixture = new HostFixture();
            ButtonHelperHost host = fixture.Start();
            using (NamedPipeClientStream client = Connect(fixture))
            {
                SendBeginAndDownReceipt(client, fixture);
                ButtonWireCodec.WriteFrame(
                    client,
                    ButtonWireCodec.EncodeRequest(
                        fixture.Request(ButtonWireCommandKind.BeginGesture, 3, 0)));
                True(fixture.Input.CleanupUpObserved.WaitOne(3000),
                    "second Begin after Down reaches owned cleanup");
            }
            ButtonHelperHostJoinedTerminal joined = CollectTerminal(
                host,
                fixture,
                "collect second Begin fault");
            Equal(ButtonHelperHostTerminalKind.GestureFailed, joined.Terminal.Kind,
                "second Begin cannot succeed");
            Equal(1, fixture.Input.DownCalls, "second Begin cannot resend Down");
            Equal(1, fixture.Input.CleanupUpCalls, "second Begin inserts cleanup Up");
            Pass();
        }

        private static void AppDeathAfterDownRunsOwnedCleanupBeforeCollection()
        {
            HostFixture fixture = new HostFixture();
            ButtonHelperHost host = fixture.Start();
            using (NamedPipeClientStream client = Connect(fixture))
            {
                SendBeginAndDownReceipt(client, fixture);
                fixture.Lease.Alive = false;
                True(fixture.Input.CleanupUpObserved.WaitOne(3000),
                    "pinned App death after Down reaches owned cleanup");
            }
            ButtonHelperHostJoinedTerminal joined = CollectTerminal(
                host,
                fixture,
                "collect App death");
            Equal(ButtonHelperHostTerminalKind.GestureFailed, joined.Terminal.Kind,
                "App death cannot succeed");
            Equal(1, fixture.Input.CleanupUpCalls, "App death inserts cleanup Up");
            True(!joined.Terminal.HasOutstandingRelease,
                "confirmed death cleanup permits later exact process collection");
            Pass();
        }

        private static void UnresolvedReleaseForbidsRunnerKill()
        {
            HostFixture fixture = new HostFixture();
            fixture.Observer.ConfirmCleanup = false;
            ButtonHelperHost host = fixture.Start(40);
            using (NamedPipeClientStream client = Connect(fixture))
            {
                SendBeginAndDownReceipt(client, fixture);
                client.Dispose();
                True(fixture.Input.CleanupUpObserved.WaitOne(3000),
                    "unconfirmed cleanup path inserted an owned Up");
            }
            Thread.Sleep(100);
            ButtonHelperHostJoinedTerminal joined = CollectTerminal(
                host,
                fixture,
                "collect unresolved release");
            Equal(GesturePhase.FailedCleanupUnresolved, joined.Terminal.OwnerPhase,
                "unconfirmed release reaches its typed terminal");
            True(joined.Terminal.HasOutstandingRelease,
                "unconfirmed release keeps the App-kill interlock closed");
            ButtonHelperRunnerStatus status = new ButtonHelperRunnerStatus
            {
                Joined = true,
                SafeToTerminateApp = false,
                Succeeded = false,
                HasOutstandingRelease = true,
                ReleaseState = ButtonHelperReleaseState.ConfirmedOutstanding,
            };
            True(!ButtonHelperRunnerApi.ScenarioSucceeded(false, 0, status),
                "unresolved release cannot be promoted to runner success");
            Pass();
        }

        private static void AppFailurePrecedesHelperSuccess()
        {
            ButtonHelperRunnerStatus helper = new ButtonHelperRunnerStatus
            {
                Joined = true,
                SafeToTerminateApp = true,
                Succeeded = true,
                HasOutstandingRelease = false,
                ReleaseState = ButtonHelperReleaseState.ConfirmedReleased,
            };
            True(ButtonHelperRunnerApi.ScenarioSucceeded(false, 0, helper),
                "clean App and helper terminals can succeed");
            True(!ButtonHelperRunnerApi.ScenarioSucceeded(true, 0, helper),
                "App timeout remains primary over helper success");
            True(!ButtonHelperRunnerApi.ScenarioSucceeded(false, 2, helper),
                "nonzero App exit remains primary over helper success");
            Pass();
        }

        private static void StillRunningReleaseStateIsUnknownAndUnsafe()
        {
            ButtonHelperRunnerStatus status =
                ButtonHelperRunnerHandle.StillRunningStatus("fixed runner timeout");
            Equal(ButtonHelperReleaseState.Unknown, status.ReleaseState,
                "unjoined host exposes unknown release state");
            True(!status.HasOutstandingRelease,
                "unknown host state does not invent a confirmed obligation");
            True(!status.SafeToTerminateApp,
                "unknown host state keeps the App-kill interlock closed");
            True(!ButtonHelperRunnerApi.ScenarioSucceeded(false, 0, status),
                "unknown release state cannot pass a scenario");
            Pass();
        }

        private static void SendBeginAndReadAccepted(
            NamedPipeClientStream client,
            HostFixture fixture)
        {
            ButtonWireCodec.WriteFrame(
                client,
                ButtonWireCodec.EncodeRequest(
                    fixture.Request(ButtonWireCommandKind.BeginGesture, 1, 0)));
            ButtonWireReply accepted = ButtonWireCodec.DecodeReply(ButtonWireCodec.ReadFrame(client));
            Equal(ButtonWireReplyKind.Accepted, accepted.Kind, "Begin reply");
        }

        private static void SendBeginAndDownReceipt(
            NamedPipeClientStream client,
            HostFixture fixture)
        {
            SendBeginAndReadAccepted(client, fixture);
            ButtonWireCodec.WriteFrame(
                client,
                ButtonWireCodec.EncodeRequest(
                    fixture.Request(ButtonWireCommandKind.DownDispatchReceipt, 2, 3)));
            ButtonWireReply accepted = ButtonWireCodec.DecodeReply(ButtonWireCodec.ReadFrame(client));
            Equal(ButtonWireReplyKind.Accepted, accepted.Kind, "Down receipt reply");
        }

        private static ButtonHelperHostJoinedTerminal CollectTerminal(
            ButtonHelperHost host,
            HostFixture fixture,
            string reason)
        {
            ButtonHelperHostJoinResult result = host.CancelAndJoin(
                reason,
                fixture.Clock.NowTick + 3000);
            ButtonHelperHostJoinedTerminal joined = result as ButtonHelperHostJoinedTerminal;
            NotNull(joined, reason + " joins");
            return joined;
        }

        private static ButtonHelperHostJoinedTerminal WaitForCollection(
            ButtonHelperHost host,
            HostFixture fixture)
        {
            Stopwatch wait = Stopwatch.StartNew();
            while (wait.ElapsedMilliseconds < 3000)
            {
                ButtonHelperHostJoinResult result = host.CancelAndJoin(
                    "later reason must not replace first",
                    fixture.Clock.NowTick + 3000);
                ButtonHelperHostJoinedTerminal joined = result as ButtonHelperHostJoinedTerminal;
                if (joined != null) return joined;
                Thread.Sleep(5);
            }
            throw new Exception("retained host did not become collectable");
        }

        private static bool WaitForReaderJoin(BoundedPipeRequestReader reader, int timeoutMilliseconds)
        {
            Stopwatch wait = Stopwatch.StartNew();
            while (wait.ElapsedMilliseconds < timeoutMilliseconds)
            {
                if (reader.TryJoinAndDispose(0)) return true;
                Thread.Sleep(5);
            }
            return false;
        }

        private static bool WaitForWriterJoin(BoundedAsyncReplySink sink, int timeoutMilliseconds)
        {
            Stopwatch wait = Stopwatch.StartNew();
            while (wait.ElapsedMilliseconds < timeoutMilliseconds)
            {
                if (sink.TryJoinAndDispose(0)) return true;
                Thread.Sleep(5);
            }
            return false;
        }

        private static NamedPipeClientStream Connect(HostFixture fixture)
        {
            NamedPipeClientStream client = new NamedPipeClientStream(
                ".",
                fixture.PipeName,
                PipeDirection.InOut,
                PipeOptions.None);
            try
            {
                client.Connect(3000);
                LocalButtonPipeClientProof.VerifyServerProcess(client, fixture.ProcessId);
                return client;
            }
            catch
            {
                client.Dispose();
                throw;
            }
        }

        private static ButtonWireReply RunSuccessfulGesture(
            NamedPipeClientStream client,
            HostFixture fixture)
        {
            ButtonWireCommandKind[] commands = new ButtonWireCommandKind[]
            {
                ButtonWireCommandKind.BeginGesture,
                ButtonWireCommandKind.DownDispatchReceipt,
                ButtonWireCommandKind.ReleaseGesture,
                ButtonWireCommandKind.UpSemanticReceipt,
            };
            for (int index = 0; index < commands.Length; index++)
            {
                uint flags = commands[index] == ButtonWireCommandKind.DownDispatchReceipt
                    || commands[index] == ButtonWireCommandKind.UpSemanticReceipt
                    ? 3U
                    : 0U;
                ButtonWireCodec.WriteFrame(
                    client,
                    ButtonWireCodec.EncodeRequest(
                        fixture.Request(commands[index], unchecked((uint)index + 1), flags)));
                ButtonWireReply reply = ButtonWireCodec.DecodeReply(ButtonWireCodec.ReadFrame(client));
                if (index + 1 == commands.Length)
                {
                    return reply;
                }
                Equal(ButtonWireReplyKind.Accepted, reply.Kind, "intermediate reply");
            }
            throw new Exception("gesture produced no terminal reply");
        }

        private static ButtonWireReply Reply(ButtonWireReplyKind kind)
        {
            ButtonWireReply reply = new ButtonWireReply();
            reply.Kind = kind;
            reply.SessionNonce = HostFixture.Session;
            reply.GestureId = 41;
            reply.Step = 1;
            reply.Phase = 0;
            reply.Detail = 0;
            return reply;
        }

        private sealed class HostFixture
        {
            internal static readonly Guid Session =
                new Guid("00112233-4455-6677-8899-aabbccddeeff");
            internal readonly uint ProcessId = unchecked((uint)Process.GetCurrentProcess().Id);
            internal readonly ulong CreationIdentity = 600;
            internal readonly string PipeName =
                "miv-ui-smoke-button-host-" + Guid.NewGuid().ToString("N");
            internal readonly StopwatchClock Clock = new StopwatchClock();
            internal readonly ulong GestureDeadline;
            internal readonly HostLease Lease;
            internal readonly HostInput Input = new HostInput();
            internal readonly HostObserver Observer = new HostObserver();
            internal readonly EmptyEvidenceSource Evidence = new EmptyEvidenceSource();
            internal readonly HostFactory Factory;

            internal HostFixture()
            {
                GestureDeadline = Clock.NowTick + 10000;
                Lease = new HostLease(ProcessId, CreationIdentity);
                Factory = new HostFactory(Lease, Input, Observer, Evidence);
            }

            internal ButtonHelperHost Start()
            {
                return Start(20);
            }

            internal ButtonHelperHost Start(ulong cleanupBudgetTicks)
            {
                ButtonHelperHostOptions options = new ButtonHelperHostOptions(
                    PipeName,
                    Session,
                    ProcessId,
                    Clock.NowTick + 5000,
                    cleanupBudgetTicks,
                    5,
                    8,
                    8);
                return ButtonHelperHost.Start(options, Clock, Factory);
            }

            internal ButtonWireRequest Request(ButtonWireCommandKind kind, uint step, uint flags)
            {
                ButtonWireRequest request = new ButtonWireRequest();
                request.Kind = kind;
                request.SessionNonce = Session;
                request.GestureId = 41;
                request.Step = step;
                request.Flags = flags;
                request.DeadlineTick = GestureDeadline;
                request.AppProcessId = ProcessId;
                request.AppCreationIdentity = CreationIdentity;
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
        }

        private sealed class StopwatchClock : IButtonClock
        {
            private readonly Stopwatch watch = Stopwatch.StartNew();
            public ulong NowTick { get { return unchecked((ulong)watch.ElapsedMilliseconds + 100); } }
        }

        private sealed class HostLease : IAppProcessLease, IDisposable
        {
            private readonly uint processId;
            private readonly ulong creationIdentity;
            internal volatile bool Alive = true;
            internal volatile bool BlockDeadRead;
            internal readonly ManualResetEvent DeadReadEntered = new ManualResetEvent(false);
            internal readonly ManualResetEvent ReleaseDeadRead = new ManualResetEvent(false);
            internal int DisposeCalls;

            internal HostLease(uint processId, ulong creationIdentity)
            {
                this.processId = processId;
                this.creationIdentity = creationIdentity;
            }

            public uint ProcessId { get { return processId; } }
            public ulong CreationIdentity { get { return creationIdentity; } }
            public bool IsAlive
            {
                get
                {
                    if (!Alive && BlockDeadRead)
                    {
                        DeadReadEntered.Set();
                        ReleaseDeadRead.WaitOne();
                    }
                    return Alive;
                }
            }
            public void Dispose() { DisposeCalls++; }
        }

        private sealed class HostFactory : IButtonHelperHostComponentFactory
        {
            private readonly HostLease lease;
            private readonly HostInput input;
            private readonly HostObserver observer;
            private readonly EmptyEvidenceSource evidence;
            internal readonly ManualResetEvent LeaseOpened = new ManualResetEvent(false);
            internal readonly ManualResetEvent ComponentsCreated = new ManualResetEvent(false);
            internal int CreateComponentsCalls;
            internal Func<Stream, IButtonReplyTransport> ReplyTransportFactory;

            internal HostFactory(
                HostLease lease,
                HostInput input,
                HostObserver observer,
                EmptyEvidenceSource evidence)
            {
                this.lease = lease;
                this.input = input;
                this.observer = observer;
                this.evidence = evidence;
            }

            public IAppProcessLease OpenAppLease(uint expectedProcessId)
            {
                if (expectedProcessId != lease.ProcessId)
                    throw new Exception("fake expected PID changed");
                LeaseOpened.Set();
                return lease;
            }

            public ButtonHelperHostOwnerComponents CreateOwnerComponents()
            {
                CreateComponentsCalls++;
                ComponentsCreated.Set();
                return new ButtonHelperHostOwnerComponents(input, observer, evidence);
            }

            public IButtonReplyTransport CreateReplyTransport(Stream stream)
            {
                return ReplyTransportFactory == null
                    ? (IButtonReplyTransport)new PipeReplyTransport(stream)
                    : ReplyTransportFactory(stream);
            }
        }

        private sealed class HostInput : IButtonInputBackend
        {
            internal volatile bool BlockDown;
            internal readonly ManualResetEvent DownEntered = new ManualResetEvent(false);
            internal readonly ManualResetEvent ReleaseDown = new ManualResetEvent(false);
            internal readonly ManualResetEvent CleanupUpObserved = new ManualResetEvent(false);
            internal int DownCalls;
            internal int NormalUpCalls;
            internal int CleanupUpCalls;
            internal int TotalCalls { get { return DownCalls + NormalUpCalls + CleanupUpCalls; } }

            public ButtonSendAttempt SendNormal(
                ButtonNormalSendRequest request,
                Func<ButtonSendPermission> permission)
            {
                ButtonSendPermission allowed = permission();
                if (allowed != ButtonSendPermission.Allowed)
                    return new ButtonSendRefused("fake permission: " + allowed);
                if (request.Edge == ButtonNormalSendEdge.Down)
                {
                    DownCalls++;
                    if (BlockDown)
                    {
                        DownEntered.Set();
                        ReleaseDown.WaitOne();
                    }
                }
                else
                {
                    NormalUpCalls++;
                }
                return new ButtonSendCalled(1, 0, null);
            }

            public ButtonSendAttempt SendOwnedCleanupUp(ButtonCleanupSendRequest request)
            {
                CleanupUpCalls++;
                CleanupUpObserved.Set();
                return new ButtonSendCalled(1, 0, null);
            }
        }

        private sealed class HostObserver : IButtonDeliveryObserver
        {
            private ObservedButtonDownAnchor anchor;
            internal bool ConfirmCleanup = true;

            public ButtonDownObservationResult ObserveTaggedDown(
                ButtonGestureTuple gesture,
                TaggedButtonDelivery delivery)
            {
                anchor = new ObservedButtonDownAnchor(
                    gesture,
                    delivery,
                    Frame(gesture, gesture.Target.InputHwnd));
                return new ButtonDownObserved(anchor);
            }

            public ButtonReleaseObservationResult ObserveNormalUp(
                ObservedButtonDownAnchor expectedAnchor,
                NormalButtonUpInsertion insertion,
                TaggedButtonDelivery delivery)
            {
                return new ButtonReleaseObserved(
                    new NormalButtonReleaseEvidence(
                        expectedAnchor,
                        delivery,
                        Frame(expectedAnchor.Gesture, 0)));
            }

            public ButtonReleaseObservationResult ObserveOwnedCleanupUp(
                ObservedButtonDownAnchor expectedAnchor,
                OwnedCleanupButtonUpInsertion insertion)
            {
                if (anchor == null || expectedAnchor == null)
                {
                    return new ButtonReleaseUnconfirmed(
                        ButtonObservationFailureKind.MissingHistoricalDown,
                        "fake cleanup has no validated Down anchor");
                }
                if (!ConfirmCleanup)
                {
                    return new ButtonReleaseUnconfirmed(
                        ButtonObservationFailureKind.PhysicalLevelMismatch,
                        "fake physical Left level remained Down");
                }
                return new ButtonReleaseObserved(
                    new OwnedCleanupButtonReleaseEvidence(
                        expectedAnchor,
                        Frame(expectedAnchor.Gesture, 0).Access));
            }

            private static ButtonNormalFrame Frame(ButtonGestureTuple gesture, ulong capture)
            {
                ButtonForegroundIdentity foreground = new ButtonForegroundIdentity(
                    gesture.Target.ParentHwnd,
                    gesture.AppProcessId,
                    801,
                    gesture.AppCreationIdentity,
                    1,
                    "S-1-5-21-host-test",
                    0x2000);
                return new ButtonNormalFrame(
                    new ButtonAccessFrame(900, foreground, false),
                    new ButtonReceiverFrame(
                        gesture.Target.InputHwnd,
                        gesture.Target.ParentHwnd,
                        gesture.AppProcessId,
                        800,
                        capture));
            }
        }

        private sealed class EmptyEvidenceSource : IButtonReleaseEvidenceSource
        {
            public bool TryTake(out ButtonReleaseEvidence evidence)
            {
                evidence = null;
                return false;
            }
        }

        private sealed class DelayedReadStream : Stream
        {
            internal readonly ManualResetEvent Entered = new ManualResetEvent(false);
            internal readonly ManualResetEvent Release = new ManualResetEvent(false);
            internal int DisposeCalls;
            public override bool CanRead { get { return true; } }
            public override bool CanSeek { get { return false; } }
            public override bool CanWrite { get { return false; } }
            public override long Length { get { throw new NotSupportedException(); } }
            public override long Position
            {
                get { throw new NotSupportedException(); }
                set { throw new NotSupportedException(); }
            }
            public override void Flush() { }
            public override int Read(byte[] buffer, int offset, int count)
            {
                Entered.Set();
                Release.WaitOne();
                return 0;
            }
            public override long Seek(long offset, SeekOrigin origin) { throw new NotSupportedException(); }
            public override void SetLength(long value) { throw new NotSupportedException(); }
            public override void Write(byte[] buffer, int offset, int count) { throw new NotSupportedException(); }
            protected override void Dispose(bool disposing) { if (disposing) DisposeCalls++; }
        }

        private sealed class DelayedReplyTransport : IButtonReplyTransport
        {
            internal readonly ManualResetEvent Entered = new ManualResetEvent(false);
            internal readonly ManualResetEvent Release = new ManualResetEvent(false);
            internal int DisposeCalls;
            public void Write(byte[] frame, CancellationToken cancellationToken)
            {
                Entered.Set();
                Release.WaitOne();
            }
            public void Dispose() { DisposeCalls++; }
        }

        private sealed class FailNthReplyTransport : IButtonReplyTransport
        {
            private readonly PipeReplyTransport inner;
            private readonly int failAt;
            private int writes;
            internal readonly ManualResetEvent Faulted = new ManualResetEvent(false);

            internal FailNthReplyTransport(Stream stream, int failAt)
            {
                inner = new PipeReplyTransport(stream);
                this.failAt = failAt;
            }

            public void Write(byte[] frame, CancellationToken cancellationToken)
            {
                writes++;
                if (writes == failAt)
                {
                    Faulted.Set();
                    throw new IOException("injected terminal reply transport failure");
                }
                inner.Write(frame, cancellationToken);
            }

            public void Dispose()
            {
                inner.Dispose();
            }
        }

        private static void NotNull(object value, string label)
        {
            if (value == null) throw new Exception(label + ": expected non-null");
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
}
