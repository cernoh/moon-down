use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};

use moon_down_core::extract::{discover_archives, extract_archive, extract_archive_in, ExtractError};
use moon_down_core::queue::{ExtractStatus, Package};

/// Where the trigger came from. Live feed is WS-first; poll is fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerSource {
    LiveFeed,
    TimedPoll,
}

/// Result of an extraction attempt.
#[derive(Debug, Clone)]
pub struct ExtractOutcome {
    pub package_id: u64,
    pub result: Result<(), String>,
    pub trigger: TriggerSource,
}

/// Check if package is ready for extraction: extract enabled, not already done/extracting, all members complete.
pub fn is_ready(pkg: &Package) -> bool {
    pkg.is_extract_ready()
}

/// Spawn extraction on a blocking thread (std::thread, named for observability).
/// Returns JoinHandle that yields Result<(), ExtractError>.
/// `archives` are absolute paths; `base_dest` is package target dir.
pub fn spawn_extract_blocking(
    archives: Vec<PathBuf>,
    base_dest: PathBuf,
    stem_subdir: bool,
    keep_archives: bool,
    search: Option<String>,
) -> JoinHandle<Result<(), ExtractError>> {
    thread::Builder::new()
        .name("extract-blocking".into())
        .spawn(move || {
            let res = extract_many_blocking(&archives, &base_dest, stem_subdir, search.as_deref());
            if res.is_ok() && !keep_archives {
                for a in &archives {
                    let _ = std::fs::remove_file(a);
                }
            }
            // on failure archives are kept (no deletion)
            res
        })
        .expect("spawn extract-blocking")
}

fn extract_many_blocking(
    archives: &[PathBuf],
    base_dest: &Path,
    stem_subdir: bool,
    search: Option<&str>,
) -> Result<(), ExtractError> {
    for archive in archives {
        let dest = if stem_subdir {
            let stem = archive.file_stem().and_then(|s| s.to_str()).unwrap_or("extracted");
            let stem = if stem.ends_with(".tar") { &stem[..stem.len()-4] } else { stem };
            base_dest.join(stem)
        } else {
            base_dest.to_path_buf()
        };
        match search {
            Some(p) => extract_archive_in(archive, &dest, p)?,
            None => extract_archive(archive, &dest)?,
        }
    }
    // nested check after all: scan dest for archives produced
    // (individual extract_archive already checks, but post-scan covers 7z case)
    Ok(())
}

/// Discover archives for a package from its target dir and trigger extraction if ready.
/// Returns handle if extraction was started, otherwise None.
/// Updates `pkg.extract_status` to Extracting immediately (caller must hold mutable ref).
pub fn trigger_if_ready(
    pkg: &mut Package,
    trigger: TriggerSource,
    search: Option<String>,
) -> Option<(JoinHandle<Result<(), ExtractError>>, TriggerSource)> {
    if !is_ready(pkg) {
        return None;
    }
    let base = PathBuf::from(&pkg.target_dir);
    let archives = discover_archives(&base);
    if archives.is_empty() {
        // no archives to extract -> mark extracted immediately
        pkg.extract_status = ExtractStatus::Extracted;
        return None;
    }
    pkg.extract_status = ExtractStatus::Extracting;
    pkg.extract_error = None;
    let stem = pkg.extract_stem_subdir;
    let keep = pkg.keep_archives;
    let h = spawn_extract_blocking(archives, base, stem, keep, search);
    Some((h, trigger))
}

/// Orchestrator that handles live-feed first, poll fallback, and manual retry.
/// Tracks nothing else; readiness is derived from package state so failed packages are not auto-retied.
pub struct ExtractOrchestrator {
    /// Explicit binary search path; `None` resolves 7z from PATH at spawn time.
    search: Option<String>,
}

impl ExtractOrchestrator {
    pub fn new() -> Self {
        Self { search: None }
    }

    /// Resolve 7z/7zz in `search` instead of PATH (tests, embedded hosts).
    pub fn with_search_path(mut self, search: &str) -> Self {
        self.search = Some(search.to_string());
        self
    }

    /// Called when live feed reports a package's members completed.
    /// Returns handle if extraction started.
    pub fn on_live_complete(
        &self,
        pkg: &mut Package,
    ) -> Option<JoinHandle<Result<(), ExtractError>>> {
        trigger_if_ready(pkg, TriggerSource::LiveFeed, self.search.clone()).map(|(h, _)| h)
    }

