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
    /// 上次看到的配置文件指纹（用于热重载）
    config_fp: Option<crate::config::Fingerprint>,
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
            config_fp: fp,
        }
    }

    /// 检测配置文件是否被界面改动，若变了就重新加载
    ///
    /// 这是必需的：守护进程常驻运行，用户在界面里改完配置点保存时守护进程
    /// 并不会重启。没有热重载的话它会一直用启动时读到的旧配置
    /// —— 典型症状就是日志里反复出现旧的 exe 路径。
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

    /// 执行一次巡检 + 修复
    pub async fn tick(&mut self) -> Result<Health> {
        // 每轮先检查配置有没有被界面改动过
        self.reload_if_changed();

        let health = self.check().await;

        match health {
            Health::Healthy => {
                if self.failures > 0 {
                    info!("已恢复正常（此前连续失败 {} 次）", self.failures);
                }
                self.failures = 0;
                self.maybe_heartbeat().await;
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
                info!("与目标主机的连接已断开，尝试重连");
                match self.connect_target().await {
                    Ok(true) => {
                        self.repairs += 1;
                        self.failures = 0;
                        info!("已重新连接到目标主机");
                    }
                    Ok(false) => {
                        self.failures += 1;
                        warn!("重连未成功（第 {} 次）", self.failures);
                    }
                    Err(e) => {
                        self.failures += 1;
                        warn!("重连出错: {e:#}");
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
            Ok(r) => match cfg.mode {
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
            },
            Err(e) => {
                debug!("API 探测失败: {e:#}");
                Health::ApiUnreachable
            }
        }
    }

    /// 客户端模式下判断是否已连上目标主机
    fn client_connected(&self, r: &api::ProbeResult) -> bool {
        let cfg = self.cfg();
        let target_id = cfg.client.target_host_id.trim();
        let target_name = cfg.client.target_host_name.trim();

        if target_id.is_empty() && target_name.is_empty() && cfg.client.target_index == 0 {
            // 未配置目标：只要 API 通就算健康（纯保活，不锁定主机）
            return true;
        }

        // 在线主机列表里还能看到目标 → 说明本机与云端链路正常
        // 注意：列表里出现目标≠已连接。真正的"已连接"要看状态消息，
        // 这里采用保守策略：目标不在线 → 断线；目标在线 → 认为可用。
        if target_id.is_empty() {
            return r.hosts.iter().any(|h| h.name == target_name);
        }
        r.hosts.iter().any(|h| h.id == target_id)
    }

    /// 进程层：启动 natpierce（提权）
    async fn start_process(&mut self) -> Result<()> {
        let cfg = self.cfg();
        let exe = std::path::PathBuf::from(&cfg.natpierce.exe_path);
        let workdir = cfg.natpierce.resolve_working_dir();

        if !process::is_elevated() {
            warn!(
                "当前没有管理员权限，无法启动 {}。\
                 请以管理员身份运行，或安装为 Windows 服务 / 计划任务（SYSTEM）。",
                exe.display()
            );
            // 仍然尝试一次，让 UAC 有机会弹出
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
        let page_pwd = secret::resolve(
            &cfg.server.page_password,
            &secret::default_secrets_path(&self.loaded.path),
        )?;
        if page_pwd.is_empty() {
            anyhow::bail!("页面访问密码为空，无法开启服务端");
        }

        let mut client = api::ApiClient::connect(
            &cfg.api.url,
            &cfg.api.fallback_url,
            Duration::from_millis(cfg.api.connect_timeout_ms),
            Duration::from_millis(cfg.api.command_timeout_ms),
        )
        .await?;

        let cmd = protocol::cmd_start_server(
            &cfg.server.connection_password,
            cfg.server.max_clients,
            &page_pwd,
            &cfg.server.lan_ip,
        );
        client.send(&cmd).await?;

        let deadline = Instant::now() + Duration::from_secs(cfg.server.start_timeout_sec);
        let mut ok = false;
        while Instant::now() < deadline && !ok {
            match client.next_message(Duration::from_millis(1500)).await {
                Some(m) => {
                    if m.is_server_running() == Some(true) {
                        ok = true;
                    }
                }
                None => continue,
            }
        }
        client.close().await;
        Ok(ok)
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

    /// 从在线主机里挑出目标
    fn pick_target(&self, hosts: &[protocol::HostEntry]) -> Result<String> {
        let cfg = self.cfg();

        // 1. 明确指定了 ID
        let id = cfg.client.target_host_id.trim();
        if !id.is_empty() {
            if let Some(h) = hosts.iter().find(|h| h.id == id) {
                return Ok(h.id.clone());
            }
            anyhow::bail!("指定的目标主机 {id} 不在线");
        }

        // 2. 按名称匹配
        let name = cfg.client.target_host_name.trim();
        if !name.is_empty() {
            if let Some(h) = hosts
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case(name))
            {
                return Ok(h.id.clone());
            }
            anyhow::bail!("按名称 {name} 未匹配到在线主机");
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
