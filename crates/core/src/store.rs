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

/// One message as stored, handed whole to a device joining the account.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    pub account: Vec<u8>,
    pub id: [u8; 16],
    pub outgoing: bool,
    pub time: u64,
    pub text: String,
    pub delivered: bool,
    pub read: bool,
    pub ttl: u32,
    pub expires: Option<u64>,
}

pub struct Store {
    db: Connection,
    path: PathBuf,
    salt: [u8; 16],
    /// The keychain secret it was opened with, and the key in use: what
    /// changing the passphrase needs.
    secret: Option<Secret>,
    key: RefCell<Secret>,
}

/// What a mailbox keeps, at most: for 30 days, per sender account 2,000
/// messages and 20 MiB, and 1 GiB in all.
const KEEP: i64 = 30 * 24 * 3600;
const PER_SENDER_COUNT: i64 = 2000;
const PER_SENDER_BYTES: i64 = 20 << 20;
const TOTAL_BYTES: i64 = 1 << 30;

/// Where the notes to ourselves are kept in `messages`.
pub(crate) const NOTE_ACCOUNT: &[u8] = b"nightpost-note-to-self";

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
            -- A mailbox's: kept for one of the account's devices, from that
            -- device of that account.
            CREATE TABLE IF NOT EXISTS deposits (
                device INTEGER NOT NULL,
                account BLOB NOT NULL,
                sender INTEGER NOT NULL,
                id BLOB NOT NULL,
                kind INTEGER NOT NULL,
                body BLOB NOT NULL,
                time INTEGER NOT NULL,
                UNIQUE (device, id)
            );
            CREATE TABLE IF NOT EXISTS bundles (
                device INTEGER NOT NULL,
                bundle BLOB NOT NULL
            );
            PRAGMA user_version = 1;",
        )?;
        // Added before any release, so without a new schema version: for an
        // outgoing message, they read it; for an incoming one, we did.
        let has_read: bool = db.query_row("SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = 'read'", [], |row| row.get(0))?;
        if !has_read {
            db.execute_batch("ALTER TABLE messages ADD COLUMN read INTEGER NOT NULL DEFAULT 0")?;
        }

        let has_ttl: bool = db.query_row("SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = 'ttl'", [], |row| row.get(0))?;
        if !has_ttl {
            // How long a message lives once it counts (seconds, 0: for good), and
            // when it ends: ours from sending, theirs from when we read it.
            db.execute_batch("ALTER TABLE messages ADD COLUMN ttl INTEGER NOT NULL DEFAULT 0; ALTER TABLE messages ADD COLUMN expires INTEGER")?;
        }
        // What disappears is zeroed in the file, not only unlinked.
        db.execute_batch("PRAGMA secure_delete = ON")?;

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

    /// The keychain secret as 24 words to write down; `None` without one
    /// (a passphrase alone is the key).
    pub fn recovery_phrase(&self) -> Option<Zeroizing<String>> {
        self.secret.as_ref().map(crate::dbkey::recovery_phrase)
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
    /// which is how a message sent twice shows up once. To disappear `ttl` seconds on (`expires`: when,
    /// in Unix milliseconds; none yet for what we have not read).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add_timed(&self, account: &[u8], id: &[u8; 16], outgoing: bool, time: u64, text: &str, ttl: u32, expires: Option<u64>) -> rusqlite::Result<bool> {
        let added = self.db.execute(
            "INSERT OR IGNORE INTO messages (account, id, outgoing, time, text, ttl, expires) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![account, id, outgoing, time as i64, text, ttl, expires.map(|e| e as i64)],
        )?;
        Ok(added == 1)
    }

    /// Deletes what ended by `now`; returns the accounts it was in.
    /// ponytail: scans for the ends; an index on `expires` when histories get long.
    pub(crate) fn expire(&self, now: u64) -> rusqlite::Result<Vec<Vec<u8>>> {
        let mut stmt = self.db.prepare("SELECT DISTINCT account FROM messages WHERE expires IS NOT NULL AND expires <= ?1")?;
        let accounts: Vec<Vec<u8>> = stmt.query_map(params![now as i64], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?;
        self.db.execute("DELETE FROM messages WHERE expires IS NOT NULL AND expires <= ?1", params![now as i64])?;
        Ok(accounts)
    }

    /// A note to ourselves: kept as sent and delivered, under a key no contact
    /// has (theirs are public keys).
    /// False if it is already there (a copy from another of our devices).
    pub(crate) fn add_note(&self, id: &[u8; 16], time: u64, text: &str) -> rusqlite::Result<bool> {
        let added = self.db.execute(
            "INSERT OR IGNORE INTO messages (account, id, outgoing, time, text, delivered, read) VALUES (?1, ?2, 1, ?3, ?4, 1, 1)",
            params![NOTE_ACCOUNT, id, time as i64, text],
        )?;
        Ok(added == 1)
    }

    /// Whether any message is kept: a device joining an account must have none.
    pub(crate) fn has_messages(&self) -> rusqlite::Result<bool> {
        self.db.query_row("SELECT EXISTS (SELECT 1 FROM messages)", [], |row| row.get(0))
    }

    /// Every message, oldest first.
    pub(crate) fn all_messages(&self) -> rusqlite::Result<Vec<MessageRow>> {
        let mut stmt = self.db.prepare("SELECT account, id, outgoing, time, text, delivered, read, ttl, expires FROM messages ORDER BY time")?;
        stmt.query_map([], |row| {
            Ok(MessageRow {
                account: row.get(0)?,
                id: row.get(1)?,
                outgoing: row.get(2)?,
                time: row.get::<_, i64>(3)? as u64,
                text: row.get(4)?,
                delivered: row.get(5)?,
                read: row.get(6)?,
                ttl: row.get(7)?,
                expires: row.get::<_, Option<i64>>(8)?.map(|e| e as u64),
            })
        })?
        .collect()
    }

    pub(crate) fn add_rows(&self, rows: &[MessageRow]) -> rusqlite::Result<()> {
        for m in rows {
            self.db.execute(
                "INSERT OR IGNORE INTO messages (account, id, outgoing, time, text, delivered, read, ttl, expires) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![m.account, m.id, m.outgoing, m.time as i64, m.text, m.delivered, m.read, m.ttl, m.expires.map(|e| e as i64)],
            )?;
        }
        Ok(())
    }

    /// Shown as delivered: a copy of what another of our devices sent.
    pub(crate) fn delivered(&self, account: &[u8], id: &[u8; 16]) -> rusqlite::Result<()> {
        self.db.execute("UPDATE messages SET delivered = 1 WHERE account = ?1 AND id = ?2", params![account, id])?;
        Ok(())
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
    pub(crate) fn read_incoming(&self, account: &[u8], now: u64) -> rusqlite::Result<Vec<[u8; 16]>> {
        let mut stmt = self.db.prepare("SELECT id FROM messages WHERE account = ?1 AND outgoing = 0 AND read = 0 ORDER BY time")?;
        let ids: Vec<[u8; 16]> = stmt.query_map(params![account], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?;
        self.db.execute("UPDATE messages SET read = 1 WHERE account = ?1 AND outgoing = 0", params![account])?;
        // Theirs start counting once we have seen them.
        self.db.execute(
            "UPDATE messages SET expires = ?2 + ttl * 1000 WHERE account = ?1 AND outgoing = 0 AND ttl > 0 AND expires IS NULL",
            params![account, now as i64],
        )?;
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

    /// A device left `account`'s list: nothing more waits for it.
    pub(crate) fn forget_device(&self, account: &[u8], device: u8) -> rusqlite::Result<()> {
        self.db.execute("DELETE FROM outbox WHERE account = ?1 AND device = ?2", params![account, device])?;
        Ok(())
    }

    // --- a mailbox's ------------------------------------------------------

    /// Keeps what `account`'s device `sender` left for our device `device`,
    /// unless that sender is over its share or the mailbox is full. What
    /// waited longer than [`KEEP`] goes first.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn deposit(&self, device: u8, account: &[u8], sender: u8, id: &[u8; 16], kind: u8, body: &[u8], now: u64) -> rusqlite::Result<bool> {
        let now = now as i64;
        self.db.execute("DELETE FROM deposits WHERE time < ?1", params![now - KEEP])?;
        let (count, bytes): (i64, i64) = self.db.query_row(
            "SELECT COUNT(*), COALESCE(SUM(LENGTH(body)), 0) FROM deposits WHERE account = ?1",
            params![account],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let total: i64 = self.db.query_row("SELECT COALESCE(SUM(LENGTH(body)), 0) FROM deposits", [], |row| row.get(0))?;
        let size = body.len() as i64;
        if count >= PER_SENDER_COUNT || bytes + size > PER_SENDER_BYTES || total + size > TOTAL_BYTES {
            return Ok(false);
        }
        self.db.execute(
            "INSERT OR IGNORE INTO deposits (device, account, sender, id, kind, body, time) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![device, account, sender, id, kind, body, now],
        )?;
        Ok(true)
    }

    /// What waits for `device`, oldest first: (account, sender, id, kind, body).
    #[allow(clippy::type_complexity)]
    pub(crate) fn deposits(&self, device: u8) -> rusqlite::Result<Vec<(Vec<u8>, u8, [u8; 16], u8, Vec<u8>)>> {
        let mut stmt = self.db.prepare("SELECT account, sender, id, kind, body FROM deposits WHERE device = ?1 ORDER BY rowid")?;
        stmt.query_map(params![device], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?.collect()
    }

    pub(crate) fn fetched(&self, device: u8, id: &[u8; 16]) -> rusqlite::Result<()> {
        self.db.execute("DELETE FROM deposits WHERE device = ?1 AND id = ?2", params![device, id])?;
        Ok(())
    }

    pub(crate) fn add_bundles(&self, device: u8, bundles: &[Vec<u8>]) -> rusqlite::Result<()> {
        for bundle in bundles {
            self.db.execute("INSERT INTO bundles (device, bundle) VALUES (?1, ?2)", params![device, bundle])?;
        }
        Ok(())
    }

    /// One of `device`'s bundles, never handed out twice.
    pub(crate) fn take_bundle(&self, device: u8) -> rusqlite::Result<Option<Vec<u8>>> {
        let bundle: Option<(i64, Vec<u8>)> = self
            .db
            .query_row("SELECT rowid, bundle FROM bundles WHERE device = ?1 LIMIT 1", params![device], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        let Some((rowid, bundle)) = bundle else { return Ok(None) };
        self.db.execute("DELETE FROM bundles WHERE rowid = ?1", params![rowid])?;
        Ok(Some(bundle))
    }

    pub(crate) fn bundle_count(&self, device: u8) -> rusqlite::Result<i64> {
        self.db.query_row("SELECT COUNT(*) FROM bundles WHERE device = ?1", params![device], |row| row.get(0))
    }

    /// Every value kept, to look for what must never be there.
    #[cfg(test)]
    pub(crate) fn all_values(&self) -> Vec<Vec<u8>> {
        let mut stmt = self.db.prepare("SELECT value FROM kv").unwrap();
        stmt.query_map([], |row| row.get::<_, Vec<u8>>(0)).unwrap().map(|v| v.unwrap()).collect()
    }

    /// A device off the account's list: nothing is kept for it any more.
    pub(crate) fn forget_mailbox_device(&self, device: u8) -> rusqlite::Result<()> {
        self.db.execute("DELETE FROM deposits WHERE device = ?1", params![device])?;
        self.db.execute("DELETE FROM bundles WHERE device = ?1", params![device])?;
        Ok(())
    }

    /// A device joined `account`'s list: what still waits for the others
    /// waits for it too.
    pub(crate) fn copy_queue(&self, account: &[u8], device: u8) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO outbox (account, device, id, envelope)
             SELECT account, ?2, id, envelope FROM outbox WHERE account = ?1 AND device != ?2 GROUP BY id ORDER BY MIN(rowid)",
            params![account, device],
        )?;
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

    /// `device` acknowledged `id`. True if that was the first device to get
    /// one of our messages, so it is delivered: one device asleep does not
    /// keep it waiting (a read receipt is not a message: nothing to show).
    pub(crate) fn acked(&self, account: &[u8], device: u8, id: &[u8; 16]) -> rusqlite::Result<bool> {
        let removed = self.db.execute(
            "DELETE FROM outbox WHERE account = ?1 AND device = ?2 AND id = ?3",
            params![account, device, id],
        )?;
        if removed == 1 {
            let updated = self.db.execute(
                "UPDATE messages SET delivered = 1 WHERE account = ?1 AND id = ?2 AND delivered = 0",
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
        // An address is named after its account's identity: a first message
        // under another identity (handed on by a mailbox, say) is not theirs.
        Ok(address.name() == hex(&identity.serialize())
            && self
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

    #[test]
    fn a_mailbox_keeps_a_share_per_sender_for_30_days() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("mailbox.db"), &Unlock::new(Some(Secret::new([7; 32])), None).unwrap()).unwrap();
        let id = |n: i64| -> [u8; 16] { (n as u128).to_be_bytes() };
        for n in 0..PER_SENDER_COUNT {
            assert!(store.deposit(2, b"bob", 1, &id(n), 1, b"x", 1000).unwrap());
        }
        assert!(!store.deposit(2, b"bob", 1, &id(9999), 1, b"x", 1000).unwrap(), "over bob's share");
        assert!(store.deposit(2, b"carol", 1, &id(9999), 1, b"x", 1000).unwrap(), "carol has hers");
        assert!(!store.deposit(2, b"carol", 1, &[0; 16], 1, &vec![0; PER_SENDER_BYTES as usize], 1000).unwrap(), "over 20 MiB");
        // 30 days on, the old ones are gone: room again.
        assert!(store.deposit(3, b"bob", 1, &id(1), 1, b"y", 1000 + KEEP as u64 + 1).unwrap());
        assert_eq!(store.deposits(2).unwrap().len(), 0);
        assert_eq!(store.deposits(3).unwrap()[0].4, b"y");
        store.fetched(3, &id(1)).unwrap();
        assert!(store.deposits(3).unwrap().is_empty());

        store.add_bundles(2, &[vec![1], vec![2]]).unwrap();
        assert_eq!(store.bundle_count(2).unwrap(), 2);
        let (a, b) = (store.take_bundle(2).unwrap().unwrap(), store.take_bundle(2).unwrap().unwrap());
        assert_ne!(a, b);
        assert_eq!(store.take_bundle(2).unwrap(), None);
    }

    /// A database from before read receipts gets the column, its messages
    /// unread, and opening it again changes nothing.
    #[test]
    fn an_older_database_gains_the_read_column() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nightpost.db");
        let unlock = Unlock::new(Some(Secret::new([7; 32])), None).unwrap();
        let store = Store::open(&path, &unlock).unwrap();
        store.add_timed(b"bob", &[1; 16], true, 1, "avant", 0, None).unwrap();
        store.db.execute_batch("ALTER TABLE messages DROP COLUMN read").unwrap();
        drop(store);
        for _ in 0..2 {
            let store = Store::open(&path, &unlock).unwrap();
            assert_eq!(store.history(b"bob", 10).unwrap(), [([1; 16], true, 1, "avant".into(), false, false)]);
        }
    }
}
