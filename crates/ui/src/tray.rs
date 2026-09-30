//! Optional system-tray indicator for the daemon.
//!
//! Behind the `tray` feature so the default build stays free of GUI
//! dependencies. Run with `moon-down --tray`; the TUI starts it alongside the
//! daemon on first run.
//!
//! Why a separate process rather than living inside the daemon: the daemon is
//! an aria2-rust host running on a tokio runtime, and GTK wants its own main
//! loop. Sharing them risks a GTK hiccup taking downloads down with it. The
//! indicator is cosmetic; the daemon is the product, so they stay separate and
//! a crash here costs a restart of the icon, not of the downloads.
//!
//! The icon is generated, not shipped. `Icon::from_rgba` takes raw pixels, so
//! there is no artwork file to maintain and no dependence on whatever icon
//! theme happens to be installed. Colours come from the same design seed as the
//! TUI, in `moon_down_ui::render`.

use glib::ControlFlow::Continue;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use muda::MenuEvent;
use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};use tray_icon::{Icon, TrayIconBuilder};

use moon_down_engine::Daemon;

/// How often the menu's status line is refreshed from the daemon.
const REFRESH: Duration = Duration::from_secs(2);

/// The TUI's accent colour, from the design seed in `render.rs`.
const ACCENT: (u8, u8, u8) = (214, 69, 217);

