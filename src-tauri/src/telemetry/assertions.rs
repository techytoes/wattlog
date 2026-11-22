use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::processes::{extract_app_name_from_path, is_system_protected_name, process_path_for_pid};
use super::types::{KeepAwakeConfig, KeepAwakeState, PowerSnapshot, SleepBlocker};

const WATTLOG_ASSERTION_TAG: &str = "Wattlog Keep-Awake";

/// Read live macOS sleep-blocking assertions natively via `IOPMCopyAssertionsByProcess`.
pub fn collect_sleep_blockers() -> Vec<SleepBlocker> {
    #[cfg(target_os = "macos")]
    {
        macos_iopm::copy_assertions_by_process()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// Safe-by-Default Keep-Awake (Caffeine) controller backed by `IOPMAssertionCreateWithName`.
/// Automatically kernel-released if the process exits or crashes.
pub struct KeepAwakeManager {
    inner: Arc<Mutex<KeepAwakeInner>>,
}

struct KeepAwakeInner {
    config: KeepAwakeConfig,
    assertion_id: Option<u32>,
    started_at: Option<Instant>,
    started_at_ms: Option<i64>,
    expires_at: Option<Instant>,
    last_auto_release_reason: Option<String>,
    watch_app_running: bool,
    was_external_connected: Option<bool>,
}

impl KeepAwakeManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(KeepAwakeInner {
                config: KeepAwakeConfig::default(),
                assertion_id: None,
                started_at: None,
                started_at_ms: None,
                expires_at: None,
                last_auto_release_reason: None,
                watch_app_running: false,
                was_external_connected: None,
            })),
        }
    }

    pub fn toggle_quick(&self, power: &PowerSnapshot) -> Result<KeepAwakeState, String> {
        let mut guard = self.inner.lock().map_err(|_| "lock poisoned".to_string())?;
        if guard.assertion_id.is_some() {
            release_inner(&mut guard, None);
            guard.config.enabled = false;
            return Ok(build_state(&guard));
        }

        let mut cfg = guard.config.clone();
        cfg.enabled = true;
        apply_config_inner(&mut guard, cfg, power)
    }

    pub fn apply_config(
        &self,
        config: KeepAwakeConfig,
        power: &PowerSnapshot,
    ) -> Result<KeepAwakeState, String> {
        let mut guard = self.inner.lock().map_err(|_| "lock poisoned".to_string())?;
        apply_config_inner(&mut guard, config, power)
    }

    /// Periodic guardrail check (called by background ticker & before snapshot).
    pub fn enforce_guardrails(&self, power: &PowerSnapshot, running_apps: &[String]) -> KeepAwakeState {
        let Ok(mut guard) = self.inner.lock() else {
            return KeepAwakeState {
                active: false,
                mode: "display".into(),
                remaining_secs: None,
                started_at_ms: None,
                auto_off_below_battery_pct: Some(20),
                auto_off_on_unplug: false,
                watch_app_name: None,
                watch_app_running: false,
                last_auto_release_reason: None,
                assertion_id: None,
            };
        };

        let ext_now = power.battery.as_ref().map(|b| b.external_connected);
        let prev_ext = guard.was_external_connected;
        guard.was_external_connected = ext_now;

        if let Some(ref target_app) = guard.config.watch_app_name {
            let needle = target_app.to_ascii_lowercase();
            guard.watch_app_running = running_apps
                .iter()
                .any(|a| a.to_ascii_lowercase().contains(&needle));
        } else {
            guard.watch_app_running = false;
        }

        // If watch-app trigger is configured, automatically activate when app is running (subject to battery safety).
        if guard.assertion_id.is_none()
            && guard.config.watch_app_name.is_some()
            && guard.watch_app_running
            && !is_below_battery_floor(&guard.config, power)
        {
            let mut cfg = guard.config.clone();
            cfg.enabled = true;
            let _ = apply_config_inner(&mut guard, cfg, power);
        }

        if guard.assertion_id.is_some() {
            // 1. Timer expiration
            if let Some(exp) = guard.expires_at {
                if Instant::now() >= exp {
                    release_inner(&mut guard, Some("Keep-Awake timer finished".into()));
                    guard.config.enabled = false;
                }
            }

            // 2. Battery floor guardrail (ON by default at 20% while discharging)
            if guard.assertion_id.is_some() && is_below_battery_floor(&guard.config, power) {
                let floor = guard.config.auto_off_below_battery_pct.unwrap_or(20);
                release_inner(
                    &mut guard,
                    Some(format!("Auto-released: battery dropped below {floor}%")),
                );
                guard.config.enabled = false;
            }

            // 3. Auto-release on AC unplug
            if guard.assertion_id.is_some()
                && guard.config.auto_off_on_unplug
                && prev_ext == Some(true)
                && ext_now == Some(false)
            {
                release_inner(
                    &mut guard,
                    Some("Auto-released: MacBook unplugged from AC".into()),
                );
                guard.config.enabled = false;
            }

            // 4. Watch-app exited
            if guard.assertion_id.is_some()
                && guard.config.watch_app_name.is_some()
                && !guard.watch_app_running
            {
                let name = guard.config.watch_app_name.clone().unwrap_or_default();
                release_inner(
                    &mut guard,
                    Some(format!("Auto-released: {name} is no longer running")),
                );
                guard.config.enabled = false;
            }
        }

        build_state(&guard)
    }

    pub fn state(&self) -> KeepAwakeState {
        let Ok(guard) = self.inner.lock() else {
            return KeepAwakeState {
                active: false,
                mode: "display".into(),
                remaining_secs: None,
                started_at_ms: None,
                auto_off_below_battery_pct: Some(20),
                auto_off_on_unplug: false,
                watch_app_name: None,
                watch_app_running: false,
                last_auto_release_reason: None,
                assertion_id: None,
            };
        };
        build_state(&guard)
    }
}

