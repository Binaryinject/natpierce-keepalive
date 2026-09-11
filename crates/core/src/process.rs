//! Windows 进程管理：检测 / 启动 / 终止
//!
//! 注意：`natpierce.exe` 的清单声明了 `requireAdministrator`，
//! 因此以普通权限 `CreateProcess` 启动它会得到
//! `ERROR_ELEVATION_REQUIRED (740)`。
//! 这里统一用 `ShellExecuteW(..., "runas", ...)` 触发 UAC 提权。

use anyhow::{bail, Result};
use std::path::Path;
use tracing::{info, warn};

/// 检测指定进程是否在运行（不区分大小写，自动补 .exe）
#[cfg(windows)]
pub fn is_running(process_name: &str) -> bool {
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };

    let target = normalize(process_name);

    unsafe {
        let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(e) => {
                warn!("创建进程快照失败: {e}");
                return false;
            }
        };
        if snapshot == INVALID_HANDLE_VALUE {
            warn!("进程快照句柄无效");
            return false;
        }

        let mut entry = PROCESSENTRY32W::default();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        let mut found = false;
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name = wide_to_string(&entry.szExeFile);
                if name.eq_ignore_ascii_case(&target) {
                    found = true;
                    break;
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        let _ = CloseHandle(snapshot);
        found
    }
}

#[cfg(not(windows))]
pub fn is_running(_process_name: &str) -> bool {
    false
}

/// 返回当前所有同名进程的 PID
#[cfg(windows)]
pub fn pids_of(process_name: &str) -> Vec<u32> {
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };

    let target = normalize(process_name);
    let mut out = Vec::new();

    unsafe {
        let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return out,
        };
        if snapshot == INVALID_HANDLE_VALUE {
            return out;
        }

        let mut entry = PROCESSENTRY32W::default();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name = wide_to_string(&entry.szExeFile);
                if name.eq_ignore_ascii_case(&target) {
                    out.push(entry.th32ProcessID);
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    out
}

#[cfg(not(windows))]
pub fn pids_of(_process_name: &str) -> Vec<u32> {
    Vec::new()
}

/// 以提权方式启动程序
///
/// 返回 `Ok(true)` 表示已发起启动；UAC 被拒绝会返回 `Err`
#[cfg(windows)]
pub fn start_elevated(exe: &Path, args: &[String], workdir: &Path) -> Result<()> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    if !exe.exists() {
        bail!("可执行文件不存在: {}", exe.display());
    }

    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(args.join(" "));
    let dir = HSTRING::from(workdir.as_os_str());
    let verb = HSTRING::from("runas"); // 关键：触发提权

    info!(
        "启动: {} {} (工作目录 {})",
        exe.display(),
        args.join(" "),
        workdir.display()
    );

    unsafe {
        let params_ptr: PCWSTR = if args.is_empty() {
            PCWSTR::null()
        } else {
            PCWSTR(params.as_ptr())
        };
        let ret = ShellExecuteW(
            None,
            &verb,
            &file,
            params_ptr,
            &dir,
            SW_SHOWNORMAL,
        );
        // ShellExecuteW 返回值 <= 32 表示失败
        let code = ret.0 as isize;
        if code <= 32 {
            bail!(
                "启动失败 (ShellExecuteW 返回 {code})。\
                 常见原因：UAC 被拒绝(5)、文件不存在(2)、路径错误(3)"
            );
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn start_elevated(_exe: &Path, _args: &[String], _workdir: &Path) -> Result<()> {
    bail!("start_elevated 仅支持 Windows")
}

/// 强制终止所有同名进程
#[cfg(windows)]
pub fn kill_all(process_name: &str) -> Result<usize> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    let pids = pids_of(process_name);
    let mut killed = 0;

    for pid in pids {
        unsafe {
            match OpenProcess(PROCESS_TERMINATE, false, pid) {
                Ok(handle) => {
                    if TerminateProcess(handle, 1).is_ok() {
                        info!("已终止 PID {pid}");
                        killed += 1;
                    } else {
                        warn!("终止 PID {pid} 失败（可能权限不足）");
                    }
                    let _ = CloseHandle(handle);
                }
                Err(e) => {
                    warn!("打开 PID {pid} 失败: {e}（通常是没有管理员权限）");
                }
            }
        }
    }
    Ok(killed)
}

#[cfg(not(windows))]
pub fn kill_all(_process_name: &str) -> Result<usize> {
    Ok(0)
}

/// 当前进程是否具有管理员权限
#[cfg(windows)]
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut ret_len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}

// ---- 工具 ----

fn normalize(name: &str) -> String {
    let n = name.trim();
    if n.to_ascii_lowercase().ends_with(".exe") {
        n.to_string()
    } else {
        format!("{n}.exe")
    }
}

#[cfg(windows)]
fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// 一份便于日志展示的进程概况
pub fn describe(process_name: &str) -> String {
    let pids = pids_of(process_name);
    if pids.is_empty() {
        format!("{process_name}: 未运行")
    } else {
        format!("{process_name}: 运行中 (PID {})", join_pids(&pids))
    }
}

fn join_pids(pids: &[u32]) -> String {
    pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_exe() {
        assert_eq!(normalize("natpierce"), "natpierce.exe");
        assert_eq!(normalize("natpierce.exe"), "natpierce.exe");
        assert_eq!(normalize("NatPierce.EXE"), "NatPierce.EXE");
    }

    #[cfg(windows)]
    #[test]
    fn detect_self_process() {
        // 当前测试进程的名字应当能被枚举到
        let me = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_default();
        if !me.is_empty() {
            assert!(is_running(&me), "应当能检测到自己: {me}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn detect_nonexistent() {
        assert!(!is_running("definitely_not_a_real_process_xyz"));
        assert!(pids_of("definitely_not_a_real_process_xyz").is_empty());
    }
}
