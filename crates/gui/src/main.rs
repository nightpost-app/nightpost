//! `nightpost-gui`: the graphical interface. Like the terminal one it only
//! talks to the daemon through the local API, and starts the daemon when
//! none is running. Received text is only ever put in text nodes, never
//! parsed as HTML: this is a webview, and a message must not be a page.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;
use nightpost_core::api::{self, Client, Event, HistoryItem, Interface, LinkState, Network, Person, Reply, Request, Route};
use nightpost_core::prefs::Prefs;
use nightpost_core::{dbkey, i18n, t};

mod chrome;
mod person;
mod settings;
mod tray;

use chrome::Chrome;
use person::PersonPanel;
use settings::{BridgesEditor, Section, SettingsPage};

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// What this process changes in its own environment for WebKit, as it was:
/// given back to what it starts, or a terminal opened from the tray would
/// run under X11 too.
static OWN_ENV: OnceLock<Vec<(&str, Option<std::ffi::OsString>)>> = OnceLock::new();
/// The typeface travels inside the program: a Tor messenger fetches nothing
/// from a font server. Space Grotesk, one variable file for every weight,
/// under the SIL Open Font License (`fonts/`).
static STYLE: LazyLock<String> = LazyLock::new(|| {
    format!(
        "@font-face {{ font-family: \"Space Grotesk\"; font-weight: 300 700; src: url(data:font/ttf;base64,{}) format(\"truetype\"); }}\n{}",
        data_encoding::BASE64.encode(include_bytes!("../fonts/SpaceGrotesk.ttf")),
        include_str!("style.css")
    )
});
const HISTORY: u32 = 200;

fn main() {
    // WebKitGTK's DMA-BUF renderer hands the compositor a first frame whose
    // size is off by a rounding error under fractional scaling (KWin 6.7 at
    // 1920.0000000000002, then 1920): KDE takes that for a resize and drops
    // its maximise animation. Shared memory frames come at the exact size.
    // Still one thread here, so setting the environment is sound.
    let _ = OWN_ENV.set(["GDK_BACKEND", "WEBKIT_DISABLE_DMABUF_RENDERER"].into_iter().map(|k| (k, std::env::var_os(k))).collect());
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
    let dir = match api::data_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("nightpost-gui: {e:#}");
            std::process::exit(1);
        }
    };
    #[cfg(windows)]
    nightpost_core::register_app_id(&dir);
    DIR.set(dir).expect("set once");
    if std::env::args().nth(1).as_deref() == Some("--tray") {
        tray::run();
    }
    // One window per data folder: a second brings the first to the front
    // and goes. The lock lasts as long as this process.
    if let Ok(lock) = std::fs::File::create(self::dir().join("gui.lock")) {
        if let Err(std::fs::TryLockError::WouldBlock) = lock.try_lock() {
            let _ = tokio::runtime::Builder::new_current_thread().enable_all().build().map(|r| r.block_on(call(Request::Show)));
            return;
        }
        std::mem::forget(lock);
    }
    // Our own frame (chrome.rs): no system title bar; macOS keeps its own
    // buttons. The page is never see-through (WebKitGTK then keeps only what
    // changes); on Linux the window is, for the corners cut out of it.
    let window = WindowBuilder::new()
        .with_title("Nightpost")
        .with_inner_size(LogicalSize::new(1040.0, 700.0))
        .with_min_inner_size(LogicalSize::new(720.0, 480.0))
        .with_decorations(cfg!(target_os = "macos"));
    #[cfg(target_os = "linux")]
    let window = {
        use dioxus::desktop::tao::platform::unix::WindowBuilderExtUnix as _;
        window.with_rgba_visual(true).with_app_paintable(true)
    };
    // The page runs up under a see-through title bar, as in Signal: the
    // system's buttons stay where it puts them, on a strip of their own.
    // (tao only moves them when its own view redraws, which the webview
    // covering it never lets happen.)
    #[cfg(target_os = "macos")]
    let window = {
        use dioxus::desktop::tao::platform::macos::WindowBuilderExtMacOS as _;
        window.with_titlebar_transparent(true).with_title_hidden(true).with_fullsize_content_view(true)
    };
    let config = Config::new().with_window(window).with_menu(None).with_background_color((15, 21, 34, 255));
    dioxus::LaunchBuilder::desktop().with_cfg(config).launch(App);
}

fn dir() -> &'static PathBuf {
    DIR.get().expect("set in main")
}

#[derive(Clone, PartialEq)]
enum Phase {
    Connecting(String),
    /// The database needs the passphrase before the daemon can start; the
    /// text says why it is asked again, if it is.
    Locked(String),
    Ready,
    Failed(String),
}

#[derive(Clone, PartialEq)]
enum Dialog {
    Add,
    Invite { name: String, link: String },
    Code { name: String, code: String },
    /// Someone used our code: keep them only if the numbers match.
    Confirm { name: String, number: String },
    /// We used a code: read our number to the inviter.
    Waiting { name: String, number: String },
    /// Full screen, over everything: the digits to read out.
    Safety { name: String, number: String },
    Bridges,
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Chat,
    Settings(Section),
}

/// Everything the components share. Signals are `Copy`.
#[derive(Clone, Copy)]
struct State {
    phase: Signal<Phase>,
    contacts: Signal<Vec<String>>,
    /// The note to ourselves: first, kept here, nobody to reach.
    note: Signal<Option<String>>,
    unread: Signal<HashMap<String, usize>>,
    selected: Signal<Option<String>>,
    messages: Signal<HashMap<String, Vec<HistoryItem>>>,
    dialog: Signal<Option<Dialog>>,
    status: Signal<String>,
    /// Names we gave a code to.
    codes: Signal<Vec<String>>,
    network: Signal<Network>,
    links: Signal<HashMap<String, LinkState>>,
    people: Signal<HashMap<String, Person>>,
    /// The last message that came in, numbered so its pulse runs once.
    arrival: Signal<Option<(String, u64)>>,
    theme: Signal<Theme>,
    prefs: Signal<Prefs>,
    view: Signal<View>,
    /// The selected person's panel is open.
    panel: Signal<bool>,
    /// The window fills the screen: no border, no edges to drag.
    maximized: Signal<bool>,
    /// We send read receipts, and so see theirs (the daemon keeps it).
    read_receipts: Signal<bool>,
    /// We tell people we are writing, and so see when they do.
    typing_indicators: Signal<bool>,
    /// Who is writing to us now, numbered as `arrival` so only the last
    /// ping takes them away.
    typing: Signal<HashMap<String, u64>>,
    /// The language spoken; choosing another draws everything again.
    language: Signal<&'static str>,
}

#[derive(Clone, Copy, PartialEq)]
enum Theme {
    System,
    Dark,
    Light,
}

impl Theme {
    const FILE: &str = "gui-theme";

    fn load() -> Theme {
        match std::fs::read_to_string(dir().join(Theme::FILE)).unwrap_or_default().trim() {
            "dark" => Theme::Dark,
            "light" => Theme::Light,
            _ => Theme::System,
        }
    }

    fn save(self) -> Theme {
        let _ = std::fs::write(dir().join(Theme::FILE), self.name());
        self
    }

    fn name(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }
}

impl State {
    fn select(mut self, name: String) {
        self.unread.write().remove(&name);
        self.selected.set(Some(name.clone()));
        if !self.messages.read().contains_key(&name) {
            spawn(async move {
                match call(Request::History { name: name.clone(), limit: HISTORY }).await {
                    Ok(Reply::History { messages }) => {
                        self.messages.write().insert(name, messages);
                    }
                    other => self.fail(other),
                }
            });
        }
        self.mark_read();
    }

