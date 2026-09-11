@echo off
chcp 65001 >nul
title 皎月连保活 - 状态查看
cd /d "%~dp0"

echo.
echo ============================================================
echo   皎月连保活 - 状态查看
echo ============================================================
echo.

if not exist "target\release\natpierce-keepalived.exe" (
  echo [错误] 找不到可执行文件，请先编译: cargo build --release
  echo.
  pause
  exit /b 1
)

echo [1] 查看状态
echo [2] 列出在线主机
echo [3] 尝试开启服务端（ensure）
echo [4] 查看日志（最近 30 行）
echo [5] 打开配置目录
echo [0] 退出
echo.
set /p choice=请选择:

if "%choice%"=="1" target\release\natpierce-keepalived.exe status
if "%choice%"=="2" target\release\natpierce-keepalived.exe hosts
if "%choice%"=="3" target\release\natpierce-keepalived.exe ensure
if "%choice%"=="4" (
  for /f "delims=" %%f in ('dir /b /o-d "logs\keepalive.log.*" 2^>nul') do (
    powershell -NoProfile -Command "Get-Content 'logs\%%f' -Tail 30 -Encoding UTF8"
    goto :done
  )
  echo 没有找到日志文件
)
if "%choice%"=="5" start "" explorer.exe "%CD%"
if "%choice%"=="0" exit /b 0

:done
echo.
pause
