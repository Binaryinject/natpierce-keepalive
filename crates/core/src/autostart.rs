//! 开机自启动（当前用户，写 HKCU\...\Run）
//!
//! 采用 HKCU Run 键而不是计划任务，好处是：
//! - 无需管理员权限即可开关
//! - 用户可在「任务管理器 → 启动」里看到并自行禁用
//! - 卸载时清理干净
//!
//! 注意：若需要以最高权限（SYSTEM/提权）随系统启动，应由 GUI 提供
//! 「安装为服务」或「注册计划任务」的选项，不走这里。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// 注册表 Run 键路径
#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// 启动项名称
pub const VALUE_NAME: &str = "NatpierceKeepalive";

/// 生成要写入的启动命令行
///
/// 形如：`"C:\path\to\natpierce-keepalive.exe" gui --config "C:\path\config.json"`
pub fn build_command(exe: &Path, config: Option<&Path>) -> String {
    let mut cmd = format!("\"{}\" gui", exe.display());
    if let Some(c) = config {
        cmd.push_str(&format!(" --config \"{}\"", c.display()));
    }
    cmd
}

/// 当前 exe 路径
pub fn current_exe() -> Result<PathBuf> {
    std::env::current_exe().context("获取当前可执行文件路径失败")
}

// ============================================================
// Windows 实现
// ============================================================

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
    };

    fn open_key(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Result<HKEY> {
        let mut key = HKEY::default();
        let sub = HSTRING::from(RUN_KEY);
        let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &sub, None, access, &mut key) };
        if rc != ERROR_SUCCESS {
            anyhow::bail!("打开注册表 Run 键失败: {rc:?}");
        }
        Ok(key)
    }

    /// 读取当前已注册的启动命令（None = 未设置）
    pub fn get() -> Result<Option<String>> {
        let key = open_key(KEY_QUERY_VALUE)?;
        let name = HSTRING::from(VALUE_NAME);

        let mut buf = [0u16; 2048];
        let mut len = (buf.len() * 2) as u32;
        let mut ty = REG_SZ;

        let rc = unsafe {
            RegQueryValueExW(
                key,
                &name,
                None,
                Some(&mut ty),
                Some(buf.as_mut_ptr() as *mut u8),
                Some(&mut len),
            )
        };
        let _ = unsafe { RegCloseKey(key) };

        if rc == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if rc != ERROR_SUCCESS {
            anyhow::bail!("读取启动项失败: {rc:?}");
        }

        let chars = (len as usize / 2).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..chars])
            .trim_end_matches('\0')
            .to_string();
        Ok(if s.is_empty() { None } else { Some(s) })
    }

    /// 写入启动项
    pub fn set(command: &str) -> Result<()> {
        let key = open_key(KEY_SET_VALUE)?;
        let name = HSTRING::from(VALUE_NAME);
        let value = HSTRING::from(command);

        // REG_SZ 需要包含结尾的 NUL
        let bytes = unsafe {
            std::slice::from_raw_parts(
                value.as_ptr() as *const u8,
                (value.len() + 1) * 2,
            )
        };

        let rc = unsafe {
            RegSetValueExW(key, &name, None, REG_SZ, Some(bytes))
        };
        let _ = unsafe { RegCloseKey(key) };

        if rc != ERROR_SUCCESS {
            anyhow::bail!("写入启动项失败: {rc:?}");
        }
        Ok(())
    }

    /// 删除启动项（不存在时返回 Ok）
    pub fn remove() -> Result<()> {
        let key = open_key(KEY_SET_VALUE)?;
        let name = HSTRING::from(VALUE_NAME);
        let rc = unsafe { RegDeleteValueW(key, &name) };
        let _ = unsafe { RegCloseKey(key) };

        if rc != ERROR_SUCCESS && rc != ERROR_FILE_NOT_FOUND {
            anyhow::bail!("删除启动项失败: {rc:?}");
        }
        Ok(())
    }

    /// 是否已启用，并且指向的还是当前这个 exe
    pub fn is_enabled_for(exe: &Path) -> bool {
        match get() {
            Ok(Some(v)) => {
                let exe_lower = exe.to_string_lossy().to_lowercase();
                v.to_lowercase().contains(&exe_lower)
            }
            _ => false,
        }
    }

    // 让 PCWSTR 引入不报警告
    #[allow(dead_code)]
    fn _unused(_p: PCWSTR) {}
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn get() -> Result<Option<String>> {
        Ok(None)
    }
    pub fn set(_command: &str) -> Result<()> {
        anyhow::bail!("开机自启仅支持 Windows")
    }
    pub fn remove() -> Result<()> {
        Ok(())
    }
    pub fn is_enabled_for(_exe: &Path) -> bool {
        false
    }
}

// ---- 对外统一接口 ----

/// 查询启动项原始值
pub fn get() -> Result<Option<String>> {
    imp::get()
}

/// 启用开机自启（指向当前 exe）
pub fn enable(config: Option<&Path>) -> Result<String> {
    let exe = current_exe()?;
    let cmd = build_command(&exe, config);
    imp::set(&cmd)?;
    Ok(cmd)
}

/// 关闭开机自启
pub fn disable() -> Result<()> {
    imp::remove()
}

/// 当前是否已启用（且指向本 exe）
pub fn is_enabled() -> bool {
    match current_exe() {
        Ok(exe) => imp::is_enabled_for(&exe),
        Err(_) => false,
    }
}

/// 切换开关，返回切换后的状态
pub fn toggle(config: Option<&Path>) -> Result<bool> {
    if is_enabled() {
        disable()?;
        Ok(false)
    } else {
        enable(config)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_format() {
        let exe = PathBuf::from(r"C:\app\natpierce-keepalive.exe");
        let cfg = PathBuf::from(r"C:\app\config.json");
        let cmd = build_command(&exe, Some(&cfg));
        assert_eq!(
            cmd,
            r#""C:\app\natpierce-keepalive.exe" gui --config "C:\app\config.json""#
        );

        let cmd2 = build_command(&exe, None);
        assert_eq!(cmd2, r#""C:\app\natpierce-keepalive.exe" gui"#);
    }
}
