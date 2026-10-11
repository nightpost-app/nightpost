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

use std::cell::RefCell;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

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

use crate::dbkey::{MARKER, Secret, Unlock};
use crate::t;

type Result<T> = std::result::Result<T, SignalProtocolError>;

/// The database layout this build reads and writes.
const SCHEMA: u32 = 1;

pub struct Store {
    db: Connection,
    path: PathBuf,
    salt: [u8; 16],
    /// The keychain secret it was opened with, and the key in use: what
    /// changing the passphrase needs.
    secret: Option<Secret>,
    key: RefCell<Secret>,
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
        // The schema version is checked before anything else is read: a
        // newer Nightpost's database is left alone, not half understood.
        let version: u32 = db.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA {
            return Err(rusqlite::Error::InvalidQuery);
        }
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS kv (
                kind TEXT NOT NULL,
                key TEXT NOT NULL,
                value BLOB NOT NULL,
                PRIMARY KEY (kind, key)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS messages (
                account BLOB NOT NULL,
                id BLOB NOT NULL,
                outgoing INTEGER NOT NULL,
                time INTEGER NOT NULL,
                text TEXT NOT NULL,
                delivered INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (account, id)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS outbox (
                account BLOB NOT NULL,
                device INTEGER NOT NULL,
                id BLOB NOT NULL,
                envelope BLOB NOT NULL,
                UNIQUE (account, device, id)
            );
            PRAGMA user_version = 1;",
        )?;
        // Added before any release, so without a new schema version: for an
        // outgoing message, they read it; for an incoming one, we did.
        let has_read: bool = db.query_row("SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = 'read'", [], |row| row.get(0))?;
        if !has_read {
            db.execute_batch("ALTER TABLE messages ADD COLUMN read INTEGER NOT NULL DEFAULT 0")?;
        }

        let store = Store { db, path: path.to_path_buf(), salt, secret: unlock.secret(), key: RefCell::new(key) };
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

    /// Encrypts the database again, for the same keychain secret with `new`
    /// rather than `current` as the passphrase (`None`: no passphrase), once
    /// `current` is shown to be the one in use. The marker next to the
    /// database tells the interfaces whether to ask for it at start.
    pub fn set_passphrase(&self, current: Option<Zeroizing<Vec<u8>>>, new: Option<Zeroizing<Vec<u8>>>) -> anyhow::Result<()> {
        // Without a keychain the passphrase comes from a file, and is the only key.
        anyhow::ensure!(self.secret.is_some(), t!("err-passphrase-keychain"));
        anyhow::ensure!(new.as_ref().is_none_or(|p| !p.is_empty()), t!("err-passphrase-empty"));
        let current = Unlock::new(self.secret.clone(), current).expect("has a secret").key(&self.salt);
        anyhow::ensure!(current == *self.key.borrow(), t!("err-passphrase-wrong"));
        let adding = new.is_some();
        let key = Unlock::new(self.secret.clone(), new).expect("has a secret").key(&self.salt);
        let pragma = Zeroizing::new(format!("PRAGMA rekey = \"x'{}{}'\"", hex(&key[..]), hex(&self.salt)));
        self.db.execute_batch(&pragma)?;
        *self.key.borrow_mut() = key;
        let marker = self.path.with_file_name(MARKER);
        if adding {
            File::create(marker)?;
        } else if marker.exists() {
            std::fs::remove_file(marker)?;
        }
        Ok(())
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

    /// The user's own Tor bridge lines; none means the built-in ones.
    pub fn bridges(&self) -> rusqlite::Result<Vec<String>> {
        let text = self.get("self", "bridges")?.map(String::from_utf8).transpose().ok().flatten().unwrap_or_default();
        Ok(text.lines().map(String::from).collect())
    }

    pub fn set_bridges(&self, lines: &[String]) -> rusqlite::Result<()> {
        self.put("self", "bridges", lines.join("\n").as_bytes())
    }

    pub(crate) fn get(&self, kind: &str, key: &str) -> rusqlite::Result<Option<Vec<u8>>> {
        self.db
            .query_row(
                "SELECT value FROM kv WHERE kind = ?1 AND key = ?2",
                params![kind, key],
                |row| row.get(0),
            )
            .optional()
    }

    pub(crate) fn put(&self, kind: &str, key: &str, value: &[u8]) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO kv (kind, key, value) VALUES (?1, ?2, ?3)",
            params![kind, key, value],
        )?;
        Ok(())
    }

