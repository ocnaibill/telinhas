; Inno Setup script for the Telinhas installer. Built by scripts/build-release.ps1,
; which passes AppVersion, SourceDir (the exe and the FFmpeg DLLs, side by side)
; and OutputDir.
;
; The app goes in the user's own Programs folder (%LocalAppData%\Programs\Telinhas),
; so installing and updating never ask for administrator rights.

#ifndef AppVersion
  #error AppVersion must be given: ISCC /DAppVersion=0.1.0
#endif
#ifndef SourceDir
  #error SourceDir must be given: ISCC /DSourceDir=C:\path\to\stage
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
; Never change this id: it is how a new version knows it replaces the old one.
AppId={{6F1D7A52-8B3E-4C0A-9D64-2E5A1B7C93F8}
AppName=Telinhas
AppVersion={#AppVersion}
AppPublisher=Telinhas
DefaultDirName={autopf}\Telinhas
DefaultGroupName=Telinhas
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows Graphics Capture, which the app captures the screen with, needs 1903.
MinVersion=10.0.18362
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
OutputDir={#OutputDir}
OutputBaseFilename=Telinhas-Setup-{#AppVersion}
SetupIconFile={#SourcePath}\..\app\assets\telinhas.ico
UninstallDisplayIcon={app}\telinhas.exe
; An update runs while the app may still be open: close it and start it again.
CloseApplications=force
RestartApplications=no

[Languages]
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"

[Tasks]
Name: "desktopicon"; Description: "Criar um atalho na área de trabalho"; Flags: unchecked

[Files]
Source: "{#SourceDir}\telinhas.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourcePath}\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Telinhas"; Filename: "{app}\telinhas.exe"
Name: "{autodesktop}\Telinhas"; Filename: "{app}\telinhas.exe"; Tasks: desktopicon

[Run]
; No "skipifsilent": the silent update the app starts also opens it again.
Filename: "{app}\telinhas.exe"; Description: "Abrir o Telinhas"; Flags: nowait postinstall runasoriginaluser
