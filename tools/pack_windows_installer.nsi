; StarQuake Windows installer (NSIS)
Unicode true
RequestExecutionLevel admin
ManifestDPIAware true

!ifndef VERSION
  !define VERSION "Beta_2.5.1"
!endif
!ifndef DISPLAY_VERSION
  !define DISPLAY_VERSION "Beta 2.5.1"
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

InstallDir "$PROGRAMFILES64\StarQuake"
InstallDirRegKey HKLM "Software\StarQuake" "InstallDir"
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!include "FileFunc.nsh"
!insertmacro GetParameters
!insertmacro GetOptions

Var IsUpdate

Function .onInit
  StrCpy $IsUpdate "0"
  ${GetParameters} $R9
  ${GetOptions} $R9 "/UPDATE" $R8
  IfErrors +2 0
  StrCpy $IsUpdate "1"


  ReadRegStr $0 HKLM "Software\StarQuake" "InstallDir"


  StrCmp $0 "" init_done


  MessageBox MB_YESNO|MB_ICONQUESTION \
    "检测到 StarQuake 已安装在以下位置：$\r$\n$0$\r$\n$\r$\n是否卸载已安装的版本并继续安装？" \
    IDYES uninstall_old \
    IDNO cancel_install

uninstall_old:

  IfFileExists "$0\Uninstall.exe" 0 uninstall_missing


  ExecWait '"$0\Uninstall.exe" _?=$0' $1


  StrCmp $1 0 init_done uninstall_failed

uninstall_missing:
  MessageBox MB_OK|MB_ICONEXCLAMATION \
    "未找到旧版本的卸载程序，无法自动卸载。$\r$\n请手动卸载旧版本后再运行安装程序。"
  Abort

uninstall_failed:
  MessageBox MB_OK|MB_ICONEXCLAMATION \
    "旧版本卸载失败或未能正常完成，安装程序将退出。"
  Abort

cancel_install:
  Abort

init_done:
FunctionEnd

!define MUI_ABORTWARNING
!define MUI_ICON "${DIST_DIR}\icon\brand\app.ico"
!define MUI_UNICON "${DIST_DIR}\icon\brand\app.ico"


!define MUI_WELCOMEFINISHPAGE_BITMAP "${SPLASH_BMP}"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "${SPLASH_BMP}"
!define MUI_FINISHPAGE_RUN "$INSTDIR\starquake.exe"
!define MUI_FINISHPAGE_RUN_TEXT "安装完成后运行 StarQuake(&R)"

Function SkipIfUpdate
  StrCmp $IsUpdate "1" 0 +2
  Abort
FunctionEnd

!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfUpdate
!insertmacro MUI_PAGE_WELCOME
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfUpdate
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfUpdate
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

AutoCloseWindow true
ShowInstDetails show

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
