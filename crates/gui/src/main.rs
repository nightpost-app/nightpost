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
use nightpost_core::api::{self, Client, Event, HistoryItem, LinkState, Network, Person, Reply, Request};
use nightpost_core::dbkey;

mod chrome;
mod person;
mod settings;

use chrome::Chrome;
use person::PersonPanel;
use settings::{BridgesEditor, Section, SettingsPage};

static DIR: OnceLock<PathBuf> = OnceLock::new();
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
    let dir = match api::data_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("nightpost-gui: {e:#}");
            std::process::exit(1);
        }
    };
    DIR.set(dir).expect("set once");
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
}

/// How notifications behave; nothing about anyone, so a plain file.
#[derive(Clone, Copy, PartialEq)]
struct Prefs {
    notify: bool,
    sound: bool,
    /// Who wrote, or only that something came.
    show_name: bool,
    /// What they wrote too (with show_name).
    show_text: bool,
    /// The system may keep notifications in its history.
    keep_history: bool,
    /// Closing the window stops the daemon too.
    lock_on_close: bool,
    /// The list of people is folded away.
    side_hidden: bool,
}

impl Prefs {
    const FILE: &str = "gui-settings";

    fn load() -> Prefs {
        let text = std::fs::read_to_string(dir().join(Prefs::FILE)).unwrap_or_default();
        let off = |key: &str| text.lines().any(|l| l == format!("{key}=0"));
        let on = |key: &str| text.lines().any(|l| l == format!("{key}=1"));
        Prefs { notify: !off("notify"), sound: on("sound"), show_name: !off("show_name"), show_text: on("show_text"), keep_history: on("keep_history"), lock_on_close: on("lock_on_close"), side_hidden: on("side_hidden") }
    }

