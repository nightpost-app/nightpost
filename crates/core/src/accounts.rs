//! Several accounts in one Nightpost: each is its own data folder, so its
//! own database, keychain secret, Tor service and daemon. The default
//! account is the root folder itself (nothing to migrate); the others live
//! in `accounts/<name>/`. `accounts.json` in the root lists them.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail, ensure};
use serde::{Deserialize, Serialize};

/// What the default account is called.
pub const DEFAULT: &str = "default";
const FILE: &str = "accounts.json";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Account {
    pub name: String,
    /// Started with the interface, so it notifies; others only when opened.
    #[serde(default = "yes")]
    pub active: bool,
}

fn yes() -> bool {
    true
}

/// The accounts, the default first. Missing file: just the default.
pub fn list(root: &Path) -> Result<Vec<Account>> {
    let mut all = match std::fs::read_to_string(root.join(FILE)) {
        Ok(text) => serde_json::from_str::<Vec<Account>>(&text).with_context(|| format!("reading {FILE}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    all.retain(|a| a.name != DEFAULT);
    all.insert(0, Account { name: DEFAULT.into(), active: true });
    Ok(all)
}

fn save(root: &Path, all: &[Account]) -> Result<()> {
    let others: Vec<&Account> = all.iter().filter(|a| a.name != DEFAULT).collect();
    let tmp = root.join(format!("{FILE}.new"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&others)?)?;
    std::fs::rename(&tmp, root.join(FILE))?;
    Ok(())
}

/// A name that is safe as a folder: letters, digits, `-` and `_`.
pub fn check_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name.len() <= 32 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "an account name is 1 to 32 letters, digits, - or _"
    );
    Ok(())
}

/// Where the account's data lives; it must exist.
pub fn dir_of(root: &Path, name: &str) -> Result<PathBuf> {
    if name == DEFAULT {
        return Ok(root.to_path_buf());
    }
    check_name(name)?;
    ensure!(list(root)?.iter().any(|a| a.name == name), "no account named {name}");
    crate::api::private_dir(&root.join("accounts").join(name))
}

/// Adds an account (an empty folder: the first start there makes it).
pub fn add(root: &Path, name: &str) -> Result<PathBuf> {
    check_name(name)?;
    let mut all = list(root)?;
    ensure!(!all.iter().any(|a| a.name.eq_ignore_ascii_case(name)), "an account named {name} exists");
    all.push(Account { name: name.into(), active: true });
    save(root, &all)?;
    crate::api::private_dir(&root.join("accounts").join(name))
}

/// Renames the label and the folder. The keychain entry is named after the
/// folder, so it moves too: the caller must not have the daemon running.
pub fn rename(root: &Path, name: &str, to: &str) -> Result<()> {
    ensure!(name != DEFAULT, "the default account cannot be renamed");
    check_name(to)?;
    let mut all = list(root)?;
    ensure!(!all.iter().any(|a| a.name.eq_ignore_ascii_case(to)), "an account named {to} exists");
    let from = dir_of(root, name)?;
    let account = all.iter_mut().find(|a| a.name == name).context("no such account")?;
    account.name = to.into();
    let target = root.join("accounts").join(to);
    let label = crate::dbkey::keychain_label(&from);
    let secret = crate::dbkey::keychain_secret(&label, false).ok().flatten();
    std::fs::rename(&from, &target)?;
    if let Some(secret) = secret {
        let target = std::fs::canonicalize(&target)?;
        crate::dbkey::store_keychain_secret(&crate::dbkey::keychain_label(&target), &secret).map_err(|e| anyhow::anyhow!("keychain: {e}"))?;
        crate::dbkey::delete_keychain_secret(&label).ok();
    }
    save(root, &all)
}

/// Marks an account as started with the interface or not.
pub fn set_active(root: &Path, name: &str, active: bool) -> Result<()> {
    let mut all = list(root)?;
    let account = all.iter_mut().find(|a| a.name == name).context("no such account")?;
    account.active = active;
    save(root, &all)
}

/// Deletes the account for good: its folder (database, keys) and keychain
/// entry. Never the default.
pub fn remove(root: &Path, name: &str) -> Result<()> {
    if name == DEFAULT {
        bail!("the default account cannot be removed");
    }
    let dir = dir_of(root, name)?;
    let label = crate::dbkey::keychain_label(&dir);
    let mut all = list(root)?;
    all.retain(|a| a.name != name);
    save(root, &all)?;
    std::fs::remove_dir_all(&dir)?;
    crate::dbkey::delete_keychain_secret(&label).ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_live_in_separate_folders() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        assert_eq!(list(&root).unwrap().len(), 1);
        assert_eq!(dir_of(&root, DEFAULT).unwrap(), root);
        let pro = add(&root, "pro").unwrap();
        assert_ne!(pro, root);
        assert!(pro.starts_with(root.join("accounts")));
        assert!(add(&root, "Pro").is_err(), "names differ by more than case");
        assert!(add(&root, "../x").is_err());
        assert!(dir_of(&root, "nobody").is_err());
        assert_ne!(crate::dbkey::keychain_label(&pro), crate::dbkey::keychain_label(&root));
        set_active(&root, "pro", false).unwrap();
        assert!(!list(&root).unwrap()[1].active);
        std::fs::write(pro.join("nightpost.db"), b"x").unwrap();
        rename(&root, "pro", "work").unwrap();
        assert!(root.join("accounts/work/nightpost.db").exists() && !pro.exists());
        assert!(rename(&root, DEFAULT, "x").is_err());
        remove(&root, "work").unwrap();
        assert!(!root.join("accounts/work").exists());
        assert_eq!(list(&root).unwrap().len(), 1);
        assert!(remove(&root, DEFAULT).is_err());
    }
}