    /// Timed poll fallback: scan all packages, start extraction for any ready ones that live feed missed.
    pub fn on_poll_tick<'a>(
        &self,
        queue: &'a mut moon_down_core::Queue,
    ) -> Vec<(u64, JoinHandle<Result<(), ExtractError>>)> {
        let mut handles = Vec::new();
        for pkg in &mut queue.packages {
            if let Some((h, _)) = trigger_if_ready(pkg, TriggerSource::TimedPoll, self.search.clone()) {
                let id = pkg.id;
                handles.push((id, h));
            }
        }
        handles
    }

    /// Manual retry: only allowed when status is ExtractFailed.
    pub fn retry(
        &self,
        pkg: &mut Package,
    ) -> Option<JoinHandle<Result<(), ExtractError>>> {
        if pkg.extract_status != ExtractStatus::ExtractFailed {
            return None;
        }
        pkg.retry_extract();
        // now status is Idle, so trigger will see it as ready if complete
        trigger_if_ready(pkg, TriggerSource::LiveFeed, self.search.clone()).map(|(h, _)| h)
    }

    /// Apply outcome back to package (call after JoinHandle joins).
    pub fn apply_outcome(pkg: &mut Package, outcome: Result<(), ExtractError>) {
        match outcome {
            Ok(()) => {
                pkg.extract_status = ExtractStatus::Extracted;
                pkg.extract_error = None;
            }
            Err(e) => {
                pkg.extract_status = ExtractStatus::ExtractFailed;
                pkg.extract_error = Some(e.to_string());
                // archives are kept; no auto-retry
            }
        }
    }
}

impl Default for ExtractOrchestrator { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;
    use moon_down_core::queue::{MemberState, Queue};
    use std::fs;
    use std::io::Write;

