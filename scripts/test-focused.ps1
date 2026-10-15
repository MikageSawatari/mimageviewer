# Select iteration tests from changed files; do not claim this is a full gate.
[CmdletBinding()]
param(
    [string] $Base = 'HEAD',
    [string[]] $Path = @(),
    [string[]] $Features = @(),
    [switch] $NoDefaultFeatures,
    [ValidatePattern('^[A-Za-z0-9_-]+$')] [string] $CargoProfile = 'test',
    [ValidatePattern('^[A-Za-z0-9_-]+$')] [string] $NextestProfile = 'default',
    [switch] $PlanOnly,
    [string] $RepoRoot = ''
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'test-tools-common.ps1')
if (-not $RepoRoot) { $RepoRoot = Join-Path $PSScriptRoot '..' }
$RepoRoot = (Resolve-Path -LiteralPath $RepoRoot).Path.TrimEnd('\', '/')
try {
    $paths = @($Path)
    if (-not $paths.Count) { $paths = @(Get-MivChangedPaths $RepoRoot $Base) }
    Write-Host "[test-focused] paths (base=$Base; explicit -Path overrides git selection):"
    foreach ($changed in $paths) { Write-Host "  $changed" }
    $plan = Get-MivFocusedPlan $RepoRoot $paths
    if ($plan.BroadPaths.Count) {
        Write-Host '[test-focused] Broad/shared or unmapped inputs; no selected tests will run:'
        foreach ($unmapped in $plan.BroadPaths) { Write-Host "  $unmapped" }
        Write-Host '[test-focused] Run scripts/test-lib-full.ps1, plus relevant tooling checks; final workspace/vendor/doc coverage uses scripts/test-full.ps1.'
        exit 2
    }
    if (-not $plan.Entries.Count) { Write-Host '[test-focused] No changed paths or test selection.'; exit 4 }
    foreach ($entry in $plan.Entries) { Write-Host "[test-focused] $($entry.Path) -> $($entry.Selection -join ' ') -E $($entry.Filter)" }
    if ($PlanOnly) { Write-Host '[test-focused] Plan only; no tests executed.'; exit 0 }
    $featureArgs = @(Get-MivFeatureArguments $Features -NoDefaultFeatures:$NoDefaultFeatures)
    foreach ($entry in $plan.Entries) {
        $run = New-MivTestRun $RepoRoot 'test-focused' $NextestProfile
        $selection = $entry.Selection + @('-E', $entry.Filter)
        $result = Invoke-MivNextest $RepoRoot $selection $featureArgs $CargoProfile $NextestProfile $run
        if ($result.ExitCode -ne 0) { exit $result.ExitCode }
        $summary = Get-MivJUnitSummary $run.JUnit
        if ($summary.Executed -eq 0 -or $summary.Failed -ne 0) { Write-Host '[test-focused] FAIL: zero matches or failed tests'; exit 4 }
        Write-Host "[test-focused] PASS: $($summary.Executed) tests; $($summary.Seconds)s"
    }
} catch { Write-Host "[test-focused] FAIL: $_"; exit 2 }
exit 0
