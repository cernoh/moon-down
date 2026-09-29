//! Background daemon lifecycle: start aria2-rust detached, attach to it later.
//!
//! The daemon is the same binary re-executed with `--daemon`, so there is no
//! external aria2c to find and no second install step. It is detached with
//! `setsid` before exec and keeps running after the TUI exits; the TUI only ever
//! attaches by RPC.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::client::{ClientError, RpcClient};
use crate::rpc::Tick;

/// How the running daemon is reached. Persisted so a second TUI attaches to the
/// same daemon instead of starting another one. The secret is 0600, like the conf.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DaemonInfo {
    pub port: u16,
    pub secret: String,
}

/// A handle to the daemon. Deliberately has no `Drop` that kills it: the daemon
/// is meant to outlive every TUI session.
pub struct Daemon {
    pub state_dir: PathBuf,
    pub info: DaemonInfo,
}

impl Daemon {
    pub fn info_path(state_dir: &Path) -> PathBuf {
        state_dir.join("daemon.json")
    }

    pub fn conf_path(state_dir: &Path) -> PathBuf {
        state_dir.join("aria2.conf")
    }

    /// The RPC client for this daemon, built from the persisted port + secret.
    pub fn client(&self) -> RpcClient {
        RpcClient::new(
            format!("http://127.0.0.1:{}/jsonrpc", self.info.port),
            self.info.secret.clone(),
        )
    }

    /// Attach to a live daemon, or start one and wait for its RPC to answer.
    /// `exe` is the moon-down binary to re-execute in daemon mode.
    pub fn ensure_running(state_dir: &Path, exe: &Path, download_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(state_dir)?;
        if let Ok(info) = Self::load(state_dir) {
            if Self::alive(&info) {
                return Ok(Self { state_dir: state_dir.to_path_buf(), info });
            }
        }

        let info = DaemonInfo {
            port: crate::port::pick_free_port().ok_or_else(|| {
                io::Error::new(io::ErrorKind::AddrInUse, "no free port in 6800-6899")
            })?,
            secret: crate::secret::generate_secret(),
        };
        Self::write_conf(state_dir, &info, download_dir)?;
        Self::write_info(state_dir, &info)?;

        // Detach before exec: the child gets its own session, so it keeps running
        // when this TUI exits and never receives the terminal's Ctrl-C.
        //
        // NOT `--daemon=true` on the aria2 side: that path double-forks from
        // inside the tokio runtime, and fork() in a threaded process deadlocks
        // the child on a lock the vanished threads held (verified: the grandchild
        // hung in futex with no sockets bound). setsid in pre_exec forks and execs
        // immediately, with no allocator or lock traffic in between.
        let mut cmd = Command::new(exe);
        cmd.arg("--daemon")
            .arg(format!("--state-dir={}", state_dir.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        detach_session(&mut cmd);
        cmd.spawn()?;

        // The daemon needs a moment to bind; poll the port rather than sleep blindly.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if Self::alive(&info) {
                return Ok(Self { state_dir: state_dir.to_path_buf(), info });
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "daemon did not answer RPC"))
    }

    /// Liveness is an authenticated round trip, not a pid file: aria2-rust only
    /// writes one in its own `--daemon` mode, which we deliberately never use.
    /// A bare `getVersion` proves both that the port is open and that the daemon
    /// is ours holding this secret.
    pub fn alive(info: &DaemonInfo) -> bool {
        let client = RpcClient::new(
            format!("http://127.0.0.1:{}/jsonrpc", info.port),
            info.secret.clone(),
        );
        matches!(
            client.call("aria2.getVersion", json!([]), 0),
            Ok(v) if v.get("result").is_some()
        )
    }

