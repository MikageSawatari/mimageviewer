using System;
using System.IO;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    internal enum ButtonWireCommandKind : ushort
    {
        BeginGesture = 1,
        DownDispatchReceipt = 2,
        ReleaseGesture = 3,
        UpSemanticReceipt = 4,
        CancelGesture = 5,
        Shutdown = 6,
    }

    internal enum ButtonWireReplyKind : ushort
    {
        Accepted = 1,
        TerminalSuccess = 2,
        TerminalFailure = 3,
        ProtocolFailure = 4,
    }

    internal enum ButtonCancelCode : uint
    {
        RunnerCancelled = 1,
        ApplicationExiting = 2,
        ScenarioDeadline = 3,
        PipeClosing = 4,
    }

    internal sealed class ButtonWireTarget
    {
        internal ulong ParentHwnd;
        internal ulong InputHwnd;
        internal int ScreenX;
        internal int ScreenY;
        internal ulong SourceEpoch;
        internal ulong PlacementGeneration;
        internal ulong HostIncarnation;
        internal ulong BackendToken;

        internal bool EqualsExact(ButtonWireTarget other)
        {
            return other != null
                && ParentHwnd == other.ParentHwnd
                && InputHwnd == other.InputHwnd
                && ScreenX == other.ScreenX
                && ScreenY == other.ScreenY
                && SourceEpoch == other.SourceEpoch
                && PlacementGeneration == other.PlacementGeneration
                && HostIncarnation == other.HostIncarnation
                && BackendToken == other.BackendToken;
        }
    }

    internal sealed class ButtonWireDelivery
    {
        internal ulong ObservedTag;
        internal ulong ReceiverHwnd;
        internal uint ReceiverProcessId;
        internal uint ReceiverThreadId;
        internal int ActualClientX;
        internal int ActualClientY;
        internal int ActualScreenX;
        internal int ActualScreenY;

        internal bool IsZero
        {
            get
            {
                return ObservedTag == 0
                    && ReceiverHwnd == 0
                    && ReceiverProcessId == 0
                    && ReceiverThreadId == 0
                    && ActualClientX == 0
                    && ActualClientY == 0
                    && ActualScreenX == 0
                    && ActualScreenY == 0;
            }
        }
    }

    internal sealed class ButtonWireRequest
    {
        internal ButtonWireCommandKind Kind;
        internal Guid SessionNonce;
        internal ulong GestureId;
        internal uint Step;
        internal uint Flags;
        internal ulong DeadlineTick;
        internal uint AppProcessId;
        internal ulong AppCreationIdentity;
        internal ButtonWireTarget Target;
        internal ulong DownTag;
        internal ulong UpTag;
        internal ButtonWireDelivery Delivery;

        internal bool ReceiptAccepted
        {
            get { return (Flags & 1U) != 0; }
        }

        internal bool RawDeliveryObserved
        {
            get { return (Flags & 2U) != 0; }
        }

        internal void ValidateShape()
        {
            if (!Enum.IsDefined(typeof(ButtonWireCommandKind), Kind))
                throw new ButtonWireProtocolException("unknown command kind");
            if (SessionNonce == Guid.Empty) throw new ButtonWireProtocolException("empty session nonce");
            if (GestureId == 0) throw new ButtonWireProtocolException("zero gesture id");
            if (Step == 0) throw new ButtonWireProtocolException("zero step");
            if (DeadlineTick == 0) throw new ButtonWireProtocolException("zero deadline");
            if (AppProcessId == 0) throw new ButtonWireProtocolException("zero app pid");
            if (AppCreationIdentity == 0)
                throw new ButtonWireProtocolException("zero app creation identity");
            if (Target == null
                || Target.ParentHwnd == 0
                || Target.InputHwnd == 0
                || Target.HostIncarnation == 0
                || Target.BackendToken == 0)
            {
                throw new ButtonWireProtocolException("incomplete fixed target");
            }
            if (DownTag == 0 || UpTag == 0 || DownTag == UpTag)
                throw new ButtonWireProtocolException("invalid button tags");

            bool receipt = Kind == ButtonWireCommandKind.DownDispatchReceipt
                || Kind == ButtonWireCommandKind.UpSemanticReceipt;
            bool cancel = Kind == ButtonWireCommandKind.CancelGesture;
            if (receipt)
            {
                if ((Flags & ~3U) != 0)
                    throw new ButtonWireProtocolException("unknown receipt flags");
                if (ReceiptAccepted && !RawDeliveryObserved)
                    throw new ButtonWireProtocolException("accepted receipt has no raw delivery");
                if (RawDeliveryObserved)
                {
                    if (Delivery == null
                        || Delivery.ObservedTag == 0
                        || Delivery.ReceiverHwnd == 0
                        || Delivery.ReceiverProcessId == 0
                        || Delivery.ReceiverThreadId == 0)
                    {
                        throw new ButtonWireProtocolException("incomplete raw button delivery");
                    }
                }
                else if (Delivery != null && !Delivery.IsZero)
                {
                    throw new ButtonWireProtocolException("raw delivery fields without observed flag");
                }
            }
            else if (cancel)
            {
                if (!Enum.IsDefined(typeof(ButtonCancelCode), Flags))
                    throw new ButtonWireProtocolException("unknown cancellation code");
            }
            else if (Flags != 0)
            {
                throw new ButtonWireProtocolException("unexpected command flags");
            }
            if (!receipt && Delivery != null && !Delivery.IsZero)
                throw new ButtonWireProtocolException("delivery fields on non-receipt command");
        }

        internal bool MatchesImmutableGesture(ButtonWireRequest first)
        {
            return first != null
                && SessionNonce == first.SessionNonce
                && GestureId == first.GestureId
                && DeadlineTick == first.DeadlineTick
                && AppProcessId == first.AppProcessId
                && AppCreationIdentity == first.AppCreationIdentity
                && DownTag == first.DownTag
                && UpTag == first.UpTag
                && Target.EqualsExact(first.Target);
        }
    }

    internal sealed class ButtonWireReply
    {
        internal ButtonWireReplyKind Kind;
        internal Guid SessionNonce;
        internal ulong GestureId;
        internal uint Step;
        internal uint Phase;
        internal uint Detail;
    }

    internal sealed class ButtonWireProtocolException : IOException
    {
        internal ButtonWireProtocolException(string message) : base(message) { }
    }

    internal static class ButtonWireCodec
    {
        internal const uint RequestMagic = 0x4256494dU;
        internal const uint ReplyMagic = 0x5256494dU;
        internal const ushort Version = 2;
        internal const int RequestPayloadLength = 176;
        internal const int ReplyPayloadLength = 48;
        internal const int MaximumPayloadLength = 256;

        internal static byte[] EncodeRequest(ButtonWireRequest request)
        {
            request.ValidateShape();
            using (MemoryStream memory = new MemoryStream(RequestPayloadLength))
            using (BinaryWriter writer = new BinaryWriter(memory))
            {
                writer.Write(RequestMagic);
                writer.Write(Version);
                writer.Write((ushort)request.Kind);
                writer.Write(request.SessionNonce.ToByteArray());
                writer.Write(request.GestureId);
                writer.Write(request.Step);
                writer.Write(request.Flags);
                writer.Write(request.DeadlineTick);
                writer.Write(request.AppProcessId);
                writer.Write(0U);
                writer.Write(request.AppCreationIdentity);
                writer.Write(request.Target.ParentHwnd);
                writer.Write(request.Target.InputHwnd);
                writer.Write(request.Target.ScreenX);
                writer.Write(request.Target.ScreenY);
                writer.Write(request.DownTag);
                writer.Write(request.UpTag);
                writer.Write(request.Target.SourceEpoch);
                writer.Write(request.Target.PlacementGeneration);
                writer.Write(request.Target.HostIncarnation);
                writer.Write(request.Target.BackendToken);
                ButtonWireDelivery delivery = request.Delivery ?? new ButtonWireDelivery();
                writer.Write(delivery.ObservedTag);
                writer.Write(delivery.ReceiverHwnd);
                writer.Write(delivery.ReceiverProcessId);
                writer.Write(delivery.ReceiverThreadId);
                writer.Write(delivery.ActualClientX);
                writer.Write(delivery.ActualClientY);
                writer.Write(delivery.ActualScreenX);
                writer.Write(delivery.ActualScreenY);
                writer.Flush();
                byte[] payload = memory.ToArray();
                if (payload.Length != RequestPayloadLength)
                    throw new InvalidOperationException("request wire size changed");
                return payload;
            }
        }

        internal static ButtonWireRequest DecodeRequest(byte[] payload)
        {
            if (payload == null || payload.Length != RequestPayloadLength)
                throw new ButtonWireProtocolException("request has wrong length");
            using (BinaryReader reader = new BinaryReader(new MemoryStream(payload, false)))
            {
                if (reader.ReadUInt32() != RequestMagic)
                    throw new ButtonWireProtocolException("request magic mismatch");
                if (reader.ReadUInt16() != Version)
                    throw new ButtonWireProtocolException("request version mismatch");
                ButtonWireRequest request = new ButtonWireRequest();
                request.Kind = (ButtonWireCommandKind)reader.ReadUInt16();
                request.SessionNonce = new Guid(ReadExact(reader, 16));
                request.GestureId = reader.ReadUInt64();
                request.Step = reader.ReadUInt32();
                request.Flags = reader.ReadUInt32();
                request.DeadlineTick = reader.ReadUInt64();
                request.AppProcessId = reader.ReadUInt32();
                if (reader.ReadUInt32() != 0)
                    throw new ButtonWireProtocolException("request reserved field was nonzero");
                request.AppCreationIdentity = reader.ReadUInt64();
                request.Target = new ButtonWireTarget();
                request.Target.ParentHwnd = reader.ReadUInt64();
                request.Target.InputHwnd = reader.ReadUInt64();
                request.Target.ScreenX = reader.ReadInt32();
                request.Target.ScreenY = reader.ReadInt32();
                request.DownTag = reader.ReadUInt64();
                request.UpTag = reader.ReadUInt64();
                request.Target.SourceEpoch = reader.ReadUInt64();
                request.Target.PlacementGeneration = reader.ReadUInt64();
                request.Target.HostIncarnation = reader.ReadUInt64();
                request.Target.BackendToken = reader.ReadUInt64();
                request.Delivery = new ButtonWireDelivery();
                request.Delivery.ObservedTag = reader.ReadUInt64();
                request.Delivery.ReceiverHwnd = reader.ReadUInt64();
                request.Delivery.ReceiverProcessId = reader.ReadUInt32();
                request.Delivery.ReceiverThreadId = reader.ReadUInt32();
                request.Delivery.ActualClientX = reader.ReadInt32();
                request.Delivery.ActualClientY = reader.ReadInt32();
                request.Delivery.ActualScreenX = reader.ReadInt32();
                request.Delivery.ActualScreenY = reader.ReadInt32();
                if (reader.BaseStream.Position != RequestPayloadLength)
                    throw new ButtonWireProtocolException("request trailing bytes");
                request.ValidateShape();
                return request;
            }
        }

        internal static byte[] EncodeReply(ButtonWireReply reply)
        {
            if (reply == null || reply.SessionNonce == Guid.Empty || reply.GestureId == 0)
                throw new ButtonWireProtocolException("invalid reply identity");
            using (MemoryStream memory = new MemoryStream(ReplyPayloadLength))
            using (BinaryWriter writer = new BinaryWriter(memory))
            {
                writer.Write(ReplyMagic);
                writer.Write(Version);
                writer.Write((ushort)reply.Kind);
                writer.Write(reply.SessionNonce.ToByteArray());
                writer.Write(reply.GestureId);
                writer.Write(reply.Step);
                writer.Write(reply.Phase);
                writer.Write(reply.Detail);
                writer.Write(0U);
                writer.Flush();
                byte[] payload = memory.ToArray();
                if (payload.Length != ReplyPayloadLength)
                    throw new InvalidOperationException("reply wire size changed");
                return payload;
            }
        }

        internal static ButtonWireReply DecodeReply(byte[] payload)
        {
            if (payload == null || payload.Length != ReplyPayloadLength)
                throw new ButtonWireProtocolException("reply has wrong length");
            using (BinaryReader reader = new BinaryReader(new MemoryStream(payload, false)))
            {
                if (reader.ReadUInt32() != ReplyMagic)
                    throw new ButtonWireProtocolException("reply magic mismatch");
                if (reader.ReadUInt16() != Version)
                    throw new ButtonWireProtocolException("reply version mismatch");
                ButtonWireReply reply = new ButtonWireReply();
                reply.Kind = (ButtonWireReplyKind)reader.ReadUInt16();
                if (!Enum.IsDefined(typeof(ButtonWireReplyKind), reply.Kind))
                    throw new ButtonWireProtocolException("unknown reply kind");
                reply.SessionNonce = new Guid(ReadExact(reader, 16));
                reply.GestureId = reader.ReadUInt64();
                reply.Step = reader.ReadUInt32();
                reply.Phase = reader.ReadUInt32();
                reply.Detail = reader.ReadUInt32();
                if (reader.ReadUInt32() != 0)
                    throw new ButtonWireProtocolException("reply reserved field was nonzero");
                return reply;
            }
        }

        internal static void WriteFrame(Stream stream, byte[] payload)
        {
            if (stream == null) throw new ArgumentNullException("stream");
            if (payload == null || payload.Length == 0 || payload.Length > MaximumPayloadLength)
                throw new ButtonWireProtocolException("outbound frame length is invalid");
            byte[] length = BitConverter.GetBytes(payload.Length);
            stream.Write(length, 0, length.Length);
            stream.Write(payload, 0, payload.Length);
            stream.Flush();
        }

        internal static byte[] ReadFrame(Stream stream)
        {
            if (stream == null) throw new ArgumentNullException("stream");
            byte[] lengthBytes = ReadExact(stream, 4);
            int length = BitConverter.ToInt32(lengthBytes, 0);
            if (length <= 0 || length > MaximumPayloadLength)
                throw new ButtonWireProtocolException("inbound frame length is invalid");
            return ReadExact(stream, length);
        }

        private static byte[] ReadExact(BinaryReader reader, int count)
        {
            byte[] bytes = reader.ReadBytes(count);
            if (bytes.Length != count) throw new EndOfStreamException();
            return bytes;
        }

        private static byte[] ReadExact(Stream stream, int count)
        {
            byte[] bytes = new byte[count];
            int offset = 0;
            while (offset < count)
            {
                int read = stream.Read(bytes, offset, count - offset);
                if (read == 0) throw new EndOfStreamException();
                offset += read;
            }
            return bytes;
        }
    }
}
