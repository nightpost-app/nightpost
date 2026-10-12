//! Tor through arti, as murmure runs it: keys in memory only, rustls, and
//! every service in restricted discovery mode.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arti_client::config::pt::TransportConfigBuilder;
use arti_client::config::{BridgeConfigBuilder, CfgPath, Reconfigure, TorClientConfig, TorClientConfigBuilder};
use arti_client::{DataStream, KeystoreSelector, TorClient};
use data_encoding::BASE32_NOPAD;
use futures::{Stream, StreamExt as _};
use safelog::DisplayRedacted as _;
use tor_cell::relaycell::msg::Connected;
use tor_config::{BoolOrAuto, ExplicitOrAuto};
use tor_hscrypto::pk::{HsClientDescEncKey, HsClientDescEncSecretKey, HsId, HsIdKeypair};
use tor_hsservice::config::OnionServiceConfigBuilder;
use tor_hsservice::config::restricted_discovery::HsClientNickname;
use tor_hsservice::{HsNickname, OnionServiceConfig, RunningOnionService, handle_rend_requests};
use tor_keymgr::config::ArtiKeystoreKind;
use tor_llcrypto::pk::{curve25519, ed25519};
use tor_rtcompat::PreferredRuntime;

use super::{DiscoveryKey, Onion, Result, TransportError, none_authorized, onion};
use crate::api::Network;
use crate::dbkey::Secret;
use crate::t;

/// The virtual port Nightpost listens on inside its onion services.
const PORT: u16 = 7777;

/// Tor Browser's default Snowflake bridges (tor-browser-build,
/// `projects/tor-expert-bundle/pt_config.json`, 15.0.24). Snowflake looks
/// like a video call and needs no bridge address of ours.
const SNOWFLAKE: [&str; 2] = [
    "snowflake 192.0.2.3:80 2B280B23E1107BB62ABFC40DDCC8824814F80A72 fingerprint=2B280B23E1107BB62ABFC40DDCC8824814F80A72 url=https://1098762253.rsc.cdn77.org/ fronts=app.datapacket.com,www.datapacket.com ice=stun:stun.epygi.com:3478,stun:stun.uls.co.za:3478,stun:stun.voipgate.com:3478,stun:stun.mixvoip.com:3478,stun:stun.telnyx.com:3478,stun:stun.hot-chilli.net:3478,stun:stun.fitauto.ru:3478,stun:stun.m-online.net:3478 utls-imitate=hellorandomizedalpn",
    "snowflake 192.0.2.4:80 8838024498816A039FCBBAB14E6F40A0843051FA fingerprint=8838024498816A039FCBBAB14E6F40A0843051FA url=https://1098762253.rsc.cdn77.org/ fronts=app.datapacket.com,www.datapacket.com ice=stun:stun.epygi.com:3478,stun:stun.uls.co.za:3478,stun:stun.voipgate.com:3478,stun:stun.mixvoip.com:3478,stun:stun.telnyx.com:3478,stun:stun.hot-chilli.net:3478,stun:stun.fitauto.ru:3478,stun:stun.m-online.net:3478 utls-imitate=hellorandomizedalpn",
];

/// No progress for this long and Tor is taken for blocked on this network:
/// the client switches to the bridges, and back without them when those
/// stall in turn (a laptop that moved). Snowflake starts slowly, so the
/// bridges get longer.
const STALL: Duration = Duration::from_secs(30);
const STALL_BRIDGED: Duration = Duration::from_secs(120);
/// How long a probe (see [`reachable`]) may take.
const PROBE: Duration = Duration::from_secs(30);
/// Onion service circuits a probe may try to build before it fails.
const PROBE_TRIES: usize = 4;

/// For networks that block Tor (some university or company Wi-Fi): lyrebird,
/// the Tor Project's pluggable-transport program (Snowflake, WebTunnel,
/// obfs4), and the user's own bridges to reach through it.
pub struct Bridges {
    pub lyrebird: PathBuf,
    /// Bridge lines as Tor Browser or bridges.torproject.org gives them;
    /// none means Tor Browser's Snowflake bridges. Used as soon as they change.
    pub lines: tokio::sync::watch::Receiver<Vec<String>>,
}

/// Errors on the first line Tor would not take.
pub fn check_bridges(lines: &[String]) -> Result<()> {
    lines.iter().try_for_each(|line| bridge(line).map(drop))
}

