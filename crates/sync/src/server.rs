//! Desktop end of the channel: LAN listener (direct-first) + relay host loop
//! (fallback), one-time pairing, trusted-device resume, and the session loop that
//! serves the review queue / repo read model to devices.
//!
//! The desktop stays the source of truth (02 §5.4): devices only see a read model
//! and send 放行 / 驳回 *requests*; the desktop verifies the signature, checks the
//! item version, executes through its own write path and reports back.

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

use crate::crypto::{self, HelloMode, Identity, Opener, Sealer, SessionSecrets, accept_hello, peek_header};
use crate::link::Link;
use crate::pairing::{PairingPayload, now};
use crate::proto::*;
use crate::relay::RelayConn;
use crate::store::{DesktopState, PairedDevice, encode_key};
use crate::transport::{lan_addresses, read_frame, read_frame_max, write_frame};

/// What the desktop app plugs in: the read model and the (human-gated) write path.
pub trait Backend: Send + Sync + 'static {
    fn repo_view(&self) -> Option<RepoView>;
    fn queue(&self) -> Vec<ReviewItem>;
    /// Run a verified decision through the desktop write path. Blocking (may take
    /// as long as a commit / push); called on the session thread.
    fn decide(&self, req: DecisionRequest) -> DecisionOutcome;
    fn log(&self, offset: u32, limit: u32) -> (u32, Vec<LogRow>);
    fn commit(&self, oid: &str) -> Option<CommitDetail>;
    fn diff(&self, oid: &str, path: &str) -> Result<(String, bool)>;
    /// A device was paired / revoked / connected — let the UI refresh its panel.
    fn on_devices_changed(&self) {}
}

#[derive(Clone, Debug)]
pub struct DecisionRequest {
    pub id: u64,
    pub version: String,
    pub accept: bool,
    pub note: String,
    pub device: DeviceInfo,
}

#[derive(Clone, Debug)]
pub struct DecisionOutcome {
    /// "done" | "expired" | "rejected" | "failed" | "unknown"
    pub outcome: String,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct SessionInfo {
    pub device: DeviceInfo,
    pub via: String,
    pub since: i64,
    pub session_id: String,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub enabled: bool,
    pub lan_port: Option<u16>,
    pub lan_addrs: Vec<String>,
    pub relay: Option<String>,
    pub relay_connected: bool,
    pub sessions: Vec<SessionInfo>,
    pub pairing: Option<PairingPayload>,
}

struct Live {
    info: SessionInfo,
    tx: mpsc::Sender<ServerMsg>,
}

struct Inner {
    config_dir: PathBuf,
    state: Mutex<DesktopState>,
    identity: Identity,
    backend: Arc<dyn Backend>,
    pending_pairing: Mutex<Option<PairingPayload>>,
    sessions: Mutex<HashMap<u64, Live>>,
    next_session: AtomicU64,
    lan_port: Mutex<Option<u16>>,
    relay_connected: AtomicBool,
    stop: AtomicBool,
    version: String,
}

/// Handle owned by the desktop app.
#[derive(Clone)]
pub struct SyncServer {
    inner: Arc<Inner>,
}

impl SyncServer {
    /// Load state, bind the LAN listener and (if configured) start the relay loop.
    pub fn start(config_dir: &Path, backend: Arc<dyn Backend>, app_version: &str) -> Result<Self> {
        let mut state = DesktopState::load_or_init(config_dir);
        let identity = state.identity();
        let _ = state.save(config_dir);
        let inner = Arc::new(Inner {
            config_dir: config_dir.to_path_buf(),
            state: Mutex::new(state),
            identity,
            backend,
            pending_pairing: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(1),
            lan_port: Mutex::new(None),
            relay_connected: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            version: app_version.to_string(),
        });
        let server = Self { inner };
        server.start_lan()?;
        server.start_relay_loop();
        Ok(server)
    }

    pub fn config_dir(&self) -> &Path {
        &self.inner.config_dir
    }

    pub fn desktop_id(&self) -> String {
        self.inner.state.lock().unwrap().desktop_id.clone()
    }

    pub fn desktop_name(&self) -> String {
        self.inner.state.lock().unwrap().desktop_name.clone()
    }

