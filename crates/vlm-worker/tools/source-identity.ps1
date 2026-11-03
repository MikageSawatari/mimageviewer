param([Parameter(Mandatory = $true)] [string] $SourceRoot)
$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath($SourceRoot).TrimEnd('\')
$names = New-Object 'System.Collections.Generic.List[string]'
$names.Add('CMakeLists.txt')
$names.Add('tools/source-identity.ps1')
foreach ($directory in @('src', 'tests')) {
    foreach ($file in Get-ChildItem -LiteralPath (Join-Path $root $directory) -File) {
        if ($file.Extension -in @('.h', '.cpp')) { $names.Add("$directory/$($file.Name)") }
    }
}
$names.Sort([System.StringComparer]::Ordinal)
$aggregate = New-Object System.Text.StringBuilder
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
