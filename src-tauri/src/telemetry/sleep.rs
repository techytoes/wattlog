use std::collections::HashMap;
use std::process::Command;

use chrono::{DateTime, FixedOffset};

use super::types::{
    DecodedWakeReason, PmsetAuditItem, SinceUnplugSummary, SleepReport, SleepSession,
    TimelineBucket, WakeEvent, WakeReasonAggregate,
};

/// Plain-English decoder table for 20+ macOS Apple Silicon & Intel sleep/wake tokens.
pub fn decode_wake_reason(raw: &str) -> DecodedWakeReason {
    let lower = raw.to_ascii_lowercase();

    // Order matters: more specific tokens first.
    let rules: &[(&[&str], &str, &str, &str, &str, &str, bool)] = &[
        (
            &["wifibt"],
            "wifibt",
            "Wi-Fi / Bluetooth Radio Wake",
            "bluetooth",
            "Wireless radio (AirDrop, Find My, Handoff, or Bluetooth accessory) woke the Mac during sleep.",
            "Disconnect chatty Bluetooth peripherals or disable Wake for Network Access on battery.",
            true,
        ),
        (
            &["smc.outboxnotempty", "smc.sysstate"],
            "SMC.OutboxNotEmpty",
            "SMC Pending Event (Wake Trigger)",
            "hardware",
            "Apple Silicon System Management Controller raised a pending event (often paired with lid, power button, or wifibt).",
            "Usually benign on lid/button wake; when paired with wifibt in DarkWake loops, disable Power Nap / TCPKeepAlive on battery.",
            true,
        ),
        (
            &["aop.outboxnotempty", "aop."],
            "AOP.OutboxNotEmpty",
            "Always-On Processor Sensor Wake",
            "hardware",
            "Always-On Processor (lid angle, Hey Siri mic, motion, or audio DSP) woke the system.",
            "Check Siri listen-for-voice settings or Bluetooth audio devices staying paired while asleep.",
            true,
        ),
        (
            &["nub.spmi", "spmi0"],
            "NUB.SPMI",
            "PMU / RTC Timer Interrupt",
            "schedule",
            "Power Management Unit scheduled an internal RTC maintenance timer wakeup.",
            "Driven by macOS Power Nap / Background App Refresh or Find My beacon check-ins.",
            true,
        ),
        (
            &["rtc/maintenance", "rtc/alarm", "rtc"],
            "RTC",
            "Scheduled Maintenance Timer",
            "schedule",
            "Real-Time Clock woke the Mac for a scheduled system alarm (Time Machine, iCloud, Mail, or dasd).",
            "Disable Power Nap on battery (`sudo pmset -b powernap 0`) if frequent DarkWakes drain battery overnight.",
            true,
        ),
        (
            &["ec.usbc", "usb-c", "usbc"],
            "EC.USBC",
            "USB-C / Thunderbolt Port Activity",
            "usb",
            "A USB-C hub, dock, or external monitor triggered a wake signal over USB-PD.",
            "Unplug bus-powered hubs/monitors before overnight sleep to stop repeated DarkWakes and reverse draw.",
            true,
        ),
        (
            &["xhc1", "xhc2", "xhc", "ehc", "usb"],
            "XHC",
            "USB Controller Device Wake",
            "usb",
            "A connected USB device (mouse, keyboard, dongle, or hub) signaled a wake event.",
            "Unplug wireless mouse/keyboard dongles or disable Bluetooth wake when travelling.",
            true,
        ),
        (
            &["ec.acattach", "acattach", "ac_attach"],
            "EC.ACAttach",
            "Charger Plugged / Unplugged",
            "power",
            "Power adapter connection state changed (plugged in, unplugged, or USB-C PD renegotiated).",
            "Normal when plugging in; if repeating while asleep, a flaky USB-C cable or monitor PD port is cycling power.",
            false,
        ),
        (
            &["ec.lidopen", "lidopen", " lid "],
            "lid",
            "Display Lid Opened",
            "user",
            "You opened the MacBook lid.",
            "Normal user wake.",
            false,
        ),
        (
            &["pwrbtn", "powerbutton", "ec.powerbutton"],
            "pwrbtn",
            "Power / Touch ID Button",
            "user",
            "Power button or Touch ID sensor was pressed.",
            "Normal user wake.",
            false,
        ),
        (
            &["hid", "keyboard", "trackpad", "useractivity"],
            "UserActivity",
            "Keyboard / Trackpad Input",
            "user",
            "Direct keyboard, trackpad, or HID input woke the Mac.",
            "Normal user wake (check for loose lid or backpack pressure if unexpected).",
            false,
        ),
        (
            &["dasd", "duetactivity"],
            "dasd",
            "Background Activity Scheduler (dasd)",
            "schedule",
            "macOS Duet Activity Scheduler ran deferred background tasks during DarkWake.",
            "Check Spotlight indexing, Photos sync, or iCloud Drive uploads.",
            true,
        ),
        (
            &["sharingd", "handoff", "airdrop"],
            "sharingd",
            "Handoff / AirDrop / Universal Control",
            "bluetooth",
            "Apple Continuity services (Handoff, AirDrop, Instant Hotspot) polled nearby Apple devices.",
            "Turn off Handoff or AirDrop receiving if experiencing repeated DarkWakes near iPhones/iPads.",
            true,
        ),
        (
            &["arpt", "wlan", "wifi", "tcpkeepalive", "network"],
            "ARPT/WiFi",
            "Wi-Fi Network Wake (TCPKeepAlive / Bonjour)",
            "network",
            "Incoming Wi-Fi packet or TCP keepalive refresh woke the Mac from Deep Idle.",
            "Disable Wake for Network Access (`womp`) or `tcpkeepalive` on battery to eliminate network DarkWakes.",
            true,
        ),
        (
            &["bluetooth", "btle", "bthci"],
            "Bluetooth",
            "Bluetooth Accessory Wake",
            "bluetooth",
            "A paired Bluetooth device (mouse, headphones, keyboard) woke the Mac.",
            "Power off Bluetooth mice/trackpads before packing your MacBook in a bag.",
            true,
        ),
        (
            &["proxwake", "proximity"],
            "ProxWake",
            "Proximity / Find My / Watch Unlock",
            "bluetooth",
            "Nearby Apple Watch or Find My network probe triggered a brief DarkWake.",
            "Benign if infrequent (<1/hr); check Bluetooth accessories if firing every few minutes.",
            true,
        ),
        (
            &["com.apple.alarm", "alarm", "calendar", "backupd"],
            "com.apple.alarm",
            "Scheduled Alarm / Time Machine",
            "schedule",
            "A system alarm or Time Machine local snapshot woke the Mac.",
            "Review `pmset -g sched` for lingering scheduled wake timers.",
            true,
        ),
        (
            &["hdef", "audio", "coreaudio"],
            "HDEF/Audio",
            "Audio Hardware Interrupt",
            "hardware",
            "Audio subsystem or external DAC/headphone jack triggered a wake.",
            "Check open browser tabs or apps holding audio streams before closing the lid.",
            true,
        ),
        (
            &["spit", "spi", "touchbar"],
            "SPIT",
            "SPI Bus (Internal Keyboard / Trackpad)",
            "hardware",
            "Internal SPI bus controller woke the system.",
            "Can trigger if the lid flexes inside a tight backpack.",
            false,
        ),
        (
            &["thermal", "overtemp", "ec.sleep"],
            "Thermal",
            "Thermal / Critical Battery Protection",
            "power",
            "System transitioned power state due to thermal limit or critically low battery.",
            "Inspect active sleep blockers and runaway processes that kept the laptop warm.",
            true,
        ),
        (
            &["maintenance"],
            "Maintenance",
            "macOS Maintenance DarkWake",
            "schedule",
            "Periodic macOS maintenance window (iCloud, Push Notifications, Find My).",
            "Normal occasionally; excessive loops usually point to `tcpkeepalive` or `powernap`.",
            true,
        ),
    ];

    for (needles, token, label, category, explanation, tip, suspect) in rules {
        if needles.iter().any(|n| lower.contains(n)) {
            return DecodedWakeReason {
                token: (*token).to_string(),
                label: (*label).to_string(),
                category: (*category).to_string(),
                explanation: (*explanation).to_string(),
                tip: (*tip).to_string(),
                is_insomnia_suspect: *suspect,
            };
        }
    }

    // Fallback when token wasn't recognized.
    let short_token = raw
        .split_whitespace()
        .next()
        .unwrap_or("Unknown")
        .trim_matches(|c| c == ':' || c == '/')
        .to_string();

    DecodedWakeReason {
        token: if short_token.is_empty() {
            "SystemWake".into()
        } else {
            short_token
        },
        label: "System Hardware / Kernel Wake".into(),
        category: "system".into(),
        explanation: format!("Kernel reported raw wake reason: {}", raw.trim()),
        tip: "Inspect `pmset -g log` around this timestamp if drain exceeds 2%/hr.".into(),
        is_insomnia_suspect: false,
    }
}

