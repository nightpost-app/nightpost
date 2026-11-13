//! The icon in the tray (the notification area), started by the daemon and
//! gone with it: while it is there, Nightpost runs, window open or not. Its
//! menu opens and closes the window and the terminal interface, and stops
//! Nightpost.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use dioxus::desktop::tao::event::{Event as Loop, StartCause};
use dioxus::desktop::tao::event_loop::{ControlFlow, EventLoopBuilder};
use dioxus::desktop::trayicon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use dioxus::desktop::trayicon::{Icon, TrayIcon, TrayIconBuilder};
use nightpost_core::api::{self, Client, Interface, Reply, Request};
use nightpost_core::accounts;
use nightpost_core::{t, terminal};

/// What a menu line does. Opening and closing share a line: which it is
/// depends on whether the window (or terminal) is open when it is pressed.
enum Action {
    Window(String),
    Terminal(String),
    Stop,
}

/// One running account, as the poll sees it.
struct Live {
    name: String,
    /// How Tor stands.
    line: String,
    window: bool,
    terminal: bool,
}

enum Wake {
    Menu(MenuEvent),
    /// The accounts whose daemon answers; empty: every daemon is gone.
    Status(Vec<Live>),
}

/// The name an account goes by in the menu.
fn label(name: &str) -> String {
    if name == accounts::DEFAULT { t!("account-default") } else { name.to_string() }
}

