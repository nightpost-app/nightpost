//! The daemon owns the database and the transport. It keeps the onion
//! service up for exactly the keys that may discover it, delivers the
//! outbox, and tells the interfaces what happens.
//!
//! Everything runs on one thread (a tokio `LocalSet`): the database has a
//! single writer by construction, and libsignal's futures, which are not
//! `Send`, can be awaited anywhere.
//!
//! Each received message is handled on its own: one that does not decrypt
//! or parse is logged, acknowledged and dropped, and the link goes on.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail, ensure};
use futures::StreamExt as _;
use libsignal_protocol::{
    CiphertextMessage, CiphertextMessageType, DeviceId, IdentityKey, IdentityKeyPair, KeyPair, PreKeyBundle, PreKeySignalMessage,
    PrivateKey, ProtocolAddress, PublicKey, SignalMessage,
};
use rand::{Rng as _, TryRngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::{Mutex, Notify, broadcast, mpsc, oneshot, watch};
use tokio::task::spawn_local;
use tokio::time::{sleep, timeout};
use zeroize::Zeroizing;

use crate::api::{ContactInfo, DeviceInfo, Event, HistoryItem, LinkState, MAX_PHOTO, Network, Person, hex};
use crate::dbkey::Secret;
use crate::devices::{self, Account, PAIRING_LIFETIME, Pairing, PairingError, Step};
use crate::identity::{Device, DeviceList, MAX_DEVICES, Role, SignedDeviceList, safety_number};
use crate::invite::{self, Code, CodeError, Invite, InviteError};
use crate::mailbox;
use crate::proto::{self, Body, Caller, Challenge, Envelope, Frame, Hello, MAX_READ, MAX_TEXT, Me, Mine, Peer, Proof, Service, TIMERS};
use crate::session::{self, WireBundle};
use crate::store::{NOTE_ACCOUNT, Store};
use crate::{PROTOCOL_VERSION, t};
use crate::transport::{DiscoveryKey, Onion, Service as _, Transport, discovery_key, onion};

/// The id of an account's first device; the others are given theirs when added.
const DEVICE: u8 = 1;
/// A handshake step that takes longer than this is abandoned.
pub(crate) const STEP: Duration = Duration::from_secs(60);
/// A link with nothing to say for this long is closed.
pub(crate) const IDLE: Duration = Duration::from_secs(60);
/// How long accepting an invitation keeps trying: a new descriptor can take
/// minutes to reach the directories.
const ACCEPT_DEADLINE: Duration = if cfg!(test) { Duration::from_secs(1) } else { Duration::from_secs(300) };
const MAX_NAME: usize = 32;
/// An oral code, and the wait for its confirmation.
const CODE_LIFETIME: Duration = Duration::from_secs(600);
/// First wait after a failed dial; it doubles up to 10 minutes.
const RETRY: Duration = if cfg!(test) { Duration::from_millis(50) } else { Duration::from_secs(10) };
/// How often we look in our mailbox, besides at start and when Tor is back.
const FETCH: Duration = if cfg!(test) { Duration::from_millis(200) } else { Duration::from_secs(600) };

pub struct Daemon<T: Transport> {
    store: Rc<Store>,
    net: T,
    /// The account's: replaced when this device joins another account.
    identity: RefCell<IdentityKeyPair>,
    own: RefCell<Own>,
    /// This device's id in `own.devices`.
    device: Cell<u8>,
    /// Taken off our device list by another of our devices: sends nothing.
    removed: Cell<bool>,
    /// `own.key`, parsed.
    signing: PrivateKey,
    events: broadcast::Sender<Event>,
    service: RefCell<Option<T::Service>>,
    /// Held while the service is started, stopped or re-authorized.
    refreshing: Mutex<()>,
    /// Devices a link is open (or being opened) to, with a way to wake it
    /// when something new is queued for them.
    links: RefCell<HashMap<Target, Option<Wake>>>,
    /// Failed attempts in a row, and when to try again.
    backoff: RefCell<HashMap<Target, (u32, Instant)>>,
    wake: Notify,
    stopping: watch::Sender<bool>,
    /// Oral codes used by someone, waiting for us to compare numbers.
    confirming: RefCell<HashMap<String, oneshot::Sender<bool>>>,
    network: RefCell<Network>,
    /// By contact name, for interfaces to show who is reachable: the best
    /// of `device_links` for their devices.
    link_states: RefCell<HashMap<String, LinkState>>,
    device_links: RefCell<HashMap<Target, LinkState>>,
    /// Devices to open a link to even with nothing queued: a link carries
    /// our device list, which changed.
    announce: RefCell<HashSet<Target>>,
    /// Accounts whose mailbox we are at now.
    visiting: RefCell<HashSet<Vec<u8>>>,
    /// To look in our mailbox now rather than at the next round.
    fetch_now: Notify,
}

/// What this device presents in a handshake, at one moment.
struct Myself {
    account: IdentityKey,
    device: u8,
    devices: SignedDeviceList,
}

type Target = (Vec<u8>, u8);
type Wake = mpsc::UnboundedSender<Poke>;

/// What wakes an open link.
enum Poke {
    /// Something was queued.
    Queued,
    /// An envelope to send at once, never queued (a typing indicator).
    Now(Vec<u8>),
    /// Our device list changed: the handshake that carried it is past.
    Devices,
}

/// This device's own keys, created with the database.
#[derive(Serialize, Deserialize)]
struct Own {
    /// Signs link handshakes; listed in the device list.
    key: Vec<u8>,
    /// The onion service's key.
    seed: [u8; 32],
    /// The account's discovery key: contacts authorize its public half.
    discovery: [u8; 32],
    devices: SignedDeviceList,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Contact {
    pub(crate) name: String,
    pub(crate) identity: Vec<u8>,
    pub(crate) devices: SignedDeviceList,
    pub(crate) discovery: DiscoveryKey,
}

/// An invitation not used yet.
#[derive(Serialize, Deserialize)]
struct Pending {
    name: String,
    throwaway: [u8; 32],
    expires: u64,
}

impl<T: Transport + 'static> Daemon<T> {
    pub fn open(store: impl Into<Rc<Store>>, net: T) -> Result<Rc<Self>> {
        let store = store.into();
        let identity = store.identity()?;
        let device = store.get("self", "device")?.and_then(|v| v.first().copied()).unwrap_or(DEVICE);
        let own = match store.get("self", "own")? {
            Some(bytes) => postcard::from_bytes(&bytes)?,
            None => {
                let mut rng = OsRng.unwrap_err();
                let key = KeyPair::generate(&mut rng);
                let seed: [u8; 32] = rng.random();
                let list = DeviceList::first(vec![Device {
                    id: DEVICE,
                    onion: onion(&Secret::new(seed)),
                    key: key.public_key.serialize().into_vec(),
                    role: Role::Normal,
                }]);
                let own = Own {
                    key: key.private_key.serialize().to_vec(),
                    seed,
                    discovery: rng.random(),
                    devices: SignedDeviceList::sign(&list, &identity).map_err(|e| anyhow::anyhow!("{e:?}"))?,
                };
                store.put("self", "own", &postcard::to_stdvec(&own)?)?;
                own
            }
        };
        let signing = PrivateKey::deserialize(&own.key)?;
        let listed = own.devices.verify(identity.identity_key(), None).map_err(|e| anyhow::anyhow!("{e:?}"))?.device(device).is_some();
        Ok(Rc::new(Daemon {
            store,
            net,
            identity: RefCell::new(identity),
            own: RefCell::new(own),
            device: Cell::new(device),
            removed: Cell::new(!listed),
            signing,
            events: broadcast::channel(256).0,
            service: RefCell::default(),
            refreshing: Mutex::default(),
            links: RefCell::default(),
            backoff: RefCell::default(),
            wake: Notify::new(),
            stopping: watch::channel(false).0,
            confirming: RefCell::default(),
            network: RefCell::default(),
            link_states: RefCell::default(),
            device_links: RefCell::default(),
            announce: RefCell::default(),
            visiting: RefCell::default(),
            fetch_now: Notify::new(),
        }))
    }

    /// Brings the service up for the current contacts and starts delivering.
    /// Must run inside a `LocalSet`.
    pub async fn start(self: &Rc<Self>) -> Result<()> {
        self.present_to(&self.own_contact())?;
        for contact in self.contacts_raw()? {
            self.present_to(&contact)?;
        }
        self.refresh_service().await?;
        spawn_local(self.clone().deliver());
        spawn_local(self.clone().fetcher());
        spawn_local(self.clone().sweeper());
        Ok(())
    }

    /// Deletes what has disappeared, every half minute while the daemon runs.
    async fn sweeper(self: Rc<Self>) {
        loop {
            if let Err(e) = self.sweep(now_millis()) {
                tracing::warn!("deleting expired messages: {e:#}");
            }
            // Gone with the daemon: `stop` waits for nothing to hold it.
            tokio::select! {
                _ = sleep(Duration::from_secs(30)) => {}
                _ = self.stopped() => return,
            }
        }
    }

    /// Deletes the messages that ended by `now`, and tells which conversations changed.
    pub fn sweep(&self, now: u64) -> Result<()> {
        let accounts = self.store.expire(now)?;
        for contact in self.contacts_raw()?.into_iter().filter(|c| accounts.contains(&c.identity)) {
            let _ = self.events.send(Event::Expired { name: contact.name });
        }
        Ok(())
    }

    /// How long messages with `contact` last, in seconds; 0 for good.
    fn timer(&self, contact: &Contact) -> Result<u32> {
        Ok(self.store.get("timer", &hex(&contact.identity))?.and_then(|v| v.try_into().ok()).map_or(0, u32::from_le_bytes))
    }

    fn keep_timer(&self, contact: &Contact, seconds: u32) -> Result<()> {
        ensure!(TIMERS.contains(&seconds), "not a timer a conversation can have");
        self.store.put("timer", &hex(&contact.identity), &seconds.to_le_bytes())?;
        Ok(())
    }

    /// Messages with `name` disappear after `seconds` (0: they stay), for
    /// them too: they are told, like any message.
    pub fn set_timer(&self, name: &str, seconds: u32) -> Result<()> {
        ensure!(!self.is_note(name)?, t!("err-note-no-timer"));
        let contact = self.contact_named(name)?;
        let devices = self.devices_of(&contact)?;
        let id: [u8; 16] = OsRng.unwrap_err().random();
        let envelope = postcard::to_stdvec(&Envelope { id, time: now_millis(), body: Body::Timer(seconds) })?;
        self.store.atomically(|| {
            self.keep_timer(&contact, seconds).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
            self.queue(&contact, &devices, &id, &envelope)?;
            self.tell_mine(Mine::Timer { account: contact.identity.clone(), seconds })
        })?;
        let _ = self.events.send(Event::TimerChanged { name: name.into(), seconds, by_me: true });
        self.nudge(&contact, &devices);
        self.nudge_mine();
        Ok(())
    }

    /// Takes the service down, closes every link, and returns once nothing
    /// else holds the daemon: dropping it then closes the database.
    pub async fn stop(self: &Rc<Self>) {
        self.shut_down();
        self.service.borrow_mut().take();
        while Rc::strong_count(self) > 1 {
            sleep(Duration::from_millis(10)).await;
        }
    }

    /// Asks everything to stop; [`Daemon::stopped`] then returns.
    pub fn shut_down(&self) {
        self.stopping.send_replace(true);
    }

    pub async fn stopped(&self) {
        let _ = self.stopping.subscribe().wait_for(|stop| *stop).await;
    }

    /// How many are following the events: interfaces, `nightpost listen`.
    pub fn listeners(&self) -> usize {
        self.events.receiver_count()
    }

    /// Passes `event` on to every interface listening.
    pub fn announce(&self, event: Event) {
        let _ = self.events.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub fn network(&self) -> Network {
        self.network.borrow().clone()
    }

    /// What the transport reports; told to interfaces when it changes.
    pub fn set_network(&self, network: Network) {
        if *self.network.borrow() != network {
            let before = std::mem::replace(&mut *self.network.borrow_mut(), network.clone());
            // Tor is back (the network came back, or it finished connecting):
            // the waits that grew while it was away are no use now.
            if (before.blocked() && !network.blocked()) || (before.percent < 100 && network.percent == 100) {
                self.backoff.borrow_mut().clear();
                self.wake.notify_one();
                self.fetch_now.notify_one();
            }
            let _ = self.events.send(Event::Network { network });
        }
    }

    pub fn links(&self) -> Vec<(String, LinkState)> {
        self.link_states.borrow().iter().map(|(name, state)| (name.clone(), *state)).collect()
    }

    /// How the link to one device stands; a contact shows the best of theirs.
    fn set_link(&self, target: &Target, state: LinkState) {
        let Ok(Some(contact)) = self.contact(&target.0) else { return };
        if self.is_own(&contact) {
            return;
        }
        self.device_links.borrow_mut().insert(target.clone(), state);
        let rank = |s: &LinkState| match s {
            LinkState::Open => 5,
            LinkState::Trying => 4,
            LinkState::Idle => 3,
            LinkState::Older | LinkState::Newer => 2,
            LinkState::Unreachable => 1,
        };
        let best = self.device_links.borrow().iter().filter(|((account, _), _)| *account == target.0).map(|(_, s)| *s).max_by_key(rank);
        let best = best.unwrap_or(state);
        if self.link_states.borrow_mut().insert(contact.name.clone(), best) != Some(best) {
            let _ = self.events.send(Event::Link { name: contact.name, state: best });
        }
    }

    /// A one-time link for someone who will be known here as `name`.
    pub async fn invite(self: &Rc<Self>, name: &str) -> Result<String> {
        self.check_name(name)?;
        let throwaway: [u8; 32] = OsRng.unwrap_err().random();
        let (invite, link) =
            Invite::create(&self.identity.borrow(), onion(&self.seed()), throwaway, now_secs());
        let pending = Pending { name: name.into(), throwaway, expires: invite.expires };
        self.store.put("invite", &hex(&invite.token), &postcard::to_stdvec(&pending)?)?;
        self.refresh_service().await?;
        Ok(link)
    }

    /// Uses `link` to become the inviter's contact, known here as `name`.
    pub async fn accept(self: &Rc<Self>, link: &str, name: &str) -> Result<()> {
        self.check_name(name)?;
        let invite = Invite::parse(link, now_secs()).map_err(|e| {
            anyhow::anyhow!(match e {
                InviteError::NotAnInvite => t!("err-invite-not"),
                InviteError::Version => t!("err-invite-version"),
                InviteError::Malformed => t!("err-invite-malformed"),
                InviteError::BadSignature => t!("err-invite-signature"),
                InviteError::Expired => t!("err-invite-expired"),
            })
        })?;
        ensure!(self.contact(&invite.identity.serialize())?.is_none(), t!("err-already-contact"));
        self.net.present(invite.onion, &Secret::new(invite.discovery))?;

        let (mut stream, _) = self.dial_until(&[invite.onion]).await?;
        let myself = self.myself();
        let me = self.me(&myself);
        let caller = Caller::new(&me, invite.onion, Some(invite.token));
        proto::send(&mut stream, caller.hello()).await?;
        let challenge: Challenge = challenge(&mut stream).await?;
        let (proof, peer) = caller.on_challenge(&me, &challenge, &invite.identity, None).map_err(handshake)?;
        proto::send(&mut stream, &proof).await?;
        let (contact, bundle) = self.exchange_cards(&mut stream, &peer, &challenge.devices, name).await?;
        self.add_contact(&contact, &bundle, peer.device).await?;
        spawn_local(self.clone().link(stream, contact, peer.device));
        Ok(())
    }

    /// A 12-digit code for someone who will be known here as `name`. Whoever
    /// uses it is kept only once [`Daemon::confirm`]ed.
    pub async fn invite_code(self: &Rc<Self>, name: &str) -> Result<String> {
        self.check_name(name)?;
        ensure!(!self.confirming.borrow().contains_key(name), t!("err-code-waiting", name = name));
        let code = Code::generate();
        let window = invite::window(now_secs());
        let derive = code.clone();
        let keys = tokio::task::spawn_blocking(move || derive.derive(window)).await?;
        let (service, incoming) = self.net.serve(&keys.onion, &[discovery_key(&keys.discovery)]).await?;
        let (daemon, name, temp) = (self.clone(), name.to_string(), onion(&keys.onion));
        spawn_local(async move {
            let _service = service;
            let mut incoming = incoming;
            let expiry = sleep(CODE_LIFETIME);
            tokio::pin!(expiry);
            loop {
                tokio::select! {
                    Some(stream) = incoming.next() => match daemon.answer_code(stream, temp, &name).await {
                        Ok(()) => break,
                        Err(e) => tracing::info!("code invitation: {e:#}"),
                    },
                    _ = &mut expiry => break,
                    _ = daemon.stopped() => break,
                }
            }
        });
        Ok(code.to_string())
    }

    async fn answer_code(self: &Rc<Self>, mut stream: T::Stream, temp: Onion, name: &str) -> Result<()> {
        let myself = self.myself();
        let me = self.me(&myself);
        let hello = hello(&mut stream).await?;
        let (service, challenge) = Service::on_hello(&me, temp, hello).map_err(handshake)?;
        proto::send(&mut stream, &challenge).await?;
        let proof: Proof = timeout(STEP, proto::receive(&mut stream)).await??;
        let peer = service.on_proof(&proof, None).map_err(handshake)?;
        // Only once they proved who they are, and told rather than dropped.
        if self.contact(&peer.account.serialize())?.is_some() {
            proto::send(&mut stream, &Frame::Refused).await?;
            bail!(t!("err-already-contact"));
        }
        let (contact, bundle) = self.exchange_cards(&mut stream, &peer, &service.hello().devices, name).await?;

        let (keep, kept) = oneshot::channel();
        self.confirming.borrow_mut().insert(name.to_string(), keep);
        let _ = self.events.send(Event::Fingerprint { name: name.into(), safety_number: self.safety_number(&contact)? });
        let kept = timeout(CODE_LIFETIME, kept).await;
        self.confirming.borrow_mut().remove(name);
        ensure!(matches!(kept, Ok(Ok(true))), t!("err-not-confirmed"));

        self.add_contact(&contact, &bundle, peer.device).await?;
        proto::send(&mut stream, &Frame::Welcome).await?;
        spawn_local(self.clone().link(stream, contact, peer.device));
        Ok(())
    }

    /// Keeps, or turns down, whoever used the code given to `name`.
    pub fn confirm(&self, name: &str, keep: bool) -> Result<()> {
        let waiting = self.confirming.borrow_mut().remove(name).with_context(|| t!("err-nobody-waiting"))?;
        let _ = waiting.send(keep);
        Ok(())
    }

    /// Uses a code read out by the inviter, known here as `name`. Returns
    /// once the inviter has confirmed.
    pub async fn accept_code(self: &Rc<Self>, code: &str, name: &str) -> Result<()> {
        self.check_name(name)?;
        let code = Code::parse(code).map_err(|e| match e {
            CodeError::Length => anyhow::anyhow!(t!("err-code-length")),
            CodeError::Typo => anyhow::anyhow!(t!("err-code-typo")),
        })?;
        let windows = invite::windows(now_secs());
        let keys = tokio::task::spawn_blocking(move || windows.map(|w| code.derive(w))).await?;
        let onions = keys.each_ref().map(|k| onion(&k.onion));
        for (k, o) in keys.iter().zip(onions) {
            self.net.present(o, &k.discovery)?;
        }
        let (mut stream, temp) = self.dial_until(&onions).await?;

        let myself = self.myself();
        let me = self.me(&myself);
        let caller = Caller::new(&me, temp, None);
        proto::send(&mut stream, caller.hello()).await?;
        let challenge: Challenge = challenge(&mut stream).await?;
        let (proof, peer) = caller.on_challenge_by_code(&me, &challenge).map_err(handshake)?;
        ensure!(self.contact(&peer.account.serialize())?.is_none(), t!("err-already-contact"));
        proto::send(&mut stream, &proof).await?;
        let (contact, bundle) = self.exchange_cards(&mut stream, &peer, &challenge.devices, name).await?;
        let _ = self.events.send(Event::Fingerprint { name: name.into(), safety_number: self.safety_number(&contact)? });
        match timeout(CODE_LIFETIME, proto::receive::<Frame>(&mut stream)).await {
            Ok(Ok(Frame::Welcome)) => {}
            _ => bail!(t!("err-did-not-confirm", name = name)),
        }
        self.add_contact(&contact, &bundle, peer.device).await?;
        spawn_local(self.clone().link(stream, contact, peer.device));
        Ok(())
    }

    /// Dials the first of `onions` that answers, retrying until the deadline:
    /// a new descriptor can take minutes to reach the directories.
    async fn dial_until(&self, onions: &[Onion]) -> Result<(T::Stream, Onion)> {
        let started = Instant::now();
        loop {
            for &to in onions {
                match timeout(STEP * 2, self.net.dial(to)).await {
                    Ok(Ok(stream)) => return Ok((stream, to)),
                    Ok(Err(e)) => tracing::info!("not reachable yet: {e}"),
                    Err(_) => tracing::info!("not reachable yet: timeout"),
                }
            }
            ensure!(started.elapsed() < ACCEPT_DEADLINE, t!("err-not-found"));
            sleep(RETRY).await;
        }
    }

    /// Queues `text` for every device of `to`; returns the message id.
    pub fn send(&self, to: &str, text: &str) -> Result<[u8; 16]> {
        ensure!(!text.is_empty() && text.len() <= MAX_TEXT, "a message is 1 to {MAX_TEXT} bytes");
        ensure!(!self.removed.get(), t!("err-device-removed"));
        if self.is_note(to)? {
            let id: [u8; 16] = OsRng.unwrap_err().random();
            let time = now_millis();
            self.store.atomically(|| {
                self.store.add_note(&id, time, text)?;
                self.tell_mine(Mine::Note { id, time, text: text.into() })
            })?;
            self.nudge_mine();
            let _ = self.events.send(Event::Sent { to: to.into(), id: hex(&id), time, text: text.into() });
            return Ok(id);
        }
        let contact = self.contacts_raw()?.into_iter().find(|c| c.name == to).with_context(|| t!("err-no-contact"))?;
        let devices = self.devices_of(&contact)?;
        let id: [u8; 16] = OsRng.unwrap_err().random();
        let time = now_millis();
        let envelope = postcard::to_stdvec(&Envelope { id, time, body: Body::Text(text.into()) })?;
        let ttl = self.timer(&contact)?;
        self.store.atomically(|| {
            // Ours end a timer after sending.
            self.store.add_timed(&contact.identity, &id, true, time, text, ttl, (ttl > 0).then(|| time + u64::from(ttl) * 1000))?;
            self.queue(&contact, &devices, &id, &envelope)?;
            self.tell_mine(Mine::Sent { to: contact.identity.clone(), id, time, text: text.into(), ttl })
        })?;
        self.nudge_mine();
        let _ = self.events.send(Event::Sent { to: to.into(), id: hex(&id), time, text: text.into() });
        self.nudge(&contact, &devices);
        Ok(id)
    }

    /// Puts an envelope in the outbox of each of their normal devices.
    fn queue(&self, contact: &Contact, devices: &DeviceList, id: &[u8; 16], envelope: &[u8]) -> rusqlite::Result<()> {
        for device in devices.devices.iter().filter(|d| d.role == Role::Normal) {
            self.store.queue(&contact.identity, device.id, id, envelope)?;
        }
        Ok(())
    }

    /// Queues `what` for our other normal devices (none: nothing to do).
    /// Inside the caller's transaction, so the copy goes with the change.
    fn tell_mine(&self, what: Mine) -> rusqlite::Result<()> {
        let own = self.own_contact();
        let Ok(devices) = self.devices_of(&own) else { return Ok(()) };
        let others: Vec<u8> = devices.devices.iter().filter(|d| d.role == Role::Normal && d.id != self.device.get()).map(|d| d.id).collect();
        if others.is_empty() {
            return Ok(());
        }
        let id: [u8; 16] = OsRng.unwrap_err().random();
        let envelope = postcard::to_stdvec(&Envelope { id, time: now_millis(), body: Body::Mine(what) })
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
        for device in others {
            self.store.queue(&own.identity, device, &id, &envelope)?;
        }
        Ok(())
    }

    fn nudge_mine(&self) {
        let own = self.own_contact();
        if let Ok(devices) = self.devices_of(&own) {
            self.nudge(&own, &devices);
        }
    }

    /// Something was queued: open links send it now, the rest get dialed.
    fn nudge(&self, contact: &Contact, devices: &DeviceList) {
        for device in &devices.devices {
            if let Some(Some(link)) = self.links.borrow().get(&(contact.identity.clone(), device.id)) {
                let _ = link.send(Poke::Queued);
            }
        }
        self.wake.notify_one();
    }

    /// Whether we send read receipts, and so take theirs. Off by default:
    /// they tell when we are at the screen, which is ours to give.
    pub fn read_receipts(&self) -> Result<bool> {
        Ok(self.store.get("self", "read_receipts")?.is_some_and(|v| v == [1]))
    }

    pub fn set_read_receipts(&self, on: bool) -> Result<()> {
        self.store.put("self", "read_receipts", &[u8::from(on)])?;
        let _ = self.events.send(Event::ReadReceipts { on });
        Ok(())
    }

    /// Whether we tell people we are writing to them, and so see when they
    /// write to us. Off by default, as read receipts.
    pub fn typing_indicators(&self) -> Result<bool> {
        Ok(self.store.get("self", "typing_indicators")?.is_some_and(|v| v == [1]))
    }

    pub fn set_typing_indicators(&self, on: bool) -> Result<()> {
        self.store.put("self", "typing_indicators", &[u8::from(on)])?;
        let _ = self.events.send(Event::TypingIndicators { on });
        Ok(())
    }

    /// We are writing to `name`: with typing indicators on, they are told
    /// over each open link. A device with none gets dialed instead, so the
    /// next one (the interfaces send one every few seconds) goes, and the
    /// message after it too, sooner.
    pub fn typing(self: &Rc<Self>, name: &str) -> Result<()> {
        if !self.typing_indicators()? {
            return Ok(());
        }
        let contact = self.contact_named(name)?;
        let id: [u8; 16] = OsRng.unwrap_err().random();
        let envelope = postcard::to_stdvec(&Envelope { id, time: now_millis(), body: Body::Typing })?;
        for device in self.devices_of(&contact)?.devices.iter().filter(|d| d.role == Role::Normal) {
            let target = (contact.identity.clone(), device.id);
            let link = self.links.borrow().get(&target).cloned();
            match link {
                Some(Some(link)) => {
                    let _ = link.send(Poke::Now(envelope.clone()));
                }
                // Being dialed already.
                Some(None) => {}
                None => {
                    if self.backoff.borrow().get(&target).is_none_or(|(_, at)| *at <= Instant::now()) {
                        self.links.borrow_mut().insert(target.clone(), None);
                        spawn_local(self.clone().dial(target));
                    }
                }
            }
        }
        Ok(())
    }

    /// We have seen what `name` wrote so far; with read receipts on, they
    /// are told which messages.
    pub fn mark_read(&self, name: &str) -> Result<()> {
        if self.is_note(name)? {
            return Ok(());
        }
        let contact = self.contact_named(name)?;
        let devices = self.devices_of(&contact)?;
        let receipts = self.read_receipts()?;
        let queued = self.store.atomically(|| {
            let ids = self.store.read_incoming(&contact.identity, now_millis())?;
            if !ids.is_empty() {
                self.tell_mine(Mine::Seen(contact.identity.clone()))?;
            }
            if !receipts {
                return Ok(false);
            }
            for chunk in ids.chunks(MAX_READ) {
                let id: [u8; 16] = OsRng.unwrap_err().random();
                let envelope = Envelope { id, time: now_millis(), body: Body::Read(chunk.to_vec()) };
                let envelope = postcard::to_stdvec(&envelope).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
                self.queue(&contact, &devices, &id, &envelope)?;
            }
            Ok(!ids.is_empty())
        })?;
        if queued {
            self.nudge(&contact, &devices);
        }
        self.nudge_mine();
        Ok(())
    }

    /// The last `limit` messages with `name`, oldest first.
    pub fn history(&self, name: &str, limit: u32) -> Result<Vec<HistoryItem>> {
        let account = if self.is_note(name)? { NOTE_ACCOUNT.to_vec() } else { self.contact_named(name)?.identity };
        Ok(self
            .store
            .history(&account, limit)?
            .into_iter()
            .map(|(id, outgoing, time, text, delivered, read)| HistoryItem { id: hex(&id), outgoing, time, text, delivered, read: outgoing && read })
            .collect())
    }

    /// The contacts, after the note to ourselves: always there, always first.
    pub fn contacts(&self) -> Result<Vec<ContactInfo>> {
        let note = ContactInfo { name: self.note_name()?, safety_number: String::new(), person: Person::default(), timer: 0, note: true };
        let mut all = vec![note];
        for c in self.contacts_raw()? {
            all.push(ContactInfo { safety_number: self.safety_number(&c)?, person: self.person(&c)?, timer: self.timer(&c)?, name: c.name, note: false });
        }
        Ok(all)
    }

    /// What the note to ourselves is called: the language of the first start,
    /// kept so that it does not change with it, and renamed like a contact.
    fn note_name(&self) -> Result<String> {
        if let Some(name) = self.store.get("self", "note_name")? {
            return Ok(String::from_utf8(name)?);
        }
        let name = t!("note-name");
        self.store.put("self", "note_name", name.as_bytes())?;
        Ok(name)
    }

    fn is_note(&self, name: &str) -> Result<bool> {
        Ok(self.note_name()? == name)
    }

    pub fn rename(&self, name: &str, to: &str) -> Result<()> {
        self.check_name(to)?;
        if self.is_note(name)? {
            self.store.put("self", "note_name", to.as_bytes())?;
            let _ = self.events.send(Event::ContactRenamed { name: name.into(), to: to.into() });
            return Ok(());
        }
        let mut contact = self.contact_named(name)?;
        contact.name = to.into();
        let bytes = postcard::to_stdvec(&contact)?;
        self.store.atomically(|| {
            self.store.put("contact", &hex(&contact.identity), &bytes)?;
            self.tell_mine(Mine::Contact(bytes.clone()))
        })?;
        self.nudge_mine();
        self.rename_link_state(name, to);
        let _ = self.events.send(Event::ContactRenamed { name: name.into(), to: to.into() });
        Ok(())
    }

    /// Takes their discovery key off the service, so they can no longer
    /// reach us or tell we are online, and stops sending to them. An open
    /// link closes at its next turn ([`Self::run_link`] looks them up).
    pub async fn remove(self: &Rc<Self>, name: &str, wipe: bool) -> Result<()> {
        ensure!(!self.is_note(name)?, t!("err-note-fixed"));
        let contact = self.contact_named(name)?;
        let key = hex(&contact.identity);
        self.store.atomically(|| {
            self.store.remove("contact", &key)?;
            self.store.remove("person", &key)?;
            self.store.forget(&contact.identity, wipe)?;
            self.tell_mine(Mine::Forget { account: contact.identity.clone(), wipe })
        })?;
        self.nudge_mine();
        self.link_states.borrow_mut().remove(name);
        self.device_links.borrow_mut().retain(|(account, _), _| *account != contact.identity);
        self.backoff.borrow_mut().retain(|(account, _), _| *account != contact.identity);
        self.refresh_service().await?;
        let _ = self.events.send(Event::ContactRemoved { name: name.into() });
        Ok(())
    }

    pub fn set_person(&self, name: &str, person: Person) -> Result<()> {
        if let Some(photo) = &person.photo {
            ensure!(photo.len() <= MAX_PHOTO && photo.starts_with("data:image/png;base64,"), "a photo is a PNG data URI of at most {MAX_PHOTO} bytes");
        }
        let contact = self.contact_named(name)?;
        self.store.put("person", &hex(&contact.identity), &postcard::to_stdvec(&person)?)?;
        let _ = self.events.send(Event::PersonChanged { name: name.into(), person });
        Ok(())
    }

    pub fn recovery_phrase(&self) -> Option<zeroize::Zeroizing<String>> {
        self.store.recovery_phrase()
    }

    pub fn set_passphrase(&self, current: Option<Zeroizing<Vec<u8>>>, new: Option<Zeroizing<Vec<u8>>>) -> Result<()> {
        self.store.set_passphrase(current, new)
    }

    fn person(&self, contact: &Contact) -> Result<Person> {
        Ok(match self.store.get("person", &hex(&contact.identity))? {
            Some(bytes) => postcard::from_bytes(&bytes)?,
            None => Person::default(),
        })
    }

    // --- our devices -------------------------------------------------------

    /// Our devices, this one included.
    pub fn devices(&self) -> Result<Vec<DeviceInfo>> {
        let this = self.device.get();
        self.devices_of(&self.own_contact())?
            .devices
            .iter()
            .map(|d| {
                let name = match self.store.get("device_name", &d.id.to_string())? {
                    Some(name) => String::from_utf8(name)?,
                    None if d.id == this => devices::this_name(),
                    None => t!("device-unnamed", id = d.id),
                };
                Ok(DeviceInfo { id: d.id, name, mailbox: d.role == Role::Mailbox, this: d.id == this })
            })
            .collect()
    }

    /// A link for a device already in an account to use, within 10 minutes:
    /// this device then joins that account. Only from an empty start, since
    /// nothing kept here would survive it.
    pub async fn pairing_link(self: &Rc<Self>) -> Result<String> {
        ensure!(self.contacts_raw()?.is_empty() && !self.store.has_messages()?, t!("err-pair-not-empty"));
        let mut rng = OsRng.unwrap_err();
        let (seed, throwaway) = (Secret::new(rng.random()), rng.random::<[u8; 32]>());
        let pairing = Pairing::new(onion(&seed), throwaway, now_secs());
        let (service, incoming) = self.net.serve(&seed, &[discovery_key(&Secret::new(throwaway))]).await?;
        let (daemon, token) = (self.clone(), pairing.token);
        spawn_local(async move {
            let _service = service;
            let mut incoming = incoming;
            let expiry = sleep(Duration::from_secs(PAIRING_LIFETIME));
            tokio::pin!(expiry);
            loop {
                tokio::select! {
                    Some(stream) = incoming.next() => match daemon.join(stream, token).await {
                        Ok(()) => break,
                        Err(e) => tracing::info!("joining an account: {e:#}"),
                    },
                    _ = &mut expiry => break,
                    _ = daemon.stopped() => break,
                }
            }
        });
        Ok(pairing.link())
    }

    /// The new device's side: show what we are, take the account.
    async fn join(self: &Rc<Self>, mut stream: T::Stream, token: [u8; 16]) -> Result<()> {
        let Step::Hello { version, token: theirs } = step(&mut stream).await? else { bail!("expected a hello") };
        if version != PROTOCOL_VERSION || theirs != token {
            let why = if version != PROTOCOL_VERSION { OtherVersion(version).to_string() } else { "not this link's token".into() };
            proto::send(&mut stream, &Step::Refused(why.clone())).await?;
            bail!(why);
        }
        let key = self.signing.public_key()?.serialize().into_vec();
        let ours = onion(&self.seed());
        proto::send(&mut stream, &Step::Offer { key: key.clone(), onion: ours, name: devices::this_name(), mailbox: false }).await?;
        let Step::Account(account) = step(&mut stream).await? else { bail!("expected the account") };
        let mut rows = Vec::new();
        loop {
            match step(&mut stream).await? {
                Step::Messages(part) => rows.extend(part),
                Step::Done => break,
                _ => bail!("expected the history"),
            }
        }

        // All checked before anything is kept.
        let identity = IdentityKeyPair::try_from(&account.identity[..])?;
        let list = account.devices.verify(identity.identity_key(), None).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let listed = list.device(account.device).context("not in the account's list")?;
        ensure!(listed.key == key && listed.onion == ours && listed.role == Role::Normal, "the list names another device");
        ensure!(self.contacts_raw()?.is_empty() && !self.store.has_messages()?, t!("err-pair-not-empty"));
        for (kind, key, value) in &account.kv {
            ensure!(devices::may_copy(kind, key), "not ours to take: {kind}");
            if kind == "contact" {
                let contact: Contact = postcard::from_bytes(value)?;
                self.devices_of(&contact)?;
            }
        }
        let own = { let own = self.own.borrow(); Own { key: own.key.clone(), seed: own.seed, discovery: account.discovery, devices: account.devices.clone() } };
        let own_bytes = postcard::to_stdvec(&own)?;
        self.store.atomically(|| {
            self.store.put("self", "identity", &identity.serialize())?;
            self.store.put("self", "own", &own_bytes)?;
            self.store.put("self", "device", &[account.device])?;
            // Signed by the identity we leave: of no use now.
            for (token, _) in self.store.list("invite")? {
                self.store.remove("invite", &token)?;
            }
            for (kind, key, value) in &account.kv {
                self.store.put(kind, key, value)?;
            }
            self.store.add_rows(&rows)
        })?;
        *self.identity.borrow_mut() = identity;
        *self.own.borrow_mut() = own;
        self.device.set(account.device);
        self.removed.set(false);
        proto::send(&mut stream, &Step::Ok).await?;

        self.present_to(&self.own_contact())?;
        for contact in self.contacts_raw()? {
            self.present_to(&contact)?;
        }
        self.refresh_service().await?;
        let _ = self.events.send(Event::Paired);
        hang_up(&mut stream).await;
        Ok(())
    }

    /// Uses the link a new device shows: it joins this account, with our
    /// contacts and history. Returns its name.
    pub async fn add_device(self: &Rc<Self>, link: &str) -> Result<String> {
        ensure!(!self.removed.get(), t!("err-device-removed"));
        let pairing = Pairing::parse(link, now_secs()).map_err(|e| {
            anyhow::anyhow!(match e {
                PairingError::NotALink => t!("err-pair-not-link"),
                PairingError::Malformed => t!("err-invite-malformed"),
                PairingError::Expired => t!("err-invite-expired"),
            })
        })?;
        self.net.present(pairing.onion, &Secret::new(pairing.discovery))?;
        let (mut stream, _) = self.dial_until(&[pairing.onion]).await?;
        proto::send(&mut stream, &Step::Hello { version: PROTOCOL_VERSION, token: pairing.token }).await?;
        let (key, their_onion, name, mailbox) = match step(&mut stream).await? {
            Step::Offer { key, onion, name, mailbox } => (key, onion, name, mailbox),
            Step::Refused(why) => bail!(why),
            _ => bail!("expected what the device is"),
        };
        PublicKey::deserialize(&key)?;
        let name = if self.check_name_shape(&name).is_ok() { name } else { "?".into() };

        let own = self.own_contact();
        let old = self.devices_of(&own)?;
        // Never an id used before: a contact's session for it would reach
        // a device that cannot read it.
        let next = self.store.get("self", "next_device")?.and_then(|v| v.first().copied()).unwrap_or(0);
        let id = next.max(old.devices.iter().map(|d| d.id).max().unwrap_or(0) + 1);
        ensure!(usize::from(id) <= MAX_DEVICES, t!("err-too-many-devices"));
        let mut list = old.devices.clone();
        list.push(Device { id, onion: their_onion, key, role: if mailbox { Role::Mailbox } else { Role::Normal } });
        let signed = SignedDeviceList::sign(&DeviceList::next(&own.devices, &old, list), &self.identity.borrow()).map_err(|e| anyhow::anyhow!("{e:?}"))?;

        // A mailbox gets what it needs to let our contacts in, not the identity.
        let identity = if mailbox { self.account() } else { self.identity.borrow().serialize().into_vec() };
        let mut kv = Vec::new();
        if mailbox {
            kv = self.mailbox_contacts()?.into_iter().map(|bytes| ("contact".to_string(), String::new(), bytes)).collect();
        } else {
            for kind in ["contact", "person", "timer", "device_name"] {
                kv.extend(self.store.list(kind)?.into_iter().map(|(key, value)| (kind.to_string(), key, value)));
            }
        }
        if !mailbox {
            for key in ["note_name", "read_receipts", "typing_indicators"] {
                if let Some(value) = self.store.get("self", key)? {
                    kv.push(("self".into(), key.into(), value));
                }
            }
        }
        let names = [(self.device.get(), self.devices()?.into_iter().find(|d| d.this).map(|d| d.name).unwrap_or_default()), (id, name.clone())];
        kv.extend(names.iter().map(|(id, name)| ("device_name".to_string(), id.to_string(), name.clone().into_bytes())));
        let discovery = if mailbox { discovery_key(&self.discovery()) } else { self.own.borrow().discovery };
        proto::send(&mut stream, &Step::Account(Account { identity, discovery, device: id, devices: signed.clone(), kv })).await?;
        if !mailbox {
            // Parts well under a frame: a message is at most 64 KiB.
            let (mut part, mut size) = (Vec::new(), 0);
            for row in self.store.all_messages()? {
                size += row.text.len() + row.account.len() + 64;
                part.push(row);
                if size > 256 * 1024 {
                    proto::send(&mut stream, &Step::Messages(std::mem::take(&mut part))).await?;
                    size = 0;
                }
            }
            if !part.is_empty() {
                proto::send(&mut stream, &Step::Messages(part)).await?;
            }
        }
        proto::send(&mut stream, &Step::Done).await?;
        match step(&mut stream).await? {
            Step::Ok => {}
            Step::Refused(why) => bail!(why),
            _ => bail!("expected the device to keep the account"),
        }

        // It has it all: now it is one of ours.
        self.store.atomically(|| {
            self.store.put("self", "next_device", &[id + 1])?;
            for (id, name) in &names {
                self.store.put("device_name", &id.to_string(), name.as_bytes())?;
            }
            self.tell_mine(Mine::Names(names.to_vec()))
        })?;
        self.update_devices(&mut self.own_contact(), &signed)?;
        self.announce_all()?;
        Ok(name)
    }

    /// Takes one of our other devices off the account: contacts stop
    /// writing to it as they learn the new list. It keeps what it had.
    pub async fn remove_device(self: &Rc<Self>, id: u8) -> Result<()> {
        ensure!(!self.removed.get(), t!("err-device-removed"));
        ensure!(id != self.device.get(), t!("err-remove-this-device"));
        let own = self.own_contact();
        let old = self.devices_of(&own)?;
        ensure!(old.device(id).is_some(), t!("err-no-device"));
        let list = old.devices.iter().filter(|d| d.id != id).cloned().collect();
        let signed = SignedDeviceList::sign(&DeviceList::next(&own.devices, &old, list), &self.identity.borrow()).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        self.store.remove("device_name", &id.to_string())?;
        self.update_devices(&mut self.own_contact(), &signed)?;
        self.announce_all()?;
        Ok(())
    }

    /// Opens a link to every device of every contact, and to our others:
    /// each handshake carries our new list.
    fn announce_all(&self) -> Result<()> {
        let mut targets = Vec::new();
        for contact in self.contacts_raw()?.into_iter().chain([self.own_contact()]) {
            for device in self.devices_of(&contact)?.devices.into_iter().filter(|d| d.role == Role::Normal) {
                if !(self.is_own(&contact) && device.id == self.device.get()) {
                    targets.push((contact.identity.clone(), device.id));
                }
            }
        }
        for target in targets {
            match self.links.borrow().get(&target) {
                Some(Some(link)) => {
                    let _ = link.send(Poke::Devices);
                }
                _ => {
                    self.announce.borrow_mut().insert(target);
                }
            }
        }
        self.wake.notify_one();
        Ok(())
    }

    // --- the service -------------------------------------------------------

    /// Authorizes exactly the contacts and the pending invitations, starting
    /// or stopping the service as that list becomes non-empty or empty.
    async fn refresh_service(self: &Rc<Self>) -> Result<()> {
        let _guard = self.refreshing.lock().await;
        let mut keys = Vec::new();
        for (token, bytes) in self.store.list("invite")? {
            let pending: Pending = postcard::from_bytes(&bytes)?;
            if pending.expires <= now_secs() {
                self.store.remove("invite", &token)?;
            } else {
                keys.push(discovery_key(&Secret::new(pending.throwaway)));
            }
        }
        keys.extend(self.contacts_raw()?.iter().map(|c| c.discovery));
        // Our other devices hold our own discovery key.
        if self.devices_of(&self.own_contact())?.devices.len() > 1 {
            keys.push(discovery_key(&self.discovery()));
        }

        if keys.is_empty() {
            self.service.borrow_mut().take();
            return Ok(());
        }
        if let Some(service) = self.service.borrow().as_ref() {
            return Ok(service.authorize(&keys)?);
        }
        let (service, incoming) = self.net.serve(&self.seed(), &keys).await?;
        *self.service.borrow_mut() = Some(service);
        let daemon = self.clone();
        spawn_local(async move {
            let mut incoming = incoming;
            loop {
                tokio::select! {
                    Some(stream) = incoming.next() => { spawn_local(daemon.clone().answer(stream)); }
                    _ = daemon.stopped() => break,
                    else => break,
                }
            }
        });
        Ok(())
    }

    async fn answer(self: Rc<Self>, stream: T::Stream) {
        if let Err(e) = self.try_answer(stream).await {
            tracing::info!("incoming link refused: {e:#}");
        }
    }

    async fn try_answer(self: &Rc<Self>, mut stream: T::Stream) -> Result<()> {
        let myself = self.myself();
        let me = self.me(&myself);
        let hello = hello(&mut stream).await?;
        // One of ours, with a newer list: signed by the account, so taken
        // as it comes, even from a device the list no longer names.
        if hello.account == self.account() {
            self.update_devices(&mut self.own_contact(), &hello.devices)?;
        }
        let (service, challenge) = Service::on_hello(&me, onion(&self.seed()), hello).map_err(handshake)?;
        let contact = self.contact(&service.hello().account)?;
        let token = match (&contact, service.hello().token) {
            (Some(_), _) => None,
            (None, Some(token)) => Some(token),
            (None, None) => bail!("not a contact"),
        };
        proto::send(&mut stream, &challenge).await?;
        let proof: Proof = timeout(STEP, proto::receive(&mut stream)).await??;
        let peer = service.on_proof(&proof, contact.as_ref().map(|c| &c.devices)).map_err(handshake)?;

        let contact = match (contact, token) {
            (Some(mut contact), _) => {
                self.update_devices(&mut contact, &service.hello().devices)?;
                contact
            }
            (None, Some(token)) => {
                // One use, even if what follows fails: whoever got here holds
                // the link, and it is spent.
                let key = hex(&token);
                let pending: Pending = postcard::from_bytes(
                    &self.store.get("invite", &key)?.context("unknown or used invitation")?,
                )?;
                self.store.remove("invite", &key)?;
                ensure!(pending.expires > now_secs(), "expired invitation");
                let (contact, bundle) = self.exchange_cards(&mut stream, &peer, &service.hello().devices, &pending.name).await?;
                self.add_contact(&contact, &bundle, peer.device).await?;
                contact
            }
            (None, None) => unreachable!(),
        };
        self.clone().link(stream, contact, peer.device).await;
        Ok(())
    }

    /// Both sides of an invitation: send our card and read theirs. Nothing
    /// is kept until [`Daemon::add_contact`].
    async fn exchange_cards(
        self: &Rc<Self>,
        stream: &mut T::Stream,
        peer: &Peer,
        devices: &SignedDeviceList,
        name: &str,
    ) -> Result<(Contact, PreKeyBundle)> {
        self.check_name(name)?;
        let bundle = WireBundle::new(&session::bundle(&self.store, device_id(self.device.get())?).await?)?;
        proto::send(stream, &Frame::Card { discovery: discovery_key(&self.discovery()), bundle }).await?;
        let Frame::Card { discovery, bundle } = (match timeout(STEP, proto::receive(stream)).await?? {
            Frame::Refused => bail!(t!("err-already-contact")),
            frame => frame,
        }) else {
            bail!("expected a contact card");
        };
        let bundle = bundle.bundle()?;
        // A bundle for any other identity than the one just authenticated
        // would start a session with someone else.
        ensure!(*bundle.identity_key()? == peer.account, "the bundle is not from the identity that signed in");
        ensure!(u8::from(bundle.device_id()?) == peer.device, "the bundle is for another device");

        let contact = Contact { name: name.into(), identity: peer.account.serialize().into_vec(), devices: devices.clone(), discovery };
        Ok((contact, bundle))
    }

    /// Starts the session from their bundle and keeps the contact.
    async fn add_contact(self: &Rc<Self>, contact: &Contact, bundle: &PreKeyBundle, device: u8) -> Result<()> {
        self.check_name(&contact.name)?;
        session::start(&self.store, &self.address(), &address(&contact.identity, device)?, bundle).await?;
        let bytes = postcard::to_stdvec(contact)?;
        self.store.atomically(|| {
            self.store.put("contact", &hex(&contact.identity), &bytes)?;
            self.tell_mine(Mine::Contact(bytes.clone()))
        })?;
        self.nudge_mine();
        self.present_to(contact)?;
        self.refresh_service().await?;
        self.fetch_now.notify_one();
        let _ = self.events.send(Event::ContactAdded { name: contact.name.clone() });
        Ok(())
    }

    // --- delivery ----------------------------------------------------------

    /// Opens a link to every device something is queued for, backing off
    /// after failures.
    async fn deliver(self: Rc<Self>) {
        loop {
            match self.store.queued_for() {
                // Off our list: nothing goes out any more.
                Ok(_) if self.removed.get() => {}
                Ok(mut targets) => {
                    targets.extend(self.announce.borrow().iter().cloned());
                    for target in targets {
                        let due = self.backoff.borrow().get(&target).is_none_or(|(_, at)| *at <= Instant::now());
                        if due && !self.links.borrow().contains_key(&target) {
                            self.links.borrow_mut().insert(target.clone(), None);
                            spawn_local(self.clone().dial(target));
                        }
                    }
                }
                Err(e) => tracing::warn!("reading the outbox: {e}"),
            }
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = sleep(Duration::from_secs(5)) => {}
                _ = self.stopped() => return,
            }
        }
    }

    async fn dial(self: Rc<Self>, target: Target) {
        self.set_link(&target, LinkState::Trying);
        match self.try_dial(&target).await {
            Ok((stream, contact)) => {
                let started = Instant::now();
                let heard = self.clone().link(stream, contact, target.1).await;
                // Closed at once without a word: turned down after the
                // handshake (a device they no longer list). As a failed dial,
                // so it is not tried again and again.
                if heard || started.elapsed() > Duration::from_secs(1) {
                    self.backoff.borrow_mut().remove(&target);
                } else {
                    self.failed(target);
                }
            }
            Err(e) => {
                tracing::info!("no link to a contact: {e:#}");
                let state = match e.downcast_ref::<OtherVersion>() {
                    Some(OtherVersion(theirs)) if *theirs > PROTOCOL_VERSION => LinkState::Newer,
                    Some(_) => LinkState::Older,
                    None => LinkState::Unreachable,
                };
                self.set_link(&target, state);
                self.links.borrow_mut().remove(&target);
                self.failed(target);
            }
        }
    }

    /// Waits longer before the next try, up to 10 minutes.
    /// From the second failure, what waits for that device goes to its
    /// account's mailbox, if it has one.
    fn failed(self: &Rc<Self>, target: Target) {
        let failures = {
            let mut backoff = self.backoff.borrow_mut();
            let failures = backoff.get(&target).map_or(0, |(n, _)| *n) + 1;
            let wait = (RETRY * (1 << failures.min(6))).min(Duration::from_secs(600));
            backoff.insert(target.clone(), (failures, Instant::now() + wait));
            failures
        };
        if failures >= 2 {
            self.visit_soon(target.0);
        }
    }

    // --- mailboxes ---------------------------------------------------------

    /// Looks in our mailbox at start, every 10 minutes, and when told to;
    /// after a failure, again sooner.
    async fn fetcher(self: Rc<Self>) {
        let mut wait = RETRY;
        loop {
            wait = match self.visit_once(self.account()).await {
                true => FETCH,
                false => (wait * 2).min(FETCH),
            };
            tokio::select! {
                _ = self.fetch_now.notified() => {}
                _ = sleep(wait) => {}
                _ = self.stopped() => return,
            }
        }
    }

    fn visit_soon(self: &Rc<Self>, account: Vec<u8>) {
        let daemon = self.clone();
        spawn_local(async move {
            daemon.visit_once(account).await;
        });
    }

    /// One visit at a time to each account's mailbox. False if it failed.
    async fn visit_once(self: &Rc<Self>, account: Vec<u8>) -> bool {
        if self.removed.get() || !self.visiting.borrow_mut().insert(account.clone()) {
            return true;
        }
        let result = self.visit(&account).await;
        self.visiting.borrow_mut().remove(&account);
        result.inspect_err(|e| tracing::info!("no visit to a mailbox: {e:#}")).is_ok()
    }

    /// Leaves at `account`'s mailbox what waits for its devices that did not
    /// answer twice. At ours, also fetches what waits for this device.
    async fn visit(self: &Rc<Self>, account: &[u8]) -> Result<()> {
        let Some(contact) = self.contact(account)? else { return Ok(()) };
        let list = self.devices_of(&contact)?;
        let Some(mailbox) = list.devices.iter().find(|d| d.role == Role::Mailbox) else { return Ok(()) };
        let (mut stream, contact) = self.try_dial(&(account.to_vec(), mailbox.id)).await?;
        let result = async {
            for device in list.devices.iter().filter(|d| d.role == Role::Normal) {
                let target = (account.to_vec(), device.id);
                let asleep = self.backoff.borrow().get(&target).is_some_and(|(failures, _)| *failures >= 2);
                if !asleep || self.links.borrow().contains_key(&target) {
                    continue;
                }
                // Not dialed while it is being left here.
                self.links.borrow_mut().insert(target.clone(), None);
                let left = self.deposit(&mut stream, &contact, device.id).await;
                self.links.borrow_mut().remove(&target);
                left?;
            }
            if self.is_own(&contact) {
                self.fetch(&mut stream).await?;
            }
            Ok(())
        }
        .await;
        let _ = stream.shutdown().await;
        result
    }

    /// Leaves what waits for `device`, encrypted for it, at its mailbox.
    async fn deposit(&self, stream: &mut T::Stream, contact: &Contact, device: u8) -> Result<()> {
        let queued = self.store.queued(&contact.identity, device)?;
        if queued.is_empty() {
            return Ok(());
        }
        let (me, them) = (self.address(), address(&contact.identity, device)?);
        if !self.has_session(&them)? {
            proto::send(stream, &Frame::BundleOf(device)).await?;
            match timeout(STEP, proto::receive(stream)).await?? {
                Frame::Bundle(bundle) => {
                    let bundle = bundle.bundle()?;
                    // The mailbox hands it on; the device made and signed it.
                    ensure!(bundle.identity_key()?.serialize()[..] == contact.identity[..], "the bundle is not from that account");
                    ensure!(u8::from(bundle.device_id()?) == device, "the bundle is for another device");
                    session::start(&self.store, &me, &them, &bundle).await?;
                }
                // None left: it waits for the device itself.
                Frame::Refused => return Ok(()),
                frame => bail!("unexpected from a mailbox: {frame:?}"),
            }
        }
        for (id, envelope) in queued {
            let message = session::encrypt(&self.store, &me, &them, &envelope).await?;
            proto::send(stream, &Frame::Deposit { device, id, kind: message.message_type() as u8, body: message.serialize().to_vec() }).await?;
            match timeout(STEP, proto::receive(stream)).await?? {
                Frame::Deposited { device: d, id: i } if d == device && i == id => {
                    if self.store.acked(&contact.identity, device, &id)? {
                        let _ = self.events.send(Event::Delivered { to: contact.name.clone(), id: hex(&id) });
                    }
                }
                // Full: it waits here.
                Frame::Refused => return Ok(()),
                frame => bail!("unexpected from a mailbox: {frame:?}"),
            }
        }
        Ok(())
    }

    /// At our mailbox: tells it our contacts, takes what waits for this
    /// device, and leaves bundles for contacts who never met it.
    async fn fetch(self: &Rc<Self>, stream: &mut T::Stream) -> Result<()> {
        proto::send(stream, &Frame::Contacts(self.mailbox_contacts()?)).await?;
        proto::send(stream, &Frame::Fetch).await?;
        loop {
            match timeout(STEP, proto::receive(stream)).await?? {
                Frame::Stored { account, device, id, kind, body } => {
                    match self.contact(&account)? {
                        Some(from) if self.devices_of(&from)?.device(device).is_some() => {
                            match self.receive(&from, &address(&account, device)?, id, kind, &body).await {
                                Ok(Some(event)) => {
                                    let _ = self.events.send(event);
                                }
                                Ok(None) => {}
                                Err(e) => tracing::warn!("dropped a message kept for us: {e:#}"),
                            }
                        }
                        _ => tracing::info!("dropped a message kept for us: not from a contact's device"),
                    }
                    proto::send(stream, &Frame::Ack { id }).await?;
                }
                Frame::Fetched { bundles } => {
                    let mut fresh = Vec::new();
                    for _ in bundles..mailbox::BUNDLES {
                        fresh.push(WireBundle::new(&session::bundle(&self.store, device_id(self.device.get())?).await?)?);
                    }
                    if !fresh.is_empty() {
                        proto::send(stream, &Frame::Bundles(fresh)).await?;
                    }
                    return Ok(());
                }
                frame => bail!("unexpected from a mailbox: {frame:?}"),
            }
        }
    }

    /// Our contacts as our mailbox gets them: without the names we gave them.
    fn mailbox_contacts(&self) -> Result<Vec<Vec<u8>>> {
        self.contacts_raw()?
            .into_iter()
            .map(|mut contact| {
                contact.name.clear();
                Ok(postcard::to_stdvec(&contact)?)
            })
            .collect()
    }

    async fn try_dial(self: &Rc<Self>, (account, device): &Target) -> Result<(T::Stream, Contact)> {
        let mut contact = self.contact(account)?.context("no longer a contact")?;
        let to: Onion = self.devices_of(&contact)?.device(*device).context("device no longer listed")?.onion;
        let mut stream = timeout(STEP * 2, self.net.dial(to)).await.context("dial timed out")??;
        let myself = self.myself();
        let me = self.me(&myself);
        let caller = Caller::new(&me, to, None);
        proto::send(&mut stream, caller.hello()).await?;
        let challenge: Challenge = challenge(&mut stream).await?;
        if *account == self.account() {
            self.update_devices(&mut contact, &challenge.devices)?;
        }
        let expected = IdentityKey::decode(account)?;
        let (proof, _) = caller.on_challenge(&me, &challenge, &expected, Some(&contact.devices)).map_err(handshake)?;
        proto::send(&mut stream, &proof).await?;
        self.update_devices(&mut contact, &challenge.devices)?;
        Ok((stream, contact))
    }

    /// Runs an authenticated link: sends what is queued for that device,
    /// stores and acknowledges what comes in, until it goes quiet or breaks.
    /// Whether anything came from the other side.
    async fn link(self: Rc<Self>, stream: T::Stream, contact: Contact, device: u8) -> bool {
        let target = (contact.identity.clone(), device);
        let (wake, mut woken) = mpsc::unbounded_channel();
        self.links.borrow_mut().insert(target.clone(), Some(wake));
        // It carried our list: announced.
        self.announce.borrow_mut().remove(&target);
        self.set_link(&target, LinkState::Open);
        let mut heard = false;
        if let Err(e) = self.run_link(stream, &contact, device, &mut woken, &mut heard).await {
            tracing::info!("link to a contact closed: {e:#}");
        }
        self.links.borrow_mut().remove(&target);
        // Nothing for someone just removed.
        self.set_link(&target, LinkState::Idle);
        // Whatever was in flight goes again on the next link.
        self.wake.notify_one();
        heard
    }

    async fn run_link(
        self: &Rc<Self>,
        stream: T::Stream,
        contact: &Contact,
        device: u8,
        woken: &mut mpsc::UnboundedReceiver<Poke>,
        heard: &mut bool,
    ) -> Result<()> {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let (frames_in, mut frames) = mpsc::unbounded_channel();
        // Reading a frame is not cancel-safe, so it gets its own task and the
        // loop below only ever waits on channels.
        let reading = spawn_local(async move {
            while let Ok(frame) = proto::receive::<Frame>(&mut reader).await {
                if frames_in.send(frame).is_err() {
                    break;
                }
            }
        });
        let result = async {
            let (me, them) = (self.address(), address(&contact.identity, device)?);
            let mut in_flight = HashSet::new();
            let mut asked = false;
            loop {
                // Renamed since, or removed: then the link ends here.
                let contact = &self.contact(&contact.identity)?.context("no longer a contact")?;
                if self.devices_of(contact)?.device(device).is_none() {
                    // One of ours taken off: it learns it from the list.
                    if self.is_own(contact) {
                        let list = self.own.borrow().devices.clone();
                        proto::send(&mut writer, &Frame::Devices(list)).await?;
                        // Until it has read it and hung up.
                        let _ = timeout(STEP, async { while frames.recv().await.is_some() {} }).await;
                    }
                    bail!("device no longer listed");
                }
                let queued = self.store.queued(&contact.identity, device)?;
                let session = self.has_session(&them)?;
                // A device met only through a device list: one bundle starts
                // the session. Whoever has nothing to send waits for the
                // other's first message to start it.
                if !session && !asked && !queued.is_empty() {
                    proto::send(&mut writer, &Frame::GetBundle).await?;
                    asked = true;
                }
                for (id, envelope) in queued.into_iter().filter(|_| session) {
                    if in_flight.insert(id) {
                        let message = session::encrypt(&self.store, &me, &them, &envelope).await?;
                        let frame = Frame::Message { id, kind: message.message_type() as u8, body: message.serialize().to_vec() };
                        proto::send(&mut writer, &frame).await?;
                    }
                }
                tokio::select! {
                    frame = frames.recv() => match frame.inspect(|_| *heard = true) {
                        None => return Ok(()),
                        Some(Frame::Message { id, kind, body }) => {
                            // Not kept from someone removed while we waited,
                            // nor from a device they no longer list.
                            let contact = &self.contact(&contact.identity)?.context("no longer a contact")?;
                            if self.devices_of(contact)?.device(device).is_none() {
                                continue;
                            }
                            match self.receive(contact, &them, id, kind, &body).await {
                                Ok(Some(event)) => { let _ = self.events.send(event); }
                                Ok(None) => {}
                                Err(e) => tracing::warn!("dropped a message from a contact: {e:#}"),
                            }
                            proto::send(&mut writer, &Frame::Ack { id }).await?;
                        }
                        Some(Frame::Ack { id }) => {
                            in_flight.remove(&id);
                            if self.store.acked(&contact.identity, device, &id)? {
                                let _ = self.events.send(Event::Delivered { to: contact.name.clone(), id: hex(&id) });
                            }
                        }
                        Some(Frame::GetBundle) => {
                            let bundle = WireBundle::new(&session::bundle(&self.store, device_id(self.device.get())?).await?)?;
                            proto::send(&mut writer, &Frame::Bundle(bundle)).await?;
                        }
                        Some(Frame::Bundle(bundle)) => {
                            let bundle = bundle.bundle()?;
                            // As for a card: from the device authenticated, nobody else.
                            ensure!(bundle.identity_key()?.serialize()[..] == contact.identity[..], "the bundle is not from the identity that signed in");
                            ensure!(u8::from(bundle.device_id()?) == device, "the bundle is for another device");
                            if !self.has_session(&them)? {
                                session::start(&self.store, &me, &them, &bundle).await?;
                            }
                        }
                        // Signed by their account: checked there, kept if newer.
                        Some(Frame::Devices(list)) => {
                            self.update_devices(&mut contact.clone(), &list)?;
                            if self.removed.get() {
                                return Ok(());
                            }
                        }
                        Some(frame) => bail!("not a frame for a link between devices: {frame:?}"),
                    },
                    Some(poke) = woken.recv() => match poke {
                        Poke::Queued => {}
                        // Not queued: nothing waits for its acknowledgement.
                        Poke::Now(envelope) => if session {
                            let id: Envelope = postcard::from_bytes(&envelope)?;
                            let message = session::encrypt(&self.store, &me, &them, &envelope).await?;
                            let frame = Frame::Message { id: id.id, kind: message.message_type() as u8, body: message.serialize().to_vec() };
                            proto::send(&mut writer, &frame).await?;
                        },
                        Poke::Devices => {
                            let list = self.own.borrow().devices.clone();
                            proto::send(&mut writer, &Frame::Devices(list)).await?
                        }
                    },
                    _ = sleep(IDLE) => return Ok(()),
                    _ = self.stopped() => return Ok(()),
                }
            }
        }
        .await;
        reading.abort();
        let _ = writer.shutdown().await;
        result
    }

    /// Decrypts and stores one message. `None` for one already received.
    async fn receive(self: &Rc<Self>, contact: &Contact, them: &ProtocolAddress, id: [u8; 16], kind: u8, body: &[u8]) -> Result<Option<Event>> {
        let message = if kind == CiphertextMessageType::PreKey as u8 {
            CiphertextMessage::PreKeySignalMessage(PreKeySignalMessage::try_from(body)?)
        } else if kind == CiphertextMessageType::Whisper as u8 {
            CiphertextMessage::SignalMessage(SignalMessage::try_from(body)?)
        } else {
            bail!("unknown message kind {kind}");
        };
        let plain = session::decrypt(&self.store, &self.address(), them, &message).await?;
        let envelope: Envelope = postcard::from_bytes(&plain)?;
        ensure!(envelope.id == id, "the envelope id does not match the frame");
        // What our own devices say is about the account; nobody else's is.
        ensure!(self.is_own(contact) == matches!(envelope.body, Body::Mine(_)), "a message for another kind of link");
        let text = match envelope.body {
            Body::Mine(mine) => return self.mine(mine).await,
            Body::Text(text) => text,
            Body::Read(ids) => {
                ensure!(ids.len() <= MAX_READ, "a read receipt naming too many messages");
                // Ours off, theirs unseen: what we do not give, we do not take.
                if !self.read_receipts()? {
                    return Ok(None);
                }
                let ids = self.store.read_outgoing(&contact.identity, &ids)?;
                if ids.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(Event::Read { to: contact.name.clone(), ids: ids.iter().map(|id| hex(id)).collect() }));
            }
            // As read receipts: taken only from someone we tell too.
            Body::Typing => return Ok(self.typing_indicators()?.then(|| Event::Typing { from: contact.name.clone() })),
            Body::Timer(seconds) => {
                self.keep_timer(contact, seconds)?;
                return Ok(Some(Event::TimerChanged { name: contact.name.clone(), seconds, by_me: false }));
            }
        };
        ensure!(text.len() <= MAX_TEXT, "message too long");
        let text = crate::text::message(&text);
        // The sender's clock, unless it claims a time more than a day ahead.
        let now = now_millis();
        let time = if envelope.time > now + 86_400_000 { now } else { envelope.time };
        // Theirs end a timer after we read them.
        if !self.store.add_timed(&contact.identity, &id, false, time, &text, self.timer(contact)?, None)? {
            return Ok(None);
        }
        Ok(Some(Event::Message { from: contact.name.clone(), id: hex(&id), time, text }))
    }

    /// What another of our devices did, done here too.
    async fn mine(self: &Rc<Self>, mine: Mine) -> Result<Option<Event>> {
        Ok(match mine {
            Mine::Sent { to, id, time, text, ttl } => {
                let contact = self.contact(&to)?.filter(|c| !self.is_own(c)).context("a copy for someone not a contact")?;
                ensure!(text.len() <= MAX_TEXT && TIMERS.contains(&ttl), "a copy out of bounds");
                let expires = (ttl > 0).then(|| time + u64::from(ttl) * 1000);
                // Its acknowledgements went to the device that sent it.
                let added = self.store.atomically(|| {
                    let added = self.store.add_timed(&to, &id, true, time, &text, ttl, expires)?;
                    self.store.delivered(&to, &id)?;
                    Ok(added)
                })?;
                added.then(|| Event::Sent { to: contact.name, id: hex(&id), time, text })
            }
            Mine::Note { id, time, text } => {
                ensure!(text.len() <= MAX_TEXT, "a note too long");
                self.store.add_note(&id, time, &text)?.then(|| Event::Sent { to: self.note_name().unwrap_or_default(), id: hex(&id), time, text })
            }
            Mine::Contact(bytes) => {
                let mut contact: Contact = postcard::from_bytes(&bytes)?;
                self.devices_of(&contact)?;
                ensure!(contact.identity != self.account(), "ourselves as a contact");
                let before = self.contact(&contact.identity)?;
                // Their newest list, whoever saw it first.
                if let Some(before) = &before {
                    match contact.devices.verify(&IdentityKey::decode(&contact.identity)?, Some(&before.devices)) {
                        Ok(list) if list.seq > before.devices.seq() => {}
                        _ => contact.devices = before.devices.clone(),
                    }
                }
                if self.contacts_raw()?.iter().any(|c| c.name == contact.name && c.identity != contact.identity) || self.is_note(&contact.name)? {
                    contact.name.push('~');
                }
                self.check_name_shape(&contact.name)?;
                self.store.put("contact", &hex(&contact.identity), &postcard::to_stdvec(&contact)?)?;
                self.present_to(&contact)?;
                self.refresh_service().await?;
                match before {
                    Some(before) if before.name != contact.name => {
                        self.rename_link_state(&before.name, &contact.name);
                        Some(Event::ContactRenamed { name: before.name, to: contact.name })
                    }
                    Some(_) => None,
                    None => Some(Event::ContactAdded { name: contact.name }),
                }
            }
            Mine::Forget { account, wipe } => {
                let Some(contact) = self.contact(&account)?.filter(|c| !self.is_own(c)) else { return Ok(None) };
                let key = hex(&account);
                self.store.atomically(|| {
                    self.store.remove("contact", &key)?;
                    self.store.remove("person", &key)?;
                    self.store.forget(&account, wipe)
                })?;
                self.link_states.borrow_mut().remove(&contact.name);
                self.device_links.borrow_mut().retain(|(a, _), _| *a != account);
                self.refresh_service().await?;
                Some(Event::ContactRemoved { name: contact.name })
            }
            Mine::Seen(account) => {
                self.store.read_incoming(&account, now_millis())?;
                None
            }
            Mine::Timer { account, seconds } => {
                let contact = self.contact(&account)?.filter(|c| !self.is_own(c)).context("a timer for someone not a contact")?;
                self.keep_timer(&contact, seconds)?;
                Some(Event::TimerChanged { name: contact.name, seconds, by_me: true })
            }
            Mine::Names(names) => {
                for (id, name) in names {
                    self.check_name_shape(&name)?;
                    self.store.put("device_name", &id.to_string(), name.as_bytes())?;
                }
                Some(Event::DevicesChanged)
            }
        })
    }

    // --- helpers -----------------------------------------------------------

    /// What this device presents now: the list changes as devices come and go.
    fn myself(&self) -> Myself {
        Myself { account: *self.identity.borrow().identity_key(), device: self.device.get(), devices: self.own.borrow().devices.clone() }
    }

    fn me<'a>(&'a self, myself: &'a Myself) -> Me<'a> {
        Me { account: &myself.account, device: myself.device, key: &self.signing, devices: &myself.devices }
    }

    /// Our identity key, serialized: the account.
    fn account(&self) -> Vec<u8> {
        self.identity.borrow().identity_key().serialize().into_vec()
    }

    /// Our own account, as a contact: our other devices are reached as
    /// contacts' are. Unnamed, and never in `contacts`.
    fn own_contact(&self) -> Contact {
        let own = self.own.borrow();
        Contact { name: String::new(), identity: self.account(), devices: own.devices.clone(), discovery: discovery_key(&Secret::new(own.discovery)) }
    }

    fn is_own(&self, contact: &Contact) -> bool {
        contact.identity == self.account()
    }

    fn has_session(&self, them: &ProtocolAddress) -> Result<bool> {
        Ok(self.store.get("session", &them.to_string())?.is_some())
    }

    fn rename_link_state(&self, name: &str, to: &str) {
        let state = self.link_states.borrow_mut().remove(name);
        if let Some(state) = state {
            self.link_states.borrow_mut().insert(to.into(), state);
        }
    }

    fn safety_number(&self, contact: &Contact) -> Result<String> {
        safety_number(self.identity.borrow().identity_key(), &IdentityKey::decode(&contact.identity)?).map_err(|e| anyhow::anyhow!("{e:?}"))
    }

    fn seed(&self) -> Secret {
        Secret::new(self.own.borrow().seed)
    }

    fn discovery(&self) -> Secret {
        Secret::new(self.own.borrow().discovery)
    }

    fn address(&self) -> ProtocolAddress {
        address(&self.account(), self.device.get()).expect("valid own address")
    }

    /// A contact by account; our own account too (see [`Self::own_contact`]).
    fn contact(&self, account: &[u8]) -> Result<Option<Contact>> {
        if *account == self.account()[..] {
            return Ok(Some(self.own_contact()));
        }
        Ok(match self.store.get("contact", &hex(account))? {
            Some(bytes) => Some(postcard::from_bytes(&bytes)?),
            None => None,
        })
    }

    fn contact_named(&self, name: &str) -> Result<Contact> {
        self.contacts_raw()?.into_iter().find(|c| c.name == name).with_context(|| t!("err-no-contact"))
    }

    fn contacts_raw(&self) -> Result<Vec<Contact>> {
        self.store.list("contact")?.into_iter().map(|(_, bytes)| Ok(postcard::from_bytes(&bytes)?)).collect()
    }

    fn devices_of(&self, contact: &Contact) -> Result<DeviceList> {
        contact.devices.verify(&IdentityKey::decode(&contact.identity)?, None).map_err(|e| anyhow::anyhow!("{e:?}"))
    }

    /// Keeps the list a contact (or one of ours) presented if it is newer
    /// than ours. What still waits for them goes to their new devices too;
    /// their devices gone get nothing more.
    fn update_devices(self: &Rc<Self>, contact: &mut Contact, presented: &SignedDeviceList) -> Result<()> {
        let identity = IdentityKey::decode(&contact.identity)?;
        let new = presented.verify(&identity, None).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let old = self.devices_of(contact)?;
        if new.seq <= old.seq {
            return Ok(());
        }
        let own = self.is_own(contact);
        contact.devices = presented.clone();
        self.store.atomically(|| {
            for gone in old.devices.iter().filter(|d| new.device(d.id).is_none()) {
                self.store.forget_device(&contact.identity, gone.id)?;
                if let Ok(them) = address(&contact.identity, gone.id) {
                    self.store.remove("session", &them.to_string())?;
                }
            }
            for added in new.devices.iter().filter(|d| d.role == Role::Normal && old.device(d.id).is_none()) {
                if !(own && added.id == self.device.get()) {
                    self.store.copy_queue(&contact.identity, added.id)?;
                }
            }
            if own {
                let mut keys = self.own.borrow_mut();
                keys.devices = presented.clone();
                self.store.put("self", "own", &postcard::to_stdvec(&*keys).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?)
            } else {
                self.store.put("contact", &hex(&contact.identity), &postcard::to_stdvec(&*contact).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?)
            }
        })?;
        self.present_to(contact)?;
        if own {
            if new.device(self.device.get()).is_none() && !self.removed.replace(true) {
                let _ = self.events.send(Event::Removed);
            }
            let _ = self.events.send(Event::DevicesChanged);
            // Our mailbox learns the list from the handshake.
            self.fetch_now.notify_one();
            let daemon = self.clone();
            spawn_local(async move {
                if let Err(e) = daemon.refresh_service().await {
                    tracing::warn!("serving our new devices: {e:#}");
                }
            });
        }
        self.wake.notify_one();
        Ok(())
    }

    /// Files our discovery key for each of the contact's devices.
    fn present_to(&self, contact: &Contact) -> Result<()> {
        for device in self.devices_of(contact)?.devices {
            self.net.present(device.onion, &self.discovery())?;
        }
        Ok(())
    }

    fn check_name_shape(&self, name: &str) -> Result<()> {
        ensure!(
            !name.trim().is_empty() && name.chars().count() <= MAX_NAME && !name.chars().any(char::is_control),
            "a name is 1 to {MAX_NAME} characters, without control characters"
        );
        Ok(())
    }

    fn check_name(&self, name: &str) -> Result<()> {
        self.check_name_shape(name)?;
        ensure!(self.contacts_raw()?.iter().all(|c| c.name != name) && !self.is_note(name)?, t!("err-name-taken", name = name));
        Ok(())
    }
}