#[derive(Debug, Clone)]
struct ParsedLogEvent {
    ts: DateTime<FixedOffset>,
    ts_str: String,
    kind: LogEventKind,
    charge_pct: Option<u8>,
    power_source: String,
    duration_secs: u64,
    reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogEventKind {
    Sleep,
    DarkWake,
    Wake,
    HibernateWake,
}

/// Read and parse `/usr/bin/pmset -g log` (zero-root).
pub fn collect_sleep_report(current_charge_pct: Option<u8>, on_battery_now: bool) -> SleepReport {
    let output = Command::new("/usr/bin/pmset")
        .args(["-g", "log"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            parse_pmset_log(&text, current_charge_pct, on_battery_now)
        }
        _ => SleepReport {
            since_unplug: SinceUnplugSummary {
                started_at: None,
                elapsed_secs: 0,
                start_charge_pct: current_charge_pct,
                current_charge_pct,
                total_drop_pct: 0,
                active_secs: 0,
                sleep_secs: 0,
                dark_wake_secs: 0,
                dark_wake_count: 0,
                drain_pct_per_hour: 0.0,
                on_battery_now,
            },
            sessions: Vec::new(),
            top_wake_reasons: Vec::new(),
            timeline: Vec::new(),
            raw_log_available: false,
        },
    }
}

pub fn parse_pmset_log(
    log_text: &str,
    current_charge_pct: Option<u8>,
    on_battery_now: bool,
) -> SleepReport {
    let mut events: Vec<ParsedLogEvent> = Vec::new();

    for line in log_text.lines() {
        // Format: "2026-11-21 19:17:50 +0530 DarkWake            \tDarkWake from Deep Idle ..."
        if line.len() < 30 {
            continue;
        }
        let ts_slice = &line[..25];
        let rest = &line[25..];
        let trimmed = rest.trim_start();

        let kind = if trimmed.starts_with("Sleep ") || trimmed.starts_with("Sleep\t") {
            if !trimmed.contains("Entering Sleep state") {
                continue;
            }
            LogEventKind::Sleep
        } else if trimmed.starts_with("DarkWake ") || trimmed.starts_with("DarkWake\t") {
            LogEventKind::DarkWake
        } else if trimmed.starts_with("Wake ") || trimmed.starts_with("Wake\t") {
            if trimmed.contains("Wake from Hibernate") || trimmed.contains("Wake from Standby") {
                LogEventKind::HibernateWake
            } else if trimmed.contains("Wake from") {
                LogEventKind::Wake
            } else {
                continue;
            }
        } else {
            continue;
        };

        let Ok(ts) = DateTime::parse_from_str(ts_slice, "%Y-%m-%d %H:%M:%S %z") else {
            continue;
        };

        let charge_pct = extract_charge_pct(trimmed);
        let power_source = extract_power_source(trimmed);
        let duration_secs = extract_trailing_secs(trimmed);
        let reason = extract_reason(trimmed, kind);

        events.push(ParsedLogEvent {
            ts,
            ts_str: ts.format("%b %d %H:%M").to_string(),
            kind,
            charge_pct,
            power_source,
            duration_secs,
            reason,
        });
    }

    // Keep last 7 days of events to keep UI fast and focused.
    if let Some(last_ts) = events.last().map(|e| e.ts) {
        let cutoff = last_ts - chrono::Duration::days(7);
        events.retain(|e| e.ts >= cutoff);
    }

    // Group Sleep -> DarkWake* -> Wake into logical SleepSessions.
    let mut sessions: Vec<SleepSession> = Vec::new();
    let mut current_start: Option<ParsedLogEvent> = None;
    let mut current_wakes: Vec<WakeEvent> = Vec::new();
    let mut current_dark_secs: u64 = 0;
    let mut current_sleep_secs: u64 = 0;
    let mut reason_agg: HashMap<String, (DecodedWakeReason, u32, u64)> = HashMap::new();

    for ev in &events {
        match ev.kind {
            LogEventKind::Sleep => {
                if current_start.is_none() {
                    current_start = Some(ev.clone());
                    current_wakes.clear();
                    current_dark_secs = 0;
                    current_sleep_secs = ev.duration_secs;
                } else {
                    // Continuation of a sleep session after a DarkWake ("Maintenance Sleep").
                    current_sleep_secs += ev.duration_secs;
                }
            }
            LogEventKind::DarkWake => {
                let decoded = decode_wake_reason(&ev.reason);
                let entry = reason_agg
                    .entry(decoded.token.clone())
                    .or_insert_with(|| (decoded.clone(), 0, 0));
                entry.1 += 1;
                entry.2 += ev.duration_secs.max(1);

                current_dark_secs += ev.duration_secs;
                current_wakes.push(WakeEvent {
                    timestamp: ev.ts_str.clone(),
                    kind: "DarkWake".into(),
                    duration_secs: ev.duration_secs,
                    charge_pct: ev.charge_pct,
                    power_source: ev.power_source.clone(),
                    raw_reason: ev.reason.clone(),
                    decoded,
                });
            }
            LogEventKind::Wake | LogEventKind::HibernateWake => {
                let decoded = decode_wake_reason(&ev.reason);
                if let Some(start) = current_start.take() {
                    let wall_secs = (ev.ts - start.ts).num_seconds().max(0) as u64;
                    let total_secs = wall_secs.max(current_sleep_secs + current_dark_secs);
                    let drain = match (start.charge_pct, ev.charge_pct) {
                        (Some(s), Some(e)) => (s as i32 - e as i32).max(0),
                        _ => 0,
                    };
                    let dark_count = current_wakes.len() as u32;
                    let worst = find_worst_wake(&current_wakes).or_else(|| Some(decoded.clone()));
                    let severity = if drain >= 8 || dark_count >= 10 {
                        "critical"
                    } else if drain >= 3 || dark_count >= 3 {
                        "warning"
                    } else {
                        "ok"
                    };

                    let mut wakes = std::mem::take(&mut current_wakes);
                    wakes.push(WakeEvent {
                        timestamp: ev.ts_str.clone(),
                        kind: if ev.kind == LogEventKind::HibernateWake {
                            "HibernateWake".into()
                        } else {
                            "Wake".into()
                        },
                        duration_secs: ev.duration_secs,
                        charge_pct: ev.charge_pct,
                        power_source: ev.power_source.clone(),
                        raw_reason: ev.reason.clone(),
                        decoded,
                    });

                    // Only record meaningful sleep sessions (>= 60s or any DarkWake/drain).
                    if total_secs >= 60 || dark_count > 0 || drain > 0 {
                        sessions.push(SleepSession {
                            id: format!("sleep-{}", sessions.len() + 1),
                            start_time: start.ts_str,
                            end_time: ev.ts_str.clone(),
                            duration_secs: total_secs,
                            start_charge_pct: start.charge_pct,
                            end_charge_pct: ev.charge_pct,
                            drain_pct: drain,
                            power_source: start.power_source,
                            sleep_reason: start.reason,
                            dark_wake_count: dark_count,
                            dark_wake_total_secs: current_dark_secs,
                            worst_wake_reason: worst,
                            wake_events: wakes,
                            severity: severity.into(),
                        });
                    }
                    current_dark_secs = 0;
                    current_sleep_secs = 0;
                }
            }
        }
    }

    // Most recent sessions first.
    sessions.reverse();
    sessions.truncate(20);

    // Top wake reasons sorted by frequency.
    let mut top_wake_reasons: Vec<WakeReasonAggregate> = reason_agg
        .into_values()
        .map(|(decoded, count, total_secs)| WakeReasonAggregate {
            decoded,
            count,
            total_secs,
        })
        .collect();
    top_wake_reasons.sort_by(|a, b| b.count.cmp(&a.count).then(b.total_secs.cmp(&a.total_secs)));
    top_wake_reasons.truncate(12);

    let since_unplug = compute_since_unplug(&events, current_charge_pct, on_battery_now);
    let timeline = build_timeline_buckets(&events, on_battery_now, current_charge_pct);

    SleepReport {
        since_unplug,
        sessions,
        top_wake_reasons,
        timeline,
        raw_log_available: !events.is_empty(),
    }
}

fn find_worst_wake(wakes: &[WakeEvent]) -> Option<DecodedWakeReason> {
    let mut counts: HashMap<String, (usize, DecodedWakeReason)> = HashMap::new();
    for w in wakes {
        let entry = counts
            .entry(w.decoded.token.clone())
            .or_insert_with(|| (0, w.decoded.clone()));
        entry.0 += 1;
    }
    counts
        .into_values()
        .max_by_key(|(c, d)| (d.is_insomnia_suspect as usize * 10_000) + *c)
        .map(|(_, d)| d)
}

fn compute_since_unplug(
    events: &[ParsedLogEvent],
    current_charge_pct: Option<u8>,
    on_battery_now: bool,
) -> SinceUnplugSummary {
    // Walk backward from the latest event to find the last AC event (or highest charge before discharge).
    let mut unplug_idx: Option<usize> = None;
    for (idx, ev) in events.iter().enumerate().rev() {
        if ev.power_source == "AC" {
            if idx + 1 < events.len() {
                unplug_idx = Some(idx + 1);
            }
            break;
        }
    }

    let slice = match unplug_idx {
        Some(i) if i < events.len() => &events[i..],
        _ => {
            // If all recent events are on BATT, take the last 24h window.
            if let Some(last) = events.last() {
                let cutoff = last.ts - chrono::Duration::hours(24);
                let start = events.iter().position(|e| e.ts >= cutoff).unwrap_or(0);
                &events[start..]
            } else {
                &[]
            }
        }
    };

    if slice.is_empty() {
        return SinceUnplugSummary {
            started_at: None,
            elapsed_secs: 0,
            start_charge_pct: current_charge_pct,
            current_charge_pct,
            total_drop_pct: 0,
            active_secs: 0,
            sleep_secs: 0,
            dark_wake_secs: 0,
            dark_wake_count: 0,
            drain_pct_per_hour: 0.0,
            on_battery_now,
        };
    }

    let first = &slice[0];
    let now = chrono::Local::now().fixed_offset();
    let elapsed_secs = (now - first.ts).num_seconds().max(60) as u64;

    let mut sleep_secs = 0u64;
    let mut dark_wake_secs = 0u64;
    let mut dark_wake_count = 0u32;

    for ev in slice {
        match ev.kind {
            LogEventKind::Sleep => sleep_secs += ev.duration_secs,
            LogEventKind::DarkWake => {
                dark_wake_count += 1;
                dark_wake_secs += ev.duration_secs.max(1);
            }
            _ => {}
        }
    }

    let active_secs = elapsed_secs.saturating_sub(sleep_secs + dark_wake_secs);
    let start_pct = first.charge_pct.or(current_charge_pct);
    let cur_pct = current_charge_pct.or_else(|| slice.last().and_then(|e| e.charge_pct));
    let total_drop = match (start_pct, cur_pct) {
        (Some(s), Some(c)) => (s as i32 - c as i32).max(0),
        _ => 0,
    };
    let hours = (elapsed_secs as f64 / 3600.0).max(0.05);

    SinceUnplugSummary {
        started_at: Some(first.ts_str.clone()),
        elapsed_secs,
        start_charge_pct: start_pct,
        current_charge_pct: cur_pct,
        total_drop_pct: total_drop,
        active_secs,
        sleep_secs,
        dark_wake_secs,
        dark_wake_count,
        drain_pct_per_hour: ((total_drop as f64 / hours) * 10.0).round() / 10.0,
        on_battery_now,
    }
}

fn build_timeline_buckets(
    events: &[ParsedLogEvent],
    on_battery_now: bool,
    current_charge_pct: Option<u8>,
) -> Vec<TimelineBucket> {
    let target_buckets = 48usize;
    if events.is_empty() {
        return Vec::new();
    }

    let now = chrono::Local::now().fixed_offset();
    let window_secs = 24 * 3600i64;
    let start = now - chrono::Duration::seconds(window_secs);
    let step_secs = window_secs / target_buckets as i64;

    let mut buckets = Vec::with_capacity(target_buckets);
    for i in 0..target_buckets {
        let b_start = start + chrono::Duration::seconds(step_secs * i as i64);
        let b_end = b_start + chrono::Duration::seconds(step_secs);
        let label = b_start.format("%H:%M").to_string();

        // Find events inside this bucket and the last known state before it.
        let mut dark_wakes = 0u32;
        let mut last_charge: Option<u8> = None;
        let mut had_sleep = false;
        let mut had_ac = false;
        let mut had_wake = false;
        let mut detail_reason = String::new();

        for ev in events {
            if ev.ts < b_start {
                if ev.charge_pct.is_some() {
                    last_charge = ev.charge_pct;
                }
                had_sleep = ev.kind == LogEventKind::Sleep;
                had_ac = ev.power_source == "AC";
            } else if ev.ts >= b_start && ev.ts < b_end {
                if ev.charge_pct.is_some() {
                    last_charge = ev.charge_pct;
                }
                if ev.power_source == "AC" {
                    had_ac = true;
                }
                match ev.kind {
                    LogEventKind::DarkWake => {
                        dark_wakes += 1;
                        if detail_reason.is_empty() {
                            detail_reason = decode_wake_reason(&ev.reason).token;
                        }
                    }
                    LogEventKind::Sleep => had_sleep = true,
                    LogEventKind::Wake | LogEventKind::HibernateWake => {
                        had_wake = true;
                        had_sleep = false;
                    }
                }
            }
        }

        let (state, detail, est_watts) = if dark_wakes > 0 {
            (
                "darkwake",
                format!("{dark_wakes} DarkWake(s) · {detail_reason}"),
                3.2,
            )
        } else if had_ac && (i < target_buckets - 1 || !on_battery_now) {
            ("charging", "On AC power".to_string(), 14.0)
        } else if had_sleep && !had_wake {
            ("sleep", "Deep Idle sleep".to_string(), 0.3)
        } else if i == target_buckets - 1 {
            (
                if on_battery_now { "active" } else { "charging" },
                if on_battery_now {
                    "Active on battery".to_string()
                } else {
                    "Active on AC".to_string()
                },
                7.5,
            )
        } else {
            ("active", "Awake / active".to_string(), 6.8)
        };

        buckets.push(TimelineBucket {
            label,
            state: state.into(),
            watts: est_watts,
            charge_pct: if i == target_buckets - 1 {
                current_charge_pct.or(last_charge)
            } else {
                last_charge
            },
            duration_secs: step_secs as u64,
            detail,
        });
    }

    buckets
}

fn extract_charge_pct(s: &str) -> Option<u8> {
    let idx = s.find("Charge:")?;
    let after = &s[idx + "Charge:".len()..];
    let digits: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<u8>().ok().map(|v| v.min(100))
}

fn extract_power_source(s: &str) -> String {
    if s.contains("Using AC") {
        "AC".into()
    } else if s.contains("Using Batt") || s.contains("Using BATT") {
        "BATT".into()
    } else {
        "BATT".into()
    }
}

fn extract_trailing_secs(s: &str) -> u64 {
    // e.g. "... (Charge:38%) 902 secs"
    if let Some(pos) = s.rfind(" secs") {
        let before = s[..pos].trim_end();
        let num_str: String = before
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if let Ok(v) = num_str.parse::<u64>() {
            return v;
        }
    }
    0
}

fn extract_reason(s: &str, kind: LogEventKind) -> String {
    match kind {
        LogEventKind::Sleep => {
            if let Some(start) = s.find("due to '") {
                let after = &s[start + "due to '".len()..];
                if let Some(end) = after.find('\'') {
                    return after[..end].to_string();
                }
            }
            "Idle Sleep".into()
        }
        LogEventKind::DarkWake | LogEventKind::Wake | LogEventKind::HibernateWake => {
            if let Some(start) = s.find("due to ") {
                let after = &s[start + "due to ".len()..];
                let end = after
                    .find(" Using ")
                    .or_else(|| after.find(" (Charge:"))
                    .unwrap_or(after.len());
                return after[..end].trim().to_string();
            }
            "Unknown".into()
        }
    }
}

/// Read-only `pmset -g custom` footgun audit (zero root daemon, provides "Copy Fix Command").
pub fn collect_pmset_audit(dark_wake_count: u32) -> Vec<PmsetAuditItem> {
    let output = Command::new("/usr/bin/pmset")
        .args(["-g", "custom"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            parse_pmset_custom_audit(&text, dark_wake_count)
        }
        _ => Vec::new(),
    }
}

pub fn parse_pmset_custom_audit(text: &str, dark_wake_count: u32) -> Vec<PmsetAuditItem> {
    let mut profile = String::new();
    let mut batt: HashMap<String, String> = HashMap::new();
    let mut ac: HashMap<String, String> = HashMap::new();

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.starts_with("Battery Power:") {
            profile = "Battery Power".into();
            continue;
        } else if line.starts_with("AC Power:") {
            profile = "AC Power".into();
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
            // Skip multi-word labels like "Sleep On Power Button 1" cleanly.
            if k == "Sleep" {
                continue;
            }
            let target = if profile == "Battery Power" {
                &mut batt
            } else {
                &mut ac
            };
            target.insert(k.to_string(), v.to_string());
        }
    }

