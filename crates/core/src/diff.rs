//! Text diff engine shared by the desktop and mobile shells (05 §4): line diff
//! via imara-diff (Histogram), hunk building with configurable context,
//! intra-line word highlighting, side-by-side pairing, and a unified-patch
//! writer used for line-level staging (`git apply --cached --unidiff-zero`).

use std::ops::Range;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    Context,
    Added,
    Deleted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: LineKind,
    /// 1-based line number in the old file (None for added lines).
    pub old_no: Option<u32>,
    /// 1-based line number in the new file (None for deleted lines).
    pub new_no: Option<u32>,
    /// Line text without the trailing newline.
    pub text: String,
    /// Byte ranges inside `text` that differ from the paired line (word-level highlight).
    pub highlights: Vec<Range<usize>>,
    /// Whether this line lacked a trailing newline (last line of a file).
    pub no_newline: bool,
}

/// Syntax token class (filled by the syntax crate; `Plain` never stored).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyntaxKind {
    Plain,
    Keyword,
    String,
    Comment,
    Number,
    Function,
    Type,
    Variable,
    Property,
    Constant,
    Operator,
    Punct,
    Attribute,
    Tag,
}

/// Byte range inside one line with its token class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxSpan {
    pub start: u32,
    pub end: u32,
    pub kind: SyntaxKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    pub fn header(&self) -> String {
        format!(
            "@@ -{},{} +{},{} @@",
            self.old_start, self.old_lines, self.new_start, self.new_lines
        )
    }
    pub fn additions(&self) -> usize {
        self.lines.iter().filter(|l| l.kind == LineKind::Added).count()
    }
    pub fn deletions(&self) -> usize {
        self.lines.iter().filter(|l| l.kind == LineKind::Deleted).count()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
    pub old_lines: u32,
    pub new_lines: u32,
    pub truncated: bool,
    /// Per-line syntax spans for the old / new side (index = line number − 1); empty when
    /// no grammar matched or the file was too large.
    #[serde(default)]
    pub syntax_old: Vec<Vec<SyntaxSpan>>,
    #[serde(default)]
    pub syntax_new: Vec<Vec<SyntaxSpan>>,
}

impl FileDiff {
    pub fn additions(&self) -> usize {
        self.hunks.iter().map(Hunk::additions).sum()
    }
    pub fn deletions(&self) -> usize {
        self.hunks.iter().map(Hunk::deletions).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DiffOptions {
    pub context: usize,
    pub ignore_whitespace: bool,
    /// Above this many lines per side the word-level pass is skipped.
    pub word_diff_max_lines: usize,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            context: 3,
            ignore_whitespace: false,
            word_diff_max_lines: 5_000,
        }
    }
}

/// True when the first 8000 bytes contain NUL (git's heuristic).
pub fn is_binary(data: &[u8]) -> bool {
    data.iter().take(8000).any(|b| *b == 0)
}

fn split_lines(data: &[u8]) -> (Vec<String>, bool) {
    let text = String::from_utf8_lossy(data);
    if text.is_empty() {
        return (Vec::new(), true);
    }
    let ends_with_newline = text.ends_with('\n');
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|s| s.strip_suffix('\r').unwrap_or(s).to_string())
        .collect();
    if ends_with_newline {
        lines.pop();
    }
    (lines, ends_with_newline)
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct Collector {
    changes: Vec<(Range<u32>, Range<u32>)>,
}

impl imara_diff::Sink for Collector {
    type Out = Vec<(Range<u32>, Range<u32>)>;
    fn process_change(&mut self, before: Range<u32>, after: Range<u32>) {
        self.changes.push((before, after));
    }
    fn finish(self) -> Self::Out {
        self.changes
    }
}

