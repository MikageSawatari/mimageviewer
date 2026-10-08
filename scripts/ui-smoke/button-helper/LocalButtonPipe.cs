using System;
using System.Collections.Concurrent;
using System.ComponentModel;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Threading;
using Microsoft.Win32.SafeHandles;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    // Opens the exact process once and never resolves the PID again. Creation
    // identity comes from that held handle, so PID reuse cannot retarget it.
    internal sealed class PinnedAppProcessLease : IAppProcessLease, IDisposable
    {
        private readonly SafeWaitHandle process;
        private readonly uint processId;
        private readonly ulong creationIdentity;

        internal PinnedAppProcessLease(uint expectedProcessId)
        {
            if (expectedProcessId == 0)
                throw new ArgumentOutOfRangeException("expectedProcessId");
            process = NativeMethods.OpenProcess(
                NativeMethods.Synchronize | NativeMethods.ProcessQueryLimitedInformation,
                false,
                expectedProcessId);
            if (process == null || process.IsInvalid)
                throw new Win32Exception(Marshal.GetLastWin32Error());
            try
            {
                uint actual = NativeMethods.GetProcessId(process);
                if (actual == 0) throw new Win32Exception(Marshal.GetLastWin32Error());
                if (actual != expectedProcessId)
                    throw new InvalidOperationException("opened process PID mismatch");
                NativeMethods.FileTime created;
                NativeMethods.FileTime exited;
                NativeMethods.FileTime kernel;
                NativeMethods.FileTime user;
                if (!NativeMethods.GetProcessTimes(
                    process,
                    out created,
                    out exited,
                    out kernel,
                    out user))
                {
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                }
                processId = actual;
                creationIdentity = created.ToUInt64();
                if (creationIdentity == 0)
                    throw new InvalidOperationException("zero process creation identity");
            }
            catch
            {
                process.Dispose();
                throw;
            }
        }

        public uint ProcessId { get { return processId; } }
        public ulong CreationIdentity { get { return creationIdentity; } }
        public bool IsAlive
        {
            get
            {
                return NativeMethods.WaitForSingleObject(process, 0)
                    == NativeMethods.WaitTimeout;
            }
        }

        public void Dispose()
        {
            process.Dispose();
        }
    }

    internal sealed class LocalButtonPipeServer : IDisposable
    {
        private readonly NamedPipeServerStream stream;
        private readonly uint expectedClientProcessId;
        private System.Threading.Tasks.Task connectionWait;
        private bool authenticated;
        private bool stopRequested;
        private bool disposed;

        private LocalButtonPipeServer(NamedPipeServerStream stream, uint expectedClientProcessId)
        {
            this.stream = stream;
            this.expectedClientProcessId = expectedClientProcessId;
        }

        // Join-only seam: it lets the fake lifecycle tests prove that closing
        // the pipe is not itself an accept-task join. No client authentication
        // or input path uses an instance created here.
        internal static LocalButtonPipeServer CreateJoinTestInstance(
            NamedPipeServerStream stream,
            System.Threading.Tasks.Task connectionWait)
        {
            if (stream == null) throw new ArgumentNullException("stream");
            if (connectionWait == null) throw new ArgumentNullException("connectionWait");
            LocalButtonPipeServer server = new LocalButtonPipeServer(stream, 1);
            server.connectionWait = connectionWait;
            return server;
        }

        internal Stream Stream { get { return stream; } }

        internal static LocalButtonPipeServer Create(string pipeName, uint expectedClientProcessId)
        {
            if (String.IsNullOrWhiteSpace(pipeName)
                || pipeName.IndexOf('\\') >= 0
                || pipeName.Length > 160)
            {
                throw new ArgumentException("invalid local pipe name", "pipeName");
            }
            if (expectedClientProcessId == 0)
                throw new ArgumentOutOfRangeException("expectedClientProcessId");

            SecurityIdentifier sid = WindowsIdentity.GetCurrent().User;
            if (sid == null) throw new InvalidOperationException("current SID is unavailable");
            string sddl = "D:P(A;;FA;;;" + sid.Value + ")";
            IntPtr descriptor = IntPtr.Zero;
            if (!NativeMethods.ConvertStringSecurityDescriptorToSecurityDescriptor(
                sddl,
                1,
                out descriptor,
                IntPtr.Zero))
            {
                throw new Win32Exception(Marshal.GetLastWin32Error());
            }

            SafePipeHandle safeHandle = null;
            try
            {
                NativeMethods.SecurityAttributes attributes = new NativeMethods.SecurityAttributes();
                attributes.Length = Marshal.SizeOf(typeof(NativeMethods.SecurityAttributes));
                attributes.SecurityDescriptor = descriptor;
                attributes.InheritHandle = 0;
                IntPtr raw = NativeMethods.CreateNamedPipe(
                    "\\\\.\\pipe\\" + pipeName,
                    NativeMethods.PipeAccessDuplex
                        | NativeMethods.FileFlagFirstPipeInstance
                        | NativeMethods.FileFlagOverlapped,
                    NativeMethods.PipeTypeByte
                        | NativeMethods.PipeReadModeByte
                        | NativeMethods.PipeWait
                        | NativeMethods.PipeRejectRemoteClients,
                    1,
                    4096,
                    4096,
                    0,
                    ref attributes);
                if (raw == new IntPtr(-1))
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                safeHandle = new SafePipeHandle(raw, true);
                NamedPipeServerStream server = new NamedPipeServerStream(
                    PipeDirection.InOut,
                    true,
                    false,
                    safeHandle);
                safeHandle = null;
                return new LocalButtonPipeServer(server, expectedClientProcessId);
            }
            finally
            {
                if (safeHandle != null) safeHandle.Dispose();
                if (descriptor != IntPtr.Zero) NativeMethods.LocalFree(descriptor);
            }
        }

        internal void WaitForAuthenticatedConnection(int timeoutMilliseconds)
        {
            if (timeoutMilliseconds <= 0)
                throw new ArgumentOutOfRangeException("timeoutMilliseconds");
            BeginWaitForAuthenticatedConnection();
            if (!PollAuthenticatedConnection(timeoutMilliseconds))
                throw new TimeoutException("timed out waiting for local named-pipe client");
        }

        internal void BeginWaitForAuthenticatedConnection()
        {
            if (disposed || stopRequested)
                throw new ObjectDisposedException("LocalButtonPipeServer");
            if (connectionWait == null)
                connectionWait = stream.WaitForConnectionAsync();
        }

        internal bool PollAuthenticatedConnection(int timeoutMilliseconds)
        {
            if (timeoutMilliseconds < 0)
                throw new ArgumentOutOfRangeException("timeoutMilliseconds");
            if (authenticated) return true;
            BeginWaitForAuthenticatedConnection();
            if (!connectionWait.Wait(timeoutMilliseconds)) return false;
            uint actual;
            if (!NativeMethods.GetNamedPipeClientProcessId(stream.SafePipeHandle, out actual))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            if (actual != expectedClientProcessId)
                throw new UnauthorizedAccessException("named-pipe client PID mismatch");
            authenticated = true;
            return true;
        }

        internal void RequestStop()
        {
            if (stopRequested) return;
            stopRequested = true;
            stream.Dispose();
        }

        internal bool TryJoinAndDispose(int timeoutMilliseconds)
        {
            if (timeoutMilliseconds < 0)
                throw new ArgumentOutOfRangeException("timeoutMilliseconds");
            if (disposed) return true;
            RequestStop();
            if (connectionWait != null && !connectionWait.IsCompleted)
            {
                try
                {
                    if (!connectionWait.Wait(timeoutMilliseconds)) return false;
                }
                catch (AggregateException)
                {
                    // A disposed stream completes the one pending accept as a
                    // fault. Completion, rather than success, is the join proof.
                }
            }
            if (connectionWait != null && !connectionWait.IsCompleted) return false;
            disposed = true;
            stream.Dispose();
            return true;
        }

        public void Dispose()
        {
            if (!TryJoinAndDispose(1000))
                throw new TimeoutException("named-pipe accept did not stop before disposal");
        }
    }

    internal static class LocalButtonPipeClientProof
    {
        internal static void VerifyServerProcess(NamedPipeClientStream client, uint expectedServerPid)
        {
            if (client == null || !client.IsConnected)
                throw new InvalidOperationException("pipe client is not connected");
            uint actual;
            if (!NativeMethods.GetNamedPipeServerProcessId(client.SafePipeHandle, out actual))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            if (actual != expectedServerPid)
                throw new UnauthorizedAccessException("named-pipe server PID mismatch");
        }
    }

    internal sealed class PipeReplyTransport : IButtonReplyTransport
    {
        private readonly Stream stream;

        internal PipeReplyTransport(Stream stream)
        {
            this.stream = stream;
        }

        public void Write(byte[] payload, CancellationToken cancellationToken)
        {
            cancellationToken.ThrowIfCancellationRequested();
            ButtonWireCodec.WriteFrame(stream, payload);
        }

        public void Dispose()
        {
            stream.Dispose();
        }
    }

    // This thread only parses and queues bounded messages. It never receives an
    // input backend or reducer reference, so only the owner can insert input.
    internal sealed class BoundedPipeRequestReader : IDisposable
    {
        private readonly Stream stream;
        private readonly BlockingCollection<ButtonWireRequest> requests;
        private readonly AutoResetEvent activity;
        private readonly Thread reader;
        private volatile string fault;
        private volatile bool eof;
        private bool stopRequested;
        private bool resourcesDisposed;

        internal BoundedPipeRequestReader(Stream stream, int capacity)
        {
            if (stream == null) throw new ArgumentNullException("stream");
            if (capacity <= 0) throw new ArgumentOutOfRangeException("capacity");
            this.stream = stream;
            requests = new BlockingCollection<ButtonWireRequest>(capacity);
            activity = new AutoResetEvent(false);
            reader = new Thread(ReadLoop);
            reader.IsBackground = true;
            reader.Name = "miv-ui-smoke-button-request-reader";
            reader.Start();
        }

        internal string Fault { get { return fault; } }
        internal bool Eof { get { return eof; } }
        internal bool HasPending { get { return requests.Count != 0; } }

        internal bool TryDequeue(out ButtonWireRequest request)
        {
            return requests.TryTake(out request);
        }

        internal bool WaitForActivity(int timeoutMilliseconds)
        {
            return activity.WaitOne(timeoutMilliseconds);
        }

        public void Dispose()
        {
            if (!TryJoinAndDispose(1000))
                throw new TimeoutException("button request reader did not stop before disposal");
        }

        internal void RequestStop()
        {
            if (stopRequested) return;
            stopRequested = true;
            stream.Dispose();
            requests.CompleteAdding();
        }

        internal bool TryJoinAndDispose(int timeoutMilliseconds)
        {
            if (timeoutMilliseconds < 0)
                throw new ArgumentOutOfRangeException("timeoutMilliseconds");
            if (resourcesDisposed) return true;
            RequestStop();
            if (!reader.Join(timeoutMilliseconds)) return false;
            activity.Dispose();
            requests.Dispose();
            resourcesDisposed = true;
            return true;
        }

        private void ReadLoop()
        {
            try
            {
                while (true)
                {
                    ButtonWireRequest request = ButtonWireCodec.DecodeRequest(
                        ButtonWireCodec.ReadFrame(stream));
                    if (!requests.TryAdd(request))
                    {
                        fault = "bounded request queue overflow";
                        activity.Set();
                        return;
                    }
                    activity.Set();
                }
            }
            catch (EndOfStreamException)
            {
                eof = true;
                activity.Set();
            }
            catch (ObjectDisposedException)
            {
                eof = true;
                activity.Set();
            }
            catch (Exception error)
            {
                fault = error.GetType().Name + ": " + error.Message;
                activity.Set();
            }
        }
    }

    internal static class NativeMethods
    {
        internal const uint Synchronize = 0x00100000;
        internal const uint ProcessQueryLimitedInformation = 0x00001000;
        internal const uint WaitTimeout = 0x00000102;
        internal const uint PipeAccessDuplex = 0x00000003;
        internal const uint FileFlagFirstPipeInstance = 0x00080000;
        internal const uint FileFlagOverlapped = 0x40000000;
        internal const uint PipeTypeByte = 0x00000000;
        internal const uint PipeReadModeByte = 0x00000000;
        internal const uint PipeWait = 0x00000000;
        internal const uint PipeRejectRemoteClients = 0x00000008;

        [StructLayout(LayoutKind.Sequential)]
        internal struct SecurityAttributes
        {
            internal int Length;
            internal IntPtr SecurityDescriptor;
            internal int InheritHandle;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct FileTime
        {
            internal uint Low;
            internal uint High;

            internal ulong ToUInt64()
            {
                return ((ulong)High << 32) | Low;
            }
        }

        [DllImport("kernel32.dll", SetLastError = true)]
        internal static extern SafeWaitHandle OpenProcess(
            uint desiredAccess,
            [MarshalAs(UnmanagedType.Bool)] bool inheritHandle,
            uint processId);

        [DllImport("kernel32.dll", SetLastError = true)]
        internal static extern uint GetProcessId(SafeWaitHandle process);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetProcessTimes(
            SafeWaitHandle process,
            out FileTime creation,
            out FileTime exit,
            out FileTime kernel,
            out FileTime user);

        [DllImport("kernel32.dll", SetLastError = true)]
        internal static extern uint WaitForSingleObject(SafeWaitHandle handle, uint milliseconds);

        [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode,
            EntryPoint = "ConvertStringSecurityDescriptorToSecurityDescriptorW")]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool ConvertStringSecurityDescriptorToSecurityDescriptor(
            string stringSecurityDescriptor,
            uint stringSdRevision,
            out IntPtr securityDescriptor,
            IntPtr securityDescriptorSize);

        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode,
            EntryPoint = "CreateNamedPipeW")]
        internal static extern IntPtr CreateNamedPipe(
            string name,
            uint openMode,
            uint pipeMode,
            uint maximumInstances,
            uint outputBufferSize,
            uint inputBufferSize,
            uint defaultTimeout,
            ref SecurityAttributes securityAttributes);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetNamedPipeClientProcessId(
            SafePipeHandle pipe,
            out uint clientProcessId);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetNamedPipeServerProcessId(
            SafePipeHandle pipe,
            out uint serverProcessId);

        [DllImport("kernel32.dll")]
        internal static extern IntPtr LocalFree(IntPtr memory);
    }
}