    let mut items: Vec<PmsetAuditItem> = Vec::new();

    // 1. disablesleep = 1 (huge hot-bag footgun!)
    if let Some(v) = batt.get("SleepDisabled").or_else(|| batt.get("disablesleep")) {
        if v == "1" {
            items.push(PmsetAuditItem {
                id: "batt-disablesleep".into(),
                profile: "Battery Power".into(),
                key: "disablesleep".into(),
                current_value: "1".into(),
                recommended_value: "0".into(),
                severity: "critical".into(),
                title: "Sleep is globally disabled (Hot-Bag Risk)".into(),
                explanation: "Closing the lid will NOT sleep this MacBook. Often left behind by closed-lid tools.".into(),
                fix_command: Some("sudo pmset -a disablesleep 0".into()),
            });
        }
    }

    // 2. sleep = 0 on battery
    if let Some(v) = batt.get("sleep") {
        if v == "0" {
            items.push(PmsetAuditItem {
                id: "batt-sleep-zero".into(),
                profile: "Battery Power".into(),
                key: "sleep".into(),
                current_value: "0".into(),
                recommended_value: "1".into(),
                severity: "critical".into(),
                title: "Idle system sleep disabled on battery".into(),
                explanation: "With `sleep 0`, macOS will never auto-sleep on battery when left idle.".into(),
                fix_command: Some("sudo pmset -b sleep 1".into()),
            });
        }
    }