impl Drop for KeepAwakeManager {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.inner.lock() {
            release_inner(&mut guard, None);
        }
    }
}

fn is_below_battery_floor(cfg: &KeepAwakeConfig, power: &PowerSnapshot) -> bool {
    let Some(floor) = cfg.auto_off_below_battery_pct else {
        return false;
    };
    let Some(ref batt) = power.battery else {
        return false;
    };
    !batt.external_connected && batt.charge_pct < floor
}

fn apply_config_inner(
    guard: &mut KeepAwakeInner,
    config: KeepAwakeConfig,
    power: &PowerSnapshot,
) -> Result<KeepAwakeState, String> {
    // Release any prior assertion before changing mode or disabling.
    release_inner(guard, None);
    guard.config = config.clone();

    if !config.enabled {
        return Ok(build_state(guard));
    }

    if is_below_battery_floor(&config, power) {
        let floor = config.auto_off_below_battery_pct.unwrap_or(20);
        guard.last_auto_release_reason = Some(format!(
            "Blocked by safety guardrail: battery is below {floor}%"
        ));
        guard.config.enabled = false;
        return Ok(build_state(guard));
    }

    let assertion_type = if config.mode == "system" {
        "PreventUserIdleSystemSleep"
    } else {
        "PreventUserIdleDisplaySleep"
    };

    let label = match config.duration_mins {
        Some(m) => format!("{WATTLOG_ASSERTION_TAG} ({} · {m}m)", config.mode),
        None => format!("{WATTLOG_ASSERTION_TAG} ({})", config.mode),
    };

    #[cfg(target_os = "macos")]
    let id = macos_iopm::create_assertion(assertion_type, &label)?;
    #[cfg(not(target_os = "macos"))]
    let id = {
        let _ = (assertion_type, label);
        1u32
    };

    let now = Instant::now();
    guard.assertion_id = Some(id);
    guard.started_at = Some(now);
    guard.started_at_ms = Some(chrono::Utc::now().timestamp_millis());
    guard.expires_at = config
        .duration_mins
        .map(|mins| now + Duration::from_secs(mins as u64 * 60));
    guard.last_auto_release_reason = None;

    Ok(build_state(guard))
}

fn release_inner(guard: &mut KeepAwakeInner, reason: Option<String>) {
    if let Some(id) = guard.assertion_id.take() {
        #[cfg(target_os = "macos")]
        let _ = macos_iopm::release_assertion(id);
        #[cfg(not(target_os = "macos"))]
        let _ = id;
    }
    guard.started_at = None;
    guard.started_at_ms = None;
    guard.expires_at = None;
    if let Some(r) = reason {
        guard.last_auto_release_reason = Some(r);
    }
}

