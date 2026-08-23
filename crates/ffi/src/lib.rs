//! UniFFI export surface of the Sluice core (02 §5.5): the iOS (Swift / UIKit) and
//! Android (Kotlin / Compose) shells call these objects and never re-implement any
//! git or domain logic (S6).
//!
//! * `SluiceSession` — open a local repository (embedded gix read path) and / or
//!   connect to a desktop Sluice instance through the sync channel (pair via QR).
//! * `RepoView` — log pagination, commit detail, diff retrieval (local or mirrored
//!   from the desktop, same API).
//! * `ReviewQueue` — pending proposals, `approve` / `reject` (signed, executed by
//!   the desktop; 放行来源设备 recorded there).
//! * `EventSink` — callback interface delivering serializable `DomainEvent`s.
//!
//! Type discipline: only records / enums / interfaces / callbacks, no generics, no
//! references, no UI types; the internal protocol types are mapped explicitly so
//! desktop refactors never touch the mobile API. All calls are async (tokio on the
//! Rust side → Swift `async` / Kotlin `suspend`); callbacks fire on Rust threads
//! and the shell hops to its main thread.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sluice_core::{BlobRev, Console, GitReader, LogQuery, Oid};
use sluice_sync::proto as p;

uniffi::setup_scaffolding!();

// ---------------------------------------------------------------------------
// Records / enums
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum SluiceError {
    #[error("{message}")]
    Failed { message: String },
}

impl From<anyhow::Error> for SluiceError {
    fn from(e: anyhow::Error) -> Self {
        SluiceError::Failed {
            message: format!("{e:#}"),
        }
    }
}

type FfiResult<T> = Result<T, SluiceError>;

