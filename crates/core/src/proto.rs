//! What goes over a link: frames, and the handshake that opens it.
//!
//! A frame is a big-endian u32 length, a postcard body, and zeros up to the
//! next multiple of 512 bytes, so an observer of the stream learns sizes only
//! to the nearest 512. Nothing larger than 1 MiB is read.
//!
//! The handshake, on a stream to an onion service:
//!
//! 1. caller → `Hello { version, account, device, devices, nonce_c }`
//! 2. service → `Challenge { version, account, device, devices, nonce_s, sig_s }`
//! 3. caller → `Proof { sig_c }`
//!
//! Each side signs, with its device key, the whole transcript: role, the
//! service's onion, both nonces, both accounts and devices. The key must be in
//! the account's signed device list, which travels in the handshake and goes
//! through [`SignedDeviceList::verify`] like any other. A mailbox can thus
//! open links without holding the account identity.
//!
//! The caller checks the service is the account it dialed before signing
//! anything: answering for a different identity would let a contact relay our
//! proof elsewhere (murmure learnt this one).

use std::io;

use libsignal_protocol::{IdentityKey, PrivateKey, PublicKey};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use rand::{Rng, TryRngCore, rngs::OsRng};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::PROTOCOL_VERSION;
use crate::identity::{DeviceList, DeviceListError, SignedDeviceList};
use crate::session::WireBundle;
use crate::transport::DiscoveryKey;

pub const MAX_FRAME: usize = 1 << 20;
const PAD: usize = 512;
const HEADER: usize = 4;
const CONTEXT: &[u8] = b"nightpost-link-v1";
const SERVICE: u8 = 1;
const CALLER: u8 = 2;

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    TooLarge,
    Malformed,
}

/// The whole frame for `msg`, padding included.
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = postcard::to_stdvec(msg).map_err(|_| FrameError::Malformed)?;
    let total = (HEADER + body.len()).next_multiple_of(PAD);
    if total > MAX_FRAME {
        return Err(FrameError::TooLarge);
    }
    let mut frame = Vec::with_capacity(total);
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    frame.resize(total, 0);
    Ok(frame)
}

/// How many bytes the frame starting with `header` has in all, header
/// included: what to read before calling [`decode`].
pub fn frame_len(header: [u8; HEADER]) -> Result<usize, FrameError> {
    // Compared before adding, so a huge length cannot wrap on 32 bits.
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME - HEADER {
        return Err(FrameError::TooLarge);
    }
    Ok((HEADER + len).next_multiple_of(PAD))
}

pub fn decode<T: DeserializeOwned>(frame: &[u8]) -> Result<T, FrameError> {
    match postcard::take_from_bytes(body(frame)?) {
        Ok((msg, [])) => Ok(msg),
        _ => Err(FrameError::Malformed),
    }
}

/// What the frame holds, padding taken off.
fn body(frame: &[u8]) -> Result<&[u8], FrameError> {
    let header: [u8; HEADER] = frame
        .get(..HEADER)
        .and_then(|h| h.try_into().ok())
        .ok_or(FrameError::Malformed)?;
    if frame.len() != frame_len(header)? {
        return Err(FrameError::Malformed);
    }
    Ok(&frame[HEADER..HEADER + u32::from_be_bytes(header) as usize])
}

/// Writes `msg` as one frame.
pub async fn send<T: Serialize>(io: &mut (impl AsyncWrite + Unpin), msg: &T) -> io::Result<()> {
    io.write_all(&encode(msg).map_err(invalid)?).await?;
    io.flush().await
}

/// Reads one frame. The length is checked before anything is allocated.
pub async fn receive<T: DeserializeOwned>(io: &mut (impl AsyncRead + Unpin)) -> io::Result<T> {
    decode(&read(io).await?).map_err(invalid)
}

/// Reads a handshake frame: a `Hello`, a `Challenge`, or a service's
/// refusal, which is its version alone. Each starts with the protocol
/// version, read first since the rest may be laid out otherwise in another
/// version. `Err(theirs)`: the other side runs another version.
pub async fn receive_handshake<T: DeserializeOwned>(io: &mut (impl AsyncRead + Unpin)) -> io::Result<Result<T, u16>> {
    let frame = read(io).await?;
    let (theirs, _) = postcard::take_from_bytes::<u16>(body(&frame).map_err(invalid)?).map_err(|_| invalid(FrameError::Malformed))?;
    if theirs != PROTOCOL_VERSION {
        return Ok(Err(theirs));
    }
    decode(&frame).map(Ok).map_err(invalid)
}

