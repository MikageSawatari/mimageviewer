# Function-only support. Dot-sourcing this file has no filesystem/process/UI effects.
function Test-ReleaseCheckContinueAfterAnalysis {
    param([string]$CaseId,[int]$ExitCode)
    # Called only after normal process completion. Collect the independent
    # uncached performance evidence even if cached has a real performance FAIL.
    # Its recorded failure still determines the suite verdict.
    return $ExitCode -eq 0 -or ($CaseId -eq 'perf-cached' -and $ExitCode -eq 1)
}

function Invoke-ReleaseCheckPreflight {
    param([scriptblock]$Action)
    try { return [pscustomobject]@{exit_code=0;value=(& $Action);environment_error=$null} }
    catch { return [pscustomobject]@{exit_code=2;value=$null;environment_error=$_.Exception.Message} }
}

function New-ReleaseCheckProcessExitResult {
    param([string]$CaseId,[int]$ProcessExitCode,[int]$ProcessId,[string]$Directory)
    if($ProcessExitCode -eq 0){return $null}
    # The runtime assigns 1 to ScriptFailure and 2 to EnvironmentFailure.
    # Unknown nonzero exits are also failures; retain the original process code.
    $environment=$ProcessExitCode -eq 2
    $message="$CaseId application exit code $ProcessExitCode"
    $failureMessages=@();$environmentMessages=@()
    if($environment){$environmentMessages+=@($message)}else{$failureMessages+=@($message)}
    return [ordered]@{
        schema_version=1;case_id=$CaseId;verdict=$(if($environment){'SKIPPED'}else{'FAIL'})
        exit_code=$(if($environment){2}else{1})
        failures=$failureMessages;environment_errors=$environmentMessages;warnings=@()
        evidence=@{process=@{pid=$ProcessId;process_exited=$true;process_exit_code=$ProcessExitCode}
            failure_artifacts=@{directory=$Directory;perf_log=(Join-Path $Directory 'perf_events.jsonl')
                app_log=(Join-Path $Directory 'mimageviewer.log');process=(Join-Path $Directory 'process.json')
                case_evidence=(Join-Path $Directory 'case-evidence.json')}}
    }
}

function Import-ReleaseCheckFunctions {
    param([string]$Path, [string[]]$Names)
    $tokens = $null; $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errors)
    if ($errors.Count) { throw "Cannot parse support source: $Path" }
    foreach ($name in $Names) {
        $nodes = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name }, $true))
        if ($nodes.Count -ne 1) { throw "Missing/ambiguous support function: $name" }
        Set-Item -Path "Function:script:$name" -Value ([scriptblock]::Create($nodes[0].Body.Extent.Text.TrimStart('{').TrimEnd('}')))
    }
}

function Assert-ReleaseCheckApproval {
    param([bool]$Approved)
    if (-not $Approved) { throw 'ReleaseChecks requires explicit approval before preparation, filesystem changes, launch or foreground use.' }
}

