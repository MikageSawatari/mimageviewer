# Disposable foreground-grant peer. No keyboard or mouse injection.
[CmdletBinding()]
param([Parameter(Mandatory=$true)][int]$RunnerPid,[Parameter(Mandatory=$true)][string]$ReadyPath)
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot '..\ui-smoke\release-checks\Support.ps1')
Initialize-ReleaseCheckNative
Add-Type -AssemblyName System.Windows.Forms
$form=New-Object Windows.Forms.Form
$form.Text='mIV relcheck foreground probe';$form.Width=480;$form.Height=160
$timer=New-Object Windows.Forms.Timer
$timer.Interval=100
$clock=[Diagnostics.Stopwatch]::StartNew()
$script:published=$false
$timer.Add_Tick({
    if($clock.Elapsed.TotalSeconds -ge 30){$form.Close();return}
    if(-not $script:published -and [MivReleaseCheckNative]::GetForegroundWindow() -eq $form.Handle){
        $grant=[MivReleaseCheckNative]::AllowSetForegroundWindow([uint32]$RunnerPid)
        Write-ReleaseCheckJson ($ReadyPath+'.pending') @{pid=$PID;root_hwnd=$form.Handle.ToInt64();background_foreground_grant=@{granted=$grant;app_pid=$PID;root_hwnd=$form.Handle.ToInt64();runner_pid=$RunnerPid}}
        [IO.File]::Move(($ReadyPath+'.pending'),$ReadyPath)
        $script:published=$true
    }
})
try{$timer.Start();[Windows.Forms.Application]::Run($form)}
finally{$timer.Stop();$timer.Dispose();$form.Dispose()}
