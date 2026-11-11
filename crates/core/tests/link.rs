//! The whole path on the in-memory transport: Alice invites, Bob accepts the
//! link, both authenticate each other, and a message goes each way.
//!
//! The prekey bundle is handed over in-process for now: its wire form is the
//! contact card, which comes with the daemon.

use std::time::{SystemTime, UNIX_EPOCH};

use libsignal_protocol::{
    CiphertextMessage, CiphertextMessageType, DeviceId, IdentityKeyPair, KeyPair, PreKeySignalMessage,
    ProtocolAddress, SignalMessage,
};
use nightpost_core::dbkey::{Secret, Unlock};
use nightpost_core::identity::{Device, DeviceList, Role, SignedDeviceList};
use nightpost_core::invite::Invite;
use nightpost_core::proto::{self, Caller, Challenge, Hello, Me, Proof, Service};
use nightpost_core::session;
use nightpost_core::store::Store;
use nightpost_core::transport::memory::Network;
use nightpost_core::transport::{Service as _, Transport as _, discovery_key, onion};
use futures::StreamExt as _;
use rand::{TryRngCore, rngs::OsRng};

struct Device1 {
    store: Store,
    key: KeyPair,
    seed: Secret,
    devices: SignedDeviceList,
}

impl Device1 {
    fn new(path: &std::path::Path, secret: u8) -> Device1 {
        let store = Store::open(path, &Unlock::new(Some(Secret::new([secret; 32])), None).unwrap()).unwrap();
        let key = KeyPair::generate(&mut OsRng.unwrap_err());
        let seed = random();
        let list = DeviceList::first(vec![Device {
            id: 1,
            onion: onion(&seed),
            key: key.public_key.serialize().into_vec(),
            role: Role::Normal,
        }]);
        let devices = SignedDeviceList::sign(&list, &store.identity().unwrap()).unwrap();
        Device1 { store, key, seed, devices }
    }

    fn identity(&self) -> IdentityKeyPair {
        self.store.identity().unwrap()
    }
}

fn random() -> Secret {
    let mut s = Secret::default();
    OsRng.try_fill_bytes(&mut s[..]).unwrap();
    s
}

/// Named after the account's identity, as the daemon names them.
fn address(identity: &libsignal_protocol::IdentityKey) -> ProtocolAddress {
    ProtocolAddress::new(identity.serialize().iter().map(|b| format!("{b:02x}")).collect(), DeviceId::new(1).unwrap())
}

fn parse(kind: u8, bytes: &[u8]) -> CiphertextMessage {
    if kind == CiphertextMessageType::PreKey as u8 {
        CiphertextMessage::PreKeySignalMessage(PreKeySignalMessage::try_from(bytes).unwrap())
    } else {
        CiphertextMessage::SignalMessage(SignalMessage::try_from(bytes).unwrap())
    }
}

#[tokio::test]
async fn invite_link_then_a_message_each_way() {
    let dir = tempfile::tempdir().unwrap();
    let alice = Device1::new(&dir.path().join("alice.db"), 1);
    let bob = Device1::new(&dir.path().join("bob.db"), 2);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let net = Network::default();
    let (alice_net, bob_net) = (net.node(), net.node());

    // Alice: a throwaway discovery key, authorized on her service, goes in the link.
    let throwaway = random();
    let (service, mut incoming) = alice_net.serve(&alice.seed, &[discovery_key(&throwaway)]).await.unwrap();
    let alice_onion = service.onion();
    let (sent, link) = Invite::create(&alice.identity(), alice_onion, *throwaway, now);
    let alice_bundle = session::bundle(&alice.store, DeviceId::new(1).unwrap()).await.unwrap();

    // Bob: the link is all he has.
    let invite = Invite::parse(&link, now).unwrap();
    bob_net.present(invite.onion, &Secret::new(invite.discovery)).unwrap();
    let mut to_alice = bob_net.dial(invite.onion).await.unwrap();
    let mut from_bob = incoming.next().await.unwrap();

    let alice_identity = alice.identity();
    let bob_identity = bob.identity();
    let alice_me = Me { account: alice_identity.identity_key(), device: 1, key: &alice.key.private_key, devices: &alice.devices };
    let bob_me = Me { account: bob_identity.identity_key(), device: 1, key: &bob.key.private_key, devices: &bob.devices };

    let bob_side = async {
        let caller = Caller::new(&bob_me, invite.onion, Some(invite.token));
        proto::send(&mut to_alice, caller.hello()).await.unwrap();
        let challenge: Challenge = proto::receive(&mut to_alice).await.unwrap();
        let (proof, peer) = caller.on_challenge(&bob_me, &challenge, &invite.identity, None).unwrap();
        proto::send(&mut to_alice, &proof).await.unwrap();
        assert_eq!(peer.account, invite.identity);

        let (b, a) = (address(bob_me.account), address(&peer.account));
        session::start(&bob.store, &b, &a, &alice_bundle).await.unwrap();
        let first = session::encrypt(&bob.store, &b, &a, b"bonjour alice").await.unwrap();
        proto::send(&mut to_alice, &(first.message_type() as u8, first.serialize().to_vec())).await.unwrap();

        let (kind, bytes): (u8, Vec<u8>) = proto::receive(&mut to_alice).await.unwrap();
        session::decrypt(&bob.store, &b, &a, &parse(kind, &bytes)).await.unwrap()
    };

    let alice_side = async {
        let hello: Hello = proto::receive(&mut from_bob).await.unwrap();
        let (pending, challenge) = Service::on_hello(&alice_me, alice_onion, hello).unwrap();
        assert_eq!(pending.hello().token, Some(sent.token));
        proto::send(&mut from_bob, &challenge).await.unwrap();
        let proof: Proof = proto::receive(&mut from_bob).await.unwrap();
        let peer = pending.on_proof(&proof, None).unwrap();
        assert_eq!(peer.account, *bob_identity.identity_key());

        let (a, b) = (address(alice_me.account), address(&peer.account));
        let (kind, bytes): (u8, Vec<u8>) = proto::receive(&mut from_bob).await.unwrap();
        let text = session::decrypt(&alice.store, &a, &b, &parse(kind, &bytes)).await.unwrap();
        let reply = session::encrypt(&alice.store, &a, &b, b"salut bob").await.unwrap();
        proto::send(&mut from_bob, &(reply.message_type() as u8, reply.serialize().to_vec())).await.unwrap();
        text
    };

    let (bob_got, alice_got) = tokio::join!(bob_side, alice_side);
    assert_eq!(alice_got, b"bonjour alice");
    assert_eq!(bob_got, b"salut bob");
}
