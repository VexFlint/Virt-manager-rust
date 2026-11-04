//! Upstream's `virtManager/lib/libvirtenummap.py` — libvirt domain-state
//! enums to the strings and icon names the UI shows — plus the
//! state predicates `vmmDomain` builds on them (`is_runable()`,
//! `is_stoppable()`, …). Pure functions over the raw `virDomainState`
//! value so they're testable without a connection.

use virt::sys;

pub type DomainState = sys::virDomainState;

pub fn pretty_run_status(status: DomainState, has_managed_save: bool) -> &'static str {
    match status {
        sys::VIR_DOMAIN_RUNNING => virtinst_core::tr!("Running"),
        sys::VIR_DOMAIN_PAUSED => virtinst_core::tr!("Paused"),
        sys::VIR_DOMAIN_SHUTDOWN => virtinst_core::tr!("Shutting Down"),
        sys::VIR_DOMAIN_SHUTOFF if has_managed_save => virtinst_core::tr!("Saved"),
        sys::VIR_DOMAIN_SHUTOFF => virtinst_core::tr!("Shutoff"),
        sys::VIR_DOMAIN_CRASHED => virtinst_core::tr!("Crashed"),
        sys::VIR_DOMAIN_PMSUSPENDED => virtinst_core::tr!("Suspended"),
        _ => virtinst_core::tr!("Unknown"),
    }
}

/// `VM_STATUS_ICONS`; unknown states fall back to NOSTATE's icon, as
/// upstream's `run_status_icon_name()` does.
pub fn run_status_icon_name(status: DomainState) -> &'static str {
    match status {
        sys::VIR_DOMAIN_PAUSED | sys::VIR_DOMAIN_PMSUSPENDED => "state_paused",
        sys::VIR_DOMAIN_CRASHED | sys::VIR_DOMAIN_SHUTDOWN | sys::VIR_DOMAIN_SHUTOFF => {
            "state_shutoff"
        }
        _ => "state_running",
    }
}

/// `pretty_status_reason`: the `(reason)` suffix the details window's
/// status line shows. libvirt's reason enums are per-state.
pub fn pretty_status_reason(status: DomainState, reason: i32) -> Option<&'static str> {
    let reasons: &[&str] = match status {
        sys::VIR_DOMAIN_RUNNING => &[
            virtinst_core::tr!("Booted"),
            virtinst_core::tr!("Migrated"),
            virtinst_core::tr!("Restored"),
            virtinst_core::tr!("From snapshot"),
            virtinst_core::tr!("Unpaused"),
            virtinst_core::tr!("Migration canceled"),
            virtinst_core::tr!("Save canceled"),
            virtinst_core::tr!("Event wakeup"),
            virtinst_core::tr!("Crashed"),
        ],
        sys::VIR_DOMAIN_PAUSED => &[
            virtinst_core::tr!("User"),
            virtinst_core::tr!("Migrating"),
            virtinst_core::tr!("Saving"),
            virtinst_core::tr!("Dumping"),
            virtinst_core::tr!("I/O error"),
            virtinst_core::tr!("Watchdog"),
            virtinst_core::tr!("From snapshot"),
            virtinst_core::tr!("Shutting down"),
            virtinst_core::tr!("Creating snapshot"),
            virtinst_core::tr!("Crashed"),
        ],
        sys::VIR_DOMAIN_SHUTDOWN => &[virtinst_core::tr!("User")],
        sys::VIR_DOMAIN_SHUTOFF => &[
            virtinst_core::tr!("Shut Down"),
            virtinst_core::tr!("Destroyed"),
            virtinst_core::tr!("Crashed"),
            virtinst_core::tr!("Migrated"),
            virtinst_core::tr!("Saved"),
            virtinst_core::tr!("Failed"),
            virtinst_core::tr!("From snapshot"),
        ],
        sys::VIR_DOMAIN_CRASHED => &[virtinst_core::tr!("Panicked")],
        _ => &[],
    };
    // Reason 0 is always "unknown", which upstream leaves out.
    usize::try_from(reason)
        .ok()
        .and_then(|r| r.checked_sub(1))
        .and_then(|r| reasons.get(r).copied())
}

