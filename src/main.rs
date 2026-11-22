#![windows_subsystem = "windows"]

mod cert;
mod config;
mod net;
mod notify;
mod tray;
mod ui;

use std::rc::Rc;

use native_windows_gui as nwg;

use config::Sub;
use net::{ConnState, NetEvent};
use ui::{Ctx, Ui};

/// 失败原因只弹一次框，免得退避重连时每 60 秒糊一个对话框在脸上
static FAIL_REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 托盘 tooltip 整个只有 128 个字符，取失败原因的第一行再截断
fn brief(detail: &str) -> String {
    let line = detail.lines().next().unwrap_or("").trim();
    let count = line.chars().count();
    let mut out: String = line.chars().take(48).collect();
    if count > 48 {
        out.push('…');
    }
    out
}

fn main() {
    if nwg::init().is_err() {
        return;
    }
    if !acquire_single_instance() {
        return;
    }

    notify::register_app_id();

    let (ui, ctx) = match Ui::build() {
        Ok(v) => v,
        Err(_) => return,
    };
    let ui = Rc::new(ui);
    let ctx = Rc::new(ctx);

    {
        let (autostart, auto_copy) = {
            let cfg = ctx.cfg.borrow();
            (cfg.settings.autostart, cfg.settings.auto_copy)
        };
        *ctx.tray.borrow_mut() = tray::Tray::new(autostart, auto_copy);
    }

    ui.refresh_list(&ctx.cfg.borrow());
    restart_network(&ctx);

    // 200ms 一次节拍：轮询托盘点击/菜单事件、同步「移除」按钮状态。
    // 用系统定时器，不再占用 nwg AnimationTimer 的那个 1ms 轮询线程。
    ui.start_tick(200);

    let handlers = bind_handlers(&ui, &ctx);
    // --silent 用于开机自启：只在托盘，不弹窗口
    let silent = std::env::args().any(|a| a == "--silent");
    ui.window.set_visible(!silent);

    nwg::dispatch_thread_events();

    for h in &handlers {
        nwg::unbind_event_handler(h);
    }
    ui.stop_tick();
}

// ---------------------------------------------------------------- 单实例

fn acquire_single_instance() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let name: Vec<u16> = "Local\\NtfyWin-SingleInstance\0".encode_utf16().collect();
        let handle = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if handle.is_null() {
            return true;
        }
        GetLastError() != ERROR_ALREADY_EXISTS
    }
    #[cfg(not(windows))]
    true
}

// ---------------------------------------------------------------- 事件

fn bind_handlers(ui: &Rc<Ui>, ctx: &Rc<Ctx>) -> Vec<nwg::EventHandler> {
    let mut handlers = Vec::new();
    let targets = [ui.window.handle, ui.sub_dlg.handle, ui.set_dlg.handle];

    for hwnd in targets {
        let ui = ui.clone();
        let ctx = ctx.clone();

        let handler = nwg::full_bind_event_handler(&hwnd, move |evt, data, handle| {
            match evt {
                nwg::Event::OnButtonClick => {
                    if handle == ui.btn_add.handle {
                        let cfg = ctx.cfg.borrow();
                        ui.open_subscribe(&cfg);
                    } else if handle == ui.btn_settings.handle {
                        let cfg = ctx.cfg.borrow();
                        ui.open_settings(&cfg);
                    } else if handle == ui.btn_remove.handle {
                        remove_selected(&ui, &ctx);
                    } else if handle == ui.sub_link.handle {
                        ui.set_advanced(true);
                        // 「使用其他服务器」就是要换成别的机器，这里必须把
                        // 沿用上次的服务器/令牌/指纹全部清空，否则一不小心
                        // 就把令牌发到另一台服务器上了
                        ui.sub_server.set_text("");
                        ui.sub_token.set_text("");
                        ui.sub_thumb.set_text("");
                        ui.sub_server.set_focus();
                    } else if handle == ui.sub_ok.handle {
                        commit_subscribe(&ui, &ctx);
                    } else if handle == ui.sub_cancel.handle {
                        ui.close_subscribe();
                    } else if handle == ui.set_ok.handle {
                        commit_settings(&ui, &ctx);
                    } else if handle == ui.set_cancel.handle {
                        ui.close_settings();
                    }
                }
                nwg::Event::OnWindowClose => {
                    // 托盘程序：关闭一律只是隐藏，绝不销毁窗口
                    if let nwg::EventData::OnWindowClose(info) = data {
                        info.close(false);
                    }
                    if handle == ui.window.handle {
                        ui.window.set_visible(false);
                    } else if handle == ui.sub_dlg.handle {
                        ui.close_subscribe();
                    } else if handle == ui.set_dlg.handle {
                        ui.close_settings();
                    }
                }
                nwg::Event::OnTimerTick => {
                    poll_tray(&ui, &ctx);
                }
                nwg::Event::OnNotice => {
                    drain_events(&ctx);
                }
                _ => {}
            }
        });

        handlers.push(handler);
    }

    handlers
}

