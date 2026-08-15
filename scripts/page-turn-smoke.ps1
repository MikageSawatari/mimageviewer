# Automated page-turn display smoke test.
#
# Scripted runs use --test-script and an isolated --data-dir. They do not use
# SendInput or the normal single-instance mutex. Host-side window activation is
# limited to establishing the focus precondition checked by wait_until.
#
# Self-test (fully unattended):
#   .\scripts\build-dev.ps1 -TestScript
#   .\scripts\page-turn-smoke.ps1 -SelfTest
#
# Real page-turn measurement setup (human prepares the actual book once):
#   .\scripts\page-turn-smoke.ps1 -Setup
# Then run the measurement unattended:
#   .\scripts\page-turn-smoke.ps1
#
# NOTE: this file is ASCII-only on purpose. PowerShell 5.1 reads BOM-less UTF-8
# as the ANSI codepage, so non-ASCII comments break parsing (CLAUDE.md).

[CmdletBinding()]
param(
    [switch]$Setup,
    [switch]$SelfTest,
    # Run an arbitrary script against an arbitrary folder in a throwaway profile. The scripted
    # focus rules and the every-100ms refocus are the reason to come through here rather than
    # launching the exe directly: a burst that changes documents loses its synthetic key target
    # otherwise, and the run fails for a reason that has nothing to do with what is being tested.
    [string]$Script,
    [string]$Folder,
    [string]$ScriptDataDir = "target\page-turn-smoke\script-data",
    # JSON overlay applied to the loaded settings, so a scenario can state the configuration it
    # needs instead of depending on whatever the profile happens to have. Inline JSON or a path.
    [string]$SettingsOverride,
    [string]$SelfTestRoot = "target\page-turn-smoke\selftest",
    [string]$DataDir = "target\page-turn-smoke\data",
    [string]$Exe = "target\dev-runtime\mimageviewer-core.exe",
    [ValidateSet("Right", "Left", "Both")]
    [string]$Direction = "Both",
    [double]$HoldSeconds = 5.0,
    [double]$RepeatHz = 30.0,
    [int]$RunTimeoutSeconds = 90
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

function Resolve-RepoPath {
    param([string]$Path)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
}

function Quote-NativeArgument {
    param([string]$Value)
    if ($Value.Contains('"')) {
        throw "native argument contains a quote: $Value"
    }
    return '"' + $Value + '"'
}

function Join-NativeArguments {
    param([string[]]$Values)
    return (($Values | ForEach-Object { Quote-NativeArgument $_ }) -join " ")
}

function Assert-NoOtherInstanceForSetup {
    # Setup is deliberately a normal, non-scripted run so a human can edit the
    # isolated profile. Normal runs still use the process-wide mutex. Scripted
    # runs skip it in src/lib.rs and must not call this check.
    $running = @(Get-Process -ErrorAction SilentlyContinue |
        Where-Object { $_.ProcessName -in @("mimageviewer", "mimageviewer-core") })
    if ($running.Count -gt 0) {
        $ids = ($running | ForEach-Object { "{0}({1})" -f $_.ProcessName, $_.Id }) -join ", "
        throw "mImageViewer is already running: $ids. Close it before -Setup only."
    }
}

if (-not ("MivSmokeWindow" -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;

public static class MivSmokeWindow
{
    delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [StructLayout(LayoutKind.Sequential)]
    struct RECT { public int Left, Top, Right, Bottom; }

    [DllImport("user32.dll")]
    static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
    [DllImport("user32.dll")]
    static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")]
    static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")]
    static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")]
    static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("kernel32.dll")]
    static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")]
    static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool attach);
    [DllImport("user32.dll")]
    static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")]
    static extern bool BringWindowToTop(IntPtr hWnd);
    [DllImport("user32.dll")]
    static extern IntPtr SetFocus(IntPtr hWnd);

    public static bool Focus(IntPtr hWnd)
    {
        IntPtr foreground = GetForegroundWindow();
        uint ignored;
        uint foregroundThread = GetWindowThreadProcessId(foreground, out ignored);
        uint targetThread = GetWindowThreadProcessId(hWnd, out ignored);
        uint currentThread = GetCurrentThreadId();
        bool attachedForeground = foregroundThread != 0 && foregroundThread != currentThread
            && AttachThreadInput(currentThread, foregroundThread, true);
        bool attachedTarget = targetThread != 0 && targetThread != currentThread
            && AttachThreadInput(currentThread, targetThread, true);
        BringWindowToTop(hWnd);
        bool focused = SetForegroundWindow(hWnd);
        SetFocus(hWnd);
        if (attachedTarget) {
            AttachThreadInput(currentThread, targetThread, false);
        }
        if (attachedForeground) {
            AttachThreadInput(currentThread, foregroundThread, false);
        }
        return focused;
    }

    public static bool FocusLargestVisibleWindow(uint processId)
    {
        IntPtr candidate = IntPtr.Zero;
        long candidateArea = -1;
        EnumWindows(delegate(IntPtr hWnd, IntPtr ignored) {
            uint owner;
            GetWindowThreadProcessId(hWnd, out owner);
            if (owner != processId || !IsWindowVisible(hWnd)) {
                return true;
            }
            RECT rect;
            if (!GetWindowRect(hWnd, out rect)) {
                return true;
            }
            long width = Math.Max(0, rect.Right - rect.Left);
            long height = Math.Max(0, rect.Bottom - rect.Top);
            long area = width * height;
            if (area > candidateArea) {
                candidate = hWnd;
                candidateArea = area;
            }
            return true;
        }, IntPtr.Zero);
        return candidate != IntPtr.Zero && Focus(candidate);
    }

    public static bool IsForeground(IntPtr hWnd) { return GetForegroundWindow() == hWnd; }
}
'@
}

