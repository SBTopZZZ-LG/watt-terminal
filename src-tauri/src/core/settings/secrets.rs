//! Where secret values live: API keys and secret header values, kept out of `settings.json`.
//!
//! All secrets sit in one blob, a JSON object `{key: value}`, behind a `SecretStore`. The production store is one
//! Keychain item, so the OS asks at most once. `FileStore` is an explicit opt-in (`WATT_SECRETS_FILE`) for
//! development. `Secrets` reads the blob lazily on first use and keeps it cached. Nothing here prints or formats a
//! value: errors and `Debug` output carry names and kinds only.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use keyring::Entry;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::write_private_atomic;
use crate::core::env::env_setting;

// The Keychain item is named after the app's identifier (`dev.watt.terminal`, see tauri.conf.json). The user's saved
// keys were stored under the old identifier, `LEGACY_KEYCHAIN_SERVICE`. The first launch under the new name copies that
// item across (`MigratingStore`). The old item is only read for the copy: the app never changes or deletes it.
const KEYCHAIN_SERVICE: &str = "dev.watt.terminal";
const LEGACY_KEYCHAIN_SERVICE: &str = "dev.agentterm.app";
const KEYCHAIN_ACCOUNT: &str = "secrets";
const SECRETS_FILE_ENV: &str = "WATT_SECRETS_FILE";
const LEGACY_SECRETS_FILE_ENV: &str = "AGENT_TERM_SECRETS_FILE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// The store cannot be read or written (Keychain denied or missing, file error). The message holds no secret.
    Unavailable(String),
    /// The stored blob is not a JSON object of strings. Nothing is overwritten.
    Corrupt,
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretError::Unavailable(msg) => write!(f, "secret store unavailable: {msg}"),
            SecretError::Corrupt => {
                write!(f, "stored secrets are not a JSON object of strings; refusing to overwrite them")
            }
        }
    }
}

impl std::error::Error for SecretError {}

fn unavailable(e: impl fmt::Display) -> SecretError {
    SecretError::Unavailable(e.to_string())
}

/// Holds the whole secrets blob. `load` returns `None` when nothing has been stored yet.
pub trait SecretStore: Send + Sync {
    fn load(&self) -> Result<Option<String>, SecretError>;
    fn save(&self, blob: &str) -> Result<(), SecretError>;
    fn kind(&self) -> &'static str;
}

/// One macOS Keychain item (generic password) holding the blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeychainStore {
    pub service: String,
    pub account: String,
}

impl KeychainStore {
    fn entry(&self) -> Result<Entry, SecretError> {
        Entry::new(&self.service, &self.account).map_err(unavailable)
    }
}

impl SecretStore for KeychainStore {
    fn load(&self) -> Result<Option<String>, SecretError> {
        match self.entry()?.get_password() {
            Ok(blob) => Ok(Some(blob)),
            Err(keyring::Error::NoEntry) => Ok(None),
            // The variant carries the raw bytes, so it is mapped here and never formatted.
            Err(keyring::Error::BadEncoding(_)) => Err(SecretError::Corrupt),
            Err(e) => Err(unavailable(e)),
        }
    }

    fn save(&self, blob: &str) -> Result<(), SecretError> {
        self.entry()?.set_password(blob).map_err(unavailable)
    }

    fn kind(&self) -> &'static str {
        "keychain"
    }
}

/// A file holding the blob, mode 0600. Used only when `WATT_SECRETS_FILE` is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStore {
    pub path: PathBuf,
}

impl SecretStore for FileStore {
    fn load(&self) -> Result<Option<String>, SecretError> {
        match fs::read_to_string(&self.path) {
            Ok(blob) => Ok(Some(blob)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(unavailable(format!("cannot read {}: {e}", self.path.display()))),
        }
    }

    fn save(&self, blob: &str) -> Result<(), SecretError> {
        write_private_atomic(&self.path, blob.as_bytes())
            .map_err(|e| unavailable(format!("cannot write {}: {e}", self.path.display())))
    }

    fn kind(&self) -> &'static str {
        "file"
    }
}

