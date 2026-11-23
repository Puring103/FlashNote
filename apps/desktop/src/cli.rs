mod search;

use crate::{
    model::{Block, DEFAULT_GROUP, Note},
    store::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    cmp::Reverse,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub const DEFAULT_LIMIT: u16 = 20;
pub const MAX_LIMIT: u16 = 100;
pub const MAX_INPUT: usize = 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub struct CliError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Input,
    NotFound,
    Busy,
    Conflict,
    Io,
}

impl ErrorKind {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Input => 2,
            Self::NotFound => 3,
            Self::Busy => 4,
            Self::Conflict => 5,
            Self::Io => 1,
        }
    }
}

impl CliError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    pub fn io(error: impl std::fmt::Display) -> Self {
        Self::new(ErrorKind::Io, error.to_string())
    }
}

pub type Reply = Result<Value, CliError>;

#[derive(Clone, Copy, Default, Serialize, Deserialize, clap::ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchScope {
    #[default]
    All,
    Title,
    Body,
    Transcript,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Request {
    List {
        filter: Filter,
    },
    Search {
        query: String,
        exact: bool,
        full: bool,
        #[serde(default)]
        scope: SearchScope,
        filter: Filter,
    },
    Get {
        id: String,
    },
    GetMany {
        ids: Vec<String>,
    },
    Export {
        ids: Vec<String>,
    },
    Open {
        id: String,
    },
    Groups,
    GroupCreate {
        name: String,
        color: u32,
    },
    GroupUpdate {
        group: String,
        name: Option<String>,
        color: Option<u32>,
    },
    GroupDelete {
        group: String,
        destination: String,
    },
    Create {
        title: String,
        group: Option<String>,
        markdown: String,
        #[serde(default)]
        attachments: Vec<PathBuf>,
    },
    Append {
        id: String,
        revision: Option<String>,
        markdown: String,
        #[serde(default)]
        attachments: Vec<PathBuf>,
    },
    Update {
        id: String,
        revision: String,
        title: Option<String>,
        group: Option<String>,
        markdown: Option<String>,
    },
    Attachment {
        id: String,
    },
}

impl Request {
    pub fn writes_store(&self) -> bool {
        matches!(
            self,
            Self::Create { .. }
                | Self::Append { .. }
                | Self::Update { .. }
                | Self::GroupCreate { .. }
                | Self::GroupUpdate { .. }
                | Self::GroupDelete { .. }
        )
    }
}

