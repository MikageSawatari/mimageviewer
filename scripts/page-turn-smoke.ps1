# Automated page-turn display smoke test.
#
# Launches core against an ISOLATED data-dir, synthesises a held page-turn key,
# then runs the perf log through `analyze_perf.py page-turn --check`.
# Purpose: stop needing a human to eyeball the same thing over and over.
#
# The spec being checked is docs/display-pipeline.md section 2.5
# ("page-turn display rules"). Invariants I1..I5 are defined there.
#
# The live %APPDATA%\mimageviewer is never touched.
#
# One-time setup (a human does this once):
#   .\scripts\page-turn-smoke.ps1 -Setup
#     -> core starts on the isolated data-dir. Open the folder/PDF you want to
#        test, turn on colorize / adjustments, go fullscreen, then exit.
#        Those settings persist in the isolated data-dir.
#
# After that, unattended:
#   .\scripts\page-turn-smoke.ps1
#
# NOTE: this file is ASCII-only on purpose. PowerShell 5.1 reads BOM-less UTF-8
# as the ANSI codepage, so non-ASCII comments break parsing (CLAUDE.md).

[CmdletBinding()]
param(
    # Setup mode: just launch and wait for the human. No input, no checking.
    [switch]$Setup,

    # Isolated data-dir. Never the live APPDATA one.
    [string]$DataDir = "target\page-turn-smoke\data",

    # Binary under test. Default is what build-dev.ps1 produces.
    [string]$Exe = "target\dev-runtime\mimageviewer-core.exe",

    # Which direction(s) to hold.
    [ValidateSet("Right", "Left", "Both")]
    [string]$Direction = "Both",

    # Seconds to hold the key per direction.
    [double]$HoldSeconds = 5.0,

    # Synthetic repeat rate. Windows default auto-repeat is roughly 30/s.
    [double]$RepeatHz = 30.0,

    # How long to wait for the window to appear.
    [double]$StartupWaitSeconds = 20.0
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

if (-not (Test-Path $Exe)) {
    throw "$Exe not found. Run .\scripts\build-dev.ps1 first."
}

$dataDirFull = Join-Path $repoRoot $DataDir
$null = New-Item -ItemType Directory -Force $dataDirFull
$perfLog = Join-Path $dataDirFull "logs\perf_events.jsonl"

# ---- synthetic keyboard input ----------------------------------------------
# Add-Type cannot redefine a type in the same session, so probe first.
if (-not ("MivSmokeInput" -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;

public static class MivSmokeInput
{
    [StructLayout(LayoutKind.Sequential)]
    struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }

    // Win32 INPUT is a union whose largest member is MOUSEINPUT, so on x64 the
    // whole struct is 40 bytes even when only the keyboard member is used.
    // Declaring just KEYBDINPUT yields 32, and SendInput then rejects every call
    // with ERROR_INVALID_PARAMETER because cbSize does not match.
    [StructLayout(LayoutKind.Explicit, Size = 40)]
    struct INPUT { [FieldOffset(0)] public uint type; [FieldOffset(8)] public KEYBDINPUT ki; }

    [DllImport("user32.dll", SetLastError = true)]
    static extern uint SendInput(uint nInputs, INPUT[] pInputs, int cbSize);

    const uint INPUT_KEYBOARD = 1;
    const uint KEYEVENTF_KEYUP = 0x0002;
    const uint KEYEVENTF_EXTENDEDKEY = 0x0001;

    public static uint LastSent;
    public static int LastError;

    static void Send(ushort vk, bool up, bool extended)
    {
        INPUT[] inputs = new INPUT[1];
        inputs[0].type = INPUT_KEYBOARD;
        inputs[0].ki.wVk = vk;
        // Arrow keys are extended keys; Enter is not (the app distinguishes main
        // Enter from numpad Enter). Press and release must carry the same flag or
        // the held state does not match a physical key.
        inputs[0].ki.dwFlags = (extended ? KEYEVENTF_EXTENDEDKEY : 0) | (up ? KEYEVENTF_KEYUP : 0);
        LastSent = SendInput(1, inputs, Marshal.SizeOf(typeof(INPUT)));
        LastError = Marshal.GetLastWin32Error();
    }

    public static void KeyDown(ushort vk) { Send(vk, false, true); }
    public static void KeyUp(ushort vk) { Send(vk, true, true); }
    public static void Tap(ushort vk) { Send(vk, false, false); Send(vk, true, false); }

    [DllImport("user32.dll")]
    static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")]
    static extern IntPtr GetForegroundWindow();

    public static bool Focus(IntPtr hWnd) { return SetForegroundWindow(hWnd); }
    public static bool IsForeground(IntPtr hWnd) { return GetForegroundWindow() == hWnd; }
    public static IntPtr Foreground() { return GetForegroundWindow(); }

    [DllImport("user32.dll", SetLastError = true)]
    static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
    public static uint ForegroundPid()
    {
        uint pid;
        GetWindowThreadProcessId(GetForegroundWindow(), out pid);
        return pid;
    }
}
'@
}

$VK = @{ Right = [uint16]0x27; Left = [uint16]0x25 }

function Invoke-HeldKey {
    param([string]$Name, [double]$Seconds, [double]$Hz)

    $vk = $VK[$Name]
    $intervalMs = [int](1000.0 / $Hz)
    $deadline = (Get-Date).AddSeconds($Seconds)

    # Send KeyUp exactly once, at the end, so the key looks physically held for
    # the whole window. The repeated KeyDown in between stands in for OS
    # auto-repeat, which SendInput does not generate on its own.
    [MivSmokeInput]::KeyDown($vk)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds $intervalMs
        [MivSmokeInput]::KeyDown($vk)
    }
    [MivSmokeInput]::KeyUp($vk)
}

