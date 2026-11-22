use std::ffi::{c_char, c_void, CString};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::types::{AdapterInfo, BatteryInfo, ComponentPower, PowerSnapshot, PowerState};

/// Cached Tier-3 Apple Silicon component metrics from `macmon` 0.10 (sudoless IOReport/CLPC).
#[derive(Debug, Clone)]
pub struct CachedComponentMetrics {
    pub data: ComponentPower,
    pub updated_at: Instant,
}

pub struct PowerReader {
    component_cache: Arc<Mutex<Option<CachedComponentMetrics>>>,
    macmon_available: Arc<Mutex<bool>>,
}

impl PowerReader {
    pub fn new(start_background_macmon: bool) -> Self {
        let component_cache = Arc::new(Mutex::new(None));
        let macmon_available = Arc::new(Mutex::new(false));

        if start_background_macmon {
            #[cfg(target_os = "macos")]
            Self::spawn_macmon_thread(Arc::clone(&component_cache), Arc::clone(&macmon_available));
        }

        Self {
            component_cache,
            macmon_available,
        }
    }

    #[cfg(target_os = "macos")]
    fn spawn_macmon_thread(
        cache: Arc<Mutex<Option<CachedComponentMetrics>>>,
        available: Arc<Mutex<bool>>,
    ) {
        thread::Builder::new()
            .name("wattlog-macmon".into())
            .spawn(move || {
                let mut sampler = match macmon::Sampler::new() {
                    Ok(s) => {
                        if let Ok(mut flag) = available.lock() {
                            *flag = true;
                        }
                        s
                    }
                    Err(_) => {
                        // Graceful fallback if macOS tightens entitlements or on non-Apple Silicon.
                        return;
                    }
                };

                loop {
                    match sampler.get_metrics(1000) {
                        Ok(m) => {
                            let cpu_w = sanitize_watts(m.cpu_power as f64);
                            let gpu_w = sanitize_watts(m.gpu_power as f64);
                            let ane_w = sanitize_watts(m.ane_power as f64);
                            let dram_w = sanitize_watts(m.ram_power as f64);
                            let sys_w = sanitize_watts(m.sys_power as f64);
                            let comp_sum = cpu_w + gpu_w + ane_w + dram_w;
                            let display_other = (sys_w - comp_sum).max(0.0);
                            let fan_rpm = m.fans.first().map(|f| f.rpm);

                            let comp = ComponentPower {
                                cpu_watts: round2(cpu_w),
                                gpu_watts: round2(gpu_w),
                                ane_watts: round2(ane_w),
                                dram_watts: round2(dram_w),
                                sys_watts: round2(sys_w),
                                display_other_watts: round2(display_other),
                                cpu_temp_c: m.temp.cpu_temp_avg.map(|t| round1(t as f64)),
                                gpu_temp_c: m.temp.gpu_temp_avg.map(|t| round1(t as f64)),
                                cpu_active_pct: round1((m.cpu_active_ratio as f64 * 100.0).clamp(0.0, 100.0)),
                                gpu_active_pct: round1((m.gpu_active_ratio as f64 * 100.0).clamp(0.0, 100.0)),
                                fan_rpm,
                                available: true,
                            };
                            if let Ok(mut guard) = cache.lock() {
                                *guard = Some(CachedComponentMetrics {
                                    data: comp,
                                    updated_at: Instant::now(),
                                });
                            }
                        }
                        Err(_) => {
                            thread::sleep(Duration::from_secs(3));
                        }
                    }
                    // Low-overhead cadence between samples (~1s sample + 1s rest = 2s).
                    thread::sleep(Duration::from_millis(1000));
                }
            })
            .ok();
    }

