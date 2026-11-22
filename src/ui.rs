//! 主窗口、订阅对话框、设置对话框。

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use native_windows_gui as nwg;

use crate::config::{self, Config};
use crate::net::NetEvent;

// ---------------------------------------------------------------- DPI

/// 逻辑像素 → 物理像素
pub fn scale(v: i32) -> i32 {
    (v as f32 * dpi_factor()).round() as i32
}

/// 尺寸参数用（只接受正数）
pub fn px(v: i32) -> u32 {
    scale(v).max(1) as u32
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

/// nwg 用的是 winapi 的 HWND，windows-sys 用的是 *mut c_void，这里转一次
#[cfg(windows)]
fn raw_handle(h: &nwg::ControlHandle) -> Option<*mut core::ffi::c_void> {
    h.hwnd().map(|x| x as *mut core::ffi::c_void)
}

#[cfg(windows)]
fn raw_hwnd(win: &nwg::Window) -> Option<*mut core::ffi::c_void> {
    raw_handle(&win.handle)
}

/// nwg 只有在没有 parent 时才把 size 当作客户区大小；带 parent 的对话框
/// size 是外框尺寸，标题栏会把内容挤掉一截。这里先按目标设一次，量出
/// 外框与客户区的差值，再补回去。
pub fn fit_client(win: &nwg::Window, cw: i32, ch: i32) {
    let want_w = cw.max(1) as u32;
    let want_h = ch.max(1) as u32;
    win.set_size(want_w, want_h);

    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect};

        let Some(h) = raw_hwnd(win) else { return };
        let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let mut cr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(h, &mut wr) == 0 || GetClientRect(h, &mut cr) == 0 {
            return;
        }
        let dw = (wr.right - wr.left) - (cr.right - cr.left);
        let dh = (wr.bottom - wr.top) - (cr.bottom - cr.top);
        win.set_size((want_w as i32 + dw).max(1) as u32, (want_h as i32 + dh).max(1) as u32);
    }
}

/// 把 child 摆到 parent 正中（屏幕坐标）
pub fn center_on(parent: &nwg::Window, child: &nwg::Window) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;

        let (Some(ph), Some(ch)) = (raw_hwnd(parent), raw_hwnd(child)) else {
            return;
        };
        let mut pr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let mut cr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(ph, &mut pr) == 0 || GetWindowRect(ch, &mut cr) == 0 {
            return;
        }
        let w = cr.right - cr.left;
        let h = cr.bottom - cr.top;
        let x = pr.left + ((pr.right - pr.left) - w) / 2;
        let y = pr.top + ((pr.bottom - pr.top) - h) / 2;
        child.set_position(x, y);
    }
}

// ---------------------------------------------------------------- 结构

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
    pub connected: Rc<Cell<bool>>,
    pub tray: RefCell<Option<crate::tray::Tray>>,
    pub tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

// 订阅对话框客户区布局（逻辑像素）
const DLG_W: i32 = 470;
const FIELD_X: i32 = 100;
const FIELD_W: i32 = 354;
const LABEL_W: i32 = 80;
const ROW_H: i32 = 26;
const BTN_W: i32 = 80;
const BTN_H: i32 = 28;

const SUB_H_BASIC: i32 = 164;
const SUB_H_ADVANCED: i32 = 236;
const SUB_BTN_Y_BASIC: i32 = 124;
const SUB_BTN_Y_ADVANCED: i32 = 196;

const SET_H: i32 = 140;

