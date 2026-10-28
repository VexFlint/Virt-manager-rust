//! `install/urlfetcher.py`: fetching and extracting files from install
//! media, a local tree, an ISO (read with `xorriso`) or a network URL.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};

use crate::progress::Meter;

/// Python's `os.path.join(a, b)`: `b` wins when it is absolute.
pub fn path_join(a: &str, b: &str) -> String {
    if b.starts_with('/') || a.is_empty() {
        return b.to_string();
    }
    if a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// Python's `os.path.basename`.
pub fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `tempfile.NamedTemporaryFile(prefix, suffix, dir, delete=False)`: a
/// new file named `{dir}/{prefix}XXXXXXXX{suffix}`.
pub fn named_tempfile(dir: &str, prefix: &str, suffix: &str) -> Result<(File, String), String> {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    for _ in 0..100 {
        let bytes = uuid::Uuid::new_v4().into_bytes();
        let name: String = bytes[..8]
            .iter()
            .map(|b| CHARS[*b as usize % CHARS.len()] as char)
            .collect();
        let path = path_join(dir, &format!("{prefix}{name}{suffix}"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => return Ok((f, path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(crate::diskbackend::oserror_str(&e, Some(&path))),
        }
    }
    Err(format!("No usable temporary file name found in {dir}"))
}

/// `tempfile.mkdtemp(dir=dir)`.
pub fn mkdtemp(dir: &str) -> Result<String, String> {
    use std::os::unix::fs::DirBuilderExt;
    for _ in 0..100 {
        let rand = uuid::Uuid::new_v4().simple().to_string();
        let path = path_join(dir, &format!("tmp{}", &rand[..8]));
        match std::fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(crate::diskbackend::oserror_str(&e, Some(&path))),
        }
    }
    Err(format!("No usable temporary directory name found in {dir}"))
}

/// `_XorrisoReader`.
struct XorrisoReader {
    location: String,
    cache_file_list: Vec<String>,
}

impl XorrisoReader {
    fn new(location: &str) -> Result<XorrisoReader, String> {
        let delim = "VIRTINST_BEGINLIST";
        let cmd = ["-indev", location, "-print", delim, "-find"];
        log::debug!("Generating iso filelist: {:?}", cmd);
        let output = Command::new("xorriso")
            .args(cmd)
            .stderr(Stdio::null())
            .output()
            .map_err(|e| crate::diskbackend::oserror_str(&e, Some("xorriso")))?;
        if !output.status.success() {
            return Err(format!(
                "Command '['xorriso', '-indev', '{location}', '-print', '{delim}', '-find']' \
                 returned non-zero exit status {}.",
                output.status.code().unwrap_or(-1)
            ));
        }
        let out = String::from_utf8_lossy(&output.stdout);
        let list = out
            .split_once(delim)
            .map(|(_, rest)| rest.trim().lines().map(str::to_string).collect())
            .unwrap_or_default();
        Ok(XorrisoReader {
            location: location.into(),
            cache_file_list: list,
        })
    }

    fn grab_file(&self, url: &str, scratchdir: &str) -> Result<Vec<u8>, String> {
        let (_, tmp) = named_tempfile(scratchdir, "virtinst-iso", &format!("-{}", basename(url)))?;
        let cmd = [
            "-osirrox",
            "on",
            "-indev",
            &self.location,
            "-extract",
            url,
            &tmp,
        ];
        log::debug!("Extracting iso file: {:?}", cmd);
        let res = Command::new("xorriso")
            .args(cmd)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let data = std::fs::read(&tmp);
        let _ = std::fs::remove_file(&tmp);
        match res {
            Ok(s) if s.success() => {}
            Ok(s) => {
                return Err(format!(
                    "Command 'xorriso' returned non-zero exit status {}.",
                    s.code().unwrap_or(-1)
                ))
            }
            Err(e) => return Err(crate::diskbackend::oserror_str(&e, Some("xorriso"))),
        }
        data.map_err(|e| crate::diskbackend::oserror_str(&e, Some(&tmp)))
    }

    fn has_file(&self, url: &str) -> bool {
        let want = format!("'.{url}'");
        self.cache_file_list.iter().any(|l| l.contains(&want))
    }
}

/// Python's `urllib.parse.unquote`.
fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The parts of an `ftp://user:pass@host:port/path` URL that ftplib
/// needs.
struct FtpUrl {
    username: String,
    password: String,
    host: String,
    port: u16,
}

fn parse_ftp_url(url: &str) -> FtpUrl {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let netloc = rest.split('/').next().unwrap_or("");
    let (userinfo, hostport) = match netloc.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, netloc),
    };
    let (username, password) = match userinfo {
        Some(u) => match u.split_once(':') {
            Some((a, b)) => (unquote(a), unquote(b)),
            None => (unquote(u), String::new()),
        },
        None => (String::new(), String::new()),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || hostport.starts_with('[') => {
            (h.to_string(), p.parse().unwrap_or(21))
        }
        _ => (hostport.to_string(), 21),
    };
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    FtpUrl {
        username,
        password,
        host,
        port,
    }
}

/// A tiny `ftplib.FTP`: control connection plus passive transfers.
struct Ftp {
    ctrl: BufReader<TcpStream>,
}

impl Ftp {
    /// One (possibly multiline) reply: (code, full text).
    fn getresp(&mut self) -> Result<(u32, String), String> {
        let mut line = String::new();
        self.ctrl.read_line(&mut line).map_err(|e| e.to_string())?;
        let code: u32 = line
            .get(..3)
            .and_then(|c| c.parse().ok())
            .ok_or_else(|| line.trim_end().to_string())?;
        let mut text = line.clone();
        if line.as_bytes().get(3) == Some(&b'-') {
            let end = format!("{code} ");
            loop {
                let mut l = String::new();
                if self.ctrl.read_line(&mut l).map_err(|e| e.to_string())? == 0 {
                    break;
                }
                text += &l;
                if l.starts_with(&end) {
                    break;
                }
            }
        }
        let text = text.trim_end().to_string();
        if code >= 400 {
            return Err(text);
        }
        Ok((code, text))
    }

    fn sendcmd(&mut self, cmd: &str) -> Result<(u32, String), String> {
        let stream = self.ctrl.get_mut();
        stream
            .write_all(format!("{cmd}\r\n").as_bytes())
            .map_err(|e| e.to_string())?;
        self.getresp()
    }

    fn connect(url: &FtpUrl) -> Result<Ftp, String> {
        let stream =
            TcpStream::connect((url.host.as_str(), url.port)).map_err(|e| e.to_string())?;
        let mut ftp = Ftp {
            ctrl: BufReader::new(stream),
        };
        ftp.getresp()?;
        // ftplib's login defaults
        let user = if url.username.is_empty() {
            "anonymous"
        } else {
            &url.username
        };
        let passwd = if url.password.is_empty() && user == "anonymous" {
            "anonymous@"
        } else {
            &url.password
        };
        let (code, _) = ftp.sendcmd(&format!("USER {user}"))?;
        if code == 331 {
            ftp.sendcmd(&format!("PASS {passwd}"))?;
        }
        Ok(ftp)
    }

    fn size(&mut self, path: &str) -> Result<u64, String> {
        let (_, text) = self.sendcmd(&format!("SIZE {path}"))?;
        text.get(4..)
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| text.clone())
    }

    fn retr(&mut self, path: &str) -> Result<Vec<u8>, String> {
        let (_, text) = self.sendcmd("PASV")?;
        let nums: Vec<u16> = text
            .split(['(', ')'])
            .nth(1)
            .unwrap_or("")
            .split(',')
            .filter_map(|n| n.trim().parse().ok())
            .collect();
        if nums.len() != 6 {
            return Err(text);
        }
        let host = format!("{}.{}.{}.{}", nums[0], nums[1], nums[2], nums[3]);
        let port = nums[4] * 256 + nums[5];
        let mut data = TcpStream::connect((host.as_str(), port)).map_err(|e| e.to_string())?;
        self.sendcmd(&format!("RETR {path}"))?;
        let mut buf = vec![];
        data.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        drop(data);
        self.getresp()?;
        Ok(buf)
    }
}