/// The macOS store: `primary` (the new Keychain item) with `legacy` (the item under the old identifier) behind it. When
/// `primary` is empty, the legacy blob is copied into it and read back. The legacy item is never changed or deleted.
/// If the copy fails, the next launch copies again, because the new item is still missing. If the copy reads back
/// different, the new item is wrong and the next launch uses it; the next save of a key rewrites it from the cache.
/// In both cases this session uses the legacy blob. Errors and logs carry no blob content.
pub struct MigratingStore {
    primary: Box<dyn SecretStore>,
    legacy: Box<dyn SecretStore>,
}

impl MigratingStore {
    pub fn new(primary: Box<dyn SecretStore>, legacy: Box<dyn SecretStore>) -> Self {
        Self { primary, legacy }
    }
}

impl SecretStore for MigratingStore {
    fn load(&self) -> Result<Option<String>, SecretError> {
        if let Some(blob) = self.primary.load()? {
            return Ok(Some(blob));
        }
        // A failed legacy read is an error, not an empty store: an empty answer would let a save write the new item
        // without the user's keys.
        let Some(legacy) = self.legacy.load()? else {
            return Ok(None);
        };
        // The copy is checked by reading it back. Either way this session uses the legacy blob, the source of truth;
        // a failed or different copy is reported on stderr, with no content.
        let copied = self.primary.save(&legacy).and_then(|()| self.primary.load());
        if !matches!(&copied, Ok(Some(back)) if *back == legacy) {
            eprintln!(
                "watt: the saved secrets were not copied to the new Keychain item; this session uses the old one"
            );
        }
        Ok(Some(legacy))
    }

    fn save(&self, blob: &str) -> Result<(), SecretError> {
        self.primary.save(blob)
    }

    fn kind(&self) -> &'static str {
        "keychain"
    }
}

/// In-memory store for tests. Clones share the same state, so a test can keep a handle after giving one to `Secrets`.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct MemoryStore {
    inner: Arc<MemoryState>,
}

#[cfg(test)]
#[derive(Default)]
struct MemoryState {
    blob: Mutex<Option<String>>,
    loads: AtomicUsize,
    fail_loads: AtomicBool,
    fail_saves: AtomicBool,
}

#[cfg(test)]
impl MemoryStore {
    pub fn with_blob(blob: &str) -> Self {
        let store = Self::default();
        *store.inner.blob.lock().unwrap() = Some(blob.to_string());
        store
    }

    pub fn loads(&self) -> usize {
        self.inner.loads.load(Ordering::SeqCst)
    }

    pub fn stored(&self) -> Option<String> {
        self.inner.blob.lock().unwrap().clone()
    }

    pub fn set_fail_saves(&self, fail: bool) {
        self.inner.fail_saves.store(fail, Ordering::SeqCst);
    }

    pub fn set_fail_loads(&self, fail: bool) {
        self.inner.fail_loads.store(fail, Ordering::SeqCst);
    }
}

#[cfg(test)]
impl SecretStore for MemoryStore {
    fn load(&self) -> Result<Option<String>, SecretError> {
        self.inner.loads.fetch_add(1, Ordering::SeqCst);
        if self.inner.fail_loads.load(Ordering::SeqCst) {
            return Err(SecretError::Unavailable("simulated failure".into()));
        }
        Ok(self.inner.blob.lock().unwrap().clone())
    }

    fn save(&self, blob: &str) -> Result<(), SecretError> {
        if self.inner.fail_saves.load(Ordering::SeqCst) {
            return Err(SecretError::Unavailable("simulated failure".into()));
        }
        *self.inner.blob.lock().unwrap() = Some(blob.to_string());
        Ok(())
    }

    fn kind(&self) -> &'static str {
        "memory"
    }
}

pub type SecretMap = BTreeMap<String, String>;

/// The secrets, read through a `SecretStore`. The blob is read on first use, not at construction, so starting the app
/// never touches the Keychain.
pub struct Secrets {
    store: Box<dyn SecretStore>,
    cache: Mutex<Option<SecretMap>>,
}

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secrets {{ store: {} }}", self.store.kind())
    }
}

impl Secrets {
    pub fn new(store: Box<dyn SecretStore>) -> Self {
        Self { store, cache: Mutex::new(None) }
    }

