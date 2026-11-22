use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use chrono::{DateTime, FixedOffset};

use super::storage::StoredUnplugTransition;
use super::types::{
    DecodedWakeReason, PmsetAuditItem, SinceUnplugSummary, SleepReport, SleepSession,
    TimelineBucket, WakeEvent, WakeReasonAggregate,
};

/// Plain-English decoder table for 20+ macOS Apple Silicon & Intel sleep/wake tokens.
/// Specific user/radio/peripheral tokens are matched BEFORE the generic `SMC.OutboxNotEmpty`
/// wrapper so `lid` and `pwrbtn` wakes are never mislabeled.
pub fn decode_wake_reason(raw: &str) -> DecodedWakeReason {
    let lower = raw.to_ascii_lowercase();

    let rules: &[(&[&str], &str, &str, &str, &str, &str, bool)] = &[
        (
            &["wifibt"],
            "wifibt",
            "Wi-Fi / Bluetooth Radio Wake",
            "bluetooth",
            "Wireless radio (AirDrop, Find My, Handoff, or a paired Bluetooth accessory) woke the Mac during sleep.",
            "Disconnect chatty Bluetooth peripherals or disable Wake for Network Access / Power Nap on battery.",
            true,
        ),
        (
            &["ec.lidopen", "lidopen", " lid ", "/lid", "lid "],
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
            &["ec.acattach", "acattach", "ac_attach"],
            "EC.ACAttach",
            "Charger Plugged / Unplugged",
            "power",
            "Power adapter connection state changed (plugged in, unplugged, or USB-C PD renegotiated).",
            "Normal when plugging in; if repeating while asleep, a flaky USB-C cable or monitor PD port is cycling power.",
            false,
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
            &["hid", "keyboard", "trackpad", "useractivity"],
            "UserActivity",
            "Keyboard / Trackpad Input",
            "user",
            "Direct keyboard, trackpad, or HID input woke the Mac.",
            "Normal user wake (check for loose lid or backpack pressure if unexpected).",
            false,
        ),
        (
            &["smc.outboxnotempty", "smc.sysstate"],
            "SMC.OutboxNotEmpty",
            "SMC Pending Event (Wake Trigger)",
            "hardware",
            "Apple Silicon System Management Controller raised a pending hardware wake event.",
            "When repeating in DarkWake loops on battery, disable Power Nap or TCPKeepAlive.",
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
    ts_ms: i64,
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
    PowerMarker, // From `Assertions Summary-` Using AC / Using Batt
}

/// Stream and parse `/usr/bin/pmset -g log` via `BufReader` + raw byte prefiltering (~64–400 KiB peak).
pub fn collect_sleep_report(
    current_charge_pct: Option<u8>,
    on_battery_now: bool,
    stored_unplug: Option<StoredUnplugTransition>,
    earliest_sample_ms: Option<i64>,
) -> SleepReport {
    let mut child = match Command::new("/usr/bin/pmset")
        .args(["-g", "log"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return empty_sleep_report(current_charge_pct, on_battery_now, "log_unavailable"),
    };

    let Some(stdout) = child.stdout.take() else {
        let _ = child.wait();
        return empty_sleep_report(current_charge_pct, on_battery_now, "log_unavailable");
    };

    let mut reader = BufReader::with_capacity(64 * 1024, stdout);
    let mut line_buf: Vec<u8> = Vec::with_capacity(512);
    let mut events: Vec<ParsedLogEvent> = Vec::new();

    loop {
        line_buf.clear();
        match reader.read_until(b'\n', &mut line_buf) {
            Ok(0) => break,
            Ok(_) => {
                if let Some(ev) = parse_prefiltered_bytes(&line_buf) {
                    events.push(ev);
                }
            }
            Err(_) => break,
        }
    }
    let _ = child.wait();

    build_sleep_report_from_events(
        events,
        current_charge_pct,
        on_battery_now,
        stored_unplug,
        earliest_sample_ms,
        chrono::Local::now().fixed_offset(),
    )
}

fn empty_sleep_report(
    current_charge_pct: Option<u8>,
    on_battery_now: bool,
    status: &str,
) -> SleepReport {
    SleepReport {
        since_unplug: SinceUnplugSummary {
            started_at: None,
            unplug_ts_ms: None,
            unplug_source: "unknown".into(),
            data_status: status.into(),
            history_coverage_secs: 0,
            elapsed_secs: 0,
            start_charge_pct: current_charge_pct,
            current_charge_pct,
            total_drop_pct: 0,
            asleep_drain_pct: 0,
            active_drain_pct: 0,
            sleep_session_count: 0,
            active_secs: 0,
            sleep_secs: 0,
            dark_wake_secs: 0,
            dark_wake_count: 0,
            drain_pct_per_hour: 0.0,
            on_battery_now,
        },
        sessions: Vec::new(),
        top_wake_reasons: Vec::new(),
        since_unplug_wake_reasons: Vec::new(),
        timeline: Vec::new(),
        raw_log_available: false,
    }
}

/// Fast raw-byte prefilter: skips ~95% of `pmset -g log` lines (mostly `Assertions`) with zero allocation.
fn parse_prefiltered_bytes(line: &[u8]) -> Option<ParsedLogEvent> {
    if line.len() < 32 {
        return None;
    }
    // Timestamp prefix must start with "20" (YYYY-MM-DD HH:MM:SS +ZZZZ)
    if line[0] != b'2' || line[4] != b'-' || line[7] != b'-' {
        return None;
    }

    let mut idx = 25;
    while idx < line.len() && (line[idx] == b' ' || line[idx] == b'\t') {
        idx += 1;
    }
    if idx >= line.len() {
        return None;
    }
    let rest_bytes = &line[idx..];

    let is_sleep = rest_bytes.starts_with(b"Sleep ") || rest_bytes.starts_with(b"Sleep\t");
    let is_dark = rest_bytes.starts_with(b"DarkWake ") || rest_bytes.starts_with(b"DarkWake\t");
    let is_wake = rest_bytes.starts_with(b"Wake ") || rest_bytes.starts_with(b"Wake\t");
    let is_assertions =
        rest_bytes.starts_with(b"Assertions ") || rest_bytes.starts_with(b"Assertions\t");

    if !is_sleep && !is_dark && !is_wake && !is_assertions {
        return None;
    }

    let line_str = std::str::from_utf8(line).ok()?;
    let trimmed = line_str[idx..].trim_end();

    let kind = if is_sleep {
        if !trimmed.contains("Entering Sleep state") {
            return None;
        }
        LogEventKind::Sleep
    } else if is_dark {
        LogEventKind::DarkWake
    } else if is_wake {
        if trimmed.contains("Wake from Hibernate") || trimmed.contains("Wake from Standby") {
            LogEventKind::HibernateWake
        } else if trimmed.contains("Wake from") {
            LogEventKind::Wake
        } else {
            return None;
        }
    } else {
        // Only keep `Assertions Summary-` lines that contain a complete `Using AC(Charge:` or `Using Batt(Charge:` marker!
        if !trimmed.contains("Summary-")
            || (!trimmed.contains("Using AC(Charge:") && !trimmed.contains("Using Batt(Charge:"))
        {
            return None;
        }
        LogEventKind::PowerMarker
    };

    let ts = DateTime::parse_from_str(&line_str[..25], "%Y-%m-%d %H:%M:%S %z").ok()?;
    let charge_pct = extract_charge_pct(trimmed);
    let power_source = extract_power_source(trimmed);
    let duration_secs = if kind == LogEventKind::PowerMarker {
        0
    } else {
        extract_trailing_secs(trimmed)
    };
    let reason = if kind == LogEventKind::PowerMarker {
        String::new()
    } else {
        extract_reason(trimmed, kind)
    };

    Some(ParsedLogEvent {
        ts,
        ts_ms: ts.timestamp_millis(),
        ts_str: ts.format("%b %d %H:%M").to_string(),
        kind,
        charge_pct,
        power_source,
        duration_secs,
        reason,
    })
}

pub fn parse_pmset_log(
    log_text: &str,
    current_charge_pct: Option<u8>,
    on_battery_now: bool,
) -> SleepReport {
    let events: Vec<ParsedLogEvent> = log_text
        .lines()
        .filter_map(|l| parse_prefiltered_bytes(l.as_bytes()))
        .collect();
    let now = events
        .last()
        .map(|e| e.ts)
        .unwrap_or_else(|| chrono::Local::now().fixed_offset());
    build_sleep_report_from_events(events, current_charge_pct, on_battery_now, None, None, now)
}

struct SessionWithIntervals {
    session: SleepSession,
    sleep_intervals: Vec<(i64, i64)>,
    dark_wake_intervals: Vec<(i64, i64)>,
}

fn build_sleep_report_from_events(
    mut events: Vec<ParsedLogEvent>,
    current_charge_pct: Option<u8>,
    on_battery_now: bool,
    stored_unplug: Option<StoredUnplugTransition>,
    earliest_sample_ms: Option<i64>,
    now: DateTime<FixedOffset>,
) -> SleepReport {
    let now_ms = now.timestamp_millis();

    // Keep last 7 days of events relative to `now`.
    let cutoff_ms = now_ms - (7 * 24 * 3600 * 1000);
    events.retain(|e| e.ts_ms >= cutoff_ms);

    let raw_log_available = !events.is_empty();

    // 1. Determine accurate `[unplug_ts_ms, now_ms]` window using SQLite samples + pmset fallback.
    let (unplug_ts_ms, unplug_start_pct, unplug_source) =
        resolve_unplug_anchor(&events, on_battery_now, stored_unplug);

    // 2. Group Sleep -> DarkWake* -> Wake into sessions (retaining exact ms intervals).
    let mut built_sessions: Vec<SessionWithIntervals> = Vec::new();
    let mut current_start: Option<ParsedLogEvent> = None;
    let mut current_wakes: Vec<WakeEvent> = Vec::new();
    let mut current_sleep_intervals: Vec<(i64, i64)> = Vec::new();
    let mut current_dark_intervals: Vec<(i64, i64)> = Vec::new();
    let mut current_dark_secs: u64 = 0;
    let mut current_sleep_secs: u64 = 0;

    let mut all_batt_reason_agg: HashMap<String, (DecodedWakeReason, u32, u64)> = HashMap::new();
    let mut unplug_reason_agg: HashMap<String, (DecodedWakeReason, u32, u64)> = HashMap::new();

    for ev in &events {
        match ev.kind {
            LogEventKind::PowerMarker => {}
            LogEventKind::Sleep => {
                let dur_ms = (ev.duration_secs as i64) * 1000;
                if current_start.is_none() {
                    current_start = Some(ev.clone());
                    current_wakes.clear();
                    current_sleep_intervals.clear();
                    current_dark_intervals.clear();
                    current_dark_secs = 0;
                    current_sleep_secs = ev.duration_secs;
                } else {
                    current_sleep_secs += ev.duration_secs;
                }
                if dur_ms > 0 {
                    current_sleep_intervals.push((ev.ts_ms, ev.ts_ms + dur_ms));
                }
            }
            LogEventKind::DarkWake => {
                let decoded = decode_wake_reason(&ev.reason);
                let dur_s = ev.duration_secs.max(1);
                if ev.power_source != "AC" {
                    let e_all = all_batt_reason_agg
                        .entry(decoded.token.clone())
                        .or_insert_with(|| (decoded.clone(), 0, 0));
                    e_all.1 += 1;
                    e_all.2 += dur_s;

                    if let Some(u_ms) = unplug_ts_ms {
                        if ev.ts_ms >= u_ms {
                            let e_u = unplug_reason_agg
                                .entry(decoded.token.clone())
                                .or_insert_with(|| (decoded.clone(), 0, 0));
                            e_u.1 += 1;
                            e_u.2 += dur_s;
                        }
                    }
                }

                current_dark_secs += ev.duration_secs;
                current_dark_intervals.push((ev.ts_ms, ev.ts_ms + (dur_s as i64) * 1000));
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
                    let wall_secs = ((ev.ts_ms - start.ts_ms).max(0) / 1000) as u64;
                    let total_secs = wall_secs.max(current_sleep_secs + current_dark_secs);
                    let drain = match (start.charge_pct, ev.charge_pct) {
                        (Some(s), Some(e)) => (s as i32 - e as i32).max(0),
                        _ => 0,
                    };
                    let dark_count = current_wakes.len() as u32;
                    let worst = find_worst_wake(&current_wakes).or_else(|| Some(decoded.clone()));
                    let severity = if start.power_source == "AC" {
                        "ok"
                    } else if drain >= 8 || dark_count >= 10 {
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

                    let in_unplug = unplug_ts_ms
                        .map(|u_ms| ev.ts_ms > u_ms && start.power_source != "AC")
                        .unwrap_or(false);

                    // Record battery sleep sessions (skip AC-only overnight charging sessions so they don't distort battery sleep health).
                    if start.power_source != "AC" && (total_secs >= 60 || dark_count > 0 || drain > 0) {
                        built_sessions.push(SessionWithIntervals {
                            session: SleepSession {
                                id: format!("sleep-{}", built_sessions.len() + 1),
                                start_time: start.ts_str,
                                end_time: ev.ts_str.clone(),
                                start_ms: start.ts_ms,
                                end_ms: ev.ts_ms,
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
                                in_current_unplug: in_unplug,
                            },
                            sleep_intervals: std::mem::take(&mut current_sleep_intervals),
                            dark_wake_intervals: std::mem::take(&mut current_dark_intervals),
                        });
                    }
                    current_dark_secs = 0;
                    current_sleep_secs = 0;
                }
            }
        }
    }

    // 3. Intersect battery sleep sessions strictly with `[unplug_ts_ms, now_ms]`!
    let since_unplug = compute_scoped_since_unplug(
        &built_sessions,
        unplug_ts_ms,
        unplug_start_pct,
        unplug_source,
        current_charge_pct,
        on_battery_now,
        raw_log_available,
        earliest_sample_ms,
        events.first().map(|e| e.ts_ms),
        now_ms,
    );

    let mut sessions: Vec<SleepSession> = built_sessions.into_iter().map(|b| b.session).collect();
    sessions.reverse();
    sessions.truncate(20);

    let top_wake_reasons = sort_wake_reasons(all_batt_reason_agg);
    let since_unplug_wake_reasons = sort_wake_reasons(unplug_reason_agg);
    let timeline = build_timeline_buckets(&events, on_battery_now, current_charge_pct, now);

    SleepReport {
        since_unplug,
        sessions,
        top_wake_reasons,
        since_unplug_wake_reasons,
        timeline,
        raw_log_available,
    }
}

fn sort_wake_reasons(
    map: HashMap<String, (DecodedWakeReason, u32, u64)>,
) -> Vec<WakeReasonAggregate> {
    let mut out: Vec<WakeReasonAggregate> = map
        .into_values()
        .map(|(decoded, count, total_secs)| WakeReasonAggregate {
            decoded,
            count,
            total_secs,
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then(b.total_secs.cmp(&a.total_secs)));
    out.truncate(12);
    out
}

fn resolve_unplug_anchor(
    events: &[ParsedLogEvent],
    on_battery_now: bool,
    stored_unplug: Option<StoredUnplugTransition>,
) -> (Option<i64>, Option<u8>, String) {
    if !on_battery_now {
        return (None, None, "unknown".into());
    }

    // 1. Primary: SQLite continuous power-source transition (captures awake unplugs!).
    if let Some(st) = stored_unplug {
        if st.batt_ts_ms - st.ac_ts_ms <= 120_000 {
            return (Some(st.batt_ts_ms), st.start_charge_pct, "samples".into());
        }
        // Gap > 120s (sleep or app closed): refine inside [ac_ts_ms, batt_ts_ms] via pmset if present.
        if let Some(ev) = events
            .iter()
            .find(|e| e.ts_ms > st.ac_ts_ms && e.ts_ms <= st.batt_ts_ms && e.power_source != "AC")
        {
            return (
                Some(ev.ts_ms),
                ev.charge_pct.or(st.start_charge_pct),
                "pmset".into(),
            );
        }
        return (Some(st.batt_ts_ms), st.start_charge_pct, "samples".into());
    }

    // 2. Fallback: Walk `pmset` events (including `Assertions Summary-` Using AC / Using Batt) backward.
    for (idx, ev) in events.iter().enumerate().rev() {
        if ev.power_source == "AC" {
            if let Some(first_batt) = events[idx + 1..].iter().find(|e| e.power_source != "AC") {
                return (
                    Some(first_batt.ts_ms),
                    first_batt.charge_pct.or(ev.charge_pct),
                    "pmset".into(),
                );
            }
            break;
        }
    }

    (None, None, "unknown".into())
}

#[allow(clippy::too_many_arguments)]
fn compute_scoped_since_unplug(
    sessions: &[SessionWithIntervals],
    unplug_ts_ms: Option<i64>,
    unplug_start_pct: Option<u8>,
    unplug_source: String,
    current_charge_pct: Option<u8>,
    on_battery_now: bool,
    raw_log_available: bool,
    earliest_sample_ms: Option<i64>,
    earliest_log_ms: Option<i64>,
    now_ms: i64,
) -> SinceUnplugSummary {
    let earliest_known = match (earliest_sample_ms, earliest_log_ms) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let history_coverage_secs = earliest_known
        .map(|e| ((now_ms - e).max(0) / 1000) as u64)
        .unwrap_or(0);

    if !raw_log_available && earliest_sample_ms.is_none() {
        return SinceUnplugSummary {
            started_at: None,
            unplug_ts_ms: None,
            unplug_source: "unknown".into(),
            data_status: "log_unavailable".into(),
            history_coverage_secs: 0,
            elapsed_secs: 0,
            start_charge_pct: current_charge_pct,
            current_charge_pct,
            total_drop_pct: 0,
            asleep_drain_pct: 0,
            active_drain_pct: 0,
            sleep_session_count: 0,
            active_secs: 0,
            sleep_secs: 0,
            dark_wake_secs: 0,
            dark_wake_count: 0,
            drain_pct_per_hour: 0.0,
            on_battery_now,
        };
    }

    if !on_battery_now {
        return SinceUnplugSummary {
            started_at: None,
            unplug_ts_ms: None,
            unplug_source: "unknown".into(),
            data_status: "on_ac".into(),
            history_coverage_secs,
            elapsed_secs: 0,
            start_charge_pct: current_charge_pct,
            current_charge_pct,
            total_drop_pct: 0,
            asleep_drain_pct: 0,
            active_drain_pct: 0,
            sleep_session_count: 0,
            active_secs: 0,
            sleep_secs: 0,
            dark_wake_secs: 0,
            dark_wake_count: 0,
            drain_pct_per_hour: 0.0,
            on_battery_now: false,
        };
    }

    let Some(u_ms) = unplug_ts_ms else {
        return SinceUnplugSummary {
            started_at: None,
            unplug_ts_ms: None,
            unplug_source: "unknown".into(),
            data_status: "insufficient_history".into(),
            history_coverage_secs,
            elapsed_secs: history_coverage_secs,
            start_charge_pct: current_charge_pct,
            current_charge_pct,
            total_drop_pct: 0,
            asleep_drain_pct: 0,
            active_drain_pct: 0,
            sleep_session_count: 0,
            active_secs: history_coverage_secs,
            sleep_secs: 0,
            dark_wake_secs: 0,
            dark_wake_count: 0,
            drain_pct_per_hour: 0.0,
            on_battery_now: true,
        };
    };

    let elapsed_secs = ((now_ms - u_ms).max(0) / 1000) as u64;
    let started_at = chrono::DateTime::from_timestamp_millis(u_ms)
        .map(|utc| utc.with_timezone(&chrono::Local).format("%b %d %H:%M").to_string());

    let mut sleep_secs = 0u64;
    let mut dark_wake_secs = 0u64;
    let mut dark_wake_count = 0u32;
    let mut asleep_drain_raw = 0i32;
    let mut sleep_session_count = 0u32;

    for s in sessions {
        if s.session.end_ms <= u_ms || s.session.power_source == "AC" {
            continue;
        }
        sleep_session_count += 1;

        for &(a, b) in &s.sleep_intervals {
            let clipped = (b.min(now_ms) - a.max(u_ms)).max(0) / 1000;
            sleep_secs += clipped as u64;
        }
        for &(a, b) in &s.dark_wake_intervals {
            if b > u_ms {
                let clipped = (b.min(now_ms) - a.max(u_ms)).max(0) / 1000;
                dark_wake_secs += clipped as u64;
                dark_wake_count += 1;
            }
        }
        // Only attribute full session drain when session began at/after unplug_ts_ms.
        if s.session.start_ms >= u_ms {
            asleep_drain_raw += s.session.drain_pct.max(0);
        } else if let (Some(u_pct), Some(e_pct)) = (unplug_start_pct, s.session.end_charge_pct) {
            asleep_drain_raw += (u_pct as i32 - e_pct as i32).max(0);
        }
    }

    let active_secs = elapsed_secs.saturating_sub(sleep_secs + dark_wake_secs);
    let total_drop = match (unplug_start_pct, current_charge_pct) {
        (Some(s), Some(c)) => (s as i32 - c as i32).max(0),
        _ => 0,
    };
    let asleep_drain = asleep_drain_raw.clamp(0, total_drop);
    let active_drain = (total_drop - asleep_drain).max(0);
    let hours = (elapsed_secs as f64 / 3600.0).max(0.05);

    let data_status = if sleep_session_count == 0 {
        "no_battery_sleep"
    } else {
        "ok"
    };

    SinceUnplugSummary {
        started_at,
        unplug_ts_ms: Some(u_ms),
        unplug_source,
        data_status: data_status.into(),
        history_coverage_secs,
        elapsed_secs,
        start_charge_pct: unplug_start_pct,
        current_charge_pct,
        total_drop_pct: total_drop,
        asleep_drain_pct: asleep_drain,
        active_drain_pct: active_drain,
        sleep_session_count,
        active_secs,
        sleep_secs,
        dark_wake_secs,
        dark_wake_count,
        drain_pct_per_hour: ((total_drop as f64 / hours) * 10.0).round() / 10.0,
        on_battery_now: true,
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

fn build_timeline_buckets(
    events: &[ParsedLogEvent],
    on_battery_now: bool,
    current_charge_pct: Option<u8>,
    now: DateTime<FixedOffset>,
) -> Vec<TimelineBucket> {
    let target_buckets = 48usize;
    if events.is_empty() {
        return Vec::new();
    }

    let window_secs = 24 * 3600i64;
    let start = now - chrono::Duration::seconds(window_secs);
    let step_secs = window_secs / target_buckets as i64;

    let mut buckets = Vec::with_capacity(target_buckets);
    for i in 0..target_buckets {
        let b_start = start + chrono::Duration::seconds(step_secs * i as i64);
        let b_end = b_start + chrono::Duration::seconds(step_secs);
        let label = b_start.format("%H:%M").to_string();

        let mut dark_wakes = 0u32;
        let mut bucket_charge: Option<u8> = None;
        let mut had_sleep = false;
        let mut had_ac = false;
        let mut had_wake = false;
        let mut saw_any_before_or_in = false;
        let mut detail_reason = String::new();

        for ev in events {
            if ev.ts < b_start {
                saw_any_before_or_in = true;
                if ev.charge_pct.is_some() {
                    bucket_charge = ev.charge_pct;
                }
                if ev.kind == LogEventKind::Sleep {
                    had_sleep = true;
                } else if ev.kind == LogEventKind::Wake || ev.kind == LogEventKind::HibernateWake {
                    had_sleep = false;
                }
                had_ac = ev.power_source == "AC";
            } else if ev.ts >= b_start && ev.ts < b_end {
                saw_any_before_or_in = true;
                if ev.charge_pct.is_some() {
                    bucket_charge = ev.charge_pct;
                }
                if ev.power_source == "AC" {
                    had_ac = true;
                }
                match ev.kind {
                    LogEventKind::DarkWake => {
                        dark_wakes += 1;
                        if detail_reason.is_empty() {
                            detail_reason = decode_wake_reason(&ev.reason).label;
                        }
                    }
                    LogEventKind::Sleep => had_sleep = true,
                    LogEventKind::Wake | LogEventKind::HibernateWake => {
                        had_wake = true;
                        had_sleep = false;
                    }
                    LogEventKind::PowerMarker => {}
                }
            }
        }

        let (state, detail, est_watts) = if !saw_any_before_or_in && i < target_buckets - 1 {
            ("empty", "No logged data".to_string(), 0.0)
        } else if dark_wakes > 0 {
            (
                "darkwake",
                format!("{dark_wakes} silent wake(s) · {detail_reason}"),
                3.2,
            )
        } else if had_ac && (i < target_buckets - 1 || !on_battery_now) {
            ("charging", "On AC power".to_string(), 14.0)
        } else if had_sleep && !had_wake {
            ("sleep", "Asleep (Deep Idle)".to_string(), 0.3)
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
                current_charge_pct.or(bucket_charge)
            } else {
                bucket_charge
            },
            duration_secs: step_secs as u64,
            detail,
        });
    }

    buckets
}

fn extract_charge_pct(s: &str) -> Option<u8> {
    let idx = s.find("Charge:")?;
    let after = s[idx + "Charge:".len()..].trim_start();
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u8>().ok().map(|v| v.min(100))
}

fn extract_power_source(s: &str) -> String {
    if s.contains("Using AC") {
        "AC".into()
    } else {
        "BATT".into()
    }
}

fn extract_trailing_secs(s: &str) -> u64 {
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
        LogEventKind::PowerMarker => String::new(),
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

    if let Some(v) = batt.get("powernap") {
        if v == "1" {
            items.push(PmsetAuditItem {
                id: "batt-powernap".into(),
                profile: "Battery Power".into(),
                key: "powernap".into(),
                current_value: "1".into(),
                recommended_value: "0".into(),
                severity: "warning".into(),
                title: "Power Nap enabled on battery".into(),
                explanation: "Allows macOS to wake periodically during sleep to sync Mail, iCloud, and Time Machine on battery.".into(),
                fix_command: Some("sudo pmset -b powernap 0".into()),
            });
        }
    }

    if let Some(v) = batt.get("tcpkeepalive") {
        if v == "1" {
            items.push(PmsetAuditItem {
                id: "batt-tcpkeepalive".into(),
                profile: "Battery Power".into(),
                key: "tcpkeepalive".into(),
                current_value: "1".into(),
                recommended_value: "0".into(),
                severity: if dark_wake_count >= 5 { "warning" } else { "ok" }.into(),
                title: if dark_wake_count >= 5 {
                    format!("TCPKeepAlive active on battery ({dark_wake_count} silent wakes)")
                } else {
                    "TCPKeepAlive active on battery (Find My / Push)".into()
                },
                explanation: "Keeps Wi-Fi sockets alive during sleep; common cause of Wi-Fi/Bluetooth wake loops. Disabling pauses Find My while asleep.".into(),
                fix_command: Some("sudo pmset -b tcpkeepalive 0".into()),
            });
        }
    }

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
        } else if v == "3" {
            items.push(PmsetAuditItem {
                id: "batt-hibernate-ok".into(),
                profile: "Battery Power".into(),
                key: "hibernatemode".into(),
                current_value: "3".into(),
                recommended_value: "3".into(),
                severity: "ok".into(),
                title: "Safe Sleep (hibernatemode 3) active".into(),
                explanation: "RAM state is saved safely before deep sleep or 0% shutdown.".into(),
                fix_command: None,
            });
        }
    }

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
                explanation: "Mac stays in higher-draw normal sleep instead of dropping into deep standby overnight.".into(),
                fix_command: Some("sudo pmset -b standby 1".into()),
            });
        }
    }

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
    fn decodes_apple_silicon_wake_tokens_with_lid_and_pwrbtn_priority() {
        let d = decode_wake_reason("smc.sysState.Wake(0x70070000) wifibt SMC.OutboxNotEmpty/");
        assert_eq!(d.token, "wifibt");
        assert!(d.is_insomnia_suspect);

        let d2 = decode_wake_reason("NUB.SPMI0.SW3.IRQ nub-spmi0.0x02 rtc/Maintenance");
        assert_eq!(d2.token, "NUB.SPMI");
        assert!(d2.is_insomnia_suspect);

        // Lid and power button MUST win over generic smc.sysState!
        let d3 = decode_wake_reason("smc.sysState.Wake(0x70070000) lid SMC.OutboxNotEmpty/UserActivity");
        assert_eq!(d3.token, "lid");
        assert!(!d3.is_insomnia_suspect);

        let d4 = decode_wake_reason("smc.sysState.Wake(0x70070000) pwrbtn SMC.OutboxNotEmpty/UserActivity");
        assert_eq!(d4.token, "pwrbtn");
        assert!(!d4.is_insomnia_suspect);
    }

    #[test]
    fn isolates_sleep_attribution_to_current_unplug_window() {
        // Yesterday: 8% overnight drain. Today: unplugged at 100%, currently 98%, no sleep since unplug!
        let sample = "\
2026-11-21 19:02:48 +0530 Sleep               \tEntering Sleep state due to 'Idle Sleep':TCPKeepAlive=active Using Batt (Charge:50%) 3600 secs
2026-11-21 20:02:48 +0530 DarkWake            \tDarkWake from Deep Idle [CDNP] : due to smc.sysState.Wake(0x70070000) wifibt SMC.OutboxNotEmpty/ Using BATT (Charge:42%) 21 secs
2026-11-21 20:03:10 +0530 Wake                \tWake from Deep Idle [CDNVA] : due to smc.sysState.Wake(0x70070000) lid SMC.OutboxNotEmpty/UserActivity Assertion Using BATT (Charge:42%)
2026-11-22 06:54:47 +0530 Assertions          \tSummary- [System: PrevIdle] Using AC(Charge: 98)
2026-11-22 08:28:00 +0530 Assertions          \tSummary- [System: PrevIdle] Using Batt(Charge: 100)
";
        let report = parse_pmset_log(sample, Some(98), true);
        assert_eq!(report.since_unplug.total_drop_pct, 2);
        assert_eq!(report.since_unplug.asleep_drain_pct, 0);
        assert_eq!(report.since_unplug.active_drain_pct, 2);
        assert_eq!(report.since_unplug.sleep_session_count, 0);
        assert_eq!(report.since_unplug.data_status, "no_battery_sleep");
        assert_eq!(report.sessions.len(), 1);
        assert!(!report.sessions[0].in_current_unplug);
    }
}
