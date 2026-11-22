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

function Assert-ReleaseCheckPublishedRootOwner {
    param([int]$AppPid,[long]$RootHwnd)
    [uint32]$owner=0
    if($RootHwnd -le 0){throw 'Published root HWND is missing.'}
    $null=[MivReleaseCheckNative]::GetWindowThreadProcessId([IntPtr]$RootHwnd,[ref]$owner)
    if($AppPid -le 0 -or $owner -ne $AppPid){throw 'Published root HWND owner PID mismatch.'}
}

function Read-ReleaseCheckRootWindow {
    param([string]$Path,[int]$AppPid)
    if(-not (Test-Path -LiteralPath $Path -PathType Leaf)){return [long]0}
    $record=Get-Content -LiteralPath $Path -Encoding UTF8 -Raw | ConvertFrom-Json
    foreach($name in @('schema_version','pid','root_hwnd','unix_ms')){
        if(($record.$name -isnot [int] -and $record.$name -isnot [long]) -or $record.$name -le 0){throw "Invalid published root field: $name"}
    }
    if($record.schema_version -ne 1 -or $record.pid -ne $AppPid){throw 'Published root schema/PID mismatch.'}
    Assert-ReleaseCheckPublishedRootOwner $AppPid ([long]$record.root_hwnd)
    return [long]$record.root_hwnd
}

function Invoke-ReleaseCheckInitialForeground {
    param([int]$AppPid,[long]$RootHwnd)
    # The existing focus budget/loop owns this call. Publication absence must
    # never cause enumeration or activation of another same-process window.
    if($RootHwnd -eq 0){return $false}
    Assert-ReleaseCheckPublishedRootOwner $AppPid $RootHwnd
    $null=[MivReleaseCheckNative]::SetForegroundWindow([IntPtr]$RootHwnd)
    $sample=Get-ReleaseCheckWindowSample $AppPid
    return $sample.foreground_pid -eq $AppPid -and $sample.foreground_hwnd -eq $RootHwnd
}

function Assert-ReleaseCheckReadyRoot {
    param($Ready,[int]$AppPid,[long]$RootHwnd,[string]$CaseId,[string]$Directory)
    if($RootHwnd -le 0){throw 'Ready received without a published root.'}
    $property=$Ready.PSObject.Properties['root_hwnd']
    $reportedRoot=if($null -eq $property){$null}else{$property.Value}
    if(($reportedRoot -isnot [int] -and $reportedRoot -isnot [long]) -or $reportedRoot -ne $RootHwnd){
        $mismatchException=New-Object System.InvalidOperationException -ArgumentList "Ready root HWND differs from published root: expected $RootHwnd, reported $reportedRoot."
        $mismatchException.Data['release_check_product_failure']=$true
        $mismatchException.Data['case_id']=$CaseId
        $mismatchException.Data['directory']=$Directory
        $mismatchException.Data['root_identity']=@{pid=$AppPid;published_root_hwnd=$RootHwnd;ready_root_hwnd=$reportedRoot}
        throw $mismatchException
    }
    if($Ready.pid -ne $AppPid){throw 'Ready PID mismatch.'}
    Assert-ReleaseCheckPublishedRootOwner $AppPid $RootHwnd
}

function New-ReleaseCheckRunnerErrorResult {
    param($ErrorRecord)
    $exception=$ErrorRecord.Exception
    $product=$exception.Data['release_check_product_failure'] -eq $true
    if($product){
        $directory=[string]$exception.Data['directory']
        return [ordered]@{schema_version=1;case_id=$exception.Data['case_id'];verdict='FAIL';exit_code=1
            failures=@($exception.Message);environment_errors=@();warnings=@()
            evidence=@{root_identity=$exception.Data['root_identity'];failure_artifacts=@{
                directory=$directory;root_window=(Join-Path $directory 'root-window.json');ready=(Join-Path $directory 'ready.json')
                foreground_timeline=(Join-Path $directory 'foreground-timeline.jsonl')}}}
    }
    return [ordered]@{case_id='suite-preparation';verdict='SKIPPED';exit_code=2;environment_errors=@($exception.Message);failures=@();warnings=@()}
}

