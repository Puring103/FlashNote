use anyhow::Result;
use windows::{
    Win32::{Foundation::*, System::Threading::*},
    core::HSTRING,
};

/// Begin a native Windows caption drag from a client-area mouse press.
pub fn drag_window(window: &gpui::Window) -> Result<()> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::{HWND, LPARAM, POINT, WPARAM},
        UI::{
            Input::KeyboardAndMouse::ReleaseCapture,
            WindowsAndMessaging::{GetCursorPos, HTCAPTION, PostMessageW, SC_MOVE, WM_SYSCOMMAND},
        },
    };
    let window_handle = HasWindowHandle::window_handle(window)
        .map_err(|error| anyhow::anyhow!("无法获取便签窗口：{error:?}"))?;
    let RawWindowHandle::Win32(handle) = window_handle.as_raw() else {
        anyhow::bail!("无法获取便签窗口");
    };
    unsafe {
        let mut position = POINT::default();
        GetCursorPos(&mut position)?;
        let coordinates =
            ((position.x as u16 as u32) | ((position.y as u16 as u32) << 16)) as isize;
        // The press may not have captured the mouse yet; that is not a reason
        // to skip the native caption drag.
        let _ = ReleaseCapture();
        PostMessageW(
            Some(HWND(handle.hwnd.get() as _)),
            WM_SYSCOMMAND,
            WPARAM((SC_MOVE | HTCAPTION) as usize),
            LPARAM(coordinates),
        )?;
    }
    Ok(())
}

pub struct HotkeyRequest {
    pub shortcuts: [String; 2],
    pub reply: std::sync::mpsc::Sender<std::result::Result<(), String>>,
}

/// Anchor the window's top midpoint to the cursor, clamped to the monitor work area.
fn cursor_anchored_window_bounds(size: (i32, i32), cursor: POINT, work: RECT) -> RECT {
    let width = size.0.min((work.right - work.left).max(1));
    let height = size.1.min((work.bottom - work.top).max(1));
    let left = (cursor.x - width / 2).clamp(work.left, work.right - width);
    let top = cursor.y.clamp(work.top, work.bottom - height);
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// Compute final placement before GPUI creates the native window, so the
/// framework owns sizing, renderer initialization and the first visible frame.
pub fn cursor_window_placement(
    logical_size: (f32, f32),
    cx: &gpui::App,
) -> Result<(gpui::Bounds<gpui::Pixels>, gpui::DisplayId)> {
    use gpui::{bounds, point, px, size};
    use windows::Win32::{
        Graphics::Gdi::*,
        UI::{HiDpi::*, WindowsAndMessaging::*},
    };
    unsafe {
        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor)?;
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        anyhow::ensure!(
            GetMonitorInfoW(monitor, &mut info).as_bool(),
            "无法读取屏幕工作区"
        );
        let mut dpi_x = 96;
        let mut dpi_y = 96;
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y)?;
        let scale = dpi_x as f32 / 96.;
        let display = cx
            .displays()
            .into_iter()
            .find(|display| {
                let rect = display.bounds();
                (f32::from(rect.origin.x) * scale - info.rcMonitor.left as f32).abs() < 2.
                    && (f32::from(rect.origin.y) * scale - info.rcMonitor.top as f32).abs() < 2.
                    && (f32::from(rect.size.width) * scale
                        - (info.rcMonitor.right - info.rcMonitor.left) as f32)
                        .abs()
                        < 2.
            })
            .ok_or_else(|| anyhow::anyhow!("无法匹配鼠标所在屏幕"))?;
        // Reserve the native resize border around the logical client bounds.
        let border = (8. * scale).ceil() as i32;
        let work = RECT {
            left: info.rcWork.left + border,
            top: info.rcWork.top + border,
            right: info.rcWork.right - border,
            bottom: info.rcWork.bottom - border,
        };
        let rect = cursor_anchored_window_bounds(
            (
                (logical_size.0 * scale).round() as i32,
                (logical_size.1 * scale).round() as i32,
            ),
            cursor,
            work,
        );
        Ok((
            bounds(
                point(px(rect.left as f32 / scale), px(rect.top as f32 / scale)),
                size(
                    px((rect.right - rect.left) as f32 / scale),
                    px((rect.bottom - rect.top) as f32 / scale),
                ),
            ),
            display.id(),
        ))
    }
}

