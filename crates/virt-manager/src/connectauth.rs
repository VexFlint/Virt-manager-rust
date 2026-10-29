//! Upstream's `virtManager/lib/connectauth.py` + `ui/connectauth.ui`:
//! the libvirt `openAuth` credentials callback and its "Authentication
//! required" dialog, the logind check behind the "Could not detect a
//! local session" hint, and `connect_error`, which words every failed
//! connection for the error dialog.
//!
//! libvirt calls the credentials callback on the connection's worker
//! thread (`gio::spawn_blocking`); the dialog runs on the GTK main loop
//! and the worker blocks until it's answered, like upstream's
//! `GLib.idle_add` + sleep loop in `creds_dialog`.
//!
//! Upstream v5.1.0 has no polkit `acquire_cred` step any more (polkit
//! agents and libvirt's own `pkttyagent` handle that) and its only
//! ssh-askpass handling is `SSH_ASKPASS_REQUIRE=force` (set in
//! `main.rs`) plus the askpass hint below.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::rc::Rc;

use futures_channel::oneshot;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use virt::connect::Connect;
use virt::error::ErrorNumber;
use virt::sys;
use virtinst_core::connection::Uri;

use crate::connection::ConnectError;

/// `virtinst.VirtinstConnection.open`'s `valid_auth_options`.
const VALID_AUTH_OPTIONS: [sys::virConnectCredentialType; 6] = [
    sys::VIR_CRED_AUTHNAME,
    sys::VIR_CRED_ECHOPROMPT,
    sys::VIR_CRED_REALM,
    sys::VIR_CRED_PASSPHRASE,
    sys::VIR_CRED_NOECHOPROMPT,
    sys::VIR_CRED_EXTERNAL,
];

/// `do_we_have_session`: whether logind knows a session for this
/// process. Blocking (system bus call): run it off the GTK thread.
pub fn do_we_have_session() -> bool {
    let pid = std::process::id();
    let res = (|| -> Result<glib::Variant, glib::Error> {
        let bus = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE)?;
        let manager = gio::DBusProxy::new_sync(
            &bus,
            gio::DBusProxyFlags::NONE,
            None,
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
            gio::Cancellable::NONE,
        )?;
        // This raises an error exception
        manager.call_sync(
            "GetSessionByPID",
            Some(&(pid,).to_variant()),
            gio::DBusCallFlags::NONE,
            -1,
            gio::Cancellable::NONE,
        )
    })();
    match res {
        Ok(_) => true,
        Err(e) => {
            eprintln!("Failure talking to logind: {e}");
            false
        }
    }
}

/// One libvirt `virConnectCredential`, as the dialog needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credential {
    pub credtype: c_int,
    pub prompt: String,
}

/// `credtype in [VIR_CRED_PASSPHRASE, VIR_CRED_NOECHOPROMPT]`: the
/// entry hides what's typed.
pub fn is_noecho(credtype: c_int) -> bool {
    credtype == sys::VIR_CRED_PASSPHRASE as c_int || credtype == sys::VIR_CRED_NOECHOPROMPT as c_int
}

/// The label text for a credential: its prompt plus `": "`. A
/// credential without a prompt fails the dialog, as upstream.
pub fn prompt_text(cred: &Credential) -> Result<String, String> {
    if cred.prompt.is_empty() {
        return Err(format!("No prompt for auth credtype={}", cred.credtype));
    }
    Ok(format!("{}: ", cred.prompt))
}

/// `run`'s copy back into `creds`: the first entry always, the second
/// only when its row is shown. Other credentials stay unanswered.
pub fn fill_results(ncreds: usize, entry1: String, entry2: Option<String>) -> Vec<Option<String>> {
    let mut results = vec![None; ncreds];
    if let Some(r) = results.get_mut(0) {
        *r = Some(entry1);
    }
    if let (Some(r), Some(text)) = (results.get_mut(1), entry2) {
        *r = Some(text);
    }
    results
}

/// `_vmmConnectAuth`: two label + entry rows, one per credential.
struct VmmConnectAuth {
    window: gtk::Window,
    entry1: gtk::Entry,
    entry2: gtk::Entry,
    response: RefCell<Option<oneshot::Sender<bool>>>,
}

