//! Tor through arti, as murmure runs it: keys in memory only, rustls, and
//! every service in restricted discovery mode.

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use arti_client::config::{Reconfigure, TorClientConfigBuilder};
use arti_client::{DataStream, KeystoreSelector, TorClient};
use data_encoding::BASE32_NOPAD;
use futures::{Stream, StreamExt as _};
use safelog::DisplayRedacted as _;
use tor_cell::relaycell::msg::Connected;
use tor_config::ExplicitOrAuto;
use tor_hscrypto::pk::{HsClientDescEncKey, HsClientDescEncSecretKey, HsId, HsIdKeypair};
use tor_hsservice::config::OnionServiceConfigBuilder;
use tor_hsservice::config::restricted_discovery::HsClientNickname;
use tor_hsservice::{HsNickname, OnionServiceConfig, RunningOnionService, StreamRequest, handle_rend_requests};
use tor_keymgr::config::ArtiKeystoreKind;
use tor_llcrypto::pk::{curve25519, ed25519};
use tor_rtcompat::PreferredRuntime;

use super::{DiscoveryKey, Onion, Result, TransportError, none_authorized, onion};
use crate::dbkey::Secret;

/// The virtual port Nightpost listens on inside its onion services.
const PORT: u16 = 7777;

pub struct TorTransport {
    client: Arc<TorClient<PreferredRuntime>>,
}

pub struct TorService {
    service: Arc<RunningOnionService>,
    nickname: HsNickname,
    onion: Onion,
    incoming: Pin<Box<dyn Stream<Item = StreamRequest> + Send>>,
}

impl TorTransport {
    /// Bootstraps a client keeping its state and cache in the two directories.
    /// `progress` gets a fraction from 0 to 1 and, when arti believes it is
    /// stuck, why: a cold first start downloads the whole consensus and takes
    /// minutes, and a number on screen is what tells that apart from a hang.
    pub async fn bootstrap(
        state_dir: &Path,
        cache_dir: &Path,
        mut progress: impl FnMut(f32, Option<String>),
    ) -> Result<TorTransport> {
        create_private_dir(state_dir)?;
        create_private_dir(cache_dir)?;
        let mut builder = TorClientConfigBuilder::from_directories(state_dir, cache_dir);
        builder.storage().keystore().primary().kind(ExplicitOrAuto::Explicit(ArtiKeystoreKind::Ephemeral));
        let config = builder.build().map_err(|e| TransportError(format!("Tor configuration: {e}")))?;

        let client = TorClient::builder()
            .config(config)
            .create_unbootstrapped_async()
            .await
            .map_err(|e| describe(e, "creating the Tor client"))?;
        // Subscribed before bootstrapping, or the early progress is lost.
        // Scoped: the bootstrap future borrows the client.
        {
            let mut events = client.bootstrap_events();
            let booting = client.bootstrap();
            futures::pin_mut!(booting);
            loop {
                tokio::select! {
                    outcome = &mut booting => {
                        outcome.map_err(|e| describe(e, "bootstrapping Tor"))?;
                        break;
                    }
                    Some(status) = events.next() => {
                        progress(status.as_frac(), status.blocked().map(|b| b.message().to_string()));
                    }
                }
            }
        }
        Ok(TorTransport { client })
    }
}

impl super::Transport for TorTransport {
    type Stream = DataStream;
    type Service = TorService;

