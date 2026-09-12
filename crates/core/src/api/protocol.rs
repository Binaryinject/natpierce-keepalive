//! 皎月连本地控制接口的协议层
//!
//! # 协议概述
//!
//! 皎月连在本地运行一个 Web 控制服务（默认 `127.0.0.1:33272`），
//! 其 Web UI 通过 WebSocket (`/ws`) 与内核通信。
//!
//! ## 报文格式
//!
//! ```text
//! 命令<$!$>参数1<$!$>参数2<$!$>...
//! ```
//!
//! 分隔符是字面量 `<$!$>` （4 字符，UTF-8 中占 6 字节）。
//!
//! ## 已知命令（由 Web UI 的前端脚本归纳）
//!
//! | 命令 | 参数 | 说明 |
//! |---|---|---|
//! | `startServer` | 连接密码, 最大连接数, 页面密码, 局域网IP | 开启服务端服务 |
//! | `stopServer` | — | 停止服务端服务 |
//! | `restart` | — | 重启服务 |
//! | `pclist` | — | 请求在线主机列表 |
//! | `pcinfo` | 主机ID | 查询指定主机信息 |
//! | `conpc` | 主机ID | 连接指定主机 |
//! | `stopcon` | — | 断开当前连接 |
//! | `dis` | 连接ID / 客户端地址 | 踢掉某个客户端 |
//! | `restart` | — | 重启 |
//!
//! ## 已知推送（服务端 → 客户端）
//!
//! | 首字段 | 含义 |
//! |---|---|
//! | `start` | 软件已启动：`start<sep>版本<sep>平台` |
//! | `1` | 服务端**未启动** |
//! | `2` | 服务端**已启动** |
//! | `pclist` | 在线主机列表 |
//! | `discon` | 远端断开 |
//! | `conerr` | 连接失败 |
//! | `log` | 连接日志（`+` 上线 / `-` 下线） |
//! | `info` | 提示信息（需弹窗） |
//! | `vipend` | VIP 到期 |

use serde::{Deserialize, Serialize};

/// 协议分隔符
pub const SEP: &str = "<$!$>";

/// 客户端 Web 控制端口默认值
pub const DEFAULT_PORT: u16 = 33272;

// ============================================================
// 命令构造
// ============================================================

/// 把多个字段拼成一条命令
pub fn build(parts: &[&str]) -> String {
    parts.join(SEP)
}

/// 开启服务端服务
///
/// 参数顺序（由 Web UI 的 `start_server()` 确定）：
/// `连接密码` `最大连接数` `页面访问密码` `局域网IP`
pub fn cmd_start_server(
    connection_password: &str,
    max_clients: u32,
    page_password: &str,
    lan_ip: &str,
) -> String {
    build(&[
        "startServer",
        connection_password,
        &max_clients.to_string(),
        page_password,
        lan_ip,
    ])
}

/// 停止服务端服务
pub fn cmd_stop_server() -> String {
    "stopServer".to_string()
}

/// 重启服务
pub fn cmd_restart() -> String {
    build(&["restart", ""])
}

/// 请求在线主机列表
pub fn cmd_pc_list() -> String {
    build(&["pclist", ""])
}

/// 查询主机信息
pub fn cmd_pc_info(host_id: &str) -> String {
    build(&["pcinfo", host_id])
}

/// 连接指定主机
pub fn cmd_connect_pc(host_id: &str) -> String {
    build(&["conpc", host_id])
}

/// 断开当前连接
pub fn cmd_stop_connect() -> String {
    build(&["stopcon", ""])
}

/// 设置「自动开启」：皎月连自身重开后自动恢复服务
///
/// 对应界面上的「自动开启」勾选框。
/// 注意：这是我们之外的**第二层**保障 —— 皎月连自己会重开服务；
/// 我们这一层则负责它彻底没开或连接掉了的情况。
pub fn cmd_autostart(on: bool) -> String {
    build(&["autostart", if on { "on" } else { "off" }])
}

/// 打开 / 关闭「组网模式」
///
/// 对应界面上那个「组网模式」开关。官方开服务端是**两步**：
///   ① 勾选组网  → `VPN<$!$>1`
///   ② 开服务器  → `startServer<连接密码><最大数><页面密码><局域网IP>`
///
/// 只做 ② 而漏掉 ① 时，natpierce 会忽略 startServer 里的页面密码：
/// 配置里 `VPN` 保持 false、`WebPwd` 保持空，表现为
/// 「命令发出去了，但服务端就是起不来」。
pub fn cmd_vpn(on: bool) -> String {
    build(&["VPN", if on { "1" } else { "0" }])
}

/// 登录（账号 + 密码）
///
/// 报文：`login<$!$>账号<$!$>密码<$!$>保存密码(0/1)<$!$>自动登录(0/1)`
pub fn cmd_login(user: &str, pwd: &str, save_pwd: bool, auto_login: bool) -> String {
    build(&[
        "login",
        user,
        pwd,
        if save_pwd { "1" } else { "0" },
        if auto_login { "1" } else { "0" },
    ])
}

