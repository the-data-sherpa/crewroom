//! Install-time credential store.
//!
//! `crewroom.secret` holds one random 256-bit key per installation. Provider
//! tokens are written to `credentials.json` only as
//! `$crewroom$v1$<base64url(nonce || ciphertext || tag)>` using
//! XChaCha20-Poly1305 with the provider name as associated data. Plaintext is
//! held only in this process's memory while a request is authenticated.
//!
//! Threat model: this protects tokens in copied configuration, backups, sync
//! folders and accidentally shared files that do not also carry
//! `crewroom.secret`. It does not protect against processes running as the same
//! OS user, which can read both files.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use crewroom_core::{Error, Result};
use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
};
use zeroize::Zeroizing;

pub const SECRET_FILE: &str = "crewroom.secret";
pub const CREDENTIALS_FILE: &str = "credentials.json";
const PREFIX: &str = "$crewroom$v1$";
const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 24;
const MAX_TOKEN: usize = 8192;
/// What an integration shows when its stored value cannot be decrypted.
pub const UNREADABLE: &str = "Credentials unreadable — reconnect";
/// Serializes writers in this process; readers rely on atomic replacement.
static WRITES: Mutex<()> = Mutex::new(());

/// Integrations whose tokens the store accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Integration {
    Jira,
    Bitbucket,
}
impl Integration {
    pub const ALL: [Self; 2] = [Self::Jira, Self::Bitbucket];
    pub fn id(self) -> &'static str {
        match self {
            Self::Jira => "jira",
            Self::Bitbucket => "bitbucket",
        }
    }
    pub fn environment(self) -> &'static str {
        match self {
            Self::Jira => "CREWROOM_JIRA_TOKEN",
            Self::Bitbucket => "CREWROOM_BITBUCKET_TOKEN",
        }
    }
    pub fn parse(value: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|i| i.id() == value)
            .ok_or_else(|| Error::NotFound("Unknown credential integration.".into()))
    }
}

/// A stored token, resolved without exposing why decryption failed.
#[derive(Clone, PartialEq, Eq)]
pub enum StoredToken {
    Missing,
    Present(Zeroizing<String>),
    Unreadable,
}
impl std::fmt::Debug for StoredToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Missing => "Missing",
            Self::Present(_) => "Present(<redacted>)",
            Self::Unreadable => "Unreadable",
        })
    }
}

enum KeyState {
    Missing,
    Invalid,
    Present(Zeroizing<[u8; KEY_BYTES]>),
}

