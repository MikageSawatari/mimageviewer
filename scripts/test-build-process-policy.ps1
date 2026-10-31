# Script-only tests: execute extracted policy blocks with mocked process/build
# commands. No product, real process stop, Cargo build, signing, or APPDATA I/O.
[CmdletBinding()]
param([switch] $PowerShellOnly)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$policyScriptRoot = $PSScriptRoot
. (Join-Path $PSScriptRoot 'build-process-policy.ps1')

function Assert-Policy {
    param([bool] $Condition, [string] $Message)
    if (-not $Condition) { throw "[build-process-policy-test] $Message" }
}
function Read-PolicyAst {
    param([string] $Name)
    $tokens = $null
    $errors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $Name), [ref] $tokens, [ref] $errors)
    Assert-Policy ($errors.Count -eq 0) "parse failed: $Name"
    return $ast
}
function Find-PolicyAssignment {
    param($Ast, [string] $Name)
    $matches = @($Ast.FindAll({
        param($node)
        $node -is [Management.Automation.Language.AssignmentStatementAst] -and
            $node.Left.Extent.Text -eq ('$' + $Name)
    }, $true))
    Assert-Policy ($matches.Count -eq 1) "ambiguous assignment: $Name"
    return $matches[0].Extent.Text
}

$releaseAst = Read-PolicyAst 'build-release.ps1'
$portableAst = Read-PolicyAst 'build-portable.ps1'
$distAst = Read-PolicyAst 'build-dist.ps1'
$shell = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'build-release.sh') -Raw -Encoding UTF8
$script:policyProcesses = @()
$script:policyStopped = @()
function Get-Process {
    param($ErrorAction)
    return $script:policyProcesses
}
function Stop-Process {
    param([int] $Id, [switch] $Force, $ErrorAction)
    $script:policyStopped += $Id
}
function cargo {
    $script:policyBuildCalls++
    $script:policyCargoArguments = @($args)
    $global:LASTEXITCODE = $script:policyCargoExit
    return $script:policyCargoMessages
}
$script:policyCargoExit = 0
$script:policyCargoMessages = @()

