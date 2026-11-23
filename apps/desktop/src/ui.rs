mod edit_session;
mod features;
use self::edit_session::EditSession;
use crate::{audio, icons::icon, model::*, store::Store, sync};
use gpui::{prelude::*, *};
use gpui_component::{
    Root, WindowExt,
    button::{Button, ButtonVariant, ButtonVariants},
    checkbox::Checkbox,
    dialog::DialogButtonProps,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
    text::TextView,
};
use gpui_note_editor::{Changed, RichEditor};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const PAPER: u32 = 0xf7f3ea;
const INK: u32 = 0x262723;
const TEAL: u32 = 0x245356;
const LINE: u32 = 0xd9d5cb;
const CARD: u32 = 0xfbf8f1;
const MUTED: u32 = 0x68645e;
const SYNC_DEPLOYMENT_GUIDE: &str = "https://github.com/Puring103/FlashNote#%E8%87%AA%E5%BB%BA%E5%90%8C%E6%AD%A5%E6%9C%8D%E5%8A%A1%E5%99%A8";
pub type Shared = Rc<RefCell<Runtime>>;
pub struct Runtime {
    #[cfg(windows)]
    pub hotkey_requests: Option<mpsc::Sender<crate::platform::HotkeyRequest>>,
    pub store: Store,
    pub generation: usize,
    pub windows: HashMap<String, AnyWindowHandle>,
    pub images: HashMap<String, Arc<Image>>,
    pub image_errors: HashMap<String, String>,
    image_loads: HashSet<String>,
    image_access: HashMap<String, Instant>,
    pub waveforms: HashMap<String, Vec<f32>>,
    pub status: String,
    job_tx: mpsc::Sender<Job>,
    job_rx: mpsc::Receiver<Job>,
    playing_asset: Option<String>,
    playback_stop: Option<mpsc::Sender<()>>,
    sync_enabled: bool,
    sync_busy: bool,
    sync_last: Instant,
    sync_result: Option<Result<sync::Cycle, String>>,
    dirty_editors: Rc<Cell<usize>>,
    editors: Vec<WeakEntity<View>>,
    sync_more: bool,
    sync_status: String,
    sync_success: Option<i64>,
    sync_usage: Option<sync::Usage>,
    image_jobs: HashSet<String>,
    image_scan_at: Instant,
    image_retry: HashMap<String, Instant>,
    model_downloads: HashMap<String, u8>,
    model_errors: HashMap<String, String>,
    account_busy: bool,
    account_error: Option<String>,
    account_cancel: Arc<AtomicBool>,
    transcript_jobs: HashMap<String, String>,
}
impl Runtime {
    #[cfg(windows)]
    pub fn has_pending_editor_edits(&self, cx: &App) -> bool {
        self.editors
            .iter()
            .filter_map(WeakEntity::upgrade)
            .any(|editor| editor.read(cx).has_unsaved_edits(cx))
    }

    pub fn new(store: Store) -> Self {
        let (job_tx, job_rx) = mpsc::channel();
        let configured = sync::config(&store);
        let sync_enabled = matches!(&configured, Ok(Some(_)))
            && std::env::var("SUOJIAN_SYNC_DISABLED").as_deref() != Ok("1");
        let sync_status = match configured {
            Ok(Some(_)) => "等待同步".into(),
            Ok(None) => "尚未连接同步服务".into(),
            Err(_) => "同步配置无法读取，请检查本机密钥与配置文件".into(),
        };
        Self {
            #[cfg(windows)]
            hotkey_requests: None,
            store,
            generation: 0,
            windows: HashMap::new(),
            images: HashMap::new(),
            waveforms: HashMap::new(),
            image_errors: HashMap::new(),
            image_loads: HashSet::new(),
            image_access: HashMap::new(),
            status: String::new(),
            job_tx,
            job_rx,
            playing_asset: None,
            playback_stop: None,
            sync_enabled,
            sync_busy: false,
            sync_last: Instant::now() - Duration::from_secs(10),
            sync_result: None,
            dirty_editors: Rc::new(Cell::new(0)),
            editors: Vec::new(),
            sync_more: false,
            sync_status,
            sync_success: None,
            sync_usage: None,
            image_jobs: HashSet::new(),
            image_scan_at: Instant::now(),
            image_retry: HashMap::new(),
            model_downloads: HashMap::new(),
            model_errors: HashMap::new(),
            account_busy: false,
            account_error: None,
            account_cancel: Arc::new(AtomicBool::new(false)),
            transcript_jobs: HashMap::new(),
        }
    }
    pub fn drain_jobs(&mut self) {
        while let Ok(job) = self.job_rx.try_recv() {
            let result = match job {
                Job::Account(result) => {
                    self.account_busy = false;
                    let result = if self.account_cancel.load(Ordering::Relaxed) {
                        if let Ok(connection) = result {
                            std::thread::spawn(move || {
                                let _ = sync::revoke_session(&connection.config);
                            });
                        }
                        Err("登录已取消".into())
                    } else {
                        result
                    };
                    match result
                        .map_err(anyhow::Error::msg)
                        .and_then(|connection| sync::connect_account(&mut self.store, connection))
                    {
                        Ok(()) => {
                            self.account_error = None;
                            self.sync_enabled = true;
                            self.sync_more = false;
                            self.sync_status = "等待同步".into();
                            self.sync_last = Instant::now() - Duration::from_secs(10);
                        }
                        Err(error) => self.account_error = Some(error.to_string()),
                    }
                    Ok(())
                }
                Job::LoggedOut(result) => {
                    if let Err(error) = result {
                        self.account_error =
                            Some(format!("本机已退出；服务端会话撤销失败：{error}"));
                    }
                    Ok(())
                }
                Job::ModelProgress(model, percent) => {
                    self.model_downloads.insert(model, percent);
                    Ok(())
                }
                Job::ModelDownloaded(model, path, result) => {
                    self.model_downloads.remove(&model);
                    match result {
                        Ok(()) => {
                            self.model_errors.remove(&model);
                            if self.store.settings.speech_model == model {
                                let mut settings = self.store.settings.clone();
                                settings.model_path = path.to_string_lossy().into_owned();
                                self.store.save_settings(settings)
                            } else {
                                Ok(())
                            }
                        }
                        Err(error) => {
                            self.model_errors.insert(model, error);
                            Ok(())
                        }
                    }
                }
                Job::RemoteImage(nid, bid, url, result) => {
                    self.image_jobs.remove(&bid);
                    self.image_retry.insert(bid.clone(), Instant::now());
                    let result = result.and_then(|p| {
                        self.store
                            .complete_remote_image(&nid, &bid, &url, p)
                            .map_err(|e| e.to_string())
                    });
                    self.store
                        .set_remote_image_error(&bid, result.as_ref().err().map(String::as_str))
                }
                Job::Synced(result) => {
                    self.sync_result = Some(result);
                    Ok(())
                }
                Job::ImageLoaded(asset, result) => {
                    self.image_loads.remove(&asset);
                    match result {
                        Ok(bytes) => {
                            self.image_access.insert(asset.clone(), Instant::now());
                            self.images.insert(
                                asset,
                                Arc::new(Image::from_bytes(ImageFormat::Png, bytes)),
                            );
                            // Recently displayed images form the active working
                            // set. Evict cold entries individually, never flush
                            // images that are still being rendered together.
                            let mut cold: Vec<_> = self
                                .image_access
                                .iter()
                                .filter(|(_, accessed)| accessed.elapsed() > Duration::from_secs(5))
                                .map(|(id, accessed)| (id.clone(), *accessed))
                                .collect();
                            cold.sort_by_key(|(_, accessed)| *accessed);
                            let mut bytes: usize =
                                self.images.values().map(|image| image.bytes.len()).sum();
                            for (id, _) in cold {
                                if bytes <= 32 * 1024 * 1024 {
                                    break;
                                }
                                if let Some(image) = self.images.remove(&id) {
                                    bytes -= image.bytes.len();
                                }
                                self.image_access.remove(&id);
                            }
                        }
                        Err(error) => {
                            self.image_errors.insert(asset, error);
                        }
                    }
                    Ok(())
                }
                Job::Attachment(nid, block) => {
                    if let Some(mut n) = self.store.notes.iter().find(|n| n.id == nid).cloned() {
                        let changed = if let Some(existing) = n
                            .blocks
                            .iter_mut()
                            .chain(n.media_archive.iter_mut())
                            .find(|b| b.id() == block.id())
                        {
                            if *existing != block {
                                *existing = block;
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        };
                        if changed {
                            self.store.save_note(n)
                        } else {
                            Ok(())
                        }
                    } else {
                        Ok(())
                    }
                }
                Job::Transcript(nid, bid, operation, result) => {
                    if self.transcript_jobs.get(&bid) == Some(&operation) {
                        self.transcript_jobs.remove(&bid);
                        self.store
                            .complete_transcript(&nid, &bid, result)
                            .map(|_| ())
                    } else {
                        Ok(())
                    }
                }
                Job::Played(asset, result) => {
                    if self.playing_asset.as_ref() == Some(&asset) {
                        self.playing_asset = None;
                        self.playback_stop = None;
                    }
                    result.map_err(anyhow::Error::msg)
                }
            };
            if let Err(e) = result {
                self.status = e.to_string();
            }
            self.generation += 1;
        }
        if let Some(result) = self.sync_result.take() {
            self.sync_busy = false;
            match result
                .and_then(|cycle| sync::apply(&mut self.store, cycle).map_err(|e| e.to_string()))
            {
                Ok(report) => {
                    self.sync_success = Some(now());
                    self.sync_usage = Some(report.usage);
                    self.sync_status = if report.conflicts > 0 {
                        self.status = format!("已同步，{} 处离线冲突已保留副本", report.conflicts);
                        self.status.clone()
                    } else if report.more {
                        "正在同步剩余变更".into()
                    } else {
                        "已同步".into()
                    };
                    self.sync_more = report.more;
                    if report.more || report.merged > 0 || report.conflicts > 0 {
                        self.sync_last = Instant::now() - Duration::from_secs(10);
                    }
                }
                Err(error) => self.sync_status = format!("暂未同步，本机修改已保留：{error}"),
            }
            self.generation += 1;
        }
        // Pending blocks survive restarts; retry bookkeeping never creates note revisions.
        let current_time = Instant::now();
        if self.image_jobs.len() < 2 && current_time >= self.image_scan_at {
            self.image_scan_at = current_time + Duration::from_millis(250);
            let pending = self
                .store
                .notes
                .iter()
                .filter(|n| n.deleted_at.is_none())
                .flat_map(|n| {
                    n.blocks.iter().filter_map(|b| {
                        if let Block::RemoteImage { id, url, .. } = b {
                            Some((n.id.clone(), id.clone(), url.clone()))
                        } else {
                            None
                        }
                    })
                })
                .find(|(_, bid, _)| {
                    !self.image_jobs.contains(bid)
                        && self
                            .image_retry
                            .get(bid)
                            .is_none_or(|t| t.elapsed() >= Duration::from_secs(30))
                });
            if let Some((nid, bid, url)) = pending {
                self.image_jobs.insert(bid.clone());
                let tx = self.job_tx.clone();
                std::thread::spawn(move || {
                    let result = suojian::remote_images::download(&url)
                        .map_err(|_| "下载失败，请检查网络或图片链接，稍后自动重试".to_string());
                    let _ = tx.send(Job::RemoteImage(nid, bid, url, result));
                });
            }
        }
        if self.sync_enabled
            && !self.sync_busy
            && self.sync_last.elapsed() >= Duration::from_secs(5)
        {
            self.sync_last = Instant::now();
            let plan = if self.sync_more {
                sync::prepare_pull(&mut self.store)
            } else {
                sync::prepare(&mut self.store)
            };
            match plan {
                Ok(Some(plan)) => {
                    self.sync_busy = true;
                    let tx = self.job_tx.clone();
                    std::thread::spawn(move || {
                        let result = sync::transfer(plan).map_err(|e| e.to_string());
                        let _ = tx.send(Job::Synced(result));
                    });
                }
                Ok(None) => {
                    self.sync_enabled = false;
                    self.sync_more = false;
                }
                Err(error) => {
                    self.sync_status = format!("同步准备失败：{error}");
                    self.generation += 1;
                }
            }
        }
    }
}
#[derive(Clone)]
pub enum Screen {
    Center,
    Note(String),
    Settings,
}
enum Job {
    Account(Result<sync::Connection, String>),
    LoggedOut(Result<(), String>),
    ModelProgress(String, u8),
    ModelDownloaded(String, std::path::PathBuf, Result<(), String>),
    RemoteImage(
        String,
        String,
        String,
        Result<crate::media::PreparedAttachment, String>,
    ),
    Synced(Result<sync::Cycle, String>),
    Attachment(String, Block),
    ImageLoaded(String, Result<Vec<u8>, String>),
    Transcript(String, String, String, Result<String, String>),
    Played(String, Result<(), String>),
}

pub struct View {
    page_focus: FocusHandle,
    embedded: bool,
    toolbar_fraction: f32,
    toolbar_frame: Instant,
    card_selected: bool,
    selected_note: Option<String>,
    multi: bool,
    selection: HashSet<String>,
    selection_anchor: Option<String>,
    selection_filter: String,
    settings_tab: usize,
    archive_busy: bool,
    include_trash: bool,
    storage_bytes: Option<(u64, u64)>,
    batch_undo: Option<(Vec<Note>, Vec<(String, String)>)>,
    feed_scroll: ScrollHandle,
    content_width: f32,
    edit: EditSession,
    cards: HashMap<String, Entity<View>>,
    shared: Shared,
    screen: Screen,
    search: Entity<InputState>,
    editor: Entity<RichEditor>,
    group_name: Entity<InputState>,
    group: Option<String>,
    expanded: HashSet<String>,
    settings: Settings,
    subscriptions: Vec<Subscription>,
    tick_task: Option<Task<()>>,
    generation: usize,
    tx: mpsc::Sender<Job>,
    recorder: Option<audio::Recorder>,
    record_started: Option<Instant>,
    recording_block: Option<String>,
    saving_audio: bool,
    #[cfg(windows)]
    pinned: bool,
    trash: bool,
    transcript_edit: Option<(String, Entity<InputState>)>,
    prompted_conflict: Option<String>,
}

fn fitting_groups(widths: &[f32], available: f32) -> usize {
    let total: f32 = widths.iter().map(|w| w + 8.).sum();
    if total <= available {
        return widths.len();
    }
    let budget = (available - 76.).max(0.);
    let mut used = 0.;
    widths
        .iter()
        .take_while(|w| {
            used += **w + 8.;
            used <= budget
        })
        .count()
}

fn settings_heading(title: &'static str) -> Div {
    div()
        .pt(px(14.))
        .text_size(px(16.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(title)
}

fn button(id: impl Into<ElementId>, name: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(32.))
        .flex_shrink_0()
        .rounded(px(4.))
        .cursor_pointer()
        .text_color(rgb(INK))
        .hover(move |s| s.bg(rgb(if name == "close" { 0xf0ddd5 } else { 0xe5e8df })))
        .child(
            icon(name)
                .size(px(if matches!(name, "minus" | "maximize" | "close") {
                    15.
                } else {
                    18.
                }))
                .when(name == "trash", |s| s.text_color(rgb(0xb4473b)))
                .when(name == "play" || name == "pause" || name == "stop", |s| {
                    s.text_color(rgb(PAPER))
                }),
        )
}
fn label_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_2()
        .rounded(px(4.))
        .cursor_pointer()
        .text_sm()
        .hover(|s| s.bg(rgb(0xe5e8df)))
        .child(label.into())
}
fn toggle_row(id: &'static str, label: &'static str, checked: bool) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .px(px(18.))
        .py(px(15.))
        .cursor_pointer()
        .text_sm()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(label),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .w(px(34.))
                .h(px(20.))
                .rounded_full()
                .px(px(3.))
                .flex()
                .items_center()
                .when(checked, |d| d.justify_end())
                .bg(rgb(if checked { TEAL } else { 0xcacbc4 }))
                .child(div().size(px(14.)).rounded_full().bg(rgb(0xffffff))),
        )
}

