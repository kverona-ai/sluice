//! Integration smoke test for the gix read path against a repository built with
//! the system `git` (skipped when git is unavailable).

use std::path::PathBuf;
use std::process::Command;

use sluice_backend_gix::GixReader;
use sluice_core::*;

fn git(dir: &PathBuf, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "will")
        .env("GIT_AUTHOR_EMAIL", "will@example.com")
        .env("GIT_COMMITTER_NAME", "will")
        .env("GIT_COMMITTER_EMAIL", "will@example.com")
        .env("GIT_AUTHOR_DATE", "@1755600000 +0800")
        .env("GIT_COMMITTER_DATE", "@1755600000 +0800")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn make_repo() -> Option<PathBuf> {
    if Command::new("git").arg("--version").output().is_err() {
        return None;
    }
    let dir = std::env::temp_dir().join(format!("sluice-gix-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    assert!(git(&dir, &["init", "-q", "-b", "main"]));
    std::fs::write(dir.join("a.txt"), "one\ntwo\n").ok()?;
    assert!(git(&dir, &["add", "-A"]));
    assert!(git(
        &dir,
        &[
            "commit",
            "-q",
            "-m",
            "feat: first\n\nCo-Authored-By: Claude <noreply@anthropic.com>"
        ]
    ));
    assert!(git(&dir, &["checkout", "-q", "-b", "topic"]));
    std::fs::write(dir.join("b.txt"), "hello\n").ok()?;
    assert!(git(&dir, &["add", "-A"]));
    assert!(git(&dir, &["commit", "-q", "-m", "feat: topic work"]));
    assert!(git(&dir, &["checkout", "-q", "main"]));
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").ok()?;
    assert!(git(&dir, &["commit", "-q", "-am", "fix: extend a"]));
    assert!(git(
        &dir,
        &["merge", "-q", "--no-ff", "topic", "-m", "merge: topic"]
    ));
    assert!(git(&dir, &["tag", "v0.0.1"]));
    Some(dir)
}

#[test]
fn refs_log_and_changes() {
    let Some(dir) = make_repo() else {
        eprintln!("git not available; skipping");
        return;
    };
    let reader = GixReader::discover(&dir, Console::new()).expect("discover");
    let info = reader.info().expect("info");
    assert_eq!(info.head.branch.as_deref(), Some("main"));
    assert!(!info.is_bare);

    let refs = reader.refs().expect("refs");
    let names: Vec<&str> = refs.iter().map(|r| r.short_name.as_str()).collect();
    assert!(names.contains(&"main"), "{names:?}");
    assert!(names.contains(&"topic"), "{names:?}");
    assert!(names.contains(&"v0.0.1"), "{names:?}");
    assert!(refs.iter().any(|r| r.is_head && r.short_name == "main"));

    let log = reader.log(&LogQuery::default()).expect("log");
    assert_eq!(log.len(), 4);
    assert_eq!(log[0].summary, "merge: topic");
    assert!(log[0].is_merge());
    // children never before parents
    let pos = |s: &str| log.iter().position(|c| c.summary == s).unwrap();
    assert!(pos("merge: topic") < pos("fix: extend a"));
    assert!(pos("fix: extend a") < pos("feat: first"));
    assert!(pos("feat: topic work") < pos("feat: first"));
    let first = log.iter().find(|c| c.summary == "feat: first").unwrap();
    assert_eq!(first.agent, Agent::ClaudeCode);

    let detail = reader.commit_detail(&first.id).expect("detail");
    assert!(
        detail
            .trailers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("co-authored-by"))
    );

    let changes = reader
        .commit_changes(&log[pos("fix: extend a")].id)
        .expect("changes");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "a.txt");
    assert_eq!(changes[0].kind, ChangeKind::Modified);
    assert_eq!(changes[0].additions, Some(1));
    assert_eq!(changes[0].deletions, Some(0));

    let root_changes = reader.commit_changes(&first.id).expect("root changes");
    assert_eq!(root_changes[0].kind, ChangeKind::Added);
    assert_eq!(root_changes[0].additions, Some(2));

    let fix = &log[pos("fix: extend a")];
    let new = reader
        .blob(&BlobRev::Commit(fix.id.clone()), "a.txt")
        .unwrap()
        .unwrap();
    let old = reader
        .blob(&BlobRev::ParentOf(fix.id.clone()), "a.txt")
        .unwrap()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&new), "one\ntwo\nthree\n");
    assert_eq!(String::from_utf8_lossy(&old), "one\ntwo\n");
    assert!(reader.blob(&BlobRev::Head, "nope.txt").unwrap().is_none());
    assert_eq!(
        reader.blob(&BlobRev::Index, "b.txt").unwrap().unwrap(),
        b"hello\n"
    );
    assert_eq!(
        reader.blob(&BlobRev::Worktree, "b.txt").unwrap().unwrap(),
        b"hello\n"
    );
    assert!(reader.console().revision() > 0);

    let _ = std::fs::remove_dir_all(&dir);
}
