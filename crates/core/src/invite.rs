//! The two ways to add a contact: a link that holds everything, or a short
//! code to read aloud.
//!
//! The link (`nightpost:1:…`) carries the inviter's identity, so the contact
//! is pinned from the first message, plus a throwaway discovery key that lets
//! the invitee find the onion service at all. It is a secret: whoever holds it
//! can ask to become a contact.
//!
//! The code is 11 random digits and a Damm check digit. Both sides stretch it
//! with Argon2id into a temporary onion service and its discovery key; the
//! salt holds the 10-minute window it was made in, so the 10^11 codes cannot
//! be precomputed once and for all. 36 bits is little, which is why the
//! inviter still confirms the invitee's fingerprint before anything is added.

use std::fmt;

use argon2::{Algorithm, Argon2, Params, Version};
use data_encoding::BASE32_NOPAD;
use libsignal_protocol::{IdentityKey, IdentityKeyPair};
use rand::{Rng, TryRngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::dbkey::Secret;

const PREFIX: &str = "nightpost:";
const VERSION: &str = "1";
const CONTEXT: &[u8] = b"nightpost-invite-v1";
/// About 200 bytes once encoded; anything much longer was not made here.
const MAX_LINK: usize = 1024;
pub const INVITE_LIFETIME: u64 = 7 * 24 * 3600;
/// Clocks disagree; an expiry further out than this was not made by a
/// Nightpost that respects its own lifetime.
const CLOCK_SLACK: u64 = 24 * 3600;

pub const CODE_WINDOW: u64 = 600;
const CODE_CONTEXT: &[u8] = b"nightpost-code-v1";

/// What a link holds once parsed and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub identity: IdentityKey,
    /// The inviter's onion service.
    pub onion: [u8; 32],
    /// Private half of the throwaway discovery key for that service.
    pub discovery: [u8; 32],
    /// Presented when connecting, so the inviter knows which invitation it is.
    pub token: [u8; 16],
    /// Unix seconds.
    pub expires: u64,
}

#[derive(Serialize, Deserialize)]
struct Body {
    identity: Vec<u8>,
    onion: [u8; 32],
    discovery: [u8; 32],
    token: [u8; 16],
    expires: u64,
}

#[derive(Serialize, Deserialize)]
struct Signed {
    body: Vec<u8>,
    signature: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum InviteError {
    NotAnInvite,
    /// Made by a different version of Nightpost: one side must update.
    Version,
    Malformed,
    BadSignature,
    Expired,
}

impl Invite {
    /// A new invitation from `identity`, and the link to hand over.
    pub fn create(
        identity: &IdentityKeyPair,
        onion: [u8; 32],
        discovery: [u8; 32],
        now: u64,
    ) -> (Invite, String) {
        let mut rng = OsRng.unwrap_err();
        let invite = Invite {
            identity: *identity.identity_key(),
            onion,
            discovery,
            token: rng.random(),
            expires: now + INVITE_LIFETIME,
        };
        let body = postcard::to_stdvec(&Body {
            identity: invite.identity.serialize().into_vec(),
            onion,
            discovery,
            token: invite.token,
            expires: invite.expires,
        })
        .expect("postcard writes to a Vec");
        let signature = identity
            .private_key()
            .calculate_signature_for_multipart_message(&[CONTEXT, &body], &mut rng)
            .expect("signing with a valid key")
            .into_vec();
        let signed = postcard::to_stdvec(&Signed { body, signature }).expect("postcard writes to a Vec");
        let link = format!("{PREFIX}{VERSION}:{}", BASE32_NOPAD.encode(&signed));
        (invite, link)
    }

