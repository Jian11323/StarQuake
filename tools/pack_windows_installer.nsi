; StarQuake Windows installer (NSIS)
Unicode true
RequestExecutionLevel admin

!ifndef VERSION
  !define VERSION "Beta_2.3.4"
!endif
!ifndef DISPLAY_VERSION
  !define DISPLAY_VERSION "Beta 2.3.4"
!endif
!ifndef DIST_DIR
  !define DIST_DIR "..\dist\StarQuake"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "..\dist\StarQuake_${VERSION}_setup.exe"
!endif
!ifndef SPLASH_BMP
  !define SPLASH_BMP "${DIST_DIR}\icon\brand\splash_installer.bmp"
!endif

Name "StarQuake ${DISPLAY_VERSION}"
OutFile "${OUT_FILE}"
InstallDir "$PROGRAMFILES32\StarQuake"
InstallDirRegKey HKLM "Software\StarQuake" "InstallDir"
SetCompressor /SOLID lzma

!include "MUI2.nsh"

!define MUI_ABORTWARNING
!define MUI_ICON "${DIST_DIR}\icon\brand\app.ico"
!define MUI_UNICON "${DIST_DIR}\icon\brand\app.ico"

; Welcome/finish sidebar bitmap (generated from splash_bg.png at pack time)
!define MUI_WELCOMEFINISHPAGE_BITMAP "${SPLASH_BMP}"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "${SPLASH_BMP}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

Section "StarQuake" SecMain
  SetOutPath "$INSTDIR"
  File /r "${DIST_DIR}\*.*"

  WriteRegStr HKLM "Software\StarQuake" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarQuake" \
    "DisplayName" "StarQuake"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarQuake" \
    "DisplayVersion" "${DISPLAY_VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarQuake" \
    "Publisher" "Jian11323"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarQuake" \
    "DisplayIcon" "$INSTDIR\starquake.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarQuake" \
    "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarQuake" \
    "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarQuake" \
    "NoRepair" 1

  CreateDirectory "$SMPROGRAMS\StarQuake"
  CreateShortCut "$SMPROGRAMS\StarQuake\StarQuake.lnk" "$INSTDIR\starquake.exe" "" "$INSTDIR\icon\brand\app.ico" 0 SW_SHOWNORMAL "" "$INSTDIR"
  CreateShortCut "$DESKTOP\StarQuake.lnk" "$INSTDIR\starquake.exe" "" "$INSTDIR\icon\brand\app.ico" 0 SW_SHOWNORMAL "" "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$DESKTOP\StarQuake.lnk"
  Delete "$SMPROGRAMS\StarQuake\StarQuake.lnk"
  RMDir "$SMPROGRAMS\StarQuake"
  Delete "$DESKTOP\EEW View.lnk"
  Delete "$SMPROGRAMS\EEW View\EEW View.lnk"
  RMDir "$SMPROGRAMS\EEW View"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarQuake"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\EEWView"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarQuake"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\EEWView"
  DeleteRegKey HKLM "Software\StarQuake"
  DeleteRegKey HKLM "Software\eew view"
  RMDir /r "$INSTDIR"
SectionEnd