function Assert-ReleaseCheckPaths {
    param([string]$RepoRoot, [string]$Exe, [string]$Data, [string]$Output)
    $null = Assert-ExactPath $Exe (Join-Path $RepoRoot 'target\portable-smoke\mimageviewer.exe') 'release-exe'
    $null = Assert-ExactPath $Data (Join-Path $RepoRoot 'target\portable-smoke\data') 'release-data'
    $runRoot = Get-NormalizedPath (Join-Path $RepoRoot 'target\ui-smoke\runs')
    if (-not (Get-NormalizedPath $Output).StartsWith($runRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'ReleaseChecks output must be a strict descendant of target/ui-smoke/runs.'
    }
    foreach ($path in @($Exe, $Data, $Output)) { Assert-NoReparsePath $path $RepoRoot 'release-path' }
    Assert-NoReparseTree $Data 'release-data'
    Assert-NoReparseTree $Output 'release-output'
}

function Assert-ReleaseCheckArtifact {
    param([string]$RepoRoot,[string]$ExpectedSourceFingerprint)
    if([string]::IsNullOrWhiteSpace($ExpectedSourceFingerprint)){throw 'Expected source fingerprint is required.'}
    $smoke = Join-Path $RepoRoot 'target\portable-smoke'
    Assert-NoReparsePath $smoke $RepoRoot 'release-artifact'
    Assert-NoReparseTree $smoke 'release-artifact'
    $marker = Join-Path $smoke 'data\.disposable-smoke-data'
    if ((Get-Content -LiteralPath $marker -Encoding ASCII -Raw).Trim() -ne 'mimageviewer-disposable-smoke-v1;test-script=true') { throw 'Disposable test-script marker mismatch.' }
    Assert-SmokeBuildManifest (Join-Path $smoke '.test-script-build.json') $ExpectedSourceFingerprint `
        (Join-Path $smoke 'mimageviewer.exe') (Join-Path $smoke 'mimageviewer-remote.exe') (Join-Path $smoke 'mimageviewer-epub-pdf.exe') `
        (Join-Path $smoke 'mimageviewer-mcp.exe') (Join-Path $smoke 'mimageviewer-musicvideo.exe')
}

function Assert-ReleaseCheckIdentity {
    param($Process, $Cim, [int]$ExpectedPid, [datetime]$ExpectedStartUtc, [string]$Exe, [string]$Data)
    if ($Process.HasExited -or $Process.Id -ne $ExpectedPid -or $Cim.ProcessId -ne $ExpectedPid -or
        $Process.StartTime.ToUniversalTime().Ticks -ne $ExpectedStartUtc.ToUniversalTime().Ticks) { throw 'Exact PID/start-time identity mismatch.' }
    $null = Assert-ExactPath ([string]$Process.Path) $Exe 'release-process'
    $null = Assert-ExactPath ([string]$Cim.ExecutablePath) $Exe 'release-command-exe'
    # Tokenize the native command line; substring matching is not identity evidence.
    $arguments = [MivReleaseCheckNative]::Arguments([string]$Cim.CommandLine)
    $dataValues = @()
    for ($i = 0; $i -lt $arguments.Length; $i++) {
        if ($arguments[$i] -eq '--data-dir' -and $i + 1 -lt $arguments.Length) { $dataValues += $arguments[$i + 1] }
    }
    if ($dataValues.Count -ne 1 -or $arguments -notcontains '--perf-log' -or $arguments -notcontains '--test-script' -or $arguments -notcontains '--settings-override') { throw 'Missing exact diagnostic command line evidence.' }
    $null = Assert-ExactPath $dataValues[0] $Data 'release-command-data'
}

function Write-ReleaseCheckJson {
    param([string]$Path, $Value)
    [IO.File]::WriteAllText($Path, ($Value | ConvertTo-Json -Depth 20), (New-Object Text.UTF8Encoding($false)))
}

function Initialize-ReleaseCheckNative {
    if ('MivReleaseCheckNative' -as [type]) { return }
    Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class MivReleaseCheckNative {
 [DllImport("shell32.dll", CharSet=CharSet.Unicode)] static extern IntPtr CommandLineToArgvW(string s, out int n);
 [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);
 public static string[] Arguments(string s) { int n; var p=CommandLineToArgvW(s,out n); if(p==IntPtr.Zero)throw new Exception("Command line unavailable"); try {var a=new string[n];for(int i=0;i<n;i++)a[i]=Marshal.PtrToStringUni(Marshal.ReadIntPtr(p,i*IntPtr.Size));return a;} finally{LocalFree(p);} }
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr w,out uint p);
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr w);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern bool PostMessage(IntPtr w,uint message,IntPtr wParam,IntPtr lParam);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr w);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr w);
 [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
 [DllImport("user32.dll")] public static extern uint GetDpiForSystem();
 [DllImport("user32.dll")] public static extern bool AllowSetForegroundWindow(uint pid);
 public delegate bool EnumProc(IntPtr w,IntPtr p);
 [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc c,IntPtr p);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr w,StringBuilder b,int n);
 [StructLayout(LayoutKind.Sequential)] public struct Rect { public int L,T,R,B; }
 [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr w,out Rect r);
 [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr w,out Rect r);
 [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern bool SystemParametersInfo(uint action,uint param,out Rect r,uint flags);
 [DllImport("user32.dll")] static extern bool AdjustWindowRectExForDpi(ref Rect r,uint style,bool menu,uint exStyle,uint dpi);
 // Query physical pixels without inheriting the PowerShell host's DPI virtualization.
 // The system DPI selects the fixture; GetDpiForWindow later proves the actual DPI.
 public static int[] DesktopGeometry(){
   var previous=SetThreadDpiAwarenessContext(new IntPtr(-4));
   if(previous==IntPtr.Zero)throw new Exception("Per-monitor DPI context unavailable");
   try{Rect r;if(!SystemParametersInfo(0x30,0,out r,0))throw new Exception("Primary work area unavailable");return new[]{(int)GetDpiForSystem(),r.L,r.T,r.R,r.B};}
   finally{SetThreadDpiAwarenessContext(previous);}
 }
 public static int[] OuterSize(int width,int height,uint dpi){
   // Winit 0.30's normal decorated, resizable root: WS_OVERLAPPEDWINDOW |
   // WS_CLIPSIBLINGS; WS_EX_WINDOWEDGE | WS_EX_ACCEPTFILES | WS_EX_APPWINDOW.
   // Visibility/clipping flags do not add nonclient space. There is no menu.
   // --window-size is logical client size, not decorated physical window size.
   var r=new Rect{R=(int)Math.Round(width*dpi/96.0),B=(int)Math.Round(height*dpi/96.0)};
   if(!AdjustWindowRectExForDpi(ref r,0x04CF0000,false,0x00040110,dpi))throw new Exception("Decorated window size unavailable");
   return new[]{r.R-r.L,r.B-r.T};
 }
 public static int[] WindowGeometry(IntPtr hwnd){
   var previous=SetThreadDpiAwarenessContext(new IntPtr(-4));
   if(previous==IntPtr.Zero)throw new Exception("Per-monitor DPI context unavailable");
   try{Rect c,r;if(!GetClientRect(hwnd,out c)||!GetWindowRect(hwnd,out r))throw new Exception("Root window geometry unavailable");return new[]{(int)GetDpiForWindow(hwnd),c.R-c.L,c.B-c.T,r.L,r.T,r.R,r.B};}
   finally{SetThreadDpiAwarenessContext(previous);}
 }
 public static int[] Counts(uint pid){int total=0,visible=0;EnumWindows((w,p)=>{uint id;GetWindowThreadProcessId(w,out id);if(id==pid){total++;if(IsWindowVisible(w)){var b=new StringBuilder(256);GetClassName(w,b,256);Rect r;var c=b.ToString();if(c!="IME"&&c!="MSCTFIME UI"&&c!="Winit Thread Event Target"&&GetWindowRect(w,out r)&&r.R-r.L>=64&&r.B-r.T>=64)visible++;}}return true;},IntPtr.Zero);return new[]{total,visible};}
 [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
 [DllImport("user32.dll")] public static extern IntPtr GetThreadDesktop(uint t);
 [DllImport("user32.dll")] public static extern IntPtr OpenInputDesktop(uint f,bool i,uint a);
 [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr p);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern bool GetUserObjectInformation(IntPtr p,int i,StringBuilder b,int n,out int needed);
 static string DesktopName(IntPtr p){int n;var b=new StringBuilder(256);if(p==IntPtr.Zero||!GetUserObjectInformation(p,2,b,512,out n))throw new Exception("Desktop query failed");return b.ToString();}
 public static void CheckDesktop(){var p=OpenInputDesktop(0,false,1);try{if(DesktopName(p)!=DesktopName(GetThreadDesktop(GetCurrentThreadId()))||GetForegroundWindow()==IntPtr.Zero)throw new Exception("Input desktop unavailable/mismatched");}finally{if(p!=IntPtr.Zero)CloseDesktop(p);}}
}
'@
}

