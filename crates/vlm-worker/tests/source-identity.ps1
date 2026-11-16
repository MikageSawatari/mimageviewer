# 実モデルもworkerも起動しない。固定ソースの照合とソースなし再利用を検査する。
$ErrorActionPreference = 'Stop'
$repo = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..'))
$testRoot = Join-Path $repo ('target/asr-identity-' + [guid]::NewGuid().ToString('N'))
$source = Join-Path $testRoot 'crates/vlm-worker'
$vendor = Join-Path $testRoot 'vendor/whisper.cpp'
$scriptPath = Join-Path $PSScriptRoot '../tools/source-identity.ps1'
function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw $Message }
}
function Assert-Rejected([scriptblock] $Action) {
    try { & $Action | Out-Null } catch { return }
    throw 'Expected pinned source rejection'
}
New-Item -ItemType Directory -Path $testRoot | Out-Null
try {
    foreach ($name in @('CMakeLists.txt', 'tools/source-identity.ps1', 'src/main.cpp', 'tests/asr.cpp')) {
        $path = Join-Path $source $name
        New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
        [System.IO.File]::WriteAllText($path, 'fixture')
    }
    $manifest = [ordered]@{version='v1.9.5';archive_sha256='7a7a73071939f16bc028ad2f713f16f26e37afb6cc155d0f19e3d65705d2b1d9';files=[ordered]@{}}
    foreach ($name in @('CMakeLists.txt', 'LICENSE', 'src/whisper.cpp', 'include/whisper.h', 'cmake/build.cmake')) {
        $path = Join-Path $vendor $name
        New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
        [System.IO.File]::WriteAllText($path, "fixed`n")
        $manifest.files[$name] = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    $manifestPath = Join-Path $source 'whisper-source-manifest.json'
    [System.IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 5))
    $hash = & $scriptPath -SourceRoot $source -VerifyVendor
    Assert-True ($hash -match '^[0-9a-f]{64}$') 'Invalid identity'
    $cpp = Join-Path $vendor 'src/whisper.cpp'
    [System.IO.File]::WriteAllText($cpp, "fixed`r`n")
    Assert-True ((& $scriptPath -SourceRoot $source -VerifyVendor) -eq $hash) 'CRLF must normalize'
    [System.IO.File]::AppendAllText($cpp, 'changed')
    Assert-Rejected { & $scriptPath -SourceRoot $source }
    [System.IO.File]::WriteAllText($cpp, "fixed`n")
    $extra = Join-Path $vendor 'src/extra.cpp'
    [System.IO.File]::WriteAllText($extra, 'unexpected source')
    Assert-Rejected { & $scriptPath -SourceRoot $source }
    Remove-Item -LiteralPath $extra
    Remove-Item -LiteralPath $cpp
    Assert-Rejected { & $scriptPath -SourceRoot $source }
    [System.IO.File]::WriteAllText($cpp, "fixed`n")
    $hidden = Join-Path $testRoot 'source-offline'
    [System.IO.Directory]::Move($vendor, $hidden)
    Assert-True ((& $scriptPath -SourceRoot $source) -eq $hash) 'Source-less identity must remain reusable'
    Assert-Rejected { & $scriptPath -SourceRoot $source -VerifyVendor }
    [System.IO.File]::AppendAllText((Join-Path $source 'src/main.cpp'), 'ASR changed')
    Assert-True ((& $scriptPath -SourceRoot $source) -ne $hash) 'Worker source change must invalidate identity'
    Write-Host 'Whisper pinned-source identity checks passed.'
} finally {
    $resolved = [System.IO.Path]::GetFullPath($testRoot)
    Assert-True ($resolved.StartsWith($repo + '\target\asr-identity-', [System.StringComparison]::OrdinalIgnoreCase)) 'Unsafe cleanup path'
    foreach ($item in Get-ChildItem -LiteralPath $resolved -Recurse -Force) {
        Assert-True (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) 'Fixture contains reparse point'
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
