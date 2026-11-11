//! The window's own frame. There is no title bar from the system: the
//! buttons sit in the top right corner, over the first row of the app, the
//! empty parts of the top rows move the window, and its edges resize it.

use dioxus::desktop::tao::window::ResizeDirection;
use dioxus::desktop::window;
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;

use nightpost_core::t;

use crate::State;

/// For a top row's empty part: pressed, it moves the window; double-clicked,
/// it maximises or restores it.
pub fn drag(e: MouseEvent) {
    if e.trigger_button() == Some(MouseButton::Primary) {
        window().drag();
    }
}

pub fn toggle_maximized(_: MouseEvent) {
    window().toggle_maximized();
}

const EDGES: [(&str, ResizeDirection); 8] = [
    ("n", ResizeDirection::North),
    ("s", ResizeDirection::South),
    ("e", ResizeDirection::East),
    ("w", ResizeDirection::West),
    ("ne", ResizeDirection::NorthEast),
    ("nw", ResizeDirection::NorthWest),
    ("se", ResizeDirection::SouthEast),
    ("sw", ResizeDirection::SouthWest),
];

#[component]
pub fn Chrome() -> Element {
    let state = use_context::<State>();
    let maximized = *state.maximized.read();
    rsx! {
        if !maximized {
            for (side, direction) in EDGES {
                div {
                    key: "{side}",
                    class: "edge {side}",
                    onmousedown: move |e: MouseEvent| {
                        if e.trigger_button() == Some(MouseButton::Primary) {
                            let _ = window().drag_resize_window(direction);
                        }
                    },
                }
            }
        }
        div { class: "window-controls",
            button { title: t!("minimise"), "aria-label": t!("minimise"), onclick: move |_| window().set_minimized(true),
                svg { width: "16", height: "16", view_box: "0 0 16 16", fill: "none", stroke: "currentColor", stroke_width: "1.6", stroke_linecap: "round", "aria-hidden": "true",
                    path { d: "M3.5 8.5h9" }
                }
            }
            button {
                title: if maximized { t!("restore") } else { t!("maximise") },
                "aria-label": if maximized { t!("restore") } else { t!("maximise") },
                onclick: toggle_maximized,
                svg { width: "16", height: "16", view_box: "0 0 16 16", fill: "none", stroke: "currentColor", stroke_width: "1.6", stroke_linejoin: "round", "aria-hidden": "true",
                    if maximized {
                        rect { x: "3.5", y: "5.5", width: "7", height: "7", rx: "1.5" }
                        path { d: "M6 3.5h5a1.5 1.5 0 0 1 1.5 1.5v5" }
                    } else {
                        rect { x: "3.5", y: "3.5", width: "9", height: "9", rx: "1.8" }
                    }
                }
            }
            button {
                class: "quit",
                title: t!("close"),
                "aria-label": t!("close"),
                onclick: move |_| {
                    // Closing from here is not the system's close request,
                    // which the app listens to: the choice is honoured here.
                    if state.prefs.peek().lock_on_close || nightpost_core::portable() {
                        crate::shut_down();
                    }
                    window().close();
                },
                svg { width: "16", height: "16", view_box: "0 0 16 16", fill: "none", stroke: "currentColor", stroke_width: "1.6", stroke_linecap: "round", "aria-hidden": "true",
                    path { d: "M4.5 4.5l7 7M11.5 4.5l-7 7" }
                }
            }
        }
    }
}

/// Round corners on Linux. WebKitGTK (2.54) keeps only the changed part of a
/// see-through page, so the page stays opaque and the corners are cut out of
/// the window once GTK has drawn it into a see-through window.
#[cfg(target_os = "linux")]
pub fn cut_corners() {
    use std::f64::consts::FRAC_PI_2;

    use dioxus::desktop::tao::platform::unix::WindowExtUnix as _;
    use gtk::cairo::{Context, FillRule, Operator};
    use gtk::glib::prelude::*;
    use gtk::prelude::*;

    let desktop = window();
    desktop.window.gtk_window().connect_local("draw", true, |args| {
        let gtk_window = args[0].get::<gtk::Window>().ok()?;
        let cr = args[1].get::<Context>().ok()?;
        if !gtk_window.is_maximized() {
            let (w, h) = (gtk_window.allocated_width() as f64, gtk_window.allocated_height() as f64);
            // As --radius in style.css, which draws the edge along the curve.
            let r = 12.0;
            cr.set_operator(Operator::Clear);
            cr.set_fill_rule(FillRule::EvenOdd);
            cr.rectangle(0.0, 0.0, w, h);
            cr.new_sub_path();
            cr.arc(w - r, r, r, -FRAC_PI_2, 0.0);
            cr.arc(w - r, h - r, r, 0.0, FRAC_PI_2);
            cr.arc(r, h - r, r, FRAC_PI_2, 2.0 * FRAC_PI_2);
            cr.arc(r, r, r, 2.0 * FRAC_PI_2, 3.0 * FRAC_PI_2);
            cr.close_path();
            let _ = cr.fill();
        }
        Some(false.to_value())
    });
}

/// No pinch zoom on a touchpad: an app is not a page to magnify. WebKitGTK
/// (GTK 3) handles the pinch in its class handler for `event`, which runs
/// after ours; stopping the pinch there leaves it nothing to do.
#[cfg(target_os = "linux")]
pub fn no_pinch_zoom() {
    use dioxus::desktop::tao::platform::unix::WindowExtUnix as _;
    use gtk::prelude::*;

    fn find_page(widget: &gtk::Widget) -> Option<gtk::Widget> {
        if widget.type_().name() == "WebKitWebView" {
            return Some(widget.clone());
        }
        widget.downcast_ref::<gtk::Container>()?.children().iter().find_map(find_page)
    }
    let gtk_window = window().window.gtk_window().clone().upcast::<gtk::Widget>();
    if let Some(page) = find_page(&gtk_window) {
        page.connect_event(|_, event| {
            if event.event_type() == gtk::gdk::EventType::TouchpadPinch { gtk::glib::Propagation::Stop } else { gtk::glib::Propagation::Proceed }
        });
    }
}

