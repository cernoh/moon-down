use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use moon_down_core::{MemberState, PackageStatus};

use crate::app::{App, RowKind, View};

fn status_badge(s: &PackageStatus) -> &'static str {
    match s {
        PackageStatus::Error => "ERR",
        PackageStatus::Gone => "GONE",
        PackageStatus::Active => "DL",
        PackageStatus::Paused => "PAUSED",
        PackageStatus::Complete => "DONE",
        PackageStatus::Queued => "QUEUED",
    }
}

fn member_state_label(s: &MemberState) -> &'static str {
    match s {
        MemberState::Queued => "queued",
        MemberState::Downloading => "downloading",
        MemberState::Paused => "paused",
        MemberState::Complete => "complete",
        MemberState::Error => "error",
        MemberState::Gone => "gone",
    }
}

pub fn render(app: &App, frame: &mut Frame) {
    render_with(app, frame, "", false)
}

/// Row heights and the accent colour, all derived from the design seed (see `main.rs`).
/// Keeping them here means tests and the binary agree on the geometry.
pub const DETAIL_ROWS: u16 = 6;
pub const LOG_ROWS: u16 = 5;
pub const HISTORY_ROWS: u16 = 7;
pub const ACCENT: Color = Color::Rgb(214, 69, 217);

/// Full render with an optional one-line status bar and a history pane of
/// already-completed packages. `status` empty and `show_history` false gives the
/// plain three-pane layout.
pub fn render_with(app: &App, frame: &mut Frame, status: &str, show_history: bool) {
    let area = frame.area();

    let mut constraints = vec![Constraint::Min(8)]; // top pane: queue always visible
    if !status.is_empty() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(DETAIL_ROWS));
    if show_history {
        constraints.push(Constraint::Length(HISTORY_ROWS));
    }
    constraints.push(Constraint::Length(LOG_ROWS));

    let mut sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area)
        .to_vec();

    let mut idx = 0;
    let top = sections[idx];
    idx += 1;
    if !status.is_empty() {
        render_status(frame, sections[idx], status);
        idx += 1;
    }
    let detail = sections[idx];
    idx += 1;
    if show_history {
        render_history(app, frame, sections[idx]);
        idx += 1;
    }
    let log = sections[idx];

    render_top(app, frame, top);
    render_detail(app, frame, detail);
    render_log(app, frame, log);

    if app.show_add_modal {
        render_add_modal(frame, area);
    }
}

