# Script-only regression checks for the unattended distribution-build switches.
# This script does not run Cargo, build product binaries, sign files, launch
# mImageViewer, stop processes, or touch APPDATA. A temporary native cargo stub
# validates the process-local Windows error mode and test-full.ps1 exit behavior.

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$buildDistPath = Join-Path $PSScriptRoot 'build-dist.ps1'
$buildReleasePath = Join-Path $PSScriptRoot 'build-release.ps1'
$buildPortablePath = Join-Path $PSScriptRoot 'build-portable.ps1'
$testFullPath = Join-Path $PSScriptRoot 'test-full.ps1'

function Assert-True {
    param(
        [bool] $Condition,
        [string] $Message
    )
    if (-not $Condition) { throw "[release-build-safety-test] $Message" }
}

function Get-ScriptAst {
    param([string] $Path)
    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile(
        $Path,
        [ref] $tokens,
        [ref] $errors
    )
    Assert-True ($errors.Count -eq 0) "PowerShell parse failed: $Path"
    return $ast
}

function Assert-OrderedText {
    param(
        [string] $Text,
        [string[]] $Needles,
        [string] $Label
    )
    $previous = -1
    foreach ($needle in $Needles) {
        $index = $Text.IndexOf($needle, $previous + 1, [System.StringComparison]::Ordinal)
        Assert-True ($index -ge 0) "$Label is missing: $needle"
        Assert-True ($index -gt $previous) "$Label is out of order at: $needle"
        $previous = $index
    }
}

$buildDistAst = Get-ScriptAst $buildDistPath
$buildReleaseAst = Get-ScriptAst $buildReleasePath
$buildPortableAst = Get-ScriptAst $buildPortablePath
$testFullAst = Get-ScriptAst $testFullPath

$buildDist = Get-Content -LiteralPath $buildDistPath -Raw -Encoding UTF8
$buildRelease = Get-Content -LiteralPath $buildReleasePath -Raw -Encoding UTF8
$buildPortable = Get-Content -LiteralPath $buildPortablePath -Raw -Encoding UTF8
$testFull = Get-Content -LiteralPath $testFullPath -Raw -Encoding UTF8

Assert-True ($buildDist.Contains('[switch] $PreserveRuntime')) 'build-dist lacks -PreserveRuntime'
Assert-True ($buildRelease.Contains('[switch] $PreserveRuntime')) 'build-release lacks -PreserveRuntime'
Assert-True ($buildPortable.Contains('[switch] $PreserveRuntime')) 'build-portable lacks -PreserveRuntime'
Assert-True ($buildPortable.Contains('[switch] $KeepRunning')) 'build-portable lacks -KeepRunning'
Assert-True ($testFull.Contains('[switch] $SuppressCrashDialogs')) 'test-full lacks -SuppressCrashDialogs'

$distResidentGuards = @($buildDistAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.IfStatementAst] -and
        $node.Clauses[0].Item1.Extent.Text -eq '$running.Count -gt 0'
}, $true))
Assert-True ($distResidentGuards.Count -eq 1) 'build-dist resident guard is missing or ambiguous'
$distGuardBody = $distResidentGuards[0].Clauses[0].Item2.Extent.Text
Assert-True ($distGuardBody.Contains('throw $message')) 'build-dist resident guard does not fail before the gate/clean'

$releaseResidentGuards = @($buildReleaseAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.IfStatementAst] -and
        $node.Clauses[0].Item1.Extent.Text -eq '$PreserveRuntime -and $candidates.Count -gt 0'
}, $true))
Assert-True ($releaseResidentGuards.Count -eq 1) 'build-release preserve-runtime resident guard is missing or ambiguous'
$releaseGuardThen = $releaseResidentGuards[0].Clauses[0].Item2.Extent.Text
$releaseGuardElse = $releaseResidentGuards[0].ElseClause.Extent.Text
Assert-True ($releaseGuardThen.Contains('throw') -and -not $releaseGuardThen.Contains('Stop-Process')) 'build-release preserve-runtime branch can stop a process'
Assert-True ($releaseGuardElse.Contains('Stop-Process')) 'build-release default process-stop branch was removed'

