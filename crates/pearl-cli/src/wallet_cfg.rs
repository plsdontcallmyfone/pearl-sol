//! The wallet `cadre` uses when no --key / --address (or PEARL_KEY / PEARL_ADDRESS) is given:
//! ~/.cadre/cli.json, written by `cadre wallet use`. It holds a key FILE PATH or a watched
//! address, never key material.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Saved {
    pub key: Option<PathBuf>,
    pub address: Option<String>,
}

pub fn path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cadre/cli.json"))
}

pub fn load() -> Option<Saved> {
    let v: Value = serde_json::from_slice(&std::fs::read(path()?).ok()?).ok()?;
    let s = Saved { key: v["key"].as_str().map(PathBuf::from), address: v["address"].as_str().map(String::from) };
    (s.key.is_some() || s.address.is_some()).then_some(s)
}

pub fn save(s: &Saved) -> Result<PathBuf> {
    let p = path().context("HOME is not set")?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let v = json!({"key": s.key.as_ref().map(|k| k.display().to_string()), "address": s.address});
    std::fs::write(&p, serde_json::to_vec_pretty(&v)?)?;
    Ok(p)
}

pub fn forget() -> Result<bool> {
    match path() {
        Some(p) if p.exists() => {
            std::fs::remove_file(&p)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Checks before saving: the key file reads as a key, the address parses.
pub fn checked(key: Option<PathBuf>, address: Option<String>) -> Result<Saved> {
    match (key, address) {
        (Some(k), None) => {
            let k = std::fs::canonicalize(&k).with_context(|| format!("{}", k.display()))?;
            pearl_node::read_key_file(&k).with_context(|| format!("{} is not a key file", k.display()))?;
            Ok(Saved { key: Some(k), address: None })
        }
        (None, Some(a)) => {
            crate::fmt::parse_chain_addr(&a)?;
            Ok(Saved { key: None, address: Some(a.trim().to_string()) })
        }
        _ => bail!("give one of --key FILE (signs) or --address ADDRESS (watch only)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_refuses_both_or_neither() {
        assert!(checked(None, None).is_err());
        assert!(checked(Some("k".into()), Some("a".into())).is_err());
        assert!(checked(None, Some("not an address".into())).is_err());
    }
}
