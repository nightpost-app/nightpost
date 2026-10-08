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
use nightpost_core::api::{self, Client, Event, HistoryItem, Reply, Request};
use nightpost_core::transport::tor;

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// The typefaces travel inside the program: a Tor messenger fetches nothing
/// from a font server. Both are under the SIL Open Font License (`fonts/`).
static STYLE: LazyLock<String> = LazyLock::new(|| {
    let face = |family: &str, weights: &str, bytes: &[u8]| {
        format!(
            "@font-face {{ font-family: \"{family}\"; font-weight: {weights}; src: url(data:font/ttf;base64,{}) format(\"truetype\"); }}\n",
            data_encoding::BASE64.encode(bytes)
        )
    };
    face("Literata", "200 900", include_bytes!("../fonts/Literata.ttf"))
        + &face("Atkinson Hyperlegible Next", "200 800", include_bytes!("../fonts/AtkinsonHyperlegibleNext.ttf"))
        + include_str!("style.css")
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
    let config = Config::new().with_window(window).with_menu(None).with_background_color((22, 27, 43, 255));
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
            Ok(Reply::Contacts { contacts }) => contacts.into_iter().map(|c| c.name).collect(),
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
        state.reload_contacts().await;
        state.phase.set(Phase::Ready);
        while let Ok(Some(event)) = events.event().await {
            state.on_event(event);
        }
        state.phase.set(Phase::Failed("The daemon stopped. Reopen Nightpost to start it again.".into()));
    });

    rsx! {
        style { {STYLE.as_str()} }
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
    rsx! {
        aside {
            p { class: "wordmark", "Nightpost" }
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
                        let pick = name.clone();
                        rsx! {
                            button { key: "{name}", class: "{class}", onclick: move |_| state.select(pick.clone()),
                                span { class: "name", "{name}" }
                                span { class: "when", "{when}" }
                                span { class: "preview", "{preview}" }
                                if unread > 0 { span { class: "count", "{unread}" } }
                            }
                        }
                    }
                }
            }
            button { class: "quiet bridges", onclick: move |_| state.dialog.set(Some(Dialog::Bridges)), "Bridges" }
            button { class: "add", onclick: move |_| state.dialog.set(Some(Dialog::Add)), "Add someone" }
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
                h2 { "{name}" }
                button {
                    class: "quiet",
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
            // Reversed column: the newest message sits at the bottom and the
            // view stays there as messages arrive.
            div { class: "thread",
                div { class: "column",
                    for row in rows.into_iter().rev() {
                        match row {
                            Row::Day(label) => rsx! { p { key: "day-{label}", class: "day", "{label}" } },
                            Row::Slip { m, meta } => rsx! {
                                div { key: "{m.id}", class: if m.outgoing { "slip-row out" } else { "slip-row in" },
                                    div { class: "slip", "{m.text}" }
                                    if meta {
                                        p { class: "meta",
                                            "{clock(m.time)}"
                                            if m.outgoing && !m.delivered { span { class: "pending", ", not delivered yet" } }
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
                button { class: "send", onclick: move |_| send_click(), "Send" }
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
        spawn(async move {
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
    let image = code.render::<svg::Color>().min_dimensions(260, 260).dark_color(svg::Color("#0b0b0b")).light_color(svg::Color("#ece8df")).build();
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
