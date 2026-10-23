//! Upstream's `virtManager/lib/statsmanager.py`: per-VM CPU, memory,
//! disk and network sampling, the rate math, and the history the
//! sparklines draw.
//!
//! Split for ticket 09's threading model: [`sample`] does the blocking
//! libvirt calls on the tick's worker thread and returns raw counters;
//! [`StatsList::append_sample`] turns them into a record on the GTK
//! thread, using the previous record for the deltas — the same math as
//! upstream's `_sample_*_stats` + `append_stats`.
//!
//! This uses upstream's per-call path (`info()`, `blockStats()`,
//! `interfaceStats()`, `memoryStats()`), which upstream itself uses for
//! the test driver and whenever `getAllDomainStats()` is unsupported. A
//! device whose stats call fails just contributes 0 for that tick;
//! upstream additionally remembers it in a skip list.

use std::collections::VecDeque;

use virt::domain::Domain;
use virt::sys;
use virtinst_core::Guest;
use virtinst_xml::XmlBound;

/// `config.get_stats_history_length()`.
pub const STATS_HISTORY_LENGTH: usize = 120;

/// Which pollers are enabled (the `stats/enable-*-poll` settings).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PollFlags {
    pub cpu: bool,
    pub mem: bool,
    pub disk: bool,
    pub net: bool,
}

/// One worker-thread sample of raw counters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawSample {
    pub timestamp: f64,
    pub state: sys::virDomainState,
    pub guestcpus: u32,
    pub cpu_time_abs: u64,
    /// (total, used) KiB, when memory polling ran.
    pub mem: Option<(u64, u64)>,
    pub disk_rd_bytes: i64,
    pub disk_wr_bytes: i64,
    pub net_rx_bytes: i64,
    pub net_tx_bytes: i64,
}

/// `_VMStatsRecord`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatsRecord {
    pub timestamp: f64,
    pub cpu_time: u64,
    pub cpu_time_abs: u64,
    pub cpu_host_percent: f64,
    pub cpu_guest_percent: f64,
    pub curmem: u64,
    pub curr_mem_percent: f64,
    pub disk_rd_kib: i64,
    pub disk_wr_kib: i64,
    pub net_rx_kib: i64,
    pub net_tx_kib: i64,
    pub disk_rd_rate: f64,
    pub disk_wr_rate: f64,
    pub net_rx_rate: f64,
    pub net_tx_rate: f64,
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn is_offline(state: sys::virDomainState) -> bool {
    matches!(state, sys::VIR_DOMAIN_SHUTOFF | sys::VIR_DOMAIN_CRASHED)
}

/// Blocking: one domain's raw counters. `set_mem_period` asks for the
/// one-time `setMemoryStatsPeriod` upstream does the first time memory
/// polling sees a running VM.
pub fn sample(dom: &Domain, flags: PollFlags, is_lxc: bool, set_mem_period: bool) -> RawSample {
    let mut raw = RawSample {
        timestamp: now(),
        ..Default::default()
    };
    let Ok(info) = dom.get_info() else {
        return raw;
    };
    raw.state = info.state;
    if is_offline(info.state) {
        return raw;
    }
    if flags.cpu {
        raw.guestcpus = info.nr_virt_cpu;
        raw.cpu_time_abs = info.cpu_time;
    }

    let (disks, ifaces) = if flags.disk || flags.net {
        device_targets(dom)
    } else {
        (vec![], vec![])
    };

    if flags.net {
        for dev in &ifaces {
            if let Ok(io) = dom.interface_stats(dev) {
                raw.net_rx_bytes += io.rx_bytes.max(0);
                raw.net_tx_bytes += io.tx_bytes.max(0);
            }
        }
    }

    if flags.disk {
        // LXC has a special blockStats method
        let lxc = is_lxc.then(|| dom.get_block_stats("").ok()).flatten();
        if let Some(io) = lxc {
            raw.disk_rd_bytes = io.rd_bytes.max(0);
            raw.disk_wr_bytes = io.wr_bytes.max(0);
        } else {
            for dev in &disks {
                if let Ok(io) = dom.get_block_stats(dev) {
                    raw.disk_rd_bytes += io.rd_bytes.max(0);
                    raw.disk_wr_bytes += io.wr_bytes.max(0);
                }
            }
        }
    }

    if flags.mem {
        // QEMU only reports fine-grained balloon stats once a period is
        // set; harmless (and ignored) where there's no virtio balloon.
        if set_mem_period {
            let _ = dom.set_memory_stats_period(5, sys::VIR_DOMAIN_AFFECT_LIVE);
        }
        if let Ok(stats) = dom.memory_stats(0) {
            let get = |tag| stats.iter().find(|s| s.tag == tag).map(|s| s.val);
            let total = get(sys::VIR_DOMAIN_MEMORY_STAT_ACTUAL_BALLOON).unwrap_or(1);
            let unused = get(sys::VIR_DOMAIN_MEMORY_STAT_UNUSED).unwrap_or(total);
            raw.mem = Some((total, total.saturating_sub(unused)));
        } else {
            raw.mem = Some((1, 0));
        }
    }
    raw
}

