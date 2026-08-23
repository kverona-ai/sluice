//! One-time pairing payload carried by the QR code (05 §6.3): where to reach the
//! desktop (LAN addresses first, relay as fallback) plus a 32-byte single-use
//! code that expires. Scanning it is the only way a device can join — there is
//! no discovery of unpaired desktops and nothing listens for unauthenticated input.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;

use crate::crypto::{KEY_LEN, random_key};

pub const PAIRING_TTL_SECS: i64 = 10 * 60;

#[derive(Clone, Debug, PartialEq)]
pub struct PairingPayload {
    pub desktop_id: String,
    pub desktop_name: String,
    /// Desktop static X25519 public key — the device pins it at pairing time.
    pub desktop_dh_public: [u8; KEY_LEN],
    /// `host:port` candidates on the local network, tried in order.
    pub lan: Vec<String>,
    /// `host:port` of a relay that forwards encrypted frames, if configured.
    pub relay: Option<String>,
    pub code: [u8; KEY_LEN],
    /// Unix seconds after which the desktop refuses this code.
    pub expires_at: i64,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl PairingPayload {
    pub fn new(
        desktop_id: &str,
        desktop_name: &str,
        desktop_dh_public: [u8; KEY_LEN],
        lan: Vec<String>,
        relay: Option<String>,
    ) -> Self {
        Self {
            desktop_id: desktop_id.to_string(),
            desktop_name: desktop_name.to_string(),
            desktop_dh_public,
            lan,
            relay,
            code: random_key(),
            expires_at: now() + PAIRING_TTL_SECS,
        }
    }

    pub fn is_expired(&self) -> bool {
        now() > self.expires_at
    }

    pub fn seconds_left(&self) -> i64 {
        (self.expires_at - now()).max(0)
    }

    /// `sluice://pair?v=1&d=…&n=…&k=<b64url pk>&a=ip:port,ip:port&r=host:port&c=<b64url>&e=<unix>`
    pub fn encode(&self) -> String {
        let mut s = format!(
            "sluice://pair?v=1&d={}&n={}&k={}&a={}",
            pct(&self.desktop_id),
            pct(&self.desktop_name),
            B64.encode(self.desktop_dh_public),
            pct(&self.lan.join(","))
        );
        if let Some(r) = &self.relay {
            s.push_str("&r=");
            s.push_str(&pct(r));
        }
        s.push_str("&c=");
        s.push_str(&B64.encode(self.code));
        s.push_str(&format!("&e={}", self.expires_at));
        s
    }

    pub fn decode(text: &str) -> Result<Self> {
        let text = text.trim();
        let query = text
            .strip_prefix("sluice://pair?")
            .or_else(|| text.strip_prefix("SLUICE://PAIR?"))
            .context("not a Sluice pairing code (expected sluice://pair?…)")?;
        let mut desktop_id = None;
        let mut desktop_name = String::new();
        let mut lan = Vec::new();
        let mut relay = None;
        let mut code = None;
        let mut pk = None;
        let mut expires_at = 0;
        let mut version = 0;
        for kv in query.split('&') {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            let v = unpct(v);
            match k {
                "v" => version = v.parse().unwrap_or(0),
                "d" => desktop_id = Some(v),
                "n" => desktop_name = v,
                "a" => {
                    lan = v
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                }
                "r" => relay = Some(v).filter(|s| !s.is_empty()),
                "c" => code = Some(key32(&v).context("bad pairing code")?),
                "k" => pk = Some(key32(&v).context("bad desktop key")?),
                "e" => expires_at = v.parse().unwrap_or(0),
                _ => {}
            }
        }
        if version != 1 {
            bail!("unsupported pairing payload version {version}");
        }
        Ok(Self {
            desktop_id: desktop_id.context("pairing payload lacks desktop id")?,
            desktop_name,
            desktop_dh_public: pk.context("pairing payload lacks the desktop key")?,
            lan,
            relay,
            code: code.context("pairing payload lacks the code")?,
            expires_at,
        })
    }
}

fn key32(v: &str) -> Result<[u8; KEY_LEN]> {
    let bytes = B64.decode(v.as_bytes())?;
    if bytes.len() != KEY_LEN {
        bail!("wrong key length");
    }
    let mut k = [0u8; KEY_LEN];
    k.copy_from_slice(&bytes);
    Ok(k)
}

fn pct(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b':'
            | b','
            | b'['
            | b']' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn unpct(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// QR modules (true = dark) for a payload, plus the side length.
pub fn qr_matrix(text: &str) -> Result<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)
        .context("payload too long for a QR code")?;
    let w = code.width();
    let cells = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Ok((w, cells))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_roundtrips_through_text() {
        let p = PairingPayload::new(
            "d1a2b3",
            "Will 的 MacBook",
            [9u8; KEY_LEN],
            vec!["192.168.1.10:51234".into(), "[fe80::1]:51234".into()],
            Some("relay.example.com:7788".into()),
        );
        let text = p.encode();
        assert!(text.starts_with("sluice://pair?v=1&"));
        let q = PairingPayload::decode(&text).unwrap();
        assert_eq!(p, q);
        assert!(!q.is_expired());
        let (w, cells) = qr_matrix(&text).unwrap();
        assert_eq!(cells.len(), w * w);
    }

    #[test]
    fn rejects_garbage() {
        assert!(PairingPayload::decode("https://example.com").is_err());
        assert!(PairingPayload::decode("sluice://pair?v=1&d=x").is_err());
    }
}
