use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CONCURRENT_MIN: u32 = 1;
pub const CONCURRENT_MAX: u32 = 32;
pub const SPLIT_MIN: u32 = 1;
pub const SPLIT_MAX: u32 = 32;
pub const CONN_PER_SERVER_MIN: u32 = 1;
pub const CONN_PER_SERVER_MAX: u32 = 16;
pub const MIN_SPLIT_SIZE_MIN: u64 = 1 << 20; // 1M
pub const MIN_SPLIT_SIZE_MAX: u64 = 1 << 30; // 1G
pub const RETRIES_MIN: u32 = 0;
pub const RETRIES_MAX: u32 = 99;
pub const RETRY_WAIT_MIN: u32 = 0;
pub const RETRY_WAIT_MAX: u32 = 3600;
pub const DEFAULT_MIN_SPLIT_SIZE: u64 = 20 * (1 << 20); // 20M
pub const WIRE_UNLIMITED: &str = "0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub max_concurrent_downloads: u32,
    /// None = unlimited (wire "0"), Some(bytes/sec)
    pub max_overall_download_limit: Option<u64>,
    pub max_overall_upload_limit: Option<u64>,
    pub split: u32,
    pub max_connection_per_server: u32,
    /// bytes
    pub min_split_size: u64,
    pub dir: PathBuf,
    /// 0 = unlimited retries
    pub max_tries: u32,
    pub retry_wait: u32,
}

impl Settings {
    pub fn default_for(state_dir: &Path) -> Self {
        Self {
            max_concurrent_downloads: 5,
            max_overall_download_limit: None,
            max_overall_upload_limit: None,
            split: 5,
            max_connection_per_server: 1,
            min_split_size: DEFAULT_MIN_SPLIT_SIZE,
            dir: state_dir.to_path_buf(),
            max_tries: 5,
            retry_wait: 0,
        }
    }

    // ---- clamping setters return hint if clamped ----

    pub fn set_max_concurrent_downloads(&mut self, raw: i64) -> Option<String> {
        let clamped = raw.clamp(CONCURRENT_MIN as i64, CONCURRENT_MAX as i64) as u32;
        let hint = if raw < CONCURRENT_MIN as i64 || raw > CONCURRENT_MAX as i64 {
            Some(format!("clamped to {}..{}", CONCURRENT_MIN, CONCURRENT_MAX))
        } else {
            None
        };
        self.max_concurrent_downloads = clamped;
        hint
    }

    pub fn set_split(&mut self, raw: i64) -> Option<String> {
        let clamped = raw.clamp(SPLIT_MIN as i64, SPLIT_MAX as i64) as u32;
        let hint = if raw < SPLIT_MIN as i64 || raw > SPLIT_MAX as i64 {
            Some(format!("clamped to {}..{}", SPLIT_MIN, SPLIT_MAX))
        } else {
            None
        };
        self.split = clamped;
        hint
    }

    pub fn set_max_connection_per_server(&mut self, raw: i64) -> Option<String> {
        let clamped = raw.clamp(CONN_PER_SERVER_MIN as i64, CONN_PER_SERVER_MAX as i64) as u32;
        let hint = if raw < CONN_PER_SERVER_MIN as i64 || raw > CONN_PER_SERVER_MAX as i64 {
            Some(format!("clamped to {}..{}", CONN_PER_SERVER_MIN, CONN_PER_SERVER_MAX))
        } else {
            None
        };
        self.max_connection_per_server = clamped;
        hint
    }

    pub fn set_min_split_size_bytes(&mut self, bytes: u64) -> Option<String> {
        let clamped = bytes.clamp(MIN_SPLIT_SIZE_MIN, MIN_SPLIT_SIZE_MAX);
        let hint = if bytes < MIN_SPLIT_SIZE_MIN || bytes > MIN_SPLIT_SIZE_MAX {
            Some(format!(
                "clamped to {}..{}",
                format_bytes(MIN_SPLIT_SIZE_MIN),
                format_bytes(MIN_SPLIT_SIZE_MAX)
            ))
        } else {
            None
        };
        self.min_split_size = clamped;
        hint
    }

    pub fn set_min_split_size_str(&mut self, raw: &str) -> Result<Option<String>, String> {
        let bytes = parse_bytes_with_unit(raw)?;
        Ok(self.set_min_split_size_bytes(bytes))
    }

    pub fn set_max_tries(&mut self, raw: i64) -> Option<String> {
        let clamped = raw.clamp(RETRIES_MIN as i64, RETRIES_MAX as i64) as u32;
        let hint = if raw < RETRIES_MIN as i64 || raw > RETRIES_MAX as i64 {
            Some(format!("clamped to {}..{}", RETRIES_MIN, RETRIES_MAX))
        } else {
            None
        };
        self.max_tries = clamped;
        hint
    }

