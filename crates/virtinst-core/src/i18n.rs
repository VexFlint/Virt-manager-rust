//! Translations: upstream's gettext setup (`virtinst/buildconfig.py`
//! + `virtManager/virtmanager.py`'s `gettext.install`) for every binary.
//!
//! Catalogs are upstream's own `po/*.po`, so a msgid here must be the
//! upstream English text byte for byte (mnemonic `_` and Python
//! `%s`/`%(name)s` placeholders included). [`trf!`] fills those
//! placeholders after the lookup, so the msgid never changes shape.
//!
//! Domain `virt-manager-rust` (not `virt-manager`) so these `.mo` files
//! never mix with an installed Python virt-manager's.
//!
//! ```ignore
//! use virtinst_core::{tr, trf};
//! let title = tr!("New VM");
//! let msg = trf!("Error starting domain: %s", err);
//! let msg = trf!("Unable to complete install: '%(error)s'", error = err);
//! ```

use std::fmt::Display;

pub const DOMAIN: &str = "virt-manager-rust";

/// Where the `.mo` files live, first match wins:
/// 1. `VIRT_MANAGER_LOCALEDIR` at run time (testing a catalog by hand);
/// 2. `VIRT_MANAGER_LOCALEDIR` at build time (packages set it);
/// 3. debug builds: the catalogs `build.rs` compiled into `OUT_DIR`,
///    so `cargo run` is translated with nothing installed;
/// 4. `/usr/share/locale`.
pub fn localedir() -> String {
    if let Ok(dir) = std::env::var("VIRT_MANAGER_LOCALEDIR") {
        return dir;
    }
    if let Some(dir) = option_env!("VIRT_MANAGER_LOCALEDIR") {
        return dir.to_string();
    }
    if cfg!(debug_assertions) {
        return concat!(env!("OUT_DIR"), "/locale").to_string();
    }
    "/usr/share/locale".to_string()
}

/// Call first thing in `main`: `setlocale(LC_ALL, "")` and binds
/// [`DOMAIN`]. Failures are ignored, as upstream does: the text just
/// stays in English.
pub fn init() {
    init_with(&localedir());
}

pub fn init_with(dir: &str) {
    use gettextrs::*;
    setlocale(LocaleCategory::LcAll, "");
    let _ = bindtextdomain(DOMAIN, dir);
    let _ = bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _ = textdomain(DOMAIN);
}

/// `_()`.
pub fn gettext(msgid: &str) -> String {
    gettextrs::dgettext(DOMAIN, msgid)
}

/// `_()` for a literal msgid, as a `&'static str` so [`tr!`] drops in
/// wherever the English literal was. The language never changes while
/// running, so each msgid is looked up and leaked once.
pub fn gettext_static(msgid: &'static str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<&'static str, &'static str>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    cache.entry(msgid).or_insert_with(|| {
        let s = gettext(msgid);
        if s == msgid {
            msgid
        } else {
            Box::leak(s.into_boxed_str())
        }
    })
}

/// `ngettext()`.
pub fn ngettext(singular: &str, plural: &str, n: u32) -> String {
    gettextrs::dngettext(DOMAIN, singular, plural, n)
}

