//! `#[derive(XmlBound)]` — generates an `impl virtinst_xml::XmlBound`
//! that reads/writes struct fields against an `edit_xml` element
//! (ticket 06's "typed structs bound to XPaths via a derive macro,
//! operating over a mutable order-preserving DOM" decision).
//!
//! Five field kinds:
//!
//! ```ignore
//! #[derive(XmlBound)]
//! #[xml(tag = "currentMemory")]
//! struct CurrentMemory {
//!     #[xml(attribute = "unit")]
//!     unit: Option<String>,
//!
//!     #[xml(text)]                    // the element's own text content
//!     value: Option<String>,
//! }
//!
//! #[derive(XmlBound)]
//! #[xml(tag = "disk")]
//! struct DeviceDisk {
//!     #[xml(path = "readonly", present)]   // a child element's presence
//!     readonly: bool,
//!
//!     #[xml(list)]
//!     partitions: Vec<Partition>,
//! }
//! ```
//!
//! `path` is a `/`-separated chain of child-element names, resolved with
//! `find()` on read (missing segment ⇒ the field reads as absent/empty)
//! and created with `Element::new` + `push_child` on write if missing —
//! never touching anything else already in the document, which is the
//! entire point (ticket 03's round-trip/preservation acceptance bar).
//! `attribute`, `text`, `list`, and `present` fields can all take a
//! `path`.
//!
//! **`text` caveat, not a footgun to discover at runtime:** `edit_xml`'s
//! `set_text_content` clears *all* children of the target element
//! (including comments/CDATA, not just prior text) before inserting the
//! new text node. Fine for the leaf-text elements libvirt XML actually
//! uses this for (`<description>`, `<title>`, `<currentMemory>`'s
//! value, …) — none of those mix text content with child elements or
//! comments in practice — but `#[xml(text)]` on an element that *does*
//! have other children would silently drop them. Don't put `text` on a
//! field whose element also has structural children.
//!
//! **`present`** is for upstream's `is_bool=True` fields — the
//! *existence* of a child element is the boolean, not any attribute or
//! text on it (e.g. `<disk><readonly/></disk>`). Always a plain `bool`
//! (not `Option<bool>` — existence-or-not is already the two states,
//! there's no third "unset" to represent). Writing `true` creates the
//! element (reusing it if already there); writing `false` removes only
//! that element, never touching its parent even if that leaves the
//! parent otherwise empty.
//!
//! `list` fields are **read-only** through this trait: `from_element`
//! collects every `T::TAG` child into the `Vec`, but `write_to`
//! generates no code for them at all. Mutating a list goes through
//! `virtinst_xml::list_add`/`list_remove` against specific elements
//! instead — see that module's docs for why a whole-Vec reconcile-on-
//! write was deliberately not built.
//!
//! **`nested`** is `list`'s singular counterpart — upstream's
//! `is_single=True` `XMLChildProperty` (`struct Guest { #[xml(nested)]
//! clock: Clock, ... }`, `Clock: XmlBound + Default`). Also **read-only**,
//! same reasoning: a bare `T` can't distinguish "absent" from
//! "present but all-default", so `write_to` never invents the child
//! element. See `virtinst_xml::nested_read`'s docs for the exact same
//! argument `list` already established.

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse_macro_input, Data, DeriveInput, Fields, GenericArgument, LitStr, PathArguments, Type,
};

#[proc_macro_derive(XmlBound, attributes(xml))]
pub fn derive_xml_bound(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

enum FieldKind {
    Attribute { attribute: String },
    Text,
    Present,
    List { item_ty: Type },
    Nested { item_ty: Type },
}

impl FieldKind {
    fn name(&self) -> &'static str {
        match self {
            FieldKind::Attribute { .. } => "attribute",
            FieldKind::Text => "text",
            FieldKind::Present => "present",
            FieldKind::List { .. } => "list",
            FieldKind::Nested { .. } => "nested",
        }
    }
}

struct FieldBinding {
    ident: syn::Ident,
    path: Vec<String>,
    kind: FieldKind,
}

