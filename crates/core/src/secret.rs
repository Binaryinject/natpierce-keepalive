//! 密码存储：Windows DPAPI 加解密 + 多来源解析
//!
//! 设计目标：配置文件里**不出现明文密码**。
//!
//! 支持三种引用写法：
//!   - `"dpapi"`          从 DPAPI 加密文件读取（推荐，绑定当前用户）
//!   - `"env:VAR_NAME"`   从环境变量读取
//!   - `"file:path"`      从独立文件读取（该文件需自行 gitignore）
//!   - 其它               视为明文（方便调试，但不推荐）

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// 密码来源解析结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSource {
    /// DPAPI 加密文件
    Dpapi,
    /// 环境变量
    Env(String),
    /// 外部文件
    File(PathBuf),
    /// 明文
    Plain(String),
    /// 空
    Empty,
}

/// 解析引用表达式
pub fn parse_reference(value: &str) -> SecretSource {
    let v = value.trim();
    if v.is_empty() {
        return SecretSource::Empty;
    }
    if v.eq_ignore_ascii_case("dpapi") {
        return SecretSource::Dpapi;
    }
    if let Some(rest) = v.strip_prefix("env:") {
        return SecretSource::Env(rest.trim().to_string());
    }
    if let Some(rest) = v.strip_prefix("file:") {
        return SecretSource::File(PathBuf::from(rest.trim()));
    }
    SecretSource::Plain(v.to_string())
}

/// 解析密码引用，得到明文
pub fn resolve(value: &str, secrets_path: &Path) -> Result<String> {
    match parse_reference(value) {
        SecretSource::Empty => Ok(String::new()),
        SecretSource::Plain(s) => Ok(s),
        SecretSource::Env(name) => {
            let v = std::env::var(&name)
                .with_context(|| format!("环境变量 {name} 未设置"))?;
            Ok(v)
        }
        SecretSource::File(p) => {
            let text = std::fs::read_to_string(&p)
                .with_context(|| format!("读取密码文件失败: {}", p.display()))?;
            Ok(text.trim().to_string())
        }
        SecretSource::Dpapi => load_dpapi(secrets_path),
    }
}

/// DPAPI 加密文件默认路径（与 config.json 同目录）
pub fn default_secrets_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|p| p.join("secrets.dpapi"))
        .unwrap_or_else(|| PathBuf::from("secrets.dpapi"))
}

// ============================================================
// DPAPI 实现 (Windows)
// ============================================================

#[cfg(windows)]
mod dpapi {
    use anyhow::{bail, Result};
    use std::ffi::c_void;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };

    /// 把字节切片包装成 CRYPT_INTEGER_BLOB
    fn blob_of(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }

    /// 把输出 blob 转成 Vec 并释放系统内存
    unsafe fn take_blob(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = if out.pbData.is_null() || out.cbData == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec()
        };
        if !out.pbData.is_null() {
            let _ = LocalFree(Some(HLOCAL(out.pbData as *mut c_void)));
        }
        v
    }

    /// 用当前用户的 DPAPI 密钥加密
    pub fn protect(plaintext: &[u8]) -> Result<Vec<u8>> {
        let input = blob_of(plaintext);
        let mut output = CRYPT_INTEGER_BLOB::default();

        // 说明文字（可选的熵/描述），保持为 None
        let ok = unsafe {
            CryptProtectData(
                &input,
                None,
                None,
                None,
                None,
                0,
                &mut output,
            )
        };

        if ok.is_err() {
            bail!("DPAPI CryptProtectData 失败: {:?}", ok);
        }
        Ok(unsafe { take_blob(output) })
    }

    /// 用当前用户的 DPAPI 密钥解密
    pub fn unprotect(ciphertext: &[u8]) -> Result<Vec<u8>> {
        let input = blob_of(ciphertext);
        let mut output = CRYPT_INTEGER_BLOB::default();

        let ok = unsafe {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                0,
                &mut output,
            )
        };

        if ok.is_err() {
            bail!(
                "DPAPI CryptUnprotectData 失败: {:?}\n\
                 常见原因：密文由其它 Windows 用户/其它机器加密，或文件被篡改",
                ok
            );
        }
        Ok(unsafe { take_blob(output) })
    }
}

