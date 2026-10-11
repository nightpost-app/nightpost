// The icon inside nightpost-gui.exe: what Explorer, the taskbar and Alt+Tab
// show. Elsewhere the system takes the icon from the app's bundle or
// desktop file. Id 32512 (IDI_APPLICATION) is where dioxus-desktop looks
// for the window's own icon; under any other the taskbar shows its default.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new().set_icon_with_id("../../icons/nightpost.ico", "32512").compile().expect("embedding the icon");
    }
}
