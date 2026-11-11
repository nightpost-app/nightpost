//! The terminal that the tray icon's "Open in the terminal" opens, chosen in
//! the settings of the window or of the terminal interface: a command line,
//! kept in the data folder, the program added at its end.

use std::path::Path;

/// The terminals offered: name, and the command line the program is added
/// to. Any other command line can be typed in.
#[cfg(not(windows))]
pub const TERMINALS: [(&str, &str); 11] = [
    ("Ghostty", "ghostty -e"),
    ("Konsole", "konsole -e"),
    ("Ptyxis", "ptyxis --"),
    ("GNOME Terminal", "gnome-terminal --"),
    ("Console", "kgx -e"),
    ("Xfce Terminal", "xfce4-terminal -x"),
    ("Alacritty", "alacritty -e"),
    ("kitty", "kitty"),
    ("foot", "foot"),
    ("WezTerm", "wezterm start --"),
    ("XTerm", "xterm -e"),
];
#[cfg(windows)]
pub const TERMINALS: [(&str, &str); 3] = [("Windows Terminal", "wt"), ("Alacritty", "alacritty -e"), ("WezTerm", "wezterm start --")];

/// The command line picked; none: the system's.
const FILE: &str = "terminal";

/// Those of [`TERMINALS`] on the PATH.
pub fn installed() -> Vec<(&'static str, &'static str)> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let found = |line: &str| {
        let command = line.split(' ').next().unwrap_or_default();
        // Not is_file: Windows Terminal's `wt.exe` is an app alias, which
        // only has metadata of its own.
        std::env::split_paths(&path).any(|dir| std::fs::symlink_metadata(dir.join(format!("{command}{}", std::env::consts::EXE_SUFFIX))).is_ok())
    };
    TERMINALS.into_iter().filter(|(_, line)| found(line)).collect()
}

/// The command line picked; empty: the system's.
pub fn load(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(FILE)).unwrap_or_default().trim().to_string()
}

/// Keeps `line`; empty: back to the system's.
pub fn save(dir: &Path, line: &str) -> std::io::Result<()> {
    let file = dir.join(FILE);
    match line.trim() {
        "" => std::fs::remove_file(file).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }),
        line => std::fs::write(file, line),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_choice_is_kept_and_forgotten() {
        let dir = std::env::temp_dir().join(format!("nightpost-terminal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        super::save(&dir, " foot --title x \n").unwrap();
        assert_eq!(super::load(&dir), "foot --title x");
        super::save(&dir, "").unwrap();
        assert_eq!(super::load(&dir), "");
        super::save(&dir, "").unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
