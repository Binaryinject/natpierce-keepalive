@echo off
chcp 65001 >nul
title 可执行文件 - 皎月连保活守护
cd /d "%~dp0target\release"
echo.
echo ============================================================
echo   可执行文件目录
echo   %CD%
echo ============================================================
echo.
echo   [1] natpierce-gui.exe         图形界面（托盘 + 设置窗口）
echo   [2] natpierce-keepalived.exe  守护进程 / 命令行
echo.
echo ------------------------------------------------------------
echo   即将打开资源管理器，请稍候...
echo ------------------------------------------------------------
echo.
start "" explorer.exe "%CD%"
timeout /t 2 >nul
