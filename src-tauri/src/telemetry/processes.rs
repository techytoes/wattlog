use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::Instant;

use super::types::{
    ActionResult, AppProcessGroup, MemoryPressureInfo, ProcessReport, ProcessRow, SleepBlocker,
};

/// Hardcoded system-process denylist to prevent accidental Quit/Kill of critical macOS daemons.
const PROTECTED_SYSTEM_PROCESSES: &[&str] = &[
    "kernel_task",
    "launchd",
    "windowserver",
    "loginwindow",
    "powerd",
    "coreaudiod",
    "bluetoothd",
    "mds",
    "mds_stores",
    "configd",
    "fseventsd",
    "distnoted",
    "usereventagent",
    "syslogd",
    "notifyd",
    "cfprefsd",
    "securityd",
    "trustd",
    "opendirectoryd",
    "diskarbitrationd",
    "runningboardd",
    "backboardd",
    "hidd",
    "coreduetd",
    "dasd",
    "thermalmonitord",
    "airportd",
    "wifid",
    "dock",
    "finder",
    "systemuiserver",
    "controlcenter",
    "usernoted",
];

pub fn is_system_protected_name(name: &str, pid: i32) -> bool {
    if pid <= 1 {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    PROTECTED_SYSTEM_PROCESSES
        .iter()
        .any(|p| *p == lower || lower.starts_with("com.apple.driver"))
}

pub fn extract_app_name_from_path(path: &str, fallback_name: &str) -> (String, Option<String>) {
    // Roll helper apps inside e.g. `/Applications/Google Chrome.app/Contents/Frameworks/.../Google Chrome Helper (Renderer).app/...`
    // up to the OUTERMOST `.app` bundle!
    if let Some(idx) = path.find(".app/") {
        let bundle = &path[..idx + 4];
        if let Some(stem) = Path::new(bundle).file_stem().and_then(|s| s.to_str()) {
            return (stem.to_string(), Some(bundle.to_string()));
        }
    }
    if path.ends_with(".app") {
        if let Some(stem) = Path::new(path).file_stem().and_then(|s| s.to_str()) {
            return (stem.to_string(), Some(path.to_string()));
        }
    }
    (fallback_name.to_string(), None)
}

#[derive(Debug, Clone, Copy)]
struct EnergyWeights {
    kcpu_time: f64,
    kcpu_wakeups: f64,
    kdiskio_read: f64,
    kdiskio_write: f64,
}

impl Default for EnergyWeights {
    fn default() -> Self {
        Self {
            kcpu_time: 1.0,
            kcpu_wakeups: 2.0e-4,
            kdiskio_read: 4.5e-10,
            kdiskio_write: 2.4e-10,
        }
    }
}

fn load_pmenergy_weights() -> EnergyWeights {
    let dir = Path::new("/usr/share/pmenergy");
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return EnergyWeights::default(),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("plist") {
            continue;
        }
        if let Ok(val) = plist::Value::from_file(&path) {
            if let Some(dict) = val
                .as_dictionary()
                .and_then(|d| d.get("energy_constants"))
                .and_then(|v| v.as_dictionary())
            {
                let get = |k: &str, def: f64| {
                    dict.get(k)
                        .and_then(|v| v.as_real())
                        .unwrap_or(def)
                };
                return EnergyWeights {
                    kcpu_time: get("kcpu_time", 1.0),
                    kcpu_wakeups: get("kcpu_wakeups", 2.0e-4),
                    kdiskio_read: get("kdiskio_bytesread", 4.5e-10),
                    kdiskio_write: get("kdiskio_byteswritten", 2.4e-10),
                };
            }
        }
    }
    EnergyWeights::default()
}

#[derive(Debug, Clone, Copy)]
struct PrevSample {
    cpu_ns: u64,
    wakeups: u64,
    disk_read: u64,
    disk_write: u64,
    ts: Instant,
}

pub struct ProcessMonitor {
    weights: EnergyWeights,
    prev: Mutex<HashMap<i32, PrevSample>>,
    timebase_numer: u64,
    timebase_denom: u64,
}

impl ProcessMonitor {
    pub fn new() -> Self {
        let (numer, denom) = mach_timebase_ratio();
        Self {
            weights: load_pmenergy_weights(),
            prev: Mutex::new(HashMap::new()),
            timebase_numer: numer,
            timebase_denom: denom,
        }
    }