    // 3. powernap = 1 on battery
    if let Some(v) = batt.get("powernap") {
        if v == "1" {
            items.push(PmsetAuditItem {
                id: "batt-powernap".into(),
                profile: "Battery Power".into(),
                key: "powernap".into(),
                current_value: "1".into(),
                recommended_value: "0".into(),
                severity: if dark_wake_count >= 5 { "warning" } else { "warning" }.into(),
                title: "Power Nap enabled on battery".into(),
                explanation: "Allows macOS to wake periodically in DarkWake to sync Mail, iCloud, and Time Machine on battery.".into(),
                fix_command: Some("sudo pmset -b powernap 0".into()),
            });
        }
    }

    // 4. tcpkeepalive = 1 on battery
    if let Some(v) = batt.get("tcpkeepalive") {
        if v == "1" {
            items.push(PmsetAuditItem {
                id: "batt-tcpkeepalive".into(),
                profile: "Battery Power".into(),
                key: "tcpkeepalive".into(),
                current_value: "1".into(),
                recommended_value: "0".into(),
                severity: if dark_wake_count >= 5 {
                    "warning".into()
                } else {
                    "ok".into()
                },
                title: if dark_wake_count >= 5 {
                    format!("TCPKeepAlive active on battery ({dark_wake_count} DarkWakes seen)")
                } else {
                    "TCPKeepAlive active on battery (Find My / Push)".into()
                },
                explanation: "Keeps Wi-Fi sockets alive during sleep; common cause of `wifibt` / `SMC.OutboxNotEmpty` DarkWake loops. Note: disabling pauses Find My while asleep.".into(),
                fix_command: Some("sudo pmset -b tcpkeepalive 0".into()),
            });
        }
    }

