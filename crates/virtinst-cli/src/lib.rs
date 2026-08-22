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
//! fired here yet even at n=3 (`--watchdog`/`--vsock`/`--disk`): each
//! adds a shape the others don't have (flat self-attrs; one dotted-alias
//! nested element; several nested elements *plus*
//! `#[xml(present)]`-bound existence-booleans *plus* a value-remapping
//! alias like `perms`) — still guessing at the general shape from three
//! points sampled from upstream's real ~57-parser distribution, not
//! discovering it. `--disk` itself is also only a partial slice of
//! upstream's real `ParserDisk` (see `disk_from_optstr`'s own doc
//! comment) — the parts that would most inform a macro design
//! (indexed `source.host[N]*`/`seclabel[N]*` sub-lists, real
//! storage-pool/volume creation) aren't represented in it at all.
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

use virtinst_core::devices::{
    DeviceDisk, DeviceMemballoon, DeviceSound, DeviceVideo, DeviceVsock, DeviceWatchdog,
};
use virtinst_xml::{list_remove, resolve_or_create_path, resolve_path, Document, Element, OnOff};

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

/// `--watchdog` — upstream's `ParserWatchdog` (`remove_first = "model"`,
/// two plain `add_arg` calls, no aliases or custom `_parse`). Unknown
/// clinames are ignored rather than erroring — upstream's real behavior
/// (unrecognized name -> `fail()` with a "no arg with name X" message)
/// isn't ported yet; this slice only needs the mapping to prove out.
pub fn watchdog_from_optstr(optstr: &str) -> Result<DeviceWatchdog, String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["model"]);

    let mut dev = DeviceWatchdog::default();
    for (cliname, val) in tuples {
        match cliname.as_str() {
            "model" => dev.model = required_value(&cliname, val)?,
            "action" => dev.action = required_value(&cliname, val)?,
            _ => {}
        }
    }
    Ok(dev)
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
/// attributes this crate doesn't even model. `key=` (empty value) goes
/// through [`required_value`] like everywhere else — removes the
/// attribute rather than writing `attr=""` (pins `edit-simple-watchdog`
/// against the restored upstream fixture, though that particular
/// fixture only exercises `action=reset`, a real value — the empty-value
/// removal path itself is proven by `edit-simple-vsock` on the sibling
/// device instead, same shared rule).
pub fn apply_watchdog_optstr(doc: &mut Document, el: Element, optstr: &str) -> Result<(), String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["model"]);

    for (cliname, val) in tuples {
        match cliname.as_str() {
            "model" | "action" => match required_value(&cliname, val)? {
                Some(v) => el.set_attribute(doc, cliname, v),
                None => {
                    el.mut_attributes(doc).remove(&cliname);
                }
            },
            _ => {}
        }
    }
    Ok(())
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

/// Upstream's `XMLProperty`'s `is_int=True` setter path
/// (`_convert_value_for_setter`'s `int(val, **intkwargs)`) — raises on
/// unparsable input rather than silently dropping the value, which a
/// bare `.parse().ok()` would do (the same class of bug `parse_onoff`
/// fixed for `cid.auto`, found on `cid.address` while checking the
/// fix's sibling field against upstream). **Known gap**: upstream also
/// accepts `0x`-prefixed hex (`intkwargs["base"] = 16`); this is
/// decimal-only. Not hit by anything built so far — `--vsock cid.address`
/// is the only `is_int` consumer yet; revisit if a real device needs hex.
fn parse_int(key: &str, val: &str) -> Result<u32, String> {
    val.parse().map_err(|_| format!("{key} must be an integer"))
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
            "model" => dev.model = required_value(&cliname, val)?,
            "cid.auto" => {
                dev.auto_cid = match required_value(&cliname, val)? {
                    Some(v) => Some(parse_onoff("cid.auto", &v)?),
                    None => None,
                };
            }
            "cid.address" => {
                dev.cid = match required_value(&cliname, val)? {
                    Some(v) => Some(parse_int("cid.address", &v)?),
                    None => None,
                };
            }
            _ => {}
        }
    }
    Ok(dev)
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