    pub fn set_retry_wait(&mut self, raw: i64) -> Option<String> {
        let clamped = raw.clamp(RETRY_WAIT_MIN as i64, RETRY_WAIT_MAX as i64) as u32;
        let hint = if raw < RETRY_WAIT_MIN as i64 || raw > RETRY_WAIT_MAX as i64 {
            Some(format!("clamped to {}..{}", RETRY_WAIT_MIN, RETRY_WAIT_MAX))
        } else {
            None
        };
        self.retry_wait = clamped;
        hint
    }

    // ---- limit fields: number with unit, empty => unlimited ----

    /// Accepts "", "500", "500K", "20M", "1G" (case-insensitive, optional B suffix). Empty => None.
    pub fn set_download_limit_str(&mut self, raw: &str) -> Result<Option<String>, String> {
        let opt = parse_limit_field(raw)?;
        self.max_overall_download_limit = opt;
        Ok(None)
    }

    pub fn set_upload_limit_str(&mut self, raw: &str) -> Result<Option<String>, String> {
        let opt = parse_limit_field(raw)?;
        self.max_overall_upload_limit = opt;
        Ok(None)
    }

    // ---- directory ----

    /// Reject empty, keep old. Returns Err if empty.
    pub fn set_dir(&mut self, raw: &str) -> Result<(), String> {
        let t = raw.trim();
        if t.is_empty() {
            return Err("directory must not be empty".into());
        }
        self.dir = PathBuf::from(t);
        Ok(())
    }

    // ---- wire mapping ----

    pub fn aria2_key(&self, field: &str) -> String {
        match field {
            "max_concurrent_downloads" => "max-concurrent-downloads".into(),
            "download_limit" => "max-overall-download-limit".into(),
            "upload_limit" => "max-overall-upload-limit".into(),
            "split" => "split".into(),
            "connections_per_server" => "max-connection-per-server".into(),
            "min_split_size" => "min-split-size".into(),
            "dir" => "dir".into(),
            "max_tries" => "max-tries".into(),
            "retry_wait" => "retry-wait".into(),
            _ => field.to_string(),
        }
    }

    /// Wire value for one option (returned as string as aria2 expects).
    pub fn wire_value_for(&self, field: &str) -> String {
        match field {
            "max_concurrent_downloads" => self.max_concurrent_downloads.to_string(),
            "download_limit" => match self.max_overall_download_limit {
                None => WIRE_UNLIMITED.to_string(),
                Some(v) => v.to_string(),
            },
            "upload_limit" => match self.max_overall_upload_limit {
                None => WIRE_UNLIMITED.to_string(),
                Some(v) => v.to_string(),
            },
            "split" => self.split.to_string(),
            "connections_per_server" => self.max_connection_per_server.to_string(),
            "min_split_size" => self.min_split_size.to_string(),
            "dir" => self.dir.display().to_string(),
            "max_tries" => self.max_tries.to_string(),
            "retry_wait" => self.retry_wait.to_string(),
            _ => String::new(),
        }
    }

    pub fn wire_pair(&self, field: &str) -> (String, String) {
        (self.aria2_key(field), self.wire_value_for(field))
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::default_for(Path::new("/tmp"))
    }
}

// ---- helpers ----

fn parse_limit_field(raw: &str) -> Result<Option<u64>, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let v = parse_bytes_with_unit(t).map_err(|e| e)?;
    Ok(Some(v))
}

pub fn parse_bytes_with_unit(raw: &str) -> Result<u64, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("empty value".into());
    }
    // split numeric prefix and suffix
    let mut num_end = 0;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() || c == '.' {
            num_end = i + c.len_utf8();
        } else {
            break;
        }
    }
    if num_end == 0 {
        return Err(format!("invalid number: {raw}"));
    }
    let num_str = &s[..num_end];
    let unit_str = s[num_end..].trim().to_ascii_uppercase();
    let num: f64 = num_str.parse().map_err(|_| format!("invalid number: {raw}"))?;
    if !num.is_finite() || num < 0.0 {
        return Err(format!("invalid number: {raw}"));
    }
    let mul: u64 = match unit_str.as_str() {
        "" | "B" => 1,
        "K" | "KB" => 1024,
        "M" | "MB" => 1024 * 1024,
        "G" | "GB" => 1024 * 1024 * 1024,
        _ => return Err(format!("unknown unit: {unit_str} (use K, M, G)")),
    };
    let bytes = (num * mul as f64).round() as u64;
    Ok(bytes)
}

pub fn format_bytes(b: u64) -> String {
    if b % (1024 * 1024 * 1024) == 0 && b >= 1024 * 1024 * 1024 {
        format!("{}G", b / (1024 * 1024 * 1024))
    } else if b % (1024 * 1024) == 0 {
        format!("{}M", b / (1024 * 1024))
    } else if b % 1024 == 0 {
        format!("{}K", b / 1024)
    } else {
        b.to_string()
    }
}

