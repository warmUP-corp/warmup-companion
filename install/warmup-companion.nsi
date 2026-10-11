; warmup-companion.nsi - NSIS installer for Warmup Companion (WarmupVkSvc).
;
; Interactive installs launch the mono_ui setup wizard
; (`warmup-companion.exe setup`). Silent /S keeps the section defaults below.
;
; Build (from the repo root):
;   makensis install\warmup-companion.nsi
; Override the binary path if needed:
;   makensis /DBIN=path\to\warmup-companion.exe install\warmup-companion.nsi
;
; Output: target\warmup-companion-setup.exe

Unicode true
SetCompressor /SOLID lzma
SetDatablockOptimize on

!define APPNAME     "Warmup Companion"
!define COMPANY     "warmUP"
; New-ReleaseArtifacts.ps1 passes /DAPPVERSION from Cargo.toml; the literal below
; is only a fallback for hand-run makensis (it goes stale -- don't trust it).
!ifndef APPVERSION
  !define APPVERSION "0.0.0"
!endif
!define SERVICE     "WarmupVkSvc"
!define WEBSITE     "https://www.warmup-gamelauncher.com"
; install.rs hardcodes this path (no spaces; sc.exe binPath breaks on quotes).
!define DATADIR     "C:\ProgramData\WarmupVk"

; Source paths. NSIS resolves File/LicenseData relative to *this script's own
; directory* (install\), not makensis's working directory, so the "." default
; only works if you run makensis from inside install\. New-ReleaseArtifacts.ps1
; always passes an absolute /DSRCROOT=... override so it works from any cwd.
!ifndef SRCROOT
  !define SRCROOT "."
!endif
!ifndef BIN
  !define BIN "${SRCROOT}\target\release\warmup-companion.exe"
!endif

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "Sections.nsh"

Var ModelChoice
Var ParakeetPresent
Var InteractiveUI

Name "${APPNAME} Setup"
BrandingText "warmUP"
OutFile "${SRCROOT}\target\warmup-companion-setup.exe"
; Must NOT be C:\Program Files\WarmupVk: `warmup-companion.exe install` treats
; that exact path as a legacy install and purges it (taskkill /F /IM ...), which
; would kill the install process itself. See install.rs remove_legacy_install_artifacts.
InstallDir "$PROGRAMFILES64\WarmupCompanion"
RequestExecutionLevel admin
ShowInstDetails hide
ShowUninstDetails hide

VIProductVersion "${APPVERSION}.0"
VIAddVersionKey "ProductName"     "${APPNAME}"
VIAddVersionKey "CompanyName"     "${COMPANY}"
VIAddVersionKey "FileDescription" "${APPNAME} Setup"
VIAddVersionKey "FileVersion"     "${APPVERSION}"
VIAddVersionKey "ProductVersion"  "${APPVERSION}"
VIAddVersionKey "LegalCopyright"  "${COMPANY}"

!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\WarmupCompanion"

!define MUI_ABORTWARNING
!define MUI_ICON "${SRCROOT}\assets\icon.ico"
!define MUI_UNICON "${SRCROOT}\assets\icon.ico"
!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_BITMAP "${SRCROOT}\assets\installer\header.bmp"
!define MUI_WELCOMEFINISHPAGE_BITMAP "${SRCROOT}\assets\installer\welcome.bmp"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "${SRCROOT}\assets\installer\welcome.bmp"
!define MUI_UNCONFIRMPAGE_TEXT_TOP "This removes the service, installer files, and optional offline voice models. Logs and diagnostics are kept under C:\ProgramData\WarmupVk so they remain available for troubleshooting."
!define MUI_UNFINISHPAGE_TITLE "${APPNAME} was removed"
!define MUI_UNFINISHPAGE_TEXT "The service and installed files were removed. Existing logs and diagnostics remain under C:\ProgramData\WarmupVk."

!insertmacro MUI_PAGE_INSTFILES

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH

!insertmacro MUI_LANGUAGE "English"

Section "!Warmup Companion service (required)" SEC_MAIN
  SectionIn RO
  SetRegView 64
  SetOutPath "$INSTDIR"
  File "${BIN}"
  File "${SRCROOT}\install\Get-WarmupSpeech.ps1"
  File "${SRCROOT}\install\Get-WarmupParakeet.ps1"
  File "${SRCROOT}\README.md"
  File "${SRCROOT}\PRIVACY.md"
  File "${SRCROOT}\SECURITY.md"
  File "${SRCROOT}\LICENSE"
  File "/oname=icon.ico" "${SRCROOT}\assets\icon.ico"

  SetOutPath "${DATADIR}"
  File "${SRCROOT}\README.md"
  File "${SRCROOT}\PRIVACY.md"
  File "${SRCROOT}\SECURITY.md"
  File "${SRCROOT}\LICENSE"
  SetOutPath "${DATADIR}\bin"
  File "/oname=icon.ico" "${SRCROOT}\assets\icon.ico"

  ${If} $InteractiveUI == "1"
    DetailPrint "Opening warmUP Companion Setup..."
    nsExec::Exec '"$INSTDIR\warmup-companion.exe" setup'
    Pop $0
    ${If} $0 == 2
      DetailPrint "Setup cancelled."
      Call RemovePayload
      Abort
    ${ElseIf} $0 != 0
      IfSilent +2
      MessageBox MB_ICONSTOP|MB_OK "Setup failed (exit $0). See ${DATADIR}\service.log." /SD IDOK
      Abort
    ${EndIf}
  ${Else}
    DetailPrint "Stopping stale ${APPNAME} processes..."
    nsExec::ExecToLog 'taskkill /F /IM warmup-companion.exe'
    Pop $0
    nsExec::ExecToLog 'taskkill /F /IM warmup-vk-prototype.exe'
    Pop $0

    DetailPrint "Installing the ${SERVICE} service..."
    nsExec::ExecToLog '"$INSTDIR\warmup-companion.exe" install'
    Pop $0
    ${If} $0 != 0
      IfSilent +2
      MessageBox MB_ICONSTOP|MB_OK "Service install failed (exit $0). See ${DATADIR}\service.log." /SD IDOK
      Abort
    ${EndIf}

    DetailPrint "Granting interactive start/stop on ${SERVICE}..."
    nsExec::ExecToLog '"$INSTDIR\warmup-companion.exe" grant-service-acl'
    Pop $0
    ${If} $0 != 0
      DetailPrint "WARNING: could not grant IU RP+WP on ${SERVICE} (exit $0)."
    ${EndIf}
  ${EndIf}

  SetShellVarContext all
  CreateShortCut "$SMPROGRAMS\Warmup Companion.lnk" "$INSTDIR\warmup-companion.exe" "wake" "$INSTDIR\icon.ico" 0 SW_SHOWMINIMIZED

  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr   HKLM "${UNINSTKEY}" "DisplayName"          "${APPNAME}"
  WriteRegStr   HKLM "${UNINSTKEY}" "DisplayVersion"       "${APPVERSION}"
  WriteRegStr   HKLM "${UNINSTKEY}" "Publisher"            "${COMPANY}"
  WriteRegStr   HKLM "${UNINSTKEY}" "URLInfoAbout"         "${WEBSITE}"
  WriteRegStr   HKLM "${UNINSTKEY}" "DisplayIcon"          "$INSTDIR\icon.ico"
  WriteRegStr   HKLM "${UNINSTKEY}" "InstallLocation"      "$INSTDIR"
  WriteRegStr   HKLM "${UNINSTKEY}" "UninstallString"      '"$INSTDIR\uninstall.exe"'
  WriteRegStr   HKLM "${UNINSTKEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINSTKEY}" "NoRepair" 1
SectionEnd

Section /o "Offline voice typing (recommended)" SEC_SPEECH
  ${If} $InteractiveUI == "1"
    Return
  ${EndIf}
  ${If} $ModelChoice == "parakeet"
    ${If} $ParakeetPresent == "1"
      DetailPrint "Parakeet voice typing is already installed."
    ${Else}
      DetailPrint "Downloading offline voice typing (Parakeet, ~670 MB)..."
      nsExec::ExecToLog '"powershell.exe" -NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File "$INSTDIR\Get-WarmupParakeet.ps1"'
      Pop $0
      ${If} $0 != 0
        IfSilent +2
        MessageBox MB_ICONEXCLAMATION "Parakeet voice typing could not be downloaded (exit $0). The app works fine without it. Add it later by running $INSTDIR\Get-WarmupParakeet.ps1."
      ${EndIf}
    ${EndIf}
  ${Else}
    ${If} $ModelChoice == ""
      StrCpy $ModelChoice "medium"
    ${EndIf}
    DetailPrint "Downloading offline voice typing (whisper '$ModelChoice' model)..."
    nsExec::ExecToLog '"powershell.exe" -NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File "$INSTDIR\Get-WarmupSpeech.ps1" -Model $ModelChoice'
    Pop $0
    ${If} $0 != 0
      IfSilent +2
      MessageBox MB_ICONEXCLAMATION "Voice typing could not be downloaded (exit $0). The app works fine without it. Add it later by running $INSTDIR\Get-WarmupSpeech.ps1, or drop whisper-server.exe + a ggml-*.bin into ${DATADIR}\speech."
    ${EndIf}
  ${EndIf}
SectionEnd

Function RemovePayload
  ${If} ${FileExists} "$INSTDIR\uninstall.exe"
    Return
  ${EndIf}
  Delete "$INSTDIR\warmup-companion.exe"
  Delete "$INSTDIR\Get-WarmupSpeech.ps1"
  Delete "$INSTDIR\Get-WarmupParakeet.ps1"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\PRIVACY.md"
  Delete "$INSTDIR\SECURITY.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\icon.ico"
  RMDir "$INSTDIR"
FunctionEnd

Function DetectParakeet
  StrCpy $ParakeetPresent "0"
  ${If} ${FileExists} "${DATADIR}\speech\parakeet\vocab.txt"
  ${AndIf} ${FileExists} "${DATADIR}\speech\parakeet\onnxruntime.dll"
    ${If} ${FileExists} "${DATADIR}\speech\parakeet\encoder-model.int8.onnx"
    ${OrIf} ${FileExists} "${DATADIR}\speech\parakeet\encoder-model.onnx"
      StrCpy $ParakeetPresent "1"
    ${EndIf}
  ${EndIf}
FunctionEnd

Function .onInit
  Call DetectParakeet
  StrCpy $ModelChoice "parakeet"
  SectionSetFlags ${SEC_SPEECH} ${SF_SELECTED}
  StrCpy $InteractiveUI "0"
  IfSilent silent_done
    StrCpy $InteractiveUI "1"
    SetSilent silent
  silent_done:
FunctionEnd

Section "Uninstall"
  SetRegView 64
  nsExec::ExecToLog '"$INSTDIR\warmup-companion.exe" uninstall'
  Pop $0
  ${If} $0 != 0
    IfSilent +2
    MessageBox MB_ICONSTOP|MB_OK "Warmup Companion could not be removed (exit $0). The service may still be running. See ${DATADIR}\service.log and try again." /SD IDOK
    Abort
  ${EndIf}

  SetShellVarContext all
  Delete "$SMPROGRAMS\Warmup Companion.lnk"
  Delete "$INSTDIR\warmup-companion.exe"
  Delete "$INSTDIR\Get-WarmupSpeech.ps1"
  Delete "$INSTDIR\Get-WarmupParakeet.ps1"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\PRIVACY.md"
  Delete "$INSTDIR\SECURITY.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\icon.ico"
  Delete "$INSTDIR\uninstall.exe"
  RMDir  "$INSTDIR"

  DeleteRegKey HKLM "${UNINSTKEY}"
  RMDir /r "${DATADIR}\speech"
  Delete "${DATADIR}\bin\icon.ico"
  Delete "${DATADIR}\bin\warmup-companion.version"
  RMDir  "${DATADIR}\bin"
SectionEnd
