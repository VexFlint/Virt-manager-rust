//! `<network>` — upstream's `virtinst/network.py` (`Network` and its
//! child classes), plus the GTK-free parts of the two virt-manager
//! files built on it:
//!
//! - `virtManager/object/network.py` (`vmmNetwork`): the
//!   `get_ipv4_network`/`get_ipv6_network` summaries and
//!   `pretty_forward_mode` the Virtual Networks tab shows;
//! - `virtManager/createnet.py` (`vmmCreateNetwork`): the DHCP range
//!   defaults, `_build_xmlobj_from_ui`, `_validate` and
//!   `_async_net_create`.
//!
//! Like `domain/pm.rs`, the model comes twice: [`Network`] is the typed
//! read side (`#[derive(XmlBound)]`), [`NETWORK`] the `XMLBuilder` class
//! table a new network is built with, so its XML comes out in
//! upstream's `_XML_PROP_ORDER`.
//!
//! The address math is Python's `ipaddress` module, ported as far as
//! these callers use it ([`IpNetwork`], [`IpAddress`]): the strings
//! they produce end up in the XML and the UI, so they have to match.

use virt::connect::Connect;
use virt::network::Network as VirNetwork;
use virtinst_xml::{OnOff, XmlBound};

use crate::xmlbuilder::{child, prop, Kind, XmlClass, XmlObject};

// ---------------------------------------------------------------------
// ipaddress
// ---------------------------------------------------------------------

/// An `IPv4Address`/`IPv6Address`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpAddress {
    v6: bool,
    value: u128,
}

/// An `IPv4Network`/`IPv6Network`, always with host bits masked off
/// (`ip_network(..., strict=False)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpNetwork {
    v6: bool,
    network: u128,
    prefixlen: u32,
}

fn max_prefixlen(v6: bool) -> u32 {
    if v6 {
        128
    } else {
        32
    }
}

fn all_ones(v6: bool) -> u128 {
    if v6 {
        u128::MAX
    } else {
        u32::MAX as u128
    }
}

fn mask_for(v6: bool, prefixlen: u32) -> u128 {
    let bits = max_prefixlen(v6);
    if prefixlen == 0 {
        0
    } else {
        (all_ones(v6) >> (bits - prefixlen)) << (bits - prefixlen) & all_ones(v6)
    }
}

/// `IPv4Address._ip_int_from_string`.
fn parse_v4(s: &str) -> Option<u128> {
    // Rust's parser takes the same dotted-quad forms: four decimal
    // octets, no leading zeros.
    s.parse::<std::net::Ipv4Addr>()
        .ok()
        .map(|a| u32::from(a) as u128)
}

/// `IPv6Address._ip_int_from_string`, without scope ids.
fn parse_v6(s: &str) -> Option<u128> {
    s.parse::<std::net::Ipv6Addr>().ok().map(u128::from)
}

/// `_BaseNetwork._prefix_from_prefix_string`.
fn prefix_from_string(s: &str, v6: bool) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = s.parse().ok()?;
    (n <= max_prefixlen(v6)).then_some(n)
}

/// `_prefix_from_ip_int`: the prefix of a contiguous netmask.
fn prefix_from_mask(mask: u128, v6: bool) -> Option<u32> {
    let bits = max_prefixlen(v6);
    let trailing_zeroes = if mask == 0 {
        bits
    } else {
        mask.trailing_zeros().min(bits)
    };
    let prefixlen = bits - trailing_zeroes;
    (mask == mask_for(v6, prefixlen)).then_some(prefixlen)
}

/// `IPv4Network._make_netmask`: a prefix length, a netmask, or a
/// hostmask.
fn v4_netmask_prefix(arg: &str) -> Option<u32> {
    if let Some(p) = prefix_from_string(arg, false) {
        return Some(p);
    }
    let ip = parse_v4(arg)?;
    prefix_from_mask(ip, false).or_else(|| prefix_from_mask(ip ^ all_ones(false), false))
}

impl IpAddress {
    /// `ipaddress.ip_address(s)`.
    pub fn parse(s: &str) -> Result<IpAddress, String> {
        if let Some(v) = parse_v4(s) {
            return Ok(IpAddress {
                v6: false,
                value: v,
            });
        }
        if let Some(v) = parse_v6(s) {
            return Ok(IpAddress { v6: true, value: v });
        }
        Err(format!(
            "'{s}' does not appear to be an IPv4 or IPv6 address"
        ))
    }

    pub fn version(&self) -> u8 {
        if self.v6 {
            6
        } else {
            4
        }
    }

    /// `address + n`, with `_check_int_address`'s range errors.
    pub fn checked_add(&self, n: i128) -> Result<IpAddress, String> {
        let bits = max_prefixlen(self.v6);
        let ver = self.version();
        if n < 0 {
            let sub = n.unsigned_abs();
            return match self.value.checked_sub(sub) {
                Some(value) => Ok(IpAddress { v6: self.v6, value }),
                None => Err(format!(
                    "-{} (< 0) is not permitted as an IPv{ver} address",
                    sub - self.value
                )),
            };
        }
        let sum = self.value.checked_add(n as u128);
        match sum {
            Some(value) if value <= all_ones(self.v6) => Ok(IpAddress { v6: self.v6, value }),
            // Python ints don't overflow: the message has the sum.
            _ => Err(format!(
                "{} (>= 2**{bits}) is not permitted as an IPv{ver} address",
                decimal_sum(self.value, n as u128)
            )),
        }
    }
}

