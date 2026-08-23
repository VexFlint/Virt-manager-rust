//! Backing logic for `InstallMethod` (ticket 08's Create VM wizard
//! enum, reused as-is by `virt-install` per ticket 16) — upstream's
//! `install/*.py`: cloud-init, unattended-install answer files, URL
//! install-tree detection/fetching.
//!
//! `--unattended`/`--cloud-init` are orthogonal modifiers threaded
//! through as extra fields on whichever `InstallMethod` variants
//! support them (ticket 16) — not separate variants of their own. Not
//! added yet: no real consumer (`virt-install`'s CLI parsing) to
//! ground which variants actually need them against yet, same "wait
//! for a second/real consumer" rule this project has followed since
//! `#[xml(nested)]`.

pub mod container;
mod domain_skeleton;
pub mod import;
pub mod iso;
pub mod manual;
pub use container::build_container_os_document;
pub use domain_skeleton::{default_disk_bus, GuestCapsDefaults};
pub use import::build_import_document;
pub use iso::{build_iso_document, build_iso_two_phase_documents};
pub use manual::{build_manual_document, build_manual_two_phase_documents};

/// One of upstream's 7 install-method sub-flows (`createvm.py`'s
/// `INSTALL_PAGE_*` constants — checked directly against
/// `_validate_install_page`'s actual per-branch required fields, not
/// assumed from the page names). Each variant carries exactly the data
/// its upstream branch requires and nothing shared across variants:
/// `os`/`storage` config are deliberately *not* fields here — ticket
/// 08 keeps those on the (not yet built) wizard-state struct that
/// holds an `InstallMethod`, since every variant but the container
/// ones uses them identically and duplicating an identically-typed
/// field across variants isn't what the enum's exhaustiveness is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallMethod {
    /// `INSTALL_PAGE_ISO` — install from removable/CD media. `media`:
    /// `cdrom` in upstream's `Installer`/`_GuestData`. `live`: boot
    /// straight into the media with no persistent disk attached at all
    /// (`_GuestData.livecd`) — the wizard derives this from "was a
    /// disk actually configured" (`_validate_storage_page`:
    /// `livecd = not storage_enabled`, **only** for this variant, disk
    /// presence means nothing for the other 6); the CLI exposes it
    /// directly as `--livecd`.
    Iso { media: String, live: bool },

    /// `INSTALL_PAGE_URL` — network install tree. `extra_args`:
    /// upstream's `extra`/CLI's repeatable `--extra-args` (`action=
    /// "append"` in `virtinstall.py` — a list, not a single string,
    /// confirmed directly rather than assumed from the wizard's one
    /// text field) — additional kernel command-line arguments,
    /// appended rather than replacing the installer's own (matches
    /// `Installer.set_extra_args`, not `install_kernel_args`, which
    /// upstream reserves for the CLI's separate `--extra-args`-as-
    /// overwrite mode and doesn't appear on the wizard path at all).
    /// Empty vec, not `Option`, for "none given".
    Url {
        location: String,
        extra_args: Vec<String>,
    },

    /// Two upstream origins land here, both "no specific install
    /// media, optionally force a boot device": the wizard's bare
    /// "Manual install" radio button (`INSTALL_PAGE_MANUAL`,
    /// `_validate_install_page` has no branch for it at all — nothing
    /// gets set, `boot_dev: None`) and the CLI's `--pxe`
    /// (`virtinstall.py`: `install_bootdev = "network"`). Not
    /// `no_install = True` in either case — that flag stays unset;
    /// this is "don't fetch or attach anything, just influence what
    /// the guest boots from first", a genuinely different upstream
    /// concept from `Import`'s `no_install`. `boot_dev` is libvirt's
    /// own `<os><boot dev=...>` string (`"network"`, `"hd"`, …),
    /// passed straight through rather than modeled as an enum here —
    /// this crate doesn't validate it any more than it validates
    /// `Iso::media` or `Import::path`.
    Manual { boot_dev: Option<String> },

    /// `INSTALL_PAGE_IMPORT` — an existing disk becomes the VM's disk
    /// as-is, no install phase (`no_install = True`). `path` is
    /// validated to already exist upstream
    /// (`DeviceDisk.path_definitely_exists`) — not enforced by this
    /// type, same as every other field here; validation is a caller
    /// concern, not something baked into a plain data enum.
    Import { path: String },

    /// `INSTALL_PAGE_CONTAINER_APP` — LXC-style: run one application
    /// as the container's init. `init`: an executable path.
    ContainerApp { init: String },

    /// `INSTALL_PAGE_CONTAINER_OS` — LXC-style: an existing directory
    /// tree becomes the container's root filesystem. `source`: that
    /// directory's path (upstream's `fs`). `bootstrap`: optionally
    /// fetch/populate that directory from a container registry first
    /// (upstream's "container bootstrap" checkbox) — `None` means the
    /// directory is assumed to already be populated.
    ContainerOs {
        source: String,
        bootstrap: Option<ContainerOsBootstrap>,
    },

    /// `INSTALL_PAGE_VZ_TEMPLATE` — Virtuozzo container template name.
    VzTemplate { template: String },
}

