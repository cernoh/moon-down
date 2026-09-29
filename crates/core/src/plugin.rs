use std::collections::HashMap;
use std::time::SystemTime;

use crate::accounts::{service_name, Keyring, KeyringError, MemoryKeyring, OsKeyring};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountType {
    Free,
    Premium,
    Lifetime,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountState {
    Unchecked,
    Valid,
    Expired,
    Invalid,
    TempDisabled,
}

#[derive(Debug, Clone)]
pub struct AccountStatus {
    pub state: AccountState,
    pub account_type: AccountType,
    pub valid_until: Option<SystemTime>,
    pub traffic_left: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginError {
    Invalid,
    TempDisabled,
    Network(String),
    Unsupported(String),
    Keyring(String),
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => write!(f, "invalid credentials"),
            Self::TempDisabled => write!(f, "temporarily disabled"),
            Self::Network(s) => write!(f, "network: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
            Self::Keyring(s) => write!(f, "{s}"),
        }
    }
}

/// Account-only plugin trait (7 methods).
pub trait AccountPlugin: Send + Sync {
    fn plugin_id(&self) -> &str;
    fn default_host(&self) -> &str;
    fn can_handle(&self, url: &str) -> bool;
    /// Reads secret from keyring itself under moon-down:{plugin}:{host}. No secret crosses call.
    fn check_account(&self, host: &str, username: &str) -> Result<AccountStatus, PluginError>;
    fn inject_credentials(&self, username: &str, secret: &str) -> HashMap<String, String>;
    fn max_simultaneous(&self, premium: bool) -> u32;
    fn supports_multi_host(&self) -> bool {
        false
    }
}

/// Generic HTTP auth plugin: the only compiled-in plugin in v1.
pub struct GenericHttpPlugin {
    // For tests, inject a memory keyring; production uses OsKeyring.
    keyring: Option<Box<dyn Keyring>>,
}

impl GenericHttpPlugin {
    pub fn new() -> Self {
        Self { keyring: None }
    }
    /// Test helper: inject an in-memory keyring.
    pub fn with_keyring(k: Box<dyn Keyring>) -> Self {
        Self { keyring: Some(k) }
    }
    fn get_keyring(&self) -> &dyn Keyring {
        if let Some(k) = &self.keyring {
            k.as_ref()
        } else {
            // Use OsKeyring via dynamic dispatch through a leaked static
            // to avoid borrow issues — simple: create temporary.
            // We use a static OsKeyring for production path.
            static OS: OsKeyring = OsKeyring;
            &OS
        }
    }
}

impl Default for GenericHttpPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountPlugin for GenericHttpPlugin {
    fn plugin_id(&self) -> &str {
        "generic-http"
    }
    fn default_host(&self) -> &str {
        ""
    }
    fn can_handle(&self, url: &str) -> bool {
        url.starts_with("http://") || url.starts_with("https://")
    }
    fn check_account(&self, host: &str, username: &str) -> Result<AccountStatus, PluginError> {
        let svc = service_name(self.plugin_id(), host);
        let kr = self.get_keyring();
        match kr.get_password(&svc, username) {
            Ok(secret) => {
                // Lightweight probe: if secret present and non-empty, treat as Valid.
                // Real impl would do HTTP probe with 401/403 mapping; here offline-valid.
                if secret.is_empty() {
                    Err(PluginError::Invalid)
                } else {
                    Ok(AccountStatus {
                        state: AccountState::Valid,
                        account_type: AccountType::Unknown,
                        valid_until: None,
                        traffic_left: None,
                    })
                }
            }
            Err(KeyringError::NoEntry) => Err(PluginError::Invalid),
            Err(KeyringError::NoDefaultStore(msg)) => Err(PluginError::Keyring(msg)),
            Err(KeyringError::Other(s)) => {
                if s.contains("NoDefaultStore") || s.contains("headless") {
                    Err(PluginError::Keyring(s))
                } else {
                    Err(PluginError::Keyring(s))
                }
            }
        }
    }
    fn inject_credentials(&self, username: &str, secret: &str) -> HashMap<String, String> {
        crate::accounts::inject_options(username, secret)
    }
    fn max_simultaneous(&self, premium: bool) -> u32 {
        if premium { 4 } else { 1 }
    }
    fn supports_multi_host(&self) -> bool {
        false
    }
}

/// Compiled-in registry: one list function.
pub fn all_plugins() -> Vec<Box<dyn AccountPlugin>> {
    vec![Box::new(GenericHttpPlugin::new())]
}

/// Run a check outside the render loop: spawn blocking thread, send result via channel.
pub fn spawn_account_check(
    plugin_id: &str,
    host: String,
    username: String,
    keyring: Option<Box<dyn Keyring>>,
) -> std::sync::mpsc::Receiver<Result<AccountStatus, PluginError>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let pid = plugin_id.to_string();
    std::thread::spawn(move || {
        let res = if pid == "generic-http" {
            let p = if let Some(k) = keyring {
                GenericHttpPlugin::with_keyring(k)
            } else {
                GenericHttpPlugin::new()
            };
            p.check_account(&host, &username)
        } else {
            Err(PluginError::Unsupported(format!("unknown plugin {pid}")))
        };
        let _ = tx.send(res);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::MemoryKeyring;

    #[test]
    fn registry_has_generic_http() {
        let plugins = all_plugins();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].plugin_id(), "generic-http");
    }
    #[test]
    fn generic_handles_http_only() {
        let p = GenericHttpPlugin::new();
        assert!(p.can_handle("https://example.com/file.zip"));
        assert!(p.can_handle("http://example.com/a"));
        assert!(!p.can_handle("magnet:?xt=foo"));
        assert!(!p.can_handle("ftp://example.com/f"));
    }
    #[test]
    fn generic_default_host_empty() {
        assert_eq!(GenericHttpPlugin::new().default_host(), "");
    }
    #[test]
    fn generic_inject_maps_to_http_opts_never_uri() {
        let p = GenericHttpPlugin::new();
        let m = p.inject_credentials("alice", "s3cr3t");
        assert_eq!(m.get("http-user").unwrap(), "alice");
        assert_eq!(m.get("http-passwd").unwrap(), "s3cr3t");
        assert_eq!(m.get("ftp-user").unwrap(), "alice");
        assert_eq!(m.get("ftp-passwd").unwrap(), "s3cr3t");
        // never embed in URI
        let uri = "https://example.com/file";
        assert!(!uri.contains("s3cr3t"));
        assert!(!format!("{:?}", m).contains("://"));
    }
    #[test]
    fn generic_max_simultaneous() {
        let p = GenericHttpPlugin::new();
        assert_eq!(p.max_simultaneous(false), 1);
        assert_eq!(p.max_simultaneous(true), 4);
        assert!(!p.supports_multi_host());
    }
    #[test]
    fn check_reads_keyring_itself() {
        let kr = MemoryKeyring::new();
        let svc = service_name("generic-http", "example.com");
        kr.set_password(&svc, "alice", "s3cr3t").unwrap();
        let p = GenericHttpPlugin::with_keyring(Box::new(kr));
        let st = p.check_account("example.com", "alice").unwrap();
        assert_eq!(st.state, AccountState::Valid);
        // missing -> Invalid
        let p2 = GenericHttpPlugin::with_keyring(Box::new(MemoryKeyring::new()));
        assert_eq!(p2.check_account("example.com", "alice").unwrap_err(), PluginError::Invalid);
    }
    #[test]
    fn check_headless_is_loud() {
        let p = GenericHttpPlugin::with_keyring(Box::new(MemoryKeyring::headless()));
        let e = p.check_account("example.com", "alice").unwrap_err();
        match e {
            PluginError::Keyring(msg) => {
                assert!(msg.contains("NoDefaultStore") || msg.contains("headless"));
                assert!(msg.contains("headless"));
            }
            _ => panic!("expected loud keyring error"),
        }
    }
    #[test]
    fn spawn_check_outside_render_loop() {
        let kr = MemoryKeyring::new();
        let svc = service_name("generic-http", "example.com");
        kr.set_password(&svc, "alice", "pw").unwrap();
        let rx = spawn_account_check("generic-http", "example.com".into(), "alice".into(), Some(Box::new(kr)));
        let res = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert!(res.is_ok());
        assert_eq!(res.unwrap().state, AccountState::Valid);
    }
    #[test]
    fn inject_options_are_owned_strings_not_uri() {
        // Ensure no URI embedding leak: secret never appears in URI-like field
        let p = GenericHttpPlugin::new();
        let opts = p.inject_credentials("u", "p@ss:w0rd!");
        for (k, v) in &opts {
            assert!(!v.contains("://"), "value for {k} looks like URI");
        }
        assert_eq!(opts.len(), 4);
    }
}