/// `a + b` in decimal, past `u128` if need be.
fn decimal_sum(a: u128, b: u128) -> String {
    match a.checked_add(b) {
        Some(s) => s.to_string(),
        None => {
            // a + b = 2**128 + (a + b - 2**128), with the remainder
            // below 2**128.
            let rest = a.wrapping_add(b);
            let mut digits: Vec<u8> = rest.to_string().bytes().rev().map(|d| d - b'0').collect();
            let two128: Vec<u8> = u128::MAX
                .to_string()
                .bytes()
                .rev()
                .map(|d| d - b'0')
                .collect();
            // u128::MAX + 1 == 2**128
            let mut carry = 1u8;
            let n = digits.len().max(two128.len());
            digits.resize(n, 0);
            for (i, d) in digits.iter_mut().enumerate() {
                let s = *d + two128.get(i).copied().unwrap_or(0) + carry;
                *d = s % 10;
                carry = s / 10;
            }
            if carry > 0 {
                digits.push(carry);
            }
            digits.iter().rev().map(|d| (d + b'0') as char).collect()
        }
    }
}

/// `_compress_hextets`: the longest run of two or more zero hextets,
/// leftmost on a tie, becomes `::`.
fn format_v6(value: u128) -> String {
    let hextets: Vec<u16> = (0..8)
        .rev()
        .map(|i| ((value >> (i * 16)) & 0xffff) as u16)
        .collect();
    let (mut best_start, mut best_len) = (None, 0);
    let mut i = 0;
    while i < 8 {
        if hextets[i] == 0 {
            let start = i;
            while i < 8 && hextets[i] == 0 {
                i += 1;
            }
            if i - start > best_len {
                best_len = i - start;
                best_start = Some(start);
            }
        } else {
            i += 1;
        }
    }
    let hex = |s: &[u16]| {
        s.iter()
            .map(|h| format!("{h:x}"))
            .collect::<Vec<_>>()
            .join(":")
    };
    match best_start {
        Some(start) if best_len > 1 => format!(
            "{}::{}",
            hex(&hextets[..start]),
            hex(&hextets[start + best_len..])
        ),
        _ => hex(&hextets),
    }
}

impl std::fmt::Display for IpAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.v6 {
            f.write_str(&format_v6(self.value))
        } else {
            write!(f, "{}", std::net::Ipv4Addr::from(self.value as u32))
        }
    }
}

impl IpNetwork {
    /// `ipaddress.ip_network(s, strict=False)`.
    pub fn parse(s: &str) -> Result<IpNetwork, String> {
        let err = || format!("'{s}' does not appear to be an IPv4 or IPv6 network");
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() > 2 {
            return Err(err());
        }
        let (addr, prefix) = (parts[0], parts.get(1).copied());
        if let Some(a) = parse_v4(addr) {
            let prefixlen = match prefix {
                None => 32,
                Some(p) => v4_netmask_prefix(p).ok_or_else(err)?,
            };
            return Ok(IpNetwork {
                v6: false,
                network: a & mask_for(false, prefixlen),
                prefixlen,
            });
        }
        if let Some(a) = parse_v6(addr) {
            let prefixlen = match prefix {
                None => 128,
                Some(p) => prefix_from_string(p, true).ok_or_else(err)?,
            };
            return Ok(IpNetwork {
                v6: true,
                network: a & mask_for(true, prefixlen),
                prefixlen,
            });
        }
        Err(err())
    }

    pub fn version(&self) -> u8 {
        if self.v6 {
            6
        } else {
            4
        }
    }

    pub fn prefixlen(&self) -> u32 {
        self.prefixlen
    }

    pub fn network_address(&self) -> IpAddress {
        IpAddress {
            v6: self.v6,
            value: self.network,
        }
    }

    pub fn netmask(&self) -> IpAddress {
        IpAddress {
            v6: self.v6,
            value: mask_for(self.v6, self.prefixlen),
        }
    }

    /// `num_addresses`; saturates for a whole IPv6 `/0`.
    pub fn num_addresses(&self) -> u128 {
        let hostbits = max_prefixlen(self.v6) - self.prefixlen;
        1u128.checked_shl(hostbits).unwrap_or(u128::MAX)
    }
}

impl std::fmt::Display for IpNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.network_address(), self.prefixlen)
    }
}

/// createnet's `_make_ipaddr`: the network, or `None` for text that
/// isn't one.
pub fn make_ipaddr(addrstr: &str) -> Option<IpNetwork> {
    IpNetwork::parse(addrstr).ok()
}

// ---------------------------------------------------------------------
// The XML model (read side)
// ---------------------------------------------------------------------

/// `_NetworkDHCPRange`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "range")]
pub struct NetworkDhcpRange {
    #[xml(attribute = "start")]
    pub start: Option<String>,
    #[xml(attribute = "end")]
    pub end: Option<String>,
}

/// `_NetworkDHCPHost`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "host")]
pub struct NetworkDhcpHost {
    #[xml(attribute = "mac")]
    pub macaddr: Option<String>,
    #[xml(attribute = "name")]
    pub name: Option<String>,
    #[xml(attribute = "ip")]
    pub ip: Option<String>,
}

/// `_NetworkIP`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "ip")]
pub struct NetworkIp {
    #[xml(attribute = "family")]
    pub family: Option<String>,
    #[xml(attribute = "address")]
    pub address: Option<String>,
    #[xml(attribute = "prefix")]
    pub prefix: Option<u32>,
    #[xml(attribute = "netmask")]
    pub netmask: Option<String>,
    #[xml(path = "tftp", attribute = "root")]
    pub tftp: Option<String>,
    #[xml(path = "dhcp/bootp", attribute = "file")]
    pub bootp_file: Option<String>,
    #[xml(path = "dhcp/bootp", attribute = "server")]
    pub bootp_server: Option<String>,
    #[xml(path = "dhcp", list)]
    pub ranges: Vec<NetworkDhcpRange>,
    #[xml(path = "dhcp", list)]
    pub hosts: Vec<NetworkDhcpHost>,
}

