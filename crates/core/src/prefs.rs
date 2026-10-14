//! How notifications and the window behave, read by the window, the
//! terminal interface and the daemon (which notifies when neither is open).
//! Nothing about anyone, so a plain file in the data folder.

use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Prefs {
    pub notify: bool,
    pub sound: bool,
    /// Who wrote, or only that something came.
    pub show_name: bool,
    /// What they wrote too (with show_name).
    pub show_text: bool,
    /// The system may keep notifications in its history.
    pub keep_history: bool,
    /// Closing the window stops the daemon too.
    pub lock_on_close: bool,
    /// The window's list of people is folded away.
    pub side_hidden: bool,
}

impl Prefs {
    /// Named before the terminal read it too.
    const FILE: &str = "gui-settings";

    pub fn load(dir: &Path) -> Prefs {
        let text = std::fs::read_to_string(dir.join(Prefs::FILE)).unwrap_or_default();
        let off = |key: &str| text.lines().any(|l| l == format!("{key}=0"));
        let on = |key: &str| text.lines().any(|l| l == format!("{key}=1"));
        Prefs {
            notify: !off("notify"),
            sound: on("sound"),
            show_name: !off("show_name"),
            show_text: on("show_text"),
            keep_history: on("keep_history"),
            lock_on_close: on("lock_on_close"),
            side_hidden: on("side_hidden"),
        }
    }

    pub fn save(self, dir: &Path) -> Prefs {
        let text = format!(
            "notify={}\nsound={}\nshow_name={}\nshow_text={}\nkeep_history={}\nlock_on_close={}\nside_hidden={}\n",
            u8::from(self.notify),
            u8::from(self.sound),
            u8::from(self.show_name),
            u8::from(self.show_text),
            u8::from(self.keep_history),
            u8::from(self.lock_on_close),
            u8::from(self.side_hidden)
        );
        let _ = std::fs::write(dir.join(Prefs::FILE), text);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_come_back_as_saved_and_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let defaults = Prefs::load(dir.path());
        assert!(defaults.notify && defaults.show_name && !defaults.show_text && !defaults.sound);
        let changed = Prefs { notify: false, show_text: true, lock_on_close: true, ..defaults }.save(dir.path());
        assert_eq!(Prefs::load(dir.path()), changed);
    }
}