impl VmmConnectAuth {
    fn new(creds: &[Credential]) -> Result<Rc<VmmConnectAuth>, String> {
        let window = gtk::Window::builder()
            .title("Authentication required")
            .modal(true)
            .resizable(false)
            .deletable(false)
            .build();

        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .build();
        let row_label = || {
            let label = gtk::Label::new(None);
            label.set_halign(gtk::Align::Start);
            label.set_xalign(0.0);
            label.set_hexpand(true);
            label.set_max_width_chars(40);
            // GTK3's sizing: as wide as the text (up to 40 chars) before
            // wrapping, not GTK4's narrower balanced natural width.
            label.set_natural_wrap_mode(gtk::NaturalWrapMode::None);
            label
        };
        let label1 = row_label();
        let label2 = row_label();
        let entry1 = gtk::Entry::new();
        entry1.set_valign(gtk::Align::Start);
        entry1.set_hexpand(true);
        let entry2 = gtk::Entry::new();
        entry2.set_valign(gtk::Align::Start);
        grid.attach(&label1, 0, 0, 1, 1);
        grid.attach(&entry1, 1, 0, 1, 1);
        grid.attach(&label2, 0, 1, 1, 1);
        grid.attach(&entry2, 1, 1, 1, 1);

        let cancel = gtk::Button::with_mnemonic("_Cancel");
        let ok = gtk::Button::with_mnemonic("_OK");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_homogeneous(true);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&ok);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox.set_margin_top(8);
        vbox.set_margin_bottom(8);
        vbox.set_margin_start(8);
        vbox.set_margin_end(8);
        vbox.append(&grid);
        vbox.append(&buttons);
        window.set_child(Some(&vbox));

        // _init_ui
        for w in [
            label1.upcast_ref::<gtk::Widget>(),
            entry1.upcast_ref(),
            label2.upcast_ref(),
            entry2.upcast_ref(),
        ] {
            w.set_visible(false);
        }
        let rows = [(&label1, &entry1), (&label2, &entry2)];
        for (idx, cred) in creds.iter().enumerate() {
            let Some((label, entry)) = rows.get(idx) else {
                return Err(format!("No dialog row for auth credential {}", idx + 1));
            };
            let prompt = prompt_text(cred)?;
            label.set_visible(true);
            entry.set_visible(true);
            label.set_text(&prompt);
            // Only a prompt past the 40 chars can wrap, as in GTK3; a
            // shorter one never shrinks to a wrapped minimum, which GTK4
            // sizes non-resizable windows around.
            label.set_wrap(prompt.chars().count() > 40);
            entry.set_visibility(!is_noecho(cred.credtype));
            entry.update_property(&[gtk::accessible::Property::Label(&format!("{prompt} entry"))]);
        }

        let this = Rc::new(VmmConnectAuth {
            window,
            entry1,
            entry2,
            response: RefCell::new(None),
        });

        let weak = Rc::downgrade(&this);
        ok.connect_clicked(move |_| {
            if let Some(t) = weak.upgrade() {
                t.respond(true);
            }
        });
        let weak = Rc::downgrade(&this);
        cancel.connect_clicked(move |_| {
            if let Some(t) = weak.upgrade() {
                t.respond(false);
            }
        });
        for entry in [&this.entry1, &this.entry2] {
            let weak = Rc::downgrade(&this);
            entry.connect_activate(move |src| {
                if let Some(t) = weak.upgrade() {
                    t.entry_activated(src);
                }
            });
        }
        // GtkDialog's Escape binding and window-manager close: a cancel.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&this);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key != gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            if let Some(t) = weak.upgrade() {
                t.respond(false);
            }
            glib::Propagation::Stop
        });
        this.window.add_controller(keys);
        let weak = Rc::downgrade(&this);
        this.window.connect_close_request(move |_| {
            if let Some(t) = weak.upgrade() {
                t.respond(false);
            }
            glib::Propagation::Proceed
        });
        Ok(this)
    }

    fn respond(&self, ok: bool) {
        if let Some(tx) = self.response.borrow_mut().take() {
            let _ = tx.send(ok);
        }
    }

    /// `_entry_cb`: Enter in the first entry moves to the second when
    /// it's shown; otherwise it clicks OK.
    fn entry_activated(&self, src: &gtk::Entry) {
        if src == &self.entry1 && WidgetExt::is_visible(&self.entry2) {
            self.entry2.grab_focus();
            return;
        }
        self.respond(true);
    }

    /// `run`: the answers for libvirt, or `None` for anything but OK.
    async fn run(&self, ncreds: usize) -> Option<Vec<Option<String>>> {
        let (tx, rx) = oneshot::channel();
        self.response.replace(Some(tx));
        self.window.present();
        let ok = rx.await.unwrap_or(false);
        let entry2 = WidgetExt::is_visible(&self.entry2).then(|| self.entry2.text().to_string());
        let results = fill_results(ncreds, self.entry1.text().to_string(), entry2);
        self.window.destroy();
        ok.then_some(results)
    }
}

