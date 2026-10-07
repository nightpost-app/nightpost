//! An in-process stand-in for Tor, so the whole messaging path runs in tests
//! without a network. It keeps the one rule that matters: a service can only
//! be dialed with an authorized discovery key, and to anyone else it looks
//! exactly like one that is not there.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::StreamExt as _;
use futures::channel::mpsc;
use tokio::io::DuplexStream;

use super::{DiscoveryKey, Onion, Result, TransportError, discovery_key, none_authorized, onion};
use crate::dbkey::Secret;

/// Everything one test's devices can reach.
#[derive(Clone, Default)]
pub struct Network(Arc<Mutex<HashMap<Onion, Listing>>>);

struct Listing {
    authorized: Vec<DiscoveryKey>,
    incoming: mpsc::UnboundedSender<DuplexStream>,
}

impl Network {
    /// A device on this network.
    pub fn node(&self) -> MemoryTransport {
        MemoryTransport { net: self.clone(), presented: Mutex::default() }
    }
}

pub struct MemoryTransport {
    net: Network,
    presented: Mutex<HashMap<Onion, DiscoveryKey>>,
}

pub struct MemoryService {
    net: Network,
    onion: Onion,
    incoming: mpsc::UnboundedReceiver<DuplexStream>,
}

impl super::Transport for MemoryTransport {
    type Stream = DuplexStream;
    type Service = MemoryService;

    async fn serve(&self, seed: &Secret, authorized: &[DiscoveryKey]) -> Result<MemoryService> {
        if authorized.is_empty() {
            return Err(none_authorized());
        }
        let onion = onion(seed);
        let (tx, rx) = mpsc::unbounded();
        let mut net = self.net.0.lock().unwrap();
        if net.contains_key(&onion) {
            return Err(TransportError("this service is already running".into()));
        }
        net.insert(onion, Listing { authorized: authorized.to_vec(), incoming: tx });
        Ok(MemoryService { net: self.net.clone(), onion, incoming: rx })
    }

    fn present(&self, onion: Onion, secret: &Secret) -> Result<()> {
        self.presented.lock().unwrap().insert(onion, discovery_key(secret));
        Ok(())
    }

    async fn dial(&self, onion: Onion) -> Result<DuplexStream> {
        let key = self.presented.lock().unwrap().get(&onion).copied();
        let net = self.net.0.lock().unwrap();
        let listing = net
            .get(&onion)
            .filter(|l| key.is_some_and(|k| l.authorized.contains(&k)))
            .ok_or_else(|| TransportError("onion service not found".into()))?;
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        listing
            .incoming
            .unbounded_send(theirs)
            .map_err(|_| TransportError("onion service not found".into()))?;
        Ok(ours)
    }
}

impl super::Service for MemoryService {
    type Stream = DuplexStream;

    fn onion(&self) -> Onion {
        self.onion
    }

    fn authorize(&self, authorized: &[DiscoveryKey]) -> Result<()> {
        if authorized.is_empty() {
            return Err(none_authorized());
        }
        if let Some(listing) = self.net.0.lock().unwrap().get_mut(&self.onion) {
            listing.authorized = authorized.to_vec();
        }
        Ok(())
    }

    async fn accept(&mut self) -> Option<DuplexStream> {
        self.incoming.next().await
    }
}

impl Drop for MemoryService {
    fn drop(&mut self) {
        self.net.0.lock().unwrap().remove(&self.onion);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Service as _, Transport as _};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    fn secret(n: u8) -> Secret {
        Secret::new([n; 32])
    }

    #[tokio::test]
    async fn only_authorized_keys_reach_a_service() {
        let net = Network::default();
        let (alice, bob, carol) = (net.node(), net.node(), net.node());
        let mut service = alice.serve(&secret(1), &[discovery_key(&secret(2))]).await.unwrap();
        let address = service.onion();

        // Carol has no key, then the wrong one: both look like nothing there.
        assert!(carol.dial(address).await.is_err());
        carol.present(address, &secret(3)).unwrap();
        assert!(carol.dial(address).await.is_err());

        bob.present(address, &secret(2)).unwrap();
        let mut stream = bob.dial(address).await.unwrap();
        let mut accepted = service.accept().await.unwrap();
        stream.write_all(b"hi").await.unwrap();
        let mut buf = [0; 2];
        accepted.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hi");

        // Revoked: Bob is a stranger again. Then Carol is let in.
        service.authorize(&[discovery_key(&secret(3))]).unwrap();
        assert!(bob.dial(address).await.is_err());
        assert!(carol.dial(address).await.is_ok());

        drop(service);
        assert!(carol.dial(address).await.is_err());
    }
}
