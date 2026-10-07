//! Everything Nightpost keeps on disk, in one SQLCipher database.
//!
//! The whole file is encrypted, indexes and table names included, so not even
//! the number of contacts shows. The key comes from [`Unlock`] (keychain
//! secret, passphrase, or both); its salt is the one SQLCipher writes in the
//! first 16 bytes of the file, so there is no second file to lose.
//!
//! libsignal's store traits are implemented on [`Db`], a borrowed handle: a
//! libsignal call takes the session store and the identity store as two `&mut`
//! at once, and two `Db`s over the same `Store` give it that.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use async_trait::async_trait;
use libsignal_protocol::{
    Direction, GenericSignedPreKey, IdentityChange, IdentityKey, IdentityKeyPair,
    IdentityKeyStore, KyberPreKeyId, KyberPreKeyRecord, KyberPreKeyStore, PreKeyId,
    PreKeyRecord, PreKeyStore, ProtocolAddress, PublicKey, SessionRecord, SessionStore,
    SignalProtocolError, SignedPreKeyId, SignedPreKeyRecord, SignedPreKeyStore,
};
use rand::{Rng, TryRngCore, rngs::OsRng};
use rusqlite::{Connection, OptionalExtension, params};
use zeroize::Zeroizing;

use crate::dbkey::Unlock;

type Result<T> = std::result::Result<T, SignalProtocolError>;

pub struct Store {
    db: Connection,
}

impl Store {
    /// Opens the database at `path`, creating it (and a new identity) if it
    /// does not exist. A wrong key fails here, not on first use.
    pub fn open(path: &Path, unlock: &Unlock) -> rusqlite::Result<Self> {
        let mut salt = [0u8; 16];
        let exists = match File::open(path) {
            Ok(mut f) => f.read_exact(&mut salt).is_ok(),
            Err(_) => false,
        };
        if !exists {
            OsRng.try_fill_bytes(&mut salt).expect("OS random generator");
        }

        let key = unlock.key(&salt);
        // Raw key followed by the salt: SQLCipher then skips its own PBKDF2
        // and stores this salt in the file header.
        let pragma = Zeroizing::new(format!(
            "PRAGMA key = \"x'{}{}'\"",
            hex(&key[..]),
            hex(&salt)
        ));

        let db = Connection::open(path)?;
        // SQLCipher otherwise prints every failed page decryption to stderr,
        // straight over the terminal interface. The error still comes back.
        db.execute_batch("PRAGMA cipher_log_level = NONE")?;
        db.execute_batch(&pragma)?;
        // The first statement that reads the file is where a wrong key shows.
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS kv (
                kind TEXT NOT NULL,
                key TEXT NOT NULL,
                value BLOB NOT NULL,
                PRIMARY KEY (kind, key)
            ) WITHOUT ROWID;",
        )?;

        let store = Store { db };
        if store.get("self", "identity")?.is_none() {
            let mut rng = OsRng.unwrap_err();
            let identity = IdentityKeyPair::generate(&mut rng);
            // Registration ids fit in 14 bits.
            let registration_id: u32 = rng.random_range(1..0x4000);
            // Both or neither: an identity without its registration id would
            // never open again.
            let tx = store.db.unchecked_transaction()?;
            store.put("self", "identity", &identity.serialize())?;
            store.put("self", "registration", &registration_id.to_le_bytes())?;
            tx.commit()?;
        }
        Ok(store)
    }

    pub fn identity(&self) -> Result<IdentityKeyPair> {
        let bytes = self.get("self", "identity").map_err(db_error)?;
        IdentityKeyPair::try_from(&bytes.ok_or_else(missing)?[..])
    }

    pub fn registration_id(&self) -> Result<u32> {
        let bytes = self.get("self", "registration").map_err(db_error)?;
        let bytes: [u8; 4] = bytes
            .ok_or_else(missing)?
            .try_into()
            .map_err(|_| missing())?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn get(&self, kind: &str, key: &str) -> rusqlite::Result<Option<Vec<u8>>> {
        self.db
            .query_row(
                "SELECT value FROM kv WHERE kind = ?1 AND key = ?2",
                params![kind, key],
                |row| row.get(0),
            )
            .optional()
    }

    fn put(&self, kind: &str, key: &str, value: &[u8]) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO kv (kind, key, value) VALUES (?1, ?2, ?3)",
            params![kind, key, value],
        )?;
        Ok(())
    }

    fn remove(&self, kind: &str, key: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "DELETE FROM kv WHERE kind = ?1 AND key = ?2",
            params![kind, key],
        )?;
        Ok(())
    }

    /// Reads a record that must be there: libsignal asks for a prekey by the id
    /// the other side named, and an unknown id is an invalid message.
    fn record(&self, kind: &str, key: &str) -> Result<Vec<u8>> {
        self.get(kind, key)
            .map_err(db_error)?
            .ok_or_else(|| SignalProtocolError::InvalidArgument(format!("no {kind} {key}")))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn db_error(e: rusqlite::Error) -> SignalProtocolError {
    SignalProtocolError::InvalidState("store", e.to_string())
}

fn missing() -> SignalProtocolError {
    SignalProtocolError::InvalidState("store", "own identity missing".into())
}

/// A borrowed handle on a [`Store`], on which libsignal's traits live.
#[derive(Clone, Copy)]
pub struct Db<'a>(pub &'a Store);

