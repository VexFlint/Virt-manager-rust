//! Shared CLI-shell code for `virt-install`/`virt-clone`/`virt-xml`
//! (ticket 16) — the thin, GTK-free counterpart to the GUI-shell layer
//! in `virt-manager`: stdout progress instead of a modal, `y/n` prompts
//! instead of confirm dialogs. Sits on the exact same `virtinst-core`
//! the GUI uses; nothing here reaches back into core.
//!
//! Also where the CLI option-string parser lives: upstream's
//! `--disk path=/foo,bus=virtio` syntax, ported from `cli.py`'s
//! `parse_optstr_tuples`/`_parse_optstr_to_dict`. Per-device optstring
//! mapping (`{device}_from_optstr`/`apply_{device}_optstr`) is still
//! hand-written, not a derive-macro extension — `#[xml(nested)]` and
//! friends only became macro field kinds after more than one *XML
//! binding* struct needed the same shape, and the same trigger hasn't
//! fired here yet: `--watchdog` and `--vsock` are different enough
//! (flat self-attrs vs. dotted aliases into a nested element) that a
//! shared macro would be guessing at the general shape from n=2, not
//! discovering it.
//!
//! `select_elements`/`remove_elements` **are** generalized already
//! (tag-parameterized) — that part had zero device-specific logic to
//! begin with, so generalizing on the second consumer (`--vsock`) cost
//! nothing and removed real duplication. `select_watchdog_elements`/
//! `remove_watchdog_elements` are now thin wrappers, kept so existing
//! call sites and tests didn't need to change.
//!
//! **Known gap**: upstream registers `--watchdog`/`--vsock` (and every
//! device flag) with argparse's `action="append"` — `--watchdog X
//! --watchdog Y` in one invocation builds *two* devices
//! (`cli.run_parser` loops over the whole list). The CLI surface here
//! only accepts one of each device flag per invocation (last one wins,
//! via clap's normal single-valued `Option<String>`). Not hit by
//! anything built so far; revisit if a real use needs multiple devices
//! of the same type added in one call.

use virtinst_core::devices::{DeviceVsock, DeviceWatchdog};
use virtinst_xml::{list_remove, resolve_or_create_path, resolve_path, Document, Element};

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

/// Target-selection logic shared by `--edit` *and* `--remove-device` —
/// upstream's `_find_objects_to_edit(guest, action_name, editval,
/// parserclass)` backs `action_edit` and `action_remove_device` both.
/// Given the domain's root `<domain>` element, a device `tag`
/// (`"watchdog"`, `"vsock"`, ...), and a selector string, returns the
/// existing `<tag>` element(s) to act on. Tag-parameterized rather than
/// per-device — this half of the selection logic has zero device-specific
/// behavior, unlike the optstring-mapping functions above/below it.
///
/// `selector` is never a true absence here: `--edit` with no value at
/// all is upstream's `editval is None` case (defaults to index `1`,
/// the first object) — the CLI layer maps a bare `--edit` to the
/// literal string `"1"` before calling this, via clap's
/// `default_missing_value`, so this function only ever sees a real
/// string. `--remove-device` has no bare form upstream (its selector
/// always comes from the device flag's own value), so that path never
/// hits the default at all.
///
/// Handles the index form (`"1"`, `"-1"`) and `"all"`, matching
/// upstream's index math exactly: 1-based from the front, negative
/// counts from the back, `0` is always invalid. Error text below says
/// "--edit" even on the `--remove-device` path — matching upstream
/// exactly: `_find_objects_to_edit`'s own index/bounds `fail()` calls
/// hardcode `"--edit"` regardless of `action_name` too, only the
/// selector-string branch's message actually varies by action. Not a
/// gap introduced here. **Not yet ported**:
/// upstream's third form, a bare selector string matched against an
/// existing device's own fields (e.g. `--edit action=reset` finding
/// the device that already has `action="reset"`,
/// `lookup_child_from_option_string`) — flagged rather than built,
/// no second selector-string consumer yet to generalize from.
pub fn select_elements(
    doc: &Document,
    domain_root: Element,
    tag: &str,
    selector: &str,
) -> Result<Vec<Element>, String> {
    let all: Vec<Element> = match resolve_path(doc, domain_root, &["devices"]) {
        Some(devices) => devices.find_all(doc, tag),
        None => Vec::new(),
    };

    if selector == "all" {
        if all.is_empty() {
            return Err(format!("No {tag} objects found in the XML"));
        }
        return Ok(all);
    }

    let idx: i64 = selector.parse().map_err(|_| {
        format!(
            "'--edit {selector}' isn't a number or 'all' — matching an existing \
             device by its own field values (e.g. 'action=reset') isn't supported yet"
        )
    })?;
    if idx == 0 {
        return Err(format!("Invalid --edit option '{selector}'"));
    }
    if all.len() < idx.unsigned_abs() as usize {
        return Err(format!(
            "'--edit {selector}' requested but there are only {} {tag} object(s) in the XML",
            all.len()
        ));
    }
    let zero_based = if idx > 0 {
        (idx - 1) as usize
    } else {
        (all.len() as i64 + idx) as usize
    };
    Ok(vec![all[zero_based]])
}