impl View {
    pub fn new(
        shared: Shared,
        screen: Screen,
        embedded: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let note = match &screen {
            Screen::Note(id) => shared
                .borrow()
                .store
                .notes
                .iter()
                .find(|n| &n.id == id)
                .cloned(),
            _ => None,
        };
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索便签…"));
        let editor = cx.new(|cx| {
            RichEditor::new(
                note.as_ref()
                    .map(|n| n.editor_markdown())
                    .unwrap_or_default(),
                cx,
            )
        });
        let host = cx.entity().downgrade();
        let copy_runtime = shared.clone();
        editor.update(cx, |e, _| {
            e.set_host_paste(true);
            #[cfg(windows)]
            e.set_clipboard_html_writer(Rc::new(crate::platform::clipboard_set_html));
            e.set_media_copier(Rc::new(move |bid| {
                let runtime = copy_runtime.borrow();
                let block = runtime
                    .store
                    .notes
                    .iter()
                    .flat_map(|n| n.blocks.iter().chain(&n.media_archive))
                    .find(|b| b.id() == bid)?;
                if let Block::Image { asset, .. } = block {
                    let bytes = runtime.store.asset(asset).ok()?;
                    let png = crate::media::image_preview(&bytes).ok()?;
                    Some(ClipboardItem::new_image(&Image::from_bytes(
                        ImageFormat::Png,
                        png,
                    )))
                } else {
                    None
                }
            }));
            e.set_media_renderer(Rc::new(move |bid, w, cx| {
                host.update(cx, |v, cx| {
                    let Some(mut note) = v.current_note() else {
                        return div().into_any_element();
                    };
                    note.blocks.retain(|b| b.id() == bid);
                    note.media_depths.clear();
                    div()
                        .w_full()
                        .child(v.blocks(&note, false, w, cx))
                        .into_any_element()
                })
                .unwrap_or_else(|_| div().into_any_element())
            }))
        });
        let group_name = cx.new(|cx| InputState::new(window, cx).placeholder("新分组名称"));
        let mut subscriptions = vec![];
        subscriptions
            .push(cx.subscribe_in(&search, window, |_, _, _: &InputEvent, _, cx| cx.notify()));
        subscriptions.push(
            cx.subscribe_in(&editor, window, |this, _, _: &Changed, _, cx| {
                this.queue_save(cx);
                cx.notify();
            }),
        );
        let focus = editor.read(cx).focus_handle(cx);
        subscriptions.push(cx.on_focus(&focus, window, |_, _, cx| cx.notify()));
        subscriptions.push(cx.on_blur(&focus, window, |_, _, cx| cx.notify()));
        if !embedded && matches!(&screen, Screen::Center) {
            subscriptions.push(cx.observe_window_activation(window, |v, w, cx| {
                if !w.is_window_active() {
                    v.clear_note_selection(w, cx);
                }
            }));
        }
        let settings = shared.borrow().store.settings.clone();
        let initial_group =
            if matches!(&screen, Screen::Center) && !shared.borrow().store.root.ends_with("demo") {
                Some(DEFAULT_GROUP.to_string())
            } else {
                None
            };
        let tx = shared.borrow().job_tx.clone();
        let edit = EditSession::new(note.as_ref(), shared.borrow().dirty_editors.clone());
        if matches!(&screen, Screen::Note(_)) {
            let mut runtime = shared.borrow_mut();
            runtime.editors.retain(|editor| editor.upgrade().is_some());
            runtime.editors.push(cx.entity().downgrade());
        }
        let mut this = Self {
            page_focus: cx.focus_handle(),
            embedded,
            toolbar_fraction: 0.,
            toolbar_frame: Instant::now(),
            card_selected: false,
            selected_note: None,
            multi: false,
            selection: HashSet::new(),
            selection_anchor: None,
            selection_filter: String::new(),
            settings_tab: 0,
            archive_busy: false,
            include_trash: false,
            storage_bytes: None,
            batch_undo: None,
            feed_scroll: ScrollHandle::new(),
            content_width: 376.,
            edit,
            cards: HashMap::new(),
            shared,
            screen,
            search,
            editor,
            group_name,
            group: initial_group,
            expanded: HashSet::new(),
            settings,
            subscriptions,
            tick_task: None,
            generation: 0,
            tx,
            recorder: None,
            record_started: None,
            recording_block: None,
            saving_audio: false,
            #[cfg(windows)]
            pinned: false,
            trash: false,
            transcript_edit: None,
            prompted_conflict: None,
        };
        this.subscriptions.push(cx.on_app_quit(|v, cx| {
            v.shared.borrow_mut().drain_jobs();
            v.flush(cx);
            if let Some(recorder) = v.recorder.take() {
                recorder.stop();
                if let Ok(Ok((bytes, duration_ms))) =
                    recorder.result.recv_timeout(Duration::from_secs(2))
                {
                    if let Some(mut note) = v.current_note() {
                        let imported = v.shared.borrow().store.import_bytes(&bytes);
                        if let Ok(asset) = imported {
                            note.blocks.push(Block::Audio {
                                id: id(),
                                asset,
                                duration_ms,
                                text: None,
                                state: AudioState::Recorded,
                            });
                            let result = v.shared.borrow_mut().store.save_note(note);
                            v.result(result);
                        }
                    }
                }
            }
            async {}
        }));
        this.tick_task = Some(cx.spawn_in(window, async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(80))
                    .await;
                if view
                    .update_in(cx, |v, w, cx| {
                        v.tick(w, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        if !embedded && matches!(this.screen, Screen::Note(_)) {
            this.editor.update(cx, |s, cx| s.focus(window, cx));
        }
        this
    }
    fn close_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush(cx);
        if self.has_unsaved_edits(cx) {
            return;
        }
        if let Some(recorder) = &self.recorder {
            recorder.stop();
            self.saving_audio = true;
            self.shared.borrow_mut().status = "正在保存录音，完成后可关闭便签".into();
            cx.notify();
            return;
        }
        self.shared.borrow_mut().drain_jobs();
        window.remove_window();
    }
    fn current_note(&self) -> Option<Note> {
        if let Screen::Note(id) = &self.screen {
            self.shared
                .borrow()
                .store
                .notes
                .iter()
                .find(|n| &n.id == id)
                .cloned()
        } else {
            None
        }
    }
    fn result(&mut self, result: anyhow::Result<()>) {
        if let Err(e) = result {
            let mut runtime = self.shared.borrow_mut();
            runtime.status = e.to_string();
            runtime.generation += 1;
        }
    }
    fn clear_note_selection(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_note.take() else {
            return;
        };
        if let Some(card) = self.cards.get(&id) {
            card.update(cx, |v, cx| {
                v.card_selected = false;
                if v.editor.read(cx).is_focused(w) {
                    w.blur();
                }
                cx.notify();
            });
        }
        cx.notify();
    }

    fn tick(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.has_unsaved_edits(cx)
            && !w.has_active_dialog(cx)
            && self.editor.read(cx).is_focused(w)
        {
            let pair = self
                .current_note()
                .and_then(|n| self.shared.borrow().store.conflict_pair(&n.id));
            if let Some((fork, cloud)) = pair {
                let signature = format!("{}/{}", fork.revision, cloud.revision);
                if self.prompted_conflict.as_ref() != Some(&signature) {
                    self.prompted_conflict = Some(signature);
                    self.resolve_dialog(fork, cloud, w, cx);
                }
            }
        }
        let recording_result = self
            .recorder
            .as_ref()
            .and_then(|r| r.result.try_recv().ok());
        if let Some(result) = recording_result {
            self.recorder = None;
            self.record_started = None;
            self.saving_audio = false;
            match result {
                Err(e) => self.recording_failed(e, cx),
                Ok((bytes, duration_ms)) => {
                    if let Some(n) = self.current_note() {
                        let imported = self.shared.borrow().store.import_bytes(&bytes);
                        match imported {
                            Err(e) => self.recording_failed(e, cx),
                            Ok(asset) => {
                                let bid = self.recording_block.take().unwrap_or_else(id);
                                let settings =
                                    audio::resolve_speech(&self.shared.borrow().store.settings);
                                let enabled = settings.auto_transcribe
                                    && !settings.model_path.is_empty()
                                    && !settings.whisper_cli.is_empty();
                                let _ = self.tx.send(Job::Attachment(
                                    n.id.clone(),
                                    Block::Audio {
                                        id: bid.clone(),
                                        asset,
                                        duration_ms,
                                        text: None,
                                        state: if enabled {
                                            AudioState::Processing
                                        } else {
                                            AudioState::Recorded
                                        },
                                    },
                                ));
                                if settings.auto_transcribe && !enabled {
                                    self.shared.borrow_mut().status =
                                        "录音已保存；随程序提供的语音组件缺失".into();
                                }
                                if enabled {
                                    let operation = id();
                                    self.shared
                                        .borrow_mut()
                                        .transcript_jobs
                                        .insert(bid.clone(), operation.clone());
                                    let tx = self.tx.clone();
                                    let root = self.shared.borrow().store.root.join("scratch");
                                    std::thread::spawn(move || {
                                        let result = audio::transcribe(bytes, &settings, root)
                                            .map_err(|e| e.to_string());
                                        let _ =
                                            tx.send(Job::Transcript(n.id, bid, operation, result));
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        let gen_id = self.shared.borrow().generation;
        if self.generation != gen_id
            && self.group.as_ref().is_some_and(|id| {
                !self
                    .shared
                    .borrow()
                    .store
                    .groups
                    .iter()
                    .any(|group| group.id == *id)
            })
        {
            self.group = None;
        }
        let pending_revision = self.has_external_revision();
        if self.generation != gen_id || self.recorder.is_some() || pending_revision {
            if pending_revision {
                self.refresh_editor(cx);
            }
            self.generation = gen_id;
            cx.notify();
        }
    }
    pub fn new_note(&mut self, cx: &mut Context<Self>) {
        let n = Note::new();
        let nid = n.id.clone();
        let result = self.shared.borrow_mut().store.save_note(n);
        if result.is_err() {
            self.result(result);
            cx.notify();
            return;
        }
        self.shared.borrow_mut().generation += 1;
        open(self.shared.clone(), Screen::Note(nid), cx);
    }
    fn insert(&mut self, text: &'static str, w: &mut Window, cx: &mut Context<Self>) {
        let kind = if text.starts_with("**") {
            "bold"
        } else if text.starts_with('*') {
            "italic"
        } else if text.contains("[ ]") {
            "todo"
        } else if text.starts_with("\n-") {
            "list"
        } else {
            "link"
        };
        if kind != "link" {
            self.editor.update(cx, |s, cx| s.format(kind, w, cx));
            return;
        }
        let label = match kind {
            "bold" => "加粗文字",
            "italic" => "斜体文字",
            "todo" => "添加待办",
            "list" => "添加列表项",
            _ => "插入链接",
        };
        let input = cx.new(|cx| {
            InputState::new(w, cx).placeholder(if kind == "link" {
                "https://…"
            } else {
                "输入内容"
            })
        });
        let focus = input.clone();
        let target = cx.entity().downgrade();
        let error = cx.new(|_| String::new());
        w.open_dialog(cx, move |dialog, _, cx| {
            let input = input.clone();
            let target = target.clone();
            let error_submit = error.clone();
            dialog
                .title(label)
                .w(px(368.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("插入")
                        .cancel_text("取消"),
                )
                .child(Input::new(&input))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xa9472f))
                        .child(error.read(cx).clone()),
                )
                .on_ok(move |_, w, cx| {
                    let value = input.read(cx).value().to_string();
                    let snippet = match crate::model::format_snippet(kind, &value) {
                        Ok(s) => s,
                        Err(e) => {
                            error_submit.update(cx, |s, cx| {
                                *s = e.to_string();
                                cx.notify();
                            });
                            w.refresh();
                            return false;
                        }
                    };
                    target
                        .update(cx, |v, cx| {
                            v.editor.update(cx, |s, cx| s.replace(snippet, w, cx));
                            v.edit.set_dirty(true);
                            v.flush(cx);
                            cx.notify();
                            !v.edit.is_dirty()
                        })
                        .unwrap_or(true)
                })
        });
        focus.update(cx, |s, cx| s.focus(w, cx));
    }
    fn dialog<T: Send + 'static>(
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> T + Send + 'static,
        apply: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let task = cx.background_executor().spawn(async move { work() });
        cx.spawn_in(window, async move |view, cx| {
            let value = task.await;
            let _ = view.update_in(cx, |v, w, cx| apply(v, value, w, cx));
        })
        .detach();
    }
    fn insert_media(&mut self, block: Block, cx: &mut Context<Self>) {
        self.flush(cx);
        if self.edit.is_dirty() || !self.refresh_editor(cx) {
            return;
        }
        let Some(mut note) = self.current_note() else {
            return;
        };
        let bid = block.id().to_string();
        if !note.blocks.iter().any(|b| b.id() == bid) {
            note.blocks.push(block);
        }
        let result = self.shared.borrow_mut().store.save_note(note);
        if result.is_err() {
            self.result(result);
            return;
        }
        if let Some(saved) = self.current_note() {
            self.edit.observe(&saved);
        }
        self.editor.update(cx, |e, cx| {
            e.insert_attachment(bid, "media".into(), "素材".into(), cx)
        });
        self.edit.set_dirty(true);
        self.flush(cx);
        cx.notify();
    }
    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush(cx);
        let picker = rfd::AsyncFileDialog::new()
            .set_title("插入图片或文件")
            .set_parent(window);
        cx.spawn_in(window, async move |view, cx| {
            let files = picker.pick_files().await;
            let _ = view.update_in(cx, |v, w, cx| {
                if let Some(files) = files {
                    let paths: Vec<_> = files.iter().map(|f| f.path().to_path_buf()).collect();
                    v.import_paths(&paths, w, cx);
                }
                v.editor.update(cx, |e, cx| e.focus(w, cx));
                cx.notify();
            });
        })
        .detach();
    }
    fn insert_prepared(
        &mut self,
        prepared: crate::media::PreparedAttachment,
        cx: &mut Context<Self>,
    ) {
        self.flush(cx);
        let Some(n) = self.current_note() else {
            let mut note = Note::new();
            note.group_id = self.group.clone().unwrap_or_else(|| DEFAULT_GROUP.into());
            let nid = note.id.clone();
            let saved = self.shared.borrow_mut().store.save_note(note);
            if let Err(e) = saved {
                self.result(Err(e));
                return;
            }
            let result = self
                .shared
                .borrow_mut()
                .store
                .attach_prepared(&nid, prepared);
            self.result(result.map(|_| ()));
            self.trash = false;
            self.shared.borrow_mut().generation += 1;
            cx.notify();
            return;
        };
        let result = self
            .shared
            .borrow_mut()
            .store
            .attach_prepared(&n.id, prepared);
        match result {
            Ok(_) => {
                if let Some(block) = self.current_note().and_then(|n| n.blocks.last().cloned()) {
                    self.insert_media(block, cx);
                }
            }
            Err(e) => self.result(Err(e)),
        }
    }
    fn paste_image(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if w.has_active_dialog(cx) {
            return;
        }
        #[cfg(windows)]
        if let Some(paths) = crate::platform::clipboard_files() {
            self.import_paths(&paths, w, cx);
            cx.stop_propagation();
            w.prevent_default();
            return;
        }
        if let Some(markdown) = cx.read_from_clipboard().and_then(|item| {
            item.metadata()
                .and_then(|m| m.strip_prefix("suojian-markdown-v1\n"))
                .map(str::to_owned)
        }) {
            self.flush(cx);
            let mut document = gpui_note_editor::document::Document::from_markdown(&markdown);
            let mut assets = Vec::new();
            for block in &mut document.blocks {
                if let gpui_note_editor::document::BlockKind::Attachment { id: bid, .. } =
                    &mut block.kind
                {
                    let source = self
                        .shared
                        .borrow()
                        .store
                        .notes
                        .iter()
                        .flat_map(|n| n.blocks.iter().chain(&n.media_archive))
                        .find(|b| b.id() == bid)
                        .cloned();
                    if let Some(mut asset) = source {
                        let new_id = id();
                        match &mut asset {
                            Block::Markdown { id, .. }
                            | Block::Image { id, .. }
                            | Block::RemoteImage { id, .. }
                            | Block::Audio { id, .. }
                            | Block::File { id, .. } => *id = new_id.clone(),
                        }
                        *bid = new_id;
                        assets.push(asset);
                    }
                }
            }
            let markdown = document.to_markdown();
            if let Some(mut note) = self.current_note() {
                note.media_archive.extend(assets);
                let result = self.shared.borrow_mut().store.save_note(note);
                if let Err(error) = result {
                    self.result(Err(error));
                    return;
                }
                self.paste_content(markdown, w, cx);
            } else if matches!(self.screen, Screen::Center) {
                let mut note = Note::new();
                note.group_id = self.group.clone().unwrap_or_else(|| DEFAULT_GROUP.into());
                note.media_archive = assets;
                note.set_editor_markdown(&markdown);
                let result = self.shared.borrow_mut().store.save_note(note);
                self.result(result);
                self.shared.borrow_mut().generation += 1;
                cx.notify();
            }
            cx.stop_propagation();
            w.prevent_default();
            return;
        }
        #[cfg(windows)]
        let html = suojian::remote_images::clipboard_html();
        #[cfg(target_os = "linux")]
        let html: Option<String> = None;
        if let Some(text) = html
            .as_deref()
            .filter(|text| suojian::remote_images::contains(text))
        {
            self.paste_content(text.to_string(), w, cx);
            return;
        }
        #[cfg(windows)]
        if let Some(bytes) = crate::platform::clipboard_bitmap() {
            Self::dialog(
                w,
                cx,
                move || crate::media::prepare_bytes("clipboard.bmp".into(), bytes, true),
                |v, result, _, cx| match result {
                    Ok(p) => v.insert_prepared(p, cx),
                    Err(e) => v.result(Err(e)),
                },
            );
            cx.stop_propagation();
            w.prevent_default();
            return;
        }
        let item = cx.read_from_clipboard();
        if let Some(item) = &item {
            if let Some(ClipboardEntry::Image(image)) = item
                .entries()
                .iter()
                .find(|e| matches!(e, ClipboardEntry::Image(_)))
            {
                self.flush(cx);
                let bytes = image.bytes.clone();
                Self::dialog(
                    w,
                    cx,
                    move || crate::media::prepare_bytes("clipboard.png".into(), bytes, true),
                    |v, result, _, cx| match result {
                        Ok(p) => v.insert_prepared(p, cx),
                        Err(e) => v.result(Err(e)),
                    },
                );
                cx.stop_propagation();
                w.prevent_default();
                return;
            }
        }
        if let Some(text) = html.or_else(|| item.and_then(|i| i.text())) {
            self.paste_content(text, w, cx);
        }
    }
    fn paste_content(&mut self, text: String, w: &mut Window, cx: &mut Context<Self>) {
        if self.current_note().is_some() {
            self.editor
                .update(cx, |e, cx| e.replace(text.replace("\r\n", "\n"), w, cx));
            self.edit.set_dirty(true);
            self.flush(cx);
        } else if matches!(self.screen, Screen::Center) {
            let mut note = Note::new();
            note.group_id = self.group.clone().unwrap_or_else(|| DEFAULT_GROUP.into());
            note.set_editor_markdown(&text);
            let result = self.shared.borrow_mut().store.save_note(note);
            self.result(result);
            self.shared.borrow_mut().generation += 1;
        } else {
            return;
        }
        cx.stop_propagation();
        w.prevent_default();
        cx.notify();
    }
    fn import_paths(
        &mut self,
        paths: &[std::path::PathBuf],
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = paths.to_vec();
        Self::dialog(
            w,
            cx,
            move || {
                paths
                    .iter()
                    .map(|p| crate::media::prepare_file(p))
                    .collect::<Vec<_>>()
            },
            |v, results, _, cx| {
                for result in results {
                    match result {
                        Ok(p) => v.insert_prepared(p, cx),
                        Err(e) => v.result(Err(e)),
                    }
                }
            },
        );
    }
    fn recording_failed(&mut self, error: anyhow::Error, cx: &mut Context<Self>) {
        if let Some(bid) = self.recording_block.take() {
            self.editor
                .update(cx, |e, cx| e.remove_attachment(&bid, cx));
            self.edit.set_dirty(true);
            self.flush(cx);
        }
        self.result(Err(error));
        cx.notify();
    }
    fn toggle_record(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = &self.recorder {
            r.stop();
            self.saving_audio = true;
        } else {
            self.flush(cx);
            let bid = id();
            self.insert_media(
                Block::Audio {
                    id: bid.clone(),
                    asset: String::new(),
                    duration_ms: 0,
                    text: None,
                    state: AudioState::Recorded,
                },
                cx,
            );
            self.recording_block = Some(bid);
            self.recorder = Some(audio::Recorder::start());
            self.record_started = Some(Instant::now());
        }
        cx.notify();
    }
    fn play(&mut self, asset: String, cx: &mut Context<Self>) {
        let active = self.shared.borrow().playing_asset.clone();
        if let Some(stop) = self.shared.borrow_mut().playback_stop.take() {
            let _ = stop.send(());
        }
        if active.as_ref() == Some(&asset) {
            return;
        }
        let bytes = self.shared.borrow().store.asset(&asset);
        match bytes {
            Ok(bytes) => {
                let tx = self.tx.clone();
                let (stop, stop_rx) = mpsc::channel();
                {
                    let mut rt = self.shared.borrow_mut();
                    rt.playing_asset = Some(asset.clone());
                    rt.playback_stop = Some(stop);
                    rt.generation += 1;
                }
                std::thread::spawn(move || {
                    let _ = tx.send(Job::Played(
                        asset,
                        audio::play(bytes, stop_rx).map_err(|e| e.to_string()),
                    ));
                });
            }
            Err(e) => self.result(Err(e)),
        }
        cx.notify();
    }
    fn retry_transcription(&mut self, bid: &str, cx: &mut Context<Self>) {
        let note = self
            .shared
            .borrow()
            .store
            .notes
            .iter()
            .find(|n| {
                n.blocks
                    .iter()
                    .any(|b| matches!(b,Block::Audio{id,..}if id==bid))
            })
            .cloned();
        if let Some(mut n) = note {
            let mut bytes = None;
            for block in &mut n.blocks {
                if let Block::Audio {
                    id, asset, state, ..
                } = block
                {
                    if id == bid {
                        bytes = self.shared.borrow().store.asset(asset).ok();
                        *state = AudioState::Processing;
                    }
                }
            }
            let settings = audio::resolve_speech(&self.shared.borrow().store.settings);
            if !std::path::Path::new(&settings.model_path).is_file()
                || !std::path::Path::new(&settings.whisper_cli).is_file()
            {
                self.shared.borrow_mut().status =
                    "随程序提供的语音组件缺失，请恢复 runtime 文件夹".into();
                return;
            }
            if let Some(bytes) = bytes {
                let nid = n.id.clone();
                let result = self.shared.borrow_mut().store.save_note(n);
                if let Err(error) = result {
                    self.result(Err(error));
                    return;
                }
                let operation = id();
                self.shared
                    .borrow_mut()
                    .transcript_jobs
                    .insert(bid.to_string(), operation.clone());
                let tx = self.tx.clone();
                let bid = bid.to_string();
                let root = self.shared.borrow().store.root.join("scratch");
                std::thread::spawn(move || {
                    let result =
                        audio::transcribe(bytes, &settings, root).map_err(|e| e.to_string());
                    let _ = tx.send(Job::Transcript(nid, bid, operation, result));
                });
                self.shared.borrow_mut().generation += 1;
                cx.notify();
            }
        }
    }
    fn audio_card(
        &self,
        bid: String,
        asset: String,
        duration: u64,
        text: Option<String>,
        state: AudioState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = self.expanded.contains(&bid);
        let toggle = bid.clone();
        let mut bars = div()
            .flex()
            .flex_1()
            .items_center()
            .gap(px(2.))
            .h(px(28.))
            .overflow_hidden();
        if !self.shared.borrow().waveforms.contains_key(&asset) {
            let decoded = self
                .shared
                .borrow()
                .store
                .asset(&asset)
                .ok()
                .map(|b| audio::waveform(&b));
            if let Some(peaks) = decoded {
                self.shared
                    .borrow_mut()
                    .waveforms
                    .insert(asset.clone(), peaks);
            }
        }
        let peaks = self
            .shared
            .borrow()
            .waveforms
            .get(&asset)
            .cloned()
            .unwrap_or_else(|| vec![0.; 39]);
        for amplitude in peaks {
            bars = bars.child(
                div()
                    .w(px(2.))
                    .h(px(3. + amplitude * 23.))
                    .flex_shrink_0()
                    .rounded(px(1.))
                    .bg(rgb(TEAL)),
            );
        }
        let playing = self.shared.borrow().playing_asset.as_ref() == Some(&asset);
        let play = button(
            SharedString::from(format!("play-{bid}")),
            if playing { "stop" } else { "play" },
        )
        .bg(rgb(TEAL))
        .text_color(rgb(PAPER))
        .rounded_full()
        .on_click(cx.listener(move |v, _, _, cx| {
            cx.stop_propagation();
            v.play(asset.clone(), cx);
        }));
        let arrow = button(
            SharedString::from(format!("audio-expand-{bid}")),
            if expanded { "down" } else { "right" },
        )
        .on_click(cx.listener(move |v, _, _, cx| {
            cx.stop_propagation();
            if !v.expanded.insert(toggle.clone()) {
                v.expanded.remove(&toggle);
            }
            cx.notify();
        }));
        let row = div()
            .flex()
            .items_center()
            .gap_3()
            .px(px(10.))
            .py(px(10.))
            .child(play)
            .child(bars)
            .child(div().text_size(px(14.)).text_color(rgb(INK)).child(format!(
                "{:02}:{:02}",
                duration / 60000,
                duration / 1000 % 60
            )))
            .child(if matches!(state, AudioState::Processing) {
                icon("loading").into_any_element()
            } else {
                arrow.into_any_element()
            });
        let mut card = div()
            .border_1()
            .border_color(rgb(LINE))
            .bg(rgb(0xf6f2e9))
            .rounded(px(6.))
            .child(row);
        let retry = bid.clone();
        let needs_retry = matches!(state, AudioState::Recorded | AudioState::Failed(_));
        match state {
            AudioState::Processing => {
                card = card.child(
                    div()
                        .pl(px(54.))
                        .pb(px(10.))
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("转写中…"),
                )
            }
            AudioState::Failed(e) if expanded => {
                card = card.child(div().p_3().text_xs().text_color(rgb(0xa34c35)).child(e))
            }
            AudioState::Recorded if expanded => {
                card = card.child(
                    div()
                        .p_3()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("尚未转写，请先在设置中配置语音模型"),
                )
            }
            _ => {}
        }
        if expanded && needs_retry {
            card = card.child(
                label_button(SharedString::from(format!("retry-{bid}")), "转写")
                    .text_color(rgb(TEAL))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        cx.stop_propagation();
                        v.retry_transcription(&retry, cx);
                    })),
            );
        }
        if expanded {
            if let Some(text) = text {
                let editing = self.transcript_edit.as_ref().filter(|(id, _)| id == &bid);
                let bid_edit = bid.clone();
                let initial = text.clone();
                let mut transcript = div()
                    .border_t_1()
                    .border_color(rgb(LINE))
                    .p(px(14.))
                    .flex()
                    .flex_col()
                    .gap_2();
                if let Some((_, input)) = editing {
                    transcript = transcript.child(Input::new(input).appearance(false).h(px(110.)));
                } else {
                    transcript = transcript.child(
                        div()
                            .text_size(px(16.))
                            .line_height(relative(1.5))
                            .child(text),
                    );
                }
                transcript=transcript.child(div().flex().justify_end().child(button(SharedString::from(format!("edit-transcript-{bid}")),if editing.is_some(){"tick"}else{"pencil"}).on_click(cx.listener(move |v,_,w,cx|{
                    cx.stop_propagation();
                    if let Some((edit_id,input))=&v.transcript_edit {
                        if edit_id==&bid_edit {
                            let value=input.read(cx).value().to_string();
                            let note={v.shared.borrow().store.notes.iter().find(|n|n.blocks.iter().any(|b|matches!(b,Block::Audio{id,..}if id==&bid_edit))).cloned()};
                            if let Some(mut note)=note {
                                for b in &mut note.blocks {if let Block::Audio{id,text,..}=b {if id==&bid_edit {*text=Some(value.clone());}}}
                                let result=v.shared.borrow_mut().store.save_note(note);if result.is_err(){v.result(result);return;}
                                v.shared.borrow_mut().generation+=1;
                            }
                            v.transcript_edit=None;cx.notify();return;
                        }
                    }
                    let input=cx.new(|cx|InputState::new(w,cx).multi_line(true));
                    input.update(cx,|s,cx|{s.set_value(initial.clone(),w,cx);s.focus(w,cx);});
                    v.transcript_edit=Some((bid_edit.clone(),input));cx.notify();
                }))));
                card = card.child(transcript);
            }
        }
        card.into_any_element()
    }
    fn blocks(
        &mut self,
        n: &Note,
        preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = div().flex().flex_col().gap_3();
        for b in &n.blocks {
            let previous = body;
            body = div().flex().flex_col().gap_3();
            match b {
                Block::Markdown { id, text } if !text.is_empty() => {
                    let text = text.clone();
                    if text
                        .lines()
                        .any(|line| line.starts_with("- [ ] ") || line.starts_with("- [x] "))
                    {
                        let mut lines = div().flex().flex_col().gap_2();
                        for (index, line) in text.lines().enumerate() {
                            if let Some(label) = line
                                .strip_prefix("- [ ] ")
                                .or_else(|| line.strip_prefix("- [x] "))
                            {
                                let checked = line.starts_with("- [x]");
                                let nid = n.id.clone();
                                let bid = id.clone();
                                let mark = div()
                                    .size(px(15.))
                                    .border_1()
                                    .border_color(rgb(TEAL))
                                    .rounded(px(2.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(checked, |d| {
                                        d.bg(rgb(TEAL)).child(
                                            icon("tick").size(px(11.)).text_color(rgb(PAPER)),
                                        )
                                    });
                                lines = lines.child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "task-{}-{id}-{index}",
                                            n.id
                                        )))
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .cursor_pointer()
                                        .child(mark)
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_size(px(16.))
                                                .child(label.to_string()),
                                        )
                                        .on_click(cx.listener(move |v, _, w, cx| {
                                            cx.stop_propagation();
                                            v.flush(cx);
                                            let note = v
                                                .shared
                                                .borrow()
                                                .store
                                                .notes
                                                .iter()
                                                .find(|n| n.id == nid)
                                                .cloned();
                                            if let Some(mut n) = note {
                                                for block in &mut n.blocks {
                                                    if let Block::Markdown { id, text } = block {
                                                        if id == &bid {
                                                            let mut lines: Vec<_> = text
                                                                .lines()
                                                                .map(str::to_string)
                                                                .collect();
                                                            if let Some(line) = lines.get_mut(index)
                                                            {
                                                                if line.starts_with("- [ ] ") {
                                                                    *line = line.replacen(
                                                                        "- [ ] ", "- [x] ", 1,
                                                                    );
                                                                } else if line.starts_with("- [x] ")
                                                                {
                                                                    *line = line.replacen(
                                                                        "- [x] ", "- [ ] ", 1,
                                                                    );
                                                                }
                                                            }
                                                            *text = lines.join("\n");
                                                        }
                                                    }
                                                }
                                                if matches!(&v.screen,Screen::Note(id)if id==&nid) {
                                                    v.editor.update(cx, |s, cx| {
                                                        s.set_value(n.editor_markdown(), w, cx)
                                                    });
                                                }
                                                let result =
                                                    v.shared.borrow_mut().store.save_note(n);
                                                v.result(result);
                                                v.shared.borrow_mut().generation += 1;
                                                cx.notify();
                                            }
                                        })),
                                );
                            } else if !line.is_empty() {
                                lines = lines.child(
                                    TextView::markdown(
                                        SharedString::from(format!("{}-{id}-{index}", n.id)),
                                        line.to_string(),
                                        window,
                                        cx,
                                    )
                                    .text_color(rgb(INK))
                                    .text_size(px(16.)),
                                );
                            }
                        }
                        body = body.child(lines);
                    } else {
                        body = body.child(
                            TextView::markdown(
                                SharedString::from(format!("{}-{id}", n.id)),
                                text,
                                window,
                                cx,
                            )
                            .text_color(rgb(INK))
                            .text_size(px(16.)),
                        );
                    }
                }
                Block::RemoteImage { id: bid, name, .. } => {
                    let bid = bid.clone();
                    let error: Option<String> = self
                        .shared
                        .borrow()
                        .store
                        .remote_image_error(&bid)
                        .ok()
                        .flatten();
                    body =
                        body.child(
                            div()
                                .p_3()
                                .border_1()
                                .border_color(rgb(LINE))
                                .rounded(px(4.))
                                .child(if name.is_empty() {
                                    "网页图片".to_string()
                                } else {
                                    name.clone()
                                })
                                .child(div().text_sm().text_color(rgb(MUTED)).child(
                                    error.unwrap_or_else(|| {
                                        "图片等待下载，联网后自动保存到本机".into()
                                    }),
                                ))
                                .child(
                                    Button::new(SharedString::from(format!("retry-{bid}")))
                                        .label("重试下载")
                                        .on_click(cx.listener(move |v, _, _, cx| {
                                            v.shared.borrow_mut().image_retry.remove(&bid);
                                            cx.notify();
                                        })),
                                ),
                        );
                }
                Block::Image { asset, name, .. } => {
                    let needs_load = {
                        let runtime = self.shared.borrow();
                        !runtime.images.contains_key(asset)
                            && !runtime.image_errors.contains_key(asset)
                            && !runtime.image_loads.contains(asset)
                            && runtime.image_loads.len() < 2
                    };
                    if needs_load {
                        let mut runtime = self.shared.borrow_mut();
                        runtime.image_loads.insert(asset.clone());
                        let root = runtime.store.root.clone();
                        let asset = asset.clone();
                        let tx = runtime.job_tx.clone();
                        std::thread::spawn(move || {
                            let result = (|| -> anyhow::Result<Vec<u8>> {
                                uuid::Uuid::parse_str(&asset)?;
                                let bytes = std::fs::read(root.join("assets").join(&asset))?;
                                crate::media::image_preview(&bytes)
                            })()
                            .map_err(|error| error.to_string());
                            let _ = tx.send(Job::ImageLoaded(asset, result));
                        });
                    }
                    let image = {
                        let mut runtime = self.shared.borrow_mut();
                        let image = runtime.images.get(asset).cloned();
                        if image.is_some() {
                            runtime.image_access.insert(asset.clone(), Instant::now());
                        }
                        image
                    };
                    if let Some(image) = image {
                        let width = if self.embedded {
                            self.content_width
                        } else {
                            f32::from(window.viewport_size().width) - 44.
                        };
                        let dimensions =
                            image::ImageReader::new(std::io::Cursor::new(&image.bytes))
                                .with_guessed_format()
                                .ok()
                                .and_then(|r| r.into_dimensions().ok());
                        let height = dimensions
                            .map(|(w, h)| width * h as f32 / w.max(1) as f32)
                            .unwrap_or(180.);
                        body = body.child(
                            div()
                                .w_full()
                                .h(px(height))
                                .flex_shrink_0()
                                .overflow_hidden()
                                .rounded(px(4.))
                                .child(img(image).size_full().object_fit(ObjectFit::Contain)),
                        );
                    } else {
                        body = body.child(
                            div()
                                .p_3()
                                .border_1()
                                .border_color(rgb(LINE))
                                .text_size(px(12.))
                                .text_color(rgb(MUTED))
                                .child(if self.shared.borrow().image_errors.contains_key(asset) {
                                    format!("图片无法预览：{name}")
                                } else {
                                    format!("正在加载图片：{name}")
                                }),
                        );
                    }
                }
                Block::Audio { asset, .. } if asset.is_empty() => {
                    let elapsed = self
                        .record_started
                        .map(|t| t.elapsed().as_secs())
                        .unwrap_or(0);
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .p_3()
                            .border_1()
                            .border_color(rgb(LINE))
                            .rounded(px(4.))
                            .child(
                                button("inline-stop", "stop")
                                    .bg(rgb(0xb4473b))
                                    .rounded_full()
                                    .on_click(cx.listener(|v, _, _, cx| v.toggle_record(cx))),
                            )
                            .child(div().flex_1().child(if self.saving_audio {
                                "保存录音…"
                            } else if self.recorder.is_some() {
                                "录音中"
                            } else {
                                "录音未完成"
                            }))
                            .child(format!("{:02}:{:02}", elapsed / 60, elapsed % 60)),
                    );
                }
                Block::Audio {
                    id,
                    asset,
                    duration_ms,
                    text,
                    state,
                } => {
                    body = body.child(self.audio_card(
                        id.clone(),
                        asset.clone(),
                        *duration_ms,
                        text.clone(),
                        state.clone(),
                        cx,
                    ));
                }
                Block::File { id, asset, name } => {
                    let asset = asset.clone();
                    let name = name.clone();
                    let label = name.clone();
                    body = body.child(
                        div()
                            .id(SharedString::from(format!("file-{id}")))
                            .flex()
                            .gap_2()
                            .p_3()
                            .border_1()
                            .border_color(rgb(LINE))
                            .rounded(px(6.))
                            .bg(rgb(0xf6f2e9))
                            .items_center()
                            .cursor_pointer()
                            .child(icon("file"))
                            .child(label)
                            .on_click(cx.listener(move |v, _, w, cx| {
                                cx.stop_propagation();
                                let path = v
                                    .shared
                                    .borrow()
                                    .store
                                    .materialize_attachment(&asset, &name);
                                Self::dialog(
                                    w,
                                    cx,
                                    move || -> anyhow::Result<()> {
                                        crate::platform::open_attachment(&path?)
                                    },
                                    |v, result, _, cx| {
                                        v.result(result);
                                        cx.notify();
                                    },
                                );
                            })),
                    );
                }
                _ => {}
            }
            let depth = n.media_depths.get(b.id()).copied().unwrap_or(0);
            let node = body.ml(px(depth as f32 * 16.));
            body = if !matches!(b, Block::Markdown { .. })
                && !self.shared.borrow().store.settings.reduced_motion
            {
                previous.child(node.with_animation(
                    SharedString::from(format!("media-enter-{}", b.id())),
                    Animation::new(Duration::from_millis(160)).with_easing(ease_in_out),
                    |d, t| d.opacity(t),
                ))
            } else {
                previous.child(node)
            };
        }
        if preview {
            return div()
                .id(SharedString::from(format!("card-scroll-{}", n.id)))
                .max_h(px(360.))
                .overflow_y_scroll()
                .child(body)
                .into_any_element();
        }
        body.into_any_element()
    }
    fn center(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let groups = self.shared.borrow().store.groups.clone();
        let query = self.search.read(cx).value().to_string();
        let pending: HashSet<_> = self
            .cards
            .iter()
            .filter(|(_, view)| view.read(cx).has_unsaved_edits(cx))
            .map(|(id, _)| id.clone())
            .collect();
        let mut notes: Vec<_> = self
            .shared
            .borrow()
            .store
            .notes
            .iter()
            .filter(|n| {
                pending.contains(&n.id)
                    || (!n.is_empty()
                        && n.deleted_at.is_some() == self.trash
                        && self.group.as_ref().is_none_or(|g| g == &n.group_id)
                        && n.matches(&query))
            })
            .cloned()
            .collect();
        let shown: HashSet<_> = notes.iter().map(|note| note.id.as_str()).collect();
        self.cards.retain(|id, _| shown.contains(id.as_str()));
        notes.sort_by_key(|n| (std::cmp::Reverse(n.created_at), n.id.clone()));
        let filter = format!("{:?}/{}/{}", self.group, self.trash, query);
        if self.selection_filter != filter {
            self.selection.clear();
            self.selection_anchor = None;
            self.selection_filter = filter;
        }
        self.selection
            .retain(|id| notes.iter().any(|n| &n.id == id));
        let mut nav = div()
            .flex()
            .items_center()
            .gap(px(14.))
            .child(
                div()
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .window_control_area(WindowControlArea::Drag)
                    .child(icon("brand").size(px(24.)).text_color(rgb(TEAL))),
            )
            .child(
                div()
                    .mr(px(8.))
                    .pr(px(16.))
                    .border_r_1()
                    .border_color(rgb(MUTED))
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(22.))
                    .window_control_area(WindowControlArea::Drag)
                    .child("闪记"),
            );
        let mut tabs = div()
            .id("group-tabs")
            .flex()
            .items_center()
            .gap_2()
            .flex_1()
            .min_w_0()
            .overflow_x_scroll();
        tabs = tabs.child(
            label_button("all", "全部")
                .flex_shrink_0()
                .when(self.group.is_none() && !self.trash, |d| {
                    d.border_b_2().border_color(rgb(TEAL))
                })
                .on_click(cx.listener(|v, _, _, cx| {
                    v.group = None;
                    v.trash = false;
                    cx.notify();
                })),
        );
        let tab_widths: Vec<f32> = groups
            .iter()
            .map(|g| {
                let run = TextRun {
                    len: g.name.len(),
                    font: font(crate::platform::UI_FONT),
                    color: rgb(INK).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                f32::from(
                    window
                        .text_system()
                        .shape_line(g.name.clone().into(), px(14.), &[run], None)
                        .width,
                ) + 44.
            })
            .collect();
        let available = (f32::from(window.viewport_size().width) - 750.).max(0.);
        let visible_count = fitting_groups(&tab_widths, available);
        for (g, tab_width) in groups.iter().zip(&tab_widths).take(visible_count) {
            let gid = g.id.clone();
            let edit_group = g.clone();
            let edit_view = cx.entity().downgrade();
            tabs = tabs.child(
                div()
                    .id(SharedString::from(gid.clone()))
                    .w(px(*tab_width))
                    .h(px(36.))
                    .px(px(10.))
                    .cursor_pointer()
                    .rounded(px(5.))
                    .text_size(px(14.))
                    .hover(|d| d.bg(rgb(0xeceee6)))
                    .text_color(rgb(g.color))
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_shrink_0()
                    .child(
                        icon(group_icon(&g.icon))
                            .size(px(16.))
                            .text_color(rgb(g.color)),
                    )
                    .child(g.name.clone())
                    .when(!self.trash && self.group.as_ref() == Some(&gid), |d| {
                        d.border_b_2().border_color(rgb(g.color))
                    })
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.group = Some(gid.clone());
                        v.trash = false;
                        cx.notify();
                    }))
                    .context_menu(move |menu, window, cx| {
                        if window.has_active_dialog(cx) {
                            return menu;
                        }
                        let target = edit_view.clone();
                        let group = edit_group.clone();
                        menu.item(PopupMenuItem::new("编辑分类").on_click(move |_, w, cx| {
                            let _ = target
                                .update(cx, |v, cx| v.group_dialog(Some(group.clone()), w, cx));
                        }))
                    }),
            );
        }
        let menu_view = cx.entity().downgrade();
        if visible_count < groups.len() {
            tabs = tabs.child(
                Button::new("more-groups")
                    .ghost()
                    .label("更多")
                    .child(icon("down").size(px(12.)))
                    .dropdown_menu(move |mut menu, _, cx| {
                        let Some(view) = menu_view.upgrade() else {
                            return menu;
                        };
                        let groups = view.read(cx).shared.borrow().store.groups.clone();
                        let selected = view.read(cx).group.clone();
                        let in_trash = view.read(cx).trash;
                        for group in groups.into_iter().skip(visible_count) {
                            let target = menu_view.clone();
                            let gid = group.id.clone();
                            menu = menu.item(
                                PopupMenuItem::element(move |_, _| {
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            icon(group_icon(&group.icon))
                                                .size(px(18.))
                                                .text_color(rgb(group.color)),
                                        )
                                        .child(
                                            div()
                                                .text_color(rgb(group.color))
                                                .child(group.name.clone()),
                                        )
                                })
                                .checked(!in_trash && selected.as_ref() == Some(&gid))
                                .on_click(move |_, _, cx| {
                                    let _ = target.update(cx, |v, cx| {
                                        v.group = Some(gid.clone());
                                        v.trash = false;
                                        cx.notify();
                                    });
                                }),
                            );
                        }
                        menu
                    }),
            );
        }
        tabs = tabs
            .child(
                button("new-group", "plus")
                    .on_click(cx.listener(|v, _, w, cx| v.create_group_dialog(w, cx))),
            )
            .child(
                button("trash-tab", "trash")
                    .when(self.trash, |d| d.bg(rgb(0xe5e8df)))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.group = None;
                        v.trash = true;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .h(px(38.))
                    .window_control_area(WindowControlArea::Drag),
            );
        nav = nav
            .child(tabs)
            .child(
                div()
                    .w(px(180.))
                    .flex_shrink_0()
                    .h(px(38.))
                    .border_1()
                    .border_color(rgb(LINE))
                    .rounded(px(6.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(icon("search").flex_shrink_0())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.search).appearance(false)),
                    ),
            )
            .child(
                button("new", "plus")
                    .bg(rgb(0xdde7df))
                    .on_click(cx.listener(|v, _, _, cx| v.new_note(cx))),
            );
        nav = nav.child(button("settings-center", "settings").on_click(cx.listener(
            |v, _, w, cx| {
                v.flush_cards(cx);
                v.clear_note_selection(w, cx);
                v.screen = Screen::Settings;
                cx.notify();
            },
        )));
        nav = nav
            .child(
                button("min-center", "minus")
                    .on_click(cx.listener(|_, _, w, _| w.minimize_window())),
            )
            .child(
                button("max-center", "maximize")
                    .on_click(cx.listener(|_, _, w, _| w.zoom_window())),
            )
            .child(
                button("close-center", "close")
                    .on_click(cx.listener(|_, _, w, _| w.remove_window())),
            );
        let top = div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(rgb(LINE))
            .child(
                div()
                    .w_full()
                    .h(px(12.))
                    .flex_shrink_0()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(div().px(px(28.)).child(nav))
            .child(
                div()
                    .w_full()
                    .h(px(12.))
                    .flex_shrink_0()
                    .window_control_area(WindowControlArea::Drag),
            );
        let width: f32 = window.viewport_size().width.into();
        let cols = (((width - 56. + 18.) / 420.).round() as usize).max(1);
        let mut columns: Vec<Vec<AnyElement>> = (0..cols).map(|_| vec![]).collect();
        let mut heights = vec![0usize; cols];
        for n in &notes {
            let g = groups
                .iter()
                .find(|g| g.id == n.group_id)
                .unwrap_or(&groups[0]);
            let nid = n.id.clone();
            let card_width = (width - 56. - 18. * (cols - 1) as f32) / cols as f32 - 40.;
            let view = if let Some(view) = self.cards.get(&nid) {
                view.clone()
            } else {
                let shared = self.shared.clone();
                let screen = Screen::Note(nid.clone());
                let view = cx.new(|cx| View::new(shared, screen, true, window, cx));
                self.cards.insert(nid.clone(), view.clone());
                view
            };
            let selected = if self.multi {
                self.selection.contains(&nid)
            } else {
                self.selected_note.as_ref() == Some(&nid)
            };
            let editor_selected = selected && !self.multi;
            view.update(cx, |v, cx| {
                v.content_width = card_width;
                if v.card_selected != editor_selected {
                    v.card_selected = editor_selected;
                    cx.notify();
                }
            });
            let wheel_scroll = view.read(cx).editor.read(cx).scroll_handle();
            let wheel_id = nid.clone();
            let select_id = nid.clone();
            let deselect_id = nid.clone();
            let body = view;
            let open_body = body.clone();
            let open_id = nid.clone();
            let header_body = body.clone();
            let header_id = nid.clone();
            let group_control = self.group_selector(n, g, cx);
            let menu_view = cx.entity().downgrade();
            let menu_id = nid.clone();
            let range_ids: Vec<_> = notes.iter().map(|n| n.id.clone()).collect();
            let card = div()
                .id(SharedString::from(format!("card-{}", n.id)))
                .relative()
                .group("note-card")
                .on_mouse_down_out(cx.listener(move |v, _: &MouseDownEvent, w, cx| {
                    if v.selected_note.as_ref() == Some(&deselect_id) {
                        v.clear_note_selection(w, cx);
                    }
                }))
                .capture_any_mouse_down(cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                    if v.multi {
                        if e.button == MouseButton::Left {
                            w.focus(&v.page_focus);
                            if e.modifiers.shift {
                                if let (Some(a), Some(b)) = (
                                    v.selection_anchor
                                        .as_ref()
                                        .and_then(|id| range_ids.iter().position(|n| n == id)),
                                    range_ids.iter().position(|n| n == &select_id),
                                ) {
                                    v.selection
                                        .extend(range_ids[a.min(b)..=a.max(b)].iter().cloned());
                                } else {
                                    v.selection.insert(select_id.clone());
                                }
                            } else if !v.selection.remove(&select_id) {
                                v.selection.insert(select_id.clone());
                            }
                            v.selection_anchor = Some(select_id.clone());
                            cx.stop_propagation();
                            cx.notify();
                        }
                        return;
                    }
                    if e.button == MouseButton::Left && v.selected_note.as_ref() != Some(&select_id)
                    {
                        v.clear_note_selection(w, cx);
                        v.selected_note = Some(select_id.clone());
                        cx.notify();
                    }
                }))
                .on_scroll_wheel(cx.listener(move |v, e: &ScrollWheelEvent, w, cx| {
                    let target = if v.selected_note.as_ref() == Some(&wheel_id) {
                        &wheel_scroll
                    } else {
                        &v.feed_scroll
                    };
                    let delta = e.delta.pixel_delta(w.line_height()).y;
                    let mut offset = target.offset();
                    offset.y = (offset.y + delta).clamp(-target.max_offset().height, px(0.));
                    target.set_offset(offset);
                    cx.stop_propagation();
                    w.refresh();
                }))
                .cursor_pointer()
                .on_click(cx.listener(move |v, e: &ClickEvent, w, cx| {
                    if e.click_count() == 2 && !v.multi && !w.has_active_dialog(cx) {
                        open_body.update(cx, |card, cx| card.flush(cx));
                        if !open_body.read(cx).has_unsaved_edits(cx) {
                            open(v.shared.clone(), Screen::Note(open_id.clone()), cx);
                        }
                        cx.stop_propagation();
                    }
                }))
                .border_1()
                .border_color(rgb(if selected { TEAL } else { LINE }))
                .rounded(px(8.))
                .bg(rgb(CARD))
                .p(px(20.))
                .pb(px(8.))
                .when(selected, |d| d.border_2().p(px(19.)).pb(px(7.)))
                .flex()
                .flex_col()
                .gap(px(16.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .text_size(px(13.))
                        .child(group_control)
                        .when(n.conflict_of.is_some() && !n.conflict_resolved, |d| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(0xa34c35))
                                    .id(SharedString::from(format!("resolve-{}", n.id)))
                                    .child("离线冲突 · 点击比较")
                                    .on_click({
                                        let nid = n.id.clone();
                                        cx.listener(move |v, _, w, cx| {
                                            cx.stop_propagation();
                                            let pair = v.shared.borrow().store.conflict_pair(&nid);
                                            if let Some((fork, cloud)) = pair {
                                                v.resolve_dialog(fork, cloud, w, cx);
                                            }
                                        })
                                    }),
                            )
                        })
                        .child(
                            div()
                                .id(SharedString::from(format!("open-note-{}", n.id)))
                                .flex_1()
                                .h(px(32.))
                                .flex()
                                .items_center()
                                .justify_end()
                                .text_size(px(11.))
                                .text_color(rgb(MUTED))
                                .child(timestamp(n.created_at))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                                        if e.click_count == 2
                                            && !v.multi
                                            && !w.has_active_dialog(cx)
                                        {
                                            header_body.update(cx, |card, cx| card.flush(cx));
                                            if !header_body.read(cx).has_unsaved_edits(cx) {
                                                open(
                                                    v.shared.clone(),
                                                    Screen::Note(header_id.clone()),
                                                    cx,
                                                );
                                            }
                                            cx.stop_propagation();
                                        }
                                    }),
                                ),
                        )
                        .when(self.multi, |d| {
                            d.child(
                                div()
                                    .size(px(18.))
                                    .border_1()
                                    .border_color(rgb(TEAL))
                                    .rounded(px(3.))
                                    .when(selected, |d| {
                                        d.bg(rgb(TEAL)).child(
                                            icon("tick").size(px(16.)).text_color(rgb(PAPER)),
                                        )
                                    }),
                            )
                        }),
                )
                .context_menu(move |mut menu, window, cx| {
                    if window.has_active_dialog(cx) {
                        return menu;
                    }
                    let Some(entity) = menu_view.upgrade() else {
                        return menu;
                    };
                    let multi = entity.read(cx).multi;
                    let trash = entity.read(cx).trash;
                    let target = menu_view.clone();
                    let nid = menu_id.clone();
                    if multi {
                        let _ = target.update(cx, |v, cx| {
                            v.selection.insert(nid.clone());
                            cx.notify();
                        });
                    } else {
                        let target = menu_view.clone();
                        let nid = menu_id.clone();
                        menu =
                            menu.item(PopupMenuItem::new("打开便签").on_click(move |_, _, cx| {
                                let _ = target.update(cx, |v, cx| {
                                    v.flush_cards(cx);
                                    open(v.shared.clone(), Screen::Note(nid.clone()), cx);
                                });
                            }));
                        let target = menu_view.clone();
                        let nid = menu_id.clone();
                        menu = menu.item(PopupMenuItem::new("多选").on_click(move |_, w, cx| {
                            let _ = target.update(cx, |v, cx| {
                                v.flush_cards(cx);
                                v.clear_note_selection(w, cx);
                                w.focus(&v.page_focus);
                                v.multi = true;
                                v.selection.clear();
                                v.selection.insert(nid.clone());
                                v.selection_anchor = Some(nid.clone());
                                cx.notify();
                            });
                        }));
                    }
                    let target = menu_view.clone();
                    let nid = menu_id.clone();
                    menu = menu.item(
                        PopupMenuItem::new(if multi {
                            "导出所选便签"
                        } else {
                            "导出"
                        })
                        .on_click(move |_, w, cx| {
                            let _ = target.update(cx, |v, cx| {
                                let ids = if v.multi {
                                    v.selection.iter().cloned().collect()
                                } else {
                                    vec![nid.clone()]
                                };
                                v.export_archive(ids, w, cx);
                            });
                        }),
                    );
                    let target = menu_view.clone();
                    let nid = menu_id.clone();
                    menu = menu.separator().item(
                        PopupMenuItem::new(if trash { "恢复" } else { "移入回收站" }).on_click(
                            move |_, w, cx| {
                                let _ = target.update(cx, |v, cx| {
                                    let ids = if v.multi {
                                        v.selection.iter().cloned().collect()
                                    } else {
                                        vec![nid.clone()]
                                    };
                                    v.batch_delete(ids, !trash, w, cx);
                                });
                            },
                        ),
                    );
                    if trash {
                        let target = menu_view.clone();
                        let nid = menu_id.clone();
                        menu =
                            menu.item(PopupMenuItem::new("永久删除").on_click(move |_, w, cx| {
                                let _ = target.update(cx, |v, cx| {
                                    let ids = if v.multi {
                                        v.selection.iter().cloned().collect()
                                    } else {
                                        vec![nid.clone()]
                                    };
                                    v.confirm_purge(ids, w, cx);
                                });
                            }));
                    }
                    menu
                })
                .child(body)
                .when(self.multi, |d| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("selection-overlay-{}", n.id)))
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full(),
                    )
                });
            let index = heights
                .iter()
                .enumerate()
                .min_by_key(|(_, h)| **h)
                .unwrap()
                .0;
            heights[index] += n.text().chars().count().min(350) + n.blocks.len() * 80 + 100;
            columns[index].push(card.into_any_element());
        }
        let mut feed = div()
            .id("feed")
            .track_scroll(&self.feed_scroll)
            .flex()
            .items_start()
            .gap(px(18.))
            .px(px(28.))
            .pt(px(24.))
            .pb(px(24.))
            .overflow_y_scroll()
            .flex_1()
            .min_h_0();
        for column in columns {
            feed = feed.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .children(column),
            );
        }
        let count = notes.len();
        if count == 0 {
            feed = div()
                .id("empty-feed")
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_4()
                .child(icon("grid").size(px(28.)).text_color(rgb(MUTED)))
                .child(div().text_color(rgb(MUTED)).child(if query.is_empty() {
                    "还没有便签"
                } else {
                    "没有找到匹配的便签"
                }))
                .when(query.is_empty(), |d| {
                    d.child(
                        label_button("empty-new", "＋ 新建便签")
                            .bg(rgb(TEAL))
                            .text_color(rgb(PAPER))
                            .on_click(cx.listener(|v, _, _, cx| v.new_note(cx))),
                    )
                });
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(top)
            .child(feed)
            .when(self.multi, |d| d.child(self.batch_toolbar(&notes, cx)))
            .into_any_element()
    }
    fn resolve_dialog(&mut self, fork: Note, cloud: Note, w: &mut Window, cx: &mut Context<Self>) {
        self.flush(cx);
        if self.has_unsaved_edits(cx) {
            return;
        }
        let target = cx.entity().downgrade();
        let error = cx.new(|_| String::new());
        w.open_dialog(cx, move |dialog, _, cx| {
            let mut actions = div().flex().gap_2().justify_end();
            for (key, label, choice) in [
                ("local", "保留本地", crate::store::ConflictChoice::Local),
                ("cloud", "保留云端", crate::store::ConflictChoice::Cloud),
                ("both", "两份都保留", crate::store::ConflictChoice::Both),
            ] {
                let target = target.clone();
                let error = error.clone();
                let (fid, frev, crev) = (
                    fork.id.clone(),
                    fork.revision.clone(),
                    cloud.revision.clone(),
                );
                actions = actions.child(Button::new(key).label(label).on_click(move |_, w, cx| {
                    let ok = target
                        .update(cx, |v, cx| {
                            let result = v
                                .shared
                                .borrow_mut()
                                .store
                                .resolve_conflict(&fid, &frev, &crev, choice);
                            let ok = result.is_ok();
                            if let Err(e) = &result {
                                error.update(cx, |message, cx| {
                                    *message = e.to_string();
                                    cx.notify();
                                });
                            }
                            v.result(result);
                            v.shared.borrow_mut().generation += 1;
                            cx.notify();
                            ok
                        })
                        .unwrap_or(false);
                    if ok {
                        w.close_dialog(cx);
                    } else {
                        w.refresh();
                    }
                }));
            }
            let preview = |note: &Note, label: &str| {
                let media = note
                    .blocks
                    .iter()
                    .filter(|b| !matches!(b, Block::Markdown { .. }))
                    .count();
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_1()
                    .border_color(rgb(LINE))
                    .child(label.to_string())
                    .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                        "{} · {} 个素材{}",
                        timestamp(note.updated_at),
                        media,
                        if note.deleted_at.is_some() {
                            " · 已在回收站"
                        } else {
                            ""
                        }
                    )))
                    .child(
                        div()
                            .id(SharedString::from(format!("version-{}", note.id)))
                            .h(px(260.))
                            .overflow_y_scroll()
                            .child(format!("{}\n{}", note.title, note.text())),
                    )
            };
            dialog
                .title("发现两个版本，请选择保留方式")
                .w(px(690.))
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(preview(&fork, "本地修改（离线副本）"))
                        .child(preview(&cloud, "云端版本")),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child("未选中的版本会保留在历史或回收站中。关闭此窗口可稍后处理。"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xa34c35))
                        .child(error.read(cx).clone()),
                )
                .child(actions)
        });
    }
    fn confirm_delete(&mut self, nid: String, w: &mut Window, cx: &mut Context<Self>) {
        let target = cx.entity().downgrade();
        w.open_dialog(cx, move |dialog, _, _| {
            let target = target.clone();
            let nid = nid.clone();
            dialog
                .title("移入回收站？")
                .w(px(340.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(icon("trash").size(px(22.)).text_color(rgb(0xb4473b)))
                        .child("可在回收站恢复此便签。"),
                )
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("移入回收站")
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text("取消"),
                )
                .on_ok(move |_, _, cx| {
                    target
                        .update(cx, |v, cx| {
                            let note = v
                                .shared
                                .borrow()
                                .store
                                .notes
                                .iter()
                                .find(|n| n.id == nid)
                                .cloned();
                            if let Some(mut n) = note {
                                n.deleted_at = Some(now());
                                let result = v.shared.borrow_mut().store.save_note(n);
                                let ok = result.is_ok();
                                v.result(result);
                                v.shared.borrow_mut().generation += 1;
                                cx.notify();
                                ok
                            } else {
                                true
                            }
                        })
                        .unwrap_or(false)
                })
        });
    }
    fn create_group_dialog(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.group_dialog(None, w, cx);
    }
    fn group_dialog(&mut self, existing: Option<Group>, w: &mut Window, cx: &mut Context<Self>) {
        self.group_name.update(cx, |s, cx| {
            s.set_value(
                existing
                    .as_ref()
                    .map(|g| g.name.clone())
                    .unwrap_or_default(),
                w,
                cx,
            )
        });
        let input = self.group_name.clone();
        let target = cx.entity().downgrade();
        let error = cx.new(|_| String::new());
        let color = cx.new(|_| existing.as_ref().map(|g| g.color).unwrap_or(TEAL));
        let chosen_icon = cx.new(|_| {
            existing
                .as_ref()
                .map(|g| group_icon(&g.icon).to_string())
                .unwrap_or_else(|| "inbox".into())
        });
        w.open_dialog(cx, move |dialog, _, cx| {
            let target = target.clone();
            let input = input.clone();
            let error_submit = error.clone();
            let selected_color = color.clone();
            let icon_submit = chosen_icon.clone();
            let editing = existing.clone();
            let mut palette = div()
                .w_full()
                .flex_shrink_0()
                .flex()
                .flex_wrap()
                .gap(px(16.))
                .py_2();
            for value in GROUP_COLORS {
                let choice = color.clone();
                palette = palette.child(
                    div()
                        .id(("group-color", value as usize))
                        .size(px(32.))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(rgb(value))
                        .border_2()
                        .border_color(rgb(if *color.read(cx) == value { INK } else { PAPER }))
                        .cursor_pointer()
                        .on_click(move |_, w, cx| {
                            choice.update(cx, |c, cx| {
                                *c = value;
                                cx.notify();
                            });
                            w.refresh();
                        }),
                );
            }
            let mut choices = div()
                .w_full()
                .flex_shrink_0()
                .flex()
                .flex_wrap()
                .gap(px(8.));
            for (key, _label) in GROUP_ICONS {
                let selection = chosen_icon.clone();
                choices = choices.child(
                    div()
                        .id(SharedString::from(format!("group-icon-{key}")))
                        .size(px(40.))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(rgb(if chosen_icon.read(cx) == key {
                            *color.read(cx)
                        } else {
                            LINE
                        }))
                        .bg(rgb(if chosen_icon.read(cx) == key {
                            0xe5ece5
                        } else {
                            CARD
                        }))
                        .cursor_pointer()
                        .text_size(px(11.))
                        .child(icon(key).size(px(20.)).text_color(rgb(*color.read(cx))))
                        .on_click(move |_, w, cx| {
                            selection.update(cx, |s, cx| {
                                *s = key.into();
                                cx.notify();
                            });
                            w.refresh();
                        }),
                );
            }
            let preview = div()
                .flex()
                .items_center()
                .gap_2()
                .child(icon(group_icon(chosen_icon.read(cx))).text_color(rgb(*color.read(cx))))
                .px_3()
                .py_2()
                .rounded(px(6.))
                .bg(rgba((*color.read(cx) << 8) | 0x12))
                .child(if input.read(cx).value().trim().is_empty() {
                    "分类预览".to_string()
                } else {
                    input.read(cx).value().to_string()
                });
            dialog
                .title(if existing.is_some() {
                    "编辑分类"
                } else {
                    "新建分类"
                })
                .w(px(352.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(if existing.is_some() {
                            "保存"
                        } else {
                            "创建"
                        })
                        .cancel_text("取消"),
                )
                .child(preview.flex_shrink_0().mb(px(8.)))
                .child(div().text_sm().text_color(rgb(MUTED)).child("分类名称"))
                .child(
                    div()
                        .w_full()
                        .h(px(38.))
                        .flex_shrink_0()
                        .child(Input::new(&input).w(px(296.)).h(px(38.))),
                )
                .child(div().mt_2().text_sm().text_color(rgb(MUTED)).child("颜色"))
                .child(palette)
                .child(div().mt_2().text_sm().text_color(rgb(MUTED)).child("图标"))
                .child(choices)
                .when(!error.read(cx).is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(0xa9472f))
                            .child(error.read(cx).clone()),
                    )
                })
                .on_ok(move |_, w, cx| {
                    target
                        .update(cx, |v, cx| {
                            let name = input.read(cx).value().trim().to_string();
                            let result = if let Some(mut group) = editing.clone() {
                                if name.is_empty()
                                    || v.shared
                                        .borrow()
                                        .store
                                        .groups
                                        .iter()
                                        .any(|g| g.id != group.id && g.name == name)
                                {
                                    error_submit.update(cx, |s, cx| {
                                        *s = "请输入不重复的分类名称".into();
                                        cx.notify();
                                    });
                                    w.refresh();
                                    return false;
                                }
                                group.name = name.clone();
                                group.color = *selected_color.read(cx);
                                group.icon = icon_submit.read(cx).clone();
                                v.shared
                                    .borrow_mut()
                                    .store
                                    .save_group(group.clone())
                                    .map(|_| group)
                            } else {
                                v.shared.borrow_mut().store.create_group_with_style(
                                    &name,
                                    *selected_color.read(cx),
                                    icon_submit.read(cx),
                                )
                            };
                            match result {
                                Ok(group) => {
                                    v.flush(cx);
                                    if editing.is_some() {
                                        // Appearance changes do not move notes.
                                    } else if let Some(mut note) = v.current_note() {
                                        note.group_id = group.id.clone();
                                        let result = v.shared.borrow_mut().store.save_note(note);
                                        if let Err(e) = result {
                                            error_submit.update(cx, |text, cx| {
                                                *text = e.to_string();
                                                cx.notify();
                                            });
                                            return false;
                                        }
                                    } else {
                                        v.group = Some(group.id);
                                        v.trash = false;
                                    }
                                    v.shared.borrow_mut().generation += 1;
                                    cx.notify();
                                    true
                                }
                                Err(e) => {
                                    error_submit.update(cx, |text, cx| {
                                        *text = e.to_string();
                                        cx.notify();
                                    });
                                    w.refresh();
                                    false
                                }
                            }
                        })
                        .unwrap_or(true)
                })
        });
        self.group_name.update(cx, |s, cx| s.focus(w, cx));
    }
    fn group_selector(&mut self, n: &Note, g: &Group, cx: &mut Context<Self>) -> AnyElement {
        let note_id = n.id.clone();
        let group_view = cx.entity().downgrade();
        Button::new(SharedString::from(format!("group-dropdown-{}", n.id)))
            .ghost()
            .px(px(8.))
            .rounded(px(5.))
            .bg(rgba((g.color << 8) | 0x12))
            .child(
                icon(group_icon(&g.icon))
                    .size(px(18.))
                    .text_color(rgb(g.color)),
            )
            .child(div().text_color(rgb(g.color)).child(g.name.clone()))
            .child(icon("down").size(px(12.)).text_color(rgb(g.color)))
            .h(px(32.))
            .text_size(px(15.))
            .dropdown_menu(move |mut menu, _, cx| {
                let Some(view) = group_view.upgrade() else {
                    return menu;
                };
                let groups = view.read(cx).shared.borrow().store.groups.clone();
                let selected = view
                    .read(cx)
                    .shared
                    .borrow()
                    .store
                    .notes
                    .iter()
                    .find(|n| n.id == note_id)
                    .map(|n| n.group_id.clone());
                for group in groups {
                    let gid = group.id.clone();
                    let target = group_view.clone();
                    let nid = note_id.clone();
                    menu = menu.item(
                        PopupMenuItem::element(move |_, _| {
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    icon(group_icon(&group.icon))
                                        .size(px(18.))
                                        .text_color(rgb(group.color)),
                                )
                                .child(group.name.clone())
                        })
                        .checked(selected.as_ref() == Some(&gid))
                        .on_click(move |_, _, cx| {
                            let _ = target.update(cx, |v, cx| {
                                v.flush(cx);
                                let note = v
                                    .shared
                                    .borrow()
                                    .store
                                    .notes
                                    .iter()
                                    .find(|n| n.id == nid)
                                    .cloned();
                                if let Some(mut n) = note {
                                    n.group_id = gid.clone();
                                    let result = v.shared.borrow_mut().store.save_note(n);
                                    v.result(result);
                                    v.shared.borrow_mut().generation += 1;
                                }
                                cx.notify();
                            });
                        }),
                    );
                }
                let target = group_view.clone();
                menu.separator()
                    .item(PopupMenuItem::new("＋ 新建分组").on_click(move |_, w, cx| {
                        let _ = target.update(cx, |v, cx| {
                            v.create_group_dialog(w, cx);
                        });
                    }))
            })
            .into_any_element()
    }
    fn note(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let width = if self.embedded {
            self.content_width
        } else {
            f32::from(window.viewport_size().width) - 44.
        };
        self.editor.update(cx, |e, _| e.set_media_width(px(width)));
        let Some(n) = self.current_note() else {
            return div().child("便签不存在").into_any_element();
        };
        let groups = self.shared.borrow().store.groups.clone();
        let g = groups
            .iter()
            .find(|g| g.id == n.group_id)
            .unwrap_or(&groups[0]);
        let group_control = self.group_selector(&n, g, cx);
        #[cfg(windows)]
        let pin_button = Some(
            button("pin-window", if self.pinned { "pin-filled" } else { "pin" })
                .when(self.pinned, |d| d.bg(rgb(0xd9e3d9)))
                .on_click(cx.listener(|v, _, w, cx| {
                    let result = crate::platform::set_pinned(w, !v.pinned);
                    if result.is_ok() {
                        v.pinned = !v.pinned;
                    }
                    v.result(result);
                    cx.notify();
                })),
        );
        #[cfg(target_os = "linux")]
        let pin_button = None::<Stateful<Div>>;
        let mut header = div()
            .flex()
            .items_center()
            .h(px(44.))
            .flex_shrink_0()
            .gap(px(6.))
            .px(px(16.))
            .border_b_1()
            .border_color(rgb(LINE))
            .child(div().w(px(3.)).h(px(18.)).rounded(px(1.)).bg(rgb(g.color)))
            .child(group_control)
            .child(
                div()
                    .id("note-header-drag")
                    .flex_1()
                    .h_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|v, _, w, cx| {
                            let result = crate::platform::drag_window(w);
                            v.result(result);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .children(pin_button)
            .child(
                button("note-center", "grid").on_click(cx.listener(|v, _, w, cx| {
                    v.flush(cx);
                    if v.has_unsaved_edits(cx) {
                        return;
                    }
                    if let Some(recorder) = &v.recorder {
                        recorder.stop();
                        v.saving_audio = true;
                        v.shared.borrow_mut().status = "正在保存录音，完成后可打开便签中心".into();
                        cx.notify();
                        return;
                    }
                    open(v.shared.clone(), Screen::Center, cx);
                    let center = v.shared.borrow().windows.get("center").copied();
                    if center.is_some_and(|handle| handle.update(cx, |_, _, _| ()).is_ok()) {
                        w.remove_window();
                    }
                })),
            )
            .child(
                button("note-trash", "trash").on_click(cx.listener(|v, _, w, cx| {
                    if v.recorder.is_some() {
                        v.shared.borrow_mut().status = "请先结束录音再删除便签".into();
                        cx.notify();
                        return;
                    }
                    v.flush(cx);
                    if let Some(note) = v.current_note() {
                        v.confirm_delete(note.id, w, cx);
                    }
                })),
            );
        header = header.child(button("close-note", "close").on_click(cx.listener(
            |v, _, w, cx| {
                v.close_note(w, cx);
            },
        )));
        let mut content = div()
            .w_full()
            .px(px(22.))
            .py(px(12.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2();
        content = content.child(self.editor.clone());
        let related: Vec<_> = self
            .shared
            .borrow()
            .store
            .notes
            .iter()
            .filter(|other| {
                other.deleted_at.is_none()
                    && other.id != n.id
                    && (n.links.contains(&other.id) || other.links.contains(&n.id))
            })
            .cloned()
            .collect();
        if !related.is_empty() {
            let mut links = div().flex().flex_wrap().gap_2();
            for other in related {
                let nid = other.id;
                links = links.child(
                    label_button(
                        SharedString::from(format!("related-{nid}")),
                        format!("↗ {}", other.title),
                    )
                    .text_color(rgb(TEAL))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        open(v.shared.clone(), Screen::Note(nid.clone()), cx)
                    })),
                );
            }
            content = content.child(links);
        }
        if n.deleted_at.is_some() {
            content = content.child(
                label_button("restore-note", "恢复便签")
                    .bg(rgb(TEAL))
                    .text_color(rgb(PAPER))
                    .on_click(cx.listener(|v, _, _, cx| {
                        if let Some(mut note) = v.current_note() {
                            note.deleted_at = None;
                            let result = v.shared.borrow_mut().store.save_note(note);
                            v.result(result);
                            v.shared.borrow_mut().generation += 1;
                        }
                        cx.notify();
                    })),
            );
        }
        let tool_size = ((width - 36.) / 10.).clamp(20., 28.);
        let mut toolbar = div()
            .flex()
            .items_center()
            .justify_start()
            .gap(px(2.))
            .px(px(12.))
            .h(px(44.))
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(LINE));
        for (key, name) in [
            ("bold", "bold"),
            ("italic", "italic"),
            ("todo", "check"),
            ("list", "list"),
            ("ordered", "ordered"),
            ("quote", "quote"),
            ("code", "code"),
            ("link", "link"),
            ("attach", "file"),
            ("record", "mic"),
        ] {
            toolbar = toolbar.child(
                button(key, name)
                    .size(px(tool_size))
                    .when(matches!(key, "todo" | "quote" | "link"), |d| d.ml(px(4.)))
                    .when(key == "record" && self.recorder.is_some(), |d| {
                        d.text_color(rgb(0xb34f3f))
                    })
                    .on_click(cx.listener(move |v, _, w, cx| match key {
                        "attach" => v.attach(w, cx),
                        "record" => v.toggle_record(cx),
                        "link" => v.insert("[文字](https://)", w, cx),
                        _ => v.editor.update(cx, |e, cx| e.format(key, w, cx)),
                    })),
            );
        }
        if self.embedded {
            let now = Instant::now();
            let visible = self.card_selected && window.is_window_active();
            let target = if visible { 1. } else { 0. };
            let reduced = self.shared.borrow().store.settings.reduced_motion;
            let step = now
                .duration_since(self.toolbar_frame)
                .as_secs_f32()
                .min(0.04)
                / 0.16;
            self.toolbar_frame = now;
            self.toolbar_fraction = if reduced {
                target
            } else if visible {
                (self.toolbar_fraction + step).min(1.)
            } else {
                (self.toolbar_fraction - step).max(0.)
            };
            let reveal = self.toolbar_fraction;
            if (reveal - target).abs() > 0.001 {
                let view = cx.entity().downgrade();
                window.on_next_frame(move |_, cx| {
                    let _ = view.update(cx, |_, cx| cx.notify());
                });
            }
            let scroll = self.editor.read(cx).scroll_handle();
            let content = content.px_0().pt_0().pb(px(4.));
            return div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(8. * reveal))
                .child(
                    div()
                        .id("embedded-scroll")
                        .max_h(px(400.))
                        .overflow_y_hidden()
                        .track_scroll(&scroll)
                        .child(content),
                )
                .child(
                    div()
                        .h(px(56. * reveal))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .child(
                            toolbar
                                .px_0()
                                .pt(px(8.))
                                .h(px(56.))
                                .opacity(reveal)
                                .when(reveal == 0., |d| d.invisible()),
                        ),
                )
                .into_any_element();
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("note-top-drag")
                    .w_full()
                    .h(px(6.))
                    .flex_shrink_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|v, _, w, cx| {
                            let result = crate::platform::drag_window(w);
                            v.result(result);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(header)
            .child(
                div()
                    .id("note-content")
                    .track_scroll(&self.editor.read(cx).scroll_handle())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
            .child(toolbar)
            .into_any_element()
    }
    fn save_settings_automatically(&mut self, cx: &mut Context<Self>) {
        let result = self
            .shared
            .borrow_mut()
            .store
            .save_settings(self.settings.clone());
        if let Err(error) = result {
            self.settings = self.shared.borrow().store.settings.clone();
            self.shared.borrow_mut().status = format!("设置保存失败：{error}");
        }
        self.shared.borrow_mut().generation += 1;
        cx.notify();
    }

    fn download_speech_model(&mut self, model: &'static str, cx: &mut Context<Self>) {
        let destination = {
            let mut runtime = self.shared.borrow_mut();
            if runtime.model_downloads.contains_key(model) {
                return;
            }
            runtime.model_downloads.insert(model.into(), 0);
            runtime.model_errors.remove(model);
            runtime
                .store
                .root
                .join("models")
                .join(format!("ggml-{model}.bin"))
        };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = audio::download_model(model, &destination, |percent| {
                let _ = tx.send(Job::ModelProgress(model.into(), percent));
            })
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(Job::ModelDownloaded(model.into(), destination, result));
        });
        cx.notify();
    }

    fn account_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let address =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://sync.example.com"));
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("账号"));
        let secret = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("密码"));
        let target = cx.entity().downgrade();
        let error = cx.new(|_| String::new());
        window.open_dialog(cx, move |dialog, _, cx| {
            let target = target.clone();
            let address_submit = address.clone();
            let username_submit = username.clone();
            let secret_submit = secret.clone();
            let error_submit = error.clone();
            dialog
                .title("登录同步账号")
                .w(px(460.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("登录")
                        .cancel_text("取消"),
                )
                // 显式内容宽度避免百分比输入框在弹窗滚动容器中按最小宽度收缩。
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .w(px(404.))
                        .child(div().text_sm().child("同步服务器网址或 IP"))
                        .child(Input::new(&address).w(px(404.)).h(px(36.)))
                        .child(
                            label_button("sync-deploy-guide", "查看服务器部署指南").on_click(
                                |_, _, _| {
                                    let _ = webbrowser::open(SYNC_DEPLOYMENT_GUIDE);
                                },
                            ),
                        )
                        .child(div().text_sm().child("账号 / 邮箱"))
                        .child(Input::new(&username).w(px(404.)).h(px(36.)))
                        .child(div().text_sm().child("密码"))
                        .child(Input::new(&secret).w(px(404.)).h(px(36.))),
                )
                .when(!error.read(cx).is_empty(), |d| {
                    d.child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa9472f))
                            .child(error.read(cx).clone()),
                    )
                })
                .on_ok(move |_, _, cx| {
                    let address = address_submit.read(cx).value();
                    let url = match sync::parse_server_address(&address) {
                        Ok(url) => url,
                        Err(error) => {
                            error_submit.update(cx, |e, cx| {
                                *e = error.to_string();
                                cx.notify();
                            });
                            return false;
                        }
                    };
                    let username = username_submit.read(cx).value().trim().to_string();
                    let secret =
                        zeroize::Zeroizing::new(secret_submit.read(cx).value().to_string());
                    if username.is_empty() || secret.is_empty() {
                        error_submit.update(cx, |e, cx| {
                            *e = "请输入账号和密码".into();
                            cx.notify();
                        });
                        return false;
                    }
                    target
                        .update(cx, |v, cx| {
                            {
                                let mut runtime = v.shared.borrow_mut();
                                if runtime.account_busy || runtime.sync_enabled {
                                    return;
                                }
                                runtime.account_busy = true;
                                runtime.account_error = None;
                                runtime.account_cancel = Arc::new(AtomicBool::new(false));
                            }
                            let cancelled = v.shared.borrow().account_cancel.clone();
                            let tx = v.tx.clone();
                            std::thread::spawn(move || {
                                let result = sync::login(&url, &username, &secret, cancelled)
                                    .map_err(|error| format!("{error:#}"));
                                let _ = tx.send(Job::Account(result));
                            });
                            cx.notify();
                        })
                        .is_ok()
                })
        });
    }

    fn settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.settings = self.shared.borrow().store.settings.clone();
        let runtime = self.shared.borrow();
        let sync_status = runtime.sync_status.clone();
        let sync_enabled = runtime.sync_enabled;
        let sync_success = runtime.sync_success;
        let pending = sync::pending_count(&runtime.store).unwrap_or(0);
        let account = sync::config(&runtime.store);
        let account_busy = runtime.account_busy;
        let account_error = runtime.account_error.clone();
        let sync_space = runtime.sync_usage.as_ref().map(|u| {
            format!(
                "云端空间 {:.2} / {:.0} MiB",
                u.used_bytes as f64 / 1048576.,
                u.quota_bytes as f64 / 1048576.
            )
        });
        let root = runtime.store.root.clone();
        let downloads = runtime.model_downloads.clone();
        let errors = runtime.model_errors.clone();
        drop(runtime);
        let mut account_panel = div()
            .flex()
            .flex_col()
            .gap_2()
            .p(px(20.))
            .bg(rgb(CARD))
            .rounded(px(7.))
            .border_1()
            .border_color(rgb(LINE));
        account_panel = match account {
            Ok(Some(config)) => account_panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .flex_wrap()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("同步账号已登录"),
                            )
                            .child(div().text_sm().child(format!("账号 {}", config.name)))
                            .child(div().text_sm().text_color(rgb(MUTED)).child(config.url)),
                    )
                    .child(
                        label_button("account-manage", "账号管理 / 修改密码")
                            .border_1()
                            .border_color(rgb(0xb8c9c2))
                            .bg(rgb(0xe8eee8))
                            .text_color(rgb(TEAL))
                            .on_click({
                                let issuer = config.issuer.clone();
                                cx.listener(move |v, _, _, cx| {
                                    let result = (|| -> anyhow::Result<()> {
                                        let mut url = suojian::auth::secure_url(&issuer)?;
                                        url.set_path("/account");
                                        webbrowser::open(url.as_str())?;
                                        Ok(())
                                    })();
                                    if let Err(error) = result {
                                        v.shared.borrow_mut().account_error =
                                            Some(error.to_string());
                                        cx.notify();
                                    }
                                })
                            }),
                    )
                    .child(
                        label_button("account-logout", "退出登录")
                            .border_1()
                            .border_color(rgb(LINE))
                            .bg(rgb(0xf3eee5))
                            .text_color(rgb(0x8e4b3f))
                            .on_click(cx.listener(|v, _, _, cx| {
                                let mut runtime = v.shared.borrow_mut();
                                match sync::logout(&runtime.store) {
                                    Ok(config) => {
                                        runtime.sync_enabled = false;
                                        runtime.sync_more = false;
                                        runtime.sync_usage = None;
                                        runtime.account_error = None;
                                        runtime.sync_status =
                                            "已退出登录，本地便签可继续使用".into();
                                        if let Some(config) = config {
                                            let tx = v.tx.clone();
                                            std::thread::spawn(move || {
                                                let result = sync::revoke_session(&config)
                                                    .map_err(|e| format!("{e:#}"));
                                                let _ = tx.send(Job::LoggedOut(result));
                                            });
                                        }
                                    }
                                    Err(error) => runtime.account_error = Some(error.to_string()),
                                }
                                cx.notify();
                            })),
                    ),
            ),
            Ok(None) => account_panel
                .child(div().font_weight(FontWeight::SEMIBOLD).child("未登录"))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child("本地便签可直接使用。同步需要自行部署服务器并登录。"),
                )
                .when(account_busy, |d| {
                    d.child(div().text_sm().child("正在登录…")).child(
                        label_button("account-cancel", "取消登录").on_click(cx.listener(
                            |v, _, _, cx| {
                                v.shared
                                    .borrow()
                                    .account_cancel
                                    .store(true, Ordering::Relaxed);
                                cx.notify();
                            },
                        )),
                    )
                })
                .when(!account_busy, |d| {
                    d.child(
                        div().flex().child(
                            label_button("account-login", "登录账号")
                                .bg(rgb(TEAL))
                                .text_color(rgb(PAPER))
                                .on_click(cx.listener(|v, _, w, cx| v.account_dialog(w, cx))),
                        ),
                    )
                }),
            Err(error) => account_panel
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xa9472f))
                        .child(format!("账号信息读取失败：{error}")),
                )
                .child(
                    label_button("account-repair", "重新登录").on_click(cx.listener(
                        |v, _, w, cx| {
                            {
                                let mut runtime = v.shared.borrow_mut();
                                match sync::logout(&runtime.store) {
                                    Ok(_) => {
                                        runtime.sync_enabled = false;
                                        runtime.sync_more = false;
                                    }
                                    Err(error) => {
                                        runtime.account_error = Some(error.to_string());
                                        return;
                                    }
                                }
                            }
                            v.account_dialog(w, cx);
                        },
                    )),
                ),
        };
        if let Some(error) = account_error {
            account_panel =
                account_panel.child(div().text_sm().text_color(rgb(0xa9472f)).child(error));
        }
        let mut models = div()
            .flex()
            .flex_col()
            .bg(rgb(CARD))
            .border_1()
            .border_color(rgb(LINE))
            .rounded(px(7.))
            .child(
                toggle_row("auto", "录音后自动转写", self.settings.auto_transcribe).on_click(
                    cx.listener(|v, _, _, cx| {
                        v.settings.auto_transcribe = !v.settings.auto_transcribe;
                        v.save_settings_automatically(cx);
                    }),
                ),
            );
        for (model, label, size_label) in [
            ("tiny", "轻量", "约 75 MB"),
            ("base", "均衡", "约 142 MB"),
            ("small", "高精度", "约 466 MB"),
        ] {
            let selected = self.settings.speech_model == model;
            let installed = audio::model_path(&self.settings, &root, model).is_file();
            let progress = downloads.get(model).copied();
            let mut row = div()
                .flex()
                .items_center()
                .gap_2()
                .px(px(18.))
                .py(px(14.))
                .border_t_1()
                .border_color(rgb(LINE))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(label))
                        .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                            "{size_label} · {}",
                            if installed { "已下载" } else { "未下载" }
                        ))),
                )
                .child(
                    label_button(
                        SharedString::from(format!("select-{model}")),
                        if selected { "已选中" } else { "选择" },
                    )
                    .when(selected, |d| {
                        d.text_color(rgb(TEAL)).font_weight(FontWeight::SEMIBOLD)
                    })
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.settings = v.shared.borrow().store.settings.clone();
                        let root = v.shared.borrow().store.root.clone();
                        let path = audio::model_path(&v.settings, &root, model);
                        v.settings.speech_model = model.into();
                        v.settings.model_path = path.to_string_lossy().into_owned();
                        v.save_settings_automatically(cx);
                    })),
                );
            row = if let Some(percent) = progress {
                row.child(
                    div()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child(format!("下载中 {percent}%")),
                )
            } else if !installed {
                row.child(
                    label_button(
                        SharedString::from(format!("download-{model}")),
                        if errors.contains_key(model) {
                            "重试下载"
                        } else {
                            "下载"
                        },
                    )
                    .on_click(cx.listener(move |v, _, _, cx| v.download_speech_model(model, cx))),
                )
            } else {
                row
            };
            models = models.child(row);
            if let Some(error) = errors.get(model) {
                models = models.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xa9472f))
                        .child(error.clone()),
                );
            }
        }
        let selected_ready =
            audio::model_path(&self.settings, &root, &self.settings.speech_model).is_file();
        let page_title = match self.settings_tab {
            0 => "常规",
            1 => "账号与同步",
            2 => "数据管理",
            3 => "分组管理",
            _ => "关于与更新",
        };
        let mut panel = div()
            .w_full()
            .max_w(px(920.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div().flex().flex_col().gap(px(6.)).pb(px(10.)).child(
                    div()
                        .text_size(px(25.))
                        .font_weight(FontWeight::BOLD)
                        .child(page_title),
                ),
            );
        panel = match self.settings_tab {
            0 => panel
                .child(self.general_panel(cx))
                .child(settings_heading("语音转写"))
                .child(models)
                .when(!selected_ready, |d| {
                    d.child(
                        div()
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child("所选模型下载完成后即可转写，录音会保留。"),
                    )
                }),
            1 => panel
                .child(account_panel)
                .when(sync_enabled, |d| {
                    d.child(div().text_sm().text_color(rgb(MUTED)).child(format!(
                            "上次成功同步：{}　·　待同步变更：{} 项",
                            sync_success
                                .map(timestamp)
                                .unwrap_or_else(|| "本次启动尚未同步".into()),
                            pending
                        )))
                })
                .when(sync_enabled, |d| {
                    d.child(settings_heading("数据同步")).child(
                        div()
                            .bg(rgb(CARD))
                            .border_1()
                            .border_color(rgb(LINE))
                            .rounded(px(7.))
                            .p(px(18.))
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .items_start()
                                    .gap_2()
                                    .child(
                                        div()
                                            .mt(px(5.))
                                            .size(px(7.))
                                            .flex_shrink_0()
                                            .rounded_full()
                                            .bg(rgb(
                                                if sync_status.starts_with("暂未同步")
                                                    || sync_status.contains("失败")
                                                {
                                                    0xb9644b
                                                } else {
                                                    TEAL
                                                },
                                            )),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_sm()
                                            .text_color(rgb(
                                                if sync_status.starts_with("暂未同步")
                                                    || sync_status.contains("失败")
                                                {
                                                    0x9b4b39
                                                } else {
                                                    INK
                                                },
                                            ))
                                            .child(sync_status.clone()),
                                    ),
                            )
                            .when_some(sync_space, |d, space| d.child(div().text_sm().child(space)))
                            .child(div().text_sm().child(
                                if self.shared.borrow().dirty_editors.get() == 0 {
                                    "本机修改已保存"
                                } else {
                                    "正在保存本机修改…"
                                },
                            ))
                            .child(
                                div().flex().items_center().child(
                                    Button::new("sync-now")
                                        .primary()
                                        .label("立即同步")
                                        .on_click(cx.listener(|v, _, _, cx| {
                                            v.flush_cards(cx);
                                            v.shared.borrow_mut().sync_last =
                                                Instant::now() - Duration::from_secs(10);
                                            cx.notify();
                                        })),
                                ),
                            ),
                    )
                }),
            2 => panel.child(self.data_panel(f32::from(window.viewport_size().width) < 1050., cx)),
            3 => panel.child(self.groups_panel(cx)),
            _ => panel
                .child(
                    div()
                        .text_size(px(21.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("闪记"),
                )
                .child(div().child(format!("版本 {}", env!("CARGO_PKG_VERSION"))))
                .child(
                    label_button("check-updates", "查看发布版本")
                        .border_1()
                        .border_color(rgb(LINE))
                        .bg(rgb(CARD))
                        .on_click(cx.listener(|v, _, _, _| {
                            v.result(
                                webbrowser::open("https://github.com/Puring103/FlashNote/releases")
                                    .map_err(Into::into),
                            );
                        })),
                )
                .child(
                    label_button("open-app-dir", "打开程序目录")
                        .border_1()
                        .border_color(rgb(LINE))
                        .bg(rgb(CARD))
                        .on_click(cx.listener(|v, _, _, _| {
                            let result = std::env::current_exe()
                                .map_err(anyhow::Error::from)
                                .and_then(|p| {
                                    let directory = p
                                        .parent()
                                        .ok_or_else(|| anyhow::anyhow!("无法定位程序所在目录"))?;
                                    crate::platform::open_location(directory)
                                });
                            v.result(result);
                        })),
                )
                .child(
                    label_button("quit-app", "退出闪记")
                        .mt(px(12.))
                        .text_color(rgb(0xb4473b))
                        .on_click(cx.listener(|v, _, _, cx| {
                            request_quit(v.shared.clone(), cx);
                        })),
                ),
        };
        let mut sidebar = div()
            .w(px(190.))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(rgb(LINE))
            .bg(rgb(0xf2eee5))
            .px(px(12.))
            .py(px(20.))
            .flex()
            .flex_col()
            .gap(px(4.));
        for (index, (title, glyph)) in [
            ("常规", "settings"),
            ("账号与同步", "link"),
            ("数据管理", "file"),
            ("分组管理", "folder"),
            ("关于与更新", "book"),
        ]
        .into_iter()
        .enumerate()
        {
            sidebar = sidebar.child(
                div()
                    .id(SharedString::from(format!("settings-tab-{index}")))
                    .h(px(42.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(11.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .text_size(px(14.))
                    .hover(|d| d.bg(rgb(0xe9e9df)))
                    .text_color(rgb(if self.settings_tab == index {
                        TEAL
                    } else {
                        MUTED
                    }))
                    .when(self.settings_tab == index, |d| {
                        d.bg(rgb(0xdde7df)).font_weight(FontWeight::SEMIBOLD)
                    })
                    .child(icon(glyph).size(px(17.)))
                    .child(title)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.settings_tab = index;
                        if index == 2 {
                            v.refresh_storage(cx);
                        }
                        cx.notify();
                    })),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(64.))
                    .flex_shrink_0()
                    .px(px(24.))
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(rgb(LINE))
                    .child(
                        label_button("settings-back", "‹ 返回便签").on_click(cx.listener(
                            |v, _, _, cx| {
                                v.screen = Screen::Center;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(22.))
                            .font_weight(FontWeight::BOLD)
                            .window_control_area(WindowControlArea::Drag)
                            .child("设置"),
                    )
                    .child(
                        button("settings-min", "minus")
                            .on_click(cx.listener(|_, _, w, _| w.minimize_window())),
                    )
                    .child(
                        button("settings-max", "maximize")
                            .on_click(cx.listener(|_, _, w, _| w.zoom_window())),
                    )
                    .child(
                        button("settings-close", "close")
                            .on_click(cx.listener(|_, _, w, _| w.remove_window())),
                    ),
            )
            .child(
                div().flex().flex_1().min_h_0().child(sidebar).child(
                    div()
                        .id("settings-scroll")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .px(px(32.))
                        .pt(px(30.))
                        .pb(px(48.))
                        .child(div().w_full().flex().justify_center().child(panel)),
                ),
            )
            .into_any_element()
    }
}
impl Render for View {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.embedded {
            let body = self.note(window, cx);
            return div()
                .w_full()
                .flex_shrink_0()
                .child(body)
                .on_drop(cx.listener(|v, paths: &ExternalPaths, w, cx| {
                    v.import_paths(paths.paths(), w, cx);
                    cx.stop_propagation();
                }))
                .on_key_down(cx.listener(|v, e: &KeyDownEvent, w, cx| {
                    if e.keystroke.modifiers.control && e.keystroke.key == "v" {
                        v.paste_image(w, cx);
                    }
                }))
                .into_any_element();
        }
        let body = match self.screen {
            Screen::Center => self.center(window, cx),
            Screen::Note(_) => self.note(window, cx),
            Screen::Settings => self.settings(window, cx),
        };
        let status = self.shared.borrow().status.clone();
        let dialogs = Root::render_dialog_layer(window, cx);
        div()
            .size_full()
            .bg(rgb(PAPER))
            .track_focus(&self.page_focus)
            .text_color(rgb(INK))
            .font_family(crate::platform::UI_FONT)
            .text_size(px(16.))
            .flex()
            .flex_col()
            .on_drop(
                cx.listener(|v, paths: &ExternalPaths, w, cx| v.import_paths(paths.paths(), w, cx)),
            )
            .on_key_down(cx.listener(|v, e: &KeyDownEvent, w, cx| {
                if w.has_active_dialog(cx) {
                    return;
                }
                if e.keystroke.modifiers.control && e.keystroke.key == "n" {
                    v.new_note(cx);
                    cx.stop_propagation();
                }
                if e.keystroke.modifiers.control && e.keystroke.key == "k" {
                    v.search.update(cx, |s, cx| s.focus(w, cx));
                }
                if e.keystroke.modifiers.control && e.keystroke.key == "v" {
                    v.paste_image(w, cx);
                }
                if e.keystroke.key == "escape" {
                    if v.multi {
                        v.multi = false;
                        v.selection.clear();
                        cx.notify();
                    } else if matches!(v.screen, Screen::Settings) {
                        v.screen = Screen::Center;
                        cx.notify();
                    } else {
                        v.flush(cx);
                        w.minimize_window();
                    }
                }
                if v.multi && e.keystroke.modifiers.control && e.keystroke.key == "a" {
                    let query = v.search.read(cx).value().to_string();
                    v.selection = v
                        .shared
                        .borrow()
                        .store
                        .notes
                        .iter()
                        .filter(|n| {
                            !n.is_empty()
                                && n.deleted_at.is_some() == v.trash
                                && v.group.as_ref().is_none_or(|g| g == &n.group_id)
                                && n.matches(&query)
                        })
                        .map(|n| n.id.clone())
                        .collect();
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(div().flex_1().min_h_0().child(body))
            .when(!status.is_empty(), |d| {
                d.child(
                    div()
                        .px_3()
                        .py_1()
                        .bg(rgb(0xe5e8df))
                        .text_xs()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().flex_1().child(status))
                        .when(self.batch_undo.is_some(), |d| {
                            d.child(
                                label_button("undo-batch", "撤销批量操作")
                                    .on_click(cx.listener(|v, _, _, cx| v.undo_batch(cx))),
                            )
                        })
                        .when(self.shared.borrow().dirty_editors.get() != 0, |d| {
                            d.child(label_button("retry-save", "重试保存").on_click(cx.listener(
                                |v, _, _, cx| {
                                    v.flush_cards(cx);
                                    cx.notify();
                                },
                            )))
                        }),
                )
            })
            .children(dialogs)
            .into_any_element()
    }
}
/// Return None at a boundary so the enclosing feed receives the wheel event.
#[cfg(test)]
fn card_scroll_step(offset: f32, maximum: f32, delta: f32) -> Option<f32> {
    let maximum = maximum.max(0.);
    let current = offset.clamp(-maximum, 0.);
    let next = (current + delta).clamp(-maximum, 0.);
    if (next - current).abs() > 0.01 {
        Some(next)
    } else {
        None
    }
}

pub fn request_quit(shared: Shared, cx: &mut App) {
    cx.defer(move |cx| {
        let editors = shared.borrow().editors.clone();
        let mut unsaved = false;
        for editor in editors {
            if let Ok(pending) = editor.update(cx, |view, cx| {
                view.flush(cx);
                view.has_unsaved_edits(cx)
            }) {
                unsaved |= pending;
            }
        }
        if unsaved {
            let mut runtime = shared.borrow_mut();
            runtime.status = "请先完成输入或处理保存冲突，再退出闪记".into();
            runtime.generation += 1;
        } else {
            cx.quit();
        }
    });
}

pub fn open_new(shared: Shared, cx: &mut App) {
    open_new_with_placement(shared, true, cx);
}

#[cfg(windows)]
pub fn open_shortcut(shared: Shared, new_note: bool, cx: &mut App) {
    if new_note {
        open_new_with_placement(shared, true, cx);
    } else {
        let _ = open_with_placement(shared, Screen::Center, true, cx);
    }
}

fn open_new_with_placement(shared: Shared, force_new: bool, cx: &mut App) {
    let note = Note::new();
    let nid = note.id.clone();
    let result = shared.borrow_mut().store.save_note(note);
    match result {
        Ok(()) => {
            shared.borrow_mut().generation += 1;
            let _ = open_with_placement(shared, Screen::Note(nid), force_new, cx);
        }
        Err(e) => {
            shared.borrow_mut().status = e.to_string();
            open(shared, Screen::Center, cx);
        }
    }
}

pub fn open(shared: Shared, screen: Screen, cx: &mut App) {
    let _ = open_with_placement(shared, screen, false, cx);
}

#[cfg(windows)]
pub fn open_cli_note(shared: Shared, id: String, cx: &mut App) -> anyhow::Result<()> {
    open_with_placement(shared, Screen::Note(id), false, cx)
}

fn open_with_placement(
    shared: Shared,
    screen: Screen,
    force_new: bool,
    cx: &mut App,
) -> anyhow::Result<()> {
    let key = match &screen {
        Screen::Center => "center".into(),
        Screen::Settings => "settings".into(),
        Screen::Note(id) => id.clone(),
    };
    let existing = if force_new {
        None
    } else {
        shared.borrow().windows.get(&key).copied()
    };
    let start = Instant::now();
    let perf_root = shared.borrow().store.root.clone();
    if let Some(handle) = existing {
        if handle
            .update(cx, move |_, w, _| {
                w.activate_window();
                w.on_next_frame(move |_, _| {
                    log_perf(&perf_root, "warm_activate_frame", start.elapsed())
                });
            })
            .is_ok()
        {
            cx.activate(true);
            return Ok(());
        }
    }
    let (width, height, title) = match &screen {
        Screen::Center => (1280., 850., "闪记"),
        Screen::Settings => (1100., 800., "闪记 · 设置"),
        Screen::Note(_) => (420., 520., "闪记 · 便签"),
    };
    let window_size = size(px(width), px(height));
    #[cfg(windows)]
    let (initial_bounds, display_id) =
        match crate::platform::cursor_window_placement((width, height), cx) {
            Ok((bounds, display)) => (bounds, Some(display)),
            Err(error) => {
                shared.borrow_mut().status = error.to_string();
                (Bounds::centered(None, window_size, cx), None)
            }
        };
    #[cfg(target_os = "linux")]
    let (initial_bounds, display_id) = (Bounds::centered(None, window_size, cx), None);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(initial_bounds)),
        display_id,
        app_id: Some("NoteHarbor.Suojian".into()),
        #[cfg(target_os = "linux")]
        window_decorations: Some(WindowDecorations::Client),
        titlebar: Some(TitlebarOptions {
            title: Some(title.into()),
            appears_transparent: true,
            ..Default::default()
        }),
        window_min_size: Some(if matches!(screen, Screen::Center | Screen::Settings) {
            size(px(900.), px(550.))
        } else {
            size(px(340.), px(360.))
        }),
        ..Default::default()
    };
    let data = shared.clone();
    let new_perf_root = shared.borrow().store.root.clone();
    let result = cx.open_window(options, move |window, cx| {
        window.set_rem_size(px(16.));
        #[cfg(windows)]
        {
            let reduced_motion = data.borrow().store.settings.reduced_motion;
            if let Err(error) = crate::platform::present_window(window, width, reduced_motion, cx) {
                data.borrow_mut().status = error.to_string();
            }
        }
        let view = cx.new(|cx| View::new(data, screen, false, window, cx));
        let focus_view = view.downgrade();
        window.on_next_frame(move |w, cx| {
            let _ = focus_view.update(cx, |v, cx| {
                if matches!(v.screen, Screen::Note(_)) {
                    v.editor.update(cx, |s, cx| s.focus(w, cx));
                } else {
                    w.focus(&v.page_focus);
                }
            });
        });
        let weak = view.downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |v, cx| {
                v.flush_cards(cx);
                if v.has_unsaved_edits(cx)
                    || v.cards
                        .values()
                        .any(|card| card.read(cx).has_unsaved_edits(cx))
                {
                    return false;
                }
                if let Some(r) = &v.recorder {
                    r.stop();
                    v.shared.borrow_mut().status = "录音保存中，请稍后关闭窗口".into();
                    false
                } else {
                    true
                }
            })
            .unwrap_or(true)
        });
        window.on_next_frame(move |_, _| {
            log_perf(&new_perf_root, "create_window_frame", start.elapsed())
        });
        cx.new(|cx| Root::new(view, window, cx))
    });
    match result {
        Ok(handle) => {
            shared.borrow_mut().windows.insert(key, handle.into());
            cx.activate(true);
            Ok(())
        }
        Err(error) => {
            eprintln!("无法打开闪记窗口：{error:#}");
            shared.borrow_mut().status = error.to_string();
            Err(error)
        }
    }
}
fn log_perf(root: &std::path::Path, event: &str, elapsed: Duration) {
    if std::env::var("SUOJIAN_PERF_LOG").as_deref() != Ok("1") {
        return;
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("performance.jsonl"))
    {
        let _ = writeln!(
            f,
            "{}",
            serde_json::json!({"event":event,"milliseconds":elapsed.as_secs_f64()*1000.})
        );
    }
}
fn timestamp(time: i64) -> String {
    let Some(date) = chrono::DateTime::from_timestamp_millis(time) else {
        return String::new();
    };
    let local = date.with_timezone(&chrono::Local);
    let today = chrono::Local::now().date_naive();
    let prefix = if local.date_naive() == today {
        "今天".to_string()
    } else if local.date_naive() == today - pchrono_day() {
        "昨天".into()
    } else {
        local.format("%m-%d").to_string()
    };
    format!("{} {}", prefix, local.format("%H:%M"))
}
fn pchrono_day() -> chrono::TimeDelta {
    chrono::TimeDelta::days(1)
}

