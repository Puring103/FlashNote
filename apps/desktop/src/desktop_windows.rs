use crate::{platform, ui};
use gpui::*;
use std::sync::mpsc::{self, Receiver};
use suojian::store::Store;

// GPUI's Windows backend exits when its last native window is destroyed.
// A hidden tool window keeps the event loop alive after all user windows close.
struct BackgroundHost;
impl Render for BackgroundHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub fn migrate_shortcuts(store: &mut Store) -> anyhow::Result<()> {
    // Apply the new desktop defaults once, preserving custom key combinations.
    if std::env::var_os("SUOJIAN_DATA_DIR").is_some() {
        return Ok(());
    }
    let mut settings = store.settings.clone();
    if settings.desktop_preferences_version >= 1 {
        return Ok(());
    }
    if settings.new_shortcut == "Ctrl + Alt + N" {
        settings.new_shortcut = "Alt + N".into();
    }
    if settings.center_shortcut == "Ctrl + Alt + Space" {
        settings.center_shortcut = "Alt + J".into();
    }
    settings.desktop_preferences_version = 1;
    store.save_settings(settings)
}

pub struct Desktop {
    instance: platform::Instance,
    hotkeys: Receiver<Result<usize, String>>,
    tray: Option<platform::Tray>,
}

impl Desktop {
    pub fn start(instance: platform::Instance, shared: &ui::Shared, cx: &mut App) -> Self {
        if let Err(error) = open_background_host(cx) {
            shared.borrow_mut().status = format!("后台驻留初始化失败：{error}");
            ui::open(shared.clone(), ui::Screen::Settings, cx);
        }
        let shortcuts = {
            let runtime = shared.borrow();
            let settings = &runtime.store.settings;
            [
                settings.new_shortcut.clone(),
                settings.center_shortcut.clone(),
            ]
        };
        let (events, hotkeys) = mpsc::channel();
        let (requests, receiver) = mpsc::channel();
        shared.borrow_mut().hotkey_requests = Some(requests);
        std::thread::spawn(move || platform::hotkey_loop(shortcuts, receiver, events));
        Self {
            instance,
            hotkeys,
            tray: platform::Tray::new().ok(),
        }
    }

    pub fn poll(&self, shared: &ui::Shared, cx: &mut App) {
        if self.instance.activation_requested() {
            ui::open_new(shared.clone(), cx);
        }
        if let Some(tray) = &self.tray {
            for event in tray_icon::menu::MenuEvent::receiver().try_iter() {
                if event.id == tray.quit {
                    ui::request_quit(shared.clone(), cx);
                } else if event.id == tray.new {
                    ui::open_new(shared.clone(), cx);
                } else if event.id == tray.settings {
                    ui::open(shared.clone(), ui::Screen::Settings, cx);
                } else if event.id == tray.center {
                    ui::open(shared.clone(), ui::Screen::Center, cx);
                }
            }
        }
        for event in self.hotkeys.try_iter() {
            match event {
                Ok(1) => ui::open_shortcut(shared.clone(), true, cx),
                Ok(_) => ui::open_shortcut(shared.clone(), false, cx),
                Err(error) => {
                    shared.borrow_mut().status = error;
                    ui::open(shared.clone(), ui::Screen::Settings, cx);
                }
            }
        }
    }
}

fn open_background_host(cx: &mut App) -> anyhow::Result<()> {
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(1.), px(1.)),
            ))),
            titlebar: None,
            show: false,
            focus: false,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        },
        |window, cx| {
            window.on_window_should_close(cx, |_, _| false);
            cx.new(|_| BackgroundHost)
        },
    )
    .map(|_| ())
}
