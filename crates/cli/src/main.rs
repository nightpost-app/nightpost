//! `nightpost`: the daemon, the terminal interface, and the commands that
//! talk to the daemon.

mod tui;

use std::io::Read as _;
use std::path::Path;
use std::rc::Rc;

use anyhow::{Context as _, Result, bail};
use nightpost_core::api::{self, BridgeLines, Client, Event, Network, Reply, Request};
use nightpost_core::daemon::Daemon;
use nightpost_core::dbkey::{self, Unlock};
use nightpost_core::store::Store;
use nightpost_core::transport::tor::{Bridges, TorTransport};
use zeroize::Zeroizing;

const USAGE: &str = "\
usage:
  nightpost                        the terminal interface (starts the daemon
                                   in the background if none runs)
  nightpost daemon                 run the daemon (Tor) in the foreground
  nightpost daemon --recovery      the same, opened with the recovery phrase
                                   (another keychain answers: new desktop)
  nightpost recovery               the 24 words that open the database when
                                   the keychain does not have its secret
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
  nightpost read-receipts [on|off] whether they learn when you read (off
                                   by default; off, you do not see theirs)
  nightpost typing-indicators [on|off]
                                   whether they see when you write (same)
  nightpost mark-read <name>       what <name> wrote so far is read
  nightpost bridges                your own Tor bridge lines
  nightpost bridges set            replace them with the lines on stdin
  nightpost bridges clear          back to the built-in ones (Snowflake)
  nightpost shutdown               stop the daemon

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
    // A daemon from another version would not understand all we ask.
    if !matches!(args.as_slice(), ["daemon" | "shutdown" | "--version" | "version"] | ["daemon", "--recovery"])
        && Client::connect(&dir).await.is_ok()
        && let running = api::running_version(&dir)
        && running.as_deref() != Some(nightpost_core::VERSION)
    {
        bail!(nightpost_core::t!("old-daemon", running = running.unwrap_or_else(|| "?".into()), mine = nightpost_core::VERSION));
    }
    let request = match args.as_slice() {
        [] => {
            start_daemon(&dir).await?;
            return tui::run(&dir).await;
        }
        ["daemon"] => return daemon(&dir, false).await,
        ["daemon", "--recovery"] => return daemon(&dir, true).await,
        ["recovery"] => Request::RecoveryPhrase,
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
        ["mark-read", name] => Request::MarkRead { name: name.to_string() },
        ["read-receipts"] => Request::ReadReceipts { on: None },
        ["read-receipts", "on"] => Request::ReadReceipts { on: Some(true) },
        ["read-receipts", "off"] => Request::ReadReceipts { on: Some(false) },
        ["typing-indicators"] => Request::TypingIndicators { on: None },
        ["typing-indicators", "on"] => Request::TypingIndicators { on: Some(true) },
        ["typing-indicators", "off"] => Request::TypingIndicators { on: Some(false) },
        ["bridges"] => Request::Bridges { lines: None },
        ["bridges", "set"] => Request::Bridges { lines: Some(std::io::stdin().lines().collect::<std::io::Result<_>>()?) },
        ["bridges", "clear"] => Request::Bridges { lines: Some(Vec::new()) },
        ["shutdown"] => Request::Shutdown,
        _ => {
            println!("{USAGE}");
            return Ok(());
        }
    };

    let mut client = Client::connect(&dir).await.map_err(|_| anyhow::anyhow!(nightpost_core::t!("err-no-daemon")))?;
    match client.call(&request).await? {
        Reply::Error { message } => bail!("{message}"),
        Reply::Sent { id } => println!("{id}"),
        Reply::Invite { link } => println!("{link}"),
        Reply::Code { code } => println!("{code}"),
        Reply::RecoveryPhrase { phrase } => println!("{}\n\n{phrase}\n\n{}", nightpost_core::t!("recovery-cli"), nightpost_core::t!("recovery-cli-after")),
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
        Reply::ReadReceipts { on } => println!("read receipts {}", if on { "on" } else { "off" }),
        Reply::TypingIndicators { on } => println!("typing indicators {}", if on { "on" } else { "off" }),
        Reply::Bridges { lines } if lines.is_empty() => eprintln!("none: the built-in ones (Snowflake), when Tor is blocked"),
        Reply::Bridges { lines } => println!("{}", lines.join("\n")),
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
    let mut client = Client::connect(dir).await.map_err(|_| anyhow::anyhow!(nightpost_core::t!("err-no-daemon")))?;
    match client.call(&Request::Listen).await? {
        Reply::Listening => Ok(client),
        other => bail!("unexpected reply {other:?}"),
    }
}


