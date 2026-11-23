//! 显式启用的远端账号登录验收；全程 HTTP 请求，不打开浏览器。
//! Uses its own local directories and a short-lived test note, never existing notes.
#[path = "support/test_store.rs"]
mod test_store;
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use suojian::{auth, model::Note, store::Store, sync};

fn sign_in(store: &mut Store, url: &str, secret: &str) {
    let username = std::env::var("SUOJIAN_SYNC_USERNAME").expect("set login account");
    let connection = sync::login(url, &username, secret, Arc::new(AtomicBool::new(false))).unwrap();
    sync::connect_account(store, connection).unwrap();
}

#[test]
#[ignore = "requires an explicit test account, a deployed service and SUOJIAN_SYNC_LOGIN_SECRET"]
fn deployed_login_and_two_device_encrypted_sync() {
    let root: PathBuf = std::env::var("SUOJIAN_LIVE_ROOT")
        .expect("set an isolated acceptance data directory")
        .into();
    let url = std::env::var("SUOJIAN_LIVE_URL").expect("set deployed HTTPS origin");
    let secret = zeroize::Zeroizing::new(
        std::env::var("SUOJIAN_SYNC_LOGIN_SECRET").expect("set login credential"),
    );
    std::fs::create_dir_all(&root).unwrap();
    let mut a = test_store::open_store(root.join("device-a")).unwrap();
    sign_in(&mut a, &url, &secret);
    let mut note = Note::new();
    note.set_markdown("真实账号登录与双设备同步验收".into());
    let nid = note.id.clone();
    a.save_note(note).unwrap();
    sync::once(&mut a).unwrap();
    let mut b = test_store::open_store(root.join("device-b")).unwrap();
    sign_in(&mut b, &url, &secret);
    for _ in 0..3 {
        sync::once(&mut b).unwrap();
        sync::once(&mut a).unwrap();
    }
    assert_eq!(*a.key.0, *b.key.0);
    let mut copied = b.notes.iter().find(|n| n.id == nid).unwrap().clone();
    assert_eq!(copied.text(), "真实账号登录与双设备同步验收");
    copied.set_markdown("第二台设备通过真实登录修改，第一台已收到".into());
    b.save_note(copied).unwrap();
    for _ in 0..3 {
        sync::once(&mut b).unwrap();
        sync::once(&mut a).unwrap();
    }
    assert_eq!(
        a.notes.iter().find(|n| n.id == nid).unwrap().text(),
        "第二台设备通过真实登录修改，第一台已收到"
    );
    // Exercise real refresh-token rotation, then persist it through the same
    // account binding path used by the client.
    let current = sync::config(&a).unwrap().unwrap();
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let session = auth::refresh(&client, &current.session).unwrap();
    let renewed = sync::account_with_session(&url, session).unwrap();
    sync::connect_account(&mut a, renewed).unwrap();
    let test_notes: Vec<_> = a
        .notes
        .iter()
        .filter(|n| {
            n.deleted_at.is_none()
                && (n.text() == "真实账号登录与双设备同步验收"
                    || n.text() == "第二台设备通过真实登录修改，第一台已收到")
        })
        .cloned()
        .collect();
    for mut cleanup in test_notes {
        cleanup.deleted_at = Some(suojian::model::now());
        a.save_note(cleanup).unwrap();
    }
    for _ in 0..3 {
        sync::once(&mut a).unwrap();
        sync::once(&mut b).unwrap();
    }
    assert!(
        b.notes
            .iter()
            .find(|n| n.id == nid)
            .unwrap()
            .deleted_at
            .is_some()
    );
    let logged_out = sync::logout(&b).unwrap().unwrap();
    sync::revoke_session(&logged_out).unwrap();
    assert!(auth::refresh(&client, &logged_out.session).is_err());
    std::fs::write(
        root.join("acceptance.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "url": url, "account": current.name, "issuer": current.issuer,
            "account_logins": 2, "two_device_sync": true, "key_secret": true,
            "refresh": true, "revocation": true, "test_note_removed": true,
            "usage": sync::usage(&a).unwrap()
        }))
        .unwrap(),
    )
    .unwrap();
}
