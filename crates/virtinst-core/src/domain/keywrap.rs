//! `<keywrap>` — upstream's `domain/keywrap.py`. A list of `<cipher>`
//! entries, same `#[xml(list)]` shape `DomainVcpus` already exercises.
//! `state` is the first real use of `Option<OnOff>` on a struct field
//! (the gap that motivated adding it) — `<cipher state="on"/>`, not
//! `yes`/`no`.

use virtinst_xml::{OnOff, XmlBound};

/// A single `<cipher>` entry inside `<keywrap>` — upstream's `_KeyWrap`.
///
/// ```xml
/// <cipher name="aes" state="on"/>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "cipher")]
pub struct KeyWrapCipher {
    #[xml(attribute = "name")]
    pub name: Option<String>,

    #[xml(attribute = "state")]
    pub state: Option<OnOff>,
}

/// ```xml
/// <keywrap>
///   <cipher name="aes" state="on"/>
///   <cipher name="dea" state="off"/>
/// </keywrap>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "keywrap")]
pub struct DomainKeyWrap {
    #[xml(list)]
    pub cipher: Vec<KeyWrapCipher>,
}
