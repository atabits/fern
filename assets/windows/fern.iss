; The Windows installer, built by script/package with Inno Setup:
;   iscc /DVersion=<version> /DFileVersion=<x.y.z> /DSourceDir=<folder with fern.exe> /DOutputDir=<dir> /DOutputName=<name> fern.iss
; It installs for the current user without asking for administrator rights; the first page
; offers installing for everyone instead.

#define AppName "Fern"
#define AppExe "fern.exe"

[Setup]
; The same id for every version, so that a new version replaces the old one.
AppId={{99571A45-E987-4A60-A898-1843F3429CD4}
AppName={#AppName}
AppVersion={#Version}
AppVerName={#AppName} {#Version}
; The installer's own file properties; FileVersion is the version without a build suffix.
VersionInfoVersion={#FileVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
AppPublisher=Amin Atabiev
AppPublisherURL=https://github.com/atabits/fern
AppSupportURL=https://github.com/atabits/fern/issues
AppUpdatesURL=https://github.com/atabits/fern/releases
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\..\LICENSE
SetupIconFile=..\icon\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
CloseApplications=yes
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; fern:// links ("Copy Link" on a task) open in Fern.
Root: HKA; Subkey: "Software\Classes\fern"; ValueType: string; ValueName: ""; ValueData: "URL:Fern task link"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\fern"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\fern\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"
Root: HKA; Subkey: "Software\Classes\fern\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
