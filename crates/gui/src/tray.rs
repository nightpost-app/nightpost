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

/// What a menu line does.
enum Action {
    Open(String),
    Terminal(String),
    CloseWindows,
    CloseTerminals,
    Stop,
}

enum Wake {
    Menu(MenuEvent),
    /// The accounts whose daemon answers, each with how Tor stands for it;
    /// empty: every daemon is gone.
    Status(Vec<(String, String)>),
}

/// The name an account goes by in the menu.
fn label(name: &str) -> String {
    if name == accounts::DEFAULT { t!("account-default") } else { name.to_string() }
}

/// The menu for these accounts: with one, what it always was; with several,
/// a line each, so that one icon serves them all.
struct Built {
    menu: Menu,
    status: Vec<(String, MenuItem)>,
    actions: HashMap<MenuId, Action>,
}

fn build(names: &[String]) -> Built {
    let several = names.len() > 1;
    let menu = Menu::new();
    let mut actions = HashMap::new();
    let mut status = Vec::new();
    for name in names {
        let item = MenuItem::new(t!("starting"), false, None);
        menu.append(&item).expect("a menu");
        status.push((name.clone(), item));
    }
    menu.append(&PredefinedMenuItem::separator()).expect("a menu");
    for name in names {
        let text = if several { t!("tray-open-account", name = label(name)) } else { t!("tray-open-window") };
        let item = MenuItem::new(text, true, None);
        menu.append(&item).expect("a menu");
        actions.insert(item.id().clone(), Action::Open(name.clone()));
    }
    let close = MenuItem::new(t!("tray-close-window"), true, None);
    menu.append_items(&[&close, &PredefinedMenuItem::separator()]).expect("a menu");
    actions.insert(close.id().clone(), Action::CloseWindows);
    // A Flatpak cannot start a terminal on the host: there, `flatpak run --command=nightpost …`.
    if std::env::var_os("FLATPAK_ID").is_none() {
        for name in names {
            let text = if several { t!("tray-terminal-account", name = label(name)) } else { t!("tray-open-terminal") };
            let item = MenuItem::new(text, true, None);
            menu.append(&item).expect("a menu");
            actions.insert(item.id().clone(), Action::Terminal(name.clone()));
        }
        let close = MenuItem::new(t!("tray-close-terminal"), true, None);
        menu.append_items(&[&close, &PredefinedMenuItem::separator()]).expect("a menu");
        actions.insert(close.id().clone(), Action::CloseTerminals);
    }
    let stop = MenuItem::new(t!("tray-stop"), true, None);
    menu.append(&stop).expect("a menu");
    actions.insert(stop.id().clone(), Action::Stop);
    Built { menu, status, actions }
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
                        live.push((account.name, line));
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
                    let names: Vec<String> = live.iter().map(|(n, _)| n.clone()).collect();
                    if built.status.iter().map(|(n, _)| n).ne(names.iter()) {
                        built = build(&names);
                        if let Some(tray) = &tray {
                            tray.set_menu(Some(Box::new(built.menu.clone())));
                        }
                    }
                    let several = live.len() > 1;
                    for ((name, line), (_, item)) in live.iter().zip(&built.status) {
                        item.set_text(if several { format!("{}: {line}", label(name)) } else { line.clone() });
                    }
                }
            }
            Loop::UserEvent(Wake::Menu(e)) => match built.actions.get(&e.id) {
                Some(Action::Open(name)) => {
                    // A second window for the same account brings the first to the front and goes.
                    if let Ok(exe) = std::env::current_exe() {
                        reap(Command::new(exe).args(["--account", name]).stdin(Stdio::null()).spawn());
                    }
                }
                Some(Action::Terminal(name)) => {
                    let program = std::env::current_exe().map(|e| e.with_file_name(format!("nightpost{}", std::env::consts::EXE_SUFFIX)));
                    if !program.is_ok_and(|p| open_terminal(&p, &["--account", name])) {
                        crate::notify("Nightpost", &t!("tray-no-terminal"), nightpost_core::prefs::Prefs::load(crate::dir()));
                    }
                }
                Some(Action::CloseWindows) => ask_all(Request::Close { interface: Interface::Window }),
                Some(Action::CloseTerminals) => ask_all(Request::Close { interface: Interface::Terminal }),
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
    use super::words;

    #[test]
    fn a_command_line_splits_as_a_shell_would() {
        assert_eq!(words("  wezterm start  -- "), ["wezterm", "start", "--"]);
        assert_eq!(words(r#""C:\Program Files\Alacritty\alacritty.exe" -e"#), [r"C:\Program Files\Alacritty\alacritty.exe", "-e"]);
        assert_eq!(words("foot --title 'Night post' ''"), ["foot", "--title", "Night post", ""]);
        assert!(words(" ").is_empty());
    }
}
