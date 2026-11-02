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
    foreach ($name in @('build-release.ps1', 'build-portable.ps1', 'build-vlm-worker.ps1', 'vlm-worker-identity.ps1', 'test-vlm-worker-identity.ps1')) {
        $path = Join-Path $PSScriptRoot $name
        $tokens = $null; $errors = $null
        $null = [System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$errors)
        Assert-True ($errors.Count -eq 0) "Parse failure: $name"
        Assert-True (-not ([System.IO.File]::ReadAllText($path) -match '[^\x00-\x7f]')) "Non-ASCII script: $name"
    }
    $release = Get-Content -Raw -Encoding UTF8 (Join-Path $PSScriptRoot 'build-release.ps1')
    $portable = Get-Content -Raw -Encoding UTF8 (Join-Path $PSScriptRoot 'build-portable.ps1')
    Assert-True ($release.IndexOf('Assert-MivVlmWorkerIdentity -RepoRoot $repoRoot') -lt $release.IndexOf('Invoke-MivSign -Files $vendorEmbedTargets')) 'Release identity must precede signing'
    Assert-True ($release.Contains("'vendor\vlm-worker\mimageviewer-vlm.exe',")) 'Release signing omits worker'
    Assert-True ($portable.Contains('-SkipBuild:$SkipBuild')) 'Portable SkipBuild bypasses worker gate'
    Assert-True ($portable.Contains("src = 'VLM-WORKER-LICENSE.txt'")) 'Portable package omits native-worker notices'
    Assert-True ($portable.IndexOf('Assert-MivVlmWorkerIdentity -RepoRoot $repoRoot') -lt $portable.IndexOf('$copies = @(')) 'Portable identity must precede copying'
    Assert-True ($portable.Substring($portable.IndexOf('$portablePe = @(')).Contains("'mimageviewer-vlm.exe',")) 'Portable signing omits worker'
    Write-Host '[vlm-identity-test] PASS'
} finally {
    $resolved = [System.IO.Path]::GetFullPath($testRoot).TrimEnd('\')
    Assert-True ($resolved.StartsWith($parent + '\', [System.StringComparison]::OrdinalIgnoreCase)) 'Cleanup escaped target'
    $null = @(Get-MivTreeFiles -Path $resolved)
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
