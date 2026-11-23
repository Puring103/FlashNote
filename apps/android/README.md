# 闪记 Android

Kotlin 2.1 / Jetpack Compose 原生界面，JNI 调用 `crates/flashnote-android`，后者复用 `suojian-core`。
最低 Android 8.0（API 26），目标 API 35。默认构建 ARM64 真机版本；可构建 x86_64 模拟器版本。

## 构建与安装

需要 JDK 17、Rust、Android SDK Platform 35 / Build Tools 35，以及 NDK 28.0.13004108。可用 `-NdkVersion` 指定本机安装的其他版本。
Gradle Wrapper 已提交，下载缓存和 SDK 位于本机，不提交仓库。

在仓库根目录执行：

```powershell
# ANDROID_HOME 指向自己的 SDK；本机默认使用 runtime/android-tools/sdk
.\scripts\build-android.ps1
# 同时包含 ARM64 和 x86_64
.\scripts\build-android.ps1 -Abi all
# 安装到已连接且允许 USB 调试的手机
adb install -r apps/android/app/build/outputs/apk/debug/app-debug.apk
```

脚本先交叉编译 Rust，再按 `-Abi` 选择的架构打包 APK，任何步骤失败都会停止。每种选定架构都需要对应的 JNI 库。
默认输出调试签名包 `app/build/outputs/apk/debug/app-debug.apk`。`-Release -Abi all` 构建含 ARM64 和 x86_64 的正式 APK，CI 使用固定的发布证书签名并上传到 GitHub Releases。调试包和正式包的签名不同，迁移前请先同步或备份本地资料。
`-DebugRust` 可加快开发构建；默认 Rust 使用 release 优化。
Android Studio 打开本目录前，先运行一次构建脚本准备 JNI 库。

## 已实现

- 单列便签卡片、标题与正文搜索、分组筛选与新建分组。
- Markdown 全屏编辑，输入停顿 1.2 秒自动保存，手动保存与离开确认。
- 旋转与系统保存状态时保留编辑草稿；保存使用版本校验，拒绝覆盖较新的版本。
- 回收站与恢复，图片/文件导入（32 MB 上限），通过系统应用打开附件与已有音频。
- 应用内账号密码登录、手动加密同步、退出登录，冲突保留本地/云端/两份。
- SQLite、便签/分组/附件格式、加密、同步合并和版本规则使用同一份 Rust 实现。

当前界面是手机版第一版：正文编辑为 Markdown 源码，尚未实现桌面原位富文本、手机录音与 whisper 转写、ZIP 导入导出界面、历史版本界面、定时后台同步和远程图片自动下载。桌面已有的素材占位与媒体结构会保留；已有附件可以打开。搜索当前为标题/正文包含匹配。共享库已保留完整桌面能力，可继续按安卓交互接入。

## 数据与线程

`filesDir/vault` 存储数据库与原始附件，使用原有 v3 格式。`vault-key.dpapi`、`sync-session.dpapi` 文件名为兼容逻辑保留，Android 上实际内容由 Android Keystore AES-256-GCM 保护，并非 Windows DPAPI。没有明文密钥回退。
本地正文与附件仍为明文，上传使用原有 XChaCha20-Poly1305。登录与同步沿用现有服务端信任模型。

数据库和网络操作串行运行在 ViewModel 专属工作线程。JNI 重用该线程的资料库连接。打开应用或手动同步时刷新列表，保存、删除、恢复和添加附件时只返回受影响的便签。JNI 边界返回结构化错误，界面保留草稿。关闭系统备份，避免恢复只有数据库却没有 Keystore 密钥的无效资料库。卸载会删除本地资料与设备密钥，卸载前需同步。

## 验证范围

已经通过 ARM64 原生库交叉编译、Compose 编译、APK 打包与 Android Lint，以及 Rust 工作区回归测试。没有连接安卓真机，未完成触屏、Keystore 实机与真实账号多端联调验收。现有部署账号集成测试仍需显式测试账号。

## 纸笺索引界面（2026-11-17）

采用已确认的单列纸笺方案：暖白底、墨绿、细边框与分组索引色标，底部便签/分组/我的导航。便签菜单收纳附件与回收站操作，图片使用异步采样缩略图。分组单独成页，编辑采用无边框输入和底部快捷工具条。

编辑正文目前仍是 Markdown 源码输入，尚未实现设计图中的完整原位富文本排版。界面编译、APK 打包与 Lint 已通过。


2026-11-17 第二轮视觉修正：颜色完全对齐当前桌面端常量；复用细线图标，缩小标题栏、搜索与分组控件，修正单列卡片密度和新建按钮遮挡。顶部只有云状态图标，“我的”提供登录入口。基础 Markdown 排版采用保留源文本及光标映射的视觉变换；完整桌面结构化编辑器尚未移植。图文、待办、分组三页已通过独立演示资料库做真机截图检查，见 `design/android-paper-index/verification`。