function Get-ReleaseCheckWindowSample {
    param([int]$AppPid,[long]$RootHwnd=0)
    $h = [MivReleaseCheckNative]::GetForegroundWindow(); [uint32]$owner = 0
    $null = [MivReleaseCheckNative]::GetWindowThreadProcessId($h, [ref]$owner)
    $counts = [MivReleaseCheckNative]::Counts([uint32]$AppPid)
    $sample=[pscustomobject]@{ unix_ms = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds(); foreground_pid = $owner; foreground_hwnd = $h.ToInt64(); total = $counts[0]; visible = $counts[1] }
    if($RootHwnd -ne 0){$sample | Add-Member -MemberType NoteProperty -Name window_geometry -Value (Get-ReleaseCheckWindowGeometry $AppPid $RootHwnd)}
    return $sample
}

function Get-ReleaseCheckCaseGeometry {
    param([string]$CaseId,[int]$Dpi)
    $table=Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\..\release_check_geometry.json') -Encoding UTF8 -Raw | ConvertFrom-Json
    if($table.schema_version -ne 1){throw 'Unknown release-check geometry schema.'}
    $rows=@($table.static_rows | Where-Object {$_.dpi -eq $Dpi})
    if($rows.Count -ne 1){throw "Unsupported ReleaseChecks DPI: $Dpi."}
    $row=if($CaseId -in @('static-foreground','static-background','seed-profile')){$rows[0]}else{$table.default}
    $columns=if($CaseId -like 'tray-*'){10}else{[int]$row.grid_cols}
    return [pscustomobject]@{dpi=$Dpi;window_size=@($row.window_size);grid_cols=$columns}
}