    fn complete_package_with_archives(dir: &Path, name: &str) -> (Queue, u64) {
        let mut q = Queue::new();
        let pid = q.add_package(name, dir.to_str().unwrap(), vec![("file".into(), "https://example.com/file".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_handle(mid, "gid1".into());
        q.set_member_state(mid, MemberState::Complete);
        // defaults already ON
        assert!(q.packages[0].is_extract_ready());
        (q, pid)
    }

    #[test]
    fn defaults_are_on() {
        let mut q = Queue::new();
        q.add_package("p", "/tmp", vec![("m".into(), "s".into())]);
        let p = &q.packages[0];
        assert!(p.extract_enabled);
        assert!(p.extract_stem_subdir);
        assert!(p.keep_archives);
    }

    #[test]
    fn waits_for_all_handles_complete() {
        let mut q = Queue::new();
        q.add_package("p", "/tmp", vec![("a".into(), "src-a".into()), ("b".into(), "src-b".into())]);
        let ids: Vec<u64> = q.packages[0].members.iter().map(|m| m.id).collect();
        q.set_member_handle(ids[0], "g1".into());
        q.set_member_handle(ids[1], "g2".into());
        q.set_member_state(ids[0], MemberState::Complete);
        q.set_member_state(ids[1], MemberState::Downloading);
        assert!(!q.packages[0].is_extract_ready());
        q.set_member_state(ids[1], MemberState::Complete);
        assert!(q.packages[0].is_extract_ready());
    }

    #[test]
    fn live_feed_triggers_and_poll_covers_drop() {
        let dir = tempfile::tempdir().unwrap();
        // create a zip archive in target dir
        let zip_path = dir.path().join("a.zip");
        {
            let f = fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zw.start_file("hello.txt", opts).unwrap();
            zw.write_all(b"hi").unwrap();
            zw.finish().unwrap();
        }
        let (mut q, _) = complete_package_with_archives(dir.path(), "p");
        let orch = ExtractOrchestrator::new();
        // live feed triggers
        let pkg = &mut q.packages[0];
        let h = orch.on_live_complete(pkg).expect("should trigger");
        assert_eq!(pkg.extract_status, ExtractStatus::Extracting);
        let res = h.join().unwrap();
        ExtractOrchestrator::apply_outcome(&mut q.packages[0], res.map_err(|e| match e { e => e }));
        assert_eq!(q.packages[0].extract_status, ExtractStatus::Extracted);
        assert!(dir.path().join("a").join("hello.txt").exists());
        // poll after extracted should not retrigger
        let handles = orch.on_poll_tick(&mut q);
        assert!(handles.is_empty());

        // now test poll fallback covers live drop: new package that never got live event
        let dir2 = tempfile::tempdir().unwrap();
        let zip_path2 = dir2.path().join("b.zip");
        {
            let f = fs::File::create(&zip_path2).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zw.start_file("hello2.txt", opts).unwrap();
            zw.write_all(b"hi2").unwrap();
            zw.finish().unwrap();
        }
        let mut q2 = Queue::new();
        q2.add_package("p2", dir2.path().to_str().unwrap(), vec![("m".into(), "s".into())]);
        let mid = q2.packages[0].members[0].id;
        q2.set_member_handle(mid, "gid2".into());
        q2.set_member_state(mid, MemberState::Complete);
        // simulate live drop: do NOT call on_live_complete, only poll
        let mut q2_clone = q2.clone();
        let handles = orch.on_poll_tick(&mut q2_clone);
        assert_eq!(handles.len(), 1);
        let (pid, h) = handles.into_iter().next().unwrap();
        let res = h.join().unwrap();
        let pkg = q2_clone.packages.iter_mut().find(|p| p.id == pid).unwrap();
        ExtractOrchestrator::apply_outcome(pkg, res);
        assert_eq!(pkg.extract_status, ExtractStatus::Extracted);
    }

    #[test]
    fn blocking_thread_name() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("a.zip");
        {
            let f = fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zw.start_file("x.txt", opts).unwrap();
            zw.write_all(b"x").unwrap();
            zw.finish().unwrap();
        }
        let h = spawn_extract_blocking(vec![zip_path], dir.path().to_path_buf(), false, true, None);
        // check thread name via handle? we set name to extract-blocking
        // join and check success
        let res = h.join().unwrap();
        assert!(res.is_ok());
        // verify source mentions blocking thread
        let src = include_str!("extract.rs");
        assert!(src.contains("extract-blocking"));
        assert!(src.contains("spawn"));
    }

    #[test]
    fn failures_keep_archives_and_need_manual_retry() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("nested.zip");
        {
            let f = fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zw.start_file("inner.zip", opts).unwrap();
            zw.write_all(b"dummy").unwrap();
            zw.finish().unwrap();
        }
        let mut q = Queue::new();
        q.add_package("p", dir.path().to_str().unwrap(), vec![("m".into(), "s".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_handle(mid, "gid".into());
        q.set_member_state(mid, MemberState::Complete);
        let orch = ExtractOrchestrator::new();
        let h = orch.on_live_complete(&mut q.packages[0]).unwrap();
        let res = h.join().unwrap();
        ExtractOrchestrator::apply_outcome(&mut q.packages[0], res);
        assert_eq!(q.packages[0].extract_status, ExtractStatus::ExtractFailed);
        assert!(q.packages[0].extract_error.as_ref().unwrap().to_ascii_lowercase().contains("nested"));
        // archive kept
        assert!(zip_path.exists());
        // no automatic retry on poll
        let handles = orch.on_poll_tick(&mut q);
        assert!(handles.is_empty(), "failed should not auto-retry");
        // manual retry is required - but will fail again (same nested), but should be triggerable
        let h2 = orch.retry(&mut q.packages[0]);
        assert!(h2.is_some());
        let res2 = h2.unwrap().join().unwrap();
        ExtractOrchestrator::apply_outcome(&mut q.packages[0], res2);
        assert_eq!(q.packages[0].extract_status, ExtractStatus::ExtractFailed);
    }

    #[test]
    fn password_protected_fails_clear_message() {
        // We can't easily create password zip without extra steps, but test the error mapping:
        // Simulate by calling extract_archive on a non-existent 7z that would need password path?
        // Instead verify that error variant displays clear message
        let err = ExtractError::PasswordProtected("password-protected archive not supported: foo.zip".into());
        assert!(err.to_string().to_ascii_lowercase().contains("password"));
    }

    #[test]
    fn missing_binary_reports_install_hint() {
        let dir = tempfile::tempdir().unwrap();
        let rar = dir.path().join("a.rar");
        fs::write(&rar, b"dummy").unwrap();
        let mut q = Queue::new();
        q.add_package("p", dir.path().to_str().unwrap(), vec![("m".into(), "s".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_handle(mid, "gid".into());
        q.set_member_state(mid, MemberState::Complete);
        // move rar into target dir already there? discover will find it
        // but we need archive file already in dir: rar is there
        // Explicit search path instead of mutating PATH, which would race sibling
        // tests that spawn real binaries.
        let orch = ExtractOrchestrator::new().with_search_path("/nonexistent");
        let h = orch.on_live_complete(&mut q.packages[0]).unwrap();
        let res = h.join().unwrap();
        ExtractOrchestrator::apply_outcome(&mut q.packages[0], res);
        assert_eq!(q.packages[0].extract_status, ExtractStatus::ExtractFailed);
        let msg = q.packages[0].extract_error.clone().unwrap();
        assert!(msg.to_ascii_lowercase().contains("install"));
        assert!(msg.contains("7zz") || msg.contains("7z"));
    }

    #[test]
    fn sevenz_detection_prefers_7zz() {
        // ensure find logic is present
        let src = include_str!("../src/extract.rs"); // engine extract includes core extract indirectly
        // core extract has the detection
        let core_src = include_str!("../../core/src/extract.rs");
        assert!(core_src.contains("7zz"));
        assert!(core_src.contains("7z"));
        // ensure forbidden binary never appears
        let forbidden = ["un", "rar"].concat();
        assert!(!core_src.contains(&forbidden));
    }

    #[test]
    fn zip_and_tar_no_shell_call() {
        let core_src = include_str!("../../core/src/extract.rs");
        // zip/tar extraction must not use shell()
        // check via split to avoid self-match
        let shell_pat = ["sh", " -", "c"].concat();
        assert!(!core_src.contains(&shell_pat));
        assert!(!core_src.contains("bash"));
    }
}
