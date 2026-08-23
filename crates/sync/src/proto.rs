//! Wire messages of the sync channel (JSON inside encrypted frames) and the
//! `DomainEvent` type shared with the FFI `EventSink` (02 §5.5). Versioned by
//! `HelloHeader.v`; new fields must be `#[serde(default)]` so phones and desktops
//! can be updated independently.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// Encrypted body of the initiator's first frame.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Hello {
    pub device_name: String,
    pub platform: String,
    #[serde(default)]
    pub app_version: String,
    /// Device static X25519 public key (base64) — persisted by the desktop at pairing.
    pub dh_public: String,
    /// Device Ed25519 verifying key (base64) — decisions are checked against it.
    pub sign_public: String,
}

/// Encrypted body of the desktop's reply: identity + the initial state snapshot.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Welcome {
    pub desktop_id: String,
    pub desktop_name: String,
    #[serde(default)]
    pub desktop_version: String,
    /// Current LAN addresses (`host:port`) — lets a device that came in through the
    /// relay try the direct path next time (02 §5.7 "经中继交换候选地址后直连").
    #[serde(default)]
    pub lan: Vec<String>,
    pub repo: Option<RepoView>,
    #[serde(default)]
    pub queue: Vec<ReviewItem>,
}

/// What the phone shows on its repo card — a read model, never a write path.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RepoView {
    pub path: String,
    pub name: String,
    pub branch: String,
    pub head_short: String,
    pub head_subject: String,
    pub ahead: u32,
    pub behind: u32,
    /// Uncommitted (working + index) entries.
    pub changed_files: u32,
    /// "git" | "jj".
    pub vcs: String,
    /// Unix seconds of the last desktop refresh.
    pub updated_at: i64,
}

/// One entry of the propose-and-confirm queue, as seen from a device (05 §7.1).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ReviewItem {
    pub id: u64,
    /// Which AI client proposed it (e.g. "claude-code").
    pub client: String,
    /// "commit" | "branch" | "push".
    pub kind: String,
    pub title: String,
    /// Commit message / branch details.
    #[serde(default)]
    pub detail: String,
    /// Files the proposal touches (commit proposals).
    #[serde(default)]
    pub files: Vec<String>,
    /// Unified diff excerpt (truncated on the desktop), when available.
    #[serde(default)]
    pub patch: Option<String>,
    pub received_at: i64,
    /// Baseline fingerprint (HEAD + working state) the item was proposed against.
    /// A decision must quote it; if the desktop has moved on, the answer is "expired".
    #[serde(default)]
    pub version: String,
    /// "pending" | "busy".
    #[serde(default)]
    pub state: String,
}

/// Commit-graph skeleton row (02 §5.4 "提交图骨架"), served page by page.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct LogRow {
    pub oid: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub at: i64,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub refs: Vec<String>,
    /// AI provenance badge text when the commit is attributed to an agent session.
    #[serde(default)]
    pub ai_badge: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CommitDetail {
    pub oid: String,
    pub short: String,
    pub subject: String,
    pub body: String,
    pub author: String,
    pub at: i64,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub files: Vec<ChangedFile>,
    #[serde(default)]
    pub ai_badge: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ChangedFile {
    pub path: String,
    /// "A" | "M" | "D" | "R" | "T".
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    Ping,
    GetState,
    /// 放行 (accept) or 驳回 (reject) a queue item. `sig` is the device's Ed25519
    /// signature over `crypto::decision_message(..)` (base64).
    Decide {
        id: u64,
        #[serde(default)]
        version: String,
        accept: bool,
        #[serde(default)]
        note: String,
        #[serde(default)]
        sig: String,
    },
    /// Commit-graph skeleton page.
    Log {
        #[serde(default)]
        offset: u32,
        #[serde(default)]
        limit: u32,
    },
    Commit {
        oid: String,
    },
    /// Diff of one file in a commit (`oid`), or of an uncommitted file when `oid` is
    /// empty; `patch` is unified text.
    Diff {
        #[serde(default)]
        oid: String,
        path: String,
    },
    /// The device asks to be forgotten (desktop removes it from the trust list).
    Unpair,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    Pong,
    /// Pushed whenever the repo or the queue changes, and in reply to `GetState`.
    State {
        repo: Option<RepoView>,
        queue: Vec<ReviewItem>,
    },
    /// Outcome of a `Decide`, after the desktop executed (or refused) it.
    Decided {
        id: u64,
        accepted: bool,
        /// "done" | "expired" | "rejected" | "failed" | "unknown"
        outcome: String,
        detail: String,
    },
    LogPage {
        offset: u32,
        total: u32,
        rows: Vec<LogRow>,
    },
    CommitDetail {
        detail: CommitDetail,
    },
    Diff {
        oid: String,
        path: String,
        patch: String,
        truncated: bool,
    },
    /// Something happened on the desktop — same event type the FFI `EventSink` gets.
    Event {
        event: DomainEvent,
    },
    Error {
        message: String,
    },
    /// The desktop is shutting the channel down (quit / revoked).
    Bye {
        reason: String,
    },
}

/// Serializable domain events (02 §5.5: "载荷为可序列化 DomainEvent"). Emitted by the
/// desktop over the channel and delivered to mobile shells through `EventSink`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DomainEvent {
    /// Channel state: `via` = "lan" | "relay" | "local".
    Connected {
        desktop_id: String,
        desktop_name: String,
        via: String,
    },
    Disconnected {
        reason: String,
    },
    /// Repo read model changed (HEAD / branch / working copy).
    RepoChanged {
        repo: RepoView,
    },
    /// A proposal entered the queue; `pending` is the new count.
    Proposed {
        item: ReviewItem,
        pending: u32,
    },
    /// The queue changed in some other way (refresh the list).
    QueueChanged {
        pending: u32,
    },
    /// A proposal left the queue (decided by any party or expired).
    Decided {
        id: u64,
        accepted: bool,
        outcome: String,
        detail: String,
        /// "desktop" | "device:<id>".
        source: String,
        source_name: String,
        pending: u32,
    },
    /// Non-fatal error worth surfacing.
    Error {
        message: String,
    },
}

/// A paired device as recorded by the desktop (05 §7.1 决定人 / 02 §5.4 放行来源设备).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub platform: String,
}

/// Audit record of a queue decision (05 §7.1 审计字段): appended to
/// `<common-dir>/sluice/audit.log` and shown in Console.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DecisionRecord {
    pub at: i64,
    pub proposal_id: u64,
    pub agent: String,
    pub kind: String,
    pub title: String,
    /// sha256 of the proposal payload (hex, 16 chars).
    #[serde(default)]
    pub payload_hash: String,
    /// "approve" | "reject" | "expire"
    pub decision: String,
    /// "desktop" or "device:<id>".
    pub source: String,
    pub source_name: String,
    #[serde(default)]
    pub note: String,
    /// Execution detail (e.g. "committed abc1234") or the failure reason.
    #[serde(default)]
    pub detail: String,
    /// Resulting commit SHA when the decision produced one.
    #[serde(default)]
    pub commit: String,
}

impl DecisionRecord {
    pub fn source_label(&self) -> String {
        if self.source == "desktop" {
            "desktop".to_string()
        } else {
            format!(
                "{} ({})",
                self.source_name,
                self.source.trim_start_matches("device:")
            )
        }
    }
}
