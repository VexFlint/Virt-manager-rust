//! Shared CLI-shell code for `virt-install`/`virt-clone`/`virt-xml`
//! (ticket 16) — the thin, GTK-free counterpart to the GUI-shell layer
//! in `virt-manager`: stdout progress instead of a modal, `y/n` prompts
//! instead of confirm dialogs. Sits on the exact same `virtinst-core`
//! the GUI uses; nothing here reaches back into core.
//!
//! The real port of upstream's `VirtCLIParser` lives in [`parser`] and
//! [`parsers`]. What is left in this file is the older hand-written
//! optstring code for `--disk`, the one device not on that engine
//! yet; it goes away once its parser is ported.

pub mod cli;
pub mod parser;
pub mod parsers;
pub mod xmlutil;

use virtinst_core::devices::DeviceDisk;
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
/// escape it here. Still not hit: `--watchdog`'s `model`/`action` and
/// this slice's `--disk` fields (`path`, `device`, `type`,
/// `bus`/`target`, `driver.name`/`driver.type`) are all plain tokens in
/// practice — real disk paths containing a literal comma are the
/// actual upstream motivation for this, and remain a real gap here.
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

/// Upstream's `_VirtCLIArgument.__init__` — every optstring value goes
/// through this rule before reaching a device's own field mapping,
/// *regardless of device* (found while grounding `--vsock`'s
/// `edit-simple-vsock`/`--watchdog`'s `edit-simple-watchdog` fixtures
/// against the real `cli.py`, but the rule itself has nothing
/// device-specific in it — the same trigger `select_elements`/
/// `remove_elements` already generalized on). Two real states hide
/// inside one `Option<String>` from [`parse_optstr_tuples`]:
///
/// - A bare key `splice_remove_first` didn't consume (`val` is `None`)
///   is upstream's `RuntimeError("Option '%s' had no value set.")` — a
///   real parse error, not something to silently skip.
/// - An explicit empty value (`key=`) means "clear this field"
///   (upstream's `if val == "": val = None`) — for a fresh struct field
///   that's just `None`; for mutating an *existing* XML element it means
///   removing the attribute, not writing `attr=""`.
///
/// Returns `Ok(Some(v))` for a real value, `Ok(None)` for an explicit
/// clear, `Err` for the bare-key case. Doesn't touch the *unrecognized
/// cliname* gap (`_ => {}` below) — that's a separate, already-flagged,
/// deliberately-deferred piece of upstream's `fail()` behavior.
fn required_value(cliname: &str, val: Option<String>) -> Result<Option<String>, String> {
    match val {
        None => Err(format!("Option '{cliname}' had no value set.")),
        Some(v) if v.is_empty() => Ok(None),
        Some(v) => Ok(Some(v)),
    }
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
/// gap introduced here. Upstream's third selector form — a bare string
/// matched against an existing device's own field values (e.g. `--edit
/// ich6 --sound pcspk` finding the `<sound>` whose `model` is already
/// `"ich6"`) — is *not* handled here; [`select_elements_by_selector`]
/// wraps this function and adds that form on top, once
/// `remove-sound-model` made it a real blocker rather than a
/// theoretical one. Direct callers of this function keep getting the
/// plain "not a number or 'all'" error on a non-numeric, non-`"all"`
/// selector — that's intentional, not a regression: `select_elements`
/// itself still only knows index math, same as its own tests always
/// assumed.
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

/// Where a device's optstring `cliname` reads or writes on its element
/// — a self-attribute, or an attribute one or more levels down a
/// nested-element chain. The read half (`field_value`, backing the
/// field-match selector below) and the write half (every
/// `apply_{device}_optstr`'s plain-attribute match arms) both encode
/// this same alias knowledge; **kept as two hand-written copies here**
/// rather than sharing code — the write side also has to interleave
/// per-field validation (`parse_int`/`parse_onoff`/`required_value`'s
/// clear rule) that the read side never needs, so a shared table would
/// only cover the location half anyway. Every entry below is copied
/// directly from its device's real `apply_*_optstr`/`*_from_optstr`
/// match arms (not re-derived or guessed) and re-verified against real
/// upstream fixtures (`edit-select-disk-target`,
/// `edit-select-disk-path`, `edit-select-sound-model`) once built —
/// if a device's aliases ever change, both copies need updating.
enum FieldLoc {
    SelfAttr(&'static str),
    Nested(&'static [&'static str], &'static str),
}

/// See [`FieldLoc`]'s doc comment. Only covers plain attribute-valued
/// fields — present-bool fields (`--disk readonly`/`shareable`/
/// `transient`) and derived ones (`--disk perms`) have no single
/// location to compare against and aren't selector-matchable here,
/// same as upstream's own `lookup_param` returning `None`/skipping for
/// argument kinds it can't compare structurally either.
fn field_loc(device: &str, cliname: &str) -> Option<FieldLoc> {
    use FieldLoc::{Nested, SelfAttr};
    match (device, cliname) {
        ("disk", "path") => Some(Nested(&["source"], "file")),
        ("disk", "device") => Some(SelfAttr("device")),
        ("disk", "type") => Some(SelfAttr("type")),
        ("disk", "bus" | "target.bus") => Some(Nested(&["target"], "bus")),
        ("disk", "target" | "target.dev") => Some(Nested(&["target"], "dev")),
        ("disk", "driver.name") => Some(Nested(&["driver"], "name")),
        ("disk", "driver.type") => Some(Nested(&["driver"], "type")),
        _ => None,
    }
}

/// Reads whatever's currently at a [`FieldLoc`] — `None` if the nested
/// chain (or the attribute itself) isn't there at all, same "absent"
/// meaning `resolve_path` already uses.
fn field_value(doc: &Document, el: Element, loc: &FieldLoc) -> Option<String> {
    match loc {
        FieldLoc::SelfAttr(attr) => el.attribute(doc, attr).map(String::from),
        FieldLoc::Nested(path, attr) => resolve_path(doc, el, path)?
            .attribute(doc, attr)
            .map(String::from),
    }
}

/// Each device's `remove_first` list — the same array every
/// `{device}_from_optstr`/`apply_{device}_optstr` already splices with
/// inline, factored out once [`select_by_field_match`] needed the exact
/// same list for a second purpose: splicing a bare positional selector
/// (`--edit ich6 --sound pcspk`'s `"ich6"`) the same way a bare
/// positional *value* already splices (`--sound pcspk`'s `"pcspk"`) —
/// upstream reuses the identical `parserclass(editval, ...)` parse path
/// for both, so this does too.
///
/// Every wired device is named explicitly, deliberately with **no**
/// wildcard fallback: a `_ => &["model"]` default reads fine today
/// (every device so far but `disk`/`video` really does splice into
/// `model`) but is a silent-wrong-answer trap the moment a future
/// device with a different or absent `remove_first` (upstream's real
/// `_ParserChar` subclasses and `ParserFilesystem` both lack one) is
/// added to [`field_loc`] without a matching arm here — it would
/// splice a bare selector into the wrong key and either match nothing
/// or, worse, match something unintended, instead of failing loudly.
/// Panics on an unwired device: every real caller only ever passes one
/// of the devices [`device_optstr`](../virt_xml) already dispatches on
/// or a test-fixture device wired in the table below, so reaching the
/// fallback means a device was added to the write path but not here —
/// a programming error to catch immediately, not a user-facing one to
/// degrade gracefully from.
fn device_remove_first(device: &str) -> &'static [&'static str] {
    match device {
        "disk" => &["path"],
        _ => panic!("device_remove_first: no remove_first entry for device '{device}'"),
    }
}

/// The field-match half of upstream's `_find_objects_to_edit` —
/// `lookup_child_from_option_string`, reached once
/// [`select_elements_by_selector`] has already ruled out an index or
/// `"all"`. Parses `selector` as this device's own optstring (through
/// [`device_remove_first`]'s splice, same as a real `--{device}`
/// value), then keeps every existing element whose current value at
/// each named field equals what was given — *every* named field must
/// match, a single mismatch excludes that element.
///
/// An unrecognized cliname is a hard error here, deliberately unlike
/// `apply_*_optstr`'s own `_ => {}` no-op arms: a no-op *filter* would
/// silently match every element instead of properly failing (matches
/// upstream's `_check_leftover_opts`, called after
/// `_optdict_to_param_list` pops every matched key — anything left over
/// fails). **Known gap, not chased**: upstream's real per-device field
/// set is wider than this crate's partial slices (e.g. `--disk`'s real
/// indexed `seclabel[N]*` fields aren't modeled at all), so a selector
/// naming a real upstream field this crate doesn't model yet hits this
/// same "unrecognized" error rather than upstream's actual "no matching
/// objects" — out of reach for the same documented reason those fields
/// are unimplemented in the first place, not a new gap. Also unlike
/// upstream: `is_onoff` selector values (e.g. a hypothetical
/// `cid.auto=on` selector) are compared as the raw string, not
/// converted to a bool first — no reachable fixture uses a boolean
/// selector value (every one is a name, a path, a MAC, a hex address —
/// checked directly against `test_cli.py`), so this is a real but
/// unexercised divergence, not a checked-and-dismissed one.
fn select_by_field_match(
    doc: &Document,
    domain_root: Element,
    tag: &str,
    action: &str,
    selector: &str,
) -> Result<Vec<Element>, String> {
    let mut tuples = parse_optstr_tuples(selector);
    splice_remove_first(&mut tuples, device_remove_first(tag));

    let mut filters = Vec::new();
    for (cliname, val) in tuples {
        let loc = field_loc(tag, &cliname)
            .ok_or_else(|| format!("--{tag} has no option named '{cliname}'"))?;
        let expected = required_value(&cliname, val)?.unwrap_or_default();
        filters.push((loc, expected));
    }

    let all: Vec<Element> = match resolve_path(doc, domain_root, &["devices"]) {
        Some(devices) => devices.find_all(doc, tag),
        None => Vec::new(),
    };

    let matches: Vec<Element> = all
        .into_iter()
        .filter(|el| {
            filters.iter().all(|(loc, expected)| {
                field_value(doc, *el, loc).as_deref() == Some(expected.as_str())
            })
        })
        .collect();

    if matches.is_empty() {
        return Err(format!(
            "No matching objects found for --{action} {selector}"
        ));
    }
    Ok(matches)
}

/// The real top-level entry point `--edit`/`--remove-device` should
/// call from now on — upstream's own `_find_objects_to_edit` in full:
/// index/`"all"` via [`select_elements`] unchanged, anything else via
/// [`select_by_field_match`]. `action` is only used on the field-match
/// path (`"edit"`/`"remove-device"`, matching upstream's own two call
/// sites) — the index/bounds error messages [`select_elements`]
/// produces internally hardcode `"--edit"` regardless, per that
/// function's own doc comment.
pub fn select_elements_by_selector(
    doc: &Document,
    domain_root: Element,
    tag: &str,
    action: &str,
    selector: &str,
) -> Result<Vec<Element>, String> {
    if selector == "all" || selector.parse::<i64>().is_ok() {
        return select_elements(doc, domain_root, tag, selector);
    }
    select_by_field_match(doc, domain_root, tag, action, selector)
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

/// Sets or removes one attribute on a *nested* child element (`el`'s
/// `path` chain, e.g. `<cid>`/`<source>`/`<target>`/`<driver>`, or a
/// two-level chain like `<model><acceleration>`) — the dotted-key case
/// every `apply_{device}_optstr` with a non-self-attribute field needs.
/// `path` is a segment list like [`resolve_path`]'s own, not a
/// `/`-joined string — passing `&["model/acceleration"]` as one segment
/// would ask `edit-xml` for a literal child tag named
/// `"model/acceleration"` instead of walking two levels (a real bug,
/// caught by `video_accel3d_edit_reaches_the_nested_acceleration_
/// element_not_a_literal_slash_named_one` after `--video
/// model.acceleration.accel3d=on` produced exactly that malformed
/// sibling tag against the real binary). `Some(v)` creates the chain
/// first if absent ([`resolve_or_create_path`]) then sets the
/// attribute; `None` (the [`required_value`] "clear" case) removes the
/// attribute *without* creating the chain if it wasn't already there —
/// clearing a field that was never set shouldn't leave a spurious empty
/// `<cid/>`/`<source/>` behind. (An earlier version of
/// `apply_vsock_optstr` used `resolve_or_create_path` unconditionally,
/// including on the clear path — harmless in practice so far since
/// every case exercised against a real domain already had the nested
/// element, but a real gap this generalization fixes rather than
/// repeats a third time for `--disk`.)
fn apply_nested_attr(
    doc: &mut Document,
    el: Element,
    path: &[&str],
    attr: &str,
    val: Option<String>,
) {
    match val {
        Some(v) => {
            resolve_or_create_path(doc, el, path).set_attribute(doc, attr, v);
        }
        None => {
            if let Some(nested) = resolve_path(doc, el, path) {
                nested.mut_attributes(doc).remove(attr);
            }
        }
    }
}

/// Creates or removes a `#[xml(present)]`-bound child element (e.g.
/// `<readonly/>`/`<shareable/>`) — the existence-is-the-boolean field
/// kind `--disk`'s `readonly`/`shareable`/`transient`/`perms` need,
/// upstream's own `is_bool=True` (distinct from `is_onoff`'s yes/no
/// *attribute* value, though the CLI-facing vocabulary a caller parses
/// the string into is the same [`parse_onoff`]). No-ops when already in
/// the requested state, so a redundant call doesn't disturb sibling
/// content on either the parent or (if one already existed) the child.
fn apply_present_bool(doc: &mut Document, el: Element, tag: &str, present: bool) {
    match (el.find(doc, tag), present) {
        (Some(existing), false) => {
            existing
                .detach(doc)
                .expect("an element found under el detaches cleanly");
        }
        (None, true) => {
            let child = Element::new(doc, tag);
            el.push_child(doc, child)
                .expect("a freshly-created detached element always attaches cleanly");
        }
        _ => {}
    }
}

/// `--disk` — upstream's real `ParserDisk`
/// (`/usr/share/virt-manager`/`~/DEV/virt-manager-upstream-reference`),
/// by far the largest and most irregular device parser in `cli.py`: 40+
/// distinct properties, indexed `source.host[0-9]*`/`seclabel[0-9]*`
/// sub-lists, and real storage-pool/volume creation logic
/// (`size`/`format`/`sparse`/`pool`/`vol`/`backing_store`) beyond
/// declarative option mapping. **Deliberately a partial slice**,
/// covering only what `DeviceDisk` (`virtinst-core`) already models:
///
/// - `path` (`remove_first`) — always `source.file`. Upstream's real
///   `set_path_cb`/`set_source_path` is disk-type-aware (picks
///   `source.file`/`source.dev`/`source.dir`/network source attrs
///   depending on `type`); block/dir/network-type disks aren't modeled
///   here at all, flagged rather than built.
/// - `device`, `type` — `type` defaults to `"file"` and `device` to
///   `"disk"` when neither is given, matching upstream's real
///   `DeviceDisk.set_defaults`/`TYPE_FILE`/`DEVICE_DISK` constants
///   (confirmed directly, not assumed) — required to match
///   `add-disk-basic`'s exact golden output, which has both attributes
///   present despite neither being in the optstring.
/// - `bus`/`target.bus` and `target`/`target.dev` — both spellings of
///   each accepted, matching upstream's own `_add_advertised_aliases`
///   (`bus`) and `cls.aliases` (`target.dev`'s registered alternate
///   spelling is literally `"target"`, confirmed by reading how
///   `cls.aliases` gets consumed in `_virtcli_class_init_common`, not
///   assumed from the dict's shape).
/// - `driver.name`, `driver.type`.
/// - `readonly`, `shareable`, `transient` — all `is_onoff` on the CLI
///   side but `#[xml(present)]`-bound child elements on the XML side
///   (existence is the boolean, not an attribute value) — the read
///   [`apply_present_bool`] exists for.
/// - `perms` (`ro`→readonly, `sh`→shareable, `rw`→no-op) matching
///   upstream's `convert_perms` exactly, including its exact error text
///   for anything else.
///
/// Not modeled at all: `size`/`format`/`sparse`/`pool`/`vol`/
/// `backing_store`/`backing_format` (real storage-pool/volume creation —
/// a separate, large feature; ticket 15's `storage` module isn't
/// started), every `source.*`/`driver.*`/`iotune.*`/`blockio.*`/
/// `geometry.*` field beyond the two `driver.*` ones above, the indexed
/// `source.host[N]*`/`seclabel[N]*` sub-lists, `snapshot`/`sgio`/
/// `rawio`/`serial`/`wwn`/`target.removable`/`target.rotation_rate`.
pub fn disk_from_optstr(optstr: &str) -> Result<DeviceDisk, String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, device_remove_first("disk"));

    let mut dev = DeviceDisk::default();
    for (cliname, val) in tuples {
        match cliname.as_str() {
            "path" => dev.source_file = required_value(&cliname, val)?,
            "device" => dev.device = required_value(&cliname, val)?,
            "type" => dev.disk_type = required_value(&cliname, val)?,
            "bus" | "target.bus" => dev.target_bus = required_value(&cliname, val)?,
            "target" | "target.dev" => dev.target_dev = required_value(&cliname, val)?,
            "driver.name" => dev.driver_name = required_value(&cliname, val)?,
            "driver.type" => dev.driver_type = required_value(&cliname, val)?,
            "readonly" => dev.read_only = onoff_or_false("readonly", val)?,
            "shareable" => dev.shareable = onoff_or_false("shareable", val)?,
            "transient" => dev.transient = onoff_or_false("transient", val)?,
            "perms" => match perms_flags(val)? {
                Some(PermsFlag::ReadOnly) => dev.read_only = true,
                Some(PermsFlag::Shareable) => dev.shareable = true,
                None => {}
            },
            _ => {}
        }
    }
    if dev.disk_type.is_none() {
        dev.disk_type = Some("file".to_string());
    }
    if dev.device.is_none() {
        dev.device = Some("disk".to_string());
    }
    Ok(dev)
}

