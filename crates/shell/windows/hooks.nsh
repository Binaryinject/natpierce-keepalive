; ---------------------------------------------------------------------------
; Tauri NSIS 安装钩子
;
; 作用：把守护进程 natpierce-keepalived.exe 一并安装到 $INSTDIR，
;       使安装包成为一个完整可用的产品（否则用户只装到界面程序，
;       保活会因为找不到守护进程而失效）。
;
; 前置条件：构建前需把 natpierce-keepalived.exe 复制到本文件同目录
;           （CI 里由 workflow 完成；本地可用 scripts\prepare-bundle.ps1）。
;           这里用 ${__FILEDIR__} 定位，避免相对路径随 NSIS 工作目录漂移。
; ---------------------------------------------------------------------------

!macro NSIS_HOOK_POSTINSTALL
  ; 安装完成后，把守护进程放到主程序旁边
  File /oname=$INSTDIR\natpierce-keepalived.exe "${__FILEDIR__}\natpierce-keepalived.exe"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; 卸载时移除守护进程（Tauri 只会清理它自己记录的文件）
  Delete "$INSTDIR\natpierce-keepalived.exe"
!macroend
