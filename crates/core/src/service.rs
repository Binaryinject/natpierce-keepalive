//! Windows 服务封装
//!
//! 目标：让保活进程能在**用户未登录**时也持续运行，并且拥有足够权限
//! 启动 `natpierce.exe`（后者要求 `requireAdministrator`）。
//!
//! 实现方式：注册一个由 SCM 托管的服务，服务主体就是本程序的
//! `run --service` 模式。

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// 服务名（SCM 内部标识）
pub const SERVICE_NAME: &str = "NatpierceKeepalive";
/// 服务显示名
pub const DISPLAY_NAME: &str = "皎月连保活守护";
/// 服务描述
pub const DESCRIPTION: &str = "自动检测并恢复皎月连 (natpierce) 的服务端/客户端连接";

/// 查询服务是否已安装
pub fn is_installed() -> bool {
    query_state().is_some()
}

/// 查询服务状态（SCM 返回的原始状态字符串）
pub fn query_state() -> Option<String> {
    let out = Command::new("sc.exe")
        .args(["query", SERVICE_NAME])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // 形如 "STATE              : 4  RUNNING"
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("STATE") {
            if let Some(pos) = l.find(':') {
                return Some(l[pos + 1..].trim().to_string());
            }
        }
    }
    Some("UNKNOWN".into())
}

/// 是否正在运行
pub fn is_running() -> bool {
    query_state()
        .map(|s| s.to_uppercase().contains("RUNNING"))
        .unwrap_or(false)
}

/// 安装服务
///
/// 需要管理员权限。`config` 为配置文件路径，会作为启动参数传给服务。
pub fn install(exe: &Path, config: Option<&Path>) -> Result<()> {
    if !is_elevated() {
        bail!("安装服务需要管理员权限，请以管理员身份运行");
    }
    if is_installed() {
        bail!("服务已存在，请先执行 uninstall");
    }

    let bin_path = match config {
        Some(c) => format!("\"{}\" run --service --config \"{}\"", exe.display(), c.display()),
        None => format!("\"{}\" run --service", exe.display()),
    };

    run_sc(&["create", SERVICE_NAME, "binPath=", &bin_path, "start=", "auto"])?;
    run_sc(&["description", SERVICE_NAME, DESCRIPTION])?;
    // 崩溃后自动重启：失败 3 次，每次间隔 5 秒，之后每天重置计数
    run_sc(&[
        "failure", SERVICE_NAME, "reset=", "86400", "actions=", "restart/5000/restart/5000/restart/5000",
    ])?;
    // 服务显示名
    run_sc(&["description", SERVICE_NAME, DISPLAY_NAME]).ok();

    Ok(())
}

/// 卸载服务
pub fn uninstall() -> Result<()> {
    if !is_elevated() {
        bail!("卸载服务需要管理员权限");
    }
    if !is_installed() {
        bail!("服务不存在");
    }
    // 先停掉
    let _ = run_sc(&["stop", SERVICE_NAME]);
    std::thread::sleep(std::time::Duration::from_secs(2));
    run_sc(&["delete", SERVICE_NAME])?;
    Ok(())
}

/// 启动服务
pub fn start() -> Result<()> {
    run_sc(&["start", SERVICE_NAME]).map(|_| ())
}

/// 停止服务
pub fn stop() -> Result<()> {
    run_sc(&["stop", SERVICE_NAME]).map(|_| ())
}

/// 当前进程是否为管理员
pub fn is_elevated() -> bool {
    crate::process::is_elevated()
}

fn run_sc(args: &[&str]) -> Result<String> {
    let out = Command::new("sc.exe")
        .args(args)
        .output()
        .with_context(|| format!("执行 sc.exe {} 失败", args.join(" ")))?;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    if !out.status.success() {
        bail!(
            "sc.exe {} 失败:\n{}{}",
            args.join(" "),
            stdout.trim(),
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!("\n{}", stderr.trim())
            }
        );
    }
    Ok(stdout)
}
