//! Secret storage is deliberately separate from serializable preferences and task records.
//!
//! Tests inject `MemoryCredentialVault`; they never access the user's Keychain.

use anyhow::{Result, anyhow, bail};
#[cfg(any(test, not(target_os = "macos")))]
use std::collections::BTreeMap;
use std::fmt;
#[cfg(not(target_os = "macos"))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(any(test, not(target_os = "macos")))]
use std::sync::Mutex;
use zeroize::Zeroizing;

pub type CredentialRef = String;

/// A secret cannot accidentally be serialized or printed by a derived Debug implementation.
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    pub fn expose(&self) -> &str {
        self.0.as_str()
    }

    pub fn is_empty(&self) -> bool {
        self.expose().trim().is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

/// Insert always creates a new immutable identity. Updating a service cannot change the
/// credential used by an already queued task. Resolving never consults mutable environment
/// variables; a user-selected environment credential must be captured at publication time.
pub trait CredentialVault: Send + Sync {
    fn insert(&self, secret: Secret) -> Result<CredentialRef>;
    fn resolve(&self, reference: &str) -> Result<Secret>;
    fn remove(&self, reference: &str) -> Result<()>;
}

fn new_reference() -> String {
    format!("credential-{}", uuid::Uuid::new_v4())
}

fn validate_reference(reference: &str) -> Result<()> {
    let id = reference.strip_prefix("credential-").unwrap_or("");
    if uuid::Uuid::parse_str(id).is_err() {
        bail!(course2md::tr!("凭据引用无效，请为此服务重新保存 API Key"));
    }
    Ok(())
}

/// In-memory isolated store for tests and caller-controlled temporary use.
/// It intentionally does not provide a plaintext file fallback.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryCredentialVault {
    values: Mutex<BTreeMap<CredentialRef, Zeroizing<String>>>,
}

#[cfg(test)]
impl MemoryCredentialVault {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
impl CredentialVault for MemoryCredentialVault {
    fn insert(&self, secret: Secret) -> Result<CredentialRef> {
        if secret.is_empty() {
            bail!("请输入 API Key");
        }
        let reference = new_reference();
        self.values
            .lock()
            .map_err(|_| anyhow!("暂时无法保存凭据"))?
            .insert(
                reference.clone(),
                Zeroizing::new(secret.expose().to_owned()),
            );
        Ok(reference)
    }

    fn resolve(&self, reference: &str) -> Result<Secret> {
        validate_reference(reference)?;
        self.values
            .lock()
            .map_err(|_| anyhow!("暂时无法读取凭据"))?
            .get(reference)
            .map(|value| Secret::new(value.as_str()))
            .ok_or_else(|| anyhow!("找不到此服务的凭据，请重新保存 API Key"))
    }

    fn remove(&self, reference: &str) -> Result<()> {
        validate_reference(reference)?;
        self.values
            .lock()
            .map_err(|_| anyhow!("暂时无法移除凭据"))?
            .remove(reference);
        Ok(())
    }
}

/// errSecItemNotFound: the Keychain has no item for this reference.
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

#[cfg(target_os = "macos")]
pub struct KeychainCredentialVault {
    service: String,
}

#[cfg(target_os = "macos")]
impl Default for KeychainCredentialVault {
    fn default() -> Self {
        Self {
            service: "com.course2md.desktop.service-credentials.v1".into(),
        }
    }
}

#[cfg(target_os = "macos")]
impl CredentialVault for KeychainCredentialVault {
    fn insert(&self, secret: Secret) -> Result<CredentialRef> {
        if secret.is_empty() {
            bail!(course2md::tr!("请输入 API Key"));
        }
        let reference = new_reference();
        security_framework::passwords::set_generic_password(
            &self.service,
            &reference,
            secret.expose().as_bytes(),
        )
        .map_err(|error| {
            anyhow!(
                "{}",
                course2md::tr!("无法将 API Key 保存到钥匙串（系统代码 {}）", error.code())
            )
        })?;
        Ok(reference)
    }

    fn resolve(&self, reference: &str) -> Result<Secret> {
        validate_reference(reference)?;
        let bytes = security_framework::passwords::get_generic_password(&self.service, reference)
            .map_err(|error| match error.code() {
            ERR_SEC_ITEM_NOT_FOUND => anyhow!(course2md::tr!(
                "钥匙串中找不到此服务的凭据，请重新保存 API Key"
            )),
            code => anyhow!(
                "{}",
                course2md::tr!("暂时无法读取此服务的钥匙串凭据（系统代码 {}）", code)
            ),
        })?;
        // Keep the intermediate byte buffer zeroizing as well as the returned UTF-8 value.
        let bytes = Zeroizing::new(bytes);
        let value = std::str::from_utf8(&bytes)
            .map_err(|_| anyhow!(course2md::tr!("此服务的凭据无法读取，请重新保存 API Key")))?;
        Ok(Secret::new(value))
    }

    fn remove(&self, reference: &str) -> Result<()> {
        validate_reference(reference)?;
        match security_framework::passwords::delete_generic_password(&self.service, reference) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(error) => bail!(
                "{}",
                course2md::tr!("无法移除此服务的钥匙串凭据（系统代码 {}）", error.code())
            ),
        }
    }
}

/// System keyring-backed credential store for Linux (Secret Service) and
/// Windows (Credential Manager). Availability is checked once through the
/// keyring crate's store status; when no system keyring is reachable the
/// caller falls back to `FileCredentialVault`.
#[cfg(not(target_os = "macos"))]
pub struct KeyringCredentialVault {
    service: String,
}

#[cfg(not(target_os = "macos"))]
impl KeyringCredentialVault {
    pub fn new() -> Self {
        Self {
            service: "com.course2md.desktop.service-credentials.v1".into(),
        }
    }

    /// Whether the platform credential store initialized successfully.
    pub fn available() -> bool {
        keyring::Entry::store_status().is_ok()
    }
}

#[cfg(not(target_os = "macos"))]
impl CredentialVault for KeyringCredentialVault {
    fn insert(&self, secret: Secret) -> Result<CredentialRef> {
        if secret.is_empty() {
            bail!(course2md::tr!("请输入 API Key"));
        }
        let reference = new_reference();
        keyring::Entry::new(&self.service, &reference)
            .and_then(|entry| entry.set_password(secret.expose()))
            .map_err(|_| anyhow!(course2md::tr!("无法将 API Key 保存到系统凭据库")))?;
        Ok(reference)
    }

    fn resolve(&self, reference: &str) -> Result<Secret> {
        validate_reference(reference)?;
        let password = keyring::Entry::new(&self.service, reference)
            .and_then(|entry| entry.get_password())
            .map_err(|error| match error {
                keyring::Error::NoEntry => {
                    anyhow!(course2md::tr!(
                        "系统凭据库中找不到此服务的凭据，请重新保存 API Key"
                    ))
                }
                _ => anyhow!(course2md::tr!("暂时无法读取系统凭据库中此服务的凭据")),
            })?;
        Ok(Secret::new(password))
    }

    fn remove(&self, reference: &str) -> Result<()> {
        validate_reference(reference)?;
        // 与 Keychain 实现一致：条目不存在视为已删除
        match keyring::Entry::new(&self.service, reference)
            .and_then(|entry| entry.delete_credential())
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => bail!(course2md::tr!("无法移除系统凭据库中此服务的凭据")),
        }
    }
}

/// File-backed fallback for platforms without a reachable system keychain.
///
/// Credentials are written as JSON next to the desktop preferences. The file is
/// created through `atomic_write`, which uses a 0o600 temporary file on Unix, so
/// the persisted secrets are not world-readable. This mirrors the CLI, which
/// stores API keys in the user's `config.toml` under the same permissions.
#[cfg(not(target_os = "macos"))]
pub struct FileCredentialVault {
    path: PathBuf,
    values: Mutex<BTreeMap<CredentialRef, Zeroizing<String>>>,
    /// Set when the file exists but cannot be read; writes are refused so an
    /// empty map never replaces the saved keys.
    load_error: Option<&'static str>,
}

#[cfg(not(target_os = "macos"))]
impl FileCredentialVault {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (values, load_error) = match Self::load(&path) {
            Ok(values) => (values, None),
            Err(error) => (BTreeMap::new(), Some(error)),
        };
        Self {
            path,
            values: Mutex::new(values),
            load_error,
        }
    }

    fn refuse_if_unreadable(&self) -> Result<()> {
        if let Some(error) = self.load_error {
            bail!(course2md::tr!(
                "{}。请备份后删除该文件再重试：{}",
                error,
                self.path.display()
            ));
        }
        Ok(())
    }

    fn load(
        path: &Path,
    ) -> std::result::Result<BTreeMap<CredentialRef, Zeroizing<String>>, &'static str> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(_) => {
                return Err(course2md::tr!(
                    "已保存的 API Key 文件无法读取，为避免丢失暂不写入"
                ));
            }
        };
        let plain: BTreeMap<CredentialRef, String> = serde_json::from_slice(&bytes)
            .map_err(|_| course2md::tr!("已保存的 API Key 文件已损坏，为避免丢失暂不写入"))?;
        Ok(plain
            .into_iter()
            .map(|(reference, value)| (reference, Zeroizing::new(value)))
            .collect())
    }

    fn persist(&self, values: &BTreeMap<CredentialRef, Zeroizing<String>>) -> Result<()> {
        let plain: BTreeMap<CredentialRef, String> = values
            .iter()
            .map(|(reference, value)| (reference.clone(), value.as_str().to_owned()))
            .collect();
        let bytes = serde_json::to_vec_pretty(&plain)?;
        course2md::checkpoint::atomic_write(&self.path, &bytes)
    }
}