/// Which `_URLFetcher` subclass a fetcher is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Http,
    Ftp,
    Local,
    Iso,
    Direct,
}

/// `_URLFetcher` and its subclasses.
pub struct Fetcher {
    pub location: String,
    pub scratchdir: String,
    kind: Kind,
    isoreader: Option<XorrisoReader>,
    http: Option<ureq::Agent>,
    ftp: Option<Ftp>,
}

const BLOCK_SIZE: usize = 16384;

/// Python's `urllib.parse.urlparse(url).path`, for the local fetcher.
fn url_path(url: &str) -> &str {
    match url.split_once("://") {
        Some((_, rest)) => rest.find('/').map_or("", |i| &rest[i..]),
        None => url,
    }
}

/// `fetcherForURI`.
pub fn fetcher_for_uri(uri: &str, scratchdir: &str, direct: bool) -> Result<Fetcher, String> {
    let kind = if uri.starts_with("http://") || uri.starts_with("https://") {
        Kind::Http
    } else if uri.starts_with("ftp://") {
        Kind::Ftp
    } else if direct || std::path::Path::new(uri).is_dir() {
        // Pointing to a local tree
        Kind::Local
    } else {
        // Pointing to a path (e.g. iso), or a block device (e.g. /dev/cdrom)
        Kind::Iso
    };
    Fetcher::new(uri, scratchdir, kind)
}