    // 5. womp = 1 on battery
    if let Some(v) = batt.get("womp") {
        if v == "1" {
            items.push(PmsetAuditItem {
                id: "batt-womp".into(),
                profile: "Battery Power".into(),
                key: "womp".into(),
                current_value: "1".into(),
                recommended_value: "0".into(),
                severity: "warning".into(),
                title: "Wake for Network Access enabled on battery".into(),
                explanation: "Remote network packets can wake your MacBook while unplugged.".into(),
                fix_command: Some("sudo pmset -b womp 0".into()),
            });
        }
    }

    // 6. hibernatemode = 0 on battery
    if let Some(v) = batt.get("hibernatemode") {
        if v == "0" {
            items.push(PmsetAuditItem {
                id: "batt-hibernate".into(),
                profile: "Battery Power".into(),
                key: "hibernatemode".into(),
                current_value: "0".into(),
                recommended_value: "3".into(),
                severity: "warning".into(),
                title: "Safe Sleep (hibernatemode 3) is off on battery".into(),
                explanation: "RAM is not backed up to disk on sleep; hitting 0% battery will hard-crash open work.".into(),
                fix_command: Some("sudo pmset -b hibernatemode 3".into()),
            });
        }
    }

    // 7. standby = 0 on battery
    if let Some(v) = batt.get("standby") {
        if v == "0" {
            items.push(PmsetAuditItem {
                id: "batt-standby".into(),
                profile: "Battery Power".into(),
                key: "standby".into(),
                current_value: "0".into(),
                recommended_value: "1".into(),
                severity: "warning".into(),
                title: "Deep Standby disabled on battery".into(),
                explanation: "Mac will stay in higher-draw normal sleep instead of dropping into deep standby overnight.".into(),
                fix_command: Some("sudo pmset -b standby 1".into()),
            });
        }
    }