    /// Take a single synchronous macmon sample for CLI (`--plain` / `--json`) mode.
    #[cfg(target_os = "macos")]
    pub fn sample_components_once(&self, interval_ms: u32) -> Option<ComponentPower> {
        if let Ok(guard) = self.component_cache.lock() {
            if let Some(cached) = guard.as_ref() {
                if cached.updated_at.elapsed() < Duration::from_secs(10) {
                    return Some(cached.data.clone());
                }
            }
        }
        let mut sampler = macmon::Sampler::new().ok()?;
        let m = sampler.get_metrics(interval_ms.clamp(120, 1000)).ok()?;
        let cpu_w = sanitize_watts(m.cpu_power as f64);
        let gpu_w = sanitize_watts(m.gpu_power as f64);
        let ane_w = sanitize_watts(m.ane_power as f64);
        let dram_w = sanitize_watts(m.ram_power as f64);
        let sys_w = sanitize_watts(m.sys_power as f64);
        let comp_sum = cpu_w + gpu_w + ane_w + dram_w;
        Some(ComponentPower {
            cpu_watts: round2(cpu_w),
            gpu_watts: round2(gpu_w),
            ane_watts: round2(ane_w),
            dram_watts: round2(dram_w),
            sys_watts: round2(sys_w),
            display_other_watts: round2((sys_w - comp_sum).max(0.0)),
            cpu_temp_c: m.temp.cpu_temp_avg.map(|t| round1(t as f64)),
            gpu_temp_c: m.temp.gpu_temp_avg.map(|t| round1(t as f64)),
            cpu_active_pct: round1((m.cpu_active_ratio as f64 * 100.0).clamp(0.0, 100.0)),
            gpu_active_pct: round1((m.gpu_active_ratio as f64 * 100.0).clamp(0.0, 100.0)),
            fan_rpm: m.fans.first().map(|f| f.rpm),
            available: true,
        })
    }

    pub fn read_snapshot(&self, sync_macmon_if_empty: bool) -> PowerSnapshot {
        #[cfg(target_os = "macos")]
        {
            let mut components = self
                .component_cache
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(|c| c.data.clone()));

            if components.is_none() && sync_macmon_if_empty {
                components = self.sample_components_once(250);
            }

            let smart = macos_iokit::read_apple_smart_battery();
            let smc_pstr = if smart
                .as_ref()
                .map(|s| s.system_load_watts.unwrap_or(0.0) <= 0.05)
                .unwrap_or(true)
            {
                macos_iokit::read_smc_pstr()
            } else {
                None
            };

            if let Some(sb) = smart {
                let signed_batt_w =
                    (sb.battery.amperage_ma as f64 * sb.battery.voltage_mv as f64) / 1_000_000.0;

                // Tier 1: AppleSmartBattery PowerTelemetryData.SystemLoad (most accurate on Apple Silicon)
                // Fallback to |InstantAmperage * Voltage| while discharging, then AppleSMC PSTR, then macmon sys_power.
                let (system_watts, mut tier) = if let Some(load_w) = sb.system_load_watts.filter(|w| *w > 0.05) {
                    (load_w, "AppleSmartBattery".to_string())
                } else if !sb.battery.external_connected && signed_batt_w.abs() > 0.05 {
                    (signed_batt_w.abs(), "AppleSmartBattery (I×V)".to_string())
                } else if let Some(pstr) = smc_pstr.filter(|w| *w > 0.05) {
                    (pstr, "AppleSMC (PSTR)".to_string())
                } else if let Some(comp) = components.as_ref().filter(|c| c.sys_watts > 0.05) {
                    (comp.sys_watts, "macmon (IOReport)".to_string())
                } else {
                    (signed_batt_w.abs(), "AppleSmartBattery".to_string())
                };

                if let Some(ref mut comp) = components {
                    let parts = comp.cpu_watts + comp.gpu_watts + comp.ane_watts + comp.dram_watts;
                    if system_watts > parts {
                        comp.display_other_watts = round2((system_watts - parts).max(0.0));
                    }
                    if comp.available && !tier.contains("macmon") {
                        tier.push_str(" + macmon");
                    }
                }

                let state = if sb.battery.fully_charged && sb.battery.external_connected {
                    PowerState::Full
                } else if sb.battery.is_charging {
                    PowerState::Charging
                } else if sb.battery.external_connected {
                    PowerState::AcOnly
                } else {
                    PowerState::Discharging
                };

                return PowerSnapshot {
                    system_watts: round2(system_watts),
                    battery_watts: round2(signed_batt_w),
                    source_tier: tier,
                    state,
                    battery: Some(sb.battery),
                    adapter: sb.adapter,
                    components,
                };
            }

            // Desktop Mac (Mac Studio / Mac Mini / Mac Pro) — Tier 2 AppleSMC PSTR -> Tier 3 macmon
            let (system_watts, tier) = if let Some(pstr) = smc_pstr.filter(|w| *w > 0.05) {
                (pstr, "AppleSMC (PSTR)".to_string())
            } else if let Some(comp) = components.as_ref().filter(|c| c.sys_watts > 0.05) {
                (comp.sys_watts, "macmon (IOReport)".to_string())
            } else {
                (0.0, "unavailable".to_string())
            };

            return PowerSnapshot {
                system_watts: round2(system_watts),
                battery_watts: 0.0,
                source_tier: tier,
                state: PowerState::AcOnly,
                battery: None,
                adapter: Some(AdapterInfo {
                    connected: true,
                    watts: None,
                    voltage_mv: None,
                    current_ma: None,
                    name: Some("AC Mains".into()),
                    description: Some("Desktop Mac (no battery)".into()),
                    is_wireless: false,
                }),
                components,
            };
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = sync_macmon_if_empty;
            PowerSnapshot {
                system_watts: 0.0,
                battery_watts: 0.0,
                source_tier: "unavailable".into(),
                state: PowerState::AcOnly,
                battery: None,
                adapter: None,
                components: None,
            }
        }
    }

    pub fn is_macmon_active(&self) -> bool {
        self.macmon_available.lock().map(|g| *g).unwrap_or(false)
    }
}