#[cfg(all(test, windows))]
#[::core::prelude::v1::test]
fn windows_list_header_drag_moves_the_native_window() {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::{HWND, LPARAM, POINT, RECT, WPARAM},
        Graphics::Gdi::ClientToScreen,
        UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
    };

    let temp = tempfile::tempdir().unwrap();
    let shared = Rc::new(RefCell::new(Runtime::new(
        Store::open(temp.path().into()).unwrap(),
    )));
    let (tx, rx) = mpsc::channel();
    Application::new()
        .with_assets(crate::icons::Assets)
        .run(move |cx| {
            gpui_component::init(cx);
            gpui_note_editor::init(cx);
            let handle = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(200.), px(100.)),
                            size(px(1280.), px(700.)),
                        ))),
                        titlebar: Some(TitlebarOptions {
                            appears_transparent: true,
                            ..Default::default()
                        }),
                        show: false,
                        focus: false,
                        ..Default::default()
                    },
                    |window, cx| {
                        let view =
                            cx.new(|cx| View::new(shared, Screen::Center, false, window, cx));
                        cx.new(|cx| Root::new(view, window, cx))
                    },
                )
                .unwrap();
            let (hwnd, width) = handle
                .update(cx, |_, window, _| {
                    let RawWindowHandle::Win32(handle) = window.window_handle().unwrap().as_raw()
                    else {
                        unreachable!()
                    };
                    (handle.hwnd.get(), f32::from(window.viewport_size().width))
                })
                .unwrap();
            let probe = cx.background_executor().spawn(async move {
                let hwnd = HWND(hwnd as _);
                assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
                let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.;
                let send = |message, wparam, point: POINT| {
                    let coordinates = (point.x as u16 as u32) | ((point.y as u16 as u32) << 16);
                    let mut result = 0;
                    let sent = unsafe {
                        SendMessageTimeoutW(
                            hwnd,
                            message,
                            WPARAM(wparam),
                            LPARAM(coordinates as isize),
                            SMTO_ABORTIFHUNG,
                            3000,
                            Some(&mut result),
                        )
                    };
                    assert_ne!(sent.0, 0);
                    result as u32
                };
                let client = |x, y| POINT {
                    x: (x * scale) as i32,
                    y: (y * scale) as i32,
                };
                let screen = |x, y| {
                    let mut point = client(x, y);
                    assert!(unsafe { ClientToScreen(hwnd, &mut point) }.as_bool());
                    point
                };
                let hits = [
                    (92., 32.),
                    (540., 57.),
                    (width - 330., 32.),
                    (width - 44., 32.),
                ]
                .map(|(x, y)| send(WM_NCHITTEST, 0, screen(x, y)));
                let bounds = || {
                    let mut bounds = RECT::default();
                    unsafe { GetWindowRect(hwnd, &mut bounds) }.unwrap();
                    bounds
                };
                let before = bounds();
                // This point is in the fixed header area. The gap near the search
                // box depends on the desktop's font metrics and viewport width.
                let start = client(92., 32.);
                let end = POINT {
                    x: start.x + 48,
                    y: start.y + 27,
                };
                send(WM_NCLBUTTONDOWN, hits[0] as usize, screen(92., 32.));
                send(WM_MOUSEMOVE, 1, end);
                let after = bounds();
                send(WM_LBUTTONUP, 0, start);
                send(WM_MOUSEMOVE, 0, end);
                let released = bounds();
                (
                    hits,
                    (after.left - before.left, after.top - before.top),
                    (released.left - after.left, released.top - after.top),
                )
            });
            cx.spawn(async move |cx| {
                let _ = tx.send(probe.await);
                let _ = cx.update(|cx| cx.quit());
            })
            .detach();
        });
    assert_eq!(
        rx.try_recv().unwrap(),
        ([HTCAPTION, HTCAPTION, HTCLIENT, HTCLIENT], (48, 27), (0, 0))
    );
}

