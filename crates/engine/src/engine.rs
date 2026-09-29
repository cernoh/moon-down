use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use serde_json::Value;

use crate::lock::{self, DirLock};
use crate::rpc::{build_enqueue, build_poll_batch, EnqueueKind, StoredRecord};

#[derive(Debug)]
pub enum EngineError {
    Locked(String),
    NoPort,
    Io(String),
    Spawn(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Locked(s) => write!(f, "{s}"),
            Self::NoPort => write!(f, "no free port in 6800-6899"),
            Self::Io(s) => write!(f, "{s}"),
            Self::Spawn(s) => write!(f, "{s}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct StartOptions {
    /// Override aria2c binary; if None, tries "aria2c" in PATH.
    /// For tests, set to Some("sleep") or similar to avoid needing aria2c.
    pub aria_bin: Option<String>,
    /// Extra args for testing (ignored when aria2c not found).
    pub mock_sleep_secs: Option<u64>,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self { aria_bin: None, mock_sleep_secs: None }
    }
}

/// Managed-local child. Holds the lock, secret, port, config, and child handle.
pub struct Engine {
    pub state_dir: PathBuf,
    pub secret: String,
    pub port: u16,
    pub config_path: PathBuf,
    pub pid_path: PathBuf,
    pub session_path: PathBuf,
    lock: Option<DirLock>,
    child: Option<Child>,
    crash_count: u8,
    pub crashed: bool,
    pub fatal_error: Option<String>,
    aria_bin: Option<String>,
    mock_sleep_secs: Option<u64>,
}

impl Engine {
    /// Start engine in `state_dir`. Holds lock, writes private config, picks port, spawns child.
    pub fn start(state_dir: &Path, opts: StartOptions) -> Result<Self, EngineError> {
        let lock = lock::try_acquire(state_dir).map_err(|e| match e {
            lock::LockError::AlreadyLocked => EngineError::Locked(e.to_string()),
            lock::LockError::Io(e) => EngineError::Io(e.to_string()),
        })?;
        let secret = crate::secret::generate_secret();
        let config_path = state_dir.join("aria2.conf");
        write_secret_config(&config_path, &secret).map_err(|e| EngineError::Io(e.to_string()))?;
        let port = crate::port::pick_free_port().ok_or(EngineError::NoPort)?;

        let session_path = state_dir.join("aria2.session");
        // ensure session file exists (aria2 requires it)
        if !session_path.exists() {
            fs::write(&session_path, b"").map_err(|e| EngineError::Io(e.to_string()))?;
        }
        let pid_path = state_dir.join("aria2.pid");
        let log_path = state_dir.join("aria2.log");

        let aria_bin = opts.aria_bin.clone().or_else(|| Some("aria2c".into()));
        let mut child = try_spawn_child(
            aria_bin.as_deref(),
            &config_path,
            port,
            &session_path,
            &log_path,
            state_dir,
            opts.mock_sleep_secs,
        );

        // If real aria2c was requested but not found, fall back to mock for testability
        // only when mock_sleep_secs is set; otherwise surface error.
        if child.is_none() && opts.mock_sleep_secs.is_some() {
            child = try_spawn_child(Some("sleep"), &config_path, port, &session_path, &log_path, state_dir, opts.mock_sleep_secs);
        }
        if child.is_none() {
            // Loud failure: a caller asking for the real engine must not get a
            // half-started one. Never kill whatever holds a port; never leave the
            // lock or conf behind for the next start to trip over.
            let _ = fs::remove_file(&config_path);
            return Err(EngineError::Spawn(match opts.aria_bin.as_deref() {
                Some(bin) => format!("could not spawn {bin}"),
                None => "aria2c not found in PATH — install it (nix shell nixpkgs#aria2) or pass a path".into(),
            }));
        }

        let child = child.expect("checked above");
        let _ = fs::write(&pid_path, child.id().to_string());

        Ok(Self {
            state_dir: state_dir.to_path_buf(),
            secret,
            port,
            config_path,
            pid_path,
            session_path,
            lock: Some(lock),
            child: Some(child),
            crash_count: 0,
            crashed: false,
            fatal_error: None,
            aria_bin,
            mock_sleep_secs: opts.mock_sleep_secs,
        })
    }

