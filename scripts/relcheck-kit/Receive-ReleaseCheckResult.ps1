# Read-only validation of a returned run; never reanalyzes or reruns it.
# Exit 0: verified PASS/PASS_WITH_WARNINGS; 1: verified FAIL; 2: verified SKIPPED.
# Exit 3: kit/result validation failed (missing, malformed or inconsistent evidence).
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
    $expectedKit=@{kit_id=$manifest.kit_id;kit_manifest_sha256=$digest;scripts_source_head=$manifest.scripts_source_head;artifact_source_head=$manifest.artifact.source_head}
    foreach($metadata in @($suite.metadata,$summary.metadata)){
        if($metadata.execution_mode -cne 'kit' -or
            $metadata.exe_sha256 -cne $manifest.artifact.manifest.core_sha256 -or
            $metadata.source -cne ($manifest.artifact.manifest.source_head+'/'+$fingerprint) -or
            $metadata.artifact.source_fingerprint_sha256 -isnot [string] -or
            $metadata.artifact.source_fingerprint_sha256 -cne $fingerprint){throw 'Returned metadata does not match this kit.'}
        foreach($field in @('kit_id','kit_manifest_sha256','scripts_source_head','artifact_source_head')){
            if($metadata.kit.$field -isnot [string] -or [string]::IsNullOrWhiteSpace($metadata.kit.$field) -or
                $metadata.kit.$field -cne $expectedKit[$field]){throw "Returned kit metadata mismatch: $field"}
        }
        foreach($field in @('execution_mode','exe_sha256','source','run_path')){
            if($metadata.$field -isnot [string] -or [string]::IsNullOrWhiteSpace($metadata.$field) -or
                $metadata.$field -cne $suite.metadata.$field){throw "Summary/suite metadata mismatch: $field"}
        }
    }
    $codes=@{PASS=0;PASS_WITH_WARNINGS=0;FAIL=1;SKIPPED=2}
    if($summary.verdict -isnot [string] -or -not $codes.ContainsKey($summary.verdict) -or
        $summary.verdict -cnotin @('PASS','PASS_WITH_WARNINGS','FAIL','SKIPPED') -or
        $summary.exit_code -isnot [int] -or $summary.exit_code -ne $codes[$summary.verdict]){throw 'Invalid summary verdict/exit code.'}
    if(-not (Test-Path -LiteralPath (Join-Path $Result 'release-record.md') -PathType Leaf)){throw 'release-record.md is missing.'}
    $python=(Get-Command python -ErrorAction Stop).Source
    $env:PYTHONDONTWRITEBYTECODE='1'
    # -I ignores PYTHON* variables, so -B also enforces the bytecode policy.
    # Render in memory and compare in Python to avoid console encoding changes.
    $recordCheck=@'
import json
import sys
from pathlib import Path

sys.path.insert(0, sys.argv[1])
from release_check_analysis import release_record

summary = json.loads(Path(sys.argv[2]).read_text(encoding='utf-8-sig'))
expected = release_record(summary)
actual = Path(sys.argv[3]).read_text(encoding='utf-8')
def normalize(text):
    return text.replace('\r\n', '\n').replace('\r', '\n')
if normalize(actual) != normalize(expected):
    raise ValueError('release-record.md does not match summary.json.')
'@
    & $python -I -B -c $recordCheck (Join-Path $Kit 'scripts') (Join-Path $Result 'summary.json') (Join-Path $Result 'release-record.md')
    if($LASTEXITCODE -ne 0){throw 'Release record generation/validation failed.'}
    Write-Host "Verdict: $($summary.verdict); exit code: $($summary.exit_code)"
    Write-Host ('Release record: '+(Join-Path $Result 'release-record.md'))
    exit $summary.exit_code
}catch{
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 3
}