fn bridge(line: &str) -> Result<BridgeConfigBuilder> {
    line.parse().map_err(|e| TransportError(format!("bridge line {line:?}: {e}")))
}

pub struct TorTransport {
    client: Arc<TorClient<PreferredRuntime>>,
    state_dir: PathBuf,
}

pub struct TorService {
    service: Arc<RunningOnionService>,
    nickname: HsNickname,
    onion: Onion,
}

impl TorTransport {
    /// Bootstraps a client keeping its state and cache in the two directories.
    /// `progress` gets where Tor stands, with a line to show ("Tor: 45%"),
    /// during the bootstrap and after it: a cold first start downloads the
    /// whole consensus and takes minutes, and a number on screen is what
    /// tells that apart from a hang.
    ///
    /// With `bridges`, the client starts the way that worked last time and
    /// switches between direct and bridged whenever it stops making progress
    /// (see [`STALL`]), for as long as it runs.
    pub async fn bootstrap(
        state_dir: &Path,
        cache_dir: &Path,
        bridges: Option<Bridges>,
        progress: impl FnMut(&Network) + Send + 'static,
    ) -> Result<TorTransport> {
        create_private_dir(state_dir)?;
        create_private_dir(cache_dir)?;
        let lines = bridges.as_ref().map(|b| b.lines.borrow().clone());
        let configs = configs(state_dir, cache_dir, bridges.as_ref().map(|b| (b.lyrebird.as_path(), lines.as_deref().unwrap_or_default())))?;
        let marker = state_dir.join("bridged");
        let bridged = configs.len() == 2 && marker.exists();
        forget_removed_bridges(&state_dir.join("state").join("guards.json"));

        let client = TorClient::builder()
            .config(configs[usize::from(bridged)].clone())
            .create_unbootstrapped_async()
            .await
            .map_err(|e| describe(e, "creating the Tor client"))?;
        // Subscribed before bootstrapping, or the early progress is lost.
        let events = client.bootstrap_events();
        let dirs = (state_dir.to_path_buf(), cache_dir.to_path_buf());
        tokio::spawn(watch(client.clone(), events, configs, bridged, marker, bridges, dirs, progress));
        client.bootstrap().await.map_err(|e| describe(e, "bootstrapping Tor"))?;
        Ok(TorTransport { client, state_dir: state_dir.to_path_buf() })
    }
}

/// Drops from arti's saved guards the bridges it marked as no longer
/// configured. arti never takes such a bridge back once its lines are set
/// again: it only fetches the descriptors of bridges still listed, and
/// without one cannot tell the line from the bridge it knew, so Tor stays
/// stuck, restart after restart (bridges set, cleared, then set again).
/// Forgotten, the bridge is taken from its line like a new one. Before
/// the client starts: arti keeps the file to itself while it runs.
fn forget_removed_bridges(guards: &Path) {
    let Ok(text) = std::fs::read_to_string(guards) else { return };
    let Ok(mut state) = serde_json::from_str::<serde_json::Value>(&text) else { return };
    let Some(bridges) = state.get_mut("bridges") else { return };
    let removed: Vec<serde_json::Value> = bridges["guards"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|g| !g["unlisted_since"].is_null())
        .map(|g| g["id"].clone())
        .collect();
    if removed.is_empty() {
        return;
    }
    for list in ["guards", "confirmed"] {
        if let Some(list) = bridges[list].as_array_mut() {
            list.retain(|g| !removed.contains(&g["id"]) && !removed.contains(g));
        }
    }
    let new = guards.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&new, state.to_string()).and_then(|()| std::fs::rename(&new, guards)) {
        tracing::warn!("forgetting removed bridges in {}: {e}", guards.display());
    }
}