// ---------------------------------------------------------------- 托盘

fn poll_tray(ui: &Rc<Ui>, ctx: &Rc<Ctx>) {
    // 「移除」只在选中行之后可点，跟着选择状态每 200ms 对一次
    ui.sync_remove_enabled(ctx.cfg.borrow().subscriptions.len());

    // 左键单击托盘图标 = 打开主窗口
    if tray::left_clicked() {
        ui.show_main();
    }

    while let Some(id) = tray::next_menu_id() {
        match id.as_str() {
            "open" => {
                ui.show_main();
            }
            "add" => {
                ui.show_main();
                let cfg = ctx.cfg.borrow();
                ui.open_subscribe(&cfg);
            }
            "settings" => {
                ui.show_main();
                let cfg = ctx.cfg.borrow();
                ui.open_settings(&cfg);
            }
            "autostart" => {
                let value = !ctx.cfg.borrow().settings.autostart;
                {
                    let mut cfg = ctx.cfg.borrow_mut();
                    cfg.settings.autostart = value;
                    let _ = cfg.save();
                }
                config::set_autostart(value);
                if let Some(tray) = ctx.tray.borrow().as_ref() {
                    tray.autostart.set_checked(value);
                }
            }
            "auto_copy" => {
                let value = !ctx.cfg.borrow().settings.auto_copy;
                {
                    let mut cfg = ctx.cfg.borrow_mut();
                    cfg.settings.auto_copy = value;
                    let _ = cfg.save();
                }
                if let Some(tray) = ctx.tray.borrow().as_ref() {
                    tray.auto_copy.set_checked(value);
                }
            }
            "quit" => std::process::exit(0),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- 网络事件

fn drain_events(ctx: &Rc<Ctx>) {
    let queued: Vec<NetEvent> = {
        let mut queue = ctx.events.lock().unwrap();
        queue.drain(..).collect()
    };

    for event in queued {
        match event {
            NetEvent::Message { title, body, .. } => {
                notify::show(&title, &body);
                if ctx.cfg.borrow().settings.auto_copy {
                    notify::copy_to_clipboard(&body);
                }
            }
            NetEvent::State { state, detail, first, .. } => {
                let connected = state == ConnState::Connected;
                let tip = match state {
                    ConnState::Connected => "ntfy · 已连接".to_string(),
                    ConnState::Disconnected => {
                        format!("ntfy · 重连中：{}", brief(&detail))
                    }
                    ConnState::AuthFailed => {
                        format!("ntfy · 认证失败：{}", brief(&detail))
                    }
                };
                if ctx.connected.replace(connected) != connected || !connected {
                    if let Some(tray) = ctx.tray.borrow().as_ref() {
                        tray.set_state(connected, &tip);
                    }
                }

                // 只有「压根没连通过」的失败才弹框。连上之后再掉线属于正常重连，
                // 只更新托盘提示——否则网络抖一下就在脸上糊一个「无法连接」，
                // 而托盘图标明明是蓝的。
                if state != ConnState::Connected && first && !detail.is_empty() {
                    if !FAIL_REPORTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        notify::message(
                            &format!(
                                "{detail}\n\n\
                                 这条提示只显示一次。之后把鼠标移到托盘图标上，\n\
                                 就能看到当前的失败原因。"
                            ),
                            match state {
                                ConnState::AuthFailed => "无法连接（认证或端点错误）",
                                _ => "无法连接，正在重试",
                            },
                        );
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- 订阅

fn commit_subscribe(ui: &Rc<Ui>, ctx: &Rc<Ctx>) {
    let topic = ui
        .sub_topic
        .text()
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_string();
    if topic.is_empty() {
        ui.sub_topic.set_focus();
        return;
    }

    let advanced = ui.advanced.get();

    let server = if advanced {
        // 展开「使用其他服务器」之后地址是空的，空着提交会被 normalize
        // 兜成 ntfy.sh —— 那等于把消息悄悄订到公共服务器上，必须当场拦住
        if ui.sub_server.text().trim().is_empty() {
            notify::message(
                "服务器地址不能为空。\n\n\
                 展开「使用其他服务器」后需要填完整地址，例如\n\
                 https://ntfy.example.com\n\n\
                 如果只是想用上次那台服务器，取消这次订阅重新点「订阅新主题」即可。",
                "缺少服务器地址",
            );
            ui.sub_server.set_focus();
            return;
        }
        normalize_server(&ui.sub_server.text())
    } else {
        let cfg = ctx.cfg.borrow();
        if cfg.last_server.is_empty() {
            "https://ntfy.sh".to_string()
        } else {
            cfg.last_server.clone()
        }
    };

    let thumb = if advanced {
        match read_thumb(ui) {
            Ok(v) => v,
            // 已经弹过提示了，这次提交作废
            Err(()) => return,
        }
    } else {
        ctx.cfg.borrow().cert_thumb_of(&server)
    };

    {
        let mut cfg = ctx.cfg.borrow_mut();

        if advanced {
            let token = ui.sub_token.text();
            cfg.set_token(&server, &token);
            cfg.set_cert_thumb(&server, thumb);
        } else if !cfg.servers.contains_key(&server) {
            cfg.servers.entry(server.clone()).or_default();
        }

        let exists = cfg
            .subscriptions
            .iter()
            .any(|s| s.topic == topic && s.server == server);
        if !exists {
            cfg.subscriptions.push(Sub { topic: topic.clone(), server: server.clone() });
        }
        cfg.last_server = server.clone();
        let _ = cfg.save();
    }

    ui.close_subscribe();
    ui.refresh_list(&ctx.cfg.borrow());
    restart_network(ctx);
}

fn remove_selected(ui: &Rc<Ui>, ctx: &Rc<Ctx>) {
    let index = {
        let cfg = ctx.cfg.borrow();
        match ui.selected_sub(&cfg) {
            Some(i) => i,
            None => return,
        }
    };

    {
        let mut cfg = ctx.cfg.borrow_mut();
        if index < cfg.subscriptions.len() {
            cfg.subscriptions.remove(index);
            let _ = cfg.save();
        }
    }

    ui.refresh_list(&ctx.cfg.borrow());
    restart_network(ctx);
}

// ---------------------------------------------------------------- 证书指纹

/// 读订阅对话框里的指纹输入框并校验。
/// 返回 `Err(())` 表示已经弹过提示、调用方应当整个放弃这次提交。
fn read_thumb(ui: &Rc<Ui>) -> Result<Option<String>, ()> {
    let raw = ui.sub_thumb.text();
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }

    let Some(thumb) = cert::normalize(raw) else {
        notify::message(
            "指纹应该是 40 位十六进制（certmgr 证书详情里那串，可以带空格）。\n\n\
             在 certmgr 里双击证书 →「详细信息」→「指纹」，复制过来就行。",
            "指纹格式不对",
        );
        ui.sub_thumb.set_focus();
        return Err(());
    };

    let Some(found) = cert::find(&thumb) else {
        notify::message(
            &format!(
                "系统证书库里找不到指纹为\n{thumb}\n的证书。\n\n\
                 请确认三件事：\n\
                 1. 证书装在「当前用户 → 个人 → 证书」或「本地计算机 → 个人 → 证书」里；\n\
                 2. 指纹没有抄错位（40 位，只认十六进制字符）；\n\
                 3. 这是本机申请或导入时带了私钥的证书。"
            ),
            "找不到证书",
        );
        ui.sub_thumb.set_focus();
        return Err(());
    };

    if !cert::has_private_key(&found) {
        notify::message(
            &format!(
                "证书 {} 没有可用的私钥，做不了客户端认证。\n\n\
                 私钥可能没跟着证书一起导入，或者被标记为不可导出。",
                cert::describe(&found)
            ),
            "证书没有私钥",
        );
        ui.sub_thumb.set_focus();
        return Err(());
    }

    Ok(Some(thumb))
}

fn commit_settings(ui: &Rc<Ui>, ctx: &Rc<Ctx>) {
    let autostart = ui.set_autostart.check_state() == nwg::CheckBoxState::Checked;
    let auto_copy = ui.set_copy.check_state() == nwg::CheckBoxState::Checked;

    {
        let mut cfg = ctx.cfg.borrow_mut();
        cfg.settings.autostart = autostart;
        cfg.settings.auto_copy = auto_copy;
        let _ = cfg.save();
    }

    config::set_autostart(autostart);
    if let Some(tray) = ctx.tray.borrow().as_ref() {
        tray.set_checked(autostart, auto_copy);
    }
    ui.close_settings();
}

// ---------------------------------------------------------------- 连接调度

fn restart_network(ctx: &Rc<Ctx>) {
    // 先让旧线程收手：置位后它们会在下一次检查时退出。
    // 不强求 join —— 阻塞在 read 上的线程最多 20 秒才醒，等它会把界面卡住。
    {
        let mut stops = ctx.stops.lock().unwrap();
        for stop in stops.drain(..) {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    // 换一代：旧线程晚到的事件靠代次号丢掉，免得把新连接的状态覆盖成旧的
    let generation = ctx
        .generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        + 1;

    // 配置改了就要重新给一次提示的机会
    FAIL_REPORTED.store(false, std::sync::atomic::Ordering::SeqCst);

    let mut by_server: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    {
        let cfg = ctx.cfg.borrow();
        for sub in &cfg.subscriptions {
            by_server
                .entry(sub.server.clone())
                .or_default()
                .push(sub.topic.clone());
        }
    }

    for (server, topics) in by_server {
        let (token, thumb, cert_err) = {
            let cfg = ctx.cfg.borrow();
            let token = cfg.token(&server);
            let mut err = None;
            // 配置里只有指纹，这里把它换成证书库里的实际证书再验一遍。
            // 用户可能已经把证书从证书库里删了，那种情况不该硬连。
            let thumb = match cfg.cert_thumb_of(&server) {
                Some(t) => match cert::find(&t) {
                    Some(c) => {
                        if !cert::has_private_key(&c) {
                            err = Some(format!(
                                "{server} 的客户端证书没有可用私钥，无法做客户端认证。\n\
                                 证书：{}\n指纹：{t}\n\n\
                                 请把带私钥的证书重新装进「个人」证书库。",
                                cert::describe(&c)
                            ));
                            None
                        } else {
                            Some(t)
                        }
                    }
                    None => {
                        err = Some(format!(
                            "{server} 用的客户端证书在系统证书库里找不到了。\n\
                             指纹：{t}\n\n\
                             请在订阅窗口重新填一次指纹，或者把证书装回「个人」证书库。"
                        ));
                        None
                    }
                },
                None => None,
            };
            (token, thumb, err)
        };

        // 证书没准备好就别连了，连上去也只会得到 403，
        // 那样还得去猜是令牌错还是证书错
        if let Some(detail) = cert_err {
            ctx.events.lock().unwrap().push_back(NetEvent::State {
                server,
                state: ConnState::AuthFailed,
                detail,
                first: true,
            });
            ctx.notice.notice();
            continue;
        }

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        ctx.stops.lock().unwrap().push(stop.clone());
        net::spawn_server(
            server,
            token,
            thumb,
            topics,
            make_sink(ctx, generation),
            stop,
        );
    }
}

fn make_sink(ctx: &Rc<Ctx>, generation: u64) -> net::Sink {
    let events = ctx.events.clone();
    let notice = ctx.notice;
    let live = ctx.generation.clone();
    std::sync::Arc::new(move |event: NetEvent| {
        // 上一代连接残留的线程还会吐几条事件，直接丢
        if live.load(std::sync::atomic::Ordering::SeqCst) != generation {
            return;
        }
        events.lock().unwrap().push_back(event);
        notice.notice();
    })
}

fn normalize_server(input: &str) -> String {
    let s = input.trim().trim_end_matches('/');
    if s.is_empty() {
        return "https://ntfy.sh".to_string();
    }
    if s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("ws://")
        || s.starts_with("wss://")
    {
        s.to_string()
    } else {
        format!("https://{s}")
    }
}
