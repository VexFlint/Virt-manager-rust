//! `install/urldetect.py`: detecting the distro of an install tree (a
//! URL, an ISO or a local directory) and where its kernel/initrd are.

use std::collections::HashMap;

use super::urlfetcher::Fetcher;
use crate::osinfo::{alphanum_key, OsDb, OsMedia, OsTree};
use crate::progress::make_meter;

/// Python's `re.match`: `pattern` anchored at the start of `text`.
fn re_match(pattern: &str, text: &str) -> bool {
    regex::Regex::new(&format!("^(?:{pattern})")).is_ok_and(|r| r.is_match(text))
}

/// The parts of `configparser.ConfigParser` that `.treeinfo` needs.
struct Ini {
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl Ini {
    fn parse(content: &str) -> Result<Ini, String> {
        let mut sections: Vec<(String, Vec<(String, String)>)> = vec![];
        for (lineno, raw) in content.lines().enumerate() {
            let line = raw.trim_end();
            let stripped = line.trim();
            if stripped.is_empty() || stripped.starts_with('#') || stripped.starts_with(';') {
                continue;
            }
            // Continuation line
            if raw.starts_with([' ', '\t']) {
                if let Some(opt) = sections.last_mut().and_then(|s| s.1.last_mut()) {
                    opt.1 += &format!("\n{stripped}");
                    continue;
                }
            }
            if let Some(name) = stripped.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                sections.push((name.to_string(), vec![]));
                continue;
            }
            let Some(sec) = sections.last_mut() else {
                return Err(format!(
                    "File contains no section headers.\nfile: '<string>', line: {}\n{:?}",
                    lineno + 1,
                    format!("{raw}\n")
                ));
            };
            let idx = stripped.find(['=', ':']).ok_or_else(|| {
                format!(
                    "Source contains parsing errors: '<string>'\n\t[line {:2}]: {:?}",
                    lineno + 1,
                    format!("{raw}\n")
                )
            })?;
            let key = stripped[..idx].trim().to_lowercase();
            let val = stripped[idx + 1..].trim().to_string();
            match sec.1.iter_mut().find(|(k, _)| *k == key) {
                Some(e) => e.1 = val,
                None => sec.1.push((key, val)),
            }
        }
        Ok(Ini { sections })
    }

    fn section(&self, section: &str) -> Option<&Vec<(String, String)>> {
        self.sections
            .iter()
            .find(|(n, _)| n == section)
            .map(|(_, o)| o)
    }

    fn get(&self, section: &str, option: &str) -> Result<String, String> {
        let sec = self
            .section(section)
            .ok_or_else(|| format!("No section: '{section}'"))?;
        let option = option.to_lowercase();
        sec.iter()
            .find(|(k, _)| *k == option)
            .map(|(_, v)| v.clone())
            .ok_or_else(|| format!("No option '{option}' in section: '{section}'"))
    }

    fn has_option(&self, section: &str, option: &str) -> bool {
        self.get(section, option).is_ok()
    }
}

/// `_SUSEContent`: the SUSE 'content' file.
struct SuseContent {
    tree_arch: Option<String>,
    product_name: Option<String>,
    product_version: Option<String>,
}