    async fn serve(&self, seed: &Secret, authorized: &[DiscoveryKey]) -> Result<TorService> {
        let onion = onion(seed);
        let nickname: HsNickname = slug(&onion)
            .try_into()
            .map_err(|e| TransportError(format!("service nickname: {e}")))?;
        let config = service_config(&nickname, authorized)?;
        let keypair = HsIdKeypair::from(ed25519::ExpandedKeypair::from(&ed25519::Keypair::from_bytes(seed)));
        // The keystore keeps the key after a service stops, so serving the
        // same seed again finds it there: same seed, same key.
        let launched = match self.client.launch_onion_service_with_hsid(config.clone(), keypair) {
            Err(e) if is_key_already_exists(&e) => {
                self.client.launch_onion_service(config).map(|s| s.map(|(s, r)| (s, r.boxed())))
            }
            other => other.map(|s| s.map(|(s, r)| (s, r.boxed()))),
        };
        let (service, requests) = launched
            .map_err(|e| describe(e, "launching the onion service"))?
            .ok_or_else(|| TransportError("the onion service is disabled".into()))?;
        if service.onion_address().map(<[u8; 32]>::from) != Some(onion) {
            return Err(TransportError("arti published a different address than our key".into()));
        }
        Ok(TorService { service, nickname, onion, incoming: Box::pin(handle_rend_requests(requests)) })
    }

    fn present(&self, onion: Onion, secret: &Secret) -> Result<()> {
        let hsid = HsId::from(onion);
        self.client
            .remove_service_discovery_key(KeystoreSelector::Primary, hsid)
            .map_err(|e| describe(e, "replacing a discovery key"))?;
        let secret = HsClientDescEncSecretKey::from(curve25519::StaticSecret::from(**secret));
        self.client
            .insert_service_discovery_key(KeystoreSelector::Primary, hsid, secret)
            .map_err(|e| describe(e, "storing a discovery key"))?;
        Ok(())
    }

    async fn dial(&self, onion: Onion) -> Result<DataStream> {
        let host = HsId::from(onion).display_unredacted().to_string();
        self.client
            .connect((host.as_str(), PORT))
            .await
            .map_err(|e| describe(e, "dialing an onion service"))
    }
}

impl super::Service for TorService {
    type Stream = DataStream;

    fn onion(&self) -> Onion {
        self.onion
    }

    fn authorize(&self, authorized: &[DiscoveryKey]) -> Result<()> {
        self.service
            .reconfigure(service_config(&self.nickname, authorized)?, Reconfigure::AllOrNothing)
            .map_err(|e| TransportError(format!("updating the authorized keys: {e}")))
    }

    /// A stream that fails to open is skipped: one bad rendezvous must not
    /// take the service down.
    async fn accept(&mut self) -> Option<DataStream> {
        while let Some(request) = self.incoming.next().await {
            match request.accept(Connected::new_empty()).await {
                Ok(stream) => return Some(stream),
                Err(e) => tracing::warn!("accepting an incoming stream failed: {e}"),
            }
        }
        None
    }
}

/// Always restricted. arti refuses a restricted config with no keys, and an
/// open descriptor would tell anyone holding the address when we are online,
/// so an empty list is an error and the caller stops the service instead.
fn service_config(nickname: &HsNickname, authorized: &[DiscoveryKey]) -> Result<OnionServiceConfig> {
    if authorized.is_empty() {
        return Err(none_authorized());
    }
    let mut builder = OnionServiceConfigBuilder::default();
    builder.nickname(nickname.clone());
    let restricted = builder.restricted_discovery();
    restricted.enabled(true);
    // Passed in memory: arti's other provider reads one plaintext file per
    // client, which would list our contacts on disk.
    let keys = restricted.static_keys().access();
    for key in authorized {
        let nickname: HsClientNickname = slug(key)
            .parse()
            .map_err(|e| TransportError(format!("client nickname: {e}")))?;
        keys.push((nickname, HsClientDescEncKey::from(curve25519::PublicKey::from(*key))));
    }
    builder.build().map_err(|e| TransportError(format!("onion service configuration: {e}")))
}

/// A name arti accepts (lowercase letters, digits), unique per key: "n" and
/// 80 bits of the key in base32. Says nothing arti does not already hold.
fn slug(key: &[u8; 32]) -> String {
    format!("n{}", BASE32_NOPAD.encode(&key[..10]).to_ascii_lowercase())
}

