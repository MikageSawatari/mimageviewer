using System;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    internal enum ButtonNormalSendEdge
    {
        Down,
        Up,
    }

    internal enum ButtonSendPermission
    {
        Allowed,
        WrongGesture,
        WrongPhase,
        ProcessExited,
        DeadlineExpired,
    }

    [Flags]
    internal enum ButtonPressedState : uint
    {
        None = 0,
        Left = 1 << 0,
        Right = 1 << 1,
        Middle = 1 << 2,
        X1 = 1 << 3,
        X2 = 1 << 4,
        Shift = 1 << 5,
        Control = 1 << 6,
        Alt = 1 << 7,
        LeftWindows = 1 << 8,
        RightWindows = 1 << 9,
    }

    internal sealed class ButtonInputStateFrame
    {
        internal readonly bool ButtonsSwapped;
        internal readonly ButtonPressedState Pressed;

        internal ButtonInputStateFrame(bool buttonsSwapped, ButtonPressedState pressed)
        {
            ButtonsSwapped = buttonsSwapped;
            Pressed = pressed;
        }

        internal bool EqualsExact(ButtonInputStateFrame other)
        {
            return other != null
                && ButtonsSwapped == other.ButtonsSwapped
                && Pressed == other.Pressed;
        }
    }

    internal sealed class ButtonScreenPoint
    {
        internal readonly int X;
        internal readonly int Y;

        internal ButtonScreenPoint(int x, int y)
        {
            X = x;
            Y = y;
        }
    }

    internal sealed class ButtonNormalSendProbe
    {
        internal readonly ButtonNormalFrame Before;
        internal readonly ButtonNormalFrame After;
        internal readonly ButtonScreenPoint CursorBefore;
        internal readonly ButtonScreenPoint CursorAfter;
        internal readonly ButtonInputStateFrame StateBefore;
        internal readonly ButtonInputStateFrame StateAfter;
        internal readonly ButtonPointTolerance ScreenPointTolerance;

        internal ButtonNormalSendProbe(
            ButtonNormalFrame before,
            ButtonNormalFrame after,
            ButtonScreenPoint cursorBefore,
            ButtonScreenPoint cursorAfter,
            ButtonInputStateFrame stateBefore,
            ButtonInputStateFrame stateAfter,
            ButtonPointTolerance screenPointTolerance)
        {
            Before = before;
            After = after;
            CursorBefore = cursorBefore;
            CursorAfter = cursorAfter;
            StateBefore = stateBefore;
            StateAfter = stateAfter;
            ScreenPointTolerance = screenPointTolerance;
        }
    }

    internal sealed class ButtonNormalSendRequest
    {
        internal readonly ButtonNormalSendEdge Edge;
        internal readonly ButtonGestureTuple Gesture;
        internal readonly ulong Tag;
        internal readonly ulong DeadlineTick;

        internal ButtonNormalSendRequest(
            ButtonNormalSendEdge edge,
            ButtonGestureTuple gesture,
            ulong tag)
        {
            if (gesture == null) throw new ArgumentNullException("gesture");
            Edge = edge;
            Gesture = gesture;
            Tag = tag;
            DeadlineTick = gesture.DeadlineTick;
        }
    }

    internal sealed class ButtonCleanupSendRequest
    {
        internal readonly ButtonGestureTuple Gesture;
        internal readonly ulong Tag;

        internal ButtonCleanupSendRequest(ButtonGestureTuple gesture, ulong tag)
        {
            if (gesture == null) throw new ArgumentNullException("gesture");
            Gesture = gesture;
            Tag = tag;
        }
    }

    internal abstract class ButtonSendAttempt
    {
        internal abstract bool WasCalled { get; }
        internal abstract int Inserted { get; }
        internal abstract string FailureDetail { get; }
        internal abstract string PostCallFault { get; }
    }

    internal sealed class ButtonSendRefused : ButtonSendAttempt
    {
        private readonly string reason;

        internal ButtonSendRefused(string reason)
        {
            this.reason = String.IsNullOrWhiteSpace(reason)
                ? "native button send was refused"
                : reason;
        }

        internal override bool WasCalled { get { return false; } }
        internal override int Inserted { get { return 0; } }
        internal override string FailureDetail { get { return reason; } }
        internal override string PostCallFault { get { return null; } }
    }

    internal sealed class ButtonSendCalled : ButtonSendAttempt
    {
        private readonly int inserted;
        private readonly int lastError;
        private readonly string postCallFault;

        internal ButtonSendCalled(int inserted, int lastError, string postCallFault)
        {
            if (inserted < 0 || inserted > 1)
                throw new ArgumentOutOfRangeException("inserted");
            this.inserted = inserted;
            this.lastError = lastError;
            this.postCallFault = String.IsNullOrWhiteSpace(postCallFault)
                ? null
                : postCallFault;
        }

        internal int LastError { get { return lastError; } }
        internal override bool WasCalled { get { return true; } }
        internal override int Inserted { get { return inserted; } }
        internal override string FailureDetail
        {
            get
            {
                return inserted == 1
                    ? null
                    : "SendInput inserted " + inserted + " of 1; last_error=" + lastError;
            }
        }
        internal override string PostCallFault { get { return postCallFault; } }
    }

    internal sealed class ButtonNativeInsertion
    {
        internal readonly int Inserted;
        internal readonly int LastError;

        internal ButtonNativeInsertion(int inserted, int lastError)
        {
            if (inserted < 0 || inserted > 1)
                throw new ArgumentOutOfRangeException("inserted");
            Inserted = inserted;
            LastError = lastError;
        }
    }

    internal interface IButtonNormalSendLease
    {
        ButtonNormalSendProbe Probe { get; }
        string Finish();
    }

    // A cleanup Up is intentionally independent from the historical target,
    // cursor, foreground and capture.  It still has to be issued by a helper
    // thread which is attached to the current input desktop, and that desktop
    // handle must remain certified until SendInput has returned.
    internal interface IButtonCleanupSendLease
    {
        string Finish();
    }

    internal interface IButtonInputFactSource
    {
        IButtonNormalSendLease OpenNormalSend(ButtonNormalSendRequest request);
        IButtonCleanupSendLease OpenCleanupSend();
    }

    internal interface IButtonNativeInserter
    {
        ButtonNativeInsertion SendOne(ButtonNormalSendEdge edge, uint tag);
    }

    internal interface IButtonInputBackend
    {
        ButtonSendAttempt SendNormal(
            ButtonNormalSendRequest request,
            Func<ButtonSendPermission> permission);
        ButtonSendAttempt SendOwnedCleanupUp(ButtonCleanupSendRequest request);
    }

    internal static class ButtonNormalSendPolicy
    {
        private const ButtonPressedState OtherButtonsAndModifiers =
            ButtonPressedState.Right
            | ButtonPressedState.Middle
            | ButtonPressedState.X1
            | ButtonPressedState.X2
            | ButtonPressedState.Shift
            | ButtonPressedState.Control
            | ButtonPressedState.Alt
            | ButtonPressedState.LeftWindows
            | ButtonPressedState.RightWindows;

        internal static string Validate(
            ButtonNormalSendRequest request,
            ButtonNormalSendProbe probe)
        {
            if (request == null || request.Gesture == null)
                return "normal send request is missing";
            if (probe == null || probe.Before == null || probe.After == null)
                return "normal send probe is missing";
            if (probe.CursorBefore == null || probe.CursorAfter == null
                || probe.StateBefore == null || probe.StateAfter == null
                || probe.ScreenPointTolerance == null)
            {
                return "normal cursor or input-state probe is missing";
            }
            if (!probe.Before.EqualsExact(probe.After))
                return "normal desktop, foreground, receiver, or capture changed during preflight";
            ButtonGestureTuple gesture = request.Gesture;
            ButtonAccessFrame access = probe.Before.Access;
            ButtonReceiverFrame receiver = probe.Before.Receiver;
            if (access.Foreground.Hwnd != gesture.Target.ParentHwnd
                || access.Foreground.ProcessId != gesture.AppProcessId
                || access.Foreground.ProcessCreationIdentity != gesture.AppCreationIdentity)
            {
                return "normal foreground does not match the immutable application owner";
            }
            if (receiver.InputHwnd != gesture.Target.InputHwnd
                || receiver.ParentHwnd != gesture.Target.ParentHwnd
                || receiver.ProcessId != gesture.AppProcessId
                || receiver.ThreadId == 0)
            {
                return "normal receiver does not match the immutable input window";
            }
            ulong expectedCapture = request.Edge == ButtonNormalSendEdge.Down
                ? 0UL
                : gesture.Target.InputHwnd;
            if (receiver.CaptureHwnd != expectedCapture)
                return "normal receiver capture does not match the requested edge";
            if (!probe.StateBefore.EqualsExact(probe.StateAfter))
                return "physical input state changed during preflight";
            if (access.ButtonsSwapped
                || probe.StateBefore.ButtonsSwapped
                || probe.StateAfter.ButtonsSwapped)
            {
                return "swapped mouse buttons are unsupported by this diagnostic slice";
            }
            if ((probe.StateBefore.Pressed & OtherButtonsAndModifiers) != 0)
                return "another mouse button or modifier is held";
            bool leftDown = (probe.StateBefore.Pressed & ButtonPressedState.Left) != 0;
            if (leftDown != (request.Edge == ButtonNormalSendEdge.Up))
                return "physical left-button level does not match the requested edge";
            if (!WithinTolerance(
                    probe.CursorBefore.X,
                    gesture.Target.ScreenX,
                    probe.ScreenPointTolerance.X)
                || !WithinTolerance(
                    probe.CursorBefore.Y,
                    gesture.Target.ScreenY,
                    probe.ScreenPointTolerance.Y)
                || !WithinTolerance(
                    probe.CursorAfter.X,
                    gesture.Target.ScreenX,
                    probe.ScreenPointTolerance.X)
                || !WithinTolerance(
                    probe.CursorAfter.Y,
                    gesture.Target.ScreenY,
                    probe.ScreenPointTolerance.Y))
            {
                return "current cursor does not match the immutable physical screen point";
            }
            return null;
        }

        private static bool WithinTolerance(int actual, int expected, int tolerance)
        {
            long delta = (long)actual - expected;
            if (delta < 0) delta = -delta;
            return delta <= tolerance;
        }
    }

    internal sealed class Win32ButtonInputBackend : IButtonInputBackend
    {
        private readonly int ownerThreadId;
        private readonly IButtonInputFactSource facts;
        private readonly IButtonNativeInserter inserter;

        internal Win32ButtonInputBackend(
            IButtonInputFactSource facts,
            IButtonNativeInserter inserter)
        {
            if (facts == null) throw new ArgumentNullException("facts");
            if (inserter == null) throw new ArgumentNullException("inserter");
            ownerThreadId = System.Threading.Thread.CurrentThread.ManagedThreadId;
            this.facts = facts;
            this.inserter = inserter;
        }

        public ButtonSendAttempt SendNormal(
            ButtonNormalSendRequest request,
            Func<ButtonSendPermission> permission)
        {
            RequireOwnerThread();
            if (request == null || permission == null)
                return new ButtonSendRefused("normal send request or permission is missing");
            uint tag;
            string tagFailure = CheckedTag(request.Tag, out tag);
            if (tagFailure != null) return new ButtonSendRefused(tagFailure);
            if ((request.Edge == ButtonNormalSendEdge.Down && request.Tag != request.Gesture.DownTag)
                || (request.Edge == ButtonNormalSendEdge.Up && request.Tag != request.Gesture.UpTag))
            {
                return new ButtonSendRefused("normal input tag does not match the immutable gesture edge");
            }

            IButtonNormalSendLease lease = null;
            ButtonNativeInsertion insertion = null;
            string refusal = null;
            string postCallFault = null;
            try
            {
                lease = facts.OpenNormalSend(request);
                refusal = ButtonNormalSendPolicy.Validate(request, lease.Probe);
                if (refusal == null)
                {
                    ButtonSendPermission allowed = permission();
                    if (allowed != ButtonSendPermission.Allowed)
                        refusal = "owner refused normal send: " + allowed;
                }
                if (refusal == null)
                    insertion = inserter.SendOne(request.Edge, tag);
            }
            catch (Exception error)
            {
                if (insertion != null)
                    postCallFault = "native send returned before exception: " + error.Message;
                else
                    refusal = "normal native preflight failed: " + error.Message;
            }
            finally
            {
                if (lease != null)
                {
                    try
                    {
                        postCallFault = JoinFaults(postCallFault, lease.Finish());
                    }
                    catch (Exception error)
                    {
                        postCallFault = JoinFaults(
                            postCallFault,
                            "normal native scope cleanup threw: " + error.Message);
                    }
                }
            }
            if (insertion == null)
            {
                return new ButtonSendRefused(JoinFaults(refusal, postCallFault));
            }
            return new ButtonSendCalled(
                insertion.Inserted,
                insertion.LastError,
                postCallFault);
        }

        public ButtonSendAttempt SendOwnedCleanupUp(ButtonCleanupSendRequest request)
        {
            RequireOwnerThread();
            if (request == null)
                return new ButtonSendRefused("cleanup send request is missing");
            if (request.Tag != request.Gesture.UpTag)
                return new ButtonSendRefused("cleanup tag does not match the immutable Up tag");
            uint tag;
            string tagFailure = CheckedTag(request.Tag, out tag);
            if (tagFailure != null) return new ButtonSendRefused(tagFailure);
            IButtonCleanupSendLease lease = null;
            ButtonNativeInsertion insertion = null;
            string refusal = null;
            string postCallFault = null;
            try
            {
                lease = facts.OpenCleanupSend();
                insertion = inserter.SendOne(ButtonNormalSendEdge.Up, tag);
            }
            catch (Exception error)
            {
                if (insertion != null)
                    postCallFault = "cleanup native send returned before exception: " + error.Message;
                else
                    refusal = "cleanup native preflight or insertion failed before a count was returned: "
                        + error.Message;
            }
            finally
            {
                if (lease != null)
                {
                    try
                    {
                        postCallFault = JoinFaults(postCallFault, lease.Finish());
                    }
                    catch (Exception error)
                    {
                        postCallFault = JoinFaults(
                            postCallFault,
                            "cleanup native scope cleanup threw: " + error.Message);
                    }
                }
            }
            if (insertion == null)
                return new ButtonSendRefused(JoinFaults(refusal, postCallFault));
            return new ButtonSendCalled(
                insertion.Inserted,
                insertion.LastError,
                postCallFault);
        }

        private static string CheckedTag(ulong value, out uint tag)
        {
            tag = 0;
            if (value == 0 || value > UInt32.MaxValue)
                return "button input tag is not a nonzero 32-bit value";
            tag = unchecked((uint)value);
            if ((ulong)new UIntPtr(tag).ToUInt64() != value)
                return "button input tag did not round-trip through ULONG_PTR";
            return null;
        }

        private static string JoinFaults(string first, string second)
        {
            if (String.IsNullOrWhiteSpace(first)) return second;
            if (String.IsNullOrWhiteSpace(second)) return first;
            return first + "; " + second;
        }

        private void RequireOwnerThread()
        {
            if (System.Threading.Thread.CurrentThread.ManagedThreadId != ownerThreadId)
                throw new InvalidOperationException("button input backend must run on its owner thread");
        }
    }
}