fn address(account: &[u8], device: u8) -> Result<ProtocolAddress> {
    Ok(ProtocolAddress::new(hex(account), device_id(device)?))
}

fn device_id(id: u8) -> Result<DeviceId> {
    DeviceId::try_from(id).map_err(|_| anyhow::anyhow!("device id {id}"))
}

/// The other side runs another protocol version, which the handshake
/// starts with: whichever is older must update before they can talk.
#[derive(Debug)]
pub(crate) struct OtherVersion(pub(crate) u16);

impl std::fmt::Display for OtherVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&if self.0 > PROTOCOL_VERSION { t!("other-newer") } else { t!("other-older") })
    }
}

impl std::error::Error for OtherVersion {}

/// The service's challenge, or its version if it runs another one.
async fn challenge(stream: &mut (impl AsyncRead + Unpin)) -> Result<Challenge> {
    timeout(STEP, proto::receive_handshake(stream)).await??.map_err(|theirs| OtherVersion(theirs).into())
}

/// The caller's hello. One running another version is told ours, so that
/// it can say which side must update, and turned away.
pub(crate) async fn hello(stream: &mut (impl AsyncRead + AsyncWrite + Unpin)) -> Result<Hello> {
    match timeout(STEP, proto::receive_handshake(stream)).await?? {
        Ok(hello) => Ok(hello),
        Err(theirs) => {
            proto::send(stream, &PROTOCOL_VERSION).await?;
            Err(OtherVersion(theirs).into())
        }
    }
}

