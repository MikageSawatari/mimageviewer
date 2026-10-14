# Headless regression: only the actual early guard runs, in a fake environment.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-True {
    param([bool] $Condition, [string] $Message)
    if (-not $Condition) { throw "[build-output-safety-test] $Message" }
}

& python (Join-Path $PSScriptRoot 'test_build_output_config.py')
Assert-True ($LASTEXITCODE -eq 0) 'config regression tests failed'

$testRoot = Join-Path (Split-Path $PSScriptRoot -Parent) 'target'
$fakeRoot = Join-Path $testRoot ('miv-output-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $fakeRoot
$savedEnv = @{}
$outputVariables = @('CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET_DIR', 'CARGO_BUILD_BUILD_DIR', 'CARGO_BUILD_TARGET', 'CARGO_BUILD_ARTIFACT_DIR', 'CARGO_BUILD_OUT_DIR')
$variables = @($outputVariables) + @('CARGO_HOME', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS')
$variables += @(Get-ChildItem Env: | Where-Object { $_.Name -like 'CARGO_TARGET_*_RUSTFLAGS' } | ForEach-Object { $_.Name })
foreach ($name in $variables) {
    $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    [Environment]::SetEnvironmentVariable($name, $null, 'Process')
}
$env:CARGO_HOME = Join-Path $fakeRoot 'cargo-home'
$null = New-Item -ItemType Directory -Path $env:CARGO_HOME
$cwdConfigDir = Join-Path $fakeRoot '.cargo'
$null = New-Item -ItemType Directory -Path $cwdConfigDir
Push-Location $fakeRoot
try {
    foreach ($scriptName in @('build-dev.ps1', 'build-release.ps1', 'build-dist.ps1', 'build-portable.ps1')) {
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile(
            (Join-Path $PSScriptRoot $scriptName), [ref] $tokens, [ref] $errors)
        Assert-True ($errors.Count -eq 0) "parse failed: $scriptName"
        $statements = @($ast.EndBlock.Statements)
        # Before signing preflight, process queries, clean, tests, or builds.
        Assert-True ($statements[0].Extent.Text -eq "`$ErrorActionPreference = 'Stop'") "unexpected preflight ordering: $scriptName"
        Assert-True ($statements[1].Extent.Text.Contains("'build-output-safety.ps1'")) "missing early import: $scriptName"
        Assert-True ($statements[2] -is [System.Management.Automation.Language.PipelineAst]) "output guard is conditional: $scriptName"
        $command = $statements[2].PipelineElements[0]
        Assert-True ($command.GetCommandName() -eq 'Assert-MivFixedCargoOutput') "missing early output guard: $scriptName"
        $preflight = [scriptblock]::Create("param([string] `$PSScriptRoot)`n" + (($statements[0..2] | ForEach-Object { $_.Extent.Text }) -join "`n"))
        foreach ($PreserveRuntime in @($false, $true)) {
            foreach ($name in $outputVariables) {
                $CargoArgs = @()
                [Environment]::SetEnvironmentVariable($name, 'another-target', 'Process')
                $message = $null
                try { . $preflight $PSScriptRoot } catch { $message = $_.Exception.Message }
                [Environment]::SetEnvironmentVariable($name, $null, 'Process')
                Assert-True ($message -like '[[]build-output[]]*fixed output paths*') "override accepted: $scriptName $name PreserveRuntime=$PreserveRuntime"
                Assert-True ($message -notmatch '[^\x00-\x7f]') 'non-ASCII refusal reason'
            }
            if ($scriptName -eq 'build-release.ps1') {
                foreach ($argsCase in @(@('--target-dir', 'another-target'), @('--target-dir=target'), @('--config', 'build.target-dir="other"'), @('--profile=dev'))) {
                    $CargoArgs = $argsCase
                    $message = $null
                    try { . $preflight $PSScriptRoot } catch { $message = $_.Exception.Message }
                    Assert-True ($message -like '[[]build-output[]]*fixed output paths*') 'release argument override accepted'
                }
            }
        }
        $CargoArgs = @()
        $homeConfig = Join-Path $env:CARGO_HOME 'config.toml'
        Set-Content -LiteralPath $homeConfig -Encoding ASCII -Value '[build]', 'target-dir="another-target"'
        $message = $null
        try { . $preflight $PSScriptRoot } catch { $message = $_.Exception.Message }
        Remove-Item -LiteralPath $homeConfig
        Assert-True ($message -like '[[]build-output[]]*fixed output paths*') "config override accepted: $scriptName"
        # dev/portable change directory to the repo; release/dist use the caller
        # directory. The checker must inspect where Cargo will actually run.
        $cwdConfig = Join-Path $cwdConfigDir 'config.toml'
        Set-Content -LiteralPath $cwdConfig -Encoding ASCII -Value '[build]', 'target-dir="another-target"'
        $message = $null
        try { . $preflight $PSScriptRoot } catch { $message = $_.Exception.Message }
        Remove-Item -LiteralPath $cwdConfig
        if ($scriptName -in @('build-dev.ps1', 'build-portable.ps1')) {
            Assert-True (-not $message) "inspected caller instead of Cargo directory: $scriptName"
        } else {
            Assert-True ($message -like '[[]build-output[]]*fixed output paths*') "missed caller Cargo config: $scriptName"
        }
        . $preflight $PSScriptRoot
    }
} finally {
    Pop-Location
    foreach ($name in $variables) {
        [Environment]::SetEnvironmentVariable($name, $savedEnv[$name], 'Process')
    }
    $resolved = [System.IO.Path]::GetFullPath($fakeRoot)
    $tempPrefix = [System.IO.Path]::GetFullPath($testRoot).TrimEnd('\') + '\'
    Assert-True ($resolved.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase) -and (Split-Path $resolved -Leaf) -like 'miv-output-*') 'unsafe temporary cleanup path'
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
Write-Host '[build-output-safety-test] PASS: fixed output guard, both runtime policies, fake config and environment'
