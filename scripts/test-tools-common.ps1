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

function Test-MivExcludedSourcePath {
    param([string] $Path)
    if ($Path -match '(^|/)target/|^target-[^/]*/|(^|/)(node_modules|__pycache__|\.git)/|^vendor/libraw/(build|object)/|^crates/vst3-host/build/|\.(obj|pdb|ilk|tlog|part)$|^tests/snapshots/.*\.(new|old|diff)\.png$') { return $true }
    # This generated measurement report is not a build/test input.
    return ($Path -eq 'docs/test-speed-report.md')
}

function Get-MivNestedFiles {
    param([string] $Absolute, [string] $Relative)
    $result = New-Object 'System.Collections.Generic.List[object]'
    $pending = New-Object 'System.Collections.Generic.Stack[object]'
    $pending.Push(@($Absolute, $Relative.TrimEnd('/')))
    while ($pending.Count -gt 0) {
        $item = $pending.Pop()
        $dirInfo = New-Object IO.DirectoryInfo $item[0]
        # Do not follow junctions/symlinks: their targets are outside this checkout.
        if ($dirInfo.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Cannot fingerprint reparse point: $($item[1])" }
        foreach ($entry in $dirInfo.GetFileSystemInfos()) {
            $rel = $item[1] + '/' + $entry.Name
            if ($entry -is [IO.DirectoryInfo]) {
                # Same build-output rules as top-level paths (the trailing slash lets
                # directory patterns such as __pycache__/ and .git/ match).
                if (Test-MivExcludedSourcePath ($rel + '/')) { continue }
                $pending.Push(@($entry.FullName, $rel))
            } else {
                if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Cannot fingerprint reparse point: $rel" }
                if (Test-MivExcludedSourcePath $rel) { continue }
                $result.Add([pscustomobject] @{ Absolute = $entry.FullName; Relative = $rel })
            }
        }
    }
    $items = $result.ToArray()
    [Array]::Sort($items, [Comparison[object]] { param($a, $b) [string]::CompareOrdinal($a.Relative, $b.Relative) })
    return $items
}

function Read-MivSourceHashCache {
    param([string] $Path, [string] $RepoRoot)
    $entries = New-Object 'System.Collections.Generic.Dictionary[string,object]' ([StringComparer]::Ordinal)
    if ([IO.File]::Exists($Path)) {
        $lines = [IO.File]::ReadAllLines($Path, [Text.Encoding]::UTF8)
        if ($lines.Length -lt 2 -or $lines[0] -cne ('miv-stat-cache-v2 ' + (Get-MivTextHash $RepoRoot))) { throw 'Invalid stat-cache header' }
        # A well-formed but altered row (for example one changed hex digit) must not
        # be trusted: the footer carries a digest of every line before it.
        $body = New-Object Text.StringBuilder
        for ($i = 0; $i -lt ($lines.Length - 1); $i++) { [void] $body.Append($lines[$i]).Append("`n") }
        $bodyDigest = Get-MivTextHash $body.ToString()
        for ($i = 1; $i -lt ($lines.Length - 1); $i++) {
            $fields = $lines[$i].Split([char]9)
            if ($fields.Length -ne 6 -or $fields[4] -cnotmatch '^[0-9A-F]{64}$') { throw 'Invalid stat-cache row' }
            $relative = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($fields[0]))
            $size = [long]::Parse($fields[1], [Globalization.CultureInfo]::InvariantCulture)
            $modified = [long]::Parse($fields[2], [Globalization.CultureInfo]::InvariantCulture)
            $created = [long]::Parse($fields[3], [Globalization.CultureInfo]::InvariantCulture)
            $recorded = [long]::Parse($fields[5], [Globalization.CultureInfo]::InvariantCulture)
            if ($size -lt 0 -or $modified -le 0 -or $created -le 0 -or $recorded -le 0) { throw 'Invalid stat-cache metadata' }
            $entries.Add($relative, @($size, $modified, $created, $fields[4], $recorded))
        }
        if ($lines[$lines.Length - 1] -cne ('end ' + $entries.Count + ' ' + $bodyDigest)) { throw 'Incomplete or altered stat cache' }
    }
    # Return the dictionary itself, including when it is empty.
    return ,$entries
}