impl SuseContent {
    fn new(content_str: &str) -> Result<SuseContent, String> {
        let mut content_dict: HashMap<&str, String> = HashMap::new();
        for line in content_str.lines() {
            for prefix in [
                "LABEL",
                "DISTRO",
                "VERSION",
                "BASEARCHS",
                "DEFAULTBASE",
                "REPOID",
            ] {
                if let Some(rest) = line.strip_prefix(&format!("{prefix} ")) {
                    content_dict.insert(prefix, rest.to_string());
                }
            }
        }
        log::debug!("SUSE content dict: {content_dict:?}");

        // `_get_tree_arch`
        let mut distro_arch = content_dict
            .get("BASEARCHS")
            .filter(|s| !s.is_empty())
            .or_else(|| content_dict.get("DEFAULTBASE"))
            .filter(|s| !s.is_empty())
            .cloned();
        if distro_arch.is_none() {
            if let Some(repoid) = content_dict.get("REPOID") {
                distro_arch = Some(
                    repoid
                        .rsplit_once('/')
                        .map(|(_, a)| a.to_string())
                        .ok_or("list index out of range")?,
                );
            }
        }
        let tree_arch = distro_arch.map(|a| {
            let a = a.trim().to_string();
            // Fix for 13.2 official oss repo
            if a.contains("i586-x86_64") {
                "x86_64".to_string()
            } else {
                a
            }
        });

        // `_get_product_name`
        let product_name = if let Some(l) = content_dict.get("LABEL") {
            Some(l.clone())
        } else {
            content_dict
                .get("DISTRO")
                .filter(|d| d.contains(','))
                .and_then(|d| d.rsplit_once(',').map(|(_, n)| n.to_string()))
        };
        log::debug!(
            "SUSE content product_name={}",
            product_name.as_deref().unwrap_or("None")
        );

        // `_get_product_version`
        let product_version = match &product_name {
            None => None,
            Some(name) => {
                let mut distro_version = content_dict.get("VERSION").cloned().unwrap_or_default();
                if let Some((v, _)) = distro_version.split_once('-') {
                    distro_version = v.to_string();
                }
                // Special case, parse version out of a line like this
                // cpe:/o:opensuse:opensuse:13.2,openSUSE
                let distro = content_dict.get("DISTRO").cloned().unwrap_or_default();
                if distro_version.is_empty() && re_match("^.*:.*,openSUSE*", &distro) {
                    let head = distro.rsplit_once(',').map_or(distro.as_str(), |(h, _)| h);
                    distro_version = head
                        .trim()
                        .split(':')
                        .nth(4)
                        .ok_or("list index out of range")?
                        .to_string();
                }
                distro_version = distro_version.trim().to_string();
                if name.contains("Enterprise") || name.contains("SLES") {
                    let parts: Vec<&str> = name.trim().split(' ').collect();
                    let mut sle_version =
                        parts.get(4).ok_or("list index out of range")?.to_string();
                    if parts.len() > 5 {
                        let sp = parts[5].chars().nth(2).ok_or("string index out of range")?;
                        sle_version = format!("{sle_version}.{sp}");
                    }
                    distro_version = sle_version;
                }
                Some(distro_version)
            }
        };
        log::debug!(
            "SUSE content product_name={} product_version={} tree_arch={}",
            product_name.as_deref().unwrap_or("None"),
            product_version.as_deref().unwrap_or("None"),
            tree_arch.as_deref().unwrap_or("None")
        );
        Ok(SuseContent {
            tree_arch,
            product_name,
            product_version,
        })
    }
}

/// `_DistroCache`.
struct DistroCache<'a> {
    fetcher: &'a mut Fetcher,
    db: &'a OsDb,
    in_testsuite: bool,
    filecache: HashMap<String, Option<String>>,

    treeinfo: Option<Ini>,
    treeinfo_family: Option<String>,
    treeinfo_version: Option<String>,
    treeinfo_name: Option<String>,
    treeinfo_matched: bool,

    suse_content: Option<SuseContent>,
    checked_for_suse_content: bool,
    debian_media_type: Option<&'static str>,
    mageia_version: Option<String>,

    libosinfo_os_variant: Option<String>,
    libosinfo_mediaobj: Option<OsMedia>,
    libosinfo_treeobj: Option<OsTree>,
}

