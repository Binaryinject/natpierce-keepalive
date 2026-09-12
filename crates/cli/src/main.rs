//! 皎月连 (natpierce) 保活守护
//!
//! 服务端 / 客户端双模式，同一个可执行文件。
//!
//! Release 构建使用 Windows GUI 子系统（双击不弹控制台黑窗），
//! 但命令行调用时通过 `AttachConsole` 附加到父进程控制台以显示输出。

// Debug 保留控制台便于开发；Release 用 GUI 子系统避免托盘模式闪黑窗
// 部分封装是给将来扩展留的 API（服务控制、客户端断开等），此处集中豁免
#![allow(dead_code)]


use anyhow::{bail, Context, Result};
use natpierce_core::api::protocol;
use natpierce_core::{Config, LoadedConfig, LogLevel, Mode};
use std::path::PathBuf;
use std::time::Duration;
use tracing::{error, warn};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Release（GUI 子系统）下若从命令行启动，附加到父进程控制台以便打印输出
///
/// 注意：仅 `AttachConsole` 还不够 —— Rust 的 stdout 在进程启动时已经绑定，
/// 必须把 `CONOUT$` 重新设为标准输出句柄，否则 `println!` 仍然写不出去。
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console() {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::GENERIC_WRITE;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{
        AttachConsole, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };

    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return; // 没有父控制台（例如双击运行），保持无控制台
        }
        let name = HSTRING::from("CONOUT$");
        if let Ok(handle) = CreateFileW(
            &name,
            GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        ) {
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, handle);
            let _ = SetStdHandle(STD_ERROR_HANDLE, handle);
        }
    }
}

#[cfg(not(all(windows, not(debug_assertions))))]
#[allow(dead_code)]
fn attach_parent_console() {}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // ⚠️ 日志必须在这里初始化并让 guard 活到进程结束。
    // 早先把 init 放在 dispatch 的分支里，guard 会在分支返回时被 drop，
    // 导致非阻塞日志线程终止——长驻命令（run/daemon）全程无日志输出。
    let _log_guard = {
        let cfg_path = natpierce_core::config::resolve_config_path();
        match natpierce_core::config::load_config_from(&cfg_path) {
            Ok(loaded) => natpierce_core::logging::init(
                &loaded.config,
                &loaded.path,
                None,
                true, // 守护进程无控制台，日志一律写文件
            ),
            Err(_) => None,
        }
    };

    // 有命令行参数 = 当作 CLI 使用，需要控制台输出
    if !args.is_empty() {
        attach_parent_console();
    }

    let mut config_path: Option<PathBuf> = None;
    let mut verbosity: Option<LogLevel> = None;
    let mut mode_override: Option<Mode> = None;
    let mut service_mode = false;
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("错误: --config 需要一个路径参数");
                    std::process::exit(2);
                }
                config_path = Some(PathBuf::from(&args[i]));
            }
            "--mode" | "-m" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("错误: --mode 需要 server 或 client");
                    std::process::exit(2);
                }
                mode_override = match args[i].to_ascii_lowercase().as_str() {
                    "server" | "s" => Some(Mode::Server),
                    "client" | "c" => Some(Mode::Client),
                    other => {
                        eprintln!("错误: 未知模式 {other}（可选 server / client）");
                        std::process::exit(2);
                    }
                };
            }
            "--verbose" | "-v" => verbosity = Some(LogLevel::Debug),
            "--trace" => verbosity = Some(LogLevel::Trace),
            "--quiet" | "-q" => verbosity = Some(LogLevel::Warn),
            "--service" => service_mode = true,
            "--help" | "-h" => {
                print_help();
                return;
            }
            "--version" | "-V" => {
                println!("natpierce-keepalived {VERSION}");
                return;
            }
            other => positional.push(other.to_string()),
        }
        i += 1;
    }

    // 无任何参数 → 默认进入托盘模式
    // （有参数时才走命令行，保持脚本可用性）
    let command = positional.first().map(|s| s.as_str()).unwrap_or("gui");
    let rest = if positional.len() > 1 {
        &positional[1..]
    } else {
        &[]
    };

    if let Err(e) = dispatch(
        command,
        rest,
        config_path,
        verbosity,
        mode_override,
        service_mode,
    ) {
        error!("{e:#}");
        eprintln!("\n[错误] {e:#}");
        std::process::exit(1);
    }
}

