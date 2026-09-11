# 打包前准备：把守护进程放到 NSIS hooks 能找到的位置
#
# 背景：Tauri 的 NSIS installerHooks 里 `File` 指令在**编译期**读取文件，
#       hooks.nsh 用 ${__FILEDIR__} 定位到 crates\shell\windows\，
#       所以打包前必须先把 natpierce-keepalived.exe 复制过去。
#
# 用法：
#   cargo build --release          # 先编译
#   .\scripts\prepare-bundle.ps1   # 再准备
#   cd crates\shell; cargo tauri bundle --bundles nsis

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$src  = Join-Path $root 'target\release\natpierce-keepalived.exe'
$dst  = Join-Path $root 'crates\shell\windows\natpierce-keepalived.exe'

if (-not (Test-Path $src)) {
    Write-Host "找不到 $src" -ForegroundColor Red
    Write-Host "请先执行: cargo build --release" -ForegroundColor Yellow
    exit 1
}

$dstDir = Split-Path -Parent $dst
if (-not (Test-Path $dstDir)) { New-Item -ItemType Directory -Force -Path $dstDir | Out-Null }

Copy-Item $src $dst -Force
$size = [math]::Round((Get-Item $dst).Length / 1MB, 2)
Write-Host "已就位: $dst ($size MB)" -ForegroundColor Green
Write-Host ""
Write-Host "接下来：" -ForegroundColor Cyan
Write-Host "  cd crates\shell"
Write-Host "  cargo tauri bundle --bundles nsis"
