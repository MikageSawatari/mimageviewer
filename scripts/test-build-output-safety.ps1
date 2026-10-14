# Headless regression: real Cargo metadata, disposable workspaces, no compilation.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-True {
    param([bool] $Condition, [string] $Message)
    if (-not $Condition) { throw "[build-output-safety-test] $Message" }
}
function Write-Config {
    param([string] $Path, [string] $Text, [bool] $Bom = $false)
    $null = New-Item -ItemType Directory -Force -Path (Split-Path $Path -Parent)
    [IO.File]::WriteAllText($Path, $Text, (New-Object Text.UTF8Encoding($Bom)))
}
function Assert-Preflight {
    param([bool] $Reject, [string] $Label)
    $message = $null
    $caller = (Get-Location).Path
    try { . $script:preflight $script:fakeScripts } catch { $message = $_.Exception.Message }
    Assert-True ((Get-Location).Path -eq $caller) "working directory not restored: $Label"
    if ($Reject) {
        Assert-True ($message -like '[[]build-output[]]*fixed*output*path*') "override accepted or wrong refusal: $Label ($message)"
        Assert-True ($message -notmatch '[^\x00-\x7f]') "non-ASCII refusal: $Label"
    } else {
        Assert-True (-not $message) "normal setting rejected: $Label ($message)"
    }
    $script:cases++
}

