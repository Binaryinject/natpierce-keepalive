//! 皎月连保活守护 — Tauri 图形界面
//!
//! # 架构
//!
//! 本程序是**纯外壳**：界面用 Web 技术（HTML/CSS/JS）渲染，由 Tauri 承载。
//! 保活逻辑在独立的守护进程 `natpierce-keepalived.exe` 里，通过 `#[tauri::command]`
//! 暴露的能力去控制它。
//!
//! ```text
//! ┌─ natpierce-gui.exe（Tauri + WebView2）──┐
//! │  · 系统托盘（Tauri 内置）                │
//! │  · Web UI 设置界面                       │
//! │  · 只负责"读配置 / 写配置 / 控进程"      │
//! └──────────────────────────────────────────┘
//!               │ 启动 / 停止
//!               ▼
//! ┌─ natpierce-keepalived.exe run ──────────┐
//! │  保活循环（独立进程，关掉界面也照跑）     │
//! └──────────────────────────────────────────┘
//! ```

// 图形程序不需要控制台
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use natpierce_core::api;
use natpierce_core::config::{self, Config, Mode};
use natpierce_core::{autostart, logging, process, secret, service, APP_NAME, VERSION};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

/// 当前生效的配置文件路径
struct AppState {
    config_path: PathBuf,
}

// ============================================================
// 与前端交换的数据结构
// ============================================================

/// 运行时状态（供界面轮询展示）
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    process_alive: bool,
    daemon_running: bool,
    elevated: bool,
    api_reachable: bool,
    server_running: Option<bool>,
    account: String,
    identification: String,
    machine_name: String,
    public_ip: String,
    mappings: String,
    hosts: Vec<HostInfo>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostInfo {
    name: String,
    id: String,
    mappings: String,
}

/// 界面读写的配置视图（前端用的扁平结构）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigView {
    #[serde(default)]
    mode: String,
    #[serde(default)]
    exe_path: String,
    #[serde(default)]
    working_dir: String,
    #[serde(default)]
    process_name: String,
    #[serde(default)]
    start_args: String,
    #[serde(default)]
    api_url: String,
    #[serde(default)]
    max_clients: String,
    #[serde(default)]
    interval_sec: String,
    #[serde(default)]
    heartbeat_sec: String,
    #[serde(default)]
    fail_threshold: String,
    #[serde(default)]
    restart_after_failures: String,
    #[serde(default)]
    target_host_id: String,
    #[serde(default)]
    target_host_name: String,
    #[serde(default)]
    close_server_first: bool,
    #[serde(default)]
    keepalive_enabled: bool,
    /// 是否已经保存过页面密码（DPAPI 密文存在）
    #[serde(default)]
    has_page_password: bool,
    /// 皎月连登录账号（邮箱）
    #[serde(default)]
    account: String,
    /// 界面新输入的登录密码（留空表示不修改）
    #[serde(default)]
    login_password: String,
    /// 是否已保存登录密码
    #[serde(default)]
    has_login_password: bool,
    /// 界面新输入的页面密码（留空表示不修改）
    #[serde(default)]
    page_password: String,
    #[serde(default)]
    connection_password: String,
}