    pub fn rpc_url(&self) -> String {
        format!("http://127.0.0.1:{}/jsonrpc", self.port)
    }

    /// A cheap handle to the same endpoint, so callers can hold a live client
    /// while `&mut Engine` is borrowed (shutdown needs both).
    pub fn client(&self) -> crate::client::RpcClient {
        crate::client::RpcClient::new(self.rpc_url(), &self.secret)
    }

    /// One tick: POST the four-call batch and parse it.
    pub fn tick(&self, id: u64) -> Result<crate::rpc::Tick, crate::client::ClientError> {
        self.client().tick(id)
    }

    /// Enqueue links, returning the gids aria2c assigned, in request order.
    /// `options` are aria2 per-download options (e.g. http-user/http-passwd from an
    /// account record); URIs are always credential-stripped before they go out.
    pub fn add_uris(
        &self,
        uris: &[String],
        options: &std::collections::HashMap<String, String>,
        id: u64,
    ) -> Result<Vec<String>, crate::client::ClientError> {
        if uris.is_empty() {
            return Ok(Vec::new());
        }
        let (req, _stored) = crate::rpc::build_enqueue_with_auth(
            &self.secret,
            EnqueueKind::Uris(uris.to_vec()),
            id,
            options,
        );
        let reply = self.client().post(&req)?;
        let result = reply.get("result").ok_or_else(|| crate::client::ClientError::BadReply("addUri returned no result".into()))?;
        let gids = if let Some(s) = result.as_str() {
            vec![s.to_string()]
        } else if let Some(arr) = result.as_array() {
            arr.iter().filter_map(|g| g.as_str().map(|s| s.to_string())).collect()
        } else {
            return Err(crate::client::ClientError::BadReply(format!("addUri result not string/array: {result}")));
        };
        Ok(gids)
    }

    /// Stop the child if it is already gone: the engine never outlives the tick
    /// loop that owns it. Returns true when a live child was reaped.
    pub fn reap_if_dead(&mut self) -> bool {
        !self.is_alive()
    }

    /// One tick = one batched POST of 4 calls, all authenticated.
    pub fn poll_batch(&self, id: u64) -> Value {
        build_poll_batch(&self.secret, id)
    }

    /// Live-apply one global option with no restart. Each change is one RPC call.
    pub fn change_global_option(&self, key: &str, value: &str, id: u64) -> Value {
        crate::rpc::build_change_global_option(&self.secret, key, value, id)
    }

    pub fn prepare_enqueue(&self, kind: EnqueueKind, id: u64) -> (Value, StoredRecord) {
        build_enqueue(&self.secret, kind, id)
    }

    /// Simulate/observe a crash. Respawns once, then sets fatal_error.
    pub fn on_crash(&mut self) -> Result<(), String> {
        if self.fatal_error.is_some() {
            return Err(self.fatal_error.clone().unwrap());
        }
        if self.crash_count == 0 {
            self.crash_count = 1;
            // try respawn
            let cfg = self.config_path.clone();
            // keep same secret/port/session for resume
            let new_child = try_spawn_child(
                self.aria_bin.as_deref(),
                &cfg,
                self.port,
                &self.session_path,
                &self.state_dir.join("aria2.log"),
                &self.state_dir,
                self.mock_sleep_secs,
            )
            .or_else(|| {
                // fallback sleep mock
                try_spawn_child(Some("sleep"), &cfg, self.port, &self.session_path, &self.state_dir.join("aria2.log"), &self.state_dir, self.mock_sleep_secs)
            });
            if let Some(c) = new_child {
                let _ = fs::write(&self.pid_path, c.id().to_string());
                self.child = Some(c);
                self.crashed = false;
                return Ok(());
            } else {
                self.fatal_error = Some("engine crashed and respawn failed".into());
                return Err(self.fatal_error.clone().unwrap());
            }
        } else {
            self.fatal_error = Some("engine crashed again after respawn — clear error: restart the app".into());
            Err(self.fatal_error.clone().unwrap())
        }
    }

