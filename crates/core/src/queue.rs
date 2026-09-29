use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemberState {
    Queued,
    Downloading,
    Paused,
    Complete,
    Error,
    Gone,
}

impl MemberState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Error | Self::Gone)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PackageStatus {
    Error,
    Gone,
    Active,
    Paused,
    Complete,
    Queued,
}

impl PackageStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Error | Self::Gone)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Member {
    pub id: u64,
    pub name: String,
    /// Current engine handle (aria2 GID). None before first enqueue.
    pub handle: Option<String>,
    /// Handle to use on retry/re-add. Kept stable.
    pub retry_source: String,
    pub state: MemberState,
    pub total_bytes: u64,
    pub completed_bytes: u64,
    pub error_message: Option<String>,
}

impl Member {
    pub fn new(id: u64, name: impl Into<String>, retry_source: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            handle: None,
            retry_source: retry_source.into(),
            state: MemberState::Queued,
            total_bytes: 0,
            completed_bytes: 0,
            error_message: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Package {
    pub id: u64,
    pub name: String,
    pub target_dir: String,
    pub created_at_ms: u64,
    pub members: Vec<Member>,
}

impl Package {
    pub fn status(&self) -> PackageStatus {
        if self.members.is_empty() {
            return PackageStatus::Complete;
        }
        // worst-case precedence: Error > Gone > Active > Paused > Complete
        if self.members.iter().any(|m| m.state == MemberState::Error) {
            return PackageStatus::Error;
        }
        if self.members.iter().any(|m| m.state == MemberState::Gone) {
            return PackageStatus::Gone;
        }
        if self
            .members
            .iter()
            .any(|m| matches!(m.state, MemberState::Downloading | MemberState::Queued))
        {
            return PackageStatus::Active;
        }
        if self.members.iter().any(|m| m.state == MemberState::Paused) {
            return PackageStatus::Paused;
        }
        if self.members.iter().all(|m| m.state == MemberState::Complete) {
            return PackageStatus::Complete;
        }
        // fallback: treat mixed paused+complete as Paused already handled;
        // if we reach here assume Queued
        PackageStatus::Queued
    }

    /// Byte-weighted progress 0.0..1.0. Zero total => 0.0 unless all complete.
    pub fn progress(&self) -> f64 {
        let total: u64 = self.members.iter().map(|m| m.total_bytes).sum();
        if total == 0 {
            return if self.status() == PackageStatus::Complete {
                1.0
            } else {
                0.0
            };
        }
        let done: u64 = self.members.iter().map(|m| m.completed_bytes.min(m.total_bytes)).sum();
        (done as f64) / (total as f64)
    }

    pub fn is_terminal(&self) -> bool {
        self.status().is_terminal()
    }
}

/// Queue owns packages in user order. Each package owns its members (and their single active handle).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Queue {
    pub packages: Vec<Package>,
    pub next_package_id: u64,
    pub next_member_id: u64,
}

impl Default for Queue {
    fn default() -> Self {
        Self {
            packages: Vec::new(),
            next_package_id: 1,
            next_member_id: 1,
        }
    }
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_package(
        &mut self,
        name: impl Into<String>,
        target_dir: impl Into<String>,
        members: Vec<(String, String)>,
    ) -> u64 {
        let pid = self.next_package_id;
        self.next_package_id += 1;
        let mut pkg_members = Vec::new();
        for (mname, source) in members {
            let mid = self.next_member_id;
            self.next_member_id += 1;
            pkg_members.push(Member::new(mid, mname, source));
        }
        self.packages.push(Package {
            id: pid,
            name: name.into(),
            target_dir: target_dir.into(),
            created_at_ms: 0,
            members: pkg_members,
        });
        pid
    }

    /// Attach handles to members (e.g. after aria2 addUri responses). Order matches package member order.
    pub fn set_member_handle(&mut self, member_id: u64, handle: String) {
        if let Some(m) = self.find_member_mut(member_id) {
            m.handle = Some(handle);
        }
    }

    pub fn set_member_progress(&mut self, member_id: u64, total: u64, completed: u64) {
        if let Some(m) = self.find_member_mut(member_id) {
            m.total_bytes = total;
            m.completed_bytes = completed;
        }
    }

    pub fn set_member_state(&mut self, member_id: u64, state: MemberState) {
        // terminal states need explicit user action to leave; enforce terminal guard
        if let Some(m) = self.find_member_mut(member_id) {
            if m.state.is_terminal() && !state.is_terminal() {
                // only retry() may exit terminal; direct set is ignored
                return;
            }
            m.state = state;
        }
    }

