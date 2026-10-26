# ASCII only. Does not build or run Cargo, WebView2, or a product executable.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'migration-common.ps1')
$script:MigrationRun = Join-Path $script:ProbeRoot ('o-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
Assert-ProbePath $script:MigrationRun
[IO.Directory]::CreateDirectory($script:MigrationRun) | Out-Null
$script:MigrationCheckpoints = Join-Path $script:MigrationRun 'checkpoints'
[IO.Directory]::CreateDirectory($script:MigrationCheckpoints) | Out-Null
$script:MigrationChildren = [Collections.Generic.List[object]]::new()
$script:MigrationUdfs = [Collections.Generic.List[string]]::new()
$checks = [Collections.Generic.List[object]]::new()
function Check([string]$Name, [bool]$Condition) {
    $checks.Add([pscustomobject]@{ name = $Name; pass = $Condition })
    if (-not $Condition) { throw "Offline check failed: $Name" }
}
$errorText = $null
try {
    $layout = Get-MigrationRunLayout
    Check 'short run layout stays in migration root' ([IO.Path]::GetDirectoryName($layout.root) -ceq $script:ProbeRoot -and
        [IO.Path]::GetFileName($layout.root) -cmatch '^[a-f0-9]{8}$')
    Check 'binary root remains separate from migration data' ($script:MigrationBinaryRoot -cne $script:ProbeRoot -and
        $script:MigrationBinaryRoot.EndsWith('\target\webview2-lockfile-probe'))
    $policy = [pscustomobject]@{version=1; legacy_root=$script:MigrationBinaryRoot;
        migration_root=$script:ProbeRoot; legacy_run_prefix='run-'; migration_run_id_length=8}
    Assert-ProbePathPolicy $policy $script:MigrationBinaryRoot
    Check 'matching build path policy accepted' $true
    $denied = $false
    try { Assert-ProbePathPolicy $null $script:MigrationBinaryRoot } catch { $denied = $_.Exception.Message -like '*Rebuild probe*' }
    Check 'legacy metadata without policy requires rebuild' $denied
    foreach ($pair in @(@('version', 0), @('legacy_root', 'C:\other\target\webview2-lockfile-probe'),
        @('migration_root', ($script:ProbeRoot + '-other')), @('legacy_run_prefix', '*'), @('migration_run_id_length', 9))) {
        $bad = $policy | ConvertTo-Json | ConvertFrom-Json
        $bad.($pair[0]) = $pair[1]
        $denied = $false
        try { Assert-ProbePathPolicy $bad $script:MigrationBinaryRoot } catch { $denied = $_.Exception.Message -like '*Rebuild probe*' }
        Check ('mismatching compiled policy rejected: ' + $pair[0]) $denied
    }
    $denied = $false
    try { $null = Read-ProbePathPolicy (Join-Path $script:MigrationRun 'missing-probe.exe') } catch {
        $denied = $_.Exception.Message -like '*Cannot verify probe path policy*Rebuild probe*'
    }
    Check 'unavailable metadata query requires rebuild before host startup' $denied
    # The real build script imports common.ps1 in its own script scope. Exercise
    # that boundary without compiling anything or launching a browser/product.
    $scopeScript = Join-Path $script:MigrationRun 'synthetic-build-scope.ps1'
    [IO.File]::WriteAllText($scopeScript, @'
param([string]$CommonPath)
. $CommonPath
Assert-ProbePath (Join-Path $script:ProbeRoot 'bin\synthetic')
$script:ProbeRoot
'@, [Text.Encoding]::ASCII)
    $migrationRootBeforeBuild = $script:ProbeRoot
    $syntheticBuildRoot = & $scopeScript (Join-Path $PSScriptRoot 'common.ps1')
    Check 'build scope still permits original binary directory' ($syntheticBuildRoot -ceq $script:MigrationBinaryRoot)
    Check 'build scope cannot reset migration guards' ($script:ProbeRoot -ceq $migrationRootBeforeBuild)
    Check 'production layout is not created by preflight' (-not [IO.Directory]::Exists($layout.root))
    $budget = Assert-MigrationPathBudget $layout.source $layout.destination
    Check 'current source and destination have reserved headroom' ($budget.checks.Count -eq 2 -and
        @($budget.checks | Where-Object { $_.headroom_utf16_units -lt 32 }).Count -eq 0)
    Check 'preflight uses source engine layout' ($layout.source -ceq (Join-Path $layout.fake_appdata 'mimageviewer-vst3-host.exe'))
    Check 'preflight uses destination engine layout' ($layout.destination -ceq (Join-Path $layout.data_dir 'effetune\webview'))
    # Historical absolute spelling is intentional: this remains an oversized
    # regression input even when this test runs from a shorter checkout.
    $oldRun = 'C:\home\mimageviewer\.claude\worktrees\search-slow-1424-88bb45\target\webview2-lockfile-probe\run-migration-20261026-185550-453-24436'
    $oldSource = Join-Path $oldRun 'fake-appdata\mimageviewer-vst3-host.exe'
    $oldDestination = Join-Path $oldRun 'isolated-data\effetune\webview'
    $denied = $false
    try { $null = Assert-MigrationPathBudget $oldSource $oldDestination } catch {
        $denied = $_.Exception.Message -like '*path budget exceeded before startup: source*'
    }
    Check 'reported long run layout rejected without opening storage' $denied
    $page = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'page.html'))
    Check 'path budget origin matches storage fixture' ($page.Contains("location.origin !== 'https://effetune-mixwright.localhost'"))
    $tail = 'EBWebView\Default\IndexedDB\https_effetune-mixwright.localhost_0.indexeddb.leveldb\MANIFEST-18446744073709551615'
    $boundary = 'C:\' + ('x' * (227 - 4 - $tail.Length))
    $atLimit = Assert-MigrationPathBudget $boundary $boundary
    Check 'exact budget boundary accepted' ($atLimit.checks[0].utf16_units -eq 227 -and
        $atLimit.checks[0].headroom_utf16_units -eq 32)
    foreach ($role in @('source', 'destination')) {
        $denied = $false
        try {
            if ($role -eq 'source') { $null = Assert-MigrationPathBudget ($boundary + 'x') 'C:\short' }
            else { $null = Assert-MigrationPathBudget 'C:\short' ($boundary + 'x') }
        } catch { $denied = $_.Exception.Message -like ('*before startup: ' + $role + ' = 228 UTF-16 units*') }
        Check ('one unit over budget rejected for ' + $role) $denied
    }
    $unicode = $boundary.Substring(0, $boundary.Length - 2) + [char]::ConvertFromUtf32(0x1F600)
    $unicodeBudget = Assert-MigrationPathBudget $unicode 'C:\short'
    Check 'non-BMP path counts surrogate pair as two units' ($unicodeBudget.checks[0].utf16_units -eq 227)
    $denied = $false
    try { $null = Get-MigrationRunLayout '..\escape' } catch { $denied = $true }
    Check 'invalid short run ID rejected' $denied
    $denied = $false
    try { Assert-ProbePath ($script:ProbeRoot + '-other\12345678') } catch { $denied = $true }
    Check 'similar migration root prefix rejected' $denied
    $runner = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-migration.ps1'))
    Check 'runner preflight precedes build and host startup' ($runner.IndexOf('Assert-MigrationPathBudget') -lt
        $runner.IndexOf("if (-not `$SkipBuild)") -and $runner.IndexOf('Assert-MigrationPathBudget') -lt $runner.IndexOf('Start-MigrationHost'))
    Check 'recorded policy checked before creating run' ($runner.IndexOf('Assert-ProbePathPolicy $build.path_policy') -gt 0 -and
        $runner.IndexOf('Assert-ProbePathPolicy $build.path_policy') -lt $runner.IndexOf('New-Item'))
    Check 'actual executable policy checked before creating run' ($runner.IndexOf('Read-ProbePathPolicy $script:MigrationProbe') -gt 0 -and
        $runner.IndexOf('Read-ProbePathPolicy $script:MigrationProbe') -lt $runner.IndexOf('New-Item'))
    foreach ($file in @('common.ps1', 'build.ps1', 'migration-common.ps1', 'run-migration.ps1', 'test-migration-offline.ps1')) {
        $path = Join-Path $PSScriptRoot $file
        $bytes = [IO.File]::ReadAllBytes($path)
        Check ($file + ' ASCII') (@($bytes | Where-Object { $_ -gt 127 }).Count -eq 0)
        $parseTokens = $null; $parseErrors = $null
        $ast = [Management.Automation.Language.Parser]::ParseFile($path, [ref]$parseTokens, [ref]$parseErrors)
        Check ($file + ' syntax') ($parseErrors.Count -eq 0)
        if ($file -in @('migration-common.ps1', 'run-migration.ps1')) {
            Check ($file + ' no forced termination') (-not ([IO.File]::ReadAllText($path) -match '\.Kill\s*\(|Stop-Process|taskkill|TerminateProcess'))
        }
    }
    foreach ($p in @('EBWebView/lockfile', 'EBWebView/Crashpad', 'EBWebView/Crashpad/reports/one.dmp',
        'EBWebView/CrashpadMetrics.pma', 'EBWebView/CrashpadMetrics-active.pma',
        'EBWebView/BrowserMetrics/a.pma', 'EBWebView/DeferredBrowserMetrics/a.pma',
        'EBWebView/EBWebViewMetrics/a.pma', 'ebwebview\crashpad\a')) {
        Check ('exact exclusion ' + $p) (Test-MigrationExcluded $p)
    }
    foreach ($p in @('EBWebView/Default/Preferences', 'EBWebView/Local State', 'EBWebView/Crashpad-copy/a',
        'EBWebView/CrashpadMetrics-other.pma', 'EBWebView/Default/Crashpad/a',
        'EBWebView/BrowserMetrics-not/a', 'BrowserMetrics/a', 'EBWebView/lockfile-backup')) {
        Check ('preserve unknown ' + $p) (-not (Test-MigrationExcluded $p))
    }
    $fixture = Join-Path $script:MigrationRun 'fixture'
    [IO.Directory]::CreateDirectory((Join-Path $fixture 'EBWebView\Default')) | Out-Null
    [IO.Directory]::CreateDirectory((Join-Path $fixture 'EBWebView\Crashpad')) | Out-Null
    [IO.File]::WriteAllText((Join-Path $fixture 'EBWebView\Default\Preferences'), '{"synthetic":true}')
    [IO.File]::WriteAllText((Join-Path $fixture 'EBWebView\Crashpad\ignored'), 'metrics')
    $manifest = Get-MigrationManifest $fixture
    Check 'raw manifest preserves excluded metric evidence' ($manifest.raw.Count -eq 2 -and $manifest.excluded.Count -eq 1)
    Check 'acceptance manifest contains only included data' ($manifest.included.Count -eq 1)
    $blocked = $false
    try { Assert-MigrationExclusions $fixture } catch { $blocked = $true }
    Check 'destination exclusion assertions reject directory' $blocked
    $clean = Join-Path $script:MigrationRun 'clean'
    [IO.Directory]::CreateDirectory($clean) | Out-Null
    Assert-MigrationExclusions $clean
    Check 'clean destination assertions pass' $true
    $lockPath = Join-Path $fixture 'EBWebView\lockfile'
    $lockFile = [IO.File]::Open($lockPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try { Check 'independent lock conflict verified' (Assert-MigrationHeldLock $fixture).held }
    finally { $lockFile.Dispose() }
    $blocked = $false
    try { $null = Assert-MigrationHeldLock $fixture } catch { $blocked = $true }
    Check 'unheld lock is rejected' $blocked
    $blocked = $false
    try { $null = Assert-MigrationHeldLock $clean } catch { $blocked = $true }
    Check 'missing lock is not a successful conflict' $blocked
    $missingHash = @([pscustomobject]@{path='x';size=1;sha256=$null;read_error='synthetic denied'})
    Check 'read errors never compare equal' (-not (Compare-Manifests $missingHash $missingHash).equal)
    $quoted = Join-MigrationArguments @('--miv-sound-migrate-only', 'C:\path with spaces\source')
    Check 'Windows argument quoting round trips' (([ProbeCommandLine]::Split('fake.exe ' + $quoted))[2] -ceq 'C:\path with spaces\source')
    foreach ($bad in @('x"y', "x`ny", 'x\')) {
        $blocked = $false
        try { $null = Join-MigrationArguments @($bad) } catch { $blocked = $true }
        Check 'reject unsupported argument' $blocked
    }
    # A real, finite PowerShell child verifies asynchronous JSONL draining and
    # checkpoint signaling. It has no browser API or product migration code.
    $childScript = Join-Path $script:MigrationRun 'synthetic-child.ps1'
    [IO.File]::WriteAllText($childScript, @'
param([string]$Checkpoints)
$ErrorActionPreference = 'Stop'
foreach ($phase in @('lock_acquired', 'copy_begin', 'copy_end')) {
    [Console]::Out.WriteLine('{"phase":"' + $phase + '","elapsed_ms":0,"synthetic":true}')
    [Console]::Out.Flush()
    $clock = [Diagnostics.Stopwatch]::StartNew()
    while (-not [IO.File]::Exists((Join-Path $Checkpoints ($phase + '.continue')))) {
        if ($clock.ElapsedMilliseconds -gt 8000) { exit 3 }
        Start-Sleep -Milliseconds 10
    }
    if ($phase -eq 'lock_acquired') {
        [Console]::Out.WriteLine('{"phase":"process_wait_begin","elapsed_ms":0,"synthetic":true}')
        [Console]::Out.WriteLine('{"phase":"process_wait_end","elapsed_ms":0,"synthetic":true}')
        [Console]::Out.Flush()
    }
}
[Console]::Out.WriteLine('{"phase":"result","elapsed_ms":0,"passed":true,"synthetic":true}')
[Console]::Out.Flush()
'@, [Text.Encoding]::ASCII)
    $ps = Join-Path $PSHOME 'powershell.exe'
    $child = Start-MigrationChild $ps @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
        $childScript, '-Checkpoints', $script:MigrationCheckpoints) 'synthetic-helper' -Helper
    foreach ($phase in @('lock_acquired', 'copy_begin', 'copy_end')) {
        $event = Wait-MigrationPhase $child $phase 5000
        Check ('synthetic checkpoint ' + $phase) ($event.synthetic -and -not $child.process.HasExited)
        Resume-MigrationPhase $phase
    }
    Wait-MigrationExit $child 5000
    Check 'synthetic helper naturally exits' ($child.process.ExitCode -eq 0)
    $events = @(Read-MigrationStdout $child)
    Check 'all helper events retained' ($events.Count -eq 6)
    Assert-MigrationHelperEvents $events
    Check 'complete ordered helper result accepted' $true
    $blocked = $false
    try { Assert-MigrationHelperEvents @($events | Where-Object phase -ne 'process_wait_end') } catch { $blocked = $true }
    Check 'missing process wait completion rejected' $blocked
    $blocked = $false
    try { Assert-MigrationHelperEvents @($events[0], $events[2], $events[1], $events[3], $events[4], $events[5]) } catch { $blocked = $true }
    Check 'out-of-order phase completion rejected' $blocked
    $events[-1].passed = $false
    $blocked = $false
    try { Assert-MigrationHelperEvents $events } catch { $blocked = $true }
    Check 'failed helper result rejected' $blocked
    Check 'completed stdout recorded' ([IO.File]::Exists(($child.log + '.stdout.txt')))
    $noPhase = $false
    try { $null = Wait-MigrationPhase $child 'not-emitted' 25 } catch { $noPhase = $true }
    Check 'missing phase never reports success' $noPhase
    $finite = Start-MigrationChild $ps @('-NoProfile', '-Command', 'Start-Sleep -Milliseconds 400') 'synthetic-finite' -Helper
    $timedOut = $false
    try { Wait-MigrationExit $finite 10 } catch { $timedOut = $true }
    Check 'deadline fails without terminating child' ($timedOut -and -not $finite.process.HasExited)
    Wait-MigrationExit $finite 5000
    Check 'timed-out child later exits naturally' ($finite.process.ExitCode -eq 0)
    $log = Join-Path $script:MigrationRun 'synthetic-host.jsonl'
    [IO.File]::WriteAllText($log, "{`"kind`":`"shutdown_complete`",`"complete`":true}`n")
    $syntheticHost = [pscustomobject]@{ log=$log; label='synthetic'; process=[pscustomobject]@{ExitCode=0} }
    Assert-MigrationHostExit $syntheticHost
    Check 'complete host shutdown accepted' $true
    [IO.File]::AppendAllText($log, "{`"kind`":`"host_error`"}`n")
    $blocked = $false
    try { Assert-MigrationHostExit $syntheticHost } catch { $blocked = $true }
    Check 'host error fails even after complete close' $blocked
    $blocked = $false
    try { Assert-ProbePath ([IO.Path]::GetTempPath()) } catch { $blocked = $true }
    Check 'outside disposable root rejected' $blocked
} catch { $errorText = $_.Exception.ToString() }
finally {
    $cleanup = Complete-MigrationCleanup
    Save-Json (Join-Path $script:MigrationRun 'offline-results.json') ([ordered]@{
        label='SYNTHETIC_NOT_A_RESULT'; pass=($null -eq $errorText -and $cleanup.Count -eq 0);
        checks=@($checks); error=$errorText; cleanup_errors=$cleanup; webview2_started=$false; product_started=$false })
}
if ($errorText -or $cleanup.Count) { throw "Migration offline checks failed: $errorText" }
Write-Host ("Migration offline checks passed: $($checks.Count) -- $script:MigrationRun")
