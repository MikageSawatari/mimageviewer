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
    param([string]$RepoRoot)
    $smoke = Join-Path $RepoRoot 'target\portable-smoke'
    Assert-NoReparsePath $smoke $RepoRoot 'release-artifact'
    Assert-NoReparseTree $smoke 'release-artifact'
    $marker = Join-Path $smoke 'data\.disposable-smoke-data'
    if ((Get-Content -LiteralPath $marker -Encoding ASCII -Raw).Trim() -ne 'mimageviewer-disposable-smoke-v1;test-script=true') { throw 'Disposable test-script marker mismatch.' }
    Assert-SmokeBuildManifest (Join-Path $smoke '.test-script-build.json') (Get-MivSourceFingerprint $RepoRoot) `
        (Join-Path $smoke 'mimageviewer.exe') (Join-Path $smoke 'mimageviewer-remote.exe') (Join-Path $smoke 'mimageviewer-epub-pdf.exe')
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
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr w);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr w);
 public delegate bool EnumProc(IntPtr w,IntPtr p);
 [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc c,IntPtr p);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr w,StringBuilder b,int n);
 [StructLayout(LayoutKind.Sequential)] public struct Rect { public int L,T,R,B; }
 [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr w,out Rect r);
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
    param([int]$AppPid)
    $h = [MivReleaseCheckNative]::GetForegroundWindow(); [uint32]$owner = 0
    $null = [MivReleaseCheckNative]::GetWindowThreadProcessId($h, [ref]$owner)
    $counts = [MivReleaseCheckNative]::Counts([uint32]$AppPid)
    return [pscustomobject]@{ unix_ms = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds(); foreground_pid = $owner; foreground_hwnd = $h.ToInt64(); total = $counts[0]; visible = $counts[1] }
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
    $Window.Show()
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
