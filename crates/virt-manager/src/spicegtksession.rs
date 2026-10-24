//! spice-gtk's `SpiceGtkSession` (`src/spice-gtk-session.c`) for GTK4:
//! the per-session state that needs a toolkit, which is clipboard
//! sharing with the guest agent, USB auto-redirection while a display
//! has the keyboard, and keeping the guest's lock keys in step with
//! the client's.
//!
//! The clipboard logic follows upstream's: the same agent types and
//! their targets, the same grab, request and release handshake with
//! the delayed release, and the same size limits and line-ending
//! fixups. GTK4's `GdkClipboard` is asynchronous throughout, so where
//! GTK3 ran a nested main loop to wait for the guest's data, the
//! guest's content here is a [`GuestClipboard`] content provider that
//! awaits it. File lists (spice-gtk's phodav-backed WebDAV sharing)
//! are left out, as in a spice-gtk built without phodav.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::spice::{self, ChannelKind, Session, UsbDeviceManager};

/// `atom2agent`: the targets each agent clipboard type is offered as.
/// The X11 atom names never show up in GTK4, which speaks MIME types
/// only, but are kept to match the incoming targets as upstream does.
const ATOM2AGENT: &[(&str, u32)] = &[
    ("UTF8_STRING", spice::CLIPBOARD_UTF8_TEXT),
    ("text/plain;charset=utf-8", spice::CLIPBOARD_UTF8_TEXT),
    ("STRING", spice::CLIPBOARD_UTF8_TEXT),
    ("TEXT", spice::CLIPBOARD_UTF8_TEXT),
    ("text/plain", spice::CLIPBOARD_UTF8_TEXT),
    ("image/png", spice::CLIPBOARD_IMAGE_PNG),
    ("image/bmp", spice::CLIPBOARD_IMAGE_BMP),
    ("image/x-bmp", spice::CLIPBOARD_IMAGE_BMP),
    ("image/x-MS-bmp", spice::CLIPBOARD_IMAGE_BMP),
    ("image/x-win-bitmap", spice::CLIPBOARD_IMAGE_BMP),
    ("image/tiff", spice::CLIPBOARD_IMAGE_TIFF),
    ("image/jpeg", spice::CLIPBOARD_IMAGE_JPG),
    ("text/uri-list", spice::CLIPBOARD_FILE_LIST),
];

/// `CLIPBOARD_RELEASE_DELAY`, in milliseconds.
const CLIPBOARD_RELEASE_DELAY: u64 = 500;

/// `GNOME_SESSION_INHIBIT_AUTOMOUNT`
const GNOME_SESSION_INHIBIT_AUTOMOUNT: u32 = 16;

fn atom2agent_index(name: &str) -> Option<usize> {
    ATOM2AGENT
        .iter()
        .position(|(xatom, _)| xatom.eq_ignore_ascii_case(name))
}

