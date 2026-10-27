//! Upstream's `xmlbuilder.py` and the xpath half of `xmlapi.py`: XML
//! backed objects whose properties map to xpaths, so the CLI parsers
//! (`virtinst-cli`'s port of `VirtCLIParser`) can set `address.type` or
//! `stats_period` by name, on a device being built from scratch
//! (virt-install, `virt-xml --add-device/--build-xml`) or on one parsed
//! from a domain's XML (`virt-xml --edit`).
//!
//! Same model as upstream:
//! - a class ([`XmlClass`]) lists its properties ([`XmlProp`]: an
//!   xpath plus a converter, `XMLProperty`), its child objects
//!   ([`XmlChildProp`], `XMLChildProperty`) and `_XML_PROP_ORDER`;
//! - setting a property only records it in the object's propstore;
//!   the XML is written when the object's XML is asked for (`get_xml`,
//!   `_add_parse_bits`), properties in `_XML_PROP_ORDER` first, so a
//!   freshly built device comes out in upstream's attribute order;
//! - an object parsed from XML reads from that XML until a property is
//!   set, and edits it in place.
//!
//! Objects are backed by the `edit_xml` DOM: the document is shared by
//! a parsed object and its children (`Rc<RefCell<Document>>`), and each
//! object is located by an anchor element plus an xpath relative to it,
//! like upstream's `_XMLState`.

use std::cell::RefCell;
use std::rc::Rc;

use virtinst_xml::{parse_libvirt_xml, Document, Element, Node, WriteOptions};

/// `XMLProperty`'s converter options (`is_bool`, `is_int`, `is_yesno`,
/// `is_onoff`, `do_abspath`), plus plain strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Str,
    /// `is_bool`: the property is the node's presence.
    Bool,
    Int,
    YesNo,
    OnOff,
    AbsPath,
}

/// A Python value as the XML layer sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
}

impl Value {
    /// `str(val)`: what goes into the XML.
    pub fn to_xml_string(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            Value::Int(i) => i.to_string(),
            Value::Bool(true) => "True".into(),
            Value::Bool(false) => "False".into(),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::Str(s.into())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Value {
        Value::Str(s)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Value {
        Value::Int(i)
    }
}

/// `XMLProperty`.
#[derive(Debug)]
pub struct XmlProp {
    pub name: &'static str,
    pub xpath: &'static str,
    pub kind: Kind,
}

/// `XMLChildProperty`.
#[derive(Debug)]
pub struct XmlChildProp {
    pub name: &'static str,
    pub class: &'static XmlClass,
    pub relative_xpath: &'static str,
    pub is_single: bool,
}

/// An `XMLBuilder` subclass: `XML_NAME`, its properties and children,
/// and `_XML_PROP_ORDER`.
#[derive(Debug)]
pub struct XmlClass {
    pub xml_name: &'static str,
    pub props: &'static [XmlProp],
    pub children: &'static [XmlChildProp],
    pub prop_order: &'static [&'static str],
}

impl XmlClass {
    pub fn prop(&self, name: &str) -> Option<&'static XmlProp> {
        self.props.iter().find(|p| p.name == name)
    }

    pub fn child(&self, name: &str) -> Option<&'static XmlChildProp> {
        self.children.iter().find(|c| c.name == name)
    }
}

/// Shorthand for class tables.
pub const fn prop(name: &'static str, xpath: &'static str, kind: Kind) -> XmlProp {
    XmlProp { name, xpath, kind }
}

/// Shorthand for class tables.
pub const fn child(
    name: &'static str,
    class: &'static XmlClass,
    relative_xpath: &'static str,
    is_single: bool,
) -> XmlChildProp {
    XmlChildProp {
        name,
        class,
        relative_xpath,
        is_single,
    }
}

/// A Python-style `ValueError`/`RuntimeError` message.
pub type Error = String;

// ---------------------------------------------------------------------
// xpath subset (`_XPathSegment`/`_XPath`)
// ---------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Segment {
    full: String,
    /// With any `ns:` prefix: edit_xml element names keep it.
    name: String,
    cond_prop: Option<(String, String)>,
    cond_num: Option<usize>,
    is_prop: bool,
}

impl Segment {
    fn parse(full: &str) -> Segment {
        let mut name = full.to_string();
        let mut cond_prop = None;
        let mut cond_num = None;
        if let Some((n, cond)) = full.trim_end_matches(']').split_once('[') {
            name = n.to_string();
            if let Some((p, v)) = cond.split_once('=') {
                cond_prop = Some((
                    p.trim_start_matches('@').to_string(),
                    v.trim_matches('\'').to_string(),
                ));
            } else if let Ok(n) = cond.parse() {
                cond_num = Some(n);
            }
        }
        let is_prop = name.starts_with('@');
        if is_prop {
            name.remove(0);
        }
        Segment {
            full: full.into(),
            name,
            cond_prop,
            cond_num,
            is_prop,
        }
    }
}

/// `_XPath`: the element segments, and the final `@prop` if any.
#[derive(Clone, Debug)]
struct XPath {
    segments: Vec<Segment>,
    propname: Option<String>,
}

