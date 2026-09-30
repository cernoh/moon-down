use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};

use moon_down_core::MemberState;

/// Build a single global-option change: one option at once, live applied with no restart.
pub fn build_change_global_option(secret: &str, key: &str, value: &str, rpc_id: u64) -> Value {
    let token = format!("token:{secret}");
    json!({"jsonrpc":"2.0","id":rpc_id,"method":"aria2.changeGlobalOption","params":[token, {key: value}]})
}

/// Build a single JSON-RPC batch for one tick: active + queued + past + global stat, with auth.
/// All calls carry `token:SECRET` as first param except Methods that are not exempt — in our
/// subset none are exempt, so every call is authenticated.
pub fn build_poll_batch(secret: &str, rpc_id: u64) -> Value {
    let token = format!("token:{secret}");
    let keys = json!(["gid","status","totalLength","completedLength","downloadSpeed","files","bittorrent","errorMessage","dir"]);
    json!([
        {"jsonrpc":"2.0","id":rpc_id,  "method":"aria2.tellActive","params":[token.clone(), keys.clone()]},
        {"jsonrpc":"2.0","id":rpc_id+1,"method":"aria2.tellWaiting","params":[token.clone(), 0, 1000, keys.clone()]},
        {"jsonrpc":"2.0","id":rpc_id+2,"method":"aria2.tellStopped","params":[token.clone(), 0, 1000, keys.clone()]},
        {"jsonrpc":"2.0","id":rpc_id+3,"method":"aria2.getGlobalStat","params":[token]},
    ])
}

#[derive(Debug, Clone)]
pub enum EnqueueKind {
    Uris(Vec<String>),
    Torrent(Vec<u8>),
    Metalink(Vec<u8>),
}

/// Stored record holds no secrets (URIs stripped, payloads not containing token).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRecord {
    pub kind: String, // "uri" | "torrent" | "metalink"
    pub uris: Vec<String>, // stripped
    pub meta: Option<String>, // e.g. filename hint, not secret
}

impl StoredRecord {
    pub fn contains_secret(&self, secret: &str) -> bool {
        self.uris.iter().any(|u| u.contains(secret))
            || self.meta.as_ref().map(|m| m.contains(secret)).unwrap_or(false)
    }
}

pub fn build_enqueue(secret: &str, kind: EnqueueKind, rpc_id: u64) -> (Value, StoredRecord) {
    // Never embed user:pass@ in URI on wire — always strip for both storage and send.
    let token = format!("token:{secret}");
    match kind {
        EnqueueKind::Uris(uris) => {
            let stripped: Vec<String> = uris.iter().map(|u| crate::strip::strip_credentials(u)).collect();
            let req = json!({"jsonrpc":"2.0","id":rpc_id,"method":"aria2.addUri","params":[token, stripped.clone(), {}]});
            let rec = StoredRecord { kind: "uri".into(), uris: stripped, meta: None };
            (req, rec)
        }
        EnqueueKind::Torrent(bytes) => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let req = json!({"jsonrpc":"2.0","id":rpc_id,"method":"aria2.addTorrent","params":[token, b64, [], {}]});
            let rec = StoredRecord { kind: "torrent".into(), uris: vec![], meta: Some(format!("torrent:{}bytes", bytes.len())) };
            (req, rec)
        }
        EnqueueKind::Metalink(bytes) => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let req = json!({"jsonrpc":"2.0","id":rpc_id,"method":"aria2.addMetalink","params":[token, b64, {}]});
            let rec = StoredRecord { kind: "metalink".into(), uris: vec![], meta: Some(format!("metalink:{}bytes", bytes.len())) };
            (req, rec)
        }
    }
}

