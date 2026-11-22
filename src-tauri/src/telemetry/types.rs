use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSnapshot {
    pub timestamp_ms: i64,
    pub demo_mode: bool,
    pub host_name: String,
    pub chip_name: String,
    pub macos_version: String,
    pub power: PowerSnapshot,
    pub sleep: SleepReport,
    pub blockers: Vec<SleepBlocker>,
    pub keep_awake: KeepAwakeState,
    pub processes: ProcessReport,
    pub audit: Vec<PmsetAuditItem>,
    pub history: Vec<SampleRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerSnapshot {
    /// Total system draw in Watts (`None` when all power sensors are unavailable).
    pub system_watts: Option<f64>,
    /// Net battery flow in Watts (negative = discharging, positive = charging)
    pub battery_watts: f64,
    /// Power source tier used ("AppleSMC (PSTR)", "AppleSmartBattery", "AppleSmartBattery (In−Batt)", "macmon (SoC only)", "unavailable", etc.)
    pub source_tier: String,
    pub state: PowerState,
    pub battery: Option<BatteryInfo>,
    pub adapter: Option<AdapterInfo>,
    pub components: Option<ComponentPower>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PowerState {
    Discharging,
    Charging,
    Full,
    AcOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryInfo {
    pub charge_pct: u8,
    pub health_pct: f64,
    pub raw_health_pct: f64,
    pub cycle_count: u32,
    pub design_cycle_count: u32,
    pub voltage_mv: u32,
    pub amperage_ma: i64,
    pub temperature_c: f64,
    pub time_remaining_mins: Option<u32>,
    pub external_connected: bool,
    pub is_charging: bool,
    pub fully_charged: bool,
    pub nominal_capacity_mah: u32,
    pub raw_max_capacity_mah: u32,
    pub design_capacity_mah: u32,
    pub current_capacity_mah: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterInfo {
    pub connected: bool,
    pub watts: Option<u32>,
    pub voltage_mv: Option<u32>,
    pub current_ma: Option<u32>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub is_wireless: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentPower {
    pub cpu_watts: f64,
    pub gpu_watts: f64,
    pub ane_watts: f64,
    pub dram_watts: f64,
    pub sys_watts: f64,
    pub display_other_watts: f64,
    pub cpu_temp_c: Option<f64>,
    pub gpu_temp_c: Option<f64>,
    pub cpu_active_pct: f64,
    pub gpu_active_pct: f64,
    pub fan_rpm: Option<u32>,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepReport {
    pub since_unplug: SinceUnplugSummary,
    pub sessions: Vec<SleepSession>,
    pub top_wake_reasons: Vec<WakeReasonAggregate>,
    pub since_unplug_wake_reasons: Vec<WakeReasonAggregate>,
    pub timeline: Vec<TimelineBucket>,
    pub raw_log_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SinceUnplugSummary {
    pub started_at: Option<String>,
    pub unplug_ts_ms: Option<i64>,
    /// "samples" | "pmset" | "unknown"
    pub unplug_source: String,
    /// "ok" | "insufficient_history" | "no_battery_sleep" | "on_ac" | "log_unavailable"
    pub data_status: String,
    pub history_coverage_secs: u64,
    pub elapsed_secs: u64,
    pub start_charge_pct: Option<u8>,
    pub current_charge_pct: Option<u8>,
    pub total_drop_pct: i32,
    pub asleep_drain_pct: i32,
    pub active_drain_pct: i32,
    pub sleep_session_count: u32,
    pub active_secs: u64,
    pub sleep_secs: u64,
    pub dark_wake_secs: u64,
    pub dark_wake_count: u32,
    pub drain_pct_per_hour: f64,
    pub on_battery_now: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepSession {
    pub id: String,
    pub start_time: String,
    pub end_time: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub duration_secs: u64,
    pub start_charge_pct: Option<u8>,
    pub end_charge_pct: Option<u8>,
    pub drain_pct: i32,
    pub power_source: String,
    pub sleep_reason: String,
    pub dark_wake_count: u32,
    pub dark_wake_total_secs: u64,
    pub worst_wake_reason: Option<DecodedWakeReason>,
    pub wake_events: Vec<WakeEvent>,
    pub severity: String, // "ok" | "warning" | "critical"
    pub in_current_unplug: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeEvent {
    pub timestamp: String,
    pub kind: String, // "DarkWake" | "Wake" | "HibernateWake"
    pub duration_secs: u64,
    pub charge_pct: Option<u8>,
    pub power_source: String,
    pub raw_reason: String,
    pub decoded: DecodedWakeReason,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedWakeReason {
    pub token: String,
    pub label: String,
    pub category: String,
    pub explanation: String,
    pub tip: String,
    pub is_insomnia_suspect: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeReasonAggregate {
    pub decoded: DecodedWakeReason,
    pub count: u32,
    pub total_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineBucket {
    pub label: String,
    /// "active" | "darkwake" | "sleep" | "blocked" | "charging" | "empty"
    pub state: String,
    pub watts: f64,
    pub charge_pct: Option<u8>,
    pub duration_secs: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepBlocker {
    pub pid: i32,
    pub start_time_secs: u64,
    pub process_name: String,
    pub app_name: String,
    pub bundle_path: Option<String>,
    pub assertion_type: String,
    pub assertion_name: String,
    pub details: Option<String>,
    pub duration_secs: u64,
    /// "critical" | "warning" | "system"
    pub severity: String,
    pub is_system_protected: bool,
    pub is_self_wattlog: bool,
    pub blocks_display: bool,
    pub blocks_system: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepAwakeConfig {
    pub enabled: bool,
    /// "display" | "system"
    pub mode: String,
    /// None = indefinite, Some(mins) = timed
    pub duration_mins: Option<u32>,
    pub auto_off_below_battery_pct: Option<u8>,
    pub auto_off_on_unplug: bool,
    pub watch_app_name: Option<String>,
}

impl Default for KeepAwakeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: "display".to_string(),
            duration_mins: Some(60),
            auto_off_below_battery_pct: Some(20),
            auto_off_on_unplug: false,
            watch_app_name: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepAwakeState {
    pub active: bool,
    pub mode: String,
    pub remaining_secs: Option<u64>,
    pub started_at_ms: Option<i64>,
    pub auto_off_below_battery_pct: Option<u8>,
    pub auto_off_on_unplug: bool,
    pub watch_app_name: Option<String>,
    pub watch_app_running: bool,
    pub last_auto_release_reason: Option<String>,
    pub assertion_id: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessReport {
    pub groups: Vec<AppProcessGroup>,
    pub memory_pressure: MemoryPressureInfo,
    pub total_cpu_pct: f64,
    pub total_footprint_bytes: u64,
    pub kernel_thermal_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppProcessGroup {
    pub id: String,
    pub name: String,
    pub primary_pid: i32,
    pub primary_start_time_secs: u64,
    pub bundle_path: Option<String>,
    pub helper_count: usize,
    pub cpu_pct: f64,
    pub footprint_bytes: u64,
    pub energy_impact: f64,
    pub wakeups_per_sec: f64,
    /// "runaway" | "blocking" | "elevated" | "system" | "normal"
    pub verdict: String,
    pub is_system_protected: bool,
    pub is_blocking_sleep: bool,
    pub mostly_in: Option<String>,
    pub children: Vec<ProcessRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRow {
    pub pid: i32,
    pub ppid: i32,
    pub start_time_secs: u64,
    pub name: String,
    pub path: Option<String>,
    pub cpu_pct: f64,
    pub footprint_bytes: u64,
    pub energy_impact: f64,
    pub wakeups_per_sec: f64,
    pub is_system_protected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPressureInfo {
    /// "green" | "yellow" | "red"
    pub level: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub wired_bytes: u64,
    pub compressed_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PmsetAuditItem {
    pub id: String,
    pub profile: String,
    pub key: String,
    pub current_value: String,
    pub recommended_value: String,
    /// "critical" | "warning" | "ok"
    pub severity: String,
    pub title: String,
    pub explanation: String,
    pub fix_command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleRecord {
    pub timestamp_ms: i64,
    pub system_watts: f64,
    pub battery_pct: Option<u8>,
    pub is_charging: bool,
    pub external_connected: bool,
    pub cpu_watts: Option<f64>,
    pub gpu_watts: Option<f64>,
    pub ane_watts: Option<f64>,
    pub state: String,
    pub memory_pressure: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult {
    pub ok: bool,
    pub message: String,
}
