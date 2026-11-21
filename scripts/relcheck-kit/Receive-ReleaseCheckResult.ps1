# Read-only validation of a returned run; never reanalyzes or reruns it.
[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$Kit,[Parameter(Mandatory=$true)][string]$Result)
$ErrorActionPreference='Stop'
Set-StrictMode -Version 2
try{
    . (Join-Path $PSScriptRoot 'KitSupport.ps1')
    $Kit=(Resolve-Path -LiteralPath $Kit).Path
    $Result=(Resolve-Path -LiteralPath $Result).Path
    $fingerprint=Assert-ReleaseCheckKit $Kit
    $manifest=Get-Content -LiteralPath (Join-Path $Kit 'release-check-kit.json') -Encoding UTF8 -Raw | ConvertFrom-Json
    $digest=(Get-Content -LiteralPath (Join-Path $Kit 'release-check-kit.sha256') -Encoding ASCII -Raw).Trim()
    $summary=Get-Content -LiteralPath (Join-Path $Result 'summary.json') -Encoding UTF8 -Raw | ConvertFrom-Json
    $suite=Get-Content -LiteralPath (Join-Path $Result 'suite-input.json') -Encoding UTF8 -Raw | ConvertFrom-Json
    if($suite.metadata.execution_mode -cne 'kit' -or $suite.metadata.kit.kit_id -cne $manifest.kit_id -or
        $suite.metadata.kit.kit_manifest_sha256 -cne $digest -or
        $suite.metadata.exe_sha256 -cne $manifest.artifact.manifest.core_sha256 -or
        $suite.metadata.artifact.source_fingerprint_sha256 -cne $fingerprint){throw 'Returned run does not match this kit.'}
    if($summary.exit_code -isnot [int] -or $summary.exit_code -notin @(0,1,2)){throw 'Invalid summary exit code.'}
    Write-Host "Verdict: $($summary.verdict); exit code: $($summary.exit_code)"
    Write-Host ('Release record: '+(Join-Path $Result 'release-record.md'))
    exit $summary.exit_code
}catch{
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
