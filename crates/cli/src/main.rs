//! `nightpost`: the daemon, the terminal interface, and the commands that
//! talk to the daemon.

mod tui;

use std::io::Read as _;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use nightpost_core::api::{Client, Event, Network, Reply, Request};
use nightpost_core::daemon::Daemon;
use nightpost_core::dbkey::{self, Unlock};
use nightpost_core::store::Store;
use nightpost_core::transport::tor::{BRIDGES_FILE, Bridges, TorTransport};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

const USAGE: &str = "\
usage:
  nightpost                        the terminal interface
  nightpost daemon                 run the daemon (Tor) in the foreground
  nightpost invite <name>          print a one-time link for <name>
  nightpost invite --code <name>   a 12-digit code to read out (10 minutes)
  nightpost accept <link> <name>   use a link; the inviter becomes <name>
  nightpost accept-code <code> <name>
                                   use a code read out by the inviter
  nightpost send <name> [text]     send text (read from stdin without it)
  nightpost listen                 print events as JSON lines
  nightpost history <name> [n]     the last n messages (default 50), JSON lines
  nightpost contacts               list contacts and safety numbers
  nightpost verify <name>          print the safety number to compare
  nightpost status                 where Tor and each link stand, as JSON

Data lives in NIGHTPOST_DIR if set. Without a system keychain, the database
passphrase is read from NIGHTPOST_PASSPHRASE_FILE (or systemd credentials).
A passphrase added in the graphical interface is asked for by `daemon`.";

#[tokio::main]
async fn main() {
    if let Err(e) = run(std::env::args().skip(1).collect()).await {
        eprintln!("nightpost: {e:#}");
        std::process::exit(1);
    }
}

async fn run(args: Vec<String>) -> Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let dir = nightpost_core::api::data_dir()?;
    let request = match args.as_slice() {
        [] => return tui::run(&dir).await,
        ["daemon"] => return daemon(&dir).await,
        ["--version" | "version"] => {
            println!("nightpost {} (protocol {})", env!("CARGO_PKG_VERSION"), nightpost_core::PROTOCOL_VERSION);
            return Ok(());
        }
        ["invite", "--code", name] => return invite_code(&dir, name).await,
        ["accept-code", code, name] => return accept_code(&dir, code, name).await,
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
        ["history", name] => Request::History { name: name.to_string(), limit: 50 },
        ["history", name, n] => Request::History { name: name.to_string(), limit: n.parse().context("a number of messages")? },
        ["verify", name] => Request::Verify { name: name.to_string() },
        ["status"] => Request::Status,
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
        Reply::Code { code } => println!("{code}"),
        Reply::Added { name } => println!("{name} added"),
        Reply::Done => {}
        Reply::Contacts { contacts } => {
            for c in contacts {
                println!("{}\t{}", c.name, tui::groups(&c.safety_number));
            }
        }
        Reply::SafetyNumber { number, .. } => println!("{}", tui::groups(&number)),
        Reply::History { messages } => {
            for m in messages {
                println!("{}", serde_json::to_string(&m)?);
            }
        }
        status @ Reply::Status { .. } => println!("{}", serde_json::to_string(&status)?),
        Reply::Listening => {
            while let Some(event) = client.event().await? {
                println!("{}", serde_json::to_string(&event)?);
            }
        }
    }
    Ok(())
}

