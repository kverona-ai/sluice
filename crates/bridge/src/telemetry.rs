//! Opt-in telemetry + crash log (05 §9): nothing is recorded unless the user
//! enabled it; events are anonymous (no paths, names, messages or diffs) and
//! queued in `~/.sluice/telemetry-queue.jsonl`. They are only sent when an
//! endpoint is configured (`SLUICE_TELEMETRY_ENDPOINT` or settings); the
//! schema is public (this file). Crash reports land in `~/.sluice/crash/`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

fn home() -> PathBuf {
    std::env::var_os("SLUICE_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn queue_path() -> PathBuf {
    home().join(".sluice").join("telemetry-queue.jsonl")
}

pub fn crash_dir() -> PathBuf {
    home().join(".sluice").join("crash")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub name: String,
    /// Unix seconds.
    pub at: i64,
    pub app_version: String,
    pub os: String,
    #[serde(default)]
    pub props: Value,
}

/// Stable anonymous installation id (random, no hardware info).
pub fn install_id() -> String {
    let p = home().join(".sluice").join("install-id");
    if let Ok(id) = std::fs::read_to_string(&p) {
        return id.trim().to_string();
    }
    let id = crate::ipc::repo_key(&std::env::temp_dir().join(format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )));
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&p, &id);
    id
}

/// Record an event when telemetry is enabled. `props` must not carry repository
/// data — callers pass counts / booleans / enum-like strings only.
pub fn record(name: &str, props: Value) {
    if !enabled() {
        return;
    }
    let ev = Event {
        name: name.to_string(),
        at: chrono::Utc::now().timestamp(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        props,
    };
    let p = queue_path();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        use std::io::Write;
        let _ = writeln!(f, "{}", serde_json::to_string(&ev).unwrap_or_default());
    }
}

pub fn queued() -> usize {
    std::fs::read_to_string(queue_path())
        .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

pub fn endpoint(configured: Option<&str>) -> Option<String> {
    std::env::var("SLUICE_TELEMETRY_ENDPOINT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| configured.map(str::to_string).filter(|s| !s.is_empty()))
}

/// POST the queue as a JSON array; on 2xx the queue is cleared. Blocking.
pub fn flush(configured_endpoint: Option<&str>) -> Result<usize> {
    let Some(url) = endpoint(configured_endpoint) else {
        return Ok(0);
    };
    let text = std::fs::read_to_string(queue_path()).unwrap_or_default();
    let events: Vec<Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if events.is_empty() {
        return Ok(0);
    }
    let body = json!({"install_id": install_id(), "events": events});
    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .set("User-Agent", &format!("sluice/{}", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(15))
        .send_string(&body.to_string())
        .context("sending telemetry")?;
    if resp.status() / 100 == 2 {
        let _ = std::fs::write(queue_path(), "");
    }
    Ok(events.len())
}

/// Install a panic hook that writes a crash log locally (always) and queues an
/// anonymous `crash` event (only when telemetry is on).
pub fn install_crash_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let dir = crash_dir();
        let _ = std::fs::create_dir_all(&dir);
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let file = dir.join(format!("crash-{ts}.log"));
        let bt = std::backtrace::Backtrace::force_capture();
        let _ = std::fs::write(
            &file,
            format!("sluice {}\n{}\n\n{}\n", env!("CARGO_PKG_VERSION"), info, bt),
        );
        record(
            "crash",
            json!({"location": info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default()}),
        );
        default(info);
    }));
}
