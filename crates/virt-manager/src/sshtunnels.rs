//! Upstream's `virtManager/details/sshtunnels.py`: where a graphical
//! console lives (`ConnectionInfo`) and the `ssh … nc` tunnels used to
//! reach one listening on a remote host's loopback.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::Read;
use std::net::IpAddr;
use std::os::fd::{IntoRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::rc::Rc;

use virtinst_core::connection::Uri;
use virtinst_xml::{Document, Element};

/// The `<graphics>` bits `ConnectionInfo` reads, from the raw XML
/// since `DeviceGraphics` doesn't bind `<listen>` children yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphicsDev {
    pub idx: usize,
    pub gtype: String,
    pub port: Option<String>,
    pub tls_port: Option<String>,
    pub listen: Option<String>,
    pub socket: Option<String>,
    pub first_listen_type: Option<String>,
    pub first_listen_socket: Option<String>,
}

/// `is_int=True` ports: unset or 0 read as `None`, like Python's
/// `str(gdev.port) if gdev.port else None`.
fn int_port(val: Option<&str>) -> Option<String> {
    let n = val?.trim().parse::<i64>().ok()?;
    (n != 0).then(|| n.to_string())
}

impl GraphicsDev {
    pub fn from_element(doc: &Document, el: Element, idx: usize) -> GraphicsDev {
        let attr = |e: Element, n: &str| e.attribute(doc, n).map(str::to_string);
        let first_listen = el.find(doc, "listen");
        GraphicsDev {
            idx,
            gtype: attr(el, "type").unwrap_or_default(),
            port: int_port(el.attribute(doc, "port")),
            tls_port: int_port(el.attribute(doc, "tlsPort")),
            listen: attr(el, "listen"),
            socket: attr(el, "socket"),
            first_listen_type: first_listen.and_then(|l| attr(l, "type")),
            first_listen_socket: first_listen.and_then(|l| attr(l, "socket")),
        }
    }

    /// Every `<graphics>` of a domain, in order (`get_xml_idx`).
    pub fn all(doc: &Document, root: Element) -> Vec<GraphicsDev> {
        let Some(devices) = root.find(doc, "devices") else {
            return vec![];
        };
        devices
            .find_all(doc, "graphics")
            .into_iter()
            .enumerate()
            .map(|(i, el)| GraphicsDev::from_element(doc, el, i))
            .collect()
    }
}

/// `ConnectionInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionInfo {
    pub gtype: String,
    pub gidx: usize,
    pub gport: Option<String>,
    pub gsocket: Option<String>,
    pub gaddr: String,
    pub gtlsport: Option<String>,
    pub glistentype: Option<String>,
    pub transport: String,
    pub connuser: String,
    connhost: String,
    connport: String,
}

fn ip(host: &str) -> Option<IpAddr> {
    host.trim_matches(|c| c == '[' || c == ']').parse().ok()
}

impl ConnectionInfo {
    pub fn new(uri: &str, gdev: &GraphicsDev) -> ConnectionInfo {
        let uri = Uri::parse(uri);
        let mut connhost = if uri.hostname.is_empty() {
            "localhost".to_string()
        } else {
            uri.hostname.clone()
        };
        if connhost == "localhost" {
            connhost = "127.0.0.1".into();
        }
        ConnectionInfo {
            gtype: gdev.gtype.clone(),
            gidx: gdev.idx,
            gport: gdev.port.clone(),
            gsocket: gdev
                .first_listen_socket
                .clone()
                .or_else(|| gdev.socket.clone()),
            gaddr: gdev
                .listen
                .clone()
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| "127.0.0.1".into()),
            gtlsport: gdev.tls_port.clone(),
            glistentype: gdev.first_listen_type.clone(),
            transport: uri.effective_transport().to_string(),
            connuser: uri.username.clone(),
            connhost,
            connport: uri.port.clone(),
        }
    }

    fn is_listen_localhost(&self, host: Option<&str>) -> bool {
        ip(host.unwrap_or(&self.gaddr)).is_some_and(|a| a.is_loopback())
    }

    fn is_listen_any(&self) -> bool {
        ip(&self.gaddr).is_some_and(|a| a.is_unspecified())
    }

    fn is_listen_none(&self) -> bool {
        if self.glistentype.as_deref() == Some("none") {
            return true;
        }
        self.gsocket.is_none() && self.gport.is_none() && self.gtlsport.is_none()
    }

    pub fn need_tunnel(&self) -> bool {
        if !self.is_listen_localhost(None) {
            return false;
        }
        self.transport == "ssh"
    }

    pub fn bad_config(&self) -> Option<String> {
        if !self.transport.is_empty() && self.is_listen_none() {
            return Some(
                "Guest is on a remote host, but is only configured \
                 to allow local file descriptor connections."
                    .into(),
            );
        }
        if self.need_tunnel() && self.gtlsport.is_some() && self.gport.is_none() {
            return Some("Guest is configured for TLS only which does not work over SSH.".into());
        }
        if !self.need_tunnel()
            && !self.transport.is_empty()
            && self.is_listen_localhost(None)
            && !self.is_listen_localhost(Some(&self.connhost))
        {
            return Some(format!(
                "Guest is on a remote host with transport '{}' \
                 but is only configured to listen locally. \
                 To connect remotely you will need to change the guest's \
                 listen address.",
                self.transport
            ));
        }
        None
    }

    /// `get_conn_host`: host/port/tlsport for a direct connection.
    pub fn get_conn_host(&self) -> (String, Option<String>, Option<String>) {
        let host = if self.is_listen_any() {
            self.connhost.clone()
        } else {
            self.gaddr.clone()
        };
        (host, self.gport.clone(), self.gtlsport.clone())
    }

    /// `get_tunnel_host`: the physical host for the ssh tunnel.
    pub fn get_tunnel_host(&self) -> (String, String) {
        (self.connhost.clone(), self.connport.clone())
    }

    pub fn logstring(&self) -> String {
        format!(
            "proto={} trans={} connhost={} connuser={} connport={} gaddr={} gport={:?} \
             gtlsport={:?} gsocket={:?}",
            self.gtype,
            self.transport,
            self.connhost,
            self.connuser,
            self.connport,
            self.gaddr,
            self.gport,
            self.gtlsport,
            self.gsocket
        )
    }
}

