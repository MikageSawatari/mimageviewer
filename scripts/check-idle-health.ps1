[CmdletBinding()]
param(
    [ValidateSet("static-foreground", "static-background", "video-pin-background", "tray-residency")]
    [string]$Scenario = "static-foreground",
    [string]$TargetKey = "",
    [string]$ExePath = "",
    [int]$ProcessId = 0,
    [switch]$NoLaunch,
    [switch]$SkipPrompt,
    [string]$ThresholdPath = "",
    [string]$OutputDirectory = "",
    [switch]$PortableSmoke,
    [switch]$InteractiveApproved,
    [datetime]$ExpectedStartTimeUtc,
    [string]$SleepEvidencePath = "",
    [long]$RootHwnd = 0,
    [long]$BackgroundHwnd = 0,
    [int]$BackgroundProcessId = 0,
    [string]$CaseId = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0

# This branch cannot launch or attach by name, or fall back to APPDATA logs.
# Reject before even compiling native helpers or creating output directories.
if ($PortableSmoke) {
    if (-not $InteractiveApproved -or $ProcessId -le 0 -or -not $SkipPrompt -or
        -not $PSBoundParameters.ContainsKey('ExpectedStartTimeUtc') -or
        [string]::IsNullOrWhiteSpace($SleepEvidencePath) -or
        [string]::IsNullOrWhiteSpace($OutputDirectory) -or
        -not [string]::IsNullOrWhiteSpace($ExePath) -or
        -not [string]::IsNullOrWhiteSpace($ThresholdPath) -or $NoLaunch) {
        [Console]::Error.WriteLine('PortableSmoke requires approval, exact PID/start time, SkipPrompt, sleep evidence and run output; executable/threshold overrides and NoLaunch are refused.')
        exit 2
    }
    & (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Sampler.ps1') `
        -Scenario $Scenario -TargetKey $TargetKey -ProcessId $ProcessId `
        -ExpectedStartTimeUtc $ExpectedStartTimeUtc -SleepEvidencePath $SleepEvidencePath `
        -OutputDirectory $OutputDirectory -RootHwnd $RootHwnd -BackgroundHwnd $BackgroundHwnd `
        -BackgroundProcessId $BackgroundProcessId -CaseId $CaseId -InteractiveApproved
    exit $LASTEXITCODE
}
if ($InteractiveApproved -or $PSBoundParameters.ContainsKey('ExpectedStartTimeUtc') -or
    $PSBoundParameters.ContainsKey('SleepEvidencePath') -or $PSBoundParameters.ContainsKey('RootHwnd') -or
    $PSBoundParameters.ContainsKey('BackgroundHwnd') -or $PSBoundParameters.ContainsKey('BackgroundProcessId') -or $PSBoundParameters.ContainsKey('CaseId')) {
    [Console]::Error.WriteLine('ReleaseChecks execution options require -PortableSmoke; they do not authorize normal-profile launch.')
    exit 2
}

if ($Scenario -eq "video-pin-background") {
    if ([string]::IsNullOrWhiteSpace($TargetKey)) {
        throw ("video-pin-background requires -TargetKey. Pass the path of the " +
            "folder whose representative image is pinned to a video. The value " +
            "is matched as a case-insensitive substring of perf-log thumbnail keys.")
    }
}
elseif ($PSBoundParameters.ContainsKey("TargetKey")) {
    throw "-TargetKey is valid only with -Scenario video-pin-background"
}

# Add-Type keeps a compiled type for the whole PowerShell session, and this
# guard skips recompiling it. A console that already ran an older copy of this
# script therefore keeps the OLD implementation, silently. Bump the trailing
# number whenever the C# below changes so a stale session recompiles instead of
# reporting results from code that is no longer in the file. (2026-08-04: the
# tray gate fix looked like it had not applied for exactly this reason.)
if (-not ("MivIdleHealthNative2" -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class MivIdleHealthNative2 {
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetClassName(IntPtr hWnd, StringBuilder buffer, int maxCount);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }

    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);

    // Framework-internal top-level windows that the app cannot hide and that the
    // user never sees. Counting them makes the tray gate unpassable:
    //   Winit Thread Event Target - winit's 15x15 event sink, always WS_VISIBLE
    //   IME / MSCTFIME UI         - the per-thread IME helpers Windows creates
    // Verified 2026-08-04 by enumerating a tray-resident core: hiding the real
    // main window took the visible count from 2 to 1, and the survivor was the
    // winit event target.
    static readonly string[] HelperClasses = {
        "Winit Thread Event Target",
        "IME",
        "MSCTFIME UI",
    };

    static bool IsUserFacing(IntPtr hWnd) {
        if (!IsWindowVisible(hWnd)) { return false; }
        var name = new StringBuilder(256);
        GetClassName(hWnd, name, name.Capacity);
        string cls = name.ToString();
        foreach (string helper in HelperClasses) {
            if (cls == helper) { return false; }
        }
        // Backstop for helpers we have not named: nothing the user can see is tiny.
        RECT r;
        if (GetWindowRect(hWnd, out r) && (r.Right - r.Left < 64 || r.Bottom - r.Top < 64)) {
            return false;
        }
        return true;
    }

    public static int[] GetTopLevelWindowCounts(uint processId) {
        int total = 0;
        int visible = 0;
        EnumWindows(delegate (IntPtr hWnd, IntPtr lParam) {
            uint ownerProcessId;
            GetWindowThreadProcessId(hWnd, out ownerProcessId);
            if (ownerProcessId == processId) {
                total++;
                if (IsUserFacing(hWnd)) {
                    visible++;
                }
            }
            return true;
        }, IntPtr.Zero);
        return new int[] { total, visible };
    }
}
'@
}

# The transport and prefix reader share bounded control/snapshot contracts.
if (-not ("MivLogDelivery1" -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;
public static class MivLogDelivery1 {
    [StructLayout(LayoutKind.Sequential)]
    struct Info {
        public uint Attributes, CreationLow, CreationHigh, AccessLow, AccessHigh;
        public uint WriteLow, WriteHigh, Volume, SizeHigh, SizeLow, Links, IndexHigh, IndexLow;
    }
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool GetFileInformationByHandle(SafeFileHandle handle, out Info info);
    static int Remaining(Stopwatch clock) {
        int remaining = 2000 - (int)clock.ElapsedMilliseconds;
        if (remaining <= 0) throw new TimeoutException("logging-control deadline expired");
        return remaining;
    }
    static void Wait(IAsyncResult task, Stopwatch clock) {
        if (!task.AsyncWaitHandle.WaitOne(Remaining(clock))) throw new TimeoutException("logging-control timeout");
    }
    static byte[] Read(PipeStream pipe, int length, Stopwatch clock) {
        var bytes = new byte[length]; int offset = 0;
        while (offset < length) {
            var read = pipe.BeginRead(bytes, offset, length - offset, null, null);
            Wait(read, clock); int count = pipe.EndRead(read); read.AsyncWaitHandle.Close();
            if (count == 0) throw new EndOfStreamException("logging-control disconnected");
            offset += count;
        }
        return bytes;
    }
    public static string Request(string name, string json) {
        var clock = Stopwatch.StartNew();
        using (var pipe = new NamedPipeClientStream(".", name, PipeDirection.InOut, PipeOptions.Asynchronous)) {
            pipe.Connect(Remaining(clock));
            byte[] payload = Encoding.UTF8.GetBytes(json);
            byte[] packet = new byte[payload.Length + 4];
            Buffer.BlockCopy(BitConverter.GetBytes((uint)payload.Length), 0, packet, 0, 4);
            Buffer.BlockCopy(payload, 0, packet, 4, payload.Length);
            var write = pipe.BeginWrite(packet, 0, packet.Length, null, null);
            Wait(write, clock); pipe.EndWrite(write); write.AsyncWaitHandle.Close();
            uint length = BitConverter.ToUInt32(Read(pipe, 4, clock), 0);
            if (length == 0 || length > 65536) throw new InvalidDataException("invalid logging-control response size");
            return new UTF8Encoding(false, true).GetString(Read(pipe, (int)length, clock));
        }
    }
    public static string Identity(SafeFileHandle handle) {
        Info info;
        if (!GetFileInformationByHandle(handle, out info)) throw new IOException("cannot obtain perf file identity");
        return info.Volume.ToString() + ":" + (((ulong)info.IndexHigh << 32) | info.IndexLow).ToString();
    }
    public static void Snapshot(string path, string identity, long length, string destination) {
        if (length < 0) throw new InvalidDataException("invalid snapshot offset");
        using (var source = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete)) {
            string actual = Identity(source.SafeFileHandle);
            if (actual != identity || source.Length < length) throw new InvalidDataException("perf snapshot identity/length mismatch");
            using (var target = new FileStream(destination, FileMode.CreateNew, FileAccess.Write, FileShare.Read)) {
                var buffer = new byte[65536]; long remaining = length;
                while (remaining > 0) {
                    int count = source.Read(buffer, 0, (int)Math.Min(buffer.Length, remaining));
                    if (count == 0) throw new EndOfStreamException("perf prefix disappeared");
                    target.Write(buffer, 0, count); remaining -= count;
                }
            }
        }
    }
}
'@
}

function Get-DeliveryCheckpoint {
    param($Session, [string]$RequestId)
    if ($null -eq $Session -or $Session.log_delivery_schema -ne 1 -or
        [string]::IsNullOrWhiteSpace([string]$Session.session_id)) {
        throw "delivery-aware session.start is unavailable"
    }
    $request = [ordered]@{
        schema = 1; request_id = $RequestId; pid = [uint32]$Session.pid
        process_start_unix_ms = [uint64]$Session.process_start_unix_ms
        session_id = [string]$Session.session_id
    }
    $name = "mimageviewer-log-$($Session.pid)-$($Session.process_start_unix_ms)"
    $ack = [MivLogDelivery1]::Request($name, ($request | ConvertTo-Json -Compress)) | ConvertFrom-Json
    if ($null -ne $ack.PSObject.Properties["error"]) { throw ([string]$ack.error) }
    if ($ack.schema -ne 1 -or $ack.request_id -ne $RequestId -or
        $ack.pid -ne $Session.pid -or $ack.session_id -ne $Session.session_id -or
        $ack.process_start_unix_ms -ne $Session.process_start_unix_ms) {
        throw "logging-control ACK identity/nonce mismatch"
    }
    return $ack
}

function Get-DeliveredLogGrowth {
    param($Start, $End)
    foreach ($field in @("session_id", "pid", "process_start_unix_ms")) {
        if ($null -eq $Start.PSObject.Properties[$field] -or
            $null -eq $End.PSObject.Properties[$field] -or $Start.$field -ne $End.$field) {
            throw "log byte watermarks belong to different or unknown processes"
        }
    }
    $growth = @{}
    foreach ($sink in @("normal", "perf")) {
        $field = $sink + "_written_bytes"
        $status = $sink + "_status"
        foreach ($checkpoint in @($Start, $End)) {
            if ($null -eq $checkpoint.PSObject.Properties[$status] -or $checkpoint.$status -ne "ready" -or
                $null -eq $checkpoint.PSObject.Properties[$field] -or
                ($checkpoint.$field -isnot [int] -and $checkpoint.$field -isnot [long] -and $checkpoint.$field -isnot [uint64]) -or
                [decimal]$checkpoint.$field -lt 0) {
                throw "unknown or invalid completed log byte watermark: $field"
            }
        }
        if ([decimal]$End.$field -lt [decimal]$Start.$field) { throw "log byte watermark regressed: $field" }
        $growth[$sink] = [decimal]$End.$field - [decimal]$Start.$field
    }
    [pscustomobject]$growth
}

function Save-DeliveryPrefix {
    param($Ack, [string]$Destination)
    [MivLogDelivery1]::Snapshot([string]$Ack.perf_path, [string]$Ack.perf_file_identity,
        [long]$Ack.perf_end_offset, $Destination)
}

function Get-FileLength {
    param([string]$Path)
    if (Test-Path -LiteralPath $Path) {
        return (Get-Item -LiteralPath $Path).Length
    }
    return 0
}

function Get-ForegroundProcessId {
    $window = [MivIdleHealthNative2]::GetForegroundWindow()
    if ($window -eq [IntPtr]::Zero) {
        return 0
    }
    [uint32]$foregroundProcessId = 0
    [void][MivIdleHealthNative2]::GetWindowThreadProcessId(
        $window,
        [ref]$foregroundProcessId
    )
    return [int]$foregroundProcessId
}

function Get-ProcessTopLevelWindowSummary {
    param([int]$Id)
    $counts = [MivIdleHealthNative2]::GetTopLevelWindowCounts([uint32]$Id)
    return [pscustomobject]@{
        Total = [int]$counts[0]
        Visible = [int]$counts[1]
    }
}

function Format-Invariant {
    param([double]$Value)
    return $Value.ToString("0.000", [System.Globalization.CultureInfo]::InvariantCulture)
}

function Get-LiveProcess {
    param([int]$Id)
    try {
        $process = Get-Process -Id $Id -ErrorAction Stop
        $process.Refresh()
        if ($process.HasExited) {
            throw "process exited"
        }
        return $process
    }
    catch {
        throw "mImageViewer process $Id is not running"
    }
}

function Get-MeasuredProcessInfo {
    param([System.Diagnostics.Process]$Process)
    $commandLine = $null
    $path = [string]$Process.Path
    try {
        $cim = Get-CimInstance Win32_Process -Filter "ProcessId=$($Process.Id)" -ErrorAction Stop
        if ($null -ne $cim) {
            if (-not [string]::IsNullOrWhiteSpace([string]$cim.CommandLine)) {
                $commandLine = [string]$cim.CommandLine
            }
            if (-not [string]::IsNullOrWhiteSpace([string]$cim.ExecutablePath)) {
                $path = [string]$cim.ExecutablePath
            }
        }
    }
    catch {
        # A denied WMI query must not make the gate unrunnable. Without a command line,
        # the analyzer is deliberately not allowed to treat an empty window as sleep.
    }
    return [pscustomobject]@{
        CommandLine = $commandLine
        Path = $path
    }
}

function Test-InitialIndexScanSettled {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        return $false
    }
    $literal = '"initial_scan_settled"'
    if (-not (Select-String -LiteralPath $Path -Pattern $literal -SimpleMatch -Quiet)) {
        return $false
    }
    foreach ($match in (Select-String -LiteralPath $Path -Pattern $literal -SimpleMatch)) {
        try {
            $event = $match.Line | ConvertFrom-Json
            if ($event.cat -eq "index" -and $event.kind -eq "initial_scan_settled") {
                return $true
            }
        }
        catch {
            # The writer may still be appending the final JSONL line. Retry on the next poll.
        }
    }
    return $false
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($ThresholdPath)) {
    $ThresholdPath = Join-Path $PSScriptRoot "idle_health_thresholds.json"
}
if (-not (Test-Path -LiteralPath $ThresholdPath)) {
    throw "threshold file not found: $ThresholdPath"
}
$Thresholds = Get-Content -LiteralPath $ThresholdPath -Encoding UTF8 -Raw | ConvertFrom-Json
$WarmupSeconds = [int]$Thresholds.warmup_seconds
$MeasureSeconds = [int]$Thresholds.measure_seconds
$IndexSettleWaitSeconds = [int]$Thresholds.index_settle_wait_seconds

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $RepoRoot "target\idle-health"
}
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null

$LogDirectory = Join-Path $env:APPDATA "mimageviewer\logs"
$PerfLog = Join-Path $LogDirectory "perf_events.jsonl"
$AppLog = Join-Path $LogDirectory "mimageviewer.log"
$Launched = $false

if ($ProcessId -gt 0) {
    $Process = Get-LiveProcess -Id $ProcessId
}
elseif ($NoLaunch) {
    if (-not (Test-Path -LiteralPath $PerfLog)) {
        throw "perf log was not found; omit -NoLaunch to start a measured process"
    }
    $SessionRecord = Get-Content -LiteralPath $PerfLog -Encoding UTF8 -TotalCount 64 |
        ForEach-Object {
            try { $_ | ConvertFrom-Json } catch { $null }
        } |
        Where-Object {
            $null -ne $_ -and $_.cat -eq "session" -and $_.kind -eq "start"
        } |
        Select-Object -First 1
    if ($null -eq $SessionRecord -or $null -eq $SessionRecord.pid) {
        throw "perf log has no session PID; start the current verification binary with --perf-log"
    }
    $Process = Get-LiveProcess -Id ([int]$SessionRecord.pid)
}
else {
    if ([string]::IsNullOrWhiteSpace($ExePath)) {
        $ExePath = Join-Path $RepoRoot "target\release\mimageviewer-core.exe"
    }
    if (-not (Test-Path -LiteralPath $ExePath)) {
        throw "verification binary not found: $ExePath`nRun .\scripts\build-release.ps1 first."
    }
    Write-Host "Starting mImageViewer with --perf-log..."
    $Process = Start-Process -FilePath $ExePath -ArgumentList "--perf-log" `
        -WorkingDirectory $RepoRoot -PassThru
    $Launched = $true
}

# Validate every attachment route at one boundary using evidence independent of the perf log.
$MeasuredProcessInfo = Get-MeasuredProcessInfo -Process $Process
$MeasuredPath = [string]$MeasuredProcessInfo.Path
$RuntimeRoot = [System.IO.Path]::GetFullPath(
    (Join-Path (Join-Path $env:APPDATA "mimageviewer") "runtime")
)
$RuntimePrefix = $RuntimeRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) +
    [System.IO.Path]::DirectorySeparatorChar
if (-not [string]::IsNullOrWhiteSpace($MeasuredPath)) {
    $MeasuredPath = [System.IO.Path]::GetFullPath($MeasuredPath)
    if ($MeasuredPath.StartsWith(
            $RuntimePrefix,
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
        throw ("launcher-extracted runtime copies cannot be measured: " + $MeasuredPath)
    }
}
$IdentityEvidence = "path_only"
if (-not [string]::IsNullOrWhiteSpace([string]$MeasuredProcessInfo.CommandLine)) {
    if ($MeasuredProcessInfo.CommandLine.IndexOf(
            "--perf-log",
            [System.StringComparison]::OrdinalIgnoreCase
        ) -lt 0) {
        throw ("The process does not write a perf log and cannot be measured. " +
            "Specify an instance started with --perf-log or omit -NoLaunch.")
    }
    $IdentityEvidence = "command_line"
}
else {
    Write-Warning ("Process command line is unavailable; identity could not be verified. " +
        "Empty measurement windows will not be accepted as sleep.")
}

# Wait for a perf log the measured process actually wrote. Existence alone is not
# enough: APPDATA keeps the previous session's perf_events.jsonl, so on every run
# after the first one a stale file is already there when Start-Process returns,
# the wait loop exits immediately, and the freshness test reads the old timestamp
# before the new process has even initialised.
$PerfFreshFloor = $Process.StartTime.AddSeconds(-2)
$ReadyDeadline = (Get-Date).AddSeconds(30)
while ($true) {
    if ((Test-Path -LiteralPath $PerfLog) -and
        (Get-Item -LiteralPath $PerfLog).LastWriteTime -ge $PerfFreshFloor) {
        break
    }
    if ((Get-Date) -ge $ReadyDeadline) {
        if (Test-Path -LiteralPath $PerfLog) {
            throw ("perf log is still the previous session's: $PerfLog`n" +
                "The measured process wrote nothing. A running mImageViewer " +
                "(installed, tray-resident, or a dev build) owns the single-instance " +
                "mutex, so the process started here forwards its arguments and exits. " +
                "Close every mImageViewer, then retry.")
        }
        throw "perf log was not created: $PerfLog"
    }
    $Process = Get-LiveProcess -Id $Process.Id
    Start-Sleep -Milliseconds 200
}

