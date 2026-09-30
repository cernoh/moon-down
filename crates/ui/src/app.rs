use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use moon_down_core::settings::{format_bytes, load_settings, save_settings, Settings};
use moon_down_core::{MemberState, Queue};

pub const SETTINGS_FIELDS: &[&str] = &[
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
    pub add_input: String,
    pub logs: Vec<String>,
    dirty: bool,
    pending_delete: Option<u64>,
    extract_failed: HashSet<u64>,
    // settings editing
    pub settings: Settings,
    pub settings_selected: usize,
    pub settings_editing: Option<String>,
    pub settings_hints: HashMap<String, String>,
    pub settings_error: Option<String>,
    pub settings_pending_rpc: Option<(String, String)>,
    settings_config_path: Option<PathBuf>,
}

impl App {
    pub fn new(queue: Queue) -> Self {
        Self {
            queue,
            view: View::Queue,
            selected: 0,
            expanded: HashSet::new(),
            show_add_modal: false,
            add_input: String::new(),
            logs: Vec::new(),
            dirty: true,
            pending_delete: None,
            extract_failed: HashSet::new(),
            settings: Settings::default(),
            settings_selected: 0,
            settings_editing: None,
            settings_hints: HashMap::new(),
            settings_error: None,
            settings_pending_rpc: None,
            settings_config_path: None,
        }
    }

    pub fn init_settings(&mut self, state_dir: &std::path::Path) {
        let cfg = state_dir.join("settings.json");
        self.settings = load_settings(&cfg, state_dir).unwrap_or_else(|_| Settings::default_for(state_dir));
        self.settings_config_path = Some(cfg);
        self.dirty = true;
    }