/// Disk and interface `<target dev=...>` names from the live XML.
fn device_targets(dom: &Domain) -> (Vec<String>, Vec<String>) {
    let Ok(xml) = dom.get_xml_desc(0) else {
        return (vec![], vec![]);
    };
    let Ok(doc) = virtinst_xml::parse_libvirt_xml(&xml) else {
        return (vec![], vec![]);
    };
    let Some(root) = doc.root_element() else {
        return (vec![], vec![]);
    };
    let guest = Guest::from_element(&doc, root);
    let disks = guest
        .devices
        .disks
        .iter()
        .filter_map(|d| d.target_dev.clone())
        .collect();
    let ifaces = guest
        .devices
        .interfaces
        .iter()
        .filter_map(|i| i.target_dev.clone())
        .collect();
    (disks, ifaces)
}

/// `_VMStatsList`: newest record first.
#[derive(Debug)]
pub struct StatsList {
    stats: VecDeque<StatsRecord>,
    pub disk_rd_max_rate: f64,
    pub disk_wr_max_rate: f64,
    pub net_rx_max_rate: f64,
    pub net_tx_max_rate: f64,
}

impl Default for StatsList {
    fn default() -> Self {
        StatsList {
            stats: VecDeque::new(),
            disk_rd_max_rate: 10.0,
            disk_wr_max_rate: 10.0,
            net_rx_max_rate: 10.0,
            net_tx_max_rate: 10.0,
        }
    }
}

impl StatsList {
    pub fn latest(&self) -> StatsRecord {
        self.stats.front().cloned().unwrap_or_default()
    }

    /// `refresh_vm_stats`'s record building (`_sample_cpu_stats`,
    /// `_sample_mem_stats`, …) followed by `append_stats`.
    pub fn append_sample(&mut self, raw: &RawSample, flags: PollFlags, hostcpus: u32) {
        let prev = self.latest();
        let offline = is_offline(raw.state);

        // _sample_cpu_stats
        let (mut cpu_time, mut cpu_time_abs, mut host_pct, mut guest_pct) = (0, 0, 0.0, 0.0);
        let timestamp = raw.timestamp;
        if flags.cpu && !offline {
            cpu_time_abs = raw.cpu_time_abs;
            cpu_time = cpu_time_abs.saturating_sub(prev.cpu_time_abs);
            let elapsed = timestamp - prev.timestamp;
            if elapsed > 0.0 {
                let pcentbase = (cpu_time as f64 * 100.0) / (elapsed * 1e9);
                host_pct = if hostcpus > 0 {
                    pcentbase / hostcpus as f64
                } else {
                    0.0
                };
                guest_pct = if raw.guestcpus > 0 {
                    pcentbase / raw.guestcpus as f64
                } else {
                    0.0
                };
            }
        }
        let host_pct = host_pct.clamp(0.0, 100.0);
        let guest_pct = guest_pct.clamp(0.0, 100.0);

        // _sample_mem_stats
        let (mem_pct, curmem) = match raw.mem {
            Some((total, cur)) if flags.mem && !offline => {
                let pct = cur as f64 / total.max(1) as f64 * 100.0;
                (pct.clamp(0.0, 100.0), cur)
            }
            _ => (0.0, 0),
        };

        let mut rec = StatsRecord {
            timestamp,
            cpu_time,
            cpu_time_abs,
            cpu_host_percent: host_pct,
            cpu_guest_percent: guest_pct,
            curmem,
            curr_mem_percent: mem_pct,
            disk_rd_kib: raw.disk_rd_bytes / 1024,
            disk_wr_kib: raw.disk_wr_bytes / 1024,
            net_rx_kib: raw.net_rx_bytes / 1024,
            net_tx_kib: raw.net_tx_bytes / 1024,
            ..Default::default()
        };
        self.append_stats(&mut rec);
        self.stats.push_front(rec);
    }

    /// `append_stats`' rate math and history trim.
    fn append_stats(&mut self, new: &mut StatsRecord) {
        self.stats.truncate(STATS_HISTORY_LENGTH + 1);
        let old = self.stats.front().cloned();
        let snapshot = new.clone();
        let rate = |f: fn(&StatsRecord) -> i64| match &old {
            Some(old) => {
                let timediff = snapshot.timestamp - old.timestamp;
                if timediff > 0.0 {
                    ((f(&snapshot) - f(old)) as f64 / timediff).max(0.0)
                } else {
                    0.0
                }
            }
            None => 0.0,
        };
        new.disk_rd_rate = rate(|r| r.disk_rd_kib);
        new.disk_wr_rate = rate(|r| r.disk_wr_kib);
        new.net_rx_rate = rate(|r| r.net_rx_kib);
        new.net_tx_rate = rate(|r| r.net_tx_kib);

        self.disk_rd_max_rate = self.disk_rd_max_rate.max(new.disk_rd_rate);
        self.disk_wr_max_rate = self.disk_wr_max_rate.max(new.disk_wr_rate);
        self.net_rx_max_rate = self.net_rx_max_rate.max(new.net_rx_rate);
        self.net_tx_max_rate = self.net_tx_max_rate.max(new.net_tx_rate);
    }

