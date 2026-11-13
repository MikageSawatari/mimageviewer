param([Parameter(Mandatory = $true)] [string] $SourceRoot, [switch] $VerifyVendor)
$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath($SourceRoot).TrimEnd('\')
$names = New-Object 'System.Collections.Generic.List[string]'
$names.Add('CMakeLists.txt')
$names.Add('tools/source-identity.ps1')
$manifestPath = Join-Path $root 'whisper-source-manifest.json'
if (Test-Path -LiteralPath $manifestPath -PathType Leaf) { $names.Add('whisper-source-manifest.json') }
elseif ($VerifyVendor) { throw 'Pinned whisper source manifest is missing.' }
foreach ($directory in @('src', 'tests')) {
    foreach ($file in Get-ChildItem -LiteralPath (Join-Path $root $directory) -File) {
        if ($file.Extension -in @('.h', '.cpp') -or ($directory -eq 'tests' -and $file.Extension -eq '.ps1')) {
            $names.Add("$directory/$($file.Name)")
        }
    }
}
$names.Sort([System.StringComparer]::Ordinal)
$aggregate = New-Object System.Text.StringBuilder
function Get-NormalizedHash([string] $Path) {
    $ancestor = [System.IO.Path]::GetFullPath($Path)
    while ($ancestor) {
        if (((Get-Item -LiteralPath $ancestor -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Reparse-point identity path forbidden: $ancestor"
        }
        $ancestor = [System.IO.Path]::GetDirectoryName($ancestor)
    }
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
if (Test-Path -LiteralPath $manifestPath -PathType Leaf) {
    # ソースなしのexe再利用も可能。ソースがある場合は固定manifestと全件照合する。
    $manifest = [System.IO.File]::ReadAllText($manifestPath) | ConvertFrom-Json
    if ($manifest.version -cne 'v1.9.5' -or
        $manifest.archive_sha256 -cne '7a7a73071939f16bc028ad2f713f16f26e37afb6cc155d0f19e3d65705d2b1d9') {
        throw 'Invalid pinned whisper source manifest.'
    }
    $vendor = [System.IO.Path]::GetFullPath((Join-Path $root '../../vendor/whisper.cpp')).TrimEnd('\')
    if ((Test-Path -LiteralPath $vendor) -or $VerifyVendor) {
        $expected = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::Ordinal)
        foreach ($entry in $manifest.files.PSObject.Properties) {
            $name = $entry.Name
            if ($name -notmatch '^(CMakeLists\.txt|LICENSE|(src|include|cmake)/[A-Za-z0-9_./-]+)$' -or
                $name.Split('/') -contains '..' -or $entry.Value -cnotmatch '^[0-9a-f]{64}$') {
                throw 'Unsafe pinned whisper manifest entry.'
            }
            [void]$expected.Add($name)
            $path = Join-Path $vendor $name
            if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or (Get-NormalizedHash $path) -cne $entry.Value) {
                throw "Pinned whisper source mismatch: $name. Restore the fixed v1.9.5 source."
            }
        }
        foreach ($directory in @('src', 'include', 'cmake')) {
            # Check directory links before recursive enumeration.
            $stack = New-Object 'System.Collections.Generic.Stack[string]'
            $stack.Push((Join-Path $vendor $directory))
            while ($stack.Count) {
                $current = $stack.Pop()
                foreach ($item in Get-ChildItem -LiteralPath $current -Force) {
                    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                        throw "Reparse-point identity path forbidden: $($item.FullName)"
                    }
                    if ($item.PSIsContainer) { $stack.Push($item.FullName); continue }
                    $relative = $item.FullName.Substring($vendor.Length + 1).Replace('\', '/')
                    if (-not $expected.Contains($relative)) { throw "Unpinned whisper source file: $relative" }
                }
            }
        }
    }
}
foreach ($name in $names) {
    $path = Join-Path $root $name
    $ancestor = [System.IO.Path]::GetFullPath($path)
    while ($ancestor) {
        if (((Get-Item -LiteralPath $ancestor -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Reparse-point identity path forbidden: $ancestor"
        }
        $ancestor = [System.IO.Path]::GetDirectoryName($ancestor)
    }
    $bytes = [System.IO.File]::ReadAllBytes($path)
    $normalized = New-Object System.IO.MemoryStream
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        # Normalize only CRLF bytes; retain BOM, lone CR, and all other bytes.
        for ($index = 0; $index -lt $bytes.Length; ++$index) {
            if ($bytes[$index] -eq 13 -and $index + 1 -lt $bytes.Length -and $bytes[$index + 1] -eq 10) { continue }
            $normalized.WriteByte($bytes[$index])
        }
        $hash = ([BitConverter]::ToString($sha.ComputeHash($normalized.ToArray()))).Replace('-', '').ToLowerInvariant()
        [void]$aggregate.Append($name).Append(':').Append($hash).Append("`n")
    } finally { $sha.Dispose(); $normalized.Dispose() }
}
$sha = [System.Security.Cryptography.SHA256]::Create()
try {
    ([BitConverter]::ToString($sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($aggregate.ToString())))).Replace('-', '').ToLowerInvariant()
} finally { $sha.Dispose() }
