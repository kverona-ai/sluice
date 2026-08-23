//! Update check (05 §9): GitHub Releases `latest` → compare with the running
//! version. Only the version number leaves the machine; can be turned off.

use anyhow::{Context as _, Result};
use serde::Deserialize;

pub const RELEASES_API: &str = "https://api.github.com/repos/kverona-ai/sluice/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/kverona-ai/sluice/releases";

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub name: String,
    pub html_url: String,
    #[serde(default)]
    pub published_at: String,
}

/// `v1.2.3` / `1.2.3-beta.1` → (1,2,3). Missing parts are 0; pre-release tags are ignored here.
pub fn parse_semver(tag: &str) -> (u64, u64, u64) {
    let core = tag
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or("");
    let mut it = core.split('.').map(|p| p.trim().parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

pub fn is_newer(tag: &str, current: &str) -> bool {
    parse_semver(tag) > parse_semver(current)
}

/// Blocking; call from a background thread. `Ok(None)` when there is no release yet.
pub fn check_latest(current: &str) -> Result<Option<Release>> {
    let resp = ureq::get(RELEASES_API)
        .set("User-Agent", &format!("sluice/{current} (+{RELEASES_PAGE})"))
        .set("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(15))
        .call();
    match resp {
        Ok(r) => {
            let rel: Release = r.into_json().context("parsing release json")?;
            Ok(if is_newer(&rel.tag_name, current) {
                Some(rel)
            } else {
                None
            })
        }
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(is_newer("1.0.0-beta.1", "0.9.9"));
        assert_eq!(parse_semver("v1.2"), (1, 2, 0));
    }
}