function Write-MivSourceHashCache {
    param([string] $Path, [string] $RepoRoot, $Entries)
    $text = New-Object Text.StringBuilder
    [void] $text.Append('miv-stat-cache-v2 ').Append((Get-MivTextHash $RepoRoot)).Append("`n")
    foreach ($entry in $Entries.GetEnumerator()) {
        $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($entry.Key))
        [void] $text.Append($encoded).Append([char]9).Append(($entry.Value -join "`t")).Append("`n")
    }
    $bodyDigest = Get-MivTextHash $text.ToString()
    [void] $text.Append('end ').Append($Entries.Count).Append(' ').Append($bodyDigest).Append("`n")
    [void] [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($Path))
    [IO.File]::WriteAllText($Path, $text.ToString(), (New-Object Text.UTF8Encoding($false)))
}

function Get-MivSourceFileHash {
    param([string] $Absolute, [string] $Relative, $Sha, $Cache)
    $info = New-Object IO.FileInfo $Absolute
    if (-not $info.Exists) { return 'missing' }
    $size = $info.Length
    $modified = $info.LastWriteTimeUtc.Ticks
    $created = $info.CreationTimeUtc.Ticks
    # A file link's stat may describe the link rather than its changing target.
    # Preserve the original content-hash behavior without caching such files.
    $canCache = $null -ne $Cache -and -not ($info.Attributes -band [IO.FileAttributes]::ReparsePoint)
    $entry = $null
    if ($canCache -and $Cache.Old.TryGetValue($Relative, [ref] $entry) -and
        $entry[0] -eq $size -and $entry[1] -eq $modified -and $entry[2] -eq $created -and
        ($entry[4] - $modified) -gt [TimeSpan]::FromSeconds(2).Ticks) {
        # Keep the original observation time: a hit must never age a racy entry.
        $Cache.New.Add($Relative, $entry)
        return $entry[3]
    }
    $recorded = [DateTime]::UtcNow.Ticks
    $stream = [IO.File]::OpenRead($Absolute)
    try { $digest = ([BitConverter]::ToString($Sha.ComputeHash($stream))).Replace('-', '') }
    finally { $stream.Dispose() }
    if ($canCache) {
        $info.Refresh()
        if ($info.Exists -and $info.Length -eq $size -and $info.LastWriteTimeUtc.Ticks -eq $modified -and $info.CreationTimeUtc.Ticks -eq $created) {
            $Cache.New.Add($Relative, @($size, $modified, $created, $digest, $recorded))
        }
    }
    return $digest
}

function Get-MivSourceId {
    [CmdletBinding()]
    param([string] $RepoRoot, [switch] $NoCache, [switch] $ForceHash)
    $cache = $null
    $cachePath = Join-Path $RepoRoot 'target/test-lib-full/source-hashes-v1.tsv'
    if (-not $NoCache) {
        try {
            $old = if ($ForceHash) { New-Object 'System.Collections.Generic.Dictionary[string,object]' ([StringComparer]::Ordinal) }
                else { Read-MivSourceHashCache $cachePath $RepoRoot }
            $cache = @{ Old = $old; New = (New-Object 'System.Collections.Generic.Dictionary[string,object]' ([StringComparer]::Ordinal)) }
        } catch { Write-Warning "Cannot read source hash cache; hashing all files without cache: $_" }
    }
    $source = Get-MivSourceManifestId $RepoRoot $cache
    if ($null -ne $cache) {
        try { Write-MivSourceHashCache $cachePath $RepoRoot $cache.New }
        catch {
            Write-Warning "Cannot write source hash cache; hashing all files without cache: $_"
            return Get-MivSourceManifestId $RepoRoot $null
        }
    }
    return $source
}