    pub fn load(state_dir: &Path) -> io::Result<DaemonInfo> {
        let raw = fs::read(Self::info_path(state_dir))?;
        serde_json::from_slice(&raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn write_info(state_dir: &Path, info: &DaemonInfo) -> io::Result<()> {
        let path = Self::info_path(state_dir);
        fs::write(&path, serde_json::to_vec_pretty(info).unwrap_or_default())?;
        set_private(&path)
    }

    /// One batched tick: active + waiting + stopped + global stat, all authenticated.
    pub fn tick(&self, rpc_id: u64) -> Result<Tick, ClientError> {
        self.client().tick(rpc_id)
    }

    /// Live-apply one global option with no restart: one authenticated RPC call.
    pub fn change_global_option(&self, key: &str, value: &str, id: u64) -> Value {
        crate::rpc::build_change_global_option(&self.info.secret, key, value, id)
    }

    /// Politely stop the daemon. Never implicit: the user must ask.
    pub fn stop(&self) -> bool {
        self.client()
            .call("aria2.shutdown", json!([]), 0)
            .is_ok()
    }

    fn write_conf(state_dir: &Path, info: &DaemonInfo, download_dir: &Path) -> io::Result<()> {
        let session = state_dir.join("aria2.session");
        if !session.exists() {
            fs::write(&session, b"")?;
        }
        fs::create_dir_all(download_dir)?;
        // `daemon=true` is deliberately absent: it belongs on the command line so
        // ordinary one-shot runs are never detached (aria2-rust requirement).
        let conf = format!(
            "enable-rpc=true\n\
             rpc-listen-all=false\n\
             rpc-listen-address=127.0.0.1\n\
             rpc-listen-port={port}\n\
             rpc-secret={secret}\n\
             dir={downloads}\n\
             continue=true\n\
             input-file={session}\n\
             save-session={session}\n\
             save-session-interval=30\n\
             max-concurrent-downloads=5\n\
             log={log}\n\
             log-level=warn\n\
             console-log-level=error\n",
            port = info.port,
            secret = info.secret,
            downloads = download_dir.display(),
            session = session.display(),
            log = state_dir.join("aria2.log").display(),
        );
        let path = Self::conf_path(state_dir);
        fs::write(&path, conf)?;
        set_private(&path)
    }
}

/// Put the child in its own session so it outlives this process's terminal.
/// Runs in the forked child before exec, where only async-signal-safe work is
/// allowed — which is why this is setsid and not a second fork plus bookkeeping.
fn detach_session(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe; no allocation, no locks.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
    }
}

fn set_private(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(path)?.permissions();
        perm.set_mode(0o600);
        fs::set_permissions(path, perm)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn first_run_writes_private_conf_and_info() {
        let dir = tempdir().unwrap();
        let info = DaemonInfo { port: 6801, secret: "s3cr3t".into() };
        Daemon::write_conf(dir.path(), &info, &dir.path().join("dl")).unwrap();
        Daemon::write_info(dir.path(), &info).unwrap();

        let conf = fs::read_to_string(Daemon::conf_path(dir.path())).unwrap();
        assert!(conf.contains("rpc-secret=s3cr3t"));
        assert!(conf.contains("rpc-listen-port=6801"));
        // daemon=true must stay off the file, it belongs on argv
        assert!(!conf.contains("daemon"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [Daemon::conf_path(dir.path()), Daemon::info_path(dir.path())] {
                let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "{} must not be world-readable", p.display());
            }
        }
    }

    #[test]
    fn info_round_trips_for_reattach() {
        let dir = tempdir().unwrap();
        let info = DaemonInfo { port: 6820, secret: "abc".into() };
        Daemon::write_info(dir.path(), &info).unwrap();
        assert_eq!(Daemon::load(dir.path()).unwrap(), info);
    }

    #[test]
    fn tick_errors_when_no_daemon_answers() {
        // A closed port must not panic or hang the UI tick.
        let dir = tempdir().unwrap();
        let port = {
            std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
        };
        let d = Daemon { state_dir: dir.path().to_path_buf(), info: DaemonInfo { port, secret: "x".into() } };
        assert!(d.tick(1).is_err());
        assert!(!d.stop());
        assert!(!Daemon::alive(&d.info));
    }
}