function Assert-NoOtherInstance {
    # The single-instance mutex name is a compile-time constant keyed only on the
    # portable feature, NOT on the data-dir. So a running mImageViewer - installed,
    # tray-resident, or a dev build - will swallow our launch: the new process
    # hands the path over to the existing instance and exits, and nothing is ever
    # written to the isolated data-dir.
    #
    # Detect that here instead of leaving a confusing empty directory behind.
    $running = @(Get-Process -ErrorAction SilentlyContinue |
        Where-Object { $_.ProcessName -in @("mimageviewer", "mimageviewer-core") })
    if ($running.Count -gt 0) {
        $ids = ($running | ForEach-Object { "{0}({1})" -f $_.ProcessName, $_.Id }) -join ", "
        throw @"
mImageViewer is already running: $ids

Close every mImageViewer first, including the tray-resident one. The
single-instance mutex does not distinguish data directories, so this run would
be handed to the existing instance and the isolated data-dir would stay empty.
"@
    }
}

function Wait-ForWindow {
    param([System.Diagnostics.Process]$Process, [double]$Seconds)

    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        $Process.Refresh()
        if ($Process.HasExited) { throw "core exited during startup (exit $($Process.ExitCode))" }
        if ($Process.MainWindowHandle -ne 0) { return $Process.MainWindowHandle }
        Start-Sleep -Milliseconds 250
    }
    throw "no window appeared within $Seconds seconds"
}

# ---- setup mode -------------------------------------------------------------
if ($Setup) {
    Assert-NoOtherInstance
    Write-Host "[page-turn-smoke] setup: starting core on the isolated data-dir"
    Write-Host "  data-dir : $dataDirFull"
    Write-Host ""
    Write-Host "  Do this, then exit mImageViewer:"
    Write-Host "    1. open the folder / PDF you want the test to run against"
    Write-Host "    2. turn on colorize / colour adjustment as you want them measured"
    Write-Host "    3. go fullscreen so page-turn keys apply"
    Write-Host "    4. exit (settings and the reopen position persist here)"
    Write-Host ""
    $p = Start-Process -FilePath $Exe -ArgumentList @("--data-dir", $dataDirFull) -PassThru
    $p.WaitForExit()
    if (-not (Test-Path (Join-Path $dataDirFull "settings.db"))) {
        throw "setup produced no settings.db in $dataDirFull - the launch was probably handed to another instance"
    }
    Write-Host "[page-turn-smoke] setup saved. Later runs need no arguments."
    exit 0
}

