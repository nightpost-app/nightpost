//! The words the interfaces show, in the system's language: English, French,
//! Spanish or German, English for anything else. The language chosen in the
//! settings (the `language` file of the data directory) comes first, and
//! `NIGHTPOST_LANG=fr` before it.
//!
//! The files in `locales/` are Fluent, kept to one line per message with
//! `{ $name }` placeables, so any Fluent tool can edit them. The daemon
//! speaks the same language: it runs as the same person on the same machine.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

const LOCALES: [(&str, &str); 4] = [
    ("en", include_str!("../locales/en.ftl")),
    ("fr", include_str!("../locales/fr.ftl")),
    ("es", include_str!("../locales/es.ftl")),
    ("de", include_str!("../locales/de.ftl")),
];

/// The language chosen by hand, next to the database.
const FILE: &str = "language";

type Table = HashMap<&'static str, &'static str>;

fn parse(text: &'static str) -> Table {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once(" = "))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect()
}

fn tables() -> &'static Vec<Table> {
    static TABLES: OnceLock<Vec<Table>> = OnceLock::new();
    TABLES.get_or_init(|| LOCALES.iter().map(|(_, t)| parse(t)).collect())
}

/// Where in [`LOCALES`] the language in use is.
fn current() -> &'static AtomicUsize {
    static CURRENT: OnceLock<AtomicUsize> = OnceLock::new();
    CURRENT.get_or_init(|| AtomicUsize::new(wanted()))
}

fn wanted() -> usize {
    // Tests check messages in English, whatever the machine speaks.
    if cfg!(test) {
        return 0;
    }
    let wanted = std::env::var("NIGHTPOST_LANG").ok().filter(|l| !l.is_empty()).or_else(chosen).or_else(sys_locale::get_locale).unwrap_or_default();
    let code = wanted.split(['-', '_', '.']).next().unwrap_or_default().trim().to_ascii_lowercase();
    LOCALES.iter().position(|(c, _)| *c == code).unwrap_or(0)
}

/// The language code in use.
pub fn language() -> &'static str {
    LOCALES[current().load(Ordering::Relaxed)].0
}

/// Each language's code and its name, in itself.
pub fn languages() -> impl Iterator<Item = (&'static str, &'static str)> {
    LOCALES.iter().zip(tables()).map(|((code, _), table)| (*code, table["language-name"]))
}

/// The language chosen in the settings; `None`: the system's.
pub fn chosen() -> Option<String> {
    let text = std::fs::read_to_string(crate::api::data_dir().ok()?.join(FILE)).ok()?;
    Some(text.trim().to_string()).filter(|c| !c.is_empty())
}

/// Chooses the language (`None`: the system's) and speaks it from now on.
pub fn choose(code: Option<&str>) -> anyhow::Result<()> {
    let file = crate::api::data_dir()?.join(FILE);
    match code {
        Some(code) => std::fs::write(file, code)?,
        None => std::fs::remove_file(file).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })?,
    }
    reload();
    Ok(())
}

/// Speaks the language chosen now, maybe by another process.
pub fn reload() {
    current().store(wanted(), Ordering::Relaxed);
}

/// The message `key`, its `{ $name }` placeables filled from `args`.
/// Missing in the language: the English one; missing in English: the key.
pub fn text(key: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let tables = tables();
    let found = tables[current().load(Ordering::Relaxed)].get(key).or_else(|| tables[0].get(key));
    fill(found.copied().unwrap_or(key), args)
}

fn fill(template: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{ ${name} }}"), &value.to_string());
    }
    out
}

/// "Monday", or "Mon" when `short`; `day` from 1 (Monday) to 7.
pub fn weekday(day: i8, short: bool) -> String {
    text(&format!("weekday-{}{day}", if short { "short-" } else { "" }), &[])
}

/// "October", or "Oct" when `short`; `month` from 1 to 12.
pub fn month(month: i8, short: bool) -> String {
    text(&format!("month-{}{month}", if short { "short-" } else { "" }), &[])
}

/// `t!("key")`, or `t!("key", name = value, …)` for its placeables.
#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::i18n::text($key, &[])
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::text($key, &[$((stringify!($name), &$value as &dyn std::fmt::Display)),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every language has every English message, with the same placeables,
    /// and nothing English does not have; every key used in the code exists.
    #[test]
    fn every_language_says_everything() {
        let placeables = |v: &str| {
            let mut p: Vec<String> = v.split("{ $").skip(1).filter_map(|s| s.split_once(" }")).map(|(n, _)| n.to_string()).collect();
            p.sort();
            p
        };
        let english = parse(LOCALES[0].1);
        assert!(english.len() > 50);
        for (code, text) in &LOCALES[1..] {
            let table = parse(text);
            for (key, value) in &english {
                let Some(theirs) = table.get(key) else { panic!("{code}: {key} is missing") };
                assert_eq!(placeables(theirs), placeables(value), "{code}: {key}");
            }
            for key in table.keys() {
                assert!(english.contains_key(key), "{code}: {key} is not in English");
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut sources = Vec::new();
        // The macOS app reads the same files, through t("key") in Swift.
        for dir in ["core/src", "core/src/transport", "cli/src", "gui/src", "../apps/macos/Sources/NightpostMac"] {
            for entry in std::fs::read_dir(root.join(dir)).unwrap() {
                let path = entry.unwrap().path();
                let call = match path.extension().and_then(|e| e.to_str()) {
                    Some("rs") => "t!(\"",
                    Some("swift") => "t(\"",
                    _ => continue,
                };
                sources.push((call, std::fs::read_to_string(path).unwrap()));
            }
        }
        let mut used = 0;
        for (call, source) in &sources {
            for (at, _) in source.match_indices(call) {
                // Not format!(" and the like.
                if source[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let key = source[at + call.len()..].split('"').next().unwrap();
                if key == "key" {
                    continue;
                }
                assert!(english.contains_key(key), "{key} is used but not in en.ftl");
                used += 1;
            }
        }
        assert!(used > 50);
    }

    #[test]
    fn placeables_are_filled() {
        let english = parse("greet = Hello { $name }, { $n }%\n# a comment = no");
        assert_eq!(english.get("greet"), Some(&"Hello { $name }, { $n }%"));
        assert_eq!(english.len(), 1);
        assert_eq!(fill(english["greet"], &[("name", &"Bob"), ("n", &40)]), "Hello Bob, 40%");
        assert_eq!(text("no-such-key", &[]), "no-such-key");
        assert_eq!(languages().map(|(_, name)| name).collect::<Vec<_>>(), ["English", "Français", "Español", "Deutsch"]);
    }
}
