use std::collections::HashMap;
use std::path::Path;

use moon_down_core::settings::{load_settings, save_settings, Settings};
use moon_down_engine::Daemon;
use serde_json::Value;

/// Settings screen: shows all nine, edits apply instantly via one changeGlobalOption, no restart.
/// Handles limit units, empty=unlimited, clamping hints, and empty-dir rejection.
pub struct SettingsScreen {
    pub settings: Settings,
    /// per-field hint (clamp messages)
    pub hints: HashMap<String, String>,
    /// last error (e.g., empty dir)
    pub error: Option<String>,
    config_path: std::path::PathBuf,
}

impl SettingsScreen {
    pub fn new(config_path: &Path, state_dir: &Path) -> Self {
        let settings = load_settings(config_path, state_dir).unwrap_or_else(|_| Settings::default_for(state_dir));
        Self {
            settings,
            hints: HashMap::new(),
            error: None,
            config_path: config_path.to_path_buf(),
        }
    }

    /// All nine fields in display order.
    pub fn fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("max_concurrent_downloads", self.settings.max_concurrent_downloads.to_string()),
            ("download_limit", match self.settings.max_overall_download_limit {
                None => String::new(),
                Some(v) => moon_down_core::settings::format_bytes(v),
            }),
            ("upload_limit", match self.settings.max_overall_upload_limit {
                None => String::new(),
                Some(v) => moon_down_core::settings::format_bytes(v),
            }),
            ("split", self.settings.split.to_string()),
            ("connections_per_server", self.settings.max_connection_per_server.to_string()),
            ("min_split_size", moon_down_core::settings::format_bytes(self.settings.min_split_size)),
            ("dir", self.settings.dir.display().to_string()),
            ("max_tries", self.settings.max_tries.to_string()),
            ("retry_wait", self.settings.retry_wait.to_string()),
        ]
    }

    fn persist(&self) {
        let _ = save_settings(&self.settings, &self.config_path);
    }

    /// Apply one field through the daemon live — returns single RPC Value.
    fn apply_one(&self, daemon: &Daemon, field: &str, id: u64) -> Value {
        let (k, v) = self.settings.wire_pair(field);
        daemon.change_global_option(&k, &v, id)
    }

    pub fn set_concurrent(&mut self, daemon: &Daemon, raw: i64, id: u64) -> Value {
        self.error = None;
        if let Some(h) = self.settings.set_max_concurrent_downloads(raw) {
            self.hints.insert("max_concurrent_downloads".into(), h);
        } else {
            self.hints.remove("max_concurrent_downloads");
        }
        self.persist();
        self.apply_one(daemon, "max_concurrent_downloads", id)
    }

    pub fn set_download_limit(&mut self, daemon: &Daemon, raw: &str, id: u64) -> Result<Value, String> {
        self.error = None;
        self.settings.set_download_limit_str(raw).map_err(|e| e.clone())?;
        self.hints.remove("download_limit");
        self.persist();
        Ok(self.apply_one(daemon, "download_limit", id))
    }

    pub fn set_upload_limit(&mut self, daemon: &Daemon, raw: &str, id: u64) -> Result<Value, String> {
        self.error = None;
        self.settings.set_upload_limit_str(raw).map_err(|e| e.clone())?;
        self.hints.remove("upload_limit");
        self.persist();
        Ok(self.apply_one(daemon, "upload_limit", id))
    }

    pub fn set_split(&mut self, daemon: &Daemon, raw: i64, id: u64) -> Value {
        self.error = None;
        if let Some(h) = self.settings.set_split(raw) {
            self.hints.insert("split".into(), h);
        } else {
            self.hints.remove("split");
        }
        self.persist();
        self.apply_one(daemon, "split", id)
    }

    pub fn set_connections(&mut self, daemon: &Daemon, raw: i64, id: u64) -> Value {
        self.error = None;
        if let Some(h) = self.settings.set_max_connection_per_server(raw) {
            self.hints.insert("connections_per_server".into(), h);
        } else {
            self.hints.remove("connections_per_server");
        }
        self.persist();
        self.apply_one(daemon, "connections_per_server", id)
    }

    pub fn set_min_split(&mut self, daemon: &Daemon, raw: &str, id: u64) -> Result<Value, String> {
        self.error = None;
        match self.settings.set_min_split_size_str(raw) {
            Ok(hint) => {
                if let Some(h) = hint {
                    self.hints.insert("min_split_size".into(), h);
                } else {
                    self.hints.remove("min_split_size");
                }
                self.persist();
                Ok(self.apply_one(daemon, "min_split_size", id))
            }
            Err(e) => Err(e),
        }
    }

    pub fn set_dir(&mut self, daemon: &Daemon, raw: &str, id: u64) -> Result<Value, String> {
        match self.settings.set_dir(raw) {
            Ok(()) => {
                self.error = None;
                self.hints.remove("dir");
                self.persist();
                Ok(self.apply_one(daemon, "dir", id))
            }
            Err(e) => {
                self.error = Some(e.clone());
                Err(e)
            }
        }
    }

    pub fn set_retries(&mut self, daemon: &Daemon, raw: i64, id: u64) -> Value {
        self.error = None;
        if let Some(h) = self.settings.set_max_tries(raw) {
            self.hints.insert("max_tries".into(), h);
        } else {
            self.hints.remove("max_tries");
        }
        self.persist();
        self.apply_one(daemon, "max_tries", id)
    }

    pub fn set_retry_wait(&mut self, daemon: &Daemon, raw: i64, id: u64) -> Value {
        self.error = None;
        if let Some(h) = self.settings.set_retry_wait(raw) {
            self.hints.insert("retry_wait".into(), h);
        } else {
            self.hints.remove("retry_wait");
        }
        self.persist();
        self.apply_one(daemon, "retry_wait", id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moon_down_engine::DaemonInfo;
    use tempfile::tempdir;

    /// A daemon handle with no process behind it: these tests only inspect the
    /// RPC payload a setting change produces, so nothing needs to be spawned.
    fn fake_daemon(state_dir: &std::path::Path) -> Daemon {
        Daemon {
            state_dir: state_dir.to_path_buf(),
            info: DaemonInfo { port: 0, secret: "test-secret".into() },
        }
    }

    fn engine_and_screen() -> (Daemon, SettingsScreen, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        let daemon = fake_daemon(&state_dir);
        let cfg = dir.path().join("settings.json");
        let screen = SettingsScreen::new(&cfg, &state_dir);
        (daemon, screen, dir)
    }

    #[test]
    fn screen_shows_all_nine_defaults() {
        let (_daemon, scr, _dir) = engine_and_screen();
        assert_eq!(scr.fields().len(), 9);
        let map: HashMap<_, _> = scr.fields().into_iter().collect();
        assert_eq!(map["max_concurrent_downloads"], "5");
        assert_eq!(map["download_limit"], ""); // unlimited => empty input
        assert_eq!(map["upload_limit"], "");
        assert_eq!(map["split"], "5");
        assert_eq!(map["connections_per_server"], "1");
        assert_eq!(map["min_split_size"], "20M");
        assert_eq!(map["max_tries"], "5");
        assert_eq!(map["retry_wait"], "0");
    }

    #[test]
    fn change_applies_at_once_with_no_restart_single_rpc() {
        let (daemon, mut scr, _dir) = engine_and_screen();
        let v = scr.set_concurrent(&daemon, 10, 1);
        assert_eq!(v["method"], "aria2.changeGlobalOption");
        assert_eq!(v["params"][1]["max-concurrent-downloads"], "10");
        assert_eq!(v["params"][1].as_object().unwrap().len(), 1);
    }

    #[test]
    fn limit_accepts_number_with_unit_empty_means_unlimited() {
        let (daemon, mut scr, _dir) = engine_and_screen();
        let v = scr.set_download_limit(&daemon, "500K", 1).unwrap();
        assert_eq!(v["params"][1]["max-overall-download-limit"], (500 * 1024).to_string());
        let v2 = scr.set_download_limit(&daemon, "", 2).unwrap();
        assert_eq!(v2["params"][1]["max-overall-download-limit"], "0");
    }

    #[test]
    fn numeric_clamps_and_shows_hint() {
        let (daemon, mut scr, _dir) = engine_and_screen();
        let _ = scr.set_concurrent(&daemon, 999, 1);
        assert!(scr.hints.contains_key("max_concurrent_downloads"));
        assert!(scr.hints["max_concurrent_downloads"].contains("clamped"));
    }

    #[test]
    fn empty_dir_fails_and_keeps_old() {
        let (daemon, mut scr, _dir) = engine_and_screen();
        let old = scr.settings.dir.clone();
        let err = scr.set_dir(&daemon, "", 1).unwrap_err();
        assert!(err.contains("must not be empty"));
        assert_eq!(scr.settings.dir, old);
        assert!(scr.error.is_some());
        // valid
        scr.set_dir(&daemon, "/tmp/x", 2).unwrap();
        assert_eq!(scr.settings.dir.display().to_string(), "/tmp/x");
    }

    #[test]
    fn first_run_merges_saved_over_defaults() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        let cfg = dir.path().join("settings.json");
        // first run: no file
        let s1 = SettingsScreen::new(&cfg, &state_dir);
        assert_eq!(s1.settings.max_concurrent_downloads, 5);
        // change via screen persists
        drop(s1);
        // second instance with saved value
        let mut settings = moon_down_core::settings::Settings::default_for(&state_dir);
        settings.max_concurrent_downloads = 12;
        moon_down_core::settings::save_settings(&settings, &cfg).unwrap();
        let s2 = SettingsScreen::new(&cfg, &state_dir);
        assert_eq!(s2.settings.max_concurrent_downloads, 12);
        // dir default preserved
        assert_eq!(s2.settings.dir, state_dir);
    }
}