    pub(crate) fn remove(&self, kind: &str, key: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "DELETE FROM kv WHERE kind = ?1 AND key = ?2",
            params![kind, key],
        )?;
        Ok(())
    }

    pub(crate) fn list(&self, kind: &str) -> rusqlite::Result<Vec<(String, Vec<u8>)>> {
        let mut stmt = self.db.prepare("SELECT key, value FROM kv WHERE kind = ?1")?;
        stmt.query_map(params![kind], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect()
    }

    /// Runs `f` in one transaction: all of its writes or none.
    pub(crate) fn atomically<R>(&self, f: impl FnOnce() -> rusqlite::Result<R>) -> rusqlite::Result<R> {
        let tx = self.db.unchecked_transaction()?;
        let result = f()?;
        tx.commit()?;
        Ok(result)
    }

    /// Records a message; false if this id is already there from `account`,
    /// which is how a message sent twice shows up once.
    pub(crate) fn add_message(&self, account: &[u8], id: &[u8; 16], outgoing: bool, time: u64, text: &str) -> rusqlite::Result<bool> {
        let added = self.db.execute(
            "INSERT OR IGNORE INTO messages (account, id, outgoing, time, text) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![account, id, outgoing, time as i64, text],
        )?;
        Ok(added == 1)
    }

    /// The last `limit` messages with `account`, oldest first.
    /// ponytail: scans the account's rows to sort by time; an index on
    /// (account, time) when histories get long.
    #[allow(clippy::type_complexity)]
    pub(crate) fn history(&self, account: &[u8], limit: u32) -> rusqlite::Result<Vec<([u8; 16], bool, u64, String, bool, bool)>> {
        let mut stmt = self.db.prepare(
            "SELECT id, outgoing, time, text, delivered, read FROM messages WHERE account = ?1 ORDER BY time DESC LIMIT ?2",
        )?;
        let mut rows: Vec<_> = stmt
            .query_map(params![account, limit], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? as u64, row.get(3)?, row.get(4)?, row.get(5)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// Marks what `account` sent us as read; returns what was not yet.
    pub(crate) fn read_incoming(&self, account: &[u8]) -> rusqlite::Result<Vec<[u8; 16]>> {
        let mut stmt = self.db.prepare("SELECT id FROM messages WHERE account = ?1 AND outgoing = 0 AND read = 0 ORDER BY time")?;
        let ids: Vec<[u8; 16]> = stmt.query_map(params![account], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?;
        self.db.execute("UPDATE messages SET read = 1 WHERE account = ?1 AND outgoing = 0", params![account])?;
        Ok(ids)
    }

    /// `account` read these messages of ours; returns those not known as
    /// read before. Ids that are not ours to them are ignored.
    pub(crate) fn read_outgoing(&self, account: &[u8], ids: &[[u8; 16]]) -> rusqlite::Result<Vec<[u8; 16]>> {
        let mut changed = Vec::new();
        for id in ids {
            let n = self.db.execute(
                "UPDATE messages SET read = 1 WHERE account = ?1 AND id = ?2 AND outgoing = 1 AND read = 0",
                params![account, id],
            )?;
            if n == 1 {
                changed.push(*id);
            }
        }
        Ok(changed)
    }

    /// Drops what still waits for `account`, and with `wipe` the
    /// conversation too.
    pub(crate) fn forget(&self, account: &[u8], wipe: bool) -> rusqlite::Result<()> {
        self.db.execute("DELETE FROM outbox WHERE account = ?1", params![account])?;
        if wipe {
            self.db.execute("DELETE FROM messages WHERE account = ?1", params![account])?;
        }
        Ok(())
    }

    pub(crate) fn queue(&self, account: &[u8], device: u8, id: &[u8; 16], envelope: &[u8]) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO outbox (account, device, id, envelope) VALUES (?1, ?2, ?3, ?4)",
            params![account, device, id, envelope],
        )?;
        Ok(())
    }

    /// What still waits for `account`'s `device`, oldest first.
    pub(crate) fn queued(&self, account: &[u8], device: u8) -> rusqlite::Result<Vec<([u8; 16], Vec<u8>)>> {
        let mut stmt = self.db.prepare(
            "SELECT id, envelope FROM outbox WHERE account = ?1 AND device = ?2 ORDER BY rowid",
        )?;
        stmt.query_map(params![account, device], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect()
    }

    /// Every (account, device) something waits for.
    pub(crate) fn queued_for(&self) -> rusqlite::Result<Vec<(Vec<u8>, u8)>> {
        let mut stmt = self.db.prepare("SELECT DISTINCT account, device FROM outbox")?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
    }

    /// `device` acknowledged `id`. True if that was the last device waiting
    /// for one of our messages, so it is delivered (a read receipt is not a
    /// message: nothing to show).
    pub(crate) fn acked(&self, account: &[u8], device: u8, id: &[u8; 16]) -> rusqlite::Result<bool> {
        let removed = self.db.execute(
            "DELETE FROM outbox WHERE account = ?1 AND device = ?2 AND id = ?3",
            params![account, device, id],
        )?;
        let left: u32 = self.db.query_row(
            "SELECT COUNT(*) FROM outbox WHERE account = ?1 AND id = ?2",
            params![account, id],
            |row| row.get(0),
        )?;
        if removed == 1 && left == 0 {
            let updated = self.db.execute(
                "UPDATE messages SET delivered = 1 WHERE account = ?1 AND id = ?2",
                params![account, id],
            )?;
            return Ok(updated == 1);
        }
        Ok(false)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbkey::{Secret, Unlock};

    /// A database from before read receipts gets the column, its messages
    /// unread, and opening it again changes nothing.
    #[test]
    fn an_older_database_gains_the_read_column() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nightpost.db");
        let unlock = Unlock::new(Some(Secret::new([7; 32])), None).unwrap();
        let store = Store::open(&path, &unlock).unwrap();
        store.add_message(b"bob", &[1; 16], true, 1, "avant").unwrap();
        store.db.execute_batch("ALTER TABLE messages DROP COLUMN read").unwrap();
        drop(store);
        for _ in 0..2 {
            let store = Store::open(&path, &unlock).unwrap();
            assert_eq!(store.history(b"bob", 10).unwrap(), [([1; 16], true, 1, "avant".into(), false, false)]);
        }
    }
}
