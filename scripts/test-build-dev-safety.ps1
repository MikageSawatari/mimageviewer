# Script-only regression checks for build-dev.ps1 process handling.
# This script does not run Cargo, build or launch mImageViewer, stop real
# processes, or touch application data. Process commands are mocked.

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$buildDevPath = Join-Path $PSScriptRoot 'build-dev.ps1'

function Assert-True {
    param(
        [bool] $Condition,
        [string] $Message
    )
    if (-not $Condition) { throw "[build-dev-safety-test] $Message" }
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

$buildDevAst = Get-ScriptAst $buildDevPath
$buildDev = Get-Content -LiteralPath $buildDevPath -Raw -Encoding UTF8

Assert-True ($buildDev.Contains('[switch] $PreserveRuntime')) 'build-dev lacks -PreserveRuntime'

$stopFunctions = @($buildDevAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Stop-StagedProcess'
}, $true))
Assert-True ($stopFunctions.Count -eq 1) 'Stop-StagedProcess is missing or ambiguous'
$stopFunction = $stopFunctions[0]
$stopFunctionText = $stopFunction.Extent.Text

$preserveGuards = @($stopFunction.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.IfStatementAst] -and
        $node.Clauses[0].Item1.Extent.Text -eq '$PreserveRuntime'
}, $true))
Assert-True ($preserveGuards.Count -eq 1) 'preserve-runtime guard is missing or ambiguous'
$preserveBody = $preserveGuards[0].Clauses[0].Item2.Extent.Text
Assert-True ($preserveBody.Contains('throw')) 'preserve-runtime guard does not fail the build'
Assert-True (-not $preserveBody.Contains('Stop-Process')) 'preserve-runtime guard can stop a process'
Assert-True ($stopFunctionText.Contains('Stop-Process -Id $_.Id -Force -ErrorAction Stop')) 'default process-stop behavior was removed'

$stopCalls = @($buildDevAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and
        $node.GetCommandName() -eq 'Stop-StagedProcess'
}, $true))
Assert-True ($stopCalls.Count -eq 2) 'build-dev must check exactly the staged core and remote service'
$stopCallText = @($stopCalls | ForEach-Object { $_.Extent.Text })
Assert-True ($stopCallText[0].Contains("-ExeName 'mimageviewer-core'") -and
    $stopCallText[0].Contains('-ExePath $coreExe') -and
    $stopCallText[0].Contains('-PreserveRuntime:$PreserveRuntime')) 'first staged-process check does not preserve the exact core policy'
Assert-True ($stopCallText[1].Contains("-ExeName 'mimageviewer-remote'") -and
    $stopCallText[1].Contains('-ExePath $remoteExe') -and
    $stopCallText[1].Contains('-PreserveRuntime:$PreserveRuntime')) 'second staged-process check does not preserve the exact remote policy'

$cargoCalls = @($buildDevAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and
        $node.GetCommandName() -eq 'cargo'
}, $true))
Assert-True ($cargoCalls.Count -eq 2) 'build-dev Cargo call count changed'

$powershellCalls = @($buildDevAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and
        $node.GetCommandName() -in @('powershell', 'powershell.exe', 'pwsh', 'pwsh.exe')
}, $true))
Assert-True ($powershellCalls.Count -eq 0) 'build-dev unexpectedly invokes another PowerShell script'

$tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\')
$testRoot = Join-Path $tempRoot ("miv-build-dev-safety-{0}" -f [guid]::NewGuid().ToString('N'))
$testRootFull = [System.IO.Path]::GetFullPath($testRoot).TrimEnd('\')
Assert-True ($testRootFull.StartsWith(($tempRoot + '\'), [System.StringComparison]::OrdinalIgnoreCase)) 'temporary test root escaped the OS temp directory'
New-Item -ItemType Directory -Path $testRootFull | Out-Null

try {
    $script:mockProcessPath = $null
    $script:mockStopCalls = 0

    function Get-Process {
        param(
            [string] $Name,
            $ErrorAction
        )
        return [pscustomobject]@{
            Id = 4242
            Path = $script:mockProcessPath
        }
    }

    function Stop-Process {
        param(
            [int] $Id,
            [switch] $Force,
            $ErrorAction
        )
        $script:mockStopCalls++
    }

    Invoke-Expression $stopFunctionText

    foreach ($case in @(
        @{ Name = 'mimageviewer-core'; File = 'mimageviewer-core.exe'; Label = 'core' },
        @{ Name = 'mimageviewer-remote'; File = 'mimageviewer-remote.exe'; Label = 'remote service' }
    )) {
        $exePath = Join-Path $testRootFull $case.File
        Set-Content -LiteralPath $exePath -Value 'mock executable' -Encoding Ascii
        $script:mockProcessPath = $exePath

        $script:mockStopCalls = 0
        $message = $null
        try {
            Stop-StagedProcess -ExeName $case.Name -ExePath $exePath -Label $case.Label `
                -PreserveRuntime
        } catch {
            $message = $_.Exception.Message
        }
        Assert-True ([bool] $message) "-PreserveRuntime did not reject the running $($case.Label)"
        Assert-True ($message.Contains('-PreserveRuntime refuses to stop it')) "-PreserveRuntime rejection changed for $($case.Label)"
        Assert-True ($script:mockStopCalls -eq 0) "-PreserveRuntime reached Stop-Process for $($case.Label)"

        $script:mockStopCalls = 0
        Stop-StagedProcess -ExeName $case.Name -ExePath $exePath -Label $case.Label
        Assert-True ($script:mockStopCalls -eq 1) "default behavior did not stop the exact staged $($case.Label)"

        $script:mockProcessPath = Join-Path $testRootFull ("other-{0}" -f $case.File)
        $script:mockStopCalls = 0
        Stop-StagedProcess -ExeName $case.Name -ExePath $exePath -Label $case.Label `
            -PreserveRuntime
        Assert-True ($script:mockStopCalls -eq 0) "-PreserveRuntime changed the exact-path filter for $($case.Label)"
    }
} finally {
    if (Test-Path -LiteralPath $testRootFull -PathType Container) {
        Remove-Item -LiteralPath $testRootFull -Recurse -Force
    }
}

Write-Host '[build-dev-safety-test] PASS'
