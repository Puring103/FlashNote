//! Small JSON boundary; no storage or sync algorithms are duplicated here.
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use suojian_core::{
    model::*,
    store::{ConflictChoice, Store},
    sync,
};

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
    let mut s = Store::open(root)?;
    dispatch_request(&mut s, request)
}

fn dispatch_request(s: &mut Store, request: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(request)?;
    let operation = field(&v, "op")?;
    let replace_notes = matches!(operation, "list" | "sync");
    let mut changed = Vec::new();
    let mut saved_id: Option<String> = None;
    let mut sync_error = None;
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
            let token = field(&v, "token")?;
            ensure!(
                token.len() == 36 && token.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
                "附件标识无效"
            );
            let path = s.root.join("imports").join(token);
            let result = (|| -> Result<()> {
                let n = note(s, &v)?;
                use std::io::Read;
                let mut bytes = Vec::new();
                std::fs::File::open(&path)?
                    .take(suojian_core::media::MAX_FILE + 1)
                    .read_to_end(&mut bytes)?;
                let prepared =
                    suojian_core::media::prepare_bytes(field(&v, "name")?.into(), bytes, false)?;
                s.attach_prepared(&n.id, prepared)?;
                changed.push(n.id);
                Ok(())
            })();
            let _ = std::fs::remove_file(path);
            result?;
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
            let connection = sync::login(
                field(&v, "url")?,
                field(&v, "username")?,
                field(&v, "password")?,
                Arc::new(AtomicBool::new(false)),
            )?;
            sync::connect_account(s, connection)?;
        }
        "logout" => {
            if let Some(config) = sync::logout(s)? {
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
    Ok(
        json!({"notes":notes,"replace_notes":replace_notes,"groups":s.groups,"account":sync::config(s)?.map(|c| c.name),"saved_id":saved_id,"sync_error":sync_error}),
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
                if cached.as_ref().is_none_or(|store| store.root != root) {
                    *cached = None;
                    *cached = Some(Store::open(root)?);
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
}
