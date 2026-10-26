//! `addhardware.py` (`vmmAddHardware`): the Add New Virtual Hardware
//! dialog. A list of device types on the left, the picked type's page
//! under the blue header on the right; Finish builds the device,
//! hotplugs it into a running VM and adds it to the persistent config.
//!
//! **Not ported yet**: the Storage, Network and host device pages
//! (`addstorage.py`, the nodedev list), whose rows stay insensitive,
//! and the XML editor tab (`xmleditor.py`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use virtinst_core::devices::build::{self, GuestInfo, NewChar};
use virtinst_core::devices::DeviceId;
use virtinst_xml::{Document, Element};

use crate::devpages::{input_pretty_name, sound_pretty_model, watchdog_pretty_action};
use crate::devpages::{EntryCombo, Grid, GridRow, SimpleCombo};
use crate::domain::VmmDomain;
use crate::error;
use crate::fsdetails::FsDetails;
use crate::gfxdetails::GraphicsDetails;
use crate::tpmdetails::TpmDetails;
use crate::vsockdetails::VsockDetails;

/// `PAGE_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Disk,
    Controller,
    Network,
    Input,
    Graphics,
    Sound,
    Hostdev,
    Char,
    Video,
    Watchdog,
    Filesystem,
    Smartcard,
    Usbredir,
    Tpm,
    Rng,
    Panic,
    Vsock,
}

impl Page {
    fn name(self) -> &'static str {
        match self {
            Page::Disk => "disk",
            Page::Controller => "controller",
            Page::Network => "network",
            Page::Input => "input",
            Page::Graphics => "graphics",
            Page::Sound => "sound",
            Page::Hostdev => "hostdev",
            Page::Char => "char",
            Page::Video => "video",
            Page::Watchdog => "watchdog",
            Page::Filesystem => "filesystem",
            Page::Smartcard => "smartcard",
            Page::Usbredir => "usbredir",
            Page::Tpm => "tpm",
            Page::Rng => "rng",
            Page::Panic => "panic",
            Page::Vsock => "vsock",
        }
    }
}

/// One `hw-list` row: name, icon name, page, is sensitive, error text,
/// device type (serial/parallel/...).
struct HwOption {
    name: &'static str,
    icon: &'static str,
    page: Page,
    sensitive: bool,
    errortxt: Option<String>,
    devtype: Option<&'static str>,
}

/// Makes a device's element in a document.
type BuildFn = Rc<dyn Fn(&mut Document) -> Result<Element, String>>;

/// A device ready to add: `build` makes its element. `usb_replace` is
/// the USB controller whose model changes instead
/// (`_remove_usb_controller`).
struct NewDev {
    build: BuildFn,
    usb_replace: Option<(DeviceId, Option<String>)>,
}

impl NewDev {
    fn new(build: impl Fn(&mut Document) -> Result<Element, String> + 'static) -> NewDev {
        NewDev {
            build: Rc::new(build),
            usb_replace: None,
        }
    }

    /// `get_xml()`.
    fn xml(&self) -> Result<String, String> {
        let mut doc = Document::new();
        let el = (self.build)(&mut doc)?;
        virtinst_xml::element_to_string(doc, el).map_err(|e| e.to_string())
    }
}

/// `char_recommended_types`.
fn char_recommended_types(tag: &str) -> Vec<&'static str> {
    if tag == "console" {
        return vec!["pty"];
    }
    let mut ret = vec!["pty", "file", "unix"];
    if tag == "channel" {
        ret.splice(0..0, ["spicevmc", "spiceport", "qemu-vdagent"]);
    }
    ret
}

/// `controller_recommended_types` with `controller_pretty_type`.
const CONTROLLER_TYPES: [(&str, &str); 4] = [
    ("scsi", "SCSI"),
    ("usb", "USB"),
    ("virtio-serial", "VirtIO Serial"),
    ("ccid", "CCID"),
];

/// `_build_input_combo`'s devices, keyed `type:bus`.
const INPUT_DEVICES: [(&str, &str); 5] = [
    ("tablet", "usb"),
    ("mouse", "usb"),
    ("keyboard", "usb"),
    ("keyboard", "virtio"),
    ("tablet", "virtio"),
];

fn page_box(child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(child);
    b
}

#[derive(Clone)]
pub struct AddHardware(Rc<Inner>);

struct Inner {
    vm: VmmDomain,
    window: gtk::Window,
    hw_list: gtk::ListBox,
    options: Vec<HwOption>,
    top_pages: gtk::Stack,
    page_title: gtk::Label,
    create_pages: gtk::Stack,
    error_label: gtk::Label,
    finish: gtk::Button,

    controller_type: SimpleCombo,
    controller_model: SimpleCombo,
    controller_model_row: GridRow,
    controller_tooltip: gtk::Image,
    input_type: SimpleCombo,
    gfx: Rc<GraphicsDetails>,
    sound_model: SimpleCombo,
    char_target_name: Rc<EntryCombo>,
    char_target_name_row: GridRow,
    char_device_type: SimpleCombo,
    char_target_type: SimpleCombo,
    char_target_type_row: GridRow,
    char_auto_socket: gtk::CheckButton,
    char_auto_socket_row: GridRow,
    char_path: gtk::Entry,
    char_path_row: GridRow,
    char_channel: gtk::Entry,
    char_channel_row: GridRow,
    char_clipboard: gtk::CheckButton,
    char_clipboard_row: GridRow,
    video_model: SimpleCombo,
    watchdog_model: SimpleCombo,
    watchdog_action: SimpleCombo,
    fs: Rc<FsDetails>,
    smartcard_mode: SimpleCombo,
    usbredir_type: SimpleCombo,
    tpm: Rc<TpmDetails>,
    rng_device: gtk::Entry,
    panic_model: SimpleCombo,
    vsock: Rc<VsockDetails>,

