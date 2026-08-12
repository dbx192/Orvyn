#define MyAppName "Orvyn"
#define MyAppVersion "0.1.0"
#define MyAppExeName "Orvyn.exe"

[Setup]
AppId={{7E8D9E1F-38E0-4D79-908C-F5B5A7A74A31}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher=Orvyn
DefaultDirName=D:\Program Files\Orvyn
DefaultGroupName={#MyAppName}
OutputDir=..\..\dist\windows
OutputBaseFilename=orvyn-{#MyAppVersion}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\icons\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}

[Files]
Source: "..\..\target\x86_64-pc-windows-msvc\release\orvyn.exe"; DestDir: "{app}"; DestName: "{#MyAppExeName}"; Flags: ignoreversion
Source: "..\..\target\x86_64-pc-windows-msvc\release\duckdb.dll"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent
