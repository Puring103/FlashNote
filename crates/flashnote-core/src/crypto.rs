use anyhow::{Result, bail};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use rand::RngCore;
use zeroize::Zeroizing;

pub struct VaultKey(pub Zeroizing<[u8; 32]>);
impl VaultKey {
    pub fn random() -> Self {
        let mut key = [0; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        Self(Zeroizing::new(key))
    }
    pub fn seal(&self, bytes: &[u8], context: &str) -> Result<Vec<u8>> {
        let mut nonce = [0; 24];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let cipher = XChaCha20Poly1305::new((&*self.0).into());
        let encrypted = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: bytes,
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| anyhow::anyhow!("加密失败"))?;
        let mut out = vec![1];
        out.extend_from_slice(&nonce);
        out.extend(encrypted);
        Ok(out)
    }
    pub fn open(&self, bytes: &[u8], context: &str) -> Result<Vec<u8>> {
        if bytes.len() < 41 || bytes[0] != 1 {
            bail!("无法识别加密格式");
        }
        XChaCha20Poly1305::new((&*self.0).into())
            .decrypt(
                XNonce::from_slice(&bytes[1..25]),
                Payload {
                    msg: &bytes[25..],
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| anyhow::anyhow!("密钥错误或数据已被修改"))
    }
}

#[cfg(windows)]
pub fn protect(bytes: &[u8], decrypt: bool) -> Result<Vec<u8>> {
    use windows::{
        Win32::{
            Foundation::{HLOCAL, LocalFree},
            Security::Cryptography::*,
        },
        core::PCWSTR,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        if decrypt {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )?;
        } else {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )?;
        }
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(Some(HLOCAL(output.pbData as _)));
        Ok(result)
    }
}

/// Non-Windows hosts install a platform key protector before opening a store.
#[cfg(not(windows))]
type Protector = fn(&[u8], bool) -> Result<Vec<u8>>;
#[cfg(not(windows))]
static PROTECTOR: std::sync::OnceLock<Protector> = std::sync::OnceLock::new();
#[cfg(not(windows))]
pub fn install_protector(protector: Protector) -> Result<()> {
    PROTECTOR
        .set(protector)
        .map_err(|_| anyhow::anyhow!("平台密钥保护器已初始化"))
}
#[cfg(not(windows))]
pub fn protect(bytes: &[u8], decrypt: bool) -> Result<Vec<u8>> {
    #[cfg(test)]
    if PROTECTOR.get().is_none() {
        static KEY: std::sync::OnceLock<VaultKey> = std::sync::OnceLock::new();
        let key = KEY.get_or_init(VaultKey::random);
        return if decrypt {
            key.open(bytes, "test-protector")
        } else {
            key.seal(bytes, "test-protector")
        };
    }
    PROTECTOR
        .get()
        .ok_or_else(|| anyhow::anyhow!("尚未初始化平台密钥保护器"))?(bytes, decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_randomized_encryption() {
        let key = VaultKey::random();
        let a = key.seal(b"private note", "note:1:v1").unwrap();
        assert_ne!(a, key.seal(b"private note", "note:1:v1").unwrap());
        assert_eq!(key.open(&a, "note:1:v1").unwrap(), b"private note");
        assert!(key.open(&a, "note:2:v1").is_err());
        let mut changed = a;
        changed[28] ^= 1;
        assert!(key.open(&changed, "note:1:v1").is_err());
        assert!(VaultKey::random().open(&changed, "note:1:v1").is_err());
    }
    #[test]
    fn local_protection_round_trip() {
        let bytes = protect(b"secret-key", false).unwrap();
        assert_eq!(protect(&bytes, true).unwrap(), b"secret-key");
    }
}
