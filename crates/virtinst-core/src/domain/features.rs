//! `<features>` — upstream's `domain/features.py`, the file that
//! originally motivated adding `Option<OnOff>` (most of these ~30
//! fields are `is_onoff=True`, not `is_yesno`). Entirely flat — no
//! child structs, no lists, every field a `present` marker or a
//! self/nested attribute. The only real structure is how many
//! different elements/attributes it touches, not any new binding
//! shape.
//!
//! `acpi`/`apic`/`pae`/`hap`/`viridian`/`privnet` are `is_bool=True` —
//! `#[xml(present)]`, same as `DeviceDisk`'s `readonly`. Everything
//! else is `is_onoff=True` (an attribute, always nested one or more
//! segments under its own element) except `gic_version`/
//! `ioapic_driver`/`msrs_unknown` (plain string attributes) and
//! `hyperv_spinlocks_retries` (`is_int=True`).
//!
//! `eoi` binds `./apic/@eoi` — the *same* `apic` path segment `apic`
//! itself (`./apic`, presence-only) resolves against, for a different
//! field kind. Same already-proven pattern as `launch_security.rs`'s
//! `quoteGenerationService`/`quoteGenerationSocket` both landing on
//! one element.

use virtinst_xml::{OnOff, XmlBound};

/// ```xml
/// <features>
///   <acpi/>
///   <apic eoi="on"/>
///   <pae/>
///   <hyperv>
///     <relaxed state="on"/>
///     <vapic state="on"/>
///     <spinlocks state="on" retries="8191"/>
///   </hyperv>
///   <kvm>
///     <hidden state="on"/>
///   </kvm>
///   <vmport state="off"/>
///   <smm state="on"/>
/// </features>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "features")]
pub struct DomainFeatures {
    #[xml(path = "acpi", present)]
    pub acpi: bool,

    #[xml(path = "apic", present)]
    pub apic: bool,

    #[xml(path = "pae", present)]
    pub pae: bool,

    #[xml(path = "gic", attribute = "version")]
    pub gic_version: Option<String>,

    #[xml(path = "hap", present)]
    pub hap: bool,

    #[xml(path = "viridian", present)]
    pub viridian: bool,

    #[xml(path = "privnet", present)]
    pub privnet: bool,

    #[xml(path = "pmu", attribute = "state")]
    pub pmu: Option<OnOff>,

    #[xml(path = "apic", attribute = "eoi")]
    pub eoi: Option<OnOff>,

    #[xml(path = "hyperv/relaxed", attribute = "state")]
    pub hyperv_relaxed: Option<OnOff>,

    #[xml(path = "hyperv/vapic", attribute = "state")]
    pub hyperv_vapic: Option<OnOff>,

    #[xml(path = "hyperv/spinlocks", attribute = "state")]
    pub hyperv_spinlocks: Option<OnOff>,

    #[xml(path = "hyperv/spinlocks", attribute = "retries")]
    pub hyperv_spinlocks_retries: Option<u32>,

    #[xml(path = "hyperv/vpindex", attribute = "state")]
    pub hyperv_vpindex: Option<OnOff>,

    #[xml(path = "hyperv/runtime", attribute = "state")]
    pub hyperv_runtime: Option<OnOff>,

    #[xml(path = "hyperv/synic", attribute = "state")]
    pub hyperv_synic: Option<OnOff>,

    #[xml(path = "hyperv/stimer", attribute = "state")]
    pub hyperv_stimer: Option<OnOff>,

    #[xml(path = "hyperv/stimer/direct", attribute = "state")]
    pub hyperv_stimer_direct: Option<OnOff>,

    #[xml(path = "hyperv/reset", attribute = "state")]
    pub hyperv_reset: Option<OnOff>,

    #[xml(path = "hyperv/frequencies", attribute = "state")]
    pub hyperv_frequencies: Option<OnOff>,

    #[xml(path = "hyperv/reenlightenment", attribute = "state")]
    pub hyperv_reenlightenment: Option<OnOff>,

    #[xml(path = "hyperv/tlbflush", attribute = "state")]
    pub hyperv_tlbflush: Option<OnOff>,

    #[xml(path = "hyperv/tlbflush/direct", attribute = "state")]
    pub hyperv_tlbflush_direct: Option<OnOff>,

    #[xml(path = "hyperv/tlbflush/extended", attribute = "state")]
    pub hyperv_tlbflush_extended: Option<OnOff>,

    #[xml(path = "hyperv/ipi", attribute = "state")]
    pub hyperv_ipi: Option<OnOff>,

    #[xml(path = "hyperv/evmcs", attribute = "state")]
    pub hyperv_evmcs: Option<OnOff>,

    #[xml(path = "hyperv/avic", attribute = "state")]
    pub hyperv_avic: Option<OnOff>,

    #[xml(path = "hyperv/xmm_input", attribute = "state")]
    pub hyperv_xmm_input: Option<OnOff>,

    #[xml(path = "hyperv/emsr_bitmap", attribute = "state")]
    pub hyperv_emsr_bitmap: Option<OnOff>,

