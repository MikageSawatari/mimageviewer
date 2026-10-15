# Behavioral regression suite using real Git/nextest and a disposable tiny crate.
# No commits, application launches, normal profile data, or tool installations.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'test-tools-common.ps1')
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$fixture = Join-Path $repo ('target/test-tools-regression-' + [guid]::NewGuid().ToString('N'))
[void] [IO.Directory]::CreateDirectory((Join-Path $fixture 'src'))
[void] [IO.Directory]::CreateDirectory((Join-Path $fixture 'tests'))
[void] [IO.Directory]::CreateDirectory((Join-Path $fixture 'helper/src'))
$utf8 = New-Object Text.UTF8Encoding($false)
function Write-Fixture { param([string] $Path, [string] $Text); [IO.File]::WriteAllText((Join-Path $fixture $Path), $Text, $utf8) }
function Assert-That { param([bool] $Condition, [string] $Message); if (-not $Condition) { throw "[test-test-tools] $Message" } }
function Run-Script {
    param([string] $Name, [string[]] $Arguments)
    $cmdArgs = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $PSScriptRoot $Name), '-RepoRoot', $fixture) + $Arguments
    $result = Invoke-MivTool 'powershell.exe' $cmdArgs $repo
    Write-Host "[test-test-tools] $Name $($Arguments -join ' '): exit $($result.ExitCode)"
    return $result
}
function Count-Runs { return @(Get-ChildItem (Join-Path $fixture 'target/test-lib-full/runs') -Directory -ErrorAction SilentlyContinue).Count }
Assert-That ((Get-MivTextHash 'abc') -eq 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad') 'SHA-256 known vector mismatch'
$caseFeatures = @(Get-MivFeatureArguments @('extra', 'EXTRA', 'extra'))
Assert-That (($caseFeatures -join ' ') -eq '--features EXTRA,extra') 'case-sensitive Cargo features were merged'
Write-Fixture 'Cargo.toml' @'
[package]
name = "mimageviewer"
version = "0.0.0"
edition = "2021"
[workspace]
members = ["helper"]
[features]
extra = []
'@
Write-Fixture 'src/lib.rs' @'
mod foo;
#[test] fn root_ok() { assert!(std::env::var("TEST_EXTERNAL_FAILURE").is_err()); assert_eq!(2 + 2, 4); }
#[test] #[ignore = "manual fixture sentinel"] fn manual() { panic!("must stay ignored"); }
#[cfg(feature = "extra")] #[test] fn extra() { assert!(true); }
'@
$foo = @'
#[test] fn alpha() { assert_eq!(1 + 1, 2); }
#[test] fn beta() { assert_eq!(3 + 3, 6); }
'@
Write-Fixture 'src/foo.rs' $foo
Write-Fixture 'tests/sample.rs' '#[test] fn integration() { assert_eq!(4 + 4, 8); }'
Write-Fixture 'helper/Cargo.toml' "[package]`nname = `"miv-test-helper`"`nversion = `"0.0.0`"`nedition = `"2021`"`n"
Write-Fixture 'helper/src/lib.rs' '#[test] fn helper() { assert_eq!(5 + 5, 10); }'
Write-Fixture '.gitignore' "target/`nvendor/`n"
[void] (Invoke-MivGit $fixture @('init', '-q'))
[void] (Invoke-MivGit $fixture @('add', '--', '.gitignore', 'Cargo.toml', 'src', 'tests', 'helper'))
$lock = Invoke-MivTool 'cargo' @('generate-lockfile', '--offline') $fixture
Assert-That ($lock.ExitCode -eq 0) 'cannot prepare fixture lockfile'
$index = Join-Path $fixture '.git/index'
$indexBefore = (Get-FileHash -LiteralPath $index).Hash
$id1 = Get-MivSourceId $fixture
Write-Fixture 'src/foo.rs' ($foo + "`n// unstaged change`n")
Assert-That ((Get-MivSourceId $fixture) -ne $id1) 'unstaged contents were not detected'
[void] (Invoke-MivGit $fixture @('add', '--', 'src/foo.rs'))
$stagedId = Get-MivSourceId $fixture
Write-Fixture 'src/foo.rs' $foo
Assert-That ((Get-MivSourceId $fixture) -ne $stagedId) 'worktree contents behind a staged edit were not detected'
Assert-That ((Get-MivSourceId $fixture) -eq $id1) 'fingerprint depends on staging instead of tested bytes'
[void] (Invoke-MivGit $fixture @('add', '--', 'src/foo.rs'))
$indexBefore = (Get-FileHash -LiteralPath $index).Hash
$unicodeName = 'src/' + [char]0x8A66 + [char]0x9A13 + '.rs'
Write-Fixture $unicodeName '// untracked Unicode path'
Assert-That ((Get-MivSourceId $fixture) -ne $id1) 'untracked Unicode input was not detected'
Remove-Item -LiteralPath (Join-Path $fixture $unicodeName)
[void] [IO.Directory]::CreateDirectory((Join-Path $fixture 'vendor/raw-samples'))
Write-Fixture 'vendor/raw-samples/input.raw' 'fixture-v1'
$rawId = Get-MivSourceId $fixture
Write-Fixture 'vendor/raw-samples/input.raw' 'fixture-v2'
Assert-That ((Get-MivSourceId $fixture) -ne $rawId) 'ignored real fixture was not detected'
$beforeDelete = Get-MivSourceId $fixture
Remove-Item -LiteralPath (Join-Path $fixture 'src/foo.rs')
Assert-That ((Get-MivSourceId $fixture) -ne $beforeDelete) 'tracked deletion was not detected'
Write-Fixture 'src/foo.rs' $foo
[void] [IO.Directory]::CreateDirectory((Join-Path $fixture 'target'))
Write-Fixture 'target/generated.txt' 'must not invalidate'
$beforeOutput = Get-MivSourceId $fixture
Write-Fixture 'target/generated.txt' 'still must not invalidate'
Assert-That ((Get-MivSourceId $fixture) -eq $beforeOutput) 'target output invalidated source'
[void] [IO.Directory]::CreateDirectory((Join-Path $fixture 'tests/snapshots'))
[void] [IO.Directory]::CreateDirectory((Join-Path $fixture 'tests/target-fixture'))
Write-Fixture 'tests/snapshots/example.new.png' 'generated comparison output'
Assert-That ((Get-MivSourceId $fixture) -eq $beforeOutput) 'generated snapshot output invalidated source'
Write-Fixture 'tests/target-fixture/source.txt' 'real fixture, not a target directory'
Assert-That ((Get-MivSourceId $fixture) -ne $beforeOutput) 'a target-prefixed source fixture was excluded'
Assert-That ((Get-FileHash -LiteralPath $index).Hash -eq $indexBefore) 'source fingerprint changed the index'

$run1 = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
Assert-That ($run1.ExitCode -eq 0 -and $run1.StdOut.Contains('MISS')) "initial miss failed: $($run1.StdOut) $($run1.StdErr)"
$count1 = Count-Runs
$run2 = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
Assert-That ($run2.ExitCode -eq 0 -and $run2.StdOut.Contains('HIT') -and (Count-Runs) -eq $count1) "hit ran tests: $($run2.StdOut)"
Write-Fixture 'src/foo.rs' ($foo + "`n// source invalidates receipt`n")
$run3 = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
Assert-That ($run3.ExitCode -eq 0 -and $run3.StdOut.Contains('MISS') -and (Count-Runs) -eq ($count1 + 1)) "dirty source did not rerun: $($run3.StdOut)"
$forced = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3', '-Force')
Assert-That ($forced.ExitCode -eq 0 -and $forced.StdOut.Contains('MISS')) 'Force did not run tests'
$externalBefore = [Environment]::GetEnvironmentVariable('TEST_EXTERNAL_FAILURE')
try {
    # Model an external condition, deliberately outside the documented hash.
    $env:TEST_EXTERNAL_FAILURE = '1'
    $forcedFailure = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3', '-Force')
    Assert-That ($forcedFailure.ExitCode -ne 0) 'forced failure was accepted'
    $afterForcedFailure = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
    Assert-That ($afterForcedFailure.ExitCode -ne 0 -and $afterForcedFailure.StdOut.Contains('MISS')) 'older success survived a failed forced rerun'
} finally { [Environment]::SetEnvironmentVariable('TEST_EXTERNAL_FAILURE', $externalBefore) }
$recover = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
Assert-That ($recover.ExitCode -eq 0 -and $recover.StdOut.Contains('MISS')) 'normal success after an external failure was not re-recorded'
$features = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '4', '-Features', 'extra')
Assert-That ($features.ExitCode -eq 0 -and $features.StdOut.Contains('4 passed')) "feature selection failed: $($features.StdOut)"
$short = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '99')
Assert-That ($short.ExitCode -ne 0) 'minimum executed-count guard accepted a short suite'
$receipts = @(Get-ChildItem (Join-Path $fixture 'target/test-lib-full') -File -Filter '*.json')
$receiptCount = $receipts.Count
Write-Fixture 'src/foo.rs' '#[test] fn intentional_failure() { panic!("fixture failure"); }'
$failed = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '1')
Assert-That ($failed.ExitCode -ne 0) 'failed suite returned success'
$failedAgain = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '1')
Assert-That ($failedAgain.ExitCode -ne 0 -and $failedAgain.StdOut.Contains('MISS')) 'failed result was reused'
Assert-That (@(Get-ChildItem (Join-Path $fixture 'target/test-lib-full') -File -Filter '*.json').Count -eq $receiptCount) 'failure wrote a success receipt'
$libOriginal = [IO.File]::ReadAllText((Join-Path $fixture 'src/lib.rs'))
Write-Fixture 'src/lib.rs' '#[test] #[ignore] fn only_ignored() { panic!("must not run"); }'
$zeroFull = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '1')
Assert-That ($zeroFull.ExitCode -eq 4) 'zero-test full suite was accepted'
Write-Fixture 'src/lib.rs' $libOriginal
Write-Fixture 'src/foo.rs' $foo

