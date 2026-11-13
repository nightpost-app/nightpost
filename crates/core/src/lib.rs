//! Nightpost engine. Both interfaces (terminal and graphical) only call into
//! this crate; nothing about identity, encryption or the network lives in them.

pub mod accounts;
pub mod api;
pub mod daemon;
pub mod dbkey;
pub mod devices;
pub mod i18n;
pub mod identity;
pub mod invite;
pub mod mailbox;
pub mod prefs;
pub mod proto;
pub mod session;
pub mod store;
pub mod terminal;
pub mod text;
pub mod transport;

/// Run from a removable key (`NIGHTPOST_PORTABLE=1`, set by its launchers):
/// the computer is only borrowed.
pub fn portable() -> bool {
    std::env::var_os("NIGHTPOST_PORTABLE").is_some_and(|v| v == "1")
}

/// What a portable daemon keeps off the key, whose file system (exFAT)
/// holds no socket and no private permissions: in this user's private
/// runtime folder, one name per data directory.
#[cfg(unix)]
pub fn runtime_path(dir: &std::path::Path, what: &str) -> anyhow::Result<std::path::PathBuf> {
    use sha2::{Digest as _, Sha256};
    #[cfg(target_os = "macos")]
    let runtime = std::env::temp_dir();
    #[cfg(not(target_os = "macos"))]
    let runtime = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| anyhow::anyhow!("no XDG_RUNTIME_DIR"))?);
    let hash = Sha256::digest(dir.to_string_lossy().as_bytes());
    Ok(runtime.join(format!("nightpost-{}.{what}", hash[..8].iter().map(|b| format!("{b:02x}")).collect::<String>())))
}

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

/// Three rings for a name without a photo, outside in: the same on every
/// machine (FNV-1a, unlike std's hasher), so a name always looks the same.
/// The terminal interface colours names with the middle one.
pub fn rings(name: &str) -> [&'static str; 3] {
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
    let hash = name.bytes().fold(0x811c_9dc5_u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    RINGS[hash as usize % RINGS.len()]
}

/// Version of the wire protocol. Peers must match exactly.
/// 2: several devices per account (alpha.2); 1 knew one device only and
/// would silently never reach the others.
pub const PROTOCOL_VERSION: u16 = 2;
