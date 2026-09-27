[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$runnerPath = Join-Path $PSScriptRoot 'ui-smoke.ps1'

$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    $runnerPath, [ref] $tokens, [ref] $errors)
if ($errors.Count -ne 0) {
    throw "failed to parse ${runnerPath}: $($errors[0].Message)"
}
foreach ($name in @(
        'Get-NormalizedPath',
        'Assert-NoReparsePath',
        'Assert-NoReparseTree',
        'Register-UiSmokeEvidenceFile',
        'Register-UiSmokeScreenshots',
        'Apply-UiSmokeArchiveDisposition',
        'Write-UiSmokeJson',
        'Save-UiSmokeEvidence'
    )) {
    $definitions = $ast.FindAll({
            param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -ceq $name
        }, $true)
    if ($definitions.Count -ne 1) {
        throw "expected exactly one function ${name}, found $($definitions.Count)"
    }
    Set-Item -Path "Function:script:$name" -Value $definitions[0].Body.GetScriptBlock()
}

# This test exercises the real screenshot registrar and evidence finalizer.
# Unrelated artifact copy helpers are inert because this probe has no app build.
function Try-AddUiSmokeEvidenceFile { }
function Try-AddUiSmokeEvidenceDirectory { }
function Try-RegisterUiSmokeEvidenceFile { }
function Try-RegisterUiSmokeEvidenceDirectory { }

$runsRoot = Join-Path $repoRoot 'target\ui-smoke-runs'
if (-not (Test-Path -LiteralPath $runsRoot)) {
    New-Item -ItemType Directory -Path $runsRoot | Out-Null
}
Assert-NoReparsePath $runsRoot $repoRoot 'screenshot-test'
$probeRoot = Join-Path $runsRoot ('screenshot-archive-probe-' + [Guid]::NewGuid().ToString('N'))
$shotsDir = Join-Path $probeRoot 'screenshots'
try {
    New-Item -ItemType Directory -Path $shotsDir | Out-Null
    $script:runDir = $probeRoot
    $script:runnerEventLog = Join-Path $probeRoot 'runner-events.log'
    [System.IO.File]::WriteAllText($script:runnerEventLog, '')
    # Valid JSON, but the PNG named by this manifest does not exist.
    [System.IO.File]::WriteAllText(
        (Join-Path $shotsDir 'manifest.jsonl'),
        '{"path":"screenshots/01-failure-root.png","label":"failure","viewport":"root","width":2,"height":2,"frame":7,"timestamp_ms":1000}')
    $script:evidenceEntries = New-Object System.Collections.ArrayList
    $script:archiveErrors = New-Object System.Collections.ArrayList
    $script:portableValidatedForRun = $true
    $script:runExitCode = 1
    $script:failureMessage = 'original script assertion'
    $script:runPhase = 'application-failed'
    $script:runStartedUtc = [DateTime]::UtcNow.ToString('o')
    $Scenario = 'ScreenshotArchiveProbe'
    $dataDir = $probeRoot
    $SkipBuild = $true

    Save-UiSmokeEvidence
    if ($script:runExitCode -ne 1 -or $script:failureMessage -ne 'original script assertion') {
        throw 'broken screenshot manifest replaced the primary scenario failure'
    }
    if ($script:archiveErrors.Count -ne 1 -or
        -not ([string]$script:archiveErrors[0]).StartsWith('screenshots:')) {
        throw "screenshot archive error was not recorded: $($script:archiveErrors)"
    }
    $metadata = Get-Content -LiteralPath (Join-Path $probeRoot 'run-metadata.json') -Raw -Encoding UTF8 |
        ConvertFrom-Json
    if ($metadata.runner_exit_code -ne 1 -or $metadata.exit_code -ne 1 -or
        $metadata.failure -ne 'original script assertion' -or
        $metadata.archive_errors.Count -ne 1) {
        throw 'run metadata lost the original failure or supplementary archive error'
    }

    $script:runExitCode = 0
    $script:failureMessage = $null
    Apply-UiSmokeArchiveDisposition
    if ($script:runExitCode -ne 2 -or
        $script:failureMessage -ne 'one or more evidence files could not be collected') {
        throw 'broken evidence from an otherwise successful run must still fail'
    }

    [System.IO.File]::WriteAllText(
        (Join-Path $shotsDir 'manifest.jsonl'),
        '{"status":"skipped","label":"two-detached","viewport":"detached-1-1","reason":"native viewport is minimized","frame":8,"timestamp_ms":1001}')
    $script:evidenceEntries = New-Object System.Collections.ArrayList
    Register-UiSmokeScreenshots
    if ($script:evidenceEntries.Count -ne 1 -or
        $script:evidenceEntries[0].path -ne 'screenshots/manifest.jsonl') {
        throw 'skipped screenshot must be recorded by the manifest without a PNG'
    }
}
finally {
    if (Test-Path -LiteralPath $probeRoot) {
        $resolvedRoot = Get-NormalizedPath $runsRoot
        $resolvedProbe = Get-NormalizedPath $probeRoot
        if (-not $resolvedProbe.StartsWith(($resolvedRoot + '\'), [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "refusing to clean unexpected screenshot probe path: $resolvedProbe"
        }
        Assert-NoReparseTree $probeRoot 'screenshot-test'
        Remove-Item -LiteralPath $probeRoot -Recurse -Force
    }
}

Write-Host '[ui-smoke-screenshots-test] PASS original-failure=1 broken-manifest=1 success-disposition=1 skipped=1'
