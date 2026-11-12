//! Where streams come from: Tor in real use, memory in tests.
//!
//! A device publishes an onion service under its own key, discoverable only
//! by holders of an authorized discovery key (Tor's restricted discovery).
//! Anyone else who knows the address can fetch the encrypted descriptor, and
//! so learn that one was published in the last few hours, but cannot open a
//! connection or tell when exactly we are online. To
//! reach a contact, the device first presents its discovery key for that
//! address, then dials. What travels on the stream is [`crate::proto`]'s
//! business.

use std::fmt;
use std::future::Future;

use tokio::io::{AsyncRead, AsyncWrite};
use tor_llcrypto::pk::{curve25519, ed25519};

use crate::dbkey::Secret;

pub mod memory;
pub mod tor;

/// An onion service address: its ed25519 public key.
pub type Onion = [u8; 32];
/// The public half of a discovery key (x25519).
pub type DiscoveryKey = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError(pub String);

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TransportError {}

pub type Result<T> = std::result::Result<T, TransportError>;

pub trait Transport: Send + Sync {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    type Service: Service;
    /// Incoming streams, until the service is dropped.
    type Incoming: futures::Stream<Item = Self::Stream> + Unpin + Send + 'static;

    /// Publishes the service whose key is `seed`. At least one key must be
    /// authorized: a service nobody may discover has no reason to run.
    fn serve(
        &self,
        seed: &Secret,
        authorized: &[DiscoveryKey],
    ) -> impl Future<Output = Result<(Self::Service, Self::Incoming)>> + Send;

    /// Files `secret` as the discovery key to use for `onion`, replacing any
    /// earlier one (an invitation's throwaway key gives way to the real one).
    fn present(&self, onion: Onion, secret: &Secret) -> Result<()>;

    fn dial(&self, onion: Onion) -> impl Future<Output = Result<Self::Stream>> + Send;
}

/// A running onion service. Dropping it takes it down.
pub trait Service: Send {
    fn onion(&self) -> Onion;

    /// Replaces the set of keys that may discover the service. Takes effect
    /// once the new descriptor is published, about a minute on Tor; already
    /// open streams are not cut.
    fn authorize(&self, authorized: &[DiscoveryKey]) -> Result<()>;

    /// Whether the service is believed reachable by those who may find it.
    /// For a transport that has no way to tell, yes.
    fn reachable(&self) -> bool {
        true
    }
}

/// The address of the service published with `seed`.
pub fn onion(seed: &Secret) -> Onion {
    ed25519::Keypair::from_bytes(seed).verifying_key().to_bytes()
}

/// The public half of the discovery key `secret`.
pub fn discovery_key(secret: &Secret) -> DiscoveryKey {
    curve25519::PublicKey::from(&curve25519::StaticSecret::from(**secret)).to_bytes()
}

fn none_authorized() -> TransportError {
    TransportError("an onion service needs at least one authorized discovery key".into())
}
