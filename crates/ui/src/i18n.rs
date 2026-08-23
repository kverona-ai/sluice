//! Minimal UI localization: Chinese source strings, English via a lookup table
//! (`assets/i18n/en.json`). `t()` keeps `&'static str` types intact by interning
//! translations once; strings that are `format!` templates stay in the source
//! language for now (05 §9 — i18n infrastructure first, full coverage follows).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(l: Lang) {
    LANG.store(if l == Lang::En { 1 } else { 0 }, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    if LANG.load(Ordering::Relaxed) == 1 {
        Lang::En
    } else {
        Lang::Zh
    }
}

fn table() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let raw: HashMap<String, String> =
            serde_json::from_str(include_str!("../../../assets/i18n/en.json")).unwrap_or_default();
        raw.into_iter()
            .map(|(k, v)| (&*Box::leak(k.into_boxed_str()), &*Box::leak(v.into_boxed_str())))
            .collect()
    })
}

/// Translate a source (Chinese) UI string for the active language.
pub fn tr(s: &'static str) -> &'static str {
    match lang() {
        Lang::Zh => s,
        Lang::En => table().get(s).copied().unwrap_or(s),
    }
}

/// Translate an owned string (used for values assembled at runtime); falls back to the input.
pub fn trs(s: &str) -> String {
    match lang() {
        Lang::Zh => s.to_string(),
        Lang::En => table()
            .get(s)
            .map(|v| v.to_string())
            .unwrap_or_else(|| s.to_string()),
    }
}

/// Translate a positional `{}` template and fill it. Used through [`tf!`].
pub fn fmt_tpl(tpl: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    let t = tr(tpl);
    let mut out = String::with_capacity(t.len() + 16);
    let mut it = args.iter();
    let mut rest = t;
    while let Some(pos) = rest.find("{}") {
        out.push_str(&rest[..pos]);
        match it.next() {
            Some(a) => out.push_str(&a.to_string()),
            None => out.push_str("{}"),
        }
        rest = &rest[pos + 2..];
    }
    out.push_str(rest);
    out
}

/// `format!` for UI strings: the template is looked up in the translation table
/// (only positional `{}` placeholders are supported).
#[macro_export]
macro_rules! tf {
    ($tpl:literal $(, $arg:expr)* $(,)?) => {
        $crate::i18n::fmt_tpl($tpl, &[$(&($arg) as &dyn ::std::fmt::Display),*])
    };
}