$testRoot = Join-Path (Split-Path $PSScriptRoot -Parent) 'target'
$fakeRoot = Join-Path $testRoot ('miv-output-' + [guid]::NewGuid().ToString('N'))
$fakeRepo = Join-Path $fakeRoot ('repo-' + [char]0x65e5)
$fakeCaller = Join-Path $fakeRoot 'caller'
$script:fakeScripts = Join-Path $fakeRepo 'scripts'
$null = New-Item -ItemType Directory -Path $script:fakeScripts
$null = New-Item -ItemType Directory -Path $fakeCaller
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'build-output-safety.ps1') -Destination $script:fakeScripts
# Workspace markers isolate these dependency-free packages from the real repo.
foreach ($directory in @($fakeRepo, $fakeCaller)) {
    Write-Config (Join-Path $directory 'Cargo.toml') "[package]`nname='fake-output'`nversion='0.1.0'`nedition='2021'`n[workspace]`n"
    Write-Config (Join-Path $directory 'src\main.rs') 'fn main() {}'
    Write-Config (Join-Path $directory 'cargo-home\config.toml') "[build]`njobs=1`n"
}
$savedEnv = @{}
$variables = @('CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET_DIR', 'CARGO_BUILD_TARGET', 'CARGO_BUILD_BUILD_DIR', 'CARGO_HOME')
foreach ($name in $variables) {
    $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    [Environment]::SetEnvironmentVariable($name, $null, 'Process')
}
$env:CARGO_HOME = 'cargo-home'
$script:cases = 0
Push-Location $fakeRepo
try {
    foreach ($scriptName in @('build-dev.ps1', 'build-release.ps1', 'build-dist.ps1', 'build-portable.ps1')) {
        $tokens = $null
        $errors = $null
        $ast = [Management.Automation.Language.Parser]::ParseFile(
            (Join-Path $PSScriptRoot $scriptName), [ref] $tokens, [ref] $errors)
        Assert-True ($errors.Count -eq 0) "parse failed: $scriptName"
        $statements = @($ast.EndBlock.Statements)
        Assert-True ($statements[0].Extent.Text -eq "`$ErrorActionPreference = 'Stop'") "unexpected preflight ordering: $scriptName"
        Assert-True ($statements[1].Extent.Text.Contains("'build-output-safety.ps1'")) "missing early import: $scriptName"
        Assert-True ($statements[2] -is [Management.Automation.Language.PipelineAst]) "output guard is conditional: $scriptName"
        $command = $statements[2].PipelineElements[0]
        Assert-True ($command.GetCommandName() -eq 'Assert-MivFixedCargoOutput') "missing early output guard: $scriptName"
        $script:preflight = [scriptblock]::Create("param([string] `$PSScriptRoot)`n" + (($statements[0..2] | ForEach-Object { $_.Extent.Text }) -join "`n"))
        $portable = $scriptName -eq 'build-portable.ps1'
        # dev/portable are deliberately invoked outside the Cargo working dir.
        if ($scriptName -in @('build-dev.ps1', 'build-portable.ps1')) { Set-Location $fakeCaller } else { Set-Location $fakeRepo }
        foreach ($PreserveRuntime in @($false, $true)) {
            $SmokeTestScript = $false
            $CargoArgs = @()
            Assert-Preflight $false "$scriptName default PreserveRuntime=$PreserveRuntime"
            $repoConfig = Join-Path $fakeRepo '.cargo\config.toml'
            Write-Config $repoConfig "[build]`njobs=1`n" $true
            Assert-Preflight $false "$scriptName BOM normal config"
            Write-Config $repoConfig "[build]`ntarget-dir='other-target'`n" $true
            # Portable's own fixed CLI directory overrides env/config, as in build.
            Assert-Preflight (-not $portable) "$scriptName BOM output config"
            Remove-Item -LiteralPath $repoConfig
            $relativeHomeConfig = Join-Path $fakeRepo 'cargo-home\config.toml'
            Write-Config $relativeHomeConfig "[build]`ntarget-dir='other-home-target'`n" $true
            Assert-Preflight (-not $portable) "$scriptName relative CARGO_HOME output config"
            Write-Config $relativeHomeConfig "[build]`njobs=1`n" $true
            $env:CARGO_TARGET_DIR = 'other-env-target'
            # Cargo's legacy CARGO_TARGET_DIR also wins over CLI --config.
            Assert-Preflight $true "$scriptName CARGO_TARGET_DIR"
            Remove-Item Env:CARGO_TARGET_DIR
            $env:CARGO_BUILD_TARGET_DIR = 'other-build-target'
            Assert-Preflight (-not $portable) "$scriptName CARGO_BUILD_TARGET_DIR"
            Remove-Item Env:CARGO_BUILD_TARGET_DIR
            $env:CARGO_BUILD_TARGET = 'x86_64-pc-windows-msvc'
            Assert-Preflight $true "$scriptName CARGO_BUILD_TARGET"
            Remove-Item Env:CARGO_BUILD_TARGET
            if ($portable) {
                $SmokeTestScript = $true
                Assert-Preflight $false 'portable diagnostic fixed target'
                $SmokeTestScript = $false
            }
            if ($scriptName -eq 'build-release.ps1') {
                $CargoArgs = $null
                Assert-Preflight $false 'release omitted arguments under StrictMode'
                foreach ($argsCase in @(@('--target-dir', 'other'), @('--target-dir=target'), @('--profile=dev'))) {
                    $CargoArgs = $argsCase
                    Assert-Preflight $true 'release output argument'
                }
                $CargoArgs = @('--config', "build.target-dir='other-cli-target'")
                Assert-Preflight $true 'release inline config output'
                $configFile = Join-Path $fakeRoot 'extra.toml'
                Write-Config $configFile "[build]`njobs=1`n" $true
                $CargoArgs = @('--config', $configFile)
                Assert-Preflight $false 'release BOM --config file normal'
                Write-Config $configFile "[build]`ntarget-dir='other-file-target'`n" $true
                Assert-Preflight $true 'release BOM --config file output'
                $CargoArgs = @('--locked')
                Assert-Preflight $false 'release explicit locked metadata'
                $CargoArgs = @()
            }
        }
    }

    # A fake Cargo verifies command/working-dir contracts and failure behavior;
    # config discovery itself was tested above by real Cargo, never reimplemented.
    $realCases = $script:cases
    . (Join-Path $PSScriptRoot 'build-output-safety.ps1')
    $script:mode = 'normal'
    $script:answerDirectory = Join-Path $fakeRepo 'target'
    function cargo {
        $script:capturedArgs = @($args)
        $script:capturedCwd = (Get-Location).Path
        $global:LASTEXITCODE = 0
        if ($script:mode -eq 'exit') {
            $global:LASTEXITCODE = 7
            return (@{ target_directory = $script:answerDirectory } | ConvertTo-Json -Compress)
        }
        if ($script:mode -eq 'json') { return 'invalid-json' }
        $dir = if ($script:mode -eq 'mismatch') { Join-Path $fakeRepo 'target-sibling' } else { $script:answerDirectory }
        return (@{ target_directory = $dir } | ConvertTo-Json -Compress)
    }
    Set-Location $fakeCaller
    Assert-MivFixedCargoOutput -WorkingDirectory $fakeRepo
    Assert-True ($script:capturedCwd -eq $fakeRepo) 'metadata used caller cwd'
    Assert-True (($script:capturedArgs -join ' ') -eq 'metadata --format-version 1 --no-deps --offline') 'metadata command contract changed'
    foreach ($script:mode in @('exit', 'json', 'mismatch')) {
        $message = $null
        try { Assert-MivFixedCargoOutput -WorkingDirectory $fakeRepo } catch { $message = $_.Exception.Message }
        Assert-True ($message -like '[[]build-output[]]*fixed*output*path*') "metadata failure accepted: $script:mode"
        Assert-True ((Get-Location).Path -eq $fakeCaller) 'failed metadata did not restore caller cwd'
    }
    $script:mode = 'normal'
    foreach ($SmokeTestScript in @($false, $true)) {
        $name = if ($SmokeTestScript) { 'target-portable-test-script' } else { 'target-portable' }
        $script:answerDirectory = Join-Path $fakeRepo $name
        Assert-Preflight $false 'portable actual call and fixed target contract'
        Assert-True (($script:capturedArgs -join ' ').Contains($name)) 'portable metadata used wrong fixed directory'
        Assert-True ($script:capturedCwd -eq $fakeRepo) 'portable metadata used caller cwd'
    }
} finally {
    Pop-Location
    foreach ($name in $variables) { [Environment]::SetEnvironmentVariable($name, $savedEnv[$name], 'Process') }
    $resolved = [IO.Path]::GetFullPath($fakeRoot)
    $tempPrefix = [IO.Path]::GetFullPath($testRoot).TrimEnd('\') + '\'
    Assert-True ($resolved.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase) -and (Split-Path $resolved -Leaf) -like 'miv-output-*') 'unsafe temporary cleanup path'
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
Write-Host ("[build-output-safety-test] PASS: {0} real Cargo preflight cases plus metadata contract/error and portable call tests" -f $realCases)