/// Shared by [`disk_from_optstr`]/[`apply_disk_optstr`]'s `readonly`/
/// `shareable`/`transient` arms: [`required_value`] then, if a value was
/// actually given, [`parse_onoff`] — an explicit clear (`key=`) or an
/// unset field both read as `false`, matching a `#[xml(present)]`
/// field's own "absent means false" XML shape.
fn onoff_or_false(key: &str, val: Option<String>) -> Result<bool, String> {
    match required_value(key, val)? {
        Some(v) => parse_onoff(key, &v),
        None => Ok(false),
    }
}

/// Which `#[xml(present)]` field upstream's `perms` value maps onto.
enum PermsFlag {
    ReadOnly,
    Shareable,
}

/// Upstream's `convert_perms`: `ro`→readonly, `sh`→shareable, `rw`→no-op
/// (`rw` is the default, nothing to set), anything else the exact same
/// `fail()` text upstream uses. `None` (an empty/absent `perms=`) is
/// also a no-op — upstream's own call site only invokes `convert_perms`
/// at all when the optstring actually named `perms`.
fn perms_flags(val: Option<String>) -> Result<Option<PermsFlag>, String> {
    let Some(v) = required_value("perms", val)? else {
        return Ok(None);
    };
    match v.as_str() {
        "ro" => Ok(Some(PermsFlag::ReadOnly)),
        "sh" => Ok(Some(PermsFlag::Shareable)),
        "rw" => Ok(None),
        other => Err(format!("Unknown 'perms' value '{other}'")),
    }
}

