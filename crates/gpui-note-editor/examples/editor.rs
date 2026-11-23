use gpui::{prelude::*, *};
use gpui_note_editor::RichEditor;
struct Example {
    editor: Entity<RichEditor>,
}
impl Render for Example {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0xf7f3ea))
            .text_color(rgb(0x262723))
            .font_family("Microsoft YaHei UI")
            .child(
                div()
                    .id("document-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.editor.read(cx).scroll_handle())
                    .p_6()
                    .child(self.editor.clone()),
            )
    }
}
fn main() {
    Application::new().run(|cx|{
        gpui_note_editor::init(cx);
        let bounds=Bounds::centered(None,size(px(520.),px(620.)),cx);
        let _=cx.open_window(WindowOptions{window_bounds:Some(WindowBounds::Windowed(bounds)),..Default::default()},|w,cx|{
            let editor=cx.new(|cx|RichEditor::new("# 原位编辑示例\n\n直接点击正文输入，试试 **加粗**、_斜体_ 与 `代码`。\n\n- [ ] 输入中文和 emoji\n- [ ] 选择、删除、撤销\n- [x] 同一份正文实时排版".into(),cx));
            editor.update(cx,|e,cx|e.focus(w,cx));cx.new(|_|Example{editor})
        });
    });
}