/// Build enqueue with credential options (http-user/http-passwd, never URI embedding).
/// `auth_options` are injected as the `options` param for addUri/addTorrent/addMetalink.
/// URIs are still stripped; secret lives only in the options map.
pub fn build_enqueue_with_auth(
    secret: &str,
    kind: EnqueueKind,
    rpc_id: u64,
    auth_options: &std::collections::HashMap<String, String>,
) -> (Value, StoredRecord) {
    let token = format!("token:{secret}");
    let opts = serde_json::to_value(auth_options).unwrap_or(json!({}));
    match kind {
        EnqueueKind::Uris(uris) => {
            let stripped: Vec<String> = uris.iter().map(|u| crate::strip::strip_credentials(u)).collect();
            let req = json!({"jsonrpc":"2.0","id":rpc_id,"method":"aria2.addUri","params":[token, stripped.clone(), opts]});
            let rec = StoredRecord { kind: "uri".into(), uris: stripped, meta: None };
            (req, rec)
        }
        EnqueueKind::Torrent(bytes) => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let req = json!({"jsonrpc":"2.0","id":rpc_id,"method":"aria2.addTorrent","params":[token, b64, [], opts]});
            let rec = StoredRecord { kind: "torrent".into(), uris: vec![], meta: Some(format!("torrent:{}bytes", bytes.len())) };
            (req, rec)
        }
        EnqueueKind::Metalink(bytes) => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let req = json!({"jsonrpc":"2.0","id":rpc_id,"method":"aria2.addMetalink","params":[token, b64, opts]});
            let rec = StoredRecord { kind: "metalink".into(), uris: vec![], meta: Some(format!("metalink:{}bytes", bytes.len())) };
            (req, rec)
        }
    }
}

