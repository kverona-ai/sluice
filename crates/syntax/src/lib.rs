//! Syntax highlighting for diffs via tree-sitter (05 §4). Language picked by
//! extension (+ shebang). The first batch of common grammars is compiled in;
//! more are added per request (the "按需加载" path).

use std::sync::OnceLock;

use sluice_core::diff::{SyntaxKind, SyntaxSpan};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    JavaScript,
    TypeScript,
    Tsx,
    Python,
    Go,
    Json,
    Toml,
    Bash,
    C,
    Cpp,
    Java,
    Yaml,
    Html,
    Css,
}

/// Capture names we map to [`SyntaxKind`] (order = highlight index).
const CAPTURES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "function",
    "function.builtin",
    "function.method",
    "function.macro",
    "keyword",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.special",
    "string.escape",
    "escape",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "label",
    "embedded",
    "boolean",
];

fn kind_of(ix: usize) -> SyntaxKind {
    match CAPTURES.get(ix).copied().unwrap_or("") {
        "attribute" => SyntaxKind::Attribute,
        "comment" => SyntaxKind::Comment,
        "constant" | "constant.builtin" | "boolean" => SyntaxKind::Constant,
        "constructor" | "type" | "type.builtin" => SyntaxKind::Type,
        "function" | "function.builtin" | "function.method" | "function.macro" => SyntaxKind::Function,
        "keyword" => SyntaxKind::Keyword,
        "number" => SyntaxKind::Number,
        "operator" => SyntaxKind::Operator,
        "property" | "variable.parameter" | "label" => SyntaxKind::Property,
        "punctuation" | "punctuation.bracket" | "punctuation.delimiter" | "punctuation.special" => {
            SyntaxKind::Punct
        }
        "string" | "string.special" | "string.escape" | "escape" => SyntaxKind::String,
        "tag" => SyntaxKind::Tag,
        "variable" | "variable.builtin" => SyntaxKind::Variable,
        _ => SyntaxKind::Plain,
    }
}

/// Pick a language from the path (and the first line for shebangs).
pub fn detect(path: &str, first_line: &str) -> Option<Lang> {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let ext = name.rsplit('.').next().filter(|e| *e != name).unwrap_or("");
    let by_ext = match ext {
        "rs" => Some(Lang::Rust),
        "js" | "mjs" | "cjs" | "jsx" => Some(Lang::JavaScript),
        "ts" | "mts" | "cts" => Some(Lang::TypeScript),
        "tsx" => Some(Lang::Tsx),
        "py" | "pyi" => Some(Lang::Python),
        "go" => Some(Lang::Go),
        "json" | "jsonc" | "json5" => Some(Lang::Json),
        "toml" => Some(Lang::Toml),
        "sh" | "bash" | "zsh" => Some(Lang::Bash),
        "c" | "h" => Some(Lang::C),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "mm" => Some(Lang::Cpp),
        "java" => Some(Lang::Java),
        "yml" | "yaml" => Some(Lang::Yaml),
        "html" | "htm" | "vue" | "svelte" => Some(Lang::Html),
        "css" | "scss" | "less" => Some(Lang::Css),
        _ => None,
    };
    if by_ext.is_some() {
        return by_ext;
    }
    if first_line.starts_with("#!") {
        let l = first_line;
        if l.contains("python") {
            return Some(Lang::Python);
        }
        if l.contains("bash") || l.contains("/sh") || l.contains("zsh") {
            return Some(Lang::Bash);
        }
        if l.contains("node") || l.contains("deno") || l.contains("bun") {
            return Some(Lang::JavaScript);
        }
    }
    None
}

