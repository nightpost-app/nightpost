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

/// Runs `program` in a terminal window: false if none could be found.
fn open_terminal(program: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // `start` gives the program a console of its own.
        reap(Command::new("cmd").args(["/c", "start", "", &*program.to_string_lossy()]).creation_flags(CREATE_NO_WINDOW).spawn())
    }
    #[cfg(not(windows))]
    {
        let program = program.as_os_str();
        // The user's choice first, then the standard launcher, then the
        // terminals desktops come with; each wants the program its own way.
        let chosen = std::env::var_os("TERMINAL").map(|t| (t, &["-e"][..]));
        let known: [(&str, &[&str]); 11] = [
            ("xdg-terminal-exec", &[]),
            ("konsole", &["-e"]),
            ("ptyxis", &["--"]),
            ("gnome-terminal", &["--"]),
            ("kgx", &["-e"]),
            ("xfce4-terminal", &["-x"]),
            ("alacritty", &["-e"]),
            ("kitty", &[]),
            ("foot", &[]),
            ("wezterm", &["start", "--"]),
            ("xterm", &["-e"]),
        ];
        chosen
            .into_iter()
            .chain(known.into_iter().map(|(t, args)| (t.into(), args)))
            .any(|(terminal, args)| reap(Command::new(terminal).args(args).arg(program).stdin(Stdio::null()).spawn()))
    }
}