# This waits for an explicit completion event, not for a time window to absorb index work.
# The limit only stops waiting and reports uncertainty; it is not a PASS/FAIL threshold.
$IndexScanWait = "timeout"
$IndexSettleDeadline = (Get-Date).AddSeconds($IndexSettleWaitSeconds)
Write-Host "Waiting for the initial index scans to settle (up to $IndexSettleWaitSeconds seconds)..."
while ($true) {
    if (Test-InitialIndexScanSettled -Path $PerfLog) {
        $IndexScanWait = "settled"
        break
    }
    if ((Get-Date) -ge $IndexSettleDeadline) {
        break
    }
    $Process = Get-LiveProcess -Id $Process.Id
    Start-Sleep -Milliseconds 500
}
if ($IndexScanWait -eq "timeout") {
    Write-Warning "Initial index scan completion could not be confirmed; measuring anyway."
}

Write-Host ""
Write-Host "=== idle health measurement ==="
Write-Host "Scenario : $Scenario"
Write-Host "PID      : $($Process.Id)"
Write-Host "Perf log : $PerfLog"
Write-Host ""
Write-Host "Prepare the requested static state, then do not touch the mouse or keyboard during measurement."
if ($Scenario -eq "tray-residency") {
    Write-Host "Open a thumbnail-heavy folder, then close the main window while thumbnails are still loading."
    Write-Host "Confirm the app remains in the tray and its taskbar/main window is no longer visible before pressing Enter."
    Write-Host "The gate verifies that the process owns at least one top-level window and that all are hidden."
}
else {
    Write-Host "After Enter, use the $WarmupSeconds-second warmup to return focus to mImageViewer for a foreground scenario."
    Write-Host "For a background scenario, leave another window in the foreground during warmup."
}
Write-Host "Input during warmup is excluded; stop interacting when the measurement countdown begins."
$Process = Get-LiveProcess -Id $Process.Id
if ($Scenario -eq "video-pin-background") {
    Write-Host "Ensure the target tile is in the current folder's keep range."
    Write-Host "The gate accepts matching work after the session's last folder load and before measurement ends."
}
if (-not $SkipPrompt) {
    Read-Host "Press Enter when the scenario is ready" | Out-Null
}

