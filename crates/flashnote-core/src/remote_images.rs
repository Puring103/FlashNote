//! External images are durable note blocks. Retry/error bookkeeping is local,
//! and neither an image URL nor its original bytes is sent in plaintext.
use crate::{
    media,
    model::{Block, id},
    store::Store,
};
use anyhow::{Result, ensure};
use gpui_note_editor::document::{BlockKind, Document};
use std::{io::Read, time::Duration};

pub fn contains(source: &str) -> bool {
    if !source.contains("![") {
        return false;
    }
    Document::from_markdown(source).blocks.iter().any(|b| {
        !matches!(b.kind, BlockKind::Code { .. })
            && b.content
                .iter()
                .any(|s| s.marks.image && s.marks.link.as_deref().is_some_and(valid_url))
    })
}

pub fn localize(document: &mut Document, available: &mut Vec<Block>) {
    let mut output = vec![];
    for mut block in std::mem::take(&mut document.blocks) {
        if matches!(block.kind, BlockKind::Code { .. }) {
            output.push(block);
            continue;
        }
        let depth = match block.kind {
            BlockKind::Bullet { depth }
            | BlockKind::Ordered { depth, .. }
            | BlockKind::Task { depth, .. }
            | BlockKind::Attachment { depth, .. } => depth,
            _ => 0,
        };
        let mut content = vec![];
        for span in std::mem::take(&mut block.content) {
            let url = span
                .marks
                .link
                .as_deref()
                .filter(|url| span.marks.image && valid_url(url));
            if let Some(url) = url {
                if !content.is_empty() {
                    let mut before = block.clone();
                    before.content = std::mem::take(&mut content);
                    output.push(before);
                }
                let bid = id();
                available.push(Block::RemoteImage {
                    id: bid.clone(),
                    url: url.into(),
                    name: span.text.clone(),
                });
                output.push(gpui_note_editor::document::Block {
                    kind: BlockKind::Attachment {
                        id: bid,
                        kind: "media".into(),
                        label: "待下载图片".into(),
                        depth,
                    },
                    content: vec![],
                });
            } else {
                content.push(span);
            }
        }
        if !content.is_empty() || output.is_empty() {
            block.content = content;
            output.push(block);
        }
    }
    document.blocks = output;
}

pub fn valid_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    }) || value.starts_with("data:image/")
}

pub fn download(url: &str) -> Result<media::PreparedAttachment> {
    ensure!(valid_url(url), "只支持 HTTP、HTTPS 或内嵌图片");
    let bytes = if url.starts_with("data:image/") {
        use base64::Engine;
        let (header, value) = url
            .split_once(',')
            .ok_or_else(|| anyhow::anyhow!("内嵌图片格式错误"))?;
        ensure!(
            header.ends_with(";base64") && value.len() as u64 <= media::MAX_FILE * 4 / 3 + 8,
            "内嵌图片格式或大小不支持"
        );
        base64::engine::general_purpose::STANDARD.decode(value)?
    } else {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(25))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()?;
        // No application token, cookies, authorization or sync credentials.
        let response = client.get(url).send()?.error_for_status()?;
        ensure!(
            response
                .content_length()
                .is_none_or(|n| n <= media::MAX_FILE),
            "图片超过 32MB"
        );
        let mut bytes = vec![];
        response.take(media::MAX_FILE + 1).read_to_end(&mut bytes)?;
        bytes
    };
    let format = image::guess_format(&bytes)?;
    let ext = format.extensions_str().first().copied().unwrap_or("png");
    media::prepare_bytes(format!("网页图片.{ext}"), bytes, true)
}