    /// The open conversation, on screen, is read; the daemon tells them only
    /// with read receipts on.
    fn mark_read(self) {
        let Some(name) = self.selected.peek().clone() else { return };
        if !dioxus::desktop::window().window.is_focused() {
            return;
        }
        spawn(async move {
            match call(Request::MarkRead { name }).await {
                Ok(Reply::Done) => {}
                other => self.fail(other),
            }
        });
    }

    /// The contacts, and the history of those not loaded yet: the list
    /// shows each one's last message.
    async fn reload_contacts(mut self) {
        let names: Vec<String> = match call(Request::Contacts).await {
            Ok(Reply::Contacts { contacts }) => {
                self.people.set(contacts.iter().map(|c| (c.name.clone(), c.person.clone())).collect());
                self.note.set(contacts.iter().find(|c| c.note).map(|c| c.name.clone()));
                contacts.into_iter().map(|c| c.name).collect()
            }
            other => return self.fail(other),
        };
        for name in &names {
            if self.messages.read().contains_key(name) {
                continue;
            }
            match call(Request::History { name: name.clone(), limit: HISTORY }).await {
                Ok(Reply::History { messages }) => {
                    self.messages.write().insert(name.clone(), messages);
                }
                other => self.fail(other),
            }
        }
        let latest = {
            let messages = self.messages.read();
            names.iter().max_by_key(|n| messages.get(*n).and_then(|m| m.last()).map(|m| m.time)).cloned()
        };
        self.contacts.set(names);
        if self.selected.read().is_none()
            && let Some(latest) = latest
        {
            self.select(latest);
        }
    }

    fn fail(mut self, reply: Result<Reply>) {
        self.status.set(match reply {
            Ok(Reply::Error { message }) => message,
            Ok(other) => t!("err-unexpected", reply = format!("{other:?}")),
            Err(e) => format!("{e:#}"),
        });
    }

    fn on_event(mut self, event: Event) {
        match event {
            Event::Message { from, id, time, text } => {
                self.typing.write().remove(&from);
                let n = self.arrival.read().as_ref().map_or(0, |a| a.1) + 1;
                self.arrival.set(Some((from.clone(), n)));
                spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    if self.arrival.read().as_ref().is_some_and(|a| a.1 == n) {
                        self.arrival.set(None);
                    }
                });
                let body = nightpost_core::text::notification(&text);
                self.messages.write().entry(from.clone()).or_default().push(HistoryItem { id, outgoing: false, time, text, delivered: true, read: false });
                let here = self.selected.read().as_deref() == Some(from.as_str());
                if here {
                    self.mark_read();
                }
                if !here || !dioxus::desktop::window().window.is_focused() {
                    if !here {
                        *self.unread.write().entry(from.clone()).or_default() += 1;
                    }
                    let prefs = *self.prefs.read();
                    let muted = self.people.read().get(&from).is_some_and(|p| muted(p).is_some());
                    if prefs.notify && !muted {
                        match (prefs.show_name, prefs.show_text) {
                            (true, true) => notify(&from, &body, prefs),
                            (true, false) => notify("Nightpost", &t!("notify-from", name = from), prefs),
                            _ => notify("Nightpost", &t!("notify-new"), prefs),
                        }
                    }
                }
            }
            Event::Sent { to, id, time, text } => {
                // A note has no one to deliver to: it is there once kept.
                let kept = self.note.read().as_deref() == Some(to.as_str());
                self.messages.write().entry(to).or_default().push(HistoryItem { id, outgoing: true, time, text, delivered: kept, read: false });
            }
            Event::Delivered { to, id } => {
                if let Some(m) = self.messages.write().get_mut(&to).and_then(|ms| ms.iter_mut().find(|m| m.id == id)) {
                    m.delivered = true;
                }
            }
            Event::Read { to, ids } => {
                for m in self.messages.write().get_mut(&to).into_iter().flatten() {
                    if ids.contains(&m.id) {
                        m.read = true;
                    }
                }
            }
            Event::ReadReceipts { on } => self.read_receipts.set(on),
            Event::Show => {
                let window = dioxus::desktop::window();
                window.set_visible(true);
                window.set_minimized(false);
                window.set_focus();
            }
            // From the tray icon, or Nightpost stopping.
            Event::Close { interface: Interface::Window } => dioxus::desktop::window().close(),
            Event::Close { interface: Interface::Terminal } => {}
            Event::TypingIndicators { on } => self.typing_indicators.set(on),
            Event::Typing { from } => {
                let n = self.typing.read().values().max().copied().unwrap_or(0) + 1;
                self.typing.write().insert(from.clone(), n);
                spawn(async move {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    if self.typing.read().get(&from) == Some(&n) {
                        self.typing.write().remove(&from);
                    }
                });
            }
            Event::ContactAdded { name } => {
                self.status.set(t!("now-contact", name = name));
                if matches!(&*self.dialog.read(), Some(Dialog::Waiting { name: n, .. } | Dialog::Invite { name: n, .. }) if *n == name) {
                    self.dialog.set(None);
                }
                spawn(async move {
                    self.reload_contacts().await;
                    self.select(name);
                });
            }
            Event::Fingerprint { name, safety_number } => {
                notify("Nightpost", &t!("notify-used-invite", name = name), *self.prefs.read());
                let ours = self.codes.read().contains(&name);
                self.dialog.set(Some(if ours {
                    Dialog::Confirm { name, number: safety_number }
                } else {
                    Dialog::Waiting { name, number: safety_number }
                }));
            }
            Event::ContactRenamed { name, to } => {
                fn rekey<V: 'static>(mut map: Signal<HashMap<String, V>>, name: &str, to: &str) {
                    let value = map.write().remove(name);
                    if let Some(value) = value {
                        map.write().insert(to.into(), value);
                    }
                }
                rekey(self.messages, &name, &to);
                rekey(self.unread, &name, &to);
                rekey(self.links, &name, &to);
                rekey(self.people, &name, &to);
                for n in self.contacts.write().iter_mut().filter(|n| **n == name) {
                    *n = to.clone();
                }
                if self.note.read().as_deref() == Some(name.as_str()) {
                    self.note.set(Some(to.clone()));
                }
                if self.selected.read().as_deref() == Some(name.as_str()) {
                    self.selected.set(Some(to));
                }
            }
            Event::ContactRemoved { name } => {
                self.contacts.write().retain(|n| *n != name);
                self.messages.write().remove(&name);
                self.unread.write().remove(&name);
                self.links.write().remove(&name);
                self.people.write().remove(&name);
                if self.selected.read().as_deref() == Some(name.as_str()) {
                    self.selected.set(None);
                    self.panel.set(false);
                }
            }
            Event::PersonChanged { name, person } => {
                self.people.write().insert(name, person);
            }
            Event::Network { network } => self.network.set(network),
            Event::Link { name, state } => {
                self.links.write().insert(name, state);
            }
        }
    }
}

