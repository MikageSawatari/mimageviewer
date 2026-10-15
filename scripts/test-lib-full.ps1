# Run all main-library tests with nextest and reuse a verified success receipt.
[CmdletBinding()]
param(
    [string[]] $Features = @(),
    [switch] $NoDefaultFeatures,
    [ValidatePattern('^[A-Za-z0-9_-]+$')] [string] $CargoProfile = 'test',
    [ValidatePattern('^[A-Za-z0-9_-]+$')] [string] $NextestProfile = 'default',
    [ValidateRange(1, 2147483647)] [int] $MinimumTests = 9000,
    [switch] $Force,
    [string] $RepoRoot = ''
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'test-tools-common.ps1')
if (-not $RepoRoot) { $RepoRoot = Join-Path $PSScriptRoot '..' }
$RepoRoot = (Resolve-Path -LiteralPath $RepoRoot).Path.TrimEnd('\', '/')
$featureArgs = @(Get-MivFeatureArguments $Features -NoDefaultFeatures:$NoDefaultFeatures)
$selection = @('-p', 'mimageviewer', '--lib', '-E', 'all()')
$store = Join-Path $RepoRoot 'target/test-lib-full'
$sourceId = $null
$key = $null
try {
    $sourceId = Get-MivSourceId $RepoRoot
    $environmentId = Get-MivEnvironmentId $RepoRoot
    $commandId = @('nextest-run-v1') + $selection + $featureArgs + @($CargoProfile, $NextestProfile, 'ignore-default-filter', 'run-ignored=default', 'retries=0', 'fail-fast=false')
    $key = Get-MivTextHash (($commandId -join "`n") + "`n$sourceId`n$environmentId")
    $receiptPath = Join-Path $store ($key + '.json')
    if (-not $Force -and [IO.File]::Exists($receiptPath)) {
        try {
            $receipt = ConvertFrom-Json ([IO.File]::ReadAllText($receiptPath))
            if ($receipt.schema -eq 1 -and $receipt.key -eq $key -and $receipt.source -eq $sourceId -and $receipt.exit_code -eq 0 -and $receipt.executed -ge $MinimumTests -and $receipt.failed -eq 0 -and [IO.File]::Exists($receipt.junit)) {
                $verified = Get-MivJUnitSummary $receipt.junit
                if ($verified.Executed -eq $receipt.executed -and $verified.Passed -eq $receipt.passed -and $verified.Failed -eq 0) {
                    Write-Host "[test-lib-full] HIT: $($receipt.executed) passed, $($receipt.skipped) skipped; $($receipt.test_seconds)s; $($receipt.completed_utc)"
                    Write-Host "[test-lib-full] source=$sourceId"
                    Write-Host "[test-lib-full] command: $($receipt.command -join ' ')"
                    Write-Host "[test-lib-full] receipt: $receiptPath"
                    Write-Host "[test-lib-full] logs: $($receipt.run_directory)"
                    exit 0
                }
            }
        } catch { Write-Warning "Cannot reuse receipt: $_" }
    }
} catch { Write-Warning "Source/receipt unavailable; tests will run without reuse: $_"; $key = $null }
Write-Host "[test-lib-full] MISS (or -Force); source=$sourceId"
if ($key -and [IO.File]::Exists($receiptPath)) {
    # A failed forced rerun must not leave an older success reusable.
    try { [IO.File]::Delete($receiptPath) }
    catch { Write-Warning "Cannot invalidate prior receipt; do not reuse it after this rerun: $_"; $key = $null }
}
try { $run = New-MivTestRun $RepoRoot 'test-lib-full' $NextestProfile }
catch {
    Write-Warning "Cannot prepare receipt/log directory; running normally without a ledger: $_"
    Push-Location $RepoRoot
    try {
        $ErrorActionPreference = 'Continue'
        & cargo nextest run @selection @featureArgs --cargo-profile $CargoProfile --profile $NextestProfile --no-tests fail --ignore-default-filter --run-ignored default --retries 0 --no-fail-fast
        exit $LASTEXITCODE
    } finally { Pop-Location }
}
$result = Invoke-MivNextest $RepoRoot $selection $featureArgs $CargoProfile $NextestProfile $run
if ($result.ExitCode -ne 0) { exit $result.ExitCode }
try {
    $summary = Get-MivJUnitSummary $run.JUnit
    if ($summary.Executed -lt $MinimumTests -or $summary.Failed -ne 0) { throw "Insufficient/failed tests: executed=$($summary.Executed), failed=$($summary.Failed), minimum=$MinimumTests" }
} catch { Write-Host "[test-lib-full] FAIL: $_"; exit 2 }
Write-Host "[test-lib-full] PASS: $($summary.Executed) passed, $($summary.Skipped) skipped; $($summary.Seconds)s"
if ($key) {
    try {
        if ((Get-MivSourceId $RepoRoot) -ne $sourceId -or (Get-MivEnvironmentId $RepoRoot) -ne $environmentId) {
            Write-Warning 'Inputs changed during execution; success is not recorded for reuse.'
        } else {
            $receipt = [ordered]@{
                schema = 1; key = $key; source = $sourceId; environment = $environmentId
                command = $result.Command; features = $featureArgs; cargo_profile = $CargoProfile; nextest_profile = $NextestProfile
                executed = $summary.Executed; passed = $summary.Passed; failed = $summary.Failed; skipped = $summary.Skipped
                test_seconds = $summary.Seconds; wall_seconds = $result.WallSeconds
                completed_utc = [DateTime]::UtcNow.ToString('o'); exit_code = 0
                junit = $run.JUnit; run_directory = $run.Directory
            }
            [IO.File]::WriteAllText($receiptPath, ($receipt | ConvertTo-Json -Depth 8), (New-Object Text.UTF8Encoding($false)))
            Write-Host "[test-lib-full] receipt: $receiptPath"
        }
    } catch { Write-Warning "Cannot record success; test result remains PASS: $_" }
}
exit 0