    /// `get_vector`: `limit` values, newest first, divided by `ceil`,
    /// zero-padded.
    pub fn get_vector(&self, f: fn(&StatsRecord) -> f64, limit: usize, ceil: f64) -> Vec<f64> {
        let len = (STATS_HISTORY_LENGTH + 1).min(limit);
        (0..len)
            .map(|i| self.stats.get(i).map(|r| f(r) / ceil).unwrap_or(0.0))
            .collect()
    }

    /// `disk_io_vectors`/`network_traffic_vectors` averaged the way the
    /// manager's sparkline does: `(in + out) / 2` per point.
    pub fn get_in_out_avg(
        &self,
        f1: fn(&StatsRecord) -> f64,
        f2: fn(&StatsRecord) -> f64,
        limit: usize,
        ceil: f64,
    ) -> Vec<f64> {
        let a = self.get_vector(f1, limit, ceil);
        let b = self.get_vector(f2, limit, ceil);
        a.iter().zip(&b).map(|(x, y)| (x + y) / 2.0).collect()
    }

    /// `disk_io_max_rate`.
    pub fn disk_io_max_rate(&self) -> f64 {
        self.disk_rd_max_rate.max(self.disk_wr_max_rate).max(10.0)
    }

    /// `network_traffic_max_rate`.
    pub fn network_traffic_max_rate(&self) -> f64 {
        self.net_rx_max_rate.max(self.net_tx_max_rate).max(10.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: PollFlags = PollFlags {
        cpu: true,
        mem: true,
        disk: true,
        net: true,
    };

    fn raw(ts: f64, cpu_ns: u64, disk_rd: i64) -> RawSample {
        RawSample {
            timestamp: ts,
            state: sys::VIR_DOMAIN_RUNNING,
            guestcpus: 2,
            cpu_time_abs: cpu_ns,
            mem: Some((1000, 250)),
            disk_rd_bytes: disk_rd,
            ..Default::default()
        }
    }

    #[test]
    fn cpu_percent_and_rates_match_upstream_math() {
        let mut list = StatsList::default();
        list.append_sample(&raw(100.0, 1_000_000_000, 0), ALL, 4);
        // 1s of CPU over 1s wall, 4 host cpus / 2 guest cpus
        list.append_sample(&raw(101.0, 2_000_000_000, 2048 * 1024), ALL, 4);
        let r = list.latest();
        assert!((r.cpu_host_percent - 25.0).abs() < 1e-9);
        assert!((r.cpu_guest_percent - 50.0).abs() < 1e-9);
        assert!((r.curr_mem_percent - 25.0).abs() < 1e-9);
        assert_eq!(r.curmem, 250);
        assert!((r.disk_rd_rate - 2048.0).abs() < 1e-9);
        assert_eq!(list.disk_io_max_rate(), 2048.0);
        assert_eq!(list.network_traffic_max_rate(), 10.0);
    }

    #[test]
    fn percentages_clamp_and_offline_zeroes() {
        let mut list = StatsList::default();
        list.append_sample(&raw(100.0, 0, 0), ALL, 1);
        list.append_sample(&raw(100.5, 10_000_000_000, 0), ALL, 1);
        assert_eq!(list.latest().cpu_host_percent, 100.0);
        let mut off = raw(101.0, 0, 0);
        off.state = sys::VIR_DOMAIN_SHUTOFF;
        list.append_sample(&off, ALL, 1);
        let r = list.latest();
        assert_eq!(r.cpu_guest_percent, 0.0);
        assert_eq!(r.curr_mem_percent, 0.0);
    }

    #[test]
    fn vectors_are_newest_first_and_padded() {
        let mut list = StatsList::default();
        list.append_sample(&raw(1.0, 0, 0), ALL, 1);
        list.append_sample(&raw(2.0, 500_000_000, 0), ALL, 1);
        let v = list.get_vector(|r| r.cpu_guest_percent, 4, 100.0);
        assert_eq!(v.len(), 4);
        assert!((v[0] - 0.25).abs() < 1e-9);
        assert_eq!(&v[1..], &[0.0, 0.0, 0.0]);
    }

    #[test]
    fn history_is_bounded() {
        let mut list = StatsList::default();
        for i in 0..300 {
            list.append_sample(&raw(i as f64, 0, 0), ALL, 1);
        }
        assert!(list.stats.len() <= STATS_HISTORY_LENGTH + 2);
    }
}
