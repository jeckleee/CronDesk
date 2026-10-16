Unicode true
!include "MUI2.nsh"
!include "x64.nsh"

!ifndef VERSION
  !error "Pass /DVERSION=<Cargo package version> to makensis"
!endif
!ifndef APP_EXE
  !define APP_EXE "../target/x86_64-pc-windows-msvc/release/crondesk.exe"
!endif
!ifndef OUTPUT_FILE
  !define OUTPUT_FILE "../dist/CronDesk-${VERSION}-windows-x64-setup.exe"
!endif
!ifndef ICON_FILE
  !define ICON_FILE "../assets/CronDesk.ico"
!endif

Name "CronDesk"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\CronDesk"
InstallDirRegKey HKCU "Software\CronDesk" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ShowInstDetails show
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "CronDesk"
VIAddVersionKey "FileDescription" "CronDesk Setup"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "CronDesk"

Var AppLock

!define MUI_ABORTWARNING
!define MUI_ICON "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"
!define MUI_WELCOMEPAGE_TEXT "安装或升级 CronDesk ${VERSION}。$\r$\n$\r$\n新版支持一键清空日志，删除任务或服务时同步清理相关日志。升级安装保留现有数据。$\r$\n$\r$\n请先通过系统托盘退出旧版，再继续安装。从 0.1.0 首次升级时会自动备份旧版数据库。"
!define MUI_FINISHPAGE_RUN "$INSTDIR\CronDesk.exe"
!define MUI_FINISHPAGE_RUN_TEXT "启动 CronDesk"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "CronDesk 需要 64 位 Windows。"
    Abort
  ${EndIf}
  StrCpy $AppLock 0
  IfFileExists "$LOCALAPPDATA\CronDesk\instance.lock" check_app_lock app_not_running
  check_app_lock:
    System::Call 'kernel32::CreateFileW(w "$LOCALAPPDATA\CronDesk\instance.lock", i 0xC0000000, i 0, p 0, i 3, i 0, p 0) p.r0'
    StrCmp $0 -1 app_is_running
    StrCpy $AppLock $0
    Goto app_not_running
  app_is_running:
    IfSilent 0 +3
      SetErrorLevel 2
      Quit
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "CronDesk 正在运行，或数据目录暂时无法访问。请先通过系统托盘退出旧版，再点击重试。原有任务和日志不会被删除。" IDRETRY check_app_lock
    Abort
  app_not_running:
FunctionEnd

Function ReleaseAppLock
  StrCmp $AppLock 0 lock_released
  System::Call 'kernel32::CloseHandle(p $AppLock)'
  StrCpy $AppLock 0
  lock_released:
FunctionEnd

Function .onInstSuccess
  Call ReleaseAppLock
FunctionEnd

Function .onGUIEnd
  Call ReleaseAppLock
FunctionEnd

Section "CronDesk" Main
  SetOutPath "$INSTDIR"
  File /oname=CronDesk.exe "${APP_EXE}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\CronDesk"
  CreateShortcut "$SMPROGRAMS\CronDesk\CronDesk.lnk" "$INSTDIR\CronDesk.exe"
  CreateShortcut "$DESKTOP\CronDesk.lnk" "$INSTDIR\CronDesk.exe"
  WriteRegStr HKCU "Software\CronDesk" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CronDesk" "DisplayName" "CronDesk"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CronDesk" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CronDesk" "DisplayIcon" "$INSTDIR\CronDesk.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CronDesk" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CronDesk" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CronDesk" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CronDesk" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\CronDesk.exe"
  IfFileExists "$INSTDIR\CronDesk.exe" 0 +3
    MessageBox MB_ICONSTOP "请先通过托盘退出 CronDesk，再重新卸载。"
    Abort
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$DESKTOP\CronDesk.lnk"
  Delete "$SMPROGRAMS\CronDesk\CronDesk.lnk"
  RMDir "$SMPROGRAMS\CronDesk"
  DeleteRegKey HKCU "Software\CronDesk"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CronDesk"
  ; User tasks and execution logs under LocalAppData\CronDesk are retained.
SectionEnd