#[component]
fn App() -> Element {
    let mut state = use_context_provider(|| State {
        phase: Signal::new(Phase::Connecting(t!("connecting-daemon"))),
        contacts: Signal::new(Vec::new()),
        note: Signal::new(None),
        unread: Signal::new(HashMap::new()),
        selected: Signal::new(None),
        messages: Signal::new(HashMap::new()),
        dialog: Signal::new(None),
        status: Signal::new(String::new()),
        codes: Signal::new(Vec::new()),
        network: Signal::new(Network::default()),
        links: Signal::new(HashMap::new()),
        people: Signal::new(HashMap::new()),
        arrival: Signal::new(None),
        theme: Signal::new(Theme::load()),
        prefs: Signal::new(Prefs::load(dir())),
        view: Signal::new(View::Chat),
        panel: Signal::new(false),
        maximized: Signal::new(false),
        read_receipts: Signal::new(false),
        typing_indicators: Signal::new(false),
        typing: Signal::new(HashMap::new()),
        language: Signal::new(i18n::language()),
    });

    use_future(move || run(state, None, None));
    #[cfg(target_os = "linux")]
    use_hook(chrome::cut_corners);
    #[cfg(target_os = "linux")]
    use_hook(chrome::no_pinch_zoom);
    let desktop = dioxus::desktop::window();
    dioxus::desktop::use_wry_event_handler(move |event, _| {
        use dioxus::desktop::tao::event::{Event, WindowEvent};
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } if state.prefs.peek().lock_on_close => shut_down(),
            Event::WindowEvent { event: WindowEvent::Focused(true), .. } => state.mark_read(),
            Event::WindowEvent { event: WindowEvent::Resized(_), .. } => {
                let maximized = desktop.is_maximized();
                if *state.maximized.peek() != maximized {
                    state.maximized.set(maximized);
                }
            }
            _ => {}
        }
    });

    let theme = *state.theme.read();
    let language = *state.language.read();
    rsx! {
        style { {STYLE.as_str()} }
        // Keyed by language: a new language draws every word again.
        for language in [language] {
            div { key: "{language}", class: "root", lang: language, "data-theme": theme.name(), "data-maximized": "{state.maximized}", "data-platform": std::env::consts::OS,
                if *state.phase.read() != Phase::Ready {
                    div { class: "drag-strip", onmousedown: chrome::drag, ondoubleclick: chrome::toggle_maximized }
                }
                match state.phase.read().clone() {
                    Phase::Connecting(what) => rsx! {
                        div { class: "splash",
                            p { class: "wordmark", "Nightpost" }
                            p { "{what}" }
                            button { class: "quiet", onclick: move |_| state.dialog.set(Some(Dialog::Bridges)), {t!("bridges-button")} }
                        }
                        DialogView {}
                    },
                    Phase::Locked(why) => rsx! { Locked { why } },
                    Phase::Failed(why) => rsx! { div { class: "splash", p { class: "wordmark", "Nightpost" } p { class: "warning", "{why}" } } },
                    Phase::Ready => rsx! {
                        div { class: if state.prefs.read().side_hidden { "app side-hidden" } else { "app" },
                            SideToggle {}
                            Sidebar {}
                            match *state.view.read() {
                                View::Chat => rsx! { Conversation {} },
                                View::Settings(section) => rsx! { SettingsPage { section } },
                            }
                        }
                        DialogView {}
                    },
                }
                if cfg!(target_os = "macos") {
                    div { class: "titlebar", onmousedown: chrome::drag, ondoubleclick: chrome::toggle_maximized }
                } else {
                    Chrome {}
                }
            }
        }
    }
}

/// Folds the list of people away, or brings it back; remembered.
#[component]
fn SideToggle() -> Element {
    let mut state = use_context::<State>();
    let hidden = state.prefs.read().side_hidden;
    let label = if hidden { t!("show-list") } else { t!("hide-list") };
    rsx! {
        button {
            class: "side-toggle",
            title: "{label}",
            "aria-label": "{label}",
            "aria-expanded": if hidden { "false" } else { "true" },
            onclick: move |_| {
                let p = *state.prefs.peek();
                state.prefs.set(Prefs { side_hidden: !p.side_hidden, ..p }.save(dir()));
            },
            svg { width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
                rect { x: "3", y: "3", width: "18", height: "18", rx: "2" }
                path { d: "M9 3v18" }
            }
        }
    }
}

/// Attaches to the daemon (starting it, with the passphrase if given) and
/// follows its events for as long as it runs.
async fn run(mut state: State, passphrase: Option<String>, recovery: Option<String>) {
    let mut events = match attach(state.phase, passphrase, recovery).await {
        Ok(Some(events)) => events,
        Ok(None) => return,
        Err(e) => {
            state.phase.set(Phase::Failed(format!("{e:#}")));
            return;
        }
    };
    match call(Request::Status).await {
        Ok(Reply::Status { network, links }) => {
            state.network.set(network);
            state.links.set(links.into_iter().collect());
        }
        other => state.fail(other),
    }
    match call(Request::ReadReceipts { on: None }).await {
        Ok(Reply::ReadReceipts { on }) => state.read_receipts.set(on),
        other => state.fail(other),
    }
    match call(Request::TypingIndicators { on: None }).await {
        Ok(Reply::TypingIndicators { on }) => state.typing_indicators.set(on),
        other => state.fail(other),
    }
    state.reload_contacts().await;
    state.phase.set(Phase::Ready);
    while let Ok(Some(event)) = events.event().await {
        state.on_event(event);
    }
    state.phase.set(Phase::Failed(t!("daemon-stopped")));
}