async fn read(io: &mut (impl AsyncRead + Unpin)) -> io::Result<Vec<u8>> {
    let mut header = [0; HEADER];
    io.read_exact(&mut header).await?;
    let mut frame = vec![0; frame_len(header).map_err(invalid)?];
    frame[..HEADER].copy_from_slice(&header);
    io.read_exact(&mut frame[HEADER..]).await?;
    Ok(frame)
}

fn invalid(e: FrameError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{e:?}"))
}

/// What travels on a link once the handshake is done.
#[derive(Serialize, Deserialize, Debug)]
pub enum Frame {
    /// Sent by both sides right after an invitation's handshake: what the
    /// other needs to find us and to start writing to us.
    Card { discovery: DiscoveryKey, bundle: WireBundle },
    /// A libsignal message. `id` is the envelope's, in the clear so that a
    /// message which does not decrypt can still be acknowledged and dropped.
    Message { id: [u8; 16], kind: u8, body: Vec<u8> },
    /// Received and stored (or dropped for good): do not send it again.
    Ack { id: [u8; 16] },
    /// After an oral code: the inviter compared safety numbers and kept us.
    Welcome,
    /// After an oral code: we cannot add you (already contacts). Sent in
    /// place of a card, so the other side can say why instead of just
    /// seeing the connection drop.
    Refused,
    /// We have no session with your device yet: send a bundle to start one.
    GetBundle,
    /// A fresh one-time bundle for the device that sent it.
    Bundle(WireBundle),
    /// Our device list changed while the link was open.
    Devices(SignedDeviceList),
}

