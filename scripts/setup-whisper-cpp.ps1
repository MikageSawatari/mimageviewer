# Pinned whisper.cpp source recovery. No models, worker execution, or CMake here.
param([string] $RepoRoot = (Join-Path $PSScriptRoot '..'))
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'vlm-worker-identity.ps1')
$script:MivWhisperVersion = 'v1.9.5' # v1.9.5
# SHA-256 of the fixed v1.9.5 GitHub tag ZIP, verified during ASR T0.
$script:MivWhisperArchiveSha256 = '7a7a73071939f16bc028ad2f713f16f26e37afb6cc155d0f19e3d65705d2b1d9'

function Expand-MivPinnedWhisperArchive {
    param([string] $Archive, [string] $Destination, [string] $Version)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    Add-Type -AssemblyName System.IO.Compression
    $root = [System.IO.Path]::GetFullPath($Destination).TrimEnd('\')
    $prefix = "whisper.cpp-$($Version.TrimStart('v'))/"
    $seen = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    $zip = [System.IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        [long] $total = 0
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName
            if (-not $name.StartsWith($prefix, [System.StringComparison]::Ordinal) -or
                $name.Contains('\') -or (($entry.ExternalAttributes -shr 16) -band 0xf000) -eq 0xa000 -or
                ($entry.ExternalAttributes -band 0x400) -ne 0) {
                throw "[whisper-setup] Unsafe archive entry: $name"
            }
            $relative = $name.Substring($prefix.Length).TrimEnd('/')
            if (-not $relative) { continue }
            foreach ($part in $relative.Split('/')) {
                if (-not $part -or $part -in @('.', '..') -or $part -match '[\x00-\x1f<>:"|?*]' -or
                    $part.EndsWith('.') -or $part.EndsWith(' ') -or
                    $part -match '^(?i:CON|PRN|AUX|NUL|CONIN\$|CONOUT\$|CLOCK\$|COM[1-9\u00b9\u00b2\u00b3]|LPT[1-9\u00b9\u00b2\u00b3])(?:\.|$)') {
                    throw "[whisper-setup] Unsafe archive path: $name"
                }
            }
            if (-not $seen.Add($relative)) { throw "[whisper-setup] Duplicate archive path: $name" }
            $path = [System.IO.Path]::GetFullPath((Join-Path $root $relative))
            if (-not $path.StartsWith($root + '\', [System.StringComparison]::OrdinalIgnoreCase)) {
                throw "[whisper-setup] Archive path escaped staging: $name"
            }
            if ($name.EndsWith('/')) {
                New-Item -ItemType Directory -Path $path -Force | Out-Null
                continue
            }
            if ($entry.Length -gt 256MB -or $entry.Length -lt 0 -or $total -gt (2GB - $entry.Length)) {
                throw '[whisper-setup] Archive exceeds extraction size limits.'
            }
            $total += $entry.Length
            New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
            $inputStream = $entry.Open()
            try {
                $outputStream = [System.IO.File]::Open($path, [System.IO.FileMode]::CreateNew)
                try {
                    $buffer = New-Object byte[] 65536
                    [long] $written = 0
                    while (($count = $inputStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                        if ($count -gt ($entry.Length - $written)) { throw '[whisper-setup] Archive length mismatch.' }
                        $outputStream.Write($buffer, 0, $count)
                        $written += $count
                    }
                    if ($written -ne $entry.Length) { throw '[whisper-setup] Archive length mismatch.' }
                } finally { $outputStream.Dispose() }
            } finally { $inputStream.Dispose() }
        }
    } finally { $zip.Dispose() }
    foreach ($required in @('CMakeLists.txt', 'LICENSE', 'ggml/CMakeLists.txt', 'include/whisper.h', 'src/whisper.cpp')) {
        if (-not (Test-Path -LiteralPath (Join-Path $root $required) -PathType Leaf)) {
            throw "[whisper-setup] Pinned archive is incomplete: $required"
        }
    }
}

function Install-MivPinnedWhisperSource {
    param([string] $RepoRoot)
    # This MUST precede all directory mutations and network calls. No hash/URL
    # override is accepted by the production entry point.
    if ($script:MivWhisperArchiveSha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw '[whisper-setup] Archive SHA-256 is not configured. Verify the pinned GitHub ZIP offline and replace the reviewed checksum placeholder before acquisition. No network or directory changes were made.'
    }
    $vendor = Join-Path ([System.IO.Path]::GetFullPath($RepoRoot)) 'vendor'
    $destination = Join-Path $vendor 'whisper.cpp'
    if (Test-Path -LiteralPath $destination) { throw '[whisper-setup] vendor/whisper.cpp already exists; it will not be replaced.' }
    $ancestor = $vendor
    while (-not (Test-Path -LiteralPath $ancestor)) { $ancestor = Split-Path -Parent $ancestor }
    Assert-MivVlmIdentityAncestors $ancestor
    New-Item -ItemType Directory -Path $vendor -Force | Out-Null
    Assert-MivVlmIdentityAncestors $vendor
    $staging = Join-Path $vendor ('.whisper-setup-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $staging | Out-Null
    try {
        $archive = Join-Path $staging 'source.zip'
        $uri = "https://github.com/ggml-org/whisper.cpp/archive/refs/tags/$($script:MivWhisperVersion).zip"
        Invoke-WebRequest -Uri $uri -OutFile $archive -UseBasicParsing
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -cne $script:MivWhisperArchiveSha256) {
            throw '[whisper-setup] Pinned archive SHA-256 mismatch; nothing was published.'
        }
        $source = Join-Path $staging 'source'
        New-Item -ItemType Directory -Path $source | Out-Null
        Expand-MivPinnedWhisperArchive -Archive $archive -Destination $source -Version $script:MivWhisperVersion
        # Rename only into an absent destination. Never remove an existing tree.
        [System.IO.Directory]::Move($source, $destination)
        Write-Host "[whisper-setup] Published pinned whisper.cpp source: $($script:MivWhisperVersion)"
    } finally {
        $resolved = [System.IO.Path]::GetFullPath($staging)
        if (-not $resolved.StartsWith($vendor + '\.whisper-setup-', [System.StringComparison]::OrdinalIgnoreCase)) {
            throw '[whisper-setup] Refusing unexpected staging cleanup path.'
        }
        $null = @(Get-MivTreeFiles -Path $resolved)
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}

if ($MyInvocation.InvocationName -ne '.') { Install-MivPinnedWhisperSource -RepoRoot $RepoRoot }
