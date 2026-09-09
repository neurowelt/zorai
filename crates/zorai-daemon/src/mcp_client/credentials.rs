//! Separate AES-256-GCM vault. Only references are persisted in ordinary configuration.
use super::{
    config::{credential_ref, credential_ref_mut},
    McpCredentialUpdate, McpServerConfig,
};
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub(super) struct CredentialStore {
    root: PathBuf,
}
impl CredentialStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            root: data_dir.join("mcp-credentials"),
        }
    }
    fn prepare(&self) -> Result<[u8; 32], String> {
        if self
            .root
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err("MCP credential directory must not be a symlink".into());
        }
        std::fs::create_dir_all(&self.root)
            .map_err(|_| "Cannot create MCP credential directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| "Cannot protect MCP credential directory")?;
        }
        let key_path = self.root.join("key");
        if key_path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err("MCP credential key must not be a symlink".into());
        }
        if !key_path
            .metadata()
            .is_ok_and(|m| m.is_file() && m.len() == 32)
        {
            match std::fs::remove_file(&key_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("Cannot repair incomplete MCP credential key".into()),
            }
            let key: [u8; 32] = rand::random();
            write_private(&key_path, &key)?;
        }
        let bytes = std::fs::read(key_path).map_err(|_| "Cannot read MCP credential key")?;
        bytes
            .try_into()
            .map_err(|_| "Invalid MCP credential key".into())
    }
    fn path(&self, reference: &str) -> Result<PathBuf, String> {
        uuid::Uuid::parse_str(reference).map_err(|_| "Invalid MCP credential reference")?;
        Ok(self.root.join(format!("{reference}.enc")))
    }
    pub fn load(&self, config: &McpServerConfig) -> Result<Option<String>, String> {
        let Some(reference) = credential_ref(&config.auth) else {
            return Ok(None);
        };
        let path = self.path(reference)?;
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err("MCP credential entry must not be a symlink".into());
        }
        let bytes = std::fs::read(path)
            .map_err(|_| "MCP credential is unavailable; replace it in Settings → MCP")?;
        if bytes.len() < 12 {
            return Err("Invalid encrypted MCP credential".into());
        }
        let key = self.prepare()?;
        let cipher = Aes256Gcm::new((&key).into());
        let clear = cipher
            .decrypt(aes_gcm::Nonce::from_slice(&bytes[..12]), &bytes[12..])
            .map_err(|_| "Cannot decrypt MCP credential")?;
        let value: serde_json::Value =
            serde_json::from_slice(&clear).map_err(|_| "Invalid MCP credential envelope")?;
        if value["server_id"].as_str() != Some(config.id.as_str())
            || value["endpoint"].as_str() != Some(config.url.as_str())
        {
            return Err("MCP credential belongs to another server or endpoint; replace it in Settings → MCP".into());
        }
        value["secret"]
            .as_str()
            .map(|s| Some(s.to_owned()))
            .ok_or_else(|| "Invalid MCP credential envelope".into())
    }
    pub fn edit(
        &self,
        config: &mut McpServerConfig,
        update: McpCredentialUpdate,
    ) -> Result<(), String> {
        match update {
            // Keeping a reference must allow disable/revocation even if the vault is broken.
            // Connection establishment reports unavailable or undecryptable credentials.
            McpCredentialUpdate::Keep => {}
            McpCredentialUpdate::Clear => {
                // Clear the reference first. Old entries are removed only after successful config persistence.
                if let Some(reference) = credential_ref_mut(&mut config.auth) {
                    *reference = None;
                }
            }
            McpCredentialUpdate::Replace(secret) => {
                validate_secret(&secret)?;
                if credential_ref_mut(&mut config.auth).is_none() {
                    return Err("Choose an authentication mode before setting a credential".into());
                }
                let key = self.prepare()?;
                let cipher = Aes256Gcm::new((&key).into());
                let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
                let envelope = serde_json::to_vec(&serde_json::json!({"server_id":config.id,"endpoint":config.url,"secret":secret})).map_err(|_| "Cannot encode MCP credential")?;
                let encrypted = cipher
                    .encrypt(&nonce, envelope.as_slice())
                    .map_err(|_| "Cannot encrypt MCP credential")?;
                let mut bytes = nonce.to_vec();
                bytes.extend(encrypted);
                let reference = uuid::Uuid::new_v4().to_string();
                write_private(&self.path(&reference)?, &bytes)?;
                *credential_ref_mut(&mut config.auth).expect("checked authentication") =
                    Some(reference);
            }
        }
        Ok(())
    }
    pub fn delete(&self, reference: &str) -> Result<(), String> {
        match std::fs::remove_file(self.path(reference)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("Cannot delete obsolete MCP credential".into()),
        }
    }
}
pub(super) fn validate_secret(secret: &str) -> Result<(), String> {
    if secret.trim().is_empty()
        || secret == "********"
        || secret == "••••••••"
        || secret.len() > 16384
        || reqwest::header::HeaderValue::from_str(secret).is_err()
    {
        return Err("Enter a valid credential; a masked placeholder cannot be saved".into());
    }
    Ok(())
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .map_err(|_| "Cannot create private MCP credential file")?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "Cannot save MCP credential".into())
}
