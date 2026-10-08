//! Where the database key comes from.
//!
//! By default a random 32-byte secret kept in the OS keychain, which the OS
//! unlocks when the user logs in. A passphrase can be added on top, and then
//! both are needed. A machine without a keychain (a headless server) uses the
//! passphrase alone, read from a file rather than typed, so the daemon can
//! start unattended. The key itself is never written anywhere.

use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use hkdf::Hkdf;
use rand::{TryRngCore, rngs::OsRng};
use sha2::Sha256;
use zeroize::Zeroizing;

pub type Secret = Zeroizing<[u8; 32]>;

/// What opens the database: the keychain secret, a passphrase, or both.
pub struct Unlock {
    secret: Option<Secret>,
    passphrase: Option<Zeroizing<Vec<u8>>>,
}

impl Unlock {
    /// `None` when given neither: a database is never left without a key.
    pub fn new(secret: Option<Secret>, passphrase: Option<Zeroizing<Vec<u8>>>) -> Option<Self> {
        (secret.is_some() || passphrase.is_some()).then_some(Unlock { secret, passphrase })
    }

    pub(crate) fn secret(&self) -> Option<Secret> {
        self.secret.clone()
    }

    /// The SQLCipher key for a database whose salt is `salt`.
    pub(crate) fn key(&self, salt: &[u8; 16]) -> Secret {
        let mut ikm = Zeroizing::new(Vec::with_capacity(64));
        if let Some(secret) = &self.secret {
            ikm.extend_from_slice(&secret[..]);
        }
        if let Some(passphrase) = &self.passphrase {
            ikm.extend_from_slice(&stretch(passphrase, salt)[..]);
        }
        let mut key = Zeroizing::new([0u8; 32]);
        Hkdf::<Sha256>::new(Some(salt), &ikm)
            .expand(self.label(), &mut key[..])
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        key
    }

    /// Which inputs went in is part of the derivation, so a database made with
    /// both can never be opened by one of them, even if the other were empty.
    fn label(&self) -> &'static [u8] {
        match (self.secret.is_some(), self.passphrase.is_some()) {
            (true, true) => b"nightpost-db-v1 keychain+passphrase",
            (true, false) => b"nightpost-db-v1 keychain",
            _ => b"nightpost-db-v1 passphrase",
        }
    }
}

/// Argon2id, 64 MiB, 3 passes: about half a second on a laptop, once per start.
fn stretch(passphrase: &[u8], salt: &[u8; 16]) -> Secret {
    let params = Params::new(64 * 1024, 3, 1, Some(32)).expect("valid Argon2 parameters");
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase, salt, &mut out[..])
        .expect("Argon2 accepts a 16-byte salt and a 32-byte output");
    out
}

/// The keychain secret for the database at `label`, created on first use.
/// An error means there is no usable keychain (no Secret Service on a headless
/// Linux, for one): the caller then needs a passphrase.
pub fn keychain_secret(label: &str) -> keyring::Result<Secret> {
    // The real reason, rather than the "no default store" it turns into.
    if let Err(e) = keyring::Entry::store_status() {
        return Err(keyring::Error::Invalid("keychain".into(), e.to_string()));
    }
    let entry = keyring::Entry::new("nightpost", label)?;
    match entry.get_secret() {
        Ok(bytes) => {
            let bytes = Zeroizing::new(bytes);
            let secret: [u8; 32] = bytes[..]
                .try_into()
                .map_err(|_| keyring::Error::Invalid("secret".into(), "not 32 bytes".into()))?;
            Ok(Zeroizing::new(secret))
        }
        Err(keyring::Error::NoEntry) => {
            let mut secret = Zeroizing::new([0u8; 32]);
            OsRng.try_fill_bytes(&mut secret[..]).expect("OS random generator");
            entry.set_secret(&secret[..])?;
            Ok(secret)
        }
        Err(e) => Err(e),
    }
}

/// Present in a data directory whose database also needs the passphrase,
/// so that whatever starts the daemon knows to ask for it.
pub const MARKER: &str = "uses-passphrase";

pub fn uses_passphrase(dir: &Path) -> bool {
    dir.join(MARKER).exists()
}

/// Where an unattended daemon reads its passphrase: a systemd credential
/// (`LoadCredentialEncrypted=nightpost-passphrase`), else the file named by
/// `NIGHTPOST_PASSPHRASE_FILE`. Trailing newlines are not part of it.
pub fn passphrase_file() -> Option<PathBuf> {
    std::env::var_os("CREDENTIALS_DIRECTORY")
        .map(|dir| PathBuf::from(dir).join("nightpost-passphrase"))
        .filter(|p| p.exists())
        .or_else(|| std::env::var_os("NIGHTPOST_PASSPHRASE_FILE").map(PathBuf::from))
}

pub fn read_passphrase(path: &Path) -> std::io::Result<Zeroizing<Vec<u8>>> {
    let mut bytes = Zeroizing::new(std::fs::read(path)?);
    while bytes.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
        bytes.pop();
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(byte: u8) -> Option<Secret> {
        Some(Zeroizing::new([byte; 32]))
    }

    fn pass(s: &str) -> Option<Zeroizing<Vec<u8>>> {
        Some(Zeroizing::new(s.as_bytes().to_vec()))
    }

    #[test]
    fn every_input_changes_the_key() {
        let salt = [7u8; 16];
        let both = Unlock::new(secret(1), pass("p")).unwrap().key(&salt);
        let keychain_only = Unlock::new(secret(1), None).unwrap().key(&salt);
        let passphrase_only = Unlock::new(None, pass("p")).unwrap().key(&salt);
        let other_pass = Unlock::new(secret(1), pass("q")).unwrap().key(&salt);
        let other_salt = Unlock::new(secret(1), pass("p")).unwrap().key(&[8u8; 16]);

        assert_eq!(both, Unlock::new(secret(1), pass("p")).unwrap().key(&salt));
        for other in [keychain_only, passphrase_only, other_pass, other_salt] {
            assert_ne!(both, other);
        }
        assert!(Unlock::new(None, None).is_none());
    }
}
