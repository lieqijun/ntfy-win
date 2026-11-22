//! 按指纹从 Windows 证书库取客户端证书。
//!
//! 私钥始终留在证书库里：程序只是拿到一个只读句柄交给 SChannel，
//! 既不导出私钥，也不经手 pfx 密码。所以这里没有「导入」「删除」这类操作。

use schannel::cert_context::{CertContext, HashAlgorithm};
use schannel::cert_store::CertStore;
use schannel::schannel_cred::{Builder as CredBuilder, Direction, SchannelCred};

/// 规范化用户输入的指纹：忽略空格、冒号、连字符，统一大写。
/// 只接受 SHA-1（40 个十六进制字符）—— certmgr 证书详情里显示的「指纹」就是这一串。
pub fn normalize(input: &str) -> Option<String> {
    let hex: String = input
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (hex.len() == 40).then_some(hex)
}

fn decode(hex: &str) -> Option<Vec<u8>> {
    let b = hex.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut i = 0;
    while i + 1 < b.len() {
        let hi = (b[i] as char).to_digit(16)?;
        let lo = (b[i + 1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Some(out)
}

/// 在「当前用户 → 个人」和「本地计算机 → 个人」两个库里按指纹找证书。
/// 两个都找不到就返回 None —— 调用方负责把这句话翻译成人话。
pub fn find(thumb_norm: &str) -> Option<CertContext> {
    let want = decode(thumb_norm)?;
    for machine in [false, true] {
        let store = if machine {
            CertStore::open_local_machine("My")
        } else {
            CertStore::open_current_user("My")
        };
        let Ok(store) = store else {
            continue;
        };
        for cert in store.certs() {
            if let Ok(digest) = cert.fingerprint(HashAlgorithm::sha1()) {
                if digest == want {
                    return Some(cert);
                }
            }
        }
    }
    None
}

/// 有没有可用的私钥。没有私钥的证书做不了客户端认证，得提前拦住。
/// silent(true) 是为了避免某些 CSP 弹 PIN 输入框。
pub fn has_private_key(cert: &CertContext) -> bool {
    let mut opts = cert.private_key();
    opts.silent(true);
    opts.acquire().is_ok()
}

/// 一句人能看懂的回显，用来说明「你填的指纹对应哪张证书」。
pub fn describe(cert: &CertContext) -> String {
    let name = cert.friendly_name().unwrap_or_default().trim().to_string();
    let expired = !cert.is_time_valid().unwrap_or(true);

    let mut parts: Vec<String> = Vec::new();
    if !name.is_empty() {
        parts.push(name);
    }
    if expired {
        parts.push("已过期".into());
    }
    if !has_private_key(cert) {
        parts.push("没有私钥".into());
    }
    if parts.is_empty() {
        "证书可用".into()
    } else {
        parts.join("，")
    }
}

/// 造一个带这张证书的客户端凭据
pub fn client_cred(cert: CertContext) -> std::io::Result<SchannelCred> {
    let mut b = CredBuilder::new();
    b.cert(cert);
    b.acquire(Direction::Outbound)
}

/// 不带客户端证书时的凭据
pub fn anonymous_cred() -> std::io::Result<SchannelCred> {
    CredBuilder::new().acquire(Direction::Outbound)
}