$portableResidentGuards = @($buildPortableAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.IfStatementAst] -and
        $node.Clauses[0].Item1.Extent.Text -eq '$KeepRunning' -and
        $node.Extent.Text.Contains('Stop-Process -Id $_.Id')
}, $true))
Assert-True ($portableResidentGuards.Count -eq 1) 'build-portable preserve-runtime resident guard is missing or ambiguous'
$portableGuard = $portableResidentGuards[0]
Assert-True ($portableGuard.Clauses.Count -eq 3) 'build-portable process policy clauses are missing'
Assert-True ($portableGuard.Clauses[1].Item1.Extent.Text -eq '$PreserveRuntime') 'build-portable preserve-runtime clause condition changed'
Assert-True ($portableGuard.Clauses[2].Item1.Extent.Text -eq '-not $SmokeTestScript') 'build-portable default process-stop clause condition changed'
$portableKeepRunningBody = $portableGuard.Clauses[0].Item2.Extent.Text
$portablePreserveBody = $portableGuard.Clauses[1].Item2.Extent.Text
$portableDefaultBody = $portableGuard.Clauses[2].Item2.Extent.Text
Assert-True (-not $portableKeepRunningBody.Contains('Stop-Process')) 'build-portable keep-running branch can stop a process'
Assert-True ($portablePreserveBody.Contains('throw') -and -not $portablePreserveBody.Contains('Stop-Process')) 'build-portable preserve-runtime branch can stop a process'
Assert-True ($portableDefaultBody.Contains('Stop-Process')) 'build-portable default process-stop branch was removed'
Assert-True ($buildPortable.Contains('if ($KeepRunning -and $PreserveRuntime)')) 'build-portable does not reject conflicting process policies'

$cacheGuards = @($buildReleaseAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.IfStatementAst] -and
        $node.Clauses[0].Item1.Extent.Text -eq '$PreserveRuntime' -and
        $node.Extent.Text.Contains('removed stale extracted VST3 bridge cache')
}, $true))
Assert-True ($cacheGuards.Count -eq 1) 'build-release VST3 cache guard is missing or ambiguous'
$cacheGuardThen = $cacheGuards[0].Clauses[0].Item2.Extent.Text
$cacheGuardElse = $cacheGuards[0].ElseClause.Extent.Text
Assert-True (-not $cacheGuardThen.Contains('Remove-Item')) 'preserve-runtime cache branch deletes APPDATA state'
Assert-True ($cacheGuardElse.Contains('Remove-Item -LiteralPath $path')) 'default VST3 cache invalidation was removed'

Assert-OrderedText $buildDist @(
    "if (`$running.Count -gt 0)",
    "if (`$PreserveRuntime) { `$testArgs += '-SuppressCrashDialogs' }",
    "& cargo clean --release -p mimageviewer",
    "if (`$PreserveRuntime) { `$releaseArgs += '-PreserveRuntime' }",
    "& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path `$scripts 'build-release.ps1')",
    "& `$isccPath (Join-Path `$repoRoot 'installer\mimageviewer.iss')",
    "Invoke-MivSign -Files @(`$setupExe) -Verify",
    "if (`$PreserveRuntime) { `$portableArgs += '-PreserveRuntime' }",
    "& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path `$scripts 'build-portable.ps1')"
) 'build-dist gate/clean/build/sign/package flow'

Assert-OrderedText $buildRelease @(
    'if ($PreserveRuntime -and $candidates.Count -gt 0)',
    'Stop-Process -Id $p.Id -Force -ErrorAction Stop',
    'Invoke-MivSign -Files $vendorEmbedTargets',
    '$coreExit = Invoke-ReleaseCargo -Args $coreCmd',
    'Invoke-MivSign -Files $launcherEmbedExecutables',
    '$launcherExit = Invoke-ReleaseCargo -Args $launcherCmd',
    'Invoke-MivSign -Files @($releaseExe) -Verify',
    'if ($PreserveRuntime)',
    'Remove-Item -LiteralPath $path -Force -ErrorAction Stop'
) 'build-release process/build/sign/cache flow'

Assert-OrderedText $buildPortable @(
    'if ($KeepRunning -and $PreserveRuntime)',
    'if ($KeepRunning)',
    'Stop-Process -Id $_.Id -Force -ErrorAction Stop',
    '& cargo build --release --bin mimageviewer-core',
    'Remove-Item -LiteralPath $pkgDir -Recurse -Force',
    'Invoke-MivSign -Files $portablePe -Verify',
    "Compress-Archive -Path (Join-Path `$pkgDir '*')"
) 'build-portable process/build/package/sign flow'

