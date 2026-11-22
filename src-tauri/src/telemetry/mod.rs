pub mod assertions;
pub mod demo;
pub mod power;
pub mod processes;
pub mod sleep;
pub mod storage;
pub mod types;

use std::process::Command;
use std::sync::{Arc, Mutex};
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
    pub demo_mode: Arc<Mutex<bool>>,
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
            demo_mode: Arc::new(Mutex::new(demo_mode)),
            power: PowerReader::new(background_macmon),
            processes: ProcessMonitor::new(),
            keep_awake: KeepAwakeManager::new(),
            storage: Mutex::new(storage),
            sleep_cache: Mutex::new(None),
            host_name: read_host_name(),
            chip_name: read_chip_name(),
            macos_version: read_macos_version(),
        };

        // Prime first process delta sample so the next read has accurate CPU% & wakeups/s.
        if !demo_mode {
            let _ = engine.processes.collect(&[]);
        }

        engine
    }

    pub fn is_demo(&self) -> bool {
        self.demo_mode.lock().map(|g| *g).unwrap_or(false)
    }

    pub fn set_demo(&self, enabled: bool) -> bool {
        if let Ok(mut g) = self.demo_mode.lock() {
            *g = enabled;
        }
        enabled
    }

    /// Lightweight power-only read for tray bar & 60s SQLite WAL logger (<20 MB idle RAM).
    pub fn read_power_fast(&self) -> PowerSnapshot {
        if self.is_demo() {
            return demo::demo_snapshot().power;
        }
        self.power.read_snapshot(false)
    }

    /// Record a 60s background sample to SQLite WAL and enforce Keep-Awake guardrails.
    pub fn tick_background_sample(&self) -> (PowerSnapshot, KeepAwakeState) {
        let power = self.read_power_fast();
        let running_names: Vec<String> = if self.keep_awake.state().watch_app_name.is_some() {
            self.processes
                .collect(&[])
                .groups
                .into_iter()
                .map(|g| g.name)
                .collect()
        } else {
            Vec::new()
        };
        let ka = self.keep_awake.enforce_guardrails(&power, &running_names);

        if !self.is_demo() {
            let sample = SampleRecord {
                timestamp_ms: chrono::Utc::now().timestamp_millis(),
                system_watts: power.system_watts,
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

        (power, ka)
    }

    /// Full dashboard snapshot (used by UI when window is open & by CLI `--plain` / `--json`).
    pub fn snapshot(&self, force_refresh_sleep: bool, sync_macmon_if_empty: bool) -> DashboardSnapshot {
        if self.is_demo() {
            let mut snap = demo::demo_snapshot();
            snap.keep_awake = self.keep_awake.state();
            return snap;
        }

        let power = self.power.read_snapshot(sync_macmon_if_empty);
        let blockers = collect_sleep_blockers();
        let processes = self.processes.collect(&blockers);

        let app_names: Vec<String> = processes.groups.iter().map(|g| g.name.clone()).collect();
        let keep_awake = self.keep_awake.enforce_guardrails(&power, &app_names);

        let current_pct = power.battery.as_ref().map(|b| b.charge_pct);
        let on_battery = power
            .battery
            .as_ref()
            .map(|b| !b.external_connected)
            .unwrap_or(false);

        let (sleep, audit) = self.cached_sleep_and_audit(force_refresh_sleep, current_pct, on_battery);

        let history = self
            .storage
            .lock()
            .ok()
            .and_then(|s| s.recent_samples(24, 360).ok())
            .unwrap_or_default();

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
            history,
        }
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
                    if cached.updated_at.elapsed() < Duration::from_secs(30) {
                        return (cached.report.clone(), cached.audit.clone());
                    }
                }
            }
        }

        let report = collect_sleep_report(current_pct, on_battery);
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