/// the old `apply_watchdog_optstr`'s disk counterpart — same shape, extended
/// with [`apply_nested_attr`] for the `source`/`target`/`driver`
/// sub-elements and [`apply_present_bool`] for the three
/// existence-is-the-boolean fields. Unlike [`disk_from_optstr`], no
/// `type`/`device` defaulting — editing an existing disk must never
/// invent attributes the optstring didn't name, same "leave everything
/// else alone" rule every `apply_*_optstr` follows.
pub fn apply_disk_optstr(doc: &mut Document, el: Element, optstr: &str) -> Result<(), String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, device_remove_first("disk"));

    for (cliname, val) in tuples {
        match cliname.as_str() {
            "path" => {
                apply_nested_attr(doc, el, &["source"], "file", required_value(&cliname, val)?)
            }
            "device" => match required_value(&cliname, val)? {
                Some(v) => el.set_attribute(doc, "device", v),
                None => {
                    el.mut_attributes(doc).remove("device");
                }
            },
            "type" => match required_value(&cliname, val)? {
                Some(v) => el.set_attribute(doc, "type", v),
                None => {
                    el.mut_attributes(doc).remove("type");
                }
            },
            "bus" | "target.bus" => {
                apply_nested_attr(doc, el, &["target"], "bus", required_value(&cliname, val)?)
            }
            "target" | "target.dev" => {
                apply_nested_attr(doc, el, &["target"], "dev", required_value(&cliname, val)?)
            }
            "driver.name" => {
                apply_nested_attr(doc, el, &["driver"], "name", required_value(&cliname, val)?)
            }
            "driver.type" => {
                apply_nested_attr(doc, el, &["driver"], "type", required_value(&cliname, val)?)
            }
            "readonly" => apply_present_bool(doc, el, "readonly", onoff_or_false("readonly", val)?),
            "shareable" => {
                apply_present_bool(doc, el, "shareable", onoff_or_false("shareable", val)?)
            }
            "transient" => {
                apply_present_bool(doc, el, "transient", onoff_or_false("transient", val)?)
            }
            "perms" => match perms_flags(val)? {
                Some(PermsFlag::ReadOnly) => apply_present_bool(doc, el, "readonly", true),
                Some(PermsFlag::Shareable) => apply_present_bool(doc, el, "shareable", true),
                None => {}
            },
            _ => {}
        }
    }
    Ok(())
}

