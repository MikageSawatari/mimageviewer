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
# a clear message. Assert-MivSignReady checks certificate presence, then signs a
# temporary copy of the running PowerShell host (never the original). Separate
# key, timestamp (up to three attempts), and verification steps diagnose failures
# before a build. The temporary directory is always removed. Usually this takes
# seconds, but signtool can stall for minutes; there is no probe time limit.
# See docs/release-operations.md, section 4.
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

function Test-MivPeFile {
    param([Parameter(Mandatory = $true)] [string] $Path)
    # Inspect the DOS/PE headers, not the extension: VST3 DLLs use .vst3.
    $stream = [System.IO.File]::OpenRead($Path)
    $reader = New-Object System.IO.BinaryReader($stream)
    try {
        if ($stream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5a4d) { return $false }
        $stream.Position = 0x3c
        $offset = $reader.ReadUInt32()
        if ($offset -lt 64 -or $offset -gt ($stream.Length - 24)) { return $false }
        $stream.Position = $offset
        return $reader.ReadUInt32() -eq 0x00004550
    } finally {
        $reader.Dispose()
    }
}

function Get-MivTreeFiles {
    param([Parameter(Mandatory = $true)] [string] $Path)
    # Do not follow junctions/symlinks, including during staging cleanup.
    $pending = New-Object 'System.Collections.Generic.Stack[string]'
    $pending.Push([System.IO.Path]::GetFullPath($Path))
    while ($pending.Count -gt 0) {
        $item = Get-Item -LiteralPath $pending.Pop() -Force -ErrorAction Stop
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "[pe] reparse point is not allowed in artifact tree: $($item.FullName)"
        }
        if ($item.PSIsContainer) {
            foreach ($child in Get-ChildItem -LiteralPath $item.FullName -Force) {
                $pending.Push($child.FullName)
            }
        } else {
            $item
        }
    }
}

function Get-MivPeFiles {
    param([Parameter(Mandatory = $true)] [string[]] $Paths)
    foreach ($path in $Paths) {
        Get-MivTreeFiles -Path $path | Where-Object { Test-MivPeFile -Path $_.FullName }
    }
}

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

function Assert-MivSignCertificatePresent {
    # The public certificate can remain present even when the key is unusable.
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

function Invoke-MivSignProbeCommand {
    param([string] $SignTool, [string[]] $Arguments)
    # Windows PowerShell 5.1 turns redirected native stderr into ErrorRecords.
    # Keep that conversion non-terminating only for this invocation, then render
    # both streams as text. The native exit code, not stderr, decides success.
    $savedPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        # Native calls update the global automatic variable; do not shadow it.
        $global:LASTEXITCODE = $null
        $output = @(& $SignTool @Arguments 2>&1)
        $exitCode = $global:LASTEXITCODE
    } finally {
        $ErrorActionPreference = $savedPreference
    }
    $diagnostic = ($output | ForEach-Object { $_.ToString() }) -join [Environment]::NewLine
    if ($null -eq $exitCode) { throw "[sign] signtool could not be started: $diagnostic" }
    return [pscustomobject]@{ ExitCode = $exitCode; Output = $diagnostic }
}

function Invoke-MivSignProbe {
    $signtool = Get-MivSignTool
    $selector = Get-MivSignSelector
    $ts = if ($env:MIV_SIGN_TS) { $env:MIV_SIGN_TS } else { $script:MivSignDefaultTimestamp }
    $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\')
    $probeDir = Join-Path $tempRoot ('miv-sign-probe-' + [guid]::NewGuid().ToString('N'))
    # Validate the absolute cleanup target before creating anything. A new GUID
    # directory is owned only by this probe, including on a failed signing step.
    if (-not ([System.IO.Path]::GetFullPath($probeDir).StartsWith(($tempRoot + '\'), [System.StringComparison]::OrdinalIgnoreCase))) {
        throw "[sign] probe directory escaped the OS temp directory: $probeDir"
    }
    New-Item -ItemType Directory -Path $probeDir -ErrorAction Stop | Out-Null
    try {
        $hostExe = (Get-Process -Id $PID).Path
        $probeFile = Join-Path $probeDir ([System.IO.Path]::GetFileName($hostExe))
        Copy-Item -LiteralPath $hostExe -Destination $probeFile -ErrorAction Stop
        if (-not (Test-MivPeFile -Path $probeFile)) { throw "[sign] probe copy is not a PE file: $probeFile" }

        $result = Invoke-MivSignProbeCommand -SignTool $signtool -Arguments (@('sign') + $selector + @('/fd', 'sha256', $probeFile))
        if ($result.ExitCode -ne 0) {
            throw ("[sign] signing key is not usable (exit {0}). Possible causes: SimplySign session expired / not logged in, SCardSvr virtual card or CSP not registered, or a restricted or non-elevated host. Log in to SimplySign Desktop again and check the signing host/service. See docs/release-operations.md section 4.{1}{2}" -f $result.ExitCode, [Environment]::NewLine, $result.Output)
        }
        Write-Host '[sign] probe key signing passed'

        for ($attempt = 1; $attempt -le 3; $attempt++) {
            $result = Invoke-MivSignProbeCommand -SignTool $signtool -Arguments @('timestamp', '/tr', $ts, '/td', 'sha256', $probeFile)
            if ($result.ExitCode -eq 0) { break }
            if ($attempt -lt 3) { Start-Sleep -Seconds 3 }
        }
        if ($result.ExitCode -ne 0) {
            throw ("[sign] timestamp server not reachable: {0} (exit {1}, after 3 attempts). Check the network / TSA service; set MIV_SIGN_TS to change the URL.{2}{3}" -f $ts, $result.ExitCode, [Environment]::NewLine, $result.Output)
        }
        Write-Host '[sign] probe timestamp passed'

        $result = Invoke-MivSignProbeCommand -SignTool $signtool -Arguments @('verify', '/pa', $probeFile)
        if ($result.ExitCode -ne 0) {
            throw ("[sign] probe verification failed (exit {0}): {1}{2}{3}" -f $result.ExitCode, $probeFile, [Environment]::NewLine, $result.Output)
        }
        Write-Host '[sign] probe verification passed'
    } finally {
        Remove-Item -LiteralPath $probeDir -Recurse -Force -ErrorAction Stop
    }
}

function Assert-MivSignReady {
    Assert-MivSignCertificatePresent
    Invoke-MivSignProbe
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
        if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { throw "[sign] file to sign not found: $f" }
        if (-not (Test-MivPeFile -Path $f)) { throw "[sign] refusing to sign a non-PE file: $f" }
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
