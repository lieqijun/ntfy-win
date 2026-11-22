//! 主窗口、订阅对话框、设置对话框。

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use native_windows_gui as nwg;

use crate::config::{self, Config};
use crate::net::NetEvent;

/// 逻辑像素 → 物理像素
pub fn scale(v: i32) -> i32 {
    (v as f32 * dpi_factor()).round() as i32
}

pub fn dpi_factor() -> f32 {
    #[cfg(windows)]
    unsafe {
        let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForSystem();
        if dpi == 0 {
            1.0
        } else {
            dpi as f32 / 96.0
        }
    }
    #[cfg(not(windows))]
    1.0
}

#[derive(Clone)]
pub struct Ui {
    pub window: nwg::Window,
    pub list: nwg::ListView,
    pub btn_add: nwg::Button,
    pub btn_settings: nwg::Button,
    pub btn_remove: nwg::Button,

    pub sub_dlg: nwg::Window,
    pub sub_topic_label: nwg::Label,
    pub sub_topic: nwg::TextInput,
    pub sub_server_label: nwg::Label,
    pub sub_server_text: nwg::Label,
    pub sub_link: nwg::Button,
    pub sub_server: nwg::TextInput,
    pub sub_token_label: nwg::Label,
    pub sub_token: nwg::TextInput,
    pub sub_cert_label: nwg::Label,
    pub sub_cert_value: nwg::Label,
    pub sub_import: nwg::Button,
    pub sub_clear: nwg::Button,
    pub sub_pwd_label: nwg::Label,
    pub sub_pwd: nwg::TextInput,
    pub sub_ok: nwg::Button,
    pub sub_cancel: nwg::Button,

    pub set_dlg: nwg::Window,
    pub set_autostart: nwg::CheckBox,
    pub set_copy: nwg::CheckBox,
    pub set_ok: nwg::Button,
    pub set_cancel: nwg::Button,

    /// 订阅对话框里当前选中的证书 id
    pub cert_sel: Rc<RefCell<Option<String>>>,
    /// 订阅对话框是否处于「使用其他服务器」展开态
    pub advanced: Rc<Cell<bool>>,
}

