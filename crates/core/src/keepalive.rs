//! 保活状态机
//!
//! # 三层判据
//!
//! ```text
//! Layer 1  进程层   natpierce.exe 是否存活
//! Layer 2  服务层   本地 API 能否连上 / 服务端是否 running / 客户端是否连上目标主机
//! Layer 3  心跳层   周期性通信，防止长时间空闲掉线
//! ```
//!
//! # 决策规则
//!
//! | 现象 | 判定 | 动作 |
//! |---|---|---|
//! | API 连不上 + 进程不在 | 软件没跑 | **启动进程** |
//! | API 连不上 + 进程在 | 软件卡死 | 累计失败，超阈值则**重启进程** |
//! | 服务端未启动 | 服务已停 | 发 `startServer` |
//! | 客户端未连上目标 | 连接已断 | 发 `conpc` 重连 |
//! | 一切正常 | 健康 | 清零失败计数，发心跳 |

use anyhow::Result;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::api::{self, protocol};
use crate::config::{Config, LoadedConfig, Mode};
use crate::process;
use crate::secret;

/// 一次巡检得到的健康状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// 皎月连处于登录界面（未登录）
    NeedLogin,
    /// 一切正常
    Healthy,
    /// 进程不在
    ProcessMissing,
    /// API 连不上（进程可能卡死）
    ApiUnreachable,
    /// 服务端未启动
    ServerStopped,
    /// 客户端未连上目标主机
    ClientDisconnected,
    /// 出错
    Error(String),
}

impl Health {
    pub fn needs_login(&self) -> bool {
        matches!(self, Health::NeedLogin)
    }

    pub fn is_healthy(&self) -> bool {
        matches!(self, Health::Healthy)
    }

    /// 是否需要"重启进程"级别的干预
    pub fn needs_process_restart(&self) -> bool {
        matches!(self, Health::ApiUnreachable)
    }

    /// 是否可以通过 API 修复
    pub fn is_api_fixable(&self) -> bool {
        matches!(self, Health::ServerStopped | Health::ClientDisconnected)
    }

    pub fn label(&self) -> String {
        match self {
            Health::Healthy => "正常".into(),
            Health::ProcessMissing => "进程未运行".into(),
            Health::ApiUnreachable => "本地接口连不上".into(),
            Health::ServerStopped => "服务端未启动".into(),
            Health::ClientDisconnected => "未连接到目标主机".into(),
            Health::NeedLogin => "未登录皎月连".into(),
            Health::Error(e) => format!("错误: {e}"),
        }
    }
}

/// 保活循环的运行时状态
pub struct Keepalive {
    pub loaded: LoadedConfig,
    /// 连续失败计数
    failures: u32,
    /// 上次重启时间（用于冷却）
    last_restart: Option<Instant>,
    /// 累计重启次数
    pub restarts: u32,
    /// 累计修复次数（API 级别的恢复）
    pub repairs: u32,
    /// 上次心跳时间
    last_heartbeat: Option<Instant>,
    /// 最近一次失败的描述（供界面展示，如"账号或密码错误"）
    last_error: Option<String>,
    /// 最近一次失败是否属于"配置/凭据类"（密码缺失、账号错误等）
    /// 这类问题重启进程毫无意义，必须由用户改配置，因此不应触发重启
    last_error_is_config: bool,
    /// 上次看到的配置文件指纹（用于热重载）
    config_fp: Option<crate::config::Fingerprint>,
    /// 客户端模式：本机是否已连上目标主机。
    ///
    /// 为什么需要它：natpierce 只在连接/断开**事件发生时**推送消息，
    /// 新开一条 WebSocket 不保证重放当前状态，探测经常拿不到连接状态。
    /// 这时靠这里记住"上次确实连上了"，避免每轮都重发 conpc 造成连接抖动。
    /// 守护进程重启后为 false，会重新确认一次连接。
    client_link_ok: bool,
}

impl Keepalive {
    pub fn new(loaded: LoadedConfig) -> Self {
        let fp = crate::config::Fingerprint::of(&loaded.path);
        Self {
            loaded,
            failures: 0,
            last_restart: None,
            restarts: 0,
            repairs: 0,
            last_heartbeat: None,
            last_error: None,
            last_error_is_config: false,
            config_fp: fp,
            client_link_ok: false,
        }
    }

