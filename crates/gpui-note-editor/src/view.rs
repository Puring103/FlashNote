//! Native GPUI view over the structured document model.
use crate::document::{BlockKind, Document, EditorState, MarkKind};
use gpui::{prelude::*, *};
use std::{cell::RefCell, ops::Range, rc::Rc};
use unicode_segmentation::UnicodeSegmentation;
actions!(note_editor, [Indent, Outdent]);

/// Install editor-specific bindings before the component root can handle Tab.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", Indent, Some("RichEditor")),
        KeyBinding::new("shift-tab", Outdent, Some("RichEditor")),
    ]);
}
#[derive(Clone, Copy, Debug, Default)]
struct Mark {
    bold: bool,
    italic: bool,
    code: bool,
    link: bool,
    strike: bool,
}
#[derive(Clone, Debug)]
struct Glyph {
    display: Range<usize>,
    source: Range<usize>,
    mark: Mark,
}
#[derive(Clone, Debug, Default)]
struct Projection {
    text: String,
    glyphs: Vec<Glyph>,
    starts: Vec<usize>,
}
impl Projection {
    fn new(doc: &Document) -> Self {
        let mut p = Self::default();
        for (i, b) in doc.blocks.iter().enumerate() {
            if i > 0 {
                p.text.push('\n');
            }
            p.starts.push(p.text.len());
            for s in &b.content {
                for c in s.text.chars() {
                    let a = p.text.len();
                    p.text.push(c);
                    let range = a..p.text.len();
                    p.glyphs.push(Glyph {
                        display: range.clone(),
                        source: range,
                        mark: Mark {
                            bold: s.marks.bold || matches!(b.kind, BlockKind::Heading(_)),
                            italic: s.marks.italic,
                            code: s.marks.code || matches!(b.kind, BlockKind::Code { .. }),
                            link: s.marks.link.is_some(),
                            strike: s.marks.strike,
                        },
                    });
                }
            }
        }
        p
    }
    fn block_at(&self, pos: usize) -> usize {
        self.starts.partition_point(|s| *s <= pos).saturating_sub(1)
    }
}
#[derive(Clone)]
struct Row {
    text_len: usize,
    line: WrappedLine,
    start: usize,
    y: Pixels,
    height: Pixels,
    x: Pixels,
    block_index: usize,
}
#[derive(Default, Clone)]
struct Layout {
    rows: Vec<Row>,
    bounds: Bounds<Pixels>,
}
impl Layout {
    fn position(&self, offset: usize) -> (Point<Pixels>, Pixels) {
        let row = self
            .rows
            .iter()
            .find(|r| offset <= r.start + r.text_len)
            .or(self.rows.last());
        if let Some(r) = row {
            let p = r
                .line
                .position_for_index(offset.saturating_sub(r.start).min(r.line.len()), r.height)
                .unwrap_or_default();
            (self.bounds.origin + point(r.x + p.x, r.y + p.y), r.height)
        } else {
            (self.bounds.origin, px(26.))
        }
    }
    fn index(&self, position: Point<Pixels>) -> usize {
        let p = position - self.bounds.origin;
        if p.y < px(0.) {
            return 0;
        }
        if let Some(last) = self.rows.last() {
            if p.y >= last.y + last.line.size(last.height).height {
                return last.start + last.text_len;
            }
        }
        let row = self
            .rows
            .iter()
            .find(|r| p.y < r.y + r.line.size(r.height).height)
            .or(self.rows.last());
        row.map(|r| {
            r.start
                + r.line
                    .closest_index_for_position(point(p.x - r.x, (p.y - r.y).max(px(0.))), r.height)
                    .unwrap_or_else(|i| i)
        })
        .unwrap_or(0)
    }
}