/// Prints the code, waits for someone to use it, and asks whether they read
/// out the same safety number.
async fn invite_code(dir: &Path, name: &str) -> Result<()> {
    let mut events = listen(dir).await?;
    let mut client = Client::connect(dir).await?;
    let code = match client.call(&Request::InviteCode { name: name.into() }).await? {
        Reply::Code { code } => code,
        Reply::Error { message } => bail!("{message}"),
        other => bail!("unexpected reply {other:?}"),
    };
    println!("Read this to {name} (valid 10 minutes):\n\n    {code}\n\nWaiting for {name}…");
    let number = loop {
        match events.event().await? {
            Some(Event::Fingerprint { name: n, safety_number }) if n == name => break safety_number,
            Some(_) => {}
            None => bail!("the daemon stopped"),
        }
    };
    println!("\nSomeone used the code. Ask {name} to read their safety number:\n\n    {}\n", tui::groups(&number));
    print!("Is it the same? [y/N] ");
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let keep = answer.trim().eq_ignore_ascii_case("y");
    match client.call(&Request::Confirm { name: name.into(), keep }).await? {
        Reply::Error { message } => bail!("{message}"),
        _ if keep => println!("{name} added"),
        _ => println!("turned down"),
    }
    Ok(())
}

async fn accept_code(dir: &Path, code: &str, name: &str) -> Result<()> {
    let mut events = listen(dir).await?;
    let name_owned = name.to_string();
    let shown = tokio::spawn(async move {
        while let Ok(Some(event)) = events.event().await {
            if let Event::Fingerprint { name: n, safety_number } = event
                && n == name_owned
            {
                println!("Read this safety number to {n}:\n\n    {}\n\nWaiting for {n} to confirm…", tui::groups(&safety_number));
            }
        }
    });
    let mut client = Client::connect(dir).await?;
    eprintln!("Looking for {name} (up to a few minutes)…");
    let reply = client.call(&Request::AcceptCode { code: code.into(), name: name.into() }).await?;
    shown.abort();
    match reply {
        Reply::Added { name } => println!("{name} added"),
        Reply::Error { message } => bail!("{message}"),
        other => bail!("unexpected reply {other:?}"),
    }
    Ok(())
}

async fn listen(dir: &Path) -> Result<Client> {
    let mut client = Client::connect(dir).await.context("is `nightpost daemon` running?")?;
    match client.call(&Request::Listen).await? {
        Reply::Listening => Ok(client),
        other => bail!("unexpected reply {other:?}"),
    }
}


/// lyrebird next to this program (as released) or on the PATH, and the
/// user's own bridges in `bridges.txt` in the data directory.
fn bridges(dir: &Path) -> Option<Bridges> {
    let name = format!("lyrebird{}", std::env::consts::EXE_SUFFIX);
    let beside = std::env::current_exe().ok()?.with_file_name(&name);
    let lyrebird = if beside.is_file() {
        beside
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(&name)).find(|p| p.is_file())?
    };
    Some(Bridges { lyrebird, file: dir.join(BRIDGES_FILE) })
}

async fn daemon(dir: &Path) -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        // Plain text when stderr is a file (the graphical interface's daemon.log).
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
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
    let bridges = bridges(dir);
    if bridges.is_none() {
        eprintln!("no lyrebird next to nightpost nor on the PATH: on a network that blocks Tor, Nightpost cannot get around it");
    }
    let mut shown = String::new();
    // The daemon only exists once Tor is up; until then the latest state waits here.
    let (status, mut status_changes) = tokio::sync::watch::channel(Network::default());
    let net = TorTransport::bootstrap(&dir.join("tor").join("state"), &dir.join("tor").join("cache"), bridges, move |network| {
        if network.line != shown {
            eprintln!("{}", network.line);
            shown = network.line.clone();
        }
        status.send_replace(network.clone());
    })
    .await?;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let daemon = Daemon::open(store, net)?;
            daemon.set_network(status_changes.borrow_and_update().clone());
            let forward = daemon.clone();
            tokio::task::spawn_local(async move {
                while status_changes.changed().await.is_ok() {
                    forward.set_network(status_changes.borrow_and_update().clone());
                }
            });
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
        // Typed without echo at a terminal, or one line from whatever started us.
        None if dbkey::uses_passphrase(dir) => Some(Zeroizing::new(if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            rpassword::prompt_password("Passphrase: ")?
        } else {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line.truncate(line.trim_end_matches(['\r', '\n']).len());
            line
        }.into_bytes())),
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

