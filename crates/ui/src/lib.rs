//! UI crate — queue inspector skeleton with ratatui.
//! One event loop owns App; render only on state change.

pub mod app;
pub mod render;

pub use app::{App, View};
pub use moon_down_core::{MemberState, PackageStatus, Queue};
pub use render::{render, render_to_string};