Write-Host "Warmup: $WarmupSeconds seconds"
Start-Sleep -Seconds $WarmupSeconds

$DeliveryFailures = New-Object System.Collections.Generic.List[string]
$StartCheckpoint = $null
$EndCheckpoint = $null
$MeasuredSession = $null
try {
    $MeasuredSession = Get-Content -LiteralPath $PerfLog -Encoding UTF8 -TotalCount 64 |
        ForEach-Object { try { $_ | ConvertFrom-Json } catch { $null } } |
        Where-Object { $null -ne $_ -and $_.cat -eq "session" -and $_.kind -eq "start" } |
        Select-Object -First 1
    if ($null -eq $MeasuredSession -or $MeasuredSession.pid -ne $Process.Id) {
        throw "measured perf session PID mismatch"
    }
    $CreationUnixMs = [long][Math]::Floor(($Process.StartTime.ToUniversalTime().Ticks - 621355968000000000) / 10000.0)
    if ([long]$MeasuredSession.process_start_unix_ms -ne $CreationUnixMs) {
        throw "measured perf process creation time mismatch"
    }
    if ($null -eq $MeasuredSession.PSObject.Properties["wall_unix_ms"] -or
        $null -eq $MeasuredSession.PSObject.Properties["wall_t"]) {
        throw "perf monotonic/wall anchor is missing"
    }
    $StartCheckpoint = Get-DeliveryCheckpoint -Session $MeasuredSession -RequestId ([guid]::NewGuid().ToString("N"))
}
catch { $DeliveryFailures.Add("before measurement: " + $_.Exception.Message) }
$Process = Get-LiveProcess -Id $Process.Id
$StartWall = Get-Date
$StartProcessElapsed = ($StartWall - $Process.StartTime).TotalSeconds
if ($DeliveryFailures.Count -eq 0) {
    $StartUnixMs = ($StartWall.ToUniversalTime().Ticks - 621355968000000000) / 10000.0
    $StartProcessElapsed = [double]$MeasuredSession.wall_t + ($StartUnixMs - [double]$MeasuredSession.wall_unix_ms) / 1000.0
}
$StartCpu = $Process.TotalProcessorTime.TotalSeconds
$StartForegroundProcessId = Get-ForegroundProcessId
$StartWindowSummary = Get-ProcessTopLevelWindowSummary -Id $Process.Id
$Stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

