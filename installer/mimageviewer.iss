; mImageViewer Inno Setup Script
; Build: "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer\mimageviewer.iss

#define MyAppName "mImageViewer"
#define MyAppVersion "1.9.0"
#define MyAppPublisher "Mikage Sawatari"
#define MyAppURL "https://mikage.to/mimageviewer/"
#define MyAppExeName "mimageviewer.exe"

[Setup]
AppId={{E8A3F2B1-7C45-4D6E-9B8A-1F2E3D4C5B6A}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
OutputDir=Output
OutputBaseFilename=mImageViewer_setup
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
MinVersion=10.0
; インストール開始画面で readme.txt を表示する (Vector 申請要件の一部を兼ねる)。
InfoBeforeFile=readme.txt
; AppMutex= による Inno Setup 組み込みの事前チェックは**使わない**。あれは Welcome 画面
; よりも前に発火してしまい、ユーザが readme / 同意 / インストール先選択を確認する前に
; 「閉じてください」ダイアログが出る。代わりに [Code] の `PrepareToInstall` で、ユーザが
; 「インストール」ボタンを押した直後 (= インストールの意思を示した後) に、shutdown event
; を投げて mIV を自動クリーン終了させる。

[Languages]
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; 配布する exe はランチャー (`mimageviewer.exe`) のみ。ランチャーは本体
; `mimageviewer-core.exe` と FFmpeg LGPL DLL を include_bytes! で
; 内包しており、初回起動時に %APPDATA%\mimageviewer\runtime\<version>\ へ
; 自動展開する。詳細は CLAUDE.md「FFmpeg LGPL DLL 管理」節。
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
; readme.txt はインストール先にも配置する (Vector 申請要件に合わせ、
; インストーラ単体でもそのまま使える形を保つ)。
Source: "readme.txt"; DestDir: "{app}"; Flags: ignoreversion
; Susie 32bit ワーカーは本体 exe (mimageviewer-core.exe) に include_bytes! で
; 埋め込まれており、初回起動時に %APPDATA%\mimageviewer\mimageviewer-susie32.exe
; へ自動展開される。そのため別ファイルとしては同梱しない。
;
; FFmpeg LGPL ライセンス本文は %APPDATA% に展開する核に同梱されるが、
; ライセンス対応のためインストール先にも配置しておく (ユーザーが見つけやすいように)。
Source: "..\vendor\ffmpeg\LICENSE.txt"; DestDir: "{app}"; DestName: "FFmpeg-LICENSE.txt"; Flags: ignoreversion
; RAR 展開には unrar crate 経由で RARLAB UnRAR ソースを組み込むため、
; UnRAR ライセンス本文をインストール先にも配置する。
Source: "..\UNRAR-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{#MyAppName} をアンインストール"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[Code]
const
  EVENT_MODIFY_STATE = $0002;
  SYNCHRONIZE = $00100000;
  ShutdownEventName = 'Global\mImageViewerShutdown_v1';
  AppMutexName = 'Global\mImageViewerInstance_v1';

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

{ 起動中の mImageViewer (トレイ常駐含む) にクリーン終了を要求する。
  - Named Event `Global\mImageViewerShutdown_v1` を SetEvent
  - その後、Mutex が解放されるまで最大 5 秒ポーリング
  返り値は「mIV が終了した (または最初から起動していなかった)」かどうか。 }
function SignalAppShutdown(): Boolean;
var
  EventHandle: THandle;
  MutexHandle: THandle;
  I: Integer;
begin
  { 最初に mutex を見て、そもそも mIV が起動していなければ何もせずに成功扱い。 }
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
begin
  { ユーザが「インストール」ボタンを押した直後に shutdown event を投げ、
    mIV (トレイ常駐含む) をクリーン終了させる。readme / 同意画面を見ている
    あいだにユーザが予定を変えてキャンセルする余地を残すため、ここまで
    起動中の mIV には手を触れない方針。 }
  if SignalAppShutdown then
    Result := ''
  else
    Result := 'mImageViewer が応答しませんでした。' + #13#10 +
              'タスクトレイアイコンを右クリック →「終了」で手動終了してから、' + #13#10 +
              '再度インストールをお試しください。';
end;

function InitializeUninstall(): Boolean;
begin
  { アンインストール時も常駐中の mIV をクリーン終了させる (exe ファイル削除が
    「使用中」で失敗するのを防ぐ)。 }
  SignalAppShutdown;
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  AppDataDir: String;
  MsgResult: Integer;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    AppDataDir := ExpandConstant('{userappdata}\mimageviewer');
    if DirExists(AppDataDir) then
    begin
      MsgResult := MsgBox(
        '設定ファイルとキャッシュを削除しますか？' + #13#10 +
        '（' + AppDataDir + '）' + #13#10 + #13#10 +
        '「いいえ」を選ぶと、再インストール時に設定が引き継がれます。',
        mbConfirmation, MB_YESNO or MB_DEFBUTTON2);
      if MsgResult = IDYES then
      begin
        DelTree(AppDataDir, True, True, True);
      end;
    end;
  end;
end;
