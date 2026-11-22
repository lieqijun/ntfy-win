//! 配置、DPAPI 加解密、开机自启。
//! 客户端证书不进配置文件，只存一个指纹，私钥始终留在系统证书库里。

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- 数据结构

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub autostart: bool,
    #[serde(default = "yes")]
    pub auto_copy: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self { autostart: false, auto_copy: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ServerCfg {
    /// DPAPI 加密后的 token（base64），空串表示没有 token
    #[serde(default)]
    pub token_enc: String,
    /// 客户端证书的 SHA-1 指纹（40 位大写十六进制），None 表示不使用客户端证书
    #[serde(default)]
    pub cert_thumb: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sub {
    pub topic: String,
    pub server: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub servers: BTreeMap<String, ServerCfg>,
    #[serde(default)]
    pub last_server: String,
    #[serde(default)]
    pub subscriptions: Vec<Sub>,
    #[serde(default)]
    pub settings: Settings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            servers: BTreeMap::new(),
            last_server: String::new(),
            subscriptions: Vec::new(),
            settings: Settings::default(),
        }
    }
}

// ---------------------------------------------------------------- 路径

/// 优先 exe 同目录；不可写则回退到 %APPDATA%\ntfy-win
pub fn data_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if is_writable(dir) {
                return dir.to_path_buf();
            }
        }
    }
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    let dir = PathBuf::from(base).join("ntfy-win");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn is_writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(".write_probe");
    match fs::write(&probe, b"x") {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

// ---------------------------------------------------------------- 配置读写

impl Config {
    pub fn load() -> Self {
        let text = fs::read_to_string(config_path()).unwrap_or_default();
        if text.trim().is_empty() {
            return Config::default();
        }
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        fs::write(config_path(), text)
    }

    pub fn set_token(&mut self, server: &str, token: &str) {
        let entry = self.servers.entry(server.to_string()).or_default();
        entry.token_enc = if token.is_empty() {
            String::new()
        } else {
            match protect(token.as_bytes()) {
                Some(enc) => b64_encode(&enc),
                None => String::new(),
            }
        };
    }

    pub fn token(&self, server: &str) -> String {
        let Some(cfg) = self.servers.get(server) else {
            return String::new();
        };
        if cfg.token_enc.is_empty() {
            return String::new();
        }
        match b64_decode(&cfg.token_enc).and_then(|raw| unprotect(&raw)) {
            Some(bytes) => String::from_utf8(bytes).unwrap_or_default(),
            None => String::new(),
        }
    }

    pub fn set_cert_thumb(&mut self, server: &str, thumb: Option<String>) {
        let entry = self.servers.entry(server.to_string()).or_default();
        entry.cert_thumb = thumb;
    }

    pub fn cert_thumb_of(&self, server: &str) -> Option<String> {
        self.servers.get(server).and_then(|s| s.cert_thumb.clone())
    }
}

// ---------------------------------------------------------------- base64

pub fn b64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

// ---------------------------------------------------------------- DPAPI

#[cfg(windows)]
pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};

    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            &mut output,
        );
        if ok == 0 || output.pbData.is_null() {
            return None;
        }
        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let out = slice.to_vec();
        let _ = LocalFree(output.pbData as *mut core::ffi::c_void);
        Some(out)
    }
}

#[cfg(windows)]
pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            &mut output,
        );
        if ok == 0 || output.pbData.is_null() {
            return None;
        }
        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let out = slice.to_vec();
        let _ = LocalFree(output.pbData as *mut core::ffi::c_void);
        Some(out)
    }
}

#[cfg(not(windows))]
pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
    Some(data.to_vec())
}

#[cfg(not(windows))]
pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    Some(data.to_vec())
}

// ---------------------------------------------------------------- 开机自启

#[cfg(windows)]
pub fn set_autostart(enable: bool) {
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
    };

    let sub = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let name = wide("NtfyWin");

    unsafe {
        if enable {
            let exe = std::env::current_exe().unwrap_or_default();
            let cmd = wide(&format!("\"{}\" --silent", exe.display()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                cmd.as_ptr() as *const core::ffi::c_void,
                (cmd.len() * 2) as u32,
            );
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, sub.as_ptr(), name.as_ptr());
        }
    }
}

#[cfg(not(windows))]
pub fn set_autostart(_enable: bool) {}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