fn create_private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| TransportError(format!("creating {}: {e}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| TransportError(format!("chmod 700 {}: {e}", path.display())))?;
    }
    Ok(())
}

/// `KeyAlreadyExists` behind arti-client's opaque error: its kind is the
/// generic `BadApiUsage`, so only the message tells it apart.
fn is_key_already_exists(err: &arti_client::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = source {
        if e.to_string().contains("Key already exists") {
            return true;
        }
        source = e.source();
    }
    false
}

/// arti's error, with its stable kind and the whole source chain.
fn describe(err: arti_client::Error, doing: &str) -> TransportError {
    use arti_client::HasKind as _;
    let kind = err.kind();
    let mut chain = vec![err.to_string()];
    let mut source = std::error::Error::source(&err);
    while let Some(e) = source {
        chain.push(e.to_string());
        source = e.source();
    }
    TransportError(format!("{doing} failed [{kind:?}]: {}", chain.join(": ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Service as _, Transport as _, discovery_key};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[test]
    fn the_service_config_is_always_restricted() {
        let nickname: HsNickname = slug(&[1; 32]).try_into().unwrap();
        assert!(service_config(&nickname, &[]).is_err());
        service_config(&nickname, &[[2; 32], [3; 32]]).unwrap();
    }

    /// Two clients on the real Tor network: Bob reaches Alice with his key,
    /// Carol, who knows the address but holds no authorized key, does not.
    ///
    /// Needs the network and takes minutes on a cold cache:
    /// `cargo test -p nightpost-core --release -- --ignored --nocapture over_tor`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the Tor network; minutes"]
    async fn over_tor_only_authorized_clients_connect() {
        let dir = tempfile::tempdir().unwrap();
        let boot = |name: &str| {
            let state = dir.path().join(name).join("state");
            let cache = dir.path().join(name).join("cache");
            let name = name.to_string();
            async move {
                TorTransport::bootstrap(&state, &cache, |f, blocked| {
                    if let Some(why) = blocked {
                        println!("{name}: stuck: {why}");
                    } else if f >= 1.0 {
                        println!("{name}: bootstrapped");
                    }
                })
                .await
                .unwrap()
            }
        };
        let alice = boot("alice").await;
        let (bob, carol) = (boot("bob").await, boot("carol").await);

        let (alice_seed, bob_key, carol_key) = (Secret::new([1; 32]), Secret::new([2; 32]), Secret::new([3; 32]));
        let mut service = alice.serve(&alice_seed, &[discovery_key(&bob_key)]).await.unwrap();
        let address = service.onion();
        bob.present(address, &bob_key).unwrap();
        carol.present(address, &carol_key).unwrap();

        let server = tokio::spawn(async move {
            let mut stream = service.accept().await.unwrap();
            let mut buf = [0; 5];
            stream.read_exact(&mut buf).await.unwrap();
            stream.write_all(b"world").await.unwrap();
            stream.flush().await.unwrap();
            (buf, service)
        });

        // The descriptor takes a while to reach the directories.
        let started = std::time::Instant::now();
        let mut stream = loop {
            match bob.dial(address).await {
                Ok(s) => break s,
                Err(e) if started.elapsed().as_secs() < 300 => {
                    println!("bob: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
                Err(e) => panic!("bob never reached alice: {e}"),
            }
        };
        println!("bob connected after {:?}", started.elapsed());
        stream.write_all(b"hello").await.unwrap();
        stream.flush().await.unwrap();
        let mut buf = [0; 5];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"world");
        let (got, _service) = server.await.unwrap();
        assert_eq!(&got, b"hello");

        // The descriptor is up and Bob just used it; Carol still cannot.
        let err = carol.dial(address).await.unwrap_err();
        println!("carol: {err}");
        assert!(err.0.contains("OnionServiceWrongClientAuth"), "{err}");
    }
}