/// `_make_ssh_command`.
pub fn make_ssh_command(ginfo: &ConnectionInfo) -> Option<Vec<String>> {
    if !ginfo.need_tunnel() {
        return None;
    }
    let (host, port) = ginfo.get_tunnel_host();
    let mut argv = vec![];
    if !port.is_empty() {
        argv.extend(["-p".to_string(), port]);
    }
    if !ginfo.connuser.is_empty() {
        argv.extend(["-l".to_string(), ginfo.connuser.clone()]);
    }
    argv.push(host);

    // Detects whether the remote `nc` needs `-q 0` to exit on EOF
    // (Debian/SUSE) or does so by default (Fedora).
    let nc_params = match &ginfo.gsocket {
        Some(sock) => format!("-U {sock}"),
        None => format!(
            "{} {}",
            ginfo.gaddr,
            ginfo.gport.as_deref().unwrap_or_default()
        ),
    };
    let nc_cmd = format!(
        "nc -q 2>&1 | grep \"requires an argument\" >/dev/null;\
         if [ $? -eq 0 ] ; then\
            CMD=\"nc -q 0 {nc_params}\";\
         else\
            CMD=\"nc {nc_params}\";\
         fi;\
         eval \"$CMD\";"
    );
    argv.push("sh -c".into());
    argv.push(format!("'{nc_cmd}'"));
    Some(argv)
}

/// `_Tunnel`.
#[derive(Default)]
struct Tunnel {
    child: Option<Child>,
    errfd: Option<UnixStream>,
    closed: bool,
}

impl Tunnel {
    fn open(&mut self, argv: &[String], sshfd: OwnedFd) -> std::io::Result<()> {
        if self.closed {
            return Ok(());
        }
        let (errread, errwrite) = UnixStream::pair()?;
        let child = Command::new("ssh")
            .args(argv)
            .stdin(Stdio::from(sshfd.try_clone()?))
            .stdout(Stdio::from(sshfd))
            .stderr(Stdio::from(OwnedFd::from(errwrite)))
            .spawn()?;
        errread.set_nonblocking(true)?;
        self.errfd = Some(errread);
        self.child = Some(child);
        Ok(())
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.errfd = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn get_err_output(&mut self) -> String {
        let mut out = String::new();
        if let Some(fd) = &mut self.errfd {
            let mut buf = [0u8; 1024];
            while let Ok(n) = fd.read(&mut buf) {
                if n == 0 {
                    break;
                }
                out += &String::from_utf8_lossy(&buf[..n]);
            }
        }
        out
    }
}

type TunnelJob = Box<dyn FnOnce()>;

thread_local! {
    /// `_TunnelScheduler`: tunnels open one at a time, the next one
    /// only once the previous viewer connected or failed, so
    /// concurrent ssh password prompts don't fight.
    static SCHEDULER: (Cell<bool>, RefCell<VecDeque<TunnelJob>>) = Default::default();
}

fn schedule(job: TunnelJob) {
    let run_now = SCHEDULER.with(|(locked, queue)| {
        if locked.get() {
            queue.borrow_mut().push_back(job);
            None
        } else {
            locked.set(true);
            Some(job)
        }
    });
    if let Some(job) = run_now {
        job();
    }
}

fn scheduler_unlock() {
    let next = SCHEDULER.with(|(locked, queue)| {
        let next = queue.borrow_mut().pop_front();
        locked.set(next.is_some());
        next
    });
    if let Some(job) = next {
        job();
    }
}

/// `SSHTunnels`.
pub struct SshTunnels {
    tunnels: Vec<Rc<RefCell<Tunnel>>>,
    sshcommand: Option<Vec<String>>,
    locked: Rc<Cell<bool>>,
}

impl SshTunnels {
    pub fn new(ginfo: &ConnectionInfo) -> SshTunnels {
        SshTunnels {
            tunnels: vec![],
            sshcommand: make_ssh_command(ginfo),
            locked: Rc::default(),
        }
    }