/// GPUI 0.2's Windows placement can retain SW_HIDE from a freshly created
/// HWND. Show explicitly without changing the initialized canvas dimensions.
fn show_window(window: &gpui::Window) -> Result<()> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::UI::WindowsAndMessaging::*;
    let raw = HasWindowHandle::window_handle(window)
        .map_err(|error| anyhow::anyhow!("无法获取窗口：{error:?}"))?;
    let RawWindowHandle::Win32(raw) = raw.as_raw() else {
        anyhow::bail!("无法获取 Windows 窗口")
    };
    unsafe {
        let hwnd = HWND(raw.hwnd.get() as _);
        SetWindowPos(
            hwnd,
            Some(HWND_NOTOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        )?;
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

/// Whole-surface compositor entrance. The HWND, layout and swap chain retain
/// their final sizes; DirectComposition scales text and background together.
struct WindowEntrance {
    anchor: gpui::Point<f32>,
}
impl WindowEntrance {
    fn begin(window: &gpui::Window) -> Result<Option<Self>> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows::Win32::{Graphics::Gdi::ClientToScreen, UI::WindowsAndMessaging::*};
        let raw = HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("无法获取窗口：{error:?}"))?;
        let RawWindowHandle::Win32(raw) = raw.as_raw() else {
            anyhow::bail!("无法获取 Windows 窗口")
        };
        unsafe {
            let hwnd = HWND(raw.hwnd.get() as _);
            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect)?;
            let mut origin = POINT::default();
            anyhow::ensure!(
                ClientToScreen(hwnd, &mut origin).as_bool(),
                "无法读取窗口位置"
            );
            let mut cursor = POINT::default();
            GetCursorPos(&mut cursor)?;
            let entrance = Self {
                anchor: gpui::point(
                    ((cursor.x - origin.x) as f32 / (rect.right - rect.left).max(1) as f32)
                        .clamp(0., 1.),
                    ((cursor.y - origin.y) as f32 / (rect.bottom - rect.top).max(1) as f32)
                        .clamp(0., 1.),
                ),
            };
            if window.set_visual_appearance(0.85, 0.4, entrance.anchor)? {
                Ok(Some(entrance))
            } else {
                Ok(None)
            }
        }
    }
    fn apply(&self, window: &gpui::Window, progress: f32) -> Result<()> {
        let t = progress.clamp(0., 1.);
        let scale = 0.85 + 0.15 * entrance_easing(t);
        let opacity = 0.4 + 0.6 * entrance_easing((t / 0.65).min(1.));
        window.set_visual_appearance(scale, opacity, self.anchor)?;
        Ok(())
    }
    fn finish(&self, window: &gpui::Window) {
        let _ = window.set_visual_appearance(1., 1., self.anchor);
    }
}

fn entrance_easing(progress: f32) -> f32 {
    // CSS cubic-bezier(0.16, 1, 0.3, 1): solve x(u)=time, then return y(u).
    let t = progress.clamp(0., 1.);
    if t == 0. || t == 1. {
        return t;
    }
    let mut low = 0.;
    let mut high = 1.;
    for _ in 0..18 {
        let u = (low + high) * 0.5;
        let v = 1. - u;
        let x = 3. * v * v * u * 0.16 + 3. * v * u * u * 0.3 + u * u * u;
        if x < t {
            low = u;
        } else {
            high = u;
        }
    }
    let u = (low + high) * 0.5;
    1. - (1. - u).powi(3)
}

#[cfg(test)]
mod cursor_placement_tests {
    use super::*;

    #[test]
    fn entrance_curve_has_exact_endpoints_and_a_soft_monotonic_landing() {
        assert_eq!(entrance_easing(0.), 0.);
        assert_eq!(entrance_easing(1.), 1.);
        let mut previous = 0.;
        for frame in 1..=100 {
            let value = entrance_easing(frame as f32 / 100.);
            assert!(value >= previous && value <= 1.);
            previous = value;
        }
        assert!(entrance_easing(0.5) > 0.9);
        assert!(1. - entrance_easing(0.9) < 0.001);
    }

