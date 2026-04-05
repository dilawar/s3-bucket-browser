/// Encrypted credential storage.
///
/// At first use a random 256-bit key is generated and written to
/// `<config_dir>/key` (mode 0o600 on Unix).  Credentials are serialised as
/// TOML, encrypted with AES-256-GCM, and stored as
/// `<config_dir>/credentials` (12-byte nonce || ciphertext).
use std::path::{Path, PathBuf};

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

const KEY_FILE: &str = "key";
const CREDS_FILE: &str = "credentials";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedCredentials {
    pub bucket: String,
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
}

pub struct CredentialStore {
    config_dir: PathBuf,
    key: Key<Aes256Gcm>,
}

impl CredentialStore {
    /// Open (or initialise) the credential store.
    /// Generates and persists an encryption key on first call.
    pub fn open() -> Result<Self> {
        let config_dir = project_config_dir()?;
        std::fs::create_dir_all(&config_dir)
            .with_context(|| format!("creating config dir {config_dir:?}"))?;

        let key = load_or_generate_key(&config_dir)?;
        debug!("Credential store opened at {config_dir:?}");
        Ok(Self { config_dir, key })
    }

    /// Load and decrypt saved credentials. Returns `None` if no credentials
    /// have been saved yet (or if the file is missing / corrupt).
    pub fn load(&self) -> Option<SavedCredentials> {
        let path = self.config_dir.join(CREDS_FILE);
        let encrypted = std::fs::read(&path).ok()?;
        if encrypted.len() < 12 {
            return None;
        }
        let (nonce_bytes, ciphertext) = encrypted.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);
        let cipher = Aes256Gcm::new(&self.key);
        let plaintext = cipher.decrypt(nonce, ciphertext).ok()?;
        let toml_str = String::from_utf8(plaintext).ok()?;
        toml::from_str(&toml_str).ok()
    }

    /// Encrypt and persist credentials to disk.
    pub fn save(&self, creds: &SavedCredentials) -> Result<()> {
        let plaintext = toml::to_string(creds).context("serialising credentials")?;
        let cipher = Aes256Gcm::new(&self.key);
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| anyhow::anyhow!("AES-GCM encryption failed"))?;

        let mut data = nonce.to_vec();
        data.extend_from_slice(&ciphertext);

        let path = self.config_dir.join(CREDS_FILE);
        std::fs::write(&path, &data).with_context(|| format!("writing credentials to {path:?}"))?;
        info!("Credentials saved to {path:?}");
        Ok(())
    }

    /// Remove saved credentials from disk.
    pub fn delete(&self) -> Result<()> {
        let path = self.config_dir.join(CREDS_FILE);
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("deleting {path:?}"))?;
            info!("Saved credentials deleted");
        }
        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn project_config_dir() -> Result<PathBuf> {
    directories::ProjectDirs::from("", "", "s3-explorer")
        .map(|d| d.config_dir().to_path_buf())
        .context("could not determine config directory")
}

fn load_or_generate_key(dir: &Path) -> Result<Key<Aes256Gcm>> {
    let path = dir.join(KEY_FILE);
    if path.exists() {
        let bytes = std::fs::read(&path).with_context(|| format!("reading key file {path:?}"))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("key file has wrong length (expected 32 bytes)"))?;
        Ok(*Key::<Aes256Gcm>::from_slice(&arr))
    } else {
        let key = Aes256Gcm::generate_key(OsRng);
        std::fs::write(&path, key.as_slice())
            .with_context(|| format!("writing new key to {path:?}"))?;
        set_private_permissions(&path)?;
        info!("Generated new encryption key at {path:?}");
        Ok(key)
    }
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting permissions on {path:?}"))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