impl XPath {
    fn parse(full: &str) -> XPath {
        let mut segments: Vec<Segment> = vec![];
        for s in full.split('/') {
            if s == ".." {
                // Resolve and flatten .. in xpaths
                segments.pop();
                continue;
            }
            segments.push(Segment::parse(s));
        }
        let propname = match segments.last() {
            Some(s) if s.is_prop => Some(segments.pop().unwrap().name),
            _ => None,
        };
        XPath { segments, propname }
    }
}

/// `_join_xpath`.
fn join_xpath(x1: &str, x2: &str) -> String {
    let x2 = x2.strip_prefix('.').unwrap_or(x2);
    format!("{x1}{x2}")
}

fn matches_segment(doc: &Document, el: Element, seg: &Segment) -> bool {
    if el.full_name(doc) != seg.name {
        return false;
    }
    match &seg.cond_prop {
        Some((p, v)) => el.attribute(doc, p) == Some(v.as_str()),
        None => true,
    }
}

/// The element a segment names under `parent`, if it exists.
fn find_child(doc: &Document, parent: Element, seg: &Segment) -> Option<Element> {
    if seg.full == "." {
        return Some(parent);
    }
    let mut matching = parent
        .child_elements(doc)
        .into_iter()
        .filter(|c| matches_segment(doc, *c, seg));
    match seg.cond_num {
        Some(n) => matching.nth(n.checked_sub(1)?),
        None => matching.next(),
    }
}

/// `_find`: the element an xpath (minus any final `@prop`) names.
fn find(doc: &Document, anchor: Element, xpath: &XPath) -> Option<Element> {
    let mut node = anchor;
    for seg in &xpath.segments {
        node = find_child(doc, node, seg)?;
    }
    Some(node)
}

/// `count`.
fn count(doc: &Document, anchor: Element, xpath: &str) -> usize {
    let xpath = XPath::parse(xpath);
    let Some((last, parents)) = xpath.segments.split_last() else {
        return 0;
    };
    let mut node = anchor;
    for seg in parents {
        match find_child(doc, node, seg) {
            Some(n) => node = n,
            None => return 0,
        }
    }
    if last.full == "." {
        return 1;
    }
    let all = node
        .child_elements(doc)
        .into_iter()
        .filter(|c| matches_segment(doc, *c, last))
        .count();
    match last.cond_num {
        Some(n) => usize::from(n >= 1 && n <= all),
        None => all,
    }
}

/// `get_xpath_content`.
fn get_xpath_content(doc: &Document, anchor: Element, xpath: &str, is_bool: bool) -> Option<Value> {
    let x = XPath::parse(xpath);
    let node = find(doc, anchor, &x)?;
    if is_bool {
        return Some(Value::Bool(true));
    }
    match &x.propname {
        Some(p) => node.attribute(doc, p).map(|v| Value::Str(v.into())),
        None => Some(Value::Str(node.text_content(doc))),
    }
}

fn has_content(doc: &Document, el: Element) -> bool {
    !el.attributes(doc).is_empty() || el.has_children(doc)
}

/// `_node_make_stub`: build every missing element of the xpath,
/// filling in `[@prop='val']` conditions.
fn make_stub(doc: &mut Document, anchor: Element, xpath: &XPath) -> Element {
    let mut parent = anchor;
    for seg in &xpath.segments {
        if let Some(n) = find_child(doc, parent, seg) {
            parent = n;
            continue;
        }
        let el = Element::new(doc, seg.name.clone());
        parent
            .push_child(doc, el)
            .expect("a fresh element always attaches");
        if let Some((p, v)) = &seg.cond_prop {
            el.set_attribute(doc, p.clone(), v.clone());
        }
        parent = el;
    }
    parent
}

/// `_node_remove_empty`: walk back up the xpath removing elements left
/// with no attributes or children.
fn remove_empty(doc: &mut Document, anchor: Element, xpath: &XPath) {
    let mut chain = vec![anchor];
    let mut node = anchor;
    for seg in &xpath.segments {
        if seg.full == "." {
            continue;
        }
        match find_child(doc, node, seg) {
            Some(n) => {
                chain.push(n);
                node = n;
            }
            None => break,
        }
    }
    // Never remove the anchor itself.
    while chain.len() > 1 {
        let child = chain.pop().unwrap();
        if has_content(doc, child) {
            break;
        }
        let _ = child.detach(doc);
    }
}

/// `node_force_remove`.
fn force_remove(doc: &mut Document, anchor: Element, xpath: &str) {
    let x = XPath::parse(xpath);
    if x.segments.iter().all(|s| s.full == ".") {
        let _ = anchor.detach(doc);
        return;
    }
    if let Some(n) = find(doc, anchor, &x) {
        let _ = n.detach(doc);
    }
}

/// `set_xpath_content`: `None` clears (and prunes emptied elements),
/// `Bool(false)` removes the node, `Bool(true)` only creates it.
fn set_xpath_content(doc: &mut Document, anchor: Element, xpath: &str, val: Option<&Value>) {
    let x = XPath::parse(xpath);
    match val {
        Some(Value::Bool(false)) => force_remove(doc, anchor, xpath),
        None => {
            if let Some(node) = find(doc, anchor, &x) {
                match &x.propname {
                    Some(p) => {
                        node.mut_attributes(doc).remove(p.as_str());
                    }
                    None => {
                        node.clear_children(doc);
                    }
                }
            }
            remove_empty(doc, anchor, &x);
        }
        Some(v) => {
            let node = make_stub(doc, anchor, &x);
            if matches!(v, Value::Bool(true)) && x.propname.is_none() {
                // Boolean property, creating the node is enough
                return;
            }
            let s = v.to_xml_string();
            match &x.propname {
                Some(p) => node.set_attribute(doc, p.clone(), s),
                None => node.set_text_content(doc, s),
            }
        }
    }
}

