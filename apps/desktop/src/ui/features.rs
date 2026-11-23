use super::*;
use gpui_component::Disableable;

impl View {
    pub(super) fn undo_batch(&mut self, cx: &mut Context<Self>) {
        self.flush_cards(cx);
        if self.shared.borrow().dirty_editors.get() != 0 {
            return;
        }
        let Some((before, after)) = self.batch_undo.take() else {
            return;
        };
        let result = self.shared.borrow_mut().store.undo_batch(&before, &after);
        if result.is_ok() {
            self.shared.borrow_mut().status = "已撤销批量操作".into();
            self.shared.borrow_mut().generation += 1;
        }
        self.result(result);
        cx.notify();
    }

    pub(super) fn refresh_storage(&mut self, cx: &mut Context<Self>) {
        let root = self.shared.borrow().store.root.clone();
        cx.spawn(async move |view, cx| {
            let bytes = cx
                .background_executor()
                .spawn(async move {
                    let assets = std::fs::read_dir(root.join("assets"))
                        .into_iter()
                        .flatten()
                        .filter_map(Result::ok)
                        .filter_map(|e| e.metadata().ok())
                        .map(|m| m.len())
                        .sum();
                    let database = ["notes.sqlite3", "notes.sqlite3-wal"]
                        .iter()
                        .filter_map(|name| std::fs::metadata(root.join(name)).ok())
                        .map(|m| m.len())
                        .sum();
                    (assets, database)
                })
                .await;
            let _ = view.update(cx, |v, cx| {
                v.storage_bytes = Some(bytes);
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn confirm_purge(
        &mut self,
        ids: Vec<String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        let target = cx.entity().downgrade();
        w.open_dialog(cx, move |dialog, _, _| {
            let ids = ids.clone(); let target = target.clone();
            dialog.title(format!("永久删除 {} 张便签？", ids.len()))
                .child(div().child("本机便签正文和历史版本将被移除，无法通过回收站恢复。删除状态会同步到其他设备。"))
                .confirm().button_props(DialogButtonProps::default().ok_text("永久删除").cancel_text("取消").ok_variant(ButtonVariant::Danger))
                .on_ok(move |_, _, cx| target.update(cx, |v, cx| {
                    v.flush_cards(cx);
                    if v.shared.borrow().sync_busy || v.shared.borrow().dirty_editors.get() != 0 { v.shared.borrow_mut().status = "正在保存或同步，请稍后再清理".into(); return false; }
                    let result = v.shared.borrow_mut().store.purge_notes(&ids);
                    let ok = result.is_ok(); v.result(result);
                    if ok { v.selection.clear(); v.multi = false; v.shared.borrow_mut().generation += 1; v.shared.borrow_mut().status = format!("已永久删除 {} 张便签", ids.len()); v.refresh_storage(cx); }
                    cx.notify(); ok
                }).unwrap_or(false))
        });
    }
    pub(super) fn flush_cards(&mut self, cx: &mut Context<Self>) {
        self.flush(cx);
        for card in self.cards.values() {
            card.update(cx, |v, cx| v.flush(cx));
        }
    }
    pub(super) fn export_archive(
        &mut self,
        ids: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.archive_busy {
            return;
        }
        self.flush_cards(cx);
        if self.shared.borrow().dirty_editors.get() != 0 {
            self.shared.borrow_mut().status = "请等待正文保存完成后重试导出".into();
            return;
        }
        let (root, manifest) = {
            let r = self.shared.borrow();
            (
                r.store.root.clone(),
                suojian::archive::manifest(&r.store, &ids),
            )
        };
        self.archive_busy = true;
        self.shared.borrow_mut().status = "正在准备导出…".into();
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let Some(path) = rfd::FileDialog::new()
                        .add_filter("闪记压缩包", &["zip"])
                        .set_file_name("闪记导出.zip")
                        .save_file()
                    else {
                        return Ok(None);
                    };
                    suojian::archive::export(root, manifest, &path).map(|_| Some(path))
                })
                .await;
            let _ = view.update_in(cx, |v, _, cx| {
                v.archive_busy = false;
                match result {
                    Ok(Some(path)) => {
                        v.multi = false;
                        v.selection.clear();
                        v.shared.borrow_mut().status = format!("已导出到 {}", path.display());
                    }
                    Ok(None) => v.shared.borrow_mut().status.clear(),
                    Err(e) => v.shared.borrow_mut().status = format!("导出失败：{e:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn choose_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.archive_busy {
            return;
        }
        self.archive_busy = true;
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async {
                    let Some(path) = rfd::FileDialog::new()
                        .add_filter("闪记压缩包", &["zip"])
                        .pick_file()
                    else {
                        return Ok(None);
                    };
                    suojian::archive::read(&path).map(Some)
                })
                .await;
            let _ = view.update_in(cx, |v, w, cx| {
                v.archive_busy = false;
                match result {
                    Ok(Some(import)) => v.preview_import(import, w, cx),
                    Ok(None) => {}
                    Err(e) => v.shared.borrow_mut().status = format!("无法导入：{e:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn preview_import(
        &mut self,
        import: suojian::archive::Import,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let duplicates = import
            .manifest
            .notes
            .iter()
            .filter(|n| {
                self.shared
                    .borrow()
                    .store
                    .notes
                    .iter()
                    .any(|old| old.id == n.id)
            })
            .count();
        let import = Rc::new(import);
        let copies = cx.new(|_| false);
        let error = cx.new(|_| String::new());
        let target = cx.entity().downgrade();
        w.open_dialog(cx, move |dialog, _, cx| {
            let toggle = copies.clone();
            let import = import.clone();
            let target = target.clone();
            let decision = copies.clone();
            let failure = error.clone();
            dialog
                .title("导入预览")
                .w(px(480.))
                .child(div().child(format!(
                    "{} 张便签 · {} 个分组 · {} 个附件",
                    import.manifest.notes.len(),
                    import.manifest.groups.len(),
                    import.assets.len()
                )))
                .child(div().text_sm().child(format!(
                    "与本机重复的便签：{duplicates} 张。默认跳过重复项。"
                )))
                .child(
                    Checkbox::new("import-copies")
                        .label("重复便签保留为副本")
                        .checked(*copies.read(cx))
                        .on_click(move |checked, _, cx| {
                            toggle.update(cx, |v, _| *v = *checked);
                        }),
                )
                .child(
                    div()
                        .text_color(rgb(0xb4473b))
                        .child(error.read(cx).clone()),
                )
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("导入")
                        .cancel_text("取消"),
                )
                .on_ok(move |_, _, cx| {
                    let keep = *decision.read(cx);
                    target
                        .update(cx, |v, cx| {
                            v.flush_cards(cx);
                            let result = v.shared.borrow_mut().store.import_archive(&import, keep);
                            match result {
                                Ok(count) => {
                                    let mut r = v.shared.borrow_mut();
                                    r.generation += 1;
                                    r.status = format!("已导入 {count} 张便签");
                                    cx.notify();
                                    true
                                }
                                Err(e) => {
                                    failure.update(cx, |s, _| *s = format!("导入失败：{e:#}"));
                                    cx.notify();
                                    false
                                }
                            }
                        })
                        .unwrap_or(false)
                })
        });
    }
    pub(super) fn batch_delete(
        &mut self,
        ids: Vec<String>,
        deleted: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        if !deleted {
            self.apply_batch(ids, None, Some(false), cx);
            return;
        }
        let target = cx.entity().downgrade();
        w.open_dialog(cx, move |dialog, _, _| {
            let ids = ids.clone();
            let target = target.clone();
            dialog
                .title(format!("将 {} 张便签移入回收站？", ids.len()))
                .child(div().child("之后可从回收站恢复。"))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("移入回收站")
                        .cancel_text("取消")
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok(move |_, _, cx| {
                    target
                        .update(cx, |v, cx| v.apply_batch(ids.clone(), None, Some(true), cx))
                        .unwrap_or(false)
                })
        });
    }
    fn apply_batch(
        &mut self,
        ids: Vec<String>,
        group: Option<String>,
        deleted: Option<bool>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.flush_cards(cx);
        if self.shared.borrow().dirty_editors.get() != 0 {
            self.shared.borrow_mut().status = "正文尚未保存，请稍后重试".into();
            return false;
        }
        let before: Vec<_> = self
            .shared
            .borrow()
            .store
            .notes
            .iter()
            .filter(|n| ids.contains(&n.id))
            .cloned()
            .collect();
        let result = self
            .shared
            .borrow_mut()
            .store
            .batch_notes(&ids, group.as_deref(), deleted);
        let ok = result.is_ok();
        self.result(result);
        if ok {
            let after = self
                .shared
                .borrow()
                .store
                .notes
                .iter()
                .filter(|n| ids.contains(&n.id))
                .map(|n| (n.id.clone(), n.revision.clone()))
                .collect();
            self.batch_undo = Some((before, after));
            self.multi = false;
            self.selection.clear();
            let mut r = self.shared.borrow_mut();
            r.generation += 1;
            r.status = format!("已处理 {} 张便签", ids.len());
        }
        cx.notify();
        ok
    }
    pub(super) fn batch_toolbar(&self, notes: &[Note], cx: &mut Context<Self>) -> AnyElement {
        let all: Vec<_> = notes.iter().map(|n| n.id.clone()).collect();
        let count = self.selection.len();
        let target = cx.entity().downgrade();
        div()
            .flex_shrink_0()
            .px(px(24.))
            .py(px(10.))
            .flex()
            .items_center()
            .gap_3()
            .border_t_1()
            .border_color(rgb(LINE))
            .child(div().text_sm().child(format!("已选择 {count} 张")))
            .child(
                label_button(
                    "select-all",
                    if count == notes.len() && count > 0 {
                        "取消全选"
                    } else {
                        "全选"
                    },
                )
                .on_click(cx.listener(move |v, _, _, cx| {
                    if v.selection.len() == all.len() {
                        v.selection.clear();
                    } else {
                        v.selection = all.iter().cloned().collect();
                    }
                    cx.notify();
                })),
            )
            .child(div().flex_1())
            .when(!self.trash, |d| {
                d.child(
                    Button::new("batch-move")
                        .label("移动分组")
                        .disabled(count == 0)
                        .dropdown_menu(move |mut menu, _, cx| {
                            let Some(view) = target.upgrade() else {
                                return menu;
                            };
                            let groups = view.read(cx).shared.borrow().store.groups.clone();
                            for group in groups {
                                let target = target.clone();
                                menu = menu.item(PopupMenuItem::new(group.name).on_click(
                                    move |_, _, cx| {
                                        let _ = target.update(cx, |v, cx| {
                                            v.apply_batch(
                                                v.selection.iter().cloned().collect(),
                                                Some(group.id.clone()),
                                                None,
                                                cx,
                                            );
                                        });
                                    },
                                ));
                            }
                            menu
                        }),
                )
            })
            .child(
                Button::new("batch-export")
                    .label("导出")
                    .disabled(count == 0 || self.archive_busy)
                    .on_click(cx.listener(|v, _, w, cx| {
                        v.export_archive(v.selection.iter().cloned().collect(), w, cx)
                    })),
            )
            .child(
                Button::new("batch-delete")
                    .label(if self.trash {
                        "恢复"
                    } else {
                        "移入回收站"
                    })
                    .disabled(count == 0)
                    .on_click(cx.listener(|v, _, w, cx| {
                        v.batch_delete(v.selection.iter().cloned().collect(), !v.trash, w, cx)
                    })),
            )
            .when(self.trash, |d| {
                d.child(
                    Button::new("batch-purge")
                        .label("永久删除")
                        .disabled(count == 0)
                        .on_click(cx.listener(|v, _, w, cx| {
                            v.confirm_purge(v.selection.iter().cloned().collect(), w, cx)
                        })),
                )
            })
            .child(
                label_button("batch-done", "完成").on_click(cx.listener(|v, _, _, cx| {
                    v.multi = false;
                    v.selection.clear();
                    cx.notify();
                })),
            )
            .into_any_element()
    }
    #[cfg(target_os = "linux")]
    pub(super) fn general_panel(&self, _: &mut Context<Self>) -> AnyElement {
        div()
            .child("托盘、自启动和全局快捷键正在适配")
            .into_any_element()
    }

    #[cfg(windows)]
    pub(super) fn general_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(settings_heading("启动与外观"))
            .child(
                div()
                    .bg(rgb(CARD))
                    .border_1()
                    .border_color(rgb(LINE))
                    .rounded(px(7.))
                    .flex()
                    .flex_col()
                    .child(
                        toggle_row(
                            "start-on-login",
                            "开机静默启动（仅驻留托盘）",
                            crate::platform::start_on_login_enabled(),
                        )
                        .on_click(cx.listener(|v, _, _, cx| {
                            let enabled = !crate::platform::start_on_login_enabled();
                            let result = crate::platform::set_start_on_login(enabled);
                            if result.is_ok() {
                                v.settings.start_on_login = enabled;
                                v.save_settings_automatically(cx);
                            } else {
                                v.result(result);
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        div().border_t_1().border_color(rgb(LINE)).child(
                            toggle_row(
                                "reduced-motion",
                                "减少动态效果",
                                self.settings.reduced_motion,
                            )
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.settings.reduced_motion = !v.settings.reduced_motion;
                                v.save_settings_automatically(cx);
                            })),
                        ),
                    ),
            )
            .child(settings_heading("快捷键"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px(px(18.))
                    .py(px(15.))
                    .bg(rgb(CARD))
                    .border_1()
                    .border_color(rgb(LINE))
                    .rounded(px(7.))
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
                                    .child("全局快捷键"),
                            )
                            .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                                "新建：{}   ·   便签中心：{}",
                                self.settings.new_shortcut, self.settings.center_shortcut
                            ))),
                    )
                    .child(
                        label_button("edit-shortcuts", "修改快捷键")
                            .on_click(cx.listener(|v, _, w, cx| v.shortcut_dialog(w, cx))),
                    ),
            )
            .into_any_element()
    }
    #[cfg(windows)]
    fn shortcut_dialog(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let new =
            cx.new(|cx| InputState::new(w, cx).default_value(self.settings.new_shortcut.clone()));
        let center = cx
            .new(|cx| InputState::new(w, cx).default_value(self.settings.center_shortcut.clone()));
        let error = cx.new(|_| String::new());
        let target = cx.entity().downgrade();
        w.open_dialog(cx, move |dialog, _, cx| {
            let new_value = new.clone();
            let center_value = center.clone();
            let target = target.clone();
            let error_value = error.clone();
            dialog
                .title("全局快捷键")
                .w(px(480.))
                .child(div().child("新建便签"))
                .child(Input::new(&new))
                .child(div().child("打开便签中心"))
                .child(Input::new(&center))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("推荐 Alt + N 新建、Alt + J 打开列表；支持 Ctrl、Alt、Shift 加字母、数字或 Space。被占用时请换一组。"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xb4473b))
                        .child(error.read(cx).clone()),
                )
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("应用")
                        .cancel_text("取消"),
                )
                .on_ok(move |_, _, cx| {
                    let values = [
                        new_value.read(cx).value().to_string(),
                        center_value.read(cx).value().to_string(),
                    ];
                    let parsed = crate::platform::parse_shortcut(&values[0]).and_then(|a| {
                        let b = crate::platform::parse_shortcut(&values[1])?;
                        anyhow::ensure!(a != b, "两个快捷键不能相同");
                        Ok(())
                    });
                    if let Err(e) = parsed {
                        error_value.update(cx, |s, cx| {
                            *s = e.to_string();
                            cx.notify();
                        });
                        return false;
                    }
                    let _ = target.update(cx, |v, cx| {
                        let Some(sender) = v.shared.borrow().hotkey_requests.clone() else {
                            return;
                        };
                        let (tx, rx) = mpsc::channel();
                        if sender
                            .send(crate::platform::HotkeyRequest {
                                shortcuts: values.clone(),
                                reply: tx,
                            })
                            .is_err()
                        {
                            v.shared.borrow_mut().status = "快捷键服务不可用".into();
                            return;
                        }
                        cx.spawn(async move |view, cx| {
                            let result = cx
                                .background_executor()
                                .spawn(async move {
                                    rx.recv_timeout(Duration::from_secs(3))
                                        .unwrap_or_else(|_| Err("快捷键服务未响应".into()))
                                })
                                .await;
                            let _ = view.update(cx, |v, cx| {
                                match result {
                                    Ok(()) => {
                                        v.settings.new_shortcut = values[0].clone();
                                        v.settings.center_shortcut = values[1].clone();
                                        v.save_settings_automatically(cx);
                                        v.shared.borrow_mut().status = "全局快捷键已生效".into();
                                    }
                                    Err(error) => v.shared.borrow_mut().status = error,
                                }
                                cx.notify();
                            });
                        })
                        .detach();
                    });
                    true
                })
        });
    }
    pub(super) fn data_panel(&mut self, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let r = self.shared.borrow();
        let active = r
            .store
            .notes
            .iter()
            .filter(|n| !n.is_empty() && n.deleted_at.is_none())
            .count();
        let trash: Vec<_> = r
            .store
            .notes
            .iter()
            .filter(|n| !n.is_empty() && n.deleted_at.is_some())
            .cloned()
            .collect();
        drop(r);
        let mut rows = div()
            .flex()
            .flex_col()
            .bg(rgb(CARD))
            .border_1()
            .border_color(rgb(LINE))
            .rounded(px(7.));
        for note in trash.iter().take(100) {
            let nid = note.id.clone();
            let purge_id = nid.clone();
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px(px(16.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(rgb(LINE))
                    .child(
                        div().flex_1().text_sm().child(
                            note.text()
                                .lines()
                                .next()
                                .unwrap_or("便签")
                                .chars()
                                .take(45)
                                .collect::<String>(),
                        ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(timestamp(note.deleted_at.unwrap())),
                    )
                    .child(
                        label_button(SharedString::from(format!("restore-{}", note.id)), "恢复")
                            .text_color(rgb(TEAL))
                            .bg(rgb(0xe8eee8))
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.apply_batch(vec![nid.clone()], None, Some(false), cx);
                            })),
                    )
                    .child(
                        label_button(SharedString::from(format!("purge-{}", note.id)), "永久删除")
                            .text_color(rgb(0xb4473b))
                            .on_click(cx.listener(move |v, _, w, cx| {
                                v.confirm_purge(vec![purge_id.clone()], w, cx)
                            })),
                    ),
            );
        }
        let target = cx.entity().downgrade();
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(format!("{active} 张便签 · {} 张在回收站", trash.len())),
            )
            .child(settings_heading("导入与导出"))
            .child(
                div()
                    .flex()
                    .when(narrow, |d| d.flex_col())
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .bg(rgb(CARD))
                            .border_1()
                            .border_color(rgb(LINE))
                            .rounded(px(6.))
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("导出数据"))
                            .child(div().text_sm().child("将全部便签、分组与附件打包为 ZIP"))
                            .child(
                                Checkbox::new("include-trash")
                                    .label("包含回收站")
                                    .checked(self.include_trash)
                                    .on_click(cx.listener(|v, checked, _, cx| {
                                        v.include_trash = *checked;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div().flex().items_center().child(
                                    Button::new("export-zip")
                                        .primary()
                                        .label(if self.archive_busy {
                                            "处理中…"
                                        } else {
                                            "导出压缩包"
                                        })
                                        .disabled(self.archive_busy)
                                        .on_click(cx.listener(|v, _, w, cx| {
                                            let ids = v
                                                .shared
                                                .borrow()
                                                .store
                                                .notes
                                                .iter()
                                                .filter(|n| {
                                                    !n.is_empty()
                                                        && (v.include_trash
                                                            || n.deleted_at.is_none())
                                                })
                                                .map(|n| n.id.clone())
                                                .collect();
                                            v.export_archive(ids, w, cx);
                                        })),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .bg(rgb(CARD))
                            .border_1()
                            .border_color(rgb(LINE))
                            .rounded(px(6.))
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("导入数据"))
                            .child(div().text_sm().child("选择闪记 ZIP 压缩包，预览后导入"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(MUTED))
                                    .child("重复便签可跳过或保留副本"),
                            )
                            .child(
                                div().flex().items_center().child(
                                    Button::new("import-zip")
                                        .label("选择压缩包")
                                        .outline()
                                        .disabled(self.archive_busy)
                                        .on_click(
                                            cx.listener(|v, _, w, cx| v.choose_import(w, cx)),
                                        ),
                                ),
                            ),
                    ),
            )
            .child(settings_heading("本机存储"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p(px(18.))
                    .bg(rgb(CARD))
                    .border_1()
                    .border_color(rgb(LINE))
                    .rounded(px(7.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(32.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(5.))
                                    .child(div().text_xs().text_color(rgb(MUTED)).child("附件"))
                                    .child(
                                        div().font_weight(FontWeight::SEMIBOLD).child(
                                            self.storage_bytes
                                                .map(|(a, _)| {
                                                    format!("{:.1} MB", a as f64 / 1048576.)
                                                })
                                                .unwrap_or_else(|| "统计中…".into()),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(5.))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .child("数据库与历史"),
                                    )
                                    .child(
                                        div().font_weight(FontWeight::SEMIBOLD).child(
                                            self.storage_bytes
                                                .map(|(_, d)| {
                                                    format!("{:.1} MB", d as f64 / 1048576.)
                                                })
                                                .unwrap_or_else(|| "统计中…".into()),
                                        ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_wrap()
                            .gap_3()
                            .pt(px(14.))
                            .border_t_1()
                            .border_color(rgb(LINE))
                            .child(
                                div()
                                    .text_sm()
                                    .flex_1()
                                    .min_w(px(120.))
                                    .child("历史版本保留"),
                            )
                            .child(
                                Button::new("history-retention")
                                    .outline()
                                    .label(if self.settings.history_days == 0 {
                                        "保留全部".into()
                                    } else {
                                        format!("{} 天", self.settings.history_days)
                                    })
                                    .dropdown_menu(move |mut menu, _, _| {
                                        for days in [0, 30, 90, 365] {
                                            let target = target.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(if days == 0 {
                                                    "保留全部".into()
                                                } else {
                                                    format!("{days} 天")
                                                })
                                                .on_click(move |_, _, cx| {
                                                    let _ = target.update(cx, |v, cx| {
                                                        v.settings.history_days = days;
                                                        v.save_settings_automatically(cx);
                                                    });
                                                }),
                                            );
                                        }
                                        menu
                                    }),
                            )
                            .child(
                                label_button("cleanup-storage", "清理到期历史与无引用附件")
                                    .text_color(rgb(TEAL))
                                    .bg(rgb(0xe8eee8))
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        if v.shared.borrow().sync_busy
                                            || v.archive_busy
                                            || v.shared.borrow().dirty_editors.get() != 0
                                        {
                                            v.shared.borrow_mut().status =
                                                "正在处理数据，请稍后清理".into();
                                            return;
                                        }
                                        let result = (|| -> anyhow::Result<(usize, u64)> {
                                            let mut r = v.shared.borrow_mut();
                                            let days = r.store.settings.history_days;
                                            let count = if days == 0 {
                                                0
                                            } else {
                                                r.store.cleanup_history(days)?
                                            };
                                            Ok((count, r.store.cleanup_unused_assets()?))
                                        })();
                                        match result {
                                            Ok((count, bytes)) => {
                                                v.shared.borrow_mut().status = format!(
                                                    "已清理 {count} 个历史版本，释放附件 {:.1} MB",
                                                    bytes as f64 / 1048576.
                                                )
                                            }
                                            Err(e) => {
                                                v.shared.borrow_mut().status =
                                                    format!("清理失败：{e:#}")
                                            }
                                        }
                                        v.refresh_storage(cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(div().text_xs().text_color(rgb(MUTED)).child(
                        "清理保留当前版本、同步基线及未上传内容；保留期限仅在点击清理时生效。",
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(settings_heading("回收站"))
                    .child(
                        Button::new("empty-trash")
                            .label("清空回收站")
                            .outline()
                            .disabled(trash.is_empty())
                            .on_click(cx.listener(|v, _, w, cx| {
                                let ids = v
                                    .shared
                                    .borrow()
                                    .store
                                    .notes
                                    .iter()
                                    .filter(|n| !n.is_empty() && n.deleted_at.is_some())
                                    .map(|n| n.id.clone())
                                    .collect();
                                v.confirm_purge(ids, w, cx);
                            })),
                    ),
            )
            .when(!trash.is_empty(), |d| d.child(rows))
            .when(trash.is_empty(), |d| {
                d.child(div().text_sm().text_color(rgb(MUTED)).child("回收站为空"))
            })
            .into_any_element()
    }
    pub(super) fn groups_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let groups = self.shared.borrow().store.groups.clone();
        let mut rows = div()
            .flex()
            .flex_col()
            .bg(rgb(CARD))
            .border_1()
            .border_color(rgb(LINE))
            .rounded(px(7.));
        for (index, group) in groups.iter().cloned().enumerate() {
            let editable = group.id != DEFAULT_GROUP;
            let edit = group.clone();
            let up = group.id.clone();
            let down = group.id.clone();
            let remove = group.clone();
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px(px(16.))
                    .py(px(12.))
                    .when(index > 0, |d| d.border_t_1().border_color(rgb(LINE)))
                    .child(icon(group_icon(&group.icon)).text_color(rgb(group.color)))
                    .child(div().flex_1().child(group.name))
                    .child(
                        label_button(
                            SharedString::from(format!("edit-group-{}", group.id)),
                            "编辑",
                        )
                        .on_click(
                            cx.listener(move |v, _, w, cx| {
                                v.group_dialog(Some(edit.clone()), w, cx)
                            }),
                        ),
                    )
                    .when(editable, |d| {
                        d.child(
                            Button::new(SharedString::from(format!("up-{}", group.id)))
                                .label("上移")
                                .disabled(index <= 1)
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    let result = v.shared.borrow_mut().store.reorder_group(&up, -1);
                                    v.result(result);
                                    v.shared.borrow_mut().generation += 1;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("down-{}", group.id)))
                                .label("下移")
                                .disabled(index + 1 == groups.len())
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    let result =
                                        v.shared.borrow_mut().store.reorder_group(&down, 1);
                                    v.result(result);
                                    v.shared.borrow_mut().generation += 1;
                                    cx.notify();
                                })),
                        )
                        .child(
                            label_button(
                                SharedString::from(format!("remove-group-{}", group.id)),
                                "删除",
                            )
                            .on_click(cx.listener(
                                move |v, _, w, cx| v.delete_group_dialog(remove.clone(), w, cx),
                            )),
                        )
                    }),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(rows)
            .child(
                div().flex().child(
                    label_button("settings-new-group", "＋ 新建分组")
                        .border_1()
                        .border_color(rgb(LINE))
                        .bg(rgb(CARD))
                        .on_click(cx.listener(|v, _, w, cx| v.create_group_dialog(w, cx))),
                ),
            )
            .into_any_element()
    }
    fn delete_group_dialog(&mut self, group: Group, w: &mut Window, cx: &mut Context<Self>) {
        let target = cx.entity().downgrade();
        let destination = cx.new(|_| DEFAULT_GROUP.to_string());
        let groups: Vec<_> = self
            .shared
            .borrow()
            .store
            .groups
            .iter()
            .filter(|g| g.id != group.id)
            .cloned()
            .collect();
        w.open_dialog(cx, move |dialog, _, cx| {
            let choice = destination.clone();
            let selected = destination.read(cx).clone();
            let options = groups.clone();
            let target = target.clone();
            let gid = group.id.clone();
            let value = destination.clone();
            dialog
                .title(format!("删除分组“{}”", group.name))
                .child(div().child("便签（包括回收站内容）将迁移到："))
                .child(
                    Button::new("delete-group-target")
                        .label(
                            groups
                                .iter()
                                .find(|g| g.id == selected)
                                .map(|g| g.name.clone())
                                .unwrap_or_default(),
                        )
                        .dropdown_menu(move |mut menu, _, _| {
                            for group in &options {
                                let choice = choice.clone();
                                let gid = group.id.clone();
                                menu = menu.item(PopupMenuItem::new(group.name.clone()).on_click(
                                    move |_, w, cx| {
                                        choice.update(cx, |s, _| *s = gid.clone());
                                        w.refresh();
                                    },
                                ));
                            }
                            menu
                        }),
                )
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("迁移并删除")
                        .cancel_text("取消"),
                )
                .on_ok(move |_, _, cx| {
                    let destination = value.read(cx).clone();
                    target
                        .update(cx, |v, cx| {
                            v.flush_cards(cx);
                            if v.shared.borrow().dirty_editors.get() != 0 {
                                return false;
                            }
                            let result =
                                v.shared.borrow_mut().store.delete_group(&gid, &destination);
                            let ok = result.is_ok();
                            v.result(result);
                            if ok {
                                if v.group.as_ref() == Some(&gid) {
                                    v.group = Some(destination);
                                }
                                v.shared.borrow_mut().generation += 1;
                            }
                            cx.notify();
                            ok
                        })
                        .unwrap_or(false)
                })
        });
    }
}