/// 强制登录（当他处已登录同一账号时，服务端会回 `y?`，用此命令确认顶掉）
pub fn cmd_force_login(user: &str, pwd: &str, save_pwd: bool, auto_login: bool) -> String {
    build(&[
        "y",
        user,
        pwd,
        if save_pwd { "1" } else { "0" },
        if auto_login { "1" } else { "0" },
    ])
}

/// 断开某个客户端
pub fn cmd_disconnect(id: &str) -> String {
    build(&["dis", id])
}

// ============================================================
// 消息解析
// ============================================================

/// 一条原始消息切分后的字段
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMessage {
    pub kind: String,
    pub fields: Vec<String>,
}

impl RawMessage {
    /// 按分隔符切分
    pub fn parse(raw: &str) -> Self {
        let parts: Vec<String> = raw.split(SEP).map(|s| s.to_string()).collect();
        let kind = parts.first().cloned().unwrap_or_default();
        let fields = if parts.len() > 1 {
            parts[1..].to_vec()
        } else {
            Vec::new()
        };
        Self { kind, fields }
    }

    /// 取第 n 个字段（0 起）
    pub fn field(&self, n: usize) -> &str {
        self.fields.get(n).map(|s| s.as_str()).unwrap_or("")
    }
}

/// 解析后的推送消息
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// 处于登录界面（未登录 / 已登出）
    NeedLogin,
    /// 软件已启动
    Started { version: String, platform: String },
    /// 服务端未启动
    ServerStopped { info: ServerInfo },
    /// 服务端已启动
    ServerRunning { info: ServerInfo },
    /// 在线主机列表
    PcList(Vec<HostEntry>),
    /// 主机信息
    PcInfo(String),
    /// 连接成功
    ConOk(String),
    /// 连接失败
    ConErr(String),
    /// 远端断开
    Disconnected(String),
    /// 客户端上线/下线日志
    ClientLog { action: LogAction, id: String, addr: String },
    /// 提示信息
    Info(String),
    /// VIP 到期
    VipEnd,
    /// 未知消息（保留原文便于排查）
    Unknown(String),
}

