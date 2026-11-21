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
$report=[ordered]@{created_utc=[DateTime]::UtcNow.ToString('o');status='NOT_READY';reasons=@();kit_valid=$false;python_valid=$false;ffmpeg_valid=$false;desktop_valid=$false;environment=@{};child_foreground=$false;child_foreground_attempts=0;child_foreground_elapsed_ms=0;runner_foreground_after_grant=$false}
$child=$null;$form=$null;$childHwnd=[IntPtr]::Zero
function New-ReleaseCheckForegroundProbeChild {
    param([string]$OutputPath)
    # Compile a GUI-subsystem peer, just like the suite's mIV process. It has
    # no console/Terminal window that could compete with the Form for foreground.
    $source=@'
using System;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows.Forms;

public static class MivReleaseCheckForegroundProbeChild {
    [DllImport("user32.dll")]
    static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")]
    static extern bool AllowSetForegroundWindow(uint processId);

    static void Publish(string path, string json) {
        File.WriteAllText(path + ".pending", json, new UTF8Encoding(false));
        File.Move(path + ".pending", path);
    }

    [STAThread]
    public static int Main(string[] args) {
        int runnerPid;
        if (args.Length != 2 || !int.TryParse(args[0], out runnerPid) || runnerPid <= 0)
            return 2;
        string readyPath = args[1];
        string pid = Process.GetCurrentProcess().Id.ToString(CultureInfo.InvariantCulture);
        string runner = runnerPid.ToString(CultureInfo.InvariantCulture);
        var clock = Stopwatch.StartNew();
        // I/O errors terminate the child instead of displaying a WinForms error dialog.
        Application.SetUnhandledExceptionMode(UnhandledExceptionMode.ThrowException);
        using (var form = new Form())
        using (var timer = new Timer()) {
            form.Text = "mIV relcheck foreground probe";
            form.Width = 480;
            form.Height = 160;
            bool published = false;
            form.Shown += delegate {
                string hwnd = form.Handle.ToInt64().ToString(CultureInfo.InvariantCulture);
                Publish(readyPath + ".window", "{\"pid\":" + pid + ",\"root_hwnd\":" + hwnd + "}");
            };
            timer.Interval = 100;
            timer.Tick += delegate {
                if (clock.Elapsed.TotalSeconds >= 30) {
                    form.Close();
                    return;
                }
                if (!published && GetForegroundWindow() == form.Handle) {
                    bool granted = AllowSetForegroundWindow((uint)runnerPid);
                    string hwnd = form.Handle.ToInt64().ToString(CultureInfo.InvariantCulture);
                    Publish(readyPath, "{\"pid\":" + pid + ",\"root_hwnd\":" + hwnd +
                        ",\"background_foreground_grant\":{\"granted\":" + (granted ? "true" : "false") +
                        ",\"app_pid\":" + pid + ",\"root_hwnd\":" + hwnd + ",\"runner_pid\":" + runner + "}}");
                    published = true;
                }
            };
            timer.Start();
            Application.Run(form);
        }
        return 0;
    }
}
'@
    try{
        Add-Type -TypeDefinition $source -ReferencedAssemblies @('System.dll','System.Windows.Forms.dll','System.Drawing.dll') -OutputType WindowsApplication -OutputAssembly $OutputPath -ErrorAction Stop
    }catch{throw ('Foreground probe child compilation failed: '+$_.Exception.Message)}
}

