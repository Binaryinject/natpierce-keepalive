//! 优雅停止标志
//!
//! 用一个标志文件通知常驻的守护进程"该退出了"。
//!
//! # 为什么不用 taskkill / TerminateProcess
//!
//! 1. **权限问题**：界面可能以普通权限运行，而守护进程是管理员/SYSTEM 启动的，
//!    强杀会失败（`Access is denied`）。文件标志与权限无关。
//! 2. **可清理**：强杀是硬终止，进程没机会收尾；标志文件让守护进程走正常
//!    返回路径，可以打印退出日志、释放资源。
//! 3. **可预期**：界面能明确知道"已请求停止"，而不是"杀了一把，成不成功不知道"。
//!
//! # 用法
//!
//! ```no_run
//! # use std::path::Path;
//! // 界面侧：请求停止
//! natpierce_core::stop_flag::request(Path::new("config.json"));
//! // 守护进程侧：每轮检查
//! if natpierce_core::stop_flag::is_requested(Path::new("config.json")) { return; }
//! ```

use std::path::{Path, PathBuf};

/// 标志文件名（与配置文件同目录）
pub const FLAG_NAME: &str = ".stop";

/// 根据配置文件路径推导标志文件路径
pub fn flag_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|d| d.join(FLAG_NAME))
        .unwrap_or_else(|| PathBuf::from(FLAG_NAME))
}

/// 请求守护进程停止（界面侧调用）
///
/// 返回标志文件路径，便于界面提示用户。
pub fn request(config_path: &Path) -> std::io::Result<PathBuf> {
    let p = flag_path(config_path);
    // 内容写时间戳，便于排查"谁在什么时候请求的"
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    std::fs::write(&p, now)?;
    Ok(p)
}

/// 是否已被请求停止（守护进程侧调用）
pub fn is_requested(config_path: &Path) -> bool {
    flag_path(config_path).exists()
}

/// 清除标志（守护进程启动时调用，避免上次的残留导致一启动就退出）
pub fn clear(config_path: &Path) {
    let _ = std::fs::remove_file(flag_path(config_path));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_path_is_sibling_of_config() {
        let p = flag_path(Path::new(r"D:\app\config.json"));
        assert_eq!(p, PathBuf::from(r"D:\app\.stop"));
    }

    #[test]
    fn request_and_clear_roundtrip() {
        let dir = std::env::temp_dir().join("npk-stopflag-test");
        let _ = std::fs::create_dir_all(&dir);
        let cfg = dir.join("config.json");

        clear(&cfg);
        assert!(!is_requested(&cfg));

        request(&cfg).expect("写入标志失败");
        assert!(is_requested(&cfg));

        clear(&cfg);
        assert!(!is_requested(&cfg));
    }
}