/// `node_clear`: drop every attribute and child.
fn node_clear(doc: &mut Document, el: Element) {
    el.mut_attributes(doc).clear();
    el.clear_children(doc);
}

/// Deep copy of `el` from `src` into `dst`, detached.
pub fn copy_element(src: &Document, el: Element, dst: &mut Document) -> Element {
    let new = Element::new(dst, el.full_name(src).to_string());
    for (k, v) in el.attributes(src).iter() {
        new.set_attribute(dst, k.clone(), v.clone());
    }
    for (p, v) in el.namespace_decls(src).iter() {
        new.set_namespace_decl(dst, p.clone(), v.clone());
    }
    for node in el.children(src).clone() {
        let copied = match node {
            Node::Element(c) => Node::Element(copy_element(src, c, dst)),
            other => other,
        };
        new.push_child(dst, copied)
            .expect("a fresh element always attaches");
    }
    new
}

fn write_opts() -> WriteOptions {
    WriteOptions {
        write_decl: false,
        ..Default::default()
    }
}

/// Serializes one element on its own, ending in a newline like
/// upstream's `get_xml`.
pub fn element_xml(doc: &Document, el: Element) -> String {
    let mut out = Document::new();
    let copy = copy_element(doc, el, &mut out);
    out.push_root_node(copy).expect("a single root attaches");
    let mut ret = out.write_str_with_opts(write_opts()).unwrap_or_default();
    ret.push('\n');
    ret
}

// ---------------------------------------------------------------------
// Value conversion (`_convert_set_value`/`_convert_get_value`)
// ---------------------------------------------------------------------