fn config(lang: Lang) -> &'static HighlightConfiguration {
    macro_rules! once {
        ($name:ident, $build:expr) => {{
            static $name: OnceLock<HighlightConfiguration> = OnceLock::new();
            $name.get_or_init(|| {
                let mut c: HighlightConfiguration = $build;
                c.configure(CAPTURES);
                c
            })
        }};
    }
    match lang {
        Lang::Rust => once!(
            RUST,
            HighlightConfiguration::new(
                tree_sitter_rust::LANGUAGE.into(),
                "rust",
                tree_sitter_rust::HIGHLIGHTS_QUERY,
                tree_sitter_rust::INJECTIONS_QUERY,
                ""
            )
            .expect("rust grammar")
        ),
        Lang::JavaScript => once!(
            JS,
            HighlightConfiguration::new(
                tree_sitter_javascript::LANGUAGE.into(),
                "javascript",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::INJECTIONS_QUERY,
                tree_sitter_javascript::LOCALS_QUERY
            )
            .expect("js grammar")
        ),
        Lang::TypeScript => once!(
            TS,
            HighlightConfiguration::new(
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                "typescript",
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                "",
                tree_sitter_typescript::LOCALS_QUERY
            )
            .expect("ts grammar")
        ),
        Lang::Tsx => once!(
            TSX,
            HighlightConfiguration::new(
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                "tsx",
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                "",
                tree_sitter_typescript::LOCALS_QUERY
            )
            .expect("tsx grammar")
        ),
        Lang::Python => once!(
            PY,
            HighlightConfiguration::new(
                tree_sitter_python::LANGUAGE.into(),
                "python",
                tree_sitter_python::HIGHLIGHTS_QUERY,
                "",
                ""
            )
            .expect("python grammar")
        ),
        Lang::Go => once!(
            GO,
            HighlightConfiguration::new(
                tree_sitter_go::LANGUAGE.into(),
                "go",
                tree_sitter_go::HIGHLIGHTS_QUERY,
                "",
                ""
            )
            .expect("go grammar")
        ),
        Lang::Json => once!(
            JSON,
            HighlightConfiguration::new(
                tree_sitter_json::LANGUAGE.into(),
                "json",
                tree_sitter_json::HIGHLIGHTS_QUERY,
                "",
                ""
            )
            .expect("json grammar")
        ),
        Lang::Toml => once!(
            TOML,
            HighlightConfiguration::new(
                tree_sitter_toml_ng::LANGUAGE.into(),
                "toml",
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
                "",
                ""
            )
            .expect("toml grammar")
        ),
        Lang::Bash => once!(
            BASH,
            HighlightConfiguration::new(
                tree_sitter_bash::LANGUAGE.into(),
                "bash",
                tree_sitter_bash::HIGHLIGHT_QUERY,
                "",
                ""
            )
            .expect("bash grammar")
        ),
        Lang::C => once!(
            C,
            HighlightConfiguration::new(
                tree_sitter_c::LANGUAGE.into(),
                "c",
                tree_sitter_c::HIGHLIGHT_QUERY,
                "",
                ""
            )
            .expect("c grammar")
        ),
        Lang::Cpp => once!(
            CPP,
            HighlightConfiguration::new(
                tree_sitter_cpp::LANGUAGE.into(),
                "cpp",
                tree_sitter_cpp::HIGHLIGHT_QUERY,
                "",
                ""
            )
            .expect("cpp grammar")
        ),
        Lang::Java => once!(
            JAVA,
            HighlightConfiguration::new(
                tree_sitter_java::LANGUAGE.into(),
                "java",
                tree_sitter_java::HIGHLIGHTS_QUERY,
                "",
                ""
            )
            .expect("java grammar")
        ),
        Lang::Yaml => once!(
            YAML,
            HighlightConfiguration::new(
                tree_sitter_yaml::LANGUAGE.into(),
                "yaml",
                tree_sitter_yaml::HIGHLIGHTS_QUERY,
                "",
                ""
            )
            .expect("yaml grammar")
        ),
        Lang::Html => once!(
            HTML,
            HighlightConfiguration::new(
                tree_sitter_html::LANGUAGE.into(),
                "html",
                tree_sitter_html::HIGHLIGHTS_QUERY,
                tree_sitter_html::INJECTIONS_QUERY,
                ""
            )
            .expect("html grammar")
        ),
        Lang::Css => once!(
            CSS,
            HighlightConfiguration::new(
                tree_sitter_css::LANGUAGE.into(),
                "css",
                tree_sitter_css::HIGHLIGHTS_QUERY,
                "",
                ""
            )
            .expect("css grammar")
        ),
    }
}

/// Highlight a whole file; returns per-line spans (byte ranges within each line).
/// Files over `max_bytes` return no spans (05 §4: large files fall back to plain).
pub fn highlight_lines(lang: Lang, text: &str, max_bytes: usize) -> Vec<Vec<SyntaxSpan>> {
    let n_lines = text.lines().count();
    let mut out: Vec<Vec<SyntaxSpan>> = vec![Vec::new(); n_lines];
    if text.len() > max_bytes || text.is_empty() {
        return out;
    }
    // line start offsets
    let mut starts: Vec<usize> = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    let line_of = |off: usize| -> usize { starts.partition_point(|&s| s <= off).saturating_sub(1) };
    let mut hl = Highlighter::new();
    let Ok(events) = hl.highlight(config(lang), text.as_bytes(), None, |_| None) else {
        return out;
    };
    let mut stack: Vec<SyntaxKind> = Vec::new();
    for ev in events.flatten() {
        match ev {
            HighlightEvent::HighlightStart(h) => stack.push(kind_of(h.0)),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let Some(kind) = stack.last().copied() else {
                    continue;
                };
                if kind == SyntaxKind::Plain {
                    continue;
                }
                // split across lines
                let mut cur = start;
                while cur < end {
                    let li = line_of(cur);
                    let line_start = starts[li];
                    let line_end = starts.get(li + 1).map(|s| s - 1).unwrap_or(text.len());
                    let seg_end = end.min(line_end);
                    if seg_end > cur && li < out.len() {
                        out[li].push(SyntaxSpan {
                            start: (cur - line_start) as u32,
                            end: (seg_end - line_start) as u32,
                            kind,
                        });
                    }
                    cur = seg_end.max(cur + 1);
                    if cur < end && cur >= line_end {
                        cur = line_end + 1;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_by_extension_and_shebang() {
        assert_eq!(detect("src/main.rs", ""), Some(Lang::Rust));
        assert_eq!(detect("a/b.tsx", ""), Some(Lang::Tsx));
        assert_eq!(detect("run", "#!/usr/bin/env python3"), Some(Lang::Python));
        assert_eq!(detect("Cargo.lock", ""), None);
    }

    #[test]
    fn rust_keywords_get_spans() {
        let src = "fn main() {\n    let x = 1; // hi\n}\n";
        let lines = highlight_lines(Lang::Rust, src, 1 << 20);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].iter().any(|s| s.kind == SyntaxKind::Keyword));
        assert!(
            lines[1]
                .iter()
                .any(|s| s.kind == SyntaxKind::Comment || s.kind == SyntaxKind::Number)
        );
    }
}