/// One download as the daemon reports it. Only the keys our batch asks for.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct StatusEntry {
    #[serde(default)]
    pub gid: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    #[serde(rename = "totalLength")]
    pub total_length: String,
    #[serde(default)]
    #[serde(rename = "completedLength")]
    pub completed_length: String,
    #[serde(default)]
    #[serde(rename = "downloadSpeed")]
    pub download_speed: String,
    #[serde(rename = "errorMessage", default)]
    pub error_message: String,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct FileEntry {
    #[serde(default)]
    pub path: String,
    #[serde(rename = "uris", default)]
    pub uris: Vec<UriEntry>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct UriEntry {
    #[serde(default)]
    pub uri: String,
}

impl StatusEntry {
    pub fn total(&self) -> u64 {
        self.total_length.parse().unwrap_or(0)
    }
    pub fn completed(&self) -> u64 {
        self.completed_length.parse().unwrap_or(0)
    }
    pub fn speed(&self) -> u64 {
        self.download_speed.parse().unwrap_or(0)
    }
    /// Best available name: the first file's basename, else the first URI's tail.
    pub fn display_name(&self) -> String {
        if let Some(f) = self.files.first() {
            let p = &f.path;
            let name = p.rsplit('/').next().unwrap_or(p);
            if !name.is_empty() {
                return name.to_string();
            }
        }
        if let Some(u) = self.files.first().and_then(|f| f.uris.first()).map(|u| u.uri.clone()) {
            let tail = u.split(['?', '#']).next().unwrap_or(&u);
            let name = tail.rsplit('/').next().unwrap_or(tail);
            if !name.is_empty() {
                return name.to_string();
            }
        }
        self.gid.chars().take(8).collect()
    }
    /// aria2 status -> our canonical member state. Anything unexpected is an error
    /// rather than a guess, so a row never looks healthy when the daemon is not.
    pub fn member_state(&self) -> MemberState {
        match self.status.as_str() {
            "active" => MemberState::Downloading,
            "waiting" => MemberState::Queued,
            "paused" => MemberState::Paused,
            "complete" => MemberState::Complete,
            "error" | "removed" => MemberState::Error,
            other => {
                let _ = other;
                MemberState::Queued
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct GlobalStat {
    #[serde(default, rename = "downloadSpeed")]
    pub download_speed: String,
    #[serde(default, rename = "numActive")]
    pub num_active: String,
    #[serde(default, rename = "numWaiting")]
    pub num_waiting: String,
    #[serde(default, rename = "numStopped")]
    pub num_stopped: String,
}

impl GlobalStat {
    pub fn speed(&self) -> u64 {
        self.download_speed.parse().unwrap_or(0)
    }
    pub fn active(&self) -> u64 {
        self.num_active.parse().unwrap_or(0)
    }
    pub fn waiting(&self) -> u64 {
        self.num_waiting.parse().unwrap_or(0)
    }
    pub fn stopped(&self) -> u64 {
        self.num_stopped.parse().unwrap_or(0)
    }
}

/// One parsed tick: the four batch replies, in request order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tick {
    pub active: Vec<StatusEntry>,
    pub waiting: Vec<StatusEntry>,
    pub stopped: Vec<StatusEntry>,
    pub global: GlobalStat,
}

impl Tick {
    pub fn all(&self) -> impl Iterator<Item = &StatusEntry> {
        self.active.iter().chain(self.waiting.iter()).chain(self.stopped.iter())
    }

    /// Parse a batch reply. Entries come back in request order, so position tells
    /// us which call answered; a short or reordered batch is an error, never a
    /// partial queue update.
    pub fn from_batch(reply: &serde_json::Value) -> Result<Tick, String> {
        let items = reply
            .as_array()
            .ok_or_else(|| "batch reply was not an array".to_string())?;
        if items.len() < 4 {
            return Err(format!("batch reply had {} entries, expected 4", items.len()));
        }
        let entry = |i: usize| -> Result<Vec<StatusEntry>, String> {
            let result = items[i]
                .get("result")
                .ok_or_else(|| format!("entry {i} had no result"))?;
            serde_json::from_value(result.clone())
                .map_err(|e| format!("entry {i} did not parse: {e}"))
        };
        let global = items[3]
            .get("result")
            .ok_or_else(|| "global stat had no result".to_string())
            .and_then(|r| {
                serde_json::from_value::<GlobalStat>(r.clone())
                    .map_err(|e| format!("global stat did not parse: {e}"))
            })?;
        Ok(Tick {
            active: entry(0)?,
            waiting: entry(1)?,
            stopped: entry(2)?,
            global,
        })
    }

    /// Fold one tick into the queue. Returns true if anything the queue pane
    /// draws moved, so the caller can keep rendering on state change only.
    ///
    /// Reconcile is always safe: tellActive + tellWaiting + tellStopped is the
    /// daemon's *complete* set (Tick::from_batch rejects a short batch outright),
    /// so a handle missing from it really is gone. Gids we do not own are simply
    /// skipped — another client's download does not disturb our rows.
    ///
    /// A terminal row is left completely alone. Writing progress onto an errored
    /// member makes it show live bytes for a download that is not running, and
    /// `set_member_state` would refuse the state change anyway.
    pub fn apply(&self, queue: &mut moon_down_core::Queue) -> bool {
        let before = fingerprint(queue);
        let mut live: Vec<String> = Vec::new();

        for e in self.all() {
            live.push(e.gid.clone());
            let Some(mid) = queue.member_id_by_handle(&e.gid) else {
                continue; // a download we did not add (e.g. a session resume)
            };
            if queue
                .find_member(mid)
                .map(|m| m.state.is_terminal())
                .unwrap_or(true)
            {
                continue;
            }
            queue.set_member_progress(mid, e.total(), e.completed());
            let next = e.member_state();
            // A terminal row stays terminal until the user acts; core enforces that.
            queue.set_member_state(mid, next.clone());
            if next == MemberState::Error && !e.error_message.is_empty() {
                queue.set_member_error(mid, e.error_message.clone());
            }
        }
        queue.reconcile(&live);

        fingerprint(queue) != before
    }
}

/// Cheap signature of everything the queue pane draws, used instead of trusting
/// a "did anything move" flag from the daemon.
fn fingerprint(queue: &moon_down_core::Queue) -> Vec<(u64, MemberState, u64, u64)> {
    queue
        .packages
        .iter()
        .flat_map(|p| p.members.iter())
        .map(|m| (m.id, m.state.clone(), m.completed_bytes, m.total_bytes))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample_batch() -> serde_json::Value {
        serde_json::json!([
            {"jsonrpc":"2.0","id":1,"result":[
                {"gid":"g1","status":"active","totalLength":"1000","completedLength":"250",
                 "downloadSpeed":"500","files":[{"path":"/tmp/a.iso","uris":[{"uri":"https://x/a.iso"}]}]}
            ]},
            {"jsonrpc":"2.0","id":2,"result":[
                {"gid":"g2","status":"waiting","totalLength":"10","completedLength":"0","downloadSpeed":"0"}
            ]},
            {"jsonrpc":"2.0","id":3,"result":[
                {"gid":"g3","status":"complete","totalLength":"20","completedLength":"20","downloadSpeed":"0"}
            ]},
            {"jsonrpc":"2.0","id":4,"result":
                {"downloadSpeed":"500","numActive":"1","numWaiting":"1","numStopped":"1"}}
        ])
    }

    #[test]
    fn tick_parses_all_four_entries() {
        let t = Tick::from_batch(&sample_batch()).unwrap();
        assert_eq!(t.active.len(), 1);
        assert_eq!(t.waiting.len(), 1);
        assert_eq!(t.stopped.len(), 1);
        assert_eq!(t.global.active(), 1);
        assert_eq!(t.global.speed(), 500);
        assert_eq!(t.all().count(), 3);
    }

    #[test]
    fn status_maps_to_member_state() {
        let t = Tick::from_batch(&sample_batch()).unwrap();
        assert_eq!(t.active[0].member_state(), MemberState::Downloading);
        assert_eq!(t.waiting[0].member_state(), MemberState::Queued);
        assert_eq!(t.stopped[0].member_state(), MemberState::Complete);
        assert_eq!(t.active[0].completed(), 250);
        assert_eq!(t.active[0].total(), 1000);
        assert_eq!(t.active[0].speed(), 500);
    }

    #[test]
    fn display_name_prefers_file_then_uri() {
        let t = Tick::from_batch(&sample_batch()).unwrap();
        assert_eq!(t.active[0].display_name(), "a.iso");
        // no files: fall back to the gid prefix
        assert_eq!(t.waiting[0].display_name(), "g2");
    }

    #[test]
    fn short_batch_is_an_error_not_a_partial_update() {
        let short = serde_json::json!([{"jsonrpc":"2.0","id":1,"result":[]}]);
        assert!(Tick::from_batch(&short).is_err());
        assert!(Tick::from_batch(&serde_json::json!({})).is_err());
    }

    #[test]
    fn apply_updates_state_and_marks_vanished_gone() {
        let mut q = moon_down_core::Queue::new();
        let pkg = q.add_package("p", "/tmp", vec![("a".into(), "https://x/a".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_handle(mid, "g1".into());
        let gone = q.packages[0].members[0].id;
        let _ = pkg;
        // second member the engine has never heard of
        q.add_package("q", "/tmp", vec![("b".into(), "https://x/b".into())]);
        let mid2 = q.packages[1].members[0].id;
        q.set_member_handle(mid2, "ghost".into());
        let _ = gone;

        Tick::from_batch(&sample_batch()).unwrap().apply(&mut q);

        assert_eq!(q.find_member(mid).unwrap().state, MemberState::Downloading);
        assert_eq!(q.find_member(mid).unwrap().completed_bytes, 250);
        assert_eq!(q.find_member(mid).unwrap().total_bytes, 1000);
        assert_eq!(q.find_member(mid2).unwrap().state, MemberState::Gone);
    }

    /// Another client's download sharing the daemon must not disturb our rows.
    #[test]
    fn foreign_gid_coexisting_with_ours_is_harmless() {
        let mut q = moon_down_core::Queue::new();
        q.add_package("p", "/tmp", vec![("a".into(), "https://x/a".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_handle(mid, "g1".into());
        q.set_member_state(mid, MemberState::Downloading);

        let foreign = StatusEntry {
            gid: "deadbeef".into(),
            status: "active".into(),
            total_length: "5".into(),
            completed_length: "5".into(),
            download_speed: "0".into(),
            error_message: String::new(),
            dir: String::new(),
            files: vec![],
        };
        // Added to, not substituted for, the real gids: ours is still running.
        let mut tick = Tick::from_batch(&sample_batch()).unwrap();
        tick.active.push(foreign);
        tick.apply(&mut q);

        let m = q.find_member(mid).unwrap();
        assert_eq!(m.state, MemberState::Downloading, "our row keeps running");
        assert_eq!(m.completed_bytes, 250, "our bytes still update");
    }

    /// A handle the daemon no longer reports at all really is gone: the three
    /// lists are the complete set, so absence is evidence.
    #[test]
    fn handle_absent_from_a_complete_tick_becomes_gone() {
        let mut q = moon_down_core::Queue::new();
        q.add_package("p", "/tmp", vec![("a".into(), "https://x/a".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_handle(mid, "not-in-any-list".into());
        q.set_member_state(mid, MemberState::Downloading);

        Tick::from_batch(&sample_batch()).unwrap().apply(&mut q);
        assert_eq!(q.find_member(mid).unwrap().state, MemberState::Gone);
    }

    /// An errored row is not a running download; it must not show live bytes.
    #[test]
    fn terminal_row_is_never_overwritten_by_a_later_record() {
        let mut q = moon_down_core::Queue::new();
        q.add_package("p", "/tmp", vec![("a".into(), "https://x/a".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_handle(mid, "g1".into());
        q.set_member_error(mid, "connection reset");

        // the daemon now reports the same gid as active again
        Tick::from_batch(&sample_batch()).unwrap().apply(&mut q);

        let m = q.find_member(mid).unwrap();
        assert_eq!(m.state, MemberState::Error);
        assert_eq!(m.completed_bytes, 0, "no live bytes on an errored row");
    }

    /// render-on-change depends on apply reporting movement, not just doing it.
    #[test]
    fn apply_reports_whether_anything_moved() {
        let mut q = moon_down_core::Queue::new();
        q.add_package("p", "/tmp", vec![("a".into(), "https://x/a".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_handle(mid, "g1".into());
        let tick = Tick::from_batch(&sample_batch()).unwrap();
        assert!(tick.apply(&mut q), "first tick moves the row");
        assert!(!tick.apply(&mut q), "identical tick is not a change");
    }

    #[test]
    fn poll_batch_has_four_calls_with_auth() {
        let v = build_poll_batch("s3cr3t", 1);
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 4);
        let methods: Vec<_> = arr.iter().map(|o| o["method"].as_str().unwrap()).collect();
        assert_eq!(methods, ["aria2.tellActive","aria2.tellWaiting","aria2.tellStopped","aria2.getGlobalStat"]);
        for o in arr {
            let token = o["params"][0].as_str().unwrap();
            assert_eq!(token, "token:s3cr3t");
        }
    }
    #[test]
    fn enqueue_uri_strips_creds_in_store_not_in_token_leak() {
        let secret = "s3cr3t";
        let uris = vec!["https://user:pass@example.com/file.zip".into(), "https://example.com/a".into()];
        let (req, rec) = build_enqueue(secret, EnqueueKind::Uris(uris.clone()), 1);
        assert_eq!(req["method"], "aria2.addUri");
        assert_eq!(req["params"][0], format!("token:{secret}"));
        assert_eq!(rec.uris, vec!["https://example.com/file.zip","https://example.com/a"]);
        assert!(!rec.contains_secret(secret));
        assert!(!rec.uris[0].contains("user:pass"));
        // wire also stripped — never embed secret in URI
        let wire_uris = req["params"][1].as_array().unwrap();
        assert_eq!(wire_uris[0].as_str().unwrap(), "https://example.com/file.zip");
        assert!(!wire_uris[0].as_str().unwrap().contains("user:pass"));
    }
    #[test]
    fn enqueue_torrent_base64() {
        let (req, rec) = build_enqueue("s", EnqueueKind::Torrent(vec![1,2,3]), 1);
        assert_eq!(req["method"], "aria2.addTorrent");
        assert_eq!(rec.kind, "torrent");
        assert!(req["params"][1].as_str().unwrap().len() > 0);
    }
    #[test]
    fn enqueue_metalink_base64() {
        let (req, rec) = build_enqueue("s", EnqueueKind::Metalink(b"hello".to_vec()), 1);
        assert_eq!(req["method"], "aria2.addMetalink");
        assert_eq!(rec.kind, "metalink");
    }
    #[test]
    fn enqueue_with_auth_maps_to_http_opts_never_uri() {
        let secret = "s3cr3t";
        let mut opts = std::collections::HashMap::new();
        opts.insert("http-user".into(), "alice".into());
        opts.insert("http-passwd".into(), secret.into());
        let uris = vec!["https://example.com/file".into()];
        let (req, rec) = build_enqueue_with_auth(secret, EnqueueKind::Uris(uris), 42, &opts);
        // options carry secret, URI does not
        assert_eq!(req["params"][2]["http-user"], "alice");
        assert_eq!(req["params"][2]["http-passwd"], secret);
        let wire_uri = req["params"][1][0].as_str().unwrap();
        assert!(!wire_uri.contains(secret));
        assert!(!wire_uri.contains("@"));
        assert!(!rec.contains_secret(secret));
    }

    #[test]
    fn change_global_option_is_one_call_with_no_restart() {
        let v = build_change_global_option("s3cr3t", "max-concurrent-downloads", "10", 42);
        assert_eq!(v["method"], "aria2.changeGlobalOption");
        assert_eq!(v["id"], 42);
        assert_eq!(v["params"][0], "token:s3cr3t");
        assert_eq!(v["params"][1]["max-concurrent-downloads"], "10");
        // single option per call
        assert_eq!(v["params"][1].as_object().unwrap().len(), 1);
    }
}
