fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("app.ico");
        res.set_manifest_file("app.manifest");
        res.set("FileDescription", "ntfy-win");
        res.set("ProductName", "ntfy-win");
        res.set("OriginalFilename", "ntfy-win.exe");
        if let Err(e) = res.compile() {
            // 资源编译失败不应阻断整个构建，但要在日志里喊出来
            println!("cargo:warning=winresource failed: {e}");
        }
    }
}
