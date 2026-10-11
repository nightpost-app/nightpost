# Nightpost

A messenger with no server in the middle. Each person's Nightpost is a Tor
onion service that only their contacts can find; messages go straight from
one to the other, end-to-end encrypted with Signal's protocol (PQXDH and the
Triple Ratchet, so post-quantum too).

- **No account, no phone number, no directory.** You add someone with a
  one-time link or QR code, or a 12-digit code read out over the phone.
- **Nobody in between sees who talks to whom.** There is no server to see
  it. Someone who has your onion address but is not your contact cannot
  connect, nor tell when you are online.
- **Messages wait.** Written while the other person is offline, a message is
  kept and delivered once you are both online.
- **Works where Tor is blocked**, through bridges (Snowflake built in, or
  your own): see [docs/bridges.md](docs/bridges.md).

> **Alpha.** Nightpost has not had an outside security review yet. Do not
> rely on it where a mistake could hurt someone. See [SECURITY.md](SECURITY.md)
> for what it protects against and what it does not.

## Install

Download your system's archive from the
[releases](https://github.com/nightpost-app/nightpost/releases). Each
release has a Sigstore attestation; to check one:

```sh
gh attestation verify nightpost-linux-x86_64.tar.gz --repo nightpost-app/nightpost
```

### Linux (x86-64)

```sh
tar xzf nightpost-linux-x86_64.tar.gz
./nightpost/nightpost-gui
```

The graphical app needs WebKitGTK 4.1 and libxdo (Fedora:
`sudo dnf install webkit2gtk4.1 libxdo`; Debian and Ubuntu:
`sudo apt install libwebkit2gtk-4.1-0 libxdo3`). The terminal app,
`./nightpost/nightpost`, needs neither.

### macOS (14 or later)

Unzip `Nightpost-macos.zip` and move *Nightpost* to Applications. It is not
notarised (that takes a paid Apple account), so the first time: right-click
it, *Open*, then *Open* again.

### Windows (10 or later, x86-64)

Unzip `nightpost-windows-x86_64.zip` anywhere and run `nightpost-gui.exe`.
Windows SmartScreen may warn about an unknown publisher: *More info*, then
*Run anyway*.

## First steps

1. Start the app and wait for *Connected to Tor* (under a minute, a few
   minutes through a bridge).
2. **Add someone:** *Add someone* → *Create a link* and send them the link
   (or show the QR code), or *Create a code to read out* and read the 12
   digits to them. They paste the link or type the code under *Add someone*
   in their own Nightpost, then *Use it*.
3. **Check it is really them:** open their card, *Compare safety number*,
   and read the digits together, in person or on a call. If they match,
   *They match*.

Your messages and keys stay on your computer, in a database encrypted with a
key from your system's keychain. In *Settings → Security* you can add a
passphrase on top, asked for at every start.

## The terminal

`nightpost` with no argument is the terminal interface. The same program runs
the daemon and has commands for scripts:

```sh
nightpost daemon                 # Tor and the messages, in the foreground
nightpost invite alice           # a one-time link for alice
nightpost send alice "hello"
nightpost listen                 # events as JSON lines
nightpost help                   # every command
```

The graphical app, the terminal app and the commands all talk to the same
daemon, and can be open at the same time.

## Building

Rust 1.93.1 or later and `protoc`; on Linux, for the graphical app,
the WebKitGTK 4.1 and libxdo development packages.

```sh
cargo build --workspace --release
```

The macOS app is in `apps/macos` (Swift, `swift build -c release`, then
`./bundle.sh <nightpost> <lyrebird>`).

## Licence

AGPL-3.0-only. Nightpost ships lyrebird, the Tor Project's bridge program,
under its own licence ([docs/lyrebird.txt](docs/lyrebird.txt)).
