; EloraNotes installer: a normal next-next-finish wizard plus an entry in
; Settings > Apps so it can be uninstalled like any other Windows app.
;
; Built by the release workflow with:
;   ISCC.exe /DAppVersion=0.7.0 packaging\windows\elora-notes.iss
;
; The install is per-user (no administrator prompt), which also lets the in-app updater
; run this same wizard silently over the old copy.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#define AppName "EloraNotes"
#define AppExe "elora.exe"
#define AppPublisher "noxilith"
#define AppUrl "https://github.com/noxilith/EloraNotes"
#define AppId "{{B7C4E2A1-9D3F-4B6E-8E5A-1F0C2B7D6A93}"

[Setup]
AppId={#AppId}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
LicenseFile=..\..\LICENSE
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=EloraNotes-{#AppVersion}-windows-x86_64-setup
SetupIconFile=..\..\assets\icons\elora-notes.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; AI apps run `elora.exe --mcp` from the install folder, with no window to ask to close; a
; polite request never reaches them, the file stays locked, and a silent update changes nothing.
CloseApplications=force
RestartApplications=no
ChangesAssociations=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; Opens share links: elora://share/... starts the app with the link as its argument.
Root: HKCU; Subkey: "Software\Classes\elora"; ValueType: string; ValueData: "URL:EloraNotes share link"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\elora"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\elora\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall

[UninstallDelete]
Type: filesandordirs; Name: "{app}"