    pub fn status(&self) -> Status {
        let st = self.inner.state.lock().unwrap();
        let port = *self.inner.lan_port.lock().unwrap();
        let pairing = self
            .inner
            .pending_pairing
            .lock()
            .unwrap()
            .clone()
            .filter(|p| !p.is_expired());
        Status {
            enabled: st.enabled,
            lan_port: port,
            lan_addrs: port
                .map(|p| lan_addresses().iter().map(|ip| fmt_addr(ip, p)).collect())
                .unwrap_or_default(),
            relay: st.relay.clone(),
            relay_connected: self.inner.relay_connected.load(Ordering::Relaxed),
            sessions: self
                .inner
                .sessions
                .lock()
                .unwrap()
                .values()
                .map(|l| l.info.clone())
                .collect(),
            pairing,
        }
    }

    pub fn devices(&self) -> Vec<PairedDevice> {
        self.inner.state.lock().unwrap().devices.clone()
    }

    /// Begin a pairing window: a fresh one-time code (10 min) rendered as QR by the UI.
    pub fn begin_pairing(&self) -> Result<PairingPayload> {
        let port = self
            .inner
            .lan_port
            .lock()
            .unwrap()
            .context("sync channel is not listening")?;
        let (id, name, relay, pk) = {
            let st = self.inner.state.lock().unwrap();
            (
                st.desktop_id.clone(),
                st.desktop_name.clone(),
                st.relay.clone(),
                self.inner.identity.dh_public(),
            )
        };
        let lan = lan_addresses().iter().map(|ip| fmt_addr(ip, port)).collect();
        let p = PairingPayload::new(&id, &name, pk, lan, relay);
        *self.inner.pending_pairing.lock().unwrap() = Some(p.clone());
        Ok(p)
    }

    pub fn cancel_pairing(&self) {
        *self.inner.pending_pairing.lock().unwrap() = None;
    }

