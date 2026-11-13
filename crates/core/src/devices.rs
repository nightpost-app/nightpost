//! Adding a device to an account. The new device shows a link (and its QR
//! code); a device already in the account uses it, dials the new device's
//! temporary onion, and hands it the account.
//!
//! That way round, a link that leaks gives the account to nobody: whoever
//! uses it only reaches the new device, which holds that onion's key.

use data_encoding::BASE32_NOPAD;
use rand::{Rng as _, TryRngCore as _, rngs::OsRng};
use serde::{Deserialize, Serialize};

use crate::identity::SignedDeviceList;
use crate::store::MessageRow;

const PREFIX: &str = "nightpost:device:";
/// A link long enough to type, too short to be the whole account.
const MAX_LINK: usize = 256;
/// How long the new device waits for its link to be used.
pub const PAIRING_LIFETIME: u64 = 600;
/// Clocks a little apart: a link from a few minutes "ahead" still works.
const CLOCK_SLACK: u64 = 300;

/// What the link holds: how to reach the new device, once.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    /// The new device's temporary onion service.
    pub onion: [u8; 32],
    /// The secret half of the throwaway key that service lets in.
    pub discovery: [u8; 32],
    pub token: [u8; 16],
    pub expires: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PairingError {
    NotALink,
    Malformed,
    Expired,
}

impl Pairing {
    pub fn new(onion: [u8; 32], discovery: [u8; 32], now: u64) -> Self {
        Pairing { onion, discovery, token: OsRng.unwrap_err().random(), expires: now + PAIRING_LIFETIME }
    }

    pub fn link(&self) -> String {
        format!("{PREFIX}{}", BASE32_NOPAD.encode(&postcard::to_stdvec(self).expect("fixed-size fields")))
    }

    pub fn parse(link: &str, now: u64) -> Result<Self, PairingError> {
        let link = link.trim();
        if link.len() > MAX_LINK || !link.to_ascii_lowercase().starts_with(PREFIX) {
            return Err(PairingError::NotALink);
        }
        let bytes = BASE32_NOPAD.decode(link[PREFIX.len()..].to_ascii_uppercase().as_bytes()).map_err(|_| PairingError::Malformed)?;
        let pairing: Pairing = postcard::from_bytes(&bytes).map_err(|_| PairingError::Malformed)?;
        if pairing.expires <= now || pairing.expires > now + PAIRING_LIFETIME + CLOCK_SLACK {
            return Err(PairingError::Expired);
        }
        Ok(pairing)
    }
}

/// What the two devices say to each other, in this order: `Hello` from the
/// device of the account, `Offer` back, then `Account`, `Messages`…,
/// `Done`, and `Ok` once the new device kept it all.
#[derive(Serialize, Deserialize, Debug)]
pub enum Step {
    Hello { version: u16, token: [u8; 16] },
    /// The new device: how it is reached and signs, and what it is.
    Offer { key: Vec<u8>, onion: [u8; 32], name: String, mailbox: bool },
    Account(Account),
    /// The history, a part at a time: a frame holds at most 1 MiB.
    Messages(Vec<MessageRow>),
    Done,
    Ok,
    /// Turned down, and why (in the new device's words).
    Refused(String),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Account {
    /// The identity key pair; for a mailbox, the public key alone.
    pub identity: Vec<u8>,
    /// The account's discovery key (secret); for a mailbox, its public half.
    pub discovery: [u8; 32],
    /// The id the new device has in `devices`.
    pub device: u8,
    pub devices: SignedDeviceList,
    /// Contacts and settings, as (kind, key, value) in the database.
    pub kv: Vec<(String, String, Vec<u8>)>,
}

/// What may be handed over in `Account::kv`: contacts and what goes with
/// them, never keys or sessions.
pub fn may_copy(kind: &str, key: &str) -> bool {
    matches!(kind, "contact" | "person" | "timer" | "device_name")
        || (kind == "self" && matches!(key, "note_name" | "note_renamed" | "read_receipts" | "typing_indicators"))
}

/// This computer's name, as the account's other devices list it.
pub fn this_name() -> String {
    let name = gethostname::gethostname().to_string_lossy().trim().to_string();
    let name: String = name.chars().filter(|c| !c.is_control()).take(32).collect();
    if name.is_empty() { std::env::consts::OS.to_string() } else { name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_comes_back_as_it_left_and_not_after_it_expired() {
        let pairing = Pairing::new([1; 32], [2; 32], 1000);
        let link = pairing.link();
        assert!(link.len() <= MAX_LINK);
        assert_eq!(Pairing::parse(&link, 1000).unwrap(), pairing);
        assert_eq!(Pairing::parse(&format!("  {}\n", link.to_lowercase()), 1000).unwrap(), pairing);
        assert_eq!(Pairing::parse(&link, 1000 + PAIRING_LIFETIME), Err(PairingError::Expired));
        // One that claims to live much longer than a pairing does.
        assert_eq!(Pairing::parse(&link, 100), Err(PairingError::Expired));
        assert_eq!(Pairing::parse("nightpost:1:ABC", 1000), Err(PairingError::NotALink));
        let mut cut = link.clone();
        cut.truncate(link.len() - 4);
        assert_eq!(Pairing::parse(&cut, 1000), Err(PairingError::Malformed));
    }

    #[test]
    fn keys_and_sessions_are_never_handed_over() {
        assert!(may_copy("contact", "ab") && may_copy("self", "note_name"));
        for (kind, key) in [("self", "own"), ("self", "identity"), ("self", "device"), ("session", "x"), ("prekey", "1"), ("invite", "x"), ("self", "bridges")] {
            assert!(!may_copy(kind, key), "{kind}/{key}");
        }
    }
}
