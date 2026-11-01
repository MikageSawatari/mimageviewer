# Trial-only bundle override regression. No Cargo, application or real profile.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'miv-sound-candidate.ps1')
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
$outputDir = Join-Path $repoRoot 'target/dev-runtime'
$officialRoot = Join-Path $repoRoot $MivSound.SOURCE_ROOT
$official = Join-Path $officialRoot $MivSound.BUNDLE_NAME
$trial = Join-Path (Join-Path $testRoot 'trial') $MivSound.BUNDLE_NAME
$destinationRoot = Join-Path $outputDir 'effetune'
$destination = Join-Path $destinationRoot $MivSound.BUNDLE_NAME
New-Item -ItemType Directory -Path $official,$trial -Force | Out-Null
function Write-FixturePe([string] $Bundle) {
    $plugin = Join-Path $Bundle ('Contents/x86_64-win/' + $MivSound.BUNDLE_NAME)
    New-Item -ItemType Directory -Path (Split-Path $plugin) -Force | Out-Null
    $bytes = New-Object byte[] 512
    $bytes[0] = 0x4d; $bytes[1] = 0x5a; $bytes[0x3c] = 0x40
    $bytes[64] = 0x50; $bytes[65] = 0x45
    [IO.File]::WriteAllBytes($plugin, $bytes)
}
Write-FixturePe $official
Write-FixturePe $trial
Set-Content -LiteralPath (Join-Path $official 'official.txt') -Value 'official' -Encoding Ascii
Set-Content -LiteralPath (Join-Path $trial 'trial.txt') -Value 'trial' -Encoding Ascii
Set-Content -LiteralPath (Join-Path $officialRoot 'VERSION') -Value $MivSound.VERSION -Encoding Ascii
$manifestPath = Join-Path $repoRoot $MivSound.Manifest
New-Item -ItemType Directory -Path (Split-Path $manifestPath) -Force | Out-Null
[IO.File]::WriteAllText($manifestPath, (Get-MivSoundManifestText (Get-MivSoundInventory $officialRoot)))
# The approved-source gate's contents/notice tests live in the distribution
# suite. This fixture checks that staging still calls that gate before copy,
# and retains the real raw-manifest verification after copy.
$script:approvalChecks = 0
function Assert-MivEffetuneSource([string] $SourceRoot, [string] $NoticesRoot) {
    if ($SourceRoot -ne $officialRoot -or $NoticesRoot -ne (Join-Path $repoRoot $MivSound.NOTICES_ROOT)) {
        throw 'default approval gate received wrong paths'
    }
    $script:approvalChecks++
}
$EffetuneBundle = $null
$MivSoundCandidate = $null
Copy-EffetuneBundleIfChanged
if (-not (Test-Path -LiteralPath (Join-Path $destination 'official.txt'))) { throw 'default source missing' }
if ($script:approvalChecks -ne 1) { throw 'default approval gate skipped' }
$unapproved = Join-Path $official 'unapproved.txt'
Set-Content -LiteralPath $unapproved -Value 'extra' -Encoding Ascii
$rejected = $false
try { Copy-EffetuneBundleIfChanged } catch {
    if (-not $_.Exception.Message.Contains('Extra unapproved')) { throw }
    $rejected = $true
}
if (-not $rejected) { throw 'raw manifest gate skipped' }
Remove-Item -LiteralPath $unapproved
# A trial replaces the entire runtime container, including old generations
# and their pointer; otherwise resolve_bundle_at would still select the old one.
Set-Content -LiteralPath (Join-Path $destinationRoot 'current') -Value 'stale' -Encoding Ascii
$EffetuneBundle = $trial
Copy-EffetuneBundleIfChanged
if (-not (Test-Path -LiteralPath (Join-Path $destination 'trial.txt'))) { throw 'trial source missing' }
if (Test-Path -LiteralPath (Join-Path $destination 'official.txt')) { throw 'mixed old bundle' }
if (Test-Path -LiteralPath (Join-Path $destinationRoot 'current')) { throw 'stale generation pointer' }
if (-not (Test-Path -LiteralPath (Join-Path $official 'official.txt'))) { throw 'official source changed' }
$hash = (Get-FileHash -LiteralPath (Join-Path $destination 'trial.txt')).Hash
Copy-EffetuneBundleIfChanged
if ($hash -ne (Get-FileHash -LiteralPath (Join-Path $destination 'trial.txt')).Hash) { throw 'repeated staging changed bytes' }
$nested = Join-Path $destination 'nested.vst3'
New-Item -ItemType Directory -Path $nested | Out-Null
Set-Content -LiteralPath (Join-Path $nested 'source.txt') -Value 'preserved' -Encoding Ascii
foreach ($overlap in @($nested, $destinationRoot, $destination, $outputDir)) {
    $EffetuneBundle = $overlap
    $rejected = $false
    try { Copy-EffetuneBundleIfChanged } catch {
        if (-not $_.Exception.Message.Contains('overlap')) { throw }
        $rejected = $true
    }
    if (-not $rejected) { throw 'overlapping source was accepted' }
    if (-not (Test-Path -LiteralPath (Join-Path $nested 'source.txt'))) { throw 'nested source was removed' }
}
$oldTrial = Join-Path $testRoot 'old/EffeTune Mixwright.vst3'
New-Item -ItemType Directory -Path $oldTrial -Force | Out-Null
$EffetuneBundle = $oldTrial
$rejected = $false
try { Copy-EffetuneBundleIfChanged } catch {
    if (-not $_.Exception.Message.Contains('old EffeTune Mixwright')) { throw }
    $rejected = $true
}
if (-not $rejected -or -not (Test-Path -LiteralPath (Join-Path $nested 'source.txt'))) { throw 'old trial was accepted or destroyed runtime' }
$EffetuneBundle = $trial
$MivSoundCandidate = 'candidate'
$rejected = $false
try { Copy-EffetuneBundleIfChanged } catch {
    if (-not $_.Exception.Message.Contains('mutually exclusive')) { throw }
    $rejected = $true
}
if (-not $rejected -or -not (Test-Path -LiteralPath (Join-Path $nested 'source.txt'))) { throw 'mixed override accepted or destroyed runtime' }
# Exercise the real candidate reader/stager with an explicitly synthetic,
# disposable fixture. This is not a distributable or approved candidate.
$artifact = Join-Path $testRoot 'candidate-artifact'
New-Item -ItemType Directory -Path $artifact -Force | Out-Null
Copy-Item -LiteralPath $official -Destination $artifact -Recurse
Set-Content -LiteralPath (Join-Path $artifact 'VERSION') -Value $MivSound.VERSION -Encoding Ascii
foreach ($notice in $MivSoundNotices) {
    $path = Join-Path $artifact ($MivSound.BUNDLE_NAME + '/' + $notice)
    New-Item -ItemType Directory -Path (Split-Path $path) -Force | Out-Null
    Set-Content -LiteralPath $path -Value 'synthetic test notice' -Encoding Ascii
}
$manifest = Get-MivSoundManifestText (Get-MivSoundInventory $artifact)
$MivSoundCandidate = Get-MivSoundTextHash $manifest
$candidateRoot = Join-Path $repoRoot ('target/miv-sound-candidates/' + $MivSoundCandidate)
New-Item -ItemType Directory -Path $candidateRoot -Force | Out-Null
Move-Item -LiteralPath $artifact -Destination (Join-Path $candidateRoot 'artifact')
$record = @{schema=1; id=$MivSoundCandidate; version=$MivSound.VERSION; manifest=$manifest; inspection_only=$true}
[IO.File]::WriteAllText((Join-Path $candidateRoot 'candidate.json'), ($record | ConvertTo-Json))
$EffetuneBundle = $null
Copy-EffetuneBundleIfChanged
if (-not (Test-Path -LiteralPath (Join-Path $destination 'official.txt'))) { throw 'candidate source missing' }
if (Test-Path -LiteralPath (Join-Path $destination 'trial.txt')) { throw 'mixed candidate/trial bundle' }
$MivSoundCandidate = $null
Copy-EffetuneBundleIfChanged
if (Test-Path -LiteralPath (Join-Path $destination $MivSoundNotices[0])) { throw 'mixed candidate/default bundle' }
Write-Host '[lyrics-trial-staging-test] PASS (approved default, raw manifest gate, trial, candidate, stale removal, source preserved, unchanged, overlap, legacy and mixed-argument rejection)'
