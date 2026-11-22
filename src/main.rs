#![windows_subsystem = "windows"]

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

fn main() {
    if !nwg::init() {
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
    let ctx = Rc::new(ctx);

    {
        let cfg = ctx.cfg.borrow();
        *ctx.tray.borrow_mut() = tray::Tray::new(cfg.settings.autostart, cfg.settings.auto_copy);
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

    let mut timer = nwg::Timer::default();
    let _ = nwg::Timer::builder()
        .parent(&ui.window)
        .interval(250)
        .active(true)
        .build(&mut timer);

    let _handlers = bind_handlers(&ui, ctx.clone(), rt.handle().clone());

    let silent = std::env::args().any(|a| a == "--silent");
    if !silent && ctx.cfg.borrow().subscriptions.is_empty() {
        ui.window.set_visible(true);
    }

    nwg::dispatch_thread_events();
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

fn bind_handlers(ui: &Ui, ctx: Rc<Ctx>, rt: tokio::runtime::Handle) -> Vec<nwg::EventHandler> {
    let mut handlers = Vec::new();
    let targets = [ui.window.handle, ui.sub_dlg.handle, ui.set_dlg.handle];

    for hwnd in targets {
        let ui = ui.clone();
        let ctx = ctx.clone();
        let rt = rt.clone();

        let handler = nwg::full_bind_event_handler(&hwnd, move |evt, _data, handle| {
            match evt {
                nwg::Event::OnButtonClick => {
                    if handle == ui.btn_add.handle {
                        ui.open_subscribe(&ctx.cfg.borrow());
                    } else if handle == ui.btn_settings.handle {
                        ui.open_settings(&ctx.cfg.borrow());
                    } else if handle == ui.btn_remove.handle {
                        remove_selected(&ui, &ctx, &rt);
                    } else if handle == ui.sub_link.handle {
                        let current = ui.sub_server.text();
                        ui.set_advanced(true);
                        ui.sub_server.set_text(&current);
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
                    if handle == ui.window.handle {
                        // 关闭只是隐藏，进程继续守在托盘
                        ui.window.set_visible(false);
                    } else if handle == ui.sub_dlg.handle {
                        ui.close_subscribe();
                    } else if handle == ui.set_dlg.handle {
                        ui.close_settings();
                    }
                }
                nwg::Event::OnTimerTick => {
                    poll_tray(&ui, &ctx);
                    drain_events(&ctx);
                }
                nwg::Event::OnNotice => {
                    drain_events(&ctx);
                }
                _ => {}
            }
        });

        if let Some(h) = handler {
            handlers.push(h);
        }
    }

    handlers
}

// ---------------------------------------------------------------- 托盘

fn poll_tray(ui: &Ui, ctx: &Ctx) {
    while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
        let Some(tray) = ctx.tray.borrow().as_ref() else {
            continue;
        };
        let id = event.id();

        if id == tray.open.id() {
            ui.window.set_visible(true);
            ui.window.set_focus();
        } else if id == tray.add.id() {
            ui.window.set_visible(true);
            ui.open_subscribe(&ctx.cfg.borrow());
        } else if id == tray.settings.id() {
            ui.window.set_visible(true);
            ui.open_settings(&ctx.cfg.borrow());
        } else if id == tray.autostart.id() {
            let value = !ctx.cfg.borrow().settings.autostart;
            {
                let mut cfg = ctx.cfg.borrow_mut();
                cfg.settings.autostart = value;
                let _ = cfg.save();
            }
            config::set_autostart(value);
            tray.autostart.set_checked(value);
        } else if id == tray.auto_copy.id() {
            let value = !ctx.cfg.borrow().settings.auto_copy;
            {
                let mut cfg = ctx.cfg.borrow_mut();
                cfg.settings.auto_copy = value;
                let _ = cfg.save();
            }
            tray.auto_copy.set_checked(value);
        } else if id == tray.quit.id() {
            std::process::exit(0);
        }
    }
}

// ---------------------------------------------------------------- 网络事件

fn drain_events(ctx: &Ctx) {
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
                ctx.connected.set(connected);
                let tip = match state {
                    ConnState::Connected => "ntfy · 已连接",
                    ConnState::Disconnected => "ntfy · 连接断开，正在重连",
                    ConnState::AuthFailed => "ntfy · 认证失败，请检查令牌或证书",
                };
                if let Some(tray) = ctx.tray.borrow().as_ref() {
                    tray.set_state(connected, tip);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- 订阅

fn commit_subscribe(ui: &Ui, ctx: &Ctx, rt: &tokio::runtime::Handle) {
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
        }

        let exists = cfg
            .subscriptions
            .iter()
            .any(|s| s.topic == topic && s.server == server);
        if !exists {
            cfg.subscriptions
                .push(Sub { topic: topic.clone(), server: server.clone() });
        }
        cfg.last_server = server.clone();
        let _ = cfg.save();
    }

    ui.close_subscribe();
    ui.refresh_list(&ctx.cfg.borrow());
    restart_network(ctx, rt);
}

fn remove_selected(ui: &Ui, ctx: &Ctx, rt: &tokio::runtime::Handle) {
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

fn import_pfx(ui: &Ui) {
    let password = ui.sub_pwd.text();
    if password.is_empty() {
        ui.sub_pwd.set_focus();
        return;
    }

    let dialog = match nwg::FileDialog::builder()
        .title("选择客户端证书（.pfx / .p12）")
        .action(nwg::FileDialogAction::Open)
        .filters("PKCS#12(*.pfx;*.p12)|*.pfx;*.p12")
        .build()
    {
        Ok(d) => d,
        Err(_) => return,
    };

    if !dialog.run(Some(&ui.sub_dlg)) {
        return;
    }
    let Some(path) = dialog.get_selected_item() else {
        return;
    };

    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };

    // 先用 native-tls 验一遍：密码错或文件坏了直接拒绝，不要存进去
    if native_tls::Identity::from_pkcs12(&bytes, &password).is_err() {
        return;
    }

    let label = std::path::Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("cert")
        .to_string();

    if let Some(id) = config::save_cert(&bytes, &password, &label) {
        *ui.cert_sel.borrow_mut() = Some(id.clone());
        ui.set_cert_display(&Some(id));
    }
}

fn commit_settings(ui: &Ui, ctx: &Ctx) {
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
        tray.autostart.set_checked(autostart);
        tray.auto_copy.set_checked(auto_copy);
    }
    ui.close_settings();
}

// ---------------------------------------------------------------- 连接调度

fn restart_network(ctx: &Ctx, rt: &tokio::runtime::Handle) {
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

fn make_sink(ctx: &Ctx) -> net::Sink {
    let events = ctx.events.clone();
    let notice = ctx.notice.clone();
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
