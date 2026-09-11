//! natpierce-core —— 皎月连保活守护的核心库
//!
//! 提供与界面无关的全部业务能力：
//!
//! | 模块 | 职责 |
//! |---|---|
//! | [`config`] | 配置模型、加载/保存、路径解析 |
//! | [`secret`] | 密码来源解析 + Windows DPAPI 加解密 |
//! | [`api`] | 本地控制接口的协议与 WebSocket 客户端 |
//! | [`keepalive`] | 保活状态机（三层判据 + 分层恢复） |
//! | [`process`] | 进程检测 / 提权启动 / 终止 |
//! | [`autostart`] | 开机自启（HKCU Run） |
//! | [`service`] | Windows 服务管理 |
//! | [`dialogs`] | 原生消息框 |
//! | [`logging`] | 日志初始化（控制台 / 文件） |

pub mod api;
pub mod autostart;
pub mod config;
pub mod dialogs;
pub mod keepalive;
pub mod logging;
pub mod process;
pub mod secret;
pub mod service;
pub mod stop_flag;

pub use config::{Config, LoadedConfig, LogLevel, Mode};
pub use keepalive::{Health, Keepalive};

/// 版本号（取自 Cargo.toml）
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 程序显示名
pub const APP_NAME: &str = "皎月连保活守护";