    #[xml(path = "vmport", attribute = "state")]
    pub vmport: Option<OnOff>,

    #[xml(path = "kvm/hidden", attribute = "state")]
    pub kvm_hidden: Option<OnOff>,

    #[xml(path = "kvm/hint-dedicated", attribute = "state")]
    pub kvm_hint_dedicated: Option<OnOff>,

    #[xml(path = "kvm/poll-control", attribute = "state")]
    pub kvm_poll_control: Option<OnOff>,

    #[xml(path = "kvm/pv-ipi", attribute = "state")]
    pub kvm_pv_ipi: Option<OnOff>,

    #[xml(path = "pvspinlock", attribute = "state")]
    pub pvspinlock: Option<OnOff>,

    #[xml(path = "smm", attribute = "state")]
    pub smm: Option<OnOff>,

    #[xml(path = "vmcoreinfo", attribute = "state")]
    pub vmcoreinfo: Option<OnOff>,

    #[xml(path = "ioapic", attribute = "driver")]
    pub ioapic_driver: Option<String>,

    #[xml(path = "msrs", attribute = "unknown")]
    pub msrs_unknown: Option<String>,
}

/// `DomainFeatures`, for the CLI parsers.
pub static DOMAIN_FEATURES: crate::xmlbuilder::XmlClass = {
    use crate::xmlbuilder::{prop, Kind, XmlClass};
    XmlClass {
        xml_name: "features",
        props: &[
            prop("acpi", "./acpi", Kind::Bool),
            prop("apic", "./apic", Kind::Bool),
            prop("pae", "./pae", Kind::Bool),
            prop("gic_version", "./gic/@version", Kind::Str),
            prop("hap", "./hap", Kind::Bool),
            prop("viridian", "./viridian", Kind::Bool),
            prop("privnet", "./privnet", Kind::Bool),
            prop("pmu", "./pmu/@state", Kind::OnOff),
            prop("eoi", "./apic/@eoi", Kind::OnOff),
            prop("hyperv_relaxed", "./hyperv/relaxed/@state", Kind::OnOff),
            prop("hyperv_vapic", "./hyperv/vapic/@state", Kind::OnOff),
            prop("hyperv_spinlocks", "./hyperv/spinlocks/@state", Kind::OnOff),
            prop(
                "hyperv_spinlocks_retries",
                "./hyperv/spinlocks/@retries",
                Kind::Int,
            ),
            prop("hyperv_vpindex", "./hyperv/vpindex/@state", Kind::OnOff),
            prop("hyperv_runtime", "./hyperv/runtime/@state", Kind::OnOff),
            prop("hyperv_synic", "./hyperv/synic/@state", Kind::OnOff),
            prop("hyperv_stimer", "./hyperv/stimer/@state", Kind::OnOff),
            prop(
                "hyperv_stimer_direct",
                "./hyperv/stimer/direct/@state",
                Kind::OnOff,
            ),
            prop("hyperv_reset", "./hyperv/reset/@state", Kind::OnOff),
            prop(
                "hyperv_frequencies",
                "./hyperv/frequencies/@state",
                Kind::OnOff,
            ),
            prop(
                "hyperv_reenlightenment",
                "./hyperv/reenlightenment/@state",
                Kind::OnOff,
            ),
            prop("hyperv_tlbflush", "./hyperv/tlbflush/@state", Kind::OnOff),
            prop(
                "hyperv_tlbflush_direct",
                "./hyperv/tlbflush/direct/@state",
                Kind::OnOff,
            ),
            prop(
                "hyperv_tlbflush_extended",
                "./hyperv/tlbflush/extended/@state",
                Kind::OnOff,
            ),
            prop("hyperv_ipi", "./hyperv/ipi/@state", Kind::OnOff),
            prop("hyperv_evmcs", "./hyperv/evmcs/@state", Kind::OnOff),
            prop("hyperv_avic", "./hyperv/avic/@state", Kind::OnOff),
            prop("hyperv_xmm_input", "./hyperv/xmm_input/@state", Kind::OnOff),
            prop(
                "hyperv_emsr_bitmap",
                "./hyperv/emsr_bitmap/@state",
                Kind::OnOff,
            ),
            prop("vmport", "./vmport/@state", Kind::OnOff),
            prop("kvm_hidden", "./kvm/hidden/@state", Kind::OnOff),
            prop(
                "kvm_hint_dedicated",
                "./kvm/hint-dedicated/@state",
                Kind::OnOff,
            ),
            prop("kvm_poll_control", "./kvm/poll-control/@state", Kind::OnOff),
            prop("kvm_pv_ipi", "./kvm/pv-ipi/@state", Kind::OnOff),
            prop("pvspinlock", "./pvspinlock/@state", Kind::OnOff),
            prop("smm", "./smm/@state", Kind::OnOff),
            prop("vmcoreinfo", "./vmcoreinfo/@state", Kind::OnOff),
            prop("ioapic_driver", "./ioapic/@driver", Kind::Str),
            prop("msrs_unknown", "./msrs/@unknown", Kind::Str),
        ],
        children: &[],
        prop_order: &["acpi", "apic", "pae", "gic_version"],
    }
};
