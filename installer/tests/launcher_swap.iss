; Never use the production identity, paths, registry entries, or shutdown event.
#ifndef TestRoot
  #error TestRoot is required
#endif
#ifndef TestToken
  #error TestToken is required
#endif
#ifndef LauncherSource
  #error LauncherSource is required
#endif
#ifndef TestOutputName
  #error TestOutputName is required
#endif
#define MyAppExeName "miv-swap-test.exe"
#define ShutdownEventName "Local\miv-swap-test-shutdown-" + TestToken
#define AppMutexName "Local\miv-swap-test-instance-" + TestToken
#define McpMutexName "Local\miv-swap-test-mcp-" + TestToken
#ifdef TestDisableLauncherRmExclusion
  #define LauncherCloseApplicationsFilterExcludes ""
#endif
#include "..\launcher_swap.iss"

[Setup]
AppId=miv-swap-test-{#TestToken}
AppName=Launcher swap isolated test
AppVersion=1.0
DefaultDirName={#TestRoot}\install
OutputDir={#TestRoot}\output
OutputBaseFilename={#TestOutputName}
PrivilegesRequired=lowest
CreateUninstallRegKey=no
UninstallDisplayName=Launcher swap isolated test
DisableDirPage=yes
DisableProgramGroupPage=yes
UsePreviousAppDir=no
UsePreviousTasks=no
UsePreviousLanguage=no
Compression=none
WizardStyle=classic

[Files]
Source: "{#LauncherSource}"; DestDir: "{app}"; DestName: "{#MyAppExeName}"; Flags: ignoreversion; AfterInstall: TestLauncherWritten
; Keep one unused resource eligible for RM even when the launcher is excluded.
; This is a data copy of PING, never loaded or executed as a DLL.
Source: "{#LauncherSource}"; DestDir: "{app}"; DestName: "rm-probe.dll"; Flags: ignoreversion
#ifdef FailAfterLauncher
; An unavailable external file fails only after the new launcher was installed.
Source: "{#TestRoot}\missing-external-file"; DestDir: "{app}"; Flags: external ignoreversion; ExternalSize: 1
#endif

[Code]
function InitializeSetup(): Boolean;
begin
  if RmSessionStarted then
    Log('Swap test: RestartManager session started')
  else
    Log('Swap test: RestartManager session unavailable');
  Result := True;
end;

procedure TestLauncherWritten();
begin
  Log('Swap test: installed launcher SHA256=' +
      GetSHA256OfFile(ExpandConstant('{app}\{#MyAppExeName}')));
end;
