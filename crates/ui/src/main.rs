//! moon-down — terminal download manager.
//!
//! Design seed: `od -An -N2 -tu2 < /dev/urandom` -> 27241.
//! Every tuning constant is derived from it so the seed explains the whole look:
//!   27241 % 360 = 241deg  -> violet accent
//!   27241 % 4   = 1       -> 2s tick, 6 detail rows
//!   27241 % 8   = 1       -> 7 history rows
//!   27241 % 5   = 1       -> 5 log rows
//!
//! Scope note: `crates/engine` can spawn and poll a real `aria2c`, but no JSON-RPC
//! transport is wired yet, so the event loop below drives *simulated* progress and
//! says so in the status bar. Swap `simulate()` for an engine poll tick and the UI
//! is unchanged.

use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use moon_down_core::{load_state, save_state, MemberState, PackageStatus, Queue};
use moon_down_ui::app::App;
use moon_down_ui::render::{human_bytes, render_with, ACCENT, DETAIL_ROWS, HISTORY_ROWS, LOG_ROWS};

const SEED: u64 = 27241;
const TICK: Duration = Duration::from_millis((SEED % 4 + 1) as u64 * 1000);
const FLUSH_EVERY_TICKS: u32 = 15; // ~30s at a 2s tick

fn main() -> io::Result<()> {
    let state_path = state_path_from_args();
    let mut boot_logs = Vec::new();
    let queue = load_or_seed(&state_path, &mut boot_logs);
    let mut app = App::new(queue);
    for line in boot_logs {
        app.push_log(line);
    }

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, &mut app, &state_path);
    let restore = restore_terminal(&mut terminal);
    result.and(restore)
}

fn state_path_from_args() -> PathBuf {
    let mut args = std::env::args().skip(1);
    let mut dir: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--state-dir" => dir = args.next(),
            other => {
                if let Some(v) = other.strip_prefix("--state-dir=") {
                    dir = Some(v.to_string());
                }
            }
        }
    }
    let dir = dir.unwrap_or_else(|| {
        std::env::var("XDG_STATE_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            format!("{home}/.local/state")
        }) + "/moon-down"
    });
    PathBuf::from(dir).join("state.json")
}

/// Restore the saved queue, or seed a first run so the UI is never blank.
fn load_or_seed(path: &PathBuf, logs: &mut Vec<String>) -> Queue {
    match load_state(path) {
        Ok(q) if !q.packages.is_empty() => {
            logs.push(format!("restored {} package(s) from {}", q.packages.len(), path.display()));
            q
        }
        Ok(_) => seed_first_run(logs),
        Err(e) => {
            logs.push(format!("no state at {} ({e}); seeding demo queue", path.display()));
            seed_first_run(logs)
        }
    }
}

fn seed_first_run(logs: &mut Vec<String>) -> Queue {
    let mut q = Queue::new();

    // Something already finished, so the history pane has content on run one.
    let done = q.add_package(
        "blender-4.2-linux-x64.tar.xz",
        "/home/user/Downloads/blender",
        vec![("blender-4.2-linux-x64.tar.xz".into(), "https://download.blender.org/4.2/x".into())],
    );
    let m = q.packages[0].members[0].id;
    q.set_member_handle(m, "demo-done-1".into());
    q.set_member_progress(m, 291_234_304, 291_234_304);
    q.set_member_state(m, MemberState::Complete);

    // A multi-part post mid-flight, with one file finished.
    let post = q.add_package(
        "ubuntu-24.04-desktop-amd64",
        "/home/user/Downloads/ubuntu",
        vec![
            ("ubuntu-24.04-desktop-amd64.iso".into(), "https://releases.ubuntu.com/24.04/ubuntu.iso".into()),
            ("ubuntu-24.04-desktop-amd64.iso.asc".into(), "https://releases.ubuntu.com/24.04/ubuntu.iso.asc".into()),
        ],
    );
    let ms: Vec<u64> = q.packages[1].members.iter().map(|m| m.id).collect();
    q.set_member_handle(ms[0], "demo-live-1".into());
    q.set_member_progress(ms[0], 6_204_733_440, 2_113_209_369);
    q.set_member_state(ms[0], MemberState::Downloading);
    q.set_member_handle(ms[1], "demo-live-2".into());
    q.set_member_progress(ms[1], 833, 833);
    q.set_member_state(ms[1], MemberState::Complete);

    // One waiting its turn.
    let queued = q.add_package(
        "rust-1.90.0-x86_64-unknown-linux-gnu.tar.xz",
        "/home/user/Downloads/rust",
        vec![("rust-1.90.0.tar.xz".into(), "https://static.rust-lang.org/dist/rust.tar.xz".into())],
    );
    let m = q.packages[2].members[0].id;
    q.set_member_handle(m, "demo-queued-1".into());
    q.set_member_progress(m, 214_958_080, 0);
    q.set_member_state(m, MemberState::Queued);

    logs.push(format!("seeded {} package(s) (ids {done}, {post}, {queued})", q.packages.len()));
    q
}