/// `_NetworkRoute`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "route")]
pub struct NetworkRoute {
    #[xml(attribute = "family")]
    pub family: Option<String>,
    #[xml(attribute = "address")]
    pub address: Option<String>,
    #[xml(attribute = "prefix")]
    pub prefix: Option<u32>,
    #[xml(attribute = "gateway")]
    pub gateway: Option<String>,
    #[xml(attribute = "netmask")]
    pub netmask: Option<String>,
}

/// `_NetworkForwardPf`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "pf")]
pub struct NetworkForwardPf {
    #[xml(attribute = "dev")]
    pub dev: Option<String>,
}

/// `_NetworkForward`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "forward")]
pub struct NetworkForward {
    #[xml(attribute = "mode")]
    pub mode: Option<String>,
    #[xml(attribute = "dev")]
    pub dev: Option<String>,
    #[xml(attribute = "managed")]
    pub managed: Option<String>,
    #[xml(list)]
    pub pf: Vec<NetworkForwardPf>,
}

/// `_NetworkPortgroup`.
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "portgroup")]
pub struct NetworkPortgroup {
    #[xml(attribute = "name")]
    pub name: Option<String>,
    #[xml(attribute = "default")]
    pub default: Option<bool>,
}

/// `Network`: the top level `<network>`.
///
/// ```xml
/// <network>
///   <name>default</name>
///   <uuid>dd8fe884-6c02-601e-7551-cca97df1c5df</uuid>
///   <forward mode='nat'/>
///   <bridge name='virbr0' stp='on' delay='0'/>
///   <ip address='192.168.122.1' netmask='255.255.255.0'>
///     <dhcp>
///       <range start='192.168.122.2' end='192.168.122.254'/>
///     </dhcp>
///   </ip>
/// </network>
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, XmlBound)]
#[xml(tag = "network")]
pub struct Network {
    #[xml(attribute = "ipv6")]
    pub ipv6: Option<bool>,
    #[xml(path = "name", text)]
    pub name: Option<String>,
    #[xml(path = "uuid", text)]
    pub uuid: Option<String>,
    #[xml(path = "virtualport", attribute = "type")]
    pub virtualport_type: Option<String>,
    /// Not entirely correct, there can be multiple routes
    #[xml(nested)]
    pub forward: NetworkForward,
    #[xml(path = "domain", attribute = "name")]
    pub domain_name: Option<String>,
    #[xml(path = "bridge", attribute = "name")]
    pub bridge: Option<String>,
    #[xml(path = "bridge", attribute = "stp")]
    pub stp: Option<OnOff>,
    #[xml(path = "bridge", attribute = "delay")]
    pub delay: Option<u32>,
    #[xml(path = "mac", attribute = "address")]
    pub macaddr: Option<String>,
    #[xml(list)]
    pub portgroups: Vec<NetworkPortgroup>,
    #[xml(list)]
    pub ips: Vec<NetworkIp>,
    #[xml(list)]
    pub routes: Vec<NetworkRoute>,
}

/// `vmmNetwork._get_network`'s summary of one address family: the
/// network as `addr/prefix`, and the first DHCP range, normalized.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkSummary {
    pub network: Option<String>,
    pub dhcp: Option<(String, String)>,
}

/// `_make_addr_str`.
fn make_addr_str(addr: &str, prefix: Option<u32>, netmask: Option<&str>) -> Result<String, String> {
    if let Some(prefix) = prefix.filter(|p| *p != 0) {
        return IpNetwork::parse(&format!("{addr}/{prefix}")).map(|n| n.to_string());
    }
    if let Some(netmask) = netmask.filter(|m| !m.is_empty()) {
        let netmask = IpAddress::parse(netmask)?;
        let network = IpAddress::parse(addr)?;
        return IpNetwork::parse(&format!("{network}/{netmask}")).map(|n| n.to_string());
    }
    IpNetwork::parse(addr).map(|n| n.to_string())
}

impl Network {
    pub fn parse(xml: &str) -> Result<Network, String> {
        let doc = virtinst_xml::parse_libvirt_xml(xml).map_err(|e| e.to_string())?;
        let root = doc.root_element().ok_or("XML has no root element")?;
        Ok(Network::from_element(&doc, root))
    }

    /// `can_pxe`.
    pub fn can_pxe(&self) -> bool {
        if let Some(forward) = self.forward.mode.as_deref() {
            if !forward.is_empty() && forward != "nat" {
                return true;
            }
        }
        self.ips.iter().any(|ip| ip.bootp_file.is_some())
    }

    /// `vmmNetwork._get_network(family)`.
    fn get_network(&self, family: &str) -> Result<NetworkSummary, String> {
        let matches = |ip: &&NetworkIp| {
            let ipfamily = ip.family.as_deref().filter(|f| !f.is_empty());
            ipfamily == Some(family) || (family == "ipv4" && ipfamily.is_none())
        };
        let mut dhcp = (None, None);
        let mut ip = None;
        for i in self.ips.iter().filter(matches) {
            if let Some(range) = i.ranges.first() {
                ip = Some(i);
                dhcp = (range.start.clone(), range.end.clone());
                break;
            }
        }
        if ip.is_none() {
            ip = self.ips.iter().find(matches);
        }

        let network = match ip {
            Some(ip) => Some(make_addr_str(
                ip.address.as_deref().unwrap_or_default(),
                ip.prefix,
                ip.netmask.as_deref(),
            )?),
            None => None,
        };
        let dhcp = match dhcp {
            (Some(start), Some(end)) if !start.is_empty() && !end.is_empty() => Some((
                IpAddress::parse(&start)?.to_string(),
                IpAddress::parse(&end)?.to_string(),
            )),
            _ => None,
        };
        Ok(NetworkSummary { network, dhcp })
    }