    pub fn settings_fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("max_concurrent_downloads", self.settings.max_concurrent_downloads.to_string()),
            ("download_limit", self.settings.max_overall_download_limit.map(format_bytes).unwrap_or_default()),
            ("upload_limit", self.settings.max_overall_upload_limit.map(format_bytes).unwrap_or_default()),
            ("split", self.settings.split.to_string()),
            ("connections_per_server", self.settings.max_connection_per_server.to_string()),
            ("min_split_size", format_bytes(self.settings.min_split_size)),
            ("dir", self.settings.dir.display().to_string()),
            ("max_tries", self.settings.max_tries.to_string()),
            ("retry_wait", self.settings.retry_wait.to_string()),
        ]
    }

    pub fn take_settings_rpc(&mut self) -> Option<(String, String)> {
        self.settings_pending_rpc.take()
    }

    fn settings_current_value(&self, field: &str) -> String {
        self.settings_fields().into_iter().find(|(k, _)| *k == field).map(|(_, v)| v).unwrap_or_default()
    }

    fn persist_settings(&self) {
        if let Some(p) = &self.settings_config_path {
            let _ = save_settings(&self.settings, p);
        }
    }

    fn commit_settings_field(&mut self, field: &str, raw: &str) {
        self.settings_error = None;
        let hint: Option<String>;
        let res: Result<Option<String>, String> = match field {
            "max_concurrent_downloads" => {
                let n: i64 = raw.trim().parse().unwrap_or(self.settings.max_concurrent_downloads as i64);
                hint = self.settings.set_max_concurrent_downloads(n);
                if let Some(h) = hint.clone() { self.settings_hints.insert(field.into(), h); } else { self.settings_hints.remove(field); }
                Ok(hint)
            }
            "download_limit" => self.settings.set_download_limit_str(raw),
            "upload_limit" => self.settings.set_upload_limit_str(raw),
            "split" => {
                let n: i64 = raw.trim().parse().unwrap_or(self.settings.split as i64);
                hint = self.settings.set_split(n);
                if let Some(h) = hint.clone() { self.settings_hints.insert(field.into(), h); } else { self.settings_hints.remove(field); }
                Ok(hint)
            }
            "connections_per_server" => {
                let n: i64 = raw.trim().parse().unwrap_or(self.settings.max_connection_per_server as i64);
                hint = self.settings.set_max_connection_per_server(n);
                if let Some(h) = hint.clone() { self.settings_hints.insert(field.into(), h); } else { self.settings_hints.remove(field); }
                Ok(hint)
            }
            "min_split_size" => self.settings.set_min_split_size_str(raw),
            "dir" => match self.settings.set_dir(raw) {
                Ok(()) => { self.settings_hints.remove(field); Ok(None) }
                Err(e) => Err(e),
            },
            "max_tries" => {
                let n: i64 = raw.trim().parse().unwrap_or(self.settings.max_tries as i64);
                hint = self.settings.set_max_tries(n);
                if let Some(h) = hint.clone() { self.settings_hints.insert(field.into(), h); } else { self.settings_hints.remove(field); }
                Ok(hint)
            }
            "retry_wait" => {
                let n: i64 = raw.trim().parse().unwrap_or(self.settings.retry_wait as i64);
                hint = self.settings.set_retry_wait(n);
                if let Some(h) = hint.clone() { self.settings_hints.insert(field.into(), h); } else { self.settings_hints.remove(field); }
                Ok(hint)
            }
            _ => Ok(None),
        };
        match res {
            Ok(maybe_hint) => {
                if let Some(h) = maybe_hint { self.settings_hints.insert(field.into(), h); }
                // limits/min_split manage hints themselves; for download/upload clear hint on success
                if matches!(field, "download_limit" | "upload_limit") { self.settings_hints.remove(field); }
                if field == "min_split_size" { /* set_min_split already handled hint above via Ok */ }
                self.persist_settings();
                let pair = self.settings.wire_pair(field);
                self.settings_pending_rpc = Some(pair.clone());
                if let Some(h) = self.settings_hints.get(field) {
                    self.logs.push(format!("{} set to {} ({})", field, pair.1, h));
                } else {
                    self.logs.push(format!("{} set to {}", field, pair.1));
                }
            }
            Err(e) => {
                self.settings_error = Some(e.clone());
                self.logs.push(format!("{} error: {}", field, e));
            }
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

        // Settings inline editing: when editing, capture typing first
        if self.view == View::Settings && self.settings_editing.is_some() {
            match key {
                '\x1b' => { self.settings_editing = None; self.settings_error = None; self.dirty = true; return true; }
                '\n' | '\r' => {
                    let buf = self.settings_editing.take().unwrap();
                    let field = SETTINGS_FIELDS[self.settings_selected].to_string();
                    self.commit_settings_field(&field, &buf);
                    self.dirty = true;
                    return true;
                }
                '\x7f' => {
                    if let Some(b) = &mut self.settings_editing { b.pop(); }
                    self.dirty = true;
                    return true;
                }
                c if !c.is_control() => {
                    if let Some(b) = &mut self.settings_editing { b.push(c); }
                    self.dirty = true;
                    return true;
                }
                _ => return false,
            }
        }

        // Settings navigation (when not editing)
        if self.view == View::Settings {
            match key {
                'j' => {
                    if self.settings_selected + 1 < SETTINGS_FIELDS.len() { self.settings_selected += 1; self.dirty = true; return true; }
                    return false;
                }
                'k' => {
                    if self.settings_selected > 0 { self.settings_selected -= 1; self.dirty = true; return true; }
                    return false;
                }
                '\n' | '\r' => {
                    let field = SETTINGS_FIELDS[self.settings_selected];
                    let cur = self.settings_current_value(field);
                    self.settings_editing = Some(cur);
                    self.settings_error = None;
                    self.dirty = true;
                    return true;
                }
                '\x1b' => { self.settings_editing = None; self.settings_error = None; self.dirty = true; return true; }
                _ => {}
            }
        }

        // 1-6 view jumps (not while editing — handled above)
        if let Some(v) = View::from_key(key) {
            if self.view != v {
                // cancel any pending settings edit on view leave
                if self.view == View::Settings { self.settings_editing = None; }
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
        self.show_add_modal || self.settings_editing.is_some()
    }

    // ---- vim-style movement ----
    pub fn move_top(&mut self) {
        if self.view == View::Settings {
            if self.settings_selected != 0 { self.settings_selected = 0; self.dirty = true; }
            return;
        }
        if self.selected != 0 {
            self.selected = 0;
            self.dirty = true;
        }
    }
    pub fn move_bottom(&mut self) {
        if self.view == View::Settings {
            let n = SETTINGS_FIELDS.len();
            if self.settings_selected + 1 != n { self.settings_selected = n - 1; self.dirty = true; }
            return;
        }
        let n = self.flat_rows().len();
        if n > 0 && self.selected + 1 != n {
            self.selected = n - 1;
            self.dirty = true;
        }
    }
    pub fn page_down(&mut self, lines: usize) {
        if self.view == View::Settings {
            let n = SETTINGS_FIELDS.len();
            let next = (self.settings_selected + lines).min(n - 1);
            if next != self.settings_selected { self.settings_selected = next; self.dirty = true; }
            return;
        }
        let n = self.flat_rows().len();
        if n == 0 { return; }
        let next = (self.selected + lines).min(n - 1);
        if next != self.selected { self.selected = next; self.dirty = true; }
    }
    pub fn page_up(&mut self, lines: usize) {
        if self.view == View::Settings {
            let next = self.settings_selected.saturating_sub(lines);
            if next != self.settings_selected { self.settings_selected = next; self.dirty = true; }
            return;
        }
        let next = self.selected.saturating_sub(lines);
        if next != self.selected { self.selected = next; self.dirty = true; }
    }
    pub fn move_by(&mut self, delta: isize) {
        if delta > 0 { self.page_down(delta as usize); } else if delta < 0 { self.page_up((-delta) as usize); }
    }

    // ---- add modal input ----
    pub fn add_input_push(&mut self, c: char) { self.add_input.push(c); self.dirty = true; }
    pub fn add_input_pop(&mut self) { if self.add_input.pop().is_some() { self.dirty = true; } }
    pub fn add_input_clear(&mut self) { if !self.add_input.is_empty() { self.add_input.clear(); self.dirty = true; } }
    pub fn take_add_input(&mut self) -> String { let s = std::mem::take(&mut self.add_input); self.dirty = true; s }
    pub fn submit_add_uris(&mut self, uris: Vec<String>, target_dir: String) -> Option<u64> {
        if uris.is_empty() { return None; }
        let entries: Vec<(String,String)> = uris.iter().map(|u| {
            let name = u.rsplit('/').next().unwrap_or("download").split(['?','#']).next().unwrap_or("download").to_string();
            let name = if name.is_empty() { "download".into() } else { name };
            (name, u.clone())
        }).collect();
        let pkg_name = entries[0].0.clone();
        let pkg_id = self.queue.add_package(&pkg_name, &target_dir, entries);
        self.dirty = true;
        self.clamp_selection();
        Some(pkg_id)
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
