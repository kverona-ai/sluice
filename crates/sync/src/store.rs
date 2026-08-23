//! Persistent trust state on both ends of the channel.
//!
//! Desktop: `<config>/sync.json` — own identity, channel settings and the list of
//! paired devices (each with the long-term key derived at pairing time).
//! Device (phone / `sluice pair` stand-in): `<config>/devices/<desktop-id>.json`.
//!
//! Keys are written with mode 0600 on Unix; files are rewritten atomically.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};

use crate::crypto::{Identity, KEY_LEN, random_id};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub platform: String,
    /// base64 of the 32-byte device key (PSK for resume handshakes).
    pub key: String,
    /// Device static X25519 public key (base64), pinned at pairing.
    #[serde(default)]
    pub dh_public: String,
    /// Device Ed25519 verifying key (base64) — decisions must verify against it.
    #[serde(default)]
    pub sign_public: String,
    pub paired_at: i64,
    #[serde(default)]
    pub last_seen: i64,
    /// "lan" | "relay" — how it last connected.
    #[serde(default)]
    pub last_via: String,
}

impl PairedDevice {
    pub fn key_bytes(&self) -> Option<[u8; KEY_LEN]> {
        decode_key(&self.key)
    }
    pub fn dh_public_bytes(&self) -> Option<[u8; 32]> {
        decode_key(&self.dh_public)
    }
    pub fn sign_public_bytes(&self) -> Option<[u8; 32]> {
        decode_key(&self.sign_public)
    }
}

pub fn encode_key(k: &[u8; KEY_LEN]) -> String {
    B64.encode(k)
}

pub fn decode_key(s: &str) -> Option<[u8; KEY_LEN]> {
    let v = B64.decode(s.as_bytes()).ok()?;
    if v.len() != KEY_LEN {
        return None;
    }
    let mut k = [0u8; KEY_LEN];
    k.copy_from_slice(&v);
    Some(k)
}

/// Desktop-side state.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DesktopState {
    pub desktop_id: String,
    pub desktop_name: String,
    /// Identity seeds (base64): static X25519 + Ed25519.
    #[serde(default)]
    pub dh_seed: String,
    #[serde(default)]
    pub sign_seed: String,
    /// Sync channel on/off (default on: pairing is still required for anything to connect).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Fixed LAN port; 0 = ephemeral (advertised through the QR each time).
    #[serde(default)]
    pub port: u16,
    /// `host:port` of a relay, if the user configured one.
    #[serde(default)]
    pub relay: Option<String>,
    #[serde(default)]
    pub devices: Vec<PairedDevice>,
}

fn default_true() -> bool {
    true
}

impl DesktopState {
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("sync.json")
    }

    pub fn load_or_init(config_dir: &Path) -> Self {
        let p = Self::path(config_dir);
        if let Ok(text) = std::fs::read_to_string(&p)
            && let Ok(s) = serde_json::from_str::<Self>(&text)
        {
            return s;
        }
        let id = Identity::generate();
        let (dh, sign) = id.seeds();
        let s = Self {
            desktop_id: random_id("d"),
            desktop_name: host_name(),
            dh_seed: encode_key(&dh),
            sign_seed: encode_key(&sign),
            enabled: true,
            port: 0,
            relay: None,
            devices: Vec::new(),
        };
        let _ = s.save(config_dir);
        s
    }

    pub fn save(&self, config_dir: &Path) -> Result<()> {
        write_private(&Self::path(config_dir), &serde_json::to_vec_pretty(self)?)
    }

    /// Own identity; (re)generated when the seeds are missing or corrupt.
    pub fn identity(&mut self) -> Identity {
        match (decode_key(&self.dh_seed), decode_key(&self.sign_seed)) {
            (Some(dh), Some(sign)) => Identity::from_seeds(dh, sign),
            _ => {
                let id = Identity::generate();
                let (dh, sign) = id.seeds();
                self.dh_seed = encode_key(&dh);
                self.sign_seed = encode_key(&sign);
                id
            }
        }
    }

    pub fn device(&self, id: &str) -> Option<&PairedDevice> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn upsert_device(&mut self, d: PairedDevice) {
        if let Some(slot) = self.devices.iter_mut().find(|x| x.id == d.id) {
            *slot = d;
        } else {
            self.devices.push(d);
        }
    }

    pub fn revoke(&mut self, id: &str) -> bool {
        let before = self.devices.len();
        self.devices.retain(|d| d.id != id);
        before != self.devices.len()
    }
}

