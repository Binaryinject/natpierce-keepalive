//! 配置管理：加载、保存、热重载检测

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 运行模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// 服务端：本机作为服务端，保活 startServer
    #[default]
    Server,
    /// 客户端：连接远端主机，保活 conpc
    Client,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mode::Server => write!(f, "服务端"),
            Mode::Client => write!(f, "客户端"),
        }
    }
}

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

/// 顶层配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// 皎月连登录账号（邮箱），仅用于展示与同步，不参与保活判定
    pub account: String,
    /// 运行模式
    pub mode: Mode,
    pub natpierce: NatpierceConfig,
    pub api: ApiConfig,
    pub server: ServerConfig,
    pub client: ClientConfig,
    pub keepalive: KeepaliveConfig,
    pub logging: LoggingConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            account: String::new(),
            mode: Mode::Server,
            natpierce: NatpierceConfig::default(),
            api: ApiConfig::default(),
            server: ServerConfig::default(),
            client: ClientConfig::default(),
            keepalive: KeepaliveConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

/// 皎月连本体配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NatpierceConfig {
    /// natpierce.exe 路径
    pub exe_path: String,
    /// 工作目录（留空则取 exe 所在目录）
    pub working_dir: String,
    /// 进程名（不含 .exe）
    pub process_name: String,
    /// 启动参数
    pub start_args: Vec<String>,
}

impl Default for NatpierceConfig {
    fn default() -> Self {
        Self {
            exe_path: r"C:\Tools\natpierce\natpierce.exe".into(),
            working_dir: String::new(),
            process_name: "natpierce".into(),
            start_args: vec!["-C".into()],
        }
    }
}

impl NatpierceConfig {
    /// 解析工作目录
    pub fn resolve_working_dir(&self) -> PathBuf {
        if !self.working_dir.is_empty() {
            return PathBuf::from(&self.working_dir);
        }
        Path::new(&self.exe_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// 本地 API 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApiConfig {
    /// 主 WebSocket 地址
    pub url: String,
    /// 备用地址（IPv6 回退）
    pub fallback_url: String,
    /// 连接超时（毫秒）
    pub connect_timeout_ms: u64,
    /// 单条命令等待响应超时（毫秒）
    pub command_timeout_ms: u64,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            url: "ws://127.0.0.1:33272/ws".into(),
            fallback_url: "ws://[::1]:33272/ws".into(),
            connect_timeout_ms: 8000,
            command_timeout_ms: 15000,
        }
    }
}

/// 服务端模式配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// 页面访问密码（6-20位）—— startServer 必填
    /// 支持引用写法： "dpapi" | "env:VAR" | "file:path" | 明文
    pub page_password: String,
    /// 连接密码（可留空）
    pub connection_password: String,
    /// 最大连接数，0 = 无限制
    pub max_clients: u32,
    /// 局域网 IP（仅 Linux 点对网模式需要，Windows 留空）
    pub lan_ip: String,
    /// 是否启用组网模式（对应界面的开关）
    pub vpn_mode: bool,
    /// 是否让皎月连自身也启用「自动开启」（对应界面上的勾选框）
    ///
    /// 这是第二层保障：皎月连自己重开后会自动恢复服务。
    /// 我们这一层负责它彻底没开、或连接掉了的情况。
    pub auto_start_server: bool,
    /// 等待"开启成功"的最长秒数（服务端要建虚拟网卡、向云端注册，通常 10-20 秒）
    pub start_timeout_sec: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            page_password: "dpapi".into(),
            connection_password: String::new(),
            max_clients: 0,
            lan_ip: String::new(),
            vpn_mode: true,
            auto_start_server: true,
            start_timeout_sec: 45,
        }
    }
}

/// 客户端模式配置（对应 GoWork 的保活）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClientConfig {
    /// 目标主机 ID（识别码）；留空则用下面的名称匹配
    pub target_host_id: String,
    /// 目标主机名（用于显示/兜底匹配）
    pub target_host_name: String,
    /// 连接密码（若目标主机设置了）
    pub connection_password: String,
    /// 在线主机列表中的序号（1 起，0 = 用 ID/名称匹配）
    pub target_index: usize,
    /// 进入客户端模式前，自动停掉本机的服务端
    ///
    /// 皎月连的服务端与客户端互斥：本机开了服务端就无法作为客户端连别人。
    /// 默认 true，避免"连不上但找不到原因"。
    pub close_server_first: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            target_host_id: String::new(),
            target_host_name: String::new(),
            connection_password: String::new(),
            target_index: 0,
            close_server_first: true,
        }
    }
}

/// 保活参数
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeepaliveConfig {
    /// 巡检间隔（秒）
    pub interval_sec: u64,
    /// 心跳间隔（秒）—— 空闲时发 pclist 防掉线
    pub heartbeat_sec: u64,
    /// 连续失败多少次才判定异常
    pub fail_threshold: u32,
    /// 两次重启之间最小间隔（秒），防重启风暴
    pub cooldown_sec: u64,
    /// 连续失败多少次后升级为"重启进程"
    pub restart_after_failures: u32,
    /// 是否启用自动保活
    pub enabled: bool,
}

