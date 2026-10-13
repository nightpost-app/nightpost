//! The local API: how the terminal and graphical interfaces, and scripts,
//! talk to the daemon. JSON lines over a Unix socket in the data directory
//! (a named pipe on Windows). The first line from the client is the token
//! the daemon writes to `api.token` at start: the socket's directory is
//! private, and the token keeps it so where a named pipe is not.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result, bail};
use interprocess::local_socket::tokio::{RecvHalf, SendHalf, prelude::*};
use interprocess::local_socket::{ListenerOptions, Name};
use rand::{TryRngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::sync::broadcast::error::RecvError;

use crate::daemon::Daemon;
use crate::store::Store;
use crate::t;
use crate::transport::Transport;

/// No request is anywhere near this; it bounds what a stranger can make the
/// daemon buffer before the token is checked.
const MAX_LINE: u64 = 1 << 20;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Send { to: String, text: String },
    /// Every [`Event`] from now on, one per line, until the client leaves.
    Listen,
    Invite { name: String },
    Accept { link: String, name: String },
    /// A 12-digit code to read out, valid 10 minutes.
    InviteCode { name: String },
    /// Waits until the inviter confirms (up to 10 minutes).
    AcceptCode { code: String, name: String },
    /// Keep (or turn down) the person who used our code, after comparing
    /// the safety number of their [`Event::Fingerprint`].
    Confirm { name: String, keep: bool },
    Contacts,
    Verify { name: String },
    /// The last `limit` messages with `name`, oldest first.
    History { name: String, limit: u32 },
    /// Where the network and each contact stand now; [`Event::Network`] and
    /// [`Event::Link`] tell what changes after.
    Status,
    /// The name only we see them by.
    Rename { name: String, to: String },
    /// Revokes their access to our onion service: they can no longer reach
    /// us nor tell when we are online. `wipe` also deletes the conversation.
    Remove { name: String, wipe: bool },
    /// What we keep about them besides the name.
    SetPerson { name: String, person: Person },
    /// Adds, changes or removes the passphrase over the keychain secret.
    SetPassphrase { current: Option<String>, new: Option<String> },
    /// We have seen `name`'s messages so far: with read receipts on, they
    /// learn it.
    MarkRead { name: String },
    /// Whether we send read receipts, and so see theirs; `on` changes it.
    /// Off unless turned on.
    ReadReceipts { on: Option<bool> },
    /// We are writing to `name` now; sent every few seconds while we do.
    /// They learn it only if typing indicators are on, on both sides.
    Typing { name: String },
    /// Whether we tell people we are writing, and so see when they do; `on`
    /// changes it. Off unless turned on.
    TypingIndicators { on: Option<bool> },
    /// Bring the open window to the front (another one was started).
    Show,
    /// Close every interface of this kind (asked from the tray icon).
    Close { interface: Interface },
    /// The user's own Tor bridge lines; `lines` replaces them (none: the
    /// built-in ones). Answered while Tor is still starting too, since a
    /// network that blocks Tor is when they are needed.
    Bridges { lines: Option<Vec<String>> },
    /// The language was chosen again (see [`crate::i18n::choose`]): the
    /// daemon speaks it from now on.
    Language,
    /// Stops the daemon: the database closes and, with a passphrase, stays
    /// locked until the next start.
    Shutdown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    Sent { id: String },
    Invite { link: String },
    Code { code: String },
    Added { name: String },
    Done,
    Contacts { contacts: Vec<ContactInfo> },
    History { messages: Vec<HistoryItem> },
    SafetyNumber { name: String, number: String },
    Listening,
    Status { network: Network, links: Vec<(String, LinkState)> },
    ReadReceipts { on: bool },
    TypingIndicators { on: bool },
    Bridges { lines: Vec<String> },
    Error { message: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Message { from: String, id: String, time: u64, text: String },
    /// Sent from any interface: each shows it from here, not from its own
    /// reply, so all of them show the same conversation.
    Sent { to: String, id: String, time: u64, text: String },
    Delivered { to: String, id: String },
    /// They read these messages of ours.
    Read { to: String, ids: Vec<String> },
    ReadReceipts { on: bool },
    /// They are writing to us: shown for a few seconds unless another comes.
    Typing { from: String },
    TypingIndicators { on: bool },
    ContactAdded { name: String },
    /// Someone used an oral code: the two people compare this number before
    /// the inviter confirms.
    Fingerprint { name: String, safety_number: String },
    Network { network: Network },
    Link { name: String, state: LinkState },
    ContactRenamed { name: String, to: String },
    ContactRemoved { name: String },
    PersonChanged { name: String, person: Person },
    Show,
    Close { interface: Interface },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Interface {
    /// The graphical interface.
    Window,
    /// The terminal interface.
    Terminal,
}

/// How far Tor got, for interfaces to draw the route a message takes.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct Network {
    /// Tor's bootstrap, 0 to 100.
    pub percent: u8,
    /// Going through a bridge rather than straight to a guard.
    pub bridged: bool,
    /// The last probe ([`crate::transport::tor`]): the first hop answered,
    /// and a circuit of the kind onion services use got built. `None` until
    /// a probe ran since the last start or switch.
    pub entry: Option<bool>,
    pub circuit: Option<bool>,
    /// The last line for people, as the daemon's log shows it.
    pub line: String,
}

impl Network {
    /// How Tor stands in a few words, then a line of detail: what the
    /// interfaces show under the conversations.
    pub fn summary(&self) -> (String, String) {
        let line = self.line.strip_prefix("Tor: ").unwrap_or(&self.line).to_string();
        if self.blocked() {
            (t!("net-blocked"), line)
        } else if self.percent < 100 {
            (t!("net-connecting"), if line.is_empty() { format!("{}%", self.percent) } else { line })
        } else if self.bridged {
            (t!("net-bridged"), t!("net-bridged-help"))
        } else {
            (t!("net-connected"), t!("net-connected-help"))
        }
    }

    /// The last probe failed: nothing goes through.
    pub fn blocked(&self) -> bool {
        self.entry == Some(false) || self.circuit == Some(false)
    }
}

/// A link to one contact.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// Dialing them.
    Trying,
    Open,
    /// Closed after use: they were reachable then.
    Idle,
    /// The last attempt did not reach them; it is tried again later.
    Unreachable,
    /// They answered with an older protocol: they must update to talk.
    Older,
    /// They answered with a newer protocol: we must update.
    Newer,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct HistoryItem {
    pub id: String,
    pub outgoing: bool,
    pub time: u64,
    pub text: String,
    pub delivered: bool,
    /// Outgoing: they read it (read receipts on, on both sides).
    #[serde(default)]
    pub read: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ContactInfo {
    pub name: String,
    pub safety_number: String,
    #[serde(default)]
    pub person: Person,
}

/// What we keep about a contact besides the name, in the encrypted
/// database like everything else: who someone talks to is no one's business.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct Person {
    /// We compared safety numbers with them.
    pub verified: bool,
    /// No notifications until then (Unix milliseconds); `u64::MAX` for good.
    pub muted_until: Option<u64>,
    /// A picture we chose for them, as a `data:image/png;base64,` URI the
    /// interface made by decoding and re-encoding the image.
    pub photo: Option<String>,
}

/// A photo's data URI, at most this long: 256 × 256 PNG with room to spare.
pub const MAX_PHOTO: usize = 512 * 1024;

/// The data directory: `NIGHTPOST_DIR` if set (tests run several), else
/// the platform's. Created owner-only, and returned absolute so the socket
/// and the keychain entry do not depend on where a command runs from.
pub fn data_dir() -> Result<PathBuf> {
    let dir = match std::env::var_os("NIGHTPOST_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => directories::ProjectDirs::from("", "", "Nightpost").context("no home directory")?.data_dir().to_path_buf(),
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(std::fs::canonicalize(&dir)?)
}

/// The daemon's socket, opened before Tor is up.
pub struct Listener {
    listener: LocalSocketListener,
    token: String,
}

/// Where the bridge lines are kept, and the Tor client that follows them.
#[derive(Clone)]
pub struct BridgeLines {
    pub store: Rc<Store>,
    pub tor: tokio::sync::watch::Sender<Vec<String>>,
}

impl BridgeLines {
    fn answer(&self, lines: Option<Vec<String>>) -> Result<Reply> {
        if let Some(lines) = lines {
            let lines: Vec<String> = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect();
            crate::transport::tor::check_bridges(&lines)?;
            self.store.set_bridges(&lines)?;
            self.tor.send_replace(lines);
        }
        Ok(Reply::Bridges { lines: self.store.bridges()? })
    }
}

/// While Tor starts: the bridges, and for anything else an error that says
/// so. Gives the socket back once `ready` fires.
pub async fn serve_starting(listener: Listener, bridges: BridgeLines, ready: tokio::sync::oneshot::Receiver<()>) -> Listener {
    tokio::pin!(ready);
    loop {
        let conn = tokio::select! {
            _ = &mut ready => return listener,
            conn = listener.listener.accept() => conn,
        };
        let Ok(conn) = conn else { continue };
        let (bridges, token) = (bridges.clone(), listener.token.clone());
        tokio::task::spawn_local(async move {
            let (recv, mut send) = conn.split();
            let mut lines = BufReader::new(recv);
            if read_line(&mut lines).await.ok().flatten().as_deref() != Some(&*token) {
                return;
            }
            while let Ok(Some(line)) = read_line(&mut lines).await {
                let reply = match serde_json::from_str::<Request>(&line) {
                    Ok(Request::Language) => {
                        crate::i18n::reload();
                        Reply::Done
                    }
                    Ok(Request::Bridges { lines }) => bridges.answer(lines).unwrap_or_else(|e| Reply::Error { message: format!("{e:#}") }),
                    Ok(_) => Reply::Error { message: t!("err-starting") },
                    Err(e) => Reply::Error { message: format!("bad request: {e}") },
                };
                if write_line(&mut send, &reply).await.is_err() {
                    return;
                }
            }
        });
    }
}

/// The version of the daemon running on `dir`, as it wrote it at start
/// (none from a daemon older than this file).
pub fn running_version(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join("api.version")).ok()
}

/// Opens the socket clients reach the daemon on.
pub async fn bind(dir: &Path) -> Result<Listener> {
    if Client::connect(dir).await.is_ok() {
        bail!("a daemon is already running on {}", dir.display());
    }
    let mut token = [0u8; 32];
    OsRng.try_fill_bytes(&mut token)?;
    let token = hex(&token);
    write_private(&dir.join("api.token"), token.as_bytes())?;
    write_private(&dir.join("api.version"), crate::VERSION.as_bytes())?;

    let options = ListenerOptions::new().name(socket(dir)?);
    // Nothing still listens there (checked above): a leftover from a crash.
    #[cfg(unix)]
    let options = options.try_overwrite(true);
    // Where the system can (not macOS, which refuses), only we may connect;
    // elsewhere the data directory, 0700, keeps others out, and every client
    // needs the token besides.
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
    let options = {
        use interprocess::os::unix::local_socket::ListenerOptionsExt as _;
        options.mode(0o600)
    };
    let listener = options.create_tokio().context("opening the local API socket")?;
    Ok(Listener { listener, token })
}

/// Answers clients until the process ends.
pub async fn serve<T: Transport + 'static>(daemon: Rc<Daemon<T>>, listener: Listener, bridges: BridgeLines) -> Result<()> {
    loop {
        let conn = listener.listener.accept().await?;
        let (daemon, token, bridges) = (daemon.clone(), listener.token.clone(), bridges.clone());
        tokio::task::spawn_local(async move {
            if let Err(e) = answer(daemon, conn, &token, &bridges).await {
                tracing::debug!("API client: {e:#}");
            }
        });
    }
}

