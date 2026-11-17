//! Who may call the API. Every credential is a bearer token with a name and
//! a role:
//!
//! - the **main** token in `token`: what this machine's own clients (the
//!   window, `todai attach`) read from the state dir; an operator token;
//! - **device** tokens in `tokens.json`, one per paired phone or remote
//!   window, so one can be revoked without cutting off the others;
//! - and, with a different role, **agent** tokens for agents on other
//!   machines (see `docs/server.md`), which can post agent events but never
//!   answer for the operator.
//!
//! Both files are re-read on each check (they are tiny), so `rotate`,
//! `device add` and `device revoke` reach a running server at once.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::registry::{load_token, random_token, write_atomic};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Reads boards and posts operator actions.
    Operator,
    /// Reads boards and posts agent events.
    Agent,
}

/// The caller behind a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principal {
    /// `main`, or the device's name.
    pub name: String,
    pub role: Role,
}

/// A named token in `tokens.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Device {
    pub name: String,
    pub token: String,
    pub role: Role,
    /// Unix ms.
    pub created: u64,
}

/// The name the main token goes by; devices can't take it.
pub const MAIN: &str = "main";

pub struct Tokens {
    dir: PathBuf,
}

impl Tokens {
    /// Creates the main token if there is none yet.
    pub fn load(dir: &Path) -> Result<Tokens> {
        load_token(dir)?;
        let t = Tokens { dir: dir.to_owned() };
        t.devices()?; // fail early on a corrupt file
        Ok(t)
    }

    pub fn main(&self) -> Result<String> {
        load_token(&self.dir)
    }

    fn file(&self) -> PathBuf {
        self.dir.join("tokens.json")
    }

    pub fn devices(&self) -> Result<Vec<Device>> {
        match std::fs::read_to_string(self.file()) {
            Ok(s) => serde_json::from_str(&s).with_context(|| format!("parse {}", self.file().display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e).with_context(|| format!("read {}", self.file().display())),
        }
    }

    fn save(&self, devices: &[Device]) -> Result<()> {
        write_atomic(&self.file(), serde_json::to_string_pretty(devices)?.as_bytes(), 0o600)
    }

    /// Who `given` belongs to. Compares against every token in constant
    /// time, so timing doesn't tell how much of a guess matched (or which).
    /// Unreadable files mean no one: a request never gets in on a broken file.
    pub fn identify(&self, given: &str) -> Option<Principal> {
        let given = given.trim().as_bytes();
        let mut found = None;
        if let Ok(main) = self.main()
            && same(given, main.as_bytes())
        {
            found = Some(Principal { name: MAIN.into(), role: Role::Operator });
        }
        for d in self.devices().unwrap_or_default() {
            if same(given, d.token.as_bytes()) && found.is_none() {
                found = Some(Principal { name: d.name, role: d.role });
            }
        }
        found
    }

    /// A new token for `name`; fails if the name is taken.
    pub fn add(&self, name: &str, role: Role) -> Result<String> {
        let name = name.trim();
        if name.is_empty() || name == MAIN || name.chars().any(char::is_control) {
            bail!("bad device name {name:?}");
        }
        let mut devices = self.devices()?;
        if devices.iter().any(|d| d.name == name) {
            bail!("a device named {name} exists; revoke it first or pick another name");
        }
        let token = random_token()?;
        devices.push(Device { name: name.into(), token: token.clone(), role, created: todai_core::store::now_ms() });
        self.save(&devices)?;
        Ok(token)
    }

    /// The token of `name` if it exists with `role`, else a new one.
    pub fn get_or_add(&self, name: &str, role: Role) -> Result<String> {
        match self.devices()?.into_iter().find(|d| d.name == name.trim()) {
            Some(d) if d.role == role => Ok(d.token),
            Some(_) => bail!("{name} is a token with another role"),
            None => self.add(name, role),
        }
    }

    /// Whether `name` existed.
    pub fn revoke(&self, name: &str) -> Result<bool> {
        let mut devices = self.devices()?;
        let before = devices.len();
        devices.retain(|d| d.name != name);
        if devices.len() == before {
            return Ok(false);
        }
        self.save(&devices)?;
        Ok(true)
    }
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_get_their_own_revocable_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let t = Tokens::load(dir.path()).unwrap();
        let main = t.main().unwrap();
        let phone = t.add("phone", Role::Operator).unwrap();
        let bot = t.add("ci", Role::Agent).unwrap();
        assert_ne!(main, phone);
        assert_eq!(t.identify(&main).unwrap().name, MAIN);
        assert_eq!(t.identify(&phone), Some(Principal { name: "phone".into(), role: Role::Operator }));
        assert_eq!(t.identify(&bot).unwrap().role, Role::Agent);
        assert_eq!(t.identify("nope"), None);
        assert!(t.add("phone", Role::Operator).is_err(), "names are unique");
        assert!(t.add("main", Role::Operator).is_err());
        assert_eq!(t.get_or_add("phone", Role::Operator).unwrap(), phone);

        assert!(t.revoke("phone").unwrap());
        assert_eq!(t.identify(&phone), None);
        assert!(t.identify(&main).is_some(), "the others keep working");
        assert!(!t.revoke("phone").unwrap());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.path().join("tokens.json")).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