fn expand(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;
    let tag = struct_tag(&input)?;

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input,
            "XmlBound can only be derived for structs",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &input,
            "XmlBound requires named fields",
        ));
    };

    let bindings = fields
        .named
        .iter()
        .map(field_binding)
        .collect::<syn::Result<Vec<_>>>()?;

    let read_fields = bindings.iter().map(|b| {
        let ident = &b.ident;
        let segs = &b.path;
        match &b.kind {
            FieldKind::Attribute { attribute } => quote! {
                #ident: ::virtinst_xml::XmlAttrValue::from_attr(
                    ::virtinst_xml::resolve_path(__doc, __el, &[#(#segs),*])
                        .and_then(|__target| __target.attribute(__doc, #attribute))
                )
            },
            FieldKind::Text => quote! {
                #ident: {
                    let __text = ::virtinst_xml::resolve_path(__doc, __el, &[#(#segs),*])
                        .map(|__target| __target.text_content(__doc));
                    ::virtinst_xml::XmlAttrValue::from_attr(__text.as_deref())
                }
            },
            FieldKind::Present => quote! {
                #ident: ::virtinst_xml::resolve_path(__doc, __el, &[#(#segs),*]).is_some()
            },
            FieldKind::List { item_ty } => quote! {
                #ident: ::virtinst_xml::list_read::<#item_ty>(__doc, __el, &[#(#segs),*])
            },
            FieldKind::Nested { item_ty } => quote! {
                #ident: ::virtinst_xml::nested_read::<#item_ty>(__doc, __el, &[#(#segs),*])
            },
        }
    });

    let write_fields = bindings.iter().filter_map(|b| {
        let ident = &b.ident;
        let segs = &b.path;
        match &b.kind {
            FieldKind::Attribute { attribute } => Some(quote! {
                if let Some(__value) = ::virtinst_xml::XmlAttrValue::to_attr(&self.#ident) {
                    let __target = ::virtinst_xml::resolve_or_create_path(__doc, __el, &[#(#segs),*]);
                    __target.set_attribute(__doc, #attribute, __value);
                }
            }),
            FieldKind::Text => Some(quote! {
                if let Some(__value) = ::virtinst_xml::XmlAttrValue::to_attr(&self.#ident) {
                    let __target = ::virtinst_xml::resolve_or_create_path(__doc, __el, &[#(#segs),*]);
                    __target.set_text_content(__doc, __value);
                }
            }),
            FieldKind::Present => Some(quote! {
                if self.#ident {
                    ::virtinst_xml::resolve_or_create_path(__doc, __el, &[#(#segs),*]);
                } else {
                    ::virtinst_xml::remove_path_if_present(__doc, __el, &[#(#segs),*])
                        .expect("removing a plain child element cannot fail");
                }
            }),
            // list/nested fields are read-only through XmlBound — see
            // the crate-level docs for why. No code generated here.
            FieldKind::List { .. } | FieldKind::Nested { .. } => None,
        }
    });

    Ok(quote! {
        impl ::virtinst_xml::XmlBound for #struct_name {
            const TAG: &'static str = #tag;

            fn from_element(__doc: &::virtinst_xml::Document, __el: ::virtinst_xml::Element) -> Self {
                Self {
                    #(#read_fields),*
                }
            }

            fn write_to(&self, __doc: &mut ::virtinst_xml::Document, __el: ::virtinst_xml::Element) {
                #(#write_fields)*
            }
        }
    })
}

/// Pulls `#[xml(tag = "...")]` off the struct itself.
fn struct_tag(input: &DeriveInput) -> syn::Result<String> {
    for attr in &input.attrs {
        if !attr.path().is_ident("xml") {
            continue;
        }
        let mut tag = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("tag") {
                let value: LitStr = meta.value()?.parse()?;
                tag = Some(value.value());
                Ok(())
            } else {
                Err(meta.error("expected `tag = \"...\"`"))
            }
        })?;
        if let Some(tag) = tag {
            return Ok(tag);
        }
    }
    Err(syn::Error::new_spanned(
        input,
        "XmlBound requires a struct-level #[xml(tag = \"...\")] attribute",
    ))
}

/// Pulls `#[xml(attribute = "...")]`, `#[xml(text)]`, `#[xml(present)]`,
/// or `#[xml(list)]` — each optionally with `path = "..."` — off one
/// field.
fn field_binding(field: &syn::Field) -> syn::Result<FieldBinding> {
    let ident = field
        .ident
        .clone()
        .ok_or_else(|| syn::Error::new_spanned(field, "XmlBound requires named fields"))?;

    let mut path = String::new();
    let mut attribute = None;
    let mut is_list = false;
    let mut is_nested = false;
    let mut is_text = false;
    let mut is_present = false;

    for attr in &field.attrs {
        if !attr.path().is_ident("xml") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("attribute") {
                let value: LitStr = meta.value()?.parse()?;
                attribute = Some(value.value());
                Ok(())
            } else if meta.path.is_ident("path") {
                let value: LitStr = meta.value()?.parse()?;
                path = value.value();
                Ok(())
            } else if meta.path.is_ident("list") {
                is_list = true;
                Ok(())
            } else if meta.path.is_ident("nested") {
                is_nested = true;
                Ok(())
            } else if meta.path.is_ident("text") {
                is_text = true;
                Ok(())
            } else if meta.path.is_ident("present") {
                is_present = true;
                Ok(())
            } else {
                Err(meta.error(
                    "expected `attribute = \"...\"`, `text`, `present`, `list`, \
                     `nested`, and/or `path = \"...\"`",
                ))
            }
        })?;
    }

    let path = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').map(str::to_string).collect()
    };

    let mut kinds = Vec::new();
    if let Some(attribute) = attribute {
        kinds.push(FieldKind::Attribute { attribute });
    }
    if is_text {
        kinds.push(FieldKind::Text);
    }
    if is_present {
        kinds.push(FieldKind::Present);
    }
    if is_list {
        let item_ty = vec_item_type(&field.ty).ok_or_else(|| {
            syn::Error::new_spanned(
                &field.ty,
                "#[xml(list)] requires a Vec<T> field, where T: XmlBound",
            )
        })?;
        kinds.push(FieldKind::List { item_ty });
    }
    if is_nested {
        kinds.push(FieldKind::Nested {
            item_ty: field.ty.clone(),
        });
    }

    let kind = match kinds.len() {
        0 => {
            return Err(syn::Error::new_spanned(
                &ident,
                "field needs one of #[xml(attribute = \"...\")], #[xml(text)], \
                 #[xml(present)], #[xml(list)], or #[xml(nested)] (optionally with \
                 `path = \"...\"`)",
            ));
        }
        1 => kinds.pop().unwrap(),
        _ => {
            let names: Vec<_> = kinds.iter().map(FieldKind::name).collect();
            return Err(syn::Error::new_spanned(
                &ident,
                format!("a field can only be one of {} — pick one", names.join(", ")),
            ));
        }
    };

    Ok(FieldBinding { ident, path, kind })
}

/// Syntactic `Vec<T>` detection — a proc-macro sees only the written
/// type, not its resolved identity, so this is a best-effort check on
/// the last path segment (the standard approach; the same one serde and
/// most other field-attribute derive macros use for this exact case).
fn vec_item_type(ty: &Type) -> Option<Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    if segment.ident != "Vec" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        GenericArgument::Type(inner) => Some(inner.clone()),
        _ => None,
    })
}
