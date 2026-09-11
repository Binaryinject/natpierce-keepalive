# natpierce-keepalive

[![Release](https://github.com/Binaryinject/natpierce-keepalive/actions/workflows/release.yml/badge.svg)](https://github.com/Binaryinject/natpierce-keepalive/actions/workflows/release.yml)
![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-blue)
![Rust](https://img.shields.io/badge/rust-1.75%2B-orange)
![License](https://img.shields.io/badge/license-MIT-green)

皎月连（natpierce）的**连接保活守护** —— 服务端 / 客户端双模式，图形界面 + 独立守护进程。

> 解决「皎月连服务端断了不会自动重开」「客户端连接掉了要手动点」「软件崩了没人管」这类问题。
> 配好一次，之后开机就自动守着。

---

## 目录

- [功能特性](#功能特性)
- [界面](#界面)
- [架构](#架构)
- [安装](#安装)
- [快速开始](#快速开始)
- [工作模式](#工作模式)
- [保活机制](#保活机制)
- [配置文件](#配置文件)
- [命令行工具](#命令行工具)
- [开机自启 / 无人值守](#开机自启--无人值守)
- [常见问题](#常见问题)
- [安全性说明](#安全性说明)
- [项目结构](#项目结构)
- [开发](#开发)
- [兼容性](#兼容性)

---

## 功能特性

| 特性 | 说明 |
|---|---|
| **双模式** | 服务端（保活本机服务）/ 客户端（保活到远端主机的连接），两者互斥 |
| **三层判据** | 进程层 → 服务层 → 心跳层，逐级定位问题再对症恢复 |
| **分层恢复** | 软件没跑就拉起来；服务停了就开；连接断了就重连；卡死了才重启 |
| **组网模式** | 自动先开「组网模式」（虚拟网卡监听全部端口），再开启服务端 |
| **自动开启** | 服务端起来后顺带打开皎月连自身的「自动开启」，形成两层保障 |
| **配置热重载** | 界面里改完保存，守护进程 1 秒内自动生效，无需重启 |
| **界面内嵌日志** | 启动/恢复全过程直接在界面上看，不用去翻日志文件 |
| **优雅停止** | 靠停止标志文件退出，与权限无关，不依赖强杀 |
| **密码加密** | 页面/登录密码用 **Windows DPAPI** 加密，不落明文 |
| **现代界面** | Tauri 2 + WebView2（HTML/CSS/JS），深色主题 |
| **崩溃自愈** | 可注册为 Windows 服务，由 SCM 托管并自动重启 |
| **防重启风暴** | 连续失败阈值 + 冷却时间，网络抖动时不会反复重启 |
| **零外部通信** | 只与本机 `127.0.0.1:33272` 通信，不向任何服务器发数据 |

---

## 界面

启动后是一个单窗口界面，自上而下：

| 区块 | 内容 |
|---|---|
| 顶栏 | 程序名 / 版本，右侧「保活：运行中」状态徽章 |
| 运行状态 | 皎月连进程、**服务端**或**目标连接**（随模式切换）、账号、识别码 |
| 在线主机 | 仅客户端模式显示，点识别码即可填入目标 |
| **运行日志** | 守护进程最近 200 行，实时刷新，WARN/ERROR 高亮 |
| 皎月连账号 | 登录账号 + 登录密码（自动登录用） |
| 工作模式 | 服务端 / 客户端 单选 |
| 服务端设置 | 页面访问密码、最大连接数、连接密码、组网模式、自动开启 |
| 客户端设置 | 目标识别码 / 目标名称、连接密码、先停本机服务端 |
| 皎月连程序 | natpierce.exe 路径（浏览选择）、工作目录、进程名、启动参数、API 地址 |
| 保活参数 | 巡检间隔、心跳、失败阈值、重启阈值 |
| 系统集成 | 开机自启、Windows 服务状态 |

**系统托盘**（左键单击或双击都能打开窗口）：

```
打开设置…
────────────
退出
```

> 托盘**故意不放设置类开关**：托盘菜单的勾选状态不会跟随界面变化，
> 两处并存必然不同步，所以设置项统一只留界面一个入口。

---

## 架构

```
┌─ natpierce-gui.exe（Tauri 2 + WebView2）───────────┐
│  · 系统托盘                                        │
│  · Web UI 设置界面 + 运行日志面板                   │
│  · 配置校验、状态展示（后端事件推送，非轮询）        │
└─────────────────────────────────────────────────────┘
                      │ 启动 / 优雅停止
                      ▼
┌─ natpierce-keepalived.exe（纯后台守护）────────────┐
│  · 保活状态机（三层判据 + 分层恢复）                │
│  · 配置热重载 / 心跳 / 防重启风暴                   │
│  · 进程检测 / 提权启动 / 优雅终止                   │
└─────────────────────────────────────────────────────┘
                      │ 本地控制接口
                      ▼
         ws://127.0.0.1:33272/ws  →  皎月连本体
```

**为什么拆成两个进程**：早期把保活逻辑塞在界面进程里，UI 线程会被网络探测和进程管理阻塞，界面严重卡顿。拆分后界面只做渲染，永远不会被阻塞；守护进程也能独立于界面常驻运行。

**两个进程怎么交换状态**：守护进程把界面需要、但探测又拿不到的信息（例如客户端是否真的连上了目标）写进同目录的 `.status` 文件，界面读取它。

---

## 安装

### 方式一：安装包（推荐）

到 [Releases](https://github.com/Binaryinject/natpierce-keepalive/releases) 下载：

| 文件 | 说明 |
|---|---|
| `natpierce-keepalive_<版本>_x64-setup.exe` | NSIS 安装包，双击安装（按当前用户安装，无需管理员） |
| `natpierce-keepalive-<版本>-portable.zip` | 便携版，解压即用 |

安装后两个 exe 都在：

```
%LOCALAPPDATA%\natpierce-keepalive\
├── natpierce-gui.exe          图形界面
├── natpierce-keepalived.exe   守护进程 / 命令行
├── config.json                配置（首次运行自动生成）
├── secrets.dpapi              DPAPI 加密的密码
└── logs\                      日志
```

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

## 快速开始

### 1. 启动

双击 `natpierce-gui.exe`。

### 2. 完成配置（仅首次）

界面会弹出**红色警告框**，逐条列出缺什么：

- **natpierce.exe 路径** —— 点「浏览…」选择你的皎月连主程序
  - 会自动填好「进程名」与「工作目录」
- **页面访问密码** —— 皎月连界面里的「页面访问密码」（6-20 位，**组网模式下必填**）
- 客户端模式还需要**目标识别码**（可在「在线主机」列表直接点选）

两个密码框下方会明确显示状态：

```
✅ 已加密保存 · 留空则不修改，输入新密码会覆盖
⚠ 尚未设置 · 必须填写并保存
```

填完点底部「**保存配置**」。**保存后保活立刻启动**，不需要再做别的。

### 3. 之后就什么都不用管了

```
打开界面 → 自动校验配置 → 自动拉起守护进程 → 自动检测并启动皎月连
        → 自动登录 → 自动开组网 → 自动开服务端（或自动连目标主机）
```

整个过程会实时显示在界面上的「运行日志」里。

---

## 工作模式

> ⚠️ **服务端与客户端互斥** —— 皎月连同一时刻只能扮演一个角色。
> 客户端模式下若本机服务端还开着，勾选「先停掉本机服务端」会自动处理。

### 服务端模式

家里 NAS/PC 做服务端，你在外面连回来。

**开启服务端是两步**（与皎月连界面一致）：

1. 先打开**组网模式** —— 虚拟网卡监听**全部端口**，不需要手填端口映射
2. 再开启服务端，此时才需要**页面访问密码**

本程序会按这个顺序自动执行，不会漏掉第一步。

```jsonc
{
  "mode": "server",
  "server": {
    "vpn_mode": true,           // 组网模式（强烈建议保持开启）
    "page_password": "dpapi",   // 页面访问密码
    "auto_start_server": true   // 顺带打开皎月连自身的「自动开启」
  }
}
```

恢复逻辑：检测到服务端未运行 → 确保组网已开 → 发 `startServer` → 等待确认。

### 客户端模式

你连别人的服务端，连接老是掉。

```jsonc
{
  "mode": "client",
  "client": {
    "target_host_id": "2",       // 目标识别码（见「在线主机」列表）
    "close_server_first": true    // 连之前自动停掉本机服务端（推荐）
  }
}
```

恢复逻辑：目标不在线或连接断开 → `stopcon` 清理 → `conpc` 重连。

> **「目标在线」≠「已连接」**：前者只是能看见对方。程序区分这两种状态，
> 只有确认连上才算健康，否则会主动发起连接。

### 两种模式对比

| | 服务端模式 | 客户端模式 |
|---|---|---|
| 作用 | 让本机可被连接 | 让本机连上别人 |
| 必需项 | natpierce.exe、页面访问密码 | natpierce.exe、目标识别码 |
| 页面访问密码 | **必需**（组网模式） | 不需要 |
| 关键命令 | `VPN` → `startServer` | `stopcon` → `conpc` |
| 状态栏显示 | 服务端：已启动/未启动 | 目标连接：已连接/已断开 |

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
│ 服务端未启动             │ 服务停了      │ 开组网 → startServer │
│ 客户端未连上目标         │ 连接断了      │ stopcon → conpc     │
│ 一切正常                 │ 健康          │ 清零计数 + 心跳      │
└────────────────────────┴──────────────┴────────────────────┘
```

**一次巡检内连续推进**：修好一步（启动进程 / 登录 / 开服务端）后立即探测下一步，
而不是每步都等满一个巡检间隔。所以「拉起皎月连 → 自动登录 → 开服务端」
整条链路通常十几秒走完，而不是等三个 60 秒。

**防误杀**：连续失败达到 `restart_after_failures`（默认 5）才重启进程；
两次重启至少间隔 `cooldown_sec`（默认 30 秒）。
**凭据类错误不会触发重启** —— 账号密码不对时重启进程毫无意义。

**优雅停止**：界面退出或保存配置时会写一个停止标志文件，守护进程在 1 秒内
自行收尾退出；只有它无响应时才会强制结束。

---

## 配置文件

### 唯一路径（重要）

配置文件**只有一个位置**：

```
%LOCALAPPDATA%\natpierce-keepalive\config.json
```

同一目录下还有：

| 文件 | 说明 |
|---|---|
| `secrets.dpapi` | DPAPI 加密的密码（登录 / 页面 / 连接） |
| `.status` | 守护进程写给界面看的运行时状态 |
| `logs\` | 日志，按天分文件 |

无论从哪个目录启动、无论是界面还是守护进程，读写的都是这一份。
（早期版本会依次探测当前目录和 exe 目录，结果一台机器上并存多份配置，
界面改了一份、守护进程读另一份 —— 该行为已废弃。）

需要临时指定其它配置时用环境变量 `NATPIERCE_KEEPALIVE_CONFIG`。

### 完整示例

模板见 [`config.example.json`](config.example.json)。

```jsonc
{
  "mode": "server",                    // server | client

  "natpierce": {
    "exe_path": "D:\\Tools\\natpierce\\natpierce.exe",
    "working_dir": "",                 // 留空 = exe 所在目录
    "process_name": "natpierce",       // 不带 .exe
    "start_args": ["-C"]
  },

  "api": {
    "url": "ws://127.0.0.1:33272/ws",
    "fallback_url": "ws://[::1]:33272/ws",   // IPv4 失败时回退 IPv6
    "connect_timeout_ms": 8000,
    "command_timeout_ms": 15000
  },

  "server": {
    "page_password": "dpapi",          // dpapi | env:VAR | file:path | 明文
    "connection_password": "",
    "max_clients": 0,                  // 0 = 无限制
    "lan_ip": "",                      // 组网模式下的虚拟网卡地址，留空自动
    "vpn_mode": true,                  // 组网模式（虚拟网卡监听全部端口）
    "auto_start_server": true,         // 让皎月连自身也开启「自动开启」
    "start_timeout_sec": 45
  },

  "client": {
    "target_host_id": "",              // 目标识别码，优先级最高
    "target_host_name": "",            // 识别码为空时按名称匹配
    "connection_password": "",
    "target_index": 0,                 // 都为空时按列表序号挑
    "close_server_first": true
  },

  "keepalive": {
    "interval_sec": 60,                // 巡检间隔
    "heartbeat_sec": 15,               // 心跳间隔
    "fail_threshold": 3,
    "cooldown_sec": 30,                // 重启冷却
    "restart_after_failures": 5,       // 连续失败多少次才重启进程
    "enabled": true
  },

  "logging": {
    "level": "info",                   // trace | debug | info | warn | error
    "directory": "logs",               // 相对路径基于配置目录
    "retain_days": 30,
    "console": true
  }
}
```

### 密码的四种来源

| 写法 | 说明 | 安全性 |
|---|---|---|
| `"dpapi"` | DPAPI 加密文件（界面保存或 `set-password` 生成，绑定当前 Windows 用户） | ⭐ 推荐 |
| `"env:NATPIERCE_PWD"` | 环境变量 | 好 |
| `"file:./secrets.txt"` | 独立文件（记得 gitignore） | 一般 |
| 直接写明文 | 方便调试 | 不推荐 |

---

## 命令行工具

`natpierce-keepalived.exe` 可独立使用：

```powershell
natpierce-keepalived status              # 状态：进程 / 服务端 / 在线主机
natpierce-keepalived hosts               # 列出在线主机及识别码
natpierce-keepalived ensure              # 执行一次保活检查并修复
natpierce-keepalived run                 # 前台运行保活循环（实时日志）
natpierce-keepalived daemon              # 后台运行保活循环
natpierce-keepalived gui                 # 拉起图形界面
natpierce-keepalived autostart on|off    # 开关机自启
natpierce-keepalived set-password page   # 录入页面密码（DPAPI 加密）
natpierce-keepalived init                # 生成默认配置文件
natpierce-keepalived version             # 版本
natpierce-keepalived help                # 帮助
natpierce-keepalived service install     # 安装为 Windows 服务（需管理员）
```

常用选项：

```
-m, --mode <模式>     覆盖配置里的模式: server | client
-c, --config <路径>   指定配置文件
-v, --verbose         Debug 级日志
```

---

## 开机自启 / 无人值守

### 方式一：开机自启（适合日常使用）

界面「系统集成」里勾选**开机自启**，然后点保存配置。

写的是 `HKCU\...\Run`，**不需要管理员权限**，可在「任务管理器 → 启动」里查看或禁用。

### 方式二：Windows 服务（推荐长期无人值守）⭐

```powershell
# 管理员 PowerShell
natpierce-keepalived service install
natpierce-keepalived service start
```

由 SCM 托管，**崩溃自动重启**，以 SYSTEM 权限运行，**完全没有 UAC 打扰**。

> 注意：服务运行在 Session 0，皎月连的图形界面可能不可见（但进程正常运行）。
> 需要看皎月连界面时用方式一。

---

## 常见问题

<details>
<summary><b>服务端起不来 / 一直显示「未启动」</b></summary>

按顺序检查：

1. **页面访问密码填了吗** —— 组网模式下这是硬性前置条件（6-20 位）。
   界面密码框下方会显示 `✅ 已加密保存` 或 `⚠ 尚未设置`。
2. **组网模式开着吗** —— 看皎月连自己的 `config`（`D:\Tools\natpierce-win-v1.06\config`，
   无扩展名的 JSON）里 `VPN` 是否为 `true`。程序会自动开，但若失败日志里会有记录。
3. **看界面「运行日志」** —— 失败时会打印 natpierce 的原始回应，
   例如 `开启服务端未成功，natpierce 回应: ...`，据此判断是密码错还是别的原因。
</details>

<details>
<summary><b>客户端模式填了识别码，但不去连接</b></summary>

确认识别码填的是**「在线主机」列表里那个数字 ID**（可点击直接填入）。

填好后保存，日志里应该出现：

```
INFO 未连接到目标主机，尝试连接
INFO 目标主机: 2
INFO 连接确认: yes
INFO 已连接到目标主机
```

状态栏「目标连接」会变成**已连接**。

> 注意区分：目标**在线**（出现在列表里）不等于**已连接**。
</details>

<details>
<summary><b>「目标连接」一直显示「探测中」</b></summary>

如果守护进程没有运行（界面顶栏显示「保活：已停止」），就没有人去维护连接，
状态自然探测不到。先确认保活已启动。

守护进程运行中仍显示「探测中」时，说明它自己也没拿到连接状态 ——
这时看日志里是否有连接成功的记录。
</details>

<details>
<summary><b>开机自启勾了但重启后失效</b></summary>

先确认注册表项还在：

```powershell
(Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name NatpierceKeepalive).NatpierceKeepalive
```

应该输出形如 `"C:\Users\<你>\AppData\Local\natpierce-keepalive\natpierce-gui.exe" --config ...`。

如果没有，回到界面重新勾选并**点保存配置**（自启是保存时写入的）。

> 曾经遇到的真实原因：**安装目录被卸载程序清空了**，注册表指向的 exe 已不存在，
> 自启自然失败。重装即可。
</details>

<details>
<summary><b>日志在哪儿 / 怎么看</b></summary>

**直接在界面上看** —— 「运行日志」区块显示守护进程最近 200 行，随状态自动刷新。

文件在 `%LOCALAPPDATA%\natpierce-keepalive\logs\keepalive.log.YYYY-MM-DD`。

每次启动会**清空当天日志**（上一次的记录改名为 `.prev`），
这样面板里只剩本次启动的过程，不会被历史记录淹没。
</details>

<details>
<summary><b>启动时弹出 UAC 提示</b></summary>

`natpierce.exe` 自身声明了 `requireAdministrator`，普通权限进程无法启动它。
本程序采用**按需提权**：只在这一步通过 `ShellExecuteW("runas")` 弹一次 UAC。

- 想避免每次弹窗 → 用 `service install` 装成服务（以 SYSTEM 运行）
- 弹窗后请选「是」，否则皎月连拉不起来（日志会有记录）

本程序自身**不要求管理员权限**。
</details>

<details>
<summary><b>日志里的中文是乱码</b></summary>

Windows 控制台默认 GBK 编码，与 UTF-8 输出不兼容。

- 界面 / 服务模式：日志写文件（UTF-8，无乱码）
- 前台 CLI 模式：先执行 `chcp 65001` 切到 UTF-8 代码页
</details>

<details>
<summary><b>会不会和 GoWork 之类的工具冲突？</b></summary>

会。多个保活工具同时管理 `natpierce.exe` 会产生多实例冲突。
**同一台机器上只用一个。**
</details>

<details>
<summary><b>怎么彻底卸载</b></summary>

1. 托盘右键 →「退出」（会先优雅停掉守护进程）
2. 从「应用和功能」卸载，或运行安装目录下的 `uninstall.exe`
3. 如需清干净，删掉 `%LOCALAPPDATA%\natpierce-keepalive\`（含配置与日志）

如果之前装过 Windows 服务：`natpierce-keepalived service uninstall`（需管理员）。
</details>

---

## 安全性说明

- 密码默认用 **DPAPI** 加密存储，密文只能由**同一 Windows 用户在同一台机器**解密
- `config.json` / `secrets.dpapi` / `logs/` 均已在 `.gitignore` 中
- 本程序**只与本机 `127.0.0.1:33272` 通信**，不向任何外部服务器发送数据
- 日志中可能包含账号、识别码等，分享日志前请自行检查
- 若曾把明文密码提交进 Git 历史，请**立即修改密码**（删除提交也无法撤回已泄露内容）

---

## 项目结构

```
natpierce-keepalive/
├── crates/
│   ├── core/                核心库
│   │   └── src/
│   │       ├── config.rs        配置读写 / 唯一路径解析 / 指纹热重载
│   │       ├── keepalive.rs     保活状态机（三层判据 + 分层恢复）
│   │       ├── secret.rs        DPAPI 加解密 / 多密钥 vault
│   │       ├── process.rs       进程检测 / 提权启动 / 终止
│   │       ├── autostart.rs     HKCU Run 开机自启
│   │       ├── service.rs       Windows 服务（sc.exe）
│   │       ├── stop_flag.rs     优雅停止标志
│   │       ├── logging.rs       日志初始化（启动清空当天）
│   │       └── api/
│   │           ├── protocol.rs  报文构造与解析
│   │           └── client.rs    WebSocket 客户端 + 一次性探测
│   ├── cli/                 守护进程 + 命令行  → natpierce-keepalived.exe
│   └── shell/               Tauri 图形界面      → natpierce-gui.exe
│       ├── src/main.rs          托盘 + 命令桥接 + 事件推送
│       ├── ui/                  index.html / style.css / app.js
│       ├── windows/hooks.nsh    NSIS 安装钩子（附带守护进程）
│       └── tauri.conf.json
├── .github/workflows/release.yml
├── config.example.json
├── HANDOFF.md               开发交接文档（踩坑记录）
└── README.md
```

---

## 开发

```powershell
# 编译
cargo build --release --workspace

# 单元测试
cargo test --release -p natpierce-core

# 格式与静态检查
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
```

**改完界面代码必须重新编译** —— 前端资源是**编译期嵌入**二进制里的，
直接改 `crates/shell/ui/` 下的文件不会生效。

**本地调试时注意**：配置只认 `%LOCALAPPDATA%\natpierce-keepalive\config.json`，
所以把编译产物复制到那里再运行，不要直接从 `target\release\` 启动
（否则它会读写另一个位置，让你以为"改了没反应"）：

```powershell
$t = "$env:LOCALAPPDATA\natpierce-keepalive"
Copy-Item target\release\natpierce-gui.exe,target\release\natpierce-keepalived.exe $t -Force
& "$t\natpierce-gui.exe"
```

**发布**：推一个 `v*` tag 即可触发 GitHub Actions 构建并创建 Release。

```powershell
git tag v0.1.2
git push origin main --tags
```

---

## 兼容性

- **Windows 10 / 11**（用到 DPAPI、Toolhelp32、ShellExecuteW、WebView2）
- 皎月连 **v1.06** 实测通过（本地控制端口 33272）
- 皎月连升级若改动端口或协议，需相应更新 `api` 配置与 `crates/core/src/api/protocol.rs`

---

## License

[MIT](LICENSE)