    /// `WATT_SECRETS_FILE` (or the old `AGENT_TERM_SECRETS_FILE`), a non-empty path, selects a `FileStore`. Otherwise
    /// the macOS Keychain is used. There is no automatic fallback from the Keychain to a file. `config_dir` is not
    /// used yet.
    pub fn from_env(_config_dir: &Path) -> Self {
        match env_setting(SECRETS_FILE_ENV, LEGACY_SECRETS_FILE_ENV) {
            Some(path) => Self::new(Box::new(FileStore { path: PathBuf::from(path) })),
            None => Self::new(Box::new(MigratingStore::new(
                Box::new(KeychainStore { service: KEYCHAIN_SERVICE.into(), account: KEYCHAIN_ACCOUNT.into() }),
                Box::new(KeychainStore { service: LEGACY_KEYCHAIN_SERVICE.into(), account: KEYCHAIN_ACCOUNT.into() }),
            ))),
        }
    }

    /// `"keychain"` or `"file"` (or `"memory"` in tests): where the blob is kept. Names the store, holds no value.
    pub fn kind(&self) -> &'static str {
        self.store.kind()
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        let mut cache = self.lock()?;
        Ok(self.loaded(&mut cache)?.get(key).cloned())
    }

    /// A copy of every stored secret, for rolling a multi-step change back with `edit`.
    pub fn snapshot(&self) -> Result<SecretMap, SecretError> {
        let mut cache = self.lock()?;
        Ok(self.loaded(&mut cache)?.clone())
    }

    /// Applies `change` to the whole map and saves it once, if the map changed. Used so one save covers a whole
    /// provider edit. A failed save puts the cache back.
    pub fn edit(&self, change: impl FnOnce(&mut SecretMap)) -> Result<(), SecretError> {
        self.update(|map| {
            let before = map.clone();
            change(map);
            *map != before
        })
    }

    pub fn has(&self, key: &str) -> Result<bool, SecretError> {
        let mut cache = self.lock()?;
        Ok(self.loaded(&mut cache)?.contains_key(key))
    }

    /// Stores `value` under `key`. An empty value removes the key.
    pub fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        if value.is_empty() {
            return self.remove(key);
        }
        self.update(|map| {
            if map.get(key).map(String::as_str) == Some(value) {
                return false;
            }
            map.insert(key.to_string(), value.to_string());
            true
        })
    }

    pub fn remove(&self, key: &str) -> Result<(), SecretError> {
        self.update(|map| map.remove(key).is_some())
    }

    /// Removes every key that starts with `prefix`, in one save. Used when a provider is deleted.
    pub fn remove_prefix(&self, prefix: &str) -> Result<(), SecretError> {
        self.update(|map| {
            let before = map.len();
            map.retain(|key, _| !key.starts_with(prefix));
            map.len() != before
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Option<SecretMap>>, SecretError> {
        self.cache.lock().map_err(|_| unavailable("internal lock poisoned"))
    }

    /// Returns the cached map, reading it from the store the first time. A failed read is not cached, so the next call
    /// tries again.
    fn loaded<'a>(&self, cache: &'a mut Option<SecretMap>) -> Result<&'a mut SecretMap, SecretError> {
        if cache.is_none() {
            let map = match self.store.load()? {
                None => SecretMap::new(),
                Some(blob) => serde_json::from_str::<SecretMap>(&blob).map_err(|_| SecretError::Corrupt)?,
            };
            *cache = Some(map);
        }
        Ok(cache.get_or_insert_with(SecretMap::new))
    }

    /// Applies `change` to the cached map and saves the result. `change` returns false when nothing changed, and then
    /// nothing is saved. If the save fails, the cache goes back to what it was.
    fn update(&self, change: impl FnOnce(&mut SecretMap) -> bool) -> Result<(), SecretError> {
        let mut cache = self.lock()?;
        let map = self.loaded(&mut cache)?;
        let before = map.clone();
        if !change(map) {
            return Ok(());
        }
        let outcome = serde_json::to_string(map)
            .map_err(|_| unavailable("could not encode secrets"))
            .and_then(|blob| self.store.save(&blob));
        if outcome.is_err() {
            *map = before;
        }
        outcome
    }
}

/// Secret name for a provider's API key.
pub fn api_key_name(provider_id: &str) -> String {
    format!("provider.{provider_id}.api_key")
}

/// Secret name for a secret header of a provider. Header names are case-insensitive, so the name is lowercased.
pub fn header_secret_name(provider_id: &str, header_name: &str) -> String {
    format!("provider.{provider_id}.header.{}", header_name.to_ascii_lowercase())
}

