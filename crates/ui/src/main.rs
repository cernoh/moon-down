//! moon-down — terminal download manager.
//!
//! Design seed: `od -An -N2 -tu2 < /dev/urandom` -> 27241.
//! Every tuning constant is derived from it so the seed explains the whole look:
//!   27241 % 360 = 241deg  -> violet accent
//!   27241 % 4   = 1       -> 2s tick, 6 detail rows
//!   27241 % 8   = 1       -> 7 history rows
//!   27241 % 5   = 1       -> 5 log rows
//!
//! Two modes in one binary:
//!   moon-down            TUI. On start it makes sure the daemon is up (first run
//!                        starts it) and then only attaches to it over JSON-RPC.
//!   moon-down --daemon   The daemon itself: aria2-rust, self-detached, logs to
//!                        the state dir. Started by the TUI, survives its exit.

use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use aria2::app::App as Aria2App;
use aria2::app::cli::CliArgs;
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use moon_down_core::{load_state, save_state, MemberState, PackageStatus, Queue};
use moon_down_engine::Daemon;
use moon_down_ui::app::App;
use moon_down_ui::render::{human_bytes, render_with, ACCENT, DETAIL_ROWS, HISTORY_ROWS, LOG_ROWS};

const SEED: u64 = 27241;
const TICK: Duration = Duration::from_millis((SEED % 4 + 1) as u64 * 1000);
const FLUSH_EVERY_TICKS: u32 = 15; // ~30s at a 2s tick

fn main() -> io::Result<()> {
    let state_dir = state_dir_from_args();
    if daemon_mode_from_args() {
        return run_daemon(&state_dir);
    }
    main_tui(&state_dir)
}

fn main_tui(state_dir: &Path) -> io::Result<()> {
    let state_path = state_dir.join("state.json");
    let mut boot_logs = Vec::new();
    let queue = load_or_seed(&state_path, &mut boot_logs);
    let mut app = App::new(queue);

    // First run (or after a crash) starts the daemon; later runs just attach.
    match Daemon::ensure_running(state_dir, &std::env::current_exe()?, &default_download_dir(state_dir)) {
        Ok(daemon) => boot_logs.push(format!("daemon up on port {}", daemon.info.port)),
        Err(e) => boot_logs.push(format!("daemon not available: {e}")),
    }
    boot_logs.push(format!("downloads dir: {}", default_download_dir(state_dir).display()));
    for line in boot_logs {
        app.push_log(line);
    }

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, &mut app, state_dir, &state_path);
    let restore = restore_terminal(&mut terminal);
    result.and(restore)
}

/// Where downloads land. A real download directory beats the state dir: the
/// state dir is for bookkeeping, and nobody wants 40 GB inside `~/.local/state`.
fn default_download_dir(state_dir: &Path) -> PathBuf {
    if let Ok(d) = std::env::var("XDG_DOWNLOAD_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join("Downloads").join("moon-down");
        if p.parent().is_some_and(|par| par.exists()) {
            return p;
        }
    }
    state_dir.join("downloads")
}

/// Daemon mode: run aria2-rust in the foreground of our own session.
/// The TUI detached us with `setsid` in `pre_exec`, so there is no daemonizing
/// to do here — forking from inside the tokio runtime would deadlock the child.
/// `daemon=true` is never passed: it would make aria2-rust fork again.
fn run_daemon(state_dir: &Path) -> io::Result<()> {
    let args = [
        "aria2c".to_string(),
        format!("--conf-path={}", Daemon::conf_path(state_dir).display()),
        // Explicit, not conf-only: aria2-rust requires --enable-rpc on argv to
        // start an RPC-only service with no download input.
        "--enable-rpc=true".to_string(),
    ];
    let cli = CliArgs::parse_from(&args);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    // Runs until aria2.shutdown; we are already detached, so block forever.
    let code = runtime.block_on(Aria2App::new().run(cli));
    if code != 0 {
        return Err(io::Error::other(format!("aria2-rust daemon exited with {code}")));
    }
    Ok(())
}

fn state_dir_from_args() -> PathBuf {
    let mut args = std::env::args().skip(1);
    let mut dir: Option<String> = None;
    while let Some(a) = args.next() {
        if a == "--state-dir" {
            dir = args.next();
        } else if let Some(v) = a.strip_prefix("--state-dir=") {
            dir = Some(v.to_string());
        }
    }
    PathBuf::from(dir.unwrap_or_else(default_state_dir))
}

fn daemon_mode_from_args() -> bool {
    std::env::args().skip(1).any(|a| a == "--daemon")
}

fn default_state_dir() -> String {
    std::env::var("XDG_STATE_HOME")
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            format!("{home}/.local/state")
        })
        + "/moon-down"
}

