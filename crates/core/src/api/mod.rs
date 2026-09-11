//! 皎月连本地控制接口
//!
//! - [`protocol`] — 报文格式、命令构造、消息解析
//! - [`client`]   — WebSocket 客户端与一次性探测

pub mod client;
pub mod protocol;

pub use client::{probe, ApiClient, ClientLink, ProbeResult};
