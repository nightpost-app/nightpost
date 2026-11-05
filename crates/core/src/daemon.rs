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

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail, ensure};
use futures::StreamExt as _;
use libsignal_protocol::{
    CiphertextMessage, CiphertextMessageType, DeviceId, IdentityKey, IdentityKeyPair, KeyPair, PreKeyBundle, PreKeySignalMessage,
    PrivateKey, ProtocolAddress, SignalMessage,
};
use rand::{Rng as _, TryRngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::{Mutex, Notify, broadcast, mpsc, oneshot, watch};
use tokio::task::spawn_local;
use tokio::time::{sleep, timeout};
use zeroize::Zeroizing;

use crate::api::{ContactInfo, Event, HistoryItem, LinkState, MAX_PHOTO, Network, Person, hex};
use crate::dbkey::Secret;
use crate::identity::{Device, DeviceList, Role, SignedDeviceList, safety_number};
use crate::invite::{self, Code, CodeError, Invite, InviteError};
use crate::proto::{self, Body, Caller, Challenge, Envelope, Frame, Hello, MAX_READ, MAX_TEXT, Me, Peer, Proof, Service, TIMERS};
use crate::session::{self, WireBundle};
use crate::store::{NOTE_ACCOUNT, Store};
use crate::{PROTOCOL_VERSION, t};
use crate::transport::{DiscoveryKey, Onion, Service as _, Transport, discovery_key, onion};

/// v1 has one device per account.
const DEVICE: u8 = 1;
/// A handshake step that takes longer than this is abandoned.
const STEP: Duration = Duration::from_secs(60);
/// A link with nothing to say for this long is closed.
const IDLE: Duration = Duration::from_secs(60);
/// How long accepting an invitation keeps trying: a new descriptor can take
/// minutes to reach the directories.
const ACCEPT_DEADLINE: Duration = if cfg!(test) { Duration::from_secs(1) } else { Duration::from_secs(300) };
const MAX_NAME: usize = 32;
/// An oral code, and the wait for its confirmation.
const CODE_LIFETIME: Duration = Duration::from_secs(600);
/// First wait after a failed dial; it doubles up to 10 minutes.
const RETRY: Duration = if cfg!(test) { Duration::from_millis(50) } else { Duration::from_secs(10) };

pub struct Daemon<T: Transport> {
    store: Rc<Store>,
    net: T,
    identity: IdentityKeyPair,
    own: Own,
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
    /// By contact name, for interfaces to show who is reachable.
    link_states: RefCell<HashMap<String, LinkState>>,
}

type Target = (Vec<u8>, u8);
/// Wakes an open link: `None` to send what is queued, `Some` an envelope to
/// send at once, never queued (a typing indicator).
type Wake = mpsc::UnboundedSender<Option<Vec<u8>>>;

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
struct Contact {
    name: String,
    identity: Vec<u8>,
    devices: SignedDeviceList,
    discovery: DiscoveryKey,
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
        Ok(Rc::new(Daemon {
            store,
            net,
            identity,
            own,
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
        }))
    }

    /// Brings the service up for the current contacts and starts delivering.
    /// Must run inside a `LocalSet`.
    pub async fn start(self: &Rc<Self>) -> Result<()> {
        for contact in self.contacts_raw()? {
            self.present_to(&contact)?;
        }
        self.refresh_service().await?;
        spawn_local(self.clone().deliver());
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
            self.queue(&contact, &devices, &id, &envelope)
        })?;
        let _ = self.events.send(Event::TimerChanged { name: name.into(), seconds });
        self.nudge(&contact, &devices);
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
            }
            let _ = self.events.send(Event::Network { network });
        }
    }

    pub fn links(&self) -> Vec<(String, LinkState)> {
        self.link_states.borrow().iter().map(|(name, state)| (name.clone(), *state)).collect()
    }

    fn set_link(&self, name: &str, state: LinkState) {
        if self.link_states.borrow_mut().insert(name.into(), state) != Some(state) {
            let _ = self.events.send(Event::Link { name: name.into(), state });
        }
    }

    /// A one-time link for someone who will be known here as `name`.
    pub async fn invite(self: &Rc<Self>, name: &str) -> Result<String> {
        self.check_name(name)?;
        let throwaway: [u8; 32] = OsRng.unwrap_err().random();
        let (invite, link) =
            Invite::create(&self.identity, onion(&self.seed()), throwaway, now_secs());
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
        let me = self.me();
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
        let me = self.me();
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

        let me = self.me();
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
        if self.is_note(to)? {
            let id: [u8; 16] = OsRng.unwrap_err().random();
            let time = now_millis();
            self.store.add_note(&id, time, text)?;
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
            self.queue(&contact, &devices, &id, &envelope)
        })?;
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

    /// Something was queued: open links send it now, the rest get dialed.
    fn nudge(&self, contact: &Contact, devices: &DeviceList) {
        for device in &devices.devices {
            if let Some(Some(link)) = self.links.borrow().get(&(contact.identity.clone(), device.id)) {
                let _ = link.send(None);
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
                    let _ = link.send(Some(envelope.clone()));
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
        self.store.put("contact", &hex(&contact.identity), &postcard::to_stdvec(&contact)?)?;
        let state = self.link_states.borrow_mut().remove(name);
        if let Some(state) = state {
            self.link_states.borrow_mut().insert(to.into(), state);
        }
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
            self.store.forget(&contact.identity, wipe)
        })?;
        self.link_states.borrow_mut().remove(name);
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
        let me = self.me();
        let hello = hello(&mut stream).await?;
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
        let bundle = WireBundle::new(&session::bundle(&self.store, device_id(DEVICE)?).await?)?;
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
        self.store.put("contact", &hex(&contact.identity), &postcard::to_stdvec(contact)?)?;
        self.present_to(contact)?;
        self.refresh_service().await?;
        let _ = self.events.send(Event::ContactAdded { name: contact.name.clone() });
        Ok(())
    }

    // --- delivery ----------------------------------------------------------

    /// Opens a link to every device something is queued for, backing off
    /// after failures.
    async fn deliver(self: Rc<Self>) {
        loop {
            match self.store.queued_for() {
                Ok(targets) => {
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
        let name = self.contact(&target.0).ok().flatten().map(|c| c.name);
        if let Some(name) = &name {
            self.set_link(name, LinkState::Trying);
        }
        match self.try_dial(&target).await {
            Ok((stream, contact)) => {
                self.backoff.borrow_mut().remove(&target);
                self.clone().link(stream, contact, target.1).await;
            }
            Err(e) => {
                tracing::info!("no link to a contact: {e:#}");
                if let Some(name) = &name {
                    let state = match e.downcast_ref::<OtherVersion>() {
                        Some(OtherVersion(theirs)) if *theirs > PROTOCOL_VERSION => LinkState::Newer,
                        Some(_) => LinkState::Older,
                        None => LinkState::Unreachable,
                    };
                    self.set_link(name, state);
                }
                self.links.borrow_mut().remove(&target);
                let mut backoff = self.backoff.borrow_mut();
                let failures = backoff.get(&target).map_or(0, |(n, _)| *n) + 1;
                let wait = (RETRY * (1 << failures.min(6))).min(Duration::from_secs(600));
                backoff.insert(target, (failures, Instant::now() + wait));
            }
        }
    }

    async fn try_dial(self: &Rc<Self>, (account, device): &Target) -> Result<(T::Stream, Contact)> {
        let mut contact = self.contact(account)?.context("no longer a contact")?;
        let to: Onion = self.devices_of(&contact)?.device(*device).context("device no longer listed")?.onion;
        let mut stream = timeout(STEP * 2, self.net.dial(to)).await.context("dial timed out")??;
        let me = self.me();
        let caller = Caller::new(&me, to, None);
        proto::send(&mut stream, caller.hello()).await?;
        let challenge: Challenge = challenge(&mut stream).await?;
        let expected = IdentityKey::decode(account)?;
        let (proof, _) = caller.on_challenge(&me, &challenge, &expected, Some(&contact.devices)).map_err(handshake)?;
        proto::send(&mut stream, &proof).await?;
        self.update_devices(&mut contact, &challenge.devices)?;
        Ok((stream, contact))
    }

    /// Runs an authenticated link: sends what is queued for that device,
    /// stores and acknowledges what comes in, until it goes quiet or breaks.
    async fn link(self: Rc<Self>, stream: T::Stream, contact: Contact, device: u8) {
        let target = (contact.identity.clone(), device);
        let (wake, mut woken) = mpsc::unbounded_channel();
        self.links.borrow_mut().insert(target.clone(), Some(wake));
        self.set_link(&contact.name, LinkState::Open);
        if let Err(e) = self.run_link(stream, &contact, device, &mut woken).await {
            tracing::info!("link to a contact closed: {e:#}");
        }
        self.links.borrow_mut().remove(&target);
        // By the name they have now; nothing for someone just removed.
        if let Ok(Some(now)) = self.contact(&contact.identity) {
            self.set_link(&now.name, LinkState::Idle);
        }
        // Whatever was in flight goes again on the next link.
        self.wake.notify_one();
    }

    async fn run_link(
        &self,
        stream: T::Stream,
        contact: &Contact,
        device: u8,
        woken: &mut mpsc::UnboundedReceiver<Option<Vec<u8>>>,
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
            loop {
                // Renamed since, or removed: then the link ends here.
                let contact = &self.contact(&contact.identity)?.context("no longer a contact")?;
                for (id, envelope) in self.store.queued(&contact.identity, device)? {
                    if in_flight.insert(id) {
                        let message = session::encrypt(&self.store, &me, &them, &envelope).await?;
                        let frame = Frame::Message { id, kind: message.message_type() as u8, body: message.serialize().to_vec() };
                        proto::send(&mut writer, &frame).await?;
                    }
                }
                tokio::select! {
                    frame = frames.recv() => match frame {
                        None => return Ok(()),
                        Some(Frame::Message { id, kind, body }) => {
                            // Not kept from someone removed while we waited.
                            let contact = &self.contact(&contact.identity)?.context("no longer a contact")?;
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
                        Some(Frame::Card { .. } | Frame::Welcome | Frame::Refused) => bail!("an invitation frame on a contact link"),
                    },
                    Some(now) = woken.recv() => {
                        // Not queued: nothing waits for its acknowledgement.
                        if let Some(envelope) = now {
                            let id: Envelope = postcard::from_bytes(&envelope)?;
                            let message = session::encrypt(&self.store, &me, &them, &envelope).await?;
                            let frame = Frame::Message { id: id.id, kind: message.message_type() as u8, body: message.serialize().to_vec() };
                            proto::send(&mut writer, &frame).await?;
                        }
                    }
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
    async fn receive(&self, contact: &Contact, them: &ProtocolAddress, id: [u8; 16], kind: u8, body: &[u8]) -> Result<Option<Event>> {
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
        let text = match envelope.body {
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
                return Ok(Some(Event::TimerChanged { name: contact.name.clone(), seconds }));
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

    // --- helpers -----------------------------------------------------------

    fn me(&self) -> Me<'_> {
        Me { account: self.identity.identity_key(), device: DEVICE, key: &self.signing, devices: &self.own.devices }
    }

    fn safety_number(&self, contact: &Contact) -> Result<String> {
        safety_number(self.identity.identity_key(), &IdentityKey::decode(&contact.identity)?).map_err(|e| anyhow::anyhow!("{e:?}"))
    }

    fn seed(&self) -> Secret {
        Secret::new(self.own.seed)
    }

    fn discovery(&self) -> Secret {
        Secret::new(self.own.discovery)
    }

    fn address(&self) -> ProtocolAddress {
        address(&self.identity.identity_key().serialize(), DEVICE).expect("valid own address")
    }

    fn contact(&self, account: &[u8]) -> Result<Option<Contact>> {
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

    /// Records a newer device list the contact presented, already verified
    /// as the successor of the one we had by the handshake.
    fn update_devices(&self, contact: &mut Contact, devices: &SignedDeviceList) -> Result<()> {
        if contact.devices != *devices {
            contact.devices = devices.clone();
            self.store.put("contact", &hex(&contact.identity), &postcard::to_stdvec(&*contact)?)?;
            self.present_to(contact)?;
        }
        Ok(())
    }

    /// Files our discovery key for each of the contact's devices.
    fn present_to(&self, contact: &Contact) -> Result<()> {
        for device in self.devices_of(contact)?.devices {
            self.net.present(device.onion, &self.discovery())?;
        }
        Ok(())
    }

    fn check_name(&self, name: &str) -> Result<()> {
        ensure!(
            !name.trim().is_empty() && name.chars().count() <= MAX_NAME && !name.chars().any(char::is_control),
            "a name is 1 to {MAX_NAME} characters, without control characters"
        );
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
struct OtherVersion(u16);

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
async fn hello(stream: &mut (impl AsyncRead + AsyncWrite + Unpin)) -> Result<Hello> {
    match timeout(STEP, proto::receive_handshake(stream)).await?? {
        Ok(hello) => Ok(hello),
        Err(theirs) => {
            proto::send(stream, &PROTOCOL_VERSION).await?;
            Err(OtherVersion(theirs).into())
        }
    }
}

fn handshake(e: proto::HandshakeError) -> anyhow::Error {
    anyhow::anyhow!("handshake: {e:?}")
}

fn now_secs() -> u64 {
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
                assert!(alice.store.history(&bob.identity.identity_key().serialize(), 10).unwrap().is_empty());
                // Bob can no longer reach Alice's service, so his messages
                // no longer get through.
                assert!(bob.net.dial(onion(&alice.seed())).await.is_err());
                bob.send("alice", "tu es là ?").unwrap();
                sleep(Duration::from_millis(500)).await;
                assert!(alice.store.history(&bob.identity.identity_key().serialize(), 10).unwrap().is_empty());
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
                assert_eq!(next(&mut alice_events).await, Event::TimerChanged { name: "bob".into(), seconds: 3_600 });
                assert_eq!(next(&mut bob_events).await, Event::TimerChanged { name: "alice".into(), seconds: 3_600 });
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
}
