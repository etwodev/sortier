//! Vaults: separate libraries, each with its own folders, tags, sequences, collections and covers. A local vault
//! keeps its library in Sortier's data folder; a remote one is a Sortier Remote server's library.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::library::data_dir;

/// Where a vault's library lives.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum VaultKind {
    Local,
    /// A Sortier Remote server at `address` (`host:port`), trusted by the SHA-256 `fingerprint` of its certificate
    /// (lower-case hex), logged into with `token`, keeping up to `cache_mb` megabytes of what was fetched from it.
    Remote {
        address: String,
        fingerprint: String,
        token: String,
        #[serde(default = "default_cache_mb")]
        cache_mb: u64,
    },
}

/// How much of a remote vault is cached unless the user says otherwise: 2 GB.
pub fn default_cache_mb() -> u64 {
    2048
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Vault {
    /// Stable and unique; names the vault's data folder.
    pub id: String,
    pub name: String,
    pub kind: VaultKind,
}

impl Vault {
    /// The folder this vault's own data (its library, and a remote vault's cache) is kept in.
    pub fn folder(&self) -> PathBuf {
        data_dir().join("vaults").join(&self.id)
    }

    pub fn is_remote(&self) -> bool {
        matches!(self.kind, VaultKind::Remote { .. })
    }
}

/// Every vault, and which one is open.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Vaults {
    pub vaults: Vec<Vault>,
    pub current: String,
}

impl Vaults {
    /// The vaults saved in `data_dir()/vaults.json`. The first time, the library from before vaults existed becomes
    /// the first vault, "Library".
    pub fn load() -> Self {
        let file = data_dir().join("vaults.json");
        if let Some(vaults) = std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice::<Vaults>(&bytes).ok())
            && !vaults.vaults.is_empty()
        {
            return vaults;
        }
        let first = Vault {
            id: "library".to_string(),
            name: "Library".to_string(),
            kind: VaultKind::Local,
        };
        let old = data_dir().join("library.json");
        let new = first.folder().join("library.json");
        if old.is_file() && !new.exists() {
            let moved = std::fs::create_dir_all(first.folder()).and_then(|_| std::fs::rename(&old, &new));
            if let Err(error) = moved {
                eprintln!("sortier: couldn't move the library into its vault: {error}");
            }
        }
        let vaults = Vaults {
            current: first.id.clone(),
            vaults: vec![first],
        };
        vaults.save();
        vaults
    }

    pub fn save(&self) {
        if let Err(error) = self.try_save() {
            eprintln!("sortier: failed to save the vaults: {error:#}");
        }
    }

    fn try_save(&self) -> Result<()> {
        let file = data_dir().join("vaults.json");
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?).context("writing the vaults")?;
        std::fs::rename(&tmp, &file).context("replacing the vaults")?;
        Ok(())
    }

    pub fn current(&self) -> &Vault {
        self.vaults.iter().find(|vault| vault.id == self.current).unwrap_or(&self.vaults[0])
    }

    pub fn get(&self, id: &str) -> Option<&Vault> {
        self.vaults.iter().find(|vault| vault.id == id)
    }

    /// Adds a vault named `name` of `kind`, returning its id.
    pub fn add(&mut self, name: &str, kind: VaultKind) -> String {
        let made = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let id = format!("v{made:x}");
        self.vaults.push(Vault {
            id: id.clone(),
            name: name.trim().to_string(),
            kind,
        });
        id
    }

    /// Sets how much of remote vault `id` is cached.
    pub fn set_cache_mb(&mut self, id: &str, megabytes: u64) {
        if let Some(Vault {
            kind: VaultKind::Remote { cache_mb, .. },
            ..
        }) = self.vaults.iter_mut().find(|vault| vault.id == id)
        {
            *cache_mb = megabytes.max(64);
        }
    }

    pub fn rename(&mut self, id: &str, name: &str) {
        if let Some(vault) = self.vaults.iter_mut().find(|vault| vault.id == id)
            && !name.trim().is_empty()
        {
            vault.name = name.trim().to_string();
        }
    }

    /// Forgets vault `id` and deletes Sortier's own data for it (its library and cache, never the files in its
    /// folders). The last vault can't be removed. Returns whether it was.
    pub fn remove(&mut self, id: &str) -> bool {
        if self.vaults.len() < 2 {
            return false;
        }
        let Some(at) = self.vaults.iter().position(|vault| vault.id == id) else { return false };
        let vault = self.vaults.remove(at);
        remove_folder(&vault.folder());
        if self.current == id {
            self.current = self.vaults[0].id.clone();
        }
        true
    }
}

fn remove_folder(folder: &Path) {
    if folder.starts_with(data_dir().join("vaults"))
        && let Err(error) = std::fs::remove_dir_all(folder)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("sortier: couldn't remove {}: {error}", folder.display());
    }
}
