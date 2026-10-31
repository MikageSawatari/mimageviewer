# Query dependency versions without installing or updating dependencies.
# The only project file written is target/dependency-updates/YYYY-MM-DD.md.
# Cargo may refresh its registry metadata cache during update --dry-run.
# Usage: .\scripts\check-dependency-updates.ps1
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$rows = New-Object 'System.Collections.Generic.List[object]'

function Add-Row {
    param([string]$Name, [string]$Current, [string]$Latest, [string]$Notes)
    $rows.Add([pscustomobject]@{ Name = $Name; Current = $Current; Latest = $Latest; Notes = $Notes })
}

function Read-Version {
    param([string]$Path)
    try { return (Get-Content -LiteralPath (Join-Path $repoRoot $Path) -Encoding UTF8 -Raw).Trim() }
    catch { return 'unknown (local file missing or unreadable)' }
}

function Invoke-QueryCommand {
    param([string]$Name, [string[]]$Arguments)
    # Capture stderr as data, including on Windows PowerShell 5.1. No shell
    # command strings are evaluated; arguments are passed directly to the tool.
    $command = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $command) { return [pscustomobject]@{ Code = -1; Text = "$Name not found" } }
    $ErrorActionPreference = 'Continue'
    try {
        $output = @(& $command.Source @Arguments 2>&1)
        return [pscustomobject]@{ Code = $LASTEXITCODE; Text = ($output | ForEach-Object { "$_" }) -join "`n" }
    } catch {
        return [pscustomobject]@{ Code = -1; Text = 'command failed' }
    }
}

function Get-Json {
    param([string]$Url)
    Invoke-RestMethod -Uri $Url -Headers @{ 'User-Agent' = 'mImageViewer-dependency-check'; Accept = 'application/json' } -TimeoutSec 20
}

function Get-GitHubJson {
    param([string]$Endpoint)
    if (Get-Command gh -CommandType Application -ErrorAction SilentlyContinue) {
        $result = Invoke-QueryCommand 'gh' @('api', $Endpoint)
        if ($result.Code -ne 0) { throw 'GitHub query failed' }
        $parsed = $result.Text | ConvertFrom-Json
    } else {
        $parsed = Get-Json "https://api.github.com/$Endpoint"
    }
    # PS5.1 ConvertFrom-Json/Invoke-RestMethod emit an array as one pipeline
    # object. Return the value to enumerate its elements at this boundary.
    return $parsed
}

function Get-LatestVst3Tag {
    param([object[]]$Tags)
    $versionedTags = @($Tags | ForEach-Object {
        if ($_.name -match '^v(\d+\.\d+\.\d+)_build_(\d+)$') {
            [pscustomobject]@{ Name = $_.name; Version = [version]$Matches[1]; Build = [int]$Matches[2] }
        }
    } | Sort-Object -Property Version, Build -Descending)
    if (-not $versionedTags) { throw 'No versioned SDK tags' }
    return $versionedTags[0].Name
}