#[cfg(test)]
mod media_regression_tests {
    use super::{Job, Runtime, card_scroll_step};
    use crate::{model::*, test_store::open_store};
    #[test]
    fn preview_completion_evicts_cold_entries_without_flushing_visible_images() {
        let temp = tempfile::tempdir().unwrap();
        let store = open_store(temp.path().to_path_buf()).unwrap();
        let mut runtime = Runtime::new(store);
        runtime.images.insert(
            "cold".into(),
            std::sync::Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                vec![0; 33 * 1024 * 1024],
            )),
        );
        runtime.image_access.insert(
            "cold".into(),
            std::time::Instant::now() - std::time::Duration::from_secs(10),
        );
        // Exercise the actual completion path. Image decoding belongs to the
        // worker, so these cache-only bytes are intentionally not rendered.
        for index in 0..50 {
            let asset = format!("visible-{index}");
            runtime.image_loads.insert(asset.clone());
            runtime
                .job_tx
                .send(Job::ImageLoaded(asset, Ok(vec![1])))
                .unwrap();
        }
        runtime.drain_jobs();
        assert!(!runtime.images.contains_key("cold"));
        assert_eq!(runtime.images.len(), 50);
        assert!(runtime.image_loads.is_empty());
    }
    #[test]
    fn stale_transcription_job_cannot_replace_a_newer_attempt() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = open_store(temp.path().to_path_buf()).unwrap();
        let mut note = Note::new();
        let nid = note.id.clone();
        let bid = id();
        note.blocks.push(Block::Audio {
            id: bid.clone(),
            asset: id(),
            duration_ms: 1,
            text: None,
            state: AudioState::Processing,
        });
        store.save_note(note).unwrap();
        let mut runtime = Runtime::new(store);
        runtime.transcript_jobs.insert(bid.clone(), "new".into());
        let revision = runtime.store.notes[0].revision.clone();
        runtime
            .job_tx
            .send(Job::Transcript(
                nid.clone(),
                bid.clone(),
                "old".into(),
                Ok("旧结果".into()),
            ))
            .unwrap();
        runtime.drain_jobs();
        assert_eq!(runtime.store.notes[0].revision, revision);
        assert_eq!(runtime.transcript_jobs.get(&bid).unwrap(), "new");
        runtime
            .job_tx
            .send(Job::Transcript(
                nid,
                bid.clone(),
                "new".into(),
                Ok("新结果".into()),
            ))
            .unwrap();
        runtime.drain_jobs();
        assert!(!runtime.transcript_jobs.contains_key(&bid));
        assert!(runtime.store.notes[0].blocks.iter().any(|b| matches!(b, Block::Audio { text: Some(text), state: AudioState::Ready, .. } if text == "新结果")));
    }
    #[test]
    fn category_overflow_only_reserves_more_when_needed() {
        assert_eq!(super::fitting_groups(&[80., 80.], 176.), 2);
        assert_eq!(super::fitting_groups(&[80., 80.], 175.), 1);
        assert_eq!(super::fitting_groups(&[80.], 20.), 0);
        assert_eq!(super::fitting_groups(&[], 0.), 0);
    }

    #[test]
    fn card_wheel_consumes_only_real_movement() {
        assert_eq!(card_scroll_step(0., 0., -30.), None);
        assert_eq!(card_scroll_step(0., 200., 30.), None);
        assert_eq!(card_scroll_step(0., 200., -30.), Some(-30.));
        assert_eq!(card_scroll_step(-200., 200., -30.), None);
        assert_eq!(card_scroll_step(-200., 200., 30.), Some(-170.));
        assert_eq!(card_scroll_step(-190., 200., -30.), Some(-200.));
        assert_eq!(card_scroll_step(-200., 300., -30.), Some(-230.)); // transcript expanded
    }
    #[test]
    fn blank_draft_and_media_import_survive_editor_roundtrip_and_reopen() {
        let root = std::env::temp_dir().join(format!("suojian-media-regression-{}", id()));
        let mut store = open_store(root.clone()).unwrap();
        let mut note = Note::new();
        let nid = note.id.clone();
        note.set_markdown(" \n\t　".into());
        store.save_note(note).unwrap();
        let count: i64 = rusqlite::Connection::open(root.join("notes.sqlite3"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM objects WHERE kind='note'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        for prepared in [
            crate::media::prepare_bytes("file.txt".into(), b"hello".to_vec(), false).unwrap(),
            crate::media::prepare_bytes(
                "photo.jpg".into(),
                include_bytes!("../../../assets/demo/desk.jpg").to_vec(),
                false,
            )
            .unwrap(),
        ] {
            let mut editor = gpui_note_editor::document::EditorState::new(
                gpui_note_editor::document::Document::from_markdown(
                    &store.notes[0].editor_markdown(),
                ),
            );
            let asset = store.attach_prepared(&nid, prepared).unwrap();
            let mut n = store.notes[0].clone();
            let bid = n.blocks.last().unwrap().id().to_string();
            editor
                .insert_attachment(bid.clone(), "media".into(), "素材".into())
                .unwrap();
            n.set_editor_markdown(&editor.document.to_markdown());
            assert!(n.blocks.iter().any(|b| b.id() == bid));
            store.save_note(n).unwrap();
            assert!(!store.asset(&asset).unwrap().is_empty());
        }
        drop(store);
        let store = open_store(root).unwrap();
        let n = store.notes.iter().find(|n| n.id == nid).unwrap();
        assert!(n.blocks.iter().any(|b| matches!(b, Block::Image { .. })));
        assert!(n.blocks.iter().any(|b| matches!(b, Block::File { .. })));
    }
    #[test]
    fn completed_recording_replaces_placeholder_at_same_position() {
        let mut store =
            open_store(std::env::temp_dir().join(format!("suojian-record-regression-{}", id())))
                .unwrap();
        let mut n = Note::new();
        let bid = id();
        let nid = n.id.clone();
        n.blocks = vec![
            Block::Markdown {
                id: id(),
                text: "之前".into(),
            },
            Block::Audio {
                id: bid.clone(),
                asset: String::new(),
                duration_ms: 0,
                text: None,
                state: AudioState::Recorded,
            },
            Block::Markdown {
                id: id(),
                text: "之后".into(),
            },
        ];
        store.save_note(n).unwrap();
        let asset = store
            .import_bytes(include_bytes!("../../../assets/demo/sample.wav"))
            .unwrap();
        let mut runtime = Runtime::new(store);
        runtime
            .job_tx
            .send(Job::Attachment(
                nid,
                Block::Audio {
                    id: bid.clone(),
                    asset: asset.clone(),
                    duration_ms: 1000,
                    text: None,
                    state: AudioState::Recorded,
                },
            ))
            .unwrap();
        runtime.drain_jobs();
        assert!(runtime.status.is_empty());
        assert!(
            matches!(&runtime.store.notes[0].blocks[1], Block::Audio {id,asset:a,..} if id == &bid && a == &asset)
        );
        assert!(!runtime.store.asset(&asset).unwrap().is_empty());
    }
}
