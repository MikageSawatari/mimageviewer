using System;
using System.Collections.Generic;

namespace Miv.UiSmoke.ButtonHelperDraft
{
    // Pure fake input tests: no native backend, clipboard, HWND, pipe or app is opened.
    public static class ClipboardKeyHelperTests
    {
        private sealed class Fake : IClipboardKeyBackend
        {
            internal readonly List<string> Steps = new List<string>();
            internal bool Control;
            internal bool V;
            internal bool RejectV;
            internal bool RejectVUp;
            internal bool ThrowVUp;
            internal bool RejectSecondValidation;
            private int validations;
            public void Validate(ulong hwnd, uint process)
            {
                Steps.Add("Validate"); validations++;
                if (RejectSecondValidation && validations == 2) throw new InvalidOperationException("changed owner");
            }
            public bool Pressed(int key) { return key == 0x11 ? Control : key == 0x56 && V; }
            public bool Send(ushort key, bool up)
            {
                Steps.Add((key == 0x11 ? "Ctrl" : "V") + (up ? "Up" : "Down"));
                if (key == 0x56 && up && ThrowVUp) throw new InvalidOperationException("V up fault");
                if (key == 0x56 && ((!up && RejectV) || (up && RejectVUp))) return false;
                if (key == 0x11) Control = !up; else V = !up;
                return true;
            }
        }
        private static void Assert(bool condition) { if (!condition) throw new Exception("clipboard key helper regression"); }
        private static void Throws(Action action) { try { action(); } catch (InvalidOperationException) { return; } throw new Exception("expected key helper rejection"); }
        public static void RunAll()
        {
            Assert(System.Runtime.InteropServices.Marshal.SizeOf(typeof(ButtonInputNative.Input)) == (IntPtr.Size == 8 ? 40 : 28));
            Assert(System.Runtime.InteropServices.Marshal.OffsetOf(typeof(ButtonInputNative.Input), "Data").ToInt32() == (IntPtr.Size == 8 ? 8 : 4));
            Fake fake = new Fake(); ClipboardKeyOwner owner = new ClipboardKeyOwner(fake);
            owner.Begin(1, 2); Assert(owner.Outstanding && fake.Control && fake.V);
            owner.Release(); Assert(!owner.Outstanding && String.Join(",", fake.Steps.ToArray()) == "Validate,CtrlDown,Validate,VDown,VUp,CtrlUp");
            owner.Begin(1, 2); owner.Release(); Assert(!owner.Outstanding); // repeated gestures
            fake = new Fake { RejectV = true }; owner = new ClipboardKeyOwner(fake);
            Throws(() => owner.Begin(1, 2)); owner.Release();
            Assert(!owner.Outstanding && !fake.Steps.Contains("VUp") && fake.Steps.Contains("CtrlUp"));
            fake = new Fake { RejectSecondValidation = true }; owner = new ClipboardKeyOwner(fake);
            Throws(() => owner.Begin(1, 2)); owner.Release();
            Assert(!owner.Outstanding && !fake.Steps.Contains("VDown"));
            fake = new Fake { Control = true }; owner = new ClipboardKeyOwner(fake);
            Throws(() => owner.Begin(1, 2)); owner.Release(); Assert(fake.Control && !fake.Steps.Contains("CtrlUp"));
            fake = new Fake { RejectVUp = true }; owner = new ClipboardKeyOwner(fake); owner.Begin(1, 2);
            Throws(() => owner.Release()); Assert(owner.VOwned && !owner.ControlOwned);
            fake = new Fake { ThrowVUp = true }; owner = new ClipboardKeyOwner(fake); owner.Begin(1, 2);
            Throws(() => owner.Release()); Assert(owner.VOwned && !owner.ControlOwned && fake.Steps.Contains("CtrlUp"));
            Console.WriteLine("ClipboardKeyHelper pure tests: 7 passed (no OS input)");
        }
    }
}