function Focus-ScriptedWindow {
    param([System.Diagnostics.Process]$Process)
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline) {
        $Process.Refresh()
        if ($Process.HasExited) {
            return
        }
        if ($Process.MainWindowHandle -ne 0) {
            $null = [MivSmokeWindow]::FocusLargestVisibleWindow([uint32]$Process.Id)
            Start-Sleep -Milliseconds 100
            $Process.Refresh()
            if ($Process.MainWindowHandle -ne 0 -and
                [MivSmokeWindow]::IsForeground($Process.MainWindowHandle)) {
                return
            }
        }
        Start-Sleep -Milliseconds 100
    }
}

function Invoke-Analyzer {
    param(
        [string]$PerfLog,
        [string]$Mode
    )
    $output = & python scripts/analyze_perf.py $PerfLog $Mode --check 2>&1
    $exitCode = $LASTEXITCODE
    $output | ForEach-Object { Write-Host $_ }
    return $exitCode
}

function Invoke-ScriptedRun {
    param(
        [string]$RunDataDir,
        [string]$ScriptPath,
        [string]$StartupFolder,
        [string[]]$AnalyzerModes,
        [string]$SettingsJson
    )

    $null = New-Item -ItemType Directory -Force -Path $RunDataDir
    $perfLog = Join-Path $RunDataDir "logs\perf_events.jsonl"
    if (Test-Path -LiteralPath $perfLog) {
        Remove-Item -LiteralPath $perfLog -Force
    }

    $arguments = @(
        "--data-dir", $RunDataDir,
        "--perf-log",
        "--test-script", $ScriptPath
    )
    if ($SettingsJson) {
        # Inline JSON cannot go on the command line: Join-NativeArguments refuses quotes, and
        # rightly so. Write it beside the run instead, which also leaves the exact configuration
        # next to the log it produced.
        $overlayPath = Join-Path $RunDataDir "settings-override.json"
        $sourceIsOverlay = $false
        $sourceExists = Test-Path -LiteralPath $SettingsJson
        if ($sourceExists) {
            $sourceIsOverlay = ((Resolve-RepoPath $SettingsJson) -eq [System.IO.Path]::GetFullPath($overlayPath))
        }
        if ($sourceIsOverlay) {
            # Already where it needs to be. Copying a file onto itself is not a no-op here: the
            # else branch below would then write the *path* as the file body, and the run would
            # fail on invalid JSON with the configuration silently destroyed.
            Write-Host "  settings : (reusing overlay in place)"
        } elseif ($sourceExists) {
            Copy-Item -LiteralPath $SettingsJson -Destination $overlayPath -Force
        } else {
            # No BOM: PowerShell 5.1's [Text.Encoding]::UTF8 writes one, and a BOM is not JSON.
            $noBom = New-Object System.Text.UTF8Encoding($false)
            [System.IO.File]::WriteAllText($overlayPath, $SettingsJson, $noBom)
        }
        Write-Host "  settings : $overlayPath"
        $arguments += @("--settings-override", $overlayPath)
    }
    if ($StartupFolder) {
        $arguments += $StartupFolder
    }

    Write-Host "[page-turn-smoke] starting scripted core"
    Write-Host "  data-dir : $RunDataDir"
    Write-Host "  script   : $ScriptPath"
    $proc = Start-Process -FilePath $exeFull `
        -ArgumentList (Join-NativeArguments $arguments) -PassThru
    # Synthetic routing deliberately follows the production foreground/focus
    # rules. Bring the new test window forward without injecting any key input;
    # wait_until still proves that egui observed the focused target.
    Focus-ScriptedWindow -Process $proc
    $runDeadline = (Get-Date).AddSeconds($RunTimeoutSeconds)
    while (-not $proc.HasExited -and (Get-Date) -lt $runDeadline) {
        # Opening fullscreen creates another top-level HWND. Keep the largest
        # visible window owned by this exact test PID focused so the script can
        # satisfy the same production focus gate across viewport transitions.
        $null = [MivSmokeWindow]::FocusLargestVisibleWindow([uint32]$proc.Id)
        Start-Sleep -Milliseconds 100
        $proc.Refresh()
    }
    if (-not $proc.HasExited) {
        Write-Host "[page-turn-smoke] scripted core timed out"
        $proc.Kill()
        $proc.WaitForExit()
        $appExit = 124
    } else {
        $appExit = $proc.ExitCode
    }
    Write-Host "[page-turn-smoke] app exit : $appExit"

    $analyzerExit = 0
    if (-not (Test-Path -LiteralPath $perfLog)) {
        Write-Host "[page-turn-smoke] no perf log was written: $perfLog"
        $analyzerExit = 2
    } else {
        foreach ($mode in $AnalyzerModes) {
            Write-Host "[page-turn-smoke] analyzer : $mode --check"
            $modeExit = Invoke-Analyzer -PerfLog $perfLog -Mode $mode
            if ($modeExit -ne 0 -and $analyzerExit -eq 0) {
                $analyzerExit = $modeExit
            }
        }
    }

    Write-Host "[page-turn-smoke] perf log : $perfLog"
    if ($appExit -eq 0 -and $analyzerExit -eq 0) {
        Write-Host "[page-turn-smoke] PASS"
        return 0
    }
    Write-Host "[page-turn-smoke] FAIL (app=$appExit analyzer=$analyzerExit)"
    return $(if ($appExit -ne 0) { $appExit } else { $analyzerExit })
}

$exeFull = Resolve-RepoPath $Exe
if (-not (Test-Path -LiteralPath $exeFull)) {
    throw "$exeFull not found. Run .\scripts\build-dev.ps1 -TestScript first."
}

if ($Setup -and $SelfTest) {
    throw "-Setup and -SelfTest cannot be combined"
}
if ($HoldSeconds -le 0 -or $RepeatHz -le 0) {
    throw "HoldSeconds and RepeatHz must be greater than zero"
}

if ($Setup) {
    Assert-NoOtherInstanceForSetup
    $dataDirFull = Resolve-RepoPath $DataDir
    $null = New-Item -ItemType Directory -Force -Path $dataDirFull
    Write-Host "[page-turn-smoke] setup: starting normal core on isolated data-dir"
    Write-Host "  data-dir : $dataDirFull"
    Write-Host "  Open the real folder/PDF, configure colorize or adjustments,"
    Write-Host "  leave a middle page selected in the grid, then exit mImageViewer."
    $setupArgs = Join-NativeArguments @("--data-dir", $dataDirFull)
    $proc = Start-Process -FilePath $exeFull -ArgumentList $setupArgs -PassThru
    $proc.WaitForExit()
    if ($proc.ExitCode -ne 0) {
        throw "setup core exited with $($proc.ExitCode)"
    }
    if (-not (Test-Path -LiteralPath (Join-Path $dataDirFull "settings.db"))) {
        throw "setup produced no settings.db in $dataDirFull"
    }
    Write-Host "[page-turn-smoke] setup saved"
    exit 0
}

if ($SelfTest) {
    $selfRoot = Resolve-RepoPath $SelfTestRoot
    $allowedRoot = (Resolve-RepoPath "target").TrimEnd('\') + '\'
    $selfRootCheck = $selfRoot.TrimEnd('\') + '\'
    if (-not $selfRootCheck.StartsWith($allowedRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "refusing to use self-test path outside $allowedRoot"
    }
    $fixtureDir = Join-Path $selfRoot "fixture"
    $dataDirFull = Join-Path $selfRoot "data"
    foreach ($resetPath in @($fixtureDir, $dataDirFull)) {
        if (Test-Path -LiteralPath $resetPath) {
            Remove-Item -LiteralPath $resetPath -Recurse -Force
        }
    }
    & python scripts/page-turn/generate_fixture.py $fixtureDir
    if ($LASTEXITCODE -ne 0) {
        throw "fixture generator failed with exit $LASTEXITCODE"
    }
    $scriptFull = Resolve-RepoPath "scripts\page-turn\selftest.rhai"
    $exitCode = Invoke-ScriptedRun -RunDataDir $dataDirFull `
        -ScriptPath $scriptFull -StartupFolder $fixtureDir `
        -AnalyzerModes @("test-script-input")
    exit $exitCode
}

if ($Script) {
    $scriptFull = Resolve-RepoPath $Script
    if (-not (Test-Path -LiteralPath $scriptFull)) {
        throw "no such script: $scriptFull"
    }
    $runDataDir = Resolve-RepoPath $ScriptDataDir
    if (Test-Path -LiteralPath $runDataDir) {
        Remove-Item -LiteralPath $runDataDir -Recurse -Force
    }
    $startupFolder = ""
    if ($Folder) {
        $startupFolder = [System.IO.Path]::GetFullPath($Folder)
        if (-not (Test-Path -LiteralPath $startupFolder)) {
            throw "no such folder: $startupFolder"
        }
    }
    $exitCode = Invoke-ScriptedRun -RunDataDir $runDataDir `
        -ScriptPath $scriptFull -StartupFolder $startupFolder `
        -AnalyzerModes @() -SettingsJson $SettingsOverride
    exit $exitCode
}

$dataDirFull = Resolve-RepoPath $DataDir
if (-not (Test-Path -LiteralPath (Join-Path $dataDirFull "settings.db"))) {
    throw "no settings in $dataDirFull. Run .\scripts\page-turn-smoke.ps1 -Setup first."
}

$measurementScript = Join-Path $dataDirFull "page-turn-measure.rhai"
$holdMs = [int][Math]::Round($HoldSeconds * 1000.0)
$directions = if ($Direction -eq "Both") { @("Right", "Left") } else { @($Direction) }
$scriptLines = @(
    'log("measurement: wait for grid");',
    'wait_until(|s| s.target_registered && s.target_rendered && s.focused && !s.is_fullscreen, 20000);',
    'run_action("GridOpenSelected");',
    'wait_until(|s| s.is_fullscreen && s.target_registered && s.target_rendered && s.focused && s.current_is_still_image && !s.music_view_active && !s.modal_open && !s.context_menu_open && !s.popup_open && !s.ime_active && !s.text_input_or_pending_focus && !s.overlay_edit_active && !s.capture_region_selection && s.fullscreen_raw_key_permit && !s.continuous_reading && s.spread_mode == "Single" && s.has_previous_page && s.has_next_page, 20000);',
    "set_repeat(250, $RepeatHz);"
)
foreach ($directionName in $directions) {
    $scriptLines += "log(`"measurement: hold $directionName`" );"
    $scriptLines += "hold_key(`"$directionName`", $holdMs);"
    $scriptLines += 'sleep(2000);'
}
[System.IO.File]::WriteAllLines($measurementScript, $scriptLines, [System.Text.Encoding]::ASCII)

$exitCode = Invoke-ScriptedRun -RunDataDir $dataDirFull `
    -ScriptPath $measurementScript -StartupFolder "" `
    -AnalyzerModes @("test-script-input", "page-turn")
exit $exitCode