impl Default for KeepaliveConfig {
    fn default() -> Self {
        Self {
            interval_sec: 60,
            heartbeat_sec: 15,
            fail_threshold: 3,
            cooldown_sec: 30,
            restart_after_failures: 5,
            enabled: true,
        }
    }
}

/// 日志配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: LogLevel,
    /// 日志目录（相对路径基于 exe 目录）
    pub directory: String,
    /// 保留天数
    pub retain_days: u32,
    /// 是否输出到控制台
    pub console: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            directory: "logs".into(),
            retain_days: 30,
            console: true,
        }
    }
}

/// 带来源信息的配置
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: Config,
    pub path: PathBuf,
}

/// 配置文件指纹，用于热重载检测
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFingerprint {
    pub modified: Option<SystemTime>,
    pub len: u64,
}

impl LoadedConfig {
    /// 计算当前配置文件指纹
    pub fn fingerprint(&self) -> ConfigFingerprint {
        fingerprint_of(&self.path)
    }

    /// 保存配置回文件
    pub fn save(&self) -> Result<()> {
        save_config(&self.config, &self.path)
    }
}

pub fn fingerprint_of(path: &Path) -> ConfigFingerprint {
    match std::fs::metadata(path) {
        Ok(md) => ConfigFingerprint {
            modified: md.modified().ok(),
            len: md.len(),
        },
        Err(_) => ConfigFingerprint {
            modified: None,
            len: 0,
        },
    }
}

/// 配置文件的轻量指纹（修改时间 + 大小）
///
/// 用于守护进程检测"配置是否被界面改过"。
/// 刻意不依赖 `filetime` crate：用纳秒时间戳差值判断，
/// 因为文件系统的精度足够（Windows 上通常 100ns）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    pub secs: u64,
    pub nanos: u32,
    pub len: u64,
}

impl Fingerprint {
    pub fn of(path: &Path) -> Option<Self> {
        let md = std::fs::metadata(path).ok()?;
        let mtime = md.modified().ok()?;
        let d = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Some(Self {
            secs: d.as_secs(),
            nanos: d.subsec_nanos(),
            len: md.len(),
        })
    }
}
/// 运行时状态文件路径（守护进程写、界面读），与配置文件同目录
pub fn status_file_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|d| d.join(".status"))
        .unwrap_or_else(|| PathBuf::from(".status"))
}

/// 唯一的配置目录：`%LOCALAPPDATA%\皎月连保活守护`
///
/// 优先 `LOCALAPPDATA`（Tauri currentUser 安装位置），
/// 缺失时退回 `APPDATA`，两者都没有才用当前目录。
pub fn config_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("APPDATA").ok().filter(|s| !s.is_empty()));
    match base {
        Some(d) => PathBuf::from(d).join(crate::APP_NAME),
        None => PathBuf::from("."),
    }
}

/// 唯一的配置文件路径：`<配置目录>\config.json`
pub fn fallback_config_path() -> Option<PathBuf> {
    Some(config_dir().join("config.json"))
}

/// 解析配置文件路径。
///
/// 顺序：
/// 1. 环境变量 `NATPIERCE_KEEPALIVE_CONFIG`（多实例 / 测试用）
/// 2. `<配置目录>\config.json` —— **唯一权威位置**
///
/// 历史教训：早期版本还会依次探测「当前工作目录」和「exe 所在目录」，
/// 结果同一台机器上并存多份 config.json（项目根、`target\release`、
/// `%LOCALAPPDATA%\皎月连保活守护`），界面改了一份、守护进程读另一份，
/// 表现为「改了配置没反应」。这两个来源已废弃 —— 配置文件只有一个位置。
pub fn resolve_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("NATPIERCE_KEEPALIVE_CONFIG") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    fallback_config_path().unwrap_or_else(|| PathBuf::from("config.json"))
}

/// 加载配置
pub fn load_config() -> Result<LoadedConfig> {
    let path = resolve_config_path();
    load_config_from(&path)
}

/// 从指定路径加载配置
pub fn load_config_from(path: &Path) -> Result<LoadedConfig> {
    if !path.exists() {
        anyhow::bail!(
            "配置文件不存在: {}\n请参考 config.example.json 创建",
            path.display()
        );
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
    // 容忍编辑器/PowerShell 写入的 UTF-8 BOM
    let text = text.trim_start_matches('\u{feff}');
    let config: Config = serde_json::from_str(text)
        .with_context(|| format!("解析配置文件失败: {}", path.display()))?;
    Ok(LoadedConfig {
        config,
        path: path.to_path_buf(),
    })
}

/// 保存配置到指定路径
pub fn save_config(config: &Config, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
        }
    }
    let text = serde_json::to_string_pretty(config)?;
    std::fs::write(path, text)
        .with_context(|| format!("写入配置文件失败: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_roundtrip() {
        let cfg = Config::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mode, Mode::Server);
        assert_eq!(back.api.url, "ws://127.0.0.1:33272/ws");
        assert_eq!(back.server.max_clients, 0);
    }

    #[test]
    fn mode_display() {
        assert_eq!(Mode::Server.to_string(), "服务端");
        assert_eq!(Mode::Client.to_string(), "客户端");
    }
}
