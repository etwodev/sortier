//! What a server keeps in its data folder besides the library: its settings, its certificate, its password, and
//! the devices logged in to it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// `config.toml`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    /// The address to listen on: `0.0.0.0` for every network, `127.0.0.1` for this computer only.
    #[serde(default = "default_bind")]
    pub bind: String,
    /// Where uploads go; the first library folder when unset.
    #[serde(default)]
    pub upload_folder: Option<PathBuf>,
    /// How often the folders are looked through again for files that came or went.
    #[serde(default = "default_rescan")]
    pub rescan_minutes: u64,
    /// The password's salt and Argon2id hash, as hex, separated by `$`.
    #[serde(default)]
    pub password: Option<String>,
}

fn default_port() -> u16 {
    sortier_client::api::DEFAULT_PORT
}

fn default_bind() -> String {
    "0.0.0.0".to_string()
}

fn default_rescan() -> u64 {
    10
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: default_port(),
            bind: default_bind(),
            upload_folder: None,
            rescan_minutes: default_rescan(),
            password: None,
        }
    }
}

/// The server's data folder: the one given, else `SORTIER_REMOTE_DIR`, else the system's (such as
/// `%LOCALAPPDATA%\sortier-remote` on Windows).
pub fn data_folder(given: Option<PathBuf>) -> PathBuf {
    given
        .or_else(|| std::env::var_os("SORTIER_REMOTE_DIR").filter(|dir| !dir.is_empty()).map(PathBuf::from))
        .unwrap_or_else(|| dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("sortier-remote"))
}

impl Config {
    pub fn load(data: &Path) -> Result<Self> {
        match std::fs::read_to_string(data.join("config.toml")) {
            Ok(text) => toml::from_str(&text).context("config.toml can't be read"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).context("config.toml can't be read"),
        }
    }

    pub fn save(&self, data: &Path) -> Result<()> {
        std::fs::create_dir_all(data)?;
        std::fs::write(data.join("config.toml"), toml::to_string_pretty(self)?).context("config.toml can't be written")
    }

    pub fn set_password(&mut self, password: &str) -> Result<()> {
        if password.chars().count() < 8 {
            bail!("use a password of at least 8 characters");
        }
        let salt = random_bytes::<16>()?;
        self.password = Some(format!("{}${}", hex(&salt), hex(&hash_password(password, &salt)?)));
        Ok(())
    }

    /// Whether `password` is the one set.
    pub fn password_matches(&self, password: &str) -> bool {
        let Some((salt, hash)) = self.password.as_deref().and_then(|stored| stored.split_once('$')) else {
            return false;
        };
        let Some(salt) = unhex(salt) else { return false };
        hash_password(password, &salt).is_ok_and(|computed| hex(&computed) == hash)
    }
}

fn hash_password(password: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let mut hash = [0_u8; 32];
    argon2::Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut hash)
        .map_err(|error| anyhow::anyhow!("the password can't be hashed: {error}"))?;
    Ok(hash)
}

pub fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("no randomness: {error}"))?;
    Ok(bytes)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok()).collect()
}

/// The server's certificate and key (DER), made the first time it's needed.
pub struct Identity {
    pub certificate: Vec<u8>,
    pub key: Vec<u8>,
}

impl Identity {
    pub fn load_or_make(data: &Path) -> Result<Self> {
        let (certificate, key) = (data.join("certificate.der"), data.join("key.der"));
        if certificate.is_file() && key.is_file() {
            return Ok(Self {
                certificate: std::fs::read(certificate)?,
                key: std::fs::read(key)?,
            });
        }
        let made = rcgen::generate_simple_self_signed(vec!["sortier-remote".to_string(), "localhost".to_string()])?;
        let identity = Self {
            certificate: made.cert.der().to_vec(),
            key: made.signing_key.serialize_der(),
        };
        std::fs::create_dir_all(data)?;
        std::fs::write(&certificate, &identity.certificate)?;
        std::fs::write(&key, &identity.key)?;
        restrict(&key);
        Ok(identity)
    }

    /// What clients are shown to compare before trusting the server.
    pub fn fingerprint(&self) -> String {
        sortier_client::fingerprint_of(&self.certificate)
    }
}

/// Lets only this user read the file at `path`, where the system has such permissions.
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    let _ = path;
}

/// The devices logged in: each token's SHA-256 (tokens themselves aren't kept) and the device's name.
#[derive(Serialize, Deserialize, Default)]
pub struct Devices {
    pub tokens: BTreeMap<String, String>,
}

impl Devices {
    pub fn load(data: &Path) -> Self {
        std::fs::read(data.join("devices.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data: &Path) -> Result<()> {
        let path = data.join("devices.json");
        std::fs::write(&path, serde_json::to_vec_pretty(self)?)?;
        restrict(&path);
        Ok(())
    }

    /// Logs in `device`, returning its new token.
    pub fn add(&mut self, device: &str) -> Result<String> {
        let token = hex(&random_bytes::<32>()?);
        self.tokens.insert(digest(&token), device.to_string());
        Ok(token)
    }

    pub fn knows(&self, token: &str) -> bool {
        !token.is_empty() && self.tokens.contains_key(&digest(token))
    }
}

fn digest(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_checked_against_their_hash() {
        let mut config = Config::default();
        assert!(config.set_password("short").is_err());
        config.set_password("correct horse").unwrap();
        assert!(config.password_matches("correct horse"));
        assert!(!config.password_matches("correct house"));
        assert!(!config.password.as_deref().unwrap().contains("correct"));
        let mut devices = Devices::default();
        let token = devices.add("laptop").unwrap();
        assert!(devices.knows(&token) && !devices.knows("") && !devices.knows("nope"));
    }
}
