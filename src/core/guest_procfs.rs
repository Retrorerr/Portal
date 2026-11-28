//! Stand-ins for the `/proc` files Android's app sandbox denies.
//!
//! SELinux keeps apps from reading `/proc/stat`, `/proc/uptime`,
//! `/proc/loadavg`, `/proc/vmstat`, `/proc/version` and several
//! `/proc/sys` limits. Ordinary Linux tools need them: `ps aux`
//! printed only a header and "Unable to get system boot time", `uptime`,
//! `htop`, Python's psutil and Plasma's System Monitor failed or showed
//! nothing; bubblewrap (Flatpak, Discover) stopped at "Can't read
//! /proc/sys/kernel/overflowuid". Portal binds generated files over those paths. Uptime and boot
//! time are exact. CPU time is the device's per-CPU time from the cpuidle
//! counters apps may read (`HostCpuAccount`), or else what Portal's own
//! processes used; the load average follows their runnable count.
//!
//! This module only renders the files and keeps the counters monotonic;
//! the Android side samples cpuidle and `/proc` and writes them.

use std::collections::HashMap;
use std::fmt::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Files Portal generates, relative to its fake-proc directory, with the
/// guest path each is bound over.
pub const FILES: [(&str, &str); 11] = [
    ("stat", "/proc/stat"),
    ("uptime", "/proc/uptime"),
    ("loadavg", "/proc/loadavg"),
    ("vmstat", "/proc/vmstat"),
    ("version", "/proc/version"),
    ("cap_last_cap", "/proc/sys/kernel/cap_last_cap"),
    ("overflowuid", "/proc/sys/kernel/overflowuid"),
    ("overflowgid", "/proc/sys/kernel/overflowgid"),
    ("ngroups_max", "/proc/sys/kernel/ngroups_max"),
    ("pid_max", "/proc/sys/kernel/pid_max"),
    ("max_user_namespaces", "/proc/sys/user/max_user_namespaces"),
];

pub const ROUTE_FILES: [(&str, &str); 2] = [
    ("route", "/proc/net/route"),
    ("ipv6_route", "/proc/net/ipv6_route"),
];

/// A unicast route from Android's active network.
pub struct NetworkRoute {
    pub interface: String,
    pub destination: IpAddr,
    pub prefix: u8,
    pub gateway: Option<IpAddr>,
}

/// Linux's legacy route files use little-endian hex for IPv4 addresses
/// and network-order hex for IPv6 addresses.
pub fn render_network_routes(routes: &[NetworkRoute]) -> (String, String) {
    let mut v4 = String::from("Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n");
    let mut v6 = String::new();
    for route in routes {
        let iface = &route.interface;
        if iface.is_empty() || iface.chars().any(char::is_whitespace) {
            continue;
        }
        let gateway = route.gateway.filter(|address| !address.is_unspecified());
        let flags = if gateway.is_some() { 3 } else { 1 }; // RTF_UP | RTF_GATEWAY
        match (route.destination, gateway) {
            (IpAddr::V4(dst), gateway) if route.prefix <= 32 => {
                let gateway = match gateway {
                    Some(IpAddr::V4(address)) => address,
                    None => Ipv4Addr::UNSPECIFIED,
                    _ => continue,
                };
                let mask = u32::MAX.checked_shl(32 - u32::from(route.prefix)).unwrap_or(0);
                let dst = u32::from_le_bytes(dst.octets());
                let gateway = u32::from_le_bytes(gateway.octets());
                let mask = u32::from_le_bytes(mask.to_be_bytes());
                let _ = writeln!(
                    v4,
                    "{iface}\t{dst:08X}\t{gateway:08X}\t{flags:04X}\t0\t0\t0\t{mask:08X}\t0\t0\t0"
                );
            }
            (IpAddr::V6(dst), gateway) if route.prefix <= 128 => {
                let gateway = match gateway {
                    Some(IpAddr::V6(address)) => address,
                    None => Ipv6Addr::UNSPECIFIED,
                    _ => continue,
                };
                let dst = u128::from_be_bytes(dst.octets());
                let gateway = u128::from_be_bytes(gateway.octets());
                let prefix = route.prefix;
                let _ = writeln!(
                    v6,
                    "{dst:032x} {prefix:02x} {zero:032x} 00 {gateway:032x} 00000000 00000000 00000000 {flags:08x} {iface}",
                    zero = 0,
                );
            }
            _ => {}
        }
    }
    (v4, v6)
}