/// `--remove-device`'s path, for any device `tag` — upstream's
/// `action_remove_device`: the exact same [`select_elements`] `--edit`
/// uses (upstream reuses `_find_objects_to_edit` for both), then one
/// `guest.remove_device(dev)` per match instead of a mutation. `selector`
/// here is the device flag's own value — upstream's `--remove-device
/// --watchdog 1` passes `"1"` straight through as the selector
/// (`action.parservalue[-1]` in `virtxml.py`). Only handles the
/// index/`"all"` forms [`select_elements`] does; see
/// [`remove_elements_by_selector`] for the field-match form
/// `--remove-device --sound ich6` needs. Returns how many elements were
/// removed, for a caller that wants to report it.
pub fn remove_elements(
    doc: &mut Document,
    domain_root: Element,
    tag: &str,
    selector: &str,
) -> Result<usize, String> {
    detach_elements(doc, select_elements(doc, domain_root, tag, selector)?)
}

/// [`remove_elements`]'s [`select_elements_by_selector`] counterpart —
/// the version `--remove-device`'s real dispatch should call, so a
/// field-match selector like `--remove-device --sound ich6`
/// (`remove-sound-model`'s real fixture) resolves the same way `--edit`
/// does rather than being rejected as "not a number or 'all'".
pub fn remove_elements_by_selector(
    doc: &mut Document,
    domain_root: Element,
    tag: &str,
    action: &str,
    selector: &str,
) -> Result<usize, String> {
    detach_elements(
        doc,
        select_elements_by_selector(doc, domain_root, tag, action, selector)?,
    )
}

