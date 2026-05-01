# scripts/collect-ffmpeg-lgpl-info.ps1
#
# Generate an auditable LGPL compliance report from the bundled FFmpeg DLLs.
# Run before each release and archive the output with release materials.
#
# Usage:
#   .\scripts\collect-ffmpeg-lgpl-info.ps1
#   .\scripts\collect-ffmpeg-lgpl-info.ps1 -OutFile docs\ffmpeg-lgpl-current-report.txt
#
# Reference: docs/ffmpeg-lgpl-source-distribution.md

param(
    [string]$VendorDir = "vendor/ffmpeg",
    [string]$OutFile
)

$ErrorActionPreference = "Stop"

function Get-RepoRoot {
    $scriptDir = Split-Path -Parent $MyInvocation.ScriptName
    if ($scriptDir) {
        return (Resolve-Path (Join-Path $scriptDir "..")).Path
    }
    return (Get-Location).Path
}

function Read-Latin1String([string]$Path) {
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    return [System.Text.Encoding]::GetEncoding("iso-8859-1").GetString($bytes)
}

function Find-ConfigureStrings([string[]]$DllPaths) {
    $results = @()
    foreach ($dll in $DllPaths) {
        $text = Read-Latin1String $dll
        $matches = [regex]::Matches($text, "--prefix=/ffbuild/prefix[\s\S]{0,12000}?--extra-version=[^\x00\s]+")
        foreach ($match in $matches) {
            $configure = $match.Value -replace "\x00", " "
            $configure = $configure -replace "\s+", " "
            $results += [pscustomobject]@{
                Dll = (Split-Path -Leaf $dll)
                Configure = $configure.Trim()
            }
        }
    }
    return $results
}

function Find-LicenseStrings([string[]]$DllPaths) {
    $results = @()
    foreach ($dll in $DllPaths) {
        $text = Read-Latin1String $dll
        $matches = [regex]::Matches($text, "libav[a-z]+ license: [^\x00]+")
        foreach ($match in $matches) {
            $value = ($match.Value -replace "\x00.*$", "").Trim()
            $results += [pscustomobject]@{
                Dll = (Split-Path -Leaf $dll)
                License = $value
            }
        }
    }
    return $results
}

function Get-LibFlags([string[]]$ConfigureStrings) {
    $flags = New-Object System.Collections.Generic.SortedSet[string]
    foreach ($configure in $ConfigureStrings) {
        foreach ($match in [regex]::Matches($configure, "--(?:enable|disable)-lib[0-9A-Za-z._+-]+")) {
            [void]$flags.Add($match.Value)
        }
    }
    return @($flags)
}

$repoRoot = Get-RepoRoot
$vendorPath = Join-Path $repoRoot $VendorDir
$binPath = Join-Path $vendorPath "bin"
$versionPath = Join-Path $vendorPath "VERSION"
$licensePath = Join-Path $vendorPath "LICENSE.txt"

if (-not (Test-Path $binPath)) {
    throw "FFmpeg bin directory not found: $binPath"
}

$dlls = Get-ChildItem -Path $binPath -Filter "*.dll" | Sort-Object Name | ForEach-Object { $_.FullName }
$version = if (Test-Path $versionPath) { (Get-Content $versionPath -Raw).Trim() } else { "(missing)" }
$licenseHeader = if (Test-Path $licensePath) {
    (Get-Content $licensePath -TotalCount 5) -join " "
} else {
    "(missing)"
}

$configureRows = Find-ConfigureStrings $dlls
$licenseRows = Find-LicenseStrings $dlls
$configureStrings = @($configureRows | ForEach-Object { $_.Configure } | Select-Object -Unique)
$libFlags = Get-LibFlags $configureStrings

if ($configureRows.Count -eq 0) {
    Write-Error "No FFmpeg configure string found in DLLs. BtbN build format may have changed; investigate before release."
    exit 1
}

$configureText = $configureStrings -join " "
$expectedFlags = @(
    "--enable-version3",
    "--enable-libsvtav1",
    "--disable-libx264",
    "--disable-libx265"
)
$missingExpectedFlags = @(
    $expectedFlags | Where-Object { $configureText -notmatch [regex]::Escape($_) }
)
if ($missingExpectedFlags.Count -gt 0) {
    Write-Warning "Missing expected FFmpeg configure flags: $($missingExpectedFlags -join ', ')"
}

$gplLeakFlags = @(
    "--enable-libx264",
    "--enable-libx265"
)
$gplLeaks = @(
    $gplLeakFlags | Where-Object { $configureText -match [regex]::Escape($_) }
)
if ($gplLeaks.Count -gt 0) {
    Write-Error "GPL contamination detected: $($gplLeaks -join ', '). Use a BtbN *-lgpl-shared-* asset only."
    exit 1
}

$lines = New-Object System.Collections.Generic.List[string]
$lines.Add("=== Bundled FFmpeg LGPL Report ===")
$lines.Add("Generated: $(Get-Date -Format o)")
$lines.Add("Vendor dir: $VendorDir")
$lines.Add("BtbN asset: $version")
$lines.Add("License file header: $licenseHeader")
$lines.Add("")
$lines.Add("=== DLL License Strings ===")
if ($licenseRows.Count -eq 0) {
    $lines.Add("(none found)")
} else {
    foreach ($row in $licenseRows) {
        $lines.Add("$($row.Dll): $($row.License)")
    }
}
$lines.Add("")
$lines.Add("=== Configure Strings ===")
if ($configureRows.Count -eq 0) {
    $lines.Add("(none found)")
} else {
    foreach ($row in $configureRows) {
        $lines.Add("[$($row.Dll)]")
        $lines.Add($row.Configure)
        $lines.Add("")
    }
}
$lines.Add("=== External Library Flags ===")
if ($libFlags.Count -eq 0) {
    $lines.Add("(none found)")
} else {
    foreach ($flag in $libFlags) {
        $lines.Add($flag)
    }
}
$lines.Add("")
$lines.Add("=== Expected Flag Check ===")
if ($missingExpectedFlags.Count -eq 0) {
    $lines.Add("OK: $($expectedFlags -join ', ')")
} else {
    $lines.Add("Missing expected flags: $($missingExpectedFlags -join ', ')")
}
$lines.Add("GPL leak check: OK, no forbidden GPL encoder flags found.")
$lines.Add("")
$lines.Add("=== Source Distribution Checklist ===")
$lines.Add("[ ] Confirm this is an LGPL shared build, not GPL or nonfree.")
$lines.Add("[ ] Keep vendor/ffmpeg/LICENSE.txt with the distribution.")
$lines.Add("[ ] Provide corresponding FFmpeg source or source offer for the exact build.")
$lines.Add("[ ] Provide source/license references for enabled external libraries.")
$lines.Add("[ ] Update docs/ffmpeg-lgpl-source-distribution.md if enabled libraries changed.")
$lines.Add("[ ] Keep software information and installer/readme.txt notices current.")
$lines.Add("")
$lines.Add("Reference docs: docs/ffmpeg-lgpl-source-distribution.md")

$report = $lines -join [Environment]::NewLine

if ($OutFile) {
    $outPath = if ([System.IO.Path]::IsPathRooted($OutFile)) { $OutFile } else { Join-Path $repoRoot $OutFile }
    $outDir = Split-Path -Parent $outPath
    if ($outDir -and -not (Test-Path $outDir)) {
        New-Item -ItemType Directory -Path $outDir -Force | Out-Null
    }
    Set-Content -Path $outPath -Value $report -Encoding UTF8
    Write-Host "Wrote $outPath"
} else {
    Write-Output $report
}
