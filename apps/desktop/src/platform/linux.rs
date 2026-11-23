use anyhow::{Context, Result};
use gpui::Window;
use std::{path::Path, process::Command};

pub const UI_FONT: &str = "Noto Sans CJK SC";

pub fn drag_window(window: &Window) -> Result<()> {
    window.start_window_move();
    Ok(())
}

pub fn open_attachment(path: &Path) -> Result<()> {
    anyhow::ensure!(path.is_file(), "附件文件不存在或无法读取");
    open_location(path)
}

pub fn open_location(path: &Path) -> Result<()> {
    Command::new("xdg-open")
        .arg(std::path::absolute(path)?)
        .spawn()
        .context("无法使用默认应用打开路径")?;
    Ok(())
}

pub fn quiet_command(program: &str) -> Command {
    Command::new(program)
}