/// lyrebird next to this program (as in the archives), where the Linux
/// packages put it, or on the PATH.
fn lyrebird() -> Option<std::path::PathBuf> {
    let name = format!("lyrebird{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe().ok()?;
    let packaged = exe.parent()?.join("../libexec/nightpost").join(&name);
    if let Some(found) = [exe.with_file_name(&name), packaged].into_iter().find(|p| p.is_file()) {
        return Some(found);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(&name)).find(|p| p.is_file())
}

/// The tray icon, from the graphical interface beside this program: it
/// shows that the daemon runs, and goes with it. Not without a desktop.
fn tray() {
    let Ok(exe) = std::env::current_exe() else { return };
    let gui = exe.with_file_name(format!("nightpost-gui{}", std::env::consts::EXE_SUFFIX));
    let desktop = !cfg!(target_os = "linux") || std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
    if desktop && gui.is_file() {
        // Its errors go where the daemon's do.
        if let Ok(mut child) = std::process::Command::new(gui).arg("--tray").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).spawn() {
            std::thread::spawn(move || child.wait());
        }
    }
}

async fn daemon(dir: &Path, recover: bool) -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        // Plain text when stderr is a file (the graphical interface's daemon.log).
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                // arti warns whenever a spare onion circuit is slower than 80 % of
                // the previous ones and so dropped, which Tor does by design to
                // about one in five: noise, rebuilt 30 s later.
                // arti also reports "missing previous key … Regenerating" as an ERROR
                // at every start: our keystore is in memory by design. RUST_LOG=debug shows it.
                .unwrap_or_else(|_| "warn,nightpost_core=info,tor_circmgr::hspool=error,tor_hsservice::ipt_mgr=off".into()),
        )
        .init();
    if Client::connect(dir).await.is_ok() {
        bail!("a daemon is already running on {}", dir.display());
    }
    let unlock = unlock(dir, recover)?;
    let store = Rc::new(Store::open(&dir.join("nightpost.db"), &unlock).context("opening the database (wrong key?)")?);
    if recover && let Some(secret) = unlock.secret() {
        // It opened: this keychain keeps the secret from now on.
        dbkey::store_keychain_secret(&dbkey::keychain_label(dir), &secret).map_err(|e| anyhow::anyhow!(nightpost_core::t!("err-recovery-store", error = e.to_string())))?;
    }
    // Before 0.1.0-alpha.1 the bridges were a file, in the clear.
    let file = dir.join("bridges.txt");
    if let Ok(text) = std::fs::read_to_string(&file) {
        let lines: Vec<String> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect();
        store.set_bridges(&lines)?;
        std::fs::remove_file(&file).with_context(|| format!("removing {}", file.display()))?;
    }
    let (tor, lines) = tokio::sync::watch::channel(store.bridges()?);
    let bridges = BridgeLines { store: store.clone(), tor };

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // Open before Tor is up: the bridges are set while it is blocked.
            let (ready, starting) = tokio::sync::oneshot::channel();
            let listener = tokio::task::spawn_local(api::serve_starting(api::bind(dir).await?, bridges.clone(), starting));

            tray();
            eprintln!("Tor: starting (minutes the first time)");
            let lyrebird = lyrebird();
            if lyrebird.is_none() {
                eprintln!("no lyrebird next to nightpost nor on the PATH: on a network that blocks Tor, Nightpost cannot get around it");
            }
            let mut shown = String::new();
            let mut top = 0;
            // The daemon only exists once Tor is up; until then the latest state waits here.
            let (status, mut status_changes) = tokio::sync::watch::channel(Network::default());
            let bridges_config = lyrebird.map(|lyrebird| Bridges { lyrebird, lines });
            let net = TorTransport::bootstrap(&dir.join("tor").join("state"), &dir.join("tor").join("cache"), bridges_config, move |network| {
                // Until Tor is up, arti counts again from the start when it goes on to
                // the bridge: a number that goes back reads as a second start.
                top = top.max(network.percent);
                let back = network.percent < top && top < 100;
                if network.line != shown && !back {
                    eprintln!("{}", network.line);
                    shown = network.line.clone();
                }
                status.send_replace(network.clone());
            })
            .await?;
            let _ = ready.send(());
            let listener = listener.await?;

            let daemon = Daemon::open(store, net)?;
            daemon.set_network(status_changes.borrow_and_update().clone());
            let tor_up = status_changes.borrow().percent == 100;
            let forward = daemon.clone();
            tokio::task::spawn_local(async move {
                while status_changes.changed().await.is_ok() {
                    forward.set_network(status_changes.borrow_and_update().clone());
                }
            });
            daemon.start().await?;
            #[cfg(windows)]
            nightpost_core::register_app_id(dir);
            #[cfg(not(target_os = "macos"))]
            tokio::task::spawn_local(notify_unwatched(daemon.clone(), dir.to_path_buf()));
            // The daemon answers; Tor may still be connecting, and messages wait for it.
            if !tor_up {
                eprintln!("ready (Tor is still connecting: messages wait)");
            } else {
                eprintln!("ready");
            }
            tokio::select! {
                result = api::serve(daemon.clone(), listener, bridges) => result,
                _ = tokio::signal::ctrl_c() => Ok(()),
                _ = daemon.stopped() => Ok(()),
            }
        })
        .await
}

