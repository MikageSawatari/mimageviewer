# Package the single extension source for Chrome and Edge without changing it.
# OutputDirectory must stay below this worktree's target/browser-extension.
[CmdletBinding()]
param(
    [string] $OutputDirectory = 'target/browser-extension'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.IO.Compression

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$sourceRoot = Join-Path $repoRoot 'browser-extension'
$outputRoot = Join-Path $repoRoot 'target/browser-extension'
$outputPath = if ([IO.Path]::IsPathRooted($OutputDirectory)) {
    [IO.Path]::GetFullPath($OutputDirectory)
} else {
    [IO.Path]::GetFullPath((Join-Path $repoRoot $OutputDirectory))
}
if (-not ($outputPath.Equals($outputRoot, [StringComparison]::OrdinalIgnoreCase) -or
    $outputPath.StartsWith($outputRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase))) {
    throw 'OutputDirectory must be inside this worktree target/browser-extension.'
}

function Assert-NoReparsePoint([string] $Path) {
    $current = $Path
    while ($current -and -not $current.Equals($repoRoot, [StringComparison]::OrdinalIgnoreCase)) {
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Reparse points are not allowed: $current"
            }
        }
        $current = [IO.Path]::GetDirectoryName($current)
    }
    if (-not $current) { throw 'Path must remain inside the worktree.' }
}

function Get-ExtensionId([string] $Key) {
    $der = [Convert]::FromBase64String($Key)
    if ($der.Length -eq 0) { throw 'An extension public key is empty.' }
    $sha = [Security.Cryptography.SHA256]::Create()
    try { $digest = $sha.ComputeHash($der) } finally { $sha.Dispose() }
    $id = New-Object Text.StringBuilder
    for ($i = 0; $i -lt 16; $i++) {
        [void] $id.Append([char](97 + ($digest[$i] -shr 4)))
        [void] $id.Append([char](97 + ($digest[$i] -band 15)))
    }
    return $id.ToString()
}

$entries = @(
    'manifest.json',
    'worker.js',
    'context.js',
    '_locales/ja/messages.json',
    '_locales/en/messages.json',
    'icons/icon16.png',
    'icons/icon32.png',
    'icons/icon48.png',
    'icons/icon128.png'
)
$utf8 = New-Object Text.UTF8Encoding($false, $true)
Assert-NoReparsePoint (Join-Path $sourceRoot 'store-keys.json')
$stores = [IO.File]::ReadAllText((Join-Path $sourceRoot 'store-keys.json'), $utf8) | ConvertFrom-Json
foreach ($store in @('chrome', 'edge')) {
    $record = $stores.$store
    if ($record.id -cnotmatch '^[a-p]{32}$' -or (Get-ExtensionId $record.key) -cne $record.id) {
        throw "The $store public key does not match its extension ID."
    }
}

# Snapshot only allowlisted files once; all packages use the same source bytes.
$contents = @{}
foreach ($name in $entries) {
    $path = Join-Path $sourceRoot $name
    Assert-NoReparsePoint $path
    $contents[$name] = [IO.File]::ReadAllBytes($path)
}
$manifestText = $utf8.GetString($contents['manifest.json'])
$manifest = $manifestText | ConvertFrom-Json
if ($manifest.key -cne $stores.chrome.key) {
    throw 'The source manifest must contain the Chrome public key.'
}
if ($manifest.version -cnotmatch '^\d+(\.\d+){0,3}$') {
    throw 'The source manifest needs a numeric extension version.'
}
# Preserve all source formatting and bytes except the generated Edge key value.
$keyMatches = [regex]::Matches($manifestText, '"key"\s*:\s*"(?<value>[A-Za-z0-9+/=]+)"')
if ($keyMatches.Count -ne 1 -or $keyMatches[0].Groups['value'].Value -cne $stores.chrome.key) {
    throw 'The source manifest must have exactly one unescaped public key field.'
}
$keyGroup = $keyMatches[0].Groups['value']
$edgeManifest = $manifestText.Substring(0, $keyGroup.Index) + $stores.edge.key +
    $manifestText.Substring($keyGroup.Index + $keyGroup.Length)

# Remove only the standalone key line, retaining all other formatting and bytes.
# Require a trailing comma: an unsupported layout must fail rather than rewrite JSON.
$keyLines = [regex]::Matches($edgeManifest, '(?m)^[ \t]*"key"[ \t]*:[ \t]*"(?<value>[A-Za-z0-9+/=]+)"[ \t]*,[ \t]*(?:\r?\n|\z)')
if ($keyLines.Count -ne 1 -or $keyLines[0].Groups['value'].Value -cne $stores.edge.key) {
    throw 'The Edge manifest must have exactly one standalone key line with a trailing comma.'
}
$keyLine = $keyLines[0]
$edgeStoreManifest = $edgeManifest.Remove($keyLine.Index, $keyLine.Length)
$edgeFields = $edgeManifest | ConvertFrom-Json
$edgeStoreFields = $edgeStoreManifest | ConvertFrom-Json
if ($null -ne $edgeStoreFields.PSObject.Properties['key']) {
    throw 'The Edge store manifest must not contain a key field.'
}
$edgeFields.PSObject.Properties.Remove('key')
if (($edgeFields | ConvertTo-Json -Depth 100 -Compress) -cne
    ($edgeStoreFields | ConvertTo-Json -Depth 100 -Compress)) {
    throw 'The Edge store manifest changed fields other than key.'
}

$packages = @(
    [pscustomobject] @{ Name = 'chrome'; Suffix = ''; ManifestBytes = $contents['manifest.json'] },
    [pscustomobject] @{ Name = 'edge'; Suffix = ''; ManifestBytes = $utf8.GetBytes($edgeManifest) },
    [pscustomobject] @{ Name = 'edge'; Suffix = '-store'; ManifestBytes = $utf8.GetBytes($edgeStoreManifest) }
)
Assert-NoReparsePoint $outputPath
[void] [IO.Directory]::CreateDirectory($outputPath)
$fixedTime = [DateTimeOffset]::new(1980, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
foreach ($package in $packages) {
    $zipPath = Join-Path $outputPath ("mimageviewer-browser-extension-{0}-v{1}{2}.zip" -f $package.Name, $manifest.version, $package.Suffix)
    Assert-NoReparsePoint $zipPath
    $file = [IO.File]::Open($zipPath, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try {
        $zip = [IO.Compression.ZipArchive]::new($file, [IO.Compression.ZipArchiveMode]::Create, $true)
        try {
            foreach ($name in $entries) {
                $entry = $zip.CreateEntry($name, [IO.Compression.CompressionLevel]::Optimal)
                $entry.LastWriteTime = $fixedTime
                $entry.ExternalAttributes = 0
                $bytes = if ($name -eq 'manifest.json') {
                    $package.ManifestBytes
                } else { $contents[$name] }
                $stream = $entry.Open()
                try { $stream.Write($bytes, 0, $bytes.Length) } finally { $stream.Dispose() }
            }
        } finally { $zip.Dispose() }
    } finally { $file.Dispose() }
    Write-Output $zipPath
}