function New-ReleaseCheckProcessExitResult {
    param([string]$CaseId,[int]$ProcessExitCode,[int]$ProcessId,[string]$Directory,[string]$ForegroundRecordingError)
    if($ProcessExitCode -eq 0){return $null}
    # The runtime assigns 1 to ScriptFailure and 2 to EnvironmentFailure.
    # Unknown nonzero exits are also failures; retain the original process code.
    $environment=$ProcessExitCode -eq 2
    $message="$CaseId application exit code $ProcessExitCode"
    $foregroundDecision=$null
    if($ProcessExitCode -eq 1 -and (Test-Path -LiteralPath (Join-Path $Directory 'foreground-wait-failure.json') -PathType Leaf)){
        # The typed record only nominates a wait. The Python classifier is the
        # sole owner of the evidence rules; the runtime remains ScriptFailure.
        try{
            if($ForegroundRecordingError){throw ('Foreground recording failed: '+$ForegroundRecordingError)}
            $python=(Get-Command python -ErrorAction Stop).Source
            $json=@(& $python -B (Join-Path $PSScriptRoot '..\..\release_check_foreground.py') --directory $Directory --expected-pid $ProcessId --case-id $CaseId)
            if($LASTEXITCODE -ne 0){throw 'Foreground classification command failed.'}
            $foregroundDecision=($json -join "`n") | ConvertFrom-Json
            if($foregroundDecision.schema_version -ne 1 -or $foregroundDecision.case_id -ne $CaseId -or $foregroundDecision.environment -isnot [bool] -or
                [string]::IsNullOrWhiteSpace([string]$foregroundDecision.summary)){throw 'Invalid foreground classification response.'}
            $environment=$foregroundDecision.environment
            $message+='; '+$foregroundDecision.summary
        }catch{
            $environment=$false
            $message+='; foreground wait evidence unavailable: '+$_.Exception.Message
        }
    }elseif(Test-Path -LiteralPath (Join-Path $Directory 'foreground-timeline.jsonl') -PathType Leaf){
        # Untyped failures keep their existing verdict. Include the saved final
        # observation for diagnosis only; never infer an environment excuse.
        try{
            $lines=@(Get-Content -LiteralPath (Join-Path $Directory 'foreground-timeline.jsonl') -Encoding UTF8)
            if(-not $lines.Count){throw 'empty timeline'}
            $message+='; foreground timeline rows='+$lines.Count+'; latest snapshot='+$lines[-1]
        }catch{$message+='; foreground timeline unreadable: '+$_.Exception.Message}
    }
    $failureMessages=@();$environmentMessages=@()
    if($environment){$environmentMessages+=@($message)}else{$failureMessages+=@($message)}
    return [ordered]@{
        schema_version=1;case_id=$CaseId;verdict=$(if($environment){'SKIPPED'}else{'FAIL'})
        exit_code=$(if($environment){2}else{1})
        failures=$failureMessages;environment_errors=$environmentMessages;warnings=@()
        evidence=@{process=@{pid=$ProcessId;process_exited=$true;process_exit_code=$ProcessExitCode};foreground_wait_classification=$foregroundDecision
            failure_artifacts=@{directory=$Directory;perf_log=(Join-Path $Directory 'perf_events.jsonl')
                app_log=(Join-Path $Directory 'mimageviewer.log');process=(Join-Path $Directory 'process.json')
                case_evidence=(Join-Path $Directory 'case-evidence.json')
                root_window=(Join-Path $Directory 'root-window.json')
                foreground_timeline=(Join-Path $Directory 'foreground-timeline.jsonl')
                foreground_wait_failure=(Join-Path $Directory 'foreground-wait-failure.json')}}
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
        (Join-Path $smoke 'mimageviewer-mcp.exe') (Join-Path $smoke 'mimageviewer-musicvideo.exe') (Join-Path $smoke 'mimageviewer-browser-host.exe')
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
 [DllImport("user32.dll")] static extern bool IsWindow(IntPtr w);
 [DllImport("user32.dll")] static extern bool IsIconic(IntPtr w);
 [DllImport("user32.dll")] static extern bool IsHungAppWindow(IntPtr w);
 [DllImport("user32.dll",ExactSpelling=true)] static extern IntPtr HungWindowFromGhostWindow(IntPtr w);
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
 public static int[] OuterRect(int width,int height,uint dpi,int logicalX,int logicalY){
   // Winit 0.30's normal decorated, resizable root: WS_OVERLAPPEDWINDOW |
   // WS_CLIPSIBLINGS; WS_EX_WINDOWEDGE | WS_EX_ACCEPTFILES | WS_EX_APPWINDOW.
   // Visibility/clipping flags do not add nonclient space. There is no menu.
   // --window-size is logical client size, not decorated physical window size.
   var r=new Rect{R=(int)Math.Round(width*dpi/96.0),B=(int)Math.Round(height*dpi/96.0)};
   if(!AdjustWindowRectExForDpi(ref r,0x04CF0000,false,0x00040110,dpi))throw new Exception("Decorated window size unavailable");
   // egui-winit passes a LogicalPosition to winit, which sets the OUTER origin.
   // AdjustWindowRectExForDpi supplies the extent, not an offset from that origin.
   int x=(int)Math.Round(logicalX*dpi/96.0),y=(int)Math.Round(logicalY*dpi/96.0);
   return new[]{x,y,x+r.R-r.L,y+r.B-r.T};
 }
 public static int[] WindowGeometry(IntPtr hwnd){
   var previous=SetThreadDpiAwarenessContext(new IntPtr(-4));
   if(previous==IntPtr.Zero)throw new Exception("Per-monitor DPI context unavailable");
   try{Rect c,r,area;if(!GetClientRect(hwnd,out c)||!GetWindowRect(hwnd,out r)||!SystemParametersInfo(0x30,0,out area,0))throw new Exception("Root window geometry/work area unavailable");return new[]{(int)GetDpiForWindow(hwnd),c.R-c.L,c.B-c.T,r.L,r.T,r.R,r.B,area.L,area.T,area.R,area.B};}
   finally{SetThreadDpiAwarenessContext(previous);}
 }
 [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] struct ProcessEntry {
   public uint size,usage,pid; public UIntPtr heap; public uint module,threads,parent;
   public int priority;public uint flags;
   [MarshalAs(UnmanagedType.ByValTStr,SizeConst=260)] public string exe;
 }
 [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr CreateToolhelp32Snapshot(uint flags,uint pid);
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,ExactSpelling=true,SetLastError=true)] static extern bool Process32FirstW(IntPtr snapshot,ref ProcessEntry entry);
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,ExactSpelling=true,SetLastError=true)] static extern bool Process32NextW(IntPtr snapshot,ref ProcessEntry entry);
 [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
 [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access,bool inherit,uint pid);
 [DllImport("kernel32.dll")] static extern bool GetProcessTimes(IntPtr h,out long creation,out long exit,out long kernel,out long user);
 static long? ProcessStart(uint pid){
   var h=OpenProcess(0x1000,false,pid);if(h==IntPtr.Zero)return null;
   try{long c,e,k,u;return GetProcessTimes(h,out c,out e,out k,out u)?(long?)c:null;}
   finally{CloseHandle(h);}
 }
 static System.Collections.Generic.Dictionary<uint,ProcessEntry> Processes(){
   var h=CreateToolhelp32Snapshot(2,0);if(h==new IntPtr(-1))return null;
   try{var result=new System.Collections.Generic.Dictionary<uint,ProcessEntry>();var e=new ProcessEntry();e.size=(uint)Marshal.SizeOf(e);
     if(!Process32FirstW(h,ref e))return null;
     do{result[e.pid]=e;}while(Process32NextW(h,ref e));
     return Marshal.GetLastWin32Error()==18?result:null;
   }finally{CloseHandle(h);}
 }
 static bool? Descendant(uint candidate,uint app,System.Collections.Generic.Dictionary<uint,ProcessEntry> processes){
   if(processes==null)return null;
   var appStart=ProcessStart(app);if(!appStart.HasValue)return null;
   var seen=new System.Collections.Generic.HashSet<uint>();
   while(candidate!=0){if(candidate==app)return true;ProcessEntry e;
     if(!seen.Add(candidate)||!processes.TryGetValue(candidate,out e))return null;
     if(e.parent==app)return true;
     var childStart=ProcessStart(candidate);if(!childStart.HasValue)return null;
     // A process already alive before this held app process cannot descend from
     // it. Stop before protected system ancestors whose times may be unreadable.
     if(childStart.Value<appStart.Value)return false;
     if(e.parent!=0){
       // A recycled parent PID must not turn an orphaned mIV descendant into
       // apparently unrelated foreground evidence. Unknown ancestry stays FAIL.
       var parentStart=ProcessStart(e.parent);
       if(!parentStart.HasValue||parentStart.Value>childStart.Value)return null;
     }
     candidate=e.parent;
   }return false;
 }
 static string ProcessName(uint pid,System.Collections.Generic.Dictionary<uint,ProcessEntry> processes){
   ProcessEntry e;return processes!=null&&processes.TryGetValue(pid,out e)?System.IO.Path.GetFileName(e.exe):"";
 }
 public class ForegroundRecord {
   public int schema_version=1; public long unix_ms,root_hwnd,foreground_hwnd,ghost_original_hwnd;
   public uint app_pid,root_pid,foreground_pid,ghost_original_pid;
   public bool root_alive,root_visible,root_minimized,root_hung,ownership_resolved;
   public bool? foreground_descendant,ghost_original_descendant;
   public string foreground_process_name="",foreground_class="",ghost_original_process_name="";
 }
 public static ForegroundRecord CaptureForeground(uint app,long root){
   var r=new ForegroundRecord();r.unix_ms=DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();r.app_pid=app;r.root_hwnd=root;
   var window=new IntPtr(root);r.root_alive=root!=0&&IsWindow(window);
   GetWindowThreadProcessId(window,out r.root_pid);
   if(r.root_alive){r.root_visible=IsWindowVisible(window);r.root_minimized=IsIconic(window);r.root_hung=IsHungAppWindow(window);}
   var fg=GetForegroundWindow();r.foreground_hwnd=fg.ToInt64();GetWindowThreadProcessId(fg,out r.foreground_pid);
   var name=new StringBuilder(256);GetClassName(fg,name,name.Capacity);r.foreground_class=name.ToString();
   var processes=Processes();r.foreground_process_name=ProcessName(r.foreground_pid,processes);
   r.foreground_descendant=Descendant(r.foreground_pid,app,processes);
   r.ownership_resolved=fg!=IntPtr.Zero&&r.foreground_pid!=0&&r.foreground_process_name!=""&&r.foreground_class!=""&&r.foreground_descendant.HasValue;
   if(r.foreground_class=="Ghost"){
     // Undocumented on some Windows versions: unavailable resolution is evidence
     // failure, never permission to classify a possibly hung mIV as external.
     try{var original=HungWindowFromGhostWindow(fg);r.ghost_original_hwnd=original.ToInt64();
       GetWindowThreadProcessId(original,out r.ghost_original_pid);
       r.ghost_original_process_name=ProcessName(r.ghost_original_pid,processes);
       r.ghost_original_descendant=Descendant(r.ghost_original_pid,app,processes);
     }catch(EntryPointNotFoundException){r.ownership_resolved=false;}
   }
   uint current;GetWindowThreadProcessId(fg,out current);
   if(GetForegroundWindow()!=fg||current!=r.foreground_pid)r.ownership_resolved=false;
   return r;
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

function New-ReleaseCheckForegroundTimeline {
    param([string]$Directory,[int]$AppPid)
    return [pscustomobject]@{path=(Join-Path $Directory 'foreground-timeline.jsonl');app_pid=$AppPid;root_hwnd=[long]0;last_unix_ms=[long]0;last_signature=''}
}

function Write-ReleaseCheckForegroundTimeline {
    param($Timeline,[long]$RootHwnd=0,[switch]$Force)
    # Follow only the first root identity. A replacement/destruction must remain
    # visible in the evidence, rather than silently following another app window.
    if($Timeline.root_hwnd -eq 0 -and $RootHwnd -ne 0){$Timeline.root_hwnd=$RootHwnd}
    $record=[MivReleaseCheckNative]::CaptureForeground([uint32]$Timeline.app_pid,$Timeline.root_hwnd)
    $signature=$record | Select-Object * -ExcludeProperty unix_ms | ConvertTo-Json -Depth 6 -Compress
    # Called by the existing outer loop only, never the sampler measurement loop.
    # Heartbeat at 500ms plus every observed change, allowing for the existing
    # identity query's latency. The classifier refuses gaps over 1500ms; a stall
    # is never filled in or retried to make the coverage appear continuous.
    if($record.unix_ms -gt $Timeline.last_unix_ms -and ($Force -or $signature -ne $Timeline.last_signature -or $record.unix_ms-$Timeline.last_unix_ms -ge 500)){
        $line=ConvertTo-Json -InputObject $record -Depth 6 -Compress
        [IO.File]::AppendAllText($Timeline.path,$line+"`n",(New-Object Text.UTF8Encoding($false)))
        $Timeline.last_signature=$signature;$Timeline.last_unix_ms=$record.unix_ms
    }
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
    param($ForegroundGeometry,$BackgroundGeometry,[int[]]$WorkArea,[int[]]$OuterRect)
    $expected=Get-ReleaseCheckCaseGeometry 'static-foreground' $ForegroundGeometry.dpi
    foreach($row in @($ForegroundGeometry,$BackgroundGeometry)){
        if($row.dpi -ne $expected.dpi -or $row.grid_cols -ne $expected.grid_cols -or
            ($row.window_size -join ',') -ne ($expected.window_size -join ',')){
            throw 'Static foreground/background geometry must match the selected DPI table row.'
        }
    }
    Assert-ReleaseCheckRectInWorkArea $OuterRect $WorkArea 'Selected static decorated window'
}

function Assert-ReleaseCheckRectInWorkArea {
    param([int[]]$WindowRect,[int[]]$WorkArea,[string]$Label='Actual window')
    # Both rectangles are physical screen coordinates, including nonzero origins.
    if($WorkArea.Count -ne 4 -or $WindowRect.Count -ne 4 -or
        $WorkArea[2] -le $WorkArea[0] -or $WorkArea[3] -le $WorkArea[1] -or
        $WindowRect[2] -le $WindowRect[0] -or $WindowRect[3] -le $WindowRect[1] -or
        $WindowRect[0] -lt $WorkArea[0] -or $WindowRect[1] -lt $WorkArea[1] -or
        $WindowRect[2] -gt $WorkArea[2] -or $WindowRect[3] -gt $WorkArea[3]){
        throw "$Label ($($WindowRect -join ',') physical px) does not fit the primary work area ($($WorkArea -join ','))."
    }
}

function Get-ReleaseCheckGeometryPreflight {
    $desktop=[MivReleaseCheckNative]::DesktopGeometry()
    $foreground=Get-ReleaseCheckCaseGeometry 'static-foreground' $desktop[0]
    $background=Get-ReleaseCheckCaseGeometry 'static-background' $desktop[0]
    $workArea=@($desktop[1..4])
    # Mirror src/lib.rs --window-size -> with_position(pos2(60.0,40.0)).
    # The kit has no product sources, so its fixed logical OUTER origin is kept
    # once in the shared table. It is absolute, not relative to the work area.
    $table=Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\..\release_check_geometry.json') -Encoding UTF8 -Raw | ConvertFrom-Json
    $position=@($table.startup_position)
    if($position.Count -ne 2 -or $position[0] -isnot [int] -or $position[1] -isnot [int]){throw 'Invalid fixed startup position in geometry table.'}
    $outer=[MivReleaseCheckNative]::OuterRect($foreground.window_size[0],$foreground.window_size[1],$desktop[0],$position[0],$position[1])
    Assert-ReleaseCheckGeometryEnvironment $foreground $background $workArea $outer
    return [pscustomobject]@{static_geometry=$foreground;startup_position=$position;work_area=$workArea;outer_rect=@($outer);outer_size=@(($outer[2]-$outer[0]),($outer[3]-$outer[1]))}
}

function Get-ReleaseCheckWindowGeometry {
    param([int]$AppPid,[long]$RootHwnd)
    [uint32]$owner=0
    $null=[MivReleaseCheckNative]::GetWindowThreadProcessId([IntPtr]$RootHwnd,[ref]$owner)
    if($RootHwnd -le 0 -or $owner -ne $AppPid){throw 'Geometry root HWND owner mismatch.'}
    $native=[MivReleaseCheckNative]::WindowGeometry([IntPtr]$RootHwnd)
    return [pscustomobject]@{pid=$AppPid;root_hwnd=$RootHwnd;dpi=$native[0];client_size=@($native[1..2]);window_rect=@($native[3..6]);work_area=@($native[7..10])}
}

function Assert-ReleaseCheckWindowGeometry {
    param($Actual,$Expected)
    $physical=@($Expected.window_size | ForEach-Object {[int][Math]::Round($_*$Expected.dpi/96.0)})
    if($Actual.dpi -ne $Expected.dpi){throw 'Actual window DPI differs from the fixture DPI.'}
    if(($Actual.client_size -join ',') -ne ($physical -join ',')){
        throw 'Actual window client size differs from the selected case geometry.'
    }
    Assert-ReleaseCheckRectInWorkArea $Actual.window_rect $Actual.work_area
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