fn dispatch(
    command: &str,
    rest: &[String],
    config_path: Option<PathBuf>,
    verbosity: Option<LogLevel>,
    mode_override: Option<Mode>,
    service_mode: bool,
) -> Result<()> {
    // 统一的加载闭包：支持 --mode 覆盖配置里的模式
    let load = |cp: Option<PathBuf>| -> Result<LoadedConfig> {
        let mut loaded = match cp {
            Some(p) => natpierce_core::config::load_config_from(&p)?,
            None => natpierce_core::config::load_config()?,
        };
        if let Some(m) = mode_override {
            loaded.config.mode = m;
        }
        Ok(loaded)
    };

    match command {
        "help" => {
            print_help();
            Ok(())
        }
        "version" => {
            println!("natpierce-keepalived {VERSION}");
            Ok(())
        }
        "init" => cmd_init(config_path),
        "set-password" => cmd_set_password(rest, config_path),
        "autostart" => cmd_autostart(rest, config_path),
        "status" => {
            let loaded = load(config_path)?;
                        run_async(cmd_status(loaded))
        }
        "hosts" => {
            let loaded = load(config_path)?;
                        run_async(cmd_hosts(loaded))
        }
        "ensure" => {
            let loaded = load(config_path)?;
                        run_async(cmd_ensure(loaded))
        }
        "run" => {
            let loaded = load(config_path)?;
            // 服务模式写文件日志；前台 CLI 模式输出到控制台
                        if service_mode {
                cmd_daemon(loaded)
            } else {
                run_async(cmd_run(loaded))
            }
        }
        // 图形界面已拆分为独立程序 natpierce-gui.exe（Tauri）
        "daemon" => {
            let loaded = load(config_path)?;
                        cmd_daemon(loaded)
        }"service" => cmd_service(rest),
        other => {
            eprintln!("未知命令: {other}\n");
            print_help();
            std::process::exit(2);
        }
    }
}


fn run_async<F: std::future::Future<Output = Result<()>>>(f: F) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("创建 tokio 运行时失败")?;
    rt.block_on(f)
}

// ============================================================
// 子命令：init
// ============================================================

fn cmd_init(config_path: Option<PathBuf>) -> Result<()> {
    let path = config_path.unwrap_or_else(natpierce_core::config::resolve_config_path);
    if path.exists() {
        bail!("配置文件已存在，未覆盖: {}", path.display());
    }
    let cfg = Config::default();
    natpierce_core::config::save_config(&cfg, &path)?;
    println!("已生成默认配置: {}", path.display());
    println!();
    println!("下一步：");
    println!("  1. 编辑配置里的 natpierce.exe 路径与模式（server / client）");
    println!("  2. 录入页面访问密码：  natpierce-keepalived set-password page");
    println!("  3. 查看状态：          natpierce-keepalived status");
    Ok(())
}

// ============================================================
// 子命令：set-password
// ============================================================

fn cmd_set_password(rest: &[String], config_path: Option<PathBuf>) -> Result<()> {
    let which = rest.first().map(|s| s.as_str()).unwrap_or("page");
    let is_page = match which {
        "page" | "page-password" => true,
        "connection" | "con" => false,
        other => bail!("未知的密码类型: {other}（可选 page / connection）"),
    };

    let path = config_path.unwrap_or_else(natpierce_core::config::resolve_config_path);
    let secrets = natpierce_core::secret::default_secrets_path(&path);

    print!(
        "请输入 {} 的明文: ",
        if is_page { "页面访问密码" } else { "连接密码" }
    );
    use std::io::Write;
    std::io::stdout().flush().ok();
    let plain = read_password()?;
    if plain.is_empty() {
        bail!("密码为空，已取消");
    }

    natpierce_core::secret::save_dpapi(&secrets, &plain)?;
    println!("已用 DPAPI 加密保存到: {}", secrets.display());
    println!("该密文只能由当前 Windows 用户在本机解密。");
    println!();

    if path.exists() {
        let mut loaded = natpierce_core::config::load_config_from(&path)?;
        let field = if is_page {
            &mut loaded.config.server.page_password
        } else {
            &mut loaded.config.server.connection_password
        };
        if !field.eq_ignore_ascii_case("dpapi") {
            *field = "dpapi".into();
            loaded.save()?;
            println!("已同步更新配置为 \"dpapi\" 引用");
        }
    }
    Ok(())
}