    // 8. networkoversleep = 1 on battery
    if let Some(v) = batt.get("networkoversleep") {
        if v == "1" {
            items.push(PmsetAuditItem {
                id: "batt-networkoversleep".into(),
                profile: "Battery Power".into(),
                key: "networkoversleep".into(),
                current_value: "1".into(),
                recommended_value: "0".into(),
                severity: "warning".into(),
                title: "Network-over-Sleep enabled on battery".into(),
                explanation: "Maintains active network interfaces during sleep, increasing overnight drain.".into(),
                fix_command: Some("sudo pmset -b networkoversleep 0".into()),
            });
        }
    }

    // 9. ttyskeepawake = 1 on battery (informational if user leaves SSH open)
    if let Some(v) = batt.get("ttyskeepawake") {
        if v == "1" {
            items.push(PmsetAuditItem {
                id: "batt-ttys".into(),
                profile: "Battery Power".into(),
                key: "ttyskeepawake".into(),
                current_value: "1".into(),
                recommended_value: "0".into(),
                severity: "ok".into(),
                title: "Active SSH/TTY sessions prevent sleep on battery".into(),
                explanation: "Leaving a remote TTY session open can keep your MacBook awake until disconnected.".into(),
                fix_command: Some("sudo pmset -b ttyskeepawake 0".into()),
            });
        }
    }