    pub fn set_member_error(&mut self, member_id: u64, msg: impl Into<String>) {
        if let Some(m) = self.find_member_mut(member_id) {
            m.state = MemberState::Error;
            m.error_message = Some(msg.into());
        }
    }

    /// Overall byte-weighted progress across all packages.
    pub fn overall_progress(&self) -> f64 {
        let total: u64 = self
            .packages
            .iter()
            .flat_map(|p| &p.members)
            .map(|m| m.total_bytes)
            .sum();
        if total == 0 {
            return 0.0;
        }
        let done: u64 = self
            .packages
            .iter()
            .flat_map(|p| &p.members)
            .map(|m| m.completed_bytes.min(m.total_bytes))
            .sum();
        (done as f64) / (total as f64)
    }

    /// Mark a handle that vanished from the engine as gone. Stays in queue.
    pub fn mark_gone_by_handle(&mut self, handle: &str) {
        for pkg in &mut self.packages {
            for m in &mut pkg.members {
                if m.handle.as_deref() == Some(handle) && !m.state.is_terminal() {
                    m.state = MemberState::Gone;
                }
            }
        }
    }

    /// Reconcile against a set of handles known to the engine. Anything previously
    /// tracked but missing becomes Gone. Never auto-prunes.
    pub fn reconcile(&mut self, live_handles: &[String]) {
        for pkg in &mut self.packages {
            for m in &mut pkg.members {
                if let Some(h) = &m.handle {
                    if !live_handles.contains(h) && !m.state.is_terminal() && m.state != MemberState::Complete {
                        m.state = MemberState::Gone;
                    }
                }
            }
        }
    }

    /// Retry swaps the handle and keeps queue position. Resets to Queued.
    /// Returns true if member was found and was terminal.
    pub fn retry_member(&mut self, member_id: u64, new_handle: String) -> bool {
        if let Some(m) = self.find_member_mut(member_id) {
            if !m.state.is_terminal() {
                return false;
            }
            m.handle = Some(new_handle);
            m.state = MemberState::Queued;
            m.error_message = None;
            // keep bytes? reset completed to allow re-download; keep total for weighting
            // completed resets to 0 on retry (preserve total)
            m.completed_bytes = 0;
            return true;
        }
        false
    }

    /// Prune a gone member; if package becomes empty it vanishes.
    pub fn prune_member(&mut self, member_id: u64) -> bool {
        for pkg in &mut self.packages {
            if let Some(pos) = pkg.members.iter().position(|m| m.id == member_id) {
                if pkg.members[pos].state != MemberState::Gone {
                    return false;
                }
                pkg.members.remove(pos);
                break;
            }
        }
        // remove empty packages
        self.packages.retain(|p| !p.members.is_empty());
        true
    }

    pub fn find_member_mut(&mut self, member_id: u64) -> Option<&mut Member> {
        for pkg in &mut self.packages {
            for m in &mut pkg.members {
                if m.id == member_id {
                    return Some(m);
                }
            }
        }
        None
    }

    pub fn find_member(&self, member_id: u64) -> Option<&Member> {
        for pkg in &self.packages {
            for m in &pkg.members {
                if m.id == member_id {
                    return Some(m);
                }
            }
        }
        None
    }

