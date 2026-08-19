//! Shared CLI-shell code for `virt-install`/`virt-clone`/`virt-xml`
//! (ticket 16) — the thin, GTK-free counterpart to the GUI-shell layer
//! in `virt-manager`: stdout progress instead of a modal, `y/n` prompts
//! instead of confirm dialogs. Sits on the exact same `virtinst-core`
//! the GUI uses; nothing here reaches back into core.
//!
//! Also where the CLI option-string parser lives: upstream's
//! `--disk path=/foo,bus=virtio` syntax, ported from `cli.py`'s
//! `parse_optstr_tuples`/`_parse_optstr_to_dict`. This first slice is
//! deliberately one device (`--watchdog`, the simplest `Parser*` class
//! upstream has — no `aliases`, no `cb=`, no custom `_parse` override)
//! parsed by hand in [`watchdog_from_optstr`], not yet a derive-macro
//! extension. `#[xml(nested)]` and friends only became macro field
//! kinds after more than one struct needed the same shape (see
//! `virtinst-xml-derive`'s history) — this waits for a second real
//! consumer before generalizing the same way.

use virtinst_core::devices::DeviceWatchdog;

/// Upstream's `parse_optstr_tuples`: split `"model=itco,action=none"`
/// into an ordered list of `(cliname, value)` tuples — `"model=itco,path=foo,model=bar"`
/// keeps both `model` tuples, in order, so a caller doing `can_comma`-style
/// or repeated-key handling later still can. A bare key with no `=`
/// (`"none"`) yields `(cliname, None)`, not `(cliname, "")` — upstream
/// branches on `val is not None` in several places downstream of this.
///
/// **Known gap**: upstream tokenizes with `shlex.shlex(optstr, posix=True)`
/// (whitespace set to `","`), which also handles quoted values
/// (`path="/my,path"`) and backslash escapes. This is a plain
/// comma-split — a value containing a literal comma has no way to
/// escape it here. Not hit by `--watchdog`'s plain `model`/`action`
/// values; add real shlex-equivalent quoting if a device that needs it
/// (e.g. `--disk`) is ported.
pub fn parse_optstr_tuples(optstr: &str) -> Vec<(String, Option<String>)> {
    optstr
        .split(',')
        .filter(|tok| !tok.is_empty())
        .map(|tok| match tok.split_once('=') {
            Some((cliname, val)) => (cliname.to_string(), Some(val.to_string())),
            None => (tok.to_string(), None),
        })
        .collect()
}

/// Upstream's "splicing in `remove_first` names upfront": lets
/// `--watchdog default` mean the same as `--watchdog model=default`.
/// Walks the tuples from the front, and for each one that has no `=`
/// (`val.is_none()`), replaces its cliname with the next name off
/// `remove_first` — stopping as soon as it hits a tuple that *does*
/// have a value, or runs out of `remove_first` names. This exact
/// break condition (not "splice every bare key") matches
/// `_parse_optstr_to_dict`'s `for idx, (cliname, val) in
/// enumerate(opttuples): if val is not None or not remove_first: break`.
pub fn splice_remove_first(tuples: &mut [(String, Option<String>)], remove_first: &[&str]) {
    let mut names = remove_first.iter();
    for (cliname, val) in tuples.iter_mut() {
        if val.is_some() {
            break;
        }
        let Some(&next_name) = names.next() else {
            break;
        };
        *val = Some(std::mem::replace(cliname, next_name.to_string()));
    }
}

/// `--watchdog` — upstream's `ParserWatchdog` (`remove_first = "model"`,
/// two plain `add_arg` calls, no aliases or custom `_parse`). Unknown
/// clinames are ignored rather than erroring — upstream's real behavior
/// (unrecognized name -> `fail()` with a "no arg with name X" message)
/// isn't ported yet; this slice only needs the mapping to prove out.
pub fn watchdog_from_optstr(optstr: &str) -> DeviceWatchdog {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["model"]);

    let mut dev = DeviceWatchdog::default();
    for (cliname, val) in tuples {
        match cliname.as_str() {
            "model" => dev.model = val,
            "action" => dev.action = val,
            _ => {}
        }
    }
    dev
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuples_split_on_comma_and_equals() {
        assert_eq!(
            parse_optstr_tuples("path=foo,size=5,path=bar"),
            vec![
                ("path".to_string(), Some("foo".to_string())),
                ("size".to_string(), Some("5".to_string())),
                ("path".to_string(), Some("bar".to_string())),
            ]
        );
    }

    #[test]
    fn tuples_bare_key_has_no_value() {
        assert_eq!(
            parse_optstr_tuples("default"),
            vec![("default".to_string(), None)]
        );
    }

    #[test]
    fn remove_first_splices_a_leading_bare_value() {
        let mut tuples = parse_optstr_tuples("default,action=poweroff");
        splice_remove_first(&mut tuples, &["model"]);
        assert_eq!(
            tuples,
            vec![
                ("model".to_string(), Some("default".to_string())),
                ("action".to_string(), Some("poweroff".to_string())),
            ]
        );
    }

    #[test]
    fn remove_first_does_not_splice_when_first_tuple_already_has_a_value() {
        let mut tuples = parse_optstr_tuples("model=itco,action=none");
        splice_remove_first(&mut tuples, &["model"]);
        assert_eq!(
            tuples,
            vec![
                ("model".to_string(), Some("itco".to_string())),
                ("action".to_string(), Some("none".to_string())),
            ]
        );
    }

    #[test]
    fn watchdog_from_optstr_handles_the_positional_form() {
        let dev = watchdog_from_optstr("default,action=poweroff");
        assert_eq!(dev.model.as_deref(), Some("default"));
        assert_eq!(dev.action.as_deref(), Some("poweroff"));
    }

    #[test]
    fn watchdog_from_optstr_handles_the_explicit_form() {
        let dev = watchdog_from_optstr("model=itco,action=none");
        assert_eq!(dev.model.as_deref(), Some("itco"));
        assert_eq!(dev.action.as_deref(), Some("none"));
    }

    /// Pins `--build-xml`'s actual printed contract: tag, self-closing,
    /// both attribute values present — not the exact attribute *order*.
    /// `edit_xml::Element` stores attributes in a `HashMap`
    /// (`element.rs`: `attributes: HashMap<String, String>`) and its
    /// writer iterates that map directly (`document.rs`'s `write`:
    /// `for (key, val) in element.attributes(self)`), so serialized
    /// attribute order is only stable *within* one process run, not
    /// across runs (confirmed empirically: this test's expected string
    /// flipped between `model` and `action` first depending on the
    /// process's `RandomState` seed). Real, upstream-in-the-dependency
    /// gap, not introduced here — worth a closer look before ticket
    /// 03's "byte-exact preservation" claim is leaned on harder (an
    /// element with untouched attributes still gets fully reserialized
    /// in map order whenever *any* write touches its document), but
    /// out of scope for this slice. Assert order-independently here
    /// rather than pin a golden string that's flaky by construction.
    #[test]
    fn watchdog_build_xml_output_matches_expected_shape() {
        let dev = watchdog_from_optstr("model=itco,action=none");
        let xml = virtinst_xml::to_xml_string(&dev).unwrap();
        assert!(xml.starts_with("<watchdog "));
        assert!(xml.ends_with("/>"));
        assert!(xml.contains(r#"model="itco""#));
        assert!(xml.contains(r#"action="none""#));
    }
}