function Get-ProbeForeground {
    $hwnd=[MivReleaseCheckNative]::GetForegroundWindow();[uint32]$owner=0
    $null=[MivReleaseCheckNative]::GetWindowThreadProcessId($hwnd,[ref]$owner)
    $class=New-Object Text.StringBuilder 256
    $null=[MivReleaseCheckNative]::GetClassName($hwnd,$class,$class.Capacity)
    return @{pid=$owner;hwnd=$hwnd.ToInt64();window_class=$class.ToString()}
}
try{
    $null=Assert-ReleaseCheckKit $kitRoot
    Assert-ReleaseCheckKitRunRoot $kitRoot
    $kit=Read-ReleaseCheckKitManifest $kitRoot
    Assert-ReleaseCheckDiagnosticBuildRoot (Join-Path $kitRoot 'target\portable-smoke-package\mimageviewer.exe') $kit.artifact.build_root
    $report.kit_valid=$true
    $env:PYTHONDONTWRITEBYTECODE='1'
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
    $probeExe=Join-Path $probeRoot 'probe-child.exe'
    $null=New-ReleaseCheckForegroundProbeChild $probeExe
    $arguments=@([string]$PID,$readyPath)
    # Match the GUI suite launch: no console, and no STARTUPINFO SW_HIDE that
    # could suppress the Form's first ShowWindow.
    $child=Start-Process -FilePath $probeExe -ArgumentList (Join-NativeArguments $arguments) -PassThru
    # One launch-time deadline covers both HWND publication and foreground.
    $clock=[Diagnostics.Stopwatch]::StartNew()
    $null=$child.Handle
    $windowPath=$readyPath+'.window'
    while(-not $child.HasExited -and -not (Test-Path -LiteralPath $windowPath -PathType Leaf) -and $clock.Elapsed.TotalSeconds -lt 20){
        Start-Sleep -Milliseconds 100;$child.Refresh()
    }
    if($child.HasExited -or -not (Test-Path -LiteralPath $windowPath -PathType Leaf)){throw 'Child Form HWND unavailable within 20 seconds.'}
    $windowReady=Get-Content -LiteralPath $windowPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $childHwnd=[IntPtr][long]$windowReady.root_hwnd
    [uint32]$windowOwner=0
    $null=[MivReleaseCheckNative]::GetWindowThreadProcessId($childHwnd,[ref]$windowOwner)
    if($windowReady.pid -ne $child.Id -or $childHwnd -eq [IntPtr]::Zero -or $windowOwner -ne $child.Id){throw 'Child Form HWND owner mismatch.'}
    $report.child_pid=$child.Id;$report.child_hwnd=$childHwnd.ToInt64()
    while(-not $child.HasExited -and -not $report.child_foreground -and $clock.Elapsed.TotalSeconds -le 20){
        $report.child_foreground_attempts++
        $report.child_set_foreground_return=[MivReleaseCheckNative]::SetForegroundWindow($childHwnd)
        $observed=Get-ReleaseCheckWindowSample $child.Id
        $report.child_foreground_observed=$observed
        $report.child_foreground=($observed.foreground_pid -eq $child.Id -and $observed.foreground_hwnd -eq $childHwnd.ToInt64())
        if($report.child_foreground){break}
        Start-Sleep -Milliseconds 100;$child.Refresh()
    }
    $report.child_foreground_elapsed_ms=$clock.Elapsed.TotalMilliseconds
    if(-not $report.child_foreground){
        $report.child_foreground_failure_observed=Get-ProbeForeground
        $reasons.Add('Exact child PID/HWND did not become foreground within 20 seconds.')
    }
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
    try{
        $observed=Set-ReleaseCheckBackgroundForeground $ready $child.Id $form $PID
        $report.runner_foreground_observed=$observed
        $report.runner_foreground_after_grant=$true
    }catch{
        $report.runner_foreground_failure_observed=Get-ProbeForeground
        throw
    }
    }
}catch{$reasons.Add($_.Exception.Message)}
finally{
    if($form){$form.Close();$form.Dispose()}
    if($child){
        $child.Refresh()
        if(-not $child.HasExited){
            # Close only the published Form, never a console/Terminal HWND.
            if($childHwnd -ne [IntPtr]::Zero){
                [uint32]$closeOwner=0
                $null=[MivReleaseCheckNative]::GetWindowThreadProcessId($childHwnd,[ref]$closeOwner)
                if($closeOwner -eq $child.Id){$null=[MivReleaseCheckNative]::PostMessage($childHwnd,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)}
            }
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