/// Shared by [`remove_elements`]/[`remove_elements_by_selector`]: detach
/// every already-selected element, return how many.
fn detach_elements(doc: &mut Document, els: Vec<Element>) -> Result<usize, String> {
    let count = els.len();
    for el in els {
        list_remove(doc, el).map_err(|e| e.to_string())?;
    }
    Ok(count)
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

    fn parse_fixture(xml: &str) -> Document {
        virtinst_xml::parse_libvirt_xml(xml).expect("fixture parses")
    }

    #[test]
    fn disk_from_optstr_handles_the_positional_path_form() {
        let dev = disk_from_optstr("/var/lib/libvirt/images/foo.qcow2").unwrap();
        assert_eq!(
            dev.source_file.as_deref(),
            Some("/var/lib/libvirt/images/foo.qcow2")
        );
        // Defaults fill in even when nothing else was given.
        assert_eq!(dev.disk_type.as_deref(), Some("file"));
        assert_eq!(dev.device.as_deref(), Some("disk"));
    }

    /// Pins the fix for `add-disk-basic`'s exact golden output — `type`
    /// and `device` are present even though the optstring never named
    /// them.
    #[test]
    fn disk_from_optstr_defaults_type_and_device_when_unset() {
        let dev = disk_from_optstr("path=/tmp/x,bus=virtio").unwrap();
        assert_eq!(dev.disk_type.as_deref(), Some("file"));
        assert_eq!(dev.device.as_deref(), Some("disk"));
    }

    /// An explicit `type=`/`device=` is never overridden by the default.
    #[test]
    fn disk_from_optstr_explicit_type_and_device_are_not_defaulted_over() {
        let dev = disk_from_optstr("path=/tmp/x,type=block,device=cdrom").unwrap();
        assert_eq!(dev.disk_type.as_deref(), Some("block"));
        assert_eq!(dev.device.as_deref(), Some("cdrom"));
    }

    /// Both the dotted canonical cliname and upstream's registered
    /// alternate spelling must work — `bus`/`target.bus` and
    /// `target`/`target.dev` are two different alternate-spelling
    /// mechanisms upstream uses (`_add_advertised_aliases` vs.
    /// `cls.aliases`) with the exact same practical effect.
    #[test]
    fn disk_from_optstr_accepts_both_spellings_of_bus_and_target() {
        let short = disk_from_optstr("path=/tmp/x,bus=virtio,target=vda").unwrap();
        let long = disk_from_optstr("path=/tmp/x,target.bus=virtio,target.dev=vda").unwrap();
        assert_eq!(short.target_bus, long.target_bus);
        assert_eq!(short.target_dev, long.target_dev);
        assert_eq!(short.target_bus.as_deref(), Some("virtio"));
        assert_eq!(short.target_dev.as_deref(), Some("vda"));
    }

    #[test]
    fn disk_from_optstr_maps_driver_fields() {
        let dev = disk_from_optstr("path=/tmp/x,driver.name=qemu,driver.type=qcow2").unwrap();
        assert_eq!(dev.driver_name.as_deref(), Some("qemu"));
        assert_eq!(dev.driver_type.as_deref(), Some("qcow2"));
    }

    #[test]
    fn disk_from_optstr_readonly_shareable_transient_are_onoff() {
        let dev = disk_from_optstr("path=/tmp/x,readonly=on,shareable=yes,transient=1").unwrap();
        assert!(dev.read_only);
        assert!(dev.shareable);
        assert!(dev.transient);

        let dev = disk_from_optstr("path=/tmp/x").unwrap();
        assert!(!dev.read_only);
        assert!(!dev.shareable);
        assert!(!dev.transient);
    }

    /// Upstream's `convert_perms` exactly: `ro`→readonly, `sh`→shareable,
    /// `rw`→no-op (the default, nothing to set).
    #[test]
    fn disk_from_optstr_perms_maps_to_readonly_or_shareable() {
        assert!(disk_from_optstr("path=/tmp/x,perms=ro").unwrap().read_only);
        assert!(disk_from_optstr("path=/tmp/x,perms=sh").unwrap().shareable);
        let rw = disk_from_optstr("path=/tmp/x,perms=rw").unwrap();
        assert!(!rw.read_only && !rw.shareable);
    }

    #[test]
    fn disk_from_optstr_perms_rejects_garbage() {
        assert!(disk_from_optstr("path=/tmp/x,perms=bogus").is_err());
    }

    /// Golden fixture, adapted from upstream's own
    /// `tests/data/cli/compare/virt-xml-add-disk-basic.xml`
    /// (`~/DEV/virt-manager-upstream-reference`): `--add-device --disk
    /// /pool-dir/testvol1.img,bus=virtio,target=vdf` must produce
    /// `<disk type="file" device="disk"><source
    /// file="/pool-dir/testvol1.img"/><target dev="vdf"
    /// bus="virtio"/></disk>` — `type`/`device` present despite neither
    /// being in the optstring.
    #[test]
    fn golden_add_disk_basic() {
        let dev = disk_from_optstr("/pool-dir/testvol1.img,bus=virtio,target=vdf").unwrap();
        let xml = virtinst_xml::to_xml_string(&dev).unwrap();
        assert!(xml.starts_with("<disk "));
        assert!(xml.contains(r#"type="file""#));
        assert!(xml.contains(r#"device="disk""#));
        assert!(xml.contains(r#"<source file="/pool-dir/testvol1.img"/>"#));
        assert!(xml.starts_with(r#"<disk type="file" device="disk">"#));
        assert!(xml.contains(r#"<target dev="vdf" bus="virtio"/>"#));
    }

    const DISK_FIXTURE: &str = r#"<domain>
  <devices>
    <disk type="file" device="disk">
      <driver name="qemu" type="qcow2"/>
      <source file="/tmp/existing.qcow2"/>
      <target dev="vda" bus="virtio"/>
    </disk>
  </devices>
</domain>"#;

    #[test]
    fn apply_disk_optstr_mutates_only_the_named_nested_attribute() {
        let mut doc = parse_fixture(DISK_FIXTURE);
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "disk", "1").unwrap()[0];

        apply_disk_optstr(&mut doc, el, "target.bus=sata").unwrap();

        let target = el.find(&doc, "target").unwrap();
        assert_eq!(target.attribute(&doc, "bus"), Some("sata"));
        // Untouched sibling attribute on the same nested element.
        assert_eq!(target.attribute(&doc, "dev"), Some("vda"));
        // Untouched sibling nested element.
        let source = el.find(&doc, "source").unwrap();
        assert_eq!(source.attribute(&doc, "file"), Some("/tmp/existing.qcow2"));
    }

    #[test]
    fn apply_disk_optstr_clearing_path_removes_the_attribute_not_sets_empty() {
        let mut doc = parse_fixture(DISK_FIXTURE);
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "disk", "1").unwrap()[0];

        apply_disk_optstr(&mut doc, el, "path=").unwrap();

        let source = el.find(&doc, "source").unwrap();
        assert_eq!(source.attribute(&doc, "file"), None);
    }

    #[test]
    fn apply_disk_optstr_readonly_toggles_the_present_child_element() {
        let mut doc = parse_fixture(DISK_FIXTURE);
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "disk", "1").unwrap()[0];
        assert!(el.find(&doc, "readonly").is_none());

        apply_disk_optstr(&mut doc, el, "readonly=on").unwrap();
        assert!(el.find(&doc, "readonly").is_some());

        apply_disk_optstr(&mut doc, el, "readonly=off").unwrap();
        assert!(el.find(&doc, "readonly").is_none());
    }

    #[test]
    fn apply_disk_optstr_perms_ro_sets_readonly_present_element() {
        let mut doc = parse_fixture(DISK_FIXTURE);
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "disk", "1").unwrap()[0];

        apply_disk_optstr(&mut doc, el, "perms=ro").unwrap();

        assert!(el.find(&doc, "readonly").is_some());
        assert!(el.find(&doc, "shareable").is_none());
    }

    /// Real end-to-end proof mirroring `add_then_remove_vsock_against_a_
    /// real_domain`, for disk instead of vsock — the device with the
    /// most nested elements of the three this crate models. Unlike
    /// watchdog/vsock, `test:///default`'s own "test" domain ships with
    /// a disk of its own already (confirmed directly: `virsh -c
    /// test:///default dumpxml test` has one `<disk>` before this test
    /// ever runs) — so this checks the *count increased by exactly one*
    /// and identifies the added disk by its distinctive `target
    /// dev="vdz"` (removing by index "1" would risk removing the
    /// pre-existing disk instead of the one this test added; "-1"
    /// targets whichever one `list_add` most recently appended).
    #[test]
    fn add_then_remove_disk_against_a_real_domain() {
        use virtinst_core::connection::{Connection, Domain};

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let domain = Domain::lookup_by_name(&conn, "test").expect("the default test domain");

        let before_xml = domain.inactive_xml().expect("fetch inactive XML");
        let before_doc = virtinst_xml::parse_libvirt_xml(&before_xml).expect("re-parses");
        let before_root = before_doc.root_element().unwrap();
        let baseline = select_elements(&before_doc, before_root, "disk", "all")
            .map(|els| els.len())
            .unwrap_or(0);

        let dev = disk_from_optstr("/tmp/harness-disk.qcow2,bus=virtio,target=vdz").unwrap();
        domain
            .add_device(&conn, &dev)
            .expect("add-device's exact call chain succeeds");

        let after_add = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&after_add).expect("re-parses");
        let root = doc.root_element().unwrap();
        let disks = select_elements(&doc, root, "disk", "all").unwrap();
        assert_eq!(
            disks.len(),
            baseline + 1,
            "exactly one disk was added on top of whatever was already there"
        );
        let newest = select_elements(&doc, root, "disk", "-1").unwrap();
        assert_eq!(
            newest[0]
                .find(&doc, "target")
                .unwrap()
                .attribute(&doc, "dev"),
            Some("vdz"),
            "the last disk is the one this test added"
        );

        domain
            .edit_inactive_xml(&conn, |doc, root| {
                remove_elements(doc, root, "disk", "-1")?;
                Ok(())
            })
            .expect("remove-device's exact call chain succeeds");

        let after_remove = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&after_remove).expect("re-parses");
        let root = doc.root_element().unwrap();
        let remaining = select_elements(&doc, root, "disk", "all")
            .map(|els| els.len())
            .unwrap_or(0);
        assert_eq!(
            remaining, baseline,
            "back to whatever disk count was there before this test"
        );
    }

    const THREE_SOUNDS_FIXTURE: &str = r#"<domain>
  <devices>
    <sound model="sb16"/>
    <sound model="es1370"/>
    <sound model="ich6"/>
  </devices>
</domain>"#;

    /// Golden fixture, pinned against upstream's own
    /// `tests/data/cli/compare/virt-xml-edit-select-disk-target.xml`:
    /// `virt-xml --edit target=hda --disk /dev/null` finds the disk
    /// whose `<target dev="hda"/>` already exists via disk's own
    /// `target`/`target.dev` alias (the same alias
    /// `apply_disk_optstr`'s own match arm uses) — not the bare
    /// `remove_first` splice (`target=hda` already has an `=`).
    #[test]
    fn golden_edit_select_disk_target() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <disk type="file" device="disk">
      <source file="/tmp/foobar"/>
      <target dev="hda" bus="ide"/>
    </disk>
    <disk type="file" device="disk">
      <source file="/tmp/other"/>
      <target dev="hdb" bus="ide"/>
    </disk>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();

        let els = select_elements_by_selector(&doc, root, "disk", "edit", "target=hda").unwrap();
        assert_eq!(els.len(), 1);
        apply_disk_optstr(&mut doc, els[0], "/dev/null").unwrap();

        let all = select_elements(&doc, root, "disk", "all").unwrap();
        assert_eq!(
            all[0].find(&doc, "source").unwrap().attribute(&doc, "file"),
            Some("/dev/null"),
            "the hda disk's path changed"
        );
        assert_eq!(
            all[1].find(&doc, "source").unwrap().attribute(&doc, "file"),
            Some("/tmp/other"),
            "the hdb disk untouched"
        );
    }

    /// Pins upstream's exact error text for a field-match selector that
    /// matches nothing (`tests/test_cli.py`'s
    /// `--edit target=vvv --disk /dev/null` -> grep
    /// `"No matching objects found for --edit target=vvv"`) — the same
    /// text upstream's own `_find_objects_to_edit` produces
    /// (`"No matching objects found for %s" % ("--%s %s" % (action_name,
    /// editval))`).
    #[test]
    fn select_by_field_match_error_text_matches_upstream_exactly() {
        let doc = parse_fixture(THREE_SOUNDS_FIXTURE);
        let root = doc.root_element().unwrap();

        let err =
            select_elements_by_selector(&doc, root, "disk", "edit", "target=vvv").unwrap_err();
        assert_eq!(err, "No matching objects found for --edit target=vvv");
    }

    /// [`field_loc`]'s own doc comment flags it as a hand-maintained
    /// second copy of the alias knowledge each `apply_*_optstr`'s plain
    /// match arms already encode — this is the "one cheap check" that
    /// catches the drift a comment alone can't: every plain (non
    /// present-bool, non-`perms`) cliname each `apply_*_optstr` handles
    /// must have a `field_loc` entry, or a selector naming that field
    /// silently can't match on it with nothing else failing. Update
    /// this list alongside any new plain arm.
    #[test]
    fn field_loc_covers_every_plain_attribute_cliname_each_apply_optstr_handles() {
        let known: &[(&str, &[&str])] = &[(
            "disk",
            &[
                "path",
                "device",
                "type",
                "bus",
                "target.bus",
                "target",
                "target.dev",
                "driver.name",
                "driver.type",
            ],
        )];
        for (device, clinames) in known {
            for cliname in *clinames {
                assert!(
                    field_loc(device, cliname).is_some(),
                    "field_loc({device:?}, {cliname:?}) is missing — apply_{device}_optstr \
                     handles it as a plain attribute, so a selector can't match on it"
                );
            }
        }
    }
}
