//! The settings page: how the app looks, reaches Tor, notifies, keeps
//! messages locked, and what it is.

use dioxus::prelude::*;
use nightpost_core::api::{Reply, Request};
use nightpost_core::{dbkey, i18n, t};

use crate::{CopyButton, State, Theme, View, as_launched, call, dir};

#[derive(Clone, Copy, PartialEq)]
pub enum Section {
    Appearance,
    Network,
    Notifications,
    Security,
    About,
}

fn sections() -> [(Section, String); 5] {
    [
        (Section::Appearance, t!("appearance")),
        (Section::Network, t!("network")),
        (Section::Notifications, t!("notifications")),
        (Section::Security, t!("security")),
        (Section::About, t!("about")),
    ]
}

/// Each section's icon, in a tile with its own outline.
fn icon(section: Section) -> Element {
    rsx! {
        svg { width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            match section {
                Section::Appearance => rsx! { circle { cx: "12", cy: "12", r: "8.5" } path { d: "M12 3.5a8.5 8.5 0 0 1 0 17z", fill: "currentColor" } },
                Section::Network => rsx! { path { d: "M4 20v-3M9 20v-7M14 20v-11M19 20V5" } },
                Section::Notifications => rsx! { path { d: "M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" } path { d: "M10.3 21a1.94 1.94 0 0 0 3.4 0" } },
                Section::Security => rsx! { rect { x: "4.5", y: "11", width: "15", height: "10", rx: "2" } path { d: "M8 11V7.5a4 4 0 0 1 8 0V11" } },
                Section::About => rsx! { path { d: "M12 11v6M12 7.5v.01" } },
            }
        }
    }
}