/// Asks for the passphrase, then starts the daemon with it.
#[component]
fn Locked(why: String) -> Element {
    let state = use_context::<State>();
    let mut passphrase = use_signal(String::new);
    let mut words = use_signal(String::new);
    // Offered after a refusal: the right passphrase can meet another keychain.
    let mut recover = use_signal(|| false);
    let has_passphrase = dbkey::uses_passphrase(dir());
    let mut unlock = move || {
        let given = passphrase.read().clone();
        let recovery = recover.read().then(|| words.read().split_whitespace().collect::<Vec<_>>().join(" "));
        if (has_passphrase && given.is_empty()) || recovery.as_ref().is_some_and(String::is_empty) {
            return;
        }
        passphrase.set(String::new());
        words.set(String::new());
        // Not `spawn`: this screen goes away as the daemon starts.
        dioxus::core::spawn_forever(run(state, has_passphrase.then_some(given), recovery));
    };
    rsx! {
        form { class: "splash", onsubmit: move |e| { e.prevent_default(); unlock(); },
            p { class: "wordmark", "Nightpost" }
            if has_passphrase {
                label { r#for: "passphrase", {t!("unlock-prompt")} }
                input { id: "passphrase", class: "name-field", r#type: "password", autofocus: !*recover.read(), value: "{passphrase}", oninput: move |e| passphrase.set(e.value()) }
            }
            if *recover.read() {
                label { r#for: "words", {t!("recovery-prompt")} }
                textarea { id: "words", class: "lines-field", rows: "3", autofocus: true, spellcheck: false, value: "{words}", oninput: move |e| words.set(e.value()) }
            }
            if !why.is_empty() { p { class: "warning", "{why}" } }
            button { class: "primary", r#type: "submit", {t!("unlock")} }
            if !why.is_empty() && !*recover.read() {
                button { class: "ghost", r#type: "button", onclick: move |_| recover.set(true), {t!("recovery-use")} }
            }
        }
    }
}

#[component]
fn Sidebar() -> Element {
    let mut state = use_context::<State>();
    let selected = state.selected.read().clone();
    // Most recent conversation first, then those not started yet.
    let mut rows: Vec<(String, Option<HistoryItem>)> = {
        let messages = state.messages.read();
        state.contacts.read().iter().map(|n| (n.clone(), messages.get(n).and_then(|m| m.last().cloned()))).collect()
    };
    let note = state.note.read().clone();
    // The note first, then the latest.
    rows.sort_by(|a, b| (note.as_ref() != Some(&a.0)).cmp(&(note.as_ref() != Some(&b.0))).then_with(|| b.1.as_ref().map(|m| m.time).cmp(&a.1.as_ref().map(|m| m.time))).then_with(|| a.0.cmp(&b.0)));
    let net = state.network.read().clone();
    let people = rows.iter().filter(|(n, _)| note.as_ref() != Some(n)).count();
    let reachable = rows.iter().filter(|(n, _)| note.as_ref() != Some(n) && reached(&net, state.links.read().get(n).copied())).count();
    let (title, _) = net.summary();
    let dot = if net.entry == Some(false) || net.circuit == Some(false) {
        "net-dot fail"
    } else if net.percent < 100 {
        "net-dot live"
    } else {
        "net-dot"
    };
    let in_settings = matches!(*state.view.read(), View::Settings(_));
    let count = (people > 0).then(|| t!("reachable-count", reachable = reachable, total = people));
    rsx! {
        aside {
            div { class: "brand", onmousedown: chrome::drag, ondoubleclick: chrome::toggle_maximized,
                p { class: "wordmark", "Nightpost" }
            }
            nav {
                for (name, last) in rows {
                    {
                        let unread = state.unread.read().get(&name).copied().unwrap_or(0);
                        let mut class = String::from("contact");
                        if !in_settings && selected.as_deref() == Some(name.as_str()) { class.push_str(" selected"); }
                        if unread > 0 { class.push_str(" unread"); }
                        let preview = match &last {
                            Some(m) if m.outgoing => t!("preview-you", text = m.text.lines().next().unwrap_or_default()),
                            Some(m) => m.text.lines().next().unwrap_or_default().to_string(),
                            None => t!("no-messages"),
                        };
                        let when = last.as_ref().map(|m| short_time(m.time)).unwrap_or_default();
                        let is_note = note.as_ref() == Some(&name);
                        let on = !is_note && reached(&net, state.links.read().get(&name).copied());
                        let photo = state.people.read().get(&name).and_then(|p| p.photo.clone());
                        let pick = name.clone();
                        rsx! {
                            button { key: "{name}", class: "{class}",
                                onclick: move |_| {
                                    state.view.set(View::Chat);
                                    state.select(pick.clone());
                                },
                                span { class: "avatar",
                                    {avatar(&name, photo.as_deref(), 44)}
                                    if !is_note { span { class: if on { "dot on" } else { "dot" } } }
                                }
                                span { class: "lines",
                                    span { class: "name", "{name}" }
                                    span { class: "when", "{when}" }
                                    span { class: "preview", "{preview}" }
                                    if unread > 0 { span { class: "count", "{unread}" } }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "footer",
                // Who is reachable sits with how Tor is: the top has the name and its logo.
                div { class: "footer-status",
                    button { class: "net-status", onclick: move |_| state.view.set(View::Settings(Section::Network)),
                        span { class: dot }
                        "{title}"
                    }
                    if let Some(count) = count { span { class: "reach-count", "{count}" } }
                }
                div { class: "footer-row",
                    button { class: "primary", onclick: move |_| state.dialog.set(Some(Dialog::Add)), {t!("add-someone")} }
                    button {
                        class: if in_settings { "gear on" } else { "gear" },
                        title: t!("settings"),
                        "aria-label": t!("settings"),
                        onclick: move |_| state.view.set(if in_settings { View::Chat } else { View::Settings(Section::Appearance) }),
                        svg { width: "20", height: "20", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
                            circle { cx: "12", cy: "12", r: "3" }
                            path { d: "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" }
                        }
                    }
                }
            }
        }
    }
}

/// Reached since the daemon started: open now, or closed after use.
/// As the conversation's header says it: a link counts only while Tor is up,
/// not from before Tor went down and started over.
fn reached(net: &Network, link: Option<LinkState>) -> bool {
    reach_line("", net, link).0 == "on"
}


/// The photo chosen for them, or three rings, the same for a name on every
/// machine: an onion of one's own.
fn avatar(name: &str, photo: Option<&str>, size: u32) -> Element {
    if let Some(photo) = photo {
        // Our own re-encoded PNG (`person::photo_uri`), checked by the daemon.
        return rsx! { img { class: "photo", width: "{size}", height: "{size}", src: "{photo}", alt: "" } };
    }
    let [a, b, c] = nightpost_core::rings(name);
    rsx! {
        svg { width: "{size}", height: "{size}", view_box: "0 0 46 46", "aria-hidden": "true",
            circle { cx: "23", cy: "23", r: "23", fill: a }
            circle { cx: "23", cy: "23", r: "16", fill: b }
            circle { cx: "23", cy: "23", r: "9", fill: c }
        }
    }
}

/// A line of the thread: a day, or a message with or without its time
/// (messages from one side within five minutes share the last one's).
enum Row {
    Day(String),
    Slip { m: HistoryItem, meta: bool },
}

fn rows(messages: &[HistoryItem]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        let day = date(m.time);
        if i == 0 || date(messages[i - 1].time) != day {
            rows.push(Row::Day(day_label(m.time)));
        }
        let last = messages.get(i + 1).is_none_or(|n| n.outgoing != m.outgoing || n.time.saturating_sub(m.time) > 5 * 60_000 || date(n.time) != day);
        rows.push(Row::Slip { m: m.clone(), meta: last || (m.outgoing && !m.delivered) });
    }
    rows
}

/// The steps on a 900 × 140 drawing, rising and falling like a route.
const STEPS: [(f32, f32); 6] = [(80.0, 62.0), (230.0, 44.0), (390.0, 78.0), (530.0, 44.0), (670.0, 78.0), (820.0, 62.0)];

fn hop_path(i: usize) -> String {
    let ((x0, y0), (x1, y1)) = (STEPS[i], STEPS[i + 1]);
    let mid = (x0 + x1) / 2.0;
    format!("M{x0} {y0} C{mid} {y0} {mid} {y1} {x1} {y1}")
}

fn whole_path() -> String {
    let mut d = format!("M{} {}", STEPS[0].0, STEPS[0].1);
    for w in STEPS.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        let mid = (x0 + x1) / 2.0;
        d += &format!(" C{mid} {y0} {mid} {y1} {x1} {y1}");
    }
    d
}

/// Lucide-style line icons (the handshake is Lucide's, ISC licence).
fn step_icon(i: usize) -> Element {
    rsx! {
        svg { view_box: "0 0 24 24", "aria-hidden": "true",
            match i {
                0 => rsx! { path { d: "M5 6.5h14v8.5H5z" } path { d: "M3 18h18" } },
                1 => rsx! { path { d: "M12 3l7 2.8v5.6c0 4.4-3 7.7-7 9.4-4-1.7-7-5-7-9.4V5.8z", stroke_dasharray: "2.4 2.2" } circle { cx: "12", cy: "11", r: "2.2" } path { d: "M12 13.2V16" } },
                2 | 3 => rsx! { path { d: "M12 3l7 2.8v5.6c0 4.4-3 7.7-7 9.4-4-1.7-7-5-7-9.4V5.8z" } circle { cx: "12", cy: "11.5", r: "2.6" } path { d: "M5 11.5h4.4M14.6 11.5H19" } },
                4 => rsx! { path { d: "m11 17 2 2a1 1 0 1 0 3-3" } path { d: "m14 14 2.5 2.5a1 1 0 1 0 3-3l-3.88-3.88a3 3 0 0 0-4.24 0l-.88.88a1 1 0 1 1-3-3l2.81-2.81a5.79 5.79 0 0 1 7.06-.87l.47.28a2 2 0 0 0 1.42.25L21 4" } path { d: "m21 3 1 11h-2" } path { d: "M3 3 2 14l6.5 6.5a1 1 0 1 0 3-3" } path { d: "M3 4h8" } },
                _ => rsx! { rect { x: "7", y: "3", width: "10", height: "18", rx: "2.2" } path { d: "M11 18h2" } },
            }
        }
    }
}

#[component]
fn RouteView(name: String) -> Element {
    let state = use_context::<State>();
    let net = state.network.read().clone();
    let link = state.links.read().get(&name).copied();
    let route = Route::new(&net, link);
    let pending = state.messages.read().get(&name).is_some_and(|ms| ms.iter().any(|m| m.outgoing && !m.delivered));
    // Going out while their device is being reached; never towards someone
    // known to be away: those messages just wait.
    let sending = pending && matches!(link, Some(LinkState::Trying | LinkState::Open));
    let arrival = state.arrival.read().as_ref().filter(|a| a.0 == name).map(|a| a.1);
    let line = net.line.strip_prefix("Tor: ").unwrap_or(&net.line).to_string();
    let entrance = if net.bridged { t!("step-private-entrance") } else { t!("step-entrance") };
    let tips: [(String, String); 6] = [
        (t!("step-computer"), t!("step-computer-help", name = name)),
        (
            entrance.clone(),
            if net.bridged { t!("step-bridge-help") } else { t!("step-guard-help") },
        ),
        (t!("step-relay"), t!("step-relay-1-help")),
        (t!("step-relay"), t!("step-relay-2-help")),
        (t!("step-meeting"), t!("step-meeting-help", name = name)),
        (name.clone(), t!("step-device-help", name = name)),
    ];
    let labels = [t!("step-you"), entrance, t!("step-relay"), t!("step-relay"), t!("step-meeting"), name.clone()];
    // What the step in orange or red is doing, for its tooltip.
    let now = match (route.live, route.fail) {
        (Some(0), _) | (_, Some(_)) => line.clone(),
        (Some(_), _) => t!("reaching", name = name),
        _ => String::new(),
    };
    rsx! {
        // Measured by the narrow layout in style.css.
        div { class: "route-box",
            div { class: "route", role: "group", "aria-label": t!("route-label", name = name),
                svg { view_box: "0 0 900 140", "aria-hidden": "true",
                    for i in 0..5 {
                        g { key: "{i}",
                            path { class: "band {route.hop(i)}", d: hop_path(i) }
                            path { class: "wire {route.hop(i)}", d: hop_path(i) }
                        }
                    }
                    if sending { path { class: "pulse out", d: whole_path(), "pathLength": "1000" } }
                    if let Some(n) = arrival { path { key: "in{n}", class: "pulse in", d: whole_path(), "pathLength": "1000" } }
                }
                for (i, ((title, body), label)) in tips.into_iter().zip(labels).enumerate() {
                    {
                        let (x, y) = STEPS[i];
                        let mut class = format!("step {}", route.step(i));
                        if i == 0 || i == 5 { class.push_str(" device"); }
                        if i == 0 { class.push_str(" first"); }
                        if i == 5 { class.push_str(" last"); }
                        let status = if route.step(i) == "live" || route.step(i) == "fail" { now.clone() } else { String::new() };
                        rsx! {
                            div { key: "step{i}", class: "{class}", style: "left: {x / 9.0}%; --y: {y / 140.0}",
                                button { "aria-label": "{title}", "aria-describedby": "tip{i}", {step_icon(i)} }
                                span { class: "label", "{label}" }
                                span { class: "tip", id: "tip{i}", role: "tooltip",
                                    strong { "{title}" }
                                    span { "{body}" }
                                    if !status.is_empty() { em { "{status}" } }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Under the name: how things stand with them.
fn reach_line(name: &str, net: &Network, link: Option<LinkState>) -> (&'static str, String) {
    let route = Route::new(net, link);
    if route.fail.is_some() {
        ("fail", t!("net-blocked"))
    } else if route.live == Some(0) {
        ("live", t!("connecting-percent", percent = net.percent))
    } else {
        match link {
            Some(LinkState::Open) => ("on", t!("link-open")),
            Some(LinkState::Idle) => ("on", t!("link-idle")),
            Some(LinkState::Trying) => ("live", t!("reaching", name = name)),
            Some(LinkState::Unreachable) => ("", t!("link-away")),
            Some(LinkState::Older) => ("fail", t!("link-older", name = name)),
            Some(LinkState::Newer) => ("fail", t!("link-newer", name = name)),
            None => ("", t!("link-unknown")),
        }
    }
}

#[component]
fn Conversation() -> Element {
    let mut state = use_context::<State>();
    let mut draft = use_signal(String::new);
    // When we last told them we are writing.
    let mut pinged = use_signal(|| None::<std::time::Instant>);
    let Some(name) = state.selected.read().clone() else {
        return rsx! {
            main {
                div { class: "drag-strip", onmousedown: chrome::drag, ondoubleclick: chrome::toggle_maximized }
                div { class: "empty",
                h2 { {t!("empty-title")} }
                p { {t!("empty-help")} }
                button { class: "primary", onclick: move |_| state.dialog.set(Some(Dialog::Add)), {t!("add-someone")} }
            }
            p { class: "status", "{state.status}" } }
        };
    };
    let is_note = state.note.read().as_deref() == Some(name.as_str());
    let alone = state.contacts.read().len() <= 1;
    let rows = rows(state.messages.read().get(&name).map(Vec::as_slice).unwrap_or_default());
    let typing = state.typing.read().contains_key(&name);
    let mut typed = {
        let name = name.clone();
        move |text: String| {
            // At most one ping every few seconds, as UmbraChat did.
            let quiet = pinged.read().is_none_or(|at| at.elapsed() >= Duration::from_secs(4));
            if *state.typing_indicators.read() && quiet && !text.trim().is_empty() {
                pinged.set(Some(std::time::Instant::now()));
                let name = name.clone();
                spawn(async move {
                    let _ = call(Request::Typing { name }).await;
                });
            }
            draft.set(text);
        }
    };
    let link = state.links.read().get(&name).copied();
    let (reach_class, reach) = if is_note { ("", t!("note-here")) } else { reach_line(&name, &state.network.read(), link) };
    let waiting = if matches!(link, Some(LinkState::Unreachable | LinkState::Older | LinkState::Newer)) { t!("waiting-reachable", name = name) } else { t!("sending") };

    let send = {
        let name = name.clone();
        move || {
            let text = draft.read().trim_end().to_string();
            if text.trim().is_empty() {
                return;
            }
            draft.set(String::new());
            let to = name.clone();
            spawn(async move {
                match call(Request::Send { to, text: text.clone() }).await {
                    // The message itself comes back as Event::Sent.
                    Ok(Reply::Sent { .. }) => {}
                    other => {
                        draft.set(text);
                        state.fail(other);
                    }
                }
            });
        }
    };
    let mut send_key = send.clone();
    let mut send_click = send;
    let person = state.people.read().get(&name).cloned().unwrap_or_default();
    let open = *state.panel.read();
    let toggle = move |_| {
        let open = *state.panel.read();
        state.panel.set(!open);
    };

    rsx! {
        main {
            header { class: "talk-head",
                button { class: if open { "who open" } else { "who" }, "aria-label": t!("about-name", name = name), onclick: toggle,
                    {avatar(&name, person.photo.as_deref(), 48)}
                    span { class: "who-text",
                        span { class: "who-name",
                            h2 { "{name}" }
                            // Each mark says what it means in a bubble like the route's.
                            if person.verified {
                                span { class: "mark-wrap",
                                    svg { class: "mark verified", view_box: "0 0 24 24", role: "img", "aria-label": t!("verified"),
                                        path { d: "M12 3l7 2.8v5.6c0 4.4-3 7.7-7 9.4-4-1.7-7-5-7-9.4V5.8z" }
                                        path { d: "M8.5 12l2.5 2.5 4.5-5" }
                                    }
                                    span { class: "tip", role: "tooltip",
                                        strong { {t!("verified-title")} }
                                        span { {t!("verified-help", name = name)} }
                                    }
                                }
                            }
                            if let Some(until) = muted(&person) {
                                span { class: "mark-wrap",
                                    // The whole bell, cut where the bar crosses it, so the bar
                                    // reads on any background.
                                    svg { class: "mark bell", view_box: "0 0 24 24", role: "img", "aria-label": t!("muted"),
                                        defs {
                                            mask { id: "bell-cut",
                                                rect { width: "24", height: "24", fill: "white", stroke: "none" }
                                                path { d: "M3 3l18 18", stroke: "black", stroke_width: "5" }
                                            }
                                        }
                                        g { "mask": "url(#bell-cut)",
                                            path { d: "M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" }
                                            path { d: "M10.3 21a1.94 1.94 0 0 0 3.4 0" }
                                        }
                                        path { d: "M3 3l18 18" }
                                    }
                                    span { class: "tip", role: "tooltip",
                                        strong { if until == u64::MAX { {t!("muted")} } else { {t!("muted-until", time = clock(until))} } }
                                        span { {t!("muted-help", name = name)} }
                                    }
                                }
                            }
                        }
                        span { class: "state {reach_class}", "{reach}" }
                    }
                }
                if !is_note { button { class: if open { "info on" } else { "info" }, title: t!("about-name", name = name), "aria-label": t!("about-name", name = name), onclick: toggle,
                    svg { width: "20", height: "20", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", "aria-hidden": "true",
                        circle { cx: "12", cy: "12", r: "9" }
                        path { d: "M12 11v5M12 7.6v.1" }
                    }
                } }
                div { class: "drag-fill", onmousedown: chrome::drag, ondoubleclick: chrome::toggle_maximized }
            }
            if !is_note { RouteView { name: name.clone() } }
            // Reversed column: the newest message sits at the bottom and the
            // view stays there as messages arrive.
            div { class: "thread",
                div { class: "column",
                    if is_note && rows.is_empty() {
                        div { class: "empty note",
                            p { {t!("note-empty-window")} }
                            // Only the note so far: how to find someone is here too.
                            if alone { button { class: "primary", onclick: move |_| state.dialog.set(Some(Dialog::Add)), {t!("add-someone")} } }
                        }
                    }
                    if typing {
                        div { class: "bubble-row in",
                            div { class: "bubble typing", "aria-hidden": "true", i {} i {} i {} }
                            span { class: "sr", role: "status", {t!("typing", name = name)} }
                        }
                    }
                    for row in rows.into_iter().rev() {
                        match row {
                            Row::Day(label) => rsx! { p { key: "day-{label}", class: "day", "{label}" } },
                            Row::Slip { m, meta } => rsx! {
                                div { key: "{m.id}", class: if m.outgoing { "bubble-row out" } else { "bubble-row in" },
                                    div { class: "bubble", "{m.text}" }
                                    if meta {
                                        p { class: "meta",
                                            "{clock(m.time)}"
                                            if m.outgoing && !m.delivered {
                                                svg { class: "tick", view_box: "0 0 16 16", "aria-hidden": "true", circle { cx: "8", cy: "8", r: "6" } }
                                                span { class: "waiting", "{waiting}" }
                                            } else if m.outgoing && !is_note {
                                                // Signal's marks: two rings once delivered, filled once read.
                                                svg { class: if m.read { "tick read" } else { "tick" }, view_box: "2 0 24 16", role: "img",
                                                    "aria-label": if m.read { t!("read") } else { t!("delivered") },
                                                    circle { cx: "10", cy: "8", r: "6" }
                                                    path { d: "M7.3 8.2l1.8 1.8 3.6-3.8" }
                                                    circle { class: "gap", cx: "18", cy: "8", r: "7.6" }
                                                    circle { class: "front", cx: "18", cy: "8", r: "6" }
                                                    path { d: "M15.3 8.2l1.8 1.8 3.6-3.8" }
                                                }
                                            }
                                        }
                                    }
                                }
                            },
                        }
                    }
                }
            }
            // One side must update: said where the eye goes to write.
            if let Some((title, help)) = match link {
                Some(LinkState::Older) => Some((t!("update-older-title", name = name), t!("update-older-help", name = name))),
                Some(LinkState::Newer) => Some((t!("update-newer-title"), t!("update-newer-help", name = name))),
                _ => None,
            } {
                div { class: "update-notice", role: "status",
                    svg { view_box: "0 0 24 24", "aria-hidden": "true",
                        circle { cx: "12", cy: "12", r: "9" }
                        path { d: "M16 12l-4-4-4 4M12 16V8" }
                    }
                    span { class: "card-text",
                        strong { "{title}" }
                        span { "{help}" }
                    }
                }
            }
            div { class: "composer",
                textarea {
                    "aria-label": t!("message-to", name = name),
                    placeholder: t!("write-to", name = name),
                    value: "{draft}",
                    rows: "1",
                    oninput: move |e| typed(e.value()),
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Enter && !e.modifiers().contains(Modifiers::SHIFT) {
                            e.prevent_default();
                            send_key();
                        }
                    },
                }
                button { class: "primary", onclick: move |_| send_click(), {t!("send")} }
            }
            p { class: "status", "{state.status}" }
            if open && !is_note { PersonPanel { key: "{name}", name: name.clone() } }
        }
    }
}

/// Until when notifications from them are off, if they are now.
fn muted(person: &Person) -> Option<u64> {
    person.muted_until.filter(|until| *until > now_millis())
}

fn now_millis() -> u64 {
    jiff::Timestamp::now().as_millisecond() as u64
}

#[component]
fn DialogView() -> Element {
    let mut state = use_context::<State>();
    let Some(dialog) = state.dialog.read().clone() else { return rsx! {} };
    let close = move |_| state.dialog.set(None);
    let body = match dialog {
        Dialog::Add => rsx! { AddDialog {} },
        Dialog::Bridges => rsx! {
            h3 { {t!("bridges")} }
            div { class: "block", BridgesEditor {} }
            div { class: "actions", button { onclick: close, {t!("close")} } }
        },
        Dialog::Safety { name, number } => return rsx! { SafetyNumber { name, number } },
        Dialog::Invite { name, link } => rsx! {
            h3 { {t!("invite-title", name = name)} }
            p { {t!("invite-help", name = name)} }
            img { class: "qr", src: "{qr(&link)}", alt: t!("invite-qr") }
            textarea { class: "link", readonly: true, rows: "4", "aria-label": t!("invite-link"), value: "{link}" }
            div { class: "actions",
                CopyButton { text: link.clone(), label: t!("copy-link") }
                button { onclick: close, {t!("done")} }
            }
        },
        Dialog::Code { name, code } => rsx! {
            h3 { {t!("code-title", name = name)} }
            p { {t!("code-help")} }
            p { class: "code", "{code.replace('-', \" \")}" }
            p { {t!("code-after", name = name)} }
            div { class: "actions",
                CopyButton { text: code.clone(), label: t!("copy-code") }
                button { onclick: close, {t!("hide")} }
            }
        },
        Dialog::Confirm { name, number } => {
            let (keep, refuse) = (name.clone(), name.clone());
            rsx! {
                h3 { {t!("confirm-title", name = name)} }
                p { {t!("confirm-help")} }
                p { class: "number", "{groups(&number)}" }
                p { class: "warning", {t!("confirm-warning", name = name)} }
                div { class: "actions",
                    button { class: "primary", onclick: move |_| confirm(state, keep.clone(), true), {t!("digits-match")} }
                    button { class: "danger", onclick: move |_| confirm(state, refuse.clone(), false), {t!("digits-differ")} }
                }
            }
        }
        Dialog::Waiting { name, number } => rsx! {
            h3 { {t!("waiting-title", name = name)} }
            p { {t!("waiting-help", name = name)} }
            p { class: "number", "{groups(&number)}" }
            p { {t!("waiting-after", name = name)} }
        },
    };
    rsx! {
        div { class: "overlay",
            div { class: "sheet", role: "dialog", {body} }
        }
    }
}

/// The digits to read out, over everything, on a blurred veil.
#[component]
fn SafetyNumber(name: String, number: String) -> Element {
    let mut state = use_context::<State>();
    let photo = state.people.read().get(&name).and_then(|p| p.photo.clone());
    let who = name.clone();
    let close = move |_| state.dialog.set(None);
    rsx! {
        div { class: "full", role: "dialog", "aria-label": t!("safety-label", name = name),
            button { class: "full-close", "aria-label": t!("close"), onclick: close,
                svg { width: "22", height: "22", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", "aria-hidden": "true", path { d: "M6 6l12 12M18 6L6 18" } }
            }
            div { class: "full-body",
                {avatar(&name, photo.as_deref(), 64)}
                h1 { {t!("safety-title", name = name)} }
                p { class: "full-help", {t!("safety-help")} }
                p { class: "full-number", "{groups(&number)}" }
                div { class: "actions",
                    button { class: "primary", onclick: move |_| {
                        state.dialog.set(None);
                        person::update(state, who.clone(), |p| p.verified = true);
                    }, {t!("they-match")} }
                    button { class: "ghost", onclick: close, {t!("not-now")} }
                }
                p { class: "full-warn", {t!("safety-warning", name = name)} }
            }
        }
    }
}

#[component]
fn AddDialog() -> Element {
    let mut state = use_context::<State>();
    let mut name = use_signal(String::new);
    let mut pasted = use_signal(String::new);
    // What is missing, said here: the dialog stays open until it is fixed.
    let mut error = use_signal(String::new);
    // Using what they gave: shown here rather than in the status line, out of sight.
    let mut looking = use_signal(|| false);

    let invite = move |by_code: bool| {
        let who = name.read().trim().to_string();
        if who.is_empty() {
            return error.set(t!("need-name"));
        }
        spawn(async move {
            let request = if by_code { Request::InviteCode { name: who.clone() } } else { Request::Invite { name: who.clone() } };
            match call(request).await {
                Ok(Reply::Invite { link }) => state.dialog.set(Some(Dialog::Invite { name: who, link })),
                Ok(Reply::Code { code }) => {
                    state.codes.write().push(who.clone());
                    state.dialog.set(Some(Dialog::Code { name: who, code }));
                }
                other => state.fail(other),
            }
        });
    };
    let accept = move |_| {
        let (who, given) = (name.read().trim().to_string(), pasted.read().trim().to_string());
        if given.is_empty() {
            return error.set(t!("need-given"));
        }
        if who.is_empty() {
            return error.set(t!("need-name-top"));
        }
        // A code is digits and separators; anything else is taken for a link.
        let request = if given.chars().all(|c| c.is_ascii_digit() || " -.".contains(c)) {
            Request::AcceptCode { code: given, name: who.clone() }
        } else {
            Request::Accept { link: given, name: who.clone() }
        };
        error.set(String::new());
        looking.set(true);
        // Not `spawn`: the dialog may be closed (or, with a code, replaced by
        // the safety number) while the daemon looks; then the status line
        // gets the answer.
        dioxus::core::spawn_forever(async move {
            let reply = call(request).await;
            let shown = looking.try_write().map(|mut l| *l = false).is_ok();
            match reply {
                Ok(Reply::Added { name }) => {
                    if matches!(*state.dialog.peek(), Some(Dialog::Add)) {
                        state.dialog.set(None);
                    }
                    state.status.set(t!("added", name = name));
                }
                Ok(Reply::Error { message }) if shown => error.set(message),
                Err(e) if shown => error.set(format!("{e:#}")),
                other => state.fail(other),
            }
        });
    };
    if *looking.read() {
        let network = state.network.read();
        let who = name.read().trim().to_string();
        let progress = if network.percent < 100 {
            t!("looking-tor", percent = network.percent)
        } else {
            t!("looking-help", name = who)
        };
        return rsx! {
            h3 { {t!("looking-title", name = who)} }
            p { role: "status", "{progress}" }
            p { class: "help", {t!("looking-close", name = who)} }
            div { class: "actions", button { onclick: move |_| state.dialog.set(None), {t!("close")} } }
        };
    }
    let (mut by_link, mut by_code) = (invite, invite);

    rsx! {
        h3 { {t!("add-title")} }
        label { {t!("add-name-label")} }
        input { autofocus: true, value: "{name}", placeholder: t!("add-name-placeholder"), oninput: move |e| { name.set(e.value()); error.set(String::new()); } }
        div { class: "actions",
            button { class: "primary", onclick: move |_| by_link(false), {t!("create-link")} }
            button { onclick: move |_| by_code(true), {t!("create-code")} }
        }
        hr {}
        label { {t!("add-use-label")} }
        input { value: "{pasted}", placeholder: "nightpost:1:… or 482-109-375-512", oninput: move |e| { pasted.set(e.value()); error.set(String::new()); } }
        if !error.read().is_empty() { p { class: "warning", role: "alert", "{error}" } }
        div { class: "actions",
            button { class: "primary", onclick: accept, {t!("use-it")} }
            button { onclick: move |_| state.dialog.set(None), {t!("cancel")} }
        }
    }
}

fn confirm(mut state: State, name: String, keep: bool) {
    state.dialog.set(None);
    state.codes.write().retain(|n| *n != name);
    spawn(async move {
        match call(Request::Confirm { name: name.clone(), keep }).await {
            Ok(Reply::Done) => state.status.set(if keep { t!("kept", name = name) } else { t!("turned-down", name = name) }),
            other => state.fail(other),
        }
    });
}

/// Stops the daemon and waits for its answer, on a thread of its own: this
/// runs as the window closes, when nothing else will poll a future.
pub fn shut_down() {
    let _ = std::thread::spawn(|| -> Result<Reply> {
        tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(call(Request::Shutdown))
    })
    .join();
}

/// One request on its own connection.
async fn call(request: Request) -> Result<Reply> {
    Client::connect(dir()).await?.call(&request).await
}

/// The event stream, starting the daemon first if none answers. `None`:
/// the daemon needs the passphrase (again) and the phase now asks for it.
async fn attach(mut phase: Signal<Phase>, passphrase: Option<String>, recovery: Option<String>) -> Result<Option<Client>> {
    // A daemon left running from before an update: it would not know what
    // this window asks. Stop it; the one next to this program takes over.
    if api::running_version(dir()).as_deref() != Some(nightpost_core::VERSION) && Client::connect(dir()).await.is_ok() {
        phase.set(Phase::Connecting(t!("restarting-update")));
        call(Request::Shutdown).await.ok();
        for _ in 0..20 {
            if Client::connect(dir()).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    if let Ok(client) = listen().await {
        return Ok(Some(client));
    }
    // It answers, but not this yet: a daemon is starting Tor.
    let starting = Client::connect(dir()).await.is_ok();
    let asked = passphrase.is_some() || recovery.is_some();
    if !starting && !asked && dbkey::uses_passphrase(dir()) {
        phase.set(Phase::Locked(String::new()));
        return Ok(None);
    }
    phase.set(Phase::Connecting(t!("starting")));
    let mut daemon = if starting { None } else { Some(start_daemon(passphrase, recovery)?) };
    let log = dir().join("daemon.log");
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Ok(client) = listen().await {
            return Ok(Some(client));
        }
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if let Some(status) = daemon.as_mut().map(std::process::Child::try_wait).transpose()?.flatten() {
            // A wrong key, or a keychain that does not have it (another
            // desktop): the words are the way out, offered on that screen.
            if (asked && text.contains("wrong key")) || text.contains(&t!("err-keychain-lost")) {
                phase.set(Phase::Locked(t!("wrong-passphrase")));
                return Ok(None);
            }
            bail!("the daemon stopped ({status}): {}", text.lines().last().unwrap_or_default());
        }
        // Its progress lines, not the log lines around them.
        let progress = text.lines().rev().find(|l| l.starts_with("Tor:")).map(String::from).unwrap_or_else(|| t!("starting"));
        phase.set(Phase::Connecting(progress));
    }
}

async fn listen() -> Result<Client> {
    let mut client = Client::connect(dir()).await?;
    match client.call(&Request::Listen).await? {
        Reply::Listening => Ok(client),
        other => bail!("unexpected reply {other:?}"),
    }
}

/// `nightpost daemon`, from next to this program, outliving it: closing the
/// window must not take the messages offline.
fn start_daemon(passphrase: Option<String>, recovery: Option<String>) -> Result<std::process::Child> {
    let exe = std::env::current_exe()?.with_file_name(format!("nightpost{}", std::env::consts::EXE_SUFFIX));
    let log = std::fs::File::create(dir().join("daemon.log"))?;
    let mut command = std::process::Command::new(&exe);
    let stdin = if passphrase.is_some() || recovery.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() };
    command.arg("daemon");
    if recovery.is_some() {
        command.arg("--recovery");
    }
    command.stdin(stdin).stdout(log.try_clone()?).stderr(log);
    as_launched(&mut command);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().with_context(|| format!("starting {}", exe.display()))?;
    // The passphrase, then the recovery words, a line each on its stdin and
    // then closed: never in its arguments or environment.
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write as _;
        for line in passphrase.into_iter().chain(recovery) {
            stdin.write_all(zeroize::Zeroizing::new(line + "\n").as_bytes())?;
        }
    }
    Ok(child)
}

/// Gives `command` the environment this program was started with, not the
/// one Dioxus set up for WebKit (`GDK_BACKEND=x11` under Wayland).
fn as_launched(command: &mut std::process::Command) {
    for (key, value) in OWN_ENV.get().into_iter().flatten() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
}

/// Shown by the system: it can be on a locked screen and kept in a history,
/// so what goes in is the user's choice (Settings, Notifications).
fn notify(title: &str, body: &str, prefs: Prefs) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        let mut notification = notify_rust::Notification::new();
        notification.summary(&title).body(&body);
        #[cfg(windows)]
        notification.app_id(nightpost_core::APP_ID);
        // Transient: shown, then let go rather than kept in the history.
        #[cfg(all(unix, not(target_os = "macos")))]
        if !prefs.keep_history {
            notification.hint(notify_rust::Hint::Transient(true));
        }
        if prefs.sound {
            // The freedesktop sound theme's name; macOS and Windows play their default.
            notification.sound_name("message-new-instant");
        }
        let _ = notification.show();
    });
}

/// Copies `text`, and says so for four seconds once the clipboard took it.
#[component]
fn CopyButton(text: String, label: String) -> Element {
    let mut copied = use_signal(|| false);
    let onclick = move |_| {
        // Our own link or code: Rust's quoting is a valid JS string literal.
        let quoted = format!("{text:?}");
        spawn(async move {
            let done = document::eval(&format!("try {{ await navigator.clipboard.writeText({quoted}); return true; }} catch {{ return false; }}")).await;
            if done.is_ok_and(|v| v.as_bool() == Some(true)) {
                copied.set(true);
                tokio::time::sleep(Duration::from_secs(4)).await;
                copied.set(false);
            }
        });
    };
    rsx! {
        // Both labels hold the width, so the buttons beside it stay put.
        button { class: "primary copy", "aria-live": "polite", onclick,
            span { class: if *copied.read() { "off" }, "{label}" }
            span { class: if !*copied.read() { "off" }, {t!("copied")} }
        }
    }
}

/// The link as an SVG QR code, ink on paper: not every phone reads it inverted.
fn qr(link: &str) -> String {
    use qrcode::render::svg;
    let Ok(code) = qrcode::QrCode::new(link.to_ascii_uppercase().as_bytes()) else { return String::new() };
    let image = code.render::<svg::Color>().min_dimensions(260, 260).dark_color(svg::Color("#10141d")).light_color(svg::Color("#ffffff")).build();
    format!("data:image/svg+xml;base64,{}", data_encoding::BASE64.encode(image.as_bytes()))
}

fn zoned(millis: u64) -> Option<jiff::Zoned> {
    Some(jiff::Timestamp::from_millisecond(millis as i64).ok()?.to_zoned(jiff::tz::TimeZone::system()))
}

fn date(millis: u64) -> Option<jiff::civil::Date> {
    zoned(millis).map(|z| z.date())
}

fn clock(millis: u64) -> String {
    zoned(millis).map(|z| z.strftime("%H:%M").to_string()).unwrap_or_default()
}

/// "Today", "Yesterday", "Monday 5 October", "5 October 2025".
fn day_label(millis: u64) -> String {
    let (Some(then), Some(today)) = (zoned(millis), zoned(jiff::Timestamp::now().as_millisecond() as u64)) else { return String::new() };
    let days = (today.date() - then.date()).get_days();
    match days {
        0 => t!("today"),
        1 => t!("yesterday"),
        _ if then.year() == today.year() => t!("date-this-year", weekday = weekday(&then), day = then.day(), month = month(&then, false)),
        _ => t!("date-other-year", day = then.day(), month = month(&then, false), year = then.year()),
    }
}

/// For the list: the time today, then "Yesterday", the weekday, the date.
fn short_time(millis: u64) -> String {
    let (Some(then), Some(today)) = (zoned(millis), zoned(jiff::Timestamp::now().as_millisecond() as u64)) else { return String::new() };
    match (today.date() - then.date()).get_days() {
        0 => then.strftime("%H:%M").to_string(),
        1 => t!("yesterday"),
        2..=6 => weekday(&then),
        _ => t!("date-short", day = then.day(), month = month(&then, true)),
    }
}

fn weekday(day: &jiff::Zoned) -> String {
    i18n::weekday(day.weekday().to_monday_one_offset(), false)
}

fn month(day: &jiff::Zoned, short: bool) -> String {
    i18n::month(day.month(), short)
}

/// 60 digits as 3 lines of 4 groups, the way they are read out.
fn groups(number: &str) -> String {
    let groups: Vec<String> = number.as_bytes().chunks(5).map(|c| String::from_utf8_lossy(c).into_owned()).collect();
    groups.chunks(4).map(|line| line.join(" ")).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nobody_is_reachable_while_tor_starts_over() {
        let ready = Network { percent: 100, ..Network::default() };
        assert!(reached(&ready, Some(LinkState::Idle)));
        assert!(!reached(&Network { percent: 92, ..ready.clone() }, Some(LinkState::Idle)));
        assert!(!reached(&Network { circuit: Some(false), ..ready }, Some(LinkState::Open)));
    }

    #[test]
    fn the_route_lights_up_as_far_as_things_got() {
        let ready = Network { percent: 100, ..Network::default() };
        let classes = |r: Route| ((0..6).map(|i| r.step(i)).collect::<Vec<_>>(), (0..5).map(|i| r.hop(i)).collect::<Vec<_>>());
        // Bootstrapping: the way to the entrance is being tried.
        let (steps, hops) = classes(Route::new(&Network { percent: 40, ..Network::default() }, None));
        assert_eq!((steps[1], hops[0], hops[1]), ("live", "live", "wait"));
        // The probe found onion circuits blocked: the second hop failed.
        let (steps, hops) = classes(Route::new(&Network { circuit: Some(false), ..ready.clone() }, Some(LinkState::Open)));
        assert_eq!((steps[1], steps[2], hops[1]), ("done", "fail", "fail"));
        // Reaching them: up to our relays, then the meeting point in orange.
        let (steps, hops) = classes(Route::new(&ready, Some(LinkState::Trying)));
        assert_eq!((steps[3], steps[4], hops[2], hops[3], hops[4]), ("done", "live", "done", "live", "wait"));
        // Away: nothing past our side, nothing in orange.
        let (steps, hops) = classes(Route::new(&ready, Some(LinkState::Unreachable)));
        assert!(!steps.contains(&"live") && hops[3] == "wait" && steps[5] == "wait");
        let (steps, hops) = classes(Route::new(&ready, Some(LinkState::Idle)));
        assert!(steps.iter().all(|s| *s == "done") && hops.iter().all(|h| *h == "done"));
    }
}
