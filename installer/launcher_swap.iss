; Shared by the production installer and the isolated launcher swap test.
; Define these before including this file to use a separate application identity.
#ifndef MyAppExeName
  #define MyAppExeName "mimageviewer.exe"
#endif
#ifndef ShutdownEventName
  #define ShutdownEventName "Global\mImageViewerShutdown_v1"
#endif
#ifndef AppMutexName
  #define AppMutexName "Global\mImageViewerInstance_v1"
#endif
#ifndef McpMutexName
  #define McpMutexName "Global\mImageViewerMcp_v1"
#endif

[Setup]
; mIV 本体は終了の合図で閉じ、AI アプリの launcher は改名して残す。
CloseApplicationsFilterExcludes={#MyAppExeName}

[UninstallDelete]
Type: files; Name: "{app}\{#MyAppExeName}.*.old"

[Code]
const
  EVENT_MODIFY_STATE = $0002;
  SYNCHRONIZE = $00100000;
  ERROR_FILE_NOT_FOUND = 2;
  ShutdownEventName = '{#ShutdownEventName}';
  AppMutexName = '{#AppMutexName}';
  McpMutexName = '{#McpMutexName}';

var
  LauncherBackupPath: String;
  LauncherInstallSucceeded: Boolean;

function OpenEventW(dwDesiredAccess: DWORD; bInheritHandle: BOOL;
  lpName: string): THandle;
  external 'OpenEventW@kernel32.dll stdcall';
function OpenMutexW(dwDesiredAccess: DWORD; bInheritHandle: BOOL;
  lpName: string): THandle;
  external 'OpenMutexW@kernel32.dll stdcall';
function SetEvent(hEvent: THandle): BOOL;
  external 'SetEvent@kernel32.dll stdcall';
function CloseHandle(hObject: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';
function CoCreateGuid(var Guid: TGUID): Integer;
  external 'CoCreateGuid@ole32.dll stdcall';
function StringFromGUID2(var Guid: TGUID; Buffer: String; MaxChars: Integer): Integer;
  external 'StringFromGUID2@ole32.dll stdcall';

{ MCP の不在確認はアンインストールだけに使う。更新は mutex に依存しない。 }
function CheckMcpStopped(): String;
var
  MutexHandle: THandle;
  ErrorCode: DWORD;
begin
  MutexHandle := OpenMutexW(SYNCHRONIZE, False, McpMutexName);
  if MutexHandle <> 0 then
  begin
    CloseHandle(MutexHandle);
    Result := 'AI アプリ (Claude・ChatGPT など) が mImageViewer を使っています。' + #13#10 +
              'AI アプリを終了してから、もう一度実行してください';
  end
  else
  begin
    ErrorCode := DLLGetLastError;
    if ErrorCode = ERROR_FILE_NOT_FOUND then
      Result := ''
    else
      Result := '確認できませんでした。AI アプリを終了してから、' + #13#10 +
                'もう一度実行してください';
  end;
end;

{ 起動中の mImageViewer (トレイ常駐含む) にクリーン終了を要求する。
  その後、mutex が解放されるまで最大 5 秒ポーリングする。 }
function SignalAppShutdown(): Boolean;
var
  EventHandle: THandle;
  MutexHandle: THandle;
  I: Integer;
begin
  MutexHandle := OpenMutexW(SYNCHRONIZE, False, AppMutexName);
  if MutexHandle = 0 then
  begin
    Result := True;
    Exit;
  end;
  CloseHandle(MutexHandle);

  EventHandle := OpenEventW(EVENT_MODIFY_STATE, False, ShutdownEventName);
  if EventHandle <> 0 then
  begin
    SetEvent(EventHandle);
    CloseHandle(EventHandle);
  end;

  { 50ms x 100 = 最大 5 秒。早期退出に対応。 }
  for I := 0 to 99 do
  begin
    MutexHandle := OpenMutexW(SYNCHRONIZE, False, AppMutexName);
    if MutexHandle = 0 then
    begin
      Result := True;
      Exit;
    end;
    CloseHandle(MutexHandle);
    Sleep(50);
  end;
  Result := False;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  LauncherPath, BackupPath, BackupIdString: String;
  BackupId: TGUID;
begin
  { インストールを決めるまでは本体を終了させず、ファイルも動かさない。 }
  if not SignalAppShutdown then
  begin
    Result := 'mImageViewer が応答しませんでした。' + #13#10 +
              'タスクトレイアイコンを右クリック →「終了」で手動終了してから、' + #13#10 +
              '再度インストールをお試しください。';
    Exit;
  end;

  Result := '';
  LauncherPath := ExpandConstant('{app}\{#MyAppExeName}');
  if not FileExists(LauncherPath) then
    Exit;

  Result := 'mImageViewer のファイルを置き換えられませんでした。' + #13#10 +
            'AI アプリ (Claude・ChatGPT など) が mImageViewer を使っている場合は終了してから、' + #13#10 +
            'もう一度実行してください';
  if CoCreateGuid(BackupId) <> 0 then
    Exit;
  SetLength(BackupIdString, 39);
  if StringFromGUID2(BackupId, BackupIdString, 39) = 0 then
    Exit;
  BackupPath := LauncherPath + '.' + Copy(BackupIdString, 1, 38) + '.old';
  if not RenameFile(LauncherPath, BackupPath) then
    Exit;

  { 今回の退避だけを取消時の復元対象にし、成功までは削除しない。 }
  LauncherBackupPath := BackupPath;
  Log('Launcher swap: backed up to ' + BackupPath);
  Result := '';
end;

<event('CurStepChanged')>
procedure LauncherSwapStepChanged(CurStep: TSetupStep);
var
  OldFile: TFindRec;
  AppDir: String;
begin
  if CurStep = ssPostInstall then
  begin
    LauncherInstallSucceeded := True;
    AppDir := ExpandConstant('{app}\');
    if FindFirst(AppDir + '{#MyAppExeName}.*.old', OldFile) then
    begin
      try
        repeat
          { 使用中のものは残す。再起動時の削除予約はしない。 }
          if (OldFile.Attributes and FILE_ATTRIBUTE_DIRECTORY) = 0 then
            DeleteFile(AppDir + OldFile.Name);
        until not FindNext(OldFile);
      finally
        FindClose(OldFile);
      end;
    end;
  end;
end;

<event('DeinitializeSetup')>
procedure LauncherSwapDeinitializeSetup();
var
  LauncherPath: String;
begin
  if LauncherInstallSucceeded or (LauncherBackupPath = '') then
    Exit;
  LauncherPath := ExpandConstant('{app}\{#MyAppExeName}');
  { Inno のロールバック後、正規の名前が空いている場合だけ元へ戻す。 }
  if FileExists(LauncherBackupPath) and not FileExists(LauncherPath) then
  begin
    if RenameFile(LauncherBackupPath, LauncherPath) then
      Log('Launcher swap: restored ' + LauncherPath)
    else
    begin
      Log('Launcher swap: restore failed: ' + LauncherBackupPath);
      if not WizardSilent then
        MsgBox('mImageViewer のファイルを元に戻せませんでした。' + #13#10 +
               'インストールをやり直してください。', mbError, MB_OK);
    end;
  end;
end;

function InitializeUninstall(): Boolean;
var
  McpError: String;
begin
  SignalAppShutdown;
  McpError := CheckMcpStopped();
  Result := McpError = '';
  if not Result then
    SuppressibleMsgBox(McpError, mbError, MB_OK, IDOK);
end;
