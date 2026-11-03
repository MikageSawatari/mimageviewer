# Sub-PC helper for computer-use testing on a single screen.
#
# While computer use is active, the Claude desktop app (Windows) docks its own
# window as a topmost side panel. The panel is left out of computer-use
# screenshots but still receives clicks, so a click on the app under test can
# land on the panel instead. See the sub-PC section of
# docs/development-build-and-test-native.md.
#
#   Info  : list Claude windows and the target window, report overlap
#   Place : move the target window into the free area beside the panel
#   Unpin : clear a stuck always-on-top flag on Claude windows (after testing)
#
# Coordinates are physical pixels (the script is per-monitor DPI aware).
param(
    [Parameter(Mandatory = $true)][ValidateSet('Info', 'Place', 'Unpin')][string]$Mode,
    [int]$ProcessId = 0,
    [int]$Margin = 16
)
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public static class SubPcWin {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int cbSize; public RECT rcMonitor; public RECT rcWork; public int dwFlags; }
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int i);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint f);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h, uint f);
    [DllImport("user32.dll")] public static extern bool GetMonitorInfo(IntPtr m, ref MONITORINFO mi);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);

    public static List<IntPtr> Visible() {
        var list = new List<IntPtr>();
        EnumWindows((h, l) => { if (IsWindowVisible(h)) list.Add(h); return true; }, IntPtr.Zero);
        return list;
    }
    public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
    public static RECT Rect(IntPtr h) { RECT r; GetWindowRect(h, out r); return r; }
    public static bool Topmost(IntPtr h) { return (GetWindowLong(h, -20) & 0x8) != 0; }
    public static RECT Work(IntPtr h) {
        var mi = new MONITORINFO(); mi.cbSize = Marshal.SizeOf(typeof(MONITORINFO));
        GetMonitorInfo(MonitorFromWindow(h, 2), ref mi); return mi.rcWork;
    }
}
'@

# Per-monitor DPI aware v2, so rects are physical pixels.
[void][SubPcWin]::SetThreadDpiAwarenessContext([IntPtr](-4))

function Format-Rect($r) { "L=$($r.L) T=$($r.T) R=$($r.R) B=$($r.B) ($($r.R - $r.L)x$($r.B - $r.T))" }
function Test-Overlap($a, $b) { ($a.L -lt $b.R) -and ($b.L -lt $a.R) -and ($a.T -lt $b.B) -and ($b.T -lt $a.B) }

$claudePids = @(Get-Process -Name claude -ErrorAction SilentlyContinue | ForEach-Object { [uint32]$_.Id })
$claudeWindows = @()
foreach ($h in [SubPcWin]::Visible()) {
    if ($claudePids -notcontains [SubPcWin]::Pid($h)) { continue }
    if ([SubPcWin]::IsIconic($h)) { continue }
    $r = [SubPcWin]::Rect($h)
    if (($r.R - $r.L) -lt 100 -or ($r.B - $r.T) -lt 100) { continue }
    $claudeWindows += [pscustomobject]@{ Handle = $h; Rect = $r; Topmost = [SubPcWin]::Topmost($h) }
}

$target = $null
if ($ProcessId -ne 0) {
    $best = 0
    foreach ($h in [SubPcWin]::Visible()) {
        if ([SubPcWin]::Pid($h) -ne [uint32]$ProcessId) { continue }
        $r = [SubPcWin]::Rect($h)
        $area = ($r.R - $r.L) * ($r.B - $r.T)
        if ($area -gt $best) { $best = $area; $target = $h }
    }
    if ($null -eq $target) { Write-Output "No visible window for PID $ProcessId."; exit 1 }
}

if ($Mode -eq 'Unpin') {
    foreach ($w in $claudeWindows | Where-Object { $_.Topmost }) {
        # HWND_NOTOPMOST, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE
        [void][SubPcWin]::SetWindowPos($w.Handle, [IntPtr](-2), 0, 0, 0, 0, 0x13)
        Write-Output "Unpinned Claude window $($w.Handle): topmost now $([SubPcWin]::Topmost($w.Handle))"
    }
    if (-not ($claudeWindows | Where-Object { $_.Topmost })) { Write-Output 'No topmost Claude window.' }
    exit 0
}

Write-Output "Claude windows ($($claudePids.Count) claude processes):"
foreach ($w in $claudeWindows) { Write-Output "  $($w.Handle) topmost=$($w.Topmost) $(Format-Rect $w.Rect)" }
if ($claudeWindows.Count -eq 0) { Write-Output '  (none visible)' }
if ($null -eq $target) { exit 0 }

$work = [SubPcWin]::Work($target)
$rect = [SubPcWin]::Rect($target)
Write-Output "Work area: $(Format-Rect $work)"
Write-Output "Target:    $(Format-Rect $rect) maximized=$([SubPcWin]::IsZoomed($target))"
$blocking = @($claudeWindows | Where-Object { $_.Topmost -and (Test-Overlap $_.Rect $work) })

if ($Mode -eq 'Info') {
    $hit = @($blocking | Where-Object { Test-Overlap $_.Rect $rect })
    if ($hit.Count -gt 0) { Write-Output 'OVERLAP: a topmost Claude window covers part of the target.' } else { Write-Output 'No overlap.' }
    exit 0
}

# Place: use the wider free strip left or right of the topmost Claude windows.
if ($blocking.Count -eq 0) { Write-Output 'No topmost Claude window on this monitor; nothing to move.'; exit 0 }
$panelL = ($blocking | ForEach-Object { $_.Rect.L } | Measure-Object -Minimum).Minimum
$panelR = ($blocking | ForEach-Object { $_.Rect.R } | Measure-Object -Maximum).Maximum
$leftW = $panelL - $Margin - $work.L
$rightW = $work.R - ($panelR + $Margin)
if ($leftW -ge $rightW) { $x = $work.L; $w = $leftW } else { $x = $panelR + $Margin; $w = $rightW }
if ($w -lt 400) { Write-Output "Free strip is only ${w}px wide; not moving. Ask the user to move the Claude window."; exit 1 }
if ([SubPcWin]::IsZoomed($target)) { [void][SubPcWin]::ShowWindow($target, 9) }
# SWP_NOZORDER | SWP_NOACTIVATE
[void][SubPcWin]::SetWindowPos($target, [IntPtr]::Zero, $x, $work.T, $w, ($work.B - $work.T), 0x14)
$rect = [SubPcWin]::Rect($target)
Write-Output "Moved:     $(Format-Rect $rect)"
if (@($blocking | Where-Object { Test-Overlap $_.Rect $rect }).Count -gt 0) { Write-Output 'OVERLAP remains.'; exit 1 }
Write-Output 'No overlap.'
