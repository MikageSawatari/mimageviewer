using System;
using System.Threading;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    internal enum ButtonObservationScope
    {
        Normal,
        OwnedCleanup,
    }

    internal enum ButtonTaggedEdgeKind
    {
        Down,
        Up,
    }

    internal enum ButtonPhysicalLevel
    {
        Up,
        Down,
    }

    internal enum ButtonCaptureExpectation
    {
        InputWindow,
        Released,
    }

    internal enum ButtonObservationFailureKind
    {
        WrongOwnerThread,
        WrongGesture,
        WrongTag,
        WrongReceiver,
        WrongScope,
        MissingHistoricalDown,
        UpNotInserted,
        MappingUnsupported,
        DesktopUnavailable,
        DesktopChanged,
        ForegroundUnavailable,
        ForegroundChanged,
        TokenAccessUnavailable,
        ReceiverUnavailable,
        CaptureMismatch,
        PointMismatch,
        PhysicalLevelMismatch,
    }

    internal sealed class ButtonObservationUnavailableException : Exception
    {
        internal ButtonObservationUnavailableException(
            ButtonObservationFailureKind kind,
            string detail)
            : base(detail)
        {
            Kind = kind;
        }

        internal ButtonObservationFailureKind Kind { get; private set; }
    }

    internal sealed class TaggedButtonDelivery
    {
        internal readonly ulong GestureId;
        internal readonly ulong Tag;
        internal readonly ButtonTaggedEdgeKind Edge;
        internal readonly ulong ReceiverHwnd;
        internal readonly uint ReceiverProcessId;
        internal readonly uint ReceiverThreadId;
        internal readonly int ActualClientX;
        internal readonly int ActualClientY;
        internal readonly int ActualScreenX;
        internal readonly int ActualScreenY;

        internal TaggedButtonDelivery(
            ulong gestureId,
            ulong tag,
            ButtonTaggedEdgeKind edge,
            ulong receiverHwnd,
            uint receiverProcessId,
            uint receiverThreadId,
            int actualClientX,
            int actualClientY,
            int actualScreenX,
            int actualScreenY)
        {
            GestureId = gestureId;
            Tag = tag;
            Edge = edge;
            ReceiverHwnd = receiverHwnd;
            ReceiverProcessId = receiverProcessId;
            ReceiverThreadId = receiverThreadId;
            ActualClientX = actualClientX;
            ActualClientY = actualClientY;
            ActualScreenX = actualScreenX;
            ActualScreenY = actualScreenY;
        }
    }

    internal abstract class ButtonUpInsertion
    {
        protected ButtonUpInsertion(
            ButtonObservationScope scope,
            ulong gestureId,
            ulong tag,
            int inserted)
        {
            Scope = scope;
            GestureId = gestureId;
            Tag = tag;
            Inserted = inserted;
        }

        internal ButtonObservationScope Scope { get; private set; }
        internal ulong GestureId { get; private set; }
        internal ulong Tag { get; private set; }
        internal int Inserted { get; private set; }
    }

    internal sealed class NormalButtonUpInsertion : ButtonUpInsertion
    {
        internal NormalButtonUpInsertion(ulong gestureId, ulong tag, int inserted)
            : base(ButtonObservationScope.Normal, gestureId, tag, inserted)
        {
        }
    }

    internal sealed class OwnedCleanupButtonUpInsertion : ButtonUpInsertion
    {
        internal OwnedCleanupButtonUpInsertion(ulong gestureId, ulong tag, int inserted)
            : base(ButtonObservationScope.OwnedCleanup, gestureId, tag, inserted)
        {
        }
    }

    internal sealed class ButtonForegroundIdentity
    {
        internal readonly ulong Hwnd;
        internal readonly uint ProcessId;
        internal readonly uint ThreadId;
        internal readonly ulong ProcessCreationIdentity;
        internal readonly uint SessionId;
        internal readonly string UserSid;
        internal readonly uint IntegrityRid;

        internal ButtonForegroundIdentity(
            ulong hwnd,
            uint processId,
            uint threadId,
            ulong processCreationIdentity,
            uint sessionId,
            string userSid,
            uint integrityRid)
        {
            Hwnd = hwnd;
            ProcessId = processId;
            ThreadId = threadId;
            ProcessCreationIdentity = processCreationIdentity;
            SessionId = sessionId;
            UserSid = userSid;
            IntegrityRid = integrityRid;
        }

        internal bool EqualsExact(ButtonForegroundIdentity other)
        {
            return other != null
                && Hwnd == other.Hwnd
                && ProcessId == other.ProcessId
                && ThreadId == other.ThreadId
                && ProcessCreationIdentity == other.ProcessCreationIdentity
                && SessionId == other.SessionId
                && String.Equals(UserSid, other.UserSid, StringComparison.Ordinal)
                && IntegrityRid == other.IntegrityRid;
        }
    }

    // A frame exists only after the platform facade has proved that its caller's
    // desktop is the input desktop, that a separately opened input desktop grants
    // READOBJECTS|HOOKCONTROL, and that the foreground token can be read without
    // crossing user/session/integrity boundaries.  Foreground is an access context;
    // it is never a replacement input target.
    internal sealed class ButtonAccessFrame
    {
        internal readonly ulong ThreadDesktopIdentity;
        internal readonly ButtonForegroundIdentity Foreground;
        internal readonly bool ButtonsSwapped;

        internal ButtonAccessFrame(
            ulong threadDesktopIdentity,
            ButtonForegroundIdentity foreground,
            bool buttonsSwapped)
        {
            if (threadDesktopIdentity == 0)
                throw new ArgumentOutOfRangeException("threadDesktopIdentity");
            if (foreground == null) throw new ArgumentNullException("foreground");
            ThreadDesktopIdentity = threadDesktopIdentity;
            Foreground = foreground;
            ButtonsSwapped = buttonsSwapped;
        }

        internal bool EqualsExact(ButtonAccessFrame other)
        {
            return other != null
                && ThreadDesktopIdentity == other.ThreadDesktopIdentity
                && ButtonsSwapped == other.ButtonsSwapped
                && Foreground.EqualsExact(other.Foreground);
        }
    }

    internal sealed class ButtonReceiverFrame
    {
        internal readonly ulong InputHwnd;
        internal readonly ulong ParentHwnd;
        internal readonly uint ProcessId;
        internal readonly uint ThreadId;
        internal readonly ulong CaptureHwnd;

        internal ButtonReceiverFrame(
            ulong inputHwnd,
            ulong parentHwnd,
            uint processId,
            uint threadId,
            ulong captureHwnd)
        {
            InputHwnd = inputHwnd;
            ParentHwnd = parentHwnd;
            ProcessId = processId;
            ThreadId = threadId;
            CaptureHwnd = captureHwnd;
        }

        internal bool EqualsExact(ButtonReceiverFrame other)
        {
            return other != null
                && InputHwnd == other.InputHwnd
                && ParentHwnd == other.ParentHwnd
                && ProcessId == other.ProcessId
                && ThreadId == other.ThreadId
                && CaptureHwnd == other.CaptureHwnd;
        }
    }

    internal sealed class ButtonNormalFrame
    {
        internal readonly ButtonAccessFrame Access;
        internal readonly ButtonReceiverFrame Receiver;

        internal ButtonNormalFrame(ButtonAccessFrame access, ButtonReceiverFrame receiver)
        {
            if (access == null) throw new ArgumentNullException("access");
            if (receiver == null) throw new ArgumentNullException("receiver");
            Access = access;
            Receiver = receiver;
        }

        internal bool EqualsExact(ButtonNormalFrame other)
        {
            return other != null
                && Access.EqualsExact(other.Access)
                && Receiver.EqualsExact(other.Receiver);
        }
    }

    internal sealed class ButtonNormalLevelProbe
    {
        internal readonly ButtonNormalFrame Before;
        internal readonly ButtonPhysicalLevel Level;
        internal readonly ButtonNormalFrame After;
        internal readonly ButtonPointTolerance ScreenPointTolerance;

        internal ButtonNormalLevelProbe(
            ButtonNormalFrame before,
            ButtonPhysicalLevel level,
            ButtonNormalFrame after,
            ButtonPointTolerance screenPointTolerance)
        {
            Before = before;
            Level = level;
            After = after;
            ScreenPointTolerance = screenPointTolerance;
        }
    }

    internal sealed class ButtonPointTolerance
    {
        internal readonly int X;
        internal readonly int Y;

        internal ButtonPointTolerance(int x, int y)
        {
            if (x < 0) throw new ArgumentOutOfRangeException("x");
            if (y < 0) throw new ArgumentOutOfRangeException("y");
            X = x;
            Y = y;
        }
    }

    internal sealed class ButtonAccessLevelProbe
    {
        internal readonly ButtonAccessFrame Before;
        internal readonly ButtonPhysicalLevel Level;
        internal readonly ButtonAccessFrame After;

        internal ButtonAccessLevelProbe(
            ButtonAccessFrame before,
            ButtonPhysicalLevel level,
            ButtonAccessFrame after)
        {
            Before = before;
            Level = level;
            After = after;
        }
    }

    internal interface IButtonObservationPlatform
    {
        ButtonNormalLevelProbe ProbeNormalLevel(
            ButtonGestureTuple gesture,
            ButtonCaptureExpectation captureExpectation);
        ButtonAccessLevelProbe ProbeAccessLevel();
    }

    internal static class ButtonObservationEnvironmentDescription
    {
        internal static string Describe(ButtonAccessLevelProbe frame)
        {
            if (frame == null || frame.Before == null || frame.After == null)
                return "available=false,reason=AccessContextUnavailable";
            if (frame.Before.ThreadDesktopIdentity != frame.After.ThreadDesktopIdentity)
                return "available=false,reason=" + ButtonObservationFailureKind.DesktopChanged;
            if (!frame.Before.EqualsExact(frame.After))
                return "available=false,reason=AccessContextChanged";
            return "available=true,swapped=" + frame.After.ButtonsSwapped
                + ",foreground_pid_nonzero=" + (frame.After.Foreground.ProcessId != 0)
                + ",foreground_tid_nonzero=" + (frame.After.Foreground.ThreadId != 0)
                + ",physical_left=" + frame.Level;
        }
    }

    internal sealed class ObservedButtonDownAnchor
    {
        internal readonly ButtonGestureTuple Gesture;
        internal readonly TaggedButtonDelivery Delivery;
        internal readonly ButtonNormalFrame Observation;

        internal ObservedButtonDownAnchor(
            ButtonGestureTuple gesture,
            TaggedButtonDelivery delivery,
            ButtonNormalFrame observation)
        {
            Gesture = gesture;
            Delivery = delivery;
            Observation = observation;
        }
    }

    internal abstract class ButtonDownObservationResult
    {
    }

    internal sealed class ButtonDownObserved : ButtonDownObservationResult
    {
        internal ButtonDownObserved(ObservedButtonDownAnchor anchor)
        {
            Anchor = anchor;
        }

        internal ObservedButtonDownAnchor Anchor { get; private set; }
    }

    internal sealed class ButtonDownUnconfirmed : ButtonDownObservationResult
    {
        internal ButtonDownUnconfirmed(ButtonObservationFailureKind kind, string detail)
        {
            Kind = kind;
            Detail = detail;
        }

        internal ButtonObservationFailureKind Kind { get; private set; }
        internal string Detail { get; private set; }
    }

    internal abstract class ButtonReleaseEvidence
    {
        protected ButtonReleaseEvidence(
            ButtonObservationScope scope,
            ObservedButtonDownAnchor anchor)
        {
            Scope = scope;
            Anchor = anchor;
        }

        internal ButtonObservationScope Scope { get; private set; }
        internal ObservedButtonDownAnchor Anchor { get; private set; }
    }

    internal sealed class NormalButtonReleaseEvidence : ButtonReleaseEvidence
    {
        internal readonly TaggedButtonDelivery UpDelivery;
        internal readonly ButtonNormalFrame UpObservation;

        internal NormalButtonReleaseEvidence(
            ObservedButtonDownAnchor anchor,
            TaggedButtonDelivery upDelivery,
            ButtonNormalFrame upObservation)
            : base(ButtonObservationScope.Normal, anchor)
        {
            UpDelivery = upDelivery;
            UpObservation = upObservation;
        }
    }

    internal sealed class OwnedCleanupButtonReleaseEvidence : ButtonReleaseEvidence
    {
        internal readonly ButtonAccessFrame UpObservation;

        internal OwnedCleanupButtonReleaseEvidence(
            ObservedButtonDownAnchor anchor,
            ButtonAccessFrame upObservation)
            : base(ButtonObservationScope.OwnedCleanup, anchor)
        {
            UpObservation = upObservation;
        }
    }

    internal abstract class ButtonReleaseObservationResult
    {
    }

    internal sealed class ButtonReleaseObserved : ButtonReleaseObservationResult
    {
        internal ButtonReleaseObserved(ButtonReleaseEvidence evidence)
        {
            Evidence = evidence;
        }

        internal ButtonReleaseEvidence Evidence { get; private set; }
    }

    internal sealed class ButtonReleaseUnconfirmed : ButtonReleaseObservationResult
    {
        internal ButtonReleaseUnconfirmed(ButtonObservationFailureKind kind, string detail)
        {
            Kind = kind;
            Detail = detail;
        }

        internal ButtonObservationFailureKind Kind { get; private set; }
        internal string Detail { get; private set; }
    }

    internal enum ButtonEvidenceDispatchDisposition
    {
        Accepted,
        WrongScope,
        WrongGesture,
    }

    internal static class ButtonEvidenceDispatch
    {
        internal static ButtonEvidenceDispatchDisposition Classify(
            ButtonObservationScope expectedScope,
            ulong expectedGestureId,
            ButtonReleaseEvidence evidence)
        {
            if (evidence == null || evidence.Scope != expectedScope)
                return ButtonEvidenceDispatchDisposition.WrongScope;
            if (evidence.Anchor.Gesture.GestureId != expectedGestureId)
                return ButtonEvidenceDispatchDisposition.WrongGesture;
            return ButtonEvidenceDispatchDisposition.Accepted;
        }
    }

    // This observer only creates evidence.  It neither sends input nor invokes a
    // reducer transition.  The future owner integration must dispatch Normal and
    // OwnedCleanup evidence only to their matching reducer phases.
    internal interface IButtonDeliveryObserver
    {
        ButtonDownObservationResult ObserveTaggedDown(
            ButtonGestureTuple gesture,
            TaggedButtonDelivery delivery);
        ButtonReleaseObservationResult ObserveNormalUp(
            ObservedButtonDownAnchor anchor,
            NormalButtonUpInsertion insertion,
            TaggedButtonDelivery delivery);
        ButtonReleaseObservationResult ObserveOwnedCleanupUp(
            ObservedButtonDownAnchor anchor,
            OwnedCleanupButtonUpInsertion insertion);
    }

    internal sealed class ButtonOsObserver : IButtonDeliveryObserver
    {
        private readonly int ownerThreadId;
        private readonly IButtonObservationPlatform platform;

        internal ButtonOsObserver(IButtonObservationPlatform platform)
        {
            if (platform == null) throw new ArgumentNullException("platform");
            ownerThreadId = Thread.CurrentThread.ManagedThreadId;
            this.platform = platform;
        }

        public ButtonDownObservationResult ObserveTaggedDown(
            ButtonGestureTuple gesture,
            TaggedButtonDelivery delivery)
        {
            ButtonDownUnconfirmed invalid = ValidateDownDelivery(gesture, delivery);
            if (invalid != null) return invalid;
            try
            {
                ButtonNormalLevelProbe probe = platform.ProbeNormalLevel(
                    gesture,
                    ButtonCaptureExpectation.InputWindow);
                ButtonDownUnconfirmed frames = ValidateNormalFrames(
                    gesture,
                    delivery,
                    probe.Before,
                    probe.After,
                    probe.Level,
                    probe.ScreenPointTolerance,
                    true);
                if (frames != null) return frames;
                return new ButtonDownObserved(
                    new ObservedButtonDownAnchor(gesture, delivery, probe.After));
            }
            catch (ButtonObservationUnavailableException error)
            {
                return new ButtonDownUnconfirmed(error.Kind, error.Message);
            }
        }

        public ButtonReleaseObservationResult ObserveNormalUp(
            ObservedButtonDownAnchor anchor,
            NormalButtonUpInsertion insertion,
            TaggedButtonDelivery delivery)
        {
            ButtonReleaseUnconfirmed invalid = ValidateUp(
                anchor,
                insertion,
                delivery,
                ButtonObservationScope.Normal);
            if (invalid != null) return invalid;
            try
            {
                ButtonNormalLevelProbe probe = platform.ProbeNormalLevel(
                    anchor.Gesture,
                    ButtonCaptureExpectation.Released);
                ButtonReleaseUnconfirmed frames = ValidateNormalReleaseFrames(
                    anchor.Gesture,
                    delivery,
                    probe.Before,
                    probe.After,
                    probe.Level,
                    probe.ScreenPointTolerance);
                if (frames != null) return frames;
                return new ButtonReleaseObserved(
                    new NormalButtonReleaseEvidence(anchor, delivery, probe.After));
            }
            catch (ButtonObservationUnavailableException error)
            {
                return new ButtonReleaseUnconfirmed(error.Kind, error.Message);
            }
        }

        public ButtonReleaseObservationResult ObserveOwnedCleanupUp(
            ObservedButtonDownAnchor anchor,
            OwnedCleanupButtonUpInsertion insertion)
        {
            if (Thread.CurrentThread.ManagedThreadId != ownerThreadId)
                return ReleaseFailure(ButtonObservationFailureKind.WrongOwnerThread, "wrong observer thread");
            if (anchor == null)
                return ReleaseFailure(
                    ButtonObservationFailureKind.MissingHistoricalDown,
                    "cleanup release has no observed Down anchor");
            if (insertion == null || insertion.Scope != ButtonObservationScope.OwnedCleanup)
                return ReleaseFailure(ButtonObservationFailureKind.WrongScope, "cleanup insertion scope mismatch");
            if (insertion.GestureId != anchor.Gesture.GestureId)
                return ReleaseFailure(ButtonObservationFailureKind.WrongGesture, "cleanup gesture mismatch");
            if (insertion.Tag != anchor.Gesture.UpTag)
                return ReleaseFailure(ButtonObservationFailureKind.WrongTag, "cleanup Up tag mismatch");
            if (insertion.Inserted != 1)
                return ReleaseFailure(ButtonObservationFailureKind.UpNotInserted, "cleanup Up was not inserted once");
            try
            {
                ButtonAccessLevelProbe probe = platform.ProbeAccessLevel();
                if (probe.Before.ButtonsSwapped || probe.After.ButtonsSwapped)
                    return ReleaseFailure(
                        ButtonObservationFailureKind.MappingUnsupported,
                        "swapped buttons are unsupported by the first diagnostic slice");
                if (!probe.Before.EqualsExact(probe.After))
                    return ReleaseFailure(
                        probe.Before.ThreadDesktopIdentity == probe.After.ThreadDesktopIdentity
                            ? ButtonObservationFailureKind.ForegroundChanged
                            : ButtonObservationFailureKind.DesktopChanged,
                        "cleanup access context changed around the physical level sample");
                if (probe.Level != ButtonPhysicalLevel.Up)
                    return ReleaseFailure(
                        ButtonObservationFailureKind.PhysicalLevelMismatch,
                        "physical left button is not observed Up after owned cleanup insertion");
                return new ButtonReleaseObserved(
                    new OwnedCleanupButtonReleaseEvidence(anchor, probe.After));
            }
            catch (ButtonObservationUnavailableException error)
            {
                return ReleaseFailure(error.Kind, error.Message);
            }
        }

        private ButtonDownUnconfirmed ValidateDownDelivery(
            ButtonGestureTuple gesture,
            TaggedButtonDelivery delivery)
        {
            if (Thread.CurrentThread.ManagedThreadId != ownerThreadId)
                return DownFailure(ButtonObservationFailureKind.WrongOwnerThread, "wrong observer thread");
            if (gesture == null || delivery == null || delivery.GestureId != gesture.GestureId)
                return DownFailure(ButtonObservationFailureKind.WrongGesture, "tagged Down gesture mismatch");
            if (delivery.Edge != ButtonTaggedEdgeKind.Down || delivery.Tag != gesture.DownTag)
                return DownFailure(ButtonObservationFailureKind.WrongTag, "tagged Down edge or tag mismatch");
            if (delivery.ReceiverHwnd != gesture.Target.InputHwnd
                || delivery.ReceiverProcessId != gesture.AppProcessId
                || delivery.ReceiverThreadId == 0)
            {
                return DownFailure(ButtonObservationFailureKind.WrongReceiver, "tagged Down receiver mismatch");
            }
            return null;
        }

        private ButtonReleaseUnconfirmed ValidateUp(
            ObservedButtonDownAnchor anchor,
            ButtonUpInsertion insertion,
            TaggedButtonDelivery delivery,
            ButtonObservationScope expectedScope)
        {
            if (Thread.CurrentThread.ManagedThreadId != ownerThreadId)
                return ReleaseFailure(ButtonObservationFailureKind.WrongOwnerThread, "wrong observer thread");
            if (anchor == null)
                return ReleaseFailure(
                    ButtonObservationFailureKind.MissingHistoricalDown,
                    "normal release has no observed Down anchor");
            if (insertion == null || insertion.Scope != expectedScope)
                return ReleaseFailure(ButtonObservationFailureKind.WrongScope, "normal insertion scope mismatch");
            if (insertion.GestureId != anchor.Gesture.GestureId
                || delivery == null
                || delivery.GestureId != anchor.Gesture.GestureId)
            {
                return ReleaseFailure(ButtonObservationFailureKind.WrongGesture, "normal Up gesture mismatch");
            }
            if (insertion.Tag != anchor.Gesture.UpTag
                || delivery.Edge != ButtonTaggedEdgeKind.Up
                || delivery.Tag != anchor.Gesture.UpTag)
            {
                return ReleaseFailure(ButtonObservationFailureKind.WrongTag, "normal Up tag mismatch");
            }
            if (insertion.Inserted != 1)
                return ReleaseFailure(ButtonObservationFailureKind.UpNotInserted, "normal Up was not inserted once");
            if (delivery.ReceiverHwnd != anchor.Gesture.Target.InputHwnd
                || delivery.ReceiverProcessId != anchor.Gesture.AppProcessId
                || delivery.ReceiverThreadId != anchor.Delivery.ReceiverThreadId)
            {
                return ReleaseFailure(ButtonObservationFailureKind.WrongReceiver, "tagged Up receiver mismatch");
            }
            return null;
        }

        private static ButtonDownUnconfirmed ValidateNormalFrames(
            ButtonGestureTuple gesture,
            TaggedButtonDelivery delivery,
            ButtonNormalFrame before,
            ButtonNormalFrame after,
            ButtonPhysicalLevel level,
            ButtonPointTolerance screenPointTolerance,
            bool expectDown)
        {
            if (before.Access.ButtonsSwapped || after.Access.ButtonsSwapped)
                return DownFailure(
                    ButtonObservationFailureKind.MappingUnsupported,
                    "swapped buttons are unsupported by the first diagnostic slice");
            ButtonFrameValidationFailure frames = ValidateNormalFramePair(
                gesture,
                delivery,
                before,
                after,
                screenPointTolerance,
                expectDown ? ButtonCaptureExpectation.InputWindow : ButtonCaptureExpectation.Released);
            if (frames != null) return DownFailure(frames.Kind, frames.Detail);
            if ((level == ButtonPhysicalLevel.Down) != expectDown)
                return DownFailure(
                    ButtonObservationFailureKind.PhysicalLevelMismatch,
                    "physical left button is not observed Down after tagged Down");
            return null;
        }

        private static ButtonReleaseUnconfirmed ValidateNormalReleaseFrames(
            ButtonGestureTuple gesture,
            TaggedButtonDelivery delivery,
            ButtonNormalFrame before,
            ButtonNormalFrame after,
            ButtonPhysicalLevel level,
            ButtonPointTolerance screenPointTolerance)
        {
            if (before.Access.ButtonsSwapped || after.Access.ButtonsSwapped)
                return ReleaseFailure(
                    ButtonObservationFailureKind.MappingUnsupported,
                    "swapped buttons are unsupported by the first diagnostic slice");
            ButtonFrameValidationFailure frames = ValidateNormalFramePair(
                gesture,
                delivery,
                before,
                after,
                screenPointTolerance,
                ButtonCaptureExpectation.Released);
            if (frames != null) return ReleaseFailure(frames.Kind, frames.Detail);
            if (level != ButtonPhysicalLevel.Up)
                return ReleaseFailure(
                    ButtonObservationFailureKind.PhysicalLevelMismatch,
                    "physical left button is not observed Up after tagged Up");
            return null;
        }

        private static ButtonFrameValidationFailure ValidateNormalFramePair(
            ButtonGestureTuple gesture,
            TaggedButtonDelivery delivery,
            ButtonNormalFrame before,
            ButtonNormalFrame after,
            ButtonPointTolerance screenPointTolerance,
            ButtonCaptureExpectation captureExpectation)
        {
            if (before.Access.ThreadDesktopIdentity != after.Access.ThreadDesktopIdentity)
                return FrameFailure(
                    ButtonObservationFailureKind.DesktopChanged,
                    "normal input desktop changed around the physical level sample");
            if (!before.Access.EqualsExact(after.Access))
                return FrameFailure(
                    ButtonObservationFailureKind.ForegroundChanged,
                    "normal foreground access context changed around the physical level sample");
            if (!AccessMatchesGesture(before.Access, gesture))
                return FrameFailure(
                    ButtonObservationFailureKind.WrongReceiver,
                    "normal foreground PID, process creation identity, or owner HWND does not match the gesture");
            if (!ReceiverIdentityEquals(before.Receiver, after.Receiver)
                || !ReceiverMatchesDelivery(before.Receiver, gesture, delivery)
                || !ReceiverMatchesDelivery(after.Receiver, gesture, delivery))
            {
                return FrameFailure(
                    ButtonObservationFailureKind.WrongReceiver,
                    "normal receiver HWND, PID, or TID does not match the tagged delivery");
            }
            if (screenPointTolerance == null
                || !WithinTolerance(
                    delivery.ActualScreenX,
                    gesture.Target.ScreenX,
                    screenPointTolerance.X)
                || !WithinTolerance(
                    delivery.ActualScreenY,
                    gesture.Target.ScreenY,
                    screenPointTolerance.Y))
            {
                return FrameFailure(
                    ButtonObservationFailureKind.PointMismatch,
                    "actual tagged button point does not match the requested physical screen point");
            }
            ulong expectedCapture = captureExpectation == ButtonCaptureExpectation.InputWindow
                ? gesture.Target.InputHwnd
                : 0;
            if (before.Receiver.CaptureHwnd != expectedCapture
                || after.Receiver.CaptureHwnd != expectedCapture)
            {
                return FrameFailure(
                    ButtonObservationFailureKind.CaptureMismatch,
                    "normal receiver capture does not match the expected button edge");
            }
            return null;
        }

        private static bool WithinTolerance(int actual, int requested, int tolerance)
        {
            long delta = (long)actual - requested;
            if (delta < 0) delta = -delta;
            return delta <= tolerance;
        }

        private static bool AccessMatchesGesture(
            ButtonAccessFrame access,
            ButtonGestureTuple gesture)
        {
            return access != null
                && gesture != null
                && access.Foreground.Hwnd == gesture.Target.ParentHwnd
                && access.Foreground.ProcessId == gesture.AppProcessId
                && access.Foreground.ProcessCreationIdentity == gesture.AppCreationIdentity;
        }

        private static bool ReceiverIdentityEquals(
            ButtonReceiverFrame before,
            ButtonReceiverFrame after)
        {
            return before != null
                && after != null
                && before.InputHwnd == after.InputHwnd
                && before.ParentHwnd == after.ParentHwnd
                && before.ProcessId == after.ProcessId
                && before.ThreadId == after.ThreadId;
        }

        private static bool ReceiverMatchesDelivery(
            ButtonReceiverFrame frame,
            ButtonGestureTuple gesture,
            TaggedButtonDelivery delivery)
        {
            return frame != null
                && gesture != null
                && delivery != null
                && frame.InputHwnd == gesture.Target.InputHwnd
                && frame.ParentHwnd == gesture.Target.ParentHwnd
                && frame.ProcessId == gesture.AppProcessId
                && frame.InputHwnd == delivery.ReceiverHwnd
                && frame.ProcessId == delivery.ReceiverProcessId
                && frame.ThreadId == delivery.ReceiverThreadId;
        }

        private static ButtonFrameValidationFailure FrameFailure(
            ButtonObservationFailureKind kind,
            string detail)
        {
            return new ButtonFrameValidationFailure(kind, detail);
        }

        private static ButtonDownUnconfirmed DownFailure(
            ButtonObservationFailureKind kind,
            string detail)
        {
            return new ButtonDownUnconfirmed(kind, detail);
        }

        private static ButtonReleaseUnconfirmed ReleaseFailure(
            ButtonObservationFailureKind kind,
            string detail)
        {
            return new ButtonReleaseUnconfirmed(kind, detail);
        }

        private sealed class ButtonFrameValidationFailure
        {
            internal ButtonFrameValidationFailure(
                ButtonObservationFailureKind kind,
                string detail)
            {
                Kind = kind;
                Detail = detail;
            }

            internal ButtonObservationFailureKind Kind { get; private set; }
            internal string Detail { get; private set; }
        }
    }
}
