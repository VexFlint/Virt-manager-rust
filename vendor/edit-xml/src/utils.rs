#![allow(clippy::wrong_self_convention)]
pub mod encoding;
use core::str;
use std::ops::Deref;

use quick_xml::{
    escape::resolve_predefined_entity,
    events::{BytesPI, BytesText},
    name::QName,
};
use tracing::debug;

use crate::EditXMLError;
/// Attribute and namespace maps. Vendored change (see VENDORED.md):
/// an insertion-ordered map instead of a hash map, so attributes are
/// written back in the order they were parsed or first set, like
/// libxml2 does. Same method names as `std::collections::HashMap` for
/// what callers use; `remove` keeps the order of the rest.
pub type HashMap<K, V> = OrderedMap<K, V>;

/// A small insertion-ordered map. Elements have a handful of
/// attributes, so a linear scan is as fast as hashing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OrderedMap<K, V> {
    entries: Vec<(K, V)>,
}

impl<K, V> Default for OrderedMap<K, V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<K: Eq, V> OrderedMap<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }

    fn position<Q>(&self, key: &Q) -> Option<usize>
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + ?Sized,
    {
        self.entries.iter().position(|(k, _)| k.borrow() == key)
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + ?Sized,
    {
        self.position(key).map(|i| &self.entries[i].1)
    }

    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + ?Sized,
    {
        self.position(key).map(|i| &mut self.entries[i].1)
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + ?Sized,
    {
        self.position(key).is_some()
    }

    /// Replaces the value in place when the key exists (keeping its
    /// position), else appends.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        match self.position(&key) {
            Some(i) => Some(std::mem::replace(&mut self.entries[i].1, value)),
            None => {
                self.entries.push((key, value));
                None
            }
        }
    }

    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + ?Sized,
    {
        self.position(key).map(|i| self.entries.remove(i).1)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        self.entries.iter_mut().map(|(k, v)| (&*k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, v)| v)
    }

    pub fn retain(&mut self, mut f: impl FnMut(&K, &mut V) -> bool) {
        self.entries.retain_mut(|(k, v)| f(k, v));
    }
}

impl<'a, K, V> IntoIterator for &'a OrderedMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = std::iter::Map<std::slice::Iter<'a, (K, V)>, fn(&'a (K, V)) -> (&'a K, &'a V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().map(|(k, v)| (k, v))
    }
}

impl<K, V> IntoIterator for OrderedMap<K, V> {
    type Item = (K, V);
    type IntoIter = std::vec::IntoIter<(K, V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<K: Eq, V> FromIterator<(K, V)> for OrderedMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = Self::new();
        map.extend(iter);
        map
    }
}

impl<K: Eq, V> Extend<(K, V)> for OrderedMap<K, V> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

#[cfg(test)]
mod ordered_map_tests {
    use super::OrderedMap;

    #[test]
    fn keeps_insertion_order_through_replace_and_remove() {
        let mut m = OrderedMap::new();
        m.insert("dev", 1);
        m.insert("bus", 2);
        m.insert("tray", 3);
        assert_eq!(m.insert("dev", 9), Some(1));
        assert_eq!(m.remove("bus"), Some(2));
        m.insert("bus", 4);
        let keys: Vec<_> = m.keys().copied().collect();
        assert_eq!(keys, ["dev", "tray", "bus"]);
        assert_eq!(m.get("dev"), Some(&9));
    }
}

/// Trait for converting quick-xml types to string
pub trait XMLStringUtils {
    /// Escapes non-ascii characters into their escape sequences
    fn escape_ascii_into_string(&self) -> Result<String, EditXMLError>;
    /// Converts the type into a string
    fn into_string(&self) -> Result<String, EditXMLError>;
    /// Unescapes the content of the type into a string
    fn unescape_to_string(&self) -> Result<String, EditXMLError> {
        let value = self.into_string()?;
        debug!("Unescaping: {}", value);
        let unescape =
            crate::utils::encoding::unescape_with(value.as_str(), resolve_predefined_entity)?;
        debug!("Unescaped: {}", unescape);
        Ok(unescape.into_owned())
    }
}

impl XMLStringUtils for BytesText<'_> {
    fn escape_ascii_into_string(&self) -> Result<String, EditXMLError> {
        Ok(self.escape_ascii().to_string())
    }

    fn into_string(&self) -> Result<String, EditXMLError> {
        String::from_utf8(self.to_vec()).map_err(EditXMLError::from)
    }
    fn unescape_to_string(&self) -> Result<String, EditXMLError> {
        bytes_to_unescaped_string(self.deref())
    }
}
impl XMLStringUtils for QName<'_> {
    fn escape_ascii_into_string(&self) -> Result<String, EditXMLError> {
        self.into_string()
    }

    fn into_string(&self) -> Result<String, EditXMLError> {
        String::from_utf8(self.0.to_vec()).map_err(EditXMLError::from)
    }

    fn unescape_to_string(&self) -> Result<String, EditXMLError> {
        bytes_to_unescaped_string(self.0)
    }
}
impl XMLStringUtils for BytesPI<'_> {
    fn escape_ascii_into_string(&self) -> Result<String, EditXMLError> {
        self.into_string()
    }

    fn into_string(&self) -> Result<String, EditXMLError> {
        Ok(String::from_utf8(self.to_vec())?)
    }

    fn unescape_to_string(&self) -> Result<String, EditXMLError> {
        bytes_to_unescaped_string(self.content())
    }
}
pub(crate) fn bytes_to_unescaped_string(cow: &[u8]) -> Result<String, EditXMLError> {
    let value = str::from_utf8(cow).map_err(EditXMLError::from)?;

    let unescape = crate::utils::encoding::unescape_with(value, resolve_predefined_entity)?;
    Ok(unescape.into_owned())
}
#[cfg(test)]
pub mod tests {
    use std::path::PathBuf;

    use std::sync::Once;
    use tracing::{debug, info};
    use tracing_subscriber::fmt;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    pub fn setup_logger() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let stdout_log = fmt::layer().pretty();
            tracing_subscriber::registry().with(stdout_log).init();
        });
        info!("Logger initialized");
        debug!("Logger initialized");
    }
    pub fn test_dir() -> std::path::PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests")
    }
    pub fn documents_dir() -> std::path::PathBuf {
        test_dir().join("documents")
    }
}
