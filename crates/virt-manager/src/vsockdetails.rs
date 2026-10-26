//! `vmmVsockDetails` (`device/vsockdetails.py`): the guest CID, picked
//! by libvirt or set by hand.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use virtinst_core::devices::vsock::MIN_GUEST_CID;
use virtinst_xml::{Document, Element};

use crate::devpages::Grid;

const AUTO_LABEL: &str = "A_uto";

pub struct VsockDetails {
    pub top_box: gtk::Box,
    pub auto: gtk::CheckButton,
    pub cid: gtk::SpinButton,
    changed_auto_cid: RefCell<Option<Box<dyn Fn()>>>,
}

impl VsockDetails {
    pub fn new() -> Rc<VsockDetails> {
        let auto = gtk::CheckButton::with_mnemonic(AUTO_LABEL);
        auto.set_halign(gtk::Align::Start);
        let cid = gtk::SpinButton::with_range(MIN_GUEST_CID as f64, u32::MAX as f64, 1.0);
        cid.set_numeric(true);
        cid.set_climb_rate(1.0);
        cid.set_snap_to_ticks(true);
        cid.set_update_policy(gtk::SpinButtonUpdatePolicy::IfValid);
        let cid_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        cid_box.append(&auto);
        cid_box.append(&cid);
        let mut g = Grid::new();
        let row = g.row("Guest C_ID:", &cid_box);
        row.set_mnemonic(&cid);
        let top_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        top_box.append(&g.into_grid());

        let this = Rc::new(VsockDetails {
            top_box,
            auto,
            cid,
            changed_auto_cid: RefCell::default(),
        });
        let weak = Rc::downgrade(&this);
        this.auto.connect_toggled(move |_| {
            if let Some(this) = weak.upgrade() {
                this.vsock_auto_toggled();
            }
        });
        this
    }

    /// The `changed-auto-cid` signal; `changed-cid` is the spin
    /// button's own `value-changed`.
    pub fn connect_changed_auto_cid(&self, cb: impl Fn() + 'static) {
        self.changed_auto_cid.replace(Some(Box::new(cb)));
    }

    pub fn reset_state(&self) {
        self.auto.set_active(true);
        self.cid.set_value(MIN_GUEST_CID as f64);
        self.cid.set_visible(false);
    }

    /// `get_values`: `(auto_cid, cid)`.
    pub fn get_values(&self) -> (bool, u32) {
        self.cid.update();
        (self.auto.is_active(), self.cid.value() as u32)
    }

    /// `set_dev`. `vm_active` adds the CID libvirt picked to the Auto
    /// label.
    pub fn set_dev(&self, doc: &Document, vsock: Element, vm_active: bool) {
        let cid_el = vsock.find(doc, "cid");
        let attr = |name| cid_el.and_then(|c| c.attribute(doc, name));
        self.reset_state();

        let is_auto = attr("auto") == Some("yes");
        let cid = attr("address")
            .and_then(|a| a.parse::<u32>().ok())
            .filter(|c| *c != 0)
            .unwrap_or(MIN_GUEST_CID);

        let mut label = AUTO_LABEL.to_string();
        if is_auto && vm_active {
            label += &format!(" (CID {cid})");
        }
        self.auto.set_label(Some(&label));
        self.auto.set_use_underline(true);

        self.auto.set_active(is_auto);
        self.cid.set_value(cid as f64);
        self.cid.set_visible(!is_auto);
    }

    /// `_vsock_auto_toggled`.
    fn vsock_auto_toggled(&self) {
        self.cid.set_visible(!self.auto.is_active());
        if let Some(cb) = &*self.changed_auto_cid.borrow() {
            cb();
        }
    }
}