/// Fixed `/proc/sys` limits, with the values of a stock arm64 kernel.
pub const SYSCTLS: [(&str, &str); 5] = [
    ("overflowuid", "65534\n"),
    ("overflowgid", "65534\n"),
    ("ngroups_max", "65536\n"),
    ("pid_max", "4194304\n"),
    ("max_user_namespaces", "15000\n"),
];

/// Highest capability on Android's 5.x/6.x kernels (CAP_CHECKPOINT_RESTORE).
pub const CAP_LAST_CAP: &str = "40\n";

/// The counters `/proc/vmstat` readers (psutil's swap stats, `vmstat`) need.
pub const VMSTAT: &str = "pgpgin 0\npgpgout 0\npswpin 0\npswpout 0\npgfault 0\npgmajfault 0\n";

/// One process's CPU time, as read from `/proc/<pid>/stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessSample {
    pub pid: u32,
    /// Start time in clock ticks since boot; tells a reused pid apart.
    pub start: u64,
    /// utime + stime in clock ticks.
    pub cpu_ticks: u64,
    pub runnable: bool,
    /// The CPU it last ran on.
    pub processor: Option<usize>,
}

/// Parse the fields Portal needs from a `/proc/<pid>/stat` line.
pub fn parse_process_stat(pid: u32, line: &str) -> Option<ProcessSample> {
    // The command name may contain spaces and parentheses; fields resume
    // after the last ')'.
    let rest = &line[line.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // fields[0] is field 3 (state); utime/stime are 14/15, starttime 22,
    // processor 39.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    let start: u64 = fields.get(19)?.parse().ok()?;
    Some(ProcessSample {
        pid,
        start,
        cpu_ticks: utime + stime,
        runnable: fields.first() == Some(&"R"),
        processor: fields.get(36).and_then(|field| field.parse().ok()),
    })
}

/// Monotonic CPU and load counters built from successive process samples.
#[derive(Debug, Default)]
pub struct CpuAccount {
    seen: HashMap<(u32, u64), u64>,
    threads: HashMap<(u32, u64), u64>,
    /// Guest thread time by the CPU each thread was last seen on.
    cpu_ticks: Vec<u64>,
    busy_ticks: u64,
    load: [f64; 3],
    runnable: usize,
    processes: usize,
    sampled_at: Option<f64>,
}

impl CpuAccount {
    /// Fold in a sample of every visible process taken at `uptime_secs`.
    /// A process's time counts once; exited processes keep what they used.
    pub fn update(&mut self, uptime_secs: f64, samples: &[ProcessSample]) {
        let mut next = HashMap::with_capacity(samples.len());
        for sample in samples {
            let key = (sample.pid, sample.start);
            let previous = self.seen.get(&key).copied().unwrap_or(0);
            self.busy_ticks += sample.cpu_ticks.saturating_sub(previous);
            next.insert(key, sample.cpu_ticks.max(previous));
        }
        self.seen = next;
        self.processes = samples.len();
        self.runnable = samples.iter().filter(|sample| sample.runnable).count();
        let runnable = self.runnable as f64;
        match self.sampled_at {
            Some(previous) if uptime_secs > previous => {
                let elapsed = uptime_secs - previous;
                for (load, period) in self.load.iter_mut().zip([60.0, 300.0, 900.0]) {
                    let decay = (-elapsed / period).exp();
                    *load = *load * decay + runnable * (1.0 - decay);
                }
            }
            Some(_) => {}
            None => self.load = [runnable; 3],
        }
        self.sampled_at = Some(uptime_secs);
    }

    pub fn busy_ticks(&self) -> u64 {
        self.busy_ticks
    }

    /// Fold in every visible thread (`/proc/<pid>/task/<tid>/stat`),
    /// crediting the time each used since its last sample to the CPU it is
    /// on now: exact for a thread that stays put, as a busy one mostly does.
    pub fn update_threads(&mut self, threads: &[ProcessSample]) {
        let mut next = HashMap::with_capacity(threads.len());
        for thread in threads {
            let previous = self.threads.get(&(thread.pid, thread.start)).copied();
            next.insert((thread.pid, thread.start), thread.cpu_ticks.max(previous.unwrap_or(0)));
            if let (Some(previous), Some(cpu @ ..1024)) = (previous, thread.processor) {
                if self.cpu_ticks.len() <= cpu {
                    self.cpu_ticks.resize(cpu + 1, 0);
                }
                self.cpu_ticks[cpu] += thread.cpu_ticks.saturating_sub(previous);
            }
        }
        self.threads = next;
    }

    /// Guest CPU seconds credited to each CPU so far.
    pub fn cpu_secs(&self, ticks_per_sec: u64) -> Vec<f64> {
        self.cpu_ticks.iter().map(|ticks| *ticks as f64 / ticks_per_sec.max(1) as f64).collect()
    }
}

/// CPUs named by a sysfs CPU list such as `0-3,6\n`.
pub fn parse_cpu_list(text: &str) -> Vec<usize> {
    let mut cpus = Vec::new();
    for part in text.trim().split(',').filter(|part| !part.is_empty()) {
        let (first, last) = part.split_once('-').unwrap_or((part, part));
        if let (Ok(first), Ok(last)) = (first.parse::<usize>(), last.parse::<usize>()) {
            cpus.extend((first..=last).take_while(|cpu| *cpu < 1024));
        }
    }
    cpus
}

/// One online CPU's cpuidle totals, which Android leaves readable to apps:
/// residency (µs) and entries summed over `cpuN/cpuidle/state*/{time,usage}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleSample {
    pub cpu: usize,
    pub idle_us: u64,
    pub entries: u64,
}

