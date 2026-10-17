# Trial-only bundle override regression. No Cargo, application or real profile.
$ErrorActionPreference = 'Stop'
$tokens = $null
$errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $PSScriptRoot 'build-dev.ps1'), [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw 'build-dev parse failed' }
$function = $ast.Find({ param($node)
    $node -is [Management.Automation.Language.FunctionDefinitionAst] -and
    $node.Name -eq 'Copy-EffetuneBundleIfChanged'
}, $true)
Invoke-Expression $function.Extent.Text
$testRoot = Join-Path (Split-Path $PSScriptRoot -Parent) ('target/lyrics-trial/staging-test-' + [guid]::NewGuid().ToString('N'))
$repoRoot = Join-Path $testRoot 'repo'
$outputDir = Join-Path $testRoot 'dev-runtime'
$official = Join-Path $repoRoot 'vendor/effetune-mixwright/EffeTune Mixwright.vst3'
$trial = Join-Path $testRoot 'trial/EffeTune Mixwright.vst3'
$destination = Join-Path $outputDir 'effetune/EffeTune Mixwright.vst3'
New-Item -ItemType Directory -Path $official,$trial -Force | Out-Null
Set-Content -LiteralPath (Join-Path $official 'official.txt') -Value 'official' -Encoding Ascii
Set-Content -LiteralPath (Join-Path $trial 'trial.txt') -Value 'trial' -Encoding Ascii
$EffetuneBundle = $null
Copy-EffetuneBundleIfChanged
if (-not (Test-Path -LiteralPath (Join-Path $destination 'official.txt'))) { throw 'default source missing' }
$EffetuneBundle = $trial
Copy-EffetuneBundleIfChanged
if (-not (Test-Path -LiteralPath (Join-Path $destination 'trial.txt'))) { throw 'trial source missing' }
if (Test-Path -LiteralPath (Join-Path $destination 'official.txt')) { throw 'mixed old bundle' }
if (-not (Test-Path -LiteralPath (Join-Path $official 'official.txt'))) { throw 'official source changed' }
$hash = (Get-FileHash -LiteralPath (Join-Path $destination 'trial.txt')).Hash
Copy-EffetuneBundleIfChanged
if ($hash -ne (Get-FileHash -LiteralPath (Join-Path $destination 'trial.txt')).Hash) { throw 'repeated staging changed bytes' }
$nested = Join-Path $destination 'nested.vst3'
New-Item -ItemType Directory -Path $nested | Out-Null
Set-Content -LiteralPath (Join-Path $nested 'source.txt') -Value 'preserved' -Encoding Ascii
foreach ($overlap in @($nested, (Split-Path $destination -Parent), $destination)) {
    $EffetuneBundle = $overlap
    $rejected = $false
    try { Copy-EffetuneBundleIfChanged } catch {
        if (-not $_.Exception.Message.Contains('overlap')) { throw }
        $rejected = $true
    }
    if (-not $rejected) { throw 'overlapping source was accepted' }
    if (-not (Test-Path -LiteralPath (Join-Path $nested 'source.txt'))) { throw 'nested source was removed' }
}
Write-Host '[lyrics-trial-staging-test] PASS (default, override, stale removal, source preserved, unchanged, overlap rejection)'