/// Upstream's container-bootstrap arg bundle (`createvm.py`'s
/// `_start_install`'s `bootstrap_arg_keys`) — fetches/populates
/// `ContainerOs::source` from a container registry before boot, rather
/// than assuming it's already populated. `dest` isn't repeated here —
/// it's `ContainerOs::source`, upstream just passes it twice under a
/// different key name at the call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerOsBootstrap {
    pub source_url: String,
    /// Upstream requires `password` be set whenever `username` is
    /// (`_validate_oscontainer_bootstrap`'s "user without passwd" —
    /// not enforced by this type either, see `InstallMethod::Import`'s
    /// doc comment for why).
    pub username: Option<String>,
    pub password: Option<String>,
    /// Root password to set inside the bootstrapped container.
    pub root_password: Option<String>,
    /// Skip TLS certificate verification against the source registry.
    pub insecure: bool,
}

impl InstallMethod {
    /// Whether the Create VM wizard's separate Storage page applies —
    /// upstream's `_should_skip_disk_page()`, inverted, and now one
    /// source of truth instead of two (that predicate duplicated the
    /// same variant list this `match` covers, maintained separately
    /// from the page-selection logic it mirrored). `Import` skips the
    /// storage page not because it has no disk, but because its one
    /// disk is already fully determined by `path` — there's nothing
    /// left for a separate sizing/creation page to do. The container
    /// variants skip it because containers don't have a disk in this
    /// sense at all.
    ///
    /// `Iso { .. }` returns `true` regardless of `live` — upstream
    /// derives `livecd` *from* the storage page's own outcome
    /// (`_validate_storage_page`: `livecd = not storage_enabled`), so
    /// by the time `live` has a value the storage page has already run;
    /// this method is only ever asked *before* that, when deciding
    /// whether to show the page at all. `Iso { live: true, .. }`
    /// reaching this method with a concrete `live` value only happens
    /// on the CLI's `--livecd` path (no wizard page-flow involved),
    /// where the field is a caller-supplied input rather than a
    /// storage-page-derived output — a real, deliberate difference in
    /// how the two callers populate the same variant, not a bug in
    /// this method.
    pub fn wants_storage_page(&self) -> bool {
        matches!(
            self,
            InstallMethod::Iso { .. } | InstallMethod::Url { .. } | InstallMethod::Manual { .. }
        )
    }
}

// TODO: pub struct CloudInit { ... }
// TODO: pub struct Unattended { ... }
// TODO: pub mod url_detect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_page_wanted_only_for_iso_url_manual() {
        assert!(InstallMethod::Iso {
            media: "x".into(),
            live: false
        }
        .wants_storage_page());
        assert!(InstallMethod::Url {
            location: "x".into(),
            extra_args: vec![]
        }
        .wants_storage_page());
        assert!(InstallMethod::Manual { boot_dev: None }.wants_storage_page());
        assert!(InstallMethod::Manual {
            boot_dev: Some("network".into())
        }
        .wants_storage_page());

        assert!(!InstallMethod::Import { path: "x".into() }.wants_storage_page());
        assert!(!InstallMethod::ContainerApp { init: "x".into() }.wants_storage_page());
        assert!(!InstallMethod::ContainerOs {
            source: "x".into(),
            bootstrap: None
        }
        .wants_storage_page());
        assert!(!InstallMethod::VzTemplate {
            template: "x".into()
        }
        .wants_storage_page());
    }
}