/// Thin wrapper over [`select_elements`], kept so existing call sites
/// and tests didn't need to change when `--vsock` generalized it.
pub fn select_watchdog_elements(
    doc: &Document,
    domain_root: Element,
    selector: &str,
) -> Result<Vec<Element>, String> {
    select_elements(doc, domain_root, "watchdog", selector)
}

/// Applies a `--watchdog` optstring's named attributes directly onto an
/// *existing* element — the surgical half of `--edit` that
/// [`watchdog_from_optstr`]'s fresh-struct-then-`write_to` approach
/// can't be reused for. Upstream re-parses the optstring against the
/// existing Python device object (`cli.run_parser(..., editinst=...)`),
/// mutating only the fields the optstring actually named and leaving
/// every other field as it already was; a fresh
/// `DeviceWatchdog::write_to` would instead write `None` for every
/// field the optstring *didn't* name, clearing them. Setting only the
/// named attributes on the real `Element` gets the same "leave
/// everything else alone" result, and for free extends it to
/// attributes this crate doesn't even model.
pub fn apply_watchdog_optstr(doc: &mut Document, el: Element, optstr: &str) {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["model"]);

    for (cliname, val) in tuples {
        let Some(val) = val else { continue };
        match cliname.as_str() {
            "model" | "action" => el.set_attribute(doc, cliname, val),
            _ => {}
        }
    }
}

/// Upstream's `_on_off_convert`/`_raw_on_off_convert` — every
/// `is_onoff=True` CLI argument (e.g. `ParserVsock`'s `cid.auto`) shares
/// this vocabulary. Deliberately wider than an XML yes/no attribute's
/// (`y`/`yes`/`1`/`true`/`t`/`on` and `n`/`no`/`0`/`false`/`f`/`off`,
/// case-insensitive — confirmed against upstream's real
/// `_raw_on_off_convert`), and unlike `virtinst_xml`'s `Option<bool>`
/// XML-attribute impl (a plain `s == "yes"`) it **fails** on anything
/// else rather than silently defaulting to `false` — matches
/// `_on_off_convert`'s own `fail()` on unrecognized input.
fn parse_onoff(key: &str, val: &str) -> Result<bool, String> {
    match val.to_lowercase().as_str() {
        "y" | "yes" | "1" | "true" | "t" | "on" => Ok(true),
        "n" | "no" | "0" | "false" | "f" | "off" => Ok(false),
        _ => Err(format!("{key} must be 'yes' or 'no'")),
    }
}

