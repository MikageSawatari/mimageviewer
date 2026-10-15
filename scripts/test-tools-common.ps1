# Shared implementation for the nextest iteration scripts. PowerShell 5.1, ASCII.
Set-StrictMode -Version Latest

function ConvertTo-MivNativeArgument {
    param([AllowEmptyString()] [string] $Value)
    # Windows CommandLineToArgvW / CRT quoting, including trailing backslashes.
    $valueQuoted = [regex]::Replace($Value, '(\\*)"', '$1$1\"')
    $valueQuoted = [regex]::Replace($valueQuoted, '(\\+)$', '$1$1')
    return '"' + $valueQuoted + '"'
}

function Invoke-MivTool {
    param([string] $File, [string[]] $Arguments, [string] $RepoRoot)
    $command = Get-Command $File -CommandType Application -ErrorAction Stop | Select-Object -First 1
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo.FileName = $command.Source
    $process.StartInfo.Arguments = (($Arguments | ForEach-Object { ConvertTo-MivNativeArgument $_ }) -join ' ')
    $process.StartInfo.WorkingDirectory = $RepoRoot
    $process.StartInfo.UseShellExecute = $false
    $process.StartInfo.CreateNoWindow = $true
    $process.StartInfo.RedirectStandardOutput = $true
    $process.StartInfo.RedirectStandardError = $true
    $process.StartInfo.StandardOutputEncoding = New-Object System.Text.UTF8Encoding($false)
    $process.StartInfo.StandardErrorEncoding = New-Object System.Text.UTF8Encoding($false)
    $process.StartInfo.EnvironmentVariables['GIT_OPTIONAL_LOCKS'] = '0'
    $watch = [Diagnostics.Stopwatch]::StartNew()
    try {
        [void] $process.Start()
        # Drain both pipes concurrently; native stderr is not a PS error record.
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            StdOut = $stdoutTask.Result
            StdErr = $stderrTask.Result
            Seconds = $watch.Elapsed.TotalSeconds
        }
    } finally { $process.Dispose() }
}

function Invoke-MivGit {
    param([string] $RepoRoot, [string[]] $Arguments)
    $result = Invoke-MivTool 'git' $Arguments $RepoRoot
    if ($result.ExitCode -ne 0) { throw "git failed: $($result.StdErr.Trim())" }
    return $result.StdOut
}

function Get-MivTextHash {
    param([string] $Value)
    $sha = New-Object Security.Cryptography.SHA256Cng
    try { return ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($Value)))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}

function Get-MivSourceId {
    param([string] $RepoRoot)
    $trackedAndNew = Invoke-MivGit $RepoRoot @('ls-files', '-z', '--cached', '--others', '--exclude-standard')
    # Include ignored native inputs and optional real fixtures, but no build outputs.
    $ignoredInputs = Invoke-MivGit $RepoRoot @('ls-files', '-z', '--others', '--ignored', '--exclude-standard', '--', '.cargo', '.config', 'src', 'tests', 'scripts', 'crates', 'vendor', 'testdata', 'testimage', 'test_images', 'assets', 'resources')
    $paths = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
    foreach ($path in ($trackedAndNew + $ignoredInputs).Split([char]0)) {
        if (-not $path) { continue }
        if ($path -match '(^|/)target/|^target-[^/]*/|(^|/)(node_modules|__pycache__|\.git)/|^vendor/libraw/(build|object)/|^crates/vst3-host/build/|\.(obj|pdb|ilk|tlog|part)$|^tests/snapshots/.*\.(new|old|diff)\.png$') { continue }
        # This generated measurement report is not a build/test input.
        if ($path -eq 'docs/test-speed-report.md') { continue }
        [void] $paths.Add($path)
    }
    $sorted = [string[]] @($paths)
    [Array]::Sort($sorted, [StringComparer]::Ordinal)
    $manifest = New-Object Text.StringBuilder
    [void] $manifest.Append("miv-source-v1`n")
    $sha = New-Object Security.Cryptography.SHA256Cng
    try {
        foreach ($path in $sorted) {
            $absolute = [IO.Path]::Combine($RepoRoot, $path)
            if ([IO.Directory]::Exists($absolute)) { throw "Cannot fingerprint nested repository/directory: $path" }
            $digest = 'missing'
            if ([IO.File]::Exists($absolute)) {
                $stream = [IO.File]::OpenRead($absolute)
                try { $digest = ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '') }
                finally { $stream.Dispose() }
            }
            [void] $manifest.Append($path).Append([char]0).Append($digest).Append("`n")
        }
    } finally { $sha.Dispose() }
    return Get-MivTextHash $manifest.ToString()
}