impl DistroCache<'_> {
    fn acquire_file_content(&mut self, path: &str) -> Option<String> {
        if !self.filecache.contains_key(path) {
            let mut meter = make_meter(true);
            let content = match self.fetcher.acquire_file_content(path, &mut meter) {
                Ok(c) => Some(c),
                Err(e) => {
                    log::debug!("Failed to acquire file={path}: {e}");
                    None
                }
            };
            self.filecache.insert(path.to_string(), content);
        }
        self.filecache[path].clone()
    }

    /// The `treeinfo` property.
    fn treeinfo(&mut self) -> Result<bool, String> {
        if self.treeinfo.is_some() {
            return Ok(true);
        }
        // Vast majority of trees here use .treeinfo. However, trees via
        // Red Hat satellite on akamai CDN will use treeinfo, because akamai
        // doesn't do dotfiles apparently:
        //
        //   https://bugzilla.redhat.com/show_bug.cgi?id=635065
        //
        // Anaconda is the canonical treeinfo consumer and they check for both
        // locations, so we need to do the same
        let treeinfostr = self
            .acquire_file_content(".treeinfo")
            .filter(|s| !s.is_empty())
            .or_else(|| self.acquire_file_content("treeinfo"));
        let Some(treeinfostr) = treeinfostr else {
            return Ok(false);
        };

        // If the file doesn't parse or there's no 'family', this will
        // error, but that should be fine because we aren't going to
        // successfully detect the tree anyways
        let treeinfo = Ini::parse(&treeinfostr)?;
        let family = treeinfo.get("general", "family")?;
        log::debug!("treeinfo family={family}");
        self.treeinfo_family = Some(family);

        if treeinfo.has_option("general", "version") {
            let v = treeinfo.get("general", "version")?;
            log::debug!("Found treeinfo version={v}");
            self.treeinfo_version = Some(v);
        }
        if treeinfo.has_option("general", "name") {
            let n = treeinfo.get("general", "name")?;
            log::debug!("Found treeinfo name={n}");
            self.treeinfo_name = Some(n);
        }
        self.treeinfo = Some(treeinfo);
        Ok(true)
    }

    fn treeinfo_family_regex(&mut self, famregex: &str) -> Result<bool, String> {
        if !self.treeinfo()? {
            return Ok(false);
        }
        let ret = re_match(famregex, self.treeinfo_family.as_deref().unwrap_or(""));
        self.treeinfo_matched = ret;
        if !ret {
            log::debug!("Didn't match treeinfo family regex={famregex}");
        }
        Ok(ret)
    }

    /// `content_regex`: does `filename` have a line matching `regex`.
    fn content_regex(&mut self, filename: &str, regex: &str) -> bool {
        let Some(content) = self.acquire_file_content(filename) else {
            return false;
        };
        if content.lines().any(|l| re_match(regex, l)) {
            return true;
        }
        log::debug!("found filename={filename} but regex={regex} didn't match");
        false
    }

    /// `get_treeinfo_media`: kernel/initrd paths from the treeinfo.
    fn get_treeinfo_media(&mut self, typ: &str) -> Vec<(String, String)> {
        if let Err(e) = self.treeinfo() {
            log::debug!("Failed to parse treeinfo kernel/initrd: {e}");
        }
        let Some(treeinfo) = &self.treeinfo else {
            log::debug!("Failed to parse treeinfo kernel/initrd");
            return vec![];
        };
        let get = |media_name: &str| -> Result<String, String> {
            let mut image_type = treeinfo.get("general", "arch")?;
            if typ == "xen" {
                image_type = "xen".into();
            }
            treeinfo.get(&format!("images-{image_type}"), media_name)
        };
        match (get("kernel"), get("initrd")) {
            (Ok(k), Ok(i)) => vec![(k, i)],
            _ => {
                log::debug!("Failed to parse treeinfo kernel/initrd");
                vec![]
            }
        }
    }

    fn split_version(&self) -> (i64, i64) {
        let verstr = self.treeinfo_version.clone().unwrap_or_default();
        let safeint = |c: &str| c.parse::<i64>().unwrap_or(0);
        // Parse a string like 6.9 or 7.4 into its two parts
        // centos altarch's have just version=7
        let mut update = 0;
        let mut version = safeint(&verstr);
        if verstr.contains('.') {
            let mut parts = verstr.split('.');
            version = safeint(parts.next().unwrap_or(""));
            update = safeint(parts.next().unwrap_or(""));
        }
        log::debug!("converted verstr={verstr} to version={version} update={update}");
        (version, update)
    }

    fn guess_os_from_iso(&mut self) -> bool {
        let Some((os, media)) = self.db.guess_media_by_iso(&self.fetcher.location) else {
            return false;
        };
        self.libosinfo_os_variant = Some(os.short_id());
        let complete = media.get_kernel_path().is_some() && media.get_initrd_path().is_some();
        self.libosinfo_mediaobj = Some(media);
        if !complete {
            // This can happen if the media is live media, or just
            // with incomplete libosinfo data
            log::debug!(
                "libosinfo didn't report any media kernel/initrd path for detected os_variant"
            );
            return false;
        }
        true
    }

    fn guess_os_from_tree(&mut self) -> bool {
        let Some((os, tree)) = self
            .db
            .guess_tree(&self.fetcher.location, self.in_testsuite)
        else {
            return false;
        };
        self.libosinfo_os_variant = Some(os.short_id());
        self.libosinfo_treeobj = Some(tree);
        self.treeinfo_matched = true;
        true
    }
}

/// The `_DistroTree` subclasses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Distro {
    Libosinfo,
    Fedora,
    Rhel,
    CentOS,
    Sles,
    Sled,
    Opensuse,
    Debian,
    Ubuntu,
    Mageia,
    GenericTreeinfo,
}

