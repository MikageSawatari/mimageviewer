# Noninteractive checks for the three extension packages; no source files are written.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.IO.Compression
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$sourceRoot = Join-Path $repoRoot 'browser-extension'
$packageScript = Join-Path $PSScriptRoot 'package-browser-extension.ps1'
$utf8 = New-Object Text.UTF8Encoding($false, $true)
$expectedNames = @('manifest.json', 'worker.js', 'context.js', '_locales/ja/messages.json',
    '_locales/en/messages.json', 'icons/icon16.png', 'icons/icon32.png', 'icons/icon48.png', 'icons/icon128.png')
$permissions = @('contextMenus', 'nativeMessaging', 'storage', 'activeTab', 'scripting', 'declarativeNetRequestWithHostAccess')
$stores = Get-Content -LiteralPath (Join-Path $sourceRoot 'store-keys.json') -Raw -Encoding UTF8 | ConvertFrom-Json

function Assert([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw $Message }
}

function Get-SourceSnapshot {
    $snapshot = @{}
    foreach ($file in Get-ChildItem -LiteralPath $sourceRoot -Recurse -File -Force) {
        $snapshot[$file.FullName] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
    }
    return $snapshot
}

function Assert-SameSet($Actual, $Expected, [string] $Message) {
    $actualValues = @($Actual | Sort-Object -CaseSensitive)
    $expectedValues = @($Expected | Sort-Object -CaseSensitive)
    Assert (($actualValues -join "`n") -ceq ($expectedValues -join "`n")) $Message
}

function Read-Entry($Entry) {
    $stream = $Entry.Open()
    $memory = New-Object IO.MemoryStream
    try {
        $stream.CopyTo($memory)
        return ,$memory.ToArray()
    } finally { $memory.Dispose(); $stream.Dispose() }
}

function Assert-KeyId([string] $Key, [string] $Expected) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { $hash = $sha.ComputeHash([Convert]::FromBase64String($Key)) } finally { $sha.Dispose() }
    $hex = -join ($hash[0..15] | ForEach-Object { $_.ToString('x2') })
    $id = -join ($hex.ToCharArray() | ForEach-Object { [char](97 + [Convert]::ToInt32([string] $_, 16)) })
    Assert ($id -ceq $Expected) 'Packaged public key does not produce the store extension ID.'
}

$before = Get-SourceSnapshot
$scratch = Join-Path $repoRoot ('target/browser-extension/package-test-' + [Guid]::NewGuid().ToString('N'))
$first = @(& $packageScript -OutputDirectory (Join-Path $scratch 'first'))
$second = @(& $packageScript -OutputDirectory (Join-Path $scratch 'second'))
Assert ($first.Count -eq 3 -and $second.Count -eq 3) 'Expected three extension packages per invocation.'
$sourceManifestBytes = [IO.File]::ReadAllBytes((Join-Path $sourceRoot 'manifest.json'))
$sourceManifestText = $utf8.GetString($sourceManifestBytes)
$sourceManifest = $sourceManifestText | ConvertFrom-Json
Assert ($sourceManifest.key -ceq $stores.chrome.key) 'The source manifest must keep the Chrome key.'
$expectedZips = @(
    "mimageviewer-browser-extension-chrome-v$($sourceManifest.version).zip",
    "mimageviewer-browser-extension-edge-v$($sourceManifest.version).zip",
    "mimageviewer-browser-extension-edge-v$($sourceManifest.version)-store.zip"
)
foreach ($paths in @($first, $second)) {
    for ($i = 0; $i -lt $expectedZips.Count; $i++) {
        Assert ((Split-Path -Leaf $paths[$i]) -ceq $expectedZips[$i]) 'Unexpected output ZIP name/order.'
        Assert (Test-Path -LiteralPath $paths[$i] -PathType Leaf) 'An expected ZIP was not created.'
    }
}

