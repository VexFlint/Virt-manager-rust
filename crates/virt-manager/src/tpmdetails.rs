//! `vmmTPMDetails` (`device/tpmdetails.py`): the TPM type, device path,
//! model and version, shared by the details page and (later) Add
//! Hardware.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use virtinst_core::devices::tpm::{
    MODEL_CRB, MODEL_SPAPR, MODEL_TIS, TYPE_EMULATOR, TYPE_PASSTHROUGH, VERSION_1_2, VERSION_2_0,
};
use virtinst_xml::{Document, Element};

use crate::devpages::{Grid, GridRow, SimpleCombo};

/// `_tpm_pretty_model`.
fn tpm_pretty_model(val: &str) -> String {
    match val {
        MODEL_TIS => "TIS".into(),
        MODEL_CRB => "CRB".into(),
        MODEL_SPAPR => "SPAPR".into(),
        _ => val.into(),
    }
}

/// `_EDIT_TPM_*`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TpmEdit {
    Type,
    DevicePath,
    Model,
    Version,
}

/// The values to write, each `Some` only when edited.
#[derive(Default)]
pub struct TpmValues {
    pub ttype: Option<Option<String>>,
    pub device_path: Option<Option<String>>,
    pub model: Option<Option<String>>,
    pub version: Option<Option<String>>,
}

impl TpmValues {
    /// `define_tpm`.
    pub fn apply(&self, doc: &mut Document, tpm: Element) {
        use virtinst_core::devices::tpm;
        if let Some(v) = &self.ttype {
            tpm::set_type(doc, tpm, v.as_deref());
        }
        if let Some(v) = &self.model {
            tpm::set_model(doc, tpm, v.as_deref());
        }
        if let Some(v) = &self.version {
            tpm::set_version(doc, tpm, v.as_deref());
        }
        if let Some(v) = &self.device_path {
            tpm::set_device_path(doc, tpm, v.as_deref());
        }
    }
}

pub struct TpmDetails {
    pub top_box: gtk::Box,
    ttype: SimpleCombo,
    device_path: gtk::Entry,
    device_path_row: GridRow,
    model: SimpleCombo,
    version: SimpleCombo,
    version_row: GridRow,
    active_edits: RefCell<Vec<TpmEdit>>,
    changed: RefCell<Option<Box<dyn Fn()>>>,
    /// Set while the model rows are rebuilt, which isn't an edit.
    quiet: Cell<bool>,
}

impl TpmDetails {
    pub fn new() -> Rc<TpmDetails> {
        let ttype = SimpleCombo::unsorted(
            vec![
                (Some(TYPE_EMULATOR.into()), "Emulated".into()),
                (Some(TYPE_PASSTHROUGH.into()), "Passthrough".into()),
            ],
            str::to_string,
        );
        let device_path = gtk::Entry::new();
        device_path.set_hexpand(true);
        let mut g = Grid::new();
        g.row("_Type:", &ttype.widget);
        let device_path_row = g.row("Device _Path:", &device_path);

        let model = SimpleCombo::unsorted(vec![], tpm_pretty_model);
        let version = SimpleCombo::unsorted(
            vec![
                (None, "Hypervisor default".into()),
                (Some(VERSION_2_0.into()), VERSION_2_0.into()),
                (Some(VERSION_1_2.into()), VERSION_1_2.into()),
            ],
            str::to_string,
        );
        let mut adv = Grid::new();
        adv.row("_Model:", &model.widget);
        let version_row = adv.row("_Version:", &version.widget);
        let adv = adv.into_grid();
        adv.set_margin_start(12);
        adv.set_margin_top(6);
        let expander = gtk::Expander::with_mnemonic("Adva_nced options");
        expander.set_child(Some(&adv));

        let top_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        top_box.append(&g.into_grid());
        top_box.append(&expander);

        let this = Rc::new(TpmDetails {
            top_box,
            ttype,
            device_path,
            device_path_row,
            model,
            version,
            version_row,
            active_edits: RefCell::default(),
            changed: RefCell::default(),
            quiet: Cell::new(false),
        });
        // Until domcaps arrive: the list `_init_ui` uses when domcaps
        // has no TPM block.
        this.set_models(None);
        this.connect_signals();
        this
    }