/// Python's `os.path.abspath`.
fn abspath(path: &str) -> String {
    let joined = if path.starts_with('/') {
        path.to_string()
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        format!("{}/{path}", cwd.display())
    };
    let mut parts: Vec<&str> = vec![];
    for part in joined.split('/') {
        match part {
            "" | "." => (),
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Python's `int(val)`, or `int(val, 16)` when `"0x"` is in it, as
/// `XMLProperty` does.
pub fn py_int(val: &str) -> Result<i64, Error> {
    let t = val.trim();
    if val.contains("0x") {
        let (neg, rest) = match t.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, t.strip_prefix('+').unwrap_or(t)),
        };
        let rest = rest.strip_prefix("0x").unwrap_or(rest);
        return i64::from_str_radix(&rest.replace('_', ""), 16)
            .map(|v| if neg { -v } else { v })
            .map_err(|_| format!("invalid literal for int() with base 16: '{val}'"));
    }
    t.replace('_', "")
        .parse::<i64>()
        .map_err(|_| format!("invalid literal for int() with base 10: '{val}'"))
}

fn convert_set(kind: Kind, val: Option<Value>) -> Result<Option<Value>, Error> {
    let Some(val) = val else {
        return Ok(None);
    };
    Ok(Some(match (kind, val) {
        (Kind::AbsPath, Value::Str(s)) => Value::Str(abspath(&s)),
        (Kind::OnOff, Value::Bool(b)) => Value::Str(if b { "on" } else { "off" }.into()),
        (Kind::YesNo, Value::Bool(b)) => Value::Str(if b { "yes" } else { "no" }.into()),
        (Kind::Int, Value::Str(s)) => Value::Int(py_int(&s)?),
        (Kind::Int, Value::Bool(b)) => Value::Int(i64::from(b)),
        (_, v) => v,
    }))
}

fn convert_get(kind: Kind, val: Option<Value>) -> Option<Value> {
    match kind {
        // `bool(val)`
        Kind::Bool => Some(Value::Bool(match val {
            Some(Value::Bool(b)) => b,
            Some(Value::Int(i)) => i != 0,
            Some(Value::Str(s)) => !s.is_empty(),
            None => false,
        })),
        Kind::Int => match val? {
            Value::Str(s) => Some(py_int(&s).map(Value::Int).unwrap_or(Value::Str(s))),
            v => Some(v),
        },
        Kind::YesNo => match val? {
            Value::Str(s) if s == "yes" => Some(Value::Bool(true)),
            Value::Str(s) if s == "no" => Some(Value::Bool(false)),
            v => Some(v),
        },
        Kind::OnOff => match val? {
            Value::Str(s) if s == "on" => Some(Value::Bool(true)),
            Value::Str(s) if s == "off" => Some(Value::Bool(false)),
            v => Some(v),
        },
        Kind::Str | Kind::AbsPath => val,
    }
}

// ---------------------------------------------------------------------
// XmlObject (`XMLBuilder`)
// ---------------------------------------------------------------------

type DocRef = Rc<RefCell<Document>>;

/// One `XMLBuilder` instance.
#[derive(Debug)]
pub struct XmlObject {
    pub class: &'static XmlClass,
    doc: DocRef,
    /// An element that always exists; the object is at `rel` from it.
    anchor: Element,
    /// `_relative_object_xpath`, relative to `anchor`.
    rel: String,
    is_build: bool,
    /// `_propstore`: set values, in the order they were (last) set.
    propstore: Vec<(&'static str, Option<Value>)>,
    /// Child objects per child property, in class order.
    children: Vec<(&'static str, Vec<XmlObject>)>,
    /// `xml_actions`: `--xml`/`xpath.*` manual edits, done last.
    pub xml_actions: Vec<XmlManualAction>,
}

/// `XMLManualAction`: one `xpath.set/create/delete/value` request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XmlManualAction {
    pub xpath_delete: Option<String>,
    pub xpath_create: Option<String>,
    pub xpath_value: Option<String>,
    pub xpath_set: Option<String>,
}

impl XmlManualAction {
    /// Sets one of the four fields by its upstream property name.
    pub fn set(&mut self, name: &str, val: Option<String>) {
        match name {
            "xpath_delete" => self.xpath_delete = val,
            "xpath_create" => self.xpath_create = val,
            "xpath_value" => self.xpath_value = val,
            "xpath_set" => self.xpath_set = val,
            _ => panic!("XMLManualAction has no property {name}"),
        }
    }

    /// `_process_args`: `(xpath, value)`, `Bool(false)` deleting and
    /// `Bool(true)` creating.
    fn process(&self) -> Result<Option<(String, Option<Value>)>, Error> {
        if let Some(x) = &self.xpath_delete {
            return Ok(Some((x.clone(), Some(Value::Bool(false)))));
        }
        if let Some(x) = &self.xpath_create {
            return Ok(Some((x.clone(), Some(Value::Bool(true)))));
        }
        let Some(xpath) = &self.xpath_set else {
            return match &self.xpath_value {
                Some(_) => Err("None: Setting xpath must be in the form of XPATH=VALUE".into()),
                None => Ok(None),
            };
        };
        let (xpath, val) = match &self.xpath_value {
            Some(v) => (xpath.clone(), v.clone()),
            None => match xpath.rsplit_once('=') {
                Some((x, v)) => (x.to_string(), v.to_string()),
                None => {
                    return Err(format!(
                        "{xpath}: Setting xpath must be in the form of XPATH=VALUE"
                    ))
                }
            },
        };
        let val = (!val.is_empty()).then_some(Value::Str(val));
        Ok(Some((xpath, val)))
    }

    /// Checks the request can be performed, before the XML is touched.
    pub fn validate(&self) -> Result<(), Error> {
        self.process().map(|_| ())
    }
}

impl XmlObject {
    /// A new object built from scratch (`Class(conn)`).
    pub fn new(class: &'static XmlClass) -> XmlObject {
        let mut doc = Document::new();
        let root = Element::new(&mut doc, class.xml_name);
        doc.push_root_node(root).expect("a single root attaches");
        let mut obj = XmlObject {
            class,
            doc: Rc::new(RefCell::new(doc)),
            anchor: root,
            rel: ".".into(),
            is_build: true,
            propstore: vec![],
            children: vec![],
            xml_actions: vec![],
        };
        obj.initial_child_parse();
        obj
    }

    /// `Class(conn, parsexml=xml)`.
    pub fn parse(class: &'static XmlClass, xml: &str) -> Result<XmlObject, Error> {
        let doc = parse_libvirt_xml(xml).map_err(|e| e.to_string())?;
        let root = doc.root_element().ok_or("XML has no root element")?;
        let name = root.full_name(&doc).rsplit(':').next().unwrap_or_default();
        if name != class.xml_name.rsplit(':').next().unwrap_or_default() {
            return Err(format!(
                "XML did not have expected root element name '{}', found '{name}'",
                class.xml_name
            ));
        }
        let mut obj = XmlObject {
            class,
            doc: Rc::new(RefCell::new(doc)),
            anchor: root,
            rel: ".".into(),
            is_build: false,
            propstore: vec![],
            children: vec![],
            xml_actions: vec![],
        };
        obj.initial_child_parse();
        Ok(obj)
    }

    fn sub(&self, class: &'static XmlClass, rel: String) -> XmlObject {
        let mut obj = XmlObject {
            class,
            doc: self.doc.clone(),
            anchor: self.anchor,
            rel,
            is_build: self.is_build,
            propstore: vec![],
            children: vec![],
            xml_actions: vec![],
        };
        obj.initial_child_parse();
        obj
    }

    /// `_initial_child_parse`.
    fn initial_child_parse(&mut self) {
        let mut children = vec![];
        for cp in self.class.children {
            let path = self.child_xpath(cp);
            let objs = if cp.is_single {
                vec![self.sub(cp.class, path)]
            } else {
                let n = {
                    let doc = self.doc.borrow();
                    count(&doc, self.anchor, &path)
                };
                (1..=n)
                    .map(|i| self.sub(cp.class, format!("{path}[{i}]")))
                    .collect()
            };
            children.push((cp.name, objs));
        }
        self.children = children;
    }

    /// `get_prop_xpath`, made absolute from the anchor.
    fn child_xpath(&self, cp: &XmlChildProp) -> String {
        join_xpath(
            &self.rel,
            &format!("{}/{}", cp.relative_xpath, cp.class.xml_name),
        )
    }

    fn abs(&self, xpath: &str) -> String {
        join_xpath(&self.rel, xpath)
    }

    pub fn is_build(&self) -> bool {
        self.is_build
    }

    /// A property's value: the set one, else what the XML has.
    pub fn get(&self, name: &str) -> Option<Value> {
        let p = self
            .class
            .prop(name)
            .unwrap_or_else(|| panic!("{} has no property {name}", self.class.xml_name));
        let raw = match self.propstore.iter().find(|(k, _)| *k == name) {
            Some((_, v)) => v.clone(),
            None => {
                let doc = self.doc.borrow();
                get_xpath_content(&doc, self.anchor, &self.abs(p.xpath), p.kind == Kind::Bool)
            }
        };
        convert_get(p.kind, raw)
    }

    /// The value as a string, for the common case.
    pub fn get_str(&self, name: &str) -> Option<String> {
        self.get(name).map(|v| v.to_xml_string())
    }

    /// `setter`: converts and records the value; the XML is written by
    /// [`XmlObject::sync`].
    pub fn set(&mut self, name: &str, val: Option<Value>) -> Result<(), Error> {
        let p = self
            .class
            .prop(name)
            .unwrap_or_else(|| panic!("{} has no property {name}", self.class.xml_name));
        let val = convert_set(p.kind, val)?;
        self.propstore.retain(|(k, _)| *k != p.name);
        self.propstore.push((p.name, val));
        Ok(())
    }

    pub fn has_prop(&self, name: &str) -> bool {
        self.class.prop(name).is_some()
    }

    /// The objects of a child property.
    pub fn children(&self, name: &str) -> &[XmlObject] {
        self.children
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_slice())
            .unwrap_or_else(|| panic!("{} has no child {name}", self.class.xml_name))
    }

    pub fn children_mut(&mut self, name: &str) -> &mut Vec<XmlObject> {
        let class = self.class.xml_name;
        self.children
            .iter_mut()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("{class} has no child {name}"))
    }

    /// An `is_single` child (`address`, `devices`, ...).
    pub fn child(&self, name: &str) -> &XmlObject {
        &self.children(name)[0]
    }

    pub fn child_mut(&mut self, name: &str) -> &mut XmlObject {
        &mut self.children_mut(name)[0]
    }

    /// `xmlutil.get_prop_path`: `address.type` reads through children.
    pub fn get_path(&self, path: &str) -> Option<Value> {
        match path.split_once('.') {
            Some((c, rest)) if self.class.child(c).is_some() => self.child(c).get_path(rest),
            _ => self.get(path),
        }
    }

    /// `xmlutil.set_prop_path`.
    pub fn set_path(&mut self, path: &str, val: Option<Value>) -> Result<(), Error> {
        match path.split_once('.') {
            Some((c, rest)) if self.class.child(c).is_some() => {
                self.child_mut(c).set_path(rest, val)
            }
            _ => self.set(path, val),
        }
    }

    /// The value `val` reads back as once set on property `path`: how
    /// upstream's `lookup_param` compares a CLI value with the XML's.
    pub fn normalize_path(&self, path: &str, val: Option<Value>) -> Result<Option<Value>, Error> {
        match path.split_once('.') {
            Some((c, rest)) if self.class.child(c).is_some() => {
                self.child(c).normalize_path(rest, val)
            }
            _ => {
                let p = self
                    .class
                    .prop(path)
                    .unwrap_or_else(|| panic!("{} has no property {path}", self.class.xml_name));
                let set = convert_set(p.kind, val)?;
                Ok(convert_get(p.kind, set))
            }
        }
    }

    /// Whether `path` names a property, through single children.
    pub fn has_path(&self, path: &str) -> bool {
        match path.split_once('.') {
            Some((c, rest)) if self.class.child(c).is_some() => self.child(c).has_path(rest),
            _ => self.has_prop(path),
        }
    }

    /// The `do_order` of `_do_add_parse_bits`.
    fn apply_order(&self) -> Vec<&'static str> {
        let childnames: Vec<&'static str> = self.class.children.iter().map(|c| c.name).collect();
        let mut do_order: Vec<&'static str> = self
            .propstore
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| !childnames.contains(k))
            .collect();
        for key in self.class.prop_order.iter().rev() {
            if let Some(i) = do_order.iter().position(|k| k == key) {
                let k = do_order.remove(i);
                do_order.insert(0, k);
            } else if childnames.contains(key) {
                do_order.insert(0, key);
            }
        }
        let mut sorted = childnames.clone();
        sorted.sort_unstable();
        for key in sorted {
            if !do_order.contains(&key) {
                do_order.push(key);
            }
        }
        do_order
    }

    /// `_add_parse_bits`: write the propstores into `doc`, `anchor`
    /// standing for this object's own anchor.
    fn apply(&self, doc: &mut Document, anchor: Element) {
        for key in self.apply_order() {
            if let Some(p) = self.class.prop(key) {
                let val = self
                    .propstore
                    .iter()
                    .find(|(k, _)| *k == key)
                    .and_then(|(_, v)| v.clone());
                set_xpath_content(doc, anchor, &self.abs(p.xpath), val.as_ref());
            } else if let Some((_, objs)) = self.children.iter().find(|(n, _)| *n == key) {
                for obj in objs {
                    obj.apply(doc, anchor);
                }
            }
        }
        for action in &self.xml_actions {
            // `perform`; checked by `validate` before.
            if let Ok(Some((xpath, val))) = action.process() {
                let xpath = if xpath.starts_with('.') {
                    self.abs(&xpath)
                } else {
                    xpath
                };
                set_xpath_content(doc, anchor, &xpath, val.as_ref());
            }
        }
    }

    /// Writes pending sets into the backing XML (a parsed object's
    /// `get_xml` side effect).
    pub fn sync(&self) {
        if self.is_build {
            return;
        }
        let mut doc = self.doc.borrow_mut();
        self.apply(&mut doc, self.anchor);
    }

    /// `get_xml`: the object's XML, pending sets applied. A built object
    /// is rendered on a copy, so it can keep changing.
    pub fn get_xml(&self) -> String {
        if self.is_build {
            let src = self.doc.borrow();
            let mut copy = Document::new();
            let root = copy_element(&src, self.anchor, &mut copy);
            copy.push_root_node(root).expect("a single root attaches");
            self.apply(&mut copy, root);
            let x = XPath::parse(&self.rel);
            return match find(&copy, root, &x) {
                Some(el) => element_xml(&copy, el),
                None => String::new(),
            };
        }
        self.sync();
        let doc = self.doc.borrow();
        match find(&doc, self.anchor, &XPath::parse(&self.rel)) {
            Some(el) => element_xml(&doc, el),
            None => String::new(),
        }
    }

    /// The whole backing document (for a parsed top level object).
    pub fn get_document_xml(&self) -> String {
        self.sync();
        let mut ret = self
            .doc
            .borrow()
            .write_str_with_opts(write_opts())
            .unwrap_or_default();
        ret.push('\n');
        ret
    }

    /// Runs `f` on the backing document and this object's element (for
    /// code still editing the DOM directly). Pending sets are written
    /// first.
    pub fn with_doc<R>(&self, f: impl FnOnce(&mut Document, Element) -> R) -> R {
        self.sync();
        let mut doc = self.doc.borrow_mut();
        let el = find(&doc, self.anchor, &XPath::parse(&self.rel))
            .expect("with_doc on an object whose element exists");
        f(&mut doc, el)
    }

    /// [`XmlObject::with_doc`] without writing pending sets first: for
    /// direct edits that must not change when the sets land.
    pub fn with_doc_unsynced<R>(&self, f: impl FnOnce(&mut Document, Element) -> R) -> Option<R> {
        let mut doc = self.doc.borrow_mut();
        let el = find(&doc, self.anchor, &XPath::parse(&self.rel))?;
        Some(f(&mut doc, el))
    }

    /// The element backing this object, when it exists.
    pub fn element(&self) -> Option<Element> {
        let doc = self.doc.borrow();
        find(&doc, self.anchor, &XPath::parse(&self.rel))
    }

    /// `clear(leave_stub)`.
    pub fn clear(&mut self, leave_stub: bool) {
        let names: Vec<&'static str> = self.propstore.iter().map(|(k, _)| *k).collect();
        for name in names {
            let _ = self.set(name, None);
        }
        for (_, objs) in self.children.iter_mut() {
            for obj in objs.iter_mut() {
                obj.clear(false);
            }
        }
        if self.is_build {
            return;
        }
        let is_child = self.rel.ends_with(']');
        let mut doc = self.doc.borrow_mut();
        if is_child || leave_stub {
            if let Some(el) = find(&doc, self.anchor, &XPath::parse(&self.rel)) {
                node_clear(&mut doc, el);
            }
        } else {
            force_remove(&mut doc, self.anchor, &self.rel);
        }
    }

    /// Re-point this object (and its children) at `doc`/`anchor`/`rel`.
    fn rebind(&mut self, doc: DocRef, anchor: Element, rel: String, is_build: bool) {
        self.doc = doc;
        self.anchor = anchor;
        self.is_build = is_build;
        let classes: Vec<&'static XmlChildProp> = self.class.children.iter().collect();
        self.rel = rel;
        for cp in classes {
            let path = self.child_xpath(cp);
            let doc = self.doc.clone();
            let anchor = self.anchor;
            let objs = self.children_mut(cp.name);
            let single = cp.is_single;
            for (i, obj) in objs.iter_mut().enumerate() {
                let rel = if single {
                    path.clone()
                } else {
                    format!("{path}[{}]", i + 1)
                };
                obj.rebind(doc.clone(), anchor, rel, is_build);
            }
        }
    }

    /// `add_child`: append `obj` to the list child property its class
    /// belongs to. Like upstream, an object parsed from XML has that XML
    /// inserted now, while one built from scratch is only written into
    /// this object's XML when that is rendered (`_add_parse_bits`), so
    /// it comes out in its class's property order.
    pub fn add_child(&mut self, mut obj: XmlObject) {
        let cp = self
            .class
            .children
            .iter()
            .find(|c| !c.is_single && std::ptr::eq(c.class, obj.class))
            .unwrap_or_else(|| {
                panic!(
                    "Didn't find child property for child_class={}",
                    obj.class.xml_name
                )
            });
        let path = self.child_xpath(cp);
        let idx = self.children(cp.name).len() + 1;
        if !obj.is_build {
            // Insert the object's XML at the end of the parent node.
            let xml = obj.get_xml();
            let src = parse_libvirt_xml(&xml).expect("rendered XML parses");
            let src_root = src.root_element().expect("rendered XML has a root");
            let mut doc = self.doc.borrow_mut();
            let segments = XPath::parse(&path).segments;
            let parent = XPath {
                segments: segments[..segments.len() - 1].to_vec(),
                propname: None,
            };
            let parent_el = make_stub(&mut doc, self.anchor, &parent);
            let new = copy_element(&src, src_root, &mut doc);
            parent_el
                .push_child(&mut doc, new)
                .expect("a fresh element always attaches");
        }
        let is_build = self.is_build || obj.is_build;
        obj.rebind(
            self.doc.clone(),
            self.anchor,
            format!("{path}[{idx}]"),
            is_build,
        );
        self.children_mut(cp.name).push(obj);
    }

    /// `remove_child`: drop the `idx`th object of child property `name`
    /// from the list and the XML, and return it standing on its own.
    pub fn remove_child(&mut self, name: &str, idx: usize) -> XmlObject {
        let xml = self.children(name)[idx].get_xml();
        let removed = self.children_mut(name).remove(idx);
        if !self.is_build {
            let mut doc = self.doc.borrow_mut();
            force_remove(&mut doc, self.anchor, &removed.rel);
        }
        // Renumber the remaining siblings.
        let cp = self.class.child(name).expect("checked by children()");
        let path = self.child_xpath(cp);
        let doc = self.doc.clone();
        let anchor = self.anchor;
        for (i, obj) in self.children_mut(name).iter_mut().enumerate() {
            let is_build = obj.is_build;
            obj.rebind(doc.clone(), anchor, format!("{path}[{}]", i + 1), is_build);
        }
        let mut standalone = XmlObject::parse(removed.class, &xml).expect("own XML reparses");
        standalone.propstore = removed.propstore;
        standalone
    }

    /// `get_xml_idx`: this object's position among its siblings.
    pub fn xml_idx(&self) -> usize {
        self.rel
            .rsplit_once('[')
            .and_then(|(_, n)| n.trim_end_matches(']').parse::<usize>().ok())
            .map(|n| n - 1)
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------
// Shared device classes (`devices/device.py`)
// ---------------------------------------------------------------------

/// `DeviceVirtioDriver`.
pub static DEVICE_VIRTIO_DRIVER: XmlClass = XmlClass {
    xml_name: "driver",
    props: &[
        prop("ats", "./@ats", Kind::OnOff),
        prop("iommu", "./@iommu", Kind::OnOff),
        prop("packed", "./@packed", Kind::OnOff),
        prop("page_per_vq", "./@page_per_vq", Kind::OnOff),
    ],
    children: &[],
    prop_order: &[],
};

/// `DeviceSeclabel`.
pub static DEVICE_SECLABEL: XmlClass = XmlClass {
    xml_name: "seclabel",
    props: &[
        prop("model", "./@model", Kind::Str),
        prop("relabel", "./@relabel", Kind::YesNo),
        prop("label", "./label", Kind::Str),
    ],
    children: &[],
    prop_order: &[],
};

/// `DeviceAlias`.
pub static DEVICE_ALIAS: XmlClass = XmlClass {
    xml_name: "alias",
    props: &[prop("name", "./@name", Kind::Str)],
    children: &[],
    prop_order: &[],
};

/// `DeviceBoot`.
pub static DEVICE_BOOT: XmlClass = XmlClass {
    xml_name: "boot",
    props: &[
        prop("order", "./@order", Kind::Int),
        prop("loadparm", "./@loadparm", Kind::Str),
    ],
    children: &[],
    prop_order: &[],
};

/// `DeviceAddress`.
pub static DEVICE_ADDRESS: XmlClass = XmlClass {
    xml_name: "address",
    props: &[
        prop("type", "./@type", Kind::Str),
        prop("domain", "./@domain", Kind::Int),
        prop("bus", "./@bus", Kind::Int),
        prop("slot", "./@slot", Kind::Int),
        prop("function", "./@function", Kind::Int),
        prop("multifunction", "./@multifunction", Kind::OnOff),
        prop("zpci_uid", "./zpci/@uid", Kind::Str),
        prop("zpci_fid", "./zpci/@fid", Kind::Str),
        prop("controller", "./@controller", Kind::Int),
        prop("unit", "./@unit", Kind::Int),
        prop("port", "./@port", Kind::Int),
        prop("target", "./@target", Kind::Int),
        prop("reg", "./@reg", Kind::Str),
        prop("cssid", "./@cssid", Kind::Str),
        prop("ssid", "./@ssid", Kind::Str),
        prop("devno", "./@devno", Kind::Str),
        prop("iobase", "./@iobase", Kind::Str),
        prop("irq", "./@irq", Kind::Str),
        prop("base", "./@base", Kind::Str),
    ],
    children: &[],
    prop_order: &[
        "type",
        "domain",
        "controller",
        "bus",
        "slot",
        "function",
        "target",
        "unit",
        "multifunction",
    ],
};

/// The children every `Device` has (`alias`, `address`, `boot`,
/// `virtio_driver`).
#[macro_export]
macro_rules! device_children {
    ($($extra:expr),* $(,)?) => {
        &[
            $crate::xmlbuilder::child("alias", &$crate::xmlbuilder::DEVICE_ALIAS, ".", true),
            $crate::xmlbuilder::child("address", &$crate::xmlbuilder::DEVICE_ADDRESS, ".", true),
            $crate::xmlbuilder::child("boot", &$crate::xmlbuilder::DEVICE_BOOT, ".", true),
            $crate::xmlbuilder::child(
                "virtio_driver",
                &$crate::xmlbuilder::DEVICE_VIRTIO_DRIVER,
                ".",
                true,
            ),
            $($extra),*
        ]
    };
}

/// A `Device` subclass's `_XML_PROP_ORDER`: its own, then
/// `Device.__init__`'s `["virtio_driver", "alias", "address"]`.
#[macro_export]
macro_rules! device_prop_order {
    ($($p:expr),* $(,)?) => {
        &[$($p,)* "virtio_driver", "alias", "address"]
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    static WATCHDOG: XmlClass = XmlClass {
        xml_name: "watchdog",
        props: &[
            prop("model", "./@model", Kind::Str),
            prop("action", "./@action", Kind::Str),
        ],
        children: device_children!(),
        prop_order: device_prop_order!("model", "action"),
    };

    static MEMBALLOON: XmlClass = XmlClass {
        xml_name: "memballoon",
        props: &[
            prop("model", "./@model", Kind::Str),
            prop("autodeflate", "./@autodeflate", Kind::OnOff),
            prop("stats_period", "./stats/@period", Kind::Int),
            prop("freePageReporting", "./@freePageReporting", Kind::OnOff),
        ],
        children: device_children!(),
        prop_order: device_prop_order!(),
    };

    static DEVICES: XmlClass = XmlClass {
        xml_name: "devices",
        props: &[],
        children: &[child("watchdog", &WATCHDOG, ".", false)],
        prop_order: &[],
    };

    static DOMAIN: XmlClass = XmlClass {
        xml_name: "domain",
        props: &[prop("name", "./name", Kind::Str)],
        children: &[child("devices", &DEVICES, ".", true)],
        prop_order: &[],
    };

    #[test]
    fn built_object_uses_prop_order() {
        let mut w = XmlObject::new(&WATCHDOG);
        w.set("action", Some("reset".into())).unwrap();
        w.set_path("address.type", Some("isa".into())).unwrap();
        w.set("model", Some("ib700".into())).unwrap();
        assert_eq!(
            w.get_xml(),
            "<watchdog model=\"ib700\" action=\"reset\">\n  <address type=\"isa\"/>\n</watchdog>\n"
        );
    }

    #[test]
    fn converters_follow_upstream() {
        let mut m = XmlObject::new(&MEMBALLOON);
        m.set("autodeflate", Some(Value::Bool(true))).unwrap();
        m.set("stats_period", Some("0x10".into())).unwrap();
        assert_eq!(m.get("stats_period"), Some(Value::Int(16)));
        assert_eq!(m.get("autodeflate"), Some(Value::Bool(true)));
        assert_eq!(
            m.get_xml(),
            "<memballoon autodeflate=\"on\">\n  <stats period=\"16\"/>\n</memballoon>\n"
        );
        assert_eq!(
            m.set("stats_period", Some("ten".into())).unwrap_err(),
            "invalid literal for int() with base 10: 'ten'"
        );
    }

    #[test]
    fn parsed_object_edits_in_place_and_prunes() {
        let xml = "<domain><name>x</name><devices>\
                   <watchdog model='i6300esb' action='poweroff'>\
                   <address type='pci' bus='0x00'/></watchdog>\
                   <watchdog model='ib700'/></devices></domain>";
        let mut d = XmlObject::parse(&DOMAIN, xml).unwrap();
        let wds = d.child_mut("devices").children_mut("watchdog");
        assert_eq!(wds.len(), 2);
        assert_eq!(wds[1].get_str("model").as_deref(), Some("ib700"));
        wds[0].set("action", Some("reset".into())).unwrap();
        wds[0].set_path("address.type", None).unwrap();
        wds[0].set_path("address.bus", None).unwrap();
        let out = d.get_document_xml();
        assert!(
            out.contains("<watchdog model=\"i6300esb\" action=\"reset\"/>"),
            "{out}"
        );

        let mut new = XmlObject::new(&WATCHDOG);
        new.set("model", Some("diag288".into())).unwrap();
        d.child_mut("devices").add_child(new);
        let removed = d.child_mut("devices").remove_child("watchdog", 1);
        assert_eq!(removed.get_str("model").as_deref(), Some("ib700"));
        let out = d.get_document_xml();
        assert!(out.contains("<watchdog model=\"diag288\"/>"), "{out}");
        assert!(!out.contains("ib700"), "{out}");
        let wds = d.child("devices").children("watchdog");
        assert_eq!(wds[1].get_str("model").as_deref(), Some("diag288"));
        assert_eq!(wds[1].xml_idx(), 1);
    }

    #[test]
    fn py_int_matches_python() {
        assert_eq!(py_int("10"), Ok(10));
        assert_eq!(py_int("0x1F"), Ok(31));
        assert_eq!(py_int(" 7 "), Ok(7));
        assert!(py_int("1.5").is_err());
    }
}
