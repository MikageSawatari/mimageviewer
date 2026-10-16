# Behavioral regression suite using real Git/nextest and a disposable tiny crate.
# No commits, application launches, normal profile data, or tool installations.
[CmdletBinding()]
param([switch] $Benchmark)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'test-tools-common.ps1')
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$fixture = Join-Path $repo ('target/test-tools-regression-' + [guid]::NewGuid().ToString('N'))
[void] [IO.Directory]::CreateDirectory((Join-Path $fixture 'src'))
[void] [IO.Directory]::CreateDirectory((Join-Path $fixture 'tests'))
[void] [IO.Directory]::CreateDirectory((Join-Path $fixture 'helper/src'))
[void] [IO.Directory]::CreateDirectory((Join-Path $fixture '.cargo'))
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
# Every TOML spelling of the Cargo env table must yield its names, or refuse reuse.
$envProbe = Join-Path $fixture 'env-probe'
[void] [IO.Directory]::CreateDirectory((Join-Path $envProbe '.cargo'))
$envProbeConfig = Join-Path $envProbe '.cargo/config.toml'
foreach ($spelling in @(
    "[env]`nPROBE_NAME = `"1`"", "[`"env`"]`nPROBE_NAME = `"1`"", "[ 'env' ]`n'PROBE_NAME' = `"1`"",
    "[env.PROBE_NAME]`nvalue = `"1`"", "[`"env`".PROBE_NAME]`nvalue = `"1`"", "[ env . `"PROBE_NAME`" ]`nvalue = `"1`"")) {
    [IO.File]::WriteAllText($envProbeConfig, $spelling + "`n", $utf8)
    $probeNames = @(Get-MivCargoEnvironmentNames $envProbe)
    Assert-That ($probeNames -contains 'PROBE_NAME') "Cargo env spelling was not collected: $spelling"
}
$escapedEnv = '"' + [char]92 + 'u0065nv"'
foreach ($unsupported in @("env = { PROBE_NAME = `"1`" }", "env.PROBE_NAME = `"1`"", "`"env`".PROBE_NAME = `"1`"",
    "[$escapedEnv]`nPROBE_NAME = `"1`"", "[$escapedEnv.PROBE_NAME]`nvalue = `"1`"", "$escapedEnv.PROBE_NAME = `"1`"")) {
    [IO.File]::WriteAllText($envProbeConfig, $unsupported + "`n", $utf8)
    $refused = $false
    try { [void] @(Get-MivCargoEnvironmentNames $envProbe) } catch { $refused = $true }
    Assert-That $refused "unsupported Cargo env syntax was silently ignored: $unsupported"
}
[IO.Directory]::Delete($envProbe, $true)
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
Write-Fixture '.cargo/config.toml' @'
[env]
FFMPEG_DIR = { value = "unused-fixture-directory" }
LIBSQLITE3_FLAGS = "SQLITE_MAX_ATTACHED=20"
"TEST_CONFIG_ENV" = "default"
[env.TEST_CONFIG_TABLE_ENV]
value = "default"
'@
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
# A cloned reference repository under an ignored fixture dir is listed by git as one
# directory entry; its files (not its .git) must be fingerprinted.
$nestedRoot = Join-Path $fixture 'vendor/nested-ref'
[void] [IO.Directory]::CreateDirectory((Join-Path $nestedRoot 'sub'))
[void] (Invoke-MivGit $nestedRoot @('init', '-q'))
Write-Fixture 'vendor/nested-ref/sub/data.txt' 'nested-v1'
$nestedId = Get-MivSourceId $fixture
Write-Fixture 'vendor/nested-ref/sub/data.txt' 'nested-v2'
Assert-That ((Get-MivSourceId $fixture) -ne $nestedId) 'file inside a nested repository was not detected'
$nestedId = Get-MivSourceId $fixture
Write-Fixture 'vendor/nested-ref/.git/description' 'git metadata only'
Assert-That ((Get-MivSourceId $fixture) -eq $nestedId) 'nested .git metadata changed the source identity'
[void] [IO.Directory]::CreateDirectory((Join-Path $nestedRoot 'sub/__pycache__'))
Write-Fixture 'vendor/nested-ref/sub/__pycache__/mod.pyc' 'generated'
Assert-That ((Get-MivSourceId $fixture) -eq $nestedId) 'generated output inside a nested repository changed the source identity'
$linkCreated = $false
try {
    [void] (New-Item -ItemType SymbolicLink -Path (Join-Path $nestedRoot 'sub/link.txt') -Target (Join-Path $nestedRoot 'sub/data.txt') -ErrorAction Stop)
    $linkCreated = $true
} catch { Write-Host "[test-test-tools] file symlink not creatable here; skipped the nested file-link check" }
if ($linkCreated) {
    $linkRefused = $false
    try { [void] (Get-MivSourceId $fixture) } catch { $linkRefused = $true }
    Assert-That $linkRefused 'a file symlink inside a nested repository was followed'
    Remove-Item -LiteralPath (Join-Path $nestedRoot 'sub/link.txt') -Force
}
Assert-That ([IO.Path]::GetFullPath($nestedRoot).StartsWith($fixture + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) 'nested cleanup escaped disposable fixture'
Remove-Item -LiteralPath $nestedRoot -Recurse -Force
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

# Isolate stat-cache transitions from the source/routing fixture above.
$statRoot = Join-Path $fixture 'target/stat-cache-probe'
[void] [IO.Directory]::CreateDirectory($statRoot)
[void] (Invoke-MivGit $statRoot @('init', '-q'))
[IO.File]::WriteAllText((Join-Path $statRoot '.gitignore'), "target/`n", $utf8)
$statFile = Join-Path $statRoot ('input-' + [char]0x8A66 + '.txt')
$statCachePath = Join-Path $statRoot 'target/test-lib-full/source-hashes-v1.tsv'
$statRelative = [IO.Path]::GetFileName($statFile)
$oldTime = [DateTime]::UtcNow.AddDays(-1)
[IO.File]::WriteAllText($statFile, 'first', $utf8)
[IO.File]::SetLastWriteTimeUtc($statFile, $oldTime)
[IO.File]::SetCreationTimeUtc($statFile, $oldTime.AddDays(-1))
$statId = Get-MivSourceId $statRoot -NoCache
Assert-That ((Get-MivSourceId $statRoot) -eq $statId) 'cold cache changed source identity'
Assert-That ((Get-MivSourceId $statRoot) -eq $statId) 'warm cache changed source identity'
$locked = [IO.File]::Open($statFile, 'Open', 'ReadWrite', 'None')
try {
    Assert-That ((Get-MivSourceId $statRoot) -eq $statId) 'warm hit read unchanged file contents'
    $forceRead = $false
    try { [void] (Get-MivSourceId $statRoot -ForceHash) } catch { $forceRead = $true }
    Assert-That $forceRead 'ForceHash reused cached contents'
} finally { $locked.Dispose() }
[IO.File]::WriteAllText($statFile, 'changed-size', $utf8)
$sizeId = Get-MivSourceId $statRoot
Assert-That ($sizeId -ne $statId -and $sizeId -eq (Get-MivSourceId $statRoot -NoCache)) 'content/size change was not detected'
# Restore a timestamp older than the observation; size must still invalidate it.
[IO.File]::SetLastWriteTimeUtc($statFile, $oldTime)
[void] (Get-MivSourceId $statRoot)
[IO.File]::WriteAllText($statFile, 'another-size-change', $utf8)
[IO.File]::SetLastWriteTimeUtc($statFile, $oldTime)
$restoredId = Get-MivSourceId $statRoot
Assert-That ($restoredId -ne $sizeId -and $restoredId -eq (Get-MivSourceId $statRoot -NoCache)) 'size change with restored old mtime was not detected'
# Poison only the cached digest to prove a creation-time-only change rehashes.
$statEntries = Read-MivSourceHashCache $statCachePath $statRoot
$statEntries[$statRelative][3] = 'A' * 64
Write-MivSourceHashCache $statCachePath $statRoot $statEntries
[IO.File]::SetCreationTimeUtc($statFile, $oldTime.AddDays(-2))
Assert-That ((Get-MivSourceId $statRoot) -eq $restoredId) 'creation-time-only change reused a stale hash'
Assert-That ((Read-MivSourceHashCache $statCachePath $statRoot)[$statRelative][3] -ne ('A' * 64)) 'creation time did not refresh the cache entry'
# An entry recorded at mtime is racy, even if all three stat values still match.
[IO.File]::SetLastWriteTimeUtc($statFile, [DateTime]::UtcNow)
[void] (Get-MivSourceId $statRoot)
[IO.File]::WriteAllText($statFile, 'same-length-change!', $utf8)
$racyEntries = Read-MivSourceHashCache $statCachePath $statRoot
Assert-That (([IO.FileInfo] $statFile).Length -eq $racyEntries[$statRelative][0]) 'racy fixture changed file size'
[IO.File]::SetLastWriteTimeUtc($statFile, [DateTime]::new($racyEntries[$statRelative][1], [DateTimeKind]::Utc))
$racyId = Get-MivSourceId $statRoot
Assert-That ($racyId -ne $restoredId -and $racyId -eq (Get-MivSourceId $statRoot -NoCache)) 'racy same-size update was not rehashed'
# Re-reading a racy entry must not promote it to a trusted old observation.
$racyEntries = Read-MivSourceHashCache $statCachePath $statRoot
$racyEntries[$statRelative][3] = 'B' * 64
Write-MivSourceHashCache $statCachePath $statRoot $racyEntries
Assert-That ((Get-MivSourceId $statRoot) -eq $racyId) 'racy entry became trusted after reuse'
# Force also repairs deliberate same-stat edits outside the detection contract.
[IO.File]::SetLastWriteTimeUtc($statFile, $oldTime)
[void] (Get-MivSourceId $statRoot)
$creationBefore = [IO.File]::GetCreationTimeUtc($statFile)
[IO.File]::WriteAllText($statFile, 'same-length-edited!', $utf8)
[IO.File]::SetLastWriteTimeUtc($statFile, $oldTime)
[IO.File]::SetCreationTimeUtc($statFile, $creationBefore)
Assert-That ((Get-MivSourceId $statRoot) -eq $racyId) 'same-stat fixture did not exercise a cache hit'
$forceId = Get-MivSourceId $statRoot -ForceHash
Assert-That ($forceId -ne $racyId -and $forceId -eq (Get-MivSourceId $statRoot -NoCache)) 'ForceHash did not repair a same-stat edit'
foreach ($broken in @('corrupt', "miv-stat-cache-v2 $(Get-MivTextHash $statRoot)`ntruncated`n")) {
    [IO.File]::WriteAllText($statCachePath, $broken, $utf8)
    $cacheWarnings = @()
    $brokenId = Get-MivSourceId $statRoot -WarningVariable cacheWarnings -WarningAction SilentlyContinue
    Assert-That ($brokenId -eq $forceId -and $cacheWarnings.Count -gt 0) 'broken cache did not warn and hash correctly'
}
# A well-formed row whose digest differs by one hex digit must not be trusted.
[void] (Get-MivSourceId $statRoot -ForceHash)
$cacheLines = [IO.File]::ReadAllLines($statCachePath, [Text.Encoding]::UTF8)
$rowFields = $cacheLines[1].Split([char]9)
$rowFields[4] = $(if ($rowFields[4][0] -eq 'A') { 'B' } else { 'A' }) + $rowFields[4].Substring(1)
$cacheLines[1] = $rowFields -join "`t"
[IO.File]::WriteAllText($statCachePath, (($cacheLines -join "`n") + "`n"), $utf8)
$cacheWarnings = @()
$alteredId = Get-MivSourceId $statRoot -WarningVariable cacheWarnings -WarningAction SilentlyContinue
Assert-That ($alteredId -eq $forceId -and $cacheWarnings.Count -gt 0) 'an altered but well-formed cache row was trusted'
# A locked cache also fails closed on both reading and writing.
[void] (Get-MivSourceId $statRoot -ForceHash)
$locked = [IO.File]::Open($statCachePath, 'Open', 'ReadWrite', 'None')
try {
    foreach ($forceHash in @($false, $true)) {
        $cacheWarnings = @()
        $lockedId = Get-MivSourceId $statRoot -ForceHash:$forceHash -WarningVariable cacheWarnings -WarningAction SilentlyContinue
        Assert-That ($lockedId -eq $forceId -and $cacheWarnings.Count -gt 0) 'cache I/O failure did not warn and hash correctly'
    }
} finally { $locked.Dispose() }
Write-Host '[test-test-tools] stat cache: identity, warm read avoidance, size/mtime/creation/racy changes, ForceHash, corruption and I/O failures PASS'

$run1 = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
Assert-That ($run1.ExitCode -eq 0 -and $run1.StdOut.Contains('MISS')) "initial miss failed: $($run1.StdOut) $($run1.StdErr)"
$count1 = Count-Runs
$run2 = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
Assert-That ($run2.ExitCode -eq 0 -and $run2.StdOut.Contains('HIT') -and (Count-Runs) -eq $count1) "hit ran tests: $($run2.StdOut)"
# Config-declared variables must be discovered, not just a hardcoded allowlist.
foreach ($probe in @(
    @('LIBSQLITE3_FLAGS', 'SQLITE_MAX_ATTACHED=21'), @('FFMPEG_DIR', 'other-fixture-directory'),
    @('TEST_CONFIG_ENV', 'changed'), @('TEST_CONFIG_TABLE_ENV', 'changed'),
    @('RUSTFLAGS', '--cfg=miv_tool_probe'), @('RUSTDOCFLAGS', '--cfg=miv_tool_probe'),
    @('CARGO_BUILD_JOBS', '1'), @('LIBCLANG_PATH', 'unused-fixture-clang'),
    @('MIV_TOOL_PROBE', 'changed'), @('MIMV_TOOL_PROBE', 'changed')
)) {
    $name = $probe[0]
    $before = [Environment]::GetEnvironmentVariable($name)
    $environmentBefore = Get-MivEnvironmentId $fixture
    $runsBefore = Count-Runs
    try {
        [Environment]::SetEnvironmentVariable($name, $probe[1])
        Assert-That ((Get-MivEnvironmentId $fixture) -ne $environmentBefore) "environment ID omitted $name"
        $envMiss = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
        Assert-That ($envMiss.ExitCode -eq 0 -and $envMiss.StdOut.Contains('MISS') -and (Count-Runs) -eq ($runsBefore + 1)) "environment change did not rerun: $name $($envMiss.StdOut)"
        $envHit = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
        Assert-That ($envHit.ExitCode -eq 0 -and $envHit.StdOut.Contains('HIT') -and (Count-Runs) -eq ($runsBefore + 1)) "environment state was not reusable: $name"
    } finally { [Environment]::SetEnvironmentVariable($name, $before) }
}
# Model manually copied DLLs beside the test executable; this tiny crate does
# not import them, so identity behavior is checked without loading fake DLLs.
$nativeDlls = @('avcodec-61.dll', 'avfilter-10.dll', 'avformat-61.dll', 'avutil-59.dll', 'swresample-5.dll', 'swscale-8.dll')
foreach ($dll in $nativeDlls) { Write-Fixture "target/debug/deps/$dll" 'native-fixture-v1' }
$dllBaseline = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
Assert-That ($dllBaseline.ExitCode -eq 0 -and $dllBaseline.StdOut.Contains('MISS')) 'DLL additions did not invalidate receipt'
foreach ($dll in $nativeDlls) {
    $runsBefore = Count-Runs
    Write-Fixture "target/debug/deps/$dll" 'native-fixture-v2'
    $dllSwap = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
    Assert-That ($dllSwap.ExitCode -eq 0 -and $dllSwap.StdOut.Contains('MISS') -and (Count-Runs) -eq ($runsBefore + 1)) "DLL replacement did not rerun: $dll"
    Remove-Item -LiteralPath (Join-Path $fixture "target/debug/deps/$dll")
    $dllRemoved = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
    Assert-That ($dllRemoved.ExitCode -eq 0 -and $dllRemoved.StdOut.Contains('MISS') -and (Count-Runs) -eq ($runsBefore + 2)) "DLL removal did not rerun: $dll"
    # Keep each next state distinct from every earlier successful receipt.
    Write-Fixture "target/debug/deps/$dll" 'native-fixture-v3'
}
$count1 = Count-Runs
Write-Fixture 'src/foo.rs' ($foo + "`n// source invalidates receipt`n")
$run3 = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
Assert-That ($run3.ExitCode -eq 0 -and $run3.StdOut.Contains('MISS') -and (Count-Runs) -eq ($count1 + 1)) "dirty source did not rerun: $($run3.StdOut)"
$forced = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3', '-Force')
Assert-That ($forced.ExitCode -eq 0 -and $forced.StdOut.Contains('MISS')) 'Force did not run tests'
# Exercise the public -Force route with an edit outside the stat contract.
$forceFile = Join-Path $fixture 'vendor/raw-samples/input.raw'
[IO.File]::SetLastWriteTimeUtc($forceFile, $oldTime)
$forceBaseline = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
Assert-That ($forceBaseline.ExitCode -eq 0) 'cannot prepare same-stat Force fixture'
$forceSourceBefore = Get-MivSourceId $fixture
$forceCreated = [IO.File]::GetCreationTimeUtc($forceFile)
Write-Fixture 'vendor/raw-samples/input.raw' 'fixture-v3'
[IO.File]::SetLastWriteTimeUtc($forceFile, $oldTime)
[IO.File]::SetCreationTimeUtc($forceFile, $forceCreated)
$sameStatHit = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3')
Assert-That ($sameStatHit.ExitCode -eq 0 -and $sameStatHit.StdOut.Contains('HIT') -and $sameStatHit.StdOut.Contains("source=$forceSourceBefore")) 'same-stat edit did not exercise a ledger hit'
$forceSourceAfter = Get-MivSourceId $fixture -NoCache
$sameStatForced = Run-Script 'test-lib-full.ps1' @('-MinimumTests', '3', '-Force')
Assert-That ($forceSourceAfter -ne $forceSourceBefore -and $sameStatForced.ExitCode -eq 0 -and $sameStatForced.StdOut.Contains('MISS') -and $sameStatForced.StdOut.Contains("source=$forceSourceAfter")) 'public Force did not rehash same-stat input'
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
Assert-That ($docs.ExitCode -eq 2 -and $docs.StdOut.Contains('docs/example.md')) 'unmapped documentation was silently dropped'
# Use a PS entry script for array arguments (powershell.exe -File does not bind
# a multi-element string[] reliably). Mix one selectable and one unsupported path.
$focusedRunsBefore = @(Get-ChildItem (Join-Path $fixture 'target/test-focused/runs') -Directory).Count
foreach ($unsupported in @('testdata/input.wav', 'scripts/tool.ps1', 'crates/ort-patched/src/lib.rs', 'vendor/egui/src/lib.rs', '.config/nextest.toml', 'build.rs', 'Cargo.toml', 'Cargo.lock', 'helper/build.rs', 'helper/Cargo.toml', 'helper/fixture.bin', 'docs/example.md')) {
    $mixedPlan = Get-MivFocusedPlan $fixture @('src/foo.rs', $unsupported)
    Assert-That ($mixedPlan.Entries.Count -eq 1 -and $mixedPlan.BroadPaths -contains $unsupported) "mixed plan omitted unsupported path: $unsupported"
    $entryScript = Join-Path $fixture 'target/mixed-focused.ps1'
    $wrapper = (Join-Path $PSScriptRoot 'test-focused.ps1').Replace("'", "''")
    $fixtureQuoted = $fixture.Replace("'", "''")
    [IO.File]::WriteAllText($entryScript, "& '$wrapper' -RepoRoot '$fixtureQuoted' -Path @('src/foo.rs', '$unsupported')`nexit `$LASTEXITCODE`n", $utf8)
    $mixed = Invoke-MivTool 'powershell.exe' @('-NoProfile', '-NonInteractive', '-File', $entryScript) $repo
    Assert-That ($mixed.ExitCode -eq 2 -and $mixed.StdOut.Contains($unsupported) -and $mixed.StdOut.Contains('test-lib-full.ps1')) "mixed changes were accepted: $unsupported $($mixed.StdOut)"
    Assert-That (@(Get-ChildItem (Join-Path $fixture 'target/test-focused/runs') -Directory).Count -eq $focusedRunsBefore) 'focused ran a partial selection before rejecting unsupported inputs'
    Write-Host "[test-test-tools] mixed selectable/unmapped $unsupported`: exit 2; no test run"
}
$escape = Run-Script 'test-focused.ps1' @('-Path', '../outside.rs')
Assert-That ($escape.ExitCode -ne 0) 'outside path was accepted'
Assert-That ((Get-FileHash -LiteralPath $index).Hash -eq $indexBefore) 'test tools changed the index'
if ($Benchmark) {
    # Optional reproducible scale check; no application or main crate is built.
    $benchRoot = Join-Path $fixture 'target/stat-cache-benchmark'
    foreach ($dir in @('src', 'testimage/flat', 'testimage/nested/sub')) {
        [void] [IO.Directory]::CreateDirectory((Join-Path $benchRoot $dir))
    }
    [void] (Invoke-MivGit $benchRoot @('init', '-q'))
    [void] (Invoke-MivGit (Join-Path $benchRoot 'testimage/nested') @('init', '-q'))
    [IO.File]::WriteAllText((Join-Path $benchRoot '.gitignore'), "target/`ntestimage/`n", $utf8)
    [IO.File]::WriteAllText((Join-Path $benchRoot 'Cargo.toml'), "[package]`nname = `"mimageviewer`"`nversion = `"0.0.0`"`nedition = `"2021`"`n[workspace]`n", $utf8)
    [IO.File]::WriteAllText((Join-Path $benchRoot 'src/lib.rs'), '#[test] fn benchmark_fixture() { assert_eq!(2 + 2, 4); }', $utf8)
    $benchLock = Invoke-MivTool 'cargo' @('generate-lockfile', '--offline') $benchRoot
    Assert-That ($benchLock.ExitCode -eq 0) 'cannot prepare benchmark lockfile'
    $bytes = New-Object byte[] 65536
    $random = New-Object Random 20261016
    $random.NextBytes($bytes)
    for ($i = 0; $i -lt 21247; $i++) {
        $dir = if ($i -lt 20000) { 'testimage/flat' } else { 'testimage/nested/sub' }
        $path = Join-Path $benchRoot ($dir + '/' + $i.ToString('D5') + '.bin')
        [IO.File]::WriteAllBytes($path, $bytes)
        [IO.File]::SetLastWriteTimeUtc($path, $oldTime)
    }
    $measurements = New-Object 'System.Collections.Generic.List[object]'
    $benchId = $null
    foreach ($mode in @('uncached', 'populate', 'warm-1', 'warm-2', 'warm-3')) {
        $watch = [Diagnostics.Stopwatch]::StartNew()
        $id = Get-MivSourceId $benchRoot -NoCache:($mode -eq 'uncached') -ForceHash:($mode -eq 'populate')
        $seconds = $watch.Elapsed.TotalSeconds
        if ($null -eq $benchId) { $benchId = $id }
        Assert-That ($id -eq $benchId) "benchmark identity mismatch: $mode"
        $measurements.Add([pscustomobject]@{ Mode = $mode; Seconds = $seconds; Source = $id })
    }
    $benchArgs = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $PSScriptRoot 'test-lib-full.ps1'), '-RepoRoot', $benchRoot, '-MinimumTests', '1')
    $prime = Invoke-MivTool 'powershell.exe' $benchArgs $repo
    Assert-That ($prime.ExitCode -eq 0 -and $prime.StdOut.Contains('PASS: 1 passed')) "benchmark receipt preparation failed: $($prime.StdOut) $($prime.StdErr)"
    $benchRuns = @(Get-ChildItem (Join-Path $benchRoot 'target/test-lib-full/runs') -Directory).Count
    foreach ($iteration in 1..3) {
        $hit = Invoke-MivTool 'powershell.exe' $benchArgs $repo
        Assert-That ($hit.ExitCode -eq 0 -and $hit.StdOut.Contains('HIT') -and $hit.StdOut.Contains("source=$benchId")) 'benchmark did not reuse the receipt'
        Assert-That (@(Get-ChildItem (Join-Path $benchRoot 'target/test-lib-full/runs') -Directory).Count -eq $benchRuns) 'benchmark HIT created a test run'
        [IO.File]::WriteAllText((Join-Path $benchRoot "target/hit-$iteration.log"), $hit.StdOut + $hit.StdErr, $utf8)
        $measurements.Add([pscustomobject]@{ Mode = "ledger-hit-$iteration"; Seconds = $hit.Seconds; Source = $benchId })
    }
    $resultPath = Join-Path $benchRoot 'target/measurements.json'
    [IO.File]::WriteAllText($resultPath, ($measurements | ConvertTo-Json), $utf8)
    Write-Host '[test-test-tools] benchmark: 21,247 binary fixtures, 1,392,443,392 bytes; 1,247 files in a nested repository'
    Write-Host ($measurements | Format-Table -AutoSize | Out-String)
    Write-Host "[test-test-tools] benchmark results: $resultPath"
}
Write-Host "[test-test-tools] PASS; disposable fixtures/logs retained at $fixture"
