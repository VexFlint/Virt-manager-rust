//! spice-gtk's `SpiceUsbDeviceWidget` (`src/usb-device-widget.c`) for
//! GTK4: a check box per USB device the session's device manager sees,
//! which redirects the device to the guest while checked, plus the
//! "SPICE CD" entry that shares an ISO image or device as a USB CD.
//!
//! As upstream, a status line counts the free redirection channels, a
//! device that can't be redirected is greyed out with the reason shown,
//! and devices are listed newest first under the CD entry. GTK4 has no
//! `GtkInfoBar` worth using (it is deprecated), so the message bar is a
//! plain icon and label row in the same place.

use std::cell::RefCell;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::spice::{Session, UsbDevice, UsbDeviceManager};

type ConnectFailed = Box<dyn Fn(&str)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct UsbDeviceWidget {
        pub device_format_string: RefCell<Option<String>>,
        pub manager: RefCell<Option<UsbDeviceManager>>,
        pub label: gtk::Label,
        pub info_bar: RefCell<Option<gtk::Box>>,
        pub empty_cd: RefCell<Option<gtk::CheckButton>>,
        pub devices: gtk::Box,
        /// Each device's check box, its device and its toggled handler.
        pub checks: RefCell<Vec<(gtk::CheckButton, UsbDevice, glib::SignalHandlerId)>>,
        pub err_msg: RefCell<Option<String>>,
        pub handlers: RefCell<Vec<glib::SignalHandlerId>>,
        pub connect_failed: RefCell<Option<ConnectFailed>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for UsbDeviceWidget {
        const NAME: &'static str = "VmmUsbDeviceWidget";
        type Type = super::UsbDeviceWidget;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for UsbDeviceWidget {
        fn dispose(&self) {
            if let Some(manager) = self.manager.take() {
                for id in self.handlers.take() {
                    manager.object().disconnect(id);
                }
            }
            self.connect_failed.take();
        }
    }

    impl WidgetImpl for UsbDeviceWidget {}
    impl BoxImpl for UsbDeviceWidget {}
}

glib::wrapper! {
    pub struct UsbDeviceWidget(ObjectSubclass<imp::UsbDeviceWidget>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl UsbDeviceWidget {
    /// `spice_usb_device_widget_new` + `constructed`.
    pub fn new(session: &Session, device_format_string: Option<&str>) -> UsbDeviceWidget {
        let obj: UsbDeviceWidget = glib::Object::builder()
            .property("orientation", gtk::Orientation::Vertical)
            .property("spacing", 6)
            .build();
        obj.imp()
            .device_format_string
            .replace(device_format_string.map(str::to_string));
        obj.constructed(session);
        obj
    }

    /// The `connect-failed` signal's single listener.
    pub fn connect_connect_failed(&self, f: impl Fn(&str) + 'static) {
        self.imp().connect_failed.replace(Some(Box::new(f)));
    }

    fn constructed(&self, session: &Session) {
        let imp = self.imp();
        imp.label.set_markup(&format!(
            "<b>{}</b>",
            glib::markup_escape_text("Select USB devices to redirect")
        ));
        imp.label.set_xalign(0.0);
        imp.label.set_yalign(0.5);
        self.append(&imp.label);

        let manager = match UsbDeviceManager::get(session) {
            Ok(m) => m,
            Err(e) => {
                self.show_info_bar(e.message(), "dialog-warning");
                return;
            }
        };
        let obj = manager.object();
        let mut ids = vec![];
        let weak = self.downgrade();
        ids.push(obj.connect_local("device-added", false, move |a| {
            if let (Some(this), Some(dev)) = (weak.upgrade(), UsbDevice::from_value(&a[1])) {
                this.device_added(&dev);
            }
            None
        }));
        let weak = self.downgrade();
        ids.push(obj.connect_local("device-removed", false, move |a| {
            if let (Some(this), Some(dev)) = (weak.upgrade(), UsbDevice::from_value(&a[1])) {
                this.device_removed(&dev);
            }
            None
        }));
        let weak = self.downgrade();
        ids.push(obj.connect_local("device-error", false, move |a| {
            if let (Some(this), Some(dev)) = (weak.upgrade(), UsbDevice::from_value(&a[1])) {
                this.device_error(&dev);
            }
            None
        }));
        imp.handlers.replace(ids);
        imp.manager.replace(Some(manager.clone()));

        self.add_empty_cd();
        imp.devices.set_orientation(gtk::Orientation::Vertical);
        imp.devices.set_spacing(6);
        self.append(&imp.devices);

        for dev in manager.devices() {
            self.device_added(&dev);
        }
        self.update_status();
    }

    fn manager(&self) -> Option<UsbDeviceManager> {
        self.imp().manager.borrow().clone()
    }

    /// `spice_usb_device_widget_add_err_msg`: appended once.
    fn add_err_msg(&self, msg: String) {
        let mut err = self.imp().err_msg.borrow_mut();
        match err.as_mut() {
            Some(old) if old.contains(&msg) => {}
            Some(old) => *old = format!("{old}\n{msg}"),
            None => *err = Some(msg),
        }
    }

    fn hide_info_bar(&self) {
        if let Some(bar) = self.imp().info_bar.take() {
            self.remove(&bar);
        }
    }

    /// `spice_usb_device_widget_show_info_bar`, under the status line.
    fn show_info_bar(&self, message: &str, icon: &str) {
        self.hide_info_bar();
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        bar.set_margin_start(12);
        bar.set_halign(gtk::Align::Fill);
        bar.append(&gtk::Image::from_icon_name(icon));
        let label = gtk::Label::new(Some(message));
        label.set_hexpand(true);
        label.set_xalign(0.0);
        label.set_wrap(true);
        bar.append(&label);
        self.insert_child_after(&bar, Some(&self.imp().label));
        self.imp().info_bar.replace(Some(bar));
    }

    /// `spice_usb_device_widget_add_empty_cd`.
    fn add_empty_cd(&self) {
        let empty_cd = gtk::CheckButton::with_label("SPICE CD (empty)");
        if let Some(label) = empty_cd.last_child().and_downcast::<gtk::Label>() {
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        }
        empty_cd.set_margin_start(12);
        let weak = self.downgrade();
        empty_cd.connect_toggled(move |toggle| {
            if let Some(this) = weak.upgrade() {
                this.empty_cd_clicked(toggle);
            }
        });
        self.append(&empty_cd);
        self.imp().empty_cd.replace(Some(empty_cd));
    }

    /// `empty_cd_clicked_cb`: pick an image or device to share.
    fn empty_cd_clicked(&self, toggle: &gtk::CheckButton) {
        if !toggle.is_active() {
            return;
        }
        toggle.set_active(false);

        let dialog = gtk::FileDialog::builder()
            .title("Select ISO file or device")
            .modal(true)
            .build();
        let parent = self.root().and_downcast::<gtk::Window>();
        let weak = self.downgrade();
        dialog.open(parent.as_ref(), gio::Cancellable::NONE, move |res| {
            let Some(this) = weak.upgrade() else { return };
            let Some(path) = res.ok().and_then(|f| f.path()) else {
                return;
            };
            let Some(manager) = this.manager() else {
                return;
            };
            let filename = path.to_string_lossy().into_owned();
            if let Err(e) = manager.create_shared_cd_device(&filename) {
                let basename = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or(filename);
                this.add_err_msg(format!("shared CD {basename}, {}", e.message()));
                this.update_status();
            }
        });
    }

    /// `spice_usb_device_widget_update_status`.
    fn update_status(&self) {
        let imp = self.imp();
        let Some(manager) = self.manager() else {
            return;
        };
        let redirecting = manager.is_redirecting();

        let free = manager.object().property::<i32>("free-channels");
        let str = if free == 1 {
            format!("Select USB devices to redirect ({free} free channel)")
        } else {
            format!("Select USB devices to redirect ({free} free channels)")
        };
        imp.label
            .set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&str)));

        // `check_can_redirect`, for each device.
        let checks: Vec<_> = imp
            .checks
            .borrow()
            .iter()
            .map(|(c, d, _)| (c.clone(), d.clone()))
            .collect();
        for (check, dev) in &checks {
            let can_redirect = if redirecting {
                false
            } else {
                match manager.can_redirect_device(dev) {
                    Ok(()) => true,
                    Err(e) => {
                        self.add_err_msg(format!("Can't redirect: {}", e.message()));
                        false
                    }
                }
            };
            check.set_sensitive(can_redirect);
        }

        if let Some(msg) = imp.err_msg.take() {
            self.show_info_bar(&msg, "dialog-warning");
        } else if redirecting {
            self.show_info_bar("Redirecting USB Device...", "dialog-information");
        } else {
            self.hide_info_bar();
        }

        if checks.is_empty() {
            self.show_info_bar("No USB devices detected", "dialog-information");
        }
    }

    fn set_check_quietly(&self, check: &gtk::CheckButton, active: bool) {
        let checks = self.imp().checks.borrow();
        let Some((_, _, id)) = checks.iter().find(|(c, _, _)| c == check) else {
            check.set_active(active);
            return;
        };
        check.block_signal(id);
        check.set_active(active);
        check.unblock_signal(id);
    }

    /// `checkbox_clicked_cb`: redirect or give back the device.
    fn checkbox_clicked(&self, check: &gtk::CheckButton, device: &UsbDevice) {
        let Some(manager) = self.manager() else {
            return;
        };
        let weak = self.downgrade();
        let check2 = check.clone();
        if check.is_active() {
            let dev = device.clone();
            manager.connect_device_async(device, move |res| {
                let Some(this) = weak.upgrade() else { return };
                if let Err(e) = res {
                    let format = this.imp().device_format_string.borrow().clone();
                    let desc = dev.description(format.as_deref());
                    let msg = format!("Could not redirect {desc}: {}", e.message());
                    if let Some(cb) = this.imp().connect_failed.borrow().as_ref() {
                        cb(&msg);
                    }
                    // Don't trigger a disconnect if connect failed.
                    this.set_check_quietly(&check2, false);
                }
                this.update_status();
            });
        } else {
            manager.disconnect_device_async(device, move |_res| {
                if let Some(this) = weak.upgrade() {
                    this.update_status();
                }
            });
        }
        self.update_status();
    }

    /// `device_added_cb`.
    fn device_added(&self, device: &UsbDevice) {
        let Some(manager) = self.manager() else {
            return;
        };
        let format = self.imp().device_format_string.borrow().clone();
        let check = gtk::CheckButton::with_label(&device.description(format.as_deref()));
        if manager.is_device_connected(device) {
            check.set_active(true);
        }
        let weak = self.downgrade();
        let dev = device.clone();
        let id = check.connect_toggled(move |check| {
            if let Some(this) = weak.upgrade() {
                this.checkbox_clicked(check, &dev);
            }
        });
        check.set_margin_start(12);
        // Newest first, under the CD entry.
        self.imp().devices.prepend(&check);
        self.imp()
            .checks
            .borrow_mut()
            .push((check.clone(), device.clone(), id));

        if manager.is_device_shared_cd(device) && !check.is_active() {
            // The toggle starts the redirection.
            check.set_active(true);
        }
        self.update_status();
    }

    /// `device_removed_cb`.
    fn device_removed(&self, device: &UsbDevice) {
        let removed: Vec<_> = {
            let mut checks = self.imp().checks.borrow_mut();
            let (gone, keep) = std::mem::take(&mut *checks)
                .into_iter()
                .partition(|(_, d, _)| d == device);
            *checks = keep;
            gone
        };
        for (check, _, _) in removed {
            self.imp().devices.remove(&check);
        }
        self.update_status();
    }

    /// `device_error_cb`: the redirection broke; uncheck it.
    fn device_error(&self, device: &UsbDevice) {
        let checks: Vec<_> = self
            .imp()
            .checks
            .borrow()
            .iter()
            .filter(|(_, d, _)| d == device)
            .map(|(c, _, _)| c.clone())
            .collect();
        for check in checks {
            check.set_active(false);
        }
        self.update_status();
    }
}