    // 10. Healthy confirmation check for AC/Battery sleep baseline
    if let Some(v) = batt.get("hibernatemode") {
        if v == "3" {
            items.push(PmsetAuditItem {
                id: "batt-hibernate-ok".into(),
                profile: "Battery Power".into(),
                key: "hibernatemode".into(),
                current_value: "3".into(),
                recommended_value: "3".into(),
                severity: "ok".into(),
                title: "Safe Sleep (hibernatemode 3) active".into(),
                explanation: "RAM state is saved safely before deep sleep/0% shutdown.".into(),
                fix_command: None,
            });
        }
    }
    if let Some(v) = ac.get("womp") {
        if v == "1" {
            items.push(PmsetAuditItem {
                id: "ac-womp-ok".into(),
                profile: "AC Power".into(),
                key: "womp".into(),
                current_value: "1".into(),
                recommended_value: "1".into(),
                severity: "ok".into(),
                title: "Wake for Network Access enabled on AC".into(),
                explanation: "Safe on AC power for remote wake, Time Machine, and AirDrop.".into(),
                fix_command: None,
            });
        }
    }

    // Sort: critical -> warning -> ok
    items.sort_by_key(|item| match item.severity.as_str() {
        "critical" => 0,
        "warning" => 1,
        _ => 2,
    });

    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_apple_silicon_wake_tokens() {
        let d = decode_wake_reason("smc.sysState.Wake(0x70070000) wifibt SMC.OutboxNotEmpty/");
        assert_eq!(d.token, "wifibt");
        assert!(d.is_insomnia_suspect);

        let d2 = decode_wake_reason("NUB.SPMI0.SW3.IRQ nub-spmi0.0x02 rtc/Maintenance");
        assert_eq!(d2.token, "NUB.SPMI");
        assert!(d2.is_insomnia_suspect);

        let d3 = decode_wake_reason("smc.sysState.Wake(0x70070000) lid SMC.OutboxNotEmpty/UserActivity");
        assert_eq!(d3.token, "SMC.OutboxNotEmpty");

        let d4 = decode_wake_reason("acattach/UserActivity Assertion");
        assert_eq!(d4.token, "EC.ACAttach");
        assert!(!d4.is_insomnia_suspect);
    }