    #[test]
    fn anchors_top_midpoint_to_cursor() {
        let work = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let bounds = cursor_anchored_window_bounds((420, 520), POINT { x: 960, y: 200 }, work);
        assert_eq!(
            (bounds.left, bounds.top, bounds.right, bounds.bottom),
            (750, 200, 1170, 720)
        );
    }

    #[test]
    fn clamps_at_every_work_area_edge() {
        let work = RECT {
            left: 0,
            top: 40,
            right: 1920,
            bottom: 1040,
        };
        for (cursor, expected) in [
            (POINT { x: 0, y: 0 }, (0, 40)),
            (POINT { x: 1919, y: 0 }, (1500, 40)),
            (POINT { x: 0, y: 1039 }, (0, 520)),
            (POINT { x: 1919, y: 1039 }, (1500, 520)),
        ] {
            let bounds = cursor_anchored_window_bounds((420, 520), cursor, work);
            assert_eq!((bounds.left, bounds.top), expected);
            assert!(bounds.right <= work.right && bounds.bottom <= work.bottom);
        }
    }

    #[test]
    fn handles_negative_monitor_coordinates_and_oversized_windows() {
        let work = RECT {
            left: -1280,
            top: -200,
            right: 0,
            bottom: 800,
        };
        let cursor = POINT { x: -640, y: -100 };
        let bounds = cursor_anchored_window_bounds((420, 520), cursor, work);
        assert_eq!((bounds.left, bounds.top), (-850, -100));
        let bounds = cursor_anchored_window_bounds((1600, 1200), cursor, work);
        assert_eq!(
            (bounds.left, bounds.top, bounds.right, bounds.bottom),
            (-1280, -200, 0, 800)
        );
    }
}
pub fn parse_shortcut(value: &str) -> Result<(u32, u32)> {
    let mut modifiers = 0u32;
    let mut key = None;
    for part in value.split('+').map(|s| s.trim().to_ascii_lowercase()) {
        match part.as_str() {
            "ctrl" | "control" => modifiers |= 2,
            "alt" => modifiers |= 1,
            "shift" => modifiers |= 4,
            "space" => {
                anyhow::ensure!(key.is_none(), "只能指定一个按键");
                key = Some(0x20);
            }
            s if s.len() == 1 && s.as_bytes()[0].is_ascii_alphanumeric() => {
                anyhow::ensure!(key.is_none(), "只能指定一个按键");
                key = Some(s.as_bytes()[0].to_ascii_uppercase() as u32);
            }
            _ => anyhow::bail!("快捷键支持 Ctrl、Alt、Shift 加字母、数字或 Space"),
        }
    }
    anyhow::ensure!(modifiers & 3 != 0, "全局快捷键需要包含 Ctrl 或 Alt");
    Ok((
        modifiers | 0x4000,
        key.ok_or_else(|| anyhow::anyhow!("请指定一个按键"))?,
    ))
}
pub fn hotkey_loop(
    initial: [String; 2],
    requests: std::sync::mpsc::Receiver<HotkeyRequest>,
    events: std::sync::mpsc::Sender<std::result::Result<usize, String>>,
) {
    use windows::Win32::UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*};
    unsafe fn register(values: &[String; 2]) -> Result<()> {
        let a = parse_shortcut(&values[0])?;
        let b = parse_shortcut(&values[1])?;
        anyhow::ensure!(a != b, "两个全局快捷键不能相同");
        unsafe {
            RegisterHotKey(None, 1, HOT_KEY_MODIFIERS(a.0), a.1)?;
            if let Err(e) = RegisterHotKey(None, 2, HOT_KEY_MODIFIERS(b.0), b.1) {
                let _ = UnregisterHotKey(None, 1);
                return Err(e.into());
            }
        }
        Ok(())
    }
    unsafe fn register_available(
        values: &[String; 2],
        events: &std::sync::mpsc::Sender<std::result::Result<usize, String>>,
    ) {
        // A conflict for one action must not disable the other action.
        for (index, value) in values.iter().enumerate() {
            let result = parse_shortcut(value).and_then(|(modifiers, key)| {
                unsafe {
                    RegisterHotKey(None, (index + 1) as i32, HOT_KEY_MODIFIERS(modifiers), key)
                }
                .map_err(Into::into)
            });
            if let Err(error) = result {
                let _ = events.send(Err(format!(
                    "快捷键 {value} 注册失败，请在常规设置中修改：{error}"
                )));
            }
        }
    }
    let mut current = initial;
    unsafe {
        register_available(&current, &events);
        loop {
            match requests.recv_timeout(std::time::Duration::from_millis(8)) {
                Ok(request) => {
                    let _ = UnregisterHotKey(None, 1);
                    let _ = UnregisterHotKey(None, 2);
                    let result = register(&request.shortcuts)
                        .map_err(|e| format!("快捷键被占用或无效：{e}"));
                    if result.is_ok() {
                        current = request.shortcuts;
                    } else {
                        register_available(&current, &events);
                    }
                    let _ = request.reply.send(result);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(_) => {}
            }
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == WM_HOTKEY {
                    let _ = events.send(Ok(message.wParam.0));
                }
            }
        }
        let _ = UnregisterHotKey(None, 1);
        let _ = UnregisterHotKey(None, 2);
    }
}
pub fn set_start_on_login(enabled: bool) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new("reg.exe");
    command.creation_flags(0x08000000);
    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    if enabled {
        command
            .args(["add", key, "/v", "Suojian", "/t", "REG_SZ", "/d"])
            .arg(format!(
                "\"{}\" --background",
                std::env::current_exe()?.display()
            ))
            .arg("/f");
    } else {
        command.args(["delete", key, "/v", "Suojian", "/f"]);
    }
    let output = command.output()?;
    anyhow::ensure!(
        output.status.success(),
        "无法修改开机启动项：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

pub fn start_on_login_enabled() -> bool {
    use windows::{Win32::System::Registry::*, core::w};
    let mut bytes = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            w!("Suojian"),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        ) == ERROR_SUCCESS
            && bytes > 2
    }
}

