# ntfy-win

Windows 托盘端 ntfy 推送客户端。只收推送，不发送。

## 行为

- 托盘常驻，左键菜单、右键菜单。窗口关闭只是隐藏到托盘。
- 每台服务器一条 WebSocket 长连接（`/ws`），连接上订阅该服务器下所有主题。
- 断线退避重连：`1s → 2s → 4s → 8s → 16s → 30s → 60s` 封顶，无次数上限，带 ±20% 抖动。
  401 / 403 不重试，直接停手，托盘图标转红。
- 90 秒没有收到任何数据判定连接已死（ntfy 每 30 秒发 keepalive）。
- 重连时带上次收到的消息 id 补漏（首次连接回溯 12 小时）。
- 收到消息弹出系统通知，标题是主题名，正文是消息内容。
- 通知内容可自动写入剪贴板。

## 配置

配置文件放在 exe 同目录的 `config.json`；该目录不可写时回退到 `%APPDATA%\ntfy-win\`。

- Access Token 用 Windows DPAPI 加密后存储，只有当前 Windows 用户能解开。
- 客户端证书导入 `.pfx` / `.p12`，整体（含密码）用 DPAPI 加密后存到 `certs/`。
- DPAPI 防的是文件被拷走，**不防同一账户下运行的其他程序**。

## 构建

```
cargo build --release
```

产物在 `target/release/ntfy-win.exe`，单文件，静态链接 CRT。

云编译：推送后 GitHub Actions 自动跑，版本号取北京时间 `yyyyMMdd-HHmmss`，
产物同时上传 artifact 和 Release。

## 图标

`app.ico` 由 `tools/make-icon.js` 生成：

```
node tools/make-icon.js .
```

托盘图标在运行时用同一套几何画出来（`src/tray.rs`），所以不依赖外部资源文件。
蓝色 `#1f7ae0` 表示已连接，红色 `#e8383d` 表示断开或认证失败。
