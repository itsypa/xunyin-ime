; 灵犀输入法 NSIS 安装/卸载钩子（Tauri installerHooks）
; 布局：
;   $INSTDIR\lingxi-ime.exe            设置中心（依赖旁路 rime.dll）
;   $INSTDIR\rime.dll                  x64 rime（打包根资源）
;   $INSTDIR\rime\data\...             共享词库
;   $INSTDIR\tsf\lingxi_tsf_x64.dll    x64 TSF（依赖 $INSTDIR\tsf\rime.dll）
;   $INSTDIR\tsf-x86\lingxi_tsf_x86.dll + rime.dll（x86 TSF 及其 32 位 rime）

!macro NSIS_HOOK_POSTINSTALL
  ; x64 TSF DLL 需要与它同目录的 rime.dll
  CopyFiles /SILENT "$INSTDIR\rime.dll" "$INSTDIR\tsf\rime.dll"
  ; 注册 64 位视图（System32\regsvr32.exe 是 64 位）
  ExecWait '"$SYSDIR\regsvr32.exe" /s "$INSTDIR\tsf\lingxi_tsf_x64.dll"'
  ; 注册 32 位视图（SysWOW64\regsvr32.exe 是 32 位，DLL 自带 32 位 rime.dll）
  IfFileExists "$WINDIR\SysWOW64\regsvr32.exe" 0 +3
  ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /s "$INSTDIR\tsf-x86\lingxi_tsf_x86.dll"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$SYSDIR\regsvr32.exe" /s /u "$INSTDIR\tsf\lingxi_tsf_x64.dll"'
  IfFileExists "$WINDIR\SysWOW64\regsvr32.exe" 0 +3
  ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /s /u "$INSTDIR\tsf-x86\lingxi_tsf_x86.dll"'
!macroend
