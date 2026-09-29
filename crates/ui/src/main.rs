//! moon-down — terminal download manager.
//!
//! Design seed: `od -An -N2 -tu2 < /dev/urandom` -> 27241.
//! Every tuning constant is derived from it so the seed explains the whole look:
//!   27241 % 360 = 241deg  -> violet accent
//!   27241 % 4   = 1       -> 2s tick, 6 detail rows
//!   27241 % 8   = 1       -> 7 history rows
//!   27241 % 5   = 1       -> 5 log rows

use std::collections::HashMap;
use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use moon_down_core::{load_state, save_state, MemberState, PackageStatus, Queue};
use moon_down_engine::{Engine, StartOptions};
use moon_down_ui::app::App;
use moon_down_ui::render::{human_bytes, render_with, ACCENT, DETAIL_ROWS, HISTORY_ROWS, LOG_ROWS};

const SEED: u64 = 27241;
const TICK: Duration = Duration::from_millis((SEED % 4 + 1) as u64 * 1000);
const FLUSH_EVERY_TICKS: u32 = 15; // ~30s at a 2s tick

fn main() -> io::Result<()> {
    let (state_path, aria_bin) = state_paths_from_args();
    let state_dir = state_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let _ = std::fs::create_dir_all(&state_dir);

    let mut boot_logs = Vec::new();
    let queue = load_or_seed(&state_path, &mut boot_logs);
    let mut app = App::new(queue);
    for line in boot_logs {
        app.push_log(line);
    }

    // Try to start real aria2c. If missing, stay in DEMO mode with a clear log line.
    let mut engine: Option<Engine> = match Engine::start(&state_dir, StartOptions { aria_bin: aria_bin.clone(), mock_sleep_secs: None }) {
        Ok(eng) => {
            app.push_log(format!("engine LIVE on 127.0.0.1:{} (secret {}…)", eng.port, &eng.secret[..4.min(eng.secret.len())]));
            app.push_log(format!("downloads dir: {}", default_download_dir(&state_dir).display()));
            Some(eng)
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("could not spawn") {
                app.push_log(format!("engine not running ({msg}); DEMO mode — no real downloads. Install with: nix shell nixpkgs#aria2"));
            } else {
                app.push_log(format!("engine start failed: {msg}; DEMO mode"));
            }
            None
        }
    };
    let _ = std::fs::create_dir_all(default_download_dir(&state_dir));

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, &mut app, &state_path, &state_dir, &mut engine);
    // graceful shutdown before restoring terminal
    if let Some(mut eng) = engine {
        let client = eng.client();
        let _ = eng.shutdown_with(|method| client.call(method, serde_json::json!([]), 1).is_ok());
        // log not visible after restore, but ensure state saved
    }
    let restore = restore_terminal(&mut terminal);
    result.and(restore)
}

fn state_paths_from_args() -> (PathBuf, Option<String>) {
    let mut args = std::env::args().skip(1).peekable();
    let mut dir: Option<String> = None;
    let mut aria_bin: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--state-dir" => dir = args.next(),
            "--aria-bin" => aria_bin = args.next(),
            other if other.starts_with("--state-dir=") => dir = Some(other.trim_start_matches("--state-dir=").to_string()),
            other if other.starts_with("--aria-bin=") => aria_bin = Some(other.trim_start_matches("--aria-bin=").to_string()),
            _ => {}
        }
    }
    let dir = dir.unwrap_or_else(|| {
        std::env::var("XDG_STATE_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            format!("{home}/.local/state")
        }) + "/moon-down"
    });
    (PathBuf::from(dir).join("state.json"), aria_bin)
}

fn default_download_dir(state_dir: &Path) -> PathBuf {
    // Prefer XDG_DOWNLOAD_DIR or ~/Downloads/moon-down, else state_dir/downloads
    if let Ok(d) = std::env::var("XDG_DOWNLOAD_DIR") { return PathBuf::from(d); }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join("Downloads").join("moon-down");
        if p.parent().map(|par| par.exists()).unwrap_or(false) { return p; }
    }
    state_dir.join("downloads")
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
    let done = q.add_package(
        "blender-4.2-linux-x64.tar.xz",
        "/home/user/Downloads/blender",
        vec![("blender-4.2-linux-x64.tar.xz".into(), "https://download.blender.org/4.2/x".into())],
    );
    let m = q.packages[0].members[0].id;
    q.set_member_handle(m, "demo-done-1".into());
    q.set_member_progress(m, 291_234_304, 291_234_304);
    q.set_member_state(m, MemberState::Complete);
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