// ---- persistence: merge saved over defaults ----

pub fn save_settings(settings: &Settings, path: &Path) -> std::io::Result<()> {
    if let Some(p) = path.parent() {
        if !p.as_os_str().is_empty() {
            std::fs::create_dir_all(p)?;
        }
    }
    let data = serde_json::to_vec_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Load settings, merging saved options over defaults. First run (no file) => defaults.
pub fn load_settings(path: &Path, state_dir: &Path) -> std::io::Result<Settings> {
    let defaults = Settings::default_for(state_dir);
    if !path.exists() {
        return Ok(defaults);
    }
    let data = std::fs::read(path)?;
    let v: serde_json::Value =
        serde_json::from_slice(&data).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // overlay present keys over defaults
    let mut out = defaults;
    if let Some(obj) = v.as_object() {
        if let Some(n) = obj.get("max_concurrent_downloads").and_then(|x| x.as_u64()) {
            out.max_concurrent_downloads = n as u32;
        }
        if let Some(x) = obj.get("max_overall_download_limit") {
            if x.is_null() {
                out.max_overall_download_limit = None;
            } else if let Some(n) = x.as_u64() {
                out.max_overall_download_limit = Some(n);
            } else if let Some(s) = x.as_str() {
                // allow "0" unlimited or byte string
                if s == "0" || s.is_empty() {
                    out.max_overall_download_limit = None;
                } else if let Ok(b) = parse_bytes_with_unit(s) {
                    out.max_overall_download_limit = Some(b);
                }
            }
        }
        if let Some(x) = obj.get("max_overall_upload_limit") {
            if x.is_null() {
                out.max_overall_upload_limit = None;
            } else if let Some(n) = x.as_u64() {
                out.max_overall_upload_limit = Some(n);
            } else if let Some(s) = x.as_str() {
                if s == "0" || s.is_empty() {
                    out.max_overall_upload_limit = None;
                } else if let Ok(b) = parse_bytes_with_unit(s) {
                    out.max_overall_upload_limit = Some(b);
                }
            }
        }
        if let Some(n) = obj.get("split").and_then(|x| x.as_u64()) {
            out.split = n as u32;
        }
        if let Some(n) = obj
            .get("max_connection_per_server")
            .and_then(|x| x.as_u64())
        {
            out.max_connection_per_server = n as u32;
        }
        if let Some(x) = obj.get("min_split_size") {
            if let Some(n) = x.as_u64() {
                out.min_split_size = n;
            } else if let Some(s) = x.as_str() {
                if let Ok(b) = parse_bytes_with_unit(s) {
                    out.min_split_size = b;
                }
            }
        }
        if let Some(s) = obj.get("dir").and_then(|x| x.as_str()) {
            if !s.trim().is_empty() {
                out.dir = PathBuf::from(s);
            }
        }
        if let Some(n) = obj.get("max_tries").and_then(|x| x.as_u64()) {
            out.max_tries = n as u32;
        }
        if let Some(n) = obj.get("retry_wait").and_then(|x| x.as_u64()) {
            out.retry_wait = n as u32;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_match_ticket() {
        let d = Settings::default_for(Path::new("/state"));
        assert_eq!(d.max_concurrent_downloads, 5);
        assert_eq!(d.max_overall_download_limit, None);
        assert_eq!(d.max_overall_upload_limit, None);
        assert_eq!(d.split, 5);
        assert_eq!(d.max_connection_per_server, 1);
        assert_eq!(d.min_split_size, 20 * 1024 * 1024);
        assert_eq!(d.dir, PathBuf::from("/state"));
        assert_eq!(d.max_tries, 5);
        assert_eq!(d.retry_wait, 0);
    }

    #[test]
    fn screen_shows_all_nine_with_defaults() {
        let s = Settings::default_for(Path::new("/state"));
        // via wire pairs: must be nine distinct keys
        let fields = [
            "max_concurrent_downloads",
            "download_limit",
            "upload_limit",
            "split",
            "connections_per_server",
            "min_split_size",
            "dir",
            "max_tries",
            "retry_wait",
        ];
        let pairs: Vec<_> = fields.iter().map(|f| s.wire_pair(f)).collect();
        assert_eq!(pairs.len(), 9);
        // defaults: unlimited maps to "0"
        assert_eq!(s.wire_value_for("download_limit"), "0");
        assert_eq!(s.wire_value_for("upload_limit"), "0");
        assert_eq!(s.wire_value_for("max_concurrent_downloads"), "5");
        assert_eq!(s.wire_value_for("dir"), "/state");
    }

    #[test]
    fn limit_field_accepts_number_with_unit() {
        let mut s = Settings::default_for(Path::new("/tmp"));
        s.set_download_limit_str("500K").unwrap();
        assert_eq!(s.max_overall_download_limit, Some(500 * 1024));
        s.set_download_limit_str("20M").unwrap();
        assert_eq!(s.max_overall_download_limit, Some(20 * 1024 * 1024));
        s.set_upload_limit_str("1G").unwrap();
        assert_eq!(s.max_overall_upload_limit, Some(1024 * 1024 * 1024));
        // plain number without unit
        s.set_download_limit_str("1024").unwrap();
        assert_eq!(s.max_overall_download_limit, Some(1024));
    }

    #[test]
    fn empty_limit_means_no_limit() {
        let mut s = Settings::default_for(Path::new("/tmp"));
        s.set_download_limit_str("1M").unwrap();
        assert!(s.max_overall_download_limit.is_some());
        s.set_download_limit_str("").unwrap();
        assert_eq!(s.max_overall_download_limit, None);
        assert_eq!(s.wire_value_for("download_limit"), "0");
        s.set_upload_limit_str("   ").unwrap();
        assert_eq!(s.max_overall_upload_limit, None);
    }

    #[test]
    fn numeric_clamps_and_shows_hint() {
        let mut s = Settings::default_for(Path::new("/tmp"));
        let hint = s.set_max_concurrent_downloads(100);
        assert!(hint.is_some());
        assert_eq!(s.max_concurrent_downloads, CONCURRENT_MAX);
        assert!(hint.unwrap().contains("clamped"));

        let hint2 = s.set_split(0);
        assert!(hint2.is_some());
        assert_eq!(s.split, SPLIT_MIN);

        let h3 = s.set_max_connection_per_server(99);
        assert!(h3.is_some());
        assert_eq!(s.max_connection_per_server, CONN_PER_SERVER_MAX);

        let h4 = s.set_min_split_size_bytes(1);
        assert!(h4.is_some());
        assert_eq!(s.min_split_size, MIN_SPLIT_SIZE_MIN);

        let h5 = s.set_max_tries(1000);
        assert!(h5.is_some());
        assert_eq!(s.max_tries, RETRIES_MAX);

        let h6 = s.set_retry_wait(9999);
        assert!(h6.is_some());
        assert_eq!(s.retry_wait, RETRY_WAIT_MAX);

        // inside range => no hint
        let no = s.set_max_concurrent_downloads(5);
        assert!(no.is_none());
    }

    #[test]
    fn empty_dir_fails_and_keeps_old() {
        let mut s = Settings::default_for(Path::new("/state"));
        let old = s.dir.clone();
        let err = s.set_dir("").unwrap_err();
        assert!(err.contains("must not be empty"));
        assert_eq!(s.dir, old);
        let err2 = s.set_dir("   ").unwrap_err();
        assert_eq!(s.dir, old);
        assert!(err2.contains("must not be empty"));
        // valid change works
        s.set_dir("/tmp/dl").unwrap();
        assert_eq!(s.dir, PathBuf::from("/tmp/dl"));
    }

    #[test]
    fn first_run_merges_saved_over_defaults() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        let cfg = dir.path().join("settings.json");
        // first run: no file => defaults
        let s = load_settings(&cfg, &state_dir).unwrap();
        assert_eq!(s.dir, state_dir);
        assert_eq!(s.max_concurrent_downloads, 5);
        // save partial override
        let mut custom = Settings::default_for(&state_dir);
        custom.max_concurrent_downloads = 10;
        custom.split = 3;
        // keep limits unlimited
        save_settings(&custom, &cfg).unwrap();
        // load merges: saved overrides, defaults for missing (e.g., dir still state_dir)
        let loaded = load_settings(&cfg, &state_dir).unwrap();
        assert_eq!(loaded.max_concurrent_downloads, 10);
        assert_eq!(loaded.split, 3);
        assert_eq!(loaded.max_overall_download_limit, None);
        assert_eq!(loaded.dir, state_dir);
        // now save a dir override and reload
        let mut custom2 = loaded.clone();
        custom2.dir = PathBuf::from("/custom/dir");
        save_settings(&custom2, &cfg).unwrap();
        let loaded2 = load_settings(&cfg, &state_dir).unwrap();
        assert_eq!(loaded2.dir, PathBuf::from("/custom/dir"));
    }

    #[test]
    fn parse_bytes_units() {
        assert_eq!(parse_bytes_with_unit("500K").unwrap(), 500 * 1024);
        assert_eq!(parse_bytes_with_unit("20M").unwrap(), 20 * 1024 * 1024);
        assert_eq!(parse_bytes_with_unit("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_bytes_with_unit("1.5M").unwrap(), (1.5 * 1024.0 * 1024.0) as u64);
        assert!(parse_bytes_with_unit("abc").is_err());
        assert!(parse_bytes_with_unit("10X").is_err());
    }
}
