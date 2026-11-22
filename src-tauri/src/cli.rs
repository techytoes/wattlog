use clap::Parser;

use crate::telemetry::processes::format_bytes_short;
use crate::telemetry::types::{DashboardSnapshot, PowerState};
use crate::telemetry::TelemetryEngine;

#[derive(Debug, Parser)]
#[command(
    name = "wattlog",
    version,
    about = "Zero-root macOS Apple Silicon power, sleep-insomnia diagnostics, Keep-Awake & grouped process monitor"
)]
pub struct CliArgs {
    /// Use rich deterministic demo telemetry (works with --plain, --json, or GUI)
    #[arg(long)]
    pub demo: bool,

    /// Print a crisp plain-text monospace diagnostic report to stdout and exit (no GUI boot)
    #[arg(long)]
    pub plain: bool,

    /// Print full structured telemetry snapshot as JSON to stdout and exit (no GUI boot)
    #[arg(long)]
    pub json: bool,
}

pub fn run_cli_if_requested(args: &CliArgs) -> bool {
    if !args.plain && !args.json {
        return false;
    }

    let engine = TelemetryEngine::new(args.demo, false);
    if !args.demo {
        std::thread::sleep(std::time::Duration::from_millis(350));
    }
    let snapshot = engine.snapshot(true, true, true);

    if args.json {
        match serde_json::to_string_pretty(&snapshot) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("wattlog: failed to serialize json: {e}");
                std::process::exit(1);
            }
        }
    } else {
        print!("{}", format_plain_report(&snapshot));
    }
    true
}

pub fn format_plain_report(s: &DashboardSnapshot) -> String {
    let mut out = String::new();
    let mode_tag = if s.demo_mode { " [demo]" } else { "" };

    out.push_str(&format!(
        "wattlog  {} · {} · {}{}\n",
        s.host_name, s.chip_name, s.macos_version, mode_tag
    ));

    let state_str = match s.power.state {
        PowerState::Discharging => "discharging",
        PowerState::Charging => "charging",
        PowerState::Full => "charged",
        PowerState::AcOnly => "ac power",
    };

    let watts_str = s
        .power
        .system_watts
        .map(|w| format!("{w:.1}W"))
        .unwrap_or_else(|| "--.-W".into());

    if let Some(ref b) = s.power.battery {
        let eta = b
            .time_remaining_mins
            .map(|m| format!(" · {}h {:02}m", m / 60, m % 60))
            .unwrap_or_default();
        out.push_str(&format!(
            "{}  {}  {}%{}  ·  {:.0}% health ({} cycles)  ·  {:.1}°C  [{}]\n",
            watts_str,
            state_str,
            b.charge_pct,
            eta,
            b.health_pct,
            b.cycle_count,
            b.temperature_c,
            s.power.source_tier
        ));
    } else {
        out.push_str(&format!(
            "{}  {}  [{}]\n",
            watts_str, state_str, s.power.source_tier
        ));
    }

    if let Some(ref c) = s.power.components {
        let temp = c
            .cpu_temp_c
            .map(|t| format!(" · CPU {:.0}°C", t))
            .unwrap_or_default();
        out.push_str(&format!(
            "  CPU {:.2}W · GPU {:.2}W · ANE {:.2}W · DRAM {:.2}W · Display/Sys {:.2}W{}\n",
            c.cpu_watts, c.gpu_watts, c.ane_watts, c.dram_watts, c.display_other_watts, temp
        ));
    }

    out.push_str("\nsince last unplug & sleep post-mortem\n");
    let u = &s.sleep.since_unplug;
    if let Some(ref started) = u.started_at {
        out.push_str(&format!(
            "  unplugged {} ({}) · -{}% (active -{}%, asleep -{}%) · {} silent wakes ({})\n",
            started,
            u.unplug_source,
            u.total_drop_pct,
            u.active_drain_pct,
            u.asleep_drain_pct,
            u.dark_wake_count,
            fmt_dur(u.dark_wake_secs),
        ));
    } else {
        out.push_str(&format!("  status: {}\n", u.data_status));
    }

    for sess in s.sleep.sessions.iter().take(4) {
        let culprit = sess
            .worst_wake_reason
            .as_ref()
            .map(|w| format!(" · mostly {} ({})", w.token, w.label))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {} → {}  {:>6}  -{:>2}%  {:>2} wakes  [{}]{}\n",
            sess.start_time,
            sess.end_time,
            fmt_dur(sess.duration_secs),
            sess.drain_pct,
            sess.dark_wake_count,
            sess.severity,
            culprit,
        ));
    }

    let active_user_blockers: Vec<_> = s
        .blockers
        .iter()
        .filter(|b| !b.is_system_protected)
        .collect();
    out.push_str(&format!(
        "\nlive sleep blockers ({} user, {} total)\n",
        active_user_blockers.len(),
        s.blockers.len()
    ));
    if s.blockers.is_empty() {
        out.push_str("  none — Mac can sleep normally\n");
    } else {
        for b in s.blockers.iter().take(6) {
            out.push_str(&format!(
                "  {:<22} {:>7}  [{}]  {}\n    └ {} (pid {})\n",
                truncate(&b.app_name, 22),
                fmt_dur(b.duration_secs),
                b.severity,
                b.assertion_type,
                truncate(&b.assertion_name, 72),
                b.pid
            ));
        }
    }

    let mp = &s.processes.memory_pressure;
    out.push_str(&format!(
        "\ngrouped apps · memory pressure: {} (used {} / {}, swap {})\n",
        mp.level.to_uppercase(),
        format_bytes_short(mp.used_bytes),
        format_bytes_short(mp.total_bytes),
        format_bytes_short(mp.swap_used_bytes),
    ));
    for g in s.processes.groups.iter().take(8) {
        let helpers = if g.helper_count > 1 {
            format!(" ({} helpers)", g.helper_count)
        } else {
            String::new()
        };
        let label = format!("{}{}", g.name, helpers);
        let tag = if g.verdict == "normal" {
            String::new()
        } else {
            format!("  [{}]", g.verdict)
        };
        out.push_str(&format!(
            "  {:<32} {:>7} · {:>5.1}% CPU · EI {:>4.1}{}\n",
            truncate(&label, 32),
            format_bytes_short(g.footprint_bytes),
            g.cpu_pct,
            g.energy_impact,
            tag,
        ));
        if let Some(ref mostly) = g.mostly_in {
            out.push_str(&format!("    └ mostly in {mostly}\n"));
        }
    }

    let warn_audit: Vec<_> = s.audit.iter().filter(|a| a.severity != "ok").collect();
    out.push_str(&format!("\nsystem pmset audit ({} findings)\n", warn_audit.len()));
    if warn_audit.is_empty() {
        out.push_str("  all battery & sleep settings look clean\n");
    } else {
        for item in warn_audit {
            out.push_str(&format!(
                "  [{}] {} ({}={}) — {}\n",
                item.severity, item.title, item.key, item.current_value, item.explanation
            ));
            if let Some(ref cmd) = item.fix_command {
                out.push_str(&format!("    └ fix: {cmd}\n"));
            }
        }
    }

    out
}

fn fmt_dur(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
