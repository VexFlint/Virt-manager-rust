//! Upstream's `virtManager/details/viewers.py`: the `Viewer` wrapper the
//! console page drives, around the graphical display widget, for VNC
//! (`VNCViewer`) and SPICE (`SpiceViewer`).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::gio;
use gtk::glib;
use gtk::glib::translate::IntoGlib;
use gtk::prelude::*;
use virtinst_core::connection::Uri;

use crate::display::{DisplayWidget, FrameCopy};
use crate::domain::VmmDomain;
use crate::engine;
use crate::gvnc;
use crate::spice::{self, ChannelKind, Session, UsbDeviceManager};
use crate::spicedisplay::{SpiceDisplay, SpiceDisplayEvent};
use crate::spicegtksession::SpiceGtkSession;
use crate::sshtunnels::{ConnectionInfo, SshTunnels};
use crate::usbdevicewidget::UsbDeviceWidget;
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
    /// `usb-redirect-error`
    UsbRedirectError(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewerType {
    Vnc,
    Spice,
}

#[derive(Clone)]
enum ViewerKind {
    Vnc(VncDisplay),
    Spice(SpiceDisplay),
}

impl ViewerKind {
    fn widget(&self) -> &DisplayWidget {
        match self {
            ViewerKind::Vnc(d) => d.widget(),
            ViewerKind::Spice(d) => d.widget(),
        }
    }
}

/// `SpiceViewer`'s own state.
#[derive(Default)]
struct SpiceState {
    session: Option<Session>,
    main_channel: Option<glib::Object>,
    audio: Option<glib::Object>,
    /// Every channel seen, with the handlers we put on it.
    channels: Vec<(glib::Object, Vec<glib::SignalHandlerId>)>,
    session_handlers: Vec<glib::SignalHandlerId>,
    usbdev_manager: Option<UsbDeviceManager>,
    usbdev_handlers: Vec<glib::SignalHandlerId>,
}

type Listener = Rc<dyn Fn(ViewerEvent)>;

struct Inner {
    vm: VmmDomain,
    ginfo: ConnectionInfo,
    viewer_type: ViewerType,
    display: RefCell<Option<ViewerKind>>,
    spice: RefCell<SpiceState>,
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
        Viewer::new(vm, ginfo, ViewerType::Vnc)
    }

    /// `SpiceViewer(vm, ginfo)`.
    pub fn new_spice(vm: &VmmDomain, ginfo: ConnectionInfo) -> Viewer {
        Viewer::new(vm, ginfo, ViewerType::Spice)
    }

    fn new(vm: &VmmDomain, ginfo: ConnectionInfo, viewer_type: ViewerType) -> Viewer {
        let tunnels = SshTunnels::new(&ginfo);
        let this = Viewer(Rc::new(Inner {
            vm: vm.clone(),
            ginfo,
            viewer_type,
            display: RefCell::default(),
            spice: RefCell::default(),
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

    fn display(&self) -> Option<ViewerKind> {
        self.0.display.borrow().clone()
    }

    fn vnc(&self) -> Option<VncDisplay> {
        match self.display() {
            Some(ViewerKind::Vnc(d)) => Some(d),
            _ => None,
        }
    }

    fn spice_display(&self) -> Option<SpiceDisplay> {
        match self.display() {
            Some(ViewerKind::Spice(d)) => Some(d),
            _ => None,
        }
    }

    /// `_cleanup`.
    pub fn cleanup(&self) {
        self.close();
        if let Some(d) = self.0.display.take() {
            match &d {
                ViewerKind::Vnc(d) => d.destroy(),
                ViewerKind::Spice(d) => d.destroy(),
            }
            if let Some(parent) = d.widget().parent() {
                if let Some(b) = parent.downcast_ref::<gtk::Box>() {
                    b.remove(d.widget());
                }
            }
        }
        self.0.tunnels.borrow_mut().close_all();
        if let Some((settings, id)) = self.0.grabkeys_handler.take() {
            settings.disconnect(id);
        }
        self.0.callback.replace(None);
    }

    // Internal helpers

    /// `_set_display`.
    fn set_display(&self, display: ViewerKind) {
        self.0.display.replace(Some(display.clone()));
        self.refresh_grab_keys();
        self.emit(ViewerEvent::AddDisplayWidget(
            display.widget().clone().upcast(),
        ));
        match &display {
            ViewerKind::Vnc(d) => self.connect_vnc_display_signals(d),
            ViewerKind::Spice(d) => self.connect_spice_display_signals(d),
        }
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
            // This makes spice loop requesting an fd. Disable until spice is
            // fixed: https://bugzilla.redhat.com/show_bug.cgi?id=1334071
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
        let Some(d) = self.display() else { return };
        let keys = engine::get().config().get_keys_combination();
        if keys.is_empty() {
            return;
        }
        let parsed: Result<Vec<u32>, _> = keys.split(',').map(|k| k.trim().parse()).collect();
        match parsed {
            Ok(keys) => match d {
                ViewerKind::Vnc(d) => d.set_grab_keys(keys),
                ViewerKind::Spice(d) => d.set_grab_keys(keys),
            },
            Err(_) => eprintln!("Error in grab_keys configuration in GSettings: {keys}"),
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

    /// `_keyboard_grab_cb`.
    fn set_keyboard_grab(&self, grab: bool) {
        self.0.keyboard_grab.set(grab);
        self.emit(if grab {
            ViewerEvent::KeyboardGrab
        } else {
            ViewerEvent::KeyboardUngrab
        });
    }

    /// `_close`.
    fn close(&self) {
        match self.0.viewer_type {
            ViewerType::Vnc => {
                if let Some(d) = self.vnc() {
                    d.close();
                }
            }
            ViewerType::Spice => self.spice_close(),
        }
    }

    // VNCViewer

    /// `VNCViewer._connect_display_signals` and the callbacks behind it.
    fn connect_vnc_display_signals(&self, display: &VncDisplay) {
        let weak = self.weak();
        display.connect_event(move |ev| {
            let Some(inner) = weak.upgrade() else { return };
            let this = Viewer(inner);
            match ev {
                DisplayEvent::PointerGrab => this.emit(ViewerEvent::PointerGrab),
                DisplayEvent::PointerUngrab => this.emit(ViewerEvent::PointerUngrab),
                DisplayEvent::KeyboardGrab => this.set_keyboard_grab(true),
                DisplayEvent::KeyboardUngrab => this.set_keyboard_grab(false),
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

    // SpiceViewer

    /// `SpiceViewer._connect_display_signals`.
    fn connect_spice_display_signals(&self, display: &SpiceDisplay) {
        let weak = self.weak();
        display.connect_event(move |ev| {
            let Some(inner) = weak.upgrade() else { return };
            let this = Viewer(inner);
            match ev {
                SpiceDisplayEvent::MouseGrab(true) => this.emit(ViewerEvent::PointerGrab),
                SpiceDisplayEvent::MouseGrab(false) => this.emit(ViewerEvent::PointerUngrab),
                SpiceDisplayEvent::KeyboardGrab(grab) => this.set_keyboard_grab(grab),
            }
        });
    }

    /// `_create_spice_session`.
    fn create_spice_session(&self) -> Session {
        let session = Session::new();
        let gtk_session = SpiceGtkSession::get(&session);
        gtk_session.set_auto_clipboard(true);

        let weak = self.weak();
        let id = session
            .object()
            .connect_local("channel-new", false, move |a| {
                if let (Some(inner), Ok(ch)) = (weak.upgrade(), a[1].get::<glib::Object>()) {
                    Viewer(inner).spice_channel_new(&ch);
                }
                None
            });
        {
            let mut st = self.0.spice.borrow_mut();
            st.session = Some(session.clone());
            st.session_handlers.push(id);
        }

        // Distros might have usb redirection compiled out, like OpenBSD
        // https://bugzilla.redhat.com/show_bug.cgi?id=1348479
        match UsbDeviceManager::get(&session) {
            Ok(manager) => {
                let mut ids = vec![];
                for signal in ["auto-connect-failed", "device-error"] {
                    let weak = self.weak();
                    ids.push(manager.object().connect_local(signal, false, move |a| {
                        if let (Some(inner), Ok(err)) = (weak.upgrade(), a[2].get::<glib::Error>())
                        {
                            Viewer(inner).usbdev_redirect_error(err.message());
                        }
                        None
                    }));
                }
                if engine::get().config().get_auto_usbredir() {
                    gtk_session.set_auto_usbredir(true);
                }
                let mut st = self.0.spice.borrow_mut();
                st.usbdev_manager = Some(manager);
                st.usbdev_handlers = ids;
            }
            Err(e) => eprintln!("Error initializing spice usb device manager: {e}"),
        }
        session
    }

    /// `_usbdev_redirect_error`.
    fn usbdev_redirect_error(&self, errstr: &str) {
        self.emit(ViewerEvent::UsbRedirectError(errstr.to_string()));
    }

    fn spice_session(&self) -> Option<Session> {
        self.0.spice.borrow().session.clone()
    }

    fn track_channel_handler(&self, ch: &glib::Object, id: glib::SignalHandlerId) {
        let mut st = self.0.spice.borrow_mut();
        match st.channels.iter_mut().find(|(c, _)| c == ch) {
            Some((_, ids)) => ids.push(id),
            None => st.channels.push((ch.clone(), vec![id])),
        }
    }

    /// `_main_channel_event_cb`.
    fn main_channel_event(&self, channel: &glib::Object, event: i32) {
        self.0.tunnels.borrow().unlock();
        if event == spice::CHANNEL_CLOSED {
            self.emit_disconnected(None);
        } else if event == spice::CHANNEL_ERROR_AUTH {
            let password = self
                .spice_session()
                .and_then(|s| s.object().property::<Option<String>>("password"))
                .unwrap_or_default();
            if password.is_empty() {
                eprintln!(
                    "Spice channel received ERROR_AUTH, but no password set, \
                     assuming it wants credentials."
                );
                self.emit(ViewerEvent::NeedAuth(true, false));
            } else {
                eprintln!(
                    "Spice channel received ERROR_AUTH, but a password is already \
                     set. Assuming authentication failed."
                );
                let msg = spice::channel_error(channel).unwrap_or_default();
                self.emit(ViewerEvent::AuthError(msg, false));
            }
        } else if event >= spice::CHANNEL_ERROR_FIRST {
            let error = spice::channel_error(channel);
            let nick = spice::channel_event_nick(event);
            eprintln!(
                "Spice channel event={nick} message={}",
                error.as_deref().unwrap_or("None")
            );
            let mut msg = format!("Encountered SPICE {nick}");
            if let Some(error) = error {
                msg.push_str(&format!(": {error}"));
            }
            self.emit_disconnected(Some(msg));
        }
    }

    /// `_channel_open_fd_request`: a channel of an fd-opened session
    /// wants its own fd. Any event on it releases the tunnel lock
    /// (`_fd_channel_event_cb`).
    fn channel_open_fd_request(&self, channel: &glib::Object) {
        if self.spice_session().is_none() {
            // Can happen if we close the details window while initially
            // connecting to spice and channel FD requests still roll in.
            return;
        }
        eprintln!("Requesting fd for channel: {}", channel.type_().name());
        let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
        let weak = self.weak();
        let h = handler.clone();
        let id = channel.connect_local("channel-event", true, move |a| {
            if let Ok(ch) = a[0].get::<glib::Object>() {
                if let Some(id) = h.borrow_mut().take() {
                    ch.disconnect(id);
                }
            }
            if let Some(inner) = weak.upgrade() {
                inner.tunnels.borrow().unlock();
            }
            None
        });
        handler.replace(Some(id));

        let this = self.clone();
        let channel = channel.clone();
        glib::spawn_future_local(async move {
            match this.get_fd_for_open().await {
                Ok(Some(fd)) => {
                    spice::channel_open_fd(&channel, fd);
                }
                Ok(None) => eprintln!("No fd available for SPICE channel"),
                Err(e) => eprintln!("Error getting fd for SPICE channel: {e}"),
            }
        });
    }

    /// `_channel_new_cb`.
    fn spice_channel_new(&self, channel: &glib::Object) {
        let weak = self.weak();
        let id = channel.connect_local("open-fd", false, move |a| {
            if let (Some(inner), Ok(ch)) = (weak.upgrade(), a[0].get::<glib::Object>()) {
                Viewer(inner).channel_open_fd_request(&ch);
            }
            None
        });
        self.track_channel_handler(channel, id);

        let (have_main, have_audio) = {
            let st = self.0.spice.borrow();
            (st.main_channel.is_some(), st.audio.is_some())
        };
        match spice::channel_kind(channel) {
            ChannelKind::Main if !have_main => {
                self.0.spice.borrow_mut().main_channel = Some(channel.clone());
                let weak = self.weak();
                let id = channel.connect_local("channel-event", true, move |a| {
                    if let (Some(inner), Ok(ch)) = (weak.upgrade(), a[0].get::<glib::Object>()) {
                        Viewer(inner).main_channel_event(&ch, spice::value_enum(&a[1]));
                    }
                    None
                });
                self.track_channel_handler(channel, id);
            }
            ChannelKind::Display if self.display().is_none() => {
                let channel_id = channel.property::<i32>("channel-id");
                if channel_id != 0 {
                    eprintln!("Spice multi-head unsupported");
                    return;
                }
                let Some(session) = self.spice_session() else {
                    return;
                };
                let display = SpiceDisplay::new(&session);
                self.set_display(ViewerKind::Spice(display));

                let weak = self.weak();
                let id = channel.connect_local("display-primary-create", true, move |a| {
                    if let Some(inner) = weak.upgrade() {
                        let w = a[2].get::<i32>().unwrap_or(0);
                        let h = a[3].get::<i32>().unwrap_or(0);
                        Viewer(inner).set_desktop_resolution(w, h);
                    }
                    None
                });
                self.track_channel_handler(channel, id);
                self.emit(ViewerEvent::Connected);
            }
            ChannelKind::Playback | ChannelKind::Record if !have_audio => {
                // It's unclear why we need this audio handle, but it
                // does matter:
                // https://bugzilla.redhat.com/show_bug.cgi?id=1881080
                let audio = self.spice_session().and_then(|s| s.audio());
                self.0.spice.borrow_mut().audio = audio;
            }
            _ => {}
        }
    }

    /// `SpiceViewer._close`.
    fn spice_close(&self) {
        let st = std::mem::take(&mut *self.0.spice.borrow_mut());
        if let Some(session) = &st.session {
            for id in st.session_handlers {
                session.object().disconnect(id);
            }
        }
        for (ch, ids) in st.channels {
            for id in ids {
                ch.disconnect(id);
            }
        }
        if let Some(manager) = &st.usbdev_manager {
            for id in st.usbdev_handlers {
                manager.object().disconnect(id);
            }
        }
        if let Some(session) = st.session {
            session.disconnect();
        }
    }

    /// `SpiceViewer._open_host`.
    fn spice_open_host(&self) {
        let (host, port, tlsport) = self.0.ginfo.get_conn_host();
        let session = self.create_spice_session();
        eprintln!(
            "Spice connecting to host={host} port={} tlsport={}",
            port.as_deref().unwrap_or("None"),
            tlsport.as_deref().unwrap_or("None")
        );
        let obj = session.object();
        obj.set_property("host", host.as_str());
        if let Some(port) = port.filter(|p| !p.is_empty()) {
            obj.set_property("port", port.as_str());
        }
        if let Some(tlsport) = tlsport.filter(|p| !p.is_empty()) {
            obj.set_property("tls-port", tlsport.as_str());
        }
        session.connect();
    }

    // APIs accessed by vmmConsolePages

    pub fn console_is_open(&self) -> bool {
        match self.0.viewer_type {
            ViewerType::Vnc => self.vnc().is_some_and(|d| d.is_open()),
            ViewerType::Spice => self.spice_session().is_some(),
        }
    }

    pub fn console_grab_focus(&self) {
        if let Some(d) = self.display() {
            d.widget().grab_focus();
        }
    }

    pub fn console_get_framebuffer(&self) -> Option<FrameCopy> {
        match self.display()? {
            ViewerKind::Vnc(d) => d.get_framebuffer(),
            ViewerKind::Spice(d) => d.get_framebuffer(),
        }
    }

    /// `console_open` / `_open`: connects through a tunnel, a
    /// libvirt-provided fd, or straight to the host.
    pub async fn console_open(&self) -> Result<(), String> {
        if self.0.viewer_type == ViewerType::Vnc {
            // VNCViewer._init_display
            let display = VncDisplay::new();
            display.set_keep_aspect_ratio(true);
            self.set_display(ViewerKind::Vnc(display));
        }
        if let Some(err) = self.0.ginfo.bad_config() {
            return Err(err);
        }
        let fd = self.get_fd_for_open().await?;
        match (self.0.viewer_type, fd) {
            (ViewerType::Vnc, Some(fd)) => {
                if let Some(d) = self.vnc() {
                    d.open_fd(fd);
                }
            }
            (ViewerType::Vnc, None) => {
                let (host, port, _) = self.0.ginfo.get_conn_host();
                let port = port.unwrap_or_default();
                eprintln!("VNC connecting to host={host} port={port}");
                if let Some(d) = self.vnc() {
                    d.open_host(&host, &port);
                }
            }
            (ViewerType::Spice, Some(fd)) => {
                self.create_spice_session().open_fd(fd);
            }
            (ViewerType::Spice, None) => self.spice_open_host(),
        }
        Ok(())
    }

    pub fn console_set_password(&self, val: &str) {
        match self.0.viewer_type {
            ViewerType::Vnc => {
                if let Some(d) = self.vnc() {
                    d.set_credential(gvnc::CREDENTIAL_PASSWORD, val);
                }
            }
            ViewerType::Spice => {
                let Some(session) = self.spice_session() else {
                    return;
                };
                session.object().set_property("password", val);
                let this = self.clone();
                glib::spawn_future_local(async move {
                    match this.get_fd_for_open().await {
                        Ok(Some(fd)) => {
                            session.open_fd(fd);
                        }
                        Ok(None) => {
                            session.connect();
                        }
                        Err(e) => this.emit_disconnected(Some(e)),
                    }
                });
            }
        }
    }

    pub fn console_set_username(&self, val: &str) {
        // SPICE has no usernames.
        if let Some(d) = self.vnc() {
            d.set_credential(gvnc::CREDENTIAL_USERNAME, val);
        }
    }

    /// `console_send_keys`: X keysym names, e.g. `Control_L`.
    pub fn console_send_keys(&self, keys: &[&str]) {
        let Some(d) = self.display() else { return };
        let keyvals: Vec<u32> = keys
            .iter()
            .filter_map(|k| gtk::gdk::Key::from_name(*k))
            .map(|k| k.into_glib())
            .collect();
        match d {
            ViewerKind::Vnc(d) => d.send_keys(&keyvals),
            ViewerKind::Spice(d) => d.send_keys(&keyvals),
        }
    }

    pub fn console_get_grab_keys(&self) -> String {
        match self.display() {
            Some(ViewerKind::Vnc(d)) => d.get_grab_keys_string(),
            Some(ViewerKind::Spice(d)) => d.get_grab_keys_string(),
            None => String::new(),
        }
    }

    pub fn console_get_preferred_size(&self) -> (i32, i32) {
        match self.spice_display() {
            // When scaling and resizeguest are disabled, this usually
            // matches the VM resolution.
            Some(d) if !d.get_scaling_enabled() && !d.get_resize_guest() => d.preferred_size(),
            _ => self.0.desktop_resolution.get(),
        }
    }

    pub fn console_set_scaling(&self, val: bool) {
        match self.display() {
            Some(ViewerKind::Vnc(d)) => {
                d.set_scaling(val);
                self.sync_force_size();
            }
            Some(ViewerKind::Spice(d)) => d.set_scaling(val),
            None => {}
        }
    }

    pub fn console_set_resizeguest(&self, val: bool) {
        match self.display() {
            Some(ViewerKind::Vnc(d)) => {
                d.set_allow_resize(val);
                self.sync_force_size();
            }
            Some(ViewerKind::Spice(d)) => d.set_resize_guest(val),
            None => {}
        }
    }

    /// `_get_usb_widget`: a new device list each time.
    pub fn console_get_usb_widget(&self) -> Option<UsbDeviceWidget> {
        let session = {
            let st = self.0.spice.borrow();
            st.usbdev_manager.as_ref()?;
            st.session.clone()?
        };
        let widget = UsbDeviceWidget::new(&session, None);
        let weak = self.weak();
        widget.connect_connect_failed(move |err| {
            if let Some(inner) = weak.upgrade() {
                Viewer(inner).usbdev_redirect_error(err);
            }
        });
        Some(widget)
    }

    /// `_has_usb_redirection`.
    pub fn console_has_usb_redirection(&self) -> bool {
        let st = self.0.spice.borrow();
        st.session.is_some() && st.usbdev_manager.is_some()
    }
}
