# Vendored edit-xml 0.1.0

Copy of [edit-xml 0.1.0](https://crates.io/crates/edit-xml) (MIT OR
Apache-2.0, see LICENSE-*), wired in through `[patch.crates-io]` in the
workspace `Cargo.toml`.

Changes:

- `utils::HashMap` (the type of element attribute and
namespace maps) is an insertion-ordered map instead of a hash map, so
XML is written with attributes in the order they were parsed or first
set. Upstream virt-manager (libxml2) keeps that order, and the CLI tools
print XML that must match it 1:1.
- The writer escapes attribute values and text like libxml2's
  serializer (`libxml2_escape` in `document.rs`). The original escaped
  attribute values twice and also escaped quotes in text.

Removed: benches, tests, dev-dependencies and the `ahash` dependency
(the `ahash` feature is kept, empty, for compatibility).
