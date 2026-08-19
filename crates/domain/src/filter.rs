//! Log filtering semantics (05 §4): text matches subject (+ author + hash
//! prefix ≥ 4), regex is Rust `regex` syntax, dimensions AND together,
//! values inside a dimension OR together.

use std::collections::BTreeSet;

use chrono::{DateTime, Duration, Local};
use sluice_core::Commit;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DateFilter {
    #[default]
    Any,
    Today,
    Yesterday,
    ThisWeek,
    ThisMonth,
}

impl DateFilter {
    pub const ALL: [DateFilter; 5] = [
        DateFilter::Any,
        DateFilter::Today,
        DateFilter::Yesterday,
        DateFilter::ThisWeek,
        DateFilter::ThisMonth,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DateFilter::Any => "Date",
            DateFilter::Today => "今天",
            DateFilter::Yesterday => "昨天",
            DateFilter::ThisWeek => "本周",
            DateFilter::ThisMonth => "本月",
        }
    }

    fn contains(self, t: DateTime<Local>, now: DateTime<Local>) -> bool {
        let today = now.date_naive();
        let d = t.date_naive();
        match self {
            DateFilter::Any => true,
            DateFilter::Today => d == today,
            DateFilter::Yesterday => d == today - Duration::days(1),
            DateFilter::ThisWeek => d > today - Duration::days(7),
            DateFilter::ThisMonth => d.year() == today.year() && d.month() == today.month(),
        }
    }
}

use chrono::Datelike;

#[derive(Clone, Debug, Default)]
pub struct LogFilter {
    pub text: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub authors: BTreeSet<String>,
    pub date: DateFilter,
    pub ai_only: bool,
}

impl LogFilter {
    pub fn is_active(&self) -> bool {
        !self.text.trim().is_empty()
            || !self.authors.is_empty()
            || self.date != DateFilter::Any
            || self.ai_only
    }

    /// Indices of commits that pass the filter (all indices when inactive).
    pub fn apply(&self, commits: &[Commit]) -> Vec<usize> {
        if !self.is_active() {
            return (0..commits.len()).collect();
        }
        let now = Local::now();
        let text = self.text.trim();
        let re = if self.regex && !text.is_empty() {
            regex::RegexBuilder::new(text)
                .case_insensitive(!self.case_sensitive)
                .build()
                .ok()
        } else {
            None
        };
        let needle = if self.case_sensitive {
            text.to_string()
        } else {
            text.to_lowercase()
        };
        let is_hash = text.len() >= 4 && text.chars().all(|c| c.is_ascii_hexdigit());
        commits
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                if self.ai_only && !c.agent.is_ai() {
                    return false;
                }
                if !self.authors.is_empty() && !self.authors.contains(&c.author.name) {
                    return false;
                }
                if !self.date.contains(c.author.time.with_timezone(&Local), now) {
                    return false;
                }
                if text.is_empty() {
                    return true;
                }
                if is_hash && c.id.as_str().starts_with(&text.to_lowercase()) {
                    return true;
                }
                match &re {
                    Some(re) => re.is_match(&c.summary) || re.is_match(&c.author.name),
                    None => {
                        let hay = format!("{} {}", c.summary, c.author.name);
                        let hay = if self.case_sensitive {
                            hay
                        } else {
                            hay.to_lowercase()
                        };
                        hay.contains(&needle)
                    }
                }
            })
            .map(|(i, _)| i)
            .collect()
    }
}
