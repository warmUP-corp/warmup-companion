; warmup-companion.nsi - NSIS installer for Warmup Companion (WarmupVkSvc).
;
; The heavy lifting (copying the service binary to C:\ProgramData\WarmupVk\bin
; and creating + starting the WarmupVkSvc service) is done by the binary itself
; via `warmup-companion.exe install` / `uninstall`. This installer just lays the
; binary and trust docs down, runs those subcommands, and registers an
; Add/Remove Programs entry. Supports silent install/uninstall with /S.
;
; Build (from the repo root):
;   makensis install\warmup-companion.nsi
; Override the binary path if needed:
;   makensis /DBIN=path\to\warmup-companion.exe install\warmup-companion.nsi
;
; Output: target\warmup-companion-setup.exe

Unicode true

!define APPNAME     "Warmup Companion"
!define COMPANY     "warmUP"
!define APPVERSION  "0.0.1"
!define SERVICE     "WarmupVkSvc"
!define WEBSITE     "https://www.warmup-gamelauncher.com"
; install.rs hardcodes this path (no spaces; sc.exe binPath breaks on quotes).
!define DATADIR     "C:\ProgramData\WarmupVk"

; Source paths. NSIS resolves File/OutFile relative to makensis's working
; directory, so the default assumes makensis is run from the repo root.
; Override with an absolute path via /DSRCROOT=... to run from anywhere.
!ifndef SRCROOT
  !define SRCROOT "."
!endif
!ifndef BIN
  !define BIN "${SRCROOT}\target\release\warmup-companion.exe"
!endif

!include "MUI2.nsh"
!include "LogicLib.nsh"

Name "${APPNAME}"
OutFile "${SRCROOT}\target\warmup-companion-setup.exe"
; Must NOT be C:\Program Files\WarmupVk: `warmup-companion.exe install` treats
; that exact path as a legacy install and purges it (taskkill /F /IM ...), which
; would kill the install process itself. See install.rs remove_legacy_install_artifacts.
InstallDir "$PROGRAMFILES64\WarmupCompanion"
RequestExecutionLevel admin       ; service install needs admin; elevate the whole installer
ShowInstDetails show
ShowUninstDetails show

VIProductVersion "0.0.1.0"
VIAddVersionKey "ProductName"     "${APPNAME}"
VIAddVersionKey "CompanyName"     "${COMPANY}"
VIAddVersionKey "FileDescription" "${APPNAME} Setup"
VIAddVersionKey "FileVersion"     "${APPVERSION}"
VIAddVersionKey "ProductVersion"  "${APPVERSION}"
VIAddVersionKey "LegalCopyright"  "${COMPANY}"

!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\WarmupCompanion"

!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${SRCROOT}\LICENSE"
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_LINK "warmUP Game Launcher - ${WEBSITE}"
!define MUI_FINISHPAGE_LINK_LOCATION "${WEBSITE}"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Install"
  SetRegView 64                   ; write the uninstall key to the native 64-bit hive
  ; Keep a copy of the binary + trust docs in the app dir.
  SetOutPath "$INSTDIR"
  File "${BIN}"
  File "${SRCROOT}\README.md"
  File "${SRCROOT}\PRIVACY.md"
  File "${SRCROOT}\SECURITY.md"
  File "${SRCROOT}\LICENSE"

  ; Trust docs also live next to the data dir (parity with Install-WarmupVk.ps1).
  SetOutPath "${DATADIR}"
  File "${SRCROOT}\README.md"
  File "${SRCROOT}\PRIVACY.md"
  File "${SRCROOT}\SECURITY.md"
  File "${SRCROOT}\LICENSE"

  ; Register + start the service. This self-copies the exe to
  ; C:\ProgramData\WarmupVk\bin and creates WarmupVkSvc (LocalSystem, auto-start).
  DetailPrint "Installing the ${SERVICE} service..."
  nsExec::ExecToLog '"$INSTDIR\warmup-companion.exe" install'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "Service install failed (exit $0). See ${DATADIR}\service.log."
    Abort "Service install failed."
  ${EndIf}

  ; Add/Remove Programs entry + uninstaller.
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr   HKLM "${UNINSTKEY}" "DisplayName"          "${APPNAME}"
  WriteRegStr   HKLM "${UNINSTKEY}" "DisplayVersion"       "${APPVERSION}"
  WriteRegStr   HKLM "${UNINSTKEY}" "Publisher"            "${COMPANY}"
  WriteRegStr   HKLM "${UNINSTKEY}" "URLInfoAbout"         "${WEBSITE}"
  WriteRegStr   HKLM "${UNINSTKEY}" "InstallLocation"      "$INSTDIR"
  WriteRegStr   HKLM "${UNINSTKEY}" "UninstallString"      '"$INSTDIR\uninstall.exe"'
  WriteRegStr   HKLM "${UNINSTKEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINSTKEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetRegView 64                   ; match the install section's hive
  ; Stop + delete the service and remove the ProgramData service binary.
  nsExec::ExecToLog '"$INSTDIR\warmup-companion.exe" uninstall'
  Pop $0

  Delete "$INSTDIR\warmup-companion.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\PRIVACY.md"
  Delete "$INSTDIR\SECURITY.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\uninstall.exe"
  RMDir  "$INSTDIR"

  DeleteRegKey HKLM "${UNINSTKEY}"
  ; Logs and the local dictionary under ${DATADIR} are intentionally left in
  ; place, matching `warmup-companion.exe uninstall`.
SectionEnd
