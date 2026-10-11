//! Nightpost engine. Both interfaces (terminal and graphical) only call into
//! this crate; nothing about identity, encryption or the network lives in them.

pub mod api;
pub mod daemon;
pub mod dbkey;
pub mod identity;
pub mod invite;
pub mod proto;
pub mod session;
pub mod store;
pub mod text;
pub mod transport;

/// Nightpost's version, the same for the daemon and every interface.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Who notifications come from, for Windows.
pub const APP_ID: &str = "io.github.nightpost-app.Nightpost";

/// Tells Windows the name and picture to show notifications under, in the
/// user's registry (without it they read "Windows PowerShell"). The picture
/// is written to the data folder: Windows wants a file.
#[cfg(windows)]
pub fn register_app_id(dir: &std::path::Path) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let icon = dir.join("icon.png");
    if let Err(e) = std::fs::write(&icon, include_bytes!("../../../icons/nightpost.png")) {
        tracing::warn!("writing the notification icon: {e}");
    }
    let key = format!("HKCU\\Software\\Classes\\AppUserModelId\\{APP_ID}");
    // The data folder comes canonical, \\?\C:\..., which Windows does not
    // take here.
    let icon = icon.display().to_string();
    let icon = icon.strip_prefix(r"\\?\").unwrap_or(&icon);
    for (name, value) in [("DisplayName", "Nightpost"), ("IconUri", icon)] {
        let done = std::process::Command::new("reg")
            .args(["add", &key, "/v", name, "/t", "REG_SZ", "/d", value, "/f"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        if !done.is_ok_and(|s| s.success()) {
            tracing::warn!("registering how notifications are shown ({name})");
        }
    }
}

/// Version of the wire protocol. Peers must match exactly.
pub const PROTOCOL_VERSION: u16 = 1;
