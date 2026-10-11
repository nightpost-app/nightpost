; The Windows installer: makensis -DVERSION=x.y.z -DDIST=<folder> -DOUT=<file>
; nightpost.nsi, with in DIST the files of the zip. Installs for the current
; user only (no administrator prompt), where the zip's users were told to
; unzip it. Messages and keys stay unless asked for at uninstall.

Unicode true
!include "MUI2.nsh"

!define NAME "Nightpost"
!define APP_ID "io.github.nightpost-app.Nightpost"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${NAME}"

Name "${NAME}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\${NAME}"
InstallDirRegKey HKCU "${UNINSTALL_KEY}" "InstallLocation"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText " "

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${NAME}"
VIAddVersionKey "FileDescription" "${NAME} installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "AGPL-3.0-only"

!define MUI_ICON "..\..\icons\nightpost.ico"
!define MUI_UNICON "..\..\icons\nightpost.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\nightpost-gui.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open Nightpost"
!define MUI_UNCOMPONENTSPAGE_TEXT_TOP "Your messages and keys are kept unless you tick the box below. Without them your contacts cannot be reached again from this computer."

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_COMPONENTS
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; A running Nightpost holds its files: the window and the daemon both go
; (the window starts the daemon again when opened).
!macro Close
  nsExec::Exec 'taskkill /im nightpost-gui.exe /f'
  nsExec::Exec 'taskkill /im nightpost.exe /f'
  nsExec::Exec 'taskkill /im lyrebird.exe /f'
  Sleep 1000
!macroend

Section "Nightpost"
  SectionIn RO
  !insertmacro Close
  SetOutPath "$INSTDIR"
  File "${DIST}\nightpost-gui.exe"
  File "${DIST}\nightpost.exe"
  File "${DIST}\lyrebird.exe"
  File "${DIST}\WebView2Loader.dll"
  File "${DIST}\LICENSE.txt"
  File "${DIST}\LICENSE-lyrebird.txt"
  File "${DIST}\BRIDGES.md"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortcut "$SMPROGRAMS\${NAME}.lnk" "$INSTDIR\nightpost-gui.exe"

  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\nightpost-gui.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "URLInfoAbout" "https://github.com/nightpost-app/nightpost"
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  SectionGetSize 0 $0
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" $0
SectionEnd

Section "un.Nightpost" UnProgram
  SectionIn RO
  !insertmacro Close
  Delete "$SMPROGRAMS\${NAME}.lnk"
  RMDir /r "$INSTDIR"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  ; Written by Nightpost itself, for the name on its notifications.
  DeleteRegKey HKCU "Software\Classes\AppUserModelId\${APP_ID}"
SectionEnd

Section /o "un.Also delete my messages and keys" UnData
  RMDir /r "$APPDATA\${NAME}"
  ; The database key in Windows' Credential Manager: one entry per data
  ; folder, named <8 bytes of its hash in hex>.nightpost.
  nsExec::Exec `powershell -NoProfile -Command "cmdkey /list | Select-String 'target=([0-9a-f]{16}\.nightpost)$$' | ForEach-Object { cmdkey /delete:$$($$_.Matches[0].Groups[1].Value) }"`
SectionEnd