    pub fn collect(&self, blockers: &[SleepBlocker]) -> ProcessReport {
        let blocker_pids: HashSet<i32> = blockers
            .iter()
            .filter(|b| !b.is_system_protected && !b.is_self_wattlog)
            .map(|b| b.pid)
            .collect();

        let mem_pressure = read_memory_pressure();

        #[cfg(target_os = "macos")]
        let raw_rows = self.sample_macos_processes();
        #[cfg(not(target_os = "macos"))]
        let raw_rows: Vec<(ProcessRow, Option<String>, String)> = Vec::new();

        // Build pid -> ppid map so helper processes outside `.app` bundles can roll up to parent app if applicable.
        let mut by_pid: HashMap<i32, (String, Option<String>, i32)> = HashMap::new();
        for (row, bundle, app_name) in &raw_rows {
            by_pid.insert(row.pid, (app_name.clone(), bundle.clone(), row.ppid));
        }

        // Group by bundle_path (preferred) or top-level app/process name.
        struct GroupAcc {
            id: String,
            name: String,
            bundle_path: Option<String>,
            primary_pid: i32,
            is_system: bool,
            is_blocking: bool,
            cpu_pct: f64,
            footprint_bytes: u64,
            energy_impact: f64,
            wakeups_per_sec: f64,
            children: Vec<ProcessRow>,
        }

        let mut groups_map: HashMap<String, GroupAcc> = HashMap::new();
        let mut kernel_cpu = 0.0f64;

        for (row, mut bundle_path, mut app_name) in raw_rows {
            if row.pid == 0 || row.name == "kernel_task" {
                kernel_cpu = row.cpu_pct;
            }

            // If this process didn't have an `.app` path directly, walk up parents (up to 4 hops) to see if it belongs to a `.app` tree.
            if bundle_path.is_none() {
                let mut cur_ppid = row.ppid;
                for _ in 0..4 {
                    if cur_ppid <= 1 {
                        break;
                    }
                    if let Some((p_app, p_bundle, next_ppid)) = by_pid.get(&cur_ppid) {
                        if p_bundle.is_some() {
                            app_name = p_app.clone();
                            bundle_path = p_bundle.clone();
                            break;
                        }
                        cur_ppid = *next_ppid;
                    } else {
                        break;
                    }
                }
            }

            let group_key = match &bundle_path {
                Some(b) => format!("bundle:{b}"),
                None => format!("proc:{app_name}"),
            };

            let blocks = blocker_pids.contains(&row.pid);
            let entry = groups_map.entry(group_key.clone()).or_insert_with(|| GroupAcc {
                id: group_key,
                name: app_name.clone(),
                bundle_path: bundle_path.clone(),
                primary_pid: row.pid,
                is_system: row.is_system_protected,
                is_blocking: blocks,
                cpu_pct: 0.0,
                footprint_bytes: 0,
                energy_impact: 0.0,
                wakeups_per_sec: 0.0,
                children: Vec::new(),
            });

            // Prefer the parent/main binary as primary_pid (lowest pid or matching app name).
            if row.name == entry.name || row.pid < entry.primary_pid {
                entry.primary_pid = row.pid;
            }
            if blocks {
                entry.is_blocking = true;
            }
            if row.is_system_protected && entry.bundle_path.is_none() {
                entry.is_system = true;
            }
            entry.cpu_pct += row.cpu_pct;
            entry.footprint_bytes = entry.footprint_bytes.saturating_add(row.footprint_bytes);
            entry.energy_impact += row.energy_impact;
            entry.wakeups_per_sec += row.wakeups_per_sec;
            entry.children.push(row);
        }

        let mut total_cpu = 0.0f64;
        let mut total_footprint = 0u64;

        let mut groups: Vec<AppProcessGroup> = groups_map
            .into_values()
            .filter(|g| g.footprint_bytes > 1024 * 1024 || g.cpu_pct >= 0.1 || g.is_blocking)
            .map(|mut g| {
                total_cpu += g.cpu_pct;
                total_footprint = total_footprint.saturating_add(g.footprint_bytes);

                g.children.sort_by(|a, b| {
                    b.energy_impact
                        .partial_cmp(&a.energy_impact)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(b.footprint_bytes.cmp(&a.footprint_bytes))
                });

                let helper_count = g.children.len();
                let mostly_in = if g.name == "kernel_task" {
                    Some(
                        "macOS kernel · high CPU here means thermal throttling protection, not a bug"
                            .to_string(),
                    )
                } else if let Some(top) = g.children.first() {
                    if helper_count > 1 || top.wakeups_per_sec >= 20.0 {
                        Some(format!(
                            "{} · {} · {:.0} wakeups/s",
                            top.name,
                            format_bytes_short(top.footprint_bytes),
                            top.wakeups_per_sec
                        ))
                    } else if g.is_system {
                        Some("protected macOS system process".into())
                    } else {
                        None
                    }
                } else {
                    None
                };

                let verdict = if g.is_system {
                    "system"
                } else if g.is_blocking {
                    "blocking"
                } else if g.cpu_pct >= 35.0 || g.energy_impact >= 40.0 || g.wakeups_per_sec >= 500.0
                {
                    "runaway"
                } else if g.cpu_pct >= 12.0 || g.footprint_bytes >= 2_500_000_000 {
                    "elevated"
                } else {
                    "normal"
                };

                // Keep top 12 children per group for crisp UI expansion.
                g.children.truncate(12);

                AppProcessGroup {
                    id: g.id,
                    name: g.name,
                    primary_pid: g.primary_pid,
                    bundle_path: g.bundle_path,
                    helper_count,
                    cpu_pct: (g.cpu_pct * 10.0).round() / 10.0,
                    footprint_bytes: g.footprint_bytes,
                    energy_impact: (g.energy_impact * 10.0).round() / 10.0,
                    wakeups_per_sec: g.wakeups_per_sec.round(),
                    verdict: verdict.into(),
                    is_system_protected: g.is_system,
                    is_blocking_sleep: g.is_blocking,
                    mostly_in,
                    children: g.children,
                }
            })
            .collect();

        // Sort worst energy/CPU/RAM culprits first.
        groups.sort_by(|a, b| {
            let score = |g: &AppProcessGroup| {
                let block_bonus = if g.is_blocking_sleep { 120.0 } else { 0.0 };
                let mem_gb = g.footprint_bytes as f64 / 1_073_741_824.0;
                block_bonus + g.energy_impact * 2.0 + g.cpu_pct * 1.5 + mem_gb * 3.0
            };
            score(b)
                .partial_cmp(&score(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        groups.truncate(35);

        let kernel_thermal_note = if kernel_cpu >= 25.0 {
            Some(format!(
                "kernel_task is at {:.0}% CPU — macOS uses kernel_task to thermally throttle the CPU when chassis/charging temps rise. Check vents, USB-C charging side, or runaway GPU/CPU apps below.",
                kernel_cpu
            ))
        } else {
            None
        };

        ProcessReport {
            groups,
            memory_pressure: mem_pressure,
            total_cpu_pct: (total_cpu * 10.0).round() / 10.0,
            total_footprint_bytes: total_footprint,
            kernel_thermal_note,
        }
    }

    #[cfg(target_os = "macos")]
    fn sample_macos_processes(&self) -> Vec<(ProcessRow, Option<String>, String)> {
        use libproc::libproc::bsd_info::BSDInfo;
        use libproc::libproc::pid_rusage::{pidrusage, RUsageInfoV4};
        use libproc::libproc::proc_pid::pidinfo;
        use libproc::processes::{pids_by_type, ProcFilter};

        let Ok(pids) = pids_by_type(ProcFilter::All) else {
            return Vec::new();
        };

        let now = Instant::now();
        let mut prev_guard = self.prev.lock().ok();
        let mut next_prev: HashMap<i32, PrevSample> = HashMap::with_capacity(pids.len());
        let mut rows: Vec<(ProcessRow, Option<String>, String)> = Vec::with_capacity(pids.len());

        for pid_u32 in pids {
            let pid = pid_u32 as i32;
            let bsd = pidinfo::<BSDInfo>(pid, 0).ok();
            let ppid = bsd.as_ref().map(|b| b.pbi_ppid as i32).unwrap_or(1);

            let path = process_path_for_pid(pid);
            let bsd_name = bsd
                .as_ref()
                .map(|b| {
                    let bytes: Vec<u8> = b
                        .pbi_name
                        .iter()
                        .take_while(|&&c| c != 0)
                        .map(|&c| c as u8)
                        .collect();
                    let s = String::from_utf8_lossy(&bytes).to_string();
                    if !s.is_empty() {
                        s
                    } else {
                        let comm: Vec<u8> = b
                            .pbi_comm
                            .iter()
                            .take_while(|&&c| c != 0)
                            .map(|&c| c as u8)
                            .collect();
                        String::from_utf8_lossy(&comm).to_string()
                    }
                })
                .unwrap_or_default();

            let proc_name = if let Some(ref p) = path {
                Path::new(p)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(bsd_name)
            } else {
                bsd_name
            };
            if proc_name.is_empty() {
                continue;
            }

            let rusage = pidrusage::<RUsageInfoV4>(pid).ok();
            let (footprint, cpu_ns, wakeups, disk_r, disk_w) = match rusage {
                Some(ru) => {
                    let raw_ticks = ru.ri_user_time.saturating_add(ru.ri_system_time);
                    let ns = (raw_ticks as u128 * self.timebase_numer as u128
                        / self.timebase_denom.max(1) as u128) as u64;
                    (
                        ru.ri_phys_footprint,
                        ns,
                        ru.ri_interrupt_wkups.saturating_add(ru.ri_pkg_idle_wkups),
                        ru.ri_diskio_bytesread,
                        ru.ri_diskio_byteswritten,
                    )
                }
                None => (0, 0, 0, 0, 0),
            };

            let (cpu_pct, wakeups_per_sec, energy_impact) = if let Some(ref prev_map) = prev_guard {
                if let Some(prev) = prev_map.get(&pid) {
                    let dt = now.duration_since(prev.ts).as_secs_f64().max(0.05);
                    let d_cpu_s = cpu_ns.saturating_sub(prev.cpu_ns) as f64 / 1_000_000_000.0;
                    let d_wk = wakeups.saturating_sub(prev.wakeups) as f64 / dt;
                    let d_r = disk_r.saturating_sub(prev.disk_read) as f64 / dt;
                    let d_w = disk_w.saturating_sub(prev.disk_write) as f64 / dt;
                    let cpu = ((d_cpu_s / dt) * 100.0).clamp(0.0, 2400.0);
                    // Apple Energy Impact formula weighted by /usr/share/pmenergy/*.plist
                    let ei = cpu * self.weights.kcpu_time
                        + d_wk * self.weights.kcpu_wakeups * 100.0
                        + d_r * self.weights.kdiskio_read * 100.0
                        + d_w * self.weights.kdiskio_write * 100.0;
                    (cpu, d_wk, ei)
                } else {
                    (0.0, 0.0, 0.0)
                }
            } else {
                (0.0, 0.0, 0.0)
            };

            next_prev.insert(
                pid,
                PrevSample {
                    cpu_ns,
                    wakeups,
                    disk_read: disk_r,
                    disk_write: disk_w,
                    ts: now,
                },
            );

            let (app_name, bundle_path) = match path.as_deref() {
                Some(p) => extract_app_name_from_path(p, &proc_name),
                None => (proc_name.clone(), None),
            };
            let is_sys = is_system_protected_name(&proc_name, pid);

            rows.push((
                ProcessRow {
                    pid,
                    ppid,
                    name: proc_name,
                    path,
                    cpu_pct: (cpu_pct * 10.0).round() / 10.0,
                    footprint_bytes: footprint,
                    energy_impact: (energy_impact * 10.0).round() / 10.0,
                    wakeups_per_sec: wakeups_per_sec.round(),
                    is_system_protected: is_sys,
                },
                bundle_path,
                app_name,
            ));
        }

        if let Some(ref mut prev_map) = prev_guard {
            **prev_map = next_prev;
        }

        rows
    }
}

pub fn process_path_for_pid(pid: i32) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        libproc::libproc::proc_pid::pidpath(pid).ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
}

/// Safe graceful Quit (`SIGTERM`, or Apple Event quit for `.app` bundles) protected by system denylist.
pub fn quit_process_safe(pid: i32, app_name: Option<&str>) -> ActionResult {
    let path = process_path_for_pid(pid);
    let proc_name = path
        .as_deref()
        .and_then(|p| Path::new(p).file_name())
        .and_then(|n| n.to_str())
        .or(app_name)
        .unwrap_or("unknown");

    if is_system_protected_name(proc_name, pid) || pid == std::process::id() as i32 {
        return ActionResult {
            ok: false,
            message: format!("Refused: {proc_name} (PID {pid}) is protected."),
        };
    }

    // If this is a GUI `.app` bundle, try a clean Apple Event quit first for graceful document saving.
    if let Some(name) = app_name {
        if path.as_deref().map(|p| p.contains(".app/")).unwrap_or(false)
            && !name.contains("Helper")
            && !name.contains('"')
            && !name.contains('\\')
        {
            let script = format!("tell application \"{name}\" to quit");
            if let Ok(out) = Command::new("/usr/bin/osascript")
                .args(["-e", &script])
                .output()
            {
                if out.status.success() {
                    return ActionResult {
                        ok: true,
                        message: format!("Sent graceful Quit to {name}"),
                    };
                }
            }
        }
    }

    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc == 0 {
        ActionResult {
            ok: true,
            message: format!("Sent SIGTERM to {proc_name} (PID {pid})"),
        }
    } else {
        ActionResult {
            ok: false,
            message: format!(
                "Could not quit {proc_name} (PID {pid}): {}",
                std::io::Error::last_os_error()
            ),
        }
    }
}

/// 2-keypress confirmed Force Kill (`SIGKILL`) protected by system denylist.
pub fn force_kill_process_safe(pid: i32, confirmed: bool) -> ActionResult {
    if !confirmed {
        return ActionResult {
            ok: false,
            message: "Force kill requires explicit 2-keypress confirmation.".into(),
        };
    }

    let path = process_path_for_pid(pid);
    let proc_name = path
        .as_deref()
        .and_then(|p| Path::new(p).file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    if is_system_protected_name(proc_name, pid) || pid == std::process::id() as i32 {
        return ActionResult {
            ok: false,
            message: format!("Refused: {proc_name} (PID {pid}) is system-protected."),
        };
    }

    let rc = unsafe { libc::kill(pid, libc::SIGKILL) };
    if rc == 0 {
        ActionResult {
            ok: true,
            message: format!("Force-killed {proc_name} (PID {pid})"),
        }
    } else {
        ActionResult {
            ok: false,
            message: format!(
                "Could not force-kill {proc_name} (PID {pid}): {}",
                std::io::Error::last_os_error()
            ),
        }
    }
}

fn mach_timebase_ratio() -> (u64, u64) {
    #[cfg(target_os = "macos")]
    unsafe {
        let mut info = mach2::mach_time::mach_timebase_info { numer: 1, denom: 1 };
        if mach2::mach_time::mach_timebase_info(&mut info) == 0 && info.denom > 0 {
            return (info.numer as u64, info.denom as u64);
        }
    }
    (1, 1)
}

fn read_memory_pressure() -> MemoryPressureInfo {
    #[cfg(target_os = "macos")]
    {
        let level_num = sysctl_u32("kern.memorystatus_vm_pressure_level").unwrap_or(1);
        let level = match level_num {
            1 => "green",
            2 => "yellow",
            _ => "red",
        };
        let total_bytes = sysctl_u64("hw.memsize").unwrap_or(0);
        let (swap_total, swap_used) = read_swap_usage();
        let (wired, compressed, used) = read_vm_stats();

        return MemoryPressureInfo {
            level: level.into(),
            total_bytes,
            used_bytes: used.min(total_bytes),
            wired_bytes: wired,
            compressed_bytes: compressed,
            swap_used_bytes: swap_used,
            swap_total_bytes: swap_total,
        };
    }
    #[cfg(not(target_os = "macos"))]
    {
        MemoryPressureInfo {
            level: "green".into(),
            total_bytes: 0,
            used_bytes: 0,
            wired_bytes: 0,
            compressed_bytes: 0,
            swap_used_bytes: 0,
            swap_total_bytes: 0,
        }
    }
}

#[cfg(target_os = "macos")]
fn sysctl_u32(name: &str) -> Option<u32> {
    let cname = std::ffi::CString::new(name).ok()?;
    let mut val: u32 = 0;
    let mut len = std::mem::size_of::<u32>();
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            &mut val as *mut _ as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc == 0 {
        Some(val)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn sysctl_u64(name: &str) -> Option<u64> {
    let cname = std::ffi::CString::new(name).ok()?;
    let mut val: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            &mut val as *mut _ as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc == 0 {
        Some(val)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn read_swap_usage() -> (u64, u64) {
    #[repr(C)]
    #[derive(Default)]
    struct XswUsage {
        xsu_total: u64,
        xsu_avail: u64,
        xsu_used: u64,
        xsu_pagesize: u32,
        xsu_encrypted: u8,
    }
    let Ok(cname) = std::ffi::CString::new("vm.swapusage") else {
        return (0, 0);
    };
    let mut val = XswUsage::default();
    let mut len = std::mem::size_of::<XswUsage>();
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            &mut val as *mut _ as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc == 0 {
        (val.xsu_total, val.xsu_used)
    } else {
        (0, 0)
    }
}

#[cfg(target_os = "macos")]
fn read_vm_stats() -> (u64, u64, u64) {
    extern "C" {
        fn mach_host_self() -> libc::mach_port_t;
        fn host_page_size(host: libc::mach_port_t, out_page_size: *mut libc::vm_size_t) -> libc::kern_return_t;
        fn host_statistics64(
            host_priv: libc::mach_port_t,
            flavor: libc::c_int,
            host_info_out: *mut libc::integer_t,
            host_info_out_cnt: *mut libc::mach_msg_type_number_t,
        ) -> libc::kern_return_t;
    }
    const HOST_VM_INFO64: libc::c_int = 4;

    unsafe {
        let host = mach_host_self();
        let mut page_size: libc::vm_size_t = 16384;
        let _ = host_page_size(host, &mut page_size);
        let mut vm_stat: libc::vm_statistics64 = std::mem::zeroed();
        let mut count = (std::mem::size_of::<libc::vm_statistics64>()
            / std::mem::size_of::<libc::integer_t>())
            as libc::mach_msg_type_number_t;
        let kr = host_statistics64(
            host,
            HOST_VM_INFO64,
            &mut vm_stat as *mut _ as *mut libc::integer_t,
            &mut count,
        );
        if kr != 0 {
            return (0, 0, 0);
        }
        let ps = page_size as u64;
        let wired = vm_stat.wire_count as u64 * ps;
        let compressed = vm_stat.compressor_page_count as u64 * ps;
        let active = vm_stat.active_count as u64 * ps;
        let internal = vm_stat.internal_page_count as u64 * ps;
        let purgeable = vm_stat.purgeable_count as u64 * ps;
        let app_mem = internal.saturating_sub(purgeable);
        let used = app_mem.saturating_add(wired).saturating_add(compressed).max(active);
        (wired, compressed, used)
    }
}

pub fn format_bytes_short(bytes: u64) -> String {
    const GB: f64 = 1_073_741_824.0;
    const MB: f64 = 1_048_576.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.0} MB", b / MB)
    } else {
        format!("{:.0} KB", b / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_chrome_helpers_to_outermost_app_bundle() {
        let p = "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Helpers/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer)";
        let (app, bundle) = extract_app_name_from_path(p, "Google Chrome Helper (Renderer)");
        assert_eq!(app, "Google Chrome");
        assert_eq!(bundle.as_deref(), Some("/Applications/Google Chrome.app"));
    }

    #[test]
    fn denylist_protects_critical_system_daemons() {
        assert!(is_system_protected_name("kernel_task", 0));
        assert!(is_system_protected_name("WindowServer", 406));
        assert!(is_system_protected_name("launchd", 1));
        assert!(!is_system_protected_name("Google Chrome", 48102));
    }

    #[test]
    fn collects_real_process_report_and_memory_pressure() {
        let mon = ProcessMonitor::new();
        let report = mon.collect(&[]);
        assert!(matches!(
            report.memory_pressure.level.as_str(),
            "green" | "yellow" | "red"
        ));
        assert!(report.memory_pressure.total_bytes > 0);
        assert!(!report.groups.is_empty());
    }
}
