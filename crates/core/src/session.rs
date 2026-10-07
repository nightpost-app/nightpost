//! 1:1 sessions: PQXDH to start one, then the Triple Ratchet (Double Ratchet
//! plus SPQR, both inside libsignal) for every message.
//!
//! There is no server holding prekeys: the bundle goes straight to the other
//! side over the connection, so each one is made for a single use.

use std::time::SystemTime;

use libsignal_protocol::{
    CiphertextMessage, DeviceId, GenericSignedPreKey, IdentityKeyStore, KeyPair,
    KyberPreKeyRecord, KyberPreKeyStore, PreKeyBundle, PreKeyRecord, PreKeyStore,
    ProtocolAddress, SignalProtocolError, SignedPreKeyRecord, SignedPreKeyStore, Timestamp, kem,
    message_decrypt, message_encrypt, process_prekey_bundle,
};
use rand::{Rng, TryRngCore, rngs::OsRng};

use crate::store::{Db, Store};

type Result<T> = std::result::Result<T, SignalProtocolError>;

/// Makes a fresh prekey bundle and keeps its private halves in `store`, ready
/// for the first message the other side sends with it.
pub async fn bundle(store: &Store, device: DeviceId) -> Result<PreKeyBundle> {
    let mut rng = OsRng.unwrap_err();
    let mut db = Db(store);
    let identity = store.identity()?;
    let now = Timestamp::from_epoch_millis(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after 1970")
            .as_millis() as u64,
    );

    let pre_key = KeyPair::generate(&mut rng);
    let pre_key_id = rng.random::<u32>().into();
    db.save_pre_key(pre_key_id, &PreKeyRecord::new(pre_key_id, &pre_key))
        .await?;

    let signed = KeyPair::generate(&mut rng);
    let signed_id = rng.random::<u32>().into();
    let signed_signature = identity
        .private_key()
        .calculate_signature(&signed.public_key.serialize(), &mut rng)?;
    db.save_signed_pre_key(
        signed_id,
        &SignedPreKeyRecord::new(signed_id, now, &signed, &signed_signature),
    )
    .await?;

    let kyber = kem::KeyPair::generate(kem::KeyType::Kyber1024, &mut rng);
    let kyber_id = rng.random::<u32>().into();
    let kyber_signature = identity
        .private_key()
        .calculate_signature(&kyber.public_key.serialize(), &mut rng)?;
    db.save_kyber_pre_key(
        kyber_id,
        &KyberPreKeyRecord::new(kyber_id, now, &kyber, &kyber_signature),
    )
    .await?;

    PreKeyBundle::new(
        db.get_local_registration_id().await?,
        device,
        Some((pre_key_id, pre_key.public_key)),
        signed_id,
        signed.public_key,
        signed_signature.to_vec(),
        kyber_id,
        kyber.public_key,
        kyber_signature.to_vec(),
        *identity.identity_key(),
    )
}

/// Starts a session with `them` from the bundle they sent. The next
/// [`encrypt`] produces the PreKey message that completes it on their side.
pub async fn start(
    store: &Store,
    me: &ProtocolAddress,
    them: &ProtocolAddress,
    bundle: &PreKeyBundle,
) -> Result<()> {
    process_prekey_bundle(
        them,
        me,
        &mut Db(store),
        &mut Db(store),
        bundle,
        SystemTime::now(),
        &mut OsRng.unwrap_err(),
    )
    .await
}

pub async fn encrypt(
    store: &Store,
    me: &ProtocolAddress,
    them: &ProtocolAddress,
    plaintext: &[u8],
) -> Result<CiphertextMessage> {
    message_encrypt(
        plaintext,
        them,
        me,
        &mut Db(store),
        &mut Db(store),
        SystemTime::now(),
        &mut OsRng.unwrap_err(),
    )
    .await
}

pub async fn decrypt(
    store: &Store,
    me: &ProtocolAddress,
    them: &ProtocolAddress,
    message: &CiphertextMessage,
) -> Result<Vec<u8>> {
    message_decrypt(
        message,
        them,
        me,
        &mut Db(store),
        &mut Db(store),
        &mut Db(store),
        &Db(store),
        &mut Db(store),
        &mut OsRng.unwrap_err(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use libsignal_protocol::{CiphertextMessageType, PreKeySignalMessage};
    use zeroize::Zeroizing;

    use crate::dbkey::Unlock;

    /// A keychain secret (here just bytes, no real keychain in tests) plus,
    /// when given, a passphrase.
    fn unlock(secret: u8, passphrase: Option<&str>) -> Unlock {
        Unlock::new(
            Some(Zeroizing::new([secret; 32])),
            passphrase.map(|p| Zeroizing::new(p.as_bytes().to_vec())),
        )
        .unwrap()
    }

    fn address(name: &str) -> ProtocolAddress {
        ProtocolAddress::new(name.into(), DeviceId::new(1).unwrap())
    }

    /// Alice starts from Bob's bundle, they talk both ways, Bob reopens his
    /// database from disk and the conversation goes on; a wrong key, the
    /// keychain secret without the passphrase, and a replayed first message are
    /// all refused.
    #[test]
    fn two_identities_talk_and_survive_a_restart() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let (a, b) = (address("alice"), address("bob"));
            let alice = Store::open(&dir.path().join("alice.db"), &unlock(1, None)).unwrap();
            let bob_path = dir.path().join("bob.db");
            let bob = Store::open(&bob_path, &unlock(2, Some("bob pass"))).unwrap();

            let bundle = bundle(&bob, DeviceId::new(1).unwrap()).await.unwrap();
            start(&alice, &a, &b, &bundle).await.unwrap();

            let first = encrypt(&alice, &a, &b, b"bonjour bob").await.unwrap();
            assert_eq!(first.message_type(), CiphertextMessageType::PreKey);
            assert_eq!(decrypt(&bob, &b, &a, &first).await.unwrap(), b"bonjour bob");

            // The prekeys were one-time: the same first message again is refused.
            let replay = CiphertextMessage::PreKeySignalMessage(
                PreKeySignalMessage::try_from(first.serialize()).unwrap(),
            );
            assert!(decrypt(&bob, &b, &a, &replay).await.is_err());

            let reply = encrypt(&bob, &b, &a, b"salut alice").await.unwrap();
            assert_eq!(reply.message_type(), CiphertextMessageType::Whisper);
            assert_eq!(decrypt(&alice, &a, &b, &reply).await.unwrap(), b"salut alice");

            drop(bob);
            assert!(Store::open(&bob_path, &unlock(2, Some("wrong pass"))).is_err());
            assert!(Store::open(&bob_path, &unlock(2, None)).is_err());
            assert!(Store::open(&bob_path, &unlock(3, Some("bob pass"))).is_err());
            let bob = Store::open(&bob_path, &unlock(2, Some("bob pass"))).unwrap();

            let later = encrypt(&alice, &a, &b, b"toujours la").await.unwrap();
            assert_eq!(decrypt(&bob, &b, &a, &later).await.unwrap(), b"toujours la");
        });
    }

    /// The file on disk holds none of the plaintext the database does.
    #[test]
    fn database_file_is_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.db");
        let store = Store::open(&path, &unlock(1, Some("pass"))).unwrap();
        let identity = store.identity().unwrap().serialize();
        drop(store);
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.starts_with(b"SQLite format 3"));
        assert!(!raw.windows(identity.len()).any(|w| w == &identity[..]));
    }
}
