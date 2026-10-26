# Noninteractive candidate/approval regression. Only disposable fixture paths.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'miv-sound-candidate.ps1')
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'build-miv-sound.ps1'), [ref]$null, [ref]$parseErrors)
if ($parseErrors.Count) { throw 'build-miv-sound.ps1 parse failed' }
$functions = $ast.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] }, $false)
. ([scriptblock]::Create(($functions | ForEach-Object { $_.Extent.Text }) -join "`n"))
function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw "[candidate-test] $Message" }
}
function Assert-Throws([scriptblock] $Action, [string] $Pattern) {
    try { & $Action } catch {
        Assert-True ($_.Exception.Message -match $Pattern) "Unexpected error: $_"
        return
    }
    throw "[candidate-test] Expected rejection: $Pattern"
}
$root = Join-Path ([IO.Path]::GetTempPath()) ('miv-sound-candidate-test-' + [Guid]::NewGuid().ToString('N'))
$utf8 = New-Object Text.UTF8Encoding($false)
New-Item -ItemType Directory -Path $root -Force | Out-Null
try {
    $source = Join-Path $root 'source'
    New-Item -ItemType Directory -Path $source -Force | Out-Null
    $null = Invoke-SoundGit $source @('init', '--quiet')
    $sourceFile = Join-Path $source 'source.txt'
    [IO.File]::WriteAllText($sourceFile, 'committed source', $utf8)
    $null = Invoke-SoundGit $source @('add', 'source.txt')
    $null = Invoke-SoundGit $source @('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'fixture')
    $null = Invoke-SoundGit $source @('tag', $MivSound.UPSTREAM_TAG)
    $identity = Get-SoundSourceIdentity $source
    Assert-True ($identity.commit.Length -eq 40) 'clean source identity missing'
    [IO.File]::WriteAllText($sourceFile, 'uncommitted change', $utf8)
    Assert-Throws { Get-SoundSourceIdentity $source } 'Uncommitted'
    [IO.File]::WriteAllText($sourceFile, 'committed source', $utf8)
    $sub = Join-Path $source 'external/dependency'
    New-Item -ItemType Directory -Path $sub -Force | Out-Null
    $null = Invoke-SoundGit $sub @('init', '--quiet')
    [IO.File]::WriteAllText((Join-Path $sub 'sub.txt'), 'submodule', $utf8)
    $null = Invoke-SoundGit $sub @('add', 'sub.txt')
    $null = Invoke-SoundGit $sub @('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'submodule fixture')
    $subCommit = Invoke-SoundGit $sub @('rev-parse', 'HEAD')
    [IO.File]::WriteAllText((Join-Path $source '.gitmodules'), "[submodule `"external/dependency`"]`npath = external/dependency`nurl = fixture.invalid`n", $utf8)
    $null = Invoke-SoundGit $source @('add', '.gitmodules')
    $null = Invoke-SoundGit $source @('update-index', '--add', '--cacheinfo', "160000,$subCommit,external/dependency")
    $null = Invoke-SoundGit $source @('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'gitlink fixture')
    $identity = Get-SoundSourceIdentity $source
    Assert-True ($identity.submodules.Count -eq 1 -and $identity.submodules[0].commit -ceq $subCommit) 'submodule commit missing'
    [IO.File]::WriteAllText((Join-Path $sub 'sub.txt'), 'dirty submodule', $utf8)
    Assert-Throws { Get-SoundSourceIdentity $source } 'Uncommitted|Dirty submodule'
    $null = Invoke-SoundGit $sub @('add', 'sub.txt')
    $null = Invoke-SoundGit $sub @('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'different pin')
    Assert-Throws { Get-SoundSubmodules $source '' } 'Mismatched'
    $work = Join-Path $root 'work'
    $artifact = Join-Path $work 'artifact'
    $pe = Join-Path $artifact $MivSound.Plugin
    New-Item -ItemType Directory -Path (Split-Path $pe) -Force | Out-Null
    $bytes = New-Object byte[] 512
    $bytes[0] = 0x4d; $bytes[1] = 0x5a; $bytes[0x3c] = 0x40
    $bytes[64] = 0x50; $bytes[65] = 0x45; $bytes[88] = 0x0b; $bytes[89] = 0x02
    [IO.File]::WriteAllBytes($pe, $bytes)
    [IO.File]::WriteAllText((Join-Path $artifact 'VERSION'), ($MivSound.VERSION + "`n"), $utf8)
    # Small synthetic licenses: tests assert complete text is retained, not just
    # the four attribution/permission/disclaimer substrings checked separately.
    $licenses = @(
        @{ path='LICENSE'; text="Copyright (c) 2026, Yoshiyuki Kobayashi`nPermission is hereby granted`nKEEP COMPLETE FIRST TEXT`nTHE SOFTWARE IS PROVIDED `"AS IS`"" },
        @{ path='external/effetune/LICENSE'; text="Copyright (c) 2025-2026, Yoshiyuki Kobayashi`nPermission is hereby granted`nKEEP COMPLETE SECOND TEXT`nTHE SOFTWARE IS PROVIDED `"AS IS`"" }
    )
    foreach ($notice in $MivSoundNotices) {
        $path = Join-Path $artifact ($MivSound.BUNDLE_NAME + '/' + $notice)
        New-Item -ItemType Directory -Path (Split-Path $path) -Force | Out-Null
        [IO.File]::WriteAllText($path, (($licenses | ForEach-Object { $_.text }) -join "`n`n"), $utf8)
    }
    $manifest = Get-MivSoundManifestText (Get-MivSoundInventory $artifact)
    $id = Get-MivSoundTextHash $manifest
    $candidateDir = Join-Path $root "target/miv-sound-candidates/$id"
    New-Item -ItemType Directory -Path (Split-Path $candidateDir) -Force | Out-Null
    Move-Item -LiteralPath $work -Destination $candidateDir
    $artifact = Join-Path $candidateDir 'artifact'
    $record = @{ schema=1; id=$id; version=$MivSound.VERSION; manifest=$manifest; provenance=@{ inspection_only=$false; identity=@{ commit=('1'*40); upstream_tag=$MivSound.UPSTREAM_TAG; upstream_commit=('2'*40) }; upstream_licenses=$licenses } }
    [IO.File]::WriteAllText((Join-Path $candidateDir 'candidate.json'), ($record | ConvertTo-Json -Depth 8), $utf8)
    $null = Read-MivSoundCandidate -RepoRoot $root -Candidate $id
    Assert-Throws { Read-MivSoundCandidate -RepoRoot $root -Candidate '../escape' } 'SHA-256'
    $extra = Join-Path $artifact 'extra'
    [IO.File]::WriteAllText($extra, 'extra', $utf8)
    Assert-Throws { Read-MivSoundCandidate -RepoRoot $root -Candidate $id } 'file set/hash mismatch'
    Remove-Item -LiteralPath $extra
    $version = Join-Path $artifact 'VERSION'
    $original = [IO.File]::ReadAllBytes($version)
    [IO.File]::WriteAllText($version, 'changed', $utf8)
    Assert-Throws { Read-MivSoundCandidate -RepoRoot $root -Candidate $id } 'file set/hash mismatch'
    Remove-Item -LiteralPath $version
    Assert-Throws { Read-MivSoundCandidate -RepoRoot $root -Candidate $id } 'file set/hash mismatch'
    [IO.File]::WriteAllBytes($version, $original)
    $runtime = Join-Path $root 'target/dev-runtime'
    Copy-MivSoundCandidateRuntime -RepoRoot $root -Candidate $id -OutputDir $runtime
    Assert-True ((Get-MivSoundManifestText (Get-MivSoundInventory (Join-Path $runtime 'effetune'))) -ceq $manifest) 'runtime differs'
    $backupRoot = Join-Path $root 'backup'
    Assert-Throws { Approve-MivSoundCandidate -RepoRoot $root -Candidate $id -BackupRoot $backupRoot } 'ctest.json'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $root $MivSound.SOURCE_ROOT))) 'published without ctest'
    $log = Join-Path $candidateDir 'ctest.log'
    [IO.File]::WriteAllText($log, '100% tests passed, 0 tests failed out of 1 (synthetic fixture only)', $utf8)
    $evidence = @{ candidate=$id; passed=$true; log_sha256=(Get-FileHash -LiteralPath $log -Algorithm SHA256).Hash.ToLowerInvariant() }
    [IO.File]::WriteAllText((Join-Path $candidateDir 'ctest.json'), ($evidence | ConvertTo-Json), $utf8)
    [IO.File]::AppendAllText($log, 'corruption', $utf8)
    Assert-Throws { Approve-MivSoundCandidate -RepoRoot $root -Candidate $id -BackupRoot $backupRoot } 'successful ctest evidence'
    [IO.File]::WriteAllText($log, '100% tests passed, 0 tests failed out of 1 (synthetic fixture only)', $utf8)
    $record.provenance.upstream_licenses[1].text += ' omitted upstream text'
    [IO.File]::WriteAllText((Join-Path $candidateDir 'candidate.json'), ($record | ConvertTo-Json -Depth 8), $utf8)
    Assert-Throws { Approve-MivSoundCandidate -RepoRoot $root -Candidate $id -BackupRoot $backupRoot } 'Full upstream license absent'
    $record.provenance.upstream_licenses[1].text = $record.provenance.upstream_licenses[1].text.Replace(' omitted upstream text', '')
    [IO.File]::WriteAllText((Join-Path $candidateDir 'candidate.json'), ($record | ConvertTo-Json -Depth 8), $utf8)
    Approve-MivSoundCandidate -RepoRoot $root -Candidate $id -BackupRoot $backupRoot
    $vendor = Join-Path $root $MivSound.SOURCE_ROOT
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $vendor 'candidate.json'))) 'management record leaked into artifact'
    Assert-MivEffetuneRawSource -SourceRoot $vendor -ManifestPath (Join-Path $root $MivSound.Manifest)
    $backup = Join-Path $backupRoot "miv-sound-$($MivSound.VERSION)"
    Assert-True ((Get-MivSoundManifestText (Get-MivSoundInventory (Join-Path $backup 'artifact'))) -ceq $manifest) 'backup differs'
    Assert-True (Test-Path -LiteralPath (Join-Path $backup 'approval/PROVENANCE.md')) 'backup approval metadata missing'
    Assert-True ((Get-Content -LiteralPath (Join-Path $backup 'approval/PROVENANCE.md') -Raw).Contains("Upstream tag: $($MivSound.UPSTREAM_TAG)")) 'upstream tag changed'
    Assert-Throws { Approve-MivSoundCandidate -RepoRoot $root -Candidate $id -BackupRoot $backupRoot } 'already exists'
    # Exercise the public build command with a disposable Git source and a
    # no-op build fixture. The actual upstream build is a separate external gate.
    $sdk = Join-Path $source 'build/windows-release/_deps/vst3sdk-src'
    New-Item -ItemType Directory -Path $sdk -Force | Out-Null
    $null = Invoke-SoundGit $sdk @('init', '--quiet')
    [IO.File]::WriteAllText((Join-Path $sdk 'sdk.txt'), 'sdk pin fixture', $utf8)
    $null = Invoke-SoundGit $sdk @('add', 'sdk.txt')
    $null = Invoke-SoundGit $sdk @('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'sdk fixture')
    $sdkCommit = Invoke-SoundGit $sdk @('rev-parse', 'HEAD')
    foreach ($dir in @('cmake', 'src/plugin', 'external/effetune', 'build/windows-release/CMakeFiles/fixture', 'build/windows-release/VST3/Release')) {
        New-Item -ItemType Directory -Path (Join-Path $source $dir) -Force | Out-Null
    }
    [IO.File]::WriteAllText((Join-Path $source '.gitignore'), "build/`n", $utf8)
    [IO.File]::WriteAllText((Join-Path $source 'build-miv.bat'), "@echo off`ngoto done`ncmake --preset windows-release -DFIXTURE=ON`n:done`necho BUILD-OK`nexit /b 0`n", [Text.Encoding]::ASCII)
    [IO.File]::WriteAllText((Join-Path $source 'CMakePresets.json'), '{}', $utf8)
    [IO.File]::WriteAllText((Join-Path $source 'src/plugin/version.h'), "#define EFFETUNE_PLUGIN_VERSION_STR `"$($MivSound.VERSION)`"`n", $utf8)
    [IO.File]::WriteAllText((Join-Path $source 'cmake/Dependencies.cmake'), "GIT_TAG $sdkCommit`n", $utf8)
    foreach ($license in $licenses) { [IO.File]::WriteAllText((Join-Path $source $license.path), $license.text, $utf8) }
    [IO.File]::WriteAllText((Join-Path $source 'build/windows-release/CMakeFiles/fixture/CMakeCXXCompiler.cmake'), 'set(CMAKE_CXX_COMPILER_VERSION "fixture")', $utf8)
    $cache = "CMAKE_BUILD_TYPE:STRING=Release`nEFFETUNE_AUTOMATION_HOST_GATE_FIXTURE:BOOL=OFF`nEFFETUNE_BUILD_PLUGIN:BOOL=ON`nEFFETUNE_BUILD_TESTS:BOOL=ON`nEFFETUNE_UPSTREAM_SOURCE_DIR:PATH=$(Join-Path $source 'external/effetune')`n"
    [IO.File]::WriteAllText((Join-Path $source 'build/windows-release/CMakeCache.txt'), $cache, $utf8)
    Copy-Item -LiteralPath (Join-Path $artifact $MivSound.BUNDLE_NAME) -Destination (Join-Path $source 'build/windows-release/VST3/Release') -Recurse
    $currentSub = Invoke-SoundGit $sub @('rev-parse', 'HEAD')
    $null = Invoke-SoundGit $source @('update-index', '--cacheinfo', "160000,$currentSub,external/dependency")
    $null = Invoke-SoundGit $source @('add', '.gitignore', 'build-miv.bat', 'CMakePresets.json', 'src', 'cmake', 'LICENSE', 'external/effetune')
    $null = Invoke-SoundGit $source @('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'build fixture')
    $repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
    $generated = Join-Path $repo "target/miv-sound-candidates/$id"
    Assert-True (-not (Test-Path -LiteralPath $generated)) 'build fixture ID already exists; refusing overwrite'
    try {
        & (Join-Path $PSScriptRoot 'build-miv-sound.ps1') -SourceRoot $source -SkipCTest
        $built = Read-MivSoundCandidate -RepoRoot $repo -Candidate $id
        Assert-True ($built.Record.provenance.identity.commit.Length -eq 40) 'build provenance missing'
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $generated 'ctest.json'))) 'skip-ctest created passing evidence'
    } finally {
        if (Test-Path -LiteralPath $generated) {
            Assert-MivSoundSafeDirectory $generated
            Remove-Item -LiteralPath $generated -Recurse -Force
        }
    }
    Write-Host '[candidate-test] PASS (synthetic artifacts only)'
} finally {
    $resolved = [IO.Path]::GetFullPath($root)
    if (-not $resolved.StartsWith([IO.Path]::GetFullPath([IO.Path]::GetTempPath()), [StringComparison]::OrdinalIgnoreCase)) { throw 'cleanup escaped temp' }
    Assert-MivSoundSafeDirectory $resolved
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
