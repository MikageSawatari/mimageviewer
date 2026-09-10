using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Principal;
using Microsoft.Win32.SafeHandles;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    // Read-only native facade. It opens no application data and never sends,
    // focuses, attaches, switches, or hooks input. OpenInputDesktop is used only
    // as a bounded access check and its acquired handle is always closed.
    internal sealed class NativeButtonObservationPlatform : IButtonObservationPlatform
    {
        private readonly uint helperProcessId;
        private readonly uint helperSessionId;
        private readonly string helperUserSid;
        private readonly uint helperIntegrityRid;

        internal NativeButtonObservationPlatform()
        {
            helperProcessId = ButtonObservationNative.GetCurrentProcessId();
            ButtonProcessIdentity helper = QueryProcessIdentity(helperProcessId);
            helperSessionId = helper.SessionId;
            helperUserSid = helper.UserSid;
            helperIntegrityRid = helper.IntegrityRid;
        }

        public ButtonNormalLevelProbe ProbeNormalLevel(
            ButtonGestureTuple gesture,
            ButtonCaptureExpectation captureExpectation)
        {
            if (gesture == null) throw new ArgumentNullException("gesture");
            IntPtr opened = OpenCertifiedInputDesktop();
            try
            {
                ButtonNormalFrame before = CaptureNormalFrame(
                    gesture,
                    captureExpectation,
                    opened);
                ButtonPhysicalLevel level = ReadPhysicalLeftButton();
                ButtonNormalFrame after = CaptureNormalFrame(
                    gesture,
                    captureExpectation,
                    opened);
                return new ButtonNormalLevelProbe(
                    before,
                    level,
                    after,
                    ReadVirtualDesktopPointTolerance());
            }
            finally
            {
                ButtonObservationNative.CloseDesktop(opened);
            }
        }

        public ButtonAccessLevelProbe ProbeAccessLevel()
        {
            IntPtr opened = OpenCertifiedInputDesktop();
            try
            {
                ButtonAccessFrame before = CaptureAccessFrame(opened);
                ButtonPhysicalLevel level = ReadPhysicalLeftButton();
                ButtonAccessFrame after = CaptureAccessFrame(opened);
                return new ButtonAccessLevelProbe(before, level, after);
            }
            finally
            {
                ButtonObservationNative.CloseDesktop(opened);
            }
        }

        internal ButtonNormalFrame CaptureNormalFrame(
            ButtonGestureTuple gesture,
            ButtonCaptureExpectation captureExpectation,
            IntPtr openedInputDesktop)
        {
            ButtonAccessFrame access = CaptureAccessFrame(openedInputDesktop);
            IntPtr input = new IntPtr(unchecked((long)gesture.Target.InputHwnd));
            IntPtr parent = new IntPtr(unchecked((long)gesture.Target.ParentHwnd));
            if (!ButtonObservationNative.IsWindow(input)
                || !ButtonObservationNative.IsWindow(parent)
                || !ButtonObservationNative.IsWindowVisible(input)
                || !ButtonObservationNative.IsWindowVisible(parent))
            {
                throw Unavailable(
                    ButtonObservationFailureKind.ReceiverUnavailable,
                    "normal receiver or parent is not a visible live window");
            }
            uint inputPid;
            uint inputTid = ButtonObservationNative.GetWindowThreadProcessId(input, out inputPid);
            uint parentPid;
            uint parentTid = ButtonObservationNative.GetWindowThreadProcessId(parent, out parentPid);
            if (inputTid == 0 || parentTid == 0
                || inputPid != gesture.AppProcessId
                || parentPid != gesture.AppProcessId
                || ButtonObservationNative.GetAncestor(input, 2) != parent
                || access.Foreground.Hwnd != gesture.Target.ParentHwnd)
            {
                throw Unavailable(
                    ButtonObservationFailureKind.WrongReceiver,
                    "normal receiver identity, root parent, or foreground owner changed");
            }
            ButtonObservationNative.GuiThreadInfo info = new ButtonObservationNative.GuiThreadInfo();
            info.Size = (uint)Marshal.SizeOf(typeof(ButtonObservationNative.GuiThreadInfo));
            if (!ButtonObservationNative.GetGUIThreadInfo(inputTid, ref info))
                throw Unavailable(
                    ButtonObservationFailureKind.ReceiverUnavailable,
                    "GetGUIThreadInfo failed for normal receiver");
            ulong capture = unchecked((ulong)info.Capture.ToInt64());
            bool captureMatches = captureExpectation == ButtonCaptureExpectation.InputWindow
                ? info.Capture == input
                : info.Capture == IntPtr.Zero;
            if (!captureMatches)
                throw Unavailable(
                    ButtonObservationFailureKind.CaptureMismatch,
                    "normal receiver capture does not match the expected button edge");
            return new ButtonNormalFrame(
                access,
                new ButtonReceiverFrame(
                    gesture.Target.InputHwnd,
                    gesture.Target.ParentHwnd,
                    inputPid,
                    inputTid,
                    capture));
        }

        internal ButtonAccessFrame CaptureAccessFrame(IntPtr openedInputDesktop)
        {
            ulong threadDesktopIdentity = ValidateInputDesktopOnly(openedInputDesktop);

            IntPtr foreground = ButtonObservationNative.GetForegroundWindow();
            if (foreground == IntPtr.Zero)
                throw Unavailable(
                    ButtonObservationFailureKind.ForegroundUnavailable,
                    "foreground window is unavailable during button-level sampling");
            uint foregroundPid;
            uint foregroundTid = ButtonObservationNative.GetWindowThreadProcessId(
                foreground,
                out foregroundPid);
            if (foregroundTid == 0 || foregroundPid == 0)
                throw Unavailable(
                    ButtonObservationFailureKind.ForegroundUnavailable,
                    "foreground PID/TID is unavailable during button-level sampling");
            ButtonProcessIdentity identity;
            try
            {
                identity = QueryProcessIdentity(foregroundPid);
            }
            catch (Exception error)
            {
                throw Unavailable(
                    ButtonObservationFailureKind.TokenAccessUnavailable,
                    "foreground process/token access is unavailable: " + error.Message);
            }
            if (identity.SessionId != helperSessionId
                || !String.Equals(identity.UserSid, helperUserSid, StringComparison.Ordinal)
                || helperIntegrityRid < identity.IntegrityRid)
            {
                throw Unavailable(
                    ButtonObservationFailureKind.TokenAccessUnavailable,
                    "foreground user/session/integrity cannot certify a zero button sample");
            }
            return new ButtonAccessFrame(
                threadDesktopIdentity,
                new ButtonForegroundIdentity(
                    unchecked((ulong)foreground.ToInt64()),
                    foregroundPid,
                    foregroundTid,
                    identity.CreationIdentity,
                    identity.SessionId,
                    identity.UserSid,
                    identity.IntegrityRid),
                ButtonObservationNative.GetSystemMetrics(23) != 0);
        }

        internal ulong ValidateInputDesktopOnly(IntPtr openedInputDesktop)
        {
            uint threadId = ButtonObservationNative.GetCurrentThreadId();
            IntPtr threadDesktop = ButtonObservationNative.GetThreadDesktop(threadId);
            if (threadDesktop == IntPtr.Zero || !DesktopIsInput(threadDesktop))
                throw Unavailable(
                    ButtonObservationFailureKind.DesktopUnavailable,
                    "helper thread desktop is not the active input desktop");

            if (openedInputDesktop == IntPtr.Zero || !DesktopIsInput(openedInputDesktop))
                throw Unavailable(
                    ButtonObservationFailureKind.DesktopUnavailable,
                    "opened input desktop stopped receiving input during button-level sampling");
            return unchecked((ulong)threadDesktop.ToInt64());
        }

        private ButtonPhysicalLevel ReadPhysicalLeftButton()
        {
            short state = ButtonObservationNative.GetAsyncKeyState(1);
            return (state & unchecked((short)0x8000)) != 0
                ? ButtonPhysicalLevel.Down
                : ButtonPhysicalLevel.Up;
        }

        internal static ButtonPointTolerance ReadVirtualDesktopPointTolerance()
        {
            int width = ButtonObservationNative.GetSystemMetrics(78);
            int height = ButtonObservationNative.GetSystemMetrics(79);
            if (width <= 0 || height <= 0)
                throw Unavailable(
                    ButtonObservationFailureKind.DesktopUnavailable,
                    "virtual desktop extent is not usable for physical point validation");
            return new ButtonPointTolerance(
                1 + (width - 1) / 131070,
                1 + (height - 1) / 131070);
        }

        internal string DescribeReadOnlyEnvironment()
        {
            try
            {
                ButtonAccessLevelProbe frame = ProbeAccessLevel();
                return ButtonObservationEnvironmentDescription.Describe(frame);
            }
            catch (ButtonObservationUnavailableException error)
            {
                return "available=false,reason=" + error.Kind;
            }
        }

        internal static IntPtr OpenCertifiedInputDesktop()
        {
            IntPtr opened = ButtonObservationNative.OpenInputDesktop(0, false, 0x00000009);
            if (opened == IntPtr.Zero)
                throw Unavailable(
                    ButtonObservationFailureKind.DesktopUnavailable,
                    "input desktop READOBJECTS|HOOKCONTROL access is unavailable");
            if (DesktopIsInput(opened)) return opened;
            ButtonObservationNative.CloseDesktop(opened);
            throw Unavailable(
                ButtonObservationFailureKind.DesktopUnavailable,
                "opened input desktop is not currently receiving input");
        }

        private static bool DesktopIsInput(IntPtr desktop)
        {
            int value;
            uint needed;
            return ButtonObservationNative.GetUserObjectInformationW(
                    desktop,
                    6,
                    out value,
                    4,
                    out needed)
                && needed == 4
                && value != 0;
        }

        private static ButtonProcessIdentity QueryProcessIdentity(uint processId)
        {
            using (SafeWaitHandle process = ButtonObservationNative.OpenProcess(
                0x00001000,
                false,
                processId))
            {
                if (process == null || process.IsInvalid)
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                ButtonObservationNative.FileTime created;
                ButtonObservationNative.FileTime exited;
                ButtonObservationNative.FileTime kernel;
                ButtonObservationNative.FileTime user;
                if (!ButtonObservationNative.GetProcessTimes(
                    process,
                    out created,
                    out exited,
                    out kernel,
                    out user))
                {
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                }
                uint sessionId;
                if (!ButtonObservationNative.ProcessIdToSessionId(processId, out sessionId))
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                SafeWaitHandle token;
                if (!ButtonObservationNative.OpenProcessToken(process, 0x0008, out token))
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                using (token)
                {
                    return new ButtonProcessIdentity(
                        created.ToUInt64(),
                        sessionId,
                        QueryTokenUserSid(token),
                        QueryTokenIntegrityRid(token));
                }
            }
        }

        private static string QueryTokenUserSid(SafeWaitHandle token)
        {
            IntPtr buffer = QueryTokenInformation(token, 1);
            try
            {
                IntPtr sid = Marshal.ReadIntPtr(buffer);
                IntPtr text;
                if (!ButtonObservationNative.ConvertSidToStringSidW(sid, out text))
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                try
                {
                    return Marshal.PtrToStringUni(text);
                }
                finally
                {
                    ButtonObservationNative.LocalFree(text);
                }
            }
            finally
            {
                Marshal.FreeHGlobal(buffer);
            }
        }

        private static uint QueryTokenIntegrityRid(SafeWaitHandle token)
        {
            IntPtr buffer = QueryTokenInformation(token, 25);
            try
            {
                IntPtr sid = Marshal.ReadIntPtr(buffer);
                IntPtr countPointer = ButtonObservationNative.GetSidSubAuthorityCount(sid);
                if (countPointer == IntPtr.Zero)
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                byte count = Marshal.ReadByte(countPointer);
                if (count == 0) throw new InvalidOperationException("integrity SID has no RID");
                IntPtr ridPointer = ButtonObservationNative.GetSidSubAuthority(sid, (uint)(count - 1));
                if (ridPointer == IntPtr.Zero)
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                return unchecked((uint)Marshal.ReadInt32(ridPointer));
            }
            finally
            {
                Marshal.FreeHGlobal(buffer);
            }
        }

        private static IntPtr QueryTokenInformation(SafeWaitHandle token, int informationClass)
        {
            uint required;
            ButtonObservationNative.GetTokenInformation(
                token,
                informationClass,
                IntPtr.Zero,
                0,
                out required);
            if (required == 0) throw new Win32Exception(Marshal.GetLastWin32Error());
            IntPtr buffer = Marshal.AllocHGlobal(unchecked((int)required));
            if (!ButtonObservationNative.GetTokenInformation(
                token,
                informationClass,
                buffer,
                required,
                out required))
            {
                int error = Marshal.GetLastWin32Error();
                Marshal.FreeHGlobal(buffer);
                throw new Win32Exception(error);
            }
            return buffer;
        }

        private static ButtonObservationUnavailableException Unavailable(
            ButtonObservationFailureKind kind,
            string detail)
        {
            return new ButtonObservationUnavailableException(kind, detail);
        }
    }

    internal sealed class ButtonProcessIdentity
    {
        internal readonly ulong CreationIdentity;
        internal readonly uint SessionId;
        internal readonly string UserSid;
        internal readonly uint IntegrityRid;

        internal ButtonProcessIdentity(
            ulong creationIdentity,
            uint sessionId,
            string userSid,
            uint integrityRid)
        {
            CreationIdentity = creationIdentity;
            SessionId = sessionId;
            UserSid = userSid;
            IntegrityRid = integrityRid;
        }
    }

    internal static class ButtonObservationNative
    {
        [StructLayout(LayoutKind.Sequential)]
        internal struct FileTime
        {
            internal uint Low;
            internal uint High;
            internal ulong ToUInt64() { return ((ulong)High << 32) | Low; }
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct Rect
        {
            internal int Left;
            internal int Top;
            internal int Right;
            internal int Bottom;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct GuiThreadInfo
        {
            internal uint Size;
            internal uint Flags;
            internal IntPtr Active;
            internal IntPtr Focus;
            internal IntPtr Capture;
            internal IntPtr MenuOwner;
            internal IntPtr MoveSize;
            internal IntPtr Caret;
            internal Rect CaretRect;
        }

        [DllImport("kernel32.dll")]
        internal static extern uint GetCurrentProcessId();

        [DllImport("kernel32.dll")]
        internal static extern uint GetCurrentThreadId();

        [DllImport("kernel32.dll", SetLastError = true)]
        internal static extern SafeWaitHandle OpenProcess(
            uint desiredAccess,
            [MarshalAs(UnmanagedType.Bool)] bool inherit,
            uint processId);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetProcessTimes(
            SafeWaitHandle process,
            out FileTime creation,
            out FileTime exit,
            out FileTime kernel,
            out FileTime user);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool ProcessIdToSessionId(uint processId, out uint sessionId);

        [DllImport("advapi32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool OpenProcessToken(
            SafeWaitHandle process,
            uint desiredAccess,
            out SafeWaitHandle token);

        [DllImport("advapi32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetTokenInformation(
            SafeWaitHandle token,
            int informationClass,
            IntPtr information,
            uint informationLength,
            out uint returnLength);

        [DllImport("advapi32.dll", SetLastError = true)]
        internal static extern IntPtr GetSidSubAuthorityCount(IntPtr sid);

        [DllImport("advapi32.dll", SetLastError = true)]
        internal static extern IntPtr GetSidSubAuthority(IntPtr sid, uint subAuthority);

        [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool ConvertSidToStringSidW(IntPtr sid, out IntPtr text);

        [DllImport("kernel32.dll")]
        internal static extern IntPtr LocalFree(IntPtr memory);

        [DllImport("user32.dll", SetLastError = true)]
        internal static extern IntPtr GetThreadDesktop(uint threadId);

        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetUserObjectInformationW(
            IntPtr handle,
            int index,
            out int information,
            uint informationLength,
            out uint returnLength);

        [DllImport("user32.dll", SetLastError = true)]
        internal static extern IntPtr OpenInputDesktop(
            uint flags,
            [MarshalAs(UnmanagedType.Bool)] bool inherit,
            uint desiredAccess);

        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool CloseDesktop(IntPtr desktop);

        [DllImport("user32.dll")]
        internal static extern IntPtr GetForegroundWindow();

        [DllImport("user32.dll", SetLastError = true)]
        internal static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool IsWindow(IntPtr hwnd);

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool IsWindowVisible(IntPtr hwnd);

        [DllImport("user32.dll")]
        internal static extern IntPtr GetAncestor(IntPtr hwnd, uint flags);

        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetGUIThreadInfo(uint threadId, ref GuiThreadInfo info);

        [DllImport("user32.dll")]
        internal static extern int GetSystemMetrics(int index);

        [DllImport("user32.dll")]
        internal static extern short GetAsyncKeyState(int key);
    }
}