/// Is a window (`gui.lock`) or a terminal (`tui.lock`) of this account open?
/// Both hold a lock on their file as long as they run.
fn held(dir: &Path, file: &str) -> bool {
    std::fs::File::open(dir.join(file)).is_ok_and(|f| matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
}

/// One account's lines.
struct Row {
    name: String,
    status: MenuItem,
    window: MenuItem,
    terminal: Option<MenuItem>,
}

/// The menu for these accounts: with one, what it always was; with several,
/// a line each, so that one icon serves them all.
struct Built {
    menu: Menu,
    rows: Vec<Row>,
    actions: HashMap<MenuId, Action>,
}

fn build(names: &[String]) -> Built {
    let menu = Menu::new();
    let mut actions = HashMap::new();
    let mut rows: Vec<Row> = names
        .iter()
        .map(|name| Row { name: name.clone(), status: MenuItem::new(t!("starting"), false, None), window: MenuItem::new("", true, None), terminal: None })
        .collect();
    for row in &rows {
        menu.append(&row.status).expect("a menu");
    }
    menu.append(&PredefinedMenuItem::separator()).expect("a menu");
    for row in &rows {
        menu.append(&row.window).expect("a menu");
        actions.insert(row.window.id().clone(), Action::Window(row.name.clone()));
    }
    // A Flatpak cannot start a terminal on the host: there, `flatpak run --command=nightpost …`.
    if std::env::var_os("FLATPAK_ID").is_none() {
        menu.append(&PredefinedMenuItem::separator()).expect("a menu");
        for row in &mut rows {
            let item = MenuItem::new("", true, None);
            menu.append(&item).expect("a menu");
            actions.insert(item.id().clone(), Action::Terminal(row.name.clone()));
            row.terminal = Some(item);
        }
    }
    menu.append(&PredefinedMenuItem::separator()).expect("a menu");
    let stop = MenuItem::new(t!("tray-stop"), true, None);
    menu.append(&stop).expect("a menu");
    actions.insert(stop.id().clone(), Action::Stop);
    Built { menu, rows, actions }
}

/// The texts of the lines, from what is open.
fn refresh(built: &Built, live: &[Live]) {
    let several = live.len() > 1;
    for (account, row) in live.iter().zip(&built.rows) {
        row.status.set_text(if several { format!("{}: {}", label(&account.name), account.line) } else { account.line.clone() });
        let name = label(&account.name);
        row.window.set_text(match (account.window, several) {
            (false, false) => t!("tray-open-window"),
            (true, false) => t!("tray-close-window"),
            (false, true) => t!("tray-open-account", name = name.clone()),
            (true, true) => t!("tray-close-account", name = name.clone()),
        });
        if let Some(item) = &row.terminal {
            item.set_text(match (account.terminal, several) {
                (false, false) => t!("tray-open-terminal"),
                (true, false) => t!("tray-close-terminal"),
                (false, true) => t!("tray-terminal-account", name = name),
                (true, true) => t!("tray-close-terminal-account", name = name),
            });
        }
    }
}

pub fn run() -> ! {
    // One for all the accounts, in the root folder: each daemon starts one
    // when it starts, and all but the first find the lock taken and go.
    let Ok(root) = api::root_dir() else { std::process::exit(1) };
    let lock = std::fs::File::create(root.join("tray.lock"));
    if !lock.as_ref().is_ok_and(|f| f.try_lock().is_ok()) {
        std::process::exit(0);
    }
    // The library warns on every start that it is deprecated: nothing we can fix.
    #[cfg(target_os = "linux")]
    gtk::glib::log_set_handler(Some("libayatana-appindicator"), gtk::glib::LogLevels::all(), false, false, |_, _, _| {});
    let event_loop = EventLoopBuilder::<Wake>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some({
        let proxy = proxy.clone();
        move |e| {
            let _ = proxy.send_event(Wake::Menu(e));
        }
    }));
    // Asked every 2 s rather than followed: a listener counts as an open
    // interface, and the daemon would then leave notifications to it.
    std::thread::spawn({
        let root = root.clone();
        move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
            let mut misses = 0;
            loop {
                let mut live = Vec::new();
                for account in accounts::list(&root).unwrap_or_default() {
                    let Ok(dir) = accounts::dir_of(&root, &account.name) else { continue };
                    let line = runtime.block_on(async {
                        match Client::connect(&dir).await?.call(&Request::Status).await? {
                            Reply::Status { network, .. } => anyhow::Ok(network.summary().0),
                            // Tor is starting: requests wait for it.
                            _ => Ok(t!("starting")),
                        }
                    });
                    if let Ok(line) = line {
                        live.push(Live { window: held(&dir, "gui.lock"), terminal: held(&dir, "tui.lock"), name: account.name, line });
                    }
                }
                misses = if live.is_empty() { misses + 1 } else { 0 };
                // Three misses in a row: every daemon is gone.
                if proxy.send_event(Wake::Status(if misses == 3 || !live.is_empty() { live } else { Vec::new() })).is_err() || misses == 3 {
                    return;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    });

    // In a Flatpak, the icon's image goes to a folder the desktop can see: this one
    // is shared with it, at the same path.
    let shared = std::env::var_os("FLATPAK_ID")
        .zip(std::env::var_os("XDG_RUNTIME_DIR"))
        .map(|(id, runtime)| Path::new(&runtime).join("app").join(id));
    let mut built = build(&[accounts::DEFAULT.to_string()]);
    let mut tray: Option<TrayIcon> = None;
    let mut misses = 0;
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            // Made once the loop runs, as macOS wants (and harmless elsewhere).
            Loop::NewEvents(StartCause::Init) => {
                let mut builder = TrayIconBuilder::new().with_menu(Box::new(built.menu.clone())).with_icon(icon()).with_tooltip("Nightpost");
                if let Some(dir) = &shared {
                    builder = builder.with_temp_dir_path(dir);
                }
                match builder.build() {
                    Ok(icon) => tray = Some(icon),
                    Err(e) => {
                        eprintln!("nightpost-gui: no tray icon: {e}");
                        *flow = ControlFlow::Exit;
                    }
                }
            }
            Loop::UserEvent(Wake::Status(live)) => {
                if live.is_empty() {
                    // The first answers can be a daemon still binding its socket.
                    misses += 1;
                    if misses >= 3 {
                        *flow = ControlFlow::Exit;
                    }
                } else {
                    misses = 0;
                    if built.rows.iter().map(|r| &r.name).ne(live.iter().map(|l| &l.name)) {
                        built = build(&live.iter().map(|l| l.name.clone()).collect::<Vec<_>>());
                        if let Some(tray) = &tray {
                            tray.set_menu(Some(Box::new(built.menu.clone())));
                        }
                    }
                    refresh(&built, &live);
                }
            }
            Loop::UserEvent(Wake::Menu(e)) => match built.actions.get(&e.id) {
                Some(Action::Window(name)) => {
                    let dir = api::root_dir().and_then(|root| accounts::dir_of(&root, name));
                    if dir.as_ref().is_ok_and(|dir| held(dir, "gui.lock")) {
                        ask(name.clone(), Request::Close { interface: Interface::Window });
                    } else if let Ok(exe) = std::env::current_exe() {
                        // A second window for the same account brings the first to the front and goes.
                        reap(Command::new(exe).args(["--account", name]).stdin(Stdio::null()).spawn());
                    }
                }
                Some(Action::Terminal(name)) => {
                    let dir = api::root_dir().and_then(|root| accounts::dir_of(&root, name));
                    if dir.as_ref().is_ok_and(|dir| held(dir, "tui.lock")) {
                        ask(name.clone(), Request::Close { interface: Interface::Terminal });
                    } else {
                        let program = std::env::current_exe().map(|e| e.with_file_name(format!("nightpost{}", std::env::consts::EXE_SUFFIX)));
                        if !program.is_ok_and(|p| open_terminal(&p, &["--account", name])) {
                            crate::notify("Nightpost", &t!("tray-no-terminal"), nightpost_core::prefs::Prefs::load(crate::dir()));
                        }
                    }
                }
                // The icon goes once the daemons have.
                Some(Action::Stop) => ask_all(Request::Shutdown),
                None => {}
            },
            _ => {}
        }
        let _ = &tray;
    })
}

