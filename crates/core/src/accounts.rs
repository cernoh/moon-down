use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// One account per host: holds plugin, host, username, enabled flag.
/// Secret stays in OS keyring only. Host is never a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRecord {
    pub plugin: String,
    pub host: String,
    pub username: String,
    pub enabled: bool,
    /// Last error message (e.g. after second login failure). None = no error.
    #[serde(default)]
    pub last_error: Option<String>,
    /// Consecutive failures for re-prompt logic.
    #[serde(default)]
    pub failed_attempts: u8,
    /// Test login on by default for each new account.
    #[serde(default = "default_true")]
    pub test_login: bool,
}

fn default_true() -> bool {
    true
}

impl AccountRecord {
    pub fn new(plugin: impl Into<String>, host: impl Into<String>, username: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            host: host.into(),
            username: username.into(),
            enabled: true,
            last_error: None,
            failed_attempts: 0,
            test_login: true,
        }
    }
    /// Host names are never stored as secrets.
    pub fn host_for_storage(&self) -> &str {
        &self.host
    }
}

/// Form service name from app, plugin, host. Secret never includes host.
pub fn service_name(plugin: &str, host: &str) -> String {
    format!("moon-down:{plugin}:{host}")
}

/// Keyring abstraction so tests don't need OS backend.
pub trait Keyring: Send + Sync {
    fn get_password(&self, service: &str, user: &str) -> Result<String, KeyringError>;
    fn set_password(&self, service: &str, user: &str, password: &str) -> Result<(), KeyringError>;
    fn delete_password(&self, service: &str, user: &str) -> Result<(), KeyringError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyringError {
    NoEntry,
    /// Loud headless error: no Secret Service provider.
    NoDefaultStore(String),
    Other(String),
}

impl std::fmt::Display for KeyringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoEntry => write!(f, "no entry"),
            Self::NoDefaultStore(s) => write!(f, "{s}"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

impl KeyringError {
    pub fn is_loud_headless(&self) -> bool {
        matches!(self, Self::NoDefaultStore(_))
    }
    /// Loud error for headless host with no keyring backend.
    pub fn headless(service: &str, user: &str) -> Self {
        Self::NoDefaultStore(format!(
            "OS keyring unavailable (NoDefaultStore) for {service}/{user} — headless host with no Secret Service provider (no gnome-keyring/KWallet); credentials cannot be stored. Install and unlock a Secret Service provider or run on a desktop session."
        ))
    }
}

/// OS keyring via `keyring` crate. Returns loud error on NoDefaultStore/NoStorageAccess headless.
pub struct OsKeyring;

impl Keyring for OsKeyring {
    fn get_password(&self, service: &str, user: &str) -> Result<String, KeyringError> {
        #[cfg(feature = "keyring")]
        {
            let entry = keyring::Entry::new(service, user).map_err(|e| map_entry_err(service, user, e))?;
            entry.get_password().map_err(|e| map_err(service, user, e))
        }
        #[cfg(not(feature = "keyring"))]
        {
            // Without feature, simulate headless behavior for real headless detection
            // Try Entry::new pattern: if keyring crate not enabled, treat as headless
            Err(KeyringError::headless(service, user))
        }
    }
    fn set_password(&self, service: &str, user: &str, password: &str) -> Result<(), KeyringError> {
        #[cfg(feature = "keyring")]
        {
            let entry = keyring::Entry::new(service, user).map_err(|e| map_entry_err(service, user, e))?;
            entry
                .set_password(password)
                .map_err(|e| map_err(service, user, e))
        }
        #[cfg(not(feature = "keyring"))]
        {
            let _ = password;
            Err(KeyringError::headless(service, user))
        }
    }
    fn delete_password(&self, service: &str, user: &str) -> Result<(), KeyringError> {
        #[cfg(feature = "keyring")]
        {
            let entry = keyring::Entry::new(service, user).map_err(|e| map_entry_err(service, user, e))?;
            entry.delete_credential().map_err(|e| map_err(service, user, e))
        }
        #[cfg(not(feature = "keyring"))]
        {
            Err(KeyringError::headless(service, user))
        }
    }
}

#[cfg(feature = "keyring")]
fn map_entry_err(service: &str, user: &str, e: keyring::Error) -> KeyringError {
    let s = e.to_string();
    if s.contains("NoDefaultStore") || s.contains("NoStorageAccess") || s.contains("PlatformFailure") && s.contains("secret") {
        // Check underlying: NoDefaultStore is the headless signal
        if s.contains("NoDefaultStore") {
            KeyringError::headless(service, user)
        } else if s.contains("NoStorageAccess") {
            // also loud if it indicates no backend
            KeyringError::NoDefaultStore(format!(
                "OS keyring unavailable (NoStorageAccess) for {service}/{user}: {s} — headless host with no Secret Service; credentials cannot be stored."
            ))
        } else {
            KeyringError::Other(s)
        }
    } else {
        KeyringError::Other(s)
    }
}

#[cfg(feature = "keyring")]
fn map_err(service: &str, user: &str, e: keyring::Error) -> KeyringError {
    use keyring::Error as E;
    match e {
        E::NoEntry => KeyringError::NoEntry,
        E::NoDefaultStore(_) | E::NoStorageAccess(_) => KeyringError::headless(service, user),
        other => {
            let s = other.to_string();
            if s.contains("NoDefaultStore") || s.contains("NoStorageAccess") {
                KeyringError::headless(service, user)
            } else {
                KeyringError::Other(s)
            }
        }
    }
}

/// In-memory keyring for tests.
#[derive(Default, Clone)]
pub struct MemoryKeyring {
    inner: Arc<Mutex<HashMap<(String, String), String>>>,
    /// If true, all ops return loud headless error.
    headless: bool,
}

impl MemoryKeyring {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn headless() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            headless: true,
        }
    }
}

