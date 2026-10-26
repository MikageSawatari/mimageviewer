# Build -> verify an immutable candidate -> explicitly approve that same input.
[CmdletBinding(DefaultParameterSetName = 'Build')]
param(
    [Parameter(ParameterSetName = 'Build')]
    [string] $SourceRoot = 'G:\home\miv-sound',
    [Parameter(ParameterSetName = 'Build')]
    [switch] $SkipCTest,
    [Parameter(Mandatory = $true, ParameterSetName = 'Approve')]
    [switch] $Approve,
    [Parameter(Mandatory = $true, ParameterSetName = 'Test')]
    [switch] $TestCandidate,
    [Parameter(Mandatory = $true, ParameterSetName = 'Approve')]
    [Parameter(Mandatory = $true, ParameterSetName = 'Test')]
    [string] $Candidate
)
$ErrorActionPreference = 'Stop'
$env:MSBUILDDISABLENODEREUSE = '1'
. (Join-Path $PSScriptRoot 'miv-sound-candidate.ps1')
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$utf8 = New-Object Text.UTF8Encoding($false)

function Invoke-SoundGit {
    param([string] $Root, [string[]] $Arguments)
    $savedPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $value = @(& git -C $Root @Arguments 2>&1)
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $savedPreference }
    if ($code -ne 0) { throw "[miv-sound] git failed: $Arguments ($value)" }
    return (($value | Where-Object { $_ -isnot [Management.Automation.ErrorRecord] }) -join "`n")
}

function Get-SoundSourceIdentity {
    param([string] $Root)
    if (Invoke-SoundGit $Root @('status', '--porcelain=v1', '--untracked-files=all', '--ignore-submodules=none')) {
        throw '[miv-sound] Uncommitted source or submodule changes; commit them before building'
    }
    $submodules = @(Get-SoundSubmodules $Root '')
    $commit = Invoke-SoundGit $Root @('rev-parse', 'HEAD')
    $upstream = Invoke-SoundGit $Root @('rev-parse', ($MivSound.UPSTREAM_TAG + '^{commit}'))
    $null = Invoke-SoundGit $Root @('merge-base', '--is-ancestor', $upstream, $commit)
    return [ordered]@{ commit = $commit; upstream_tag = $MivSound.UPSTREAM_TAG; upstream_commit = $upstream; submodules = $submodules }
}

function Get-SoundSubmodules {
    param([string] $Root, [string] $Prefix, [switch] $AllowUnusedSdkModules)
    # Use git plumbing rather than `git submodule` (which starts sh.exe and
    # cannot run reliably inside the Windows sandbox).
    foreach ($line in (Invoke-SoundGit $Root @('ls-files', '--stage')).Split("`n")) {
        if ($line -match '^160000 ([a-f0-9]{40}) 0\t(.+)$') {
            $expected = $Matches[1]; $name = $Matches[2]
            $subRoot = Join-Path $Root $name
            if ($AllowUnusedSdkModules -and $name -in @('doc', 'tutorials', 'vstgui4') -and
                -not (Test-Path -LiteralPath (Join-Path $subRoot '.git'))) {
                [ordered]@{ path = "$Prefix$name"; commit = $expected; initialized = $false }
                continue
            }
            $actual = Invoke-SoundGit $subRoot @('rev-parse', 'HEAD')
            if ($actual -cne $expected) { throw "[miv-sound] Mismatched/uninitialized submodule: $Prefix$name" }
            if (Invoke-SoundGit $subRoot @('status', '--porcelain=v1', '--untracked-files=all')) {
                throw "[miv-sound] Dirty submodule: $Prefix$name"
            }
            [ordered]@{ path = "$Prefix$name"; commit = $actual }
            Get-SoundSubmodules $subRoot "$Prefix$name/"
        } elseif ($line -match '^160000 ') {
            throw '[miv-sound] Conflicted submodule entry'
        }
    }
}

