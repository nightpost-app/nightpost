//! The settings page: how the app looks, reaches Tor, notifies, keeps
//! messages locked, and what it is.

use dioxus::prelude::*;
use nightpost_core::api::{Reply, Request};
use nightpost_core::dbkey;
use nightpost_core::transport::tor;

use crate::{State, Theme, View, call, dir, network_summary};

#[derive(Clone, Copy, PartialEq)]
pub enum Section {
    Appearance,
    Network,
    Notifications,
    Security,
    About,
}

const SECTIONS: [(Section, &str); 5] = [
    (Section::Appearance, "Appearance"),
    (Section::Network, "Network"),
    (Section::Notifications, "Notifications"),
    (Section::Security, "Security"),
    (Section::About, "About"),
];

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
                button { class: "close", title: "Back to the conversation", "aria-label": "Back to the conversation", onclick: move |_| state.view.set(View::Chat),
                    svg { width: "20", height: "20", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true", path { d: "M15 5l-7 7 7 7" } }
                }
                h1 { "Settings" }
                div { class: "drag-fill", onmousedown: crate::chrome::drag, ondoubleclick: crate::chrome::toggle_maximized }
            }
            div { class: "settings-body",
                nav { class: "sections", "aria-label": "Settings sections",
                    for (s, label) in SECTIONS {
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
    let options = [(Theme::System, "As the system", "#0f1522", "#f4f5f7"), (Theme::Dark, "Dark", "#0f1522", "#111a29"), (Theme::Light, "Light", "#f4f5f7", "#ffffff")];
    rsx! {
        section { class: "block",
            h2 { "Theme" }
            div { class: "themes", role: "radiogroup", "aria-label": "Theme",
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
            p { class: "help", "“As the system” follows your computer’s light or dark mode, and changes with it." }
        }
    }
}

#[component]
fn NetworkSection() -> Element {
    let state = use_context::<State>();
    let net = state.network.read().clone();
    let (title, detail) = network_summary(&net);
    let dot = if net.entry == Some(false) || net.circuit == Some(false) {
        "net-dot big fail"
    } else if net.percent < 100 {
        "net-dot big live"
    } else {
        "net-dot big"
    };
    rsx! {
        section { class: "block",
            h2 { "Connection" }
            div { class: "card",
                span { class: dot }
                span { class: "card-text",
                    span { class: "card-title", "{title}" }
                    span { class: "help", "{detail}" }
                }
            }
        }
        section { class: "block",
            h2 { "Bridges" }
            BridgesEditor {}
        }
    }
}

/// Snowflake when needed, or lines of one's own, checked and written to
/// the bridges file; the daemon picks the change up without a restart.
#[component]
pub fn BridgesEditor() -> Element {
    let mut state = use_context::<State>();
    let file = dir().join(tor::BRIDGES_FILE);
    let saved = tor::read_bridges(&file);
    let mut own = use_signal(|| !saved.is_empty());
    let mut text = use_signal(|| saved.join("\n"));
    let mut error = use_signal(String::new);
    let save = move |_| {
        let lines: Vec<String> = if *own.read() {
            text.read().lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect()
        } else {
            Vec::new()
        };
        match tor::save_bridges(&file, &lines) {
            Ok(()) => {
                error.set(String::new());
                state.status.set(if lines.is_empty() { "Back to the built-in bridges".into() } else { "Bridges saved: Tor reconnects through them".into() });
            }
            Err(e) => error.set(e.to_string()),
        }
    };
    let modes = [
        (false, "Built-in, when needed", "Snowflake, nothing to set up. Used only if Tor does not get through."),
        (true, "Bridges of your own", "From a friend, bridges.torproject.org (WebTunnel gets through most often), or your own server. Used all the time."),
    ];
    rsx! {
        p { class: "help", "Some networks (university or work Wi-Fi) block Tor. Nightpost then goes in through a bridge, an entrance that is not on the public list." }
        div { role: "radiogroup", "aria-label": "Bridges", class: "choices",
            for (mine, label, help) in modes {
                button {
                    key: "{label}",
                    class: if *own.read() == mine { "choice on" } else { "choice" },
                    role: "radio",
                    "aria-checked": "{*own.read() == mine}",
                    onclick: move |_| own.set(mine),
                    span { class: "radio" }
                    span { class: "card-text", span { class: "card-title", "{label}" } span { class: "help", "{help}" } }
                }
            }
        }
        if *own.read() {
            label { r#for: "bridges", class: "field-label", "Bridge lines, one per line" }
            textarea { id: "bridges", class: "lines-field", rows: "4", placeholder: "webtunnel [2001:db8::1]:443 … url=https://…", value: "{text}", oninput: move |e| text.set(e.value()) }
        }
        if !error.read().is_empty() { p { class: "warning", "{error}" } }
        div { button { class: "primary", onclick: save, "Save bridges" } }
    }
}

#[component]
fn Notifications() -> Element {
    let mut state = use_context::<State>();
    let prefs = *state.prefs.read();
    let toggles = [
        ("Show notifications", "When a message arrives and Nightpost is not in front.", prefs.notify, 0),
        ("Play a sound", "A short chime with each notification.", prefs.sound, 1),
        ("Keep in history", "Off: the system lets each one go once shown, rather than keeping it in its list of past notifications.", prefs.keep_history, 2),
    ];
    // (name, text), its label, and the notification it gives.
    let shows = [
        ((true, true), "Who and what", "Léa", "See you at eight?"),
        ((true, false), "Who wrote", "Nightpost", "Message from Léa"),
        ((false, false), "Nothing", "Nightpost", "New message"),
    ];
    let shown = (prefs.show_name, prefs.show_name && prefs.show_text);
    rsx! {
        section { class: "block",
            h2 { "Notifications" }
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
                        state.prefs.set(p.save());
                    },
                    span { class: "card-text", span { class: "card-title", "{label}" } span { class: "help", "{help}" } }
                    span { class: if on { "switch on" } else { "switch" }, span {} }
                }
            }
        }
        section { class: "block",
            h2 { "What a notification shows" }
            div { role: "radiogroup", "aria-label": "What a notification shows", class: "choices",
                for (choice, label, title, body) in shows {
                    button {
                        key: "{label}",
                        class: if shown == choice { "choice on" } else { "choice" },
                        role: "radio",
                        "aria-checked": "{shown == choice}",
                        onclick: move |_| {
                            let mut p = *state.prefs.read();
                            (p.show_name, p.show_text) = choice;
                            state.prefs.set(p.save());
                        },
                        span { class: "radio" }
                        span { class: "card-text",
                            span { class: "card-title", "{label}" }
                            span { class: "toast", b { "{title}" } span { "{body}" } }
                        }
                    }
                }
            }
            p { class: "help", "A notification can show on a locked screen." }
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
            return error.set("Choose a passphrase.".into());
        }
        if wants && *new.read() != *again.read() {
            return error.set("The two passphrases differ.".into());
        }
        let request = Request::SetPassphrase { current: (c != Change::Add).then(|| current.read().clone()), new: wants.then(|| new.read().clone()) };
        busy.set(true);
        spawn(async move {
            match call(request).await {
                Ok(Reply::Done) => {
                    has.set(wants);
                    open(None);
                    state.status.set(if wants { "Passphrase saved".into() } else { "Passphrase removed".into() });
                }
                Ok(Reply::Error { message }) => error.set(message),
                other => state.fail(other),
            }
            busy.set(false);
        });
    };

    let (title, help) = if *has.read() {
        ("With your keychain and a passphrase", "Your messages and keys are stored encrypted. Nightpost asks for the passphrase each time it starts.")
    } else {
        ("With your system keychain", "Your messages and keys are stored encrypted. The key opens when you log in to this computer.")
    };
    let c = *change.read();
    let lock_on_close = state.prefs.read().lock_on_close;
    let leave_help = if *has.read() {
        "Messages stop arriving, and your passphrase is asked for when you open Nightpost again."
    } else {
        "Messages stop arriving until you open Nightpost again. Add a passphrase above for this to lock it."
    };
    rsx! {
        section { class: "block",
            h2 { "How your messages are locked" }
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
                        button { class: "ghost", onclick: move |_| open(Some(Change::Replace)), "Change the passphrase" }
                        button { class: "ghost", onclick: move |_| open(Some(Change::Remove)), "Remove the passphrase" }
                    } else {
                        button { class: "ghost", onclick: move |_| open(Some(Change::Add)), "Add a passphrase" }
                    }
                }
            }
        }
        if let Some(c) = c {
            form { class: "block", onsubmit: save,
                h2 {
                    match c {
                        Change::Add => "Add a passphrase",
                        Change::Replace => "Change the passphrase",
                        Change::Remove => "Remove the passphrase",
                    }
                }
                if c != Change::Add {
                    label { r#for: "current", class: "field-label", "Current passphrase" }
                    input { id: "current", class: "name-field", r#type: "password", autofocus: true, value: "{current}", oninput: move |e| current.set(e.value()) }
                }
                if c != Change::Remove {
                    label { r#for: "new", class: "field-label", "New passphrase" }
                    input { id: "new", class: "name-field", r#type: "password", autofocus: c == Change::Add, value: "{new}", oninput: move |e| new.set(e.value()) }
                    label { r#for: "again", class: "field-label", "The same again" }
                    input { id: "again", class: "name-field", r#type: "password", value: "{again}", oninput: move |e| again.set(e.value()) }
                    p { class: "warning", "Forget it and your messages are gone: nobody can recover it, not even with this computer." }
                } else {
                    p { class: "help", "The keychain alone will open your messages again." }
                }
                if !error.read().is_empty() { p { class: "warning", "{error}" } }
                div { class: "row",
                    button { class: "primary", r#type: "submit", disabled: *busy.read(),
                        match c {
                            Change::Add => "Add the passphrase",
                            Change::Replace => "Change it",
                            Change::Remove => "Remove it",
                        }
                    }
                    button { class: "ghost", r#type: "button", onclick: move |_| open(None), "Cancel" }
                }
            }
        }
        section { class: "block",
            h2 { "Leaving" }
            button {
                class: "toggle-row",
                role: "switch",
                "aria-checked": "{lock_on_close}",
                onclick: move |_| {
                    let mut p = *state.prefs.read();
                    p.lock_on_close = !p.lock_on_close;
                    state.prefs.set(p.save());
                },
                span { class: "card-text",
                    span { class: "card-title", if *has.read() { "Closing the window locks Nightpost" } else { "Closing the window quits Nightpost" } }
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
                    if *has.read() { "Quit and lock" } else { "Quit Nightpost" }
                }
            }
        }
    }
}

#[component]
fn About() -> Element {
    let folder = dir().display().to_string();
    let licences = [
        ("Nightpost", "AGPL-3.0"),
        ("Space Grotesk", "SIL Open Font License 1.1"),
        ("lyrebird (bridges)", "BSD-3-Clause, Tor Project"),
        ("Icons", "Lucide, ISC"),
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
            p { class: "help", "Messages between people who know each other, over Tor, with no server in between." }
        }
        section { class: "block",
            h2 { "Your data" }
            div { class: "kv", span { "Folder" } span { "{folder}" } }
            div { button { class: "ghost", onclick: move |_| open_folder(dir()), "Open the folder" } }
        }
        section { class: "block",
            h2 { "Licences" }
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
    let _ = std::process::Command::new(opener).arg(path).spawn();
}