if (-not (Test-Path (Join-Path $dataDirFull "settings.db"))) {
    throw "no settings in $dataDirFull. Run .\scripts\page-turn-smoke.ps1 -Setup first."
}

# ---- measure ----------------------------------------------------------------
if (Test-Path $perfLog) { Remove-Item $perfLog -Force }

Assert-NoOtherInstance

Write-Host "[page-turn-smoke] starting core (isolated data-dir + --perf-log)"
$proc = Start-Process -FilePath $Exe -ArgumentList @("--data-dir", $dataDirFull, "--perf-log") -PassThru
try {
    $hwnd = Wait-ForWindow -Process $proc -Seconds $StartupWaitSeconds
    Write-Host "[page-turn-smoke] window is up; settling for 3 seconds"
    Start-Sleep -Seconds 3

    # Synthetic keys go to whatever owns the foreground, so make sure that is the
    # window under test. Sending page-turn keys into some other application would
    # be both useless and rude.
    $null = [MivSmokeInput]::Focus($hwnd)
    Start-Sleep -Milliseconds 500
    if (-not [MivSmokeInput]::IsForeground($hwnd)) {
        throw "could not bring the mImageViewer window to the foreground; refusing to send input"
    }

    # Page-turn keys only apply in fullscreen. Enter opens the selected item, so
    # press it rather than depending on whatever view state the fixture saved.
    Write-Host "[page-turn-smoke] entering fullscreen (Enter)"
    [MivSmokeInput]::Tap([uint16]0x0D)
    Write-Host ("[page-turn-smoke] SendInput inserted={0} lastError={1} fgPid={2} ourPid={3}" -f `
        [MivSmokeInput]::LastSent, [MivSmokeInput]::LastError, [MivSmokeInput]::ForegroundPid(), $proc.Id)
    if ([MivSmokeInput]::LastSent -eq 0) {
        throw "SendInput was blocked (inserted=0, lastError=$([MivSmokeInput]::LastError)). Synthetic input cannot reach the app from this session."
    }
    Start-Sleep -Seconds 4

    $directions = switch ($Direction) {
        "Both"  { @("Right", "Left") }
        default { @($Direction) }
    }

    foreach ($d in $directions) {
        Write-Host "[page-turn-smoke] holding $d for $HoldSeconds s at $RepeatHz Hz"
        Invoke-HeldKey -Name $d -Seconds $HoldSeconds -Hz $RepeatHz
        # Let the release settle so I4 (ends on the composite) is observable.
        Start-Sleep -Seconds 2
    }
}
finally {
    if (-not $proc.HasExited) {
        Write-Host "[page-turn-smoke] closing core"
        $null = $proc.CloseMainWindow()
        if (-not $proc.WaitForExit(10000)) { $proc.Kill() }
    }
}

if (-not (Test-Path $perfLog)) { throw "no perf log was written: $perfLog" }

Write-Host ""
Write-Host "[page-turn-smoke] checking invariants I1..I5 (docs/display-pipeline.md 2.5)"
$checkOutput = & python scripts/analyze_perf.py $perfLog page-turn --check 2>&1
$checkExit = $LASTEXITCODE
$checkOutput | ForEach-Object { Write-Host $_ }

# A run that observed no page turns proves nothing. Treat it as a failure of the
# harness, not as a pass, or a broken build would slip through silently.
if ($checkOutput -match "checked bursts=0") {
    Write-Host ""
    Write-Host "[page-turn-smoke] no page-turn bursts were observed - the app was probably"
    Write-Host "                  not in fullscreen, or the keys went somewhere else."
    $checkExit = 2
}

Write-Host ""
Write-Host "[page-turn-smoke] perf log : $perfLog"
if ($checkExit -eq 0) { Write-Host "[page-turn-smoke] PASS" } else { Write-Host "[page-turn-smoke] FAIL" }
exit $checkExit