    /// 检测配置文件是否被界面改动，若变了就重新加载
    ///
    /// 这是必需的：守护进程常驻运行，用户在界面里改完配置点保存时守护进程
    /// 并不会重启。没有热重载的话它会一直用启动时读到的旧配置
    /// —— 典型症状就是日志里反复出现旧的 exe 路径。
    /// 最近一次失败原因，成功后清空
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// 把最近状态写到配置文件同目录的 .status（供界面展示）
    ///
    /// 守护进程与界面是两个进程，无法直接共享内存，用状态文件做最轻量的通道。
    fn write_status_file(&self) {
        let path = crate::config::status_file_path(&self.loaded.path);
        let body = serde_json::json!({
            "lastError": self.last_error,
            "failures": self.failures,
            "restarts": self.restarts,
            "repairs": self.repairs,
            "mode": format!("{}", self.cfg().mode),
            "account": self.cfg().account,
            // 客户端模式的**真实**连接状态，供界面显示「目标连接」。
            // 不能改用 probe() 的瞬时结果：natpierce 只在连接事件发生时
            // 推送 conpc，短连接探测拿不到，永远是 unknown。
            "clientLinked": self.client_link_ok,
            "updatedAt": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        });
        let _ = std::fs::write(path, body.to_string());
    }

    fn reload_if_changed(&mut self) {
        let path = self.loaded.path.clone();
        let now = match crate::config::Fingerprint::of(&path) {
            Some(fp) => fp,
            None => return,
        };

        if Some(now) == self.config_fp {
            return;
        }
        if self.config_fp.is_none() {
            self.config_fp = Some(now);
            return;
        }

        match crate::config::load_config_from(&path) {
            Ok(fresh) => {
                info!(
                    "检测到配置变更，已热重载（模式={}，exe={}）",
                    fresh.config.mode, fresh.config.natpierce.exe_path
                );
                self.loaded = fresh;
                self.config_fp = Some(now);
                self.failures = 0; // 立即用新配置重试
            }
            Err(e) => {
                warn!("配置已变更但解析失败，继续沿用旧配置: {e:#}");
                self.config_fp = Some(now); // 避免每轮重复解析坏文件
            }
        }
    }

    pub fn cfg(&self) -> &Config {
        &self.loaded.config
    }

    /// 执行一次巡检 + 修复，并**连续推进**直到状态稳定
    ///
    /// 修好一步（启动进程 / 登录 / 开服务端）后立刻重新探测下一步，
    /// 而不是每修一步都等满一个巡检间隔 —— 否则
    /// 「启动进程 → 自动登录 → 开服务端」这条链要走 3 个 60 秒，
    /// 体感就是"皎月连明明起来了，却半天不去开服务端"。
    pub async fn tick(&mut self) -> Result<Health> {
        /// 单次巡检最多推进的步数，防止状态反复时死循环
        const MAX_STEPS: usize = 5;

        let mut last = Health::Healthy;
        for step in 1..=MAX_STEPS {
            last = self.tick_once().await?;

            // 只有"这一步确实做了修复、而且没失败"才继续往下推。
            // 失败会让 failures 递增，此时应当退回常规巡检节奏，避免疯狂重试。
            let progresses = self.failures == 0
                && matches!(
                    last,
                    Health::ProcessMissing
                        | Health::NeedLogin
                        | Health::ServerStopped
                        | Health::ClientDisconnected
                );
            if !progresses || step == MAX_STEPS {
                break;
            }

            info!("→ 继续处理下一步（本次巡检已推进 {step} 步）");
            // 留一点时间让皎月连把状态落实（例如刚登录完，服务端状态才刷新）
            tokio::time::sleep(Duration::from_millis(600)).await;
        }
        Ok(last)
    }