for ($i = 0; $i -lt 2; $i++) {
    $store = @('chrome', 'edge')[$i]
    Assert ((Get-FileHash -LiteralPath $first[$i]).Hash -ceq (Get-FileHash -LiteralPath $second[$i]).Hash) "The $store ZIP is not reproducible."
    $file = [IO.File]::OpenRead($first[$i])
    try {
        $zip = [IO.Compression.ZipArchive]::new($file, [IO.Compression.ZipArchiveMode]::Read, $true)
        try {
            Assert (($zip.Entries.FullName -join "`n") -ceq ($expectedNames -join "`n")) 'ZIP entry allowlist/order differs.'
            $textByName = @{}
            foreach ($entry in $zip.Entries) {
                Assert ($entry.LastWriteTime.DateTime -eq [DateTime]::new(1980, 1, 1)) 'ZIP entry timestamp is not fixed.'
                $bytes = Read-Entry $entry
                if ($entry.FullName -ne 'manifest.json' -or $store -eq 'chrome') {
                    $sourceBytes = [IO.File]::ReadAllBytes((Join-Path $sourceRoot $entry.FullName))
                    Assert ([Convert]::ToBase64String($bytes) -ceq [Convert]::ToBase64String($sourceBytes)) "Source bytes changed in $($entry.FullName)."
                }
                if ($entry.FullName.EndsWith('.json')) { $textByName[$entry.FullName] = $utf8.GetString($bytes) }
            }
            $manifest = $textByName['manifest.json'] | ConvertFrom-Json
            Assert ($manifest.key -ceq $stores.$store.key) "Incorrect $store manifest key."
            Assert-KeyId $manifest.key $stores.$store.id
            if ($store -eq 'edge') {
                Assert ($textByName['manifest.json'].Replace($stores.edge.key, $stores.chrome.key) -ceq $sourceManifestText) 'The Edge manifest changed more than its public key.'
            }
            Assert ($manifest.manifest_version -eq 3) 'Expected Manifest V3.'
            Assert-SameSet $manifest.permissions $permissions 'Unexpected required permissions.'
            Assert-SameSet $manifest.optional_host_permissions @('http://*/*', 'https://*/*') 'Unexpected optional host permissions.'
            foreach ($forbidden in @('host_permissions', 'optional_permissions', 'content_scripts')) {
                Assert ($null -eq $manifest.PSObject.Properties[$forbidden]) "Unexpected manifest field: $forbidden"
            }
            Assert ($manifest.background.service_worker -ceq 'worker.js') 'Unexpected service worker.'
            Assert ($manifest.default_locale -ceq 'ja') 'Expected Japanese default locale.'
            foreach ($size in @('16', '32', '48', '128')) {
                Assert ($manifest.icons.$size -ceq "icons/icon$size.png") "Missing icon $size."
            }
            $ja = $textByName['_locales/ja/messages.json'] | ConvertFrom-Json
            $en = $textByName['_locales/en/messages.json'] | ConvertFrom-Json
            Assert-SameSet $ja.PSObject.Properties.Name $en.PSObject.Properties.Name 'Locale message keys differ.'
            foreach ($locale in @($ja, $en)) {
                foreach ($property in $locale.PSObject.Properties) {
                    Assert (-not [string]::IsNullOrWhiteSpace($property.Value.message)) "Empty localized message: $($property.Name)"
                }
                foreach ($match in [regex]::Matches($textByName['manifest.json'], '__MSG_([A-Za-z0-9_]+)__')) {
                    Assert ($null -ne $locale.PSObject.Properties[$match.Groups[1].Value]) 'Manifest references a missing locale message.'
                }
            }
        } finally { $zip.Dispose() }
    } finally { $file.Dispose() }
    Write-Output "PASS: $store key/ID, permissions, locales, allowlist, source bytes, reproducibility"
}

Assert ((Get-FileHash -LiteralPath $first[2]).Hash -ceq (Get-FileHash -LiteralPath $second[2]).Hash) 'The Edge store ZIP is not reproducible.'
$edgeFile = [IO.File]::OpenRead($first[1])
try {
    $edgeZip = [IO.Compression.ZipArchive]::new($edgeFile, [IO.Compression.ZipArchiveMode]::Read, $true)
    try {
        $storeFile = [IO.File]::OpenRead($first[2])
        try {
            $storeZip = [IO.Compression.ZipArchive]::new($storeFile, [IO.Compression.ZipArchiveMode]::Read, $true)
            try {
                Assert (($storeZip.Entries.FullName -join "`n") -ceq ($expectedNames -join "`n")) 'Edge store ZIP entry allowlist/order differs.'
                foreach ($entry in $storeZip.Entries) {
                    $edgeEntry = $edgeZip.GetEntry($entry.FullName)
                    Assert ($entry.LastWriteTime -eq $edgeEntry.LastWriteTime) 'Edge store ZIP entry timestamp differs.'
                    Assert ($entry.ExternalAttributes -eq $edgeEntry.ExternalAttributes) 'Edge store ZIP entry attributes differ.'
                    $bytes = Read-Entry $entry
                    $edgeBytes = Read-Entry $edgeEntry
                    if ($entry.FullName -eq 'manifest.json') {
                        $storeText = $utf8.GetString($bytes)
                        $edgeText = $utf8.GetString($edgeBytes)
                        $storeManifest = $storeText | ConvertFrom-Json
                        $edgeManifest = $edgeText | ConvertFrom-Json
                        Assert ($null -eq $storeManifest.PSObject.Properties['key']) 'The Edge store manifest contains key.'
                        $edgeManifest.PSObject.Properties.Remove('key')
                        Assert (($storeManifest | ConvertTo-Json -Depth 100 -Compress) -ceq
                            ($edgeManifest | ConvertTo-Json -Depth 100 -Compress)) 'Edge store manifest fields differ beyond key.'
                        $keyLines = @($edgeText -split '(?<=\n)' | Where-Object { $_ -cmatch '^[ \t]*"key"[ \t]*:' })
                        Assert ($keyLines.Count -eq 1) 'Expected one key line in the Edge verification manifest.'
                        Assert ($storeText -ceq $edgeText.Replace($keyLines[0], '')) 'Edge store manifest changed bytes outside the key line.'
                    } else {
                        Assert ([Convert]::ToBase64String($bytes) -ceq [Convert]::ToBase64String($edgeBytes)) "Edge store bytes differ in $($entry.FullName)."
                        Assert ($entry.CompressedLength -eq $edgeEntry.CompressedLength) 'Edge store compressed entry length differs.'
                    }
                }
            } finally { $storeZip.Dispose() }
        } finally { $storeFile.Dispose() }
    } finally { $edgeZip.Dispose() }
} finally { $edgeFile.Dispose() }
Write-Output 'PASS: Edge store key absent, remaining fields/bytes, entry metadata, reproducibility'

$rejected = $false
try { & $packageScript -OutputDirectory $repoRoot | Out-Null } catch {
    $rejected = $_.Exception.Message -like 'OutputDirectory must be inside*'
}
Assert $rejected 'Output outside target/browser-extension was not rejected.'
$after = Get-SourceSnapshot
Assert-SameSet $before.Keys $after.Keys 'Packaging changed the source file list.'
foreach ($name in $before.Keys) { Assert ($before[$name] -ceq $after[$name]) "Packaging modified source: $name" }
Write-Output 'PASS: output confinement and complete source tree unchanged'
Write-Output "Test packages: $scratch"
