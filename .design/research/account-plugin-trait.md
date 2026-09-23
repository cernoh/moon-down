# Native account-plugin trait contract (ticket #9)

Gating: #6 (JD concept map, mirror `e9d5051`) and #5 (keyring flow) both CLOSED.
Scope: signatures only, no implementation. Repo has no `src/` yet; this file is the
contract the build tickets code against.

## Glossary (domain-modeling)

- **Plugin**: one `AccountPlugin` impl (e.g. `generic-http`). Owns URL matching,
  credential validation, and credential-to-aria2-option mapping for the hosts it claims.
- **Host**: a hostname an account belongs to (e.g. `example.com`). Tiers and passwords
  differ per host, so accounts are per-host even when one plugin claims many hosts.
- **Account**: `{plugin, host, username, enabled}` in app config + exactly one secret
  (password/token) in the OS keyring under service `moon-down:{plugin}:{host}`,
  user = username (#5 §4). Config never holds secrets; the session file holds GIDs/paths only.

## v1 trait (original Rust, no ported code)

```rust
// ponytail: account-only surface; checking/linking files is aria2c's job,
// decrypters/wait/CAPTCHA stay fog (map Not-yet-specified).
use std::collections::HashMap;
use std::time::SystemTime;

pub struct AccountRecord {
    pub plugin: String,   // plugin_id, e.g. "generic-http"
    pub host: String,     // hostname only; secrets live in the keyring
    pub username: String,
    pub enabled: bool,    // per-account kill switch, checked before any plugin call
}

pub enum AccountType { Free, Premium, Lifetime, Unknown }
pub enum AccountState { Unchecked, Valid, Expired, Invalid, TempDisabled }

pub struct AccountStatus {
    pub state: AccountState,
    pub account_type: AccountType,
    pub valid_until: Option<SystemTime>,
    pub traffic_left: Option<u64>, // None = unlimited/unknown
}

pub enum PluginError { Invalid, TempDisabled, Network, Unsupported }

pub trait AccountPlugin: Send + Sync {
    fn plugin_id(&self) -> &str;
    /// Prefill for the add-dialog host field (#5 §4 UX).
    fn default_host(&self) -> &str;
    /// Link eligibility: true when this plugin handles `url`.
    fn can_handle(&self, url: &str) -> bool;
    /// Validate stored credentials, refresh session state.
    /// Reads the secret from the keyring itself
    /// (`moon-down:{plugin_id}:{host}` / username) — no secret crosses this call.
    async fn check_account(&self, host: &str, username: &str) -> Result<AccountStatus, PluginError>;
    /// Map a keyring secret to per-download `addUri` options (#5 §3:
    /// `http-user`/`http-passwd`, never URI embedding). The caller zeroes the
    /// secret buffer after the call; the map holds only owned option strings.
    fn inject_credentials(&self, username: &str, secret: &str) -> HashMap<String, String>;
    fn max_simultaneous(&self, premium: bool) -> u32;
    fn supports_multi_host(&self) -> bool { false }
}
```

Notes:

- `check_account` takes `host` + `username` (not just `username` as in the #6 sketch):
  one plugin may claim several hosts and keyring keys are per `{plugin}:{host}`.
- `inject_credentials` is the only call a secret crosses, as `&str`, owned into the
  options map at `addUri` time; secrets never enter `tellStatus`/`getFiles` render
  paths (#5 §3 leak matrix).
- No traffic/multihost-list surface: v1 has no per-account traffic UI.

## Registration / discovery: compiled-in static registry

```rust
pub fn all_plugins() -> Vec<&'static dyn AccountPlugin>;
```

One registry module owns the list; adding a plugin is one struct + one vec line.
No dynamic `.so` loading in v1 (ABI/versioning burden, zero requesters).
Third-party path (settled round 3): one-file `impl AccountPlugin` + one registry
line + a worked docs example; `inventory`-style auto-registration waits for the
second real plugin.

## Per-host enablement: per-account flag in config

`AccountRecord.enabled`, checked by the controller before `check_account`/`inject`.
Covers premium-on-A / free-on-B; no plugin-global toggle in v1.

## Future hooks: none reserved

No wait/captcha/decrypter stubs in the v1 trait — uncallable defaulted methods rot
and guess signatures wrong. Fog stays in the map's Not-yet-specified list; fog
tickets extend the trait when they land.

## v1 built-ins: generic HTTP-auth reference plugin only

- `plugin_id = "generic-http"`, `default_host = ""` (user fills in).
- `can_handle`: true for `http(s)://` URLs.
- `check_account`: lightweight authenticated probe; `Invalid` on 401/403.
- `inject_credentials`: `{http-user, http-passwd, ftp-user, ftp-passwd}` from the
  same pair (strictly dominates URI embedding, #5 §3).
- `max_simultaneous`: 1 free / 4 premium (aria2-side caps stay global per #12).
- No named premium hosters in v1: no ticket measured any hoster's auth flow, so any
  name would be a guess. First real hoster extends this file's table, not the trait.

## Counting basis

No code surface to measure: repo has no `src/` (tracked files: `.gitignore`,
`AGENTS.md`, `LICENSE`). Gating claims verified by read: #6 sketch
(`.design/research/jd-concepts.md:135-141`), #5 key shape + leak matrix
(`.design/research/keyring.md:136-139`, `:117-132`).
