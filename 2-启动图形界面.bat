@echo off
chcp 65001 >nul
title 启动 皎月连保活守护
cd /d "%~dp0target\release"

if not exist "natpierce-gui.exe" (
  echo.
  echo [错误] 找不到 natpierce-gui.exe
  echo        请先编译: cargo build --release
  echo.
  pause
  exit /b 1
)

echo.
echo 正在启动图形界面...
echo （窗口可能出现在托盘，右键托盘图标可打开设置）
echo.

start "" "natpierce-gui.exe"
timeout /t 2 >nul
