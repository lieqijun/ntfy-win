#![windows_subsystem = "windows"]

mod config;
mod net;
mod notify;
mod tray;
mod ui;

use std::rc::Rc;
use std::time::Duration;

use native_windows_gui as nwg;

use config::Sub;
use net::{ConnState, NetEvent};
use ui::{Ctx, Ui};

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

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(_) => return,
    };

    ui.refresh_list(&ctx.cfg.borrow());
    restart_network(&ctx, rt.handle());

    let mut timer = nwg::AnimationTimer::default();
    let _ = nwg::AnimationTimer::builder()
        .parent(&ui.window)
        .interval(Duration::from_millis(200))
        .active(true)
        .build(&mut timer);

    let handlers = bind_handlers(&ui, &ctx, rt.handle().clone());

    // --silent 用于开机自启：只在托盘，不弹窗口
    let silent = std::env::args().any(|a| a == "--silent");
    ui.window.set_visible(!silent);

    nwg::dispatch_thread_events();

    for h in &handlers {
        nwg::unbind_event_handler(h);
    }
    drop(timer);
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

fn bind_handlers(
    ui: &Rc<Ui>,
    ctx: &Rc<Ctx>,
    rt: tokio::runtime::Handle,
) -> Vec<nwg::EventHandler> {
    let mut handlers = Vec::new();
    let targets = [ui.window.handle, ui.sub_dlg.handle, ui.set_dlg.handle];

    for hwnd in targets {
        let ui = ui.clone();
        let ctx = ctx.clone();
        let rt = rt.clone();

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
                        remove_selected(&ui, &ctx, &rt);
                    } else if handle == ui.sub_link.handle {
                        let current = ui.sub_server.text();
                        ui.set_advanced(true);
                        if current.trim().is_empty() {
                            let last = ctx.cfg.borrow().last_server.clone();
                            ui.sub_server.set_text(&last);
                        }
                        ui.sub_server.set_focus();
                    } else if handle == ui.sub_import.handle {
                        import_pfx(&ui);
                    } else if handle == ui.sub_clear.handle {
                        *ui.cert_sel.borrow_mut() = None;
                        ui.set_cert_display(&None);
                    } else if handle == ui.sub_ok.handle {
                        commit_subscribe(&ui, &ctx, &rt);
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

    while let Some(id) = tray::next_menu_id() {
        match id.as_str() {
            "open" => {
                ui.window.set_visible(true);
                ui.window.set_focus();
            }
            "add" => {
                ui.window.set_visible(true);
                let cfg = ctx.cfg.borrow();
                ui.open_subscribe(&cfg);
            }
            "settings" => {
                ui.window.set_visible(true);
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
            NetEvent::State { state, .. } => {
                let connected = state == ConnState::Connected;
                let tip = match state {
                    ConnState::Connected => "ntfy · 已连接",
                    ConnState::Disconnected => "ntfy · 连接断开，正在重连",
                    ConnState::AuthFailed => "ntfy · 认证失败，请检查令牌或证书",
                };
                if ctx.connected.replace(connected) != connected || !connected {
                    if let Some(tray) = ctx.tray.borrow().as_ref() {
                        tray.set_state(connected, tip);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- 订阅

fn commit_subscribe(ui: &Rc<Ui>, ctx: &Rc<Ctx>, rt: &tokio::runtime::Handle) {
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
        normalize_server(&ui.sub_server.text())
    } else {
        let cfg = ctx.cfg.borrow();
        if cfg.last_server.is_empty() {
            "https://ntfy.sh".to_string()
        } else {
            cfg.last_server.clone()
        }
    };

    let cert = if advanced {
        ui.cert_sel.borrow().clone()
    } else {
        ctx.cfg.borrow().cert_of(&server)
    };

    {
        let mut cfg = ctx.cfg.borrow_mut();

        if advanced {
            let token = ui.sub_token.text();
            cfg.set_token(&server, &token);
            cfg.set_cert(&server, cert);
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
    restart_network(ctx, rt);
}

fn remove_selected(ui: &Rc<Ui>, ctx: &Rc<Ctx>, rt: &tokio::runtime::Handle) {
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
    restart_network(ctx, rt);
}

fn import_pfx(ui: &Rc<Ui>) {
    // 顺序很重要：先要密码，再选文件。之前是先要密码后静默 return，
    // 密码框空着点按钮就什么都不会发生 —— 看起来就是「点了没反应」。
    let password = ui.sub_pwd.text();
    if password.is_empty() {
        notify::message(
            "请先在「证书密码」里填好这张 pfx 的密码，再点「导入 pfx...」。\n\
             pfx 的密码是导出证书时设定的那一个。",
            "需要证书密码",
        );
        ui.sub_pwd.set_focus();
        return;
    }

    let mut dialog = nwg::FileDialog::default();
    // nwg 的 filters 不是 Win32 那套 "描述|通配符"，它按 '|' 切成若干段，
    // 每段必须写成 "描述(通配符)" 的形式，少一个括号就直接 build 失败。
    if nwg::FileDialog::builder()
        .title("选择客户端证书")
        .action(nwg::FileDialogAction::Open)
        .filters("PKCS#12 证书(*.pfx;*.p12)|所有文件(*.*)")
        .build(&mut dialog)
        .is_err()
    {
        notify::message("无法打开文件选择窗口。", "导入失败");
        return;
    }

    if !dialog.run(Some(&ui.sub_dlg)) {
        return;
    }
    let Ok(item) = dialog.get_selected_item() else {
        return;
    };
    let path = item.to_string_lossy().to_string();

    let Ok(bytes) = std::fs::read(&path) else {
        notify::message(&format!("读不到这个文件：\n{path}"), "导入失败");
        return;
    };

    // 先用 native-tls 验一遍：密码错或文件坏了直接拒绝，不要存进去
    if native_tls::Identity::from_pkcs12(&bytes, &password).is_err() {
        notify::message(
            "这个 pfx 打不开。可能是密码不对，或者导出时没有勾「包含私钥」。\n\
             请在 certmgr 里重新导出一次，务必勾上导出私钥。",
            "导入失败",
        );
        return;
    }

    let label = std::path::Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("cert")
        .to_string();

    match config::save_cert(&bytes, &password, &label) {
        Some(id) => {
            *ui.cert_sel.borrow_mut() = Some(id.clone());
            ui.set_cert_display(&Some(id));
        }
        None => notify::message("证书保存失败：写不进配置目录。", "导入失败"),
    }
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

fn restart_network(ctx: &Rc<Ctx>, rt: &tokio::runtime::Handle) {
    {
        let mut tasks = ctx.tasks.lock().unwrap();
        for task in tasks.drain(..) {
            task.abort();
        }
    }

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
        let (token, identity) = {
            let cfg = ctx.cfg.borrow();
            let token = cfg.token(&server);
            let identity = cfg.cert_of(&server).and_then(|id| {
                let (pfx, pwd) = config::load_cert(&id)?;
                native_tls::Identity::from_pkcs12(&pfx, &pwd).ok()
            });
            (token, identity)
        };

        let task = net::spawn_server(rt, server, token, topics, identity, make_sink(ctx));
        ctx.tasks.lock().unwrap().push(task);
    }
}

fn make_sink(ctx: &Rc<Ctx>) -> net::Sink {
    let events = ctx.events.clone();
    let notice = ctx.notice;
    std::sync::Arc::new(move |event: NetEvent| {
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
