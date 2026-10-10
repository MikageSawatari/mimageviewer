# Sequential, disposable suite; never uses process-name activation or global instance IPC.
[CmdletBinding()]
param([switch]$InteractiveApproved, [switch]$SkipBuild, [switch]$IncludeFirstAi,
    [ValidateRange(30,3600)][int]$TimeoutSeconds=360)
$ErrorActionPreference='Stop'
Set-StrictMode -Version 2
if (-not $InteractiveApproved) { [Console]::Error.WriteLine('ReleaseChecks requires explicit interactive approval.'); exit 2 }
. (Join-Path $PSScriptRoot 'Support.ps1')
$repo=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$scripts=Join-Path $repo 'scripts'
Import-ReleaseCheckFunctions (Join-Path $scripts 'ui-smoke.ps1') @('Get-NormalizedPath','Assert-ExactPath','Assert-NoReparsePath','Assert-NoReparseTree','Join-NativeArguments','Quote-NativeArgument','Open-UiSmokeRunnerLock')
Import-ReleaseCheckFunctions (Join-Path $scripts 'build-portable.ps1') @('Get-MivOrdinalUniquePaths','Get-MivSourceFingerprintRecords','Get-MivSourceFingerprint','Assert-SmokeBuildManifest')
$smoke=Join-Path $repo 'target\portable-smoke'
$exe=Join-Path $smoke 'mimageviewer.exe'; $data=Join-Path $smoke 'data'
$run=Join-Path $repo ('target\ui-smoke\runs\release-checks-'+[DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss')+'-'+[Guid]::NewGuid().ToString('N').Substring(0,8))
$preflight=Invoke-ReleaseCheckPreflight {
Assert-ReleaseCheckPaths $repo $exe $data $run
Initialize-ReleaseCheckNative
# Read-only preflight happens before prepare/build/fixture generation.
[MivReleaseCheckNative]::CheckDesktop()
Assert-NoReparsePath (Join-Path $repo 'target') $repo 'release-lock'
New-Item -ItemType Directory -Path (Join-Path $repo 'target\ui-smoke') -Force | Out-Null
Open-UiSmokeRunnerLock (Join-Path $repo 'target\ui-smoke\runner.lock')
}
if($preflight.exit_code -ne 0){
    # No unsafe run path is written, and desktop/lock failures are never exit 1.
    [Console]::Error.WriteLine(('ReleaseChecks SKIPPED: '+$preflight.environment_error))
    exit 2
}
$lock=$preflight.value
$cases=New-Object 'Collections.Generic.List[object]'
$owned=$null; $ownedStart=$null; $background=$null; $activeDir=$null
$metadata=[ordered]@{suite='ReleaseChecks'; include_first_ai=[bool]$IncludeFirstAi; run_directory=$run; approval_scope='5 idle + 2 perf + stopped-profile seed sessions'; started_utc=[DateTime]::UtcNow.ToString('o'); os=[Environment]::OSVersion.VersionString; window_size=@(1328,900); dpi_by_case=@{}; cpu_names=@(); gpu_names=@()}

function Assert-NoSmokeProcess {
    foreach($p in (Get-Process -ErrorAction Stop)) {
        if($p.ProcessName -notin @('mimageviewer','mimageviewer-core','mimageviewer-remote','mimageviewer-epub-worker')){continue}
        try{$path=$p.Path}catch{throw 'Cannot inspect an existing mIV process; no replacement is safe.'}
        if(-not $path){throw 'Cannot inspect an existing mIV process; no replacement is safe.'}
        if((Get-NormalizedPath $path).StartsWith((Get-NormalizedPath $smoke)+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Another smoke process owns the disposable tree.'}
    }
}
function Stop-OwnedReleaseProcess {
    if($null -eq $script:owned){return}
    $script:owned.Refresh()
    if(-not $script:owned.HasExited){
        $c=Get-CimInstance Win32_Process -Filter "ProcessId=$($script:owned.Id)" -ErrorAction Stop
        Assert-ReleaseCheckIdentity $script:owned $c $script:owned.Id $script:ownedStart $exe $data
        Stop-Process -Id $script:owned.Id -Force -ErrorAction Stop
        $script:owned.WaitForExit()
    }
    $script:owned=$null
}
function Test-OwnedReleaseProcessAlive {
    # Normal Quit may finish between the loop condition and the WMI lookup.
    # The owned Process handle distinguishes that exit from a reused numeric PID.
    $script:owned.Refresh()
    if($script:owned.HasExited){return $false}
    try{
        $c=Get-CimInstance Win32_Process -Filter "ProcessId=$($script:owned.Id)" -ErrorAction Stop
        Assert-ReleaseCheckIdentity $script:owned $c $script:owned.Id $script:ownedStart $exe $data
        return $true
    }catch{
        $script:owned.Refresh()
        if($script:owned.HasExited){return $false}
        throw
    }
}
function Save-ReleaseLogs {
    param([string]$Directory)
    Assert-NoReparseTree (Join-Path $data 'logs') 'release-logs'
    foreach($name in @('perf_events.jsonl','mimageviewer.log')){
        $source=Join-Path $data ('logs\'+$name)
        if(Test-Path -LiteralPath $source -PathType Leaf){Copy-Item -LiteralPath $source -Destination (Join-Path $Directory $name)}
    }
}
function Restore-ReleaseSeed {
    param([string]$Seed)
    Assert-NoSmokeProcess
    Assert-ReleaseCheckPaths $repo $exe $data $run
    Assert-NoReparseTree $Seed 'release-seed'
    $null=Assert-ExactPath $data (Join-Path $repo 'target\portable-smoke\data') 'release-restore'
    Remove-Item -LiteralPath $data -Recurse -Force
    Copy-Item -LiteralPath $Seed -Destination $data -Recurse
    Assert-ReleaseCheckArtifact $repo
}
function Complete-ReleaseSession {
    param([string]$CaseId,[string]$Directory)
    $script:owned.WaitForExit();$exitCode=$script:owned.ExitCode
    $processPid=$script:owned.Id;$script:owned=$null
    $exitResult=New-ReleaseCheckProcessExitResult $CaseId $exitCode $processPid $Directory
    # Record the process verdict before any fallible archive/evidence operation.
    # A later environment error cannot erase ScriptFailure or a crash.
    if($null -ne $exitResult){$cases.Add($exitResult)}
    Save-ReleaseLogs $Directory
    if($null -eq $exitResult){
        $caseEvidence=Join-Path $Directory 'case-evidence.json'
        if(-not (Test-Path -LiteralPath $caseEvidence)){throw "$CaseId missing normal completion evidence."}
        $evidence=Get-Content -LiteralPath $caseEvidence -Encoding UTF8 -Raw | ConvertFrom-Json
        $evidence | Add-Member -MemberType NoteProperty -Name exit -Value @{process_exited=$true;forced=$false;process_exit_code=$exitCode} -Force
        Write-ReleaseCheckJson $caseEvidence $evidence
    }
    return [pscustomobject]@{directory=$Directory;pid=$processPid;process_exit_code=$exitCode;exit_result=$exitResult}
}
function Invoke-ReleaseSession {
    param([string]$CaseId,[string]$Overlay,[bool]$Idle)
    Assert-NoSmokeProcess
    Assert-ReleaseCheckArtifact $repo
    $dir=Join-Path $run $CaseId
    New-Item -ItemType Directory -Path $dir | Out-Null
    $script:activeDir=$dir
    & python (Join-Path $scripts 'ui-smoke\release_checks_preflight.py') --manifest $fixtureManifest --data-dir $data --case-id $CaseId --json-out (Join-Path $dir 'pre-launch-cache.json') --stopped
    if($LASTEXITCODE -ne 0){throw 'Stopped-profile cache proof unavailable.'}
    $scriptPath=Join-Path $dir 'scenario.rhai'
    $template=Get-Content -LiteralPath (Join-Path $scripts 'ui-smoke\release-checks.rhai') -Encoding UTF8 -Raw
    $template=$template.Replace('__CASE_ID__',$CaseId).Replace('__CASE_DIRECTORY__',$dir.Replace('\','/')).Replace('__FIXTURE_ROOT__',$fixture.Replace('\','/')).Replace('__INCLUDE_FIRST_AI__',([bool]$IncludeFirstAi).ToString().ToLowerInvariant())
    $template=$template.Replace('__RUNNER_PID__',([string]$PID))
    $mode=if($CaseId -eq 'perf-uncached'){'uncached'}else{'cached'}
    $many=if($CaseId -eq 'tray-close-while-loading'){$m.paths.'tray-while'}elseif($CaseId -eq 'tray-close-after-loading'){$m.paths.'tray-after'}else{$m.paths.$mode}
    foreach($token in @{
        '__STATIC_PATH__'=$m.paths.static; '__MANY_PATH__'=$many; '__VIDEO_CHILD__'=$m.video_pin.child;
        '__VIDEO_SOURCE__'=$m.video_pin.source; '__VIDEO_PARENT__'=$m.video_pin.parent; '__SEARCH_TERM__'=$m.search.term
    }.GetEnumerator()){
        if([string]$token.Value -match '["\r\n]'){throw 'Unsafe generated Rhai literal.'}
        $template=$template.Replace($token.Key,([string]$token.Value).Replace('\','/'))
    }
    $template=$template.Replace('__SEARCH_HITS__',(@($m.search.expected_hits)|ConvertTo-Json -Compress))
    $seedRecipe=@($m.seed_requirements.sessions | Where-Object {$_.case_id -eq $CaseId})
    $cachePaths=if($seedRecipe.Count -eq 1){@($seedRecipe[0].cache_paths)}else{@($m.seed_requirements.cache.prepare)}
    $template=$template.Replace('__CACHE_PATHS__',(ConvertTo-Json -InputObject @($cachePaths) -Compress))
    $template=$template.Replace('__NAVIGATION_PATHS__',(@($m.perf.$mode.navigation_order)|ConvertTo-Json -Compress))
    $template=$template.Replace('__LOAD_PATHS__',(ConvertTo-Json -InputObject @($m.perf.$mode.load_order) -Compress))
    [IO.File]::WriteAllText($scriptPath,$template,(New-Object Text.UTF8Encoding($false)))
    $sessionOverlay=Get-Content -LiteralPath $Overlay -Encoding UTF8 -Raw | ConvertFrom-Json
    # Perf measures the real initial many-folder startup shape. Other roots
    # remain untouched until the recipe opens them after indexing settles.
    $sessionOverlay.startup_folder_path=if($seedRecipe.Count -eq 1){$seedRecipe[0].startup_path}elseif($CaseId -like 'perf-*'){$many}else{$m.paths.static}
    Write-ReleaseCheckJson (Join-Path $dir 'settings-override.json') $sessionOverlay
    $arguments=@('--data-dir',$data,'--perf-log','--test-script',$scriptPath,'--test-evidence-dir',$dir,'--settings-override',(Join-Path $dir 'settings-override.json'),'--window-size','1328x900')
    $script:owned=Start-Process -FilePath $exe -ArgumentList (Join-NativeArguments $arguments) -WorkingDirectory $smoke -PassThru
    $null=$script:owned.Handle  # cache the handle so ExitCode is available after exit
    $script:ownedStart=$script:owned.StartTime.ToUniversalTime()
    Write-ReleaseCheckJson (Join-Path $dir 'process.json') @{pid=$script:owned.Id;start_time_utc=$script:ownedStart.ToString('o');exe=$exe;data_dir=$data}
    $timer=[Diagnostics.Stopwatch]::StartNew(); $focusTimer=[Diagnostics.Stopwatch]::StartNew(); $ready=Join-Path $dir 'ready.json';$focusAcquired=$false
    while(-not $script:owned.HasExited){
        if(-not (Test-OwnedReleaseProcessAlive)){break}
        if($timer.Elapsed.TotalSeconds -gt $TimeoutSeconds){throw "$CaseId session deadline exceeded."}
        if(-not $focusAcquired -and $focusTimer.Elapsed.TotalSeconds -le 20 -and $script:owned.MainWindowHandle -ne [IntPtr]::Zero){
            $null=[MivReleaseCheckNative]::SetForegroundWindow($script:owned.MainWindowHandle)
            $fg=Get-ReleaseCheckWindowSample $script:owned.Id
            $focusAcquired=$fg.foreground_pid -eq $script:owned.Id -and $fg.foreground_hwnd -eq $script:owned.MainWindowHandle.ToInt64()
        }
        if(-not $focusAcquired -and $focusTimer.Elapsed.TotalSeconds -gt 20){throw 'Exact root foreground unavailable within 20 seconds.'}
        if($Idle -and (Test-Path -LiteralPath $ready -PathType Leaf)){
            $r=Get-Content -LiteralPath $ready -Encoding UTF8 -Raw | ConvertFrom-Json
            if($r.pid -ne $script:owned.Id){throw 'Ready PID mismatch.'}
            [uint32]$hwndPid=0
            $null=[MivReleaseCheckNative]::GetWindowThreadProcessId([IntPtr][long]$r.root_hwnd,[ref]$hwndPid)
            if($hwndPid -ne $script:owned.Id){throw 'Ready root HWND owner mismatch.'}
            $metadata.dpi_by_case[$CaseId]=[MivReleaseCheckNative]::GetDpiForWindow([IntPtr][long]$r.root_hwnd)
            $backgroundHwnd=0
            if($CaseId.Contains('background')){
                Add-Type -AssemblyName System.Windows.Forms
                $script:background=New-Object Windows.Forms.Form
                $script:background.Text='mIV disposable release-check background';$script:background.Width=480;$script:background.Height=160
                $foregroundProof=Set-ReleaseCheckBackgroundForeground $r $script:owned.Id $script:background $PID
                $backgroundHwnd=$foregroundProof.foreground_hwnd
                Write-ReleaseCheckJson (Join-Path $dir 'background-ready.json') $foregroundProof
            } elseif($CaseId -eq 'static-foreground'){
                $null=[MivReleaseCheckNative]::SetForegroundWindow([IntPtr][long]$r.root_hwnd)
            }
            $scenario=if($CaseId.StartsWith('tray-')){'tray-residency'}else{$CaseId}
            $samplerArgs=@('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',(Join-Path $scripts 'check-idle-health.ps1'),'-PortableSmoke','-InteractiveApproved','-SkipPrompt','-Scenario',$scenario,'-CaseId',$CaseId,'-ProcessId',[string]$script:owned.Id,'-ExpectedStartTimeUtc',$script:ownedStart.ToString('o'),'-SleepEvidencePath',(Join-Path $dir 'completion.json'),'-OutputDirectory',$dir,'-RootHwnd',[string]$r.root_hwnd,'-BackgroundHwnd',[string]$backgroundHwnd)
            if($backgroundHwnd -ne 0){$samplerArgs+=@('-BackgroundProcessId',[string]$PID)}
            if($CaseId -eq 'video-pin-background'){$samplerArgs+=@('-TargetKey',[string]$r.target_key)}
            # Separate host preserves the suite even when the sampler exits.
            $hostExe=[Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
            $sampler=Start-Process -FilePath $hostExe -ArgumentList (Join-NativeArguments $samplerArgs) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $dir 'sampler.stdout.log') -RedirectStandardError (Join-Path $dir 'sampler.stderr.log')
            $null=$sampler.Handle  # cache the handle so ExitCode is available after exit
            while(-not $sampler.HasExited){
                if($script:background){[Windows.Forms.Application]::DoEvents()}
                if($timer.Elapsed.TotalSeconds -gt $TimeoutSeconds){Stop-Process -Id $sampler.Id -Force;throw 'Sampler deadline exceeded.'}
                Start-Sleep -Milliseconds 100;$sampler.Refresh()
            }
            Write-ReleaseCheckJson (Join-Path $dir 'sampler-finished.json') @{exit_code=$sampler.ExitCode}
            if($script:background){$script:background.Close();$script:background.Dispose();$script:background=$null}
            $Idle=$false
        }
        Start-Sleep -Milliseconds 100
    }
    return Complete-ReleaseSession $CaseId $dir
}
try {
    New-Item -ItemType Directory -Path $run | Out-Null
    Assert-NoSmokeProcess
    $metadata.cpu_names=@(Get-CimInstance Win32_Processor -ErrorAction Stop | ForEach-Object {$_.Name})
    $metadata.gpu_names=@(Get-CimInstance Win32_VideoController -ErrorAction Stop | ForEach-Object {$_.Name})
    $prepare=@('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',(Join-Path $scripts 'prepare-portable-smoke.ps1'),'-TestScript')
    if($SkipBuild){$prepare+='-SkipBuild'}
    $prepareProcess=Start-Process -FilePath ([Diagnostics.Process]::GetCurrentProcess().MainModule.FileName) -ArgumentList (Join-NativeArguments $prepare) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $run 'prepare.stdout.log') -RedirectStandardError (Join-Path $run 'prepare.stderr.log')
    $null=$prepareProcess.Handle  # cache the handle so ExitCode is available after exit
    $prepareProcess.WaitForExit()
    if($prepareProcess.ExitCode -ne 0){throw 'Portable preparation failed.'}
    Assert-ReleaseCheckArtifact $repo
    $metadata.artifact=Get-Content -LiteralPath (Join-Path $smoke '.test-script-build.json') -Encoding UTF8 -Raw | ConvertFrom-Json
    $metadata.source=$metadata.artifact.source_head+'/'+$metadata.artifact.source_fingerprint_sha256
    $metadata.exe_sha256=$metadata.artifact.core_sha256
    $metadata.run_path=$run
    $metadata.thresholds_version=(Get-FileHash -LiteralPath (Join-Path $scripts 'idle_health_thresholds.json') -Algorithm SHA256).Hash.ToLowerInvariant()
    Copy-Item -LiteralPath (Join-Path $smoke '.test-script-build.json') -Destination (Join-Path $run 'test-script-build.json')
    $fixture=Join-Path $run 'fixture'
    $ffmpeg=(Get-Command ffmpeg.exe -ErrorAction Stop).Source
    & python (Join-Path $scripts 'ui-smoke\generate_release_checks_fixture.py') --output $fixture --ffmpeg $ffmpeg
    if($LASTEXITCODE -ne 0){throw 'Fixture generation failed.'}
    & python (Join-Path $scripts 'ui-smoke\generate_release_checks_fixture.py') --output $fixture --verify
    if($LASTEXITCODE -ne 0){throw 'Generated fixture integrity check failed.'}
    $fixtureManifest=Join-Path $fixture 'fixture-manifest.json'
    $m=Get-Content -LiteralPath $fixtureManifest -Encoding UTF8 -Raw | ConvertFrom-Json
    $metadata.fixture_manifest=$fixtureManifest
    $metadata.fixture_version=$m.recipe_version
    # Seed each geometry in a normal session, retaining the same isolated data.
    # No case clone is made until both sessions have exited and flushed.
    foreach($seedRecipe in $m.seed_requirements.sessions){
        $seedSession=Invoke-ReleaseSession $seedRecipe.case_id $m.overlays.($seedRecipe.overlay) $false
        if($null -ne $seedSession.exit_result){throw 'Seed session did not complete successfully; process verdict retained.'}
        $seedEvents=@(Get-Content -LiteralPath (Join-Path $seedSession.directory 'perf_events.jsonl') -Encoding UTF8 | ForEach-Object {try{$_|ConvertFrom-Json}catch{}})
        if(-not @($seedEvents|Where-Object {$_.cat -eq 'qa' -and $_.kind -eq 'on_exit'}).Count){throw 'Seed normal on_exit/flush proof missing.'}
        Assert-NoSmokeProcess
        Assert-NoReparseTree $data 'release-seed-source'
        $seedEvidencePath=Join-Path $seedSession.directory 'case-evidence.json'
        $seedEvidence=Get-Content -LiteralPath $seedEvidencePath -Encoding UTF8 -Raw | ConvertFrom-Json
        $seedEvidence|Add-Member -MemberType NoteProperty -Name seed_phase -Value 'preparation' -Force
        $seedEvidence|Add-Member -MemberType NoteProperty -Name seed_kinds -Value @($seedRecipe.seed_kinds) -Force
        $seedEvidence|Add-Member -MemberType NoteProperty -Name seed_cache_paths -Value @($seedRecipe.cache_paths) -Force
        # Mark only after the stopped profile has actually been copied.
        $seedCheckpoint=Join-Path $run ($seedRecipe.case_id+'-stopped')
        Copy-Item -LiteralPath $data -Destination $seedCheckpoint -Recurse
        $seedEvidence|Add-Member -MemberType NoteProperty -Name copied_after_process_exit -Value $true -Force
        Write-ReleaseCheckJson $seedEvidencePath $seedEvidence
        & python (Join-Path $scripts 'release_check_analysis.py') evidence $seedEvidencePath --jsonl (Join-Path $seedSession.directory 'perf_events.jsonl') --expected-pid $seedSession.pid --json-out (Join-Path $seedSession.directory 'seed-check.json')
        $seedExit=$LASTEXITCODE
        $cases.Add((Join-Path $seedSession.directory 'seed-check.json'))
        if($seedExit -ne 0){throw 'Normal seed cache/pin/index persistence was not proven.'}
    }
    $seed=Join-Path $run 'seed-perf-cached-stopped'
    $caseIds=@('static-foreground','static-background','video-pin-background','tray-close-while-loading','tray-close-after-loading','perf-cached','perf-uncached')
    if($IncludeFirstAi){$caseIds+='first-ai';$metadata.approval_scope+=' + isolated first AI'}
    foreach($id in $caseIds){
        Restore-ReleaseSeed $seed
        $overlay=if($id -eq 'first-ai'){$m.overlays.'static-foreground'}else{$m.overlays.$id}
        $session=Invoke-ReleaseSession $id $overlay ($id -notlike 'perf-*' -and $id -ne 'first-ai')
        if($null -ne $session.exit_result){break}
        $resultPath=Join-Path $session.directory 'case.json'
        if($id -like 'perf-*' -or $id -eq 'first-ai'){
            & python (Join-Path $scripts 'analyze_perf.py') (Join-Path $session.directory 'perf_events.jsonl') perf-smoke --case-id $id --expected-pid $session.pid --case-evidence (Join-Path $session.directory 'case-evidence.json') --fixture-manifest $fixtureManifest --json-out $resultPath
        }else{
            & python (Join-Path $scripts 'release_check_analysis.py') idle-case (Join-Path $session.directory 'perf_events.jsonl') --case-id $id --expected-pid $session.pid --sampler (Join-Path $session.directory 'sampler.json') --case-evidence (Join-Path $session.directory 'case-evidence.json') --json-out $resultPath
        }
        $verdictExit=$LASTEXITCODE
        if(-not (Test-Path -LiteralPath $resultPath)){throw "$id analysis did not produce evidence."}
        $cases.Add($resultPath)
        if(-not (Test-ReleaseCheckContinueAfterAnalysis $id $verdictExit)){break}
    }
}catch{
    $cases.Add([ordered]@{case_id='suite-preparation';verdict='SKIPPED';exit_code=2;environment_errors=@($_.Exception.Message);failures=@();warnings=@()})
}finally{
    try{Stop-OwnedReleaseProcess}catch{$cases.Add([ordered]@{case_id='cleanup';verdict='SKIPPED';exit_code=2;environment_errors=@($_.Exception.Message)})}
    if($script:activeDir){try{Save-ReleaseLogs $script:activeDir}catch{$cases.Add([ordered]@{case_id='archive';verdict='SKIPPED';exit_code=2;environment_errors=@($_.Exception.Message)})}}
    if($script:background){$script:background.Close();$script:background.Dispose()}
    $lock.Dispose()
}
$metadata.environment=@{os=$metadata.os;cpu=$metadata.cpu_names;gpu=$metadata.gpu_names;window_size=$metadata.window_size;dpi_by_case=$metadata.dpi_by_case}
Write-ReleaseCheckJson (Join-Path $run 'suite-input.json') @{metadata=$metadata;cases=@($cases.ToArray())}
& python (Join-Path $scripts 'release_check_analysis.py') summary (Join-Path $run 'suite-input.json') --json-out (Join-Path $run 'summary.json') --markdown-out (Join-Path $run 'release-record.md')
Write-Host "ReleaseChecks evidence: $run"
exit $LASTEXITCODE
