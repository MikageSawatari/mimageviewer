; mImageViewer Inno Setup Script
; Build: "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer\mimageviewer.iss

#define MyAppName "mImageViewer"
#define MyAppVersion "0.8.1"
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
; タスクトレイ常駐中の mImageViewer を自動検出して「閉じてください」ダイアログを出す。
; アプリ側の `src/single_instance.rs::MUTEX_NAME` と完全一致させること。
; 名前が食い違うと、アップデート時に常駐プロセスが DLL を掴んだまま上書きが失敗する。
AppMutex=Global\mImageViewerInstance_v1

[Languages]
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
; readme.txt はインストール先にも配置する (Vector 申請要件に合わせ、
; インストーラ単体でもそのまま使える形を保つ)。
Source: "readme.txt"; DestDir: "{app}"; Flags: ignoreversion
; Susie 32bit ワーカーは本体 exe に include_bytes! で埋め込まれており、
; 初回起動時に %APPDATA%\mimageviewer\mimageviewer-susie32.exe へ自動展開される。
; そのため別ファイルとしては同梱しない。

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{#MyAppName} をアンインストール"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[Code]
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
