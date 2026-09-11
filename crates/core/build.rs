//! 构建脚本：嵌入 Windows manifest（版本信息、DPI 感知、兼容性声明）
//!
//! # 关于权限
//!
//! 这里**故意不声明** `requireAdministrator`。
//!
//! 原因：`cargo test` 生成的测试可执行文件会继承主程序的 manifest，
//! 一旦声明了强制提权，测试进程会以
//! `请求的操作需要提升 (os error 740)` 直接失败，整个测试套件跑不起来。
//!
//! 本程序的策略是**按需提权**：
//! - 托盘 / 查询状态等操作以普通权限运行，不打扰用户
//! - 只有需要拉起 `natpierce.exe`（其自身要求管理员权限）时，
//!   才通过 `ShellExecuteW("runas")` 触发一次 UAC
//! - 若希望完全避免 UAC，可安装为 Windows 服务（以 SYSTEM 运行）

use std::path::PathBuf;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR 未设置"),
    )
    .join("natpierce-keepalive.manifest");

    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed=build.rs");

    if !manifest.exists() {
        println!(
            "cargo:warning=manifest 文件不存在，跳过嵌入: {}",
            manifest.display()
        );
        return;
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR 未设置"));
    let rc_file = out_dir.join("app.rc");
    let escaped = manifest.display().to_string().replace('\\', "\\\\");

    // 资源 ID 1 + 类型 24 = RT_MANIFEST
    let rc_content = format!("#pragma code_page(65001)\n1 24 \"{escaped}\"\n");
    if std::fs::write(&rc_file, rc_content).is_err() {
        println!("cargo:warning=写入 .rc 失败，跳过 manifest 嵌入");
        return;
    }

    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let compiled = if target_env == "msvc" {
        compile_with_rc(&rc_file, &out_dir)
    } else {
        compile_with_windres(&rc_file, &out_dir)
    };

    match compiled {
        Some(lib) => {
            println!("cargo:rustc-link-arg={}", lib.display());
            println!("cargo:warning=已嵌入 manifest（版本信息 / DPI 感知，不要求提权）");
        }
        None => println!(
            "cargo:warning=未找到 rc.exe/windres，manifest 未嵌入（不影响功能）"
        ),
    }
}

/// 用 MSVC 的 rc.exe 编译
fn compile_with_rc(rc_file: &PathBuf, out_dir: &PathBuf) -> Option<PathBuf> {
    let res_file = out_dir.join("app.res");
    for rc in find_rc_candidates() {
        if let Ok(s) = std::process::Command::new(&rc)
            .arg("/nologo")
            .arg("/fo")
            .arg(&res_file)
            .arg(rc_file)
            .status()
        {
            if s.success() && res_file.exists() {
                return Some(res_file);
            }
        }
    }
    None
}

/// 在常见位置查找 rc.exe
fn find_rc_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();

    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join("rc.exe");
            if p.exists() {
                out.push(p);
            }
        }
    }

    let pf86 = std::env::var("ProgramFiles(x86)")
        .unwrap_or_else(|_| r"C:\Program Files (x86)".into());

    // vswhere → VS 安装目录下的 MSVC
    let vswhere = PathBuf::from(&pf86).join("Microsoft Visual Studio\\Installer\\vswhere.exe");
    if vswhere.exists() {
        if let Ok(output) = std::process::Command::new(&vswhere)
            .args(["-latest", "-property", "installationPath"])
            .output()
        {
            let base = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !base.is_empty() {
                if let Ok(entries) =
                    std::fs::read_dir(PathBuf::from(&base).join("VC\\Tools\\MSVC"))
                {
                    for e in entries.flatten() {
                        let rc = e.path().join("bin\\Hostx64\\x64\\rc.exe");
                        if rc.exists() {
                            out.push(rc);
                        }
                    }
                }
            }
        }
    }

    // Windows Kits
    if let Ok(entries) = std::fs::read_dir(PathBuf::from(&pf86).join("Windows Kits\\10\\bin")) {
        let mut versions: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        versions.sort();
        versions.reverse();
        for v in versions {
            let rc = v.join("x64\\rc.exe");
            if rc.exists() {
                out.push(rc);
            }
        }
    }

    out
}

/// 用 GNU 工具链的 windres 编译
fn compile_with_windres(rc_file: &PathBuf, out_dir: &PathBuf) -> Option<PathBuf> {
    let obj_file = out_dir.join("app.o");
    let status = std::process::Command::new("windres")
        .arg(rc_file)
        .args(["-O", "coff", "-o"])
        .arg(&obj_file)
        .status()
        .ok()?;
    if status.success() && obj_file.exists() {
        Some(obj_file)
    } else {
        None
    }
}
