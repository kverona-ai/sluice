//! Console log (05 §4): every git CLI invocation (command level) and, when
//! verbose, the git-equivalent of gix reads. Shared by backends and the UI.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsoleKind {
    /// gix read path — shown only in verbose mode.
    Read,
    /// git CLI write / network command.
    Write,
    /// AI tool bridge / MCP traffic.
    Ai,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsoleEntry {
    pub at: DateTime<Local>,
    pub kind: ConsoleKind,
    pub command: String,
    pub duration_ms: u128,
    pub exit_code: Option<i32>,
    pub summary: String,
    pub stderr: String,
}

const MAX_ENTRIES: usize = 2_000;

#[derive(Clone, Default)]
pub struct Console {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    entries: VecDeque<ConsoleEntry>,
    revision: u64,
}

impl Console {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn log(&self, entry: ConsoleEntry) {
        let mut g = self.inner.lock().unwrap();
        if g.entries.len() >= MAX_ENTRIES {
            g.entries.pop_front();
        }
        g.entries.push_back(entry);
        g.revision += 1;
    }

    pub fn read(&self, command: impl Into<String>, elapsed: Duration, summary: impl Into<String>) {
        self.log(ConsoleEntry {
            at: Local::now(),
            kind: ConsoleKind::Read,
            command: command.into(),
            duration_ms: elapsed.as_millis(),
            exit_code: Some(0),
            summary: summary.into(),
            stderr: String::new(),
        });
    }

    /// Monotonic counter the UI can poll to know whether to re-render.
    /// App-level AI/bridge note (not a git command), e.g. "ai connect".
    pub fn note(&self, what: impl Into<String>, detail: impl Into<String>) {
        self.log(ConsoleEntry {
            at: Local::now(),
            kind: ConsoleKind::Ai,
            command: what.into(),
            duration_ms: 0,
            exit_code: Some(0),
            summary: detail.into(),
            stderr: String::new(),
        });
    }

    pub fn revision(&self) -> u64 {
        self.inner.lock().unwrap().revision
    }

    pub fn entries(&self) -> Vec<ConsoleEntry> {
        self.inner.lock().unwrap().entries.iter().cloned().collect()
    }

    pub fn clear(&self) {
        let mut g = self.inner.lock().unwrap();
        g.entries.clear();
        g.revision += 1;
    }
}
