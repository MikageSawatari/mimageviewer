# Interactive environment probe: two disposable forms, no mIV or input injection.
[CmdletBinding()]
param([string]$OutputPath)
$ErrorActionPreference='Stop'
Set-StrictMode -Version 2
$kitRoot=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
if(-not $OutputPath){$OutputPath=Join-Path $kitRoot 'target\ui-smoke\env-probe.json'}
$OutputPath=[IO.Path]::GetFullPath($OutputPath)
. (Join-Path $PSScriptRoot 'KitSupport.ps1')
. (Join-Path $PSScriptRoot '..\ui-smoke\release-checks\Support.ps1')
Import-ReleaseCheckFunctions (Join-Path $kitRoot 'scripts\ui-smoke.ps1') @('Join-NativeArguments','Quote-NativeArgument')
$reasons=New-Object 'Collections.Generic.List[string]'
$report=[ordered]@{created_utc=[DateTime]::UtcNow.ToString('o');status='NOT_READY';reasons=@();kit_valid=$false;python_valid=$false;ffmpeg_valid=$false;desktop_valid=$false;environment=@{};child_foreground=$false;runner_foreground_after_grant=$false}
$child=$null;$form=$null
function Get-ProbeForeground {
    $hwnd=[MivReleaseCheckNative]::GetForegroundWindow();[uint32]$owner=0
    $null=[MivReleaseCheckNative]::GetWindowThreadProcessId($hwnd,[ref]$owner)
    $class=New-Object Text.StringBuilder 256
    $null=[MivReleaseCheckNative]::GetClassName($hwnd,$class,$class.Capacity)
    return @{pid=$owner;hwnd=$hwnd.ToInt64();window_class=$class.ToString()}
}
try{
    $null=Assert-ReleaseCheckKit $kitRoot
    $report.kit_valid=$true
    try{
        $python=(Get-Command python -ErrorAction Stop).Source
        $version=@(& $python -c 'import sys; print(sys.version); sys.exit(0 if sys.version_info >= (3, 10) else 2)')
        $report.python_version=$version -join "`n"
        if($LASTEXITCODE -ne 0){throw 'Python 3.10 or newer is required.'}
        $report.python_valid=$true
    }catch{$reasons.Add($_.Exception.Message)}
    try{
        $ffmpeg=@(& (Join-Path $kitRoot 'tools\ffmpeg\ffmpeg.exe') -version)
        if($LASTEXITCODE -ne 0 -or -not $ffmpeg.Count){throw 'Kit ffmpeg -version failed.'}
        $report.ffmpeg_version=$ffmpeg[0]
        $report.ffmpeg_valid=$true
    }catch{$reasons.Add($_.Exception.Message)}
    Initialize-ReleaseCheckNative
    try{[MivReleaseCheckNative]::CheckDesktop();$report.desktop_valid=$true}
    catch{$reasons.Add($_.Exception.Message)}
    Add-Type -AssemblyName System.Windows.Forms
    $screen=[Windows.Forms.Screen]::PrimaryScreen.Bounds
    $report.environment=@{
        remote_session=([MivReleaseCheckNative]::GetSystemMetrics(0x1000) -ne 0);session_name=$env:SESSIONNAME
        primary_screen=('{0}x{1}' -f $screen.Width,$screen.Height);system_dpi=[MivReleaseCheckNative]::GetDpiForSystem()
        gpu_names=@(Get-CimInstance Win32_VideoController | ForEach-Object {$_.Name})
        cpu_names=@(Get-CimInstance Win32_Processor | ForEach-Object {$_.Name})
        os=(Get-CimInstance Win32_OperatingSystem).Caption
    }
    if($report.desktop_valid){
    # Ready evidence is runtime output, excluded from immutable kit records.
    $probeRoot=Join-Path $kitRoot ('target\ui-smoke\env-probe-'+[Guid]::NewGuid().ToString('N'))
    Assert-ReleaseCheckKitNoReparsePath $probeRoot
    New-Item -ItemType Directory -Path $probeRoot -Force | Out-Null
    $readyPath=Join-Path $probeRoot 'child-ready.json'
    $arguments=@('-NoProfile','-STA','-ExecutionPolicy','Bypass','-File',(Join-Path $PSScriptRoot 'ForegroundProbeChild.ps1'),'-RunnerPid',[string]$PID,'-ReadyPath',$readyPath)
    $child=Start-Process -FilePath (Get-Command powershell.exe -ErrorAction Stop).Source -ArgumentList (Join-NativeArguments $arguments) -WindowStyle Hidden -PassThru
    $null=$child.Handle
    $clock=[Diagnostics.Stopwatch]::StartNew()
    while(-not $child.HasExited -and $child.MainWindowHandle -eq [IntPtr]::Zero -and $clock.Elapsed.TotalSeconds -lt 20){
        Start-Sleep -Milliseconds 100;$child.Refresh()
    }
    if($child.HasExited -or $child.MainWindowHandle -eq [IntPtr]::Zero){throw 'Child window unavailable within 20 seconds.'}
    $childHwnd=$child.MainWindowHandle
    $report.child_pid=$child.Id;$report.child_hwnd=$childHwnd.ToInt64()
    $report.child_set_foreground_return=[MivReleaseCheckNative]::SetForegroundWindow($childHwnd)
    $observed=Get-ProbeForeground
    $report.child_foreground_observed=$observed
    $report.child_foreground=($observed.pid -eq $child.Id -and $observed.hwnd -eq $childHwnd.ToInt64())
    if(-not $report.child_foreground){$reasons.Add('Exact child PID/HWND did not become foreground.')}
    $clock.Restart()
    while(-not (Test-Path -LiteralPath $readyPath -PathType Leaf) -and -not $child.HasExited -and $clock.Elapsed.TotalSeconds -lt 20){
        Start-Sleep -Milliseconds 100;$child.Refresh()
    }
    if(-not (Test-Path -LiteralPath $readyPath -PathType Leaf)){throw 'Child foreground grant evidence unavailable within 20 seconds.'}
    $ready=Get-Content -LiteralPath $readyPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $report.background_foreground_grant=$ready.background_foreground_grant
    if($ready.pid -ne $child.Id -or $ready.root_hwnd -ne $childHwnd.ToInt64() -or
        $ready.background_foreground_grant.app_pid -ne $child.Id -or
        $ready.background_foreground_grant.root_hwnd -ne $childHwnd.ToInt64() -or
        $ready.background_foreground_grant.runner_pid -ne $PID -or -not $ready.background_foreground_grant.granted){throw 'Invalid child foreground grant.'}
    $form=New-Object Windows.Forms.Form
    $form.Text='mIV relcheck runner foreground probe';$form.Width=480;$form.Height=160
    $form.Show()
    $report.runner_set_foreground_return=[MivReleaseCheckNative]::SetForegroundWindow($form.Handle)
    [Windows.Forms.Application]::DoEvents()
    $observed=Get-ProbeForeground
    $report.runner_foreground_observed=$observed
    $report.runner_foreground_after_grant=($observed.pid -eq $PID -and $observed.hwnd -eq $form.Handle.ToInt64())
    if(-not $report.runner_foreground_after_grant){$reasons.Add('Runner PID/HWND did not become foreground after child grant.')}
    }
}catch{$reasons.Add($_.Exception.Message)}
finally{
    if($form){$form.Close();$form.Dispose()}
    if($child){
        $child.Refresh()
        if(-not $child.HasExited){
            $null=$child.CloseMainWindow()
            if(-not $child.WaitForExit(5000)){Stop-Process -Id $child.Id -Force -ErrorAction Stop;$child.WaitForExit()}
        }
        $child.Dispose()
    }
}
$report.reasons=@($reasons.ToArray())
if(-not $reasons.Count -and $report.child_foreground -and $report.runner_foreground_after_grant){$report.status='READY'}
Assert-ReleaseCheckKitNoReparsePath $OutputPath
New-Item -ItemType Directory -Path (Split-Path -Parent $OutputPath) -Force | Out-Null
Write-ReleaseCheckJson $OutputPath $report
Write-Host ($report.status+': '+($report.reasons -join '; '))
Write-Host "Environment report: $OutputPath"
if($report.status -eq 'READY'){exit 0}else{exit 2}
