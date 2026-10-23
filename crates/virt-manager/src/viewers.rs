//! Upstream's `virtManager/details/viewers.py`: the `Viewer` wrapper the
//! console page drives, around the graphical display widget.
//!
//! Only `VNCViewer` exists so far; `SpiceViewer` is the next slice and
//! slots in as another [`ViewerKind`].

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::gio;
use gtk::glib;
use gtk::glib::translate::IntoGlib;
use gtk::prelude::*;
use virtinst_core::connection::Uri;

use crate::domain::VmmDomain;
use crate::engine;
use crate::gvnc;
use crate::sshtunnels::{ConnectionInfo, SshTunnels};
use crate::vncdisplay::{DisplayEvent, VncDisplay};

/// `Viewer`'s signals.
#[derive(Debug, Clone)]
pub enum ViewerEvent {
    AddDisplayWidget(gtk::Widget),
    DesktopResolutionChanged,
    PointerGrab,
    PointerUngrab,
    KeyboardGrab,
    KeyboardUngrab,
    Connected,
    /// `(errdetails, ssherr)`
    Disconnected(Option<String>, String),
    /// `(errmsg, viewer_will_disconnect)`
    AuthError(String, bool),
    /// `(with_password, with_username)`
    NeedAuth(bool, bool),
}

enum ViewerKind {
    Vnc(VncDisplay),
}

type Listener = Rc<dyn Fn(ViewerEvent)>;

struct Inner {
    vm: VmmDomain,
    ginfo: ConnectionInfo,
    display: RefCell<Option<ViewerKind>>,
    tunnels: RefCell<SshTunnels>,
    keyboard_grab: Cell<bool>,
    desktop_resolution: Cell<(i32, i32)>,
    callback: RefCell<Option<Listener>>,
    grabkeys_handler: RefCell<Option<(gio::Settings, glib::SignalHandlerId)>>,
}

#[derive(Clone)]
pub struct Viewer(Rc<Inner>);

impl Viewer {
    /// `VNCViewer(vm, ginfo)`.
    pub fn new_vnc(vm: &VmmDomain, ginfo: ConnectionInfo) -> Viewer {
        let tunnels = SshTunnels::new(&ginfo);
        let this = Viewer(Rc::new(Inner {
            vm: vm.clone(),
            ginfo,
            display: RefCell::default(),
            tunnels: RefCell::new(tunnels),
            keyboard_grab: Cell::new(false),
            desktop_resolution: Cell::new((0, 0)),
            callback: RefCell::default(),
            grabkeys_handler: RefCell::default(),
        }));
        let weak = Rc::downgrade(&this.0);
        let handler = engine::get().config().on_keys_combination_changed(move || {
            if let Some(inner) = weak.upgrade() {
                Viewer(inner).refresh_grab_keys();
            }
        });
        this.0.grabkeys_handler.replace(Some(handler));
        this
    }

    pub fn same(&self, other: &Viewer) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    pub fn connect_event(&self, f: impl Fn(ViewerEvent) + 'static) {
        self.0.callback.replace(Some(Rc::new(f)));
    }

    fn emit(&self, ev: ViewerEvent) {
        let cb = self.0.callback.borrow().clone();
        if let Some(cb) = cb {
            cb(ev);
        }
    }