pub struct Instance {
    mutex: HANDLE,
    event: HANDLE,
}
impl Instance {
    pub fn acquire() -> Result<Option<Self>> {
        unsafe {
            let suffix = "Local";
            let mutex = CreateMutexW(
                None,
                false,
                &HSTRING::from(format!("Local\\Suojian-{suffix}-Mutex")),
            )?;
            let exists = GetLastError() == ERROR_ALREADY_EXISTS;
            let event = CreateEventW(
                None,
                false,
                false,
                &HSTRING::from(format!("Local\\Suojian-{suffix}-Activate")),
            )?;
            if exists {
                if std::env::args().any(|arg| arg == "--background") {
                    CloseHandle(event)?;
                    CloseHandle(mutex)?;
                    return Ok(None);
                }
                // Reuse the running app rather than leaving it hidden behind a dialog.
                SetEvent(event)?;
                CloseHandle(event)?;
                CloseHandle(mutex)?;
                return Ok(None);
            }
            Ok(Some(Self { mutex, event }))
        }
    }
    pub fn activation_requested(&self) -> bool {
        unsafe { WaitForSingleObject(self.event, 0) == WAIT_OBJECT_0 }
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.event);
            let _ = CloseHandle(self.mutex);
        }
    }
}

pub struct Tray {
    pub _icon: tray_icon::TrayIcon,
    pub new: tray_icon::menu::MenuId,
    pub center: tray_icon::menu::MenuId,
    pub settings: tray_icon::menu::MenuId,
    pub quit: tray_icon::menu::MenuId,
}
impl Tray {
    pub fn new() -> Result<Self> {
        use tray_icon::{
            Icon, TrayIconBuilder,
            menu::{Menu, MenuItem},
        };
        let menu = Menu::new();
        let new = MenuItem::new("新建便签", true, None);
        let center = MenuItem::new("便签中心", true, None);
        let settings = MenuItem::new("设置", true, None);
        let quit = MenuItem::new("退出闪记", true, None);
        menu.append_items(&[&new, &center, &settings, &quit])?;
        let mut rgba = vec![0u8; 32 * 32 * 4];
        for y in 3..29 {
            for x in 3..29 {
                if x < 15 && y < 15 || x > 17 && y < 15 || x < 15 && y > 17 || x > 17 && y > 17 {
                    let i = (y * 32 + x) * 4;
                    rgba[i..i + 4].copy_from_slice(&[35, 76, 80, 255]);
                }
            }
        }
        let icon = TrayIconBuilder::new()
            .with_tooltip("闪记 · Ctrl+Alt+N 快速记录")
            .with_icon(Icon::from_rgba(rgba, 32, 32)?)
            .with_menu(Box::new(menu))
            .build()?;
        Ok(Self {
            _icon: icon,
            new: new.id().clone(),
            center: center.id().clone(),
            settings: settings.id().clone(),
            quit: quit.id().clone(),
        })
    }
}