Write-Host "Measuring: $MeasureSeconds seconds (do not interact)"
Start-Sleep -Seconds $MeasureSeconds

$Stopwatch.Stop()
$Process = Get-LiveProcess -Id $Process.Id
$EndWall = Get-Date
$EndProcessElapsed = ($EndWall - $Process.StartTime).TotalSeconds
if ($DeliveryFailures.Count -eq 0) {
    $EndUnixMs = ($EndWall.ToUniversalTime().Ticks - 621355968000000000) / 10000.0
    $EndProcessElapsed = [double]$MeasuredSession.wall_t + ($EndUnixMs - [double]$MeasuredSession.wall_unix_ms) / 1000.0
}
$EndCpu = $Process.TotalProcessorTime.TotalSeconds
$EndForegroundProcessId = Get-ForegroundProcessId
$EndWindowSummary = Get-ProcessTopLevelWindowSummary -Id $Process.Id

$ElapsedSeconds = [Math]::Max($Stopwatch.Elapsed.TotalSeconds, 0.001)
$CpuDeltaSeconds = [Math]::Max($EndCpu - $StartCpu, 0.0)
# 1.0 means one logical core is fully occupied; do not divide by machine core count.
$CpuCoreRatio = $CpuDeltaSeconds / $ElapsedSeconds
$AppLogGrowth = $null
$PerfLogGrowth = $null

$Timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$SafeScenario = $Scenario -replace "[^A-Za-z0-9_.-]", "_"
$PerfReportPath = Join-Path $OutputDirectory "$Timestamp-$SafeScenario-perf.json"
$ProcessReportPath = Join-Path $OutputDirectory "$Timestamp-$SafeScenario-process.json"
$PerfPrefixPath = Join-Path $OutputDirectory "$Timestamp-$SafeScenario-prefix.jsonl"
$DeliveryAckPath = Join-Path $OutputDirectory "$Timestamp-$SafeScenario-checkpoint.json"
try {
    $EndCheckpoint = Get-DeliveryCheckpoint -Session $MeasuredSession -RequestId ([guid]::NewGuid().ToString("N"))
    if ($EndCheckpoint.coverage_t -lt $EndProcessElapsed) { throw "checkpoint coverage does not reach measurement end" }
    Save-DeliveryPrefix -Ack $EndCheckpoint -Destination $PerfPrefixPath
    $LogGrowth = Get-DeliveredLogGrowth -Start $StartCheckpoint -End $EndCheckpoint
    $AppLogGrowth = $LogGrowth.normal
    $PerfLogGrowth = $LogGrowth.perf
    $EndCheckpoint | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $DeliveryAckPath -Encoding UTF8
}
catch { $DeliveryFailures.Add("after measurement: " + $_.Exception.Message) }