Assert-OrderedText $testFull @(
    'public static extern uint SetErrorMode(uint mode);',
    'public static extern uint GetErrorMode();',
    '$effectiveMode = [MivTestErrorMode]::GetErrorMode()',
    '& cargo test --workspace --features pack-build-tools --no-fail-fast',
    '& cargo test --manifest-path vendor/egui/Cargo.toml --lib',
    '& cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib',
    '& cargo test --manifest-path vendor/eframe/Cargo.toml --no-default-features --features wgpu --lib',
    '[void][MivTestErrorMode]::SetErrorMode($modeBefore)',
    '$restoredMode = [MivTestErrorMode]::GetErrorMode()',
    'exit $gateExit'
) 'test-full error-mode/test/restore flow'

$tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\')
$testRoot = Join-Path $tempRoot ("miv-release-build-safety-{0}" -f [guid]::NewGuid().ToString('N'))
$testRootFull = [System.IO.Path]::GetFullPath($testRoot).TrimEnd('\')
Assert-True ($testRootFull.StartsWith(($tempRoot + '\'), [System.StringComparison]::OrdinalIgnoreCase)) 'temporary test root escaped the OS temp directory'
New-Item -ItemType Directory -Path $testRootFull | Out-Null

try {
    $fakeCargo = Join-Path $testRootFull 'cargo.exe'
    $fakeCargoSource = @'
using System;
using System.IO;
using System.Runtime.InteropServices;

public static class MivFakeCargo
{
    [DllImport("kernel32.dll")]
    public static extern uint GetErrorMode();

    public static int Main(string[] args)
    {
        string log = Environment.GetEnvironmentVariable("MIV_FAKE_CARGO_LOG");
        int call = File.Exists(log) ? File.ReadAllLines(log).Length + 1 : 1;
        uint mode = GetErrorMode();
        File.AppendAllText(log, String.Format("call={0};mode=0x{1:X8};args={2}{3}", call, mode, String.Join(" ", args), Environment.NewLine));
        int failCall;
        int failCode;
        Int32.TryParse(Environment.GetEnvironmentVariable("MIV_FAKE_CARGO_FAIL_CALL"), out failCall);
        Int32.TryParse(Environment.GetEnvironmentVariable("MIV_FAKE_CARGO_FAIL_CODE"), out failCode);
        return call == failCall ? failCode : 0;
    }
}
'@
    $fakeCargoSourcePath = Join-Path $testRootFull 'cargo.cs'
    [System.IO.File]::WriteAllText($fakeCargoSourcePath, $fakeCargoSource, (New-Object System.Text.UTF8Encoding($false)))
    $csc = Join-Path $env:SystemRoot 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
    if (-not (Test-Path -LiteralPath $csc -PathType Leaf)) {
        $csc = Join-Path $env:SystemRoot 'Microsoft.NET\Framework\v4.0.30319\csc.exe'
    }
    Assert-True (Test-Path -LiteralPath $csc -PathType Leaf) 'C# compiler not found for the native child stub'
    & $csc /nologo /target:exe "/out:$fakeCargo" $fakeCargoSourcePath
    Assert-True ($LASTEXITCODE -eq 0 -and (Test-Path -LiteralPath $fakeCargo -PathType Leaf)) 'failed to compile the native child stub'

    $stubRunner = Join-Path $testRootFull 'run-test-full-with-stub.ps1'
    $stubRunnerSource = @'
[CmdletBinding()]
param(
    [string] $TestFull,
    [string] $FakeCargo,
    [string] $CargoLog,
    [int] $FailCall,
    [int] $FailCode,
    [switch] $SuppressCrashDialogs
)

$ErrorActionPreference = 'Stop'
$stubDir = Split-Path -Parent $FakeCargo
$env:PATH = $stubDir
$expectedCargo = [System.IO.Path]::GetFullPath($FakeCargo)
$cargoCommands = @(Get-Command cargo -CommandType Application -ErrorAction Stop)
if ($cargoCommands.Count -ne 1) {
    throw "[stub-runner] expected exactly one resolved cargo command, found $($cargoCommands.Count)"
}
$resolvedCargo = [System.IO.Path]::GetFullPath($cargoCommands[0].Source)
if (-not $resolvedCargo.Equals($expectedCargo, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "[stub-runner] refusing non-stub Cargo: $resolvedCargo (expected $expectedCargo)"
}
Write-Host "[stub-runner] cargo=$resolvedCargo"

$env:MIV_FAKE_CARGO_LOG = $CargoLog
$env:MIV_FAKE_CARGO_FAIL_CALL = $FailCall.ToString()
$env:MIV_FAKE_CARGO_FAIL_CODE = $FailCode.ToString()
if ($SuppressCrashDialogs) {
    & $TestFull -SuppressCrashDialogs
} else {
    & $TestFull
}
exit $LASTEXITCODE
'@
    [System.IO.File]::WriteAllText($stubRunner, $stubRunnerSource, (New-Object System.Text.UTF8Encoding($false)))

    $baselineLog = Join-Path $testRootFull 'baseline.log'
    $baselineOutput = @(& powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $stubRunner -TestFull $testFullPath -FakeCargo $fakeCargo -CargoLog $baselineLog -FailCall 0 -FailCode 0 2>&1)
    $baselineExit = $LASTEXITCODE
    Assert-True ($baselineExit -eq 0) "test-full default success stub returned $baselineExit; output: $($baselineOutput -join ' | ')"
    $baselineCalls = @(Get-Content -LiteralPath $baselineLog -Encoding UTF8)
    Assert-True ($baselineCalls.Count -eq 4) "test-full default path invoked cargo $($baselineCalls.Count) times"
    Assert-True (-not (($baselineOutput -join "`n").Contains('crash-dialog suppression active'))) 'test-full default path enabled crash-dialog suppression'

    $successLog = Join-Path $testRootFull 'success.log'
    $successOutput = @(& powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $stubRunner -TestFull $testFullPath -FakeCargo $fakeCargo -CargoLog $successLog -FailCall 0 -FailCode 0 -SuppressCrashDialogs 2>&1)
    $successExit = $LASTEXITCODE
    Assert-True ($successExit -eq 0) "test-full success stub returned $successExit; output: $($successOutput -join ' | ')"
    $successCalls = @(Get-Content -LiteralPath $successLog -Encoding UTF8)
    Assert-True ($successCalls.Count -eq 4) "test-full success path invoked cargo $($successCalls.Count) times"
    Assert-True ((@($successCalls | Where-Object { $_ -notmatch 'mode=0x[0-9A-Fa-f]{7}[37BFbf];' })).Count -eq 0) 'a success-path cargo child did not inherit both error-mode bits'
    Assert-True (($successOutput -join "`n").Contains('[test-full] process error mode restored:')) 'test-full success path did not report error-mode restoration'

    $failureLog = Join-Path $testRootFull 'failure.log'
    $failureOutput = @(& powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $stubRunner -TestFull $testFullPath -FakeCargo $fakeCargo -CargoLog $failureLog -FailCall 2 -FailCode 37 -SuppressCrashDialogs 2>&1)
    $failureExit = $LASTEXITCODE
    Assert-True ($failureExit -eq 37) "test-full did not preserve failing cargo exit 37 (actual $failureExit); output: $($failureOutput -join ' | ')"
    $failureCalls = @(Get-Content -LiteralPath $failureLog -Encoding UTF8)
    Assert-True ($failureCalls.Count -eq 2) "test-full retried or continued after failure (cargo calls: $($failureCalls.Count))"
    Assert-True ((@($failureCalls | Where-Object { $_ -notmatch 'mode=0x[0-9A-Fa-f]{7}[37BFbf];' })).Count -eq 0) 'a failure-path cargo child did not inherit both error-mode bits'
    Assert-True (($failureOutput -join "`n").Contains('[test-full] process error mode restored:')) 'test-full failure path did not report error-mode restoration'
} finally {
    $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRootFull).TrimEnd('\')
    Assert-True ($resolvedTestRoot.Equals($testRootFull, [System.StringComparison]::OrdinalIgnoreCase)) 'temporary cleanup target changed'
    Assert-True ($resolvedTestRoot.StartsWith(($tempRoot + '\'), [System.StringComparison]::OrdinalIgnoreCase)) 'refusing temporary cleanup outside the OS temp directory'
    if (Test-Path -LiteralPath $resolvedTestRoot) {
        Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
    }
}

Write-Host '[release-build-safety-test] PASS'
$global:LASTEXITCODE = 0
