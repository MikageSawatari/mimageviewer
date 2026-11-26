# Measure one cold or warm build of the test graph (sections 1.908-1.910).
# ASCII only on purpose (Windows PowerShell 5.1).
#
# Records, once per second: the system commit charge (GetPerformanceInfo), the
# summed private bytes of compiler/linker processes, and how many of them run.
# Writes samples.csv, summary.txt and the cargo log to -OutDir.
#
# Safety valve: if the system commit charge exceeds -AbortFraction of the commit
# limit, the cargo process tree is killed and the run is recorded as ABORTED.
# The v5.8.0 build stopped with os error 1455 at the commit limit; this keeps a
# measurement from starving other programs on the PC.
#
# Run through the shared heavy-job runner, exclusively:
#   run-heavy.ps1 -FullTest -Owner ... -WorkDir <repo> -Command ".\scripts\measure-test-build.ps1 -Label base -CleanDebug"
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidatePattern('^[A-Za-z0-9_.-]+$')][string] $Label,
    [string] $OutDir = '',
    [int] $Jobs = 0,
    [switch] $CleanDebug,
    [double] $AbortFraction = 0.92,
    [string[]] $CargoArgs = @('test', '--workspace', '--features', 'pack-build-tools', '--no-run')
)
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not $OutDir) { $OutDir = Join-Path $repoRoot ('target\build-measure\' + $Label) }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class MivPerfInfo
{
    [StructLayout(LayoutKind.Sequential)]
    public struct PERFORMANCE_INFORMATION
    {
        public uint cb;
        public UIntPtr CommitTotal;
        public UIntPtr CommitLimit;
        public UIntPtr CommitPeak;
        public UIntPtr PhysicalTotal;
        public UIntPtr PhysicalAvailable;
        public UIntPtr SystemCache;
        public UIntPtr KernelTotal;
        public UIntPtr KernelPaged;
        public UIntPtr KernelNonpaged;
        public UIntPtr PageSize;
        public uint HandleCount;
        public uint ProcessCount;
        public uint ThreadCount;
    }
    [DllImport("psapi.dll", SetLastError = true)]
    static extern bool GetPerformanceInfo(out PERFORMANCE_INFORMATION pi, uint cb);
    // Returns { commitBytes, limitBytes, physAvailBytes }.
    public static ulong[] Read()
    {
        PERFORMANCE_INFORMATION pi;
        if (!GetPerformanceInfo(out pi, (uint)Marshal.SizeOf(typeof(PERFORMANCE_INFORMATION))))
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        ulong page = pi.PageSize.ToUInt64();
        return new ulong[] { pi.CommitTotal.ToUInt64() * page, pi.CommitLimit.ToUInt64() * page, pi.PhysicalAvailable.ToUInt64() * page };
    }
}
'@

$toolNames = @('cargo', 'rustc', 'rust-lld', 'lld-link', 'link', 'cl', 'clang', 'cmake', 'msbuild', 'mspdbsrv')
function Get-ToolSample {
    $procs = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
            $toolNames -contains $_.ProcessName -or $_.ProcessName -like 'build-script-*' -or $_.ProcessName -like 'build_script_*'
        })
    $sum = [uint64]0; $rustcMax = [uint64]0; $lldMax = [uint64]0; $rustcN = 0; $lldN = 0
    foreach ($p in $procs) {
        $pb = [uint64]0
        try { $pb = [uint64]$p.PrivateMemorySize64 } catch { continue }
        $sum += $pb
        if ($p.ProcessName -eq 'rustc') { $rustcN++; if ($pb -gt $rustcMax) { $rustcMax = $pb } }
        if ($p.ProcessName -eq 'rust-lld' -or $p.ProcessName -eq 'lld-link' -or $p.ProcessName -eq 'link') { $lldN++; if ($pb -gt $lldMax) { $lldMax = $pb } }
    }
    return [pscustomobject]@{ Sum = $sum; RustcN = $rustcN; RustcMax = $rustcMax; LinkN = $lldN; LinkMax = $lldMax }
}

