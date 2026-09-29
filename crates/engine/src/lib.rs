//! Embedded aria2-rust daemon: start it detached, attach by RPC, enqueue, stop.
//! Ponytail: no external binary to find, no HTTP client crate, no abstractions
//! without a caller.

pub mod client;
pub mod daemon;
pub mod extract;
pub mod port;
pub mod rpc;
pub mod secret;
pub mod strip;

pub use client::{ClientError, RpcClient, RpcError};
pub use daemon::{Daemon, DaemonInfo};
pub use port::{pick_free_port, pick_free_port_in};
pub use rpc::{
    build_change_global_option, build_enqueue_with_auth, build_poll_batch, EnqueueKind,
    GlobalStat, StatusEntry, StoredRecord, Tick,
};
pub use secret::generate_secret;
pub use strip::strip_credentials;