    /// `get_ipv4_network`.
    pub fn get_ipv4_network(&self) -> Result<NetworkSummary, String> {
        self.get_network("ipv4")
    }

    /// `get_ipv6_network`.
    pub fn get_ipv6_network(&self) -> Result<NetworkSummary, String> {
        self.get_network("ipv6")
    }

    /// `get_ipv6_enabled`.
    pub fn get_ipv6_enabled(&self) -> bool {
        self.ipv6 == Some(true)
    }

    /// `vmmNetwork.pretty_forward_mode`.
    pub fn pretty_forward_mode(&self) -> String {
        let dev = self.forward.dev.as_deref().filter(|d| !d.is_empty());
        let mode = self.forward.mode.as_deref().filter(|m| !m.is_empty());
        match (mode, dev) {
            (None, _) => "Isolated network".into(),
            (Some("nat"), Some(dev)) => format!("NAT to {dev}"),
            (Some("nat"), None) => "NAT".into(),
            (Some("route"), Some(dev)) => format!("Route to {dev}"),
            (Some("route"), None) => "Routed network".into(),
            (Some(mode), _) => format!("{} network", capitalize(mode)),
        }
    }
}

/// Python's `str.capitalize`.
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f
            .to_uppercase()
            .chain(c.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------
// The XMLBuilder classes (build side)
// ---------------------------------------------------------------------

/// `_NetworkDHCPRange`.
pub static NETWORK_DHCP_RANGE: XmlClass = XmlClass {
    xml_name: "range",
    props: &[
        prop("start", "./@start", Kind::Str),
        prop("end", "./@end", Kind::Str),
    ],
    children: &[],
    prop_order: &[],
};

/// `_NetworkDHCPHost`.
pub static NETWORK_DHCP_HOST: XmlClass = XmlClass {
    xml_name: "host",
    props: &[
        prop("macaddr", "./@mac", Kind::Str),
        prop("name", "./@name", Kind::Str),
        prop("ip", "./@ip", Kind::Str),
    ],
    children: &[],
    prop_order: &[],
};

/// `_NetworkIP`.
pub static NETWORK_IP: XmlClass = XmlClass {
    xml_name: "ip",
    props: &[
        prop("family", "./@family", Kind::Str),
        prop("address", "./@address", Kind::Str),
        prop("prefix", "./@prefix", Kind::Int),
        prop("netmask", "./@netmask", Kind::Str),
        prop("tftp", "./tftp/@root", Kind::Str),
        prop("bootp_file", "./dhcp/bootp/@file", Kind::Str),
        prop("bootp_server", "./dhcp/bootp/@server", Kind::Str),
    ],
    children: &[
        child("ranges", &NETWORK_DHCP_RANGE, "./dhcp", false),
        child("hosts", &NETWORK_DHCP_HOST, "./dhcp", false),
    ],
    prop_order: &[],
};

/// `_NetworkRoute`.
pub static NETWORK_ROUTE: XmlClass = XmlClass {
    xml_name: "route",
    props: &[
        prop("family", "./@family", Kind::Str),
        prop("address", "./@address", Kind::Str),
        prop("prefix", "./@prefix", Kind::Int),
        prop("gateway", "./@gateway", Kind::Str),
        prop("netmask", "./@netmask", Kind::Str),
    ],
    children: &[],
    prop_order: &[],
};

/// `_NetworkForwardPf`.
pub static NETWORK_FORWARD_PF: XmlClass = XmlClass {
    xml_name: "pf",
    props: &[prop("dev", "./@dev", Kind::Str)],
    children: &[],
    prop_order: &[],
};

/// `_NetworkForward`.
pub static NETWORK_FORWARD: XmlClass = XmlClass {
    xml_name: "forward",
    props: &[
        prop("mode", "./@mode", Kind::Str),
        prop("dev", "./@dev", Kind::Str),
        prop("managed", "./@managed", Kind::Str),
    ],
    children: &[child("pf", &NETWORK_FORWARD_PF, ".", false)],
    prop_order: &[],
};

/// `_NetworkPortgroup`.
pub static NETWORK_PORTGROUP: XmlClass = XmlClass {
    xml_name: "portgroup",
    props: &[
        prop("name", "./@name", Kind::Str),
        prop("default", "./@default", Kind::YesNo),
    ],
    children: &[],
    prop_order: &[],
};

/// `Network`.
pub static NETWORK: XmlClass = XmlClass {
    xml_name: "network",
    props: &[
        prop("ipv6", "./@ipv6", Kind::YesNo),
        prop("name", "./name", Kind::Str),
        prop("uuid", "./uuid", Kind::Str),
        prop("virtualport_type", "./virtualport/@type", Kind::Str),
        prop("domain_name", "./domain/@name", Kind::Str),
        prop("bridge", "./bridge/@name", Kind::Str),
        prop("stp", "./bridge/@stp", Kind::OnOff),
        prop("delay", "./bridge/@delay", Kind::Int),
        prop("macaddr", "./mac/@address", Kind::Str),
    ],
    children: &[
        child("forward", &NETWORK_FORWARD, ".", true),
        child("portgroups", &NETWORK_PORTGROUP, ".", false),
        child("ips", &NETWORK_IP, ".", false),
        child("routes", &NETWORK_ROUTE, ".", false),
    ],
    prop_order: &[
        "ipv6",
        "name",
        "uuid",
        "forward",
        "virtualport_type",
        "bridge",
        "stp",
        "delay",
        "domain_name",
        "macaddr",
        "ips",
        "routes",
    ],
};

/// `Network(conn, parsexml=xml).get_xml()`: the XML as virtinst
/// re-serializes it, for the XML editor (`get_xml_to_define`).
pub fn get_xml_to_define(xml: &str) -> Result<String, String> {
    XmlObject::parse(&NETWORK, xml).map(|o| o.get_xml())
}

// ---------------------------------------------------------------------
// createnet.py
// ---------------------------------------------------------------------

/// `_change_ipv4_network_cb`: the DHCP start and end createnet fills in
/// for a new IPv4 network, halfway through it up to its last host.
/// `None` for a non-IPv4 network; the end is `Err` where Python's
/// address arithmetic would raise (a `/32` at 0.0.0.0), after the start
/// was already set.
pub fn dhcpv4_defaults(ip: &IpNetwork) -> Option<(String, Result<String, String>)> {
    if ip.version() != 4 {
        return None;
    }
    let start = (ip.num_addresses() / 2) as i128;
    let end = ip.num_addresses() as i128 - 2;
    let net = ip.network_address();
    let start = net.checked_add(start).ok()?.to_string();
    Some((start, net.checked_add(end).map(|a| a.to_string())))
}

/// `_change_ipv6_network_cb`: `network + 256` to `network + 511`.
pub fn dhcpv6_defaults(ip: &IpNetwork) -> Option<(String, Result<String, String>)> {
    if ip.version() != 6 {
        return None;
    }
    let net = ip.network_address();
    let start = net.checked_add(256).ok()?.to_string();
    Some((start, net.checked_add(512 - 1).map(|a| a.to_string())))
}

/// The forward modes createnet offers (`net-forward-mode`): `[mode,
/// label]`.
pub const FORWARD_MODES: &[(&str, &str)] = &[
    ("nat", "NAT"),
    ("route", "Routed"),
    ("open", "Open"),
    ("isolated", "Isolated"),
    ("hostdev", "SR-IOV pool"),
];

/// What `_build_xmlobj_from_ui` reads off the dialog, already through
/// its `get_config_*` helpers: an address is `None` where the widget is
/// hidden or insensitive, or its text isn't a network.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkConfig {
    pub name: String,
    /// `get_config_domain_name`.
    pub domain_name: Option<String>,
    /// `net-forward-mode`'s value.
    pub forward_mode: String,
    /// `_get_config_net_forward_dev`.
    pub forward_dev: Option<String>,
    /// `net-hostdevs`' value, for `hostdev`.
    pub hostdev: Option<String>,
    pub ipv4_enable: bool,
    pub ip4: Option<IpNetwork>,
    pub dhcpv4_enable: bool,
    pub dhcpv4_start: Option<IpNetwork>,
    pub dhcpv4_end: Option<IpNetwork>,
    pub ipv6_enable: bool,
    pub ip6: Option<IpNetwork>,
    pub dhcpv6_enable: bool,
    pub dhcpv6_start: Option<IpNetwork>,
    pub dhcpv6_end: Option<IpNetwork>,
}

