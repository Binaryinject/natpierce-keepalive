//! 本地控制接口的 WebSocket 客户端
//!
//! 负责：连接（含备用地址回退）、发送命令、收集响应、心跳。

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tracing::{debug, warn};

use super::protocol::{self, Message, SEP};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// 与本地控制接口的一条连接
pub struct ApiClient {
    ws: Ws,
    command_timeout: Duration,
    /// 已收集但尚未消费的消息
    pending: Vec<Message>,
}

impl ApiClient {
    /// 连接本地接口，失败时自动尝试备用地址
    pub async fn connect(
        url: &str,
        fallback_url: &str,
        connect_timeout: Duration,
        command_timeout: Duration,
    ) -> Result<Self> {
        let mut last_err = None;

        for candidate in [url, fallback_url] {
            if candidate.is_empty() {
                continue;
            }
            match tokio::time::timeout(connect_timeout, connect_async(candidate)).await {
                Ok(Ok((ws, _resp))) => {
                    debug!("已连接 {candidate}");
                    return Ok(Self {
                        ws,
                        command_timeout,
                        pending: Vec::new(),
                    });
                }
                Ok(Err(e)) => {
                    warn!("连接 {candidate} 失败: {e}");
                    last_err = Some(anyhow::anyhow!("{candidate}: {e}"));
                }
                Err(_) => {
                    warn!("连接 {candidate} 超时");
                    last_err = Some(anyhow::anyhow!("{candidate}: 连接超时"));
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("没有可用的 API 地址")))
    }

    /// 发送一条命令
    pub async fn send(&mut self, command: &str) -> Result<()> {
        debug!("→ {command}");
        self.ws
            .send(WsMessage::Text(command.to_string().into()))
            .await
            .context("发送命令失败（连接可能已断开）")?;
        Ok(())
    }

    /// 读取下一条消息（带超时），返回 None 表示超时
    pub async fn next_message(&mut self, timeout: Duration) -> Option<Message> {
        // 先看缓冲区
        if !self.pending.is_empty() {
            return Some(self.pending.remove(0));
        }

        let deadline = Instant::now() + timeout;
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                return None;
            }
            match tokio::time::timeout(remain, self.ws.next()).await {
                Ok(Some(Ok(WsMessage::Text(txt)))) => {
                    let msg = Message::parse(&txt);
                    debug!("← {txt}");
                    return Some(msg);
                }
                Ok(Some(Ok(WsMessage::Binary(_)))) => continue,
                Ok(Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_)))) => continue,
                Ok(Some(Ok(WsMessage::Close(_)))) => {
                    warn!("连接被服务端关闭");
                    return None;
                }
                Ok(Some(Ok(WsMessage::Frame(_)))) => continue,
                Ok(Some(Err(e))) => {
                    warn!("读取失败: {e}");
                    return None;
                }
                Ok(None) => {
                    warn!("连接已结束");
                    return None;
                }
                Err(_) => return None, // 超时
            }
        }
    }

    /// 在指定时间内收集所有消息
    pub async fn drain(&mut self, window: Duration) -> Vec<Message> {
        let mut out = Vec::new();
        let deadline = Instant::now() + window;
        while let Some(remain) = deadline.checked_duration_since(Instant::now()) {
            match self.next_message(remain).await {
                Some(m) => out.push(m),
                None => break,
            }
        }
        out
    }

    /// 发送命令并收集响应
    pub async fn request(&mut self, command: &str) -> Result<Vec<Message>> {
        self.send(command).await?;
        Ok(self.drain(self.command_timeout).await)
    }

    /// 心跳：发一条 pclist，保持连接活跃
    pub async fn heartbeat(&mut self) -> Result<()> {
        self.send(&protocol::cmd_pc_list()).await
    }

    /// 主动关闭
    pub async fn close(mut self) {
        let _ = self.ws.close(None).await;
    }
}

/// 一次性的状态查询结果
#[derive(Debug, Clone, Default)]
pub struct ProbeResult {
    /// 是否处于登录界面（未登录）
    pub need_login: bool,
    /// 服务端是否在运行；None 表示无法判定
    pub server_running: Option<bool>,
    /// 服务端概况
    pub server_info: Option<protocol::ServerInfo>,
    /// 在线主机
    pub hosts: Vec<protocol::HostEntry>,
    /// 原始消息（调试用）
    pub raw: Vec<String>,
    /// 收到的提示信息
    pub infos: Vec<String>,
}

/// 连一次、探一次、断开。用于 `status` 子命令和保活巡检。
pub async fn probe(
    url: &str,
    fallback_url: &str,
    connect_timeout: Duration,
    command_timeout: Duration,
) -> Result<ProbeResult> {
    let mut client = ApiClient::connect(url, fallback_url, connect_timeout, command_timeout).await?;

    let mut result = ProbeResult::default();

    // 1. 先收初始推送（握手后会立即推 1 或 2）
    let initial = client.drain(Duration::from_millis(1500)).await;
    for m in &initial {
        result.raw.push(format!("{m:?}"));
        absorb(&mut result, m);
    }

    // 2. 主动要一次主机列表
    if client.send(&protocol::cmd_pc_list()).await.is_ok() {
        let extra = client.drain(Duration::from_millis(2500)).await;
        for m in &extra {
            result.raw.push(format!("{m:?}"));
            absorb(&mut result, m);
        }
    }

    client.close().await;
    Ok(result)
}

fn absorb(r: &mut ProbeResult, m: &Message) {
    if matches!(m, Message::NeedLogin) {
        r.need_login = true;
        return;
    }
    if let Some(running) = m.is_server_running() {
        r.server_running = Some(running);
    }
    match m {
        Message::ServerRunning { info } | Message::ServerStopped { info } => {
            r.server_info = Some(info.clone());
        }
        Message::PcList(hosts) => {
            if !hosts.is_empty() {
                r.hosts = hosts.clone();
            }
        }
        Message::Info(s) => r.infos.push(s.clone()),
        _ => {}
    }
}

/// 把消息列表压缩成人类可读的一行摘要
pub fn summarize(msgs: &[Message]) -> String {
    let mut parts = Vec::new();
    for m in msgs {
        match m {
            Message::Started { version, .. } => parts.push(format!("软件 {version}")),
            Message::ServerRunning { .. } => parts.push("服务端=运行中".into()),
            Message::ServerStopped { .. } => parts.push("服务端=已停止".into()),
            Message::PcList(h) => parts.push(format!("在线主机 {} 台", h.len())),
            Message::Disconnected(_) => parts.push("远端断开".into()),
            Message::VipEnd => parts.push("VIP 到期".into()),
            Message::Unknown(s) if s.contains(SEP) => parts.push(format!("?{}", s.split(SEP).next().unwrap_or(""))),
            _ => {}
        }
    }
    if parts.is_empty() {
        "(无有效消息)".into()
    } else {
        parts.join(", ")
    }
}
