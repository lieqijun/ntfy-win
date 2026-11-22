//! 托盘图标与右键菜单。

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub struct Tray {
    icon: TrayIcon,
    pub autostart: CheckMenuItem,
    pub auto_copy: CheckMenuItem,
}

impl Tray {
    pub fn new(autostart: bool, auto_copy: bool) -> Option<Self> {
        let menu = Menu::new();

        let open = MenuItem::with_id("open", "打开主窗口", true, None);
        let add = MenuItem::with_id("add", "订阅新主题...", true, None);
        let sep1 = PredefinedMenuItem::separator();
        let autostart_item =
            CheckMenuItem::with_id("autostart", "开机自启", true, autostart, None);
        let copy_item =
            CheckMenuItem::with_id("auto_copy", "自动复制通知内容", true, auto_copy, None);
        let sep2 = PredefinedMenuItem::separator();
        let settings = MenuItem::with_id("settings", "设置...", true, None);
        let sep3 = PredefinedMenuItem::separator();
        let quit = MenuItem::with_id("quit", "退出", true, None);

        let _ = menu.append_items(&[
            &open,
            &add,
            &sep1,
            &autostart_item,
            &copy_item,
            &sep2,
            &settings,
            &sep3,
            &quit,
        ]);

        let icon = Icon::from_rgba(bell_rgba(32, COLOR_OK), 32, 32).ok()?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("ntfy · 连接中")
            .with_icon(icon)
            .with_menu_on_left_click(false)
            .build()
            .ok()?;

        Some(Self { icon: tray, autostart: autostart_item, auto_copy: copy_item })
    }

    /// 连接状态变化时换图标颜色，并把原因写进 tooltip
    pub fn set_state(&self, connected: bool, tip: &str) {
        let color = if connected { COLOR_OK } else { COLOR_ERR };
        if let Ok(icon) = Icon::from_rgba(bell_rgba(32, color), 32, 32) {
            let _ = self.icon.set_icon(Some(icon));
        }
        let _ = self.icon.set_tooltip(Some(tip));
    }

    pub fn set_checked(&self, autostart: bool, auto_copy: bool) {
        self.autostart.set_checked(autostart);
        self.auto_copy.set_checked(auto_copy);
    }
}

/// 取出菜单事件里的 id。窗口被隐藏时也照常收到。
pub fn next_menu_id() -> Option<String> {
    match MenuEvent::receiver().try_recv() {
        Ok(event) => Some(event.id.0.clone()),
        Err(_) => None,
    }
}

pub const COLOR_OK: [u8; 3] = [0x1f, 0x7a, 0xe0];
pub const COLOR_ERR: [u8; 3] = [0xe8, 0x38, 0x3d];

// ---------------------------------------------------------------- 图标绘制

/// 与 tools/make-icon.js 相同的几何，保证托盘图标和 exe 图标是同一个形状
fn in_bell(x: f32, y: f32) -> bool {
    if (0.03..=0.12).contains(&y) && (x - 0.5).abs() < 0.05 {
        return true;
    }
    if y > 0.10 && y <= 0.685 {
        let t = (y - 0.10) / 0.585;
        let half = 0.085 + 0.335 * t.powf(0.85);
        if (x - 0.5).abs() < half {
            return true;
        }
    }
    if y > 0.685 && y <= 0.765 && (x - 0.5).abs() < 0.425 {
        return true;
    }
    let dx = x - 0.5;
    let dy = y - 0.865;
    dx * dx + dy * dy < 0.0075
}

fn bell_rgba(size: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut buf = vec![0u8; (size * size * 4) as usize];
    const SS: u32 = 4;

    for py in 0..size {
        for px in 0..size {
            let mut hits = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    if in_bell(x, y) {
                        hits += 1;
                    }
                }
            }
            let alpha = (hits as f32 / (SS * SS) as f32 * 255.0) as u8;
            let i = ((py * size + px) * 4) as usize;
            buf[i] = rgb[0];
            buf[i + 1] = rgb[1];
            buf[i + 2] = rgb[2];
            buf[i + 3] = alpha;
        }
    }
    buf
}