/// Restore the saved queue, or seed a first run so the UI is never blank.
fn load_or_seed(path: &Path, logs: &mut Vec<String>) -> Queue {
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

fn status_line(app: &App, daemon: Option<&Daemon>) -> String {
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
    let engine = match daemon {
        Some(d) => format!("aria2-rust daemon :{}", d.info.port),
        None => "daemon offline".to_string(),
    };
    format!(
        " seed {SEED}  │  {engine}  │  {active} active, {done} done, {} total  │  q quit",
        human_bytes(bytes)
    )
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    state_dir: &Path,
    state_path: &Path,
) -> io::Result<()> {
    // Re-attach per session: the daemon is long-lived, and a crash or an explicit
    // stop between sessions must not leave the TUI polling a dead port forever.
    let exe = std::env::current_exe()?;
    let mut daemon = Daemon::ensure_running(state_dir, &exe, &default_download_dir(state_dir)).ok();
    let mut ticks: u32 = 0;
    let mut rpc_id: u64 = 0;
    let mut pending_g = false;
    loop {
        // Render on state change only: the loop polls, mutates, and draws when dirty.
        if app.take_dirty() {
            terminal.draw(|f| render_with(app, f, &status_line(app, daemon.as_ref()), true))?;
        }

        match event::poll(TICK)? {
            true => match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if is_quit(k, app) {
                        break;
                    }
                    if !handle_vim_nav(k, app, &mut pending_g) {
                        app.handle_key(key_char(k));
                    }
                }
                Event::Resize(_, _) => app.mark_dirty(),
                _ => {}
            },
            false => {
                ticks += 1;
                rpc_id += 4;
                // Attach on the first tick of a session and again after any failure,
                // so a daemon that dies mid-session is picked back up.
                if daemon.is_none() {
                    if let Ok(d) = Daemon::ensure_running(state_dir, &exe, &default_download_dir(state_dir)) {
                        app.push_log(format!("daemon up on port {}", d.info.port));
                        daemon = Some(d);
                    }
                }
                match daemon.as_ref().map(|d| d.tick(rpc_id)) {
                    // Render only when the queue actually moved, so an idle daemon
                    // does not repaint every tick.
                    Some(Ok(tick)) => {
                        if tick.apply(&mut app.queue) {
                            app.mark_dirty();
                        }
                    }
                    Some(Err(e)) => {
                        // Transport and auth failures are visible but not fatal.
                        // Throttled so a down daemon cannot flood the log pane.
                        if e.to_string().contains("Unauthorized") {
                            app.push_log(format!("tick auth failed: {e}"));
                        } else if ticks % 5 == 0 {
                            app.push_log(format!("tick failed: {e}"));
                        }
                        // Never trust queue state we could not fetch.
                        daemon = None;
                    }
                    None => daemon = None,
                }
                if ticks % FLUSH_EVERY_TICKS == 0 {
                    persist(app, state_path);
                }
            }
        }
    }
    persist(app, state_path);
    Ok(())
}

/// Vim-style motion, layered over the app's own key handling. Returns true when
/// the key was consumed here, so `h`/`l` still fall through to the app's
/// collapse/expand behaviour.
fn handle_vim_nav(k: KeyEvent, app: &mut App, pending_g: &mut bool) -> bool {
    if k.modifiers.contains(KeyModifiers::CONTROL) {
        match k.code {
            KeyCode::Char('d') => { app.page_down(10); *pending_g = false; return true; }
            KeyCode::Char('u') => { app.page_up(10); *pending_g = false; return true; }
            KeyCode::Char('f') => { app.page_down(20); *pending_g = false; return true; }
            KeyCode::Char('b') => { app.page_up(20); *pending_g = false; return true; }
            _ => {}
        }
    }
    match k.code {
        KeyCode::Char('g') if !k.modifiers.contains(KeyModifiers::CONTROL) => {
            if *pending_g {
                app.move_top();
                *pending_g = false;
            } else {
                *pending_g = true;
            }
            true
        }
        KeyCode::Char('G') => { app.move_bottom(); *pending_g = false; true }
        KeyCode::Up => { app.move_by(-1); *pending_g = false; true }
        KeyCode::Down => { app.move_by(1); *pending_g = false; true }
        KeyCode::Left | KeyCode::Right => {
            // h/l belong to the app: collapse and expand a package.
            *pending_g = false;
            false
        }
        KeyCode::Home => { app.move_top(); *pending_g = false; true }
        KeyCode::End => { app.move_bottom(); *pending_g = false; true }
        KeyCode::PageDown => { app.page_down(20); *pending_g = false; true }
        KeyCode::PageUp => { app.page_up(20); *pending_g = false; true }
        _ => {
            if *pending_g {
                *pending_g = false;
            }
            false
        }
    }
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

fn persist(app: &mut App, path: &Path) {
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