/// What a libsignal message holds.
#[derive(Serialize, Deserialize, Debug)]
pub struct Envelope {
    pub id: [u8; 16],
    /// Milliseconds since 1970, as the sender's clock had it.
    pub time: u64,
    pub body: Body,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Body {
    Text(String),
    /// These messages of ours were read. Sent only by someone who turned
    /// read receipts on.
    Read(Vec<[u8; 16]>),
    /// They are writing to us now. Sent only by someone who turned typing
    /// indicators on, over a link already open, never queued.
    Typing,
    /// Messages between us now disappear after this many seconds (0: they
    /// stay). Either side sets it; the last one to speak wins.
    Timer(u32),
    /// Between the devices of one account only: what one did, for the
    /// others to show the same.
    Mine(Mine),
}

/// What one of our devices tells the others. Accounts are identity keys.
#[derive(Serialize, Deserialize, Debug)]
pub enum Mine {
    /// A message it sent: the same id, so that receipts find it here too.
    Sent { to: Vec<u8>, id: [u8; 16], time: u64, text: String, ttl: u32 },
    /// A note to ourselves.
    Note { id: [u8; 16], time: u64, text: String },
    /// A contact added or renamed, as kept in the database.
    Contact(Vec<u8>),
    /// A contact removed; with `wipe`, the conversation too.
    Forget { account: Vec<u8>, wipe: bool },
    /// What this contact wrote so far was read.
    Seen(Vec<u8>),
    Timer { account: Vec<u8>, seconds: u32 },
    /// The names of our devices, by id.
    Names(Vec<(u8, String)>),
}

/// The timers a conversation can have, in seconds: off, an hour, eight
/// hours, a day, a week, four weeks.
pub const TIMERS: [u32; 6] = [0, 3_600, 28_800, 86_400, 604_800, 2_419_200];

/// Longest text message, in bytes.
pub const MAX_TEXT: usize = 64 * 1024;
/// Most messages one read receipt names.
pub const MAX_READ: usize = 512;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Hello {
    pub version: u16,
    pub account: Vec<u8>,
    pub device: u8,
    pub devices: SignedDeviceList,
    pub nonce: [u8; 32],
    /// The invitation being answered, when the caller is not a contact yet.
    pub token: Option<[u8; 16]>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Challenge {
    pub version: u16,
    pub account: Vec<u8>,
    pub device: u8,
    pub devices: SignedDeviceList,
    pub nonce: [u8; 32],
    pub signature: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Proof {
    pub signature: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HandshakeError {
    /// The other side runs another protocol version: one must update.
    Version(u16),
    /// The service is not the account that was dialed.
    WrongAccount,
    Devices(DeviceListError),
    UnknownDevice,
    BadSignature,
    Malformed,
}

/// This device, as it presents itself.
pub struct Me<'a> {
    pub account: &'a IdentityKey,
    pub device: u8,
    /// The device's signing key, listed under `device` in `devices`.
    pub key: &'a PrivateKey,
    pub devices: &'a SignedDeviceList,
}

/// The other side, once authenticated.
#[derive(Debug)]
pub struct Peer {
    pub account: IdentityKey,
    pub device: u8,
    pub devices: DeviceList,
}

/// The caller's side. `onion` is the service dialed.
pub struct Caller {
    onion: [u8; 32],
    hello: Hello,
}

impl Caller {
    pub fn new(me: &Me, onion: [u8; 32], token: Option<[u8; 16]>) -> Caller {
        let hello = Hello {
            version: PROTOCOL_VERSION,
            account: me.account.serialize().into_vec(),
            device: me.device,
            devices: me.devices.clone(),
            nonce: OsRng.unwrap_err().random(),
            token,
        };
        Caller { onion, hello }
    }

    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// Checks the service is `expected` and answers. `previous` is the device
    /// list already accepted from that account, if any.
    pub fn on_challenge(
        &self,
        me: &Me,
        challenge: &Challenge,
        expected: &IdentityKey,
        previous: Option<&SignedDeviceList>,
    ) -> Result<(Proof, Peer), HandshakeError> {
        check_version(challenge.version)?;
        if challenge.account != expected.serialize().as_ref() {
            return Err(HandshakeError::WrongAccount);
        }
        self.answer(me, challenge, previous)
    }

    /// Answers whoever holds the service, for an oral code only: there is
    /// no identity to expect yet, and the onion itself came from the code.
    /// The two people then compare safety numbers before anything is kept.
    pub fn on_challenge_by_code(&self, me: &Me, challenge: &Challenge) -> Result<(Proof, Peer), HandshakeError> {
        check_version(challenge.version)?;
        self.answer(me, challenge, None)
    }

    fn answer(&self, me: &Me, challenge: &Challenge, previous: Option<&SignedDeviceList>) -> Result<(Proof, Peer), HandshakeError> {
        let peer = authenticate(
            &challenge.account,
            challenge.device,
            &challenge.devices,
            previous,
            &transcript(SERVICE, &self.onion, &self.hello, &challenge.account, challenge.device, &challenge.nonce),
            &challenge.signature,
        )?;
        let signature = sign(
            me,
            &transcript(CALLER, &self.onion, &self.hello, &challenge.account, challenge.device, &challenge.nonce),
        );
        Ok((Proof { signature }, peer))
    }
}

/// The service's side, from a received `Hello` to an authenticated caller.
pub struct Service {
    onion: [u8; 32],
    hello: Hello,
    challenge: Challenge,
}

impl Service {
    /// Answers `hello` on the service `onion` (ours). The caller is not
    /// authenticated until [`Service::on_proof`].
    pub fn on_hello(me: &Me, onion: [u8; 32], hello: Hello) -> Result<(Service, Challenge), HandshakeError> {
        check_version(hello.version)?;
        let account = me.account.serialize().into_vec();
        let nonce = OsRng.unwrap_err().random();
        let signature = sign(me, &transcript(SERVICE, &onion, &hello, &account, me.device, &nonce));
        let challenge = Challenge {
            version: PROTOCOL_VERSION,
            account,
            device: me.device,
            devices: me.devices.clone(),
            nonce,
            signature,
        };
        Ok((Service { onion, hello, challenge: challenge.clone() }, challenge))
    }

    /// Who is calling, as the `Hello` claims: the account to look up the
    /// known device list of, and the invitation token if any.
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    pub fn on_proof(&self, proof: &Proof, previous: Option<&SignedDeviceList>) -> Result<Peer, HandshakeError> {
        let c = &self.challenge;
        authenticate(
            &self.hello.account,
            self.hello.device,
            &self.hello.devices,
            previous,
            &transcript(CALLER, &self.onion, &self.hello, &c.account, c.device, &c.nonce),
            &proof.signature,
        )
    }
}

fn check_version(theirs: u16) -> Result<(), HandshakeError> {
    if theirs == PROTOCOL_VERSION { Ok(()) } else { Err(HandshakeError::Version(theirs)) }
}

/// Everything both sides agree on once the challenge is out; `role` says who
/// signs it. Fixed-size fields first, then the one variable field (an
/// account) behind its length.
fn transcript(role: u8, onion: &[u8; 32], hello: &Hello, service: &[u8], service_device: u8, nonce_s: &[u8; 32]) -> Vec<u8> {
    let mut t = Vec::with_capacity(256);
    t.extend_from_slice(CONTEXT);
    t.push(role);
    t.extend_from_slice(onion);
    t.extend_from_slice(&hello.nonce);
    t.extend_from_slice(nonce_s);
    for (account, device) in [(&hello.account[..], hello.device), (service, service_device)] {
        t.extend_from_slice(&(account.len() as u16).to_be_bytes());
        t.extend_from_slice(account);
        t.push(device);
    }
    t
}

fn sign(me: &Me, transcript: &[u8]) -> Vec<u8> {
    me.key
        .calculate_signature(transcript, &mut OsRng.unwrap_err())
        .expect("signing with a valid key")
        .into_vec()
}

/// Accepts `devices` for `account`, then checks `signature` over
/// `transcript` comes from the key it lists for `device`.
fn authenticate(
    account: &[u8],
    device: u8,
    devices: &SignedDeviceList,
    previous: Option<&SignedDeviceList>,
    transcript: &[u8],
    signature: &[u8],
) -> Result<Peer, HandshakeError> {
    let account = IdentityKey::decode(account).map_err(|_| HandshakeError::Malformed)?;
    let devices = devices.verify(&account, previous).map_err(HandshakeError::Devices)?;
    let key = devices.device(device).ok_or(HandshakeError::UnknownDevice)?;
    let key = PublicKey::deserialize(&key.key).map_err(|_| HandshakeError::Malformed)?;
    if !key.verify_signature(transcript, signature) {
        return Err(HandshakeError::BadSignature);
    }
    Ok(Peer { account, device, devices })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Device, Role};
    use libsignal_protocol::{IdentityKeyPair, KeyPair};

    struct Account {
        identity: IdentityKeyPair,
        device_key: KeyPair,
        devices: SignedDeviceList,
    }

    impl Account {
        fn new() -> Account {
            let mut rng = OsRng.unwrap_err();
            let identity = IdentityKeyPair::generate(&mut rng);
            let device_key = KeyPair::generate(&mut rng);
            let list = DeviceList::first(vec![Device {
                id: 1,
                onion: [7; 32],
                key: device_key.public_key.serialize().into_vec(),
                role: Role::Normal,
            }]);
            let devices = SignedDeviceList::sign(&list, &identity).unwrap();
            Account { identity, device_key, devices }
        }

        fn me(&self) -> Me<'_> {
            Me { account: self.identity.identity_key(), device: 1, key: &self.device_key.private_key, devices: &self.devices }
        }
    }

    const ONION: [u8; 32] = [7; 32];

    #[test]
    fn frames_are_padded_bounded_and_exact() {
        let frame = encode(&("hello", 42u32)).unwrap();
        assert_eq!(frame.len(), PAD);
        assert_eq!(frame_len(frame[..4].try_into().unwrap()), Ok(PAD));
        assert_eq!(decode::<(String, u32)>(&frame), Ok(("hello".into(), 42)));

        let big = encode(&vec![1u8; 600]).unwrap();
        assert_eq!(big.len(), 2 * PAD);
        assert_eq!(encode(&vec![0u8; MAX_FRAME]), Err(FrameError::TooLarge));
        assert_eq!(frame_len(u32::MAX.to_be_bytes()), Err(FrameError::TooLarge));
        assert_eq!(frame_len((MAX_FRAME as u32).to_be_bytes()), Err(FrameError::TooLarge));

        // Cut short, or a body longer than what it decodes to.
        assert_eq!(decode::<(String, u32)>(&frame[..PAD - 1]), Err(FrameError::Malformed));
        assert_eq!(decode::<String>(&frame), Err(FrameError::Malformed));
    }

    #[test]
    fn both_sides_learn_who_the_other_is() {
        let (alice, bob) = (Account::new(), Account::new());
        let caller = Caller::new(&alice.me(), ONION, Some([5; 16]));
        // Over the wire and back, as the service receives it.
        let hello: Hello = decode(&encode(caller.hello()).unwrap()).unwrap();
        let (service, challenge) = Service::on_hello(&bob.me(), ONION, hello).unwrap();
        assert_eq!(service.hello().token, Some([5; 16]));
        let (proof, bob_seen) = caller.on_challenge(&alice.me(), &challenge, bob.identity.identity_key(), None).unwrap();
        let alice_seen = service.on_proof(&proof, None).unwrap();
        assert_eq!(alice_seen.account, *alice.identity.identity_key());
        assert_eq!(bob_seen.account, *bob.identity.identity_key());
        assert_eq!((alice_seen.device, bob_seen.device), (1, 1));
    }

    #[test]
    fn the_caller_refuses_a_wrong_onion_nonce_or_account() {
        let (alice, bob, carol) = (Account::new(), Account::new(), Account::new());
        let bob_id = bob.identity.identity_key();
        let caller = Caller::new(&alice.me(), ONION, None);
        let answer = |onion, hello: Hello| Service::on_hello(&bob.me(), onion, hello).unwrap().1;

        // Bob's answer signed for another onion: a relay, not the service dialed.
        let challenge = answer([8; 32], caller.hello().clone());
        let err = caller.on_challenge(&alice.me(), &challenge, bob_id, None).unwrap_err();
        assert_eq!(err, HandshakeError::BadSignature);

        // The caller's nonce changed on the way, or the service's after signing.
        let mut hello = caller.hello().clone();
        hello.nonce[0] ^= 1;
        let err = caller.on_challenge(&alice.me(), &answer(ONION, hello), bob_id, None).unwrap_err();
        assert_eq!(err, HandshakeError::BadSignature);
        let mut challenge = answer(ONION, caller.hello().clone());
        challenge.nonce[0] ^= 1;
        let err = caller.on_challenge(&alice.me(), &challenge, bob_id, None).unwrap_err();
        assert_eq!(err, HandshakeError::BadSignature);

        // Alice dialed Carol; Bob answering is refused before Alice signs.
        let challenge = answer(ONION, caller.hello().clone());
        let err = caller.on_challenge(&alice.me(), &challenge, carol.identity.identity_key(), None).unwrap_err();
        assert_eq!(err, HandshakeError::WrongAccount);

        // Bob claiming a device his list does not have.
        let mut challenge = answer(ONION, caller.hello().clone());
        challenge.device = 2;
        let err = caller.on_challenge(&alice.me(), &challenge, bob_id, None).unwrap_err();
        assert_eq!(err, HandshakeError::UnknownDevice);

        let mut challenge = answer(ONION, caller.hello().clone());
        challenge.version += 1;
        let err = caller.on_challenge(&alice.me(), &challenge, bob_id, None).unwrap_err();
        assert_eq!(err, HandshakeError::Version(PROTOCOL_VERSION + 1));
    }

    #[test]
    fn the_service_refuses_a_borrowed_account_or_a_replayed_proof() {
        let (alice, bob, carol) = (Account::new(), Account::new(), Account::new());
        let bob_id = bob.identity.identity_key();

        // Alice claims to be Carol, with Carol's (public) device list.
        let caller = Caller::new(&alice.me(), ONION, None);
        let mut hello = caller.hello().clone();
        hello.account = carol.identity.identity_key().serialize().into_vec();
        hello.devices = carol.devices.clone();
        let (service, _) = Service::on_hello(&bob.me(), ONION, hello).unwrap();
        let (proof, _) = caller
            .on_challenge(&alice.me(), &Service::on_hello(&bob.me(), ONION, caller.hello().clone()).unwrap().1, bob_id, None)
            .unwrap();
        assert_eq!(service.on_proof(&proof, None).unwrap_err(), HandshakeError::BadSignature);

        // Carol's account with Alice's list: the list is not Carol's.
        let mut hello = caller.hello().clone();
        hello.account = carol.identity.identity_key().serialize().into_vec();
        let (service, _) = Service::on_hello(&bob.me(), ONION, hello).unwrap();
        assert_eq!(
            service.on_proof(&proof, None).unwrap_err(),
            HandshakeError::Devices(DeviceListError::BadSignature)
        );

        // A proof from one session does not open another: new service nonce.
        let (service, _) = Service::on_hello(&bob.me(), ONION, caller.hello().clone()).unwrap();
        assert_eq!(service.on_proof(&proof, None).unwrap_err(), HandshakeError::BadSignature);

        let mut hello = caller.hello().clone();
        hello.version = 0;
        assert_eq!(Service::on_hello(&bob.me(), ONION, hello).err(), Some(HandshakeError::Version(0)));
    }

    /// Another version's handshake frame is told apart by its first field
    /// alone, whatever follows; so is a service's refusal.
    #[tokio::test]
    async fn another_version_is_read_before_the_rest() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        send(&mut a, &(PROTOCOL_VERSION + 1, [0xffu8; 9])).await.unwrap();
        assert_eq!(receive_handshake::<Hello>(&mut b).await.unwrap().err(), Some(PROTOCOL_VERSION + 1));
        send(&mut a, &(PROTOCOL_VERSION + 1)).await.unwrap();
        assert_eq!(receive_handshake::<Challenge>(&mut b).await.unwrap().err(), Some(PROTOCOL_VERSION + 1));
        let caller = Caller::new(&Account::new().me(), ONION, None);
        send(&mut a, caller.hello()).await.unwrap();
        assert!(receive_handshake::<Hello>(&mut b).await.unwrap().is_ok());
    }
}
