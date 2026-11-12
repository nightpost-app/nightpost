//! A mailbox: a device of an account that holds its public identity alone,
//! never the key pair. It keeps what contacts (and our own devices) leave
//! for one of our devices that is off, until that device fetches it, and
//! the one-time bundles our devices leave for contacts who never met them.
//!
//! It sees which account writes to which of our devices, and when; never
//! what. It never dials anyone.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Context as _, Result, bail, ensure};
use futures::StreamExt as _;
use libsignal_protocol::{IdentityKey, KeyPair, PrivateKey};
use rand::{Rng as _, TryRngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use tokio::task::spawn_local;
use tokio::time::{sleep, timeout};

use crate::PROTOCOL_VERSION;
use crate::api::hex;
use crate::daemon::{Contact, IDLE, OtherVersion, STEP, hang_up, handshake, hello, now_secs, step};
use crate::dbkey::Secret;
use crate::devices::{self, PAIRING_LIFETIME, Pairing, Step};
use crate::identity::{Role, SignedDeviceList};
use crate::proto::{self, Frame, Me, Peer, Proof, Service};
use crate::store::Store;
use crate::transport::{DiscoveryKey, Service as _, Transport, discovery_key, onion};

/// How many one-time bundles each of our devices keeps at the mailbox.
/// How often the service is looked at, and how long it may stay unreachable.
const CHECK: std::time::Duration = std::time::Duration::from_secs(60);
const UNREACHABLE: std::time::Duration = std::time::Duration::from_secs(20 * 60);

pub const BUNDLES: u8 = 10;

pub struct Mailbox<T: Transport> {
    store: Rc<Store>,
    net: T,
    keys: Keys,
    signing: PrivateKey,
    joined: RefCell<Joined>,
    service: RefCell<Option<T::Service>>,
}

/// This device's own, made at the first start.
#[derive(Serialize, Deserialize)]
struct Keys {
    key: Vec<u8>,
    seed: [u8; 32],
}

/// The account it joined, public halves only.
#[derive(Serialize, Deserialize, Clone)]
struct Joined {
    account: Vec<u8>,
    discovery: DiscoveryKey,
    device: u8,
    devices: SignedDeviceList,
}

impl<T: Transport + 'static> Mailbox<T> {
    /// Opens the mailbox, joining an account first if it has none: `show`
    /// gets the link for one of that account's devices to use. Must run
    /// inside a `LocalSet`.
    pub async fn open(store: impl Into<Rc<Store>>, net: T, show: impl FnOnce(&str)) -> Result<Rc<Self>> {
        let store = store.into();
        let keys: Keys = match store.get("mailbox", "keys")? {
            Some(bytes) => postcard::from_bytes(&bytes)?,
            None => {
                let mut rng = OsRng.unwrap_err();
                let keys = Keys { key: KeyPair::generate(&mut rng).private_key.serialize().to_vec(), seed: rng.random() };
                store.put("mailbox", "keys", &postcard::to_stdvec(&keys)?)?;
                keys
            }
        };
        let signing = PrivateKey::deserialize(&keys.key)?;
        let joined = match store.get("mailbox", "joined")? {
            Some(bytes) => postcard::from_bytes(&bytes)?,
            None => join(&store, &net, &keys, &signing, show).await?,
        };
        Ok(Rc::new(Mailbox { store, net, keys, signing, joined: RefCell::new(joined), service: RefCell::default() }))
    }

    /// Serves our devices and contacts, nobody else, until the service ends.
    pub async fn run(self: Rc<Self>) -> Result<()> {
        let (service, mut incoming) = self.net.serve(&Secret::new(self.keys.seed), &self.authorized()?).await?;
        *self.service.borrow_mut() = Some(service);
        let mut watch = std::pin::pin!(self.clone().watch());
        loop {
            tokio::select! {
                Some(stream) = incoming.next() => {
                    let mailbox = self.clone();
                    spawn_local(async move {
                        if let Err(e) = mailbox.answer(stream).await {
                            tracing::info!("mailbox link: {e:#}");
                        }
                    });
                }
                result = &mut watch => return result,
                else => return Ok(()),
            }
        }
    }

    /// Ends with an error once the service has not been reachable for
    /// `UNREACHABLE`: nobody can find a mailbox like that, and what the
    /// service manager does with a stopped one (start it again, from
    /// scratch) is what has worked.
    async fn watch(self: Rc<Self>) -> Result<()> {
        let mut since = None::<std::time::Instant>;
        loop {
            tokio::time::sleep(CHECK).await;
            let reachable = self.service.borrow().as_ref().is_none_or(|s| s.reachable());
            match since {
                _ if reachable => since = None,
                None => since = Some(std::time::Instant::now()),
                Some(at) if at.elapsed() > UNREACHABLE => bail!("not reachable for {} minutes: starting over", UNREACHABLE.as_secs() / 60),
                Some(_) => {}
            }
        }
    }

    fn authorized(&self) -> Result<Vec<DiscoveryKey>> {
        let mut keys = vec![self.joined.borrow().discovery];
        keys.extend(self.store.list("contact")?.iter().map(|(_, bytes)| Ok::<_, anyhow::Error>(postcard::from_bytes::<Contact>(bytes)?.discovery)).collect::<Result<Vec<_>>>()?);
        Ok(keys)
    }

    async fn answer(&self, mut stream: T::Stream) -> Result<()> {
        let hello = hello(&mut stream).await?;
        let own = hello.account == self.joined.borrow().account;
        if own {
            self.update_own(&hello.devices)?;
        }
        let joined = self.joined.borrow().clone();
        let identity = IdentityKey::decode(&joined.account)?;
        let me = Me { account: &identity, device: joined.device, key: &self.signing, devices: &joined.devices };
        let (service, challenge) = Service::on_hello(&me, onion(&Secret::new(self.keys.seed)), hello).map_err(handshake)?;
        let contact = match own {
            true => None,
            false => Some(self.contact(&service.hello().account)?.context("not a contact")?),
        };
        proto::send(&mut stream, &challenge).await?;
        let proof: Proof = timeout(STEP, proto::receive(&mut stream)).await??;
        let peer = service.on_proof(&proof, Some(contact.as_ref().map_or(&joined.devices, |c| &c.devices))).map_err(handshake)?;
        if let Some(mut contact) = contact {
            let presented = &service.hello().devices;
            if presented.verify(&peer.account, None).is_ok() && presented.seq() > contact.devices.seq() {
                contact.devices = presented.clone();
                self.store.put("contact", &hex(&contact.identity), &postcard::to_stdvec(&contact)?)?;
            }
        }
        // Only our own devices take things away or change what we hold.
        let ours = own && peer.devices.device(peer.device).is_some_and(|d| d.role == Role::Normal);
        self.serve(stream, &peer, ours).await
    }

    async fn serve(&self, mut stream: T::Stream, peer: &Peer, ours: bool) -> Result<()> {
        let account = peer.account.serialize();
        loop {
            let Ok(Ok(frame)) = timeout(IDLE, proto::receive::<Frame>(&mut stream)).await else { return Ok(()) };
            match frame {
                Frame::Deposit { device, id, kind, body } => {
                    let kept = self.is_for(device)? && self.store.deposit(device, &account, peer.device, &id, kind, &body, now_secs())?;
                    proto::send(&mut stream, &if kept { Frame::Deposited { device, id } } else { Frame::Refused }).await?;
                }
                Frame::BundleOf(device) => {
                    let reply = match self.store.take_bundle(device)? {
                        Some(bytes) => Frame::Bundle(postcard::from_bytes(&bytes)?),
                        None => Frame::Refused,
                    };
                    proto::send(&mut stream, &reply).await?;
                }
                Frame::Fetch if ours => {
                    for (account, sender, id, kind, body) in self.store.deposits(peer.device)? {
                        proto::send(&mut stream, &Frame::Stored { account, device: sender, id, kind, body }).await?;
                        match timeout(STEP, proto::receive(&mut stream)).await?? {
                            Frame::Ack { id: acked } if acked == id => self.store.fetched(peer.device, &id)?,
                            _ => bail!("expected an acknowledgement"),
                        }
                    }
                    let bundles = self.store.bundle_count(peer.device)?.min(i64::from(BUNDLES)) as u8;
                    proto::send(&mut stream, &Frame::Fetched { bundles }).await?;
                }
                Frame::Bundles(bundles) if ours => {
                    ensure!(self.store.bundle_count(peer.device)? + bundles.len() as i64 <= i64::from(BUNDLES), "too many bundles");
                    let mut keep = Vec::new();
                    for wire in &bundles {
                        // Its own, signed by the account: nobody else's to hand out.
                        let bundle = wire.bundle()?;
                        ensure!(*bundle.identity_key()? == peer.account, "a bundle of another account");
                        ensure!(u8::from(bundle.device_id()?) == peer.device, "a bundle of another device");
                        keep.push(postcard::to_stdvec(wire)?);
                    }
                    self.store.add_bundles(peer.device, &keep)?;
                }
                Frame::Contacts(contacts) if ours => self.set_contacts(&contacts)?,
                frame => bail!("not for a mailbox: {frame:?}"),
            }
        }
    }

    /// One of the account's devices that reads messages.
    fn is_for(&self, device: u8) -> Result<bool> {
        let joined = self.joined.borrow();
        let list = joined.devices.verify(&IdentityKey::decode(&joined.account)?, None).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok(list.device(device).is_some_and(|d| d.role == Role::Normal))
    }

    fn contact(&self, account: &[u8]) -> Result<Option<Contact>> {
        Ok(match self.store.get("contact", &hex(account))? {
            Some(bytes) => Some(postcard::from_bytes(&bytes)?),
            None => None,
        })
    }

    /// Keeps our newer device list; what was kept for a device gone goes.
    fn update_own(&self, presented: &SignedDeviceList) -> Result<()> {
        let mut joined = self.joined.borrow_mut();
        let identity = IdentityKey::decode(&joined.account)?;
        let new = presented.verify(&identity, None).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let old = joined.devices.verify(&identity, None).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        if new.seq <= old.seq {
            return Ok(());
        }
        joined.devices = presented.clone();
        let bytes = postcard::to_stdvec(&*joined)?;
        self.store.atomically(|| {
            for gone in old.devices.iter().filter(|d| new.device(d.id).is_none()) {
                self.store.forget_mailbox_device(gone.id)?;
            }
            self.store.put("mailbox", "joined", &bytes)
        })?;
        Ok(())
    }

    /// The account's contacts, as one of our devices has them now.
    fn set_contacts(&self, contacts: &[Vec<u8>]) -> Result<()> {
        let mut keep = Vec::new();
        for bytes in contacts {
            let contact: Contact = postcard::from_bytes(bytes)?;
            contact.devices.verify(&IdentityKey::decode(&contact.identity)?, None).map_err(|e| anyhow::anyhow!("{e:?}"))?;
            keep.push((hex(&contact.identity), bytes));
        }
        self.store.atomically(|| {
            for (key, _) in self.store.list("contact")? {
                self.store.remove("contact", &key)?;
            }
            for (key, bytes) in &keep {
                self.store.put("contact", key, bytes)?;
            }
            Ok(())
        })?;
        if let Some(service) = self.service.borrow().as_ref() {
            service.authorize(&self.authorized()?)?;
        }
        Ok(())
    }
}

