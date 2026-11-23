use super::*;

pub(super) struct EditSession {
    revision: Option<String>,
    source: String,
    dirty: bool,
    dirty_editors: Rc<Cell<usize>>,
    save_task: Option<Task<()>>,
}

impl EditSession {
    pub(super) fn new(note: Option<&Note>, dirty_editors: Rc<Cell<usize>>) -> Self {
        Self {
            revision: note.map(|note| note.revision.clone()),
            source: note.map(Note::editor_markdown).unwrap_or_default(),
            dirty: false,
            dirty_editors,
            save_task: None,
        }
    }

    pub(super) fn observe(&mut self, note: &Note) {
        self.revision = Some(note.revision.clone());
        self.source = note.editor_markdown();
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub(super) fn set_dirty(&mut self, dirty: bool) {
        if dirty != self.dirty {
            let count = self.dirty_editors.get();
            self.dirty_editors
                .set(if dirty { count + 1 } else { count - 1 });
            self.dirty = dirty;
        }
    }
}

impl Drop for EditSession {
    fn drop(&mut self) {
        self.set_dirty(false);
    }
}

impl View {
    pub(super) fn has_unsaved_edits(&self, cx: &App) -> bool {
        self.edit.is_dirty() || self.editor.read(cx).is_composing()
    }

    pub(super) fn has_external_revision(&self) -> bool {
        let Screen::Note(id) = &self.screen else {
            return false;
        };
        let runtime = self.shared.borrow();
        runtime
            .store
            .notes
            .iter()
            .find(|n| &n.id == id)
            .is_some_and(|n| self.edit.revision.as_ref() != Some(&n.revision))
    }

    pub(super) fn refresh_editor(&mut self, cx: &mut Context<Self>) -> bool {
        if self.edit.is_dirty() || self.editor.read(cx).is_composing() {
            return false;
        }
        if !self.has_external_revision() {
            return true;
        }
        let Some(note) = self.current_note() else {
            return false;
        };
        let source = note.editor_markdown();
        if !self
            .editor
            .update(cx, |editor, cx| editor.sync_external(source, cx))
        {
            return false;
        }
        self.edit.observe(&note);
        true
    }

    pub(super) fn queue_save(&mut self, cx: &mut Context<Self>) {
        self.edit.set_dirty(true);
        self.edit.save_task = Some(cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(350))
                .await;
            let _ = view.update(cx, |view, cx| view.flush(cx));
        }));
    }

    pub(super) fn flush(&mut self, cx: &mut Context<Self>) {
        if !self.edit.is_dirty() || self.editor.read(cx).is_composing() {
            return;
        }
        let Some(note) = self.current_note() else {
            return;
        };
        let text = self.editor.read(cx).value().to_string();
        if note.editor_markdown() == text {
            self.edit.observe(&note);
            self.edit.set_dirty(false);
            return;
        }
        let result = self.shared.borrow_mut().store.save_editor(
            &note.id,
            self.edit.revision.as_deref().unwrap_or(&note.revision),
            &self.edit.source,
            &text,
        );
        if let Ok(saved) = &result {
            // Feed merged changes back while retaining trailing paragraphs in
            // the live editor when the only difference is normalization.
            let mut local = note;
            local.set_editor_markdown(&text);
            let source = saved.editor_markdown();
            if source != local.editor_markdown() {
                self.editor
                    .update(cx, |editor, cx| editor.sync_external(source, cx));
            }
            self.edit.observe(saved);
            self.edit.set_dirty(false);
        }
        self.result(result.map(|_| ()));
        self.shared.borrow_mut().generation += 1;
    }
}
