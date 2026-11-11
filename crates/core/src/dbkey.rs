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
use sha2::{Digest as _, Sha256};
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

    pub fn secret(&self) -> Option<Secret> {
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

/// One keychain entry per data directory, so test instances do not share.
pub fn keychain_label(dir: &Path) -> String {
    let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// The keychain secret for the database at `label`. With `create`, a missing
/// one is made (the first start); without, `None` says it is missing: a
/// database made earlier then cannot be opened with a new secret, which is
/// what happens when another keychain answers (another desktop), and must
/// not be hidden by making one.
/// An error means there is no usable keychain (no Secret Service on a headless
/// Linux, for one): the caller then needs a passphrase.
pub fn keychain_secret(label: &str, create: bool) -> keyring::Result<Option<Secret>> {
    keychain()?;
    let entry = keyring::Entry::new("nightpost", label)?;
    match entry.get_secret() {
        Ok(bytes) => {
            let bytes = Zeroizing::new(bytes);
            let secret: [u8; 32] = bytes[..]
                .try_into()
                .map_err(|_| keyring::Error::Invalid("secret".into(), "not 32 bytes".into()))?;
            Ok(Some(Zeroizing::new(secret)))
        }
        Err(keyring::Error::NoEntry) if create => {
            let mut secret = Zeroizing::new([0u8; 32]);
            OsRng.try_fill_bytes(&mut secret[..]).expect("OS random generator");
            entry.set_secret(&secret[..])?;
            Ok(Some(secret))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether this computer's keychain may be used. Not from a portable key:
/// the computer is borrowed, its keychain is not ours to read or fill, and
/// the passphrase alone opens the database.
fn keychain() -> keyring::Result<()> {
    if crate::portable() {
        return Err(keyring::Error::Invalid("keychain".into(), "not used from a portable key".into()));
    }
    // The real reason, rather than the "no default store" it turns into.
    if let Err(e) = keyring::Entry::store_status() {
        return Err(keyring::Error::Invalid("keychain".into(), e.to_string()));
    }
    Ok(())
}

/// Puts `secret` in this keychain, in place of what is there: after it was
/// recovered from its words, so that the next start finds it.
pub fn store_keychain_secret(label: &str, secret: &Secret) -> keyring::Result<()> {
    keychain()?;
    keyring::Entry::new("nightpost", label)?.set_secret(&secret[..])
}

/// The secret as 24 words (BIP-39, English): to write down once, and to
/// type when the keychain does not have it (another desktop, a new
/// computer). The last word carries a checksum.
pub fn recovery_phrase(secret: &Secret) -> Zeroizing<String> {
    Zeroizing::new(bip39::Mnemonic::from_entropy(&secret[..]).expect("32 bytes make 24 words").to_string())
}

/// The secret from its 24 words, as typed: case and spacing do not matter,
/// a wrong or swapped word is refused by the checksum.
pub fn secret_from_phrase(phrase: &str) -> anyhow::Result<Secret> {
    let bad = || anyhow::anyhow!(crate::t!("err-recovery-phrase"));
    let mnemonic = bip39::Mnemonic::parse(phrase.trim().to_lowercase()).map_err(|_| bad())?;
    let bytes = Zeroizing::new(mnemonic.to_entropy());
    let secret: [u8; 32] = bytes[..].try_into().map_err(|_| bad())?;
    Ok(Zeroizing::new(secret))
}

/// Present in a data directory whose database also needs the passphrase,
/// so that whatever starts the daemon knows to ask for it.
pub const MARKER: &str = "uses-passphrase";

pub fn uses_passphrase(dir: &Path) -> bool {
    crate::portable() || dir.join(MARKER).exists()
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

    #[test]
    fn the_secret_comes_back_from_its_24_words() {
        let secret = Zeroizing::new([0x5au8; 32]);
        let phrase = recovery_phrase(&secret);
        assert_eq!(phrase.split(' ').count(), 24);
        assert_eq!(*secret_from_phrase(&phrase).unwrap(), *secret);
        // As typed: capitals, extra spaces, a line break.
        let typed = format!("  {}\n", phrase.to_uppercase().replace(' ', "  "));
        assert_eq!(*secret_from_phrase(&typed).unwrap(), *secret);
        // A word changed, one cut off, nothing.
        let words: Vec<&str> = phrase.split(' ').collect();
        let other = if words[0] == "abandon" { "ability" } else { "abandon" };
        assert!(secret_from_phrase(&[&[other], &words[1..]].concat().join(" ")).is_err());
        assert!(secret_from_phrase(&words[1..].join(" ")).is_err());
        assert!(secret_from_phrase("").is_err());
        // Twelve words are a valid BIP-39 phrase, but not our secret's size.
        let short = bip39::Mnemonic::from_entropy(&[1u8; 16]).unwrap().to_string();
        assert!(secret_from_phrase(&short).is_err());
    }
}