/// Shows a pairing link and waits until one of the account's devices used
/// it, as a new device does (daemon.rs), but takes no identity and no history.
async fn join<T: Transport>(store: &Store, net: &T, keys: &Keys, signing: &PrivateKey, show: impl FnOnce(&str)) -> Result<Joined> {
    let mut rng = OsRng.unwrap_err();
    let (seed, throwaway) = (Secret::new(rng.random()), rng.random::<[u8; 32]>());
    let pairing = Pairing::new(onion(&seed), throwaway, now_secs());
    let (_service, mut incoming) = net.serve(&seed, &[discovery_key(&Secret::new(throwaway))]).await?;
    show(&pairing.link());
    let expiry = sleep(std::time::Duration::from_secs(PAIRING_LIFETIME));
    tokio::pin!(expiry);
    loop {
        tokio::select! {
            Some(mut stream) = incoming.next() => match join_on(&mut stream, pairing.token, store, keys, signing).await {
                Ok(joined) => return Ok(joined),
                Err(e) => tracing::info!("joining an account: {e:#}"),
            },
            _ = &mut expiry => bail!("the link expired: start the mailbox again for a new one"),
        }
    }
}

async fn join_on<S>(stream: &mut S, token: [u8; 16], store: &Store, keys: &Keys, signing: &PrivateKey) -> Result<Joined>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let Step::Hello { version, token: theirs } = step(stream).await? else { bail!("expected a hello") };
    if version != PROTOCOL_VERSION || theirs != token {
        let why = if version != PROTOCOL_VERSION { OtherVersion(version).to_string() } else { "not this link's token".into() };
        proto::send(stream, &Step::Refused(why.clone())).await?;
        bail!(why);
    }
    let key = signing.public_key()?.serialize().into_vec();
    let ours = onion(&Secret::new(keys.seed));
    proto::send(stream, &Step::Offer { key: key.clone(), onion: ours, name: devices::this_name(), mailbox: true }).await?;
    let Step::Account(account) = step(stream).await? else { bail!("expected the account") };
    let Step::Done = step(stream).await? else { bail!("a mailbox takes no history") };

    let identity = IdentityKey::decode(&account.identity).context("a mailbox takes the public identity alone")?;
    let list = account.devices.verify(&identity, None).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let listed = list.device(account.device).context("not in the account's list")?;
    ensure!(listed.key == key && listed.onion == ours && listed.role == Role::Mailbox, "the list names another device");
    let mut contacts = Vec::new();
    for (kind, _, value) in &account.kv {
        match kind.as_str() {
            "contact" => {
                let contact: Contact = postcard::from_bytes(value)?;
                contact.devices.verify(&IdentityKey::decode(&contact.identity)?, None).map_err(|e| anyhow::anyhow!("{e:?}"))?;
                contacts.push((hex(&contact.identity), value));
            }
            "device_name" => {}
            _ => bail!("not for a mailbox: {kind}"),
        }
    }
    let joined = Joined { account: account.identity, discovery: account.discovery, device: account.device, devices: account.devices };
    let bytes = postcard::to_stdvec(&joined)?;
    store.atomically(|| {
        for (key, value) in &contacts {
            store.put("contact", key, value)?;
        }
        store.put("mailbox", "joined", &bytes)
    })?;
    proto::send(stream, &Step::Ok).await?;
    hang_up(stream).await;
    Ok(joined)
}
