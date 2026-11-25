# gpui-note-editor

原生 GPUI 的结构化便签编辑器试验版。Rust crate，无 WebView。闪记通过 path dependency 使用它，也可以单独引用。

## 移植来源与边界

参考 ProseMirror 的文档模型、Selection、Transaction 和 Step 设计。`src/mapping.rs` 将 prosemirror-transform 的 `_map` 算法转写为 Rust，当前仅支持单次替换映射；来源 commit、原文及 MIT 许可证保存在 `reference/`。

其余文档、历史、输入规则和 GPUI 视图是面向便签的新实现，不是完整 ProseMirror 移植，不兼容 ProseMirror JSON/插件，也不能据此宣称继承了其交互成熟度。没有复制 Zed 的 GPL 编辑器代码。

## 分层

- `document.rs`：段落、标题、列表、待办、引用、代码块、行内格式；可见正文的 UTF-8 位置；编辑事务及位置映射；选区；撤销重做；输入法组合事务；Markdown 导入导出；结构化 JSON。
- `mapping.rs`：替换前后位置映射，包含边界关联方向。
- `view.rs`：原生 GPUI 布局与绘制、换行、点击、拖选、键盘导航、剪贴板、UTF-16 输入法接口及光标滚动。

正文模型中的文字不包含加粗等 Markdown 定界符。输入规则将语法转换成结构，保存时再生成 Markdown。待办框是独立的绘制和点击区域，不进入可编辑文本位置；采用矢量绘制，不依赖字体里的复选框字符。

## 引用

```toml
[dependencies]
gpui-note-editor = { path = "path/to/gpui-note-editor" }
```

```rust,ignore
gpui_note_editor::init(cx); // 应用初始化时调用一次，注册编辑区专用 Tab 快捷键
let editor = cx.new(|cx| gpui_note_editor::RichEditor::new(markdown, cx));
// 在布局中直接 child(editor.clone())。
// 外层滚动容器 track_scroll(&editor.read(cx).scroll_handle())。
// 订阅 Changed 后，通过 value() 读取用于保存的 Markdown。
```

只有文档核心、不需要 GPUI：

```toml
[dependencies]
gpui-note-editor = { path = "path/to/gpui-note-editor", default-features = false }
```

```rust
use gpui_note_editor::document::{Document, EditorState, MarkKind};
let mut editor = EditorState::new(Document::from_markdown("你好"));
editor.select(0, 6).unwrap();
editor.toggle_mark(MarkKind::Bold).unwrap();
assert_eq!(editor.document.to_markdown(), "**你好**");
```

## 验证与独立示例

从闪记项目目录执行：

```powershell
cargo test -p gpui-note-editor --no-default-features
cargo run -p gpui-note-editor --example editor
```

第二条命令会打开独立示例窗口。本次开发只编译示例，没有运行它。

## 当前支持与限制

支持基础文字编辑、常用行内格式、标题、列表和待办输入规则、段落拆分合并、Unicode 字素删除、格式化片段插入、事务回滚、撤销重做及组合输入提交。导入导出支持常用 Markdown；空白会规范化，不承诺 Markdown 源码逐字还原。内嵌 Markdown 图片引用保留，目前正文显示替代文字。

`Attachment` 是保留的节点数据接口，尚未完成原子节点选择、宿主渲染器和拖动协议。闪记现有的图片、文件和录音卡片继续由应用负责，不算已统一进入这个编辑文档。

复杂表格、完整多层嵌套结构、富文本剪贴板、辅助功能、双向文字、拖选自动滚动、输入历史的连续打字分组等仍未完整实现。当前按编辑操作保存历史，输入法组合提交合并为一次历史操作。实际中文候选框、鼠标拖选和窗口缩放尚未进行桌面实测。

MIT 许可证；转写来源的版权声明见 `reference/LICENSE`。

数字列表支持输入 `1. ` / `1) ` 转换，回车自动续号。Tab / Shift+Tab 在列表中调整层级，在普通正文中增加/移除四个空格；编辑区拦截 Tab，避免触发父级焦点切换。