$gb = [double](1024 * 1024 * 1024)
Push-Location $repoRoot
try {
    if ($CleanDebug) {
        Write-Host '[measure] cargo clean --profile dev (removes target\debug)'
        & cargo clean --profile dev
        if ($LASTEXITCODE -ne 0) { throw "cargo clean failed: $LASTEXITCODE" }
    }
    if ($Jobs -gt 0) { $env:CARGO_BUILD_JOBS = [string]$Jobs } else { Remove-Item Env:\CARGO_BUILD_JOBS -ErrorAction SilentlyContinue }
    $jobsText = if ($env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS } else { 'default' }
    $start0 = [MivPerfInfo]::Read()
    $stdout = Join-Path $OutDir 'cargo-stdout.log'
    $stderr = Join-Path $OutDir 'cargo-stderr.log'
    $csv = Join-Path $OutDir 'samples.csv'
    'seconds,commit_gb,limit_gb,phys_avail_gb,tools_private_gb,rustc_n,rustc_max_gb,link_n,link_max_gb' | Set-Content -Encoding ascii -LiteralPath $csv
    Write-Host ("[measure] {0}: cargo {1} (CARGO_BUILD_JOBS={2})" -f $Label, ($CargoArgs -join ' '), $jobsText)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $proc = Start-Process -FilePath 'cargo' -ArgumentList $CargoArgs -NoNewWindow -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $peakCommit = [uint64]0; $peakTools = [uint64]0; $peakRustc = [uint64]0; $peakLink = [uint64]0
    $maxRustcN = 0; $maxLinkN = 0; $peakAt = 0.0; $aborted = $false; $limit = $start0[1]
    while (-not $proc.HasExited) {
        $m = [MivPerfInfo]::Read(); $t = Get-ToolSample; $s = $sw.Elapsed.TotalSeconds
        $limit = $m[1]
        if ($m[0] -gt $peakCommit) { $peakCommit = $m[0]; $peakAt = $s }
        if ($t.Sum -gt $peakTools) { $peakTools = $t.Sum }
        if ($t.RustcMax -gt $peakRustc) { $peakRustc = $t.RustcMax }
        if ($t.LinkMax -gt $peakLink) { $peakLink = $t.LinkMax }
        if ($t.RustcN -gt $maxRustcN) { $maxRustcN = $t.RustcN }
        if ($t.LinkN -gt $maxLinkN) { $maxLinkN = $t.LinkN }
        ('{0:F0},{1:F2},{2:F2},{3:F2},{4:F2},{5},{6:F2},{7},{8:F2}' -f $s, ($m[0] / $gb), ($m[1] / $gb), ($m[2] / $gb), ($t.Sum / $gb), $t.RustcN, ($t.RustcMax / $gb), $t.LinkN, ($t.LinkMax / $gb)) |
            Add-Content -Encoding ascii -LiteralPath $csv
        if ($m[0] -gt ($AbortFraction * $m[1])) {
            Write-Host ("[measure] ABORT: commit {0:F1} GB exceeds {1:P0} of the limit {2:F1} GB" -f ($m[0] / $gb), $AbortFraction, ($m[1] / $gb))
            & taskkill /T /F /PID $proc.Id | Out-Null
            $aborted = $true
            break
        }
        Start-Sleep -Milliseconds 1000
    }
    $proc.WaitForExit()
    $sw.Stop()
    $exit = if ($aborted) { 'ABORTED' } else { [string]$proc.ExitCode }

    $rlibs = @(Get-ChildItem -LiteralPath (Join-Path $repoRoot 'target\debug') -Recurse -Filter 'libmimageviewer-*.rlib' -File -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending)
    $rlib = if ($rlibs.Count) { $rlibs[0] } else { $null }
    $testExes = @()
    $buildDir = Join-Path $repoRoot 'target\debug\build'
    if (Test-Path -LiteralPath $buildDir) {
        $testExes = @(Get-ChildItem -LiteralPath $buildDir -Recurse -Filter '*.exe' -File -ErrorAction SilentlyContinue |
                Where-Object { $_.LastWriteTime -ge $proc.StartTime -and $_.Name -notlike 'build-script-*' -and $_.Name -notlike 'build_script_*' })
    }
    $exeBytes = [uint64]0; foreach ($e in $testExes) { $exeBytes += [uint64]$e.Length }

    $lines = @(
        "label=$Label",
        "date=$(Get-Date -Format s)",
        "git_head=$(git rev-parse --short HEAD)",
        "cargo_args=$($CargoArgs -join ' ')",
        "cargo_build_jobs=$jobsText",
        "clean_debug=$([bool]$CleanDebug)",
        "exit=$exit",
        ('wall_seconds={0:F0}' -f $sw.Elapsed.TotalSeconds),
        ('commit_at_start_gb={0:F2}' -f ($start0[0] / $gb)),
        ('commit_limit_gb={0:F2}' -f ($limit / $gb)),
        ('peak_commit_gb={0:F2}' -f ($peakCommit / $gb)),
        ('peak_commit_rise_gb={0:F2}' -f (([double]$peakCommit - [double]$start0[0]) / $gb)),
        ('peak_commit_at_seconds={0:F0}' -f $peakAt),
        ('peak_tools_private_gb={0:F2}' -f ($peakTools / $gb)),
        ('peak_single_rustc_gb={0:F2}' -f ($peakRustc / $gb)),
        ('peak_single_linker_gb={0:F2}' -f ($peakLink / $gb)),
        "max_concurrent_rustc=$maxRustcN",
        "max_concurrent_linker=$maxLinkN",
        "rlib=$(if ($rlib) { $rlib.FullName.Substring($repoRoot.Length + 1) } else { 'none' })",
        ('rlib_gb={0:F3}' -f $(if ($rlib) { $rlib.Length / $gb } else { 0 })),
        "new_exe_count=$($testExes.Count)",
        ('new_exe_total_gb={0:F3}' -f ($exeBytes / $gb))
    )
    $lines | Set-Content -Encoding ascii -LiteralPath (Join-Path $OutDir 'summary.txt')
    $lines | ForEach-Object { Write-Host "[measure] $_" }
    if ($aborted) { exit 3 }
    exit $proc.ExitCode
} finally {
    Pop-Location
}
