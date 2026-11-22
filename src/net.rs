//! 每个服务器一条 WebSocket 长连接，连接上订阅该服务器下所有主题。
//!
//! ntfy 的 WebSocket 端点格式是 `/<topic>/ws`，主题在路径里，多个主题用逗号分隔：
//! `wss://host/topic1,topic2/ws?since=12h`。没有裸 `/ws` 这种端点。
//! 连上之后客户端不需要再发任何消息，服务器直接推 JSON 帧。
//!
//! 这里用的是阻塞 IO + 一服务器一线程：TLS 由 SChannel 直接接手系统证书库里的
//! 证书句柄，tokio 那套只能吃 pfx 文件，用不上。

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use schannel::tls_stream;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::HeaderValue;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::Message;

#[derive(Clone, Debug, PartialEq)]
pub enum ConnState {
    Connected,
    Disconnected,
    AuthFailed,
}

#[derive(Clone, Debug)]
pub enum NetEvent {
    Message { topic: String, title: String, body: String },
    /// detail 在失败时带具体原因，成功时为空。
    /// first = true 表示「这条连接压根没连通过」，只有这种失败才值得弹框打扰人；
    /// 连上过之后的掉线属于正常重连，只更新托盘提示。
    State { server: String, state: ConnState, detail: String, first: bool },
}

pub type Sink = Arc<dyn Fn(NetEvent) + Send + Sync>;

/// 明文和 TLS 两种底层用一个枚举收拢，省得为了 Read/Write 上 trait object
enum Sock {
    Plain(TcpStream),
    Tls(Box<tls_stream::TlsStream<TcpStream>>),
}

impl Read for Sock {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Sock::Plain(s) => s.read(buf),
            Sock::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Sock {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Sock::Plain(s) => s.write(buf),
            Sock::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Sock::Plain(s) => s.flush(),
            Sock::Tls(s) => s.flush(),
        }
    }
}


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

/// 为一台服务器起一个常驻线程。stop 置位后线程会在下一次检查时退出。
pub fn spawn_server(
    server: String,
    token: String,
    thumb: Option<String>,
    topics: Vec<String>,
    sink: Sink,
    stop: Arc<AtomicBool>,
) {
    let name = format!("ws {server}");
    let spawned = std::thread::Builder::new().name(name).spawn(move || {
        if topics.is_empty() {
            return;
        }

        let mut backoff: u64 = 1;
        // 该服务器上已收到的最后一条消息时间（Unix 秒），重连时用来补漏
        let last_seen: Arc<Mutex<Option<i64>>> = Arc::new(Mutex::new(None));
        // 本轮是否真的订阅成功过，用来决定要不要把退避重置
        let reached = Arc::new(AtomicBool::new(false));
        // 这条服务器有没有「连通过」。连通过之后再掉线只是重连，不该再弹框。
        let mut ever_connected = false;

        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            reached.store(false, Ordering::Relaxed);

            let outcome = connect_once(
                &server,
                &token,
                thumb.as_deref(),
                &topics,
                &last_seen,
                &reached,
                &sink,
                &stop,
            );

            // 连上过又断了，说明网络是通的，退避从头算，不要一直卡在 60 秒
            if reached.load(Ordering::Relaxed) {
                backoff = 1;
                ever_connected = true;
            }

            match outcome {
                Outcome::Fatal(detail) => {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    sink(NetEvent::State {
                        server: server.clone(),
                        state: ConnState::AuthFailed,
                        detail,
                        // 认证/端点错通常要人动手改配置，照旧弹一次
                        first: true,
                    });
                    return;
                }
                Outcome::Lost(detail) => {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    sink(NetEvent::State {
                        server: server.clone(),
                        state: ConnState::Disconnected,
                        detail,
                        first: !ever_connected,
                    });
                }
            }

            // ±20% 抖动，避免多客户端同时重连。按 200ms 切片睡，收到停止信号能早点走。
            let jitter = (backoff as f64 * 0.2 * pseudo_random()).round() as u64;
            let wait = backoff.saturating_add(jitter).max(1);
            for _ in 0..(wait * 5) {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            backoff = (backoff * 2).clamp(1, 60);
        }
    });

    // Ok 就 detach（停止靠 stop 标志），Err 说明线程起不来，也只能作罢
    let _ = spawned;
}