impl ConfigView {
    fn from_config(cfg: &Config, config_path: &PathBuf) -> Self {
        Self {
            mode: match cfg.mode {
                Mode::Server => "server".into(),
                Mode::Client => "client".into(),
            },
            exe_path: cfg.natpierce.exe_path.clone(),
            working_dir: cfg.natpierce.working_dir.clone(),
            process_name: cfg.natpierce.process_name.clone(),
            start_args: cfg.natpierce.start_args.join(" "),
            api_url: cfg.api.url.clone(),
            max_clients: cfg.server.max_clients.to_string(),
            interval_sec: cfg.keepalive.interval_sec.to_string(),
            heartbeat_sec: cfg.keepalive.heartbeat_sec.to_string(),
            fail_threshold: cfg.keepalive.fail_threshold.to_string(),
            restart_after_failures: cfg.keepalive.restart_after_failures.to_string(),
            target_host_id: cfg.client.target_host_id.clone(),
            target_host_name: cfg.client.target_host_name.clone(),
            close_server_first: cfg.client.close_server_first,
            keepalive_enabled: cfg.keepalive.enabled,
            has_page_password: secret::default_secrets_path(config_path).exists(),
            account: cfg.account.clone(),
            login_password: String::new(),
            has_login_password: secret::load_key(
                &secret::default_secrets_path(config_path),
                secret::KEY_LOGIN,
            )
            .map(|s| !s.is_empty())
            .unwrap_or(false),
            page_password: String::new(),
            connection_password: String::new(),
        }
    }

    fn apply_to(&self, cfg: &mut Config) {
        cfg.mode = if self.mode.eq_ignore_ascii_case("client") {
            Mode::Client
        } else {
            Mode::Server
        };
        cfg.natpierce.exe_path = self.exe_path.trim().to_string();
        cfg.natpierce.working_dir = self.working_dir.trim().to_string();
        cfg.natpierce.process_name = self.process_name.trim().to_string();
        cfg.natpierce.start_args = self
            .start_args
            .split_whitespace()
            .map(String::from)
            .collect();
        cfg.api.url = self.api_url.trim().to_string();
        cfg.server.max_clients = self.max_clients.trim().parse().unwrap_or(0);
        cfg.keepalive.interval_sec = self.interval_sec.trim().parse().unwrap_or(60);
        cfg.keepalive.heartbeat_sec = self.heartbeat_sec.trim().parse().unwrap_or(15);
        cfg.keepalive.fail_threshold = self.fail_threshold.trim().parse().unwrap_or(3);
        cfg.keepalive.restart_after_failures =
            self.restart_after_failures.trim().parse().unwrap_or(5);
        cfg.keepalive.enabled = self.keepalive_enabled;
        if !self.account.trim().is_empty() {
            cfg.account = self.account.trim().to_string();
        }
        cfg.client.target_host_id = self.target_host_id.trim().to_string();
        cfg.client.target_host_name = self.target_host_name.trim().to_string();
        cfg.client.close_server_first = self.close_server_first;
        if !self.connection_password.is_empty() {
            cfg.server.connection_password = self.connection_password.clone();
        }
    }
}

// ============================================================
// Tauri 命令
// ============================================================

/// 读取配置
#[tauri::command]
fn get_config(state: tauri::State<'_, Mutex<AppState>>) -> Result<ConfigView, String> {
    let st = state.lock().map_err(|e| e.to_string())?;
    let loaded = config::load_config_from(&st.config_path).map_err(|e| format!("{e:#}"))?;
    Ok(ConfigView::from_config(&loaded.config, &st.config_path))
}

