//! The settings page: how the app looks, reaches Tor, notifies, keeps
//! messages locked, and what it is.

use dioxus::prelude::*;
use nightpost_core::transport::tor;

use crate::{State, Theme, View, dir, network_summary};

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
                h1 { "Settings" }
                button { class: "close", "aria-label": "Close settings", onclick: move |_| state.view.set(View::Chat),
                    svg { width: "20", height: "20", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", "aria-hidden": "true", path { d: "M6 6l12 12M18 6L6 18" } }
                }
            }
            div { class: "settings-body",
                nav { class: "sections", "aria-label": "Settings sections",
                    for (s, label) in SECTIONS {
                        button {
                            key: "{label}",
                            class: if s == section { "section on" } else { "section" },
                            "aria-current": if s == section { "page" } else { "false" },
                            onclick: move |_| state.view.set(View::Settings(s)),
                            span { class: "tile", {icon(s)} }
                            "{label}"
                        }
                    }
                }
                div { class: "settings-content",
                    match section {
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
    ];
    let shows = [(true, "Who wrote", "Message from Léa"), (false, "Nothing", "New message")];
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
                        if which == 0 { p.notify = !p.notify } else { p.sound = !p.sound }
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
                for (name, label, sample) in shows {
                    button {
                        key: "{label}",
                        class: if prefs.show_name == name { "choice on" } else { "choice" },
                        role: "radio",
                        "aria-checked": "{prefs.show_name == name}",
                        onclick: move |_| {
                            let mut p = *state.prefs.read();
                            p.show_name = name;
                            state.prefs.set(p.save());
                        },
                        span { class: "radio" }
                        span { class: "card-text",
                            span { class: "card-title", "{label}" }
                            span { class: "toast", b { "Nightpost" } span { "{sample}" } }
                        }
                    }
                }
            }
            p { class: "help", "Never the text of a message: a notification can show on a locked screen." }
        }
    }
}

#[component]
fn Security() -> Element {
    rsx! {
        section { class: "block",
            h2 { "How your messages are locked" }
            div { class: "card",
                svg { class: "card-icon", width: "22", height: "22", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
                    rect { x: "5", y: "11", width: "14", height: "10", rx: "2" }
                    path { d: "M8 11V8a4 4 0 0 1 8 0v3" }
                }
                span { class: "card-text",
                    span { class: "card-title", "With your system keychain" }
                    span { class: "help", "Your messages and keys are stored encrypted. The key opens when you log in to this computer." }
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
                span { class: "reach-count", "{env!(\"CARGO_PKG_VERSION\")}" }
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
