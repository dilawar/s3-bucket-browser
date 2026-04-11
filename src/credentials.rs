use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedCredentials {
    pub bucket: String,
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
}

// ── Native implementation ─────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
pub use native::CredentialStore;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::{Path, PathBuf};

    use aes_gcm::{
        Aes256Gcm, Key, Nonce,
        aead::{Aead, AeadCore, KeyInit, OsRng},
    };
    use anyhow::{Context, Result};
    use tracing::{debug, info};

    use super::SavedCredentials;

    const KEY_FILE: &str = "key";
    const CREDS_FILE: &str = "credentials";

    pub struct CredentialStore {
        config_dir: PathBuf,
        key: Key<Aes256Gcm>,
    }

    impl CredentialStore {
        pub fn open() -> Result<Self> {
            let config_dir = project_config_dir()?;
            std::fs::create_dir_all(&config_dir)
                .with_context(|| format!("creating config dir {config_dir:?}"))?;
            let key = load_or_generate_key(&config_dir)?;
            debug!("Credential store opened at {config_dir:?}");
            Ok(Self { config_dir, key })
        }

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
            std::fs::write(&path, &data)
                .with_context(|| format!("writing credentials to {path:?}"))?;
            info!("Credentials saved to {path:?}");
            Ok(())
        }

        pub fn delete(&self) -> Result<()> {
            let path = self.config_dir.join(CREDS_FILE);
            if path.exists() {
                std::fs::remove_file(&path)
                    .with_context(|| format!("deleting {path:?}"))?;
                info!("Saved credentials deleted");
            }
            Ok(())
        }
    }

    fn project_config_dir() -> Result<PathBuf> {
        directories::ProjectDirs::from("", "", "s3-explorer")
            .map(|d| d.config_dir().to_path_buf())
            .context("could not determine config directory")
    }

    fn load_or_generate_key(dir: &Path) -> Result<Key<Aes256Gcm>> {
        let path = dir.join(KEY_FILE);
        if path.exists() {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("reading key file {path:?}"))?;
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
}

// ── WASM implementation (localStorage) ───────────────────────────────────────

#[cfg(target_arch = "wasm32")]
pub use wasm::CredentialStore;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use aes_gcm::{
        Aes256Gcm, Key, Nonce,
        aead::{Aead, AeadCore, KeyInit, OsRng},
    };
    use anyhow::{Result, anyhow};
    use base64::{Engine as _, engine::general_purpose::STANDARD as B64};

    use super::SavedCredentials;

    const LS_KEY:   &str = "s3_explorer_key";
    const LS_CREDS: &str = "s3_explorer_creds";

    pub struct CredentialStore {
        key: Key<Aes256Gcm>,
    }

    impl CredentialStore {
        pub fn open() -> Result<Self> {
            let key = load_or_generate_key()?;
            Ok(Self { key })
        }

        pub fn load(&self) -> Option<SavedCredentials> {
            let ls = local_storage().ok()?;
            let b64 = ls.get_item(LS_CREDS).ok()??;
            let encrypted = B64.decode(&b64).ok()?;
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

        pub fn save(&self, creds: &SavedCredentials) -> Result<()> {
            let plaintext = toml::to_string(creds)
                .map_err(|e| anyhow!("serialising credentials: {e}"))?;
            let cipher = Aes256Gcm::new(&self.key);
            let nonce = Aes256Gcm::generate_nonce(OsRng);
            let ciphertext = cipher
                .encrypt(&nonce, plaintext.as_bytes())
                .map_err(|_| anyhow!("AES-GCM encryption failed"))?;
            let mut data = nonce.to_vec();
            data.extend_from_slice(&ciphertext);
            let b64 = B64.encode(&data);
            local_storage()?
                .set_item(LS_CREDS, &b64)
                .map_err(|_| anyhow!("localStorage.setItem failed"))?;
            Ok(())
        }

        pub fn delete(&self) -> Result<()> {
            local_storage()?
                .remove_item(LS_CREDS)
                .map_err(|_| anyhow!("localStorage.removeItem failed"))?;
            Ok(())
        }
    }

    fn local_storage() -> Result<web_sys::Storage> {
        web_sys::window()
            .ok_or_else(|| anyhow!("no window"))?
            .local_storage()
            .map_err(|_| anyhow!("localStorage unavailable"))?
            .ok_or_else(|| anyhow!("localStorage is null"))
    }

    fn load_or_generate_key() -> Result<Key<Aes256Gcm>> {
        let ls = local_storage()?;
        if let Some(hex_str) = ls
            .get_item(LS_KEY)
            .map_err(|_| anyhow!("localStorage.getItem failed"))?
        {
            let bytes = hex::decode(&hex_str)
                .map_err(|_| anyhow!("corrupt key in localStorage"))?;
            let arr: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow!("key has wrong length"))?;
            Ok(*Key::<Aes256Gcm>::from_slice(&arr))
        } else {
            let key = Aes256Gcm::generate_key(OsRng);
            let hex_str = hex::encode(key.as_slice());
            ls.set_item(LS_KEY, &hex_str)
                .map_err(|_| anyhow!("localStorage.setItem failed"))?;
            Ok(key)
        }
    }
}

