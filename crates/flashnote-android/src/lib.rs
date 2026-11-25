//! Small JSON boundary; no storage or sync algorithms are duplicated here.
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use suojian_core::{
    archive, media,
    model::*,
    store::{ConflictChoice, Store},
    sync,
};

fn transfer_path(store: &Store, request: &Value) -> Result<PathBuf> {
    let token = field(request, "token")?;
    ensure!(
        token.len() == 36 && token.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "文件标识无效"
    );
    Ok(store.root.join("imports").join(token))
}

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("缺少参数: {key}"))
}
fn note(store: &Store, v: &Value) -> Result<Note> {
    let id = field(v, "id")?;
    let n = store
        .notes
        .iter()
        .find(|n| n.id == id)
        .ok_or_else(|| anyhow::anyhow!("便签不存在"))?;
    ensure!(
        n.revision == field(v, "revision")?,
        "便签已更新，请重新打开；当前草稿保留"
    );
    Ok(n.clone())
}
pub fn dispatch(root: PathBuf, request: &str) -> Result<Value> {
    let mut s = suojian_core::profiles::open(root)?;
    dispatch_request(&mut s, request)
}

fn dispatch_request(s: &mut Store, request: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(request)?;
    let operation = field(&v, "op")?;
    let replace_notes = matches!(
        operation,
        "list" | "sync" | "import_zip" | "login" | "logout" | "import_local"
    );
    let mut changed = Vec::new();
    let mut saved_id: Option<String> = None;
    let mut sync_error = None;
    let mut details = Value::Null;
    match operation {
        "list" => {}
        "save" => {
            let mut n = if v["id"].as_str().unwrap_or("").is_empty() {
                Note::new()
            } else {
                note(s, &v)?
            };
            n.title = field(&v, "title")?.into();
            n.group_id = field(&v, "group")?.into();
            n.set_editor_markdown(field(&v, "source")?);
            saved_id = Some(n.id.clone());
            changed.push(n.id.clone());
            s.save_note(n)?;
        }
        "delete" | "restore" => {
            let mut n = note(s, &v)?;
            changed.push(n.id.clone());
            n.deleted_at = if v["op"] == "delete" {
                Some(now())
            } else {
                None
            };
            s.save_note(n)?;
        }
        "attach" => {
            let path = transfer_path(s, &v)?;
            let result = (|| -> Result<()> {
                let n = note(s, &v)?;
                let mut bytes = Vec::new();
                std::fs::File::open(&path)?
                    .take(suojian_core::media::MAX_FILE + 1)
                    .read_to_end(&mut bytes)?;
                let prepared = media::prepare_bytes(field(&v, "name")?.into(), bytes, false)?;
                s.attach_prepared(&n.id, prepared)?;
                changed.push(n.id);
                Ok(())
            })();
            let _ = std::fs::remove_file(path);
            result?;
        }
        "recording" => {
            let mut n = if field(&v, "id")?.is_empty() {
                Note::new()
            } else {
                note(s, &v)?
            };
            ensure!(n.deleted_at.is_none(), "便签已移入回收站");
            n.title = field(&v, "title")?.into();
            n.group_id = field(&v, "group")?.into();
            n.set_editor_markdown(field(&v, "source")?);
            let mut bytes = Vec::new();
            std::fs::File::open(transfer_path(s, &v)?)?
                .take(media::MAX_FILE + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() > 44
                    && bytes.len() as u64 <= media::MAX_FILE
                    && bytes.starts_with(b"RIFF")
                    && &bytes[8..12] == b"WAVE",
                "录音文件无效"
            );
            let asset = s.import_bytes(&bytes)?;
            n.blocks.push(Block::Audio {
                id: id(),
                asset: asset.clone(),
                duration_ms: ((bytes.len() - 44) as u64 * 1000) / 32000,
                text: None,
                state: AudioState::Recorded,
            });
            saved_id = Some(n.id.clone());
            changed.push(n.id.clone());
            if let Err(error) = s.save_note(n) {
                let _ = std::fs::remove_file(s.root.join("assets").join(asset));
                return Err(error);
            }
        }
        "transcript" => {
            let nid = field(&v, "id")?;
            let bid = field(&v, "block")?;
            let transcript = field(&v, "text")?.trim();
            ensure!(!transcript.is_empty(), "未识别到文字");
            let mut n = s
                .notes
                .iter()
                .find(|n| n.id == nid && n.deleted_at.is_none())
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("便签不存在"))?;
            let Some(Block::Audio { text, state, .. }) =
                n.blocks.iter_mut().find(|b| b.id() == bid)
            else {
                bail!("录音不存在")
            };
            ensure!(
                text.is_none() && matches!(state, AudioState::Recorded | AudioState::Failed(_)),
                "录音已有转写结果或正在转写"
            );
            *text = Some(transcript.to_owned());
            *state = AudioState::Ready;
            changed.push(n.id.clone());
            s.save_note(n)?;
        }
        "history" => {
            details = json!(
                s.history(field(&v, "id")?)?
                    .iter()
                    .map(|n| json!({
                        "revision":n.revision, "title":n.title, "source":n.editor_markdown(),
                        "updated":n.updated_at,
                    }))
                    .collect::<Vec<_>>()
            );
        }
        "restore_history" => {
            let current = note(s, &v)?;
            let revision = field(&v, "restore_revision")?;
            let mut old = s
                .history(&current.id)?
                .into_iter()
                .find(|n| n.revision == revision)
                .ok_or_else(|| anyhow::anyhow!("历史版本不存在"))?;
            old.revision = current.revision;
            old.deleted_at = current.deleted_at;
            old.conflict_of = current.conflict_of;
            old.conflict_resolved = current.conflict_resolved;
            if !s.groups.iter().any(|g| g.id == old.group_id && !g.deleted) {
                old.group_id = DEFAULT_GROUP.into();
            }
            saved_id = Some(old.id.clone());
            changed.push(old.id.clone());
            s.save_note(old)?;
        }
        "export_zip" => {
            let ids = s.notes.iter().map(|n| n.id.clone()).collect::<Vec<_>>();
            archive::export(
                s.root.clone(),
                archive::manifest(s, &ids),
                &transfer_path(s, &v)?,
            )?;
        }
        "import_zip" => {
            let import = archive::read(&transfer_path(s, &v)?)?;
            details = json!(s.import_archive(&import, true)?);
        }
        "asset" => {
            let path = s.materialize_attachment(field(&v, "asset")?, field(&v, "name")?)?;
            return Ok(json!({"file":path}));
        }
        "group" => {
            let color = v["color"].as_u64().unwrap_or(GROUP_COLORS[0] as u64);
            ensure!(color <= 0xffffff, "分组颜色无效");
            s.create_group_with_color(field(&v, "name")?, color as u32)?;
        }
        "login" => {
            let connection = sync::login_device(
                &suojian_core::profiles::credentials(s)?,
                v["words"].as_str().unwrap_or(""),
                v["allow_managed"].as_bool().unwrap_or(false),
                field(&v, "url")?,
                field(&v, "username")?,
                field(&v, "password")?,
                Arc::new(AtomicBool::new(false)),
            )?;
            let show_words = connection.config.e2ee;
            suojian_core::profiles::connect(s, connection)?;
            if show_words {
                details = json!(sync::recovery_words(s)?.as_str());
            }
        }
        "recovery_words" => {
            details = json!(sync::recovery_words(s)?.as_str());
        }
        "import_local" => suojian_core::profiles::import_local(s)?,
        "logout" => {
            if let Some(config) = suojian_core::profiles::logout(s)? {
                let _ = sync::revoke_session(&config);
            }
        }
        "sync" => {
            // Earlier pages may have committed even if a later page fails.
            // Return the current snapshot with the error so the UI catches up.
            sync_error = sync::once(s).err().map(|error| format!("{error:#}"));
        }
        "resolve" => {
            let id = field(&v, "id")?;
            let choice = match field(&v, "choice")? {
                "local" => ConflictChoice::Local,
                "cloud" => ConflictChoice::Cloud,
                "both" => ConflictChoice::Both,
                _ => bail!("未知冲突处理方式"),
            };
            if let Some((fork, cloud)) = s.conflict_pair(id) {
                changed.extend([fork.id, cloud.id]);
            }
            s.resolve_conflict(
                id,
                field(&v, "revision")?,
                field(&v, "cloud_revision")?,
                choice,
            )?;
        }
        _ => bail!("未知操作"),
    }
    let notes: Vec<Value> = s
        .notes
        .iter()
        .filter(|n| replace_notes || changed.contains(&n.id))
        .map(|n| json!({"note":n, "source":n.editor_markdown()}))
        .collect();
    let config = sync::config(s)?;
    Ok(
        json!({"storage_root":s.root, "has_local":config.is_some() && suojian_core::profiles::has_local(s)?, "e2ee": config.as_ref().is_some_and(|c| c.e2ee), "notes":notes,"replace_notes":replace_notes,"groups":s.groups,"account":config.map(|c| c.name),"saved_id":saved_id,"sync_error":sync_error,"details":details}),
    )
}