/// Delegate file associations to Explorer, keeping shell extensions out of our process.
pub fn open_attachment(path: &std::path::Path) -> Result<()> {
    anyhow::ensure!(path.is_file(), "附件文件不存在或无法读取");
    open_location(path)
}

pub fn open_location(path: &std::path::Path) -> Result<()> {
    use anyhow::Context;
    // Explorer expects a normal absolute Windows path rather than a verbatim path.
    let absolute = std::path::absolute(path).context("无法定位附件文件")?;
    let windows = std::env::var_os("SystemRoot").context("无法定位 Windows 目录")?;
    std::process::Command::new(std::path::PathBuf::from(windows).join("explorer.exe"))
        .arg(&absolute)
        .spawn()
        .context("无法使用默认应用打开附件")?;
    Ok(())
}

// Read Windows file clipboard separately: flattening CF_HDROP to text loses boundaries.
pub fn clipboard_files() -> Option<Vec<std::path::PathBuf>> {
    use windows::Win32::{
        System::DataExchange::*,
        UI::Shell::{DragQueryFileW, HDROP},
    };
    unsafe {
        if OpenClipboard(None).is_err() {
            return None;
        }
        let result = (|| {
            let handle = GetClipboardData(15).ok()?;
            let drop = HDROP(handle.0);
            let count = DragQueryFileW(drop, u32::MAX, None);
            let mut paths = Vec::new();
            for index in 0..count {
                let length = DragQueryFileW(drop, index, None);
                let mut buffer = vec![0; length as usize + 1];
                DragQueryFileW(drop, index, Some(&mut buffer));
                paths.push(std::path::PathBuf::from(String::from_utf16_lossy(
                    &buffer[..length as usize],
                )));
            }
            if paths.is_empty() { None } else { Some(paths) }
        })();
        let _ = CloseClipboard();
        result
    }
}

// CF_DIB / CF_DIBV5 are common for screenshots but GPUI only reads registered image formats.
pub fn clipboard_bitmap() -> Option<Vec<u8>> {
    use windows::Win32::{
        Foundation::HGLOBAL,
        System::{DataExchange::*, Memory::*},
    };
    unsafe {
        if OpenClipboard(None).is_err() {
            return None;
        }
        let result = (|| {
            let handle = GetClipboardData(17).or_else(|_| GetClipboardData(8)).ok()?;
            let handle = HGLOBAL(handle.0);
            let length = GlobalSize(handle);
            if !(40..=suojian_core::media::MAX_FILE as usize - 14).contains(&length) {
                return None;
            }
            let ptr = GlobalLock(handle);
            if ptr.is_null() {
                return None;
            }
            let dib = std::slice::from_raw_parts(ptr.cast::<u8>(), length).to_vec();
            let _ = GlobalUnlock(handle);
            dib_to_bmp(&dib)
        })();
        let _ = CloseClipboard();
        result
    }
}
fn dib_to_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 40 {
        return None;
    }
    let u32_at = |at| u32::from_le_bytes(dib[at..at + 4].try_into().unwrap());
    let header = u32_at(0) as usize;
    if ![40, 52, 56, 108, 124].contains(&header) || header > dib.len() {
        return None;
    }
    let bits = u16::from_le_bytes(dib[14..16].try_into().unwrap());
    let compression = u32_at(16);
    let colors = u32_at(32) as usize;
    let colors = if colors != 0 {
        colors
    } else if bits <= 8 {
        1usize << bits
    } else {
        0
    };
    let masks = if header == 40 {
        match compression {
            3 => 12,
            6 => 16,
            _ => 0,
        }
    } else {
        0
    };
    let offset = header
        .checked_add(masks)?
        .checked_add(colors.checked_mul(4)?)?;
    if offset > dib.len() {
        return None;
    }
    let mut bmp = Vec::with_capacity(dib.len() + 14);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&((offset + 14) as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    Some(bmp)
}

