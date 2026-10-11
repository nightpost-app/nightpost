//! The words the interfaces show, in the system's language: English, French,
//! Spanish or German, English for anything else. `NIGHTPOST_LANG=fr` picks
//! one by hand.
//!
//! The files in `locales/` are Fluent, kept to one line per message with
//! `{ $name }` placeables, so any Fluent tool can edit them. The daemon
//! speaks the same language: it runs as the same person on the same machine.

use std::collections::HashMap;
use std::sync::OnceLock;

const LOCALES: [(&str, &str); 4] = [
    ("en", include_str!("../locales/en.ftl")),
    ("fr", include_str!("../locales/fr.ftl")),
    ("es", include_str!("../locales/es.ftl")),
    ("de", include_str!("../locales/de.ftl")),
];

type Table = HashMap<&'static str, &'static str>;

fn parse(text: &'static str) -> Table {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once(" = "))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect()
}

/// The language code to use, from `NIGHTPOST_LANG` or the system.
pub fn language() -> &'static str {
    static LANG: OnceLock<&'static str> = OnceLock::new();
    LANG.get_or_init(|| {
        // Tests check messages in English, whatever the machine speaks.
        if cfg!(test) {
            return "en";
        }
        let wanted = std::env::var("NIGHTPOST_LANG").ok().or_else(sys_locale::get_locale).unwrap_or_default();
        let code = wanted.split(['-', '_', '.']).next().unwrap_or_default().to_ascii_lowercase();
        LOCALES.iter().map(|(c, _)| *c).find(|c| *c == code).unwrap_or("en")
    })
}

fn tables() -> &'static (Table, Table) {
    static TABLES: OnceLock<(Table, Table)> = OnceLock::new();
    TABLES.get_or_init(|| {
        let chosen = LOCALES.iter().find(|(c, _)| *c == language()).map_or("", |(_, t)| t);
        (parse(chosen), parse(LOCALES[0].1))
    })
}

/// The message `key`, its `{ $name }` placeables filled from `args`.
/// Missing in the language: the English one; missing in English: the key.
pub fn text(key: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let (chosen, english) = tables();
    fill(chosen.get(key).or_else(|| english.get(key)).copied().unwrap_or(key), args)
}

fn fill(template: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{ ${name} }}"), &value.to_string());
    }
    out
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
        for dir in ["core/src", "core/src/transport", "cli/src", "gui/src"] {
            for entry in std::fs::read_dir(root.join(dir)).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_some_and(|e| e == "rs") {
                    sources.push(std::fs::read_to_string(path).unwrap());
                }
            }
        }
        let mut used = 0;
        for source in &sources {
            for (at, _) in source.match_indices("t!(\"") {
                // Not format!(" and the like.
                if source[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let key = source[at + 4..].split('"').next().unwrap();
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
    }
}
