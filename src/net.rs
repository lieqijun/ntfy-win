//! 每个服务器一条 WebSocket 长连接，连接上订阅该服务器下所有主题。
//!
//! ntfy 的 WebSocket 端点格式是 `/<topic>/ws`，主题在路径里，多个主题用逗号分隔：
//! `wss://host/topic1,topic2/ws?since=12h`。没有裸 `/ws` 这种端点。
//! 连上之后客户端不需要再发任何消息，服务器直接推 JSON 帧。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async_tls_with_config, Connector};

#[derive(Clone, Debug, PartialEq)]
pub enum ConnState {
    Connected,
    Disconnected,
    AuthFailed,
}

#[derive(Clone, Debug)]
pub enum NetEvent {
    Message { topic: String, title: String, body: String },
    /// detail 在失败时带具体原因，成功时为空
    State { server: String, state: ConnState, detail: String },
}

pub type Sink = Arc<dyn Fn(NetEvent) + Send + Sync>;

enum Outcome {
    /// 认证失败或端点不存在，重试没有意义
    Fatal(String),
    /// 普通断开，继续退避重连
    Lost(String),
}

/// 把用户输入的地址规范成 WebSocket 端点。
/// since 为空时不带该参数 —— ntfy 在不带 since 时只推新消息，
/// 带 `since=12h` 会把 12 小时缓存一口气重放出来，开机就炸一堆通知。
pub fn ws_endpoint(server: &str, topics: &[String], since: &str) -> String {
    let s = server.trim().trim_end_matches('/');
    let base = if let Some(rest) = s.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = s.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if s.starts_with("ws://") || s.starts_with("wss://") {
        s.to_string()
    } else {
        format!("wss://{s}")
    };
    let path = topics.join(",");
    if since.is_empty() {
        format!("{base}/{path}/ws")
    } else {
        format!("{base}/{path}/ws?since={since}")
    }
}

/// 为一台服务器起一个常驻 task
pub fn spawn_server(
    handle: &tokio::runtime::Handle,
    server: String,
    token: String,
    topics: Vec<String>,
    identity: Option<native_tls::Identity>,
    sink: Sink,
) -> tokio::task::JoinHandle<()> {
    handle.spawn(async move {
        if topics.is_empty() {
            return;
        }

        let mut backoff: u64 = 1;
        // 该服务器上已收到的最后一条消息时间（Unix 秒），重连时用来补漏
        let last_seen: Arc<Mutex<Option<i64>>> = Arc::new(Mutex::new(None));
        // 本轮是否真的订阅成功过，用来决定要不要把退避重置
        let reached: Arc<std::sync::atomic::AtomicBool> =
            Arc::new(std::sync::atomic::AtomicBool::new(false));

        loop {
            reached.store(false, std::sync::atomic::Ordering::Relaxed);

            let outcome = connect_once(
                &server,
                &token,
                &topics,
                identity.clone(),
                &last_seen,
                &reached,
                &sink,
            )
            .await;

            // 连上过又断了，说明网络是通的，退避从头算，不要一直卡在 60 秒
            if reached.load(std::sync::atomic::Ordering::Relaxed) {
                backoff = 1;
            }

            match outcome {
                Outcome::Fatal(detail) => {
                    sink(NetEvent::State {
                        server: server.clone(),
                        state: ConnState::AuthFailed,
                        detail,
                    });
                    return;
                }
                Outcome::Lost(detail) => {
                    sink(NetEvent::State {
                        server: server.clone(),
                        state: ConnState::Disconnected,
                        detail,
                    });
                }
            }

            // ±20% 抖动，避免多客户端同时重连
            let jitter = (backoff as f64 * 0.2 * pseudo_random()).round() as u64;
            let wait = backoff.saturating_add(jitter).max(1);
            tokio::time::sleep(Duration::from_secs(wait)).await;
            backoff = (backoff * 2).clamp(1, 60);
        }
    })
}

