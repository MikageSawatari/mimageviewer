$ErrorActionPreference = 'Stop'

# Creates only a hidden diagnostic HWND; never shows, activates, or sends input to a window.
# This probes USER32 behavior, not the product's message pump or physical flicker.
if (-not ('HiddenHudZOrderProbe' -as [type])) {
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public static class HiddenHudZOrderProbe {
    delegate IntPtr WndProc(IntPtr hwnd, uint msg, IntPtr wparam, IntPtr lparam);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct WndClass {
        public uint size, style;
        public WndProc proc;
        public int classExtra, windowExtra;
        public IntPtr instance, icon, cursor, background;
        public string menuName, className;
        public IntPtr iconSmall;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct WindowPos {
        public IntPtr hwnd, insertAfter;
        public int x, y, cx, cy;
        public uint flags;
    }
    public sealed class Observation {
        public string Operation;
        public long InsertAfter;
        public uint Flags;
        public bool NoZOrder;
        public bool CurrentCodeRequestsRaise;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct Rect { public int left, top, right, bottom; }
    public sealed class CandidateResult {
        public string Operation;
        public int ChangingNotifications;
        public bool CurrentPredicateRequestsRaise;
        public bool NoZOrderFilteredPredicateRequestsRaise;
        public bool HudAboveEditor;
        public bool GeometryUnchanged;
        public bool Topmost;
        public bool ForegroundUnchanged;
        public bool RemainedHidden;
    }
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    static extern IntPtr GetModuleHandleW(string name);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern ushort RegisterClassExW(ref WndClass cls);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateWindowExW(uint exStyle, string cls, string title, uint style,
        int x, int y, int width, int height, IntPtr parent, IntPtr menu, IntPtr instance, IntPtr param);
    [DllImport("user32.dll", SetLastError = true)]
    static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")]
    static extern IntPtr DefWindowProcW(IntPtr hwnd, uint msg, IntPtr wparam, IntPtr lparam);
    [DllImport("user32.dll")]
    static extern bool DestroyWindow(IntPtr hwnd);
    [DllImport("user32.dll")]
    static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")]
    static extern IntPtr GetWindow(IntPtr hwnd, uint command);
    [DllImport("user32.dll")]
    static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")]
    static extern bool GetWindowRect(IntPtr hwnd, out Rect rect);
    [DllImport("user32.dll")]
    static extern int GetWindowLongW(IntPtr hwnd, int index);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern bool UnregisterClassW(string cls, IntPtr instance);
    static readonly List<Observation> observations = new List<Observation>();
    static string operation;
    static readonly WndProc callback = OnMessage;
    static IntPtr OnMessage(IntPtr hwnd, uint msg, IntPtr wparam, IntPtr lparam) {
        if (msg == 0x0046 && lparam != IntPtr.Zero && operation != null) {
            var wp = (WindowPos)Marshal.PtrToStructure(lparam, typeof(WindowPos));
            long raw = wp.insertAfter.ToInt64();
            observations.Add(new Observation {
                Operation = operation, InsertAfter = raw, Flags = wp.flags,
                NoZOrder = (wp.flags & 0x0004) != 0,
                CurrentCodeRequestsRaise = raw == 0 || raw == -1
            });
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    public static Observation[] Run() {
        observations.Clear();
        string clsName = "MivHiddenHudZOrderProbe_" + Guid.NewGuid().ToString("N");
        IntPtr instance = GetModuleHandleW(null);
        var cls = new WndClass { size = (uint)Marshal.SizeOf(typeof(WndClass)),
            proc = callback, instance = instance, className = clsName };
        if (RegisterClassExW(ref cls) == 0) throw new System.ComponentModel.Win32Exception();
        IntPtr hwnd = IntPtr.Zero;
        try {
            hwnd = CreateWindowExW(0x08000088, clsName, "hidden diagnostic", 0x80000000,
                0, 0, 16, 16, IntPtr.Zero, IntPtr.Zero, instance, IntPtr.Zero);
            if (hwnd == IntPtr.Zero) throw new System.ComponentModel.Win32Exception();
            for (int n = 1; n <= 3; n++) {
                operation = "raise_to_top_" + n;
                if (!SetWindowPos(hwnd, new IntPtr(-1), 0, 0, 0, 0, 0x0213))
                    throw new System.ComponentModel.Win32Exception();
            }
            operation = "geometry_with_nozorder";
            if (!SetWindowPos(hwnd, IntPtr.Zero, 1, 1, 16, 16, 0x0214))
                throw new System.ComponentModel.Win32Exception();
            if (IsWindowVisible(hwnd)) throw new Exception("Diagnostic HWND unexpectedly visible");
            operation = null;
            return observations.ToArray();
        } finally {
            operation = null;
            if (hwnd != IntPtr.Zero) DestroyWindow(hwnd);
            UnregisterClassW(clsName, instance);
        }
    }
    static bool IsAbove(IntPtr above, IntPtr below) {
        IntPtr current = GetWindow(below, 3);
        for (int n = 0; current != IntPtr.Zero && n < 4096; n++) {
            if (current == above) return true;
            current = GetWindow(current, 3);
        }
        return false;
    }
    public static CandidateResult[] RunCandidates() {
        var results = new List<CandidateResult>();
        string clsName = "MivHiddenHudCandidateProbe_" + Guid.NewGuid().ToString("N");
        IntPtr instance = GetModuleHandleW(null);
        var cls = new WndClass { size = (uint)Marshal.SizeOf(typeof(WndClass)),
            proc = callback, instance = instance, className = clsName };
        if (RegisterClassExW(ref cls) == 0) throw new System.ComponentModel.Win32Exception();
        IntPtr owner = IntPtr.Zero, hud = IntPtr.Zero, editor = IntPtr.Zero;
        IntPtr foregroundBefore = GetForegroundWindow();
        try {
            owner = CreateWindowExW(0x08000080, clsName, "hidden owner", 0x80000000,
                0, 0, 32, 32, IntPtr.Zero, IntPtr.Zero, instance, IntPtr.Zero);
            hud = CreateWindowExW(0x08000088, clsName, "hidden hud", 0x80000000,
                0, 0, 16, 16, owner, IntPtr.Zero, instance, IntPtr.Zero);
            editor = CreateWindowExW(0x08000088, clsName, "hidden editor", 0x80000000,
                0, 0, 16, 16, owner, IntPtr.Zero, instance, IntPtr.Zero);
            if (owner == IntPtr.Zero || hud == IntPtr.Zero || editor == IntPtr.Zero)
                throw new System.ComponentModel.Win32Exception();
            string[] names = { "current_raise", "own_raise_nosendchanging", "geometry_nozorder", "external_raise_notifications_retained" };
            uint[] flags = { 0x0213, 0x0613, 0x0214, 0x0213 };
            for (int n = 0; n < names.Length; n++) {
                operation = null;
                if (!SetWindowPos(editor, new IntPtr(-1), 0, 0, 0, 0, 0x0613))
                    throw new System.ComponentModel.Win32Exception();
                if (!IsAbove(editor, hud)) throw new Exception("Could not arrange hidden editor above HUD");
                Rect before, after;
                if (!GetWindowRect(hud, out before)) throw new System.ComponentModel.Win32Exception();
                observations.Clear();
                operation = names[n];
                IntPtr insertAfter = n == 2 ? IntPtr.Zero : new IntPtr(-1);
                if (!SetWindowPos(hud, insertAfter, 1, 1, 16, 16, flags[n]))
                    throw new System.ComponentModel.Win32Exception();
                operation = null;
                if (!GetWindowRect(hud, out after)) throw new System.ComponentModel.Win32Exception();
                bool oldRequest = false, filteredRequest = false;
                foreach (var observation in observations) {
                    oldRequest |= observation.CurrentCodeRequestsRaise;
                    filteredRequest |= observation.CurrentCodeRequestsRaise && !observation.NoZOrder;
                }
                results.Add(new CandidateResult {
                    Operation = names[n], ChangingNotifications = observations.Count,
                    CurrentPredicateRequestsRaise = oldRequest,
                    NoZOrderFilteredPredicateRequestsRaise = filteredRequest,
                    HudAboveEditor = IsAbove(hud, editor),
                    GeometryUnchanged = before.left == after.left && before.top == after.top
                        && before.right == after.right && before.bottom == after.bottom,
                    Topmost = (GetWindowLongW(hud, -20) & 8) != 0,
                    ForegroundUnchanged = GetForegroundWindow() == foregroundBefore,
                    RemainedHidden = !IsWindowVisible(owner) && !IsWindowVisible(hud) && !IsWindowVisible(editor)
                });
            }
            return results.ToArray();
        } finally {
            operation = null;
            if (editor != IntPtr.Zero) DestroyWindow(editor);
            if (hud != IntPtr.Zero) DestroyWindow(hud);
            if (owner != IntPtr.Zero) DestroyWindow(owner);
            UnregisterClassW(clsName, instance);
        }
    }
}
'@
}

$currentNotifications = [HiddenHudZOrderProbe]::Run()
$candidates = [HiddenHudZOrderProbe]::RunCandidates()
if ($candidates[1].ChangingNotifications -ne 0 -or
    -not $candidates[1].HudAboveEditor -or
    -not $candidates[1].GeometryUnchanged -or
    -not $candidates[1].Topmost -or
    -not $candidates[1].RemainedHidden) {
    throw 'The hidden own-raise candidate did not preserve the expected USER32 behavior.'
}
if (-not $candidates[2].CurrentPredicateRequestsRaise -or
    $candidates[2].NoZOrderFilteredPredicateRequestsRaise -or
    $candidates[2].HudAboveEditor) {
    throw 'The hidden NOZORDER candidate did not preserve the expected USER32 behavior.'
}
if (-not $candidates[3].NoZOrderFilteredPredicateRequestsRaise -or
    -not $candidates[3].HudAboveEditor) {
    throw 'The ordinary changing notification was unexpectedly suppressed.'
}
[pscustomobject]@{
    CurrentNotifications = $currentNotifications
    Candidates = $candidates
} | ConvertTo-Json -Depth 5
