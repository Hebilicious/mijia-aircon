//! Where the AC's address and token are kept.
//!
//! The miIO token gives full control of the AC to anyone on the LAN, so the
//! file is created with mode 600 in a mode 700 directory:
//! `$AIRCON_CONFIG`, else `$XDG_CONFIG_HOME/aircon/config.json`, else
//! `~/.config/aircon/config.json`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceConfig {
    pub host: String,
    pub token: String,
    pub did: String,
    pub model: String,
    pub name: String,
    pub mac: String,
}

pub fn path() -> PathBuf {
    if let Some(path) = std::env::var_os("AIRCON_CONFIG") {
        return PathBuf::from(path);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("aircon").join("config.json")
}

pub fn load() -> Result<DeviceConfig> {
    let path = path();
    let text = fs::read_to_string(&path)
        .with_context(|| format!("no AC configured in {}; run `aircon setup`", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
}

pub fn save(config: &DeviceConfig) -> Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    fs::write(&path, serde_json::to_string_pretty(config)? + "\n")?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}
