//! Local IPC between the desktop app and `sluice mcp serve` / `sluice hook` /
//! `sluice askpass` helpers (05 §7.2). Loopback TCP + per-instance token file:
//! `~/.sluice/ipc/<repo-key>.json` = { port, token, pid, repo }. Requests and
//! responses are newline-delimited JSON. The app never executes a proposal
//! until a human accepts it in the queue (03 §3).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How long a proposal waits for a human decision before the caller is told "pending".
pub const DECISION_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub repo: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProposalKind {
    /// Stage `paths` (all tracked changes when None) and commit with `message`.
    Commit {
        message: String,
        paths: Option<Vec<String>>,
    },
    Branch {
        name: String,
        checkout: bool,
    },
    Push {
        set_upstream: bool,
    },
}

impl ProposalKind {
    pub fn title(&self) -> String {
        match self {
            ProposalKind::Commit { message, paths } => format!(
                "提交 {}{}",
                message.lines().next().unwrap_or(""),
                paths
                    .as_ref()
                    .map(|p| format!("（{} 个文件）", p.len()))
                    .unwrap_or_else(|| "（全部变更）".into())
            ),
            ProposalKind::Branch { name, checkout } => {
                format!("新建分支 {name}{}", if *checkout { " 并切换" } else { "" })
            }
            ProposalKind::Push { set_upstream } => {
                format!(
                    "推送到远端{}",
                    if *set_upstream {
                        "（-u 设置 upstream）"
                    } else {
                        ""
                    }
                )
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Decision {
    Accepted { detail: String },
    Rejected { reason: String },
    Timeout,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proposal {
    pub id: u64,
    pub client: String,
    pub kind: ProposalKind,
    /// Unix seconds.
    pub received_at: i64,
}

/// What the app receives from the IPC thread.
pub enum Inbound {
    Proposal {
        proposal: Proposal,
        reply: mpsc::Sender<Decision>,
    },
    Askpass {
        prompt: String,
        reply: mpsc::Sender<Option<String>>,
    },
}

#[derive(Serialize, Deserialize)]
struct Request {
    token: String,
    id: u64,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize, Deserialize)]
struct Response {
    id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn home() -> PathBuf {
    std::env::var_os("SLUICE_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn endpoint_dir() -> PathBuf {
    home().join(".sluice").join("ipc")
}

pub fn repo_key(repo: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let canon = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    canon.to_string_lossy().to_lowercase().hash(&mut h);
    format!("{:016x}", h.finish())
}

fn endpoint_file(repo: &Path) -> PathBuf {
    endpoint_dir().join(format!("{}.json", repo_key(repo)))
}

fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for _ in 0..4 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

pub fn find_endpoint(repo: &Path) -> Option<Endpoint> {
    let text = std::fs::read_to_string(endpoint_file(repo)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn remove_endpoint(repo: &Path) {
    let _ = std::fs::remove_file(endpoint_file(repo));
}

/// Start the app-side server. Blocking accept loop on its own thread; each request
/// is handed to the app over `tx` with a reply channel, and the connection waits for
/// the human decision (up to [`DECISION_TIMEOUT`]).
pub fn serve(repo: PathBuf, tx: async_channel::Sender<Inbound>) -> Result<Endpoint> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("binding loopback IPC port")?;
    let port = listener.local_addr()?.port();
    let ep = Endpoint {
        port,
        token: random_token(),
        pid: std::process::id(),
        repo: repo.clone(),
    };
    std::fs::create_dir_all(endpoint_dir())?;
    let file = endpoint_file(&repo);
    std::fs::write(&file, serde_json::to_string_pretty(&ep)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    }
    let token = ep.token.clone();
    std::thread::Builder::new()
        .name("sluice-ipc".into())
        .spawn(move || {
            for (n, stream) in listener.incoming().flatten().enumerate() {
                let tx = tx.clone();
                let token = token.clone();
                let id = n as u64 + 1;
                std::thread::spawn(move || {
                    if let Err(e) = handle_conn(stream, &token, id, tx) {
                        tracing::debug!("ipc conn: {e:#}");
                    }
                });
            }
        })?;
    Ok(ep)
}

fn handle_conn(stream: TcpStream, token: &str, id: u64, tx: async_channel::Sender<Inbound>) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let req: Request = serde_json::from_str(line.trim()).context("bad ipc request")?;
    let mut out = stream;
    let respond = |out: &mut TcpStream, resp: Response| -> Result<()> {
        serde_json::to_writer(&mut *out, &resp)?;
        out.write_all(b"\n")?;
        out.flush()?;
        Ok(())
    };
    if req.token != token {
        return respond(
            &mut out,
            Response {
                id: req.id,
                result: None,
                error: Some("bad token".into()),
            },
        );
    }
    match req.method.as_str() {
        "ping" => respond(
            &mut out,
            Response {
                id: req.id,
                result: Some(json!({"pong": true, "pid": std::process::id()})),
                error: None,
            },
        ),
        "propose" => {
            let kind: ProposalKind =
                serde_json::from_value(req.params.get("proposal").cloned().unwrap_or(Value::Null))
                    .context("bad proposal")?;
            let client = req
                .params
                .get("client")
                .and_then(Value::as_str)
                .unwrap_or("mcp client")
                .to_string();
            let proposal = Proposal {
                id,
                client,
                kind,
                received_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            };
            let (reply_tx, reply_rx) = mpsc::channel();
            tx.send_blocking(Inbound::Proposal {
                proposal,
                reply: reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("app gone"))?;
            let decision = reply_rx
                .recv_timeout(DECISION_TIMEOUT)
                .unwrap_or(Decision::Timeout);
            respond(
                &mut out,
                Response {
                    id: req.id,
                    result: Some(serde_json::to_value(decision)?),
                    error: None,
                },
            )
        }
        "askpass" => {
            let prompt = req
                .params
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let (reply_tx, reply_rx) = mpsc::channel();
            tx.send_blocking(Inbound::Askpass {
                prompt,
                reply: reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("app gone"))?;
            let secret = reply_rx.recv_timeout(Duration::from_secs(300)).ok().flatten();
            respond(
                &mut out,
                Response {
                    id: req.id,
                    result: Some(json!({"secret": secret})),
                    error: None,
                },
            )
        }
        other => respond(
            &mut out,
            Response {
                id: req.id,
                result: None,
                error: Some(format!("unknown method {other}")),
            },
        ),
    }
}

// ----- client side -----------------------------------------------------------

fn call(ep: &Endpoint, method: &str, params: Value, timeout: Duration) -> Result<Value> {
    let stream =
        TcpStream::connect_timeout(&format!("127.0.0.1:{}", ep.port).parse()?, Duration::from_secs(3))
            .context("Sluice desktop is not running for this repository")?;
    stream.set_read_timeout(Some(timeout))?;
    let mut out = stream.try_clone()?;
    let req = Request {
        token: ep.token.clone(),
        id: 1,
        method: method.into(),
        params,
    };
    serde_json::to_writer(&mut out, &req)?;
    out.write_all(b"\n")?;
    out.flush()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let resp: Response = serde_json::from_str(line.trim()).context("bad ipc response")?;
    if let Some(e) = resp.error {
        bail!("{e}");
    }
    Ok(resp.result.unwrap_or(Value::Null))
}

/// Is a desktop instance reachable for this repository?
pub fn ping(repo: &Path) -> bool {
    find_endpoint(repo)
        .map(|ep| call(&ep, "ping", json!({}), Duration::from_secs(3)).is_ok())
        .unwrap_or(false)
}

/// Send a proposal and wait for the human decision (or timeout).
pub fn propose(repo: &Path, kind: ProposalKind, client: &str) -> Result<Decision> {
    let ep =
        find_endpoint(repo).context("Sluice desktop is not running for this repository (open it first)")?;
    let v = call(
        &ep,
        "propose",
        json!({"proposal": kind, "client": client}),
        DECISION_TIMEOUT + Duration::from_secs(5),
    )?;
    Ok(serde_json::from_value(v)?)
}

/// Ask the desktop to prompt the user for a secret (GIT_ASKPASS helper).
pub fn askpass(repo: &Path, prompt: &str) -> Result<Option<String>> {
    let ep = find_endpoint(repo).context("Sluice desktop is not running")?;
    let v = call(
        &ep,
        "askpass",
        json!({"prompt": prompt}),
        Duration::from_secs(305),
    )?;
    Ok(v.get("secret").and_then(Value::as_str).map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_proposal_over_loopback() {
        let dir = std::env::temp_dir().join(format!("sluice-ipc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: single-threaded test setup; only this test reads the variable.
        unsafe { std::env::set_var("SLUICE_CONFIG_HOME", &dir) };
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let (tx, rx) = async_channel::unbounded();
        let ep = serve(repo.clone(), tx).unwrap();
        assert!(ep.port > 0);
        std::thread::spawn(move || {
            if let Ok(Inbound::Proposal { proposal, reply }) = rx.recv_blocking() {
                assert_eq!(proposal.client, "test");
                reply.send(Decision::Accepted { detail: "ok".into() }).unwrap();
            }
        });
        assert!(ping(&repo));
        let d = propose(&repo, ProposalKind::Push { set_upstream: false }, "test").unwrap();
        assert_eq!(d, Decision::Accepted { detail: "ok".into() });
        remove_endpoint(&repo);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