fn build_state(guard: &KeepAwakeInner) -> KeepAwakeState {
    let remaining_secs = guard.expires_at.map(|exp| {
        let now = Instant::now();
        if exp > now {
            (exp - now).as_secs()
        } else {
            0
        }
    });

    KeepAwakeState {
        active: guard.assertion_id.is_some(),
        mode: guard.config.mode.clone(),
        remaining_secs,
        started_at_ms: guard.started_at_ms,
        auto_off_below_battery_pct: guard.config.auto_off_below_battery_pct,
        auto_off_on_unplug: guard.config.auto_off_on_unplug,
        watch_app_name: guard.config.watch_app_name.clone(),
        watch_app_running: guard.watch_app_running,
        last_auto_release_reason: guard.last_auto_release_reason.clone(),
        assertion_id: guard.assertion_id,
    }
}

#[cfg(target_os = "macos")]
mod macos_iopm {
    use super::*;
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::date::CFDate;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};

    type IoReturn = i32;
    type IopmAssertionId = u32;
    type IopmAssertionLevel = u32;

    const K_IO_RETURN_SUCCESS: IoReturn = 0;
    const K_IOPM_ASSERTION_LEVEL_ON: IopmAssertionLevel = 255;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMCopyAssertionsByProcess(assertions_by_pid: *mut CFDictionaryRef) -> IoReturn;
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            assertion_level: IopmAssertionLevel,
            assertion_name: CFStringRef,
            assertion_id: *mut IopmAssertionId,
        ) -> IoReturn;
        fn IOPMAssertionRelease(assertion_id: IopmAssertionId) -> IoReturn;
    }

    pub fn create_assertion(assertion_type: &str, name: &str) -> Result<u32, String> {
        let cf_type = CFString::new(assertion_type);
        let cf_name = CFString::new(name);
        let mut id: IopmAssertionId = 0;
        let kr = unsafe {
            IOPMAssertionCreateWithName(
                cf_type.as_concrete_TypeRef(),
                K_IOPM_ASSERTION_LEVEL_ON,
                cf_name.as_concrete_TypeRef(),
                &mut id,
            )
        };
        if kr == K_IO_RETURN_SUCCESS {
            Ok(id)
        } else {
            Err(format!("IOPMAssertionCreateWithName failed (code {kr:#x})"))
        }
    }

    pub fn release_assertion(id: u32) -> Result<(), String> {
        let kr = unsafe { IOPMAssertionRelease(id) };
        if kr == K_IO_RETURN_SUCCESS {
            Ok(())
        } else {
            Err(format!("IOPMAssertionRelease failed (code {kr:#x})"))
        }
    }

    fn dict_str(d: &CFDictionary<CFString, CFType>, key: &str) -> Option<String> {
        let k = CFString::new(key);
        d.find(&k)?.downcast::<CFString>().map(|s| s.to_string())
    }

    fn dict_i64(d: &CFDictionary<CFString, CFType>, key: &str) -> Option<i64> {
        let k = CFString::new(key);
        d.find(&k)?.downcast::<CFNumber>()?.to_i64()
    }

    fn dict_duration_secs(d: &CFDictionary<CFString, CFType>) -> u64 {
        // `AssertStartWhen` / `AssertionOnSince` is a CFDate (seconds since 2001-01-01).
        let now_cf = CFDate::now().abs_time();
        for key in ["AssertionOnSince", "AssertStartWhen"] {
            let k = CFString::new(key);
            if let Some(val) = d.find(&k) {
                if let Some(date) = val.downcast::<CFDate>() {
                    let delta = now_cf - date.abs_time();
                    if delta >= 0.0 && delta < 86_400.0 * 365.0 {
                        return delta as u64;
                    }
                }
            }
        }
        0
    }

    pub fn copy_assertions_by_process() -> Vec<SleepBlocker> {
        let mut raw_dict: CFDictionaryRef = std::ptr::null();
        let kr = unsafe { IOPMCopyAssertionsByProcess(&mut raw_dict) };
        if kr != K_IO_RETURN_SUCCESS || raw_dict.is_null() {
            return Vec::new();
        }

        let top_dict: CFDictionary<CFNumber, CFArray<CFType>> =
            unsafe { CFDictionary::wrap_under_create_rule(raw_dict) };

        let (keys, values) = top_dict.get_keys_and_values();
        let my_pid = std::process::id() as i32;
        let mut out: Vec<SleepBlocker> = Vec::new();

        for (k_ptr, v_ptr) in keys.into_iter().zip(values.into_iter()) {
            let pid_num = unsafe { CFNumber::wrap_under_get_rule(k_ptr as _) };
            let pid = pid_num.to_i32().unwrap_or(0);
            let arr = unsafe { CFArray::<CFType>::wrap_under_get_rule(v_ptr as _) };

            let proc_path = process_path_for_pid(pid);
            let proc_name = proc_path
                .as_deref()
                .and_then(|p| std::path::Path::new(p).file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            let (app_name, bundle_path) = match proc_path.as_deref() {
                Some(p) => extract_app_name_from_path(p, &proc_name),
                None => (proc_name.clone(), None),
            };
            let is_sys = is_system_protected_name(&proc_name, pid);

            for item in arr.iter() {
                let Some(sub) = item.downcast::<CFDictionary>() else {
                    continue;
                };
                let d: CFDictionary<CFString, CFType> = unsafe {
                    CFDictionary::wrap_under_get_rule(sub.as_concrete_TypeRef())
                };

                let assert_type = dict_str(&d, "AssertType").unwrap_or_default();
                // Filter to actual sleep-blocking assertion types (skip noisy UserIsActive tickles).
                let blocks_display = matches!(
                    assert_type.as_str(),
                    "PreventUserIdleDisplaySleep"
                        | "NoDisplaySleepAssertion"
                        | "InternalPreventDisplaySleep"
                );
                let blocks_system = matches!(
                    assert_type.as_str(),
                    "PreventUserIdleSystemSleep"
                        | "NoIdleSleepAssertion"
                        | "PreventSystemSleep"
                        | "PreventUserIdleDisplaySleep"
                        | "NoDisplaySleepAssertion"
                );
                if !blocks_display && !blocks_system {
                    continue;
                }

                let assert_name = dict_str(&d, "AssertName")
                    .unwrap_or_else(|| "Unnamed sleep assertion".into());
                let details = dict_str(&d, "Details").or_else(|| {
                    dict_i64(&d, "AssertPID")
                        .filter(|&p| p as i32 != pid && p > 0)
                        .map(|p| format!("Created on behalf of PID {p}"))
                });
                let duration_secs = dict_duration_secs(&d);
                let is_self = pid == my_pid || assert_name.contains(WATTLOG_ASSERTION_TAG);

                let severity = if is_self {
                    "warning"
                } else if is_sys
                    || assert_type == "InternalPreventDisplaySleep"
                    || proc_name == "powerd"
                {
                    "system"
                } else if blocks_display || assert_type == "PreventSystemSleep" {
                    "critical"
                } else {
                    "warning"
                };

                out.push(SleepBlocker {
                    pid,
                    process_name: proc_name.clone(),
                    app_name: app_name.clone(),
                    bundle_path: bundle_path.clone(),
                    assertion_type: assert_type,
                    assertion_name: assert_name,
                    details,
                    duration_secs,
                    severity: severity.into(),
                    is_system_protected: is_sys,
                    is_self_wattlog: is_self,
                    blocks_display,
                    blocks_system,
                });
            }
        }

        // Sort: non-system critical first, then warning, then system; longer durations first.
        out.sort_by(|a, b| {
            let rank = |s: &str| match s {
                "critical" => 0,
                "warning" => 1,
                _ => 2,
            };
            rank(&a.severity)
                .cmp(&rank(&b.severity))
                .then(b.duration_secs.cmp(&a.duration_secs))
        });

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_native_sleep_blockers() {
        let blockers = collect_sleep_blockers();
        // Should never panic and on a live awake Mac typically contains powerd or app assertions.
        assert!(blockers.len() < 10_000);
    }
}
