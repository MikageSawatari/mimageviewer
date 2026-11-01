# Upload the signed installer for the Microsoft Store to Cloudflare R2 and verify it.
#
# The Store downloads the installer from a versioned URL that must answer 200
# without a redirect and must never change after submission. This script
# refuses to overwrite an existing object, uploads, then checks the public URL
# (status, no Location header, size) and the SHA-256 of the served bytes.
#
# Prerequisite: AWS CLI profile "r2-mikage" (see docs/release-checklist.md, Phase 5 step 19).
#
# Usage:
#   .\scripts\upload-store-installer.ps1 -Version 4.11.0
#   .\scripts\upload-store-installer.ps1 -Version 4.11.0 -File installer\Output\mImageViewer_setup.exe
#   .\scripts\upload-store-installer.ps1 -Version 4.11.0 -VerifyOnly

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string] $Version,
    [string] $File = 'installer\Output\mImageViewer_setup.exe',
    [switch] $VerifyOnly
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$profileName = 'r2-mikage'
$bucket = 'mikage-dl'
$key = "mimageviewer/v$Version/mImageViewer_setup.exe"
$url = "https://dl.mikage.jp/$key"

# Windows PowerShell 5.1 turns native stderr into a terminating error under
# ErrorActionPreference=Stop. Run native tools with Continue and check exit codes.
function Invoke-Native {
    param([scriptblock] $Block)
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $Block } finally { $ErrorActionPreference = $saved }
}

$filePath = if ([System.IO.Path]::IsPathRooted($File)) { $File } else { Join-Path $repoRoot $File }
if (-not (Test-Path -LiteralPath $filePath -PathType Leaf)) {
    throw "[store-upload] installer not found: $filePath"
}
$localItem = Get-Item -LiteralPath $filePath
$localHash = (Get-FileHash -LiteralPath $filePath -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Host "[store-upload] local file : $($localItem.FullName)"
Write-Host "[store-upload] local size : $($localItem.Length)"
Write-Host "[store-upload] local sha256: $localHash"
Write-Host "[store-upload] target     : $url"

if (-not $VerifyOnly) {
    # 1. Never overwrite: the Store requires the bytes behind a submitted URL to stay the same.
    Invoke-Native { & aws s3api head-object --profile $profileName --bucket $bucket --key $key 2>$null | Out-Null }
    if ($LASTEXITCODE -eq 0) {
        throw "[store-upload] already exists, refusing to overwrite: $key (use -VerifyOnly to check it)"
    }

    # 2. Upload with the same content type the old server used.
    Invoke-Native { & aws s3 cp $filePath "s3://$bucket/$key" --profile $profileName --content-type application/octet-stream }
    if ($LASTEXITCODE -ne 0) {
        throw "[store-upload] upload failed (exit $LASTEXITCODE)"
    }
}

# 3. Public URL: 200, no redirect, same size.
$headLines = Invoke-Native { & curl.exe -sI $url }
if ($LASTEXITCODE -ne 0) { throw "[store-upload] HEAD request failed (exit $LASTEXITCODE)" }
$status = ($headLines | Select-Object -First 1).Trim()
$location = $headLines | Where-Object { $_ -match '^(?i)location:' }
$lengthLine = $headLines | Where-Object { $_ -match '^(?i)content-length:' } | Select-Object -First 1
$remoteLength = if ($lengthLine) { [int64](($lengthLine -split ':', 2)[1].Trim()) } else { -1 }
Write-Host "[store-upload] status     : $status"
Write-Host "[store-upload] length     : $remoteLength"
if ($status -notmatch '^HTTP/\S+ 200') { throw "[store-upload] expected 200, got: $status" }
if ($location) { throw "[store-upload] redirect is not allowed: $location" }
if ($remoteLength -ne $localItem.Length) {
    throw "[store-upload] size mismatch: remote $remoteLength, local $($localItem.Length)"
}

# 4. Served bytes: same SHA-256 as the local signed installer.
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("store-upload-" + [guid]::NewGuid().ToString('N') + '.exe')
try {
    Invoke-Native { & curl.exe -sf -o $tmp $url }
    if ($LASTEXITCODE -ne 0) { throw "[store-upload] download failed (exit $LASTEXITCODE)" }
    $remoteHash = (Get-FileHash -LiteralPath $tmp -Algorithm SHA256).Hash.ToLowerInvariant()
}
finally {
    if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Force }
}
Write-Host "[store-upload] remote sha256: $remoteHash"
if ($remoteHash -ne $localHash) { throw '[store-upload] sha256 mismatch between local and served file' }

Write-Host "[store-upload] OK. Store package URL: $url"