pub struct Ctx {
    pub cfg: Rc<RefCell<Config>>,
    pub events: Arc<Mutex<VecDeque<NetEvent>>>,
    pub notice: nwg::NoticeSender,
    /// Notice 接收端必须一直活着，否则 notice() 不会触发事件
    pub _notice: nwg::Notice,
    pub advanced: Rc<Cell<bool>>,
    pub connected: Rc<Cell<bool>>,
    pub tray: RefCell<Option<crate::tray::Tray>>,
    pub tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Ui {
    pub fn build() -> Result<(Ui, Ctx), nwg::NwgError> {
        let s = scale;

        // ---------------- 主窗口 ----------------
        let mut window = nwg::Window::default();
        nwg::Window::builder()
            .size((s(480), s(348)))
            .position((s(420), s(260)))
            .title("ntfy-win")
            .build(&mut window)?;

        let mut list = nwg::ListView::default();
        nwg::ListView::builder()
            .parent(&window)
            .position((s(10), s(10)))
            .size((s(444), s(268)))
            .list_style(
                nwg::ListViewFlags::REPORT
                    | nwg::ListViewFlags::SINGLE_SEL
                    | nwg::ListViewFlags::SHOWSELALWAYS,
            )
            .ex_flags(nwg::ListViewExFlags::FULLROWSELECT)
            .build(&mut list)?;

        list.insert_column(nwg::ListViewColumn {
            text: "主题".to_string(),
            width: s(140),
            ..Default::default()
        });
        list.insert_column(nwg::ListViewColumn {
            text: "服务器".to_string(),
            width: s(290),
            ..Default::default()
        });

        let mut btn_add = nwg::Button::default();
        nwg::Button::builder()
            .parent(&window)
            .text("订阅新主题...")
            .position((s(150), s(288)))
            .size((s(112), s(28)))
            .build(&mut btn_add)?;

        let mut btn_settings = nwg::Button::default();
        nwg::Button::builder()
            .parent(&window)
            .text("设置...")
            .position((s(268), s(288)))
            .size((s(96), s(28)))
            .build(&mut btn_settings)?;

        let mut btn_remove = nwg::Button::default();
        nwg::Button::builder()
            .parent(&window)
            .text("移除")
            .position((s(370), s(288)))
            .size((s(84), s(28)))
            .build(&mut btn_remove)?;

        // ---------------- 订阅对话框 ----------------
        let mut sub_dlg = nwg::Window::default();
        nwg::Window::builder()
            .parent(&window)
            .size((s(460), s(152)))
            .position((s(520), s(320)))
            .title("订阅新主题")
            .build(&mut sub_dlg)?;

        let mut sub_topic_label = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("主题名称:")
            .position((s(16), s(20)))
            .size((s(76), s(20)))
            .build(&mut sub_topic_label)?;

        let mut sub_topic = nwg::TextInput::default();
        nwg::TextInput::builder()
            .parent(&sub_dlg)
            .position((s(96), s(17)))
            .size((s(344), s(26)))
            .build(&mut sub_topic)?;

        let mut sub_server_label = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("服务器:")
            .position((s(16), s(56)))
            .size((s(76), s(20)))
            .build(&mut sub_server_label)?;

        let mut sub_server_text = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("")
            .position((s(96), s(56)))
            .size((s(344), s(20)))
            .build(&mut sub_server_text)?;

        let mut sub_link = nwg::Button::default();
        nwg::Button::builder()
            .parent(&sub_dlg)
            .text("使用其他服务器...")
            .position((s(96), s(82)))
            .size((s(150), s(26)))
            .build(&mut sub_link)?;

        let mut sub_server = nwg::TextInput::default();
        nwg::TextInput::builder()
            .parent(&sub_dlg)
            .position((s(96), s(82)))
            .size((s(344), s(26)))
            .build(&mut sub_server)?;
        sub_server.set_visible(false);

        let mut sub_token_label = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("访问令牌:")
            .position((s(16), s(118)))
            .size((s(76), s(20)))
            .build(&mut sub_token_label)?;

        let mut sub_token = nwg::TextInput::default();
        nwg::TextInput::builder()
            .parent(&sub_dlg)
            .position((s(96), s(115)))
            .size((s(344), s(26)))
            .build(&mut sub_token)?;

        let mut sub_cert_label = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("客户端证书:")
            .position((s(16), s(154)))
            .size((s(76), s(20)))
            .build(&mut sub_cert_label)?;

        let mut sub_cert_value = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("不使用")
            .position((s(96), s(155)))
            .size((s(146), s(20)))
            .build(&mut sub_cert_value)?;

        let mut sub_import = nwg::Button::default();
        nwg::Button::builder()
            .parent(&sub_dlg)
            .text("导入 pfx...")
            .position((s(240), s(151)))
            .size((s(112), s(26)))
            .build(&mut sub_import)?;

        let mut sub_clear = nwg::Button::default();
        nwg::Button::builder()
            .parent(&sub_dlg)
            .text("不使用")
            .position((s(358), s(151)))
            .size((s(80), s(26)))
            .build(&mut sub_clear)?;

        let mut sub_pwd_label = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("证书密码:")
            .position((s(16), s(190)))
            .size((s(76), s(20)))
            .build(&mut sub_pwd_label)?;

        let mut sub_pwd = nwg::TextInput::default();
        nwg::TextInput::builder()
            .parent(&sub_dlg)
            .position((s(96), s(187)))
            .size((s(220), s(26)))
            .build(&mut sub_pwd)?;

        let mut sub_ok = nwg::Button::default();
        nwg::Button::builder()
            .parent(&sub_dlg)
            .text("确定")
            .position((s(270), s(110)))
            .size((s(80), s(28)))
            .build(&mut sub_ok)?;

        let mut sub_cancel = nwg::Button::default();
        nwg::Button::builder()
            .parent(&sub_dlg)
            .text("取消")
            .position((s(358), s(110)))
            .size((s(80), s(28)))
            .build(&mut sub_cancel)?;

        // ---------------- 设置对话框 ----------------
        let mut set_dlg = nwg::Window::default();
        nwg::Window::builder()
            .parent(&window)
            .size((s(380), s(146)))
            .position((s(560), s(360)))
            .title("设置")
            .build(&mut set_dlg)?;

        let mut set_autostart = nwg::CheckBox::default();
        nwg::CheckBox::builder()
            .parent(&set_dlg)
            .text("开机自启到托盘（登录后静默启动）")
            .position((s(16), s(30)))
            .size((s(340), s(22)))
            .build(&mut set_autostart)?;

        let mut set_copy = nwg::CheckBox::default();
        nwg::CheckBox::builder()
            .parent(&set_dlg)
            .text("通知内容自动复制到剪贴板")
            .position((s(16), s(60)))
            .size((s(340), s(22)))
            .build(&mut set_copy)?;

        let mut set_ok = nwg::Button::default();
        nwg::Button::builder()
            .parent(&set_dlg)
            .text("确定")
            .position((s(198), s(100)))
            .size((s(80), s(28)))
            .build(&mut set_ok)?;

        let mut set_cancel = nwg::Button::default();
        nwg::Button::builder()
            .parent(&set_dlg)
            .text("取消")
            .position((s(284), s(100)))
            .size((s(80), s(28)))
            .build(&mut set_cancel)?;

        let ui = Ui {
            window,
            list,
            btn_add,
            btn_settings,
            btn_remove,
            sub_dlg,
            sub_topic_label,
            sub_topic,
            sub_server_label,
            sub_server_text,
            sub_link,
            sub_server,
            sub_token_label,
            sub_token,
            sub_cert_label,
            sub_cert_value,
            sub_import,
            sub_clear,
            sub_pwd_label,
            sub_pwd,
            sub_ok,
            sub_cancel,
            set_dlg,
            set_autostart,
            set_copy,
            set_ok,
            set_cancel,
            cert_sel: Rc::new(RefCell::new(None)),
            advanced: Rc::new(Cell::new(false)),
        };

        let (notice_sender, notice) = nwg::Notice::new(&window);
        let ctx = Ctx {
            cfg: Rc::new(RefCell::new(Config::load())),
            events: Arc::new(Mutex::new(VecDeque::new())),
            notice: notice_sender,
            _notice: notice,
            connected: Rc::new(Cell::new(false)),
            tray: RefCell::new(None),
            tasks: Mutex::new(Vec::new()),
        };

        Ok((ui, ctx))
    }

    // ------------------------------------------------------------ 列表

    pub fn refresh_list(&self, cfg: &Config) {
        self.list.clear();
        for (i, sub) in cfg.subscriptions.iter().enumerate() {
            let item = nwg::ListViewItem {
                text: sub.topic.clone(),
                subtext: vec![sub.server.clone()],
                ..Default::default()
            };
            let _ = self.list.insert_item(item, i);
        }
    }

    pub fn selected_sub(&self, cfg: &Config) -> Option<usize> {
        self.list.selected_item().filter(|i| *i < cfg.subscriptions.len())
    }

    // ------------------------------------------------------------ 订阅对话框

    pub fn open_subscribe(&self, cfg: &Config) {
        self.sub_topic.set_text("");
        self.sub_pwd.set_text("");

        let last = if cfg.last_server.is_empty() {
            "https://ntfy.sh".to_string()
        } else {
            cfg.last_server.clone()
        };
        self.sub_server_text
            .set_text(&format!("{last}（沿用上次的服务器与令牌）"));
        self.sub_server.set_text(&last);
        self.sub_token.set_text("");

        let cert = cfg.cert_of(&last);
        *self.cert_sel.borrow_mut() = cert.clone();
        self.set_cert_display(&cert);

        self.set_advanced(false);
        self.sub_dlg.set_visible(true);
        self.sub_dlg.set_enabled(true);
        self.sub_topic.set_focus();
        self.window.set_enabled(false);
    }

    pub fn set_cert_display(&self, id: &Option<String>) {
        self.sub_cert_value.set_text(&cert_label(id));
    }

    pub fn close_subscribe(&self) {
        self.sub_dlg.set_visible(false);
        self.window.set_enabled(true);
    }

    pub fn set_advanced(&self, advanced: bool) {
        self.advanced_changed(advanced);
    }

    fn advanced_changed(&self, advanced: bool) {
        self.advanced.set(advanced);
        self.sub_server_text.set_visible(!advanced);
        self.sub_link.set_visible(!advanced);

        self.sub_server.set_visible(advanced);
        self.sub_token_label.set_visible(advanced);
        self.sub_token.set_visible(advanced);
        self.sub_cert_label.set_visible(advanced);
        self.sub_cert_value.set_visible(advanced);
        self.sub_import.set_visible(advanced);
        self.sub_clear.set_visible(advanced);
        self.sub_pwd_label.set_visible(advanced);
        self.sub_pwd.set_visible(advanced);

        let height = if advanced { scale(266) } else { scale(152) };
        self.sub_dlg.set_size(scale(460), height);

        let y = if advanced { scale(226) } else { scale(110) };
        self.sub_ok.set_position(scale(270), y);
        self.sub_cancel.set_position(scale(358), y);
    }

    // ------------------------------------------------------------ 设置对话框

    pub fn open_settings(&self, cfg: &Config) {
        self.set_autostart
            .set_check_state(if cfg.settings.autostart {
                nwg::CheckBoxState::Checked
            } else {
                nwg::CheckBoxState::Unchecked
            });
        self.set_copy.set_check_state(if cfg.settings.auto_copy {
            nwg::CheckBoxState::Checked
        } else {
            nwg::CheckBoxState::Unchecked
        });
        self.set_dlg.set_visible(true);
        self.window.set_enabled(false);
    }

    pub fn close_settings(&self) {
        self.set_dlg.set_visible(false);
        self.window.set_enabled(true);
    }
}

pub fn cert_label(id: &Option<String>) -> String {
    match id {
        None => "不使用".to_string(),
        Some(id) => config::list_certs()
            .into_iter()
            .find(|c| &c.id == id)
            .map(|c| c.label)
            .unwrap_or_else(|| "已导入".to_string()),
    }
}
