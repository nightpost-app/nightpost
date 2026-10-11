# Security

## Reporting a vulnerability

Use GitHub's private reporting: the repository's *Security* tab, *Report a
vulnerability*. Please do not open a public issue for it.

## Status

Nightpost is alpha software and has had no outside security review. It is
built on audited parts (libsignal, Tor through arti, SQLCipher), but the way
they are put together has not been checked by anyone else yet.

## What it protects against

| Who | Cannot learn | Can learn |
|---|---|---|
| Your local network, your provider | what you send, and to whom (Tor) | that you use Tor |
| A malicious Tor relay | what you send (Signal's protocol), who either end is | traffic patterns, to an adversary watching both ends of the network |
| Someone who has your onion address but is not your contact | anything: they cannot connect, nor tell when exactly you are online (restricted discovery) | that your onion service was published in the last few hours |
| A contact who turns hostile | anything you did not send them | when you are online, until you remove them |
| Someone who steals your computer while it is off | your messages and keys (encrypted database, its key in the system keychain) | anything, if your session was left open, unless you added a passphrase |
| A future quantum computer | the content of messages (ML-KEM in PQXDH and the Triple Ratchet) | Tor's circuit metadata (Tor is not post-quantum yet) |

## What it does not protect against

- **A computer that is already compromised** while Nightpost runs: malware
  can read the screen, the keyboard and memory.
- **Mistakes when adding someone.** An invitation link is a secret: whoever
  gets it first becomes that contact. Send it over a channel you trust, and
  compare safety numbers.
- **A global adversary** able to watch both ends of the Tor network at once.

## Design notes

- Every frame between two Nightposts is padded to a multiple of 512 bytes,
  so its size says little.
- Links between two Nightposts are authenticated both ways before anything
  is exchanged, and a contact's keys are pinned from the invitation on.
- Received text is never rendered as HTML.
- Your onion address is never in the name of a file or folder on disk.