$Python = Get-Command python -ErrorAction Stop
$Analyzer = Join-Path $PSScriptRoot "analyze_perf.py"
if ($null -eq $EndCheckpoint) { $EndCheckpoint = [pscustomobject]@{ request_id = "unconfirmed" } }
$AnalyzerArgs = @(
    $Analyzer,
    $PerfPrefixPath,
    "idle-health",
    "--start-t", (Format-Invariant -Value $StartProcessElapsed),
    "--end-t", (Format-Invariant -Value $EndProcessElapsed),
    "--target-update-rate", (Format-Invariant -Value ([double]$Thresholds.target_update_rate_per_sec)),
    "--max-update-rate", (Format-Invariant -Value ([double]$Thresholds.max_update_rate_per_sec)),
    "--max-reason-streak-secs", (Format-Invariant -Value ([double]$Thresholds.max_reason_streak_seconds)),
    "--max-same-work", ([string][int]$Thresholds.max_same_work),
    "--max-input-events", ([string][int]$Thresholds.max_input_events),
    "--expected-pid", ([string][int]$Process.Id),
    "--json-out", $PerfReportPath,
    "--checkpoint-id", ([string]$EndCheckpoint.request_id),
    "--delivery-ack", $DeliveryAckPath
)
if ($IdentityEvidence -eq "command_line") {
    $AnalyzerArgs += "--allow-sleeping-window"
}
if ($Scenario -eq "video-pin-background") {
    $AnalyzerArgs += @(
        "--require-work-key", $TargetKey
    )
}