impl Distro {
    fn class_name(self) -> &'static str {
        match self {
            Distro::Libosinfo => "_LibosinfoDistro",
            Distro::Fedora => "_FedoraDistro",
            Distro::Rhel => "_RHELDistro",
            Distro::CentOS => "_CentOSDistro",
            Distro::Sles => "_SLESDistro",
            Distro::Sled => "_SLEDDistro",
            Distro::Opensuse => "_OpensuseDistro",
            Distro::Debian => "_DebianDistro",
            Distro::Ubuntu => "_UbuntuDistro",
            Distro::Mageia => "_MageiaDistro",
            Distro::GenericTreeinfo => "_GenericTreeinfoDistro",
        }
    }

    fn matching_distros(self) -> &'static [&'static str] {
        match self {
            Distro::Fedora => &["fedora"],
            Distro::Rhel => &["rhel"],
            Distro::CentOS => &["centos"],
            Distro::Sles => &["sles"],
            Distro::Sled => &["sled"],
            Distro::Opensuse => &["opensuse"],
            Distro::Debian => &["debian"],
            Distro::Ubuntu => &["ubuntu"],
            Distro::Mageia => &["mageia"],
            Distro::Libosinfo | Distro::GenericTreeinfo => &[],
        }
    }

    fn variant_prefix(self) -> &'static str {
        match self {
            Distro::CentOS => "centos",
            Distro::Sles => "sles",
            Distro::Sled => "sled",
            Distro::Opensuse => "opensuse",
            _ => "rhel",
        }
    }

    fn is_suse(self) -> bool {
        matches!(self, Distro::Sles | Distro::Sled | Distro::Opensuse)
    }

    fn debname(self) -> &'static str {
        if self == Distro::Ubuntu {
            "ubuntu"
        } else {
            "debian"
        }
    }

    /// `is_valid`.
    fn is_valid(self, cache: &mut DistroCache) -> Result<bool, String> {
        Ok(match self {
            Distro::Libosinfo => {
                if cache.fetcher.is_iso() {
                    cache.guess_os_from_iso()
                } else {
                    cache.guess_os_from_tree()
                }
            }
            Distro::Fedora => cache.treeinfo_family_regex(".*Fedora.*")?,
            // Matches:
            //   Red Hat Enterprise Linux
            //   RHEL Atomic Host
            Distro::Rhel => cache.treeinfo_family_regex(".*(Red Hat Enterprise Linux|RHEL).*")?,
            Distro::CentOS => {
                cache.treeinfo_family_regex(".*CentOS.*")?
                    || cache.treeinfo_family_regex(".*Scientific.*")?
            }
            Distro::Sles | Distro::Sled | Distro::Opensuse => {
                let (famregex, suse_regex): (&str, &[&str]) = match self {
                    Distro::Sles => (
                        ".*SUSE Linux Enterprise.*",
                        &[".*SUSE Linux Enterprise Server*", ".*SUSE SLES*"],
                    ),
                    Distro::Sled => (
                        ".*SUSE Linux Enterprise.*",
                        &[".*SUSE Linux Enterprise Desktop*"],
                    ),
                    _ => (".*openSUSE.*", &[".*openSUSE.*"]),
                };
                if cache.treeinfo_family_regex(famregex)? {
                    return Ok(true);
                }
                if !cache.checked_for_suse_content {
                    cache.checked_for_suse_content = true;
                    let Some(content_str) = cache.acquire_file_content("content") else {
                        return Ok(false);
                    };
                    match SuseContent::new(&content_str) {
                        Ok(c) => cache.suse_content = Some(c),
                        Err(e) => {
                            log::debug!("Error parsing SUSE content file: {e}");
                            return Ok(false);
                        }
                    }
                }
                let Some(content) = &cache.suse_content else {
                    return Ok(false);
                };
                let name = content.product_name.clone().unwrap_or_default();
                suse_regex.iter().any(|r| re_match(r, &name))
            }
            Distro::Debian | Distro::Ubuntu => {
                let debname = self.debname();
                let check_manifest = |cache: &mut DistroCache, mfile: &str| {
                    let is_ubuntu = debname == "ubuntu";
                    if cache.content_regex(mfile, ".*[Uu]buntu.*") {
                        return is_ubuntu;
                    }
                    cache.content_regex(mfile, ".*[Dd]ebian.*")
                };
                let mut capitalized = debname.to_string();
                capitalized[..1].make_ascii_uppercase();
                let media_type = if check_manifest(cache, "current/images/MANIFEST") {
                    Some("url")
                } else if check_manifest(cache, "current/legacy-images/MANIFEST") {
                    Some("legacy_url")
                } else if check_manifest(cache, "daily/MANIFEST") {
                    Some("daily")
                } else if cache.content_regex(".disk/info", &format!("{capitalized}.*")) {
                    // There's two cases here:
                    // 1) Direct access ISO, attached as CDROM afterwards. We
                    //    use one set of kernels in that case which seem to
                    //    assume the presence of CDROM media
                    // 2) ISO mounted and exported over URL. We use a different
                    //    set of kernels that expect to boot from the network
                    if cache.fetcher.is_iso() {
                        Some("disk")
                    } else {
                        Some("mounted_iso_url")
                    }
                } else {
                    None
                };
                if media_type.is_some() {
                    cache.debian_media_type = media_type;
                }
                media_type.is_some()
            }
            Distro::Mageia => {
                if cache.mageia_version.is_none() {
                    let Some(content) = cache
                        .acquire_file_content("VERSION")
                        .filter(|c| !c.is_empty())
                    else {
                        return Ok(false);
                    };
                    let re = regex::Regex::new(r"^Mageia (\d+) .*").expect("valid regex");
                    let Some(m) = re.captures(&content) else {
                        return Ok(false);
                    };
                    cache.mageia_version = Some(m[1].to_string());
                }
                cache.mageia_version.is_some()
            }
            Distro::GenericTreeinfo => {
                if cache.treeinfo()? {
                    cache.treeinfo_matched = true;
                    true
                } else {
                    false
                }
            }
        })
    }
}