fn render_status(frame: &mut Frame, area: Rect, status: &str) {
    let line = Line::from(vec![
        Span::styled(" moon-down ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(status, Style::default().fg(Color::Gray)),
    ]);
    frame.render_widget(Paragraph::new(line).style(Style::default().bg(Color::Black)), area);
}

/// Everything already finished, so previously-downloaded work stays visible.
fn render_history(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" History — already downloaded ");

    let done: Vec<&moon_down_core::Package> = app
        .queue
        .packages
        .iter()
        .filter(|p| p.status() == PackageStatus::Complete)
        .collect();

    if done.is_empty() {
        frame.render_widget(
            Paragraph::new("(nothing completed yet)").block(block),
            area,
        );
        return;
    }

    let lines: Vec<Line> = done
        .iter()
        .map(|p| {
            let bytes: u64 = p.members.iter().map(|m| m.total_bytes).sum();
            Line::from(Span::styled(
                format!(" ✓ {}  {}  {} member(s)", p.name, human_bytes(bytes), p.members.len()),
                Style::default().fg(ACCENT),
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn render_top(app: &App, frame: &mut Frame, area: Rect) {
    // Build title reflecting view but queue data stays visible in body.
    let title = match app.view {
        View::Queue => " Queue [1] ",
        View::Detail => " Queue — Detail focus [2] ",
        View::Accounts => " Accounts [3] — Queue below ",
        View::Settings => " Settings [4] — Queue below ",
        View::Log => " Queue — Log focus [5] ",
        View::Help => " Help [6] ",
    };
    let block = Block::default().borders(Borders::ALL).title(title);

    let rows = app.flat_rows();
    let mut lines: Vec<Line> = Vec::new();

    // When view is Accounts/Settings, prepend a header line for that view
    // but keep queue rows visible underneath to satisfy "queue stays visible".
    match app.view {
        View::Accounts => {
            lines.push(Line::from(Span::styled(
                " Accounts — plugin | host | user | enabled  (a add, t test, space toggle) ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(" ─────────────────────────────────────────── "));
        }
        View::Settings => {
            lines.push(Line::from(Span::styled(
                " Settings — Queue/Speed/Threads/Paths  (arrows + Enter apply) ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(" ─────────────────────────────────────────── "));
        }
        _ => {}
    }

    if rows.is_empty() {
        lines.push(Line::from(" (empty queue)  press a to add "));
    } else {
        for (idx, row) in rows.iter().enumerate() {
            let selected = idx == app.selected;
            let prefix = if selected { "▶ " } else { "  " };
            match row {
                RowKind::Package { pkg_id } => {
                    let pkg = app.queue.find_package(*pkg_id).unwrap();
                    let st = pkg.status();
                    let prog = (pkg.progress() * 100.0) as u64;
                    let badge = status_badge(&st);
                    let bar = progress_bar(prog);
                    let n_done = pkg.members.iter().filter(|m| m.state == MemberState::Complete).count();
                    let n_total = pkg.members.len();
                    let expanded = app.expanded.contains(pkg_id);
                    let exp_mark = if expanded { "▼" } else { "▶" };
                    let line_str = format!(
                        "{}{} {} [{}] {} {}/{} {}%",
                        prefix, exp_mark, pkg.name, badge, bar, n_done, n_total, prog
                    );
                    let mut style = Style::default();
                    if selected {
                        style = style.bg(Color::DarkGray).fg(Color::White);
                    }
                    lines.push(Line::from(Span::styled(line_str, style)));
                }
                RowKind::Member { member_id, .. } => {
                    let m = app.queue.find_member(*member_id).unwrap();
                    let state = member_state_label(&m.state);
                    let prog = if m.total_bytes == 0 {
                        0
                    } else {
                        (m.completed_bytes.min(m.total_bytes) * 100 / m.total_bytes.max(1)) as u64
                    };
                    let mut suffix = String::new();
                    match m.state {
                        MemberState::Error => {
                            let msg = m.error_message.as_deref().unwrap_or("");
                            suffix = format!("  {}  [r]etry", msg);
                        }
                        MemberState::Gone => {
                            suffix = "  [r]e-add [x]prune".to_string();
                        }
                        _ => {}
                    }
                    let line_str = format!("{}  └ {} [{}] {}%{}", prefix, m.name, state, prog, suffix);
                    let mut style = Style::default().fg(Color::Gray);
                    if selected {
                        style = style.bg(Color::DarkGray).fg(Color::White);
                    }
                    lines.push(Line::from(Span::styled(line_str, style)));
                }
            }
        }
    }

    let para = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

fn progress_bar(pct: u64) -> String {
    let filled = (pct / 10).min(10) as usize;
    let empty = 10 - filled;
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

fn render_detail(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Detail ");
    let content = if let Some(pid) = app.selected_package_id() {
        if let Some(pkg) = app.queue.find_package(pid) {
            let st = pkg.status();
            let prog = (pkg.progress() * 100.0) as u64;
            let mut s = format!(" {}  [{}]  {}%  {}/{} complete\n", pkg.name, status_badge(&st), prog, pkg.members.iter().filter(|m| m.state==MemberState::Complete).count(), pkg.members.len());
            s.push_str(&format!(" dir: {}\n", pkg.target_dir));
            for m in &pkg.members {
                let err = m.error_message.as_deref().unwrap_or("");
                s.push_str(&format!("  - {} [{}] {}/{} {}\n", m.name, member_state_label(&m.state), m.completed_bytes, m.total_bytes, err));
            }
            // inline hints for detail too
            if matches!(st, PackageStatus::Error) {
                s.push_str(" press r to retry\n");
            }
            if matches!(st, PackageStatus::Gone) {
                s.push_str(" press r to re-add, x to prune\n");
            }
            s
        } else {
            " (no package) ".into()
        }
    } else {
        " (no selection) ".into()
    };
    let para = Paragraph::new(content).block(block).wrap(Wrap { trim: true });
    frame.render_widget(para, area);
}

fn render_log(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Log tail ");
    let tail: Vec<Line> = app
        .logs
        .iter()
        .rev()
        .take(area.height as usize)
        .rev()
        .map(|l| Line::from(l.as_str()))
        .collect();
    let para = if tail.is_empty() {
        Paragraph::new("(no logs)").block(block)
    } else {
        Paragraph::new(tail).block(block).wrap(Wrap { trim: false })
    };
    frame.render_widget(para, area);
}

fn render_add_modal(frame: &mut Frame, area: Rect) {
    let modal_w = 60.min(area.width.saturating_sub(4));
    let modal_h = 14.min(area.height.saturating_sub(4));
    let x = (area.width - modal_w) / 2;
    let y = (area.height - modal_h) / 2;
    let modal_area = Rect::new(x, y, modal_w, modal_h);
    frame.render_widget(Clear, modal_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Add downloads (Esc to close, Enter to submit) ")
        .style(Style::default().bg(Color::Black).fg(Color::White));
    let content = Paragraph::new(vec![
        Line::from(" URIs / magnets (one per line):"),
        Line::from(" ┌─────────────────────────────────┐"),
        Line::from(" │                                 │"),
        Line::from(" └─────────────────────────────────┘"),
        Line::from(" Target dir: [                    ]"),
        Line::from(" [x] extract  [ ] keep archives   "),
        Line::from(""),
        Line::from(" Tab: next field   Enter: add package   Esc: close "),
    ])
    .block(block)
    .wrap(Wrap { trim: false });
    frame.render_widget(content, modal_area);
}

/// Helper for tests: render to string via TestBackend
pub fn render_to_string(app: &App, width: u16, height: u16) -> String {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(app, f))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..height {
        for x in 0..width {
            let cell = buffer.cell((x, y)).unwrap();
            out.push_str(cell.symbol());
        }
        out.push('\n');
    }
    out
}

/// Helper for tests: render the status bar + history variant to string.
pub fn render_with_to_string(app: &App, status: &str, width: u16, height: u16) -> String {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_with(app, f, status, true))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..height {
        for x in 0..width {
            out.push_str(buffer.cell((x, y)).unwrap().symbol());
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use moon_down_core::{MemberState, Queue};

    fn queue_with_error() -> Queue {
        let mut q = Queue::new();
        q.add_package("pkg", "/tmp", vec![("file1".into(), "https://ex.com/a".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_error(mid, "net fail");
        q
    }
    fn queue_with_gone() -> Queue {
        let mut q = Queue::new();
        q.add_package("pkg", "/tmp", vec![("file1".into(), "https://ex.com/a".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_handle(mid, "gid1".into());
        q.set_member_state(mid, MemberState::Downloading);
        q.mark_gone_by_handle("gid1");
        q
    }

    #[test]
    fn queue_visible_always_even_in_accounts() {
        let q = queue_with_error();
        let mut app = App::new(q);
        app.view = View::Accounts;
        let s = render_to_string(&app, 80, 24);
        assert!(s.contains("Queue"), "queue header must stay visible in Accounts view, got:\n{s}");
        // pkg name should still be visible
        assert!(s.contains("pkg"));
    }

    #[test]
    fn detail_and_log_beneath_queue_order() {
        let q = Queue::new();
        let app = App::new(q);
        let s = render_to_string(&app, 80, 24);
        let q_pos = s.find("Queue").unwrap();
        let d_pos = s.find("Detail").unwrap();
        let l_pos = s.find("Log tail").unwrap();
        assert!(q_pos < d_pos && d_pos < l_pos, "layout must be queue -> detail -> log");
    }

    #[test]
    fn accounts_swaps_top_pane() {
        let q = Queue::new();
        let mut app = App::new(q);
        app.view = View::Accounts;
        let s = render_to_string(&app, 80, 24);
        assert!(s.contains("Accounts"));
        // Settings not shown
        assert!(!s.contains("Settings"));
        app.view = View::Settings;
        let s2 = render_to_string(&app, 80, 24);
        assert!(s2.contains("Settings"));
    }

    #[test]
    fn add_modal_centered() {
        let mut app = App::new(Queue::new());
        app.show_add_modal = true;
        let s = render_to_string(&app, 80, 24);
        assert!(s.contains("Add downloads"));
    }

    #[test]
    fn error_row_shows_retry_key() {
        let q = queue_with_error();
        let mut app = App::new(q);
        // expand to see member row
        let pid = app.queue.packages[0].id;
        app.expanded.insert(pid);
        let s = render_to_string(&app, 80, 24);
        assert!(s.contains("[r]etry"), "error row must show retry key, got:\n{s}");
    }

    #[test]
    fn gone_row_shows_readd_and_prune() {
        let q = queue_with_gone();
        let mut app = App::new(q);
        let pid = app.queue.packages[0].id;
        app.expanded.insert(pid);
        let s = render_to_string(&app, 80, 24);
        assert!(s.contains("[r]e-add"), "gone row must show re-add, got:\n{s}");
        assert!(s.contains("[x]prune"), "gone row must show prune, got:\n{s}");
    }

    #[test]
    fn status_and_history_panes_render() {
        let mut q = Queue::new();
        let done = q.add_package(
            "blender.tar.xz",
            "/tmp/blender",
            vec![("blender.tar.xz".into(), "https://x/y".into())],
        );
        let m = q.packages[0].members[0].id;
        q.set_member_progress(m, 1024, 1024);
        q.set_member_state(m, MemberState::Complete);
        let live = q.add_package(
            "ubuntu.iso",
            "/tmp/ubuntu",
            vec![("ubuntu.iso".into(), "https://x/z".into())],
        );
        let m2 = q.packages[1].members[0].id;
        q.set_member_progress(m2, 2048, 1024);
        q.set_member_state(m2, MemberState::Downloading);
        assert_ne!(done, live);

        let app = App::new(q);
        let s = render_with_to_string(&app, " seed 27241 | q quit", 100, 40);
        assert!(s.contains("seed 27241"), "status line missing, got:\n{s}");
        assert!(s.contains("History"), "history pane missing, got:\n{s}");
        assert!(s.contains("blender.tar.xz"), "completed package missing from history, got:\n{s}");
        assert!(s.contains("1.0 KiB"), "history should show human size, got:\n{s}");
        assert!(s.contains("[DL]"), "active package stays visible in the queue, got:\n{s}");
    }
}
