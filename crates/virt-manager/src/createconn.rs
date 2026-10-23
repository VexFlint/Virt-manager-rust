//! Upstream's `virtManager/createconn.py` + `ui/createconn.ui`
//! (`vmmCreateConn`): File > Add Connection.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::connection::VmmConnection;
use crate::engine;
use crate::error;

/// Upstream's `HV_*` ids, in combo order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hv {
    Qemu,
    QemuSession,
    Xen,
    Lxc,
    Bhyve,
    Vz,
    Custom,
}

const HVS: &[(Hv, &str)] = &[
    (Hv::Qemu, "QEMU/KVM"),
    (Hv::QemuSession, "QEMU/KVM user session"),
    (Hv::Xen, "Xen"),
    (Hv::Lxc, "Libvirt-LXC"),
    (Hv::Bhyve, "Bhyve"),
    (Hv::Vz, "Virtuozzo"),
    (Hv::Custom, "Custom URI..."),
];

/// Python's `urllib.parse.quote` with its default `safe='/'`.
fn url_quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `generate_uri`.
pub fn generate_uri(hv: Hv, host: &str, user: &str, is_remote: bool) -> String {
    let host = host.trim();
    let hvstr = match hv {
        Hv::Xen => "xen",
        Hv::Qemu | Hv::QemuSession => "qemu",
        Hv::Bhyve => "bhyve",
        Hv::Vz => "vz",
        _ => "lxc",
    };
    let mut addrstr = String::new();
    if !user.is_empty() {
        addrstr.push_str(&url_quote(user));
        addrstr.push('@');
    }
    if host.matches(':').count() > 1 {
        addrstr.push_str(&format!("[{host}]"));
    } else {
        addrstr.push_str(host);
    }
    let hoststr = if is_remote {
        format!("+ssh://{addrstr}/")
    } else {
        ":///".to_string()
    };
    let mut uri = format!("{hvstr}{hoststr}");
    match hv {
        Hv::Qemu | Hv::Bhyve | Hv::Vz => uri.push_str("system"),
        Hv::QemuSession => uri.push_str("session"),
        _ => {}
    }
    uri
}

struct Widgets {
    window: gtk::Window,
    hypervisor: gtk::DropDown,
    session_warning: gtk::Box,
    connect_remote: gtk::CheckButton,
    username_label: gtk::Label,
    username: gtk::Entry,
    hostname_label: gtk::Label,
    hostname: gtk::Entry,
    autoconnect: gtk::CheckButton,
    uri_generated_label: gtk::Label,
    uri_label: gtk::Label,
    uri_custom_label: gtk::Label,
    uri_entry: gtk::Entry,
}

#[derive(Clone)]
pub struct VmmCreateConn(Rc<Widgets>);

thread_local! {
    static INSTANCE: RefCell<Option<VmmCreateConn>> = const { RefCell::new(None) };
}

fn grid_label(text: &str, target: &impl IsA<gtk::Widget>) -> gtk::Label {
    let label = gtk::Label::with_mnemonic(text);
    label.set_xalign(1.0);
    label.set_mnemonic_widget(Some(target));
    label
}

impl VmmCreateConn {
    /// `vmmCreateConn.get_instance(parent).show(parent.topwin)`.
    pub fn show_instance(parent: &gtk::Window) {
        let this = INSTANCE.with(|i| i.borrow_mut().get_or_insert_with(Self::new).clone());
        this.show(parent);
    }

