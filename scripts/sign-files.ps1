# Code-signing helper for mImageViewer distribution builds (ASCII only).
#
# Dot-source this file to import the signing functions:
#     . (Join-Path $PSScriptRoot 'sign-files.ps1')
#     Assert-MivSignReady
#     Invoke-MivSign -Files @('a.exe','b.dll') -Verify
#
# Signing uses the Certum Open Source Code Signing certificate exposed through
# SimplySign Desktop (the cloud key appears as a virtual smart card in the
# CurrentUser certificate store). SimplySign Desktop MUST be running and logged
# in for a signed build; otherwise Assert-MivSignReady / Invoke-MivSign fail with
# a clear message. See CLAUDE.md "code signing".
#
# Configuration via environment (all optional):
#   MIV_SIGN_SHA1     - certificate thumbprint (overrides subject-name selection).
#                       Use this if the default subject match becomes ambiguous or
#                       after a renewal you want to pin an exact cert.
#   MIV_SIGN_SUBJECT  - certificate subject-name substring (default below). The
#                       subject survives yearly renewals, so this is the default.
#   MIV_SIGN_TS       - RFC3161 timestamp URL (default http://time.certum.pl).
#   MIV_SIGNTOOL      - explicit path to signtool.exe (else auto-detected).
#
# ASCII-only: PowerShell 5.1 reads BOM-less .ps1 as the system ANSI codepage, so
# non-ASCII here can be mis-decoded (see CLAUDE.md encoding policy).

$script:MivSignDefaultSubject = 'Open Source Developer Taku Sano'
$script:MivSignDefaultTimestamp = 'http://time.certum.pl'

function Get-MivSignTool {
    if ($env:MIV_SIGNTOOL) {
        if (Test-Path $env:MIV_SIGNTOOL) { return $env:MIV_SIGNTOOL }
        throw "[sign] MIV_SIGNTOOL is set but does not exist: $($env:MIV_SIGNTOOL)"
    }
    $roots = @(
        'C:\Program Files (x86)\Windows Kits\10\bin',
        'C:\Program Files\Windows Kits\10\bin'
    )
    $found = @()
    foreach ($r in $roots) {
        if (-not (Test-Path $r)) { continue }
        $found += Get-ChildItem -Path $r -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -like '*\x64\signtool.exe' }
    }
    if ($found.Count -eq 0) {
        throw "[sign] signtool.exe (x64) not found under Windows Kits. Install the Windows 10/11 SDK, or set MIV_SIGNTOOL."
    }
    # Highest SDK build wins. Version dirs share the '...\10\bin\10.0.NNNNN.0\...'
    # prefix, so a descending string sort of the full path selects the newest.
    $best = $found | Sort-Object -Property FullName -Descending | Select-Object -First 1
    return $best.FullName
}

function Get-MivSignSelector {
    # Returns the signtool certificate-selection arguments as a string array.
    if ($env:MIV_SIGN_SHA1) { return @('/sha1', $env:MIV_SIGN_SHA1) }
    $subject = if ($env:MIV_SIGN_SUBJECT) { $env:MIV_SIGN_SUBJECT } else { $script:MivSignDefaultSubject }
    return @('/n', $subject)
}

function Assert-MivSignReady {
    # Confirm the signing certificate is present (SimplySign Desktop logged in).
    $selector = Get-MivSignSelector
    if ($selector[0] -eq '/sha1') {
        $cert = Get-ChildItem Cert:\CurrentUser\My -ErrorAction SilentlyContinue |
            Where-Object { $_.Thumbprint -eq $selector[1] }
    } else {
        $cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert -ErrorAction SilentlyContinue |
            Where-Object { $_.Subject -like ("*{0}*" -f $selector[1]) }
    }
    if (-not $cert) {
        throw ("[sign] signing certificate not found in CurrentUser\My (selector: {0}). Start SimplySign Desktop and log in before a signed build." -f ($selector -join ' '))
    }
    Write-Host ("[sign] certificate ready: {0}" -f (@($cert)[0].Subject))
}

function Invoke-MivSign {
    param(
        [Parameter(Mandatory = $true)] [string[]] $Files,
        [switch] $Verify
    )
    $signtool = Get-MivSignTool
    $selector = Get-MivSignSelector
    $ts = if ($env:MIV_SIGN_TS) { $env:MIV_SIGN_TS } else { $script:MivSignDefaultTimestamp }

    foreach ($f in $Files) {
        if (-not (Test-Path $f)) { throw "[sign] file to sign not found: $f" }
        $signArgs = @('sign') + $selector + @('/fd', 'sha256', '/tr', $ts, '/td', 'sha256', $f)
        $ok = $false
        # Timestamping hits Certum's TSA over the network; a signed distribution
        # signs 10+ files, so retry a couple of times to ride out transient TSA
        # hiccups. Re-signing replaces the signature, so a retry is safe.
        for ($attempt = 1; $attempt -le 3; $attempt++) {
            & $signtool @signArgs
            if ($LASTEXITCODE -eq 0) { $ok = $true; break }
            Write-Warning ("[sign] attempt {0}/3 failed for {1} (exit {2}); retrying..." -f $attempt, $f, $LASTEXITCODE)
            Start-Sleep -Seconds 3
        }
        if (-not $ok) { throw "[sign] signing failed after 3 attempts: $f" }
        Write-Host "[sign] signed: $f"
    }

    if ($Verify) {
        foreach ($f in $Files) {
            & $signtool verify /pa $f
            if ($LASTEXITCODE -ne 0) { throw "[sign] verification failed: $f" }
        }
    }
}