function Get-MivSourceManifestId {
    param([string] $RepoRoot, $Cache)
    $trackedAndNew = Invoke-MivGit $RepoRoot @('ls-files', '-z', '--cached', '--others', '--exclude-standard')
    # Include ignored native inputs and optional real fixtures, but no build outputs.
    $ignoredInputs = Invoke-MivGit $RepoRoot @('ls-files', '-z', '--others', '--ignored', '--exclude-standard', '--', '.cargo', '.config', 'src', 'tests', 'scripts', 'crates', 'vendor', 'testdata', 'testimage', 'test_images', 'assets', 'resources')
    $paths = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
    foreach ($path in ($trackedAndNew + $ignoredInputs).Split([char]0)) {
        if (-not $path) { continue }
        if (Test-MivExcludedSourcePath $path) { continue }
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
            if ([IO.Directory]::Exists($absolute)) {
                # git lists a nested repository (e.g. a cloned reference under testdata)
                # as one directory entry. Hash its files, excluding its .git.
                foreach ($nested in (Get-MivNestedFiles $absolute $path)) {
                    $digest = Get-MivSourceFileHash $nested.Absolute $nested.Relative $sha $Cache
                    [void] $manifest.Append($nested.Relative).Append([char]0).Append($digest).Append("`n")
                }
                continue
            }
            $digest = Get-MivSourceFileHash $absolute $path $sha $Cache
            [void] $manifest.Append($path).Append([char]0).Append($digest).Append("`n")
        }
    } finally { $sha.Dispose() }
    return Get-MivTextHash $manifest.ToString()
}