async fn answer<T: Transport + 'static>(daemon: Rc<Daemon<T>>, conn: LocalSocketStream, token: &str, bridges: &BridgeLines) -> Result<()> {
    let (recv, mut send) = conn.split();
    let mut lines = BufReader::new(recv);
    if read_line(&mut lines).await?.as_deref() != Some(token) {
        bail!("wrong token");
    }
    while let Some(line) = read_line(&mut lines).await? {
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(Request::Listen) => {
                let mut events = daemon.subscribe();
                write_line(&mut send, &Reply::Listening).await?;
                loop {
                    tokio::select! {
                        event = events.recv() => match event {
                            Ok(event) => write_line(&mut send, &event).await?,
                            Err(RecvError::Lagged(n)) => tracing::warn!("a listener missed {n} events"),
                            Err(RecvError::Closed) => return Ok(()),
                        },
                        // A listener sends nothing more: this is it hanging
                        // up. Noticed now, not at the next event, so the
                        // daemon knows at once that no window is open.
                        _ = read_line(&mut lines) => return Ok(()),
                    }
                }
            }
            Ok(Request::Shutdown) => {
                // Answered first: the caller learns it was heard.
                write_line(&mut send, &Reply::Done).await?;
                // Stopping Nightpost closes what shows it; a daemon that just
                // dies leaves them saying so.
                daemon.announce(Event::Close { interface: Interface::Window });
                daemon.announce(Event::Close { interface: Interface::Terminal });
                daemon.shut_down();
                return Ok(());
            }
            Ok(Request::Language) => {
                crate::i18n::reload();
                Reply::Done
            }
            Ok(Request::Bridges { lines }) => bridges.answer(lines).unwrap_or_else(|e| Reply::Error { message: format!("{e:#}") }),
            Ok(request) => handle(&daemon, request).await.unwrap_or_else(|e| Reply::Error { message: format!("{e:#}") }),
            Err(e) => Reply::Error { message: format!("bad request: {e}") },
        };
        write_line(&mut send, &reply).await?;
    }
    Ok(())
}

