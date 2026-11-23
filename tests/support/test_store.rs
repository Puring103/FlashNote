use std::path::PathBuf;
use suojian::store::Store;

pub fn open_store(root: PathBuf) -> anyhow::Result<Store> {
    #[cfg(target_os = "linux")]
    {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            suojian::linux_keyring::install().expect("install Linux test key protector");
        });
    }
    Store::open(root)
}