/// What `getDistroStore` found: the `_DistroTree` instance.
pub struct DistroStore {
    pub kernel_paths: Vec<(String, String)>,
    pub os_variant: Option<String>,
    pub os_media: Option<OsMedia>,
    pub os_tree: Option<OsTree>,
}

/// The `_DistroTree` constructor and its per-distro hooks.
struct TreeBuilder<'c, 'a> {
    distro: Distro,
    vmtype: String,
    arch: String,
    uri: String,
    cache: &'c mut DistroCache<'a>,
    os_variant: Option<String>,
    kernel_paths: Vec<(String, String)>,
}

impl TreeBuilder<'_, '_> {
    fn lookup(&self, name: &str) -> bool {
        self.cache.db.lookup_os(name).is_some()
    }

    /// `_detect_version`.
    fn detect_version(&mut self) -> Option<String> {
        match self.distro {
            Distro::Fedora => {
                let latest_variant = "fedora-unknown".to_string();
                let Some(verstr) = self.cache.treeinfo_version.clone() else {
                    log::debug!("No treeinfo version? Assume latest_variant={latest_variant}");
                    return Some(latest_variant);
                };
                // rawhide trees changed to use version=Rawhide in Apr 2016
                if ["development", "rawhide", "Rawhide"].contains(&verstr.as_str()) {
                    log::debug!("treeinfo version={verstr}, using latest_variant={latest_variant}");
                    return Some(latest_variant);
                }
                // treeinfo version is just an integer
                let variant = format!("fedora{verstr}");
                if self.lookup(&variant) {
                    return Some(variant);
                }
                log::debug!(
                    "variant={variant} from treeinfo version={verstr} not found, using \
                     latest_variant={latest_variant}"
                );
                Some(latest_variant)
            }
            Distro::Rhel | Distro::CentOS => {
                if self.cache.treeinfo_version.is_none() {
                    log::debug!("No treeinfo version? Not setting an os_variant");
                    return None;
                }
                let (version, mut update) = self.cache.split_version();
                // start with example base=rhel7, then walk backwards
                // through the OS list to find the latest os name that matches
                // this way we handle rhel7.6 from treeinfo when osdict only
                // knows about rhel7.5
                let base = format!("{}{version}", self.distro.variant_prefix());
                while update >= 0 {
                    let tryvar = format!("{base}.{update}");
                    if self.lookup(&tryvar) {
                        return Some(tryvar);
                    }
                    update -= 1;
                }
                None
            }
            Distro::Sles | Distro::Sled | Distro::Opensuse => self
                .suse_detect_from_treeinfo()
                .or_else(|| self.suse_detect_osdict_from_url())
                .or_else(|| self.suse_detect_osdict_from_suse_content()),
            Distro::Debian | Distro::Ubuntu => self.debian_detect_version(),
            Distro::Mageia => {
                // version is just an integer
                let variant = format!("mageia{}", self.cache.mageia_version.clone()?);
                self.lookup(&variant).then_some(variant)
            }
            Distro::Libosinfo | Distro::GenericTreeinfo => {
                log::debug!(
                    "{} does not implement any osdict detection",
                    self.distro.class_name()
                );
                None
            }
        }
    }

    fn sorted_oses(&self, root: &str) -> Vec<crate::osinfo::OsInfo> {
        let mut oses: Vec<_> = self
            .cache
            .db
            .all_os()
            .into_iter()
            .filter(|o| o.short_id().starts_with(root))
            .collect();
        oses.sort_by_cached_key(|o| alphanum_key(&o.short_id()));
        oses
    }

    fn suse_detect_osdict_from_suse_content(&self) -> Option<String> {
        let content = self.cache.suse_content.as_ref()?;
        let distro_version = content.product_version.clone().filter(|v| !v.is_empty())?;
        let (version, rest) = match distro_version.split_once('.') {
            Some((v, r)) => (v.trim().to_string(), Some(r.trim().to_string())),
            None => (distro_version.trim().to_string(), None),
        };
        let prefix = self.distro.variant_prefix();
        if prefix.starts_with("sles") || prefix.starts_with("sled") {
            let sp_version = rest.map(|r| format!("sp{r}")).unwrap_or_default();
            return Some(format!("{prefix}{version}{sp_version}"));
        }
        Some(format!("{prefix}{distro_version}"))
    }

    fn suse_detect_osdict_from_url(&self) -> Option<String> {
        let root = "opensuse";
        for osobj in self.sorted_oses(root) {
            let name = osobj.short_id();
            let codename = &name[root.len()..];
            if re_match(&format!(".*/{codename}/"), &self.uri) {
                return Some(name);
            }
        }
        None
    }