/// 保存配置
///
/// 注意：`ConfigView` 使用 `rename_all = "camelCase"`，因此前端必须发 camelCase 字段
/// （`exePath` 而非 `exe_path`），否则会报
/// `invalid args 'view' for command 'save_config': missing field 'exe_path'`。
#[tauri::command]
fn save_config(
    view: ConfigView,
    state: tauri::State<'_, Mutex<AppState>>,
    app: AppHandle,
) -> Result<String, String> {
    // 在独立作用域内完成写入，尽早释放 state 借用
    let (path, was_running) = {
        let st = state.lock().map_err(|e| e.to_string())?;
        let mut loaded =
            config::load_config_from(&st.config_path).map_err(|e| format!("{e:#}"))?;

        // 新输入的页面密码 → DPAPI 加密落盘
        if !view.page_password.trim().is_empty() {
            let secrets = secret::default_secrets_path(&st.config_path);
            secret::save_key(&secrets, secret::KEY_PAGE, view.page_password.trim())
                .map_err(|e| format!("保存密码失败: {e:#}"))?;
            loaded.config.server.page_password = "dpapi".into();
        }

        // 登录密码 → DPAPI（与页面密码分开存）
        if !view.login_password.trim().is_empty() {
            let secrets = secret::default_secrets_path(&st.config_path);
            secret::save_key(&secrets, secret::KEY_LOGIN, view.login_password.trim())
                .map_err(|e| format!("保存登录密码失败: {e:#}"))?;
        }

        view.apply_to(&mut loaded.config);
        loaded.save().map_err(|e| format!("保存失败: {e:#}"))?;

        (
            st.config_path.clone(),
            process::is_running("natpierce-keepalived"),
        )
    };

    if !was_running {
        return Ok("已保存".into());
    }

    // 异步重启，避免阻塞界面（停止要等守护进程退出，最长数秒）
    let handle = app.clone();
    std::thread::spawn(move || {
        // ① 优雅停止
        let _ = natpierce_core::stop_flag::request(&path);
        for _ in 0..25 {
            std::thread::sleep(Duration::from_millis(200));
            if !process::is_running("natpierce-keepalived") {
                break;
            }
        }
        if process::is_running("natpierce-keepalived") {
            tracing::warn!("保存配置：守护进程未响应停止请求，尝试强制结束");
            let _ = process::kill_all("natpierce-keepalived");
            std::thread::sleep(Duration::from_millis(500));
        }

        // ② 用新配置重新启动
        match spawn_daemon(&handle) {
            Ok(_) => tracing::info!("保存配置：已用新配置重启保活"),
            Err(e) => tracing::warn!("保存配置：重启保活失败（{e}）"),
        }
    });

    Ok("已保存，正在用新配置重启保活…".into())
}

/// 查询运行时状态
#[tauri::command]
async fn get_status(state: tauri::State<'_, Mutex<AppState>>) -> Result<Status, String> {
    let path = {
        let st = state.lock().map_err(|e| e.to_string())?;
        st.config_path.clone()
    };
    collect_status(&path).await
}
/// 守护进程是否在运行
#[tauri::command]
fn daemon_status() -> bool {
    process::is_running("natpierce-keepalived")
}

/// 配置校验结果（供界面提示缺什么）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigCheck {
    /// 是否已满足启动保活的最低要求
    ok: bool,
    /// 问题列表（人话）
    problems: Vec<String>,
    /// 校验到的 exe 路径
    exe_path: String,
    /// 该路径是否真实存在
    exe_exists: bool,
}

/// 校验配置是否可用于启动保活
#[tauri::command]
fn check_config(state: tauri::State<'_, Mutex<AppState>>) -> Result<ConfigCheck, String> {
    let st = state.lock().map_err(|e| e.to_string())?;
    let loaded = config::load_config_from(&st.config_path).map_err(|e| format!("{e:#}"))?;
    let cfg = &loaded.config;

    let mut problems = Vec::new();
    let exe_raw = cfg.natpierce.exe_path.trim().to_string();
    let exe = std::path::PathBuf::from(&exe_raw);

    // ① exe 路径
    if exe_raw.is_empty() {
        problems.push("尚未设置「natpierce.exe 路径」，请点右侧「浏览…」选择皎月连主程序".into());
    } else if !exe.exists() {
        problems.push(format!(
            "配置的路径不存在：{exe_raw}\n请点「浏览…」重新选择皎月连主程序（通常名为 natpierce.exe）"
        ));
    } else if exe
        .file_name()
        .map(|n| !n.to_string_lossy().to_lowercase().ends_with(".exe"))
        .unwrap_or(true)
    {
        problems.push(format!("路径不像可执行文件：{exe_raw}"));
    }

    // ② 服务端模式必须能拿到页面访问密码
    if cfg.mode == Mode::Server {
        let pwd_ref = cfg.server.page_password.trim();
        let secrets = secret::default_secrets_path(&st.config_path);
        if pwd_ref.is_empty() {
            problems.push("尚未设置「页面访问密码」，请在界面填写后保存".into());
        } else if pwd_ref.eq_ignore_ascii_case("dpapi") && !secrets.exists() {
            problems.push("「页面访问密码」尚未录入，请在界面填写后保存".into());
        }
    }

    // ③ 客户端模式要能定位目标
    if cfg.mode == Mode::Client
        && cfg.client.target_host_id.trim().is_empty()
        && cfg.client.target_host_name.trim().is_empty()
        && cfg.client.target_index == 0
    {
        problems.push("客户端模式需要指定「目标识别码」，可在上方在线主机列表点选".into());
    }

    Ok(ConfigCheck {
        ok: problems.is_empty(),
        problems,
        exe_path: exe_raw,
        exe_exists: exe.exists(),
    })
}

