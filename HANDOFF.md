# 交接文档（HANDOFF）

> 用途：本对话上下文过长、压缩报错时的接续参考。
> 新会话读这份即可掌握全部关键状态，不必回溯历史。

---

## 项目

**仓库**：`D:\GIT\natpierce-keepalive` → https://github.com/Binaryinject/natpierce-keepalive

皎月连（natpierce）保活守护。三 crate workspace：

| crate | 产物 | 职责 |
|---|---|---|
| `core` | 库 | 配置 / 协议 / DPAPI / 保活状态机 / 进程 / 服务 / 停止标志 |
| `cli` | `natpierce-keepalived.exe` | 守护进程 + 命令行（**纯后台，无 UI**） |
| `shell` | `natpierce-gui.exe` | Tauri 2 + Web UI（托盘 + 设置窗口） |

---

## 用户环境（重要）

| 项 | 值 |
|---|---|
| 皎月连路径 | `D:\Tools\natpierce-win-v1.06\natpierce.exe` |
| 皎月连登录账号 | `274089056@qq.com` |
| 识别码 | `48672718` |
| 本地接口 | `ws://127.0.0.1:33272/ws` |
| **配置文件（安装版）** | `C:\Users\wbn\AppData\Local\皎月连保活守护\config.json` |
| 配置文件（开发版） | `D:\GIT\natpierce-keepalive\target\release\config.json` |

⚠️ **存在多份 config.json**，界面顶部会显示实际使用的路径。这是历史上多次"改配置不生效"的原因。

---

## 皎月连协议（逆向所得）

### 命令（`命令<$!$>参数1<$!$>参数2`）

| 命令 | 参数 | 说明 |
|---|---|---|
| `startServer` | 连接密码, 最大连接数, **页面密码**, 局域网IP | 开服务端 |
| `stopServer` | — | 停服务端 |
| `login` | 账号, 密码, 保存(0/1), 自动登录(0/1) | 登录 |
| `y` | 同上 | 强制登录（顶掉别处会话） |
| `autostart` | `on` / `off` | **自动开启**（皎月连自身重开时恢复服务） |
| `pclist` | — | 在线主机列表 |
| `conpc` | 主机ID | 连接主机 |
| `stopcon` | — | 断开 |

### 状态码（推送消息首字段）

| 码 | 含义 |
|---|---|
| `0` | **登录界面（未登录）** |
| `1` | 已登录，**服务端未启动** |
| `2` | 已登录，服务端已启动 |
| `start` | 软件已启动（`start<sep>版本<sep>平台`） |

### 关键语义

- **组网模式** = 虚拟网卡监听**所有端口**，无需手填端口映射（用户确认：默认必开）
- 官方 JS 逻辑：**只有组网模式开启时才读页面访问密码**，否则传空串
- **页面访问密码是开服务端的硬性前置条件**（组网模式下 6-20 位）
- 新用户流程：① 开组网 → ② 设页面密码 → ③ 才能开服务

---

## 当前状态

### Git

```
4841083  feat(server): 组网模式语义修正 + 启用皎月连自身「自动开启」   ← 本地，未推送
57502fb  perf(ci): Tauri CLI 改用 npm 预编译二进制                    ← 已推送
c517359  chore(release): v0.1.1                                      ← 已推送
```

**本地领先 origin/main 1 个提交**（用户要求先本地测试不上传）。

### 版本

- `v0.1.0`、`v0.1.1` tag 已发布，CI 走 `.github/workflows/release.yml`
- CI 产出：NSIS 安装包（含两个 exe，靠 installerHooks）+ 便携版 zip
- 本地 exe：`target\release\natpierce-gui.exe`（6.5MB）、`natpierce-keepalived.exe`（1.9MB）

---

## 已修复的关键 bug（踩坑记录）

这些都是实测中发现的，很容易再犯：