async fn handle<T: Transport + 'static>(daemon: &Rc<Daemon<T>>, request: Request) -> Result<Reply> {
    Ok(match request {
        Request::Send { to, text } => Reply::Sent { id: hex(&daemon.send(&to, &text)?) },
        Request::Invite { name } => Reply::Invite { link: daemon.invite(&name).await? },
        Request::Accept { link, name } => {
            daemon.accept(&link, &name).await?;
            Reply::Added { name }
        }
        Request::InviteCode { name } => Reply::Code { code: daemon.invite_code(&name).await? },
        Request::AcceptCode { code, name } => {
            daemon.accept_code(&code, &name).await?;
            Reply::Added { name }
        }
        Request::Confirm { name, keep } => {
            daemon.confirm(&name, keep)?;
            Reply::Done
        }
        Request::Contacts => Reply::Contacts { contacts: daemon.contacts()? },
        Request::History { name, limit } => Reply::History { messages: daemon.history(&name, limit.min(1000))? },
        Request::Verify { name } => {
            let number = daemon.contacts()?.into_iter().find(|c| c.name == name).with_context(|| t!("err-no-contact"))?.safety_number;
            Reply::SafetyNumber { name, number }
        }
        Request::Status => Reply::Status { network: daemon.network(), links: daemon.links() },
        Request::MarkRead { name } => {
            daemon.mark_read(&name)?;
            Reply::Done
        }
        Request::ReadReceipts { on } => {
            if let Some(on) = on {
                daemon.set_read_receipts(on)?;
            }
            Reply::ReadReceipts { on: daemon.read_receipts()? }
        }
        Request::Show => {
            daemon.announce(Event::Show);
            Reply::Done
        }
        Request::Close { interface } => {
            daemon.announce(Event::Close { interface });
            Reply::Done
        }
        Request::Typing { name } => {
            daemon.typing(&name)?;
            Reply::Done
        }
        Request::TypingIndicators { on } => {
            if let Some(on) = on {
                daemon.set_typing_indicators(on)?;
            }
            Reply::TypingIndicators { on: daemon.typing_indicators()? }
        }
        Request::Rename { name, to } => {
            daemon.rename(&name, &to)?;
            Reply::Done
        }
        Request::Remove { name, wipe } => {
            daemon.remove(&name, wipe).await?;
            Reply::Done
        }
        Request::SetPerson { name, person } => {
            daemon.set_person(&name, person)?;
            Reply::Done
        }
        Request::SetPassphrase { current, new } => {
            let bytes = |p: Option<String>| p.map(|p| zeroize::Zeroizing::new(p.into_bytes()));
            daemon.set_passphrase(bytes(current), bytes(new))?;
            Reply::Done
        }
        Request::Listen | Request::Shutdown | Request::Bridges { .. } | Request::Language => unreachable!("answered by the caller"),
    })
}