/// The direct configuration and, with bridges, the bridged one. Both list
/// the bridges and lyrebird; only `enabled` differs, so switching is a
/// reconfiguration of the running client.
/// `bridges` = lyrebird and the bridge lines.
fn configs(state_dir: &Path, cache_dir: &Path, bridges: Option<(&Path, &[String])>) -> Result<Vec<TorClientConfig>> {
    let base = || {
        let mut builder = TorClientConfigBuilder::from_directories(state_dir, cache_dir);
        builder.storage().keystore().primary().kind(ExplicitOrAuto::Explicit(ArtiKeystoreKind::Ephemeral));
        builder
    };
    let build = |builder: TorClientConfigBuilder| builder.build().map_err(|e| TransportError(format!("Tor configuration: {e}")));
    let Some((program, lines)) = bridges else { return Ok(vec![build(base())?]) };
    let lines: Vec<&str> = if lines.is_empty() { SNOWFLAKE.to_vec() } else { lines.iter().map(String::as_str).collect() };
    let mut configs = Vec::new();
    for enabled in [false, true] {
        let mut builder = base();
        for line in &lines {
            builder.bridges().bridges().push(bridge(line)?);
        }
        let mut lyrebird = TransportConfigBuilder::default();
        let protocols = ["snowflake", "webtunnel", "obfs4"].map(|p| p.parse().expect("a valid transport name"));
        lyrebird.protocols(protocols.into()).path(CfgPath::new_literal(program)).run_on_startup(false);
        builder.bridges().transports().push(lyrebird);
        builder.bridges().enabled(BoolOrAuto::Explicit(enabled));
        configs.push(build(builder)?);
    }
    Ok(configs)
}

/// Reports progress, and switches between `configs` when it stalls: no
/// progress while bootstrapping, or, once bootstrapped, two failed probes
/// in a row. Remembers in `marker` the way that last got the client ready.
/// New bridge lines are used at once.
#[allow(clippy::too_many_arguments)]
async fn watch(
    client: Arc<TorClient<PreferredRuntime>>,
    mut events: arti_client::status::BootstrapEvents,
    mut both: Vec<TorClientConfig>,
    mut bridged: bool,
    marker: PathBuf,
    bridges: Option<Bridges>,
    (state_dir, cache_dir): (PathBuf, PathBuf),
    mut progress: impl FnMut(&Network),
) {
    let (mut best, mut since, mut saved, mut failures) = (-1.0_f32, Instant::now(), None, 0);
    let mut net = Network { bridged, ..Network::default() };
    macro_rules! say {
        ($line:expr $(,)?) => {{
            net.line = $line;
            progress(&net);
        }};
    }
    // After a switch: a new route, nothing known about it yet.
    let restart = |net: &mut Network, bridged: bool, client: &TorClient<PreferredRuntime>| {
        *net = Network { percent: percent(client.bootstrap_status().as_frac()), bridged, ..Network::default() };
    };
    let mut updates = bridges.as_ref().map(|b| b.lines.clone());
    loop {
        let stall = if bridged { STALL_BRIDGED } else { STALL };
        let next = tokio::select! {
            next = tokio::time::timeout_at((since + stall).into(), events.next()) => next,
            changed = async {
                match updates.as_mut() {
                    Some(u) => u.changed().await,
                    None => std::future::pending().await,
                }
            } => {
                let (Some(b), Some(u)) = (bridges.as_ref(), updates.as_mut()) else { continue };
                if changed.is_err() {
                    // Nobody left to change them: what there is stays.
                    updates = None;
                    continue;
                }
                let lines = u.borrow_and_update().clone();
                match configs(&state_dir, &cache_dir, Some((&b.lyrebird, &lines))) {
                    Ok(new) => both = new,
                    Err(e) => {
                        say!(t!("tor-error", error = e));
                        continue;
                    }
                }
                // Lines of one's own are there to be used.
                bridged |= !lines.is_empty();
                if let Err(e) = client.reconfigure(&both[usize::from(bridged)], Reconfigure::AllOrNothing) {
                    say!(t!("tor-switch-failed", error = e));
                }
                restart(&mut net, bridged, &client);
                say!(if bridged { t!("tor-reconnecting-bridges") } else { t!("tor-reconnecting") });
                (best, since, saved, failures) = (client.bootstrap_status().as_frac(), Instant::now(), None, 0);
                continue;
            }
        };
        match next {
            Ok(None) => return,
            Ok(Some(status)) => {
                if status.as_frac() > best {
                    (best, since) = (status.as_frac(), Instant::now());
                }
                if status.ready_for_traffic() && saved != Some(bridged) {
                    let _ = if bridged { std::fs::write(&marker, b"") } else { std::fs::remove_file(&marker).or(Ok(())) };
                    saved = Some(bridged);
                }
                net.percent = percent(status.as_frac());
                say!(match status.blocked() {
                    // arti's reason is English and technical: the log keeps it.
                    Some(why) => {
                        tracing::info!("Tor bootstrap blocked: {}", why.message());
                        t!("tor-stuck", percent = net.percent)
                    }
                    None => t!("tor-percent", percent = net.percent),
                });
            }
            Err(_) if both.len() < 2 => {
                if client.bootstrap_status().ready_for_traffic() {
                    let (entry, circuit) = reachable(&client).await;
                    (net.entry, net.circuit) = (Some(entry), Some(circuit));
                    progress(&net);
                }
                since = Instant::now();
            }
            Err(_) => {
                // Bootstrapped, arti's status says "online" for good after
                // one success: ask the network itself. Still bootstrapping
                // with no progress: stalled already.
                if client.bootstrap_status().ready_for_traffic() {
                    let (entry, circuit) = reachable(&client).await;
                    (net.entry, net.circuit) = (Some(entry), Some(circuit));
                    progress(&net);
                    failures = if circuit { 0 } else { failures + 1 };
                    if failures < 2 {
                        since = Instant::now();
                        continue;
                    }
                }
                bridged = !bridged;
                if let Err(e) = client.reconfigure(&both[usize::from(bridged)], Reconfigure::AllOrNothing) {
                    say!(t!("tor-switch-failed", error = e));
                }
                restart(&mut net, bridged, &client);
                say!(if bridged { t!("tor-to-bridge") } else { t!("tor-from-bridge") });
                (best, since, saved, failures) = (client.bootstrap_status().as_frac(), Instant::now(), None, 0);
            }
        }
    }
}