| # | 问题 | 根因 | 修复 |
|---|---|---|---|
| 1 | 日志完全消失 | `cli/main.rs` 残留 `windows_subsystem = "windows"`，GUI 子系统无控制台 | 删除该声明 |
| 2 | 日志中途断掉 | `WorkerGuard` 写在 match 分支里，分支返回即 drop | 提升到 `main()` 持有 |
| 3 | 启动慢 60 秒 | 首次巡检要等满一个 interval | `last_check` 回拨一个 interval |
| 4 | 改配置不生效 | 守护进程启动后从不重读配置 | 加配置热重载（指纹检测） |
| 5 | 保存配置报 missing field | 前后端字段命名不一致 | 统一 camelCase + `serde(default)` |
| 6 | 打包失败 | `bundle.resources` 在**编译期**校验路径存在 | 改用 NSIS `installerHooks` |
| 7 | NSIS 找不到文件 | `${__FILEDIR__}` 是**脚本生成目录**（`target\release\nsis\x64\`），不是 hooks 目录 | 用 `..\..\` 回退两级 |
| 8 | 登录成功却判为失败 | 登录后服务端通常仍是"未启动"（码 1），原判定只认码 2 | 码 1 / 2 都算登录成功 |
| 9 | 误导性 "DPAPI 数据无效" | `load_key` 对不存在的键静默返回空串 | 明确 bail 并指明缺哪个密码 |
| 10 | 无意义重启 15 次 | 密码类错误也触发重启进程 | `last_error_is_config` 守卫 |

---

## 待验证 / 待办

1. **本地测试**：`crates\shell\ui` 改动后需重新编译（UI 是编译期嵌入的）
   ```powershell
   cd D:\GIT\natpierce-keepalive
   cargo build --release
   .\target\release\natpierce-gui.exe
   ```
2. **用户需在界面填「页面访问密码」** —— 这是当前唯一阻塞服务端启动的问题
3. 上下文压缩报错：已修 `dsh-llm` 4 处调用（加 `?.`），**需重启 DSH 生效**

---

## 上游 DSH 的 bug（与项目无关）

**报错**：`this.adapters.get(...)?.adapter.imageRequestPricing is not a function`

**根因**：`dsh-llm` 的 `LlmRegistry.imageRequestPricing`（`lib/types/index.js:531`）：
```javascript
return this.adapters.get(provider)?.adapter.imageRequestPricing(provider, model);
//                              ↑ 只保护了 get()，没保护方法本身
```
`registerAdapter` → `prepareRoutes` 把 adapter **原样存表**，不校验接口完整性。
插件注册的精简 adapter 缺此方法 → 压缩时 token meter 遍历到它就崩。

**修复**（4 处，已应用）：
```diff
- ?.adapter.imageRequestPricing(provider, model)
+ ?.adapter.imageRequestPricing?.(provider, model)
```

文件位置（两处副本都要改）：
- `%USERPROFILE%\.dsh\profiles\node_modules\@deepseek-ai\dsh-llm\lib\{types/index,index}.js`
- `%USERPROFILE%\.dsh\profiles\node_modules\@deepseek-ai\dsh-{repeat-tool-reminder,tmux-context}\lib\index.js`
- 同上路径的 `%LOCALAPPDATA%\npm-cache\_npx\ebf017b61addb8bd\node_modules\...`

备份：同目录 `.dsh-bak` 后缀。**会被 npx 更新覆盖。**

---

## 常用命令

```powershell
# 编译 + 测试
cd D:\GIT\natpierce-keepalive
cargo build --release
cargo test --release -p natpierce-core

# 看守护进程日志（非连接失败行）
$d = "C:\Users\wbn\AppData\Local\皎月连保活守护\logs"   # 或 target\release\logs
Get-Content (Get-ChildItem $d -File | Sort LastWriteTime -Desc | Select -First 1).FullName -Encoding UTF8 |
  Where-Object { $_ -notmatch '连接 ws' } | Select -Last 30

# 守护进程运行时状态（含 lastError）
Get-Content "C:\Users\wbn\AppData\Local\皎月连保活守护\.status" -Encoding UTF8

# 优雅停止守护进程（写停止标志，与权限无关）
New-Item -ItemType File -Path "<配置目录>\.stop" -Force

# 强制清理
taskkill /F /IM natpierce-gui.exe /T
taskkill /F /IM natpierce-keepalived.exe /T
```