#[cfg(not(target_os = "macos"))]
impl CredentialVault for FileCredentialVault {
    fn insert(&self, secret: Secret) -> Result<CredentialRef> {
        if secret.is_empty() {
            bail!(course2md::tr!("请输入 API Key"));
        }
        self.refuse_if_unreadable()?;
        let reference = new_reference();
        let mut values = self
            .values
            .lock()
            .map_err(|_| anyhow!(course2md::tr!("暂时无法保存凭据")))?;
        values.insert(
            reference.clone(),
            Zeroizing::new(secret.expose().to_owned()),
        );
        if let Err(error) = self.persist(&values) {
            values.remove(&reference);
            return Err(error);
        }
        Ok(reference)
    }

    fn resolve(&self, reference: &str) -> Result<Secret> {
        validate_reference(reference)?;
        self.values
            .lock()
            .map_err(|_| anyhow!(course2md::tr!("暂时无法读取凭据")))?
            .get(reference)
            .map(|value| Secret::new(value.as_str()))
            .ok_or_else(|| anyhow!(course2md::tr!("找不到此服务的凭据，请重新保存 API Key")))
    }

    fn remove(&self, reference: &str) -> Result<()> {
        validate_reference(reference)?;
        self.refuse_if_unreadable()?;
        let mut values = self
            .values
            .lock()
            .map_err(|_| anyhow!(course2md::tr!("暂时无法移除凭据")))?;
        let removed = values.remove(reference);
        if let Err(error) = self.persist(&values) {
            if let Some(value) = removed {
                values.insert(reference.to_owned(), value);
            }
            return Err(error);
        }
        Ok(())
    }
}

pub fn system_vault(path: impl Into<PathBuf>) -> Arc<dyn CredentialVault> {
    #[cfg(target_os = "macos")]
    {
        let _ = path;
        Arc::new(KeychainCredentialVault::default())
    }
    #[cfg(not(target_os = "macos"))]
    {
        if KeyringCredentialVault::available() {
            Arc::new(KeyringCredentialVault::new())
        } else {
            Arc::new(FileCredentialVault::new(path))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_errors_cannot_expose_secret() {
        let secret = Secret::new("test-only-do-not-print");
        assert_eq!(format!("{secret:?}"), "Secret([REDACTED])");
        let vault = MemoryCredentialVault::new();
        let error = vault.resolve("test-only-do-not-print").unwrap_err();
        assert!(!error.to_string().contains("test-only-do-not-print"));
        assert!(vault.insert(Secret::new("   ")).is_err());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn file_vault_persists_credentials_across_instances() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("desktop-credentials.json");
        let reference = {
            let vault = FileCredentialVault::new(&path);
            let reference = vault.insert(Secret::new("test-only-file-key")).unwrap();
            assert_eq!(
                vault.resolve(&reference).unwrap().expose(),
                "test-only-file-key"
            );
            reference
        };
        // A fresh instance reads the same credential back from disk.
        let reopened = FileCredentialVault::new(&path);
        assert_eq!(
            reopened.resolve(&reference).unwrap().expose(),
            "test-only-file-key"
        );
        reopened.remove(&reference).unwrap();
        assert!(reopened.resolve(&reference).is_err());
        // Removal is also persisted.
        let after_remove = FileCredentialVault::new(&path);
        assert!(after_remove.resolve(&reference).is_err());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn file_vault_resolves_nothing_from_a_missing_or_corrupt_file() {
        let directory = tempfile::tempdir().unwrap();
        let missing = FileCredentialVault::new(directory.path().join("missing.json"));
        assert!(
            missing
                .resolve("credential-00000000-0000-0000-0000-000000000000")
                .is_err()
        );

        let corrupt = directory.path().join("corrupt.json");
        std::fs::write(&corrupt, b"not json").unwrap();
        let vault = FileCredentialVault::new(&corrupt);
        assert!(
            vault
                .resolve("credential-00000000-0000-0000-0000-000000000000")
                .is_err()
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn corrupt_file_is_never_overwritten_by_an_empty_map() {
        let directory = tempfile::tempdir().unwrap();
        let corrupt = directory.path().join("corrupt.json");
        std::fs::write(&corrupt, b"not json").unwrap();
        let vault = FileCredentialVault::new(&corrupt);
        // 损坏文件存在期间：写入与删除都必须拒绝，不得静默覆盖丢密钥
        assert!(vault.insert(Secret::new("test-only-key")).is_err());
        assert!(
            vault
                .remove("credential-00000000-0000-0000-0000-000000000000")
                .is_err()
        );
        assert_eq!(std::fs::read(&corrupt).unwrap(), b"not json");
    }
}