pub type MediaCopier = Rc<dyn Fn(&str) -> Option<ClipboardItem>>;
pub type MediaRenderer = Rc<dyn Fn(&str, &mut Window, &mut App) -> AnyElement>;
pub struct RichEditor {
    media_renderer: Option<MediaRenderer>,
    media_copier: Option<MediaCopier>,
    clipboard_html_writer: Option<Rc<dyn Fn(&str)>>,
    host_paste: bool,
    state: EditorState,
    source: String,
    projection: Projection,
    selection: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    focus_handle: FocusHandle,
    selecting: bool,
    layout: Layout,
    scroll: ScrollHandle,
    reveal: bool,
    media_bounds: Vec<Bounds<Pixels>>,
    media_width: Option<Pixels>,
}
pub struct Changed;
impl EventEmitter<Changed> for RichEditor {}
impl RichEditor {
    pub fn new(source: String, cx: &mut Context<Self>) -> Self {
        let state = EditorState::new(Document::from_markdown(&source));
        let projection = Projection::new(&state.document);
        Self {
            media_renderer: None,
            media_copier: None,
            clipboard_html_writer: None,
            host_paste: false,
            state,
            source,
            projection,
            selection: 0..0,
            reversed: false,
            marked: None,
            focus_handle: cx.focus_handle(),
            selecting: false,
            layout: Layout::default(),
            scroll: ScrollHandle::new(),
            reveal: false,
            media_bounds: vec![],
            media_width: None,
        }
    }
    pub fn set_host_paste(&mut self, enabled: bool) {
        self.host_paste = enabled;
    }
    pub fn set_media_width(&mut self, width: Pixels) {
        self.media_width = Some(width.max(px(40.)));
    }
    pub fn set_clipboard_html_writer(&mut self, writer: Rc<dyn Fn(&str)>) {
        self.clipboard_html_writer = Some(writer);
    }
    pub fn set_media_copier(&mut self, copier: MediaCopier) {
        self.media_copier = Some(copier);
    }
    pub fn set_media_renderer(&mut self, renderer: MediaRenderer) {
        self.media_renderer = Some(renderer);
    }
    pub fn insert_attachment(
        &mut self,
        id: String,
        kind: String,
        label: String,
        cx: &mut Context<Self>,
    ) {
        self.commit(cx);
        self.sync_selection();
        if self.state.insert_attachment(id, kind, label).is_ok() {
            self.sync(true, cx);
        }
    }
    pub fn remove_attachment(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(index) = self
            .state
            .document
            .blocks
            .iter()
            .position(|b| matches!(&b.kind, BlockKind::Attachment {id: bid,..} if bid == id))
        {
            let start = self.state.document.block_start(index);
            let end = start + self.state.document.blocks[index].len();
            if self.state.replace(start..end, "", false).is_ok() {
                self.sync(true, cx);
            }
        }
    }
    pub fn is_focused(&self, w: &Window) -> bool {
        self.focus_handle.is_focused(w)
    }
    pub fn value(&self) -> &str {
        &self.source
    }
    pub fn document(&self) -> &Document {
        &self.state.document
    }
    pub fn scroll_handle(&self) -> ScrollHandle {
        self.scroll.clone()
    }
    pub fn focus(&self, w: &mut Window, _: &mut Context<Self>) {
        w.focus(&self.focus_handle);
    }
    pub fn is_composing(&self) -> bool {
        self.marked.is_some()
    }
    pub fn sync_external(&mut self, source: String, cx: &mut Context<Self>) -> bool {
        let doc = Document::from_markdown(&source);
        if !self.state.sync_external(doc) {
            return false;
        }
        self.source = source;
        self.sync(false, cx);
        self.reveal = false;
        true
    }
    pub fn set_value(&mut self, text: String, _: &mut Window, cx: &mut Context<Self>) {
        self.state = EditorState::new(Document::from_markdown(&text));
        self.source = text;
        self.marked = None;
        self.sync(false, cx);
    }
    /// Inserts Markdown supplied by application commands (e.g. a link dialog).
    pub fn replace(&mut self, text: String, w: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        self.sync_selection();
        if self
            .state
            .replace_markdown(self.selection.clone(), &text)
            .is_ok()
        {
            self.sync(true, cx);
        }
        self.focus(w, cx);
    }
    fn sync_selection(&mut self) {
        let (a, h) = if self.reversed {
            (self.selection.end, self.selection.start)
        } else {
            (self.selection.start, self.selection.end)
        };
        let _ = self.state.select(a, h);
    }
    fn sync(&mut self, changed: bool, cx: &mut Context<Self>) {
        self.projection = Projection::new(&self.state.document);
        self.selection = self.state.selection.range();
        self.reversed = self.state.selection.head < self.state.selection.anchor;
        self.marked = self.state.composition_range();
        self.reveal = true;
        if changed {
            self.source = self.state.document.to_markdown();
            cx.emit(Changed);
        }
        cx.notify();
    }
    fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }
    fn move_to(&mut self, offset: usize, shift: bool, cx: &mut Context<Self>) {
        let offset = offset.min(self.projection.text.len());
        let anchor = if shift {
            if self.reversed {
                self.selection.end
            } else {
                self.selection.start
            }
        } else {
            offset
        };
        let _ = self.state.select(anchor, offset);
        self.selection = anchor.min(offset)..anchor.max(offset);
        self.reversed = offset < anchor;
        self.reveal = true;
        cx.notify();
    }
    fn hit(&self, p: Point<Pixels>) -> usize {
        self.layout.index(p).min(self.projection.text.len())
    }
    fn commit(&mut self, cx: &mut Context<Self>) {
        if self.marked.take().is_some() {
            self.state.commit_composition();
            self.sync(true, cx);
        }
    }
    fn delete_visible(&mut self, back: bool, cx: &mut Context<Self>) {
        self.commit(cx);
        if self.state.delete(back).is_ok() {
            self.sync(true, cx);
        }
    }
    fn navigate(&mut self, direction: &str, shift: bool, control: bool, cx: &mut Context<Self>) {
        let d = self.cursor();
        let text = &self.projection.text;
        let next = match direction {
            "left" if !shift && !self.selection.is_empty() => {
                self.move_to(self.selection.start, false, cx);
                return;
            }
            "right" if !shift && !self.selection.is_empty() => {
                self.move_to(self.selection.end, false, cx);
                return;
            }
            "left" if control => text[..d]
                .unicode_word_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0),
            "right" if control => text[d..]
                .unicode_word_indices()
                .next()
                .map(|(i, s)| d + i + s.len())
                .unwrap_or(text.len()),
            "left" => text
                .grapheme_indices(true)
                .rev()
                .find(|(i, _)| *i < d)
                .map(|(i, _)| i)
                .unwrap_or(0),
            "right" => text
                .grapheme_indices(true)
                .find(|(i, _)| *i > d)
                .map(|(i, _)| i)
                .unwrap_or(text.len()),
            "home" if control => 0,
            "end" if control => text.len(),
            "home" => text[..d].rfind('\n').map(|i| i + 1).unwrap_or(0),
            "end" => text[d..].find('\n').map(|i| d + i).unwrap_or(text.len()),
            "up" | "down" => {
                let (p, h) = self.layout.position(d);
                self.layout
                    .index(p + point(px(0.), if direction == "up" { -h / 2. } else { h * 1.5 }))
            }
            _ => d,
        };
        let (block, _) = self.state.document.locate(next);
        let next = if !shift
            && matches!(
                self.state.document.blocks[block].kind,
                BlockKind::Attachment { .. }
            ) {
            if direction == "left" || direction == "up" {
                self.state.document.block_start(block).saturating_sub(1)
            } else if block + 1 < self.state.document.blocks.len() {
                self.state.document.block_start(block + 1)
            } else {
                next
            }
        } else {
            next
        };
        self.move_to(next, shift, cx);
    }
    fn history(&mut self, redo: bool, cx: &mut Context<Self>) {
        self.commit(cx);
        if if redo {
            self.state.redo()
        } else {
            self.state.undo()
        } {
            self.sync(true, cx);
        }
    }
    pub fn format(&mut self, kind: &str, w: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        let result = match kind {
            "bold" => self.state.toggle_mark(MarkKind::Bold),
            "italic" => self.state.toggle_mark(MarkKind::Italic),
            "strike" => self.state.toggle_mark(MarkKind::Strike),
            "code" => {
                let (index, _) = self.state.document.locate(self.state.selection.head);
                if matches!(
                    self.state.document.blocks[index].kind,
                    BlockKind::Code { .. }
                ) {
                    self.state.exit_code()
                } else {
                    self.state.set_block(BlockKind::Code {
                        language: String::new(),
                    })
                }
            }
            "inline-code" => self.state.toggle_mark(MarkKind::Code),
            "heading" => self.state.set_block(BlockKind::Heading(2)),
            "paragraph" => self.state.set_block(BlockKind::Paragraph),
            "quote" => self.state.set_block(BlockKind::Quote),
            "ordered" => self.state.set_block(BlockKind::Ordered {
                number: 1,
                depth: 0,
            }),
            "todo" => self.state.set_block(BlockKind::Task {
                checked: false,
                depth: 0,
            }),
            _ => self.state.set_block(BlockKind::Bullet { depth: 0 }),
        };
        if result.is_ok() {
            self.sync(true, cx);
        }
        self.focus(w, cx);
    }
    fn enter(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.enter().is_ok() {
            self.sync(true, cx);
        }
    }
    fn indent(&mut self, outdent: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        if self.state.indent(outdent).is_ok() {
            self.sync(true, cx);
        }
        cx.stop_propagation();
        window.prevent_default();
    }
    fn key(&mut self, e: &KeyDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return;
        }
        let key = e.keystroke.key.as_str();
        let ctrl = e.keystroke.modifiers.control;
        let shift = e.keystroke.modifiers.shift;
        match (ctrl, key) {
            (true, "a") => {
                let n = self.projection.text.len();
                self.move_to(0, false, cx);
                self.move_to(n, true, cx);
            }
            (true, "c") | (true, "x") => {
                if !self.selection.is_empty() {
                    let markdown = self
                        .state
                        .document
                        .selected_markdown(self.selection.clone());
                    let (block, local) = self.state.document.locate(self.selection.start);
                    let selected_block = &self.state.document.blocks[block];
                    let media = if local == 0 && self.selection.len() == selected_block.len() {
                        if let BlockKind::Attachment { id, .. } = &selected_block.kind {
                            self.media_copier.as_ref().and_then(|copy| copy(id))
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let copied_image = media.is_some();
                    cx.write_to_clipboard(media.unwrap_or_else(|| {
                        ClipboardItem::new_string_with_metadata(
                            self.projection.text[self.selection.clone()].to_string(),
                            format!("suojian-markdown-v1\n{markdown}"),
                        )
                    }));
                    if !copied_image {
                        if let Some(writer) = &self.clipboard_html_writer {
                            let mut html = String::new();
                            pulldown_cmark::html::push_html(
                                &mut html,
                                pulldown_cmark::Parser::new_ext(
                                    &markdown,
                                    pulldown_cmark::Options::all(),
                                ),
                            );
                            writer(&html);
                        }
                    }
                    if key == "x" {
                        self.delete_visible(false, cx);
                    }
                }
            }
            (true, "v") => {
                if self.host_paste {
                    return;
                }
                if cx.read_from_clipboard().is_some_and(|item| {
                    item.entries()
                        .iter()
                        .any(|e| matches!(e, ClipboardEntry::Image(_)))
                }) {
                    return; // Let the host import image bytes even when the clipboard also contains text.
                }
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    self.replace_text_in_range(None, &text.replace("\r\n", "\n"), w, cx);
                } else {
                    return;
                }
            }
            (true, "z") => self.history(shift, cx),
            (true, "y") => self.history(true, cx),
            (true, "b") => self.format("bold", w, cx),
            (true, "i") => self.format("italic", w, cx),
            (_, "left" | "right" | "up" | "down" | "home" | "end") => {
                self.navigate(key, shift, ctrl, cx)
            }
            (false, "backspace") => self.delete_visible(true, cx),
            (false, "delete") => self.delete_visible(false, cx),
            (true, "enter") => {
                if self.state.exit_code().is_ok() {
                    self.sync(true, cx);
                }
            }
            (false, "enter") => self.enter(w, cx),
            (false, "tab") => self.indent(shift, w, cx),
            _ => return,
        }
        cx.stop_propagation();
        w.prevent_default();
    }
    fn mouse_down(&mut self, e: &MouseDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        if let Some(index) = self
            .media_bounds
            .iter()
            .position(|b| b.contains(&e.position))
        {
            if let Some((block_index, block)) = self
                .state
                .document
                .blocks
                .iter()
                .enumerate()
                .filter(|(_, b)| matches!(b.kind, BlockKind::Attachment { .. }))
                .nth(index)
            {
                let start = self.state.document.block_start(block_index);
                let end = start + block.len();
                self.focus(w, cx);
                self.move_to(start, false, cx);
                self.move_to(end, true, cx);
            }
            return;
        }
        self.focus(w, cx);
        let hit_row =
            self.layout.rows.iter().find(|r| {
                e.position.y < self.layout.bounds.top() + r.y + r.line.size(r.height).height
            });
        if let Some(row) = hit_row {
            if e.position.x < self.layout.bounds.left() + row.x
                && matches!(
                    self.state.document.blocks[row.block_index].kind,
                    BlockKind::Task { .. }
                )
            {
                let index = row.block_index;
                let _ = self.state.toggle_task(index);
                self.sync(true, cx);
                cx.stop_propagation();
                return;
            }
        }
        self.selecting = true;
        let offset = self.hit(e.position);
        self.move_to(offset, e.modifiers.shift, cx);
        if e.click_count >= 2 {
            let range = crate::document::click_selection(
                &self.projection.text,
                offset,
                e.click_count as usize,
            );
            self.move_to(range.start, false, cx);
            self.move_to(range.end, true, cx);
            self.selecting = false;
        }
        cx.stop_propagation();
    }
}
fn from_utf16(text: &str, offset: usize) -> usize {
    let mut count = 0;
    for (i, c) in text.char_indices() {
        if count >= offset {
            return i;
        }
        count += c.len_utf16();
    }
    text.len()
}
fn to_utf16(text: &str, offset: usize) -> usize {
    text[..offset].encode_utf16().count()
}
impl EntityInputHandler for RichEditor {
    fn text_for_range(
        &mut self,
        r: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text = &self.projection.text;
        let r = from_utf16(text, r.start)..from_utf16(text, r.end);
        *actual = Some(to_utf16(text, r.start)..to_utf16(text, r.end));
        Some(text[r].into())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: to_utf16(&self.projection.text, self.selection.start)
                ..to_utf16(&self.projection.text, self.selection.end),
            reversed: self.reversed,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| {
            to_utf16(&self.projection.text, r.start)..to_utf16(&self.projection.text, r.end)
        })
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
    }
    fn replace_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = r
            .map(|r| {
                from_utf16(&self.projection.text, r.start)..from_utf16(&self.projection.text, r.end)
            })
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        if self.marked.is_some() {
            if self.state.begin_composition(range, text).is_ok() {
                self.state.commit_composition();
                self.marked = None;
                self.sync(true, cx);
            }
        } else if self.state.replace(range, text, true).is_ok() {
            self.sync(true, cx);
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = r
            .map(|r| {
                from_utf16(&self.projection.text, r.start)..from_utf16(&self.projection.text, r.end)
            })
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        if let Ok(marked) = self.state.begin_composition(range, text) {
            if let Some(r) = selected {
                self.state.selection = crate::document::Selection {
                    anchor: marked.start + from_utf16(text, r.start),
                    head: marked.start + from_utf16(text, r.end),
                };
            }
            self.sync(false, cx);
        }
    }
    fn bounds_for_range(
        &mut self,
        r: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let a = from_utf16(&self.projection.text, r.start);
        let b = from_utf16(&self.projection.text, r.end);
        let (p, h) = self.layout.position(a);
        let (q, _) = self.layout.position(b);
        Some(Bounds::new(
            p,
            size(
                if p.y == q.y {
                    (q.x - p.x).max(px(2.))
                } else {
                    px(2.)
                },
                h,
            ),
        ))
    }
    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(to_utf16(&self.projection.text, self.hit(p)))
    }
}
impl Focusable for RichEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
impl Render for RichEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .flex_shrink_0()
            .key_context("RichEditor")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|v, _: &Indent, w, cx| v.indent(false, w, cx)))
            .on_action(cx.listener(|v, _: &Outdent, w, cx| v.indent(true, w, cx)))
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(|s, e: &MouseMoveEvent, _, cx| {
                if s.selecting {
                    let p = s.hit(e.position);
                    s.move_to(p, true, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|s, _, _, _| s.selecting = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|s, _, _, _| s.selecting = false),
            )
            .child(EditorElement {
                editor: cx.entity(),
            })
    }
}
struct EditorElement {
    editor: Entity<RichEditor>,
}
struct MediaLayout {
    x: Pixels,
    block: usize,
    element: AnyElement,
    y: Pixels,
    height: Pixels,
}
struct EditorLayout {
    text: Rc<RefCell<Layout>>,
    media: Rc<RefCell<Vec<MediaLayout>>>,
}
impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for EditorElement {
    type RequestLayoutState = EditorLayout;
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        w: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let state = Rc::new(RefCell::new(Layout::default()));
        let measured = state.clone();
        let input = self.editor.read(cx);
        let p = input.projection.clone();
        let empty = input.projection.text.is_empty() && input.state.document.blocks.len() == 1;
        let blocks = input.state.document.blocks.clone();
        let marked = input.marked.clone();
        let renderer = input.media_renderer.clone();
        let previous_width = input.layout.bounds.size.width;
        let host_width = if previous_width > px(0.) {
            Some(previous_width)
        } else {
            input.media_width
        };
        let mut children = vec![];
        if let Some(renderer) = renderer {
            for (index, block) in blocks.iter().enumerate() {
                if let BlockKind::Attachment { id, depth, .. } = &block.kind {
                    children.push(MediaLayout {
                        x: px(*depth as f32 * 16.),
                        block: index,
                        element: renderer(id, w, cx),
                        y: px(0.),
                        height: px(0.),
                    });
                }
            }
        }
        // Measure hosted nodes before entering Taffy's measure callback. Nested
        // compute_layout inside that callback would re-enter the layout engine.
        let media_width = host_width.unwrap_or((w.viewport_size().width - px(44.)).max(px(30.)));
        for slot in &mut children {
            slot.height = slot
                .element
                .layout_as_root(
                    size(
                        AvailableSpace::Definite((media_width - slot.x).max(px(40.))),
                        AvailableSpace::MaxContent,
                    ),
                    w,
                    cx,
                )
                .height;
        }
        let media = Rc::new(RefCell::new(children));
        let measure_media = media.clone();
        let base = w.text_style();
        let layout = w.request_measured_layout(Style::default(), move |known, available, w, _| {
            let width = known
                .width
                .or(match available.width {
                    AvailableSpace::Definite(w) => Some(w),
                    _ => None,
                })
                .unwrap_or(px(360.))
                .max(px(30.));
            let text = if empty { "记录…" } else { &p.text };
            let mut rows = vec![];
            let mut start = 0;
            let mut y = px(2.);
            for (line_index, line) in text.split('\n').enumerate() {
                let glyphs: Vec<_> = p
                    .glyphs
                    .iter()
                    .filter(|g| g.display.start >= start && g.display.start < start + line.len())
                    .collect();
                let block_index = p.block_at(start);
                let kind = &blocks[block_index].kind;
                if let Some(slot) = measure_media
                    .borrow_mut()
                    .iter_mut()
                    .find(|m| m.block == block_index)
                {
                    slot.y = y;
                    y += slot.height + px(8.);
                    start += line.len() + 1;
                    continue;
                }
                let is_code = matches!(kind, BlockKind::Code { .. });
                let first_code_line = is_code && start == p.starts[block_index];
                if first_code_line {
                    y += px(10.);
                }
                let syntax = if is_code {
                    code_colors(line)
                } else {
                    Vec::new()
                };
                let heading = if let BlockKind::Heading(h) = kind {
                    *h
                } else {
                    0
                };
                let x = px(match kind {
                    BlockKind::Task { depth, .. }
                    | BlockKind::Bullet { depth }
                    | BlockKind::Ordered { depth, .. } => 26. + 16. * *depth as f32,
                    BlockKind::Quote => 14.,
                    BlockKind::Code { .. } => 12.,
                    _ => 0.,
                });
                let font_size = px(match heading {
                    1 => 24.,
                    2 => 21.,
                    3 => 19.,
                    _ => 16.,
                });
                let font_size = if is_code { px(14.) } else { font_size };
                let height = if is_code {
                    px(21.)
                } else {
                    font_size + px(10.)
                };
                let mut runs = vec![];
                for g in glyphs {
                    let mut font = base.font();
                    if g.mark.bold && !is_code {
                        font.weight = FontWeight::BOLD;
                    }
                    if g.mark.italic && !is_code {
                        font.style = FontStyle::Italic;
                    }
                    if g.mark.code {
                        font.family = "Consolas".into();
                    }
                    let composing = marked
                        .as_ref()
                        .is_some_and(|r| r.start < g.source.end && r.end > g.source.start);
                    runs.push(TextRun {
                        len: g.display.len(),
                        font,
                        color: rgb(if is_code {
                            *syntax.get(g.display.start - start).unwrap_or(&0x303a40)
                        } else if g.mark.link {
                            0x245356
                        } else {
                            0x262723
                        })
                        .into(),
                        background_color: if g.mark.code && !is_code {
                            Some(rgb(0xece8df).into())
                        } else {
                            None
                        },
                        underline: if g.mark.link || composing {
                            Some(UnderlineStyle {
                                thickness: px(1.),
                                color: None,
                                wavy: false,
                            })
                        } else {
                            None
                        },
                        strikethrough: if g.mark.strike {
                            Some(StrikethroughStyle {
                                thickness: px(1.),
                                color: None,
                            })
                        } else {
                            None
                        },
                    });
                }
                if runs.is_empty() {
                    runs.push(TextRun {
                        len: line.len().max(1),
                        font: if is_code {
                            font("Consolas")
                        } else {
                            base.font()
                        },
                        color: rgb(if empty { 0x8b877f } else { 0x262723 }).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    });
                }
                if let Ok(shaped) = w.text_system().shape_text(
                    (if line.is_empty() { " " } else { line })
                        .to_string()
                        .into(),
                    font_size,
                    &runs,
                    Some((width - x - if is_code { px(12.) } else { px(0.) }).max(px(20.))),
                    None,
                ) {
                    for shaped in shaped {
                        let h = shaped.size(height).height.max(height);
                        rows.push(Row {
                            text_len: line.len(),
                            line: shaped,
                            start,
                            y,
                            height,
                            x,
                            block_index,
                        });
                        y += h;
                    }
                }
                start += line.len() + 1;
                let continues_code =
                    is_code && start <= p.starts[block_index] + blocks[block_index].len();
                if is_code && !continues_code {
                    y += px(10.);
                }
                if line_index + 1 < text.split('\n').count() && !continues_code {
                    y += px(3.);
                }
            }
            measured.replace(Layout {
                rows,
                bounds: Bounds::default(),
            });
            size(width, y + px(6.))
        });
        (layout, EditorLayout { text: state, media })
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        w: &mut Window,
        cx: &mut App,
    ) {
        state.text.borrow_mut().bounds = bounds;
        if !state.media.borrow().is_empty()
            && self.editor.read(cx).layout.bounds.size.width != bounds.size.width
        {
            let editor = self.editor.clone();
            w.on_next_frame(move |_, cx| {
                editor.update(cx, |_, cx| cx.notify());
            });
        }
        for slot in state.media.borrow_mut().iter_mut() {
            slot.element
                .prepaint_at(bounds.origin + point(slot.x, slot.y), w, cx);
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        _: &mut (),
        w: &mut Window,
        cx: &mut App,
    ) {
        let layout = state.text.borrow().clone();
        let input = self.editor.read(cx);
        let focus = input.focus_handle.clone();
        let selected = input.selection.clone();
        let cursor = input.cursor();
        let (caret, h) = layout.position(cursor);
        let reveal = input.reveal;
        let scroll = input.scroll.clone();
        let blocks = input.state.document.blocks.clone();
        w.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        for (index, block) in blocks.iter().enumerate() {
            if !matches!(block.kind, BlockKind::Code { .. }) {
                continue;
            }
            let mut rows = layout.rows.iter().filter(|r| r.block_index == index);
            if let Some(first) = rows.next() {
                let last = rows.last().unwrap_or(first);
                let top = first.y - px(10.);
                let bottom = last.y + last.line.size(last.height).height.max(last.height) + px(10.);
                let rect = Bounds::new(
                    bounds.origin + point(px(0.), top),
                    size(bounds.size.width, bottom - top),
                );
                w.paint_quad(fill(rect, rgb(0xeeeae2)));
            }
        }
        for row in &layout.rows {
            let origin = bounds.origin + point(row.x, row.y);
            let is_first = layout
                .rows
                .iter()
                .find(|r| r.block_index == row.block_index)
                .is_some_and(|r| r.start == row.start && r.y == row.y);
            if is_first {
                let marker_origin =
                    bounds.origin + point(row.x - px(24.), row.y + (row.height - px(15.)) / 2.);
                if let BlockKind::Task { checked, .. } = &blocks[row.block_index].kind {
                    let marker_bounds = Bounds::new(marker_origin, size(px(15.), px(15.)));
                    let mut quad = if *checked {
                        fill(marker_bounds, rgb(0x245356))
                    } else {
                        outline(marker_bounds, rgb(0x68645e), BorderStyle::Solid)
                    };
                    quad.corner_radii = px(3.).into();
                    w.paint_quad(quad);
                    if *checked {
                        let mut path = PathBuilder::stroke(px(1.6));
                        path.move_to(marker_origin + point(px(3.), px(7.5)));
                        path.line_to(marker_origin + point(px(6.), px(10.5)));
                        path.line_to(marker_origin + point(px(12.), px(4.5)));
                        if let Ok(path) = path.build() {
                            w.paint_path(path, rgb(0xf7f3ea));
                        }
                    }
                }
                let label = match &blocks[row.block_index].kind {
                    BlockKind::Bullet { .. } => "•".into(),
                    BlockKind::Ordered { number, .. } => format!("{number}."),
                    BlockKind::Quote => "│".into(),
                    _ => String::new(),
                };
                if !label.is_empty() {
                    let run = TextRun {
                        len: label.len(),
                        font: w.text_style().font(),
                        color: rgb(0x245356).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let shaped = w
                        .text_system()
                        .shape_line(label.into(), px(16.), &[run], None);
                    let _ = shaped.paint(
                        bounds.origin
                            + point(
                                row.x
                                    - if matches!(blocks[row.block_index].kind, BlockKind::Quote) {
                                        px(14.)
                                    } else {
                                        px(24.)
                                    },
                                row.y,
                            ),
                        row.height,
                        w,
                        cx,
                    );
                }
            }
            if focus.is_focused(w) && !selected.is_empty() {
                let a = selected.start.max(row.start);
                let b = selected.end.min(row.start + row.line.len());
                // Paint selection by grapheme so wrapped lines and mixed-width CJK stay aligned.
                for (i, g) in row.line.text.grapheme_indices(true) {
                    if row.start + i >= a && row.start + i < b {
                        let p = row
                            .line
                            .position_for_index(i, row.height)
                            .unwrap_or_default();
                        let q = row
                            .line
                            .position_for_index(i + g.len(), row.height)
                            .unwrap_or(p);
                        let width = if q.y == p.y {
                            (q.x - p.x).max(px(3.))
                        } else {
                            bounds.size.width - p.x
                        };
                        w.paint_quad(fill(
                            Bounds::new(origin + p, size(width, row.height)),
                            rgba(0x24535630),
                        ));
                    }
                }
            }
            let _ =
                row.line
                    .paint_background(origin, row.height, TextAlign::Left, Some(bounds), w, cx);
            let _ = row
                .line
                .paint(origin, row.height, TextAlign::Left, Some(bounds), w, cx);
        }
        for slot in state.media.borrow_mut().iter_mut() {
            slot.element.paint(w, cx);
            let start = input_block_start(&blocks, slot.block);
            let end = start + blocks[slot.block].len();
            if selected.start < end && selected.end > start {
                let rect = Bounds::new(
                    bounds.origin + point(slot.x, slot.y),
                    size((bounds.size.width - slot.x).max(px(0.)), slot.height),
                );
                w.paint_quad(fill(rect, rgba(0x24535628)));
            }
        }
        if focus.is_focused(w) && selected.is_empty() {
            w.paint_quad(fill(Bounds::new(caret, size(px(1.5), h)), rgb(0x245356)));
        }
        self.editor.update(cx, |s, _| {
            s.media_bounds = state
                .media
                .borrow()
                .iter()
                .map(|slot| {
                    Bounds::new(
                        bounds.origin + point(slot.x, slot.y),
                        size((bounds.size.width - slot.x).max(px(0.)), slot.height),
                    )
                })
                .collect();
            s.layout = layout;
            s.reveal = false;
        });
        if reveal && focus.is_focused(w) {
            let viewport = scroll.bounds();
            let mut offset = scroll.offset();
            if viewport.size.height > px(0.) {
                if caret.y < viewport.top() + px(8.) {
                    offset.y += viewport.top() + px(8.) - caret.y;
                } else if caret.y + h > viewport.bottom() - px(8.) {
                    offset.y -= caret.y + h - viewport.bottom() + px(8.);
                }
                if offset != scroll.offset() {
                    let editor = self.editor.clone();
                    w.on_next_frame(move |w, cx| {
                        scroll.set_offset(offset);
                        editor.update(cx, |s, cx| {
                            s.reveal = true;
                            cx.notify();
                        });
                        w.refresh();
                    });
                }
            }
        }
    }
}

fn input_block_start(blocks: &[crate::document::Block], index: usize) -> usize {
    blocks[..index].iter().map(|b| b.len() + 1).sum()
}

/// Lightweight lexical highlighting: comments, strings, numbers and common keywords.
fn code_colors(line: &str) -> Vec<u32> {
    let bytes = line.as_bytes();
    let mut colors = vec![0x303a40; bytes.len()];
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if bytes[i] == b'#' || (bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/')) {
            colors[i..].fill(0x7b8575);
            break;
        } else if matches!(bytes[i], b'"' | b'\'' | b'`') {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                } else {
                    let c = bytes[i];
                    i += 1;
                    if c == quote {
                        break;
                    }
                }
            }
            colors[start..i].fill(0x8a623e);
        } else if bytes[i].is_ascii_digit() {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.') {
                i += 1;
            }
            colors[start..i].fill(0x95647c);
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if matches!(
                &line[start..i],
                "fn" | "let"
                    | "mut"
                    | "pub"
                    | "use"
                    | "impl"
                    | "struct"
                    | "enum"
                    | "if"
                    | "else"
                    | "for"
                    | "while"
                    | "return"
                    | "match"
                    | "const"
                    | "function"
                    | "class"
                    | "import"
                    | "from"
                    | "def"
                    | "in"
                    | "async"
                    | "await"
                    | "true"
                    | "false"
                    | "null"
                    | "None"
                    | "self"
                    | "this"
            ) {
                colors[start..i].fill(0x406e9b);
            }
        } else {
            i += 1;
        }
    }
    colors
}