/// The placeholder for a real engine tick: walk the queue and advance the clock.
/// ponytail: simulated bytes, not real ones — swapped out when the RPC transport lands.
fn simulate(app: &mut App) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut changed = false;
    for pkg in &mut app.queue.packages {
        pkg.created_at_ms = now;
        for m in &mut pkg.members {
            match m.state {
                MemberState::Queued if now % 4 == 0 => {
                    m.state = MemberState::Downloading;
                    changed = true;
                }
                MemberState::Downloading => {
                    // ~7 MB/s, so a 6 GB ISO takes a visible while
                    let step = 7_000_000u64;
                    m.completed_bytes = (m.completed_bytes + step).min(m.total_bytes);
                    if m.completed_bytes >= m.total_bytes {
                        m.state = MemberState::Complete;
                    }
                    changed = true;
                }
                _ => {}
            }
        }
    }
    if changed {
        app.mark_dirty();
    }
}

fn status_line(app: &App) -> String {
    let active = app
        .queue
        .packages
        .iter()
        .filter(|p| p.status() == PackageStatus::Active)
        .count();
    let done = app
        .queue
        .packages
        .iter()
        .filter(|p| p.status() == PackageStatus::Complete)
        .count();
    let bytes: u64 = app.queue.packages.iter().map(|p| p.members.iter().map(|m| m.total_bytes).sum::<u64>()).sum();
    format!(
        " seed {SEED}  │  DEMO: simulated progress, aria2c not wired yet  │  {active} active, {done} done, {} total  │  q quit",
        human_bytes(bytes)
    )
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    state_path: &PathBuf,
) -> io::Result<()> {
    let mut ticks: u32 = 0;
    loop {
        // Render on state change only: the loop polls, mutates, and draws when dirty.
        if app.take_dirty() {
            terminal.draw(|f| render_with(app, f, &status_line(app), true))?;
        }

        match event::poll(TICK)? {
            true => match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if is_quit(k, app) {
                        break;
                    }
                    app.handle_key(key_char(k));
                }
                Event::Resize(_, _) => app.mark_dirty(),
                _ => {}
            },
            false => {
                simulate(app);
                ticks += 1;
                if ticks % FLUSH_EVERY_TICKS == 0 {
                    persist(app, state_path);
                }
            }
        }
    }
    persist(app, state_path);
    Ok(())
}

fn key_char(k: KeyEvent) -> char {
    match k.code {
        KeyCode::Char(c) => c,
        KeyCode::Enter => '\n',
        KeyCode::Esc => '\x1b',
        KeyCode::Backspace => '\x7f',
        _ => '\0',
    }
}

fn is_quit(k: KeyEvent, app: &App) -> bool {
    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
        return true;
    }
    // While the add modal is open, q belongs to the modal.
    k.code == KeyCode::Char('q') && !app.is_modal_open()
}

fn persist(app: &mut App, path: &PathBuf) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match save_state(&app.queue, path) {
        Ok(()) => app.push_log(format!("state saved to {}", path.display())),
        Err(e) => app.push_log(format!("save failed: {e}")),
    }
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(out))
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    io::stdout().flush()
}

/// Layout constants re-exported for the binary's own tests and future tuning.
#[allow(dead_code)]
pub const LAYOUT: (u16, u16, u16) = (DETAIL_ROWS, HISTORY_ROWS, LOG_ROWS);
#[allow(dead_code)]
pub const ACCENT_RGB: (u8, u8, u8) = match ACCENT {
    ratatui::style::Color::Rgb(r, g, b) => (r, g, b),
    _ => (0, 0, 0),
};