/// [`apply_watchdog_optstr`]'s vsock counterpart — same "mutate only the
/// named attributes on the existing element" approach (including the
/// shared `key=` → remove-the-attribute rule, via [`required_value`]),
/// extended to reach through to the nested `<cid>` child for `cid.*`
/// keys (via [`apply_nested_attr`]) and to [`vsock_from_optstr`]'s
/// positional shorthand and `cid.auto` failure mode. Pins
/// `edit-simple-vsock` against the restored upstream fixture —
/// `cid.address=,cid.auto=on` on a domain with an existing `<cid
/// auto="no" address="5"/>` must produce `<cid auto="yes"/>`, `address`
/// gone entirely.
pub fn apply_vsock_optstr(doc: &mut Document, el: Element, optstr: &str) -> Result<(), String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["model"]);

    for (cliname, val) in tuples {
        match cliname.as_str() {
            "model" => match required_value(&cliname, val)? {
                Some(v) => el.set_attribute(doc, "model", v),
                None => {
                    el.mut_attributes(doc).remove("model");
                }
            },
            "cid.auto" => {
                let normalized = match required_value(&cliname, val)? {
                    Some(v) => Some(
                        if parse_onoff("cid.auto", &v)? {
                            "yes"
                        } else {
                            "no"
                        }
                        .to_string(),
                    ),
                    None => None,
                };
                apply_nested_attr(doc, el, &["cid"], "auto", normalized);
            }
            "cid.address" => {
                let checked = match required_value(&cliname, val)? {
                    Some(v) => {
                        parse_int("cid.address", &v)?;
                        Some(v)
                    }
                    None => None,
                };
                apply_nested_attr(doc, el, &["cid"], "address", checked);
            }
            _ => {}
        }
    }
    Ok(())
}

/// `--memballoon` — upstream's real `ParserMemballoon` (`remove_first =
/// "model"`, same positional shorthand as `--watchdog`/`--vsock`:
/// `--memballoon none` means `model=none`). `autodeflate` and
/// `freePageReporting` are both `is_onoff=True` ([`parse_onoff`]) *and*
/// land on `Option<OnOff>` self-attributes (`DeviceMemballoon`) — unlike
/// `--vsock cid.auto`, no CLI-vocabulary-vs-XML-spelling mismatch to
/// normalize here, upstream's own XML property is `is_onoff` too, so the
/// parsed bool goes straight into `OnOff`. `stats.period` is a dotted
/// nested-element key like `--vsock cid.address`, `is_int=True`
/// ([`parse_int`]).
pub fn memballoon_from_optstr(optstr: &str) -> Result<DeviceMemballoon, String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["model"]);

    let mut dev = DeviceMemballoon::default();
    for (cliname, val) in tuples {
        match cliname.as_str() {
            "model" => dev.model = required_value(&cliname, val)?,
            "autodeflate" => {
                dev.autodeflate = match required_value(&cliname, val)? {
                    Some(v) => Some(OnOff(parse_onoff("autodeflate", &v)?)),
                    None => None,
                };
            }
            "freePageReporting" => {
                dev.free_page_reporting = match required_value(&cliname, val)? {
                    Some(v) => Some(OnOff(parse_onoff("freePageReporting", &v)?)),
                    None => None,
                };
            }
            "stats.period" => {
                dev.stats_period = match required_value(&cliname, val)? {
                    Some(v) => Some(parse_int("stats.period", &v)?),
                    None => None,
                };
            }
            _ => {}
        }
    }
    Ok(dev)
}

/// [`apply_watchdog_optstr`]'s memballoon counterpart. `autodeflate`/
/// `freePageReporting` are self-attributes (unlike `--vsock`'s nested
/// `cid.*`), so they're set/removed directly on `el` like `--watchdog`'s
/// `model`/`action` — just through the shared on/off vocabulary first.
/// `stats.period` still reaches through to a nested `<stats>` child via
/// [`apply_nested_attr`], same shape as `cid.address`.
pub fn apply_memballoon_optstr(
    doc: &mut Document,
    el: Element,
    optstr: &str,
) -> Result<(), String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["model"]);

    for (cliname, val) in tuples {
        match cliname.as_str() {
            "model" => match required_value(&cliname, val)? {
                Some(v) => el.set_attribute(doc, "model", v),
                None => {
                    el.mut_attributes(doc).remove("model");
                }
            },
            "autodeflate" | "freePageReporting" => match required_value(&cliname, val)? {
                Some(v) => {
                    let on = parse_onoff(&cliname, &v)?;
                    el.set_attribute(doc, cliname, if on { "on" } else { "off" });
                }
                None => {
                    el.mut_attributes(doc).remove(&cliname);
                }
            },
            "stats.period" => {
                let checked = match required_value(&cliname, val)? {
                    Some(v) => {
                        parse_int("stats.period", &v)?;
                        Some(v)
                    }
                    None => None,
                };
                apply_nested_attr(doc, el, &["stats"], "period", checked);
            }
            _ => {}
        }
    }
    Ok(())
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
    splice_remove_first(&mut tuples, &["path"]);

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