pub fn is_crashed(status: DomainState) -> bool {
    status == sys::VIR_DOMAIN_CRASHED
}

pub fn is_shutoff(status: DomainState) -> bool {
    status == sys::VIR_DOMAIN_SHUTOFF
}

pub fn is_stoppable(status: DomainState) -> bool {
    matches!(
        status,
        sys::VIR_DOMAIN_RUNNING
            | sys::VIR_DOMAIN_PAUSED
            | sys::VIR_DOMAIN_CRASHED
            | sys::VIR_DOMAIN_PMSUSPENDED
    )
}

pub fn is_destroyable(status: DomainState) -> bool {
    is_stoppable(status) || status == sys::VIR_DOMAIN_CRASHED
}

pub fn is_runable(status: DomainState) -> bool {
    is_shutoff(status)
}

pub fn is_pauseable(status: DomainState) -> bool {
    status == sys::VIR_DOMAIN_RUNNING
}

pub fn is_unpauseable(status: DomainState) -> bool {
    status == sys::VIR_DOMAIN_PAUSED
}

pub fn is_paused(status: DomainState) -> bool {
    status == sys::VIR_DOMAIN_PAUSED
}

pub fn is_cloneable(status: DomainState) -> bool {
    status == sys::VIR_DOMAIN_SHUTOFF
}

/// `vmmDomain.is_active()`: anything but shutoff.
pub fn is_active(status: DomainState) -> bool {
    status != sys::VIR_DOMAIN_SHUTOFF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_matches_upstream() {
        assert_eq!(pretty_run_status(sys::VIR_DOMAIN_RUNNING, false), "Running");
        assert_eq!(pretty_run_status(sys::VIR_DOMAIN_SHUTOFF, false), "Shutoff");
        assert_eq!(pretty_run_status(sys::VIR_DOMAIN_SHUTOFF, true), "Saved");
        assert_eq!(
            pretty_run_status(sys::VIR_DOMAIN_PMSUSPENDED, false),
            "Suspended"
        );
        assert_eq!(pretty_run_status(999, false), "Unknown");
    }

    #[test]
    fn status_reasons_match_upstream() {
        assert_eq!(
            pretty_status_reason(sys::VIR_DOMAIN_RUNNING, 1),
            Some("Booted")
        );
        assert_eq!(
            pretty_status_reason(sys::VIR_DOMAIN_SHUTOFF, 2),
            Some("Destroyed")
        );
        assert_eq!(pretty_status_reason(sys::VIR_DOMAIN_RUNNING, 0), None);
        assert_eq!(pretty_status_reason(sys::VIR_DOMAIN_SHUTDOWN, 2), None);
    }

    #[test]
    fn icons_match_vm_status_icons() {
        assert_eq!(
            run_status_icon_name(sys::VIR_DOMAIN_BLOCKED),
            "state_running"
        );
        assert_eq!(
            run_status_icon_name(sys::VIR_DOMAIN_CRASHED),
            "state_shutoff"
        );
        assert_eq!(
            run_status_icon_name(sys::VIR_DOMAIN_PMSUSPENDED),
            "state_paused"
        );
        assert_eq!(
            run_status_icon_name(sys::VIR_DOMAIN_NOSTATE),
            "state_running"
        );
        assert_eq!(run_status_icon_name(999), "state_running");
    }

    #[test]
    fn predicates() {
        assert!(is_runable(sys::VIR_DOMAIN_SHUTOFF));
        assert!(!is_runable(sys::VIR_DOMAIN_RUNNING));
        assert!(is_stoppable(sys::VIR_DOMAIN_PMSUSPENDED));
        assert!(!is_stoppable(sys::VIR_DOMAIN_SHUTOFF));
        assert!(is_pauseable(sys::VIR_DOMAIN_RUNNING));
        assert!(!is_pauseable(sys::VIR_DOMAIN_PAUSED));
        assert!(is_unpauseable(sys::VIR_DOMAIN_PAUSED));
        assert!(is_destroyable(sys::VIR_DOMAIN_CRASHED));
        assert!(is_active(sys::VIR_DOMAIN_PAUSED));
        assert!(!is_active(sys::VIR_DOMAIN_SHUTOFF));
    }
}