Write-Host ""
$AnalyzerExitCode = 2
if ($DeliveryFailures.Count -eq 0) {
    & $Python.Source @AnalyzerArgs
    $AnalyzerExitCode = $LASTEXITCODE
}
else {
    [ordered]@{ status = "CannotJudge"; delivery_failures = @($DeliveryFailures) } |
        ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $PerfReportPath -Encoding UTF8
}

$Failures = New-Object System.Collections.Generic.List[string]
$Warnings = New-Object System.Collections.Generic.List[string]
$ScenarioLower = $Scenario.ToLowerInvariant()
$TargetForegroundAtStart = $StartForegroundProcessId -eq $Process.Id
$TargetForegroundAtEnd = $EndForegroundProcessId -eq $Process.Id
if ($Scenario -eq "tray-residency") {
    if ($StartWindowSummary.Total -lt 1 -or $EndWindowSummary.Total -lt 1) {
        $Failures.Add(
            "tray-residency could not observe an owned top-level window " +
            "(start=$($StartWindowSummary.Total) end=$($EndWindowSummary.Total))"
        )
    }
    if ($StartWindowSummary.Visible -ne 0 -or $EndWindowSummary.Visible -ne 0) {
        $Failures.Add(
            "tray-residency had a visible top-level window " +
            "(start=$($StartWindowSummary.Visible) end=$($EndWindowSummary.Visible))"
        )
    }
}
elseif ($StartForegroundProcessId -eq 0 -or $EndForegroundProcessId -eq 0) {
    $Failures.Add("foreground process could not be observed at both measurement boundaries")
}
elseif ($ScenarioLower.Contains("background")) {
    if ($TargetForegroundAtStart -or $TargetForegroundAtEnd) {
        $Failures.Add(
            "background scenario had mImageViewer in foreground " +
            "(start=$TargetForegroundAtStart end=$TargetForegroundAtEnd)"
        )
    }
}
elseif ($ScenarioLower.Contains("foreground")) {
    if (-not $TargetForegroundAtStart -or -not $TargetForegroundAtEnd) {
        $Failures.Add(
            "foreground scenario did not keep mImageViewer in foreground " +
            "(start=$TargetForegroundAtStart end=$TargetForegroundAtEnd)"
        )
    }
}
if ($CpuCoreRatio -gt [double]$Thresholds.max_cpu_core_ratio) {
    $Failures.Add(
        "CPU one-core ratio $([Math]::Round($CpuCoreRatio, 4)) exceeds $($Thresholds.max_cpu_core_ratio)"
    )
}
elseif ($CpuCoreRatio -gt [double]$Thresholds.target_cpu_core_ratio) {
    $Warnings.Add(
        "CPU one-core ratio $([Math]::Round($CpuCoreRatio, 4)) exceeds target $($Thresholds.target_cpu_core_ratio)"
    )
}
if ($AppLogGrowth -gt [long]$Thresholds.max_app_log_growth_bytes) {
    $Failures.Add(
        "mimageviewer.log grew by $AppLogGrowth bytes (limit $($Thresholds.max_app_log_growth_bytes))"
    )
}
if ($PerfLogGrowth -gt [long]$Thresholds.max_perf_log_growth_bytes) {
    $Failures.Add(
        "perf_events.jsonl grew by $PerfLogGrowth bytes (limit $($Thresholds.max_perf_log_growth_bytes))"
    )
}
if ($AnalyzerExitCode -ne 0 -and $AnalyzerExitCode -ne 2) {
    $Failures.Add("analyze_perf.py idle-health failed with exit code $AnalyzerExitCode")
}

