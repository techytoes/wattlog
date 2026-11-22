use super::sleep::decode_wake_reason;
use super::types::*;

pub fn demo_snapshot() -> DashboardSnapshot {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let power = PowerSnapshot {
        system_watts: Some(8.4),
        battery_watts: -8.4,
        source_tier: "AppleSMC (PSTR) + macmon (demo)".into(),
        state: PowerState::Discharging,
        battery: Some(BatteryInfo {
            charge_pct: 74,
            health_pct: 91.2,
            raw_health_pct: 89.4,
            cycle_count: 184,
            design_cycle_count: 1000,
            voltage_mv: 11890,
            amperage_ma: -706,
            temperature_c: 31.4,
            time_remaining_mins: Some(372),
            external_connected: false,
            is_charging: false,
            fully_charged: false,
            nominal_capacity_mah: 5699,
            raw_max_capacity_mah: 5586,
            design_capacity_mah: 6249,
            current_capacity_mah: 4133,
        }),
        adapter: Some(AdapterInfo {
            connected: false,
            watts: None,
            voltage_mv: None,
            current_ma: None,
            name: None,
            description: Some("Unplugged · USB-C PD ready".into()),
            is_wireless: false,
        }),
        components: Some(ComponentPower {
            cpu_watts: 3.1,
            gpu_watts: 1.4,
            ane_watts: 0.2,
            dram_watts: 0.7,
            sys_watts: 8.4,
            display_other_watts: 3.0,
            cpu_temp_c: Some(47.2),
            gpu_temp_c: Some(43.8),
            cpu_active_pct: 24.5,
            gpu_active_pct: 11.2,
            fan_rpm: Some(0),
            available: true,
        }),
    };

    let wifibt = decode_wake_reason("smc.sysState.Wake(0x70070000) wifibt SMC.OutboxNotEmpty/");
    let rtc = decode_wake_reason("NUB.SPMI0.SW3.IRQ nub-spmi0.0x02 rtc/Maintenance");
    let usbc = decode_wake_reason("EC.USBC PD renegotiation");
    let lid = decode_wake_reason("lid UserActivity");

    let unplug_ts_ms = now_ms - 77_200 * 1000;
    let sess1_start = now_ms - 59_000 * 1000;
    let sess1_end = sess1_start + 30_240 * 1000;
    let sess2_start = now_ms - 95_000 * 1000;
    let sess2_end = sess2_start + 5_700 * 1000;

    let sessions = vec![
        SleepSession {
            id: "demo-sleep-1".into(),
            start_time: "Nov 21 23:18".into(),
            end_time: "Nov 22 07:42".into(),
            start_ms: sess1_start,
            end_ms: sess1_end,
            duration_secs: 30_240,
            start_charge_pct: Some(88),
            end_charge_pct: Some(71),
            drain_pct: 17,
            power_source: "BATT".into(),
            sleep_reason: "Clamshell Sleep".into(),
            dark_wake_count: 26,
            dark_wake_total_secs: 418,
            worst_wake_reason: Some(wifibt.clone()),
            wake_events: vec![
                WakeEvent {
                    timestamp: "Nov 21 23:44".into(),
                    kind: "DarkWake".into(),
                    duration_secs: 21,
                    charge_pct: Some(87),
                    power_source: "BATT".into(),
                    raw_reason: "smc.sysState.Wake(0x70070000) wifibt SMC.OutboxNotEmpty/".into(),
                    decoded: wifibt.clone(),
                },
                WakeEvent {
                    timestamp: "Nov 22 01:12".into(),
                    kind: "DarkWake".into(),
                    duration_secs: 19,
                    charge_pct: Some(83),
                    power_source: "BATT".into(),
                    raw_reason: "NUB.SPMI0.SW3.IRQ nub-spmi0.0x02 rtc/Maintenance".into(),
                    decoded: rtc.clone(),
                },
                WakeEvent {
                    timestamp: "Nov 22 03:05".into(),
                    kind: "DarkWake".into(),
                    duration_secs: 16,
                    charge_pct: Some(79),
                    power_source: "BATT".into(),
                    raw_reason: "smc.sysState.Wake(0x70070000) wifibt SMC.OutboxNotEmpty/".into(),
                    decoded: wifibt.clone(),
                },
                WakeEvent {
                    timestamp: "Nov 22 07:42".into(),
                    kind: "Wake".into(),
                    duration_secs: 0,
                    charge_pct: Some(71),
                    power_source: "BATT".into(),
                    raw_reason: "lid / UserActivity Assertion".into(),
                    decoded: lid.clone(),
                },
            ],
            severity: "critical".into(),
            in_current_unplug: true,
        },
        SleepSession {
            id: "demo-sleep-2".into(),
            start_time: "Nov 21 14:10".into(),
            end_time: "Nov 21 15:45".into(),
            start_ms: sess2_start,
            end_ms: sess2_end,
            duration_secs: 5_700,
            start_charge_pct: Some(96),
            end_charge_pct: Some(95),
            drain_pct: 1,
            power_source: "BATT".into(),
            sleep_reason: "Idle Sleep".into(),
            dark_wake_count: 2,
            dark_wake_total_secs: 28,
            worst_wake_reason: Some(usbc.clone()),
            wake_events: vec![WakeEvent {
                timestamp: "Nov 21 14:52".into(),
                kind: "DarkWake".into(),
                duration_secs: 14,
                charge_pct: Some(96),
                power_source: "BATT".into(),
                raw_reason: "EC.USBC".into(),
                decoded: usbc.clone(),
            }],
            severity: "ok".into(),
            in_current_unplug: false,
        },
    ];

    let timeline: Vec<TimelineBucket> = (0..48)
        .map(|i| {
            let hour = (15 + (i / 2)) % 24;
            let min = (i % 2) * 30;
            let label = format!("{hour:02}:{min:02}");
            let (state, watts, pct, detail) = if i < 6 {
                ("charging", 42.0, 92 + (i as u8), "USB-C PD 67W charging")
            } else if i < 14 {
                ("active", 9.6, 98 - ((i - 6) as u8), "Active coding session")
            } else if i < 34 {
                if i % 4 == 0 {
                    ("darkwake", 3.4, 88 - (((i - 14) * 17) / 20) as u8, "3 silent wakes · Wi-Fi / Bluetooth")
                } else {
                    ("sleep", 0.3, 88 - (((i - 14) * 17) / 20) as u8, "Asleep (Deep Idle)")
                }
            } else if i < 39 {
                ("blocked", 11.2, 77 - ((i - 34) as u8), "Sleep blocked by Chrome WebRTC")
            } else {
                ("active", 8.4, 74, "Active on battery")
            };
            TimelineBucket {
                label,
                state: state.into(),
                watts,
                charge_pct: Some(pct.min(100)),
                duration_secs: 1800,
                detail: detail.into(),
            }
        })
        .collect();

    let unplug_reasons = vec![
        WakeReasonAggregate {
            decoded: wifibt.clone(),
            count: 19,
            total_secs: 304,
        },
        WakeReasonAggregate {
            decoded: rtc.clone(),
            count: 7,
            total_secs: 114,
        },
    ];

    let sleep = SleepReport {
        since_unplug: SinceUnplugSummary {
            started_at: Some("Nov 21 18:10".into()),
            unplug_ts_ms: Some(unplug_ts_ms),
            unplug_source: "samples".into(),
            data_status: "ok".into(),
            history_coverage_secs: 86_400,
            elapsed_secs: 77_200,
            start_charge_pct: Some(98),
            current_charge_pct: Some(74),
            total_drop_pct: 24,
            asleep_drain_pct: 17,
            active_drain_pct: 7,
            sleep_session_count: 1,
            active_secs: 46_542,
            sleep_secs: 30_240,
            dark_wake_secs: 418,
            dark_wake_count: 26,
            drain_pct_per_hour: 1.1,
            on_battery_now: true,
        },
        sessions,
        top_wake_reasons: vec![
            WakeReasonAggregate {
                decoded: wifibt,
                count: 19,
                total_secs: 304,
            },
            WakeReasonAggregate {
                decoded: rtc,
                count: 7,
                total_secs: 114,
            },
            WakeReasonAggregate {
                decoded: usbc,
                count: 2,
                total_secs: 28,
            },
        ],
        since_unplug_wake_reasons: unplug_reasons,
        timeline,
        raw_log_available: true,
    };

    let blockers = vec![
        SleepBlocker {
            pid: 48210,
            start_time_secs: 1_700_000_001,
            process_name: "Google Chrome Helper (Renderer)".into(),
            app_name: "Google Chrome".into(),
            bundle_path: Some("/Applications/Google Chrome.app".into()),
            assertion_type: "PreventUserIdleDisplaySleep".into(),
            assertion_name: "WebRTC active audio/video session (meet.google.com)".into(),
            details: Some("Blocks display & idle sleep while unplugged".into()),
            duration_secs: 4_920,
            severity: "critical".into(),
            is_system_protected: false,
            is_self_wattlog: false,
            blocks_display: true,
            blocks_system: true,
        },
        SleepBlocker {
            pid: 51904,
            start_time_secs: 1_700_000_002,
            process_name: "caffeinate".into(),
            app_name: "caffeinate".into(),
            bundle_path: None,
            assertion_type: "PreventUserIdleSystemSleep".into(),
            assertion_name: "caffeinate command-line tool".into(),
            details: Some("Started from terminal session (PID 41200)".into()),
            duration_secs: 1_845,
            severity: "warning".into(),
            is_system_protected: false,
            is_self_wattlog: false,
            blocks_display: false,
            blocks_system: true,
        },
        SleepBlocker {
            pid: 344,
            start_time_secs: 1_700_000_000,
            process_name: "powerd".into(),
            app_name: "powerd".into(),
            bundle_path: None,
            assertion_type: "PreventUserIdleSystemSleep".into(),
            assertion_name: "Powerd - Prevent sleep while display is on".into(),
            details: Some("Normal macOS assertion while screen is awake".into()),
            duration_secs: 8_112,
            severity: "system".into(),
            is_system_protected: true,
            is_self_wattlog: false,
            blocks_display: false,
            blocks_system: true,
        },
    ];

    let processes = ProcessReport {
        groups: vec![
            AppProcessGroup {
                id: "app:Google Chrome".into(),
                name: "Google Chrome".into(),
                primary_pid: 48102,
                primary_start_time_secs: 1_700_000_010,
                bundle_path: Some("/Applications/Google Chrome.app".into()),
                helper_count: 18,
                cpu_pct: 42.4,
                footprint_bytes: 6_871_947_673,
                energy_impact: 54.8,
                wakeups_per_sec: 842.0,
                verdict: "runaway".into(),
                is_system_protected: false,
                is_blocking_sleep: true,
                mostly_in: Some("Chrome Helper (Renderer) · 4.1 GB · 842 wakeups/s".into()),
                children: vec![
                    ProcessRow {
                        pid: 48210,
                        ppid: 48102,
                        start_time_secs: 1_700_000_001,
                        name: "Google Chrome Helper (Renderer)".into(),
                        path: Some("/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Helpers/Google Chrome Helper (Renderer).app".into()),
                        cpu_pct: 28.6,
                        footprint_bytes: 4_402_341_478,
                        energy_impact: 38.2,
                        wakeups_per_sec: 610.0,
                        is_system_protected: false,
                    },
                    ProcessRow {
                        pid: 48144,
                        ppid: 48102,
                        start_time_secs: 1_700_000_011,
                        name: "Google Chrome Helper (GPU)".into(),
                        path: None,
                        cpu_pct: 8.9,
                        footprint_bytes: 1_288_490_188,
                        energy_impact: 11.4,
                        wakeups_per_sec: 164.0,
                        is_system_protected: false,
                    },
                    ProcessRow {
                        pid: 48102,
                        ppid: 1,
                        start_time_secs: 1_700_000_010,
                        name: "Google Chrome".into(),
                        path: Some("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into()),
                        cpu_pct: 4.9,
                        footprint_bytes: 1_181_116_007,
                        energy_impact: 5.2,
                        wakeups_per_sec: 68.0,
                        is_system_protected: false,
                    },
                ],
            },
            AppProcessGroup {
                id: "app:Cursor".into(),
                name: "Cursor".into(),
                primary_pid: 39011,
                primary_start_time_secs: 1_700_000_020,
                bundle_path: Some("/Applications/Cursor.app".into()),
                helper_count: 11,
                cpu_pct: 18.7,
                footprint_bytes: 3_435_973_836,
                energy_impact: 22.1,
                wakeups_per_sec: 312.0,
                verdict: "elevated".into(),
                is_system_protected: false,
                is_blocking_sleep: false,
                mostly_in: Some("Cursor Helper (Plugin) · 1.9 GB · 214 wakeups/s".into()),
                children: vec![
                    ProcessRow {
                        pid: 39118,
                        ppid: 39011,
                        start_time_secs: 1_700_000_021,
                        name: "Cursor Helper (Plugin)".into(),
                        path: None,
                        cpu_pct: 12.1,
                        footprint_bytes: 2_040_109_465,
                        energy_impact: 14.3,
                        wakeups_per_sec: 214.0,
                        is_system_protected: false,
                    },
                    ProcessRow {
                        pid: 39011,
                        ppid: 1,
                        start_time_secs: 1_700_000_020,
                        name: "Cursor".into(),
                        path: Some("/Applications/Cursor.app/Contents/MacOS/Cursor".into()),
                        cpu_pct: 6.6,
                        footprint_bytes: 1_395_864_371,
                        energy_impact: 7.8,
                        wakeups_per_sec: 98.0,
                        is_system_protected: false,
                    },
                ],
            },
            AppProcessGroup {
                id: "app:Slack".into(),
                name: "Slack".into(),
                primary_pid: 27410,
                primary_start_time_secs: 1_700_000_030,
                bundle_path: Some("/Applications/Slack.app".into()),
                helper_count: 6,
                cpu_pct: 5.4,
                footprint_bytes: 1_181_116_006,
                energy_impact: 7.9,
                wakeups_per_sec: 128.0,
                verdict: "normal".into(),
                is_system_protected: false,
                is_blocking_sleep: false,
                mostly_in: Some("Slack Helper (Renderer) · 720 MB · 92 wakeups/s".into()),
                children: vec![ProcessRow {
                    pid: 27451,
                    ppid: 27410,
                    start_time_secs: 1_700_000_031,
                    name: "Slack Helper (Renderer)".into(),
                    path: None,
                    cpu_pct: 4.1,
                    footprint_bytes: 754_974_720,
                    energy_impact: 5.8,
                    wakeups_per_sec: 92.0,
                    is_system_protected: false,
                }],
            },
            AppProcessGroup {
                id: "sys:WindowServer".into(),
                name: "WindowServer".into(),
                primary_pid: 406,
                primary_start_time_secs: 1_700_000_000,
                bundle_path: None,
                helper_count: 1,
                cpu_pct: 9.2,
                footprint_bytes: 912_261_120,
                energy_impact: 10.4,
                wakeups_per_sec: 240.0,
                verdict: "system".into(),
                is_system_protected: true,
                is_blocking_sleep: false,
                mostly_in: Some("macOS Display Compositor · protected system daemon".into()),
                children: vec![ProcessRow {
                    pid: 406,
                    ppid: 1,
                    start_time_secs: 1_700_000_000,
                    name: "WindowServer".into(),
                    path: Some("/System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer".into()),
                    cpu_pct: 9.2,
                    footprint_bytes: 912_261_120,
                    energy_impact: 10.4,
                    wakeups_per_sec: 240.0,
                    is_system_protected: true,
                }],
            },
        ],
        memory_pressure: MemoryPressureInfo {
            level: "green".into(),
            total_bytes: 34_359_738_368,
            used_bytes: 21_904_333_209,
            wired_bytes: 4_509_715_660,
            compressed_bytes: 3_650_722_201,
            swap_used_bytes: 536_870_912,
            swap_total_bytes: 2_147_483_648,
        },
        total_cpu_pct: 81.8,
        total_footprint_bytes: 12_401_298_635,
        kernel_thermal_note: None,
    };

    let audit = vec![
        PmsetAuditItem {
            id: "demo-powernap".into(),
            profile: "Battery Power".into(),
            key: "powernap".into(),
            current_value: "1".into(),
            recommended_value: "0".into(),
            severity: "warning".into(),
            title: "Power Nap enabled on battery".into(),
            explanation: "Triggers periodic silent wakes on battery for Mail, iCloud, and Time Machine.".into(),
            fix_command: Some("sudo pmset -b powernap 0".into()),
        },
        PmsetAuditItem {
            id: "demo-tcpkeepalive".into(),
            profile: "Battery Power".into(),
            key: "tcpkeepalive".into(),
            current_value: "1".into(),
            recommended_value: "0".into(),
            severity: "warning".into(),
            title: "TCPKeepAlive active on battery (26 silent wakes)".into(),
            explanation: "Common cause of Wi-Fi / Bluetooth overnight wake loops. Disabling pauses Find My while asleep.".into(),
            fix_command: Some("sudo pmset -b tcpkeepalive 0".into()),
        },
        PmsetAuditItem {
            id: "demo-hibernatemode".into(),
            profile: "Battery Power".into(),
            key: "hibernatemode".into(),
            current_value: "3".into(),
            recommended_value: "3".into(),
            severity: "ok".into(),
            title: "Safe Sleep (hibernatemode 3) active".into(),
            explanation: "RAM is safely backed up before deep sleep or 0% shutdown.".into(),
            fix_command: None,
        },
    ];

    DashboardSnapshot {
        timestamp_ms: now_ms,
        demo_mode: true,
        host_name: "MacBook-Pro.local".into(),
        chip_name: "Apple M3 Pro".into(),
        macos_version: "macOS 26.5".into(),
        power,
        sleep,
        blockers,
        keep_awake: KeepAwakeState {
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
        },
        processes,
        audit,
        history: Vec::new(),
    }
}
