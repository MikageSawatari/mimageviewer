# Diagnostic helper for the section 1.864 Browser Back/Forward input trace.
# This is not part of the product or any build/release procedure.
# Enable the mIV mouse-seek trace separately, then focus its video fullscreen
# window during the countdown. SendInput is global. The foreground process
# check is best-effort and cannot guarantee mIV-only delivery if focus changes.
# Do not touch other windows while it runs. Close browsers/forms you care about
# or keep them unfocused. Run this helper manually on the user's PC.
[CmdletBinding()]
param(
    [ValidateRange(1, 2147483647)]
    [int]$Count = 5,
    [ValidateRange(0, 2147483647)]
    [int]$IntervalMs = 1500,
    [ValidateRange(0, 2147483647)]
    [int]$DelaySeconds = 5,
    [switch]$Forward
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

if (-not ('MivBrowserKeyDiagnostic.Native' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

namespace MivBrowserKeyDiagnostic {
    public static class Native {
        [StructLayout(LayoutKind.Sequential)]
        private struct KEYBDINPUT {
            public ushort wVk, wScan;
            public uint dwFlags, time;
            public UIntPtr dwExtraInfo;
        }
        [StructLayout(LayoutKind.Sequential)]
        private struct MOUSEINPUT {
            public int dx, dy;
            public uint mouseData, dwFlags, time;
            public UIntPtr dwExtraInfo;
        }
        [StructLayout(LayoutKind.Explicit)]
        private struct INPUTUNION {
            [FieldOffset(0)] public KEYBDINPUT key;
            // Retain the full native INPUT size and alignment on x86 and x64.
            [FieldOffset(0)] public MOUSEINPUT mouse;
        }
        [StructLayout(LayoutKind.Sequential)]
        private struct INPUT {
            public uint type;
            public INPUTUNION data;
        }
        [DllImport("user32.dll")]
        public static extern IntPtr GetForegroundWindow();
        [DllImport("user32.dll")]
        public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
        [DllImport("user32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
        private static extern int GetWindowTextW(IntPtr window, StringBuilder text, int count);
        [DllImport("user32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
        private static extern int GetWindowTextLengthW(IntPtr window);
        [DllImport("user32.dll", ExactSpelling = true)]
        public static extern uint MapVirtualKeyW(uint code, uint mapType);
        [DllImport("user32.dll", SetLastError = true)]
        private static extern uint SendInput(uint count, INPUT[] inputs, int size);

        public static string Title(IntPtr window) {
            var text = new StringBuilder(GetWindowTextLengthW(window) + 1);
            GetWindowTextW(window, text, text.Capacity);
            return text.ToString();
        }

        public static uint SendPair(IntPtr expectedWindow, uint expectedProcess,
                                    ushort virtualKey, ushort scan, out int error) {
            error = 0;
            INPUT[] inputs = new INPUT[2];
            for (int i = 0; i < inputs.Length; ++i) {
                inputs[i].type = 1; // INPUT_KEYBOARD
                inputs[i].data.key.wVk = virtualKey;
                inputs[i].data.key.wScan = scan;
                inputs[i].data.key.dwFlags = 0x0001u | (i == 1 ? 0x0002u : 0u);
                // KEYEVENTF_EXTENDEDKEY, plus KEYEVENTF_KEYUP for the release.
            }
            int inputSize = Marshal.SizeOf(typeof(INPUT));
            // Best-effort only: focus can still change after this global check.
            uint currentProcess;
            IntPtr currentWindow = GetForegroundWindow();
            GetWindowThreadProcessId(currentWindow, out currentProcess);
            if (currentWindow != expectedWindow || currentProcess != expectedProcess)
                return 0;
            uint sent = SendInput(2, inputs, inputSize);
            if (sent != 2) error = Marshal.GetLastWin32Error();
            return sent;
        }
    }
}
'@
}

$browserVk = if ($Forward) { [uint16]0xA7 } else { [uint16]0xA6 }
$browserName = if ($Forward) { 'Browser Forward' } else { 'Browser Back' }
$browserScan = [uint16]([MivBrowserKeyDiagnostic.Native]::MapVirtualKeyW($browserVk, 0) -band 0xFF)
if ($browserScan -eq 0) {
    throw 'MapVirtualKeyW could not map the Browser key; nothing was sent.'
}
Write-Host ('Diagnostic: {0}, VK=0x{1:X2}, scan=0x{2:X2}, count={3}' -f $browserName, $browserVk, $browserScan, $Count)
Write-Host 'SendInput is global. The foreground check is best-effort; mIV-only delivery is not guaranteed.'
Write-Host 'Click the mIV video fullscreen window to focus it.'
Write-Host 'Then do not touch other windows while it runs. Close browsers/forms you care about or keep them unfocused.'
for ($remainingSeconds = $DelaySeconds; $remainingSeconds -gt 0; $remainingSeconds--) {
    Write-Host ('Starting in {0} second(s)...' -f $remainingSeconds)
    Start-Sleep -Seconds 1
}

for ($sendIndex = 1; $sendIndex -le $Count; $sendIndex++) {
    $foregroundWindow = [MivBrowserKeyDiagnostic.Native]::GetForegroundWindow()
    [uint32]$foregroundProcessId = 0
    [void][MivBrowserKeyDiagnostic.Native]::GetWindowThreadProcessId($foregroundWindow, [ref]$foregroundProcessId)
    $foregroundProcess = Get-Process -Id $foregroundProcessId -ErrorAction SilentlyContinue
    $foregroundName = if ($null -ne $foregroundProcess) { $foregroundProcess.ProcessName } else { '<unknown>' }
    $foregroundTitle = [MivBrowserKeyDiagnostic.Native]::Title($foregroundWindow)
    Write-Host ('[{0}/{1}] Foreground: {2}.exe | {3}' -f $sendIndex, $Count, $foregroundName, $foregroundTitle)
    if ($null -eq $foregroundProcess -or $foregroundName -notlike 'mimageviewer*') {
        Write-Warning 'SKIPPED: the foreground process is not mimageviewer*.exe.'
    } else {
        [int]$sendError = 0
        $sentInputs = [MivBrowserKeyDiagnostic.Native]::SendPair($foregroundWindow, $foregroundProcessId, $browserVk, $browserScan, [ref]$sendError)
        if ($sentInputs -eq 2) {
            Write-Host ('Sent one {0} down/up pair.' -f $browserName)
        } elseif ($sentInputs -eq 0 -and $sendError -eq 0) {
            Write-Warning 'SKIPPED: foreground changed before SendInput, or SendInput was blocked.'
        } else {
            Write-Warning ('SendInput inserted {0}/2 events; Win32 error={1}. No retry.' -f $sentInputs, $sendError)
        }
    }
    if ($sendIndex -lt $Count) { Start-Sleep -Milliseconds $IntervalMs }
}