function Get-MivCargoEnvironmentNames {
    param([string] $RepoRoot)
    $config = Join-Path $RepoRoot '.cargo/config.toml'
    if (-not [IO.File]::Exists($config)) { return }
    $section = ''
    foreach ($line in [IO.File]::ReadAllLines($config)) {
        if ($line -match '^\s*(#.*)?$') { continue }
        if ($line -match '^\s*\[([^\]]+)\]\s*(#.*)?$') {
            # Escapes in quoted keys can spell env (e.g. "\u0065nv"); do not decode them.
            if ($Matches[1].Contains('\')) { throw 'Escaped Cargo config table name; cannot safely reuse results' }
            # TOML allows quoted and spaced key parts ("env", 'env' . NAME); compare
            # the header with those removed so every spelling of the env table is seen.
            $section = ($Matches[1] -replace '\s', '') -replace '["'']', ''
            if ($section -match '^env\.([A-Za-z_][A-Za-z0-9_-]*)$') {
                $Matches[1]
            } elseif ($section -match '^env\.') { throw 'Unsupported Cargo env table syntax; cannot safely reuse results' }
            continue
        }
        if ($section -eq '' -and ($line -match '^\s*["'']?env["'']?\s*[.=]' -or $line -match '^\s*"[^"]*\\')) {
            throw 'Unsupported Cargo env syntax outside an [env] table; cannot safely reuse results'
        }
        if ($section -eq 'env') {
            if ($line -notmatch '^\s*([A-Za-z_][A-Za-z0-9_-]*|"[A-Za-z_][A-Za-z0-9_-]*"|''[A-Za-z_][A-Za-z0-9_-]*'')\s*=') {
                throw 'Unsupported Cargo env entry syntax; cannot safely reuse results'
            }
            $Matches[1].Trim('"', "'")
        }
    }
}

function Get-MivEnvironmentId {
    param([string] $RepoRoot)
    $parts = New-Object 'System.Collections.Generic.List[string]'
    $names = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach ($name in @('PATH', 'PATHEXT', 'FFMPEG_DIR', 'LIBCLANG_PATH', 'WGPU_BACKEND', 'UPDATE_SNAPSHOTS', 'TEMP', 'TMP', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA', 'LIB', 'INCLUDE', 'CC', 'CXX', 'AR', 'CFLAGS', 'CXXFLAGS', 'LDFLAGS', 'MSBUILDDISABLENODEREUSE') + @(Get-MivCargoEnvironmentNames $RepoRoot)) {
        [void] $names.Add($name.ToUpperInvariant())
    }
    foreach ($entry in (Get-ChildItem Env:)) {
        if ($entry.Name -match '^(CARGO|RUST|NEXTEST|MIV|MIMV|CMAKE|LLVM|VCPKG|PKG_CONFIG|CC_|CXX_)') { [void] $names.Add($entry.Name.ToUpperInvariant()) }
    }
    $sorted = [string[]] @($names)
    [Array]::Sort($sorted, [StringComparer]::Ordinal)
    foreach ($name in $sorted) {
        $value = [Environment]::GetEnvironmentVariable($name)
        $parts.Add($name + '=' + $(if ($null -eq $value) { 'absent' } else { $value.Length.ToString() + ':' + $value }))
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

function Get-MivNativeRuntimeId {
    param([string] $RepoRoot, [string] $CargoProfile)
    $meta = Invoke-MivTool 'cargo' @('metadata', '--no-deps', '--format-version', '1') $RepoRoot
    if ($meta.ExitCode -ne 0) { throw "Cannot locate native runtime directory: $($meta.StdErr)" }
    $target = (ConvertFrom-Json $meta.StdOut).target_directory
    $profileDir = switch ($CargoProfile) { 'test' { 'debug' } 'dev' { 'debug' } 'bench' { 'release' } default { $CargoProfile } }
    $roots = @($target)
    # Explicit target triples put executables one directory below target_directory.
    if ([IO.Directory]::Exists($target)) { $roots += @(Get-ChildItem -LiteralPath $target -Directory | Select-Object -ExpandProperty FullName) }
    $parts = New-Object 'System.Collections.Generic.List[string]'
    $parts.Add('miv-native-runtime-v1')
    $sha = New-Object Security.Cryptography.SHA256Cng
    try {
        foreach ($root in ($roots | Sort-Object -Unique)) {
            foreach ($dir in @((Join-Path $root $profileDir), (Join-Path $root "$profileDir/deps"))) {
                if (-not [IO.Directory]::Exists($dir)) { continue }
                foreach ($dll in (Get-ChildItem -LiteralPath $dir -File -Filter '*.dll' | Sort-Object Name)) {
                    # Cargo's hashed Rust/proc-macro artifacts are build outputs. All other
                    # app-local DLLs (including all six FFmpeg DLLs) are runtime inputs.
                    if ($dll.Name -match '-[0-9a-f]{16}\.dll$') { continue }
                    $stream = [IO.File]::OpenRead($dll.FullName)
                    try { $digest = [BitConverter]::ToString($sha.ComputeHash($stream)) }
                    finally { $stream.Dispose() }
                    $parts.Add($dll.FullName + [char]0 + $digest)
                }
            }
        }
    } finally { $sha.Dispose() }
    # The set of paths records existence as well as content: removal changes this ID.
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
        if ($path -match '(^|/)(Cargo\.(toml|lock)|build\.rs)$|^src/(lib|main|app)\.rs$|^src/app/tests\.rs$|^(\.cargo/|\.config/|vendor/|tests/(support|snapshots)/)') {
            $broad.Add($path); continue
        }
        $package = $null
        foreach ($candidate in $packages) {
            $dir = [IO.Path]::GetDirectoryName($candidate.manifest_path)
            if ($absolute.StartsWith(($dir + [IO.Path]::DirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase)) { $package = $candidate; break }
        }
        if ($null -eq $package) { $broad.Add($path); continue }
        $selection = @('-p', $package.name)
        $filter = 'all()'
        $packageDir = [IO.Path]::GetDirectoryName($package.manifest_path)
        if (-not $packageDir.Equals($RepoRoot, [StringComparison]::OrdinalIgnoreCase)) {
            # Select every target in a changed workspace crate, including bins/tests.
            if ($path -notmatch '\.(rs|toml|lock|c|cpp|h|hpp)$') { $broad.Add($path); continue }
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
        } else { $broad.Add($path); continue }
        $key = ($selection -join '|') + '|' + $filter
        if ($seen.Add($key)) { $entries.Add([pscustomobject]@{ Path = $path; Selection = $selection; Filter = $filter }) }
    }
    return [pscustomobject]@{ Entries = $entries.ToArray(); BroadPaths = $broad.ToArray() }
}
