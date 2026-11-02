# Synthetic PE/source fixtures only. Never launch the worker or access models.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'vlm-worker-identity.ps1')
function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw "[vlm-identity-test] $Message" }
}
function Assert-Throws([scriptblock] $Action, [string] $Pattern) {
    try { & $Action } catch {
        Assert-True ($_.Exception.Message -match $Pattern) "Unexpected error: $_"
        return
    }
    throw "[vlm-identity-test] Missing expected failure: $Pattern"
}
function Write-FakeWorker([string] $Path, [string] $Marker) {
    $bytes = New-Object byte[] 256
    $bytes[0] = 0x4d; $bytes[1] = 0x5a; $bytes[0x3c] = 0x40
    $bytes[64] = 0x50; $bytes[65] = 0x45
    $text = [System.Text.Encoding]::ASCII.GetBytes($Marker)
    $out = New-Object byte[] ($bytes.Length + $text.Length)
    [Array]::Copy($bytes, $out, $bytes.Length)
    [Array]::Copy($text, 0, $out, $bytes.Length, $text.Length)
    [System.IO.File]::WriteAllBytes($Path, $out)
}
$repo = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$parent = Join-Path $repo 'target'
$testRoot = Join-Path $parent ('vlm-identity-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
try {
    $source = Join-Path $testRoot 'crates\vlm-worker'
    $worker = Join-Path $testRoot 'vendor\vlm-worker\mimageviewer-vlm.exe'
    New-Item -ItemType Directory -Path (Split-Path -Parent $worker) -Force | Out-Null
    foreach ($name in @('CMakeLists.txt', 'tools/source-identity.ps1', 'src/prompt.h', 'src/main.cpp', 'tests/protocol.cpp')) {
        $path = Join-Path $source $name
        New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
        [System.IO.File]::WriteAllText($path, "fixture $name`r`n")
    }
    $hash = Get-MivVlmWorkerSourceHash $source
    Assert-Throws { Assert-MivVlmWorkerIdentity $testRoot } 'Vendor worker is missing'
    Write-FakeWorker $worker ''
    Assert-Throws { Assert-MivVlmWorkerIdentity $testRoot } 'Missing/stale/duplicate'
    Write-FakeWorker $worker ('MIV_VLM_SOURCE_SHA256:' + ('0' * 64))
    Assert-Throws { Assert-MivVlmWorkerIdentity $testRoot } 'Missing/stale/duplicate'
    Write-FakeWorker $worker ('MIV_VLM_SOURCE_SHA256:' + $hash)
    Assert-MivVlmWorkerIdentity $testRoot
    & powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'vlm-worker-identity.ps1') -ValidateRepo $testRoot
    Assert-True ($LASTEXITCODE -eq 0) 'CLI rejected matching PE'
    # This is the exact skip/reassembly entry: it must still reject stale PE.
    & (Join-Path $PSScriptRoot 'build-vlm-worker.ps1') -RepoRoot $testRoot -SkipBuild
    Write-FakeWorker $worker ((('MIV_VLM_SOURCE_SHA256:' + $hash) + "`n") * 2)
    Assert-Throws { Assert-MivVlmWorkerIdentity $testRoot } 'Missing/stale/duplicate'
    Assert-Throws { & (Join-Path $PSScriptRoot 'build-vlm-worker.ps1') -RepoRoot $testRoot -SkipBuild } 'Missing/stale/duplicate'
    Write-FakeWorker $worker ('MIV_VLM_SOURCE_SHA256:' + $hash)
    $prompt = Join-Path $source 'src\prompt.h'
    [System.IO.File]::WriteAllText($prompt, "fixture src/prompt.h`n")
    Assert-True ((Get-MivVlmWorkerSourceHash $source) -eq $hash) 'CRLF differs from LF'
    [System.IO.File]::AppendAllText($prompt, 'changed')
    Assert-Throws { Assert-MivVlmWorkerIdentity $testRoot } 'Missing/stale/duplicate'

    $currentHash = Get-MivVlmWorkerSourceHash $source
    Write-FakeWorker $worker ('MIV_VLM_SOURCE_SHA256:' + $currentHash)
    # Matching worker reuse must return before even looking for native tools.
    function global:cmake { throw 'Fake CMake must not be invoked for reuse.' }
    & (Join-Path $PSScriptRoot 'build-vlm-worker.ps1') -RepoRoot $testRoot
    Assert-MivVlmWorkerIdentity $testRoot
    Remove-Item Function:\cmake
    $savedPath = $env:PATH
    try {
        $env:PATH = $testRoot
        Assert-True (-not (Get-Command cmake -ErrorAction SilentlyContinue)) 'CMake is unexpectedly available in no-tools fixture'
        & (Join-Path $PSScriptRoot 'build-vlm-worker.ps1') -RepoRoot $testRoot
    } finally { $env:PATH = $savedPath }
    $global:MivVlmTestCmakeMarker = Join-Path $testRoot 'cmake-called.txt'
    $global:MivVlmTestBuiltImage = Join-Path $testRoot 'built.exe'
    $global:MivVlmTestWorkerDestination = $worker
    $global:MivVlmTestPublish = $true
    Write-FakeWorker $global:MivVlmTestBuiltImage ('MIV_VLM_SOURCE_SHA256:' + $currentHash)
    New-Item -ItemType Directory -Path (Join-Path $testRoot 'vendor\llama.cpp') -Force | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $testRoot 'vendor\llama.cpp\CMakeLists.txt'), 'fixture')
    function global:cmake {
        param([Parameter(ValueFromRemainingArguments = $true)] [object[]] $CmakeArguments)
        [System.IO.File]::AppendAllText($global:MivVlmTestCmakeMarker, "called`n")
        if ($CmakeArguments -contains '--build' -and $global:MivVlmTestPublish) {
            [System.IO.File]::Copy($global:MivVlmTestBuiltImage, $global:MivVlmTestWorkerDestination, $true)
        }
        $global:LASTEXITCODE = 0
    }
    Write-FakeWorker $worker ('MIV_VLM_SOURCE_SHA256:' + ('0' * 64))
    Assert-Throws { & (Join-Path $PSScriptRoot 'build-vlm-worker.ps1') -RepoRoot $testRoot -SkipBuild } 'Missing/stale/duplicate'
    Assert-True (-not (Test-Path -LiteralPath $global:MivVlmTestCmakeMarker)) 'SkipBuild invoked CMake'
    & (Join-Path $PSScriptRoot 'build-vlm-worker.ps1') -RepoRoot $testRoot
    Assert-True (([System.IO.File]::ReadAllLines($global:MivVlmTestCmakeMarker)).Count -eq 2) 'Stale worker did not configure/build once'
    Remove-Item -LiteralPath $worker
    $global:MivVlmTestPublish = $false
    Assert-Throws { & (Join-Path $PSScriptRoot 'build-vlm-worker.ps1') -RepoRoot $testRoot } 'Vendor worker is missing'
    Assert-True (-not (Test-Path -LiteralPath $worker)) 'Failed build published a worker'
    $global:MivVlmTestPublish = $true
    & (Join-Path $PSScriptRoot 'build-vlm-worker.ps1') -RepoRoot $testRoot
    Assert-MivVlmWorkerIdentity $testRoot

    $beforeForcedBuild = ([System.IO.File]::ReadAllLines($global:MivVlmTestCmakeMarker)).Count
    & (Join-Path $PSScriptRoot 'build-vlm-worker.ps1') -RepoRoot $testRoot -Rebuild
    Assert-True (([System.IO.File]::ReadAllLines($global:MivVlmTestCmakeMarker)).Count -eq $beforeForcedBuild + 2) 'Rebuild did not force native configure/build'
    Assert-Throws { & (Join-Path $PSScriptRoot 'build-vlm-worker.ps1') -RepoRoot $testRoot -SkipBuild -Rebuild } 'cannot be combined'

    # Never publish a stale backup; preserve an already-valid vendor worker.
    $backup = Join-Path $testRoot 'backup.exe'
    Write-FakeWorker $backup ('MIV_VLM_SOURCE_SHA256:' + ('0' * 64))
    Restore-MivVlmWorkerBackup -RepoRoot $testRoot -BackupPath $backup
    Assert-MivVlmWorkerIdentity $testRoot
    Remove-Item -LiteralPath $worker
    Assert-Throws { Restore-MivVlmWorkerBackup -RepoRoot $testRoot -BackupPath $backup } 'Missing/stale/duplicate'
    Assert-True (-not (Test-Path -LiteralPath $worker)) 'Stale backup was published'
    Assert-True (@(Get-ChildItem -LiteralPath (Split-Path -Parent $worker) -Filter '.restore-*').Count -eq 0) 'Rejected candidate remains'
    Assert-Throws { Restore-MivVlmWorkerBackup -RepoRoot $testRoot -BackupPath (Join-Path $testRoot 'missing-backup.exe') } 'Backup missing'
    Write-FakeWorker $backup ('MIV_VLM_SOURCE_SHA256:' + $currentHash)
    Restore-MivVlmWorkerBackup -RepoRoot $testRoot -BackupPath $backup
    Assert-MivVlmWorkerIdentity $testRoot

    . (Join-Path $PSScriptRoot 'setup-llama-cpp.ps1')
    function global:Invoke-WebRequest { throw 'Network command was called with an unset checksum.' }
    $newRepo = Join-Path $testRoot 'must-not-be-created'
    Assert-Throws { Install-MivPinnedLlamaSource -RepoRoot $newRepo } 'SHA-256 is not configured'
    Assert-True (-not (Test-Path -LiteralPath $newRepo)) 'Unset checksum mutated directories'
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    Add-Type -AssemblyName System.IO.Compression
    function Write-TestSourceZip([string] $Path, [object[]] $Entries) {
        $archive = [System.IO.Compression.ZipFile]::Open($Path, [System.IO.Compression.ZipArchiveMode]::Create)
        try {
            foreach ($fixture in $Entries) {
                $entry = $archive.CreateEntry($fixture.Name)
                if ($fixture.ContainsKey('Attributes')) { $entry.ExternalAttributes = $fixture.Attributes }
                $stream = $entry.Open()
                try { $bytes = [System.Text.Encoding]::UTF8.GetBytes('fixture'); $stream.Write($bytes, 0, $bytes.Length) }
                finally { $stream.Dispose() }
            }
        } finally { $archive.Dispose() }
    }
    $commit = 'dde879bc4f954b809892fd0d660515f42536394a'
    $prefix = "llama.cpp-$commit/"
    $goodArchive = Join-Path $testRoot 'good.zip'
    Write-TestSourceZip $goodArchive @(
        @{Name=$prefix+'CMakeLists.txt'}, @{Name=$prefix+'LICENSE'},
        @{Name=$prefix+'ggml/CMakeLists.txt'}, @{Name=$prefix+'tools/mtmd/CMakeLists.txt'})
    $expanded = Join-Path $testRoot 'expanded'
    New-Item -ItemType Directory -Path $expanded | Out-Null
    Expand-MivPinnedLlamaArchive $goodArchive $expanded $commit
    Assert-True (Test-Path -LiteralPath (Join-Path $expanded 'LICENSE') -PathType Leaf) 'Safe archive was not extracted'
    $badIndex = 0
    foreach ($entry in @(
        @{Name=$prefix+'../outside.txt'}, @{Name=$prefix+'C:/outside.txt'},
        @{Name=$prefix+'dir\outside.txt'}, @{Name=$prefix+'NUL.txt'},
        @{Name='wrong-root/CMakeLists.txt'}, @{Name=$prefix+'link'; Attributes=-1610612736})) {
        $badArchive = Join-Path $testRoot ("bad-$badIndex.zip")
        Write-TestSourceZip $badArchive @($entry)
        Assert-Throws { Expand-MivPinnedLlamaArchive $badArchive (Join-Path $testRoot "bad-output-$badIndex") $commit } 'Unsafe archive'
        $badIndex++
    }
    $duplicate = Join-Path $testRoot 'duplicate.zip'
    Write-TestSourceZip $duplicate @(@{Name=$prefix+'duplicate'}, @{Name=$prefix+'DUPLICATE'})
    Assert-Throws { Expand-MivPinnedLlamaArchive $duplicate (Join-Path $testRoot 'duplicate-output') $commit } 'Duplicate archive'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $testRoot 'outside.txt'))) 'Archive escaped staging'

    # Private fixture pin and mocked transport: prove hash verification precedes
    # extraction/publication. The production script exposes no pin/URL override.
    $global:MivVlmTestArchive = $goodArchive
    $global:MivVlmTestNetworkMarker = Join-Path $testRoot 'fake-download.txt'
    function global:Invoke-WebRequest {
        param([string] $Uri, [string] $OutFile, [switch] $UseBasicParsing)
        if ($Uri -cne 'https://github.com/ggml-org/llama.cpp/archive/dde879bc4f954b809892fd0d660515f42536394a.zip') { throw 'Unexpected archive URI.' }
        [System.IO.File]::AppendAllText($global:MivVlmTestNetworkMarker, "fixture download`n")
        [System.IO.File]::Copy($global:MivVlmTestArchive, $OutFile)
    }
    $fixtureRepo = Join-Path $testRoot 'source-install'
    New-Item -ItemType Directory -Path $fixtureRepo | Out-Null
    $script:MivLlamaArchiveSha256 = '0' * 64
    Assert-Throws { Install-MivPinnedLlamaSource $fixtureRepo } 'SHA-256 mismatch'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $fixtureRepo 'vendor\llama.cpp'))) 'Wrong archive hash was published'
    $script:MivLlamaArchiveSha256 = (Get-FileHash -LiteralPath $goodArchive -Algorithm SHA256).Hash.ToLowerInvariant()
    Install-MivPinnedLlamaSource $fixtureRepo
    $installedLicense = Join-Path $fixtureRepo 'vendor\llama.cpp\LICENSE'
    Assert-True (Test-Path -LiteralPath $installedLicense) 'Verified archive was not published'
    Assert-Throws { Install-MivPinnedLlamaSource $fixtureRepo } 'already exists'
    Assert-True ((Get-Content -LiteralPath $installedLicense -Raw -Encoding UTF8) -eq 'fixture') 'Existing source tree changed'
    $script:MivLlamaArchiveSha256 = ''

    foreach ($name in @('build-release.ps1', 'build-portable.ps1', 'build-vlm-worker.ps1', 'vlm-worker-identity.ps1', 'test-vlm-worker-identity.ps1', 'setup-llama-cpp.ps1')) {
        $path = Join-Path $PSScriptRoot $name
        $tokens = $null; $errors = $null
        $null = [System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$errors)
        Assert-True ($errors.Count -eq 0) "Parse failure: $name"
        Assert-True (-not ([System.IO.File]::ReadAllText($path) -match '[^\x00-\x7f]')) "Non-ASCII script: $name"
    }
    $release = Get-Content -Raw -Encoding UTF8 (Join-Path $PSScriptRoot 'build-release.ps1')
    $portable = Get-Content -Raw -Encoding UTF8 (Join-Path $PSScriptRoot 'build-portable.ps1')
    $bootstrap = Get-Content -Raw -Encoding UTF8 (Join-Path $PSScriptRoot 'bootstrap-vendor.sh')
    $setup = Get-Content -Raw -Encoding UTF8 (Join-Path $PSScriptRoot 'setup-llama-cpp.ps1')
    Assert-True ($bootstrap.Contains('-RestoreBackupRepo "$REPO_ROOT"')) 'Bootstrap omits verified backup restoration'
    Assert-True ($setup.Contains('$script:MivLlamaArchiveSha256 = ' + "''")) 'Production archive checksum placeholder is no longer unset'
    Assert-True ($release.IndexOf('Assert-MivVlmWorkerIdentity -RepoRoot $repoRoot') -lt $release.IndexOf('Invoke-MivSign -Files $vendorEmbedTargets')) 'Release identity must precede signing'
    Assert-True ($release.Contains("'vendor\vlm-worker\mimageviewer-vlm.exe',")) 'Release signing omits worker'
    Assert-True ($portable.Contains('-SkipBuild:$SkipBuild')) 'Portable SkipBuild bypasses worker gate'
    Assert-True ($portable.Contains("src = 'VLM-WORKER-LICENSE.txt'")) 'Portable package omits native-worker notices'
    Assert-True ($portable.IndexOf('Assert-MivVlmWorkerIdentity -RepoRoot $repoRoot') -lt $portable.IndexOf('$copies = @(')) 'Portable identity must precede copying'
    Assert-True ($portable.Substring($portable.IndexOf('$portablePe = @(')).Contains("'mimageviewer-vlm.exe',")) 'Portable signing omits worker'
    Write-Host '[vlm-identity-test] PASS'
} finally {
    Remove-Item Function:\cmake -ErrorAction SilentlyContinue
    Remove-Item Function:\Invoke-WebRequest -ErrorAction SilentlyContinue
    foreach ($name in @('MivVlmTestCmakeMarker','MivVlmTestBuiltImage','MivVlmTestWorkerDestination','MivVlmTestPublish','MivVlmTestArchive','MivVlmTestNetworkMarker')) {
        Remove-Variable -Scope Global -Name $name -ErrorAction SilentlyContinue
    }
    $resolved = [System.IO.Path]::GetFullPath($testRoot).TrimEnd('\')
    Assert-True ($resolved.StartsWith($parent + '\', [System.StringComparison]::OrdinalIgnoreCase)) 'Cleanup escaped target'
    $null = @(Get-MivTreeFiles -Path $resolved)
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
