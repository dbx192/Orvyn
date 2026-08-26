#define MyAppName "Orvyn"
#define MyAppVersion "1.0.0"
#define MyAppExeName "Orvyn.exe"
#ifndef MyIconHash
  #define MyIconHash "dev"
#endif

[Setup]
AppId={{7E8D9E1F-38E0-4D79-908C-F5B5A7A74A31}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher=Orvyn
; Per-user installs avoid UAC and keep the uninstaller executable under the
; current user's profile, where it can be run without administrator rights.
DefaultDirName={localappdata}\Programs\Orvyn
DefaultGroupName={#MyAppName}
OutputDir=..\..\dist\windows
OutputBaseFilename=orvyn-{#MyAppVersion}-windows-x86_64-{#MyIconHash}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\icons\icon.ico
UninstallDisplayIcon={app}\orvyn-icon-{#MyIconHash}.ico

[Files]
Source: "..\..\target\x86_64-pc-windows-msvc\release\orvyn.exe"; DestDir: "{app}"; DestName: "{#MyAppExeName}"; Flags: ignoreversion
Source: "..\..\target\x86_64-pc-windows-msvc\release\duckdb.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\icons\icon.ico"; DestDir: "{app}"; DestName: "orvyn-icon-{#MyIconHash}.ico"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\orvyn-icon-{#MyIconHash}.ico"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\orvyn-icon-{#MyIconHash}.ico"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent
