//! Handshake, frame encryption and decision signatures for the sync channel
//! (02 §5.7: "X25519 / Noise 握手 · 双方持久化对端公钥 · 放行指令由手机私钥签名 ·
//! 中继只见密文").
//!
//! Roles: the **initiator** is the device (phone, or `sluice pair` as a stand-in),
//! the **responder** is the desktop. Pattern is Noise-IK-like with a PSK:
//!
//! * The QR carries the desktop's static X25519 public key `S_d` and a one-time
//!   32-byte code (the PSK of the first handshake). Afterwards the PSK is the
//!   device key both sides derived at pairing — the code itself is never reused.
//! * Frame 0 (device → desktop): clear header (`{v, mode, device_id, desktop_id}`)
//!   ‖ `e_i` ‖ AEAD(k0, Hello), where `k0 = HKDF(salt=psk, ikm=DH(e_i, S_d))`.
//!   Only the holder of the desktop's static key *and* the PSK can read it; Hello
//!   carries the device's static X25519 key `S_i` and its Ed25519 signing key.
//! * Frame 1 (desktop → device): `e_r` ‖ AEAD(k1, Welcome), with
//!   `master = HKDF(salt=psk, ikm = DH(e_i,S_d) ‖ DH(e_i,e_r) ‖ DH(S_i,e_r))`.
//!   `DH(S_i, e_r)` proves the device owns `S_i`; the desktop owning `S_d` is
//!   proven by decrypting frame 0. On resume, the desktop additionally checks that
//!   `S_i` equals the key persisted at pairing time.
//! * Transport keys, the session id and (on pairing) the long-term device key are
//!   HKDF-Expand'ed from `master` → forward secrecy; keys never travel.
//! * Each data frame: `counter(8) ‖ ChaCha20-Poly1305(k_dir, nonce=counter, ad=counter)`;
//!   counters are strictly increasing per direction, so replays are rejected.
//! * Decisions (放行 / 驳回) are Ed25519-signed by the device over a canonical
//!   string that binds desktop id, session id, item id and item version; the
//!   desktop verifies before executing (a compromised relay cannot forge them).

use anyhow::{Context as _, Result, bail};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

pub const KEY_LEN: usize = 32;
/// Largest frame accepted from the wire (the review queue carries diffs, not repos).
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

/// Clear-text prefix of the initiator's first frame — lets the responder pick the
/// right PSK (a pending pairing code, or the key of an already-trusted device) and
/// refuse hellos meant for another desktop before doing any crypto.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HelloHeader {
    pub v: u32,
    pub mode: HelloMode,
    pub device_id: String,
    pub desktop_id: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HelloMode {
    Pair,
    Resume,
}

/// Long-term identity of one party: X25519 static key (handshake) + Ed25519 key
/// (signatures). Persisted as two 32-byte seeds.
#[derive(Clone)]
pub struct Identity {
    dh: StaticSecret,
    sign: SigningKey,
}

impl Identity {
    pub fn generate() -> Self {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        OsRng.fill_bytes(&mut a);
        OsRng.fill_bytes(&mut b);
        Self::from_seeds(a, b)
    }
    pub fn from_seeds(dh: [u8; 32], sign: [u8; 32]) -> Self {
        Self {
            dh: StaticSecret::from(dh),
            sign: SigningKey::from_bytes(&sign),
        }
    }
    pub fn seeds(&self) -> ([u8; 32], [u8; 32]) {
        (self.dh.to_bytes(), self.sign.to_bytes())
    }
    pub fn dh_public(&self) -> [u8; 32] {
        PublicKey::from(&self.dh).to_bytes()
    }
    pub fn sign_public(&self) -> [u8; 32] {
        self.sign.verifying_key().to_bytes()
    }
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.sign.sign(msg).to_bytes()
    }
}

pub fn verify_signature(sign_public: &[u8; 32], msg: &[u8], sig: &[u8]) -> bool {
    let Ok(vk) = VerifyingKey::from_bytes(sign_public) else {
        return false;
    };
    let Ok(sig) = Signature::from_slice(sig) else {
        return false;
    };
    vk.verify(msg, &sig).is_ok()
}

/// Peer public keys learned during the handshake (persist them — 02 §5.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerKeys {
    pub dh_public: [u8; 32],
    pub sign_public: [u8; 32],
}

#[derive(Clone, Debug, Default)]
pub struct SessionSecrets {
    pub session_id: String,
    /// Long-term PSK for the next handshakes — only adopted after a `Pair` run.
    pub device_key: [u8; KEY_LEN],
}