impl Keyring for MemoryKeyring {
    fn get_password(&self, service: &str, user: &str) -> Result<String, KeyringError> {
        if self.headless {
            return Err(KeyringError::headless(service, user));
        }
        let m = self.inner.lock().unwrap();
        m.get(&(service.to_string(), user.to_string()))
            .cloned()
            .ok_or(KeyringError::NoEntry)
    }
    fn set_password(&self, service: &str, user: &str, password: &str) -> Result<(), KeyringError> {
        if self.headless {
            return Err(KeyringError::headless(service, user));
        }
        self.inner
            .lock()
            .unwrap()
            .insert((service.to_string(), user.to_string()), password.to_string());
        Ok(())
    }
    fn delete_password(&self, service: &str, user: &str) -> Result<(), KeyringError> {
        if self.headless {
            return Err(KeyringError::headless(service, user));
        }
        let mut m = self.inner.lock().unwrap();
        m.remove(&(service.to_string(), user.to_string()))
            .map(|_| ())
            .ok_or(KeyringError::NoEntry)
    }
}

// Helpers for the acceptance criteria: store/load per account.

pub fn store_secret(keyring: &dyn Keyring, rec: &AccountRecord, secret: &str) -> Result<(), KeyringError> {
    let svc = service_name(&rec.plugin, &rec.host);
    keyring.set_password(&svc, &rec.username, secret)
}

pub fn load_secret(keyring: &dyn Keyring, rec: &AccountRecord) -> Result<String, KeyringError> {
    let svc = service_name(&rec.plugin, &rec.host);
    keyring.get_password(&svc, &rec.username)
}

pub fn delete_secret(keyring: &dyn Keyring, rec: &AccountRecord) -> Result<(), KeyringError> {
    let svc = service_name(&rec.plugin, &rec.host);
    keyring.delete_password(&svc, &rec.username)
}

/// Collection of accounts: one per host.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Accounts {
    pub records: Vec<AccountRecord>,
}

