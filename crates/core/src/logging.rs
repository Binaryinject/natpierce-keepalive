//! 日志初始化
//!
//! 两种模式：
//! - **控制台模式**（CLI / 调试）：输出到 stdout
//! - **文件模式**（托盘 / 服务）：按天轮转到 `<配置目录>/logs/keepalive.log.YYYY-MM-DD`
//!
//! 之所以要分模式：托盘程序没有控制台，而且 Windows 控制台默认 GBK 编码
//! 会把 UTF-8 的中文打成乱码，写文件反而更可靠。

use crate::config::{Config, LogLevel};
use std::path::{Path, PathBuf};
use tracing_subscriber::EnvFilter;

/// 解析日志目录（相对路径基于配置文件所在目录）
pub fn resolve_log_dir(cfg: &Config, config_path: &Path) -> PathBuf {
    let dir = PathBuf::from(&cfg.logging.directory);
    if dir.is_absolute() {
        return dir;
    }
    config_path
        .parent()
        .map(|p| p.join(&dir))
        .unwrap_or(dir)
}

/// 初始化日志
///
/// - `to_file == false`：写控制台
/// - `to_file == true`：写文件（返回的 guard 必须存活到程序结束，否则日志会被截断）
pub fn init(
    cfg: &Config,
    config_path: &Path,
    override_level: Option<LogLevel>,
    to_file: bool,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let level = override_level.unwrap_or(cfg.logging.level);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("natpierce_core={}", level.as_str())));

    if !to_file {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
                "%Y-%m-%d %H:%M:%S".into(),
            ))
            .try_init();
        return None;
    }

    let log_dir = resolve_log_dir(cfg, config_path);
    if std::fs::create_dir_all(&log_dir).is_err() {
        return None;
    }

    let appender = tracing_appender::rolling::daily(&log_dir, "keepalive.log");
    let (nb, guard) = tracing_appender::non_blocking(appender);

    let ok = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(false)
        .with_writer(nb)
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
            "%Y-%m-%d %H:%M:%S".into(),
        ))
        .try_init()
        .is_ok();

    if ok {
        Some(guard)
    } else {
        None
    }
}
