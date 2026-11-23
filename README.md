# 闪记 FlashNote

这个时代，你的灵感很重要!
基于 Rust GPU 的便签软件，极速，轻量，可自部署同步服务

**[下载闪记（Windows、Linux、Android）](https://github.com/Puring103/FlashNote/releases/latest)**

## 功能

- 随时随处打开便签，快速记录灵感
- 按分组整理，输入关键词查找，也能恢复误删的便签
- 在桌面端录音并转成文字
- 命令行工具，让 agent 操作和获取你的便签
- 支持自部署同步服务，在不同设备之间同步便签

## 应用截图

![Windows 便签列表](assets/screenshots/windows-list.png)

| Windows 便签 | Android 便签列表 | Android 编辑便签 |
| :---: | :---: | :---: |
| <img src="assets/screenshots/windows-note.png" width="260" alt="Windows 便签编辑页"> | <img src="assets/screenshots/android-list.png" width="190" alt="Android 便签列表"> | <img src="assets/screenshots/android-editor.png" width="190" alt="Android 便签编辑页"> |

## 自建同步服务器

在 Linux 服务器上安装 Docker Compose，开放 TCP 80、443。使用域名时，将 DNS 指向服务器。

获取 [`sync-server/` 部署文件](https://github.com/Puring103/FlashNote/tree/main/sync-server)：

```sh
git clone --depth=1 --filter=blob:none --sparse https://github.com/Puring103/FlashNote.git
git -C FlashNote sparse-checkout set sync-server
cd FlashNote/sync-server
cp .env.example .env
```

将 `.env` 中的 `SUOJIAN_ORIGIN` 填为自己的 HTTPS 域名或公网 IP，然后运行 `docker compose up -d --build`。

用 `sudo cat .casdoor/initial-account.json` 查看初始账号和密码，在桌面端「设置 → 账号与同步」或 Android 端「我的」填写服务器地址和凭据。

## 许可证

项目原创代码采用 [MIT 许可证](LICENSE)。第三方组件和素材沿用各自的许可。
