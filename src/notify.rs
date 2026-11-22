//! 系统通知（WinRT Toast）与剪贴板。

/// 必须与注册表里注册的 AppUserModelId 一致，否则通知顶部会显示成别的程序名
pub const APP_ID: &str = "NtfyWin.App";

/// 把 AppID 注册到当前用户，让通知显示 "ntfy" 而不是 powershell.exe
#[cfg(windows)]
pub fn register_app_id() {
    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};

    let sub = wide("Software\\Classes\\AppUserModelId\\NtfyWin.App");
    let name = wide("DisplayName");
    let value = wide("ntfy");

    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            value.as_ptr() as *const core::ffi::c_void,
            (value.len() * 2) as u32,
        );
    }
}

#[cfg(not(windows))]
pub fn register_app_id() {}

/// 弹出系统通知。必须在已初始化 COM 的线程（UI 线程）调用。
pub fn show(topic: &str, body: &str) {
    use tauri_winrt_notification::Toast;

    let body = if body.trim().is_empty() { " " } else { body };
    let _ = Toast::new(APP_ID).title(topic).text1(body).show();
}

// ---------------------------------------------------------------- 剪贴板

#[cfg(windows)]
pub fn copy_to_clipboard(text: &str) -> bool {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    const CF_UNICODETEXT: u32 = 13;

    // 剪贴板可能被别的程序占着，重试几次
    for _ in 0..5 {
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                std::thread::sleep(std::time::Duration::from_millis(30));
                continue;
            }
            let _ = EmptyClipboard();

            let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = utf16.len() * 2;

            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if handle.is_null() {
                let _ = CloseClipboard();
                return false;
            }
            let ptr = GlobalLock(handle);
            if ptr.is_null() {
                let _ = CloseClipboard();
                return false;
            }
            std::ptr::copy_nonoverlapping(utf16.as_ptr() as *const u8, ptr as *mut u8, bytes);
            let _ = GlobalUnlock(handle);
            SetClipboardData(CF_UNICODETEXT, handle);
            let _ = CloseClipboard();
            return true;
        }
    }
    false
}

#[cfg(not(windows))]
pub fn copy_to_clipboard(_text: &str) -> bool {
    false
}

// ---------------------------------------------------------------- 消息框

/// 导入证书这类操作失败时必须说话，否则用户看到的就是「点了没反应」
#[cfg(windows)]
pub fn message(text: &str, title: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK};

    let text = wide(text);
    let title = wide(title);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONWARNING,
        );
    }
}

#[cfg(not(windows))]
pub fn message(_text: &str, _title: &str) {}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