/// `DirectFetcher`: each file is its own full URL.
pub fn direct_fetcher(scratchdir: &str) -> Fetcher {
    Fetcher {
        location: String::new(),
        scratchdir: scratchdir.into(),
        kind: Kind::Direct,
        isoreader: None,
        http: None,
        ftp: None,
    }
}

impl Fetcher {
    fn new(location: &str, scratchdir: &str, kind: Kind) -> Result<Fetcher, String> {
        log::debug!("Using scratchdir={scratchdir}");
        let mut f = direct_fetcher(scratchdir);
        f.location = location.into();
        f.kind = kind;
        f.prepare()?;
        Ok(f)
    }

    /// `_prepare`.
    fn prepare(&mut self) -> Result<(), String> {
        match self.kind {
            Kind::Http => {
                self.http = Some(ureq::AgentBuilder::new().try_proxy_from_env(true).build());
            }
            Kind::Ftp => {
                let url = parse_ftp_url(&self.location);
                let res = Ftp::connect(&url).and_then(|mut ftp| {
                    // Force binary mode
                    ftp.sendcmd("TYPE I")?;
                    Ok(ftp)
                });
                match res {
                    Ok(ftp) => self.ftp = Some(ftp),
                    Err(e) => {
                        return Err(format!("Opening URL {} failed: {e}", self.location));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// `is_iso`: a fetcher for local CDROM/ISO.
    pub fn is_iso(&self) -> bool {
        self.kind == Kind::Iso
    }

    /// `_make_full_url`.
    fn make_full_url(&self, filename: &str) -> String {
        match self.kind {
            Kind::Iso => path_join("/", filename),
            Kind::Direct => filename.into(),
            _ if filename.is_empty() => self.location.clone(),
            _ => path_join(&self.location, filename),
        }
    }

    fn isoreader(&mut self) -> Result<&XorrisoReader, String> {
        if self.isoreader.is_none() {
            self.isoreader = Some(XorrisoReader::new(&self.location)?);
        }
        Ok(self.isoreader.as_ref().expect("just set"))
    }

    /// `can_access`: does the location URL seem valid.
    pub fn can_access(&mut self) -> bool {
        match self.kind {
            Kind::Http => self.has_file(""),
            _ => true,
        }
    }

    fn has_file_url(&mut self, url: &str) -> bool {
        match self.kind {
            Kind::Local => std::path::Path::new(url_path(url)).exists(),
            Kind::Iso => match self.isoreader() {
                Ok(r) => r.has_file(url),
                Err(e) => {
                    log::debug!("{e}");
                    false
                }
            },
            Kind::Direct => true,
            Kind::Http => {
                // We just do a HEAD request to see if the file exists
                let agent = self.http.as_ref().expect("prepared");
                match agent.head(url).call() {
                    Ok(_) => true,
                    Err(e) => {
                        log::debug!("HTTP hasFile request failed: {e}");
                        false
                    }
                }
            }
            Kind::Ftp => {
                let path = url_path(url).to_string();
                let ftp = self.ftp.as_mut().expect("prepared");
                // If it's a file, else if it's a dir
                let res = ftp
                    .size(&path)
                    .map(|_| ())
                    .or_else(|_| ftp.sendcmd(&format!("CWD {path}")).map(|_| ()));
                match res {
                    Ok(()) => true,
                    Err(e) => {
                        log::debug!("FTP hasFile: couldn't access {url}: {e}");
                        false
                    }
                }
            }
        }
    }

    /// `hasFile`: does `self.location` have `filename`.
    pub fn has_file(&mut self, filename: &str) -> bool {
        let url = self.make_full_url(filename);
        let ret = self.has_file_url(&url);
        log::debug!(
            "hasFile({url}) returning {}",
            if ret { "True" } else { "False" }
        );
        ret
    }

    /// `_grabber`: the reader and size for `url`.
    fn grabber(&mut self, url: &str) -> Result<(Box<dyn Read>, Option<u64>), String> {
        match self.kind {
            Kind::Local => {
                let path = url_path(url);
                let f = File::open(path)
                    .map_err(|e| crate::diskbackend::oserror_str(&e, Some(path)))?;
                let size = f.metadata().map(|m| m.len()).ok();
                Ok((Box::new(f), size))
            }
            Kind::Iso => {
                if !self.has_file_url(url) {
                    return Err(format!("iso doesn't have file={url}"));
                }
                let scratchdir = self.scratchdir.clone();
                let data = self.isoreader()?.grab_file(url, &scratchdir)?;
                let len = data.len() as u64;
                Ok((Box::new(std::io::Cursor::new(data)), Some(len)))
            }
            Kind::Direct => Err("DirectFetcher shouldn't be used for file access.".into()),
            Kind::Http => {
                let agent = self.http.as_ref().expect("prepared");
                let response = agent.get(url).call().map_err(|e| e.to_string())?;
                let size = response
                    .header("content-length")
                    .and_then(|s| s.parse().ok());
                Ok((response.into_reader(), size))
            }
            Kind::Ftp => {
                let path = url_path(url).to_string();
                let ftp = self.ftp.as_mut().expect("prepared");
                let size = ftp.size(&path).ok();
                let data = ftp.retr(&path)?;
                Ok((Box::new(std::io::Cursor::new(data)), size))
            }
        }
    }

    /// `_grabURL`: download `filename` (or `fullurl`) into `out`.
    fn grab_url(
        &mut self,
        filename: &str,
        out: &mut dyn Write,
        fullurl: Option<&str>,
        meter: &mut dyn Meter,
    ) -> Result<(), String> {
        let url = match fullurl {
            Some(u) => u.to_string(),
            None => self.make_full_url(filename),
        };
        let (mut urlobj, size) = self
            .grabber(&url)
            .map_err(|e| format!("Couldn't acquire file {url}: {e}"))?;
        log::debug!("Fetching URI: {url}");
        meter.start(&format!("Retrieving '{}'", basename(filename)), size);
        let mut total = 0u64;
        let mut buf = vec![0u8; BLOCK_SIZE];
        loop {
            let n = urlobj.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            total += n as u64;
            meter.update(total);
        }
        out.flush().map_err(|e| e.to_string())?;
        meter.end();
        Ok(())
    }

    /// `acquireFile`: save `filename` from the location to a temporary
    /// file in the scratchdir and return its path.
    pub fn acquire_file(
        &mut self,
        filename: &str,
        fullurl: Option<&str>,
        meter: &mut dyn Meter,
    ) -> Result<String, String> {
        if self.kind == Kind::Direct {
            let fullurl = fullurl.unwrap_or(filename).to_string();
            let mut fetcher = fetcher_for_uri(&fullurl, &self.scratchdir, true)?;
            return fetcher.acquire_file(basename(filename), Some(&fullurl), meter);
        }
        let (mut file, path) = named_tempfile(
            &self.scratchdir,
            "virtinst-",
            &format!("-{}", basename(filename)),
        )?;
        match self.grab_url(filename, &mut file, fullurl, meter) {
            Ok(()) => {
                log::debug!("Saved file to {path}");
                Ok(path)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                Err(e)
            }
        }
    }

    /// `acquireFileContent`: `filename` from the location, as text.
    pub fn acquire_file_content(
        &mut self,
        filename: &str,
        meter: &mut dyn Meter,
    ) -> Result<String, String> {
        let mut buf: Vec<u8> = vec![];
        self.grab_url(filename, &mut buf, None, meter)?;
        String::from_utf8(buf).map_err(|e| e.to_string())
    }
}

impl Drop for Fetcher {
    /// `_cleanup`.
    fn drop(&mut self) {
        if let Some(mut ftp) = self.ftp.take() {
            if let Err(e) = ftp.sendcmd("QUIT") {
                log::debug!("Error quitting ftp connection: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_like_python() {
        assert_eq!(path_join("/a", "b"), "/a/b");
        assert_eq!(path_join("/a/", "b"), "/a/b");
        assert_eq!(path_join("/a", "/b"), "/b");
        assert_eq!(url_path("http://h/x/y"), "/x/y");
        assert_eq!(url_path("/x/y"), "/x/y");
        let u = parse_ftp_url("ftp://me:p%40ss@example.com:2121/pub/tree");
        assert_eq!(
            (
                u.username.as_str(),
                u.password.as_str(),
                u.host.as_str(),
                u.port
            ),
            ("me", "p@ss", "example.com", 2121)
        );
        let u = parse_ftp_url("ftp://example.com/pub");
        assert_eq!((u.host.as_str(), u.port), ("example.com", 21));
    }
}