    fn weak(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    fn vnc(&self) -> Option<VncDisplay> {
        self.0
            .display
            .borrow()
            .as_ref()
            .map(|ViewerKind::Vnc(d)| d.clone())
    }

    /// `_cleanup`.
    pub fn cleanup(&self) {
        if let Some(d) = self.vnc() {
            d.close();
            d.destroy();
            if let Some(parent) = d.widget().parent() {
                if let Some(b) = parent.downcast_ref::<gtk::Box>() {
                    b.remove(d.widget());
                }
            }
        }
        self.0.display.replace(None);
        self.0.tunnels.borrow_mut().close_all();
        if let Some((settings, id)) = self.0.grabkeys_handler.take() {
            settings.disconnect(id);
        }
        self.0.callback.replace(None);
    }

    // Internal helpers

    /// `_set_display`.
    fn set_display(&self, display: VncDisplay) {
        self.0
            .display
            .replace(Some(ViewerKind::Vnc(display.clone())));
        self.refresh_grab_keys();
        self.emit(ViewerEvent::AddDisplayWidget(
            display.widget().clone().upcast(),
        ));
        self.connect_display_signals(&display);
        display.widget().set_visible(true);
    }

    /// `_get_fd_for_open`.
    async fn get_fd_for_open(&self) -> Result<Option<i32>, String> {
        let ginfo = &self.0.ginfo;
        if ginfo.need_tunnel() {
            return self
                .0
                .tunnels
                .borrow_mut()
                .open_new()
                .map(Some)
                .map_err(|e| e.to_string());
        }
        let uri = Uri::parse(&self.0.vm.conn().map(|c| c.get_uri()).unwrap_or_default());
        if uri.is_remote() {
            // OpenGraphics only works for local libvirtd connections
            return Ok(None);
        }
        if ginfo.gtlsport.is_some() && ginfo.gport.is_none() {
            return Ok(None);
        }
        // conn.support.domain_open_graphics(): qemu only.
        if !uri.is_qemu() {
            return Ok(None);
        }
        self.0
            .vm
            .open_graphics_fd(ginfo.gidx as u32)
            .await
            .map(Some)
    }

    /// `_refresh_grab_keys`.
    fn refresh_grab_keys(&self) {
        let Some(d) = self.vnc() else { return };
        let keys = engine::get().config().get_keys_combination();
        let parsed: Result<Vec<u32>, _> = keys.split(',').map(|k| k.trim().parse()).collect();
        match parsed {
            Ok(keys) if !keys.is_empty() => d.set_grab_keys(keys),
            _ => eprintln!("Error in grab_keys configuration in GSettings: {keys}"),
        }
    }

    fn emit_disconnected(&self, errdetails: Option<String>) {
        let ssherr = self.0.tunnels.borrow().get_err_output();
        self.emit(ViewerEvent::Disconnected(errdetails, ssherr));
    }

    /// `_set_desktop_resolution`.
    fn set_desktop_resolution(&self, w: i32, h: i32) {
        if w == 0 || h == 0 {
            return;
        }
        if self.0.desktop_resolution.get() == (w, h) {
            return;
        }
        self.0.desktop_resolution.set((w, h));
        self.emit(ViewerEvent::DesktopResolutionChanged);
    }

    // VNCViewer

    /// `VNCViewer._connect_display_signals` and the callbacks behind it.
    fn connect_display_signals(&self, display: &VncDisplay) {
        let weak = self.weak();
        display.connect_event(move |ev| {
            let Some(inner) = weak.upgrade() else { return };
            let this = Viewer(inner);
            match ev {
                DisplayEvent::PointerGrab => this.emit(ViewerEvent::PointerGrab),
                DisplayEvent::PointerUngrab => this.emit(ViewerEvent::PointerUngrab),
                DisplayEvent::KeyboardGrab => {
                    this.0.keyboard_grab.set(true);
                    this.emit(ViewerEvent::KeyboardGrab);
                }
                DisplayEvent::KeyboardUngrab => {
                    this.0.keyboard_grab.set(false);
                    this.emit(ViewerEvent::KeyboardUngrab);
                }
                DisplayEvent::AuthCredential(creds) => this.auth_credential(&creds),
                DisplayEvent::AuthFailure(msg) => {
                    this.emit(ViewerEvent::AuthError(msg, true));
                }
                DisplayEvent::Initialized => {
                    this.0.tunnels.borrow().unlock();
                    this.emit(ViewerEvent::Connected);
                }
                DisplayEvent::Disconnected => {
                    this.0.tunnels.borrow().unlock();
                    this.emit_disconnected(None);
                }
                DisplayEvent::DesktopResize(w, h) => this.set_desktop_resolution(w, h),
                DisplayEvent::Error(msg) => eprintln!("VNC error: {msg}"),
            }
        });
    }

    /// `_auth_credential`.
    fn auth_credential(&self, creds: &[i32]) {
        let mut with_username = false;
        let mut with_password = false;
        for cred in creds {
            match *cred {
                gvnc::CREDENTIAL_PASSWORD => with_password = true,
                gvnc::CREDENTIAL_USERNAME => with_username = true,
                gvnc::CREDENTIAL_CLIENTNAME => {
                    if let Some(d) = self.vnc() {
                        d.set_credential(*cred, "libvirt-vnc");
                    }
                }
                other => {
                    let errmsg = format!(
                        "Unable to provide requested credentials to the VNC server.\n\
                         The credential type {other} is not supported"
                    );
                    self.emit(ViewerEvent::AuthError(errmsg, true));
                    return;
                }
            }
        }
        if with_username || with_password {
            self.emit(ViewerEvent::NeedAuth(with_password, with_username));
        }
    }

    /// `_sync_force_size`.
    fn sync_force_size(&self) {
        if let Some(d) = self.vnc() {
            d.set_force_size(!d.get_scaling() && !d.get_allow_resize());
        }
    }

    // APIs accessed by vmmConsolePages

    pub fn console_is_open(&self) -> bool {
        self.vnc().is_some_and(|d| d.is_open())
    }

    pub fn console_grab_focus(&self) {
        if let Some(d) = self.vnc() {
            d.widget().grab_focus();
        }
    }

    pub fn console_get_framebuffer(&self) -> Option<gvnc::Framebuffer> {
        self.vnc().and_then(|d| d.get_framebuffer())
    }

    /// `console_open` / `_open`: creates the display, then connects
    /// through a tunnel, a libvirt-provided fd, or straight to the host.
    pub async fn console_open(&self) -> Result<(), String> {
        // _init_display
        let display = VncDisplay::new();
        display.set_keep_aspect_ratio(true);
        self.set_display(display.clone());
        if let Some(err) = self.0.ginfo.bad_config() {
            return Err(err);
        }
        match self.get_fd_for_open().await? {
            Some(fd) => {
                display.open_fd(fd);
            }
            None => {
                let (host, port, _) = self.0.ginfo.get_conn_host();
                let port = port.unwrap_or_default();
                eprintln!("VNC connecting to host={host} port={port}");
                display.open_host(&host, &port);
            }
        }
        Ok(())
    }

    pub fn console_set_password(&self, val: &str) {
        if let Some(d) = self.vnc() {
            d.set_credential(gvnc::CREDENTIAL_PASSWORD, val);
        }
    }

    pub fn console_set_username(&self, val: &str) {
        if let Some(d) = self.vnc() {
            d.set_credential(gvnc::CREDENTIAL_USERNAME, val);
        }
    }

    /// `console_send_keys`: X keysym names, e.g. `Control_L`.
    pub fn console_send_keys(&self, keys: &[&str]) {
        let Some(d) = self.vnc() else { return };
        let keyvals: Vec<u32> = keys
            .iter()
            .filter_map(|k| gtk::gdk::Key::from_name(*k))
            .map(|k| k.into_glib())
            .collect();
        d.send_keys(&keyvals);
    }

    pub fn console_get_grab_keys(&self) -> String {
        self.vnc()
            .map(|d| d.get_grab_keys_string())
            .unwrap_or_default()
    }

    pub fn console_get_preferred_size(&self) -> (i32, i32) {
        self.0.desktop_resolution.get()
    }

    pub fn console_set_scaling(&self, val: bool) {
        if let Some(d) = self.vnc() {
            d.set_scaling(val);
            self.sync_force_size();
        }
    }

    pub fn console_set_resizeguest(&self, val: bool) {
        if let Some(d) = self.vnc() {
            d.set_allow_resize(val);
            self.sync_force_size();
        }
    }

    pub fn console_has_usb_redirection(&self) -> bool {
        false
    }
}