/// `spice_dos2unix`: CRLF to LF, dropping one trailing NUL.
fn dos2unix(data: &[u8]) -> Vec<u8> {
    let data = data.strip_suffix(&[0]).unwrap_or(data);
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\r' && data.get(i + 1) == Some(&b'\n') {
            out.push(b'\n');
            i += 2;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

/// `spice_unix2dos`: LF to CRLF, not doubling a CR already there.
fn unix2dos(data: &[u8]) -> Vec<u8> {
    let data = data.strip_suffix(&[0]).unwrap_or(data);
    let mut out = Vec::with_capacity(data.len() * 2);
    for &b in data {
        if b == b'\n' && out.last() != Some(&b'\r') {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

/// spice-gtk's `SpiceDesktopIntegration`: GNOME's session manager is
/// asked not to automount USB storage while it is auto-redirected.
struct DesktopIntegration {
    proxy: Option<gio::DBusProxy>,
    cookie: u32,
}

impl DesktopIntegration {
    fn new() -> DesktopIntegration {
        let proxy = gio::DBusProxy::for_bus_sync(
            gio::BusType::Session,
            gio::DBusProxyFlags::NONE,
            None,
            "org.gnome.SessionManager",
            "/org/gnome/SessionManager",
            "org.gnome.SessionManager",
            gio::Cancellable::NONE,
        )
        .map_err(|e| {
            glib::g_warning!(
                "virt-manager",
                "Could not create org.gnome.SessionManager dbus proxy: {e}"
            );
        })
        .ok()
        .filter(|p| p.name_owner().is_some());
        if proxy.is_none() {
            glib::g_warning!(
                "virt-manager",
                "Warning no automount-inhibiting implementation available"
            );
        }
        DesktopIntegration { proxy, cookie: 0 }
    }

    fn inhibit_automount(&mut self) {
        let Some(proxy) = &self.proxy else { return };
        if self.cookie != 0 {
            return;
        }
        let prgname = glib::prgname().map(|s| s.to_string()).unwrap_or_default();
        let reason = "Automounting has been inhibited for USB auto-redirecting";
        match proxy.call_sync(
            "Inhibit",
            Some(&(prgname, 0u32, reason, GNOME_SESSION_INHIBIT_AUTOMOUNT).to_variant()),
            gio::DBusCallFlags::NONE,
            -1,
            gio::Cancellable::NONE,
        ) {
            Ok(v) => self.cookie = v.get::<(u32,)>().map_or(0, |(c,)| c),
            Err(e) => glib::g_warning!(
                "virt-manager",
                "Error calling 'org.gnome.SessionManager.Inhibit': {e}"
            ),
        }
    }

    fn uninhibit_automount(&mut self) {
        let Some(proxy) = &self.proxy else { return };
        if self.cookie == 0 {
            return;
        }
        if let Err(e) = proxy.call_sync(
            "Uninhibit",
            Some(&(self.cookie,).to_variant()),
            gio::DBusCallFlags::NONE,
            -1,
            gio::Cancellable::NONE,
        ) {
            glib::g_warning!(
                "virt-manager",
                "Error calling 'org.gnome.SessionManager.Uninhibit': {e}"
            );
        }
        self.cookie = 0;
    }
}

struct State {
    auto_clipboard_enable: bool,
    main: Option<glib::Object>,
    main_handlers: Vec<glib::SignalHandlerId>,
    /// `clip_targets`: the guest's offer, as indexes into `ATOM2AGENT`.
    clip_targets: [Vec<usize>; spice::CLIPBOARD_LAST],
    clip_hasdata: [bool; spice::CLIPBOARD_LAST],
    clip_grabbed: [bool; spice::CLIPBOARD_LAST],
    clipboard_by_guest: [bool; spice::CLIPBOARD_LAST],
    clipboard_release_delay: [Option<glib::SourceId>; spice::CLIPBOARD_LAST],

    auto_usbredir_enable: bool,
    auto_usbredir_reqs: i32,
    desktop: Option<DesktopIntegration>,
    keyboard_has_focus: bool,
    mouse_has_pointer: bool,
}

struct Inner {
    session: glib::WeakRef<glib::Object>,
    /// `clipboard` and `clipboard_primary`, by selection.
    clipboards: [gdk::Clipboard; 2],
    handlers: RefCell<Vec<(glib::Object, glib::SignalHandlerId)>>,
    state: RefCell<State>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        for (obj, id) in self.handlers.take() {
            obj.disconnect(id);
        }
        let st = self.state.get_mut();
        if let Some(main) = st.main.take() {
            for id in st.main_handlers.drain(..) {
                main.disconnect(id);
            }
        }
        for id in st
            .clipboard_release_delay
            .iter_mut()
            .filter_map(Option::take)
        {
            id.remove();
        }
        if let Some(d) = st.desktop.as_mut() {
            d.uninhibit_automount();
        }
    }
}

/// `SpiceGtkSession`, 1:1 with a [`Session`] (see [`Self::get`]).
#[derive(Clone)]
pub struct SpiceGtkSession(Rc<Inner>);

const DATA_KEY: &str = "vmm-spice-gtk-session";

impl SpiceGtkSession {
    /// `spice_gtk_session_get`: created on first use and kept on the
    /// session object, so it lives exactly as long as the session.
    pub fn get(session: &Session) -> SpiceGtkSession {
        let obj = session.object();
        // SAFETY: only this function touches the key, always with
        // this type.
        if let Some(p) = unsafe { obj.data::<SpiceGtkSession>(DATA_KEY) } {
            return unsafe { p.as_ref() }.clone();
        }
        let this = Self::new(session);
        unsafe { obj.set_data(DATA_KEY, this.clone()) };
        this
    }

    fn new(session: &Session) -> SpiceGtkSession {
        let display = gdk::Display::default().expect("a display");
        let this = SpiceGtkSession(Rc::new(Inner {
            session: session.object().downgrade(),
            clipboards: [display.clipboard(), display.primary_clipboard()],
            handlers: RefCell::default(),
            state: RefCell::new(State {
                auto_clipboard_enable: true,
                main: None,
                main_handlers: vec![],
                clip_targets: Default::default(),
                clip_hasdata: [false; spice::CLIPBOARD_LAST],
                clip_grabbed: [false; spice::CLIPBOARD_LAST],
                clipboard_by_guest: [false; spice::CLIPBOARD_LAST],
                clipboard_release_delay: Default::default(),
                auto_usbredir_enable: false,
                auto_usbredir_reqs: 0,
                desktop: None,
                keyboard_has_focus: false,
                mouse_has_pointer: false,
            }),
        }));
        this.init(session, &display);
        this
    }

    fn weak(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    fn st(&self) -> std::cell::RefMut<'_, State> {
        self.0.state.borrow_mut()
    }

    fn session(&self) -> Option<Session> {
        self.0.session.upgrade().map(Session::from_object)
    }

    /// `spice_gtk_session_init` + `spice_gtk_session_constructed`.
    fn init(&self, session: &Session, display: &gdk::Display) {
        let mut handlers = vec![];
        for (selection, cb) in self.0.clipboards.iter().enumerate() {
            let weak = self.weak();
            let id = cb.connect_changed(move |_| {
                if let Some(inner) = weak.upgrade() {
                    SpiceGtkSession(inner).clipboard_owner_change(selection as u32);
                }
            });
            handlers.push((cb.clone().upcast::<glib::Object>(), id));
        }

        // `keymap_modifiers_changed`
        if let Some(kbd) = display.default_seat().and_then(|s| s.keyboard()) {
            for prop in ["caps-lock-state", "num-lock-state", "scroll-lock-state"] {
                let weak = self.weak();
                let id = kbd.connect_notify_local(Some(prop), move |_, _| {
                    let Some(inner) = weak.upgrade() else { return };
                    let this = SpiceGtkSession(inner);
                    // No need when focused: key events carry the state.
                    if !this.0.state.borrow().keyboard_has_focus {
                        this.sync_keyboard_modifiers();
                    }
                });
                handlers.push((kbd.clone().upcast::<glib::Object>(), id));
            }
        }

        let obj = session.object().clone();
        let weak = self.weak();
        let id = obj.connect_local("channel-new", false, move |a| {
            if let (Some(inner), Ok(ch)) = (weak.upgrade(), a[1].get::<glib::Object>()) {
                SpiceGtkSession(inner).channel_new(&ch);
            }
            None
        });
        handlers.push((obj.clone(), id));
        let weak = self.weak();
        let id = obj.connect_local("channel-destroy", false, move |a| {
            if let (Some(inner), Ok(ch)) = (weak.upgrade(), a[1].get::<glib::Object>()) {
                SpiceGtkSession(inner).channel_destroy(&ch);
            }
            None
        });
        handlers.push((obj, id));
        self.0.handlers.replace(handlers);

        for ch in session.channels() {
            self.channel_new(&ch);
        }
    }

    fn read_only(&self) -> bool {
        self.session().is_some_and(|s| s.read_only())
    }

    fn main(&self) -> Option<glib::Object> {
        self.0.state.borrow().main.clone()
    }

    /// `get_clipboard_from_selection`.
    fn clipboard(&self, selection: u32) -> Option<&gdk::Clipboard> {
        let cb = match selection {
            spice::CLIPBOARD_SELECTION_CLIPBOARD => Some(&self.0.clipboards[0]),
            spice::CLIPBOARD_SELECTION_PRIMARY => Some(&self.0.clipboards[1]),
            _ => None,
        };
        if cb.is_none() {
            glib::g_warning!("virt-manager", "Unhandled clipboard selection: {selection}");
        }
        cb
    }

    fn is_ours(cb: &gdk::Clipboard) -> bool {
        cb.content().is_some_and(|c| c.is::<GuestClipboard>())
    }

    // Properties

    /// `auto-clipboard`
    pub fn set_auto_clipboard(&self, enable: bool) {
        self.st().auto_clipboard_enable = enable;
    }

    /// `auto-usbredir`: toggling it while a display has the keyboard
    /// starts or stops auto-redirection right away.
    pub fn set_auto_usbredir(&self, enable: bool) {
        let (old, reqs) = {
            let st = self.0.state.borrow();
            (st.auto_usbredir_enable, st.auto_usbredir_reqs)
        };
        if old == enable {
            return;
        }
        if reqs > 0 && !enable {
            self.apply_auto_usbredir(false);
        }
        self.st().auto_usbredir_enable = enable;
        if reqs > 0 && enable {
            self.apply_auto_usbredir(true);
        }
    }

    // Clipboard: client to guest

    /// `clipboard_owner_change`.
    fn clipboard_owner_change(&self, selection: u32) {
        let Some(cb) = self.clipboard(selection).cloned() else {
            return;
        };
        let Some(main) = self.main() else { return };
        let sel = selection as usize;

        if cb.formats().mime_types().is_empty() {
            // The owner went away (or emptied it).
            let grabbed = std::mem::take(&mut self.st().clip_grabbed[sel]);
            if grabbed
                && spice::main_agent_test_capability(&main, spice::AGENT_CAP_CLIPBOARD_BY_DEMAND)
            {
                spice::main_clipboard_selection_release(&main, selection);
            }
            self.st().clip_hasdata[sel] = false;
            return;
        }

        // We set it ourselves, from a guest grab.
        if Self::is_ours(&cb) {
            return;
        }

        let auto = {
            let mut st = self.st();
            st.clipboard_by_guest[sel] = false;
            st.clip_hasdata[sel] = true;
            st.auto_clipboard_enable
        };
        if auto && !self.read_only() {
            self.clipboard_get_targets(selection, &cb);
        }
    }

    /// `clipboard_get_targets`: announce what the client clipboard
    /// holds to the agent.
    fn clipboard_get_targets(&self, selection: u32, cb: &gdk::Clipboard) {
        let Some(main) = self.main() else { return };
        let sel = selection as usize;
        let mut types: Vec<u32> = vec![];
        for mime in cb.formats().mime_types() {
            for &(xatom, vdagent) in ATOM2AGENT {
                if !xatom.eq_ignore_ascii_case(&mime) {
                    continue;
                }
                // File lists need the WebDAV channel, not built here.
                if vdagent == spice::CLIPBOARD_FILE_LIST {
                    break;
                }
                if !types.contains(&vdagent) {
                    types.push(vdagent);
                }
            }
        }
        if types.is_empty() {
            glib::g_debug!("virt-manager", "No clipboard targets will be sent");
            return;
        }

        self.st().clip_grabbed[sel] = true;
        if spice::main_agent_test_capability(&main, spice::AGENT_CAP_CLIPBOARD_BY_DEMAND) {
            spice::main_clipboard_selection_grab(&main, selection, &types);
        }
        // Sending a grab causes the agent to do an implicit release.
        self.st().clip_targets[sel].clear();
    }

    /// `check_clipboard_size_limits`.
    fn check_size_limits(main: &glib::Object, len: usize) -> bool {
        let max = main.property::<i32>("max-clipboard");
        if max != -1 && len > max as usize {
            glib::g_warning!(
                "virt-manager",
                "discarded clipboard of size {len} (max: {max})"
            );
            false
        } else if len == 0 {
            glib::g_debug!("virt-manager", "discarding empty clipboard");
            false
        } else {
            true
        }
    }

    /// `clipboard_request`: the guest wants the client's data.
    fn clipboard_request(&self, selection: u32, type_: u32) -> bool {
        let Some(cb) = self.clipboard(selection).cloned() else {
            return false;
        };
        let sel = selection as usize;
        {
            let st = self.0.state.borrow();
            if st.clipboard_by_guest[sel] || !st.clip_grabbed[sel] {
                return false;
            }
        }
        if self.read_only() {
            return false;
        }

        let weak = self.weak();
        if type_ == spice::CLIPBOARD_UTF8_TEXT {
            cb.read_text_async(gio::Cancellable::NONE, move |res| {
                if let Some(inner) = weak.upgrade() {
                    let text = res.ok().flatten();
                    SpiceGtkSession(inner).clipboard_received_text(selection, text.as_deref());
                }
            });
        } else if type_ == spice::CLIPBOARD_FILE_LIST {
            return false;
        } else {
            // Prefer a target the clipboard holds; GTK3 would convert.
            let formats = cb.formats();
            let candidates: Vec<&str> = ATOM2AGENT
                .iter()
                .filter(|(_, t)| *t == type_)
                .map(|(x, _)| *x)
                .collect();
            let Some(mime) = candidates
                .iter()
                .find(|m| formats.contain_mime_type(m))
                .or(candidates.first())
                .copied()
            else {
                return false;
            };
            glib::spawn_future_local(async move {
                let data = read_all(&cb, mime).await;
                if let Some(inner) = weak.upgrade() {
                    SpiceGtkSession(inner).clipboard_received(selection, mime, data);
                }
            });
        }
        true
    }

    /// `clipboard_received_text_cb`.
    fn clipboard_received_text(&self, selection: u32, text: Option<&str>) {
        let Some(main) = self.main() else { return };
        let data = text.and_then(|text| {
            if !Self::check_size_limits(&main, text.len()) {
                return None;
            }
            // GTK's text is always LF.
            let conv =
                if spice::main_agent_test_capability(&main, spice::AGENT_CAP_GUEST_LINEEND_CRLF) {
                    unix2dos(text.as_bytes())
                } else {
                    // Never send a trailing NUL to the agent.
                    text.trim_end_matches('\0').as_bytes().to_vec()
                };
            Self::check_size_limits(&main, conv.len()).then_some(conv)
        });
        spice::main_clipboard_selection_notify(
            &main,
            selection,
            spice::CLIPBOARD_UTF8_TEXT,
            data.as_deref(),
        );
    }

    /// `clipboard_received_cb`.
    fn clipboard_received(&self, selection: u32, mime: &str, data: Option<Vec<u8>>) {
        let Some(main) = self.main() else { return };
        let data = data.unwrap_or_default();
        if !Self::check_size_limits(&main, data.len()) {
            return;
        }
        let type_ = match atom2agent_index(mime) {
            Some(m) => ATOM2AGENT[m].1,
            None => {
                glib::g_warning!(
                    "virt-manager",
                    "clipboard_received for unsupported type: {mime}"
                );
                spice::CLIPBOARD_NONE
            }
        };
        spice::main_clipboard_selection_notify(&main, selection, type_, Some(&data));
    }

    // Clipboard: guest to client

    /// `clipboard_grab`: the guest has new clipboard data.
    fn clipboard_grab(&self, selection: u32, types: &[u32]) -> bool {
        self.clipboard_release_delay_remove(selection, false);
        let Some(cb) = self.clipboard(selection).cloned() else {
            return false;
        };
        let sel = selection as usize;

        let mut targets: Vec<usize> = vec![];
        for &t in types {
            let mut found = false;
            for (m, &(_, vdagent)) in ATOM2AGENT.iter().enumerate() {
                if vdagent == t && !targets.contains(&m) {
                    found = true;
                    targets.push(m);
                }
            }
            if !found {
                glib::g_warning!(
                    "virt-manager",
                    "clipboard: couldn't find a matching type for: {t}"
                );
            }
        }

        let auto = {
            let mut st = self.st();
            st.clip_targets[sel] = targets.clone();
            // Receiving a grab implies we've released our own grab.
            st.clip_grabbed[sel] = false;
            st.auto_clipboard_enable
        };
        if self.read_only() || !auto || targets.is_empty() {
            return true;
        }

        let provider = GuestClipboard::new(self, selection, &targets);
        if cb.set_content(Some(&provider)).is_err() {
            glib::g_warning!("virt-manager", "clipboard grab failed");
            return false;
        }
        let mut st = self.st();
        st.clipboard_by_guest[sel] = true;
        st.clip_hasdata[sel] = false;
        true
    }

    /// `clipboard_release`.
    fn clipboard_release(&self, selection: u32) {
        let Some(cb) = self.clipboard(selection).cloned() else {
            return;
        };
        let sel = selection as usize;
        {
            let mut st = self.st();
            st.clip_targets[sel].clear();
            if !st.clipboard_by_guest[sel] {
                return;
            }
        }
        if Self::is_ours(&cb) {
            let _ = cb.set_content(gdk::ContentProvider::NONE);
        }
        self.st().clipboard_by_guest[sel] = false;
    }

    fn clipboard_release_delay_remove(&self, selection: u32, release_if_delayed: bool) {
        let Some(id) = self
            .st()
            .clipboard_release_delay
            .get_mut(selection as usize)
            .and_then(Option::take)
        else {
            return;
        };
        if release_if_delayed {
            glib::g_debug!("virt-manager", "delayed clipboard release, sel:{selection}");
            self.clipboard_release(selection);
        }
        id.remove();
    }

    /// `clipboard_release_delay`: agents release between two grabs,
    /// which sets clipboard managers racing the guest for ownership,
    /// so the release waits half a second unless the agent promises
    /// not to (`VD_AGENT_CAP_CLIPBOARD_NO_RELEASE_ON_REGRAB`).
    fn clipboard_release_delay(&self, selection: u32) {
        if self.clipboard(selection).is_none() {
            return;
        }
        let Some(main) = self.main() else { return };
        self.clipboard_release_delay_remove(selection, true);

        if spice::main_agent_test_capability(&main, spice::AGENT_CAP_CLIPBOARD_NO_RELEASE_ON_REGRAB)
        {
            self.clipboard_release(selection);
            return;
        }

        let weak = self.weak();
        let id = glib::timeout_add_local_once(
            std::time::Duration::from_millis(CLIPBOARD_RELEASE_DELAY),
            move || {
                let Some(inner) = weak.upgrade() else { return };
                let this = SpiceGtkSession(inner);
                // The source is done: forget it rather than remove it.
                let pending = this.st().clipboard_release_delay[selection as usize]
                    .take()
                    .is_some();
                if pending {
                    glib::g_debug!("virt-manager", "delayed clipboard release, sel:{selection}");
                    this.clipboard_release(selection);
                }
            },
        );
        self.st().clipboard_release_delay[selection as usize] = Some(id);
    }

    fn release_pending(&self, selection: u32) -> bool {
        self.0.state.borrow().clipboard_release_delay[selection as usize].is_some()
    }

    // Channels

    /// `channel_new`.
    fn channel_new(&self, ch: &glib::Object) {
        match spice::channel_kind(ch) {
            ChannelKind::Main => {
                if let Some(old) = self.st().main.take() {
                    let ids = std::mem::take(&mut self.st().main_handlers);
                    for id in ids {
                        old.disconnect(id);
                    }
                }
                let mut ids = vec![];
                let weak = self.weak();
                ids.push(
                    ch.connect_local("main-clipboard-selection-grab", false, move |a| {
                        let selection = a[1].get::<u32>().unwrap_or(0);
                        let p = spice::value_pointer(&a[2]) as *const u32;
                        let n = a[3].get::<u32>().unwrap_or(0) as usize;
                        let types = if p.is_null() {
                            vec![]
                        } else {
                            // SAFETY: the signal's `ntypes` agent types.
                            unsafe { std::slice::from_raw_parts(p, n) }.to_vec()
                        };
                        let ok = weak.upgrade().is_some_and(|inner| {
                            SpiceGtkSession(inner).clipboard_grab(selection, &types)
                        });
                        Some(ok.to_value())
                    }),
                );
                let weak = self.weak();
                ids.push(
                    ch.connect_local("main-clipboard-selection-request", false, move |a| {
                        let selection = a[1].get::<u32>().unwrap_or(0);
                        let type_ = a[2].get::<u32>().unwrap_or(0);
                        let ok = weak.upgrade().is_some_and(|inner| {
                            SpiceGtkSession(inner).clipboard_request(selection, type_)
                        });
                        Some(ok.to_value())
                    }),
                );
                let weak = self.weak();
                ids.push(
                    ch.connect_local("main-clipboard-selection-release", false, move |a| {
                        let selection = a[1].get::<u32>().unwrap_or(0);
                        if let Some(inner) = weak.upgrade() {
                            SpiceGtkSession(inner).clipboard_release_delay(selection);
                        }
                        None
                    }),
                );
                let mut st = self.st();
                st.main = Some(ch.clone());
                st.main_handlers = ids;
            }
            ChannelKind::Inputs => {
                let weak = self.weak();
                let id = ch.connect_local("inputs-modifiers", false, move |a| {
                    if let (Some(inner), Ok(inputs)) = (weak.upgrade(), a[0].get::<glib::Object>())
                    {
                        SpiceGtkSession(inner).sync_keyboard_modifiers_for_channel(&inputs, false);
                    }
                    None
                });
                self.0.handlers.borrow_mut().push((ch.clone(), id));
                self.sync_keyboard_modifiers_for_channel(ch, true);
            }
            _ => {}
        }
    }

    /// `channel_destroy`.
    fn channel_destroy(&self, ch: &glib::Object) {
        if spice::channel_kind(ch) == ChannelKind::Inputs {
            let mut handlers = self.0.handlers.borrow_mut();
            handlers.retain(|(obj, _)| obj != ch);
            return;
        }
        if spice::channel_kind(ch) != ChannelKind::Main || self.main().as_ref() != Some(ch) {
            return;
        }
        let ids = {
            let mut st = self.st();
            st.main = None;
            std::mem::take(&mut st.main_handlers)
        };
        for id in ids {
            ch.disconnect(id);
        }
        for (sel, cb) in self.0.clipboards.iter().enumerate() {
            let by_guest = std::mem::take(&mut self.st().clipboard_by_guest[sel]);
            if by_guest && Self::is_ours(cb) {
                let _ = cb.set_content(gdk::ContentProvider::NONE);
            }
        }
        let mut st = self.st();
        for sel in 0..spice::CLIPBOARD_LAST {
            st.clipboard_by_guest[sel] = false;
            st.clip_grabbed[sel] = false;
            st.clip_targets[sel].clear();
        }
    }

    // Keyboard modifiers

    /// `get_keyboard_lock_modifiers`.
    fn keyboard_lock_modifiers() -> u32 {
        let Some(kbd) = gdk::Display::default()
            .and_then(|d| d.default_seat())
            .and_then(|s| s.keyboard())
        else {
            return 0;
        };
        let mut modifiers = 0;
        if kbd.is_caps_locked() {
            modifiers |= spice::KEY_LOCK_CAPS;
        }
        if kbd.is_num_locked() {
            modifiers |= spice::KEY_LOCK_NUM;
        }
        if kbd.is_scroll_locked() {
            modifiers |= spice::KEY_LOCK_SCROLL;
        }
        modifiers
    }

    /// `spice_gtk_session_sync_keyboard_modifiers_for_channel`.
    fn sync_keyboard_modifiers_for_channel(&self, inputs: &glib::Object, force: bool) {
        let guest = inputs.property::<i32>("key-modifiers") as u32;
        let client = Self::keyboard_lock_modifiers();
        if force || client != guest {
            spice::inputs_set_key_locks(inputs, client);
        }
    }

    /// `spice_gtk_session_sync_keyboard_modifiers`.
    pub fn sync_keyboard_modifiers(&self) {
        let Some(session) = self.session() else {
            return;
        };
        for ch in session.channels() {
            if spice::channel_kind(&ch) == ChannelKind::Inputs {
                self.sync_keyboard_modifiers_for_channel(&ch, true);
            }
        }
    }

    // USB auto-redirection and focus

    /// `spice_gtk_session_request_auto_usbredir`: each display asks
    /// while it holds the keyboard.
    pub fn request_auto_usbredir(&self, state: bool) {
        {
            let mut st = self.st();
            if state {
                st.auto_usbredir_reqs += 1;
                if st.auto_usbredir_reqs != 1 {
                    return;
                }
            } else {
                if st.auto_usbredir_reqs <= 0 {
                    return;
                }
                st.auto_usbredir_reqs -= 1;
                if st.auto_usbredir_reqs != 0 {
                    return;
                }
            }
            if !st.auto_usbredir_enable {
                return;
            }
        }
        self.apply_auto_usbredir(state);
    }

    fn apply_auto_usbredir(&self, state: bool) {
        let Some(manager) = self.session().and_then(|s| UsbDeviceManager::get(&s).ok()) else {
            return;
        };
        manager.object().set_property("auto-connect", state);
        let mut st = self.st();
        let desktop = st.desktop.get_or_insert_with(DesktopIntegration::new);
        if state {
            desktop.inhibit_automount();
        } else {
            desktop.uninhibit_automount();
        }
    }

    pub fn set_keyboard_has_focus(&self, focus: bool) {
        self.st().keyboard_has_focus = focus;
    }

    pub fn keyboard_has_focus(&self) -> bool {
        self.0.state.borrow().keyboard_has_focus
    }

    pub fn set_mouse_has_pointer(&self, has: bool) {
        self.st().mouse_has_pointer = has;
    }

    pub fn mouse_has_pointer(&self) -> bool {
        self.0.state.borrow().mouse_has_pointer
    }
}

/// Reads all of `mime` from `cb`; `None` on any failure.
async fn read_all(cb: &gdk::Clipboard, mime: &str) -> Option<Vec<u8>> {
    let (stream, _) = cb
        .read_future(&[mime], glib::Priority::DEFAULT)
        .await
        .map_err(|e| {
            glib::g_debug!(
                "virt-manager",
                "Retrieving the clipboard data has failed: {e}"
            )
        })
        .ok()?;
    let out = gio::MemoryOutputStream::new_resizable();
    out.splice_future(
        &stream,
        gio::OutputStreamSpliceFlags::CLOSE_SOURCE | gio::OutputStreamSpliceFlags::CLOSE_TARGET,
        glib::Priority::DEFAULT,
    )
    .await
    .ok()?;
    Some(out.steal_as_bytes().to_vec())
}

/// `clipboard_get`: waits for the guest's answer to a request.
async fn request_from_guest(
    session: Weak<Inner>,
    selection: u32,
    m: usize,
) -> Result<Vec<u8>, glib::Error> {
    let fail = |msg: &str| glib::Error::new(gio::IOErrorEnum::Failed, msg);
    let this = session
        .upgrade()
        .map(SpiceGtkSession)
        .ok_or_else(|| fail("session gone"))?;
    if this.release_pending(selection) {
        return Err(fail(
            "not requesting data from guest during delayed release",
        ));
    }
    let main = this.main().ok_or_else(|| fail("no main channel"))?;
    drop(this);

    let (tx, rx) = futures_channel::oneshot::channel::<Option<Vec<u8>>>();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let crlf = spice::main_agent_test_capability(&main, spice::AGENT_CAP_GUEST_LINEEND_CRLF);
    let is_text = ATOM2AGENT[m].1 == spice::CLIPBOARD_UTF8_TEXT;

    let got = {
        let tx = tx.clone();
        main.connect_local("main-clipboard-selection", false, move |a| {
            if a[1].get::<u32>().ok() != Some(selection) {
                return None;
            }
            let p = spice::value_pointer(&a[3]) as *const u8;
            let n = a[4].get::<u32>().unwrap_or(0) as usize;
            let data = if p.is_null() {
                vec![]
            } else {
                // SAFETY: the signal's `size` bytes of data.
                unsafe { std::slice::from_raw_parts(p, n) }.to_vec()
            };
            // On Windows GTK would convert line endings, not on unix.
            let data = if is_text && crlf {
                dos2unix(&data)
            } else {
                data
            };
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(Some(data));
            }
            None
        })
    };
    let agent = {
        let tx = tx.clone();
        main.connect_notify_local(Some("agent-connected"), move |_, _| {
            glib::g_warning!(
                "virt-manager",
                "agent status changed, cancel clipboard request"
            );
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(None);
            }
        })
    };

    spice::main_clipboard_selection_request(&main, selection, ATOM2AGENT[m].1);
    let result = if main.property::<bool>("agent-connected") {
        rx.await.ok().flatten()
    } else {
        glib::g_debug!(
            "virt-manager",
            "canceled clipboard_get, before running loop"
        );
        None
    };
    main.disconnect(got);
    main.disconnect(agent);
    result.ok_or_else(|| fail("the guest sent no clipboard data"))
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct GuestClipboard {
        pub(super) session: RefCell<Weak<Inner>>,
        pub selection: std::cell::Cell<u32>,
        /// Indexes into `ATOM2AGENT`.
        pub targets: RefCell<Vec<usize>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GuestClipboard {
        const NAME: &'static str = "VmmSpiceGuestClipboard";
        type Type = super::GuestClipboard;
        type ParentType = gdk::ContentProvider;
    }

    impl ObjectImpl for GuestClipboard {}

    impl ContentProviderImpl for GuestClipboard {
        fn formats(&self) -> gdk::ContentFormats {
            let mimes: Vec<&str> = self
                .targets
                .borrow()
                .iter()
                .map(|&m| ATOM2AGENT[m].0)
                .filter(|x| x.contains('/'))
                .collect();
            gdk::ContentFormats::new(&mimes)
        }

        fn write_mime_type_future(
            &self,
            mime_type: &str,
            stream: &gio::OutputStream,
            io_priority: glib::Priority,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), glib::Error>> + 'static>>
        {
            let m = self
                .targets
                .borrow()
                .iter()
                .copied()
                .find(|&m| ATOM2AGENT[m].0.eq_ignore_ascii_case(mime_type));
            let session = self.session.borrow().clone();
            let selection = self.selection.get();
            let stream = stream.clone();
            Box::pin(async move {
                let m = m.ok_or_else(|| {
                    glib::Error::new(gio::IOErrorEnum::NotSupported, "unsupported clipboard type")
                })?;
                let data = request_from_guest(session, selection, m).await?;
                stream
                    .write_all_future(data, io_priority)
                    .await
                    .map_err(|(_, e)| e)?;
                Ok(())
            })
        }
    }
}

glib::wrapper! {
    /// The guest's clipboard offer, set on the client clipboard by a
    /// guest grab; its data is fetched from the agent when pasted.
    pub struct GuestClipboard(ObjectSubclass<imp::GuestClipboard>)
        @extends gdk::ContentProvider;
}

impl GuestClipboard {
    fn new(session: &SpiceGtkSession, selection: u32, targets: &[usize]) -> GuestClipboard {
        let obj: GuestClipboard = glib::Object::new();
        let imp = obj.imp();
        imp.session.replace(session.weak());
        imp.selection.set(selection);
        imp.targets.replace(targets.to_vec());
        obj
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newlines() {
        assert_eq!(dos2unix(b"a\r\nb\r\n\0"), b"a\nb\n");
        assert_eq!(dos2unix(b"a\rb"), b"a\rb");
        assert_eq!(unix2dos(b"a\nb\r\nc\0"), b"a\r\nb\r\nc");
    }

    #[test]
    fn targets() {
        assert_eq!(atom2agent_index("TEXT/PLAIN"), Some(4));
        assert_eq!(ATOM2AGENT[atom2agent_index("image/x-ms-bmp").unwrap()].1, 3);
        assert_eq!(atom2agent_index("application/x-foo"), None);
    }
}