fn sanitize_watts(w: f64) -> f64 {
    if w.is_finite() && w >= 0.0 && w < 400.0 {
        w
    } else {
        0.0
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(target_os = "macos")]
mod macos_iokit {
    use super::*;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef, CFMutableDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_foundation_sys::base::kCFAllocatorDefault;

    type MachPort = u32;
    type IoObject = u32;
    type IoRegistryEntry = u32;
    type IoService = u32;
    type IoConnect = u32;
    type KernReturn = i32;

    const K_IO_MAIN_PORT_DEFAULT: MachPort = 0;
    const KERN_SUCCESS: KernReturn = 0;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
        fn IOServiceGetMatchingService(
            main_port: MachPort,
            matching: CFMutableDictionaryRef,
        ) -> IoService;
        fn IORegistryEntryCreateCFProperties(
            entry: IoRegistryEntry,
            properties: *mut CFMutableDictionaryRef,
            allocator: core_foundation_sys::base::CFAllocatorRef,
            options: u32,
        ) -> KernReturn;
        fn IOObjectRelease(object: IoObject) -> KernReturn;
        fn IOServiceOpen(
            service: IoService,
            owning_task: MachPort,
            conn_type: u32,
            connect: *mut IoConnect,
        ) -> KernReturn;
        fn IOServiceClose(connect: IoConnect) -> KernReturn;
        fn IOConnectCallStructMethod(
            connection: IoConnect,
            selector: u32,
            input_struct: *const c_void,
            input_struct_cnt: usize,
            output_struct: *mut c_void,
            output_struct_cnt: *mut usize,
        ) -> KernReturn;
    }

    pub struct SmartBatteryReading {
        pub battery: BatteryInfo,
        pub adapter: Option<AdapterInfo>,
        pub system_load_watts: Option<f64>,
    }

    fn dict_get_i64(dict: &CFDictionary<CFString, CFType>, key: &str) -> Option<i64> {
        let k = CFString::new(key);
        let val = dict.find(&k)?;
        let num = val.downcast::<CFNumber>()?;
        num.to_i64()
    }

    fn dict_get_bool(dict: &CFDictionary<CFString, CFType>, key: &str) -> Option<bool> {
        let k = CFString::new(key);
        let val = dict.find(&k)?;
        let b = val.downcast::<CFBoolean>()?;
        Some(bool::from(b))
    }

    fn dict_get_string(dict: &CFDictionary<CFString, CFType>, key: &str) -> Option<String> {
        let k = CFString::new(key);
        let val = dict.find(&k)?;
        let s = val.downcast::<CFString>()?;
        Some(s.to_string())
    }

    fn dict_get_subdict(
        dict: &CFDictionary<CFString, CFType>,
        key: &str,
    ) -> Option<CFDictionary<CFString, CFType>> {
        let k = CFString::new(key);
        let val = dict.find(&k)?;
        let sub = val.downcast::<CFDictionary>()?;
        // Safety: AppleSmartBattery sub-dictionaries have CFString keys and CFType values.
        unsafe {
            let cf_ref: CFDictionaryRef = sub.as_concrete_TypeRef();
            Some(CFDictionary::<CFString, CFType>::wrap_under_get_rule(cf_ref))
        }
    }

    /// Tier 1: Direct zero-root IOKit read of `AppleSmartBattery`.
    pub fn read_apple_smart_battery() -> Option<SmartBatteryReading> {
        let cname = CString::new("AppleSmartBattery").ok()?;
        unsafe {
            let matching = IOServiceMatching(cname.as_ptr());
            if matching.is_null() {
                return None;
            }
            let service = IOServiceGetMatchingService(K_IO_MAIN_PORT_DEFAULT, matching);
            if service == 0 {
                return None;
            }

            let mut props_ref: CFMutableDictionaryRef = std::ptr::null_mut();
            let kr = IORegistryEntryCreateCFProperties(
                service,
                &mut props_ref,
                kCFAllocatorDefault,
                0,
            );
            IOObjectRelease(service);

            if kr != KERN_SUCCESS || props_ref.is_null() {
                return None;
            }

            let dict: CFDictionary<CFString, CFType> =
                CFDictionary::wrap_under_create_rule(props_ref as CFDictionaryRef);

            let current_pct = dict_get_i64(&dict, "CurrentCapacity")
                .unwrap_or(0)
                .clamp(0, 100) as u8;
            let raw_current = dict_get_i64(&dict, "AppleRawCurrentCapacity")
                .unwrap_or(0)
                .max(0) as u32;
            let raw_max = dict_get_i64(&dict, "AppleRawMaxCapacity")
                .unwrap_or(0)
                .max(0) as u32;
            let nominal_max = dict_get_i64(&dict, "NominalChargeCapacity")
                .unwrap_or(raw_max as i64)
                .max(0) as u32;
            let design_cap = dict_get_i64(&dict, "DesignCapacity")
                .unwrap_or(0)
                .max(0) as u32;
            let cycle_count = dict_get_i64(&dict, "CycleCount").unwrap_or(0).max(0) as u32;
            let design_cycles = dict_get_i64(&dict, "DesignCycleCount9C")
                .unwrap_or(1000)
                .max(1) as u32;
            let voltage_mv = dict_get_i64(&dict, "Voltage").unwrap_or(0).max(0) as u32;
            // InstantAmperage is signed two's complement 64-bit integer in mA (negative = discharging).
            let amperage_ma = dict_get_i64(&dict, "InstantAmperage").unwrap_or(0);
            let temp_centi = dict_get_i64(&dict, "Temperature").unwrap_or(0);
            let temperature_c = round1(temp_centi as f64 / 100.0);

            let external_connected = dict_get_bool(&dict, "ExternalConnected").unwrap_or(false);
            let is_charging = dict_get_bool(&dict, "IsCharging").unwrap_or(false);
            let fully_charged = dict_get_bool(&dict, "FullyCharged").unwrap_or(false);

            let time_raw = if is_charging {
                dict_get_i64(&dict, "AvgTimeToFull")
            } else {
                dict_get_i64(&dict, "AvgTimeToEmpty")
                    .or_else(|| dict_get_i64(&dict, "TimeRemaining"))
            };
            let time_remaining_mins = time_raw
                .filter(|&v| v > 0 && v < 60_000)
                .map(|v| v as u32);

            let health_pct = if design_cap > 0 {
                round1(((nominal_max as f64 / design_cap as f64) * 100.0).clamp(0.0, 100.0))
            } else {
                100.0
            };
            let raw_health_pct = if design_cap > 0 {
                round1(((raw_max as f64 / design_cap as f64) * 100.0).clamp(0.0, 100.0))
            } else {
                health_pct
            };

            // PowerTelemetryData.SystemLoad is reported in mW on Apple Silicon!
            let system_load_watts = dict_get_subdict(&dict, "PowerTelemetryData").and_then(|ptd| {
                dict_get_i64(&ptd, "SystemLoad")
                    .filter(|&mw| mw > 0 && mw < 400_000)
                    .map(|mw| mw as f64 / 1000.0)
            });

            // AdapterDetails (USB-C PD negotiated wattage / voltage / current)
            let adapter = dict_get_subdict(&dict, "AdapterDetails").map(|ad| {
                let watts = dict_get_i64(&ad, "Watts")
                    .filter(|&w| w > 0)
                    .map(|w| w as u32);
                let v_mv = dict_get_i64(&ad, "AdapterVoltage")
                    .filter(|&v| v > 0)
                    .map(|v| v as u32);
                let c_ma = dict_get_i64(&ad, "Current")
                    .filter(|&c| c > 0)
                    .map(|c| c as u32);
                let name = dict_get_string(&ad, "Name");
                let description = dict_get_string(&ad, "Description");
                let is_wireless = dict_get_bool(&ad, "IsWireless").unwrap_or(false);
                AdapterInfo {
                    connected: external_connected,
                    watts,
                    voltage_mv: v_mv,
                    current_ma: c_ma,
                    name,
                    description,
                    is_wireless,
                }
            });

            Some(SmartBatteryReading {
                battery: BatteryInfo {
                    charge_pct: current_pct,
                    health_pct,
                    raw_health_pct,
                    cycle_count,
                    design_cycle_count: design_cycles,
                    voltage_mv,
                    amperage_ma,
                    temperature_c,
                    time_remaining_mins,
                    external_connected,
                    is_charging,
                    fully_charged,
                    nominal_capacity_mah: nominal_max,
                    raw_max_capacity_mah: raw_max,
                    design_capacity_mah: design_cap,
                    current_capacity_mah: raw_current,
                },
                adapter,
                system_load_watts,
            })
        }
    }

    // ---- Tier 2: Read-only AppleSMC `PSTR` key fallback ----
    #[repr(C)]
    #[derive(Default, Copy, Clone)]
    struct SmcKeyDataVers {
        major: u8,
        minor: u8,
        build: u8,
        reserved: u8,
        release: u16,
    }

    #[repr(C)]
    #[derive(Default, Copy, Clone)]
    struct SmcKeyDataPLimitData {
        version: u16,
        length: u16,
        cpu_p_limit: u32,
        gpu_p_limit: u32,
        mem_p_limit: u32,
    }

    #[repr(C)]
    #[derive(Default, Copy, Clone)]
    struct SmcKeyDataKeyInfo {
        data_size: u32,
        data_type: u32,
        data_attributes: u8,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct SmcKeyData {
        key: u32,
        vers: SmcKeyDataVers,
        p_limit_data: SmcKeyDataPLimitData,
        key_info: SmcKeyDataKeyInfo,
        result: u8,
        status: u8,
        data8: u8,
        data32: u32,
        bytes: [u8; 32],
    }

    impl Default for SmcKeyData {
        fn default() -> Self {
            Self {
                key: 0,
                vers: Default::default(),
                p_limit_data: Default::default(),
                key_info: Default::default(),
                result: 0,
                status: 0,
                data8: 0,
                data32: 0,
                bytes: [0u8; 32],
            }
        }
    }

    fn fourcc(s: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*s)
    }

    /// Read-only AppleSMC `PSTR` (System Total Power) in Watts. Zero-root, read-only selector.
    pub fn read_smc_pstr() -> Option<f64> {
        let cname = CString::new("AppleSMC").ok()?;
        unsafe {
            let matching = IOServiceMatching(cname.as_ptr());
            if matching.is_null() {
                return None;
            }
            let service = IOServiceGetMatchingService(K_IO_MAIN_PORT_DEFAULT, matching);
            if service == 0 {
                return None;
            }
            let mut conn: IoConnect = 0;
            let kr = IOServiceOpen(service, mach2::traps::mach_task_self(), 0, &mut conn);
            IOObjectRelease(service);
            if kr != KERN_SUCCESS || conn == 0 {
                return None;
            }

            let key = fourcc(b"PSTR");
            let mut input = SmcKeyData {
                key,
                data8: 9, // kSMCGetKeyInfo
                ..Default::default()
            };
            let mut output = SmcKeyData::default();
            let mut out_size = std::mem::size_of::<SmcKeyData>();

            let kr1 = IOConnectCallStructMethod(
                conn,
                2, // kSMCHandleYPCEvent
                &input as *const _ as *const c_void,
                std::mem::size_of::<SmcKeyData>(),
                &mut output as *mut _ as *mut c_void,
                &mut out_size,
            );
            if kr1 != KERN_SUCCESS || output.key_info.data_size == 0 {
                IOServiceClose(conn);
                return None;
            }

            let data_type = output.key_info.data_type;
            let data_size = output.key_info.data_size;
            input.key_info.data_size = data_size;
            input.data8 = 5; // kSMCReadKey
            output = SmcKeyData::default();
            out_size = std::mem::size_of::<SmcKeyData>();

            let kr2 = IOConnectCallStructMethod(
                conn,
                2,
                &input as *const _ as *const c_void,
                std::mem::size_of::<SmcKeyData>(),
                &mut output as *mut _ as *mut c_void,
                &mut out_size,
            );
            IOServiceClose(conn);

            if kr2 != KERN_SUCCESS {
                return None;
            }

            // Apple Silicon `PSTR` is IEEE-754 little-endian `flt ` (4 bytes).
            if data_type == fourcc(b"flt ") && data_size == 4 {
                let val = f32::from_le_bytes([
                    output.bytes[0],
                    output.bytes[1],
                    output.bytes[2],
                    output.bytes[3],
                ]) as f64;
                if val.is_finite() && val > 0.0 && val < 400.0 {
                    return Some(val);
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_local_power_snapshot_without_panic() {
        let reader = PowerReader::new(false);
        let snap = reader.read_snapshot(false);
        assert!(snap.system_watts >= 0.0);
        assert!(!snap.source_tier.is_empty());
    }
}