$Status = "pass"
if ($Failures.Count -gt 0) {
    $Status = "fail"
}
if ($AnalyzerExitCode -eq 2) { $Status = "CannotJudge" }
$ProcessReport = [ordered]@{
    delivery_failures = @($DeliveryFailures)
    delivery_checkpoint = $EndCheckpoint
    status = $Status
    scenario = $Scenario
    timestamp = $Timestamp
    index_scan_wait = $IndexScanWait
    process = [ordered]@{
        id = $Process.Id
        launched_by_script = $Launched
        identity_evidence = $IdentityEvidence
        command_line = $MeasuredProcessInfo.CommandLine
        path = $MeasuredPath
    }
    window = [ordered]@{
        start_process_elapsed_secs = $StartProcessElapsed
        end_process_elapsed_secs = $EndProcessElapsed
        measured_wall_secs = $ElapsedSeconds
        top_level_count_start = $StartWindowSummary.Total
        top_level_count_end = $EndWindowSummary.Total
        visible_count_start = $StartWindowSummary.Visible
        visible_count_end = $EndWindowSummary.Visible
    }
    metrics = [ordered]@{
        cpu_delta_secs = $CpuDeltaSeconds
        cpu_one_core_ratio = $CpuCoreRatio
        app_log_growth_bytes = $AppLogGrowth
        perf_log_growth_bytes = $PerfLogGrowth
        foreground_process_id_start = $StartForegroundProcessId
        foreground_process_id_end = $EndForegroundProcessId
        target_foreground_start = $TargetForegroundAtStart
        target_foreground_end = $TargetForegroundAtEnd
    }
    thresholds = $Thresholds
    perf_report = $PerfReportPath
    warnings = @($Warnings)
    failures = @($Failures)
}
$ProcessReport | ConvertTo-Json -Depth 8 |
    Set-Content -LiteralPath $ProcessReportPath -Encoding UTF8

Write-Host ""
Write-Host "CPU one-core ratio : $([Math]::Round($CpuCoreRatio, 4))"
Write-Host "App log growth     : $AppLogGrowth bytes"
Write-Host "Perf log growth    : $PerfLogGrowth bytes"
Write-Host "Target foreground  : start=$TargetForegroundAtStart end=$TargetForegroundAtEnd"
Write-Host "Top-level windows  : start=$($StartWindowSummary.Total)/$($StartWindowSummary.Visible) visible " +
    "end=$($EndWindowSummary.Total)/$($EndWindowSummary.Visible) visible"
foreach ($Warning in $Warnings) {
    Write-Warning $Warning
}
foreach ($Failure in $Failures) {
    Write-Host "FAIL: $Failure" -ForegroundColor Red
}
Write-Host "Process report     : $ProcessReportPath"
Write-Host "Result             : $($Status.ToUpperInvariant())"
if ($Launched) {
    Write-Host "mImageViewer remains running (PID $($Process.Id)); exit it normally when checks are complete."
}

if ($Status -eq "CannotJudge") { exit 2 }
if ($Failures.Count -gt 0) {
    exit 1
}
exit 0