    /// A new tunnel; returns the viewer's end as a raw fd it owns.
    pub fn open_new(&mut self) -> std::io::Result<i32> {
        let tunnel = Rc::new(RefCell::new(Tunnel::default()));
        self.tunnels.push(tunnel.clone());
        let (viewerfd, sshfd) = UnixStream::pair()?;
        let argv = self.sshcommand.clone().unwrap_or_default();
        let locked = self.locked.clone();
        schedule(Box::new(move || {
            locked.set(true);
            if let Err(e) = tunnel.borrow_mut().open(&argv, OwnedFd::from(sshfd)) {
                eprintln!("Error opening SSH tunnel: {e}");
            }
        }));
        Ok(viewerfd.into_raw_fd())
    }

    pub fn close_all(&mut self) {
        for t in self.tunnels.drain(..) {
            t.borrow_mut().close();
        }
        self.unlock();
    }

    pub fn get_err_output(&self) -> String {
        let mut errs: Vec<String> = vec![];
        for t in &self.tunnels {
            let e = t.borrow_mut().get_err_output().trim().to_string();
            if !e.is_empty() && !errs.contains(&e) {
                errs.push(e);
            }
        }
        errs.join("\n")
    }

    pub fn unlock(&self) {
        if self.locked.replace(false) {
            scheduler_unlock();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gdev(port: Option<&str>, listen: Option<&str>) -> GraphicsDev {
        GraphicsDev {
            gtype: "vnc".into(),
            port: port.map(str::to_string),
            listen: listen.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn graphics_from_xml() {
        let xml = r#"<domain><devices>
            <graphics type="vnc" port="5900" autoport="yes" listen="127.0.0.1">
              <listen type="address" address="127.0.0.1"/>
            </graphics>
            <graphics type="spice" port="-1" tlsPort="0">
              <listen type="socket" socket="/run/spice.sock"/>
            </graphics>
        </devices></domain>"#;
        let doc = virtinst_xml::parse_libvirt_xml(xml).unwrap();
        let all = GraphicsDev::all(&doc, doc.root_element().unwrap());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].port.as_deref(), Some("5900"));
        assert_eq!(all[0].first_listen_type.as_deref(), Some("address"));
        assert_eq!(all[1].idx, 1);
        assert_eq!(all[1].port.as_deref(), Some("-1"));
        assert_eq!(all[1].tls_port, None);
        let ginfo = ConnectionInfo::new("qemu:///system", &all[1]);
        assert_eq!(ginfo.gsocket.as_deref(), Some("/run/spice.sock"));
    }

    #[test]
    fn local_connection() {
        let g = ConnectionInfo::new("qemu:///system", &gdev(Some("5900"), None));
        assert_eq!(g.transport, "");
        assert!(!g.need_tunnel());
        assert_eq!(g.bad_config(), None);
        assert_eq!(
            g.get_conn_host(),
            ("127.0.0.1".into(), Some("5900".into()), None)
        );
    }

    #[test]
    fn ssh_tunnel() {
        let g = ConnectionInfo::new(
            "qemu+ssh://root@example.com:2222/system",
            &gdev(Some("5901"), Some("127.0.0.1")),
        );
        assert!(g.need_tunnel());
        assert_eq!(g.bad_config(), None);
        let argv = make_ssh_command(&g).unwrap();
        assert_eq!(&argv[..5], &["-p", "2222", "-l", "root", "example.com"]);
        assert_eq!(argv[5], "sh -c");
        assert!(argv[6].contains("CMD=\"nc -q 0 127.0.0.1 5901\""));
    }

    #[test]
    fn remote_listen_any_uses_conn_host() {
        let g = ConnectionInfo::new(
            "qemu+tls://example.com/system",
            &gdev(Some("5900"), Some("0.0.0.0")),
        );
        assert!(!g.need_tunnel());
        assert_eq!(g.bad_config(), None);
        assert_eq!(g.get_conn_host().0, "example.com");
    }

    #[test]
    fn bad_configs() {
        // Remote but fd-only.
        let g = ConnectionInfo::new("qemu+ssh://h/system", &gdev(None, None));
        assert!(g.bad_config().unwrap().contains("local file descriptor"));
        // TLS only over SSH.
        let mut d = gdev(None, None);
        d.tls_port = Some("5902".into());
        let g = ConnectionInfo::new("qemu+ssh://h/system", &d);
        assert!(g.bad_config().unwrap().contains("TLS only"));
        // Remote non-ssh with a loopback listen.
        let g = ConnectionInfo::new(
            "qemu+tcp://example.com/system",
            &gdev(Some("5900"), Some("127.0.0.1")),
        );
        assert!(g.bad_config().unwrap().contains("transport 'tcp'"));
    }
}