    fn new() -> VmmCreateConn {
        let window = gtk::Window::builder()
            .title("Add Connection")
            .modal(true)
            .resizable(false)
            .hide_on_close(true)
            .build();

        let names: Vec<&str> = HVS.iter().map(|(_, n)| *n).collect();
        let hypervisor = gtk::DropDown::from_strings(&names);
        hypervisor.update_property(&[gtk::accessible::Property::Label("Hypervisor")]);

        let session_warning = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        let warn_icon = gtk::Image::from_icon_name("dialog-warning");
        warn_icon.set_valign(gtk::Align::Start);
        let warn = gtk::Label::new(None);
        warn.set_markup(
            "<small>QEMU usermode session is not the virt-manager\n\
             default. It is likely that any pre-existing QEMU/KVM\n\
             guests will not be available. Networking options\n\
             are very limited. </small>",
        );
        session_warning.append(&warn_icon);
        session_warning.append(&warn);

        let connect_remote = gtk::CheckButton::with_mnemonic("Connect to _remote host over SSH");
        let username = gtk::Entry::new();
        let hostname = gtk::Entry::new();
        hostname.set_hexpand(true);
        let autoconnect = gtk::CheckButton::new();
        let uri_label = gtk::Label::new(Some("uri"));
        uri_label.set_xalign(0.0);
        uri_label.set_selectable(true);
        uri_label.set_max_width_chars(22);
        uri_label.set_wrap(true);
        let uri_entry = gtk::Entry::new();

        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .build();
        let hv_label = grid_label("_Hypervisor:", &hypervisor);
        let username_label = grid_label("_Username:", &username);
        let hostname_label = grid_label("H_ostname:", &hostname);
        let autoconnect_label = grid_label("_Autoconnect:", &autoconnect);
        let uri_generated_label = gtk::Label::new(Some("Generated URI:"));
        uri_generated_label.set_xalign(1.0);
        let uri_custom_label = grid_label("Cu_stom URI:", &uri_entry);

        grid.attach(&hv_label, 0, 0, 1, 1);
        grid.attach(&hypervisor, 1, 0, 1, 1);
        grid.attach(&session_warning, 0, 1, 2, 1);
        grid.attach(&connect_remote, 0, 2, 2, 1);
        grid.attach(&username_label, 0, 3, 1, 1);
        grid.attach(&username, 1, 3, 1, 1);
        grid.attach(&hostname_label, 0, 4, 1, 1);
        grid.attach(&hostname, 1, 4, 1, 1);
        grid.attach(
            &gtk::Separator::new(gtk::Orientation::Horizontal),
            0,
            5,
            2,
            1,
        );
        grid.attach(&autoconnect_label, 0, 6, 1, 1);
        grid.attach(&autoconnect, 1, 6, 1, 1);
        grid.attach(&uri_generated_label, 0, 7, 1, 1);
        grid.attach(&uri_label, 1, 7, 1, 1);
        grid.attach(&uri_custom_label, 0, 8, 1, 1);
        grid.attach(&uri_entry, 1, 8, 1, 1);

        let cancel = gtk::Button::with_mnemonic("_Cancel");
        let connect = gtk::Button::with_mnemonic("Co_nnect");
        connect.add_css_class("suggested-action");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&connect);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);
        vbox.append(&grid);
        vbox.append(&buttons);
        window.set_child(Some(&vbox));
        window.set_default_widget(Some(&connect));

        let this = VmmCreateConn(Rc::new(Widgets {
            window,
            hypervisor,
            session_warning,
            connect_remote,
            username_label,
            username,
            hostname_label,
            hostname,
            autoconnect,
            uri_generated_label,
            uri_label,
            uri_custom_label,
            uri_entry,
        }));

        let w = &this.0;
        let t = this.clone();
        w.hypervisor
            .connect_selected_notify(move |_| t.hypervisor_changed());
        let t = this.clone();
        w.connect_remote
            .connect_toggled(move |_| t.connect_remote_toggled());
        let t = this.clone();
        w.username.connect_changed(move |_| t.populate_uri());
        let t = this.clone();
        w.hostname.connect_changed(move |_| t.populate_uri());
        let t = this.clone();
        cancel.connect_clicked(move |_| t.close());
        let t = this.clone();
        connect.connect_clicked(move |_| t.open_conn());
        w.username.set_activates_default(true);
        w.hostname.set_activates_default(true);
        w.uri_entry.set_activates_default(true);
        this
    }

    fn show(&self, parent: &gtk::Window) {
        let win = &self.0.window;
        if win.is_visible() {
            win.present();
            return;
        }
        self.reset_state();
        win.set_transient_for(Some(parent));
        win.present();
    }

    fn close(&self) {
        self.0.window.set_visible(false);
    }

    fn hv(&self) -> Hv {
        HVS[self.0.hypervisor.selected() as usize].0
    }

    fn reset_state(&self) {
        let w = &self.0;
        // set_default_hypervisor
        let default = engine::default_uri();
        let hv = match default {
            Some(d) if d.starts_with("xen") => Hv::Xen,
            _ => Hv::Qemu,
        };
        let idx = HVS.iter().position(|(h, _)| *h == hv).unwrap_or(0);
        w.hypervisor.set_selected(idx as u32);
        self.hypervisor_changed();
        w.autoconnect.set_sensitive(true);
        w.autoconnect.set_active(true);
        w.hostname.set_text("");
        w.connect_remote.set_active(false);
        w.username.set_text("");
        w.uri_entry.set_text("");
        self.connect_remote_toggled();
        self.populate_uri();
    }

    fn is_remote(&self) -> bool {
        self.0.connect_remote.is_active()
    }

    fn hypervisor_changed(&self) {
        let w = &self.0;
        let hv = self.hv();
        let is_session = hv == Hv::QemuSession;
        let is_custom = hv == Hv::Custom;
        let show_remote = !is_session && !is_custom;
        w.session_warning.set_visible(is_session);
        w.connect_remote.set_visible(show_remote);
        w.username_label.set_visible(show_remote);
        w.username.set_visible(show_remote);
        w.hostname_label.set_visible(show_remote);
        w.hostname.set_visible(show_remote);
        if !show_remote {
            w.connect_remote.set_active(false);
        }
        w.uri_generated_label.set_visible(!is_custom);
        w.uri_label.set_visible(!is_custom);
        w.uri_custom_label.set_visible(is_custom);
        w.uri_entry.set_visible(is_custom);
        if is_custom {
            w.uri_entry.set_text(&w.uri_label.text());
            w.uri_entry.grab_focus();
        }
        self.populate_uri();
    }

    fn connect_remote_toggled(&self) {
        let w = &self.0;
        let is_remote = self.is_remote();
        w.hostname.set_sensitive(is_remote);
        w.autoconnect.set_active(!is_remote);
        w.username.set_sensitive(is_remote);
        if is_remote && w.username.text().is_empty() {
            w.username.set_text("root");
        }
        self.populate_uri();
    }

    fn generate_uri(&self) -> String {
        let w = &self.0;
        generate_uri(
            self.hv(),
            &w.hostname.text(),
            &w.username.text(),
            self.is_remote(),
        )
    }

    fn populate_uri(&self) {
        self.0.uri_label.set_text(&self.generate_uri());
    }

    fn open_conn(&self) {
        let w = &self.0;
        if self.is_remote() && w.hostname.text().is_empty() {
            let win = w.window.clone();
            glib::spawn_future_local(async move {
                error::val_err(Some(&win), "A hostname is required for remote connections.").await;
            });
            return;
        }
        let auto = w.autoconnect.is_sensitive() && w.autoconnect.is_active();
        let uri = if w.uri_label.is_visible() {
            self.generate_uri()
        } else {
            w.uri_entry.text().to_string()
        };

        let cm = engine::get().connmanager();
        let conn = cm.add_conn(&uri);
        cm.set_autoconnect(&conn, auto);
        if conn.is_active() {
            self.close();
            return;
        }
        w.window.set_cursor_from_name(Some("progress"));
        let this = self.clone();
        let conn2 = conn.clone();
        conn.open(move |err| this.conn_open_completed(&conn2, err));
    }

    fn conn_open_completed(
        &self,
        conn: &VmmConnection,
        err: Option<crate::connection::ConnectError>,
    ) {
        self.0.window.set_cursor_from_name(None);
        let Some(err) = err else {
            self.close();
            return;
        };
        let this = self.clone();
        let uri = conn.get_uri();
        glib::spawn_future_local(async move {
            let msg = format!(
                "{}\n\nWould you still like to remember this connection?",
                err.msg
            );
            let (remember, _) = error::run_dialog(
                Some(&this.0.window),
                error::DialogSpec {
                    kind: error::MessageType::Question,
                    buttons: error::Buttons::YesNo,
                    text1: &msg,
                    text2: None,
                    title: &err.title,
                    details: Some(&err.details),
                    chktext: None,
                },
            )
            .await;
            if remember {
                this.close();
            } else {
                engine::get().connmanager().remove_conn(&uri);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_uris() {
        assert_eq!(generate_uri(Hv::Qemu, "", "", false), "qemu:///system");
        assert_eq!(
            generate_uri(Hv::QemuSession, "", "", false),
            "qemu:///session"
        );
        assert_eq!(generate_uri(Hv::Xen, "", "", false), "xen:///");
        assert_eq!(generate_uri(Hv::Lxc, "", "", false), "lxc:///");
        assert_eq!(generate_uri(Hv::Bhyve, "", "", false), "bhyve:///system");
        assert_eq!(generate_uri(Hv::Vz, "", "", false), "vz:///system");
    }

    #[test]
    fn remote_uris() {
        assert_eq!(
            generate_uri(Hv::Qemu, " host ", "root", true),
            "qemu+ssh://root@host/system"
        );
        assert_eq!(
            generate_uri(Hv::Qemu, "fe80::1", "", true),
            "qemu+ssh://[fe80::1]/system"
        );
        assert_eq!(
            generate_uri(Hv::Xen, "h", "a b@c", true),
            "xen+ssh://a%20b%40c@h/"
        );
    }
}