/// 启动保活（拉起守护进程）
#[tauri::command]
fn start_daemon(state: tauri::State<'_, Mutex<AppState>>) -> Result<String, String> {
    let path = {
        let st = state.lock().map_err(|e| e.to_string())?;
        st.config_path.clone()
    };

    // ⚠️ 强制校验：配置不完整就直接拒绝，并说明缺什么。
    // 之前不做校验，用户没设置 natpierce.exe 路径也能"启动保活"，
    // 守护进程随后静默失败，表现为"点了一点反应都没有"。
    let check = check_config(state)?;
    if !check.ok {
        return Err(format!(
            "启动保活前请先完善配置：\n\n• {}",
            check.problems.join("\n\n• ")
        ));
    }

    if process::is_running("natpierce-keepalived") {
        return Ok("保活已在运行".into());
    }

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().unwrap_or(std::path::Path::new("."));
    let daemon = dir.join("natpierce-keepalived.exe");

    if !daemon.exists() {
        return Err(format!(
            "未找到守护程序：{}\n请把它和本程序放在同一目录。",
            daemon.display()
        ));
    }

    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut child = std::process::Command::new(&daemon)
        .arg("run")
        .arg("--config")
        .arg(&path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("启动失败: {e}"))?;

    // 关键：spawn 成功 ≠ 进程活着。必须等一下确认它没立刻退出，
    // 否则界面会显示"已启动保活"但实际上是失败的。
    std::thread::sleep(Duration::from_millis(1200));

    match child.try_wait() {
        Ok(Some(status)) => Err(format!(
            "守护进程启动后立即退出（退出码 {:?}）。\n\
             请用命令行查看原因：\n\
             natpierce-keepalived.exe run --config \"{}\"",
            status.code(),
            path.display()
        )),
        Ok(None) => {
            // 仍在运行，再确认一次按名字能找到
            if process::is_running("natpierce-keepalived") {
                let admin = "普通用户";
                Ok(format!("已启动保活（当前权限：{admin}）"))
            } else {
                Err("守护进程已启动但未能按名称检测到，请检查是否被杀软拦截".into())
            }
        }
        Err(e) => Err(format!("无法确认守护进程状态: {e}")),
    }
}

/// 停止保活
///
/// 优先用**停止标志文件**让守护进程自己优雅退出 —— 与权限无关，
/// 即使守护进程是管理员/SYSTEM 启动的也能停掉。
/// 只有超时未退出时才回退到强制终止。
#[tauri::command]
fn stop_daemon(state: tauri::State<'_, Mutex<AppState>>) -> Result<String, String> {
    let path = {
        let st = state.lock().map_err(|e| e.to_string())?;
        st.config_path.clone()
    };

    if !process::is_running("natpierce-keepalived") {
        return Ok("保活未在运行".into());
    }

    // ① 请求优雅退出
    natpierce_core::stop_flag::request(&path).map_err(|e| format!("写入停止标志失败: {e}"))?;

    // ② 等它自己退出（最多 5 秒）
    for _ in 0..25 {
        std::thread::sleep(Duration::from_millis(200));
        if !process::is_running("natpierce-keepalived") {
            return Ok("已停止保活".into());
        }
    }

    // ③ 兜底：标志未生效才强杀
    let n = process::kill_all("natpierce-keepalived").unwrap_or(0);
    if process::is_running("natpierce-keepalived") {
        Err("守护进程未响应停止请求，且权限不足无法强制结束。\n请在任务管理器中结束 natpierce-keepalived.exe".into())
    } else {
        Ok(format!("已停止保活（强制结束 {n} 个进程）"))
    }
}

