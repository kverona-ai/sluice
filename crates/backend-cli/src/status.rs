//! `git status --porcelain=v2 -z --branch` parser.

use sluice_core::*;

fn kind_of(c: u8) -> Option<ChangeKind> {
    match c {
        b'M' => Some(ChangeKind::Modified),
        b'A' => Some(ChangeKind::Added),
        b'D' => Some(ChangeKind::Deleted),
        b'R' => Some(ChangeKind::Renamed),
        b'C' => Some(ChangeKind::Copied),
        b'T' => Some(ChangeKind::TypeChanged),
        _ => None,
    }
}

pub fn parse_porcelain_v2(data: &[u8]) -> WorkStatus {
    let mut st = WorkStatus::default();
    let mut fields = data.split(|b| *b == 0).peekable();
    while let Some(rec) = fields.next() {
        if rec.is_empty() {
            continue;
        }
        let line = String::from_utf8_lossy(rec).into_owned();
        let mut parts = line.splitn(2, ' ');
        let tag = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        match tag {
            "#" => {
                if let Some(v) = rest.strip_prefix("branch.head ") {
                    if v != "(detached)" {
                        st.branch = Some(v.to_string());
                    }
                } else if let Some(v) = rest.strip_prefix("branch.upstream ") {
                    st.upstream = Some(v.to_string());
                } else if let Some(v) = rest.strip_prefix("branch.ab ") {
                    for tok in v.split_whitespace() {
                        if let Some(a) = tok.strip_prefix('+') {
                            st.ahead = a.parse().unwrap_or(0);
                        } else if let Some(b) = tok.strip_prefix('-') {
                            st.behind = b.parse().unwrap_or(0);
                        }
                    }
                }
            }
            "1" => {
                // XY sub mH mI mW hH hI path
                let cols: Vec<&str> = rest.splitn(8, ' ').collect();
                if cols.len() < 8 {
                    continue;
                }
                let xy = cols[0].as_bytes();
                st.entries.push(StatusEntry {
                    path: cols[7].to_string(),
                    old_path: None,
                    staged: kind_of(xy[0]),
                    unstaged: kind_of(xy[1]),
                    untracked: false,
                    conflict: None,
                    submodule: cols[1].starts_with('S'),
                });
            }
            "2" => {
                // XY sub mH mI mW hH hI Xscore path  \0 origPath
                let cols: Vec<&str> = rest.splitn(9, ' ').collect();
                if cols.len() < 9 {
                    continue;
                }
                let xy = cols[0].as_bytes();
                let orig = fields.next().map(|b| String::from_utf8_lossy(b).into_owned());
                st.entries.push(StatusEntry {
                    path: cols[8].to_string(),
                    old_path: orig,
                    staged: kind_of(xy[0]),
                    unstaged: kind_of(xy[1]),
                    untracked: false,
                    conflict: None,
                    submodule: cols[1].starts_with('S'),
                });
            }
            "u" => {
                // XY sub m1 m2 m3 mW h1 h2 h3 path
                let cols: Vec<&str> = rest.splitn(10, ' ').collect();
                if cols.len() < 10 {
                    continue;
                }
                st.entries.push(StatusEntry {
                    path: cols[9].to_string(),
                    old_path: None,
                    staged: None,
                    unstaged: None,
                    untracked: false,
                    conflict: Some(cols[0].to_string()),
                    submodule: cols[1].starts_with('S'),
                });
            }
            "?" => st.entries.push(StatusEntry {
                path: rest.to_string(),
                old_path: None,
                staged: None,
                unstaged: None,
                untracked: true,
                conflict: None,
                submodule: false,
            }),
            _ => {} // "!" ignored entries are not requested
        }
    }
    st.entries.sort_by(|a, b| a.path.cmp(&b.path));
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_and_entries() {
        let data =
            b"# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaaa bbbb src/lib.rs\0\
1 A. N... 000000 100644 100644 0000 cccc new.rs\0\
2 R. N... 100644 100644 100644 dddd dddd R100 new_name.rs\0old_name.rs\0\
u UU N... 100644 100644 100644 100644 e1 e2 e3 conflict.rs\0\
? untracked.txt\0";
        let st = parse_porcelain_v2(data);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert_eq!(st.entries.len(), 5);
        let by = |p: &str| st.entries.iter().find(|e| e.path == p).unwrap();
        assert_eq!(by("src/lib.rs").unstaged, Some(ChangeKind::Modified));
        assert_eq!(by("src/lib.rs").staged, None);
        assert_eq!(by("new.rs").staged, Some(ChangeKind::Added));
        assert_eq!(by("new_name.rs").old_path.as_deref(), Some("old_name.rs"));
        assert_eq!(by("new_name.rs").staged, Some(ChangeKind::Renamed));
        assert_eq!(by("conflict.rs").conflict.as_deref(), Some("UU"));
        assert!(by("untracked.txt").untracked);
        assert_eq!(st.staged().count(), 2);
        assert_eq!(st.unstaged().count(), 1);
        assert_eq!(st.untracked().count(), 1);
        assert_eq!(st.conflicted().count(), 1);
    }
}
