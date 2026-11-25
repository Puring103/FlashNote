#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod audio;
#[cfg(windows)]
mod desktop_windows;
mod icons;
mod platform;
#[cfg(test)]
#[path = "../../../tests/support/test_store.rs"]
mod test_store;
mod ui;
use gpui::*;
use std::{cell::RefCell, rc::Rc, time::Duration};
use suojian::{media, model, store, sync};

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        !args.iter().any(|arg| arg == "--demo"),
        "演示模式已移除，请不带 --demo 参数启动。"
    );
    let center = args.iter().any(|arg| arg == "--center");
    let background = args.iter().any(|arg| arg == "--background");
    #[cfg(windows)]
    let Some(instance) = platform::Instance::acquire()? else {
        return Ok(());
    };
    let root = suojian::cli::data_dir();
    #[cfg(target_os = "linux")]
    suojian::linux_keyring::install()?;
    let mut store = suojian::profiles::open(root.clone())?;
    #[cfg(windows)]
    desktop_windows::migrate_shortcuts(&mut store)?;
    audio::cleanup_scratch(&store.root.join("scratch"));
    let interrupted: Vec<_> = store
        .notes
        .iter()
        .filter(|n| {
            n.blocks.iter().chain(&n.media_archive).any(|b| {
                matches!(
                    b,
                    model::Block::Audio {
                        state: model::AudioState::Processing,
                        ..
                    }
                )
            })
        })
        .cloned()
        .collect();
    for mut n in interrupted {
        for block in n.blocks.iter_mut().chain(n.media_archive.iter_mut()) {
            if let model::Block::Audio { state, .. } = block {
                if *state == model::AudioState::Processing {
                    *state = model::AudioState::Recorded;
                }
            }
        }
        store.save_note(n)?;
    }
    let mut runtime = ui::Runtime::new(store);
    #[cfg(windows)]
    let cli_jobs = match suojian::cli_pipe::listen(&root) {
        Ok(jobs) => Some(jobs),
        Err(error) => {
            runtime.status = format!("命令行接口启动失败：{error}");
            None
        }
    };
    let shared = Rc::new(RefCell::new(runtime));
    Application::new()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_component::init(cx);
            gpui_note_editor::init(cx);
            gpui_component::Theme::change(gpui_component::ThemeMode::Light, None, cx);
            {
                let theme = gpui_component::Theme::global_mut(cx);
                theme.font_family = platform::UI_FONT.into();
                theme.colors.primary = rgb(0x234c50).into();
                theme.colors.foreground = rgb(0x262d2b).into();
                theme.colors.background = rgb(0xf6f3eb).into();
                theme.colors.popover = rgb(0xfbf8f1).into();
                theme.colors.popover_foreground = rgb(0x262d2b).into();
                theme.colors.border = rgb(0xcac7bd).into();
                theme.colors.caret = rgb(0x234c50).into();
            }
            #[cfg(windows)]
            let desktop = desktop_windows::Desktop::start(instance, &shared, cx);
            match (background, center) {
                (true, _) if cfg!(windows) => {}
                (true, _) | (_, true) => ui::open(shared.clone(), ui::Screen::Center, cx),
                (false, false) => ui::open_new(shared.clone(), cx),
            }
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(8))
                        .await;
                    let result = cx.update(|_app| {
                        #[cfg(windows)]
                        desktop.poll(&shared, _app);
                        let mut runtime = shared.borrow_mut();
                        runtime.drain_jobs();
                        #[cfg(windows)]
                        if let Some((request, send)) =
                            cli_jobs.as_ref().and_then(|jobs| jobs.try_recv().ok())
                        {
                            let writes_store = request.writes_store();
                            let open = match &request {
                                suojian::cli::Request::Open { id } => Some(id.clone()),
                                _ => None,
                            };
                            let reply =
                                if matches!(&request, suojian::cli::Request::GroupDelete { .. })
                                    && runtime.has_pending_editor_edits(_app)
                                {
                                    Err(suojian::cli::CliError::new(
                                        suojian::cli::ErrorKind::Busy,
                                        "请先完成桌面端正在编辑的内容，再删除分组",
                                    ))
                                } else {
                                    suojian::cli::execute(&mut runtime.store, request)
                                };
                            if writes_store && reply.is_ok() {
                                runtime.generation += 1;
                            }
                            drop(runtime);
                            let reply = reply.and_then(|value| {
                                if let Some(id) = open {
                                    ui::open_cli_note(shared.clone(), id, _app)
                                        .map_err(suojian::cli::CliError::io)?;
                                }
                                Ok(value)
                            });
                            let _ = send.send(reply);
                        }
                    });
                    if result.is_err() {
                        break;
                    }
                }
            })
            .detach();
        });
    Ok(())
}