#[derive(Serialize, Deserialize)]
pub struct Filter {
    pub group: Option<String>,
    pub since: Option<i64>,
    #[serde(default)]
    pub before: Option<i64>,
    pub limit: u16,
    pub cursor: Option<String>,
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("SUOJIAN_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir)
}

pub(crate) fn default_data_dir() -> PathBuf {
    let current = directories::ProjectDirs::from("cn", "FlashNote", "FlashNote").unwrap();
    let legacy = directories::ProjectDirs::from("cn", "Suojian", "Suojian").unwrap();
    let (current, legacy) = (current.data_local_dir(), legacy.data_local_dir());
    if !current.join("notes.sqlite3").is_file() && legacy.join("notes.sqlite3").is_file() {
        legacy.to_path_buf()
    } else {
        current.to_path_buf()
    }
}

pub fn execute(store: &mut Store, request: Request) -> Reply {
    match request {
        Request::List { filter } => page(store, filter, None, false),
        Request::Search {
            filter,
            query,
            exact,
            full,
            scope,
        } => {
            let query = search::Query::new(&query, exact, scope)?;
            page(store, filter, Some(query), full)
        }
        Request::Get { id } => Ok(full_note(store, find_note(store, &id)?)),
        Request::GetMany { ids } => read_notes(store, &ids, false),
        Request::Export { ids } => read_notes(store, &ids, true),
        Request::Open { id } => Ok(json!({"id": find_note(store, &id)?.id})),
        Request::Groups => Ok(json!({ "items": store.groups.iter().map(|g| {
            group_value(g)
        }).collect::<Vec<_>>() })),
        Request::GroupCreate { name, color } => {
            validate_group(store, None, name.trim(), color)?;
            let group = store
                .create_group_with_color(&name, color)
                .map_err(CliError::io)?;
            Ok(group_value(&group))
        }
        Request::GroupUpdate { group, name, color } => {
            let mut group = find_group(store, &group)?.clone();
            if name.is_none() && color.is_none() {
                return Err(CliError::new(ErrorKind::Input, "请指定分组名称或颜色"));
            }
            if let Some(name) = name {
                group.name = name.trim().into();
            }
            if let Some(color) = color {
                group.color = color;
            }
            validate_group(store, Some(&group.id), &group.name, group.color)?;
            store.save_group(group.clone()).map_err(CliError::io)?;
            Ok(group_value(&group))
        }
        Request::GroupDelete { group, destination } => {
            let gid = group_id(store, &group)?.to_owned();
            let destination = group_id(store, &destination)?.to_owned();
            if gid == DEFAULT_GROUP || gid == destination {
                return Err(CliError::new(
                    ErrorKind::Input,
                    "默认分组不能删除，迁移目标不能是原分组",
                ));
            }
            let moved = store
                .notes
                .iter()
                .filter(|note| note.group_id == gid)
                .count();
            store
                .delete_group(&gid, &destination)
                .map_err(CliError::io)?;
            Ok(json!({"id": gid, "destination_id": destination, "moved": moved}))
        }
        Request::Create {
            title,
            group,
            markdown,
            attachments,
        } => {
            validate_write(&markdown, &attachments)?;
            let mut note = Note::new();
            note.title = title.trim().into();
            if let Some(group) = group.as_deref() {
                note.group_id = group_id(store, group)?.into();
            }
            note.set_editor_markdown(&markdown);
            if note.is_empty() && attachments.is_empty() {
                return Err(CliError::new(ErrorKind::Input, "便签不能为空"));
            }
            save_with_attachments(store, note, &attachments)
        }
        Request::Append {
            id: note_id,
            revision,
            markdown,
            attachments,
        } => {
            validate_write(&markdown, &attachments)?;
            let mut note = find_note(store, &note_id)?.clone();
            if let Some(revision) = revision.as_deref() {
                check_revision(&note, revision)?;
            }
            // Parse the addition separately so it cannot replace or reposition existing attachments.
            let mut addition = Note::new();
            addition.set_editor_markdown(&markdown);
            if addition.is_empty() && attachments.is_empty() {
                return Err(CliError::new(ErrorKind::Input, "追加内容不能为空"));
            }
            note.blocks.append(&mut addition.blocks);
            note.media_depths.extend(addition.media_depths);
            save_with_attachments(store, note, &attachments)
        }
        Request::Update {
            id,
            revision,
            title,
            group,
            markdown,
        } => {
            let mut note = find_note(store, &id)?.clone();
            check_revision(&note, &revision)?;
            if title.is_none() && group.is_none() && markdown.is_none() {
                return Err(CliError::new(
                    ErrorKind::Input,
                    "请提供要修改的标题、分组或正文",
                ));
            }
            if let Some(title) = title {
                note.title = title.trim().into();
            }
            if let Some(group) = group.as_deref() {
                note.group_id = group_id(store, group)?.into();
            }
            if let Some(markdown) = markdown {
                validate_body(&markdown)?;
                let mut replacement = Note::new();
                replacement.set_editor_markdown(&markdown);
                if replacement.is_empty() {
                    return Err(CliError::new(ErrorKind::Input, "替换正文不能为空"));
                }
                for block in &replacement.blocks {
                    if let Some(depth) = replacement.media_depths.get(block.id()) {
                        note.media_depths.insert(block.id().into(), *depth);
                    }
                }
                // Replace text independently of stored media. Keep existing attachments in order.
                note.blocks
                    .retain(|block| !matches!(block, Block::Markdown { .. }));
                replacement.blocks.append(&mut note.blocks);
                note.blocks = replacement.blocks;
            }
            if note.is_empty() {
                return Err(CliError::new(ErrorKind::Input, "便签不能为空"));
            }
            save(store, note)
        }
        Request::Attachment { id } => {
            let asset_id = Uuid::parse_str(&id)
                .map_err(|_| CliError::new(ErrorKind::Input, "附件 ID 无效"))?;
            let path = asset_path(store, &asset_id.to_string())
                .ok_or_else(|| CliError::new(ErrorKind::NotFound, "附件文件在本地不可用"))?;
            Ok(json!({"id": asset_id, "local_path": path}))
        }
    }
}

fn validate_body(markdown: &str) -> Result<(), CliError> {
    if markdown.len() > MAX_INPUT || markdown.trim().is_empty() {
        return Err(CliError::new(
            ErrorKind::Input,
            "Markdown 不能为空，且最多 1 MiB",
        ));
    }
    Ok(())
}

fn validate_write(markdown: &str, attachments: &[PathBuf]) -> Result<(), CliError> {
    if markdown.trim().is_empty() && !attachments.is_empty() && markdown.len() <= MAX_INPUT {
        return Ok(());
    }
    validate_body(markdown)
}

fn group_value(group: &crate::model::Group) -> Value {
    json!({"id": group.id, "name": group.name, "color": format!("#{:06X}", group.color)})
}

fn validate_group(store: &Store, id: Option<&str>, name: &str, color: u32) -> Result<(), CliError> {
    if name.is_empty() || name.chars().count() > 20 || color > 0xffffff {
        return Err(CliError::new(
            ErrorKind::Input,
            "分组名称须为 1–20 字，颜色须为 RGB",
        ));
    }
    if id == Some(DEFAULT_GROUP) && name != "默认" {
        return Err(CliError::new(ErrorKind::Input, "默认分组不能重命名"));
    }
    if store
        .groups
        .iter()
        .any(|g| Some(g.id.as_str()) != id && g.name.to_lowercase() == name.to_lowercase())
    {
        return Err(CliError::new(ErrorKind::Input, "已有同名分组"));
    }
    Ok(())
}

fn check_revision(note: &Note, revision: &str) -> Result<(), CliError> {
    if note.revision != revision {
        return Err(CliError::new(
            ErrorKind::Conflict,
            "便签版本已变化，请重新读取",
        ));
    }
    Ok(())
}

fn find_note<'a>(store: &'a Store, id: &str) -> Result<&'a Note, CliError> {
    store
        .notes
        .iter()
        .find(|n| n.id == id && n.deleted_at.is_none() && !n.is_empty())
        .ok_or_else(|| CliError::new(ErrorKind::NotFound, format!("找不到便签 {id}")))
}

