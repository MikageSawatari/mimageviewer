# Noninteractive safety regressions: fake processes and argument parsing only.
# No apps, helpers, foreground operations, builds or real process commands run.
[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
Set-StrictMode -Version 2
. (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Support.ps1')
function Assert-True {param([bool]$Condition,[string]$Message) if(-not $Condition){throw $Message}}
function Assert-Throws {param([scriptblock]$Action,[string]$Message) $threw=$false;try{& $Action}catch{$threw=$true};Assert-True $threw $Message}
function Get-TestAst {
    param([string]$Path)
    $tokens=$null;$errors=$null
    $ast=[Management.Automation.Language.Parser]::ParseFile($Path,[ref]$tokens,[ref]$errors)
    Assert-True ($errors.Count -eq 0) "Parse errors: $Path"
    return $ast
}
$repo=Split-Path $PSScriptRoot -Parent
Import-ReleaseCheckFunctions (Join-Path $PSScriptRoot 'ui-smoke.ps1') @('Get-NormalizedPath','Assert-ExactPath','Assert-NoReparsePath','Assert-NoReparseTree')
Assert-Throws {Assert-ReleaseCheckApproval $false} 'Missing approval was accepted.'
foreach($reason in @('path refused','desktop unavailable','runner lock busy')){
    $preflight=Invoke-ReleaseCheckPreflight {throw $reason}
    Assert-True ($preflight.exit_code -eq 2 -and $preflight.environment_error -eq $reason -and $null -eq $preflight.value) 'Preflight failure escaped SKIPPED/2 classification.'
}
$preflight=Invoke-ReleaseCheckPreflight {'fake-owned-lock'}
Assert-True ($preflight.exit_code -eq 0 -and $preflight.value -eq 'fake-owned-lock') 'Successful preflight lost its lock.'
$exe=Join-Path $repo 'target\portable-smoke\mimageviewer.exe';$data=Join-Path $repo 'target\portable-smoke\data';$output=Join-Path $repo 'target\ui-smoke\runs\fake-release-check'
# Mock filesystem observers: the tests do not read real application/profile data.
function Assert-NoReparsePath {param($Path,$StopAt,$Label)}
function Assert-NoReparseTree {param($Path,$Label)}
Assert-ReleaseCheckPaths $repo $exe $data $output
Assert-Throws {Assert-ReleaseCheckPaths $repo (Join-Path $repo 'target\release\mimageviewer.exe') $data $output} 'Normal executable accepted.'
Assert-Throws {Assert-ReleaseCheckPaths $repo $exe (Join-Path $repo 'fake-normal-profile') $output} 'Other profile accepted.'
Assert-Throws {Assert-ReleaseCheckPaths $repo $exe $data (Join-Path $repo 'target\ui-smoke\runs-other\fake')} 'Sibling output prefix accepted.'
Assert-Throws {Assert-ReleaseCheckPaths $repo $exe $data (Join-Path $repo 'target\ui-smoke\runs')} 'Run root itself accepted.'
if(-not ('MivReleaseCheckNative' -as [type])){
    # Fake command-line parser; no native imports and no real process queries.
    Add-Type @'
using System.Text.RegularExpressions;
public static class MivReleaseCheckNative {
 public static uint BackgroundOwner=202;
 public static bool ActivationSucceeds=true;
 public static int ActivationCalls=0;
 public static uint GetWindowThreadProcessId(System.IntPtr h,out uint p){p=BackgroundOwner;return 1;}
 public static bool SetForegroundWindow(System.IntPtr h){ActivationCalls++;return ActivationSucceeds;}
 public static string[] Arguments(string s) {var m=Regex.Matches(s,"\"([^\"]*)\"|([^ ]+)");var a=new string[m.Count];for(int i=0;i<a.Length;i++)a[i]=m[i].Groups[1].Success?m[i].Groups[1].Value:m[i].Groups[2].Value;return a;}
}
'@
}
$start=[datetime]'2026-10-09T00:00:00Z'
$fake=[pscustomobject]@{Id=101;Path=$exe;StartTime=$start;HasExited=$false}
$line='"'+$exe+'" --perf-log --test-script "fake.rhai" --settings-override "fake.json" --data-dir "'+$data+'"'
$cim=[pscustomobject]@{ProcessId=101;ExecutablePath=$exe;CommandLine=$line}
Assert-ReleaseCheckIdentity $fake $cim 101 $start $exe $data
Assert-Throws {Assert-ReleaseCheckIdentity $fake $cim 102 $start $exe $data} 'Wrong PID accepted.'
Assert-Throws {Assert-ReleaseCheckIdentity $fake $cim 101 $start.AddSeconds(1) $exe $data} 'Reused PID/start time accepted.'
$cim.CommandLine=$line.Replace('--data-dir','--other-data-dir')
Assert-Throws {Assert-ReleaseCheckIdentity $fake $cim 101 $start $exe $data} 'Substring data argument accepted.'
$cim.CommandLine=$line+' --data-dir "'+$data+'"'
Assert-Throws {Assert-ReleaseCheckIdentity $fake $cim 101 $start $exe $data} 'Duplicate data override accepted.'
$cim.CommandLine=$line;$fake.HasExited=$true
Assert-Throws {Assert-ReleaseCheckIdentity $fake $cim 101 $start $exe $data} 'Exited PID accepted.'
$sample=[pscustomobject]@{foreground_pid=101;foreground_hwnd=888;total=2;visible=1}
Assert-ReleaseCheckWindowSamples @($sample) 'static-foreground' 101 888 0
Assert-Throws {Assert-ReleaseCheckWindowSamples @($sample) 'static-foreground' 101 889 0} 'Other same-PID HWND accepted.'
Assert-Throws {Assert-ReleaseCheckWindowSamples @($sample) 'static-background' 101 888 999} 'App foreground accepted as background.'
$sample.foreground_pid=202;$sample.foreground_hwnd=999
Assert-ReleaseCheckWindowSamples @($sample) 'static-background' 101 888 999 202
Assert-Throws {Assert-ReleaseCheckWindowSamples @($sample) 'static-background' 101 888 998 202} 'User window accepted instead of runner-owned window.'
Assert-Throws {Assert-ReleaseCheckWindowSamples @($sample) 'static-background' 101 888 999 203} 'Reused runner HWND owned by another PID was accepted.'
& {
    $ready=[pscustomobject]@{pid=101;root_hwnd=888;background_foreground_grant=[pscustomobject]@{granted=$true;app_pid=101;root_hwnd=888;runner_pid=202}}
    $window=[pscustomobject]@{Handle=[IntPtr]999}
    $window|Add-Member -MemberType ScriptMethod -Name Show -Value {}
    $script:backgroundSample=[pscustomobject]@{unix_ms=1700000000000;foreground_pid=202;foreground_hwnd=999;total=2;visible=1}
    function Get-ReleaseCheckWindowSample {param($AppPid) $script:backgroundSample}
    $proof=Set-ReleaseCheckBackgroundForeground $ready 101 $window 202
    Assert-True ($proof.foreground_pid -eq 202 -and $proof.foreground_hwnd -eq 999) 'Runner foreground preparation lost the native PID/HWND proof.'
    [MivReleaseCheckNative]::ActivationCalls=0
    $ready.background_foreground_grant.runner_pid=203
    Assert-Throws {Set-ReleaseCheckBackgroundForeground $ready 101 $window 202} 'Mismatched grant receiver was accepted.'
    $ready.background_foreground_grant.runner_pid=202
    [MivReleaseCheckNative]::BackgroundOwner=303
    Assert-Throws {Set-ReleaseCheckBackgroundForeground $ready 101 $window 202} 'Another process background HWND was activated.'
    Assert-True ([MivReleaseCheckNative]::ActivationCalls -eq 0) 'Identity refusal still attempted foreground activation.'
    [MivReleaseCheckNative]::BackgroundOwner=202;[MivReleaseCheckNative]::ActivationSucceeds=$false
    Assert-Throws {Set-ReleaseCheckBackgroundForeground $ready 101 $window 202} 'OS foreground refusal was accepted.'
    [MivReleaseCheckNative]::ActivationSucceeds=$true
    $script:backgroundSample.foreground_hwnd=1000
    Assert-Throws {Set-ReleaseCheckBackgroundForeground $ready 101 $window 202} 'Activation success replaced actual HWND verification.'
    $script:backgroundSample.foreground_hwnd=999;$script:backgroundSample.foreground_pid=101
    Assert-Throws {Set-ReleaseCheckBackgroundForeground $ready 101 $window 202} 'App foreground was accepted for background setup.'
}
$sample.visible=0
Assert-ReleaseCheckWindowSamples @($sample) 'tray-residency' 101 888 0
$sample.total=0
Assert-Throws {Assert-ReleaseCheckWindowSamples @($sample) 'tray-residency' 101 888 0} 'No top-level windows accepted as tray.'
Import-ReleaseCheckFunctions (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1') @('Assert-NoSmokeProcess','Stop-OwnedReleaseProcess','Test-OwnedReleaseProcessAlive')
$smoke=Split-Path $exe -Parent
$installed=[pscustomobject]@{ProcessName='mimageviewer';Id=303;Path='C:\Program Files\mImageViewer\mimageviewer.exe';HasExited=$false}
$inventory=@($installed)
function Get-Process {param($ErrorAction) return $script:inventory}
Assert-NoSmokeProcess
$inventory=@([pscustomobject]@{ProcessName='mimageviewer';Id=404;Path=$exe})
Assert-Throws {Assert-NoSmokeProcess} 'Another portable smoke was not rejected.'
$inventory=@($installed)
$fake.HasExited=$false
$fake|Add-Member -MemberType ScriptMethod -Name Refresh -Value {} -Force
$fake|Add-Member -MemberType ScriptMethod -Name WaitForExit -Value {$this.HasExited=$true} -Force
$cim.CommandLine=$line
$owned=$fake;$ownedStart=$start.AddSeconds(1);$stopped=@()
function Get-CimInstance {param($ClassName,$Filter,$ErrorAction) return $script:cim}
function Stop-Process {param($Id,[switch]$Force,$ErrorAction) $script:stopped+=@($Id)}
Assert-Throws {Test-OwnedReleaseProcessAlive} 'Live mismatched identity was accepted.'
$ownedStart=$start
Assert-True (Test-OwnedReleaseProcessAlive) 'Correct live owned process was rejected.'
function Get-CimInstance {param($ClassName,$Filter,$ErrorAction) $script:owned.HasExited=$true;throw 'fake normal-exit lookup race'}
Assert-True (-not (Test-OwnedReleaseProcessAlive)) 'Normal exit during WMI was misclassified as an identity failure.'
$fake.HasExited=$false;$ownedStart=$start.AddSeconds(1)
function Get-CimInstance {param($ClassName,$Filter,$ErrorAction) return $script:cim}
Assert-Throws {Stop-OwnedReleaseProcess} 'Cleanup stopped a reused/mismatched process.'
Assert-True ($stopped.Count -eq 0) 'Mismatch still issued Stop-Process.'
$ownedStart=$start
Stop-OwnedReleaseProcess
Assert-True ($stopped.Count -eq 1 -and $stopped[0] -eq 101) 'Cleanup did not target exactly the owned PID.'
Assert-True (-not $installed.HasExited) 'Installed/tray instance was changed.'
# Exercise the actual completion path with fake handles/archive functions.
# No launch, real process query, filesystem read or write occurs.
Import-ReleaseCheckFunctions (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1') @('Complete-ReleaseSession')
function Save-ReleaseLogs {param($Directory) if($script:fakeArchiveFails){throw 'fake archive environment failure'}}
$fakeArchiveFails=$false
foreach($code in @(1,2,-1073741819)){
    $cases=New-Object 'Collections.Generic.List[object]'
    $owned=[pscustomobject]@{Id=101;ExitCode=$code}
    $owned|Add-Member -MemberType ScriptMethod -Name WaitForExit -Value {}
    $session=Complete-ReleaseSession 'tray-close-after-loading' 'C:\fake\run\case'
    $expected=if($code -eq 2){'SKIPPED'}else{'FAIL'}
    $expectedCode=if($code -eq 2){2}else{1}
    Assert-True ($cases.Count -eq 1 -and $session.exit_result -eq $cases[0]) 'Nonzero app exit was discarded.'
    Assert-True ($cases[0].verdict -eq $expected -and $cases[0].exit_code -eq $expectedCode) 'Script/environment/crash exit was misclassified.'
    Assert-True ($cases[0].failures -is [array] -and $cases[0].environment_errors -is [array]) 'Failure diagnostics do not preserve JSON array shape.'
    Assert-True ($cases[0].evidence.process.process_exit_code -eq $code -and $session.process_exit_code -eq $code) 'Original process exit code was overwritten.'
    Assert-True ($cases[0].evidence.failure_artifacts.perf_log -eq 'C:\fake\run\case\perf_events.jsonl') 'Failure evidence location was lost.'
}
Assert-True ($null -eq (New-ReleaseCheckProcessExitResult 'case' 0 101 'C:\fake')) 'Zero process exit was classified as successful suite evidence.'
$cases=New-Object 'Collections.Generic.List[object]';$fakeArchiveFails=$true
$owned=[pscustomobject]@{Id=101;ExitCode=1}
$owned|Add-Member -MemberType ScriptMethod -Name WaitForExit -Value {}
Assert-Throws {Complete-ReleaseSession 'perf-cached' 'C:\fake\run\case'} 'Fake archive error did not exercise the environment catch.'
Assert-True ($cases.Count -eq 1 -and $cases[0].verdict -eq 'FAIL' -and $cases[0].failures -like '*exit code 1*') 'Environment archive error erased an already observed application FAIL.'
# Exercise the prepare/analyzer helper with the uncached ExitCode=null behavior.
& {
    Import-ReleaseCheckFunctions (Join-Path $PSScriptRoot 'ui-smoke.ps1') @('Invoke-UiSmokeCapturedProcess','Join-NativeArguments','Quote-NativeArgument')
    function Start-Process {
        param($FilePath,$ArgumentList,$WorkingDirectory,$RedirectStandardOutput,$RedirectStandardError,$WindowStyle,[switch]$PassThru,[switch]$Wait)
        Assert-True (-not $Wait) 'Captured helper waits inside Start-Process before caching the handle.'
        $script:fakeSequence.Add('start')
        return $script:fakeProcess
    }
    foreach($code in @(0,17)) {
        $script:fakeSequence=New-Object 'Collections.Generic.List[string]'
        $script:fakeProcess=[pscustomobject]@{Id=101;Cached=$false;ExitCode=$null;ExpectedCode=$code}
        $script:fakeProcess|Add-Member -MemberType ScriptProperty -Name Handle -Value {
            $script:fakeSequence.Add('handle');$this.Cached=$true;[IntPtr]101
        }
        $script:fakeProcess|Add-Member -MemberType ScriptMethod -Name WaitForExit -Value {
            $script:fakeSequence.Add('wait');if($this.Cached){$this.ExitCode=$this.ExpectedCode}
        }
        $result=Invoke-UiSmokeCapturedProcess 'fake.exe' @('fake argument') 'fake-dir' 'fake-out' 'fake-err'
        Assert-True ($result.ExitCode -is [int] -and $result.ExitCode -eq $code) 'Captured helper lost the actual integer exit code.'
        Assert-True (($script:fakeSequence -join ',') -eq 'start,handle,wait') 'Handle was not cached before the exit wait.'
    }
}
$runnerText=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'ui-smoke.ps1') -Encoding UTF8 -Raw
Assert-True ($runnerText.Contains('$null = $script:process.Handle # retain for both normal and button-helper launches')) 'Primary/helper app launch does not retain its handle.'
Assert-True ($runnerText.Contains('$null = $script:process.Handle # retain before the restart can exit')) 'Restart app launch does not retain its handle.'
foreach($relative in @('ui-smoke.ps1','check-idle-health.ps1','ui-smoke\release-checks\Run.ps1','ui-smoke\release-checks\Sampler.ps1')){
    $path=Join-Path $PSScriptRoot $relative
    $ast=Get-TestAst $path
    $text=Get-Content -LiteralPath $path -Encoding UTF8 -Raw
    $approvalOffset=$text.IndexOf('if (-not $InteractiveApproved')
    if($relative -eq 'check-idle-health.ps1'){$approvalOffset=$text.IndexOf('if (-not $InteractiveApproved -or $ProcessId')}
    Assert-True ($approvalOffset -ge 0) "Approval guard missing: $relative"
    $topCalls=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -in @('Start-Process','New-Item','Add-Type')},$true))
    foreach($call in $topCalls){Assert-True ($call.Extent.StartOffset -gt $approvalOffset) "Side effect before guard: $relative"}
}
$samplerText=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Sampler.ps1') -Encoding UTF8 -Raw
Assert-True ($samplerText.Contains('clock-window $perf --sampler')) 'Session clock conversion missing.'
Assert-True ($samplerText.Contains('clock=$converted.evidence.clock') -and $samplerText.Contains('measure_start_unix_ms=$startUtc') -and $samplerText.Contains('measure_end_unix_ms=$endUtc')) 'Sampler lost the converted session clock or external UTC boundaries.'
Assert-True (-not $samplerText.Contains('$env:APPDATA') -and -not $samplerText.Contains('StartTime).TotalSeconds')) 'Sampler falls back to normal data or OS elapsed clock.'
Assert-True ($samplerText.Contains("perf_evaluation='deferred_until_normal_exit'") -and -not $samplerText.Contains("'idle-health'")) 'Sampler evaluates perf before the shutdown delivery checkpoint.'
# Execute the sampler's real measurement/catch/verdict statements with fakes.
# No preflight, launch, process query, filesystem access or desktop input runs.
& {
    $ast=Get-TestAst (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Sampler.ps1')
    $body=@($ast.EndBlock.Statements | Where-Object {$_ -is [Management.Automation.Language.TryStatementAst]})
    $verdicts=@($ast.EndBlock.Statements | Where-Object {$_.Extent.Text -match '^\$report\.(verdict|exit_code)='})
    Assert-True ($body.Count -eq 1 -and $verdicts.Count -eq 2) 'Sampler fake cannot identify its real measurement/verdict statements.'
    Import-ReleaseCheckFunctions (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Sampler.ps1') @('Get-ExactMeasuredProcess')
    $thresholds=[pscustomobject]@{warmup_seconds=0;measure_seconds=0.3;max_cpu_core_ratio=0.02;target_cpu_core_ratio=0.01;max_app_log_growth_bytes=100;max_perf_log_growth_bytes=100}
    $report=[ordered]@{failures=@();environment_errors=@();warnings=@();verdict='SKIPPED';exit_code=2}
    $ProcessId=101;$Scenario='tray-residency';$TargetKey='';$RootHwnd=1001;$BackgroundHwnd=0;$BackgroundProcessId=0
    $perf='fake-perf';$app='fake-app';$reportPath='fake-report';$OutputDirectory='fake-output';$SleepEvidencePath='fake-completion';$scripts='fake-scripts';$ExpectedStartTimeUtc=[datetime]'2026-10-09T00:00:00Z'
    $script:fakeCpuCounter=0;$script:fakeProcessQueries=0
    $script:fakeWmiWindows=New-Object 'Collections.Generic.List[object]'
    $script:fakeMeasured=[pscustomobject]@{Id=101;HasExited=$false;StartTime=$ExpectedStartTimeUtc;Path=$exe;Handle=[IntPtr]101}
    $script:fakeMeasured|Add-Member -MemberType ScriptMethod -Name Refresh -Value {$script:fakeCpuCounter++}
    $script:fakeMeasured|Add-Member -MemberType ScriptProperty -Name TotalProcessorTime -Value {[timespan]::FromSeconds($script:fakeCpuCounter / 10.0)}
    function Get-Process {param($Id,$ErrorAction) $script:fakeProcessQueries++;return $script:fakeMeasured}
    function Get-CimInstance {
        param($ClassName,$Filter,$ErrorAction)
        $begin=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        # Reproduce the slow identity lookup from Phase 4, using only a fake.
        Microsoft.PowerShell.Utility\Start-Sleep -Milliseconds 1200
        $script:fakeWmiWindows.Add(@{start=$begin;end=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()})
        return $script:cim
    }
    function Test-InitialIndexScanSettled {param($Path) $true}
    function Start-Sleep {param($Seconds,$Milliseconds) if($Milliseconds){Microsoft.PowerShell.Utility\Start-Sleep -Milliseconds $Milliseconds}}
    function Get-FileLength {param($Path) 0}
    function Get-ReleaseCheckWindowSample {param($ProcessId) [pscustomobject]@{unix_ms=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds();total=1;visible=0}}
    function Assert-ReleaseCheckWindowSamples {param($Samples,$Scenario,$ProcessId,$RootHwnd,$BackgroundHwnd,$BackgroundProcessId)}
    function Test-Path {param($LiteralPath,$PathType) $true}
    function Get-Content {param($LiteralPath,$Encoding,[switch]$Raw) '{"pid":101,"sleep_start_unix_ms":1,"sleep_end_unix_ms":2}'}
    function Write-ReleaseCheckJson {param($Path,$Value)}
    function python {throw 'fake missing session clock anchor'}
    & ([scriptblock]::Create($body[0].Extent.Text))
    foreach($statement in $verdicts){& ([scriptblock]::Create($statement.Extent.Text))}
    Assert-True ($report.verdict -eq 'FAIL' -and $report.exit_code -eq 1) 'Clock environment failure erased the completed external CPU FAIL.'
    Assert-True ($report.failures -contains 'CPU one-core limit exceeded.' -and $report.environment_errors -contains 'fake missing session clock anchor') ('External CPU failure or clock evidence was lost: '+($report|ConvertTo-Json -Depth 4 -Compress))
    Assert-True ($script:fakeProcessQueries -eq 1 -and $script:fakeWmiWindows.Count -eq 3) 'Sampler reacquired a PID or polled WMI during measurement.'
    foreach($lookup in $script:fakeWmiWindows){
        Assert-True ($lookup.end -le $report.measure_start_unix_ms -or $lookup.start -ge $report.measure_end_unix_ms) 'Slow identity query overlapped the CPU/log/foreground window.'
    }
    Assert-True ($report.wall_seconds -ge 0.3 -and $report.wall_seconds -lt 0.55) 'Identity latency or a full extra sleep distorted the measured duration.'
    Assert-True ($report.window_samples.Count -ge 3 -and $report.window_samples[0].unix_ms -le $report.measure_start_unix_ms -and $report.window_samples[-1].unix_ms -ge $report.measure_end_unix_ms - 100) 'Native samples did not bracket the measurement window.'
    for($i=1;$i -lt $report.window_samples.Count;$i++){
        Assert-True ($report.window_samples[$i].unix_ms-$report.window_samples[$i-1].unix_ms -le 1100) 'Slow WMI caused a lifetime sampling gap.'
    }
    # A held handle is only useful if process exit still invalidates the run.
    $report=[ordered]@{failures=@();environment_errors=@();warnings=@();verdict='SKIPPED';exit_code=2}
    $script:fakeMeasured.HasExited=$false;$script:fakeSampleCount=0
    function Get-ReleaseCheckWindowSample {
        param($ProcessId)
        $script:fakeSampleCount++
        if($script:fakeSampleCount -eq 2){$script:fakeMeasured.HasExited=$true}
        [pscustomobject]@{unix_ms=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds();total=1;visible=0}
    }
    & ([scriptblock]::Create($body[0].Extent.Text))
    foreach($statement in $verdicts){& ([scriptblock]::Create($statement.Extent.Text))}
    Assert-True ($report.verdict -eq 'SKIPPED' -and ($report.environment_errors -join ' ') -like '*Owned measured process exited*') 'Process exit during sampling was accepted.'
    # Reproduce the real background failure after a completed measurement.
    # It must still wait for/import worker completion and convert its clock.
    $report=[ordered]@{failures=@();environment_errors=@();warnings=@();verdict='SKIPPED';exit_code=2}
    $script:fakeMeasured.HasExited=$false;$Scenario='static-background'
    $thresholds.max_cpu_core_ratio=100;$thresholds.target_cpu_core_ratio=100
    function Get-ReleaseCheckWindowSample {param($ProcessId) [pscustomobject]@{unix_ms=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds();foreground_pid=101;foreground_hwnd=1001;total=1;visible=1}}
    function Assert-ReleaseCheckWindowSamples {param($Samples,$Scenario,$ProcessId,$RootHwnd,$BackgroundHwnd,$BackgroundProcessId) throw 'fake lost runner background'}
    function Get-Content {
        param($LiteralPath,$Encoding,[switch]$Raw)
        if($LiteralPath -like '*clock.json'){return '{"evidence":{"clock":{"included":true}}}'}
        return '{"pid":101,"sleep_start_unix_ms":1,"sleep_end_unix_ms":2}'
    }
    $script:fakeClockConverted=$false
    function python {$script:fakeClockConverted=$true;$global:LASTEXITCODE=0}
    & ([scriptblock]::Create($body[0].Extent.Text))
    foreach($statement in $verdicts){& ([scriptblock]::Create($statement.Extent.Text))}
    Assert-True ($report.verdict -eq 'SKIPPED' -and $report.environment_errors -contains 'fake lost runner background') 'Lost background condition was erased by successful clock proof.'
    Assert-True ($report.sleep_start_unix_ms -eq 1 -and $report.sleep_end_unix_ms -eq 2 -and $script:fakeClockConverted -and $report.evidence.clock.included) 'Window failure discarded independent UTC/sleep/clock evidence.'
}
Write-Host 'release-check safety: PASS (approval, path, exact PID/start time, lifetime windows and clock routing; no applications/process commands)'
