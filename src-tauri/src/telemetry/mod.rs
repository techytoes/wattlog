pub mod assertions;
pub mod demo;
pub mod power;
pub mod processes;
pub mod sleep;
pub mod storage;
pub mod types;

use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use assertions::{collect_sleep_blockers, KeepAwakeManager};
use power::PowerReader;
use processes::ProcessMonitor;
use sleep::{collect_pmset_audit, collect_sleep_report};
use storage::Storage;
use types::*;

struct CachedSleep {
    report: SleepReport,
    audit: Vec<PmsetAuditItem>,
    updated_at: Instant,
}

pub struct TelemetryEngine {
    demo_mode: bool,
    power: PowerReader,
    processes: ProcessMonitor,
    pub keep_awake: KeepAwakeManager,
    storage: Mutex<Storage>,
    sleep_cache: Mutex<Option<CachedSleep>>,
    host_name: String,
    chip_name: String,
    macos_version: String,
}

impl TelemetryEngine {
    pub fn new(demo_mode: bool, background_macmon: bool) -> Self {
        let storage = Storage::open_default()
            .or_else(|_| Storage::open_in_memory())
            .expect("sqlite storage");

        let engine = Self {
            demo_mode,
            power: PowerReader::new(background_macmon && !demo_mode),
            processes: ProcessMonitor::new(),
            keep_awake: KeepAwakeManager::new(demo_mode),
            storage: Mutex::new(storage),
            sleep_cache: Mutex::new(None),
            host_name: read_host_name(),
            chip_name: read_chip_name(),
            macos_version: read_macos_version(),
        };

        if !demo_mode {
            let _ = engine.processes.collect(&[]);
        }

        engine
    }

    pub fn is_demo(&self) -> bool {
        self.demo_mode
    }

    /// Park or resume the on-demand `macmon` worker thread when the dashboard hides/shows.
    pub fn set_dashboard_active(&self, active: bool) {
        if !self.demo_mode {
            self.power.set_dashboard_active(active);
        }
    }

    /// Lightweight power-only read for tray bar & 60s SQLite WAL logger (<20 MB idle RAM).
    pub fn read_power_fast(&self) -> PowerSnapshot {
        if self.demo_mode {
            return demo::demo_snapshot().power;
        }
        self.power.read_snapshot(false)
    }

    /// Lightweight tick for the 2.5s tray loop. Only enumerates running apps if a watch-app is configured!
    pub fn tick_tray(&self, record_sample: bool) -> (PowerSnapshot, KeepAwakeState) {
        let power = self.read_power_fast();
        let watched_check = if !self.demo_mode && self.keep_awake.has_watch_app() {
            let names: Vec<String> = self
                .processes
                .collect(&[])
                .groups
                .into_iter()
                .map(|g| g.name)
                .collect();
            Some(names)
        } else {
            None
        };
        let ka = self
            .keep_awake
            .enforce_guardrails(&power, watched_check.as_deref());

        if record_sample && !self.demo_mode {
            if let Some(sys_w) = power.system_watts {
                let sample = SampleRecord {
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                    system_watts: sys_w,
                    battery_pct: power.battery.as_ref().map(|b| b.charge_pct),
                    is_charging: power
                        .battery
                        .as_ref()
                        .map(|b| b.is_charging)
                        .unwrap_or(false),
                    external_connected: power
                        .battery
                        .as_ref()
                        .map(|b| b.external_connected)
                        .unwrap_or(true),
                    cpu_watts: power.components.as_ref().map(|c| c.cpu_watts),
                    gpu_watts: power.components.as_ref().map(|c| c.gpu_watts),
                    ane_watts: power.components.as_ref().map(|c| c.ane_watts),
                    state: format!("{:?}", power.state).to_lowercase(),
                    memory_pressure: "green".into(),
                };
                if let Ok(store) = self.storage.lock() {
                    let _ = store.insert_sample(&sample);
                }
            }
        }

        (power, ka)
    }

    /// Dashboard snapshot split cleanly by view:
    /// - When `include_sleep == false` (`Live` tab): skips `pmset -g log` completely (returns last cached sleep summary or empty placeholder)!
    /// - When `include_sleep == true` (`Sleep & Battery` tab or CLI): streams `pmset -g log` lazily (cached 5 min unless forced).
    pub fn snapshot(
        &self,
        include_sleep: bool,
        force_refresh_sleep: bool,
        sync_macmon_if_empty: bool,
    ) -> DashboardSnapshot {
        if self.demo_mode {
            let mut snap = demo::demo_snapshot();
            snap.keep_awake = self.keep_awake.state();
            return snap;
        }

        let power = self.power.read_snapshot(sync_macmon_if_empty);
        let blockers = collect_sleep_blockers();
        let processes = self.processes.collect(&blockers);

        let app_names: Vec<String> = processes.groups.iter().map(|g| g.name.clone()).collect();
        let keep_awake = self
            .keep_awake
            .enforce_guardrails(&power, Some(&app_names));

        let current_pct = power.battery.as_ref().map(|b| b.charge_pct);
        let on_battery = power
            .battery
            .as_ref()
            .map(|b| !b.external_connected)
            .unwrap_or(false);

        let (sleep, audit) = if include_sleep {
            self.cached_sleep_and_audit(force_refresh_sleep, current_pct, on_battery)
        } else {
            self.cached_sleep_if_present(current_pct, on_battery)
        };

        DashboardSnapshot {
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            demo_mode: false,
            host_name: self.host_name.clone(),
            chip_name: self.chip_name.clone(),
            macos_version: self.macos_version.clone(),
            power,
            sleep,
            blockers,
            keep_awake,
            processes,
            audit,
            history: Vec::new(),
        }
    }

    fn cached_sleep_if_present(
        &self,
        current_pct: Option<u8>,
        on_battery: bool,
    ) -> (SleepReport, Vec<PmsetAuditItem>) {
        if let Ok(guard) = self.sleep_cache.lock() {
            if let Some(cached) = guard.as_ref() {
                return (cached.report.clone(), cached.audit.clone());
            }
        }
        (
            SleepReport {
                since_unplug: SinceUnplugSummary {
                    started_at: None,
                    unplug_ts_ms: None,
                    unplug_source: "unknown".into(),
                    data_status: if on_battery {
                        "insufficient_history".into()
                    } else {
                        "on_ac".into()
                    },
                    history_coverage_secs: 0,
                    elapsed_secs: 0,
                    start_charge_pct: current_pct,
                    current_charge_pct: current_pct,
                    total_drop_pct: 0,
                    asleep_drain_pct: 0,
                    active_drain_pct: 0,
                    sleep_session_count: 0,
                    active_secs: 0,
                    sleep_secs: 0,
                    dark_wake_secs: 0,
                    dark_wake_count: 0,
                    drain_pct_per_hour: 0.0,
                    on_battery_now: on_battery,
                },
                sessions: Vec::new(),
                top_wake_reasons: Vec::new(),
                since_unplug_wake_reasons: Vec::new(),
                timeline: Vec::new(),
                raw_log_available: false,
            },
            Vec::new(),
        )
    }

    fn cached_sleep_and_audit(
        &self,
        force: bool,
        current_pct: Option<u8>,
        on_battery: bool,
    ) -> (SleepReport, Vec<PmsetAuditItem>) {
        if !force {
            if let Ok(guard) = self.sleep_cache.lock() {
                if let Some(cached) = guard.as_ref() {
                    if cached.updated_at.elapsed() < Duration::from_secs(300) {
                        return (cached.report.clone(), cached.audit.clone());
                    }
                }
            }
        }

        let (stored_unplug, earliest_ms) = self
            .storage
            .lock()
            .ok()
            .map(|s| {
                (
                    s.find_last_unplug_transition().ok().flatten(),
                    s.earliest_sample_ts().ok().flatten(),
                )
            })
            .unwrap_or((None, None));

        let report = collect_sleep_report(current_pct, on_battery, stored_unplug, earliest_ms);
        let audit = collect_pmset_audit(report.since_unplug.dark_wake_count);

        if let Ok(mut guard) = self.sleep_cache.lock() {
            *guard = Some(CachedSleep {
                report: report.clone(),
                audit: audit.clone(),
                updated_at: Instant::now(),
            });
        }

        (report, audit)
    }

    pub fn save_window_bounds(&self, bounds: &WindowBounds) {
        if let Ok(store) = self.storage.lock() {
            let _ = store.save_window_bounds(bounds);
        }
    }

    pub fn load_window_bounds(&self) -> Option<WindowBounds> {
        self.storage
            .lock()
            .ok()
            .and_then(|s| s.load_window_bounds().ok().flatten())
    }
}

fn read_host_name() -> String {
    Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Mac".into())
}

fn read_chip_name() -> String {
    Command::new("/usr/sbin/sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Apple Silicon".into())
}

fn read_macos_version() -> String {
    Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| format!("macOS {}", String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or_else(|| "macOS".into())
}