fn full_note(store: &Store, note: &Note) -> Value {
    let mut value = summary(store, note, None);
    add_body(store, note, &mut value, false);
    value
}

fn read_notes(store: &Store, ids: &[String], portable: bool) -> Reply {
    if !(1..=usize::from(MAX_LIMIT)).contains(&ids.len()) {
        return Err(CliError::new(ErrorKind::Input, "一次可读取 1–100 条便签"));
    }
    let items = ids
        .iter()
        .map(|id| {
            let note = find_note(store, id)?;
            let mut value = summary(store, note, None);
            add_body(store, note, &mut value, portable);
            Ok(value)
        })
        .collect::<Result<Vec<_>, CliError>>()?;
    Ok(json!({"items": items}))
}

fn save(store: &mut Store, note: Note) -> Reply {
    save_with_attachments(store, note, &[])
}

fn save_with_attachments(store: &mut Store, note: Note, attachments: &[PathBuf]) -> Reply {
    use anyhow::Context;
    let id = note.id.clone();
    let prepared = attachments.iter().map(|path| {
        crate::media::prepare_file(path).with_context(|| format!("无法添加附件 {}", path.display()))
    });
    store
        .save_note_with_attachments(note, prepared)
        .map_err(|error| CliError::io(format!("{error:#}")))?;
    let saved = find_note(store, &id)?;
    Ok(json!({ "id": saved.id, "revision": saved.revision }))
}

fn group_id<'a>(store: &'a Store, name: &str) -> Result<&'a str, CliError> {
    Ok(&find_group(store, name)?.id)
}

fn find_group<'a>(store: &'a Store, name: &str) -> Result<&'a crate::model::Group, CliError> {
    if let Some(group) = store.groups.iter().find(|g| g.id == name) {
        return Ok(group);
    }
    let mut matching = store.groups.iter().filter(|g| g.name == name);
    let group = matching
        .next()
        .ok_or_else(|| CliError::new(ErrorKind::NotFound, "找不到分组"))?;
    if matching.next().is_some() {
        return Err(CliError::new(ErrorKind::Input, "分组重名，请指定 ID"));
    }
    Ok(group)
}

fn page(store: &Store, filter: Filter, query: Option<search::Query>, full: bool) -> Reply {
    if !(1..=MAX_LIMIT).contains(&filter.limit) {
        return Err(CliError::new(ErrorKind::Input, "数量范围为 1–100"));
    }
    let group = filter
        .group
        .as_deref()
        .map(|name| group_id(store, name))
        .transpose()?;
    let mut matches: Vec<_> = store
        .notes
        .iter()
        .filter_map(|note| {
            if note.deleted_at.is_some()
                || group.is_some_and(|g| g != note.group_id)
                || filter.since.is_some_and(|since| since > note.updated_at)
                || filter
                    .before
                    .is_some_and(|before| before <= note.updated_at)
                || note.is_empty()
            {
                return None;
            }
            let hit = match &query {
                Some(query) => Some(query.find(note)?),
                None => None,
            };
            Some((note, hit))
        })
        .collect();
    matches.sort_unstable_by_key(|(note, hit)| {
        (
            hit.as_ref().map(|hit| hit.rank),
            Reverse(note.updated_at),
            &note.id,
        )
    });
    let start = match filter.cursor {
        Some(cursor) => matches
            .iter()
            .position(|(n, _)| n.id == cursor)
            .map(|i| i + 1)
            .ok_or_else(|| CliError::new(ErrorKind::Input, "分页游标不可用，请重新查询第一页"))?,
        None => 0,
    };
    let end = (start + usize::from(filter.limit)).min(matches.len());
    let items: Vec<_> = matches[start..end]
        .iter()
        .map(|(note, hit)| {
            let mut value = summary(store, note, hit.as_ref());
            if full {
                add_body(store, note, &mut value, false);
            }
            value
        })
        .collect();
    let next_cursor = (end < matches.len()).then(|| &matches[end - 1].0.id);
    Ok(json!({ "items": items, "next_cursor": next_cursor }))
}

