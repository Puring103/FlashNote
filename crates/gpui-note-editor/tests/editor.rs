use gpui_note_editor::document::*;
fn state(md: &str) -> EditorState {
    EditorState::new(Document::from_markdown(md))
}
#[test]
fn external_revision_is_deferred_until_composition_finishes() {
    let mut editor = state("正文");
    editor.begin_composition(0..0, "拼").unwrap();
    let external = Document::from_markdown("外部更新");
    assert!(!editor.sync_external(external.clone()));
    assert_eq!(editor.document.text(), "拼正文");
    assert!(editor.composition_range().is_some());
    editor.commit_composition();
    assert!(editor.sync_external(external));
    assert_eq!(editor.document.text(), "外部更新");
    assert!(editor.composition_range().is_none());
}
fn type_text(s: &mut EditorState, text: &str) {
    for c in text.chars() {
        let r = s.selection.range();
        s.replace(r, &c.to_string(), true).unwrap();
    }
}
#[test]
fn edits_visible_text_without_hidden_delimiters() {
    let mut s = state("**你好** 世界");
    assert_eq!(s.document.text(), "你好 世界");
    s.select(3, 6).unwrap();
    s.replace(3..6, "们", false).unwrap();
    assert_eq!(s.document.text(), "你们 世界");
    assert!(s.document.blocks[0].content[0].marks.bold);
    assert_eq!(s.document.to_markdown(), "**你们** 世界");
}
#[test]
fn deleting_complete_marked_text_leaves_no_syntax() {
    let mut s = state("**中文**");
    s.select(0, 6).unwrap();
    s.delete(true).unwrap();
    assert_eq!(s.document.text(), "");
    assert_eq!(s.document.to_markdown(), "");
    assert!(s.undo());
    assert_eq!(s.document.to_markdown(), "**中文**");
}
#[test]
fn typing_markdown_converts_and_continues_outside_mark() {
    let mut s = state("");
    type_text(&mut s, "**加粗**普通");
    assert_eq!(s.document.text(), "加粗普通");
    assert!(s.document.blocks[0].content[0].marks.bold);
    assert!(!s.document.blocks[0].content.last().unwrap().marks.bold);
}
#[test]
fn heading_input_rule_and_enter() {
    let mut s = state("");
    type_text(&mut s, "## 标题");
    assert_eq!(s.document.text(), "标题");
    assert_eq!(s.document.blocks[0].kind, BlockKind::Heading(2));
    s.enter().unwrap();
    type_text(&mut s, "正文");
    assert_eq!(s.document.blocks[1].kind, BlockKind::Paragraph);
    assert_eq!(s.document.text(), "标题\n正文");
}
#[test]
fn todo_continue_toggle_exit_undo() {
    let mut s = state("");
    type_text(&mut s, "- [ ] 买菜");
    assert_eq!(s.document.text(), "买菜");
    assert!(matches!(
        s.document.blocks[0].kind,
        BlockKind::Task { checked: false, .. }
    ));
    s.toggle_task(0).unwrap();
    s.enter().unwrap();
    assert!(matches!(
        s.document.blocks[1].kind,
        BlockKind::Task { checked: false, .. }
    ));
    s.enter().unwrap();
    assert_eq!(s.document.blocks[1].kind, BlockKind::Paragraph);
    assert!(s.undo());
    assert!(matches!(s.document.blocks[1].kind, BlockKind::Task { .. }));
}
#[test]
fn delete_grapheme_and_merge_paragraphs() {
    let mut s = state("🙂👨‍👩‍👧‍👦\n\n第二段");
    s.select(29, 29).unwrap();
    s.delete(true).unwrap();
    assert_eq!(s.document.text(), "🙂\n第二段");
    s.select(5, 5).unwrap();
    s.delete(true).unwrap();
    assert_eq!(s.document.text(), "🙂第二段");
}
#[test]
fn toggle_marks_and_keep_caret() {
    let mut s = state("hello");
    s.select(5, 5).unwrap();
    s.toggle_mark(MarkKind::Bold).unwrap();
    s.replace(5..5, " world", false).unwrap();
    assert_eq!(s.selection.head, 11);
    assert!(s.document.blocks[0].content.last().unwrap().marks.bold);
    s.toggle_mark(MarkKind::Bold).unwrap();
    s.replace(11..11, "!", false).unwrap();
    assert!(!s.document.blocks[0].content.last().unwrap().marks.bold);
}
#[test]
fn composition_updates_are_one_undo_unit() {
    let mut s = state("**前**");
    s.select(3, 3).unwrap();
    s.begin_composition(3..3, "n").unwrap();
    s.begin_composition(3..4, "ni").unwrap();
    s.begin_composition(3..5, "你好🙂").unwrap();
    s.commit_composition();
    assert_eq!(s.document.text(), "前你好🙂");
    assert!(s.document.blocks[0].content[0].marks.bold);
    s.undo();
    assert_eq!(s.document.text(), "前");
    s.redo();
    assert_eq!(s.document.text(), "前你好🙂");
}
#[test]
fn transaction_rolls_back_all_steps_on_invalid_range() {
    let mut s = state("text");
    let before = s.snapshot();
    let result = s.apply(Transaction {
        steps: vec![
            Step::Replace {
                range: 0..0,
                text: "new".into(),
                marks: Marks::default(),
            },
            Step::Replace {
                range: 99..100,
                text: "bad".into(),
                marks: Marks::default(),
            },
        ],
        selection: None,
    });
    assert!(result.is_err());
    assert_eq!(s.snapshot(), before);
    assert!(!s.undo());
}
#[test]
fn reversed_selection_maps_through_insert() {
    let mut s = state("abcd");
    s.select(4, 1).unwrap();
    s.apply(Transaction {
        steps: vec![Step::Replace {
            range: 0..0,
            text: "中".into(),
            marks: Marks::default(),
        }],
        selection: None,
    })
    .unwrap();
    assert_eq!(s.selection.anchor, 7);
    assert_eq!(s.selection.head, 4);
    s.undo();
    assert_eq!(s.selection.anchor, 4);
    assert_eq!(s.selection.head, 1);
}
#[test]
fn markdown_roundtrip_preserves_marks_and_literal_syntax() {
    for md in [
        "# 标题\n\n普通 **加粗**、_斜体_、`代码`、[链接](https://example.com)",
        "- [ ] 第一项\n- [x] 第二项",
        "```rust\nlet x = 1;\n```",
        r"\*字面星号\* 和 \[括号\]",
    ] {
        let doc = Document::from_markdown(md);
        let output = doc.to_markdown();
        assert_eq!(Document::from_markdown(&output), doc, "{output}");
    }
}
#[test]
fn mixed_marks_and_whitespace_roundtrip() {
    let mut s = state("hello");
    s.select(5, 5).unwrap();
    s.toggle_mark(MarkKind::Bold).unwrap();
    s.replace(5..5, " world ", false).unwrap();
    s.toggle_mark(MarkKind::Italic).unwrap();
    s.replace(12..12, "both", false).unwrap();
    let md = s.document.to_markdown();
    let restored = Document::from_markdown(&md);
    assert_eq!(restored.text(), s.document.text(), "{md}");
    assert!(
        restored.blocks[0]
            .content
            .iter()
            .any(|s| s.text.contains("world") && s.marks.bold),
        "{md}"
    );
}
#[test]
fn markdown_link_insertion_keeps_structured_link() {
    let mut s = state("before:");
    s.select(7, 7).unwrap();
    s.replace_markdown(7..7, "[网站](https://example.com)")
        .unwrap();
    assert_eq!(s.document.text(), "before:网站");
    assert_eq!(
        s.document.blocks[0]
            .content
            .last()
            .unwrap()
            .marks
            .link
            .as_deref(),
        Some("https://example.com")
    );
}
#[test]
fn invalid_utf8_edit_is_rejected() {
    let mut s = state("中文");
    let before = s.snapshot();
    assert!(s.replace(1..2, "x", false).is_err());
    assert_eq!(s.snapshot(), before);
}
#[test]
fn json_roundtrip_is_independent_of_markdown() {
    let doc = Document::from_markdown("# 标题\n\n- [x] **完成**");
    let json = serde_json::to_string(&doc).unwrap();
    assert_eq!(serde_json::from_str::<Document>(&json).unwrap(), doc);
}
#[test]
fn nested_lists_do_not_add_phantom_paragraphs() {
    let doc = Document::from_markdown("- parent\n  - child\n- other");
    assert_eq!(doc.text(), "parent\nchild\nother");
    assert_eq!(
        Document::from_markdown(&doc.to_markdown()).text(),
        doc.text()
    );
}
#[test]
fn image_reference_survives_text_edits() {
    let mut s = state("before ![草图](attachments/image.png) after");
    s.select(0, 0).unwrap();
    s.replace(0..0, "new ", false).unwrap();
    let md = s.document.to_markdown();
    assert!(md.contains("![草图](attachments/image.png)"), "{md}");
    let restored = Document::from_markdown(&md);
    assert_eq!(restored.text(), s.document.text());
    assert!(restored.blocks[0].content.iter().any(|s| s.marks.image));
}
#[test]
fn inserting_document_slice_preserves_block_kinds() {
    let mut s = state("prefix suffix");
    s.select(7, 7).unwrap();
    s.replace_markdown(7..7, "## heading\n\n- [ ] item")
        .unwrap();
    assert!(matches!(s.document.blocks[0].kind, BlockKind::Heading(2)));
    assert!(matches!(
        s.document.blocks[1].kind,
        BlockKind::Task { checked: false, .. }
    ));
    assert_eq!(s.document.text(), "prefix heading\nitemsuffix");
    assert!(s.undo());
    assert_eq!(s.document.text(), "prefix suffix");
}