fn connect_once(
    server: &str,
    token: &str,
    thumb: Option<&str>,
    topics: &[String],
    last_seen: &Arc<Mutex<Option<i64>>>,
    reached: &Arc<AtomicBool>,
    sink: &Sink,
    stop: &Arc<AtomicBool>,
) -> Outcome {
    // 首次连上不带 since（只收新消息）；断线重连才从最后一条消息往后接，
    // 这样既不会漏掉断开期间的消息，也不会把整个缓存重放一遍
    let since = match *last_seen.lock().unwrap() {
        Some(t) => (t + 1).to_string(),
        None => String::new(),
    };

    let url = ws_endpoint(server, topics, &since);

    let uri = match url.parse::<tungstenite::http::Uri>() {
        Ok(u) => u,
        Err(e) => return Outcome::Fatal(format!("服务器地址无效（{url}）：{e}")),
    };

    // 只有 wss 才走 TLS，其余按明文处理
    let secure = uri.scheme_str() == Some("wss");
    let host = match uri.host() {
        Some(h) if !h.is_empty() => h.to_string(),
        _ => return Outcome::Fatal(format!("地址里没有主机名：{url}")),
    };
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });

    // ---------------- TCP ----------------
    let addrs: Vec<_> = match (host.as_str(), port).to_socket_addrs() {
        Ok(it) => it.collect(),
        Err(e) => return Outcome::Lost(format!("解析不了「{host}」：{e}")),
    };
    if addrs.is_empty() {
        return Outcome::Lost(format!("「{host}」解析不出任何地址，DNS 可能有问题。"));
    }

    let mut last_err = String::new();
    let mut connected = None;
    for addr in &addrs {
        match TcpStream::connect_timeout(addr, Duration::from_secs(15)) {
            Ok(s) => {
                connected = Some(s);
                break;
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    let Some(tcp) = connected else {
        return Outcome::Lost(format!(
            "连不上 {host}:{port}（15 秒超时）。\n{last_err}\n\n\
             地址或端口不通，也可能被防火墙挡了。"
        ));
    };

    let _ = tcp.set_nodelay(true);
    // 握手阶段用短超时，握手完成后按下面的说明重设（见「空闲不算断开」那段）
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(15)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(15)));
    // 留一份句柄：TlsStream 拿走所有权后还能改超时
    let ctl = tcp.try_clone().ok();

    // ---------------- TLS ----------------
    let stream: Sock = if secure {
        let cred = match thumb {
            Some(t) => match crate::cert::find(t) {
                Some(cert) => match crate::cert::client_cred(cert) {
                    Ok(c) => c,
                    Err(e) => {
                        return Outcome::Fatal(format!(
                            "证书 {t} 无法用于 TLS 握手：{e}\n\
                             私钥可能不可用（受保护、被锁，或装在了别的账户下）。"
                        ))
                    }
                },
                None => {
                    return Outcome::Fatal(format!(
                        "系统证书库里找不到指纹 {t} 的证书。\n\n\
                         确认三件事：\n\
                         1. 证书装在「当前用户 → 个人 → 证书」或「本地计算机 → 个人 → 证书」里；\n\
                         2. 指纹是 certmgr 里那串 40 位十六进制，没抄错位；\n\
                         3. 导出时勾了「包含私钥」，或者本来就是这台机器上申请的。"
                    ))
                }
            },
            None => match crate::cert::anonymous_cred() {
                Ok(c) => c,
                Err(e) => return Outcome::Lost(format!("初始化 TLS 失败：{e}")),
            },
        };

        let mut builder = tls_stream::Builder::new();
        builder.domain(&host);
        match builder.connect(cred, tcp) {
            Ok(s) => Sock::Tls(Box::new(s)),
            Err(e) => {
                return Outcome::Lost(format!(
                    "TLS 握手失败（{host}）。\n{e}\n\n\
                     如果是自建服务器用了自签证书，需要把签发它的根证书导入\n\
                     「受信任的根证书颁发机构」，否则服务器身份没法验证。"
                ))
            }
        }
    } else {
        Sock::Plain(tcp)
    };

    // ---------------- WebSocket 握手 ----------------
    let mut request = match url.clone().into_client_request() {
        Ok(r) => r,
        Err(e) => return Outcome::Fatal(format!("服务器地址无效（{url}）：{e}")),
    };
    {
        let headers = request.headers_mut();
        if !token.is_empty() {
            if let Ok(v) = HeaderValue::from_str(&format!("Bearer {token}")) {
                headers.insert("authorization", v);
            }
        }
        headers.insert(
            "user-agent",
            HeaderValue::from_static(concat!("ntfy-win/", env!("CARGO_PKG_VERSION"))),
        );
    }

    let config = WebSocketConfig {
        max_message_size: Some(4 * 1024 * 1024),
        max_frame_size: Some(4 * 1024 * 1024),
        ..Default::default()
    };

    let (mut ws, _resp) = match tungstenite::client::client_with_config(request, stream, Some(config)) {
        Ok(v) => v,
        Err(e) => {
            let raw = format!("{e}");
            let detail = describe(&raw, &url);
            return if is_fatal(&raw) { Outcome::Fatal(detail) } else { Outcome::Lost(detail) };
        }
    };

    // ntfy 的 WebSocket 是单向推送：订阅一个安静的主题时，服务器可以十几分钟
    // 一个字节都不发（它的 keepalive 是否发 ping 取决于版本和配置）。所以
    // 「安静」绝对不能当成「断开」——2.0.0 就是拿 90 秒静默当死亡，结果空闲
    // 连接每分半被自己掐一次，托盘刚变蓝又弹「无法连接」。
    //
    // 判断真死链交给 TCP keepalive：下面给 socket 开了 30 秒探测 + 10 秒间隔，
    // 对端真没了会在 1 分钟内以 RST 形式报上来，read 直接返回错误。
    // 读超时缩到 20 秒，只是为了让线程定期醒一下（好及时看到停止标志），
    // 醒来没数据就继续等，不算失败。
    if let Some(c) = &ctl {
        let _ = c.set_read_timeout(Some(Duration::from_secs(20)));
        let keepalive = socket2::TcpKeepalive::new()
            .with_time(Duration::from_secs(30))
            .with_interval(Duration::from_secs(10));
        let _ = socket2::SockRef::from(c).set_tcp_keepalive(&keepalive);
    }
    let settled_at = std::time::Instant::now();

    // 连上不等于订阅成功：ntfy 会先回一帧 {"event":"open","topic":"..."}。
    // 等不到这一帧就说明主题没订上，不能算连接成功。
    let mut announced = false;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Outcome::Lost(String::new());
        }

        let msg = match ws.read() {
            Ok(m) => m,
            Err(e) => {
                if is_timeout(&e) {
                    // 空闲连接：没消息就是没帧，接着等。
                    // 只有「握手成功但一直等不到 open 事件」才说明订阅没成功。
                    if !announced && settled_at.elapsed() >= Duration::from_secs(60) {
                        return Outcome::Lost(format!(
                            "连上了，但一分钟内没收到 ntfy 的 open 事件。\n\
                             端点：{url}\n主题是否拼错？服务器是否支持 WebSocket？"
                        ));
                    }
                    continue;
                }
                let raw = format!("{e}");
                let detail = describe(&raw, &url);
                return if is_fatal(&raw) { Outcome::Fatal(detail) } else { Outcome::Lost(detail) };
            }
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
            // Ping/Pong 由 tungstenite 自己排队应答，这里把排队的帧冲出去
            Message::Ping(_) | Message::Pong(_) => {
                let _ = ws.flush();
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
    reached: &Arc<AtomicBool>,
    sink: &Sink,
) {
    if *announced || !is_open_event(text) {
        return;
    }
    *announced = true;
    reached.store(true, Ordering::Relaxed);
    sink(NetEvent::State {
        server: server.to_string(),
        state: ConnState::Connected,
        detail: String::new(),
        first: false,
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

/// 底层 socket 的读超时。这**不**代表连接死了（见 connect_once 里的说明），
/// 只说明这段时间一个字节都没来。
fn is_timeout(err: &tungstenite::Error) -> bool {
    if let tungstenite::Error::Io(e) = err {
        return matches!(
            e.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        );
    }
    let lower = format!("{err}").to_lowercase();
    lower.contains("timed out") || lower.contains("10060") || lower.contains("would block")
}

/// 错误文本里已经带了足够信息（HTTP 状态码、TLS 错误），直接用它，
/// 只额外补一句自签证书的中文提示。
fn describe(raw: &str, url: &str) -> String {
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
fn is_fatal(raw: &str) -> bool {
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
