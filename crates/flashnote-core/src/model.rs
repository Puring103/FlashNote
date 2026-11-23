use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DEFAULT_GROUP: &str = "00000000-0000-0000-0000-000000000000";
pub fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
pub fn id() -> String {
    Uuid::new_v4().to_string()
}

pub const GROUP_COLORS: [u32; 12] = [
    0x245356, 0x347d78, 0x4078a0, 0x5264a0, 0x8063a0, 0xa36387, 0xb86543, 0xb84d50, 0xc39b45,
    0x747e49, 0x4b7759, 0x7c8179,
];
pub const GROUP_ICONS: [(&str, &str); 12] = [
    ("inbox", "收集"),
    ("briefcase", "工作"),
    ("bulb", "灵感"),
    ("coffee", "生活"),
    ("book", "学习"),
    ("home", "家庭"),
    ("heart", "健康"),
    ("wallet", "财务"),
    ("plane", "旅行"),
    ("calendar", "计划"),
    ("star", "收藏"),
    ("folder", "项目"),
];
pub fn default_group_icon() -> String {
    "dot".into()
}
pub fn group_icon(name: &str) -> &'static str {
    // Keep previously saved icon keys readable without offering unrelated editor tools.
    match name {
        "dot" => return "dot",
        "file" => return "file",
        "pencil" => return "pencil",
        "list" => return "list",
        "image" => return "image",
        "mic" => return "mic",
        "link" => return "link",
        "pin" => return "pin",
        _ => {}
    }
    GROUP_ICONS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(key, _)| *key)
        .unwrap_or("dot")
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Group {
    #[serde(default)]
    pub order: i64,
    #[serde(default)]
    pub deleted: bool,
    pub id: String,
    pub name: String,
    pub color: u32,
    #[serde(default = "default_group_icon")]
    pub icon: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Markdown {
        id: String,
        text: String,
    },
    Image {
        id: String,
        asset: String,
        name: String,
    },
    RemoteImage {
        id: String,
        url: String,
        name: String,
    },
    Audio {
        id: String,
        asset: String,
        duration_ms: u64,
        text: Option<String>,
        state: AudioState,
    },
    File {
        id: String,
        asset: String,
        name: String,
    },
}
impl Block {
    pub fn id(&self) -> &str {
        match self {
            Self::Markdown { id, .. }
            | Self::RemoteImage { id, .. }
            | Self::Image { id, .. }
            | Self::Audio { id, .. }
            | Self::File { id, .. } => id,
        }
    }
}
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub enum AudioState {
    Recorded,
    Processing,
    Ready,
    Failed(String),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Note {
    pub id: String,
    pub group_id: String,
    pub title: String,
    pub blocks: Vec<Block>,
    pub links: Vec<String>,
    #[serde(default)]
    pub media_archive: Vec<Block>,
    #[serde(default)]
    pub media_depths: std::collections::HashMap<String, u8>,
    pub created_at: i64,
    pub updated_at: i64,
    pub revision: String,
    pub parent_revision: Option<String>,
    pub deleted_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_of: Option<String>,
    #[serde(default)]
    pub conflict_resolved: bool,
}
impl Note {
    pub fn new() -> Self {
        Self {
            id: id(),
            group_id: DEFAULT_GROUP.into(),
            title: String::new(),
            blocks: vec![Block::Markdown {
                id: id(),
                text: String::new(),
            }],
            links: vec![],
            media_archive: vec![],
            media_depths: Default::default(),
            created_at: now(),
            updated_at: now(),
            revision: id(),
            parent_revision: None,
            deleted_at: None,
            conflict_of: None,
            conflict_resolved: false,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.title.trim().is_empty()
            && self.blocks.iter().all(|b| match b {
                Block::Markdown { text, .. } => {
                    gpui_note_editor::document::Document::from_markdown(text)
                        .text()
                        .chars()
                        .all(|c| c.is_whitespace() || c == '\u{200b}' || c == '\u{feff}')
                }
                Block::RemoteImage { .. } => false,
                Block::Image { asset, .. }
                | Block::Audio { asset, .. }
                | Block::File { asset, .. } => asset.is_empty(),
            })
    }
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Markdown { text, .. } => Some(text.as_str()),
                Block::Audio {
                    text: Some(text), ..
                } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[cfg(test)]
    pub fn markdown(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Markdown { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    pub fn set_markdown(&mut self, value: String) {
        if let Some(Block::Markdown { text, .. }) = self
            .blocks
            .iter_mut()
            .find(|b| matches!(b, Block::Markdown { .. }))
        {
            *text = value;
        } else {
            self.blocks.insert(
                0,
                Block::Markdown {
                    id: id(),
                    text: value,
                },
            );
        }
    }
    pub fn editor_markdown(&self) -> String {
        self.blocks
            .iter()
            .map(|b| match b {
                Block::Markdown { text, .. } => text.clone(),
                _ => format!(
                    "[素材](attachment:{}?kind=media&depth={})",
                    b.id(),
                    self.media_depths.get(b.id()).copied().unwrap_or(0)
                ),
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
    pub fn set_editor_markdown(&mut self, source: &str) {
        use gpui_note_editor::document::{BlockKind, Document};
        let mut document = Document::from_markdown(source);
        crate::remote_images::localize(&mut document, &mut self.media_archive);
        let mut available = self.media_archive.clone();
        available.extend(
            self.blocks
                .iter()
                .filter(|b| !matches!(b, Block::Markdown { .. }))
                .cloned(),
        );
        let mut blocks = vec![];
        let mut text = vec![];
        for b in document.blocks {
            if let BlockKind::Attachment {
                id: ref bid, depth, ..
            } = b.kind
            {
                self.media_depths.insert(bid.clone(), depth);
                if !text.is_empty() {
                    blocks.push(Block::Markdown {
                        id: id(),
                        text: Document {
                            blocks: std::mem::take(&mut text),
                        }
                        .to_markdown(),
                    });
                }
                if let Some(media) = available.iter().rev().find(|a| a.id() == bid) {
                    blocks.push(media.clone());
                }
            } else {
                text.push(b);
            }
        }
        if !text.is_empty() {
            blocks.push(Block::Markdown {
                id: id(),
                text: Document { blocks: text }.to_markdown(),
            });
        }
        // Preserve identities of unchanged text segments across serialization.
        let mut used = std::collections::HashSet::new();
        for block in &mut blocks {
            if let Block::Markdown { id: bid, text } = block {
                if let Some(Block::Markdown { id: previous, .. }) = self.blocks.iter().find(|old| {
                    matches!(old, Block::Markdown { id, text: old_text } if old_text == text && !used.contains(id))
                }) {
                    *bid = previous.clone();
                    used.insert(previous.clone());
                }
            }
        }
        available.retain(|a| !blocks.iter().any(|b| b.id() == a.id()));
        available.sort_by(|a, b| a.id().cmp(b.id()));
        available.dedup_by(|a, b| a.id() == b.id());
        self.media_archive = available;
        self.blocks = blocks;
    }
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let hay = format!("{} {}", self.title, self.text()).to_lowercase();
        if hay.contains(&query) {
            return true;
        }
        let mut chars = hay.chars();
        query.chars().all(|c| chars.by_ref().any(|h| h == c))
            || hay
                .split_whitespace()
                .any(|w| strsim::normalized_levenshtein(w, &query) > 0.72)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Settings {
    pub speech_model: String,
    pub whisper_cli: String,
    pub model_path: String,
    pub reduced_motion: bool,
    pub auto_transcribe: bool,
    pub start_on_login: bool,
    pub new_shortcut: String,
    pub center_shortcut: String,
    pub desktop_preferences_version: u32,
    pub history_days: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            speech_model: "tiny".into(),
            whisper_cli: String::new(),
            model_path: String::new(),
            reduced_motion: false,
            auto_transcribe: true,
            start_on_login: false,
            new_shortcut: "Alt + N".into(),
            center_shortcut: "Alt + J".into(),
            desktop_preferences_version: 0,
            history_days: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn toolbar_generates_valid_content_without_empty_placeholders() {
        assert_eq!(format_snippet("todo", "买咖啡").unwrap(), "- [ ] 买咖啡");
        assert_eq!(format_snippet("bold", "重点").unwrap(), "**重点**");
        assert!(format_snippet("link", "https://").is_err());
        assert!(format_snippet("link", "javascript:alert(1)").is_err());
        assert!(format_snippet("italic", "  ").is_err());
    }
    #[test]
    fn mixed_note_search_includes_transcript() {
        let mut n = Note::new();
        n.title = "产品讨论".into();
        n.blocks.push(Block::Audio {
            id: id(),
            asset: id(),
            duration_ms: 20,
            text: Some("整理下一周的计划".into()),
            state: AudioState::Ready,
        });
        assert!(n.matches("下周计划"));
        assert!(!n.matches("完全不存在"));
    }
}

pub fn format_snippet(kind: &str, value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("请输入内容");
    }
    Ok(match kind {
        "bold" => format!("**{value}**"),
        "italic" => format!("*{value}*"),
        "todo" => value
            .lines()
            .map(|s| format!("- [ ] {}", s.trim()))
            .collect::<Vec<_>>()
            .join("\n"),
        "list" => value
            .lines()
            .map(|s| format!("- {}", s.trim()))
            .collect::<Vec<_>>()
            .join("\n"),
        "link" => {
            if value
                .split_once("://")
                .is_none_or(|(_, host)| host.is_empty() || host.starts_with('/'))
                || !(value.starts_with("https://") || value.starts_with("http://"))
                || value.contains(['\n', '\r', ' ', '(', ')'])
            {
                anyhow::bail!("请输入有效的 http:// 或 https:// 链接");
            }
            format!("[链接]({value})")
        }
        _ => anyhow::bail!("未知格式"),
    })
}

#[cfg(test)]
mod media_flow_tests {
    use super::*;
    #[test]
    fn repeated_serialization_preserves_text_block_identities() {
        let mut note = Note::new();
        note.blocks.push(Block::File {
            id: id(),
            asset: id(),
            name: "file".into(),
        });
        let bid = note.blocks.last().unwrap().id().to_owned();
        note.set_editor_markdown(&format!(
            "重复\n\n[素材](attachment:{bid}?kind=media)\n\n重复"
        ));
        let before = serde_json::to_value(&note).unwrap();
        note.set_editor_markdown(&note.editor_markdown());
        assert_eq!(serde_json::to_value(&note).unwrap(), before);
    }
    #[test]
    fn ordered_media_survives_save_delete_and_undo_reference() {
        let mut note = Note::new();
        let bid = id();
        note.blocks.push(Block::File {
            id: bid.clone(),
            asset: id(),
            name: "行程.pdf".into(),
        });
        let source = format!("前文\n\n[素材](attachment:{bid}?kind=media)\n\n后文");
        note.set_editor_markdown(&source);
        assert!(matches!(note.blocks[0], Block::Markdown { .. }));
        assert!(matches!(note.blocks[1], Block::File { .. }));
        assert!(matches!(note.blocks[2], Block::Markdown { .. }));
        let serialized = serde_json::to_string(&note).unwrap();
        let mut note: Note = serde_json::from_str(&serialized).unwrap();
        note.set_editor_markdown("前文后文");
        assert_eq!(note.media_archive.len(), 1);
        note.set_editor_markdown(&source);
        assert!(matches!(note.blocks[1], Block::File { .. }));
        assert!(note.media_archive.is_empty());
        assert_eq!(note.text(), "前文\n后文");
    }
}