/// [`apply_watchdog_optstr`]'s disk counterpart — same shape, extended
/// with [`apply_nested_attr`] for the `source`/`target`/`driver`
/// sub-elements and [`apply_present_bool`] for the three
/// existence-is-the-boolean fields. Unlike [`disk_from_optstr`], no
/// `type`/`device` defaulting — editing an existing disk must never
/// invent attributes the optstring didn't name, same "leave everything
/// else alone" rule every `apply_*_optstr` follows.
pub fn apply_disk_optstr(doc: &mut Document, el: Element, optstr: &str) -> Result<(), String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["path"]);

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

/// `--sound` — upstream's real `ParserSound` (`remove_first = "model"`,
/// same positional shorthand as `--watchdog`/`--vsock`/`--memballoon`).
/// `audio.id` is a dotted nested-element key ([`apply_nested_attr`],
/// same shape as `cid.address`/`stats.period`). `multichannel`/
/// `streams` are deliberately **not** validated or normalized —
/// checked directly against upstream's real `add_arg` calls: neither
/// passes `is_onoff=True`, so upstream's own `_VirtCLIArgument.__init__`
/// never runs them through `_on_off_convert` at all, and
/// `XMLProperty._convert_set_value`'s `is_yesno` branch only coerces an
/// actual Python `True`/`False` — a raw string (which is all the CLI
/// ever produces without that flag) passes straight through unexamined.
/// A real, faithfully-reproduced footgun: `--sound multichannel=on`
/// upstream writes `multichannel="on"` verbatim, which is never `"yes"`
/// so reads back as `false` — not ported around, matched. `codec[0-9]*`
/// (indexed sub-list) isn't modeled, same category of gap as `--disk`'s
/// indexed `source.host[N]*`.
pub fn sound_from_optstr(optstr: &str) -> Result<DeviceSound, String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["model"]);

    let mut dev = DeviceSound::default();
    for (cliname, val) in tuples {
        match cliname.as_str() {
            "model" => dev.model = required_value(&cliname, val)?,
            "audio.id" => dev.audio_id = required_value(&cliname, val)?,
            "multichannel" => {
                dev.multichannel = required_value(&cliname, val)?.map(|v| v == "yes");
            }
            "streams" => dev.streams = required_value(&cliname, val)?,
            _ => {}
        }
    }
    Ok(dev)
}

/// [`apply_watchdog_optstr`]'s sound counterpart. `multichannel` writes
/// the raw string verbatim (see [`sound_from_optstr`]'s doc comment for
/// why that's faithful, not a missing validation step) rather than
/// going through `parse_onoff`/a bool.
pub fn apply_sound_optstr(doc: &mut Document, el: Element, optstr: &str) -> Result<(), String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["model"]);

    for (cliname, val) in tuples {
        match cliname.as_str() {
            "model" | "multichannel" | "streams" => match required_value(&cliname, val)? {
                Some(v) => el.set_attribute(doc, cliname, v),
                None => {
                    el.mut_attributes(doc).remove(&cliname);
                }
            },
            "audio.id" => {
                apply_nested_attr(doc, el, &["audio"], "id", required_value(&cliname, val)?)
            }
            _ => {}
        }
    }
    Ok(())
}

/// `--video` — upstream's real `ParserVideo` (`remove_first =
/// "model.type"` — `--video cirrus` means `model.type=cirrus`, spliced
/// into a *dotted* nested-element key rather than a self-attribute, the
/// first `remove_first` target seen so far that isn't flat). Every
/// field except `model.type` itself is a plain `./model/@...`
/// attribute one level down; `model.acceleration.accel3d` reaches a
/// second level (`./model/acceleration/@accel3d`), still just another
/// `apply_nested_attr` path/attr pair, this one with a `/`-joined path.
/// `model.blob` (`is_onoff`, `./model/@blob`) isn't modeled — no
/// `blob` field on `DeviceVideo` yet, same "not built" category as
/// `--disk`'s unported fields, not a new gap invented here.
pub fn video_from_optstr(optstr: &str) -> Result<DeviceVideo, String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["model.type"]);

    let mut dev = DeviceVideo::default();
    for (cliname, val) in tuples {
        match cliname.as_str() {
            "model.type" => dev.model = required_value(&cliname, val)?,
            "model.heads" => {
                dev.heads = match required_value(&cliname, val)? {
                    Some(v) => Some(parse_int(&cliname, &v)?),
                    None => None,
                };
            }
            "model.ram" => {
                dev.ram = match required_value(&cliname, val)? {
                    Some(v) => Some(parse_int(&cliname, &v)?),
                    None => None,
                };
            }
            "model.vram" => {
                dev.vram = match required_value(&cliname, val)? {
                    Some(v) => Some(parse_int(&cliname, &v)?),
                    None => None,
                };
            }
            "model.vram64" => {
                dev.vram64 = match required_value(&cliname, val)? {
                    Some(v) => Some(parse_int(&cliname, &v)?),
                    None => None,
                };
            }
            "model.vgamem" => {
                dev.vgamem = match required_value(&cliname, val)? {
                    Some(v) => Some(parse_int(&cliname, &v)?),
                    None => None,
                };
            }
            "model.acceleration.accel3d" => {
                dev.accel3d = match required_value(&cliname, val)? {
                    Some(v) => Some(parse_onoff(&cliname, &v)?),
                    None => None,
                };
            }
            _ => {}
        }
    }
    Ok(dev)
}