fn read_password() -> Result<String> {
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).context("读取输入失败")?;
    Ok(s.trim_end_matches(['\r', '\n']).to_string())
}

// ============================================================
// 子命令：autostart
// ============================================================

fn cmd_autostart(rest: &[String], config_path: Option<PathBuf>) -> Result<()> {
    let action = rest.first().map(|s| s.as_str()).unwrap_or("status");
    let cfg_path = config_path.unwrap_or_else(natpierce_core::config::resolve_config_path);

    match action {
        "status" => {
            let enabled = natpierce_core::autostart::is_enabled();
            println!(
                "开机自启: {}",
                if enabled { "[已启用]" } else { "[未启用]" }
            );
            match natpierce_core::autostart::get()? {
                Some(cmd) => println!("注册命令: {cmd}"),
                None => println!("注册命令: （无）"),
            }
            let me = natpierce_core::autostart::current_exe()?;
            println!("当前程序: {}", me.display());
            if let Ok(Some(cmd)) = natpierce_core::autostart::get() {
                if !cmd.to_lowercase().contains(&me.to_string_lossy().to_lowercase()) {
                    println!("注意: 启动项指向的不是当前程序，建议重新启用");
                }
            }
        }
        "on" | "enable" => {
            let cmd = natpierce_core::autostart::enable(Some(&cfg_path))?;
            println!("已启用开机自启");
            println!("  {cmd}");
            println!();
            println!("可在「任务管理器 -> 启动」中查看/禁用。");
        }
        "off" | "disable" => {
            natpierce_core::autostart::disable()?;
            println!("已关闭开机自启");
        }
        "toggle" => {
            let now = natpierce_core::autostart::toggle(Some(&cfg_path))?;
            println!(
                "开机自启: {}",
                if now { "[已启用]" } else { "[已关闭]" }
            );
        }
        other => bail!("未知参数: {other}（可选 on / off / status / toggle）"),
    }
    Ok(())
}

// ============================================================
// 子命令：status
// ============================================================

async fn cmd_status(loaded: LoadedConfig) -> Result<()> {
    let cfg = &loaded.config;

    println!("natpierce-keepalive v{VERSION}");
    println!("配置文件  : {}", loaded.path.display());
    println!("运行模式  : {}", cfg.mode);
    println!("进程状态  : {}", natpierce_core::process::describe(&cfg.natpierce.process_name));
    println!(
        "管理员权限: {}",
        if natpierce_core::process::is_elevated() {
            "是"
        } else {
            "否（无法自动启动皎月连）"
        }
    );
    println!(
        "开机自启  : {}",
        if natpierce_core::autostart::is_enabled() { "已启用" } else { "未启用" }
    );
    println!();

    match probe_api(cfg).await {
        Ok(r) => {
            let state = match r.server_running {
                Some(true) => "已启动",
                Some(false) => "未启动",
                None => "无法判定",
            };
            println!("服务端状态: {state}");
            if let Some(info) = &r.server_info {
                println!("  账号     : {}", info.account);
                println!("  识别码   : {}", info.identification);
                println!("  主机名   : {}", info.machine_name);
                println!("  公网IP   : {}", info.public_ip);
                println!("  映射     : {}", info.mappings);
            }
            if !r.hosts.is_empty() {
                println!("在线主机 ({} 台):", r.hosts.len());
                for (n, h) in r.hosts.iter().enumerate() {
                    println!("  {}. {}", n + 1, h.name);
                }
            }
            for info in &r.infos {
                println!("提示     : {info}");
            }
        }
        Err(e) => {
            println!("服务端状态: 无法连接本地接口");
            println!("  原因: {e:#}");
            println!(
                "  提示: 请确认皎月连正在运行，且控制端口为 {}",
                protocol::DEFAULT_PORT
            );
        }
    }
    Ok(())
}

// ============================================================
// 子命令：hosts
// ============================================================