/// Line diff of two blobs, built into hunks with `opts.context` lines of context.
pub fn diff_bytes(old: &[u8], new: &[u8], opts: &DiffOptions) -> FileDiff {
    if is_binary(old) || is_binary(new) {
        return FileDiff {
            binary: true,
            ..Default::default()
        };
    }
    let (old_lines, old_nl) = split_lines(old);
    let (new_lines, new_nl) = split_lines(new);
    let changes = {
        let (a, b): (Vec<String>, Vec<String>) = if opts.ignore_whitespace {
            (
                old_lines.iter().map(|l| normalize_ws(l)).collect(),
                new_lines.iter().map(|l| normalize_ws(l)).collect(),
            )
        } else {
            (old_lines.clone(), new_lines.clone())
        };
        let a_refs: Vec<&str> = a.iter().map(String::as_str).collect();
        let b_refs: Vec<&str> = b.iter().map(String::as_str).collect();
        let input = imara_diff::intern::InternedInput::new(Slice(&a_refs), Slice(&b_refs));
        imara_diff::diff(
            imara_diff::Algorithm::Histogram,
            &input,
            Collector { changes: Vec::new() },
        )
    };
    let mut file = FileDiff {
        old_lines: old_lines.len() as u32,
        new_lines: new_lines.len() as u32,
        ..Default::default()
    };
    if changes.is_empty() {
        return file;
    }

    // Group changes into hunks separated by more than 2*context unchanged lines.
    let ctx = opts.context as u32;
    let mut groups: Vec<Vec<(Range<u32>, Range<u32>)>> = Vec::new();
    for ch in changes {
        match groups.last_mut() {
            Some(g) if ch.0.start.saturating_sub(g.last().unwrap().0.end) <= ctx * 2 => g.push(ch),
            _ => groups.push(vec![ch]),
        }
    }
    for group in groups {
        let first = &group[0];
        let last = group.last().unwrap();
        let old_from = first.0.start.saturating_sub(ctx);
        let new_from = first.1.start.saturating_sub(ctx);
        let old_to = (last.0.end + ctx).min(old_lines.len() as u32);
        let new_to = (last.1.end + ctx).min(new_lines.len() as u32);
        let mut lines = Vec::new();
        let (mut o, mut n) = (old_from, new_from);
        for (before, after) in &group {
            // context before the change
            while o < before.start {
                lines.push(ctx_line(&old_lines, o, n));
                o += 1;
                n += 1;
            }
            let dels: Vec<DiffLine> = (before.start..before.end)
                .map(|i| DiffLine {
                    kind: LineKind::Deleted,
                    old_no: Some(i + 1),
                    new_no: None,
                    text: old_lines[i as usize].clone(),
                    highlights: Vec::new(),
                    no_newline: !old_nl && i as usize + 1 == old_lines.len(),
                })
                .collect();
            let adds: Vec<DiffLine> = (after.start..after.end)
                .map(|i| DiffLine {
                    kind: LineKind::Added,
                    old_no: None,
                    new_no: Some(i + 1),
                    text: new_lines[i as usize].clone(),
                    highlights: Vec::new(),
                    no_newline: !new_nl && i as usize + 1 == new_lines.len(),
                })
                .collect();
            let (dels, adds) = if dels.len() + adds.len() <= opts.word_diff_max_lines {
                word_highlight(dels, adds)
            } else {
                (dels, adds)
            };
            lines.extend(dels);
            lines.extend(adds);
            o = before.end;
            n = after.end;
        }
        while o < old_to && n < new_to {
            lines.push(ctx_line(&old_lines, o, n));
            o += 1;
            n += 1;
        }
        file.hunks.push(Hunk {
            old_start: if old_to > old_from { old_from + 1 } else { old_from },
            old_lines: old_to - old_from,
            new_start: if new_to > new_from { new_from + 1 } else { new_from },
            new_lines: new_to - new_from,
            lines,
        });
    }
    file
}

fn ctx_line(old_lines: &[String], o: u32, n: u32) -> DiffLine {
    DiffLine {
        kind: LineKind::Context,
        old_no: Some(o + 1),
        new_no: Some(n + 1),
        text: old_lines[o as usize].clone(),
        highlights: Vec::new(),
        no_newline: false,
    }
}