/// 开机自启状态
#[tauri::command]
fn autostart_status() -> bool {
    autostart::is_enabled()
}

/// 切换开机自启
#[tauri::command]
fn autostart_set(
    enable: bool,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<String, String> {
    let path = {
        let st = state.lock().map_err(|e| e.to_string())?;
        st.config_path.clone()
    };
    if enable {
        autostart::enable(Some(&path)).map_err(|e| format!("{e:#}"))?;
        Ok("已启用开机自启".into())
    } else {
        autostart::disable().map_err(|e| format!("{e:#}"))?;
        Ok("已关闭开机自启".into())
    }
}

/// Windows 服务状态
#[tauri::command]
fn service_status() -> ServiceInfo {
    ServiceInfo {
        installed: service::is_installed(),
        running: service::is_running(),
        state: service::query_state().unwrap_or_else(|| "未安装".into()),
    }
}

#[derive(Serialize)]
struct ServiceInfo {
    installed: bool,
    running: bool,
    state: String,
}

/// 在资源管理器中打开目录
#[tauri::command]
fn open_dir(which: String, state: tauri::State<'_, Mutex<AppState>>) -> Result<(), String> {
    let st = state.lock().map_err(|e| e.to_string())?;
    let dir = match which.as_str() {
        "log" => logging::resolve_log_dir(
            &config::load_config_from(&st.config_path)
                .map(|l| l.config)
                .unwrap_or_default(),
            &st.config_path,
        ),
        _ => st
            .config_path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
    };
    let _ = std::fs::create_dir_all(&dir);
    std::process::Command::new("explorer.exe")
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("打开目录失败: {e}"))?;
    Ok(())
}

/// 读取保活日志尾部，供界面直接显示
///
/// 取日志目录里最新的一个 `keepalive.log.*`，返回最后 `lines` 行。
/// 守护进程是独立进程写文件，界面按需读取即可，无需额外 IPC。
#[tauri::command]
fn read_logs(
    lines: Option<usize>,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<serde_json::Value, String> {
    let st = state.lock().map_err(|e| e.to_string())?;
    let cfg = config::load_config_from(&st.config_path)
        .map(|l| l.config)
        .unwrap_or_default();
    let dir = logging::resolve_log_dir(&cfg, &st.config_path);
    let want = lines.unwrap_or(200).clamp(1, 2000);

    let latest = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            // 排除 .prev 备份：否则新日志还没落盘时会显示上一次运行的记录
            n.starts_with("keepalive.log") && !n.ends_with(".prev")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(t, _)| *t);

    let Some((_, path)) = latest else {
        return Ok(serde_json::json!({
            "exists": false,
            "path": dir.display().to_string(),
            "total": 0,
            "lines": [],
        }));
    };

    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(want);

    Ok(serde_json::json!({
        "exists": true,
        "path": path.display().to_string(),
        "total": all.len(),
        "lines": all[start..].to_vec(),
    }))
}

/// 版本信息
#[tauri::command]
fn app_info() -> serde_json::Value {
    serde_json::json!({
        "name": APP_NAME,
        "version": VERSION,
        "configPath": config::resolve_config_path().display().to_string(),
    })
}

// ============================================================
// 入口
// ============================================================