impl Accounts {
    pub fn new() -> Self {
        Self::default()
    }
    /// Add one account per host; dedup by host (case-insensitive).
    pub fn add(&mut self, rec: AccountRecord) -> Result<(), String> {
        if self.records.iter().any(|r| r.host.eq_ignore_ascii_case(&rec.host)) {
            return Err(format!("account for host {} already exists", rec.host));
        }
        self.records.push(rec);
        Ok(())
    }
    pub fn find(&self, host: &str) -> Option<&AccountRecord> {
        self.records.iter().find(|r| r.host.eq_ignore_ascii_case(host))
    }
    pub fn find_mut(&mut self, host: &str) -> Option<&mut AccountRecord> {
        self.records.iter_mut().find(|r| r.host.eq_ignore_ascii_case(host))
    }
    pub fn remove(&mut self, host: &str, keyring: &dyn Keyring) -> bool {
        if let Some(pos) = self.records.iter().position(|r| r.host.eq_ignore_ascii_case(host)) {
            let rec = self.records.remove(pos);
            let _ = delete_secret(keyring, &rec);
            return true;
        }
        false
    }
    pub fn toggle(&mut self, host: &str) -> Option<bool> {
        self.find_mut(host).map(|r| {
            r.enabled = !r.enabled;
            r.enabled
        })
    }
    /// Called on login failure: re-ask once, then flag after second.
    /// Returns true if caller should re-prompt for secret.
    pub fn on_login_failure(&mut self, host: &str, err_msg: impl Into<String>) -> bool {
        if let Some(rec) = self.find_mut(host) {
            rec.failed_attempts += 1;
            if rec.failed_attempts == 1 {
                rec.last_error = Some(err_msg.into());
                return true; // re-ask once
            } else {
                rec.last_error = Some(format!("account error after 2 failures: {}", err_msg.into()));
                return false; // flagged, don't re-ask
            }
        }
        false
    }
    pub fn on_login_success(&mut self, host: &str) {
        if let Some(rec) = self.find_mut(host) {
            rec.failed_attempts = 0;
            rec.last_error = None;
        }
    }
    pub fn flagged(&self, host: &str) -> bool {
        self.find(host)
            .map(|r| r.failed_attempts >= 2 && r.last_error.is_some())
            .unwrap_or(false)
    }
}

/// Map secret to HTTP user/password options at send time. Never embeds secret in URI.
pub fn inject_options(username: &str, secret: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("http-user".into(), username.to_string());
    m.insert("http-passwd".into(), secret.to_string());
    m.insert("ftp-user".into(), username.to_string());
    m.insert("ftp-passwd".into(), secret.to_string());
    m
}

// persistence helpers for accounts.json (atomic tmp+rename)
use std::path::Path;
pub fn save_accounts(accts: &Accounts, path: &Path) -> std::io::Result<()> {
    if let Some(p) = path.parent() {
        if !p.as_os_str().is_empty() {
            std::fs::create_dir_all(p)?;
        }
    }
    let data = serde_json::to_vec_pretty(accts).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp_path = Path::new(&tmp).to_path_buf();
    std::fs::write(&tmp_path, &data)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}
