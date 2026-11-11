; Inno Setup script for RV Report (built by .github/workflows/release.yml).
; Per-user install: no administrator rights needed on the examiner's PC.
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\.."
#endif

[Setup]
AppId={{D943D333-1B25-4E70-B3FC-2256DADA9F4B}
AppName=RV Report
AppVersion={#AppVersion}
AppPublisher=Rakvor
AppPublisherURL=https://github.com/Panacota96/rv-report
DefaultDirName={localappdata}\Programs\Rakvor\RV Report
DefaultGroupName=Rakvor
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile={#SourceDir}\LICENSE
OutputBaseFilename=rv-report-{#AppVersion}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
SetupIconFile={#SourceDir}\assets\brand\rv.ico
ChangesEnvironment=yes
UninstallDisplayIcon={app}\rv-report-gui.exe

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"

[Tasks]
Name: "addtopath"; Description: "Add the rv-report command to PATH"; Flags: checkedonce
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\target\release\rv-report.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\target\release\rv-report-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"
Source: "{#SourceDir}\NOTICE"; DestDir: "{app}"
Source: "{#SourceDir}\CHANGELOG.md"; DestDir: "{app}"
Source: "{#SourceDir}\docs\*"; DestDir: "{app}\docs"; Flags: recursesubdirs
Source: "{#SourceDir}\examples\sample\evidence\*"; DestDir: "{app}\examples\sample\evidence"

[Icons]
Name: "{group}\RV Report"; Filename: "{app}\rv-report-gui.exe"
Name: "{group}\RV Report documentation"; Filename: "{app}\docs"
Name: "{autodesktop}\RV Report"; Filename: "{app}\rv-report-gui.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\rv-report-gui.exe"; Description: "Start RV Report"; Flags: nowait postinstall skipifsilent

[Code]
function NeedsAddPath(Dir: string): Boolean;
var
  Paths: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Paths) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') = 0;
end;