/// `--vsock` — upstream's real `ParserVsock` (`/usr/share/virt-manager`,
/// reinstalled after the machine wipe noted in the prior commit, so this
/// is grounded in the actual class again, not inferred). `remove_first =
/// "model"` — same positional shorthand as `--watchdog`, `--vsock
/// default` means `model=default`. `cid.auto`/`cid.address` are dotted
/// aliases (`aliases = {"cid.auto": "auto_cid", "cid.address": "cid"}`)
/// landing on the nested `<cid>` element, not self-attributes. `cid.auto`
/// is `is_onoff=True` — [`parse_onoff`], not a new convention.
pub fn vsock_from_optstr(optstr: &str) -> Result<DeviceVsock, String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["model"]);

    let mut dev = DeviceVsock::default();
    for (cliname, val) in tuples {
        match cliname.as_str() {
            "model" => dev.model = val,
            "cid.auto" => {
                if let Some(v) = val {
                    dev.auto_cid = Some(parse_onoff("cid.auto", &v)?);
                }
            }
            "cid.address" => dev.cid = val.and_then(|v| v.parse().ok()),
            _ => {}
        }
    }
    Ok(dev)
}

/// [`apply_watchdog_optstr`]'s vsock counterpart — same "mutate only the
/// named attributes on the existing element" approach, extended to
/// reach through to the nested `<cid>` child for `cid.*` keys and to
/// [`vsock_from_optstr`]'s positional shorthand and `cid.auto` failure
/// mode. [`resolve_or_create_path`] creates `<cid>` if the vsock element
/// didn't have one yet (e.g. editing a `<vsock model="virtio"/>` with no
/// cid set at all), without disturbing anything else on `el`.
pub fn apply_vsock_optstr(doc: &mut Document, el: Element, optstr: &str) -> Result<(), String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["model"]);

    for (cliname, val) in tuples {
        let Some(val) = val else { continue };
        match cliname.as_str() {
            "model" => el.set_attribute(doc, "model", val),
            "cid.auto" => {
                let normalized = if parse_onoff("cid.auto", &val)? {
                    "yes"
                } else {
                    "no"
                };
                resolve_or_create_path(doc, el, &["cid"]).set_attribute(doc, "auto", normalized);
            }
            "cid.address" => {
                resolve_or_create_path(doc, el, &["cid"]).set_attribute(doc, "address", val);
            }
            _ => {}
        }
    }
    Ok(())
}

/// `--remove-device`'s path, for any device `tag` — upstream's
/// `action_remove_device`: the exact same [`select_elements`] `--edit`
/// uses (upstream reuses `_find_objects_to_edit` for both), then one
/// `guest.remove_device(dev)` per match instead of a mutation. Note
/// `selector` here is the device flag's own value, not a `model=`/
/// `action=` optstring — upstream's `--remove-device --watchdog 1`
/// passes `"1"` straight through as the selector
/// (`action.parservalue[-1]` in `virtxml.py`), the device flag's value
/// slot means something different depending on which action flag is
/// active. Returns how many elements were removed, for a caller that
/// wants to report it.
pub fn remove_elements(
    doc: &mut Document,
    domain_root: Element,
    tag: &str,
    selector: &str,
) -> Result<usize, String> {
    let els = select_elements(doc, domain_root, tag, selector)?;
    let count = els.len();
    for el in els {
        list_remove(doc, el).map_err(|e| e.to_string())?;
    }
    Ok(count)
}