/// Waits for the other side to close: the temporary service that carried
/// the last words must outlive them, or Tor drops them on the way.
pub(crate) async fn hang_up(stream: &mut (impl AsyncRead + Unpin)) {
    let _ = timeout(STEP, stream.read(&mut [0; 1])).await;
}

/// The next step of a pairing, within a handshake step's time.
pub(crate) async fn step(stream: &mut (impl AsyncRead + Unpin)) -> Result<Step> {
    Ok(timeout(STEP, proto::receive(stream)).await??)
}

pub(crate) fn handshake(e: proto::HandshakeError) -> anyhow::Error {
    anyhow::anyhow!("handshake: {e:?}")
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("clock after 1970").as_secs()
}

fn now_millis() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("clock after 1970").as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbkey::Unlock;
    use crate::transport::memory::{MemoryTransport, Network};
    use std::path::Path;
    use tokio::task::LocalSet;
    use crate::mailbox::Mailbox;

    /// The contacts, without the note to ourselves that is always first.
    fn others(daemon: &Daemon<MemoryTransport>) -> Vec<ContactInfo> {
        daemon.contacts().unwrap().into_iter().filter(|c| !c.note).collect()
    }

    fn open(dir: &Path, name: &str, net: &Network) -> Rc<Daemon<MemoryTransport>> {
        let secret = Secret::new([name.len() as u8; 32]);
        let store = Store::open(&dir.join(name), &Unlock::new(Some(secret), None).unwrap()).unwrap();
        Daemon::open(store, net.node()).unwrap()
    }

    /// The next event about messages and contacts; how links and Tor stand
    /// is checked through `links()` where it matters.
    async fn next(events: &mut broadcast::Receiver<Event>) -> Event {
        loop {
            match timeout(Duration::from_secs(10), events.recv()).await.expect("no event within 10 s").unwrap() {
                Event::Link { .. } | Event::Network { .. } => continue,
                event => return event,
            }
        }
    }

    /// Skips events until one matches.
    async fn wait_for(events: &mut broadcast::Receiver<Event>, what: impl Fn(&Event) -> bool) -> Event {
        loop {
            let event = timeout(Duration::from_secs(10), events.recv()).await.expect("not within 10 s").unwrap();
            if what(&event) {
                return event;
            }
        }
    }

    /// Waits until `who` knows `count` devices of `of`'s account.
    async fn knows_devices(who: &Daemon<MemoryTransport>, of: &str, count: usize) {
        for _ in 0..200 {
            if who.devices_of(&who.contact_named(of).unwrap()).unwrap().devices.len() == count {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
        panic!("never learned {of}'s {count} devices");
    }

    /// Alice on one device and Bob, contacts, then Alice's second device.
    async fn alice_twice(dir: &Path, net: &Network) -> [Rc<Daemon<MemoryTransport>>; 3] {
        let (a1, bob) = (open(dir, "alice", net), open(dir, "bob-device", net));
        a1.start().await.unwrap();
        bob.start().await.unwrap();
        let link = a1.invite("bob").await.unwrap();
        bob.accept(&link, "alice").await.unwrap();
        let a2 = open(dir, "alice-2", net);
        a2.start().await.unwrap();
        let mut a2_events = a2.subscribe();
        let link = a2.pairing_link().await.unwrap();
        a1.add_device(&link).await.unwrap();
        wait_for(&mut a2_events, |e| *e == Event::Paired).await;
        knows_devices(&bob, "alice", 2).await;
        [a1, a2, bob]
    }

    /// A mailbox for `a1`'s account, kept in `store`, serving.
    async fn mailbox_for(a1: &Rc<Daemon<MemoryTransport>>, store: Rc<Store>, net: &Network) -> Rc<Mailbox<MemoryTransport>> {
        let (shown, link) = oneshot::channel();
        let opening = spawn_local(Mailbox::open(store, net.node(), move |link| {
            let _ = shown.send(link.to_string());
        }));
        a1.add_device(&link.await.unwrap()).await.unwrap();
        let mailbox = opening.await.unwrap().unwrap();
        spawn_local(mailbox.clone().run());
        mailbox
    }

    fn mailbox_store(dir: &Path) -> Rc<Store> {
        Rc::new(Store::open(&dir.join("mailbox"), &Unlock::new(Some(Secret::new([9; 32])), None).unwrap()).unwrap())
    }

    async fn until(what: impl Fn() -> bool, why: &str) {
        for _ in 0..200 {
            if what() {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
        panic!("{why}");
    }

    fn sent(to: &str, id: [u8; 16], text: &str) -> impl Fn(&Event) -> bool {
        let (to, id, text) = (to.to_string(), hex(&id), text.to_string());
        move |e| matches!(e, Event::Sent { to: t, id: i, text: x, .. } if *t == to && *i == id && *x == text)
    }

    fn message(from: &str, id: [u8; 16], text: &str) -> impl Fn(&Event) -> bool {
        let (from, id, text) = (from.to_string(), hex(&id), text.to_string());
        move |e| matches!(e, Event::Message { from: f, id: i, text: t, .. } if *f == from && *i == id && *t == text)
    }

    /// Invitation, a message each way, then one sent while Bob is off
    /// arrives when he comes back.
    #[tokio::test]
    async fn invite_talk_and_deliver_after_a_restart() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let alice = open(dir.path(), "alice", &net);
                let bob = open(dir.path(), "bob-device", &net);
                alice.start().await.unwrap();
                bob.start().await.unwrap();
                let (mut alice_events, mut bob_events) = (alice.subscribe(), bob.subscribe());

                let link = alice.invite("bob").await.unwrap();
                bob.accept(&link, "alice").await.unwrap();
                assert_eq!(next(&mut bob_events).await, Event::ContactAdded { name: "alice".into() });
                assert_eq!(next(&mut alice_events).await, Event::ContactAdded { name: "bob".into() });
                assert_eq!(others(&alice)[0].safety_number, others(&bob)[0].safety_number);

                // The link is spent: someone else holding it gets nowhere.
                let carol = open(dir.path(), "carol", &net);
                carol.start().await.unwrap();
                assert!(carol.accept(&link, "alice").await.is_err());
                assert!(others(&carol).is_empty());

                let id = bob.send("alice", "bonjour").unwrap();
                assert!(message("bob", id, "bonjour")(&next(&mut alice_events).await));
                assert!(sent("alice", id, "bonjour")(&next(&mut bob_events).await));
                assert_eq!(next(&mut bob_events).await, Event::Delivered { to: "alice".into(), id: hex(&id) });
                let id = alice.send("bob", "salut").unwrap();
                assert!(message("alice", id, "salut")(&next(&mut bob_events).await));
                let history = bob.history("alice", 10).unwrap();
                assert_eq!(history.iter().map(|m| (m.outgoing, m.text.as_str())).collect::<Vec<_>>(), [(true, "bonjour"), (false, "salut")]);
                assert!(sent("bob", id, "salut")(&next(&mut alice_events).await));
                assert_eq!(next(&mut alice_events).await, Event::Delivered { to: "bob".into(), id: hex(&id) });

                bob.stop().await;
                drop(bob);
                let id = alice.send("bob", "tu es là ?").unwrap();
                assert!(sent("bob", id, "tu es là ?")(&next(&mut alice_events).await));
                sleep(Duration::from_millis(200)).await;
                assert_eq!(alice.links(), [("bob".to_string(), LinkState::Unreachable)]);

                let bob = open(dir.path(), "bob-device", &net);
                let mut bob_events = bob.subscribe();
                bob.start().await.unwrap();
                assert!(message("alice", id, "tu es là ?")(&next(&mut bob_events).await));
                assert_eq!(next(&mut alice_events).await, Event::Delivered { to: "bob".into(), id: hex(&id) });
            })
            .await;
    }

    /// Tor coming back clears the waits that grew while it was away, so
    /// what is queued goes at once instead of after the last wait.
    #[tokio::test]
    async fn tor_coming_back_clears_the_waits() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let alice = open(dir.path(), "alice", &Network::default());
                let target = (vec![1u8; 33], 1u8);
                let waiting = || alice.backoff.borrow_mut().insert(target.clone(), (5, Instant::now() + Duration::from_secs(600)));
                let tor = |percent, entry| crate::api::Network { percent, entry, ..Default::default() };
                alice.set_network(tor(100, Some(false)));
                waiting();
                alice.set_network(tor(100, Some(false)));
                assert!(!alice.backoff.borrow().is_empty(), "blocked: still waiting");
                alice.set_network(tor(100, Some(true)));
                assert!(alice.backoff.borrow().is_empty(), "back: the waits are gone");
                waiting();
                alice.set_network(tor(50, None));
                alice.set_network(tor(100, None));
                assert!(alice.backoff.borrow().is_empty(), "connected: the waits are gone");
            })
            .await;
    }

    /// Renamed, they are reached under the new name; removed, they can no
    /// longer reach us, and with `wipe` the conversation is gone.
    #[tokio::test]
    async fn rename_then_remove() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let (alice, bob) = (open(dir.path(), "alice", &net), open(dir.path(), "bob-device", &net));
                alice.start().await.unwrap();
                bob.start().await.unwrap();
                let (mut alice_events, mut bob_events) = (alice.subscribe(), bob.subscribe());
                let link = alice.invite("bob").await.unwrap();
                bob.accept(&link, "alice").await.unwrap();
                next(&mut bob_events).await;
                next(&mut alice_events).await;

                alice.rename("bob", "robert").unwrap();
                assert_eq!(next(&mut alice_events).await, Event::ContactRenamed { name: "bob".into(), to: "robert".into() });
                assert!(alice.rename("robert", "robert").is_err() && alice.rename("bob", "x").is_err());
                let id = bob.send("alice", "toujours moi").unwrap();
                assert!(message("robert", id, "toujours moi")(&next(&mut alice_events).await));

                let person = Person { verified: true, muted_until: Some(u64::MAX), photo: None };
                alice.set_person("robert", person.clone()).unwrap();
                assert_eq!(next(&mut alice_events).await, Event::PersonChanged { name: "robert".into(), person: person.clone() });
                assert_eq!(others(&alice)[0].person, person);
                let bad = Person { photo: Some("data:text/html,<script>".into()), ..Person::default() };
                assert!(alice.set_person("robert", bad).is_err());

                alice.remove("robert", true).await.unwrap();
                assert_eq!(next(&mut alice_events).await, Event::ContactRemoved { name: "robert".into() });
                assert!(others(&alice).is_empty());
                assert!(alice.store.history(&bob.account(), 10).unwrap().is_empty());
                // Bob can no longer reach Alice's service, so his messages
                // no longer get through.
                assert!(bob.net.dial(onion(&alice.seed())).await.is_err());
                bob.send("alice", "tu es là ?").unwrap();
                sleep(Duration::from_millis(500)).await;
                assert!(alice.store.history(&bob.account(), 10).unwrap().is_empty());
                assert_eq!(bob.links(), [("alice".to_string(), LinkState::Unreachable)]);
            })
            .await;
    }

    /// A message sent again (the link broke before its ack came back) is
    /// stored and announced once, and stays queued until acknowledged.
    #[tokio::test]
    async fn a_message_sent_twice_shows_up_once() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let (alice, bob) = (open(dir.path(), "alice", &net), open(dir.path(), "bob-device", &net));
                alice.start().await.unwrap();
                bob.start().await.unwrap();
                let mut bob_events = bob.subscribe();
                let link = alice.invite("bob").await.unwrap();
                bob.accept(&link, "alice").await.unwrap();
                next(&mut bob_events).await;
                // Links closed, so nothing is delivered behind the test's back.
                alice.stop().await;
                let alice = { drop(alice); open(dir.path(), "alice", &net) };

                let id = alice.send("bob", "une fois").unwrap();
                let bob_contact = &alice.contacts_raw().unwrap()[0];
                let alice_contact = &bob.contacts_raw().unwrap()[0];
                let (envelope_id, envelope) = alice.store.queued(&bob_contact.identity, DEVICE).unwrap().remove(0);
                assert_eq!(envelope_id, id);
                let (a, b) = (alice.address(), address(&bob_contact.identity, DEVICE).unwrap());
                for (attempt, expect_new) in [(1, true), (2, false)] {
                    let sent = session::encrypt(&alice.store, &a, &b, &envelope).await.unwrap();
                    let got = bob.receive(alice_contact, &a, id, sent.message_type() as u8, sent.serialize()).await.unwrap();
                    assert_eq!(got.is_some(), expect_new, "attempt {attempt}");
                }
                assert_eq!(alice.store.queued(&bob_contact.identity, DEVICE).unwrap().len(), 1);
                assert!(alice.store.acked(&bob_contact.identity, DEVICE, &id).unwrap());
                assert!(alice.store.queued(&bob_contact.identity, DEVICE).unwrap().is_empty());
            })
            .await;
    }

    /// Read receipts go out only when on, and are taken only from someone
    /// who has them on too; one is never shown as a delivered message.
    #[tokio::test]
    async fn read_receipts_are_off_by_default_and_mutual() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let (alice, bob) = (open(dir.path(), "alice", &net), open(dir.path(), "bob-device", &net));
                alice.start().await.unwrap();
                bob.start().await.unwrap();
                let (mut alice_events, mut bob_events) = (alice.subscribe(), bob.subscribe());
                let link = alice.invite("bob").await.unwrap();
                bob.accept(&link, "alice").await.unwrap();
                next(&mut bob_events).await;
                next(&mut alice_events).await;
                let talk = async |text: &str, alice_events: &mut broadcast::Receiver<Event>, bob_events: &mut broadcast::Receiver<Event>| {
                    let id = alice.send("bob", text).unwrap();
                    assert!(message("alice", id, text)(&next(bob_events).await));
                    assert!(sent("bob", id, text)(&next(alice_events).await));
                    assert_eq!(next(alice_events).await, Event::Delivered { to: "bob".into(), id: hex(&id) });
                    bob.mark_read("alice").unwrap();
                    sleep(Duration::from_millis(300)).await;
                };
                let quiet = |events: &mut broadcast::Receiver<Event>| {
                    while let Ok(event) = events.try_recv() {
                        assert!(matches!(event, Event::Link { .. } | Event::Network { .. }), "{event:?}");
                    }
                };

                assert!(!alice.read_receipts().unwrap() && !bob.read_receipts().unwrap());
                talk("un", &mut alice_events, &mut bob_events).await;
                quiet(&mut alice_events);
                // Bob on, Alice off: his receipt arrives and is acknowledged,
                // but she gave none, so she sees none.
                bob.set_read_receipts(true).unwrap();
                assert_eq!(next(&mut bob_events).await, Event::ReadReceipts { on: true });
                talk("deux", &mut alice_events, &mut bob_events).await;
                quiet(&mut alice_events);
                quiet(&mut bob_events);
                let bob_contact = &alice.contacts_raw().unwrap()[0];
                let alice_contact = &bob.contacts_raw().unwrap()[0];
                assert!(bob.store.queued(&alice_contact.identity, DEVICE).unwrap().is_empty());

                alice.set_read_receipts(true).unwrap();
                assert_eq!(next(&mut alice_events).await, Event::ReadReceipts { on: true });
                let id = alice.send("bob", "trois").unwrap();
                next(&mut bob_events).await;
                next(&mut alice_events).await;
                next(&mut alice_events).await;
                bob.mark_read("alice").unwrap();
                assert_eq!(next(&mut alice_events).await, Event::Read { to: "bob".into(), ids: vec![hex(&id)] });
                let read: Vec<_> = alice.history("bob", 10).unwrap().iter().map(|m| (m.text.clone(), m.read)).collect();
                assert_eq!(read, [("un".into(), false), ("deux".into(), false), ("trois".into(), true)]);
                assert!(alice.store.queued(&bob_contact.identity, DEVICE).unwrap().is_empty());
                // Nothing new to read: nothing sent.
                bob.mark_read("alice").unwrap();
                assert!(bob.store.queued(&alice_contact.identity, DEVICE).unwrap().is_empty());
            })
            .await;
    }

    /// "Is typing" goes only over an open link, is never stored nor queued,
    /// and is seen only when both sides have typing indicators on.
    #[tokio::test]
    async fn typing_indicators_are_off_by_default_and_mutual() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let (alice, bob) = (open(dir.path(), "alice", &net), open(dir.path(), "bob-device", &net));
                alice.start().await.unwrap();
                bob.start().await.unwrap();
                let (mut alice_events, mut bob_events) = (alice.subscribe(), bob.subscribe());
                let link = alice.invite("bob").await.unwrap();
                bob.accept(&link, "alice").await.unwrap();
                next(&mut bob_events).await;
                next(&mut alice_events).await;
                // A message first, so the link is open.
                let id = alice.send("bob", "un").unwrap();
                assert!(message("alice", id, "un")(&next(&mut bob_events).await));
                next(&mut alice_events).await;
                next(&mut alice_events).await;
                let quiet = |events: &mut broadcast::Receiver<Event>| {
                    while let Ok(event) = events.try_recv() {
                        assert!(matches!(event, Event::Link { .. } | Event::Network { .. }), "{event:?}");
                    }
                };

                assert!(!alice.typing_indicators().unwrap() && !bob.typing_indicators().unwrap());
                alice.typing("bob").unwrap();
                sleep(Duration::from_millis(300)).await;
                quiet(&mut bob_events);
                // Alice on, Bob off: hers arrives, but he gives none, so he sees none.
                alice.set_typing_indicators(true).unwrap();
                assert_eq!(next(&mut alice_events).await, Event::TypingIndicators { on: true });
                alice.typing("bob").unwrap();
                sleep(Duration::from_millis(300)).await;
                quiet(&mut bob_events);

                bob.set_typing_indicators(true).unwrap();
                assert_eq!(next(&mut bob_events).await, Event::TypingIndicators { on: true });
                alice.typing("bob").unwrap();
                assert_eq!(next(&mut bob_events).await, Event::Typing { from: "alice".into() });
                sleep(Duration::from_millis(300)).await;
                // Its acknowledgement is no delivery, and nothing waits in a queue.
                quiet(&mut alice_events);
                let bob_contact = &alice.contacts_raw().unwrap()[0];
                assert!(alice.store.queued(&bob_contact.identity, DEVICE).unwrap().is_empty());
                assert_eq!(bob.history("alice", 10).unwrap().len(), 1);
            })
            .await;
    }

    /// A contact on another protocol version is told ours and turned away;
    /// one answering with a newer version shows as such, not as away.
    #[tokio::test]
    async fn another_protocol_version_says_who_must_update() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let (alice, bob) = (open(dir.path(), "alice", &net), open(dir.path(), "bob-device", &net));
                alice.start().await.unwrap();
                bob.start().await.unwrap();
                let (mut alice_events, mut bob_events) = (alice.subscribe(), bob.subscribe());
                let link = alice.invite("bob").await.unwrap();
                bob.accept(&link, "alice").await.unwrap();
                next(&mut bob_events).await;
                next(&mut alice_events).await;

                // Bob, newer, calls: the rest of his hello is unreadable here.
                let caller = net.node();
                caller.present(onion(&alice.seed()), &bob.discovery()).unwrap();
                let mut stream = caller.dial(onion(&alice.seed())).await.unwrap();
                proto::send(&mut stream, &(PROTOCOL_VERSION + 1, [0xffu8; 9])).await.unwrap();
                assert_eq!(proto::receive::<u16>(&mut stream).await.unwrap(), PROTOCOL_VERSION);

                // Bob's device, newer, answers Alice in his place.
                bob.shut_down();
                bob.service.borrow_mut().take();
                let (_service, mut incoming) = net.node().serve(&bob.seed(), &[discovery_key(&alice.discovery())]).await.unwrap();
                spawn_local(async move {
                    while let Some(mut stream) = incoming.next().await {
                        let _ = proto::receive_handshake::<Hello>(&mut stream).await;
                        let _ = proto::send(&mut stream, &(PROTOCOL_VERSION + 1)).await;
                    }
                });
                alice.send("bob", "hé").unwrap();
                timeout(Duration::from_secs(5), async {
                    while alice.links() != [("bob".to_string(), LinkState::Newer)] {
                        sleep(Duration::from_millis(20)).await;
                    }
                })
                .await
                .expect("shown as newer");
            })
            .await;
    }

    /// A code is checked for typos at once, and whoever uses it is kept
    /// only once the inviter confirms, after comparing safety numbers.
    /// Ours end a timer after sending, theirs a timer after being read; the
    /// other side applies the same timer, and nothing ends before its time.
    #[tokio::test]
    async fn messages_disappear_after_the_timer() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let (alice, bob) = (open(dir.path(), "alice", &net), open(dir.path(), "bob-device", &net));
                alice.start().await.unwrap();
                bob.start().await.unwrap();
                let (mut alice_events, mut bob_events) = (alice.subscribe(), bob.subscribe());
                let link = alice.invite("bob").await.unwrap();
                bob.accept(&link, "alice").await.unwrap();
                next(&mut bob_events).await;
                next(&mut alice_events).await;

                assert!(alice.set_timer("bob", 5).is_err());
                alice.set_timer("bob", 3_600).unwrap();
                assert_eq!(next(&mut alice_events).await, Event::TimerChanged { name: "bob".into(), seconds: 3_600, by_me: true });
                assert_eq!(next(&mut bob_events).await, Event::TimerChanged { name: "alice".into(), seconds: 3_600, by_me: false });
                assert_eq!(others(&bob)[0].timer, 3_600);

                let id = alice.send("bob", "secret").unwrap();
                assert!(message("alice", id, "secret")(&next(&mut bob_events).await));
                let hour = 3_600_000;
                let now = now_millis();
                // Not read yet: it does not count down.
                bob.sweep(now + 2 * hour).unwrap();
                assert_eq!(bob.history("alice", 10).unwrap().len(), 1);
                bob.mark_read("alice").unwrap();
                bob.sweep(now + hour / 2).unwrap();
                assert_eq!(bob.history("alice", 10).unwrap().len(), 1);
                bob.sweep(now + 2 * hour).unwrap();
                assert_eq!(next(&mut bob_events).await, Event::Expired { name: "alice".into() });
                assert!(bob.history("alice", 10).unwrap().is_empty());

                // Ours, from sending.
                alice.sweep(now + 2 * hour).unwrap();
                assert!(alice.history("bob", 10).unwrap().is_empty());

                // Off again: it stays.
                alice.set_timer("bob", 0).unwrap();
                alice.send("bob", "pour toujours").unwrap();
                alice.sweep(now + 10_000 * hour).unwrap();
                assert_eq!(alice.history("bob", 10).unwrap().len(), 1);
                assert!(alice.set_timer(&alice.contacts().unwrap()[0].name, 3_600).is_err());
            })
            .await;
    }

    /// The note to ourselves is first, kept on this device only, and cannot be removed.
    #[tokio::test]
    async fn the_note_to_ourselves_stays_here() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let alice = open(dir.path(), "alice", &net);
                alice.start().await.unwrap();
                let first = alice.contacts().unwrap().remove(0);
                assert!(first.note && !first.name.is_empty());
                let mut events = alice.subscribe();
                alice.send(&first.name, "le code du wifi : 1234").unwrap();
                assert!(matches!(next(&mut events).await, Event::Sent { to, text, .. } if to == first.name && text == "le code du wifi : 1234"));
                let kept = alice.history(&first.name, 10).unwrap();
                assert_eq!((kept.len(), kept[0].outgoing, kept[0].delivered), (1, true, true));
                // Nothing waits to be sent anywhere.
                assert!(alice.store.queued_for().unwrap().is_empty());
                assert!(alice.remove(&first.name, true).await.is_err());
                // A contact cannot take its name; renamed, it keeps its notes.
                assert!(alice.invite(&first.name).await.is_err());
                alice.rename(&first.name, "carnet").unwrap();
                assert_eq!(alice.history("carnet", 10).unwrap().len(), 1);
                assert_eq!(alice.contacts().unwrap()[0].name, "carnet");
            })
            .await;
    }

    #[tokio::test]
    async fn an_oral_code_waits_for_the_inviters_confirmation() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let (alice, bob, carol) =
                    (open(dir.path(), "alice", &net), open(dir.path(), "bob-device", &net), open(dir.path(), "carol", &net));
                for d in [&alice, &bob, &carol] {
                    d.start().await.unwrap();
                }
                let (mut alice_events, mut bob_events) = (alice.subscribe(), bob.subscribe());

                let code = alice.invite_code("bob").await.unwrap();
                let last = code.chars().last().unwrap().to_digit(10).unwrap();
                let typo = format!("{}{}", &code[..code.len() - 1], (last + 1) % 10);
                let err = bob.accept_code(&typo, "alice").await.unwrap_err();
                assert!(err.to_string().contains("typo"), "{err}");

                let accepting = spawn_local({
                    let (bob, code) = (bob.clone(), code.clone());
                    async move { bob.accept_code(&code, "alice").await }
                });
                let Event::Fingerprint { name, safety_number } = next(&mut alice_events).await else { panic!() };
                assert_eq!(name, "bob");
                let Event::Fingerprint { safety_number: bobs, .. } = next(&mut bob_events).await else { panic!() };
                assert_eq!(safety_number, bobs);
                assert!(others(&bob).is_empty() && others(&alice).is_empty());
                alice.confirm("bob", true).unwrap();
                accepting.await.unwrap().unwrap();
                assert_eq!(next(&mut bob_events).await, Event::ContactAdded { name: "alice".into() });
                assert_eq!(next(&mut alice_events).await, Event::ContactAdded { name: "bob".into() });
                let id = bob.send("alice", "trouvée par code").unwrap();
                assert!(message("bob", id, "trouvée par code")(&next(&mut alice_events).await));

                // Turned down: neither side keeps the other.
                let code = alice.invite_code("carol").await.unwrap();
                let accepting = spawn_local({
                    let carol = carol.clone();
                    async move { carol.accept_code(&code, "alice").await }
                });
                loop {
                    if let Event::Fingerprint { name, .. } = next(&mut alice_events).await {
                        assert_eq!(name, "carol");
                        break;
                    }
                }
                alice.confirm("carol", false).unwrap();
                assert!(accepting.await.unwrap().is_err());
                assert!(others(&carol).is_empty());
                assert_eq!(others(&alice).len(), 1);

                // Alice still has bob, who no longer has her: she says so
                // instead of just dropping the connection.
                bob.remove("alice", false).await.unwrap();
                let code = alice.invite_code("bob-again").await.unwrap();
                let err = bob.accept_code(&code, "alice").await.unwrap_err();
                assert_eq!(err.to_string(), t!("err-already-contact"));
            })
            .await;
    }
    /// A second device joins with the contacts, the history and the notes;
    /// then each device gets what the contact writes, and a copy of what
    /// the other one sends.
    #[tokio::test]
    async fn a_second_device_gets_everything() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let (a1, bob) = (open(dir.path(), "alice", &net), open(dir.path(), "bob-device", &net));
                a1.start().await.unwrap();
                bob.start().await.unwrap();
                let (mut a1_events, mut bob_events) = (a1.subscribe(), bob.subscribe());
                let link = a1.invite("bob").await.unwrap();
                bob.accept(&link, "alice").await.unwrap();
                let before = bob.send("alice", "avant").unwrap();
                wait_for(&mut a1_events, message("bob", before, "avant")).await;
                let note = a1.note_name().unwrap();
                a1.send(&note, "note 1").unwrap();

                let a2 = open(dir.path(), "alice-2", &net);
                a2.start().await.unwrap();
                let mut a2_events = a2.subscribe();
                // Not from a device that already has something of its own.
                assert!(bob.pairing_link().await.is_err());
                let link = a2.pairing_link().await.unwrap();
                assert!(bob.add_device("nightpost:1:AAAA").await.is_err());
                a1.add_device(&link).await.unwrap();
                wait_for(&mut a2_events, |e| *e == Event::Paired).await;
                assert_eq!(others(&a2).iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["bob"]);
                assert_eq!(a2.history("bob", 10).unwrap().iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["avant"]);
                assert_eq!(a2.history(&note, 10).unwrap()[0].text, "note 1");
                assert_eq!(a2.safety_number(&a2.contact_named("bob").unwrap()).unwrap(), others(&bob)[0].safety_number);
                for alice in [&a1, &a2] {
                    let devices = alice.devices().unwrap();
                    assert_eq!(devices.iter().map(|d| (d.id, d.this)).collect::<Vec<_>>(), [(1, alice.device.get() == 1), (2, alice.device.get() == 2)]);
                }
                knows_devices(&bob, "alice", 2).await;

                let id = bob.send("alice", "à vous deux").unwrap();
                wait_for(&mut a1_events, message("bob", id, "à vous deux")).await;
                wait_for(&mut a2_events, message("bob", id, "à vous deux")).await;

                let id = a1.send("bob", "depuis a1").unwrap();
                wait_for(&mut bob_events, message("alice", id, "depuis a1")).await;
                wait_for(&mut a2_events, sent("bob", id, "depuis a1")).await;
                let id = a2.send("bob", "depuis a2").unwrap();
                wait_for(&mut bob_events, message("alice", id, "depuis a2")).await;
                wait_for(&mut a1_events, sent("bob", id, "depuis a2")).await;
                let id = a1.send(&note, "note 2").unwrap();
                wait_for(&mut a2_events, sent(&note, id, "note 2")).await;

                a2.set_timer("bob", 3_600).unwrap();
                wait_for(&mut a1_events, |e| *e == Event::TimerChanged { name: "bob".into(), seconds: 3_600, by_me: true }).await;
                a1.rename("bob", "robert").unwrap();
                wait_for(&mut a2_events, |e| *e == Event::ContactRenamed { name: "bob".into(), to: "robert".into() }).await;
            })
            .await;
    }

    /// A new device and the contact write first at the same moment, each
    /// without a session with the other: nothing is lost.
    #[tokio::test]
    async fn first_messages_cross_without_loss() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let [a1, a2, bob] = alice_twice(dir.path(), &net).await;
                let (mut a1_events, mut a2_events, mut bob_events) = (a1.subscribe(), a2.subscribe(), bob.subscribe());
                let from_bob = bob.send("alice", "toi d'abord").unwrap();
                let from_a2 = a2.send("bob", "non, toi").unwrap();
                wait_for(&mut a2_events, message("bob", from_bob, "toi d'abord")).await;
                wait_for(&mut bob_events, message("alice", from_a2, "non, toi")).await;
                wait_for(&mut a1_events, message("bob", from_bob, "toi d'abord")).await;
                // And the sessions that crossed keep working both ways.
                let again = a2.send("bob", "encore").unwrap();
                wait_for(&mut bob_events, message("alice", again, "encore")).await;
                let back = bob.send("alice", "reçu").unwrap();
                wait_for(&mut a2_events, message("bob", back, "reçu")).await;
            })
            .await;
    }

    /// One device asleep does not keep a message "not delivered"; it still
    /// gets it when it wakes.
    #[tokio::test]
    async fn delivered_at_the_first_device() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let [a1, a2, bob] = alice_twice(dir.path(), &net).await;
                a2.stop().await;
                drop(a2);
                let mut bob_events = bob.subscribe();
                let id = bob.send("alice", "pour les deux").unwrap();
                wait_for(&mut bob_events, |e| *e == Event::Delivered { to: "alice".into(), id: hex(&id) }).await;
                assert_eq!(bob.store.queued(&a1.account(), 2).unwrap().len(), 1, "still waiting for the other device");
                let a2 = open(dir.path(), "alice-2", &net);
                let mut a2_events = a2.subscribe();
                a2.start().await.unwrap();
                wait_for(&mut a2_events, message("bob", id, "pour les deux")).await;
            })
            .await;
    }

    /// Bob writes while Alice's second device is off, then goes away too:
    /// it gets the message from the mailbox when it comes back, and the
    /// mailbox keeps nothing after.
    #[tokio::test]
    async fn the_mailbox_keeps_messages_for_an_offline_device() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let [a1, a2, bob] = alice_twice(dir.path(), &net).await;
                let store = mailbox_store(dir.path());
                let _mailbox = mailbox_for(&a1, store.clone(), &net).await;
                assert!(a1.devices().unwrap().iter().any(|d| d.id == 3 && d.mailbox));
                knows_devices(&bob, "alice", 3).await;
                // a2 learns of it from a1, and leaves bundles there for Bob,
                // who never met it.
                until(|| store.bundle_count(2).unwrap() == i64::from(mailbox::BUNDLES), "no bundles from a2").await;
                a2.stop().await;
                drop(a2);

                let mut a1_events = a1.subscribe();
                let id = bob.send("alice", "pour a2 aussi").unwrap();
                wait_for(&mut a1_events, message("bob", id, "pour a2 aussi")).await;
                let alice = a1.account();
                until(|| bob.store.queued(&alice, 2).unwrap().is_empty(), "never left at the mailbox").await;
                assert_eq!(store.deposits(2).unwrap().len(), 1);
                assert_eq!(store.bundle_count(2).unwrap(), i64::from(mailbox::BUNDLES) - 1);
                bob.stop().await;
                drop(bob);

                let a2 = open(dir.path(), "alice-2", &net);
                let mut a2_events = a2.subscribe();
                a2.start().await.unwrap();
                wait_for(&mut a2_events, message("bob", id, "pour a2 aussi")).await;
                until(|| store.deposits(2).unwrap().is_empty(), "still kept after the fetch").await;
                until(|| store.bundle_count(2).unwrap() == i64::from(mailbox::BUNDLES), "bundles not topped up").await;
            })
            .await;
    }

    /// The mailbox gets the account's public key, never the key pair.
    #[tokio::test]
    async fn the_mailbox_never_holds_the_identity() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let (a1, bob) = (open(dir.path(), "alice", &net), open(dir.path(), "bob-device", &net));
                a1.start().await.unwrap();
                bob.start().await.unwrap();
                let link = a1.invite("bob").await.unwrap();
                bob.accept(&link, "alice").await.unwrap();
                let store = mailbox_store(dir.path());
                let _mailbox = mailbox_for(&a1, store.clone(), &net).await;

                let private = a1.identity.borrow().private_key().serialize();
                let discovery = a1.own.borrow().discovery;
                for value in store.all_values() {
                    assert!(!value.windows(private.len()).any(|w| w == &private[..]), "the identity's private key");
                    assert!(!value.windows(32).any(|w| w == discovery), "the account's discovery key");
                    assert!(!value.windows(3).any(|w| w == b"bob"), "a contact's name");
                }
                assert_ne!(store.identity().unwrap().identity_key(), a1.identity.borrow().identity_key());
            })
            .await;
    }

    /// A device taken off the account gets nothing more, and learns it.
    #[tokio::test]
    async fn a_removed_device_gets_nothing() {
        LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let net = Network::default();
                let [a1, a2, bob] = alice_twice(dir.path(), &net).await;
                let (mut a1_events, mut a2_events) = (a1.subscribe(), a2.subscribe());
                assert!(a1.remove_device(1).await.is_err(), "not this one");
                a1.remove_device(2).await.unwrap();
                knows_devices(&bob, "alice", 1).await;
                let id = bob.send("alice", "seulement a1").unwrap();
                wait_for(&mut a1_events, message("bob", id, "seulement a1")).await;
                assert!(bob.store.queued(&a1.account(), 2).unwrap().is_empty());

                // It learns from the first of our devices it talks to.
                a2.send("bob", "je suis encore là").unwrap();
                wait_for(&mut a2_events, |e| *e == Event::Removed).await;
                assert!(a2.send("bob", "et maintenant").is_err());
                sleep(Duration::from_millis(300)).await;
                assert!(a2.history("bob", 10).unwrap().iter().all(|m| m.text != "seulement a1"));
                assert!(bob.history("alice", 10).unwrap().iter().all(|m| m.text != "je suis encore là"));
            })
            .await;
    }
}