#[cfg(target_os = "android")]
mod android {
    use super::*;
    use jni::{
        JNIEnv, JavaVM,
        objects::{GlobalRef, JClass, JObject, JString, JValue},
        sys::jstring,
    };
    use std::sync::OnceLock;
    static HOST: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();
    // Kotlin serializes requests on its dedicated executor. Reuse the
    // connection on that thread and release it when the executor exits.
    thread_local! {
        static STORE: std::cell::RefCell<Option<Store>> = const { std::cell::RefCell::new(None) };
    }
    fn protect(bytes: &[u8], decrypt: bool) -> Result<Vec<u8>> {
        let (vm, class) = HOST
            .get()
            .ok_or_else(|| anyhow::anyhow!("Android host unavailable"))?;
        let mut env = vm.attach_current_thread()?;
        let input = JObject::from(env.byte_array_from_slice(bytes)?);
        let output = env.call_static_method(
            <&JClass>::from(class.as_obj()),
            "protect",
            "([BZ)[B",
            &[JValue::Object(&input), JValue::Bool(decrypt as u8)],
        );
        if env.exception_check()? {
            env.exception_clear()?;
            bail!("Android Keystore 密钥保护失败");
        }
        Ok(env.convert_byte_array(jni::objects::JByteArray::from(output?.l()?))?)
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_noteharbor_flashnote_Native_request(
        mut env: JNIEnv,
        class: JClass,
        root: JString,
        request: JString,
    ) -> jstring {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value> {
            if HOST.get().is_none() {
                HOST.set((env.get_java_vm()?, env.new_global_ref(class)?))
                    .map_err(|_| anyhow::anyhow!("初始化冲突"))?;
                suojian_core::crypto::install_protector(protect)?;
            }
            let root: String = env.get_string(&root)?.into();
            let request: String = env.get_string(&request)?.into();
            STORE.with(|cached| {
                let mut cached = cached.borrow_mut();
                let root = PathBuf::from(root);
                if cached.is_none() {
                    *cached = None;
                    *cached = Some(suojian_core::profiles::open(root)?);
                }
                dispatch_request(cached.as_mut().unwrap(), &request)
            })
        }));
        let response = match result {
            Ok(Ok(data)) => json!({"ok":true,"data":data}),
            Ok(Err(e)) => json!({"ok":false,"error":format!("{e:#}")}),
            Err(_) => json!({"ok":false,"error":"核心发生异常，请重新打开应用"}),
        };
        env.new_string(response.to_string())
            .map(|s| s.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn mobile_group_colors_survive_reload_and_reject_invalid_values() {
        let dir = tempfile::tempdir().unwrap();
        dispatch(
            dir.path().into(),
            &json!({"op":"group", "name":"生活", "color":0xc39b45}).to_string(),
        )
        .unwrap();
        let data = dispatch(dir.path().into(), r#"{"op":"list"}"#).unwrap();
        assert!(
            data["groups"]
                .as_array()
                .unwrap()
                .iter()
                .any(|g| g["name"] == "生活" && g["color"] == 0xc39b45)
        );
        assert!(
            dispatch(
                dir.path().into(),
                &json!({"op":"group", "name":"无效", "color":0x1000000}).to_string()
            )
            .is_err()
        );
    }
    #[test]
    fn mobile_mutation_only_returns_changed_notes() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().into()).unwrap();
        for title in ["第一张", "第二张"] {
            let result = dispatch_request(
                &mut store,
                &json!({"op":"save","id":"","title":title,"source":"","group":DEFAULT_GROUP})
                    .to_string(),
            )
            .unwrap();
            let notes = result["notes"].as_array().unwrap();
            assert_eq!(notes.len(), 1);
            assert_eq!(notes[0]["note"]["title"], title);
            assert_eq!(result["replace_notes"], false);
        }
        let result = dispatch_request(&mut store, r#"{"op":"list"}"#).unwrap();
        assert_eq!(result["notes"].as_array().unwrap().len(), 2);
        assert_eq!(result["replace_notes"], true);
    }

    #[test]
    fn mobile_roundtrip_rejects_stale_edits_and_restores_trash() {
        let dir = tempfile::tempdir().unwrap();
        let call = |v: Value| dispatch(dir.path().into(), &v.to_string());
        let data = call(json!({"op":"save","id":"","title":"手机便签","source":"- [ ] 同步测试","group":DEFAULT_GROUP})).unwrap();
        let n = &data["notes"][0]["note"];
        let id = n["id"].as_str().unwrap();
        let rev = n["revision"].as_str().unwrap();
        let deleted = call(json!({"op":"delete","id":id,"revision":rev})).unwrap();
        assert!(!deleted["notes"][0]["note"]["deleted_at"].is_null());
        assert!(call(json!({"op":"save","id":id,"revision":rev,"title":"过期编辑","source":"不得覆盖","group":DEFAULT_GROUP})).is_err());
        let restored = call(
            json!({"op":"restore","id":id,"revision":deleted["notes"][0]["note"]["revision"]}),
        )
        .unwrap();
        assert!(restored["notes"][0]["note"]["deleted_at"].is_null());
        assert_eq!(restored["notes"][0]["note"]["title"], "手机便签");
        assert!(restored.get("session").is_none());
    }

    #[test]
    fn mobile_records_transcribes_restores_and_roundtrips_zip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().into()).unwrap();
        let imports = dir.path().join("imports");
        std::fs::create_dir(&imports).unwrap();
        let recording = id();
        let mut wav = b"RIFF\x64\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data\x40\0\0\0".to_vec();
        wav.extend([0; 64]);
        std::fs::write(imports.join(&recording), &wav).unwrap();
        let recorded = dispatch_request(&mut store, &json!({
            "op":"recording", "id":"", "title":"语音", "source":"第一版", "group":DEFAULT_GROUP, "token":recording,
        }).to_string()).unwrap();
        let audio = &recorded["notes"][0]["note"]["blocks"][1];
        let nid = recorded["saved_id"].as_str().unwrap();
        let first_revision = &recorded["notes"][0]["note"]["revision"];
        assert_eq!(audio["kind"], "audio");
        assert_eq!(audio["duration_ms"], 2);
        let transcript = dispatch_request(
            &mut store,
            &json!({
                "op":"transcript", "id":nid, "block":audio["id"], "text":"转写结果",
            })
            .to_string(),
        )
        .unwrap();
        let recent = &transcript["notes"][0]["note"];
        assert_eq!(recent["blocks"][1]["text"], "转写结果");
        let history =
            dispatch_request(&mut store, &json!({"op":"history", "id":nid}).to_string()).unwrap();
        assert_eq!(&history["details"][1]["revision"], first_revision);
        let restore = |revision: &Value| {
            json!({
            "op":"restore_history", "id":nid, "revision":revision, "restore_revision": first_revision,
        }).to_string()
        };
        assert!(dispatch_request(&mut store, &restore(first_revision)).is_err());
        let restored = dispatch_request(&mut store, &restore(&recent["revision"])).unwrap();
        assert!(!restored["notes"][0]["note"]["blocks"][1]["text"].is_string());
        assert_ne!(&restored["notes"][0]["note"]["revision"], first_revision);
        let zip = id();
        dispatch_request(
            &mut store,
            &json!({"op":"export_zip", "token":zip}).to_string(),
        )
        .unwrap();
        let imported = dispatch_request(
            &mut store,
            &json!({"op":"import_zip", "token":zip}).to_string(),
        )
        .unwrap();
        assert_eq!(imported["details"], 1);
        assert_eq!(imported["notes"].as_array().unwrap().len(), 2);
        let copy = store.notes.iter().find(|note| note.id != nid).unwrap();
        let Block::Audio { asset, .. } = &copy.blocks[1] else {
            panic!("missing audio block")
        };
        assert_eq!(store.asset(asset).unwrap(), wav);
    }
}
