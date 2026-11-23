use gpui::*;
use std::borrow::Cow;
pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        // Component icons use canonical names; do not confuse Check with our checklist tool.
        if path.starts_with("icons/") {
            let bytes: &'static [u8] = match path {
                "icons/asterisk.svg" => {
                    include_bytes!("../../../assets/icons/components/asterisk.svg")
                }
                "icons/case-sensitive.svg" => {
                    include_bytes!("../../../assets/icons/components/case-sensitive.svg")
                }
                "icons/check.svg" => include_bytes!("../../../assets/icons/components/check.svg"),
                "icons/chevron-down.svg" => {
                    include_bytes!("../../../assets/icons/components/chevron-down.svg")
                }
                "icons/chevron-left.svg" => {
                    include_bytes!("../../../assets/icons/components/chevron-left.svg")
                }
                "icons/chevron-right.svg" => {
                    include_bytes!("../../../assets/icons/components/chevron-right.svg")
                }
                "icons/circle-x.svg" => {
                    include_bytes!("../../../assets/icons/components/circle-x.svg")
                }
                "icons/external-link.svg" => {
                    include_bytes!("../../../assets/icons/components/external-link.svg")
                }
                "icons/eye.svg" => include_bytes!("../../../assets/icons/components/eye.svg"),
                "icons/minus.svg" => include_bytes!("../../../assets/icons/components/minus.svg"),
                "icons/plus.svg" => include_bytes!("../../../assets/icons/components/plus.svg"),
                "icons/replace.svg" => {
                    include_bytes!("../../../assets/icons/components/replace.svg")
                }
                "icons/chevron-up.svg" => {
                    include_bytes!("../../../assets/icons/components/chevron-up.svg")
                }
                "icons/loader-circle.svg" => {
                    include_bytes!("../../../assets/icons/components/loader-circle.svg")
                }
                "icons/close.svg" => include_bytes!("../../../assets/icons/lucide/close.svg"),
                _ => return Ok(None),
            };
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        let path = path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .trim_end_matches(".svg");
        let bytes: &'static [u8] = match path {
            "loading" => include_bytes!("../../../assets/icons/components/loader-circle.svg"),
            "pencil" => include_bytes!("../../../assets/icons/lucide/pencil.svg"),
            "minus" => include_bytes!("../../../assets/icons/lucide/minus.svg"),
            "maximize" => include_bytes!("../../../assets/icons/lucide/maximize.svg"),
            "search" => include_bytes!("../../../assets/icons/lucide/search.svg"),
            "play" => include_bytes!("../../../assets/icons/lucide/play.svg"),
            "pause" => include_bytes!("../../../assets/icons/lucide/pause.svg"),
            "stop" => include_bytes!("../../../assets/icons/lucide/stop.svg"),
            "right" => include_bytes!("../../../assets/icons/lucide/right.svg"),
            "down" => include_bytes!("../../../assets/icons/lucide/down.svg"),
            "plus" => include_bytes!("../../../assets/icons/lucide/plus.svg"),
            "close" => include_bytes!("../../../assets/icons/lucide/close.svg"),
            "tick" => include_bytes!("../../../assets/icons/lucide/tick.svg"),
            "check" => include_bytes!("../../../assets/icons/lucide/check.svg"),
            "list" => include_bytes!("../../../assets/icons/lucide/list.svg"),
            "image" => include_bytes!("../../../assets/icons/lucide/image.svg"),
            "mic" => include_bytes!("../../../assets/icons/lucide/mic.svg"),
            "link" => include_bytes!("../../../assets/icons/lucide/link.svg"),
            "pin" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M9 3h6l-1 7 4 4v2H6v-2l4-4-1-7ZM12 16v5" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "pin-filled" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M9 3h6l-1 7 4 4v2H6v-2l4-4-1-7Z" fill="#000"/><path d="M12 16v5" stroke="#000" stroke-width="1.8" stroke-linecap="round"/></svg>"##,
            "settings" => include_bytes!("../../../assets/icons/lucide/settings.svg"),
            "book" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3 3h7l2 2 2-2h7v17h-7l-2 1-2-1H3V3Z M12 5v16" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "home" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M2 11 12 3l10 8 M5 9v12h5v-7h4v7h5V9" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "heart" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M12 21 3 12C-3 4 7-1 12 6 17-1 27 4 21 12L12 21Z" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "wallet" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3 5h17v16H3V5Z M3 5l14-3v3 M15 11h6v5h-6v-5Z" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "plane" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="m3 10 7 1 6-8 3 1-4 9 6 4-1 2-8-2-4 4-2-1 2-5-6-3 1-2Z" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "calendar" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3 5h18v16H3V5Z M7 2v6 M17 2v6 M3 10h18 M7 14h3 M14 14h3 M7 18h3" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "star" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="m12 2 3 6 7 1-5 5 1 7-6-3-6 3 1-7-5-5 7-1 3-6Z" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "folder" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3 5h7l2 3h9v13H3V5Z" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "inbox" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M4 4h16l2 12v4H2v-4L4 4Z M2 15h6l2 3h4l2-3h6" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "briefcase" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M9 7V4h6v3 M3 7h18v14H3V7Z M3 12h18 M10 11v4h4v-4" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "bulb" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M9 18h6 M9 21h6 M8 15a7 7 0 1 1 8 0l-1 3H9l-1-3Z" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "coffee" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3 8h13v9a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4V8Z M16 9h2a4 4 0 0 1 0 8h-2 M6 2v3 M11 2v3" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "dot" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="4" fill="#000"/></svg>"##,
            "grid" => include_bytes!("../../../assets/icons/lucide/grid.svg"),
            "history" => include_bytes!("../../../assets/icons/lucide/history.svg"),
            "trash" => include_bytes!("../../../assets/icons/lucide/trash.svg"),
            "file" => include_bytes!("../../../assets/icons/lucide/file.svg"),
            "more" => include_bytes!("../../../assets/icons/lucide/more.svg"),
            "ordered" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M4 3h1v6M3 9h4M3 15c0-3 4-3 4 0 0 1-4 4-4 6h4M11 5h10M11 12h10M11 19h10" fill="none" stroke="#000" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "quote" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M4 5h6v8H4V5Zm0 8c0 4 2 6 6 6M14 5h6v8h-6V5Zm0 8c0 4 2 6 6 6" fill="none" stroke="#000" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "code" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="m7 7-5 5 5 5M17 7l5 5-5 5M14 4l-4 16" fill="none" stroke="#000" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>"##,
            "bold" => include_bytes!("../../../assets/icons/lucide/bold.svg"),
            "italic" => include_bytes!("../../../assets/icons/lucide/italic.svg"),
            "brand" => br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3 4H16V17H3ZM8 8H21V21H8Z" fill="none" stroke="#000" stroke-width="1.6"/></svg>"##,
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(bytes)))
    }

    fn list(&self, _: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(vec![])
    }
}
pub fn icon(name: &'static str) -> gpui::Svg {
    svg().path(name).size(px(20.)).text_color(rgb(0x262d2b))
}

#[cfg(test)]
mod tests {
    use super::Assets;
    use gpui::AssetSource;
    #[test]
    fn component_check_is_not_the_checklist_tool() {
        let assets = Assets;
        assert_ne!(
            assets.load("icons/check.svg").unwrap().unwrap(),
            assets.load("check").unwrap().unwrap()
        );
        for path in [
            "icons/check.svg",
            "icons/chevron-down.svg",
            "icons/chevron-right.svg",
            "icons/close.svg",
            "icons/circle-x.svg",
        ] {
            assert!(assets.load(path).unwrap().is_some(), "missing {path}");
        }
    }
}