async fn connect_once(
    server: &str,
    token: &str,
    topics: &[String],
    identity: Option<native_tls::Identity>,
    last_seen: &Arc<Mutex<Option<i64>>>,
    reached: &Arc<std::sync::atomic::AtomicBool>,
    sink: &Sink,
) -> Outcome {
    // 首次连上不带 since（只收新消息）；断线重连才从最后一条消息往后接，
    // 这样既不会漏掉断开期间的消息，也不会把整个缓存重放一遍
    let since = match *last_seen.lock().unwrap() {
        Some(t) => (t + 1).to_string(),
        None => String::new(),
    };

    let url = ws_endpoint(server, topics, &since);

    let mut request = match url.clone().into_client_request() {
        Ok(r) => r,
        Err(e) => return Outcome::Fatal(format!("服务器地址无效（{url}）：{e}")),
    };
    {
        let headers = request.headers_mut();
        if !token.is_empty() {
            if let Ok(v) = format!("Bearer {token}").parse() {
                headers.insert("authorization", v);
            }
        }
        if let Ok(v) = "ntfy-win/0.1".parse() {
            headers.insert("user-agent", v);
        }
    }

    let connector = match identity {
        Some(id) => match native_tls::TlsConnector::builder().identity(id).build() {
            Ok(tls) => Some(Connector::NativeTls(tls)),
            Err(e) => {
                return Outcome::Fatal(format!(
                    "客户端证书无法用于 TLS 握手：{e}\n请重新导出 pfx，确认勾选了包含私钥。"
                ))
            }
        },
        None => None,
    };

    let connect = connect_async_tls_with_config(request, None, false, connector);
    let (mut ws, _resp) = match tokio::time::timeout(Duration::from_secs(15), connect).await {
        Ok(Ok(pair)) => pair,
        Ok(Err(err)) => {
            let detail = describe(&err, &url);
            return if is_fatal(&err) { Outcome::Fatal(detail) } else { Outcome::Lost(detail) };
        }
        Err(_) => {
            return Outcome::Lost(format!(
                "连接 {url} 超时（15 秒）。可能是地址或端口不通、证书被拒绝，或服务器没开。"
            ))
        }
    };

    // 连上不等于订阅成功：ntfy 会先回一帧 {"event":"open","topic":"..."}。
    // 等不到这一帧就说明主题没订上，不能算连接成功。
    let mut announced = false;
    loop {
        let wait: u64 = if announced { 90 } else { 20 };
        let next = tokio::time::timeout(Duration::from_secs(wait), ws.next()).await;

        let msg = match next {
            Err(_) => {
                return if announced {
                    Outcome::Lost("90 秒没收到任何数据，判定连接已死。".to_string())
                } else {
                    Outcome::Lost(format!(
                        "已建立连接，但 20 秒内没收到 ntfy 的 open 事件。\n\
                         端点：{url}\n主题是否拼错？服务器是否支持 WebSocket？"
                    ))
                }
            }
            Ok(None) => return Outcome::Lost("服务器关闭了连接。".to_string()),
            Ok(Some(Err(err))) => {
                let detail = describe(&err, &url);
                return if is_fatal(&err) { Outcome::Fatal(detail) } else { Outcome::Lost(detail) };
            }
            Ok(Some(Ok(m))) => m,
        };

        match msg {
            Message::Text(text) => {
                take_open(&text, &mut announced, server, reached, sink);
                handle_frame(&text, last_seen, sink);
            }
            Message::Binary(bytes) => {
                if let Ok(text) = String::from_utf8(bytes) {
                    take_open(&text, &mut announced, server, reached, sink);
                    handle_frame(&text, last_seen, sink);
                }
            }
            Message::Close(frame) => {
                let why = frame
                    .map(|f| format!("（{} {}）", u16::from(f.code), f.reason))
                    .unwrap_or_default();
                return Outcome::Lost(format!("服务器关闭了连接{why}"));
            }
            _ => {}
        }
    }
}

/// 收到 ntfy 的 open 帧才算订阅成功，这时候才敢对外报“已连接”
fn take_open(
    text: &str,
    announced: &mut bool,
    server: &str,
    reached: &Arc<std::sync::atomic::AtomicBool>,
    sink: &Sink,
) {
    if *announced || !is_open_event(text) {
        return;
    }
    *announced = true;
    reached.store(true, std::sync::atomic::Ordering::Relaxed);
    sink(NetEvent::State {
        server: server.to_string(),
        state: ConnState::Connected,
        detail: String::new(),
    });
}

fn is_open_event(text: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => v.get("event").and_then(|e| e.as_str()) == Some("open"),
        Err(_) => false,
    }
}

fn handle_frame(text: &str, last_seen: &Arc<Mutex<Option<i64>>>, sink: &Sink) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let event = value.get("event").and_then(|v| v.as_str()).unwrap_or("");

    // 记住最后一条消息的时间，重连时据此补漏
    if event == "message" {
        if let Some(t) = value.get("time").and_then(|v| v.as_i64()) {
            let mut guard = last_seen.lock().unwrap();
            let current = *guard;
            if current.map(|old| t > old).unwrap_or(true) {
                *guard = Some(t);
            }
        }
    }

    if event != "message" {
        return;
    }

    let topic_name = value.get("topic").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let title = value.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let body = value.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string();

    // 消息没带标题时，用主题名当标题
    let title = if title.trim().is_empty() { topic_name.clone() } else { title };
    sink(NetEvent::Message { topic: topic_name, title, body });
}

/// tungstenite 的 Display 已经带了足够信息（HTTP 状态码、TLS 错误文本），
/// 直接用它，比按变体拆更不容易写错。这里只额外补一句证书链的中文提示。
fn describe(err: &tokio_tungstenite::tungstenite::Error, url: &str) -> String {
    let raw = format!("{err}");
    let lower = raw.to_lowercase();

    if lower.contains("certificate")
        || lower.contains("cert")
        || lower.contains("unknown ca")
        || lower.contains("self signed")
    {
        return format!(
            "服务器证书不被信任。\n端点：{url}\n原始错误：{raw}\n\n\
             如果是自建服务器用了自签证书，需要把签发它的根证书导入\n\
             「受信任的根证书颁发机构」，或者换成受信任的证书。"
        );
    }

    if lower.contains("401") || lower.contains("403") {
        return format!("服务器拒绝访问（令牌或证书权限不足）。\n端点：{url}\n原始错误：{raw}");
    }

    if lower.contains("404") {
        return format!(
            "服务器上没有这个端点（404）。\n端点：{url}\n\
             主题名可能拼错了，或者这台服务器不是 ntfy。"
        );
    }

    format!("连接失败。\n端点：{url}\n原始错误：{raw}")
}

/// 认证失败 / 端点不存在 —— 这类错误重试多少次都一样。
/// 5xx 不算，服务器临时故障还是应该重试。
fn is_fatal(err: &tokio_tungstenite::tungstenite::Error) -> bool {
    let raw = format!("{err}");
    raw.contains("401") || raw.contains("403") || raw.contains("404")
}

/// 够用的伪随机，避免为了抖动引入 rand
fn pseudo_random() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1000) as f64 / 1000.0
}
