//! Device end of the channel (the phone through UniFFI, or `sluice pair` /
//! `sluice remote` on a second machine as a stand-in): pairing from a scanned
//! payload, LAN-first / relay-fallback connection, signed decisions, and a cached
//! read model the shell renders.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

use crate::crypto::{self, HelloHeader, HelloMode, Identity, Initiator, Sealer};
use crate::link::Link;
use crate::pairing::{PairingPayload, now};
use crate::proto::*;
use crate::relay::RelayConn;
use crate::store::{DeviceState, PairedDesktop, encode_key};
use crate::transport::{connect_timeout, read_frame, write_frame};

pub type Sink = Arc<dyn Fn(DomainEvent) + Send + Sync>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConnInfo {
    pub desktop_id: String,
    pub desktop_name: String,
    pub desktop_version: String,
    /// "lan" | "relay"
    pub via: String,
    pub session_id: String,
    pub since: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Cache {
    pub repo: Option<RepoView>,
    pub queue: Vec<ReviewItem>,
    pub connected: Option<ConnInfo>,
    /// Last 50 events, newest last (for shells that attach late).
    pub events: VecDeque<DomainEvent>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Decided {
    pub id: u64,
    pub accepted: bool,
    pub outcome: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    State,
    Decided,
    LogPage,
    CommitDetail,
    Diff,
}

struct Live {
    out: Mutex<(Sealer, Box<dyn Write + Send>)>,
    waiting: Mutex<Option<(Expect, mpsc::Sender<ServerMsg>)>>,
    call_lock: Mutex<()>,
    closer: Arc<dyn Fn() + Send + Sync>,
    info: ConnInfo,
    alive: AtomicBool,
}

pub struct Client {
    config_dir: PathBuf,
    app_version: String,
    state: Mutex<DeviceState>,
    identity: Identity,
    live: Mutex<Option<Arc<Live>>>,
    cache: Arc<Mutex<Cache>>,
    sink: Arc<Mutex<Option<Sink>>>,
}

const LAN_TIMEOUT: Duration = Duration::from_millis(1500);
const RELAY_TIMEOUT: Duration = Duration::from_secs(8);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const DECIDE_TIMEOUT: Duration = Duration::from_secs(180);

impl Client {
    pub fn new(config_dir: &Path, platform: &str, app_version: &str) -> Self {
        let mut state = DeviceState::load_or_init(config_dir, platform);
        let identity = state.identity();
        let _ = state.save(config_dir);
        Self {
            config_dir: config_dir.to_path_buf(),
            app_version: app_version.to_string(),
            state: Mutex::new(state),
            identity,
            live: Mutex::new(None),
            cache: Arc::new(Mutex::new(Cache::default())),
            sink: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_sink(&self, sink: Option<Sink>) {
        *self.sink.lock().unwrap() = sink;
    }

    pub fn device_id(&self) -> String {
        self.state.lock().unwrap().device_id.clone()
    }

    pub fn device_name(&self) -> String {
        self.state.lock().unwrap().device_name.clone()
    }

    pub fn set_device_name(&self, name: &str) {
        let mut st = self.state.lock().unwrap();
        if !name.trim().is_empty() {
            st.device_name = name.trim().to_string();
            let _ = st.save(&self.config_dir);
        }
    }

    pub fn desktops(&self) -> Vec<PairedDesktop> {
        self.state.lock().unwrap().desktops.clone()
    }

    pub fn cache(&self) -> Cache {
        self.cache.lock().unwrap().clone()
    }

    pub fn is_connected(&self) -> bool {
        self.live
            .lock()
            .unwrap()
            .as_ref()
            .map(|l| l.alive.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    pub fn connection(&self) -> Option<ConnInfo> {
        self.live
            .lock()
            .unwrap()
            .as_ref()
            .filter(|l| l.alive.load(Ordering::Relaxed))
            .map(|l| l.info.clone())
    }

    /// Pair with a desktop from the scanned QR text and stay connected.
    pub fn pair(&self, payload_text: &str) -> Result<ConnInfo> {
        let p = PairingPayload::decode(payload_text)?;
        if p.is_expired() {
            bail!("this pairing code has expired — show a new QR on the desktop");
        }
        self.disconnect_inner("re-pairing");
        let link = open_link(&p.lan, p.relay.as_deref(), &p.desktop_id)?;
        let (info, welcome, secrets) =
            self.handshake(link, &p.desktop_id, p.desktop_dh_public, p.code, HelloMode::Pair)?;
        {
            let mut st = self.state.lock().unwrap();
            st.upsert_desktop(PairedDesktop {
                desktop_id: p.desktop_id.clone(),
                desktop_name: welcome.desktop_name.clone(),
                key: encode_key(&secrets.device_key),
                dh_public: B64.encode(p.desktop_dh_public),
                lan: if welcome.lan.is_empty() {
                    p.lan.clone()
                } else {
                    welcome.lan.clone()
                },
                relay: p.relay.clone(),
                paired_at: now(),
            });
            let _ = st.save(&self.config_dir);
        }
        Ok(info)
    }

    /// Connect to a paired desktop (the only one when `desktop_id` is None).
    pub fn connect(&self, desktop_id: Option<&str>) -> Result<ConnInfo> {
        let d = {
            let st = self.state.lock().unwrap();
            match desktop_id {
                Some(id) => st.desktops.iter().find(|d| d.desktop_id == id).cloned(),
                None => st.desktops.first().cloned(),
            }
        }
        .context("no paired desktop — scan the QR from Sluice → 移动端 first")?;
        if let Some(c) = self.connection()
            && c.desktop_id == d.desktop_id
        {
            return Ok(c);
        }
        self.disconnect_inner("reconnecting");
        let psk = d.key_bytes().context("corrupt device key")?;
        let pk = d.dh_public_bytes().context("corrupt desktop key")?;
        let link = open_link(&d.lan, d.relay.as_deref(), &d.desktop_id)?;
        let (info, welcome, _) = self.handshake(link, &d.desktop_id, pk, psk, HelloMode::Resume)?;
        if !welcome.lan.is_empty() && welcome.lan != d.lan {
            let mut st = self.state.lock().unwrap();
            if let Some(slot) = st.desktops.iter_mut().find(|x| x.desktop_id == d.desktop_id) {
                slot.lan = welcome.lan.clone();
                slot.desktop_name = welcome.desktop_name.clone();
            }
            let _ = st.save(&self.config_dir);
        }
        Ok(info)
    }

    pub fn disconnect(&self) {
        self.disconnect_inner("disconnected by the user");
    }

    /// Ask the desktop to forget this device, then drop it locally.
    pub fn unpair(&self, desktop_id: &str) -> Result<()> {
        if self.connect(Some(desktop_id)).is_ok() {
            if let Some(l) = self.live.lock().unwrap().clone() {
                let _ = send(&l, &ClientMsg::Unpair);
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        self.disconnect_inner("unpaired");
        self.forget(desktop_id);
        Ok(())
    }

    pub fn forget(&self, desktop_id: &str) {
        let mut st = self.state.lock().unwrap();
        st.forget(desktop_id);
        let _ = st.save(&self.config_dir);
    }

    /// Pull the current read model (also pushed by the desktop on change).
    pub fn refresh(&self) -> Result<Cache> {
        let l = self.live()?;
        match self.call(&l, Expect::State, &ClientMsg::GetState, CALL_TIMEOUT)? {
            ServerMsg::State { repo, queue } => {
                let mut c = self.cache.lock().unwrap();
                c.repo = repo;
                c.queue = queue;
                Ok(c.clone())
            }
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// 放行 (accept) / 驳回 (reject) — signed with the device key; the desktop
    /// executes and answers with the outcome.
    pub fn decide(&self, id: u64, version: &str, accept: bool, note: &str) -> Result<Decided> {
        let l = self.live()?;
        let msg = crypto::decision_message(&l.info.desktop_id, &l.info.session_id, id, version, accept, note);
        let sig = B64.encode(self.identity.sign(&msg));
        let req = ClientMsg::Decide {
            id,
            version: version.to_string(),
            accept,
            note: note.to_string(),
            sig,
        };
        match self.call(&l, Expect::Decided, &req, DECIDE_TIMEOUT)? {
            ServerMsg::Decided {
                id,
                accepted,
                outcome,
                detail,
            } => Ok(Decided {
                id,
                accepted,
                outcome,
                detail,
            }),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    pub fn log(&self, offset: u32, limit: u32) -> Result<(u32, Vec<LogRow>)> {
        let l = self.live()?;
        match self.call(
            &l,
            Expect::LogPage,
            &ClientMsg::Log { offset, limit },
            CALL_TIMEOUT,
        )? {
            ServerMsg::LogPage { total, rows, .. } => Ok((total, rows)),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    pub fn commit(&self, oid: &str) -> Result<CommitDetail> {
        let l = self.live()?;
        match self.call(
            &l,
            Expect::CommitDetail,
            &ClientMsg::Commit { oid: oid.to_string() },
            CALL_TIMEOUT,
        )? {
            ServerMsg::CommitDetail { detail } => Ok(detail),
            ServerMsg::Error { message } => bail!("{message}"),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    pub fn diff(&self, oid: &str, path: &str) -> Result<(String, bool)> {
        let l = self.live()?;
        match self.call(
            &l,
            Expect::Diff,
            &ClientMsg::Diff {
                oid: oid.to_string(),
                path: path.to_string(),
            },
            CALL_TIMEOUT,
        )? {
            ServerMsg::Diff { patch, truncated, .. } => Ok((patch, truncated)),
            ServerMsg::Error { message } => bail!("{message}"),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    // ------------------------------------------------------------------------

    fn live(&self) -> Result<Arc<Live>> {
        self.live
            .lock()
            .unwrap()
            .clone()
            .filter(|l| l.alive.load(Ordering::Relaxed))
            .context("not connected to the desktop")
    }

    fn disconnect_inner(&self, reason: &str) {
        let prev = self.live.lock().unwrap().take();
        if let Some(l) = prev {
            l.alive.store(false, Ordering::Relaxed);
            (l.closer)();
            self.cache.lock().unwrap().connected = None;
            self.emit(DomainEvent::Disconnected {
                reason: reason.to_string(),
            });
        }
    }

    fn emit(&self, ev: DomainEvent) {
        {
            let mut c = self.cache.lock().unwrap();
            c.events.push_back(ev.clone());
            while c.events.len() > 50 {
                c.events.pop_front();
            }
        }
        let sink = self.sink.lock().unwrap().clone();
        if let Some(s) = sink {
            s(ev);
        }
    }

    fn handshake(
        &self,
        mut link: Link,
        desktop_id: &str,
        desktop_pk: [u8; 32],
        psk: [u8; 32],
        mode: HelloMode,
    ) -> Result<(ConnInfo, Welcome, crypto::SessionSecrets)> {
        let (device_id, device_name, platform) = {
            let st = self.state.lock().unwrap();
            (st.device_id.clone(), st.device_name.clone(), st.platform.clone())
        };
        let init = Initiator::new(self.identity.clone(), desktop_pk, psk);
        let header = HelloHeader {
            v: PROTOCOL_VERSION,
            mode,
            device_id: device_id.clone(),
            desktop_id: desktop_id.to_string(),
        };
        let hello = Hello {
            device_name,
            platform,
            app_version: self.app_version.clone(),
            dh_public: B64.encode(self.identity.dh_public()),
            sign_public: B64.encode(self.identity.sign_public()),
        };
        let f0 = init.first_frame(&header, &serde_json::to_vec(&hello)?)?;
        link.set_read_timeout(Some(Duration::from_secs(10)));
        write_frame(&mut link.writer, &f0)?;
        let reply = read_frame(&mut link.reader).context("desktop did not answer the handshake")?;
        let (sealer, mut opener, secrets, welcome_bytes) = init.finish(&reply)?;
        let welcome: Welcome = serde_json::from_slice(&welcome_bytes).context("bad welcome payload")?;
        link.set_read_timeout(None);
        let info = ConnInfo {
            desktop_id: welcome.desktop_id.clone(),
            desktop_name: welcome.desktop_name.clone(),
            desktop_version: welcome.desktop_version.clone(),
            via: link.via.to_string(),
            session_id: secrets.session_id.clone(),
            since: now(),
        };
        let closer = link.closer();
        let Link {
            mut reader, writer, ..
        } = link;
        let live = Arc::new(Live {
            out: Mutex::new((sealer, writer)),
            waiting: Mutex::new(None),
            call_lock: Mutex::new(()),
            closer,
            info: info.clone(),
            alive: AtomicBool::new(true),
        });
        {
            let mut c = self.cache.lock().unwrap();
            c.repo = welcome.repo.clone();
            c.queue = welcome.queue.clone();
            c.connected = Some(info.clone());
        }
        *self.live.lock().unwrap() = Some(live.clone());
        self.emit(DomainEvent::Connected {
            desktop_id: info.desktop_id.clone(),
            desktop_name: info.desktop_name.clone(),
            via: info.via.clone(),
        });

        // Reader thread: cache updates, call replies, events.
        let cache = Arc::new(ReaderCtx {
            live: live.clone(),
            on_msg: Box::new({
                let sink = SharedSink(self.sink.clone());
                let cache = SharedCache(self.cache.clone());
                move |msg: ServerMsg, live: &Live| handle_inbound(msg, live, &sink, &cache)
            }),
        });
        std::thread::Builder::new()
            .name("sluice-sync-client".into())
            .spawn(move || {
                let reason: String;
                loop {
                    let frame = match read_frame(&mut reader) {
                        Ok(f) => f,
                        Err(e) => {
                            reason = format!("{e:#}");
                            break;
                        }
                    };
                    let plain = match opener.open(&frame) {
                        Ok(p) => p,
                        Err(e) => {
                            reason = format!("{e:#}");
                            break;
                        }
                    };
                    let Ok(msg) = serde_json::from_slice::<ServerMsg>(&plain) else {
                        continue;
                    };
                    if let ServerMsg::Bye { reason: r } = &msg {
                        reason = r.clone();
                        (cache.on_msg)(msg, &cache.live);
                        break;
                    }
                    (cache.on_msg)(msg, &cache.live);
                }
                if cache.live.alive.swap(false, Ordering::Relaxed) {
                    (cache.live.closer)();
                    if let Some((_, tx)) = cache.live.waiting.lock().unwrap().take() {
                        let _ = tx.send(ServerMsg::Error {
                            message: format!("disconnected: {reason}"),
                        });
                    }
                    (cache.on_msg)(
                        ServerMsg::Event {
                            event: DomainEvent::Disconnected { reason },
                        },
                        &cache.live,
                    );
                }
            })?;
        Ok((info, welcome, secrets))
    }

    fn call(&self, l: &Arc<Live>, expect: Expect, msg: &ClientMsg, timeout: Duration) -> Result<ServerMsg> {
        let _guard = l.call_lock.lock().unwrap();
        let (tx, rx) = mpsc::channel();
        *l.waiting.lock().unwrap() = Some((expect, tx));
        if let Err(e) = send(l, msg) {
            *l.waiting.lock().unwrap() = None;
            return Err(e);
        }
        let r = rx.recv_timeout(timeout);
        *l.waiting.lock().unwrap() = None;
        match r {
            Ok(ServerMsg::Error { message }) => bail!("{message}"),
            Ok(m) => Ok(m),
            Err(_) => bail!("desktop did not answer in time"),
        }
    }
}

fn send(l: &Live, msg: &ClientMsg) -> Result<()> {
    let bytes = serde_json::to_vec(msg)?;
    let mut out = l.out.lock().unwrap();
    let frame = out.0.seal(&bytes)?;
    write_frame(&mut out.1, &frame)
}

type OnMsg = Box<dyn Fn(ServerMsg, &Live) + Send + Sync>;

struct ReaderCtx {
    live: Arc<Live>,
    on_msg: OnMsg,
}

#[derive(Clone)]
struct SharedSink(Arc<Mutex<Option<Sink>>>);
#[derive(Clone)]
struct SharedCache(Arc<Mutex<Cache>>);

fn handle_inbound(msg: ServerMsg, live: &Live, sink: &SharedSink, cache: &SharedCache) {
    // Deliver to a waiting call when the variant matches; always update the cache.
    let deliver = |m: &ServerMsg, e: Expect| -> bool {
        let mut w = live.waiting.lock().unwrap();
        match w.as_ref() {
            Some((exp, _)) if *exp == e => {
                let (_, tx) = w.take().unwrap();
                let _ = tx.send(m.clone());
                true
            }
            _ => false,
        }
    };
    let emit = |ev: DomainEvent| {
        {
            let mut c = cache.0.lock().unwrap();
            c.events.push_back(ev.clone());
            while c.events.len() > 50 {
                c.events.pop_front();
            }
        }
        let s = sink.0.lock().unwrap().clone();
        if let Some(s) = s {
            s(ev);
        }
    };
    match msg {
        ServerMsg::Pong => {}
        ServerMsg::State { repo, queue } => {
            let (repo_changed, pending) = {
                let mut c = cache.0.lock().unwrap();
                let changed = c.repo != repo;
                let queue_changed = c.queue != queue;
                c.repo = repo.clone();
                c.queue = queue.clone();
                (
                    changed,
                    if queue_changed {
                        Some(queue.len() as u32)
                    } else {
                        None
                    },
                )
            };
            let m = ServerMsg::State {
                repo: repo.clone(),
                queue,
            };
            let delivered = deliver(&m, Expect::State);
            if repo_changed && let Some(r) = repo {
                emit(DomainEvent::RepoChanged { repo: r });
            }
            if let Some(p) = pending
                && !delivered
            {
                emit(DomainEvent::QueueChanged { pending: p });
            }
        }
        m @ ServerMsg::Decided { .. } => {
            deliver(&m, Expect::Decided);
        }
        m @ ServerMsg::LogPage { .. } => {
            deliver(&m, Expect::LogPage);
        }
        m @ ServerMsg::CommitDetail { .. } => {
            deliver(&m, Expect::CommitDetail);
        }
        m @ ServerMsg::Diff { .. } => {
            deliver(&m, Expect::Diff);
        }
        ServerMsg::Event { event } => {
            if let DomainEvent::Disconnected { .. } = &event {
                cache.0.lock().unwrap().connected = None;
            }
            emit(event);
        }
        m @ ServerMsg::Error { .. } => {
            // An error answers whichever call is in flight; otherwise surface it.
            let mut w = live.waiting.lock().unwrap();
            if let Some((_, tx)) = w.take() {
                let _ = tx.send(m);
            } else if let ServerMsg::Error { message } = m {
                drop(w);
                emit(DomainEvent::Error { message });
            }
        }
        ServerMsg::Bye { reason } => {
            cache.0.lock().unwrap().connected = None;
            emit(DomainEvent::Disconnected { reason });
        }
    }
}

/// Try the LAN candidates first (short timeout each), then the relay.
fn open_link(lan: &[String], relay: Option<&str>, room: &str) -> Result<Link> {
    let mut errors = Vec::new();
    for addr in lan {
        match connect_timeout(addr, LAN_TIMEOUT) {
            Ok(s) => return Link::tcp(s),
            Err(e) => errors.push(format!("{addr}: {e}")),
        }
    }
    if let Some(relay) = relay {
        match RelayConn::join(relay, room, false, RELAY_TIMEOUT) {
            Ok(c) => return Link::relay(c),
            Err(e) => errors.push(format!("relay {relay}: {e}")),
        }
    }
    if errors.is_empty() {
        bail!("the pairing payload lists no address to connect to");
    }
    bail!("cannot reach the desktop — {}", errors.join("; "))
}
