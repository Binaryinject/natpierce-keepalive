# 皎月连保活守护 (natpierce-keepalive)

[![Release](https://github.com/Binaryinject/natpierce-keepalive/actions/workflows/release.yml/badge.svg)](https://github.com/Binaryinject/natpierce-keepalive/actions/workflows/release.yml)
![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-blue)
![License](https://img.shields.io/badge/license-MIT-green)

皎月连（natpierce）的**连接保活守护** —— 服务端 / 客户端双模式，图形界面 + 独立守护进程。

> 解决"皎月连服务端断了不会自动重连"、"客户端连接掉了要手动点"这类问题。

---

## 功能特性

| 特性 | 说明 |
|---|---|
| **双模式** | 服务端（保活本机 `startServer`）/ 客户端（保活与远端主机的连接） |
| **三层判据** | 进程层 → 服务层 → 心跳层，逐级定位并恢复 |
| **分层恢复** | 服务停了发 `startServer`；连接断了重连；软件卡死重启进程 |
| **自动开启** | 配置齐全时打开界面即自动保活，无需点任何按钮 |
| **配置热重载** | 界面里改完配置立即生效，守护进程无需重启 |
| **优雅停止** | 通过停止标志文件退出，与权限无关，不依赖强杀 |
| **现代界面** | Tauri 2 + Web UI（HTML/CSS/JS），深色主题 |
| **密码加密** | 页面/连接密码用 **Windows DPAPI** 加密，不落明文 |
| **崩溃自愈** | 支持注册为 Windows 服务，由 SCM 托管并自动重启 |
| **防重启风暴** | 连续失败阈值 + 冷却时间，避免网络抖动时反复重启 |

---

## 架构

```
┌─ natpierce-gui.exe（Tauri 2 + WebView2）──────────┐
│  · 系统托盘（Tauri 内置）                          │
│  · Web UI 设置界面                                 │
│  · 配置校验、状态展示（后端事件推送，非轮询）        │
└────────────────────────────────────────────────────┘
                     │ 启动 / 优雅停止
                     ▼
┌─ natpierce-keepalived.exe（纯后台守护）────────────┐
│  · 保活状态机（三层判据 + 分层恢复）                │
│  · 配置热重载                                      │
│  · 进程检测 / 提权启动 / 终止                       │
└────────────────────────────────────────────────────┘
                     │ 本地控制接口
                     ▼
         ws://127.0.0.1:33272/ws  →  皎月连本体
```

**为什么拆成两个进程**：早期把保活逻辑塞在界面进程里，UI 线程会被网络探测和进程管理阻塞，导致界面严重卡顿。拆分后界面只做渲染，永远不会被阻塞。

---

## 快速开始

### 方式一：下载安装包（推荐）

到 [Releases](https://github.com/Binaryinject/natpierce-keepalive/releases) 下载最新版：

- `natpierce-keepalive_x.x.x_x64-setup.exe` —— NSIS 安装包

### 方式二：从源码构建

需要 [Rust](https://rustup.rs/) 1.75+ 与 MSVC 工具链：

```powershell
git clone https://github.com/Binaryinject/natpierce-keepalive.git
cd natpierce-keepalive
cargo build --release
```

产物在 `target\release\`：

| 文件 | 说明 |
|---|---|
| `natpierce-gui.exe` | 图形界面（托盘 + 设置窗口） |
| `natpierce-keepalived.exe` | 守护进程 / 命令行工具 |

---

## 使用步骤

### 1. 启动界面

双击 `natpierce-gui.exe`（或 `2-启动图形界面.bat`）。

### 2. 完成配置（首次必需）

界面顶部会弹出**红色警告框**，逐条列出缺什么：

- **natpierce.exe 路径** —— 点「浏览…」选择你的皎月连主程序
  - 会自动填好「进程名」和「工作目录」
- **页面访问密码** —— 皎月连界面里的「页面访问密码」（6-20 位，组网模式下必填）

填完点底部「**保存配置**」，红框消失。

### 3. 自动保活

**配置齐全后无需任何操作** —— 保活会自动开启：

```
打开界面 → 自动校验配置 → 自动拉起守护进程 → 自动检测并启动皎月连
```

界面状态会在几秒内自动变绿（后端事件推送，不需要点刷新）。

---

## 命令行工具

`natpierce-keepalived.exe` 也可独立使用：

```powershell
natpierce-keepalived status          # 查看状态（进程 / 服务端 / 在线主机）
natpierce-keepalived hosts           # 列出在线主机及识别码
natpierce-keepalived ensure          # 若服务端未启动则开启它
natpierce-keepalived run             # 前台运行保活循环（可看实时日志）
natpierce-keepalived set-password page   # 录入页面密码（DPAPI 加密）
natpierce-keepalived autostart on    # 开机自启
natpierce-keepalived service install # 安装为 Windows 服务（需管理员）
```

常用选项：

```
-m, --mode <模式>    覆盖配置里的模式: server | client
-c, --config <路径>  指定配置文件
-v, --verbose        Debug 级日志
```

> 命令行按**当前工作目录**查找 `config.json`；界面按 **exe 所在目录**查找。
> 为避免混淆，建议统一从 `target\release\` 目录运行。

---

## 工作模式

> ⚠️ **服务端与客户端互斥** —— 皎月连同一时刻只能扮演一个角色。

### 服务端模式

家里 NAS/PC 做服务端，你在外面连回来。

```jsonc
{ "mode": "server" }
```

恢复逻辑：检测到服务端未运行 → 发 `startServer` → 等待确认。

### 客户端模式

你连别人的服务端，连接老是掉。

```jsonc
{
  "mode": "client",
  "client": {
    "target_host_id": "2",        // 见「在线主机」列表
    "close_server_first": true     // 连之前自动停掉本机服务端（推荐）
  }
}
```

恢复逻辑：目标主机消失或连接断开 → `stopcon` 清理 → `conpc` 重连。

---

## 保活机制

```
三层判据
┌──────────────────────────────────────────────┐
│ Layer 1  进程    natpierce.exe 是否存活        │
│ Layer 2  服务    API 可达性 + 业务状态         │
│ Layer 3  心跳    周期通信，防空闲掉线          │
└──────────────────────────────────────────────┘

决策表
┌────────────────────────┬──────────────┬────────────────────┐
│ 现象                    │ 判定          │ 动作                │
├────────────────────────┼──────────────┼────────────────────┤
│ API 不通 + 进程不在      │ 软件没跑      │ 提权启动进程         │
│ API 不通 + 进程在        │ 软件卡死      │ 累计失败 → 重启进程  │
│ 服务端未启动             │ 服务停了      │ 发 startServer      │
│ 客户端未连上目标         │ 连接断了      │ stopcon + conpc     │
│ 一切正常                 │ 健康          │ 清零计数 + 心跳      │
└────────────────────────┴──────────────┴────────────────────┘
```

**防误杀**：连续失败达到 `restart_after_failures`（默认 5）才重启进程；
两次重启至少间隔 `cooldown_sec`（默认 30 秒）。

---

## 配置文件

完整模板见 [`config.example.json`](config.example.json)。

```jsonc
{
  "mode": "server",

  "natpierce": {
    "exe_path": "D:\\Tools\\natpierce\\natpierce.exe",
    "working_dir": "",              // 留空 = exe 所在目录
    "process_name": "natpierce",
    "start_args": ["-C"]
  },

  "api": {
    "url": "ws://127.0.0.1:33272/ws",
    "fallback_url": "ws://[::1]:33272/ws",   // IPv4 失败时回退 IPv6
    "connect_timeout_ms": 8000,
    "command_timeout_ms": 15000
  },

  "server": {
    "page_password": "dpapi",       // dpapi | env:VAR | file:path | 明文
    "connection_password": "",
    "max_clients": 0,               // 0 = 无限制
    "start_timeout_sec": 45
  },

  "client": {
    "target_host_id": "",
    "target_host_name": "",
    "close_server_first": true
  },

  "keepalive": {
    "interval_sec": 60,             // 巡检间隔
    "heartbeat_sec": 15,            // 心跳间隔
    "fail_threshold": 3,
    "cooldown_sec": 30,             // 重启冷却
    "restart_after_failures": 5,    // 连续失败多少次才重启进程
    "enabled": true
  },

  "logging": {
    "level": "info",
    "directory": "logs",
    "retain_days": 30,
    "console": true
  }
}
```

### 密码的三种来源

| 写法 | 说明 | 安全性 |
|---|---|---|
| `"dpapi"` | DPAPI 加密文件（`set-password` 生成，绑定当前 Windows 用户） | ⭐ 推荐 |
| `"env:NATPIERCE_PWD"` | 环境变量 | 好 |
| `"file:./secrets.txt"` | 独立文件（记得 gitignore） | 一般 |
| 直接写明文 | 方便调试 | 不推荐 |

---

## 开机自启 / 无人值守

### 方式一：开机自启（适合日常使用）

界面里勾选「开机自启」，或：

```powershell
natpierce-keepalived autostart on
```

写的是 `HKCU\...\Run`，**不需要管理员权限**，可在「任务管理器 → 启动」管理。

### 方式二：Windows 服务（推荐长期无人值守）⭐

```powershell
# 管理员 PowerShell
natpierce-keepalived service install
natpierce-keepalived service start
```

由 SCM 托管，**崩溃自动重启**，以 SYSTEM 权限运行，**完全没有 UAC 打扰**。

> 注意：服务运行在 Session 0，皎月连的 GUI 界面可能不可见（进程正常运行）。
> 需要看皎月连界面时用方式一。

---

## 常见问题

<details>
<summary><b>配置都正确，但保活不动 / 日志里是旧的 exe 路径</b></summary>

**检查是不是有多个 config.json。** 界面顶部会显示实际使用的配置文件路径。

命令行按**当前工作目录**找配置，界面按 **exe 同目录**找 —— 两者可能命中不同文件。

统一做法：都从 `target\release\` 目录运行。

> 守护进程已支持**配置热重载**：改完配置保存后 1 秒内自动生效，无需重启。
</details>

<details>
<summary><b>日志里的中文是乱码</b></summary>

Windows 控制台默认 GBK 编码，与 UTF-8 输出不兼容。

- `gui` / `service` 模式：日志写文件（UTF-8，无乱码），用托盘「打开日志目录」查看
- 前台 CLI 模式：先执行 `chcp 65001` 切到 UTF-8 代码页
</details>

<details>
<summary><b>界面提示"配置文件不存在"</b></summary>

首次运行会自动创建默认配置并弹出设置窗口引导，按提示操作即可。

若想手动生成：`natpierce-keepalived init`
</details>

<details>
<summary><b>皎月连拉不起来（没有反应）</b></summary>

按顺序排查：

1. **exe 路径是否真实存在** —— 界面顶部红框会提示
2. **页面访问密码是否录入** —— `startServer` 强制要求 6-20 位
3. **看日志** —— 托盘「打开日志目录」，或 `3-查看状态.bat` 选 4
4. **权限** —— `natpierce.exe` 自身要求管理员权限，启动时会弹 UAC；
   若想完全无 UAC，请用 `service install` 装成服务
</details>

<details>
<summary><b>会不会和 GoWork 之类的工具冲突？</b></summary>

会。多个保活工具同时管理 `natpierce.exe` 会产生多实例冲突。
**同一台机器上只用一个。**
</details>

---

## 安全性说明

- 密码默认用 **DPAPI** 加密存储，密文只能由**同一 Windows 用户在同一台机器**解密
- `config.json` / `secrets.dpapi` / `logs/` 均已在 `.gitignore` 中
- 本程序**只与本机 `127.0.0.1:33272` 通信**，不向任何外部服务器发送数据
- 若曾把明文密码提交进 Git 历史，请**立即修改密码**（删除提交也无法撤回已泄露内容）

---

## 项目结构

```
crates/
├── core/     核心库（配置 / 协议 / DPAPI / 保活状态机 / 进程 / 服务 / 停止标志）
├── cli/      守护进程 + 命令行  → natpierce-keepalived.exe
└── shell/    Tauri 图形界面
    ├── src/main.rs        托盘 + 命令桥接 + 事件推送
    ├── ui/                index.html / style.css / app.js
    └── tauri.conf.json
```

---

## 兼容性

- **Windows 10 / 11**（用到 DPAPI、Toolhelp32、ShellExecuteW、WebView2）
- 皎月连 **v1.06** 实测通过（本地控制端口 33272）
- 皎月连升级若改动端口或协议，需相应更新 `api` 配置与 `protocol.rs`

---

## License

[MIT](LICENSE)