pub fn clipboard_set_html(fragment: &str) {
    use windows::{
        Win32::{
            Foundation::HANDLE,
            System::{DataExchange::*, Memory::*},
        },
        core::w,
    };
    let prefix = "<html><body><!--StartFragment-->";
    let suffix = "<!--EndFragment--></body></html>";
    let header = |start: usize, end: usize, a: usize, b: usize| {
        format!(
            "Version:1.0\r\nStartHTML:{start:010}\r\nEndHTML:{end:010}\r\nStartFragment:{a:010}\r\nEndFragment:{b:010}\r\n"
        )
    };
    let start = header(0, 0, 0, 0).len();
    let a = start + prefix.len();
    let b = a + fragment.len();
    let text = format!(
        "{}{prefix}{fragment}{suffix}\0",
        header(start, b + suffix.len(), a, b)
    );
    unsafe {
        if OpenClipboard(None).is_err() {
            return;
        }
        if let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, text.len()) {
            let ptr = GlobalLock(memory);
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(text.as_ptr(), ptr.cast::<u8>(), text.len());
                let _ = GlobalUnlock(memory);
                let format = RegisterClipboardFormatW(w!("HTML Format"));
                if SetClipboardData(format, Some(HANDLE(memory.0))).is_err() {
                    let _ = windows::Win32::Foundation::GlobalFree(Some(memory));
                }
            } else {
                let _ = windows::Win32::Foundation::GlobalFree(Some(memory));
            }
        }
        let _ = CloseClipboard();
    }
}

pub const UI_FONT: &str = "Microsoft YaHei UI";

pub fn set_pinned(window: &gpui::Window, pinned: bool) -> Result<()> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::UI::WindowsAndMessaging::*;
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|error| anyhow::anyhow!("无法获取便签窗口：{error:?}"))?;
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        anyhow::bail!("无法获取便签窗口");
    };
    unsafe {
        SetWindowPos(
            HWND(raw.hwnd.get() as _),
            Some(if pinned { HWND_TOPMOST } else { HWND_NOTOPMOST }),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )?;
    }
    Ok(())
}

pub fn present_window(
    window: &gpui::Window,
    width: f32,
    reduced_motion: bool,
    cx: &mut gpui::App,
) -> Result<()> {
    apply_window_appearance(window);
    let animation = if reduced_motion {
        Ok(())
    } else {
        animate_entrance(window, width, cx)
    };
    let visibility = show_window(window);
    animation.and(visibility)
}

fn animate_entrance(window: &gpui::Window, width: f32, cx: &mut gpui::App) -> Result<()> {
    use std::time::{Duration, Instant};
    let entrance = match WindowEntrance::begin(window) {
        Ok(Some(entrance)) => entrance,
        Ok(None) => return Ok(()),
        Err(error) => {
            let _ = window.set_visual_appearance(1., 1., gpui::point(0.5, 0.));
            return Err(error);
        }
    };
    let handle = window.window_handle();
    let duration = if width > 600. { 0.20 } else { 0.18 };
    cx.spawn(async move |cx| {
        let start = Instant::now();
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(8))
                .await;
            let progress = (start.elapsed().as_secs_f32() / duration).min(1.);
            handle.update(cx, |_, window, _| {
                let result = entrance.apply(window, progress);
                if result.is_err() {
                    entrance.finish(window);
                }
                window.refresh();
                result
            })??;
            if progress >= 1. {
                return Ok::<(), anyhow::Error>(());
            }
        }
    })
    .detach();
    Ok(())
}

fn apply_window_appearance(window: &gpui::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Graphics::Dwm::*;
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(raw.hwnd.get() as _);
    let caption: u32 = 0xebf3f6;
    let text: u32 = 0x2b2d26;
    let light = 0i32;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &light as *const _ as _,
            4,
        );
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, &caption as *const _ as _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_TEXT_COLOR, &text as *const _ as _, 4);
    }
}

pub fn quiet_command(program: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new(program);
    command.creation_flags(0x08000000);
    command
}
