---
id: 08
title: Create VM wizard
type: grilling
status: resolved
blocked_by: [04, 05, 06, 07]
claimed_by: null
---

## Question

Ticket 07 flagged this as the single biggest/riskiest screen (`createvm.py`,
2106 LOC). Its structure: a `GtkNotebook` state machine of 5 top-level pages
(Name → Install → Memory/CPU → Storage → Finish) where the Install page
itself branches into 7 install-method sub-flows (ISO, URL, PXE/manual,
import-existing-disk, container-app, container-OS, Virtuozzo template), plus
conditional page-skip logic (e.g. the Storage page is skipped entirely for
container installs). It pulls in most of the shared sub-widget layer from
ticket 07 (OS list, storage config, network device panel, media combo,
storage-volume browser) and both `virtinst-core` (ticket 02's XML/domain
layer) and `libosinfo` (ticket 04) to build and validate the final `Guest`
before calling `domain create`.

What's the wizard's state-machine model in `gtk4-rs` — an enum-driven
`GtkStack`/`GtkAssistant`-equivalent with explicit page-skip transitions
mirroring `createvm.py`'s `_get_next_pagenum`/`_should_skip_disk_page`
logic, or something else — and how does per-install-method page validity
(which fields are required/hidden per branch) get represented so it's
exhaustive at compile time rather than the runtime `if instmethod ==`
branching upstream uses?

## Resolution

Grilled across 4 rounds, grounded in `createvm.py`'s actual mechanics
(checked directly, not assumed): upstream's "notebook" is index-addressed
only, tabs hidden, visibility managed by hand; there's exactly one skip rule
(Storage, for import/container-app/container-os/VZ-template); `_gdata` is
mutated incrementally as each page validates; the real `Guest` is built once
at Finish via `_gdata.build_guest()`; OS-detect-on-Next is a self-contained
early-return-then-callback-reinvokes-forward pattern.

**State machine.**
- Page container: `GtkStack` — the direct `gtk4-rs` analog of what upstream
  is actually doing (index-addressed child switching, no tab UI), not a
  replicated `GtkNotebook`-with-hidden-tabs.
- Transitions/skip logic: hand-written `match` functions on a `WizardPage`
  enum, porting `_get_next_pagenum`/`_should_skip_disk_page` directly. One
  real skip rule doesn't justify a generic declarative page-graph engine.
- `WizardState` mutates incrementally — each page's Next-click validates
  that page's raw widget values and writes them in immediately, mirroring
  upstream's per-page `_gdata` writes (e.g. `_validate_mem_page`).
  `InstallMethod` is finalized the moment the Install page validates.
- `WizardState` stays a plain struct with `Option<T>` fields (`name`,
  `install`, `mem`, `os`, `storage`, …), not a typestate/phantom-type chain.
  Step ordering is already enforced procedurally — the only way to reach a
  later page is the Next button — so typestate would be compile-time proof
  of something the UI already makes structurally impossible to violate.
  One final "all required fields are `Some`" check at Finish.

**Data model (the ticket's core "exhaustive at compile time" question).**
- `InstallMethod` is an enum with per-variant required (non-`Option`)
  fields — `Iso { media }`, `Url { location, extra_args }`, `Manual`,
  `Import { path }`, `ContainerApp { .. }`, `ContainerOs { .. }`,
  `VzTemplate { .. }` — replacing upstream's flat ~20-field-optional
  `_GuestData` accumulator. A flat `Option`-heavy struct still lets you
  construct nonsense field combinations the compiler won't catch; the
  enum-of-structs doesn't.
- Storage-page relevance is derived from `InstallMethod::wants_storage_page()`
  rather than a separately-maintained predicate — one source of truth
  instead of upstream's two (the enum, and `_should_skip_disk_page`).
- `os: Option<OsInfo>` (ticket 04) and `storage: Option<StorageConfig>` are
  top-level `WizardState` fields, not duplicated across the 4 `InstallMethod`
  variants that use them. Duplicating an identically-typed field across
  variants to shave one `Option` isn't what the exhaustiveness goal was
  after — that goal was about method-specific *required data* (a URL isn't
  a path isn't a container image), which the enum-of-structs already covers.

**Guest construction.** `WizardState → Guest` bypasses ticket 06's
order-preserving `edit-xml` DOM entirely — direct struct construction,
fresh serialization, matching upstream's `_gdata.build_guest()`. That DOM
layer exists to satisfy ticket 03's round-trip-preservation acceptance bar
for *editing existing* documents; there's no existing document on the
create path for it to preserve. The DOM path belongs to edit-surface
tickets (virt-xml, host/network/pool edit dialogs) when those are opened.

> **Amendment (2026-08-20, `virt-install --import`, ticket 16):** "direct
> struct construction, fresh serialization" turned out not to mean
> `to_xml_string(&guest)` — that call only ever serializes `Guest`'s flat
> attribute/text fields. `devices`, `vcpulist`, and every other
> `#[xml(nested)]`/`#[xml(list)]` field are **read-only by design**
> (`virtinst-xml`'s `nested_read`/`list_read` — the round-trip
> preservation guarantee this ticket's own DOM paragraph above describes
> requires it: no "absent" state distinguishable from "present but
> empty", so `write_to` can never invent an empty `<devices>`/`<vcpu>`
> on a document that never had one). Calling `to_xml_string` on a real
> `Guest` silently drops `<os>`/`<vcpu>`/`<devices>` entirely — libvirt
> rejects the result outright (no `<os>` = unbootable). Never caught
> earlier because the only `to_xml_string` test used `DeviceRng`, an
> all-flat-fields struct.
>
> The actual fresh-creation path (`virt-install --import`, and later
> the real Create VM wizard) has to build the domain XML via direct
> `edit_xml::Document`/`Element` construction — the exact same
> primitives `cloner.rs` already uses for its DOM surgery, applied here
> to a *fresh* document instead of an edited one — not through
> `Guest`'s derive-macro `write_to` at all. `virtinst-xml`'s macro
> itself is not being changed to support this (that would reopen ticket
> 03's guarantee); this is a "build via DOM either way" amendment, not
> a macro fix. See `xml_binding_tests.rs`'s
> `to_xml_string_on_guest_omits_nested_and_list_fields` for the pinned
> gap and `virt-install`'s `--import` implementation for the DOM-
> construction pattern in practice.

**Explicitly out of scope, deferred elsewhere:**
- Async OS-detection gate (`_start_detect_os_if_needed`): spun out as
  **ticket 09** (async/background-task model) — cross-cutting (the
  async-job modal, migrate, and clone all need the same answer), not
  wizard-specific. Ticket 08 doesn't need to reserve any special
  "pending" transition state for it either way — traced the upstream
  callback chain and it's a self-contained early-return-then-the-callback-
  reinvokes-forward pattern, works under any async mechanism ticket 09
  picks.
- "Customize before install" dialog handoff: deferred until the
  VM-details-window screen has its own ticket (ticket 07 flagged it as the
  second-biggest screen, 2522 LOC). Ticket 08 ends at "Finish produces a
  `Guest` + installer, ready to be opened by something."
