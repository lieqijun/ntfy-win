//! 配置、DPAPI 加解密、证书文件、开机自启。

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
    /// certs/ 下的文件名（不含扩展名），None 表示不使用客户端证书
    #[serde(default)]
    pub cert_id: Option<String>,
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

pub fn certs_dir() -> PathBuf {
    let d = data_dir().join("certs");
    let _ = fs::create_dir_all(&d);
    d
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

    pub fn set_cert(&mut self, server: &str, cert_id: Option<String>) {
        let entry = self.servers.entry(server.to_string()).or_default();
        entry.cert_id = cert_id;
    }

    pub fn cert_of(&self, server: &str) -> Option<String> {
        self.servers.get(server).and_then(|s| s.cert_id.clone())
    }

    /// 把引用这张证书的服务器全部改成「不使用证书」，返回改动了几处。
    /// 删证书文件之前必须先调这个，否则服务器会指向一个不存在的文件。
    pub fn drop_cert_refs(&mut self, id: &str) -> usize {
        let mut hit = 0;
        for server in self.servers.values_mut() {
            if server.cert_id.as_deref() == Some(id) {
                server.cert_id = None;
                hit += 1;
            }
        }
        hit
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

// ---------------------------------------------------------------- 证书文件

#[derive(Serialize, Deserialize)]
struct CertBlob {
    pfx: String,
    password: String,
}

#[derive(Clone, Debug)]
pub struct CertInfo {
    pub id: String,
    pub label: String,
}

/// 导入 pfx：整体 DPAPI 加密后写 certs/<id>.bin
pub fn save_cert(pfx: &[u8], password: &str, label: &str) -> Option<String> {
    let id = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let blob = CertBlob { pfx: b64_encode(pfx), password: password.to_string() };
    let json = serde_json::to_vec(&blob).ok()?;
    let enc = protect(&json)?;
    let _ = fs::write(certs_dir().join(format!("{id}.bin")), enc);
    // 记录一个可读名字，仅用于下拉显示
    let _ = fs::write(certs_dir().join(format!("{id}.name")), label.as_bytes());
    Some(id)
}

pub fn load_cert(id: &str) -> Option<(Vec<u8>, String)> {
    let raw = fs::read(certs_dir().join(format!("{id}.bin"))).ok()?;
    let json = unprotect(&raw)?;
    let blob: CertBlob = serde_json::from_slice(&json).ok()?;
    Some((b64_decode(&blob.pfx)?, blob.password))
}

/// 删掉 certs/<id>.bin 和同名的 .name。返回 .bin 是否真的删掉了。
pub fn delete_cert(id: &str) -> bool {
    let dir = certs_dir();
    let ok = fs::remove_file(dir.join(format!("{id}.bin"))).is_ok();
    let _ = fs::remove_file(dir.join(format!("{id}.name")));
    ok
}

pub fn list_certs() -> Vec<CertInfo> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(certs_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("bin") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let id = stem.to_string();
        let label = fs::read_to_string(certs_dir().join(format!("{id}.name")))
            .map(|s| s.trim().to_string())
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| id.clone());
        out.push(CertInfo { id, label });
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
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