    #[test]
    fn parses_real_pmset_log_session() {
        let sample = "\
2026-11-21 19:02:48 +0530 Sleep               \tEntering Sleep state due to 'Maintenance Sleep':TCPKeepAlive=active Using Batt (Charge:38%) 902 secs
2026-11-21 19:17:50 +0530 DarkWake            \tDarkWake from Deep Idle [CDNP] : due to smc.sysState.Wake(0x70070000) wifibt SMC.OutboxNotEmpty/ Using BATT (Charge:38%) 21 secs
2026-11-21 19:18:11 +0530 Sleep               \tEntering Sleep state due to 'Maintenance Sleep':TCPKeepAlive=active Using Batt (Charge:38%) 726 secs
2026-11-21 19:30:17 +0530 DarkWake            \tDarkWake from Deep Idle [CDNP] : due to NUB.SPMI0.SW3.IRQ nub-spmi0.0x02 rtc/Maintenance Using BATT (Charge:35%) 21 secs
2026-11-21 20:52:01 +0530 Wake                \tWake from Deep Idle [CDNVA] : due to smc.sysState.Wake(0x70070000) lid SMC.OutboxNotEmpty/UserActivity Assertion Using BATT (Charge:34%)
";
        let report = parse_pmset_log(sample, Some(34), true);
        assert_eq!(report.sessions.len(), 1);
        assert_eq!(report.sessions[0].dark_wake_count, 2);
        assert_eq!(report.sessions[0].drain_pct, 4);
        assert_eq!(report.sessions[0].severity, "warning");
        assert!(report.raw_log_available);
    }

    #[test]
    fn audits_pmset_custom_footguns() {
        let sample = "\
Battery Power:
 disablesleep         1
 powernap             1
 tcpkeepalive         1
 hibernatemode        0
 sleep                1
AC Power:
 womp                 1
";
        let audit = parse_pmset_custom_audit(sample, 12);
        assert!(audit.iter().any(|i| i.key == "disablesleep" && i.severity == "critical"));
        assert!(audit.iter().any(|i| i.key == "powernap" && i.fix_command.is_some()));
        assert!(audit.iter().any(|i| i.key == "hibernatemode" && i.severity == "warning"));
    }
}