    /// 执行**单步**巡检 + 修复（由 [`tick`](Self::tick) 连续调用）
    async fn tick_once(&mut self) -> Result<Health> {
        // 每轮先检查配置有没有被界面改动过
        self.reload_if_changed();

        let health = self.check().await;

        match health {
            Health::Healthy => {
                if self.failures > 0 {
                    info!("已恢复正常（此前连续失败 {} 次）", self.failures);
                }
                self.failures = 0;
                self.last_error = None;
                self.maybe_heartbeat().await;
            }

            Health::NeedLogin => {
                info!("检测到皎月连未登录，尝试自动登录");
                match self.auto_login().await {
                    Ok(true) => {
                        info!("自动登录成功，稍后会自动开启服务端");
                        self.failures = 0;
                        self.last_error = None;
                        self.last_error_is_config = false;
                    }
                    Ok(false) => {
                        self.failures += 1;
                        let msg = "自动登录未成功，请检查界面「皎月连账号」里的账号与密码".to_string();
                        warn!("{msg}");
                        self.last_error = Some(msg);
                        self.last_error_is_config = true;
                    }
                    Err(e) => {
                        self.failures += 1;
                        // bail! 里带的是服务端返回的具体原因（如"账号或密码错误"）
                        let msg = format!("{e}");
                        warn!("自动登录失败: {msg}");
                        self.last_error = Some(msg);
                        self.last_error_is_config = true;
                    }
                }
            }

            Health::ProcessMissing => {
                warn!("检测到 {} 未运行，尝试启动", self.cfg().natpierce.process_name);
                self.failures = 0;
                self.start_process().await?;
            }

            Health::ServerStopped => {
                info!("服务端未启动，尝试开启");
                match self.start_server().await {
                    Ok(true) => {
                        self.repairs += 1;
                        self.failures = 0;
                        info!("服务端已恢复");
                    }
                    Ok(false) => {
                        self.failures += 1;
                        warn!("开启服务端未成功（第 {} 次）", self.failures);
                    }
                    Err(e) => {
                        self.failures += 1;
                        warn!("开启服务端出错: {e:#}");
                    }
                }
                self.maybe_restart_process().await?;
            }

            Health::ClientDisconnected => {
                info!("未连接到目标主机，尝试连接");
                match self.connect_target().await {
                    Ok(true) => {
                        self.repairs += 1;
                        self.failures = 0;
                        self.client_link_ok = true;
                        info!("已连接到目标主机");
                    }
                    Ok(false) => {
                        self.failures += 1;
                        self.client_link_ok = false;
                        warn!("连接目标主机未成功（第 {} 次）", self.failures);
                    }
                    Err(e) => {
                        self.failures += 1;
                        self.client_link_ok = false;
                        warn!("连接目标主机出错: {e:#}");
                    }
                }
                self.maybe_restart_process().await?;
            }

            Health::ApiUnreachable => {
                self.failures += 1;
                warn!(
                    "本地接口无法连接（进程存在但疑似卡死），连续失败 {}/{}",
                    self.failures, self.cfg().keepalive.restart_after_failures
                );
                self.maybe_restart_process().await?;
            }

            Health::Error(ref e) => {
                self.failures += 1;
                warn!("巡检出错: {e}");
            }
        }

        self.write_status_file();
        Ok(health)
    }

    /// 只检查，不修复
    pub async fn check(&self) -> Health {
        let cfg = self.cfg();

        // Layer 1: 进程
        if !process::is_running(&cfg.natpierce.process_name) {
            return Health::ProcessMissing;
        }

        // Layer 2: API + 业务状态
        match api::probe(
            &cfg.api.url,
            &cfg.api.fallback_url,
            Duration::from_millis(cfg.api.connect_timeout_ms),
            Duration::from_millis(cfg.api.command_timeout_ms),
        )
        .await
        {
            Ok(r) => {
                // 未登录优先处理：此时既没有服务端也没有客户端连接
                if r.need_login {
                    return Health::NeedLogin;
                }
                match cfg.mode {
                    Mode::Server => match r.server_running {
                        Some(true) => Health::Healthy,
                        Some(false) => Health::ServerStopped,
                        None => Health::Error("无法判定服务端状态".into()),
                    },
                    Mode::Client => {
                        if self.client_connected(&r) {
                            Health::Healthy
                        } else {
                            Health::ClientDisconnected
                        }
                    }
                }
            }
            Err(e) => {
                debug!("API 探测失败: {e:#}");
                Health::ApiUnreachable
            }
        }
    }