/// Bounds the busy time a core's guesses may owe or be owed.
const OWED_CAP_SECS: f64 = 4.0;

#[derive(Debug, Clone, Copy, Default)]
struct Core {
    seen: bool,
    online: bool,
    was_online: bool,
    idle_us: u64,
    entries: u64,
    /// When the residency was last complete (an idle exit seen).
    resolved_at: f64,
    last_pass: f64,
    busy: f64,
    idle: f64,
    /// Busy time guessed since `resolved_at`.
    guessed: f64,
    /// Measured minus emitted busy time, paid back in later passes.
    owed: f64,
    /// Busy share of the last settled span.
    ratio: f64,
    /// Busy share judged from guest threads during a silence.
    judged: Option<f64>,
}

impl Core {
    /// Start a new span at `now`.
    fn restart(&mut self, now: f64, sample: &IdleSample) {
        self.idle_us = sample.idle_us;
        self.entries = sample.entries;
        self.resolved_at = now;
        self.last_pass = now;
        self.guessed = 0.0;
        self.judged = None;
    }
}

/// Per-CPU busy and idle time for the whole device, from cpuidle.
///
/// The kernel credits an idle period only when the CPU leaves it, so a CPU
/// with no idle exit since the last pass was either asleep (big cores sleep
/// for tens of seconds) or busy throughout. Such a silence is split by the
/// core's last busy share, then by the guest thread time on that CPU
/// (`presume`); the next exit shows what the guesses got wrong, and later
/// passes pay it back. The counters only grow, and each core's busy and
/// idle add up to the time elapsed, as the kernel's do.
#[derive(Debug, Default)]
pub struct HostCpuAccount {
    cores: Vec<Core>,
    presumed_at: Option<(f64, Vec<f64>)>,
}