function Assert-ReleaseCheckGeometryEnvironment {
    param($ForegroundGeometry,$BackgroundGeometry,[int[]]$WorkArea,[int[]]$OuterSize)
    $expected=Get-ReleaseCheckCaseGeometry 'static-foreground' $ForegroundGeometry.dpi
    foreach($row in @($ForegroundGeometry,$BackgroundGeometry)){
        if($row.dpi -ne $expected.dpi -or $row.grid_cols -ne $expected.grid_cols -or
            ($row.window_size -join ',') -ne ($expected.window_size -join ',')){
            throw 'Static foreground/background geometry must match the selected DPI table row.'
        }
    }
    if($WorkArea.Count -ne 4 -or $OuterSize.Count -ne 2 -or $OuterSize[0] -le 0 -or $OuterSize[1] -le 0 -or
        $OuterSize[0] -gt ($WorkArea[2]-$WorkArea[0]) -or $OuterSize[1] -gt ($WorkArea[3]-$WorkArea[1])){
        throw "Selected static decorated window ($($OuterSize -join 'x') physical px) does not fit the primary work area ($($WorkArea -join ','))."
    }
}

function Get-ReleaseCheckGeometryPreflight {
    $desktop=[MivReleaseCheckNative]::DesktopGeometry()
    $foreground=Get-ReleaseCheckCaseGeometry 'static-foreground' $desktop[0]
    $background=Get-ReleaseCheckCaseGeometry 'static-background' $desktop[0]
    $workArea=@($desktop[1..4])
    $outer=[MivReleaseCheckNative]::OuterSize($foreground.window_size[0],$foreground.window_size[1],$desktop[0])
    Assert-ReleaseCheckGeometryEnvironment $foreground $background $workArea $outer
    return [pscustomobject]@{static_geometry=$foreground;work_area=$workArea;outer_size=@($outer)}
}

function Get-ReleaseCheckWindowGeometry {
    param([int]$AppPid,[long]$RootHwnd)
    [uint32]$owner=0
    $null=[MivReleaseCheckNative]::GetWindowThreadProcessId([IntPtr]$RootHwnd,[ref]$owner)
    if($RootHwnd -le 0 -or $owner -ne $AppPid){throw 'Geometry root HWND owner mismatch.'}
    $native=[MivReleaseCheckNative]::WindowGeometry([IntPtr]$RootHwnd)
    return [pscustomobject]@{pid=$AppPid;root_hwnd=$RootHwnd;dpi=$native[0];client_size=@($native[1..2]);window_rect=@($native[3..6])}
}

function Assert-ReleaseCheckWindowGeometry {
    param($Actual,$Expected)
    $physical=@($Expected.window_size | ForEach-Object {[int][Math]::Round($_*$Expected.dpi/96.0)})
    if($Actual.dpi -ne $Expected.dpi){throw 'Actual window DPI differs from the fixture DPI.'}
    if(($Actual.client_size -join ',') -ne ($physical -join ',')){
        throw 'Actual window client size differs from the selected case geometry.'
    }
}