/// Python's `%` operator on a translated string: `%s`/`%d`/`%i` take the
/// positional `args` in order, `%(name)s`/`%(name)d` the `named` ones,
/// `%%` is a literal `%`. Anything else is copied as is.
pub fn format(template: &str, args: &[&dyn Display], named: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut next = 0;
    let mut rest = template;
    while let Some(pos) = rest.find('%') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        if let Some(tail) = after.strip_prefix('%') {
            out.push('%');
            rest = tail;
        } else if let Some(conv) = after
            .chars()
            .next()
            .filter(|c| matches!(c, 's' | 'd' | 'i'))
        {
            match args.get(next) {
                Some(a) => out.push_str(&a.to_string()),
                None => out.push_str(&rest[pos..pos + 2]),
            }
            next += 1;
            rest = &after[conv.len_utf8()..];
        } else if let Some(body) = after.strip_prefix('(') {
            let parsed = body.find(')').and_then(|end| {
                let conv = body[end + 1..].chars().next()?;
                matches!(conv, 's' | 'd' | 'i').then_some((end, conv))
            });
            match parsed {
                Some((end, conv)) => {
                    let name = &body[..end];
                    match named.iter().find(|(n, _)| *n == name) {
                        Some((_, v)) => out.push_str(&v.to_string()),
                        None => out.push_str(&rest[pos..pos + end + 4]),
                    }
                    rest = &body[end + 1 + conv.len_utf8()..];
                }
                None => {
                    out.push('%');
                    rest = after;
                }
            }
        } else {
            out.push('%');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// `_("text")`: a `&'static str`, a drop-in for the English literal.
#[macro_export]
macro_rules! tr {
    ($msgid:literal) => {
        $crate::i18n::gettext_static($msgid)
    };
}

/// `_("text %s") % value` or `_("text %(name)s") % {"name": value}`.
#[macro_export]
macro_rules! trf {
    ($msgid:expr, $($name:ident = $val:expr),+ $(,)?) => {
        $crate::i18n::format(&$crate::i18n::gettext($msgid), &[], &[$((stringify!($name), &$val)),+])
    };
    ($msgid:expr, $($val:expr),+ $(,)?) => {
        $crate::i18n::format(&$crate::i18n::gettext($msgid), &[$(&$val),+], &[])
    };
}

/// `ngettext(singular, plural, n)`, with placeholders like [`trf!`].
#[macro_export]
macro_rules! ntr {
    ($singular:expr, $plural:expr, $n:expr) => {
        $crate::i18n::ngettext($singular, $plural, $n)
    };
    ($singular:expr, $plural:expr, $n:expr, $($val:expr),+ $(,)?) => {
        $crate::i18n::format(&$crate::i18n::ngettext($singular, $plural, $n), &[$(&$val),+], &[])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_positional_named_and_percent() {
        assert_eq!(format("%s of %d", &[&"a", &3], &[]), "a of 3");
        assert_eq!(
            format("%(name)s: %(n)d%%", &[], &[("name", &"x"), ("n", &5)]),
            "x: 5%"
        );
        // Missing values and unknown conversions stay visible.
        assert_eq!(format("%s %s", &[&1], &[]), "1 %s");
        assert_eq!(format("%(nope)s", &[], &[]), "%(nope)s");
        assert_eq!(format("50% %.1f", &[], &[]), "50% %.1f");
        assert_eq!(format("ação %s", &[&"é"], &[]), "ação é");
    }

    #[test]
    fn macros() {
        // No catalog bound in this test: msgids come back untouched.
        assert_eq!(crate::trf!("Unknown text %s", 7), "Unknown text 7");
        assert_eq!(crate::trf!("%(a)s-%(b)s", a = 1, b = "z"), "1-z");
        assert_eq!(crate::ntr!("%d disk", "%d disks", 2, 2), "2 disks");
    }

    /// The pt_BR catalog `build.rs` compiled from upstream's
    /// `po/pt_BR.po` really translates, singular/plural included.
    /// Runs in a child process: locale and env are process-wide.
    #[test]
    fn pt_br_catalog_translates() {
        if std::env::var_os("I18N_CHILD").is_some() {
            init_with(concat!(env!("OUT_DIR"), "/locale"));
            println!(
                "{}|{}|{}",
                crate::tr!("New VM"),
                crate::tr!("_New"),
                crate::tr!("not in the catalog")
            );
            return;
        }
        let mo = concat!(
            env!("OUT_DIR"),
            "/locale/pt_BR/LC_MESSAGES/virt-manager-rust.mo"
        );
        assert!(
            std::path::Path::new(mo).exists(),
            "{mo} missing: install gettext (msgfmt)"
        );
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "i18n::tests::pt_br_catalog_translates",
                "--nocapture",
            ])
            .env("I18N_CHILD", "1")
            .env("LC_ALL", "pt_BR.UTF-8")
            .env_remove("LANGUAGE")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        // glibc ignores catalogs under the C locale, so pt_BR.UTF-8 has
        // to exist: `sudo localedef -i pt_BR -f UTF-8 pt_BR.UTF-8`.
        assert!(
            stdout.contains("Nova VM|_Nova|not in the catalog"),
            "{stdout}\n(is the pt_BR.UTF-8 locale installed? `locale -a`)"
        );
    }
}