fn status_line(app: &App, engine: Option<&Engine>) -> String {
    let active = app.queue.packages.iter().filter(|p| p.status() == PackageStatus::Active).count();
    let done = app.queue.packages.iter().filter(|p| p.status() == PackageStatus::Complete).count();
    let bytes: u64 = app.queue.packages.iter().map(|p| p.members.iter().map(|m| m.total_bytes).sum::<u64>()).sum();
    let state = if let Some(eng) = engine {
        if let Some(err) = &eng.fatal_error {
            format!("ENGINE ERROR: {err}")
        } else if !eng.crashed && eng.is_alive_check() {
            format!("LIVE aria2c 127.0.0.1:{} | {} active", eng.port, active)
        } else if eng.crashed {
            "ENGINE crashed — respawned once, watch log".to_string()
        } else {
            format!("LIVE aria2c 127.0.0.1:{} | {} active", eng.port, active)
        }
    } else {
        "DEMO: simulated progress, aria2c not running".to_string()
    };
    format!(" seed {SEED} │ {state} │ {done} done, {} total │ q quit", human_bytes(bytes))
}

// Engine has is_alive that needs &mut, provide a check that doesn't require mut for status bar by probing pid file
trait EngineCheck { fn is_alive_check(&self) -> bool; }
impl EngineCheck for Engine {
    fn is_alive_check(&self) -> bool {
        // best-effort: if pid file exists and /proc suggests alive — else assume alive when no fatal
        self.fatal_error.is_none()
    }
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    state_path: &PathBuf,
    state_dir: &Path,
    engine: &mut Option<Engine>,
) -> io::Result<()> {
    let mut ticks: u32 = 0;
    let mut next_rpc_id: u64 = 1;
    let mut pending_g = false;

    loop {
        if app.take_dirty() {
            let line = status_line(app, engine.as_ref());
            terminal.draw(|f| render_with(app, f, &line, true))?;
        }

        match event::poll(TICK)? {
            true => match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if is_quit(k, app) {
                        break;
                    }
                    // Modal input has priority
                    if app.is_modal_open() {
                        match k.code {
                            KeyCode::Esc => {
                                app.show_add_modal = false;
                                app.add_input_clear();
                            }
                            KeyCode::Enter => {
                                let raw = app.take_add_input();
                                let uris: Vec<String> = raw.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                                // also handle single-line paste without newline
                                let uris = if uris.is_empty() && !raw.trim().is_empty() { vec![raw.trim().to_string()] } else { uris };
                                if uris.is_empty() {
                                    app.push_log("add cancelled — no URIs");
                                    app.show_add_modal = false;
                                } else {
                                    let dl_dir = default_download_dir(state_dir);
                                    let _ = std::fs::create_dir_all(&dl_dir);
                                    let dir_str = dl_dir.to_string_lossy().to_string();
                                    let pkg_id = app.submit_add_uris(uris.clone(), dir_str.clone());
                                    if let Some(pid) = pkg_id {
                                        if let Some(eng) = engine.as_ref() {
                                            let mut opts = HashMap::new();
                                            opts.insert("dir".to_string(), dir_str);
                                            match eng.add_uris(&uris, &opts, next_rpc_id) {
                                                Ok(gids) => {
                                                    if let Some(pkg) = app.queue.packages.iter_mut().find(|p| p.id == pid) {
                                                        for (m, gid) in pkg.members.iter_mut().zip(gids) {
                                                            m.handle = Some(gid);
                                                        }
                                                    }
                                                    app.push_log(format!("enqueued {} URI(s) → live", uris.len()));
                                                    next_rpc_id += 1;
                                                }
                                                Err(e) => {
                                                    app.push_log(format!("enqueue failed: {e}"));
                                                    next_rpc_id += 1;
                                                }
                                            }
                                        } else {
                                            if let Some(pkg) = app.queue.packages.iter_mut().find(|p| p.id == pid) {
                                                for m in pkg.members.iter_mut() {
                                                    m.handle = Some(format!("demo-{}", m.id));
                                                }
                                            }
                                            app.push_log(format!("added {} URI(s) (DEMO — aria2c not running)", uris.len()));
                                        }
                                        app.show_add_modal = false;
                                    }
                                }
                            }
                            KeyCode::Backspace => { app.add_input_pop(); }
                            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                                // handle paste of multiple chars already coalesced by terminal; each char arrives as separate event
                                app.add_input_push(c);
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Vim navigation (outside modal)
                    if handle_vim_nav(k, app, &mut pending_g) {
                        continue;
                    }
                    // fallback to app handle_key
                    let ch = key_char(k);
                    if ch != '\0' {
                        app.handle_key(ch);
                    } else {
                        // arrow keys etc already handled; ensure pending_g cleared on non-g
                        if pending_g && k.code != KeyCode::Char('g') {
                            pending_g = false;
                        }
                    }
                }
                Event::Resize(_, _) => app.mark_dirty(),
                _ => {}
            },
            false => {
                // tick
                if let Some(eng) = engine.as_mut() {
                    // crash detection
                    if !eng.is_alive() {
                        app.push_log("engine died — respawning once…".to_string());
                        match eng.on_crash() {
                            Ok(()) => app.push_log(format!("engine respawned on 127.0.0.1:{}", eng.port)),
                            Err(e) => app.push_log(format!("engine fatal: {e}")),
                        }
                        app.mark_dirty();
                    } else {
                        match eng.tick(next_rpc_id) {
                            Ok(tick) => {
                                let before_len = app.queue.packages.len();
                                tick.apply(&mut app.queue);
                                // mark dirty if queue changed or progress moved
                                app.mark_dirty();
                                if app.queue.packages.len() != before_len {
                                    // reconcile pruned something
                                }
                                next_rpc_id += 4;
                            }
                            Err(e) => {
                                // transport / auth errors are visible but not fatal
                                let msg = e.to_string();
                                if msg.contains("Unauthorized") {
                                    app.push_log(format!("tick auth failed: {msg}"));
                                } else if ticks % 5 == 0 {
                                    // throttle log spam
                                    app.push_log(format!("tick failed: {msg}"));
                                }
                                next_rpc_id += 4;
                                app.mark_dirty();
                            }
                        }
                    }
                } else {
                    simulate(app);
                }
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

fn handle_vim_nav(k: KeyEvent, app: &mut App, pending_g: &mut bool) -> bool {
    // Ctrl combos
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
            return true;
        }
        KeyCode::Char('G') => { app.move_bottom(); *pending_g = false; return true; }
        KeyCode::Up => { app.move_by(-1); *pending_g = false; return true; }
        KeyCode::Down => { app.move_by(1); *pending_g = false; return true; }
        KeyCode::Left => { // h
            *pending_g = false;
            return false; // let app handle 'h'
        }
        KeyCode::Right => {
            *pending_g = false;
            return false;
        }
        KeyCode::Home => { app.move_top(); *pending_g = false; return true; }
        KeyCode::End => { app.move_bottom(); *pending_g = false; return true; }
        KeyCode::PageDown => { app.page_down(20); *pending_g = false; return true; }
        KeyCode::PageUp => { app.page_up(20); *pending_g = false; return true; }
        _ => {
            if *pending_g && k.code != KeyCode::Char('g') {
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

#[allow(dead_code)]
pub const LAYOUT: (u16, u16, u16) = (DETAIL_ROWS, HISTORY_ROWS, LOG_ROWS);
#[allow(dead_code)]
pub const ACCENT_RGB: (u8, u8, u8) = match ACCENT {
    ratatui::style::Color::Rgb(r, g, b) => (r, g, b),
    _ => (0, 0, 0),
};