fn percent(frac: f32) -> u8 {
    (frac * 100.0).round().clamp(0.0, 100.0) as u8
}

/// Whether Tor gets through right now: a fresh one-hop circuit to a guard
/// (retired afterwards, so the next probe builds a new one), and a circuit of
/// the kind onion services need (our guard, then the vanguards). The first
/// alone is not enough: on some filtered networks (a university Wi-Fi) a few
/// relays answer and Tor says 100%, while onion service circuits never get
/// built. The second alone comes from a pool built ahead, and so notices
/// a network that just started blocking only once the pool runs dry.
/// Returns whether each of the two got through.
async fn reachable(client: &TorClient<PreferredRuntime>) -> (bool, bool) {
    let (Ok(circmgr), Ok(pool), Ok(dirmgr)) = (client.circmgr(), client.hs_circ_pool(), client.dirmgr()) else { return (true, true) };
    let Ok(netdir) = dirmgr.timely_netdir() else { return (false, false) };
    let started = Instant::now();
    let one_hop = match tokio::time::timeout(PROBE, circmgr.get_or_launch_dir(netdir.as_ref().into())).await {
        Ok(Ok(tunnel)) => {
            circmgr.retire_circ(&tunnel.unique_id());
            true
        }
        _ => false,
    };
    // One attempt each (arti leaves retrying to the caller): a single failed
    // circuit is common on Tor and says nothing about the network.
    let onion = one_hop
        && tokio::time::timeout(PROBE, async {
            for _ in 0..PROBE_TRIES {
                match pool.get_or_launch_client_rend(&netdir).await {
                    Ok(_) => return true,
                    Err(e) => tracing::debug!("Tor probe: onion circuit: {e}"),
                }
            }
            false
        })
        .await
        .unwrap_or(false);
    tracing::debug!("Tor probe: one hop {one_hop}, onion circuit {onion}, {:?}", started.elapsed());
    (one_hop, onion)
}

impl super::Transport for TorTransport {
    type Stream = DataStream;
    type Service = TorService;
    type Incoming = Pin<Box<dyn Stream<Item = DataStream> + Send>>;

