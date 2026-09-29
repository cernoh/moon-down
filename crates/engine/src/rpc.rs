use base64::Engine as _;
use serde_json::{json, Value};

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

#[cfg(test)]
mod tests {
    use super::*;
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
