//! Runtime support for ticket 06's XPath-binding approach. See
//! `virtinst-xml-derive` for the `#[derive(XmlBound)]` macro that
//! generates code against the traits and helpers here.

pub use edit_xml::{Document, EditXMLError, Element, Node, ReadOptions, Result, WriteOptions};
pub use virtinst_xml_derive::XmlBound;

/// `Document::parse_str`, but relaxed to not require an XML declaration
/// — libvirt's own `virDomainGetXMLDesc`-style output never has one, so
/// requiring it (`edit_xml`'s default) would reject every real document
/// this crate is actually meant to read.
pub fn parse_libvirt_xml(xml: &str) -> Result<Document> {
    Document::parse_str_with_opts(xml, ReadOptions::relaxed())
}

/// Serializes a fresh `T` to a standalone XML fragment — upstream's
/// `dev.get_xml()`, the "fresh creation" path `guest.rs`'s doc comment
/// already named (build the struct directly, no existing document to
/// preserve or diff against). Backs calls like `virDomainAttachDevice`
/// that take one device's XML on its own, not embedded in a document —
/// no `<?xml ...?>` declaration, which would be invalid on a fragment.
pub fn to_xml_string<T: XmlBound>(item: &T) -> Result<String> {
    let mut doc = Document::new();
    let el = Element::new(&mut doc, T::TAG);
    doc.push_root_node(el)?;
    item.write_to(&mut doc, el);
    doc.write_str_with_opts(WriteOptions {
        write_decl: false,
        ..Default::default()
    })
}

/// Implemented by every `#[derive(XmlBound)]` struct: read/write itself
/// against one `edit_xml` element, touching only its own bound fields
/// and never anything else already in the document (ticket 03's
/// round-trip/preservation acceptance bar — see the crate-level test in
/// `virtinst-core` for a concrete check of this).
pub trait XmlBound: Sized {
    /// The XML tag name this struct binds to, e.g. `"disk"`.
    const TAG: &'static str;

    fn from_element(doc: &Document, el: Element) -> Self;
    fn write_to(&self, doc: &mut Document, el: Element);
}

/// How one field's value converts to/from an XML attribute string.
/// `to_attr() -> None` means "don't write this attribute at all" —
/// the mechanism `Option<T>` fields use to stay absent from the XML
/// rather than round-tripping as an empty string.
pub trait XmlAttrValue: Sized {
    fn from_attr(s: Option<&str>) -> Self;
    fn to_attr(&self) -> Option<String>;
}

impl XmlAttrValue for Option<String> {
    fn from_attr(s: Option<&str>) -> Self {
        s.map(str::to_string)
    }
    fn to_attr(&self) -> Option<String> {
        self.clone()
    }
}

impl XmlAttrValue for String {
    fn from_attr(s: Option<&str>) -> Self {
        s.unwrap_or_default().to_string()
    }
    fn to_attr(&self) -> Option<String> {
        Some(self.clone())
    }
}

/// libvirt XML's usual boolean spelling: `yes`/`no`, not `true`/`false`.
/// A **required** yes/no attribute — always writes one or the other.
/// Most yes/no attributes in libvirt XML are actually optional (absent
/// means "use the driver's default", not "false"); for those, bind to
/// `Option<bool>` instead, which stays genuinely unwritten when unset
/// rather than defaulting to `false` and then writing `="no"` on an
/// attribute the source document never had at all.
impl XmlAttrValue for bool {
    fn from_attr(s: Option<&str>) -> Self {
        s == Some("yes")
    }
    fn to_attr(&self) -> Option<String> {
        Some(if *self { "yes" } else { "no" }.to_string())
    }
}

/// The optional counterpart to the `bool` impl above — absent stays
/// absent on write, rather than round-tripping through `false`.
impl XmlAttrValue for Option<bool> {
    fn from_attr(s: Option<&str>) -> Self {
        s.map(|s| s == "yes")
    }
    fn to_attr(&self) -> Option<String> {
        self.map(|b| if b { "yes" } else { "no" }.to_string())
    }
}

/// `is_int=True` attributes (upstream's `xmlbuilder.py` terminology) —
/// `<controller index="0"/>`, `<graphics port="5900"/>`, etc. Unparsable
/// or absent both read as `None`; not distinguished, same as every
/// other field kind treats "couldn't get a value" here.
impl XmlAttrValue for Option<u32> {
    fn from_attr(s: Option<&str>) -> Self {
        s.and_then(|s| s.parse().ok())
    }
    fn to_attr(&self) -> Option<String> {
        self.map(|n| n.to_string())
    }
}

/// libvirt XML's other boolean spelling: `on`/`off` (`is_onoff` in
/// upstream), distinct from `yes`/`no` (`bool`/`Option<bool>` above) —
/// e.g. `<cipher state="on"/>` (`domain/keywrap.py`), `<cpu
/// migratable="on"/>` (`domain/cpu.py`). A newtype rather than
/// overloading `bool` to guess which spelling a given field wants;
/// `Option<OnOff>` is the shape that actually gets used (every
/// `is_onoff` attribute found so far is optional in upstream too).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OnOff(pub bool);

impl XmlAttrValue for Option<OnOff> {
    fn from_attr(s: Option<&str>) -> Self {
        s.map(|s| OnOff(s == "on"))
    }
    fn to_attr(&self) -> Option<String> {
        self.map(|OnOff(b)| if b { "on" } else { "off" }.to_string())
    }
}