    async fn serve(&self, seed: &Secret, authorized: &[DiscoveryKey]) -> Result<(TorService, Self::Incoming)> {
        let onion = onion(seed);
        // Before 0.1.0-alpha.1 the state was named after the address itself.
        let old = self.state_dir.join("hss").join(format!("n{}", BASE32_NOPAD.encode(&onion[..10]).to_ascii_lowercase()));
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_file(old.with_extension("lock"));
        let nickname: HsNickname = slug(&seed[..])
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
        // A stream that fails to open is skipped: one bad rendezvous must not
        // take the service down.
        let incoming = handle_rend_requests(requests).filter_map(|request| async move {
            match request.accept(Connected::new_empty()).await {
                Ok(stream) => Some(stream),
                Err(e) => {
                    tracing::warn!("accepting an incoming stream failed: {e}");
                    None
                }
            }
        });
        Ok((TorService { service, nickname, onion }, Box::pin(incoming)))
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
    fn onion(&self) -> Onion {
        self.onion
    }

    fn authorize(&self, authorized: &[DiscoveryKey]) -> Result<()> {
        self.service
            .reconfigure(service_config(&self.nickname, authorized)?, Reconfigure::AllOrNothing)
            .map_err(|e| TransportError(format!("updating the authorized keys: {e}")))
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
/// 80 bits of a hash of the key in base32. The service's names a folder on
/// disk, so it must not give the address away.
fn slug(key: &[u8]) -> String {
    use sha2::Digest as _;
    let hash = sha2::Sha256::new().chain_update(b"nightpost-hs-nickname").chain_update(key).finalize();
    format!("n{}", BASE32_NOPAD.encode(&hash[..10]).to_ascii_lowercase())
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
    fn the_bridges_parse_and_differ_only_in_being_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let lyrebird = Path::new("/opt/lyrebird");
        let both = configs(dir.path(), dir.path(), Some((lyrebird, &[]))).unwrap();
        assert_eq!(both.len(), 2);
        assert_ne!(both[0], both[1]);
        assert_eq!(configs(dir.path(), dir.path(), None).unwrap().len(), 1);
        assert!(configs(dir.path(), dir.path(), Some((lyrebird, &["snowflake nonsense".into()]))).is_err());

        assert!(check_bridges(&["obfs4 nonsense".into()]).is_err());
        check_bridges(&["obfs4 192.0.2.5:443 0123456789ABCDEF0123456789ABCDEF01234567 cert=abc iat-mode=0".into()]).unwrap();
    }

    #[test]
    fn removed_bridges_are_forgotten_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("guards.json");
        let guard = |id: &str, unlisted: Option<&str>| serde_json::json!({ "id": { "rsa": id }, "unlisted_since": unlisted });
        let state = serde_json::json!({
            "default": { "guards": [guard("d", Some("2026-10-11T16:18:51Z"))], "confirmed": [] },
            "bridges": {
                "guards": [guard("a", None), guard("b", Some("2026-10-11T16:18:51Z"))],
                "confirmed": [{ "rsa": "a" }, { "rsa": "b" }],
            },
        });
        std::fs::write(&file, state.to_string()).unwrap();
        forget_removed_bridges(&file);
        let after: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(after["bridges"]["guards"], serde_json::json!([guard("a", None)]));
        assert_eq!(after["bridges"]["confirmed"], serde_json::json!([{ "rsa": "a" }]));
        assert_eq!(after["default"], state["default"]);
        // No file, or not arti's: left alone.
        forget_removed_bridges(&dir.path().join("none.json"));
        std::fs::write(&file, "not json").unwrap();
        forget_removed_bridges(&file);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "not json");
    }

    #[test]
    fn the_state_folder_does_not_give_the_address_away() {
        let seed = Secret::new([1; 32]);
        let name = slug(&seed[..]);
        assert_eq!(name, slug(&seed[..]));
        assert_ne!(name, slug(&[2; 32]));
        let address = BASE32_NOPAD.encode(&onion(&seed)).to_ascii_lowercase();
        assert!(!address.starts_with(&name[1..5]), "{name} {address}");
        HsNickname::try_from(name).unwrap();
    }

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
                TorTransport::bootstrap(&state, &cache, None, move |net| println!("{name}: {}", net.line)).await.unwrap()
            }
        };
        let alice = boot("alice").await;
        let (bob, carol) = (boot("bob").await, boot("carol").await);

        let (alice_seed, bob_key, carol_key) = (Secret::new([1; 32]), Secret::new([2; 32]), Secret::new([3; 32]));
        let (service, mut incoming) = alice.serve(&alice_seed, &[discovery_key(&bob_key)]).await.unwrap();
        let address = service.onion();
        bob.present(address, &bob_key).unwrap();
        carol.present(address, &carol_key).unwrap();

        let server = tokio::spawn(async move {
            let mut stream = incoming.next().await.unwrap();
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
