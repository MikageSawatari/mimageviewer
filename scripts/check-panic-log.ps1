# Fail the release while a recorded crash has not been dispositioned.
#
# panic.log accumulates across sessions and is never rotated at startup, so a
# crash from days ago is still there. It was, and nobody looked: two ownership
# panics sat in it for ten days while the release was prepared.
#
# This script lists every panic record and compares it against the
# dispositions checked in at docs/panic-acknowledged.tsv. Anything not listed
# there fails, so a crash has to be fixed, filed, or explained before shipping.
# It reads only; it never launches the application or writes to its data.
#
# Usage (from the repository root):
#   .\scripts\check-panic-log.ps1              # gate: exit 1 on anything new
#   .\scripts\check-panic-log.ps1 -List        # show every record, exit 0
#   .\scripts\check-panic-log.ps1 -LogDir <p>  # another profile's logs
[CmdletBinding()]
param(
    [string]$LogDir = '',
    [string]$AckFile = '',
    [switch]$List
)

$ErrorActionPreference = 'Stop'

if ([string]::IsNullOrEmpty($AckFile)) {
    # $PSScriptRoot is empty in some hosts (dot-sourcing, -Command, some agents),
    # and a parameter default is bound before the body runs, so resolve it here
    # and fall back to the command path and then the working directory.
    $root = $PSScriptRoot
    if ([string]::IsNullOrEmpty($root) -and -not [string]::IsNullOrEmpty($PSCommandPath)) {
        $root = Split-Path -Parent $PSCommandPath
    }
    if ([string]::IsNullOrEmpty($root)) {
        $AckFile = Join-Path (Get-Location).Path 'docs\panic-acknowledged.tsv'
    } else {
        $AckFile = Join-Path $root '..\docs\panic-acknowledged.tsv'
    }
}
if ([string]::IsNullOrEmpty($LogDir)) {
    $LogDir = Join-Path $env:APPDATA 'mimageviewer\logs'
}

function ConvertFrom-FileTimeIntervals {
    param([string]$Intervals)
    # The log stores SystemTime as 100ns intervals since 1601-01-01 UTC.
    $value = [double]$Intervals
    $unix = $value / 1e7 - 11644473600
    return [DateTimeOffset]::FromUnixTimeSeconds([long]$unix).LocalDateTime
}

function Get-PanicFingerprint {
    param([string]$Location, [string]$Payload)
    # Ids, addresses and counts differ between occurrences of the same defect.
    $normalized = [regex]::Replace($Payload, '\d+', '#')
    return "$Location`t$normalized"
}

function Read-PanicRecords {
    param([string]$Dir)
    $records = @()
    # The writer rotates panic.log to panic.log.bak at 4 MiB, so read the
    # older generation first to keep the list chronological.
    foreach ($name in @('panic.log.bak', 'panic.log')) {
        $path = Join-Path $Dir $name
        if (-not (Test-Path -LiteralPath $path)) { continue }
        foreach ($line in [System.IO.File]::ReadLines($path)) {
            $m = [regex]::Match(
                $line,
                '^\[SystemTime \{ intervals: (?<iv>\d+) \}\] PANIC at (?<loc>\S+): (?<msg>.*)$')
            if (-not $m.Success) { continue }
            $records += [pscustomobject]@{
                When        = ConvertFrom-FileTimeIntervals $m.Groups['iv'].Value
                Location    = $m.Groups['loc'].Value
                Payload     = $m.Groups['msg'].Value
                Fingerprint = Get-PanicFingerprint $m.Groups['loc'].Value $m.Groups['msg'].Value
                Source      = $name
            }
        }
    }
    return $records
}

function Read-Acknowledged {
    param([string]$Path)
    $ack = @{}
    if (-not (Test-Path -LiteralPath $Path)) { return $ack }
    foreach ($line in [System.IO.File]::ReadLines($Path)) {
        if ($line.Trim().Length -eq 0) { continue }
        if ($line.StartsWith('#')) { continue }
        $parts = $line -split "`t"
        if ($parts.Count -lt 3) { continue }
        # location <TAB> normalized-payload <TAB> disposition [<TAB> note]
        $ack["$($parts[0])`t$($parts[1])"] = $parts[2]
    }
    return $ack
}

if (-not (Test-Path -LiteralPath $LogDir)) {
    Write-Host "panic log directory not found: $LogDir"
    Write-Host 'Nothing to check. Pass -LogDir if the profile lives elsewhere.'
    exit 0
}

$records = Read-PanicRecords -Dir $LogDir
if ($records.Count -eq 0) {
    Write-Host "no panic records in $LogDir"
    exit 0
}

$groups = $records | Group-Object -Property Fingerprint | ForEach-Object {
    $sorted = $_.Group | Sort-Object -Property When
    [pscustomobject]@{
        Fingerprint = $_.Name
        Count       = $_.Count
        First       = $sorted[0].When
        Last        = $sorted[-1].When
        Location    = $sorted[-1].Location
        Payload     = $sorted[-1].Payload
    }
} | Sort-Object -Property Last

if ($List) {
    Write-Host "panic records in ${LogDir}: $($records.Count) in $($groups.Count) distinct kinds"
    foreach ($g in $groups) {
        Write-Host ''
        Write-Host ("  {0}  x{1}" -f $g.Last.ToString('yyyy-MM-dd HH:mm:ss'), $g.Count)
        Write-Host ("    at {0}" -f $g.Location)
        Write-Host ("    {0}" -f $g.Payload)
    }
    exit 0
}

$ack = Read-Acknowledged -Path $AckFile
$new = @($groups | Where-Object { -not $ack.ContainsKey($_.Fingerprint) })

Write-Host "panic records in ${LogDir}: $($records.Count) in $($groups.Count) distinct kinds"
Write-Host "dispositions on file: $($ack.Count) ($AckFile)"

if ($new.Count -eq 0) {
    Write-Host 'OK: every recorded panic has a disposition.'
    exit 0
}

Write-Host ''
Write-Host "UNDISPOSITIONED PANICS: $($new.Count)"
foreach ($g in $new) {
    Write-Host ''
    Write-Host ("  last {0}  x{1}  (first {2})" -f `
        $g.Last.ToString('yyyy-MM-dd HH:mm:ss'), $g.Count, $g.First.ToString('yyyy-MM-dd HH:mm:ss'))
    Write-Host ("    at {0}" -f $g.Location)
    Write-Host ("    {0}" -f $g.Payload)
    Write-Host '    add this line to the disposition file to clear it:'
    Write-Host ("      {0}`t<disposition>`t<note>" -f $g.Fingerprint)
}
Write-Host ''
Write-Host "Fix it, file it, or explain it in $AckFile before shipping."
exit 1