fn main() {
    // 首次运行：自动创建配置，避免"打开就报错"
    let config_path = config::resolve_config_path();
    if !config_path.exists() {
        let target = if config::save_config(&Config::default(), &config_path).is_ok() {
            config_path.clone()
        } else if let Some(fb) = config::fallback_config_path() {
            let _ = config::save_config(&Config::default(), &fb);
            fb
        } else {
            config_path.clone()
        };
        let _ = target;
    }

    // 日志写文件（GUI 程序没有控制台）
    let loaded = config::load_config_from(&config_path).ok();
    let _guard = loaded.as_ref().and_then(|l| {
        logging::init(&l.config, &l.path, None, true)
    });

    let state = Mutex::new(AppState {
        config_path: config_path.clone(),
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // 第二次启动时把已有窗口拉到前面
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            get_status,
            daemon_status,
            start_daemon,
            stop_daemon,
            autostart_status,
            autostart_set,
            service_status,
            check_config,
            open_dir,
            read_logs,
            app_info,
        ])
        .setup(|app| {
            build_tray(app.handle())?;

            // ① 后端主动推送状态（取代前端轮询）
            spawn_status_pusher(app.handle().clone());

            // ② 默认自动开启保活：配置齐全就自动拉起守护进程
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(600));
                autostart_keepalive_if_ready(&handle);
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            // 关闭窗口 = 隐藏到托盘，不退出程序。
            // 真正退出请用托盘菜单「退出」，那会连同守护进程一起结束。
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}

/// 退出前清理：连同守护进程一起优雅结束
///
/// 先写停止标志让守护进程自己退出（与权限无关），超时未退才回退到强制终止。
fn shutdown_all(app: &AppHandle) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    if !process::is_running("natpierce-keepalived") {
        app.exit(0);
        return;
    }

    // ① 优雅停止：写标志文件
    if let Ok(st) = app.state::<Mutex<AppState>>().lock() {
        let _ = natpierce_core::stop_flag::request(&st.config_path);
    }
    for _ in 0..15 {
        std::thread::sleep(Duration::from_millis(200));
        if !process::is_running("natpierce-keepalived") {
            tracing::info!("退出：守护进程已优雅停止");
            app.exit(0);
            return;
        }
    }

    // ② 兜底强杀
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/IM", "natpierce-keepalived.exe", "/T"])
        .creation_flags(CREATE_NO_WINDOW)
        .status();
    if process::is_running("natpierce-keepalived") {
        tracing::warn!("守护进程权限较高，未能终止，请在任务管理器中手动结束");
    }
    app.exit(0);
}
/// 采集一次完整状态（事件推送与 `get_status` 共用）
async fn collect_status(path: &std::path::Path) -> Result<Status, String> {
    let cfg = config::load_config_from(path)
        .map(|l| l.config)
        .unwrap_or_default();

    let mut s = Status {
        process_alive: process::is_running(&cfg.natpierce.process_name),
        daemon_running: process::is_running("natpierce-keepalived"),
        elevated: process::is_elevated(),
        ..Default::default()
    };

    match api::probe(
        &cfg.api.url,
        &cfg.api.fallback_url,
        Duration::from_millis(cfg.api.connect_timeout_ms),
        Duration::from_millis(cfg.api.command_timeout_ms),
    )
    .await
    {
        Ok(r) => {
            s.api_reachable = true;
            s.server_running = r.server_running;
            if let Some(info) = &r.server_info {
                s.account = info.account.clone();
                s.identification = info.identification.clone();
                s.machine_name = info.machine_name.clone();
                s.public_ip = info.public_ip.clone();
                s.mappings = info.mappings.clone();
            }
            s.hosts = r.hosts.iter().map(|h| HostInfo {
                name: h.name.clone(),
                id: h.id.clone(),
                mappings: h.mappings.clone(),
            }).collect();
        }
        Err(e) => {
            s.api_reachable = false;
            s.error = Some(format!("{e}"));
        }
    }
    Ok(s)
}