#[cfg(windows)]
pub fn save_dpapi(path: &Path, plaintext: &str) -> Result<()> {
    let cipher = dpapi::protect(plaintext.as_bytes())?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, &cipher)
        .with_context(|| format!("写入加密文件失败: {}", path.display()))?;
    Ok(())
}

#[cfg(windows)]
pub fn load_dpapi(path: &Path) -> Result<String> {
    if !path.exists() {
        bail!(
            "DPAPI 密文文件不存在: {}\n请先用 `set-password` 子命令录入密码",
            path.display()
        );
    }
    let cipher = std::fs::read(path)
        .with_context(|| format!("读取加密文件失败: {}", path.display()))?;
    let plain = dpapi::unprotect(&cipher)?;
    String::from_utf8(plain).context("解密结果不是合法 UTF-8")
}

// ---- 非 Windows 平台的占位实现 ----
#[cfg(not(windows))]
pub fn save_dpapi(_path: &Path, _plaintext: &str) -> Result<()> {
    anyhow::bail!("DPAPI 仅支持 Windows 平台")
}

#[cfg(not(windows))]
pub fn load_dpapi(_path: &Path) -> Result<String> {
    anyhow::bail!("DPAPI 仅支持 Windows 平台")
}
// ============================================================
// 多密钥存储（登录密码 / 页面密码分开保存）
// ============================================================

/// 密钥用途：
/// - `page`      页面访问密码（开启服务端用）
/// - `login`     皎月连登录密码（自动登录用）
/// - `connection` 连接密码
pub const KEY_PAGE: &str = "page";
pub const KEY_LOGIN: &str = "login";
pub const KEY_CONNECTION: &str = "connection";

/// 读取多密钥文件（JSON: {"page":"密文base64","login":"..."}）
fn load_vault(path: &Path) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return map;
    };
    if let Ok(v) = serde_json::from_str::<std::collections::HashMap<String, String>>(&text) {
        return v;
    }
    // 兼容旧的单密码格式（整个文件是 base64 密文）
    if let Ok(plain) = load_dpapi_legacy(path) {
        map.insert(KEY_PAGE.to_string(), plain);
    }
    map
}

/// 旧格式读取（文件内容直接是 DPAPI 密文的 base64）
fn load_dpapi_legacy(path: &Path) -> Result<String> {
    let b64 = std::fs::read_to_string(path)?;
    let cipher = base64_decode(b64.trim())?;
    let plain = dpapi_unprotect(&cipher)?;
    String::from_utf8(plain).context("解密结果不是合法 UTF-8")
}

/// 保存某个用途的密码
pub fn save_key(path: &Path, key: &str, plaintext: &str) -> Result<()> {
    let mut vault = load_vault(path);
    let cipher = dpapi_protect(plaintext.as_bytes())?;
    vault.insert(key.to_string(), base64_encode(&cipher));
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let text = serde_json::to_string_pretty(&vault)?;
    std::fs::write(path, text)?;
    Ok(())
}

/// 读取某个用途的密码
pub fn load_key(path: &Path, key: &str) -> Result<String> {
    let vault = load_vault(path);
    // 密钥不存在时要明确报错，而不是静默返回空串 ——
    // 否则上层会发出"空密码"的请求，报出与真实原因无关的错误
    // （例如把"页面密码没保存"表现成 "DPAPI 数据无效"）。
    let Some(b64) = vault.get(key) else {
        anyhow::bail!(
            "尚未保存{}，请在界面里填写后点「保存配置」",
            match key {
                KEY_PAGE => "页面访问密码",
                KEY_LOGIN => "登录密码",
                KEY_CONNECTION => "连接密码",
                _ => "该密码",
            }
        );
    };
    let cipher = base64_decode(b64)?;
    let plain = dpapi_unprotect(&cipher)?;
    String::from_utf8(plain).context("解密结果不是合法 UTF-8")
}