    fn save(self) -> Prefs {
        let text = format!(
            "notify={}\nsound={}\nshow_name={}\nshow_text={}\nkeep_history={}\nlock_on_close={}\nside_hidden={}\n",
            u8::from(self.notify),
            u8::from(self.sound),
            u8::from(self.show_name),
            u8::from(self.show_text),
            u8::from(self.keep_history),
            u8::from(self.lock_on_close),
            u8::from(self.side_hidden)
        );
        let _ = std::fs::write(dir().join(Prefs::FILE), text);
        self
    }
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
            Ok(other) => format!("unexpected reply {other:?}"),
            Err(e) => format!("{e:#}"),
        });
    }

    fn on_event(mut self, event: Event) {
        match event {
            Event::Message { from, id, time, text } => {
                let n = self.arrival.read().as_ref().map_or(0, |a| a.1) + 1;
                self.arrival.set(Some((from.clone(), n)));
                spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    if self.arrival.read().as_ref().is_some_and(|a| a.1 == n) {
                        self.arrival.set(None);
                    }
                });
                let body = preview(&text);
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
                            (true, false) => notify("Nightpost", &format!("Message from {from}"), prefs),
                            _ => notify("Nightpost", "New message", prefs),
                        }
                    }
                }
            }
            Event::Sent { to, id, time, text } => {
                self.messages.write().entry(to).or_default().push(HistoryItem { id, outgoing: true, time, text, delivered: false, read: false });
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
            Event::ContactAdded { name } => {
                self.status.set(format!("{name} is now a contact"));
                if matches!(&*self.dialog.read(), Some(Dialog::Waiting { name: n, .. } | Dialog::Invite { name: n, .. }) if *n == name) {
                    self.dialog.set(None);
                }
                spawn(async move {
                    self.reload_contacts().await;
                    self.select(name);
                });
            }
            Event::Fingerprint { name, safety_number } => {
                notify("Nightpost", &format!("{name} used your invitation"), *self.prefs.read());
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
        phase: Signal::new(Phase::Connecting("Connecting to the daemon…".into())),
        contacts: Signal::new(Vec::new()),
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
        prefs: Signal::new(Prefs::load()),
        view: Signal::new(View::Chat),
        panel: Signal::new(false),
        maximized: Signal::new(false),
        read_receipts: Signal::new(false),
    });

    use_future(move || run(state, None));
    #[cfg(target_os = "linux")]
    use_hook(chrome::cut_corners);
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
    rsx! {
        style { {STYLE.as_str()} }
        div { class: "root", "data-theme": theme.name(), "data-maximized": "{state.maximized}",
            if *state.phase.read() != Phase::Ready {
                div { class: "drag-strip", onmousedown: chrome::drag, ondoubleclick: chrome::toggle_maximized }
            }
            match state.phase.read().clone() {
                Phase::Connecting(what) => rsx! {
                    div { class: "splash",
                        p { class: "wordmark", "Nightpost" }
                        p { "{what}" }
                        button { class: "quiet", onclick: move |_| state.dialog.set(Some(Dialog::Bridges)), "Tor blocked here? Bridges" }
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
            if !cfg!(target_os = "macos") {
                Chrome {}
            }
        }
    }
}

/// Folds the list of people away, or brings it back; remembered.
#[component]
fn SideToggle() -> Element {
    let mut state = use_context::<State>();
    let hidden = state.prefs.read().side_hidden;
    let label = if hidden { "Show the list" } else { "Hide the list" };
    rsx! {
        button {
            class: "side-toggle",
            title: label,
            "aria-label": label,
            "aria-expanded": if hidden { "false" } else { "true" },
            onclick: move |_| {
                let p = *state.prefs.peek();
                state.prefs.set(Prefs { side_hidden: !p.side_hidden, ..p }.save());
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
async fn run(mut state: State, passphrase: Option<String>) {
    let mut events = match attach(state.phase, passphrase).await {
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
    state.reload_contacts().await;
    state.phase.set(Phase::Ready);
    while let Ok(Some(event)) = events.event().await {
        state.on_event(event);
    }
    state.phase.set(Phase::Failed("The daemon stopped. Reopen Nightpost to start it again.".into()));
}

/// Asks for the passphrase, then starts the daemon with it.
#[component]
fn Locked(why: String) -> Element {
    let state = use_context::<State>();
    let mut passphrase = use_signal(String::new);
    let mut unlock = move || {
        let given = passphrase.read().clone();
        if given.is_empty() {
            return;
        }
        passphrase.set(String::new());
        // Not `spawn`: this screen goes away as the daemon starts.
        dioxus::core::spawn_forever(run(state, Some(given)));
    };
    rsx! {
        form { class: "splash", onsubmit: move |e| { e.prevent_default(); unlock(); },
            p { class: "wordmark", "Nightpost" }
            label { r#for: "passphrase", "Your passphrase opens your messages." }
            input { id: "passphrase", class: "name-field", r#type: "password", autofocus: true, value: "{passphrase}", oninput: move |e| passphrase.set(e.value()) }
            if !why.is_empty() { p { class: "warning", "{why}" } }
            button { class: "primary", r#type: "submit", "Unlock" }
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
    rows.sort_by(|a, b| b.1.as_ref().map(|m| m.time).cmp(&a.1.as_ref().map(|m| m.time)).then_with(|| a.0.cmp(&b.0)));
    let net = state.network.read().clone();
    let reachable = rows.iter().filter(|(n, _)| reached(&net, state.links.read().get(n).copied())).count();
    let (title, _) = network_summary(&net);
    let dot = if net.entry == Some(false) || net.circuit == Some(false) {
        "net-dot fail"
    } else if net.percent < 100 {
        "net-dot live"
    } else {
        "net-dot"
    };
    let in_settings = matches!(*state.view.read(), View::Settings(_));
    rsx! {
        aside {
            div { class: "brand", onmousedown: chrome::drag, ondoubleclick: chrome::toggle_maximized,
                p { class: "wordmark", "Nightpost" }
                if !rows.is_empty() { span { class: "reach-count", "{reachable} reachable of {rows.len()}" } }
            }
            nav {
                for (name, last) in rows {
                    {
                        let unread = state.unread.read().get(&name).copied().unwrap_or(0);
                        let mut class = String::from("contact");
                        if !in_settings && selected.as_deref() == Some(name.as_str()) { class.push_str(" selected"); }
                        if unread > 0 { class.push_str(" unread"); }
                        let preview = match &last {
                            Some(m) if m.outgoing => format!("You: {}", m.text.lines().next().unwrap_or_default()),
                            Some(m) => m.text.lines().next().unwrap_or_default().to_string(),
                            None => "No messages yet".into(),
                        };
                        let when = last.as_ref().map(|m| short_time(m.time)).unwrap_or_default();
                        let on = reached(&net, state.links.read().get(&name).copied());
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
                                    span { class: if on { "dot on" } else { "dot" } }
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
                button { class: "net-status", onclick: move |_| state.view.set(View::Settings(Section::Network)),
                    span { class: dot }
                    "{title}"
                }
                div { class: "footer-row",
                    button { class: "primary", onclick: move |_| state.dialog.set(Some(Dialog::Add)), "Add someone" }
                    button {
                        class: if in_settings { "gear on" } else { "gear" },
                        title: "Settings",
                        "aria-label": "Settings",
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

/// What the panel under the list says about Tor.
fn network_summary(net: &Network) -> (&'static str, String) {
    let line = net.line.strip_prefix("Tor: ").unwrap_or(&net.line).to_string();
    if net.entry == Some(false) || net.circuit == Some(false) {
        ("Tor is not getting through", line)
    } else if net.percent < 100 {
        ("Connecting to Tor", if line.is_empty() { format!("{}%", net.percent) } else { line })
    } else if net.bridged {
        ("Through a bridge", "This network does not see that you use Tor.".into())
    } else {
        ("Connected to Tor", "Messages pass three relays before they reach anyone.".into())
    }
}

/// The photo chosen for them, or three rings, the same for a name on every
/// machine: an onion of one's own.
fn avatar(name: &str, photo: Option<&str>, size: u32) -> Element {
    if let Some(photo) = photo {
        // Our own re-encoded PNG (`person::photo_uri`), checked by the daemon.
        return rsx! { img { class: "photo", width: "{size}", height: "{size}", src: "{photo}", alt: "" } };
    }
    const RINGS: [[&str; 3]; 8] = [
        ["#F2B880", "#C8553D", "#2D3047"],
        ["#A7C4E2", "#3A6EA5", "#F4E285"],
        ["#B8D8BA", "#5B8E7D", "#EF8354"],
        ["#D7C3E8", "#7A5C99", "#F2EEF6"],
        ["#F6D6AD", "#D08C60", "#3F4B3B"],
        ["#9FD8D3", "#2F7F86", "#FCE7C8"],
        ["#F4B6C2", "#B04A6C", "#2E294E"],
        ["#C9D6A3", "#6B8F3A", "#F3EBD3"],
    ];
    // FNV-1a: stable across versions and machines, unlike std's hasher.
    let hash = name.bytes().fold(0x811c_9dc5_u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    let [a, b, c] = RINGS[hash as usize % RINGS.len()];
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

/// Where a message to someone gets now: the steps reached, the one being
/// tried (orange), the one that failed. Steps: this computer, the entrance
/// (a guard, or a bridge), two relays, the meeting point, their device.
#[derive(Clone, Copy, PartialEq)]
struct Route {
    lit: usize,
    live: Option<usize>,
    fail: Option<usize>,
}

impl Route {
    fn new(net: &Network, link: Option<LinkState>) -> Route {
        let at = |lit, live, fail| Route { lit, live, fail };
        if net.entry == Some(false) {
            at(1, None, Some(0))
        } else if net.percent < 100 {
            at(1, Some(0), None)
        } else if net.circuit == Some(false) {
            at(2, None, Some(1))
        } else {
            match link {
                Some(LinkState::Open | LinkState::Idle) => at(6, None, None),
                Some(LinkState::Trying) => at(4, Some(3), None),
                Some(LinkState::Unreachable) | None => at(4, None, None),
            }
        }
    }

    /// The class of the hop that leaves step `i`.
    fn hop(self, i: usize) -> &'static str {
        if self.live == Some(i) {
            "live"
        } else if self.fail == Some(i) {
            "fail"
        } else if i + 1 < self.lit {
            "done"
        } else {
            "wait"
        }
    }

    fn step(self, i: usize) -> &'static str {
        if i < self.lit {
            "done"
        } else if i > 0 && self.live == Some(i - 1) {
            "live"
        } else if i > 0 && self.fail == Some(i - 1) {
            "fail"
        } else {
            "wait"
        }
    }
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
    let entrance = if net.bridged { "Private entrance" } else { "Entrance" };
    let tips: [(String, String); 6] = [
        ("Your computer".into(), format!("Messages are locked here before they leave. Only {name}’s device can open them.")),
        (
            entrance.into(),
            if net.bridged {
                "A bridge: a way into Tor that is not on the public list, so this network cannot tell it apart.".into()
            } else {
                "The first Tor relay. It sees your address, never where messages go.".into()
            },
        ),
        ("Relay".into(), "Kept for weeks: it guards your side of the route. It cannot read the message.".into()),
        ("Relay".into(), "Changes every few hours. It knows neither who writes nor to whom.".into()),
        ("Meeting point".into(), format!("Where your route and {name}’s meet. Neither of you learns where the other is.")),
        (name.clone(), format!("{name}’s device, the only one that can open your messages.")),
    ];
    let labels = [String::from("You"), entrance.into(), "Relay".into(), "Relay".into(), "Meeting point".into(), name.clone()];
    // What the step in orange or red is doing, for its tooltip.
    let now = match (route.live, route.fail) {
        (Some(0), _) | (_, Some(_)) => line.clone(),
        (Some(_), _) => format!("Reaching {name}…"),
        _ => String::new(),
    };
    rsx! {
        // Measured by the narrow layout in style.css.
        div { class: "route-box",
            div { class: "route", role: "group", "aria-label": "Route to {name}",
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
        ("fail", "Tor is not getting through".into())
    } else if route.live == Some(0) {
        ("live", format!("Connecting to Tor, {}%", net.percent))
    } else {
        match link {
            Some(LinkState::Open) => ("on", "Connected".into()),
            Some(LinkState::Idle) => ("on", "Reachable".into()),
            Some(LinkState::Trying) => ("live", format!("Reaching {name}…")),
            Some(LinkState::Unreachable) => ("", "Not reachable right now".into()),
            None => ("", "Reached when you write".into()),
        }
    }
}

#[component]
fn Conversation() -> Element {
    let mut state = use_context::<State>();
    let mut draft = use_signal(String::new);
    let Some(name) = state.selected.read().clone() else {
        return rsx! {
            main {
                div { class: "drag-strip", onmousedown: chrome::drag, ondoubleclick: chrome::toggle_maximized }
                div { class: "empty",
                h2 { "Nobody here yet" }
                p { "Nightpost only connects people who know each other. Send someone a one-time link, or read them a code on the phone." }
                button { class: "primary", onclick: move |_| state.dialog.set(Some(Dialog::Add)), "Add someone" }
            }
            p { class: "status", "{state.status}" } }
        };
    };
    let rows = rows(state.messages.read().get(&name).map(Vec::as_slice).unwrap_or_default());
    let link = state.links.read().get(&name).copied();
    let (reach_class, reach) = reach_line(&name, &state.network.read(), link);
    let waiting = if link == Some(LinkState::Unreachable) { format!("Will be sent once {name} is reachable") } else { "Sending…".into() };

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
                button { class: if open { "who open" } else { "who" }, "aria-label": "About {name}", onclick: toggle,
                    {avatar(&name, person.photo.as_deref(), 48)}
                    span { class: "who-text",
                        span { class: "who-name",
                            h2 { "{name}" }
                            if person.verified {
                                svg { class: "mark verified", view_box: "0 0 24 24", role: "img", "aria-label": "Verified",
                                    path { d: "M12 3l7 2.8v5.6c0 4.4-3 7.7-7 9.4-4-1.7-7-5-7-9.4V5.8z" }
                                    path { d: "M8.5 12l2.5 2.5 4.5-5" }
                                }
                            }
                            if muted(&person).is_some() {
                                svg { class: "mark", view_box: "0 0 24 24", role: "img", "aria-label": "Muted",
                                    path { d: "M6 8a6 6 0 0 1 9.3-5M18 8c0 7 3 9 3 9H9" }
                                    path { d: "M10.3 21a1.94 1.94 0 0 0 3.4 0" }
                                    path { d: "M3 3l18 18" }
                                }
                            }
                        }
                        span { class: "state {reach_class}", "{reach}" }
                    }
                }
                button { class: if open { "info on" } else { "info" }, title: "About {name}", "aria-label": "About {name}", onclick: toggle,
                    svg { width: "20", height: "20", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", "aria-hidden": "true",
                        circle { cx: "12", cy: "12", r: "9" }
                        path { d: "M12 11v5M12 7.6v.1" }
                    }
                }
                div { class: "drag-fill", onmousedown: chrome::drag, ondoubleclick: chrome::toggle_maximized }
            }
            RouteView { name: name.clone() }
            // Reversed column: the newest message sits at the bottom and the
            // view stays there as messages arrive.
            div { class: "thread",
                div { class: "column",
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
                                            } else if m.outgoing {
                                                // Signal's marks: two rings once delivered, filled once read.
                                                svg { class: if m.read { "tick read" } else { "tick" }, view_box: "2 0 24 16", role: "img",
                                                    "aria-label": if m.read { "Read" } else { "Delivered" },
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
            div { class: "composer",
                textarea {
                    "aria-label": "Message to {name}",
                    placeholder: "Write to {name}",
                    value: "{draft}",
                    rows: "1",
                    oninput: move |e| draft.set(e.value()),
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Enter && !e.modifiers().contains(Modifiers::SHIFT) {
                            e.prevent_default();
                            send_key();
                        }
                    },
                }
                button { class: "primary", onclick: move |_| send_click(), "Send" }
            }
            p { class: "status", "{state.status}" }
            if open { PersonPanel { key: "{name}", name: name.clone() } }
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
            h3 { "Bridges" }
            BridgesEditor {}
            div { class: "actions", button { onclick: close, "Close" } }
        },
        Dialog::Safety { name, number } => return rsx! { SafetyNumber { name, number } },
        Dialog::Invite { name, link } => rsx! {
            h3 { "An invitation for {name}" }
            p { "Give it to {name} only, through a channel you trust. It works once, for 7 days." }
            img { class: "qr", src: "{qr(&link)}", alt: "QR code of the invitation" }
            textarea { class: "link", readonly: true, rows: "4", "aria-label": "Invitation link", value: "{link}" }
            div { class: "actions",
                CopyButton { text: link.clone(), label: "Copy the link" }
                button { onclick: close, "Done" }
            }
        },
        Dialog::Code { name, code } => rsx! {
            h3 { "Read this code to {name}" }
            p { "On a call or in person. It works for 10 minutes." }
            p { class: "code", "{code.replace('-', \" \")}" }
            p { "When {name} types it in, you will both compare a safety number before anything is kept." }
            div { class: "actions",
                CopyButton { text: code.clone(), label: "Copy the code" }
                button { onclick: close, "Hide" }
            }
        },
        Dialog::Confirm { name, number } => {
            let (keep, refuse) = (name.clone(), name.clone());
            rsx! {
                h3 { "Someone used your code for {name}" }
                p { "Ask them to read their safety number aloud. Yours is:" }
                p { class: "number", "{groups(&number)}" }
                p { class: "warning", "Keep them only if every digit matches. Otherwise it is not {name}." }
                div { class: "actions",
                    button { class: "primary", onclick: move |_| confirm(state, keep.clone(), true), "The digits match" }
                    button { class: "danger", onclick: move |_| confirm(state, refuse.clone(), false), "They differ, turn down" }
                }
            }
        }
        Dialog::Waiting { name, number } => rsx! {
            h3 { "Waiting for {name}" }
            p { "Read this safety number to {name}. They check that it matches theirs:" }
            p { class: "number", "{groups(&number)}" }
            p { "{name} is added as soon as they confirm." }
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
        div { class: "full", role: "dialog", "aria-label": "Safety number with {name}",
            button { class: "full-close", "aria-label": "Close", onclick: close,
                svg { width: "22", height: "22", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", "aria-hidden": "true", path { d: "M6 6l12 12M18 6L6 18" } }
            }
            div { class: "full-body",
                {avatar(&name, photo.as_deref(), 64)}
                h1 { "Your safety number with {name}" }
                p { class: "full-help", "Read it to each other on a call or in person. The same digits on both sides mean nobody sits in between." }
                p { class: "full-number", "{groups(&number)}" }
                div { class: "actions",
                    button { class: "primary", onclick: move |_| {
                        state.dialog.set(None);
                        person::update(state, who.clone(), |p| p.verified = true);
                    }, "They match" }
                    button { class: "ghost", onclick: close, "Not now" }
                }
                p { class: "full-warn", "If even one digit differs, it is not {name} at the other end: do not mark it." }
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

    let invite = move |by_code: bool| {
        let who = name.read().trim().to_string();
        if who.is_empty() {
            return error.set("Give them a name first.".into());
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
            return error.set("Paste their link, or type the code they read you.".into());
        }
        if who.is_empty() {
            return error.set("Give them a name first, at the top.".into());
        }
        // A code is digits and separators; anything else is taken for a link.
        let request = if given.chars().all(|c| c.is_ascii_digit() || " -.".contains(c)) {
            Request::AcceptCode { code: given, name: who.clone() }
        } else {
            Request::Accept { link: given, name: who.clone() }
        };
        state.dialog.set(None);
        state.status.set(format!("Looking for {who} (up to a few minutes)…"));
        // Not `spawn`: that task belongs to this dialog, which just closed,
        // and would be dropped before the request even left.
        dioxus::core::spawn_forever(async move {
            match call(request).await {
                Ok(Reply::Added { name }) => state.status.set(format!("{name} added")),
                other => state.fail(other),
            }
        });
    };
    let (mut by_link, mut by_code) = (invite, invite);

    rsx! {
        h3 { "Add someone" }
        label { "Their name, as you will see it" }
        input { autofocus: true, value: "{name}", placeholder: "Alice", oninput: move |e| { name.set(e.value()); error.set(String::new()); } }
        div { class: "actions",
            button { class: "primary", onclick: move |_| by_link(false), "Create a link" }
            button { onclick: move |_| by_code(true), "Create a code to read out" }
        }
        hr {}
        label { "Or use what they gave you: their link, or the code they read you" }
        input { value: "{pasted}", placeholder: "nightpost:1:… or 482-109-375-512", oninput: move |e| { pasted.set(e.value()); error.set(String::new()); } }
        if !error.read().is_empty() { p { class: "warning", role: "alert", "{error}" } }
        div { class: "actions",
            button { class: "primary", onclick: accept, "Use it" }
            button { onclick: move |_| state.dialog.set(None), "Cancel" }
        }
    }
}

fn confirm(mut state: State, name: String, keep: bool) {
    state.dialog.set(None);
    state.codes.write().retain(|n| *n != name);
    spawn(async move {
        match call(Request::Confirm { name: name.clone(), keep }).await {
            Ok(Reply::Done) => state.status.set(if keep { format!("{name} kept") } else { format!("{name} turned down") }),
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
async fn attach(mut phase: Signal<Phase>, passphrase: Option<String>) -> Result<Option<Client>> {
    if let Ok(client) = listen().await {
        return Ok(Some(client));
    }
    let asked = passphrase.is_some();
    if !asked && dbkey::uses_passphrase(dir()) {
        phase.set(Phase::Locked(String::new()));
        return Ok(None);
    }
    phase.set(Phase::Connecting("Starting…".into()));
    let mut daemon = start_daemon(passphrase)?;
    let log = dir().join("daemon.log");
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Ok(client) = listen().await {
            return Ok(Some(client));
        }
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if let Some(status) = daemon.try_wait()? {
            if asked && text.contains("wrong key") {
                phase.set(Phase::Locked("That passphrase does not open them. Try again.".into()));
                return Ok(None);
            }
            bail!("the daemon stopped ({status}): {}", text.lines().last().unwrap_or_default());
        }
        // Its progress lines, not the log lines around them.
        let progress = text.lines().rev().find(|l| l.starts_with("Tor:")).unwrap_or("Starting…");
        phase.set(Phase::Connecting(progress.into()));
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
fn start_daemon(passphrase: Option<String>) -> Result<std::process::Child> {
    let exe = std::env::current_exe()?.with_file_name(format!("nightpost{}", std::env::consts::EXE_SUFFIX));
    let log = std::fs::File::create(dir().join("daemon.log"))?;
    let mut command = std::process::Command::new(&exe);
    let stdin = if passphrase.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() };
    command.arg("daemon").stdin(stdin).stdout(log.try_clone()?).stderr(log);
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
    // One line on its stdin, then closed: never in its arguments or environment.
    if let (Some(passphrase), Some(mut stdin)) = (passphrase, child.stdin.take()) {
        use std::io::Write as _;
        stdin.write_all(zeroize::Zeroizing::new(passphrase + "\n").as_bytes())?;
    }
    Ok(child)
}

/// Shown by the system: it can be on a locked screen and kept in a history,
/// so what goes in is the user's choice (Settings, Notifications).
fn notify(title: &str, body: &str, prefs: Prefs) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        let mut notification = notify_rust::Notification::new();
        notification.summary(&title).body(&body);
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

/// A message as a notification's body: short, and markup-free. Some servers
/// (Plasma) read the body as HTML, and an <img> in a message would load a
/// picture from wherever the sender likes, giving our address away.
fn preview(text: &str) -> String {
    const MAX: usize = 200;
    let mut short: String = text.chars().take(MAX).collect();
    if text.chars().nth(MAX).is_some() {
        short.push('…');
    }
    short.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Copies `text`, and says so for four seconds once the clipboard took it.
#[component]
fn CopyButton(text: String, label: &'static str) -> Element {
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
            span { class: if !*copied.read() { "off" }, "Copied" }
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
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ if then.year() == today.year() => then.strftime("%A %-d %B").to_string(),
        _ => then.strftime("%-d %B %Y").to_string(),
    }
}

/// For the list: the time today, then "Yesterday", the weekday, the date.
fn short_time(millis: u64) -> String {
    let (Some(then), Some(today)) = (zoned(millis), zoned(jiff::Timestamp::now().as_millisecond() as u64)) else { return String::new() };
    match (today.date() - then.date()).get_days() {
        0 => then.strftime("%H:%M").to_string(),
        1 => "Yesterday".into(),
        2..=6 => then.strftime("%A").to_string(),
        _ => then.strftime("%-d %b").to_string(),
    }
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
    fn a_message_in_a_notification_is_short_and_inert() {
        assert_eq!(preview("<img src=\"http://x/\"> & co"), "&lt;img src=\"http://x/\"&gt; &amp; co");
        let long = "é".repeat(250);
        assert_eq!(preview(&long), format!("{}…", "é".repeat(200)));
        assert_eq!(preview(&"é".repeat(200)), "é".repeat(200));
    }

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