    /// `_remove_usb_controller`: the guest's `model='none'` USB
    /// controller, which a new USB controller replaces.
    remove_usb_controller: RefCell<Option<DeviceId>>,
    /// Set while a programmatic change runs, so the char page's
    /// listeners don't fire on it.
    quiet: Cell<bool>,
}

impl AddHardware {
    pub fn new(vm: &VmmDomain) -> AddHardware {
        let conn = vm.conn();
        let is_qemu = conn.as_ref().is_some_and(|c| c.is_qemu());
        let is_xen = conn.as_ref().is_some_and(|c| c.is_xen());
        let is_test = conn.as_ref().is_some_and(|c| c.is_test());
        let guest = guest_info(vm);

        let window = gtk::Window::builder()
            .title("Add New Virtual Hardware")
            .hide_on_close(true)
            .build();
        window.set_size_request(500, 400);

        // hw-list
        let hw_list = gtk::ListBox::new();
        hw_list.set_selection_mode(gtk::SelectionMode::Browse);
        hw_list.update_property(&[gtk::accessible::Property::Label("Hardware")]);
        let hw_scroll = gtk::ScrolledWindow::builder()
            .child(&hw_list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .has_frame(true)
            .min_content_width(165)
            .build();

        // Controller
        let controller_type = SimpleCombo::new(
            CONTROLLER_TYPES
                .iter()
                .map(|(v, l)| (Some(v.to_string()), l.to_string()))
                .collect(),
            str::to_string,
        );
        controller_type.set(Some("scsi"));
        let controller_tooltip = gtk::Image::from_icon_name("dialog-information");
        controller_tooltip.set_visible(false);
        let controller_model = SimpleCombo::unsorted(vec![], str::to_string);
        let ctype_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        ctype_box.append(&controller_type.widget);
        ctype_box.append(&controller_tooltip);
        let mut g = Grid::new();
        let row = g.row("_Type:", &ctype_box);
        row.set_mnemonic(&controller_type.widget);
        let controller_model_row = g.row("_Model:", &controller_model.widget);
        let controller_page = page_box(&g.into_grid());

        // Input
        let input_type = SimpleCombo::new(
            INPUT_DEVICES
                .iter()
                .map(|(t, b)| (Some(format!("{t}:{b}")), input_pretty_name(t, b)))
                .collect(),
            str::to_string,
        );
        input_type.widget.set_selected(0);
        let mut g = Grid::new();
        g.row("_Type:", &input_type.widget);
        let input_page = page_box(&g.into_grid());

        // Graphics
        let gfx = GraphicsDetails::new(conn.clone());
        let graphics_page = page_box(&gfx.top_box);

        // Sound
        let sound_model = SimpleCombo::new(
            virtinst_core::devices::sound::RECOMMENDED_MODELS
                .iter()
                .map(|m| (Some(m.to_string()), sound_pretty_model(m)))
                .collect(),
            sound_pretty_model,
        );
        sound_model.set(Some(guest.default_sound_model()));
        let mut g = Grid::new();
        g.row("_Model:", &sound_model.widget);
        let sound_page = page_box(&g.into_grid());

        // Char
        let char_target_name = EntryCombo::new(
            build::CHANNEL_NAMES
                .iter()
                .map(|n| (Some(n.to_string()), n.to_string()))
                .collect(),
        );
        char_target_name.set_active(0);
        let char_device_type = SimpleCombo::new(vec![], str::to_string);
        let char_target_type = SimpleCombo::new(
            if is_qemu {
                vec![(Some("virtio".into()), "VirtIO".into())]
            } else {
                vec![(None, "Hypervisor default".into())]
            },
            str::to_string,
        );
        char_target_type.widget.set_selected(0);
        let char_auto_socket = gtk::CheckButton::new();
        let char_path = gtk::Entry::new();
        char_path.set_hexpand(true);
        let char_channel = gtk::Entry::new();
        let char_clipboard = gtk::CheckButton::new();
        let mut g = Grid::new();
        let char_target_name_row = g.row("_Name:", &char_target_name.top);
        char_target_name_row.set_mnemonic(&char_target_name.entry);
        g.row("Device _Type:", &char_device_type.widget);
        let char_target_type_row = g.row("T_ype:", &char_target_type.widget);
        let char_auto_socket_row = g.row("_Auto socket:", &char_auto_socket);
        let char_path_row = g.row("_Path:", &char_path);
        let char_channel_row = g.row("_Channel:", &char_channel);
        let char_clipboard_row = g.row("_Share Clipboard:", &char_clipboard);
        let char_page = page_box(&g.into_grid());

        // Video
        let mut values: Vec<_> =
            virtinst_core::devices::video::recommended_models(is_xen, is_qemu || is_test)
                .iter()
                .map(|m| (Some(m.to_string()), crate::details::video_pretty_model(m)))
                .collect();
        if values.is_empty() {
            values.push((None, "Hypervisor default".into()));
        }
        let video_model = SimpleCombo::new(values, crate::details::video_pretty_model);
        video_model.set(guest.default_video_model(None, is_qemu));
        let mut g = Grid::new();
        g.row("_Model:", &video_model.widget);
        let video_page = page_box(&g.into_grid());

        // Watchdog
        use virtinst_core::devices::watchdog::{ACTIONS, MODELS};
        let watchdog_model = SimpleCombo::new(
            MODELS
                .iter()
                .map(|m| (Some(m.to_string()), m.to_uppercase()))
                .collect(),
            |m| m.to_uppercase(),
        );
        watchdog_model.set(Some("i6300esb"));
        let watchdog_action = SimpleCombo::new(
            ACTIONS
                .iter()
                .map(|a| (Some(a.to_string()), watchdog_pretty_action(a)))
                .collect(),
            watchdog_pretty_action,
        );
        watchdog_action.set(Some("reset"));
        let mut g = Grid::new();
        g.row("_Model:", &watchdog_model.widget);
        g.row("Ac_tion:", &watchdog_action.widget);
        let watchdog_page = page_box(&g.into_grid());

        // Filesystem
        let fs = FsDetails::new(conn.as_ref());
        let fs_page = page_box(&fs.top_box);

        // Smartcard
        let smartcard_mode = SimpleCombo::new(
            vec![
                (Some("passthrough".into()), "Passthrough".into()),
                (Some("host".into()), "Host".into()),
            ],
            str::to_string,
        );
        smartcard_mode.widget.set_selected(0);
        let mut g = Grid::new();
        g.row("_Mode:", &smartcard_mode.widget);
        let smartcard_page = page_box(&g.into_grid());

        // USB redirection
        let usbredir_type = SimpleCombo::new(
            vec![(Some("spicevmc".into()), "Spice channel".into())],
            str::to_string,
        );
        usbredir_type.widget.set_selected(0);
        let mut g = Grid::new();
        g.row("_Type:", &usbredir_type.widget);
        let usbredir_page = page_box(&g.into_grid());

        // TPM
        let tpm = TpmDetails::new();
        let tpm_page = page_box(&tpm.top_box);

        // RNG
        let rng_device = gtk::Entry::new();
        rng_device.set_hexpand(true);
        let mut g = Grid::new();
        g.row("Host _Device:", &rng_device);
        let rng_page = page_box(&g.into_grid());

        // Panic
        let panic_model =
            SimpleCombo::new(vec![(None, "Hypervisor default".into())], str::to_string);
        panic_model.set(None);
        let mut g = Grid::new();
        g.row("_Model:", &panic_model.widget);
        let panic_page = page_box(&g.into_grid());

        // VSOCK
        let vsock = VsockDetails::new();
        let vsock_page = page_box(&vsock.top_box);

        let create_pages = gtk::Stack::new();
        create_pages.set_vhomogeneous(false);
        create_pages.set_hhomogeneous(false);
        for (page, widget) in [
            (Page::Controller, &controller_page),
            (Page::Input, &input_page),
            (Page::Graphics, &graphics_page),
            (Page::Sound, &sound_page),
            (Page::Char, &char_page),
            (Page::Video, &video_page),
            (Page::Watchdog, &watchdog_page),
            (Page::Filesystem, &fs_page),
            (Page::Smartcard, &smartcard_page),
            (Page::Usbredir, &usbredir_page),
            (Page::Tpm, &tpm_page),
            (Page::Rng, &rng_page),
            (Page::Panic, &panic_page),
            (Page::Vsock, &vsock_page),
        ] {
            create_pages.add_named(widget, Some(page.name()));
        }

        // The blue header over the page.
        let page_title = gtk::Label::new(Some("Page title"));
        page_title.set_halign(gtk::Align::Start);
        page_title.set_valign(gtk::Align::Start);
        page_title.set_margin_start(6);
        page_title.set_margin_end(6);
        page_title.set_margin_top(6);
        page_title.set_margin_bottom(6);
        page_title.add_css_class("vmm-header-text");
        let page_title_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        page_title_box.add_css_class("vmm-header");
        page_title_box.append(&page_title);
        let create_vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        create_vbox.append(&page_title_box);
        create_vbox.append(&create_pages);

        let error_label = gtk::Label::new(Some("label"));
        error_label.set_wrap(true);
        let top_pages = gtk::Stack::new();
        top_pages.set_hexpand(true);
        top_pages.add_named(&create_vbox, Some("info"));
        top_pages.add_named(&error_label, Some("error"));

        let mainhbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        mainhbox.set_vexpand(true);
        mainhbox.append(&hw_scroll);
        mainhbox.append(&top_pages);

        let cancel = gtk::Button::with_mnemonic("_Cancel");
        let finish = gtk::Button::with_mnemonic("_Finish");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&finish);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.append(&mainhbox);
        vbox.append(&buttons);
        window.set_child(Some(&vbox));

        let options = hw_options(&guest);
        for opt in &options {
            hw_list.append(&hw_row(opt));
        }

        let this = AddHardware(Rc::new(Inner {
            vm: vm.clone(),
            window,
            hw_list,
            options,
            top_pages,
            page_title,
            create_pages,
            error_label,
            finish: finish.clone(),
            controller_type,
            controller_model,
            controller_model_row,
            controller_tooltip,
            input_type,
            gfx,
            sound_model,
            char_target_name,
            char_target_name_row,
            char_device_type,
            char_target_type,
            char_target_type_row,
            char_auto_socket,
            char_auto_socket_row,
            char_path,
            char_path_row,
            char_channel,
            char_channel_row,
            char_clipboard,
            char_clipboard_row,
            video_model,
            watchdog_model,
            watchdog_action,
            fs,
            smartcard_mode,
            usbredir_type,
            tpm,
            rng_device,
            panic_model,
            vsock,
            remove_usb_controller: RefCell::default(),
            quiet: Cell::new(false),
        }));

        let weak = Rc::downgrade(&this.0);
        let with = move |f: fn(&AddHardware)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&AddHardware(inner));
                }
            }
        };
        let i = &this.0;
        let cb = with(AddHardware::close);
        cancel.connect_clicked(move |_| cb());
        let cb = with(|this| {
            let this = this.clone();
            glib::spawn_future_local(async move { this.finish().await });
        });
        finish.connect_clicked(move |_| cb());
        let cb = with(AddHardware::hw_selected);
        i.hw_list.connect_row_selected(move |_, _| cb());
        let cb = with(AddHardware::change_char_device_type);
        i.char_device_type
            .widget
            .connect_selected_notify(move |_| cb());
        let cb = with(AddHardware::change_char_target_name);
        i.char_target_name.entry.connect_changed(move |_| cb());
        let cb = with(AddHardware::change_char_auto_socket);
        i.char_auto_socket.connect_toggled(move |_| cb());
        let cb = with(AddHardware::change_controller_type);
        i.controller_type
            .widget
            .connect_selected_notify(move |_| cb());

        // bind_escape_key_close
        let keys = gtk::EventControllerKey::new();
        let cb = with(AddHardware::close);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                cb();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        i.window.add_controller(keys);

        this.load_domcaps();
        this
    }

    /// The parts of `_set_initial_state` and `_reset_state` that read
    /// domcaps: the panic models, the video default, the TPM models
    /// and virtiofs.
    fn load_domcaps(&self) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let i = &this.0;
            let caps = i.vm.get_domain_capabilities().await;
            let caps = caps.as_deref();
            let guest = guest_info(&i.vm);
            let is_qemu = i.vm.conn().is_some_and(|c| c.is_qemu());

            let mut values = vec![(None, "Hypervisor default".to_string())];
            values.extend(
                build::supported_panic_models(caps)
                    .into_iter()
                    .map(|m| (Some(m.clone()), m)),
            );
            i.panic_model.set_values(values);
            i.panic_model.set(None);

            i.video_model.set(guest.default_video_model(caps, is_qemu));

            if let Some(caps) = caps {
                let tpm = &caps.devices.tpm;
                i.tpm.set_models(tpm.supported.is_some().then(|| {
                    tpm.get_enum("model")
                        .map(|e| e.values.clone())
                        .unwrap_or_default()
                }));
                let container = i.vm.conn().is_some_and(|c| c.is_container_only());
                if !container {
                    i.fs.set_virtiofs_supported(caps.supports_filesystem_virtiofs());
                }
            }
            i.tpm
                .reset_state_with_model(build::default_tpm_model(caps, &guest));
        });
    }

    /// `show`.
    pub fn show(&self, parent: &gtk::Window) {
        glib::g_debug!("virt-manager", "Showing addhw");
        self.reset_state();
        self.0.window.set_transient_for(Some(parent));
        self.0.window.present();
    }

    /// `close`.
    pub fn close(&self) {
        if self.0.window.is_visible() {
            glib::g_debug!("virt-manager", "Closing addhw");
            self.0.window.set_visible(false);
        }
    }

    /// `_cleanup`.
    pub fn cleanup(&self) {
        self.0.window.destroy();
    }

    /// `_reset_state`.
    fn reset_state(&self) {
        let i = &self.0;
        self.set_hw_selection(0);

        // Char parameters
        i.char_path.set_text("");
        i.char_channel.set_text("");
        i.char_auto_socket.set_active(true);
        i.char_clipboard.set_active(true);

        // RNG params; every libvirt we support has `conn_rng_urandom`.
        i.rng_device.set_text("/dev/urandom");

        // Remaining devices
        i.fs.reset_state();
        i.gfx.reset_state();
        i.vsock.reset_state();
        let guest = guest_info(&i.vm);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let i = &this.0;
            let caps = i.vm.get_domain_capabilities().await;
            i.tpm
                .reset_state_with_model(build::default_tpm_model(caps.as_deref(), &guest));
            let (have_shared_mem, _) = i.vm.has_shared_mem().await;
            i.fs.set_have_shared_mem(have_shared_mem);
        });
    }

    fn set_hw_selection(&self, idx: i32) {
        let list = &self.0.hw_list;
        list.select_row(list.row_at_index(idx).as_ref());
    }

    fn get_hw_selection(&self) -> Option<&HwOption> {
        let row = self.0.hw_list.selected_row()?;
        self.0.options.get(row.index() as usize)
    }

    /// `_get_char_class`, as the row's tag.
    fn get_char_class(&self) -> &'static str {
        self.get_hw_selection()
            .and_then(|o| o.devtype)
            .unwrap_or("serial")
    }

    /// `_set_error_page`.
    fn set_error_page(&self, msg: Option<&str>) {
        let i = &self.0;
        i.top_pages.set_visible_child_name("error");
        i.error_label
            .set_text(msg.unwrap_or("Hardware selection error."));
        i.finish.set_sensitive(false);
    }

    /// `_hw_selected_cb`.
    fn hw_selected(&self) {
        let i = &self.0;
        i.finish.set_sensitive(true);
        let Some(opt) = self.get_hw_selection() else {
            self.set_error_page(None);
            return;
        };
        if !opt.sensitive {
            self.set_error_page(opt.errortxt.as_deref());
            return;
        }
        let page = opt.page;

        if page == Page::Char {
            // Need to do this here, since we share the char page between
            // multiple different HW options
            self.populate_char_device_type_combo();
            self.change_char_device_type();
            self.change_char_target_name();
        }
        if page == Page::Controller {
            // We need to trigger this as it can desensitive 'finish'
            self.change_controller_type();
        }

        self.set_page_title(page);
        i.create_pages.set_visible_child_name(page.name());
        i.top_pages.set_visible_child_name("info");
    }

    /// `_dev_to_title`.
    fn dev_to_title(&self, page: Page) -> String {
        match page {
            Page::Disk => "Storage".into(),
            Page::Controller => "Controller".into(),
            Page::Network => "Network".into(),
            Page::Input => "Input".into(),
            Page::Graphics => "Graphics".into(),
            Page::Sound => "Sound".into(),
            Page::Video => "Video Device".into(),
            Page::Watchdog => "Watchdog Device".into(),
            Page::Filesystem => "Filesystem Passthrough".into(),
            Page::Smartcard => "Smartcard".into(),
            Page::Usbredir => "USB Redirection".into(),
            Page::Tpm => "TPM".into(),
            Page::Rng => "Random Number Generator".into(),
            Page::Panic => "Panic Notifier".into(),
            Page::Vsock => "VM Sockets".into(),
            Page::Char => {
                let tag = self.get_char_class();
                let mut c = tag.chars();
                let cap: String = c
                    .next()
                    .map(|f| f.to_uppercase().chain(c).collect())
                    .unwrap_or_default();
                format!("{cap} Device")
            }
            Page::Hostdev => match self.get_hw_selection().and_then(|o| o.devtype) {
                Some("pci") => "PCI Device".into(),
                Some("mdev") => "MDEV Device".into(),
                _ => "USB Device".into(),
            },
        }
    }

    /// `_set_page_title`.
    fn set_page_title(&self, page: Page) {
        self.0.page_title.set_markup(&self.dev_to_title(page));
    }

    /// `_populate_char_device_type_combo`.
    fn populate_char_device_type_combo(&self) {
        let i = &self.0;
        let tag = self.get_char_class();
        i.quiet.set(true);
        i.char_device_type.set_values(
            char_recommended_types(tag)
                .into_iter()
                .map(|t| {
                    let label = format!("{} ({t})", crate::details::char_pretty_type(t));
                    (Some(t.to_string()), label)
                })
                .collect(),
        );
        i.char_device_type.set(Some("pty"));
        i.quiet.set(false);
    }

    /// `_change_char_auto_socket`.
    fn change_char_auto_socket(&self) {
        let i = &self.0;
        if !i.char_auto_socket.is_visible() {
            return;
        }
        let doshow = !i.char_auto_socket.is_active();
        i.char_path_row.set_visible(doshow);
    }

    /// `_change_char_target_name`.
    fn change_char_target_name(&self) {
        let i = &self.0;
        if i.quiet.get() || !WidgetExt::is_visible(&i.char_target_name.entry) {
            return;
        }
        let text = i.char_target_name.entry.text();
        let settype = match text.as_str() {
            build::CHANNEL_NAME_SPICE => Some("spicevmc"),
            build::CHANNEL_NAME_SPICE_WEBDAV => {
                i.char_channel.set_text(&text);
                Some("spiceport")
            }
            build::CHANNEL_NAME_QEMUGA | build::CHANNEL_NAME_LIBGUESTFS => Some("unix"),
            _ => None,
        };
        if let Some(settype) = settype {
            i.char_device_type.set(Some(settype));
        }
    }

    /// `_change_char_device_type`.
    fn change_char_device_type(&self) {
        let i = &self.0;
        if i.quiet.get() {
            return;
        }
        let Some(devtype) = i.char_device_type.get() else {
            return;
        };
        let tag = self.get_char_class();
        let ischan = tag == "channel";
        let iscon = tag == "console";
        let show_auto = devtype == "unix" && ischan;

        let supports_path = ["file", "unix", "dev", "pipe"];
        i.char_path_row
            .set_visible(supports_path.contains(&devtype.as_str()));
        i.char_channel_row.set_visible(devtype == "spiceport");
        i.char_clipboard_row.set_visible(devtype == "qemu-vdagent");

        i.char_target_name_row.set_visible(ischan);
        i.char_target_type_row.set_visible(iscon);
        i.char_auto_socket_row.set_visible(show_auto);
        self.change_char_auto_socket();
    }

    /// `_change_controller_type`.
    fn change_controller_type(&self) {
        let i = &self.0;
        let combo = &i.controller_model;
        let vmname = i.vm.get_name();
        i.controller_tooltip.set_visible(false);
        i.controller_tooltip.set_tooltip_text(Some(&format!(
            "{vmname} already has a USB controller attached.\n\
             Adding more than one USB controller is not supported.\n\
             You can change the USB controller type in the VM details screen."
        )));
        combo.widget.set_sensitive(true);
        // Unlike upstream, a USB pick that's changed back doesn't leave
        // the controller to be replaced behind.
        i.remove_usb_controller.replace(None);

        let controller_type = i.controller_type.get().unwrap_or_default();
        if controller_type == "usb" {
            let usb: Vec<(usize, Element)> = i
                .vm
                .xmlobj()
                .map(|x| {
                    let devices = virtinst_xml::resolve_path(&x.doc, x.root(), &["devices"]);
                    let ids: Vec<_> = devices
                        .map(|d| d.find_all(&x.doc, "controller"))
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|c| c.attribute(&x.doc, "type") == Some("usb"))
                        .enumerate()
                        .collect();
                    if let [(idx, el)] = ids.as_slice() {
                        if el.attribute(&x.doc, "model") == Some("none") {
                            i.remove_usb_controller.replace(Some(
                                virtinst_core::devices::device_id(&x.doc, *el, "controller", *idx),
                            ));
                        }
                    }
                    ids
                })
                .unwrap_or_default();
            if usb.is_empty() || i.remove_usb_controller.borrow().is_some() {
                i.finish.set_sensitive(true);
            } else {
                i.controller_tooltip.set_visible(true);
                i.finish.set_sensitive(false);
            }
        } else {
            i.finish.set_sensitive(true);
        }

        // `populate_controller_model_combo`
        let rows = virtinst_core::devices::controller::model_rows(&controller_type);
        let n_rows = rows.len();
        let first = rows.first().and_then(|(v, _)| *v);
        combo.set_values(
            rows.into_iter()
                .map(|(v, l)| (v.map(str::to_string), l.to_string()))
                .collect(),
        );
        combo.set(first);
        i.controller_model_row.set_visible(n_rows > 1);
    }

    /// `_build_device_page`, with `set_defaults` folded into the
    /// builders.
    fn build_device_page(&self, page: Page) -> Result<NewDev, String> {
        let i = &self.0;
        let guest = guest_info(&i.vm);
        let conn = i.vm.conn();
        Ok(match page {
            Page::Controller => return self.build_controller(),
            Page::Input => {
                let val = i.input_type.get().unwrap_or_default();
                let (typ, bus) = val.split_once(':').unwrap_or(("mouse", "usb"));
                let (typ, bus) = (typ.to_string(), bus.to_string());
                NewDev::new(move |doc| Ok(build::input(doc, &typ, &bus)))
            }
            Page::Graphics => {
                let values = i.gfx.build_values();
                let is_remote = conn.as_ref().is_some_and(|c| c.is_remote());
                NewDev::new(move |doc| {
                    let el = Element::new(doc, "graphics");
                    values.apply(doc, el)?;
                    build::graphics_defaults(doc, el, is_remote);
                    Ok(el)
                })
            }
            Page::Sound => {
                let model = i.sound_model.get();
                NewDev::new(move |doc| Ok(build::sound(doc, model.as_deref(), &guest)))
            }
            Page::Char => {
                let tag = self.get_char_class();
                let path = i.char_path.text().to_string();
                let channel = i.char_channel.text().to_string();
                let c = NewChar {
                    tag: tag.to_string(),
                    char_type: i.char_device_type.get().unwrap_or_else(|| "pty".into()),
                    source_path: i.char_path_row.is_visible().then_some(path),
                    source_channel: i.char_channel_row.is_visible().then_some(channel),
                    clipboard: i
                        .char_clipboard_row
                        .is_visible()
                        .then(|| i.char_clipboard.is_active()),
                    target_name: i
                        .char_target_name_row
                        .is_visible()
                        .then(|| i.char_target_name.entry.text().to_string()),
                    target_type: i
                        .char_target_type_row
                        .is_visible()
                        .then(|| i.char_target_type.get())
                        .flatten(),
                };
                NewDev::new(move |doc| Ok(build::char_device(doc, &c)))
            }
            Page::Video => {
                let model = i.video_model.get();
                let is_qemu = conn.as_ref().is_some_and(|c| c.is_qemu());
                let default = guest.default_video_model(None, is_qemu);
                NewDev::new(move |doc| Ok(build::video(doc, model.as_deref(), default, &guest)))
            }
            Page::Watchdog => {
                let model = i.watchdog_model.get();
                let action = i.watchdog_action.get();
                NewDev::new(move |doc| {
                    Ok(build::watchdog(doc, model.as_deref(), action.as_deref()))
                })
            }
            Page::Filesystem => {
                let values = i.fs.build_values();
                let driver_mode = conn
                    .as_ref()
                    .is_some_and(|c| c.is_qemu() || c.is_lxc() || c.is_test());
                let dev = NewDev::new(move |doc| {
                    let el = Element::new(doc, "filesystem");
                    values.apply(doc, el);
                    build::filesystem_defaults(doc, el, driver_mode)?;
                    Ok(el)
                });
                // `build_device` validates as it builds.
                dev.xml()?;
                dev
            }
            Page::Smartcard => {
                let mode = i.smartcard_mode.get();
                NewDev::new(move |doc| Ok(build::smartcard(doc, mode.as_deref())))
            }
            Page::Usbredir => {
                let typ = i.usbredir_type.get();
                NewDev::new(move |doc| Ok(build::redirdev(doc, typ.as_deref())))
            }
            Page::Tpm => {
                let values = i.tpm.build_values();
                NewDev::new(move |doc| {
                    let el = Element::new(doc, "tpm");
                    values.apply(doc, el);
                    tpm_defaults(doc, el);
                    Ok(el)
                })
            }
            Page::Rng => {
                let device = i.rng_device.text().to_string();
                NewDev::new(move |doc| Ok(build::rng(doc, &device)))
            }
            Page::Panic => {
                let model = i.panic_model.get();
                NewDev::new(move |doc| Ok(build::panic(doc, model.as_deref())))
            }
            Page::Vsock => {
                let (auto_cid, cid) = i.vsock.get_values();
                NewDev::new(move |doc| Ok(build::vsock(doc, auto_cid, cid)))
            }
            Page::Disk | Page::Network | Page::Hostdev => {
                return Err("This device type isn't supported yet".into())
            }
        })
    }

    /// `_build_controller`.
    fn build_controller(&self) -> Result<NewDev, String> {
        let i = &self.0;
        let controller_type = i.controller_type.get().unwrap_or_default();
        let model = i.controller_model.get();

        if let Some(id) = i.remove_usb_controller.borrow().clone() {
            let mut dev = NewDev::new(|_| Err("unused".into()));
            dev.usb_replace = Some((id, model));
            return Ok(dev);
        }

        let index = i.vm.xmlobj().and_then(|x| {
            let devices = virtinst_xml::resolve_path(&x.doc, x.root(), &["devices"])?;
            devices
                .find_all(&x.doc, "controller")
                .into_iter()
                .filter(|c| c.attribute(&x.doc, "type") == Some(controller_type.as_str()))
                .map(|c| {
                    c.attribute(&x.doc, "index")
                        .and_then(|v| v.parse::<u32>().ok())
                        .unwrap_or(0)
                })
                .max()
                .map(|m| m + 1)
        });
        Ok(NewDev::new(move |doc| {
            Ok(build::controller(
                doc,
                &controller_type,
                model.as_deref(),
                index,
            ))
        }))
    }

    /// `_build_device`.
    async fn build_device(&self) -> Option<NewDev> {
        let page = self.get_hw_selection()?.page;
        match self.build_device_page(page) {
            Ok(dev) => Some(dev),
            Err(e) => {
                self.show_err(&format!("Error building device XML: {e}"), None)
                    .await;
                None
            }
        }
    }

    async fn show_err(&self, msg: &str, details: Option<&str>) {
        error::show_err(Some(&self.0.window), msg, details, "").await;
    }

    /// `_finish`.
    async fn finish(&self) {
        let Some(dev) = self.build_device().await else {
            return;
        };

        let i = &self.0;
        i.window.set_cursor_from_name(Some("progress"));
        let err = crate::asyncjob::run(
            Some(&i.window),
            "Creating device",
            "Depending on the device, this may take a few minutes to complete.",
            true,
            // `_setup_device` only has work for disks.
            |_| Ok(()),
        )
        .await;
        // `_finish_cb`
        let mut failure = true;
        let mut error = err.map(|(e, d)| (e, Some(d)));
        if error.is_none() {
            match self.add_device(&dev).await {
                Ok(f) => failure = f,
                Err(e) => error = Some((format!("Unable to add device: {e}"), None)),
            }
        }
        if let Some((e, details)) = error {
            self.show_err(&e, details.as_deref()).await;
        }
        i.window.set_cursor_from_name(None);
        if !failure {
            self.close();
        }
    }

    /// `_add_device`: hotplug, asking whether to go on when that fails,
    /// then the persistent config. Resolves to whether it failed.
    async fn add_device(&self, dev: &NewDev) -> Result<bool, String> {
        let i = &self.0;
        let vm = &i.vm;
        let parent = Some(&i.window);

        if let Some((id, model)) = &dev.usb_replace {
            // `change_config_helper(define_controller, ...)`
            let define_model = model.clone();
            let res = vm
                .define_device(id, move |doc, el| {
                    let devices = el.parent(doc).ok_or("Controller has no parent")?;
                    virtinst_core::devices::controller::set_usb_model(
                        doc,
                        devices,
                        define_model.as_deref(),
                    );
                    Ok(())
                })
                .await;
            if let Err(e) = res {
                error::show_err(
                    parent.map(|w| w.upcast_ref()),
                    &format!("Error changing VM configuration: {e}"),
                    None,
                    "",
                )
                .await;
            } else if vm.is_active() {
                error::show_notice(
                    parent.map(|w| w.upcast_ref()),
                    "These changes will take effect after the next guest shutdown.",
                    None,
                )
                .await;
            }
            i.remove_usb_controller.replace(None);
            return Ok(false);
        }

        let xml = dev.xml()?;
        glib::g_debug!("virt-manager", "Adding device:\n{xml}");

        // Hotplug device
        if let Err(e) = vm.attach_device(xml).await {
            glib::g_debug!("virt-manager", "Device could not be hotplugged: {e}");
            let (res, _) = error::run_dialog(
                parent.map(|w| w.upcast_ref()),
                error::DialogSpec {
                    kind: error::MessageType::Warning,
                    buttons: error::Buttons::YesNo,
                    text1: "Are you sure you want to add this device?",
                    text2: Some(
                        "This device could not be attached to the running machine. \
                         Would you like to make the device available after the next \
                         guest shutdown?",
                    ),
                    title: "",
                    details: Some(&e),
                    chktext: None,
                },
            )
            .await;
            if !res {
                return Ok(false);
            }
        }

        // Alter persistent config
        let build = dev.build.clone();
        vm.add_device(move |doc| build(doc)).await?;
        Ok(false)
    }
}

