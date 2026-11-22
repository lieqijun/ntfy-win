//! 每个服务器一条 WebSocket 长连接，连接上订阅该服务器下所有主题。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
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
    State { server: String, state: ConnState },
}

pub type Sink = Arc<dyn Fn(NetEvent) + Send + Sync>;

enum Outcome {
    /// 认证失败，停止重试
    AuthFail,
    /// 普通的断开，继续退避重连
    Lost,
}

/// 把用户输入的地址规范成 WebSocket 端点
pub fn ws_endpoint(server: &str) -> String {
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
    format!("{base}/ws")
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
        let mut backoff: u64 = 1;
        let last_ids: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));

        loop {
            let outcome = connect_once(
                &server,
                &token,
                &topics,
                identity.clone(),
                &last_ids,
                &sink,
            )
            .await;

            match outcome {
                Outcome::AuthFail => {
                    sink(NetEvent::State { server: server.clone(), state: ConnState::AuthFailed });
                    return;
                }
                Outcome::Lost => {
                    sink(NetEvent::State {
                        server: server.clone(),
                        state: ConnState::Disconnected,
                    });
                }
            }

            // ±20% 抖动，避免多客户端同时重连
            let jitter = (backoff as f64 * 0.2 * pseudo_random()).round() as u64;
            let wait = backoff.saturating_add(jitter).max(1);
            tokio::time::sleep(Duration::from_secs(wait)).await;
            backoff = (backoff * 2).min(60);
            backoff = backoff.max(1);
        }
    })
}

async fn connect_once(
    server: &str,
    token: &str,
    topics: &[String],
    identity: Option<native_tls::Identity>,
    last_ids: &Arc<Mutex<HashMap<String, String>>>,
    sink: &Sink,
) -> Outcome {
    let url = ws_endpoint(server);

    let mut request = match url.clone().into_client_request() {
        Ok(r) => r,
        Err(_) => return Outcome::AuthFail,
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
            Err(_) => None,
        },
        None => None,
    };

    let connect = connect_async_tls_with_config(request, None, false, connector);
    let (mut ws, _resp) = match tokio::time::timeout(Duration::from_secs(15), connect).await {
        Ok(Ok(pair)) => pair,
        Ok(Err(err)) => {
            if is_auth_error(&err) {
                return Outcome::AuthFail;
            }
            return Outcome::Lost;
        }
        Err(_) => return Outcome::Lost,
    };

    // 订阅该服务器下的全部主题
    for topic in topics {
        let since = {
            let map = last_ids.lock().unwrap();
            map.get(topic).cloned().unwrap_or_else(|| "12h".to_string())
        };
        let payload = json!({ "id": topic, "topic": topic, "since": since });
        if ws.send(Message::Text(payload.to_string())).await.is_err() {
            return Outcome::Lost;
        }
    }

    sink(NetEvent::State { server: server.to_string(), state: ConnState::Connected });

    loop {
        let next = tokio::time::timeout(Duration::from_secs(90), ws.next()).await;
        let msg = match next {
            Err(_) => return Outcome::Lost,       // 90 秒没有任何数据，判定连接已死
            Ok(None) => return Outcome::Lost,
            Ok(Some(Err(err))) => {
                if is_auth_error(&err) {
                    return Outcome::AuthFail;
                }
                return Outcome::Lost;
            }
            Ok(Some(Ok(m))) => m,
        };

        match msg {
            Message::Text(text) => handle_frame(&text, last_ids, sink),
            Message::Binary(bytes) => {
                if let Ok(text) = String::from_utf8(bytes) {
                    handle_frame(&text, last_ids, sink);
                }
            }
            Message::Close(_) => return Outcome::Lost,
            _ => {}
        }
    }
}

fn handle_frame(text: &str, last_ids: &Arc<Mutex<HashMap<String, String>>>, sink: &Sink) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let event = value.get("event").and_then(|v| v.as_str()).unwrap_or("");

    // 记住消息 id，重连时用来补漏
    if let (Some(id), Some(topic)) = (
        value.get("id").and_then(|v| v.as_str()),
        value.get("topic").and_then(|v| v.as_str()),
    ) {
        if !id.is_empty() {
            last_ids.lock().unwrap().insert(topic.to_string(), id.to_string());
        }
    }

    if event != "message" {
        return;
    }

    let topic_name = value.get("topic").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let body = value
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // 消息没带标题时，用主题名当标题
    let title = if title.trim().is_empty() { topic_name.clone() } else { title };
    sink(NetEvent::Message { topic: topic_name, title, body });
}

fn is_auth_error(err: &tokio_tungstenite::tungstenite::Error) -> bool {
    use tokio_tungstenite::tungstenite::Error;
    match err {
        Error::Http(resp) => matches!(resp.status().as_u16(), 401 | 403),
        _ => false,
    }
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
