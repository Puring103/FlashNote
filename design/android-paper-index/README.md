# Android 纸笺索引 · 单列版

用户选定第一版视觉，首页采用单列布局。

- 暖白纸面、墨绿主色、细边框、轻阴影。
- 全宽单列便签卡片，高度随内容适配；分组使用卡片左上角色标。
- 图片在列表采用小缩略图；正文页可使用大图。
- 首页、编辑页和分组页保持一致的字体、图标、颜色与间距。
- 实现时为悬浮新建按钮预留空间，避免遮挡卡片内容。

图片：single-column-v1.png。通过内置 image_gen 编辑第一版方案生成；此图为设计参考，不代表功能已实现。

生成提示词摘要：保留原参考图的暖白、墨绿、索引色标、细边框和导航；仅将首页改为全宽单列、自然高度卡片，显示文字、待办、图片缩略图和语音；编辑页与分组页保持原风格；三个完整 Android 页面并列展示。

## 第二轮实际实现与验收（2026-11-17）

颜色以当前桌面源码 `apps/desktop/src/ui.rs` 为准：纸面 #F7F3EA、卡片 #FBF8F1、正文 #262723、墨绿 #245356、边框 #D9D5CB、次要文字 #68645E。手机布局沿用已选单列概念图；图标复用桌面 Lucide SVG 的路径，并补充同线宽的云、用户与文件夹。

- 字号：品牌 26sp，卡片标题 18sp，预览正文 14sp，元信息 11sp，编辑正文 16sp。
- 搜索框最小高 40dp；分组标签视觉高度约 32dp，点击区域至少 44dp；卡片圆角 8dp、细边框和弱阴影。
- 索引签为单色轮廓，不使用渐变和厚阴影；分组颜色来自共享数据库，新建分组可选颜色。
- 顶部只保留云状态：灰色未登录，墨绿已登录，成功同步后显示勾，同步中进度环，失败红色。账号在底部“我的”。
- 新建按钮有独立的底部保留区域，不再叠在滚动卡片上；分组页回收站固定在列表下方。
- 正文增加基础 Markdown 视觉变换，隐藏格式标记并显示标题、粗体、斜体和待办；保持原文及光标位置映射。此实现不是完整的桌面结构化编辑器，图片暂在正文输入区域下方展示。

真实设备截图（1080×2400，系统字体缩放 1.0）：
- verification/home-v2.png
- verification/groups-v2.png
- verification/editor-v2.png

截图使用调试专用 `design-preview-v2` 资料库，通过同一 JNI/SQLite 路径生成示例；不写入正式 vault，不连接账号，不上传。可用 `adb shell am start -n com.noteharbor.suojian/.MainActivity --ez design_preview true --es preview_screen notes` 打开；切换前 force-stop。preview_screen 也可为 groups/editor。仅 BuildConfig.DEBUG 接受该入口。

验证：Android assembleDebug、lintDebug（0 errors）、三项 Markdown 映射单元测试和两项 Rust Android 接口测试通过。实际检查了非空首页、分组与编辑页，安装后切回正式账号登录页。