impl Store {
    /// Apply to the latest note, only if the pending block is still present.
    /// A removed image or deleted note is never resurrected by a late response.
    pub fn complete_remote_image(
        &mut self,
        nid: &str,
        bid: &str,
        url: &str,
        prepared: media::PreparedAttachment,
    ) -> Result<bool> {
        let Some(mut note) = self
            .notes
            .iter()
            .find(|n| n.id == nid && n.deleted_at.is_none())
            .cloned()
        else {
            return Ok(false);
        };
        let Some(block) = note
            .blocks
            .iter_mut()
            .find(|b| matches!(b, Block::RemoteImage{id,url:source,..} if id==bid && source==url))
        else {
            return Ok(false);
        };
        // The same pending block downloaded on two devices must converge even
        // when both finish before receiving the other's result. UUIDv5 is scoped
        // to the random block ID and the exact original image bytes.
        let asset = uuid::Uuid::new_v5(&uuid::Uuid::parse_str(bid)?, &prepared.bytes).to_string();
        let path = self.root.join("assets").join(&asset);
        use std::io::Write;
        let created = match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(error) = file
                    .write_all(&prepared.bytes)
                    .and_then(|_| file.sync_all())
                {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(error.into());
                }
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                ensure!(
                    std::fs::read(&path)? == prepared.bytes,
                    "图片附件 ID 已存在且内容不一致"
                );
                false
            }
            Err(error) => return Err(error.into()),
        };
        *block = Block::Image {
            id: bid.into(),
            asset: asset.clone(),
            name: prepared.name,
        };
        if let Err(e) = self.save_note(note) {
            if created {
                let _ = std::fs::remove_file(path);
            }
            return Err(e);
        }
        Ok(true)
    }
}

/// Read Windows CF_HTML without interpreting or executing HTML. The clipboard
/// owner controls the offsets, so validate them against GlobalSize first.
#[cfg(windows)]
pub fn clipboard_html() -> Option<String> {
    use windows::{
        Win32::{
            Foundation::HGLOBAL,
            System::{DataExchange::*, Memory::*},
        },
        core::w,
    };
    unsafe {
        let format = RegisterClipboardFormatW(w!("HTML Format"));
        if !IsClipboardFormatAvailable(format).is_ok() || OpenClipboard(None).is_err() {
            return None;
        }
        struct Close;
        impl Drop for Close {
            fn drop(&mut self) {
                unsafe {
                    let _ = CloseClipboard();
                }
            }
        }
        let _close = Close;
        let handle = GetClipboardData(format).ok()?;
        let global = HGLOBAL(handle.0);
        let len = GlobalSize(global);
        if len == 0 || len > 8 * 1024 * 1024 {
            return None;
        }
        let ptr = GlobalLock(global);
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), len).to_vec();
        let _ = GlobalUnlock(global);
        html_fragment(&bytes)
    }
}