fn sval(s: impl Into<String>) -> Option<crate::xmlbuilder::Value> {
    Some(crate::xmlbuilder::Value::Str(s.into()))
}

/// The `ipobj.ranges.add_new()` of `_build_xmlobj_from_ui`.
fn add_dhcp_range(
    ipobj: &mut XmlObject,
    start: Option<&IpNetwork>,
    end: Option<&IpNetwork>,
) -> Result<(), String> {
    let mut dhcpobj = XmlObject::new(&NETWORK_DHCP_RANGE);
    if let Some(start) = start {
        dhcpobj.set("start", sval(start.network_address().to_string()))?;
    }
    if let Some(end) = end {
        dhcpobj.set("end", sval(end.network_address().to_string()))?;
    }
    ipobj.add_child(dhcpobj);
    Ok(())
}

/// `_build_xmlobj_from_ui`. An `Err` is what createnet shows as "Error
/// building XML: ...".
pub fn build_network(cfg: &NetworkConfig) -> Result<XmlObject, String> {
    let mut net = XmlObject::new(&NETWORK);
    net.set("name", sval(cfg.name.clone()))?;
    net.set("domain_name", cfg.domain_name.clone().and_then(sval))?;

    let mode = (cfg.forward_mode != "isolated").then(|| cfg.forward_mode.clone());
    let forward = net.child_mut("forward");
    forward.set("mode", mode.clone().and_then(sval))?;
    forward.set("dev", cfg.forward_dev.clone().and_then(sval))?;

    if mode.as_deref() == Some("hostdev") {
        forward.set("managed", sval("yes"))?;
        let mut pfobj = XmlObject::new(&NETWORK_FORWARD_PF);
        pfobj.set("dev", cfg.hostdev.clone().and_then(sval))?;
        forward.add_child(pfobj);
        return Ok(net);
    }

    if cfg.ipv4_enable {
        let mut ipobj = XmlObject::new(&NETWORK_IP);
        if let Some(ip) = &cfg.ip4 {
            ipobj.set(
                "address",
                sval(ip.network_address().checked_add(1)?.to_string()),
            )?;
            ipobj.set("netmask", sval(ip.netmask().to_string()))?;
        }
        if cfg.dhcpv4_enable {
            add_dhcp_range(
                &mut ipobj,
                cfg.dhcpv4_start.as_ref(),
                cfg.dhcpv4_end.as_ref(),
            )?;
        }
        net.add_child(ipobj);
    }

    if cfg.ipv6_enable {
        let mut ipobj = XmlObject::new(&NETWORK_IP);
        ipobj.set("family", sval("ipv6"))?;
        if let Some(ip) = &cfg.ip6 {
            ipobj.set(
                "address",
                sval(ip.network_address().checked_add(1)?.to_string()),
            )?;
            ipobj.set("prefix", sval(ip.prefixlen().to_string()))?;
        }
        if cfg.dhcpv6_enable {
            add_dhcp_range(
                &mut ipobj,
                cfg.dhcpv6_start.as_ref(),
                cfg.dhcpv6_end.as_ref(),
            )?;
        }
        net.add_child(ipobj);
    }
    Ok(net)
}

