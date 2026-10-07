//! `nightpost`: the daemon, and the commands that talk to it.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use nightpost_core::api::{Client, Reply, Request};
use nightpost_core::daemon::Daemon;
use nightpost_core::dbkey::{self, Unlock};
use nightpost_core::store::Store;
use nightpost_core::transport::tor::TorTransport;
use sha2::{Digest as _, Sha256};

const USAGE: &str = "\
usage:
  nightpost daemon                 run the daemon (Tor) in the foreground
  nightpost invite <name>          print a one-time link for <name>
  nightpost accept <link> <name>   use a link; the inviter becomes <name>
  nightpost send <name> [text]     send text (read from stdin without it)
  nightpost listen                 print events as JSON lines
  nightpost contacts               list contacts and safety numbers
  nightpost verify <name>          print the safety number to compare

Data lives in NIGHTPOST_DIR if set. Without a system keychain, the database
passphrase is read from NIGHTPOST_PASSPHRASE_FILE (or systemd credentials).";

#[tokio::main]
async fn main() {
    if let Err(e) = run(std::env::args().skip(1).collect()).await {
        eprintln!("nightpost: {e:#}");
        std::process::exit(1);
    }
}

async fn run(args: Vec<String>) -> Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let dir = data_dir()?;
    let request = match args.as_slice() {
        ["daemon"] => return daemon(&dir).await,
        ["--version" | "version"] => {
            println!("nightpost {} (protocol {})", env!("CARGO_PKG_VERSION"), nightpost_core::PROTOCOL_VERSION);
            return Ok(());
        }
        ["invite", name] => Request::Invite { name: name.to_string() },
        ["accept", link, name] => Request::Accept { link: link.to_string(), name: name.to_string() },
        ["send", to] | ["send", to, "-"] => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            Request::Send { to: to.to_string(), text: text.trim_end_matches(['\n', '\r']).to_string() }
        }
        ["send", to, text @ ..] => Request::Send { to: to.to_string(), text: text.join(" ") },
        ["listen"] => Request::Listen,
        ["contacts"] => Request::Contacts,
        ["verify", name] => Request::Verify { name: name.to_string() },
        _ => {
            println!("{USAGE}");
            return Ok(());
        }
    };

    let mut client = Client::connect(&dir).await.context("is `nightpost daemon` running?")?;
    match client.call(&request).await? {
        Reply::Error { message } => bail!("{message}"),
        Reply::Sent { id } => println!("{id}"),
        Reply::Invite { link } => println!("{link}"),
        Reply::Added { name } => println!("{name} added"),
        Reply::Contacts { contacts } => {
            for c in contacts {
                println!("{}\t{}", c.name, c.safety_number);
            }
        }
        Reply::SafetyNumber { number, .. } => {
            // Twelve groups of five, the way people read it out.
            let groups: Vec<&str> = number.as_bytes().chunks(5).map(|c| std::str::from_utf8(c).unwrap_or("")).collect();
            println!("{}", groups.join(" "));
        }
        Reply::Listening => {
            while let Some(event) = client.event().await? {
                println!("{}", serde_json_line(&event)?);
            }
        }
    }
    Ok(())
}

async fn daemon(dir: &Path) -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,nightpost_core=info".into()),
        )
        .init();
    if Client::connect(dir).await.is_ok() {
        bail!("a daemon is already running on {}", dir.display());
    }
    let store = Store::open(&dir.join("nightpost.db"), &unlock(dir)?).context("opening the database (wrong key?)")?;

    eprintln!("Tor: starting (minutes the first time)");
    let mut shown = String::new();
    let net = TorTransport::bootstrap(&dir.join("tor").join("state"), &dir.join("tor").join("cache"), |frac, blocked| {
        let line = match blocked {
            Some(why) => format!("Tor: stuck? {why}"),
            None => format!("Tor: {:.0}%", frac * 100.0),
        };
        if line != shown {
            eprintln!("{line}");
            shown = line;
        }
    })
    .await?;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let daemon = Daemon::open(store, net)?;
            daemon.start().await?;
            eprintln!("ready");
            tokio::select! {
                result = nightpost_core::api::serve(daemon.clone(), dir) => result,
                _ = tokio::signal::ctrl_c() => Ok(()),
            }
        })
        .await
}

/// The keychain secret, plus the passphrase if one is configured. Without a
/// keychain (a server), the passphrase alone.
fn unlock(dir: &Path) -> Result<Unlock> {
    let passphrase = match dbkey::passphrase_file() {
        Some(path) => Some(dbkey::read_passphrase(&path).with_context(|| format!("reading {}", path.display()))?),
        None => None,
    };
    // One keychain entry per data directory, so test instances do not share.
    let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
    let label: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    let secret = match dbkey::keychain_secret(&label) {
        Ok(secret) => Some(secret),
        Err(e) if passphrase.is_some() => {
            eprintln!("no keychain ({e}): using the passphrase alone");
            None
        }
        Err(e) => bail!("no keychain ({e}); set NIGHTPOST_PASSPHRASE_FILE to use a passphrase instead"),
    };
    Unlock::new(secret, passphrase).context("nothing to derive the database key from")
}

fn data_dir() -> Result<PathBuf> {
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
    // Absolute, so the socket and keychain label do not depend on where the
    // command runs from.
    Ok(std::fs::canonicalize(&dir)?)
}

fn serde_json_line(event: &nightpost_core::api::Event) -> Result<String> {
    Ok(serde_json::to_string(event)?)
}