/// A connection to the daemon of the data directory `dir`.
pub struct Client {
    recv: BufReader<RecvHalf>,
    send: SendHalf,
}

impl Client {
    pub async fn connect(dir: &Path) -> Result<Client> {
        let conn = LocalSocketStream::connect(socket(dir)?).await.context("no daemon running")?;
        let token = std::fs::read_to_string(dir.join("api.token")).context("reading api.token")?;
        let (recv, mut send) = conn.split();
        send.write_all(format!("{}\n", token.trim()).as_bytes()).await?;
        Ok(Client { recv: BufReader::new(recv), send })
    }

    pub async fn call(&mut self, request: &Request) -> Result<Reply> {
        write_line(&mut self.send, request).await?;
        let line = read_line(&mut self.recv).await?.context("the daemon closed the connection")?;
        Ok(serde_json::from_str(&line)?)
    }

    /// The next event, after [`Request::Listen`]; `None` when the daemon stops.
    pub async fn event(&mut self) -> Result<Option<Event>> {
        match read_line(&mut self.recv).await? {
            Some(line) => Ok(Some(serde_json::from_str(&line)?)),
            None => Ok(None),
        }
    }
}

async fn read_line(r: &mut BufReader<RecvHalf>) -> Result<Option<String>> {
    let mut line = String::new();
    if (&mut *r).take(MAX_LINE).read_line(&mut line).await? == 0 {
        return Ok(None);
    }
    if !line.ends_with('\n') {
        bail!("line too long or cut short");
    }
    line.pop();
    Ok(Some(line))
}