impl Ui {
    pub fn build() -> Result<(Ui, Ctx), nwg::NwgError> {
        // 系统默认字体在中文 Windows 上是 9pt 宋体，125% 缩放下糊成一团。
        // 换成雅黑，并且按实际 DPI 取像素高度（size_absolute 传负值），
        // 这样字体和下面按 scale() 算出来的布局是同一套刻度。
        let mut font = nwg::Font::default();
        let mut font_handle: *mut core::ffi::c_void = std::ptr::null_mut();
        if nwg::Font::builder()
            .family("Microsoft YaHei UI")
            .size_absolute(px(12))
            .build(&mut font)
            .is_ok()
        {
            font_handle = font.handle as *mut core::ffi::c_void;
            let _ = nwg::Font::set_global_default(Some(font));
        }

        let s = scale;

        // ---------------- 主窗口（无 parent，size 就是客户区） ----------------
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
            .size((s(460), s(268)))
            .list_style(nwg::ListViewStyle::Detailed)
            .flags(
                nwg::ListViewFlags::VISIBLE
                    | nwg::ListViewFlags::TAB_STOP
                    | nwg::ListViewFlags::SINGLE_SELECTION
                    | nwg::ListViewFlags::ALWAYS_SHOW_SELECTION,
            )
            .ex_flags(nwg::ListViewExFlags::FULL_ROW_SELECT)
            .double_buffer(true)
            .build(&mut list)?;

        // nwg 的 ListView::forced_flags() 里硬编码了 LVS_NOCOLUMNHEADER
        // （源码 controls/list_view.rs:989），建窗口时会 OR 进真实样式，
        // 所以 Detailed 模式下列头永远不出现。必须显式把它去掉。
        list.set_headers_enabled(true);

        list.insert_column(nwg::InsertListViewColumn {
            text: Some("主题".to_string()),
            width: Some(s(150)),
            ..Default::default()
        });
        list.insert_column(nwg::InsertListViewColumn {
            text: Some("服务器".to_string()),
            width: Some(s(290)),
            ..Default::default()
        });

        // nwg 没有给 ListView 留 set_font，只能自己发 WM_SETFONT，
        // 否则列表行和表头还是宋体，跟下面那几个雅黑按钮不是一个风格。
        // 列头是 ListView 内部的 header 子控件，字体不会自动跟着继承，
        // 要单独拿 LVM_GETHEADER 取到它的 HWND 再设一次。
        #[cfg(windows)]
        if !font_handle.is_null() {
            if let Some(h) = raw_handle(&list.handle) {
                unsafe {
                    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;
                    const WM_SETFONT: u32 = 0x0030;
                    const LVM_GETHEADER: u32 = 0x1000 + 31;
                    SendMessageW(h, WM_SETFONT, font_handle as usize, 1);
                    let header = SendMessageW(h, LVM_GETHEADER, 0, 0) as *mut core::ffi::c_void;
                    if !header.is_null() {
                        SendMessageW(header, WM_SETFONT, font_handle as usize, 1);
                    }
                }
            }
        }

        let mut btn_add = nwg::Button::default();
        nwg::Button::builder()
            .parent(&window)
            .text("订阅新主题...")
            .position((s(156), s(288)))
            .size((s(112), s(28)))
            .build(&mut btn_add)?;

        let mut btn_settings = nwg::Button::default();
        nwg::Button::builder()
            .parent(&window)
            .text("设置...")
            .position((s(274), s(288)))
            .size((s(96), s(28)))
            .build(&mut btn_settings)?;

        let mut btn_remove = nwg::Button::default();
        nwg::Button::builder()
            .parent(&window)
            .text("移除")
            .position((s(376), s(288)))
            .size((s(84), s(28)))
            .build(&mut btn_remove)?;

        // ---------------- 订阅对话框 ----------------
        let mut sub_dlg = nwg::Window::default();
        nwg::Window::builder()
            .parent(Some(&window))
            .flags(nwg::WindowFlags::WINDOW)
            .size((s(DLG_W), s(SUB_H_BASIC)))
            .position((s(520), s(320)))
            .title("订阅新主题")
            .build(&mut sub_dlg)?;
        fit_client(&sub_dlg, s(DLG_W), s(SUB_H_BASIC));
        center_on(&window, &sub_dlg);

        let mut sub_topic_label = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("主题名称:")
            .position((s(16), s(17)))
            .size((s(LABEL_W), s(20)))
            .build(&mut sub_topic_label)?;

        let mut sub_topic = nwg::TextInput::default();
        nwg::TextInput::builder()
            .parent(&sub_dlg)
            .position((s(FIELD_X), s(14)))
            .size((s(FIELD_W), s(ROW_H)))
            .build(&mut sub_topic)?;

        let mut sub_server_label = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("服务器:")
            .position((s(16), s(55)))
            .size((s(LABEL_W), s(20)))
            .build(&mut sub_server_label)?;

        let mut sub_server_text = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("")
            .position((s(FIELD_X), s(55)))
            .size((s(FIELD_W), s(20)))
            .build(&mut sub_server_text)?;

        let mut sub_link = nwg::Button::default();
        nwg::Button::builder()
            .parent(&sub_dlg)
            .text("使用其他服务器...")
            .position((s(FIELD_X), s(80)))
            .size((s(150), s(28)))
            .build(&mut sub_link)?;

        let mut sub_server = nwg::TextInput::default();
        nwg::TextInput::builder()
            .parent(&sub_dlg)
            .position((s(FIELD_X), s(50)))
            .size((s(FIELD_W), s(ROW_H)))
            .build(&mut sub_server)?;

        let mut sub_token_label = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("访问令牌:")
            .position((s(16), s(88)))
            .size((s(LABEL_W), s(20)))
            .build(&mut sub_token_label)?;

        let mut sub_token = nwg::TextInput::default();
        nwg::TextInput::builder()
            .parent(&sub_dlg)
            .position((s(FIELD_X), s(84)))
            .size((s(FIELD_W), s(ROW_H)))
            .build(&mut sub_token)?;

        let mut sub_cert_label = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("客户端证书:")
            .position((s(16), s(121)))
            .size((s(LABEL_W), s(20)))
            .build(&mut sub_cert_label)?;

        let mut sub_cert_value = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("不使用")
            .position((s(FIELD_X), s(121)))
            .size((s(140), s(20)))
            .build(&mut sub_cert_value)?;

        let mut sub_import = nwg::Button::default();
        nwg::Button::builder()
            .parent(&sub_dlg)
            .text("导入 pfx...")
            .position((s(246), s(118)))
            .size((s(110), s(26)))
            .build(&mut sub_import)?;

        let mut sub_clear = nwg::Button::default();
        nwg::Button::builder()
            .parent(&sub_dlg)
            .text("清除")
            .position((s(364), s(118)))
            .size((s(90), s(26)))
            .build(&mut sub_clear)?;

        let mut sub_pwd_label = nwg::Label::default();
        nwg::Label::builder()
            .parent(&sub_dlg)
            .text("证书密码:")
            .position((s(16), s(157)))
            .size((s(LABEL_W), s(20)))
            .build(&mut sub_pwd_label)?;

        let mut sub_pwd = nwg::TextInput::default();
        nwg::TextInput::builder()
            .parent(&sub_dlg)
            .position((s(FIELD_X), s(154)))
            .size((s(220), s(ROW_H)))
            .build(&mut sub_pwd)?;
        sub_pwd.set_password_char(Some('*'));

        let mut sub_ok = nwg::Button::default();
        nwg::Button::builder()
            .parent(&sub_dlg)
            .text("确定")
            .position((s(270), s(SUB_BTN_Y_BASIC)))
            .size((s(BTN_W), s(BTN_H)))
            .build(&mut sub_ok)?;

        let mut sub_cancel = nwg::Button::default();
        nwg::Button::builder()
            .parent(&sub_dlg)
            .text("取消")
            .position((s(364), s(SUB_BTN_Y_BASIC)))
            .size((s(BTN_W), s(BTN_H)))
            .build(&mut sub_cancel)?;

        // ---------------- 设置对话框 ----------------
        let mut set_dlg = nwg::Window::default();
        nwg::Window::builder()
            .parent(Some(&window))
            .flags(nwg::WindowFlags::WINDOW)
            .size((s(380), s(SET_H)))
            .position((s(560), s(360)))
            .title("设置")
            .build(&mut set_dlg)?;
        fit_client(&set_dlg, s(380), s(SET_H));
        center_on(&window, &set_dlg);

        let mut set_autostart = nwg::CheckBox::default();
        nwg::CheckBox::builder()
            .parent(&set_dlg)
            .text("开机自启到托盘（登录后静默启动）")
            .position((s(16), s(26)))
            .size((s(340), s(24)))
            .build(&mut set_autostart)?;

        let mut set_copy = nwg::CheckBox::default();
        nwg::CheckBox::builder()
            .parent(&set_dlg)
            .text("通知内容自动复制到剪贴板")
            .position((s(16), s(58)))
            .size((s(340), s(24)))
            .build(&mut set_copy)?;

        let mut set_ok = nwg::Button::default();
        nwg::Button::builder()
            .parent(&set_dlg)
            .text("确定")
            .position((s(190), s(96)))
            .size((s(BTN_W), s(BTN_H)))
            .build(&mut set_ok)?;

        let mut set_cancel = nwg::Button::default();
        nwg::Button::builder()
            .parent(&set_dlg)
            .text("取消")
            .position((s(284), s(96)))
            .size((s(BTN_W), s(BTN_H)))
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

        // 对话框初始为非高级态；没选中行时「移除」应当是灰的
        ui.advanced_changed(false);
        ui.btn_remove.set_enabled(false);

        let notice = nwg::Notice::create(&ui.window)?;
        let ctx = Ctx {
            cfg: Rc::new(RefCell::new(Config::load())),
            events: Arc::new(Mutex::new(VecDeque::new())),
            notice: notice.sender(),
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
        for sub in &cfg.subscriptions {
            self.list.insert_items_row(
                None,
                &[
                    nwg::InsertListViewItem {
                        text: Some(sub.topic.clone()),
                        ..Default::default()
                    },
                    nwg::InsertListViewItem {
                        text: Some(sub.server.clone()),
                        ..Default::default()
                    },
                ],
            );
        }
        self.sync_remove_enabled(cfg.subscriptions.len());
    }

    /// 没有选中行时把「移除」置灰
    pub fn sync_remove_enabled(&self, count: usize) {
        let on = matches!(self.list.selected_item(), Some(i) if i < count);
        self.btn_remove.set_enabled(on);
    }

    pub fn selected_sub(&self, cfg: &Config) -> Option<usize> {
        self.list.selected_item().filter(|i| *i < cfg.subscriptions.len())
    }

    // ------------------------------------------------------------ 订阅对话框

    pub fn open_subscribe(&self, cfg: &Config) {
        self.sub_topic.set_text("");
        self.sub_pwd.set_text("");
        self.sub_token.set_text("");

        let last = if cfg.last_server.is_empty() {
            "https://ntfy.sh".to_string()
        } else {
            cfg.last_server.clone()
        };
        self.sub_server_text.set_text(&last);
        self.sub_server.set_text(&last);

        let cert = cfg.cert_of(&last);
        *self.cert_sel.borrow_mut() = cert.clone();
        self.set_cert_display(&cert);

        self.set_advanced(false);
        center_on(&self.window, &self.sub_dlg);
        self.sub_dlg.set_visible(true);
        self.sub_topic.set_focus();
        self.window.set_enabled(false);
    }

    pub fn set_cert_display(&self, id: &Option<String>) {
        self.sub_cert_value.set_text(&cert_label(id));
    }

    pub fn close_subscribe(&self) {
        self.sub_dlg.set_visible(false);
        self.window.set_enabled(true);
        self.bring_to_front();
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

        let h = if advanced { SUB_H_ADVANCED } else { SUB_H_BASIC };
        fit_client(&self.sub_dlg, px(DLG_W) as i32, px(h) as i32);

        let y = if advanced { SUB_BTN_Y_ADVANCED } else { SUB_BTN_Y_BASIC };
        self.sub_ok.set_position(scale(270), scale(y));
        self.sub_cancel.set_position(scale(364), scale(y));
    }

    // ------------------------------------------------------------ 设置对话框

    pub fn open_settings(&self, cfg: &Config) {
        self.set_autostart.set_check_state(if cfg.settings.autostart {
            nwg::CheckBoxState::Checked
        } else {
            nwg::CheckBoxState::Unchecked
        });
        self.set_copy.set_check_state(if cfg.settings.auto_copy {
            nwg::CheckBoxState::Checked
        } else {
            nwg::CheckBoxState::Unchecked
        });
        center_on(&self.window, &self.set_dlg);
        self.set_dlg.set_visible(true);
        self.window.set_enabled(false);
    }

    pub fn close_settings(&self) {
        self.set_dlg.set_visible(false);
        self.window.set_enabled(true);
        self.bring_to_front();
    }

    /// 关闭子对话框后把主窗口拉回前台，否则它会掉到别的窗口后面
    pub fn bring_to_front(&self) {
        #[cfg(windows)]
        unsafe {
            if let Some(h) = raw_hwnd(&self.window) {
                windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(h);
            }
        }
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
