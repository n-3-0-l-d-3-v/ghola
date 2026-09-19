//! Synchronizing repositories over distrans (ticket 007). See
//! `docs/design/decisions/ADR-007-sync.md`.
//!
//! Layers, top to bottom: `client` (fetch / push / pull against any
//! `Remote`), `wire` + `server` (the request protocol, served by a pure
//! function over a repository), `net` (a `Remote` that carries the protocol
//! over distrans RPC, transport and a hostile simulated channel).

mod client;
mod net;
mod server;
mod wire;

pub use client::{
    fetch, list_refs, pull, push, Direct, FetchReport, PushReport, Remote, SyncError,
};
pub use net::{DistransRemote, NetConfig, NetStats};
pub use server::{plan, serve};
pub use wire::{BATCH_BYTES, FETCH, LIST_REFS, PUT_OBJECTS, UPDATE_REF};