fn icon() -> Icon {
    let image = image::load_from_memory(include_bytes!("../../../icons/nightpost.png")).expect("the icon decodes").into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).expect("a square RGBA icon")
}

/// Waits for the child in the background, so that it does not linger
/// once it is done.
fn reap(child: std::io::Result<std::process::Child>) -> bool {
    let Ok(mut child) = child else { return false };
    std::thread::spawn(move || child.wait());
    true
}

/// Sends `request` to one account's daemon without waiting for the answer.
fn ask(account: String, request: Request) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
        let dir = accounts::dir_of(&api::root_dir().ok()?, &account).ok()?;
        runtime.block_on(async { Client::connect(&dir).await?.call(&request).await }).ok()
    });
}

/// Sends `request` to every account's daemon without waiting for the answers.
fn ask_all(request: Request) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
        let root = api::root_dir().ok()?;
        for account in accounts::list(&root).ok()? {
            let Ok(dir) = accounts::dir_of(&root, &account.name) else { continue };
            runtime.block_on(async { Client::connect(&dir).await?.call(&request).await }).ok();
        }
        Some(())
    });
}

/// A command line in words, as a shell splits it: quotes hold spaces
/// together (`"C:\Program Files\…"`); backslashes are left alone, for
/// Windows paths.
fn words(line: &str) -> Vec<String> {
    let (mut words, mut word, mut quote, mut started) = (Vec::new(), String::new(), None, false);
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => (quote, started) = (Some(c), true),
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, c) => (word, started) = (word + &c.to_string(), true),
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// Runs `program` at the end of the command line `line`.
fn run_in(line: &str, program: &Path, args: &[&str]) -> bool {
    let words = words(line);
    let Some((terminal, rest)) = words.split_first() else { return false };
    reap(Command::new(terminal).args(rest).arg(program).args(args).stdin(Stdio::null()).spawn())
}

/// Runs `program` in a terminal window: false if none could be found.
fn open_terminal(program: &Path, args: &[&str]) -> bool {
    let picked = terminal::load(crate::dir());
    if run_in(&picked, program, args) {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // `start` gives the program a console of its own, in the terminal
        // Windows is set to use.
        reap(Command::new("cmd").args(["/c", "start", "", &*program.to_string_lossy()]).args(args).creation_flags(CREATE_NO_WINDOW).spawn())
    }
    #[cfg(not(windows))]
    {
        // The user's TERMINAL, then the standard launcher, then the
        // terminals desktops come with.
        let chosen = std::env::var_os("TERMINAL").map(|t| reap(Command::new(t).arg("-e").arg(program).args(args).stdin(Stdio::null()).spawn()));
        chosen == Some(true) || std::iter::once("xdg-terminal-exec").chain(terminal::TERMINALS.map(|(_, line)| line)).any(|line| run_in(line, program, args))
    }
}

#[cfg(test)]
mod tests {
    use super::{held, words};

    #[test]
    fn an_open_window_or_terminal_shows_by_its_lock() {
        let dir = std::env::temp_dir().join(format!("np-held-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!held(&dir, "gui.lock"), "no file: nothing open");
        let file = std::fs::File::create(dir.join("gui.lock")).unwrap();
        assert!(!held(&dir, "gui.lock"), "file, no lock: nothing open");
        file.try_lock().unwrap();
        assert!(held(&dir, "gui.lock"), "a window holds it");
        drop(file);
        assert!(!held(&dir, "gui.lock"), "closed: released");
        let terminal = std::fs::File::create(dir.join("tui.lock")).unwrap();
        terminal.try_lock_shared().unwrap();
        assert!(held(&dir, "tui.lock"), "a terminal holds it shared");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_command_line_splits_as_a_shell_would() {
        assert_eq!(words("  wezterm start  -- "), ["wezterm", "start", "--"]);
        assert_eq!(words(r#""C:\Program Files\Alacritty\alacritty.exe" -e"#), [r"C:\Program Files\Alacritty\alacritty.exe", "-e"]);
        assert_eq!(words("foot --title 'Night post' ''"), ["foot", "--title", "Night post", ""]);
        assert!(words(" ").is_empty());
    }
}
