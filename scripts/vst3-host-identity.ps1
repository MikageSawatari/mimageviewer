# Source identity gate for the embedded VST3 host. No host is ever launched.
param([string] $ValidateRepo, [string] $HashSourceRoot)
. (Join-Path $PSScriptRoot 'sign-files.ps1')

function Assert-MivVst3IdentityAncestors {
    param([string] $Path)
    $current = [System.IO.Path]::GetFullPath($Path)
    while ($current) {
        $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "[vst3-identity] Reparse-point identity path forbidden: $current"
        }
        $current = [System.IO.Path]::GetDirectoryName($current)
    }
}

# Byte normalization only: remove CR immediately followed by LF. Preserve every
# other byte, including lone CR, BOM, NUL and non-UTF-8 bytes. CMake and the gate
# both call this implementation; never decode/re-encode source text.
function Get-MivVst3NormalizedFileHash {
    param([Parameter(Mandatory = $true)] [string] $Path)
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $normalized = New-Object System.IO.MemoryStream
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        for ($index = 0; $index -lt $bytes.Length; ++$index) {
            if ($bytes[$index] -eq 13 -and $index + 1 -lt $bytes.Length -and $bytes[$index + 1] -eq 10) { continue }
            $normalized.WriteByte($bytes[$index])
        }
        return ([BitConverter]::ToString($sha.ComputeHash($normalized.ToArray()))).Replace('-', '').ToLowerInvariant()
    } finally { $sha.Dispose(); $normalized.Dispose() }
}

function Get-MivVst3HostSourceHash {
    param([Parameter(Mandatory = $true)] [string] $SourceRoot)
    $root = [System.IO.Path]::GetFullPath($SourceRoot).TrimEnd('\')
    Assert-MivVst3IdentityAncestors -Path $root
    $names = New-Object 'System.Collections.Generic.List[string]'
    $files = @(Get-MivTreeFiles -Path (Join-Path $root 'CMakeLists.txt'))
    foreach ($directory in @('include', 'src', 'tests')) {
        $path = Join-Path $root $directory
        if (Test-Path -LiteralPath $path -PathType Container) { $files += Get-MivTreeFiles -Path $path }
    }
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($root.Length + 1).Replace('\', '/')
        if ($relative -eq 'CMakeLists.txt' -or
            ($relative.StartsWith('include/') -and $file.Extension.ToLowerInvariant() -eq '.h') -or
            ($relative.StartsWith('src/') -and $file.Extension.ToLowerInvariant() -in @('.cpp', '.h')) -or
            ($relative.StartsWith('tests/') -and $file.Extension.ToLowerInvariant() -eq '.cpp')) {
            if ($relative -match '[^\x20-\x7e]') { throw "[vst3-identity] Source path must be ASCII: $relative" }
            $names.Add($relative)
        }
    }
    if (-not $names.Contains('CMakeLists.txt')) { throw '[vst3-identity] CMakeLists.txt is missing' }
    $names.Sort([System.StringComparer]::Ordinal)
    $aggregate = New-Object System.Text.StringBuilder
    foreach ($name in $names) {
        $hash = Get-MivVst3NormalizedFileHash -Path (Join-Path $root $name)
        [void]$aggregate.Append($name).Append(':').Append($hash).Append("`n")
    }
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($aggregate.ToString())
        return ([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
    } finally { $sha.Dispose() }
}

function Assert-MivVst3HostIdentity {
    param([Parameter(Mandatory = $true)] [string] $RepoRoot)
    $source = Join-Path $RepoRoot 'crates\vst3-host'
    $hostPath = Join-Path $RepoRoot 'vendor\vst3-host\mimageviewer-vst3-host.exe'
    $recover = 'Restore vendor/vst3sdk with scripts/setup-vst3-sdk.sh, then rebuild crates/vst3-host with CMake. Only the current source-matching vendor host may be reused; APPDATA/other host caches are never imported.'
    if (-not (Test-Path -LiteralPath $hostPath -PathType Leaf)) {
        throw "[vst3-identity] Vendor host is missing: $hostPath. $recover"
    }
    Assert-MivVst3IdentityAncestors -Path $hostPath
    $item = Get-Item -LiteralPath $hostPath -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        -not (Test-MivPeFile $hostPath)) { throw "[vst3-identity] Vendor host must be a regular PE: $hostPath. $recover" }
    $expected = Get-MivVst3HostSourceHash -SourceRoot $source
    $image = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($hostPath))
    $markers = [regex]::Matches($image, 'MIV_VST3_HOST_SOURCE_SHA256:([0-9a-f]{64})(?![0-9a-f])')
    if ($markers.Count -ne 1 -or $markers[0].Groups[1].Value -ne $expected) {
        throw "[vst3-identity] Missing/stale source identity in $hostPath (expected $expected). $recover"
    }
    Write-Host "[vst3-identity] current vendor host verified: $expected"
}

if ($HashSourceRoot) {
    $ErrorActionPreference = 'Stop'
    Get-MivVst3HostSourceHash -SourceRoot $HashSourceRoot
} elseif ($ValidateRepo) {
    $ErrorActionPreference = 'Stop'
    Assert-MivVst3HostIdentity -RepoRoot $ValidateRepo
}