/// `Network(conn, parsexml=xml)` for the XML editor's text, as
/// `_build_xmlobj_from_xmleditor` takes it.
pub fn parse_network(xml: &str) -> Result<XmlObject, String> {
    XmlObject::parse(&NETWORK, xml)
}

/// `_validate`: a usable name no other network has.
pub fn validate(conn: &Connect, name: &str) -> Result<(), String> {
    crate::storage::validate_generic_name("Network", name)?;
    if VirNetwork::lookup_by_name(conn, name).is_err() {
        return Ok(());
    }
    Err(format!("Name '{name}' already in use by another network."))
}

/// `reset_state`'s `generatename.generate_name("network", ...)`: a name
/// no network has yet.
pub fn find_free_name(conn: &Connect, basename: &str) -> String {
    crate::storage::generate_name(basename, "", |n| {
        VirNetwork::lookup_by_name(conn, n).is_ok()
    })
}

/// `_async_net_create`: define, start and autostart the network, and
/// undefine it again if starting fails.
pub fn create(conn: &Connect, xml: &str) -> Result<(), String> {
    let msg = |e: virt::error::Error| e.message().to_string();
    let netobj = VirNetwork::define_xml(conn, xml).map_err(msg)?;
    let res = netobj
        .create()
        .map(|_| ())
        .and_then(|_| netobj.set_autostart(true).map(|_| ()))
        .map_err(msg);
    if res.is_err() {
        let _ = netobj.undefine();
    }
    res
}