    pub fn find_package(&self, package_id: u64) -> Option<&Package> {
        self.packages.iter().find(|p| p.id == package_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg_with_states(states: Vec<MemberState>) -> Package {
        let members = states
            .into_iter()
            .enumerate()
            .map(|(i, s)| Member {
                id: i as u64 + 1,
                name: format!("m{i}"),
                handle: Some(format!("gid{i}")),
                retry_source: format!("https://example.com/{i}"),
                state: s,
                total_bytes: 100,
                completed_bytes: 0,
                error_message: None,
            })
            .collect();
        Package {
            id: 1,
            name: "p".into(),
            target_dir: "/tmp".into(),
            created_at_ms: 0,
            members,
        }
    }

    #[test]
    fn rollup_error_beats_active() {
        let p = pkg_with_states(vec![MemberState::Error, MemberState::Downloading]);
        assert_eq!(p.status(), PackageStatus::Error);
    }

    #[test]
    fn rollup_active_beats_paused() {
        let p = pkg_with_states(vec![MemberState::Downloading, MemberState::Paused]);
        assert_eq!(p.status(), PackageStatus::Active);
    }

    #[test]
    fn rollup_paused_beats_complete() {
        let p = pkg_with_states(vec![MemberState::Paused, MemberState::Complete]);
        assert_eq!(p.status(), PackageStatus::Paused);
    }

    #[test]
    fn rollup_gone_beats_active() {
        let p = pkg_with_states(vec![MemberState::Gone, MemberState::Downloading]);
        assert_eq!(p.status(), PackageStatus::Gone);
    }

    #[test]
    fn rollup_all_complete() {
        let p = pkg_with_states(vec![MemberState::Complete, MemberState::Complete]);
        assert_eq!(p.status(), PackageStatus::Complete);
    }

    #[test]
    fn progress_weighted_by_bytes() {
        let mut p = pkg_with_states(vec![MemberState::Downloading, MemberState::Downloading]);
        p.members[0].total_bytes = 1000;
        p.members[0].completed_bytes = 500;
        p.members[1].total_bytes = 100;
        p.members[1].completed_bytes = 100;
        // 600/1100
        let prog = p.progress();
        assert!((prog - 600.0 / 1100.0).abs() < 1e-9);
    }

    #[test]
    fn queue_progress_weighted() {
        let mut q = Queue::new();
        q.add_package("a", "/tmp", vec![("m1".into(), "src1".into())]);
        q.add_package("b", "/tmp", vec![("m2".into(), "src2".into())]);
        let m1 = q.packages[0].members[0].id;
        let m2 = q.packages[1].members[0].id;
        q.set_member_progress(m1, 1000, 1000);
        q.set_member_progress(m2, 100, 0);
        // 1000/1100
        assert!((q.overall_progress() - 1000.0 / 1100.0).abs() < 1e-9);
    }

    #[test]
    fn stale_handle_moves_to_gone_stays_in_queue() {
        let mut q = Queue::new();
        q.add_package("p", "/tmp", vec![("m".into(), "src".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_handle(mid, "gid1".into());
        q.set_member_state(mid, MemberState::Downloading);
        q.mark_gone_by_handle("gid1");
        assert_eq!(q.find_member(mid).unwrap().state, MemberState::Gone);
        assert_eq!(q.packages.len(), 1);
        assert_eq!(q.packages[0].members.len(), 1);
    }

    #[test]
    fn retry_swaps_handle_keeps_position() {
        let mut q = Queue::new();
        q.add_package(
            "p",
            "/tmp",
            vec![
                ("a".into(), "src-a".into()),
                ("b".into(), "src-b".into()),
                ("c".into(), "src-c".into()),
            ],
        );
        let ids: Vec<u64> = q.packages[0].members.iter().map(|m| m.id).collect();
        let mid = ids[1];
        q.set_member_handle(mid, "old".into());
        q.set_member_error(mid, "boom");
        let ok = q.retry_member(mid, "new".into());
        assert!(ok);
        let m = q.find_member(mid).unwrap();
        assert_eq!(m.handle.as_deref(), Some("new"));
        assert_eq!(m.state, MemberState::Queued);
        // position preserved
        assert_eq!(q.packages[0].members[1].id, mid);
        // order unchanged
        assert_eq!(
            q.packages[0].members.iter().map(|m| m.id).collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn gone_and_error_are_terminal_need_explicit_action() {
        let mut q = Queue::new();
        q.add_package("p", "/tmp", vec![("m".into(), "src".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_error(mid, "fail");
        // direct state change out of terminal is blocked
        q.set_member_state(mid, MemberState::Downloading);
        assert_eq!(q.find_member(mid).unwrap().state, MemberState::Error);
        // retry is the explicit action
        assert!(q.retry_member(mid, "gid2".into()));
        assert_eq!(q.find_member(mid).unwrap().state, MemberState::Queued);

        q.set_member_state(mid, MemberState::Gone);
        // Gone is also terminal (set via mark); but via retry path also works
        // simulate gone via mark
        q.set_member_handle(mid, "gid2".into());
        q.mark_gone_by_handle("gid2");
        // Now gone, direct transition blocked
        q.set_member_state(mid, MemberState::Complete);
        assert_eq!(q.find_member(mid).unwrap().state, MemberState::Gone);
        assert!(q.retry_member(mid, "gid3".into()));
        assert_eq!(q.find_member(mid).unwrap().state, MemberState::Queued);
    }
}