/// `DeviceTpm.set_defaults` for what the TPM page leaves: a device
/// path implies passthrough, else emulator.
fn tpm_defaults(doc: &mut Document, tpm: Element) {
    let backend = virtinst_xml::resolve_or_create_path(doc, tpm, &["backend"]);
    if backend.attribute(doc, "type").is_none() {
        let has_path = virtinst_xml::resolve_path(doc, backend, &["device"])
            .and_then(|d| d.attribute(doc, "path"))
            .is_some();
        let typ = if has_path { "passthrough" } else { "emulator" };
        backend.set_attribute(doc, "type", typ);
    }
}

/// The guest facts the device defaults read, from the VM's XML.
fn guest_info(vm: &VmmDomain) -> GuestInfo {
    let Some(x) = vm.xmlobj() else {
        return GuestInfo::default();
    };
    let doc = &x.doc;
    let os = &x.guest.os;
    let graphics = virtinst_xml::resolve_path(doc, x.root(), &["devices"])
        .map(|d| d.find_all(doc, "graphics"))
        .unwrap_or_default();
    let has_spice = graphics
        .iter()
        .any(|g| g.attribute(doc, "type") == Some("spice"));
    let has_gl = graphics
        .iter()
        .any(|g| g.find(doc, "gl").and_then(|gl| gl.attribute(doc, "enable")) == Some("yes"));
    let is_uefi = (os.loader.is_some() && os.loader_type.as_deref() == Some("pflash"))
        || os.firmware.as_deref() == Some("efi");
    let os_virtiogpu = x
        .guest
        .metadata
        .libosinfo
        .os_id
        .as_deref()
        .and_then(crate::oslist::lookup_osinfo)
        .is_some_and(|o| o.supports_virtiogpu());
    GuestInfo {
        os_type: os.os_type.clone(),
        arch: os.arch.clone(),
        machine: os.machine.clone(),
        domain_type: x.guest.domain_type.clone(),
        has_spice,
        has_gl,
        is_uefi,
        os_virtiogpu,
    }
}

