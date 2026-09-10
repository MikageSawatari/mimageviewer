using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    internal sealed class NativeButtonInputFactSource : IButtonInputFactSource
    {
        private readonly NativeButtonObservationPlatform observations;

        internal NativeButtonInputFactSource(NativeButtonObservationPlatform observations)
        {
            if (observations == null) throw new ArgumentNullException("observations");
            this.observations = observations;
        }

        public IButtonNormalSendLease OpenNormalSend(ButtonNormalSendRequest request)
        {
            return new NativeButtonNormalSendLease(observations, request);
        }

        public IButtonCleanupSendLease OpenCleanupSend()
        {
            return new NativeButtonCleanupSendLease(observations);
        }
    }

    internal sealed class NativeButtonCleanupSendLease : IButtonCleanupSendLease
    {
        private IntPtr inputDesktop;
        private bool finished;

        internal NativeButtonCleanupSendLease(NativeButtonObservationPlatform observations)
        {
            if (observations == null) throw new ArgumentNullException("observations");
            try
            {
                inputDesktop = NativeButtonObservationPlatform.OpenCertifiedInputDesktop();
                observations.ValidateInputDesktopOnly(inputDesktop);
            }
            catch
            {
                Finish();
                throw;
            }
        }

        public string Finish()
        {
            if (finished) return null;
            finished = true;
            if (inputDesktop == IntPtr.Zero) return null;
            bool closed = ButtonObservationNative.CloseDesktop(inputDesktop);
            int error = closed ? 0 : Marshal.GetLastWin32Error();
            inputDesktop = IntPtr.Zero;
            return closed
                ? null
                : "CloseDesktop failed after cleanup button operation; error=" + error;
        }
    }

    internal sealed class NativeButtonNormalSendLease : IButtonNormalSendLease
    {
        private readonly NativeButtonObservationPlatform observations;
        private IntPtr inputDesktop;
        private IntPtr previousDpiContext;
        private bool finished;
        private readonly ButtonNormalSendProbe probe;

        internal NativeButtonNormalSendLease(
            NativeButtonObservationPlatform observations,
            ButtonNormalSendRequest request)
        {
            if (observations == null) throw new ArgumentNullException("observations");
            if (request == null || request.Gesture == null)
                throw new ArgumentNullException("request");
            this.observations = observations;
            previousDpiContext = ButtonInputNative.SetThreadDpiAwarenessContext(
                ButtonInputNative.DpiAwarenessContextPerMonitorAwareV2);
            if (previousDpiContext == IntPtr.Zero)
                throw new Win32Exception(Marshal.GetLastWin32Error(),
                    "SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2) failed");
            try
            {
                inputDesktop = NativeButtonObservationPlatform.OpenCertifiedInputDesktop();
                ButtonCaptureExpectation capture = request.Edge == ButtonNormalSendEdge.Down
                    ? ButtonCaptureExpectation.Released
                    : ButtonCaptureExpectation.InputWindow;
                ButtonNormalFrame before = observations.CaptureNormalFrame(
                    request.Gesture,
                    capture,
                    inputDesktop);
                ButtonScreenPoint cursorBefore = ReadCursor();
                ButtonInputStateFrame stateBefore = ReadInputState();
                ButtonInputStateFrame stateAfter = ReadInputState();
                ButtonScreenPoint cursorAfter = ReadCursor();
                ButtonNormalFrame after = observations.CaptureNormalFrame(
                    request.Gesture,
                    capture,
                    inputDesktop);
                probe = new ButtonNormalSendProbe(
                    before,
                    after,
                    cursorBefore,
                    cursorAfter,
                    stateBefore,
                    stateAfter,
                    NativeButtonObservationPlatform.ReadVirtualDesktopPointTolerance());
            }
            catch
            {
                Finish();
                throw;
            }
        }

        public ButtonNormalSendProbe Probe { get { return probe; } }

        public string Finish()
        {
            if (finished) return null;
            finished = true;
            string fault = null;
            if (inputDesktop != IntPtr.Zero)
            {
                if (!ButtonObservationNative.CloseDesktop(inputDesktop))
                {
                    fault = "CloseDesktop failed after native button operation; error="
                        + Marshal.GetLastWin32Error();
                }
                inputDesktop = IntPtr.Zero;
            }
            if (previousDpiContext != IntPtr.Zero)
            {
                if (ButtonInputNative.SetThreadDpiAwarenessContext(previousDpiContext)
                    == IntPtr.Zero)
                {
                    string restore = "SetThreadDpiAwarenessContext restore failed; error="
                        + Marshal.GetLastWin32Error();
                    fault = String.IsNullOrWhiteSpace(fault) ? restore : fault + "; " + restore;
                }
                previousDpiContext = IntPtr.Zero;
            }
            return fault;
        }

        internal static ButtonScreenPoint ReadCursor()
        {
            ButtonInputNative.Point point;
            if (!ButtonInputNative.GetCursorPos(out point))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "GetCursorPos failed");
            return new ButtonScreenPoint(point.X, point.Y);
        }

        internal static ButtonInputStateFrame ReadInputState()
        {
            ButtonPressedState pressed = ButtonPressedState.None;
            AddIfPressed(ref pressed, ButtonPressedState.Left, 0x01);
            AddIfPressed(ref pressed, ButtonPressedState.Right, 0x02);
            AddIfPressed(ref pressed, ButtonPressedState.Middle, 0x04);
            AddIfPressed(ref pressed, ButtonPressedState.X1, 0x05);
            AddIfPressed(ref pressed, ButtonPressedState.X2, 0x06);
            AddIfPressed(ref pressed, ButtonPressedState.Shift, 0x10);
            AddIfPressed(ref pressed, ButtonPressedState.Control, 0x11);
            AddIfPressed(ref pressed, ButtonPressedState.Alt, 0x12);
            AddIfPressed(ref pressed, ButtonPressedState.LeftWindows, 0x5B);
            AddIfPressed(ref pressed, ButtonPressedState.RightWindows, 0x5C);
            return new ButtonInputStateFrame(
                ButtonObservationNative.GetSystemMetrics(23) != 0,
                pressed);
        }

        private static void AddIfPressed(
            ref ButtonPressedState state,
            ButtonPressedState bit,
            int virtualKey)
        {
            short value = ButtonObservationNative.GetAsyncKeyState(virtualKey);
            if ((value & unchecked((short)0x8000)) != 0) state |= bit;
        }
    }

    // This probe performs only read-only access checks.  It never constructs or
    // invokes NativeButtonEventInserter and does not identify any target window.
    internal static class NativeButtonInputEnvironmentDescription
    {
        internal static string Describe()
        {
            NativeButtonObservationPlatform observations;
            try
            {
                observations = new NativeButtonObservationPlatform();
            }
            catch (Exception error)
            {
                return "input_backend_available=false,reason=identity_"
                    + error.GetType().Name;
            }

            IntPtr previousDpi = IntPtr.Zero;
            IntPtr inputDesktop = IntPtr.Zero;
            string result;
            string cleanupFault = null;
            try
            {
                previousDpi = ButtonInputNative.SetThreadDpiAwarenessContext(
                    ButtonInputNative.DpiAwarenessContextPerMonitorAwareV2);
                if (previousDpi == IntPtr.Zero)
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                inputDesktop = NativeButtonObservationPlatform.OpenCertifiedInputDesktop();
                ButtonAccessFrame before = observations.CaptureAccessFrame(inputDesktop);
                NativeButtonNormalSendLease.ReadCursor();
                ButtonInputStateFrame stateBefore = NativeButtonNormalSendLease.ReadInputState();
                ButtonInputStateFrame stateAfter = NativeButtonNormalSendLease.ReadInputState();
                NativeButtonNormalSendLease.ReadCursor();
                ButtonAccessFrame after = observations.CaptureAccessFrame(inputDesktop);
                ButtonPointTolerance tolerance =
                    NativeButtonObservationPlatform.ReadVirtualDesktopPointTolerance();
                bool accessStable = before.EqualsExact(after);
                bool stateStable = stateBefore.EqualsExact(stateAfter);
                bool mappingSupported = !before.ButtonsSwapped
                    && !stateBefore.ButtonsSwapped
                    && !stateAfter.ButtonsSwapped;
                result = "input_backend_available=" + (accessStable && stateStable)
                    + ",architecture_bits=" + (IntPtr.Size * 8)
                    + ",access_stable=" + accessStable
                    + ",input_state_stable=" + stateStable
                    + ",mapping_supported=" + mappingSupported
                    + ",any_pressed=" + (stateBefore.Pressed != ButtonPressedState.None)
                    + ",cursor_query=true,tolerance_valid="
                    + (tolerance.X >= 0 && tolerance.Y >= 0);
            }
            catch (ButtonObservationUnavailableException error)
            {
                result = "input_backend_available=false,reason=" + error.Kind;
            }
            catch (Exception error)
            {
                result = "input_backend_available=false,reason="
                    + error.GetType().Name;
            }
            finally
            {
                if (inputDesktop != IntPtr.Zero
                    && !ButtonObservationNative.CloseDesktop(inputDesktop))
                {
                    cleanupFault = "close_desktop";
                }
                if (previousDpi != IntPtr.Zero
                    && ButtonInputNative.SetThreadDpiAwarenessContext(previousDpi)
                        == IntPtr.Zero)
                {
                    cleanupFault = String.IsNullOrWhiteSpace(cleanupFault)
                        ? "restore_dpi"
                        : cleanupFault + "+restore_dpi";
                }
            }
            if (!String.IsNullOrWhiteSpace(cleanupFault))
                result += ",cleanup_fault=" + cleanupFault;
            result += ",cleanup_ok=" + String.IsNullOrWhiteSpace(cleanupFault);
            return result;
        }
    }

    internal sealed class NativeButtonEventInserter : IButtonNativeInserter
    {
        internal static readonly int InputSize = Marshal.SizeOf(typeof(ButtonInputNative.Input));

        public ButtonNativeInsertion SendOne(ButtonNormalSendEdge edge, uint tag)
        {
            ButtonInputNative.Input input = BuildInput(edge, tag);
            ButtonInputNative.Input[] one = new ButtonInputNative.Input[] { input };
            uint inserted = ButtonInputNative.SendInput(1, one, InputSize);
            int residualLastError = Marshal.GetLastWin32Error();
            return new ButtonNativeInsertion(unchecked((int)inserted), residualLastError);
        }

        internal static ButtonInputNative.Input BuildInput(ButtonNormalSendEdge edge, uint tag)
        {
            ButtonInputNative.MouseInput mouse = new ButtonInputNative.MouseInput();
            mouse.Flags = edge == ButtonNormalSendEdge.Down
                ? ButtonInputNative.MouseEventLeftDown
                : ButtonInputNative.MouseEventLeftUp;
            mouse.ExtraInfo = new UIntPtr(tag);
            ButtonInputNative.InputUnion data = new ButtonInputNative.InputUnion();
            data.Mouse = mouse;
            ButtonInputNative.Input input = new ButtonInputNative.Input();
            input.Type = ButtonInputNative.InputMouse;
            input.Data = data;
            return input;
        }
    }

    internal static class ButtonInputNative
    {
        internal const uint InputMouse = 0;
        internal const uint MouseEventLeftDown = 0x0002;
        internal const uint MouseEventLeftUp = 0x0004;
        internal static readonly IntPtr DpiAwarenessContextPerMonitorAwareV2 = new IntPtr(-4);

        [StructLayout(LayoutKind.Sequential)]
        internal struct Point
        {
            internal int X;
            internal int Y;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct MouseInput
        {
            internal int Dx;
            internal int Dy;
            internal uint MouseData;
            internal uint Flags;
            internal uint Time;
            internal UIntPtr ExtraInfo;
        }

        [StructLayout(LayoutKind.Explicit)]
        internal struct InputUnion
        {
            [FieldOffset(0)]
            internal MouseInput Mouse;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct Input
        {
            internal uint Type;
            internal InputUnion Data;
        }

        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetCursorPos(out Point point);

        [DllImport("user32.dll", SetLastError = true)]
        internal static extern IntPtr SetThreadDpiAwarenessContext(IntPtr dpiContext);

        [DllImport("user32.dll", SetLastError = true)]
        internal static extern uint SendInput(
            uint inputCount,
            [In] Input[] inputs,
            int inputSize);
    }
}