async fn cmd_hosts(loaded: LoadedConfig) -> Result<()> {
    let cfg = &loaded.config;
    let r = probe_api(cfg).await?;
    if r.hosts.is_empty() {
        println!("未发现在线主机。");
        println!("（提示：需要至少一台其它设备登录皎月连并开启服务端）");
        return Ok(());
    }
    println!("在线主机 ({} 台):", r.hosts.len());
    println!("{:<4} {:<28} {}", "序号", "名称", "映射");
    for (n, h) in r.hosts.iter().enumerate() {
        println!("{:<4} {:<28} {}", n + 1, h.name, h.mappings);
    }
    println!();
    println!("把「名称」填进 config.json 的 client.target_host_name 即可锁定目标。");
    println!("注：会话编号每次接入都会重新分配，组网虚拟 IP 协议里不提供，");
    println!("    所以主机名是唯一重启不变的标识。");
    Ok(())
}

// ============================================================
// 子命令：ensure
// ============================================================

async fn cmd_ensure(loaded: LoadedConfig) -> Result<()> {
    let cfg = &loaded.config;

    if cfg.mode != Mode::Server {
        bail!(
            "ensure 仅在服务端模式可用（当前模式: {}）。\n\
             如需切换：natpierce-keepalived ensure --mode server",
            cfg.mode
        );
    }

    let r = probe_api(cfg).await?;
    if r.server_running == Some(true) {
        println!("OK: 服务端已在运行");
        return Ok(());
    }

    println!(
        "检测到服务端未启动，尝试开启（最多等待 {} 秒）…",
        cfg.server.start_timeout_sec
    );

    let page_pwd = natpierce_core::secret::resolve_key(
        &cfg.server.page_password,
        &natpierce_core::secret::default_secrets_path(&loaded.path),
        natpierce_core::secret::KEY_PAGE,
    )
    .context("解析页面访问密码失败")?;

    if page_pwd.is_empty() {
        bail!("页面访问密码为空。请先执行: natpierce-keepalived set-password page");
    }
    if page_pwd.chars().count() < 6 {
        warn!("页面访问密码长度不足 6 位，服务端可能拒绝");
    }

    let mut client = natpierce_core::api::ApiClient::connect(
        &cfg.api.url,
        &cfg.api.fallback_url,
        Duration::from_millis(cfg.api.connect_timeout_ms),
        Duration::from_millis(cfg.api.command_timeout_ms),
    )
    .await?;

    let start_cmd = protocol::cmd_start_server(
        &cfg.server.connection_password,
        cfg.server.max_clients,
        &page_pwd,
        &cfg.server.lan_ip,
    );
    client.send(&start_cmd).await?;

    // 服务端启动需要十几秒（建虚拟网卡 + 向云端注册），必须一直等到明确结论
    let deadline = std::time::Instant::now() + Duration::from_secs(cfg.server.start_timeout_sec);
    let mut verdict: Option<bool> = None;
    let mut seen: Vec<String> = Vec::new();

    while std::time::Instant::now() < deadline && verdict.is_none() {
        match client.next_message(Duration::from_millis(1500)).await {
            Some(m) => {
                if let Some(running) = m.is_server_running() {
                    if running {
                        verdict = Some(true);
                    }
                }
                seen.push(format!("{m:?}"));
            }
            None => continue,
        }
    }
    client.close().await;

    match verdict {
        Some(true) => {
            println!("服务端已成功开启");
            Ok(())
        }
        _ => {
            eprintln!(
                "未在 {} 秒内收到“已启动”确认，共收到 {} 条消息:",
                cfg.server.start_timeout_sec,
                seen.len()
            );
            let tail: Vec<_> = seen.iter().rev().take(8).collect();
            for m in tail.into_iter().rev() {
                eprintln!("   {m}");
            }
            bail!("开启失败（请检查页面访问密码，或查看皎月连界面连接日志）");
        }
    }
}

// ============================================================
// 子命令：run（保活循环）
// ============================================================

async fn cmd_run(loaded: LoadedConfig) -> Result<()> {
    let cfg = loaded.config.clone();
    println!("进入保活循环（Ctrl+C 退出）");
    println!("  模式    : {}", cfg.mode);
    println!("  巡检间隔: {} 秒", cfg.keepalive.interval_sec);
    println!("  心跳间隔: {} 秒", cfg.keepalive.heartbeat_sec);
    println!();

    let mut ka = natpierce_core::Keepalive::new(loaded);
    let (tx, rx) = tokio::sync::watch::channel(false);

    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            println!("\n收到 Ctrl+C，正在退出…");
            let _ = tx.send(true);
        }
    });

    ka.run(rx).await
}