/// `creds_dialog`: thread safe wrapper for the libvirt `openAuth`
/// callback. Called on the connection's worker thread; shows the dialog
/// on the GTK main loop and waits for it. `None` means "return -1".
pub fn creds_dialog(creds: Vec<Credential>) -> Option<Vec<Option<String>>> {
    let (tx, rx) = std::sync::mpsc::channel();
    // GLib.idle_add: always runs on the thread iterating the default
    // context. (`MainContext::invoke` would run it right here when the
    // GTK thread happens to be between iterations.)
    glib::idle_add_once(move || {
        glib::spawn_future_local(async move {
            let ret = match VmmConnectAuth::new(&creds) {
                Ok(dialog) => dialog.run(creds.len()).await,
                Err(e) => {
                    eprintln!("Error from creds dialog: {e}");
                    None
                }
            };
            let _ = tx.send(ret);
        });
    });
    rx.recv().ok().flatten()
}

unsafe fn cstr_or_empty(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

/// The C side of `creds_dialog`: libvirt's `virConnectAuthCallbackPtr`.
unsafe extern "C" fn auth_cb(
    cred: sys::virConnectCredentialPtr,
    ncred: c_uint,
    _cbdata: *mut c_void,
) -> c_int {
    extern "C" {
        fn strdup(s: *const c_char) -> *mut c_char;
    }
    let ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let n = ncred as usize;
        let creds: Vec<Credential> = (0..n)
            .map(|i| {
                let c = &*cred.add(i);
                Credential {
                    credtype: c.type_,
                    prompt: cstr_or_empty(c.prompt),
                }
            })
            .collect();
        let Some(results) = creds_dialog(creds) else {
            return -1;
        };
        for (i, res) in results.into_iter().enumerate().take(n) {
            let Some(res) = res else { continue };
            let Ok(res) = CString::new(res) else {
                return -1;
            };
            // libvirt free()s the result.
            let c = &mut *cred.add(i);
            c.resultlen = res.as_bytes().len() as c_uint;
            c.result = strdup(res.as_ptr());
        }
        0
    }));
    ret.unwrap_or(-1)
}

/// A failed `openAuth`: libvirt's error code, when it got that far, and
/// its message (Python's `str(libvirtError)`).
#[derive(Clone, Debug)]
pub struct OpenError {
    pub code: Option<ErrorNumber>,
    pub message: String,
}

/// `virtinst.VirtinstConnection.open`: `libvirt.openAuth(uri,
/// [valid_auth_options, creds_dialog, conn], 0)`. Blocking; call it
/// from a worker thread.
pub fn open_auth(uri: &str) -> Result<Connect, OpenError> {
    let c_uri = CString::new(uri).map_err(|e| OpenError {
        code: None,
        message: e.to_string(),
    })?;
    let mut credtype: Vec<c_int> = VALID_AUTH_OPTIONS.iter().map(|&c| c as c_int).collect();
    let mut auth = sys::virConnectAuth {
        credtype: credtype.as_mut_ptr(),
        ncredtype: credtype.len() as c_uint,
        cb: Some(auth_cb),
        cbdata: std::ptr::null_mut(),
    };
    // SAFETY: `c_uri` and `auth` (with `credtype`) outlive the call.
    let ptr = unsafe { sys::virConnectOpenAuth(c_uri.as_ptr(), &mut auth, 0) };
    if ptr.is_null() {
        let e = virt::error::Error::last_error();
        return Err(OpenError {
            code: Some(e.code()),
            message: e.message().to_string(),
        });
    }
    // SAFETY: a fresh connection whose reference we now own.
    Ok(unsafe { Connect::from_ptr(ptr) })
}

/// Upstream's `re.search(r"virt[a-z]*-sock", tb)`.
fn mentions_virt_socket(text: &str) -> bool {
    text.match_indices("virt").any(|(idx, _)| {
        let rest = text[idx + 4..].trim_start_matches(|c: char| c.is_ascii_lowercase());
        rest.starts_with("-sock")
    })
}

/// Upstream's `re.search(r"nc: .* -- 'U'", tb)`.
fn mentions_netcat_u(text: &str) -> bool {
    text.lines().any(|line| {
        line.match_indices("nc: ")
            .any(|(idx, _)| line[idx + 4..].contains(" -- 'U'"))
    })
}

