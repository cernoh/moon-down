use std::collections::HashSet;

use moon_down_core::{MemberState, Queue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Queue,
    Detail,
    Accounts,
    Settings,
    Log,
    Help,
}

impl View {
    pub fn from_key(c: char) -> Option<Self> {
        match c {
            '1' => Some(Self::Queue),
            '2' => Some(Self::Detail),
            '3' => Some(Self::Accounts),
            '4' => Some(Self::Settings),
            '5' => Some(Self::Log),
            '6' => Some(Self::Help),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum RowKind {
    Package { pkg_id: u64 },
    Member { pkg_id: u64, member_id: u64 },
}

pub struct App {
    pub queue: Queue,
    pub view: View,
    pub selected: usize,
    pub expanded: HashSet<u64>,
    pub show_add_modal: bool,
    pub logs: Vec<String>,
    dirty: bool,
    pending_delete: Option<u64>,
    extract_failed: HashSet<u64>,
}

impl App {
    pub fn new(queue: Queue) -> Self {
        Self {
            queue,
            view: View::Queue,
            selected: 0,
            expanded: HashSet::new(),
            show_add_modal: false,
            logs: Vec::new(),
            dirty: true,
            pending_delete: None,
            extract_failed: HashSet::new(),
        }
    }

    pub fn needs_render(&self) -> bool {
        self.dirty
    }
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }
    /// For the single-owner event loop: returns true if a render is needed and clears flag.
    pub fn take_dirty(&mut self) -> bool {
        let d = self.dirty;
        self.dirty = false;
        d
    }

    pub fn flat_rows(&self) -> Vec<RowKind> {
        let mut rows = Vec::new();
        for pkg in &self.queue.packages {
            rows.push(RowKind::Package { pkg_id: pkg.id });
            if self.expanded.contains(&pkg.id) {
                for m in &pkg.members {
                    rows.push(RowKind::Member { pkg_id: pkg.id, member_id: m.id });
                }
            }
        }
        rows
    }

    pub fn selected_row(&self) -> Option<RowKind> {
        self.flat_rows().get(self.selected).cloned()
    }

    pub fn selected_package_id(&self) -> Option<u64> {
        match self.selected_row()? {
            RowKind::Package { pkg_id } => Some(pkg_id),
            RowKind::Member { pkg_id, .. } => Some(pkg_id),
        }
    }

    fn clamp_selection(&mut self) {
        let n = self.flat_rows().len();
        if n == 0 {
            self.selected = 0;
        } else if self.selected >= n {
            self.selected = n - 1;
        }
    }

    /// Returns true if state changed (dirty).
    pub fn handle_key(&mut self, key: char) -> bool {
        // Modal consumes keys first: Esc closes, Enter submits (close for skeleton)
        if self.show_add_modal {
            if key == '\x1b' || key == 'q' {
                self.show_add_modal = false;
                self.dirty = true;
                return true;
            }
            if key == '\n' || key == '\r' {
                self.show_add_modal = false;
                self.dirty = true;
                return true;
            }
            // while modal open, ignore other keys except Esc
            return false;
        }

        // 1-6 view jumps
        if let Some(v) = View::from_key(key) {
            if self.view != v {
                self.view = v;
                self.dirty = true;
                return true;
            }
            return false;
        }

        match key {
            'j' => {
                let n = self.flat_rows().len();
                if n > 0 && self.selected + 1 < n {
                    self.selected += 1;
                    self.dirty = true;
                    return true;
                }
                false
            }
            'k' => {
                if self.selected > 0 {
                    self.selected -= 1;
                    self.dirty = true;
                    return true;
                }
                false
            }
            'h' => {
                if let Some(pid) = self.selected_package_id() {
                    if self.expanded.remove(&pid) {
                        self.dirty = true;
                        self.clamp_selection();
                        return true;
                    }
                }
                false
            }
            'l' => {
                if let Some(pid) = self.selected_package_id() {
                    if self.expanded.insert(pid) {
                        self.dirty = true;
                        return true;
                    }
                }
                false
            }
            '\n' | '\r' => {
                // Enter toggles expand/collapse
                if let Some(pid) = self.selected_package_id() {
                    if self.expanded.contains(&pid) {
                        self.expanded.remove(&pid);
                    } else {
                        self.expanded.insert(pid);
                    }
                    self.dirty = true;
                    self.clamp_selection();
                    return true;
                }
                false
            }
            ' ' => {
                // space: pause/resume package
                if let Some(pid) = self.selected_package_id() {
                    if let Some(pkg) = self.queue.packages.iter_mut().find(|p| p.id == pid) {
                        let is_paused = pkg.members.iter().any(|m| m.state == MemberState::Paused);
                        if is_paused {
                            for m in &mut pkg.members {
                                if m.state == MemberState::Paused {
                                    m.state = MemberState::Queued;
                                }
                            }
                        } else {
                            for m in &mut pkg.members {
                                if matches!(m.state, MemberState::Queued | MemberState::Downloading) {
                                    m.state = MemberState::Paused;
                                }
                            }
                        }
                        self.dirty = true;
                        return true;
                    }
                }
                false
            }
            'd' => {
                // remove keeping files
                if let Some(pid) = self.selected_package_id() {
                    let before = self.queue.packages.len();
                    self.queue.packages.retain(|p| p.id != pid);
                    // also clean expanded
                    self.expanded.remove(&pid);
                    self.pending_delete = None;
                    if self.queue.packages.len() != before {
                        self.dirty = true;
                        self.clamp_selection();
                        return true;
                    }
                }
                false
            }
            'D' => {
                // delete with files: requires confirm (two presses)
                if let Some(pid) = self.selected_package_id() {
                    if self.pending_delete == Some(pid) {
                        self.queue.packages.retain(|p| p.id != pid);
                        self.expanded.remove(&pid);
                        self.pending_delete = None;
                        self.dirty = true;
                        self.clamp_selection();
                        return true;
                    } else {
                        self.pending_delete = Some(pid);
                        self.dirty = true;
                        return true;
                    }
                }
                false
            }
            'r' => {
                // retry error/gone members in selected package, keep position
                if let Some(pid) = self.selected_package_id() {
                    let mut changed = false;
                    // collect member ids to retry
                    let mids: Vec<u64> = self
                        .queue
                        .packages
                        .iter()
                        .find(|p| p.id == pid)
                        .map(|p| {
                            p.members
                                .iter()
                                .filter(|m| m.state.is_terminal())
                                .map(|m| m.id)
                                .collect()
                        })
                        .unwrap_or_default();
                    for mid in mids {
                        let new_handle = format!("retry-{mid}-{}", self.queue.next_member_id);
                        if self.queue.retry_member(mid, new_handle) {
                            changed = true;
                        }
                    }
                    if changed {
                        self.dirty = true;
                        return true;
                    }
                }
                false
            }
            'x' => {
                // prune gone rows in selected package
                if let Some(pid) = self.selected_package_id() {
                    let mids: Vec<u64> = self
                        .queue
                        .packages
                        .iter()
                        .find(|p| p.id == pid)
                        .map(|p| {
                            p.members
                                .iter()
                                .filter(|m| m.state == MemberState::Gone)
                                .map(|m| m.id)
                                .collect()
                        })
                        .unwrap_or_default();
                    if mids.is_empty() {
                        return false;
                    }
                    let mut changed = false;
                    for mid in mids {
                        if self.queue.prune_member(mid) {
                            changed = true;
                        }
                    }
                    if changed {
                        // also remove expanded entry if package vanished
                        if self.queue.find_package(pid).is_none() {
                            self.expanded.remove(&pid);
                        }
                        self.clamp_selection();
                        self.dirty = true;
                        return true;
                    }
                }
                false
            }
            'e' => {
                // retry extraction: clear extract_failed for selected package and retry its error members
                if let Some(pid) = self.selected_package_id() {
                    let had_failed = self.extract_failed.remove(&pid);
                    // Also treat as retry for error members (same as r)
                    let mids: Vec<u64> = self
                        .queue
                        .packages
                        .iter()
                        .find(|p| p.id == pid)
                        .map(|p| {
                            p.members
                                .iter()
                                .filter(|m| m.state == MemberState::Error)
                                .map(|m| m.id)
                                .collect()
                        })
                        .unwrap_or_default();
                    let mut changed = had_failed;
                    for mid in mids {
                        let new_handle = format!("extract-retry-{mid}-{}", self.queue.next_member_id);
                        if self.queue.retry_member(mid, new_handle) {
                            changed = true;
                        }
                    }
                    if changed || had_failed {
                        self.dirty = true;
                        return true;
                    }
                    // even if nothing to retry, e is still a valid key that does nothing but is handled
                    // return false to indicate no state change
                    return false;
                }
                false
            }
            'a' => {
                self.show_add_modal = true;
                self.dirty = true;
                return true;
            }
            _ => false,
        }
    }

    pub fn push_log(&mut self, line: impl Into<String>) {
        self.logs.push(line.into());
        if self.logs.len() > 500 {
            let drain = self.logs.len() - 500;
            self.logs.drain(0..drain);
        }
        self.dirty = true;
    }

    pub fn mark_extract_failed(&mut self, pkg_id: u64) {
        self.extract_failed.insert(pkg_id);
        self.dirty = true;
    }

    pub fn is_modal_open(&self) -> bool {
        self.show_add_modal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moon_down_core::Queue;

    fn sample_queue() -> Queue {
        let mut q = Queue::new();
        q.add_package("pkg-a", "/tmp", vec![("f1".into(), "https://ex.com/1".into())]);
        q.add_package("pkg-b", "/tmp", vec![("f2".into(), "https://ex.com/2".into())]);
        q.add_package("pkg-c", "/tmp", vec![("f3".into(), "https://ex.com/3".into())]);
        q
    }

    #[test]
    fn keys_1_through_6_switch_views() {
        let mut app = App::new(Queue::new());
        // start at Queue, so '1' is no-op
        assert_eq!(app.view, View::Queue);
        assert!(!app.handle_key('1'));
        for (k, expected) in [('2', View::Detail), ('3', View::Accounts), ('4', View::Settings), ('5', View::Log), ('6', View::Help), ('1', View::Queue)] {
            let dirty = app.handle_key(k);
            assert!(dirty, "key {k} should be dirty");
            assert_eq!(app.view, expected);
            assert!(app.needs_render());
            app.clear_dirty();
        }
    }

    #[test]
    fn j_k_move_cursor() {
        let mut app = App::new(sample_queue());
        assert_eq!(app.selected, 0);
        assert!(app.handle_key('j'));
        assert_eq!(app.selected, 1);
        assert!(app.handle_key('k'));
        assert_eq!(app.selected, 0);
        // k at top stays
        assert!(!app.handle_key('k'));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn expand_collapse_with_l_h_enter() {
        let mut app = App::new(sample_queue());
        let pid = app.queue.packages[0].id;
        assert!(!app.expanded.contains(&pid));
        assert!(app.handle_key('l'));
        assert!(app.expanded.contains(&pid));
        assert!(app.handle_key('h'));
        assert!(!app.expanded.contains(&pid));
        // Enter toggles
        assert!(app.handle_key('\n'));
        assert!(app.expanded.contains(&pid));
        assert!(app.handle_key('\n'));
        assert!(!app.expanded.contains(&pid));
    }

    #[test]
    fn space_pauses_and_resumes_package() {
        let mut q = Queue::new();
        q.add_package("pkg", "/tmp", vec![("f".into(), "src".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_state(mid, MemberState::Downloading);
        let mut app = App::new(q);
        assert!(app.handle_key(' '));
        assert_eq!(app.queue.find_member(mid).unwrap().state, MemberState::Paused);
        assert!(app.handle_key(' '));
        assert_eq!(app.queue.find_member(mid).unwrap().state, MemberState::Queued);
    }

    #[test]
    fn d_removes_package() {
        let mut app = App::new(sample_queue());
        let first_id = app.queue.packages[0].id;
        assert!(app.handle_key('d'));
        assert!(app.queue.find_package(first_id).is_none());
    }

    #[test]
    fn big_d_requires_confirm() {
        let mut app = App::new(sample_queue());
        let first_id = app.queue.packages[0].id;
        assert!(app.handle_key('D')); // first press arms
        assert!(app.queue.find_package(first_id).is_some());
        assert!(app.handle_key('D')); // second deletes
        assert!(app.queue.find_package(first_id).is_none());
    }

    #[test]
    fn r_retries_error_row_keeping_position() {
        let mut q = Queue::new();
        q.add_package("pkg", "/tmp", vec![("a".into(), "src-a".into()), ("b".into(), "src-b".into()), ("c".into(), "src-c".into())]);
        let ids: Vec<u64> = q.packages[0].members.iter().map(|m| m.id).collect();
        let mid = ids[1];
        q.set_member_handle(mid, "old".into());
        q.set_member_error(mid, "boom");
        let mut app = App::new(q);
        assert!(app.handle_key('r'));
        let m = app.queue.find_member(mid).unwrap();
        assert_eq!(m.state, MemberState::Queued);
        assert!(m.handle.as_deref().unwrap().starts_with("retry-"));
        // position preserved
        assert_eq!(app.queue.packages[0].members[1].id, mid);
    }

    #[test]
    fn x_prunes_gone_rows() {
        let mut q = Queue::new();
        q.add_package("pkg", "/tmp", vec![("f".into(), "src".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_handle(mid, "gid1".into());
        q.set_member_state(mid, MemberState::Downloading);
        q.mark_gone_by_handle("gid1");
        let mut app = App::new(q);
        assert!(app.handle_key('x'));
        // package vanishes because last member pruned
        assert!(app.queue.packages.is_empty());
    }

    #[test]
    fn e_retries_extract() {
        let mut q = Queue::new();
        q.add_package("pkg", "/tmp", vec![("f".into(), "src".into())]);
        let pid = q.packages[0].id;
        let mid = q.packages[0].members[0].id;
        q.set_member_error(mid, "extract fail");
        let mut app = App::new(q);
        app.mark_extract_failed(pid);
        assert!(app.extract_failed.contains(&pid));
        assert!(app.handle_key('e'));
        assert!(!app.extract_failed.contains(&pid));
    }

    #[test]
    fn a_opens_add_modal_centered() {
        let mut app = App::new(Queue::new());
        assert!(!app.show_add_modal);
        assert!(app.handle_key('a'));
        assert!(app.show_add_modal);
        // Esc closes
        assert!(app.handle_key('\x1b'));
        assert!(!app.show_add_modal);
    }

    #[test]
    fn render_only_on_state_change() {
        let mut app = App::new(Queue::new());
        // new app starts dirty
        assert!(app.take_dirty());
        assert!(!app.needs_render());
        // no change on unknown key
        assert!(!app.handle_key('z'));
        assert!(!app.needs_render());
        // j on empty does nothing
        assert!(!app.handle_key('j'));
        assert!(!app.needs_render());
        // adding log dirties
        app.push_log("hello");
        assert!(app.needs_render());
        assert!(app.take_dirty());
        assert!(!app.needs_render());
    }

    #[test]
    fn single_owner_event_loop_pattern() {
        // One App owned mutably in one loop; state only mutated via handle_key.
        let mut app = App::new(sample_queue());
        app.clear_dirty();
        let events = vec!['j', 'l', 'j'];
        let mut renders = 0;
        for k in events {
            if app.handle_key(k) {
                // only render when dirty
                if app.take_dirty() {
                    renders += 1;
                }
            }
        }
        assert!(renders > 0);
        assert!(!app.needs_render());
    }
}