async fn write_line(w: &mut SendHalf, value: &impl Serialize) -> Result<()> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    Ok(())
}

fn socket(dir: &Path) -> Result<Name<'static>> {
    #[cfg(unix)]
    {
        use interprocess::local_socket::GenericFilePath;
        Ok(dir.join("api.sock").to_fs_name::<GenericFilePath>()?.into_owned())
    }
    #[cfg(windows)]
    {
        use interprocess::local_socket::GenericNamespaced;
        use sha2::{Digest as _, Sha256};
        // Pipe names are machine-wide: one per data directory.
        let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
        Ok(format!("nightpost-{}", hex(&digest[..8])).to_ns_name::<GenericNamespaced>()?.into_owned())
    }
}

/// Written readable by the owner only.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    use std::io::Write as _;
    options.open(path)?.write_all(bytes)?;
    Ok(())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbkey::Unlock;

    /// Before Tor is up, the bridges are already checked, kept and passed on
    /// to it; anything else waits.
    #[tokio::test]
    async fn the_bridges_are_answered_while_tor_starts() {
        let dir = tempfile::tempdir().unwrap();
        let unlock = Unlock::new(Some(zeroize::Zeroizing::new([1; 32])), None).unwrap();
        let store = Rc::new(Store::open(&dir.path().join("nightpost.db"), &unlock).unwrap());
        let (tor, mut lines) = tokio::sync::watch::channel(Vec::new());
        let bridges = BridgeLines { store: store.clone(), tor };
        let line = "obfs4 192.0.2.5:443 0123456789ABCDEF0123456789ABCDEF01234567 cert=abc iat-mode=0".to_string();
        tokio::task::LocalSet::new()
            .run_until(async {
                let (ready, starting) = tokio::sync::oneshot::channel();
                let server = tokio::task::spawn_local(serve_starting(bind(dir.path()).await.unwrap(), bridges, starting));
                let mut client = Client::connect(dir.path()).await.unwrap();
                assert_eq!(running_version(dir.path()).as_deref(), Some(crate::VERSION));
                assert!(matches!(client.call(&Request::Contacts).await.unwrap(), Reply::Error { .. }));
                let bad = Request::Bridges { lines: Some(vec!["obfs4 nonsense".into()]) };
                assert!(matches!(client.call(&bad).await.unwrap(), Reply::Error { .. }));
                assert!(!lines.has_changed().unwrap());

                let set = Request::Bridges { lines: Some(vec!["# mine".into(), String::new(), format!("  {line}")]) };
                assert_eq!(client.call(&set).await.unwrap(), Reply::Bridges { lines: vec![line.clone()] });
                assert_eq!(*lines.borrow_and_update(), std::slice::from_ref(&line));
                assert_eq!(client.call(&Request::Bridges { lines: None }).await.unwrap(), Reply::Bridges { lines: vec![line.clone()] });
                ready.send(()).unwrap();
                server.await.unwrap();
            })
            .await;
        assert_eq!(store.bridges().unwrap(), [line]);
        store.set_bridges(&[]).unwrap();
        assert!(store.bridges().unwrap().is_empty());
    }

    /// A window that closes stops counting as a listener at once, not at the
    /// next event: the daemon then tells of messages itself.
    #[tokio::test]
    async fn a_listener_that_hangs_up_is_gone_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let unlock = Unlock::new(Some(zeroize::Zeroizing::new([1; 32])), None).unwrap();
        let store = Rc::new(Store::open(&dir.path().join("nightpost.db"), &unlock).unwrap());
        let daemon = Daemon::open(store.clone(), crate::transport::memory::Network::default().node()).unwrap();
        let bridges = BridgeLines { store, tor: tokio::sync::watch::channel(Vec::new()).0 };
        tokio::task::LocalSet::new()
            .run_until(async {
                tokio::task::spawn_local(serve(daemon.clone(), bind(dir.path()).await.unwrap(), bridges));
                let mut client = Client::connect(dir.path()).await.unwrap();
                assert_eq!(client.call(&Request::Listen).await.unwrap(), Reply::Listening);
                assert_eq!(daemon.listeners(), 1);
                drop(client);
                for _ in 0..100 {
                    if daemon.listeners() == 0 {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                panic!("still counted as listening 2 s after hanging up");
            })
            .await;
    }
}