/// `_set_initial_state`'s `add_hw_option` rows.
fn hw_options(guest: &GuestInfo) -> Vec<HwOption> {
    let is_hvm = guest.is_hvm();
    let not_hvm = Some("Not supported for this guest type.");
    let unported = Some("This device type isn't supported yet.");
    let vsock_err = Some("Not supported for this hypervisor/libvirt/arch combination.");
    let opt = |name, icon, page, sensitive: bool, errortxt: Option<&str>, devtype| HwOption {
        name,
        icon,
        page,
        sensitive,
        errortxt: errortxt.map(str::to_string),
        devtype,
    };
    vec![
        opt(
            "Storage",
            "drive-harddisk",
            Page::Disk,
            false,
            unported,
            None,
        ),
        opt(
            "Controller",
            "device_pci",
            Page::Controller,
            true,
            None,
            None,
        ),
        opt(
            "Network",
            "network-idle",
            Page::Network,
            false,
            unported,
            None,
        ),
        opt("Input", "input-mouse", Page::Input, is_hvm, not_hvm, None),
        opt(
            "Graphics",
            "video-display",
            Page::Graphics,
            true,
            None,
            None,
        ),
        opt("Sound", "audio-card", Page::Sound, is_hvm, not_hvm, None),
        opt(
            "Serial",
            "device_serial",
            Page::Char,
            is_hvm,
            not_hvm,
            Some("serial"),
        ),
        opt(
            "Parallel",
            "device_serial",
            Page::Char,
            is_hvm,
            not_hvm,
            Some("parallel"),
        ),
        opt(
            "Console",
            "device_serial",
            Page::Char,
            true,
            None,
            Some("console"),
        ),
        opt(
            "Channel",
            "device_serial",
            Page::Char,
            is_hvm,
            not_hvm,
            Some("channel"),
        ),
        opt(
            "USB Host Device",
            "device_usb",
            Page::Hostdev,
            false,
            unported,
            Some("usb"),
        ),
        opt(
            "PCI Host Device",
            "device_pci",
            Page::Hostdev,
            false,
            unported,
            Some("pci"),
        ),
        opt(
            "MDEV Host Device",
            "device_pci",
            Page::Hostdev,
            false,
            unported,
            Some("mdev"),
        ),
        opt("Video", "video-display", Page::Video, true, None, None),
        opt(
            "Watchdog",
            "device_pci",
            Page::Watchdog,
            is_hvm,
            not_hvm,
            None,
        ),
        opt("Filesystem", "folder", Page::Filesystem, true, None, None),
        opt(
            "Smartcard",
            "device_serial",
            Page::Smartcard,
            true,
            None,
            None,
        ),
        opt(
            "USB Redirection",
            "device_usb",
            Page::Usbredir,
            true,
            None,
            None,
        ),
        opt("TPM", "device_cpu", Page::Tpm, true, None, None),
        opt("RNG", "system-run", Page::Rng, true, None, None),
        opt(
            "Panic Notifier",
            "system-run",
            Page::Panic,
            true,
            None,
            None,
        ),
        opt(
            "VirtIO VSOCK",
            "network-idle",
            Page::Vsock,
            is_hvm,
            vsock_err,
            None,
        ),
    ]
}

/// A `hw-list` row: the icon and name, greyed when insensitive. The row
/// itself stays selectable, to show the error page.
fn hw_row(opt: &HwOption) -> gtk::ListBoxRow {
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    hbox.set_margin_top(3);
    hbox.set_margin_bottom(3);
    hbox.set_margin_start(3);
    hbox.set_margin_end(3);
    let image = gtk::Image::from_icon_name(opt.icon);
    let label = gtk::Label::new(Some(opt.name));
    label.set_xalign(0.0);
    label.set_margin_start(6);
    hbox.append(&image);
    hbox.append(&label);
    hbox.set_sensitive(opt.sensitive);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&hbox));
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_types() {
        assert_eq!(char_recommended_types("console"), ["pty"]);
        assert_eq!(char_recommended_types("serial"), ["pty", "file", "unix"]);
        assert_eq!(
            char_recommended_types("channel"),
            [
                "spicevmc",
                "spiceport",
                "qemu-vdagent",
                "pty",
                "file",
                "unix"
            ]
        );
    }
}
