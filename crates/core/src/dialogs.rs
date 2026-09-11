//! Windows 原生对话框辅助

use anyhow::Result;

/// 弹出一个简单的消息框（用于提示）
#[cfg(windows)]
pub fn message_box(title: &str, text: &str, is_error: bool) {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND,
    };

    let t = HSTRING::from(title);
    let x = HSTRING::from(text);
    let icon = if is_error { MB_ICONERROR } else { MB_ICONINFORMATION };

    unsafe {
        MessageBoxW(None, &x, &t, MB_OK | icon | MB_SETFOREGROUND);
    }
}

#[cfg(not(windows))]
pub fn message_box(title: &str, text: &str, _is_error: bool) {
    eprintln!("[{title}] {text}");
}

/// 从标准输入读取一行（用于命令行密码录入）
pub fn read_line_from_stdin() -> Result<String> {
    use std::io::BufRead;
    let mut s = String::new();
    std::io::stdin().lock().read_line(&mut s)?;
    Ok(s.trim_end_matches(['\r', '\n']).to_string())
}
