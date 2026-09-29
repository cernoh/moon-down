//! Engine crate — managed-local aria2c integration.
//! Queue scaffolding re-exports core state so engine and UI share the same model.

pub use moon_down_core::{load_state, save_state, Member, MemberState, Package, PackageStatus, Queue};
