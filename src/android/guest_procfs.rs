//! Writes the generated `/proc` stand-ins (see `core::guest_procfs`) and
//! the binds that put them over the paths Android denies.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::guest_procfs::{self as procfs, CpuAccount, ProcessSample};
use crate::core::runtime::BindMount;

static ACCOUNT: Mutex<Option<CpuAccount>> = Mutex::new(None);

fn directory() -> PathBuf {
    Path::new(crate::core::config::APP_FILES_ROOT).join("fake-proc")
}

/// Binds for every stand-in whose real file the app cannot read, after
/// writing their first contents. Called once per session launch.
pub fn session_binds() -> Vec<BindMount> {
    let dir = directory();
    if let Err(error) = fs::create_dir_all(&dir) {
        log::warn!("fake /proc directory could not be created: {error}");
        return Vec::new();
    }
    if let Ok(mut account) = ACCOUNT.lock() {
        *account = Some(CpuAccount::default());
    }
    write(&dir, "cap_last_cap", procfs::CAP_LAST_CAP);
    write(&dir, "vmstat", procfs::VMSTAT);
    let (release, version) = uname();
    write(&dir, "version", &procfs::render_version(&release, &version));
    refresh();
    procfs::FILES
        .iter()
        .filter(|(_, guest)| fs::File::open(guest).is_err())
        .map(|(name, guest)| BindMount::new(dir.join(name), *guest))
        .collect()
}

/// Rewrite the live files (uptime, CPU time, load). Cheap: one pass over
/// Portal's own processes in `/proc`.
pub fn refresh() {
    let Some(uptime) = boot_clock_secs() else {
        return;
    };
    let samples = sample_processes();
    let last_pid = samples.iter().map(|sample| sample.pid).max().unwrap_or(1);
    let cpus = std::thread::available_parallelism().map_or(1, |count| count.get());
    let ticks = clock_ticks();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64());
    let boot_time = (now - uptime).max(0.0) as u64;
    let Ok(mut guard) = ACCOUNT.lock() else {
        return;
    };
    let account = guard.get_or_insert_with(CpuAccount::default);
    account.update(uptime, &samples);
    let dir = directory();
    write(&dir, "uptime", &procfs::render_uptime(uptime, cpus, account, ticks));
    write(&dir, "stat", &procfs::render_stat(uptime, cpus, boot_time, account, ticks));
    write(&dir, "loadavg", &procfs::render_loadavg(account, last_pid));
}

fn write(dir: &Path, name: &str, contents: &str) {
    // Replace by rename so a reader never sees a half-written file.
    let path = dir.join(name);
    let temporary = dir.join(format!(".{name}.tmp"));
    if let Err(error) = fs::write(&temporary, contents).and_then(|()| fs::rename(&temporary, &path)) {
        log::debug!("fake /proc/{name} could not be written: {error}");
    }
}

fn sample_processes() -> Vec<ProcessSample> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let line = fs::read_to_string(entry.path().join("stat")).ok()?;
            procfs::parse_process_stat(pid, &line)
        })
        .collect()
}

fn boot_clock_secs() -> Option<f64> {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    (unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut now) } == 0)
        .then(|| now.tv_sec as f64 + now.tv_nsec as f64 / 1e9)
}

fn clock_ticks() -> u64 {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 {
        ticks as u64
    } else {
        100
    }
}

fn uname() -> (String, String) {
    let mut name: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut name) } != 0 {
        return ("unknown".into(), "#1 SMP".into());
    }
    let field = |raw: &[libc::c_char]| {
        let bytes: Vec<u8> = raw.iter().take_while(|byte| **byte != 0).map(|byte| *byte as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    (field(&name.release), field(&name.version))
}
