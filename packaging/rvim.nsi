; NSIS installer for RustVim 1.1 (M8 packaging).
; Build:
;   cargo build --release --offline
;   makensis packaging\rvim.nsi
; The runtime tree is optional; omit the "File /r" runtime line for a single-exe install.

!define APPNAME "RustVim"
!define VERSION "1.1.0"
!define PUBLISHER "RustVim"
!define EXE "rvim.exe"

Name "${APPNAME} ${VERSION}"
OutFile "..\target\dist\${APPNAME}-${VERSION}-installer.exe"
Icon "..\assets\icon\rvim.ico"
UninstallIcon "..\assets\icon\rvim.ico"
InstallDir "$PROGRAMFILES64\${APPNAME}"
RequestExecutionLevel admin
Unicode true

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "RustVim (required)"
  SetOutPath "$INSTDIR"
  File "..\target\release\${EXE}"
  File "..\README.md"
  File "..\LICENSE"
  ; Merged runtime: comment this line out for a single-exe install.
  File /r "..\reference\vim\runtime"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateDirectory "$SMPROGRAMS\${APPNAME}"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\RustVim.lnk" "$INSTDIR\${EXE}"
  WriteRegStr HKLM "Software\${APPNAME}" "InstallDir" "$INSTDIR"
  ; Add/Remove Programs entry, including the embedded app icon.
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" "DisplayName" "${APPNAME} ${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" "DisplayIcon" "$INSTDIR\${EXE},0"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" "InstallLocation" "$INSTDIR"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\${EXE}"
  RMDir /r "$INSTDIR\runtime"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$SMPROGRAMS\${APPNAME}\RustVim.lnk"
  RMDir "$SMPROGRAMS\${APPNAME}"
  DeleteRegKey HKLM "Software\${APPNAME}"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}"
  RMDir "$INSTDIR"
SectionEnd