/// Windows 服务主体：无控制台，日志写文件
fn cmd_daemon(loaded: LoadedConfig) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("创建运行时失败")?;
    rt.block_on(async move {
        let mut ka = natpierce_core::Keepalive::new(loaded);
        let (_tx, rx) = tokio::sync::watch::channel(false);
        ka.run(rx).await
    })
}

// ============================================================
// 子命令：service（Windows 服务管理）
// ============================================================

fn cmd_service(rest: &[String]) -> Result<()> {
    let action = rest.first().map(|s| s.as_str()).unwrap_or("status");
    let exe = std::env::current_exe().context("获取当前程序路径失败")?;
    let cfg_path = natpierce_core::config::resolve_config_path();

    match action {
        "status" => {
            println!("服务名  : {}", natpierce_core::service::SERVICE_NAME);
            println!("显示名  : {}", natpierce_core::service::DISPLAY_NAME);
            match natpierce_core::service::query_state() {
                Some(s) => println!("状态    : {s}"),
                None => println!("状态    : 未安装"),
            }
            println!("管理员  : {}", if natpierce_core::service::is_elevated() { "是" } else { "否" });
        }
        "install" => {
            natpierce_core::service::install(&exe, Some(&cfg_path))?;
            println!("服务已安装");
            println!("  启动: natpierce-keepalived service start");
            println!("  或  : sc.exe start {}", natpierce_core::service::SERVICE_NAME);
        }
        "uninstall" | "remove" => {
            natpierce_core::service::uninstall()?;
            println!("服务已卸载");
        }
        "start" => {
            natpierce_core::service::start()?;
            println!("服务已启动");
        }
        "stop" => {
            natpierce_core::service::stop()?;
            println!("服务已停止");
        }
        other => bail!("未知参数: {other}（可选 install / uninstall / start / stop / status）"),
    }
    Ok(())
}

// ============================================================
// 探测辅助
// ============================================================

async fn probe_api(cfg: &Config) -> Result<natpierce_core::api::ProbeResult> {
    natpierce_core::api::probe(
        &cfg.api.url,
        &cfg.api.fallback_url,
        Duration::from_millis(cfg.api.connect_timeout_ms),
        Duration::from_millis(cfg.api.command_timeout_ms),
    )
    .await
}

// ============================================================
// 帮助
// ============================================================

fn print_help() {
    println!(
        r#"natpierce-keepalive — 守护进程 / 命令行  v{VERSION}

用法:
  natpierce-keepalived <命令> [选项]

说明:
  本程序是**守护进程**，没有任何图形界面。
  图形界面是独立程序 natpierce-gui.exe（Tauri + Web UI），
  它通过启动/停止本进程来控制保活。

命令:
  status              查看当前状态（进程 / 服务端 / 在线主机）
  hosts               列出在线主机及其组网虚拟 IP
  ensure              若服务端未启动则开启它（幂等，适合一次性修复）
  run                 进入保活循环（前台，Ctrl+C 退出；服务模式由 SCM 调用）
  daemon              同 run，但用于后台常驻（不做 Ctrl+C 处理）
  set-password <类型>  录入密码并用 DPAPI 加密保存
                        类型: page（页面访问密码）| connection（连接密码）
  autostart <动作>     开机自启管理: on / off / status / toggle
  service <动作>       Windows 服务管理: install / uninstall / start / stop / status
  init                生成默认配置文件
  version             显示版本
  help                显示本帮助

选项:
  -m, --mode <模式>    覆盖配置里的模式: server（服务端）| client（客户端）
  -c, --config <路径>  指定配置文件（默认自动查找 config.json）
  -v, --verbose        Debug 级日志
      --trace          Trace 级日志
  -q, --quiet          仅显示警告与错误

保活策略:
  服务端与客户端互斥，同一时刻只能选一个。
  - 服务端模式：检测到 startServer 未运行 → 自动开启
  - 客户端模式：检测到与目标主机的连接断开 → 自动重连
  三层判据：进程层 → 服务层 → 心跳层，逐级定位并恢复。

示例:
  natpierce-keepalived status
  natpierce-keepalived run                  # 前台保活（可看实时日志）
  natpierce-keepalived ensure --mode server
  natpierce-keepalived set-password page
  natpierce-keepalived service install      # 装成服务，开机自启 + 崩溃自愈

相关程序:
  natpierce-gui.exe    图形界面（托盘 + 设置窗口），推荐日常使用
"#
    );
}