/// File-backed store rooted at Crewroom's private data directory.
#[derive(Clone, Debug)]
pub struct SecretStore {
    root: PathBuf,
}
impl SecretStore {
    /// Opens the store and creates the install key on first run. The key is never
    /// generated while encrypted values exist, so a missing or replaced key leaves
    /// those values unreadable instead of silently orphaning them.
    pub fn open(root: &Path) -> Result<Self> {
        let store = Self::at(root);
        let _guard = WRITES.lock().map_err(|_| poisoned())?;
        if matches!(store.key(), KeyState::Missing) && !store.has_encrypted_values()? {
            store.create_key()?;
        }
        Ok(store)
    }
    /// A read-only handle that never creates the key.
    pub fn at(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
        }
    }
    pub fn secret_path(&self) -> PathBuf {
        self.root.join(SECRET_FILE)
    }
    pub fn credentials_path(&self) -> PathBuf {
        self.root.join(CREDENTIALS_FILE)
    }
    fn key(&self) -> KeyState {
        let path = self.secret_path();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return KeyState::Missing,
            Err(_) => return KeyState::Invalid,
        };
        // SAFETY: geteuid has no preconditions.
        if !metadata.is_file()
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.len() != KEY_BYTES as u64
        {
            return KeyState::Invalid;
        }
        match std::fs::read(&path) {
            Ok(bytes) if bytes.len() == KEY_BYTES => {
                let bytes = Zeroizing::new(bytes);
                let mut key = Zeroizing::new([0u8; KEY_BYTES]);
                key.copy_from_slice(&bytes);
                KeyState::Present(key)
            }
            _ => KeyState::Invalid,
        }
    }
    fn create_key(&self) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let mut key = Zeroizing::new([0u8; KEY_BYTES]);
        getrandom::getrandom(key.as_mut_slice())
            .map_err(|_| Error::Conflict("The system random source is unavailable.".into()))?;
        let path = self.secret_path();
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => file,
            // Another process created it first; never replace an existing key.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let written = file.write_all(key.as_slice()).and_then(|_| file.sync_all());
        if let Err(error) = written {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(error.into());
        }
        std::fs::File::open(&self.root)?.sync_all()?;
        Ok(())
    }
    fn document(&self) -> Result<Value> {
        match std::fs::read(self.credentials_path()) {
            Ok(bytes) => {
                let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
                    Error::Conflict(format!("{CREDENTIALS_FILE} is not valid JSON."))
                })?;
                if !value.is_object() {
                    return Err(Error::Conflict(format!(
                        "{CREDENTIALS_FILE} must contain a JSON object."
                    )));
                }
                Ok(value)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
            Err(error) => Err(error.into()),
        }
    }
    fn has_encrypted_values(&self) -> Result<bool> {
        Ok(self
            .document()?
            .as_object()
            .into_iter()
            .flatten()
            .any(|(_, entry)| {
                entry["token"]
                    .as_str()
                    .is_some_and(|t| t.starts_with(PREFIX))
            }))
    }
    /// Decrypts one integration's stored token in memory.
    pub fn token(&self, integration: Integration) -> StoredToken {
        let Ok(document) = self.document() else {
            return StoredToken::Unreadable;
        };
        let Some(value) = document[integration.id()].get("token") else {
            return StoredToken::Missing;
        };
        let Some(sealed) = value.as_str() else {
            return StoredToken::Unreadable;
        };
        let KeyState::Present(key) = self.key() else {
            return StoredToken::Unreadable;
        };
        match open(&key, integration, sealed) {
            Some(token) => StoredToken::Present(token),
            None => StoredToken::Unreadable,
        }
    }
    /// Encrypts and stores a token. A missing key is created only when no other
    /// encrypted value would be orphaned, or when the caller explicitly replaces
    /// unreadable values (which are then removed and must be reconnected).
    pub fn save(
        &self,
        integration: Integration,
        token: &str,
        replace_unreadable: bool,
    ) -> Result<()> {
        let token = validate(token)?;
        let _guard = WRITES.lock().map_err(|_| poisoned())?;
        let mut document = self.document()?;
        let key = match self.key() {
            KeyState::Present(key) => key,
            KeyState::Invalid => {
                return Err(Error::Conflict(format!(
                    "{SECRET_FILE} is unreadable or not private (owner-only, 0600). Restore it from the backup taken with {CREDENTIALS_FILE}; Crewroom does not replace it."
                )));
            }
            KeyState::Missing => {
                let orphaned = self.has_encrypted_values()?;
                if orphaned && !replace_unreadable {
                    return Err(Error::Conflict(format!(
                        "{SECRET_FILE} is missing while encrypted credentials exist. Restore it from the same backup, or reconnect to replace the unreadable credentials."
                    )));
                }
                if orphaned {
                    // Without the key every encrypted value is unreadable.
                    document = json!({});
                }
                self.create_key()?;
                match self.key() {
                    KeyState::Present(key) => key,
                    _ => {
                        return Err(Error::Conflict(format!(
                            "{SECRET_FILE} could not be read after creation."
                        )));
                    }
                }
            }
        };
        if replace_unreadable {
            for other in Integration::ALL {
                if other != integration
                    && let Some(sealed) = document[other.id()]["token"].as_str()
                    && open(&key, other, sealed).is_none()
                {
                    document.as_object_mut().unwrap().remove(other.id());
                }
            }
        }
        document[integration.id()] = json!({"token": seal(&key, integration, &token)?});
        self.write(&document)
    }
    /// Removes one integration's stored token. Other values and the key remain.
    pub fn clear(&self, integration: Integration) -> Result<()> {
        let _guard = WRITES.lock().map_err(|_| poisoned())?;
        let mut document = self.document()?;
        if document
            .as_object_mut()
            .unwrap()
            .remove(integration.id())
            .is_some()
        {
            self.write(&document)?;
        }
        Ok(())
    }
    fn write(&self, document: &Value) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(document)?;
        bytes.push(b'\n');
        crewroom_core::authority::write_private(&self.credentials_path(), &bytes)
    }
    /// Public facts about one integration's credential: where the active value
    /// comes from and whether a stored value can be used. Never the value itself.
    pub fn status(&self, integration: Integration, environment: Option<&str>) -> Value {
        let stored = match self.token(integration) {
            StoredToken::Missing => "none",
            StoredToken::Present(_) => "stored",
            StoredToken::Unreadable => "unreadable",
        };
        let overridden = environment.is_some_and(|v| !v.is_empty());
        let (source, label) = if overridden {
            (
                "environment",
                format!(
                    "Using {} from the server environment; it overrides any saved token.",
                    integration.environment()
                ),
            )
        } else {
            match stored {
                "stored" => (
                    "stored",
                    "Saved token (encrypted with this install's key).".into(),
                ),
                "unreadable" => ("unreadable", UNREADABLE.into()),
                _ => ("none", "No token saved.".into()),
            }
        };
        json!({
            "source": source,
            "stored": stored,
            "environment": integration.environment(),
            "label": label,
        })
    }
}
fn poisoned() -> Error {
    Error::Conflict("Credential store lock needs a restart.".into())
}
fn validate(token: &str) -> Result<Zeroizing<String>> {
    let token = token.trim();
    if token.is_empty()
        || token.len() > MAX_TOKEN
        || token.chars().any(|c| c.is_control() || c == ' ')
    {
        return Err(Error::Invalid(
            "Enter a single-line access token without spaces (at most 8192 characters).".into(),
        ));
    }
    Ok(Zeroizing::new(token.to_owned()))
}
fn aad(integration: Integration) -> Vec<u8> {
    format!("crewroom:v1:{}", integration.id()).into_bytes()
}
fn seal(key: &[u8; KEY_BYTES], integration: Integration, token: &str) -> Result<String> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce = [0u8; NONCE_BYTES];
    getrandom::getrandom(&mut nonce)
        .map_err(|_| Error::Conflict("The system random source is unavailable.".into()))?;
    let aad = aad(integration);
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: token.as_bytes(),
                aad: &aad,
            },
        )
        .map_err(|_| Error::Conflict("Credential encryption failed.".into()))?;
    let mut sealed = nonce.to_vec();
    sealed.extend_from_slice(&ciphertext);
    Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(sealed)))
}
fn open(key: &[u8; KEY_BYTES], integration: Integration, value: &str) -> Option<Zeroizing<String>> {
    let bytes = URL_SAFE_NO_PAD.decode(value.strip_prefix(PREFIX)?).ok()?;
    if bytes.len() <= NONCE_BYTES {
        return None;
    }
    let (nonce, ciphertext) = bytes.split_at(NONCE_BYTES);
    let aad = aad(integration);
    let plain = Zeroizing::new(
        XChaCha20Poly1305::new(key.into())
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .ok()?,
    );
    let text = String::from_utf8(plain.to_vec()).ok()?;
    (!text.is_empty()).then(|| Zeroizing::new(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const TOKEN: &str = "synthetic-stored-token-0001";

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }
    fn present(token: StoredToken) -> String {
        match token {
            StoredToken::Present(v) => v.to_string(),
            other => panic!("expected a readable token, got {other:?}"),
        }
    }

    #[test]
    fn first_run_creates_one_private_key_and_reopening_keeps_it() {
        let root = tempfile::tempdir().unwrap();
        let store = SecretStore::open(root.path()).unwrap();
        let key = std::fs::read(store.secret_path()).unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(mode(&store.secret_path()), 0o600);
        assert!(!store.credentials_path().exists());
        SecretStore::open(root.path()).unwrap();
        assert_eq!(std::fs::read(store.secret_path()).unwrap(), key);
        let other = tempfile::tempdir().unwrap();
        SecretStore::open(other.path()).unwrap();
        assert_ne!(std::fs::read(other.path().join(SECRET_FILE)).unwrap(), key);
    }

    #[test]
    fn saved_tokens_are_encrypted_on_disk_and_bound_to_their_integration() {
        let root = tempfile::tempdir().unwrap();
        let store = SecretStore::open(root.path()).unwrap();
        store
            .save(Integration::Jira, &format!("  {TOKEN}\n"), false)
            .unwrap();
        store
            .save(Integration::Bitbucket, "synthetic-bitbucket-0002", false)
            .unwrap();
        let raw = std::fs::read_to_string(store.credentials_path()).unwrap();
        assert!(!raw.contains("synthetic"));
        assert_eq!(mode(&store.credentials_path()), 0o600);
        let document: Value = serde_json::from_str(&raw).unwrap();
        let sealed = document["jira"]["token"].as_str().unwrap();
        assert!(sealed.starts_with("$crewroom$v1$"));
        assert_eq!(present(store.token(Integration::Jira)), TOKEN);
        assert_eq!(
            present(store.token(Integration::Bitbucket)),
            "synthetic-bitbucket-0002"
        );
        // Two encryptions of one token differ (random nonce).
        store.save(Integration::Jira, TOKEN, false).unwrap();
        let again: Value =
            serde_json::from_slice(&std::fs::read(store.credentials_path()).unwrap()).unwrap();
        assert_ne!(again["jira"]["token"].as_str().unwrap(), sealed);
        // A value copied to another integration does not decrypt there.
        let mut swapped = again.clone();
        swapped["bitbucket"]["token"] = again["jira"]["token"].clone();
        std::fs::write(store.credentials_path(), swapped.to_string()).unwrap();
        assert_eq!(store.token(Integration::Bitbucket), StoredToken::Unreadable);
        store.clear(Integration::Bitbucket).unwrap();
        assert_eq!(store.token(Integration::Bitbucket), StoredToken::Missing);
        assert_eq!(present(store.token(Integration::Jira)), TOKEN);
        for bad in ["", "   ", "two words", "line\nbreak"] {
            assert!(matches!(
                store.save(Integration::Jira, bad, false),
                Err(Error::Invalid(_))
            ));
        }
        assert!(matches!(
            store.save(Integration::Jira, &"x".repeat(8193), false),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn missing_or_mismatched_key_is_unreadable_and_never_silently_regenerated() {
        let root = tempfile::tempdir().unwrap();
        let store = SecretStore::open(root.path()).unwrap();
        store.save(Integration::Jira, TOKEN, false).unwrap();
        store
            .save(Integration::Bitbucket, "synthetic-bitbucket-0002", false)
            .unwrap();
        let credentials = std::fs::read(store.credentials_path()).unwrap();

        // Missing key: reopening must not create a replacement.
        std::fs::remove_file(store.secret_path()).unwrap();
        SecretStore::open(root.path()).unwrap();
        assert!(!store.secret_path().exists());
        assert_eq!(store.token(Integration::Jira), StoredToken::Unreadable);
        let status = store.status(Integration::Jira, None);
        assert_eq!(status["source"], "unreadable");
        assert_eq!(status["label"], UNREADABLE);
        // An ordinary save refuses to orphan the other value.
        assert!(matches!(
            store.save(Integration::Jira, TOKEN, false),
            Err(Error::Conflict(_))
        ));
        assert!(!store.secret_path().exists());
        assert_eq!(
            std::fs::read(store.credentials_path()).unwrap(),
            credentials
        );
        // Explicit reconnect replaces unreadable values under a new key.
        store
            .save(Integration::Jira, "synthetic-reconnected", true)
            .unwrap();
        assert!(store.secret_path().exists());
        assert_eq!(
            present(store.token(Integration::Jira)),
            "synthetic-reconnected"
        );
        assert_eq!(store.token(Integration::Bitbucket), StoredToken::Missing);

        // Mismatched key (another install's secret): unreadable, key untouched.
        let foreign = tempfile::tempdir().unwrap();
        SecretStore::open(foreign.path()).unwrap();
        let foreign_key = std::fs::read(foreign.path().join(SECRET_FILE)).unwrap();
        std::fs::remove_file(store.secret_path()).unwrap();
        std::fs::copy(foreign.path().join(SECRET_FILE), store.secret_path()).unwrap();
        SecretStore::open(root.path()).unwrap();
        assert_eq!(std::fs::read(store.secret_path()).unwrap(), foreign_key);
        assert_eq!(store.token(Integration::Jira), StoredToken::Unreadable);
        // Reconnecting under the present key works without replacing it.
        store.save(Integration::Jira, TOKEN, false).unwrap();
        assert_eq!(std::fs::read(store.secret_path()).unwrap(), foreign_key);
        assert_eq!(present(store.token(Integration::Jira)), TOKEN);

        // A key readable by group/other is refused, not rewritten.
        std::fs::set_permissions(store.secret_path(), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        assert_eq!(store.token(Integration::Jira), StoredToken::Unreadable);
        assert!(matches!(
            store.save(Integration::Jira, TOKEN, true),
            Err(Error::Conflict(_))
        ));
        assert_eq!(std::fs::read(store.secret_path()).unwrap(), foreign_key);
    }

    #[test]
    fn environment_override_is_reported_as_the_active_source() {
        let root = tempfile::tempdir().unwrap();
        let store = SecretStore::open(root.path()).unwrap();
        assert_eq!(store.status(Integration::Jira, None)["source"], "none");
        assert_eq!(store.status(Integration::Jira, Some(""))["source"], "none");
        store.save(Integration::Jira, TOKEN, false).unwrap();
        assert_eq!(store.status(Integration::Jira, None)["source"], "stored");
        let status = store.status(Integration::Jira, Some("synthetic-env"));
        assert_eq!(status["source"], "environment");
        assert_eq!(status["stored"], "stored");
        assert!(
            status["label"]
                .as_str()
                .unwrap()
                .contains("CREWROOM_JIRA_TOKEN")
        );
        assert!(!status.to_string().contains("synthetic"));
    }

    #[test]
    fn whole_state_restore_with_the_secret_decrypts_existing_values() {
        let live = tempfile::tempdir().unwrap();
        let data = live.path().join("data");
        let store = SecretStore::open(&data).unwrap();
        store.save(Integration::Jira, TOKEN, false).unwrap();
        store
            .save(Integration::Bitbucket, "synthetic-bitbucket-0002", false)
            .unwrap();
        // Back up the stopped data directory, lose it, restore at the same path.
        let backup = tempfile::tempdir().unwrap();
        for name in [SECRET_FILE, CREDENTIALS_FILE] {
            std::fs::copy(data.join(name), backup.path().join(name)).unwrap();
            let permissions = std::fs::metadata(data.join(name)).unwrap().permissions();
            std::fs::set_permissions(backup.path().join(name), permissions).unwrap();
        }
        std::fs::remove_dir_all(&data).unwrap();
        std::fs::create_dir(&data).unwrap();
        for name in [SECRET_FILE, CREDENTIALS_FILE] {
            std::fs::copy(backup.path().join(name), data.join(name)).unwrap();
            let permissions = std::fs::metadata(backup.path().join(name))
                .unwrap()
                .permissions();
            std::fs::set_permissions(data.join(name), permissions).unwrap();
        }
        let restored = SecretStore::open(&data).unwrap();
        assert_eq!(present(restored.token(Integration::Jira)), TOKEN);
        assert_eq!(
            present(restored.token(Integration::Bitbucket)),
            "synthetic-bitbucket-0002"
        );
        // Restoring configuration without the secret leaves values unreadable.
        let partial = tempfile::tempdir().unwrap();
        std::fs::copy(
            backup.path().join(CREDENTIALS_FILE),
            partial.path().join(CREDENTIALS_FILE),
        )
        .unwrap();
        let partial = SecretStore::open(partial.path()).unwrap();
        assert!(!partial.secret_path().exists());
        assert_eq!(partial.token(Integration::Jira), StoredToken::Unreadable);
    }
}