    /// 客户端模式下判断本机是否**已连接**到目标主机
    ///
    /// 此处曾经错把「目标出现在在线主机列表里」当作「已连接」：
    /// 只要对方在线就判定健康，`connect_target()` 永远不会被触发，
    /// 表现为「填了识别码，日志里却根本不发起连接」。
    /// **在线 ≠ 已连接** —— 前者只说明"能看见对方"。
    ///
    /// 匹配用的是**组网虚拟 IP**（会话编号每次接入都会变，不能当标识）。
    fn client_connected(&self, r: &api::ProbeResult) -> bool {
        let cfg = self.cfg();
        let target_addr = cfg.client.target_addr.trim();
        let target_name = cfg.client.target_host_name.trim();

        if target_addr.is_empty() && target_name.is_empty() && cfg.client.target_index == 0 {
            // 未配置目标：只要 API 通就算健康（纯保活，不锁定主机）
            return true;
        }

        // ① 探测期间收到了明确的连接类消息 → 以它为准
        match r.client_link {
            api::ClientLink::Connected => return true,
            api::ClientLink::Failed | api::ClientLink::Disconnected => return false,
            api::ClientLink::Unknown => {}
        }

        // ② 拿不到连接状态时：目标必须在线（不在线必然连不上），
        //    再叠加"上一次我们确实连上过"。两个都满足才算健康，
        //    否则交给 connect_target() 去连。
        let target_online = if !target_addr.is_empty() {
            r.hosts
                .iter()
                .any(|h| h.addr == target_addr || h.addr.split(':').next() == Some(target_addr))
        } else {
            r.hosts
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case(target_name))
        };
        target_online && self.client_link_ok
    }

    /// 进程层：启动 natpierce（提权）
    async fn start_process(&mut self) -> Result<()> {
        let cfg = self.cfg();
        let exe = std::path::PathBuf::from(&cfg.natpierce.exe_path);
        let workdir = cfg.natpierce.resolve_working_dir();

        if !process::is_elevated() {
            // 非提权不是错误 —— 下面用 ShellExecuteW("runas") 触发 UAC 提权即可。
            // 这里原来打的是 WARN「没有管理员权限，无法启动」，但实际能启动成功，
            // 既吓人又和后面的「已启动」自相矛盾，改成如实说明。
            info!(
                "当前非管理员权限，将通过 UAC 提权启动 {}（若弹出确认框请选「是」）",
                exe.display()
            );
        }

        process::start_elevated(&exe, &cfg.natpierce.start_args, &workdir)?;

        // 等进程起来
        let wait = Duration::from_secs(20);
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            if process::is_running(&cfg.natpierce.process_name) {
                info!("{} 已启动", cfg.natpierce.process_name);
                // 再等一会儿让本地 API 就绪
                tokio::time::sleep(Duration::from_secs(5)).await;
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
        anyhow::bail!("等待进程启动超时")
    }

    /// 服务层：发 startServer
    async fn start_server(&self) -> Result<bool> {
        let cfg = self.cfg();
        let page_pwd = secret::resolve_key(
            &cfg.server.page_password,
            &secret::default_secrets_path(&self.loaded.path),
            secret::KEY_PAGE,
        )?;
        // 组网模式（虚拟网卡监听所有端口）下页面密码是开服务的硬性前置条件
        if cfg.server.vpn_mode && page_pwd.is_empty() {
            anyhow::bail!(
                "组网模式需要页面访问密码，请在界面「服务端设置」里填写 6-20 位密码并保存"
            );
        }

        let mut client = api::ApiClient::connect(
            &cfg.api.url,
            &cfg.api.fallback_url,
            Duration::from_millis(cfg.api.connect_timeout_ms),
            Duration::from_millis(cfg.api.command_timeout_ms),
        )
        .await?;

        // ① 先打开「组网模式」。
        // 官方界面是这样两步走的：勾选组网（发 VPN<$!$>1），再开服务器。
        // 漏掉这一步时 startServer 里的页面密码会被忽略 ——
        // 皎月连配置里 VPN 仍是 false、WebPwd 仍是空，服务端起不来。
        if cfg.server.vpn_mode {
            client.send(&protocol::cmd_vpn(true)).await?;
            // 留一点时间让它把配置落盘
            let _ = client.drain(Duration::from_millis(600)).await;
        }

        // ② 再开服务器。组网模式下带页面密码；端口映射模式官方传空串。
        let page_arg: &str = if cfg.server.vpn_mode { &page_pwd } else { "" };

        let cmd = protocol::cmd_start_server(
            &cfg.server.connection_password,
            cfg.server.max_clients,
            page_arg,
            &cfg.server.lan_ip,
        );
        client.send(&cmd).await?;

        let deadline = Instant::now() + Duration::from_secs(cfg.server.start_timeout_sec);
        let mut ok = false;
        // 记下 natpierce 的回应：失败时打出来，否则只有一个"未成功"没法查
        let mut replies: Vec<String> = Vec::new();
        while Instant::now() < deadline && !ok {
            match client.next_message(Duration::from_millis(1500)).await {
                Some(m) => {
                    if m.is_server_running() == Some(true) {
                        ok = true;
                    } else if replies.len() < 6 {
                        replies.push(format!("{m:?}"));
                    }
                }
                None => continue,
            }
        }
        // 服务端起来之后，natpierce 一定已就绪 —— 这时再顺手开启它自身的
        // 「自动开启」，它重开后就能自行恢复服务端。
        // 放在守护进程启动瞬间发太早：那会儿它可能还没登录，命令会被丢掉
        // （表现为皎月连配置里 Auto_start 一直是 0）。
        if ok && cfg.server.auto_start_server {
            if client.send(&protocol::cmd_autostart(true)).await.is_ok() {
                info!("已为皎月连启用「自动开启」（它重开后能自行恢复服务端）");
            }
        }

        client.close().await;

        if !ok && !replies.is_empty() {
            warn!("开启服务端未成功，natpierce 回应: {}", replies.join(" | "));
        }
        Ok(ok)
    }

    /// 登录层：自动登录皎月连
    ///
    /// 场景：皎月连的登录存档被清除（或首次使用）时，进程虽在运行，
    /// 但停在登录界面 —— 此时既没有服务端也没有客户端连接，
    /// 必须先用账号密码登录，后续的 startServer / conpc 才有意义。
    ///
    /// 账号与密码都来自配置（密码存在 DPAPI 密文里）。
    async fn auto_login(&self) -> Result<bool> {
        let cfg = self.cfg();

        let account = cfg.account.trim();
        if account.is_empty() {
            anyhow::bail!("未配置登录账号，请在界面「账号与密码」里填写");
        }

        let login_pwd = crate::secret::load_key(
            &crate::secret::default_secrets_path(&self.loaded.path),
            crate::secret::KEY_LOGIN,
        )
        .unwrap_or_default();

        if login_pwd.is_empty() {
            anyhow::bail!("未配置登录密码，请在界面「账号与密码」里填写并保存");
        }

        let mut client = api::ApiClient::connect(
            &cfg.api.url,
            &cfg.api.fallback_url,
            Duration::from_millis(cfg.api.connect_timeout_ms),
            Duration::from_millis(cfg.api.command_timeout_ms),
        )
        .await?;

        // 先丢弃握手推送，避免干扰后续判定
        let _ = client.drain(Duration::from_millis(800)).await;

        let cmd = protocol::cmd_login(account, &login_pwd, true, true);
        client.send(&cmd).await?;

        // 等待登录结果。
        // 注意：登录成功 ≠ 收到 ServerRunning —— 登录后服务端通常仍是"未启动"，
        // 服务端会推 1（已登录未启动）或 2（已登录且已启动）。
        // 收到 0 才是"仍未登录"，收到 Info 则是失败原因（如账号或密码错误）。
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut logged_in = false;
        let mut fail_reason: Option<String> = None;

        while Instant::now() < deadline && !logged_in && fail_reason.is_none() {
            match client.next_message(Duration::from_millis(1500)).await {
                // 已登录：服务端已启动
                Some(protocol::Message::ServerRunning { .. }) => logged_in = true,
                // 已登录：服务端未启动（这才是常见情况，后续 tick 会去 startServer）
                Some(protocol::Message::ServerStopped { .. }) => logged_in = true,
                // 仍未登录：说明这次尝试没成功
                Some(protocol::Message::NeedLogin) => {
                    fail_reason = Some("登录未生效（账号或密码可能不正确）".into());
                }
                // 服务端把失败原因放在 info 里
                Some(protocol::Message::Info(text)) => {
                    if !text.trim().is_empty() {
                        fail_reason = Some(text);
                    }
                }
                Some(m) => {
                    // 该账号已在别处登录时服务端回 y?，确认顶掉旧会话
                    let raw = format!("{m:?}");
                    if raw.contains("y?") {
                        let force = protocol::cmd_force_login(account, &login_pwd, true, true);
                        client.send(&force).await?;
                    }
                }
                None => continue,
            }
        }

        // 双向确认：再用一次探测看 need_login 是否已消除
        if logged_in {
            match api::probe(
                &cfg.api.url,
                &cfg.api.fallback_url,
                Duration::from_millis(cfg.api.connect_timeout_ms),
                Duration::from_millis(cfg.api.command_timeout_ms),
            )
            .await
            {
                Ok(r) if r.need_login => {
                    logged_in = false;
                    fail_reason = Some("登录后仍处于登录界面，请检查账号与密码".into());
                }
                _ => {}
            }
        }

        client.close().await;

        if let Some(reason) = fail_reason {
            anyhow::bail!("{reason}");
        }
        Ok(logged_in)
    }

    /// 客户端层：连目标主机
    async fn connect_target(&self) -> Result<bool> {
        let cfg = self.cfg();

        let mut client = api::ApiClient::connect(
            &cfg.api.url,
            &cfg.api.fallback_url,
            Duration::from_millis(cfg.api.connect_timeout_ms),
            Duration::from_millis(cfg.api.command_timeout_ms),
        )
        .await?;

        // 客户端模式要求本机服务端关闭（两者互斥）
        if cfg.client.close_server_first {
            let initial = client.drain(Duration::from_millis(1200)).await;
            if initial.iter().any(|m| m.is_server_running() == Some(true)) {
                info!("本机服务端正在运行，客户端模式需要先停掉它");
                client.send(&protocol::cmd_stop_server()).await?;
                tokio::time::sleep(Duration::from_secs(3)).await;
                client.drain(Duration::from_millis(1500)).await;
            }
        }

        // 拿在线主机列表
        client.send(&protocol::cmd_pc_list()).await?;
        let msgs = client.drain(Duration::from_millis(3000)).await;
        let hosts: Vec<_> = msgs
            .iter()
            .find_map(|m| match m {
                protocol::Message::PcList(h) if !h.is_empty() => Some(h.clone()),
                _ => None,
            })
            .unwrap_or_default();

        let target_id = self.pick_target(&hosts)?;
        if target_id.is_empty() {
            client.close().await;
            anyhow::bail!("没有匹配到目标主机（在线 {} 台）", hosts.len());
        }
        info!("目标主机: {target_id}");

        // 先清理旧连接，再连
        client.send(&protocol::cmd_stop_connect()).await?;
        tokio::time::sleep(Duration::from_millis(600)).await;
        client.drain(Duration::from_millis(600)).await;

        client.send(&protocol::cmd_connect_pc(&target_id)).await?;

        // conpc 成功可能等较久
        let timeout = Duration::from_secs(cfg.server.start_timeout_sec.min(120));
        let deadline = Instant::now() + timeout;
        let mut ok = false;
        while Instant::now() < deadline && !ok {
            match client.next_message(Duration::from_millis(1500)).await {
                Some(protocol::Message::ConOk(v)) => {
                    info!("连接确认: {v}");
                    ok = true;
                }
                Some(protocol::Message::ConErr(e)) => {
                    warn!("连接被拒绝: {e}");
                    break;
                }
                Some(protocol::Message::Disconnected(d)) => {
                    debug!("连接过程中收到 discon: {d}");
                }
                Some(_) => {}
                None => continue,
            }
        }
        client.close().await;
        Ok(ok)
    }

    /// 从在线主机里挑出目标，返回它**当前**的会话编号
    ///
    /// `pclist` 里的 `id` 是会话编号，每接入一次就变（日志里能见到
    /// `[1]→[2]→[3]→[4]`）。所以这里用**组网虚拟 IP**这类稳定标识去匹配，
    /// 再取出此刻的编号交给 `conpc` —— 对方重连、编号变了都不影响。
    fn pick_target(&self, hosts: &[protocol::HostEntry]) -> Result<String> {
        let cfg = self.cfg();

        // 1. 按组网虚拟 IP 匹配（首选：按设备分配，最稳定）
        let addr = cfg.client.target_addr.trim();
        if !addr.is_empty() {
            if let Some(h) = hosts
                .iter()
                .find(|h| h.addr == addr || h.addr.split(':').next() == Some(addr))
            {
                return Ok(h.id.clone());
            }
            let online: Vec<String> = hosts
                .iter()
                .map(|h| format!("{} ({})", h.addr, h.name))
                .collect();
            anyhow::bail!(
                "目标虚拟 IP {addr} 不在线（当前在线：{}）",
                if online.is_empty() {
                    "无".to_string()
                } else {
                    online.join("、")
                }
            );
        }

        // 2. 按名称匹配（**主要的持久化识别方式**）
        //    会话编号每次接入都会重新分配，虚拟 IP 协议里不给，
        //    所以主机名是唯一重启不变的标识。
        //    先精确匹配（忽略大小写），再退化为包含匹配，
        //    容忍"家宝" ↔ "家宝-PC" 这种差异。
        let name = cfg.client.target_host_name.trim();
        if !name.is_empty() {
            if let Some(h) = hosts
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case(name))
            {
                return Ok(h.id.clone());
            }
            let needle = name.to_lowercase();
            let cands: Vec<&protocol::HostEntry> = hosts
                .iter()
                .filter(|h| h.name.to_lowercase().contains(&needle))
                .collect();
            match cands.len() {
                1 => return Ok(cands[0].id.clone()),
                0 => anyhow::bail!("按名称「{name}」未匹配到在线主机"),
                n => {
                    let list: Vec<String> =
                        cands.iter().map(|h| h.name.clone()).collect();
                    anyhow::bail!(
                        "名称「{name}」匹配到 {n} 台主机（{}），请填更完整的名字",
                        list.join("、")
                    );
                }
            }
        }

        // 3. 按序号
        let idx = cfg.client.target_index;
        if idx >= 1 {
            return hosts
                .get(idx - 1)
                .map(|h| h.id.clone())
                .ok_or_else(|| anyhow::anyhow!("序号 {idx} 超出范围（在线 {} 台）", hosts.len()));
        }

        // 4. 没配置：只有一台就直接用，多台则要求配置
        match hosts.len() {
            0 => Ok(String::new()),
            1 => {
                info!("未配置目标，自动选择唯一在线主机: {}", hosts[0].name);
                Ok(hosts[0].id.clone())
            }
            n => anyhow::bail!(
                "在线主机有 {n} 台，请在配置里指定 client.target_host_id（可用 hosts 子命令查看）"
            ),
        }
    }

    /// Layer 3: 心跳，防空闲掉线
    async fn maybe_heartbeat(&mut self) {
        let cfg = self.cfg();
        if !cfg.keepalive.enabled || cfg.keepalive.heartbeat_sec == 0 {
            return;
        }
        let due = self
            .last_heartbeat
            .map(|t| t.elapsed() >= Duration::from_secs(cfg.keepalive.heartbeat_sec))
            .unwrap_or(true);
        if !due {
            return;
        }

        match api::ApiClient::connect(
            &cfg.api.url,
            &cfg.api.fallback_url,
            Duration::from_millis(cfg.api.connect_timeout_ms),
            Duration::from_millis(cfg.api.command_timeout_ms),
        )
        .await
        {
            Ok(mut c) => {
                if let Err(e) = c.heartbeat().await {
                    debug!("心跳发送失败: {e:#}");
                } else {
                    debug!("心跳已发送");
                }
                c.close().await;
                self.last_heartbeat = Some(Instant::now());
            }
            Err(e) => debug!("心跳连接失败: {e:#}"),
        }
    }

    /// 必要时重启进程（带冷却）
    async fn maybe_restart_process(&mut self) -> Result<()> {
        let cfg = self.cfg();
        // 配置/凭据类错误重启进程没有任何帮助，只会把状态搅乱
        if self.last_error_is_config {
            debug!("当前失败属于配置类问题，跳过重启进程（改配置才会生效）");
            return Ok(());
        }
        let threshold = cfg.keepalive.restart_after_failures.max(1);
        if self.failures < threshold {
            return Ok(());
        }

        // 冷却检查
        if let Some(last) = self.last_restart {
            let cooldown = Duration::from_secs(cfg.keepalive.cooldown_sec);
            if last.elapsed() < cooldown {
                debug!("处于冷却期（{:?}），暂不重启", cooldown - last.elapsed());
                return Ok(());
            }
        }

        warn!(
            "连续失败 {} 次（阈值 {}），重启 {} 进程",
            self.failures, threshold, cfg.natpierce.process_name
        );

        let name = &cfg.natpierce.process_name;
        let killed = process::kill_all(name)?;
        info!("已终止 {killed} 个进程");

        // 等待进程完全退出
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && process::is_running(name) {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;

        self.start_process().await?;
        self.failures = 0;
        self.restarts += 1;
        self.last_restart = Some(Instant::now());
        info!("重启完成（累计 {} 次）", self.restarts);
        Ok(())
    }

    /// 保活循环：一直跑到 `stop` 被置位
    pub async fn run(&mut self, mut stop: tokio::sync::watch::Receiver<bool>) -> Result<()> {
        let cfg = self.cfg().clone();
        let config_path = self.loaded.path.clone();

        // 启动时清除可能残留的停止标志，避免"一启动就退出"
        crate::stop_flag::clear(&config_path);

        info!(
            "保活启动 | 模式={} | 巡检间隔={}s | 心跳={}s | 失败阈值={}",
            cfg.mode, cfg.keepalive.interval_sec, cfg.keepalive.heartbeat_sec, cfg.keepalive.fail_threshold
        );

        // 注：「为皎月连启用自动开启」不在这里做 —— 守护进程刚启动时
        // 它多半还没登录，命令会被丢掉（配置里 Auto_start 一直是 0，
        // 日志却打了"已启用"，纯属误导）。改到 start_server 成功之后发，
        // 那时它必然已就绪。

        let interval = Duration::from_secs(cfg.keepalive.interval_sec.max(5));

        // 用 1 秒轮询而非直接睡一个巡检间隔 —— 这样停止标志能快速被响应
        let mut poll = tokio::time::interval(Duration::from_millis(1000));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        // 让首次巡检**立即**触发：把"上次检查时间"回拨一个间隔，
        // 否则 last_check.elapsed() 需要等满 interval 才满足条件，
        // 表现为"启动后要等 60 秒才第一次拉起皎月连"。
        let mut last_check = Instant::now()
            .checked_sub(interval)
            .unwrap_or_else(Instant::now);

        loop {
            tokio::select! {
                _ = poll.tick() => {
                    // ① 停止标志文件：界面请求退出，与权限无关
                    if crate::stop_flag::is_requested(&config_path) {
                        info!("检测到停止标志，保活优雅退出");
                        crate::stop_flag::clear(&config_path);
                        break;
                    }

                    // ② 巡检间隔到了才做完整检查
                    if last_check.elapsed() >= interval {
                        last_check = Instant::now();
                        if let Err(e) = self.tick().await {
                            warn!("巡检异常: {e:#}");
                        }
                    }
                }
                _ = stop.changed() => {
                    if *stop.borrow() {
                        info!("收到停止信号，保活退出");
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    /// 单次 ensure：不做循环，尽量把状态修到健康
    pub async fn ensure_once(&mut self) -> Result<Health> {
        let mut last = Health::Error("未执行".into());
        // 最多尝试 3 轮，给启动留时间
        for _ in 0..3 {
            last = self.tick().await?;
            if last.is_healthy() {
                break;
            }
            if !last.is_api_fixable() && !last.needs_process_restart() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        Ok(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_classification() {
        assert!(Health::Healthy.is_healthy());
        assert!(!Health::ServerStopped.is_healthy());

        assert!(Health::ServerStopped.is_api_fixable());
        assert!(Health::ClientDisconnected.is_api_fixable());
        assert!(!Health::ApiUnreachable.is_api_fixable());

        assert!(Health::ApiUnreachable.needs_process_restart());
        assert!(!Health::Healthy.needs_process_restart());
    }

    #[test]
    fn health_labels_non_empty() {
        for h in [
            Health::Healthy,
            Health::ProcessMissing,
            Health::ApiUnreachable,
            Health::ServerStopped,
            Health::ClientDisconnected,
        ] {
            assert!(!h.label().is_empty());
        }
    }
}