#[component]
pub fn SettingsPage(section: Section) -> Element {
    let mut state = use_context::<State>();
    rsx! {
        main { class: "settings",
            header { class: "settings-head",
                button { class: "close", title: t!("back"), "aria-label": t!("back"), onclick: move |_| state.view.set(View::Chat),
                    svg { width: "20", height: "20", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true", path { d: "M15 5l-7 7 7 7" } }
                }
                h1 { {t!("settings")} }
                div { class: "drag-fill", onmousedown: crate::chrome::drag, ondoubleclick: crate::chrome::toggle_maximized }
            }
            div { class: "settings-body",
                nav { class: "sections", "aria-label": t!("settings-sections"),
                    for (s, label) in sections() {
                        button {
                            key: "{label}",
                            class: if s == section { "section on" } else { "section" },
                            "aria-current": if s == section { "page" } else { "false" },
                            title: "{label}",
                            onclick: move |_| state.view.set(View::Settings(s)),
                            span { class: "tile", {icon(s)} }
                            span { class: "section-label", "{label}" }
                        }
                    }
                }
                // Keyed by section: each one is a new scroll box, opened at its top
                // rather than at the place the previous one was left.
                for s in [section] {
                    div { key: "{s as u8}", class: "settings-content",
                        match s {
                            Section::Appearance => rsx! { Appearance {} },
                            Section::Network => rsx! { NetworkSection {} },
                            Section::Notifications => rsx! { Notifications {} },
                            Section::Security => rsx! { Security {} },
                            Section::About => rsx! { About {} },
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn Appearance() -> Element {
    let mut state = use_context::<State>();
    let theme = *state.theme.read();
    // A signal: choosing the language already spoken redraws nothing else.
    let mut chosen = use_signal(i18n::chosen);
    let mut terminal = use_signal(|| std::fs::read_to_string(dir().join(crate::tray::TERMINAL_FILE)).unwrap_or_default().trim().to_string());
    let installed = crate::tray::installed();
    // A command line of one's own: none of the buttons.
    let mut other = use_signal(|| {
        let line = terminal.peek().clone();
        !line.is_empty() && !installed.iter().any(|&(_, l)| l == line)
    });
    let options = [(Theme::System, t!("theme-system"), "#0f1522", "#f4f5f7"), (Theme::Dark, t!("theme-dark"), "#0f1522", "#111a29"), (Theme::Light, t!("theme-light"), "#f4f5f7", "#ffffff")];
    rsx! {
        section { class: "block",
            h2 { {t!("theme")} }
            div { class: "themes", role: "radiogroup", "aria-label": t!("theme"),
                for (t, label, a, b) in options {
                    button {
                        key: "{label}",
                        class: if t == theme { "theme-option on" } else { "theme-option" },
                        role: "radio",
                        "aria-checked": "{t == theme}",
                        onclick: move |_| state.theme.set(t.save()),
                        span { class: "swatch", span { style: "background: {a}" } span { style: "background: {b}" } }
                        span { "{label}" }
                    }
                }
            }
            p { class: "help", {t!("theme-help")} }
        }
        section { class: "block",
            h2 { {t!("language")} }
            div { class: "themes", role: "radiogroup", "aria-label": t!("language"),
                for (code, label) in std::iter::once((None, t!("theme-system"))).chain(i18n::languages().map(|(c, n)| (Some(c), n.to_string()))) {
                    button {
                        key: "{label}",
                        class: if code == chosen.read().as_deref() { "language-option on" } else { "language-option" },
                        role: "radio",
                        "aria-checked": "{code == chosen.read().as_deref()}",
                        lang: code.unwrap_or_default(),
                        onclick: move |_| {
                            if i18n::choose(code).is_ok() {
                                chosen.set(code.map(String::from));
                                state.language.set(i18n::language());
                                // The daemon's words too: errors, Tor's progress.
                                dioxus::core::spawn_forever(async move {
                                    call(Request::Language).await.ok();
                                });
                            }
                        },
                        "{label}"
                    }
                }
            }
            p { class: "help", {t!("language-help")} }
        }
        section { class: "block",
            h2 { {t!("terminal")} }
            div { class: "themes", role: "radiogroup", "aria-label": t!("terminal"),
                for (line, label) in std::iter::once(("", t!("theme-system"))).chain(installed.iter().map(|&(n, l)| (l, n.to_string()))) {
                    button {
                        key: "{label}",
                        class: if !*other.read() && line == *terminal.read() { "language-option on" } else { "language-option" },
                        role: "radio",
                        "aria-checked": "{!*other.read() && line == *terminal.read()}",
                        onclick: move |_| {
                            if save_terminal(line) {
                                terminal.set(line.to_string());
                                other.set(false);
                            }
                        },
                        "{label}"
                    }
                }
                button {
                    class: if *other.read() { "language-option on" } else { "language-option" },
                    role: "radio",
                    "aria-checked": "{other}",
                    onclick: move |_| other.set(true),
                    {t!("terminal-other")}
                }
            }
            if *other.read() {
                // Saved as typed: the next "Open in a terminal" uses it.
                input {
                    class: "name-field",
                    "aria-label": t!("terminal-other"),
                    placeholder: if cfg!(windows) { "wt" } else { "alacritty -e" },
                    spellcheck: false,
                    value: "{terminal}",
                    oninput: move |e| {
                        if save_terminal(e.value().trim()) {
                            terminal.set(e.value());
                        }
                    },
                }
                p { class: "help", {t!("terminal-other-help")} }
            } else {
                p { class: "help", {t!("terminal-help")} }
            }
        }
    }
}

#[component]
fn NetworkSection() -> Element {
    let state = use_context::<State>();
    let net = state.network.read().clone();
    let (title, detail) = net.summary();
    let dot = if net.entry == Some(false) || net.circuit == Some(false) {
        "net-dot big fail"
    } else if net.percent < 100 {
        "net-dot big live"
    } else {
        "net-dot big"
    };
    rsx! {
        section { class: "block",
            h2 { {t!("connection")} }
            div { class: "card",
                span { class: dot }
                span { class: "card-text",
                    span { class: "card-title", "{title}" }
                    span { class: "help", "{detail}" }
                }
            }
        }
        section { class: "block",
            h2 { {t!("bridges")} }
            BridgesEditor {}
        }
    }
}

/// Snowflake when needed, or lines of one's own, kept by the daemon in the
/// database; Tor reconnects through them without a restart. The daemon
/// answers this while Tor is still starting.
#[component]
pub fn BridgesEditor() -> Element {
    let mut own = use_signal(|| false);
    let mut text = use_signal(String::new);
    let mut error = use_signal(String::new);
    // Shown here: the status line is behind this page and the dialog.
    let mut saved = use_signal(String::new);
    use_future(move || async move {
        if let Ok(Reply::Bridges { lines }) = call(Request::Bridges { lines: None }).await {
            own.set(!lines.is_empty());
            text.set(lines.join("\n"));
        }
    });
    let save = move |_| {
        let lines: Vec<String> = if *own.read() { text.read().lines().map(String::from).collect() } else { Vec::new() };
        spawn(async move {
            match call(Request::Bridges { lines: Some(lines) }).await {
                Ok(Reply::Bridges { lines }) => {
                    error.set(String::new());
                    saved.set(if lines.is_empty() { t!("bridges-reset") } else { t!("bridges-saved") });
                }
                reply => {
                    saved.set(String::new());
                    error.set(match reply {
                        Ok(Reply::Error { message }) => message,
                        Ok(other) => t!("err-unexpected", reply = format!("{other:?}")),
                        Err(e) => t!("daemon-silent", error = format!("{e:#}")),
                    });
                }
            }
        });
    };
    let modes = [
        (false, t!("bridges-builtin"), t!("bridges-builtin-help")),
        (true, t!("bridges-own"), t!("bridges-own-help")),
    ];
    rsx! {
        p { class: "help", {t!("bridges-help")} }
        div { role: "radiogroup", "aria-label": t!("bridges"), class: "choices",
            for (mine, label, help) in modes {
                button {
                    key: "{label}",
                    class: if *own.read() == mine { "choice on" } else { "choice" },
                    role: "radio",
                    "aria-checked": "{*own.read() == mine}",
                    onclick: move |_| {
                        own.set(mine);
                        saved.set(String::new());
                    },
                    span { class: "radio" }
                    span { class: "card-text", span { class: "card-title", "{label}" } span { class: "help", "{help}" } }
                }
            }
        }
        if *own.read() {
            label { r#for: "bridges", class: "field-label", {t!("bridges-lines")} }
            textarea { id: "bridges", class: "lines-field", rows: "4", placeholder: "webtunnel [2001:db8::1]:443 … url=https://…", value: "{text}", oninput: move |e| {
                text.set(e.value());
                saved.set(String::new());
            } }
        }
        if !error.read().is_empty() { p { class: "warning", "{error}" } }
        div { button { class: "primary", onclick: save, {t!("bridges-save")} } }
        p { class: "help", role: "status", "{saved}" }
    }
}

#[component]
fn Notifications() -> Element {
    let mut state = use_context::<State>();
    let prefs = *state.prefs.read();
    let toggles = [
        (t!("notify-show"), t!("notify-show-help"), prefs.notify, 0),
        (t!("notify-sound"), t!("notify-sound-help"), prefs.sound, 1),
        (t!("notify-history"), t!("notify-history-help"), prefs.keep_history, 2),
    ];
    // (name, text), its label, and the notification it gives.
    let shows = [
        ((true, true), t!("notify-who-what"), t!("notify-example-name"), t!("notify-example-text")),
        ((true, false), t!("notify-who"), "Nightpost".into(), t!("notify-from", name = t!("notify-example-name"))),
        ((false, false), t!("notify-nothing"), "Nightpost".into(), t!("notify-new")),
    ];
    let shown = (prefs.show_name, prefs.show_name && prefs.show_text);
    rsx! {
        section { class: "block",
            h2 { {t!("notifications")} }
            for (label, help, on, which) in toggles {
                button {
                    key: "{label}",
                    class: "toggle-row",
                    role: "switch",
                    "aria-checked": "{on}",
                    onclick: move |_| {
                        let mut p = *state.prefs.read();
                        match which {
                            0 => p.notify = !p.notify,
                            1 => p.sound = !p.sound,
                            _ => p.keep_history = !p.keep_history,
                        }
                        state.prefs.set(p.save(dir()));
                    },
                    span { class: "card-text", span { class: "card-title", "{label}" } span { class: "help", "{help}" } }
                    span { class: if on { "switch on" } else { "switch" }, span {} }
                }
            }
        }
        section { class: "block",
            h2 { {t!("notify-shows")} }
            div { role: "radiogroup", "aria-label": t!("notify-shows"), class: "choices",
                for (choice, label, title, body) in shows {
                    button {
                        key: "{label}",
                        class: if shown == choice { "choice on" } else { "choice" },
                        role: "radio",
                        "aria-checked": "{shown == choice}",
                        onclick: move |_| {
                            let mut p = *state.prefs.read();
                            (p.show_name, p.show_text) = choice;
                            state.prefs.set(p.save(dir()));
                        },
                        span { class: "radio" }
                        span { class: "card-text",
                            span { class: "card-title", "{label}" }
                            span { class: "toast", b { "{title}" } span { "{body}" } }
                        }
                    }
                }
            }
            p { class: "help", {t!("notify-locked")} }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Change {
    Add,
    Replace,
    Remove,
}

#[component]
fn Security() -> Element {
    let mut state = use_context::<State>();
    let mut has = use_signal(|| dbkey::uses_passphrase(dir()));
    let mut change = use_signal(|| None::<Change>);
    let mut current = use_signal(String::new);
    let mut new = use_signal(String::new);
    let mut again = use_signal(String::new);
    let mut error = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let read_receipts = *state.read_receipts.read();
    let typing_indicators = *state.typing_indicators.read();

    let mut open = move |c: Option<Change>| {
        change.set(c);
        for mut field in [current, new, again, error] {
            field.set(String::new());
        }
    };
    let save = move |e: FormEvent| {
        e.prevent_default();
        let Some(c) = *change.read() else { return };
        let wants = c != Change::Remove;
        if wants && new.read().is_empty() {
            return error.set(t!("need-passphrase"));
        }
        if wants && *new.read() != *again.read() {
            return error.set(t!("passphrases-differ"));
        }
        let request = Request::SetPassphrase { current: (c != Change::Add).then(|| current.read().clone()), new: wants.then(|| new.read().clone()) };
        busy.set(true);
        spawn(async move {
            match call(request).await {
                Ok(Reply::Done) => {
                    has.set(wants);
                    open(None);
                    state.status.set(if wants { t!("passphrase-saved") } else { t!("passphrase-removed") });
                }
                Ok(Reply::Error { message }) => error.set(message),
                other => state.fail(other),
            }
            busy.set(false);
        });
    };

    let (title, help) = if *has.read() {
        (t!("locked-both"), t!("locked-both-help"))
    } else {
        (t!("locked-keychain"), t!("locked-keychain-help"))
    };
    let c = *change.read();
    let lock_on_close = state.prefs.read().lock_on_close;
    let leave_help = if *has.read() {
        t!("leave-lock-help")
    } else {
        t!("leave-quit-help")
    };
    rsx! {
        section { class: "block",
            h2 { {t!("how-locked")} }
            div { class: "card",
                svg { class: "card-icon", width: "22", height: "22", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
                    rect { x: "5", y: "11", width: "14", height: "10", rx: "2" }
                    path { d: "M8 11V8a4 4 0 0 1 8 0v3" }
                }
                span { class: "card-text",
                    span { class: "card-title", "{title}" }
                    span { class: "help", "{help}" }
                }
            }
            if c.is_none() {
                div { class: "row",
                    if *has.read() {
                        button { class: "ghost", onclick: move |_| open(Some(Change::Replace)), {t!("passphrase-change")} }
                        button { class: "ghost", onclick: move |_| open(Some(Change::Remove)), {t!("passphrase-remove")} }
                    } else {
                        button { class: "ghost", onclick: move |_| open(Some(Change::Add)), {t!("passphrase-add")} }
                    }
                }
            }
        }
        if let Some(c) = c {
            form { class: "block", onsubmit: save,
                h2 {
                    {match c {
                        Change::Add => t!("passphrase-add"),
                        Change::Replace => t!("passphrase-change"),
                        Change::Remove => t!("passphrase-remove"),
                    }}
                }
                if c != Change::Add {
                    label { r#for: "current", class: "field-label", {t!("passphrase-current")} }
                    input { id: "current", class: "name-field", r#type: "password", autofocus: true, value: "{current}", oninput: move |e| current.set(e.value()) }
                }
                if c != Change::Remove {
                    label { r#for: "new", class: "field-label", {t!("passphrase-new")} }
                    input { id: "new", class: "name-field", r#type: "password", autofocus: c == Change::Add, value: "{new}", oninput: move |e| new.set(e.value()) }
                    label { r#for: "again", class: "field-label", {t!("passphrase-again")} }
                    input { id: "again", class: "name-field", r#type: "password", value: "{again}", oninput: move |e| again.set(e.value()) }
                    p { class: "warning", {t!("passphrase-warning")} }
                } else {
                    p { class: "help", {t!("passphrase-remove-help")} }
                }
                if !error.read().is_empty() { p { class: "warning", "{error}" } }
                div { class: "row",
                    button { class: "primary", r#type: "submit", disabled: *busy.read(),
                        {match c {
                            Change::Add => t!("passphrase-add-it"),
                            Change::Replace => t!("change-it"),
                            Change::Remove => t!("remove-it"),
                        }}
                    }
                    button { class: "ghost", r#type: "button", onclick: move |_| open(None), {t!("cancel")} }
                }
            }
        }
        Recovery {}
        section { class: "block",
            h2 { {t!("read-receipts")} }
            button {
                class: "toggle-row",
                role: "switch",
                "aria-checked": "{read_receipts}",
                onclick: move |_| {
                    spawn(async move {
                        match call(Request::ReadReceipts { on: Some(!read_receipts) }).await {
                            Ok(Reply::ReadReceipts { on }) => state.read_receipts.set(on),
                            other => state.fail(other),
                        }
                    });
                },
                span { class: "card-text",
                    span { class: "card-title", {t!("read-receipts-send")} }
                    span { class: "help", {t!("read-receipts-help")} }
                }
                span { class: if read_receipts { "switch on" } else { "switch" }, span {} }
            }
        }
        section { class: "block",
            h2 { {t!("typing-indicators")} }
            button {
                class: "toggle-row",
                role: "switch",
                "aria-checked": "{typing_indicators}",
                onclick: move |_| {
                    spawn(async move {
                        match call(Request::TypingIndicators { on: Some(!typing_indicators) }).await {
                            Ok(Reply::TypingIndicators { on }) => state.typing_indicators.set(on),
                            other => state.fail(other),
                        }
                    });
                },
                span { class: "card-text",
                    span { class: "card-title", {t!("typing-indicators-send")} }
                    span { class: "help", {t!("typing-indicators-help")} }
                }
                span { class: if typing_indicators { "switch on" } else { "switch" }, span {} }
            }
        }
        section { class: "block",
            h2 { {t!("leaving")} }
            button {
                class: "toggle-row",
                role: "switch",
                "aria-checked": "{lock_on_close}",
                onclick: move |_| {
                    let mut p = *state.prefs.read();
                    p.lock_on_close = !p.lock_on_close;
                    state.prefs.set(p.save(dir()));
                },
                span { class: "card-text",
                    span { class: "card-title", {if *has.read() { t!("close-locks") } else { t!("close-quits") }} }
                    span { class: "help", "{leave_help}" }
                }
                span { class: if lock_on_close { "switch on" } else { "switch" }, span {} }
            }
            div {
                button {
                    class: "ghost",
                    onclick: move |_| {
                        crate::shut_down();
                        dioxus::desktop::window().close();
                    },
                    {if *has.read() { t!("quit-lock") } else { t!("quit") }}
                }
            }
        }
    }
}

/// The 24 words that open the database where the keychain has no key: shown
/// only on request, and forgotten when hidden.
#[component]
fn Recovery() -> Element {
    let state = use_context::<State>();
    let mut phrase = use_signal(|| None::<String>);
    let show = move |_| {
        if phrase.read().is_some() {
            return phrase.set(None);
        }
        spawn(async move {
            match call(Request::RecoveryPhrase).await {
                Ok(Reply::RecoveryPhrase { phrase: words }) => phrase.set(Some(words)),
                other => state.fail(other),
            }
        });
    };
    rsx! {
        section { class: "block",
            h2 { {t!("recovery-title")} }
            p { class: "help", {t!("recovery-help")} }
            if let Some(words) = phrase.read().as_ref() {
                ol { class: "recovery",
                    for (i, word) in words.split(' ').enumerate() {
                        li { key: "{i}", "{word}" }
                    }
                }
            }
            div { class: "row",
                button { class: "ghost", onclick: show, {if phrase.read().is_some() { t!("recovery-hide") } else { t!("recovery-show") }} }
                if let Some(words) = phrase.read().as_ref() {
                    CopyButton { text: words.clone(), label: t!("copy-phrase") }
                }
            }
        }
    }
}

#[component]
fn About() -> Element {
    let folder = dir().display().to_string();
    let licences = [
        (String::from("Nightpost"), "AGPL-3.0"),
        ("Space Grotesk".into(), "SIL Open Font License 1.1"),
        (t!("licence-bridges"), "BSD-3-Clause, Tor Project"),
        (t!("licence-icons"), "Lucide, ISC"),
    ];
    rsx! {
        section { class: "block",
            div { class: "about-head",
                span { class: "wordmark big", "Nightpost" }
                span { class: "reach-count",
                    {concat!(env!("CARGO_PKG_VERSION_MAJOR"), ".", env!("CARGO_PKG_VERSION_MINOR"), ".", env!("CARGO_PKG_VERSION_PATCH"))}
                    // "alpha.1" says the version is not stable yet: it gets a mark of its own.
                    if !env!("CARGO_PKG_VERSION_PRE").is_empty() {
                        span { class: "pre-release", {env!("CARGO_PKG_VERSION_PRE").replace('.', " ")} }
                    }
                }
            }
            p { class: "help", {t!("tagline")} }
        }
        section { class: "block",
            h2 { {t!("your-data")} }
            div { class: "kv", span { {t!("folder")} } span { "{folder}" } }
            div { button { class: "ghost", onclick: move |_| open_folder(dir()), {t!("open-folder")} } }
        }
        section { class: "block",
            h2 { {t!("licences")} }
            for (what, licence) in licences {
                div { key: "{what}", class: "kv", span { "{what}" } span { "{licence}" } }
            }
        }
    }
}

fn open_folder(path: &std::path::Path) {
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut command = std::process::Command::new(opener);
    as_launched(&mut command);
    let _ = command.arg(path).spawn();
}

/// The terminal's command line for the tray; empty: the system's.
fn save_terminal(line: &str) -> bool {
    let file = dir().join(crate::tray::TERMINAL_FILE);
    if line.is_empty() { std::fs::remove_file(file).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }) } else { std::fs::write(file, line) }.is_ok()
}