$focused = Run-Script 'test-focused.ps1' @('-Path', 'src/foo.rs')
Assert-That ($focused.ExitCode -eq 0 -and $focused.StdOut.Contains('PASS: 2 tests')) "module filter failed: $($focused.StdOut)"
$integration = Run-Script 'test-focused.ps1' @('-Path', 'tests/sample.rs')
Assert-That ($integration.ExitCode -eq 0 -and $integration.StdOut.Contains('--test sample') -and $integration.StdOut.Contains('PASS: 1 tests')) 'integration target selection failed'
$crate = Run-Script 'test-focused.ps1' @('-Path', 'helper/src/lib.rs')
Assert-That ($crate.ExitCode -eq 0 -and $crate.StdOut.Contains('-p miv-test-helper') -and $crate.StdOut.Contains('PASS: 1 tests')) "workspace crate selection failed: $($crate.StdOut)"
$zero = Run-Script 'test-focused.ps1' @('-Path', 'src/missing_module.rs')
Assert-That ($zero.ExitCode -eq 4) "zero-match did not return nextest exit 4: $($zero.StdOut) $($zero.StdErr)"
$broad = Run-Script 'test-focused.ps1' @('-Path', 'src/app/tests.rs')
Assert-That ($broad.ExitCode -eq 2 -and $broad.StdOut.Contains('test-lib-full.ps1')) 'cross-cutting tests did not require a deliberate full/filter choice'
$docs = Run-Script 'test-focused.ps1' @('-Path', 'docs/example.md')
Assert-That ($docs.ExitCode -eq 4) 'no selection was reported as test success'
$escape = Run-Script 'test-focused.ps1' @('-Path', '../outside.rs')
Assert-That ($escape.ExitCode -ne 0) 'outside path was accepted'
Assert-That ((Get-FileHash -LiteralPath $index).Hash -eq $indexBefore) 'test tools changed the index'
Write-Host "[test-test-tools] PASS; disposable fixtures/logs retained at $fixture"
