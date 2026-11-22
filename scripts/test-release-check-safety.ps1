# Noninteractive safety regressions: fake processes and argument parsing only.
# No mIV, desktop probes, foreground operations or product builds run. The GUI
# probe peer is compiled under TEMP but never launched. Child shells only
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
& {
    # Inspect wrappers as text only: they must never launch from this test.
    $expected=[ordered]@{
        'Run-SubPcProbe.cmd'='powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0Test-SubPcEnvironment.ps1"'
        'Run-SubPcSuite.cmd'='powershell -NoProfile -ExecutionPolicy Bypass -File "%CD%\scripts\ui-smoke.ps1" -Scenario ReleaseChecks -FromKit -InteractiveApproved'
    }
    foreach($entry in $expected.GetEnumerator()){
        $path=Join-Path $PSScriptRoot ('relcheck-kit\'+$entry.Key)
        Assert-True (Test-Path -LiteralPath $path -PathType Leaf) "Wrapper missing: $($entry.Key)"
        Assert-True (@([IO.File]::ReadAllBytes($path) | Where-Object {$_ -gt 127}).Count -eq 0) "Non-ASCII wrapper: $($entry.Key)"
        $text=[IO.File]::ReadAllText($path)
        $lines=@($text -split '\r?\n' | ForEach-Object {$_.Trim()} | Where-Object {$_})
        Assert-True ($lines[0] -ceq '@echo off' -and $lines[1] -ceq 'setlocal') "Wrapper initialization changed: $($entry.Key)"
        Assert-True ($text.Contains('cd /d "%~dp0..\.."')) "Wrapper does not enter its quoted kit root: $($entry.Key)"
        $calls=@($lines | Where-Object {$_ -match '^powershell(?:\.exe)?\s'})
        Assert-True ($calls.Count -eq 1 -and $calls[0] -ceq $entry.Value) "Wrapper calls unexpected script/options: $($entry.Key)"
        Assert-True (-not $text.Contains('%*') -and $text -notmatch '%[1-9]') "Wrapper accepts external arguments: $($entry.Key)"
        Assert-True ($text.Contains('set "relcheck_exit=%ERRORLEVEL%"') -and $text.Contains('echo EXIT=%relcheck_exit%') -and
            $text.Contains('exit /b %relcheck_exit%')) "Wrapper does not display/return the captured exit code: $($entry.Key)"
    }
}
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
 public static object ForegroundSample;
 public static object CaptureForeground(uint app,long root){return ForegroundSample;}
 public static int GeometryDpi=96;
 public static int[] WorkArea=new[]{0,0,1920,1040};
 public static int[] ClientSize=new[]{1420,900};
 public static int[] DesktopGeometry(){return new[]{GeometryDpi,WorkArea[0],WorkArea[1],WorkArea[2],WorkArea[3]};}
 public static int[] OuterRect(int w,int h,uint dpi,int logicalX,int logicalY){int x=(int)System.Math.Round(logicalX*dpi/96.0),y=(int)System.Math.Round(logicalY*dpi/96.0);return new[]{x,y,x+(int)(w*dpi/96)+16,y+(int)(h*dpi/96)+39};}
 public static int[] WindowGeometry(System.IntPtr h){return new[]{GeometryDpi,ClientSize[0],ClientSize[1],60,40,60+ClientSize[0]+16,40+ClientSize[1]+39,WorkArea[0],WorkArea[1],WorkArea[2],WorkArea[3]};}
 public static uint BackgroundOwner=202;
 public static bool ActivationSucceeds=true;
 public static int ActivationCalls=0;
 public static long PostedCloseHwnd=0;
 public static bool PostMessage(System.IntPtr h,uint m,System.IntPtr w,System.IntPtr l){if(m!=0x0010)throw new System.Exception("Unexpected probe message");PostedCloseHwnd=h.ToInt64();return true;}
 public static long ForegroundHwnd=888;
 public static uint ForegroundOwner=101;
 public static System.IntPtr GetForegroundWindow(){return new System.IntPtr(ForegroundHwnd);}
 public static uint GetWindowThreadProcessId(System.IntPtr h,out uint p){p=h.ToInt64()==999?BackgroundOwner:(h.ToInt64()==ForegroundHwnd?ForegroundOwner:0);return 1;}
 public static bool SetForegroundWindow(System.IntPtr h){ActivationCalls++;if(ActivationSucceeds){ForegroundHwnd=h.ToInt64();ForegroundOwner=BackgroundOwner;}return ActivationSucceeds;}
 public static string[] Arguments(string s) {var m=Regex.Matches(s,"\"([^\"]*)\"|([^ ]+)");var a=new string[m.Count];for(int i=0;i<a.Length;i++)a[i]=m[i].Groups[1].Success?m[i].Groups[1].Value:m[i].Groups[2].Value;return a;}
}
'@
}
& {
    # Pure/fake geometry gates: no native desktop reads or window creation.
    $runAst=Get-TestAst (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1')
    $launchArguments=@($runAst.FindAll({param($n)$n -is [Management.Automation.Language.AssignmentStatementAst] -and
        $n.Left.Extent.Text -eq '$arguments' -and $n.Right.Extent.Text.Contains("'--window-size'")},$true))
    $windowMetadata=@($runAst.FindAll({param($n)$n -is [Management.Automation.Language.AssignmentStatementAst] -and
        $n.Left.Extent.Text -eq '$metadata.window_size[$CaseId]'},$true))
    Assert-True ($launchArguments.Count -eq 1 -and $windowMetadata.Count -eq 1) 'Case launch/metadata geometry statements missing.'
    foreach($dpi in @(96,144)){
        $foreground=Get-ReleaseCheckCaseGeometry 'static-foreground' $dpi
        $background=Get-ReleaseCheckCaseGeometry 'static-background' $dpi
        $seed=Get-ReleaseCheckCaseGeometry 'seed-profile' $dpi
        $expected=if($dpi -eq 96){'1420,900/5'}else{'1328,900/7'}
        Assert-True ($foreground.window_size.Count -eq 2 -and $foreground.window_size[0] -is [int] -and $foreground.window_size[1] -is [int]) 'Selected window size is not a flat pair of integers.'
        Assert-True ((($foreground.window_size -join ',')+'/'+$foreground.grid_cols) -eq $expected) 'Wrong calibrated static row.'
        # Evaluate the actual runner expressions, without running its launch.
        $geometry=$foreground;$dir=Join-Path ([IO.Path]::GetTempPath()) 'fake geometry case'
        $data=Join-Path $dir 'data';$scriptPath=Join-Path $dir 'scenario.rhai'
        $CaseId='static-foreground';$metadata=@{window_size=@{}}
        . ([scriptblock]::Create($launchArguments[0].Extent.Text))
        . ([scriptblock]::Create($windowMetadata[0].Extent.Text))
        $sizeIndex=[array]::IndexOf($arguments,'--window-size')
        $expectedSize=if($dpi -eq 96){'1420x900'}else{'1328x900'}
        Assert-True ($sizeIndex -ge 0 -and $arguments[$sizeIndex+1] -is [string] -and $arguments[$sizeIndex+1] -eq $expectedSize) 'Runner did not emit one exact window-size argument.'
        $storedSize=$metadata.window_size[$CaseId]
        Assert-True ($storedSize.Count -eq 2 -and $storedSize[0] -is [int] -and $storedSize[1] -is [int] -and ($storedSize -join 'x') -eq $expectedSize) 'Runner metadata window size is not a flat logical pair.'
        Assert-True (($seed|ConvertTo-Json -Compress) -eq ($foreground|ConvertTo-Json -Compress)) 'Static seed uses another geometry.'
        $other=Get-ReleaseCheckCaseGeometry 'perf-cached' $dpi
        Assert-True (($other.window_size -join ',') -eq '1328,900' -and $other.grid_cols -eq 7) 'Nonstatic geometry changed.'
        $tray=Get-ReleaseCheckCaseGeometry 'tray-close-while-loading' $dpi
        Assert-True (($tray.window_size -join ',') -eq '1328,900' -and $tray.grid_cols -eq 10) 'Tray geometry changed.'
        [MivReleaseCheckNative]::GeometryDpi=$dpi
        [MivReleaseCheckNative]::WorkArea=@(0,0,3840,2120)
        $preflight=Get-ReleaseCheckGeometryPreflight
        Assert-True ($preflight.static_geometry.dpi -eq $dpi) 'Preflight lost selected DPI.'
        Assert-True (($preflight.startup_position -join ',') -eq '60,40') 'Fixed logical outer position differs from src/lib.rs.'
        $expectedOrigin=if($dpi -eq 96){'60,40'}else{'90,60'}
        Assert-True (($preflight.outer_rect[0..1] -join ',') -eq $expectedOrigin) 'Logical startup position was not converted to physical screen coordinates.'
        Assert-ReleaseCheckGeometryEnvironment $foreground $background @(0,0,3840,2120) $preflight.outer_rect
        Assert-Throws {Assert-ReleaseCheckGeometryEnvironment $foreground $background @(0,0,1000,2000) $preflight.outer_rect} 'Too-narrow work area accepted.'
        Assert-Throws {Assert-ReleaseCheckGeometryEnvironment $foreground $background @(0,0,4000,800) $preflight.outer_rect} 'Taskbar-reduced work area accepted.'
        $background.window_size=@(1200,900)
        Assert-Throws {Assert-ReleaseCheckGeometryEnvironment $foreground $background @(0,0,3840,2120) $preflight.outer_rect} 'Different foreground/background planned sizes accepted.'
        $physical=@($foreground.window_size | ForEach-Object {[int][Math]::Round($_*$dpi/96.0)})
        $expectedPhysical=if($dpi -eq 96){'1420x900'}else{'1992x1350'}
        Assert-True ($physical.Count -eq 2 -and $physical[0] -is [int] -and $physical[1] -is [int] -and ($physical -join 'x') -eq $expectedPhysical) 'Physical window size is not a flat converted pair.'
        $actual=[pscustomobject]@{dpi=$dpi;client_size=$physical;window_rect=$preflight.outer_rect;work_area=$preflight.work_area}
        Assert-ReleaseCheckWindowGeometry $actual $foreground
        # Comma binds tighter than subtraction in PowerShell; isolate arithmetic.
        $actual.client_size=@(($physical[0]-1),$physical[1])
        Assert-Throws {Assert-ReleaseCheckWindowGeometry $actual $foreground} 'Different actual background size accepted.'
        $actual.client_size=$physical;$actual.dpi=120
        Assert-Throws {Assert-ReleaseCheckWindowGeometry $actual $foreground} 'Actual DPI different from fixture accepted.'
    }
    [MivReleaseCheckNative]::GeometryDpi=120
    $unsupported=Invoke-ReleaseCheckPreflight {Get-ReleaseCheckGeometryPreflight}
    Assert-True ($unsupported.exit_code -eq 2 -and $unsupported.environment_error -like '*Unsupported*DPI*') 'Unknown DPI did not skip before launch.'
    [MivReleaseCheckNative]::GeometryDpi=96;[MivReleaseCheckNative]::WorkArea=@(0,0,1920,900)
    $tooSmall=Invoke-ReleaseCheckPreflight {Get-ReleaseCheckGeometryPreflight}
    Assert-True ($tooSmall.exit_code -eq 2 -and $tooSmall.environment_error -like '*does not fit*') 'Insufficient physical work area did not skip before launch.'
    [MivReleaseCheckNative]::WorkArea=@(0,0,1920,1040)
    $positioned=Get-ReleaseCheckGeometryPreflight
    Assert-True (($positioned.outer_rect -join ',') -eq '60,40,1496,979' -and ($positioned.outer_size -join ',') -eq '1436,939') 'Fake decorated rectangle/extent changed.'
    # The extent fits 1920x960, but the fixed y=40 puts its bottom at 979.
    [MivReleaseCheckNative]::WorkArea=@(0,0,1920,960)
    Assert-True ($positioned.outer_size[0] -le 1920 -and $positioned.outer_size[1] -le 960) 'Position regression no longer exercises a size-only false positive.'
    $outsideAtStartup=Invoke-ReleaseCheckPreflight {Get-ReleaseCheckGeometryPreflight}
    Assert-True ($outsideAtStartup.exit_code -eq 2 -and $outsideAtStartup.environment_error -like '*does not fit*') 'Fixed startup position outside work area was accepted.'
    foreach($area in @(@(50,30,1920,1040),@(-100,-50,1820,990))){
        [MivReleaseCheckNative]::WorkArea=$area
        $shifted=Get-ReleaseCheckGeometryPreflight
        Assert-True (($shifted.outer_rect -join ',') -eq '60,40,1496,979' -and ($shifted.work_area -join ',') -eq ($area -join ',')) 'Startup origin was incorrectly made relative to the work area.'
    }
    foreach($area in @(@(61,0,1920,1040),@(0,41,1920,1040))){
        [MivReleaseCheckNative]::WorkArea=$area
        $shifted=Invoke-ReleaseCheckPreflight {Get-ReleaseCheckGeometryPreflight}
        Assert-True ($shifted.exit_code -eq 2) 'Fixed window outside a shifted work-area origin was accepted.'
    }
    [MivReleaseCheckNative]::WorkArea=@(0,0,1920,1040)
    $observed=Get-ReleaseCheckWindowGeometry 101 888
    Assert-True (($observed.client_size -join ',') -eq '1420,900' -and $observed.pid -eq 101 -and $observed.root_hwnd -eq 888) 'Native client geometry lost exact root identity.'
    Assert-True (($observed.window_rect -join ',') -eq '60,40,1496,979' -and ($observed.work_area -join ',') -eq '0,0,1920,1040') 'Actual window/work-area evidence lost physical coordinates.'
    $expected=Get-ReleaseCheckCaseGeometry 'static-foreground' 96
    Assert-ReleaseCheckWindowGeometry $observed $expected
    foreach($rect in @(@(-1,40,1435,979),@(60,-1,1496,938),@(500,40,1936,979),@(60,110,1496,1049))){
        $observed.window_rect=$rect
        Assert-Throws {Assert-ReleaseCheckWindowGeometry $observed $expected} 'Actual window crossing a work-area edge was accepted.'
    }
    $observed.window_rect=@(60,40,1496,979);$observed.work_area=@(50,30,1920,1040)
    Assert-ReleaseCheckWindowGeometry $observed $expected
    $observed.work_area=@(61,30,1920,1040)
    Assert-Throws {Assert-ReleaseCheckWindowGeometry $observed $expected} 'Actual window left of a shifted work-area origin was accepted.'
    Assert-Throws {Get-ReleaseCheckWindowGeometry 102 888} 'Another PID supplied window geometry.'
    foreach($relative in @('ui-smoke\release-checks\Run.ps1','relcheck-kit\Test-SubPcEnvironment.ps1')){
        $ast=Get-TestAst (Join-Path $PSScriptRoot $relative)
        $gates=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Get-ReleaseCheckGeometryPreflight'},$true))
        Assert-True ($gates.Count -eq 1) "Shared geometry preflight missing: $relative"
        $effects=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -in @('Start-Process','New-ReleaseCheckForegroundProbeChild','New-Item','Open-UiSmokeRunnerLock')},$true))
        foreach($effect in $effects){Assert-True ($gates[0].Extent.StartOffset -lt $effect.Extent.StartOffset) "Geometry preflight follows side effects: $relative"}
    }
    $runner=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1') -Encoding UTF8 -Raw
    Assert-True ($runner.Contains("(`$geometry.window_size -join 'x')") -and -not $runner.Contains("'--window-size','1328x900'")) 'Launch dimensions do not use selected geometry.'
    Assert-True ($runner.Contains('--dpi $metadata.static_geometry.dpi') -and $runner.Contains('$metadata.window_size[$CaseId]')) 'Fixture DPI/per-case metadata do not use selected geometry.'
    $sampler=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Sampler.ps1') -Encoding UTF8 -Raw
    Assert-True ($sampler.Contains('Get-ReleaseCheckWindowSample $ProcessId -RootHwnd $sampleRoot') -and $sampler.Contains('Assert-ReleaseCheckWindowGeometry $sample.window_geometry $script:windowPlan')) 'Static background geometry is not validated throughout sampling.'
    $samplerAst=Get-TestAst (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Sampler.ps1')
    $checks=@($samplerAst.FindAll({param($n)$n -is [Management.Automation.Language.IfStatementAst] -and
        $n.Clauses[0].Item1.Extent.Text -eq "`$Scenario -like 'static-*'" -and
        $n.Clauses[0].Item2.Extent.Text.Contains('Assert-ReleaseCheckWindowGeometry')},$true))
    Assert-True ($checks.Count -eq 1) 'Sampler geometry-validation block missing.'
    $script:windowPlan=Get-ReleaseCheckCaseGeometry 'static-background' 96
    foreach($Scenario in @('static-foreground','static-background')){
        $report=@{environment_errors=@()}
        $samples=@([pscustomobject]@{window_geometry=[pscustomobject]@{dpi=96;client_size=@(1420,900);window_rect=@(60,40,1496,979);work_area=@(0,0,1920,1040)}},
                   [pscustomobject]@{window_geometry=[pscustomobject]@{dpi=96;client_size=@(1420,900);window_rect=@(60,40,1496,979);work_area=@(0,0,1920,1040)}})
        & ([scriptblock]::Create($checks[0].Extent.Text))
        Assert-True ($report.environment_errors.Count -eq 0) 'Matching native sample geometry rejected.'
        $samples[1].window_geometry.client_size=@(1328,900)
        & ([scriptblock]::Create($checks[0].Extent.Text))
        Assert-True (($report.environment_errors -join ' ') -like '*client size differs*') 'Resize after foreground/background readiness accepted.'
        $report=@{environment_errors=@()};$samples[1].window_geometry.client_size=@(1420,900)
        $samples[1].window_geometry.window_rect=@(60,110,1496,1049)
        & ([scriptblock]::Create($checks[0].Extent.Text))
        Assert-True (($report.environment_errors -join ' ') -like '*does not fit*') 'Window leaving work area during foreground/background sampling was accepted.'
        $report=@{environment_errors=@()};$samples[1].window_geometry.window_rect=@(60,40,1496,979)
        $samples[1].window_geometry.work_area=@(0,0,1920,960)
        & ([scriptblock]::Create($checks[0].Extent.Text))
        Assert-True (($report.environment_errors -join ' ') -like '*does not fit*') 'A changed work area was ignored during sampling.'
    }
    # Compile the real native declarations without invoking any imported API.
    $support=Get-TestAst (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Support.ps1')
    $native=@($support.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Initialize-ReleaseCheckNative'},$true))[0]
    $compile=@($native.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Add-Type'},$true))[0]
    $source=$compile.CommandElements[1].Value.Replace('MivReleaseCheckNative','MivReleaseCheckGeometryCompilationOnly')
    Assert-True ($source.Contains('SetThreadDpiAwarenessContext') -and $source.Contains('AdjustWindowRectExForDpi') -and $source.Contains('GetClientRect')) 'Physical geometry native declarations missing.'
    Add-Type -TypeDefinition $source
}
& {
    # Saved foreground evidence only: the native capture is a fake and the
    # classifier reads synthetic files. No desktop or app is touched.
    $temporaryBase=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
    $temporary=Join-Path $temporaryBase ('miv-foreground-safety-'+[Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $temporary | Out-Null
    $utf8=New-Object Text.UTF8Encoding($false)
    function New-FakeForegroundRecord {
        param([long]$Time)
        return [pscustomobject][ordered]@{
            schema_version=1;unix_ms=$Time;app_pid=101;root_hwnd=888;root_pid=101
            root_alive=$true;root_visible=$true;root_minimized=$false;root_hung=$false
            foreground_hwnd=999;foreground_pid=202;foreground_process_name='other.exe';foreground_class='OtherClass'
            foreground_descendant=$false;ownership_resolved=$true
            ghost_original_hwnd=0;ghost_original_pid=0;ghost_original_descendant=$null;ghost_original_process_name=''
        }
    }
    try{
        $timeline=New-ReleaseCheckForegroundTimeline $temporary 101
        [MivReleaseCheckNative]::ForegroundSample=New-FakeForegroundRecord 1000
        Write-ReleaseCheckForegroundTimeline $timeline 888
        [MivReleaseCheckNative]::ForegroundSample=New-FakeForegroundRecord 1400
        Write-ReleaseCheckForegroundTimeline $timeline 777
        Assert-True ($timeline.root_hwnd -eq 888) 'Foreground recorder followed a replacement root.'
        Assert-True (@(Get-Content -LiteralPath $timeline.path).Count -eq 1) 'Unchanged foreground emitted before the heartbeat.'
        [MivReleaseCheckNative]::ForegroundSample=New-FakeForegroundRecord 1500
        Write-ReleaseCheckForegroundTimeline $timeline
        [MivReleaseCheckNative]::ForegroundSample=New-FakeForegroundRecord 2000
        Write-ReleaseCheckForegroundTimeline $timeline
        [MivReleaseCheckNative]::ForegroundSample=New-FakeForegroundRecord 2100
        [MivReleaseCheckNative]::ForegroundSample.foreground_class='ChangedClass'
        Write-ReleaseCheckForegroundTimeline $timeline
        [MivReleaseCheckNative]::ForegroundSample=New-FakeForegroundRecord 2200
        [MivReleaseCheckNative]::ForegroundSample.foreground_class='ChangedClass'
        Write-ReleaseCheckForegroundTimeline $timeline -Force
        $rows=@(Get-Content -LiteralPath $timeline.path -Encoding UTF8 | ForEach-Object {$_ | ConvertFrom-Json})
        Assert-True (($rows.unix_ms -join ',') -eq '1000,1500,2000,2100,2200') 'Foreground change/heartbeat/final evidence cadence changed.'
        $failurePath=Join-Path $temporary 'foreground-wait-failure.json'
        $failure=@{schema_version=1;pid=101;root_hwnd=888;wait_kind='exact-root-foreground';wait_start_unix_ms=1500;failure_unix_ms=2100}
        Write-ReleaseCheckJson $failurePath $failure
        $result=New-ReleaseCheckProcessExitResult 'seed-profile' 1 101 $temporary
        Assert-True ($result.verdict -eq 'SKIPPED' -and $result.exit_code -eq 2 -and $result.evidence.process.process_exit_code -eq 1) 'External typed wait did not preserve raw exit 1 and classify SKIPPED/2.'
        Assert-True (($result.environment_errors -join ' ') -like '*other.exe*pid=202*1500*' -or ($result.environment_errors -join ' ') -like '*1500*other.exe*pid=202*') 'Foreground classification reason lost interval/identity.'
        $result=New-ReleaseCheckProcessExitResult 'seed-profile' 1 101 $temporary -ForegroundRecordingError 'fake append failure'
        Assert-True ($result.verdict -eq 'FAIL' -and $result.exit_code -eq 1) 'Failed foreground recording hid process exit 1.'
        # A same-PID popup is a potential product regression, not an environment excuse.
        $bad=New-FakeForegroundRecord 2000;$bad.foreground_pid=101;$bad.foreground_hwnd=12345
        $lines=@((New-FakeForegroundRecord 1000),$bad,(New-FakeForegroundRecord 2200)) | ForEach-Object {$_ | ConvertTo-Json -Compress}
        [IO.File]::WriteAllText($timeline.path,($lines -join "`n")+"`n",$utf8)
        $result=New-ReleaseCheckProcessExitResult 'seed-profile' 1 101 $temporary
        Assert-True ($result.verdict -eq 'FAIL' -and $result.exit_code -eq 1 -and ($result.failures -join ' ') -like '*application or descendant*') 'Same-PID foreground wait was hidden as SKIPPED.'
        [IO.File]::WriteAllText($failurePath,'not json',$utf8)
        $result=New-ReleaseCheckProcessExitResult 'seed-profile' 1 101 $temporary
        Assert-True ($result.verdict -eq 'FAIL' -and $result.exit_code -eq 1) 'Unreadable typed failure was hidden as SKIPPED.'
        Remove-Item -LiteralPath $failurePath
        $result=New-ReleaseCheckProcessExitResult 'seed-profile' 1 101 $temporary
        Assert-True ($result.verdict -eq 'FAIL' -and $result.failures[0].StartsWith('seed-profile application exit code 1; foreground timeline rows=')) 'Untyped script failure changed classification or lost saved foreground evidence.'
        $runAst=Get-TestAst (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1')
        $session=@($runAst.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Invoke-ReleaseSession'},$true))[0]
        $calls=@($session.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Write-ReleaseCheckForegroundTimeline'},$true))
        $completion=@($runAst.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Complete-ReleaseSession'},$true))[0]
        $finalCalls=@($completion.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Write-ReleaseCheckForegroundTimeline'},$true))
        Assert-True ($calls.Count -eq 1 -and $finalCalls.Count -eq 1 -and $finalCalls[0].Extent.Text.Contains('-Force') -and $session.Body.Extent.Text.Contains('Complete-ReleaseSession $CaseId $dir $foregroundTimeline')) 'Session foreground sampling/final boundary missing.'
        $focusCalls=@($session.FindAll({param($n)$n -is [Management.Automation.Language.InvokeMemberExpressionAst] -and $n.Member.Extent.Text -eq 'SetForegroundWindow'},$true))
        Assert-True ($focusCalls.Count -eq 2) 'Foreground instrumentation added an activation/reacquisition path.'
        $samplerLoops=@($session.FindAll({param($n)$n -is [Management.Automation.Language.WhileStatementAst] -and $n.Condition.Extent.Text -eq '-not $sampler.HasExited'},$true))
        Assert-True ($samplerLoops.Count -eq 1 -and -not $samplerLoops[0].Body.Extent.Text.Contains('Write-ReleaseCheckForegroundTimeline')) 'Foreground instrumentation entered the sampler measurement loop.'
        $support=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Support.ps1') -Encoding UTF8 -Raw
        Assert-True (-not $support.Contains('GetWindowText') -and $support.Contains('HungWindowFromGhostWindow') -and $support.Contains('IsHungAppWindow') -and $support.Contains('Process32FirstW') -and $support.Contains('GetProcessTimes')) 'Foreground capture uses unsafe messages or lacks native ownership/health evidence.'
        $script=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'ui-smoke\release-checks.rhai') -Encoding UTF8 -Raw
        Assert-True (($script.Split(@('release_check_foreground_wait_failure('),[StringSplitOptions]::None).Count-1) -eq 2 -and -not $script.Contains('environment:')) 'Foreground wait allowlist/non-foreground failure messages changed.'
        Assert-True ($script.Contains('release_check_foreground_wait_failure("exact-root-foreground", wait_start)') -and
            $script.Contains('release_check_foreground_wait_failure("ctrl-g-search-focus", wait_start)') -and
            $script.Contains('for attempt in 0..200') -and $script.Contains('for attempt in 0..300')) 'Foreground wait kinds or original wait bounds changed.'
    }finally{
        $resolved=[IO.Path]::GetFullPath($temporary)
        if(-not $resolved.StartsWith($temporaryBase+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unsafe foreground test cleanup path.'}
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
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
    & ([scriptblock]::Create($finally.Extent.Text.TrimStart('{').TrimEnd('}')))
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
    $script:windowPlan=Get-ReleaseCheckCaseGeometry 'static-background' 96
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
    function Get-ReleaseCheckWindowSample {param($ProcessId,$RootHwnd) [pscustomobject]@{unix_ms=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds();foreground_pid=101;foreground_hwnd=1001;total=1;visible=1;window_geometry=[pscustomobject]@{dpi=96;client_size=@(1420,900);window_rect=@(60,40,1496,979);work_area=@(0,0,1920,1040)}}}
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
        $manifest=[ordered]@{schema_version=1;kit_id='relcheck-fake';scripts_source_head=('b'*40);artifact=@{source_head=('a'*40);source_fingerprint_sha256=$fingerprint;manifest=$build;build_root=$root};ffmpeg=@{};files=@()}
        Seal-FakeKit $root $manifest
        return $root
    }
    function Invoke-FakeResultReceiver {
        param([string]$KitPath,[string]$ResultPath)
        $result=Invoke-KitSafetyChild @('-File',(Join-Path $PSScriptRoot 'relcheck-kit\Receive-ReleaseCheckResult.ps1'),'-Kit',$KitPath,'-Result',$ResultPath)
        return $result.code
    }
    function Write-FakeReleaseRecord {
        param([string]$KitPath,[string]$ResultPath,[string]$SummaryPath)
        if(-not $SummaryPath){$SummaryPath=Join-Path $ResultPath 'summary.json'}
        $generate=@'
import json
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from release_check_analysis import release_record
summary = json.loads(Path(sys.argv[2]).read_text(encoding='utf-8'))
Path(sys.argv[3]).write_text(release_record(summary), encoding='utf-8')
'@
        & python -I -B -c $generate (Join-Path $KitPath 'scripts') $SummaryPath (Join-Path $ResultPath 'release-record.md')
        Assert-True ($LASTEXITCODE -eq 0) 'Could not generate fixture release record.'
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
        Assert-ReleaseCheckKitRunRoot $root
        Assert-ReleaseCheckKitRunRoot ($root.ToUpperInvariant()+'\')
        $stored=New-FakeKit 'different-build-root'
        $storedManifest=Get-Content -LiteralPath (Join-Path $stored 'release-check-kit.json') -Encoding UTF8 -Raw | ConvertFrom-Json
        $requiredRoot=Join-Path $testRoot 'required-run-root'
        $storedManifest.artifact.build_root=$requiredRoot
        Seal-FakeKit $stored $storedManifest
        $null=Assert-ReleaseCheckKit $stored # storage/Receive may use another root
        $message=''
        try{Assert-ReleaseCheckKitRunRoot $stored}catch{$message=$_.Exception.Message}
        Assert-True ($message -ceq "Kit must be placed at $requiredRoot (the diagnostic build validates evidence paths against its build root); current root is $stored.") 'Run-root mismatch did not explain the required/current roots.'
        foreach($invalidRoot in @($null,'','  ','relative-root','C:relative-root','\drive-relative-root')){
            if($null -eq $invalidRoot){$storedManifest.artifact.PSObject.Properties.Remove('build_root')}
            else{$storedManifest.artifact | Add-Member -MemberType NoteProperty -Name build_root -Value $invalidRoot -Force}
            Seal-FakeKit $stored $storedManifest
            Assert-Throws {Read-ReleaseCheckKitManifest $stored} 'Missing/invalid build_root was accepted.'
        }
        $embeddedFile=Join-Path $testRoot 'embedded-root.bin'
        $embeddedRoot='G:\'+[char]0x30C6+'\build-root'
        [byte[]]$payload=@(0,255)+[Text.Encoding]::UTF8.GetBytes($embeddedRoot)+@(0,254)
        [IO.File]::WriteAllBytes($embeddedFile,$payload)
        Assert-ReleaseCheckDiagnosticBuildRoot $embeddedFile $embeddedRoot
        $message=''
        try{Assert-ReleaseCheckDiagnosticBuildRoot $embeddedFile 'G:\different-root'}catch{$message=$_.Exception.Message}
        Assert-True ($message -ceq 'diagnostic exe does not embed the recorded build root') 'Missing embedded build root was accepted or lost its reason.'
        $support=Get-TestAst (Join-Path $PSScriptRoot 'relcheck-kit\KitSupport.ps1')
        $embedding=$support.Find({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Assert-ReleaseCheckDiagnosticBuildRoot'},$true)
        $reads=@($embedding.FindAll({param($n)$n -is [Management.Automation.Language.InvokeMemberExpressionAst] -and $n.Member.Extent.Text -eq 'ReadAllBytes'},$true))
        Assert-True ($reads.Count -eq 1) 'Embedded build-root check reads the executable more than once.'
        & {
            $hashPaths=New-Object 'Collections.Generic.List[string]'
            function Get-ChildItem {throw 'Light kit validation enumerated the tree.'}
            function Assert-ReleaseCheckKit {throw 'Light kit validation called full validation.'}
            function Get-FileHash {
                param($LiteralPath,$Algorithm,$ErrorAction)
                $hashPaths.Add($LiteralPath)
                Microsoft.PowerShell.Utility\Get-FileHash -LiteralPath $LiteralPath -Algorithm $Algorithm -ErrorAction Stop
            }
            Assert-True ((Get-ReleaseCheckKitExpectedFingerprint $root) -eq $fingerprint) 'Light kit validation returned wrong fingerprint.'
            Assert-True ($hashPaths.Count -eq 2 -and $hashPaths[0] -eq (Join-Path $root 'release-check-kit.json') -and
                $hashPaths[1] -eq (Join-Path $root 'source-fingerprint-records.txt')) 'Light kit validation hashed payload files.'
        }
        foreach($relative in @('target\ui-smoke\runs\fake\result.json','target\portable-smoke\data\fake.db')){
            Write-FakeFile (Join-Path $root $relative) 'runtime output'
        }
        Assert-True ((Assert-ReleaseCheckKit $root) -eq $fingerprint) 'Runtime outputs invalidated kit.'
        foreach($relative in @('scripts\__pycache__\x.pyc','scripts\nested\extra.pyc','target\ui-smoke\__pycache__\x.pyc','target\portable-smoke\extra.pyc')){
            $dirty=New-FakeKit ('bytecode-'+[Guid]::NewGuid().ToString('N'))
            Write-FakeFile (Join-Path $dirty $relative) 'extra bytecode'
            $message=''
            try{$null=Assert-ReleaseCheckKit $dirty}catch{$message=$_.Exception.Message}
            Assert-True ($message -like '*__pycache__*') "Python bytecode was not rejected explicitly: $relative"
        }
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
                ($n.Clauses[0].Item2.Extent.Text.Contains('Assert-ReleaseCheckKit $repo') -or
                 $n.Clauses[0].Item2.Extent.Text.Contains('Get-ReleaseCheckKitExpectedFingerprint $repo'))},$true))
            Assert-True ($kitBranches.Count -ge 1) "Kit preflight missing: $relative"
            & {
                $FromKit=$true;$repo=$root;$scripts=$PSScriptRoot
                function Get-MivSourceFingerprint {throw 'Kit preflight invoked git/source fingerprinting.'}
                $oldBytecode=$env:PYTHONDONTWRITEBYTECODE
                try{
                    . ([scriptblock]::Create($kitBranches[0].Clauses[0].Item2.Extent.Text.TrimStart('{').TrimEnd('}')))
                    Assert-True ($expectedFingerprint -eq $fingerprint -and $env:PYTHONDONTWRITEBYTECODE -eq '1') 'Kit preflight lost verified fingerprint/bytecode policy.'
                    $inherited=Invoke-KitSafetyChild @('-Command','[Environment]::GetEnvironmentVariable(''PYTHONDONTWRITEBYTECODE'')')
                    Assert-True ($inherited.code -eq 0 -and $inherited.text.Trim() -eq '1') 'Child shell did not inherit bytecode suppression.'
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
        foreach($mutation in @('byte','missing','extra','records','digest','package-fingerprint','package-extra-field','git','exe')){
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
                'package-extra-field' {
                    $path=Join-Path $root 'target\portable-smoke-package\.test-script-build.json'
                    $build=Get-Content -LiteralPath $path -Encoding UTF8 -Raw | ConvertFrom-Json
                    $build | Add-Member -MemberType NoteProperty -Name extra_field -Value 'unrecorded'
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
            if($mutation -in @('records','digest','package-fingerprint','package-extra-field')){
                Assert-Throws {Get-ReleaseCheckKitExpectedFingerprint $root} "Light kit validation accepted $mutation"
            }
        }
        $root=New-FakeKit 'receiver'
        $result=Join-Path $testRoot 'returned-run'
        $manifest=Get-Content -LiteralPath (Join-Path $root 'release-check-kit.json') -Encoding UTF8 -Raw | ConvertFrom-Json
        foreach($name in @('release_check_analysis.py','analyze_perf.py','release_check_geometry.py','release_check_geometry.json')){
            Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination (Join-Path $root ('scripts\'+$name))
        }
        Seal-FakeKit $root $manifest
        $meta=[ordered]@{execution_mode='kit';kit=@{kit_id=$manifest.kit_id;kit_manifest_sha256=(Get-Content -LiteralPath (Join-Path $root 'release-check-kit.sha256') -Raw).Trim();scripts_source_head=$manifest.scripts_source_head;artifact_source_head=$manifest.artifact.source_head};exe_sha256=$manifest.artifact.manifest.core_sha256;source=($manifest.artifact.manifest.source_head+'/'+$manifest.artifact.source_fingerprint_sha256);run_path='C:\sub-pc\kit\target\ui-smoke\runs\fake';artifact=@{source_fingerprint_sha256=$manifest.artifact.source_fingerprint_sha256}}
        Write-FakeFile (Join-Path $result 'suite-input.json') (@{metadata=$meta}|ConvertTo-Json -Depth 10)
        foreach($entry in (@{PASS=0;PASS_WITH_WARNINGS=0;FAIL=1;SKIPPED=2}).GetEnumerator()){
            Write-FakeFile (Join-Path $result 'summary.json') (@{verdict=$entry.Key;exit_code=$entry.Value;metadata=$meta}|ConvertTo-Json -Depth 10)
            Write-FakeReleaseRecord $root $result
            Assert-True ((Invoke-FakeResultReceiver $root $result) -eq $entry.Value) "Receiver lost verified summary verdict $($entry.Key)"
        }
        $validSummary=@{verdict='PASS';exit_code=0;metadata=$meta}|ConvertTo-Json -Depth 10
        $validSuite=@{metadata=$meta}|ConvertTo-Json -Depth 10
        Write-FakeFile (Join-Path $result 'summary.json') $validSummary
        Write-FakeReleaseRecord $root $result
        $recordPath=Join-Path $result 'release-record.md'
        $validRecord=[IO.File]::ReadAllText($recordPath)
        foreach($record in @($validRecord.Replace("`r`n","`n"),$validRecord.Replace("`r`n","`n").Replace("`n","`r`n"))){
            Write-FakeFile $recordPath $record
            $before=(Get-FileHash -LiteralPath $recordPath -Algorithm SHA256).Hash
            Assert-True ((Invoke-FakeResultReceiver $root $result) -eq 0) 'Receiver rejected matching release record/newline normalization.'
            Assert-True ((Get-FileHash -LiteralPath $recordPath -Algorithm SHA256).Hash -eq $before) 'Receiver rewrote the release record.'
        }
        $otherSummary=$validSummary|ConvertFrom-Json
        $otherSummary.metadata.run_path='C:\sub-pc\kit\target\ui-smoke\runs\other'
        $otherSummaryPath=Join-Path $testRoot 'other-run-summary.json'
        Write-FakeFile $otherSummaryPath ($otherSummary|ConvertTo-Json -Depth 10)
        Write-FakeReleaseRecord $root $result $otherSummaryPath
        Assert-True ((Invoke-FakeResultReceiver $root $result) -eq 3) 'Receiver accepted another run release record.'
        Write-FakeFile $recordPath ('+'+$validRecord.Substring(1))
        Assert-True ((Invoke-FakeResultReceiver $root $result) -eq 3) 'Receiver accepted a one-character release record change.'
        Write-FakeFile $recordPath 'fake release record'
        Assert-True ((Invoke-FakeResultReceiver $root $result) -eq 3) 'Receiver accepted an arbitrary release record.'
        Write-FakeFile $recordPath $validRecord
        $invalidRendering=$validSummary|ConvertFrom-Json
        $invalidRendering | Add-Member -MemberType NoteProperty -Name cases -Value @(1)
        Write-FakeFile (Join-Path $result 'summary.json') ($invalidRendering|ConvertTo-Json -Depth 10)
        Assert-True ((Invoke-FakeResultReceiver $root $result) -eq 3) 'Receiver accepted a release_record generation failure.'
        Write-FakeFile (Join-Path $result 'summary.json') $validSummary
        $noPython=Join-Path $testRoot 'receive-without-python.ps1'
        Write-FakeFile $noPython 'param($Receiver,$Kit,$Result) $env:PATH=""; & $Receiver -Kit $Kit -Result $Result; exit $LASTEXITCODE'
        $missingPython=Invoke-KitSafetyChild @('-File',$noPython,'-Receiver',(Join-Path $PSScriptRoot 'relcheck-kit\Receive-ReleaseCheckResult.ps1'),'-Kit',$root,'-Result',$result)
        Assert-True ($missingPython.code -eq 3) 'Receiver did not classify missing Python as validation failure.'
        $null=Assert-ReleaseCheckKit $root # isolated imports did not create bytecode
        foreach($field in @('execution_mode','kit_id','kit_manifest_sha256','scripts_source_head','artifact_source_head','exe_sha256','source','run_path')){
            foreach($which in @('summary-only','both')){
                $summary=$validSummary|ConvertFrom-Json
                $suite=$validSuite|ConvertFrom-Json
                if($field -in @('kit_id','kit_manifest_sha256','scripts_source_head','artifact_source_head')){
                    $summary.metadata.kit.$field='wrong'
                    if($which -eq 'both'){$suite.metadata.kit.$field='wrong'}
                }else{
                    $summary.metadata.$field=if($field -eq 'run_path' -and $which -eq 'both'){''}else{'wrong'}
                    if($which -eq 'both'){$suite.metadata.$field=$summary.metadata.$field}
                }
                Write-FakeFile (Join-Path $result 'summary.json') ($summary|ConvertTo-Json -Depth 10)
                Write-FakeFile (Join-Path $result 'suite-input.json') ($suite|ConvertTo-Json -Depth 10)
                Assert-True ((Invoke-FakeResultReceiver $root $result) -eq 3) "Receiver accepted $which metadata mismatch: $field"
            }
        }
        Write-FakeFile (Join-Path $result 'suite-input.json') $validSuite
        foreach($invalid in @(@{verdict='PASS';exit_code=1},@{verdict='PASS_WITH_WARNINGS';exit_code=2},@{verdict='FAIL';exit_code=0},@{verdict='SKIPPED';exit_code=0},@{verdict='UNKNOWN';exit_code=0},@{verdict='pass';exit_code=0},@{verdict='PASS';exit_code='0'},@{verdict='PASS';exit_code=3})){
            $invalid.metadata=$meta
            Write-FakeFile (Join-Path $result 'summary.json') ($invalid|ConvertTo-Json -Depth 10)
            Assert-True ((Invoke-FakeResultReceiver $root $result) -eq 3) 'Receiver accepted invalid summary verdict/exit mapping.'
        }
        foreach($file in @('summary.json','suite-input.json')){
            $other=if($file -eq 'summary.json'){'suite-input.json'}else{'summary.json'}
            Write-FakeFile (Join-Path $result $other) $(if($other -eq 'summary.json'){$validSummary}else{$validSuite})
            foreach($invalid in @('{','{}','{"metadata":null}','{"metadata":[]}')){
                Write-FakeFile (Join-Path $result $file) $invalid
                Assert-True ((Invoke-FakeResultReceiver $root $result) -eq 3) "Receiver accepted malformed/incomplete $file"
            }
            Remove-Item -LiteralPath (Join-Path $result $file)
            Assert-True ((Invoke-FakeResultReceiver $root $result) -eq 3) "Receiver accepted missing $file"
        }
        Write-FakeFile (Join-Path $result 'suite-input.json') $validSuite
        Write-FakeFile (Join-Path $result 'summary.json') $validSummary
        Remove-Item -LiteralPath (Join-Path $result 'release-record.md')
        Assert-True ((Invoke-FakeResultReceiver $root $result) -eq 3) 'Receiver accepted missing release record.'

        if(Get-Command git -ErrorAction SilentlyContinue){
            # Commits exist only in these disposable fixture repositories. No
            # workspace staging, commits, global config, hooks or signing run.
            function Invoke-FixtureGit {
                param([string]$Root,[string[]]$Arguments)
                $ErrorActionPreference='Continue'
                $output=@(& git -C $Root -c core.autocrlf=false -c commit.gpgSign=false -c "core.hooksPath=$testRoot\empty-hooks" -c user.name=KitSafety -c user.email=kit-safety@example.invalid @Arguments 2>&1)
                Assert-True ($LASTEXITCODE -eq 0) ('Fixture git failed: '+($output -join ' '))
                return $output
            }
            $creatorRepo=Join-Path $testRoot 'creator-repo'
            foreach($relative in @('relcheck-kit\New-ReleaseCheckKit.ps1','relcheck-kit\KitSupport.ps1','ui-smoke\release-checks\Support.ps1')){
                $destination=Join-Path $creatorRepo ('scripts\'+$relative)
                New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force | Out-Null
                Copy-Item -LiteralPath (Join-Path $PSScriptRoot $relative) -Destination $destination
            }
            $null=Invoke-FixtureGit $creatorRepo @('init','--quiet')
            $null=Invoke-FixtureGit $creatorRepo @('add','--','scripts')
            $null=Invoke-FixtureGit $creatorRepo @('commit','--quiet','-m','fixture scripts')
            $artifactRepo=New-FakeKit 'artifact-repo'
            Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'build-portable.ps1') -Destination (Join-Path $artifactRepo 'scripts\build-portable.ps1')
            Write-FakeFile (Join-Path $artifactRepo 'Cargo.toml') '# fingerprint fixture; never build'
            $null=Invoke-FixtureGit $artifactRepo @('init','--quiet')
            $null=Invoke-FixtureGit $artifactRepo @('add','--','Cargo.toml','scripts/build-portable.ps1')
            $null=Invoke-FixtureGit $artifactRepo @('commit','--quiet','-m','fixture artifact source')
            Import-ReleaseCheckFunctions (Join-Path $PSScriptRoot 'build-portable.ps1') @('Get-NormalizedPath','Get-MivOrdinalUniquePaths','Get-MivSourceFingerprintRecords','Get-MivSourceFingerprint')
            $buildPath=Join-Path $artifactRepo 'target\portable-smoke-package\.test-script-build.json'
            $build=Get-Content -LiteralPath $buildPath -Encoding UTF8 -Raw | ConvertFrom-Json
            $build.source_head=[string](Invoke-FixtureGit $artifactRepo @('rev-parse','HEAD'))
            $build.source_fingerprint_sha256=Get-MivSourceFingerprint $artifactRepo
            $corePath=Join-Path $artifactRepo 'target\portable-smoke-package\mimageviewer.exe'
            $coreText='fake diagnostic core; embedded root: '+$artifactRepo
            Write-FakeFile $corePath $coreText
            $build.core_sha256=(Get-FileHash -LiteralPath $corePath -Algorithm SHA256).Hash.ToLowerInvariant()
            Write-FakeFile $buildPath ($build|ConvertTo-Json -Depth 5)
            $fakeFfmpeg=Join-Path $testRoot 'fake-tools\ffmpeg.ps1'
            Write-FakeFile $fakeFfmpeg 'param([switch]$version) "ffmpeg fake test version"; $global:LASTEXITCODE=0'
            Write-FakeFile (Join-Path $testRoot 'fake-tools\test.dll') 'fake ffmpeg dependency'
            $creator=Join-Path $creatorRepo 'scripts\relcheck-kit\New-ReleaseCheckKit.ps1'
            $outputRoot=Join-Path $testRoot 'created-kits'
            $creation=Invoke-KitSafetyChild @('-File',$creator,'-ArtifactRepo',$artifactRepo,'-OutputRoot',$outputRoot,'-FfmpegPath',$fakeFfmpeg)
            Assert-True ($creation.code -eq 0) ('Fixture kit creation failed: '+$creation.text)
            $kits=@(Get-ChildItem -LiteralPath $outputRoot -Directory)
            Assert-True ($kits.Count -eq 1 -and (Assert-ReleaseCheckKit $kits[0].FullName) -eq $build.source_fingerprint_sha256) 'Created fixture kit did not self-verify.'
            $createdManifest=Read-ReleaseCheckKitManifest $kits[0].FullName
            Assert-True ($createdManifest.artifact.build_root -ceq ([IO.Path]::GetFullPath($artifactRepo).TrimEnd('\')) -and
                $creation.text.Contains("Run root on the sub-PC: $artifactRepo")) 'Kit creation did not record/display the normalized artifact build root.'
            Assert-Throws {Assert-ReleaseCheckKitRunRoot $kits[0].FullName} 'Stored kit incorrectly accepted its storage root as the run root.'
            Assert-ReleaseCheckDiagnosticBuildRoot (Join-Path $kits[0].FullName 'target\portable-smoke-package\mimageviewer.exe') $createdManifest.artifact.build_root
            Assert-True (Test-Path -LiteralPath (Join-Path $kits[0].FullName 'tools\ffmpeg\test.dll')) 'Kit creation lost companion FFmpeg DLL.'
            # Even a hash-consistent package must embed the recorded source root.
            Write-FakeFile $corePath 'fake core without a build root'
            $build.core_sha256=(Get-FileHash -LiteralPath $corePath -Algorithm SHA256).Hash.ToLowerInvariant()
            Write-FakeFile $buildPath ($build|ConvertTo-Json -Depth 5)
            $noRootOutput=Join-Path $testRoot 'rejected-embedded-root'
            $creation=Invoke-KitSafetyChild @('-File',$creator,'-ArtifactRepo',$artifactRepo,'-OutputRoot',$noRootOutput,'-FfmpegPath',$fakeFfmpeg)
            Assert-True ($creation.code -ne 0 -and $creation.text.Contains('diagnostic exe does not embed the recorded build root') -and
                -not (Test-Path -LiteralPath $noRootOutput)) 'Kit creation accepted a core without the recorded build root.'
            Write-FakeFile $corePath $coreText
            $build.core_sha256=(Get-FileHash -LiteralPath $corePath -Algorithm SHA256).Hash.ToLowerInvariant()
            Write-FakeFile $buildPath ($build|ConvertTo-Json -Depth 5)
            # scripts/build-portable.ps1 is outside the product fingerprint list:
            # this independent gate must reject an uncommitted imported helper.
            Add-Content -LiteralPath (Join-Path $artifactRepo 'scripts\build-portable.ps1') -Encoding ASCII -Value '# uncommitted imported helper'
            $dirtyOutput=Join-Path $testRoot 'rejected-kits'
            $creation=Invoke-KitSafetyChild @('-File',$creator,'-ArtifactRepo',$artifactRepo,'-OutputRoot',$dirtyOutput,'-FfmpegPath',$fakeFfmpeg)
            Assert-True ($creation.code -ne 0 -and $creation.text -like '*Artifact scripts/build-portable.ps1 must be committed and clean*' -and
                -not (Test-Path -LiteralPath $dirtyOutput)) 'Dirty ArtifactRepo helper was imported or packaged.'
        }else{Write-Host 'Fixture kit creation: SKIPPED (git unavailable)'}

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
        $fullChecks=@($sampler.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Assert-ReleaseCheckKit'},$true))
        Assert-True ($fullChecks.Count -eq 0) 'Sampler rehashes the full kit during measurement preparation.'
        $calls=@($sampler.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Assert-ReleaseCheckArtifact'},$true))
        Assert-True ($calls.Count -eq 1 -and $calls[0].Extent.Text.Contains('-ExpectedSourceFingerprint')) 'Sampler lost explicit fingerprint.'
        $runnerText=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1') -Encoding UTF8 -Raw
        $healthText=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'check-idle-health.ps1') -Encoding UTF8 -Raw
        Assert-True ($runnerText.Contains("if(`$FromKit){`$samplerArgs+='-FromKit'}") -and $healthText.Contains('-FromKit:$FromKit')) 'Kit mode did not reach the idle sampler.'
        $captureAst=Get-TestAst (Join-Path $PSScriptRoot 'ui-smoke.ps1')
        $capture=$captureAst.Find({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Invoke-UiSmokeCapturedProcess'},$true)
        foreach($ast in @($capture,(Get-TestAst (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Run.ps1')))){
            $resets=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Start-Process' -and
                @($n.CommandElements | Where-Object {$_ -is [Management.Automation.Language.CommandParameterAst] -and $_.ParameterName -in @('UseNewEnvironment','Environment')}).Count -gt 0},$true))
            Assert-True ($resets.Count -eq 0) 'Python/sampler child launch replaces the inherited environment.'
        }
        Assert-True ($healthText.IndexOf('Sampler.ps1') -lt $healthText.IndexOf('exit $LASTEXITCODE') -and
            $healthText.IndexOf('exit $LASTEXITCODE') -lt $healthText.IndexOf('$Python = Get-Command python')) 'PortableSmoke reaches the normal-profile analyzer instead of returning from Sampler.'
        # All Python starts occur after kit preflight. Start-Process and direct
        # native invocation inherit the environment; no child environment is reset.
        foreach($relative in @('ui-smoke\release-checks\Run.ps1','ui-smoke\release-checks\Sampler.ps1','relcheck-kit\Test-SubPcEnvironment.ps1')){
            $path=Join-Path $PSScriptRoot $relative
            $ast=Get-TestAst $path
            $assignments=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.AssignmentStatementAst] -and
                $n.Left.Extent.Text -eq '$env:PYTHONDONTWRITEBYTECODE'},$true))
            Assert-True ($assignments.Count -eq 1 -and $assignments[0].Right.Extent.Text -eq "'1'") "Bytecode policy missing/reset: $relative"
            $pythonCalls=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and
                ($n.GetCommandName() -eq 'python' -or $n.Extent.Text.StartsWith('& $python '))},$true))
            Assert-True ($pythonCalls.Count -gt 0) "Python call inventory missing: $relative"
            foreach($call in $pythonCalls){Assert-True ($call.Extent.StartOffset -gt $assignments[0].Extent.StartOffset) "Python starts before bytecode policy: $relative"}
        }
        $probe=Get-TestAst (Join-Path $PSScriptRoot 'relcheck-kit\Test-SubPcEnvironment.ps1')
        foreach($relative in @('ui-smoke\release-checks\Run.ps1','ui-smoke\release-checks\Sampler.ps1','relcheck-kit\Test-SubPcEnvironment.ps1')){
            $ast=Get-TestAst (Join-Path $PSScriptRoot $relative)
            $checks=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Assert-ReleaseCheckKitRunRoot'},$true))
            Assert-True ($checks.Count -eq 1) "Run-root assertion missing: $relative"
            $launchesBeforeCheck=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and
                $n.GetCommandName() -in @('Initialize-ReleaseCheckNative','Start-Process','New-ReleaseCheckForegroundProbeChild') -and
                $n.Extent.StartOffset -lt $checks[0].Extent.EndOffset},$true))
            Assert-True ($launchesBeforeCheck.Count -eq 0) "Run-root assertion follows desktop/native work: $relative"
        }
        $probeEmbedding=@($probe.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Assert-ReleaseCheckDiagnosticBuildRoot'},$true))
        Assert-True ($probeEmbedding.Count -eq 1) 'Environment probe does not check the embedded diagnostic build root.'
        $receiver=Get-TestAst (Join-Path $PSScriptRoot 'relcheck-kit\Receive-ReleaseCheckResult.ps1')
        $renderCalls=@($receiver.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.Extent.Text.StartsWith('& $python ')},$true))
        Assert-True ($renderCalls.Count -eq 1 -and $renderCalls[0].Extent.Text.Contains('-I -B -c $recordCheck')) 'Receiver Python is not isolated with bytecode disabled.'
        $bytecodePolicy=@($receiver.FindAll({param($n)$n -is [Management.Automation.Language.AssignmentStatementAst] -and $n.Left.Extent.Text -eq '$env:PYTHONDONTWRITEBYTECODE'},$true))
        Assert-True ($bytecodePolicy.Count -eq 1 -and $bytecodePolicy[0].Right.Extent.Text -eq "'1'" -and
            $bytecodePolicy[0].Extent.StartOffset -lt $renderCalls[0].Extent.StartOffset) 'Receiver Python bytecode environment policy is missing or late.'
        $launches=@($probe.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Start-Process'},$true))
        Assert-True ($launches.Count -eq 1 -and -not ($launches[0].CommandElements | Where-Object {$_ -is [Management.Automation.Language.CommandParameterAst] -and $_.ParameterName -eq 'WindowStyle'})) 'Foreground child launch still uses STARTUPINFO WindowStyle.'
        $transitions=@($probe.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Set-ReleaseCheckBackgroundForeground'},$true))
        Assert-True ($transitions.Count -eq 1) 'Foreground probe does not use the suite background transition.'
        $probeText=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'relcheck-kit\Test-SubPcEnvironment.ps1') -Encoding UTF8 -Raw
        Assert-True ($probeText.Contains('Get-ReleaseCheckWindowSample $child.Id') -and -not $probeText.Contains('$child.MainWindowHandle') -and
            $probeText.Contains('$childHwnd=[IntPtr][long]$windowReady.root_hwnd')) 'Foreground probe does not use the child Form exact PID/HWND.'
        Assert-True ($launches[0].Extent.Text.Contains('-FilePath $probeExe') -and -not $probeText.Contains('ForegroundProbeChild.ps1')) 'Foreground probe still launches a PowerShell console child.'
        $builder=$probe.Find({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'New-ReleaseCheckForegroundProbeChild'},$true)
        $sources=@($builder.FindAll({param($n)$n -is [Management.Automation.Language.AssignmentStatementAst] -and $n.Left.Extent.Text -eq '$source'},$true))
        Assert-True ($sources.Count -eq 1) 'Embedded foreground child source missing.'
        $childSource=& ([scriptblock]::Create($sources[0].Right.Extent.Text))
        Assert-True (@($childSource.ToCharArray() | Where-Object {[int]$_ -gt 127}).Count -eq 0) 'Foreground child C# source is not ASCII.'
        $compilers=@($builder.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Add-Type'},$true))
        Assert-True ($compilers.Count -eq 1 -and $compilers[0].Extent.Text.Contains('-OutputType WindowsApplication') -and
            $compilers[0].Extent.Text.Contains('-OutputAssembly $OutputPath')) 'Foreground child is not compiled as a GUI executable.'
        Import-ReleaseCheckFunctions (Join-Path $PSScriptRoot 'relcheck-kit\Test-SubPcEnvironment.ps1') @('New-ReleaseCheckForegroundProbeChild')
        $compiledRoot=Join-Path $testRoot 'compiled-probe'
        New-Item -ItemType Directory -Path $compiledRoot | Out-Null
        $compiledExe=Join-Path $compiledRoot 'probe-child.exe'
        $null=New-ReleaseCheckForegroundProbeChild $compiledExe
        $compiledBytes=[IO.File]::ReadAllBytes($compiledExe)
        $peOffset=[BitConverter]::ToInt32($compiledBytes,0x3c)
        # PE32 and PE32+ both store Subsystem at optional-header offset 68.
        Assert-True ([BitConverter]::ToUInt16($compiledBytes,$peOffset+24+68) -eq 2) 'Compiled probe has a console PE subsystem.'
        & {
            function Add-Type {param($TypeDefinition,$ReferencedAssemblies,$OutputType,$OutputAssembly,$ErrorAction) throw 'fake compiler unavailable'}
            $message=''
            try{New-ReleaseCheckForegroundProbeChild 'unused.exe'}catch{$message=$_.Exception.Message}
            Assert-True ($message -eq 'Foreground probe child compilation failed: fake compiler unavailable') 'Compiler failure lost its environment error reason.'
        }
        $focusLoops=@($probe.FindAll({param($n)$n -is [Management.Automation.Language.WhileStatementAst] -and
            $n.Body.Extent.Text.Contains('::SetForegroundWindow($childHwnd)')},$true))
        Assert-True ($focusLoops.Count -eq 1 -and $focusLoops[0].Condition.Extent.Text.Contains('$clock.Elapsed.TotalSeconds -le 20') -and
            $focusLoops[0].Body.Extent.Text.Contains('Start-Sleep -Milliseconds 100') -and
            $focusLoops[0].Body.Extent.Text.Contains('Get-ReleaseCheckWindowSample $child.Id')) 'Child foreground acquisition is not a 20-second/100ms exact-window loop.'
        $windowLoops=@($probe.FindAll({param($n)$n -is [Management.Automation.Language.WhileStatementAst] -and
            $n.Condition.Extent.Text.Contains('$windowPath')},$true))
        Assert-True ($windowLoops.Count -eq 1 -and $windowLoops[0].Condition.Extent.Text.Contains('$clock.Elapsed.TotalSeconds -lt 20')) 'HWND publication does not use the foreground deadline.'
        $clockStarts=@($probe.FindAll({param($n)$n -is [Management.Automation.Language.AssignmentStatementAst] -and
            $n.Left.Extent.Text -eq '$clock'},$true))
        Assert-True ($clockStarts.Count -eq 1 -and $clockStarts[0].Extent.StartOffset -gt $launches[0].Extent.EndOffset -and
            $clockStarts[0].Extent.EndOffset -lt $windowLoops[0].Extent.StartOffset) 'Foreground clock does not start directly after child launch.'
        $earlyResets=@($probe.FindAll({param($n)$n -is [Management.Automation.Language.InvokeMemberExpressionAst] -and
            $n.Expression.Extent.Text -eq '$clock' -and $n.Member.Extent.Text -in @('Restart','Reset','Start') -and
            $n.Extent.StartOffset -gt $clockStarts[0].Extent.StartOffset -and $n.Extent.StartOffset -lt $focusLoops[0].Extent.EndOffset},$true))
        Assert-True ($earlyResets.Count -eq 0) 'HWND publication resets the launch-time foreground deadline.'
        $elapsed=@($probe.FindAll({param($n)$n -is [Management.Automation.Language.AssignmentStatementAst] -and
            $n.Left.Extent.Text -eq '$report.child_foreground_elapsed_ms'},$true))
        Assert-True ($elapsed.Count -eq 1) 'Foreground elapsed time is not recorded.'
        & {
            # Drive the actual focus loop using fake native calls, time and samples.
            # First reject a same-PID wrong HWND, then accept the exact Form.
            $child=[pscustomobject]@{Id=202;HasExited=$false}
            $child | Add-Member -MemberType ScriptMethod -Name Refresh -Value {}
            $childHwnd=[IntPtr]999
            $clock=[pscustomobject]@{}
            $clock | Add-Member -MemberType ScriptProperty -Name Elapsed -Value {[TimeSpan]::FromMilliseconds($script:probeFakeMs)}
            function Start-Sleep {param($Milliseconds) Assert-True ($Milliseconds -eq 100) 'Foreground polling cadence changed.';$script:probeFakeMs+=$script:probeFakeSleepStep;$script:probeFakeSleeps++}
            function Get-ReleaseCheckWindowSample {
                param($AppPid)
                $script:probeFakeSamples++
                [pscustomobject]@{foreground_pid=202;foreground_hwnd=$(if(-not $script:probeFakeNeverExact -and $script:probeFakeSamples -ge 2){999}else{998})}
            }
            $script:probeFakeMs=0;$script:probeFakeSleepStep=100;$script:probeFakeSleeps=0;$script:probeFakeSamples=0;$script:probeFakeNeverExact=$false
            $report=@{child_foreground=$false;child_foreground_attempts=0}
            & ([scriptblock]::Create($focusLoops[0].Extent.Text))
            & ([scriptblock]::Create($elapsed[0].Extent.Text))
            Assert-True ($report.child_foreground -and $report.child_foreground_attempts -eq 2 -and $script:probeFakeSleeps -eq 1 -and
                $report.child_foreground_elapsed_ms -eq 100) 'Foreground loop did not retry until exact PID/HWND or record attempts/time.'
            $script:probeFakeMs=0;$script:probeFakeSleepStep=11000;$script:probeFakeSleeps=0;$script:probeFakeSamples=0;$script:probeFakeNeverExact=$true
            $report=@{child_foreground=$false;child_foreground_attempts=0}
            & ([scriptblock]::Create($focusLoops[0].Extent.Text))
            & ([scriptblock]::Create($elapsed[0].Extent.Text))
            Assert-True (-not $report.child_foreground -and $report.child_foreground_attempts -eq 2 -and
                $report.child_foreground_elapsed_ms -eq 22000) 'Foreground loop ignored its deadline or accepted another HWND.'
            # Spend 18 seconds waiting for .window, then use only the remaining
            # launch-time budget for foreground. No native/window work runs.
            $windowPath='fake-window-ready'
            function Test-Path {param($LiteralPath,$PathType) Assert-True ($LiteralPath -eq $windowPath) 'Unexpected HWND publication path.';return $script:probeFakeMs -ge 18000}
            $script:probeFakeMs=0;$script:probeFakeSleepStep=6000;$script:probeFakeSleeps=0;$script:probeFakeSamples=0
            & ([scriptblock]::Create($windowLoops[0].Extent.Text))
            Assert-True ($script:probeFakeMs -eq 18000 -and $script:probeFakeSleeps -eq 3) 'HWND wait fixture did not consume 18 seconds.'
            $script:probeFakeSleepStep=1000
            $report=@{child_foreground=$false;child_foreground_attempts=0}
            & ([scriptblock]::Create($focusLoops[0].Extent.Text))
            & ([scriptblock]::Create($elapsed[0].Extent.Text))
            Assert-True (-not $report.child_foreground -and $report.child_foreground_attempts -eq 3 -and
                $report.child_foreground_elapsed_ms -eq 21000) 'HWND waiting granted another 20 seconds instead of sharing the foreground deadline.'
        }
        & {
            # Execute only the real cleanup block with fake handles/processes.
            # No native call, form, foreground change or process kill occurs.
            $cleanup=@($probe.EndBlock.Statements | Where-Object {$_ -is [Management.Automation.Language.TryStatementAst]})[-1].Finally
            $child=[pscustomobject]@{Id=202;HasExited=$false}
            $child | Add-Member -MemberType ScriptMethod -Name Refresh -Value {}
            $child | Add-Member -MemberType ScriptMethod -Name WaitForExit -Value {param($Timeout) $true}
            $child | Add-Member -MemberType ScriptMethod -Name Dispose -Value {}
            $form=$null;$childHwnd=[IntPtr]999
            [MivReleaseCheckNative]::BackgroundOwner=202
            [MivReleaseCheckNative]::PostedCloseHwnd=0
            function Stop-Process {throw 'Successful Form cleanup killed a process.'}
            & ([scriptblock]::Create($cleanup.Extent.Text.TrimStart('{').TrimEnd('}')))
            Assert-True ([MivReleaseCheckNative]::PostedCloseHwnd -eq 999) 'Probe cleanup did not close the exact child Form.'
            $child.Id=203
            [MivReleaseCheckNative]::PostedCloseHwnd=0
            & ([scriptblock]::Create($cleanup.Extent.Text.TrimStart('{').TrimEnd('}')))
            Assert-True ([MivReleaseCheckNative]::PostedCloseHwnd -eq 0) 'Probe cleanup sent WM_CLOSE to another owner.'
        }
        Assert-Throws {Assert-ReleaseCheckArtifact 'unused' -ExpectedSourceFingerprint ''} 'Empty fingerprint accepted.'
        foreach($relative in @('relcheck-kit\KitSupport.ps1','relcheck-kit\New-ReleaseCheckKit.ps1','relcheck-kit\Receive-ReleaseCheckResult.ps1','relcheck-kit\Test-SubPcEnvironment.ps1','prepare-portable-smoke.ps1','ui-smoke.ps1','ui-smoke\release-checks\Run.ps1','ui-smoke\release-checks\Support.ps1','ui-smoke\release-checks\Sampler.ps1','check-idle-health.ps1','test-release-check-safety.ps1')){
            $path=Join-Path $PSScriptRoot $relative
            $null=Get-TestAst $path
            Assert-True (@([IO.File]::ReadAllBytes($path) | Where-Object {$_ -gt 127}).Count -eq 0) "Non-ASCII PowerShell: $relative"
        }
    }finally{
        # Remove only our resolved GUID tree; never recurse through reparse points.
        $full=[IO.Path]::GetFullPath($testRoot)
        $prefix=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')+'\'
        Assert-True ($full.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase) -and (Split-Path -Leaf $full) -match '^miv-kit-safety-[0-9a-f]{32}$') 'Unsafe test cleanup path.'
        if(Test-Path -LiteralPath $full){
            Import-ReleaseCheckFunctions (Join-Path $PSScriptRoot 'prepare-portable-smoke.ps1') @('Assert-NoReparseTree')
            Assert-NoReparseTree $full 'kit-test-cleanup'
            Remove-Item -LiteralPath $full -Recurse -Force
        }
    }
}
Write-Host 'release-check safety: PASS (approval, kit hashes, argument guards, returned results, exact PID/HWND, lifetime and clock routing; no application or desktop interaction)'