// ---- DPAPI 与 base64 的平台无关包装 ----

#[cfg(windows)]
fn dpapi_protect(data: &[u8]) -> Result<Vec<u8>> {
    dpapi::protect(data)
}
#[cfg(windows)]
fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>> {
    dpapi::unprotect(data)
}
#[cfg(not(windows))]
fn dpapi_protect(_d: &[u8]) -> Result<Vec<u8>> {
    anyhow::bail!("DPAPI 仅支持 Windows")
}
#[cfg(not(windows))]
fn dpapi_unprotect(_d: &[u8]) -> Result<Vec<u8>> {
    anyhow::bail!("DPAPI 仅支持 Windows")
}

fn base64_encode(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn base64_decode(s: &str) -> Result<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        if chunk.len() < 4 {
            break;
        }
        let mut n = 0u32;
        let mut pad = 0;
        for (i, &c) in chunk.iter().enumerate() {
            if c == b'=' {
                pad += 1;
                n <<= 6;
            } else {
                n = (n << 6) | val(c).ok_or_else(|| anyhow::anyhow!("非法 base64 字符: {}", c as char))?;
            }
            let _ = i;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_references() {
        assert_eq!(parse_reference(""), SecretSource::Empty);
        assert_eq!(parse_reference("  "), SecretSource::Empty);
        assert_eq!(parse_reference("dpapi"), SecretSource::Dpapi);
        assert_eq!(parse_reference("DPAPI"), SecretSource::Dpapi);
        assert_eq!(
            parse_reference("env:MY_PWD"),
            SecretSource::Env("MY_PWD".into())
        );
        assert_eq!(
            parse_reference("file:./secrets.json"),
            SecretSource::File(PathBuf::from("./secrets.json"))
        );
        assert_eq!(
            parse_reference("plaintext123"),
            SecretSource::Plain("plaintext123".into())
        );
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip() {
        let secret = "测试密码-abc123!@#";
        let cipher = dpapi::protect(secret.as_bytes()).expect("encrypt");
        assert_ne!(cipher, secret.as_bytes());
        let plain = dpapi::unprotect(&cipher).expect("decrypt");
        assert_eq!(String::from_utf8(plain).unwrap(), secret);
    }

    /// 诊断用：逐环节拆解真实 vault 的解密过程
    #[cfg(windows)]
    #[test]
    fn probe_real_vault() {
        let p = std::path::Path::new(r"D:\GIT\natpierce-keepalive\target\release\secrets.dpapi");
        if !p.exists() {
            eprintln!("跳过：{} 不存在", p.display());
            return;
        }
        let text = std::fs::read_to_string(p).unwrap();
        eprintln!("[1] 文件 {} 字节，前 60 字符: {:?}", text.len(), &text[..text.len().min(60)]);

        let parsed = serde_json::from_str::<std::collections::HashMap<String, String>>(&text);
        eprintln!("[2] serde_json 解析: {:?}", parsed.as_ref().map(|m| m.keys().cloned().collect::<Vec<_>>()));

        let vault = load_vault(p);
        eprintln!("[3] load_vault keys = {:?}", vault.keys().collect::<Vec<_>>());

        for (k, v) in &vault {
            eprintln!("--- key={k}, base64 长度={} ---", v.len());
            match base64_decode(v) {
                Ok(b) => {
                    eprintln!("[4] base64 解码 = {} 字节, 头 8 字节 = {:02x?}", b.len(), &b[..b.len().min(8)]);
                    eprintln!("    期望 230 字节: {}", if b.len() == 230 { "OK" } else { "*** 不符 ***" });
                    match dpapi::unprotect(&b) {
                        Ok(plain) => eprintln!("[5] DPAPI 解密成功 = {:?}", String::from_utf8_lossy(&plain)),
                        Err(e) => eprintln!("[5] DPAPI 解密失败 = {e:?}"),
                    }
                }
                Err(e) => eprintln!("[4] base64 解码失败 = {e:?}"),
            }
        }
    }
}