/// Token source over a slice of lines (imara-diff's built-in `&str` source splits on newlines).
struct Slice<'a>(&'a [&'a str]);

impl<'a> imara_diff::intern::TokenSource for Slice<'a> {
    type Token = &'a str;
    type Tokenizer = std::iter::Copied<std::slice::Iter<'a, &'a str>>;
    fn tokenize(&self) -> Self::Tokenizer {
        self.0.iter().copied()
    }
    fn estimate_tokens(&self) -> u32 {
        self.0.len() as u32
    }
}

/// Split a line into word / punctuation / whitespace tokens, keeping byte offsets.
fn tokens(s: &str) -> Vec<(Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    let kind_of = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let mut last_kind: Option<u8> = None;
    for (i, c) in s.char_indices() {
        let k = kind_of(c);
        match last_kind {
            Some(lk) if lk == k && k != 2 => {}
            _ => {
                if i > start {
                    out.push((start..i, &s[start..i]));
                }
                start = i;
            }
        }
        last_kind = Some(k);
    }
    if start < s.len() {
        out.push((start..s.len(), &s[start..]));
    }
    out
}

/// Pair deleted and added lines positionally and compute word-level highlights
/// (only when the pair is similar enough to be a modification, 05 §4: ratio ≤ 1:3).
fn word_highlight(mut dels: Vec<DiffLine>, mut adds: Vec<DiffLine>) -> (Vec<DiffLine>, Vec<DiffLine>) {
    let n = dels.len().min(adds.len());
    if n == 0 || dels.len() > adds.len() * 3 || adds.len() > dels.len() * 3 {
        return (dels, adds);
    }
    for i in 0..n {
        let (ra, rb) = word_diff(&dels[i].text, &adds[i].text);
        dels[i].highlights = ra;
        adds[i].highlights = rb;
    }
    (dels, adds)
}

/// Byte ranges of `a` and `b` that are not part of their token-level common subsequence.
pub fn word_diff(a: &str, b: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let ta = tokens(a);
    let tb = tokens(b);
    if ta.is_empty() || tb.is_empty() || ta.len() * tb.len() > 40_000 {
        #[allow(clippy::single_range_in_vec_init)]
        return (vec![0..a.len()], vec![0..b.len()]);
    }
    // LCS table on tokens (lines are short; O(n·m) is fine under the guard above).
    let (n, m) = (ta.len(), tb.len());
    let mut dp = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if ta[i].1 == tb[j].1 {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut keep_a = vec![false; n];
    let mut keep_b = vec![false; m];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if ta[i].1 == tb[j].1 {
            keep_a[i] = true;
            keep_b[j] = true;
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let collect = |toks: &[(Range<usize>, &str)], keep: &[bool]| -> Vec<Range<usize>> {
        let mut out: Vec<Range<usize>> = Vec::new();
        for (t, k) in toks.iter().zip(keep) {
            if *k {
                continue;
            }
            match out.last_mut() {
                Some(last) if last.end == t.0.start => last.end = t.0.end,
                _ => out.push(t.0.clone()),
            }
        }
        out
    };
    (collect(&ta, &keep_a), collect(&tb, &keep_b))
}

/// One visual row of a side-by-side view.
#[derive(Clone, Debug)]
pub struct SideBySideRow {
    pub left: Option<usize>,
    pub right: Option<usize>,
}

/// Pair the lines of a hunk for side-by-side rendering: context lines share a
/// row; runs of deletions and additions are zipped positionally.
pub fn side_by_side(hunk: &Hunk) -> Vec<SideBySideRow> {
    let mut rows = Vec::with_capacity(hunk.lines.len());
    let lines = &hunk.lines;
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            LineKind::Context => {
                rows.push(SideBySideRow {
                    left: Some(i),
                    right: Some(i),
                });
                i += 1;
            }
            _ => {
                let mut dels = Vec::new();
                let mut adds = Vec::new();
                while i < lines.len() && lines[i].kind == LineKind::Deleted {
                    dels.push(i);
                    i += 1;
                }
                while i < lines.len() && lines[i].kind == LineKind::Added {
                    adds.push(i);
                    i += 1;
                }
                let n = dels.len().max(adds.len());
                for k in 0..n {
                    rows.push(SideBySideRow {
                        left: dels.get(k).copied(),
                        right: adds.get(k).copied(),
                    });
                }
            }
        }
    }
    rows
}

/// Build a unified patch containing only the selected changed lines of `file`
/// (`selected(hunk_ix, line_ix)`), suitable for `git apply --cached --unidiff-zero`.
/// Unselected deletions become context; unselected additions are dropped.
pub fn partial_patch(file: &FileDiff, path: &str, selected: impl Fn(usize, usize) -> bool) -> Option<String> {
    let mut out = String::new();
    let mut any = false;
    out.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
    // Track how earlier hunks shift new-file line numbers.
    let mut offset: i64 = 0;
    for (hi, hunk) in file.hunks.iter().enumerate() {
        let mut body = String::new();
        let (mut old_count, mut new_count) = (0u32, 0u32);
        let mut hunk_any = false;
        for (li, line) in hunk.lines.iter().enumerate() {
            match line.kind {
                LineKind::Context => {
                    body.push(' ');
                    body.push_str(&line.text);
                    body.push('\n');
                    old_count += 1;
                    new_count += 1;
                }
                LineKind::Deleted => {
                    if selected(hi, li) {
                        body.push('-');
                        body.push_str(&line.text);
                        body.push('\n');
                        old_count += 1;
                        hunk_any = true;
                    } else {
                        body.push(' ');
                        body.push_str(&line.text);
                        body.push('\n');
                        old_count += 1;
                        new_count += 1;
                    }
                }
                LineKind::Added => {
                    if selected(hi, li) {
                        body.push('+');
                        body.push_str(&line.text);
                        body.push('\n');
                        new_count += 1;
                        hunk_any = true;
                    }
                }
            }
        }
        if !hunk_any {
            continue;
        }
        any = true;
        // git semantics: a zero-length range `-N,0` / `+N,0` means "after line N".
        let old_start = if old_count == 0 {
            if hunk.old_lines == 0 {
                hunk.old_start
            } else {
                hunk.old_start.saturating_sub(1)
            }
        } else {
            hunk.old_start
        };
        let new_start = if old_count == 0 {
            (old_start as i64 + offset + 1).max(1) as u32
        } else if new_count == 0 {
            (old_start as i64 - 1 + offset).max(0) as u32
        } else {
            (old_start as i64 + offset).max(1) as u32
        };
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start, old_count, new_start, new_count
        ));
        out.push_str(&body);
        offset += new_count as i64 - old_count as i64;
    }
    any.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunks_and_counts() {
        let old = b"a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = b"a\nb\nC\nd\ne\nf\ng\nh\ni\n";
        let d = diff_bytes(
            old,
            new,
            &DiffOptions {
                context: 1,
                ..Default::default()
            },
        );
        assert!(!d.binary);
        assert_eq!(d.hunks.len(), 2);
        assert_eq!(d.additions(), 2);
        assert_eq!(d.deletions(), 1);
        assert_eq!(d.hunks[0].header(), "@@ -2,3 +2,3 @@");
        let l = &d.hunks[0].lines;
        assert_eq!(l.iter().filter(|x| x.kind == LineKind::Deleted).count(), 1);
        assert_eq!(d.hunks[1].header(), "@@ -8,1 +8,2 @@");
    }

    #[test]
    fn word_level_ranges() {
        let (a, b) = word_diff(
            "self.occupied.insert(lane, c.id);",
            "self.occupied.insert(lane, c.id, c.at);",
        );
        assert!(a.is_empty() || a.iter().all(|r| r.start <= r.end));
        assert_eq!(&"self.occupied.insert(lane, c.id, c.at);"[b[0].clone()], ", c.at");
    }

    #[test]
    fn side_by_side_pairs_changes() {
        let d = diff_bytes(b"x\ny\n", b"x\nY\nz\n", &DiffOptions::default());
        let rows = side_by_side(&d.hunks[0]);
        assert_eq!(rows.len(), 3); // x | x ; y | Y ; - | z
        assert!(rows[1].left.is_some() && rows[1].right.is_some());
        assert!(rows[2].left.is_none() && rows[2].right.is_some());
    }

    #[test]
    fn partial_patch_keeps_only_selected() {
        let d = diff_bytes(
            b"a\nb\nc\n",
            b"a\nB\nc\nd\n",
            &DiffOptions {
                context: 0,
                ..Default::default()
            },
        );
        // select only the addition of "d" (hunk 1), not the b->B change (hunk 0)
        let p = partial_patch(&d, "f.txt", |h, _| h == 1).unwrap();
        assert!(p.contains("+d"));
        assert!(!p.contains("+B"));
        assert!(p.contains("@@ -3,0 +4,1 @@"), "{p}");
    }
}