/// With no interface open, the daemon tells of messages itself, as the
/// graphical interface's notification settings say (its file in the data
/// folder). On macOS the app stays open for that.
#[cfg(not(target_os = "macos"))]
async fn notify_unwatched(daemon: Rc<Daemon<TorTransport>>, dir: std::path::PathBuf) {
    use tokio::sync::broadcast::error::RecvError;
    let mut events = daemon.subscribe();
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return,
        };
        // This receiver is ours; any other is an interface, which tells of it.
        if daemon.listeners() > 1 {
            continue;
        }
        let prefs = nightpost_core::prefs::Prefs::load(&dir);
        if !prefs.notify {
            continue;
        }
        let (title, body) = match event {
            Event::Message { from, text, .. } => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                let person = daemon.contacts().ok().and_then(|all| all.into_iter().find(|c| c.name == from)).map(|c| c.person);
                if person.and_then(|p| p.muted_until).is_some_and(|until| until > now) {
                    continue;
                }
                match (prefs.show_name, prefs.show_text) {
                    (true, true) => (from, nightpost_core::text::notification(&text)),
                    (true, false) => ("Nightpost".to_string(), format!("Message from {from}")),
                    _ => ("Nightpost".to_string(), "New message".to_string()),
                }
            }
            Event::Fingerprint { name, .. } => ("Nightpost".to_string(), format!("{name} used your invitation")),
            _ => continue,
        };
        // Kept in the system's history only if asked (a hint of freedesktop servers).
        #[cfg_attr(not(unix), allow(unused_variables))]
        let (sound, keep) = (prefs.sound, prefs.keep_history);
        // Blocking (D-Bus): on a thread of its own. No session bus (a server): nothing shown.
        std::thread::spawn(move || {
            let mut notification = notify_rust::Notification::new();
            notification.summary(&title).body(&body);
            #[cfg(windows)]
            notification.app_id(nightpost_core::APP_ID);
            #[cfg(unix)]
            if !keep {
                notification.hint(notify_rust::Hint::Transient(true));
            }
            if sound {
                notification.sound_name("message-new-instant");
            }
            let _ = notification.show();
        });
    }
}