fn summary(store: &Store, note: &Note, hit: Option<&search::Hit>) -> Value {
    let group_name = store
        .groups
        .iter()
        .find(|g| g.id == note.group_id)
        .map(|g| &g.name);
    let preview = match hit {
        Some(hit) => hit.snippet.clone(),
        None => note
            .text()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(120)
            .collect(),
    };
    json!({ "id": note.id, "title": note.title, "group_id": note.group_id, "group": group_name,
        "updated_at": chrono::DateTime::from_timestamp_millis(note.updated_at), "revision": note.revision,
        "snippet": preview, "matched_field": hit.map(|hit| hit.field) })
}

fn asset_path(store: &Store, asset_id: &str) -> Option<PathBuf> {
    Uuid::parse_str(asset_id).ok()?;
    let path = std::path::absolute(store.root.join("assets").join(asset_id)).ok()?;
    path.is_file().then_some(path)
}

fn add_body(store: &Store, note: &Note, value: &mut Value, portable: bool) {
    let mut markdown = Vec::new();
    if portable && !note.title.trim().is_empty() {
        markdown.push(format!("# {}", note.title.replace(['\r', '\n'], " ")));
    }
    let mut attachments = Vec::new();
    for block in &note.blocks {
        let (kind, name, asset_id, transcript) = match block {
            Block::Markdown { text, .. } => {
                markdown.push(text.clone());
                continue;
            }
            Block::Image { name, asset, .. } => {
                ("image", name.as_str(), Some(asset.as_str()), None)
            }
            Block::File { name, asset, .. } => ("file", name.as_str(), Some(asset.as_str()), None),
            Block::Audio { asset, text, .. } => {
                ("audio", "录音.wav", Some(asset.as_str()), text.as_deref())
            }
            Block::RemoteImage { name, .. } => ("image", name.as_str(), None, None),
        };
        let local_path = asset_id.and_then(|asset| asset_path(store, asset));
        let label = name
            .replace(['\r', '\n'], " ")
            .replace('\\', "\\\\")
            .replace('[', "\\[")
            .replace(']', "\\]");
        let portable_path = if portable && local_path.is_some() {
            asset_id.map(|id| {
                let extension = Path::new(name)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .filter(|ext| {
                        !ext.is_empty()
                            && ext.len() <= 16
                            && ext.bytes().all(|b| b.is_ascii_alphanumeric())
                    });
                match extension {
                    Some(ext) => format!("assets/{id}.{}", ext.to_ascii_lowercase()),
                    None => format!("assets/{id}"),
                }
            })
        } else {
            None
        };
        let target = if portable {
            portable_path.clone()
        } else {
            local_path
                .as_deref()
                .and_then(|p| url::Url::from_file_path(p).ok())
                .map(|url| url.to_string())
        };
        if let Some(target) = target {
            let marker = if kind == "image" { "!" } else { "" };
            markdown.push(format!("{marker}[{label}](<{target}>)"));
        } else {
            markdown.push(format!("{label}（附件在本地不可用）"));
        }
        if let Some(text) = transcript {
            markdown.push(text.into());
        }
        let mut attachment = json!({"id": asset_id, "block_id": block.id(), "name": name, "kind": kind,
            "local_path": local_path, "available": local_path.is_some(), "transcript": transcript});
        if let Some(path) = portable_path {
            attachment["export_path"] = json!(path);
        }
        attachments.push(attachment);
    }
    value["markdown"] = json!(markdown.join("\n\n"));
    value["attachments"] = json!(attachments);
}

pub fn export_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    let mut input = std::fs::File::open(source)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result = std::io::copy(&mut input, &mut output).and_then(|_| output.sync_all());
    if result.is_err() {
        drop(output);
        let _ = std::fs::remove_file(destination);
    }
    result
}
