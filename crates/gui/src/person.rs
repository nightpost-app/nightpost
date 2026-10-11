//! Someone's panel, beside the conversation: photo, name, safety number,
//! notifications, removal.

use anyhow::{Result, ensure};
use dioxus::prelude::*;
use nightpost_core::api::{MAX_PHOTO, Person, Reply, Request};

use nightpost_core::t;

use crate::{Dialog, State, avatar, call, clock, muted, now_millis};

/// Changes what we keep about them; the daemon announces the result.
/// `spawn_forever`: the caller may be a dialog that closes as it calls.
pub fn update(state: State, name: String, change: impl FnOnce(&mut Person)) {
    let mut person = state.people.read().get(&name).cloned().unwrap_or_default();
    change(&mut person);
    dioxus::core::spawn_forever(async move {
        match call(Request::SetPerson { name, person }).await {
            Ok(Reply::Done) => {}
            other => state.fail(other),
        }
    });
}

/// The picked image, decoded and re-encoded as a 256 × 256 PNG: whatever
/// the file held besides pixels does not survive, and nothing but our own
/// PNG is ever put in the page.
pub fn photo_uri(bytes: &[u8]) -> Result<String> {
    ensure!(bytes.len() <= 30 << 20, t!("photo-too-big"));
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    reader.limits(limits);
    let picture = reader.decode()?.resize_to_fill(256, 256, image::imageops::FilterType::Triangle);
    let mut png = Vec::new();
    picture.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    let uri = format!("data:image/png;base64,{}", data_encoding::BASE64.encode(&png));
    ensure!(uri.len() <= MAX_PHOTO, t!("photo-too-detailed"));
    Ok(uri)
}

const HOUR: u64 = 3_600_000;

