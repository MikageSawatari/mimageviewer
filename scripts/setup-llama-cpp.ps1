# Pinned llama.cpp source recovery. No models, worker execution, or CMake here.
param([string] $RepoRoot = (Join-Path $PSScriptRoot '..'))
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'vlm-worker-identity.ps1')
$script:MivLlamaCommit = 'dde879bc4f954b809892fd0d660515f42536394a' # b12032
# SHA-256 of the GitHub ZIP for this commit, checked 2026-11-03: its contents
# match the vendor/llama.cpp tree used to build the worker.
$script:MivLlamaArchiveSha256 = 'd705712976fc7a4f6b664b1b29e6c35af4cf256a35fca41c4bb7f9d26d3f9751'

function Expand-MivPinnedLlamaArchive {
    param([string] $Archive, [string] $Destination, [string] $Commit)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    Add-Type -AssemblyName System.IO.Compression
    $root = [System.IO.Path]::GetFullPath($Destination).TrimEnd('\')
    $prefix = "llama.cpp-$Commit/"
    $seen = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    $zip = [System.IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        [long] $total = 0
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName
            if (-not $name.StartsWith($prefix, [System.StringComparison]::Ordinal) -or
                $name.Contains('\') -or (($entry.ExternalAttributes -shr 16) -band 0xf000) -eq 0xa000 -or
                ($entry.ExternalAttributes -band 0x400) -ne 0) {
                throw "[llama-setup] Unsafe archive entry: $name"
            }
            $relative = $name.Substring($prefix.Length).TrimEnd('/')
            if (-not $relative) { continue }
            foreach ($part in $relative.Split('/')) {
                if (-not $part -or $part -in @('.', '..') -or $part -match '[\x00-\x1f<>:"|?*]' -or
                    $part.EndsWith('.') -or $part.EndsWith(' ') -or
                    $part -match '^(?i:CON|PRN|AUX|NUL|CONIN\$|CONOUT\$|CLOCK\$|COM[1-9\u00b9\u00b2\u00b3]|LPT[1-9\u00b9\u00b2\u00b3])(?:\.|$)') {
                    throw "[llama-setup] Unsafe archive path: $name"
                }
            }
            if (-not $seen.Add($relative)) { throw "[llama-setup] Duplicate archive path: $name" }
            $path = [System.IO.Path]::GetFullPath((Join-Path $root $relative))
            if (-not $path.StartsWith($root + '\', [System.StringComparison]::OrdinalIgnoreCase)) {
                throw "[llama-setup] Archive path escaped staging: $name"
            }
            if ($name.EndsWith('/')) {
                New-Item -ItemType Directory -Path $path -Force | Out-Null
                continue
            }
            if ($entry.Length -gt 256MB -or $entry.Length -lt 0 -or $total -gt (2GB - $entry.Length)) {
                throw '[llama-setup] Archive exceeds extraction size limits.'
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
                        if ($count -gt ($entry.Length - $written)) { throw '[llama-setup] Archive length mismatch.' }
                        $outputStream.Write($buffer, 0, $count)
                        $written += $count
                    }
                    if ($written -ne $entry.Length) { throw '[llama-setup] Archive length mismatch.' }
                } finally { $outputStream.Dispose() }
            } finally { $inputStream.Dispose() }
        }
    } finally { $zip.Dispose() }
    foreach ($required in @('CMakeLists.txt', 'LICENSE', 'ggml/CMakeLists.txt', 'tools/mtmd/CMakeLists.txt')) {
        if (-not (Test-Path -LiteralPath (Join-Path $root $required) -PathType Leaf)) {
            throw "[llama-setup] Pinned archive is incomplete: $required"
        }
    }
}

function Install-MivPinnedLlamaSource {
    param([string] $RepoRoot)
    # This MUST precede all directory mutations and network calls. No hash/URL
    # override is accepted by the production entry point.
    if ($script:MivLlamaArchiveSha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw '[llama-setup] Archive SHA-256 is not configured. Verify the pinned GitHub ZIP offline and replace the reviewed checksum placeholder before acquisition. No network or directory changes were made.'
    }
    $vendor = Join-Path ([System.IO.Path]::GetFullPath($RepoRoot)) 'vendor'
    $destination = Join-Path $vendor 'llama.cpp'
    if (Test-Path -LiteralPath $destination) { throw '[llama-setup] vendor/llama.cpp already exists; it will not be replaced.' }
    $ancestor = $vendor
    while (-not (Test-Path -LiteralPath $ancestor)) { $ancestor = Split-Path -Parent $ancestor }
    Assert-MivVlmIdentityAncestors $ancestor
    New-Item -ItemType Directory -Path $vendor -Force | Out-Null
    Assert-MivVlmIdentityAncestors $vendor
    $staging = Join-Path $vendor ('.llama-setup-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $staging | Out-Null
    try {
        $archive = Join-Path $staging 'source.zip'
        $uri = "https://github.com/ggml-org/llama.cpp/archive/$($script:MivLlamaCommit).zip"
        Invoke-WebRequest -Uri $uri -OutFile $archive -UseBasicParsing
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -cne $script:MivLlamaArchiveSha256) {
            throw '[llama-setup] Pinned archive SHA-256 mismatch; nothing was published.'
        }
        $source = Join-Path $staging 'source'
        New-Item -ItemType Directory -Path $source | Out-Null
        Expand-MivPinnedLlamaArchive -Archive $archive -Destination $source -Commit $script:MivLlamaCommit
        # Rename only into an absent destination. Never remove an existing tree.
        [System.IO.Directory]::Move($source, $destination)
        Write-Host "[llama-setup] Published pinned b12032 source: $($script:MivLlamaCommit)"
    } finally {
        $resolved = [System.IO.Path]::GetFullPath($staging)
        if (-not $resolved.StartsWith($vendor + '\.llama-setup-', [System.StringComparison]::OrdinalIgnoreCase)) {
            throw '[llama-setup] Refusing unexpected staging cleanup path.'
        }
        $null = @(Get-MivTreeFiles -Path $resolved)
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}

if ($MyInvocation.InvocationName -ne '.') { Install-MivPinnedLlamaSource -RepoRoot $RepoRoot }
