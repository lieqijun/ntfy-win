# ntfy-win

Windows 托盘端 ntfy 推送客户端。只收推送，不发送。

## 行为

- 托盘常驻，左键打开窗口、右键菜单。窗口关闭只是隐藏到托盘。
- 每台服务器一条 WebSocket 长连接（端点是 `/<topic>/ws`，多主题逗号分隔），
  一条连接订阅该服务器下所有主题。
- 断线退避重连：`1s → 2s → 4s → 8s → 16s → 30s → 60s` 封顶，无次数上限，带 ±20% 抖动。
  401 / 403 / 404 不重试，直接停手，托盘图标转红。
- 90 秒没有收到任何数据判定连接已死（ntfy 会定时发 ping）。
- 首次连接只收新消息；重连时带 `since=<最后一条消息时间+1>` 补漏。
- 收到消息弹出系统通知，标题是主题名，正文是消息内容。
- 通知内容可自动写入剪贴板。

## 配置

配置文件放在 exe 同目录的 `config.json`；该目录不可写时回退到 `%APPDATA%\ntfy-win\`。

- Access Token 用 Windows DPAPI 加密后存储，只有当前 Windows 用户能解开。
- 客户端证书**不落盘、不导出**：订阅时填证书指纹（SHA-1，40 位十六进制），
  程序到「当前用户 → 个人」或「本地计算机 → 个人」证书库里按指纹找到它，
  把证书句柄交给 SChannel。私钥始终留在证书库里。
- DPAPI 防的是 token 文件被拷走，**不防同一账户下运行的其他程序**。

## 构建

```
cargo build --release
```

产物在 `target/release/ntfy-win.exe`，单文件，静态链接 CRT。

云编译：推送后 GitHub Actions 自动跑，版本号取 `Cargo.toml` 的 `version`，
产物同时上传 artifact 和 Release（tag 形如 `v2.0.0`）。

## 图标

`app.ico` 由 `tools/make-icon.js` 生成：

```
node tools/make-icon.js .
```

托盘图标在运行时用同一套几何画出来（`src/tray.rs`），所以不依赖外部资源文件。
蓝色 `#1f7ae0` 表示已连接，红色 `#e8383d` 表示断开或认证失败。