/// `connect_error`: the error dialog's text for a failed open. `tb` is
/// what upstream searches for hints (its traceback); here that's the
/// libvirt message, which carries the same ssh/netcat/socket output.
pub fn connect_error(uri: &str, errmsg: &str, tb: &str, warnconsole: bool) -> ConnectError {
    let parsed = Uri::parse(uri);
    let errmsg = errmsg.trim_matches(|c| c == ' ' || c == '\n');
    let tb = tb.trim_matches(|c| c == ' ' || c == '\n');
    let mut hint = String::new();
    let mut show_errmsg = true;

    let is_remote = !parsed.hostname.is_empty();
    // get_uri_transport: libvirt defaults to tls with a hostname.
    let transport = if is_remote && parsed.transport.is_empty() {
        "tls"
    } else {
        parsed.transport.as_str()
    };
    let is_xen = parsed.scheme.starts_with("xen") || parsed.scheme.starts_with("libxl");

    if is_remote {
        if mentions_netcat_u(tb) {
            hint.push_str(
                "The remote host requires a version of netcat/nc which supports the -U option.",
            );
            show_errmsg = false;
        } else if transport == "ssh" && tb.contains("askpass") {
            hint.push_str(
                "Configure SSH key access for the remote host, \
                 or install an SSH askpass package locally.",
            );
            show_errmsg = false;
        } else {
            hint.push_str(
                "Verify that an appropriate libvirt daemon is running on the remote host.",
            );
        }
    } else if is_xen {
        hint.push_str(
            "Verify that:\n \
             - A Xen host kernel was booted\n \
             - The Xen service has been started",
        );
    } else if warnconsole {
        hint.push_str(
            "Could not detect a local session: if you are \
             running virt-manager over ssh -X or VNC, you \
             may not be able to connect to libvirt as a \
             regular user. Try running as root.",
        );
        show_errmsg = false;
    } else if mentions_virt_socket(tb) {
        hint.push_str("Verify that an appropriate libvirt daemon is running.");
        show_errmsg = false;
    }

    let mut msg = format!("Unable to connect to libvirt {uri}.");
    if show_errmsg {
        msg.push_str(&format!("\n\n{errmsg}"));
    }
    if !hint.is_empty() {
        msg.push_str(&format!("\n\n{hint}"));
    }
    let msg = msg.trim_matches('\n').to_string();
    let details = format!("{msg}\n\nLibvirt URI is: {uri}\n\n{tb}");
    ConnectError {
        msg,
        details,
        title: "Virtual Machine Manager Connection Failure".into(),
    }
}

