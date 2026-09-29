//! Managed-local aria2c child — spawn, poll, enqueue, shutdown, respawn.
//! Ponytail: shortest diff that satisfies the 8 criteria, no speculative abstractions.

pub mod secret;
pub mod lock;
pub mod port;
pub mod rpc;
pub mod strip;
pub mod engine;
pub mod extract;
pub mod client;

pub use client::{ClientError, RpcClient, RpcError};
pub use engine::{Engine, EngineError, StartOptions};
pub use lock::{DirLock, LockError};
pub use port::{pick_free_port, pick_free_port_in};
pub use rpc::{
    build_change_global_option, build_enqueue_with_auth, build_poll_batch, EnqueueKind,
    GlobalStat, StatusEntry, StoredRecord, Tick,
};
pub use secret::generate_secret;
pub use strip::strip_credentials;