/// 服务端概况
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerInfo {
    pub account: String,
    pub identification: String,
    pub mappings: String,
    pub public_ip: String,
    pub machine_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogAction {
    Online,
    Offline,
}

/// 在线主机
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEntry {
    /// 会话编号 —— **每次接入都会重新分配**（日志里能看到 [1]→[2]→[3]→[4]），
    /// 所以它不能当稳定标识用。
    pub id: String,
    /// 机器名。相对稳定，但对方改名就失效。
    pub name: String,
    /// 映射信息（若有）
    pub mappings: String,
    /// 组网虚拟 IP（如 `10.6.22.2`）。
    /// 这是目前最稳定的标识 —— 按设备分配，不随重连变化。
    pub addr: String,
}

impl Message {
    /// 解析一条原始推送
    pub fn parse(raw: &str) -> Self {
        let m = RawMessage::parse(raw);
        match m.kind.as_str() {
            "start" => Message::Started {
                version: m.field(0).to_string(),
                platform: m.field(1).to_string(),
            },

            // 登录界面（未登录）
            "0" => Message::NeedLogin,

            // 服务端未启动
            "1" => Message::ServerStopped {
                info: parse_server_info(&m),
            },

            // 服务端已启动
            "2" => Message::ServerRunning {
                info: parse_server_info(&m),
            },

            "pclist" => Message::PcList(parse_host_list(&m)),
            "pcinfo" => Message::PcInfo(m.field(0).to_string()),
            "conpc" => Message::ConOk(m.field(0).to_string()),
            "conerr" => Message::ConErr(m.field(0).to_string()),
            "discon" => Message::Disconnected(m.fields.join(SEP)),
            "vipend" => Message::VipEnd,

            "log" => match m.field(0) {
                "+" => Message::ClientLog {
                    action: LogAction::Online,
                    id: m.field(1).to_string(),
                    addr: m.field(2).to_string(),
                },
                "-" => Message::ClientLog {
                    action: LogAction::Offline,
                    id: m.field(1).to_string(),
                    addr: m.field(2).to_string(),
                },
                _ => Message::Unknown(raw.to_string()),
            },

            "info" => Message::Info(m.field(0).to_string()),

            // 握手时会收到 selectClient，静默忽略
            "selectClient" => Message::Unknown(raw.to_string()),

            _ => Message::Unknown(raw.to_string()),
        }
    }

    /// 这条消息是否表示"服务端正在运行"
    pub fn is_server_running(&self) -> Option<bool> {
        match self {
            Message::ServerRunning { .. } => Some(true),
            Message::ServerStopped { .. } => Some(false),
            _ => None,
        }
    }
}

/// 从状态消息里提取服务端概况
///
/// 实测报文（`1` 与 `2` 的字段布局一致）：
/// ```text
/// 1<sep>账号<sep>1<sep>识别码<sep>映射<sep><sep><sep>公网IP<sep>主机名<sep>0<sep><sep>
///   0      1     2      3     4   5  6     7        8     9  10
/// ```
fn parse_server_info(m: &RawMessage) -> ServerInfo {
    ServerInfo {
        account: m.field(0).to_string(),
        identification: m.field(2).to_string(),
        mappings: m.field(3).to_string(),
        public_ip: m.field(6).to_string(),
        machine_name: m.field(7).to_string(),
    }
}

/// 解析在线主机列表
///
/// 报文形如：
/// ```text
/// pclist<sep>id<sep>name<sep>mappings<sep>addr<sep>id<sep>name<sep>mappings<sep>addr<sep>...
/// ```
///
/// 即 `kind` 之后的字段是**若干条 4 元组**。参考实现见
/// 社区版 keepalive 的 `parseHostList()`：以 4 为步长遍历，跳过空 id 与 `me`。
///
/// 其中 `id` 是**会话编号**（每次接入重新分配），`addr` 才是组网虚拟 IP ——
/// 后者用来做持久化识别。
pub fn parse_host_list(m: &RawMessage) -> Vec<HostEntry> {
    let f = &m.fields;
    let mut out = Vec::new();

    let mut i = 0;
    while i + 3 < f.len() {
        let id = f[i].trim().to_string();
        let name = f[i + 1].trim().to_string();
        let mappings = f[i + 2].trim().to_string();
        let addr = f[i + 3].trim().to_string();

        // 跳过自己与空记录
        if !id.is_empty() && id != "me" && id != "0" {
            out.push(HostEntry {
                id,
                name,
                mappings,
                addr,
            });
        }
        i += 4;
    }
    out
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_commands() {
        assert_eq!(cmd_pc_list(), "pclist<$!$>");
        assert_eq!(cmd_stop_server(), "stopServer");
        assert_eq!(
            cmd_start_server("", 0, "abc123", ""),
            "startServer<$!$><$!$>0<$!$>abc123<$!$>"
        );
        assert_eq!(cmd_connect_pc("48672718"), "conpc<$!$>48672718");
    }

    #[test]
    fn parse_started() {
        let m = Message::parse("start<$!$>1.06<$!$>windows");
        match m {
            Message::Started { version, platform } => {
                assert_eq!(version, "1.06");
                assert_eq!(platform, "windows");
            }
            _ => panic!("应解析为 Started"),
        }
    }

    #[test]
    fn parse_real_stopped_message() {
        // 这是从真实环境抓到的报文
        let raw = "1<$!$>274089056@qq.com<$!$>1<$!$>48672718<$!$>\
                   虚拟局域网|10.6.22.1|all|all|all|111111<$!$><$!$><$!$>\
                   171.222.190.1<$!$>WBN-PC<$!$>0<$!$><$!$>";
        let m = Message::parse(raw);
        match m {
            Message::ServerStopped { info } => {
                assert_eq!(info.account, "274089056@qq.com");
                assert_eq!(info.identification, "48672718");
                assert_eq!(info.public_ip, "171.222.190.1");
                assert_eq!(info.machine_name, "WBN-PC");
                assert!(info.mappings.contains("10.6.22.1"));
            }
            other => panic!("应解析为 ServerStopped，实际: {other:?}"),
        }
        assert_eq!(Message::parse(raw).is_server_running(), Some(false));
    }

    #[test]
    fn parse_server_running() {
        let raw = "2<$!$>user@x.com<$!$>1<$!$>12345678<$!$>map1<$!$>a<$!$>b<\
                   $!$>1.2.3.4<$!$>PC1<$!$>0<$!$><$!$>";
        let m = Message::parse(raw);
        assert_eq!(m.is_server_running(), Some(true));
        match m {
            Message::ServerRunning { info } => {
                assert_eq!(info.identification, "12345678");
                assert_eq!(info.machine_name, "PC1");
            }
            other => panic!("应解析为 ServerRunning，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_disconnect_and_log() {
        assert!(matches!(
            Message::parse("discon<$!$>somehost"),
            Message::Disconnected(_)
        ));
        assert!(matches!(
            Message::parse("log<$!$>+<$!$>id1<$!$>1.2.3.4:5678"),
            Message::ClientLog {
                action: LogAction::Online,
                ..
            }
        ));
        assert!(matches!(
            Message::parse("log<$!$>-<$!$>id1<$!$>1.2.3.4:5678"),
            Message::ClientLog {
                action: LogAction::Offline,
                ..
            }
        ));
        assert!(matches!(Message::parse("vipend"), Message::VipEnd));
    }

    #[test]
    fn host_list_skips_self() {
        let raw = "pclist<$!$>me<$!$>MYPC<$!$>map<$!$>addr<$!$>\
                   id001<$!$>RemoteHost<$!$>m2<$!$>a2";
        let hosts = parse_host_list(&RawMessage::parse(raw));
        assert_eq!(hosts.len(), 1, "应跳过 me，只保留 1 台主机");
        assert_eq!(hosts[0].id, "id001");
        assert_eq!(hosts[0].name, "RemoteHost");
    }

    #[test]
    fn unknown_message_kept() {
        match Message::parse("somethingweird<$!$>x") {
            Message::Unknown(s) => assert!(s.contains("somethingweird")),
            other => panic!("应保留为 Unknown，实际: {other:?}"),
        }
    }
}