/// [`apply_watchdog_optstr`]'s video counterpart — every field here
/// reaches through [`apply_nested_attr`] to `<model>` or
/// `<model><acceleration>`, none is a self-attribute on `<video>`
/// itself (unlike every device before this one).
pub fn apply_video_optstr(doc: &mut Document, el: Element, optstr: &str) -> Result<(), String> {
    let mut tuples = parse_optstr_tuples(optstr);
    splice_remove_first(&mut tuples, &["model.type"]);

    for (cliname, val) in tuples {
        match cliname.as_str() {
            "model.type" => {
                apply_nested_attr(doc, el, &["model"], "type", required_value(&cliname, val)?)
            }
            "model.heads" => {
                let checked = check_int_or_clear(&cliname, val)?;
                apply_nested_attr(doc, el, &["model"], "heads", checked);
            }
            "model.ram" => {
                let checked = check_int_or_clear(&cliname, val)?;
                apply_nested_attr(doc, el, &["model"], "ram", checked);
            }
            "model.vram" => {
                let checked = check_int_or_clear(&cliname, val)?;
                apply_nested_attr(doc, el, &["model"], "vram", checked);
            }
            "model.vram64" => {
                let checked = check_int_or_clear(&cliname, val)?;
                apply_nested_attr(doc, el, &["model"], "vram64", checked);
            }
            "model.vgamem" => {
                let checked = check_int_or_clear(&cliname, val)?;
                apply_nested_attr(doc, el, &["model"], "vgamem", checked);
            }
            "model.acceleration.accel3d" => {
                let normalized = match required_value(&cliname, val)? {
                    Some(v) => Some(
                        if parse_onoff(&cliname, &v)? {
                            "yes"
                        } else {
                            "no"
                        }
                        .to_string(),
                    ),
                    None => None,
                };
                apply_nested_attr(doc, el, &["model", "acceleration"], "accel3d", normalized);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Shared by every `--video` `is_int` field's `apply_*` half: validate
/// via [`parse_int`], but (like `cid.address`) write the original
/// string back unchanged rather than a reformatted one — an int
/// attribute's XML text is just its decimal digits either way.
fn check_int_or_clear(cliname: &str, val: Option<String>) -> Result<Option<String>, String> {
    match required_value(cliname, val)? {
        Some(v) => {
            parse_int(cliname, &v)?;
            Ok(Some(v))
        }
        None => Ok(None),
    }
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
        let dev = watchdog_from_optstr("default,action=poweroff").unwrap();
        assert_eq!(dev.model.as_deref(), Some("default"));
        assert_eq!(dev.action.as_deref(), Some("poweroff"));
    }

    #[test]
    fn watchdog_from_optstr_handles_the_explicit_form() {
        let dev = watchdog_from_optstr("model=itco,action=none").unwrap();
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
        let dev = watchdog_from_optstr("model=itco,action=none").unwrap();
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

        apply_watchdog_optstr(&mut doc, el, "action=poweroff").unwrap();

        // The named attribute changed...
        assert_eq!(el.attribute(&doc, "action"), Some("poweroff"));
        // ...the untouched one on the same element didn't...
        assert_eq!(el.attribute(&doc, "model"), Some("i6300esb"));
        // ...and the second watchdog wasn't touched at all.
        let els = select_watchdog_elements(&doc, root, "2").unwrap();
        assert_eq!(els[0].attribute(&doc, "action"), Some("none"));
        assert_eq!(els[0].attribute(&doc, "model"), Some("ib700"));
    }

    /// A bare key `splice_remove_first` didn't consume is a hard parse
    /// error upstream (`RuntimeError("Option '%s' had no value set.")`),
    /// not something silently ignored — `--watchdog default,action` has
    /// a recognized `action` key left over with no `=` and nothing left
    /// to splice it into (must be a *recognized* cliname to reach
    /// `required_value` at all — an unrecognized one hits the separate,
    /// already-deferred "unknown cliname" gap in the `_ => {}` arm
    /// instead, a real trap this test's first draft fell into).
    #[test]
    fn watchdog_from_optstr_rejects_a_second_bare_key() {
        assert!(watchdog_from_optstr("default,action").is_err());
    }

    /// Golden fixture, pinned directly from upstream's own
    /// `tests/data/cli/compare/virt-xml-edit-simple-watchdog.xml`
    /// (`~/DEV/virt-manager-upstream-reference`, restored this session):
    /// `virt-xml --edit --watchdog action=reset` against a domain with
    /// `<watchdog model="ib700" action="poweroff"/>` must produce
    /// `<watchdog model="ib700" action="reset"/>` — `model` untouched.
    #[test]
    fn golden_edit_simple_watchdog() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <watchdog model="ib700" action="poweroff"/>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();
        let el = select_watchdog_elements(&doc, root, "1").unwrap()[0];

        apply_watchdog_optstr(&mut doc, el, "action=reset").unwrap();

        assert_eq!(el.attribute(&doc, "model"), Some("ib700"));
        assert_eq!(el.attribute(&doc, "action"), Some("reset"));
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
                    apply_watchdog_optstr(doc, el, "action=poweroff")?;
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
        apply_watchdog_optstr(&mut doc, el, "action=poweroff").unwrap();

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
        let dev = watchdog_from_optstr("model=i6300esb,action=reset").unwrap();
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

    /// Matches `XMLProperty`'s `is_int=True` setter (`int(val)` raises
    /// on unparsable input) — a bare `.parse().ok()` would have silently
    /// dropped `cid.address` to `None` instead, the same class of bug
    /// `cid.auto`'s garbage-rejection test above pins for its own field.
    #[test]
    fn vsock_from_optstr_cid_address_rejects_garbage() {
        assert!(vsock_from_optstr("cid.address=not-a-number").is_err());
    }

    /// Same rejection on the edit path. `cid.address` comes first in
    /// this optstring deliberately (same convention as
    /// `apply_vsock_optstr_rejects_garbage_cid_auto_without_mutating_
    /// anything`): this isn't transactional, an earlier key that
    /// succeeded before a later one fails would stay applied, so the
    /// failing key has to be first to honestly assert "nothing changed".
    #[test]
    fn apply_vsock_optstr_cid_address_rejects_garbage_without_mutating_anything() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <vsock model="virtio"><cid auto="no" address="5"/></vsock>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "vsock", "1").unwrap()[0];

        assert!(apply_vsock_optstr(&mut doc, el, "cid.address=nope,cid.auto=yes").is_err());

        let cid = el.find(&doc, "cid").unwrap();
        assert_eq!(cid.attribute(&doc, "auto"), Some("no"));
        assert_eq!(cid.attribute(&doc, "address"), Some("5"));
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

    /// Pins [`apply_nested_attr`]'s clear-doesn't-create fix: clearing
    /// `cid.address` on a vsock with no `<cid>` at all yet must not
    /// spuriously create an empty one.
    #[test]
    fn apply_vsock_optstr_clearing_an_absent_nested_attr_creates_nothing() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <vsock model="virtio"/>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "vsock", "1").unwrap()[0];

        apply_vsock_optstr(&mut doc, el, "cid.address=").unwrap();

        assert!(el.find(&doc, "cid").is_none(), "no <cid> should exist");
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

    /// Same rule as watchdog's (must be a *recognized* cliname —
    /// `cid.auto`, not an unrecognized one, for the same reason spelled
    /// out on `watchdog_from_optstr_rejects_a_second_bare_key`), proven
    /// on the sibling device too since `required_value` is shared,
    /// device-agnostic logic.
    #[test]
    fn vsock_from_optstr_rejects_a_second_bare_key() {
        assert!(vsock_from_optstr("virtio,cid.auto").is_err());
    }

    /// `key=` (empty value) clears the field on a fresh struct too — a
    /// no-op in practice (nothing was set yet) but proves
    /// `required_value`'s `Ok(None)` path reaches `dev.cid`, not just
    /// the mutate-existing-element path `apply_vsock_optstr` covers.
    #[test]
    fn vsock_from_optstr_empty_value_clears_the_field() {
        let dev = vsock_from_optstr("cid.address=3,cid.address=").unwrap();
        assert_eq!(dev.cid, None);
    }

    /// Golden fixture, pinned directly from upstream's own
    /// `tests/data/cli/compare/virt-xml-edit-simple-vsock.xml`
    /// (`~/DEV/virt-manager-upstream-reference`, restored this session):
    /// `virt-xml --edit --vsock model=virtio,cid.address=,cid.auto=on`
    /// against a domain with `<cid auto="no" address="5"/>` must produce
    /// `<cid auto="yes"/>` — `address` gone entirely, not `address=""`.
    /// This is the fixture that first surfaced the empty-value-means-
    /// clear rule `required_value` exists for.
    #[test]
    fn golden_edit_simple_vsock() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <vsock model="virtio"><cid auto="no" address="5"/></vsock>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "vsock", "1").unwrap()[0];

        apply_vsock_optstr(&mut doc, el, "model=virtio,cid.address=,cid.auto=on").unwrap();

        let cid = el.find(&doc, "cid").unwrap();
        assert_eq!(cid.attribute(&doc, "auto"), Some("yes"));
        assert_eq!(
            cid.attribute(&doc, "address"),
            None,
            "address must be removed, not \"\""
        );
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

    /// `remove_first = "model"` — `--memballoon none` means `model=none`,
    /// same positional shorthand as `--watchdog`/`--vsock`.
    #[test]
    fn memballoon_from_optstr_handles_the_positional_form() {
        let dev = memballoon_from_optstr("none").unwrap();
        assert_eq!(dev.model.as_deref(), Some("none"));
    }

    #[test]
    fn memballoon_from_optstr_parses_onoff_and_int_fields() {
        let dev =
            memballoon_from_optstr("autodeflate=on,freePageReporting=off,stats.period=10").unwrap();
        assert_eq!(dev.autodeflate, Some(OnOff(true)));
        assert_eq!(dev.free_page_reporting, Some(OnOff(false)));
        assert_eq!(dev.stats_period, Some(10));
    }

    #[test]
    fn memballoon_from_optstr_rejects_garbage_onoff() {
        assert!(memballoon_from_optstr("autodeflate=maybe").is_err());
    }

    /// A bare key `splice_remove_first` didn't consume is a hard parse
    /// error, same rule every other device shares — `default,action`
    /// has an unconsumed bare `action` after `default` splices into
    /// `model`.
    #[test]
    fn memballoon_from_optstr_rejects_a_second_bare_key() {
        assert!(memballoon_from_optstr("default,autodeflate").is_err());
    }

    /// Golden fixture, pinned directly from upstream's own
    /// `tests/data/cli/compare/virt-xml-edit-simple-memballoon.xml`:
    /// `virt-xml --edit --memballoon model=none` against
    /// `<memballoon model="virtio"/>` must produce `<memballoon
    /// model="none"/>`.
    #[test]
    fn golden_edit_simple_memballoon() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <memballoon model="virtio"/>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "memballoon", "1").unwrap()[0];

        apply_memballoon_optstr(&mut doc, el, "model=none").unwrap();

        assert_eq!(el.attribute(&doc, "model"), Some("none"));
    }

    /// Same golden output, upstream's other route to it:
    /// `tests/data/cli/compare/virt-xml-edit-disable-memballoon.xml`
    /// is `virt-xml --edit --memballoon none` — the positional-shorthand
    /// form (`remove_first = "model"`), not the explicit `model=none`
    /// `golden_edit_simple_memballoon` already covers. Same underlying
    /// mutation, different optstring shape reaching it.
    #[test]
    fn golden_edit_disable_memballoon() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <memballoon model="virtio"/>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "memballoon", "1").unwrap()[0];

        apply_memballoon_optstr(&mut doc, el, "none").unwrap();

        assert_eq!(el.attribute(&doc, "model"), Some("none"));
    }

    /// Real end-to-end proof, but shaped differently from every other
    /// `add_then_remove_*_against_a_real_domain` test: the real `test://`
    /// driver rejects a second `<memballoon>` outright (`internal error:
    /// only a single memory balloon device is supported` — libvirt-level
    /// validation, discovered empirically when this test first tried the
    /// usual add-a-second/remove-it-by-`-1` shape). That's *why*
    /// upstream's own compare corpus has no `add-memballoon` fixture at
    /// all — a real hardware constraint, not a gap in this crate. So
    /// this instead pins `remove-memballoon`'s actual shape
    /// (`--remove-device --memballoon all` against a domain that already
    /// has exactly one) directly against the stock "test" domain's own
    /// pre-existing memballoon, then restores it afterward so later
    /// tests sharing this domain still see one — the same "leave the
    /// shared fixture domain as found" discipline
    /// `add_then_remove_disk_against_a_real_domain` established via a
    /// baseline count, here via a full snapshot-and-recreate since
    /// removal must actually go to zero to match the fixture.
    #[test]
    fn remove_then_restore_memballoon_against_a_real_domain() {
        use virtinst_core::connection::{Connection, Domain};

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let domain = Domain::lookup_by_name(&conn, "test").expect("the default test domain");

        let before_xml = domain.inactive_xml().expect("fetch inactive XML");
        let before_doc = virtinst_xml::parse_libvirt_xml(&before_xml).expect("re-parses");
        let before_root = before_doc.root_element().unwrap();
        let existing = select_elements(&before_doc, before_root, "memballoon", "all")
            .expect("the stock test domain ships with a memballoon");
        assert_eq!(
            existing.len(),
            1,
            "exactly one, matching the fixture's starting state"
        );
        let original_model = existing[0]
            .attribute(&before_doc, "model")
            .map(String::from);

        domain
            .edit_inactive_xml(&conn, |doc, root| {
                remove_elements(doc, root, "memballoon", "all")?;
                Ok(())
            })
            .expect("remove-device's exact call chain succeeds");

        let after_remove = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&after_remove).expect("re-parses");
        let root = doc.root_element().unwrap();
        assert!(
            select_elements(&doc, root, "memballoon", "all").is_err(),
            "the memballoon is actually gone, matching remove-memballoon's diff"
        );

        // Restore, so tests running later against this same shared
        // domain still find one.
        let restored = DeviceMemballoon {
            model: original_model,
            ..Default::default()
        };
        domain
            .add_device(&conn, &restored)
            .expect("restoring the pre-existing memballoon succeeds");
        let after_restore = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&after_restore).expect("re-parses");
        let root = doc.root_element().unwrap();
        assert_eq!(
            select_elements(&doc, root, "memballoon", "all")
                .unwrap()
                .len(),
            1,
            "back to exactly one, as found"
        );
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
        assert!(
            xml.contains(r#"<target dev="vdf" bus="virtio"/>"#) || {
                // attribute order on <target> isn't guaranteed (see
                // watchdog_build_xml_output_matches_expected_shape's doc
                // comment) — check both orders rather than pin one.
                xml.contains(r#"<target bus="virtio" dev="vdf"/>"#)
            }
        );
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

    #[test]
    fn sound_from_optstr_handles_the_positional_form() {
        let dev = sound_from_optstr("pcspk").unwrap();
        assert_eq!(dev.model.as_deref(), Some("pcspk"));
    }

    /// `multichannel`/`streams` are raw pass-through, not validated —
    /// see `sound_from_optstr`'s doc comment. Garbage that isn't `"yes"`
    /// reads back as `Some(false)` on the `Option<bool>` field (matching
    /// `_convert_get_value`'s own `is_yesno` fallback exactly: anything
    /// other than the literal strings `"yes"`/`"no"` just fails the
    /// equality check), not an error.
    #[test]
    fn sound_from_optstr_multichannel_is_unvalidated_pass_through() {
        let dev = sound_from_optstr("multichannel=on").unwrap();
        assert_eq!(
            dev.multichannel,
            Some(false),
            "\"on\" isn't \"yes\", so it reads back as false, same as upstream"
        );
        let dev = sound_from_optstr("multichannel=yes").unwrap();
        assert_eq!(dev.multichannel, Some(true));
    }

    /// Golden fixture, pinned against upstream's own
    /// `tests/data/cli/compare/virt-xml-edit-simple-soundhw.xml`:
    /// `virt-xml --edit --sound pcspk` against `<sound model="sb16"/>`
    /// must produce `<sound model="pcspk"/>`.
    #[test]
    fn golden_edit_simple_soundhw() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <sound model="sb16"/>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "sound", "1").unwrap()[0];

        apply_sound_optstr(&mut doc, el, "pcspk").unwrap();

        assert_eq!(el.attribute(&doc, "model"), Some("pcspk"));
    }

    /// Real end-to-end proof, same shape as
    /// `add_then_remove_vsock_against_a_real_domain` — pins
    /// `add-sound`'s real fixture shape too (`--add-device --sound
    /// pcspk`, a bare positional model on an otherwise-empty domain).
    #[test]
    fn add_then_remove_sound_against_a_real_domain() {
        use virtinst_core::connection::{Connection, Domain};

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let domain = Domain::lookup_by_name(&conn, "test").expect("the default test domain");

        let dev = sound_from_optstr("pcspk").unwrap();
        domain
            .add_device(&conn, &dev)
            .expect("add-device's exact call chain succeeds");

        let after_add = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&after_add).expect("re-parses");
        let root = doc.root_element().unwrap();
        let sounds = select_elements(&doc, root, "sound", "all").unwrap();
        assert_eq!(sounds.len(), 1);
        assert_eq!(sounds[0].attribute(&doc, "model"), Some("pcspk"));

        domain
            .edit_inactive_xml(&conn, |doc, root| {
                remove_elements(doc, root, "sound", "all")?;
                Ok(())
            })
            .expect("remove-device's exact call chain succeeds");

        let after_remove = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&after_remove).expect("re-parses");
        let root = doc.root_element().unwrap();
        assert!(select_elements(&doc, root, "sound", "all").is_err());
    }

    /// `remove_first = "model.type"` — a dotted nested-element key, the
    /// first positional-shorthand target that isn't a flat self-attribute.
    #[test]
    fn video_from_optstr_handles_the_positional_form() {
        let dev = video_from_optstr("cirrus").unwrap();
        assert_eq!(dev.model.as_deref(), Some("cirrus"));
    }

    #[test]
    fn video_from_optstr_parses_int_and_onoff_fields() {
        let dev = video_from_optstr(
            "model.type=virtio,model.heads=2,model.vram=65536,model.acceleration.accel3d=on",
        )
        .unwrap();
        assert_eq!(dev.model.as_deref(), Some("virtio"));
        assert_eq!(dev.heads, Some(2));
        assert_eq!(dev.vram, Some(65536));
        assert_eq!(dev.accel3d, Some(true));
    }

    /// Golden fixture, pinned against upstream's own
    /// `tests/data/cli/compare/virt-xml-edit-simple-video.xml`:
    /// `virt-xml --edit 1 --video cirrus` against a `<model type="vmvga"
    /// vram="16384" heads="1" primary="yes"/>` must change only `type`
    /// to `cirrus`, every sibling attribute on the same nested `<model>`
    /// element untouched.
    #[test]
    fn golden_edit_simple_video() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <video>
      <model type="vmvga" vram="16384" heads="1" primary="yes"/>
    </video>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "video", "1").unwrap()[0];

        apply_video_optstr(&mut doc, el, "cirrus").unwrap();

        let model = el.find(&doc, "model").unwrap();
        assert_eq!(model.attribute(&doc, "type"), Some("cirrus"));
        assert_eq!(model.attribute(&doc, "vram"), Some("16384"));
        assert_eq!(model.attribute(&doc, "heads"), Some("1"));
        assert_eq!(model.attribute(&doc, "primary"), Some("yes"));
    }

    /// `model.acceleration.accel3d` reaches a *two-level* nested chain
    /// (`<model><acceleration accel3d=.../></model>`) — the first
    /// `apply_nested_attr` caller with more than one path segment.
    /// Regression test for a real bug caught against the compiled
    /// binary: an earlier version passed `"model/acceleration"` as one
    /// literal path string into `resolve_or_create_path(doc, el,
    /// &[path])`, which asked `edit-xml` for a single child tag
    /// literally named `"model/acceleration"` — produced a malformed
    /// `<model/acceleration accel3d="yes"/>` sibling instead of nesting
    /// under the real `<model>`. `apply_nested_attr` takes a segment
    /// slice now, matching `resolve_path`'s own convention.
    #[test]
    fn video_accel3d_edit_reaches_the_nested_acceleration_element_not_a_literal_slash_named_one() {
        let mut doc = parse_fixture(
            r#"<domain>
  <devices>
    <video>
      <model type="virtio" heads="1"/>
    </video>
  </devices>
</domain>"#,
        );
        let root = doc.root_element().unwrap();
        let el = select_elements(&doc, root, "video", "1").unwrap()[0];

        apply_video_optstr(&mut doc, el, "model.acceleration.accel3d=on").unwrap();

        let model = el.find(&doc, "model").unwrap();
        assert_eq!(
            model.attribute(&doc, "type"),
            Some("virtio"),
            "sibling attribute on <model> untouched"
        );
        let acceleration = model
            .find(&doc, "acceleration")
            .expect("<acceleration> nested under <model>, not a sibling of it");
        assert_eq!(acceleration.attribute(&doc, "accel3d"), Some("yes"));
        assert!(
            el.find(&doc, "model/acceleration").is_none(),
            "must not create a literal tag named \"model/acceleration\""
        );
    }

    /// Real end-to-end proof, also pinning `remove-video-all`'s real
    /// fixture shape (`--remove-device --video all`).
    #[test]
    fn add_then_remove_video_against_a_real_domain() {
        use virtinst_core::connection::{Connection, Domain};

        let _guard = TEST_DOMAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open("test:///default").expect("test:/// driver always opens");
        let domain = Domain::lookup_by_name(&conn, "test").expect("the default test domain");

        let dev = video_from_optstr("model.type=cirrus,model.vram=16384").unwrap();
        domain
            .add_device(&conn, &dev)
            .expect("add-device's exact call chain succeeds");

        let after_add = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&after_add).expect("re-parses");
        let root = doc.root_element().unwrap();
        let videos = select_elements(&doc, root, "video", "all").unwrap();
        assert_eq!(videos.len(), 1);

        domain
            .edit_inactive_xml(&conn, |doc, root| {
                remove_elements(doc, root, "video", "all")?;
                Ok(())
            })
            .expect("remove-device's exact call chain succeeds");

        let after_remove = domain.inactive_xml().expect("fetch inactive XML");
        let doc = virtinst_xml::parse_libvirt_xml(&after_remove).expect("re-parses");
        let root = doc.root_element().unwrap();
        assert!(select_elements(&doc, root, "video", "all").is_err());
    }
}