    pub fn revoke(&self, device_id: &str) -> bool {
        let removed = {
            let mut st = self.inner.state.lock().unwrap();
            let r = st.revoke(device_id);
            let _ = st.save(&self.inner.config_dir);
            r
        };
        if removed {
            let ids: Vec<u64> = self
                .inner
                .sessions
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, l)| l.info.device.id == device_id)
                .map(|(k, _)| *k)
                .collect();
            for k in ids {
                if let Some(l) = self.inner.sessions.lock().unwrap().get(&k) {
                    let _ = l.tx.send(ServerMsg::Bye {
                        reason: "device revoked on the desktop".into(),
                    });
                }
            }
            self.inner.backend.on_devices_changed();
        }
        removed
    }

    pub fn set_relay(&self, relay: Option<String>) {
        let mut st = self.inner.state.lock().unwrap();
        st.relay = relay.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
        let _ = st.save(&self.inner.config_dir);
        drop(st);
        self.start_relay_loop();
    }

    pub fn set_enabled(&self, enabled: bool) {
        let mut st = self.inner.state.lock().unwrap();
        st.enabled = enabled;
        let _ = st.save(&self.inner.config_dir);
    }

    pub fn set_desktop_name(&self, name: &str) {
        let mut st = self.inner.state.lock().unwrap();
        if !name.trim().is_empty() {
            st.desktop_name = name.trim().to_string();
            let _ = st.save(&self.inner.config_dir);
        }
    }

    /// Push the current read model to every connected device (call after refresh /
    /// queue changes). Cheap when nobody is connected.
    pub fn broadcast_state(&self) {
        let sessions = self.inner.sessions.lock().unwrap();
        if sessions.is_empty() {
            return;
        }
        let msg = ServerMsg::State {
            repo: self.inner.backend.repo_view(),
            queue: self.inner.backend.queue(),
        };
        for l in sessions.values() {
            let _ = l.tx.send(msg.clone());
        }
    }

    pub fn broadcast_event(&self, event: DomainEvent) {
        let sessions = self.inner.sessions.lock().unwrap();
        for l in sessions.values() {
            let _ = l.tx.send(ServerMsg::Event { event: event.clone() });
        }
    }

    pub fn connected_count(&self) -> usize {
        self.inner.sessions.lock().unwrap().len()
    }

    pub fn shutdown(&self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        let sessions = self.inner.sessions.lock().unwrap();
        for l in sessions.values() {
            let _ = l.tx.send(ServerMsg::Bye {
                reason: "desktop is quitting".into(),
            });
        }
    }

    // ----------------------------------------------------------------------------

    fn start_lan(&self) -> Result<()> {
        let port = self.inner.state.lock().unwrap().port;
        let listener = TcpListener::bind(("0.0.0.0", port))
            .or_else(|_| TcpListener::bind(("0.0.0.0", 0)))
            .context("binding the sync listener")?;
        let actual = listener.local_addr()?.port();
        *self.inner.lan_port.lock().unwrap() = Some(actual);
        listener.set_nonblocking(true)?;
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name("sluice-sync-lan".into())
            .spawn(move || {
                while !inner.stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, peer)) => {
                            if !inner.state.lock().unwrap().enabled {
                                continue;
                            }
                            let inner = inner.clone();
                            std::thread::spawn(move || {
                                // BSD/macOS: accepted sockets inherit O_NONBLOCK from the listener.
                                let _ = stream.set_nonblocking(false);
                                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                                match Link::tcp(stream) {
                                    Ok(link) => {
                                        if let Err(e) = run_session(inner, link) {
                                            tracing::debug!("sync session from {peer} ended: {e:#}");
                                        }
                                    }
                                    Err(e) => tracing::debug!("sync accept: {e:#}"),
                                }
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(())
    }

    fn start_relay_loop(&self) {
        let relay = self.inner.state.lock().unwrap().relay.clone();
        let Some(relay) = relay else {
            return;
        };
        let inner = self.inner.clone();
        let _ = std::thread::Builder::new()
            .name("sluice-sync-relay".into())
            .spawn(move || {
                let mut backoff = 2u64;
                loop {
                    if inner.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    // Stop this loop if the relay setting changed (a new loop was started).
                    let current = inner.state.lock().unwrap().relay.clone();
                    if current.as_deref() != Some(relay.as_str()) {
                        break;
                    }
                    let room = inner.state.lock().unwrap().desktop_id.clone();
                    match RelayConn::join(&relay, &room, true, Duration::from_secs(8)) {
                        Ok(conn) => {
                            inner.relay_connected.store(true, Ordering::Relaxed);
                            inner.backend.on_devices_changed();
                            backoff = 2;
                            // Wait for a guest: keepalive every 30s while parked.
                            let _ = conn.set_read_timeout(Some(Duration::from_secs(30)));
                            let mut parked = conn;
                            let first = loop {
                                match parked.recv() {
                                    Ok(f) => break Some(f),
                                    Err(e) => {
                                        if crate::relay::is_timeout(&e) {
                                            if parked.keepalive().is_err()
                                                || inner.stop.load(Ordering::Relaxed)
                                            {
                                                break None;
                                            }
                                            continue;
                                        }
                                        break None;
                                    }
                                }
                            };
                            match first {
                                Some(first) => {
                                    if inner.state.lock().unwrap().enabled
                                        && let Ok(link) = Link::relay(parked)
                                        && let Err(e) = run_session_with_first(inner.clone(), link, first)
                                    {
                                        tracing::debug!("relay session ended: {e:#}");
                                    }
                                }
                                None => {
                                    inner.relay_connected.store(false, Ordering::Relaxed);
                                    inner.backend.on_devices_changed();
                                }
                            }
                            inner.relay_connected.store(false, Ordering::Relaxed);
                        }
                        Err(e) => {
                            tracing::debug!("relay {relay}: {e:#}");
                            inner.relay_connected.store(false, Ordering::Relaxed);
                            inner.backend.on_devices_changed();
                            std::thread::sleep(Duration::from_secs(backoff));
                            backoff = (backoff * 2).min(60);
                        }
                    }
                }
            });
    }
}

fn fmt_addr(ip: &std::net::IpAddr, port: u16) -> String {
    match ip {
        std::net::IpAddr::V4(v4) => format!("{v4}:{port}"),
        std::net::IpAddr::V6(v6) => format!("[{v6}]:{port}"),
    }
}

/// Hellos are a few hundred bytes; anything bigger is not a Sluice device.
const MAX_HELLO: usize = 64 * 1024;

fn run_session(inner: Arc<Inner>, mut link: Link) -> Result<()> {
    let first = read_frame_max(&mut link.reader, MAX_HELLO)?;
    run_session_with_first(inner, link, first)
}

/// Handshake (pair or resume) on `link`, then serve until the peer leaves.
fn run_session_with_first(inner: Arc<Inner>, mut link: Link, first: Vec<u8>) -> Result<()> {
    if first.len() > MAX_HELLO {
        bail!("hello too large");
    }
    let (header, start) = peek_header(&first)?;
    let my_id = inner.state.lock().unwrap().desktop_id.clone();
    if header.desktop_id != my_id {
        bail!("hello addressed to another desktop");
    }
    // Pick the PSK.
    let (psk, pairing, stored) = match header.mode {
        HelloMode::Pair => {
            let p = inner.pending_pairing.lock().unwrap().clone();
            match p {
                Some(p) if !p.is_expired() => (p.code, true, None),
                Some(_) => bail!("pairing code expired"),
                None => bail!("no pairing in progress"),
            }
        }
        HelloMode::Resume => {
            let st = inner.state.lock().unwrap();
            let d = st
                .device(&header.device_id)
                .context("unknown device (revoked?)")?
                .clone();
            (d.key_bytes().context("corrupt device key")?, false, Some(d))
        }
    };
    let (resp, hello_bytes) = accept_hello(&inner.identity, &psk, &first, start)?;
    let hello: Hello = serde_json::from_slice(&hello_bytes).context("bad hello payload")?;
    let dh_pub = decode32(&hello.dh_public).context("bad device key in hello")?;
    let sign_pub = decode32(&hello.sign_public).context("bad device signing key in hello")?;
    if let Some(d) = &stored
        && let Some(pinned) = d.dh_public_bytes()
        && pinned != dh_pub
    {
        bail!("device static key does not match the pinned key");
    }
    let lan_port = *inner.lan_port.lock().unwrap();
    let welcome = Welcome {
        desktop_id: my_id.clone(),
        desktop_name: inner.state.lock().unwrap().desktop_name.clone(),
        desktop_version: inner.version.clone(),
        lan: lan_port
            .map(|p| lan_addresses().iter().map(|ip| fmt_addr(ip, p)).collect())
            .unwrap_or_default(),
        repo: inner.backend.repo_view(),
        queue: inner.backend.queue(),
    };
    let (sealer, opener, secrets, reply) = resp.respond(dh_pub, &serde_json::to_vec(&welcome)?)?;
    write_frame(&mut link.writer, &reply)?;

    // Persist trust (pairing) / last-seen (resume).
    let device = DeviceInfo {
        id: header.device_id.clone(),
        name: hello.device_name.clone(),
        platform: hello.platform.clone(),
    };
    {
        let mut st = inner.state.lock().unwrap();
        let rec = PairedDevice {
            id: device.id.clone(),
            name: device.name.clone(),
            platform: device.platform.clone(),
            key: if pairing {
                encode_key(&secrets.device_key)
            } else {
                stored.as_ref().map(|d| d.key.clone()).unwrap_or_default()
            },
            dh_public: B64.encode(dh_pub),
            sign_public: B64.encode(sign_pub),
            paired_at: stored.as_ref().map(|d| d.paired_at).unwrap_or_else(now),
            last_seen: now(),
            last_via: link.via.to_string(),
        };
        st.upsert_device(rec);
        let _ = st.save(&inner.config_dir);
    }
    if pairing {
        *inner.pending_pairing.lock().unwrap() = None; // one-time code consumed
    }
    inner.backend.on_devices_changed();
    link.set_read_timeout(None);
    serve(inner, link, sealer, opener, secrets, device, sign_pub)
}

fn decode32(s: &str) -> Option<[u8; 32]> {
    let v = B64.decode(s.as_bytes()).ok()?;
    if v.len() != 32 {
        return None;
    }
    let mut k = [0u8; 32];
    k.copy_from_slice(&v);
    Some(k)
}

const KEEPALIVE: Duration = Duration::from_secs(25);

fn serve(
    inner: Arc<Inner>,
    link: Link,
    mut sealer: Sealer,
    mut opener: Opener,
    secrets: SessionSecrets,
    device: DeviceInfo,
    sign_pub: [u8; 32],
) -> Result<()> {
    let key = inner.next_session.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel::<ServerMsg>();
    let info = SessionInfo {
        device: device.clone(),
        via: link.via.to_string(),
        since: now(),
        session_id: secrets.session_id.clone(),
    };
    inner
        .sessions
        .lock()
        .unwrap()
        .insert(key, Live { info, tx: tx.clone() });
    inner.backend.on_devices_changed();
    let closer = link.closer();
    let Link {
        mut reader,
        mut writer,
        ..
    } = link;

    // Writer thread: serializes + encrypts outbound messages, keepalive while idle.
    let writer_closer = closer.clone();
    let writer_thread = std::thread::spawn(move || {
        let mut last = Instant::now();
        loop {
            match rx.recv_timeout(KEEPALIVE) {
                Ok(msg) => {
                    let bye = matches!(msg, ServerMsg::Bye { .. });
                    let bytes = match serde_json::to_vec(&msg) {
                        Ok(b) => b,
                        Err(_) => continue,
                    };
                    let Ok(frame) = sealer.seal(&bytes) else { break };
                    if write_frame(&mut writer, &frame).is_err() {
                        break;
                    }
                    last = Instant::now();
                    if bye {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if last.elapsed() >= KEEPALIVE {
                        let Ok(frame) =
                            sealer.seal(&serde_json::to_vec(&ServerMsg::Pong).unwrap_or_default())
                        else {
                            break;
                        };
                        if write_frame(&mut writer, &frame).is_err() {
                            break;
                        }
                        last = Instant::now();
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        writer_closer();
    });

    // Reader loop (this thread): decrypt, dispatch.
    let result: Result<()> = (|| {
        loop {
            let frame = read_frame(&mut reader)?;
            let plain = opener.open(&frame)?;
            let msg: ClientMsg = serde_json::from_slice(&plain).context("bad client message")?;
            match msg {
                ClientMsg::Ping => {
                    let _ = tx.send(ServerMsg::Pong);
                }
                ClientMsg::GetState => {
                    let _ = tx.send(ServerMsg::State {
                        repo: inner.backend.repo_view(),
                        queue: inner.backend.queue(),
                    });
                }
                ClientMsg::Decide {
                    id,
                    version,
                    accept,
                    note,
                    sig,
                } => {
                    let desktop_id = inner.state.lock().unwrap().desktop_id.clone();
                    let msg = crypto::decision_message(
                        &desktop_id,
                        &secrets.session_id,
                        id,
                        &version,
                        accept,
                        &note,
                    );
                    let sig_bytes = B64.decode(sig.as_bytes()).unwrap_or_default();
                    if !crypto::verify_signature(&sign_pub, &msg, &sig_bytes) {
                        let _ = tx.send(ServerMsg::Decided {
                            id,
                            accepted: accept,
                            outcome: "failed".into(),
                            detail: "signature verification failed — decision ignored".into(),
                        });
                        continue;
                    }
                    let out = inner.backend.decide(DecisionRequest {
                        id,
                        version,
                        accept,
                        note,
                        device: device.clone(),
                    });
                    let _ = tx.send(ServerMsg::Decided {
                        id,
                        accepted: accept,
                        outcome: out.outcome,
                        detail: out.detail,
                    });
                }
                ClientMsg::Log { offset, limit } => {
                    let limit = if limit == 0 { 50 } else { limit.min(500) };
                    let (total, rows) = inner.backend.log(offset, limit);
                    let _ = tx.send(ServerMsg::LogPage { offset, total, rows });
                }
                ClientMsg::Commit { oid } => match inner.backend.commit(&oid) {
                    Some(detail) => {
                        let _ = tx.send(ServerMsg::CommitDetail { detail });
                    }
                    None => {
                        let _ = tx.send(ServerMsg::Error {
                            message: format!("unknown commit {oid}"),
                        });
                    }
                },
                ClientMsg::Diff { oid, path } => match inner.backend.diff(&oid, &path) {
                    Ok((patch, truncated)) => {
                        let _ = tx.send(ServerMsg::Diff {
                            oid,
                            path,
                            patch,
                            truncated,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(ServerMsg::Error {
                            message: format!("{e:#}"),
                        });
                    }
                },
                ClientMsg::Unpair => {
                    {
                        let mut st = inner.state.lock().unwrap();
                        st.revoke(&device.id);
                        let _ = st.save(&inner.config_dir);
                    }
                    let _ = tx.send(ServerMsg::Bye {
                        reason: "unpaired".into(),
                    });
                    break Ok(());
                }
            }
        }
    })();
    inner.sessions.lock().unwrap().remove(&key);
    drop(tx);
    closer();
    let _ = writer_thread.join();
    inner.backend.on_devices_changed();
    result
}