function Get-MivEnvironmentId {
    param([string] $RepoRoot)
    $parts = New-Object 'System.Collections.Generic.List[string]'
    foreach ($entry in (Get-ChildItem Env: | Sort-Object Name)) {
        if ($entry.Name -match '^(CARGO|RUST|NEXTEST|MIV|MIMV|CMAKE|LLVM|VCPKG|PKG_CONFIG|CC_|CXX_)' -or $entry.Name -in @('PATH', 'PATHEXT', 'FFMPEG_DIR', 'LIBCLANG_PATH', 'WGPU_BACKEND', 'UPDATE_SNAPSHOTS', 'TEMP', 'TMP', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA', 'LIB', 'INCLUDE', 'CC', 'CXX', 'AR', 'CFLAGS', 'CXXFLAGS', 'LDFLAGS', 'MSBUILDDISABLENODEREUSE')) {
            $parts.Add($entry.Name + '=' + $entry.Value)
        }
    }
    foreach ($tool in @(@('cargo', '--version'), @('cargo', 'nextest', '--version'), @('rustc', '-Vv'))) {
        $result = Invoke-MivTool $tool[0] $tool[1..($tool.Length - 1)] $RepoRoot
        if ($result.ExitCode -ne 0) { throw "Required tool unavailable: $($tool -join ' '). Install cargo-nextest with cargo install cargo-nextest --locked." }
        $parts.Add($result.StdOut)
    }
    $parts.Add('logical-cpus=' + [Environment]::ProcessorCount)
    # Values may contain credentials: persist only this digest, never the values.
    return Get-MivTextHash ($parts -join "`n")
}

function Get-MivFeatureArguments {
    param([string[]] $Features, [switch] $NoDefaultFeatures)
    $unique = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
    foreach ($value in $Features) {
        foreach ($name in ($value -split '[,\s]+')) { if ($name) { [void] $unique.Add($name) } }
    }
    $names = [string[]] @($unique)
    [Array]::Sort($names, [StringComparer]::Ordinal)
    if ($names.Count) { '--features'; ($names -join ',') }
    if ($NoDefaultFeatures) { '--no-default-features' }
}

function New-MivTestRun {
    param([string] $RepoRoot, [string] $StoreName, [string] $NextestProfile)
    $id = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 8)
    $runDir = Join-Path $RepoRoot "target/$StoreName/runs/$id"
    [void] [IO.Directory]::CreateDirectory($runDir)
    $config = ''
    $repoConfig = Join-Path $RepoRoot '.config/nextest.toml'
    if ([IO.File]::Exists($repoConfig)) { $config = [IO.File]::ReadAllText($repoConfig) }
    if ($config -match '(?m)^\[profile\.miv-tool[.\]]') { throw 'nextest profile miv-tool is reserved by the test scripts' }
    $junit = Join-Path $runDir 'junit.xml'
    $config += "`n[profile.miv-tool]`ninherits = `"$NextestProfile`"`n[profile.miv-tool.junit]`npath = `"$($junit.Replace('\', '/'))`"`nreport-skipped = `"all`"`n"
    $configPath = Join-Path $runDir 'nextest.toml'
    [IO.File]::WriteAllText($configPath, $config, (New-Object Text.UTF8Encoding($false)))
    return [pscustomobject]@{ Id = $id; Directory = $runDir; Config = $configPath; JUnit = $junit }
}

function Get-MivJUnitSummary {
    param([string] $Path)
    $xml = New-Object Xml.XmlDocument
    $xml.XmlResolver = $null
    $xml.Load($Path)
    $root = $xml.DocumentElement
    if ($root.get_Name() -ne 'testsuites') { throw 'Missing nextest testsuites result' }
    $cases = @($xml.SelectNodes('/testsuites/testsuite/testcase'))
    $skipped = @($xml.SelectNodes('/testsuites/testsuite/testcase[skipped]')).Count
    $failed = @($xml.SelectNodes('/testsuites/testsuite/testcase[failure or error]')).Count
    if ($cases.Count -ne [int] $root.GetAttribute('tests') -or $skipped -ne [int] $root.GetAttribute('skipped') -or $failed -ne ([int] $root.GetAttribute('failures') + [int] $root.GetAttribute('errors'))) {
        throw 'Inconsistent nextest JUnit counts'
    }
    return [pscustomobject]@{
        Total = $cases.Count; Executed = $cases.Count - $skipped
        Passed = $cases.Count - $skipped - $failed; Failed = $failed; Skipped = $skipped
        Seconds = [double]::Parse($root.GetAttribute('time'), [Globalization.CultureInfo]::InvariantCulture)
    }
}

function Invoke-MivNextest {
    param([string] $RepoRoot, [string[]] $Selection, [string[]] $FeatureArguments,
        [string] $CargoProfile, [string] $NextestProfile, $Run)
    $cmdArgs = @('nextest', 'run') + $Selection + $FeatureArguments + @('--cargo-profile', $CargoProfile, '--profile', 'miv-tool', '--config-file', $Run.Config,
        '--color', 'never', '--message-format', 'human', '--no-tests', 'fail', '--run-ignored', 'default', '--ignore-default-filter', '--retries', '0', '--no-fail-fast')
    Write-Host "[test-tools] cargo $($cmdArgs -join ' ')"
    Write-Host "[test-tools] logs: $($Run.Directory)"
    $result = Invoke-MivTool 'cargo' $cmdArgs $RepoRoot
    foreach ($streamName in @('StdOut', 'StdErr')) {
        try { [IO.File]::WriteAllText((Join-Path $Run.Directory ($streamName.ToLowerInvariant() + '.log')), $result.$streamName, (New-Object Text.UTF8Encoding($false))) }
        catch { Write-Warning "Cannot write $streamName log: $_" }
    }
    $tail = @($result.StdErr -split '\r?\n' | Where-Object { $_ } | Select-Object -Last 20)
    if ($tail.Count) { Write-Host ($tail -join "`n") }
    return [pscustomobject]@{ ExitCode = $result.ExitCode; Command = @('cargo') + $cmdArgs; WallSeconds = $result.Seconds }
}

