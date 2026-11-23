use crate::mapping::StepMap;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: Option<String>,
    #[serde(default)]
    pub image: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    Bold,
    Italic,
    Code,
    Strike,
}
impl Marks {
    fn get(&self, k: MarkKind) -> bool {
        match k {
            MarkKind::Bold => self.bold,
            MarkKind::Italic => self.italic,
            MarkKind::Code => self.code,
            MarkKind::Strike => self.strike,
        }
    }
    fn set(&mut self, k: MarkKind, v: bool) {
        match k {
            MarkKind::Bold => self.bold = v,
            MarkKind::Italic => self.italic = v,
            MarkKind::Code => self.code = v,
            MarkKind::Strike => self.strike = v,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    pub marks: Marks,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockKind {
    #[default]
    Paragraph,
    Heading(u8),
    Bullet {
        depth: u8,
    },
    Ordered {
        number: u64,
        depth: u8,
    },
    Task {
        checked: bool,
        depth: u8,
    },
    Quote,
    Code {
        language: String,
    },
    /// Host owns bytes and rendering. The ID is stable across machines.
    Attachment {
        id: String,
        kind: String,
        label: String,
        #[serde(default)]
        depth: u8,
    },
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub kind: BlockKind,
    pub content: Vec<Span>,
}
impl Block {
    pub fn text(&self) -> String {
        self.content.iter().map(|s| s.text.as_str()).collect()
    }
    pub fn len(&self) -> usize {
        self.content.iter().map(|s| s.text.len()).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn append(&mut self, text: &str, marks: Marks) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.content.last_mut().filter(|s| s.marks == marks) {
            last.text.push_str(text);
        } else {
            self.content.push(Span {
                text: text.into(),
                marks,
            });
        }
    }
    fn slice(&self, range: Range<usize>) -> Vec<Span> {
        let mut result = Block::default();
        let mut pos = 0;
        for s in &self.content {
            let a = range.start.max(pos);
            let b = range.end.min(pos + s.text.len());
            if a < b {
                result.append(&s.text[a - pos..b - pos], s.marks.clone());
            }
            pos += s.text.len();
        }
        result.content
    }
    fn continuation(&self) -> BlockKind {
        match &self.kind {
            BlockKind::Heading(_) => BlockKind::Paragraph,
            BlockKind::Task { depth, .. } => BlockKind::Task {
                checked: false,
                depth: *depth,
            },
            BlockKind::Ordered { number, depth } => BlockKind::Ordered {
                number: number + 1,
                depth: *depth,
            },
            other => other.clone(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub blocks: Vec<Block>,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            blocks: vec![Block::default()],
        }
    }
}
impl Document {
    fn renumber_ordered_lists(&mut self) {
        let mut counters: Vec<(u8, u64)> = Vec::new();
        let mut previous_depth = None;
        for block in &mut self.blocks {
            match &mut block.kind {
                BlockKind::Ordered { number, depth } => {
                    while counters.last().is_some_and(|(d, _)| *d > *depth) {
                        counters.pop();
                    }
                    if let Some((_, next)) = counters.last_mut().filter(|(d, _)| *d == *depth) {
                        *number = *next;
                        *next = next.saturating_add(1);
                    } else {
                        if previous_depth.is_some_and(|d| *depth > d) {
                            *number = 1;
                        }
                        counters.push((*depth, number.saturating_add(1)));
                    }
                    previous_depth = Some(*depth);
                }
                BlockKind::Bullet { depth } | BlockKind::Task { depth, .. } => {
                    while counters.last().is_some_and(|(d, _)| *d >= *depth) {
                        counters.pop();
                    }
                    previous_depth = Some(*depth);
                }
                _ => {
                    counters.clear();
                    previous_depth = None;
                }
            }
        }
    }
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .map(Block::text)
            .collect::<Vec<_>>()
            .join("\n")
    }
    pub fn len(&self) -> usize {
        self.blocks.iter().map(Block::len).sum::<usize>() + self.blocks.len().saturating_sub(1)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn selected_markdown(&self, range: Range<usize>) -> String {
        let mut blocks = Vec::new();
        for (i, block) in self.blocks.iter().enumerate() {
            let start = self.block_start(i);
            let a = range.start.saturating_sub(start).min(block.len());
            let b = range.end.saturating_sub(start).min(block.len());
            if a < b {
                blocks.push(Block {
                    kind: block.kind.clone(),
                    content: block.slice(a..b),
                });
            }
        }
        Document { blocks }.to_markdown()
    }
    pub fn locate(&self, offset: usize) -> (usize, usize) {
        let mut pos = 0;
        for (i, b) in self.blocks.iter().enumerate() {
            if offset <= pos + b.len() {
                return (i, offset.saturating_sub(pos));
            }
            pos += b.len() + 1;
        }
        (self.blocks.len() - 1, self.blocks.last().unwrap().len())
    }
    pub fn block_start(&self, index: usize) -> usize {
        self.blocks[..index].iter().map(|b| b.len() + 1).sum()
    }
    fn valid_range(&self, r: &Range<usize>) -> bool {
        let t = self.text();
        r.start <= r.end
            && r.end <= t.len()
            && t.is_char_boundary(r.start)
            && t.is_char_boundary(r.end)
    }
    pub fn marks_at(&self, pos: usize) -> Marks {
        let (i, p) = self.locate(pos);
        let mut at = 0;
        let b = &self.blocks[i];
        for s in &b.content {
            if p <= at + s.text.len() {
                return s.marks.clone();
            }
            at += s.text.len();
        }
        Marks::default()
    }
    pub fn replace(
        &mut self,
        r: Range<usize>,
        text: &str,
        marks: Marks,
    ) -> Result<StepMap, &'static str> {
        if !self.valid_range(&r) {
            return Err("invalid UTF-8 document range");
        }
        let (i, a) = self.locate(r.start);
        let (j, b) = self.locate(r.end);
        let first = &self.blocks[i];
        let last = &self.blocks[j];
        let prefix = first.slice(0..a);
        let suffix = last.slice(b..last.len());
        if i == j && matches!(first.kind, BlockKind::Code { .. }) {
            let mut block = Block {
                kind: first.kind.clone(),
                content: prefix,
            };
            block.append(text, marks);
            for span in suffix {
                block.append(&span.text, span.marks);
            }
            self.blocks[i] = block;
            return Ok(StepMap {
                start: r.start,
                old_size: r.len(),
                new_size: text.len(),
            });
        }
        let parts: Vec<_> = text.split('\n').collect();
        let mut blocks = vec![];
        for (n, part) in parts.iter().enumerate() {
            let kind = if n == 0 {
                first.kind.clone()
            } else if n == parts.len() - 1 && j > i {
                last.kind.clone()
            } else {
                first.continuation()
            };
            let mut block = Block {
                kind,
                content: if n == 0 { prefix.clone() } else { vec![] },
            };
            block.append(part, marks.clone());
            if n == parts.len() - 1 {
                for s in &suffix {
                    block.append(&s.text, s.marks.clone());
                }
            }
            if matches!(block.kind, BlockKind::Attachment { .. }) && block.text() != "\u{fffc}" {
                block.kind = BlockKind::Paragraph;
            }
            blocks.push(block);
        }
        self.blocks.splice(i..=j, blocks);
        Ok(StepMap {
            start: r.start,
            old_size: r.len(),
            new_size: text.len(),
        })
    }
    pub fn set_mark(
        &mut self,
        r: Range<usize>,
        kind: MarkKind,
        value: bool,
    ) -> Result<(), &'static str> {
        if !self.valid_range(&r) {
            return Err("invalid mark range");
        }
        let mut pos = 0;
        for b in &mut self.blocks {
            let old = b.clone();
            b.content.clear();
            let end = pos + old.len();
            let a = r.start.max(pos).min(end) - pos;
            let z = r.end.max(pos).min(end) - pos;
            for s in old.slice(0..a) {
                b.append(&s.text, s.marks);
            }
            for mut s in old.slice(a..z) {
                s.marks.set(kind, value);
                b.append(&s.text, s.marks);
            }
            for s in old.slice(z..old.len()) {
                b.append(&s.text, s.marks);
            }
            pos = end + 1;
        }
        Ok(())
    }
    fn range_has_mark(&self, range: &Range<usize>, kind: MarkKind) -> bool {
        let mut pos = 0;
        let mut found = false;
        for block in &self.blocks {
            for span in &block.content {
                let end = pos + span.text.len();
                if pos < range.end && end > range.start {
                    found = true;
                    if !span.marks.get(kind) {
                        return false;
                    }
                }
                pos = end;
            }
            pos += 1;
        }
        found
    }
    pub fn replace_document(
        &mut self,
        range: Range<usize>,
        mut fragment: Document,
    ) -> Result<StepMap, &'static str> {
        if !self.valid_range(&range) || fragment.blocks.is_empty() {
            return Err("invalid document slice");
        }
        let (i, a) = self.locate(range.start);
        let (j, b) = self.locate(range.end);
        let prefix = self.blocks[i].slice(0..a);
        let suffix = self.blocks[j].slice(b..self.blocks[j].len());
        let new_size = fragment.len();
        let original_kind = self.blocks[i].kind.clone();
        let first = &mut fragment.blocks[0];
        let old = std::mem::take(&mut first.content);
        if matches!(first.kind, BlockKind::Paragraph) {
            first.kind = original_kind;
        }
        for span in prefix.into_iter().chain(old) {
            first.append(&span.text, span.marks);
        }
        let last = fragment.blocks.last_mut().unwrap();
        for span in suffix {
            last.append(&span.text, span.marks);
        }
        self.blocks.splice(i..=j, fragment.blocks);
        Ok(StepMap {
            start: range.start,
            old_size: range.len(),
            new_size,
        })
    }
    pub fn from_markdown(source: &str) -> Self {
        let mut blocks = vec![];
        let mut block = Block::default();
        let mut mark = Marks::default();
        let mut stack: Vec<Marks> = vec![];
        let mut image_starts: Vec<usize> = vec![];
        let mut lists: Vec<Option<u64>> = vec![];
        let mut in_item = false;
        let mut code = false;
        let mut quote = 0;
        fn flush(block: &mut Block, blocks: &mut Vec<Block>) {
            if block.content.len() == 1 {
                let span = &block.content[0];
                if let Some(url) = span
                    .marks
                    .link
                    .as_deref()
                    .and_then(|s| s.strip_prefix("attachment:"))
                {
                    let (id, kind) = url.split_once("?kind=").unwrap_or((url, "file"));
                    let (kind, depth) = kind
                        .split_once("&depth=")
                        .map(|(k, d)| (k, d.parse::<u8>().unwrap_or(0).min(16)))
                        .unwrap_or((kind, 0));
                    block.kind = BlockKind::Attachment {
                        id: id.into(),
                        kind: kind.into(),
                        label: span.text.clone(),
                        depth,
                    };
                    block.content = vec![Span {
                        text: "\u{fffc}".into(),
                        marks: Marks::default(),
                    }];
                }
            }
            blocks.push(std::mem::take(block));
        }
        for event in Parser::new_ext(
            source,
            Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH,
        ) {
            match event {
                Event::Start(Tag::Paragraph) => {
                    if !block.is_empty() {
                        flush(&mut block, &mut blocks);
                    }
                    if quote > 0 && !in_item {
                        block.kind = BlockKind::Quote;
                    }
                }
                Event::End(TagEnd::Paragraph) => {
                    if !in_item {
                        flush(&mut block, &mut blocks);
                    }
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    if !block.is_empty() {
                        flush(&mut block, &mut blocks);
                    }
                    block.kind = BlockKind::Heading(level as u8);
                }
                Event::End(TagEnd::Heading(_)) => flush(&mut block, &mut blocks),
                Event::Start(Tag::List(start)) => {
                    lists.push(start);
                }
                Event::End(TagEnd::List(_)) => {
                    lists.pop();
                }
                Event::Start(Tag::Item) => {
                    if !block.is_empty() {
                        flush(&mut block, &mut blocks);
                    }
                    in_item = true;
                    let depth = lists.len().saturating_sub(1) as u8;
                    block.kind = match lists.last().copied().flatten() {
                        Some(number) => BlockKind::Ordered { number, depth },
                        None => BlockKind::Bullet { depth },
                    };
                }
                Event::End(TagEnd::Item) => {
                    if !block.is_empty() || !matches!(block.kind, BlockKind::Paragraph) {
                        flush(&mut block, &mut blocks);
                    }
                    in_item = lists.len() > 1;
                    if let Some(Some(n)) = lists.last_mut() {
                        *n += 1;
                    }
                }
                Event::TaskListMarker(checked) => {
                    block.kind = BlockKind::Task {
                        checked,
                        depth: lists.len().saturating_sub(1) as u8,
                    };
                }
                Event::Start(Tag::BlockQuote(_)) => quote += 1,
                Event::End(TagEnd::BlockQuote(_)) => quote -= 1,
                Event::Start(Tag::CodeBlock(kind)) => {
                    if !block.is_empty() {
                        flush(&mut block, &mut blocks);
                    }
                    block.kind = BlockKind::Code {
                        language: match kind {
                            pulldown_cmark::CodeBlockKind::Fenced(s) => s.to_string(),
                            _ => String::new(),
                        },
                    };
                    code = true;
                }
                Event::End(TagEnd::CodeBlock) => {
                    if let Some(s) = block.content.last_mut() {
                        if s.text.ends_with('\n') {
                            s.text.pop();
                        }
                    }
                    flush(&mut block, &mut blocks);
                    code = false;
                }
                Event::Start(Tag::Strong) => {
                    stack.push(mark.clone());
                    mark.bold = true;
                }
                Event::Start(Tag::Emphasis) => {
                    stack.push(mark.clone());
                    mark.italic = true;
                }
                Event::Start(Tag::Strikethrough) => {
                    stack.push(mark.clone());
                    mark.strike = true;
                }
                Event::Start(Tag::Image { dest_url, .. }) => {
                    stack.push(mark.clone());
                    mark.link = Some(dest_url.to_string());
                    mark.image = true;
                    image_starts.push(block.content.len());
                    block.content.push(Span {
                        text: String::new(),
                        marks: mark.clone(),
                    });
                }
                Event::End(TagEnd::Image) => {
                    if let Some(start) = image_starts.pop() {
                        let alt = block
                            .content
                            .split_off(start)
                            .into_iter()
                            .map(|s| s.text)
                            .collect::<String>();
                        block.content.push(Span {
                            text: if alt.is_empty() { "图片".into() } else { alt },
                            marks: mark.clone(),
                        });
                    }
                    mark = stack.pop().unwrap_or_default();
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    stack.push(mark.clone());
                    mark.link = Some(dest_url.to_string());
                }
                Event::End(
                    TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link,
                ) => mark = stack.pop().unwrap_or_default(),
                Event::Code(s) => {
                    let mut m = mark.clone();
                    m.code = true;
                    block.append(&s, m);
                }
                Event::Text(s) | Event::Html(s) | Event::InlineHtml(s) => {
                    if code {
                        for (i, line) in s.split('\n').enumerate() {
                            if i > 0 {
                                block.append("\n", mark.clone());
                            }
                            block.append(line, mark.clone());
                        }
                    } else {
                        block.append(&s, mark.clone());
                    }
                }
                Event::SoftBreak | Event::HardBreak => block.append("\n", mark.clone()),
                Event::Rule => {
                    block.append("────────", Marks::default());
                    flush(&mut block, &mut blocks);
                }
                _ => {}
            }
        }
        if !block.is_empty() || blocks.is_empty() {
            blocks.push(block);
        }
        if blocks
            .last()
            .is_some_and(|b| matches!(b.kind, BlockKind::Attachment { .. }))
        {
            blocks.push(Block::default());
        }
        Self { blocks }
    }
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        for (i, b) in self.blocks.iter().enumerate() {
            if i > 0 {
                let previous = &self.blocks[i - 1].kind;
                let compact = matches!(
                    previous,
                    BlockKind::Bullet { .. } | BlockKind::Ordered { .. } | BlockKind::Task { .. }
                ) && matches!(
                    b.kind,
                    BlockKind::Bullet { .. } | BlockKind::Ordered { .. } | BlockKind::Task { .. }
                );
                out.push_str(if compact { "\n" } else { "\n\n" });
            }
            let body = inline_markdown(&b.content);
            match &b.kind {
                BlockKind::Paragraph => out.push_str(&preserve_indentation(&body)),
                BlockKind::Heading(level) => {
                    out.push_str(&"#".repeat((*level).clamp(1, 6) as usize));
                    out.push(' ');
                    out.push_str(&body);
                }
                BlockKind::Bullet { depth } => {
                    out.push_str(&"    ".repeat(*depth as usize));
                    out.push_str("- ");
                    out.push_str(&body);
                }
                BlockKind::Ordered { number, depth } => {
                    out.push_str(&format!(
                        "{}{}. {}",
                        "    ".repeat(*depth as usize),
                        number,
                        body
                    ));
                }
                BlockKind::Task { checked, depth } => {
                    out.push_str(&format!(
                        "{}- [{}] {}",
                        "    ".repeat(*depth as usize),
                        if *checked { "x" } else { " " },
                        body
                    ));
                }
                BlockKind::Quote => {
                    out.push_str(&format!("> {}", body.replace('\n', "\n> ")));
                }
                BlockKind::Code { language } => {
                    let text = b.text();
                    let fence = "`".repeat(longest_ticks(&text).max(2) + 1);
                    out.push_str(&format!("{fence}{language}\n{text}\n{fence}"));
                }
                BlockKind::Attachment {
                    id,
                    kind,
                    label,
                    depth,
                } => {
                    out.push_str(&format!(
                        "[{}](attachment:{}?kind={}&depth={})",
                        escape(label),
                        id,
                        kind,
                        depth
                    ));
                }
            }
        }
        out
    }
}
// Literal indentation is document text, not an implicit Markdown code block.
fn preserve_indentation(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let count = line
                .bytes()
                .take_while(|c| *c == b' ' || *c == b'\t')
                .count();
            let mut encoded = String::new();
            for c in line[..count].chars() {
                encoded.push_str(if c == '\t' { "&#9;" } else { "&#32;" });
            }
            encoded.push_str(&line[count..]);
            encoded
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn longest_ticks(s: &str) -> usize {
    s.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}
fn escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '*' | '_' | '[' | ']' | '`' | '~' | '#' | '>' | '!' | '|' | '-' | '.' | '+'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
#[derive(Clone, PartialEq)]
enum Delimiter {
    Bold,
    Italic,
    Strike,
    Link(String, bool),
}
fn inline_markdown(spans: &[Span]) -> String {
    fn open(d: &Delimiter) -> &str {
        match d {
            Delimiter::Bold => "**",
            Delimiter::Italic => "_",
            Delimiter::Strike => "~~",
            Delimiter::Link(_, image) => {
                if *image {
                    "!["
                } else {
                    "["
                }
            }
        }
    }
    fn close(d: &Delimiter) -> String {
        match d {
            Delimiter::Link(url, _) => format!(
                "]({})",
                url.replace('(', "%28")
                    .replace(')', "%29")
                    .replace(' ', "%20")
            ),
            _ => open(d).into(),
        }
    }
    fn transition(active: &mut Vec<Delimiter>, wanted: Vec<Delimiter>, out: &mut String) {
        let common = active
            .iter()
            .zip(&wanted)
            .take_while(|(a, b)| a == b)
            .count();
        for d in active[common..].iter().rev() {
            out.push_str(&close(d));
        }
        for d in &wanted[common..] {
            out.push_str(open(d));
        }
        *active = wanted;
    }
    let mut out = String::new();
    let mut active: Vec<Delimiter> = vec![];
    for s in spans {
        if s.text.is_empty() {
            continue;
        }
        let trimmed = s.text.trim_matches(char::is_whitespace);
        if trimmed.is_empty() {
            transition(&mut active, vec![], &mut out);
            out.push_str(&s.text);
            continue;
        }
        let leading = s.text.len() - s.text.trim_start_matches(char::is_whitespace).len();
        let trailing = s.text.trim_end_matches(char::is_whitespace).len();
        if leading > 0 {
            transition(&mut active, vec![], &mut out);
            out.push_str(&s.text[..leading]);
        }
        let mut wanted = vec![];
        if let Some(url) = &s.marks.link {
            wanted.push(Delimiter::Link(url.clone(), s.marks.image));
        }
        if s.marks.bold {
            wanted.push(Delimiter::Bold);
        }
        if s.marks.italic {
            wanted.push(Delimiter::Italic);
        }
        if s.marks.strike {
            wanted.push(Delimiter::Strike);
        }
        transition(&mut active, wanted, &mut out);
        let text = &s.text[leading..trailing];
        if s.marks.code {
            let fence = "`".repeat(longest_ticks(text) + 1);
            out.push_str(&fence);
            if text.starts_with('`') {
                out.push(' ');
            }
            out.push_str(text);
            if text.ends_with('`') {
                out.push(' ');
            }
            out.push_str(&fence);
        } else {
            out.push_str(&escape(text));
        }
        if trailing < s.text.len() {
            transition(&mut active, vec![], &mut out);
            out.push_str(&s.text[trailing..]);
        }
    }
    transition(&mut active, vec![], &mut out);
    out
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}
impl Selection {
    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
    pub fn caret(p: usize) -> Self {
        Self { anchor: p, head: p }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub document: Document,
    pub selection: Selection,
    pub stored_marks: Option<Marks>,
}
#[derive(Clone, Debug)]
pub enum Step {
    Replace {
        range: Range<usize>,
        text: String,
        marks: Marks,
    },
    ToggleMark {
        range: Range<usize>,
        kind: MarkKind,
    },
    SetBlock {
        index: usize,
        kind: BlockKind,
    },
    ToggleTask {
        index: usize,
    },
}
#[derive(Clone, Debug, Default)]
pub struct Transaction {
    pub steps: Vec<Step>,
    pub selection: Option<Selection>,
}
#[derive(Clone, Debug)]
pub struct EditorState {
    pub document: Document,
    pub selection: Selection,
    pub stored_marks: Option<Marks>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    composition: Option<(Snapshot, Range<usize>)>,
}
impl EditorState {
    pub fn new(document: Document) -> Self {
        Self {
            document,
            selection: Selection::caret(0),
            stored_marks: None,
            undo: vec![],
            redo: vec![],
            composition: None,
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            document: self.document.clone(),
            selection: self.selection.clone(),
            stored_marks: self.stored_marks.clone(),
        }
    }
    /// Returns false while IME owns a provisional range. The caller must keep
    /// the external revision pending until it is accepted or explicitly merged.
    pub fn sync_external(&mut self, document: Document) -> bool {
        if self.composition.is_some() {
            return false;
        }
        if document.to_markdown() == self.document.to_markdown() {
            return true;
        }
        let text = document.text();
        let mut cursor = self.selection.head.min(text.len());
        while !text.is_char_boundary(cursor) {
            cursor -= 1;
        }
        *self = Self::new(document);
        self.selection = Selection::caret(cursor);
        true
    }
    fn restore(&mut self, s: Snapshot) {
        self.document = s.document;
        self.selection = s.selection;
        self.stored_marks = s.stored_marks;
    }
    fn record(&mut self, before: Snapshot) {
        if before != self.snapshot() {
            self.undo.push(before);
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }
    pub fn select(&mut self, anchor: usize, head: usize) -> Result<(), &'static str> {
        let t = self.document.text();
        if anchor > t.len()
            || head > t.len()
            || !t.is_char_boundary(anchor)
            || !t.is_char_boundary(head)
        {
            return Err("invalid selection");
        }
        self.selection = Selection { anchor, head };
        self.stored_marks = None;
        Ok(())
    }
    pub fn apply(&mut self, transaction: Transaction) -> Result<Vec<StepMap>, &'static str> {
        let before = self.snapshot();
        let normalize_lists = transaction
            .steps
            .iter()
            .any(|s| matches!(s, Step::SetBlock { .. }));
        let mut maps = vec![];
        let result = (|| {
            for step in transaction.steps {
                match step {
                    Step::Replace { range, text, marks } => {
                        let map = self.document.replace(range, &text, marks)?;
                        self.selection.anchor = map.map(self.selection.anchor, 1).position;
                        self.selection.head = map.map(self.selection.head, 1).position;
                        maps.push(map);
                    }
                    Step::ToggleMark { range, kind } => {
                        let enabled = !self.document.range_has_mark(&range, kind);
                        self.document.set_mark(range, kind, enabled)?;
                    }
                    Step::SetBlock { index, kind } => {
                        self.document
                            .blocks
                            .get_mut(index)
                            .ok_or("invalid block")?
                            .kind = kind;
                    }
                    Step::ToggleTask { index } => {
                        match &mut self
                            .document
                            .blocks
                            .get_mut(index)
                            .ok_or("invalid block")?
                            .kind
                        {
                            BlockKind::Task { checked, .. } => *checked = !*checked,
                            _ => return Err("not a task"),
                        }
                    }
                }
            }
            if normalize_lists {
                self.document.renumber_ordered_lists();
            }
            if let Some(s) = transaction.selection {
                self.select(s.anchor, s.head)?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.restore(before);
            return Err(e);
        }
        self.record(before);
        Ok(maps)
    }
    pub fn replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        input_rules: bool,
    ) -> Result<(), &'static str> {
        let before = self.snapshot();
        let marks = self
            .stored_marks
            .clone()
            .unwrap_or_else(|| self.document.marks_at(range.start));
        self.document.replace(range.clone(), text, marks)?;
        self.selection = Selection::caret(range.start + text.len());
        if input_rules {
            self.input_rules();
        }
        self.record(before);
        Ok(())
    }
    pub fn replace_markdown(
        &mut self,
        range: Range<usize>,
        source: &str,
    ) -> Result<(), &'static str> {
        let before = self.snapshot();
        let (index, _) = self.document.locate(range.start);
        if matches!(self.document.blocks[index].kind, BlockKind::Code { .. }) {
            return self.replace(range, source, false);
        }
        let incoming = Document::from_markdown(source);
        let map = self.document.replace_document(range.clone(), incoming)?;
        self.selection = Selection::caret(range.start + map.new_size);
        self.record(before);
        Ok(())
    }
    /// Insert a host-owned atomic media node between the two sides of the current selection.
    pub fn insert_attachment(
        &mut self,
        id: String,
        kind: String,
        label: String,
    ) -> Result<(), &'static str> {
        let before = self.snapshot();
        let range = self.selection.range();
        let (index, _) = self.document.locate(range.start);
        let depth = match self.document.blocks[index].kind {
            BlockKind::Bullet { depth }
            | BlockKind::Ordered { depth, .. }
            | BlockKind::Task { depth, .. }
            | BlockKind::Attachment { depth, .. } => depth,
            _ => (self.document.blocks[index]
                .text()
                .chars()
                .take_while(|c| *c == ' ')
                .count()
                / 4)
            .min(16) as u8,
        };
        let fragment = Document {
            blocks: vec![
                Block::default(),
                Block {
                    kind: BlockKind::Attachment {
                        id,
                        kind,
                        label,
                        depth,
                    },
                    content: vec![Span {
                        text: "\u{fffc}".into(),
                        marks: Marks::default(),
                    }],
                },
                Block::default(),
            ],
        };
        let map = self.document.replace_document(range.clone(), fragment)?;
        self.selection = Selection::caret(range.start + map.new_size);
        self.record(before);
        Ok(())
    }
    pub fn toggle_mark(&mut self, kind: MarkKind) -> Result<(), &'static str> {
        let r = self.selection.range();
        if r.is_empty() {
            let mut m = self
                .stored_marks
                .clone()
                .unwrap_or_else(|| self.document.marks_at(r.start));
            m.set(kind, !m.get(kind));
            self.stored_marks = Some(m);
            Ok(())
        } else {
            self.apply(Transaction {
                steps: vec![Step::ToggleMark { range: r, kind }],
                selection: None,
            })
            .map(|_| ())
        }
    }
    pub fn set_block(&mut self, kind: BlockKind) -> Result<(), &'static str> {
        let (i, _) = self.document.locate(self.selection.head);
        self.apply(Transaction {
            steps: vec![Step::SetBlock { index: i, kind }],
            selection: None,
        })
        .map(|_| ())
    }
    pub fn toggle_task(&mut self, index: usize) -> Result<(), &'static str> {
        self.apply(Transaction {
            steps: vec![Step::ToggleTask { index }],
            selection: None,
        })
        .map(|_| ())
    }
    pub fn delete(&mut self, back: bool) -> Result<(), &'static str> {
        let r = self.selection.range();
        if !r.is_empty() {
            return self.replace(r, "", false);
        }
        let p = r.start;
        let (i, local) = self.document.locate(p);
        if back && local == 0 && !matches!(self.document.blocks[i].kind, BlockKind::Paragraph) {
            return self.set_block(BlockKind::Paragraph);
        }
        let t = self.document.text();
        let q = if back {
            t.grapheme_indices(true)
                .rev()
                .find(|(n, _)| *n < p)
                .map(|(n, _)| n)
                .unwrap_or(0)
        } else {
            t.grapheme_indices(true)
                .find(|(n, _)| *n > p)
                .map(|(n, _)| n)
                .unwrap_or(t.len())
        };
        self.replace(p.min(q)..p.max(q), "", false)
    }
    pub fn exit_code(&mut self) -> Result<(), &'static str> {
        let (index, _) = self.document.locate(self.selection.head);
        if !matches!(self.document.blocks[index].kind, BlockKind::Code { .. }) {
            return Ok(());
        }
        let before = self.snapshot();
        self.document.blocks.insert(index + 1, Block::default());
        self.selection = Selection::caret(self.document.block_start(index + 1));
        self.stored_marks = None;
        self.record(before);
        Ok(())
    }
    pub fn enter(&mut self) -> Result<(), &'static str> {
        let (i, local) = self.document.locate(self.selection.head);
        let text = self.document.blocks[i].text();
        if matches!(self.document.blocks[i].kind, BlockKind::Code { .. })
            && self.selection.range().is_empty()
            && local == text.len()
            && (text.is_empty() || text.ends_with('\n'))
        {
            let before = self.snapshot();
            if text.ends_with('\n') {
                let block = &mut self.document.blocks[i];
                block.content = block.slice(0..block.len() - 1);
            }
            self.document.blocks.insert(i + 1, Block::default());
            self.selection = Selection::caret(self.document.block_start(i + 1));
            self.stored_marks = None;
            self.record(before);
            return Ok(());
        }
        if matches!(self.document.blocks[i].kind, BlockKind::Paragraph) && local == text.len() {
            if let Some(language) = text
                .strip_prefix("```")
                .or_else(|| text.strip_prefix("~~~"))
            {
                if !language.contains(['`', '~', ' ']) {
                    let start = self.document.block_start(i);
                    self.apply(Transaction {
                        steps: vec![Step::SetBlock {
                            index: i,
                            kind: BlockKind::Code {
                                language: language.into(),
                            },
                        }],
                        selection: None,
                    })?;
                    return self.replace(start..start + text.len(), "", false);
                }
            }
        }
        if self.document.blocks[i].is_empty()
            && !matches!(
                self.document.blocks[i].kind,
                BlockKind::Paragraph | BlockKind::Code { .. }
            )
        {
            return self.set_block(BlockKind::Paragraph);
        }
        self.replace(self.selection.range(), "\n", false)
    }
    pub fn indent(&mut self, outdent: bool) -> Result<(), &'static str> {
        let range = self.selection.range();
        let (first, _) = self.document.locate(range.start);
        let (mut last, _) = self.document.locate(range.end);
        if !range.is_empty() && last > first && self.document.block_start(last) == range.end {
            last -= 1;
        }
        let mut steps = Vec::new();
        for index in (first..=last).rev() {
            let block = &self.document.blocks[index];
            if !outdent {
                let depth = match block.kind {
                    BlockKind::Bullet { depth }
                    | BlockKind::Ordered { depth, .. }
                    | BlockKind::Task { depth, .. } => Some(depth),
                    _ => None,
                };
                if let Some(depth) = depth {
                    let parent =
                        index
                            .checked_sub(1)
                            .and_then(|i| match self.document.blocks[i].kind {
                                BlockKind::Bullet { depth }
                                | BlockKind::Ordered { depth, .. }
                                | BlockKind::Task { depth, .. } => Some(depth),
                                _ => None,
                            });
                    if parent.is_none_or(|previous| previous < depth) {
                        continue;
                    }
                }
            }
            let kind = match block.kind {
                BlockKind::Bullet { depth } => Some(if outdent && depth == 0 {
                    BlockKind::Paragraph
                } else {
                    BlockKind::Bullet {
                        depth: if outdent {
                            depth.saturating_sub(1)
                        } else {
                            depth.saturating_add(1).min(16)
                        },
                    }
                }),
                BlockKind::Ordered { number, depth } => Some(if outdent && depth == 0 {
                    BlockKind::Paragraph
                } else {
                    BlockKind::Ordered {
                        number,
                        depth: if outdent {
                            depth.saturating_sub(1)
                        } else {
                            depth.saturating_add(1).min(16)
                        },
                    }
                }),
                BlockKind::Task { checked, depth } => Some(if outdent && depth == 0 {
                    BlockKind::Paragraph
                } else {
                    BlockKind::Task {
                        checked,
                        depth: if outdent {
                            depth.saturating_sub(1)
                        } else {
                            depth.saturating_add(1).min(16)
                        },
                    }
                }),
                BlockKind::Attachment {
                    ref id,
                    ref kind,
                    ref label,
                    depth,
                } => Some(BlockKind::Attachment {
                    id: id.clone(),
                    kind: kind.clone(),
                    label: label.clone(),
                    depth: if outdent {
                        depth.saturating_sub(1)
                    } else {
                        depth.saturating_add(1).min(16)
                    },
                }),
                _ => None,
            };
            if let Some(kind) = kind {
                steps.push(Step::SetBlock { index, kind });
            } else {
                let start = self.document.block_start(index);
                let text = block.text();
                let mut lines = Vec::new();
                let mut local = 0;
                for line in text.split('\n') {
                    let pos = start + local;
                    if (range.is_empty() && range.start >= pos && range.start <= pos + line.len())
                        || (!range.is_empty()
                            && pos <= range.end
                            && pos + line.len() >= range.start
                            && pos < range.end)
                    {
                        let remove = if outdent {
                            if line.starts_with('\t') {
                                1
                            } else {
                                line.bytes().take_while(|b| *b == b' ').count().min(4)
                            }
                        } else {
                            0
                        };
                        if !outdent || remove > 0 {
                            lines.push(Step::Replace {
                                range: pos..pos + remove,
                                text: if outdent {
                                    String::new()
                                } else {
                                    "    ".into()
                                },
                                marks: Marks::default(),
                            });
                        }
                    }
                    local += line.len() + 1;
                }
                steps.extend(lines.into_iter().rev());
            }
        }
        self.apply(Transaction {
            steps,
            selection: None,
        })
        .map(|_| ())
    }
    fn input_rules(&mut self) {
        let p = self.selection.head;
        let (i, local) = self.document.locate(p);
        if matches!(
            self.document.blocks[i].kind,
            BlockKind::Code { .. } | BlockKind::Attachment { .. }
        ) {
            return;
        }
        let start = self.document.block_start(i);
        let line = self.document.blocks[i].text();
        let before = &line[..local];
        let kind = match before {
            "# " => Some(BlockKind::Heading(1)),
            "## " => Some(BlockKind::Heading(2)),
            "### " => Some(BlockKind::Heading(3)),
            "#### " => Some(BlockKind::Heading(4)),
            "##### " => Some(BlockKind::Heading(5)),
            "###### " => Some(BlockKind::Heading(6)),
            "- " | "* " => Some(BlockKind::Bullet { depth: 0 }),
            "- [ ] " | "[ ] " => Some(BlockKind::Task {
                checked: false,
                depth: 0,
            }),
            "- [x] " | "[x] " => Some(BlockKind::Task {
                checked: true,
                depth: 0,
            }),
            "> " => Some(BlockKind::Quote),
            _ => before.strip_suffix(' ').and_then(|prefix| {
                let digits = prefix
                    .strip_suffix('.')
                    .or_else(|| prefix.strip_suffix(')'))?;
                if digits.is_empty()
                    || digits.len() > 9
                    || !digits.bytes().all(|b| b.is_ascii_digit())
                {
                    return None;
                }
                Some(BlockKind::Ordered {
                    number: digits.parse().ok()?,
                    depth: 0,
                })
            }),
        };
        if let Some(kind) = kind {
            let _ = self.document.replace(start..p, "", Marks::default());
            self.document.blocks[i].kind = kind;
            self.selection = Selection::caret(start);
            return;
        }
        for (marker, kind) in [
            ("**", MarkKind::Bold),
            ("__", MarkKind::Bold),
            ("~~", MarkKind::Strike),
            ("*", MarkKind::Italic),
            ("_", MarkKind::Italic),
            ("`", MarkKind::Code),
        ] {
            if let Some(body) = before.strip_suffix(marker) {
                if let Some(open) = body.rfind(marker) {
                    if marker.len() == 1
                        && ((open > 0 && body.as_bytes()[open - 1] == marker.as_bytes()[0])
                            || body.as_bytes().get(open + 1) == Some(&marker.as_bytes()[0]))
                    {
                        continue;
                    }
                    let inside = &body[open + marker.len()..];
                    if !inside.is_empty()
                        && !inside.starts_with(char::is_whitespace)
                        && !inside.ends_with(char::is_whitespace)
                    {
                        let a = start + open;
                        let b = p - marker.len();
                        let _ = self.document.replace(b..p, "", Marks::default());
                        let _ = self
                            .document
                            .replace(a..a + marker.len(), "", Marks::default());
                        let end = b - marker.len();
                        let _ = self.document.set_mark(a..end, kind, true);
                        self.selection = Selection::caret(end);
                        let mut next = self.stored_marks.clone().unwrap_or_default();
                        next.set(kind, false);
                        self.stored_marks = Some(next);
                        return;
                    }
                }
            }
        }
    }
    pub fn begin_composition(
        &mut self,
        range: Range<usize>,
        text: &str,
    ) -> Result<Range<usize>, &'static str> {
        let initial = self
            .composition
            .as_ref()
            .map(|(s, _)| s.clone())
            .unwrap_or_else(|| self.snapshot());
        let before = self.snapshot();
        let marks = self
            .stored_marks
            .clone()
            .unwrap_or_else(|| self.document.marks_at(range.start));
        if let Err(e) = self.document.replace(range.clone(), text, marks) {
            self.restore(before);
            return Err(e);
        }
        let marked = range.start..range.start + text.len();
        self.selection = Selection::caret(marked.end);
        self.composition = Some((initial, marked.clone()));
        Ok(marked)
    }
    pub fn composition_range(&self) -> Option<Range<usize>> {
        self.composition.as_ref().map(|(_, r)| r.clone())
    }
    pub fn commit_composition(&mut self) {
        if let Some((before, _)) = self.composition.take() {
            self.record(before);
        }
    }
    pub fn undo(&mut self) -> bool {
        self.commit_composition();
        if let Some(s) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(s);
            true
        } else {
            false
        }
    }
    pub fn redo(&mut self) -> bool {
        if let Some(s) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(s);
            true
        } else {
            false
        }
    }
}

/// Selection uses the whole Unicode word containing the hit, rather than two partial slices.
pub fn click_selection(text: &str, offset: usize, count: usize) -> Range<usize> {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    if count >= 4 {
        return 0..text.len();
    }
    if count == 3 {
        let start = text[..offset].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let end = text[offset..]
            .find('\n')
            .map(|p| offset + p)
            .unwrap_or(text.len());
        return start..end;
    }
    let hit = if offset == text.len() && offset > 0 {
        text[..offset]
            .char_indices()
            .last()
            .map(|(p, _)| p)
            .unwrap_or(0)
    } else {
        offset
    };
    text.split_word_bound_indices()
        .find(|(p, word)| *p <= hit && hit < *p + word.len())
        .map(|(p, word)| p..p + word.len())
        .unwrap_or(offset..offset)
}