/// `vmmConnection._do_open`'s error half: a cancelled auth is no error
/// (`None`); a polkit "not authorized" on a local URI checks for a
/// logind session first, so the hint can say why. Blocking.
pub fn open_error(uri: &str, err: &OpenError) -> Option<ConnectError> {
    if err.code == Some(ErrorNumber::AuthCancelled) {
        eprintln!("User cancelled auth, not raising any error.");
        return None;
    }
    let mut warnconsole = false;
    if err.code == Some(ErrorNumber::AuthFailed)
        && err.message.to_lowercase().contains("not authorized")
        && Uri::parse(uri).hostname.is_empty()
    {
        warnconsole = !do_we_have_session();
    }
    Some(connect_error(uri, &err.message, &err.message, warnconsole))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(credtype: sys::virConnectCredentialType, prompt: &str) -> Credential {
        Credential {
            credtype: credtype as c_int,
            prompt: prompt.into(),
        }
    }

    #[test]
    fn credential_rows_follow_upstream() {
        assert!(is_noecho(sys::VIR_CRED_PASSPHRASE as c_int));
        assert!(is_noecho(sys::VIR_CRED_NOECHOPROMPT as c_int));
        for t in [
            sys::VIR_CRED_AUTHNAME,
            sys::VIR_CRED_ECHOPROMPT,
            sys::VIR_CRED_REALM,
            sys::VIR_CRED_EXTERNAL,
        ] {
            assert!(!is_noecho(t as c_int));
        }
        assert_eq!(
            prompt_text(&cred(sys::VIR_CRED_AUTHNAME, "Username")).unwrap(),
            "Username: "
        );
        assert_eq!(
            prompt_text(&cred(sys::VIR_CRED_PASSPHRASE, "")).unwrap_err(),
            "No prompt for auth credtype=5"
        );
    }

    #[test]
    fn results_fill_the_first_two_credentials() {
        assert_eq!(
            fill_results(1, "foo".into(), None),
            vec![Some("foo".into())]
        );
        assert_eq!(
            fill_results(2, "foo".into(), Some("bar".into())),
            vec![Some("foo".into()), Some("bar".into())]
        );
        assert_eq!(
            fill_results(2, "foo".into(), None),
            vec![Some("foo".into()), None]
        );
        assert!(fill_results(0, "foo".into(), None).is_empty());
    }

    #[test]
    fn auth_options_match_virtinst() {
        assert_eq!(
            VALID_AUTH_OPTIONS,
            [
                sys::VIR_CRED_AUTHNAME,
                sys::VIR_CRED_ECHOPROMPT,
                sys::VIR_CRED_REALM,
                sys::VIR_CRED_PASSPHRASE,
                sys::VIR_CRED_NOECHOPROMPT,
                sys::VIR_CRED_EXTERNAL,
            ]
        );
    }

    #[test]
    fn connect_error_shapes() {
        let local = connect_error("qemu:///system", "  boom \n", "boom", false);
        assert_eq!(
            local.msg,
            "Unable to connect to libvirt qemu:///system.\n\nboom"
        );
        assert!(local
            .details
            .contains("Libvirt URI is: qemu:///system\n\nboom"));
        assert_eq!(local.title, "Virtual Machine Manager Connection Failure");
        let remote = connect_error("qemu+ssh://h/system", "x", "x", false);
        assert!(remote.msg.ends_with("running on the remote host."));
        assert!(remote.msg.contains("\n\nx\n\n"));
        let sock = "Failed to connect socket to '/run/libvirt/virtqemud-sock'";
        let sock = connect_error("qemu:///system", sock, sock, false);
        assert_eq!(
            sock.msg,
            "Unable to connect to libvirt qemu:///system.\n\nVerify that an appropriate libvirt daemon is running."
        );
    }

    #[test]
    fn connect_error_hints() {
        let nc = "nc: invalid option -- 'U'";
        let e = connect_error("qemu+ssh://h/system", nc, nc, false);
        assert_eq!(
            e.msg,
            "Unable to connect to libvirt qemu+ssh://h/system.\n\n\
             The remote host requires a version of netcat/nc which supports the -U option."
        );
        let ask = "ssh_askpass: exec(/usr/bin/ssh-askpass): No such file";
        let e = connect_error("qemu+ssh://h/system", ask, ask, false);
        assert!(e
            .msg
            .ends_with("or install an SSH askpass package locally."));
        assert!(!e.msg.contains("ssh_askpass:"));
        // askpass only matters over ssh; tls is the hostname default.
        let e = connect_error("qemu://h/system", ask, ask, false);
        assert!(e.msg.ends_with("running on the remote host."));
        let e = connect_error("xen:///", "x", "x", false);
        assert!(e.msg.ends_with(
            "Verify that:\n - A Xen host kernel was booted\n - The Xen service has been started"
        ));
        let e = connect_error("qemu:///system", "not authorized", "not authorized", true);
        assert_eq!(
            e.msg,
            "Unable to connect to libvirt qemu:///system.\n\n\
             Could not detect a local session: if you are running virt-manager over \
             ssh -X or VNC, you may not be able to connect to libvirt as a regular user. \
             Try running as root."
        );
    }

    #[test]
    fn cancelled_auth_is_not_an_error() {
        let cancelled = OpenError {
            code: Some(ErrorNumber::AuthCancelled),
            message: "cancelled".into(),
        };
        assert!(open_error("qemu+ssh://h/system", &cancelled).is_none());
        let failed = OpenError {
            code: Some(ErrorNumber::AuthFailed),
            message: "authentication failed".into(),
        };
        let e = open_error("test:///default", &failed).unwrap();
        assert!(e.msg.ends_with("authentication failed"));
    }

    #[test]
    fn regex_ports() {
        assert!(mentions_netcat_u("nc: invalid option -- 'U'"));
        assert!(mentions_netcat_u("x\nnc:  -- 'U'"));
        assert!(!mentions_netcat_u("nc: invalid option\n -- 'U'"));
        assert!(mentions_virt_socket("/run/libvirt/libvirt-sock"));
        assert!(mentions_virt_socket("virtqemud-sock"));
        assert!(!mentions_virt_socket("virt 1-sock"));
    }
}
