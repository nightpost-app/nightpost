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
    ContactAdded { name: String },
    /// Someone used an oral code: the two people compare this number before
    /// the inviter confirms.
    Fingerprint { name: String, safety_number: String },
    Network { network: Network },
    Link { name: String, state: LinkState },
    ContactRenamed { name: String, to: String },
    ContactRemoved { name: String },
    PersonChanged { name: String, person: Person },
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
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct HistoryItem {
    pub id: String,
    pub outgoing: bool,
    pub time: u64,
    pub text: String,
    pub delivered: bool,
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

/// Answers clients until the process ends.
pub async fn serve<T: Transport + 'static>(daemon: Rc<Daemon<T>>, dir: &Path) -> Result<()> {
    if Client::connect(dir).await.is_ok() {
        bail!("a daemon is already running on {}", dir.display());
    }
    let mut token = [0u8; 32];
    OsRng.try_fill_bytes(&mut token)?;
    let token = hex(&token);
    write_private(&dir.join("api.token"), token.as_bytes())?;

    let options = ListenerOptions::new().name(socket(dir)?);
    #[cfg(unix)]
    let options = {
        use interprocess::os::unix::local_socket::ListenerOptionsExt as _;
        // Nothing still listens there (checked above): a leftover from a crash.
        options.try_overwrite(true).mode(0o600)
    };
    let listener = options.create_tokio().context("opening the local API socket")?;
    loop {
        let conn = listener.accept().await?;
        let (daemon, token) = (daemon.clone(), token.clone());
        tokio::task::spawn_local(async move {
            if let Err(e) = answer(daemon, conn, &token).await {
                tracing::debug!("API client: {e:#}");
            }
        });
    }
}

async fn answer<T: Transport + 'static>(daemon: Rc<Daemon<T>>, conn: LocalSocketStream, token: &str) -> Result<()> {
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
                    match events.recv().await {
                        Ok(event) => write_line(&mut send, &event).await?,
                        Err(RecvError::Lagged(n)) => tracing::warn!("a listener missed {n} events"),
                        Err(RecvError::Closed) => return Ok(()),
                    }
                }
            }
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
            let number = daemon.contacts()?.into_iter().find(|c| c.name == name).context("no such contact")?.safety_number;
            Reply::SafetyNumber { name, number }
        }
        Request::Status => Reply::Status { network: daemon.network(), links: daemon.links() },
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
        Request::Listen => unreachable!("answered by the caller"),
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