/// Walks a `/`-separated chain of child-element names from `current`,
/// using `find()` at each step. An empty `segments` (a field bound
/// directly on its own element, no `path`) returns `current` unchanged.
/// A missing segment anywhere in the chain returns `None` — this is how
/// an unset optional nested field reads back as absent rather than
/// panicking.
pub fn resolve_path(doc: &Document, mut current: Element, segments: &[&str]) -> Option<Element> {
    for seg in segments {
        current = current.find(doc, seg)?;
    }
    Some(current)
}

/// Same walk as [`resolve_path`], but creates each missing segment as a
/// new child element instead of returning `None`. Only ever adds the
/// exact chain asked for — never touches sibling content, which is the
/// entire preservation guarantee this macro exists for.
pub fn resolve_or_create_path(
    doc: &mut Document,
    mut current: Element,
    segments: &[&str],
) -> Element {
    for seg in segments {
        current = match current.find(doc, seg) {
            Some(existing) => existing,
            None => {
                let created = Element::new(doc, *seg);
                current
                    .push_child(doc, created)
                    .expect("a freshly-created detached element always attaches cleanly");
                created
            }
        };
    }
    current
}

/// Removes the element at `segments`, if it's there at all — the write
/// half of `#[xml(present)]` fields (upstream's `is_bool=True`, e.g.
/// `<disk><readonly/></disk>`: the marker element's *presence*, not its
/// value, is the boolean). Only ever touches that one element, never
/// its ancestors — even if removing it leaves an intermediate container
/// empty, that container is left alone, since other fields may still
/// depend on it existing or add to it later.
pub fn remove_path_if_present(
    doc: &mut Document,
    current: Element,
    segments: &[&str],
) -> Result<()> {
    if let Some(target) = resolve_path(doc, current, segments) {
        target.detach(doc)?;
    }
    Ok(())
}

/// Serializes one element and its subtree on its own, e.g. a device
/// for `virDomainUpdateDeviceFlags`. Consumes the document, since the
/// element is moved to be its new root.
pub fn element_to_string(mut doc: Document, el: Element) -> Result<String> {
    el.detach(&mut doc)?;
    doc.container().clear_children(&mut doc);
    doc.push_root_node(el)?;
    doc.write_str()
}

/// Repeated child elements — `#[xml(list)]` on a `Vec<T>` field, e.g.
/// `Vec<DeviceDisk>` under a `<devices>` container. Deliberately
/// **not** a `write_to`/reconcile-a-Vec design: nothing in this project
/// ever needs "take a whole new list, diff it against the document" —
/// every real usage (tickets 10/12: Add Hardware, Remove Device) is a
/// single item added or removed against a live DOM. A bulk rewrite
/// would also actively fight the preservation guarantee, regenerating
/// elements (and losing any of *their* attributes this crate doesn't
/// model) even for logically-unchanged items. So: [`list_read`]
/// collects owned values, [`list_add`]/[`list_remove`] mutate one
/// element at a time, reusing existing elements in place rather than
/// ever clearing and rebuilding the list.
pub fn list_read<T: XmlBound>(doc: &Document, container: Element, path: &[&str]) -> Vec<T> {
    match resolve_path(doc, container, path) {
        Some(list_parent) => list_parent
            .find_all(doc, T::TAG)
            .into_iter()
            .map(|el| T::from_element(doc, el))
            .collect(),
        None => Vec::new(),
    }
}

/// Reads exactly one `T` at `path` + `T::TAG` under `container` —
/// upstream's `is_single=True` `XMLChildProperty`, backing
/// `#[xml(nested)]`. Missing element reads as `T::default()`, same
/// "absent reads as the empty state" rule every other field kind
/// already follows.
///
/// **Read-only, like [`list_read`], for the same reason:** a bare `T`
/// field has no "absent" state distinguishable from "present with all
/// fields at their defaults", so a write arm would have to either
/// unconditionally create the child element on every `write_to` call —
/// inventing `<clock/>`/`<cpu/>`/etc. on documents that never had one,
/// wrecking the round-trip preservation guarantee — or reimplement
/// whole-struct diffing, which ticket 06 already rejected. Mutating a
/// nested struct goes through `find()`-ing its element directly and
/// calling that struct's own `write_to`, exactly how every leaf struct
/// in this crate already gets edited.
pub fn nested_read<T: XmlBound + Default>(doc: &Document, container: Element, path: &[&str]) -> T {
    let mut full_path: Vec<&str> = path.to_vec();
    full_path.push(T::TAG);
    resolve_path(doc, container, &full_path)
        .map(|el| T::from_element(doc, el))
        .unwrap_or_default()
}

/// Appends one new `T` as a child of `container` (creating `path` under
/// it first if missing, same as [`resolve_or_create_path`]), writes
/// `item`'s bound fields into the new element, and returns it — callers
/// that need to track the specific element (e.g. to remove it later)
/// get the handle back rather than having to re-find it.
pub fn list_add<T: XmlBound>(
    doc: &mut Document,
    container: Element,
    path: &[&str],
    item: &T,
) -> Element {
    let list_parent = resolve_or_create_path(doc, container, path);
    let el = Element::new(doc, T::TAG);
    list_parent
        .push_child(doc, el)
        .expect("a freshly-created detached element always attaches cleanly");
    item.write_to(doc, el);
    el
}

/// Removes one element (typically one previously returned by
/// [`list_read`] or [`list_add`]) from its parent, touching nothing
/// else in the document.
pub fn list_remove(doc: &mut Document, item: Element) -> Result<()> {
    item.detach(doc)
}
