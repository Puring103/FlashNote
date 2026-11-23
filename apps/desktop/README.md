# Windows 桌面端

`src/` 保存 GPUI 界面、录音、托盘、快捷键和 Windows 平台适配。
根目录 Cargo 包仍名为 `suojian`，通过显式入口指向本目录，原有运行和打包命令不变：

```powershell
cargo run
cargo build --release --bins
.\scripts\package.ps1
```

`src/lib.rs` 保留 `suojian::model` 等兼容导出；业务代码位于 `crates/flashnote-core`。
