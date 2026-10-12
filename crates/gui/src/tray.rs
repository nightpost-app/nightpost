//! The icon in the tray (the notification area), started by the daemon and
//! gone with it: while it is there, Nightpost runs, window open or not. Its
//! menu opens and closes the window and the terminal interface, and stops
//! Nightpost.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use dioxus::desktop::tao::event::{Event as Loop, StartCause};
use dioxus::desktop::tao::event_loop::{ControlFlow, EventLoopBuilder};
use dioxus::desktop::trayicon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use dioxus::desktop::trayicon::{Icon, TrayIconBuilder};
use nightpost_core::api::{Interface, Reply, Request};
use nightpost_core::t;

enum Wake {
    Menu(MenuEvent),
    /// How Tor stands; `None`: the daemon is gone.
    Status(Option<String>),
}

pub fn run() -> ! {
    // One per data folder: the daemon starts one each time it starts.
    let lock = std::fs::File::create(crate::dir().join("tray.lock"));
    if !lock.as_ref().is_ok_and(|f| f.try_lock().is_ok()) {
        std::process::exit(0);
    }
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
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
        let mut misses = 0;
        loop {
            let status = match runtime.block_on(crate::call(Request::Status)) {
                Ok(Reply::Status { network, .. }) => Some(crate::network_summary(&network).0),
                // Tor is starting: requests wait for it.
                Ok(_) => Some(t!("starting")),
                Err(_) => None,
            };
            misses = if status.is_some() { 0 } else { misses + 1 };
            // Three misses in a row: the daemon is gone.
            if (status.is_some() || misses == 3) && (proxy.send_event(Wake::Status(status)).is_err() || misses == 3) {
                return;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });

    let status = MenuItem::new(t!("starting"), false, None);
    let open = MenuItem::new(t!("tray-open-window"), true, None);
    let close = MenuItem::new(t!("tray-close-window"), true, None);
    let terminal = MenuItem::new(t!("tray-open-terminal"), true, None);
    let close_terminal = MenuItem::new(t!("tray-close-terminal"), true, None);
    let stop = MenuItem::new(t!("tray-stop"), true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &status,
        &PredefinedMenuItem::separator(),
        &open,
        &close,
        &PredefinedMenuItem::separator(),
        &terminal,
        &close_terminal,
        &PredefinedMenuItem::separator(),
        &stop,
    ])
    .expect("a menu");
    let mut tray = None;
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            // Made once the loop runs, as macOS wants (and harmless elsewhere).
            Loop::NewEvents(StartCause::Init) => {
                match TrayIconBuilder::new().with_menu(Box::new(menu.clone())).with_icon(icon()).with_tooltip("Nightpost").build() {
                    Ok(icon) => tray = Some(icon),
                    Err(e) => {
                        eprintln!("nightpost-gui: no tray icon: {e}");
                        *flow = ControlFlow::Exit;
                    }
                }
            }
            Loop::UserEvent(Wake::Status(Some(line))) => status.set_text(line),
            Loop::UserEvent(Wake::Status(None)) => *flow = ControlFlow::Exit,
            Loop::UserEvent(Wake::Menu(e)) => {
                if e.id == *open.id() {
                    // A second window brings the first to the front and goes.
                    if let Ok(exe) = std::env::current_exe() {
                        reap(Command::new(exe).stdin(Stdio::null()).spawn());
                    }
                } else if e.id == *close.id() {
                    ask(Request::Close { interface: Interface::Window });
                } else if e.id == *terminal.id() {
                    let program = std::env::current_exe().map(|e| e.with_file_name(format!("nightpost{}", std::env::consts::EXE_SUFFIX)));
                    if !program.is_ok_and(|p| open_terminal(&p)) {
                        crate::notify("Nightpost", &t!("tray-no-terminal"), crate::Prefs::load());
                    }
                } else if e.id == *close_terminal.id() {
                    ask(Request::Close { interface: Interface::Terminal });
                } else if e.id == *stop.id() {
                    // The icon goes once the daemon has.
                    ask(Request::Shutdown);
                }
            }
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

/// Sends `request` without waiting for the answer.
fn ask(request: Request) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
        runtime.block_on(crate::call(request)).ok()
    });
}

/// The terminals offered in Settings: name, and the command line the
/// program is added to. Any other command line can be typed in.
#[cfg(not(windows))]
pub const TERMINALS: [(&str, &str); 11] = [
    ("Ghostty", "ghostty -e"),
    ("Konsole", "konsole -e"),
    ("Ptyxis", "ptyxis --"),
    ("GNOME Terminal", "gnome-terminal --"),
    ("Console", "kgx -e"),
    ("Xfce Terminal", "xfce4-terminal -x"),
    ("Alacritty", "alacritty -e"),
    ("kitty", "kitty"),
    ("foot", "foot"),
    ("WezTerm", "wezterm start --"),
    ("XTerm", "xterm -e"),
];
#[cfg(windows)]
pub const TERMINALS: [(&str, &str); 3] = [("Windows Terminal", "wt"), ("Alacritty", "alacritty -e"), ("WezTerm", "wezterm start --")];

/// The command line picked in Settings; none: the system's.
pub const TERMINAL_FILE: &str = "terminal";

/// Those of [`TERMINALS`] on the PATH.
pub fn installed() -> Vec<(&'static str, &'static str)> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let found = |line: &str| {
        let command = line.split(' ').next().unwrap_or_default();
        // Not is_file: Windows Terminal's `wt.exe` is an app alias, which
        // only has metadata of its own.
        std::env::split_paths(&path).any(|dir| std::fs::symlink_metadata(dir.join(format!("{command}{}", std::env::consts::EXE_SUFFIX))).is_ok())
    };
    TERMINALS.into_iter().filter(|(_, line)| found(line)).collect()
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
fn run_in(line: &str, program: &Path) -> bool {
    let words = words(line);
    let Some((terminal, args)) = words.split_first() else { return false };
    reap(Command::new(terminal).args(args).arg(program).stdin(Stdio::null()).spawn())
}

/// Runs `program` in a terminal window: false if none could be found.
fn open_terminal(program: &Path) -> bool {
    let picked = std::fs::read_to_string(crate::dir().join(TERMINAL_FILE)).unwrap_or_default();
    if run_in(&picked, program) {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // `start` gives the program a console of its own, in the terminal
        // Windows is set to use.
        reap(Command::new("cmd").args(["/c", "start", "", &*program.to_string_lossy()]).creation_flags(CREATE_NO_WINDOW).spawn())
    }
    #[cfg(not(windows))]
    {
        // The user's TERMINAL, then the standard launcher, then the
        // terminals desktops come with.
        let chosen = std::env::var_os("TERMINAL").map(|t| reap(Command::new(t).arg("-e").arg(program).stdin(Stdio::null()).spawn()));
        chosen == Some(true) || std::iter::once("xdg-terminal-exec").chain(TERMINALS.map(|(_, line)| line)).any(|line| run_in(line, program))
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