function Test-GitBashPath {
    param([string]$Path)
    if (-not $Path) { return $false }
    try {
        $fullPath = [IO.Path]::GetFullPath($Path.Trim())
        # Never choose the WSL launcher from Windows, even when it is first
        # on PATH. Account for System32/SysWOW64 path casing and separators.
        if ($env:WINDIR) {
            $windowsRoot = [IO.Path]::GetFullPath($env:WINDIR).TrimEnd('\')
            if ($fullPath.StartsWith("$windowsRoot\", [StringComparison]::OrdinalIgnoreCase)) { return $false }
        }
        return ([IO.Path]::GetFileName($fullPath) -ieq 'bash.exe') -and
            (Test-Path -LiteralPath $fullPath -PathType Leaf)
    } catch { return $false }
}

function Find-GitBash {
    # Prefer Git for Windows even when its directories are absent from PATH.
    $knownPaths = @('C:\Program Files\Git\bin\bash.exe')
    if ($env:ProgramFiles) { $knownPaths += Join-Path $env:ProgramFiles 'Git/bin/bash.exe' }
    foreach ($candidate in $knownPaths) {
        if (Test-GitBashPath $candidate) { return [IO.Path]::GetFullPath($candidate) }
    }

    $git = Invoke-QueryCommand 'git' @('--exec-path')
    if ($git.Code -eq 0 -and $git.Text.Trim()) {
        # Git for Windows: <root>/mingw64/libexec/git-core (or mingw32).
        foreach ($relative in @('../../../bin/bash.exe', '../../../usr/bin/bash.exe')) {
            $candidate = Join-Path $git.Text.Trim() $relative
            if (Test-GitBashPath $candidate) { return [IO.Path]::GetFullPath($candidate) }
        }
    }

    $where = Invoke-QueryCommand 'where.exe' @('bash')
    if ($where.Code -eq 0) {
        foreach ($candidate in ($where.Text -split '\r?\n')) {
            if (Test-GitBashPath $candidate) { return [IO.Path]::GetFullPath($candidate.Trim()) }
        }
    }
    return ''
}

function Get-TomlSections {
    param([string]$Path)
    # Only inspect manifest sections and string fields. Support both inline
    # tables (including multiline bodies) and [patch.crates-io.NAME] tables.
    $content = Get-Content -LiteralPath $Path -Encoding UTF8 -Raw
    $headers = [regex]::Matches($content, '(?m)^\s*\[([^\[\]\r\n]+)\][ \t]*(?:#[^\r\n]*)?\r?$')
    for ($i = 0; $i -lt $headers.Count; $i++) {
        $start = $headers[$i].Index + $headers[$i].Length
        $end = $content.Length
        if ($i + 1 -lt $headers.Count) { $end = $headers[$i + 1].Index }
        [pscustomobject]@{ Name = $headers[$i].Groups[1].Value.Trim(); Body = $content.Substring($start, $end - $start) }
    }
}

function Get-TomlString {
    param([string]$Body, [string]$Key)
    $match = [regex]::Match($Body, ('(?m)(?:^|[,\r\n])\s*' + [regex]::Escape($Key) + '\s*=\s*["'']([^"''\r\n]+)["'']'))
    if ($match.Success) { return $match.Groups[1].Value }
    return ''
}

function Get-PatchRecords {
    param([string]$Path)
    $records = @('PATCHES.md', 'MIV-PATCH.md') | Where-Object {
        Test-Path -LiteralPath (Join-Path $repoRoot "$Path/$_") -PathType Leaf
    } | ForEach-Object { "$Path/$_" }
    if ($records) { return $records -join '; ' }
    # Older egui forks keep their maintenance notes in the README instead.
    $readme = Join-Path $repoRoot "$Path/README.md"
    if ((Test-Path -LiteralPath $readme -PathType Leaf) -and
        (Select-String -LiteralPath $readme -Pattern '^## mImageViewer patch' -Quiet)) {
        return "$Path/README.md (mImageViewer patch)"
    }
    return 'patch record not found'
}

function Get-LocalCrates {
    param([object[]]$Sections)
    foreach ($section in $Sections) {
        $isPatch = $section.Name -eq 'patch.crates-io'
        $isDependency = $section.Name -match '(^|\.)(dev-|build-)?dependencies$'
        if ($isPatch -or $isDependency) {
            foreach ($entry in [regex]::Matches($section.Body, '(?ms)^\s*["'']?([\w-]+)["'']?\s*=\s*\{([^}]+)\}')) {
                $path = Get-TomlString $entry.Groups[2].Value 'path'
                if ($isPatch -or ($path -and (Get-PatchRecords $path) -ne 'patch record not found')) {
                    [pscustomobject]@{ Name = $entry.Groups[1].Value; Path = $path; Kind = $(if ($isPatch) { 'patch.crates-io' } else { 'direct path dependency' }) }
                }
            }
        } elseif ($section.Name -match '^patch\.crates-io\.(.+)$') {
            [pscustomobject]@{ Name = $Matches[1].Trim('"', "'"); Path = (Get-TomlString $section.Body 'path'); Kind = 'patch.crates-io' }
        } elseif ($section.Name -match '(?:^|\.)(?:dev-|build-)?dependencies\.([\w-]+)$') {
            $name = $Matches[1]
            $path = Get-TomlString $section.Body 'path'
            if ($path -and (Get-PatchRecords $path) -ne 'patch record not found') {
                [pscustomobject]@{ Name = $name; Path = $path; Kind = 'direct path dependency' }
            }
        }
    }
}

function Get-Comparison {
    param([string]$Current, [string]$Latest)
    if ($Current -eq $Latest) { return 'up to date' }
    return 'versions differ; review before updating'
}

function Add-SetupCheck {
    param([string]$Name, [string]$Script, [string]$VersionFile, [string]$BashPath)
    $current = Read-Version $VersionFile
    if (-not $BashPath) {
        Add-Row $Name $current 'skipped (bash not found)' "$Script check"
        return
    }
    $result = Invoke-QueryCommand $BashPath @($Script, 'check')
    if ($result.Code -ne 0) {
        Add-Row $Name $current 'unavailable' "$Script check failed; check network and gh availability"
        return
    }
    $latest = 'unavailable'
    switch ($Name) {
        'PDFium' { if ($result.Text -match '(?m)^Latest release:\s*(.+)') { $latest = $Matches[1].Trim() } }
        'FFmpeg' { if ($result.Text -match '(?m)^Matched asset:\s*(.+)') { $latest = $Matches[1].Trim() } }
        'LibRaw' { if ($result.Text -match 'latest release:\s*([^\r\n]+)') { $latest = $Matches[1].Trim() } }
    }
    $summary = 'unexpected check output'
    if ($result.Text -match 'Up to date\.') { $summary = 'up to date' }
    elseif ($result.Text -match 'New version available') { $summary = 'new version available' }
    Add-Row $Name $current $latest "$summary; $Script check"
}

function ConvertTo-TableCell {
    param([string]$Value)
    # Never forward localized native errors or non-ASCII release/tag names.
    $value = [regex]::Replace($Value, '\x1B\[[0-9;]*[A-Za-z]', '')
    $value = [regex]::Replace($value, '[^\x20-\x7E\r\n\t]', '?')
    return (($value -replace '\r?\n', '; ' -replace '\t', ' ') -replace '\|', '\|')
}

$locationPushed = $false
$oldTls = [Net.ServicePointManager]::SecurityProtocol
$oldCargoTimeout = $env:CARGO_HTTP_TIMEOUT
$oldCargoRetry = $env:CARGO_NET_RETRY
$oldRustupAutoInstall = $env:RUSTUP_AUTO_INSTALL
try {
    Push-Location -LiteralPath $repoRoot
    $locationPushed = $true
    [Net.ServicePointManager]::SecurityProtocol = $oldTls -bor [Net.SecurityProtocolType]::Tls12
    $env:CARGO_HTTP_TIMEOUT = '20'
    $env:CARGO_NET_RETRY = '0'
    $env:RUSTUP_AUTO_INSTALL = '0'

    try {
        $sections = @(Get-TomlSections (Join-Path $repoRoot 'Cargo.toml'))
        $seen = @{}
        # Discover patched crates from Cargo.toml, including libmimalloc-sys.
        # Its wrapper version is 0.1.49; the bundled C version and removal
        # condition live in vendor/libmimalloc-sys/PATCHES.md.
        foreach ($crate in Get-LocalCrates $sections) {
            $current = 'unknown (no local manifest)'
            $name = $crate.Name
            $record = 'patch record not found'
            if ($crate.Path) {
                $record = Get-PatchRecords $crate.Path
                try {
                    $package = @(Get-TomlSections (Join-Path $repoRoot "$($crate.Path)/Cargo.toml") | Where-Object { $_.Name -eq 'package' })
                    $packageName = Get-TomlString $package[0].Body 'name'
                    if ($packageName) { $name = $packageName }
                    $current = Get-TomlString $package[0].Body 'version'
                    if (-not $current) { $current = 'unknown (version not a string)' }
                } catch { $current = 'unknown (local manifest unavailable)' }
            }
            if ($seen.ContainsKey($name)) { continue }
            $seen[$name] = $true
            $latest = 'unavailable'
            $comparison = 'crates.io query failed'
            try {
                $response = Get-Json ("https://crates.io/api/v1/crates/" + [uri]::EscapeDataString($name))
                $latest = $response.crate.max_version
                if (-not $latest) { throw 'No crate version' }
                $comparison = Get-Comparison $current $latest
                if ($latest -match '-') { $comparison += '; latest includes prerelease' }
            } catch { $latest = 'unavailable' }
            Add-Row "crate: $name" $current $latest "$comparison; $($crate.Kind); $record"
        }
    } catch { Add-Row 'local crate discovery' 'Cargo.toml' 'unavailable' 'manifest could not be read' }

    $bashPath = Find-GitBash
    Add-SetupCheck 'PDFium' 'scripts/setup-pdfium.sh' 'vendor/pdfium/VERSION' $bashPath
    Add-SetupCheck 'FFmpeg' 'scripts/setup-ffmpeg.sh' 'vendor/ffmpeg/VERSION' $bashPath
    Add-SetupCheck 'LibRaw' 'scripts/setup-libraw.sh' 'vendor/libraw/VERSION' $bashPath

    $current = Read-Version 'vendor/ort/VERSION'
    try {
        $index = Get-Json 'https://api.nuget.org/v3/index.json'
        $base = @($index.resources | Where-Object { $_.'@type' -eq 'PackageBaseAddress/3.0.0' })[0].'@id'
        $versions = Get-Json ($base.TrimEnd('/') + '/microsoft.ml.onnxruntime.directml/index.json')
        $latest = @($versions.versions | Where-Object { $_ -match '^\d+\.\d+\.\d+(\.\d+)?$' } | Sort-Object { [version]$_ })[-1]
        if (-not $latest) { throw 'No stable NuGet version' }
        Add-Row 'ONNX Runtime DirectML' $current $latest "$(Get-Comparison $current $latest); NuGet stable; check ort C API compatibility"
    } catch { Add-Row 'ONNX Runtime DirectML' $current 'unavailable' 'NuGet query failed; vendor/ort/VERSION' }

    $current = Read-Version 'vendor/effetune-mixwright/VERSION'
    try {
        $release = Get-GitHubJson 'repos/Frieve-A/effetune-mixwright/releases/latest'
        if (-not $release.tag_name) { throw 'No release tag' }
        Add-Row 'EffeTune Mixwright' $current $release.tag_name "$(Get-Comparison $current $release.tag_name); GitHub latest release"
    } catch { Add-Row 'EffeTune Mixwright' $current 'unavailable' 'GitHub release query failed' }

    $current = 'unknown'
    # Do not use setup-vst3-sdk.sh check: it performs git fetch.
    if (Test-Path -LiteralPath (Join-Path $repoRoot 'vendor/vst3sdk/.git')) {
        $result = Invoke-QueryCommand 'git' @('-C', 'vendor/vst3sdk', 'describe', '--tags', '--always')
        if ($result.Code -eq 0) { $current = $result.Text.Trim() }
    }
    try {
        $latest = Get-LatestVst3Tag @(Get-GitHubJson 'repos/steinbergmedia/vst3sdk/tags?per_page=100')
        Add-Row 'VST3 SDK' $current $latest 'GitHub latest version/build tag; local git describe (may include commits after tag)'
    } catch { Add-Row 'VST3 SDK' $current 'unavailable' 'GitHub tag query failed; local git describe' }

    $result = Invoke-QueryCommand 'rustc' @('--version')
    $current = 'unknown (rustc unavailable)'
    if ($result.Code -eq 0) { $current = $result.Text.Trim() }
    try {
        $manifest = Invoke-RestMethod -Uri 'https://static.rust-lang.org/dist/channel-rust-stable.toml' -TimeoutSec 20
        $match = [regex]::Match([string]$manifest, '(?ms)^\[pkg\.rust\]\s*.*?^version\s*=\s*"([^" ]+)')
        if (-not $match.Success) { throw 'No stable Rust version' }
        Add-Row 'Rust' $current $match.Groups[1].Value 'official stable channel manifest'
    } catch { Add-Row 'Rust' $current 'unavailable' 'stable channel query failed' }

    $current = 'unknown (provenance unavailable)'
    try {
        $provenance = Get-Content -LiteralPath (Join-Path $repoRoot 'vendor/vcrt/provenance.json') -Encoding UTF8 -Raw | ConvertFrom-Json
        if ($provenance.servicing_version) { $current = $provenance.servicing_version }
    } catch { }
    Add-Row 'Microsoft VC++ runtime' $current 'manual check' 'vendor/vcrt/provenance.json; no reliable latest-version API used'

    $result = Invoke-QueryCommand 'cargo' @('update', '--dry-run', '--color', 'never')
    if ($result.Code -ne 0) {
        Add-Row 'ordinary crates' 'Cargo.lock' 'unavailable' 'cargo update --dry-run failed; check network and cargo availability'
    } else {
        $updates = @([regex]::Matches($result.Text, '(?m)^\s*Updating\s+([\w-]+)\s+v([^\s]+)\s+->\s+v([^\s]+)'))
        # Each line is one package version (multiple versions may share a name).
        $main = @($updates | Sort-Object { $_.Groups[1].Value } | Select-Object -First 10 | ForEach-Object {
            "$($_.Groups[1].Value) $($_.Groups[2].Value) -> $($_.Groups[3].Value)"
        }) -join '; '
        if (-not $main) { $main = 'no compatible version upgrades' }
        $source = 'within manifest constraints'
        if ($env:CARGO_NET_OFFLINE -match '^(true|1)$') { $source += '; offline cache (latest online versions not queried)' }
        Add-Row 'ordinary crates' 'Cargo.lock' "$($updates.Count) upgradable package versions" "$main; cargo update --dry-run; $source"
    }
} catch {
    Add-Row 'check' 'unknown' 'unavailable' 'unexpected query failure'
} finally {
    $env:CARGO_HTTP_TIMEOUT = $oldCargoTimeout
    $env:CARGO_NET_RETRY = $oldCargoRetry
    $env:RUSTUP_AUTO_INSTALL = $oldRustupAutoInstall
    [Net.ServicePointManager]::SecurityProtocol = $oldTls
    if ($locationPushed) { Pop-Location }
}

$date = Get-Date -Format 'yyyy-MM-dd'
$lines = @(
    "# Dependency updates ($date)",
    '',
    'Query only. No dependency downloads, installs, or project updates. Cargo may refresh registry metadata.',
    'Only this report is written. Unavailable queries do not fail the check. Updating is a human decision.',
    '',
    '| Dependency | Current | Latest / result | Notes / patch record |',
    '| --- | --- | --- | --- |'
)
foreach ($row in $rows) {
    $cells = @($row.Name, $row.Current, $row.Latest, $row.Notes) | ForEach-Object { ConvertTo-TableCell $_ }
    $lines += '| ' + ($cells -join ' | ') + ' |'
}
$report = $lines -join "`r`n"
Write-Output $report
try {
    $reportDir = Join-Path $repoRoot 'target/dependency-updates'
    $null = New-Item -ItemType Directory -Path $reportDir -Force
    [IO.File]::WriteAllText((Join-Path $reportDir "$date.md"), $report + "`r`n", [Text.Encoding]::ASCII)
} catch { Write-Output 'Report file unavailable (could not write target/dependency-updates).' }
exit 0