/// `nightpost daemon` in the background when none answers, as the window
/// does: the passphrase asked here goes on its stdin, its output to
/// daemon.log, and Tor's progress shows on one line until it listens.
async fn start_daemon(dir: &Path) -> Result<()> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    use nightpost_core::t;
    if Client::connect(dir).await.is_ok() {
        return Ok(());
    }
    let asks = dbkey::passphrase_file().is_none() && dbkey::uses_passphrase(dir);
    // The words, once the passphrase failed and they were asked for.
    let mut recovery: Option<Zeroizing<String>> = None;
    for _ in 0..3 {
        let passphrase = if asks { Some(Zeroizing::new(rpassword::prompt_password("Passphrase: ")?)) } else { None };
        let log_path = dir.join("daemon.log");
        let log = std::fs::File::create(&log_path)?;
        let mut command = Command::new(std::env::current_exe()?);
        command.arg("daemon").stdin(if asks || recovery.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(log.try_clone()?).stderr(log);
        if recovery.is_some() {
            command.arg("--recovery");
        }
        // Its own process group: Ctrl+C here, or the terminal closing,
        // leaves it running.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut daemon = command.spawn().context("starting the daemon")?;
        if let Some(mut stdin) = daemon.stdin.take() {
            // The passphrase, then the words: one line each.
            for line in passphrase.iter().map(|p| p.as_str()).chain(recovery.iter().map(|r| r.as_str())) {
                stdin.write_all(Zeroizing::new(format!("{line}\n")).as_bytes())?;
            }
        }
        let mut shown = String::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            // While Tor starts, the daemon takes requests and answers them
            // once it is up.
            let ready = async { Client::connect(dir).await?.call(&Request::Status).await };
            if let Ok(Ok(Reply::Status { .. })) = tokio::time::timeout(std::time::Duration::from_millis(400), ready).await {
                eprint!("\r\x1b[K");
                return Ok(());
            }
            let text = std::fs::read_to_string(&log_path).unwrap_or_default();
            if let Some(status) = daemon.try_wait()? {
                let lost = text.contains(&t!("err-keychain-lost"));
                if lost || text.contains("wrong key") {
                    eprintln!("\r\x1b[K{}", if lost { t!("err-keychain-lost") } else { t!("wrong-passphrase") });
                    // Right passphrase, other keychain: the words open it.
                    eprint!("{} ", t!("recovery-ask"));
                    std::io::stderr().flush()?;
                    let mut answer = String::new();
                    std::io::stdin().read_line(&mut answer)?;
                    recovery = None;
                    if answer.trim().eq_ignore_ascii_case("y") {
                        let words = Zeroizing::new(rpassword::prompt_password(format!("{} ", t!("recovery-prompt")))?);
                        match dbkey::secret_from_phrase(&words) {
                            // Single line, normalised: what the daemon reads back.
                            Ok(secret) => recovery = Some(dbkey::recovery_phrase(&secret)),
                            Err(e) => eprintln!("{e:#}"),
                        }
                    }
                    break;
                }
                bail!("the daemon stopped ({status}): {}", text.lines().last().unwrap_or_default());
            }
            let progress = text.lines().rev().find(|l| l.starts_with("Tor:")).map_or_else(|| t!("starting"), String::from);
            if progress != shown {
                eprint!("\r\x1b[K{progress}");
                shown = progress;
            }
        }
    }
    bail!(t!("wrong-passphrase"))
}

/// The keychain secret, plus the passphrase if one is configured. Without a
/// keychain (a server), the passphrase alone.
fn unlock(dir: &Path, recover: bool) -> Result<Unlock> {
    // Typed without echo at a terminal, or one line from whatever started us.
    let line = |prompt: &str| -> Result<String> {
        if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            Ok(rpassword::prompt_password(prompt)?)
        } else {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line.truncate(line.trim_end_matches(['\r', '\n']).len());
            Ok(line)
        }
    };
    let passphrase = match dbkey::passphrase_file() {
        Some(path) => Some(dbkey::read_passphrase(&path).with_context(|| format!("reading {}", path.display()))?),
        None if dbkey::uses_passphrase(dir) => Some(Zeroizing::new(line("Passphrase: ")?.into_bytes())),
        None => None,
    };
    let secret = if recover {
        Some(dbkey::secret_from_phrase(&line(&format!("{} ", nightpost_core::t!("recovery-prompt")))?)?)
    } else {
        // A database made earlier is never opened with a secret made now:
        // another keychain answering (another desktop) says so, and the
        // words are the way out.
        let made = dir.join("nightpost.db").exists();
        match dbkey::keychain_secret(&dbkey::keychain_label(dir), !made) {
            Ok(Some(secret)) => Some(secret),
            Ok(None) => bail!(nightpost_core::t!("err-keychain-lost")),
            Err(e) if passphrase.is_some() => {
                eprintln!("no keychain ({e}): using the passphrase alone");
                None
            }
            Err(e) => bail!("no keychain ({e}); set NIGHTPOST_PASSPHRASE_FILE to use a passphrase instead"),
        }
    };
    Unlock::new(secret, passphrase).context("nothing to derive the database key from")
}