/// A desktop as remembered by a device after pairing.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PairedDesktop {
    pub desktop_id: String,
    pub desktop_name: String,
    /// Device key (PSK) derived at pairing, base64.
    pub key: String,
    /// Desktop static X25519 public key (base64), pinned from the QR.
    #[serde(default)]
    pub dh_public: String,
    #[serde(default)]
    pub lan: Vec<String>,
    #[serde(default)]
    pub relay: Option<String>,
    pub paired_at: i64,
}

impl PairedDesktop {
    pub fn key_bytes(&self) -> Option<[u8; KEY_LEN]> {
        decode_key(&self.key)
    }
    pub fn dh_public_bytes(&self) -> Option<[u8; 32]> {
        decode_key(&self.dh_public)
    }
}

/// Device-side state (the phone, or the CLI stand-in).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeviceState {
    pub device_id: String,
    pub device_name: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub dh_seed: String,
    #[serde(default)]
    pub sign_seed: String,
    #[serde(default)]
    pub desktops: Vec<PairedDesktop>,
}

impl DeviceState {
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("device.json")
    }

    pub fn load_or_init(config_dir: &Path, platform: &str) -> Self {
        let p = Self::path(config_dir);
        if let Ok(text) = std::fs::read_to_string(&p)
            && let Ok(s) = serde_json::from_str::<Self>(&text)
        {
            return s;
        }
        let id = Identity::generate();
        let (dh, sign) = id.seeds();
        let s = Self {
            device_id: random_id("m"),
            device_name: host_name(),
            platform: platform.to_string(),
            dh_seed: encode_key(&dh),
            sign_seed: encode_key(&sign),
            desktops: Vec::new(),
        };
        let _ = s.save(config_dir);
        s
    }

    pub fn save(&self, config_dir: &Path) -> Result<()> {
        write_private(&Self::path(config_dir), &serde_json::to_vec_pretty(self)?)
    }

    pub fn identity(&mut self) -> Identity {
        match (decode_key(&self.dh_seed), decode_key(&self.sign_seed)) {
            (Some(dh), Some(sign)) => Identity::from_seeds(dh, sign),
            _ => {
                let id = Identity::generate();
                let (dh, sign) = id.seeds();
                self.dh_seed = encode_key(&dh);
                self.sign_seed = encode_key(&sign);
                id
            }
        }
    }

    pub fn upsert_desktop(&mut self, d: PairedDesktop) {
        if let Some(slot) = self.desktops.iter_mut().find(|x| x.desktop_id == d.desktop_id) {
            *slot = d;
        } else {
            self.desktops.push(d);
        }
    }

    pub fn forget(&mut self, desktop_id: &str) -> bool {
        let before = self.desktops.len();
        self.desktops.retain(|d| d.desktop_id != desktop_id);
        before != self.desktops.len()
    }
}

pub fn host_name() -> String {
    std::env::var("SLUICE_DEVICE_NAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| {
            std::process::Command::new("scutil")
                .args(["--get", "ComputerName"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "Sluice".to_string())
}

/// Append a decision to the repo's audit log (05 §7.1: `<common-dir>/sluice/audit.log`,
/// append-only JSON lines) — the trail of 放行 / 驳回 and which device it came from.
pub fn append_audit(common_dir: &Path, rec: &crate::proto::DecisionRecord) -> Result<PathBuf> {
    use std::io::Write as _;
    let dir = common_dir.join("sluice");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("audit.log");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    f.write_all(serde_json::to_string(rec)?.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(path)
}

pub fn recent_audit(common_dir: &Path, limit: usize) -> Vec<crate::proto::DecisionRecord> {
    let Ok(text) = std::fs::read_to_string(common_dir.join("sluice").join("audit.log")) else {
        return Vec::new();
    };
    let mut v: Vec<_> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    v.reverse();
    v.truncate(limit);
    v
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_state_persists_devices() {
        let dir = std::env::temp_dir().join(format!("sluice-sync-test-{}", random_id("")));
        let mut s = DesktopState::load_or_init(&dir);
        assert!(s.devices.is_empty());
        s.upsert_device(PairedDevice {
            id: "m1".into(),
            name: "phone".into(),
            platform: "ios".into(),
            key: encode_key(&[7u8; KEY_LEN]),
            dh_public: String::new(),
            sign_public: String::new(),
            paired_at: 1,
            last_seen: 0,
            last_via: String::new(),
        });
        s.save(&dir).unwrap();
        let again = DesktopState::load_or_init(&dir);
        assert_eq!(again, s);
        assert_eq!(again.device("m1").unwrap().key_bytes().unwrap(), [7u8; KEY_LEN]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
