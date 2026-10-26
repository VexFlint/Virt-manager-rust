//! `device/gfxdetails.py`: the graphics (VNC/Spice server) settings,
//! shared by the details page and, later, Add Hardware.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use virtinst_xml::{Document, Element};

use crate::devpages::{GridRow, SimpleCombo};

/// `_EDIT_GFX_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GfxEdit {
    Type,
    Listen,
    Port,
    Opengl,
    Passwd,
}

/// `get_values()`: only what was edited is `Some`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GfxValues {
    pub gtype: Option<String>,
    pub port: Option<Option<i64>>,
    pub listen: Option<Option<String>>,
    pub passwd: Option<Option<String>>,
    pub gl: Option<Option<bool>>,
    pub rendernode: Option<Option<String>>,
}

impl GfxValues {
    /// `define_graphics`, in its order: listen, port, passwd, type, gl,
    /// rendernode.
    pub fn apply(&self, doc: &mut Document, gfx: Element) -> Result<(), String> {
        use virtinst_core::devices::graphics as g;
        if let Some(listen) = &self.listen {
            g::set_listen(doc, gfx, listen.as_deref());
        }
        if let Some(port) = self.port {
            g::set_port(doc, gfx, port)?;
        }
        if let Some(passwd) = &self.passwd {
            g::set_passwd(doc, gfx, passwd.as_deref());
        }
        if let Some(gtype) = &self.gtype {
            g::set_type(doc, gfx, gtype);
        }
        if let Some(gl) = self.gl {
            g::set_gl(doc, gfx, gl);
        }
        if let Some(rendernode) = &self.rendernode {
            g::set_rendernode(doc, gfx, rendernode.as_deref());
        }
        Ok(())
    }
}

/// The `graphics-address` combo, which has an entry: a row, or a typed
/// address. GTK4 has no non-deprecated combo with an entry, so this is
/// an entry plus a drop-down button.
struct AddressCombo {
    top: gtk::Box,
    entry: gtk::Entry,
    popover: gtk::Popover,
    rows: Vec<(Option<String>, String)>,
    active: Cell<Option<usize>>,
    setting: Cell<bool>,
}

impl AddressCombo {
    fn new(rows: Vec<(Option<String>, String)>) -> Rc<AddressCombo> {
        let entry = gtk::Entry::new();
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        for (_, label) in &rows {
            let l = gtk::Label::new(Some(label));
            l.set_xalign(0.0);
            l.set_margin_top(3);
            l.set_margin_bottom(3);
            l.set_margin_start(3);
            list.append(&l);
        }
        let popover = gtk::Popover::new();
        popover.set_child(Some(&list));
        let button = gtk::MenuButton::new();
        button.set_popover(Some(&popover));
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        top.add_css_class("linked");
        top.set_halign(gtk::Align::Start);
        top.append(&entry);
        top.append(&button);
        let this = Rc::new(AddressCombo {
            top,
            entry,
            popover,
            rows,
            active: Cell::new(None),
            setting: Cell::new(false),
        });
        let weak = Rc::downgrade(&this);
        this.entry.connect_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                if !this.setting.get() {
                    this.active.set(None);
                }
            }
        });
        let weak = Rc::downgrade(&this);
        list.connect_row_activated(move |_, row| {
            if let Some(this) = weak.upgrade() {
                this.set_active(row.index() as usize);
                this.popover.popdown();
            }
        });
        this
    }

    fn set_active(&self, idx: usize) {
        let Some((_, label)) = self.rows.get(idx) else {
            return;
        };
        // Signals run with `active` already set, so listeners read the
        // row's value.
        self.active.set(Some(idx));
        self.setting.set(true);
        self.entry.set_text(label);
        self.setting.set(false);
    }

    /// `get_list_selection`: the row's value, else the typed text.
    fn get(&self) -> Option<String> {
        match self.active.get() {
            Some(idx) => self.rows[idx].0.clone(),
            None => Some(self.entry.text().trim().to_string()).filter(|t| !t.is_empty()),
        }
    }

    /// `set_list_selection`: an unlisted value goes in the entry.
    fn set(&self, val: Option<&str>) {
        match self.rows.iter().position(|(v, _)| v.as_deref() == val) {
            Some(idx) => self.set_active(idx),
            None => {
                self.active.set(None);
                self.entry.set_text(val.unwrap_or_default());
            }
        }
    }
}

type ChangedCb = Box<dyn Fn()>;

pub struct GraphicsDetails {
    pub top_box: gtk::Grid,
    gtype: SimpleCombo,
    listen_type: SimpleCombo,
    address: Rc<AddressCombo>,
    address_row: GridRow,
    port_row: GridRow,
    port_auto: gtk::CheckButton,
    port: gtk::SpinButton,
    password_row: GridRow,
    password_chk: gtk::CheckButton,
    password: gtk::Entry,
    visibility_chk: gtk::CheckButton,
    listen_row: GridRow,
    opengl: gtk::CheckButton,
    opengl_row: GridRow,
    subopts_row: GridRow,
    rendernode: SimpleCombo,
    warn_virtio: (gtk::Image, gtk::Label),
    warn_listen: (gtk::Image, gtk::Label),
    has_virtio_3d: Cell<bool>,
    /// Set while the rendernode rows arrive, which isn't an edit.
    quiet: Cell<bool>,
    active_edits: RefCell<Vec<GfxEdit>>,
    changed: RefCell<Option<ChangedCb>>,
}

fn warn_row(text: &str) -> (gtk::Image, gtk::Label) {
    let icon = gtk::Image::from_icon_name("dialog-warning");
    let label = gtk::Label::new(Some(text));
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.set_max_width_chars(40);
    (icon, label)
}

impl GraphicsDetails {
    pub fn new(conn: Option<crate::connection::VmmConnection>) -> Rc<GraphicsDetails> {
        // `_init_ui`
        let gtype = SimpleCombo::unsorted(
            vec![
                (Some("spice".into()), "Spice server".into()),
                (Some("vnc".into()), "VNC server".into()),
            ],
            str::to_string,
        );
        let listen_type = SimpleCombo::unsorted(
            vec![
                (Some("address".into()), "Address".into()),
                (Some("none".into()), "None".into()),
            ],
            str::to_string,
        );
        let address = AddressCombo::new(vec![
            (None, "Hypervisor default".into()),
            (Some("127.0.0.1".into()), "Localhost only".into()),
            (Some("0.0.0.0".into()), "All interfaces".into()),
        ]);
        let rendernode = SimpleCombo::unsorted(vec![(None, "Auto".into())], str::to_string);

        let port_auto = gtk::CheckButton::with_mnemonic("A_uto");
        let port = gtk::SpinButton::with_range(5900.0, 50000.0, 1.0);
        let port_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        port_box.append(&port_auto);
        port_box.append(&port);

        let password_chk = gtk::CheckButton::new();
        let password = gtk::Entry::new();
        password.set_visibility(false);
        password.update_property(&[gtk::accessible::Property::Label("Password")]);
        let visibility_chk = gtk::CheckButton::with_mnemonic("Show passwor_d");
        let password_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        password_box.append(&password_chk);
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 3);
        inner.append(&password);
        inner.append(&visibility_chk);
        password_box.append(&inner);

        let opengl = gtk::CheckButton::new();
        let warn_virtio =
            warn_row("OpenGL only works with 'virtio' graphics with '3D acceleration' enabled");
        let warn_listen = warn_row("OpenGL only works with 'Listen type' value 'none'");
        let subopts = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .build();
        rendernode
            .widget
            .update_property(&[gtk::accessible::Property::Label("Rendernode")]);
        subopts.attach(&rendernode.widget, 0, 0, 2, 1);
        subopts.attach(&warn_virtio.0, 0, 1, 1, 1);
        subopts.attach(&warn_virtio.1, 1, 1, 1, 1);
        subopts.attach(&warn_listen.0, 0, 2, 1, 1);
        subopts.attach(&warn_listen.1, 1, 2, 1, 1);

        let mut g = crate::devpages::Grid::new();
        g.row("_Type:", &gtype.widget);
        let listen_row = g.row("L_isten type:", &listen_type.widget);
        let address_row = g.row("Addr_ess:", &address.top);
        address_row.set_mnemonic(&address.entry);
        let port_row = g.row("_Port:", &port_box);
        port_row.set_mnemonic(&port);
        let password_row = g.row("Pa_ssword:", &password_box);
        password_row.set_mnemonic(&password);
        let opengl_row = g.row("Open_GL:", &opengl);
        let subopts_row = g.row("", &subopts);

        let this = Rc::new(GraphicsDetails {
            top_box: g.into_grid(),
            gtype,
            listen_type,
            address,
            address_row,
            port_row,
            port_auto,
            port,
            password_row,
            password_chk,
            password,
            visibility_chk,
            listen_row,
            opengl,
            opengl_row,
            subopts_row,
            rendernode,
            warn_virtio,
            warn_listen,
            has_virtio_3d: Cell::new(false),
            quiet: Cell::new(false),
            active_edits: RefCell::default(),
            changed: RefCell::default(),
        });
        this.connect_signals();
        this.init_rendernodes(conn);
        this.reset_state();
        this
    }

    /// `connect_changed`: the `changed` signal.
    pub fn connect_changed(&self, cb: impl Fn() + 'static) {
        self.changed.replace(Some(Box::new(cb)));
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let edit = move |e: GfxEdit| {
            let weak = weak.clone();
            move || {
                if let Some(this) = weak.upgrade() {
                    this.change_cb(e);
                }
            }
        };
        let cb = edit(GfxEdit::Opengl);
        self.opengl.connect_toggled(move |_| cb());
        let cb = edit(GfxEdit::Type);
        self.gtype.widget.connect_selected_notify(move |_| cb());
        let cb = edit(GfxEdit::Passwd);
        self.password_chk.connect_toggled(move |_| cb());
        let cb = edit(GfxEdit::Listen);
        self.listen_type
            .widget
            .connect_selected_notify(move |_| cb());
        let cb = edit(GfxEdit::Passwd);
        self.password.connect_changed(move |_| cb());
        let cb = edit(GfxEdit::Listen);
        self.address.entry.connect_changed(move |_| cb());
        let cb = edit(GfxEdit::Port);
        self.port.connect_value_changed(move |_| cb());
        let cb = edit(GfxEdit::Opengl);
        self.rendernode
            .widget
            .connect_selected_notify(move |_| cb());

        let weak = Rc::downgrade(self);
        self.visibility_chk.connect_toggled(move |_| {
            if let Some(this) = weak.upgrade() {
                this.sync_ui();
            }
        });
        let weak = Rc::downgrade(self);
        self.port_auto.connect_toggled(move |b| {
            if let Some(this) = weak.upgrade() {
                // `_change_port_auto`
                b.set_inconsistent(false);
                this.change_cb(GfxEdit::Port);
            }
        });
    }

    /// `_init_ui`'s rendernode rows: the host's DRM render nodes,
    /// fetched on a worker.
    fn init_rendernodes(self: &Rc<Self>, conn: Option<crate::connection::VmmConnection>) {
        let Some(conn) = conn else {
            return;
        };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let devs = conn.list_nodedevs().await;
            let Some(this) = weak.upgrade() else {
                return;
            };
            let mut rows = vec![(None, "Auto".to_string())];
            for dev in devs.iter().filter(|d| d.is_drm_render()) {
                if let Some(node) = dev.get_devnode() {
                    rows.push((
                        Some(node.to_string()),
                        virtinst_core::nodedev::pretty_name(dev, &devs),
                    ));
                }
            }
            if rows.len() == 1 {
                return;
            }
            let current = this.rendernode.get();
            let last = rows.last().and_then(|r| r.0.clone());
            this.quiet.set(true);
            this.rendernode.set_values(rows);
            // `reset_state` suggests the last render node.
            this.rendernode.set(current.as_deref().or(last.as_deref()));
            this.quiet.set(false);
        });
    }

    /// `_sync_ui`.
    fn sync_ui(&self) {
        let gtype = self.gtype.get();
        let is_vnc = gtype.as_deref() == Some("vnc");
        let is_spice = gtype.as_deref() == Some("spice");
        let listen = self.listen_type.get();
        let has_listen_none = matches!(listen.as_deref(), Some("none" | "socket"));

        let show_virtio = !self.has_virtio_3d.get();
        self.warn_virtio.0.set_visible(show_virtio);
        self.warn_virtio.1.set_visible(show_virtio);
        self.warn_listen.0.set_visible(!has_listen_none);
        self.warn_listen.1.set_visible(!has_listen_none);

        let passwd_enabled = self.password_chk.is_active();
        self.password.set_sensitive(passwd_enabled);
        if !passwd_enabled && !self.password.text().is_empty() {
            self.password.set_text("");
        }
        self.password
            .set_visibility(self.visibility_chk.is_active());

        let glval = self.opengl.is_active();
        let is_auto = self.port_auto.is_active() || self.port_auto.is_inconsistent();
        self.port.set_visible(!is_auto);

        let listen_address = listen.as_deref() == Some("address");
        let (vnc_or_spice, spice) = (is_vnc || is_spice, is_spice);
        self.password_row.set_visible(vnc_or_spice);
        self.listen_row.set_visible(vnc_or_spice);
        self.port_row.set_visible(vnc_or_spice && listen_address);
        self.address_row.set_visible(vnc_or_spice && listen_address);
        self.opengl_row.set_visible(spice);
        self.subopts_row.set_visible(spice && glval);
    }

    /// `reset_state`.
    pub fn reset_state(&self) {
        self.gtype
            .set(Some(&crate::engine::get().config().get_graphics_type()));
        self.listen_type.widget.set_selected(0);
        self.address.set_active(0);
        let rows = self.rendernode.widget.model().map_or(0, |m| m.n_items());
        self.rendernode.widget.set_selected(rows.saturating_sub(1));
        self.port_auto.set_active(true);
        self.password.set_text("");
        self.password.set_sensitive(false);
        self.password_chk.set_active(false);
        self.opengl.set_active(false);
        self.sync_ui();
        self.active_edits.borrow_mut().clear();
    }

    /// `set_dev`. `has_virtio_3d` is whether the VM has a virtio video
    /// with 3D acceleration on, for the OpenGL warning.
    pub fn set_dev(&self, doc: &Document, gfx: Element, has_virtio_3d: bool) {
        self.has_virtio_3d.set(has_virtio_3d);
        self.reset_state();
        let attr = |name: &str| gfx.attribute(doc, name).map(str::to_string);
        let portval: Option<i64> = attr("port").and_then(|p| p.parse().ok());
        let autoport = attr("autoport").as_deref() == Some("yes");

        let mut portautolabel = "A_uto".to_string();
        let portauto = if portval == Some(-1) || autoport {
            if let Some(p) = portval.filter(|p| *p != -1 && *p != 0) {
                portautolabel = format!("A_uto (Port {p})");
            }
            Some(true)
        } else if portval.is_none() {
            None
        } else {
            Some(false)
        };
        self.port.set_value(portval.unwrap_or(0) as f64);
        self.port_auto.set_label(Some(&portautolabel));
        self.port_auto.set_active(portauto.unwrap_or(false));
        self.port_auto.set_inconsistent(portauto.is_none());

        self.gtype.set(attr("type").as_deref());

        let passwd = attr("passwd");
        self.password_chk.set_active(passwd.is_some());
        self.password
            .set_text(passwd.as_deref().unwrap_or_default());

        let listentype = virtinst_core::devices::graphics::first_listen_type(doc, gfx);
        self.listen_type.set(listentype.as_deref());
        self.address.set(attr("listen").as_deref());

        let gl = gfx.find(doc, "gl");
        let glval = gl.and_then(|g| g.attribute(doc, "enable")) == Some("yes");
        self.opengl.set_active(glval);
        if glval {
            // Only sync rendernode UI with XML if gl=on, otherwise keep
            // the suggested rendernode already selected.
            let renderval = gl
                .and_then(|g| g.attribute(doc, "rendernode"))
                .filter(|r| !r.is_empty());
            self.rendernode.set(renderval);
        }
        self.sync_ui();
        // This comes last
        self.active_edits.borrow_mut().clear();
    }

    /// `get_values`.
    pub fn get_values(&self) -> GfxValues {
        let glval = self
            .opengl_row
            .is_visible()
            .then(|| self.opengl.is_active());
        let rendernode = self
            .subopts_row
            .is_visible()
            .then(|| self.rendernode.get())
            .flatten();
        let passwd = self
            .password_chk
            .is_active()
            .then(|| self.password.text().to_string());

        let mut portauto = self.port_auto.is_active();
        let mut port = Some(self.port.value() as i64);
        if self.port_auto.is_inconsistent() && self.port_row.is_visible() {
            // When switching from listen=None to listen=address with
            // Hypervisor default, we need to force autoport otherwise
            // the XML change doesn't stick
            let mut edits = self.active_edits.borrow_mut();
            if !edits.contains(&GfxEdit::Port) {
                edits.push(GfxEdit::Port);
            }
            portauto = true;
        }
        if portauto {
            port = Some(-1);
        }

        let mut listen = self.listen_type.get();
        let addr = self.address.get();
        if listen.as_deref() == Some("none") {
            port = None;
        } else if listen.is_some() {
            listen = addr;
        }

        let edits = self.active_edits.borrow();
        let edited = |e| edits.contains(&e);
        let mut ret = GfxValues::default();
        if edited(GfxEdit::Type) {
            ret.gtype = self.gtype.get();
        }
        if edited(GfxEdit::Port) {
            ret.port = Some(port);
        }
        if edited(GfxEdit::Listen) {
            ret.listen = Some(listen);
        }
        if edited(GfxEdit::Passwd) {
            ret.passwd = Some(passwd);
        }
        if edited(GfxEdit::Opengl) {
            ret.gl = Some(glval);
            ret.rendernode = Some(rendernode);
        }
        ret
    }

    /// `_change_cb`.
    fn change_cb(&self, edit: GfxEdit) {
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