/// `vmmLibvirtObject.define_name` + `vmmConnection.rename_object` for a
/// network: undefine it, define it again under the new name (putting
/// the original back if that fails), and keep its autostart.
pub fn rename(conn: &Connect, net: &VirNetwork, newname: &str) -> Result<(), String> {
    let msg = |e: virt::error::Error| e.message().to_string();
    let oldautostart = net.get_autostart().map_err(msg)?;
    let mut xmlobj = parse_network(&net.get_xml_desc(0).map_err(msg)?)?;
    if xmlobj.get_str("name").as_deref() == Some(newname) {
        return Ok(());
    }
    let origxml = xmlobj.get_xml();
    xmlobj.set("name", sval(newname))?;
    let newxml = xmlobj.get_xml();

    net.undefine().map_err(msg)?;
    let newobj = match VirNetwork::define_xml(conn, &newxml) {
        Ok(n) => n,
        Err(renameerr) => {
            return match VirNetwork::define_xml(conn, &origxml) {
                Ok(_) => Err(renameerr.message().to_string()),
                Err(fixerr) => Err(format!(
                    "network rename failed. Attempting to recover also failed.\n\n\
                     Original error: {}\n\nRecover error: {}",
                    renameerr.message(),
                    fixerr.message()
                )),
            };
        }
    };
    newobj.set_autostart(oldautostart).map_err(msg)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(s: &str) -> IpNetwork {
        IpNetwork::parse(s).unwrap()
    }

    #[test]
    fn ip_network_parses_like_python() {
        assert_eq!(net("192.168.100.0/24").to_string(), "192.168.100.0/24");
        // strict=False masks the host bits
        assert_eq!(net("192.168.100.7/24").to_string(), "192.168.100.0/24");
        assert_eq!(net("192.168.100.128").to_string(), "192.168.100.128/32");
        // netmask and hostmask forms
        assert_eq!(net("10.1.2.3/255.255.0.0").to_string(), "10.1.0.0/16");
        assert_eq!(net("10.1.2.3/0.0.255.255").to_string(), "10.1.0.0/16");
        assert_eq!(net("fd00::5/64").to_string(), "fd00::/64");
        assert_eq!(net("::").to_string(), "::/128");
        assert_eq!(net("0.0.0.0/0").num_addresses(), 1 << 32);
        assert_eq!(
            net("192.168.100.0/24").netmask().to_string(),
            "255.255.255.0"
        );
        for bad in [
            "",
            "foo",
            "192.168.100.0/33",
            "192.168.1.0/24/1",
            "192.168.001.0/24",
            "192.168.1.0/ 24",
            "10.0.0.0/255.0.255.0",
            "fd00::/129",
            "fd00::/ffff::",
        ] {
            assert!(IpNetwork::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(
            IpNetwork::parse("foo").unwrap_err(),
            "'foo' does not appear to be an IPv4 or IPv6 network"
        );
        assert_eq!(make_ipaddr("nope"), None);
    }

    #[test]
    fn ipv6_formatting_compresses_like_python() {
        let a = |s: &str| IpAddress::parse(s).unwrap().to_string();
        assert_eq!(a("fd00:0:0:0:0:0:0:100"), "fd00::100");
        assert_eq!(a("2001:db8:0:0:1:0:0:1"), "2001:db8::1:0:0:1");
        assert_eq!(a("2001:0:0:1:0:0:0:1"), "2001:0:0:1::1");
        assert_eq!(a("1:0:2:3:4:5:6:7"), "1:0:2:3:4:5:6:7");
        assert_eq!(a("::1"), "::1");
        assert_eq!(a("fe80::"), "fe80::");
        assert_eq!(a("0:0:0:0:0:0:0:0"), "::");
    }

    #[test]
    fn address_arithmetic_errors_like_python() {
        let a = IpAddress::parse("0.0.0.0").unwrap();
        assert_eq!(
            a.checked_add(-1).unwrap_err(),
            "-1 (< 0) is not permitted as an IPv4 address"
        );
        let a = IpAddress::parse("255.255.255.255").unwrap();
        assert_eq!(
            a.checked_add(1).unwrap_err(),
            "4294967296 (>= 2**32) is not permitted as an IPv4 address"
        );
        assert_eq!(
            IpAddress::parse("10.0.0.255")
                .unwrap()
                .checked_add(1)
                .unwrap()
                .to_string(),
            "10.0.1.0"
        );
    }

    #[test]
    fn dhcp_defaults_follow_createnet() {
        let (start, end) = dhcpv4_defaults(&net("192.168.100.0/24")).unwrap();
        assert_eq!(start, "192.168.100.128");
        assert_eq!(end.unwrap(), "192.168.100.254");
        let (start, end) = dhcpv4_defaults(&net("10.0.0.0/8")).unwrap();
        assert_eq!(start, "10.128.0.0");
        assert_eq!(end.unwrap(), "10.255.255.254");
        let (start, end) = dhcpv4_defaults(&net("192.168.1.5/32")).unwrap();
        assert_eq!(start, "192.168.1.5");
        assert_eq!(end.unwrap(), "192.168.1.4");
        let (start, end) = dhcpv4_defaults(&net("0.0.0.0/32")).unwrap();
        assert_eq!(start, "0.0.0.0");
        assert!(end.is_err());
        assert!(dhcpv4_defaults(&net("fd00::/64")).is_none());

        let (start, end) = dhcpv6_defaults(&net("fd00::/64")).unwrap();
        assert_eq!(start, "fd00::100");
        assert_eq!(end.unwrap(), "fd00::1ff");
        assert!(dhcpv6_defaults(&net("192.168.100.0/24")).is_none());
    }

    /// createnet's defaults: `reset_state`'s values through
    /// `_build_xmlobj_from_ui`, checked against upstream's own output.
    fn default_config() -> NetworkConfig {
        NetworkConfig {
            name: "network".into(),
            domain_name: Some("network".into()),
            forward_mode: "nat".into(),
            ipv4_enable: true,
            ip4: make_ipaddr("192.168.100.0/24"),
            dhcpv4_enable: true,
            dhcpv4_start: make_ipaddr("192.168.100.128"),
            dhcpv4_end: make_ipaddr("192.168.100.254"),
            ..Default::default()
        }
    }

    #[test]
    fn build_default_network_matches_upstream() {
        let xml = build_network(&default_config()).unwrap().get_xml();
        assert_eq!(
            xml,
            "<network>\n  <name>network</name>\n  <forward mode=\"nat\"/>\n  \
             <domain name=\"network\"/>\n  \
             <ip address=\"192.168.100.1\" netmask=\"255.255.255.0\">\n    <dhcp>\n      \
             <range start=\"192.168.100.128\" end=\"192.168.100.254\"/>\n    </dhcp>\n  \
             </ip>\n</network>\n"
        );
    }

    #[test]
    fn build_ipv6_and_modes_match_upstream() {
        let mut cfg = default_config();
        cfg.ipv6_enable = true;
        cfg.ip6 = make_ipaddr("fd00::/64");
        cfg.dhcpv6_enable = true;
        cfg.dhcpv6_start = make_ipaddr("fd00::100");
        cfg.dhcpv6_end = make_ipaddr("fd00::1ff");
        let xml = build_network(&cfg).unwrap().get_xml();
        assert!(
            xml.contains(
                "  <ip family=\"ipv6\" address=\"fd00::1\" prefix=\"64\">\n    <dhcp>\n      \
                 <range start=\"fd00::100\" end=\"fd00::1ff\"/>\n    </dhcp>\n  </ip>\n</network>"
            ),
            "{xml}"
        );

        // `hostdev`: the pf device and nothing else, no addresses
        let cfg = NetworkConfig {
            name: "hd".into(),
            forward_mode: "hostdev".into(),
            hostdev: Some("eth3".into()),
            ipv4_enable: true,
            ip4: make_ipaddr("192.168.100.0/24"),
            ..Default::default()
        };
        assert_eq!(
            build_network(&cfg).unwrap().get_xml(),
            "<network>\n  <name>hd</name>\n  <forward mode=\"hostdev\" managed=\"yes\">\n    \
             <pf dev=\"eth3\"/>\n  </forward>\n</network>\n"
        );
        // ... and with no device to pick, no `<pf>` at all
        let cfg = NetworkConfig {
            hostdev: None,
            ..cfg
        };
        assert_eq!(
            build_network(&cfg).unwrap().get_xml(),
            "<network>\n  <name>hd</name>\n  <forward mode=\"hostdev\" managed=\"yes\"/>\n</network>\n"
        );

        // `isolated` is no forward mode; `route` takes the device
        let cfg = NetworkConfig {
            name: "iso".into(),
            forward_mode: "isolated".into(),
            ..Default::default()
        };
        assert_eq!(
            build_network(&cfg).unwrap().get_xml(),
            "<network>\n  <name>iso</name>\n</network>\n"
        );
        let cfg = NetworkConfig {
            name: "rt".into(),
            forward_mode: "route".into(),
            forward_dev: Some("eth0".into()),
            ipv4_enable: true,
            ..Default::default()
        };
        assert_eq!(
            build_network(&cfg).unwrap().get_xml(),
            "<network>\n  <name>rt</name>\n  <forward mode=\"route\" dev=\"eth0\"/>\n</network>\n"
        );
    }

    #[test]
    fn build_reports_address_overflow() {
        let cfg = NetworkConfig {
            name: "x".into(),
            forward_mode: "nat".into(),
            ipv4_enable: true,
            ip4: make_ipaddr("255.255.255.255/32"),
            ..Default::default()
        };
        assert_eq!(
            build_network(&cfg).unwrap_err(),
            "4294967296 (>= 2**32) is not permitted as an IPv4 address"
        );
    }

    const DEFAULT_XML: &str = "<network>
  <name>default</name>
  <uuid>dd8fe884-6c02-601e-7551-cca97df1c5df</uuid>
  <forward mode='nat'/>
  <bridge name='virbr0' stp='on' delay='0'/>
  <ip address='192.168.122.1' netmask='255.255.255.0'>
    <dhcp>
      <range start='192.168.122.2' end='192.168.122.254'/>
    </dhcp>
  </ip>
</network>";

    #[test]
    fn parse_reads_the_test_driver_default_network() {
        let n = Network::parse(DEFAULT_XML).unwrap();
        assert_eq!(n.name.as_deref(), Some("default"));
        assert_eq!(n.bridge.as_deref(), Some("virbr0"));
        assert_eq!(n.stp, Some(OnOff(true)));
        assert_eq!(n.delay, Some(0));
        assert_eq!(n.forward.mode.as_deref(), Some("nat"));
        assert_eq!(n.pretty_forward_mode(), "NAT");
        assert!(!n.can_pxe());
        assert_eq!(
            n.get_ipv4_network().unwrap(),
            NetworkSummary {
                network: Some("192.168.122.0/24".into()),
                dhcp: Some(("192.168.122.2".into(), "192.168.122.254".into())),
            }
        );
        assert_eq!(n.get_ipv6_network().unwrap(), NetworkSummary::default());
        assert!(!n.get_ipv6_enabled());
    }

    #[test]
    fn network_summaries_follow_vmmnetwork() {
        let n = Network::parse(
            "<network ipv6='yes'><name>n</name><domain name='example'/>\
             <forward mode='route' dev='eth1'/>\
             <ip address='10.0.0.1' prefix='8'/>\
             <ip family='ipv6' address='fd00::1' prefix='64'/>\
             <ip family='ipv6' address='fd01::1' prefix='64'>\
             <dhcp><range start='fd01:0::100' end='fd01::1ff'/></dhcp></ip>\
             <ip address='192.168.5.9'><tftp root='/srv'/><dhcp><bootp file='pxe'/></dhcp></ip>\
             </network>",
        )
        .unwrap();
        assert_eq!(n.domain_name.as_deref(), Some("example"));
        assert_eq!(n.pretty_forward_mode(), "Route to eth1");
        assert!(n.get_ipv6_enabled());
        assert!(n.can_pxe());
        assert_eq!(
            n.get_ipv4_network().unwrap().network.as_deref(),
            Some("10.0.0.0/8")
        );
        // The first ipv6 `<ip>` with a DHCP range wins
        assert_eq!(
            n.get_ipv6_network().unwrap(),
            NetworkSummary {
                network: Some("fd01::/64".into()),
                dhcp: Some(("fd01::100".into(), "fd01::1ff".into())),
            }
        );
        assert_eq!(n.ips[3].tftp.as_deref(), Some("/srv"));
        assert_eq!(n.ips[3].bootp_file.as_deref(), Some("pxe"));

        let iso = Network::parse("<network><name>i</name></network>").unwrap();
        assert_eq!(iso.pretty_forward_mode(), "Isolated network");
        let open = Network::parse("<network><forward mode='open'/></network>").unwrap();
        assert_eq!(open.pretty_forward_mode(), "Open network");
        let nat = Network::parse("<network><forward mode='nat' dev='eth0'/></network>").unwrap();
        assert_eq!(nat.pretty_forward_mode(), "NAT to eth0");
        let rt = Network::parse("<network><forward mode='route'/></network>").unwrap();
        assert_eq!(rt.pretty_forward_mode(), "Routed network");

        let bad = Network::parse("<network><ip address='nope' prefix='24'/></network>").unwrap();
        assert!(bad.get_ipv4_network().is_err());
    }

    #[test]
    fn xml_to_define_reserializes_like_virtinst() {
        let xml = get_xml_to_define(DEFAULT_XML).unwrap();
        assert!(xml.contains("<forward mode=\"nat\"/>"), "{xml}");
        assert!(
            xml.starts_with("<network>\n  <name>default</name>"),
            "{xml}"
        );
    }

    /// Define/create/rename/undefine against the test driver, whose
    /// state is process-wide: unique names, and the crate's lock.
    #[test]
    fn create_rename_and_validate_against_the_test_driver() {
        let _lock = crate::test_support::TEST_DOMAIN_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut conn = Connect::open(Some("test:///default")).unwrap();
        assert_eq!(
            validate(&conn, "default").unwrap_err(),
            "Name 'default' already in use by another network."
        );
        assert_eq!(
            validate(&conn, "").unwrap_err(),
            "A name must be specified for the Network"
        );
        assert_eq!(
            validate(&conn, "a/b").unwrap_err(),
            "Network name 'a/b' can not contain '/' character."
        );

        let name = find_free_name(&conn, "rusttestnet");
        assert!(validate(&conn, &name).is_ok());
        let mut cfg = default_config();
        cfg.name = name.clone();
        cfg.ip4 = make_ipaddr("192.168.231.0/24");
        cfg.dhcpv4_start = make_ipaddr("192.168.231.128");
        cfg.dhcpv4_end = make_ipaddr("192.168.231.254");
        create(&conn, &build_network(&cfg).unwrap().get_xml()).unwrap();
        let n = VirNetwork::lookup_by_name(&conn, &name).unwrap();
        assert!(n.is_active().unwrap());
        assert!(n.get_autostart().unwrap());
        assert_ne!(find_free_name(&conn, &name), name);

        n.destroy().unwrap();
        let newname = format!("{name}-renamed");
        rename(&conn, &n, &newname).unwrap();
        assert!(VirNetwork::lookup_by_name(&conn, &name).is_err());
        let renamed = VirNetwork::lookup_by_name(&conn, &newname).unwrap();
        assert!(renamed.get_autostart().unwrap());
        renamed.undefine().unwrap();
        let _ = conn.close();
    }
}