    /// Checks a pasted link. Case does not matter, and neither does
    /// whitespace around it, since links get retyped and wrapped.
    pub fn parse(link: &str, now: u64) -> Result<Invite, InviteError> {
        let link = link.trim();
        if link.len() > MAX_LINK {
            return Err(InviteError::Malformed);
        }
        let link = link.to_ascii_uppercase();
        let rest = link
            .strip_prefix(&PREFIX.to_ascii_uppercase())
            .ok_or(InviteError::NotAnInvite)?;
        let (version, data) = rest.split_once(':').ok_or(InviteError::Malformed)?;
        if version != VERSION {
            return Err(InviteError::Version);
        }

        let bytes = BASE32_NOPAD.decode(data.as_bytes()).map_err(|_| InviteError::Malformed)?;
        let signed: Signed = postcard::from_bytes(&bytes).map_err(|_| InviteError::Malformed)?;
        let body: Body = postcard::from_bytes(&signed.body).map_err(|_| InviteError::Malformed)?;
        let identity = IdentityKey::decode(&body.identity).map_err(|_| InviteError::Malformed)?;
        if !identity
            .public_key()
            .verify_signature_for_multipart_message(&[CONTEXT, &signed.body], &signed.signature)
        {
            return Err(InviteError::BadSignature);
        }
        if body.expires <= now || body.expires > now + INVITE_LIFETIME + CLOCK_SLACK {
            return Err(InviteError::Expired);
        }
        Ok(Invite {
            identity,
            onion: body.onion,
            discovery: body.discovery,
            token: body.token,
            expires: body.expires,
        })
    }
}

/// A 12-digit code, the last digit being the Damm check digit of the others.
#[derive(Clone, PartialEq, Eq)]
pub struct Code([u8; 12]);

#[derive(Debug, PartialEq, Eq)]
pub enum CodeError {
    /// Not 12 digits.
    Length,
    /// The check digit does not match: a digit is wrong or two are swapped.
    Typo,
}

/// What both sides derive from a code.
pub struct CodeKeys {
    /// Seed of the temporary onion service's key.
    pub onion: Secret,
    /// The discovery key for that service.
    pub discovery: Secret,
}

impl Code {
    pub fn generate() -> Code {
        let mut rng = OsRng.unwrap_err();
        let mut digits = [0u8; 12];
        for d in &mut digits[..11] {
            *d = rng.random_range(0..10);
        }
        digits[11] = damm(&digits[..11]);
        Code(digits)
    }

    /// Reads a typed code. Spaces, dashes and dots are ignored, so it can be
    /// typed the way it was read out.
    pub fn parse(typed: &str) -> Result<Code, CodeError> {
        let mut digits = [0u8; 12];
        let mut n = 0;
        for c in typed.chars() {
            match c {
                ' ' | '-' | '.' => {}
                '0'..='9' if n < 12 => {
                    digits[n] = c as u8 - b'0';
                    n += 1;
                }
                _ => return Err(CodeError::Length),
            }
        }
        if n != 12 {
            return Err(CodeError::Length);
        }
        if damm(&digits) != 0 {
            return Err(CodeError::Typo);
        }
        Ok(Code(digits))
    }

    /// The keys for the window starting at `window * CODE_WINDOW`. The
    /// inviter uses the current window; the invitee tries [`windows`].
    pub fn derive(&self, window: u64) -> CodeKeys {
        let password: Vec<u8> = self.0.iter().map(|d| d + b'0').collect();
        let salt = [CODE_CONTEXT, &window.to_be_bytes()].concat();
        let mut out = Zeroizing::new([0u8; 64]);
        // About a second on a 2017 desktop CPU: the cost an attacker pays per guess.
        Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            Params::new(128 * 1024, 10, 1, Some(64)).expect("valid Argon2 parameters"),
        )
        .hash_password_into(&password, &salt, &mut out[..])
        .expect("valid Argon2 output length");
        let mut onion = Secret::default();
        let mut discovery = Secret::default();
        onion.copy_from_slice(&out[..32]);
        discovery.copy_from_slice(&out[32..]);
        CodeKeys { onion, discovery }
    }
}

/// The code is a secret: it never goes into a log.
impl fmt::Debug for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Code(…)")
    }
}

impl fmt::Display for Code {
    /// `482-109-375-512`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, group) in self.0.chunks(3).enumerate() {
            if i > 0 {
                f.write_str("-")?;
            }
            for d in group {
                write!(f, "{d}")?;
            }
        }
        Ok(())
    }
}

/// The window a code made at `now` (unix seconds) belongs to.
pub fn window(now: u64) -> u64 {
    now / CODE_WINDOW
}

/// The windows the invitee tries: the current one, then the one before, for
/// a code made just before the change or a clock a little ahead.
pub fn windows(now: u64) -> [u64; 2] {
    let w = window(now);
    [w, w.saturating_sub(1)]
}