function Get-SoundBuildBundle {
    param([string] $Root)
    return Join-Path $Root ("build/windows-release/VST3/Release/" + $MivSound.BUNDLE_NAME)
}

function Write-SoundCTestRecord {
    param([object] $Item, [string] $Log, [string] $ContractHash)
    $logPath = Join-Path $Item.Root 'ctest.log'
    [IO.File]::WriteAllText($logPath, $Log, $utf8)
    $evidence = [ordered]@{ candidate = $Item.Record.id; build_contract_sha256 = $ContractHash; passed = $true; command = 'ctest --preset windows-release --output-on-failure'; utc = [DateTime]::UtcNow.ToString('o'); log_sha256 = (Get-FileHash -LiteralPath $logPath -Algorithm SHA256).Hash.ToLowerInvariant() }
    [IO.File]::WriteAllText((Join-Path $Item.Root 'ctest.json'), ($evidence | ConvertTo-Json), $utf8)
}

function Invoke-SoundCandidateCTest {
    param([object] $Item)
    $contractHash = Assert-SoundBuildContract $Item
    $source = $item.Record.source_root
    $identity = Get-SoundSourceIdentity $source
    if (($identity | ConvertTo-Json -Compress) -cne ($item.Record.provenance.identity | ConvertTo-Json -Compress)) { throw '[miv-sound] Candidate source identity changed' }
    # Do not rebuild: test exactly the binaries copied into this candidate.
    $bundleEntries = Get-MivSoundInventory (Get-SoundBuildBundle $source)
    foreach ($name in $bundleEntries.Keys) {
        if ($item.Entries[$MivSound.BUNDLE_NAME + '/' + $name] -cne $bundleEntries[$name]) { throw '[miv-sound] Build output differs from candidate' }
    }
    if ($bundleEntries.Count + 1 -ne $item.Entries.Count) { throw '[miv-sound] Build output file set changed' }
    $cmake = $item.Record.provenance.tools.cmake_path
    Push-Location $source
    try {
        $savedPreference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $log = & (Join-Path (Split-Path $cmake) 'ctest.exe') --preset windows-release --output-on-failure 2>&1 | Out-String
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $savedPreference; Pop-Location }
    Write-Host $log
    if ($code -ne 0 -or $log -notmatch '100% tests passed, 0 tests failed out of [1-9][0-9]*') { throw '[miv-sound] ctest did not pass' }
    if ((Get-SoundSourceIdentity $source | ConvertTo-Json -Compress) -cne ($identity | ConvertTo-Json -Compress)) { throw '[miv-sound] Source changed during ctest' }
    $after = Get-MivSoundInventory (Get-SoundBuildBundle $source)
    if ((Get-MivSoundManifestText $after) -cne (Get-MivSoundManifestText $bundleEntries)) { throw '[miv-sound] Build output changed during ctest' }
    if ((Assert-SoundBuildContract $item) -cne $contractHash) { throw '[miv-sound] Build/test contract changed during ctest' }
    $testCount = @($item.Record.provenance.build_contract.selected_tests).Count
    if ($log -notmatch ("100% tests passed, 0 tests failed out of " + $testCount + '(?![0-9])')) { throw '[miv-sound] ctest did not run the complete candidate test inventory' }
    Write-SoundCTestRecord $item $log $contractHash
}

if ($Approve -or $TestCandidate) {
    $item = Read-MivSoundCandidate -RepoRoot $repo -Candidate $Candidate
    if ($TestCandidate) {
        Invoke-SoundCandidateCTest $item
        exit 0
    }
    Approve-MivSoundCandidate -RepoRoot $repo -Candidate $Candidate -BackupRoot 'C:\home\mimageviewer_vendor_backup'
    exit 0
}

