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

fn main() -> io::Result<()> {
    let state_dir = state_dir_from_args();
    if daemon_mode_from_args() {
        return run_daemon(&state_dir);
    }
    if tray_mode_from_args() {
        return run_tray(&state_dir);
    }
    match cli_command() {
        Some(cmd) => return run_cli_command(&cmd, &state_dir),
        None => {}
    }
    main_tui(&state_dir)
}

// ============================================================================
// CLI
//
// The TUI's add modal is still a skeleton, so these are the supported way to
// put something in the queue. They talk to the same daemon the TUI watches, so
// a download added here shows up in the TUI on its next tick.
// ============================================================================

#[derive(Debug, PartialEq)]
enum Cli {
    Add { uris: Vec<String>, name: Option<String>, dir: Option<String> },
    Ls,
    Help,
    Unknown(Vec<String>),
}

/// The first non-flag argument decides the command. Everything else is a
/// positional for that command, which keeps parsing trivial and predictable.
fn cli_command() -> Option<Cli> {
    let words: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with("--state-dir")).collect();
    let mut it = words.iter();
    let verb = it.next().map(String::as_str)?;
    let rest: Vec<String> = it.cloned().collect();

    match verb {
        "add" | "a" => {
            let mut uris = Vec::new();
            let mut name = None;
            let mut dir = None;
            let mut it = rest.into_iter();
            while let Some(a) = it.next() {
                if let Some(v) = a.strip_prefix("--name=") {
                    name = Some(v.to_string());
                } else if let Some(v) = a.strip_prefix("--dir=") {
                    dir = Some(v.to_string());
                } else {
                    uris.push(a);
                }
            }
            Some(Cli::Add { uris, name, dir })
        }
        "ls" | "list" => Some(Cli::Ls),
        "help" | "--help" | "-h" => Some(Cli::Help),
        _ => Some(Cli::Unknown(words)),
    }
}

fn run_cli_command(cmd: &Cli, state_dir: &Path) -> io::Result<()> {
    match cmd {
        Cli::Help => {
            println!("{}", USAGE);
            Ok(())
        }
        Cli::Unknown(words) => {
            eprintln!("unknown command: {}\n\n{}", words.join(" "), USAGE);
            Err(io::Error::other("bad command"))
        }
        Cli::Ls => {
            // Live, not the last persisted file: the TUI only writes on local
            // edits, so the file on disk can be stale. Fold in one daemon tick
            // so `ls` reports what is actually happening right now.
            let state_path = state_dir.join("state.json");
            let mut queue = load_state(&state_path).unwrap_or_default();
            let dl = default_download_dir(state_dir);
            if let Ok(daemon) = Daemon::ensure_running(state_dir, &std::env::current_exe()?, &dl) {
                match daemon.tick(1) {
                    Ok(tick) => {
                        tick.apply(&mut queue);
                    }
                    Err(e) => eprintln!("(daemon not answering: {e})"),
                }
            }
            print_queue(&queue);
            Ok(())
        }
        Cli::Add { uris, name, dir } => add_uris(state_dir, uris, name.as_deref(), dir.as_deref()),
    }
}

const USAGE: &str = "\
moon-down — terminal download manager

  moon-down                    open the TUI
  moon-down add <url>...      queue a download (see the TUI within a tick)
      --name=NAME              package name shown in the queue
      --dir=DIR                download directory (default: the configured one)
  moon-down ls                 print the queue
  moon-down --tray             system-tray icon (needs --features tray)

Global: --state-dir=PATH";

/// Queue one or more URIs as a single package.
///
/// The GID the daemon returns is stored as the member handle straight away:
/// that is the join key `Tick::apply` uses to fold progress back into the row,
/// so skipping it is what makes a download invisible in the TUI.
fn add_uris(state_dir: &Path, uris: &[String], name: Option<&str>, dir: Option<&str>) -> io::Result<()> {
    if uris.is_empty() {
        eprintln!("nothing to add — give at least one URL");
        return Err(io::Error::other("no urls"));
    }
    let exe = std::env::current_exe()?;
    let download_dir = match dir {
        Some(d) => PathBuf::from(d),
        None => default_download_dir(state_dir),
    };
    let daemon = Daemon::ensure_running(state_dir, &exe, &download_dir)?;

    // One addUri per URL. Passing them all in one call would make aria2 treat
    // them as mirror sources for a single file — one GID, one output name, and
    // the extra URLs would overwrite the first. Each URL is its own download.
    let mut gids = Vec::new();
    for uri in uris {
        let gid = daemon
            .enqueue(std::slice::from_ref(uri), &download_dir)
            .map_err(|e| io::Error::other(format!("daemon refused {uri}: {e}")))?;
        gids.push(gid);
    }

    let state_path = state_dir.join("state.json");
    let mut queue = load_state(&state_path).unwrap_or_default();
    let package_name = name.map(String::from).unwrap_or_else(|| {
        // Name the package after its first file, which is what a user expects.
        file_name_of(&uris[0])
    });
    let members: Vec<(String, String)> =
        uris.iter().map(|u| (file_name_of(u), u.clone())).collect();
    let id = queue.add_package(package_name, download_dir.display().to_string(), members);
    if let Some(pkg) = queue.packages.iter_mut().find(|p| p.id == id) {
        for (m, gid) in pkg.members.iter_mut().zip(&gids) {
            m.handle = Some(gid.clone());
            m.state = MemberState::Queued;
        }
    }
    save_state(&queue, &state_path)
        .map_err(|e| io::Error::other(format!("queued in the daemon but could not save state: {e}")))?;

    println!("queued {id}  ->  {}", download_dir.display());
    for (uri, gid) in uris.iter().zip(&gids) {
        println!("  {gid}  {uri}");
    }
    Ok(())
}