    fn suse_detect_from_treeinfo(&self) -> Option<String> {
        let name = self.cache.treeinfo_name.as_deref()?;
        if name.contains("openSUSE Tumbleweed") {
            return Some("opensusetumbleweed".into());
        }
        let (version, mut update) = self.cache.split_version();
        let base = format!("{}{version}", self.distro.variant_prefix());
        while update >= 0 {
            let mut tryvar = base.clone();
            // SLE doesn't use '.0' for initial releases in
            // osinfo-db (sles11, sles12, etc)
            if update > 0 || !base.starts_with("sle") {
                tryvar += &format!(".{update}");
            }
            if self.lookup(&tryvar) {
                return Some(tryvar);
            }
            update -= 1;
        }
        None
    }

    fn debian_detect_version(&mut self) -> Option<String> {
        let debname = self.distro.debname();
        let oses = self.sorted_oses(debname);
        let mut disk_info = None;
        match self.cache.debian_media_type {
            Some("daily") => {
                log::debug!("Appears to be debian 'daily' URL, using latest debiantesting");
                return Some("debiantesting".into());
            }
            Some("disk") => {
                disk_info = self
                    .cache
                    .acquire_file_content(".disk/info")
                    .map(|s| s.to_lowercase());
            }
            _ => {}
        }
        for osobj in oses {
            let codename = osobj.codename();
            let codename = if !codename.is_empty() {
                // Ubuntu codenames look like 'Warty Warthog'
                codename
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_lowercase()
            } else {
                let label = osobj.name();
                if !label.contains(' ') {
                    continue;
                }
                // Debian labels look like 'Debian Sarge'
                label.split_whitespace().nth(1).unwrap_or("").to_lowercase()
            };
            if self.uri.contains(&format!("/{codename}/")) {
                log::debug!("Found codename={codename} in the URL string");
                return Some(osobj.short_id());
            }
            if disk_info
                .as_deref()
                .is_some_and(|d| d.contains(&format!("\"{codename}\"")))
            {
                log::debug!("Found codename={codename} in the disk/info file");
                return Some(osobj.short_id());
            }
        }
        None
    }

    /// `_set_manual_kernel_paths`.
    fn set_manual_kernel_paths(&mut self) {
        match self.distro {
            d if d.is_suse() => self.suse_kernel_paths(),
            Distro::Debian | Distro::Ubuntu => {
                if self.cache.debian_media_type == Some("disk") {
                    self.debian_installcd_paths();
                } else {
                    self.debian_url_paths();
                }
            }
            Distro::Mageia => {
                let a = &self.arch;
                self.kernel_paths.push((
                    format!("isolinux/{a}/vmlinuz"),
                    format!("isolinux/{a}/all.rdz"),
                ));
            }
            Distro::Libosinfo => {
                if let Some(m) = &self.cache.libosinfo_mediaobj {
                    self.kernel_paths.push((
                        m.get_kernel_path().unwrap_or_default(),
                        m.get_initrd_path().unwrap_or_default(),
                    ));
                }
            }
            _ => {}
        }
    }

    fn suse_kernel_paths(&mut self) {
        // We only reach here if no treeinfo was matched
        let mut tree_arch = self
            .cache
            .suse_content
            .as_ref()
            .and_then(|c| c.tree_arch.clone())
            .unwrap_or_default();
        if re_match(r"i[4-9]86", &tree_arch) {
            tree_arch = "i386".into();
        }
        let mut oldkern = "linux".to_string();
        let mut oldinit = "initrd".to_string();
        if tree_arch == "x86_64" {
            oldkern += "64";
            oldinit += "64";
        }
        let t = &tree_arch;
        if self.vmtype == "xen" {
            // Matches Opensuse > 10.2 and sles 10
            self.kernel_paths.push((
                format!("boot/{t}/vmlinuz-xen"),
                format!("boot/{t}/initrd-xen"),
            ));
        }
        let variant = self.os_variant.clone().unwrap_or_else(|| "None".into());
        if variant.starts_with("sles11") || variant.starts_with("sled11") {
            if t == "s390x" {
                self.kernel_paths
                    .push(("boot/s390x/vmrdr.ikr".into(), "boot/s390x/initrd".into()));
            }
            if t == "ppc64" {
                self.kernel_paths
                    .push(("suseboot/linux64".into(), "suseboot/initrd64".into()));
            }
        }
        // Tested with SLES 12 for ppc64le, all s390x
        self.kernel_paths
            .push((format!("boot/{t}/linux"), format!("boot/{t}/initrd")));
        // Tested with Opensuse 10.0
        self.kernel_paths.push((
            format!("boot/loader/{oldkern}"),
            format!("boot/loader/{oldinit}"),
        ));
        // Tested with Opensuse >= 10.2, 11, and sles 10
        self.kernel_paths.push((
            format!("boot/{t}/loader/linux"),
            format!("boot/{t}/loader/initrd"),
        ));
    }