$source = (Resolve-Path -LiteralPath $SourceRoot).Path
$identity = Get-SoundSourceIdentity $source
$savedPreference = $ErrorActionPreference
try {
    $ErrorActionPreference = 'Continue'
    # Capture the configuration/test inventory before running tests. The same
    # checked ctest path serves normal builds and later -TestCandidate runs.
    $buildLog = & (Join-Path $source 'build-miv.bat') --no-test 2>&1 | Out-String
    $code = $LASTEXITCODE
} finally { $ErrorActionPreference = $savedPreference }
Write-Host $buildLog
if ($code -ne 0) { throw "[miv-sound] build-miv.bat failed ($code)" }
if ((Get-SoundSourceIdentity $source | ConvertTo-Json -Compress) -cne ($identity | ConvertTo-Json -Compress)) { throw '[miv-sound] Source changed during build' }
$cache = Join-Path $source 'build/windows-release/CMakeCache.txt'
$cacheText = [IO.File]::ReadAllText($cache)
foreach ($setting in @('CMAKE_BUILD_TYPE:STRING=Release', 'EFFETUNE_AUTOMATION_HOST_GATE_FIXTURE:BOOL=OFF', 'EFFETUNE_BUILD_PLUGIN:BOOL=ON', 'EFFETUNE_BUILD_TESTS:BOOL=ON')) {
    if (-not $cacheText.Contains($setting)) { throw "[miv-sound] Unexpected configuration: $setting" }
}
if ($cacheText -notmatch '(?m)^EFFETUNE_UPSTREAM_SOURCE_DIR:PATH=(.+)\r?$' -or
    [IO.Path]::GetFullPath($Matches[1].Trim()) -ne [IO.Path]::GetFullPath((Join-Path $source 'external/effetune'))) {
    throw '[miv-sound] Build used an alternate upstream source directory'
}
if ($cacheText -match '(?m)^FETCHCONTENT_SOURCE_DIR_VST3SDK:PATH=(.+)\r?$' -and $Matches[1].Trim()) {
    throw '[miv-sound] Build used an alternate VST3 SDK source directory'
}
$versionHeader = [IO.File]::ReadAllText((Join-Path $source 'src/plugin/version.h'))
if ($versionHeader -notmatch ('#define EFFETUNE_PLUGIN_VERSION_STR "' + [regex]::Escape($MivSound.VERSION) + '"')) { throw '[miv-sound] Source version differs from distribution identity' }
$candidateParent = Join-Path $repo 'target/miv-sound-candidates'
Assert-MivSoundSafeDirectory $candidateParent
$temporary = Join-Path $candidateParent ('building-' + [Guid]::NewGuid().ToString('N'))
$artifact = Join-Path $temporary 'artifact'
New-Item -ItemType Directory -Path $artifact -Force | Out-Null
Copy-Item -LiteralPath (Get-SoundBuildBundle $source) -Destination $artifact -Recurse
[IO.File]::WriteAllText((Join-Path $artifact 'VERSION'), ($MivSound.VERSION + "`n"), $utf8)
$entries = Get-MivSoundInventory $artifact
$manifest = Get-MivSoundManifestText $entries
$id = Get-MivSoundTextHash $manifest
$cmakePath = 'C:\Program Files\CMake\bin\cmake.exe'
$compilerFiles = @(Get-ChildItem -LiteralPath (Join-Path $source 'build/windows-release/CMakeFiles') -Recurse -Filter CMakeCXXCompiler.cmake -File)
if ($compilerFiles.Count -ne 1) { throw '[miv-sound] Cannot identify MSVC compiler' }
$compiler = [IO.File]::ReadAllText($compilerFiles[0].FullName)
$sdk = Join-Path $source 'build/windows-release/_deps/vst3sdk-src'
$sdkCommit = Invoke-SoundGit $sdk @('rev-parse', 'HEAD')
if (Invoke-SoundGit $sdk @('status', '--porcelain=v1', '--untracked-files=all')) { throw '[miv-sound] Dirty VST3 SDK' }
$sdkPin = [IO.File]::ReadAllText((Join-Path $source 'cmake/Dependencies.cmake'))
if (-not $sdkPin.Contains('GIT_TAG ' + $sdkCommit)) { throw '[miv-sound] VST3 SDK differs from source pin' }
$buildScript = [IO.File]::ReadAllText((Join-Path $source 'build-miv.bat'))
if ($buildScript -notmatch '(?m)^cmake --preset windows-release[^\r\n]*') { throw '[miv-sound] Cannot record configure command from build-miv.bat' }
$configureCommand = $Matches[0]
$buildContract = Get-SoundBuildContract $source $cmakePath
$record = [ordered]@{
    schema = 1; id = $id; version = $MivSound.VERSION; source_root = $source; manifest = $manifest
    provenance = [ordered]@{
        identity = $identity
        preset = 'windows-release'
        configure_command = $configureCommand
        cmake_presets = [IO.File]::ReadAllText((Join-Path $source 'CMakePresets.json'))
        cmake_cache = $cacheText; compiler_configuration = $compiler
        build_contract = $buildContract
        build_script_sha256 = (Get-FileHash -LiteralPath (Join-Path $source 'build-miv.bat') -Algorithm SHA256).Hash.ToLowerInvariant()
        tools = [ordered]@{ cmake_path = $cmakePath; cmake = (& $cmakePath --version | Out-String).Trim(); node = (& node --version | Out-String).Trim(); vst3sdk_pin = $sdkCommit; vst3sdk_submodules = @(Get-SoundSubmodules $sdk '' -AllowUnusedSdkModules) }
        upstream_licenses = @('LICENSE', 'external/effetune/LICENSE') | ForEach-Object { [ordered]@{ path = $_; text = [IO.File]::ReadAllText((Join-Path $source $_)); sha256 = (Get-FileHash -LiteralPath (Join-Path $source $_) -Algorithm SHA256).Hash.ToLowerInvariant() } }
    }
}
if ((Get-SoundSourceIdentity $source | ConvertTo-Json -Compress) -cne ($identity | ConvertTo-Json -Compress)) { throw '[miv-sound] Source changed during candidate capture' }
$buildEntries = Get-MivSoundInventory (Get-SoundBuildBundle $source)
if ($buildEntries.Count + 1 -ne $entries.Count) { throw '[miv-sound] Build output changed during candidate capture' }
foreach ($name in $buildEntries.Keys) {
    if ($entries[$MivSound.BUNDLE_NAME + '/' + $name] -cne $buildEntries[$name]) { throw '[miv-sound] Build output changed during candidate capture' }
}
[IO.File]::WriteAllText((Join-Path $temporary 'candidate.json'), ($record | ConvertTo-Json -Depth 30), $utf8)
$destination = Join-Path $candidateParent $id
if (Test-Path -LiteralPath $destination) {
    $old = Read-MivSoundCandidate -RepoRoot $repo -Candidate $id
    if ($old.Record.provenance.identity.commit -cne $identity.commit) { throw '[miv-sound] Identical artifact already recorded from a different commit' }
    if ((Assert-SoundBuildContract $old) -cne (Get-SoundBuildContractHash $buildContract)) { throw '[miv-sound] Identical artifact already recorded with a different build/test contract' }
    Assert-MivSoundSafeDirectory $temporary
    Remove-Item -LiteralPath $temporary -Recurse -Force
} else {
    Move-Item -LiteralPath $temporary -Destination $destination
}
$item = Read-MivSoundCandidate -RepoRoot $repo -Candidate $id
if (-not $SkipCTest) {
    Invoke-SoundCandidateCTest $item
}
Write-Host "[miv-sound] CANDIDATE $id (not approved)"
if ($SkipCTest) { Write-Host '[miv-sound] ctest pending: -TestCandidate -Candidate <ID> is required before approval' }