function Get-MivChangedPaths {
    param([string] $RepoRoot, [string] $Base)
    $commit = (Invoke-MivGit $RepoRoot @('rev-parse', '--verify', '--end-of-options', ($Base + '^{commit}'))).Trim()
    $changed = Invoke-MivGit $RepoRoot @('diff', '--name-only', '-z', '--no-renames', $commit, '--')
    $new = Invoke-MivGit $RepoRoot @('ls-files', '--others', '--exclude-standard', '-z')
    return @(($changed + $new).Split([char]0) | Where-Object { $_ } | Sort-Object -Unique)
}

function Get-MivFocusedPlan {
    param([string] $RepoRoot, [string[]] $Paths)
    $meta = Invoke-MivTool 'cargo' @('metadata', '--no-deps', '--format-version', '1') $RepoRoot
    if ($meta.ExitCode -ne 0) { throw "cargo metadata failed: $($meta.StdErr)" }
    $packages = @((ConvertFrom-Json $meta.StdOut).packages | Sort-Object { $_.manifest_path.Length } -Descending)
    $entries = New-Object 'System.Collections.Generic.List[object]'
    $broad = New-Object 'System.Collections.Generic.List[string]'
    $seen = New-Object 'System.Collections.Generic.HashSet[string]'
    foreach ($original in $Paths) {
        $path = $original.Replace('\', '/') -replace '^\./', ''
        $absolute = [IO.Path]::GetFullPath((Join-Path $RepoRoot $path))
        if (-not $absolute.StartsWith(($RepoRoot.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase)) { throw "Path outside repository: $original" }
        if ($path -match '^(Cargo\.(toml|lock)|build\.rs|src/(lib|main|app)\.rs|src/app/tests\.rs)$|^(\.cargo/|\.config/|vendor/|tests/(support|snapshots)/)') {
            $broad.Add($path); continue
        }
        $package = $null
        foreach ($candidate in $packages) {
            $dir = [IO.Path]::GetDirectoryName($candidate.manifest_path)
            if ($absolute.StartsWith(($dir + [IO.Path]::DirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase)) { $package = $candidate; break }
        }
        if ($null -eq $package) { continue }
        $selection = @('-p', $package.name)
        $filter = 'all()'
        $packageDir = [IO.Path]::GetDirectoryName($package.manifest_path)
        if (-not $packageDir.Equals($RepoRoot, [StringComparison]::OrdinalIgnoreCase)) {
            # Select every target in a changed workspace crate, including bins/tests.
            if ($path -notmatch '\.(rs|toml|lock|c|cpp|h|hpp)$') { continue }
        } elseif ($path -match '^src/(.+)\.rs$') {
            $module = $Matches[1] -replace '/mod$', ''
            $components = @($module -split '/')
            if (@($components | Where-Object { $_ -notmatch '^[A-Za-z_][A-Za-z0-9_]*$' }).Count) { $broad.Add($path); continue }
            if ($components[0] -eq 'bin') { $broad.Add($path); continue }
            $selection += '--lib'
            $filter = 'test(~' + ($components -join '::') + '::)'
        } elseif ($path -match '^tests/([A-Za-z_][A-Za-z0-9_-]*)\.rs$') {
            $selection += @('--test', $Matches[1])
            $filter = 'kind(test)'
        } elseif ($path -match '^tests/') {
            $broad.Add($path); continue
        } else { continue }
        $key = ($selection -join '|') + '|' + $filter
        if ($seen.Add($key)) { $entries.Add([pscustomobject]@{ Path = $path; Selection = $selection; Filter = $filter }) }
    }
    return [pscustomobject]@{ Entries = $entries.ToArray(); BroadPaths = $broad.ToArray() }
}
