# Noninteractive safety regressions: fake processes and argument parsing only.
# No mIV, desktop probes, foreground operations or builds run. Child shells only
# exercise argument guards, fake-package copying and read-only result validation.
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
Assert-True (Test-ReleaseCheckContinueAfterAnalysis 'perf-cached' 1) 'Cached performance FAIL prevented independent uncached evidence.'
Assert-True (Test-ReleaseCheckContinueAfterAnalysis 'perf-uncached' 0) 'Successful case prevented continuation.'
foreach($case in @('static-foreground','tray-close-after-loading','perf-uncached')){
    Assert-True (-not (Test-ReleaseCheckContinueAfterAnalysis $case 1)) 'Unrelated failure bypassed the stop condition.'
}
Assert-True (-not (Test-ReleaseCheckContinueAfterAnalysis 'perf-cached' 2)) 'Environment-only cached SKIPPED bypassed the stop condition.'
& {
    # Exercise the actual per-session substitutions without invoking the runner.
    # In PS5 a pipeline ConvertTo-Json unwraps a one-element path list.
    $ast=Get-TestAst (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1')
    $statements=$ast.FindAll({param($n)
        $n -is [Management.Automation.Language.AssignmentStatementAst] -and (
            $n.Left.Extent.Text -eq '$seedRecipe' -or $n.Left.Extent.Text -eq '$cachePaths' -or (
                $n.Left.Extent.Text -eq '$template' -and (
                    $n.Right.Extent.Text.Contains("'__CACHE_PATHS__'") -or
                    $n.Right.Extent.Text.Contains("'__LOAD_PATHS__'"))))
    },$true)
    Assert-True ($statements.Count -eq 4) 'Seed/load template substitution statements missing.'
    $m=[pscustomobject]@{
        seed_requirements=[pscustomobject]@{
            sessions=@([pscustomobject]@{case_id='seed-profile';cache_paths=@('C:\fixture\static')},
                       [pscustomobject]@{case_id='seed-perf-cached';cache_paths=@('C:\fixture\cached','C:\fixture\cached\folder-0001')})
            cache=[pscustomobject]@{prepare=@('C:\fixture\static','C:\fixture\cached')}
        }
        perf=[pscustomobject]@{cached=[pscustomobject]@{load_order=@('C:\fixture\cached\folder-0001')}}
    }
    $mode='cached'
    foreach($CaseId in @('seed-profile','seed-perf-cached')){
        $template='__CACHE_PATHS__|__LOAD_PATHS__'
        foreach($statement in $statements){. ([scriptblock]::Create($statement.Extent.Text))}
        $parts=$template.Split('|')
        Assert-True ($parts[0].StartsWith('[') -and $parts[1].StartsWith('[')) 'One-path seed/load list became a Rhai scalar.'
        $paths=ConvertFrom-Json -InputObject $parts[0]
        if($CaseId -eq 'seed-profile'){
            Assert-True ($paths.Count -eq 1 -and $paths[0] -eq 'C:\fixture\static') 'Static seed received perf cache paths.'
        }else{
            Assert-True ($paths.Count -eq 2 -and $paths[0] -eq 'C:\fixture\cached') ('Perf seed received wrong cache paths: '+$parts[0])
        }
    }
}
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
 public static long ForegroundHwnd=888;
 public static uint ForegroundOwner=101;
 public static System.IntPtr GetForegroundWindow(){return new System.IntPtr(ForegroundHwnd);}
 public static uint GetWindowThreadProcessId(System.IntPtr h,out uint p){p=h.ToInt64()==999?BackgroundOwner:(h.ToInt64()==ForegroundHwnd?ForegroundOwner:0);return 1;}
 public static bool SetForegroundWindow(System.IntPtr h){ActivationCalls++;if(ActivationSucceeds){ForegroundHwnd=h.ToInt64();ForegroundOwner=BackgroundOwner;}return ActivationSucceeds;}
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
    $script:backgroundShowCalls=0
    $window|Add-Member -MemberType ScriptMethod -Name Show -Value {$script:backgroundShowCalls++}
    $script:backgroundSample=[pscustomobject]@{unix_ms=1700000000000;foreground_pid=202;foreground_hwnd=999;total=2;visible=1}
    function Get-ReleaseCheckWindowSample {param($AppPid) $script:backgroundSample}
    $proof=Set-ReleaseCheckBackgroundForeground $ready 101 $window 202
    Assert-True ($proof.foreground_pid -eq 202 -and $proof.foreground_hwnd -eq 999) 'Runner foreground preparation lost the native PID/HWND proof.'
    Assert-True ($script:backgroundShowCalls -eq 1 -and [MivReleaseCheckNative]::ActivationCalls -eq 1) 'Exact-root transition did not show/activate once.'
    # Already foreground in this exact runner window is also an allowed source.
    $proof=Set-ReleaseCheckBackgroundForeground $ready 101 $window 202
    Assert-True ($script:backgroundShowCalls -eq 2 -and [MivReleaseCheckNative]::ActivationCalls -eq 2) 'Exact runner foreground was refused.'
    foreach($source in @(@{pid=303;hwnd=1000},@{pid=101;hwnd=889},@{pid=202;hwnd=1000},@{pid=303;hwnd=888},@{pid=0;hwnd=0})){
        [MivReleaseCheckNative]::ForegroundOwner=$source.pid;[MivReleaseCheckNative]::ForegroundHwnd=$source.hwnd
        [MivReleaseCheckNative]::ActivationCalls=0;$script:backgroundShowCalls=0
        $refusal=Invoke-ReleaseCheckPreflight {Set-ReleaseCheckBackgroundForeground $ready 101 $window 202}
        Assert-True ($refusal.exit_code -eq 2 -and $refusal.environment_error -like 'Foreground changed before background Show:*' -and $null -eq $refusal.value) 'Lost foreground was not SKIPPED/environment-not-met.'
        Assert-True ($script:backgroundShowCalls -eq 0 -and [MivReleaseCheckNative]::ActivationCalls -eq 0) 'Foreign foreground still caused Show/activation.'
    }
    # Verify the actual suite catch records this refusal as SKIPPED/2.
    $ast=Get-TestAst (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1')
    $suiteTry=@($ast.EndBlock.Statements | Where-Object {$_ -is [Management.Automation.Language.TryStatementAst]})[-1]
    $catchBody=($suiteTry.CatchClauses[0].Body.Statements | ForEach-Object {$_.Extent.Text}) -join "`n"
    $cases=New-Object 'Collections.Generic.List[object]'
    [MivReleaseCheckNative]::ForegroundHwnd=1000;[MivReleaseCheckNative]::ForegroundOwner=303
    try{Set-ReleaseCheckBackgroundForeground $ready 101 $window 202}
    catch{& ([scriptblock]::Create($catchBody))}
    Assert-True ($cases.Count -eq 1 -and $cases[0].verdict -eq 'SKIPPED' -and $cases[0].exit_code -eq 2 -and $cases[0].environment_errors[0] -like 'Foreground changed before background Show:*') 'Suite catch did not preserve foreground loss as SKIPPED/2.'
    Assert-True ($script:backgroundShowCalls -eq 0 -and [MivReleaseCheckNative]::ActivationCalls -eq 0) 'Suite refusal caused foreground side effects.'
    # A foreground loss caused during Show must stop the subsequent activation.
    [MivReleaseCheckNative]::ForegroundHwnd=888;[MivReleaseCheckNative]::ForegroundOwner=101
    $window|Add-Member -MemberType ScriptMethod -Name Show -Force -Value {
        $script:backgroundShowCalls++
        [MivReleaseCheckNative]::ForegroundHwnd=1000;[MivReleaseCheckNative]::ForegroundOwner=303
    }
    $refusal=Invoke-ReleaseCheckPreflight {Set-ReleaseCheckBackgroundForeground $ready 101 $window 202}
    Assert-True ($refusal.exit_code -eq 2 -and $refusal.environment_error -like 'Foreground changed before background activation:*') 'Foreground loss after Show was not an environment refusal.'
    Assert-True ($script:backgroundShowCalls -eq 1 -and [MivReleaseCheckNative]::ActivationCalls -eq 0) 'Activation stole foreground after Show lost the exact source.'
    $window|Add-Member -MemberType ScriptMethod -Name Show -Force -Value {$script:backgroundShowCalls++}
    [MivReleaseCheckNative]::ForegroundHwnd=888;[MivReleaseCheckNative]::ForegroundOwner=101
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
    $activeDir='C:\fake\run\case'
    $session=Complete-ReleaseSession 'tray-close-after-loading' 'C:\fake\run\case'
    Assert-True ($null -eq $activeDir) 'Archived session still owns logs after a profile restore.'
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
& {
    # Run the real prelaunch and final archive paths with fakes. Restored seed
    # logs belong to no new case until that case has actually been launched.
    Import-ReleaseCheckFunctions (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1') @('Invoke-ReleaseSession')
    $ast=Get-TestAst (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1')
    $finally=@($ast.EndBlock.Statements | Where-Object {$_ -is [Management.Automation.Language.TryStatementAst]})[-1].Finally
    $run='C:\fake\run';$scripts='C:\fake\scripts';$fixtureManifest='fake-manifest';$repo='C:\fake';$data='C:\fake\data'
    $script:activeDir=$null;$script:background=$null
    $lock=[pscustomobject]@{};$lock|Add-Member -MemberType ScriptMethod -Name Dispose -Value {}
    $cases=New-Object 'Collections.Generic.List[object]'
    $script:fakeArchived=New-Object 'Collections.Generic.List[string]'
    function Assert-NoSmokeProcess {}
    $FromKit=$false
    function Get-MivSourceFingerprint {param($Repo) 'fake-source-fingerprint'}
    function Assert-ReleaseCheckArtifact {param($Repo,$ExpectedSourceFingerprint)}
    function New-Item {param($ItemType,$Path)}
    function Get-Command {param($Name,$ErrorAction) [pscustomobject]@{Source='fake-python'}}
    function Invoke-UiSmokeCapturedProcess {
        param($FilePath,$ArgumentValues,$WorkingDirectory,$StandardOutputPath,$StandardErrorPath)
        Assert-True ($ArgumentValues -contains '--stopped') 'Preflight lost the stopped assertion.'
        Assert-True ($StandardErrorPath -eq 'C:\fake\run\static-foreground\preflight.stderr.log') 'Preflight diagnostic was not captured in its case.'
        [pscustomobject]@{ExitCode=2}
    }
    function Get-Content {param($LiteralPath,$Encoding,[switch]$Raw) 'fake invalid stopped WAL'}
    function Clear-ReleaseLogs {throw 'Preflight failure discarded profile logs before launch.'}
    function Start-Process {throw 'Preflight failure launched an application.'}
    function Stop-OwnedReleaseProcess {}
    function Save-ReleaseLogs {param($Directory) $script:fakeArchived.Add($Directory)}
    try {Invoke-ReleaseSession 'static-foreground' 'fake-overlay' $true;throw 'Preflight failure was ignored.'}
    catch {Assert-True ($_.Exception.Message -eq 'Stopped-profile cache proof unavailable: fake invalid stopped WAL') 'Preflight failure lost its actionable diagnostic.'}
    & ([scriptblock]::Create($finally.Extent.Text))
    Assert-True ($null -eq $script:activeDir -and $script:fakeArchived.Count -eq 0) 'Prelaunch failure archived restored seed logs as a new case.'
}
& {
    Import-ReleaseCheckFunctions (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1') @('Clear-ReleaseLogs')
    $data='C:\fake\data'
    $script:fakeLogOperations=New-Object 'Collections.Generic.List[string]'
    function Assert-NoReparseTree {param($Path,$Label) $script:fakeLogOperations.Add('check:'+ $Path)}
    function Test-Path {param($LiteralPath,$PathType) $true}
    function Remove-Item {param($LiteralPath,[switch]$Force) $script:fakeLogOperations.Add('remove:'+ $LiteralPath)}
    Clear-ReleaseLogs
    Assert-True (($script:fakeLogOperations -join '|') -eq 'check:C:\fake\data\logs|remove:C:\fake\data\logs\perf_events.jsonl|remove:C:\fake\data\logs\mimageviewer.log') 'Log cleanup escaped the checked disposable runtime log files.'
    $ast=Get-TestAst (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1')
    $invoke=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Invoke-ReleaseSession'},$true))[0]
    $body=$invoke.Body.Extent.Text
    Assert-True ($body.IndexOf('Clear-ReleaseLogs') -lt $body.IndexOf('$script:owned=Start-Process') -and $body.IndexOf('$script:activeDir=$dir') -gt $body.IndexOf('$script:owned=Start-Process')) 'Session log ownership does not bracket the actual launch.'
}
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
& {
    . (Join-Path $PSScriptRoot 'relcheck-kit\KitSupport.ps1')
    $testRoot=Join-Path ([IO.Path]::GetTempPath()) ('miv-kit-safety-'+[Guid]::NewGuid().ToString('N'))
    $utf8=New-Object Text.UTF8Encoding($false)
    function Write-FakeFile {
        param([string]$Path,[string]$Text)
        New-Item -ItemType Directory -Path (Split-Path -Parent $Path) -Force | Out-Null
        [IO.File]::WriteAllText($Path,$Text,$utf8)
    }
    function Seal-FakeKit {
        param([string]$Root,$Manifest)
        $Manifest.files=@(Get-ReleaseCheckKitFileRecords $Root)
        $json=Join-Path $Root 'release-check-kit.json'
        Write-FakeFile $json ($Manifest|ConvertTo-Json -Depth 20)
        Write-FakeFile (Join-Path $Root 'release-check-kit.sha256') ((Get-FileHash -LiteralPath $json -Algorithm SHA256).Hash.ToLowerInvariant()+"`n")
    }
    function New-FakeKit {
        param([string]$Name)
        $root=Join-Path $testRoot $Name
        Write-FakeFile (Join-Path $root 'scripts\fake.ps1') '# fake ASCII script'
        Write-FakeFile (Join-Path $root 'tools\ffmpeg\ffmpeg.exe') 'fake ffmpeg; never execute'
        Write-FakeFile (Join-Path $root 'source-fingerprint-records.txt') "src/fake.rs`t012345"
        $fingerprint=(Get-FileHash -LiteralPath (Join-Path $root 'source-fingerprint-records.txt') -Algorithm SHA256).Hash.ToLowerInvariant()
        $build=[ordered]@{schema_version=1;artifact_flavor='portable-test-script';cargo_profile='release';features=@('portable','test-script');source_head=('a'*40);source_fingerprint_sha256=$fingerprint}
        foreach($entry in ([ordered]@{core='mimageviewer.exe';remote='mimageviewer-remote.exe';epub_worker='mimageviewer-epub-pdf.exe';mcp='mimageviewer-mcp.exe';musicvideo='mimageviewer-musicvideo.exe'}).GetEnumerator()){
            $file=Join-Path $root ('target\portable-smoke-package\'+$entry.Value)
            Write-FakeFile $file ('fake '+$entry.Key)
            $build[$entry.Key+'_sha256']=(Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        Write-FakeFile (Join-Path $root 'target\portable-smoke-package\.test-script-build.json') ($build|ConvertTo-Json -Depth 5)
        $manifest=[ordered]@{schema_version=1;kit_id='relcheck-fake';scripts_source_head=('b'*40);artifact=@{source_head=('a'*40);source_fingerprint_sha256=$fingerprint;manifest=$build};ffmpeg=@{};files=@()}
        Seal-FakeKit $root $manifest
        return $root
    }
    function Invoke-FakeResultReceiver {
        param([string]$KitPath,[string]$ResultPath)
        $result=Invoke-KitSafetyChild @('-File',(Join-Path $PSScriptRoot 'relcheck-kit\Receive-ReleaseCheckResult.ps1'),'-Kit',$KitPath,'-Result',$ResultPath)
        return $result.code
    }
    function Invoke-KitSafetyChild {
        param([string[]]$Arguments)
        # Expected child stderr must not become a terminating PS5 NativeCommandError.
        $ErrorActionPreference='Continue'
        $output=& powershell -NoProfile -ExecutionPolicy Bypass @Arguments 2>&1
        return [pscustomobject]@{code=$LASTEXITCODE;text=($output -join ' ')}
    }
    try{
        $root=New-FakeKit 'valid'
        $fingerprint=Assert-ReleaseCheckKit $root
        Assert-True ($fingerprint.Length -eq 64) 'Valid fake kit rejected.'
        foreach($relative in @('scripts\__pycache__\x.pyc','scripts\nested\extra.pyc','target\ui-smoke\runs\fake\result.json','target\portable-smoke\data\fake.db')){
            Write-FakeFile (Join-Path $root $relative) 'runtime output'
        }
        Assert-True ((Assert-ReleaseCheckKit $root) -eq $fingerprint) 'Runtime/Python outputs invalidated kit.'
        # Successful kit preparation also needs no Cargo.toml or build script.
        $prepare=Join-Path $root 'scripts\prepare-portable-smoke.ps1'
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'prepare-portable-smoke.ps1') -Destination $prepare
        $manifest=Get-Content -LiteralPath (Join-Path $root 'release-check-kit.json') -Encoding UTF8 -Raw | ConvertFrom-Json
        Seal-FakeKit $root $manifest
        foreach($prepareArgs in @(@('-FromKit','-TestScript'),@('-FromKit','-TestScript','-SkipBuild'))){
            $child=Invoke-KitSafetyChild (@('-File',$prepare)+$prepareArgs)
            Assert-True ($child.code -eq 0) ('Kit preparation attempted build/version lookup: '+$child.text)
            $marker=Get-Content -LiteralPath (Join-Path $root 'target\portable-smoke\data\.disposable-smoke-data') -Raw -Encoding ASCII
            Assert-True ($marker.Trim() -eq 'mimageviewer-disposable-smoke-v1;test-script=true') 'Kit preparation lost isolation marker.'
            $null=Assert-ReleaseCheckKit $root
        }
        # Exercise the real kit-only preflight branches without desktop/native work.
        foreach($relative in @('Run.ps1','Sampler.ps1')){
            $ast=Get-TestAst (Join-Path $PSScriptRoot ('ui-smoke\release-checks\'+$relative))
            $kitBranches=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.IfStatementAst] -and
                $n.Clauses[0].Item1.Extent.Text -eq '$FromKit' -and
                $n.Clauses[0].Item2.Extent.Text.Contains('Assert-ReleaseCheckKit $repo')},$true))
            Assert-True ($kitBranches.Count -ge 1) "Kit preflight missing: $relative"
            & {
                $FromKit=$true;$repo=$root;$scripts=$PSScriptRoot
                function Get-MivSourceFingerprint {throw 'Kit preflight invoked git/source fingerprinting.'}
                $oldBytecode=$env:PYTHONDONTWRITEBYTECODE
                try{
                    . ([scriptblock]::Create($kitBranches[0].Clauses[0].Item2.Extent.Text.TrimStart('{').TrimEnd('}')))
                    Assert-True ($expectedFingerprint -eq $fingerprint -and $env:PYTHONDONTWRITEBYTECODE -eq '1') 'Kit preflight lost verified fingerprint/bytecode policy.'
                }finally{$env:PYTHONDONTWRITEBYTECODE=$oldBytecode}
            }
        }
        Import-ReleaseCheckFunctions (Join-Path $PSScriptRoot 'relcheck-kit\New-ReleaseCheckKit.ps1') @('Copy-KitTree')
        $source=Join-Path $testRoot 'copy-source';$destination=Join-Path $testRoot 'copy-scripts'
        Write-FakeFile (Join-Path $source 'keep.ps1') '# keep'
        Write-FakeFile (Join-Path $source '__pycache__\ignore.pyc') 'ignore'
        Write-FakeFile (Join-Path $source 'nested\ignore.pyc') 'ignore'
        Write-FakeFile (Join-Path $source 'data\ignore.db') 'package user data'
        Copy-KitTree $source $destination -Scripts
        Assert-True ((Test-Path -LiteralPath (Join-Path $destination 'keep.ps1')) -and -not (Test-Path -LiteralPath (Join-Path $destination '__pycache__')) -and -not (Test-Path -LiteralPath (Join-Path $destination 'nested\ignore.pyc'))) 'Script copy included bytecode or lost scripts.'
        $destination=Join-Path $testRoot 'copy-package'
        Copy-KitTree $source $destination -Package
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $destination 'data')) -and (Test-Path -LiteralPath (Join-Path $destination 'keep.ps1'))) 'Package copy included user data or lost payload.'
        # The source-cleanliness path list is derived from the same AST as fingerprinting.
        $ast=Get-TestAst (Join-Path $PSScriptRoot 'build-portable.ps1')
        $function=$ast.Find({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Get-MivSourceFingerprintRecords'},$true)
        $assignments=@($function.FindAll({param($n)$n -is [Management.Automation.Language.AssignmentStatementAst] -and $n.Left.Extent.Text -eq '$sourcePaths'},$true))
        Assert-True ($assignments.Count -eq 1) 'Cannot derive fingerprint source path list.'
        $sourcePaths=@(& ([scriptblock]::Create($assignments[0].Right.Extent.Text)))
        Assert-True ($sourcePaths.Count -gt 0 -and $sourcePaths -contains 'Cargo.toml' -and $sourcePaths -contains 'vendor/twemoji') 'Fingerprint source path AST extraction failed.'
        foreach($mutation in @('byte','missing','extra','records','digest','package-fingerprint','git','exe')){
            $root=New-FakeKit $mutation
            $manifest=Get-Content -LiteralPath (Join-Path $root 'release-check-kit.json') -Encoding UTF8 -Raw | ConvertFrom-Json
            switch($mutation){
                'byte' {Write-FakeFile (Join-Path $root 'scripts\fake.ps1') '! fake ASCII script'}
                'missing' {Remove-Item -LiteralPath (Join-Path $root 'scripts\fake.ps1')}
                'extra' {Write-FakeFile (Join-Path $root 'extra.txt') 'extra'}
                'records' {
                    Write-FakeFile (Join-Path $root 'source-fingerprint-records.txt') 'altered records'
                    Seal-FakeKit $root $manifest # isolate records-to-fingerprint validation
                }
                'digest' {Write-FakeFile (Join-Path $root 'release-check-kit.sha256') ('0'*64)}
                'package-fingerprint' {
                    $path=Join-Path $root 'target\portable-smoke-package\.test-script-build.json'
                    $build=Get-Content -LiteralPath $path -Encoding UTF8 -Raw | ConvertFrom-Json
                    $build.source_fingerprint_sha256='0'*64
                    Write-FakeFile $path ($build|ConvertTo-Json -Depth 5)
                    Seal-FakeKit $root $manifest
                }
                'git' {New-Item -ItemType Directory -Path (Join-Path $root '.git') | Out-Null}
                'exe' {
                    Write-FakeFile (Join-Path $root 'target\portable-smoke-package\mimageviewer.exe') 'altered executable'
                    Seal-FakeKit $root $manifest # manifest hashes must still match binaries
                }
            }
            Assert-Throws {Assert-ReleaseCheckKit $root} "Kit mutation accepted: $mutation"
        }
        $root=New-FakeKit 'receiver'
        $result=Join-Path $testRoot 'returned-run'
        $manifest=Get-Content -LiteralPath (Join-Path $root 'release-check-kit.json') -Encoding UTF8 -Raw | ConvertFrom-Json
        $meta=[ordered]@{execution_mode='kit';kit=@{kit_id=$manifest.kit_id;kit_manifest_sha256=(Get-Content -LiteralPath (Join-Path $root 'release-check-kit.sha256') -Raw).Trim()};exe_sha256=$manifest.artifact.manifest.core_sha256;artifact=@{source_fingerprint_sha256=$manifest.artifact.source_fingerprint_sha256}}
        Write-FakeFile (Join-Path $result 'suite-input.json') (@{metadata=$meta}|ConvertTo-Json -Depth 10)
        foreach($code in @(0,1,2)){
            Write-FakeFile (Join-Path $result 'summary.json') (@{verdict=@('PASS','FAIL','SKIPPED')[$code];exit_code=$code}|ConvertTo-Json)
            Assert-True ((Invoke-FakeResultReceiver $root $result) -eq $code) "Receiver lost summary exit code $code"
        }
        Write-FakeFile (Join-Path $result 'summary.json') '{"verdict":"PASS","exit_code":0}'
        $meta.kit.kit_id='wrong-kit'
        Write-FakeFile (Join-Path $result 'suite-input.json') (@{metadata=$meta}|ConvertTo-Json -Depth 10)
        Assert-True ((Invoke-FakeResultReceiver $root $result) -eq 1) 'Receiver accepted wrong kit ID.'
        $meta.kit.kit_id=$manifest.kit_id;$meta.exe_sha256='0'*64
        Write-FakeFile (Join-Path $result 'suite-input.json') (@{metadata=$meta}|ConvertTo-Json -Depth 10)
        Assert-True ((Invoke-FakeResultReceiver $root $result) -eq 1) 'Receiver accepted wrong exe hash.'

        # Only invalid argument combinations are run; guards precede all UI/build work.
        $child=Invoke-KitSafetyChild @('-File',(Join-Path $PSScriptRoot 'ui-smoke.ps1'),'-Scenario','MultiWindowPdf','-FromKit','-InteractiveApproved')
        Assert-True ($child.code -eq 2 -and $child.text -like '*FromKit is valid only*') 'FromKit reached a non-ReleaseChecks scenario.'
        foreach($args in @(@('-FromKit'),@('-FromKit','-TestScript'))){
            # A copied prepare script has no kit JSON and no build script.
            $prepare=Join-Path $testRoot 'no-kit\scripts\prepare-portable-smoke.ps1'
            New-Item -ItemType Directory -Path (Split-Path -Parent $prepare) -Force | Out-Null
            Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'prepare-portable-smoke.ps1') -Destination $prepare -Force
            $child=Invoke-KitSafetyChild (@('-File',$prepare)+$args)
            Assert-True ($child.code -eq 1 -and $child.text -match '(requires -TestScript|kit manifest missing)') 'Invalid kit prepare passed its early guard.'
        }
        $ast=Get-TestAst (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1')
        $calls=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Assert-ReleaseCheckArtifact'},$true))
        Assert-True ($calls.Count -eq 3) 'Artifact call inventory changed; review all paths.'
        foreach($call in $calls){
            $parameters=@($call.CommandElements | Where-Object {$_ -is [Management.Automation.Language.CommandParameterAst] -and $_.ParameterName -eq 'ExpectedSourceFingerprint'})
            Assert-True ($parameters.Count -eq 1) 'Artifact call lacks explicit fingerprint.'
        }
        $sampler=Get-TestAst (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Sampler.ps1')
        $calls=@($sampler.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Assert-ReleaseCheckArtifact'},$true))
        Assert-True ($calls.Count -eq 1 -and $calls[0].Extent.Text.Contains('-ExpectedSourceFingerprint')) 'Sampler lost explicit fingerprint.'
        $runnerText=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1') -Encoding UTF8 -Raw
        $healthText=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'check-idle-health.ps1') -Encoding UTF8 -Raw
        Assert-True ($runnerText.Contains("if(`$FromKit){`$samplerArgs+='-FromKit'}") -and $healthText.Contains('-FromKit:$FromKit')) 'Kit mode did not reach the idle sampler.'
        Assert-Throws {Assert-ReleaseCheckArtifact 'unused' -ExpectedSourceFingerprint ''} 'Empty fingerprint accepted.'
        foreach($relative in @('relcheck-kit\KitSupport.ps1','relcheck-kit\New-ReleaseCheckKit.ps1','relcheck-kit\Receive-ReleaseCheckResult.ps1','relcheck-kit\Test-SubPcEnvironment.ps1','relcheck-kit\ForegroundProbeChild.ps1','prepare-portable-smoke.ps1','ui-smoke.ps1','ui-smoke\release-checks\Run.ps1','ui-smoke\release-checks\Support.ps1','ui-smoke\release-checks\Sampler.ps1','check-idle-health.ps1','test-release-check-safety.ps1')){
            $path=Join-Path $PSScriptRoot $relative
            $null=Get-TestAst $path
            Assert-True (@([IO.File]::ReadAllBytes($path) | Where-Object {$_ -gt 127}).Count -eq 0) "Non-ASCII PowerShell: $relative"
        }
    }finally{
        # Remove only our resolved GUID tree; never recurse through reparse points.
        $full=[IO.Path]::GetFullPath($testRoot)
        $prefix=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')+'\'
        Assert-True ($full.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase) -and (Split-Path -Leaf $full) -match '^miv-kit-safety-[0-9a-f]{32}$') 'Unsafe test cleanup path.'
        if(Test-Path -LiteralPath $full){$null=Get-ReleaseCheckKitFileRecords $full;Remove-Item -LiteralPath $full -Recurse -Force}
    }
}
Write-Host 'release-check safety: PASS (approval, kit hashes, argument guards, returned results, exact PID/HWND, lifetime and clock routing; no application or desktop interaction)'