/// Thin wrapper over [`remove_elements`], kept so existing call sites
/// and tests didn't need to change when `--vsock` generalized it.
pub fn remove_watchdog_elements(
    doc: &mut Document,
    domain_root: Element,
    selector: &str,
) -> Result<usize, String> {
    remove_elements(doc, domain_root, "watchdog", selector)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `test:///default`'s `"test"` domain is one shared, process-wide
    /// object — the two real-libvirt tests below both do a
    /// fetch-inactive-XML / mutate / define cycle against it, and
    /// `cargo test`'s default parallelism runs them concurrently.
    /// `virtinst-core`'s own equivalent tests hit this for real
    /// (~20% failure rate, one test's `add_device` racing away
    /// another's), which is where this lock's doc comment lives in
    /// full — same fixture, same fix, applied here too rather than
    /// relying on this crate's smaller set of racers not yet having
    /// been unlucky.
    static TEST_DOMAIN_LOCK: Mutex<()> = Mutex::new(());

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

    /// Two watchdogs (an artificial domain — real ones have at most
    /// one — chosen deliberately so index/`all` selection has more
    /// than a single trivial case to prove out).
    const TWO_WATCHDOGS_FIXTURE: &str = r#"<domain>
  <devices>
    <watchdog model="i6300esb" action="reset"/>
    <watchdog model="ib700" action="none"/>
  </devices>
</domain>"#;

    fn parse_fixture(xml: &str) -> Document {
        virtinst_xml::parse_libvirt_xml(xml).expect("fixture parses")
    }

    #[test]
    fn select_defaults_to_the_first_watchdog() {
        let doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();
        let els = select_watchdog_elements(&doc, root, "1").unwrap();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].attribute(&doc, "model"), Some("i6300esb"));
    }

    #[test]
    fn select_by_positive_index() {
        let doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();
        let els = select_watchdog_elements(&doc, root, "2").unwrap();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].attribute(&doc, "model"), Some("ib700"));
    }

    #[test]
    fn select_by_negative_index_counts_from_the_end() {
        let doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();
        let els = select_watchdog_elements(&doc, root, "-1").unwrap();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].attribute(&doc, "model"), Some("ib700"));
    }

    /// The boundary case where `-N == -len`: exercises the exact
    /// `all.len() < idx.unsigned_abs()` check and the
    /// `all.len() as i64 + idx` arithmetic together, not just `-1`.
    #[test]
    fn select_by_negative_index_at_the_far_boundary() {
        let doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();
        let els = select_watchdog_elements(&doc, root, "-2").unwrap();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].attribute(&doc, "model"), Some("i6300esb"));
    }

    #[test]
    fn select_negative_index_past_the_start_is_an_error() {
        let doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();
        assert!(select_watchdog_elements(&doc, root, "-3").is_err());
    }

    #[test]
    fn select_all_returns_every_watchdog() {
        let doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();
        let els = select_watchdog_elements(&doc, root, "all").unwrap();
        assert_eq!(els.len(), 2);
    }

    #[test]
    fn select_index_zero_is_invalid() {
        let doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();
        assert!(select_watchdog_elements(&doc, root, "0").is_err());
    }

    #[test]
    fn select_out_of_range_index_is_an_error() {
        let doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();
        assert!(select_watchdog_elements(&doc, root, "5").is_err());
    }

    #[test]
    fn select_unmatched_selector_string_is_an_error() {
        let doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();
        assert!(select_watchdog_elements(&doc, root, "action=reset").is_err());
    }

    #[test]
    fn apply_optstr_mutates_only_the_named_attribute() {
        let mut doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();
        let el = select_watchdog_elements(&doc, root, "1").unwrap()[0];

        apply_watchdog_optstr(&mut doc, el, "action=poweroff");

        // The named attribute changed...
        assert_eq!(el.attribute(&doc, "action"), Some("poweroff"));
        // ...the untouched one on the same element didn't...
        assert_eq!(el.attribute(&doc, "model"), Some("i6300esb"));
        // ...and the second watchdog wasn't touched at all.
        let els = select_watchdog_elements(&doc, root, "2").unwrap();
        assert_eq!(els[0].attribute(&doc, "action"), Some("none"));
        assert_eq!(els[0].attribute(&doc, "model"), Some("ib700"));
    }

    #[test]
    fn remove_by_index_deletes_only_that_one_element() {
        let mut doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();

        let removed = remove_watchdog_elements(&mut doc, root, "1").unwrap();
        assert_eq!(removed, 1);

        let remaining = select_watchdog_elements(&doc, root, "all").unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].attribute(&doc, "model"), Some("ib700"));
    }

    #[test]
    fn remove_all_deletes_every_watchdog() {
        let mut doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();

        let removed = remove_watchdog_elements(&mut doc, root, "all").unwrap();
        assert_eq!(removed, 2);

        assert!(select_watchdog_elements(&doc, root, "all").is_err());
    }

    #[test]
    fn remove_out_of_range_index_is_an_error_and_removes_nothing() {
        let mut doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();

        assert!(remove_watchdog_elements(&mut doc, root, "5").is_err());
        // Nothing removed — both still there.
        assert_eq!(
            select_watchdog_elements(&doc, root, "all").unwrap().len(),
            2
        );
    }

    /// The selector form users hit first among the deferred ones —
    /// upstream's own `--remove-device` help text uses a field-match
    /// example (`--remove-device --disk /some/path`). Not supported
    /// yet ([`select_watchdog_elements`]'s doc comment), but it must
    /// be *rejected*, not silently treated as "remove nothing" or
    /// "remove everything" — and rejecting it must not remove anything
    /// either, since the parse failure happens before any mutation.
    #[test]
    fn remove_by_field_match_selector_is_rejected_not_silently_ignored() {
        let mut doc = parse_fixture(TWO_WATCHDOGS_FIXTURE);
        let root = doc.root_element().unwrap();

        assert!(remove_watchdog_elements(&mut doc, root, "model=ib700").is_err());
        assert_eq!(
            select_watchdog_elements(&doc, root, "all").unwrap().len(),
            2
        );
    }

    /// Real end-to-end proof of `virt-xml --edit`'s exact call chain
    /// (`Domain::edit_inactive_xml` + this crate's own
    /// `select_watchdog_elements`/`apply_watchdog_optstr`, not core's
    /// simpler inlined equivalent in `connection::domain`'s own test)
    /// against a live `test:///default` domain — the same guarantee
    /// `connection::domain`'s tests already proved for the underlying
    /// primitive, now proved for this crate's own selection/apply
    /// logic specifically.
    #[test]
    fn edit_against_a_real_domain_uses_this_crates_own_select_and_apply() {
        use virtinst_core::connection::{Connection, Domain};
        use virtinst_core::devices::DeviceWatchdog;

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let domain = Domain::lookup_by_name(&conn, "test").expect("the default test domain");

        let watchdog = DeviceWatchdog {
            model: Some("i6300esb".into()),
            action: Some("reset".into()),
        };
        domain
            .add_device(&conn, &watchdog)
            .expect("seeding a watchdog to edit succeeds");

        domain
            .edit_inactive_xml(&conn, |doc, root| {
                let els = select_watchdog_elements(doc, root, "1")?;
                for el in els {
                    apply_watchdog_optstr(doc, el, "action=poweroff");
                }
                Ok(())
            })
            .expect("editing via this crate's own selector/apply succeeds");

        // Re-parse and check the watchdog element's own attributes
        // directly rather than scanning the whole document for
        // substrings — `test:///`'s domain XML has enough unrelated
        // text that a bare `contains`/`!contains` pair can pass for
        // the wrong reason.
        let inactive_xml = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&inactive_xml).expect("re-parses");
        let root = doc.root_element().unwrap();
        let watchdog = select_watchdog_elements(&doc, root, "1").unwrap()[0];
        assert_eq!(watchdog.attribute(&doc, "action"), Some("poweroff"));
        assert_eq!(watchdog.attribute(&doc, "model"), Some("i6300esb"));

        // `test:///default`'s "test" domain persists across tests within
        // one test binary run (only a fresh process resets it) — clean
        // up what this test added so `add_then_remove_device_against_a_
        // real_domain`'s own "exactly one watchdog after one add" isn't
        // thrown off by a leftover from this one. Confirmed empirically:
        // without this, that test saw 2 watchdogs after adding 1.
        domain
            .edit_inactive_xml(&conn, |doc, root| {
                remove_watchdog_elements(doc, root, "all")?;
                Ok(())
            })
            .expect("test cleanup succeeds");
    }

    /// Ticket 03's actual acceptance bar: editing one watchdog attribute
    /// must not disturb a completely foreign `<qemu:commandline>` block
    /// elsewhere in the document — content this crate doesn't model at
    /// all, not just an attribute it happens to leave alone. Content-level
    /// preservation (children, attributes, text all still there), not
    /// byte-for-byte — see `watchdog_build_xml_output_matches_expected_shape`'s
    /// doc comment for why exact serialized order isn't a guarantee this
    /// crate's `edit_xml` dependency currently provides.
    #[test]
    fn edit_leaves_a_foreign_qemu_commandline_block_completely_untouched() {
        const FIXTURE: &str = r#"<domain type="test" xmlns:qemu="http://libvirt.org/schemas/domain/qemu/1.0">
  <name>foo</name>
  <devices>
    <watchdog model="i6300esb" action="reset"/>
  </devices>
  <qemu:commandline>
    <qemu:arg value="-foo"/>
    <qemu:arg value="bar"/>
  </qemu:commandline>
</domain>"#;

        let mut doc = parse_fixture(FIXTURE);
        let root = doc.root_element().unwrap();
        let el = select_watchdog_elements(&doc, root, "1").unwrap()[0];
        apply_watchdog_optstr(&mut doc, el, "action=poweroff");

        let written = doc.write_str().expect("serializes");
        let doc2 = virtinst_xml::parse_libvirt_xml(&written).expect("re-parses");
        let root2 = doc2.root_element().unwrap();

        let commandline = root2.find_all(&doc2, "commandline");
        assert_eq!(commandline.len(), 1, "the foreign block itself survived");
        let args = commandline[0].find_all(&doc2, "arg");
        assert_eq!(args.len(), 2, "both its children survived");
        assert_eq!(args[0].attribute(&doc2, "value"), Some("-foo"));
        assert_eq!(args[1].attribute(&doc2, "value"), Some("bar"));

        // And the actual edit still landed.
        let watchdog2 = select_watchdog_elements(&doc2, root2, "1").unwrap()[0];
        assert_eq!(watchdog2.attribute(&doc2, "action"), Some("poweroff"));
    }

    /// Real end-to-end proof of `--add-device`/`--remove-device`'s exact
    /// call chains against a live `test:///default` domain:
    /// `watchdog_from_optstr` + `Domain::add_device` (the same
    /// `add_device` `--build-xml`/`--edit`'s own tests already exercise
    /// — `--add-device` doesn't need anything new from core, just a
    /// fresh struct instead of a mutation), then
    /// `remove_watchdog_elements` through `Domain::edit_inactive_xml`
    /// to take it back out.
    #[test]
    fn add_then_remove_device_against_a_real_domain() {
        use virtinst_core::connection::{Connection, Domain};

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let domain = Domain::lookup_by_name(&conn, "test").expect("the default test domain");

        // --add-device --watchdog model=i6300esb,action=reset
        let dev = watchdog_from_optstr("model=i6300esb,action=reset");
        domain
            .add_device(&conn, &dev)
            .expect("add-device's exact call chain succeeds");

        let after_add = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&after_add).expect("re-parses");
        let root = doc.root_element().unwrap();
        assert_eq!(
            select_watchdog_elements(&doc, root, "all").unwrap().len(),
            1,
            "the watchdog actually landed in the persistent config"
        );

        // --remove-device --watchdog 1
        domain
            .edit_inactive_xml(&conn, |doc, root| {
                remove_watchdog_elements(doc, root, "1")?;
                Ok(())
            })
            .expect("remove-device's exact call chain succeeds");

        let after_remove = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&after_remove).expect("re-parses");
        let root = doc.root_element().unwrap();
        assert!(
            select_watchdog_elements(&doc, root, "all").is_err(),
            "the watchdog is actually gone from the persistent config"
        );
    }

    #[test]
    fn vsock_from_optstr_maps_model_and_dotted_cid_keys() {
        let dev = vsock_from_optstr("model=virtio,cid.auto=no,cid.address=3").unwrap();
        assert_eq!(dev.model.as_deref(), Some("virtio"));
        assert_eq!(dev.auto_cid, Some(false));
        assert_eq!(dev.cid, Some(3));
    }

    /// `remove_first = "model"` on the real `ParserVsock` — same
    /// positional shorthand `--watchdog` has, confirmed against upstream
    /// after the machine-wipe caveat on the original wip commit.
    #[test]
    fn vsock_from_optstr_handles_the_positional_form() {
        let dev = vsock_from_optstr("virtio,cid.address=3").unwrap();
        assert_eq!(dev.model.as_deref(), Some("virtio"));
        assert_eq!(dev.cid, Some(3));
    }

    /// `is_onoff=True`'s wider vocabulary, not just the literal
    /// `"yes"`/`"no"` an XML attribute round-trips through.
    #[test]
    fn vsock_from_optstr_cid_auto_accepts_the_full_onoff_vocabulary() {
        for (input, expected) in [
            ("yes", true),
            ("y", true),
            ("1", true),
            ("true", true),
            ("t", true),
            ("on", true),
            ("no", false),
            ("n", false),
            ("0", false),
            ("false", false),
            ("f", false),
            ("off", false),
            ("YES", true),
            ("OFF", false),
        ] {
            let dev = vsock_from_optstr(&format!("cid.auto={input}")).unwrap();
            assert_eq!(dev.auto_cid, Some(expected), "input was {input:?}");
        }
    }

    /// Matches `_on_off_convert`'s own `fail()` — garbage input is
    /// rejected, not silently coerced to `false` the way an XML
    /// attribute's `Option<bool>` impl would.
    #[test]
    fn vsock_from_optstr_cid_auto_rejects_garbage() {
        assert!(vsock_from_optstr("cid.auto=maybe").is_err());
    }

    #[test]
    fn vsock_build_xml_output_matches_expected_shape() {
        let dev = vsock_from_optstr("model=virtio,cid.auto=no,cid.address=3").unwrap();
        let xml = virtinst_xml::to_xml_string(&dev).unwrap();
        assert!(xml.starts_with("<vsock "));
        assert!(xml.contains(r#"model="virtio""#));
        assert!(xml.contains("<cid "));
        assert!(xml.contains(r#"auto="no""#));
        assert!(xml.contains(r#"address="3""#));
    }

    /// The risk surface specific to vsock: `cid.*` keys target a nested
    /// element the top-level `<vsock>` tag may not have yet at all — the
    /// reason `apply_vsock_optstr` reaches for
    /// [`resolve_or_create_path`] instead of [`Element::set_attribute`]
    /// directly like [`apply_watchdog_optstr`] can.
    #[test]
    fn apply_vsock_optstr_creates_the_nested_cid_element_when_absent() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <vsock model="virtio"/>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "vsock", "1").unwrap()[0];

        apply_vsock_optstr(&mut doc, el, "cid.auto=no,cid.address=3").unwrap();

        let cid = el.find(&doc, "cid").expect("cid element was created");
        assert_eq!(cid.attribute(&doc, "auto"), Some("no"));
        assert_eq!(cid.attribute(&doc, "address"), Some("3"));
        // The untouched attribute on the parent element is still there.
        assert_eq!(el.attribute(&doc, "model"), Some("virtio"));
    }

    /// Pins the fix for a real divergence ultrareview caught, now under
    /// `is_onoff`'s real (wider) vocabulary: [`vsock_from_optstr`] and
    /// [`apply_vsock_optstr`] must agree on non-canonical-but-valid
    /// `cid.auto` input (`"true"` is truthy in this vocabulary, not
    /// rejected and not silently coerced to `false` the way an XML
    /// attribute's `Option<bool>` impl would read it).
    #[test]
    fn apply_vsock_optstr_normalizes_cid_auto_the_same_way_vsock_from_optstr_does() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <vsock model="virtio"/>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "vsock", "1").unwrap()[0];

        apply_vsock_optstr(&mut doc, el, "cid.auto=true").unwrap();

        let cid = el.find(&doc, "cid").expect("cid element was created");
        assert_eq!(cid.attribute(&doc, "auto"), Some("yes"));
        assert_eq!(
            vsock_from_optstr("cid.auto=true").unwrap().auto_cid,
            Some(true)
        );
    }

    /// Matches `_on_off_convert`'s `fail()` on the edit path too, and
    /// (like `remove_by_field_match_selector_is_rejected_not_silently_
    /// ignored` for watchdog) confirms rejection doesn't leave a
    /// half-applied mutation behind.
    #[test]
    fn apply_vsock_optstr_rejects_garbage_cid_auto_without_mutating_anything() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <vsock model="virtio"><cid auto="yes" address="7"/></vsock>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "vsock", "1").unwrap()[0];

        assert!(apply_vsock_optstr(&mut doc, el, "cid.auto=maybe,cid.address=9").is_err());

        // Nothing changed — including the address that came earlier in
        // the same optstring, since cid.auto is what fails.
        let cid = el.find(&doc, "cid").unwrap();
        assert_eq!(cid.attribute(&doc, "auto"), Some("yes"));
        assert_eq!(cid.attribute(&doc, "address"), Some("7"));
    }

    #[test]
    fn apply_vsock_optstr_mutates_only_the_named_cid_attribute() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <vsock model="virtio"><cid auto="yes" address="7"/></vsock>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "vsock", "1").unwrap()[0];

        apply_vsock_optstr(&mut doc, el, "cid.address=3").unwrap();

        let cid = el.find(&doc, "cid").unwrap();
        assert_eq!(cid.attribute(&doc, "address"), Some("3"));
        assert_eq!(cid.attribute(&doc, "auto"), Some("yes"));
    }

    /// Real end-to-end proof mirroring `add_then_remove_device_against_a_
    /// real_domain`, for the nested-element device instead of the
    /// flat-attribute one — the case ticket 06's order-preserving DOM
    /// actually has to construct a new child element for, not just set
    /// an attribute on an existing one.
    #[test]
    fn add_then_remove_vsock_against_a_real_domain() {
        use virtinst_core::connection::{Connection, Domain};

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let domain = Domain::lookup_by_name(&conn, "test").expect("the default test domain");

        let dev = vsock_from_optstr("model=virtio,cid.auto=no,cid.address=3").unwrap();
        domain
            .add_device(&conn, &dev)
            .expect("add-device's exact call chain succeeds");

        let after_add = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&after_add).expect("re-parses");
        let root = doc.root_element().unwrap();
        assert_eq!(
            select_elements(&doc, root, "vsock", "all").unwrap().len(),
            1,
            "the vsock actually landed in the persistent config"
        );

        domain
            .edit_inactive_xml(&conn, |doc, root| {
                remove_elements(doc, root, "vsock", "1")?;
                Ok(())
            })
            .expect("remove-device's exact call chain succeeds");

        let after_remove = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&after_remove).expect("re-parses");
        let root = doc.root_element().unwrap();
        assert!(
            select_elements(&doc, root, "vsock", "all").is_err(),
            "the vsock is actually gone from the persistent config"
        );
    }
}