#[async_trait(?Send)]
impl IdentityKeyStore for Db<'_> {
    async fn get_identity_key_pair(&self) -> Result<IdentityKeyPair> {
        self.0.identity()
    }

    async fn get_local_registration_id(&self) -> Result<u32> {
        self.0.registration_id()
    }

    async fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<IdentityChange> {
        let previous = self.get_identity(address).await?;
        self.0
            .put("identity", &address.to_string(), &identity.serialize())
            .map_err(db_error)?;
        Ok(IdentityChange::from_changed(
            previous.is_some_and(|p| p != *identity),
        ))
    }

    /// Trust on first use. Contacts added with an invitation will be pinned
    /// before the first message, so for them this is never the first use.
    async fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
        _direction: Direction,
    ) -> Result<bool> {
        Ok(self
            .get_identity(address)
            .await?
            .is_none_or(|known| known == *identity))
    }

    async fn get_identity(&self, address: &ProtocolAddress) -> Result<Option<IdentityKey>> {
        self.0
            .get("identity", &address.to_string())
            .map_err(db_error)?
            .map(|bytes| IdentityKey::decode(&bytes))
            .transpose()
    }
}

#[async_trait(?Send)]
impl PreKeyStore for Db<'_> {
    async fn get_pre_key(&self, id: PreKeyId) -> Result<PreKeyRecord> {
        PreKeyRecord::deserialize(&self.0.record("prekey", &id.to_string())?)
    }

    async fn save_pre_key(&mut self, id: PreKeyId, record: &PreKeyRecord) -> Result<()> {
        self.0
            .put("prekey", &id.to_string(), &record.serialize()?)
            .map_err(db_error)
    }

    async fn remove_pre_key(&mut self, id: PreKeyId) -> Result<()> {
        self.0.remove("prekey", &id.to_string()).map_err(db_error)
    }
}

#[async_trait(?Send)]
impl SignedPreKeyStore for Db<'_> {
    async fn get_signed_pre_key(&self, id: SignedPreKeyId) -> Result<SignedPreKeyRecord> {
        SignedPreKeyRecord::deserialize(&self.0.record("signed", &id.to_string())?)
    }

    async fn save_signed_pre_key(
        &mut self,
        id: SignedPreKeyId,
        record: &SignedPreKeyRecord,
    ) -> Result<()> {
        self.0
            .put("signed", &id.to_string(), &record.serialize()?)
            .map_err(db_error)
    }
}

#[async_trait(?Send)]
impl KyberPreKeyStore for Db<'_> {
    async fn get_kyber_pre_key(&self, id: KyberPreKeyId) -> Result<KyberPreKeyRecord> {
        KyberPreKeyRecord::deserialize(&self.0.record("kyber", &id.to_string())?)
    }

    async fn save_kyber_pre_key(
        &mut self,
        id: KyberPreKeyId,
        record: &KyberPreKeyRecord,
    ) -> Result<()> {
        self.0
            .put("kyber", &id.to_string(), &record.serialize()?)
            .map_err(db_error)
    }

    /// Every Kyber prekey Nightpost hands out is one-time: delete on use.
    async fn mark_kyber_pre_key_used(
        &mut self,
        id: KyberPreKeyId,
        _ec_prekey_id: SignedPreKeyId,
        _base_key: &PublicKey,
    ) -> Result<()> {
        self.0.remove("kyber", &id.to_string()).map_err(db_error)
    }
}

#[async_trait(?Send)]
impl SessionStore for Db<'_> {
    async fn load_session(&self, address: &ProtocolAddress) -> Result<Option<SessionRecord>> {
        self.0
            .get("session", &address.to_string())
            .map_err(db_error)?
            .map(|bytes| SessionRecord::deserialize(&bytes))
            .transpose()
    }

    async fn store_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<()> {
        self.0
            .put("session", &address.to_string(), &record.serialize()?)
            .map_err(db_error)
    }
}