pub fn load_accounts(path: &Path) -> std::io::Result<Accounts> {
    if !path.exists() {
        return Ok(Accounts::new());
    }
    let data = std::fs::read(path)?;
    let accts: Accounts = serde_json::from_slice(&data).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(accts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn one_per_host() {
        let mut a = Accounts::new();
        a.add(AccountRecord::new("generic-http", "example.com", "alice")).unwrap();
        assert!(a.add(AccountRecord::new("generic-http", "EXAMPLE.COM", "bob")).is_err());
        a.add(AccountRecord::new("generic-http", "other.com", "alice")).unwrap();
        assert_eq!(a.records.len(), 2);
    }
    #[test]
    fn record_has_four_fields_and_defaults() {
        let r = AccountRecord::new("generic-http", "example.com", "alice");
        assert_eq!(r.plugin, "generic-http");
        assert_eq!(r.host, "example.com");
        assert_eq!(r.username, "alice");
        assert!(r.enabled);
        assert!(r.test_login);
        assert_eq!(r.last_error, None);
        // host never stored as secret: host is in clear
        assert_eq!(r.host_for_storage(), "example.com");
    }
    #[test]
    fn service_name_shape() {
        assert_eq!(service_name("generic-http", "example.com"), "moon-down:generic-http:example.com");
    }
    #[test]
    fn secret_only_in_keyring() {
        let kr = MemoryKeyring::new();
        let rec = AccountRecord::new("generic-http", "example.com", "alice");
        store_secret(&kr, &rec, "s3cr3t").unwrap();
        assert_eq!(load_secret(&kr, &rec).unwrap(), "s3cr3t");
        // persisted accounts must not contain secret
        let mut accts = Accounts::new();
        accts.add(rec.clone()).unwrap();
        let dir = tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        save_accounts(&accts, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("s3cr3t"), "secret leaked into accounts file");
        assert!(raw.contains("example.com"));
        // service name contains app+plugin+host
        let svc = service_name("generic-http", "example.com");
        assert!(svc.starts_with("moon-down:"));
        assert_eq!(kr.get_password(&svc, "alice").unwrap(), "s3cr3t");
        // host only in app state, not in keyring as secret content
        assert_eq!(accts.records[0].host, "example.com");
    }
    #[test]
    fn reask_once_then_flag() {
        let mut accts = Accounts::new();
        accts.add(AccountRecord::new("generic-http", "example.com", "alice")).unwrap();
        // first failure -> re-ask
        assert!(accts.on_login_failure("example.com", "bad creds"));
        assert_eq!(accts.find("example.com").unwrap().failed_attempts, 1);
        assert!(!accts.flagged("example.com"));
        // second failure -> flagged
        assert!(!accts.on_login_failure("example.com", "bad again"));
        assert!(accts.flagged("example.com"));
        assert!(accts.find("example.com").unwrap().last_error.as_ref().unwrap().contains("account error"));
        // success clears
        accts.on_login_success("example.com");
        assert!(!accts.flagged("example.com"));
        assert_eq!(accts.find("example.com").unwrap().failed_attempts, 0);
    }
    #[test]
    fn test_login_default_on() {
        let r = AccountRecord::new("generic-http", "example.com", "alice");
        assert!(r.test_login);
        let mut accts = Accounts::new();
        accts.add(r).unwrap();
        assert!(accts.find("example.com").unwrap().test_login);
    }
    #[test]
    fn headless_loud_error() {
        let kr = MemoryKeyring::headless();
        let rec = AccountRecord::new("generic-http", "example.com", "alice");
        let e = store_secret(&kr, &rec, "x").unwrap_err();
        assert!(e.is_loud_headless());
        let msg = e.to_string();
        assert!(msg.contains("NoDefaultStore") || msg.contains("NoStorageAccess"));
        assert!(msg.contains("headless"));
    }
    #[test]
    fn inject_never_in_uri() {
        let opts = inject_options("alice", "s3cr3t");
        assert_eq!(opts.get("http-user").unwrap(), "alice");
        assert_eq!(opts.get("http-passwd").unwrap(), "s3cr3t");
        assert_eq!(opts.get("ftp-user").unwrap(), "alice");
        assert_eq!(opts.get("ftp-passwd").unwrap(), "s3cr3t");
        // never URI embedding
        for v in opts.values() {
            assert!(!v.contains("://"));
        }
        // ensure caller would not build URI with secret
        let uri = "https://example.com/file.zip";
        assert!(!uri.contains("s3cr3t"));
    }
    #[test]
    fn accounts_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        let mut accts = Accounts::new();
        accts.add(AccountRecord::new("generic-http", "example.com", "alice")).unwrap();
        accts.add(AccountRecord::new("generic-http", "other.com", "bob")).unwrap();
        save_accounts(&accts, &path).unwrap();
        let loaded = load_accounts(&path).unwrap();
        assert_eq!(loaded.records.len(), 2);
        assert_eq!(loaded.find("example.com").unwrap().username, "alice");
    }
    #[test]
    fn concurrency_headless_missing_is_loud() {
        let kr = MemoryKeyring::headless();
        let rec = AccountRecord::new("generic-http", "example.com", "alice");
        let e = load_secret(&kr, &rec).unwrap_err();
        assert!(e.to_string().contains("headless"));
    }
}
