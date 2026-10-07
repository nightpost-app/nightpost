//! Nightpost engine. Both interfaces (terminal and graphical) only call into
//! this crate; nothing about identity, encryption or the network lives in them.

pub mod dbkey;
pub mod session;
pub mod store;

/// Version of the wire protocol. Peers must match exactly.
pub const PROTOCOL_VERSION: u16 = 1;