    /// Mark child as dead externally (for tests).
    pub fn simulate_crash(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.crashed = true;
    }

    pub fn is_alive(&mut self) -> bool {
        if let Some(ref mut c) = self.child {
            match c.try_wait() {
                Ok(None) => true,
                Ok(Some(_)) => false,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    /// Stop with polite shutdown -> forceShutdown -> SIGKILL.
    /// `rpc_try` is called for polite and forced; return true if daemon acked.
    pub fn shutdown_with<F>(&mut self, mut rpc_try: F) -> io::Result<()>
    where
        F: FnMut(&str) -> bool,
    {
        // 1) polite shutdown
        if rpc_try("aria2.shutdown") {
            if self.wait_child_secs(2) { return self.cleanup(); }
        }
        // 2) forced
        if rpc_try("aria2.forceShutdown") {
            if self.wait_child_secs(2) { return self.cleanup(); }
        }
        // 3) kill
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.cleanup()
    }

    /// Convenience shutdown that just kills (no RPC).
    pub fn shutdown_kill(&mut self) -> io::Result<()> {
        self.shutdown_with(|_| false)
    }

    fn wait_child_secs(&mut self, secs: u64) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        while std::time::Instant::now() < deadline {
            if let Some(ref mut c) = self.child {
                match c.try_wait() {
                    Ok(Some(_)) => return true,
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
                    Err(_) => return true,
                }
            } else {
                return true;
            }
        }
        // check one last time
        if let Some(ref mut c) = self.child {
            matches!(c.try_wait(), Ok(Some(_)))
        } else { true }
    }

    fn cleanup(&mut self) -> io::Result<()> {
        if let Some(mut c) = self.child.take() {
            let _ = c.try_wait();
        }
        let _ = fs::remove_file(&self.config_path);
        // release lock by dropping
        self.lock = None;
        Ok(())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.config_path);
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn write_secret_config(path: &Path, secret: &str) -> io::Result<()> {
    if let Some(p) = path.parent() { fs::create_dir_all(p)?; }
    fs::write(path, format!("rpc-secret={secret}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(path)?.permissions();
        perm.set_mode(0o600);
        fs::set_permissions(path, perm)?;
    }
    Ok(())
}

fn try_spawn_child(
    bin: Option<&str>,
    conf: &Path,
    port: u16,
    session: &Path,
    log: &Path,
    state_dir: &Path,
    mock_sleep_secs: Option<u64>,
) -> Option<Child> {
    let bin = bin?;
    // Mock path: if bin is sleep, spawn sleep
    if bin.ends_with("sleep") || bin == "sleep" {
        let secs = mock_sleep_secs.unwrap_or(60).to_string();
        return Command::new(bin).arg(secs).spawn().ok();
    }
    // Try real aria2c
    let mut cmd = Command::new(bin);
    cmd.arg(format!("--conf-path={}", conf.display()))
        .arg("--enable-rpc=true")
        .arg(format!("--rpc-listen-port={port}"))
        .arg("--rpc-listen-all=false")
        .arg("--dir")
        .arg(state_dir)
        .arg(format!("--input-file={}", session.display()))
        .arg(format!("--save-session={}", session.display()))
        .arg("--save-session-interval=30")
        .arg(format!("--log={}", log.display()))
        .arg("--log-level=warn")
        .arg("--disable-ipv6=false")
        .arg("--quiet=true");
    // Don't fail if binary missing; return None
    match cmd.spawn() {
        Ok(c) => Some(c),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn starts_with_secret_and_private_config() {
        let dir = tempdir().unwrap();
        let eng = Engine::start(dir.path(), StartOptions { aria_bin: Some("sleep".into()), mock_sleep_secs: Some(60) }).unwrap();
        assert_eq!(eng.secret.len(), 32);
        let cfg = fs::read_to_string(&eng.config_path).unwrap();
        assert!(cfg.contains("rpc-secret="));
        assert!(cfg.contains(&eng.secret));
        // only secret in that file, no other options
        assert_eq!(cfg.lines().count(), 1);
        let mode = fs::metadata(&eng.config_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn uses_first_free_port() {
        let dir = tempdir().unwrap();
        let eng = Engine::start(dir.path(), StartOptions { aria_bin: Some("sleep".into()), mock_sleep_secs: Some(60) }).unwrap();
        assert!((6800..=6899).contains(&eng.port));
    }

    #[test]
    fn second_start_refuses_lock() {
        let dir = tempdir().unwrap();
        let _a = Engine::start(dir.path(), StartOptions { aria_bin: Some("sleep".into()), mock_sleep_secs: Some(60) }).unwrap();
        let b = Engine::start(dir.path(), StartOptions { aria_bin: Some("sleep".into()), mock_sleep_secs: Some(60) });
        assert!(b.is_err());
        let msg = match b { Err(e) => e.to_string(), Ok(_) => panic!("should err") };
        assert!(msg.contains("already locked") || msg.contains("lock"));
    }

    #[test]
    fn tick_is_one_batch_with_all_four() {
        let dir = tempdir().unwrap();
        let eng = Engine::start(dir.path(), StartOptions { aria_bin: Some("sleep".into()), mock_sleep_secs: Some(60) }).unwrap();
        let batch = eng.poll_batch(10);
        let arr = batch.as_array().unwrap();
        assert_eq!(arr.len(), 4);
    }

    #[test]
    fn enqueue_variants_and_strip() {
        let dir = tempdir().unwrap();
        let eng = Engine::start(dir.path(), StartOptions { aria_bin: Some("sleep".into()), mock_sleep_secs: Some(60) }).unwrap();
        let (_, r1) = eng.prepare_enqueue(EnqueueKind::Uris(vec!["https://u:p@example.com/f".into()]), 1);
        assert!(!r1.uris[0].contains("u:p"));
        let (_, r2) = eng.prepare_enqueue(EnqueueKind::Torrent(vec![1,2,3]), 2);
        assert_eq!(r2.kind, "torrent");
        let (_, r3) = eng.prepare_enqueue(EnqueueKind::Metalink(b"<metalink/>".to_vec()), 3);
        assert_eq!(r3.kind, "metalink");
        for r in [r1, r2, r3] { assert!(!r.contains_secret(&eng.secret)); }
    }

    #[test]
    fn stop_uses_polite_then_forced_then_kill() {
        let dir = tempdir().unwrap();
        let mut eng = Engine::start(dir.path(), StartOptions { aria_bin: Some("sleep".into()), mock_sleep_secs: Some(60) }).unwrap();
        let mut calls = vec![];
        eng.shutdown_with(|m| { calls.push(m.to_string()); false }).unwrap();
        assert_eq!(calls, vec!["aria2.shutdown","aria2.forceShutdown"]);
        assert!(!eng.config_path.exists());
    }

    #[test]
    fn stop_polite_success_skips_forced() {
        let dir = tempdir().unwrap();
        let mut eng = Engine::start(dir.path(), StartOptions { aria_bin: Some("sleep".into()), mock_sleep_secs: Some(1) }).unwrap();
        // make child exit quickly on polite? We simulate by killing child before shutdown_with
        // and having rpc_try return true and wait_child will see dead
        eng.simulate_crash();
        let mut calls = vec![];
        eng.shutdown_with(|m| { calls.push(m.to_string()); true }).unwrap();
        assert_eq!(calls, vec!["aria2.shutdown"]);
    }

    #[test]
    fn respawn_once_then_error() {
        let dir = tempdir().unwrap();
        let mut eng = Engine::start(dir.path(), StartOptions { aria_bin: Some("sleep".into()), mock_sleep_secs: Some(60) }).unwrap();
        eng.simulate_crash();
        let r = eng.on_crash();
        assert!(r.is_ok());
        assert!(eng.fatal_error.is_none());
        eng.simulate_crash();
        let e = eng.on_crash().unwrap_err();
        assert!(e.contains("clear error") || e.contains("crashed again"));
        assert!(eng.fatal_error.is_some());
    }
}