pub fn run(state_dir: &Path) -> std::io::Result<()> {
    gtk::init().map_err(|e| std::io::Error::other(format!("gtk init failed: {e}")))?;

    // Disabled so it reads as a status line rather than a command.
    let status = MenuItem::new("moon-down: starting…", false, None);
    let open = MenuItem::new("Open queue", true, None);
    let quit_daemon = MenuItem::new("Stop daemon", true, None);
    let quit_tray = MenuItem::new("Quit icon", true, None);

    let menu = Menu::with_items(&[
        &status,
        &PredefinedMenuItem::separator(),
        &open,
        &quit_daemon,
        &PredefinedMenuItem::separator(),
        &quit_tray,
    ])
    .map_err(io_err)?;

    let tray = TrayIconBuilder::new()
        // AppIndicator will not display without an icon AND a menu.
        .with_menu(Box::new(menu))
        .with_icon(generic_icon()?)
        .build()
        .map_err(io_err)?;

    // The daemon is optional: the icon should still appear so the user can see
    // "daemon offline" and use the menu, rather than the process vanishing.
    let dir = state_dir.to_path_buf();
    let mut daemon = Daemon::load(&dir).ok().map(|info| Daemon {
        state_dir: dir.clone(),
        info,
    });
    let mut rpc_id: u64 = 0;
    let status = Arc::new(status);

    // ponytail: the tick is a blocking POST on the GTK main loop every 2s. It
    // is loopback and sub-millisecond in practice, so a worker thread would be
    // the wrong kind of complexity — revisit only if the menu ever feels sticky.
    let refreshing = status.clone();
    let poll_dir = dir.clone();
    glib::timeout_add_local(REFRESH, move || {
        if daemon.is_none() {
            daemon = Daemon::load(&poll_dir).ok().map(|info| Daemon {
                state_dir: poll_dir.clone(),
                info,
            });
        }
        let line = match daemon.as_ref().map(|d| d.tick(rpc_id)) {
            Some(Ok(tick)) => {
                rpc_id += 4;
                describe(&tick)
            }
            Some(Err(_)) => "daemon not answering".to_string(),
            None => "daemon offline".to_string(),
        };
        refreshing.set_text(&line);
        Continue
    });

    // Menu items are Rc-based and so are not `Send`, which rules out muda's
    // `set_event_handler` (it demands Send + Sync). The event channel is polled
    // from the main loop instead, which is also the right thread for GTK work.
    let quit_tray_id = quit_tray.id().clone();
    let quit_daemon_id = quit_daemon.id().clone();
    let open_id = open.id().clone();

    // Keep the icon alive for the process lifetime; dropping it removes the icon.
    let _ = tray;
    loop {
        // Drain any menu clicks, oldest first.
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id().as_ref();
            if id == quit_tray_id.as_ref() {
                return Ok(());
            } else if id == quit_daemon_id.as_ref() {
                if let Ok(info) = Daemon::load(&dir) {
                    Daemon { state_dir: dir.clone(), info }.stop();
                }
            } else if id == open_id.as_ref() {
                // Reopen the TUI, detached so closing it does not kill the icon.
                let _ = std::process::Command::new(std::env::current_exe().unwrap_or_default())
                    .arg(format!("--state-dir={}", dir.display()))
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
        }
        // Pump the GTK main loop without blocking, so the status timeout fires.
        glib::MainContext::default().iteration(false);
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One line of status for the menu header: what is running, at what speed.
fn describe(tick: &moon_down_engine::Tick) -> String {
    let active = tick.active.len();
    let total: u64 = tick.active.iter().map(|e| e.total()).sum();
    let done: u64 = tick.active.iter().map(|e| e.completed()).sum();
    let speed = tick.global.speed();
    if active == 0 {
        return "moon-down: idle".to_string();
    }
    format!(
        "moon-down: {active} active · {} · {}",
        percent(done, total),
        human_bytes(speed)
    )
}

fn percent(done: u64, total: u64) -> String {
    if total == 0 {
        return "0%".to_string();
    }
    format!("{}%", (done as f64 / total as f64 * 100.0).round() as u64)
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B/s")
    } else {
        format!("{v:.1} {}/s", UNITS[u])
    }
}

/// A generic 32x32 download glyph: a filled accent disc with a white down arrow.
/// Drawn here rather than shipped so there is no artwork to maintain and no
/// dependency on the user's icon theme.
fn generic_icon() -> std::io::Result<Icon> {
    const S: usize = 32;
    let mut px = vec![0u8; S * S * 4];
    let c = (S as f32 - 1.0) / 2.0;
    let r = S as f32 * 0.46;

    for y in 0..S {
        for x in 0..S {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            if (dx * dx + dy * dy).sqrt() > r {
                continue; // outside the disc: leave transparent
            }
            let (mut rr, mut gg, mut bb, mut aa) = (ACCENT.0, ACCENT.1, ACCENT.2, 255u8);

            // Arrow shaft: a vertical bar in the middle third, upper half.
            // Arrow head: a triangle widening downward.
            let in_shaft = dx.abs() <= 2.0 && dy <= 2.0 && dy >= -7.0;
            let head_t = (dy + 1.0) / 9.0; // 0 at the shaft base, 1 at the tip
            let in_head = dy > 1.0 && dy <= 10.0 && dx.abs() <= 7.0 * (1.0 - head_t);
            if in_shaft || in_head {
                rr = 255;
                gg = 255;
                bb = 255;
            }
            let i = (y * S + x) * 4;
            px[i] = rr;
            px[i + 1] = gg;
            px[i + 2] = bb;
            px[i + 3] = aa;
        }
    }
    Icon::from_rgba(px, S as u32, S as u32)
        .map_err(|e| std::io::Error::other(format!("bad icon: {e}")))
}

fn io_err(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_builds_at_the_expected_shape() {
        // `from_rgba` rejects a buffer whose length is not w*h*4, so Ok() means
        // the generated buffer was well formed.
        assert!(generic_icon().is_ok());
    }

    #[test]
    fn human_bytes_renders_a_rate() {
        assert_eq!(human_bytes(512), "512 B/s");
        assert_eq!(human_bytes(2048), "2.0 KiB/s");
    }

    #[test]
    fn percent_guards_against_zero_total() {
        assert_eq!(percent(0, 0), "0%");
        assert_eq!(percent(1, 4), "25%");
    }

    #[test]
    fn idle_tick_says_idle() {
        let tick = moon_down_engine::Tick::default();
        assert!(describe(&tick).contains("idle"));
    }
}