/// Every secret name of a provider starts with this.
pub fn provider_prefix(provider_id: &str) -> String {
    format!("provider.{provider_id}.")
}

#[cfg(test)]
mod tests {
    use super::super::test_support::TempDir;
    use super::*;

    fn memory(blob: Option<&str>) -> (Secrets, MemoryStore) {
        let store = match blob {
            Some(blob) => MemoryStore::with_blob(blob),
            None => MemoryStore::default(),
        };
        (Secrets::new(Box::new(store.clone())), store)
    }

    #[test]
    fn blob_is_read_once_and_only_on_first_use() {
        let (secrets, store) = memory(Some(r#"{"a":"1"}"#));
        assert_eq!(store.loads(), 0, "construction must not read the store");
        assert_eq!(secrets.get("a").unwrap().as_deref(), Some("1"));
        assert_eq!(secrets.get("b").unwrap(), None);
        assert!(secrets.has("a").unwrap());
        assert!(!secrets.has("b").unwrap());
        assert_eq!(store.loads(), 1);
    }

    #[test]
    fn set_get_remove() {
        let (secrets, store) = memory(None);
        assert_eq!(secrets.get("k").unwrap(), None);
        secrets.set("k", "v1").unwrap();
        assert_eq!(secrets.get("k").unwrap().as_deref(), Some("v1"));
        assert_eq!(store.stored().as_deref(), Some(r#"{"k":"v1"}"#));
        secrets.set("k", "v2").unwrap();
        assert_eq!(secrets.get("k").unwrap().as_deref(), Some("v2"));
        secrets.remove("k").unwrap();
        assert_eq!(secrets.get("k").unwrap(), None);
        assert_eq!(store.stored().as_deref(), Some("{}"));
    }

    #[test]
    fn empty_value_removes_the_key() {
        let (secrets, store) = memory(None);
        secrets.set("k", "v").unwrap();
        secrets.set("k", "").unwrap();
        assert!(!secrets.has("k").unwrap());
        assert!(!store.stored().unwrap().contains("\"k\""));
    }

    #[test]
    fn remove_prefix_removes_one_provider_only() {
        let (secrets, store) = memory(None);
        secrets.set(&api_key_name("a"), "1").unwrap();
        secrets.set(&header_secret_name("a", "X-Key"), "2").unwrap();
        secrets.set(&api_key_name("ab"), "3").unwrap();
        secrets.remove_prefix(&provider_prefix("a")).unwrap();
        assert!(!secrets.has(&api_key_name("a")).unwrap());
        assert!(!secrets.has(&header_secret_name("a", "x-key")).unwrap());
        assert_eq!(secrets.get(&api_key_name("ab")).unwrap().as_deref(), Some("3"));
        assert_eq!(store.stored().as_deref(), Some(r#"{"provider.ab.api_key":"3"}"#));
    }

    #[test]
    fn key_names() {
        assert_eq!(api_key_name("openrouter"), "provider.openrouter.api_key");
        assert_eq!(header_secret_name("p", "X-Api-Key"), "provider.p.header.x-api-key");
        assert_eq!(provider_prefix("p"), "provider.p.");
        assert!(header_secret_name("p", "A").starts_with(&provider_prefix("p")));
    }

    #[test]
    fn failed_save_rolls_the_cache_back() {
        let (secrets, store) = memory(Some(r#"{"old":"1"}"#));
        store.set_fail_saves(true);

        assert!(matches!(secrets.set("new", "2"), Err(SecretError::Unavailable(_))));
        assert!(!secrets.has("new").unwrap());

        assert!(secrets.set("old", "changed").is_err());
        assert_eq!(secrets.get("old").unwrap().as_deref(), Some("1"));

        assert!(secrets.remove("old").is_err());
        assert_eq!(secrets.get("old").unwrap().as_deref(), Some("1"));

        assert!(secrets.remove_prefix("").is_err());
        assert_eq!(secrets.get("old").unwrap().as_deref(), Some("1"));

        store.set_fail_saves(false);
        secrets.set("new", "2").unwrap();
        assert_eq!(secrets.get("new").unwrap().as_deref(), Some("2"));
    }

    #[test]
    fn corrupt_blob_is_refused_and_never_overwritten() {
        for blob in ["not json", "[1, 2]", r#"{"k": 5}"#] {
            let (secrets, store) = memory(Some(blob));
            assert_eq!(secrets.get("k"), Err(SecretError::Corrupt), "blob {blob:?}");
            assert_eq!(secrets.set("k", "v"), Err(SecretError::Corrupt), "blob {blob:?}");
            assert_eq!(secrets.remove("k"), Err(SecretError::Corrupt), "blob {blob:?}");
            assert_eq!(store.stored().as_deref(), Some(blob), "the blob must not change");
        }
    }

    #[test]
    fn debug_and_errors_never_show_values() {
        let value = "DUMMY-secret-value-77";
        let (secrets, store) = memory(None);
        secrets.set("k", value).unwrap();
        assert!(!format!("{secrets:?}").contains(value));

        store.set_fail_saves(true);
        let err = secrets.set("k2", value).unwrap_err();
        assert!(!err.to_string().contains(value));
        assert!(!format!("{err:?}").contains(value));
    }

    #[test]
    fn file_store_missing_file_is_none() {
        let dir = TempDir::new();
        let store = FileStore { path: dir.path().join("secrets.json") };
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn file_store_round_trip_is_private_atomic_and_leaves_no_temp_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let path = dir.path().join("config").join("secrets.json");
        let store = FileStore { path: path.clone() };

        store.save(r#"{"a":"1"}"#).unwrap();
        store.save(r#"{"a":"2"}"#).unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some(r#"{"a":"2"}"#));

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let dir_mode = fs::metadata(path.parent().unwrap()).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700);

        let names: Vec<String> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["secrets.json".to_string()]);
    }

    #[test]
    fn secrets_over_a_file_store_persist_across_instances() {
        let dir = TempDir::new();
        let path = dir.path().join("secrets.json");
        let first = Secrets::new(Box::new(FileStore { path: path.clone() }));
        first.set(&api_key_name("p"), "DUMMY-key").unwrap();
        let second = Secrets::new(Box::new(FileStore { path }));
        assert_eq!(second.get(&api_key_name("p")).unwrap().as_deref(), Some("DUMMY-key"));
    }

    /// A store whose saves come back changed, to exercise the read-back check.
    struct ChangingStore(MemoryStore);

    impl SecretStore for ChangingStore {
        fn load(&self) -> Result<Option<String>, SecretError> {
            self.0.load()
        }

        fn save(&self, blob: &str) -> Result<(), SecretError> {
            self.0.save(&format!("{blob} "))
        }

        fn kind(&self) -> &'static str {
            "memory"
        }
    }

    fn migrating(primary: &MemoryStore, legacy: &MemoryStore) -> MigratingStore {
        MigratingStore::new(Box::new(primary.clone()), Box::new(legacy.clone()))
    }

    #[test]
    fn legacy_blob_is_copied_once_and_the_legacy_item_is_left_alone() {
        let primary = MemoryStore::default();
        let legacy = MemoryStore::with_blob(r#"{"a":"1"}"#);
        let store = migrating(&primary, &legacy);

        assert_eq!(store.load().unwrap().as_deref(), Some(r#"{"a":"1"}"#));
        assert_eq!(primary.stored().as_deref(), Some(r#"{"a":"1"}"#));
        assert_eq!(legacy.stored().as_deref(), Some(r#"{"a":"1"}"#));

        // Once the new item exists it is the one read, so a later change to the old item does not come back.
        legacy.save(r#"{"a":"changed"}"#).unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some(r#"{"a":"1"}"#));
        assert_eq!(legacy.stored().as_deref(), Some(r#"{"a":"changed"}"#));
    }

    #[test]
    fn the_new_item_wins_when_both_exist() {
        let primary = MemoryStore::with_blob(r#"{"a":"new"}"#);
        let legacy = MemoryStore::with_blob(r#"{"a":"old"}"#);
        let store = migrating(&primary, &legacy);

        assert_eq!(store.load().unwrap().as_deref(), Some(r#"{"a":"new"}"#));
        assert_eq!(primary.stored().as_deref(), Some(r#"{"a":"new"}"#));
        assert_eq!(legacy.stored().as_deref(), Some(r#"{"a":"old"}"#));
    }

    #[test]
    fn no_legacy_item_means_no_blob() {
        let primary = MemoryStore::default();
        let legacy = MemoryStore::default();
        let store = migrating(&primary, &legacy);

        assert_eq!(store.load().unwrap(), None);
        assert_eq!(primary.stored(), None, "nothing must be written when there is nothing to copy");
    }

    #[test]
    fn a_failed_copy_falls_back_to_the_legacy_blob_and_is_retried_later() {
        let primary = MemoryStore::default();
        let legacy = MemoryStore::with_blob(r#"{"a":"1"}"#);
        let store = migrating(&primary, &legacy);

        primary.set_fail_saves(true);
        assert_eq!(store.load().unwrap().as_deref(), Some(r#"{"a":"1"}"#));
        assert_eq!(primary.stored(), None);

        // The next launch sees the new item still missing and copies again.
        primary.set_fail_saves(false);
        let next_launch = migrating(&primary, &legacy);
        assert_eq!(next_launch.load().unwrap().as_deref(), Some(r#"{"a":"1"}"#));
        assert_eq!(primary.stored().as_deref(), Some(r#"{"a":"1"}"#));
    }

    #[test]
    fn a_copy_that_does_not_read_back_is_not_trusted_and_the_legacy_blob_is_used() {
        let primary = MemoryStore::default();
        let legacy = MemoryStore::with_blob(r#"{"a":"1"}"#);
        let store = MigratingStore::new(Box::new(ChangingStore(primary.clone())), Box::new(legacy.clone()));

        assert_eq!(store.load().unwrap().as_deref(), Some(r#"{"a":"1"}"#));
        assert_eq!(primary.stored().as_deref(), Some("{\"a\":\"1\"} "), "the copy is what was written");
        assert_eq!(legacy.stored().as_deref(), Some(r#"{"a":"1"}"#));
    }

    #[test]
    fn a_failed_legacy_read_is_an_error_and_writes_nothing_and_shows_no_value() {
        let value = "DUMMY-secret-value-88";
        let primary = MemoryStore::default();
        let legacy = MemoryStore::with_blob(&format!(r#"{{"k":"{value}"}}"#));
        legacy.set_fail_loads(true);
        let store = migrating(&primary, &legacy);

        let err = store.load().unwrap_err();
        assert!(matches!(err, SecretError::Unavailable(_)));
        assert!(!err.to_string().contains(value));
        assert!(!format!("{err:?}").contains(value));
        assert_eq!(primary.stored(), None, "an unreadable old item must not produce an empty new one");
    }

    #[test]
    fn secrets_over_a_migrating_store_read_the_old_keys() {
        let primary = MemoryStore::default();
        let legacy = MemoryStore::with_blob(r#"{"provider.p.api_key":"DUMMY-key"}"#);
        let secrets = Secrets::new(Box::new(migrating(&primary, &legacy)));
        assert_eq!(secrets.get("provider.p.api_key").unwrap().as_deref(), Some("DUMMY-key"));
        secrets.remove("provider.p.api_key").unwrap();
        assert_eq!(primary.stored().as_deref(), Some("{}"));
        assert!(legacy.stored().unwrap().contains("DUMMY-key"), "the old item is never changed");
    }

    /// Real macOS Keychain round trip. Ignored by default because it touches the login Keychain. Run with
    /// `cargo test -- --ignored keychain`. Uses a throwaway service name and dummy values, and deletes the item on
    /// drop, including when an assertion fails.
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        struct Cleanup(KeychainStore);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Ok(entry) = Entry::new(&self.0.service, &self.0.account) {
                    let _ = entry.delete_credential();
                }
            }
        }

        let store = KeychainStore {
            service: format!("watt-terminal-test-{}", std::process::id()),
            account: "secrets".into(),
        };
        let _cleanup = Cleanup(store.clone());

        assert_eq!(store.load().unwrap(), None, "the test service should start empty");

        // 4 KB of dummy text inside a JSON object, like a real blob.
        let value = "0123456789abcdef".repeat(256);
        let blob = serde_json::json!({ "provider.test.api_key": value }).to_string();
        assert!(blob.len() > 4096);

        store.save(&blob).unwrap();
        // assert! rather than assert_eq!, so a mismatch does not print the value.
        assert!(store.load().unwrap().as_deref() == Some(blob.as_str()), "round trip mismatch");

        let secrets = Secrets::new(Box::new(store.clone()));
        assert!(secrets.has("provider.test.api_key").unwrap());
        secrets.remove("provider.test.api_key").unwrap();
        assert!(store.load().unwrap().as_deref() == Some("{}"), "blob after removal is not empty");
    }
}
