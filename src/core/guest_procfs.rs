//! Stand-ins for the `/proc` files Android's app sandbox denies.
//!
//! SELinux keeps apps from reading `/proc/stat`, `/proc/uptime`,
//! `/proc/loadavg`, `/proc/vmstat`, `/proc/version` and
//! `/proc/sys/kernel/cap_last_cap`. Ordinary Linux tools need them: `ps aux`
//! printed only a header and "Unable to get system boot time", `uptime`,
//! `htop`, Python's psutil and Plasma's System Monitor failed or showed
//! nothing. Portal binds generated files over those paths. Uptime and boot
//! time are exact; CPU time is what Portal's own processes (the desktop and
//! everything in it) used, since nothing else is visible to the app; the
//! load average follows their runnable count.
//!
//! This module only renders the files and keeps the counters monotonic;
//! the Android side samples `/proc/<pid>/stat` and writes them.

use std::collections::HashMap;

/// Files Portal generates, relative to its fake-proc directory, with the
/// guest path each is bound over.
pub const FILES: [(&str, &str); 6] = [
    ("stat", "/proc/stat"),
    ("uptime", "/proc/uptime"),
    ("loadavg", "/proc/loadavg"),
    ("vmstat", "/proc/vmstat"),
    ("version", "/proc/version"),
    ("cap_last_cap", "/proc/sys/kernel/cap_last_cap"),
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
}

/// Parse the fields Portal needs from a `/proc/<pid>/stat` line.
pub fn parse_process_stat(pid: u32, line: &str) -> Option<ProcessSample> {
    // The command name may contain spaces and parentheses; fields resume
    // after the last ')'.
    let rest = &line[line.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // fields[0] is field 3 (state); utime/stime are 14/15, starttime 22.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    let start: u64 = fields.get(19)?.parse().ok()?;
    Some(ProcessSample {
        pid,
        start,
        cpu_ticks: utime + stime,
        runnable: fields.first() == Some(&"R"),
    })
}

/// Monotonic CPU and load counters built from successive process samples.
#[derive(Debug, Default)]
pub struct CpuAccount {
    seen: HashMap<(u32, u64), u64>,
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
}

/// `/proc/uptime`: seconds since boot and aggregate idle seconds.
pub fn render_uptime(uptime_secs: f64, cpus: usize, account: &CpuAccount, ticks_per_sec: u64) -> String {
    let total = uptime_secs * cpus.max(1) as f64;
    let busy = account.busy_ticks() as f64 / ticks_per_sec.max(1) as f64;
    format!("{:.2} {:.2}\n", uptime_secs, (total - busy).max(0.0))
}

/// `/proc/stat` with an aggregate line, one line per CPU and the boot time.
pub fn render_stat(
    uptime_secs: f64,
    cpus: usize,
    boot_time_secs: u64,
    account: &CpuAccount,
    ticks_per_sec: u64,
) -> String {
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
    text.push_str(&format!(
        "intr 0\nctxt 0\nbtime {boot_time_secs}\nprocesses {}\nprocs_running {}\nprocs_blocked 0\nsoftirq 0 0 0 0 0 0 0 0 0 0 0\n",
        account.processes,
        account.runnable.max(1),
    ));
    text
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
        ProcessSample { pid, start, cpu_ticks, runnable: false }
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
        let text = render_stat(100.0, 8, 1_700_000_000, &account, 100);
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
        account.update(50.0, &[ProcessSample { pid: 3, start: 1, cpu_ticks: 1000, runnable: true }]);
        assert_eq!(render_uptime(50.0, 4, &account, 100), "50.00 190.00\n");
        assert_eq!(render_loadavg(&account, 77), "1.00 1.00 1.00 1/1 77\n");
    }
}