impl HostCpuAccount {
    /// Fold in the online CPUs' totals read at `now` (CLOCK_MONOTONIC
    /// seconds; cpuidle residency excludes suspend like that clock does).
    pub fn update(&mut self, now: f64, samples: &[IdleSample]) {
        for core in &mut self.cores {
            core.was_online = core.online;
            core.online = false;
        }
        for sample in samples {
            if self.cores.len() <= sample.cpu {
                self.cores.resize(sample.cpu + 1, Core::default());
            }
            let core = &mut self.cores[sample.cpu];
            core.online = true;
            if !core.seen {
                // Since-boot baseline: whatever was not idle was busy.
                core.seen = true;
                core.idle = (sample.idle_us as f64 / 1e6).min(now.max(0.0));
                core.busy = now - core.idle;
                core.restart(now, sample);
                continue;
            }
            // No residency accrues while offline, and a counter that went
            // back cannot be compared: start over without counting the gap.
            if !core.was_online
                || now < core.last_pass
                || sample.idle_us < core.idle_us
                || sample.entries < core.entries
            {
                core.owed = 0.0;
                core.restart(now, sample);
                continue;
            }
            let elapsed = now - core.last_pass;
            let settled = sample.entries != core.entries;
            let guess = if settled {
                let span = now - core.resolved_at;
                // Can exceed the span: a sleep that began before it is
                // credited only now, and repays busy guessed for it.
                let idle = (sample.idle_us - core.idle_us) as f64 / 1e6;
                if span > 0.0 {
                    core.ratio = ((span - idle) / span).clamp(0.0, 1.0);
                }
                span - idle - core.guessed
            } else {
                elapsed * core.judged.unwrap_or(core.ratio)
            };
            let wanted = guess + core.owed;
            let busy = wanted.clamp(0.0, elapsed);
            core.owed = (wanted - busy).clamp(-OWED_CAP_SECS, OWED_CAP_SECS);
            core.busy += busy;
            core.idle += elapsed - busy;
            if settled {
                core.restart(now, sample);
            } else {
                core.guessed += guess;
                core.last_pass = now;
            }
        }
    }