/// Last path segment of a URL, ignoring query and fragment; falls back to the
/// whole string so a name is always produced.
fn file_name_of(uri: &str) -> String {
    let without_scheme = uri.split("://").last().unwrap_or(uri);
    let path = without_scheme
        .split(['?', '#'])
        .next()
        .unwrap_or(without_scheme);
    path.rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(uri)
        .to_string()
}

fn print_queue(queue: &Queue) {
    if queue.packages.is_empty() {
        println!("(empty) — add one with: moon-down add <url>");
        return;
    }
    for pkg in &queue.packages {
        let prog = (pkg.progress() * 100.0) as u64;
        println!("{:<4} {:<28} [{:<8}] {:>3}%", pkg.id, pkg.name, format!("{:?}", pkg.status()), prog);
        for m in &pkg.members {
            println!(
                "       └ {:<26} {:>9}/{:>9}  {:?}",
                m.name, m.completed_bytes, m.total_bytes, m.state
            );
        }
    }
}

/// The tray icon is a separate process on purpose; see `tray.rs` for why.
#[cfg(feature = "tray")]
fn run_tray(state_dir: &Path) -> io::Result<()> {
    moon_down_ui::tray::run(state_dir)
}

/// Without the feature this is a build-configuration error, not a runtime one,
/// so say so plainly rather than silently ignoring the flag.
#[cfg(not(feature = "tray"))]
fn run_tray(_state_dir: &Path) -> io::Result<()> {
    Err(io::Error::other(
        "built without the `tray` feature — rebuild with: cargo build --features tray",
    ))
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

fn tray_mode_from_args() -> bool {
    std::env::args().skip(1).any(|a| a == "--tray")
}

fn default_state_dir() -> String {
    std::env::var("XDG_STATE_HOME")
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            format!("{home}/.local/state")
        })
        + "/moon-down"
}

/// Restore the saved queue. A first run is genuinely empty — no demo rows.
///
/// An earlier version seeded three fake packages here. They had invented
/// handles like `demo-live-1`, which can never match a real aria2 GID, so those
/// rows could never show progress and only ever made the TUI look busy. An
/// empty queue is honest; `moon-down add` fills it.
fn load_or_seed(path: &Path, logs: &mut Vec<String>) -> Queue {
    match load_state(path) {
        Ok(q) if !q.packages.is_empty() => {
            logs.push(format!(
                "restored {} package(s) from {}",
                q.packages.len(),
                path.display()
            ));
            q
        }
        Ok(_) => {
            logs.push("empty queue — add one with: moon-down add <url>".into());
            Queue::new()
        }
        Err(e) => {
            logs.push(format!("no state at {} ({e}); starting empty", path.display()));
            Queue::new()
        }
    }
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
                        if app.handle_key(key_char(k)) {
                            // Write local edits straight away: the tick loop reloads
                            // from disk, so an unsaved delete would come back.
                            persist(app, state_path);
                        }
                    }
                }
                Event::Resize(_, _) => app.mark_dirty(),
                _ => {}
            },
            false => {
                ticks += 1;
                rpc_id += 4;
                // Re-read the queue from disk every tick. `moon-down add` writes
                // here, and this TUI holds its own copy in memory, so without a
                // reload a CLI-added download never appears. Local edits are
                // persisted the moment they happen (see handle_key) so this
                // reload cannot undo them.
                reload_queue(app, state_path);
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

/// Pull in queue changes written by the CLI since the last tick. A read error
/// is ignored: the in-memory queue is still the best thing we have.
fn reload_queue(app: &mut App, state_path: &Path) {
    if let Ok(fresh) = load_state(state_path) {
        if fresh != app.queue {
            let before = app.queue.packages.len();
            app.queue = fresh;
            if app.queue.packages.len() != before {
                app.push_log(format!("queue reloaded ({} package(s))", app.queue.packages.len()));
            }
            app.mark_dirty();
        }
    }
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