    fn debian_find_treearch(&self) -> Option<String> {
        for pattern in [r"^.*/installer-(\w+)/?$", r"^.*/daily-images/(\w+)/?$"] {
            let re = regex::Regex::new(pattern).expect("valid regex");
            if let Some(c) = re.captures(&self.uri) {
                log::debug!("Found pattern={pattern} treearch={} in uri", &c[1]);
                return Some(c[1].to_string());
            }
        }
        // Check for standard arch strings which will be
        // in the URI name for --location $ISO mounts
        for arch in ["i386", "amd64", "arm64", "riscv64", "s390x", "ppc64el"] {
            if self.uri.contains(arch) {
                log::debug!("Found treearch={arch} in uri");
                return Some(arch.into());
            }
        }
        None
    }

    fn debian_url_paths(&mut self) {
        let url_prefix = match self.cache.debian_media_type {
            Some("daily") => "daily",
            Some("mounted_iso_url") => "install",
            Some("legacy_url") => "current/legacy-images",
            _ => "current/images",
        };
        let tree_arch = self.debian_find_treearch().unwrap_or_else(|| {
            log::debug!("No treearch found in uri, defaulting to arch=i386");
            "i386".into()
        });
        let debname = self.distro.debname();
        let mut hvmroot = format!("{url_prefix}/netboot/{debname}-installer/{tree_arch}/");
        let mut initrd_basename = "initrd.gz".to_string();
        let mut kernel_basename = "linux".to_string();
        if tree_arch == "ppc64el" {
            kernel_basename = "vmlinux".into();
        }
        if tree_arch == "s390x" {
            hvmroot = format!("{url_prefix}/generic/");
            kernel_basename = format!("kernel.{debname}");
            initrd_basename = format!("initrd.{debname}");
        }
        if self.vmtype == "xen" {
            let xenroot = format!("{url_prefix}/netboot/xen/");
            self.kernel_paths
                .push((format!("{xenroot}vmlinuz"), format!("{xenroot}initrd.gz")));
        }
        self.kernel_paths.push((
            format!("{hvmroot}{kernel_basename}"),
            format!("{hvmroot}{initrd_basename}"),
        ));
    }

    fn debian_installcd_paths(&mut self) {
        let tree_arch = self.debian_find_treearch();
        let kpair = if self.distro.debname() == "ubuntu" {
            if tree_arch.as_deref() != Some("s390x") {
                ("install/vmlinuz", "install/initrd.gz")
            } else {
                ("boot/kernel.ubuntu", "boot/initrd.ubuntu")
            }
        } else {
            match tree_arch.as_deref() {
                Some("amd64") => ("install.amd/vmlinuz", "install.amd/initrd.gz"),
                Some("i386") => ("install.386/vmlinuz", "install.386/initrd.gz"),
                Some("arm64") => ("install.a64/vmlinuz", "install.a64/initrd.gz"),
                Some("ppc64el") => ("install/vmlinux", "install/initrd.gz"),
                Some("s390x") => ("boot/linux_vm", "boot/root.bin"),
                _ => ("install/vmlinuz", "install/initrd.gz"),
            }
        };
        self.kernel_paths
            .push((kpair.0.to_string(), kpair.1.to_string()));
    }

    fn build(mut self) -> DistroStore {
        self.os_variant = match self.cache.libosinfo_os_variant.clone() {
            Some(v) => Some(v),
            None => self.detect_version(),
        };
        if let Some(v) = &self.os_variant {
            if !self.lookup(v) {
                log::debug!("Detected os_variant as {v}, which is not in osdict.");
                self.os_variant = None;
            }
        }
        if self.cache.treeinfo_matched {
            self.kernel_paths = self.cache.get_treeinfo_media(&self.vmtype);
        } else {
            self.set_manual_kernel_paths();
        }
        DistroStore {
            kernel_paths: self.kernel_paths,
            os_variant: self.os_variant,
            os_media: self.cache.libosinfo_mediaobj.take(),
            os_tree: self.cache.libosinfo_treeobj.take(),
        }
    }
}

/// `_build_distro_list`.
fn build_distro_list(db: &OsDb, os_name: &str) -> Vec<Distro> {
    let mut allstores = vec![
        // Libosinfo takes priority
        Distro::Libosinfo,
        Distro::Fedora,
        Distro::Rhel,
        Distro::CentOS,
        Distro::Sles,
        Distro::Sled,
        Distro::Opensuse,
        Distro::Debian,
        Distro::Ubuntu,
        Distro::Mageia,
        // Always stick GenericDistro at the end, since it's a catchall
        Distro::GenericTreeinfo,
    ];

    // If user manually specified an os_distro, bump its URL class
    // to the top of the list
    let distro = db
        .lookup_os(os_name)
        .map(|o| o.distro())
        .unwrap_or_default();
    if !distro.is_empty() {
        log::debug!(
            "variant={os_name} has distro={distro}, looking for matching distro store to \
             prioritize"
        );
        let found = allstores
            .iter()
            .rev()
            .find(|s| s.matching_distros().contains(&distro.as_str()))
            .copied();
        match found {
            Some(store) => {
                log::debug!("Prioritizing distro store={}", store.class_name());
                allstores.retain(|s| *s != store);
                allstores.insert(0, store);
            }
            None => log::debug!("No matching store found, not prioritizing anything"),
        }
    }

    if let Ok(force) = std::env::var("VIRTINST_TEST_SUITE_FORCE_LIBOSINFO") {
        if force.parse::<i64>().unwrap_or(0) != 0 {
            allstores = vec![Distro::Libosinfo];
        } else {
            allstores.retain(|s| *s != Distro::Libosinfo);
        }
    }
    allstores
}

