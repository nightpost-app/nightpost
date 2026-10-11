//! Nightpost engine. Both interfaces (terminal and graphical) only call into
//! this crate; nothing about identity, encryption or the network lives in them.

pub mod api;
pub mod daemon;
pub mod dbkey;
pub mod identity;
pub mod invite;
pub mod proto;
pub mod session;
pub mod store;
pub mod text;
pub mod transport;

/// Nightpost's version, the same for the daemon and every interface.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Version of the wire protocol. Peers must match exactly.
pub const PROTOCOL_VERSION: u16 = 1;