/// Damm's algorithm: 0 for a valid sequence; for the others, the digit to
/// append. It catches every single wrong digit and every swap of two
/// neighbours.
fn damm(digits: &[u8]) -> u8 {
    const TABLE: [[u8; 10]; 10] = [
        [0, 3, 1, 7, 5, 9, 8, 6, 4, 2],
        [7, 0, 9, 2, 1, 5, 4, 8, 6, 3],
        [4, 2, 0, 6, 8, 7, 1, 3, 5, 9],
        [1, 7, 5, 0, 9, 8, 3, 4, 2, 6],
        [6, 1, 2, 3, 0, 4, 5, 9, 7, 8],
        [3, 6, 7, 4, 2, 0, 9, 5, 8, 1],
        [5, 8, 6, 9, 7, 2, 0, 1, 3, 4],
        [8, 9, 4, 5, 3, 6, 2, 0, 1, 7],
        [9, 4, 3, 8, 6, 1, 7, 2, 0, 5],
        [2, 5, 8, 1, 4, 3, 6, 7, 9, 0],
    ];
    digits.iter().fold(0, |row, &d| TABLE[row as usize][d as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn identity() -> IdentityKeyPair {
        IdentityKeyPair::generate(&mut OsRng.unwrap_err())
    }

    #[test]
    fn a_link_comes_back_as_it_left() {
        let alice = identity();
        let (invite, link) = Invite::create(&alice, [1; 32], [2; 32], NOW);
        assert!(link.starts_with("nightpost:1:"));
        assert_eq!(Invite::parse(&link, NOW + 60), Ok(invite.clone()));
        // Retyped in lower case, with a trailing newline.
        assert_eq!(Invite::parse(&format!("{}\n", link.to_lowercase()), NOW), Ok(invite));
    }

    #[test]
    fn expired_altered_or_foreign_links_are_refused() {
        let alice = identity();
        let (_, link) = Invite::create(&alice, [1; 32], [2; 32], NOW);
        assert_eq!(Invite::parse(&link, NOW + INVITE_LIFETIME), Err(InviteError::Expired));
        // An expiry far beyond the lifetime means the clocks or the link are wrong.
        assert_eq!(Invite::parse(&link, NOW - 2 * CLOCK_SLACK), Err(InviteError::Expired));

        // Every single-character change is caught, by the signature or the decoder.
        let prefix = "nightpost:1:".len();
        for i in prefix..link.len() {
            let mut altered = link.clone().into_bytes();
            altered[i] = if altered[i] == b'A' { b'B' } else { b'A' };
            let altered = String::from_utf8(altered).unwrap();
            assert!(Invite::parse(&altered, NOW).is_err(), "change at {i} accepted");
        }

        assert_eq!(Invite::parse("nightpost:2:AAAA", NOW), Err(InviteError::Version));
        assert_eq!(Invite::parse("https://example.org", NOW), Err(InviteError::NotAnInvite));
        assert_eq!(Invite::parse(&"nightpost:1:".repeat(100), NOW), Err(InviteError::Malformed));
    }

    #[test]
    fn the_check_digit_catches_every_typo_and_swap() {
        for _ in 0..200 {
            let code = Code::generate();
            assert_eq!(Code::parse(&code.to_string()), Ok(code.clone()));
            for i in 0..12 {
                for wrong in 0..10 {
                    if wrong != code.0[i] {
                        let mut typo = code.0;
                        typo[i] = wrong;
                        assert_eq!(Code::parse(&Code(typo).to_string()), Err(CodeError::Typo));
                    }
                }
                if i < 11 && code.0[i] != code.0[i + 1] {
                    let mut swapped = code.0;
                    swapped.swap(i, i + 1);
                    assert_eq!(Code::parse(&Code(swapped).to_string()), Err(CodeError::Typo));
                }
            }
        }
        assert_eq!(Code::parse("123-456"), Err(CodeError::Length));
        assert_eq!(Code::parse("123-456-789-0123"), Err(CodeError::Length));
        assert_eq!(Code::parse("12a-456-789-012"), Err(CodeError::Length));
    }

    #[test]
    fn same_code_and_window_give_the_same_keys() {
        let code = Code::generate();
        let w = window(NOW);
        let alice = code.derive(w);
        // The invitee, typing it in a minute later, just after the window changed.
        let typed = Code::parse(&code.to_string()).unwrap();
        let bob = windows(NOW + CODE_WINDOW)
            .map(|w| typed.derive(w))
            .into_iter()
            .find(|k| k.onion == alice.onion)
            .expect("one of the two windows matches");
        assert_eq!(bob.discovery, alice.discovery);
        assert_ne!(alice.onion, alice.discovery);

        let other = Code::generate();
        assert_ne!(other.derive(w).onion, alice.onion);
        assert_ne!(code.derive(w + 1).onion, alice.onion);
    }
}