#[test]
fn toggling_selection_at_span_boundary_removes_existing_mark() {
    let mut s = state("plain **bold**");
    s.select(6, 10).unwrap();
    s.toggle_mark(MarkKind::Bold).unwrap();
    assert_eq!(s.document.to_markdown(), "plain bold");
    s.select(0, 10).unwrap();
    s.toggle_mark(MarkKind::Bold).unwrap();
    assert!(
        s.document.blocks[0]
            .content
            .iter()
            .all(|span| span.marks.bold)
    );
}

#[test]
fn ordered_input_continues_and_exits_on_empty_enter() {
    for prefix in ["1. ", "3) "] {
        let mut s = state("");
        type_text(&mut s, prefix);
        let number = if prefix.starts_with('1') { 1 } else { 3 };
        assert_eq!(
            s.document.blocks[0].kind,
            BlockKind::Ordered { number, depth: 0 }
        );
        type_text(&mut s, "第一项");
        s.enter().unwrap();
        assert_eq!(
            s.document.blocks[1].kind,
            BlockKind::Ordered {
                number: number + 1,
                depth: 0
            }
        );
        s.enter().unwrap();
        assert_eq!(s.document.blocks[1].kind, BlockKind::Paragraph);
        assert!(s.undo());
        assert!(matches!(
            s.document.blocks[1].kind,
            BlockKind::Ordered { .. }
        ));
    }
}

