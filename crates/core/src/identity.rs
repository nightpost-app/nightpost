//! Accounts and their devices.
//!
//! An account is a libsignal identity key. Its devices are listed in a chain of
//! signed snapshots: each one names the hash of the one before, so a contact
//! who knows one list accepts only its direct successor. That is the only way
//! a device key, an onion address or a role reaches a contact, and
//! [`SignedDeviceList::verify`] is the only function that accepts one: two
//! paths to the same state with different checks is how a removed member got
//! back into an UmbraChat group.

use libsignal_protocol::{Fingerprint, FingerprintError, IdentityKey, IdentityKeyPair, PublicKey};
use rand::{TryRngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// libsignal device ids are 1..=127.
pub const MAX_DEVICES: usize = 127;
/// 127 devices at about 80 bytes each, with room to spare. Checked before the
/// signature, so an oversized list costs nothing to refuse.
const MAX_BODY: usize = 16 * 1024;
const CONTEXT: &[u8] = b"nightpost-devices-v1";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Holds the account identity and reads messages.
    Normal,
    /// Stores envelopes for the other devices and never holds the identity.
    Mailbox,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Device {
    pub id: u8,
    /// The onion service's public key (the address without `.onion`).
    pub onion: [u8; 32],
    /// The device's own signing key, a serialized libsignal public key: it is
    /// what a link handshake is signed with.
    pub key: Vec<u8>,
    pub role: Role,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct DeviceList {
    pub seq: u64,
    /// Hash of the signed list this one replaces; zeros for the first.
    pub prev: [u8; 32],
    pub devices: Vec<Device>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct SignedDeviceList {
    body: Vec<u8>,
    signature: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DeviceListError {
    TooLarge,
    BadSignature,
    Malformed,
    /// Valid, but not the successor of the list we know.
    NotNext,
    Invalid(&'static str),
}

impl DeviceList {
    pub fn first(devices: Vec<Device>) -> Self {
        DeviceList { seq: 0, prev: [0; 32], devices }
    }

    pub fn next(previous: &SignedDeviceList, previous_list: &DeviceList, devices: Vec<Device>) -> Self {
        DeviceList { seq: previous_list.seq + 1, prev: previous.hash(), devices }
    }

    pub fn device(&self, id: u8) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }

    fn check(&self) -> Result<(), DeviceListError> {
        use DeviceListError::Invalid;
        if self.devices.is_empty() || self.devices.len() > MAX_DEVICES {
            return Err(Invalid("between 1 and 127 devices"));
        }
        if !self.devices.iter().any(|d| d.role == Role::Normal) {
            return Err(Invalid("no device that can read"));
        }
        let mut seen = [false; 128];
        for d in &self.devices {
            if d.id == 0 || d.id as usize > MAX_DEVICES || std::mem::replace(&mut seen[d.id as usize], true) {
                return Err(Invalid("device ids must be distinct, 1..=127"));
            }
            PublicKey::deserialize(&d.key).map_err(|_| Invalid("device key"))?;
        }
        Ok(())
    }
}

impl SignedDeviceList {
    pub fn sign(list: &DeviceList, identity: &IdentityKeyPair) -> Result<Self, DeviceListError> {
        list.check()?;
        let body = postcard::to_stdvec(list).map_err(|_| DeviceListError::Malformed)?;
        if body.len() > MAX_BODY {
            return Err(DeviceListError::TooLarge);
        }
        let signature = identity
            .private_key()
            .calculate_signature_for_multipart_message(&[CONTEXT, &body], &mut OsRng.unwrap_err())
            .map_err(|_| DeviceListError::BadSignature)?
            .into_vec();
        Ok(SignedDeviceList { body, signature })
    }

    /// Accepts this list from `identity`'s account. With `previous` (the list
    /// already accepted from that account), returns the newer of the two: a
    /// device that missed some changes still talks to one that saw them,
    /// and an old list replayed never replaces a newer one. Two different
    /// lists with the same `seq` are a conflict, refused.
    pub fn verify(
        &self,
        identity: &IdentityKey,
        previous: Option<&SignedDeviceList>,
    ) -> Result<DeviceList, DeviceListError> {
        if self.body.len() > MAX_BODY {
            return Err(DeviceListError::TooLarge);
        }
        if !identity
            .public_key()
            .verify_signature_for_multipart_message(&[CONTEXT, &self.body], &self.signature)
        {
            return Err(DeviceListError::BadSignature);
        }
        let list: DeviceList =
            postcard::from_bytes(&self.body).map_err(|_| DeviceListError::Malformed)?;
        list.check()?;

        if let Some(previous) = previous {
            if previous.body == self.body {
                return Ok(list);
            }
            let previous_list: DeviceList =
                postcard::from_bytes(&previous.body).map_err(|_| DeviceListError::Malformed)?;
            if list.seq == previous_list.seq {
                return Err(DeviceListError::NotNext);
            }
            if list.seq < previous_list.seq {
                return Ok(previous_list);
            }
        }
        Ok(list)
    }

    /// Its `seq`, unchecked: for lists already verified.
    pub fn seq(&self) -> u64 {
        postcard::from_bytes::<DeviceList>(&self.body).map_or(0, |l| l.seq)
    }

    /// What the next list names as `prev`. The body alone: it already holds
    /// `seq` and `prev`, and XEdDSA signatures are randomized, so hashing the
    /// signature too would make one list have many hashes.
    pub fn hash(&self) -> [u8; 32] {
        Sha256::digest(&self.body).into()
    }
}

/// The 60-digit number two people read to each other to check that each has
/// the other's real key. Same digits on both sides.
pub fn safety_number(mine: &IdentityKey, theirs: &IdentityKey) -> Result<String, FingerprintError> {
    Fingerprint::new(2, 5200, &mine.serialize(), mine, &theirs.serialize(), theirs)?.display_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use libsignal_protocol::KeyPair;

    fn device(id: u8, role: Role) -> Device {
        let key = KeyPair::generate(&mut OsRng.unwrap_err()).public_key.serialize().into_vec();
        Device { id, onion: [id; 32], key, role }
    }

    fn identity() -> IdentityKeyPair {
        IdentityKeyPair::generate(&mut OsRng.unwrap_err())
    }

    #[test]
    fn a_chain_is_accepted_link_by_link() {
        let me = identity();
        let first = DeviceList::first(vec![device(1, Role::Normal)]);
        let signed1 = SignedDeviceList::sign(&first, &me).unwrap();
        assert_eq!(signed1.verify(me.identity_key(), None).unwrap(), first);

        let second = DeviceList::next(&signed1, &first, vec![device(1, Role::Normal), device(2, Role::Mailbox)]);
        let signed2 = SignedDeviceList::sign(&second, &me).unwrap();
        assert_eq!(signed2.verify(me.identity_key(), Some(&signed1)).unwrap(), second);
        // The same list again is fine; an older one leaves the newer in place.
        assert!(signed2.verify(me.identity_key(), Some(&signed2)).is_ok());
        assert_eq!(signed1.verify(me.identity_key(), Some(&signed2)).unwrap(), second);
        // Changes missed: a list several steps ahead is taken.
        let fourth = DeviceList { seq: 3, prev: [7; 32], devices: second.devices.clone() };
        let signed4 = SignedDeviceList::sign(&fourth, &me).unwrap();
        assert_eq!(signed4.verify(me.identity_key(), Some(&signed1)).unwrap(), fourth);
    }

    #[test]
    fn forged_tampered_or_conflicting_lists_are_refused() {
        let me = identity();
        let first = DeviceList::first(vec![device(1, Role::Normal)]);
        let signed1 = SignedDeviceList::sign(&first, &me).unwrap();

        // Signed by someone else.
        let forged = SignedDeviceList::sign(&first, &identity()).unwrap();
        assert_eq!(forged.verify(me.identity_key(), None), Err(DeviceListError::BadSignature));

        // One byte changed after signing.
        let mut tampered = signed1.clone();
        let last = tampered.body.len() - 1;
        tampered.body[last] ^= 1;
        assert_eq!(tampered.verify(me.identity_key(), None), Err(DeviceListError::BadSignature));

        // Validly signed, but another list under the same seq.
        let fork = DeviceList { seq: 0, prev: [9; 32], devices: first.devices.clone() };
        let fork = SignedDeviceList::sign(&fork, &me).unwrap();
        assert_eq!(fork.verify(me.identity_key(), Some(&signed1)), Err(DeviceListError::NotNext));
    }

    #[test]
    fn nonsensical_lists_are_refused_even_when_signed() {
        let me = identity();
        for devices in [
            vec![],
            vec![device(1, Role::Mailbox)],
            vec![device(1, Role::Normal), device(1, Role::Normal)],
            vec![device(0, Role::Normal)],
            vec![device(128, Role::Normal)],
            vec![Device { key: vec![1, 2, 3], ..device(1, Role::Normal) }],
        ] {
            let list = DeviceList::first(devices);
            assert!(matches!(SignedDeviceList::sign(&list, &me), Err(DeviceListError::Invalid(_))));
        }
    }

    #[test]
    fn safety_number_is_the_same_on_both_sides() {
        let (a, b) = (identity(), identity());
        let from_a = safety_number(a.identity_key(), b.identity_key()).unwrap();
        let from_b = safety_number(b.identity_key(), a.identity_key()).unwrap();
        assert_eq!(from_a, from_b);
        assert_eq!(from_a.len(), 60);
        assert_ne!(from_a, safety_number(a.identity_key(), identity().identity_key()).unwrap());
    }
}
