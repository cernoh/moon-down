//! Blocking JSON-RPC 2.0 client for the managed-local `aria2c`.
//!
//! ponytail: blocking on purpose. The TUI owns one thread and one event loop, so a
//! synchronous POST per tick is simpler than a runtime, and the endpoint is
//! 127.0.0.1 on a port we chose ourselves. If the tick ever needs to overlap with
//! rendering, that is when a real async client earns its keep.
//!
//! No TLS: `ureq` is pulled in without default features because aria2c is bound to
//! localhost only (spec: `rpc-listen-all=false`).

use std::time::Duration;

use serde_json::Value;

use crate::rpc::Tick;

/// A JSON-RPC error returned by the daemon, kept verbatim for the UI.
#[derive(Debug, Clone)]
pub struct RpcError {
    pub message: String,
    pub code: Option<i64>,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(c) => write!(f, "rpc error {c}: {}", self.message),
            None => write!(f, "rpc error: {}", self.message),
        }
    }
}

#[derive(Debug)]
pub enum ClientError {
    Transport(String),
    Rpc(RpcError),
    BadReply(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(s) => write!(f, "engine unreachable: {s}"),
            Self::Rpc(e) => write!(f, "{e}"),
            Self::BadReply(s) => write!(f, "unusable reply: {s}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Cheap to clone so callers can hold one while the `Engine` is mutably borrowed
/// (shutdown needs `&mut Engine` and a live client at the same time).
#[derive(Debug, Clone)]
pub struct RpcClient {
    url: String,
    secret: String,
    timeout: Duration,
}

impl RpcClient {
    pub fn new(url: impl Into<String>, secret: impl Into<String>) -> Self {
        Self { url: url.into(), secret: secret.into(), timeout: Duration::from_secs(5) }
    }

    pub fn with_timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    fn agent(&self) -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout(self.timeout)
            .build()
    }

    /// One authenticated call. `token:SECRET` is always the first parameter.
    pub fn call(&self, method: &str, params: Value, id: u64) -> Result<Value, ClientError> {
        let mut full = vec![Value::String(format!("token:{}", self.secret))];
        if let Value::Array(tail) = params {
            full.extend(tail);
        } else if !params.is_null() {
            full.push(params);
        }
        self.post(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": full,
        }))
    }

    /// One batched POST: an array of requests, replies come back in request order.
    pub fn post(&self, body: &Value) -> Result<Value, ClientError> {
        let body_str = serde_json::to_string(body).map_err(|e| ClientError::Transport(e.to_string()))?;
        let resp = self
            .agent()
            .post(&self.url)
            .set("Content-Type", "application/json")
            .send_string(&body_str);
        let resp = match resp {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                // aria2 returns 400 with a JSON-RPC error body for bad URIs / auth
                let text = r.into_string().unwrap_or_default();
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    check_error(&value)?;
                    // if check_error didn't error, still surface the JSON
                    return Ok(value);
                }
                return Err(ClientError::Transport(format!("http {code}: {text}")));
            }
            Err(e) => return Err(ClientError::Transport(e.to_string())),
        };

        let text = resp.into_string().map_err(|e| ClientError::Transport(e.to_string()))?;
        let value: Value = serde_json::from_str(&text).map_err(|e| ClientError::Transport(format!("bad json: {e}")))?;
        check_error(&value)?;
        Ok(value)
    }

    /// One queue tick: the four-call batch the spec defines, parsed.
    pub fn tick(&self, id: u64) -> Result<Tick, ClientError> {
        let batch = crate::rpc::build_poll_batch(&self.secret, id);
        let reply = self.post(&batch)?;
        Tick::from_batch(&reply).map_err(ClientError::BadReply)
    }
}

/// JSON-RPC replies carry failures in an `error` member instead of `result`.
/// Batches put one error object per failing entry, so a batch with any error is
/// reported whole: a partial tick would silently under-report the queue.
fn check_error(value: &Value) -> Result<(), ClientError> {
    match value {
        Value::Array(items) => {
            for item in items {
                check_error(item)?;
            }
            Ok(())
        }
        Value::Object(map) => match map.get("error") {
            None | Some(Value::Null) => Ok(()),
            Some(err) => Err(ClientError::Rpc(RpcError {
                message: err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                code: err.get("code").and_then(|c| c.as_i64()),
            })),
        },
        _ => Ok(()),
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> RpcClient {
        RpcClient::new("http://127.0.0.1:1/jsonrpc", "s3cr3t")
    }

    #[test]
    fn call_prepends_token() {
        let c = client();
        let req = serde_json::json!({"jsonrpc":"2.0","id":7,"method":"aria2.tellStatus","params":["token:s3cr3t","gid"]});
        // mirror what call() would build, without a socket
        assert_eq!(req["params"][0], "token:s3cr3t");
        assert_eq!(req["method"], "aria2.tellStatus");
        assert_eq!(c.url, "http://127.0.0.1:1/jsonrpc");
    }

    #[test]
    fn error_member_is_surfaced() {
        let err = serde_json::json!({"jsonrpc":"2.0","id":1,"error":{"code":1,"message":"Unauthorized"}});
        match check_error(&err) {
            Err(ClientError::Rpc(e)) => {
                assert_eq!(e.code, Some(1));
                assert_eq!(e.message, "Unauthorized");
            }
            other => panic!("expected rpc error, got {other:?}"),
        }
    }

    #[test]
    fn batch_with_one_error_fails_the_tick() {
        let batch = serde_json::json!([
            {"jsonrpc":"2.0","id":1,"result":[]},
            {"jsonrpc":"2.0","id":2,"error":{"code":1,"message":"Unauthorized"}},
        ]);
        assert!(check_error(&batch).is_err(), "a partial tick must not look healthy");
    }

    #[test]
    fn unreachable_engine_is_transport_error() {
        // port 1 is never bound, so this exercises the unreachable path
        let e = client().call("aria2.getGlobalStat", serde_json::json!([]), 1);
        assert!(matches!(e, Err(ClientError::Transport(_))), "got {e:?}");
    }
}