#[test]
fn tab_and_shift_tab_change_list_depth_without_changing_text() {
    let mut s = state("1. first\n2. second\n3. third");
    s.select(6, 12).unwrap();
    let selection = s.selection.clone();
    s.indent(false).unwrap();
    assert_eq!(s.document.text(), "first\nsecond\nthird");
    let reloaded = Document::from_markdown(&s.document.to_markdown());
    assert_eq!(reloaded.text(), s.document.text());
    assert!(matches!(
        reloaded.blocks[1].kind,
        BlockKind::Ordered { depth: 1, .. }
    ));
    assert_eq!(s.selection, selection);
    assert_eq!(
        s.document.blocks[0].kind,
        BlockKind::Ordered {
            number: 1,
            depth: 0
        }
    );
    assert_eq!(
        s.document.blocks[1].kind,
        BlockKind::Ordered {
            number: 1,
            depth: 1
        }
    );
    assert_eq!(
        s.document.blocks[2].kind,
        BlockKind::Ordered {
            number: 2,
            depth: 0
        }
    );
    s.indent(true).unwrap();
    assert_eq!(
        s.document.blocks[1].kind,
        BlockKind::Ordered {
            number: 2,
            depth: 0
        }
    );
    s.undo();
    assert!(matches!(
        s.document.blocks[1].kind,
        BlockKind::Ordered { depth: 1, .. }
    ));
}

#[test]
fn tab_indents_selected_paragraphs_and_preserves_reversed_selection() {
    let mut s = state("alpha\n\nbeta");
    s.select(10, 0).unwrap();
    s.indent(false).unwrap();
    assert_eq!(s.document.text(), "    alpha\n    beta");
    assert_eq!(
        Document::from_markdown(&s.document.to_markdown()).text(),
        s.document.text()
    );
    assert!(s.selection.anchor > s.selection.head);
    s.indent(true).unwrap();
    assert_eq!(s.document.text(), "alpha\nbeta");
    assert_eq!(s.selection.anchor, 10);
    assert_eq!(s.selection.head, 0);
    s.undo();
    assert_eq!(s.document.text(), "    alpha\n    beta");
}

#[test]
fn shift_tab_exits_first_level_task_but_keeps_content() {
    let mut s = state("- [x] 完成");
    s.indent(true).unwrap();
    assert_eq!(s.document.text(), "完成");
    assert_eq!(s.document.blocks[0].kind, BlockKind::Paragraph);
    s.undo();
    assert_eq!(
        s.document.blocks[0].kind,
        BlockKind::Task {
            checked: true,
            depth: 0
        }
    );
}