/// Sending half of a session (one per direction, so reader and writer threads can
/// own their halves independently).
pub struct Sealer {
    key: ChaCha20Poly1305,
    ctr: u64,
}

/// Receiving half of a session.
pub struct Opener {
    key: ChaCha20Poly1305,
    ctr: u64,
}

fn derive(master: &Hkdf<Sha256>, info: &[u8]) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    master
        .expand(info, &mut out)
        .expect("32 bytes is a valid HKDF length");
    out
}

fn aead(key: &[u8; KEY_LEN]) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(Key::from_slice(key))
}

fn nonce_for(counter: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_be_bytes());
    Nonce::from(n)
}

pub fn random_key() -> [u8; KEY_LEN] {
    let mut k = [0u8; KEY_LEN];
    OsRng.fill_bytes(&mut k);
    k
}

pub fn random_id(prefix: &str) -> String {
    let mut b = [0u8; 8];
    OsRng.fill_bytes(&mut b);
    format!("{prefix}{}", hex(&b))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Device-side state between frame 0 and frame 1.
pub struct Initiator {
    // `StaticSecret` (not `EphemeralSecret`) because the same ephemeral key is used
    // for two DH operations; it is still generated per handshake and zeroized on drop.
    e: StaticSecret,
    e_pub: PublicKey,
    es: [u8; 32],
    psk: [u8; KEY_LEN],
    identity: Identity,
}

impl Initiator {
    /// `desktop_dh_public` comes from the QR (pairing) or from the store (resume).
    pub fn new(identity: Identity, desktop_dh_public: [u8; 32], psk: [u8; KEY_LEN]) -> Self {
        let e = StaticSecret::random_from_rng(OsRng);
        let e_pub = PublicKey::from(&e);
        let es = e.diffie_hellman(&PublicKey::from(desktop_dh_public)).to_bytes();
        Self {
            e,
            e_pub,
            es,
            psk,
            identity,
        }
    }

    /// `header_json ‖ 0x0a ‖ e_i(32) ‖ aead(k0, payload)` — `payload` is the serialized Hello.
    pub fn first_frame(&self, header: &HelloHeader, payload: &[u8]) -> Result<Vec<u8>> {
        let header = serde_json::to_vec(header)?;
        let mut ad = header.clone();
        ad.extend_from_slice(self.e_pub.as_bytes());
        let k0 = derive(&Hkdf::<Sha256>::new(Some(&self.psk), &self.es), b"sluice-hs-init");
        let ct = aead(&k0)
            .encrypt(
                &nonce_for(0),
                Payload {
                    msg: payload,
                    aad: &ad,
                },
            )
            .map_err(|_| anyhow::anyhow!("hello encryption failed"))?;
        let mut out = header;
        out.push(b'\n');
        out.extend_from_slice(self.e_pub.as_bytes());
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Consume frame 1 → transport halves, secrets and the Welcome payload.
    pub fn finish(self, frame: &[u8]) -> Result<(Sealer, Opener, SessionSecrets, Vec<u8>)> {
        if frame.len() < 32 + 16 {
            bail!("short handshake reply");
        }
        let (r_pub_bytes, ct) = frame.split_at(32);
        let mut r_pub = [0u8; 32];
        r_pub.copy_from_slice(r_pub_bytes);
        let r_pub = PublicKey::from(r_pub);
        let ee = self.e.diffie_hellman(&r_pub).to_bytes();
        let se = self.identity.dh.diffie_hellman(&r_pub).to_bytes();
        let master = master_kdf(&self.psk, &self.es, &ee, &se);
        let mut ad = r_pub.as_bytes().to_vec();
        ad.extend_from_slice(self.e_pub.as_bytes());
        let welcome = aead(&derive(&master, b"sluice-hs-resp"))
            .decrypt(&nonce_for(0), Payload { msg: ct, aad: &ad })
            .map_err(|_| anyhow::anyhow!("handshake reply rejected (wrong code, key or desktop)"))?;
        let (sealer, opener, secrets) = finish_keys(&master, true);
        Ok((sealer, opener, secrets, welcome))
    }
}

/// Parse the clear header of an initiator frame (before any key is known).
pub fn peek_header(frame: &[u8]) -> Result<(HelloHeader, usize)> {
    let nl = frame
        .iter()
        .position(|&b| b == b'\n')
        .context("malformed hello (no header)")?;
    let header: HelloHeader = serde_json::from_slice(&frame[..nl]).context("malformed hello header")?;
    if header.v != 1 {
        bail!("unsupported sync protocol v{}", header.v);
    }
    Ok((header, nl + 1))
}

/// Desktop side, step 1: open the hello with the PSK selected from the header.
/// Returns the decrypted payload plus an opaque state for `respond`.
pub struct Responder {
    psk: [u8; KEY_LEN],
    es: [u8; 32],
    i_pub: PublicKey,
}

pub fn accept_hello(
    identity: &Identity,
    psk: &[u8; KEY_LEN],
    frame: &[u8],
    body_start: usize,
) -> Result<(Responder, Vec<u8>)> {
    let header = &frame[..body_start - 1];
    let rest = &frame[body_start..];
    if rest.len() < 32 + 16 {
        bail!("short hello");
    }
    let (i_pub_bytes, ct) = rest.split_at(32);
    let mut i_pub = [0u8; 32];
    i_pub.copy_from_slice(i_pub_bytes);
    let i_pub = PublicKey::from(i_pub);
    let es = identity.dh.diffie_hellman(&i_pub).to_bytes();
    let mut ad = header.to_vec();
    ad.extend_from_slice(i_pub.as_bytes());
    let k0 = derive(&Hkdf::<Sha256>::new(Some(psk), &es), b"sluice-hs-init");
    let hello = aead(&k0)
        .decrypt(&nonce_for(0), Payload { msg: ct, aad: &ad })
        .map_err(|_| anyhow::anyhow!("hello rejected (wrong pairing code or device key)"))?;
    Ok((Responder { psk: *psk, es, i_pub }, hello))
}

impl Responder {
    /// Desktop side, step 2: given the device's static DH key (from Hello, checked
    /// against the store on resume), produce frame 1 and the session halves.
    pub fn respond(
        self,
        device_dh_public: [u8; 32],
        welcome_payload: &[u8],
    ) -> Result<(Sealer, Opener, SessionSecrets, Vec<u8>)> {
        let e = StaticSecret::random_from_rng(OsRng);
        let e_pub = PublicKey::from(&e);
        let ee = e.diffie_hellman(&self.i_pub).to_bytes();
        let se = e.diffie_hellman(&PublicKey::from(device_dh_public)).to_bytes();
        let master = master_kdf(&self.psk, &self.es, &ee, &se);
        let mut ad = e_pub.as_bytes().to_vec();
        ad.extend_from_slice(self.i_pub.as_bytes());
        let ct = aead(&derive(&master, b"sluice-hs-resp"))
            .encrypt(
                &nonce_for(0),
                Payload {
                    msg: welcome_payload,
                    aad: &ad,
                },
            )
            .map_err(|_| anyhow::anyhow!("welcome encryption failed"))?;
        let mut reply = e_pub.as_bytes().to_vec();
        reply.extend_from_slice(&ct);
        let (sealer, opener, secrets) = finish_keys(&master, false);
        Ok((sealer, opener, secrets, reply))
    }
}

fn master_kdf(psk: &[u8; KEY_LEN], es: &[u8; 32], ee: &[u8; 32], se: &[u8; 32]) -> Hkdf<Sha256> {
    let mut ikm = Vec::with_capacity(96);
    ikm.extend_from_slice(es);
    ikm.extend_from_slice(ee);
    ikm.extend_from_slice(se);
    Hkdf::<Sha256>::new(Some(psk), &ikm)
}

fn finish_keys(master: &Hkdf<Sha256>, initiator: bool) -> (Sealer, Opener, SessionSecrets) {
    let i2r = derive(master, b"sluice-i2r");
    let r2i = derive(master, b"sluice-r2i");
    let (send, recv) = if initiator { (i2r, r2i) } else { (r2i, i2r) };
    let secrets = SessionSecrets {
        session_id: hex(&derive(master, b"sluice-session-id")[..8]),
        device_key: derive(master, b"sluice-device-key"),
    };
    (
        Sealer {
            key: aead(&send),
            ctr: 1,
        },
        Opener {
            key: aead(&recv),
            ctr: 1,
        },
        secrets,
    )
}

impl Sealer {
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let n = self.ctr;
        self.ctr += 1;
        let mut out = n.to_be_bytes().to_vec();
        let ct = self
            .key
            .encrypt(
                &nonce_for(n),
                Payload {
                    msg: plaintext,
                    aad: &n.to_be_bytes(),
                },
            )
            .map_err(|_| anyhow::anyhow!("frame encryption failed"))?;
        out.extend_from_slice(&ct);
        Ok(out)
    }
}

impl Opener {
    pub fn open(&mut self, frame: &[u8]) -> Result<Vec<u8>> {
        if frame.len() < 8 + 16 {
            bail!("short frame");
        }
        let (ctr, ct) = frame.split_at(8);
        let mut c = [0u8; 8];
        c.copy_from_slice(ctr);
        let n = u64::from_be_bytes(c);
        if n < self.ctr {
            bail!("replayed or out-of-order frame");
        }
        let pt = self
            .key
            .decrypt(&nonce_for(n), Payload { msg: ct, aad: ctr })
            .map_err(|_| anyhow::anyhow!("frame authentication failed"))?;
        self.ctr = n + 1;
        Ok(pt)
    }
}

/// Canonical bytes a device signs for a decision (and the desktop verifies).
pub fn decision_message(
    desktop_id: &str,
    session_id: &str,
    item_id: u64,
    version: &str,
    accept: bool,
    note: &str,
) -> Vec<u8> {
    format!(
        "sluice-decide-v1\n{desktop_id}\n{session_id}\n{item_id}\n{version}\n{}\n{note}",
        if accept { "approve" } else { "reject" }
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(mode: HelloMode) -> HelloHeader {
        HelloHeader {
            v: 1,
            mode,
            device_id: "m1".into(),
            desktop_id: "d1".into(),
        }
    }

    #[test]
    fn pairing_then_resume_with_static_keys() {
        let desktop = Identity::generate();
        let device = Identity::generate();
        let code = random_key();
        // --- pair ---
        let init = Initiator::new(device.clone(), desktop.dh_public(), code);
        let f0 = init.first_frame(&header(HelloMode::Pair), b"hello").unwrap();
        let (hdr, start) = peek_header(&f0).unwrap();
        assert_eq!(hdr.mode, HelloMode::Pair);
        let (resp, hello) = accept_hello(&desktop, &code, &f0, start).unwrap();
        assert_eq!(hello, b"hello");
        let (mut r_seal, mut r_open, r_secrets, reply) =
            resp.respond(device.dh_public(), b"welcome").unwrap();
        let (mut i_seal, mut i_open, i_secrets, welcome) = init.finish(&reply).unwrap();
        assert_eq!(welcome, b"welcome");
        assert_eq!(i_secrets.device_key, r_secrets.device_key);
        assert_eq!(i_secrets.session_id, r_secrets.session_id);
        let f = i_seal.seal(b"ping").unwrap();
        assert_eq!(r_open.open(&f).unwrap(), b"ping");
        assert!(r_open.open(&f).is_err(), "replay must be rejected");
        let g = r_seal.seal(b"pong").unwrap();
        assert_eq!(i_open.open(&g).unwrap(), b"pong");
        // --- resume with the derived device key as PSK ---
        let psk = i_secrets.device_key;
        let init = Initiator::new(device.clone(), desktop.dh_public(), psk);
        let f0 = init.first_frame(&header(HelloMode::Resume), b"again").unwrap();
        let (_, start) = peek_header(&f0).unwrap();
        let (resp, _) = accept_hello(&desktop, &psk, &f0, start).unwrap();
        let (_, _, _, reply) = resp.respond(device.dh_public(), b"wb").unwrap();
        assert!(init.finish(&reply).is_ok());
        // a device that does not own the static key the desktop expects fails
        let other = Identity::generate();
        let init = Initiator::new(other, desktop.dh_public(), psk);
        let f0 = init.first_frame(&header(HelloMode::Resume), b"x").unwrap();
        let (_, start) = peek_header(&f0).unwrap();
        let (resp, _) = accept_hello(&desktop, &psk, &f0, start).unwrap();
        let (_, _, _, reply) = resp.respond(device.dh_public(), b"wb").unwrap();
        assert!(init.finish(&reply).is_err());
    }

    #[test]
    fn wrong_code_or_wrong_desktop_is_rejected() {
        let desktop = Identity::generate();
        let device = Identity::generate();
        let code = random_key();
        let init = Initiator::new(device.clone(), desktop.dh_public(), code);
        let f0 = init.first_frame(&header(HelloMode::Pair), b"x").unwrap();
        let (_, start) = peek_header(&f0).unwrap();
        assert!(accept_hello(&desktop, &random_key(), &f0, start).is_err());
        assert!(accept_hello(&Identity::generate(), &code, &f0, start).is_err());
    }

    #[test]
    fn decision_signatures_verify() {
        let device = Identity::generate();
        let msg = decision_message("d1", "s1", 7, "v1", true, "ok");
        let sig = device.sign(&msg);
        assert!(verify_signature(&device.sign_public(), &msg, &sig));
        let tampered = decision_message("d1", "s1", 7, "v1", false, "ok");
        assert!(!verify_signature(&device.sign_public(), &tampered, &sig));
    }
}
