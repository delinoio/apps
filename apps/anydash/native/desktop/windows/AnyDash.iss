; SPDX-License-Identifier: Apache-2.0
; Per-user Windows x64 installer for the signed AnyDash desktop app. The release
; job compiles it with Inno Setup:
;   iscc /DAppVersion=X.Y.Z /DSourceDir=<signed application directory> /O<output> AnyDash.iss
; The application directory already contains the signed widget MSIX.
; The app lives in "AnyDash Desktop\App" so that the in-app updater can swap
; that directory with "App.previous"; the uninstaller stays outside it. The CLI
; install script uses the separate "AnyDash" directory.

#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef SourceDir
  #error SourceDir is required
#endif

[Setup]
AppId={{6FE99477-E229-4046-9BB8-EB0A75D863CD}
AppName=AnyDash
AppVersion={#AppVersion}
AppVerName=AnyDash {#AppVersion}
AppPublisher=Delino
AppPublisherURL=https://anydash.delino.io
AppSupportURL=https://github.com/delinoio/apps/issues
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\AnyDash Desktop\App
UninstallFilesDir={localappdata}\Programs\AnyDash Desktop\Uninstall
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
OutputBaseFilename=AnyDash-Setup-{#AppVersion}-windows-x64
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
UninstallDisplayName=AnyDash
UninstallDisplayIcon={app}\resources\app\AnyDash.ico
LicenseFile={#SourceDir}\resources\app\LICENSE

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
Name: "{userprograms}\AnyDash"; Filename: "{app}\lynxtron.exe"; IconFilename: "{app}\resources\app\AnyDash.ico"

[Run]
; The Widgets Board provider is a separate signed MSIX package. [Code] quotes
; its path for PowerShell because the path contains the user profile path.
Filename: "powershell.exe"; Parameters: "-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command ""Add-AppxPackage -Path {code:WidgetPackagePathArgument} -ForceApplicationShutdown"""; Flags: runhidden waituntilterminated; StatusMsg: "Adding the AnyDash widgets..."
Filename: "{app}\lynxtron.exe"; Description: "Start AnyDash"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "powershell.exe"; Parameters: "-NoProfile -NonInteractive -Command ""Get-AppxPackage -Name Delino.AnyDash.Widgets | Remove-AppxPackage"""; Flags: runhidden waituntilterminated; RunOnceId: "RemoveAnyDashWidgets"

[UninstallDelete]
; In-app updates replace files that the install log does not list.
Type: filesandordirs; Name: "{app}"
Type: filesandordirs; Name: "{app}.previous"

[Code]
procedure DoubleQuoteCharacter(var Value: String; Quote: String);
begin
  StringChangeEx(Value, Quote, Quote + Quote, True);
end;

// Returns the widget MSIX path as a PowerShell single-quoted string. A profile
// path can contain an apostrophe (C:\Users\O'Brien). PowerShell also ends a
// single-quoted string at the typographic single quotation marks U+2018 to
// U+201B, and reads each of these characters doubled as one literal character.
function WidgetPackagePathArgument(Param: String): String;
begin
  Result := ExpandConstant('{app}\resources\app\widgets\AnyDash.Widgets.msix');
  DoubleQuoteCharacter(Result, '''');
  DoubleQuoteCharacter(Result, #$2018);
  DoubleQuoteCharacter(Result, #$2019);
  DoubleQuoteCharacter(Result, #$201A);
  DoubleQuoteCharacter(Result, #$201B);
  Result := '''' + Result + '''';
end;
