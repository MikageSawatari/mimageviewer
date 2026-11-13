# Upload the mikage.to site files that this repository owns (htdocs/) over SSH.
#
# By default mimageviewer/ (recursive, into /home/mikagewww/htdocs/mimageviewer)
# and the site-root sitemap.xml are uploaded, matching the user's manual routine.
# The other site-root files kept in htdocs/ (robots.txt, index.html) were never
# updated by hand; they are only compared and reported. Pass -IncludeRootFiles
# to upload them as well.
# Everything else under the document root belongs to other sites and is never
# touched.
#
# Default is a dry run: it compares SHA-256 of local and remote files and lists
# what would be uploaded. Pass -Apply to upload the new / changed files.
# Remote-only files are reported but never deleted.
#
# Usage:
#   PS> .\scripts\deploy-site.ps1            # dry run
#   PS> .\scripts\deploy-site.ps1 -Apply     # upload, then re-check remote hashes
#
# The default account is mikage@mikage.to (owner of the site files; key set up
# by the user 2026-11-02). Remote-only files (old downloads, old manuals) are
# counted; -ShowRemoteOnly lists them.
#
# After -Apply, check the public URLs with:
#   PS> python scripts\verify-site.py htdocs https://www.mikage.to
#
# Requirements: Windows OpenSSH (ssh / scp) and tar.exe (bsdtar) on PATH.
# The SSH account must own (or be able to write) the managed files.

param(
    [switch]$Apply,
    [switch]$ShowRemoteOnly,
    [switch]$IncludeRootFiles,
    [string]$Target = 'mikage@mikage.to',
    [string]$RemoteRoot = '/home/mikagewww/htdocs'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$localRoot = Join-Path $repo 'htdocs'
$rootFiles = @('robots.txt', 'index.html')
$managed = @('mimageviewer', 'sitemap.xml')
if ($IncludeRootFiles) { $managed += $rootFiles }
$sshOpts = @('-o', 'BatchMode=yes', '-o', 'ConnectTimeout=20')

function Invoke-Remote([string]$cmd) {
    $out = & ssh @sshOpts $Target $cmd
    if ($LASTEXITCODE -ne 0) { throw "ssh failed (exit $LASTEXITCODE): $cmd" }
    return $out
}

function Get-RemoteHashes {
    $list = ($managed | ForEach-Object { "'$_'" }) -join ' '
    $lines = Invoke-Remote "cd '$RemoteRoot' && find $list -type f -exec sha256sum {} + 2>/dev/null; true"
    $map = @{}
    foreach ($l in $lines) {
        if ($l -match '^([0-9a-f]{64})  (.+)$') { $map[$Matches[2]] = $Matches[1] }
    }
    return $map
}

# Local hashes, keyed by forward-slash relative path.
$local = @{}
foreach ($m in $managed) {
    $p = Join-Path $localRoot $m
    if (Test-Path $p -PathType Leaf) {
        $local[$m] = (Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLower()
    } elseif (Test-Path $p -PathType Container) {
        Get-ChildItem -LiteralPath $p -Recurse -File -Force | ForEach-Object {
            $rel = $_.FullName.Substring($localRoot.Length + 1).Replace('\', '/')
            $local[$rel] = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLower()
        }
    }
}

$remote = Get-RemoteHashes
if (-not $IncludeRootFiles) {
    $saved = $managed
    $managed = $rootFiles
    $rootRemote = Get-RemoteHashes
    $managed = $saved
    foreach ($f in $rootFiles) {
        $lp = Join-Path $localRoot $f
        if (-not (Test-Path $lp -PathType Leaf)) { continue }
        $lh = (Get-FileHash -LiteralPath $lp -Algorithm SHA256).Hash.ToLower()
        if ($rootRemote[$f] -ne $lh) {
            Write-Host "[site] NOTE: site-root $f differs from htdocs/$f (not uploaded; use -IncludeRootFiles)"
        }
    }
}
$changed = @($local.Keys | Where-Object { $remote[$_] -ne $local[$_] } | Sort-Object)
$remoteOnly = @($remote.Keys | Where-Object { -not $local.ContainsKey($_) } | Sort-Object)

Write-Host "[site] local files : $($local.Count)"
Write-Host "[site] remote files: $($remote.Count)"
Write-Host "[site] to upload   : $($changed.Count)"
foreach ($c in ($changed | Select-Object -First 60)) {
    $tag = if ($remote.ContainsKey($c)) { 'changed' } else { 'new    ' }
    Write-Host "  $tag $c"
}
if ($changed.Count -gt 60) { Write-Host "  ... and $($changed.Count - 60) more" }
if ($remoteOnly.Count -gt 0) {
    Write-Host "[site] remote-only (not deleted): $($remoteOnly.Count) (list with -ShowRemoteOnly)"
    if ($ShowRemoteOnly) { foreach ($r in $remoteOnly) { Write-Host "  remote-only $r" } }
}

if (-not $Apply) {
    Write-Host '[site] dry run. Re-run with -Apply to upload.'
    return
}
if ($changed.Count -eq 0) {
    Write-Host '[site] nothing to upload.'
    return
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$tmpDir = Join-Path $env:TEMP "miv-site-$stamp"
New-Item -ItemType Directory -Force $tmpDir | Out-Null
$listFile = Join-Path $tmpDir 'files.txt'
$tarFile = Join-Path $tmpDir 'site.tar'
# bsdtar reads -T in the ANSI code page; a UTF-8 list breaks non-ASCII names.
# The archive itself stores UTF-8 (pax) names, which GNU tar on the server reads.
[System.IO.File]::WriteAllLines($listFile, [string[]]$changed, [System.Text.Encoding]::Default)
& tar.exe -C $localRoot -cf $tarFile -T $listFile
if ($LASTEXITCODE -ne 0) { throw "tar failed (exit $LASTEXITCODE)" }

$remoteTar = "/tmp/miv-site-$stamp.tar"
& scp @sshOpts -q $tarFile "${Target}:$remoteTar"
if ($LASTEXITCODE -ne 0) { throw "scp failed (exit $LASTEXITCODE)" }
# --overwrite writes into existing files instead of unlinking them first: the site root
# directory belongs to another account, so the unlink of a root file (sitemap.xml) fails.
# GNU tar refuses --overwrite together with --no-overwrite-dir. The archive holds only the
# changed files (no directory members), so existing directories' metadata is not touched.
Invoke-Remote "tar -xf '$remoteTar' -C '$RemoteRoot' --no-same-owner --overwrite; rc=`$?; rm -f '$remoteTar'; exit `$rc" | Out-Null
Remove-Item -Recurse -Force $tmpDir

$after = Get-RemoteHashes
$stillDiff = @($changed | Where-Object { $after[$_] -ne $local[$_] })
if ($stillDiff.Count -gt 0) {
    Write-Host "[site] FAILED: $($stillDiff.Count) file(s) still differ on the server:"
    $stillDiff | ForEach-Object { Write-Host "  $_" }
    exit 1
}
Write-Host "[site] OK. uploaded $($changed.Count) file(s); remote hashes match."
Write-Host '[site] next: python scripts\verify-site.py htdocs https://www.mikage.to'
