//! Small helpers shared by the in-place device edits (`disk.rs`,
//! `network.rs`): XMLProperty-style get/set of `./child/@attr`.

use virtinst_xml::{Document, Element};

pub(crate) fn attr(doc: &Document, el: Element, path: &[&str], name: &str) -> Option<String> {
    virtinst_xml::resolve_path(doc, el, path)
        .and_then(|c| c.attribute(doc, name))
        .map(str::to_string)
}

/// Sets `path@name`, or removes the attribute (and the element, once it
/// has nothing left) for `None`, like setting an `XMLProperty` to None.
pub(crate) fn set_attr(doc: &mut Document, el: Element, path: &str, name: &str, val: Option<&str>) {
    match val {
        Some(val) => {
            virtinst_xml::resolve_or_create_path(doc, el, &[path]).set_attribute(doc, name, val);
        }
        None => {
            if let Some(child) = el.find(doc, path) {
                child.mut_attributes(doc).remove(name);
                if child.attributes(doc).is_empty() && !child.has_children(doc) {
                    let _ = child.detach(doc);
                }
            }
        }
    }
}
