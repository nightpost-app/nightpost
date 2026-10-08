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
use nightpost_core::transport::tor;

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
    let window = WindowBuilder::new().with_title("Nightpost").with_inner_size(LogicalSize::new(1040.0, 700.0));
    let config = Config::new().with_window(window).with_menu(None).with_background_color((15, 21, 34, 255));
    dioxus::LaunchBuilder::desktop().with_cfg(config).launch(App);
}

fn dir() -> &'static PathBuf {
    DIR.get().expect("set in main")
}

#[derive(Clone, PartialEq)]
enum Phase {
    Connecting(String),
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
    Verify { name: String, number: String },
    Bridges,
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

    fn next(self) -> Theme {
        let next = match self {
            Theme::System => Theme::Dark,
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::System,
        };
        let _ = std::fs::write(dir().join(Theme::FILE), next.name());
        next
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
                self.messages.write().entry(from.clone()).or_default().push(HistoryItem { id, outgoing: false, time, text, delivered: true });
                let here = self.selected.read().as_deref() == Some(from.as_str());
                if !here || !dioxus::desktop::window().window.is_focused() {
                    if !here {
                        *self.unread.write().entry(from.clone()).or_default() += 1;
                    }
                    notify(&format!("Message from {from}"));
                }
            }
            Event::Sent { to, id, time, text } => {
                self.messages.write().entry(to).or_default().push(HistoryItem { id, outgoing: true, time, text, delivered: false });
            }
            Event::Delivered { to, id } => {
                if let Some(m) = self.messages.write().get_mut(&to).and_then(|ms| ms.iter_mut().find(|m| m.id == id)) {
                    m.delivered = true;
                }
            }
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
                notify(&format!("{name} used your invitation"));
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
    });

    use_future(move || async move {
        let mut state = state;
        let mut events = match attach(state.phase).await {
            Ok(events) => events,
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
        state.reload_contacts().await;
        state.phase.set(Phase::Ready);
        while let Ok(Some(event)) = events.event().await {
            state.on_event(event);
        }
        state.phase.set(Phase::Failed("The daemon stopped. Reopen Nightpost to start it again.".into()));
    });

    let theme = *state.theme.read();
    rsx! {
        style { {STYLE.as_str()} }
        div { class: "root", "data-theme": theme.name(),
            match state.phase.read().clone() {
                Phase::Connecting(what) => rsx! {
                    div { class: "splash",
                        p { class: "wordmark", "Nightpost" }
                        p { "{what}" }
                        button { class: "quiet", onclick: move |_| state.dialog.set(Some(Dialog::Bridges)), "Tor blocked here? Bridges" }
                    }
                    DialogView {}
                },
                Phase::Failed(why) => rsx! { div { class: "splash", p { class: "wordmark", "Nightpost" } p { class: "warning", "{why}" } } },
                Phase::Ready => rsx! {
                    div { class: "app",
                        Sidebar {}
                        Conversation {}
                    }
                    DialogView {}
                },
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
    rows.sort_by(|a, b| b.1.as_ref().map(|m| m.time).cmp(&a.1.as_ref().map(|m| m.time)).then_with(|| a.0.cmp(&b.0)));
    let reachable = rows.iter().filter(|(n, _)| reached(state.links.read().get(n).copied())).count();
    let theme = *state.theme.read();
    let theme_label = match theme {
        Theme::System => "Theme: as the system",
        Theme::Dark => "Theme: dark",
        Theme::Light => "Theme: light",
    };
    let (title, detail) = network_summary(&state.network.read());
    rsx! {
        aside {
            div { class: "brand",
                p { class: "wordmark", "Nightpost" }
                if !rows.is_empty() { span { class: "reach-count", "{reachable} reachable of {rows.len()}" } }
                button { class: "theme", title: theme_label, "aria-label": theme_label, onclick: move |_| state.theme.set(theme.next()),
                    {theme_icon(theme)}
                }
            }
            nav {
                for (name, last) in rows {
                    {
                        let unread = state.unread.read().get(&name).copied().unwrap_or(0);
                        let mut class = String::from("contact");
                        if selected.as_deref() == Some(name.as_str()) { class.push_str(" selected"); }
                        if unread > 0 { class.push_str(" unread"); }
                        let preview = match &last {
                            Some(m) if m.outgoing => format!("You: {}", m.text.lines().next().unwrap_or_default()),
                            Some(m) => m.text.lines().next().unwrap_or_default().to_string(),
                            None => "No messages yet".into(),
                        };
                        let when = last.as_ref().map(|m| short_time(m.time)).unwrap_or_default();
                        let on = reached(state.links.read().get(&name).copied());
                        let pick = name.clone();
                        rsx! {
                            button { key: "{name}", class: "{class}", onclick: move |_| state.select(pick.clone()),
                                span { class: "avatar",
                                    {avatar(&name, 44)}
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
            section { class: "network", "aria-label": "Network",
                p { class: "title", "{title}" }
                p { class: "detail", "{detail}" }
                div { class: "buttons",
                    button { class: "ghost", onclick: move |_| state.dialog.set(Some(Dialog::Bridges)), "Bridges" }
                    button { class: "primary", onclick: move |_| state.dialog.set(Some(Dialog::Add)), "Add someone" }
                }
            }
        }
    }
}

/// Reached since the daemon started: open now, or closed after use.
fn reached(link: Option<LinkState>) -> bool {
    matches!(link, Some(LinkState::Open | LinkState::Idle))
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

/// Three rings, the same for a name on every machine: an onion of one's own.
fn avatar(name: &str, size: u32) -> Element {
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

fn theme_icon(theme: Theme) -> Element {
    rsx! {
        svg { width: "20", height: "20", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            match theme {
                Theme::System => rsx! { circle { cx: "12", cy: "12", r: "8" } path { d: "M12 4a8 8 0 0 1 0 16z", fill: "currentColor" } },
                Theme::Dark => rsx! { path { d: "M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z" } },
                Theme::Light => rsx! { circle { cx: "12", cy: "12", r: "4" } path { d: "M12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4" } },
            }
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
                        div { key: "step{i}", class: "{class}", style: "left: {x / 9.0}%; top: {y / 1.4}%",
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
            main { div { class: "empty",
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
    let verify_name = name.clone();

    rsx! {
        main {
            header {
                div { class: "who",
                    {avatar(&name, 48)}
                    div {
                        h2 { "{name}" }
                        p { class: "state {reach_class}", "{reach}" }
                    }
                }
                button {
                    class: "ghost",
                    onclick: move |_| {
                        let name = verify_name.clone();
                        spawn(async move {
                            match call(Request::Verify { name: name.clone() }).await {
                                Ok(Reply::SafetyNumber { number, .. }) => state.dialog.set(Some(Dialog::Verify { name, number })),
                                other => state.fail(other),
                            }
                        });
                    },
                    "Compare safety number"
                }
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
                                                span { class: "waiting", "{waiting}" }
                                            } else if m.outgoing {
                                                svg { view_box: "0 0 24 24", role: "img", "aria-label": "Delivered", polyline { points: "4 12.5 9.5 18 20 6.5" } }
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
        }
    }
}

#[component]
fn DialogView() -> Element {
    let mut state = use_context::<State>();
    let Some(dialog) = state.dialog.read().clone() else { return rsx! {} };
    let close = move |_| state.dialog.set(None);
    let body = match dialog {
        Dialog::Add => rsx! { AddDialog {} },
        Dialog::Bridges => rsx! { BridgesDialog {} },
        Dialog::Invite { name, link } => rsx! {
            h3 { "An invitation for {name}" }
            p { "Give it to {name} only, through a channel you trust. It works once, for 7 days." }
            img { class: "qr", src: "{qr(&link)}", alt: "QR code of the invitation" }
            textarea { class: "link", readonly: true, rows: "4", "aria-label": "Invitation link", value: "{link}" }
            div { class: "actions",
                button { class: "primary", onclick: move |_| copy(&link), "Copy the link" }
                button { onclick: close, "Done" }
            }
        },
        Dialog::Code { name, code } => rsx! {
            h3 { "Read this code to {name}" }
            p { "On a call or in person. It works for 10 minutes." }
            p { class: "code", "{code.replace('-', \" \")}" }
            p { "When {name} types it in, you will both compare a safety number before anything is kept." }
            div { class: "actions", button { onclick: close, "Hide" } }
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
        Dialog::Verify { name, number } => rsx! {
            h3 { "Your safety number with {name}" }
            p { "Read it to each other on a call or in person. The same digits on both sides mean nobody sits in between." }
            p { class: "number", "{groups(&number)}" }
            div { class: "actions", button { class: "primary", onclick: close, "Done" } }
        },
    };
    rsx! {
        div { class: "overlay",
            div { class: "sheet", role: "dialog", {body} }
        }
    }
}

#[component]
fn BridgesDialog() -> Element {
    let mut state = use_context::<State>();
    let file = dir().join(tor::BRIDGES_FILE);
    let mut text = use_signal(|| tor::read_bridges(&file).join("\n"));
    let mut error = use_signal(String::new);
    let save = move |_| {
        let lines: Vec<String> = text.read().lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect();
        match tor::save_bridges(&file, &lines) {
            Ok(()) => {
                state.dialog.set(None);
                state.status.set(if lines.is_empty() { "Back to the built-in Snowflake bridges".into() } else { "Bridges saved: Tor reconnects through them".into() });
            }
            Err(e) => error.set(e.to_string()),
        }
    };
    rsx! {
        h3 { "Bridges" }
        p { "Some networks (university or work Wi-Fi) block Tor. Nightpost then goes through a bridge: Snowflake by default, with nothing to set up." }
        p { "If that does not get through, paste bridge lines here: your own, a friend's, or from bridges.torproject.org (WebTunnel gets through most often). They are used at once." }
        label { r#for: "bridges", "Bridge lines, one per line" }
        textarea { id: "bridges", class: "link", rows: "5", placeholder: "webtunnel [2001:db8::1]:443 … url=https://…", value: "{text}", oninput: move |e| text.set(e.value()) }
        if !error.read().is_empty() { p { class: "warning", "{error}" } }
        div { class: "actions",
            button { class: "primary", onclick: save, "Save" }
            button { onclick: move |_| state.dialog.set(None), "Cancel" }
        }
    }
}

#[component]
fn AddDialog() -> Element {
    let mut state = use_context::<State>();
    let mut name = use_signal(String::new);
    let mut pasted = use_signal(String::new);

    let invite = move |by_code: bool| {
        let who = name.read().trim().to_string();
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
    let (by_link, by_code) = (invite, invite);

    rsx! {
        h3 { "Add someone" }
        label { "Their name, as you will see it" }
        input { autofocus: true, value: "{name}", placeholder: "Alice", oninput: move |e| name.set(e.value()) }
        div { class: "actions",
            button { class: "primary", onclick: move |_| by_link(false), "Create a link" }
            button { onclick: move |_| by_code(true), "Create a code to read out" }
        }
        hr {}
        label { "Or use what they gave you: their link, or the code they read you" }
        input { value: "{pasted}", placeholder: "nightpost:1:… or 482-109-375-512", oninput: move |e| pasted.set(e.value()) }
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

/// One request on its own connection.
async fn call(request: Request) -> Result<Reply> {
    Client::connect(dir()).await?.call(&request).await
}

/// The event stream, starting the daemon first if none answers.
async fn attach(mut phase: Signal<Phase>) -> Result<Client> {
    if let Ok(client) = listen().await {
        return Ok(client);
    }
    let mut daemon = start_daemon()?;
    let log = dir().join("daemon.log");
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Ok(client) = listen().await {
            return Ok(client);
        }
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if let Some(status) = daemon.try_wait()? {
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
fn start_daemon() -> Result<std::process::Child> {
    let exe = std::env::current_exe()?.with_file_name(format!("nightpost{}", std::env::consts::EXE_SUFFIX));
    let log = std::fs::File::create(dir().join("daemon.log"))?;
    let mut command = std::process::Command::new(&exe);
    command.arg("daemon").stdin(std::process::Stdio::null()).stdout(log.try_clone()?).stderr(log);
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
    command.spawn().with_context(|| format!("starting {}", exe.display()))
}

/// The name only, never the text: a notification can show on a locked screen.
fn notify(text: &str) {
    let text = text.to_string();
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new().summary("Nightpost").body(&text).show();
    });
}

fn copy(text: &str) {
    // Our own link (base32): Rust's quoting is a valid JS string literal.
    let quoted = format!("{text:?}");
    document::eval(&format!("navigator.clipboard.writeText({quoted})"));
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
