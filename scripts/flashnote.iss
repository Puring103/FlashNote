[Setup]
AppId=FlashNote
AppName=闪记 FlashNote
AppVersion={#Version}
AppPublisher=FlashNote
DefaultDirName={code:InstallDirectory}
DefaultGroupName=闪记
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\闪记.exe
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
AppMutex=Local\Suojian-Local-Mutex
CloseApplications=no
WizardStyle=modern
OutputBaseFilename=FlashNote-{#Version}-windows-x64-setup
OutputDir={#PackageOutput}
Compression=lzma2
SolidCompression=yes

[Files]
Source: "..\target\release\suojian.exe"; DestDir: "{app}"; DestName: "闪记.exe"; Flags: ignoreversion
Source: "..\target\release\flashnote.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\flashnote.exe"; DestDir: "{app}"; DestName: "suojian-cli.exe"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\crates\gpui\LICENSE-APACHE"; DestDir: "{app}"; DestName: "LICENSE-gpui.txt"; Flags: ignoreversion
Source: "..\crates\gpui-component\LICENSE-APACHE"; DestDir: "{app}"; DestName: "LICENSE-gpui-component.txt"; Flags: ignoreversion
Source: "..\crates\gpui-note-editor\LICENSE"; DestDir: "{app}"; DestName: "LICENSE-gpui-note-editor.txt"; Flags: ignoreversion
Source: "..\assets\icons\lucide\LICENSE"; DestDir: "{app}"; DestName: "LICENSE-lucide.txt"; Flags: ignoreversion
#ifdef IncludeSpeech
Source: "..\runtime\whisper\Release\*"; DestDir: "{app}\runtime\whisper\Release"; Flags: ignoreversion recursesubdirs
Source: "..\assets\licenses\whisper.cpp.txt"; DestDir: "{app}\runtime\whisper"; DestName: "LICENSE-whisper.cpp.txt"; Flags: ignoreversion
Source: "..\assets\licenses\SDL2.txt"; DestDir: "{app}\runtime\whisper"; DestName: "LICENSE-SDL2.txt"; Flags: ignoreversion
#endif

[Icons]
Name: "{group}\闪记 FlashNote"; Filename: "{app}\闪记.exe"; WorkingDir: "{app}"; AppUserModelID: "NoteHarbor.Suojian"

[Registry]
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\NoteHarbor.Suojian"; ValueType: string; ValueName: "DisplayName"; ValueData: "闪记 FlashNote"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\NoteHarbor.Suojian"; ValueType: string; ValueName: "IconUri"; ValueData: "{app}\闪记.exe"
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Suojian"; ValueData: """{app}\闪记.exe"" --background"; Check: ShouldEnableStartup

[Run]
Filename: "{app}\闪记.exe"; Description: "{cm:LaunchProgram,闪记 FlashNote}"; Flags: nowait postinstall skipifsilent

[Code]
const
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\';
  LegacyKey = UninstallKey + 'Suojian';
  StartupKey = 'Software\Microsoft\Windows\CurrentVersion\Run';

var
  EnableStartup: Boolean;

function LegacyDirectory(): String;
begin
  Result := '';
  if RegQueryStringValue(HKCU32, LegacyKey, 'InstallLocation', Result) and
     not DirExists(Result) then
    Result := '';
end;

function InstallDirectory(Param: String): String;
begin
  Result := LegacyDirectory();
  if Result = '' then
    Result := ExpandConstant('{localappdata}\Programs\FlashNote');
end;

procedure InitializeWizard();
begin
  EnableStartup := RegValueExists(HKCU, StartupKey, 'Suojian') or
    ((LegacyDirectory() = '') and not RegKeyExists(HKCU, UninstallKey + 'FlashNote_is1'));
end;

function ShouldEnableStartup(): Boolean;
begin
  Result := EnableStartup;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  PreviousDirectory: String;
begin
  Result := '';
  PreviousDirectory := LegacyDirectory();
  if (PreviousDirectory <> '') and
     (CompareText(RemoveBackslashUnlessRoot(PreviousDirectory), RemoveBackslashUnlessRoot(ExpandConstant('{app}'))) <> 0) then
    Result := 'To upgrade FlashNote, select the existing installation folder: ' + PreviousDirectory;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  PreviousDirectory: String;
begin
  if CurStep <> ssPostInstall then
    Exit;
  PreviousDirectory := LegacyDirectory();
  if PreviousDirectory <> '' then
  begin
    RegDeleteKeyIncludingSubkeys(HKCU32, LegacyKey);
    DeleteFile(AddBackslash(PreviousDirectory) + 'uninstall.ps1');
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  StartupCommand: String;
begin
  if (CurUninstallStep = usUninstall) and
     RegQueryStringValue(HKCU, StartupKey, 'Suojian', StartupCommand) and
     (CompareText(StartupCommand, '"' + ExpandConstant('{app}\闪记.exe') + '" --background') = 0) then
    RegDeleteValue(HKCU, StartupKey, 'Suojian');
end;
