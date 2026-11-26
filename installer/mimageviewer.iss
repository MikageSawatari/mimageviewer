; mImageViewer Inno Setup Script
; Build: "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer\mimageviewer.iss

#define MyAppName "mImageViewer"
#define MyAppVersion "5.8.0"
#define MyAppPublisher "Mikage Sawatari"
#define MyAppURL "https://mikage.to/mimageviewer/"
#ifndef MyAppId
  #define MyAppId "{{E8A3F2B1-7C45-4D6E-9B8A-1F2E3D4C5B6A}"
#endif
#ifndef LauncherSource
  #define LauncherSource "..\target\release\mimageviewer.exe"
#endif

#include "launcher_swap.iss"

[Setup]
AppId={#MyAppId}
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
; `mimageviewer-core.exe`、`mimageviewer-remote.exe`、`mimageviewer-epub-pdf.exe`、
; `mimageviewer-mcp.exe`、`mimageviewer-musicvideo.exe`、`mimageviewer-browser-host.exe` と FFmpeg LGPL DLL を include_bytes! で
; 内包しており、初回起動時に %APPDATA%\mimageviewer\runtime\<version>\ へ
; 自動展開する。詳細は docs/native-dependencies.md「FFmpeg LGPL DLL 管理」節。
Source: "{#LauncherSource}"; DestDir: "{app}"; DestName: "{#MyAppExeName}"; Flags: ignoreversion
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
; PDF, RAW, AVIF and JPEG components: full upstream licenses and attributions.
Source: "..\LIBRAW-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\PDFIUM-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LIBAVIF-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\DAV1D-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LIBYUV-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\ZLIB-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LIBJPEG-TURBO-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
; ローカル差し替えした egui-wgpu を含む egui 0.33.3 のデュアルライセンス本文。
Source: "..\vendor\egui-wgpu\LICENSE-MIT"; DestDir: "{app}"; DestName: "egui-LICENSE-MIT.txt"; Flags: ignoreversion
Source: "..\vendor\egui-wgpu\LICENSE-APACHE"; DestDir: "{app}"; DestName: "egui-LICENSE-APACHE.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{#MyAppName} をアンインストール"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[Code]
// Delete only our managed tree, never follow a junction into user data.
procedure RemoveBrowserHostTree(const Dir: String);
var
  FindRec: TFindRec;
  Child: String;
begin
  if FindFirst(Dir + '\*', FindRec) then
  begin
    try
      repeat
        if (FindRec.Name <> '.') and (FindRec.Name <> '..') then
        begin
          Child := Dir + '\' + FindRec.Name;
          if (FindRec.Attributes and FILE_ATTRIBUTE_DIRECTORY) <> 0 then
          begin
            if (FindRec.Attributes and $400) = 0 then RemoveBrowserHostTree(Child);
            RemoveDir(Child);
          end
          else DeleteFile(Child);
        end;
      until not FindNext(FindRec);
    finally
      FindClose(FindRec);
    end;
  end;
  RemoveDir(Dir);
end;

procedure RemoveBrowserRegistration;
var
  Root, Manifest, Key, Value: String;
  Index: Integer;
  FindRec: TFindRec;
begin
  Root := ExpandConstant('{userappdata}\mimageviewer\browser-host');
  Manifest := Root + '\to.mikage.mimageviewer.json';
  for Index := 0 to 1 do
  begin
    if Index = 0 then Key := 'Software\Google\Chrome\NativeMessagingHosts\to.mikage.mimageviewer'
    else Key := 'Software\Microsoft\Edge\NativeMessagingHosts\to.mikage.mimageviewer';
    if RegQueryStringValue(HKCU, Key, '', Value) and (CompareText(Value, Manifest) = 0) then
      RegDeleteKeyIncludingSubkeys(HKCU, Key);
  end;
  // This is the uninstalling account only; other accounts unregister in mIV.
  if FindFirst(Root, FindRec) then
  begin
    try
      if (FindRec.Attributes and $400) = 0 then RemoveBrowserHostTree(Root)
      else RemoveDir(Root);
    finally
      FindClose(FindRec);
    end;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  AppDataDir: String;
  MsgResult: Integer;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    RemoveBrowserRegistration;
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