function Assert-ReleaseCheckBackgroundTransitionForeground {
    param([int]$AppPid, [long]$RootHwnd, [int]$RunnerPid, [long]$BackgroundHwnd, [string]$Operation)
    # A saved grant can survive a foreground change. Check the current exact
    # PID/HWND pair at each side-effect boundary, without enumerating windows.
    $foreground=[MivReleaseCheckNative]::GetForegroundWindow(); [uint32]$owner=0
    $null=[MivReleaseCheckNative]::GetWindowThreadProcessId($foreground,[ref]$owner)
    $hwnd=$foreground.ToInt64()
    if (-not (($owner -eq $AppPid -and $hwnd -eq $RootHwnd) -or
              ($owner -eq $RunnerPid -and $hwnd -eq $BackgroundHwnd))) {
        throw "Foreground changed before background ${Operation}: expected root PID=$AppPid HWND=$RootHwnd or runner PID=$RunnerPid HWND=$BackgroundHwnd, observed PID=$owner HWND=$hwnd."
    }
}

function Set-ReleaseCheckBackgroundForeground {
    param($Ready, [int]$AppPid, $Window, [int]$RunnerPid)
    # The foreground app grants ONLY this runner before publishing ready.json.
    # Showing/activating a form from a nonforeground process is not sufficient.
    $grant=$Ready.background_foreground_grant
    if ($Ready.pid -ne $AppPid -or $grant.granted -ne $true -or
        $grant.app_pid -ne $AppPid -or $grant.runner_pid -ne $RunnerPid -or
        $grant.root_hwnd -ne $Ready.root_hwnd -or $Ready.root_hwnd -le 0 -or
        $RunnerPid -le 0 -or $RunnerPid -eq $AppPid) { throw 'Exact runner foreground grant missing or mismatched.' }
    $hwnd=$Window.Handle.ToInt64(); [uint32]$owner=0
    $null=[MivReleaseCheckNative]::GetWindowThreadProcessId([IntPtr]$hwnd,[ref]$owner)
    if ($hwnd -le 0 -or $hwnd -eq $Ready.root_hwnd -or $owner -ne $RunnerPid) { throw 'Background HWND is not owned by this runner.' }
    Assert-ReleaseCheckBackgroundTransitionForeground $AppPid $Ready.root_hwnd $RunnerPid $hwnd 'Show'
    $Window.Show()
    Assert-ReleaseCheckBackgroundTransitionForeground $AppPid $Ready.root_hwnd $RunnerPid $hwnd 'activation'
    if (-not [MivReleaseCheckNative]::SetForegroundWindow([IntPtr]$hwnd)) { throw 'Runner background foreground activation was refused.' }
    $sample=Get-ReleaseCheckWindowSample $AppPid
    Assert-ReleaseCheckWindowSamples @($sample) 'static-background' $AppPid $Ready.root_hwnd $hwnd $RunnerPid
    return $sample
}

function Assert-ReleaseCheckWindowSamples {
    param($Samples, [string]$Scenario, [int]$AppPid, [long]$RootHwnd, [long]$BackgroundHwnd, [int]$BackgroundProcessId=0)
    foreach ($s in $Samples) {
        if ($Scenario -eq 'tray-residency') {
            if ($s.total -lt 1 -or $s.visible -ne 0) { throw 'Tray visibility/lifetime condition was not maintained.' }
        } elseif ($Scenario -eq 'static-foreground') {
            if ($s.foreground_pid -ne $AppPid -or $s.foreground_hwnd -ne $RootHwnd) { throw 'Exact root foreground condition was not maintained.' }
        } else {
            if ($BackgroundHwnd -eq 0 -or $BackgroundProcessId -le 0 -or
                $s.foreground_hwnd -ne $BackgroundHwnd -or $s.foreground_pid -ne $BackgroundProcessId -or
                $s.foreground_pid -eq $AppPid) { throw "Runner-owned background window condition was not maintained: expected PID=$BackgroundProcessId HWND=$BackgroundHwnd, observed PID=$($s.foreground_pid) HWND=$($s.foreground_hwnd)." }
        }
    }
}