#[component]
pub fn PersonPanel(name: String) -> Element {
    let mut state = use_context::<State>();
    let person = state.people.read().get(&name).cloned().unwrap_or_default();
    let mut new_name = use_signal(|| name.clone());
    let mut removing = use_signal(|| false);
    let mut wipe = use_signal(|| false);

    let quiet = muted(&person);
    let mute_choice = match quiet {
        None => "on",
        Some(u64::MAX) => "off",
        Some(until) if until - now_millis() > 2 * HOUR => "8h",
        Some(_) => "1h",
    };
    let mute_help = match quiet {
        None => t!("mute-on-help", name = name),
        Some(u64::MAX) => t!("mute-off-help", name = name),
        Some(until) => t!("mute-until-help", time = clock(until)),
    };
    let mutes = [("on", t!("mute-on"), None), ("1h", t!("mute-1h"), Some(HOUR)), ("8h", t!("mute-8h"), Some(8 * HOUR)), ("off", t!("mute-off"), Some(u64::MAX))];

    let (pick, rings, rename, compare, remove) = (name.clone(), name.clone(), name.clone(), name.clone(), name.clone());
    rsx! {
        aside { class: "person", "aria-label": t!("about-name", name = name),
            div { class: "person-top",
                button { class: "close", "aria-label": t!("close"), onclick: move |_| state.panel.set(false),
                    svg { width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", "aria-hidden": "true", path { d: "M6 6l12 12M18 6L6 18" } }
                }
                div { class: "drag-fill", onmousedown: crate::chrome::drag, ondoubleclick: crate::chrome::toggle_maximized }
            }
            div { class: "person-photo",
                {avatar(&name, person.photo.as_deref(), 96)}
                div { class: "row",
                    label { class: "small-button",
                        {if person.photo.is_some() { t!("change-photo") } else { t!("choose-photo") }}
                        input {
                            r#type: "file",
                            accept: "image/png, image/jpeg, image/webp, image/gif",
                            class: "hidden-file",
                            onchange: move |e: FormEvent| {
                                let Some(file) = e.files().into_iter().next() else { return };
                                let name = pick.clone();
                                spawn(async move {
                                    let picture = match file.read_bytes().await {
                                        Ok(bytes) => photo_uri(&bytes),
                                        Err(e) => Err(anyhow::anyhow!("{e}")),
                                    };
                                    match picture {
                                        Ok(uri) => update(state, name, |p| p.photo = Some(uri)),
                                        Err(e) => state.status.set(t!("photo-failed", error = format!("{e:#}"))),
                                    }
                                });
                            },
                        }
                    }
                    if person.photo.is_some() {
                        button { class: "small-button", onclick: move |_| update(state, rings.clone(), |p| p.photo = None), {t!("use-rings")} }
                    }
                }
                p { class: "help", {t!("photo-help", name = name)} }
            }

            section { class: "block",
                label { r#for: "rename", class: "panel-title", {t!("name")} }
                div { class: "row",
                    input { id: "rename", class: "name-field", value: "{new_name}", maxlength: "32", oninput: move |e| new_name.set(e.value()) }
                    button {
                        class: "small-button",
                        disabled: new_name.read().trim().is_empty() || *new_name.read() == name,
                        onclick: move |_| {
                            let (from, to) = (rename.clone(), new_name.read().trim().to_string());
                            // The panel is keyed by name: renaming replaces it.
                            dioxus::core::spawn_forever(async move {
                                match call(Request::Rename { name: from, to }).await {
                                    Ok(Reply::Done) => {}
                                    other => state.fail(other),
                                }
                            });
                        },
                        {t!("rename")}
                    }
                }
                p { class: "help", {t!("name-help")} }
            }

            section { class: "block",
                div { class: "row spread",
                    h2 { class: "panel-title", {t!("safety-number")} }
                    span { class: if person.verified { "chip on" } else { "chip" }, {if person.verified { t!("verified") } else { t!("not-compared") }} }
                }
                p { class: "help",
                    {if person.verified { t!("compared-help", name = name) } else { t!("compare-help", name = name) }}
                }
                div {
                    button {
                        class: "small-button",
                        onclick: move |_| {
                            let name = compare.clone();
                            spawn(async move {
                                match call(Request::Verify { name: name.clone() }).await {
                                    Ok(Reply::SafetyNumber { number, .. }) => state.dialog.set(Some(Dialog::Safety { name, number })),
                                    other => state.fail(other),
                                }
                            });
                        },
                        {t!("compare")}
                    }
                }
            }

            section { class: "block",
                h2 { class: "panel-title", {t!("notifications")} }
                div { class: "segmented", role: "radiogroup", "aria-label": t!("notifications-from", name = name),
                    for (id, label, length) in mutes {
                        {
                            let who = name.clone();
                            rsx! {
                                button {
                                    key: "{id}",
                                    class: if id == mute_choice { "on" } else { "" },
                                    role: "radio",
                                    "aria-checked": "{id == mute_choice}",
                                    onclick: move |_| {
                                        let until = length.map(|l: u64| if l == u64::MAX { l } else { now_millis() + l });
                                        update(state, who.clone(), |p| p.muted_until = until);
                                    },
                                    "{label}"
                                }
                            }
                        }
                    }
                }
                p { class: "help", "{mute_help}" }
            }

            section { class: "block danger-zone",
                if *removing.read() {
                    h2 { class: "panel-title danger-text", {t!("remove-title", name = name)} }
                    p { class: "help", {t!("remove-help", name = name)} }
                    button { class: "check", role: "checkbox", "aria-checked": "{wipe}", onclick: move |_| { let w = *wipe.read(); wipe.set(!w); },
                        span { class: if *wipe.read() { "box on" } else { "box" }, if *wipe.read() { "✓" } }
                        {t!("remove-wipe")}
                    }
                    div { class: "row",
                        button {
                            class: "danger",
                            onclick: move |_| {
                                let (name, wipe) = (remove.clone(), *wipe.read());
                                state.panel.set(false);
                                // Not `spawn`: this panel goes away with them.
                                dioxus::core::spawn_forever(async move {
                                    match call(Request::Remove { name: name.clone(), wipe }).await {
                                        Ok(Reply::Done) => state.status.set(t!("removed", name = name)),
                                        other => state.fail(other),
                                    }
                                });
                            },
                            {t!("remove")}
                        }
                        button { class: "small-button", onclick: move |_| { removing.set(false); wipe.set(false); }, {t!("cancel")} }
                    }
                } else {
                    button { class: "danger-ghost", onclick: move |_| removing.set(true), {t!("remove-name", name = name)} }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photo_becomes_a_small_png() {
        let mut jpeg = Vec::new();
        image::RgbImage::from_pixel(640, 480, image::Rgb([200, 80, 40])).write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg).unwrap();
        let uri = photo_uri(&jpeg).unwrap();
        let png = data_encoding::BASE64.decode(uri.strip_prefix("data:image/png;base64,").unwrap().as_bytes()).unwrap();
        let picture = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        assert_eq!((picture.width(), picture.height()), (256, 256));
        assert!(photo_uri(b"<svg onload=alert(1)>").is_err());
    }
}