# Fixed output preflight must be the first effective operation in both PS
# entry points. Execute only that prefix, then mocked stop/build sentinels.
$environmentNames = @('CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'CARGO_BUILD_TARGET_DIR', 'MIV_POLICY_LOG', 'MIV_CARGO_ARTIFACT_JSON', 'MIV_CARGO_EXPECTED')
$savedEnvironment = @{}
foreach ($name in $environmentNames) {
    $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    Remove-Item "Env:$name" -ErrorAction SilentlyContinue
}
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
$testRoot = Join-Path $tempRoot ('miv-build-policy-' + [guid]::NewGuid().ToString('N'))
Assert-Policy ($testRoot.StartsWith($tempRoot + '\', [StringComparison]::OrdinalIgnoreCase)) 'temp path escaped'
New-Item -ItemType Directory -Path $testRoot | Out-Null
try {
    foreach ($ast in @($releaseAst, $distAst)) {
        $preflight = @($ast.FindAll({
            param($node)
            $node -is [Management.Automation.Language.CommandAst] -and
                $node.GetCommandName() -eq 'Assert-MivReleaseOutputContract'
        }, $true))
        Assert-Policy ($preflight.Count -eq 1) 'missing output preflight'
        $statements = @($ast.EndBlock.Statements)
        Assert-Policy ($statements[0].Extent.Text -eq "`$ErrorActionPreference = 'Stop'") 'side effect before output preflight'
        Assert-Policy ($statements[1].Extent.Text.Contains("'build-process-policy.ps1'")) 'policy import order changed'
        Assert-Policy ($statements[2].Extent.Text -eq $preflight[0].Extent.Text) 'output preflight must precede all process/build/sign operations'
        $prefix = ($statements[0..2] | ForEach-Object { $_.Extent.Text }) -join "`n"
        $prefix = $prefix.Replace('$PSScriptRoot', '$policyScriptRoot')
        if ($ast -eq $releaseAst) {
            foreach ($option in @('--target-dir', '--target', '--profile', '--out-dir', '--artifact-dir', '--config', '--message-format', '--unknown')) {
                foreach ($arguments in @(@($option, 'unexpected-output'), @($option + '=unexpected-output'))) {
                    $CargoArgs = $arguments
                    $script:policyStopped = @()
                    $script:policyBuildCalls = 0
                    $rejected = $false
                    $failureMessage = ''
                    try { Invoke-Expression $prefix; Stop-Process -Id 999; cargo build } catch {
                        $failureMessage = $_.Exception.Message
                        $rejected = $_.Exception.Message.Contains('release outputs are fixed')
                    }
                    Assert-Policy ($rejected -and $script:policyStopped.Count -eq 0 -and $script:policyBuildCalls -eq 0) "CLI override reached stop/build: $arguments; $failureMessage"
                }
            }
            $CargoArgs = @('--features', 'example')
            Invoke-Expression $prefix
            Assert-Policy ($CargoArgs.Count -eq 2 -and $CargoArgs[1] -eq 'example') 'ordinary Cargo args changed'
            foreach ($CargoArgs in @(
                @('--features=example'), @('--all-features'), @('--no-default-features'), @('--locked'),
                @('--frozen'), @('--offline'), @('-v'), @('--verbose'), @('-j', '2'), @('--jobs', '2')
            )) { Invoke-Expression $prefix }
            foreach ($CargoArgs in @(@('--features'), @('-j'), @('--jobs=2'), @('-j2'), @('--jobs', '--config'))) {
                $rejected = $false
                try { Invoke-Expression $prefix } catch { $rejected = $_.Exception.Message.Contains('release outputs are fixed') }
                Assert-Policy $rejected "invalid allow-list form accepted: $CargoArgs"
            }
        }
    }

    # Execute real selection/decision blocks with installed parent/child mocks,
    # exact build outputs, sibling outputs and inaccessible process paths.
    $releaseDir = Join-Path $repoRoot 'target\release'
    $portableSource = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'build-portable.ps1') -Raw -Encoding UTF8
    $portableStart = $portableSource.IndexOf('$portableTargetName =')
    $portableEnd = $portableSource.IndexOf('# ---------------------------------------------------------------------------', $portableStart)
    $portableSetup = $portableSource.Substring($portableStart, $portableEnd - $portableStart)
    & {
        foreach ($SmokeTestScript in @($true, $false)) {
            foreach ($SkipBuild in @($true, $false)) {
                $version = 'fixture'
                Invoke-Expression $portableSetup
                $targetMcp = Join-Path $portableTargetDir 'release\mimageviewer-mcp.exe'
                Assert-Policy (($exactBuildOutputPaths -contains $targetMcp) -eq (-not $SkipBuild)) 'portable stop policy ignored whether it builds MCP'
                Assert-Policy ($exactBuildOutputPaths -contains (Join-Path $pkgDir 'mimageviewer.exe')) 'portable package executable is missing from exact output policy'
                Assert-Policy ($exactBuildOutputPaths -contains (Join-Path $pkgDir 'mimageviewer-mcp.exe')) 'portable package MCP is missing from exact output policy'
            }
        }
    }
    $invokeDefinition = @($releaseAst.FindAll({
        param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Invoke-ReleaseCargo'
    }, $true))
    Assert-Policy ($invokeDefinition.Count -eq 1) 'release Cargo wrapper missing'
    Invoke-Expression $invokeDefinition[0].Extent.Text
    foreach ($name in @('mimageviewer-core', 'mimageviewer-remote', 'mimageviewer-epub-pdf', 'mimageviewer-mcp', 'mimageviewer')) {
        $expectedExecutable = Join-Path $releaseDir ($name + '.exe')
        foreach ($fresh in @($true, $false)) {
            $script:policyCargoMessages = @(@{ reason = 'compiler-artifact'; executable = $expectedExecutable; fresh = $fresh } | ConvertTo-Json -Compress)
            $result = Invoke-ReleaseCargo -Args @('build', '--release', '--bin', $name) -ExpectedExecutable $expectedExecutable
            Assert-Policy ($result -eq 0) "artifact accepted path failed: $name fresh=$fresh"
            Assert-Policy ($script:policyCargoArguments -contains '--message-format=json-render-diagnostics') 'Cargo artifact JSON was not requested'
            $script:policyCargoMessages = @(@{ reason = 'compiler-artifact'; executable = (Join-Path $testRoot ($name + '.exe')); fresh = $fresh } | ConvertTo-Json -Compress)
            $afterValidation = 0
            $rejected = $false
            try {
                Invoke-ReleaseCargo -Args @('build', '--release', '--bin', $name) -ExpectedExecutable $expectedExecutable | Out-Null
                $afterValidation++ # represents signing / next build / embedding
            } catch { $rejected = $_.Exception.Message.Contains('output mismatch') }
            Assert-Policy ($rejected -and $afterValidation -eq 0) "wrong artifact reached next stage: $name fresh=$fresh"
        }
    }
    $unicodeRoot = Join-Path $testRoot ([string][char]0x65E5 + [char]0x672C + [char]0x8A9E)
    $unicodeExecutable = Join-Path $unicodeRoot 'target\release\mimageviewer-mcp.exe'
    $script:policyCargoMessages = @(@{ reason = 'compiler-artifact'; executable = $unicodeExecutable; fresh = $true } | ConvertTo-Json -Compress)
    $originalEncoding = [Console]::OutputEncoding
    Assert-Policy ((Invoke-ReleaseCargo -Args @('build') -ExpectedExecutable $unicodeExecutable) -eq 0) 'Unicode artifact path was not preserved'
    Assert-Policy ([Console]::OutputEncoding.CodePage -eq $originalEncoding.CodePage) 'console encoding was not restored'
    $nativeCargoScript = Join-Path $testRoot 'native-cargo.py'
    [IO.File]::WriteAllText($nativeCargoScript, @'
import json, os, sys
sys.stdout.buffer.write((json.dumps({"reason": "compiler-artifact", "executable": os.environ["MIV_CARGO_EXPECTED"], "fresh": True}, ensure_ascii=False) + "\n").encode("utf-8"))
'@, (New-Object Text.UTF8Encoding($false)))
    $env:MIV_CARGO_EXPECTED = $unicodeExecutable
    $mockCargoFunction = (Get-Item Function:cargo).ScriptBlock
    try {
        Set-Item Function:script:cargo -Value { & python $nativeCargoScript }
        [Console]::OutputEncoding = [Text.Encoding]::GetEncoding(932)
        Assert-Policy ((Invoke-ReleaseCargo -Args @('build') -ExpectedExecutable $unicodeExecutable) -eq 0) 'native UTF-8 Cargo JSON path was corrupted under CP932'
        Assert-Policy ([Console]::OutputEncoding.CodePage -eq 932) 'native stdout test did not restore CP932'
    } finally {
        [Console]::OutputEncoding = $originalEncoding
        Set-Item Function:script:cargo -Value $mockCargoFunction
    }
    $script:policyCargoMessages = @('{"reason":"build-finished","success":true}')
    $rejected = $false
    try { Invoke-ReleaseCargo -Args @('build') -ExpectedExecutable $expectedExecutable | Out-Null }
    catch { $rejected = $_.Exception.Message.Contains('did not report') }
    Assert-Policy $rejected 'missing artifact accepted'
    $script:policyCargoExit = 37
    Assert-Policy ((Invoke-ReleaseCargo -Args @('build') -ExpectedExecutable $expectedExecutable) -eq 37) 'Cargo failure exit was lost'
    $script:policyCargoExit = 0
    $script:policyCargoMessages = @()

    $releaseExe = Join-Path $releaseDir 'mimageviewer.exe'
    $releaseMcpExe = Join-Path $releaseDir 'mimageviewer-mcp.exe'
    $exactBuildProcessNames = @('mimageviewer', 'mimageviewer-mcp')
    $exactBuildOutputPaths = @($releaseExe, $releaseMcpExe)
    $stoppableProcessNames = @('mimageviewer', 'mimageviewer-mcp', 'mimageviewer-core')
    $runningNames = $stoppableProcessNames
    $repoRootPrefixLower = ($repoRoot.TrimEnd('\') + '\').ToLowerInvariant()
    $repoPrefix = $repoRootPrefixLower
    $appDataRootPrefixLower = 'C:\mock-appdata\mimageviewer\'
    $appDataProcessNames = @('mimageviewer-core')
    $installed = @(
        [pscustomobject]@{ Name = 'mimageviewer'; ProcessName = 'mimageviewer'; Id = 1; Path = 'C:\Program Files\mIV\mimageviewer.exe' },
        [pscustomobject]@{ Name = 'mimageviewer-mcp'; ProcessName = 'mimageviewer-mcp'; Id = 2; Path = 'C:\mock-appdata\mimageviewer\runtime\v1\mimageviewer-mcp.exe' },
        [pscustomobject]@{ Name = 'mimageviewer'; ProcessName = 'mimageviewer'; Id = 3; Path = $null },
        [pscustomobject]@{ Name = 'mimageviewer-mcp'; ProcessName = 'mimageviewer-mcp'; Id = 4; Path = $null },
        [pscustomobject]@{ Name = 'mimageviewer'; ProcessName = 'mimageviewer'; Id = 5; Path = (Join-Path $repoRoot 'target\other\mimageviewer.exe') }
    )
    $inaccessible = [pscustomobject]@{ Name = 'mimageviewer-mcp'; ProcessName = 'mimageviewer-mcp'; Id = 6 }
    $inaccessible | Add-Member -MemberType ScriptProperty -Name Path -Value { throw 'path unavailable' }
    $installed += $inaccessible
    $outputs = @(
        [pscustomobject]@{ Name = 'mimageviewer'; ProcessName = 'mimageviewer'; Id = 10; Path = $releaseExe.ToUpperInvariant() },
        [pscustomobject]@{ Name = 'mimageviewer-mcp'; ProcessName = 'mimageviewer-mcp'; Id = 11; Path = $releaseMcpExe }
    )
    $releaseLoop = @($releaseAst.FindAll({
        param($node)
        $node -is [Management.Automation.Language.ForEachStatementAst] -and $node.Condition.Extent.Text -eq '$candidates'
    }, $true))
    Assert-Policy ($releaseLoop.Count -eq 1) 'release selection missing'
    $releaseGuard = @($releaseAst.FindAll({
        param($node)
        $node -is [Management.Automation.Language.IfStatementAst] -and
            $node.Clauses[0].Item1.Extent.Text -eq '$PreserveRuntime -and $toKill.Count -gt 0'
    }, $true))[0].Extent.Text
    $portableGuard = @($portableAst.FindAll({
        param($node)
        $node -is [Management.Automation.Language.IfStatementAst] -and
            $node.Clauses[0].Item1.Extent.Text -eq '$KeepRunning' -and $node.Extent.Text.Contains('Stop-Process')
    }, $true))[0].Extent.Text
    $distSelection = Find-PolicyAssignment $distAst 'running'
    $distGuard = @($distAst.FindAll({
        param($node)
        $node -is [Management.Automation.Language.IfStatementAst] -and
            $node.Clauses[0].Item1.Extent.Text -eq '$running.Count -gt 0'
    }, $true))[0].Extent.Text
    foreach ($includeOutputs in @($false, $true)) {
        $script:policyProcesses = @($installed)
        if ($includeOutputs) { $script:policyProcesses += $outputs }
        $candidates = $script:policyProcesses
        $toKill = @()
        Invoke-Expression $releaseLoop[0].Extent.Text
        Assert-Policy ($toKill.Count -eq $(if ($includeOutputs) { 2 } else { 0 })) 'release selected installed/unknown/sibling path'
        $PreserveRuntime = $true
        $script:policyStopped = @()
        $rejected = $false
        try { Invoke-Expression $releaseGuard } catch { $rejected = $_.Exception.Message.Contains('-PreserveRuntime refuses') }
        Assert-Policy ($rejected -eq $includeOutputs -and $script:policyStopped.Count -eq 0) 'release preserve decision used unfiltered candidates'
        $PreserveRuntime = $false
        Invoke-Expression $releaseGuard
        Assert-Policy (($script:policyStopped -join ',') -eq $(if ($includeOutputs) { '10,11' } else { '' })) 'release stopped unrelated process'
        $KeepRunning = $false
        $SmokeTestScript = $false
        foreach ($preserve in @($true, $false)) {
            $PreserveRuntime = $preserve
            $script:policyStopped = @()
            $rejected = $false
            try { Invoke-Expression $portableGuard } catch { $rejected = $_.Exception.Message.Contains('-PreserveRuntime refuses') }
            Assert-Policy ($rejected -eq ($includeOutputs -and $preserve)) 'portable preserve decision selected unrelated process'
            $expectedStops = if ($includeOutputs -and -not $preserve) { '10,11' } else { '' }
            Assert-Policy (($script:policyStopped -join ',') -eq $expectedStops) 'portable stopped installed/unknown/sibling path'
        }
        Invoke-Expression $distSelection
        $rejected = $false
        try { Invoke-Expression $distGuard } catch { $rejected = $_.Exception.Message.Contains('mImageViewer is running') }
        Assert-Policy ($rejected -eq $includeOutputs) 'dist refusal selected installed/unknown/sibling path'
    }

    # The shell query uses the same helper; evaluate its actual PowerShell text
    # with the process mock, never start PowerShell against the real processes.
    $queryStart = $shell.IndexOf('    $ErrorActionPreference = "Stop"')
    $queryEnd = $shell.IndexOf("`n')", $queryStart)
    Assert-Policy ($queryStart -ge 0 -and $queryEnd -gt $queryStart) 'shell path query missing'
    Push-Location $repoRoot
    try { $queryResult = @(Invoke-Expression $shell.Substring($queryStart, $queryEnd - $queryStart)) }
    finally { Pop-Location }
    Assert-Policy (($queryResult -join ',') -eq '10,11') 'shell query selected unrelated process'
    Assert-Policy (-not $shell.Contains('taskkill //IM "$name" //F') -or $shell.Contains('targets=("mimageviewer-susie32.exe"')) 'shell launcher/MCP name-only taskkill returned'

    # Run only the shell preflight/process block with command stubs. This
    # verifies refusal happens before its first mocked process query/stop/build.
    if (-not $PowerShellOnly) {
    $bash = Join-Path $env:ProgramFiles 'Git\bin\bash.exe'
    Assert-Policy (Test-Path -LiteralPath $bash -PathType Leaf) 'Git Bash missing for shell policy regression'
    $oldPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { $bashVersion = @(& $bash --version 2>&1); $bashExit = $LASTEXITCODE }
    finally { $ErrorActionPreference = $oldPreference }
    Assert-Policy ($bashExit -eq 0) "Git Bash could not start (exit $bashExit): $($bashVersion -join ' ')"
    & $bash -n (Join-Path $PSScriptRoot 'build-release.sh')
    Assert-Policy ($LASTEXITCODE -eq 0) 'build-release.sh Bash syntax check failed'
    $blockEnd = $shell.IndexOf('# T56 (Codex R-REL-002)')
    Assert-Policy ($blockEnd -gt $queryEnd) 'shell policy boundary missing'
    $mockShell = @'
function powershell.exe { printf 'query\n' >> "$MIV_POLICY_LOG"; printf '10\n'; }
function taskkill { printf 'stop\n' >> "$MIV_POLICY_LOG"; return 0; }
function sleep { :; }
function cargo { printf 'build:%s\n' "$*" >> "$MIV_POLICY_LOG"; }
'@
    $shellPath = Join-Path $testRoot 'shell-policy.sh'
    $shellText = $mockShell + "`n" + $shell.Substring(0, $blockEnd) + "`ncargo `"`$@`"`n"
    [IO.File]::WriteAllText($shellPath, $shellText.Replace("`r`n", "`n"), (New-Object Text.UTF8Encoding($false)))
    $env:MIV_POLICY_LOG = Join-Path $testRoot 'shell.log'
    foreach ($option in @('--target-dir', '--target', '--profile', '--out-dir', '--artifact-dir', '--config', '--message-format', '--unknown')) {
        foreach ($arguments in @(@($option, 'unexpected-output'), @($option + '=unexpected-output'))) {
            $oldPreference = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            try { & $bash $shellPath.Replace('\', '/') @arguments 2>$null | Out-Null; $shellExit = $LASTEXITCODE }
            finally { $ErrorActionPreference = $oldPreference }
            Assert-Policy ($shellExit -ne 0 -and -not (Test-Path $env:MIV_POLICY_LOG)) "shell override reached query/stop/build: $arguments"
        }
    }
    foreach ($arguments in @(@('--features'), @('-j'), @('--jobs=2'), @('-j2'), @('--jobs', '--config'))) {
        $oldPreference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try { & $bash $shellPath.Replace('\', '/') @arguments 2>$null | Out-Null; $shellExit = $LASTEXITCODE }
        finally { $ErrorActionPreference = $oldPreference }
        Assert-Policy ($shellExit -ne 0 -and -not (Test-Path $env:MIV_POLICY_LOG)) "shell invalid form reached query/stop/build: $arguments"
    }
    & $bash $shellPath.Replace('\', '/') --features example | Out-Null
    Assert-Policy ($LASTEXITCODE -eq 0) 'shell rejected ordinary Cargo arguments'
    $shellLog = Get-Content -LiteralPath $env:MIV_POLICY_LOG -Raw
    Assert-Policy ($shellLog.Contains('query') -and $shellLog.Contains('stop') -and $shellLog.Contains('build:--features example')) 'shell command stubs did not exercise valid path'
    foreach ($arguments in @(
        @('--features=example'), @('--all-features'), @('--no-default-features'), @('--locked'),
        @('--frozen'), @('--offline'), @('-v'), @('--verbose'), @('-j', '2'), @('--jobs', '2')
    )) {
        & $bash $shellPath.Replace('\', '/') @arguments | Out-Null
        Assert-Policy ($LASTEXITCODE -eq 0) "shell rejected allowed form: $arguments"
    }

    $artifactFunctionStart = $shell.IndexOf('invoke_release_cargo()')
    $artifactFunctionEnd = $shell.IndexOf('# An installed launcher', $artifactFunctionStart)
    Assert-Policy ($artifactFunctionStart -ge 0 -and $artifactFunctionEnd -gt $artifactFunctionStart) 'shell artifact wrapper missing'
    $artifactShell = Join-Path $testRoot 'shell-artifact.sh'
    $artifactShellText = "set -euo pipefail`nfunction cargo { cat `"`$MIV_CARGO_ARTIFACT_JSON`"; }`n" +
        $shell.Substring($artifactFunctionStart, $artifactFunctionEnd - $artifactFunctionStart) +
        "`ninvoke_release_cargo mimageviewer-mcp.exe build --release -p mimageviewer-mcp --bin mimageviewer-mcp`n" +
        "printf 'after-artifact' > `"`$MIV_POLICY_LOG`"`n"
    [IO.File]::WriteAllText($artifactShell, $artifactShellText.Replace("`r`n", "`n"), (New-Object Text.UTF8Encoding($false)))
    $env:MIV_CARGO_ARTIFACT_JSON = Join-Path $testRoot 'artifact.json'
    New-Item -ItemType Directory -Path (Join-Path $unicodeRoot 'scripts') -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'check-cargo-build-output.py') -Destination (Join-Path $unicodeRoot 'scripts\check-cargo-build-output.py')
    Push-Location $unicodeRoot
    try {
        foreach ($fresh in @($true, $false)) {
            foreach ($correctPath in @($true, $false)) {
                Remove-Item -LiteralPath $env:MIV_POLICY_LOG -ErrorAction SilentlyContinue
                $artifactPath = if ($correctPath) { $unicodeExecutable } else { Join-Path $testRoot 'mimageviewer-mcp.exe' }
                $artifactJson = @{ reason = 'compiler-artifact'; executable = $artifactPath; fresh = $fresh } | ConvertTo-Json -Compress
                [IO.File]::WriteAllText($env:MIV_CARGO_ARTIFACT_JSON, $artifactJson + "`n", (New-Object Text.UTF8Encoding($false)))
                $oldPreference = $ErrorActionPreference
                $ErrorActionPreference = 'Continue'
                try { & $bash $artifactShell.Replace('\', '/') 2>$null | Out-Null; $shellExit = $LASTEXITCODE }
                finally { $ErrorActionPreference = $oldPreference }
                Assert-Policy (($shellExit -eq 0) -eq $correctPath) "shell artifact result wrong: correct=$correctPath fresh=$fresh"
                Assert-Policy ((Test-Path $env:MIV_POLICY_LOG) -eq $correctPath) 'shell artifact mismatch reached next stage'
            }
        }
    } finally { Pop-Location }
    }

    # MCP participates in the same diagnostic manifest hash contract as the
    # existing companions; importing these functions never runs a product build.
    foreach ($functionName in @('Write-SmokeBuildManifest', 'Assert-SmokeBuildManifest')) {
        $definition = @($portableAst.FindAll({
            param($node)
            $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $functionName
        }, $true))
        Assert-Policy ($definition.Count -eq 1) "manifest function missing: $functionName"
        Invoke-Expression $definition[0].Extent.Text
    }
    $fixturePaths = @('core', 'remote', 'epub', 'mcp') | ForEach-Object {
        $fixture = Join-Path $testRoot ($_.ToString() + '.exe')
        Set-Content -LiteralPath $fixture -Value $_ -Encoding ASCII
        $fixture
    }
    $manifestPath = Join-Path $testRoot 'manifest.json'
    Write-SmokeBuildManifest $manifestPath 'fixture-source' @fixturePaths
    Assert-SmokeBuildManifest $manifestPath 'fixture-source' @fixturePaths
    Set-Content -LiteralPath $fixturePaths[3] -Value 'changed-mcp' -Encoding ASCII
    $rejected = $false
    try { Assert-SmokeBuildManifest $manifestPath 'fixture-source' @fixturePaths }
    catch { $rejected = $_.Exception.Message.Contains('hash does not match') }
    Assert-Policy $rejected 'changed MCP was accepted by portable manifest'
    $consumer = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Support.ps1') -Raw -Encoding UTF8
    Assert-Policy ($consumer.Contains("(Join-Path `$smoke 'mimageviewer-mcp.exe')")) 'release checks omitted MCP manifest hash'
} finally {
    foreach ($name in $environmentNames) { [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process') }
    $cleanup = [IO.Path]::GetFullPath($testRoot).TrimEnd('\')
    Assert-Policy ($cleanup.StartsWith($tempRoot + '\', [StringComparison]::OrdinalIgnoreCase)) 'cleanup escaped temp'
    if (Test-Path -LiteralPath $cleanup) { Remove-Item -LiteralPath $cleanup -Recurse -Force }
}
if ($PowerShellOnly) { Write-Host '[build-process-policy-test] PowerShell-only PASS; Bash execution was explicitly excluded' }
else { Write-Host '[build-process-policy-test] PASS' }