/// 启动后台状态推送线程
///
/// 取代前端"每 8 秒轮询一次"的模式：后端主动把状态推给界面，
/// 操作完成时立即可见，不需要等下一个轮询周期。
///
/// 发两个事件：
/// - `status-changed`：状态对象
/// - `probe-state`：探测阶段（probing / ok / error），用于显示"检测中…"
fn spawn_status_pusher(app: AppHandle) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => { tracing::error!("状态推送线程启动失败: {e}"); return; }
        };
        rt.block_on(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(3));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let path = match app.state::<Mutex<AppState>>().lock() {
                    Ok(st) => st.config_path.clone(),
                    Err(_) => continue,
                };
                let _ = app.emit("probe-state", "probing");
                match collect_status(&path).await {
                    Ok(s) => {
                        let _ = app.emit("status-changed", &s);
                        let _ = app.emit("probe-state", "ok");
                    }
                    Err(e) => {
                        let _ = app.emit("probe-state", "error");
                        tracing::debug!("状态推送失败: {e}");
                    }
                }
            }
        });
    });
}

/// 配置齐全时自动启动保活（GUI 启动即生效，无需用户点按钮）
///
/// 静默处理失败 —— 配置不完整时界面上的红色提示已经说明原因，
/// 这里不再弹窗打扰。
fn autostart_keepalive_if_ready(app: &AppHandle) {
    // 已在运行就不重复启动
    if process::is_running("natpierce-keepalived") {
        tracing::info!("保活已在运行，跳过自动启动");
        return;
    }

    let path = match app.state::<Mutex<AppState>>().lock() {
        Ok(st) => st.config_path.clone(),
        Err(_) => return,
    };

    // 复用配置校验：不完整就不启动
    let loaded = match config::load_config_from(&path) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("自动启动保活失败（配置无法读取）: {e:#}");
            return;
        }
    };
    let cfg = &loaded.config;

    let exe = std::path::PathBuf::from(cfg.natpierce.exe_path.trim());
    if cfg.natpierce.exe_path.trim().is_empty() || !exe.exists() {
        tracing::warn!(
            "自动启动保活失败：natpierce.exe 路径无效 ({})，请在界面里设置",
            cfg.natpierce.exe_path
        );
        return;
    }
    if cfg.mode == Mode::Server {
        let pwd = cfg.server.page_password.trim();
        let secrets = secret::default_secrets_path(&path);
        if pwd.is_empty() || (pwd.eq_ignore_ascii_case("dpapi") && !secrets.exists()) {
            tracing::warn!("自动启动保活失败：页面访问密码尚未录入");
            return;
        }
    }

    match spawn_daemon(app) {
        Ok(_) => tracing::info!("配置齐全，已自动开启保活"),
        Err(e) => tracing::warn!("自动开启保活失败: {e}"),
    }
}

/// 构建系统托盘
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "打开设置…", true, None::<&str>)?;
    let autostart_item =
        CheckMenuItem::with_id(app, "autostart", "开机自启", true, autostart::is_enabled(), None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "立即刷新", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[&show, &sep1, &autostart_item, &sep2, &refresh, &quit],
    )?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .icon(app.default_window_icon().cloned().unwrap())
        .tooltip(format!("{APP_NAME} v{VERSION}"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "autostart" => {
                let cfg = config::resolve_config_path();
                let _ = autostart::toggle(Some(&cfg));
            }
            "refresh" => {
                let _ = app.emit("refresh", ());
            }
            "quit" => shutdown_all(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        })
        .build(app)?;

    Ok(())
}

/// 供托盘菜单使用的启动逻辑
fn spawn_daemon(app: &AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().unwrap_or(std::path::Path::new("."));
    let daemon = dir.join("natpierce-keepalived.exe");
    let cfg = config::resolve_config_path();
    let _ = app;

    if !daemon.exists() {
        return Err("未找到 natpierce-keepalived.exe".into());
    }

    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(&daemon)
        .arg("run")
        .arg("--config")
        .arg(&cfg)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
