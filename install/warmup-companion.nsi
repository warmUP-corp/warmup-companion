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
!include "nsDialogs.nsh"
!include "Sections.nsh"

Var ModelChoice
Var RbTiny
Var RbBase
Var RbSmall
Var RbMedium

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
!insertmacro MUI_PAGE_COMPONENTS
Page custom ModelPageShow ModelPageLeave
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_LINK "warmUP Game Launcher - ${WEBSITE}"
!define MUI_FINISHPAGE_LINK_LOCATION "${WEBSITE}"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "!Warmup Companion service (required)" SEC_MAIN
  SectionIn RO                    ; always installed; user can't uncheck the service
  SetRegView 64                   ; write the uninstall key to the native 64-bit hive
  ; Keep a copy of the binary + trust docs in the app dir.
  SetOutPath "$INSTDIR"
  File "${BIN}"
  File "${SRCROOT}\install\Get-WarmupSpeech.ps1"
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

Section /o "Offline voice typing (whisper)" SEC_SPEECH
  ; Opt-in, unchecked by default. Downloads the whisper.cpp runner + a model into
  ; ${DATADIR}\speech; the Mic key on the on-screen keyboard appears only once
  ; those exist (src\win\speech_input.rs::available). Recognition is fully local.
  ${If} $ModelChoice == ""
    StrCpy $ModelChoice "medium"
  ${EndIf}
  DetailPrint "Downloading offline voice typing (whisper '$ModelChoice' model)..."
  nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\Get-WarmupSpeech.ps1" -Model $ModelChoice'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_ICONEXCLAMATION "Voice typing could not be downloaded (exit $0). The app works fine without it. Add it later by running $INSTDIR\Get-WarmupSpeech.ps1, or drop whisper-server.exe + a ggml-*.bin into ${DATADIR}\speech."
  ${EndIf}
SectionEnd

LangString DESC_MAIN   ${LANG_ENGLISH} "The Warmup Companion service (sign-in / lock / UAC gamepad keyboard). Required."
LangString DESC_SPEECH ${LANG_ENGLISH} "Optional, fully offline voice typing. Downloads the whisper.cpp engine + a speech model (~150 MB) to ${DATADIR}\speech. The on-screen Mic key stays hidden unless this is installed. No cloud; recognition runs locally."
!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_MAIN}   $(DESC_MAIN)
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_SPEECH} $(DESC_SPEECH)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; Custom page: pick the whisper model — shown only if voice typing is selected.
Function ModelPageShow
  SectionGetFlags ${SEC_SPEECH} $0
  IntOp $0 $0 & ${SF_SELECTED}
  ${If} $0 == 0
    Abort                          ; voice typing not chosen — skip this page
  ${EndIf}

  !insertmacro MUI_HEADER_TEXT "Voice model" "Choose the offline speech model to download."
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}

  ${NSD_CreateLabel} 0 0 100% 22u "Bigger models are more accurate but slower and larger to download. All run fully offline."
  Pop $1
  ${NSD_CreateRadioButton} 8u 28u 95% 12u "Tiny  -  ~75 MB, fastest"
  Pop $RbTiny
  ${NSD_CreateRadioButton} 8u 42u 95% 12u "Base  -  ~142 MB"
  Pop $RbBase
  ${NSD_CreateRadioButton} 8u 56u 95% 12u "Small  -  ~466 MB, better accuracy"
  Pop $RbSmall
  ${NSD_CreateRadioButton} 8u 70u 95% 12u "Medium  -  ~1.5 GB, best accuracy (recommended)"
  Pop $RbMedium

  ${If} $ModelChoice == "tiny"
    ${NSD_Check} $RbTiny
  ${ElseIf} $ModelChoice == "base"
    ${NSD_Check} $RbBase
  ${ElseIf} $ModelChoice == "small"
    ${NSD_Check} $RbSmall
  ${Else}
    ${NSD_Check} $RbMedium
  ${EndIf}
  nsDialogs::Show
FunctionEnd

Function ModelPageLeave
  ${NSD_GetState} $RbTiny $0
  ${If} $0 == ${BST_CHECKED}
    StrCpy $ModelChoice "tiny"
    Return
  ${EndIf}
  ${NSD_GetState} $RbSmall $0
  ${If} $0 == ${BST_CHECKED}
    StrCpy $ModelChoice "small"
    Return
  ${EndIf}
  ${NSD_GetState} $RbMedium $0
  ${If} $0 == ${BST_CHECKED}
    StrCpy $ModelChoice "medium"
    Return
  ${EndIf}
  StrCpy $ModelChoice "base"
FunctionEnd

Section "Uninstall"
  SetRegView 64                   ; match the install section's hive
  ; Stop + delete the service and remove the ProgramData service binary.
  nsExec::ExecToLog '"$INSTDIR\warmup-companion.exe" uninstall'
  Pop $0

  Delete "$INSTDIR\warmup-companion.exe"
  Delete "$INSTDIR\Get-WarmupSpeech.ps1"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\PRIVACY.md"
  Delete "$INSTDIR\SECURITY.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\uninstall.exe"
  RMDir  "$INSTDIR"

  DeleteRegKey HKLM "${UNINSTKEY}"
  ; Downloaded voice-typing engine + model are not user data — remove them.
  RMDir /r "${DATADIR}\speech"
  ; Logs and the local dictionary under ${DATADIR} are intentionally left in
  ; place, matching `warmup-companion.exe uninstall`.
SectionEnd