    /// `_init_ui`'s model rows: domcaps' TPM models, if it lists any.
    /// The selected model stays selected.
    pub fn set_models(&self, domcaps_models: Option<Vec<String>>) {
        let values =
            domcaps_models.unwrap_or_else(|| vec![MODEL_CRB.to_string(), MODEL_TIS.to_string()]);
        let mut rows = vec![(None, "Hypervisor default".to_string())];
        rows.extend(values.into_iter().map(|v| {
            let label = tpm_pretty_model(&v);
            (Some(v), label)
        }));
        let cur = self.model.get();
        self.quiet.set(true);
        self.model.set_values(rows);
        self.model.set(cur.as_deref());
        self.quiet.set(false);
    }

    pub fn connect_changed(&self, cb: impl Fn() + 'static) {
        self.changed.replace(Some(Box::new(cb)));
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let edit = move |e: TpmEdit| {
            let weak = weak.clone();
            move || {
                if let Some(this) = weak.upgrade() {
                    this.change_cb(e);
                }
            }
        };
        let cb = edit(TpmEdit::Type);
        self.ttype.widget.connect_selected_notify(move |_| cb());
        let cb = edit(TpmEdit::DevicePath);
        self.device_path.connect_changed(move |_| cb());
        let cb = edit(TpmEdit::Model);
        self.model.widget.connect_selected_notify(move |_| cb());
        let cb = edit(TpmEdit::Version);
        self.version.widget.connect_selected_notify(move |_| cb());
    }

    /// `_sync_ui`.
    fn sync_ui(&self) {
        let devtype = self.ttype.get();
        self.device_path_row
            .set_visible(devtype.as_deref() == Some(TYPE_PASSTHROUGH));
        self.version_row
            .set_visible(devtype.as_deref() == Some(TYPE_EMULATOR));
    }

    /// `reset_state`, without `default_model`: `set_dev` always
    /// replaces the model.
    fn reset_state(&self) {
        self.device_path.set_text("/dev/tpm0");
        self.ttype.set(Some(TYPE_EMULATOR));
        self.version.set(None);
    }

    /// `reset_state` for a new device, with `DeviceTpm.default_model`.
    pub fn reset_state_with_model(&self, default_model: Option<&str>) {
        self.reset_state();
        self.model.set(default_model);
        self.sync_ui();
        self.active_edits.borrow_mut().clear();
    }

    /// `build_device`'s values: every field counts as edited.
    pub fn build_values(&self) -> TpmValues {
        self.active_edits.replace(vec![
            TpmEdit::Type,
            TpmEdit::DevicePath,
            TpmEdit::Model,
            TpmEdit::Version,
        ]);
        self.get_values()
    }

    /// `set_dev`.
    pub fn set_dev(&self, doc: &Document, tpm: Element) {
        let attr = |path: &[&str], name: &str| {
            virtinst_xml::resolve_path(doc, tpm, path)
                .and_then(|e| e.attribute(doc, name))
                .map(str::to_string)
        };
        self.reset_state();
        self.ttype.set(attr(&["backend"], "type").as_deref());
        self.model.set(attr(&[], "model").as_deref());
        self.version.set(attr(&["backend"], "version").as_deref());
        self.device_path.set_text(
            attr(&["backend", "device"], "path")
                .as_deref()
                .unwrap_or_default(),
        );
        self.sync_ui();
        self.active_edits.borrow_mut().clear();
    }

    /// `_set_values`: what `update_device` changes.
    pub fn get_values(&self) -> TpmValues {
        let edits = self.active_edits.borrow();
        let edited = |e| edits.contains(&e);
        let device_path = self
            .device_path_row
            .is_visible()
            .then(|| self.device_path.text().to_string());
        let version = if self.version_row.is_visible() {
            self.version.get()
        } else {
            None
        };
        TpmValues {
            ttype: edited(TpmEdit::Type).then(|| self.ttype.get()),
            device_path: edited(TpmEdit::DevicePath).then_some(device_path),
            model: edited(TpmEdit::Model).then(|| self.model.get()),
            version: edited(TpmEdit::Version).then_some(version),
        }
    }

    /// `_change_cb`.
    fn change_cb(&self, edit: TpmEdit) {
        if self.quiet.get() {
            return;
        }
        self.sync_ui();
        {
            let mut edits = self.active_edits.borrow_mut();
            if !edits.contains(&edit) {
                edits.push(edit);
            }
        }
        if let Some(cb) = &*self.changed.borrow() {
            cb();
        }
    }
}