/// The guest facts `getDistroStore` reads.
pub struct GuestFacts<'a> {
    pub arch: &'a str,
    pub os_type: &'a str,
    pub os_name: &'a str,
}

/// `getDistroStore`.
pub fn get_distro_store(
    guest: &GuestFacts,
    fetcher: &mut Fetcher,
    skip_error: bool,
    in_testsuite: bool,
) -> Result<Option<DistroStore>, String> {
    log::debug!("Finding distro store for location={}", fetcher.location);
    let db = OsDb::load_default().map_err(|e| e.to_string())?;
    let stores = build_distro_list(&db, guest.os_name);
    let location = fetcher.location.clone();
    let mut cache = DistroCache {
        fetcher,
        db: &db,
        in_testsuite,
        filecache: HashMap::new(),
        treeinfo: None,
        treeinfo_family: None,
        treeinfo_version: None,
        treeinfo_name: None,
        treeinfo_matched: false,
        suse_content: None,
        checked_for_suse_content: false,
        debian_media_type: None,
        mageia_version: None,
        libosinfo_os_variant: None,
        libosinfo_mediaobj: None,
        libosinfo_treeobj: None,
    };

    for sclass in stores {
        if !sclass.is_valid(&mut cache)? {
            continue;
        }
        let store = TreeBuilder {
            distro: sclass,
            vmtype: guest.os_type.into(),
            arch: guest.arch.into(),
            uri: location.clone(),
            cache: &mut cache,
            os_variant: None,
            kernel_paths: vec![],
        }
        .build();
        log::debug!(
            "Detected class={} osvariant={}",
            sclass.class_name(),
            store.os_variant.as_deref().unwrap_or("None")
        );
        return Ok(Some(store));
    }

    if skip_error {
        return Ok(None);
    }

    // No distro was detected. See if the URL even resolves, and if not
    // give the user a hint that maybe they mistyped. This won't always
    // be true since some webservers don't allow directory listing.
    // https://www.redhat.com/archives/virt-tools-list/2014-December/msg00048.html
    let mut extramsg = "";
    if !cache.fetcher.can_access() {
        extramsg = ": The URL could not be accessed, maybe you mistyped?";
    }
    Err(format!(
        "Could not find an installable distribution at URL '{location}'{extramsg}\n\n\
         The location must be the root directory of an install tree.\n\
         See virt-install man page for various distro examples."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treeinfo_parses_like_configparser() {
        let ini =
            Ini::parse("[general]\nFamily = Fedora\nversion: 17\n\n[images-x86_64]\nkernel = a\n")
                .unwrap();
        assert_eq!(ini.get("general", "family").unwrap(), "Fedora");
        assert_eq!(ini.get("general", "version").unwrap(), "17");
        assert_eq!(ini.get("images-x86_64", "kernel").unwrap(), "a");
        assert_eq!(
            ini.get("general", "name").unwrap_err(),
            "No option 'name' in section: 'general'"
        );
        assert_eq!(ini.get("nope", "x").unwrap_err(), "No section: 'nope'");
    }

    #[test]
    fn suse_content_versions() {
        let c = SuseContent::new(
            "DISTRO cpe:/o:suse:sled:12:sp3,SUSE Linux Enterprise Desktop 12 SP3\nREPOID obsproduct://build.suse.de/SUSE:SLE-12-SP3:GA/SLED/12.3/DVD/x86_64\n",
        )
        .unwrap();
        assert_eq!(c.tree_arch.as_deref(), Some("x86_64"));
        assert_eq!(
            c.product_name.as_deref(),
            Some("SUSE Linux Enterprise Desktop 12 SP3")
        );
        assert_eq!(c.product_version.as_deref(), Some("12.3"));

        let c = SuseContent::new(
            "DISTRO cpe:/o:opensuse:opensuse:13.2,openSUSE\nBASEARCHS i586 x86_64\n",
        )
        .unwrap();
        assert_eq!(c.product_version.as_deref(), Some("13.2"));
        assert_eq!(c.tree_arch.as_deref(), Some("i586 x86_64"));
    }
}