#[test]
fn markdown_input_rules_do_not_transform_code_contents() {
    let mut s = state("```text\n\n```");
    type_text(&mut s, "1. **literal**");
    assert_eq!(s.document.text(), "1. **literal**");
    assert!(matches!(s.document.blocks[0].kind, BlockKind::Code { .. }));
}

#[test]
fn first_list_item_does_not_create_an_orphan_indented_list() {
    let mut s = state("1. first\n2. second");
    s.indent(false).unwrap();
    assert_eq!(
        s.document.blocks[0].kind,
        BlockKind::Ordered {
            number: 1,
            depth: 0
        }
    );
    assert_eq!(
        Document::from_markdown(&s.document.to_markdown()).text(),
        s.document.text()
    );
}
#[test]
fn media_insertion_preserves_both_sides_and_roundtrips() {
    let mut s = state("前面后面");
    s.select(6, 6).unwrap();
    s.insert_attachment("recording-1".into(), "audio".into(), "录音".into())
        .unwrap();
    assert_eq!(s.document.blocks.len(), 3);
    assert_eq!(s.document.blocks[0].text(), "前面");
    assert_eq!(s.document.blocks[2].text(), "后面");
    assert!(
        matches!(&s.document.blocks[1].kind, BlockKind::Attachment {id,..} if id == "recording-1")
    );
    let reloaded = Document::from_markdown(&s.document.to_markdown());
    assert_eq!(reloaded.text(), s.document.text());
    assert!(matches!(
        reloaded.blocks[1].kind,
        BlockKind::Attachment { .. }
    ));
    type_text(&mut s, "继续");
    assert_eq!(s.document.blocks[2].text(), "继续后面");
    assert!(s.undo());
}
#[test]
fn deleting_media_removes_the_atomic_node() {
    let mut s = state("之前\n\n[录音](attachment:abc?kind=audio)\n\n之后");
    let start = s.document.block_start(1);
    s.select(start, start + "\u{fffc}".len()).unwrap();
    s.delete(true).unwrap();
    assert!(
        !s.document
            .blocks
            .iter()
            .any(|b| matches!(b.kind, BlockKind::Attachment { .. }))
    );
    assert!(s.undo());
    assert!(
        s.document
            .blocks
            .iter()
            .any(|b| matches!(b.kind, BlockKind::Attachment { .. }))
    );
}

#[test]
fn media_indent_roundtrips_and_reduces_without_changing_identity() {
    let mut s = state("前文\n\n[素材](attachment:abc?kind=audio&depth=1)\n\n后文");
    let start = s.document.block_start(1);
    s.select(start, start).unwrap();
    s.indent(false).unwrap();
    assert!(
        matches!(&s.document.blocks[1].kind, BlockKind::Attachment {id, depth: 2, ..} if id == "abc")
    );
    let doc = Document::from_markdown(&s.document.to_markdown());
    assert!(matches!(
        doc.blocks[1].kind,
        BlockKind::Attachment { depth: 2, .. }
    ));
    s.indent(true).unwrap();
    assert!(matches!(
        s.document.blocks[1].kind,
        BlockKind::Attachment { depth: 1, .. }
    ));
}

#[test]
fn trailing_audio_has_editable_paragraph_after_reload() {
    let mut s = state("前文\n\n[录音](attachment:audio-1?kind=audio)");
    assert!(matches!(
        s.document.blocks.last().unwrap().kind,
        BlockKind::Paragraph
    ));
    let end = s.document.len();
    s.select(end, end).unwrap();
    type_text(&mut s, "音频之后");
    assert_eq!(s.document.blocks.last().unwrap().text(), "音频之后");
    assert!(matches!(
        s.document.blocks[1].kind,
        BlockKind::Attachment { .. }
    ));
}
#[test]
fn multiple_clicks_expand_from_word_to_paragraph_to_document() {
    let text = "hello world\n第二段";
    assert_eq!(&text[click_selection(text, 7, 2)], "world");
    assert_eq!(&text[click_selection(text, 6, 2)], "world");
    assert_eq!(&text[click_selection(text, 7, 3)], "hello world");
    assert_eq!(&text[click_selection(text, 7, 4)], text);
    let chinese = click_selection(text, 13, 2);
    assert!(text.is_char_boundary(chinese.start) && text.is_char_boundary(chinese.end));
}
