using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;
using System.Threading.Tasks;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    internal interface IClipboardKeyBackend
    {
        void Validate(ulong hwnd, uint process);
        bool Pressed(int key);
        bool Send(ushort key, bool up);
    }

    // One owner owns the two release obligations; no other component sends keys.
    internal sealed class ClipboardKeyOwner
    {
        private readonly IClipboardKeyBackend backend;
        internal bool ControlOwned;
        internal bool VOwned;
        internal bool Outstanding { get { return ControlOwned || VOwned; } }
        internal ClipboardKeyOwner(IClipboardKeyBackend backend) { this.backend = backend; }
        internal void Begin(ulong hwnd, uint process)
        {
            if (Outstanding) throw new InvalidOperationException("clipboard keys already owned");
            backend.Validate(hwnd, process);
            foreach (int key in new int[] { 0x11, 0x56, 0x10, 0x12, 0x5b, 0x5c })
                if (backend.Pressed(key)) throw new InvalidOperationException("user modifier or V is pressed");
            if (!backend.Send(0x11, false)) throw new InvalidOperationException("Ctrl Down was not inserted");
            ControlOwned = true;
            backend.Validate(hwnd, process);
            if (!backend.Send(0x56, false)) throw new InvalidOperationException("V Down was not inserted");
            VOwned = true;
        }
        internal void Release()
        {
            // Both releases are attempted even when one fails. Never release an unowned key.
            try { if (VOwned && backend.Send(0x56, true) && !backend.Pressed(0x56)) VOwned = false; } catch { }
            try { if (ControlOwned && backend.Send(0x11, true) && !backend.Pressed(0x11)) ControlOwned = false; } catch { }
            if (Outstanding) throw new InvalidOperationException("clipboard key release remains unconfirmed");
        }
    }

    internal sealed class NativeClipboardKeyBackend : IClipboardKeyBackend
    {
        private string ownedDesktop;
        [DllImport("user32.dll", EntryPoint = "GetUserObjectInformationW", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool GetDesktopName(IntPtr handle, int index, [Out] byte[] bytes, uint length, out uint needed);
        public bool Pressed(int key) { return ButtonObservationNative.GetAsyncKeyState(key) < 0; }
        private static string DesktopName(IntPtr handle)
        {
            byte[] bytes = new byte[512];
            uint needed;
            if (handle == IntPtr.Zero || !GetDesktopName(handle, 2, bytes, (uint)bytes.Length, out needed))
                throw new InvalidOperationException("clipboard desktop name unavailable");
            return System.Text.Encoding.Unicode.GetString(bytes, 0, (int)needed).TrimEnd('\0');
        }
        public void Validate(ulong rawHwnd, uint process)
        {
            IntPtr hwnd = new IntPtr(unchecked((long)rawHwnd));
            uint actual;
            uint thread = ButtonObservationNative.GetWindowThreadProcessId(hwnd, out actual);
            if (actual != process || !ButtonObservationNative.IsWindow(hwnd) || !ButtonObservationNative.IsWindowVisible(hwnd)
                || ButtonObservationNative.GetAncestor(hwnd, 2) != hwnd || ButtonObservationNative.GetForegroundWindow() != hwnd)
                throw new InvalidOperationException("clipboard exact root HWND is not the visible foreground owner");
            IntPtr input = ButtonObservationNative.OpenInputDesktop(0, false, 1);
            try
            {
                string name = DesktopName(input);
                if (name != DesktopName(ButtonObservationNative.GetThreadDesktop(thread))
                    || name != DesktopName(ButtonObservationNative.GetThreadDesktop(ButtonObservationNative.GetCurrentThreadId())))
                    throw new InvalidOperationException("clipboard input desktops differ");
                ownedDesktop = name;
            }
            finally { if (input != IntPtr.Zero) ButtonObservationNative.CloseDesktop(input); }
        }
        public bool Send(ushort key, bool up)
        {
            // Cleanup releases only this owner's desktop; it does not retarget a
            // newly selected input desktop after App death or foreground changes.
            IntPtr inputDesktop = ButtonObservationNative.OpenInputDesktop(0, false, 1);
            try
            {
                if (ownedDesktop == null || ownedDesktop != DesktopName(inputDesktop)
                    || ownedDesktop != DesktopName(ButtonObservationNative.GetThreadDesktop(ButtonObservationNative.GetCurrentThreadId())))
                    return false;
            }
            finally { if (inputDesktop != IntPtr.Zero) ButtonObservationNative.CloseDesktop(inputDesktop); }
            ButtonInputNative.Input input = new ButtonInputNative.Input();
            input.Type = 1;
            input.Data.Keyboard.VirtualKey = key;
            input.Data.Keyboard.Flags = up ? 2U : 0U;
            if (ButtonInputNative.SendInput(1, new ButtonInputNative.Input[] { input }, Marshal.SizeOf(typeof(ButtonInputNative.Input))) != 1) return false;
            if (up)
            {
                System.Diagnostics.Stopwatch clock = System.Diagnostics.Stopwatch.StartNew();
                while (Pressed(key) && clock.ElapsedMilliseconds < 500) Thread.Sleep(5);
            }
            return true;
        }
    }

    // Runner-owned thread survives App failure/termination and joins before App cleanup.
    // It reuses the same authenticated local SID pipe and pinned process lease as mouse input.
    public sealed class ClipboardKeyHelperHandle
    {
        private readonly string pipeName;
        private readonly Guid session;
        private readonly uint appPid;
        private readonly int acceptTimeout;
        private readonly Thread worker;
        private readonly ManualResetEventSlim stop = new ManualResetEventSlim(false);
        private LocalButtonPipeServer pipe;
        private volatile ButtonHelperRunnerStatus status;
        private ClipboardKeyHelperHandle(string pipeName, Guid session, uint appPid, int timeout)
        {
            this.pipeName = pipeName; this.session = session; this.appPid = appPid; acceptTimeout = timeout;
            worker = new Thread(Run); worker.IsBackground = true; worker.Name = "clipboard-smoke-key-owner";
            worker.Start();
        }
        public static ClipboardKeyHelperHandle Start(string name, string nonce, uint pid, int timeout)
        {
            Guid guid;
            if (!Guid.TryParseExact(nonce, "D", out guid) || guid == Guid.Empty || pid == 0 || timeout <= 0)
                throw new ArgumentException("clipboard key helper identity is incomplete");
            return new ClipboardKeyHelperHandle(name, guid, pid, timeout);
        }
        public ButtonHelperRunnerStatus CancelAndJoin(string reason, int timeoutMilliseconds)
        {
            stop.Set();
            if (!worker.Join(timeoutMilliseconds)) return ButtonHelperRunnerHandle.StillRunningStatus(reason);
            return status;
        }
        private void Run()
        {
            ClipboardKeyOwner owner = new ClipboardKeyOwner(new NativeClipboardKeyBackend());
            PinnedAppProcessLease app = null;
            Task<int> read = null;
            Task writing = null;
            string failure = null;
            int completed = 0;
            bool pipeJoined = false;
            try
            {
                app = new PinnedAppProcessLease(appPid);
                pipe = LocalButtonPipeServer.Create(pipeName, appPid);
                System.Diagnostics.Stopwatch clock = System.Diagnostics.Stopwatch.StartNew();
                while (!pipe.PollAuthenticatedConnection(10))
                {
                    if (stop.IsSet || !app.IsAlive || clock.ElapsedMilliseconds >= acceptTimeout)
                        throw new OperationCanceledException("clipboard helper accept stopped");
                }
                ulong current = 0;
                ulong hwnd = 0;
                long heldDeadline = 0;
                while (!stop.IsSet && app.IsAlive)
                {
                    byte[] request = new byte[56];
                    int offset = 0;
                    while (offset < request.Length)
                    {
                        read = pipe.Stream.ReadAsync(request, offset, request.Length - offset);
                        while (!read.Wait(10))
                        {
                            if (stop.IsSet || !app.IsAlive || (owner.Outstanding && clock.ElapsedMilliseconds >= heldDeadline))
                                throw new OperationCanceledException("clipboard key request/receipt timed out or cancelled");
                        }
                        int count = read.Result;
                        read = null;
                        if (count == 0)
                        {
                            if (owner.Outstanding) throw new EndOfStreamException("clipboard app disconnected while keys held");
                            return;
                        }
                        offset += count;
                    }
                    uint magic = BitConverter.ToUInt32(request, 0);
                    uint kind = BitConverter.ToUInt32(request, 4);
                    ulong id = BitConverter.ToUInt64(request, 8);
                    byte[] nonce = new byte[16]; Array.Copy(request, 16, nonce, 0, 16);
                    ulong requestedHwnd = BitConverter.ToUInt64(request, 32);
                    uint pid = BitConverter.ToUInt32(request, 40);
                    ulong token = BitConverter.ToUInt64(request, 48);
                    if (magic != 0x4b43564d || new Guid(nonce) != session || pid != appPid || token == 0 || id == 0)
                        throw new InvalidDataException("clipboard key pipe identity mismatch");
                    if (!app.IsAlive || stop.IsSet) throw new OperationCanceledException("clipboard app is no longer alive");
                    if (kind == 1)
                    {
                        if (id <= current || owner.Outstanding) throw new InvalidDataException("clipboard begin is stale or concurrent");
                        current = id; hwnd = requestedHwnd; heldDeadline = clock.ElapsedMilliseconds + 5000;
                        owner.Begin(hwnd, appPid);
                    }
                    else if (kind == 2)
                    {
                        if (id != current || hwnd != requestedHwnd || !owner.Outstanding) throw new InvalidDataException("clipboard release identity mismatch");
                        owner.Release(); completed++;
                    }
                    else throw new InvalidDataException("unknown clipboard key request");
                    byte[] reply = new byte[16];
                    Array.Copy(BitConverter.GetBytes(magic), 0, reply, 0, 4);
                    Array.Copy(BitConverter.GetBytes(id), 0, reply, 8, 8);
                    // Async replies cannot strand the input owner on a stalled App.
                    writing = pipe.Stream.WriteAsync(reply, 0, reply.Length);
                    while (!writing.Wait(10))
                        if (stop.IsSet || !app.IsAlive || (owner.Outstanding && clock.ElapsedMilliseconds >= heldDeadline))
                            throw new OperationCanceledException("clipboard reply stopped");
                    writing.GetAwaiter().GetResult();
                    writing = null;
                }
            }
            catch (Exception error) { failure = error.Message; }
            finally
            {
                try { owner.Release(); } catch (Exception error) { if (failure == null) failure = error.Message; }
                if (pipe != null)
                {
                    pipe.RequestStop();
                    try { if (read != null) read.Wait(1000); } catch (AggregateException) { }
                    try { if (writing != null) writing.Wait(1000); } catch (AggregateException) { }
                    pipeJoined = (read == null || read.IsCompleted) && (writing == null || writing.IsCompleted) && pipe.TryJoinAndDispose(1000);
                }
                else pipeJoined = true;
                if (app != null) app.Dispose();
                status = new ButtonHelperRunnerStatus {
                    Joined = pipeJoined,
                    SafeToTerminateApp = pipeJoined && !owner.Outstanding,
                    Succeeded = failure == null && completed > 0 && !owner.Outstanding,
                    HasOutstandingRelease = owner.Outstanding,
                    ReleaseState = owner.Outstanding ? ButtonHelperReleaseState.ConfirmedOutstanding : (completed > 0 ? ButtonHelperReleaseState.ConfirmedReleased : ButtonHelperReleaseState.NoOwnedDown),
                    TerminalKind = failure == null ? "Succeeded" : "ClipboardKeyFailure",
                    PrimaryFailure = failure,
                    Detail = "completed clipboard key gestures=" + completed
                };
            }
        }
    }
}