/// Read model of the repository shown on the repo card.
#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct RepoSummary {
    pub path: String,
    pub name: String,
    pub branch: String,
    pub head_short: String,
    pub head_subject: String,
    pub ahead: u32,
    pub behind: u32,
    pub changed_files: u32,
    /// "git" | "jj"
    pub vcs: String,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct ReviewItem {
    pub id: u64,
    pub client: String,
    /// "commit" | "branch" | "push"
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub files: Vec<String>,
    pub patch: Option<String>,
    pub received_at: i64,
    /// Baseline version the decision must quote (stale → "expired").
    pub version: String,
    /// "pending" | "busy"
    pub state: String,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct LogRow {
    pub oid: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub at: i64,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
    pub ai_badge: Option<String>,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct LogPage {
    pub offset: u32,
    pub total: u32,
    pub rows: Vec<LogRow>,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct CommitDetail {
    pub oid: String,
    pub short: String,
    pub subject: String,
    pub body: String,
    pub author: String,
    pub at: i64,
    pub parents: Vec<String>,
    pub files: Vec<ChangedFile>,
    pub ai_badge: Option<String>,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct DiffText {
    pub oid: String,
    pub path: String,
    /// Unified diff; the shell renders it with its own viewer (word-level
    /// highlighting is computed on the shell from the +/- pairs).
    pub patch: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct ConnectionInfo {
    pub desktop_id: String,
    pub desktop_name: String,
    pub desktop_version: String,
    /// "lan" | "relay" | "local"
    pub via: String,
    pub session_id: String,
    pub since: i64,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct PairedDesktop {
    pub desktop_id: String,
    pub desktop_name: String,
    pub lan: Vec<String>,
    pub relay: Option<String>,
    pub paired_at: i64,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct Decision {
    pub id: u64,
    pub accepted: bool,
    /// "done" | "expired" | "rejected" | "failed" | "unknown"
    pub outcome: String,
    pub detail: String,
}

/// Same event family the desktop emits over the channel (02 §5.5 "与桌面 UI 订阅的是同一套事件类型").
#[derive(Clone, Debug, uniffi::Enum)]
pub enum DomainEvent {
    Connected {
        desktop_id: String,
        desktop_name: String,
        via: String,
    },
    Disconnected {
        reason: String,
    },
    RepoChanged {
        repo: RepoSummary,
    },
    Proposed {
        item: ReviewItem,
        pending: u32,
    },
    QueueChanged {
        pending: u32,
    },
    Decided {
        id: u64,
        accepted: bool,
        outcome: String,
        detail: String,
        source: String,
        source_name: String,
        pending: u32,
    },
    Error {
        message: String,
    },
}

#[uniffi::export(callback_interface)]
pub trait EventSink: Send + Sync {
    fn on_event(&self, event: DomainEvent);
}

// ---------------------------------------------------------------------------
// Mapping layer (protocol types → exported types)
// ---------------------------------------------------------------------------

fn map_repo(r: &p::RepoView) -> RepoSummary {
    RepoSummary {
        path: r.path.clone(),
        name: r.name.clone(),
        branch: r.branch.clone(),
        head_short: r.head_short.clone(),
        head_subject: r.head_subject.clone(),
        ahead: r.ahead,
        behind: r.behind,
        changed_files: r.changed_files,
        vcs: r.vcs.clone(),
        updated_at: r.updated_at,
    }
}

fn map_item(i: &p::ReviewItem) -> ReviewItem {
    ReviewItem {
        id: i.id,
        client: i.client.clone(),
        kind: i.kind.clone(),
        title: i.title.clone(),
        detail: i.detail.clone(),
        files: i.files.clone(),
        patch: i.patch.clone(),
        received_at: i.received_at,
        version: i.version.clone(),
        state: i.state.clone(),
    }
}

fn map_row(r: &p::LogRow) -> LogRow {
    LogRow {
        oid: r.oid.clone(),
        short: r.short.clone(),
        subject: r.subject.clone(),
        author: r.author.clone(),
        at: r.at,
        parents: r.parents.clone(),
        refs: r.refs.clone(),
        ai_badge: r.ai_badge.clone(),
    }
}

fn map_detail(d: &p::CommitDetail) -> CommitDetail {
    CommitDetail {
        oid: d.oid.clone(),
        short: d.short.clone(),
        subject: d.subject.clone(),
        body: d.body.clone(),
        author: d.author.clone(),
        at: d.at,
        parents: d.parents.clone(),
        files: d
            .files
            .iter()
            .map(|f| ChangedFile {
                path: f.path.clone(),
                status: f.status.clone(),
                additions: f.additions,
                deletions: f.deletions,
            })
            .collect(),
        ai_badge: d.ai_badge.clone(),
    }
}

fn map_event(e: p::DomainEvent) -> DomainEvent {
    match e {
        p::DomainEvent::Connected {
            desktop_id,
            desktop_name,
            via,
        } => DomainEvent::Connected {
            desktop_id,
            desktop_name,
            via,
        },
        p::DomainEvent::Disconnected { reason } => DomainEvent::Disconnected { reason },
        p::DomainEvent::RepoChanged { repo } => DomainEvent::RepoChanged {
            repo: map_repo(&repo),
        },
        p::DomainEvent::Proposed { item, pending } => DomainEvent::Proposed {
            item: map_item(&item),
            pending,
        },
        p::DomainEvent::QueueChanged { pending } => DomainEvent::QueueChanged { pending },
        p::DomainEvent::Decided {
            id,
            accepted,
            outcome,
            detail,
            source,
            source_name,
            pending,
        } => DomainEvent::Decided {
            id,
            accepted,
            outcome,
            detail,
            source,
            source_name,
            pending,
        },
        p::DomainEvent::Error { message } => DomainEvent::Error { message },
    }
}

fn map_conn(c: &sluice_sync::ConnInfo) -> ConnectionInfo {
    ConnectionInfo {
        desktop_id: c.desktop_id.clone(),
        desktop_name: c.desktop_name.clone(),
        desktop_version: c.desktop_version.clone(),
        via: c.via.clone(),
        session_id: c.session_id.clone(),
        since: c.since,
    }
}

// ---------------------------------------------------------------------------
// Objects
// ---------------------------------------------------------------------------

/// A local clone opened through the embedded gix read path (offline browsing,
/// or a repository that lives on the phone).
struct LocalRepo {
    reader: Arc<dyn GitReader>,
    path: PathBuf,
}

struct Shared {
    client: sluice_sync::Client,
    local: Mutex<Option<Arc<LocalRepo>>>,
    sink: Mutex<Option<Arc<dyn EventSink>>>,
}

impl Shared {
    fn emit(&self, ev: DomainEvent) {
        let sink = self.sink.lock().unwrap().clone();
        if let Some(s) = sink {
            s.on_event(ev);
        }
    }
}

#[derive(uniffi::Object)]
pub struct SluiceSession {
    shared: Arc<Shared>,
}

#[uniffi::export(async_runtime = "tokio")]
impl SluiceSession {
    /// `config_dir`: a private, persistent directory (keys live here — on iOS / Android
    /// the shell should point it at app-private storage; key material is also suitable
    /// for Keychain / Keystore wrapping by the shell). `platform`: "ios" | "android" | ….
    #[uniffi::constructor]
    pub fn new(config_dir: String, platform: String, device_name: Option<String>) -> Arc<Self> {
        let client = sluice_sync::Client::new(Path::new(&config_dir), &platform, env!("CARGO_PKG_VERSION"));
        if let Some(n) = device_name {
            client.set_device_name(&n);
        }
        let shared = Arc::new(Shared {
            client,
            local: Mutex::new(None),
            sink: Mutex::new(None),
        });
        let weak = Arc::downgrade(&shared);
        shared.client.set_sink(Some(Arc::new(move |ev| {
            if let Some(s) = weak.upgrade() {
                s.emit(map_event(ev));
            }
        })));
        Arc::new(Self { shared })
    }

    /// Receive `DomainEvent`s (connection, queue, repo changes). Replaces any previous sink.
    pub fn set_event_sink(&self, sink: Option<Box<dyn EventSink>>) {
        *self.shared.sink.lock().unwrap() = sink.map(Arc::from);
    }

    pub fn device_id(&self) -> String {
        self.shared.client.device_id()
    }

    pub fn device_name(&self) -> String {
        self.shared.client.device_name()
    }

    pub fn set_device_name(&self, name: String) {
        self.shared.client.set_device_name(&name);
    }

    /// Pair with a desktop from the scanned QR text (one-time code) and stay connected.
    pub async fn pair(&self, payload: String) -> FfiResult<ConnectionInfo> {
        let shared = self.shared.clone();
        blocking(move || shared.client.pair(&payload).map(|c| map_conn(&c))).await
    }

    /// Connect to a paired desktop (the only one when `desktop_id` is None):
    /// LAN direct first, relay fallback.
    pub async fn connect(&self, desktop_id: Option<String>) -> FfiResult<ConnectionInfo> {
        let shared = self.shared.clone();
        blocking(move || shared.client.connect(desktop_id.as_deref()).map(|c| map_conn(&c))).await
    }

    pub fn disconnect(&self) {
        self.shared.client.disconnect();
    }

    pub fn is_connected(&self) -> bool {
        self.shared.client.is_connected()
    }

    pub fn connection(&self) -> Option<ConnectionInfo> {
        self.shared.client.connection().map(|c| map_conn(&c))
    }

    pub fn desktops(&self) -> Vec<PairedDesktop> {
        self.shared
            .client
            .desktops()
            .iter()
            .map(|d| PairedDesktop {
                desktop_id: d.desktop_id.clone(),
                desktop_name: d.desktop_name.clone(),
                lan: d.lan.clone(),
                relay: d.relay.clone(),
                paired_at: d.paired_at,
            })
            .collect()
    }

    /// Ask the desktop to forget this device, then drop the pairing locally.
    pub async fn unpair(&self, desktop_id: String) -> FfiResult<()> {
        let shared = self.shared.clone();
        blocking(move || shared.client.unpair(&desktop_id)).await
    }

    /// Open a repository on this device through the embedded gix read path.
    pub async fn open_local(&self, path: String) -> FfiResult<RepoSummary> {
        let shared = self.shared.clone();
        blocking(move || {
            let reader = sluice_backend_gix::GixReader::discover(&path, Console::new())?;
            let info = reader.info()?;
            let local = Arc::new(LocalRepo {
                reader: Arc::new(reader),
                path: PathBuf::from(&path),
            });
            let summary = local_summary(&local, &info);
            *shared.local.lock().unwrap() = Some(local);
            shared.emit(DomainEvent::Connected {
                desktop_id: String::new(),
                desktop_name: info.name.clone(),
                via: "local".into(),
            });
            shared.emit(DomainEvent::RepoChanged {
                repo: summary.clone(),
            });
            Ok(summary)
        })
        .await
    }

    pub fn close_local(&self) {
        *self.shared.local.lock().unwrap() = None;
    }

    /// Repository read model: the desktop mirror when connected, else the local clone.
    pub fn repo(&self) -> Arc<RepoView> {
        Arc::new(RepoView {
            shared: self.shared.clone(),
        })
    }

    pub fn review_queue(&self) -> Arc<ReviewQueue> {
        Arc::new(ReviewQueue {
            shared: self.shared.clone(),
        })
    }
}

fn local_summary(local: &LocalRepo, info: &sluice_core::RepoInfo) -> RepoSummary {
    let head_subject = info
        .head
        .id
        .as_ref()
        .and_then(|id| local.reader.commit_detail(id).ok())
        .map(|d| d.commit.summary.clone())
        .unwrap_or_default();
    RepoSummary {
        path: local.path.to_string_lossy().to_string(),
        name: info.name.clone(),
        branch: info.head.branch.clone().unwrap_or_else(|| "detached HEAD".into()),
        head_short: info
            .head
            .id
            .as_ref()
            .map(|o| o.short(7).to_string())
            .unwrap_or_default(),
        head_subject,
        ahead: info.head.ahead as u32,
        behind: info.head.behind as u32,
        changed_files: 0,
        vcs: if info.vcs.is_jj() { "jj" } else { "git" }.to_string(),
        updated_at: sluice_sync::pairing::now(),
    }
}

/// Log pagination, commit detail and diff retrieval (02 §5.5).
#[derive(uniffi::Object)]
pub struct RepoView {
    shared: Arc<Shared>,
}

#[uniffi::export(async_runtime = "tokio")]
impl RepoView {
    /// Current summary (desktop mirror when connected, otherwise the local clone).
    pub fn summary(&self) -> Option<RepoSummary> {
        if self.shared.client.is_connected() {
            return self.shared.client.cache().repo.as_ref().map(map_repo);
        }
        let local = self.shared.local.lock().unwrap().clone()?;
        let info = local.reader.info().ok()?;
        Some(local_summary(&local, &info))
    }

    pub async fn log(&self, offset: u32, limit: u32) -> FfiResult<LogPage> {
        let shared = self.shared.clone();
        blocking(move || {
            if shared.client.is_connected() {
                let (total, rows) = shared.client.log(offset, limit)?;
                return Ok(LogPage {
                    offset,
                    total,
                    rows: rows.iter().map(map_row).collect(),
                });
            }
            let local = local_or_err(&shared)?;
            let limit = if limit == 0 { 50 } else { limit };
            let query = LogQuery {
                limit: (offset as usize) + (limit as usize) + 1,
                ..Default::default()
            };
            let commits = local.reader.log(&query)?;
            let refs = local.reader.refs().unwrap_or_default();
            let rows = commits
                .iter()
                .skip(offset as usize)
                .take(limit as usize)
                .map(|c| LogRow {
                    oid: c.id.to_string(),
                    short: c.id.short(7).to_string(),
                    subject: c.summary.clone(),
                    author: c.author.name.clone(),
                    at: c.author.time.timestamp(),
                    parents: c.parents.iter().map(|x| x.to_string()).collect(),
                    refs: refs
                        .iter()
                        .filter(|r| r.target == c.id)
                        .map(|r| r.short_name.clone())
                        .collect(),
                    ai_badge: c.agent.is_ai().then(|| c.agent.label().to_string()),
                })
                .collect();
            Ok(LogPage {
                offset,
                total: commits.len() as u32,
                rows,
            })
        })
        .await
    }

    pub async fn commit(&self, oid: String) -> FfiResult<CommitDetail> {
        let shared = self.shared.clone();
        blocking(move || {
            if shared.client.is_connected() {
                return Ok(map_detail(&shared.client.commit(&oid)?));
            }
            let local = local_or_err(&shared)?;
            let id = Oid::new(oid);
            let d = local.reader.commit_detail(&id)?;
            let changes = local.reader.commit_changes(&id).unwrap_or_default();
            let (subject, body) = match d.message.split_once('\n') {
                Some((s, b)) => (s.to_string(), b.trim().to_string()),
                None => (d.message.clone(), String::new()),
            };
            Ok(CommitDetail {
                oid: d.commit.id.to_string(),
                short: d.commit.id.short(7).to_string(),
                subject,
                body,
                author: d.commit.author.name.clone(),
                at: d.commit.author.time.timestamp(),
                parents: d.commit.parents.iter().map(|x| x.to_string()).collect(),
                files: changes
                    .iter()
                    .map(|f| ChangedFile {
                        path: f.path.clone(),
                        status: f.kind.mark().to_string(),
                        additions: f.additions.unwrap_or(0),
                        deletions: f.deletions.unwrap_or(0),
                    })
                    .collect(),
                ai_badge: d.commit.agent.is_ai().then(|| d.commit.agent.label().to_string()),
            })
        })
        .await
    }

    /// Unified diff of `path` in commit `oid` (or of the uncommitted file when `oid` is empty).
    pub async fn diff(&self, oid: String, path: String) -> FfiResult<DiffText> {
        let shared = self.shared.clone();
        blocking(move || {
            if shared.client.is_connected() {
                let (patch, truncated) = shared.client.diff(&oid, &path)?;
                return Ok(DiffText {
                    oid,
                    path,
                    patch,
                    truncated,
                });
            }
            let local = local_or_err(&shared)?;
            let (old, new) = if oid.is_empty() {
                (
                    local.reader.blob(&BlobRev::Head, &path)?.unwrap_or_default(),
                    local.reader.blob(&BlobRev::Worktree, &path)?.unwrap_or_default(),
                )
            } else {
                let id = Oid::new(oid.clone());
                (
                    local
                        .reader
                        .blob(&BlobRev::ParentOf(id.clone()), &path)?
                        .unwrap_or_default(),
                    local
                        .reader
                        .blob(&BlobRev::Commit(id), &path)?
                        .unwrap_or_default(),
                )
            };
            let d = sluice_core::diff::diff_bytes(&old, &new, &sluice_core::diff::DiffOptions::default());
            let (patch, truncated) = unified(&d, &path);
            Ok(DiffText {
                oid,
                path,
                patch,
                truncated,
            })
        })
        .await
    }
}

fn unified(d: &sluice_core::diff::FileDiff, path: &str) -> (String, bool) {
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    if d.binary {
        out.push_str("Binary files differ\n");
        return (out, false);
    }
    for h in &d.hunks {
        out.push_str(&h.header());
        out.push('\n');
        for l in &h.lines {
            out.push(match l.kind {
                sluice_core::diff::LineKind::Context => ' ',
                sluice_core::diff::LineKind::Added => '+',
                sluice_core::diff::LineKind::Deleted => '-',
            });
            out.push_str(&l.text);
            out.push('\n');
            if out.len() > 256 * 1024 {
                out.push_str("… (truncated)\n");
                return (out, true);
            }
        }
    }
    (out, d.truncated)
}

fn local_or_err(shared: &Shared) -> Result<Arc<LocalRepo>, SluiceError> {
    shared
        .local
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| SluiceError::Failed {
            message: "not connected to a desktop and no local repository open".into(),
        })
}

/// Pending proposals and the two human actions (02 §5.5 / 05 §7.1).
#[derive(uniffi::Object)]
pub struct ReviewQueue {
    shared: Arc<Shared>,
}

#[uniffi::export(async_runtime = "tokio")]
impl ReviewQueue {
    /// Cached items (pushed by the desktop on every change).
    pub fn items(&self) -> Vec<ReviewItem> {
        self.shared.client.cache().queue.iter().map(map_item).collect()
    }

    /// Pull a fresh snapshot from the desktop.
    pub async fn refresh(&self) -> FfiResult<Vec<ReviewItem>> {
        let shared = self.shared.clone();
        blocking(move || Ok(shared.client.refresh()?.queue.iter().map(map_item).collect())).await
    }

    /// 放行 — signed with the device key; the desktop executes and reports the outcome.
    pub async fn approve(&self, id: u64, version: String, note: String) -> FfiResult<Decision> {
        self.decide(id, version, true, note).await
    }

    /// 驳回 — signed; the desktop tells the proposing agent.
    pub async fn reject(&self, id: u64, version: String, note: String) -> FfiResult<Decision> {
        self.decide(id, version, false, note).await
    }
}

impl ReviewQueue {
    async fn decide(&self, id: u64, version: String, accept: bool, note: String) -> FfiResult<Decision> {
        let shared = self.shared.clone();
        blocking(move || {
            let d = shared.client.decide(id, &version, accept, &note)?;
            Ok(Decision {
                id: d.id,
                accepted: d.accepted,
                outcome: d.outcome,
                detail: d.detail,
            })
        })
        .await
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> FfiResult<T> {
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r.map_err(SluiceError::from),
        Err(e) => Err(SluiceError::Failed {
            message: format!("task failed: {e}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Collect(Arc<Mutex<Vec<DomainEvent>>>);
    impl EventSink for Collect {
        fn on_event(&self, event: DomainEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    /// The M0 ⑤ smoke the roadmap asks for (04 §1): call repo_status and receive one event.
    #[tokio::test]
    async fn open_local_repo_status_and_one_event() {
        let dir = std::env::temp_dir().join(format!("sluice-ffi-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let session = SluiceSession::new(
            dir.to_string_lossy().to_string(),
            "test".into(),
            Some("ci".into()),
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        session.set_event_sink(Some(Box::new(Collect(events.clone()))));
        // this workspace is a git repository
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let summary = session
            .open_local(root.to_string_lossy().to_string())
            .await
            .unwrap();
        assert!(!summary.head_short.is_empty());
        let view = session.repo();
        let page = view.log(0, 5).await.unwrap();
        assert!(!page.rows.is_empty());
        let detail = view.commit(page.rows[0].oid.clone()).await.unwrap();
        assert_eq!(detail.short, page.rows[0].short);
        let got = events.lock().unwrap().clone();
        assert!(got.iter().any(|e| matches!(e, DomainEvent::RepoChanged { .. })));
        assert!(!session.is_connected());
        assert!(session.review_queue().items().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