#[cfg(windows)]
pub fn html_fragment(bytes: &[u8]) -> Option<String> {
    let header = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let offset = |key: &str| -> Option<usize> {
        header
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|s| s.trim().parse().ok())
    };
    let start = offset("StartFragment:")?;
    let end = offset("EndFragment:")?;
    if start > end || end > bytes.len() {
        return None;
    }
    let html = std::str::from_utf8(&bytes[start..end]).ok()?;
    let mut markdown = html2md::parse_html(html);
    // CF_HTML may carry relative image URLs. Resolve against the source page.
    if let Some(base) = header
        .lines()
        .find_map(|l| l.strip_prefix("SourceURL:"))
        .and_then(|s| reqwest::Url::parse(s.trim()).ok())
    {
        let mut doc = Document::from_markdown(&markdown);
        for block in &mut doc.blocks {
            for span in &mut block.content {
                if span.marks.image {
                    if let Some(url) = &span.marks.link {
                        if let Ok(joined) = base.join(url) {
                            span.marks.link = Some(joined.to_string());
                        }
                    }
                }
            }
        }
        markdown = doc.to_markdown();
    }
    Some(markdown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Note;

    #[test]
    fn markdown_images_preserve_text_order_depth_and_pending_state_across_restart() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().join("vault")).unwrap();
        let mut note = Note::new();
        note.set_editor_markdown("前面 ![图片](https://example.com/private.png) 后面\n\n[普通链接](https://example.com/link)\n\n```\n![代码](https://example.com/code.png)\n```\n\n- 清单\n  - ![缩进](https://example.com/indent.png)");
        assert_eq!(
            note.blocks
                .iter()
                .filter(|b| matches!(b, Block::RemoteImage { .. }))
                .count(),
            2
        );
        assert!(matches!(note.blocks[1], Block::RemoteImage { .. }));
        assert!(note.text().contains("前面"));
        assert!(note.text().contains("后面"));
        assert!(note.text().contains("code.png"));
        let bid = note
            .blocks
            .iter()
            .find_map(|b| {
                if let Block::RemoteImage { id, url, .. } = b {
                    if url.ends_with("indent.png") {
                        Some(id.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(note.media_depths.get(&bid), Some(&1));
        store.save_note(note).unwrap();
        let saved = store.notes[0].editor_markdown();
        let root = store.root.clone();
        drop(store);
        let store = Store::open(root).unwrap();
        assert_eq!(store.notes[0].editor_markdown(), saved);
        assert_eq!(
            store.notes[0]
                .blocks
                .iter()
                .filter(|b| matches!(b, Block::RemoteImage { .. }))
                .count(),
            2
        );
    }

    #[cfg(windows)]
    #[test]
    fn html_offsets_relative_images_and_invalid_offsets() {
        let html =
            "<p>前面<img src=\"/private.png\" alt=\"图片\">后面</p><script>alert('no')</script>";
        let header = "Version:1.0\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\nSourceURL:https://example.com/page\r\n";
        let content = format!(
            "{}{}",
            header
                .replace(
                    "StartFragment:0000000000",
                    &format!("StartFragment:{:010}", header.len())
                )
                .replace(
                    "EndFragment:0000000000",
                    &format!("EndFragment:{:010}", header.len() + html.len())
                ),
            html
        );
        let text = html_fragment(content.as_bytes()).unwrap();
        let mut note = Note::new();
        note.set_editor_markdown(&text);
        assert!(note.blocks.iter().any(
            |b| matches!(b,Block::RemoteImage{url,..} if url=="https://example.com/private.png")
        ));
        assert!(html_fragment(b"StartFragment:999999\r\nEndFragment:1").is_none());
    }

    #[test]
    fn empty_alt_adjacent_and_styled_alt_are_each_one_image() {
        let mut note = Note::new();
        note.set_editor_markdown(
            "前 ![](https://example.com/a)![**粗体** 文字](https://example.com/a) 后",
        );
        assert_eq!(
            note.blocks
                .iter()
                .filter(|b| matches!(b, Block::RemoteImage { .. }))
                .count(),
            2
        );
        assert!(note.text().contains("前"));
        assert!(note.text().contains("后"));
    }

    #[test]
    fn successful_download_validates_bytes_and_late_completion_does_not_restore_removals() {
        use base64::Engine;
        let bytes = include_bytes!("../../../assets/demo/desk.jpg");
        let url = format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        let prepared = download(&url).unwrap();
        assert_eq!(prepared.bytes, bytes);
        assert!(download("data:image/png;base64,bm90IGFuIGltYWdl").is_err());
        assert!(!valid_url("file:///C:/private.png"));
        assert!(!valid_url("https://name:password@example.com/image"));
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().join("vault")).unwrap();
        let mut note = Note::new();
        note.set_editor_markdown("前 ![图](https://example.com/image) 后");
        let nid = note.id.clone();
        let bid = note
            .blocks
            .iter()
            .find(|b| matches!(b, Block::RemoteImage { .. }))
            .unwrap()
            .id()
            .to_string();
        store.save_note(note).unwrap();
        let mut note = store.notes[0].clone();
        note.set_editor_markdown("用户已经移除图片");
        store.save_note(note).unwrap();
        assert!(
            !store
                .complete_remote_image(&nid, &bid, "https://example.com/image", prepared)
                .unwrap()
        );
        assert_eq!(
            std::fs::read_dir(store.root.join("assets"))
                .unwrap()
                .count(),
            0
        );
        let mut note = store.notes[0].clone();
        note.set_editor_markdown("最新文字 ![图](https://example.com/new) 末尾");
        store.save_note(note).unwrap();
        let bid = store.notes[0]
            .blocks
            .iter()
            .find(|b| matches!(b, Block::RemoteImage { .. }))
            .unwrap()
            .id()
            .to_string();
        assert!(
            store
                .complete_remote_image(
                    &nid,
                    &bid,
                    "https://example.com/new",
                    download(&url).unwrap()
                )
                .unwrap()
        );
        assert!(store.notes[0].text().contains("最新文字"));
        let aid = store.notes[0]
            .blocks
            .iter()
            .find_map(|b| {
                if let Block::Image { asset, .. } = b {
                    Some(asset)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(store.asset(aid).unwrap(), bytes);
    }
}
