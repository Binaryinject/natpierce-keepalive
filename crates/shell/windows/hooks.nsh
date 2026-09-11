; ---------------------------------------------------------------------------
; Tauri NSIS 安装钩子
;
; 作用：把守护进程 natpierce-keepalived.exe 一并安装到 $INSTDIR，
;       让安装包成为完整可用的产品（否则只有界面程序，保活无法工作）。
;
; ⚠️ 路径说明（踩过坑）
;   `${__FILEDIR__}` 在本钩子里**不是** hooks 文件所在目录，
;   而是 NSIS 脚本的生成目录：<target>\release\nsis\x64\
;   从那里回退两级即 <target>\release\，守护进程正好在那里。
;   曾误以为它是 hooks 目录，导致：
;     File: "...\nsis\x64\natpierce-keepalived.exe" -> no files found.
;
; 所以构建前无需把 exe 复制到任何特殊位置，只要 cargo build --release
; 产出 target\release\natpierce-keepalived.exe 即可。
; ---------------------------------------------------------------------------

!macro NSIS_HOOK_POSTINSTALL
  ; 安装守护进程到主程序目录
  ; SetOutPath 切换输出目录，File 里只写文件名（/oname 不接受带路径的名字）
  SetOutPath "$INSTDIR"
  File "${__FILEDIR__}\..\..\natpierce-keepalived.exe"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; 卸载时清理守护进程（Tauri 只清理它自己记录的文件）
  Delete "$INSTDIR\natpierce-keepalived.exe"
!macroend