    /// Judge the silent CPUs from the guest thread time credited to each
    /// (cumulative seconds, by CPU) since the previous call.
    pub fn presume(&mut self, now: f64, guest_cpu_secs: &[f64]) {
        let previous = self.presumed_at.replace((now, guest_cpu_secs.to_vec()));
        let Some((then, before)) = previous.filter(|(then, _)| now > *then) else {
            return;
        };
        let share = |cpu: usize| {
            let used = guest_cpu_secs.get(cpu).unwrap_or(&0.0) - before.get(cpu).unwrap_or(&0.0);
            (used / (now - then)).max(0.0)
        };
        // A thread is credited to the CPU it was on when sampled, so one CPU
        // can get more than it could run while a sibling gets none: hand the
        // surplus to the silent CPUs by their spare capacity.
        let online = (0..self.cores.len()).filter(|cpu| self.cores[*cpu].online);
        let surplus: f64 = online.clone().map(|cpu| (share(cpu) - 1.0).max(0.0)).sum();
        let silent: Vec<usize> = online.filter(|cpu| self.cores[*cpu].resolved_at < now).collect();
        let spare: f64 = silent.iter().map(|cpu| 1.0 - share(*cpu).min(1.0)).sum();
        for cpu in silent {
            let base = share(cpu).min(1.0);
            let mut judged = if spare > 0.0 {
                (base + surplus * (1.0 - base) / spare).min(1.0)
            } else {
                base
            };
            // Guest threads ran on a CPU with no idle exit all window: it ran
            // throughout (Android's own work there is invisible) unless a
            // sleep began after them, which its next exit settles.
            if self.cores[cpu].resolved_at <= then && judged > 0.05 {
                judged = 1.0;
            }
            self.cores[cpu].judged = Some(judged);
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.cores.iter().any(|core| core.online)
    }
}

/// `/proc/uptime`: seconds since boot and aggregate idle seconds.
pub fn render_uptime(
    uptime_secs: f64,
    cpus: usize,
    account: &CpuAccount,
    host: &HostCpuAccount,
    ticks_per_sec: u64,
) -> String {
    let total = uptime_secs * cpus.max(1) as f64;
    let busy = account.busy_ticks() as f64 / ticks_per_sec.max(1) as f64;
    let idle = match host.is_empty() {
        true => (total - busy).max(0.0),
        false => host.cores.iter().map(|core| core.idle).sum(),
    };
    format!("{:.2} {:.2}\n", uptime_secs, idle)
}

/// `/proc/stat` with an aggregate line, one line per CPU and the boot time.
/// With cpuidle readable the lines are the device's own time per online CPU
/// (busy as user time: nothing finer is visible), as in `host_stat_lines`.
pub fn render_stat(
    uptime_secs: f64,
    cpus: usize,
    boot_time_secs: u64,
    account: &CpuAccount,
    host: &HostCpuAccount,
    ticks_per_sec: u64,
) -> String {
    if !host.is_empty() {
        let mut text = host_stat_lines(host, ticks_per_sec);
        text.push_str(&stat_tail(boot_time_secs, account));
        return text;
    }
    let cpus = cpus.max(1);
    let total = (uptime_secs * ticks_per_sec as f64) as u64 * cpus as u64;
    let busy = account.busy_ticks().min(total);
    let idle = total - busy;
    // Guest work is user time as far as anyone can tell.
    let mut text = format!("cpu  {busy} 0 0 {idle} 0 0 0 0 0 0\n");
    for cpu in 0..cpus as u64 {
        let share = |value: u64| value / cpus as u64 + u64::from(cpu < value % cpus as u64);
        text.push_str(&format!("cpu{cpu} {} 0 0 {} 0 0 0 0 0 0\n", share(busy), share(idle)));
    }
    text.push_str(&stat_tail(boot_time_secs, account));
    text
}

fn host_stat_lines(host: &HostCpuAccount, ticks_per_sec: u64) -> String {
    let ticks = |secs: f64| (secs * ticks_per_sec as f64) as u64;
    // The kernel sums every possible CPU, so offline ones keep theirs.
    let busy: u64 = host.cores.iter().map(|core| ticks(core.busy)).sum();
    let idle: u64 = host.cores.iter().map(|core| ticks(core.idle)).sum();
    let mut text = format!("cpu  {busy} 0 0 {idle} 0 0 0 0 0 0\n");
    for (cpu, core) in host.cores.iter().enumerate().filter(|(_, core)| core.online) {
        let (busy, idle) = (ticks(core.busy), ticks(core.idle));
        text.push_str(&format!("cpu{cpu} {busy} 0 0 {idle} 0 0 0 0 0 0\n"));
    }
    text
}

fn stat_tail(boot_time_secs: u64, account: &CpuAccount) -> String {
    format!(
        "intr 0\nctxt 0\nbtime {boot_time_secs}\nprocesses {}\nprocs_running {}\nprocs_blocked 0\nsoftirq 0 0 0 0 0 0 0 0 0 0 0\n",
        account.processes,
        account.runnable.max(1),
    )
}

/// `/proc/loadavg`: 1, 5 and 15 minute averages, runnable/total, last pid.
pub fn render_loadavg(account: &CpuAccount, last_pid: u32) -> String {
    format!(
        "{:.2} {:.2} {:.2} {}/{} {last_pid}\n",
        account.load[0],
        account.load[1],
        account.load[2],
        account.runnable.max(1),
        account.processes.max(1),
    )
}

/// `/proc/version` from `uname`'s release and version.
pub fn render_version(release: &str, version: &str) -> String {
    format!("Linux version {release} (android@localhost) (Android clang) {version}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(pid: u32, start: u64, cpu_ticks: u64) -> ProcessSample {
        ProcessSample { pid, start, cpu_ticks, runnable: false, processor: None }
    }

    #[test]
    fn renders_default_and_link_routes_in_linux_format() {
        let (v4, v6) = render_network_routes(&[
            NetworkRoute {
                interface: "wlan0".into(),
                destination: "0.0.0.0".parse().unwrap(),
                prefix: 0,
                gateway: Some("192.0.2.1".parse().unwrap()),
            },
            NetworkRoute {
                interface: "wlan0".into(),
                destination: "192.0.2.0".parse().unwrap(),
                prefix: 24,
                gateway: None,
            },
            NetworkRoute {
                interface: "wlan0".into(),
                destination: "::".parse().unwrap(),
                prefix: 0,
                gateway: Some("fe80::1".parse().unwrap()),
            },
        ]);
        let mut routes = v4.lines().skip(1);
        assert_eq!(
            routes.next(),
            Some("wlan0\t00000000\t010200C0\t0003\t0\t0\t0\t00000000\t0\t0\t0")
        );
        assert_eq!(
            routes.next(),
            Some("wlan0\t000200C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0")
        );
        assert_eq!(
            v6,
            "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000000 00000000 00000000 00000003 wlan0\n"
        );
    }

    #[test]
    fn an_offline_network_has_no_routes() {
        let (v4, v6) = render_network_routes(&[]);
        assert_eq!(v4.lines().count(), 1);
        assert!(v6.is_empty());
    }

    #[test]
    fn parses_a_command_with_spaces_and_parentheses() {
        let line = "1234 (Web Content (x)) R 1 2 3 4 5 6 7 8 9 10 70 30 0 0 20 0 9 0 4242 0 0";
        let parsed = parse_process_stat(1234, line).unwrap();
        assert_eq!(parsed.cpu_ticks, 100);
        assert_eq!(parsed.start, 4242);
        assert!(parsed.runnable);
    }

    #[test]
    fn cpu_time_only_grows_and_exited_processes_keep_theirs() {
        let mut account = CpuAccount::default();
        account.update(10.0, &[sample(1, 5, 100), sample(2, 6, 50)]);
        assert_eq!(account.busy_ticks(), 150);
        account.update(11.0, &[sample(1, 5, 130)]);
        assert_eq!(account.busy_ticks(), 180);
        // pid 2 reused by a new process: its time is new time.
        account.update(12.0, &[sample(1, 5, 130), sample(2, 900, 7)]);
        assert_eq!(account.busy_ticks(), 187);
    }

    #[test]
    fn stat_lines_add_up_and_carry_the_boot_time() {
        let mut account = CpuAccount::default();
        account.update(100.0, &[sample(1, 5, 801)]);
        let host = HostCpuAccount::default();
        let text = render_stat(100.0, 8, 1_700_000_000, &account, &host, 100);
        assert!(text.starts_with("cpu  801 0 0 79199 "));
        let per_cpu_busy: u64 = text
            .lines()
            .filter(|line| line.starts_with("cpu") && !line.starts_with("cpu "))
            .map(|line| line.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap())
            .sum();
        assert_eq!(per_cpu_busy, 801);
        assert_eq!(text.lines().filter(|line| line.starts_with("cpu")).count(), 9);
        assert!(text.contains("\nbtime 1700000000\n"));
    }

    #[test]
    fn uptime_and_loadavg_are_well_formed() {
        let mut account = CpuAccount::default();
        account.update(50.0, &[ProcessSample { runnable: true, ..sample(3, 1, 1000) }]);
        let host = HostCpuAccount::default();
        assert_eq!(render_uptime(50.0, 4, &account, &host, 100), "50.00 190.00\n");
        assert_eq!(render_loadavg(&account, 77), "1.00 1.00 1.00 1/1 77\n");
    }

    fn idle(cpu: usize, idle_us: u64, entries: u64) -> IdleSample {
        IdleSample { cpu, idle_us, entries }
    }

    fn host_stat(host: &HostCpuAccount) -> String {
        render_stat(0.0, 2, 1, &CpuAccount::default(), host, 100)
    }

    /// (user, idle) ticks of each named line of `host`'s `/proc/stat`.
    fn lines(host: &HostCpuAccount, names: &[&str]) -> Vec<(u64, u64)> {
        let text = host_stat(host);
        let ticks = |name| {
            let line = text.lines().find(|line| line.split(' ').next() == Some(name)).unwrap();
            let fields: Vec<u64> = line.split_whitespace().skip(1).map(|f| f.parse().unwrap()).collect();
            (fields[0], fields[3])
        };
        names.iter().map(|name| ticks(*name)).collect()
    }

    /// (busy, idle) ticks each named line gains over `passes`.
    fn gains(host: &mut HostCpuAccount, names: &[&str], passes: impl FnOnce(&mut HostCpuAccount)) -> Vec<(u64, u64)> {
        let before = lines(host, names);
        passes(host);
        let after = lines(host, names);
        before.iter().zip(after).map(|(b, a)| (a.0 - b.0, a.1 - b.1)).collect()
    }

    #[test]
    fn host_cpu_lines_follow_cpuidle_residency() {
        assert_eq!(parse_cpu_list("0-2,5\n"), vec![0, 1, 2, 5]);
        let mut host = HostCpuAccount::default();
        host.update(100.0, &[idle(0, 60_000_000, 10), idle(1, 90_000_000, 5)]);
        assert_eq!(lines(&host, &["cpu0"]), [(4000, 6000)]);
        host.update(101.0, &[idle(0, 60_250_000, 30), idle(1, 90_900_000, 9)]);
        assert_eq!(lines(&host, &["cpu0", "cpu"]), [(4075, 6025), (5085, 15115)]);
        // An offline CPU has no line, and its gap counts as neither.
        host.update(102.0, &[idle(0, 61_000_000, 31)]);
        assert!(!host_stat(&host).contains("\ncpu1 "));
        host.update(110.0, &[idle(0, 69_000_000, 32), idle(1, 90_900_000, 9)]);
        let back = gains(&mut host, &["cpu1"], |host| {
            host.update(111.0, &[idle(0, 69_000_000, 33), idle(1, 91_400_000, 10)])
        });
        assert_eq!(back, [(50, 50)]);
    }

    #[test]
    fn a_saturated_core_waking_now_and_then_reads_busy() {
        let mut host = HostCpuAccount::default();
        host.update(10.0, &[idle(0, 0, 0)]);
        host.update(11.0, &[idle(0, 10_000, 1)]);
        // Passes every 0.5 s; one 10 ms idle a second.
        let busy = gains(&mut host, &["cpu0"], |host| {
            for step in 1..=20u64 {
                let now = 11.0 + step as f64 * 0.5;
                host.update(now, &[idle(0, (1 + step / 2) * 10_000, 1 + step / 2)]);
                host.presume(now, &[0.0]);
            }
        });
        assert_eq!(busy, [(990, 10)]);
    }

    #[test]
    fn silent_cores_are_judged_from_guest_threads() {
        let silent = [idle(0, 0, 0), idle(1, 0, 0)];
        let judge = |guest: [f64; 2]| {
            let mut host = HostCpuAccount::default();
            host.update(10.0, &silent);
            host.presume(10.0, &[0.0; 2]);
            host.update(12.0, &silent);
            host.presume(12.0, &guest);
            gains(&mut host, &["cpu0", "cpu1"], |host| host.update(13.0, &silent))
        };
        // cpu0 slept; cpu1 ran guest threads part of the time and Android's
        // own work the rest, which the guest's share cannot show.
        assert_eq!(judge([0.0, 1.2]), [(0, 100), (100, 0)]);
        // Both threads were on cpu0 when sampled, but one ran on cpu1.
        assert_eq!(judge([3.6, 0.0]), [(100, 0), (100, 0)]);
    }

    #[test]
    fn misjudged_time_is_paid_back() {
        let mut host = HostCpuAccount::default();
        host.update(10.0, &[idle(0, 0, 0)]);
        // A busy span sets an 80% share, then the core sleeps unseen.
        host.update(10.5, &[idle(0, 100_000, 1)]);
        host.update(11.0, &[idle(0, 100_000, 1)]);
        host.update(11.5, &[idle(0, 100_000, 1)]);
        // The 1 s sleep is credited at its exit: 0.3 s too much was guessed,
        // and comes off the next busy half second.
        host.update(12.0, &[idle(0, 1_100_000, 2)]);
        assert_eq!(lines(&host, &["cpu0"]), [(1120, 80)]);
        host.update(12.5, &[idle(0, 1_100_000, 3)]);
        assert_eq!(lines(&host, &["cpu0"]), [(1140, 110)]);
    }

    #[test]
    fn thread_time_is_credited_to_the_cpu_it_runs_on() {
        let thread = |tid, cpu_ticks, cpu| ProcessSample {
            pid: tid,
            start: 1,
            cpu_ticks,
            runnable: true,
            processor: Some(cpu),
        };
        let mut account = CpuAccount::default();
        account.update_threads(&[thread(5, 1000, 7), thread(6, 40, 2)]);
        account.update_threads(&[thread(5, 1200, 7), thread(6, 50, 3)]);
        assert_eq!(account.cpu_secs(100), [0.0, 0.0, 0.0, 0.1, 0.0, 0.0, 0.0, 2.0]);
        let line = "5 (a b) R 1 2 3 4 5 6 7 8 9 10 70 30 0 0 20 0 9 0 4242 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 17 6 0";
        assert_eq!(parse_process_stat(5, line).unwrap().processor, Some(6));
    }
}
